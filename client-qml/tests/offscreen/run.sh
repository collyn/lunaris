#!/bin/sh
# Renders the app's screens off screen (no window is opened) and checks what
# they do: qmltestrunner with a stub bridge. The pictures
# go to out/ beside this file, or to the directory given.
#   client-qml/tests/offscreen/run.sh [directory for the pictures]
set -e
here="$(cd "$(dirname "$0")" && pwd)"
runner="${QMLTESTRUNNER:-$(command -v qmltestrunner || echo /usr/lib/qt6/bin/qmltestrunner)}"
mkdir -p "$here/out"
# Off screen in every way: no window system, and no session bus.
export QT_QPA_PLATFORM=offscreen QT_QUICK_BACKEND=software QT_QUICK_CONTROLS_STYLE=Basic
export DBUS_SESSION_BUS_ADDRESS=unix:path=/nonexistent
"$runner" -import "$here/stubs" -input "$here/tst_launcher.qml"
"$runner" -import "$here/stubs" -input "$here/tst_session_settings.qml"
"$runner" -import "$here/stubs" -input "$here/tst_pointer_in_picture.qml"
"$runner" -import "$here/stubs" -input "$here/tst_keyboard.qml"
"$runner" -import "$here/stubs" -input "$here/tst_stats_overlay.qml"
"$runner" -input "$here/tst_updates.qml"
if [ -n "$1" ]; then
    mkdir -p "$1"
    mv "$here"/out/*.png "$1"/
fi
