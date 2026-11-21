<p align="center">
  <img src="client-qml/qml/icon.png" alt="Lunaris" width="128" />
</p>

<h1 align="center">Lunaris</h1>

<p align="center">
  <strong>Open-source remote desktop with gaming-grade streaming performance.</strong>
</p>

<p align="center">
  <a href="#features">Features</a> ·
  <a href="#architecture">Architecture</a> ·
  <a href="#getting-started">Getting Started</a> ·
  <a href="#project-structure">Project Structure</a> ·
  <a href="#roadmap">Roadmap</a> ·
  <a href="#acknowledgements">Acknowledgements</a>
</p>

---

Lunaris is a fully open-source remote desktop solution that combines the device management convenience of [RustDesk](https://rustdesk.com/) with gaming-grade, GPU-accelerated streaming — accessible from any web browser or native desktop app.

The entire system is written in **Rust** and **TypeScript/React**, optimized for performance and security.

## Features

- **Remote desktop control** — Stream and control any computer from your browser or desktop app.
- **Gaming-grade performance** — GPU-accelerated encoding via lunaris-media (NVENC, AMF, QuickSync, VAAPI).
- **Multiple clients** — Web client (React), native Qt6/QML desktop client, and Tauri desktop wrapper.
- **Smart host agent** — A command-line daemon (systemd user or system service, Windows service) managed from the web; an optional tray accepts connections. Configures and manages the streaming backend automatically.
- **Secure by default** — JWT authentication, bcrypt password hashing, two-factor sign-in, TLS/WSS connections, SPAKE2 password checks for direct connections, signed updates and installers, a Content-Security-Policy on the web client.
- **NAT traversal** — Automatic hole punching with STUN, fallback via TURN relay.
- **Device management** — Web dashboard to manage multiple remote machines with access controls.
- **Remote app launcher** — Browse and launch applications/games on the host from the dashboard.

## Architecture

```
┌──────────────────┐                      ┌──────────────────────┐                     ┌──────────────┐
│   Web Client     │◄────── WebRTC ──────►│   Central Server     │◄── WebRTC ────────►│ Host Agent   │
│   (React)        │   Video/Audio/Input   │   (Rust / Axum)      │    Signaling        │ + lunaris-   │
│                  │                       │   + API              │                      │   media      │
├──────────────────┤                       └──────────┬───────────┘                      └──────────────┘
│  Desktop Client  │                                  │
│  (Qt6/QML/Rust)  │                                  │ WebSocket
│                  │                                  ▼
├──────────────────┤                       ┌──────────────────────┐
│  Tauri Client    │                       │   Host Agent         │
│  (Web wrapper)   │                       │   (Rust, CLI/daemon) │
└──────────────────┘                       └──────────────────────┘
```

| Component | Tech Stack | Role |
|---|---|---|
| **Central Server** | Rust, Axum, SQLite, WebRTC | API server, WebSocket signaling |
| **Web Client** | React, TypeScript, Vite | Dashboard + in-browser stream viewer |
| **Desktop Client** | Rust, Qt6/QML, FFmpeg, cpal | Native client with hardware video decoding and raw input |
| **Tauri Client** | Rust, Tauri 2, WebView | Desktop wrapper for the web client with system-level input capture |
| **Host Agent** | Rust (command line / service, no GUI) | Runs on the remote host; manages streaming via lunaris-media; managed from the web |
| **Approval helper** | Rust, Qt6/QML (optional tray) | Lets the person at the host accept connections made without the password |
| **lunaris-media** | Rust | Screen capture, GPU encoding, and audio capture library |
| **Common** | Rust | Shared types and protocol definitions |

## Getting Started

### Install the server

The server is a single file: accounts, host directory, signaling and the web
client (embedded). Download it from the
[latest release](https://github.com/collyn/lunaris/releases/latest).

**Debian / Ubuntu** (package, amd64 or arm64):

```bash
sudo apt install ./lunaris-server_<version>_amd64.deb
sudo cat /var/lib/lunaris/setup_code.txt   # one-time code for the setup page
```

The package runs the `lunaris-server` systemd service as its own `lunaris`
user (enabled and started on install, restarted on upgrade), with settings in
`/etc/lunaris/server.env` and data in `/var/lib/lunaris`. `apt remove` keeps
both; `apt purge` deletes them.

**Windows** (installer): run `lunaris-server-installer-windows.exe`. It
registers the `LunarisServer` service and a firewall rule, keeps data in
`%ProgramData%\Lunaris\Server` (administrators only), and its last page opens
the web console and shows the setup code.

**Linux** (static binary, any distro, x86_64 or ARM64):

```bash
curl -L https://github.com/collyn/lunaris/releases/latest/download/lunaris-server-linux-x86_64.tar.gz | tar xz
sudo ./lunaris-server-linux-x86_64/lunaris-server install        # optional: --port 9000
```

`install` copies the binary to `/usr/local/bin`, creates the `lunaris-server`
systemd service (starts at boot, restarts on failure), keeps data in
`/var/lib/lunaris` and settings in `/etc/lunaris/server.env`, then prints the
web address, where the **setup code** is, and the agent token.
`lunaris-server status` / `uninstall [--purge]` manage it afterwards.

**Windows** (zip): unzip `lunaris-server-windows-x86_64.zip` and run
`lunaris-server.exe install` from an Administrator prompt (Windows service,
firewall rule, data in `%ProgramData%\Lunaris\Server`; an older server's
files in `%ProgramData%\Lunaris` are moved there, the agent's stay).

**Docker**:

```bash
docker compose up -d          # or: docker build -t lunaris-server . && docker run -d -p 8090:8090 -p 8443:8443 -v lunaris-data:/data lunaris-server
docker exec lunaris-server cat /data/setup_code.txt     # one-time code for the setup page
docker exec lunaris-server cat /data/server_token.txt   # agent token
```

Publish the HTTPS port (8443) too: agents installed with the web page's
command connect to it (`wss://`, pinned to the server's certificate), also
when the page itself was opened over plain http.

Then open `http://<server>:8090`: a new server shows a **setup page** that
asks for the one-time setup code from `setup_code.txt` in its data folder
(`/var/lib/lunaris` for the Linux package and `install`,
`%ProgramData%\Lunaris\Server` on Windows, `/data` in Docker; readable by
root / Administrators only) and the name and password of the first
administrator. The code works once and setup closes for good as soon as an
account exists; the server does not write the code or the token to its log.
For unattended installs set `LUNARIS_ADMIN_PASSWORD` before the first start
instead: the `admin` account gets that password and there is no setup page.
New account passwords need at least 10 characters (existing ones keep
working). Sign-in, two-factor codes, setup and link tickets are limited per
address and per account (also against many requests at once); turning
two-factor on asks for the password again, and **Account security → Sign out
everywhere** ends every session of the account.

**Moving from the app's built-in server.** The desktop app's "server on this
computer" kept its data in `~/.local/share/lunaris-server` (Linux),
`%APPDATA%\lunaris-server` (Windows) or `~/Library/Application
Support/lunaris-server` (macOS). To take its accounts, computers, groups and
keys over into the installed service:

```bash
sudo systemctl stop lunaris-server
sudo lunaris-server import /home/<you>/.local/share/lunaris-server   # --replace if the service already has accounts
sudo systemctl start lunaris-server
```

(Windows, from an Administrator prompt: `net stop LunarisServer`,
`lunaris-server.exe import "C:\Users\<you>\AppData\Roaming\lunaris-server"`,
`net start LunarisServer`.) It copies a snapshot of the database and the
agent token, sign-in, two-factor and TLS keys, makes them readable by the
service only (the `lunaris` user of the package), and keeps a database the
service already had as `lunaris.db.before-import-<time>`. Agents keep their
token; point them at the service's address if it differs, and turn the app's
server off. Running `lunaris-server` without a command starts it in the
foreground instead.

**Server gateway.** With `lunaris-agent` installed on the server's machine too
(the server package recommends it; on Windows install Lunaris Agent; Docker:
build with `--build-arg AGENT_APPIMAGE_URL=...`, see the Dockerfile), the server
runs it as its own gateway: RustDesk, Sunshine, RDP and VNC computers can then
be added and reached through the server itself, without an agent anywhere else
(a server on a VPS, a box in the LAN). The gateway listens on 127.0.0.1 only and
never shows the server's own screen. Its state and switch are on the web page's
Server tab; `LUNARIS_GATEWAY=off|on|<path of lunaris-agent>` overrides.

**Updates.** The server checks GitHub for a newer release once a day
(`LUNARIS_NO_UPDATE_CHECK` turns it off). Administrators then see a notice on
every page of the web client and a dot on the Server tab, which says how to
update (the server never updates itself); closing the notice hides it until
the next release.

**Several people at once.** Like RustDesk, a computer takes any number of
connections at the same time, each accepted on its own (its `approve_mode`),
each with its own mouse and keyboard: nobody is pushed out when someone else
connects. Viewers of the same screen share one capture and one encoder; the
first one's size, frame rate and codec are the stream's, and its bitrate and
frame rate follow the slowest link. A viewer that cannot decode that codec
gets a stream of its own. The other computers an agent shows (RustDesk,
Sunshine, RDP, VNC) still show one viewer at a time. Both the agent and the
server need 0.7.3 or later: with an older agent the server keeps one session
per computer, as before.

### Install the agent (the computers to control)

The **agent** (`lunaris-agent`) shares a computer with a Lunaris server. It
has no window: it is a command-line program that runs as a service, and
everything else is done on the server's web page (Computers): adding the
computer, its settings (when the computer allows it, see below), its access
password. The easiest way is the one-line installer the web page shows under
**Computers → Add** (it carries a one-time install token); the manual steps
are below. The command fetches the script pinned to the server's certificate
(`curl --pinnedpubkey`, or a certificate check in PowerShell), and the script
installs only release files whose SHA-256 matches the release's signed
`SHA256SUMS` (written into the script by the server, or checked against the
signature with OpenSSL 3).

**Linux desktop** (shares the screen of a desktop user):

```bash
# what the web page's command does (Ubuntu 24.04+, Debian 13+; other distros get the AppImage):
curl -fsSL https://<server>/install/agent.sh | sudo LUNARIS_INSTALL_TOKEN='lit_…' bash -s -- [--name NAME]

# by hand, from the release files:
sudo apt install ./lunaris-agent_<ver>_amd64.deb ./lunaris-approval-helper_<ver>_amd64.deb
LUNARIS_INSTALL_TOKEN='lit_…' lunaris-agent --enroll --server https://<server>   # as the desktop user, not root
```

`--enroll` saves the server and the token in
`~/.config/lunaris-agent/agent_config.json` and enables the systemd **user**
service `lunaris-agent.service` for that user only (it starts with the
graphical session of GNOME, KDE Plasma and other systemd sessions; desktops
without one use the login entry `~/.config/autostart/lunaris-agent.desktop`
it also writes, and computers without systemd only that). The package also
enables `lunaris-capture.service` (root; unattended screen capture on
Wayland without the portal prompt, see `docs/linux-capture-service.md`) and
the uinput udev rule for remote keyboard and mouse. Manage it in the session
with `systemctl --user status|restart|stop lunaris-agent` and
`journalctl --user -u lunaris-agent`.

**Linux headless** (a VPS or a server without a desktop: it reaches computers
of other types — RDP, VNC, Sunshine — added through it, and offers direct
access; it shares no screen):

```bash
curl -fsSL https://<server>/install/agent.sh | sudo LUNARIS_INSTALL_TOKEN='lit_…' bash -s -- --headless
# or by hand:
sudo apt install --no-install-recommends ./lunaris-agent_<ver>_amd64.deb
sudo LUNARIS_INSTALL_TOKEN='lit_…' lunaris-agent --system --enroll --server https://<server>
```

The installer picks this mode by itself when the user has no graphical
session and no login screen starts at boot. `--system` uses the system
service `lunaris-agent-system.service`: user `lunaris-agent`, data in
`/var/lib/lunaris-agent`, hardened like the server's unit; `--enroll` enables
and starts it. The `lunaris-agent` package has no Qt or desktop dependency
(portal, PipeWire and VA-API drivers are only recommended).

**Windows**: run the server's one-line PowerShell installer (Computers → Add)
in an elevated PowerShell, or `lunaris-agent-installer-windows.exe`. It
installs the agent as the `LunarisAgentService` Windows service (as SYSTEM in
the console session, so it can control the logon screen and elevated
windows; starts at boot) and, as an optional component that is selected by
default, the approval helper. Join a server from an elevated prompt:

```powershell
$env:LUNARIS_INSTALL_TOKEN='lit_…'; & "C:\Program Files\Lunaris Agent\lunaris-agent.exe" --system --enroll --server https://<server>
Restart-Service LunarisAgentService
```

Settings and the log are in `%ProgramData%\Lunaris` (administrators only).
An earlier version's agent window settings (`%APPDATA%\lunaris-agent`) are
taken over by the installer when the service has none yet.

**On the command line** (every platform; add `--system` for the system
service's config, as root / administrator):

```bash
lunaris-agent status [--show-password]   # server, running/connected, addresses, settings
lunaris-agent set direct on              # see `lunaris-agent set --help` for the keys
lunaris-agent set server-settings allow  # let the server's administrators change the settings
lunaris-agent set password random        # or `-` to read one from stdin (12 to 128 characters)
lunaris-agent set file-access full       # whole disk for file transfer (only here, never from the server)
lunaris-agent set legacy-direct off      # refuse apps from before TLS (password in a plain ws:// URL)
lunaris-agent --import-config FILE       # merge another agent_config.json
lunaris-agent --probe-encoders           # which hardware encoders work
```

A change made with `set` applies when the agent starts again
(`systemctl --user restart lunaris-agent`, `sudo systemctl restart
lunaris-agent-system`, `Restart-Service LunarisAgentService`). By default the
server may only *read* a computer's settings; `set server-settings allow` (or
`--allow-server-settings`) lets its administrators change them, and see the
access password, from the web page, where they apply at once. Even then the
server can't widen what remote people reach: giving file transfer the whole
disk (or a folder outside the user's home) and switching "ask before each
server session" off are only possible with `lunaris-agent set` on the
computer itself. File transfer reaches the home folder by default, also for
the Windows service (the logged-on user's profile, not the whole disk).

**Approval helper** (`lunaris-approval-helper`, optional, needs Qt): a tray
program for the person at the computer. When someone connects without the
access password (approval modes `click` and `password-click`), it asks
"accept this connection?". Without it only the password lets anyone in. It
talks to the agent over a local channel only (a named pipe of the service
on Windows, which only trusts a helper from the agent's own folder; a socket
in `$XDG_RUNTIME_DIR` of the user's agent on Linux), and starts at every
login (`/etc/xdg/autostart`, the HKLM Run key on Windows). The headless
system service has no one to ask.

### Install the stream window (the computers that control)

Any browser works as a viewer. The **Lunaris client** (`lunaris-client`) is
the low-latency stream window with hardware video decoding, opened from the
server's web page (**Connect**):

| System | Package |
|---|---|
| Ubuntu 24.04+, Debian 13+ | `sudo apt install ./lunaris-client_<ver>_amd64.deb` |
| Fedora, Arch and other Linux (glibc 2.35+) | `lunaris-client-desktop-linux.AppImage` |
| Windows | `lunaris-client-installer-windows.exe` |

The distro packages use the system's Qt and PipeWire and carry their own small
static FFmpeg (hardware encoders/decoders, x264, SVT-AV1), so they work the same
on every distro. (`.rpm` and Arch packages of the agent, the helper and the
client are prepared in `scripts/packaging/` and will be published once the
PipeWire bindings are updated.)

### Connect directly, without a server

On a LAN, a VPN or through a forwarded port, a computer can also be reached
without installing it against a server. On the computer to control, run
`lunaris-agent set direct on` (then restart the agent) or
`lunaris-agent --cli --direct`: it prints the computer's addresses (port
8091, `--direct-port` / `set direct-port` change it) and its access password
(`lunaris-agent status --show-password` shows it later). On the server's web
page, **Computers → Add computer** with that address and password adds it: the
server connects out to it.

The direct listener speaks TLS with a certificate the agent keeps next to its
config (`direct_tls_cert.der`); the password is checked with SPAKE2 (bound to
that certificate), so whoever answers at the address can't test passwords
offline. A server pins the certificate of a computer it added by address the
first time an agent that keeps it answers: if the computer is reinstalled,
**reset key** on its card accepts the new one. Agents from before 0.7.1 still
connect (older password check, certificate not pinned) until they are updated.

**RustDesk clients** (`lunaris-agent set rustdesk on`, port 21128 by default;
RustDesk itself uses 21118): an unmodified RustDesk connects by address with
the access password — type `address:21128`. RustDesk's direct IP access is
**unencrypted** (only the password is protected): keep it to a LAN or a VPN.

### Prerequisites (building from source)

- **Rust** 1.75+ (with `cargo`)
- **Node.js** 20+ and npm
- **FFmpeg** libraries (for the native desktop client)
- **Qt6** (for the QML desktop client)

### Quick Start

```bash
# Clone the repository
git clone https://github.com/collyn/lunaris.git
cd lunaris

# Use the interactive control panel
./run.sh

# Or run development mode directly (server + web dev server)
./run.sh --dev
```

The `run.sh` script provides several options:

| Flag | Description |
|---|---|
| `--dev` | Run backend (debug) and Vite dev server concurrently |
| `--prod` | Build web assets, then run the server in release mode |
| `--build` | Build the entire workspace (Rust release + web) |
| `--build-web` | Build React frontend only |
| `--build-rust` | Build Rust workspace only (release) |

### Manual Setup

```bash
# Terminal 1 — Start the backend server
cargo run --manifest-path server/Cargo.toml --bin server

# Terminal 2 — Start the web frontend dev server
cd web && npm install && npm run dev

# Open http://localhost:5173 in your browser
```

### Environment Variables

| Variable | Default | Description |
|---|---|---|
| `SERVER_PORT` | `8090` | HTTP port (web client, API, WebSocket signaling) |
| `LUNARIS_HTTPS_PORT` | `8443` | HTTPS port for the web client (`off` disables it). Browsers only allow game controllers, the clipboard API and WebTransport on https pages (or localhost). Agents installed with the web page's command connect here (`wss://`, certificate pinned); agents set up with a `ws://` address keep working, with a warning in `lunaris-agent status` |
| `LUNARIS_TLS_CERT`, `LUNARIS_TLS_KEY` | *(self-signed)* | PEM certificate and key for HTTPS. Without them a self-signed certificate is made once in the data directory (the browser asks to accept it) |
| `LUNARIS_DATA_DIR` | working directory | Database, agent token, keys and the first-start `setup_code.txt` (`/var/lib/lunaris` for the Linux service, `%ProgramData%\Lunaris\Server` for the Windows service, `/data` in Docker) |
| `LUNARIS_TOKEN` | *(auto-generated)* | Token for agent-to-server authentication |
| `LUNARIS_ADMIN_PASSWORD` | *(unset: setup page)* | First start only: makes the `admin` account with this password and skips the setup page (headless installs, Docker) |
| `DATABASE_URL` | `sqlite://<data dir>/lunaris.db` | SQLite database path |
| `RUST_LOG` | `info,server=debug` | Log level configuration |
| `LUNARIS_GATEWAY` | *(on when `lunaris-agent` is found)* | The server gateway: `off`, `on`, or the path of the `lunaris-agent` to run. Otherwise switched on the Server tab |
| `LUNARIS_GATEWAY_LOG` | *(agent default)* | Log level of the gateway's agent (its `RUST_LOG`) |
| `LUNARIS_TURN` | on | Built-in TURN relay (`0` disables it). Lets WebRTC connect through the server when the two sides can't reach each other directly. Credentials are made per session; it never relays to the server's own addresses (other than its relay ports) or to loopback, link-local and other special ranges. Its state and the sessions relaying now are on the web page (TURN servers → Built-in → Details) |
| `LUNARIS_TURN_PORT` | `3478` | TURN listening port (UDP) |
| `LUNARIS_TURN_RELAY_PORTS` | `49160-49260` | UDP ports used for relayed streams (open them in the firewall together with the TURN port) |
| `LUNARIS_TURN_PUBLIC_IP` | *(local addresses)* | Address to advertise when the server is behind 1:1 NAT, e.g. a cloud VM |
| `LUNARIS_WEB_DIR` | *(embedded)* | Serve the web client from this folder (with `index.html`) instead of the copy built into the server |
| `LUNARIS_CORS_ORIGINS` | *(this server only)* | Comma list of other origins (or `*`) whose pages may call the API, e.g. a web client served from another host |
| `LUNARIS_TRUSTED_PROXIES` | *(loopback)* | Comma list of IPs/CIDRs of reverse proxies whose `X-Forwarded-For` is believed (sign-in limits count the real client) |
| `LUNARIS_HSTS` | off | `1` sends Strict-Transport-Security on the HTTPS port (only with a real certificate: it removes the browser's "continue anyway" for a self-signed one) |

### Linux Host (X11 and Wayland)

Remote input uses virtual devices on `/dev/uinput` (like Sunshine), which works on X11 and every Wayland compositor. Grant access once:

```bash
sudo scripts/linux-host/setup-host.sh                         # udev rule + load uinput
sudo scripts/linux-host/setup-host.sh --kms target/release/lunaris-agent  # optional: KMS capture
sudo scripts/linux-host/setup-host.sh --uninstall
```

On Wayland the screen is captured through the XDG desktop portal + PipeWire. The first session asks which monitor to share; the choice is remembered (`~/.config/lunaris/screencast-token`). Frames stay on the GPU end to end (DMA-BUF → VAAPI) when encoding with VAAPI on the GPU that drives the displays.

| Variable | Default | Description |
|---|---|---|
| `LUNARIS_RENDER_NODE` | *(auto: display GPU)* | Render node for VAAPI/EGL, e.g. `/dev/dri/renderD129` |
| `LUNARIS_PIPEWIRE_ZERO_COPY` | `1` | `0` reads frames back to the CPU in the capture thread |
| `LUNARIS_CAPTURE` | *(auto)* | `kms` uses KMS capture on Wayland (needs `--kms` setup above) |
| `LUNARIS_EMBED_HOST_CURSOR` | `0` | `1` draws the cursor into the video instead of the client overlay |

## Project Structure

```
lunaris/
├── server/                 # Rust backend — API, signaling
│   └── src/
│       ├── main.rs             # Axum HTTP server and route definitions
│       ├── signaling.rs        # WebSocket signaling for agents and clients
│       ├── auth.rs             # JWT authentication
│       ├── db.rs               # SQLite database layer
│       ├── input.rs            # Input event handling
│       └── video/              # Video processing
│
├── agent/                  # Rust host agent library — capture, encode, input, sessions
│   └── src/
│       ├── main.rs             # The lunaris-agent program (command line, no GUI)
│       ├── lib.rs              # Agent entry (agent::run), server and session logic
│       ├── direct.rs           # Direct connections (no server)
│       ├── bridge.rs           # WebRTC bridge session
│       └── pairing.rs          # Configuration (server, direct password)
│
├── approval-channel/       # Local channel between the agent (daemon) and the approval helper
├── approval-helper/        # lunaris-approval-helper: optional Qt tray that accepts connections
│
├── web/                    # React frontend — dashboard and stream viewer
│   ├── src/
│   │   ├── App.tsx             # Main app (auth, dashboard, host settings)
│   │   └── components/
│   │       └── StreamPlayer.tsx    # WebRTC stream viewer
│   └── public/
│       └── favicon.svg
│
├── client-qml/             # Rust + Qt6/QML native desktop client
│   ├── src/
│   │   ├── lib.rs              # Client core logic
│   │   ├── bridge.rs           # WebRTC protocol bridge
│   │   ├── decoder.rs          # FFmpeg hardware video decoding
│   │   └── audio.rs            # Opus audio decoding via cpal
│   └── qml/
│       ├── main.qml            # Main window
│       ├── Home.qml            # Home: this computer, remote computers
│       ├── AppSettings.qml     # Settings
│       ├── Dashboard.qml       # Server sign-in and computers (Home > Server)
│       └── Settings.qml        # Per-computer stream settings
│
├── lunaris-media/          # Rust streaming media library (capture, encode, audio)
├── client-desktop/         # The Lunaris app binary: app, `agent` and `server` modes
├── client/                 # SDL2-based native client (legacy)
├── common/                 # Shared Rust crate — types and protocol definitions
│
├── Cargo.toml              # Rust workspace configuration
├── run.sh                  # Build and run utility script
└── project_architecture.md # Detailed architecture documentation
```

## Data Flow

### Session Negotiation

```
Web Client                    Central Server                  Host Agent
    │                              │                                │
    ├── Request connection ───────►│                                │
    │                              ├── Wake / prepare ─────────────►│
    │                              │◄── Ready ─────────────────────┤
    │◄── SDP / ICE ───────────────┤                                │
    │                              │                                │
    │◄══════════ WebRTC Handshake (via STUN/TURN if needed) ═══════►│
```

### Streaming

- **Video/Audio:** lunaris-media (H.264/HEVC encode) → WebRTC RTP → Client (GPU decode and render)
- **Input:** Client (mouse/keyboard events) → WebRTC Data Channel → Host Agent → Host OS

## Roadmap

- [x] Core WebRTC streaming and web client MVP
- [x] Central server with Axum, SQLite, and JWT auth
- [x] Host agent as a service, managed from the web
- [x] Qt6/QML desktop client with hardware video decoding
- [x] lunaris-media native capture and encoding backend
- [x] NAT traversal: STUN plus a built-in TURN relay in the server
- [x] Direct connections to an agent without a server
- [ ] File transfer via WebRTC Data Channel
- [ ] Bidirectional clipboard sync
- [ ] Multi-monitor support

## Acknowledgements

Lunaris stands on the shoulders of several excellent open-source projects. Special thanks to:

- **[RustDesk](https://rustdesk.com/)** — Inspiration for the device management model and user experience.

## License

Lunaris is licensed under the **GNU General Public License v3.0 or later** (`GPL-3.0-or-later`) — see [LICENSE](LICENSE).
Releases before this change were published under the MIT licence.

Prebuilt packages bundle third-party components (Qt, FFmpeg, x264, …); see
[`scripts/packaging/THIRD-PARTY-NOTICES.md`](scripts/packaging/THIRD-PARTY-NOTICES.md).
