#!/usr/bin/env bash
# Assemble a Lunaris AppImage from a built binary. Runs inside the build container
# (see build-appimages.sh).
#
# Usage: make-appimage.sh <agent|client-desktop|client-qml> <binary> <output-dir>
#   agent           Lunaris-Agent-<version>-x86_64.AppImage   (agent-app: the agent with its window)
#   client-desktop  client-desktop-x86_64.AppImage            (the unified app: client + agent + server)
#   client-qml      client-qml-x86_64.AppImage                (the same program, kept for old links)
#
# Env: APPIMAGE_ZSTD_LEVEL  squashfs zstd level (default 22)
#      SKIP_SMOKE=1    skip the start-up check at the end
set -euo pipefail

VARIANT="$1"
BIN="$2"
OUT="$3"
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
HERE="$ROOT/scripts/packaging"
VERSION="$(sed -n '/^\[workspace.package\]/,/^\[/s/^version *= *"\(.*\)"/\1/p' "$ROOT/Cargo.toml" | head -1)"
DEPS=/opt/deps

case "$VARIANT" in
    agent)
        NAME=lunaris-agent TITLE="Lunaris Agent" ICON_NAME=lunaris-agent
        COMMENT="Share this computer's screen with Lunaris clients"
        QML_DIR="$ROOT/agent-app/qml" ICON_SRC="$ROOT/agent-app/qml/icons/icon.png" ICON_SIZE=256
        EXEC=lunaris-agent MIME="" CONFIG_DIR=lunaris-agent
        OUTFILE="$OUT/Lunaris-Agent-${VERSION}-x86_64.AppImage" SMOKE_ARGS=(--minimized)
        ;;
    client-desktop | client-qml)
        NAME=$VARIANT TITLE="Lunaris Client" ICON_NAME=lunaris-client
        COMMENT="Connect to computers running the Lunaris agent"
        QML_DIR="$ROOT/client-qml/qml" EXEC=$VARIANT MIME="MimeType=x-scheme-handler/lunaris;" CONFIG_DIR=""
        if [[ "$VARIANT" == client-desktop ]]; then
            ICON_SRC="$ROOT/client-desktop/icons/128x128.png" ICON_SIZE=128
        else
            ICON_SRC="$ROOT/client-qml/qml/icon.png" ICON_SIZE=256
        fi
        OUTFILE="$OUT/${VARIANT}-x86_64.AppImage" SMOKE_ARGS=()
        ;;
    *) echo "usage: $0 <agent|client-desktop|client-qml> <binary> <output-dir>" >&2; exit 2 ;;
esac

WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT
APPDIR="$WORK/AppDir"
USR="$APPDIR/usr"

install -Dm755 "$BIN" "$USR/bin/$NAME"
# The Rust binaries are built with symbols; they are not needed at run time.
strip --strip-unneeded "$USR/bin/$NAME"
install -Dm644 "$ICON_SRC" "$USR/share/icons/hicolor/${ICON_SIZE}x${ICON_SIZE}/apps/$ICON_NAME.png"
install -d "$USR/share/applications"
cat > "$USR/share/applications/$NAME.desktop" <<DESKTOP
[Desktop Entry]
Type=Application
Name=$TITLE
Comment=$COMMENT
Exec=$EXEC %u
Icon=$ICON_NAME
Terminal=false
Categories=Network;RemoteAccess;
$MIME
DESKTOP

# Host setup (uinput access for remote keyboard/mouse), run via `--setup-host`.
SHARE="$USR/share/$NAME"
install -Dm755 "$ROOT/scripts/linux-host/setup-host.sh" "$SHARE/setup-host.sh"
install -Dm644 "$ROOT/scripts/linux-host/60-lunaris-input.rules" "$SHARE/60-lunaris-input.rules"
install -Dm644 "$ROOT/scripts/linux-host/60-lunaris.conf" "$SHARE/60-lunaris.conf"

