# Video pipeline rewrite: plan

Goal: the host captures and encodes the way Sunshine does, the desktop client
decodes and presents the way Moonlight does, with every platform and feature
they cover (Windows, Linux, macOS; H.264, HEVC, AV1; 4:4:4; HDR 10-bit).

## Rules

1. **No invented logic.** Each ported behaviour comes from the reference
   source and says so (file and line) in the code comment or the commit
   message. Where Lunaris has to differ (Qt Quick instead of SDL, WebRTC
   instead of the GameStream protocol, Rust instead of C++), the difference
   and its reason are written down in this file under the phase.
2. **References** (GPL-3.0, same family as this project's licence):
   - Sunshine `f186813` (2026-11-12), checked out at `_ref/Sunshine`
   - moonlight-qt `67dbfdd` (2026-11-11), checked out at `_ref/moonlight-qt`
   - Reading reports with line citations in `_ref/reports/`:
     `sunshine-encoder-core.md`, `sunshine-windows-capture.md`,
     `sunshine-linux-macos.md`, `moonlight-renderers.md`, `lunaris-current.md`.
     A report is a map, not a source: a claim is checked against the
     reference file before it is ported.
3. **One phase = one or more releases**, each tested on a real machine before
   the next starts:
   - Linux: this machine (KDE Wayland; NVIDIA + RX 6600) and the X11 laptop.
   - Windows: 192.168.10.45 (RX 6600 XT, AMF), built there in
     `F:\lunaris-dev`, tested with the loopback script `F:\lunaris-test\run.ps1`
     (host and client on that machine, screenshot and logs back over SSH).
   - macOS: no machine. Code is compiled by a macOS CI job only and is marked
     untested until someone runs it.
4. **Work split:** the lead writes each task's specification from the
   reference, a smaller-model agent implements it, the lead reviews the diff
   against the reference, builds on Linux and Windows, runs the tests, and
   only then commits.

## Where Lunaris is today (from `lunaris-current.md`, to be re-checked per phase)

- Client on Windows: D3D11VA decode, but frames go through the CPU to
  `QVideoSink` (direct present was broken: decoder and Qt used different
  D3D11 devices). No frame pacer; a 120 ms reorder timeout instead of a
  jitter buffer.
- Colour: the Windows encoders convert to BT.709 studio range and write no
  tags; the FFmpeg encoders tag BT.709 full range; one CUDA fallback converts
  to studio range but tags full range. No BT.2020, no PQ, no 10-bit anywhere.
- Encoders: no probing (capabilities come from FFmpeg's encoder list and
  over-claim Main10 and 4:4:4); native NVENC on Windows ignores the bitrate;
  native QSV halves it; H.264 forced to baseline; no reference-frame
  invalidation; every bitrate change forces an IDR; FIR and `RequestVideoIdr`
  are ignored.
- Windows capture: Desktop Duplication without `ACCESS_LOST` handling (mode
  change, UAC, lock screen), no Windows Graphics Capture.
- Linux capture: NvFBC, KMS, PipeWire portal, X11; no wlroots, no KWin
  screencast, no HDR.
- macOS: no capture, cursor or audio source files; the media crate does not
  compile there.

## Phases

### Phase 1 — Windows client: decode and present on the GPU (Moonlight `d3d11va.cpp`)

Port of the "separate devices, bind" path (`d3d11va.cpp:375-434`): the
decoder gets its own D3D11 device on the adapter Qt renders with; its texture
array is created shared (`MiscFlags |= SHARED | SHARED_NTHANDLE`,
`BindFlags |= SHADER_RESOURCE`, `d3d11va.cpp:714-759`), opened on Qt's device
(`1773-1801`), sampled through per-slice SRVs (`1897-1940`), and the two
devices are synchronised with fences (`1024-1034`, `1079-1095`). Shaders and
constants as `d3d11_yuv420_pixel` / `d3d11_ayuv_pixel` / `d3d11_y410_pixel`
with the colour maths of `renderer.h:384-414`.

Differences from Moonlight, and why:
- Qt Quick owns the swap chain and the render device, so the swap-chain code
  (`d3d11va.cpp` §3.2) is not ported; the video is drawn into Qt's render
  pass. HDR output therefore waits for Phase 5.
- Moonlight's "shared device, copy" fallback needs a lock around every use of
  the render context (`765-767`), which Qt does not offer. Where the
  separate-device path is not allowed by Moonlight's vendor gate
  (`381-411`), Lunaris keeps the CPU path that works today.

- Qt redraws the whole window on every render, so the frame on display is
  drawn again when no new frame arrived, and the pipeline state Moonlight
  sets once is set on every draw (Qt's passes change it). Depth is off and
  the viewport is the item's rectangle: Qt's target has a depth buffer and
  QML does the letterboxing.
- Untagged frames are BT.709 limited range (Moonlight's D3D11VA default is
  Rec.601 / full): Lunaris hosts encode BT.709. Phase 2 makes the hosts tag
  every stream, after which the default no longer matters.
- Moonlight returns `AV_PIX_FMT_NONE` from `get_format` when the preparation
  fails and lets its start-up test decode pick another decoder. Lunaris has
  no test decode yet (Phase 8): `get_format` falls back to the first software
  format and the frames go through the CPU path.
- Not ported yet (Phase 8, with decoder selection): `checkDecoderSupport`
  (`d3d11va.cpp:1327-1489`) and the pre-FL11.1 Intel check (`328-332`).

Result (2026-11-13, RX 6600 XT, driver 32.0.21045.16010): H.264 4:2:0 is
decoded and presented on the GPU ("bind", monitored fences), correct picture
over a 45 s loopback run; `LUNARIS_D3D11_PRESENT=0` gives the CPU path.
Not exercised: HEVC and AV1 (the host did not produce them in the loopback
test — HEVC gave no video track on either path, AV1 negotiated H.264; a host
problem for Phase 3), 4:4:4 and 10-bit textures, resolution change during a
stream, repeated stream start/stop.

Done when: the loopback test on the Windows machine shows a correct picture
with `D3D11VA decode + D3D11 present` in the log for H.264 4:2:0, HEVC, AV1
(as far as the RX 6600 XT decodes them), and the CPU fallback still works.

### Phase 2 — One colour model (Sunshine `video_colorspace.cpp`)

- `lunaris-media`: a `colorspace` module that is a port of
  `video_colorspace.cpp` (Kr/Kb constants, limited/full range and 8/10-bit
  formulas, the FFmpeg and NVENC VUI mappings).
- Every encoder writes the tags that match what it converted (H.264/HEVC VUI,
  AV1 colour config); every converter (D3D11 shaders, GL, CUDA, VAAPI VPP)
  takes its matrix from the module.
- Protocol: the client asks for a colourspace and range as Moonlight does
  (`colorspace_from_client_config`); default as Sunshine's.
- Client: the constants of every renderer come from one port of Moonlight's
  `getFramePremultipliedCscConstants` (already unified in 0.3.1, re-based on
  the reference maths including chroma siting).

### Phase 3 — Encoder core (Sunshine `video.cpp`, `nvenc/`)

- Encoder definition tables (FFmpeg names, device types, pixel formats,
  option lists with their config keys) ported from `video.cpp:720-1250`.
- `probe_encoders` at start (`video.cpp:3108-3272`): each encoder is opened
  with the test configuration; H.264, HEVC, AV1, 4:4:4 and HDR support are
  recorded from what opened, and that is what the host advertises.
- Session configuration as `make_avcodec_encode_session`: profiles, infinite
  GOP, no B-frames, slices, VBV = bitrate/framerate with Sunshine's
  exceptions, low-delay flags.
- Native NVENC on Windows as `src/nvenc`: rate control actually applied,
  reference-frame invalidation, HDR/10-bit/4:4:4.
- AMF and QSV: Sunshine drives them through FFmpeg (`h264_amf`, `h264_qsv`
  on D3D11 frames). Lunaris has native implementations; the decision between
  porting Sunshine's FFmpeg option tables onto them and switching to FFmpeg
  is made at the start of the phase from a measurement on the Windows
  machine (latency and frame pacing of both), and recorded here.
- IDR and invalidation flow from the network (PLI, FIR, client message) to
  the encoder as `stream.cpp` does; bitrate changes without an IDR where the
  encoder allows it.

### Phase 4 — Windows capture (Sunshine `display_base.cpp`, `display_vram.cpp`, `display_wgc.cpp`)

- `DuplicateOutput1` with Sunshine's format list; re-initialisation on
  `ACCESS_LOST`, `ACCESS_DENIED`, `WAIT_ABANDONED`, size or format change;
  GPU and thread priorities; the pacing loop on a high-resolution timer.
- Two devices on one adapter with keyed-mutex shared textures; cursor drawn
  on a private copy in two passes (alpha, invert).
- Colour conversion by Sunshine's HLSL shaders instead of the D3D11 video
  processor (needed for exact matrices, 4:4:4 and HDR).
- Windows Graphics Capture as the fallback when Desktop Duplication fails.

### Phase 5 — HDR and 10-bit, end to end

- Capture: HDR detection from the output colour space, FP16 duplication,
  scRGB to PQ shaders (Windows); `HDR_OUTPUT_METADATA` (KMS) and negotiated
  BT.2020 + PQ (PipeWire) on Linux.
- Encode: Main10 / AV1 10-bit, P010 and 10-bit 4:4:4 formats, mastering and
  content-light metadata, as each encoder table says.
- Protocol: HDR capability and request, metadata to the client.
- Client: 10-bit decode formats (P010, Y410) in the D3D11 renderer; HDR
  output through Qt (swap-chain format and colour space) as far as Qt allows,
  following what Moonlight's renderers set; SDR fallback when the display is
  not HDR. The web client depends on the browser and is out of scope beyond
  not breaking.

### Phase 6 — Linux parity (Sunshine `platform/linux`)

- Backend selection order and conditions as `misc.cpp:1424-1503`.
- wlroots screencopy and KWin screencast backends; portal restore token.
- VAAPI: entrypoint preference and rate-control selection; the GL conversion
  pipeline (`graphics.cpp` `sws_t`) with the Phase 2 matrices; 10-bit.
- NVENC on Linux: the GL to CUDA path including 10-bit.
- Vulkan encode: evaluated after the above (it is new in Sunshine).

### Phase 7 — macOS (Sunshine `platform/macos`, Moonlight `vt_*.mm`)

- Host: ScreenCaptureKit capture (AVFoundation before macOS 14), zero-copy
  `CVPixelBuffer` to VideoToolbox, cursor, audio.
- Client: VideoToolbox decode and a Metal renderer.
- Compiled in CI on macOS runners; marked untested until run on a Mac.

### Phase 8 — Client parity (Moonlight `ffmpeg.cpp`, `pacer/`, `session.cpp`)

- Decoder and renderer selection with test frames; codec preference order
  and capability probing; the frame pacer; decode-error and IDR handling.

## Status

| Phase | State |
|---|---|
| Preparation: references, reports, Windows build and loopback test | done 2026-11-13 |
| 1 | H.264 done and tested on Windows; other codecs wait for the host (Phase 3) |
| 2–8 | not started |
