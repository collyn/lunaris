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
    let content = format!(
        r#"[Desktop Entry]
Type=Application
Name=Lunaris Client
Exec='{}' %u
Terminal=false
MimeType=x-scheme-handler/lunaris;
Categories=Network;
"#,
        exe_path
    );

    std::fs::write(&dest_file, content)?;
    info!("Registered desktop entry at {:?}", dest_file);

    // Register mimetype handler via xdg-mime
    let status = std::process::Command::new("xdg-mime")
        .args(&[
            "default",
            "lunaris-client.desktop",
            "x-scheme-handler/lunaris",
        ])
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
