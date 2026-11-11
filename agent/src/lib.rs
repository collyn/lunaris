
use clap::Parser;
use futures_util::{SinkExt, StreamExt};
use std::sync::Arc;
use tokio::sync::{mpsc, Mutex};
use tokio_tungstenite::connect_async_tls_with_config;
use tokio_tungstenite::tungstenite::Message as WsMessage;
use tracing::{debug, error, info, warn};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

use common::{
    AgentMessage, RtcSdpType, RtcSessionDescription, ServerToAgentMessage, SignalingMessage,
};
use webrtc::ice_transport::ice_candidate::RTCIceCandidateInit;
use webrtc::peer_connection::sdp::session_description::RTCSessionDescription;

mod bridge;
mod buffer;
mod direct;
mod hub;
mod input;
mod pairing;
mod vdisplay;
mod video;

#[cfg(feature = "gui")]
pub mod agent_gui;

pub static CONNECTED_TO_SERVER: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);
pub static AGENT_ACTIVE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
pub static LAST_ERROR: std::sync::Mutex<Option<String>> = std::sync::Mutex::new(None);
pub static LAST_STREAM_STOP_TIME_MS: std::sync::atomic::AtomicU64 =
    std::sync::atomic::AtomicU64::new(0);

fn get_current_time_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

fn encoder_supports_codec(
    encoder: &lunaris_media::EncoderInfo,
    codec: lunaris_media::VideoCodec,
) -> bool {
    encoder.supported_codecs.contains(&codec)
}

fn host_gpu_likely_supports_av1_encode() -> bool {
    let Some(gpu) = lunaris_media::encode::describe_host_gpu(None) else {
        return false;
    };
    let gpu = gpu.to_ascii_lowercase();

    if gpu.contains("nvidia") {
        return gpu.contains("rtx 40")
            || gpu.contains("rtx 50")
            || gpu.contains("ada")
            || gpu.contains("blackwell")
            || gpu.contains(" l4")
            || gpu.contains(" l40");
    }

    if gpu.contains("intel") {
        return gpu.contains("arc")
            || gpu.contains("battlemage")
            || gpu.contains("data center gpu")
            || gpu.contains(" flex");
    }

    if gpu.contains("amd") || gpu.contains("radeon") {
        return gpu.contains("rx 7")
            || gpu.contains("rx 8")
            || gpu.contains("rx 9")
            || gpu.contains("rdna3")
            || gpu.contains("rdna 3");
    }

    false
}

fn codec_supported_for_realtime(
    encoders: &[lunaris_media::EncoderInfo],
    codec: lunaris_media::VideoCodec,
) -> bool {
    match codec {
        lunaris_media::VideoCodec::H264 => encoders.iter().any(|e| encoder_supports_codec(e, codec)),
        lunaris_media::VideoCodec::H265 => encoders.iter().any(|e| {
            e.hw_type != lunaris_media::HwAccelType::Software && encoder_supports_codec(e, codec)
        }),
        lunaris_media::VideoCodec::AV1 => {
            host_gpu_likely_supports_av1_encode()
                && encoders.iter().any(|e| {
                    e.hw_type != lunaris_media::HwAccelType::Software
                        && encoder_supports_codec(e, codec)
                })
        }
    }
}

use crate::bridge::{setup_bridge_session, BridgeSession};
use crate::pairing::AgentConfig;

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    #[arg(long, default_value = "127.0.0.1")]
    host_ip: String,

    #[arg(long)]
    pair: bool,

    #[arg(long)]
    pin: Option<String>,

    #[arg(long)]
    server: Option<String>,

    #[arg(long)]
    name: Option<String>,

    #[arg(long)]
    cli: bool,

    /// Config file (default: %APPDATA%\\lunaris-agent / ~/.config/lunaris-agent,
    /// or ./agent_config.json when it exists)
    #[arg(long)]
    config: Option<String>,

    #[arg(long)]
    import_config: Option<String>,

    #[arg(long)]
    minimized: bool,

    /// Try to open each encoder at 1920x1080@60 and print which ones work, then exit
    #[arg(long)]
    probe_encoders: bool,

    /// Accept direct connections (no server) and print the addresses and the
    /// access password (saved in the config, generated on first use)
    #[arg(long)]
    direct: bool,

    /// Port for direct connections (default 8091)
    #[arg(long)]
    direct_port: Option<u16>,
}

