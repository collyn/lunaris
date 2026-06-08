#include "gpu_video_item.h"
#include <QQuickWindow>
#include <QOpenGLContext>
#include <QOpenGLFunctions>
#include <QRunnable>
#include <QDebug>
#include <iostream>

#if defined(_WIN32)
#include <windows.h>
#else
#include <dlfcn.h>
#endif

// CUDA driver API types and structures
typedef unsigned long long CUdeviceptr;
typedef struct CUgraphicsResource_st* CUgraphicsResource;
typedef struct CUarray_st* CUarray;

enum CUmemorytype {
    CU_MEMORYTYPE_HOST = 1,
    CU_MEMORYTYPE_DEVICE = 2,
    CU_MEMORYTYPE_ARRAY = 3,
    CU_MEMORYTYPE_UNIFIED = 4
};

struct CUDA_MEMCPY2D {
    size_t srcXInBytes;
    size_t srcY;
    CUmemorytype srcMemoryType;
    const void* srcHost;
    CUdeviceptr srcDevice;
    CUarray srcArray;
    size_t srcPitch;

    size_t dstXInBytes;
    size_t dstY;
    CUmemorytype dstMemoryType;
    void* dstHost;
    CUdeviceptr dstDevice;
    CUarray dstArray;
    size_t dstPitch;

    size_t WidthInBytes;
    size_t Height;
};

// Function pointer definitions
typedef int (*t_cuInit)(unsigned int Flags);
typedef int (*t_cuGraphicsGLRegisterImage)(CUgraphicsResource* pCudaResource, unsigned int image, unsigned int target, unsigned int flags);
typedef int (*t_cuGraphicsUnregisterResource)(CUgraphicsResource resource);
typedef int (*t_cuGraphicsMapResources)(int count, CUgraphicsResource* resources, void* hStream);
typedef int (*t_cuGraphicsUnmapResources)(int count, CUgraphicsResource* resources, void* hStream);
typedef int (*t_cuGraphicsSubResourceGetMappedArray)(CUarray* pArray, CUgraphicsResource resource, unsigned int arrayIndex, unsigned int mipLevel);
typedef int (*t_cuMemcpy2D)(const CUDA_MEMCPY2D* pCopy);
typedef int (*t_cuDeviceGet)(int* device, int ordinal);
typedef int (*t_cuCtxCreate)(void** pctx, unsigned int flags, int dev);
typedef int (*t_cuCtxSetCurrent)(void* ctx);
typedef int (*t_cuCtxDestroy)(void* ctx);

struct CudaApi {
    void* handle = nullptr;
    t_cuInit cuInit = nullptr;
    t_cuGraphicsGLRegisterImage cuGraphicsGLRegisterImage = nullptr;
    t_cuGraphicsUnregisterResource cuGraphicsUnregisterResource = nullptr;
    t_cuGraphicsMapResources cuGraphicsMapResources = nullptr;
    t_cuGraphicsUnmapResources cuGraphicsUnmapResources = nullptr;
    t_cuGraphicsSubResourceGetMappedArray cuGraphicsSubResourceGetMappedArray = nullptr;
    t_cuMemcpy2D cuMemcpy2D = nullptr;
    t_cuDeviceGet cuDeviceGet = nullptr;
    t_cuCtxCreate cuCtxCreate = nullptr;
    t_cuCtxSetCurrent cuCtxSetCurrent = nullptr;
    t_cuCtxDestroy cuCtxDestroy = nullptr;

