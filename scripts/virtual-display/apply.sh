#!/usr/bin/env bash
# Apply the forced high-refresh virtual display config for DP-2.
#
# This copies the Xorg drop-in into /etc/X11/xorg.conf.d/ (needs sudo) and then
# tells you how to restart X. It does NOT restart X for you — restarting X kills
# every graphical app (including any running Lunaris agent and your desktop).
#
# READ scripts/virtual-display/README.md FIRST — including the recovery steps.
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
SRC="$HERE/20-lunaris-virtual-dp2.conf"
DST="/etc/X11/xorg.conf.d/20-lunaris-virtual-dp2.conf"

if [[ ! -f "$SRC" ]]; then
  echo "ERROR: $SRC not found" >&2
  exit 1
fi

echo "This will install:"
echo "  $SRC"
echo "    -> $DST"
echo
echo "You will then need to restart X (log out / restart the display manager)."
read -r -p "Continue? [y/N] " ans
[[ "${ans,,}" == "y" ]] || { echo "Aborted."; exit 0; }

if [[ -f "$DST" ]]; then
  sudo cp -a "$DST" "${DST}.bak.$(date +%s)"
  echo "Backed up existing $DST"
fi

sudo install -m 0644 "$SRC" "$DST"
echo "Installed $DST"
echo
echo "Next steps:"
echo "  1. Restart X:   sudo systemctl restart display-manager    (or log out/in)"
echo "  2. Verify:      xrandr | grep -A6 'DP-2'"
echo "     You should now see DP-2 'connected' with 1920x1080 144.00 / 240.00 modes."
echo "  3. Activate:    xrandr --output DP-2 --mode 1920x1080_240.00 --right-of DP-1"
echo "  4. In the Lunaris client: pick display DP-2 and the FPS you want."
echo
echo "If X fails to start after the restart, see the RECOVERY section in README.md"
echo "(TTY: Ctrl+Alt+F3 -> 'sudo $HERE/revert.sh' -> restart display-manager)."
