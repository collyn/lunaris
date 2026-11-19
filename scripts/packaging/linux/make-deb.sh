#!/usr/bin/env bash
# Builds lunaris-agent_<ver>_<arch>.deb and lunaris-client_<ver>_<arch>.deb
# (Ubuntu 24.04+, Debian 13+). Run on the build distro: dpkg-shlibdeps computes
# the shared-library dependencies (Qt, PipeWire, libva, ...) against it.
#
# One package serves every release from the build distro (Ubuntu 24.04) up, so
# the Qt dependencies name both spellings of each library package, see
# qt_package_alternatives. scripts/packaging/linux/smoke-deb.sh installs and
# starts the result on each supported release.
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

# Ubuntu 24.04 ships the Qt libraries as libqt6gui6t64, libqt6widgets6t64,
# libqt6opengl6t64 ...; Ubuntu 26.04 (Qt 6.10) and Debian 13 (Qt 6.8) name them
# libqt6gui6, libqt6widgets6, libqt6opengl6 again, with no Provides for the old
# names (libqt6core6t64 kept its name). Without the second name apt refuses the
# package there: "Depends: libqt6gui6t64 but it is not installable". The
# libraries are the same public Qt 6 ABI on 64-bit, so either name will do:
#   libqt6gui6t64 (>= 6.1.2)  ->  libqt6gui6t64 (>= 6.1.2) | libqt6gui6 (>= 6.1.2)
qt_package_alternatives() {
    sed -E 's/(libqt6[a-z0-9-]+)t64( \([^)]*\))?/\1t64\2 | \1\2/g'
}

# VA-API drivers: mesa-va-drivers covers AMD; Intel GPUs need Intel's own
# driver (iHD, Broadwell and newer), which Ubuntu does not install by
# default. Without it VAAPI and Quick Sync cannot start and the agent
# encodes on the CPU ("Failed to initialise VAAPI connection").
INTEL_VA="intel-media-va-driver | intel-media-va-driver-non-free"

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
    # A dependency on Qt's private ABI pins the package to the exact Qt version
    # of the build distro; the alternatives below would then promise too much.
    if grep -q 'private-abi' <<<"$shlibs"; then
        echo "error: $name uses Qt private API ($shlibs): it would only run with the build distro's Qt" >&2
        exit 1
    fi
    shlibs="$(qt_package_alternatives <<<"$shlibs")"

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

    # The agent package also runs the capture service (unattended capture on
    # Wayland without the portal prompt, docs/linux-capture-service.md).
    if [[ "$kind" == agent ]]; then
        cat >> "$root/DEBIAN/postinst" <<'EOF'
if [ "$1" = configure ] && [ -d /run/systemd/system ]; then
    systemctl daemon-reload >/dev/null 2>&1 || true
    if [ -z "$2" ]; then
        systemctl enable --now lunaris-capture.service >/dev/null 2>&1 || true
    else
        # Upgrade: keep the admin's choice, run the new binary.
        systemctl try-restart lunaris-capture.service >/dev/null 2>&1 || true
    fi
fi
EOF
        cat > "$root/DEBIAN/prerm" <<'EOF'
#!/bin/sh
set -e
if [ "$1" = remove ] && [ -d /run/systemd/system ]; then
    systemctl disable --now lunaris-capture.service >/dev/null 2>&1 || true
fi
EOF
        cat > "$root/DEBIAN/postrm" <<'EOF'
#!/bin/sh
set -e
if [ -d /run/systemd/system ]; then
    systemctl daemon-reload >/dev/null 2>&1 || true
fi
EOF
        chmod 755 "$root/DEBIAN/prerm" "$root/DEBIAN/postrm"
    fi

    dpkg-deb --build --root-owner-group "$root" "$OUT/${name}_${VERSION}_${ARCH}.deb" >/dev/null
    echo "Built $OUT/${name}_${VERSION}_${ARCH}.deb"
    dpkg-deb --field "$OUT/${name}_${VERSION}_${ARCH}.deb" Depends
    rm -rf "$work"
}

build agent "$TARGET/release/agent" "" \
    ", xdg-desktop-portal, pipewire, mesa-va-drivers, $INTEL_VA" \
    "Lunaris remote desktop host agent
 Shares this computer's screen with Lunaris clients: unattended KMS capture
 through the lunaris-capture service (Wayland without the portal prompt),
 PipeWire/portal or X11 otherwise, hardware encoding (VAAPI, NVENC, Quick
 Sync) and remote keyboard/mouse through uinput."

build client "$TARGET/release/client-desktop" \
    ", qml6-module-qtmultimedia, qml6-module-qtquick-shapes" \
    ", mesa-va-drivers, $INTEL_VA, xdg-utils, xdg-desktop-portal, pipewire" \
    "Lunaris remote desktop
 Control other computers running Lunaris, with hardware video decoding
 (VAAPI, NVDEC), and let them control this one (built-in agent and server:
 lunaris-client agent, lunaris-client server)."
