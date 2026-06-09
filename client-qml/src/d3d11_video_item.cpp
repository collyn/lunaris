#include "d3d11_video_item.h"

#include <QQuickWindow>
#include <QSGNode>
#include <QtQml/qqml.h>
#include <atomic>
#include <iostream>

#if defined(Q_OS_WIN)
#include <d3d11.h>
#endif

namespace {
std::atomic_bool g_d3d11RenderFailed{false};
bool g_d3d11StreamActive = false;
D3D11VideoItem *g_activeItem = nullptr;

[[maybe_unused]] void markD3d11Failed(const char *reason) {
    g_d3d11RenderFailed.store(true, std::memory_order_release);
    std::cerr << "Lunaris: D3D11 renderer disabled: " << reason << std::endl;
    if (g_activeItem) {
        QMetaObject::invokeMethod(g_activeItem, "setD3d11Active", Qt::QueuedConnection, Q_ARG(bool, false));
    }
}
}

D3D11VideoItem::D3D11VideoItem(QQuickItem *parent) : QQuickItem(parent) {
    g_activeItem = this;
    setFlag(ItemHasContents, true);
    connect(this, &QQuickItem::windowChanged, this, &D3D11VideoItem::handleWindowChanged);
}

D3D11VideoItem::~D3D11VideoItem() {
    g_activeItem = nullptr;
}

bool D3D11VideoItem::d3d11Supported() const {
#if defined(Q_OS_WIN)
    return true;
#else
    return false;
#endif
}

bool D3D11VideoItem::d3d11Active() const {
    return m_d3d11Active;
}

void D3D11VideoItem::setD3d11Active(bool active) {
    if (m_d3d11Active == active) {
        return;
    }
    m_d3d11Active = active;
    emit d3d11ActiveChanged();
    update();
}

void D3D11VideoItem::registerTypes() {
    qmlRegisterType<D3D11VideoItem>("com.lunaris.client.gpu", 1, 0, "D3D11VideoItem");
}

void D3D11VideoItem::handleWindowChanged(QQuickWindow *win) {
    if (win) {
        connect(win, &QQuickWindow::beforeRenderPassRecording, this, &D3D11VideoItem::renderNative, Qt::DirectConnection);
    }
}

QSGNode *D3D11VideoItem::updatePaintNode(QSGNode *oldNode, UpdatePaintNodeData *) {
    delete oldNode;
    return nullptr;
}

void D3D11VideoItem::renderNative() {
#if defined(Q_OS_WIN)
    if (!g_d3d11StreamActive || g_d3d11RenderFailed.load(std::memory_order_acquire)) {
        return;
    }
    // Placeholder renderer. The decoder probes this path, but until the Qt D3D11
    // texture import path is implemented and verified on Windows, keep CPU fallback active.
#endif
}

extern "C" bool deliver_d3d11_frame(uint64_t texture_ptr, int array_index, int width, int height, uint32_t format) {
#if defined(Q_OS_WIN)
    Q_UNUSED(texture_ptr);
    Q_UNUSED(array_index);
    Q_UNUSED(width);
    Q_UNUSED(height);
    Q_UNUSED(format);
    markD3d11Failed("native D3D11 presentation is not enabled yet");
    return false;
#else
    Q_UNUSED(texture_ptr);
    Q_UNUSED(array_index);
    Q_UNUSED(width);
    Q_UNUSED(height);
    Q_UNUSED(format);
    return false;
#endif
}

extern "C" bool d3d11_render_failed() {
    return g_d3d11RenderFailed.load(std::memory_order_acquire);
}

extern "C" void set_d3d11_stream_active(bool active) {
    g_d3d11StreamActive = active;
    if (!active && g_activeItem) {
        QMetaObject::invokeMethod(g_activeItem, "setD3d11Active", Qt::QueuedConnection, Q_ARG(bool, false));
    }
}
