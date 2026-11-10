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
Summary:        Lunaris remote desktop host agent
Requires:       qt6-qtdeclarative
Recommends:     xdg-desktop-portal
Recommends:     pipewire
Recommends:     mesa-va-drivers
Recommends:     qt6-qtwayland

%description -n lunaris-agent
Shares this computer's screen with Lunaris clients: PipeWire/portal capture on
Wayland (X11 otherwise), hardware encoding (VAAPI, NVENC, Quick Sync) and
remote keyboard/mouse through uinput.

%package -n lunaris-client
Summary:        Lunaris remote desktop client
Requires:       qt6-qtdeclarative
Requires:       qt6-qtmultimedia
Recommends:     xdg-utils
Recommends:     mesa-va-drivers
Recommends:     qt6-qtwayland

%description -n lunaris-client
Native low-latency client for computers running the Lunaris agent, with
hardware video decoding (VAAPI, NVDEC).

%install
cp -a %{stage_agent}/. %{buildroot}/
cp -a %{stage_client}/. %{buildroot}/

%post -n lunaris-agent
modprobe uinput >/dev/null 2>&1 || :
udevadm control --reload-rules >/dev/null 2>&1 || :
udevadm trigger --subsystem-match=misc --sysname-match=uinput >/dev/null 2>&1 || :

%files -n lunaris-agent
%license /usr/share/licenses/lunaris-agent
/usr/bin/lunaris-agent
/usr/share/applications/lunaris-agent.desktop
/usr/share/icons/hicolor/256x256/apps/lunaris-agent.png
/usr/lib/udev/rules.d/60-lunaris-input.rules
/usr/lib/modules-load.d/lunaris.conf

%files -n lunaris-client
%license /usr/share/licenses/lunaris-client
/usr/bin/lunaris-client
/usr/share/applications/lunaris-client.desktop
/usr/share/icons/hicolor/256x256/apps/lunaris-client.png
