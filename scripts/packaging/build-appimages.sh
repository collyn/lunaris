#!/usr/bin/env bash
# Build the portable Lunaris AppImages (they run on glibc >= 2.35 distros):
# the agent and the client (client-desktop, plus client-qml, the same program
# under its old name).
#
# The build happens inside an Ubuntu 22.04 container with Qt 6.8 and a trimmed,
# static FFmpeg 8.1 (+ x264, SVT-AV1, dav1d) — see Containerfile.appimage. One
# cargo build makes both binaries; make-appimage.sh packs each and runs the
# start-up check (smoke-appimage.sh) in the container; a second check then runs
# the results in a bare ubuntu:22.04 container.
# Output: dist/Lunaris-Agent-*.AppImage, dist/client-desktop-x86_64.AppImage,
#         dist/client-qml-x86_64.AppImage
#
# Env: CONTAINER=docker       use docker instead of podman
#      PODMAN_ROOT=<dir>      podman storage/temp location (when / is short on space)
#      REBUILD_IMAGE=1        rebuild the build image
#      LUNARIS_MEDIA_PATH=<checkout>  build against a local lunaris-media (unpushed changes)
#      LUNARIS_APPIMAGE_TARGET_DIR / LUNARIS_CARGO_CACHE  cargo target / registry dirs
#                             (default: build-dir/, so CI can cache them)
#      LUNARIS_APPIMAGE_OUT_DIR  where the AppImages go (default: dist/)
#      APPIMAGE_ZSTD_LEVEL=22 squashfs zstd level (see make-appimage.sh)
#      SKIP_SMOKE=1           skip the start-up checks
#      SKIP_CLEAN_SMOKE=1     skip only the bare-container check
#      ONLY="agent client-desktop client-qml"  build a subset of the AppImages
#                             (the web client must be built in web/dist for client-*)
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
HERE="$ROOT/scripts/packaging"
IMAGE=localhost/lunaris-appimage-build:22.04
ENGINE="${CONTAINER:-$(command -v podman >/dev/null && echo podman || echo docker)}"
TARGET_DIR="${LUNARIS_APPIMAGE_TARGET_DIR:-$ROOT/build-dir/target-appimage}"
CARGO_CACHE="${LUNARIS_CARGO_CACHE:-$ROOT/build-dir/cargo-cache}"
OUT_DIR="${LUNARIS_APPIMAGE_OUT_DIR:-$ROOT/dist}"
ONLY="${ONLY:-agent client-desktop client-qml}"
CLEAN_IMAGE=docker.io/library/ubuntu:22.04

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
# static-ffmpeg: the FFmpeg of the build image is linked into the binaries.
PKGS=()
[[ " $ONLY " == *" agent "* ]] && PKGS+=(-p agent-app)
[[ " $ONLY " == *" client-"* ]] && PKGS+=(-p client-desktop)
CARGO_ARGS="--release ${PKGS[*]} --features static-ffmpeg --target-dir /target"
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
    -e CARGO_ARGS="$CARGO_ARGS" -e HOME=/tmp -e ONLY="$ONLY" \
    -e APPIMAGE_ZSTD_LEVEL="${APPIMAGE_ZSTD_LEVEL:-22}" -e SKIP_SMOKE="${SKIP_SMOKE:-}" \
    -v "$ROOT:/src" \
    -v "$TARGET_DIR:/target" \
    -v "$OUT_DIR:/out" \
    -v "$CARGO_CACHE/registry:/opt/cargo/registry" \
    -v "$CARGO_CACHE/git:/opt/cargo/git" \
    -w /src \
    "$IMAGE" \
    bash -c 'set -e
             cargo build $CARGO_ARGS
             for v in $ONLY; do
                 bin=/target/release/client-desktop; [ "$v" = agent ] && bin=/target/release/agent
                 /src/scripts/packaging/make-appimage.sh "$v" "$bin" /out
             done'

# The same start-up check in a bare Ubuntu 22.04 with only what a desktop provides
# (X11/GL/ALSA/udev system libraries, which AppImages do not bundle; no Qt, no build
# tools): finds libraries the build image happened to provide.
if [[ -z "${SKIP_SMOKE:-}" && -z "${SKIP_CLEAN_SMOKE:-}" ]]; then
    for v in $ONLY; do
        case "$v" in
            agent) f=$(basename "$(ls "$OUT_DIR"/Lunaris-Agent-*-x86_64.AppImage | tail -1)"); args=--minimized ;;
            *)     f="$v-x86_64.AppImage"; args="" ;;
        esac
        "${ENGINE_CMD[@]}" run --rm -v "$HERE:/pkg:ro" -v "$OUT_DIR:/out:ro" "$CLEAN_IMAGE" bash -c '
            export DEBIAN_FRONTEND=noninteractive
            apt-get -o Acquire::Retries=5 update -qq >/dev/null && apt-get install -y -qq --no-install-recommends \
                xvfb xauth libgl1 libgl1-mesa-dri libglx-mesa0 libegl1 libdrm2 libasound2 libudev1 libx11-6 libxext6 \
                libxfixes3 libxtst6 libfontconfig1 libxkbcommon-x11-0 libxcb-cursor0 libxcb-icccm4 libxcb-image0 \
                libxcb-keysyms1 libxcb-randr0 libxcb-render-util0 libxcb-shape0 libxcb-xinerama0 libxcb-xkb1 \
                libdbus-1-3 libpipewire-0.3-0 >/dev/null
            /pkg/smoke-appimage.sh "/out/$0" 8 $1' "$f" "$args"
    done
fi
