#!/bin/bash

# Setup cleanup on Ctrl+C (SIGINT) and SIGTERM
cleanup() {
    echo ""
    echo "Stopping background jobs..."
    # Kill all background jobs started by this shell
    jobs -p | xargs -r kill 2>/dev/null || true
    exit 0
}
trap cleanup SIGINT SIGTERM

# Function to check and install web dependencies
check_web_deps() {
    if [ ! -d "web/node_modules" ]; then
        echo "[Web] Frontend dependencies not found. Installing..."
        cd web && npm install && cd ..
    else
        echo "[Web] Frontend dependencies are already installed."
    fi
}

run_dev() {
    echo "=================================================="
    echo " Starting Lunaris in DEV MODE (Concurrent)...     "
    echo "=================================================="
    check_web_deps
    
    # Run the backend server
    echo "[Backend] Starting Rust signaling server..."
    cargo run --release --manifest-path server/Cargo.toml --bin server &

    # Run the frontend in dev mode
    echo "[Frontend] Starting Vite development server..."
    npm --prefix web run dev &

    # Wait for background jobs to finish
    wait
}

build_web() {
    echo "=================================================="
    echo " Building Web Frontend Static Assets...          "
    echo "=================================================="
    check_web_deps
    npm --prefix web run build
}

build_rust() {
    echo "=================================================="
    echo " Building Rust Backend (Release)...              "
    echo "=================================================="
    cargo build --release --workspace
}

build_all() {
    build_web
    build_rust
    echo "=================================================="
    echo " All Release Builds Completed Successfully!      "
    echo "=================================================="
}

run_prod() {
    build_web
    echo "=================================================="
    echo " Starting Lunaris in PRODUCTION MODE...          "
    echo "=================================================="
    # Run the backend server in the foreground
    echo "[Backend] Starting Rust signaling server..."
    cargo run --release --manifest-path server/Cargo.toml --bin server
}

# Non-interactive CLI flags check
if [ "$#" -gt 0 ]; then
    case "$1" in
        --dev)
            run_dev
            exit 0
            ;;
        --build)
            build_all
            exit 0
            ;;
        --build-web)
            build_web
            exit 0
            ;;
        --build-rust)
            build_rust
            exit 0
            ;;
        --prod)
            run_prod
            exit 0
            ;;
        *)
            echo "Usage: $0 [--dev | --build | --build-web | --build-rust | --prod]"
            exit 1
            ;;
    esac
fi

# Interactive Menu
while true; do
    echo ""
    echo "=================================================="
    echo "           LUNARIS CONTROL PANEL                  "
    echo "=================================================="
    echo " 1) Run Development Mode (Server + Web Dev)"
    echo " 2) Run Production Mode (Build Web + Start Server)"
    echo " 3) Build All Release (Rust Workspace + Web)"
    echo " 4) Build Rust Backend Only (Release)"
    echo " 5) Build Web Frontend Only"
    echo " 6) Exit"
    echo "=================================================="
    read -p "Choose an option [1-6]: " opt
    echo ""

    case $opt in
        1)
            run_dev
            ;;
        2)
            run_prod
            ;;
        3)
            build_all
            ;;
        4)
            build_rust
            ;;
        5)
            build_web
            ;;
        6)
            echo "Goodbye!"
            exit 0
            ;;
        *)
            echo "Invalid option. Please choose between 1 and 6."
            ;;
    esac
done