    bool load() {
#if defined(_WIN32)
        handle = (void*)LoadLibraryA("nvcuda.dll");
#else
        handle = dlopen("libcuda.so.1", RTLD_LAZY);
        if (!handle) handle = dlopen("libcuda.so", RTLD_LAZY);
#endif
        if (!handle) {
            std::cerr << "Lunaris: Dynamic load of libcuda failed." << std::endl;
            return false;
        }

#if defined(_WIN32)
        #define GET_PROC(name) name = (t_##name)GetProcAddress((HMODULE)handle, #name)
        #define GET_PROC_V2(name) \
            name = (t_##name)GetProcAddress((HMODULE)handle, #name "_v2"); \
            if (!name) name = (t_##name)GetProcAddress((HMODULE)handle, #name)
#else
        #define GET_PROC(name) name = (t_##name)dlsym(handle, #name)
        #define GET_PROC_V2(name) \
            name = (t_##name)dlsym(handle, #name "_v2"); \
            if (!name) name = (t_##name)dlsym(handle, #name)
#endif
        GET_PROC(cuInit);
        GET_PROC(cuGraphicsGLRegisterImage);
        GET_PROC(cuGraphicsUnregisterResource);
        GET_PROC(cuGraphicsMapResources);
        GET_PROC(cuGraphicsUnmapResources);
        GET_PROC(cuGraphicsSubResourceGetMappedArray);
        GET_PROC_V2(cuMemcpy2D);
        GET_PROC(cuDeviceGet);
        GET_PROC_V2(cuCtxCreate);
        GET_PROC(cuCtxSetCurrent);
        GET_PROC_V2(cuCtxDestroy);

        if (!cuInit || !cuGraphicsGLRegisterImage || !cuGraphicsUnregisterResource ||
            !cuGraphicsMapResources || !cuGraphicsUnmapResources ||
            !cuGraphicsSubResourceGetMappedArray || !cuMemcpy2D ||
            !cuDeviceGet || !cuCtxCreate || !cuCtxSetCurrent || !cuCtxDestroy) {
            std::cerr << "Lunaris: Failed to resolve all CUDA driver symbols." << std::endl;
            return false;
        }

        int ret = cuInit(0);
        if (ret != 0) {
            std::cerr << "Lunaris: cuInit failed with error: " << ret << std::endl;
            return false;
        }
        std::cerr << "Lunaris: Dynamically loaded and initialized CUDA driver API successfully." << std::endl;
        return true;
    }
};

static CudaApi g_cudaApi;
static bool g_cudaSupported = false;
static bool g_streamActive = false;

struct PendingCudaFrame {
    uint64_t cuda_ctx = 0;
    uint64_t y_ptr = 0;
    int y_stride = 0;
    uint64_t uv_ptr = 0;
    int uv_stride = 0;
    int width = 0;
    int height = 0;
    bool new_frame = false;
};

static PendingCudaFrame g_pendingFrame;
static QMutex g_frameMutex;
static GpuVideoItem* g_activeItem = nullptr;

// OpenGL Function Pointers for shader/VBO operations (resolved via QOpenGLContext)
static QOpenGLFunctions* gl = nullptr;

static void checkGlError(const char* op) {
    if (!gl) return;
    for (GLenum error = gl->glGetError(); error; error = gl->glGetError()) {
        std::cerr << "Lunaris: After " << op << " glError: 0x" << std::hex << error << std::dec << std::endl;
    }
}

// Shaders code
// Shaders code unused legacy version 120 removed

GpuVideoItem::GpuVideoItem(QQuickItem *parent) : QQuickItem(parent), m_cudaActive(false) {
    std::cerr << "Lunaris: GpuVideoItem constructor called." << std::endl;
    g_activeItem = this;
    setFlag(ItemHasContents, true);
    connect(this, &QQuickItem::windowChanged, this, &GpuVideoItem::handleWindowChanged);

    // Try loading CUDA driver API on construction
    static bool checkedCuda = false;
    if (!checkedCuda) {
        if (qEnvironmentVariableIsSet("LUNARIS_DISABLE_CUDA")) {
            std::cerr << "Lunaris: CUDA is disabled via environment variable LUNARIS_DISABLE_CUDA." << std::endl;
            g_cudaSupported = false;
        } else {
            g_cudaSupported = g_cudaApi.load();
        }
        checkedCuda = true;
    }
}

GpuVideoItem::~GpuVideoItem() {
    g_activeItem = nullptr;
    cleanupCudaGL(true);
}

bool GpuVideoItem::cudaSupported() const {
    return g_cudaSupported;
}

bool GpuVideoItem::cudaActive() const {
    return m_cudaActive;
}

void GpuVideoItem::setCudaActive(bool active) {
    QMutexLocker locker(&m_mutex);
    if (m_cudaActive != active) {
        m_cudaActive = active;
        emit cudaActiveChanged();
    }
}

void GpuVideoItem::registerTypes() {
    qmlRegisterType<GpuVideoItem>("com.lunaris.client.gpu", 1, 0, "GpuVideoItem");
    std::cerr << "Lunaris: Registered GpuVideoItem type." << std::endl;
}

void GpuVideoItem::handleWindowChanged(QQuickWindow *win) {
    if (win) {
        connect(win, &QQuickWindow::beforeRenderPassRecording, this, &GpuVideoItem::renderNative, Qt::DirectConnection);
    }
}

static GLuint compileShader(GLenum type, const char* source) {
    GLuint shader = gl->glCreateShader(type);
    gl->glShaderSource(shader, 1, &source, nullptr);
    gl->glCompileShader(shader);
    
    GLint compiled = 0;
    gl->glGetShaderiv(shader, GL_COMPILE_STATUS, &compiled);
    if (!compiled) {
        GLint infoLen = 0;
        gl->glGetShaderiv(shader, GL_INFO_LOG_LENGTH, &infoLen);
        if (infoLen > 1) {
            char* infoLog = new char[infoLen];
            gl->glGetShaderInfoLog(shader, infoLen, nullptr, infoLog);
            std::cerr << "Lunaris: Shader compile error: " << infoLog << std::endl;
            delete[] infoLog;
        }
        gl->glDeleteShader(shader);
        return 0;
    }
    return shader;
}

void GpuVideoItem::initCudaGL() {
    QOpenGLContext* currentContext = QOpenGLContext::currentContext();
    if (!currentContext) {
        std::cerr << "Lunaris: CUDA-GL renderer unavailable: no current OpenGL context." << std::endl;
        setCudaActive(false);
        return;
    }

    if (!gl) {
        gl = currentContext->functions();
        if (!gl) {
            std::cerr << "Lunaris: CUDA-GL renderer unavailable: no OpenGL functions." << std::endl;
            setCudaActive(false);
            return;
        }
        gl->initializeOpenGLFunctions();
    }

    const char* glVersion = (const char*)gl->glGetString(GL_VERSION);
    std::cerr << "Lunaris: OpenGL Version: " << (glVersion ? glVersion : "Unknown") << std::endl;

    // Define shader variations
    const char* vsCore =
        "#version 130\n"
        "in vec2 position;\n"
        "out vec2 texCoord;\n"
        "void main() {\n"
        "    gl_Position = vec4(position, 0.0, 1.0);\n"
        "    texCoord = (position + 1.0) * 0.5;\n"
        "    texCoord.y = 1.0 - texCoord.y;\n"
        "}\n";

    const char* fsCore =
        "#version 130\n"
        "out vec4 fragColor;\n"
        "uniform sampler2D yTexture;\n"
        "uniform sampler2D uvTexture;\n"
        "in vec2 texCoord;\n"
        "void main() {\n"
        "    float y = texture(yTexture, texCoord).r;\n"
        "    vec2 uv = texture(uvTexture, texCoord).rg;\n"
        "    y = y - (16.0 / 255.0);\n"
        "    float cb = uv.r - (128.0 / 255.0);\n"
        "    float cr = uv.g - (128.0 / 255.0);\n"
        "    float r = 1.164 * y + 1.793 * cr;\n"
        "    float g = 1.164 * y - 0.534 * cr - 0.213 * cb;\n"
        "    float b = 1.164 * y + 2.115 * cb;\n"
        "    fragColor = vec4(clamp(vec3(r, g, b), 0.0, 1.0), 1.0);\n"
        "}\n";

    const char* vsLegacy =
        "attribute vec2 position;\n"
        "varying vec2 texCoord;\n"
        "void main() {\n"
        "    gl_Position = vec4(position, 0.0, 1.0);\n"
        "    texCoord = (position + 1.0) * 0.5;\n"
        "    texCoord.y = 1.0 - texCoord.y;\n"
        "}\n";

    const char* fsLegacy =
        "uniform sampler2D yTexture;\n"
        "uniform sampler2D uvTexture;\n"
        "varying vec2 texCoord;\n"
        "void main() {\n"
        "    float y = texture2D(yTexture, texCoord).r;\n"
        "    vec2 uv = texture2D(uvTexture, texCoord).rg;\n"
        "    y = y - (16.0 / 255.0);\n"
        "    float cb = uv.r - (128.0 / 255.0);\n"
        "    float cr = uv.g - (128.0 / 255.0);\n"
        "    float r = 1.164 * y + 1.793 * cr;\n"
        "    float g = 1.164 * y - 0.534 * cr - 0.213 * cb;\n"
        "    float b = 1.164 * y + 2.115 * cb;\n"
        "    gl_FragColor = vec4(clamp(vec3(r, g, b), 0.0, 1.0), 1.0);\n"
        "}\n";

    const char* fsES =
        "precision mediump float;\n"
        "uniform sampler2D yTexture;\n"
        "uniform sampler2D uvTexture;\n"
        "varying vec2 texCoord;\n"
        "void main() {\n"
        "    float y = texture2D(yTexture, texCoord).r;\n"
        "    vec2 uv = texture2D(uvTexture, texCoord).rg;\n"
        "    y = y - (16.0 / 255.0);\n"
        "    float cb = uv.r - (128.0 / 255.0);\n"
        "    float cr = uv.g - (128.0 / 255.0);\n"
        "    float r = 1.164 * y + 1.793 * cr;\n"
        "    float g = 1.164 * y - 0.534 * cr - 0.213 * cb;\n"
        "    float b = 1.164 * y + 2.115 * cb;\n"
        "    gl_FragColor = vec4(clamp(vec3(r, g, b), 0.0, 1.0), 1.0);\n"
        "}\n";

    // Try Core first, then Legacy, then ES
    GLuint vs = compileShader(GL_VERTEX_SHADER, vsCore);
    GLuint fs = 0;
    if (vs) {
        fs = compileShader(GL_FRAGMENT_SHADER, fsCore);
        if (!fs) {
            gl->glDeleteShader(vs);
            vs = 0;
        }
    }

    if (!vs) {
        std::cerr << "Lunaris: Falling back to Legacy/ES shaders..." << std::endl;
        vs = compileShader(GL_VERTEX_SHADER, vsLegacy);
        if (vs) {
            fs = compileShader(GL_FRAGMENT_SHADER, fsLegacy);
            if (!fs) {
                // Try ES fragment shader
                fs = compileShader(GL_FRAGMENT_SHADER, fsES);
            }
            if (!fs) {
                gl->glDeleteShader(vs);
                vs = 0;
            }
        }
    }

    if (!vs || !fs) {
        std::cerr << "Lunaris: Failed to compile shaders." << std::endl;
        return;
    }

    m_program = gl->glCreateProgram();
    gl->glAttachShader(m_program, vs);
    gl->glAttachShader(m_program, fs);
    gl->glLinkProgram(m_program);

    GLint linked = 0;
    gl->glGetProgramiv(m_program, GL_LINK_STATUS, &linked);
    if (!linked) {
        GLint infoLen = 0;
        gl->glGetProgramiv(m_program, GL_INFO_LOG_LENGTH, &infoLen);
        if (infoLen > 1) {
            char* infoLog = new char[infoLen];
            gl->glGetProgramInfoLog(m_program, infoLen, nullptr, infoLog);
            std::cerr << "Lunaris: Shader program link error: " << infoLog << std::endl;
            delete[] infoLog;
        }
        gl->glDeleteProgram(m_program);
        m_program = 0;
    }

    gl->glDeleteShader(vs);
    gl->glDeleteShader(fs);

    // Create quad vertices
    float vertices[] = {
        -1.0f, -1.0f,
         1.0f, -1.0f,
        -1.0f,  1.0f,
        -1.0f,  1.0f,
         1.0f, -1.0f,
         1.0f,  1.0f
    };
    gl->glGenBuffers(1, &m_vbo);
    gl->glBindBuffer(GL_ARRAY_BUFFER, m_vbo);
    gl->glBufferData(GL_ARRAY_BUFFER, sizeof(vertices), vertices, GL_STATIC_DRAW);

    m_vao = new QOpenGLVertexArrayObject();
    m_vao->create();
    m_vao->bind();
    gl->glBindBuffer(GL_ARRAY_BUFFER, m_vbo);
    GLint posAttr = gl->glGetAttribLocation(m_program, "position");
    if (posAttr != -1) {
        gl->glEnableVertexAttribArray(posAttr);
        gl->glVertexAttribPointer(posAttr, 2, GL_FLOAT, GL_FALSE, 0, nullptr);
    } else {
        std::cerr << "Lunaris: Warning: attribute 'position' not found!" << std::endl;
    }
    m_vao->release();
    gl->glBindBuffer(GL_ARRAY_BUFFER, 0);

    m_cudaInitialized = true;
    std::cerr << "Lunaris: Shaders, VBO, and VAO initialized successfully." << std::endl;
}

class CudaGLCleanupJob : public QRunnable {
public:
    void* cudaContext;
    void* cudaYRes;
    void* cudaUvRes;
    unsigned int yTexture;
    unsigned int uvTexture;
    unsigned int program;
    unsigned int vbo;
    QOpenGLVertexArrayObject* vao;
    bool skipCuda;

    CudaGLCleanupJob(void* ctx, void* yRes, void* uvRes, unsigned int yTex, unsigned int uvTex, unsigned int prog, unsigned int vb, QOpenGLVertexArrayObject* v, bool skip)
        : cudaContext(ctx), cudaYRes(yRes), cudaUvRes(uvRes), yTexture(yTex), uvTexture(uvTex), program(prog), vbo(vb), vao(v), skipCuda(skip) {
        setAutoDelete(true);
    }

    void run() override {
        if (g_cudaSupported && !skipCuda && cudaContext) {
            g_cudaApi.cuCtxSetCurrent(cudaContext);
        }
        if (cudaYRes && g_cudaSupported && !skipCuda) {
            g_cudaApi.cuGraphicsUnregisterResource((CUgraphicsResource)cudaYRes);
        }
        if (cudaUvRes && g_cudaSupported && !skipCuda) {
            g_cudaApi.cuGraphicsUnregisterResource((CUgraphicsResource)cudaUvRes);
        }
        if (g_cudaSupported && !skipCuda && cudaContext) {
            g_cudaApi.cuCtxSetCurrent(nullptr);
        }

        if (gl && QOpenGLContext::currentContext()) {
            if (vao) {
                vao->destroy();
                delete vao;
            }
            if (yTexture) gl->glDeleteTextures(1, &yTexture);
            if (uvTexture) gl->glDeleteTextures(1, &uvTexture);
            if (program) gl->glDeleteProgram(program);
            if (vbo) gl->glDeleteBuffers(1, &vbo);
        } else {
            if (vao) delete vao;
        }
    }
};

void GpuVideoItem::cleanupCudaGL(bool skipCuda) {
    QMutexLocker frameLocker(&g_frameMutex);
    QMutexLocker locker(&m_mutex);
    if (!m_texturesInitialized && !m_cudaInitialized) return;

    CudaGLCleanupJob* job = new CudaGLCleanupJob(
        m_cudaContext,
        m_cudaYRes,
        m_cudaUvRes,
        m_yTexture,
        m_uvTexture,
        m_program,
        m_vbo,
        m_vao,
        skipCuda
    );

    m_cudaContext = nullptr;
    m_cudaYRes = nullptr;
    m_cudaUvRes = nullptr;
    m_yTexture = 0;
    m_uvTexture = 0;
    m_program = 0;
    m_vbo = 0;
    m_vao = nullptr;

    m_texturesInitialized = false;
    m_cudaInitialized = false;
    m_videoWidth = 0;
    m_videoHeight = 0;

    if (window()) {
        window()->scheduleRenderJob(job, QQuickWindow::BeforeSynchronizingStage);
    } else {
        job->run();
    }
}

void GpuVideoItem::renderNative() {
    if (!g_cudaSupported) return;
    if (!QOpenGLContext::currentContext()) {
        if (m_cudaActive) setCudaActive(false);
        return;
    }

    QMutexLocker frameLocker(&g_frameMutex);
    QMutexLocker locker(&m_mutex);
    if (!g_pendingFrame.new_frame && !m_texturesInitialized) return;

    if (g_pendingFrame.new_frame) {
        m_cudaContext = (void*)g_pendingFrame.cuda_ctx;
    }

    if (!m_cudaInitialized) {
        initCudaGL();
    }

    if (!m_program) {
        // Shaders failed to compile/link, cannot render
        return;
    }

    if (!m_cudaContext) {
        return;
    }
    int ctx_ret = g_cudaApi.cuCtxSetCurrent(m_cudaContext);
    if (ctx_ret != 0) {
        std::cerr << "Lunaris: cuCtxSetCurrent failed: " << ctx_ret << " context: " << m_cudaContext << std::endl;
    }

    int width = g_pendingFrame.width;
    int height = g_pendingFrame.height;

    // Initialize or resize textures if video dimensions changed
    if (width != m_videoWidth || height != m_videoHeight) {
        if (m_cudaYRes) {
            g_cudaApi.cuGraphicsUnregisterResource((CUgraphicsResource)m_cudaYRes);
            m_cudaYRes = nullptr;
        }
        if (m_cudaUvRes) {
            g_cudaApi.cuGraphicsUnregisterResource((CUgraphicsResource)m_cudaUvRes);
            m_cudaUvRes = nullptr;
        }

        if (!m_yTexture) gl->glGenTextures(1, &m_yTexture);
        gl->glBindTexture(GL_TEXTURE_2D, m_yTexture);
        gl->glTexImage2D(GL_TEXTURE_2D, 0, GL_RED, width, height, 0, GL_RED, GL_UNSIGNED_BYTE, nullptr);
        gl->glTexParameteri(GL_TEXTURE_2D, GL_TEXTURE_MIN_FILTER, GL_LINEAR);
        gl->glTexParameteri(GL_TEXTURE_2D, GL_TEXTURE_MAG_FILTER, GL_LINEAR);
        gl->glTexParameteri(GL_TEXTURE_2D, GL_TEXTURE_WRAP_S, GL_CLAMP_TO_EDGE);
        gl->glTexParameteri(GL_TEXTURE_2D, GL_TEXTURE_WRAP_T, GL_CLAMP_TO_EDGE);

        if (!m_uvTexture) gl->glGenTextures(1, &m_uvTexture);
        gl->glBindTexture(GL_TEXTURE_2D, m_uvTexture);
        gl->glTexImage2D(GL_TEXTURE_2D, 0, GL_RG, width / 2, height / 2, 0, GL_RG, GL_UNSIGNED_BYTE, nullptr);
        gl->glTexParameteri(GL_TEXTURE_2D, GL_TEXTURE_MIN_FILTER, GL_LINEAR);
        gl->glTexParameteri(GL_TEXTURE_2D, GL_TEXTURE_MAG_FILTER, GL_LINEAR);
        gl->glTexParameteri(GL_TEXTURE_2D, GL_TEXTURE_WRAP_S, GL_CLAMP_TO_EDGE);
        gl->glTexParameteri(GL_TEXTURE_2D, GL_TEXTURE_WRAP_T, GL_CLAMP_TO_EDGE);

        // Register textures with CUDA
        int ret = g_cudaApi.cuGraphicsGLRegisterImage((CUgraphicsResource*)&m_cudaYRes, m_yTexture, GL_TEXTURE_2D, 2 /* CU_GRAPHICS_REGISTER_FLAGS_WRITE_DISCARD */);
        std::cerr << "Lunaris: Registered Y texture: ID=" << m_yTexture << " Resource=" << m_cudaYRes << " ret=" << ret << std::endl;
        if (ret != 0) {
            return;
        }
        ret = g_cudaApi.cuGraphicsGLRegisterImage((CUgraphicsResource*)&m_cudaUvRes, m_uvTexture, GL_TEXTURE_2D, 2 /* CU_GRAPHICS_REGISTER_FLAGS_WRITE_DISCARD */);
        std::cerr << "Lunaris: Registered UV texture: ID=" << m_uvTexture << " Resource=" << m_cudaUvRes << " ret=" << ret << std::endl;
        if (ret != 0) {
            return;
        }

        m_videoWidth = width;
        m_videoHeight = height;
        m_texturesInitialized = true;
    }

    // Perform CUDA to GL copy if there is a new frame
    if (g_pendingFrame.new_frame) {
        CUgraphicsResource resources[2] = { (CUgraphicsResource)m_cudaYRes, (CUgraphicsResource)m_cudaUvRes };
        int ret = g_cudaApi.cuGraphicsMapResources(2, resources, nullptr);
        if (ret == 0) {
            CUarray yArray = nullptr, uvArray = nullptr;
            int r1 = g_cudaApi.cuGraphicsSubResourceGetMappedArray(&yArray, resources[0], 0, 0);
            int r2 = g_cudaApi.cuGraphicsSubResourceGetMappedArray(&uvArray, resources[1], 0, 0);
            if (r1 != 0 || r2 != 0) {
                std::cerr << "Lunaris: cuGraphicsSubResourceGetMappedArray failed: Y=" << r1 << " UV=" << r2 << std::endl;
            }

            CUDA_MEMCPY2D copyY;
            std::memset(&copyY, 0, sizeof(copyY));
            copyY.srcMemoryType = CU_MEMORYTYPE_DEVICE;
            copyY.srcDevice = (CUdeviceptr)g_pendingFrame.y_ptr;
            copyY.srcPitch = g_pendingFrame.y_stride;
            copyY.dstMemoryType = CU_MEMORYTYPE_ARRAY;
            copyY.dstArray = yArray;
            copyY.WidthInBytes = width;
            copyY.Height = height;
            int r3 = g_cudaApi.cuMemcpy2D(&copyY);

            CUDA_MEMCPY2D copyUv;
            std::memset(&copyUv, 0, sizeof(copyUv));
            copyUv.srcMemoryType = CU_MEMORYTYPE_DEVICE;
            copyUv.srcDevice = (CUdeviceptr)g_pendingFrame.uv_ptr;
            copyUv.srcPitch = g_pendingFrame.uv_stride;
            copyUv.dstMemoryType = CU_MEMORYTYPE_ARRAY;
            copyUv.dstArray = uvArray;
            copyUv.WidthInBytes = width;
            copyUv.Height = height / 2;
            int r4 = g_cudaApi.cuMemcpy2D(&copyUv);

            if (r3 != 0 || r4 != 0) {
                std::cerr << "Lunaris: cuMemcpy2D failed: Y=" << r3 << " UV=" << r4 << std::endl;
            }

            g_cudaApi.cuGraphicsUnmapResources(2, resources, nullptr);
        } else {
            std::cerr << "Lunaris: cuGraphicsMapResources failed: " << ret << std::endl;
        }
        g_pendingFrame.new_frame = false;
    }

    // Calculate viewport based on item geometry in the QML scene
    QPointF localPos(0, 0);
    QPointF globalPos = mapToScene(localPos);
    qreal dpr = window()->devicePixelRatio();
    int x = globalPos.x() * dpr;
    // QML coordinates have Y starting from top, OpenGL viewport expects Y from bottom
    int y = (window()->height() - globalPos.y() - this->height()) * dpr;
    int w = this->width() * dpr;
    int h = this->height() * dpr;

    static int last_w = 0, last_h = 0;
    if (w != last_w || h != last_h) {
        std::cerr << "Lunaris: Viewport updated: x=" << x << " y=" << y << " w=" << w << " h=" << h 
                  << " Video dimensions: " << width << "x" << height << " dpr=" << dpr << std::endl;
        last_w = w;
        last_h = h;
    }

    window()->beginExternalCommands();

    // Configure OpenGL state for custom drawing
    gl->glDisable(GL_DEPTH_TEST);
    gl->glDisable(GL_CULL_FACE);
    gl->glDisable(GL_SCISSOR_TEST);
    gl->glDisable(GL_STENCIL_TEST);
    gl->glDisable(GL_BLEND);
    gl->glColorMask(GL_TRUE, GL_TRUE, GL_TRUE, GL_TRUE);
    gl->glDepthMask(GL_FALSE);

    // Setup OpenGL pipeline and render the quad
    gl->glViewport(x, y, w, h);
    gl->glUseProgram(m_program);

    gl->glActiveTexture(GL_TEXTURE0);
    gl->glBindTexture(GL_TEXTURE_2D, m_yTexture);
    gl->glUniform1i(gl->glGetUniformLocation(m_program, "yTexture"), 0);

    gl->glActiveTexture(GL_TEXTURE1);
    gl->glBindTexture(GL_TEXTURE_2D, m_uvTexture);
    gl->glUniform1i(gl->glGetUniformLocation(m_program, "uvTexture"), 1);

    if (m_vao) m_vao->bind();
    gl->glDrawArrays(GL_TRIANGLES, 0, 6);
    if (m_vao) m_vao->release();

    gl->glUseProgram(0);

    checkGlError("renderNative drawing");

    window()->endExternalCommands();

    g_cudaApi.cuCtxSetCurrent(nullptr);
}

QSGNode* GpuVideoItem::updatePaintNode(QSGNode *oldNode, UpdatePaintNodeData *) {
    // Return empty node tree because we render directly in beforeRendering()
    // This allows us to keep the QQuickItem aligned with QML coordinates
    // but execute the raw GL draw calls inside renderNative().
    return oldNode;
}

// C-linkage FFI implementation called by Rust
extern "C" void deliver_cuda_frame(uint64_t cuda_ctx, uint64_t y_ptr, int y_stride, uint64_t uv_ptr, int uv_stride, int width, int height) {
    QMutexLocker locker(&g_frameMutex);
    g_pendingFrame.cuda_ctx = cuda_ctx;
    g_pendingFrame.y_ptr = y_ptr;
    g_pendingFrame.y_stride = y_stride;
    g_pendingFrame.uv_ptr = uv_ptr;
    g_pendingFrame.uv_stride = uv_stride;
    g_pendingFrame.width = width;
    g_pendingFrame.height = height;
    g_pendingFrame.new_frame = true;

    if (g_activeItem) {
        // Set cudaActive to true thread-safely
        QMetaObject::invokeMethod(g_activeItem, "setCudaActive", Qt::QueuedConnection, Q_ARG(bool, true));
        // Trigger a redraw of the QQuickItem
        QMetaObject::invokeMethod(g_activeItem, "update", Qt::QueuedConnection);
    }
}

extern "C" void register_gpu_video_item_type() {
    GpuVideoItem::registerTypes();
}

extern "C" void set_cuda_stream_active(bool active) {
    g_streamActive = active;
    if (active) {
        if (g_activeItem) {
            QMetaObject::invokeMethod(g_activeItem, "setCudaActive", Qt::QueuedConnection, Q_ARG(bool, false));
        }
    } else {
        {
            QMutexLocker locker(&g_frameMutex);
            g_pendingFrame.cuda_ctx = 0;
            g_pendingFrame.y_ptr = 0;
            g_pendingFrame.uv_ptr = 0;
            g_pendingFrame.new_frame = false;
        }

        if (g_activeItem) {
            g_activeItem->cleanupCudaGL(true);
            QMetaObject::invokeMethod(g_activeItem, "setCudaActive", Qt::QueuedConnection, Q_ARG(bool, false));
        }
    }
}
