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

Status (2026-11-13):
- Host (`lunaris-media` `3fd0e56`): `StreamConfig`/`EncoderConfig` carry a
  `StreamColorspace`; the FFmpeg encoders tag from it (`video.cpp:2115-2120`,
  `2311-2314`), swscale converts from it (`video.cpp:318-321`), and the D3D11
  video processor and the VAAPI VPP take matrix and range from it. Measured
  with `examples/color_roundtrip.rs` (colour bars → encode → ffmpeg decode):
  libx264 and VAAPI are within 2/255 for Rec. 601/709 × limited/full.
- Protocol: `encoder_csc_mode` (GameStream's `encoderCscMode`) in the session
  request. A client that sends none gets Rec. 709 limited — a Lunaris default
  (Sunshine's RTSP default is Rec. 601): such clients are browsers and
  Lunaris ≤ 0.3.2. This changes the FFmpeg hosts from full to limited range.
- Client: only Moonlight's `COLOR_SPACE_OVERRIDE` / `COLOR_RANGE_OVERRIDE`
  (`session.cpp:540-556`) are ported; the per-renderer request needs decoder
  selection before the stream starts (Phase 8). Untagged frames are taken as
  what was requested (`renderer.h:245-275`).

Still open in this phase:
- The native Windows encoders ignore the request and stay at Rec. 709 limited
  (`lunaris-media` `a6e72a6`). Measured on AMF with a Rec. 601 full-range
  request: the client decodes its H.264 stream as BT.709 limited whatever the
  host converted to, so a different conversion showed wrong colours. With the
  fixed conversion the desktop and its streamed copy match within 1/255 in
  the loopback screenshot. Phase 3 makes the Windows host honour the request.
- NVENC fed RGB on Linux converts to BT.601 limited whatever is requested;
  FFmpeg tags it so, the picture is right, the request is not honoured.
  Sunshine converts to NV12 itself before NVENC (Phase 6).
- NvFBC's own NV12 conversion is untouched (Phase 6).
- The conversion vectors (`color_vectors`) have no user until the shader
  converters of Phases 4 and 6.
- The VAAPI VPP (DMA-BUF) path is parametrised but was not measured.


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

Findings before the start (2026-11-13, RX 6600 XT):
- HEVC and AV1 never worked through the native AMF encoder: its component
  ids are wrong (`AMFVideoEncoder_HEVC` / `_AV1`; the SDK headers say
  `AMFVideoEncoderHW_HEVC` / `AMFVideoEncoderHW_AV1`), `CreateComponent`
  returns `AMF_NOT_SUPPORTED`, and about twenty HEVC/AV1 property names in
  `windows_amf.rs` do not exist in `VideoEncoderHEVC.h` / `VideoEncoderAV1.h`
  (`AV1Usage` for `Av1Usage`, `HevcIDRPeriod`, `HevcInsertVPS`, …). The agent
  still offers H.265 and the session gets no video.
- The FFmpeg backend is not compiled into the Windows host at all (no
  software encoder either), although the app ships FFmpeg for decoding and
  the CI build of FFmpeg already contains `h264_amf`, `hevc_amf`, `av1_amf`,
  the NVENC encoders, libx264 and SVT-AV1.

Done so far (`lunaris-media` `0514233`, then the fix after it): the FFmpeg
backend is compiled into the Windows host again and used when an FFmpeg
encoder is asked for by name (`--encoder ffmpeg_amf`); the default selection
is unchanged. Its D3D11 frames context was declared with fields FFmpeg does
not have, so the pool textures had no bind flags; it now follows
`hwcontext_d3d11va.h` and uses `D3D11_BIND_RENDER_TARGET` as Sunshine's
`init_hwframes` (`display_vram.cpp:1157-1159`). On the RX 6600 XT `h264_amf`
and `hevc_amf` then stream correctly (desktop and streamed copy within
1/255), and the client presents HEVC through the Phase 1 D3D11 path.

Then (`lunaris-media` `610d8e6`, `8a7168e`, and the default switch after
them):
- `sunshine_tables.rs`: transcription of Sunshine's avcodec encoder tables
  (`video.cpp:761-1473`), the config defaults they point at, and the session
  helpers (profile, GOP, slices, bitrate/VBV, flags). Reviewed against the
  source so far: `amdvce` and the AMF / NVENC constants (against the SDK
  headers). The other tables are reviewed when their encoder is wired.
- The FFmpeg AMF encoders are configured from that table and the session
  setup of `make_avcodec_encode_session` (`video.cpp:2046-2305`).
  Deviations: H.264 4:2:0 uses CAVLC / Constrained Baseline (the agent's SDP
  offers `42002a`; High needs an SDP change — still to do), one slice per
  frame (Lunaris clients send no slice count).
- AMD hosts use FFmpeg AMF by default; the hand-written AMF encoder stays one
  release as the H.264 fallback. Measured on the RX 6600 XT, 1280x720@60,
  8 Mbps target: native 3.1-3.4 ms per frame and 8.3-10.9 Mbps; FFmpeg
  2.4-2.7 ms and at or under 8 Mbps; both hold 60 fps. HEVC works by default.
  A Rec. 601 full-range request is honoured and tagged (Phase 2 is thereby
  complete on AMD; native NVENC and QSV still ignore the request).

- Real streams from a Linux host (this machine, KDE Wayland) to the Windows
  client, 1080p60, 10 Mbps, 14 s each: h264_vaapi (default), hevc_vaapi,
  h264_nvenc, hevc_nvenc, libx264 — all 60 fps sent, no drops, 3.5-4.9 ms per
  frame in hardware and 9.7 ms in libx264; picture correct on the client.
- NVENC through FFmpeg (Linux) and libx264 / libx265 are on the tables too
  (`lunaris-media` `defb1ee`); all seven remaining tables were checked
  against `video.cpp` by a second, independent pass (132 options, no
  difference). Colour bars round trip on the RTX 2060 and in software:
  within 2/255, expected profiles, no B-frames. NVENC 4:4:4 keeps `rgb_mode=yuv444` (BGRA input path).

- The host offers H.265 and AV1 only when a hardware encoder for them really
  opens, on every platform (`probe_encoder`, now also on Windows; the guess
  from the GPU's name is gone). RX 6600 XT: H.264 and H.265 through AMF, AV1
  none, and an AV1 request falls back to H.264. Of Sunshine's
  `probe_encoders` this is the opening only: the test frame with SPS/VUI
  validation, the HDR and 4:4:4 probes and re-probing are not ported.
- FIR and the client's IDR request message ask for an IDR frame as PLI does
  (`stream.cpp:1221-1225`).

Still to do in this phase: VAAPI (with Sunshine's platform rate-control code,
Phase 6), software and QSV on Windows, `probe_encoders` and what the
host advertises, native NVENC as `src/nvenc` (bitrate, invalidation), H.264
High over WebRTC, the IDR / invalidation flow, bitrate change without IDR.

Decision: AMF goes through FFmpeg as in Sunshine (`amdvce`,
`video.cpp:983-1100`, D3D11 frames); the hand-written AMF encoder is removed
once the FFmpeg one is measured to be no worse for H.264 on this machine.
The planned latency comparison stays, but as an acceptance test, not as the
way to choose: one of the two candidates does not work for two codecs.
QSV follows the same route (needs libvpl in the FFmpeg build).

### Phase 4 — Windows capture (Sunshine `display_base.cpp`, `display_vram.cpp`, `display_wgc.cpp`)

- `DuplicateOutput1` with Sunshine's format list; re-initialisation on
  `ACCESS_LOST`, `ACCESS_DENIED`, `WAIT_ABANDONED`, size or format change;
  GPU and thread priorities; the pacing loop on a high-resolution timer.
- Two devices on one adapter with keyed-mutex shared textures; cursor drawn
  on a private copy in two passes (alpha, invert).
- Colour conversion by Sunshine's HLSL shaders instead of the D3D11 video
  processor (needed for exact matrices, 4:4:4 and HDR).
- Windows Graphics Capture as the fallback when Desktop Duplication fails.

Done (2026-11-13, `lunaris-media`): duplication start-up as Sunshine
(`syncThreadDesktop` before each of two `DuplicateOutput` tries, display
power-on when no output is found, display kept awake while capturing) and
recovery: the reinit class of errors (`ACCESS_LOST`, `ACCESS_DENIED`,
`WAIT_ABANDONED`, stale factory, changed size) makes the pipeline stop the
capture and the encoder and build both again on a new device, as
`video.cpp:1752-1810` and `2831-2975` do. Measured on the RX 6600 XT: a mode
change during a stream recovers in about one second (before: the stream
ended). Re-creating only the duplication on the old device was tried first
and does not work.

Frame pacing (2026-11-13, reported by the owner as stutter while dragging a
window): the pipeline's fixed-rate ticker gave capture intervals of 2.4-32 ms
at 60 fps on Windows (8-13 late frames per second). The Desktop Duplication
backend now paces itself with Sunshine's frame pacing groups on a
high-resolution timer (`display_base.cpp:226-345`, `misc.cpp:1824-1870`):
12-21 ms, 0-2 late frames per second. Linux (PipeWire, already
frame-driven) measures 9-24 ms on a 75 Hz desktop streamed at 60 fps — the
rate mismatch itself; streaming at the display's rate is not offered yet.
Not ported: process / GPU priorities (`misc.cpp:1228`, `display_base.cpp:689-718`),
the dedicated capture thread.

Not confirmed: the morning's failure (`DuplicateOutput` access denied after
the machine sat idle, fixed by waking the display) could not be reproduced
later — neither the new nor the old build failed with the monitor timed out
— so the start-up port is untested against it. A monitor switched off by
force gives no frames until an input wakes it (same in Sunshine).

Open: a retry round that falls back to GDI; finding the output again by name
(`refresh_displays`); `SetThreadDesktop` returns `ERROR_BUSY` on threads that
own windows (matters for the secure desktop, with the service work below).
Still to port in this phase: `DuplicateOutput1` and formats, priorities and
the timer pacing, the two-device layout, cursor passes, the HLSL conversion,
WGC.

Requirement from the owner (2026-11-13): on Windows the agent has to run
elevated, otherwise it cannot operate system applications such as Task
Manager (Windows blocks input from a normal process into elevated windows),
and the UAC / lock screen desktop needs more than that. Sunshine runs as a
Windows service that starts the streaming process in the console session
with the SYSTEM token (`tools/sunshinesvc.cpp`), which covers elevated
windows, the secure desktop and start before logon. To port with this phase,
together with the desktop switching of `syncThreadDesktop`.

Written, NOT RUN (2026-11-13): `agent/src/win_service.rs`, a function by
function port of `tools/sunshinesvc.cpp` — `--service-run` (service main,
SYSTEM token retargeted at the console session, `CreateProcessAsUserW` on
`winsta0\default`, job object, 3 s poll, relaunch on console-session change,
stop through a `--terminate` helper then `TerminateProcess` after 20 s),
`--install-service` / `--uninstall-service` (Sunshine's service scripts).
Opt-in: nothing installs it. The host it starts reads
`%ProgramData%\Lunaris\agent_config.json` and logs to
`agent-service.log` there. Injected input follows the input desktop
(Sunshine v0.23.1 `input.cpp:468-505`). It compiles on Windows and the normal
host still streams; installing the service on the test machine was refused
by the session's permission system, so nothing of it has been executed.
Known before testing: the host executables are GUI-subsystem, so the
terminate helper's `AttachConsole` will probably fail and every stop will end
in the 20 s hard kill; the host has no CTRL_C handling; approvals are
"unattended" (password mode only); `SetThreadDesktop` fails on threads that
own windows.

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

## Transport (outside the phases; measured 2026-11-13)

Windows host → Chrome on Linux over a WireGuard tunnel, 1080p60, 15 Mbit/s,
5 minutes, presented-frame gaps above 100 ms:

| Build | Hitches | Notes |
|---|---|---|
| `webrtc` 0.17, NACK only | 25 | 1510 packets asked for again, 325 more than once |
| `webrtc` 0.21.1 + FlexFEC-03 20% (`d5ad8c2`) | 8 | all in the first 2 minutes |
| + AMF bitrate change without a forced key frame (media `601d5bb`) | 1 | at start; 108 NACKs, 1 PLI, no freezes |

- The host paces RTP at 2.5× the bitrate in 1 ms groups; the desktop client
  NACKs every 20 ms and asks for a key frame again while it waits for one.
- Open: the desktop client had one bad run after the FEC change (4.3 s
  freeze, 21 PLIs), not yet repeated with `601d5bb`; key frames are sent as
  one burst; no session longer than 5 minutes has been measured.
- Open (Phase 4): when the desktop changes at about 30 fps the Windows host
  sends about 51 fps with uneven spacing (3–34 ms), because repeated frames
  are added between the new ones.

## Status

| Phase | State |
|---|---|
| Preparation: references, reports, Windows build and loopback test | done 2026-11-13 |
| 1 | H.264 and HEVC tested on Windows (HEVC with `--encoder ffmpeg_amf`); AV1 not decodable on this GPU's host side yet |
| 2 | host and protocol done; see the phase for what is open |
| 3 | AMD on Windows done (FFmpeg AMF with Sunshine's table, default); rest open |
| 4 | start-up and recovery of Desktop Duplication done; rest open |
| 5–8 | not started |
