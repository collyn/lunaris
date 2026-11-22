#!/usr/bin/env bash
# Lunaris agent installer for Linux (x86_64), made by the Lunaris server.
#
#   curl -fsSL https://SERVER/install/agent.sh | sudo LUNARIS_INSTALL_TOKEN='TOKEN' bash -s -- [--name NAME]
#   (or: ... | sudo bash -s -- --token TOKEN [--name NAME])
#
# Installs the Lunaris agent of this server's version from GitHub Releases
# (the .deb with apt, otherwise the AppImage in /opt/lunaris-agent) and joins
# it to the server with the install token (used once, then the agent signs in
# with its own key). Every downloaded file is checked before it is installed:
# its SHA-256 must be the one in the release's SHA256SUMS, whose Ed25519
# signature is checked against the Lunaris release key (by the server, which
# then writes the expected values into this script, or else here with
# OpenSSL 3); anything else stops the installation. The agent has no window;
# it runs as a service:
#   - desktop: the systemd user service lunaris-agent of the desktop user
#     (shares that user's screen), plus the lunaris-capture service (root:
#     unattended capture on Wayland without the portal prompt) and, with apt,
#     the approval helper (a tray that asks before anyone connects without
#     the password);
#   - headless (a VPS, a server; chosen when the user has no graphical
#     session and no login screen starts at boot, or with --headless): the
#     system service lunaris-agent-system (user lunaris-agent, data in
#     /var/lib/lunaris-agent; computers of other types reached through it,
#     direct access; no screen).
#
# Options:
#   --token TOKEN   install token from the server's web page (or the
#                   LUNARIS_INSTALL_TOKEN environment variable)
#   --name NAME     name shown for this computer (default: its host name)
#   --user USER     desktop user the agent runs for (default: the one who ran sudo)
#   --headless      the system service, without a desktop session
#   --desktop       the desktop user's service, even without a graphical session now
#   --no-helper     do not install the approval helper (desktop, apt)
#   --appimage      use the AppImage even where apt is available
set -euo pipefail

SERVER_URL='@@SERVER_URL@@'
VERSION='@@VERSION@@'
CERT_SHA256='@@CERT_SHA256@@'
# The Lunaris release signing key (Ed25519, the base64 of a PEM public key).
RELEASE_KEY='@@RELEASE_KEY@@'
# The SHA-256 of the release files, written in by the server after it
# checked the release's signed SHA256SUMS (empty: checked here instead).
PINNED_AGENT_DEB='@@SHA256_AGENT_DEB@@'
PINNED_HELPER_DEB='@@SHA256_HELPER_DEB@@'
PINNED_APPIMAGE='@@SHA256_APPIMAGE@@'
RELEASE_BASE="https://github.com/collyn/lunaris/releases/download/v${VERSION}"
INSTALL_DIR=/opt/lunaris-agent
SYSTEM_DATA=/var/lib/lunaris-agent

die() { echo "error: $*" >&2; exit 1; }
info() { echo "==> $*"; }

TOKEN="${LUNARIS_INSTALL_TOKEN:-}"
NAME=""
TARGET_USER="${SUDO_USER:-}"
FORCE_APPIMAGE=0
MODE=""
HELPER=1
while [ $# -gt 0 ]; do
    case "$1" in
        --token) [ $# -ge 2 ] || die "--token needs a value"; TOKEN="$2"; shift 2 ;;
        --token=*) TOKEN="${1#*=}"; shift ;;
        --name) [ $# -ge 2 ] || die "--name needs a value"; NAME="$2"; shift 2 ;;
        --name=*) NAME="${1#*=}"; shift ;;
        --user) [ $# -ge 2 ] || die "--user needs a value"; TARGET_USER="$2"; shift 2 ;;
        --user=*) TARGET_USER="${1#*=}"; shift ;;
        --headless) MODE=headless; shift ;;
        --desktop) MODE=desktop; shift ;;
        --no-helper) HELPER=0; shift ;;
        --appimage) FORCE_APPIMAGE=1; shift ;;
        -h|--help) sed -n '2,35p' "$0" 2>/dev/null || true; exit 0 ;;
        *) die "unknown option: $1" ;;
    esac
done

