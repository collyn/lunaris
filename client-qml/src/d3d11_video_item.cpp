#include "d3d11_video_item.h"

#include <QQuickWindow>
#include <QSGNode>
#include <QtQml/qqml.h>
#include <atomic>
#include <iostream>
#include <mutex>

#if defined(Q_OS_WIN)
#include <d3d11.h>
#include <d3dcompiler.h>
#include <private/qrhi_p.h>
#endif

namespace {
std::atomic_bool g_d3d11RenderFailed{false};
bool g_d3d11StreamActive = false;
D3D11VideoItem *g_activeItem = nullptr;

void markD3d11Failed(const char *reason) {
    g_d3d11RenderFailed.store(true, std::memory_order_release);
    std::cerr << "Lunaris: D3D11 renderer disabled: " << reason << std::endl;
    if (g_activeItem) {
        QMetaObject::invokeMethod(g_activeItem, "setD3d11Active", Qt::QueuedConnection, Q_ARG(bool, false));
    }
}

// ── Pending frame ──────────────────────────────────────────────────────
struct PendingD3d11Frame {
#if defined(Q_OS_WIN)
    ID3D11Texture2D *texture = nullptr; // borrowed, do not Release
    unsigned int array_index = 0;
#else
    int array_index = 0;
#endif
    int width = 0;
    int height = 0;
    bool new_frame = false;
};
std::mutex g_frameMutex;
PendingD3d11Frame g_pendingFrame;

// ── Cached Qt D3D11 device ─────────────────────────────────────────────
#if defined(Q_OS_WIN)
static QQuickWindow *g_window = nullptr;
static ID3D11Device *g_qtDevice = nullptr;
static ID3D11DeviceContext *g_qtContext = nullptr;

// Render-target texture + SRV (recreated on resize).
static ID3D11Texture2D *g_renderTex = nullptr;
static ID3D11ShaderResourceView *g_renderSrv = nullptr;
static int g_texW = 0, g_texH = 0;

// Simple full-screen quad vertex + pixel shaders for NV12→RGBA.
static ID3D11VertexShader *g_vs = nullptr;
static ID3D11PixelShader *g_ps = nullptr;
static ID3D11InputLayout *g_inputLayout = nullptr;
static ID3D11Buffer *g_quadVB = nullptr;
static ID3D11SamplerState *g_sampler = nullptr;
static ID3D11BlendState *g_blendState = nullptr;
static ID3D11RasterizerState *g_rasterState = nullptr;
static ID3D11DepthStencilState *g_depthState = nullptr;

// Separate Y + UV plane textures & SRVs for NV12 conversion.
static ID3D11Texture2D *g_yTex = nullptr;
static ID3D11ShaderResourceView *g_ySrv = nullptr;
static ID3D11Texture2D *g_uvTex = nullptr;
static ID3D11ShaderResourceView *g_uvSrv = nullptr;

static void releaseD3dResources() {
    if (g_ySrv) { g_ySrv->Release(); g_ySrv = nullptr; }
    if (g_yTex) { g_yTex->Release(); g_yTex = nullptr; }
    if (g_uvSrv) { g_uvSrv->Release(); g_uvSrv = nullptr; }
    if (g_uvTex) { g_uvTex->Release(); g_uvTex = nullptr; }
    if (g_renderSrv) { g_renderSrv->Release(); g_renderSrv = nullptr; }
    if (g_renderTex) { g_renderTex->Release(); g_renderTex = nullptr; }
    g_texW = 0; g_texH = 0;
    // Shaders, VB, states are kept for the lifetime of the stream.
}

static void releaseAllD3d() {
    releaseD3dResources();
    if (g_sampler) { g_sampler->Release(); g_sampler = nullptr; }
    if (g_blendState) { g_blendState->Release(); g_blendState = nullptr; }
    if (g_rasterState) { g_rasterState->Release(); g_rasterState = nullptr; }
    if (g_depthState) { g_depthState->Release(); g_depthState = nullptr; }
    if (g_quadVB) { g_quadVB->Release(); g_quadVB = nullptr; }
    if (g_inputLayout) { g_inputLayout->Release(); g_inputLayout = nullptr; }
    if (g_vs) { g_vs->Release(); g_vs = nullptr; }
    if (g_ps) { g_ps->Release(); g_ps = nullptr; }
    if (g_qtContext) { g_qtContext->Release(); g_qtContext = nullptr; }
    g_qtDevice = nullptr;
    g_window = nullptr;
}

/// Get Qt's D3D11 device + context from the QRhi.
static bool ensureQtD3D(QQuickWindow *window) {
    if (g_qtDevice && g_window == window) return true;
    if (!window) return false;

    QRhi *rhi = window->rhi();
    if (!rhi || rhi->backend() != QRhi::D3D11) {
        markD3d11Failed("Qt RHI backend is not D3D11");
        return false;
    }

    // QRhi nativeHandles() gives us the D3D11 device pointer.
    // Cast the opaque void* to ID3D11Device* — the layout is guaranteed
    // by the Qt RHI contract when backend == D3D11.
    const void *handles = rhi->nativeHandles();
    if (!handles) {
        markD3d11Failed("QRhi::nativeHandles() returned null");
        return false;
    }
    // The first field of QRhiD3D11NativeHandles is `ID3D11Device *dev`.
    g_qtDevice = *static_cast<ID3D11Device *const *>(handles);
    if (!g_qtDevice) {
        markD3d11Failed("QRhi D3D11 device is null");
        return false;
    }
    g_qtDevice->GetImmediateContext(&g_qtContext);
    g_window = window;
    std::cerr << "Lunaris: D3D11 present — obtained Qt device ok." << std::endl;
    return true;
}

// Minimal HLSL: pass-through vertex shader + NV12→RGBA pixel shader.
static const char *kVertexShader = R"(
struct VS_IN { float2 pos : POSITION; float2 uv : TEXCOORD; };
struct VS_OUT { float4 pos : SV_POSITION; float2 uv : TEXCOORD; };
VS_OUT main(VS_IN input) { VS_OUT o; o.pos = float4(input.pos, 0, 1); o.uv = input.uv; return o; }
)";

