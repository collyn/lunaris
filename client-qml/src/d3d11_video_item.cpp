// Direct3D 11 presentation of D3D11VA-decoded frames inside Qt Quick.
//
// This is a port of moonlight-qt's D3D11VA renderer (app/streaming/video/
// ffmpeg-renderers/d3d11va.cpp, renderer.h, ffmpeg.cpp, app/shaders/*.hlsl,
// commit 67dbfdd), the "separate devices, bind" path. moonlight-qt is
// GPL-3.0, the same family as this project's licence; the HLSL below is the
// text of Moonlight's shaders.
//
// What differs from Moonlight (Qt Quick owns the render device and the swap
// chain, see docs/video-rewrite-plan.md, Phase 1):
//  - the render device is Qt's own D3D11 device, the decode device is a second
//    device on the same adapter (setupSharedDevice, d3d11va.cpp:175-247);
//  - the quad is drawn into Qt's render pass (beforeRenderPassRecording) and
//    Qt's render target is kept; no swap chain code;
//  - Moonlight's "shared device, copy" fallback is not ported (it needs a lock
//    around every use of the render context, which Qt does not give us).

#include "d3d11_video_item.h"
#include "video_helper.h"

#include <QQuickWindow>
#include <QSGNode>
#include <QtQml/qqml.h>
#include <atomic>
#include <iostream>
#include <mutex>

#if defined(Q_OS_WIN)
#include <array>
#include <chrono>
#include <cstdio>
#include <string>
#include <condition_variable>
#include <memory>
#include <vector>

#include <d3d11_4.h>
#include <d3dcompiler.h>
#include <dxgi1_6.h>
#include <wrl/client.h>
#include <QSGRendererInterface>

extern "C" {
#include <libavcodec/avcodec.h>
#include <libavutil/hwcontext.h>
#include <libavutil/hwcontext_d3d11va.h>
#include <libavutil/pixdesc.h>
}
#endif

namespace {
std::atomic_bool g_d3d11RenderFailed{false};
std::atomic_bool g_d3d11StreamActive{false};
D3D11VideoItem *g_activeItem = nullptr;

// Only called from the Direct3D11 code paths (Q_OS_WIN).
[[maybe_unused]] void markD3d11Failed(const char *reason) {
    g_d3d11RenderFailed.store(true, std::memory_order_release);
    std::cerr << "Lunaris: D3D11 renderer disabled: " << reason << std::endl;
    if (g_activeItem) {
        QMetaObject::invokeMethod(g_activeItem, "setD3d11Active", Qt::QueuedConnection, Q_ARG(bool, false));
    }
}

#if defined(Q_OS_WIN)
using Microsoft::WRL::ComPtr;

// Asks the window to render once (any thread).
void requestWindowUpdate() {
    QQuickItem *item = g_activeItem;
    if (!item) return;
    QMetaObject::invokeMethod(item, [item]() {
        if (QQuickWindow *w = item->window()) w->update();
    }, Qt::QueuedConnection);
}

// ── Shaders: moonlight-qt app/shaders/*.hlsl, verbatim ─────────────────
// d3d11_vertex.hlsl
const char *kVertexShader = R"(struct ShaderInput
{
    float2 pos : POSITION;
    float2 tex : TEXCOORD0;
};

struct ShaderOutput
{
    float4 pos : SV_POSITION;
    float2 tex : TEXCOORD0;
};

ShaderOutput main(ShaderInput input)
{
    ShaderOutput output;
    output.pos = float4(input.pos, 0.0, 1.0);
    output.tex = input.tex;
    return output;
})";

// d3d11_yuv420_pixel.hlsl
const char *kPixelShaderYuv420 = R"(Texture2D<min16float> luminancePlane : register(t0);
Texture2D<min16float2> chrominancePlane : register(t1);
SamplerState theSampler : register(s0);

struct ShaderInput
{
    float4 pos : SV_POSITION;
    float2 tex : TEXCOORD0;
};

cbuffer CSC_CONST_BUF : register(b0)
{
    min16float3x3 cscMatrix;
    min16float3 offsets;
    min16float2 chromaOffset;
    min16float2 chromaTexMax;
};

min16float4 main(ShaderInput input) : SV_TARGET
{
    // Clamp the chrominance texcoords to avoid sampling the row of texels adjacent to the alignment padding
    min16float3 yuv = min16float3(luminancePlane.Sample(theSampler, input.tex),
                                  chrominancePlane.Sample(theSampler, min(input.tex + chromaOffset, chromaTexMax.rg)));

    // Subtract the YUV offset for limited vs full range
    yuv -= offsets;

    // Multiply by the conversion matrix for this colorspace
    yuv = mul(yuv, cscMatrix);

    return min16float4(yuv, 1.0);
})";

// d3d11_yuv444_pixel_start.hlsli
const char *kPixelShaderYuv444Start = R"(Texture2D<min16float3> videoTex : register(t0);
SamplerState theSampler : register(s0);

struct ShaderInput
{
    float4 pos : SV_POSITION;
    float2 tex : TEXCOORD0;
};

cbuffer CSC_CONST_BUF : register(b0)
{
    min16float3x3 cscMatrix;
    min16float3 offsets;
    min16float2 chromaOffset; // Unused for 4:4:4
    min16float2 chromaTexMax; // Unused for 4:4:4
};
)";

// d3d11_yuv444_pixel_end.hlsli
const char *kPixelShaderYuv444End = R"(
min16float4 main(ShaderInput input) : SV_TARGET
{
    min16float3 yuv = swizzle(videoTex.Sample(theSampler, input.tex));

    // Subtract the YUV offset for limited vs full range
    yuv -= offsets;

    // Multiply by the conversion matrix for this colorspace
    yuv = mul(yuv, cscMatrix);

    return min16float4(yuv, 1.0);
})";

// d3d11_ayuv_pixel.hlsl: start + this + end
const char *kPixelShaderAyuvSwizzle = R"(
min16float3 swizzle(min16float3 input)
{
    // AYUV SRVs are in VUYA order
    return input.bgr;
}
)";

// d3d11_y410_pixel.hlsl: start + this + end
const char *kPixelShaderY410Swizzle = R"(
min16float3 swizzle(min16float3 input)
{
    // Y410 SRVs are in UYVA order
    return input.grb;
}
)";

// moonlight-qt d3d11va.cpp:52-53 (D3D11VARenderer::PixelShaders)
enum PixelShaders {
    GENERIC_YUV_420,
    GENERIC_AYUV,
    GENERIC_Y410,
    PIXEL_SHADERS_COUNT
};

// moonlight-qt d3d11va.cpp:28-49
typedef struct _VERTEX
{
    float x, y;
    float tu, tv;
} VERTEX, *PVERTEX;

#define CSC_MATRIX_RAW_ELEMENT_COUNT 9
#define CSC_MATRIX_PACKED_ELEMENT_COUNT 12
#define OFFSETS_ELEMENT_COUNT 3

typedef struct _CSC_CONST_BUF
{
    // CscMatrix value from above but packed and scaled
    float cscMatrix[CSC_MATRIX_PACKED_ELEMENT_COUNT];

    // YUV offset values
    float offsets[OFFSETS_ELEMENT_COUNT];

    // Padding float to end 16-byte boundary
    float padding;

    // Chroma offset values
    float chromaOffset[2];

    // Max UV coordinates to avoid sampling alignment padding
    float chromaUVMax[2];
} CSC_CONST_BUF, *PCSC_CONST_BUF;
static_assert(sizeof(CSC_CONST_BUF) % 16 == 0, "Constant buffer sizes must be a multiple of 16");

// moonlight-qt d3d11va.h:62-66 (SupportedFenceType)
enum class SupportedFenceType {
    None,
    NonMonitored,
    Monitored,
};

// ── Colour maths: moonlight-qt renderer.h ──────────────────────────────
enum { COLORSPACE_REC_601, COLORSPACE_REC_709, COLORSPACE_REC_2020 };

// moonlight-qt renderer.h:245-261 (getFrameColorspace). An untagged frame is
// assumed to have the colourspace that was requested of the host
// (requested_csc_mode(), bits 1.. of encoderCscMode; same as frame_color() in
// decoder.rs).
int getFrameColorspace(const AVFrame *frame) {
    switch (frame->colorspace) {
    case AVCOL_SPC_SMPTE170M:
    case AVCOL_SPC_BT470BG:
        return COLORSPACE_REC_601;
    case AVCOL_SPC_BT709:
        return COLORSPACE_REC_709;
    case AVCOL_SPC_BT2020_NCL:
    case AVCOL_SPC_BT2020_CL:
        return COLORSPACE_REC_2020;
    default:
        return requested_csc_mode() >> 1;
    }
}

