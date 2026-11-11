#!/usr/bin/env bash
# Builds lunaris-agent_<ver>_<arch>.deb and lunaris-client_<ver>_<arch>.deb
# (Ubuntu 24.04+, Debian 13+). Run on the build distro: dpkg-shlibdeps computes
# the shared-library dependencies (Qt, PipeWire, libva, ...) against it.
#
# Usage: make-deb.sh <cargo target dir> <output dir> <static ffmpeg prefix>
set -euo pipefail

TARGET="$1"; OUT="$(mkdir -p "$2" && cd "$2" && pwd)"; FFMPEG="$3"
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$HERE/../../.." && pwd)"
VERSION="$(sed -n '/^\[workspace.package\]/,/^\[/s/^version *= *"\(.*\)"/\1/p' "$ROOT/Cargo.toml" | head -1)"
ARCH="$(dpkg --print-architecture)"

# Loaded at runtime (QML imports, Qt platform plugins), so invisible to shlibdeps.
QML_COMMON="qml6-module-qtquick, qml6-module-qtquick-controls, qml6-module-qtquick-layouts, \
qml6-module-qtquick-templates, qml6-module-qtquick-window, qml6-module-qtqml-workerscript, \
qml6-module-qt-labs-platform, qt6-qpa-plugins"

build() {
    local kind="$1" bin="$2" extra_deps="$3" recommends="$4" description="$5"
    local name="lunaris-$kind"
    local work; work="$(mktemp -d)"
    local root="$work/$name"

    "$HERE/stage.sh" "$kind" "$bin" "$root" "$FFMPEG"
    strip --strip-unneeded "$root/usr/bin/$name"
    install -Dm644 "$ROOT/LICENSE" "$root/usr/share/doc/$name/copyright"

    mkdir -p "$work/debian"
    printf 'Source: %s\n' "$name" > "$work/debian/control"
    local shlibs
    shlibs="$(cd "$work" && dpkg-shlibdeps -O -e "$root/usr/bin/$name" 2>/dev/null | sed -n 's/^shlibs:Depends=//p')"

    mkdir -p "$root/DEBIAN"
    cat > "$root/DEBIAN/control" <<EOF
Package: $name
Version: $VERSION
Architecture: $ARCH
Maintainer: Lunaris <noreply@lunaris.invalid>
Section: net
Priority: optional
Homepage: https://github.com/collyn/lunaris
Installed-Size: $(du -sk "$root" | cut -f1)
Depends: $shlibs, $QML_COMMON$extra_deps
Recommends: qt6-wayland$recommends
Description: $description
EOF

    # Both packages give remote keyboard/mouse (the app is also the agent).
    if [[ "$kind" == agent || "$kind" == client ]]; then
        cat > "$root/DEBIAN/postinst" <<'EOF'
#!/bin/sh
set -e
if [ "$1" = configure ]; then
    modprobe uinput 2>/dev/null || true
    if command -v udevadm >/dev/null; then
        udevadm control --reload-rules 2>/dev/null || true
        udevadm trigger --subsystem-match=misc --sysname-match=uinput 2>/dev/null || true
    fi
fi
EOF
        chmod 755 "$root/DEBIAN/postinst"
    fi

    dpkg-deb --build --root-owner-group "$root" "$OUT/${name}_${VERSION}_${ARCH}.deb" >/dev/null
    echo "Built $OUT/${name}_${VERSION}_${ARCH}.deb"
    dpkg-deb --field "$OUT/${name}_${VERSION}_${ARCH}.deb" Depends
    rm -rf "$work"
}

build agent "$TARGET/release/agent" "" \
    ", xdg-desktop-portal, pipewire, mesa-va-drivers" \
    "Lunaris remote desktop host agent
 Shares this computer's screen with Lunaris clients: PipeWire/portal capture
 on Wayland (X11 otherwise), hardware encoding (VAAPI, NVENC, Quick Sync) and
 remote keyboard/mouse through uinput."

build client "$TARGET/release/client-desktop" \
    ", qml6-module-qtmultimedia, qml6-module-qtquick-shapes" \
    ", mesa-va-drivers, xdg-utils, xdg-desktop-portal, pipewire" \
    "Lunaris remote desktop
 Control other computers running Lunaris, with hardware video decoding
 (VAAPI, NVDEC), and let them control this one (built-in agent and server:
 lunaris-client agent, lunaris-client server)."
