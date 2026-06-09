#include "vaapi_dmabuf_video_item.h"
#include "d3d11_video_item.h"

#include <QOpenGLContext>
#include <QOpenGLFunctions>
#include <QQuickWindow>
#include <QSGNode>
#include <QVector>
#include <QtQml/qqml.h>
#include <atomic>
#include <cerrno>
#include <cstring>
#include <iostream>

#if defined(Q_OS_LINUX)
#include <EGL/egl.h>
#include <EGL/eglext.h>
#include <unistd.h>
#endif

namespace {
constexpr uint32_t DRM_FORMAT_NV12 = 0x3231564e; // 'NV12'

struct PendingDmabufFrame {
    int fd0 = -1;
    int fd1 = -1;
    uint32_t fourcc = 0;
    uint64_t modifier = 0;
    int offset0 = 0;
    int pitch0 = 0;
    int offset1 = 0;
    int pitch1 = 0;
    int width = 0;
    int height = 0;
    bool newFrame = false;
};

PendingDmabufFrame g_pendingFrame;
QMutex g_frameMutex;
VaapiDmabufVideoItem *g_activeItem = nullptr;
std::atomic_bool g_dmabufRenderFailed{false};
bool g_dmabufStreamActive = false;
QOpenGLFunctions *g_gl = nullptr;

#if defined(Q_OS_LINUX)
PFNEGLCREATEIMAGEKHRPROC eglCreateImageKHRFn = nullptr;
PFNEGLDESTROYIMAGEKHRPROC eglDestroyImageKHRFn = nullptr;
using GlEglImageTargetTexture2DOESProc = void (*)(GLenum target, void *image);
GlEglImageTargetTexture2DOESProc glEGLImageTargetTexture2DOESFn = nullptr;
#endif

void closeFd(int &fd) {
#if defined(Q_OS_LINUX)
    if (fd >= 0) {
        ::close(fd);
        fd = -1;
    }
#else
    fd = -1;
#endif
}

void closeFrameFds(PendingDmabufFrame &frame) {
    closeFd(frame.fd0);
    closeFd(frame.fd1);
}

void markDmabufFailed(const char *reason) {
    g_dmabufRenderFailed.store(true, std::memory_order_release);
    std::cerr << "Lunaris: DMABUF renderer disabled: " << reason << std::endl;
    if (g_activeItem) {
        QMetaObject::invokeMethod(g_activeItem, "setDmabufActive", Qt::QueuedConnection, Q_ARG(bool, false));
    }
}

#if defined(Q_OS_LINUX)
void appendModifierAttrs(QVector<EGLint> &attrs, int plane, uint64_t modifier) {
#ifdef EGL_DMA_BUF_PLANE0_MODIFIER_LO_EXT
    if (modifier == 0 || modifier == UINT64_MAX) {
        return;
    }
    const EGLint loAttrs[] = {
        EGL_DMA_BUF_PLANE0_MODIFIER_LO_EXT,
        EGL_DMA_BUF_PLANE1_MODIFIER_LO_EXT,
        EGL_DMA_BUF_PLANE2_MODIFIER_LO_EXT,
        EGL_DMA_BUF_PLANE3_MODIFIER_LO_EXT,
    };
    const EGLint hiAttrs[] = {
        EGL_DMA_BUF_PLANE0_MODIFIER_HI_EXT,
        EGL_DMA_BUF_PLANE1_MODIFIER_HI_EXT,
        EGL_DMA_BUF_PLANE2_MODIFIER_HI_EXT,
        EGL_DMA_BUF_PLANE3_MODIFIER_HI_EXT,
    };
    if (plane >= 0 && plane < 4) {
        attrs.append(loAttrs[plane]);
        attrs.append(static_cast<EGLint>(modifier & 0xffffffffu));
        attrs.append(hiAttrs[plane]);
        attrs.append(static_cast<EGLint>((modifier >> 32) & 0xffffffffu));
    }
#endif
}

EGLImageKHR createPlaneImage(EGLDisplay display,
                             int fd,
                             int width,
                             int height,
                             uint32_t fourcc,
                             int offset,
                             int pitch,
                             uint64_t modifier) {
    QVector<EGLint> attrs;
    attrs << EGL_WIDTH << width
          << EGL_HEIGHT << height
          << EGL_LINUX_DRM_FOURCC_EXT << static_cast<EGLint>(fourcc)
          << EGL_DMA_BUF_PLANE0_FD_EXT << fd
          << EGL_DMA_BUF_PLANE0_OFFSET_EXT << offset
          << EGL_DMA_BUF_PLANE0_PITCH_EXT << pitch;
    appendModifierAttrs(attrs, 0, modifier);
    attrs << EGL_NONE;
    return eglCreateImageKHRFn(display, EGL_NO_CONTEXT, EGL_LINUX_DMA_BUF_EXT, nullptr, attrs.constData());
}
#endif
}