# Lunaris' own licence plus the third-party licence texts (GPL/LGPL require
# shipping them, together with where to get the corresponding source).
LIC="$USR/share/licenses"
mkdir -p "$LIC"
install -Dm644 "$ROOT/LICENSE" "$LIC/lunaris/LICENSE"
cp -r "$DEPS/share/licenses/." "$LIC/"
install -Dm644 "$QTDIR/../../Licenses/LICENSE.LGPL3" "$LIC/qt/LICENSE.LGPL3" 2>/dev/null \
    || find "$QTDIR/.." -maxdepth 3 -iname 'LICENSE.LGPL*' -exec install -Dm644 {} "$LIC/qt/" \; -quit 2>/dev/null || true
install -Dm644 "$HERE/THIRD-PARTY-NOTICES.md" "$LIC/THIRD-PARTY-NOTICES.md"

# AppRun: offer --setup-host / --licenses; the agent also keeps agent_config.json in
# a per-user directory (it is read from the working directory).
cat > "$WORK/AppRun" <<EOF
#!/bin/sh
HERE="\$(dirname "\$(readlink -f "\$0")")"
if [ "\$1" = "--setup-host" ]; then
    # The AppImage mount is not readable by root, so stage the files first.
    tmp="\$(mktemp -d)"
    cp "\$HERE"/usr/share/$NAME/* "\$tmp/"
    shift
    if [ "\$(id -u)" -eq 0 ]; then bash "\$tmp/setup-host.sh" "\$@"
    elif command -v pkexec >/dev/null 2>&1 && [ -n "\$DISPLAY\$WAYLAND_DISPLAY" ]; then
        pkexec env SUDO_USER="\$(id -un)" bash "\$tmp/setup-host.sh" "\$@"
    else sudo bash "\$tmp/setup-host.sh" "\$@"
    fi
    rc=\$?; rm -rf "\$tmp"; exit \$rc
fi
if [ "\$1" = "--licenses" ]; then
    echo "Third-party licences: \$HERE/usr/share/licenses"
    cat "\$HERE/usr/share/licenses/THIRD-PARTY-NOTICES.md"; exit 0
fi
EOF
if [[ -n "$CONFIG_DIR" ]]; then
    cat >> "$WORK/AppRun" <<EOF
dir="\${XDG_CONFIG_HOME:-\$HOME/.config}/$CONFIG_DIR"
mkdir -p "\$dir" && cd "\$dir" || exit 1
EOF
fi
# VA-API: the video drivers are the host's, and a driver built for a newer libva
# does not load into the older bundled one ("radeonsi_drv_video.so has no function
# __vaDriverInit_1_0": no hardware decoding or encoding at all). So the host's
# libva is used when it has one, and the bundled copy only when it has none (the
# binary needs libva.so.2 to start).
cat >> "$WORK/AppRun" <<EOF
if ! /sbin/ldconfig -p 2>/dev/null | grep -q 'libva\\.so\\.2 '; then
    export LD_LIBRARY_PATH="\$HERE/usr/lib/libva-fallback\${LD_LIBRARY_PATH:+:\$LD_LIBRARY_PATH}"
fi
EOF
echo "exec \"\$HERE/usr/bin/$NAME\" \"\$@\"" >> "$WORK/AppRun"
chmod 755 "$WORK/AppRun"

export QML_SOURCES_PATHS="$QML_DIR"
export EXTRA_QT_MODULES="waylandclient"
export EXTRA_PLATFORM_PLUGINS="libqwayland-egl.so;libqwayland-generic.so"
export APPIMAGE_EXTRACT_AND_RUN=1 ARCH=x86_64
export LINUXDEPLOY_OUTPUT_VERSION="$VERSION"
export LD_LIBRARY_PATH="$QTDIR/lib:$DEPS/lib:${LD_LIBRARY_PATH:-}"

# linuxdeploy args shared by both runs. Never bundle: PipeWire (must match the
# running daemon), GPU vendor driver libraries, and libs on the AppImage
# excludelist (glibc, libGL, libdrm, X11 ...).
LD_ARGS=(--appdir "$APPDIR" --custom-apprun "$WORK/AppRun"
    --desktop-file "$USR/share/applications/$NAME.desktop"
    --icon-file "$USR/share/icons/hicolor/${ICON_SIZE}x${ICON_SIZE}/apps/$ICON_NAME.png"
    --exclude-library 'libpipewire-0.3.so*'
    --exclude-library 'libnvidia*' --exclude-library 'libcuda*')

cd "$WORK"

# 1. Deploy Qt (libraries, plugins, the QML modules the app imports) and their dependencies.
linuxdeploy-x86_64 "${LD_ARGS[@]}" --plugin qt

# 2. Qt Wayland needs its graphics-integration plugin to create an OpenGL context
#    ("wayland-egl"); linuxdeploy-plugin-qt only deploys the platform plugin, and
#    without this directory the app aborts on every Wayland session with
#    "Failed to load client buffer integration: wayland-egl" / "Failed to create RHI".
mkdir -p "$USR/plugins"
cp -a "$QTDIR/plugins/wayland-graphics-integration-client" "$USR/plugins/"
linuxdeploy-x86_64 --appdir "$APPDIR" --exclude-library 'libpipewire-0.3.so*' \
    $(find "$USR/plugins/wayland-graphics-integration-client" -name '*.so' -printf '--deploy-deps-only %p ')

# 3. Prune what the apps never use (see prune_appdir).
# shellcheck source=prune-appdir.sh
. "$HERE/prune-appdir.sh"
prune_appdir "$APPDIR" "$VARIANT"

# 3b. Client: the CPU video path (QVideoSink) must work with what is left.
if [[ "$VARIANT" != agent ]]; then
    "$HERE/videosink-check.sh" "$APPDIR"
fi

# 4. Squashfs image, written by appimagetool (the tool linuxdeploy's appimage plugin
#    wraps) directly, to set the compression: zstd level 22 with 1 MB blocks. The
#    runtime inside this appimagetool reads only zlib and zstd ("Squashfs image uses
#    xz compression, this version supports only zlib, zstd"), so xz (4-5 MB smaller
#    on these bundles, measured with the distro's mksquashfs) is not an option until
#    the runtime is replaced. linuxdeploy already put the .desktop file, icon and
#    AppRun in place.
APPIMAGETOOL=/opt/linuxdeploy-x86_64/plugins/linuxdeploy-plugin-appimage/usr/bin/appimagetool
COMP_ARGS=(--comp zstd --mksquashfs-opt -b --mksquashfs-opt 1M --mksquashfs-opt -Xcompression-level --mksquashfs-opt "${APPIMAGE_ZSTD_LEVEL:-22}")
linuxdeploy-x86_64 "${LD_ARGS[@]}"      # AppDir top level (desktop file, icon, AppRun)
# libva out of the default library path (see AppRun above). After the last
# linuxdeploy run, which would copy it back next to the other libraries.
mkdir -p "$USR/lib/libva-fallback"
find "$USR/lib" -maxdepth 1 -name 'libva*.so*' -exec mv {} "$USR/lib/libva-fallback/" \;
[[ -e "$USR/lib/libva-fallback/libva.so.2" ]] || { echo "libva.so.2 was not bundled" >&2; exit 1; }

rm -f "$OUTFILE"
"$APPIMAGETOOL" --no-appstream "${COMP_ARGS[@]}" "$APPDIR" "$OUTFILE"
ls -l "$OUTFILE"

# 5. Start it with the build machine's Qt hidden.
if [[ -z "${SKIP_SMOKE:-}" ]]; then
    "$HERE/smoke-appimage.sh" "$OUTFILE" 8 "${SMOKE_ARGS[@]}"
fi
