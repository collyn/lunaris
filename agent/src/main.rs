#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::sync::Arc;
use tokio::sync::{mpsc, Mutex};
use futures_util::{StreamExt, SinkExt};
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::Message as WsMessage;
use clap::Parser;
use tracing::{info, error, warn, debug};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

use common::{
    AgentMessage, ServerToAgentMessage, SignalingMessage,
    RtcSdpType, RtcSessionDescription,
};
use webrtc::peer_connection::sdp::session_description::RTCSessionDescription;
use webrtc::ice_transport::ice_candidate::RTCIceCandidateInit;

mod buffer;
mod input;
mod pairing;
mod video;
mod bridge;

#[cfg(feature = "gui")]
pub mod gui;

pub static CONNECTED_TO_SERVER: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
pub static SUNSHINE_PID: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
pub static AGENT_ACTIVE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
pub static LAST_ERROR: std::sync::Mutex<Option<String>> = std::sync::Mutex::new(None);

use crate::pairing::{save_config, perform_pairing, query_sunshine_codec_support, auto_pair_local_sunshine, AgentConfig};
use crate::bridge::{setup_bridge_session, BridgeSession};

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    #[arg(long, default_value = "127.0.0.1")]
    host_ip: String,

    #[arg(long, default_value_t = 47989)]
    host_port: u16,

    #[arg(long)]
    pair: bool,

    #[arg(long)]
    pin: Option<String>,

    #[arg(long)]
    server: Option<String>,

    #[arg(long)]
    name: Option<String>,

    #[arg(long)]
    no_auto_start_sunshine: bool,

    #[arg(long, default_value = "sunshine")]
    sunshine_path: String,

    #[arg(long)]
    cli: bool,

    #[arg(long, default_value = "agent_config.json")]
    config: String,

    #[arg(long)]
    import_config: Option<String>,
}

#[allow(dead_code)]
const LINUX_SUNSHINE_URL: &str = "https://github.com/LizardByte/Sunshine/releases/latest/download/sunshine.AppImage";
#[allow(dead_code)]
const WINDOWS_SUNSHINE_URL: &str = "https://github.com/LizardByte/Sunshine/releases/latest/download/Sunshine-Windows-AMD64-portable.zip";

#[allow(dead_code)]
fn which_sunshine() -> Result<(), &'static str> {
    let check_cmd = if cfg!(target_os = "windows") { "where" } else { "which" };
    let output = std::process::Command::new(check_cmd)
        .arg("sunshine")
        .output();
    match output {
        Ok(out) if out.status.success() => Ok(()),
        _ => Err("sunshine not found in PATH"),
    }
}

#[allow(dead_code)]
async fn download_file(url: &str, dest: &std::path::Path) -> Result<(), anyhow::Error> {
    warn!("Downloading {} to {:?}", url, dest);
    let client = reqwest::Client::builder()
        .user_agent("Mozilla/5.0")
        .connect_timeout(std::time::Duration::from_secs(30))
        .timeout(std::time::Duration::from_secs(300))
        .build()?;
    let response = client.get(url).send().await?;
    if !response.status().is_success() {
        return Err(anyhow::anyhow!("Failed to download: HTTP {}", response.status()));
    }
    
    let mut file = tokio::fs::File::create(dest).await?;
    let mut stream = response.bytes_stream();
    use futures_util::StreamExt;
    use tokio::io::AsyncWriteExt;
    
    while let Some(chunk_result) = stream.next().await {
        let chunk = chunk_result?;
        file.write_all(&chunk).await?;
    }
    
    warn!("Download complete!");
    Ok(())
}

#[allow(dead_code)]
fn unzip_file(zip_path: &std::path::Path, extract_to: &std::path::Path) -> Result<(), anyhow::Error> {
    warn!("Extracting zip {:?} to {:?}", zip_path, extract_to);
    let file = std::fs::File::open(zip_path)?;
    let mut archive = zip::ZipArchive::new(file)?;
    
    std::fs::create_dir_all(extract_to)?;
    
    for i in 0..archive.len() {
        let mut file = archive.by_index(i)?;
        let outpath = match file.enclosed_name() {
            Some(path) => extract_to.join(path),
            None => continue,
        };

        if file.name().ends_with('/') {
            std::fs::create_dir_all(&outpath)?;
        } else {
            if let Some(p) = outpath.parent() {
                if !p.exists() {
                    std::fs::create_dir_all(&p)?;
                }
            }
            let mut outfile = std::fs::File::create(&outpath)?;
            std::io::copy(&mut file, &mut outfile)?;
        }
    }
    warn!("Extraction complete!");
    Ok(())
}