// moonlight-qt renderer.h:263-274 (isFrameFullRange). An untagged frame is
// assumed to have the range that was requested of the host
// (requested_csc_mode() & 1), the same as frame_color() in decoder.rs.
bool isFrameFullRange(const AVFrame *frame) {
    switch (frame->color_range) {
    case AVCOL_RANGE_JPEG:
        return true;
    case AVCOL_RANGE_MPEG:
        return false;
    default:
        return (requested_csc_mode() & 1) != 0;
    }
}

// moonlight-qt renderer.h:361-368
AVPixelFormat getFrameSwPixelFormat(const AVFrame *frame) {
    // For hwaccel formats, we want to get the real underlying format
    if (frame->hw_frames_ctx) {
        return ((AVHWFramesContext *)frame->hw_frames_ctx->data)->sw_format;
    }
    return (AVPixelFormat)frame->format;
}

// moonlight-qt renderer.h:372-382
int getFrameBitsPerChannel(const AVFrame *frame) {
    const AVPixFmtDescriptor *formatDesc = av_pix_fmt_desc_get(getFrameSwPixelFormat(frame));
    if (!formatDesc) {
        // This shouldn't be possible but handle it anyway
        return 8;
    }

    // This assumes plane 0 is exclusively the Y component
    return formatDesc->comp[0].depth;
}

// moonlight-qt renderer.h:384-437
void getFramePremultipliedCscConstants(const AVFrame *frame, std::array<float, 9> &cscMatrix, std::array<float, 3> &offsets) {
    static const std::array<float, 9> k_CscMatrix_Bt601 = {
        1.0f, 1.0f, 1.0f,
        0.0f, -0.3441f, 1.7720f,
        1.4020f, -0.7141f, 0.0f,
    };
    static const std::array<float, 9> k_CscMatrix_Bt709 = {
        1.0f, 1.0f, 1.0f,
        0.0f, -0.1873f, 1.8556f,
        1.5748f, -0.4681f, 0.0f,
    };
    static const std::array<float, 9> k_CscMatrix_Bt2020 = {
        1.0f, 1.0f, 1.0f,
        0.0f, -0.1646f, 1.8814f,
        1.4746f, -0.5714f, 0.0f,
    };

    bool fullRange = isFrameFullRange(frame);
    int bitsPerChannel = getFrameBitsPerChannel(frame);
    int channelRange = (1 << bitsPerChannel);
    double yMin = (fullRange ? 0 : (16 << (bitsPerChannel - 8)));
    double yMax = (fullRange ? (channelRange - 1) : (235 << (bitsPerChannel - 8)));
    double yScale = (channelRange - 1) / (yMax - yMin);
    double uvMin = (fullRange ? 0 : (16 << (bitsPerChannel - 8)));
    double uvMax = (fullRange ? (channelRange - 1) : (240 << (bitsPerChannel - 8)));
    double uvScale = (channelRange - 1) / (uvMax - uvMin);

    // Calculate YUV offsets
    offsets[0] = yMin / (double)(channelRange - 1);
    offsets[1] = (channelRange / 2) / (double)(channelRange - 1);
    offsets[2] = (channelRange / 2) / (double)(channelRange - 1);

    // Start with the standard full range color matrix
    switch (getFrameColorspace(frame)) {
    default:
    case COLORSPACE_REC_601:
        cscMatrix = k_CscMatrix_Bt601;
        break;
    case COLORSPACE_REC_709:
        cscMatrix = k_CscMatrix_Bt709;
        break;
    case COLORSPACE_REC_2020:
        cscMatrix = k_CscMatrix_Bt2020;
        break;
    }

    // Scale the color matrix according to the color range
    for (int i = 0; i < 3; i++) {
        cscMatrix[i] *= yScale;
    }
    for (int i = 3; i < 9; i++) {
        cscMatrix[i] *= uvScale;
    }
}

// moonlight-qt renderer.h:439-486
void getFrameChromaCositingOffsets(const AVFrame *frame, std::array<float, 2> &chromaOffsets) {
    const AVPixFmtDescriptor *formatDesc = av_pix_fmt_desc_get(getFrameSwPixelFormat(frame));
    if (!formatDesc) {
        chromaOffsets.fill(0);
        return;
    }

    switch (frame->chroma_location) {
    default:
    case AVCHROMA_LOC_LEFT:
        chromaOffsets[0] = 0.5;
        chromaOffsets[1] = 0;
        break;
    case AVCHROMA_LOC_CENTER:
        chromaOffsets[0] = 0;
        chromaOffsets[1] = 0;
        break;
    case AVCHROMA_LOC_TOPLEFT:
        chromaOffsets[0] = 0.5;
        chromaOffsets[1] = 0.5;
        break;
    case AVCHROMA_LOC_TOP:
        chromaOffsets[0] = 0;
        chromaOffsets[1] = 0.5;
        break;
    case AVCHROMA_LOC_BOTTOMLEFT:
        chromaOffsets[0] = 0.5;
        chromaOffsets[1] = -0.5;
        break;
    case AVCHROMA_LOC_BOTTOM:
        chromaOffsets[0] = 0;
        chromaOffsets[1] = -0.5;
        break;
    }

    // Force the offsets to 0 if chroma is not subsampled in that dimension
    if (formatDesc->log2_chroma_w == 0) {
        chromaOffsets[0] = 0;
    }
    if (formatDesc->log2_chroma_h == 0) {
        chromaOffsets[1] = 0;
    }
}

// ── Shared state ───────────────────────────────────────────────────────

// Qt's render device and its immediate context (d3d11va.h:73-74 types).
// The device is published to the decoder thread; the context is only used on
// the render thread.
std::mutex g_qtDeviceMutex;
std::condition_variable g_qtDeviceCv;
ComPtr<ID3D11Device5> g_renderDevice;
ComPtr<ID3D11DeviceContext4> g_renderContext;
QQuickWindow *g_window = nullptr;

// Serialises the render hook with releaseRenderResources().
std::mutex g_renderMutex;

// FFmpeg's lock for the decode device context (d3d11va.cpp:1501-1512; SDL
// mutexes are recursive).
std::recursive_mutex g_decodeContextLock;
void lockContext(void *) { g_decodeContextLock.lock(); }
void unlockContext(void *) { g_decodeContextLock.unlock(); }

// The decode device and the fences of the current stream (d3d11va.h:68-83).
struct DecodeSession {
    ComPtr<ID3D11Device5> decodeDevice;
    ComPtr<ID3D11DeviceContext4> decodeContext;
    ComPtr<ID3D11Fence> decodeD2RFence, renderD2RFence;
    UINT64 d2rFenceValue = 1;
    ComPtr<ID3D11Fence> decodeR2DFence, renderR2DFence;
    UINT64 r2dFenceValue = 1;
};

// Render side of the current frames context (m_RenderSharedTextureArray,
// m_TextureFormat, m_VideoTextureResourceViews).
struct FrameResources {
    const void *framesCtx = nullptr; // AVHWFramesContext* these views belong to
    ComPtr<ID3D11Texture2D> sharedTextureArray;
    DXGI_FORMAT textureFormat = DXGI_FORMAT_UNKNOWN;
    std::vector<std::array<ComPtr<ID3D11ShaderResourceView>, 2>> srvs;
};

// Written by the decoder thread (setup, get_format), read by the render thread.
std::mutex g_sessionMutex;
std::shared_ptr<DecodeSession> g_session;
std::shared_ptr<FrameResources> g_frameResources;

// Frame hand-off. The pending frame is the newest one not rendered yet; the
// displayed frame stays referenced until the next one is rendered (what
// Moonlight's Pacer does; d3d11va.cpp:1083-1086).
std::mutex g_frameMutex;
AVFrame *g_pendingFrame = nullptr;
AVFrame *g_displayedFrame = nullptr;

// Set when the stream ends: the render thread drops its per-stream state.
std::atomic_bool g_resetRenderState{false};