VaapiDmabufVideoItem::VaapiDmabufVideoItem(QQuickItem *parent) : QQuickItem(parent) {
    g_activeItem = this;
    setFlag(ItemHasContents, true);
    connect(this, &QQuickItem::windowChanged, this, &VaapiDmabufVideoItem::handleWindowChanged);
}

VaapiDmabufVideoItem::~VaapiDmabufVideoItem() {
    g_activeItem = nullptr;
    QMutexLocker locker(&m_mutex);
    cleanupGlResources();
}

bool VaapiDmabufVideoItem::dmabufSupported() const {
#if defined(Q_OS_LINUX)
    return true;
#else
    return false;
#endif
}

bool VaapiDmabufVideoItem::dmabufActive() const {
    return m_dmabufActive;
}

void VaapiDmabufVideoItem::setDmabufActive(bool active) {
    if (m_dmabufActive == active) {
        return;
    }
    m_dmabufActive = active;
    emit dmabufActiveChanged();
    update();
}

void VaapiDmabufVideoItem::registerTypes() {
    qmlRegisterType<VaapiDmabufVideoItem>("com.lunaris.client.gpu", 1, 0, "VaapiDmabufVideoItem");
}

void VaapiDmabufVideoItem::handleWindowChanged(QQuickWindow *win) {
    if (win) {
        connect(win, &QQuickWindow::beforeRenderPassRecording, this, &VaapiDmabufVideoItem::renderNative, Qt::DirectConnection);
    }
}

QSGNode *VaapiDmabufVideoItem::updatePaintNode(QSGNode *oldNode, UpdatePaintNodeData *) {
    delete oldNode;
    return nullptr;
}