[ -n "$TOKEN" ] || die "missing install token: set LUNARIS_INSTALL_TOKEN or pass --token (copy the command from the server's web page)"
[[ "$TOKEN" =~ ^lit_[A-Za-z0-9_-]{16,128}$ ]] || die "this is not an install token (they start with lit_)"
[ "${#NAME}" -le 128 ] || die "--name is longer than 128 characters"
[ "$(uname -s)" = Linux ] || die "this installer is for Linux"
[ "$(uname -m)" = x86_64 ] || die "only x86_64 (amd64) computers are supported, this one is $(uname -m)"
[ "$(id -u)" -eq 0 ] || die "run as root: curl -fsSL <url>/install/agent.sh | sudo LUNARIS_INSTALL_TOKEN=<token> bash -s"
command -v curl >/dev/null || die "curl is needed"
command -v sha256sum >/dev/null || die "sha256sum is needed (coreutils)"
SYSTEMD=0
if [ -d /run/systemd/system ] && command -v systemctl >/dev/null; then SYSTEMD=1; fi

# Whether this computer has a desktop for the agent to share: the user has a
# graphical session now, or the computer starts a display manager (a login
# screen) at boot. sudo drops DISPLAY, so the sessions are asked; the boot
# target is no sign (many server and cloud images boot graphical.target).
has_desktop() {
    [ -n "${DISPLAY:-}${WAYLAND_DISPLAY:-}" ] && return 0
    if command -v loginctl >/dev/null; then
        local s t
        for s in $(loginctl list-sessions --no-legend 2>/dev/null | awk -v u="$TARGET_USER" '$3 == u { print $1 }'); do
            t="$(loginctl show-session "$s" -p Type --value 2>/dev/null || true)"
            case "$t" in x11|wayland|mir) return 0 ;; esac
        done
    fi
    [ -e /etc/systemd/system/display-manager.service ] && return 0
    return 1
}

if [ -z "$MODE" ]; then
    if [ -z "$TARGET_USER" ] || [ "$TARGET_USER" = root ]; then
        MODE=headless
    elif has_desktop; then
        MODE=desktop
    else
        MODE=headless
    fi
fi
if [ "$MODE" = desktop ]; then
    if [ -z "$TARGET_USER" ] || [ "$TARGET_USER" = root ]; then
        die "the desktop agent runs in a desktop user's session, not as root. Run this with sudo from that user's account, add --user <name>, or use --headless."
    fi
    TARGET_HOME="$(getent passwd "$TARGET_USER" | cut -d: -f6 || true)"
    [ -n "$TARGET_HOME" ] && [ -d "$TARGET_HOME" ] || die "user $TARGET_USER has no home folder"
    TARGET_UID="$(id -u "$TARGET_USER")"
    info "Desktop computer: the agent runs in $TARGET_USER's desktop session"
else
    [ "$SYSTEMD" -eq 1 ] || die "the headless agent runs as a systemd service, and systemd is not running here"
    info "Headless computer: the agent runs as the system service lunaris-agent-system"
fi

tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT
# apt reads the package as its own sandbox user.
chmod 755 "$tmp"

download() {
    info "Downloading $1"
    curl -fL --proto '=https' --tlsv1.2 --retry 3 --progress-bar -o "$2" "$RELEASE_BASE/$1" \
        || die "cannot download $RELEASE_BASE/$1 (is version $VERSION released?)"
}

# The release's SHA256SUMS, checked once: its signature with the release key
# (OpenSSL 3 can check Ed25519; without it only the HTTPS download from
# GitHub vouches for the list) and its first line, which names the version.
SUMS=""
load_sums() {
    [ -n "$SUMS" ] && return 0
    SUMS="$tmp/SHA256SUMS"
    curl -fsSL --proto '=https' --tlsv1.2 --retry 3 -o "$SUMS" "$RELEASE_BASE/SHA256SUMS" \
        || die "cannot download $RELEASE_BASE/SHA256SUMS: the files can't be checked, nothing is installed"
    if [ -n "$RELEASE_KEY" ] && command -v openssl >/dev/null && openssl pkeyutl -help 2>&1 | grep -q -- '-rawin'; then
        curl -fsSL --proto '=https' --tlsv1.2 --retry 3 -o "$tmp/SHA256SUMS.sig" "$RELEASE_BASE/SHA256SUMS.sig" \
            || die "cannot download $RELEASE_BASE/SHA256SUMS.sig: the files can't be checked, nothing is installed"
        printf -- '-----BEGIN PUBLIC KEY-----\n%s\n-----END PUBLIC KEY-----\n' "$RELEASE_KEY" > "$tmp/release-key.pem"
        openssl pkeyutl -verify -pubin -inkey "$tmp/release-key.pem" -rawin \
            -in "$SUMS" -sigfile "$tmp/SHA256SUMS.sig" >/dev/null 2>&1 \
            || die "the signature of SHA256SUMS of release $VERSION is not valid: nothing is installed"
        info "SHA256SUMS: signature checked"
    else
        echo "warning: OpenSSL 3 is not installed: the release's checksum list is trusted for coming from GitHub over HTTPS, its signature is not checked" >&2
    fi
    [ "$(head -n 1 "$SUMS")" = "# lunaris $VERSION" ] \
        || die "SHA256SUMS is not the list of release $VERSION: nothing is installed"
}

