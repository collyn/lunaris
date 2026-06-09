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

private:
    bool m_d3d11Active = false;
};

extern "C" {
bool deliver_d3d11_frame(uint64_t texture_ptr, int array_index, int width, int height, uint32_t format);
bool d3d11_render_failed();
void set_d3d11_stream_active(bool active);
}