bool VaapiDmabufVideoItem::ensureGlResources() {
#if defined(Q_OS_LINUX)
    if (!window()) {
        return false;
    }
    QOpenGLContext *ctx = QOpenGLContext::currentContext();
    if (!ctx) {
        markDmabufFailed("no current OpenGL context");
        return false;
    }
    if (!g_gl) {
        g_gl = ctx->functions();
        g_gl->initializeOpenGLFunctions();
    }
    if (!eglCreateImageKHRFn) {
        eglCreateImageKHRFn = reinterpret_cast<PFNEGLCREATEIMAGEKHRPROC>(eglGetProcAddress("eglCreateImageKHR"));
        eglDestroyImageKHRFn = reinterpret_cast<PFNEGLDESTROYIMAGEKHRPROC>(eglGetProcAddress("eglDestroyImageKHR"));
        glEGLImageTargetTexture2DOESFn = reinterpret_cast<GlEglImageTargetTexture2DOESProc>(eglGetProcAddress("glEGLImageTargetTexture2DOES"));
        if (!glEGLImageTargetTexture2DOESFn) {
            glEGLImageTargetTexture2DOESFn = reinterpret_cast<GlEglImageTargetTexture2DOESProc>(ctx->getProcAddress("glEGLImageTargetTexture2DOES"));
        }
        if (!eglCreateImageKHRFn || !eglDestroyImageKHRFn || !glEGLImageTargetTexture2DOESFn) {
            markDmabufFailed("missing EGL DMABUF import functions");
            return false;
        }
    }
    if (!m_texturesInitialized) {
        g_gl->glGenTextures(1, &m_yTexture);
        g_gl->glGenTextures(1, &m_uvTexture);
        g_gl->glGenBuffers(1, &m_vbo);
        const char *vertexSrc = R"(
            attribute vec2 position;
            attribute vec2 texCoord;
            varying vec2 vTexCoord;
            void main() {
                gl_Position = vec4(position, 0.0, 1.0);
                vTexCoord = texCoord;
            }
        )";
        const char *fragmentSrc = R"(
            varying vec2 vTexCoord;
            uniform sampler2D yTexture;
            uniform sampler2D uvTexture;
            void main() {
                float y = texture2D(yTexture, vTexCoord).r;
                vec2 uv = texture2D(uvTexture, vTexCoord).ra - vec2(0.5, 0.5);
                float r = y + 1.402 * uv.y;
                float g = y - 0.344136 * uv.x - 0.714136 * uv.y;
                float b = y + 1.772 * uv.x;
                gl_FragColor = vec4(r, g, b, 1.0);
            }
        )";
        auto compileShader = [](GLenum type, const char *src) -> GLuint {
            GLuint shader = g_gl->glCreateShader(type);
            g_gl->glShaderSource(shader, 1, &src, nullptr);
            g_gl->glCompileShader(shader);
            GLint ok = 0;
            g_gl->glGetShaderiv(shader, GL_COMPILE_STATUS, &ok);
            if (!ok) {
                char log[512] = {};
                g_gl->glGetShaderInfoLog(shader, sizeof(log), nullptr, log);
                std::cerr << "Lunaris: DMABUF shader compile failed: " << log << std::endl;
                g_gl->glDeleteShader(shader);
                return 0;
            }
            return shader;
        };
        GLuint vs = compileShader(GL_VERTEX_SHADER, vertexSrc);
        GLuint fs = compileShader(GL_FRAGMENT_SHADER, fragmentSrc);
        if (!vs || !fs) {
            markDmabufFailed("shader compile failed");
            return false;
        }
        m_program = g_gl->glCreateProgram();
        g_gl->glAttachShader(m_program, vs);
        g_gl->glAttachShader(m_program, fs);
        g_gl->glBindAttribLocation(m_program, 0, "position");
        g_gl->glBindAttribLocation(m_program, 1, "texCoord");
        g_gl->glLinkProgram(m_program);
        g_gl->glDeleteShader(vs);
        g_gl->glDeleteShader(fs);
        GLint linked = 0;
        g_gl->glGetProgramiv(m_program, GL_LINK_STATUS, &linked);
        if (!linked) {
            markDmabufFailed("shader link failed");
            return false;
        }
        m_texturesInitialized = true;
    }
    return true;
#else
    return false;
#endif
}

void VaapiDmabufVideoItem::cleanupFrameLocked() {
#if defined(Q_OS_LINUX)
    EGLDisplay display = eglGetCurrentDisplay();
    if (display != EGL_NO_DISPLAY && eglDestroyImageKHRFn) {
        if (m_yImage) {
            eglDestroyImageKHRFn(display, static_cast<EGLImageKHR>(m_yImage));
            m_yImage = nullptr;
        }
        if (m_uvImage) {
            eglDestroyImageKHRFn(display, static_cast<EGLImageKHR>(m_uvImage));
            m_uvImage = nullptr;
        }
    }
#endif
    m_haveImportedFrame = false;
}

void VaapiDmabufVideoItem::cleanupGlResources() {
    cleanupFrameLocked();
    if (g_gl) {
        if (m_yTexture) g_gl->glDeleteTextures(1, &m_yTexture);
        if (m_uvTexture) g_gl->glDeleteTextures(1, &m_uvTexture);
        if (m_vbo) g_gl->glDeleteBuffers(1, &m_vbo);
    }
    m_yTexture = 0;
    m_uvTexture = 0;
    m_vbo = 0;
    m_texturesInitialized = false;
}

