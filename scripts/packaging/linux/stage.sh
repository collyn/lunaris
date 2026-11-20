#!/usr/bin/env bash
# Lays out the files of one Linux package under a staging root (shared by the
# .deb, .rpm and Arch builds).
#
# Usage: stage.sh agent|client|approval-helper <built binary> <destdir> [<ffmpeg prefix with share/licenses>]
#   agent            lunaris-agent: the command-line agent and its services (no GUI)
#   client           lunaris-client: the stream window
#   approval-helper  lunaris-approval-helper: the optional tray that accepts connections
set -euo pipefail

KIND="$1"; BIN="$2"; DEST="$3"; FFMPEG_PREFIX="${4:-}"
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$HERE/../../.." && pwd)"
NAME="lunaris-$KIND"

install -Dm755 "$BIN" "$DEST/usr/bin/$NAME"
case "$KIND" in
    agent)
        # No window, no launcher: a daemon. The systemd user service for a
        # desktop session, the system service for headless computers and its
        # account (see the units); --enroll enables the right one.
        install -Dm644 "$HERE/lunaris-agent.service" "$DEST/usr/lib/systemd/user/lunaris-agent.service"
        install -Dm644 "$HERE/lunaris-agent-system.service" "$DEST/usr/lib/systemd/system/lunaris-agent-system.service"
        install -Dm644 "$HERE/lunaris-agent.sysusers" "$DEST/usr/lib/sysusers.d/lunaris-agent.conf"
        # Remote keyboard/mouse via /dev/uinput (same files as scripts/linux-host).
        install -Dm644 "$ROOT/scripts/linux-host/60-lunaris-input.rules" "$DEST/usr/lib/udev/rules.d/60-lunaris-input.rules"
        install -Dm644 "$ROOT/scripts/linux-host/60-lunaris.conf" "$DEST/usr/lib/modules-load.d/lunaris.conf"
        # Unattended screen capture on Wayland (no portal prompt): the root
        # capture service, see docs/linux-capture-service.md.
        install -Dm644 "$ROOT/scripts/linux-host/lunaris-capture.service" "$DEST/usr/lib/systemd/system/lunaris-capture.service"
        ;;
    client)
        # The stream window only: no uinput access (that is the agent's).
        install -Dm644 "$HERE/$NAME.desktop" "$DEST/usr/share/applications/$NAME.desktop"
        install -Dm644 "$ROOT/client-qml/qml/icon.png" "$DEST/usr/share/icons/hicolor/256x256/apps/$NAME.png"
        ;;
    approval-helper)
        # Started at every desktop login; it waits quietly while no agent
        # runs for that user (approval-channel/).
        install -Dm644 "$ROOT/approval-helper/lunaris-approval-helper.desktop" "$DEST/etc/xdg/autostart/$NAME.desktop"
        install -Dm644 "$ROOT/approval-helper/qml/icons/icon.png" "$DEST/usr/share/icons/hicolor/256x256/apps/$NAME.png"
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
