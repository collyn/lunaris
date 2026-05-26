# --- Stage 1: Build the Web Frontend ---
FROM node:20-slim AS frontend-builder
WORKDIR /app
COPY web/package*.json ./web/
WORKDIR /app/web
RUN npm ci || npm install
COPY web/ ./
RUN npm run build

# --- Stage 2: Build the Rust Server ---
FROM rust:1.80-bookworm AS rust-builder
RUN apt-get update && apt-get install -y --no-install-recommends \
    cmake \
    clang \
    pkg-config \
    libssl-dev \
    libavahi-compat-libdnssd-dev \
    && rm -rf /var/lib/apt/lists/*

# Install the Rust nightly version matching the project's CI/CD configuration
RUN rustup toolchain install nightly-2026-04-17 && rustup default nightly-2026-04-17

WORKDIR /app
COPY Cargo.toml Cargo.lock ./
COPY common/ ./common/
COPY server/ ./server/

RUN cargo build --release --bin server

# --- Stage 3: Runtime ---
FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y --no-install-recommends \
    libssl3 \
    libavahi-compat-libdnssd1 \
    ca-certificates \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /app

# Copy the server executable from rust-builder
COPY --from=rust-builder /app/target/release/server /app/server

# Copy the static assets of the frontend from frontend-builder
COPY --from=frontend-builder /app/web/dist /app/web/dist

# Set default environment variables
ENV SERVER_PORT=8080
ENV RUST_LOG=info,server=debug

EXPOSE 8080

# Run the server in the /app/data directory so that lunaris.db and server_token.txt 
# are automatically created and stored on the externally mounted volume.
WORKDIR /app/data

CMD ["/app/server"]
