#!/usr/bin/env bash
# Builds the Linux .debs (Ubuntu 24.04+, Debian 13+):
#   lunaris-agent_<ver>_<arch>.deb            the agent: command line and services, no Qt
#                                             (desktops and headless servers alike)
#   lunaris-approval-helper_<ver>_<arch>.deb  optional tray that accepts connections (Qt)
#   lunaris-client_<ver>_<arch>.deb           the stream window (Qt)
# Run on the build distro: dpkg-shlibdeps computes the shared-library
# dependencies (Qt, PipeWire, libva, ...) against it.
#
# One package serves every release from the build distro (Ubuntu 24.04) up, so
# the Qt dependencies name both spellings of each library package, see
# qt_package_alternatives. scripts/packaging/linux/smoke-deb.sh installs and
# starts the result on each supported release.
#
# Usage: make-deb.sh <cargo target dir> <output dir> <static ffmpeg prefix>
# Env:   ONLY="agent approval-helper client"  build a subset
set -euo pipefail

TARGET="$1"; OUT="$(mkdir -p "$2" && cd "$2" && pwd)"; FFMPEG="$3"
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$HERE/../../.." && pwd)"
VERSION="$(sed -n '/^\[workspace.package\]/,/^\[/s/^version *= *"\(.*\)"/\1/p' "$ROOT/Cargo.toml" | head -1)"
ARCH="$(dpkg --print-architecture)"

# Loaded at runtime (QML imports, Qt platform plugins), so invisible to shlibdeps.
QML_COMMON="qml6-module-qtquick, qml6-module-qtquick-controls, qml6-module-qtquick-layouts, \
qml6-module-qtquick-templates, qml6-module-qtquick-window, qml6-module-qtqml-workerscript, \
qml6-module-qt-labs-platform, qt6-qpa-plugins"

# Ubuntu 24.04 ships the Qt libraries as libqt6gui6t64, libqt6widgets6t64,
# libqt6opengl6t64 ...; Ubuntu 26.04 (Qt 6.10) and Debian 13 (Qt 6.8) name them
# libqt6gui6, libqt6widgets6, libqt6opengl6 again, with no Provides for the old
# names (libqt6core6t64 kept its name). Without the second name apt refuses the
# package there: "Depends: libqt6gui6t64 but it is not installable". The
# libraries are the same public Qt 6 ABI on 64-bit, so either name will do:
#   libqt6gui6t64 (>= 6.1.2)  ->  libqt6gui6t64 (>= 6.1.2) | libqt6gui6 (>= 6.1.2)
qt_package_alternatives() {
    sed -E 's/(libqt6[a-z0-9-]+)t64( \([^)]*\))?/\1t64\2 | \1\2/g'
}

# VA-API drivers: mesa-va-drivers covers AMD; Intel GPUs need Intel's own
# driver (iHD, Broadwell and newer), which Ubuntu does not install by
# default. Without it VAAPI and Quick Sync cannot start and the agent
# encodes on the CPU ("Failed to initialise VAAPI connection").
INTEL_VA="intel-media-va-driver | intel-media-va-driver-non-free"


# Maintainer scripts of the agent: the service account, remote input
# (uinput), the capture service (unattended capture on Wayland without the
# portal prompt, docs/linux-capture-service.md) and the system service
# (headless computers: installed, enabled only by `lunaris-agent --system
# --enroll`). The user service is enabled per user by `--enroll`.
agent_scripts() {
    local root="$1"
    cat > "$root/DEBIAN/postinst" <<'EOF'
#!/bin/sh
set -e
if [ "$1" = configure ]; then
    if command -v systemd-sysusers >/dev/null; then
        systemd-sysusers lunaris-agent.conf >/dev/null 2>&1 || true
    fi
    if ! getent passwd lunaris-agent >/dev/null; then
        adduser --system --group --home /var/lib/lunaris-agent --no-create-home \
            --shell /usr/sbin/nologin lunaris-agent >/dev/null 2>&1 || true
        for g in video render input; do
            if getent group "$g" >/dev/null; then adduser lunaris-agent "$g" >/dev/null 2>&1 || true; fi
        done
    fi
    modprobe uinput 2>/dev/null || true
    if command -v udevadm >/dev/null; then
        udevadm control --reload-rules 2>/dev/null || true
        udevadm trigger --subsystem-match=misc --sysname-match=uinput 2>/dev/null || true
    fi
    if [ -d /run/systemd/system ]; then
        systemctl daemon-reload >/dev/null 2>&1 || true
        if [ -z "$2" ]; then
            systemctl enable --now lunaris-capture.service >/dev/null 2>&1 || true
        else
            # Upgrade: keep the admin's choices, run the new binary. (Agents
            # run by users restart themselves once their file is replaced.)
            systemctl try-restart lunaris-capture.service lunaris-agent-system.service >/dev/null 2>&1 || true
        fi
    fi
fi
EOF
    cat > "$root/DEBIAN/prerm" <<'EOF'
#!/bin/sh
set -e
if [ "$1" = remove ] && [ -d /run/systemd/system ]; then
    systemctl disable --now lunaris-capture.service lunaris-agent-system.service >/dev/null 2>&1 || true
fi
EOF
    cat > "$root/DEBIAN/postrm" <<'EOF'
#!/bin/sh
set -e
if [ -d /run/systemd/system ]; then
    systemctl daemon-reload >/dev/null 2>&1 || true
fi
# Purge: the system service's data (its config holds the agent key) goes
# too; the users' own configs (~/.config/lunaris-agent) stay.
if [ "$1" = purge ]; then
    rm -rf /var/lib/lunaris-agent
fi
EOF
    chmod 755 "$root/DEBIAN/postinst" "$root/DEBIAN/prerm" "$root/DEBIAN/postrm"
}