/// `--probe-encoders`: open each codec's encoder for real and report it.
fn probe_encoders() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let _ = tracing_subscriber::registry()
        .with(tracing_subscriber::EnvFilter::new(
            std::env::var("RUST_LOG").unwrap_or_else(|_| "off,ffmpeg=warn,lunaris_media=error".into()),
        ))
        .with(tracing_subscriber::fmt::layer())
        .try_init();
    #[cfg(not(target_os = "windows"))]
    for codec in [
        lunaris_media::VideoCodec::H264,
        lunaris_media::VideoCodec::H265,
        lunaris_media::VideoCodec::AV1,
    ] {
        match lunaris_media::encode::probe_encoder(codec, 1920, 1080, 60) {
            Some(e) => println!("{:?}: {} ({:?})", codec, e.name, e.hw_type),
            None => println!("{:?}: no working encoder", codec),
        }
    }
    #[cfg(target_os = "windows")]
    for e in lunaris_media::encode::list_available_encoders() {
        println!("{} ({:?}): {:?}", e.name, e.hw_type, e.supported_codecs);
    }
    Ok(())
}

/// Runs the agent with command-line arguments (the first one is the program
/// name), as the `agent` binary and `lunaris agent` do.
pub fn run<I, T>(args: I) -> std::process::ExitCode
where
    I: IntoIterator<Item = T>,
    T: Into<std::ffi::OsString> + Clone,
{
    let args = match Args::try_parse_from(args) {
        Ok(args) => args,
        Err(e) => e.exit(),
    };
    let runtime = match tokio::runtime::Runtime::new() {
        Ok(rt) => rt,
        Err(e) => {
            eprintln!("error: cannot start the async runtime: {e}");
            return std::process::ExitCode::FAILURE;
        }
    };
    match runtime.block_on(main_async(args)) {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e}");
            std::process::ExitCode::FAILURE
        }
    }
}

async fn main_async(args: Args) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    if args.probe_encoders {
        return probe_encoders();
    }
    crate::pairing::set_config_path(args.config.clone());
    let config_path = crate::pairing::config_path().to_string();

    // If import_config is specified, we perform the import first
    if let Some(ref import_path) = args.import_config {
        let _ = tracing_subscriber::registry()
            .with(tracing_subscriber::EnvFilter::new(
                std::env::var("RUST_LOG").unwrap_or_else(|_| "info,agent=info,webrtc_sctp=off,dtls=error".into()),
            ))
            .with(tracing_subscriber::fmt::layer())
            .try_init();

        info!(
            "Importing configuration from {} into {}...",
            import_path, config_path
        );
        if let Err(e) = crate::pairing::import_config_file(import_path, &config_path) {
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
                        {
                            if cfg!(debug_assertions) {
                                "info,agent=info,webrtc_sctp=off,dtls=error"
                            } else {
                                "info,agent=info,lunaris_media=info,webrtc_sctp=off,dtls=error,webrtc_ice=warn"
                            }
                        }
                        .into()
                    }),
                ))
                .with(
                    tracing_subscriber::fmt::layer()
                        .with_writer(agent_gui::ChannelMakeWriter)
                        .with_ansi(false),
                )
                .try_init();

            info!("Launching Lunaris Agent in Desktop GUI mode...");
            agent_gui::run_gui(args.minimized);
            return Ok(());
        }
    }

    // CLI mode execution
    // Init standard tracing logger
    let _ = tracing_subscriber::registry()
        .with(tracing_subscriber::EnvFilter::new(
            std::env::var("RUST_LOG").unwrap_or_else(|_| {
                "info,agent=info,webrtc_sctp=off,dtls=error".into()
            }),
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
        info!("lunaris-media is the streaming backend.");
        return Ok(());
    }

    // Load or create agent config
    let mut config = match crate::pairing::load_config(&config_path) {
        Ok(c) => c,
        Err(e) => {
            error!("Failed to load agent config from {}: {:?}", config_path, e);
            std::process::exit(1);
        }
    };

    if args.direct || args.direct_port.is_some() {
        config.direct_enabled = true;
        if let Some(port) = args.direct_port {
            config.direct_port = port;
        }
        if crate::pairing::ensure_direct_password(&mut config) {
            if let Err(e) = crate::pairing::save_config(&config, &config_path) {
                warn!("Could not save the direct-access password to {}: {}", config_path, e);
            }
        }
    }
    if config.direct_enabled {
        // Printed, not logged: the password should not end up in log files.
        println!("Direct connections — connect a client to one of:");
        for addr in direct::local_addresses().await {
            println!("    {}:{}", addr, config.direct_port);
        }
        println!("Access password: {}", config.direct_password);
    }

    run_agent(config, name).await
}

