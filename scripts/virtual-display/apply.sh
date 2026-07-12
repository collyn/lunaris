#!/usr/bin/env bash
# Apply the forced high-refresh virtual display for DP-2 (CustomEDID approach).
#
# Installs the EDID + Xorg drop-in (needs sudo), then tells you how to restart X.
# It does NOT restart X for you — that kills every graphical app (your desktop
# and any running Lunaris agent).
#
# READ scripts/virtual-display/README.md FIRST, especially the RECOVERY section.
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
CONF_SRC="$HERE/20-lunaris-virtual-dp2.conf"
EDID_SRC="$HERE/dp2-1080p-highrefresh.bin"
CONF_DST="/etc/X11/xorg.conf.d/20-lunaris-virtual-dp2.conf"
EDID_DST="/etc/X11/lunaris-dp2-edid.bin"

[[ -f "$CONF_SRC" ]] || { echo "ERROR: $CONF_SRC not found" >&2; exit 1; }
if [[ ! -f "$EDID_SRC" ]]; then
  echo "EDID not found — generating it..."
  ( cd "$HERE" && python3 gen_edid.py "$EDID_SRC" )
fi

echo "About to install:"
echo "  $EDID_SRC  -> $EDID_DST"
echo "  $CONF_SRC  -> $CONF_DST"
echo
echo "The .conf lists ConnectedMonitor \"DP-1, HDMI-0, DP-2\". Confirm those match"
echo "your real connected outputs (xrandr --query) or the wrong ones go dark."
read -r -p "Continue? [y/N] " ans
[[ "${ans,,}" == "y" ]] || { echo "Aborted."; exit 0; }

[[ -f "$CONF_DST" ]] && { sudo cp -a "$CONF_DST" "${CONF_DST}.bak.$(date +%s)"; echo "Backed up $CONF_DST"; }

sudo install -m 0644 "$EDID_SRC" "$EDID_DST"
sudo install -m 0644 "$CONF_SRC" "$CONF_DST"
echo "Installed EDID + config."
echo
echo "Next steps:"
echo "  1. Restart X:   sudo systemctl restart display-manager    (or log out/in)"
echo "  2. Verify:      xrandr | grep -A8 'DP-2'"
echo "     DP-2 should be 'connected' with 1920x1080 143.88 / 119.93 / 59.96 modes,"
echo "     and DP-1/HDMI-0 should still be connected."
echo "  3. Position it: $HERE/activate-dp2.sh 144   (keeps DP-1 primary)"
echo "  4. In the Lunaris client: pick display DP-2 and the FPS you want, then run"
echo "     the content you want to stream ON the DP-2 screen area."
echo
echo "If X fails to start: TTY (Ctrl+Alt+F3) -> 'sudo $HERE/revert.sh' -> restart X."