// ── Render-thread-only state ───────────────────────────────────────────
ComPtr<ID3D11VertexShader> g_vertexShader;
ComPtr<ID3D11InputLayout> g_inputLayout;
std::array<ComPtr<ID3D11PixelShader>, PIXEL_SHADERS_COUNT> g_videoPixelShaders;
ComPtr<ID3D11SamplerState> g_sampler;
ComPtr<ID3D11Buffer> g_indexBuffer;
ComPtr<ID3D11BlendState> g_videoBlendState;
// Not in Moonlight (its swap chain has no depth buffer and default state):
// Qt's render target comes with a depth buffer, so depth is turned off, and
// the rasterizer state is the D3D11 default without scissor.
ComPtr<ID3D11DepthStencilState> g_depthState;
ComPtr<ID3D11RasterizerState> g_rasterState;
ComPtr<ID3D11Buffer> g_videoVertexBuffer;
ComPtr<ID3D11Buffer> g_cscConstantBuffer;
bool g_shadersReady = false;
bool g_reportedActive = false;
const void *g_lastSession = nullptr;

// hasFrameFormatChanged() state, moonlight-qt renderer.h:565-572
int g_lastFrameWidth = 0;
int g_lastFrameHeight = 0;
AVPixelFormat g_lastFramePixelFormat = AV_PIX_FMT_NONE;
AVColorRange g_lastColorRange = AVCOL_RANGE_UNSPECIFIED;
AVColorPrimaries g_lastColorPrimaries = AVCOL_PRI_UNSPECIFIED;
AVColorTransferCharacteristic g_lastColorTrc = AVCOL_TRC_UNSPECIFIED;
AVColorSpace g_lastColorSpace = AVCOL_SPC_UNSPECIFIED;
AVChromaLocation g_lastChromaLocation = AVCHROMA_LOC_UNSPECIFIED;

// moonlight-qt renderer.h:488-512
bool hasFrameFormatChanged(const AVFrame *frame) {
    AVPixelFormat format = getFrameSwPixelFormat(frame);
    if (frame->width == g_lastFrameWidth &&
        frame->height == g_lastFrameHeight &&
        format == g_lastFramePixelFormat &&
        frame->color_range == g_lastColorRange &&
        frame->color_primaries == g_lastColorPrimaries &&
        frame->color_trc == g_lastColorTrc &&
        frame->colorspace == g_lastColorSpace &&
        frame->chroma_location == g_lastChromaLocation) {
        return false;
    }

    g_lastFrameWidth = frame->width;
    g_lastFrameHeight = frame->height;
    g_lastFramePixelFormat = format;
    g_lastColorRange = frame->color_range;
    g_lastColorPrimaries = frame->color_primaries;
    g_lastColorTrc = frame->color_trc;
    g_lastColorSpace = frame->colorspace;
    g_lastChromaLocation = frame->chroma_location;
    return true;
}

void freeFrame(AVFrame *&frame) {
    if (frame) av_frame_free(&frame);
}

// Drops the pending and displayed frames (any thread).
void dropHeldFrames() {
    AVFrame *pending = nullptr, *displayed = nullptr;
    {
        std::lock_guard<std::mutex> lock(g_frameMutex);
        pending = g_pendingFrame;
        displayed = g_displayedFrame;
        g_pendingFrame = nullptr;
        g_displayedFrame = nullptr;
    }
    freeFrame(pending);
    freeFrame(displayed);
}

// Per-stream render objects; the device-level ones (shaders, states) stay.
void resetStreamRenderState() {
    g_videoVertexBuffer.Reset();
    g_cscConstantBuffer.Reset();
    g_lastFrameWidth = 0;
    g_lastFrameHeight = 0;
    g_lastFramePixelFormat = AV_PIX_FMT_NONE;
    g_lastColorRange = AVCOL_RANGE_UNSPECIFIED;
    g_lastColorPrimaries = AVCOL_PRI_UNSPECIFIED;
    g_lastColorTrc = AVCOL_TRC_UNSPECIFIED;
    g_lastColorSpace = AVCOL_SPC_UNSPECIFIED;
    g_lastChromaLocation = AVCHROMA_LOC_UNSPECIFIED;
    g_reportedActive = false;
}

// Everything created on Qt's device. Called with g_renderMutex held.
void releaseAllRenderObjects() {
    resetStreamRenderState();
    for (auto &shader : g_videoPixelShaders) shader.Reset();
    g_vertexShader.Reset();
    g_inputLayout.Reset();
    g_sampler.Reset();
    g_indexBuffer.Reset();
    g_videoBlendState.Reset();
    g_depthState.Reset();
    g_rasterState.Reset();
    g_shadersReady = false;
    if (g_renderContext) {
        g_renderContext.Reset();
    }
    {
        std::lock_guard<std::mutex> lock(g_qtDeviceMutex);
        g_renderDevice.Reset();
    }
    g_window = nullptr;
}

/// Captures Qt's D3D11 device (render thread). Runs on every render, whether or
/// not a frame is pending, so that the decoder thread can find the device
/// before the stream starts.
void captureQtDevice(QQuickWindow *window) {
    if (g_renderContext && g_window == window) return;

    QSGRendererInterface *rif = window->rendererInterface();
    if (!rif || rif->graphicsApi() != QSGRendererInterface::Direct3D11) return;

    void *devPtr = rif->getResource(window, QSGRendererInterface::DeviceResource);
    if (!devPtr) return;

    ComPtr<ID3D11Device> device(static_cast<ID3D11Device *>(devPtr));
    ComPtr<ID3D11Device5> device5;
    if (FAILED(device.As(&device5))) {
        std::cerr << "Lunaris: D3D11 present: Qt's device does not support ID3D11Device5" << std::endl;
        return;
    }
    ComPtr<ID3D11DeviceContext> context;
    device->GetImmediateContext(&context);
    ComPtr<ID3D11DeviceContext4> context4;
    if (FAILED(context.As(&context4))) {
        std::cerr << "Lunaris: D3D11 present: Qt's context does not support ID3D11DeviceContext4" << std::endl;
        return;
    }

    g_renderContext = context4;
    g_window = window;
    {
        std::lock_guard<std::mutex> lock(g_qtDeviceMutex);
        g_renderDevice = device5;
    }
    g_qtDeviceCv.notify_all();
}

// moonlight-qt d3d11va.cpp:134-173
bool createSharedFencePair(SupportedFenceType fenceType, UINT64 initialValue, ID3D11Device5 *dev1, ID3D11Device5 *dev2,
                           ComPtr<ID3D11Fence> &dev1Fence, ComPtr<ID3D11Fence> &dev2Fence) {
    HRESULT hr;
    D3D11_FENCE_FLAG flags;

    flags = D3D11_FENCE_FLAG_SHARED;
    if (fenceType == SupportedFenceType::NonMonitored) {
        flags |= D3D11_FENCE_FLAG_NON_MONITORED;
    }

    hr = dev1->CreateFence(initialValue, flags, IID_PPV_ARGS(&dev1Fence));
    if (FAILED(hr)) {
        std::cerr << "Lunaris: ID3D11Device5::CreateFence() failed: " << std::hex << hr << std::dec << std::endl;
        return false;
    }

    HANDLE fenceHandle;
    hr = dev1Fence->CreateSharedHandle(nullptr, GENERIC_ALL, nullptr, &fenceHandle);
    if (FAILED(hr)) {
        std::cerr << "Lunaris: ID3D11Fence::CreateSharedHandle() failed: " << std::hex << hr << std::dec << std::endl;
        dev1Fence.Reset();
        return false;
    }

    hr = dev2->OpenSharedFence(fenceHandle, IID_PPV_ARGS(&dev2Fence));
    CloseHandle(fenceHandle);
    if (FAILED(hr)) {
        std::cerr << "Lunaris: ID3D11Device5::OpenSharedFence() failed: " << std::hex << hr << std::dec << std::endl;
        dev1Fence.Reset();
        return false;
    }

    return true;
}

