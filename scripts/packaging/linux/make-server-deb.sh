#!/usr/bin/env bash
# Builds lunaris-server_<ver>_<arch>.deb from the static (musl) server binary of
# the "Build Linux Server (static)" job: no shared-library dependencies, so one
# package per architecture serves every Debian/Ubuntu release with systemd.
#
# The package installs
#   /usr/bin/lunaris-server
#   /usr/lib/systemd/system/lunaris-server.service   (lunaris-server.service)
#   /usr/lib/sysusers.d/lunaris-server.conf          (the "lunaris" system user)
# and on first install creates /etc/lunaris/server.env and enables and starts
# the service. Upgrades restart it when it is enabled (a disabled one stays
# off); remove stops it and keeps the settings and data; purge deletes
# /etc/lunaris/server.env and /var/lib/lunaris.
#
# Usage: make-server-deb.sh <static lunaris-server binary> <amd64|arm64> <output dir>
set -euo pipefail

BIN="$1"; ARCH="$2"; OUT="$(mkdir -p "$3" && cd "$3" && pwd)"
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$HERE/../../.." && pwd)"
VERSION="$(sed -n '/^\[workspace.package\]/,/^\[/s/^version *= *"\(.*\)"/\1/p' "$ROOT/Cargo.toml" | head -1)"
NAME=lunaris-server

case "$ARCH" in amd64|arm64) ;; *) echo "unknown architecture: $ARCH (amd64|arm64)" >&2; exit 1 ;; esac
if [[ "$(file -b "$BIN")" != *"statically linked"* && "$(file -b "$BIN")" != *"static-pie linked"* ]]; then
    echo "error: $BIN is not a static binary; the package declares no library dependencies" >&2
    exit 1
fi

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
root="$work/$NAME"

install -Dm755 "$BIN" "$root/usr/bin/$NAME"
install -Dm644 "$HERE/lunaris-server.service" "$root/usr/lib/systemd/system/lunaris-server.service"
install -Dm644 "$HERE/lunaris-server.sysusers" "$root/usr/lib/sysusers.d/lunaris-server.conf"
install -Dm644 "$ROOT/LICENSE" "$root/usr/share/doc/$NAME/copyright"

