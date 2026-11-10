#!/usr/bin/env bash
# Copies assets/cursors/*.svg to the web client and renders them as 64 px PNGs
# (2x of the 32 px cursor, sharp on HiDPI) for the Qt client, which then needs
# no SVG image plugin. Needs rsvg-convert (librsvg2-bin).
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
SRC="$ROOT/assets/cursors"
WEB="$ROOT/web/public/cursors"
QT="$ROOT/client-qml/qml/cursors"
mkdir -p "$WEB" "$QT"
for svg in "$SRC"/*.svg; do
    name="$(basename "$svg" .svg)"
    cp "$svg" "$WEB/$name.svg"
    rsvg-convert -w 64 -h 64 "$svg" -o "$QT/$name.png"
done
echo "Cursors written to $WEB and $QT"