static const char *kPixelShaderNv12 = R"(
Texture2D    texY  : register(t0);
Texture2D    texUV : register(t1);
SamplerState samp  : register(s0);
struct PS_IN { float4 pos : SV_POSITION; float2 uv : TEXCOORD; };
float4 main(PS_IN input) : SV_TARGET {
    float  y  = texY.Sample(samp, input.uv).r;
    float2 uv = texUV.Sample(samp, input.uv).rg;
    float c = 1.1640625 * (y - 0.0625);
    float d = uv.x - 0.5;
    float e = uv.y - 0.5;
    float r = saturate(c + 1.79296875 * d);
    float g = saturate(c - 0.2138671875 * d - 0.533203125 * e);
    float b = saturate(c + 2.1171875 * e);
    return float4(r, g, b, 1);
}
)";

static bool compileShaders() {
    if (g_vs && g_ps) return true;
    if (!g_qtDevice) return false;

    ID3DBlob *vsBlob = nullptr, *psBlob = nullptr, *errBlob = nullptr;
    HRESULT hr;

    hr = D3DCompile(kVertexShader, strlen(kVertexShader), "vs", nullptr, nullptr,
                    "main", "vs_4_0", 0, 0, &vsBlob, &errBlob);
    if (FAILED(hr)) {
        if (errBlob) { std::cerr << "D3D11 VS: " << (char*)errBlob->GetBufferPointer() << std::endl; errBlob->Release(); }
        markD3d11Failed("Vertex shader compile failed");
        return false;
    }

    hr = D3DCompile(kPixelShaderNv12, strlen(kPixelShaderNv12), "ps", nullptr, nullptr,
                    "main", "ps_4_0", 0, 0, &psBlob, &errBlob);
    if (FAILED(hr)) {
        if (errBlob) { std::cerr << "D3D11 PS: " << (char*)errBlob->GetBufferPointer() << std::endl; errBlob->Release(); }
        vsBlob->Release();
        markD3d11Failed("Pixel shader compile failed");
        return false;
    }

    g_qtDevice->CreateVertexShader(vsBlob->GetBufferPointer(), vsBlob->GetBufferSize(), nullptr, &g_vs);
    g_qtDevice->CreatePixelShader(psBlob->GetBufferPointer(), psBlob->GetBufferSize(), nullptr, &g_ps);

    // Input layout: position (float2) + texcoord (float2)
    D3D11_INPUT_ELEMENT_DESC layout[] = {
        { "POSITION", 0, DXGI_FORMAT_R32G32_FLOAT, 0, 0, D3D11_INPUT_PER_VERTEX_DATA, 0 },
        { "TEXCOORD", 0, DXGI_FORMAT_R32G32_FLOAT, 0, 8, D3D11_INPUT_PER_VERTEX_DATA, 0 },
    };
    g_qtDevice->CreateInputLayout(layout, 2, vsBlob->GetBufferPointer(), vsBlob->GetBufferSize(), &g_inputLayout);
    vsBlob->Release();
    psBlob->Release();

    if (!g_vs || !g_ps || !g_inputLayout) {
        markD3d11Failed("Shader creation failed");
        return false;
    }

    // Full-screen quad (NDC coords + UVs)
    struct QuadVertex { float x, y, u, v; };
    QuadVertex verts[] = {
        {-1,-1, 0,1}, {1,-1, 1,1}, {-1,1, 0,0},
        {-1,1, 0,0}, {1,-1, 1,1}, {1,1, 1,0},
    };
    D3D11_BUFFER_DESC vbDesc = {};
    vbDesc.ByteWidth = sizeof(verts);
    vbDesc.Usage = D3D11_USAGE_IMMUTABLE;
    vbDesc.BindFlags = D3D11_BIND_VERTEX_BUFFER;
    D3D11_SUBRESOURCE_DATA vbData = { verts, 0, 0 };
    g_qtDevice->CreateBuffer(&vbDesc, &vbData, &g_quadVB);

    // Sampler
    D3D11_SAMPLER_DESC sampDesc = {};
    sampDesc.Filter = D3D11_FILTER_MIN_MAG_LINEAR_MIP_POINT;
    sampDesc.AddressU = D3D11_TEXTURE_ADDRESS_CLAMP;
    sampDesc.AddressV = D3D11_TEXTURE_ADDRESS_CLAMP;
    g_qtDevice->CreateSamplerState(&sampDesc, &g_sampler);

    // States
    D3D11_BLEND_DESC blendDesc = {};
    blendDesc.RenderTarget[0].RenderTargetWriteMask = D3D11_COLOR_WRITE_ENABLE_ALL;
    g_qtDevice->CreateBlendState(&blendDesc, &g_blendState);

    D3D11_RASTERIZER_DESC rastDesc = {};
    rastDesc.FillMode = D3D11_FILL_SOLID;
    rastDesc.CullMode = D3D11_CULL_NONE;
    g_qtDevice->CreateRasterizerState(&rastDesc, &g_rasterState);

    D3D11_DEPTH_STENCIL_DESC depthDesc = {};
    g_qtDevice->CreateDepthStencilState(&depthDesc, &g_depthState);

    std::cerr << "Lunaris: D3D11 NV12 shaders compiled ok." << std::endl;
    return true;
}