// moonlight-qt d3d11va.cpp:175-247. Returns null when anything fails.
std::shared_ptr<DecodeSession> setupSharedDevice(IDXGIAdapter1 *adapter, SupportedFenceType fenceType,
                                                 ID3D11Device5 *renderDevice) {
    const D3D_FEATURE_LEVEL supportedFeatureLevels[] = { D3D_FEATURE_LEVEL_11_1, D3D_FEATURE_LEVEL_11_0 };
    D3D_FEATURE_LEVEL featureLevel;
    HRESULT hr;
    ComPtr<ID3D11Device> device;
    ComPtr<ID3D11DeviceContext> deviceContext;

    // We don't support cross-device sharing without fences
    if (fenceType == SupportedFenceType::None) {
        return nullptr;
    }

    auto session = std::make_shared<DecodeSession>();

    // If we're going to use separate devices for decoding and rendering, create the decoding device
    hr = D3D11CreateDevice(adapter,
                           D3D_DRIVER_TYPE_UNKNOWN,
                           nullptr,
                           D3D11_CREATE_DEVICE_VIDEO_SUPPORT,
                           supportedFeatureLevels,
                           ARRAYSIZE(supportedFeatureLevels),
                           D3D11_SDK_VERSION,
                           &device,
                           &featureLevel,
                           &deviceContext);
    if (FAILED(hr)) {
        std::cerr << "Lunaris: D3D11CreateDevice() failed: " << std::hex << hr << std::dec << std::endl;
        return nullptr;
    }

    hr = device.As(&session->decodeDevice);
    if (FAILED(hr)) {
        std::cerr << "Lunaris: ID3D11Device::QueryInterface(ID3D11Device5) failed: " << std::hex << hr << std::dec << std::endl;
        return nullptr;
    }

    hr = deviceContext.As(&session->decodeContext);
    if (FAILED(hr)) {
        std::cerr << "Lunaris: ID3D11DeviceContext::QueryInterface(ID3D11DeviceContext4) failed: " << std::hex << hr << std::dec << std::endl;
        return nullptr;
    }

    // Create our decode->render fence
    session->d2rFenceValue = 1;
    if (!createSharedFencePair(fenceType, 0, session->decodeDevice.Get(), renderDevice, session->decodeD2RFence, session->renderD2RFence)) {
        return nullptr;
    }

    // Create our render->decode fence
    session->r2dFenceValue = 1;
    if (!createSharedFencePair(fenceType, 0, session->decodeDevice.Get(), renderDevice, session->decodeR2DFence, session->renderR2DFence)) {
        return nullptr;
    }

    return session;
}

// moonlight-qt d3d11va.cpp:1828-1841 (getVideoTextureSRVFormats). Moonlight
// decides 4:4:4 / 10-bit from the video format mask; here the format of the
// decoder's texture says the same (NV12 / P010 are 4:2:0, AYUV / Y410 are
// 4:4:4). Empty for any other format.
std::vector<DXGI_FORMAT> getVideoTextureSRVFormats(DXGI_FORMAT textureFormat) {
    switch (textureFormat) {
    case DXGI_FORMAT_AYUV:
        // YUV 4:4:4 formats don't use a second SRV
        return { DXGI_FORMAT_R8G8B8A8_UNORM };
    case DXGI_FORMAT_Y410:
        return { DXGI_FORMAT_R10G10B10A2_UNORM };
    case DXGI_FORMAT_P010:
        return { DXGI_FORMAT_R16_UNORM, DXGI_FORMAT_R16G16_UNORM };
    case DXGI_FORMAT_NV12:
        return { DXGI_FORMAT_R8_UNORM, DXGI_FORMAT_R8G8_UNORM };
    default:
        return {};
    }
}

// moonlight-qt d3d11va.cpp:1897-1940 (setupTexturePoolViews)
bool setupTexturePoolViews(ID3D11Device *renderDevice, AVHWFramesContext *framesContext, FrameResources &res) {
    AVD3D11VAFramesContext *d3d11vaFramesContext = (AVD3D11VAFramesContext *)framesContext->hwctx;

    D3D11_SHADER_RESOURCE_VIEW_DESC srvDesc = {};
    srvDesc.ViewDimension = D3D11_SRV_DIMENSION_TEXTURE2DARRAY;
    srvDesc.Texture2DArray.MostDetailedMip = 0;
    srvDesc.Texture2DArray.MipLevels = 1;
    srvDesc.Texture2DArray.ArraySize = 1;

    res.srvs.resize(framesContext->initial_pool_size);

    // Create luminance and chrominance SRVs for each texture in the pool
    for (int i = 0; i < framesContext->initial_pool_size; i++) {
        HRESULT hr;

        srvDesc.Texture2DArray.FirstArraySlice = d3d11vaFramesContext->texture_infos[i].index;

        size_t srvIndex = 0;
        for (DXGI_FORMAT srvFormat : getVideoTextureSRVFormats(res.textureFormat)) {
            srvDesc.Format = srvFormat;
            hr = renderDevice->CreateShaderResourceView(res.sharedTextureArray.Get(),
                                                        &srvDesc,
                                                        &res.srvs[i][srvIndex]);
            if (FAILED(hr)) {
                std::cerr << "Lunaris: ID3D11Device::CreateShaderResourceView() failed: " << std::hex << hr << std::dec << std::endl;
                return false;
            }

            srvIndex++;
        }
    }

    return true;
}

// moonlight-qt d3d11va.cpp:1767-1826 (setupFrameRenderingResources), the
// separate-devices + bind branch. NB: this can be called more than once (and
// with different frame dimensions).
bool setupFrameRenderingResources(ID3D11Device5 *renderDevice, AVHWFramesContext *framesContext) {
    auto d3d11vaFramesContext = (AVD3D11VAFramesContext *)framesContext->hwctx;
    auto res = std::make_shared<FrameResources>();
    res->framesCtx = framesContext;

    // Open the decoder texture array on the renderer device
    ComPtr<IDXGIResource1> dxgiDecoderResource;

    HRESULT hr = d3d11vaFramesContext->texture_infos->texture->QueryInterface(IID_PPV_ARGS(&dxgiDecoderResource));
    if (FAILED(hr)) {
        std::cerr << "Lunaris: ID3D11Texture2D::QueryInterface(IDXGIResource1) failed: " << std::hex << hr << std::dec << std::endl;
        return false;
    }

    HANDLE sharedHandle;
    hr = dxgiDecoderResource->CreateSharedHandle(nullptr, DXGI_SHARED_RESOURCE_READ, nullptr, &sharedHandle);
    if (FAILED(hr)) {
        std::cerr << "Lunaris: IDXGIResource1::CreateSharedHandle() failed: " << std::hex << hr << std::dec << std::endl;
        return false;
    }

    hr = renderDevice->OpenSharedResource1(sharedHandle, IID_PPV_ARGS(&res->sharedTextureArray));
    CloseHandle(sharedHandle);
    if (FAILED(hr)) {
        std::cerr << "Lunaris: ID3D11Device1::OpenSharedResource1() failed: " << std::hex << hr << std::dec << std::endl;
        return false;
    }

    // Query the format of the underlying texture array
    D3D11_TEXTURE2D_DESC textureDesc;
    res->sharedTextureArray->GetDesc(&textureDesc);
    res->textureFormat = textureDesc.Format;

    if (getVideoTextureSRVFormats(res->textureFormat).empty()) {
        std::cerr << "Lunaris: unsupported decoder texture format " << res->textureFormat << std::endl;
        return false;
    }

    // Create SRVs for all textures in the decoder pool
    if (!setupTexturePoolViews(renderDevice, framesContext, *res)) {
        return false;
    }

    std::lock_guard<std::mutex> lock(g_sessionMutex);
    g_frameResources = res;
    return true;
}

