//! What this agent is, as the server is told (`common::AgentBuild`): its
//! version, its OS and how it was installed, which decides how it is
//! updated (the server shows the command, see server/src/install/).

use std::path::Path;

/// Where the Linux installer (server/src/install/agent.sh) puts the AppImage.
const INSTALLED_APPIMAGE: &str = "/opt/lunaris-agent/lunaris-agent.AppImage";

pub fn current() -> common::AgentBuild {
    common::AgentBuild {
        version: env!("CARGO_PKG_VERSION").to_string(),
        os: std::env::consts::OS.to_string(),
        install: install_kind().to_string(),
        game_mouse: crate::game_mouse::availability().map(str::to_string),
        remote_update: Some(crate::remote_update::offered()),
    }
}

/// How this agent was installed (the `INSTALL_*` values).
pub fn current_install() -> String {
    install_kind().to_string()
}

#[cfg(target_os = "linux")]
fn install_kind() -> &'static str {
    let exe = std::env::current_exe().ok();
    linux_kind(
        std::env::var_os("APPIMAGE").as_deref().map(Path::new),
        exe.as_deref(),
        Path::new("/var/lib/dpkg/info/lunaris-agent.list").exists(),
    )
}

/// `appimage`: the AppImage being run ($APPIMAGE); `exe`: this program;
/// `dpkg_owns`: the lunaris-agent package is installed with dpkg.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn linux_kind(appimage: Option<&Path>, exe: Option<&Path>, dpkg_owns: bool) -> &'static str {
    match appimage {
        Some(p) if p == Path::new(INSTALLED_APPIMAGE) => common::INSTALL_APPIMAGE,
        Some(_) => common::INSTALL_MANUAL,
        None if exe == Some(Path::new("/usr/bin/lunaris-agent")) => {
            if dpkg_owns { common::INSTALL_DEB } else { common::INSTALL_PACKAGE }
        }
        None => common::INSTALL_MANUAL,
    }
}

#[cfg(windows)]
fn install_kind() -> &'static str {
    // The installer puts it in Program Files (its service runs it from there).
    let exe = std::env::current_exe().ok().map(|p| p.to_string_lossy().to_lowercase());
    let program_files = std::env::var("ProgramFiles").ok().map(|p| p.to_lowercase());
    match (exe, program_files) {
        (Some(exe), Some(dir)) if exe.starts_with(&format!("{}\\", dir.trim_end_matches('\\'))) => common::INSTALL_WINDOWS,
        _ => common::INSTALL_MANUAL,
    }
}

#[cfg(not(any(target_os = "linux", windows)))]
fn install_kind() -> &'static str {
    common::INSTALL_MANUAL
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn how_it_was_installed() {
        let usr = Some(Path::new("/usr/bin/lunaris-agent"));
        assert_eq!(linux_kind(None, usr, true), "deb");
        assert_eq!(linux_kind(None, usr, false), "package");
        assert_eq!(linux_kind(Some(Path::new(INSTALLED_APPIMAGE)), Some(Path::new("/tmp/.mount/usr/bin/x")), false), "appimage");
        assert_eq!(linux_kind(Some(Path::new("/home/a/Lunaris-Agent.AppImage")), None, true), "manual");
        assert_eq!(linux_kind(None, Some(Path::new("/home/a/lunaris/target/release/lunaris-agent")), true), "manual");
        assert_eq!(current().version, env!("CARGO_PKG_VERSION"));
    }
}
