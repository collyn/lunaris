//! How this copy was installed, and which release file updates it.
//!
//! Release file names come from .github/workflows/release.yml ("Organize
//! Release Files") and scripts/packaging/linux/build-packages.sh:
//!   lunaris-client_<v>_amd64.deb            lunaris-agent_<v>_amd64.deb
//!   lunaris-client-<v>-1.fcNN.x86_64.rpm    lunaris-agent-<v>-1.fcNN.x86_64.rpm
//!   lunaris-client-<v>-1-x86_64.pkg.tar.zst lunaris-agent-<v>-1-x86_64.pkg.tar.zst
//!   lunaris-client-desktop-linux.AppImage   lunaris-agent-linux-x86_64.AppImage
//!   lunaris-client-qml-linux.AppImage       (the same program, older name)
//!   lunaris-client-installer-windows.exe    lunaris-agent-installer-windows.exe

use std::path::{Path, PathBuf};

/// Which program updates itself.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum App {
    /// The Lunaris app (client-desktop: client + agent + server).
    Client,
    /// The agent (lunaris-agent).
    Agent,
}

impl App {
    /// The Linux package and binary name.
    pub fn package_name(self) -> &'static str {
        match self {
            App::Client => "lunaris-client",
            App::Agent => "lunaris-agent",
        }
    }

    pub fn other(self) -> App {
        match self {
            App::Client => App::Agent,
            App::Agent => App::Client,
        }
    }

    pub fn id(self) -> &'static str {
        match self {
            App::Client => "client",
            App::Agent => "agent",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PackageManager {
    Dpkg,
    Rpm,
    Pacman,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Os {
    Linux,
    Windows,
    Other,
}

impl Os {
    pub fn current() -> Os {
        if cfg!(target_os = "linux") {
            Os::Linux
        } else if cfg!(windows) {
            Os::Windows
        } else {
            Os::Other
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum InstallKind {
    /// A Linux AppImage at this path (user-owned file, replaced in place).
    AppImage { path: PathBuf },
    /// A distro package of this manager (installed through pkexec).
    Package(PackageManager),
    /// The Windows NSIS installer's folder (uninstall.exe and install.log beside the exe).
    WindowsInstaller { dir: PathBuf },
    /// Built from a source tree (cargo target folder).
    Dev,
    /// Unpacked from a zip / tar.gz, or copied: no installer to run.
    Portable,
    /// An OS without release files (macOS).
    Unsupported,
}

impl InstallKind {
    pub fn id(&self) -> &'static str {
        match self {
            InstallKind::AppImage { .. } => "appimage",
            InstallKind::Package(PackageManager::Dpkg) => "deb",
            InstallKind::Package(PackageManager::Rpm) => "rpm",
            InstallKind::Package(PackageManager::Pacman) => "arch",
            InstallKind::WindowsInstaller { .. } => "windows-installer",
            InstallKind::Dev => "dev",
            InstallKind::Portable => "portable",
            InstallKind::Unsupported => "unsupported",
        }
    }
}

/// What [`classify`] looks at.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Probe {
    pub os: Os,
    /// The program file (for an AppImage: inside its mount).
    pub exe: PathBuf,
    /// `$APPIMAGE` (the AppImage file), set by the AppImage runtime.
    pub appimage: Option<PathBuf>,
    /// `$APPDIR` (the AppImage mount), set by the AppImage runtime.
    pub appdir: Option<PathBuf>,
    /// The package manager that owns `exe`, if one does.
    pub package_owner: Option<PackageManager>,
    /// Windows: `uninstall.exe` and `install.log` beside the exe.
    pub nsis_files_present: bool,
}

/// How this copy was installed (pure).
pub fn classify(p: &Probe) -> InstallKind {
    match p.os {
        Os::Linux => {
            if let Some(image) = p.appimage.as_ref().filter(|a| !a.as_os_str().is_empty()) {
                // A program started from an AppImage inherits $APPIMAGE; it
                // is ours only when we run from inside its mount.
                let inside = match &p.appdir {
                    Some(dir) if !dir.as_os_str().is_empty() => p.exe.starts_with(dir),
                    _ => true,
                };
                if inside {
                    return InstallKind::AppImage { path: image.clone() };
                }
            }
            if let Some(m) = p.package_owner {
                return InstallKind::Package(m);
            }
            if in_cargo_target(&p.exe) {
                InstallKind::Dev
            } else {
                InstallKind::Portable
            }
        }
        Os::Windows => {
            if p.nsis_files_present {
                InstallKind::WindowsInstaller {
                    dir: p.exe.parent().map(Path::to_path_buf).unwrap_or_default(),
                }
            } else if in_cargo_target(&p.exe) {
                InstallKind::Dev
            } else {
                InstallKind::Portable
            }
        }
        Os::Other => InstallKind::Unsupported,
    }
}

/// `…/target/debug/x`, `…/lunaris-target/release/x`, `…/target/<triple>/release/x`.
fn in_cargo_target(exe: &Path) -> bool {
    let parent = exe.parent().and_then(|p| p.file_name()).and_then(|n| n.to_str());
    matches!(parent, Some("debug" | "release"))
        || exe
            .ancestors()
            .any(|a| a.file_name().and_then(|n| n.to_str()).is_some_and(|n| n == "target" || n.ends_with("-target")))
}

/// The release file that updates `app` installed as `kind` to `version`,
/// when the release has it (pure). `exe_name` is the program's file name
/// (the AppImage of the app comes under two names), `arch` is
/// `std::env::consts::ARCH`.
pub fn asset_for(app: App, kind: &InstallKind, exe_name: &str, version: &str, arch: &str, assets: &[&str]) -> Option<String> {
    if arch != "x86_64" {
        return None; // the only architecture with app and agent builds
    }
    let pkg = app.package_name();
    let has = |n: &str| assets.contains(&n).then(|| n.to_string());
    match kind {
        InstallKind::AppImage { .. } => match app {
            App::Client if exe_name.starts_with("client-qml") => has("lunaris-client-qml-linux.AppImage"),
            App::Client => has("lunaris-client-desktop-linux.AppImage"),
            App::Agent => has("lunaris-agent-linux-x86_64.AppImage"),
        },
        InstallKind::Package(PackageManager::Dpkg) => has(&format!("{pkg}_{version}_amd64.deb")),
        InstallKind::Package(PackageManager::Rpm) => {
            let prefix = format!("{pkg}-{version}-");
            let mut found: Vec<&str> = assets
                .iter()
                .copied()
                .filter(|n| n.starts_with(&prefix) && n.ends_with(".x86_64.rpm"))
                .filter(|n| !n.contains("debuginfo") && !n.contains("debugsource"))
                .collect();
            // A Fedora-built package first, then the shortest name.
            found.sort_by_key(|n| (!n.contains(".fc"), n.len()));
            found.first().map(|n| n.to_string())
        }
        InstallKind::Package(PackageManager::Pacman) => {
            let prefix = format!("{pkg}-{version}-");
            assets
                .iter()
                .find(|n| {
                    n.starts_with(&prefix)
                        && n.ends_with("-x86_64.pkg.tar.zst")
                        // `<pkg>-<ver>-<rel>-x86_64…`: nothing but the release number between
                        && !n[prefix.len()..n.len() - "-x86_64.pkg.tar.zst".len()].contains('-')
                })
                .map(|n| n.to_string())
        }
        InstallKind::WindowsInstaller { .. } => match app {
            App::Client => has("lunaris-client-installer-windows.exe"),
            App::Agent => has("lunaris-agent-installer-windows.exe"),
        },
        InstallKind::Dev | InstallKind::Portable | InstallKind::Unsupported => None,
    }
}

/// Why an update cannot be installed from the app (only the notice and a link).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ManualReason {
    Dev,
    Portable,
    Unsupported,
    /// The release has no file for this kind of install.
    NoAsset,
    /// This build has the placeholder signing key.
    NotSigned,
    /// The release has no SHA256SUMS / SHA256SUMS.sig.
    ReleaseNotSigned,
}

impl ManualReason {
    pub fn id(self) -> &'static str {
        match self {
            ManualReason::Dev => "dev",
            ManualReason::Portable => "portable",
            ManualReason::Unsupported => "unsupported",
            ManualReason::NoAsset => "noAsset",
            ManualReason::NotSigned => "notSigned",
            ManualReason::ReleaseNotSigned => "releaseNotSigned",
        }
    }

    pub fn message(self) -> &'static str {
        match self {
            ManualReason::Dev => "This is a development build: update it from its source tree.",
            ManualReason::Portable => "This copy was not installed with a package or the installer: download the new version from the release page.",
            ManualReason::Unsupported => "Lunaris cannot update itself on this system: download the new version from the release page.",
            ManualReason::NoAsset => "The release has no file for this kind of install: download it from the release page.",
            ManualReason::NotSigned => "Updates are not signed in this build: download the new version from the release page.",
            ManualReason::ReleaseNotSigned => "This release is not signed: download it from the release page.",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Plan {
    /// Download, check and install these release files (the first is the
    /// program itself; the others are companion packages, see [`plan`]).
    Install { assets: Vec<String> },
    Manual(ManualReason),
}

/// What the Update button does (pure). `companions`: the other Lunaris
/// package when it is installed by the same package manager; it is updated
/// in the same transaction so the app, the agent and the capture service
/// stay on one version.
#[allow(clippy::too_many_arguments)]
pub fn plan(
    app: App,
    kind: &InstallKind,
    exe_name: &str,
    version: &str,
    arch: &str,
    assets: &[&str],
    signing_configured: bool,
    companions: &[App],
) -> Plan {
    match kind {
        InstallKind::Dev => return Plan::Manual(ManualReason::Dev),
        InstallKind::Portable => return Plan::Manual(ManualReason::Portable),
        InstallKind::Unsupported => return Plan::Manual(ManualReason::Unsupported),
        _ => {}
    }
    let Some(main) = asset_for(app, kind, exe_name, version, arch, assets) else {
        return Plan::Manual(ManualReason::NoAsset);
    };
    if !signing_configured {
        return Plan::Manual(ManualReason::NotSigned);
    }
    if !assets.contains(&crate::verify::SUMS_NAME) || !assets.contains(&crate::verify::SIG_NAME) {
        return Plan::Manual(ManualReason::ReleaseNotSigned);
    }
    let mut list = vec![main];
    if matches!(kind, InstallKind::Package(_)) {
        for c in companions.iter().filter(|c| **c != app) {
            if let Some(a) = asset_for(*c, kind, c.package_name(), version, arch, assets) {
                list.push(a);
            }
        }
    }
    Plan::Install { assets: list }
}

// ---- Looking at this computer (not pure) ----

/// The [`Probe`] for this process. `exe` is the program file as started
/// (see `restart::launch_exe`).
pub fn probe(app: App, exe: &Path) -> Probe {
    let env_path = |k: &str| std::env::var_os(k).filter(|v| !v.is_empty()).map(PathBuf::from);
    let os = Os::current();
    let package_owner = if os == Os::Linux { package_owner(app, exe) } else { None };
    let nsis_files_present = os == Os::Windows
        && exe
            .parent()
            .is_some_and(|d| d.join("uninstall.exe").is_file() && d.join("install.log").is_file());
    Probe {
        os,
        exe: exe.to_path_buf(),
        appimage: env_path("APPIMAGE"),
        appdir: env_path("APPDIR"),
        package_owner,
        nsis_files_present,
    }
}

/// The manager whose package `app` owns `exe` (`/usr/bin/lunaris-client`).
fn package_owner(app: App, exe: &Path) -> Option<PackageManager> {
    let pkg = app.package_name();
    if exe != Path::new("/usr/bin").join(pkg) {
        return None;
    }
    [PackageManager::Dpkg, PackageManager::Pacman, PackageManager::Rpm]
        .into_iter()
        .find(|m| package_installed(*m, pkg))
}

/// Whether the package `pkg` is installed by `m` (dpkg and pacman: their
/// database files; rpm: `rpm -q`).
pub fn package_installed(m: PackageManager, pkg: &str) -> bool {
    match m {
        PackageManager::Dpkg => {
            let info = Path::new("/var/lib/dpkg/info");
            let listed = info.join(format!("{pkg}.list")).is_file()
                || info.join(format!("{pkg}:amd64.list")).is_file();
            // A removed but not purged package keeps no .list; a half-
            // configured one does, and apt would fix it, so that counts.
            listed
        }
        PackageManager::Pacman => std::fs::read_dir("/var/lib/pacman/local")
            .map(|dir| {
                dir.flatten().any(|e| {
                    e.file_name().to_str().is_some_and(|n| pacman_entry_is(n, pkg))
                })
            })
            .unwrap_or(false),
        PackageManager::Rpm => {
            let Some(rpm) = find_program("rpm") else { return false };
            if !Path::new("/var/lib/rpm").exists() && !Path::new("/usr/lib/sysimage/rpm").exists() {
                return false;
            }
            std::process::Command::new(rpm)
                .args(["-q", "--qf", "%{NAME}", pkg])
                .stdin(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .output()
                .map(|o| o.status.success() && o.stdout == pkg.as_bytes())
                .unwrap_or(false)
        }
    }
}

/// `/var/lib/pacman/local/<pkg>-<ver>-<rel>` (a package named `<pkg>-x` is another one).
fn pacman_entry_is(entry: &str, pkg: &str) -> bool {
    entry
        .strip_prefix(pkg)
        .and_then(|r| r.strip_prefix('-'))
        .is_some_and(|r| r.matches('-').count() == 1 && r.chars().next().is_some_and(|c| c.is_ascii_digit()))
}

/// A system program by name, in the standard folders only (pkexec runs it as root).
pub fn find_program(name: &str) -> Option<PathBuf> {
    ["/usr/bin", "/bin", "/usr/sbin", "/sbin"]
        .iter()
        .map(|d| Path::new(d).join(name))
        .find(|p| p.is_file())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn linux(exe: &str) -> Probe {
        Probe {
            os: Os::Linux,
            exe: exe.into(),
            appimage: None,
            appdir: None,
            package_owner: None,
            nsis_files_present: false,
        }
    }

    #[test]
    fn classify_appimage() {
        let mut p = linux("/tmp/.mount_LunariXYZ/usr/bin/client-desktop");
        p.appimage = Some("/home/u/Apps/Lunaris.AppImage".into());
        p.appdir = Some("/tmp/.mount_LunariXYZ".into());
        assert_eq!(classify(&p), InstallKind::AppImage { path: "/home/u/Apps/Lunaris.AppImage".into() });
        // Started from an AppImage's environment, but not running from it.
        p.exe = "/usr/bin/lunaris-client".into();
        p.package_owner = Some(PackageManager::Dpkg);
        assert_eq!(classify(&p), InstallKind::Package(PackageManager::Dpkg));
        // Empty $APPIMAGE does not count.
        let mut q = linux("/opt/x/client-desktop");
        q.appimage = Some("".into());
        assert_eq!(classify(&q), InstallKind::Portable);
    }

    #[test]
    fn classify_packages_dev_and_portable() {
        for m in [PackageManager::Dpkg, PackageManager::Rpm, PackageManager::Pacman] {
            let mut p = linux("/usr/bin/lunaris-agent");
            p.package_owner = Some(m);
            assert_eq!(classify(&p), InstallKind::Package(m));
        }
        assert_eq!(classify(&linux("/mnt/nvme/lunaris-target/debug/client-desktop")), InstallKind::Dev);
        assert_eq!(classify(&linux("/home/u/src/lunaris/target/x86_64-unknown-linux-musl/release/server")), InstallKind::Dev);
        assert_eq!(classify(&linux("/home/u/lunaris-agent/lunaris-agent")), InstallKind::Portable);
        // /usr/bin but no package owns it (copied there by hand).
        assert_eq!(classify(&linux("/usr/bin/lunaris-client")), InstallKind::Portable);
    }

    #[test]
    fn classify_windows_and_others() {
        let mut p = linux("C:\\Program Files\\Lunaris Client\\client-desktop.exe");
        p.os = Os::Windows;
        p.nsis_files_present = true;
        assert!(matches!(classify(&p), InstallKind::WindowsInstaller { .. }));
        p.nsis_files_present = false;
        assert_eq!(classify(&p), InstallKind::Portable);
        p.os = Os::Other;
        assert_eq!(classify(&p), InstallKind::Unsupported);
    }

    /// The v0.6.2 release's files plus what the rpm/arch builds and the signature add.
    const ASSETS: &[&str] = &[
        "lunaris-agent-installer-windows.exe",
        "lunaris-agent-linux-x86_64.AppImage",
        "lunaris-agent-windows.zip",
        "lunaris-agent_0.6.4_amd64.deb",
        "lunaris-client-desktop-linux.AppImage",
        "lunaris-client-desktop-windows.zip",
        "lunaris-client-installer-windows.exe",
        "lunaris-client-qml-linux.AppImage",
        "lunaris-client-qml-windows.zip",
        "lunaris-client_0.6.4_amd64.deb",
        "lunaris-client-0.6.4-1.fc44.x86_64.rpm",
        "lunaris-client-debuginfo-0.6.4-1.fc44.x86_64.rpm",
        "lunaris-agent-0.6.4-1.fc44.x86_64.rpm",
        "lunaris-client-0.6.4-1-x86_64.pkg.tar.zst",
        "lunaris-client-debug-0.6.4-1-x86_64.pkg.tar.zst",
        "lunaris-agent-0.6.4-1-x86_64.pkg.tar.zst",
        "lunaris-server-linux-x86_64.tar.gz",
        "SHA256SUMS",
        "SHA256SUMS.sig",
    ];

    fn a(app: App, kind: InstallKind, exe: &str) -> Option<String> {
        asset_for(app, &kind, exe, "0.6.4", "x86_64", ASSETS)
    }

    #[test]
    fn asset_names() {
        let image = InstallKind::AppImage { path: "/x".into() };
        assert_eq!(a(App::Client, image.clone(), "client-desktop").as_deref(), Some("lunaris-client-desktop-linux.AppImage"));
        assert_eq!(a(App::Client, image.clone(), "client-qml").as_deref(), Some("lunaris-client-qml-linux.AppImage"));
        assert_eq!(a(App::Agent, image, "lunaris-agent").as_deref(), Some("lunaris-agent-linux-x86_64.AppImage"));
        let deb = InstallKind::Package(PackageManager::Dpkg);
        assert_eq!(a(App::Client, deb.clone(), "lunaris-client").as_deref(), Some("lunaris-client_0.6.4_amd64.deb"));
        assert_eq!(a(App::Agent, deb, "lunaris-agent").as_deref(), Some("lunaris-agent_0.6.4_amd64.deb"));
        let rpm = InstallKind::Package(PackageManager::Rpm);
        assert_eq!(a(App::Client, rpm.clone(), "lunaris-client").as_deref(), Some("lunaris-client-0.6.4-1.fc44.x86_64.rpm"));
        assert_eq!(a(App::Agent, rpm, "lunaris-agent").as_deref(), Some("lunaris-agent-0.6.4-1.fc44.x86_64.rpm"));
        let arch = InstallKind::Package(PackageManager::Pacman);
        assert_eq!(a(App::Client, arch.clone(), "lunaris-client").as_deref(), Some("lunaris-client-0.6.4-1-x86_64.pkg.tar.zst"));
        assert_eq!(a(App::Agent, arch, "lunaris-agent").as_deref(), Some("lunaris-agent-0.6.4-1-x86_64.pkg.tar.zst"));
        let win = InstallKind::WindowsInstaller { dir: "C:\\x".into() };
        assert_eq!(a(App::Client, win.clone(), "client-desktop.exe").as_deref(), Some("lunaris-client-installer-windows.exe"));
        assert_eq!(a(App::Agent, win, "lunaris-agent.exe").as_deref(), Some("lunaris-agent-installer-windows.exe"));
        // Another version, another architecture, a non-installable kind.
        assert_eq!(asset_for(App::Client, &InstallKind::Package(PackageManager::Dpkg), "x", "0.6.5", "x86_64", ASSETS), None);
        assert_eq!(asset_for(App::Client, &InstallKind::Package(PackageManager::Dpkg), "x", "0.6.4", "aarch64", ASSETS), None);
        assert_eq!(a(App::Client, InstallKind::Portable, "x"), None);
    }

    #[test]
    fn plans() {
        let deb = InstallKind::Package(PackageManager::Dpkg);
        assert_eq!(
            plan(App::Client, &deb, "lunaris-client", "0.6.4", "x86_64", ASSETS, true, &[App::Agent]),
            Plan::Install { assets: vec!["lunaris-client_0.6.4_amd64.deb".into(), "lunaris-agent_0.6.4_amd64.deb".into()] }
        );
        assert_eq!(
            plan(App::Agent, &deb, "lunaris-agent", "0.6.4", "x86_64", ASSETS, true, &[]),
            Plan::Install { assets: vec!["lunaris-agent_0.6.4_amd64.deb".into()] }
        );
        // The placeholder key: notice only.
        assert_eq!(
            plan(App::Agent, &deb, "lunaris-agent", "0.6.4", "x86_64", ASSETS, false, &[]),
            Plan::Manual(ManualReason::NotSigned)
        );
        // An old release without SHA256SUMS.
        let unsigned: Vec<&str> = ASSETS.iter().copied().filter(|n| !n.starts_with("SHA256SUMS")).collect();
        assert_eq!(
            plan(App::Agent, &deb, "lunaris-agent", "0.6.4", "x86_64", &unsigned, true, &[]),
            Plan::Manual(ManualReason::ReleaseNotSigned)
        );
        assert_eq!(plan(App::Client, &InstallKind::Dev, "x", "0.6.4", "x86_64", ASSETS, true, &[]), Plan::Manual(ManualReason::Dev));
        assert_eq!(plan(App::Client, &InstallKind::Portable, "x", "0.6.4", "x86_64", ASSETS, true, &[]), Plan::Manual(ManualReason::Portable));
        assert_eq!(
            plan(App::Client, &deb, "x", "0.6.9", "x86_64", ASSETS, true, &[]),
            Plan::Manual(ManualReason::NoAsset)
        );
        // AppImage: no companions even when asked.
        let image = InstallKind::AppImage { path: "/x".into() };
        assert_eq!(
            plan(App::Client, &image, "client-desktop", "0.6.4", "x86_64", ASSETS, true, &[App::Agent]),
            Plan::Install { assets: vec!["lunaris-client-desktop-linux.AppImage".into()] }
        );
    }

    #[test]
    fn pacman_entries() {
        assert!(pacman_entry_is("lunaris-agent-0.6.3-1", "lunaris-agent"));
        assert!(!pacman_entry_is("lunaris-agent-debug-0.6.3-1", "lunaris-agent"));
        assert!(!pacman_entry_is("lunaris-agentx-0.6.3-1", "lunaris-agent"));
    }
}