// Render objects on Qt's device: moonlight-qt d3d11va.cpp:1557-1717
// (setupRenderingResources) without the overlay and swap chain parts.
bool setupRenderingResources(ID3D11Device *device) {
    HRESULT hr;
    ComPtr<ID3DBlob> vsBlob, errBlob;

    // The shaders are compiled at run time; Moonlight ships them precompiled with
    // `fxc /T vs_5_0 /O3` / `fxc /T ps_5_0 /O3` (app/shaders/build_hlsl.bat).
    hr = D3DCompile(kVertexShader, strlen(kVertexShader), "d3d11_vertex.hlsl", nullptr, nullptr,
                    "main", "vs_5_0", D3DCOMPILE_OPTIMIZATION_LEVEL3, 0, &vsBlob, &errBlob);
    if (FAILED(hr)) {
        if (errBlob) std::cerr << "Lunaris: D3D11 VS: " << (char *)errBlob->GetBufferPointer() << std::endl;
        markD3d11Failed("vertex shader compile failed");
        return false;
    }

    hr = device->CreateVertexShader(vsBlob->GetBufferPointer(), vsBlob->GetBufferSize(), nullptr, &g_vertexShader);
    if (FAILED(hr)) {
        markD3d11Failed("ID3D11Device::CreateVertexShader() failed");
        return false;
    }

    const D3D11_INPUT_ELEMENT_DESC vertexDesc[] =
    {
        { "POSITION", 0, DXGI_FORMAT_R32G32_FLOAT, 0, 0, D3D11_INPUT_PER_VERTEX_DATA, 0 },
        { "TEXCOORD", 0, DXGI_FORMAT_R32G32_FLOAT, 0, 8, D3D11_INPUT_PER_VERTEX_DATA, 0 },
    };
    hr = device->CreateInputLayout(vertexDesc, ARRAYSIZE(vertexDesc), vsBlob->GetBufferPointer(), vsBlob->GetBufferSize(), &g_inputLayout);
    if (FAILED(hr)) {
        markD3d11Failed("ID3D11Device::CreateInputLayout() failed");
        return false;
    }

    const std::string ayuv = std::string(kPixelShaderYuv444Start) + kPixelShaderAyuvSwizzle + kPixelShaderYuv444End;
    const std::string y410 = std::string(kPixelShaderYuv444Start) + kPixelShaderY410Swizzle + kPixelShaderYuv444End;
    const std::array<std::pair<const char *, std::string>, PIXEL_SHADERS_COUNT> sources = {{
        { "d3d11_yuv420_pixel.hlsl", kPixelShaderYuv420 },
        { "d3d11_ayuv_pixel.hlsl", ayuv },
        { "d3d11_y410_pixel.hlsl", y410 },
    }};
    for (int i = 0; i < PIXEL_SHADERS_COUNT; i++) {
        ComPtr<ID3DBlob> psBlob;
        errBlob.Reset();
        hr = D3DCompile(sources[i].second.c_str(), sources[i].second.size(), sources[i].first, nullptr, nullptr,
                        "main", "ps_5_0", D3DCOMPILE_OPTIMIZATION_LEVEL3, 0, &psBlob, &errBlob);
        if (FAILED(hr)) {
            if (errBlob) std::cerr << "Lunaris: D3D11 PS: " << (char *)errBlob->GetBufferPointer() << std::endl;
            markD3d11Failed("pixel shader compile failed");
            return false;
        }

        hr = device->CreatePixelShader(psBlob->GetBufferPointer(), psBlob->GetBufferSize(), nullptr, &g_videoPixelShaders[i]);
        if (FAILED(hr)) {
            markD3d11Failed("ID3D11Device::CreatePixelShader() failed");
            return false;
        }
    }

    // We use a common sampler for all pixel shaders
    {
        D3D11_SAMPLER_DESC samplerDesc = {};
        samplerDesc.Filter = D3D11_FILTER_MIN_MAG_MIP_LINEAR;
        samplerDesc.AddressU = D3D11_TEXTURE_ADDRESS_CLAMP;
        samplerDesc.AddressV = D3D11_TEXTURE_ADDRESS_CLAMP;
        samplerDesc.AddressW = D3D11_TEXTURE_ADDRESS_CLAMP;
        samplerDesc.MipLODBias = 0.0f;
        samplerDesc.MaxAnisotropy = 1;
        samplerDesc.ComparisonFunc = D3D11_COMPARISON_ALWAYS;
        samplerDesc.MinLOD = 0.0f;
        samplerDesc.MaxLOD = D3D11_FLOAT32_MAX;

        hr = device->CreateSamplerState(&samplerDesc, &g_sampler);
        if (FAILED(hr)) {
            markD3d11Failed("ID3D11Device::CreateSamplerState() failed");
            return false;
        }
    }

    // We use a common index buffer for all geometry
    {
        const int indexes[] = {0, 1, 2, 3, 2, 1};
        D3D11_BUFFER_DESC indexBufferDesc = {};
        indexBufferDesc.ByteWidth = sizeof(indexes);
        indexBufferDesc.Usage = D3D11_USAGE_IMMUTABLE;
        indexBufferDesc.BindFlags = D3D11_BIND_INDEX_BUFFER;
        indexBufferDesc.CPUAccessFlags = 0;
        indexBufferDesc.MiscFlags = 0;
        indexBufferDesc.StructureByteStride = sizeof(int);

        D3D11_SUBRESOURCE_DATA indexBufferData = {};
        indexBufferData.pSysMem = indexes;
        indexBufferData.SysMemPitch = sizeof(int);

        hr = device->CreateBuffer(&indexBufferDesc, &indexBufferData, &g_indexBuffer);
        if (FAILED(hr)) {
            markD3d11Failed("ID3D11Device::CreateBuffer() failed");
            return false;
        }
    }

    // Create our video blend state
    {
        D3D11_BLEND_DESC blendDesc = {};
        blendDesc.AlphaToCoverageEnable = FALSE;
        blendDesc.IndependentBlendEnable = FALSE;
        blendDesc.RenderTarget[0].BlendEnable = FALSE;
        blendDesc.RenderTarget[0].RenderTargetWriteMask = D3D11_COLOR_WRITE_ENABLE_ALL;

        hr = device->CreateBlendState(&blendDesc, &g_videoBlendState);
        if (FAILED(hr)) {
            markD3d11Failed("ID3D11Device::CreateBlendState() failed");
            return false;
        }
    }

    // Qt-specific state, see the comment at g_depthState.
    {
        D3D11_DEPTH_STENCIL_DESC depthDesc = {};
        depthDesc.DepthEnable = FALSE;
        depthDesc.StencilEnable = FALSE;
        hr = device->CreateDepthStencilState(&depthDesc, &g_depthState);
        if (FAILED(hr)) {
            markD3d11Failed("ID3D11Device::CreateDepthStencilState() failed");
            return false;
        }

        D3D11_RASTERIZER_DESC rastDesc = {};
        rastDesc.FillMode = D3D11_FILL_SOLID;
        rastDesc.CullMode = D3D11_CULL_BACK;
        rastDesc.DepthClipEnable = TRUE;
        hr = device->CreateRasterizerState(&rastDesc, &g_rasterState);
        if (FAILED(hr)) {
            markD3d11Failed("ID3D11Device::CreateRasterizerState() failed");
            return false;
        }
    }

    g_shadersReady = true;
    return true;
}

// moonlight-qt d3d11va.cpp:888-942 (bindVideoVertexBuffer). The quad fills
// the item's viewport (Lunaris letterboxes in QML) instead of scaling the
// source into the window.
void bindVideoVertexBuffer(ID3D11Device *device, ID3D11DeviceContext *ctx, bool frameChanged, AVFrame *frame) {
    if (frameChanged || !g_videoVertexBuffer) {
        // Normalized device coordinates of the whole viewport
        struct { float x, y, w, h; } renderRect = { -1.0f, -1.0f, 2.0f, 2.0f };

        // Don't sample from the alignment padding area
        auto framesContext = (AVHWFramesContext *)frame->hw_frames_ctx->data;
        float uMax = (float)frame->width / framesContext->width;
        float vMax = (float)frame->height / framesContext->height;

        VERTEX verts[] =
        {
            {renderRect.x, renderRect.y, 0, vMax},
            {renderRect.x, renderRect.y+renderRect.h, 0, 0},
            {renderRect.x+renderRect.w, renderRect.y, uMax, vMax},
            {renderRect.x+renderRect.w, renderRect.y+renderRect.h, uMax, 0},
        };

        D3D11_BUFFER_DESC vbDesc = {};
        vbDesc.ByteWidth = sizeof(verts);
        vbDesc.Usage = D3D11_USAGE_IMMUTABLE;
        vbDesc.BindFlags = D3D11_BIND_VERTEX_BUFFER;
        vbDesc.CPUAccessFlags = 0;
        vbDesc.MiscFlags = 0;
        vbDesc.StructureByteStride = sizeof(VERTEX);

        D3D11_SUBRESOURCE_DATA vbData = {};
        vbData.pSysMem = verts;

        HRESULT hr = device->CreateBuffer(&vbDesc, &vbData, g_videoVertexBuffer.ReleaseAndGetAddressOf());
        if (FAILED(hr)) {
            std::cerr << "Lunaris: ID3D11Device::CreateBuffer() failed: " << std::hex << hr << std::dec << std::endl;
            return;
        }
    }

    // Bind video rendering vertex buffer
    UINT stride = sizeof(VERTEX);
    UINT offset = 0;
    ctx->IASetVertexBuffers(0, 1, g_videoVertexBuffer.GetAddressOf(), &stride, &offset);
}