#[cfg(target_os = "linux")]
fn make_executable(path: &std::path::Path) -> Result<(), anyhow::Error> {
    use std::os::unix::fs::PermissionsExt;
    let mut perms = std::fs::metadata(path)?.permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(path, perms)?;
    Ok(())
}

async fn prepare_sunshine() -> Result<std::path::PathBuf, anyhow::Error> {
    let bin_dir = std::path::PathBuf::from("./bin");
    if !bin_dir.exists() {
        tokio::fs::create_dir_all(&bin_dir).await?;
    }

    #[cfg(target_os = "linux")]
    {
        let appimage_path = bin_dir.join("sunshine.AppImage");
        if !appimage_path.exists() {
            warn!("Sunshine AppImage not found. Downloading latest LizardByte Sunshine (approx. 170MB)...");
            download_file(LINUX_SUNSHINE_URL, &appimage_path).await?;
            make_executable(&appimage_path)?;
        }
        return Ok(appimage_path);
    }

    #[cfg(target_os = "windows")]
    {
        let exe_path = bin_dir.join("sunshine").join("sunshine.exe");
        if !exe_path.exists() {
            let zip_path = bin_dir.join("sunshine.zip");
            warn!("Sunshine Windows portable binary not found. Downloading latest LizardByte Sunshine (approx. 150MB)...");
            download_file(WINDOWS_SUNSHINE_URL, &zip_path).await?;
            unzip_file(&zip_path, &bin_dir.join("sunshine"))?;
            let _ = std::fs::remove_file(zip_path);
        }
        return Ok(exe_path);
    }

    #[cfg(target_os = "macos")]
    {
        if which_sunshine().is_ok() {
            info!("Sunshine is available in system PATH");
            return Ok(std::path::PathBuf::from("sunshine"));
        }
        return Err(anyhow::anyhow!(
            "Sunshine auto-download is not supported on macOS. Please run 'brew install sunshine' first."
        ));
    }

    #[cfg(not(any(target_os = "linux", target_os = "windows", target_os = "macos")))]
    {
        if which_sunshine().is_ok() {
            info!("Sunshine is available in system PATH");
            return Ok(std::path::PathBuf::from("sunshine"));
        }
        return Err(anyhow::anyhow!("Unsupported operating system"));
    }
}

fn read_sunshine_conf() -> Result<String, anyhow::Error> {
    let sunshine_dir = crate::pairing::get_sunshine_dir()
        .ok_or_else(|| anyhow::anyhow!("Could not locate configuration directory"))?;
    let conf_path = sunshine_dir.join("sunshine.conf");
    if !conf_path.exists() {
        return Ok("{}".to_string());
    }
    let content = std::fs::read_to_string(&conf_path)?;
    let mut map = serde_json::Map::new();
    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some(pos) = line.find('=') {
            let key = line[..pos].trim().to_string();
            let value = line[pos + 1..].trim().to_string();
            map.insert(key, serde_json::Value::String(value));
        }
    }
    let json = serde_json::to_string(&serde_json::Value::Object(map))?;
    Ok(json)
}

fn write_sunshine_conf(config_json: &str) -> Result<(), anyhow::Error> {
    let sunshine_dir = crate::pairing::get_sunshine_dir()
        .ok_or_else(|| anyhow::anyhow!("Could not locate configuration directory"))?;
    let conf_path = sunshine_dir.join("sunshine.conf");
    
    let new_config: serde_json::Value = serde_json::from_str(config_json)?;
    let new_map = new_config.as_object().ok_or_else(|| anyhow::anyhow!("Config is not a JSON object"))?;

    let mut lines: Vec<String> = if conf_path.exists() {
        let content = std::fs::read_to_string(&conf_path)?;
        content.lines().map(|s| s.to_string()).collect()
    } else {
        Vec::new()
    };

    let mut updated_keys = std::collections::HashSet::new();

    for line in lines.iter_mut() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        if let Some(pos) = trimmed.find('=') {
            let key = trimmed[..pos].trim();
            if let Some(new_val) = new_map.get(key) {
                let key_owned = key.to_string();
                let new_val_str = match new_val {
                    serde_json::Value::String(s) => s.clone(),
                    other => other.to_string(),
                };
                *line = format!("{} = {}", key_owned, new_val_str);
                updated_keys.insert(key_owned);
            } else {
                // Key was removed, comment it out to revert to default
                *line = format!("# {}", trimmed);
            }
        }
    }

    for (key, val) in new_map {
        if !updated_keys.contains(key) {
            let val_str = match val {
                serde_json::Value::String(s) => s.clone(),
                other => other.to_string(),
            };
            lines.push(format!("{} = {}", key, val_str));
        }
    }

    let new_content = lines.join("\n");
    std::fs::write(&conf_path, new_content)?;
    Ok(())
}

