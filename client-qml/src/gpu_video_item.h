#pragma once
#include <QQuickItem>
#include <QOpenGLFunctions>
#include <QtOpenGL/QOpenGLVertexArrayObject>
#include <QMutex>
#include <cstdint>

class GpuVideoItem : public QQuickItem {
    Q_OBJECT
    Q_PROPERTY(bool cudaSupported READ cudaSupported CONSTANT)
public:
    explicit GpuVideoItem(QQuickItem *parent = nullptr);
    ~GpuVideoItem();

    bool cudaSupported() const;
    void cleanupCudaGL(bool skipCuda = false);

    static void registerTypes();

protected:
    QSGNode *updatePaintNode(QSGNode *oldNode, UpdatePaintNodeData *data) override;

private slots:
    void handleWindowChanged(QQuickWindow *win);
    void renderNative();

private:
    void initCudaGL();

    QMutex m_mutex;
    unsigned int m_yTexture = 0;
    unsigned int m_uvTexture = 0;
    void* m_cudaYRes = nullptr;
    void* m_cudaUvRes = nullptr;
    int m_videoWidth = 0;
    int m_videoHeight = 0;
    bool m_texturesInitialized = false;
    bool m_cudaInitialized = false;
    void* m_cudaContext = nullptr;

    // Shader program for YUV to RGB
    unsigned int m_program = 0;
    unsigned int m_vbo = 0;
    QOpenGLVertexArrayObject* m_vao = nullptr;
};

extern "C" {
void deliver_cuda_frame(uint64_t cuda_ctx, uint64_t y_ptr, int y_stride, uint64_t uv_ptr, int uv_stride, int width, int height);
void register_gpu_video_item_type();
void set_cuda_stream_active(bool active);
}

