# Linux capture service (unattended screen capture on Wayland)

On Wayland the agent normally captures through xdg-desktop-portal and
PipeWire. The portal makes a person at the computer click "Share" once
(a restore token remembers the answer afterwards). That rules out truly
unattended access, so Lunaris ships a small root service that reads the
screen through DRM/KMS and hands it to the agent in the user's session.

The approach (a privileged KMS reader that passes DMA-BUFs to an
unprivileged process) is the same idea as RustDesk's and Sunshine's
`kmsgrab`. The code is our own (lunaris-media `capture/drm_service/`,
written from the kernel UAPI headers).

## Architecture

```
root: lunaris-agent --capture-service             user session: lunaris-agent
(lunaris-capture.service)                         (started at login)
┌───────────────────────────────────┐            ┌──────────────────────────────┐
│ every /dev/dri/card*:             │  frames    │ DrmServiceCapture            │
│  GETRESOURCES/CONNECTOR/CRTC      │ ─────────▶ │  (lunaris-media ScreenCapture)│
│  GETFB2 → PRIME_HANDLE_TO_FD      │  DMA-BUF fd│  → VAAPI on the scan-out GPU │
│  (read-only DMA-BUF)              │  + cursor  │    render node, zero-copy     │
│  cursor plane: CRTC_X/Y + pixels  │ ◀───────── │  → encode, network, input     │
│ auth: SO_PEERCRED, logind, exe    │  Start/Ack │    (uinput, unchanged)        │
└───────────────────────────────────┘            └──────────────────────────────┘
          /run/lunaris/capture.sock (dir root 0755, socket 0666)
```

* The service never encodes, never opens a network socket (systemd also
  gives it `PrivateNetwork=yes`, `RestrictAddressFamilies=AF_UNIX`), never
  writes to the screen and never becomes DRM master. It uses only "get"
  ioctls, `PRIME_HANDLE_TO_FD` and `GEM_CLOSE` on handles it received, and
  `SET_CLIENT_CAP` (universal planes / atomic properties: changes what its
  own file descriptor is shown, nothing else). Connector queries never force
  an output re-probe.
* The agent picks the backend on Wayland in this order (lunaris-media
  `capture::create_screen_capture`): capture service (socket present and
  the service accepts us) → direct KMS if the process itself has
  `CAP_SYS_ADMIN` → portal/PipeWire → X11. `LUNARIS_CAPTURE=service|kms|portal`
  tries one first; `LUNARIS_CAPTURE_SOCKET` changes the socket path. The log
  says which backend was chosen and why.