pub async fn run_agent(
    mut config: AgentConfig,
    name: String,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    CONNECTED_TO_SERVER.store(false, std::sync::atomic::Ordering::SeqCst);

    let hub = hub::Hub::new(config.client_unique_id.clone());
    let (agent_tx, router) = hub::spawn_router(hub.clone());
    let (inbound_tx, inbound_rx) = mpsc::unbounded_channel::<SignalingMessage>();
    let mut tasks = AbortOnDrop(vec![router]);

    if config.direct_enabled {
        if crate::pairing::ensure_direct_password(&mut config) {
            if let Err(e) = crate::pairing::save_config(&config, crate::pairing::config_path()) {
                warn!("Could not save the new direct-access password: {}", e);
            }
        }
        let port = config.direct_port;
        let password = config.direct_password.clone();
        let hub = hub.clone();
        let inbound_tx = inbound_tx.clone();
        tasks.0.push(tokio::spawn(async move {
            if let Err(e) = direct::run_listener(port, password, hub, inbound_tx).await {
                let msg = format!("Direct connections unavailable: cannot listen on port {}: {}", port, e);
                error!("{}", msg);
                if let Ok(mut last) = LAST_ERROR.lock() {
                    *last = Some(msg);
                }
            }
        }));
    }

    if config.server_url.trim().is_empty() {
        info!("No Lunaris server configured: direct connections only.");
    } else {
        tasks.0.push(tokio::spawn(run_server_connection(
            config.clone(),
            name.clone(),
            hub.clone(),
            inbound_tx.clone(),
        )));
    }

    info!("Starting Host Agent: {} ({})", name, config.client_unique_id);
    // `inbound_tx` stays alive here, so the session logic runs until this
    // future is dropped (GUI stop) even when nothing is connected.
    run_session_logic(config, agent_tx, inbound_rx).await;
    drop(inbound_tx);
    drop(tasks);
    Ok(())
}

/// Aborts the agent's background tasks when `run_agent` stops.
struct AbortOnDrop(Vec<tokio::task::JoinHandle<()>>);

impl Drop for AbortOnDrop {
    fn drop(&mut self) {
        for task in &self.0 {
            task.abort();
        }
    }
}

/// Keeps a connection to the signaling server, reconnecting every 3 s.
async fn run_server_connection(
    config: AgentConfig,
    name: String,
    hub: Arc<hub::Hub>,
    inbound_tx: mpsc::UnboundedSender<SignalingMessage>,
) {
    let mut last_error = String::new();
    loop {
        match connect_server_once(&config, &name, &hub, &inbound_tx).await {
            Ok(()) => {
                info!("Disconnected from the signaling server. Reconnecting in 3 seconds...");
                last_error.clear();
            }
            Err(e) => {
                let msg = format!("Signaling server connection failed: {}", e);
                // Report each new error once instead of every 3 seconds.
                if msg != last_error {
                    error!("{}. Retrying every 3 seconds...", msg);
                    if let Ok(mut last) = LAST_ERROR.lock() {
                        *last = Some(msg.clone());
                    }
                    last_error = msg;
                } else {
                    debug!("{}", msg);
                }
            }
        }
        hub.set_server(None);
        CONNECTED_TO_SERVER.store(false, std::sync::atomic::Ordering::SeqCst);
        tokio::time::sleep(tokio::time::Duration::from_secs(3)).await;
    }
}

