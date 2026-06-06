#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use clap::Parser;
use futures_util::{SinkExt, StreamExt};
use std::sync::Arc;
use tokio::sync::{mpsc, Mutex};
use tokio_tungstenite::connect_async;
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
mod input;
mod pairing;
mod video;

#[cfg(feature = "gui")]
pub mod gui;

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

    #[arg(long, default_value = "agent_config.json")]
    config: String,

    #[arg(long)]
    import_config: Option<String>,

    #[arg(long)]
    minimized: bool,
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
                std::env::var("RUST_LOG").unwrap_or_else(|_| "info,agent=info,webrtc_sctp=off,dtls=error".into()),
            ))
            .with(tracing_subscriber::fmt::layer())
            .try_init();

        info!(
            "Importing configuration from {} into {}...",
            import_path, args.config
        );
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
                        .with_writer(gui::ChannelMakeWriter)
                        .with_ansi(false),
                )
                .try_init();

            info!("Launching Lunaris Agent in Desktop GUI mode...");
            gui::run_gui(args.minimized);
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
    let config = match crate::pairing::load_config(&args.config) {
        Ok(c) => c,
        Err(e) => {
            error!("Failed to load agent config from {}: {:?}", args.config, e);
            std::process::exit(1);
        }
    };

    loop {
        let loop_res = run_agent_loop(
            config.clone(),
            name.clone(),
            args.config.clone(),
        )
        .await;

        if let Err(e) = loop_res {
            error!("Agent loop returned error: {:?}. Reconnecting in 3 seconds...", e);
        } else {
            info!("Agent loop finished. Reconnecting in 3 seconds...");
        }

        tokio::time::sleep(tokio::time::Duration::from_secs(3)).await;
    }
}

pub async fn run_agent_loop(
    config: AgentConfig,
    name: String,
    _config_path: String,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    // Reset status
    CONNECTED_TO_SERVER.store(false, std::sync::atomic::Ordering::SeqCst);

    // Query codec support from lunaris-media encoders
    let codec_support = {
        let encoders = lunaris_media::encode::list_available_encoders();
        let h264 = encoders.iter().any(|e| e.supported_codecs.contains(&lunaris_media::VideoCodec::H264));
        let h265 = encoders.iter().any(|e| e.supported_codecs.contains(&lunaris_media::VideoCodec::H265));
        let mut bits: u32 = 0;
        if h264 { bits |= 262145; }
        if h265 { bits |= 1573632; }
        Some(bits)
    };

    info!(
        "Starting Host Agent: {} ({})",
        name, config.client_unique_id
    );

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
                            app_id: _,
                            encoder,
                            display_id,
                            virtual_display,
                            ice_servers,
                        } => {
                            info!("Incoming session request from client: {}", client_id);

                            // Clean up previous active session
                            {
                                let mut lock = active_session.lock().await;
                                if let Some(old_session) = lock.take() {
                                    info!("Cleaning up previous active session before setting up new one...");
                                    let _ = old_session.peer_connection.close().await;
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

                            let _ = agent_tx.send(sdp_msg);
                            warn!("[SIGNALING] SDP Offer sent to client: {}", client_id);
                        }
                        SignalingMessage::Sdp { target_id, sdp, .. } => {
                            warn!("[SIGNALING] Received SDP answer from client: {}", target_id);
                            let lock = active_session.lock().await;
                            if let Some(session) = lock.as_ref() {
                                // Force Level 4.2 in SDP Answer to prevent browser decoder from silently rejecting Level 4.0/4.2 AMF GPU streams.
                                let forced_sdp = sdp.sdp.replace("profile-level-id=42001f", "profile-level-id=42002a");
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
                            info!("Session ended by client: {}", target_id);
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
                            tokio::spawn(async move {
                                let mut displays = Vec::new();
                                let mut encoders = Vec::new();

                                // Query available displays from capture backends
                                match lunaris_media::capture::create_screen_capture() {
                                    Ok(cap) => {
                                        match cap.list_displays().await {
                                            Ok(display_list) => {
                                                for d in display_list {
                                                    displays.push(common::DisplayInfoMsg {
                                                        id: d.id,
                                                        name: d.name,
                                                        width: d.width,
                                                        height: d.height,
                                                        refresh_rate: d.refresh_rate,
                                                        is_primary: d.is_primary,
                                                    });
                                                }
                                            }
                                            Err(e) => {
                                                error!("Failed to list displays: {:?}", e);
                                            }
                                        }
                                    }
                                    Err(e) => {
                                        error!("Failed to create screen capture for display listing: {:?}", e);
                                    }
                                }

                                // Query available encoders. Generic entries control backend family;
                                // concrete entries allow advanced users to force a specific encoder.
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
                                // Always include software as fallback
                                if !encoders.contains(&"software".to_string()) {
                                    encoders.push("software".to_string());
                                }

                                let resp = AgentMessage::Signaling(
                                    SignalingMessage::CapabilitiesResponse {
                                        target_id,
                                        displays,
                                        encoders,
                                        gpu_info: None,
                                    },
                                );
                                let _ = agent_tx_clone.send(resp);
                            });
                        }
                        SignalingMessage::StopActiveStream { target_id } => {
                            info!(
                                "Received StopActiveStream request from target: {}",
                                target_id
                            );
                            // Clean up active session
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
            }
        }
    }

    CONNECTED_TO_SERVER.store(false, std::sync::atomic::Ordering::SeqCst);
    write_task.abort();

    // Clean up active session
    {
        let mut session_lock = active_session.lock().await;
        if let Some(session) = session_lock.take() {
            info!("Stopping active remote streaming session on agent exit...");
            let _ = session.peer_connection.close().await;
        }
    }

    info!("Host Agent finished.");
    Ok(())
}