fn check_and_start_sunshine(path: &str, ip: &str, port: u16) -> Option<tokio::process::Child> {
    // Check if the streaming port is already open
    if std::net::TcpStream::connect(format!("{}:{}", ip, port)).is_ok() {
        info!("Sunshine is already running on {}:{}", ip, port);
        return None;
    }

    info!("Sunshine is not running. Spawning local Sunshine process from path: {}", path);
    let mut cmd = tokio::process::Command::new(path);
    cmd.kill_on_drop(true);

    #[cfg(unix)]
    {
        unsafe {
            cmd.pre_exec(|| {
                libc::setpgid(0, 0);
                Ok(())
            });
        }
    }

    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x08000000); // CREATE_NO_WINDOW
    }

    match cmd.spawn() {
        Ok(child) => {
            info!("Local Sunshine process spawned successfully (PID: {:?})", child.id());
            Some(child)
        }
        Err(e) => {
            error!("Failed to spawn local Sunshine process: {:?}", e);
            None
        }
    }
}

#[cfg(all(target_os = "macos", target_arch = "x86_64"))]
#[no_mangle]
#[allow(non_upper_case_globals)]
pub static mut __cpu_model: [u32; 4] = [0; 4];

#[cfg(all(target_os = "macos", target_arch = "x86_64"))]
#[no_mangle]
pub extern "C" fn __cpu_indicator_init() -> i32 {
    0
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let args = Args::parse();

    // If import_config is specified, we perform the import first
    if let Some(ref import_path) = args.import_config {
        let _ = tracing_subscriber::registry()
            .with(tracing_subscriber::EnvFilter::new(
                std::env::var("RUST_LOG").unwrap_or_else(|_| "info,agent=debug".into()),
            ))
            .with(tracing_subscriber::fmt::layer())
            .try_init();

        info!("Importing configuration from {} into {}...", import_path, args.config);
        if let Err(e) = crate::pairing::import_config_file(import_path, &args.config) {
            error!("Failed to import configuration: {:?}", e);
            std::process::exit(1);
        }
        info!("Configuration imported and merged successfully!");
    }

    #[cfg(feature = "gui")]
    {
        if !args.cli && !args.pair {
            // Init tracing logger with custom MakeWriter to stream logs to GUI
            let _ = tracing_subscriber::registry()
                .with(tracing_subscriber::EnvFilter::new(
                    std::env::var("RUST_LOG").unwrap_or_else(|_| {
                        if cfg!(debug_assertions) {
                            "info,agent=debug"
                        } else {
                            "warn"
                        }
                    }.into()),
                ))
                .with(tracing_subscriber::fmt::layer()
                    .with_writer(gui::ChannelMakeWriter)
                    .with_ansi(false))
                .try_init();

            info!("Launching Lunaris Agent in Desktop GUI mode...");
            gui::run_gui();
            return Ok(());
        }
    }

    // CLI mode execution
    // Init standard tracing logger
    let _ = tracing_subscriber::registry()
        .with(tracing_subscriber::EnvFilter::new(
            std::env::var("RUST_LOG").unwrap_or_else(|_| {
                if cfg!(debug_assertions) {
                    "info,agent=debug"
                } else {
                    "warn"
                }
            }.into()),
        ))
        .with(tracing_subscriber::fmt::layer())
        .try_init();

    info!("Launching Lunaris Agent in CLI mode...");

    let name = args.name.clone().unwrap_or_else(|| {
        hostname::get()
            .map(|h| h.to_string_lossy().into_owned())
            .unwrap_or_else(|_| "LunarisHost".to_string())
    });

    if args.pair {
        let pin = args.pin.ok_or_else(|| anyhow::anyhow!("--pin is required when pairing"))?;
        info!("Starting Sunshine pairing handshake with {}:{} using PIN {}", args.host_ip, args.host_port, pin);
        let config = perform_pairing(&args.host_ip, args.host_port, &pin, &name, args.server.clone()).await?;
        save_config(&config, &args.config)?;
        info!("Pairing completed successfully! Config saved to {}", args.config);
        return Ok(());
    }

    // Auto pairing / key generation before Sunshine starts
    info!("Ensuring Agent credentials are paired with local Sunshine configuration...");
    let config = match auto_pair_local_sunshine(&name, &args.config, args.server.clone()) {
        Ok(c) => c,
        Err(e) => {
            error!("Failed to auto-pair local Sunshine config: {:?}", e);
            std::process::exit(1);
        }
    };

    run_agent_loop(
        config,
        name,
        args.host_ip,
        args.host_port,
        args.no_auto_start_sunshine,
        args.sunshine_path,
        args.config,
    )
    .await?;

    Ok(())
}

