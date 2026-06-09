#pragma once

#include <QMutex>
#include <QQuickItem>
#include <array>
#include <cstdint>

class VaapiDmabufVideoItem : public QQuickItem {
    Q_OBJECT
    Q_PROPERTY(bool dmabufSupported READ dmabufSupported CONSTANT)
    Q_PROPERTY(bool dmabufActive READ dmabufActive NOTIFY dmabufActiveChanged)

public:
    explicit VaapiDmabufVideoItem(QQuickItem *parent = nullptr);
    ~VaapiDmabufVideoItem();

    bool dmabufSupported() const;
    bool dmabufActive() const;

    static void registerTypes();

public slots:
    void setDmabufActive(bool active);

signals:
    void dmabufActiveChanged();

protected:
    QSGNode *updatePaintNode(QSGNode *oldNode, UpdatePaintNodeData *data) override;

private slots:
    void handleWindowChanged(QQuickWindow *win);
    void renderNative();

private:
    void cleanupFrameLocked();
    void cleanupGlResources();
    bool ensureGlResources();
    bool importPendingFrameLocked();

    QMutex m_mutex;
    bool m_dmabufActive = false;
    bool m_texturesInitialized = false;
    bool m_haveImportedFrame = false;
    int m_videoWidth = 0;
    int m_videoHeight = 0;
    unsigned int m_yTexture = 0;
    unsigned int m_uvTexture = 0;
    unsigned int m_program = 0;
    unsigned int m_vbo = 0;
    void *m_yImage = nullptr;
    void *m_uvImage = nullptr;
};

extern "C" {
bool deliver_dmabuf_frame(int fd0,
                         int fd1,
                         uint32_t fourcc,
                         uint64_t modifier,
                         int offset0,
                         int pitch0,
                         int offset1,
                         int pitch1,
                         int width,
                         int height);
bool dmabuf_render_failed();
void set_dmabuf_stream_active(bool active);
void register_native_video_item_types();
}