bool VaapiDmabufVideoItem::importPendingFrameLocked() {
#if defined(Q_OS_LINUX)
    if (!g_pendingFrame.newFrame) {
        return m_haveImportedFrame;
    }
    if (g_pendingFrame.fourcc != DRM_FORMAT_NV12) {
        closeFrameFds(g_pendingFrame);
        g_pendingFrame.newFrame = false;
        markDmabufFailed("only NV12 DMABUF frames are supported initially");
        return false;
    }
    EGLDisplay display = eglGetCurrentDisplay();
    if (display == EGL_NO_DISPLAY) {
        closeFrameFds(g_pendingFrame);
        g_pendingFrame.newFrame = false;
        markDmabufFailed("no current EGL display");
        return false;
    }

    void *newYImage = createPlaneImage(display, g_pendingFrame.fd0, g_pendingFrame.width, g_pendingFrame.height,
                                       0x20203852 /* R8 */, g_pendingFrame.offset0, g_pendingFrame.pitch0,
                                       g_pendingFrame.modifier);
    void *newUvImage = createPlaneImage(display, g_pendingFrame.fd1, g_pendingFrame.width / 2, g_pendingFrame.height / 2,
                                        0x38384752 /* GR88 */, g_pendingFrame.offset1, g_pendingFrame.pitch1,
                                        g_pendingFrame.modifier);
    closeFrameFds(g_pendingFrame);
    g_pendingFrame.newFrame = false;

    if (!newYImage || !newUvImage) {
        if (newYImage) eglDestroyImageKHRFn(display, static_cast<EGLImageKHR>(newYImage));
        if (newUvImage) eglDestroyImageKHRFn(display, static_cast<EGLImageKHR>(newUvImage));
        markDmabufFailed("eglCreateImageKHR failed for NV12 planes");
        return false;
    }

    cleanupFrameLocked();
    m_yImage = newYImage;
    m_uvImage = newUvImage;
    m_videoWidth = g_pendingFrame.width;
    m_videoHeight = g_pendingFrame.height;

    g_gl->glBindTexture(GL_TEXTURE_2D, m_yTexture);
    g_gl->glTexParameteri(GL_TEXTURE_2D, GL_TEXTURE_MIN_FILTER, GL_LINEAR);
    g_gl->glTexParameteri(GL_TEXTURE_2D, GL_TEXTURE_MAG_FILTER, GL_LINEAR);
    g_gl->glTexParameteri(GL_TEXTURE_2D, GL_TEXTURE_WRAP_S, GL_CLAMP_TO_EDGE);
    g_gl->glTexParameteri(GL_TEXTURE_2D, GL_TEXTURE_WRAP_T, GL_CLAMP_TO_EDGE);
    glEGLImageTargetTexture2DOESFn(GL_TEXTURE_2D, m_yImage);

    g_gl->glBindTexture(GL_TEXTURE_2D, m_uvTexture);
    g_gl->glTexParameteri(GL_TEXTURE_2D, GL_TEXTURE_MIN_FILTER, GL_LINEAR);
    g_gl->glTexParameteri(GL_TEXTURE_2D, GL_TEXTURE_MAG_FILTER, GL_LINEAR);
    g_gl->glTexParameteri(GL_TEXTURE_2D, GL_TEXTURE_WRAP_S, GL_CLAMP_TO_EDGE);
    g_gl->glTexParameteri(GL_TEXTURE_2D, GL_TEXTURE_WRAP_T, GL_CLAMP_TO_EDGE);
    glEGLImageTargetTexture2DOESFn(GL_TEXTURE_2D, m_uvImage);

    m_haveImportedFrame = true;
    setDmabufActive(true);
    return true;
#else
    return false;
#endif
}

