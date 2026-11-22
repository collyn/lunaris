#!/usr/bin/env bash
# Builds the Linux distro packages inside a matching container:
#   deb   ubuntu:24.04   → lunaris-{agent,approval-helper,client}_<ver>_amd64.deb (Ubuntu 24.04+, Debian 13+)
#                          one package for all of them: built on the oldest release, checked
#                          on each by smoke-deb.sh (run in ubuntu:24.04, ubuntu:26.04, debian:13)
#   rpm   fedora:44      → lunaris-{agent,approval-helper,client}-<ver>-1.fcNN.x86_64.rpm
#   arch  archlinux      → lunaris-{agent,approval-helper,client}-<ver>-1-x86_64.pkg.tar.zst (makepkg)
#
# Usage (as root in the container, repo at the working directory):
#   scripts/packaging/linux/build-packages.sh deb|rpm|arch <output dir>
# Env: FFMPEG_PREFIX  where the static FFmpeg is built/cached (default /opt/lunaris-ffmpeg)
#      CARGO_TARGET_DIR  cargo target dir (default ./target)
set -euo pipefail

FORMAT="$1"; OUT="$(mkdir -p "$2" && cd "$2" && pwd)"
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)"
FFMPEG_PREFIX="${FFMPEG_PREFIX:-/opt/lunaris-ffmpeg}"
TARGET="${CARGO_TARGET_DIR:-$ROOT/target}"
RUST_TOOLCHAIN=nightly-2026-04-17
cd "$ROOT"
# Absolute: make-deb.sh and stage.sh use it from other directories.
TARGET="$(mkdir -p "$TARGET" && cd "$TARGET" && pwd)"

COMMON_TOOLS_NOTE="toolchain, nasm, meson, ninja, cmake, git, pkg-config, clang"

install_deps() {
    case "$FORMAT" in
    deb)
        export DEBIAN_FRONTEND=noninteractive
        for i in 1 2 3 4 5; do apt-get -o Acquire::Retries=5 update && break; sleep 20; done
        apt-get install -y --no-install-recommends \
            build-essential clang libclang-dev cmake meson ninja-build nasm pkg-config git curl \
            ca-certificates python3 file dpkg-dev \
            libva-dev libdrm-dev libpipewire-0.3-dev libspa-0.2-dev libasound2-dev libopus-dev \
            libssl-dev libx11-dev libxtst-dev libxfixes-dev libxext-dev libxrandr-dev libegl-dev \
            libgl-dev libgbm-dev libudev-dev libdbus-1-dev \
            qt6-base-dev qt6-base-private-dev qt6-declarative-dev qt6-multimedia-dev
        ;;
    rpm)
        dnf -y install --setopt=install_weak_deps=False \
            gcc gcc-c++ clang clang-devel cmake meson ninja-build nasm pkgconf-pkg-config git \
            ca-certificates python3 file rpm-build \
            libva-devel libdrm-devel pipewire-devel alsa-lib-devel opus-devel openssl-devel \
            libX11-devel libXtst-devel libXfixes-devel libXext-devel libXrandr-devel \
            mesa-libEGL-devel mesa-libGL-devel mesa-libgbm-devel systemd-devel dbus-devel \
            qt6-qtbase-devel qt6-qtbase-private-devel qt6-qtdeclarative-devel qt6-qtmultimedia-devel
        ;;
    arch)
        pacman -Syu --noconfirm --needed base-devel rust clang cmake meson ninja nasm git pkgconf \
            qt6-base qt6-declarative qt6-multimedia pipewire libpipewire libva libdrm opus \
            alsa-lib openssl libx11 libxtst libxfixes libxext libxrandr mesa
        ;;
    *) echo "unknown format: $FORMAT (deb|rpm|arch)" >&2; exit 1 ;;
    esac
}

