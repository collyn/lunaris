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

Lunaris is a fully open-source remote desktop solution that combines the device management convenience of [RustDesk](https://rustdesk.com/) with the low-latency, GPU-accelerated streaming of [Moonlight](https://moonlight-stream.org/) / [Sunshine](https://github.com/LizardByte/Sunshine) — accessible from any web browser or native desktop app.

The entire system is written in **Rust** and **TypeScript/React**, optimized for performance and security.

## Features

- **Remote desktop control** — Stream and control any computer from your browser or desktop app.
- **Gaming-grade performance** — GPU-accelerated encoding via Sunshine (NVENC, AMF, QuickSync, VAAPI).
- **Multiple clients** — Web client (React), native Qt6/QML desktop client, and Tauri desktop wrapper.
- **Smart host agent** — Automatically downloads, installs, and configures Sunshine. Runs as a daemon or with a Tauri GUI.
- **Secure by default** — JWT authentication, bcrypt password hashing, TLS/WSS connections.
- **NAT traversal** — Automatic hole punching with STUN, fallback via TURN relay.
- **Device management** — Web dashboard to manage multiple remote machines with access controls.
- **Remote app launcher** — Browse and launch applications/games on the Sunshine host from the dashboard.
- **Remote Sunshine configuration** — Change encoder, preset, port, and other Sunshine settings directly from the web UI.

## Architecture

```
┌──────────────────┐                      ┌──────────────────────┐                     ┌──────────────┐
│   Web Client     │◄────── WebRTC ──────►│   Central Server     │◄── Moonlight ──────►│   Sunshine   │
│   (React)        │   Video/Audio/Input   │   (Rust / Axum)      │    Protocol          │   (Host)     │
│                  │                       │   + Bridge + API     │                      │              │
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
| **Central Server** | Rust, Axum, SQLite, WebRTC | API server, WebSocket signaling, Moonlight-to-WebRTC bridge |
| **Web Client** | React, TypeScript, Vite | Dashboard + in-browser stream viewer |
| **Desktop Client** | Rust, Qt6/QML, FFmpeg, cpal | Native client with hardware video decoding and raw input |
| **Tauri Client** | Rust, Tauri 2, WebView | Desktop wrapper for the web client with system-level input capture |
| **Host Agent** | Rust, Tauri 2 (optional GUI) | Runs on the remote host; manages Sunshine, handles auto-pairing |
| **Common** | Rust | Shared types and protocol definitions |

## Getting Started

### Prerequisites

- **Rust** 1.75+ (with `cargo`)
- **Node.js** 20+ and npm
- **Sunshine** installed on the machine you want to control
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
| `DATABASE_URL` | `sqlite://lunaris.db` | SQLite database path |
| `SERVER_PORT` | `8080` | API server port |
| `RUST_LOG` | `info,server=debug` | Log level configuration |
| `LUNARIS_TOKEN` | *(auto-generated)* | Token for agent-to-server authentication |

## Project Structure

```
lunaris/
├── server/                 # Rust backend — API, signaling, Moonlight bridge
│   └── src/
│       ├── main.rs             # Axum HTTP server and route definitions
│       ├── signaling.rs        # WebSocket signaling for agents and clients
│       ├── bridge.rs           # Moonlight → WebRTC protocol bridge
│       ├── pairing.rs          # Sunshine auto-pairing handshake
│       ├── auth.rs             # JWT authentication
│       ├── db.rs               # SQLite database layer
│       ├── input.rs            # Input event handling
│       └── video/              # Video processing
│
├── agent/                  # Rust host agent — manages Sunshine on the remote machine
│   ├── src/
│   │   ├── main.rs             # Agent daemon logic, auto-install Sunshine
│   │   ├── bridge.rs           # Agent-side Moonlight bridge
│   │   ├── gui.rs              # Tauri GUI (optional, behind feature flag)
│   │   └── pairing.rs          # Pairing with the central server
│   └── tauri.conf.json
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
│   │   ├── bridge.rs           # Moonlight protocol bridge
│   │   ├── decoder.rs          # FFmpeg hardware video decoding
│   │   └── audio.rs            # Opus audio decoding via cpal
│   └── qml/
│       ├── main.qml            # Main window
│       ├── Dashboard.qml       # Device dashboard
│       └── Settings.qml        # Stream settings
│
├── client-desktop/         # Tauri wrapper for client-qml
├── client/                 # SDL2-based native client (legacy)
├── common/                 # Shared Rust crate — types and protocol definitions
├── moonlight-common-rust/  # Moonlight protocol library (git dependency)
│
├── Cargo.toml              # Rust workspace configuration
├── run.sh                  # Build and run utility script
└── project_architecture.md # Detailed architecture documentation
```

## Data Flow

### Session Negotiation

```
Web Client                    Central Server                  Host (Agent / Sunshine)
    │                              │                                │
    ├── Request connection ───────►│                                │
    │                              ├── Wake / prepare ─────────────►│
    │                              │◄── Ready ─────────────────────┤
    │◄── SDP / ICE ───────────────┤                                │
    │                              │                                │
    │◄══════════ WebRTC Handshake (via STUN/TURN if needed) ═══════►│
```

### Streaming

- **Video/Audio:** Sunshine (H.264/HEVC encode) → Bridge (repackage as WebRTC RTP) → Client (GPU decode and render)
- **Input:** Client (mouse/keyboard events) → WebRTC Data Channel → Bridge → Sunshine input driver → Host OS

## Roadmap

- [x] Core Moonlight bridge and web client MVP
- [x] Central server with Axum, SQLite, and JWT auth
- [x] Host agent with auto-install Sunshine and Tauri GUI
- [x] Qt6/QML desktop client with hardware video decoding
- [ ] Production-ready NAT traversal (STUN/TURN)
- [ ] File transfer via WebRTC Data Channel
- [ ] Bidirectional clipboard sync
- [ ] Multi-monitor support

## Acknowledgements

Lunaris stands on the shoulders of several excellent open-source projects. Special thanks to:

- **[moonlight-common-rust](https://github.com/MrCreativ3001/moonlight-common-rust)** by [@MrCreativ3001](https://github.com/MrCreativ3001) — A Rust implementation of the Moonlight protocol. This library is the foundation for all Sunshine communication in Lunaris. Without it, this project would not exist.

- **[moonlight-web-stream](https://github.com/MrCreativ3001/moonlight-web-stream)** by [@MrCreativ3001](https://github.com/MrCreativ3001) — A proof-of-concept for streaming Moonlight via a web browser. This project directly inspired the WebRTC bridge architecture in Lunaris.

- **[Sunshine](https://github.com/LizardByte/Sunshine)** by [LizardByte](https://github.com/LizardByte) — The self-hosted game streaming server that powers the screen capture and GPU encoding on the host side.

- **[Moonlight](https://moonlight-stream.org/)** — The open-source GameStream client whose protocol Lunaris builds upon.

- **[RustDesk](https://rustdesk.com/)** — Inspiration for the device management model and user experience.

## License

GPLv3 — See [LICENSE](LICENSE) for details.