void VaapiDmabufVideoItem::renderNative() {
#if defined(Q_OS_LINUX)
    if (!g_dmabufStreamActive || g_dmabufRenderFailed.load(std::memory_order_acquire) || !window()) {
        return;
    }
    QMutexLocker locker(&g_frameMutex);
    if (!ensureGlResources() || !importPendingFrameLocked()) {
        return;
    }

    QPointF globalPos = mapToScene(QPointF(0, 0));
    qreal dpr = window()->devicePixelRatio();
    int x = static_cast<int>(globalPos.x() * dpr);
    int y = static_cast<int>((window()->height() - globalPos.y() - height()) * dpr);
    int w = static_cast<int>(width() * dpr);
    int h = static_cast<int>(height() * dpr);

    const float vertices[] = {
        -1.0f, -1.0f, 0.0f, 1.0f,
         1.0f, -1.0f, 1.0f, 1.0f,
        -1.0f,  1.0f, 0.0f, 0.0f,
         1.0f,  1.0f, 1.0f, 0.0f,
    };

    window()->beginExternalCommands();
    g_gl->glDisable(GL_DEPTH_TEST);
    g_gl->glDisable(GL_CULL_FACE);
    g_gl->glDisable(GL_BLEND);
    g_gl->glViewport(x, y, w, h);
    g_gl->glUseProgram(m_program);
    g_gl->glActiveTexture(GL_TEXTURE0);
    g_gl->glBindTexture(GL_TEXTURE_2D, m_yTexture);
    g_gl->glUniform1i(g_gl->glGetUniformLocation(m_program, "yTexture"), 0);
    g_gl->glActiveTexture(GL_TEXTURE1);
    g_gl->glBindTexture(GL_TEXTURE_2D, m_uvTexture);
    g_gl->glUniform1i(g_gl->glGetUniformLocation(m_program, "uvTexture"), 1);
    g_gl->glBindBuffer(GL_ARRAY_BUFFER, m_vbo);
    g_gl->glBufferData(GL_ARRAY_BUFFER, sizeof(vertices), vertices, GL_STATIC_DRAW);
    g_gl->glEnableVertexAttribArray(0);
    g_gl->glVertexAttribPointer(0, 2, GL_FLOAT, GL_FALSE, 4 * sizeof(float), nullptr);
    g_gl->glEnableVertexAttribArray(1);
    g_gl->glVertexAttribPointer(1, 2, GL_FLOAT, GL_FALSE, 4 * sizeof(float), reinterpret_cast<void *>(2 * sizeof(float)));
    g_gl->glDrawArrays(GL_TRIANGLE_STRIP, 0, 4);
    g_gl->glDisableVertexAttribArray(0);
    g_gl->glDisableVertexAttribArray(1);
    g_gl->glBindBuffer(GL_ARRAY_BUFFER, 0);
    window()->endExternalCommands();
#endif
}

extern "C" bool deliver_dmabuf_frame(int fd0,
                                      int fd1,
                                      uint32_t fourcc,
                                      uint64_t modifier,
                                      int offset0,
                                      int pitch0,
                                      int offset1,
                                      int pitch1,
                                      int width,
                                      int height) {
#if defined(Q_OS_LINUX)
    if (g_dmabufRenderFailed.load(std::memory_order_acquire)) {
        int a = fd0;
        int b = fd1;
        closeFd(a);
        closeFd(b);
        return false;
    }
    QMutexLocker locker(&g_frameMutex);
    closeFrameFds(g_pendingFrame);
    g_pendingFrame.fd0 = fd0;
    g_pendingFrame.fd1 = fd1;
    g_pendingFrame.fourcc = fourcc;
    g_pendingFrame.modifier = modifier;
    g_pendingFrame.offset0 = offset0;
    g_pendingFrame.pitch0 = pitch0;
    g_pendingFrame.offset1 = offset1;
    g_pendingFrame.pitch1 = pitch1;
    g_pendingFrame.width = width;
    g_pendingFrame.height = height;
    g_pendingFrame.newFrame = true;
    g_dmabufStreamActive = true;
    if (g_activeItem) {
        QMetaObject::invokeMethod(g_activeItem, [item = g_activeItem]() {
            if (item) {
                item->update();
            }
        }, Qt::QueuedConnection);
    }
    return true;
#else
    Q_UNUSED(fd0); Q_UNUSED(fd1); Q_UNUSED(fourcc); Q_UNUSED(modifier); Q_UNUSED(offset0);
    Q_UNUSED(pitch0); Q_UNUSED(offset1); Q_UNUSED(pitch1); Q_UNUSED(width); Q_UNUSED(height);
    return false;
#endif
}

extern "C" bool dmabuf_render_failed() {
    return g_dmabufRenderFailed.load(std::memory_order_acquire);
}

extern "C" void set_dmabuf_stream_active(bool active) {
    g_dmabufStreamActive = active;
    if (!active) {
        QMutexLocker locker(&g_frameMutex);
        closeFrameFds(g_pendingFrame);
        g_pendingFrame.newFrame = false;
        if (g_activeItem) {
            QMetaObject::invokeMethod(g_activeItem, "setDmabufActive", Qt::QueuedConnection, Q_ARG(bool, false));
        }
    }
}

extern "C" void register_native_video_item_types() {
    D3D11VideoItem::registerTypes();
    VaapiDmabufVideoItem::registerTypes();
}
