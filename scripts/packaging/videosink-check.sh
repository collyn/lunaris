#!/usr/bin/env bash
# Builds and runs videosink-check.cpp against the libraries, plugins and QML modules
# of a deployed client AppDir, with the build machine's Qt hidden (only the headers
# and the compiler come from it). Needs g++, Qt headers (QTDIR) and xvfb-run (or a display).
# usage: videosink-check.sh <AppDir>
set -euo pipefail
APPDIR="$(readlink -f "$1")"
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
T="$(mktemp -d)"; trap 'rm -rf "$T"' EXIT
Q="${QTDIR:?QTDIR is not set}"

printf 'import QtQuick\nimport QtMultimedia\nItem { VideoOutput { objectName: "output"; anchors.fill: parent } }\n' > "$T/videosink-check.qml"
inc=(); for m in Core Gui Quick Qml Multimedia QmlIntegration Network; do inc+=(-I"$Q/include/Qt$m"); done
g++ -std=c++17 -fPIC "$HERE/videosink-check.cpp" -o "$T/videosink-check" -I"$Q/include" "${inc[@]}" \
    -L"$Q/lib" -lQt6Multimedia -lQt6Quick -lQt6Qml -lQt6Gui -lQt6Core
printf '[Paths]\nPlugins=%s/usr/plugins\n' "$APPDIR" > "$T/qt.conf"

ENVV=(PATH=/usr/bin:/bin HOME="$T" LANG=C.UTF-8 LD_LIBRARY_PATH="$APPDIR/usr/lib" QML2_IMPORT_PATH="$APPDIR/usr/qml"
      QT_QPA_PLATFORM=xcb QSG_RENDER_LOOP=basic QT_FORCE_STDERR_LOGGING=1 XDG_CACHE_HOME="$T/.cache" MESA_SHADER_CACHE_DISABLE=true)
# With no display, start a virtual one; DISPLAY / XAUTHORITY pass through env -i.
LAUNCH=(); [[ -z "${DISPLAY:-}" ]] && LAUNCH=(xvfb-run -a)
set +e
out=$("${LAUNCH[@]}" sh -c 'exec env -i ${DISPLAY:+DISPLAY=$DISPLAY} ${XAUTHORITY:+XAUTHORITY=$XAUTHORITY} "$@"' sh \
      "${ENVV[@]}" "$T/videosink-check" 2>&1); rc=$?
set -e
echo "$out" | grep -av '^Detected locale\|^Qt depends\|^If this\|^for more'
if [[ $rc -ne 0 ]] || grep -aq 'Failed to create QVideoSink\|No QtMultimedia backends' <<<"$out"; then
    echo "videosink-check: FAIL (exit $rc)" >&2; exit 1
fi
echo "videosink-check: OK"
