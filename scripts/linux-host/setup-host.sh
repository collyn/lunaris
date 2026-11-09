#!/usr/bin/env bash
# Prepare a Linux machine to run the Lunaris host agent.
#
#   sudo ./setup-host.sh                 install udev rule + load uinput
#   sudo ./setup-host.sh --kms <agent>   also allow KMS capture (cap_sys_admin)
#   sudo ./setup-host.sh --uninstall     remove everything again
#
# Mirrors what Sunshine's packages install: input works on X11 and every
# Wayland compositor through /dev/uinput; KMS capture is optional (PipeWire
# portal capture needs no privileges).
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
RULES_DST=/etc/udev/rules.d/60-lunaris-input.rules
MODULES_DST=/etc/modules-load.d/60-lunaris.conf

die() { echo "error: $*" >&2; exit 1; }
[[ $EUID -eq 0 ]] || die "run as root (sudo $0 $*)"

reload_udev() {
    udevadm control --reload-rules
    udevadm trigger --subsystem-match=misc --sysname-match=uinput || true
}

case "${1:-}" in
    --uninstall)
        rm -f "$RULES_DST" "$MODULES_DST"
        reload_udev
        echo "Removed Lunaris udev rule and module autoload."
        echo "Note: capabilities set with --kms stay on the agent binary; clear them with:"
        echo "  sudo setcap -r /path/to/agent"
        exit 0
        ;;
    --kms)
        AGENT_BIN="${2:-}"
        [[ -n "$AGENT_BIN" && -x "$AGENT_BIN" ]] || die "--kms needs the path of the agent binary"
        ;;
    "") ;;
    *) die "unknown option: $1 (see header of $0)" ;;
esac

install -Dm644 "$HERE/60-lunaris-input.rules" "$RULES_DST"
install -Dm644 "$HERE/60-lunaris.conf" "$MODULES_DST"
modprobe uinput
reload_udev
echo "Installed $RULES_DST and $MODULES_DST; uinput loaded."

if [[ -n "${AGENT_BIN:-}" ]]; then
    command -v setcap >/dev/null || die "setcap not found (install libcap2-bin / libcap)"
    # Same capability Sunshine uses for kmsgrab. Enable at runtime with
    # LUNARIS_CAPTURE=kms; without it the agent uses the PipeWire portal.
    setcap cap_sys_admin+p "$AGENT_BIN"
    echo "Granted cap_sys_admin to $AGENT_BIN (KMS capture: run the agent with LUNARIS_CAPTURE=kms)."
fi

ls -l /dev/uinput
if command -v getfacl >/dev/null && [[ -n "${SUDO_USER:-}" ]]; then
    if getfacl -p /dev/uinput 2>/dev/null | grep -q "user:${SUDO_USER}:rw"; then
        echo "OK: ${SUDO_USER} can write /dev/uinput."
    else
        echo "Note: ACL for ${SUDO_USER} appears after the next login on the local seat;"
        echo "      or add the user to the 'input' group: sudo usermod -aG input ${SUDO_USER}"
    fi
fi