// moonlight-qt d3d11va.cpp:946-1019 (bindColorConversion). Moonlight's
// m_TextureFormat decides the 4:4:4 shader here as well (yuv444 from the
// texture format, see getVideoTextureSRVFormats above).
void bindColorConversion(ID3D11Device *device, ID3D11DeviceContext *ctx, DXGI_FORMAT textureFormat,
                         bool frameChanged, AVFrame *frame) {
    auto framesContext = (AVHWFramesContext *)frame->hw_frames_ctx->data;

    if (textureFormat == DXGI_FORMAT_AYUV) {
        ctx->PSSetShader(g_videoPixelShaders[GENERIC_AYUV].Get(), nullptr, 0);
    }
    else if (textureFormat == DXGI_FORMAT_Y410) {
        ctx->PSSetShader(g_videoPixelShaders[GENERIC_Y410].Get(), nullptr, 0);
    }
    else {
        // We'll need to use the generic 4:2:0 shader for this colorspace and color range combo
        ctx->PSSetShader(g_videoPixelShaders[GENERIC_YUV_420].Get(), nullptr, 0);
    }

    // If nothing has changed since last frame, the constant buffer is reused.
    // (Moonlight leaves it bound on its own context; Qt's context state is not
    // ours across frames, so it is bound again below.)
    if (frameChanged || !g_cscConstantBuffer) {
        D3D11_BUFFER_DESC constDesc = {};
        constDesc.ByteWidth = sizeof(CSC_CONST_BUF);
        constDesc.Usage = D3D11_USAGE_IMMUTABLE;
        constDesc.BindFlags = D3D11_BIND_CONSTANT_BUFFER;
        constDesc.CPUAccessFlags = 0;
        constDesc.MiscFlags = 0;

        CSC_CONST_BUF constBuf = {};
        std::array<float, 9> cscMatrix;
        std::array<float, 3> yuvOffsets;
        getFramePremultipliedCscConstants(frame, cscMatrix, yuvOffsets);

        std::copy(yuvOffsets.cbegin(), yuvOffsets.cend(), constBuf.offsets);

        // We need to adjust our CSC matrix to be column-major and with float3 vectors
        // padded with a float in between each of them to adhere to HLSL requirements.
        for (int i = 0; i < 3; i++) {
            for (int j = 0; j < 3; j++) {
                constBuf.cscMatrix[i * 4 + j] = cscMatrix[j * 3 + i];
            }
        }

        std::array<float, 2> chromaOffset;
        getFrameChromaCositingOffsets(frame, chromaOffset);
        constBuf.chromaOffset[0] = chromaOffset[0] / framesContext->width;
        constBuf.chromaOffset[1] = chromaOffset[1] / framesContext->height;

        // Limit chroma texcoords to avoid sampling from alignment texels
        constBuf.chromaUVMax[0] = frame->width != framesContext->width ?
                                      ((float)(frame->width - 1) / framesContext->width) : 1.0f;
        constBuf.chromaUVMax[1] = frame->height != (int)framesContext->height ?
                                      ((float)(frame->height - 1) / framesContext->height) : 1.0f;

        D3D11_SUBRESOURCE_DATA constData = {};
        constData.pSysMem = &constBuf;

        HRESULT hr = device->CreateBuffer(&constDesc, &constData, g_cscConstantBuffer.ReleaseAndGetAddressOf());
        if (FAILED(hr)) {
            std::cerr << "Lunaris: ID3D11Device::CreateBuffer() failed: " << std::hex << hr << std::dec << std::endl;
            return;
        }
    }
    ctx->PSSetConstantBuffers(0, 1, g_cscConstantBuffer.GetAddressOf());
}

// moonlight-qt d3d11va.cpp:1022-1096 (renderVideo), separate-devices + bind
// branch. Returns false when the frame was not drawn.
bool renderVideo(DecodeSession &session, const FrameResources &res, AVFrame *frame) {
    ID3D11DeviceContext4 *ctx = g_renderContext.Get();
    ID3D11Device *device = g_renderDevice.Get();

    // Insert a fence to force the render context to wait for the decode context to finish writing
    lockContext(nullptr);
    if (SUCCEEDED(session.decodeContext->Signal(session.decodeD2RFence.Get(), session.d2rFenceValue))) {
        ctx->Wait(session.renderD2RFence.Get(), session.d2rFenceValue++);
    }
    unlockContext(nullptr);

    // Our indexing logic depends on a direct mapping into the SRV array
    // based on the texture index provided by FFmpeg.
    UINT srvIndex = (uintptr_t)frame->data[1];
    if (srvIndex >= res.srvs.size()) {
        std::cerr << "Lunaris: unexpected texture index: " << srvIndex << std::endl;
        return false;
    }

    bool frameChanged = hasFrameFormatChanged(frame);

    // Bind our vertex buffer
    bindVideoVertexBuffer(device, ctx, frameChanged, frame);

    // Bind our CSC shader (and constant buffer, if required)
    bindColorConversion(device, ctx, res.textureFormat, frameChanged, frame);

    // Bind SRVs for this frame
    ID3D11ShaderResourceView *frameSrvs[] = { res.srvs[srvIndex][0].Get(), res.srvs[srvIndex][1].Get() };
    ctx->PSSetShaderResources(0, 2, frameSrvs);

    // Draw the video
    ctx->DrawIndexed(6, 0, 0);

    // Unbind SRVs for this frame
    ID3D11ShaderResourceView *nullSrvs[2] = {};
    ctx->PSSetShaderResources(0, 2, nullSrvs);

    // Insert a fence to force the decode context to wait for the render context to finish reading
    //
    // Because the frame is kept referenced until the next frame is rendered, we insert a
    // wait for the previous frame's fence value rather than the current one.
    // This means the fence should generally not cause a pipeline bubble for the decoder
    // unless rendering is taking much longer than expected.
    if (SUCCEEDED(ctx->Signal(session.renderR2DFence.Get(), session.r2dFenceValue))) {
        lockContext(nullptr);
        session.decodeContext->Wait(session.decodeR2DFence.Get(), session.r2dFenceValue - 1);
        unlockContext(nullptr);
        session.r2dFenceValue++;
    }
    return true;
}

const char *fenceTypeName(SupportedFenceType t) {
    return t == SupportedFenceType::Monitored ? "monitored" :
           (t == SupportedFenceType::NonMonitored ? "non-monitored" : "unsupported");
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
    releaseRenderResources();
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
        connect(win, &QQuickWindow::sceneGraphInvalidated,
                this, &D3D11VideoItem::releaseRenderResources, Qt::DirectConnection);
    }
}

QSGNode *D3D11VideoItem::updatePaintNode(QSGNode *oldNode, UpdatePaintNodeData *) {
    delete oldNode;
    return nullptr;
}

void D3D11VideoItem::releaseRenderResources() {
#if defined(Q_OS_WIN)
    std::lock_guard<std::mutex> lock(g_renderMutex);
    releaseAllRenderObjects();
    dropHeldFrames();
#endif
}

