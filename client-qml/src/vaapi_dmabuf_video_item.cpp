#include "video_helper.h"
#include "vaapi_dmabuf_video_item.h"
#include "d3d11_video_item.h"

#include <QOpenGLContext>
#include <QOpenGLFunctions>
#include <QQuickWindow>
#include <QScreen>
#include <QSGNode>
#include <QVector>
#include <QtQml/qqml.h>
#include <atomic>
#include <cerrno>
#include <chrono>
#include <cstring>
#include <deque>
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
    // The decoder's AVFrame, kept until the frame is no longer queued or
    // shown: the DMA-BUF is the decoder's surface, which would otherwise go
    // back to its pool and be decoded into again while still on screen.
    uint64_t owner = 0;
};

// Frame pacing, a port of Moonlight's Pacer (moonlight-qt
// app/streaming/video/ffmpeg-renderers/pacer/pacer.cpp): decoded frames wait
// in a queue and the render thread takes one per rendered frame, instead of
// each new frame replacing the one not shown yet. The window is rendered on
// the display's clock whenever anything in it changes (the pointer moving is
// enough), so frames that arrive close together would otherwise be lost:
// measured with the pointer moving at 1000 Hz, 4-8 of 75 frames per second
// (16 in the worst second) against 0.1 with the pointer at rest.
// All guarded by g_frameMutex.
QMutex g_frameMutex;
std::deque<PendingDmabufFrame> g_pacingQueue;      // Pacer::m_PacingQueue
std::deque<int> g_pacingQueueHistory;              // Pacer::m_PacingQueueHistory
uint64_t g_displayedOwner = 0;                     // the frame on screen
// Frames per second of the stream, measured over the last second
// (Pacer::m_MaxVideoFps comes from the stream configuration there).
int g_streamFps = 0;
int g_streamFpsCount = 0;
std::chrono::steady_clock::time_point g_streamFpsSince = std::chrono::steady_clock::now();
// Safety cap, not in Moonlight (its queue is bounded by handleVsync running on
// every V-sync; this render callback does not run while the window is hidden).
constexpr size_t MAX_PACING_QUEUE = 8;
const bool g_statsEnabled = qEnvironmentVariableIsSet("LUNARIS_CLIENT_STATS");
int g_statDelivered = 0, g_statShown = 0, g_statReplaced = 0;
std::chrono::steady_clock::time_point g_statSince = std::chrono::steady_clock::now();
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

void releaseOwner(uint64_t &owner) {
    if (owner) {
        lunaris_release_video_frame(owner);
        owner = 0;
    }
}

// A queued frame that will not be shown.
void dropFrame(PendingDmabufFrame &frame) {
    closeFrameFds(frame);
    releaseOwner(frame.owner);
}

void clearPacingQueueLocked() {
    for (PendingDmabufFrame &frame : g_pacingQueue) dropFrame(frame);
    g_pacingQueue.clear();
    g_pacingQueueHistory.clear();
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
        // Free the GL objects while their context is still current (render
        // thread). By the time the item is destroyed at exit the context is
        // gone, and g_gl would point into it.
        connect(win, &QQuickWindow::sceneGraphInvalidated, this, &VaapiDmabufVideoItem::handleSceneGraphInvalidated, Qt::DirectConnection);
    }
}

