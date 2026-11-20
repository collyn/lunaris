# RPM packages for prebuilt Lunaris binaries (Fedora). Built by make-rpm.sh,
# which stages the files and passes them in with --define.
# rpmbuild's ELF dependency generator adds the shared-library Requires.

%global debug_package %{nil}
%global __brp_strip_static_archive %{nil}

Name:           lunaris
Version:        %{lunaris_version}
Release:        1%{?dist}
Summary:        Low-latency remote desktop
License:        GPL-3.0-or-later
URL:            https://github.com/collyn/lunaris

%description
Lunaris remote desktop.

%package -n lunaris-agent
Summary:        Lunaris remote desktop agent (command line and services, no GUI)
Recommends:     xdg-desktop-portal
Recommends:     pipewire
Recommends:     mesa-va-drivers
Suggests:       lunaris-approval-helper

%description -n lunaris-agent
Shares this computer with a Lunaris server and is managed from its web page.
On a desktop it runs as the systemd user service lunaris-agent (KMS capture
through the lunaris-capture service, PipeWire/portal or X11 otherwise,
hardware encoding, remote keyboard/mouse through uinput); on a headless server
as the system service lunaris-agent-system. Join a server with
lunaris-agent --enroll (see lunaris-agent --help).

%package -n lunaris-approval-helper
Summary:        Lunaris agent: tray that accepts connections
Requires:       lunaris-agent >= %{version}
Requires:       qt6-qtdeclarative
Recommends:     qt6-qtwayland

%description -n lunaris-approval-helper
Optional companion of lunaris-agent on a desktop: started at login, it asks
the person at the computer to accept or decline a connection made without the
access password.

%package -n lunaris-client
Summary:        Lunaris remote desktop client
Requires:       qt6-qtdeclarative
Requires:       qt6-qtmultimedia
Recommends:     xdg-utils
Recommends:     mesa-va-drivers
Recommends:     qt6-qtwayland

%description -n lunaris-client
The low-latency stream window for the computers of a Lunaris server, opened
from its web page, with hardware video decoding (VAAPI, NVDEC). Computers are
managed on the server's web page; lunaris-agent shares this computer.

%install
cp -a %{stage_agent}/. %{buildroot}/
cp -a %{stage_helper}/. %{buildroot}/
cp -a %{stage_client}/. %{buildroot}/

%post -n lunaris-agent
# The account of the headless system service (lunaris-agent-system.service).
systemd-sysusers lunaris-agent.conf >/dev/null 2>&1 || :
modprobe uinput >/dev/null 2>&1 || :
udevadm control --reload-rules >/dev/null 2>&1 || :
udevadm trigger --subsystem-match=misc --sysname-match=uinput >/dev/null 2>&1 || :
# Capture service (unattended Wayland capture): enable on install, restart
# on upgrade, stop and disable on removal.
if [ -d /run/systemd/system ]; then
    systemctl daemon-reload >/dev/null 2>&1 || :
    if [ "$1" -eq 1 ]; then
        systemctl enable --now lunaris-capture.service >/dev/null 2>&1 || :
    else
        systemctl try-restart lunaris-capture.service lunaris-agent-system.service >/dev/null 2>&1 || :
    fi
fi

%preun -n lunaris-agent
if [ "$1" -eq 0 ] && [ -d /run/systemd/system ]; then
    systemctl disable --now lunaris-capture.service lunaris-agent-system.service >/dev/null 2>&1 || :
fi

%postun -n lunaris-agent
if [ -d /run/systemd/system ]; then
    systemctl daemon-reload >/dev/null 2>&1 || :
fi

%files -n lunaris-agent
%license /usr/share/licenses/lunaris-agent
/usr/bin/lunaris-agent
/usr/lib/systemd/user/lunaris-agent.service
/usr/lib/systemd/system/lunaris-agent-system.service
/usr/lib/sysusers.d/lunaris-agent.conf
/usr/lib/udev/rules.d/60-lunaris-input.rules
/usr/lib/modules-load.d/lunaris.conf
/usr/lib/systemd/system/lunaris-capture.service

%files -n lunaris-approval-helper
%license /usr/share/licenses/lunaris-approval-helper
/usr/bin/lunaris-approval-helper
%config(noreplace) /etc/xdg/autostart/lunaris-approval-helper.desktop
/usr/share/icons/hicolor/256x256/apps/lunaris-approval-helper.png

%files -n lunaris-client
%license /usr/share/licenses/lunaris-client
/usr/bin/lunaris-client
/usr/share/applications/lunaris-client.desktop
/usr/share/icons/hicolor/256x256/apps/lunaris-client.png