/// Split NV12/P010 source into separate Y + UV plane textures that a
/// pixel shader can sample.
static bool splitNv12Planes(ID3D11Texture2D *srcTex, UINT srcArrayIdx, int w, int h) {
    if (!g_qtDevice) return false;

    // Check if we need to recreate the Y/UV staging textures.
    if (g_yTex) {
        D3D11_TEXTURE2D_DESC d;
        g_yTex->GetDesc(&d);
        if ((int)d.Width != w || (int)d.Height != h) {
            g_ySrv->Release(); g_ySrv = nullptr;
            g_yTex->Release(); g_yTex = nullptr;
            g_uvSrv->Release(); g_uvSrv = nullptr;
            g_uvTex->Release(); g_uvTex = nullptr;
        }
    }

    if (!g_yTex) {
        D3D11_TEXTURE2D_DESC planeDesc = {};
        planeDesc.Width = w;
        planeDesc.Height = h;
        planeDesc.MipLevels = 1;
        planeDesc.ArraySize = 1;
        planeDesc.SampleDesc.Count = 1;
        planeDesc.Usage = D3D11_USAGE_DEFAULT;
        planeDesc.BindFlags = D3D11_BIND_SHADER_RESOURCE;

        planeDesc.Format = DXGI_FORMAT_R8_UNORM; // Y plane
        HRESULT hr = g_qtDevice->CreateTexture2D(&planeDesc, nullptr, &g_yTex);
        if (FAILED(hr)) return false;
        g_qtDevice->CreateShaderResourceView(g_yTex, nullptr, &g_ySrv);

        planeDesc.Width = w / 2;
        planeDesc.Height = h / 2;
        planeDesc.Format = DXGI_FORMAT_R8G8_UNORM; // UV interleaved
        hr = g_qtDevice->CreateTexture2D(&planeDesc, nullptr, &g_uvTex);
        if (FAILED(hr)) return false;
        g_qtDevice->CreateShaderResourceView(g_uvTex, nullptr, &g_uvSrv);
    }

    // Copy Y plane (element 0 from the NV12 texture array).
    D3D11_BOX yBox = { 0, 0, 0, (UINT)w, (UINT)h, 1 };
    g_qtContext->CopySubresourceRegion(g_yTex, 0, 0, 0, 0, srcTex, srcArrayIdx, &yBox);

    // Copy UV plane (element 1 from the texture array).
    D3D11_BOX uvBox = { 0, 0, 0, (UINT)w / 2, (UINT)h / 2, 1 };
    g_qtContext->CopySubresourceRegion(g_uvTex, 0, 0, 0, 0, srcTex, srcArrayIdx + 1, &uvBox);

    return true;
}
#endif // Q_OS_WIN
} // anonymous namespace