void VaapiDmabufVideoItem::handleSceneGraphInvalidated() {
    QMutexLocker locker(&m_mutex);
    cleanupGlResources();
    // The functions belong to the context that is going away.
    g_gl = nullptr;
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
            uniform vec4 uRange;
            uniform vec4 uCoef;
            void main() {
                float y = (texture2D(yTexture, vTexCoord).r - uRange.x) * uRange.y;
                vec2 uv = (texture2D(uvTexture, vTexCoord).rg - vec2(128.0 / 255.0)) * uRange.z;
                float r = y + uCoef.x * uv.y;
                float g = y - uCoef.y * uv.x - uCoef.z * uv.y;
                float b = y + uCoef.w * uv.x;
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
    releaseOwner(g_displayedOwner);
    m_haveImportedFrame = false;
}

void VaapiDmabufVideoItem::cleanupGlResources() {
    // GL/EGL calls need the item's context to be current: true on the render
    // thread (sceneGraphInvalidated), not in the destructor at shutdown, where
    // the objects die with the context anyway.
    if (!QOpenGLContext::currentContext()) {
        m_yImage = nullptr;
        m_uvImage = nullptr;
        m_haveImportedFrame = false;
        m_yTexture = 0;
        m_uvTexture = 0;
        m_vbo = 0;
        m_texturesInitialized = false;
        return;
    }
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
    // Pacer::handleVsync (pacer.cpp:215-262), run once per rendered frame.
    // "If the queue length history entries are large, be strict about
    // dropping excess frames."
    size_t frameDropTarget = 1;
    const int displayFps = (window() && window()->screen()) ? qRound(window()->screen()->refreshRate()) : 0;
    // "If we may get more frames per second than we can display, use frame
    // history to drop frames only if consistently above the one queued frame
    // mark."
    if (displayFps > 0 && g_streamFps >= displayFps) {
        for (int queueHistoryEntry : g_pacingQueueHistory) {
            if (queueHistoryEntry <= 1) {
                // "Be lenient as long as the queue length resolves before
                // the end of frame history"
                frameDropTarget = 3;
                break;
            }
        }
        // "Keep a rolling 500 ms window of pacing queue history"
        if (static_cast<int>(g_pacingQueueHistory.size()) >= displayFps / 2) {
            g_pacingQueueHistory.pop_front();
        }
        g_pacingQueueHistory.push_back(static_cast<int>(g_pacingQueue.size()));
    }
    // "Catch up if we're several frames ahead"
    while (g_pacingQueue.size() > frameDropTarget) {
        dropFrame(g_pacingQueue.front());
        g_pacingQueue.pop_front();
        g_statReplaced++;
    }
    if (g_pacingQueue.empty()) {
        return m_haveImportedFrame;
    }
    PendingDmabufFrame frame = g_pacingQueue.front();
    g_pacingQueue.pop_front();

    if (frame.fourcc != DRM_FORMAT_NV12) {
        dropFrame(frame);
        markDmabufFailed("only NV12 DMABUF frames are supported initially");
        return false;
    }
    EGLDisplay display = eglGetCurrentDisplay();
    if (display == EGL_NO_DISPLAY) {
        dropFrame(frame);
        markDmabufFailed("no current EGL display");
        return false;
    }

    void *newYImage = createPlaneImage(display, frame.fd0, frame.width, frame.height,
                                       0x20203852 /* R8 */, frame.offset0, frame.pitch0,
                                       frame.modifier);
    void *newUvImage = createPlaneImage(display, frame.fd1, frame.width / 2, frame.height / 2,
                                        0x38385247 /* GR88: U in red, V in green */, frame.offset1, frame.pitch1,
                                        frame.modifier);
    closeFrameFds(frame);

    if (!newYImage || !newUvImage) {
        if (newYImage) eglDestroyImageKHRFn(display, static_cast<EGLImageKHR>(newYImage));
        if (newUvImage) eglDestroyImageKHRFn(display, static_cast<EGLImageKHR>(newUvImage));
        std::cerr << "Lunaris: eglCreateImageKHR for the NV12 planes failed: EGL error 0x" << std::hex << eglGetError()
                  << " (modifier 0x" << frame.modifier << std::dec << ", " << frame.width << "x"
                  << frame.height << ", pitches " << frame.pitch0 << "/" << frame.pitch1 << ")" << std::endl;
        releaseOwner(frame.owner);
        markDmabufFailed("eglCreateImageKHR failed for NV12 planes");
        return false;
    }

    // The previous frame's images and surface go only now (Pacer keeps the
    // frame being shown until the next one is rendered).
    cleanupFrameLocked();
    g_displayedOwner = frame.owner;
    g_statShown++;
    m_yImage = newYImage;
    m_uvImage = newUvImage;
    m_videoWidth = frame.width;
    m_videoHeight = frame.height;

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
    if (!g_pacingQueue.empty()) {
        // More frames wait: render again on the next refresh.
        QMetaObject::invokeMethod(this, [this]() { update(); }, Qt::QueuedConnection);
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
    {
        const YuvCoefficients c = video_yuv_coefficients();
        g_gl->glUniform4f(g_gl->glGetUniformLocation(m_program, "uRange"), c.y_offset, c.y_scale, c.c_scale, 0.0f);
        g_gl->glUniform4f(g_gl->glGetUniformLocation(m_program, "uCoef"), c.rv, c.gu, c.gv, c.bu);
    }
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
                                      int height,
                                      uint64_t owner) {
#if defined(Q_OS_LINUX)
    if (g_dmabufRenderFailed.load(std::memory_order_acquire)) {
        int a = fd0;
        int b = fd1;
        closeFd(a);
        closeFd(b);
        return false;
    }
    QMutexLocker locker(&g_frameMutex);
    const auto now = std::chrono::steady_clock::now();
    g_streamFpsCount++;
    if (now - g_streamFpsSince >= std::chrono::seconds(1)) {
        g_streamFps = qRound(g_streamFpsCount / std::chrono::duration<double>(now - g_streamFpsSince).count());
        g_streamFpsCount = 0;
        g_streamFpsSince = now;
    }
    if (g_statsEnabled) {
        // LUNARIS_CLIENT_STATS=1: frames handed over, shown, and dropped
        // before they were shown, once per second.
        g_statDelivered++;
        if (now - g_statSince >= std::chrono::seconds(1)) {
            std::cerr << "Present stats: delivered=" << g_statDelivered << " shown=" << g_statShown
                      << " replaced_unshown=" << g_statReplaced << " queued=" << g_pacingQueue.size() << std::endl;
            g_statDelivered = g_statShown = g_statReplaced = 0;
            g_statSince = now;
        }
    }
    while (g_pacingQueue.size() >= MAX_PACING_QUEUE) {
        dropFrame(g_pacingQueue.front());
        g_pacingQueue.pop_front();
        g_statReplaced++;
    }
    PendingDmabufFrame frame;
    frame.fd0 = fd0;
    frame.fd1 = fd1;
    frame.fourcc = fourcc;
    frame.modifier = modifier;
    frame.offset0 = offset0;
    frame.pitch0 = pitch0;
    frame.offset1 = offset1;
    frame.pitch1 = pitch1;
    frame.width = width;
    frame.height = height;
    frame.owner = owner;
    g_pacingQueue.push_back(frame);
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
    Q_UNUSED(pitch0); Q_UNUSED(offset1); Q_UNUSED(pitch1); Q_UNUSED(width); Q_UNUSED(height); Q_UNUSED(owner);
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
        clearPacingQueueLocked();
        if (g_activeItem) {
            QMetaObject::invokeMethod(g_activeItem, "setDmabufActive", Qt::QueuedConnection, Q_ARG(bool, false));
        }
    }
}

extern "C" void register_native_video_item_types() {
    D3D11VideoItem::registerTypes();
    VaapiDmabufVideoItem::registerTypes();
}