async fn connect_server_once(
    config: &AgentConfig,
    name: &str,
    hub: &Arc<hub::Hub>,
    inbound_tx: &mpsc::UnboundedSender<SignalingMessage>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    // Query codec support from lunaris-media encoders. H.264 software is a
    // practical fallback; H.265/AV1 software is too slow for interactive remote
    // desktop, so only advertise those when a realtime-capable hardware encoder exists.
    let codec_support = {
        let encoders = lunaris_media::encode::list_available_encoders();
        let h264 = codec_supported_for_realtime(&encoders, lunaris_media::VideoCodec::H264);
        let h265 = codec_supported_for_realtime(&encoders, lunaris_media::VideoCodec::H265);
        let av1 = codec_supported_for_realtime(&encoders, lunaris_media::VideoCodec::AV1);
        let mut bits: u32 = 0;
        if h264 {
            bits |= 262145;
        }
        if h265 {
            bits |= 1573632;
        }
        if av1 {
            bits |= 6488064;
        }
        Some(bits)
    };

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
            urlencoding::encode(name),
            support,
            urlencoding::encode(&config.server_token)
        )
    } else {
        format!(
            "{}/ws/agent?id={}&name={}&token={}",
            base_ws_url.trim_end_matches('/'),
            config.client_unique_id,
            urlencoding::encode(name),
            urlencoding::encode(&config.server_token)
        )
    };

    // Never log server_ws_url itself: it carries the agent token.
    debug!("Connecting to signaling server at {}", base_ws_url);
    // Build a TLS connector that accepts self-signed certificates.
    // The signaling server typically uses a self-signed cert for local/LAN use.
    let tls_connector = native_tls::TlsConnector::builder()
        .danger_accept_invalid_certs(true)
        .build()
        .map_err(|e| anyhow::anyhow!("Failed to build TLS connector: {}", e))?;
    let connector = tokio_tungstenite::Connector::NativeTls(tls_connector);

    let (ws_stream, _) = connect_async_tls_with_config(
        server_ws_url,
        None,
        false,
        Some(connector),
    ).await?;
    info!("Connected to signaling server!");
    CONNECTED_TO_SERVER.store(true, std::sync::atomic::Ordering::SeqCst);

    let (mut ws_write, mut ws_read) = ws_stream.split();
    let (server_tx, mut server_rx) = mpsc::unbounded_channel::<AgentMessage>();
    hub.set_server(Some(server_tx));

    // Outbound: the hub routes the session logic's replies for server clients here.
    let write_task = tokio::spawn(async move {
        while let Some(msg) = server_rx.recv().await {
            match serde_json::to_string(&msg) {
                Ok(json) => {
                    if let Err(e) = ws_write.send(WsMessage::Text(json)).await {
                        error!("WebSocket write error: {:?}", e);
                        break;
                    }
                }
                Err(e) => error!("Failed to serialize outbound message: {:?}", e),
            }
        }
    });

    while let Some(message_result) = ws_read.next().await {
        let text = match message_result {
            Ok(WsMessage::Text(text)) => text,
            Ok(_) => continue,
            Err(e) => {
                error!("WebSocket read error: {:?}", e);
                break;
            }
        };
        match serde_json::from_str::<ServerToAgentMessage>(&text) {
            Ok(ServerToAgentMessage::Registered { success }) => {
                info!("Registration success: {}", success);
            }
            Ok(ServerToAgentMessage::Signaling(sig)) => {
                let _ = inbound_tx.send(sig);
            }
            Err(e) => error!("Failed to parse server message: {}. Payload: {}", e, text),
        }
    }

    write_task.abort();
    hub.set_server(None);
    CONNECTED_TO_SERVER.store(false, std::sync::atomic::Ordering::SeqCst);
    Ok(())
}

