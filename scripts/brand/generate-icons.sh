#!/usr/bin/env bash
# Regenerates every Lunaris icon from the master artwork assets/brand/lunaris.svg
# (web favicon, agent and client PNG/ICO/ICNS, QML window icons).
# Needs: rsvg-convert (librsvg) and ImageMagick 7 (magick).
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
SVG="$ROOT/assets/brand/lunaris.svg"
WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT

png() { rsvg-convert -w "$1" -h "$1" "$SVG" -o "$2"; }

for size in 16 24 32 48 64 128 256 512 1024; do
    png "$size" "$WORK/$size.png"
done

for dir in agent/icons client-desktop/icons; do
    cp "$WORK/32.png" "$ROOT/$dir/32x32.png"
    cp "$WORK/128.png" "$ROOT/$dir/128x128.png"
    cp "$WORK/256.png" "$ROOT/$dir/128x128@2x.png"
    magick "$WORK"/{16,24,32,48,64,128,256}.png "$ROOT/$dir/icon.ico"
    # ImageMagick cannot write ICNS: build it from PNG payloads (macOS 10.7+).
    python3 - "$WORK" "$ROOT/$dir/icon.icns" <<'PY'
import struct, sys
work, out = sys.argv[1], sys.argv[2]
types = [("icp4", 16), ("icp5", 32), ("icp6", 64), ("ic07", 128), ("ic08", 256),
         ("ic09", 512), ("ic10", 1024), ("ic11", 32), ("ic12", 64), ("ic13", 256), ("ic14", 512)]
body = b""
for code, size in types:
    data = open(f"{work}/{size}.png", "rb").read()
    body += code.encode() + struct.pack(">I", len(data) + 8) + data
open(out, "wb").write(b"icns" + struct.pack(">I", len(body) + 8) + body)
PY
done

cp "$WORK/256.png" "$ROOT/agent/qml/icons/icon.png"
cp "$WORK/256.png" "$ROOT/client-qml/qml/icon.png"
cp "$SVG" "$ROOT/web/public/favicon.svg"
echo "Icons regenerated from $SVG"
