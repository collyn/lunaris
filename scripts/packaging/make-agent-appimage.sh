#!/usr/bin/env bash
# Assemble Lunaris-Agent-<version>-x86_64.AppImage from a built agent binary.
# Runs inside the build container (see build-agent-appimage.sh).
#
# Usage: make-agent-appimage.sh <agent-binary> <output-dir>
set -euo pipefail

BIN="$1"
OUT="$2"
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
VERSION="$(sed -n '/^\[workspace.package\]/,/^\[/s/^version *= *"\(.*\)"/\1/p' "$ROOT/Cargo.toml" | head -1)"
DEPS=/opt/deps

WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT
APPDIR="$WORK/AppDir"

install -Dm755 "$BIN" "$APPDIR/usr/bin/lunaris-agent"
install -Dm644 "$ROOT/agent/qml/icons/icon.png" "$APPDIR/usr/share/icons/hicolor/256x256/apps/lunaris-agent.png"
install -d "$APPDIR/usr/share/applications"
cat > "$APPDIR/usr/share/applications/lunaris-agent.desktop" <<'EOF'
[Desktop Entry]
Type=Application
Name=Lunaris Agent
Comment=Share this computer's screen with Lunaris clients
Exec=lunaris-agent
Icon=lunaris-agent
Terminal=false
Categories=Network;RemoteAccess;
EOF

# Host setup (uinput access for remote keyboard/mouse), run via `--setup-host`.
SHARE="$APPDIR/usr/share/lunaris-agent"
install -Dm755 "$ROOT/scripts/linux-host/setup-host.sh" "$SHARE/setup-host.sh"
install -Dm644 "$ROOT/scripts/linux-host/60-lunaris-input.rules" "$SHARE/60-lunaris-input.rules"
install -Dm644 "$ROOT/scripts/linux-host/60-lunaris.conf" "$SHARE/60-lunaris.conf"

# Third-party licences (LGPL requires shipping the texts + where to get sources).
LIC="$APPDIR/usr/share/licenses"
mkdir -p "$LIC"
cp -r "$DEPS/share/licenses/." "$LIC/"
install -Dm644 "$QTDIR/../../Licenses/LICENSE.LGPL3" "$LIC/qt/LICENSE.LGPL3" 2>/dev/null \
    || find "$QTDIR/.." -maxdepth 3 -iname 'LICENSE.LGPL*' -exec install -Dm644 {} "$LIC/qt/" \; -quit 2>/dev/null || true
install -Dm644 "$ROOT/scripts/packaging/THIRD-PARTY-NOTICES.md" "$LIC/THIRD-PARTY-NOTICES.md"

# AppRun: keep agent_config.json in a per-user directory and offer --setup-host.
cat > "$WORK/AppRun" <<'EOF'
#!/bin/sh
HERE="$(dirname "$(readlink -f "$0")")"
if [ "$1" = "--setup-host" ]; then
    # The AppImage mount is not readable by root, so stage the files first.
    tmp="$(mktemp -d)"
    cp "$HERE"/usr/share/lunaris-agent/* "$tmp/"
    shift
    if [ "$(id -u)" -eq 0 ]; then sh "$tmp/setup-host.sh" "$@"
    elif command -v pkexec >/dev/null 2>&1 && [ -n "$DISPLAY$WAYLAND_DISPLAY" ]; then
        pkexec env SUDO_USER="$(id -un)" sh "$tmp/setup-host.sh" "$@"
    else sudo sh "$tmp/setup-host.sh" "$@"
    fi
    rc=$?; rm -rf "$tmp"; exit $rc
fi
if [ "$1" = "--licenses" ]; then
    echo "Third-party licences: $HERE/usr/share/licenses"
    cat "$HERE/usr/share/licenses/THIRD-PARTY-NOTICES.md"; exit 0
fi
dir="${XDG_CONFIG_HOME:-$HOME/.config}/lunaris-agent"
mkdir -p "$dir" && cd "$dir" || exit 1
exec "$HERE/usr/bin/lunaris-agent" "$@"
EOF
chmod 755 "$WORK/AppRun"

export QML_SOURCES_PATHS="$ROOT/agent/qml"
export EXTRA_QT_MODULES="waylandclient"
export EXTRA_PLATFORM_PLUGINS="libqwayland-egl.so;libqwayland-generic.so"
export LDAI_COMP=zstd
export APPIMAGE_EXTRACT_AND_RUN=1 ARCH=x86_64
export LINUXDEPLOY_OUTPUT_VERSION="$VERSION"
export OUTPUT="$OUT/Lunaris-Agent-${VERSION}-x86_64.AppImage"
export LD_LIBRARY_PATH="$QTDIR/lib:$DEPS/lib:${LD_LIBRARY_PATH:-}"

# Never bundle: PipeWire (must match the running daemon), GPU vendor driver
# libraries, and libs on the AppImage excludelist (glibc, libGL, libdrm, X11...).
cd "$WORK"
linuxdeploy-x86_64 --appdir "$APPDIR" \
    --custom-apprun "$WORK/AppRun" \
    --desktop-file "$APPDIR/usr/share/applications/lunaris-agent.desktop" \
    --icon-file "$APPDIR/usr/share/icons/hicolor/256x256/apps/lunaris-agent.png" \
    --exclude-library 'libpipewire-0.3.so*' \
    --exclude-library 'libnvidia*' --exclude-library 'libcuda*' \
    --plugin qt --output appimage

ls -l "$OUTPUT"