void D3D11VideoItem::renderNative() {
#if defined(Q_OS_WIN)
    std::lock_guard<std::mutex> renderLock(g_renderMutex);

    QQuickWindow *window = this->window();
    if (!window) return;

    // Qt's device, captured on every render so the decoder thread can find it.
    captureQtDevice(window);

    if (g_resetRenderState.exchange(false)) {
        resetStreamRenderState();
    }

    if (!g_d3d11StreamActive.load() || d3d11_render_failed()) {
        dropHeldFrames();
        return;
    }
    if (!g_renderContext) return;

    std::shared_ptr<DecodeSession> session;
    std::shared_ptr<FrameResources> res;
    {
        std::lock_guard<std::mutex> lock(g_sessionMutex);
        session = g_session;
        res = g_frameResources;
    }
    if (!session || !res) return;
    if (session.get() != g_lastSession) {
        resetStreamRenderState();
        g_lastSession = session.get();
    }

    if (!g_shadersReady && !setupRenderingResources(g_renderDevice.Get())) return;

    // The newest frame if there is one; Qt redraws the whole window on every
    // render, so the frame on display is drawn again when no new one arrived.
    AVFrame *newFrame = nullptr;
    AVFrame *frame = nullptr;
    {
        std::lock_guard<std::mutex> lock(g_frameMutex);
        newFrame = g_pendingFrame;
        g_pendingFrame = nullptr;
        frame = newFrame ? newFrame : g_displayedFrame;
    }
    if (!frame) return;

    // Views of another frames context (e.g. after a resolution change) do not
    // match this frame.
    if (!frame->hw_frames_ctx || frame->hw_frames_ctx->data != res->framesCtx || frame->width <= 0 || frame->height <= 0) {
        if (newFrame) av_frame_free(&newFrame);
        return;
    }

    ID3D11DeviceContext4 *ctx = g_renderContext.Get();

    // Render with beginExternalCommands → D3D11 → endExternalCommands.
    window->beginExternalCommands();

    // Save D3D11 state (Qt's render target; the viewport is not Moonlight's
    // but Qt only sets it when its own passes start).
    ID3D11RenderTargetView *oldRtv = nullptr;
    ID3D11DepthStencilView *oldDsv = nullptr;
    ctx->OMGetRenderTargets(1, &oldRtv, &oldDsv);
    UINT oldViewportCount = 1;
    D3D11_VIEWPORT oldViewport = {};
    ctx->RSGetViewports(&oldViewportCount, &oldViewport);

    // Viewport: the item's rectangle in device pixels.
    QRectF rect = boundingRect();
    qreal dpr = window->effectiveDevicePixelRatio();
    QPointF topLeft = mapToScene(QPointF(0, 0));
    D3D11_VIEWPORT vp = { (float)(topLeft.x() * dpr), (float)(topLeft.y() * dpr),
                          (float)(rect.width() * dpr), (float)(rect.height() * dpr),
                          0.0f, 1.0f };
    ctx->RSSetViewports(1, &vp);

    // The pipeline state Moonlight sets once in setupRenderingResources()
    // (d3d11va.cpp:1559, 1566-1603, 1651-1664, 1699-1707) is set per draw,
    // because Qt's passes change it.
    ctx->IASetPrimitiveTopology(D3D11_PRIMITIVE_TOPOLOGY_TRIANGLELIST);
    ctx->VSSetShader(g_vertexShader.Get(), nullptr, 0);
    ctx->IASetInputLayout(g_inputLayout.Get());
    ctx->PSSetSamplers(0, 1, g_sampler.GetAddressOf());
    ctx->IASetIndexBuffer(g_indexBuffer.Get(), DXGI_FORMAT_R32_UINT, 0);
    ctx->OMSetBlendState(g_videoBlendState.Get(), nullptr, 0xffffffff);
    ctx->OMSetDepthStencilState(g_depthState.Get(), 0);
    ctx->RSSetState(g_rasterState.Get());

    bool drawn = renderVideo(*session, *res, frame);

    // Restore old state.
    ctx->OMSetRenderTargets(1, &oldRtv, oldDsv);
    if (oldViewportCount > 0) ctx->RSSetViewports(1, &oldViewport);
    if (oldRtv) oldRtv->Release();
    if (oldDsv) oldDsv->Release();

    window->endExternalCommands();

    if (newFrame) {
        // The frame being displayed stays referenced until the next one has
        // been rendered (d3d11va.cpp:1083-1086).
        AVFrame *old = nullptr;
        {
            std::lock_guard<std::mutex> lock(g_frameMutex);
            old = g_displayedFrame;
            g_displayedFrame = newFrame;
        }
        freeFrame(old);
    }

    if (drawn && !g_reportedActive) {
        g_reportedActive = true;
        QMetaObject::invokeMethod(this, "setD3d11Active", Qt::QueuedConnection, Q_ARG(bool, true));
    }
#endif
}

// ── Extern "C" FFI ─────────────────────────────────────────────────────

// Port of the decoder side of D3D11VARenderer::initialize() and
// createDeviceByAdapterIndex() (d3d11va.cpp:251-420, 668-695), on Qt's adapter.
extern "C" uint64_t d3d11_setup_decoder_device() {
#if defined(Q_OS_WIN)
    {
        bool ok = false;
        int v = qEnvironmentVariableIntValue("LUNARIS_D3D11_PRESENT", &ok);
        if (ok && v == 0) {
            markD3d11Failed("disabled by LUNARIS_D3D11_PRESENT=0");
            return 0;
        }
    }

    // Qt's device is captured by the render hook; wait for a render if needed.
    ComPtr<ID3D11Device5> renderDevice;
    {
        std::unique_lock<std::mutex> lk(g_qtDeviceMutex);
        const auto deadline = std::chrono::steady_clock::now() + std::chrono::seconds(1);
        while (!g_renderDevice) {
            requestWindowUpdate();
            if (std::chrono::steady_clock::now() >= deadline) break;
            g_qtDeviceCv.wait_for(lk, std::chrono::milliseconds(100));
        }
        renderDevice = g_renderDevice;
    }
    if (!renderDevice) {
        markD3d11Failed("Qt's Direct3D 11 device is not available (scene graph is not on Direct3D 11, or the window is not rendering)");
        return 0;
    }

    HRESULT hr;
    ComPtr<IDXGIDevice> dxgiDevice;
    ComPtr<IDXGIAdapter> adapter0;
    ComPtr<IDXGIAdapter1> adapter;
    DXGI_ADAPTER_DESC1 adapterDesc;
    LARGE_INTEGER umdVersion;

    if (FAILED(renderDevice.As(&dxgiDevice)) || FAILED(dxgiDevice->GetAdapter(&adapter0)) || FAILED(adapter0.As(&adapter))) {
        markD3d11Failed("cannot get the DXGI adapter of Qt's device");
        return 0;
    }

    hr = adapter->GetDesc1(&adapterDesc);
    if (FAILED(hr)) {
        markD3d11Failed("IDXGIAdapter::GetDesc() failed");
        return 0;
    }

    if (adapterDesc.Flags & DXGI_ADAPTER_FLAG_SOFTWARE) {
        // d3d11va.cpp:277-281: the WARP device is skipped
        markD3d11Failed("Qt renders on a software adapter");
        return 0;
    }

    // Query the GPU driver version (d3d11va.cpp:283-300)
    hr = adapter->CheckInterfaceSupport(__uuidof(IDXGIDevice), &umdVersion);
    if (FAILED(hr)) {
        markD3d11Failed("IDXGIAdapter::CheckInterfaceSupport() failed");
        return 0;
    }

    std::fprintf(stderr, "Lunaris: D3D11 present: GPU %S (%x:%x) (driver: %u.%u.%u.%u)\n",
                 adapterDesc.Description,
                 adapterDesc.VendorId,
                 adapterDesc.DeviceId,
                 HIWORD(umdVersion.HighPart),
                 LOWORD(umdVersion.HighPart),
                 HIWORD(umdVersion.LowPart),
                 LOWORD(umdVersion.LowPart));

    // Check which fence types are supported by this GPU (d3d11va.cpp:355-373)
    SupportedFenceType fenceType = SupportedFenceType::None;
    {
        ComPtr<IDXGIAdapter4> adapter4;
        if (SUCCEEDED(adapter.As(&adapter4))) {
            DXGI_ADAPTER_DESC3 desc3;
            if (SUCCEEDED(adapter4->GetDesc3(&desc3))) {
                if (desc3.Flags & DXGI_ADAPTER_FLAG3_SUPPORT_MONITORED_FENCES) {
                    // Monitored fences must be used when they are supported
                    fenceType = SupportedFenceType::Monitored;
                }
                else if (desc3.Flags & DXGI_ADAPTER_FLAG3_SUPPORT_NON_MONITORED_FENCES) {
                    // Non-monitored fences must only be used when monitored fences are unsupported
                    fenceType = SupportedFenceType::NonMonitored;
                }
            }
        }
    }

    // The gate, d3d11va.cpp:375-419
    bool separateDevices;
    {
        bool ok = false;
        int v = qEnvironmentVariableIntValue("D3D11VA_FORCE_SEPARATE_DEVICES", &ok);
        if (ok) {
            separateDevices = v != 0;
            std::cerr << "Lunaris: Using D3D11VA_FORCE_SEPARATE_DEVICES to override default logic" << std::endl;
        }
        else {
            D3D11_FEATURE_DATA_D3D11_OPTIONS d3d11Options;

            // Check if cross-device sharing works for YUV textures and fences are supported
            hr = renderDevice->CheckFeatureSupport(D3D11_FEATURE_D3D11_OPTIONS, &d3d11Options, sizeof(d3d11Options));
            separateDevices = SUCCEEDED(hr) && d3d11Options.ExtendedResourceSharing && fenceType != SupportedFenceType::None;

            if (separateDevices) {
                // Use minimum precision support to differentiate Vega and later from Polaris and earlier
                D3D11_FEATURE_DATA_SHADER_MIN_PRECISION_SUPPORT minPrecSupport;
                hr = renderDevice->CheckFeatureSupport(D3D11_FEATURE_SHADER_MIN_PRECISION_SUPPORT, &minPrecSupport, sizeof(minPrecSupport));
                if (FAILED(hr)) {
                    minPrecSupport = {};
                }

                // This texture array sharing codepath is quite prone to driver bugs.
                //
                // Broken GPU vendors/cards/drivers include:
                // - Moore Threads (texture is all zero/green)
                // - Qualcomm (decoding is unstable/slow on QC710)
                // - AMD prior to Vega (Polaris cards display corrupt output - see #2003,
                //                      HD 5570 drivers deadlock with shared texture arrays)
                // - Nvidia drivers prior to ~471.11 (Earlier drivers display all zero/green,
                //                                    We approximate by requiring WDDM 3.0+)
                //
                // Due to all these issues, we will only use this path for Intel/AMD and NVIDIA where we know it
                // provides tangible benefits (performance for the former and VRR support for the latter).
                separateDevices = adapterDesc.VendorId == 0x8086 || // Intel
                                  (adapterDesc.VendorId == 0x10DE && HIWORD(umdVersion.HighPart) >= 30) || // NVIDIA WDDM 3.0+ (PCI ID)
                                  adapterDesc.VendorId == 'ADVN' || // NVIDIA (WoA)
                                  (adapterDesc.VendorId == 0x1002 && (minPrecSupport.PixelShaderMinPrecision & D3D11_SHADER_MIN_PRECISION_16_BIT)); // AMD Vega+
            }
        }
    }
    if (!separateDevices) {
        // Moonlight falls back to one shared device with a copy here
        // (d3d11va.cpp:421-424); Lunaris uses the CPU path instead.
        markD3d11Failed("separate decode device not allowed for this GPU/driver (extended resource sharing, fences or vendor rule)");
        return 0;
    }

    std::shared_ptr<DecodeSession> session = setupSharedDevice(adapter.Get(), fenceType, renderDevice.Get());
    if (!session) {
        markD3d11Failed("could not create the decode device and fences");
        return 0;
    }

    // d3d11va.cpp:668-695: the decode device goes to FFmpeg
    AVBufferRef *hwDeviceContext = av_hwdevice_ctx_alloc(AV_HWDEVICE_TYPE_D3D11VA);
    if (!hwDeviceContext) {
        markD3d11Failed("failed to allocate D3D11VA device context");
        return 0;
    }

    AVHWDeviceContext *deviceContext = (AVHWDeviceContext *)hwDeviceContext->data;
    AVD3D11VADeviceContext *d3d11vaDeviceContext = (AVD3D11VADeviceContext *)deviceContext->hwctx;

    // FFmpeg will take ownership of these pointers, so we use CopyTo() to bump the ref count
    session->decodeDevice.CopyTo(&d3d11vaDeviceContext->device);
    session->decodeContext.CopyTo(&d3d11vaDeviceContext->device_context);

    // Set lock functions that we will use to synchronize with FFmpeg's usage of our device context
    d3d11vaDeviceContext->lock = lockContext;
    d3d11vaDeviceContext->unlock = unlockContext;
    d3d11vaDeviceContext->lock_ctx = nullptr;

    int err = av_hwdevice_ctx_init(hwDeviceContext);
    if (err < 0) {
        av_buffer_unref(&hwDeviceContext);
        markD3d11Failed("failed to initialize the D3D11VA device context");
        return 0;
    }

    {
        std::lock_guard<std::mutex> lock(g_sessionMutex);
        g_session = session;
        g_frameResources.reset();
    }
    g_resetRenderState.store(true);

    std::fprintf(stderr, "Lunaris: D3D11 present: decoder texture access: bind (fence: %s)\n", fenceTypeName(fenceType));
    std::fprintf(stderr, "Lunaris: D3D11 present: using separate device for decoding and rendering\n");
    return (uint64_t)(uintptr_t)hwDeviceContext;
#else
    return 0;
#endif
}

