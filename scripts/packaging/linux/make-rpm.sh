#!/usr/bin/env bash
# Builds lunaris-agent and lunaris-client RPMs (Fedora) from prebuilt binaries.
#
# Usage: make-rpm.sh <cargo target dir> <output dir> <static ffmpeg prefix>
set -euo pipefail

TARGET="$1"; OUT="$(mkdir -p "$2" && cd "$2" && pwd)"; FFMPEG="$3"
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$HERE/../../.." && pwd)"
VERSION="$(sed -n '/^\[workspace.package\]/,/^\[/s/^version *= *"\(.*\)"/\1/p' "$ROOT/Cargo.toml" | head -1)"

WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT
"$HERE/stage.sh" agent "$TARGET/release/agent" "$WORK/agent" "$FFMPEG"
"$HERE/stage.sh" client "$TARGET/release/client-desktop" "$WORK/client" "$FFMPEG"
strip --strip-unneeded "$WORK/agent/usr/bin/lunaris-agent" "$WORK/client/usr/bin/lunaris-client"

rpmbuild -bb "$HERE/lunaris.spec" \
    --define "_topdir $WORK/rpmbuild" \
    --define "lunaris_version $VERSION" \
    --define "stage_agent $WORK/agent" \
    --define "stage_client $WORK/client"

find "$WORK/rpmbuild/RPMS" -name '*.rpm' -exec cp {} "$OUT/" \;
for f in "$OUT"/lunaris-*.rpm; do
    echo "Built $f"
    rpm -qp --requires "$f" | grep -v '^rpmlib(' | tr '\n' ' '
    echo
done
