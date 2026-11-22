#[cfg(any(target_os = "linux", target_os = "windows"))]
use tracing::{info, warn};

pub fn register_protocol() -> Result<(), anyhow::Error> {
    #[cfg(any(target_os = "linux", target_os = "windows"))]
    {
        let current_exe = std::env::current_exe()?;
        let exe_path = current_exe.to_string_lossy().into_owned();

        #[cfg(target_os = "linux")]
        {
            register_linux(&exe_path)?;
        }

        #[cfg(target_os = "windows")]
        {
            register_windows(&exe_path)?;
        }
    }

    Ok(())
}

/// The program in a desktop entry `Exec=` line: in double quotes, with the
/// characters the spec reserves escaped (once for Exec, once more for the
/// desktop file's own string escapes) and `%` doubled.
#[cfg(any(target_os = "linux", test))]
fn desktop_exec_quote(path: &str) -> String {
    let mut out = String::from("\"");
    for ch in path.chars() {
        match ch {
            '"' | '`' | '$' => {
                out.push_str("\\\\");
                out.push(ch);
            }
            '\\' => out.push_str("\\\\\\\\"),
            '%' => out.push_str("%%"),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// The app's icon, for a desktop entry of an app that no package installed.
#[cfg(target_os = "linux")]
const APP_ICON: &[u8] = include_bytes!("../qml/icon.png");

/// The desktop entry a package installs (scripts/packaging/linux/stage.sh).
#[cfg(target_os = "linux")]
const PACKAGED_ENTRY: &str = "/usr/share/applications/lunaris-client.desktop";

/// Whether this program is the one a package installed: it runs from /usr
/// and the package's desktop entry is there.
#[cfg(any(target_os = "linux", test))]
fn installed_by_a_package(exe_path: &str, packaged_entry_exists: bool) -> bool {
    packaged_entry_exists && exe_path.starts_with("/usr/")
}

/// The user's own desktop entry for a program run from a folder. The name
/// of the icon is the one the packages use, so a theme's icon is found too.
#[cfg(any(target_os = "linux", test))]
fn desktop_entry(exe_path: &str) -> String {
    format!(
        r#"[Desktop Entry]
Type=Application
Name=Lunaris Client
Comment=Connect to computers running the Lunaris agent
Exec={} %u
Icon=lunaris-client
Terminal=false
MimeType=x-scheme-handler/lunaris;
Categories=Network;RemoteAccess;
NoDisplay=true
"#,
        desktop_exec_quote(exe_path)
    )
}

#[cfg(target_os = "linux")]
fn register_linux(exe_path: &str) -> Result<(), anyhow::Error> {
    let home = std::env::var("HOME")?;
    let dest_dir = std::path::Path::new(&home)
        .join(".local")
        .join("share")
        .join("applications");

    if !dest_dir.exists() {
        std::fs::create_dir_all(&dest_dir)?;
    }

    // Clean up old QML desktop entry file if it exists to avoid duplicates/confusion
    let old_file = dest_dir.join("lunaris-client-qml.desktop");
    if old_file.exists() {
        let _ = std::fs::remove_file(&old_file);
    }

    let dest_file = dest_dir.join("lunaris-client.desktop");
    if installed_by_a_package(exe_path, std::path::Path::new(PACKAGED_ENTRY).exists()) {
        // The package's own entry (with its icon) does this job. An entry
        // here, left by an earlier run, has the same name and would hide it:
        // that is how an installed app came to show no icon.
        if dest_file.exists() {
            let _ = std::fs::remove_file(&dest_file);
            info!("Removed {:?}: the installed package has its own entry", dest_file);
        }
    } else {
        // Run from a folder (a build, an AppImage): the entry and its icon
        // are the user's own.
        let icon_file = std::path::Path::new(&home).join(".local/share/icons/hicolor/256x256/apps/lunaris-client.png");
        if std::fs::metadata(&icon_file).map(|m| m.len() != APP_ICON.len() as u64).unwrap_or(true) {
            if let Some(dir) = icon_file.parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            let _ = std::fs::write(&icon_file, APP_ICON);
        }
        std::fs::write(&dest_file, desktop_entry(exe_path))?;
        info!("Registered desktop entry at {:?}", dest_file);
    }

    // Register mimetype handler via xdg-mime
    let status = std::process::Command::new("xdg-mime")
        .args(&[
            "default",
            "lunaris-client.desktop",
            "x-scheme-handler/lunaris",
        ])
        // xdg-mime's own complaints (e.g. "qtpaths: not found" on KDE) are
        // noise here; success is judged by the exit status.
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status();

    match status {
        Ok(s) if s.success() => {
            info!("Registered lunaris URI scheme handler successfully");
        }
        other => {
            warn!("Failed to run xdg-mime register: {:?}", other);
        }
    }

    // Update the desktop database so desktop environments index it immediately
    let _ = std::process::Command::new("update-desktop-database")
        .arg(&dest_dir)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status();

    Ok(())
}

#[cfg(target_os = "windows")]
fn register_windows(exe_path: &str) -> Result<(), anyhow::Error> {
    let _ = std::process::Command::new("reg")
        .args(&[
            "add",
            "HKCU\\Software\\Classes\\lunaris",
            "/v",
            "URL Protocol",
            "/t",
            "REG_SZ",
            "/d",
            "",
            "/f",
        ])
        .status();

    let _ = std::process::Command::new("reg")
        .args(&[
            "add",
            "HKCU\\Software\\Classes\\lunaris\\shell\\open\\command",
            "/ve",
            "/t",
            "REG_SZ",
            "/d",
            &format!("\"{}\" \"%1\"", exe_path),
            "/f",
        ])
        .status();

    info!("Registered Windows registry entry for lunaris://");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{desktop_entry, desktop_exec_quote, installed_by_a_package};

    #[test]
    fn a_program_run_from_a_folder_gets_an_entry_with_the_icon() {
        let entry = desktop_entry("/home/a b/lunaris/client-desktop");
        assert!(entry.contains("\nIcon=lunaris-client\n"));
        assert!(entry.contains("\nExec=\"/home/a b/lunaris/client-desktop\" %u\n"));
        assert!(entry.contains("\nMimeType=x-scheme-handler/lunaris;\n"));
        // Opened by links only: not in the menus.
        assert!(entry.contains("\nNoDisplay=true\n"));
    }

    #[test]
    fn the_packaged_app_leaves_the_entry_to_its_package() {
        assert!(installed_by_a_package("/usr/bin/lunaris-client", true));
        assert!(!installed_by_a_package("/usr/bin/lunaris-client", false));
        assert!(!installed_by_a_package("/home/a/target/release/client-desktop", true));
        assert!(!installed_by_a_package("/tmp/.mount_Lunaris/usr/bin/lunaris-client", true));
    }

    #[test]
    fn exec_path_is_quoted() {
        assert_eq!(desktop_exec_quote("/opt/Lunaris App/client"), "\"/opt/Lunaris App/client\"");
        assert_eq!(desktop_exec_quote("/a\"b$c"), "\"/a\\\\\"b\\\\$c\"");
        assert_eq!(desktop_exec_quote("/100%"), "\"/100%%\"");
    }
}