* The setting "Wayland screen capture" (app: Settings → Security; agent
  window; `wayland_capture` in `agent_config.json`) chooses between `auto`
  (the default, the order above) and `portal`: only portal/PipeWire, then
  X11. In `portal` mode the agent never connects to the capture service or
  uses KMS, even when they are available, so the person at the computer
  approves through the portal and the desktop shows its sharing indicator.
  (The portal's restore token still remembers an earlier approval.) It
  applies when the agent starts (`lunaris-agent set wayland-capture ...`,
  then restart the agent; a change from the server's web page restarts it
  by itself). `LUNARIS_CAPTURE` is a developer override and
  wins over the setting.
* With the service the agent can list and show every monitor (RustDesk
  sessions included), not just the one picked in the portal.
* Frames are sent at the requested rate (KMS has no damage information, and
  a double-buffered compositor can flip back to the same framebuffer id
  between two samples, so "only when the id changed" would miss updates).
* The DMA-BUF is exported read-only. VAAPI imports it on the GPU that scans
  it out (card → render node through sysfs) and colour-converts it there.
  Compressed multi-plane scan-out buffers (AMD DCC, Intel CCS: several
  planes in one buffer object) and 10-bit formats (AR30/XR30/AB30/XB30) are
  passed through as such; this was verified on an RX 6600 with KWin's
  `AB30` + DCC modifier `0x0200000018767b03` (3 planes).
* Monitor position in the desktop is a compositor concept KMS does not know:
  the agent looks the connector name up in `kscreen-doctor -j` (KDE) or
  `xrandr --listmonitors`, else assumes the monitors sit left to right.

## Protocol (version 1)

`AF_UNIX` `SOCK_SEQPACKET`: one message per packet, descriptors attached to
exactly their packet with `SCM_RIGHTS`. Little-endian. Header: `u32 len`
(whole packet, must equal the received size), `u16 type`, `u16 flags` (0).

| type | name | direction | body |
|---|---|---|---|
| 1 | Hello | both | `u32 magic "LNCS"`, `u16 version`, `u16 0` |
| 2 | DisplayList | service | `u32 n ≤ 16`, n × display |
| 3 | Start | client | `u64 display_id`, `u32 max_fps 1..240`, `u32 max_in_flight 1..4` |
| 4 | Frame | service | `u64 seq, display_id, timestamp_us (CLOCK_MONOTONIC)`, `u32 w, h, fourcc`, `u64 modifier, buffer_id (DMA-BUF inode)`, `u32 crop x,y,w,h`, `u32 n ≤ 4`, n × `{u32 fd_index, offset, stride}` + the fds |
| 5 | FrameAck | client | `u64 seq` (returns one credit) |
| 6 | Cursor | service | `u64 display_id`, `u8 visible`, `u8 has_image`, `u16 0`, `i32 x, y, hot_x, hot_y`, `u32 w, h, generation`, RGBA `w*h*4` if has_image (≤ 256×256) |
| 7 | DisplaysChanged | service | as DisplayList |
| 8 | Status | service | `u64 display_id`, `u32` 1 Active, 2 NoFramebuffer (screen off), 3 Gone, 4 Paused (session not active) |
| 9 | Error | service | `u32` 1 Unauthorized, 2 BadVersion, 3 BadMessage, 4 NoSuchDisplay, 5 Busy, 6 Internal; `u16 len ≤ 256` + UTF-8 |
| 10 | Stop | client | – |
| 11 | ListDisplays | client | – |

Display: `u64 id ((card minor << 32) | connector id)`, `u8 len + card name`,
`u32 connector_id, crtc_id`, `u8 len + connector name (≤ 32)`,
`i32 x, y` (0: KMS does not know), `u32 w, h, refresh (mHz), flags`
(1 active, 2 position known).

Flow: client `Hello` → service checks the peer, answers `Hello` +
`DisplayList` (or `Error Unauthorized` and closes) → `Start` → `Frame`s while
the client has credit, `Cursor` when position/visibility/picture change
(the picture only when `generation` changes) → `Stop` or close.
Every packet is bounded (192 KiB); truncated packets, trailing bytes, unknown
types/flags/values, wrong descriptor counts and oversized lists are refused
(the connection is closed after an `Error`).

## Threat model

**What the service can do.** It runs as root with `CAP_SYS_ADMIN` and can
read every screen of every GPU. Anyone who can make it hand out frames sees
the screen, including password prompts and the lock screen. Everything below
is about who gets frames.

**Who may connect.** For each connection (fail closed on any error):

1. `SO_PEERCRED` uid is root, or the uid of the **active graphical session
   on seat0** according to logind (`sd_seat_get_active` through libsystemd
   or libelogind; logind's runtime files as fallback). Greeter, tty, remote
   (ssh) and inactive sessions do not count. Other local users get nothing,
   even while logged in in the background.
2. `/proc/<pid>/exe` is the service's own binary or an allow-listed one,
   compared by device and inode. Allow-listed files and all their parent
   directories must be owned by root and not writable by group/others, so an
   unprivileged user cannot replace them. The package allows
   `/usr/bin/lunaris-agent` (itself: the agent of the user's session, run as
   the systemd user service `lunaris-agent`). The AppImage install allows its AppImage through the
   `APPIMAGE` variable (see limits).
3. No `LD_PRELOAD`/`LD_AUDIT` (nor `LD_LIBRARY_PATH` for normal binaries)
   and no tracer attached (`TracerPid`).
4. The pid really is the connecting process: `SO_PEERPIDFD` liveness check,
   else the process start time is compared around the checks.

While streaming, the uid and tracer checks are repeated every second. When
the session switches (another user, the greeter) the service sends `Paused`
and no frames or cursor until that user's session is active again. A locked
screen is still the same user's active session: frames continue, so a remote
user can unlock it (this is the point of unattended access, and what the
portal path also allows once shared).

Refusals are logged (pid, uid, executable path, reason; rate-limited, no
secrets). The client side checks that the socket is served by root.

**What if the agent binary is replaced.** It cannot be by the user: the
allow-listed binaries live in root-owned, non-writable paths and are compared
by inode. A package upgrade replaces the inode; agents still running the old
(deleted) binary are refused until restarted (they fall back to the portal).

**What the check does not stop.** A process that runs as the active user can
already do almost anything that user can: start the real agent binary with a
changed configuration, inject code through means other than the checked
environment variables, or read the agent's memory where ptrace is allowed
(Yama `ptrace_scope` 1, the Ubuntu default, only allows tracing children).
The executable check is therefore not a boundary against malware running as
the desktop user; that boundary is the uid. This is the same trust level as
an X11 session (where any client of the user can read the screen) and as the
portal once its "remember" token exists. Other users and system services
other than root cannot get frames.

**Descriptor hygiene.** The service exports per frame, sends, and closes its
copy right after the send; GEM handles are closed immediately after export.
Received descriptors are wrapped in owning types before the packet is parsed
(so every error path closes them) and are close-on-exec. Tests count open
descriptors around thousands of frames, malformed packets and reconnects.

**Limits.** At most 4 clients (busy clients get `Error Busy`), 240 fps,
4 frames in flight, bounded packets; a client that stops reading is dropped
after a 2 s send timeout. systemd hardening: only `CAP_SYS_ADMIN` (export),
`CAP_SYS_PTRACE` (read another user's `/proc/<pid>/exe` and `environ`) and
`CAP_DAC_READ_SEARCH` (open that `environ`, mode 0400; read only). No
`CAP_KILL`: the liveness check of a client treats EPERM as running. Also
`NoNewPrivileges`, `DevicePolicy=closed` + `DeviceAllow=char-drm`,
`ProtectSystem=strict`, `ProtectHome=read-only`, `PrivateTmp`,
`@system-service` syscall filter. (`MemoryDenyWriteExecute` is not set
because the agent binary links Qt/FFmpeg; worth trying.)

## Known limitations

* Only the primary plane is captured. Content the compositor puts on
  overlay planes (some video players, fullscreen direct scan-out with
  overlays) is missing; the hardware cursor is reported separately.
* Tearing is possible: the compositor may render into a buffer again while
  the encoder reads it (same as `kmsgrab`).
* KMS cursor planes have no hotspot (except on virtual GPUs that expose
  `HOTSPOT_X/Y`); the position is the image's top-left corner. Compositors
  that use a software cursor have no cursor plane: the cursor is in the
  picture and reported invisible.
* Screen off / DPMS / idle blanking: the CRTC has no framebuffer or is
  inactive, the service reports `NoFramebuffer` and the agent repeats the
  last picture until the screen comes back. Waking the screen is not done
  automatically; a future option is a harmless uinput event (e.g. a
  zero-distance relative move) from the agent when a viewer connects.
* AppImage: its code runs from a FUSE mount the user owns, so there is no
  root-owned file to compare; the AppImage path in `APPIMAGE` must match the
  root-owned allow-listed AppImage. That variable is under the user's
  control: with the AppImage install the check is effectively "the active
  user" (see above). Packages (.deb/.rpm/Arch) get the inode check.
* The monitor layout comes from `kscreen-doctor`/`xrandr`; on GNOME the
  XWayland names may not match KMS connector names (single-monitor laptops
  are unaffected; otherwise monitors are assumed left to right).
* Only the agent of the active desktop user gets frames: the headless system
  service `lunaris-agent-system` (user `lunaris-agent`) never does, and the
  `lunaris-client` stream window shares nothing.
* Multi-GPU: displays on several GPUs are listed; the encoder follows the
  GPU of the display being streamed. Buffers with planes in separate buffer
  objects are not supported (not seen on scan-out buffers).

## Phase 2: login screen (design notes, not built)

Without a user session (greeter on seat0) the service already sees the
greeter's framebuffers; what is missing is a client allowed to receive them
and a place to run the encoder:

* A second mode of the service for "no active user session": accept the
  root-run headless agent (`lunaris-agent --cli` as a system service, or a
  dedicated `--login-screen` helper) when the active session's class is
  `greeter`, and stop serving it as soon as a user session becomes active.
* Input at the greeter needs uinput from that root helper (no user agent).
* The handover greeter → user session must be explicit (Paused/Gone, then
  the user agent connects) so the greeter helper never sees the user's
  desktop.

## Testing

* `cargo test --lib` in lunaris-media: protocol (round trips, every
  truncation, limits, mutation fuzz), transport (fd passing with memfds,
  leaks, truncated control data), authorization (pure decision table,
  `/proc` parsers, logind files, trusted paths), KMS struct sizes and ioctl
  numbers, cursor conversion, card→render node with a fake sysfs, and an
  end-to-end fake producer driving the real `DrmServiceCapture` (credits,
  pause/resume, screen off, hotplug, cursor, busy, reconnect after a service
  restart, descriptor counts).
* `cargo run --example drm_service_probe enumerate` (no root): what the
  service would see.
* `cargo run --example dmabuf_import_check` (no root): allocates a buffer with
  the compositor's scan-out format/modifier on the same GPU, then checks EGL
  readback and the VAAPI zero-copy encode with decoded colours.
* With root: `drm_service_probe serve` + `drm_service_probe client` (lists,
  grabs, saves PNGs, measures fps/latency, negative authorization checks).