// ── C++ member functions ───────────────────────────────────────────────

D3D11VideoItem::D3D11VideoItem(QQuickItem *parent) : QQuickItem(parent) {
    g_activeItem = this;
    setFlag(ItemHasContents, true);
    connect(this, &QQuickItem::windowChanged, this, &D3D11VideoItem::handleWindowChanged);
}

D3D11VideoItem::~D3D11VideoItem() {
    g_activeItem = nullptr;
#if defined(Q_OS_WIN)
    releaseAllD3d();
#endif
}

bool D3D11VideoItem::d3d11Supported() const {
#if defined(Q_OS_WIN)
    return true;
#else
    return false;
#endif
}

bool D3D11VideoItem::d3d11Active() const { return m_d3d11Active; }

void D3D11VideoItem::setD3d11Active(bool active) {
    if (m_d3d11Active == active) return;
    m_d3d11Active = active;
    emit d3d11ActiveChanged();
    update();
}

void D3D11VideoItem::registerTypes() {
    qmlRegisterType<D3D11VideoItem>("com.lunaris.client.gpu", 1, 0, "D3D11VideoItem");
}

void D3D11VideoItem::handleWindowChanged(QQuickWindow *win) {
    if (win) {
        connect(win, &QQuickWindow::beforeRenderPassRecording,
                this, &D3D11VideoItem::renderNative, Qt::DirectConnection);
    }
}

QSGNode *D3D11VideoItem::updatePaintNode(QSGNode *oldNode, UpdatePaintNodeData *) {
    delete oldNode;
    return nullptr;
}