# build <kind> <binary> <qt: yes|no> <extra Depends> <extra Recommends> <Suggests> <description>
build() {
    local kind="$1" bin="$2" qt="$3" extra_deps="$4" recommends="$5" suggests="$6" description="$7"
    local name="lunaris-$kind"
    local work; work="$(mktemp -d)"
    local root="$work/$name"

    "$HERE/stage.sh" "$kind" "$bin" "$root" "$FFMPEG"
    strip --strip-unneeded "$root/usr/bin/$name"
    install -Dm644 "$ROOT/LICENSE" "$root/usr/share/doc/$name/copyright"

    mkdir -p "$work/debian"
    printf 'Source: %s\n' "$name" > "$work/debian/control"
    local shlibs
    shlibs="$(cd "$work" && dpkg-shlibdeps -O -e "$root/usr/bin/$name" 2>/dev/null | sed -n 's/^shlibs:Depends=//p')"
    if [[ "$qt" == yes ]]; then
        # A dependency on Qt's private ABI pins the package to the exact Qt
        # version of the build distro; the alternatives below would then
        # promise too much.
        if grep -q 'private-abi' <<<"$shlibs"; then
            echo "error: $name uses Qt private API ($shlibs): it would only run with the build distro's Qt" >&2
            exit 1
        fi
        shlibs="$(qt_package_alternatives <<<"$shlibs")"
        extra_deps=", $QML_COMMON$extra_deps"
        recommends="qt6-wayland$recommends"
    else
        # The agent runs on servers without any desktop: no Qt at all.
        if grep -qi 'qt6' <<<"$shlibs"; then
            echo "error: $name links Qt ($shlibs); it must not" >&2
            exit 1
        fi
        recommends="${recommends#, }"
    fi

    mkdir -p "$root/DEBIAN"
    {
        echo "Package: $name"
        echo "Version: $VERSION"
        echo "Architecture: $ARCH"
        echo "Maintainer: Lunaris <noreply@lunaris.invalid>"
        echo "Section: net"
        echo "Priority: optional"
        echo "Homepage: https://github.com/collyn/lunaris"
        echo "Installed-Size: $(du -sk "$root" | cut -f1)"
        echo "Depends: $shlibs$extra_deps"
        if [[ -n "$recommends" ]]; then echo "Recommends: $recommends"; fi
        if [[ -n "$suggests" ]]; then echo "Suggests: $suggests"; fi
        echo "Description: $description"
    } > "$root/DEBIAN/control"

    if [[ "$kind" == agent ]]; then agent_scripts "$root"; fi
    # Files under /etc are the admin's to change (and to keep on upgrades).
    if [[ -d "$root/etc" ]]; then
        (cd "$root" && find etc -type f -printf '/%p\n') > "$root/DEBIAN/conffiles"
    fi

    dpkg-deb --build --root-owner-group "$root" "$OUT/${name}_${VERSION}_${ARCH}.deb" >/dev/null
    echo "Built $OUT/${name}_${VERSION}_${ARCH}.deb"
    dpkg-deb --field "$OUT/${name}_${VERSION}_${ARCH}.deb" Depends
    rm -rf "$work"
}

# Which packages (default: all three); e.g. ONLY="agent" without Qt installed.
ONLY="${ONLY:-agent approval-helper client}"

# The agent: Wayland capture through the portal needs xdg-desktop-portal and
# PipeWire, hardware encoding the VA-API drivers: only where there is a
# screen, so recommended, not required (a VPS installs none of it).
if [[ " $ONLY " == *" agent "* ]]; then
build agent "$TARGET/release/lunaris-agent" no "" \
    ", xdg-desktop-portal, pipewire, mesa-va-drivers, $INTEL_VA" \
    "lunaris-approval-helper" \
    "Lunaris remote desktop agent (command line and services, no GUI)
 Shares this computer with a Lunaris server and is managed from its web
 page. On a desktop it runs as the systemd user service lunaris-agent:
 unattended KMS capture through the lunaris-capture service (Wayland
 without the portal prompt), PipeWire/portal or X11 otherwise, hardware
 encoding (VAAPI, NVENC, Quick Sync) and remote keyboard/mouse through
 uinput. On a headless server it runs as the system service
 lunaris-agent-system (RDP, VNC and Sunshine sources, direct access).
 Join a server with lunaris-agent --enroll (see lunaris-agent --help)."
fi

if [[ " $ONLY " == *" approval-helper "* ]]; then
build approval-helper "$TARGET/release/lunaris-approval-helper" yes ", lunaris-agent (>= $VERSION)" "" "" \
    "Lunaris agent: tray that accepts connections
 Optional companion of lunaris-agent on a desktop: started at login, it
 asks the person at the computer to accept or decline a connection made
 without the access password. Without it only the password lets anyone in."
fi

if [[ " $ONLY " == *" client "* ]]; then
build client "$TARGET/release/client-desktop" yes \
    ", qml6-module-qtmultimedia, qml6-module-qtquick-shapes" \
    ", mesa-va-drivers, $INTEL_VA, xdg-utils, xdg-desktop-portal, pipewire" "" \
    "Lunaris remote desktop stream window
 The low-latency window for the computers of a Lunaris server, opened from
 its web page (Connect), with hardware video decoding (VAAPI, NVDEC), sound,
 clipboard, file transfer and game controllers. Computers are managed on the
 server's web page; lunaris-agent shares this computer."
fi
