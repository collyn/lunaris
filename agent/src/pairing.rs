use serde::{Deserialize, Serialize};
use std::fs;

fn default_server_url() -> String {
    "ws://127.0.0.1:8080".to_string()
}

fn default_webtransport_port() -> u16 {
    55200
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct AgentConfig {
    #[serde(default)]
    pub client_unique_id: String,
    #[serde(default)]
    pub client_private_key: String,
    #[serde(default)]
    pub client_certificate: String,
    #[serde(default)]
    pub server_certificate: String,
    #[serde(default = "default_server_url")]
    pub server_url: String,
    #[serde(default)]
    pub server_token: String,
    #[serde(default = "default_webtransport_port")]
    pub webtransport_port: u16,
    #[serde(default)]
    pub autostart: bool,
    #[serde(default)]
    pub close_to_tray: bool,
}



pub fn load_config(path: &str) -> Result<AgentConfig, anyhow::Error> {
    let content = fs::read_to_string(path)?;
    let config: AgentConfig = serde_json::from_str(&content)?;
    Ok(config)
}

pub fn save_config(config: &AgentConfig, path: &str) -> Result<(), anyhow::Error> {
    let content = serde_json::to_string_pretty(config)?;
    fs::write(path, content)?;
    Ok(())
}

pub fn import_config_file(imported_path: &str, target_path: &str) -> Result<(), anyhow::Error> {
    let content = fs::read_to_string(imported_path)?;
    let mut imported: AgentConfig = serde_json::from_str(&content)?;

    // Load existing config if available to preserve keys/certificates
    if let Ok(ext) = load_config(target_path) {
        if imported.client_unique_id.is_empty() {
            imported.client_unique_id = ext.client_unique_id;
        }
        if imported.client_private_key.is_empty() {
            imported.client_private_key = ext.client_private_key;
        }
        if imported.client_certificate.is_empty() {
            imported.client_certificate = ext.client_certificate;
        }
        if imported.server_certificate.is_empty() {
            imported.server_certificate = ext.server_certificate;
        }
    }

    save_config(&imported, target_path)?;
    Ok(())
}



#[cfg(target_os = "linux")]
fn get_autostart_path_linux() -> Option<std::path::PathBuf> {
    let home = std::env::var("HOME").ok()?;
    let mut path = std::path::PathBuf::from(home);
    path.push(".config");
    path.push("autostart");
    let _ = std::fs::create_dir_all(&path);
    path.push("lunaris-agent.desktop");
    Some(path)
}

#[allow(dead_code)]
fn get_autostart_path_macos() -> Option<std::path::PathBuf> {
    let home = std::env::var("HOME").ok()?;
    let mut path = std::path::PathBuf::from(home);
    path.push("Library");
    path.push("LaunchAgents");
    let _ = std::fs::create_dir_all(&path);
    path.push("com.lunaris.agent.plist");
    Some(path)
}

#[allow(dead_code)]
pub fn is_autostart_enabled_impl() -> bool {
    #[cfg(target_os = "linux")]
    {
        if let Some(path) = get_autostart_path_linux() {
            return path.exists();
        }
    }
    #[cfg(target_os = "macos")]
    {
        if let Some(path) = get_autostart_path_macos() {
            return path.exists();
        }
    }
    #[cfg(target_os = "windows")]
    {
        let output = std::process::Command::new("reg")
            .args(&[
                "query",
                "HKCU\\Software\\Microsoft\\Windows\\CurrentVersion\\Run",
                "/v",
                "LunarisAgent",
            ])
            .output();
        if let Ok(out) = output {
            return out.status.success();
        }
    }
    false
}

pub fn set_autostart_enabled_impl(enabled: bool) {
    let exe_path = match std::env::current_exe() {
        Ok(path) => path.to_string_lossy().to_string(),
        Err(_) => return,
    };

    if enabled {
        #[cfg(target_os = "linux")]
        {
            if let Some(path) = get_autostart_path_linux() {
                let content = format!(
                    "[Desktop Entry]\nType=Application\nName=Lunaris Agent\nExec=\"{}\" --minimized\nIcon=lunaris-agent\nX-GNOME-Autostart-enabled=true\n",
                    exe_path
                );
                let _ = std::fs::write(path, content);
            }
        }
        #[cfg(target_os = "macos")]
        {
            if let Some(path) = get_autostart_path_macos() {
                let content = format!(
                    r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>com.lunaris.agent</string>
    <key>ProgramArguments</key>
    <array>
        <string>{}</string>
        <string>--minimized</string>
    </array>
    <key>RunAtLoad</key>
    <true/>
</dict>
</plist>"#,
                    exe_path
                );
                let _ = std::fs::write(path, content);
            }
        }
        #[cfg(target_os = "windows")]
        {
            let val = format!("\"{}\" --minimized", exe_path);
            let _ = std::process::Command::new("reg")
                .args(&[
                    "add",
                    "HKCU\\Software\\Microsoft\\Windows\\CurrentVersion\\Run",
                    "/v",
                    "LunarisAgent",
                    "/t",
                    "REG_SZ",
                    "/d",
                    &val,
                    "/f",
                ])
                .output();
        }
    } else {
        #[cfg(target_os = "linux")]
        {
            if let Some(path) = get_autostart_path_linux() {
                let _ = std::fs::remove_file(path);
            }
        }
        #[cfg(target_os = "macos")]
        {
            if let Some(path) = get_autostart_path_macos() {
                let _ = std::fs::remove_file(path);
            }
        }
        #[cfg(target_os = "windows")]
        {
            let _ = std::process::Command::new("reg")
                .args(&[
                    "delete",
                    "HKCU\\Software\\Microsoft\\Windows\\CurrentVersion\\Run",
                    "/v",
                    "LunarisAgent",
                    "/f",
                ])
                .output();
        }
    }
}
