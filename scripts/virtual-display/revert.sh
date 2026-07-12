#!/usr/bin/env bash
# Remove the forced high-refresh virtual display for DP-2 and return to the stock
# auto-detected display setup.
set -euo pipefail

CONF_DST="/etc/X11/xorg.conf.d/20-lunaris-virtual-dp2.conf"
EDID_DST="/etc/X11/lunaris-dp2-edid.bin"

for f in "$CONF_DST" "$EDID_DST"; do
  if [[ -f "$f" ]]; then
    sudo rm -f "$f"
    echo "Removed $f"
  fi
done

echo "Now restart X to return to the default configuration:"
echo "  sudo systemctl restart display-manager    (or log out/in)"