void D3D11VideoItem::renderNative() {
#if defined(Q_OS_WIN)
    if (!g_d3d11StreamActive || g_d3d11RenderFailed.load(std::memory_order_acquire))
        return;

    QQuickWindow *window = this->window();
    if (!window) return;

    // Get Qt's D3D11 device.
    if (!ensureQtD3D(window)) return;

    // Compile shaders (first call only).
    if (!compileShaders()) return;

    // Get the pending frame.
    PendingD3d11Frame frame;
    {
        std::lock_guard<std::mutex> lock(g_frameMutex);
        if (!g_pendingFrame.new_frame || !g_pendingFrame.texture) return;
        frame = g_pendingFrame;
        g_pendingFrame.new_frame = false;
    }
    if (frame.width <= 0 || frame.height <= 0) return;

    // Split NV12 planes from the decoder texture.
    if (!splitNv12Planes(frame.texture, frame.array_index, frame.width, frame.height)) {
        return;
    }

    // Render with beginExternalCommands → D3D11 → endExternalCommands.
    window->beginExternalCommands();

    // Save D3D11 state.
    ID3D11RenderTargetView *oldRtv = nullptr;
    ID3D11DepthStencilView *oldDsv = nullptr;
    g_qtContext->OMGetRenderTargets(1, &oldRtv, &oldDsv);

    // Get the Qt swapchain's current render target.
    ID3D11Texture2D *swapTex = nullptr;
    if (oldRtv) {
        ID3D11Resource *res = nullptr;
        oldRtv->GetResource(&res);
        swapTex = static_cast<ID3D11Texture2D*>(res);
    }

    // Viewport.
    QRectF rect = boundingRect();
    qreal dpr = window->effectiveDevicePixelRatio();
    D3D11_VIEWPORT vp = { (float)(rect.x() * dpr), (float)(rect.y() * dpr),
                          (float)(rect.width() * dpr), (float)(rect.height() * dpr),
                          0.0f, 1.0f };

    g_qtContext->RSSetViewports(1, &vp);

    // Pipeline state.
    g_qtContext->VSSetShader(g_vs, nullptr, 0);
    g_qtContext->PSSetShader(g_ps, nullptr, 0);
    g_qtContext->IASetInputLayout(g_inputLayout);
    g_qtContext->IASetPrimitiveTopology(D3D11_PRIMITIVE_TOPOLOGY_TRIANGLELIST);

    UINT stride = 16; // float2 pos + float2 uv
    UINT offset = 0;
    g_qtContext->IASetVertexBuffers(0, 1, &g_quadVB, &stride, &offset);

    g_qtContext->PSSetShaderResources(0, 1, &g_ySrv);
    g_qtContext->PSSetShaderResources(1, 1, &g_uvSrv);
    g_qtContext->PSSetSamplers(0, 1, &g_sampler);

    float blendFactor[4] = {1,1,1,1};
    g_qtContext->OMSetBlendState(g_blendState, blendFactor, 0xFFFFFFFF);
    g_qtContext->RSSetState(g_rasterState);
    g_qtContext->OMSetDepthStencilState(g_depthState, 0);

    g_qtContext->Draw(6, 0);

    // Restore old state.
    ID3D11ShaderResourceView *nullSrvs[2] = { nullptr, nullptr };
    g_qtContext->PSSetShaderResources(0, 2, nullSrvs);
    g_qtContext->OMSetRenderTargets(1, &oldRtv, oldDsv);
    if (oldRtv) oldRtv->Release();
    if (oldDsv) oldDsv->Release();
    if (swapTex) swapTex->Release();

    window->endExternalCommands();

    if (!m_d3d11Active) {
        m_d3d11Active = true;
        emit d3d11ActiveChanged();
    }
#endif
}

// ── Extern "C" FFI ─────────────────────────────────────────────────────

extern "C" bool deliver_d3d11_frame(uint64_t texture_ptr, int array_index,
                                     int width, int height, uint32_t format) {
#if defined(Q_OS_WIN)
    if (!g_d3d11StreamActive || g_d3d11RenderFailed.load(std::memory_order_acquire))
        return false;

    {
        std::lock_guard<std::mutex> lock(g_frameMutex);
        g_pendingFrame.texture = reinterpret_cast<ID3D11Texture2D*>(
            static_cast<uintptr_t>(texture_ptr));
        g_pendingFrame.array_index = static_cast<UINT>(array_index);
        g_pendingFrame.width = width;
        g_pendingFrame.height = height;
        g_pendingFrame.new_frame = true;
    }

    if (g_activeItem) {
        QMetaObject::invokeMethod(g_activeItem, "update", Qt::QueuedConnection);
    }
    return true;
#else
    Q_UNUSED(texture_ptr); Q_UNUSED(array_index);
    Q_UNUSED(width); Q_UNUSED(height); Q_UNUSED(format);
    return false;
#endif
}

extern "C" bool d3d11_render_failed() {
    return g_d3d11RenderFailed.load(std::memory_order_acquire);
}

extern "C" void set_d3d11_stream_active(bool active) {
    g_d3d11StreamActive = active;
    if (!active) {
#if defined(Q_OS_WIN)
        {
            std::lock_guard<std::mutex> lock(g_frameMutex);
            g_pendingFrame = PendingD3d11Frame{};
        }
        releaseD3dResources();
#endif
        if (g_activeItem) {
            QMetaObject::invokeMethod(g_activeItem, "setD3d11Active",
                                      Qt::QueuedConnection, Q_ARG(bool, false));
        }
    }
}