async fn kill_running_sunshine() {
    info!("Attempting to kill any running Sunshine instances...");
    #[cfg(target_os = "windows")]
    {
        let _ = tokio::process::Command::new("taskkill")
            .args(&["/F", "/IM", "sunshine.exe"])
            .output()
            .await;
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = tokio::process::Command::new("pkill")
            .args(&["-9", "sunshine"])
            .output()
            .await;
    }
    // Give it a moment to release ports/resources
    tokio::time::sleep(tokio::time::Duration::from_millis(1000)).await;
}

pub async fn run_agent_loop(
    mut config: AgentConfig,
    name: String,
    host_ip: String,
    host_port: u16,
    no_auto_start_sunshine: bool,
    sunshine_path: String,
    config_path: String,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    // Reset status
    CONNECTED_TO_SERVER.store(false, std::sync::atomic::Ordering::SeqCst);
    SUNSHINE_PID.store(0, std::sync::atomic::Ordering::SeqCst);

    // Check if Sunshine is already running and if we are authorized
    let mut port_open = std::net::TcpStream::connect(format!("{}:{}", host_ip, host_port)).is_ok();
    let mut is_authorized = false;
    if port_open {
        if !config.server_certificate.is_empty() {
            info!("Sunshine is running. Checking authorization...");
            match query_sunshine_codec_support(&host_ip, host_port, &config).await {
                Ok(_) => {
                    info!("Sunshine is already authorized. No restart needed.");
                    is_authorized = true;
                }
                Err(e) => {
                    warn!("Sunshine is running but unauthorized: {:?}", e);
                }
            }
        } else {
            info!("Sunshine is running, but we do not have the server certificate yet.");
        }
    }

    if port_open && !is_authorized && !no_auto_start_sunshine {
        info!("Killing unauthorized Sunshine instance to re-apply configuration...");
        kill_running_sunshine().await;
        port_open = false;
    }

    // Auto start Sunshine if needed
    let mut local_sunshine_path: Option<String> = None;
    let mut _sunshine_child = None;

    if !is_authorized && !no_auto_start_sunshine {
        // Since we are starting/restarting Sunshine, we must ensure the auto-pair runs
        // when Sunshine is not running, so that we can write to sunshine_state.json safely.
        info!("Ensuring Agent credentials are paired with local Sunshine configuration...");
        config = match auto_pair_local_sunshine(&name, &config_path, Some(config.server_url.clone())) {
            Ok(c) => c,
            Err(e) => {
                error!("Failed to auto-pair local Sunshine config: {:?}", e);
                return Err(e.into());
            }
        };

        // Prepare/Download portable Sunshine if necessary
        let path_to_run = match prepare_sunshine().await {
            Ok(p) => p.to_string_lossy().into_owned(),
            Err(e) => {
                warn!("Could not automatically prepare Sunshine binary: {:?}. Fallback to path: {}", e, sunshine_path);
                sunshine_path.clone()
            }
        };
        local_sunshine_path = Some(path_to_run.clone());
        let child_opt = check_and_start_sunshine(&path_to_run, &host_ip, host_port);
        if let Some(ref child) = child_opt {
            SUNSHINE_PID.store(child.id().unwrap_or(0), std::sync::atomic::Ordering::SeqCst);
        }
        _sunshine_child = child_opt;
    } else if !port_open && no_auto_start_sunshine {
        // If it's not running and we can't start it, we still run auto-pair just in case
        info!("Sunshine is not running and auto-start is disabled. Ensuring configuration is prepared...");
        config = match auto_pair_local_sunshine(&name, &config_path, Some(config.server_url.clone())) {
            Ok(c) => c,
            Err(e) => {
                error!("Failed to auto-pair local Sunshine config: {:?}", e);
                return Err(e.into());
            }
        };
    }

    // If server_certificate is empty, we will try to read it from Sunshine credentials directory
    if config.server_certificate.is_empty() {
        info!("Server certificate not loaded yet. Waiting for Sunshine to initialize credentials...");
        // Wait up to 10 seconds for the cert to appear
        if let Some(sunshine_dir) = crate::pairing::get_sunshine_dir() {
            let cert_path = sunshine_dir.join("credentials").join("cacert.pem");
            let mut cert_loaded = false;
            for _ in 0..10 {
                if cert_path.exists() {
                    if let Ok(cert_pem) = std::fs::read_to_string(&cert_path) {
                        config.server_certificate = cert_pem;
                        if let Err(e) = save_config(&config, &config_path) {
                            error!("Failed to save server certificate to {}: {:?}", config_path, e);
                        } else {
                            info!("Successfully read and saved server certificate!");
                            cert_loaded = true;
                        }
                        break;
                    }
                }
                tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;
            }
            if !cert_loaded {
                warn!("Sunshine server certificate was not found after 10 seconds. WebRTC pairing may fail.");
            }
        }
    }

    info!("Starting Host Agent: {} ({})", name, config.client_unique_id);

    // Query Sunshine capabilities with retry
    let mut codec_support = None;
    for i in 1..=5 {
        match query_sunshine_codec_support(&host_ip, host_port, &config).await {
            Ok(support) => {
                info!("Successfully queried Sunshine codec support bitmask: {}", support);
                codec_support = Some(support);
                break;
            }
            Err(e) => {
                if i == 5 {
                    warn!("Failed to query Sunshine codec support after 5 attempts: {:?}", e);
                } else {
                    info!("Sunshine not ready yet, retrying query (attempt {}/5)...", i);
                    tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;
                }
            }
        }
    }

    let mut base_ws_url = config.server_url.clone();
    if base_ws_url.starts_with("http://") {
        base_ws_url = base_ws_url.replacen("http://", "ws://", 1);
    } else if base_ws_url.starts_with("https://") {
        base_ws_url = base_ws_url.replacen("https://", "wss://", 1);
    } else if !base_ws_url.starts_with("ws://") && !base_ws_url.starts_with("wss://") {
        base_ws_url = format!("ws://{}", base_ws_url);
    }

    // Connect to Signaling Server
    let server_ws_url = if let Some(support) = codec_support {
        format!(
            "{}/ws/agent?id={}&name={}&codec_support={}&token={}",
            base_ws_url.trim_end_matches('/'),
            config.client_unique_id,
            urlencoding::encode(&name),
            support,
            urlencoding::encode(&config.server_token)
        )
    } else {
        format!(
            "{}/ws/agent?id={}&name={}&token={}",
            base_ws_url.trim_end_matches('/'),
            config.client_unique_id,
            urlencoding::encode(&name),
            urlencoding::encode(&config.server_token)
        )
    };

    info!("Connecting to signaling server at: {}", server_ws_url);
    let (ws_stream, _) = connect_async(server_ws_url).await?;
    info!("Connected to signaling server!");
    CONNECTED_TO_SERVER.store(true, std::sync::atomic::Ordering::SeqCst);

    let (mut ws_write, mut ws_read) = ws_stream.split();
    let (agent_tx, mut agent_rx) = mpsc::unbounded_channel::<AgentMessage>();

    // Spawn outbound WS writing task
    let write_task = tokio::spawn(async move {
        while let Some(msg) = agent_rx.recv().await {
            if let Ok(json) = serde_json::to_string(&msg) {
                if let Err(e) = ws_write.send(WsMessage::Text(json)).await {
                    error!("WebSocket write error: {:?}", e);
                    break;
                }
            }
        }
    });

    // Active session
    let active_session: Arc<Mutex<Option<Arc<BridgeSession>>>> = Arc::new(Mutex::new(None));

    // Handle inbound WS signaling messages
    while let Some(message_result) = ws_read.next().await {
        let ws_msg = match message_result {
            Ok(msg) => msg,
            Err(e) => {
                error!("WebSocket read error: {:?}", e);
                break;
            }
        };

        if let WsMessage::Text(text) = ws_msg {
            let server_msg: ServerToAgentMessage = match serde_json::from_str(&text) {
                Ok(m) => m,
                Err(e) => {
                    error!("Failed to parse server message: {}. Payload: {}", e, text);
                    continue;
                }
            };

            match server_msg {
                ServerToAgentMessage::Registered { success } => {
                    info!("Registration success: {}", success);
                }
                ServerToAgentMessage::Signaling(sig) => {
                    match sig {
                        SignalingMessage::IncomingSession {
                            client_id,
                            width,
                            height,
                            fps,
                            bitrate,
                            codec,
                            app_id,
                        } => {
                            info!("Incoming session request from client: {}", client_id);
                            
                            // Clean up previous active session on the agent to release Sunshine stream
                            {
                                let mut lock = active_session.lock().await;
                                if let Some(old_session) = lock.take() {
                                    info!("Cleaning up previous active session before setting up new one...");
                                    let _ = old_session.peer_connection.close().await;
                                    let mut stream_lock = old_session.moonlight_stream.write().unwrap();
                                    if let Some(stream) = stream_lock.take() {
                                        info!("Stopping Moonlight stream for previous session...");
                                        stream.stop();
                                    }
                                }
                            }

                            // Initialize bridge session
                            let session = match setup_bridge_session(
                                config.clone(),
                                client_id.clone(),
                                host_ip.clone(),
                                host_port,
                                agent_tx.clone(),
                                width,
                                height,
                                fps,
                                bitrate,
                                codec,
                                app_id,
                            ).await {
                                Ok(s) => s,
                                Err(e) => {
                                    error!("Failed to setup WebRTC bridge session: {:?}", e);
                                    let _ = agent_tx.send(AgentMessage::Signaling(SignalingMessage::Error {
                                        message: format!("Failed to bridge connection: {:?}", e),
                                    }));
                                    continue;
                                }
                            };

                            // Generate SDP Offer
                            let offer = match session.peer_connection.create_offer(None).await {
                                Ok(o) => o,
                                Err(e) => {
                                    error!("Failed to create SDP Offer: {:?}", e);
                                    continue;
                                }
                            };

                            if let Err(e) = session.peer_connection.set_local_description(offer.clone()).await {
                                error!("Failed to set local description: {:?}", e);
                                continue;
                            }

                            // Send Offer to Client
                            let sdp_msg = AgentMessage::Signaling(SignalingMessage::Sdp {
                                target_id: client_id.clone(),
                                sdp: RtcSessionDescription {
                                    ty: RtcSdpType::Offer,
                                    sdp: offer.sdp,
                                },
                            });
                            let _ = agent_tx.send(sdp_msg);

                            let mut lock = active_session.lock().await;
                            *lock = Some(session);
                            info!("SDP Offer sent to client: {}", client_id);
                        }
                        SignalingMessage::Sdp { target_id, sdp } => {
                            info!("Received SDP description from client: {}", target_id);
                            let lock = active_session.lock().await;
                            if let Some(session) = lock.as_ref() {
                                    match RTCSessionDescription::answer(sdp.sdp) {
                                        Ok(rtc_sdp) => {
                                            if let Err(e) = session.peer_connection.set_remote_description(rtc_sdp).await {
                                                error!("Failed to set remote description: {:?}", e);
                                            } else {
                                                info!("SDP Answer set successfully!");
                                            }
                                        }
                                        Err(e) => {
                                            error!("Failed to parse SDP Answer: {:?}", e);
                                        }
                                    }
                            } else {
                                warn!("Received SDP without an active session");
                            }
                        }
                        SignalingMessage::IceCandidate { target_id: _, candidate } => {
                            let lock = active_session.lock().await;
                            if let Some(session) = lock.as_ref() {
                                let rtc_cand = RTCIceCandidateInit {
                                    candidate: candidate.candidate,
                                    sdp_mid: candidate.sdp_mid,
                                    sdp_mline_index: candidate.sdp_mline_index,
                                    username_fragment: candidate.username_fragment,
                                };
                                if let Err(e) = session.peer_connection.add_ice_candidate(rtc_cand).await {
                                    debug!("Failed to add ICE candidate: {:?}", e);
                                }
                            }
                        }
                        SignalingMessage::EndSession { target_id } => {
                            info!("Session ended by client: {}", target_id);
                            let mut lock = active_session.lock().await;
                            if let Some(session) = lock.take() {
                                let _ = session.peer_connection.close().await;
                                let mut stream_lock = session.moonlight_stream.write().unwrap();
                                if let Some(stream) = stream_lock.take() {
                                    info!("Stopping Moonlight stream...");
                                    stream.stop();
                                }
                            }
                        }
                        SignalingMessage::GetAppList { target_id } => {
                            info!("Received GetAppList request from target: {}", target_id);
                            let config_clone = config.clone();
                            let host_ip_clone = host_ip.clone();
                            let agent_tx_clone = agent_tx.clone();
                            tokio::spawn(async move {
                                match get_agent_apps(&config_clone, &host_ip_clone, host_port).await {
                                    Ok((apps, current_game_id)) => {
                                        let resp = AgentMessage::Signaling(SignalingMessage::AppListResponse {
                                            target_id,
                                            apps,
                                            current_game_id,
                                        });
                                        let _ = agent_tx_clone.send(resp);
                                    }
                                    Err(e) => {
                                        error!("Failed to get app list: {:?}", e);
                                        let resp = AgentMessage::Signaling(SignalingMessage::Error {
                                            message: format!("Failed to retrieve app list: {:?}", e),
                                        });
                                        let _ = agent_tx_clone.send(resp);
                                    }
                                }
                            });
                        }
                        SignalingMessage::StopActiveStream { target_id } => {
                            info!("Received StopActiveStream request from target: {}", target_id);
                            // Clean up active session locally first
                            {
                                let mut lock = active_session.lock().await;
                                if let Some(session) = lock.take() {
                                    let _ = session.peer_connection.close().await;
                                    let mut stream_lock = session.moonlight_stream.write().unwrap();
                                    if let Some(stream) = stream_lock.take() {
                                        info!("Stopping Moonlight stream...");
                                        stream.stop();
                                    }
                                }
                            }
                            let config_clone = config.clone();
                            let host_ip_clone = host_ip.clone();
                            let agent_tx_clone = agent_tx.clone();
                            tokio::spawn(async move {
                                match stop_agent_stream(&config_clone, &host_ip_clone, host_port).await {
                                    Ok(success) => {
                                        let resp = AgentMessage::Signaling(SignalingMessage::StopActiveStreamResponse {
                                            target_id,
                                            success,
                                            error: None,
                                        });
                                        let _ = agent_tx_clone.send(resp);
                                    }
                                    Err(e) => {
                                        error!("Failed to stop stream: {:?}", e);
                                        let resp = AgentMessage::Signaling(SignalingMessage::StopActiveStreamResponse {
                                            target_id,
                                            success: false,
                                            error: Some(e.to_string()),
                                        });
                                        let _ = agent_tx_clone.send(resp);
                                    }
                                }
                            });
                        }
                        SignalingMessage::GetSunshineConfig { target_id } => {
                            info!("Received GetSunshineConfig request from target: {}", target_id);
                            let config = match read_sunshine_conf() {
                                Ok(cfg) => cfg,
                                Err(e) => {
                                    error!("Failed to read sunshine.conf: {:?}", e);
                                    "{}".to_string()
                                }
                            };
                            let resp = AgentMessage::Signaling(SignalingMessage::SunshineConfigResponse {
                                target_id,
                                config,
                            });
                            let _ = agent_tx.send(resp);
                        }
                        SignalingMessage::UpdateSunshineConfig { target_id, config: config_str } => {
                            info!("Received UpdateSunshineConfig request from target: {}", target_id);
                            let mut success = true;
                            let mut error = None;
                            if let Err(e) = write_sunshine_conf(&config_str) {
                                error!("Failed to write sunshine.conf: {:?}", e);
                                success = false;
                                error = Some(e.to_string());
                            } else {
                                info!("Successfully updated sunshine.conf, restarting Sunshine process...");
                                if !no_auto_start_sunshine {
                                    if let Some(path) = &local_sunshine_path {
                                        if let Some(mut child) = _sunshine_child.take() {
                                            info!("Killing local Sunshine process (PID: {:?}) for configuration update...", child.id());
                                            #[cfg(unix)]
                                            {
                                                if let Some(pid) = child.id() {
                                                    // Kill the entire process group
                                                    unsafe {
                                                        libc::kill(-(pid as libc::pid_t), libc::SIGKILL);
                                                    }
                                                }
                                            }
                                            #[cfg(not(unix))]
                                            {
                                                let _ = child.kill().await;
                                            }
                                            let _ = child.wait().await;
                                            tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;
                                        }
                                        let child_opt = check_and_start_sunshine(path, &host_ip, host_port);
                                        if let Some(ref child) = child_opt {
                                            SUNSHINE_PID.store(child.id().unwrap_or(0), std::sync::atomic::Ordering::SeqCst);
                                        } else {
                                            SUNSHINE_PID.store(0, std::sync::atomic::Ordering::SeqCst);
                                        }
                                        _sunshine_child = child_opt;
                                    } else {
                                        warn!("Sunshine path is not known, cannot restart automatically.");
                                    }
                                } else {
                                    info!("no_auto_start_sunshine is true, not restarting Sunshine automatically.");
                                }
                            }
                            let resp = AgentMessage::Signaling(SignalingMessage::UpdateSunshineConfigResponse {
                                target_id,
                                success,
                                error,
                            });
                            let _ = agent_tx.send(resp);
                        }
                        _ => {}
                    }
                }
            }
        }
    }

    CONNECTED_TO_SERVER.store(false, std::sync::atomic::Ordering::SeqCst);
    write_task.abort();

    // Clean up active session and stop Moonlight stream
    {
        let mut session_lock = active_session.lock().await;
        if let Some(session) = session_lock.take() {
            info!("Stopping active remote streaming session on agent exit...");
            let _ = session.peer_connection.close().await;
            let mut stream_lock = session.moonlight_stream.write().unwrap();
            if let Some(stream) = stream_lock.take() {
                info!("Stopping Moonlight stream...");
                stream.stop();
            }
        }
    }
    
    // Explicitly kill Sunshine child if we spawned it
    if let Some(mut child) = _sunshine_child {
        info!("Stopping local Sunshine child process...");
        #[cfg(unix)]
        {
            if let Some(pid) = child.id() {
                unsafe {
                    libc::kill(-(pid as libc::pid_t), libc::SIGKILL);
                }
            }
        }
        #[cfg(not(unix))]
        {
            let _ = child.kill().await;
        }
        let _ = child.wait().await;
        SUNSHINE_PID.store(0, std::sync::atomic::Ordering::SeqCst);
    }

    info!("Host Agent finished.");
    Ok(())
}

