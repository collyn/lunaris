#!/usr/bin/env bash
# Remove the forced high-refresh virtual display config for DP-2 and return to
# the stock auto-detected display setup.
set -euo pipefail

DST="/etc/X11/xorg.conf.d/20-lunaris-virtual-dp2.conf"

if [[ -f "$DST" ]]; then
  sudo rm -f "$DST"
  echo "Removed $DST"
else
  echo "$DST not present — nothing to remove."
fi

echo "Now restart X to return to the default configuration:"
echo "  sudo systemctl restart display-manager    (or log out/in)"
