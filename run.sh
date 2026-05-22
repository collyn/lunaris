#!/bin/bash

# Exit on error
set -e

# Setup cleanup on Ctrl+C (SIGINT) and SIGTERM
cleanup() {
    echo ""
    echo "Stopping Lunaris development servers..."
    # Kill all background jobs started by this shell
    jobs -p | xargs -r kill 2>/dev/null || true
    exit 0
}
trap cleanup SIGINT SIGTERM

echo "=================================================="
echo " Starting Lunaris Development Environment...     "
echo "=================================================="

# 1. Ensure SQLite database file exists
DATABASE_FILE="lunaris.db"
if [ ! -f "$DATABASE_FILE" ]; then
    echo "[DB] Creating empty SQLite database file: $DATABASE_FILE"
    touch "$DATABASE_FILE"
fi

# 2. Check and install frontend dependencies
if [ ! -d "web/node_modules" ]; then
    echo "[Web] frontend dependencies not found. Installing..."
    cd web && npm install && cd ..
else
    echo "[Web] Frontend dependencies are already installed."
fi

# 3. Build & Run the backend server
echo "[Backend] Starting Rust signaling server..."
cargo run --release --manifest-path server/Cargo.toml --bin server &

# 4. Run the frontend in dev mode
echo "[Frontend] Starting Vite development server..."
npm --prefix web run dev &

# Wait for background jobs to finish (which keeps the script running until interrupted)
wait
