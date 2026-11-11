use serde::{Deserialize, Serialize};
use std::fs;

fn default_server_url() -> String {
    "ws://127.0.0.1:8090".to_string()
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
    /// Connector name of the forced high-refresh "virtual" display (default "DP-2").
    #[serde(default)]
    pub virtual_display_output: Option<String>,
    /// Direct connections: clients connect straight to this agent at
    /// `ws://<address>:<direct_port>` with `direct_password`, without a
    /// Lunaris server (LAN, VPN or a forwarded port).
    #[serde(default)]
    pub direct_enabled: bool,
    #[serde(default = "default_direct_port")]
    pub direct_port: u16,
    #[serde(default)]
    pub direct_password: String,
}

pub fn default_direct_port() -> u16 {
    8091
}

/// A new access password like "k7wq-3mzp-x9tr" (unambiguous lowercase letters
/// and digits, ~62 bits of entropy).
pub fn generate_direct_password() -> String {
    const ALPHABET: &[u8] = b"abcdefghjkmnpqrstuvwxyz23456789";
    let bytes = *uuid::Uuid::new_v4().as_bytes();
    let chars: Vec<char> = bytes
        .iter()
        .take(12)
        .map(|b| ALPHABET[*b as usize % ALPHABET.len()] as char)
        .collect();
    format!(
        "{}-{}-{}",
        chars[0..4].iter().collect::<String>(),
        chars[4..8].iter().collect::<String>(),
        chars[8..12].iter().collect::<String>()
    )
}

/// Gives the config a direct-access password when it has none; returns
/// whether it changed (so the caller can save it).
pub fn ensure_direct_password(config: &mut AgentConfig) -> bool {
    if config.direct_password.trim().is_empty() {
        config.direct_password = generate_direct_password();
        true
    } else {
        false
    }
}



static CONFIG_PATH: std::sync::OnceLock<String> = std::sync::OnceLock::new();

/// Sets the config file path for this process (from `--config`); call once at startup.
/// A fresh configuration for this computer: a new id, no server and a new
/// direct-access password. Direct connections stay off until the user turns
/// remote control on.
pub fn new_config() -> AgentConfig {
    let mut config: AgentConfig = serde_json::from_str("{}").expect("every field has a default");
    config.client_unique_id = uuid::Uuid::new_v4().to_string().to_uppercase();
    config.server_url = String::new();
    ensure_direct_password(&mut config);
    config
}

pub fn set_config_path(path: Option<String>) {
    let _ = CONFIG_PATH.set(path.unwrap_or_else(default_config_path));
}

/// Path of agent_config.json used by both the CLI and the GUI.
pub fn config_path() -> &'static str {
    CONFIG_PATH.get_or_init(default_config_path)
}

/// `agent_config.json` in the working directory when one exists there (dev
/// checkouts, older setups); otherwise the per-user config directory
/// (`%APPDATA%\lunaris-agent`, `~/.config/lunaris-agent`, ...), since the
/// install location (e.g. Program Files) is not writable.
fn default_config_path() -> String {
    const NAME: &str = "agent_config.json";
    if std::path::Path::new(NAME).exists() {
        return NAME.to_string();
    }
    match dirs::config_dir() {
        Some(dir) => {
            let dir = dir.join("lunaris-agent");
            let _ = fs::create_dir_all(&dir);
            dir.join(NAME).to_string_lossy().into_owned()
        }
        None => NAME.to_string(),
    }
}

pub fn load_config(path: &str) -> Result<AgentConfig, anyhow::Error> {
    let content = fs::read_to_string(path)?;
    let config: AgentConfig = serde_json::from_str(&content)?;
    Ok(config)
}

pub fn save_config(config: &AgentConfig, path: &str) -> Result<(), anyhow::Error> {
    let content = serde_json::to_string_pretty(config)?;
    fs::write(path, content)?;
    // The file holds the server token and the client key: owner-only.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(path, fs::Permissions::from_mode(0o600));
    }
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
        let output = reg_command()
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

/// Runs `reg.exe` without flashing a console window from the GUI app.
#[cfg(target_os = "windows")]
fn reg_command() -> std::process::Command {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let mut cmd = reg_command();
    cmd.creation_flags(CREATE_NO_WINDOW);
    cmd
}

pub fn set_autostart_enabled_impl(enabled: bool) {
    // Inside an AppImage current_exe() points into a temporary mount; the
    // AppImage file itself is what has to be started at login.
    let exe_path = match std::env::var("APPIMAGE").ok().filter(|p| !p.is_empty()) {
        Some(appimage) => appimage,
        None => match std::env::current_exe() {
            Ok(path) => path.to_string_lossy().to_string(),
            Err(_) => return,
        },
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
            let _ = reg_command()
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
            let _ = reg_command()
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
