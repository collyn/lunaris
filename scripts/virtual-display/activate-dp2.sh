#!/usr/bin/env bash
# Activate the virtual DP-2 display at a target refresh rate, keeping DP-1 as the
# primary desktop. Run this after X has restarted with the CustomEDID config.
#
# Usage: activate-dp2.sh [target_hz]   (default 144)
set -euo pipefail

TARGET="${1:-144}"

if ! xrandr --query | grep -q '^DP-2 connected'; then
  echo "DP-2 is not connected. Did you apply the config and restart X?" >&2
  echo "Run: scripts/virtual-display/apply.sh" >&2
  exit 1
fi

# Pick the 1920x1080 refresh rate DP-2 advertises that is closest to TARGET.
best_rate="$(xrandr --query | awk '
  /^DP-2 connected/ {inblk=1; next}
  /^[^ ]/ {inblk=0}
  inblk && $1=="1920x1080" {
    for (i=2;i<=NF;i++){ r=$i; gsub(/[*+]/,"",r); print r }
  }' | sort -u | awk -v t="$TARGET" '
    { d=($1>t)?$1-t:t-$1; if (best=="" || d<bd){best=$1; bd=d} }
    END{ if (best!="") print best }')"

if [[ -z "$best_rate" ]]; then
  echo "DP-2 has no 1920x1080 mode. Check the EDID/config." >&2
  xrandr --query | sed -n '/^DP-2 connected/,/^[^ ]/p'
  exit 1
fi

echo "Setting DP-2 to 1920x1080 @ ${best_rate}Hz (requested ${TARGET})..."
xrandr --output DP-1 --primary \
       --output DP-2 --mode 1920x1080 --rate "$best_rate" --right-of DP-1

echo "Done. Current layout:"
xrandr --query | grep -E '^(DP-1|DP-2|HDMI-0) '
echo
echo "Now select display DP-2 in the Lunaris client and run the content you want"
echo "to stream on the DP-2 screen area."
