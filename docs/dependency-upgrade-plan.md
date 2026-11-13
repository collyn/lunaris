# Dependency upgrade: plan

Goal (owner, 2026-11-13): every library on its latest version. Reason: the
stutter Moonlight does not have came from an old `webrtc` without forward
error correction; the fix existed upstream.

Survey (2026-11-13, crates.io): 70 direct dependencies across `lunaris` and
`lunaris-media`, 32 of them behind by a major version (0.x minor counts as
major). List at the end.

## Rules

1. **One batch at a time per repository**, each in its own commit, so a
   regression points at one batch. `lunaris` and `lunaris-media` have separate
   lock files and can move in parallel, except for the libraries they share at
   link level (`ffmpeg-next`: one batch for both).
2. **A batch is done when** it builds on Linux and on the Windows test
   machine, the unit tests pass, and its own gate (below) passes on real
   machines. A batch that fails its gate is fixed or reverted before the next
   one starts.
3. **An upgrade changes versions, not behaviour.** Where a new API forces a
   behavioural choice, it is written into the batch's commit message.
4. **Work split:** the lead writes each batch's task from the changelogs, an
   agent does the port, the lead reviews the diff and runs the gate.
5. `Cargo.lock` of `lunaris` keeps pinning `lunaris-media` by commit; that pin
   moves only when a `lunaris-media` batch is finished.

## Gates

- **G-build:** `cargo build --release --workspace` and the unit tests on
  Linux; `client-desktop` on Windows (`F:\lunaris-dev`).
- **G-stream-win:** Windows host → Windows client loopback, H.264 and HEVC:
  picture correct, 60 fps, D3D11 present.
- **G-stream-linux:** Linux host (VAAPI, NVENC, libx264) → Windows client.
- **G-web:** throwaway server + Windows host → headless Chrome: login, stream
  starts, 60 fps, input reaches the host (remote window drag test).
- **G-loss:** Chrome over the WireGuard path, 5 minutes at 15 Mbit/s: hitches
  above 100 ms (baseline before FEC: 25).
- **G-capture:** `capture_encode` on this KDE Wayland machine (PipeWire
  portal) for VAAPI and NVENC.
- **G-audio:** sound audible end to end (needs the owner's ears; the batch is
  marked "awaiting owner" until confirmed).

## Batches

| # | Repository | Libraries | Gate | State |
|---|---|---|---|---|
| W0 | lunaris | `webrtc` 0.17 → 0.21.1, FlexFEC-03 20% | G-build, G-stream-win, G-stream-linux, G-web, G-loss | done (`d5ad8c2`); G-loss: 25 → 8, then 1 after W1 |
| W1 | both | semver-compatible `cargo update` | G-build, G-stream-win, G-web | done (media `edfb786`); G-loss 1 hitch, no freezes |
| M1 | lunaris-media | `nix` 0.31, `libloading` 0.9, `image` 0.25, `evdev` 0.13, `windows` 0.62 | G-build, G-stream-win (input, capture), G-capture | done (media `29bbeaf`); open: uinput, cursor shapes, SendInput by hand |
| M2 | lunaris-media | `ashpd` 0.14, `pipewire` 0.10 | G-build, G-capture, G-stream-linux | done (media `eed0974`) |
| M3 | both | `opus` 0.4, `cpal` 0.18 | G-build, G-audio | done (media `359a93c`); G-audio measured Windows host → Linux client (a sound played on the host is recorded from the client's output); Linux host capture not re-checked |
| L1 | lunaris (server) | `axum` 0.8, `tower-http` 0.7, `sqlx` 0.9, `bcrypt` 0.19, `jsonwebtoken` 11, `rcgen` 0.14, `thiserror` 2, `num-derive` 0.5 | G-build, G-web (login, device list, TLS) | done; `jsonwebtoken` uses the `rust_crypto` backend |
| L2 | lunaris (agent, client) | `tokio-tungstenite` 0.30, `reqwest` 0.13, `base64` 0.23, `dirs` 7, `zip` 9, `png` 0.18, `windows` 0.62, `windows-service` 0.8, `evdev` 0.13 | G-build, G-stream-win, G-web, file transfer and update check by hand | done except by hand: file transfer, update check, gamepad (uinput), Windows service; `zip` removed (unused) |
| F1 | both + CI | `ffmpeg-next` 9, vendored `ffmpeg-sys-next` 9, FFmpeg version of the Windows CI build and of the Linux packages | all stream gates on both hosts, G-capture | after L2 |
| Q1 | lunaris (client-qml, agent-app) | `cxx-qt`, `cxx-qt-lib`, `cxx-qt-build` 0.6 → 0.10 | G-build, the client UI by hand on both platforms, all stream gates | last |
| — | lunaris-media | `screencapturekit` 0.3 → 11 | none possible (no Mac) | with video-rewrite Phase 7 |

Notes on order:
- F1 and Q1 are last because they are the two that can break the build on a
  platform (FFmpeg headers and libraries per platform; the Rust ↔ Qt/QML
  bridge). Everything before them is plain Rust.
- `windows` appears in both repositories; the two copies are independent
  (different versions can coexist), so M1 and L2 need not be simultaneous.
- A release is cut after W0+W1 (the FEC is the user-visible part) and again
  after each of L2, F1 and Q1.

## Survey (behind by a major version)

```
ashpd 0.9 → 0.14            lunaris-media
axum 0.7 → 0.8              server
base64 0.22 → 0.23          agent, common
bcrypt 0.15 → 0.19          server
cpal 0.15 → 0.18            lunaris-media, client-qml
cxx-qt* 0.6 → 0.10          agent-app, client-qml
dirs 5 → 7                  agent, client-qml
evdev 0.11 → 0.13           lunaris-media, agent
ffmpeg-next 8 → 9.1         lunaris-media, client-qml (ffmpeg-sys-next 8.1 → 9.1: agent, vendored)
image 0.24 → 0.25           lunaris-media
jsonwebtoken 9 → 11         server
libloading 0.8 → 0.9        lunaris-media
nix 0.29 → 0.31             lunaris-media
num-derive 0.4 → 0.5        agent, server
opus 0.2 / 0.3 → 0.4        client-qml / lunaris-media
pipewire 0.8 → 0.10         lunaris-media
png 0.17 → 0.18             client-qml
rcgen 0.13 → 0.14           server
reqwest 0.12 → 0.13         agent, agent-app, client-qml
screencapturekit 0.3 → 11   lunaris-media (macOS)
sqlx 0.7 → 0.9              server
thiserror 1 → 2             agent, server
tokio-tungstenite 0.21 → 0.30   agent, client-qml, server
tower-http 0.5 → 0.7        server
webrtc 0.17 → 0.21          agent, client-qml, server
windows 0.58 → 0.62         lunaris-media, agent
windows-service 0.7 → 0.8   agent, server
zip 0.6 → 9                 agent
```
