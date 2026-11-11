#!/usr/bin/env bash
# Build the portable Lunaris agent AppImage (runs on glibc >= 2.35 distros).
#
# The build happens inside an Ubuntu 22.04 container with Qt 6.8 and a trimmed
# FFmpeg 8.1 (+ x264, SVT-AV1) — see Containerfile.appimage.
# Output: dist/Lunaris-Agent-*.AppImage
#
# Env: CONTAINER=docker       use docker instead of podman
#      PODMAN_ROOT=<dir>      podman storage/temp location (when / is short on space)
#      REBUILD_IMAGE=1        rebuild the build image
#      LUNARIS_MEDIA_PATH=<checkout>  build against a local lunaris-media (unpushed changes)
#      LUNARIS_APPIMAGE_TARGET_DIR / LUNARIS_CARGO_CACHE  cargo target / registry dirs
#                             (default: build-dir/, so CI can cache them)
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
HERE="$ROOT/scripts/packaging"
IMAGE=localhost/lunaris-appimage-build:22.04
ENGINE="${CONTAINER:-$(command -v podman >/dev/null && echo podman || echo docker)}"
TARGET_DIR="${LUNARIS_APPIMAGE_TARGET_DIR:-$ROOT/build-dir/target-appimage}"
CARGO_CACHE="${LUNARIS_CARGO_CACHE:-$ROOT/build-dir/cargo-cache}"
OUT_DIR="$ROOT/dist"

if [[ "$ENGINE" == podman && -n "${PODMAN_ROOT:-}" ]]; then
    mkdir -p "$PODMAN_ROOT/storage" "$PODMAN_ROOT/tmp"
    export TMPDIR="$PODMAN_ROOT/tmp"
    ENGINE_CMD=(podman --root "$PODMAN_ROOT/storage" --runroot "${XDG_RUNTIME_DIR:-/tmp}/lunaris-podman")
else
    ENGINE_CMD=("$ENGINE")
fi

if [[ -n "${REBUILD_IMAGE:-}" ]] || ! "${ENGINE_CMD[@]}" image inspect "$IMAGE" >/dev/null 2>&1; then
    "${ENGINE_CMD[@]}" build -t "$IMAGE" -f "$HERE/Containerfile.appimage" "$HERE"
fi

mkdir -p "$TARGET_DIR" "$OUT_DIR" "$CARGO_CACHE/registry" "$CARGO_CACHE/git"
if [[ "$ENGINE" == podman ]]; then
    RUN_ARGS=(--userns=keep-id)          # output files owned by the caller
else
    RUN_ARGS=(--user "$(id -u):$(id -g)")
fi

# NvFBC and CUDA are loaded at runtime (dlopen), so one binary runs everywhere.
CARGO_ARGS="--release -p agent-app --target-dir /target"
if [[ -n "${LUNARIS_MEDIA_PATH:-}" ]]; then
    RUN_ARGS+=(-v "$(cd "$LUNARIS_MEDIA_PATH" && pwd):/lunaris-media:ro")
    cat > "$TARGET_DIR/media-patch.toml" <<'EOF'
[patch."https://github.com/collyn/lunaris-media.git"]
lunaris-media = { path = "/lunaris-media" }
EOF
    CARGO_ARGS+=" --config /target/media-patch.toml"
    # The override rewrites Cargo.lock; put the pinned one back afterwards.
    cp "$ROOT/Cargo.lock" "$TARGET_DIR/Cargo.lock.pinned"
    trap 'cp "$TARGET_DIR/Cargo.lock.pinned" "$ROOT/Cargo.lock"' EXIT
else
    CARGO_ARGS+=" --locked"
fi

"${ENGINE_CMD[@]}" run --rm "${RUN_ARGS[@]}" \
    -e CARGO_ARGS="$CARGO_ARGS" -e HOME=/tmp \
    -v "$ROOT:/src" \
    -v "$TARGET_DIR:/target" \
    -v "$OUT_DIR:/out" \
    -v "$CARGO_CACHE/registry:/opt/cargo/registry" \
    -v "$CARGO_CACHE/git:/opt/cargo/git" \
    -w /src \
    "$IMAGE" \
    bash -c 'cargo build $CARGO_ARGS \
             && /src/scripts/packaging/make-agent-appimage.sh /target/release/agent /out'