install_rust() {
    if ! command -v cargo >/dev/null; then
        # rustup-init 1.29.3, verified against its published SHA-256 (not piped into a shell)
        local triple sha tmp
        case "$(uname -m)" in
            x86_64)  triple=x86_64-unknown-linux-gnu;  sha=b1c8cb14e30889e7e2999e6b69e62d9e9b9ffc0f4df8180564d306d9680e6c55 ;;
            aarch64) triple=aarch64-unknown-linux-gnu; sha=9f608c142e0e38a78957d0661ba1dfaf0b6fb37184cfaa4b9a0aa57af6184282 ;;
            *) echo "no pinned rustup-init for $(uname -m); install rust first" >&2; exit 1 ;;
        esac
        tmp=$(mktemp -d)
        curl --proto '=https' --tlsv1.2 -fsSLo "$tmp/rustup-init" "https://static.rust-lang.org/rustup/archive/1.29.3/$triple/rustup-init"
        echo "$sha  $tmp/rustup-init" | sha256sum -c -
        chmod +x "$tmp/rustup-init"
        "$tmp/rustup-init" -y --profile minimal --default-toolchain "$RUST_TOOLCHAIN"
        rm -rf "$tmp"
    fi
    # shellcheck disable=SC1091
    [[ -f "$HOME/.cargo/env" ]] && . "$HOME/.cargo/env"
}

build_ffmpeg() {
    if [[ -f "$FFMPEG_PREFIX/lib/pkgconfig/libavcodec.pc" ]]; then
        echo "Using cached static FFmpeg in $FFMPEG_PREFIX"
    else
        bash scripts/packaging/build-ffmpeg-static.sh "$FFMPEG_PREFIX"
    fi
}

build_binaries() {
    export PKG_CONFIG_PATH="$FFMPEG_PREFIX/lib/pkgconfig" QMAKE=/usr/bin/qmake6
    # (The client is the stream window only: it embeds no web client. The
    # server, which does, is packaged by make-server-deb.sh.)
    # One cargo run for all programs: built separately, the crates they
    # share (agent, lunaris-media, webrtc, ...) get different features and
    # are compiled twice (7 more minutes in CI). The agent links no Qt; the
    # approval helper and the client do.
    cargo build --release --locked -p agent -p approval-helper -p client-qml \
        --features agent/static-ffmpeg,client-qml/static-ffmpeg --target-dir "$TARGET"
    for b in lunaris-agent client-desktop; do
        if ldd "$TARGET/release/$b" | grep -qE 'libav(codec|util)|libx264'; then
            echo "error: $b still links FFmpeg dynamically" >&2; exit 1
        fi
    done
    if ldd "$TARGET/release/lunaris-agent" | grep -q 'libQt6'; then
        echo "error: lunaris-agent links Qt" >&2; exit 1
    fi
}

install_deps
case "$FORMAT" in
deb)
    install_rust; build_ffmpeg; build_binaries
    bash scripts/packaging/linux/make-deb.sh "$TARGET" "$OUT" "$FFMPEG_PREFIX"
    ;;
rpm)
    install_rust; build_ffmpeg; build_binaries
    bash scripts/packaging/linux/make-rpm.sh "$TARGET" "$OUT" "$FFMPEG_PREFIX"
    ;;
arch)
    # makepkg refuses to run as root; build the PKGBUILD as an unprivileged user
    # from a tarball of this checkout (named like the PKGBUILD's source).
    version="$(sed -n '/^\[workspace.package\]/,/^\[/s/^version *= *"\(.*\)"/\1/p' Cargo.toml | head -1)"
    id builder >/dev/null 2>&1 || useradd -m builder
    work="$(mktemp -d)"
    cp scripts/packaging/arch/PKGBUILD scripts/packaging/arch/lunaris-agent.install "$work/"
    sed -i "s/^pkgver=.*/pkgver=$version/" "$work/PKGBUILD"
    git config --global --add safe.directory "$ROOT"
    git -C "$ROOT" archive --prefix="lunaris-$version/" -o "$work/lunaris-$version.tar.gz" HEAD
    chown -R builder: "$work"
    su builder -c "cd '$work' && makepkg --noconfirm --skipchecksums"
    cp "$work"/lunaris-*.pkg.tar.zst "$OUT/"
    for f in "$OUT"/*.pkg.tar.zst; do echo "Built $f"; done
    ;;
esac