async fn get_agent_apps(
    config: &crate::pairing::AgentConfig,
    host_ip: &str,
    host_port: u16,
) -> Result<(Vec<common::AppInfo>, u32), anyhow::Error> {
    use moonlight_common::http::client::tokio_hyper::TokioHyperClient;
    use moonlight_common::http::{ClientIdentifier, ClientSecret, ServerIdentifier};
    use moonlight_common::high::tokio::MoonlightHost;

    let host = MoonlightHost::<TokioHyperClient>::new(host_ip.to_string(), host_port, Some(config.client_unique_id.clone()))?;
    
    let client_cert_pem = pem::parse(&config.client_certificate)?;
    let client_key_pem = pem::parse(&config.client_private_key)?;
    let server_cert_pem = pem::parse(&config.server_certificate)?;

    host.set_identity(
        ClientIdentifier::from_pem(client_cert_pem),
        ClientSecret::from_pem(client_key_pem),
        ServerIdentifier::from_pem(server_cert_pem),
    )
    .await?;

    let apps = host.app_list().await?;
    let current_game = host.current_game().await?;

    let host_ref = &host;
    let mut futures = Vec::new();
    for app in apps {
        futures.push(async move {
            let icon_base64 = match host_ref.request_app_image(app.id).await {
                Ok(bytes) => {
                    use common::base64::Engine;
                    Some(common::base64::prelude::BASE64_STANDARD.encode(bytes))
                }
                Err(e) => {
                    log::warn!("Failed to fetch icon for app {}: {:?}", app.id, e);
                    None
                }
            };
            common::AppInfo {
                id: app.id,
                title: app.title,
                icon_base64,
            }
        });
    }
    let app_infos = futures_util::future::join_all(futures).await;

    Ok((app_infos, current_game))
}

async fn stop_agent_stream(
    config: &crate::pairing::AgentConfig,
    host_ip: &str,
    host_port: u16,
) -> Result<bool, anyhow::Error> {
    use moonlight_common::http::client::tokio_hyper::TokioHyperClient;
    use moonlight_common::http::{ClientIdentifier, ClientSecret, ServerIdentifier};
    use moonlight_common::high::tokio::MoonlightHost;

    let host = MoonlightHost::<TokioHyperClient>::new(host_ip.to_string(), host_port, Some(config.client_unique_id.clone()))?;
    
    let client_cert_pem = pem::parse(&config.client_certificate)?;
    let client_key_pem = pem::parse(&config.client_private_key)?;
    let server_cert_pem = pem::parse(&config.server_certificate)?;

    host.set_identity(
        ClientIdentifier::from_pem(client_cert_pem),
        ClientSecret::from_pem(client_key_pem),
        ServerIdentifier::from_pem(server_cert_pem),
    )
    .await?;

    let cancelled = host.cancel().await?;
    Ok(cancelled)
}
