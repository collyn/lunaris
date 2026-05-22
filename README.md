# 🌙 Lunaris

**Open-source remote desktop solution powered by Sunshine streaming.**

Lunaris combines the convenience of RustDesk's device management with the gaming-grade streaming performance of Moonlight/Sunshine — accessible from any web browser or desktop app.

## ✨ Features

- 🖥️ **Remote Desktop Control** — Stream and control any computer from your browser
- ⚡ **Gaming-Grade Performance** — GPU-accelerated encoding via Sunshine (NVENC, AMF, QuickSync)
- 🌐 **Web & Desktop** — Works in any modern browser, or install the lightweight Tauri desktop client
- 🔒 **Secure** — End-to-end encryption, JWT authentication, and access controls
- 🌍 **NAT Traversal** — Automatic hole punching with STUN/TURN fallback
- 📱 **Device Management** — Dashboard to manage and share multiple remote devices

## 🏗️ Architecture

```
┌──────────────┐     WebRTC      ┌──────────────────┐    Moonlight     ┌──────────┐
│  Web Client  │ ◄──────────────► │  Central Server  │ ◄──────────────► │ Sunshine │
│  (React/     │                  │  (Go/Fiber)      │                  │ (Host)   │
│   Tauri)     │                  │  + Bridge        │                  │          │
└──────────────┘                  └──────────────────┘                  └──────────┘
                                          ▲
                                          │ WebSocket
                                          ▼
                                  ┌──────────────────┐
                                  │   Host Agent     │
                                  │   (Rust)         │
                                  └──────────────────┘
```

## 🚀 Quick Start

### Prerequisites
- Docker & Docker Compose
- Go 1.22+ (for server development)
- Node.js 20+ (for web client development)
- Sunshine installed on the host machine to control

### Development Setup

```bash
# 1. Clone the repository
git clone https://github.com/your-org/lunaris.git
cd lunaris

# 2. Start PostgreSQL database
docker compose up -d postgres

# 3. Start the server
cd server
go run ./cmd/lunaris-server

# 4. Start the web client (in another terminal)
cd web
npm install
npm run dev

# 5. Open http://localhost:5173 in your browser
```

### Environment Variables

| Variable | Default | Description |
|----------|---------|-------------|
| `DATABASE_URL` | `postgres://lunaris:lunaris_dev_password@localhost:5432/lunaris?sslmode=disable` | PostgreSQL connection string |
| `JWT_SECRET` | `lunaris-dev-secret-change-in-production` | JWT signing secret |
| `SERVER_PORT` | `8080` | API server port |
| `TURN_URL` | `turn:localhost:3478` | TURN server URL |
| `TURN_USERNAME` | `lunaris` | TURN credentials |
| `TURN_PASSWORD` | `lunaris_turn_secret` | TURN credentials |

## 📂 Project Structure

```
lunaris/
├── server/          # Go backend (API, signaling, bridge)
├── web/             # React frontend (dashboard, stream viewer)
├── agent/           # Rust host agent (Phase 3)
├── deploy/          # Deployment configs (Docker, nginx, coturn)
└── docs/            # Documentation
```

## 🗺️ Roadmap

- [x] **Phase 1**: Core Bridge + Web Client MVP
- [ ] **Phase 2**: NAT Traversal & Production Server
- [ ] **Phase 3**: Host Agent (auto-install Sunshine)
- [ ] **Phase 4**: Tauri Desktop Client & Advanced Features

## 📄 License

GPLv3 — See [LICENSE](LICENSE) for details.