/// The agent's session logic: one streaming session at a time, driven by
/// signaling from server and direct clients alike (addressed with the client's
/// connection id). Replies go to `agent_tx`, which the hub routes.
async fn run_session_logic(
    config: AgentConfig,
    agent_tx: mpsc::UnboundedSender<AgentMessage>,
    mut inbound: mpsc::UnboundedReceiver<SignalingMessage>,
) {
    let active_session: Arc<Mutex<Option<Arc<BridgeSession>>>> = Arc::new(Mutex::new(None));
    // The client connection that owns `active_session`.
    let mut active_client: Option<String> = None;

    while let Some(sig) = inbound.recv().await {
        match sig {
            SignalingMessage::IncomingSession {
                client_id,
                width,
                height,
                fps,
                bitrate,
                codec,
                app_id: _,
                encoder,
                display_id,
                virtual_display,
                chroma,
                ice_servers,
            } => {
                info!(
                    "Incoming session request from client: {} codec={:?} encoder={:?} res={:?}x{:?} fps={:?} bitrate={:?} display={:?} virtual_display={:?} chroma={:?}",
                    client_id, codec, encoder, width, height, fps, bitrate, display_id, virtual_display, chroma
                );

                // Clean up previous active session
                {
                    let mut lock = active_session.lock().await;
                    if let Some(old_session) = lock.take() {
                        info!("Cleaning up previous active session before setting up new one...");
                        let _ = old_session.peer_connection.close().await;
                        LAST_STREAM_STOP_TIME_MS.store(
                            get_current_time_ms(),
                            std::sync::atomic::Ordering::SeqCst,
                        );
                    }
                }

                // Wait if the stream was stopped very recently (let encoder release resources)
                let last_stop = LAST_STREAM_STOP_TIME_MS.load(std::sync::atomic::Ordering::SeqCst);
                if last_stop > 0 {
                    let now = get_current_time_ms();
                    let elapsed = now.saturating_sub(last_stop);
                    if elapsed < 1200 {
                        let sleep_ms = 1200 - elapsed;
                        info!("Previous stream was stopped recently ({}ms ago). Delaying new stream setup by {}ms to prevent encoder conflicts...", elapsed, sleep_ms);
                        tokio::time::sleep(std::time::Duration::from_millis(sleep_ms)).await;
                    }
                }

                active_client = None;

                // Initialize bridge session
                let session = match setup_bridge_session(
                    config.clone(),
                    client_id.clone(),
                    agent_tx.clone(),
                    width,
                    height,
                    fps,
                    bitrate,
                    codec,
                    encoder,
                    display_id,
                    virtual_display,
                    chroma.as_deref() == Some("444"),
                    ice_servers,
                )
                .await
                {
                    Ok(s) => s,
                    Err(e) => {
                        error!("Failed to setup WebRTC bridge session: {:?}", e);
                        let _ = agent_tx.send(AgentMessage::Signaling(
                            SignalingMessage::Error {
                                message: format!(
                                    "Failed to bridge connection: {:?}",
                                    e
                                ),
                            },
                        ));
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

                if let Err(e) = session
                    .peer_connection
                    .set_local_description(offer.clone())
                    .await
                {
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
                    ice_servers: None,
                    webtransport_port: session.webtransport_port,
                    webtransport_cert_hash: session.webtransport_cert_hash.clone(),
                });

                // CRITICAL: Store session BEFORE sending the offer to avoid a race
                // condition where the client's SDP answer arrives before the session
                // is stored, causing it to be silently dropped (=> no ICE candidate
                // pairs => connection timeout after ~7s).
                {
                    let mut lock = active_session.lock().await;
                    *lock = Some(session);
                }
                active_client = Some(client_id.clone());

                let _ = agent_tx.send(sdp_msg);
                warn!("[SIGNALING] SDP Offer sent to client: {}", client_id);
            }
            SignalingMessage::Sdp { target_id, sdp, .. } => {
                warn!("[SIGNALING] Received SDP answer from client: {}", target_id);
                let lock = active_session.lock().await;
                if let Some(session) = lock.as_ref() {
                    // Force Level 4.2 in SDP Answer to prevent browser decoder from silently rejecting Level 4.0/4.2 AMF GPU streams.
                    let forced_sdp = sdp
                        .sdp
                        .replace("profile-level-id=42001f", "profile-level-id=42002a")
                        // Same for H.264 High 4:4:4 (YUV 4:4:4 sessions).
                        .replace("profile-level-id=f4001f", "profile-level-id=f4002a");
                    match RTCSessionDescription::answer(forced_sdp) {
                        Ok(rtc_sdp) => {
                            if let Err(e) = session
                                .peer_connection
                                .set_remote_description(rtc_sdp)
                                .await
                            {
                                error!("[SIGNALING] Failed to set remote description: {:?}", e);
                            } else {
                                warn!("[SIGNALING] SDP Answer set successfully! WebRTC negotiation in progress.");
                            }
                        }
                        Err(e) => {
                            error!("[SIGNALING] Failed to parse SDP Answer: {:?}", e);
                        }
                    }
                } else {
                    warn!("[SIGNALING] *** BUG: Received SDP answer but active_session is None! Answer dropped.");
                }
            }
            SignalingMessage::IceCandidate {
                target_id: _,
                candidate,
            } => {
                let lock = active_session.lock().await;
                if let Some(session) = lock.as_ref() {
                    let rtc_cand = RTCIceCandidateInit {
                        candidate: candidate.candidate,
                        sdp_mid: candidate.sdp_mid,
                        sdp_mline_index: candidate.sdp_mline_index,
                        username_fragment: candidate.username_fragment,
                    };
                    if let Err(e) =
                        session.peer_connection.add_ice_candidate(rtc_cand).await
                    {
                        debug!("Failed to add ICE candidate: {:?}", e);
                    }
                }
            }
            SignalingMessage::EndSession { target_id } => {
                // Only the client that owns the session ends it: another
                // client (e.g. a direct connection that only asked for the
                // display list) disconnecting must not cut the stream.
                if active_client.as_deref() != Some(target_id.as_str()) {
                    debug!("Ignoring EndSession from {} (not the session's client)", target_id);
                    continue;
                }
                info!("Session ended by client: {}", target_id);
                active_client = None;
                let mut lock = active_session.lock().await;
                if let Some(session) = lock.take() {
                    let _ = session.peer_connection.close().await;
                    LAST_STREAM_STOP_TIME_MS.store(get_current_time_ms(), std::sync::atomic::Ordering::SeqCst);
                }
            }
            SignalingMessage::GetAppList { target_id } => {
                info!("Received GetAppList request from target: {}", target_id);
                let agent_tx_clone = agent_tx.clone();
                let apps = vec![common::AppInfo {
                    id: 0,
                    title: "Desktop".to_string(),
                    icon_base64: None,
                }];
                let resp = AgentMessage::Signaling(
                    SignalingMessage::AppListResponse {
                        target_id,
                        apps,
                        current_game_id: 0,
                    },
                );
                let _ = agent_tx_clone.send(resp);
            }
            SignalingMessage::GetCapabilities { target_id } => {
                info!("Received GetCapabilities request from target: {}", target_id);
                let agent_tx_clone = agent_tx.clone();

                // 1. List displays. Uses the lightweight enumeration
                // (NvFBC status only, no CUDA context) so opening the
                // client settings no longer allocates/leaks GPU memory.
                let mut displays = Vec::new();
                match lunaris_media::capture::list_displays_for_capabilities().await {
                    Ok(display_list) => {
                        info!("[GetCapabilities] Discovered {} display(s)", display_list.len());
                        for d in &display_list {
                            info!("  Display: {} ({}) {}x{} @{}Hz primary={}", d.id, d.name, d.width, d.height, d.refresh_rate, d.is_primary);
                            displays.push(common::DisplayInfoMsg {
                                id: d.id.clone(),
                                name: d.name.clone(),
                                width: d.width,
                                height: d.height,
                                refresh_rate: d.refresh_rate,
                                is_primary: d.is_primary,
                            });
                        }
                    }
                    Err(e) => error!("Failed to list displays: {:?}", e),
                }

                // 3. Query encoders (fast, non-blocking)
                info!("[GetCapabilities] Querying encoders...");
                let mut encoders = Vec::new();
                encoders.push("native".to_string());
                encoders.push("ffmpeg".to_string());
                let encoder_list = lunaris_media::encode::list_available_encoders();
                for enc in encoder_list {
                    if !encoders.contains(&enc.name) {
                        encoders.push(enc.name.clone());
                    }
                    let hw_name = format!("{}", enc.hw_type).to_lowercase();
                    if !encoders.contains(&hw_name) {
                        encoders.push(hw_name);
                    }
                }
                if !encoders.contains(&"software".to_string()) {
                    encoders.push("software".to_string());
                }

                // 4. Send response
                info!("[GetCapabilities] Sending response: {} displays, {} encoders", displays.len(), encoders.len());
                let resp = AgentMessage::Signaling(
                    SignalingMessage::CapabilitiesResponse {
                        target_id,
                        displays,
                        encoders,
                        gpu_info: lunaris_media::encode::describe_host_gpu(None),
                        host_os: Some(std::env::consts::OS.to_string()),
                    },
                );
                if let Err(e) = agent_tx_clone.send(resp) {
                    error!("Failed to send CapabilitiesResponse: {:?}", e);
                } else {
                    info!("[GetCapabilities] Response sent!");
                }
            }
            SignalingMessage::ConfigureVirtualDisplay { target_id, enable, width, height, refresh_hz } => {
                let output = config
                    .virtual_display_output
                    .clone()
                    .unwrap_or_else(|| "DP-2".to_string());
                info!(
                    "Received ConfigureVirtualDisplay: enable={} {}x{}@{}Hz output={}",
                    enable, width, height, refresh_hz, output
                );
                let agent_tx_clone = agent_tx.clone();

                // 1. Enable/disable the virtual output via xrandr.
                let (success, message) = if enable {
                    match vdisplay::enable(&output, width, height, refresh_hz) {
                        Ok(()) => (
                            true,
                            format!("Virtual display '{}' enabled at {}x{}@{}Hz", output, width, height, refresh_hz),
                        ),
                        Err(e) => (false, e),
                    }
                } else {
                    match vdisplay::disable(&output) {
                        Ok(()) => (true, format!("Virtual display '{}' disabled", output)),
                        Err(e) => (false, e),
                    }
                };
                if success {
                    info!("ConfigureVirtualDisplay: {}", message);
                } else {
                    warn!("ConfigureVirtualDisplay failed: {}", message);
                }

                // 2. Re-enumerate displays so the client dropdown refreshes.
                let mut displays = Vec::new();
                match lunaris_media::capture::list_displays_for_capabilities().await {
                    Ok(display_list) => {
                        for d in &display_list {
                            displays.push(common::DisplayInfoMsg {
                                id: d.id.clone(),
                                name: d.name.clone(),
                                width: d.width,
                                height: d.height,
                                refresh_rate: d.refresh_rate,
                                is_primary: d.is_primary,
                            });
                        }
                    }
                    Err(e) => error!("Failed to list displays: {:?}", e),
                }
                // Fix up refresh labels (NvFBC reports a hardcoded 60Hz).
                vdisplay::enrich_refresh_rates(&mut displays);

                // 3. Reply.
                let resp = AgentMessage::Signaling(
                    SignalingMessage::VirtualDisplayConfigured {
                        target_id,
                        success,
                        message,
                        displays,
                    },
                );
                if let Err(e) = agent_tx_clone.send(resp) {
                    error!("Failed to send VirtualDisplayConfigured: {:?}", e);
                }
            }
            SignalingMessage::StopActiveStream { target_id } => {
                info!(
                    "Received StopActiveStream request from target: {}",
                    target_id
                );
                // Clean up active session
                active_client = None;
                let success = {
                    let mut lock = active_session.lock().await;
                    if let Some(session) = lock.take() {
                        let _ = session.peer_connection.close().await;
                        LAST_STREAM_STOP_TIME_MS.store(get_current_time_ms(), std::sync::atomic::Ordering::SeqCst);
                        true
                    } else {
                        false
                    }
                };
                let resp = AgentMessage::Signaling(
                    SignalingMessage::StopActiveStreamResponse {
                        target_id,
                        success,
                        error: if success { None } else { Some("No active session".to_string()) },
                    },
                );
                let _ = agent_tx.send(resp);
            }
            _ => {}
        }
    }

    let mut session_lock = active_session.lock().await;
    if let Some(session) = session_lock.take() {
        info!("Stopping active remote streaming session on agent exit...");
        let _ = session.peer_connection.close().await;
    }
}
