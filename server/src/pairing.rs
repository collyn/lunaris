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
use std::sync::Arc;
use std::time::Duration;
use tracing::{error, info, warn};
use uuid::Uuid;

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct AgentConfig {
    pub client_unique_id: String,
    pub client_private_key: String,
    pub client_certificate: String,
    pub server_certificate: String,
    pub server_codec_mode_support: u32,
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

pub async fn approve_sunshine_pin(
    ip: &str,
    username: &str,
    password: &str,
    pin: &str,
    client_name: &str,
) -> Result<(), anyhow::Error> {
    let url = format!("https://{}:47990/api/pin", ip);

    // Create an HTTP client that accepts self-signed certificates
    let client = reqwest::Client::builder()
        .danger_accept_invalid_certs(true)
        .build()?;

    let payload = serde_json::json!({
        "pin": pin,
        "name": client_name,
    });

    info!("Submitting PIN {} to Sunshine API at {}", pin, url);

    let res = client
        .post(&url)
        .basic_auth(username, Some(password))
        .json(&payload)
        .send()
        .await?;

    if res.status().is_success() {
        info!("Successfully approved pairing PIN on Sunshine host");
        Ok(())
    } else {
        let status = res.status();
        let body = res.text().await.unwrap_or_default();
        error!(
            "Sunshine PIN approval failed with status {}: {}",
            status, body
        );
        Err(anyhow::anyhow!("Sunshine API error: {} - {}", status, body))
    }
}

pub async fn perform_pairing(
    ip: &str,
    port: u16,
    sunshine_username: &str,
    sunshine_password: &str,
    client_name: &str,
) -> Result<AgentConfig, anyhow::Error> {
    // Generate a random 4-digit PIN
    let uuid = Uuid::new_v4();
    let bytes = uuid.as_bytes();
    let pin_val = (((bytes[0] as u32) << 8 | (bytes[1] as u32)) % 10000) as u32;
    let pin_str = format!("{:04}", pin_val);

    info!("Generated pairing PIN: {}", pin_str);

    // Parse PIN
    let pin_chars: Vec<char> = pin_str.chars().collect();
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

    // Spawn Sunshine PIN approval in a background task.
    // It waits a brief moment to ensure Sunshine has received/processed the connection start before we submit the PIN.
    let ip_str = ip.to_string();
    let sunshine_username_str = sunshine_username.to_string();
    let sunshine_password_str = sunshine_password.to_string();
    let pin_str_clone = pin_str.clone();
    let client_name_str = client_name.to_string();

    let approval_handle = tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(500)).await;
        approve_sunshine_pin(
            &ip_str,
            &sunshine_username_str,
            &sunshine_password_str,
            &pin_str_clone,
            &client_name_str,
        )
        .await
    });

    // Perform Moonlight pairing handshake in the main task (which blocks until PIN is approved)
    host.pair(
        &client_identifier,
        &client_secret,
        client_name.to_string(),
        pin,
        crypto_provider.clone(),
    )
    .await?;

    // Wait for the approval task to finalize and check if it succeeded
    match approval_handle.await {
        Ok(Ok(_)) => {
            info!("Sunshine PIN auto-approval succeeded!");
        }
        Ok(Err(e)) => {
            return Err(anyhow::anyhow!("Sunshine PIN auto-approval failed: {}", e));
        }
        Err(e) => {
            return Err(anyhow::anyhow!(
                "Sunshine approval task panicked or was aborted: {}",
                e
            ));
        }
    }

    let (_, _, server_identifier) = host
        .identity()
        .await
        .ok_or_else(|| anyhow::anyhow!("No identity found"))?;

    host.set_identity(
        client_identifier.clone(),
        client_secret.clone(),
        server_identifier.clone(),
    )
    .await?;

    let server_codec_mode_support = match host.server_codec_mode_support().await {
        Ok(support) => support.bits(),
        Err(e) => {
            warn!(
                "Failed to query server codec support during pairing: {:?}",
                e
            );
            0
        }
    };

    // Convert to PEM strings
    let client_private_key = client_secret.to_pem().to_string();
    let client_certificate = client_identifier.to_pem().to_string();
    let server_certificate = server_identifier.to_pem().to_string();

    Ok(AgentConfig {
        client_unique_id,
        client_private_key,
        client_certificate,
        server_certificate,
        server_codec_mode_support,
    })
}
