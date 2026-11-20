# Lunaris server image: one static-ish binary with the web client embedded.
#
#   docker build -t lunaris-server .
#   docker run -d -p 8090:8090 -p 8443:8443 -v lunaris-data:/data --name lunaris lunaris-server
#   docker exec lunaris cat /data/setup_code.txt     # one-time code for the setup page (first admin)
#   docker exec lunaris cat /data/server_token.txt   # agent token
# (or set LUNARIS_ADMIN_PASSWORD to make the `admin` account without the setup page)
#
# The container runs as root on purpose: existing /data volumes are owned by root and
# a non-root user could not write to them.
#
# Server gateway (optional, server/src/gateway.rs): with lunaris-agent in the image
# the server runs it as its gateway, so RustDesk, Sunshine, RDP and VNC computers
# can be added and reached through the server itself. Building the agent here would
# need FFmpeg, PipeWire, VA-API... development files, so the image takes the agent's
# released x86_64 AppImage instead (unpacked; no FUSE needed), when asked for:
#   docker build --build-arg AGENT_APPIMAGE_URL=https://github.com/collyn/lunaris/releases/download/v<version>/lunaris-agent-linux-x86_64.AppImage -t lunaris-server .
# (the same version as the server; amd64 images only). Without it the image is as
# before and the Server page says the gateway's agent is not found.

# --- Web client ---
FROM node:20-slim AS web
WORKDIR /src/web
COPY web/package*.json ./
RUN npm ci
COPY web/ ./
# The page shows the workspace's version (vite.config.ts).
COPY Cargo.toml /src/Cargo.toml
RUN npm run build

# --- Server (embeds web/dist at compile time) ---
FROM rust:1-bookworm AS build
ARG RUST_TOOLCHAIN=nightly-2026-04-17
RUN rustup toolchain install ${RUST_TOOLCHAIN} --profile minimal && rustup default ${RUST_TOOLCHAIN}
WORKDIR /src
COPY . .
COPY --from=web /src/web/dist web/dist
RUN cargo build --release --locked -p server && cp target/release/server /lunaris-server

# --- The gateway's agent (only with AGENT_APPIMAGE_URL, see above) ---
FROM debian:bookworm-slim AS agent
ARG AGENT_APPIMAGE_URL=
RUN mkdir -p /opt/lunaris-agent \
    && if [ -n "$AGENT_APPIMAGE_URL" ]; then \
        apt-get update && apt-get install -y --no-install-recommends curl ca-certificates \
        && curl -fsSL -o /tmp/agent.AppImage "$AGENT_APPIMAGE_URL" \
        && chmod +x /tmp/agent.AppImage && cd /tmp && ./agent.AppImage --appimage-extract >/dev/null \
        && cp -a /tmp/squashfs-root/. /opt/lunaris-agent/ && rm -rf /tmp/agent.AppImage /tmp/squashfs-root; \
    fi

# --- Runtime ---
FROM debian:bookworm-slim
COPY --from=agent /opt/lunaris-agent /opt/lunaris-agent
# With the agent: the system libraries an AppImage leaves out (glibc's, X11,
# EGL/GL, DRM, PipeWire), then a check that it starts and knows --bridge-only.
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates \
    && if [ -x /opt/lunaris-agent/AppRun ]; then \
        apt-get install -y --no-install-recommends libx11-6 libxcb1 libxext6 libxfixes3 libxrandr2 \
            libxdamage1 libxtst6 libegl1 libgl1 libgbm1 libdrm2 libwayland-client0 libpipewire-0.3-0 \
            libasound2 libpulse0 libdbus-1-3 \
        && ln -s /opt/lunaris-agent/AppRun /usr/local/bin/lunaris-agent \
        && HOME=/tmp lunaris-agent --bridge-only --help >/dev/null; \
    else rmdir /opt/lunaris-agent; fi \
    && rm -rf /var/lib/apt/lists/*
COPY --from=build /lunaris-server /usr/local/bin/lunaris-server
COPY LICENSE /usr/share/doc/lunaris/LICENSE

ENV SERVER_PORT=8090 \
    LUNARIS_DATA_DIR=/data \
    RUST_LOG=info
VOLUME /data
EXPOSE 8090 8443 3478/udp 49160-49260/udp
ENTRYPOINT ["/usr/local/bin/lunaris-server"]
CMD ["run"]