// Port of D3D11VARenderer::prepareDecoderContextInGetFormat()
// (d3d11va.cpp:714-759). The device context is the codec context's own
// hw_device_ctx (the one d3d11_setup_decoder_device() returned).
extern "C" bool d3d11_prepare_decoder_context_in_get_format(uint64_t codec_ctx, int pixel_format) {
#if defined(Q_OS_WIN)
    AVCodecContext *context = (AVCodecContext *)(uintptr_t)codec_ctx;

    std::shared_ptr<DecodeSession> session;
    ComPtr<ID3D11Device5> renderDevice;
    {
        std::lock_guard<std::mutex> lock(g_sessionMutex);
        session = g_session;
    }
    {
        std::lock_guard<std::mutex> lock(g_qtDeviceMutex);
        renderDevice = g_renderDevice;
    }
    if (!context || !session || !renderDevice || !context->hw_device_ctx) {
        markD3d11Failed("no decode session in get_format");
        return false;
    }

    // Create a new hardware frames context suitable for decoding our specified format
    av_buffer_unref(&context->hw_frames_ctx);
    int err = avcodec_get_hw_frames_parameters(context, context->hw_device_ctx, (AVPixelFormat)pixel_format, &context->hw_frames_ctx);
    if (err < 0) {
        std::cerr << "Lunaris: failed to get hwframes context parameters: " << err << std::endl;
        markD3d11Failed("avcodec_get_hw_frames_parameters() failed");
        return false;
    }

    auto framesContext = (AVHWFramesContext *)context->hw_frames_ctx->data;
    auto d3d11vaFramesContext = (AVD3D11VAFramesContext *)framesContext->hwctx;

    // We're binding output textures directly, so we need to add the SRV bind flag
    d3d11vaFramesContext->BindFlags |= D3D11_BIND_SHADER_RESOURCE;

    // We're using separate decode and render devices, so we need to create shared textures
    d3d11vaFramesContext->MiscFlags |= D3D11_RESOURCE_MISC_SHARED | D3D11_RESOURCE_MISC_SHARED_NTHANDLE;

    // Mimic the logic in ff_decode_get_hw_frames_ctx() which adds an extra 3 frames
    if (framesContext->initial_pool_size) {
        framesContext->initial_pool_size += 3;
    }

    err = av_hwframe_ctx_init(context->hw_frames_ctx);
    if (err < 0) {
        av_buffer_unref(&context->hw_frames_ctx);
        std::cerr << "Lunaris: failed initialize hwframes context: " << err << std::endl;
        markD3d11Failed("av_hwframe_ctx_init() failed");
        return false;
    }

    if (!setupFrameRenderingResources(renderDevice.Get(), framesContext)) {
        av_buffer_unref(&context->hw_frames_ctx);
        markD3d11Failed("could not open the decoder textures on Qt's device");
        return false;
    }

    return true;
#else
    (void)codec_ctx; (void)pixel_format;
    return false;
#endif
}

extern "C" bool deliver_d3d11_frame(uint64_t frame_ptr) {
#if defined(Q_OS_WIN)
    if (!g_d3d11StreamActive.load() || d3d11_render_failed() || frame_ptr == 0)
        return false;

    AVFrame *replaced = nullptr;
    {
        std::lock_guard<std::mutex> lock(g_frameMutex);
        replaced = g_pendingFrame;
        g_pendingFrame = (AVFrame *)(uintptr_t)frame_ptr;
    }
    // A frame that was never rendered is replaced by the newer one.
    freeFrame(replaced);

    requestWindowUpdate();
    return true;
#else
    (void)frame_ptr;
    return false;
#endif
}

extern "C" bool d3d11_render_failed() {
    return g_d3d11RenderFailed.load(std::memory_order_acquire);
}

extern "C" void set_d3d11_stream_active(bool active) {
    g_d3d11StreamActive.store(active);
    if (active) {
        // A new stream decides again whether the path is usable.
        g_d3d11RenderFailed.store(false, std::memory_order_release);
        return;
    }
#if defined(Q_OS_WIN)
    {
        // The decode device, fences and views are shared with the render
        // thread through shared_ptr copies, so dropping ours is safe; the
        // render thread drops its own per-stream objects at its next render.
        std::lock_guard<std::mutex> lock(g_sessionMutex);
        g_session.reset();
        g_frameResources.reset();
    }
    // The frames are in use by the render thread; it drops them when it sees
    // the stream is inactive (renderNative).
    g_resetRenderState.store(true);
    requestWindowUpdate();
#endif
    if (g_activeItem) {
        QMetaObject::invokeMethod(g_activeItem, "setD3d11Active",
                                  Qt::QueuedConnection, Q_ARG(bool, false));
    }
}