# Refuses file $2 (release file $1) unless its SHA-256 is the expected one:
# $3 (written in by the server), or else its line in SHA256SUMS.
check_file() {
    local want got
    if [ -n "$3" ]; then
        want="$3"
    else
        load_sums
        want="$(awk -v f="$1" '($2 == f || $2 == "*" f) && $1 ~ /^[0-9a-f]+$/ && length($1) == 64 { print $1; exit }' "$SUMS")"
    fi
    [ -n "$want" ] || die "$1 is not in the checksum list of release $VERSION: nothing is installed"
    got="$(sha256sum "$2" | cut -d' ' -f1)"
    [ "$got" = "$want" ] || die "$1 does not match its checksum (expected $want, got $got): nothing is installed"
    info "$1: checksum OK"
}

if [ "$FORCE_APPIMAGE" -eq 0 ] && command -v apt-get >/dev/null && command -v dpkg >/dev/null; then
    debs=("lunaris-agent_${VERSION}_amd64.deb")
    files=()
    for deb in "${debs[@]}"; do
        download "$deb" "$tmp/$deb"
        check_file "$deb" "$tmp/$deb" "$PINNED_AGENT_DEB"
        chmod 644 "$tmp/$deb"
        files+=("$tmp/$deb")
    done
    # The helper is optional: without it the agent still works (password only).
    if [ "$MODE" = desktop ] && [ "$HELPER" -eq 1 ]; then
        deb="lunaris-approval-helper_${VERSION}_amd64.deb"
        info "Downloading $deb"
        if curl -fL --proto '=https' --tlsv1.2 --retry 3 --progress-bar -o "$tmp/$deb" "$RELEASE_BASE/$deb"; then
            check_file "$deb" "$tmp/$deb" "$PINNED_HELPER_DEB"
            chmod 644 "$tmp/$deb"
            debs+=("$deb")
            files+=("$tmp/$deb")
        else
            echo "warning: $deb is not in release $VERSION: installing the agent without it (connections are let in by password only)" >&2
        fi
    fi
    info "Installing ${debs[*]}"
    # A headless computer gets none of the desktop's Recommends (portal, PipeWire).
    if [ "$MODE" = headless ]; then
        DEBIAN_FRONTEND=noninteractive apt-get install -y --no-install-recommends "${files[@]}"
    else
        DEBIAN_FRONTEND=noninteractive apt-get install -y "${files[@]}"
    fi
    AGENT_BIN=/usr/bin/lunaris-agent
else
    appimage="lunaris-agent-linux-x86_64.AppImage"
    download "$appimage" "$tmp/$appimage"
    check_file "$appimage" "$tmp/$appimage" "$PINNED_APPIMAGE"
    info "Installing $INSTALL_DIR/lunaris-agent.AppImage"
    install -d -m 755 "$INSTALL_DIR"
    install -m 755 "$tmp/$appimage" "$INSTALL_DIR/lunaris-agent.AppImage"
    ln -sfn "$INSTALL_DIR/lunaris-agent.AppImage" /usr/local/bin/lunaris-agent
    AGENT_BIN="$INSTALL_DIR/lunaris-agent.AppImage"
    # Remote keyboard/mouse through /dev/uinput (what the packages install,
    # scripts/linux-host/setup-host.sh).
    cat > /etc/udev/rules.d/60-lunaris-input.rules <<'RULES'
