# Lunaris server image: one static-ish binary with the web client embedded.
#
#   docker build -t lunaris-server .
#   docker run -d -p 8090:8090 -v lunaris-data:/data --name lunaris lunaris-server
#   docker logs lunaris        # shows the initial admin password and agent token

# --- Web client ---
FROM node:20-slim AS web
WORKDIR /src/web
COPY web/package*.json ./
RUN npm ci
COPY web/ ./
RUN npm run build

# --- Server (embeds web/dist at compile time) ---
FROM rust:1-bookworm AS build
ARG RUST_TOOLCHAIN=nightly-2026-04-17
RUN rustup toolchain install ${RUST_TOOLCHAIN} --profile minimal && rustup default ${RUST_TOOLCHAIN}
WORKDIR /src
COPY . .
COPY --from=web /src/web/dist web/dist
RUN cargo build --release --locked -p server && cp target/release/server /lunaris-server

# --- Runtime ---
FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/*
COPY --from=build /lunaris-server /usr/local/bin/lunaris-server
COPY LICENSE /usr/share/doc/lunaris/LICENSE

ENV SERVER_PORT=8090 \
    LUNARIS_DATA_DIR=/data \
    RUST_LOG=info
VOLUME /data
EXPOSE 8090 3478/udp 49160-49260/udp
ENTRYPOINT ["/usr/local/bin/lunaris-server"]
CMD ["run"]
