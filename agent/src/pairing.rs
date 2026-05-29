use moonlight_common::{
    crypto::openssl::OpenSSLCryptoBackend,
    high::tokio::MoonlightHost,
    http::{
        client::tokio_hyper::TokioHyperClient,
        pair::{PairPin, PairingCryptoBackend},
    },
};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;
use std::sync::Arc;
use uuid::Uuid;

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

pub fn get_sunshine_dir() -> Option<PathBuf> {
    #[cfg(target_os = "windows")]
    {
        if let Ok(pg_files) = std::env::var("ProgramFiles") {
            let p = PathBuf::from(pg_files).join("Sunshine").join("config");
            if p.exists() {
                return Some(p);
            }
        }
    }

    dirs::config_dir().map(|mut p| {
        p.push("sunshine");
        p
    })
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

pub fn auto_pair_local_sunshine(
    client_name: &str,
    config_path: &str,
    cli_server_url: Option<String>,
) -> Result<AgentConfig, anyhow::Error> {
    let sunshine_dir = get_sunshine_dir()
        .ok_or_else(|| anyhow::anyhow!("Could not locate configuration directory"))?;
    if !sunshine_dir.exists() {
        fs::create_dir_all(&sunshine_dir)?;
    }

    // 1. Load or Generate Agent config (keys/cert)
    let mut config = if let Ok(mut existing_config) = load_config(config_path) {
        if existing_config.client_private_key.is_empty()
            || existing_config.client_certificate.is_empty()
        {
            let crypto_provider = Arc::new(OpenSSLCryptoBackend);
            let (client_identifier, client_secret) = crypto_provider.generate_client_identity()?;
            existing_config.client_private_key = client_secret.to_pem().to_string();
            existing_config.client_certificate = client_identifier.to_pem().to_string();
        }
        if existing_config.client_unique_id.is_empty() {
            existing_config.client_unique_id = Uuid::new_v4().to_string().to_uppercase();
        }
        existing_config
    } else {
        let client_unique_id = Uuid::new_v4().to_string().to_uppercase();
        let crypto_provider = Arc::new(OpenSSLCryptoBackend);
        let (client_identifier, client_secret) = crypto_provider.generate_client_identity()?;

        let client_private_key = client_secret.to_pem().to_string();
        let client_certificate = client_identifier.to_pem().to_string();

        AgentConfig {
            client_unique_id,
            client_private_key,
            client_certificate,
            server_certificate: "".to_string(), // Will be updated later
            server_url: cli_server_url.clone().unwrap_or_else(default_server_url),
            server_token: "".to_string(),
            webtransport_port: default_webtransport_port(),
            autostart: false,
            close_to_tray: false,
        }
    };

    // If server_url is explicitly passed via CLI, update the config
    if let Some(url) = cli_server_url {
        config.server_url = url;
    }

    // 2. Load and update sunshine_state.json
    let state_file_path = sunshine_dir.join("sunshine_state.json");
    let mut state_json: serde_json::Value = if state_file_path.exists() {
        let content = fs::read_to_string(&state_file_path)?;
        serde_json::from_str(&content).unwrap_or(serde_json::json!({}))
    } else {
        serde_json::json!({})
    };

    if state_json["root"].is_null() {
        state_json["root"] = serde_json::json!({
            "uniqueid": Uuid::new_v4().to_string().to_uppercase(),
            "named_devices": []
        });
    } else if state_json["root"]["named_devices"].is_null() {
        state_json["root"]["named_devices"] = serde_json::json!([]);
    }

    let devices = state_json["root"]["named_devices"]
        .as_array_mut()
        .ok_or_else(|| anyhow::anyhow!("named_devices is not an array"))?;

    let mut found_idx = None;
    for (idx, dev) in devices.iter().enumerate() {
        if let Some(uuid_str) = dev["uuid"].as_str() {
            if uuid_str.to_uppercase() == config.client_unique_id.to_uppercase() {
                found_idx = Some(idx);
                break;
            }
        }
    }

    let mut needs_write = false;
    if let Some(idx) = found_idx {
        let dev = &mut devices[idx];
        if dev["cert"].as_str() != Some(&config.client_certificate)
            || dev["name"].as_str() != Some(client_name)
        {
            dev["cert"] = serde_json::json!(config.client_certificate);
            dev["name"] = serde_json::json!(client_name);
            needs_write = true;
        }
    } else {
        devices.push(serde_json::json!({
            "name": client_name,
            "cert": config.client_certificate,
            "uuid": config.client_unique_id.to_uppercase(),
            "enabled": "true"
        }));
        needs_write = true;
    }

    if needs_write {
        let updated_state = serde_json::to_string_pretty(&state_json)?;
        if let Err(e) = fs::write(&state_file_path, updated_state) {
            eprintln!("Warning: Failed to write to Sunshine state file at {:?}: {:?}. You may need to run as Administrator or pair manually.", state_file_path, e);
        }
    }

    // 3. Try to read server_certificate (cacert.pem) if available
    let mut updated_config = config.clone();
    let server_cert_path = sunshine_dir.join("credentials").join("cacert.pem");
    if server_cert_path.exists() {
        if let Ok(cert_pem) = fs::read_to_string(&server_cert_path) {
            updated_config.server_certificate = cert_pem;
        }
    }

    save_config(&updated_config, config_path)?;
    Ok(updated_config)
}

pub async fn perform_pairing(
    ip: &str,
    port: u16,
    pin_str: &str,
    client_name: &str,
    cli_server_url: Option<String>,
) -> Result<AgentConfig, anyhow::Error> {
    // Parse PIN
    let pin_chars: Vec<char> = pin_str.chars().collect();
    if pin_chars.len() != 4 {
        return Err(anyhow::anyhow!("PIN must be exactly 4 digits"));
    }
    let n1 = pin_chars[0]
        .to_digit(10)
        .ok_or_else(|| anyhow::anyhow!("Invalid PIN digit"))? as u8;
    let n2 = pin_chars[1]
        .to_digit(10)
        .ok_or_else(|| anyhow::anyhow!("Invalid PIN digit"))? as u8;
    let n3 = pin_chars[2]
        .to_digit(10)
        .ok_or_else(|| anyhow::anyhow!("Invalid PIN digit"))? as u8;
    let n4 = pin_chars[3]
        .to_digit(10)
        .ok_or_else(|| anyhow::anyhow!("Invalid PIN digit"))? as u8;

    let pin = PairPin::new(n1, n2, n3, n4)
        .ok_or_else(|| anyhow::anyhow!("Invalid PIN (digits must be 0-9)"))?;

    let client_unique_id = Uuid::new_v4().to_string();
    let crypto_provider = Arc::new(OpenSSLCryptoBackend);

    // Generate client identity (private key + cert)
    let (client_identifier, client_secret) = crypto_provider.generate_client_identity()?;

    // Create MoonlightHost client
    let host = MoonlightHost::<TokioHyperClient>::new(
        ip.to_string(),
        port,
        Some(client_unique_id.clone()),
    )?;

    // Perform pair handshake
    host.pair(
        &client_identifier,
        &client_secret,
        client_name.to_string(),
        pin,
        crypto_provider.clone(),
    )
    .await?;

    let (_, _, server_identifier) = host
        .identity()
        .await
        .ok_or_else(|| anyhow::anyhow!("No identity found"))?;

    // Convert to PEM strings
    let client_private_key = client_secret.to_pem().to_string();
    let client_certificate = client_identifier.to_pem().to_string();
    let server_certificate = server_identifier.to_pem().to_string();

    Ok(AgentConfig {
        client_unique_id,
        client_private_key,
        client_certificate,
        server_certificate,
        server_url: cli_server_url.unwrap_or_else(default_server_url),
        server_token: "".to_string(),
        webtransport_port: default_webtransport_port(),
        autostart: false,
        close_to_tray: false,
    })
}

use moonlight_common::http::{ClientIdentifier, ClientSecret, ServerIdentifier};

pub async fn query_sunshine_codec_support(
    ip: &str,
    port: u16,
    config: &AgentConfig,
) -> Result<u32, anyhow::Error> {
    if config.client_certificate.is_empty() {
        return Err(anyhow::anyhow!("Client certificate is empty"));
    }
    if config.client_private_key.is_empty() {
        return Err(anyhow::anyhow!("Client private key is empty"));
    }
    if config.server_certificate.is_empty() {
        return Err(anyhow::anyhow!("Server certificate is empty"));
    }

    let host = MoonlightHost::<TokioHyperClient>::new(
        ip.to_string(),
        port,
        Some(config.client_unique_id.clone()),
    )?;

    let client_cert_pem = pem::parse(&config.client_certificate)?;
    let client_key_pem = pem::parse(&config.client_private_key)?;
    let server_cert_pem = pem::parse(&config.server_certificate)?;

    host.set_identity(
        ClientIdentifier::from_pem(client_cert_pem),
        ClientSecret::from_pem(client_key_pem),
        ServerIdentifier::from_pem(server_cert_pem),
    )
    .await?;

    let support = host.server_codec_mode_support().await?;
    Ok(support.bits())
}

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