# Lunaris host agent: virtual keyboard/mouse through /dev/uinput.
KERNEL=="uinput", SUBSYSTEM=="misc", OPTIONS+="static_node=uinput", GROUP="input", MODE="0660", TAG+="uaccess"
RULES
    echo uinput > /etc/modules-load.d/60-lunaris.conf
    modprobe uinput 2>/dev/null || true
    if command -v udevadm >/dev/null; then
        udevadm control --reload-rules 2>/dev/null || true
        udevadm trigger --subsystem-match=misc --sysname-match=uinput 2>/dev/null || true
    fi
    # AppImages need FUSE 2 (the services unpack it instead).
    if [ "$MODE" = desktop ] && ! { ldconfig -p 2>/dev/null | grep -q 'libfuse\.so\.2'; }; then
        info "Installing FUSE 2 for the AppImage"
        if command -v dnf >/dev/null; then dnf install -y fuse-libs || true
        elif command -v zypper >/dev/null; then zypper --non-interactive install libfuse2 || true
        elif command -v pacman >/dev/null; then pacman -S --noconfirm --needed fuse2 || true
        fi
        if ! { ldconfig -p 2>/dev/null | grep -q 'libfuse\.so\.2'; }; then
            echo "warning: libfuse.so.2 not found: install FUSE 2 (fuse2 / libfuse2 / fuse-libs), or the agent can't start at login" >&2
            export APPIMAGE_EXTRACT_AND_RUN=1
        fi
    fi
    if [ "$MODE" = headless ]; then
        # The system service and its account, as the package has them
        # (scripts/packaging/linux/lunaris-agent-system.service).
        if ! getent passwd lunaris-agent >/dev/null; then
            useradd --system --home-dir "$SYSTEM_DATA" --no-create-home --shell /usr/sbin/nologin \
                --user-group lunaris-agent
        fi
        for g in video render input; do
            if getent group "$g" >/dev/null; then usermod -aG "$g" lunaris-agent || true; fi
        done
        cat > /etc/systemd/system/lunaris-agent-system.service <<UNIT
# Lunaris agent as a system service (headless). Made by the Lunaris installer.
[Unit]
Description=Lunaris agent (headless system service)
After=network-online.target
Wants=network-online.target
ConditionPathExists=$SYSTEM_DATA/agent_config.json

[Service]
Type=simple
# Unpacked to the private /tmp instead of a FUSE mount.
Environment=APPIMAGE_EXTRACT_AND_RUN=1
ExecStart=$AGENT_BIN --cli --config $SYSTEM_DATA/agent_config.json
User=lunaris-agent
Group=lunaris-agent
WorkingDirectory=$SYSTEM_DATA
StateDirectory=lunaris-agent
StateDirectoryMode=0700
# always: back after a stray kill; `systemctl stop` still stops it.
Restart=always
RestartSec=5
CapabilityBoundingSet=
NoNewPrivileges=yes
ProtectSystem=strict
ProtectHome=yes
PrivateTmp=yes
DevicePolicy=closed
DeviceAllow=char-drm rw
DeviceAllow=/dev/uinput rw
ProtectKernelTunables=yes
ProtectKernelModules=yes
ProtectControlGroups=yes
RestrictAddressFamilies=AF_INET AF_INET6 AF_UNIX AF_NETLINK
RestrictNamespaces=yes
LockPersonality=yes
SystemCallArchitectures=native
UMask=0077

[Install]
WantedBy=multi-user.target
UNIT
        chmod 644 /etc/systemd/system/lunaris-agent-system.service
        systemctl daemon-reload || true
    fi
fi

# Unattended capture on Wayland (desktop): the root capture service hands
# the screen to the agent of the active desktop session (no portal prompt).
# The .deb installs and enables it itself; the AppImage gets the same unit
# here. A headless computer has no screen to capture: it is turned off.
CAPTURE_SERVICE=0
if [ "$SYSTEMD" -eq 1 ] && [ "$MODE" = headless ]; then
    systemctl disable --now lunaris-capture.service >/dev/null 2>&1 || true
elif [ "$SYSTEMD" -eq 1 ]; then
    if [ "$AGENT_BIN" != /usr/bin/lunaris-agent ]; then
        info "Installing the capture service (lunaris-capture.service)"
        cat > /etc/systemd/system/lunaris-capture.service <<UNIT
# Lunaris capture service: lets the Lunaris agent in the active desktop
# session capture the screen without the Wayland portal prompt. Reads the
# screen only; no network, no input. Made by the Lunaris installer.
[Unit]
Description=Lunaris capture service (unattended screen capture)
After=systemd-logind.service