mkdir -p "$root/DEBIAN"
cat > "$root/DEBIAN/control" <<EOF
Package: $NAME
Version: $VERSION
Architecture: $ARCH
Maintainer: Lunaris <noreply@lunaris.invalid>
Section: net
Priority: optional
Homepage: https://github.com/collyn/lunaris
Installed-Size: $(du -sk "$root" | cut -f1)
Depends: passwd
Recommends: systemd, lunaris-agent
Description: Lunaris remote desktop server
 Accounts, host directory, signaling, TURN relay and the web console for
 Lunaris. Runs as the lunaris-server systemd service under its own system
 user; everything is managed from the web console (http://<server>:8090).
 Settings in /etc/lunaris/server.env, data in /var/lib/lunaris.
 .
 With lunaris-agent installed, the server runs it as its gateway (sources
 only, never this machine's screen) so RustDesk, Sunshine, RDP and VNC
 computers can be added and reached through the server itself.
EOF

# ── postinst ──
cat > "$root/DEBIAN/postinst" <<'EOF'
#!/bin/sh
set -e

ENV_FILE=/etc/lunaris/server.env
DATA=/var/lib/lunaris
OLD_UNIT=/etc/systemd/system/lunaris-server.service

[ "$1" = configure ] || exit 0

# The service account.
if ! getent passwd lunaris >/dev/null; then
    if command -v systemd-sysusers >/dev/null 2>&1; then
        systemd-sysusers /usr/lib/sysusers.d/lunaris-server.conf
    else
        useradd --system --user-group --home-dir "$DATA" --no-create-home \
            --shell /usr/sbin/nologin --comment "Lunaris server" lunaris
    fi
fi

# `lunaris-server install` (from the tar.gz) wrote its own unit, which would
# hide this package's one (/etc wins over /usr/lib), and ran with DynamicUser=,
# keeping the data in /var/lib/private/lunaris. Take both over.
if [ -f "$OLD_UNIT" ] && grep -q '^ExecStart=/usr/local/bin/lunaris-server ' "$OLD_UNIT"; then
    if [ -d /run/systemd/system ]; then
        systemctl stop lunaris-server.service >/dev/null 2>&1 || true
    fi
    mv "$OLD_UNIT" "$OLD_UNIT.pre-package"
    echo "lunaris-server: replaced the unit written by \`lunaris-server install\` (kept as $OLD_UNIT.pre-package)."
    if [ -e /usr/local/bin/lunaris-server ]; then
        echo "lunaris-server: /usr/local/bin/lunaris-server (the old copy) comes first in PATH; remove it."
    fi
fi
if [ -L "$DATA" ] && [ -d /var/lib/private/lunaris ]; then
    rm "$DATA"
    mv /var/lib/private/lunaris "$DATA"
fi
if [ -d "$DATA" ]; then
    chown -R lunaris:lunaris "$DATA"
    chmod 700 "$DATA"
fi

# A server run by hand on this computer (data in a user's folder): the
# service takes it over while it has no account yet; otherwise the command
# to do it is printed. Nothing is replaced. (server/src/takeover.rs)
if [ -d /run/systemd/system ]; then
    /usr/bin/lunaris-server import --auto || echo "lunaris-server: could not take over a server run by hand (see above)."
fi

# Settings: not a conffile (`lunaris-server install` may have written it
# already), created once and then the admin's.
if [ ! -e "$ENV_FILE" ]; then
    mkdir -p /etc/lunaris
    umask 077
    cat > "$ENV_FILE" <<'ENV'
# Lunaris server settings (KEY=value). Restart the service after editing:
#   systemctl restart lunaris-server
# All options: lunaris-server help
SERVER_PORT=8090
# HTTPS port for the web console (self-signed certificate unless
# LUNARIS_TLS_CERT and LUNARIS_TLS_KEY are set; "off" disables it):
# LUNARIS_HTTPS_PORT=8443
# This machine's LAN or public address, for the built-in TURN relay:
# LUNARIS_TURN_PUBLIC_IP=
# Token agents use to register (generated on first start when unset):
# LUNARIS_TOKEN=
# Log level:
# RUST_LOG=info
ENV
fi

# Whether this start creates the database (and the first-start setup code).
had_db=no
[ -e "$DATA/lunaris.db" ] && had_db=yes

# First install: enabled (also offline, e.g. in an image build). Afterwards
# the admin's choice is kept: an enabled service is (re)started with the new
# binary, a disabled one stays off. `remove` only stops it, so a later
# reinstall finds it still enabled.
if command -v systemctl >/dev/null 2>&1; then
    if [ -z "$2" ]; then
        systemctl enable lunaris-server.service >/dev/null 2>&1 || true
    fi
    if [ -d /run/systemd/system ]; then
        systemctl daemon-reload >/dev/null 2>&1 || true
        if systemctl -q is-enabled lunaris-server.service 2>/dev/null; then
            systemctl restart lunaris-server.service >/dev/null 2>&1 || true
        fi
    fi
fi

if [ -z "$2" ] || [ "$had_db" = no ]; then
    port="$(sed -n 's/^[[:space:]]*SERVER_PORT=[[:space:]]*\([0-9]*\).*/\1/p' "$ENV_FILE" | tail -1)"
    echo
    if [ "$had_db" = yes ]; then
        echo "Lunaris server: open http://<this-machine>:${port:-8090} and sign in."
        echo "  Existing database kept: use your current admin account."
    else
        # The code itself is not printed: apt keeps the terminal output in
        # /var/log/apt/term.log.
        echo "Lunaris server: open http://<this-machine>:${port:-8090} and create the first admin."
        echo "  The page asks for the one-time setup code in $DATA/setup_code.txt"
        echo "  (written when the service first starts):"
        echo "    sudo cat $DATA/setup_code.txt"
        echo "  Or take over a server run by hand elsewhere (service stopped):"
        echo "    sudo lunaris-server import /home/<user>/.local/share/lunaris-server"
    fi
    echo "  Settings: $ENV_FILE   Logs: journalctl -u lunaris-server"
fi
exit 0
EOF

# ── prerm ──
cat > "$root/DEBIAN/prerm" <<'EOF'
#!/bin/sh
set -e
# Stopped, not disabled: a reinstall starts it again (purge drops the link).
if [ "$1" = remove ] && [ -d /run/systemd/system ]; then
    systemctl stop lunaris-server.service >/dev/null 2>&1 || true
fi
exit 0
EOF

# ── postrm ──
cat > "$root/DEBIAN/postrm" <<'EOF'
#!/bin/sh
set -e
if [ -d /run/systemd/system ]; then
    systemctl daemon-reload >/dev/null 2>&1 || true
fi
if [ "$1" = purge ]; then
    # Settings and data (database, agent token, sign-in keys, certificate).
    # The lunaris user stays, as is usual for system users.
    rm -f /etc/lunaris/server.env
    rmdir /etc/lunaris 2>/dev/null || true
    rm -rf /var/lib/lunaris /var/lib/private/lunaris
    rm -f /etc/systemd/system/multi-user.target.wants/lunaris-server.service
    if [ -d /run/systemd/system ]; then
        systemctl reset-failed lunaris-server.service >/dev/null 2>&1 || true
    fi
fi
exit 0
EOF
chmod 755 "$root/DEBIAN/postinst" "$root/DEBIAN/prerm" "$root/DEBIAN/postrm"

deb="$OUT/${NAME}_${VERSION}_${ARCH}.deb"
dpkg-deb --build --root-owner-group "$root" "$deb" >/dev/null
echo "Built $deb"
