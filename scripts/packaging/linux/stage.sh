#!/usr/bin/env bash
# Lays out the files of one Linux package under a staging root (shared by the
# .deb, .rpm and Arch builds).
#
# Usage: stage.sh agent|client <built binary> <destdir> [<ffmpeg prefix with share/licenses>]
set -euo pipefail

KIND="$1"; BIN="$2"; DEST="$3"; FFMPEG_PREFIX="${4:-}"
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$HERE/../../.." && pwd)"
NAME="lunaris-$KIND"

install -Dm755 "$BIN" "$DEST/usr/bin/$NAME"
install -Dm644 "$HERE/$NAME.desktop" "$DEST/usr/share/applications/$NAME.desktop"
case "$KIND" in
    agent)
        install -Dm644 "$ROOT/agent/qml/icons/icon.png" "$DEST/usr/share/icons/hicolor/256x256/apps/$NAME.png"
        # Remote keyboard/mouse via /dev/uinput (same files as scripts/linux-host).
        install -Dm644 "$ROOT/scripts/linux-host/60-lunaris-input.rules" "$DEST/usr/lib/udev/rules.d/60-lunaris-input.rules"
        install -Dm644 "$ROOT/scripts/linux-host/60-lunaris.conf" "$DEST/usr/lib/modules-load.d/lunaris.conf"
        ;;
    client)
        install -Dm644 "$ROOT/client-qml/qml/icon.png" "$DEST/usr/share/icons/hicolor/256x256/apps/$NAME.png"
        ;;
    *) echo "unknown package kind: $KIND" >&2; exit 1 ;;
esac

LIC="$DEST/usr/share/licenses/$NAME"
install -Dm644 "$ROOT/LICENSE" "$LIC/LICENSE"
install -Dm644 "$ROOT/scripts/packaging/THIRD-PARTY-NOTICES.md" "$LIC/THIRD-PARTY-NOTICES.md"
if [[ -n "$FFMPEG_PREFIX" && -d "$FFMPEG_PREFIX/share/licenses" ]]; then
    mkdir -p "$LIC/bundled"
    cp -r "$FFMPEG_PREFIX/share/licenses/." "$LIC/bundled/"
fi