[Service]
Type=simple
# Unpacked to the private /tmp instead of a FUSE mount.
Environment=APPIMAGE_EXTRACT_AND_RUN=1
ExecStart=$AGENT_BIN --capture-service --allow-appimage $AGENT_BIN
# always: back after a stray kill; `systemctl stop` still stops it.
Restart=always
RestartSec=2
RuntimeDirectory=lunaris
RuntimeDirectoryMode=0755
CapabilityBoundingSet=CAP_SYS_ADMIN CAP_SYS_PTRACE CAP_DAC_READ_SEARCH
NoNewPrivileges=yes
DevicePolicy=closed
DeviceAllow=char-drm rw
PrivateNetwork=yes
RestrictAddressFamilies=AF_UNIX
IPAddressDeny=any
ProtectSystem=strict
ProtectHome=read-only
PrivateTmp=yes
ProtectKernelTunables=yes
ProtectKernelModules=yes
ProtectControlGroups=yes
RestrictNamespaces=yes
LockPersonality=yes
SystemCallArchitectures=native
UMask=0077

[Install]
WantedBy=multi-user.target
UNIT
        chmod 644 /etc/systemd/system/lunaris-capture.service
        systemctl daemon-reload || true
    fi
    if systemctl enable --now lunaris-capture.service >/dev/null 2>&1; then
        CAPTURE_SERVICE=1
    else
        echo "warning: the capture service did not start (systemctl status lunaris-capture); Wayland capture will use the portal prompt" >&2
    fi
fi

args=(--enroll --server "$SERVER_URL")
[ -n "$NAME" ] && args+=(--name "$NAME")
[ -n "$CERT_SHA256" ] && args+=(--server-cert-sha256 "$CERT_SHA256")

info "Joining this computer to $SERVER_URL"
# The token goes in the environment, not on the agent's command line.
export LUNARIS_INSTALL_TOKEN="$TOKEN"
if [ "$MODE" = headless ]; then
    # Writes /var/lib/lunaris-agent/agent_config.json (owned by lunaris-agent)
    # and enables and starts lunaris-agent-system.service.
    (cd / && "$AGENT_BIN" --system "${args[@]}") || die "the agent could not be set up (see above)"
else
    # As the desktop user, in that user's environment: writes the config and
    # enables the systemd user service (and the login autostart entry).
    config="$TARGET_HOME/.config/lunaris-agent/agent_config.json"
    (cd "$TARGET_HOME" && runuser -u "$TARGET_USER" -- env -u XDG_CONFIG_HOME \
        HOME="$TARGET_HOME" USER="$TARGET_USER" XDG_RUNTIME_DIR="/run/user/$TARGET_UID" \
        LUNARIS_INSTALL_TOKEN="$TOKEN" "$AGENT_BIN" "${args[@]}" --config "$config") \
        || die "the agent could not be set up (see above)"
fi
unset LUNARIS_INSTALL_TOKEN

if [ "$MODE" = headless ]; then
    cat <<EOF

Lunaris Agent $VERSION is installed and joined to $SERVER_URL as a headless
computer.

  * It runs as the system service lunaris-agent-system (user lunaris-agent,
    data in $SYSTEM_DATA) and starts at boot:
      systemctl status lunaris-agent-system; journalctl -u lunaris-agent-system
  * The computer appears on the server within a few seconds; the install
    token is used then, after that the agent signs in with its own key.
  * State and settings: sudo lunaris-agent --system status
    (changes: on the server's web page, or sudo lunaris-agent --system set ...)
EOF
    exit 0
fi

if [ "$CAPTURE_SERVICE" -eq 1 ]; then
    WAYLAND_NOTE="  * Wayland (GNOME, KDE, ...): the screen is captured through the
    lunaris-capture service, without asking on this screen (only for the
    user of the active desktop session; see systemctl status lunaris-capture)."
else
    WAYLAND_NOTE="  * Wayland (GNOME, KDE, ...): the first remote session asks on this screen
    which monitor to share. Someone has to accept it once; the choice is
    remembered (~/.config/lunaris/screencast-token)."
fi

cat <<EOF

Lunaris Agent $VERSION is installed for $TARGET_USER and joined to $SERVER_URL.

  * It runs in $TARGET_USER's desktop session as the systemd user service
    lunaris-agent (started with the desktop; see above whether it runs
    already). In that session: systemctl --user status lunaris-agent,
    journalctl --user -u lunaris-agent, lunaris-agent status.
  * The computer appears on the server once the agent runs; the install
    token is used then, after that the agent signs in with its own key.
$WAYLAND_NOTE
  * Remote keyboard/mouse works after the next login (/dev/uinput).
EOF
