#!/usr/bin/env bash
# Builds the Linux distro packages inside a matching container:
#   deb   ubuntu:24.04   → lunaris-{agent,client}_<ver>_amd64.deb (Ubuntu 24.04+, Debian 13+)
#   rpm   fedora:44      → lunaris-{agent,client}-<ver>-1.fcNN.x86_64.rpm
#   arch  archlinux      → lunaris-{agent,client}-<ver>-1-x86_64.pkg.tar.zst (makepkg)
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
            mesa-libEGL-devel mesa-libGL-devel mesa-libgbm-devel systemd-devel dbus-devel glib2-devel \
            qt6-qtbase-devel qt6-qtbase-private-devel qt6-qtdeclarative-devel qt6-qtmultimedia-devel
        ;;
    arch)
        pacman -Syu --noconfirm --needed base-devel rust clang cmake meson ninja nasm git pkgconf \
            qt6-base qt6-declarative qt6-multimedia pipewire libpipewire libva libdrm opus \
            alsa-lib openssl libx11 libxtst libxfixes libxext libxrandr mesa dbus glib2
        ;;
    *) echo "unknown format: $FORMAT (deb|rpm|arch)" >&2; exit 1 ;;
    esac
}

install_rust() {
    if ! command -v cargo >/dev/null; then
        curl -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal --default-toolchain "$RUST_TOOLCHAIN"
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

# Fedora/Arch ship PipeWire headers newer than the pipewire/libspa crates
# support; compile against PipeWire 1.0 (runtime still uses the system lib).
build_pipewire_dev() {
    if [[ -f "$FFMPEG_PREFIX/pipewire/lib/pkgconfig/libpipewire-0.3.pc" ]]; then
        echo "Using cached PipeWire 1.0 development files"
    else
        bash scripts/packaging/build-pipewire-dev.sh "$FFMPEG_PREFIX/pipewire"
    fi
}

build_binaries() {
    export PKG_CONFIG_PATH="$FFMPEG_PREFIX/lib/pkgconfig" QMAKE=/usr/bin/qmake6
    if [[ "$FORMAT" == rpm ]]; then
        PKG_CONFIG_PATH="$FFMPEG_PREFIX/pipewire/lib/pkgconfig:$PKG_CONFIG_PATH"
    fi
    cargo build --release --locked -p agent --features static-ffmpeg --target-dir "$TARGET"
    cargo build --release --locked -p client-desktop --features static-ffmpeg --target-dir "$TARGET"
    for b in agent client-desktop; do
        if ldd "$TARGET/release/$b" | grep -qE 'libav(codec|util)|libx264'; then
            echo "error: $b still links FFmpeg dynamically" >&2; exit 1
        fi
    done
}

install_deps
case "$FORMAT" in
deb)
    install_rust; build_ffmpeg; build_binaries
    bash scripts/packaging/linux/make-deb.sh "$TARGET" "$OUT" "$FFMPEG_PREFIX"
    ;;
rpm)
    install_rust; build_ffmpeg; build_pipewire_dev; build_binaries
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
