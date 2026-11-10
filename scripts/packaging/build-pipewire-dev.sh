#!/usr/bin/env bash
# Installs PipeWire 1.0 headers + libpipewire into <prefix>, to compile against.
#
# The pipewire/libspa 0.8 crates match the PipeWire 1.0 API; newer headers
# (Fedora 44, Arch) changed structs they use (e.g. spa_pod_builder). Binaries
# built against 1.0 still run with the newer system libpipewire, which stays
# ABI compatible — only the build uses this copy.
#
# Needs: a C toolchain, meson, ninja, git, pkg-config, dbus and glib dev files.
# Usage: build-pipewire-dev.sh <prefix>
set -euo pipefail

PREFIX="$(mkdir -p "$1" && cd "$1" && pwd)"
PIPEWIRE_VERSION=1.0.7
WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT

git clone -q --depth 1 -b "$PIPEWIRE_VERSION" https://gitlab.freedesktop.org/pipewire/pipewire.git "$WORK/pipewire"
meson setup "$WORK/build" "$WORK/pipewire" --prefix="$PREFIX" --libdir=lib -Dauto_features=disabled \
    -Dsession-managers=[] -Dexamples=disabled -Dtests=disabled -Dman=disabled -Ddocs=disabled >/dev/null
ninja -C "$WORK/build" install >/dev/null
echo "PipeWire $PIPEWIRE_VERSION development files installed in $PREFIX"
