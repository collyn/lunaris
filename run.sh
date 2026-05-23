#!/bin/bash

# Exit on error
set -e

# Setup cleanup on Ctrl+C (SIGINT) and SIGTERM
cleanup() {
    echo ""
    echo "Stopping Lunaris servers..."
    # Kill all background jobs started by this shell
    jobs -p | xargs -r kill 2>/dev/null || true
    exit 0
}
trap cleanup SIGINT SIGTERM

# Check if --dev flag is passed
DEV_MODE=false
for arg in "$@"; do
    if [ "$arg" == "--dev" ]; then
        DEV_MODE=true
    fi
done

# Check and install frontend dependencies
if [ ! -d "web/node_modules" ]; then
    echo "[Web] Frontend dependencies not found. Installing..."
    cd web && npm install && cd ..
else
    echo "[Web] Frontend dependencies are already installed."
fi

if [ "$DEV_MODE" = true ]; then
    echo "=================================================="
    echo " Starting Lunaris in DEV MODE (Concurrent)...     "
    echo "=================================================="

    # Run the backend server
    echo "[Backend] Starting Rust signaling server..."
    cargo run --release --manifest-path server/Cargo.toml --bin server &

    # Run the frontend in dev mode
    echo "[Frontend] Starting Vite development server..."
    npm --prefix web run dev &

    # Wait for background jobs to finish
    wait
else
    echo "=================================================="
    echo " Starting Lunaris in UNIFIED PRODUCTION MODE...   "
    echo "=================================================="

    # Build the frontend assets
    echo "[Frontend] Building static web assets..."
    npm --prefix web run build

    # Run the backend server in the foreground
    echo "[Backend] Starting Rust signaling server..."
    cargo run --release --manifest-path server/Cargo.toml --bin server
fi
