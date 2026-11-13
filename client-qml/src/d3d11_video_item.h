#pragma once

#include <QQuickItem>
#include <cstdint>

class D3D11VideoItem : public QQuickItem {
    Q_OBJECT
    Q_PROPERTY(bool d3d11Supported READ d3d11Supported CONSTANT)
    Q_PROPERTY(bool d3d11Active READ d3d11Active NOTIFY d3d11ActiveChanged)

public:
    explicit D3D11VideoItem(QQuickItem *parent = nullptr);
    ~D3D11VideoItem();

    bool d3d11Supported() const;
    bool d3d11Active() const;

    static void registerTypes();

public slots:
    void setD3d11Active(bool active);

signals:
    void d3d11ActiveChanged();

protected:
    QSGNode *updatePaintNode(QSGNode *oldNode, UpdatePaintNodeData *data) override;

private slots:
    void handleWindowChanged(QQuickWindow *win);
    void renderNative();
    void releaseRenderResources();

private:
    bool m_d3d11Active = false;
};

extern "C" {
// Windows: creates the D3D11 device FFmpeg decodes on (a second device on Qt's
// adapter) and returns an AVBufferRef* (D3D11VA hw device context, one
// reference owned by the caller) as an integer; 0 when the path is not
// available (the CPU path is used then).
uint64_t d3d11_setup_decoder_device();
// get_format helper: builds the hw frames context for `pixel_format` on the
// codec context (AVCodecContext*) and the render-side resources for it.
bool d3d11_prepare_decoder_context_in_get_format(uint64_t codec_ctx, int pixel_format);
// Hands a decoded AV_PIX_FMT_D3D11 frame (AVFrame*) to the renderer. Returns
// true when the renderer took ownership of the frame; false leaves it with the
// caller.
bool deliver_d3d11_frame(uint64_t frame_ptr);
bool d3d11_render_failed();
void set_d3d11_stream_active(bool active);
}
