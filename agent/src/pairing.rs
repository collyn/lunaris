use std::sync::Arc;
use std::fs;
use std::path::PathBuf;
use serde::{Serialize, Deserialize};
use moonlight_common::{
    crypto::openssl::OpenSSLCryptoBackend,
    high::tokio::MoonlightHost,
    http::{
        client::tokio_hyper::TokioHyperClient,
        pair::{PairPin, PairingCryptoBackend},
    },
};
use uuid::Uuid;

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct AgentConfig {
    pub client_unique_id: String,
    pub client_private_key: String,
    pub client_certificate: String,
    pub server_certificate: String,
}

pub fn get_sunshine_dir() -> Option<PathBuf> {
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

pub fn auto_pair_local_sunshine(
    client_name: &str,
    config_path: &str,
) -> Result<AgentConfig, anyhow::Error> {
    let sunshine_dir = get_sunshine_dir().ok_or_else(|| anyhow::anyhow!("Could not locate configuration directory"))?;
    if !sunshine_dir.exists() {
        fs::create_dir_all(&sunshine_dir)?;
    }

    // 1. Load or Generate Agent config (keys/cert)
    let config = if let Ok(existing_config) = load_config(config_path) {
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
        }
    };

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

    let mut found = false;
    for dev in devices.iter() {
        if let Some(uuid_str) = dev["uuid"].as_str() {
            if uuid_str.to_uppercase() == config.client_unique_id.to_uppercase() {
                found = true;
                break;
            }
        }
    }

    if !found {
        devices.push(serde_json::json!({
            "name": client_name,
            "cert": config.client_certificate,
            "uuid": config.client_unique_id.to_uppercase(),
            "enabled": "true"
        }));
        
        let updated_state = serde_json::to_string_pretty(&state_json)?;
        fs::write(&state_file_path, updated_state)?;
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
) -> Result<AgentConfig, anyhow::Error> {
    // Parse PIN
    let pin_chars: Vec<char> = pin_str.chars().collect();
    if pin_chars.len() != 4 {
        return Err(anyhow::anyhow!("PIN must be exactly 4 digits"));
    }
    let n1 = pin_chars[0].to_digit(10).ok_or_else(|| anyhow::anyhow!("Invalid PIN digit"))? as u8;
    let n2 = pin_chars[1].to_digit(10).ok_or_else(|| anyhow::anyhow!("Invalid PIN digit"))? as u8;
    let n3 = pin_chars[2].to_digit(10).ok_or_else(|| anyhow::anyhow!("Invalid PIN digit"))? as u8;
    let n4 = pin_chars[3].to_digit(10).ok_or_else(|| anyhow::anyhow!("Invalid PIN digit"))? as u8;

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

    let (_, _, server_identifier) = host.identity().await.ok_or_else(|| anyhow::anyhow!("No identity found"))?;

    // Convert to PEM strings
    let client_private_key = client_secret.to_pem().to_string();
    let client_certificate = client_identifier.to_pem().to_string();
    let server_certificate = server_identifier.to_pem().to_string();

    Ok(AgentConfig {
        client_unique_id,
        client_private_key,
        client_certificate,
        server_certificate,
    })
}

use moonlight_common::http::{ClientIdentifier, ClientSecret, ServerIdentifier};

pub async fn query_sunshine_codec_support(
    ip: &str,
    port: u16,
    config: &AgentConfig,
) -> Result<u32, anyhow::Error> {
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

