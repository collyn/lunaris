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
- **Smart host agent** — Runs as a daemon or with a Tauri GUI. Configures and manages the streaming backend automatically.
- **Secure by default** — JWT authentication, bcrypt password hashing, TLS/WSS connections.
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
│  (Web wrapper)   │                       │   (Rust + Tauri GUI) │
└──────────────────┘                       └──────────────────────┘
```

| Component | Tech Stack | Role |
|---|---|---|
| **Central Server** | Rust, Axum, SQLite, WebRTC | API server, WebSocket signaling |
| **Web Client** | React, TypeScript, Vite | Dashboard + in-browser stream viewer |
| **Desktop Client** | Rust, Qt6/QML, FFmpeg, cpal | Native client with hardware video decoding and raw input |
| **Tauri Client** | Rust, Tauri 2, WebView | Desktop wrapper for the web client with system-level input capture |
| **Host Agent** | Rust, Tauri 2 (optional GUI) | Runs on the remote host; manages streaming via lunaris-media |
| **lunaris-media** | Rust | Screen capture, GPU encoding, and audio capture library |
| **Common** | Rust | Shared types and protocol definitions |

## Getting Started

### Install the server

The server is a single file: accounts, host directory, signaling and the web
client (embedded). Download it from the
[latest release](https://github.com/collyn/lunaris/releases/latest).

**Linux** (static binary, any distro, x86_64 or ARM64):

```bash
curl -L https://github.com/collyn/lunaris/releases/latest/download/lunaris-server-linux-x86_64.tar.gz | tar xz
sudo ./lunaris-server-linux-x86_64/lunaris-server install        # optional: --port 9000
```

`install` copies the binary to `/usr/local/bin`, creates the `lunaris-server`
systemd service (starts at boot, restarts on failure), keeps data in
`/var/lib/lunaris` and settings in `/etc/lunaris/server.env`, then prints the
web address, the **initial admin password** and the agent token.
`lunaris-server status` / `uninstall [--purge]` manage it afterwards.

**Windows**: unzip `lunaris-server-windows-x86_64.zip` and run
`lunaris-server.exe install` from an Administrator prompt (Windows service,
firewall rule, data in `%ProgramData%\Lunaris`).

**Docker**:

```bash
docker compose up -d          # or: docker build -t lunaris-server . && docker run -d -p 8090:8090 -v lunaris-data:/data lunaris-server
docker logs lunaris-server    # initial admin password and agent token
```

Then open `http://<server>:8090`, sign in as `admin` with the printed password
and change it. Running `lunaris-server` without a command starts it in the
foreground instead.

### Install the app

The **Lunaris app** controls other computers and lets them control this one
(click **Allow remote control**); it is also the agent and the server (see
below). The standalone **agent** is for computers that are only controlled
(servers, headless boxes), and any browser works as a viewer too. From the
[latest release](https://github.com/collyn/lunaris/releases/latest):

| System | App (control and be controlled) | Agent only |
|---|---|---|
| Ubuntu 24.04+, Debian 13+ | `sudo apt install ./lunaris-client_<ver>_amd64.deb` | `sudo apt install ./lunaris-agent_<ver>_amd64.deb` |
| Fedora, Arch and other Linux (glibc 2.35+) | `lunaris-client-desktop-linux.AppImage`, run once with `--setup-host` to be controlled | `lunaris-agent-linux-x86_64.AppImage`, run once with `--setup-host` |
| Windows | `lunaris-client-installer-windows.exe` | `lunaris-agent-installer-windows.exe` |

The distro packages use the system's Qt and PipeWire and carry their own small
static FFmpeg (hardware encoders/decoders, x264, SVT-AV1), so they work the same
on every distro. They also install the uinput udev rule needed for remote
keyboard/mouse. (`.rpm` and Arch packages are prepared in `scripts/packaging/`
and will be published once the PipeWire bindings are updated.)

The app is one program with three modes:

```bash
lunaris-client                    # the app
lunaris-client agent [--cli ...]  # the agent, headless (same options as lunaris-agent)
lunaris-client server [command]   # the server (same commands as lunaris-server)
```

### Connect directly, without a server

On a LAN, a VPN or through a forwarded port, one computer can control another
without a server. On the computer to control, turn on **Allow remote control**
(app, Home) or **Allow direct connections** (agent window): it shows the
computer's addresses (port 8091) and an access password. In the app, type the
address under **Control a remote computer** and enter the password; it then
appears under **Recent** (add it to **Favorites** from its menu). Headless
agents: `lunaris-agent --cli --direct` prints the address and password
(`--direct-port` changes the port). Only the signaling uses this connection
(plain WebSocket, so keep it to trusted networks); video and input stay
end-to-end encrypted WebRTC.

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
| `LUNARIS_HTTPS_PORT` | `8443` | HTTPS port for the web client (`off` disables it). Browsers only allow game controllers, the clipboard API and WebTransport on https pages (or localhost); agents keep using `ws://` on `SERVER_PORT` |
| `LUNARIS_TLS_CERT`, `LUNARIS_TLS_KEY` | *(self-signed)* | PEM certificate and key for HTTPS. Without them a self-signed certificate is made once in the data directory (the browser asks to accept it) |
| `LUNARIS_DATA_DIR` | working directory | Database, agent token and initial admin password (`/var/lib/lunaris` for the service, `/data` in Docker) |
| `LUNARIS_TOKEN` | *(auto-generated)* | Token for agent-to-server authentication |
| `LUNARIS_ADMIN_PASSWORD` | *(random)* | Initial `admin` password — first start only |
| `DATABASE_URL` | `sqlite://<data dir>/lunaris.db` | SQLite database path |
| `RUST_LOG` | `info,server=debug` | Log level configuration |
| `LUNARIS_TURN` | on | Built-in TURN relay (`0` disables it). Lets WebRTC connect through the server when the two sides can't reach each other directly |
| `LUNARIS_TURN_PORT` | `3478` | TURN listening port (UDP) |
| `LUNARIS_TURN_RELAY_PORTS` | `49160-49260` | UDP ports used for relayed streams (open them in the firewall together with the TURN port) |
| `LUNARIS_TURN_PUBLIC_IP` | *(local addresses)* | Address to advertise when the server is behind 1:1 NAT, e.g. a cloud VM |

### Linux Host (X11 and Wayland)

Remote input uses virtual devices on `/dev/uinput` (like Sunshine), which works on X11 and every Wayland compositor. Grant access once:

```bash
sudo scripts/linux-host/setup-host.sh                         # udev rule + load uinput
sudo scripts/linux-host/setup-host.sh --kms target/release/agent  # optional: KMS capture
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
│       ├── lib.rs              # Agent entry (agent::run), server and session logic
│       ├── direct.rs           # Direct connections (no server)
│       ├── bridge.rs           # WebRTC bridge session
│       └── pairing.rs          # Configuration (server, direct password)
│
├── agent-app/              # The standalone agent binary and its Qt window
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
- [x] Host agent with Tauri GUI
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
