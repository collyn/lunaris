use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;
use futures_util::{StreamExt, SinkExt};
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::Message as WsMessage;
use url::Url;
use tracing::{info, error, warn, debug};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

use common::{ClientMessage, ServerToClientMessage, SignalingMessage, RtcSessionDescription, RtcSdpType};
use webrtc::api::APIBuilder;
use webrtc::api::media_engine::MediaEngine;
use webrtc::peer_connection::configuration::RTCConfiguration;
use webrtc::peer_connection::peer_connection_state::RTCPeerConnectionState;
use webrtc::peer_connection::sdp::session_description::RTCSessionDescription;
use webrtc::ice_transport::ice_server::RTCIceServer;
use webrtc::ice_transport::ice_candidate::RTCIceCandidateInit;
use webrtc::data_channel::RTCDataChannel;
use webrtc::track::track_remote::TrackRemote;

use openh264::decoder::Decoder as OpenH264Decoder;
use openh264::formats::YUVSource;
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};

mod protocol;
mod input;
mod ui;

// SDL2 Window Dimensions

struct YUVFrame {
    width: i32,
    height: i32,
    y: Vec<u8>,
    u: Vec<u8>,
    v: Vec<u8>,
    y_stride: i32,
    u_stride: i32,
    v_stride: i32,
}

#[derive(Clone, Debug)]
struct AppArgs {
    host_id: String,
    server_url: String,
    token: String,
    width: u32,
    height: u32,
    fps: u32,
    bitrate: u32,
    codec: String,
}

fn parse_args() -> Option<AppArgs> {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 2 {
        return None;
    }

    // Check if deep linked: lunaris://connect?host_id=...&server=...&token=...
    if args[1].starts_with("lunaris://") {
        if let Ok(parsed_url) = Url::parse(&args[1]) {
            let mut host_id = String::new();
            let mut server_url = String::new();
            let mut token = String::new();
            
            let mut width = 1920; // Default resolution
            let mut height = 1080;
            let mut fps = 60;
            let mut bitrate = 8000;
            let mut codec = "h264".to_string();

            for (k, v) in parsed_url.query_pairs() {
                match k.as_ref() {
                    "host_id" => host_id = v.into_owned(),
                    "server" => server_url = v.into_owned(),
                    "token" => token = v.into_owned(),
                    "res" => {
                        let parts: Vec<&str> = v.split('x').collect();
                        if parts.len() == 2 {
                            if let (Ok(w), Ok(h)) = (parts[0].parse::<u32>(), parts[1].parse::<u32>()) {
                                width = w;
                                height = h;
                            }
                        }
                    }
                    "fps" => {
                        if let Ok(f) = v.parse::<u32>() {
                            fps = f;
                        }
                    }
                    "bitrate" => {
                        if let Ok(b) = v.parse::<u32>() {
                            bitrate = b;
                        }
                    }
                    "codec" => {
                        codec = v.into_owned().to_lowercase();
                    }
                    _ => {}
                }
            }

            if !host_id.is_empty() && !server_url.is_empty() && !token.is_empty() {
                return Some(AppArgs { host_id, server_url, token, width, height, fps, bitrate, codec });
            }
        }
    }

    // Fallback to normal CLI arguments: client --host-id ID --server URL --token TOKEN ...
    let mut host_id = String::new();
    let mut server_url = String::new();
    let mut token = String::new();
    
    let mut width = 1920;
    let mut height = 1080;
    let mut fps = 60;
    let mut bitrate = 8000;
    let mut codec = "h264".to_string();

    let mut i = 1;
    while i < args.len() - 1 {
        match args[i].as_str() {
            "--host-id" => {
                host_id = args[i + 1].clone();
                i += 2;
            }
            "--server" => {
                server_url = args[i + 1].clone();
                i += 2;
            }
            "--token" => {
                token = args[i + 1].clone();
                i += 2;
            }
            "--res" => {
                let parts: Vec<&str> = args[i + 1].split('x').collect();
                if parts.len() == 2 {
                    if let (Ok(w), Ok(h)) = (parts[0].parse::<u32>(), parts[1].parse::<u32>()) {
                        width = w;
                        height = h;
                    }
                }
                i += 2;
            }
            "--fps" => {
                if let Ok(f) = args[i + 1].parse::<u32>() {
                    fps = f;
                }
                i += 2;
            }
            "--bitrate" => {
                if let Ok(b) = args[i + 1].parse::<u32>() {
                    bitrate = b;
                }
                i += 2;
            }
            "--codec" => {
                codec = args[i + 1].clone().to_lowercase();
                i += 2;
            }
            _ => {
                i += 1;
            }
        }
    }

    if !host_id.is_empty() && !server_url.is_empty() && !token.is_empty() {
        Some(AppArgs { host_id, server_url, token, width, height, fps, bitrate, codec })
    } else {
        None
    }
}

// Setup CPAL Audio Output
fn setup_audio() -> Option<mpsc::UnboundedSender<Vec<f32>>> {
    let host = cpal::default_host();
    let device = host.default_output_device()?;
    let config = device.default_output_config().ok()?;
    
    let config_channels = config.channels();
    let config_sample_rate = config.sample_rate().0;
    info!("Initializing CPAL audio output device: {} channels, {} Hz", config_channels, config_sample_rate);
    
    let (tx, mut rx) = mpsc::unbounded_channel::<Vec<f32>>();
    let mut audio_buffer = Vec::<f32>::new();
    
    let stream = device.build_output_stream(
        &config.into(),
        move |data: &mut [f32], _: &cpal::OutputCallbackInfo| {
            while let Ok(mut samples) = rx.try_recv() {
                audio_buffer.append(&mut samples);
            }
            
            let target_channels = config_channels as usize;
            let target_sample_rate = config_sample_rate as f64;
            let ratio = 48000.0 / target_sample_rate;
            let output_frames = data.len() / target_channels;
            
            let needed_input_frames = (output_frames as f64 * ratio).ceil() as usize + 2;
            let available_input_frames = audio_buffer.len() / 2;
            
            if available_input_frames < needed_input_frames {
                data.fill(0.0);
                return;
            }
            
            let mut source_frame_ptr = 0.0f64;
            for i in 0..output_frames {
                let idx = source_frame_ptr as usize;
                let fract = (source_frame_ptr - idx as f64) as f32;
                
                let l1 = audio_buffer[idx * 2];
                let r1 = audio_buffer[idx * 2 + 1];
                let l2 = audio_buffer[idx * 2 + 2];
                let r2 = audio_buffer[idx * 2 + 3];
                
                let left = l1 + (l2 - l1) * fract;
                let right = r1 + (r2 - r1) * fract;
                
                let out_idx = i * target_channels;
                if target_channels == 1 {
                    data[out_idx] = (left + right) * 0.5;
                } else if target_channels == 2 {
                    data[out_idx] = left;
                    data[out_idx + 1] = right;
                } else {
                    data[out_idx] = left;
                    data[out_idx + 1] = right;
                    for c in 2..target_channels {
                        data[out_idx + c] = 0.0;
                    }
                }
                
                source_frame_ptr += ratio;
            }
            
            let consumed_frames = source_frame_ptr.floor() as usize;
            if consumed_frames * 2 <= audio_buffer.len() {
                audio_buffer.drain(..consumed_frames * 2);
            } else {
                audio_buffer.clear();
            }
        },
        |err| warn!("an error occurred on cpal stream: {}", err),
        None,
    ).ok()?;
    
    let _ = stream.play();
    Box::leak(Box::new(stream)); // Leak stream so it runs in background
    
    Some(tx)
}

#[cfg(unix)]
fn redirect_stdout_stderr() {
    use std::os::unix::io::AsRawFd;
    if let Ok(home) = std::env::var("HOME") {
        let log_path = std::path::Path::new(&home).join("lunaris-client.log");
        if let Ok(file) = std::fs::File::create(log_path) {
            let fd = file.as_raw_fd();
            unsafe {
                extern "C" {
                    fn dup2(src: i32, dst: i32) -> i32;
                }
                let _ = dup2(fd, 1); // stdout
                let _ = dup2(fd, 2); // stderr
            }
        }
    }
}

#[cfg(not(unix))]
fn redirect_stdout_stderr() {}

#[tokio::main]
async fn main() {
    redirect_stdout_stderr();

    // Init standard tracing logger
    let _ = tracing_subscriber::registry()
        .with(tracing_subscriber::EnvFilter::new(
            std::env::var("RUST_LOG").unwrap_or_else(|_| "info,client=debug".into()),
        ))
        .with(tracing_subscriber::fmt::layer())
        .try_init();

    // Try registering custom URI scheme
    if let Err(e) = protocol::register_protocol() {
        warn!("Failed to auto-register protocol handler: {:?}", e);
    }

    let current_args = parse_args();
    if current_args.is_none() {
        let usage_msg = "Usage: client --host-id <HOST_ID> --server <SERVER_URL> --token <JWT_TOKEN> [options]\n\nOptions:\n  --res <w>x<h>\n  --fps <fps>\n  --bitrate <kbps>\n  --codec <h264|h265|av1>\n\nOr deep-link connect:\nlunaris://connect?host_id=<HOST_ID>&server=<SERVER_URL>&token=<JWT_TOKEN>&res=1920x1080&fps=60...";
        let _ = sdl2::messagebox::show_simple_message_box(
            sdl2::messagebox::MessageBoxFlag::WARNING,
            "Lunaris Client Usage",
            usage_msg,
            None,
        );
        std::process::exit(1);
    }

    let mut args = current_args.unwrap();
    loop {
        match run_client(args.clone()).await {
            Ok(Some(new_args)) => {
                info!("Reconnecting with new settings: {}x{} @ {} FPS, {} kbps, codec {}", new_args.width, new_args.height, new_args.fps, new_args.bitrate, new_args.codec);
                args = new_args;
            }
            Ok(None) => {
                info!("Client exited cleanly.");
                break;
            }
            Err(e) => {
                error!("Client error: {:?}", e);
                let _ = sdl2::messagebox::show_simple_message_box(
                    sdl2::messagebox::MessageBoxFlag::ERROR,
                    "Lunaris Client Error",
                    &format!("Failed to connect or stream:\n{}", e),
                    None,
                );
                std::process::exit(1);
            }
        }
    }
}

fn is_inside(mx: i32, my: i32, rect: sdl2::rect::Rect) -> bool {
    mx >= rect.x() && mx <= rect.x() + rect.width() as i32 && my >= rect.y() && my <= rect.y() + rect.height() as i32
}

async fn run_client(args: AppArgs) -> Result<Option<AppArgs>, anyhow::Error> {
    info!("Launching Lunaris Client: Connecting to {} for Host {}", args.server_url, args.host_id);

    // -------------------------------------------------------------------------
    // SDL2 Initialization
    // -------------------------------------------------------------------------
    let sdl_context = sdl2::init().map_err(|e| anyhow::anyhow!("SDL init err: {}", e))?;
    let video_subsystem = sdl_context.video().map_err(|e| anyhow::anyhow!("SDL video err: {}", e))?;
    
    let window = video_subsystem.window("Lunaris Player Client", args.width, args.height)
        .position_centered()
        .resizable()
        .build()
        .map_err(|e| anyhow::anyhow!("SDL window err: {}", e))?;
        
    let mut canvas = window.into_canvas().present_vsync().build().map_err(|e| anyhow::anyhow!("SDL canvas err: {}", e))?;
    let texture_creator = canvas.texture_creator();

    // -------------------------------------------------------------------------
    // Audio Output Initialization (CPAL)
    // -------------------------------------------------------------------------
    let audio_tx = setup_audio();
    if audio_tx.is_none() {
        warn!("No default audio output device found. Audio will be disabled.");
    }

    // Channels to communicate media frames from WebRTC threads to SDL rendering loop
    let (video_frame_tx, mut video_frame_rx) = mpsc::channel::<YUVFrame>(2);

    // -------------------------------------------------------------------------
    // WebRTC & Media Engine Setup
    // -------------------------------------------------------------------------
    let mut media_engine = MediaEngine::default();
    media_engine.register_default_codecs()?;
    
    let api = APIBuilder::new()
        .with_media_engine(media_engine)
        .build();

    let rtc_config = RTCConfiguration {
        ice_servers: vec![
            RTCIceServer {
                urls: vec!["stun:stun.l.google.com:19302".to_string()],
                ..Default::default()
            },
            RTCIceServer {
                urls: vec!["stun:stun1.l.google.com:19302".to_string()],
                ..Default::default()
            },
        ],
        ..Default::default()
    };
    
    let peer_connection = Arc::new(api.new_peer_connection(rtc_config).await?);

    // -------------------------------------------------------------------------
    // Connect to Signaling Server
    // -------------------------------------------------------------------------
    let server_ws_url = format!(
        "{}/ws/client?token={}",
        args.server_url.trim_end_matches('/').replace("http", "ws"),
        args.token
    );

    info!("Connecting to signaling server at: {}", server_ws_url);
    let (ws_stream, _) = connect_async(server_ws_url).await?;
    info!("Connected to signaling server!");

    let (mut ws_write, mut ws_read) = ws_stream.split();
    let (outbox_tx, mut outbox_rx) = mpsc::unbounded_channel::<ClientMessage>();

    // Outbox WebSocket writing task
    tokio::spawn(async move {
        while let Some(msg) = outbox_rx.recv().await {
            if let Ok(json) = serde_json::to_string(&msg) {
                if let Err(e) = ws_write.send(WsMessage::Text(json)).await {
                    error!("WebSocket write error: {:?}", e);
                    break;
                }
            }
        }
    });

    // -------------------------------------------------------------------------
    // Track Listeners (Video/Audio Decoders)
    // -------------------------------------------------------------------------
    let audio_tx_clone = audio_tx.clone();
    let peer_connection_clone = Arc::clone(&peer_connection);
    
    peer_connection.on_track(Box::new(move |track: Arc<TrackRemote>, _receiver, _| {
        let track_clone = Arc::clone(&track);
        let audio_tx_inner = audio_tx_clone.clone();
        let video_tx_inner = video_frame_tx.clone();
        let pc_clone = Arc::clone(&peer_connection_clone);
        
        info!("Received remote WebRTC track: {} (Mime: {})", track.id(), track.codec().capability.mime_type);
        
        tokio::spawn(async move {
            let codec = track_clone.codec();
            if codec.capability.mime_type.to_lowercase() == "video/h264" {
                info!("Starting H.264 video decoding worker...");
                let mut decoder = OpenH264Decoder::new().unwrap();
                let mut annex_b_buf = Vec::<u8>::new();
                let mut packet_count = 0u64;
                let mut decoded_count = 0u64;
                
                let has_decoded = Arc::new(std::sync::atomic::AtomicBool::new(false));
                let has_decoded_clone = Arc::clone(&has_decoded);
                let media_ssrc = track_clone.ssrc();
                let pc_clone_inner = Arc::clone(&pc_clone);
                
                tokio::spawn(async move {
                    use webrtc::rtcp::payload_feedbacks::picture_loss_indication::PictureLossIndication;
                    info!("PLI requester task started for video track SSRC: {}", media_ssrc);
                    
                    while !has_decoded_clone.load(std::sync::atomic::Ordering::SeqCst) {
                        let pli = PictureLossIndication {
                            sender_ssrc: 0,
                            media_ssrc,
                        };
                        debug!("Sending PLI request to host for keyframe...");
                        if let Err(e) = pc_clone_inner.write_rtcp(&[Box::new(pli)]).await {
                            warn!("Failed to send PLI request: {:?}", e);
                        }
                        tokio::time::sleep(Duration::from_millis(1000)).await;
                    }
                    info!("First frame decoded, stopping periodic PLI requests.");
                });
                
                while let Ok((rtp_packet, _)) = track_clone.read_rtp().await {
                    let payload = rtp_packet.payload;
                    if payload.is_empty() {
                        continue;
                    }
                    
                    packet_count += 1;
                    if packet_count % 300 == 0 {
                        info!("Video receiver stats: received {} RTP packets, decoded {} frames", packet_count, decoded_count);
                    }
                    
                    let nal_type = payload[0] & 0x1F;
                    if nal_type >= 1 && nal_type <= 23 {
                        // Single NAL unit
                        annex_b_buf.clear();
                        annex_b_buf.extend_from_slice(&[0, 0, 0, 1]);
                        annex_b_buf.extend_from_slice(&payload);
                        
                        match decoder.decode(&annex_b_buf) {
                            Ok(Some(decoded)) => {
                                if decoded_count == 0 {
                                    has_decoded.store(true, std::sync::atomic::Ordering::SeqCst);
                                }
                                decoded_count += 1;
                                let frame = YUVFrame {
                                    width: decoded.width(),
                                    height: decoded.height(),
                                    y: decoded.y().to_vec(),
                                    u: decoded.u().to_vec(),
                                    v: decoded.v().to_vec(),
                                    y_stride: decoded.y_stride(),
                                    u_stride: decoded.u_stride(),
                                    v_stride: decoded.v_stride(),
                                };
                                let _ = video_tx_inner.try_send(frame);
                            }
                            Ok(None) => {}
                            Err(e) => {
                                warn!("OpenH264 decode error (Single NAL): {:?}", e);
                            }
                        }
                    } else if nal_type == 24 {
                        // STAP-A Aggregation Packet
                        let mut offset = 1;
                        while offset + 2 <= payload.len() {
                            let nalu_size = ((payload[offset] as usize) << 8) | (payload[offset + 1] as usize);
                            offset += 2;
                            if offset + nalu_size > payload.len() {
                                break;
                            }
                            let nalu_data = &payload[offset..offset + nalu_size];
                            offset += nalu_size;
                            
                            if !nalu_data.is_empty() {
                                annex_b_buf.clear();
                                annex_b_buf.extend_from_slice(&[0, 0, 0, 1]);
                                annex_b_buf.extend_from_slice(nalu_data);
                                if let Err(e) = decoder.decode(&annex_b_buf) {
                                    warn!("OpenH264 decode error (STAP-A sub-NALU): {:?}", e);
                                }
                            }
                        }
                    } else if nal_type == 28 {
                        // FU-A Fragmentation Unit
                        if payload.len() < 2 {
                            continue;
                        }
                        let fu_indicator = payload[0];
                        let fu_header = payload[1];
                        let start_bit = (fu_header & 0x80) != 0;
                        let end_bit = (fu_header & 0x40) != 0;
                        let inner_nal_type = fu_header & 0x1F;
                        let reconstructed_header = (fu_indicator & 0xE0) | inner_nal_type;
                        
                        if start_bit {
                            annex_b_buf.clear();
                            annex_b_buf.extend_from_slice(&[0, 0, 0, 1, reconstructed_header]);
                            annex_b_buf.extend_from_slice(&payload[2..]);
                        } else {
                            annex_b_buf.extend_from_slice(&payload[2..]);
                        }
                        
                        if end_bit {
                            match decoder.decode(&annex_b_buf) {
                                Ok(Some(decoded)) => {
                                    if decoded_count == 0 {
                                        has_decoded.store(true, std::sync::atomic::Ordering::SeqCst);
                                    }
                                    decoded_count += 1;
                                    let frame = YUVFrame {
                                        width: decoded.width(),
                                        height: decoded.height(),
                                        y: decoded.y().to_vec(),
                                        u: decoded.u().to_vec(),
                                        v: decoded.v().to_vec(),
                                        y_stride: decoded.y_stride(),
                                        u_stride: decoded.u_stride(),
                                        v_stride: decoded.v_stride(),
                                    };
                                    let _ = video_tx_inner.try_send(frame);
                                }
                                Ok(None) => {}
                                Err(e) => {
                                    warn!("OpenH264 decode error (FU-A): {:?}", e);
                                }
                            }
                        }
                    }
                }
            } else if codec.capability.mime_type.to_lowercase() == "audio/opus" {
                info!("Starting Opus audio decoding worker...");
                let mut decoder = match opus::Decoder::new(48000, opus::Channels::Stereo) {
                    Ok(d) => d,
                    Err(e) => {
                        error!("Failed to initialize Opus decoder: {:?}", e);
                        return;
                    }
                };
                let mut pcm_output = vec![0.0f32; 1920 * 2]; // Stereo frame buffer
                
                while let Ok((rtp_packet, _)) = track_clone.read_rtp().await {
                    if let Some(ref tx) = audio_tx_inner {
                        if let Ok(num_samples) = decoder.decode_float(&rtp_packet.payload, &mut pcm_output, false) {
                            let stereo_samples = pcm_output[..num_samples * 2].to_vec();
                            let _ = tx.send(stereo_samples);
                        }
                    }
                }
            }
        });
        
        Box::pin(async {})
    }));

    // -------------------------------------------------------------------------
    // Input Data Channel Capture
    // -------------------------------------------------------------------------
    let keyboard_chan = Arc::new(std::sync::Mutex::new(None));
    let mouse_abs_chan = Arc::new(std::sync::Mutex::new(None));
    let mouse_rel_chan = Arc::new(std::sync::Mutex::new(None));
    
    let k_chan = Arc::clone(&keyboard_chan);
    let ma_chan = Arc::clone(&mouse_abs_chan);
    let mr_chan = Arc::clone(&mouse_rel_chan);
    
    peer_connection.on_data_channel(Box::new(move |d: Arc<RTCDataChannel>| {
        let label = d.label().to_string();
        info!("Remote Peer created DataChannel: {}", label);
        let channel_ref = Arc::clone(&d);
        
        match label.as_str() {
            "keyboard" => {
                let mut lock = k_chan.lock().unwrap();
                *lock = Some(channel_ref);
            }
            "mouse_absolute" => {
                let mut lock = ma_chan.lock().unwrap();
                *lock = Some(channel_ref);
            }
            "mouse_relative" => {
                let mut lock = mr_chan.lock().unwrap();
                *lock = Some(channel_ref);
            }
            _ => {}
        }
        
        Box::pin(async {})
    }));

    // -------------------------------------------------------------------------
    // Peer Connection State Change Handler
    // -------------------------------------------------------------------------
    peer_connection.on_peer_connection_state_change(Box::new(move |s: RTCPeerConnectionState| {
        info!("WebRTC Connection State changed to: {}", s);
        Box::pin(async {})
    }));

    // -------------------------------------------------------------------------
    // Send Request Session (Signaling Handshake)
    // -------------------------------------------------------------------------
    let req_msg = ClientMessage::Signaling(SignalingMessage::RequestSession {
        host_id: args.host_id.clone(),
        width: Some(args.width),
        height: Some(args.height),
        fps: Some(args.fps),
        bitrate: Some(args.bitrate),
        codec: Some(args.codec.clone()),
        app_id: None, // Default to Desktop
    });
    outbox_tx.send(req_msg)?;

    // Handle incoming Signaling WebSocket messages in a background task
    let peer_connection_signaling = Arc::clone(&peer_connection);
    let outbox_tx_signaling = outbox_tx.clone();
    let host_id_signaling = args.host_id.clone();
    
    tokio::spawn(async move {
        while let Some(msg_res) = ws_read.next().await {
            let ws_msg = match msg_res {
                Ok(m) => m,
                Err(e) => {
                    error!("WebSocket read error in signaling task: {:?}", e);
                    break;
                }
            };
            
            if let WsMessage::Text(text) = ws_msg {
                let server_msg: ServerToClientMessage = match serde_json::from_str(&text) {
                    Ok(m) => m,
                    Err(e) => {
                        error!("Failed to parse server message: {}. Payload: {}", e, text);
                        continue;
                    }
                };
                
                match server_msg {
                    ServerToClientMessage::Signaling(sig) => {
                        match sig {
                            SignalingMessage::Sdp { sdp, .. } => {
                                if sdp.ty == RtcSdpType::Offer {
                                    info!("Received SDP Offer from server, setting remote description...");
                                    if let Ok(rtc_sdp) = RTCSessionDescription::offer(sdp.sdp) {
                                        if let Err(e) = peer_connection_signaling.set_remote_description(rtc_sdp).await {
                                            error!("Failed to set remote description: {:?}", e);
                                            continue;
                                        }
                                        
                                        // Create SDP Answer
                                        info!("Creating SDP Answer...");
                                        let answer = match peer_connection_signaling.create_answer(None).await {
                                            Ok(ans) => ans,
                                            Err(e) => {
                                                error!("Failed to create SDP Answer: {:?}", e);
                                                continue;
                                            }
                                        };
                                        
                                        if let Err(e) = peer_connection_signaling.set_local_description(answer.clone()).await {
                                            error!("Failed to set local description: {:?}", e);
                                            continue;
                                        }
                                        
                                        // Send SDP Answer to Server
                                        let answer_msg = ClientMessage::Signaling(SignalingMessage::Sdp {
                                            target_id: host_id_signaling.clone(),
                                            sdp: RtcSessionDescription {
                                                ty: RtcSdpType::Answer,
                                                sdp: answer.sdp,
                                            },
                                        });
                                        let _ = outbox_tx_signaling.send(answer_msg);
                                        info!("SDP Answer sent back to server!");
                                    }
                                }
                            }
                            SignalingMessage::IceCandidate { candidate, .. } => {
                                let rtc_cand = RTCIceCandidateInit {
                                    candidate: candidate.candidate,
                                    sdp_mid: candidate.sdp_mid,
                                    sdp_mline_index: candidate.sdp_mline_index,
                                    username_fragment: candidate.username_fragment,
                                };
                                if let Err(e) = peer_connection_signaling.add_ice_candidate(rtc_cand).await {
                                    debug!("Failed to add ICE candidate: {:?}", e);
                                }
                            }
                            SignalingMessage::EndSession { .. } => {
                                info!("Session ended by host/server.");
                                break;
                            }
                            _ => {}
                        }
                    }
                }
            }
        }
    });

    // -------------------------------------------------------------------------
    // Main Rendering & Event Loop (SDL2)
    // -------------------------------------------------------------------------
    let mut event_pump = sdl_context.event_pump().map_err(|e| anyhow::anyhow!("SDL event pump err: {}", e))?;
    let mut texture: Option<sdl2::render::Texture> = None;
    let mut texture_width = 0i32;
    let mut texture_height = 0i32;

    // Keep track of active window sizes for input coordinate mappings
    let mut win_w = args.width as i16;
    let mut win_h = args.height as i16;

    // UI states
    let mut show_menu = false;
    let mut menu_y_offset = -38i32; // starts hidden (since menu height is 38)
    let mut show_stats = false;
    let mut pointer_locked = false;
    let mut fullscreen = false;
    let mut show_settings = false;
    let mut lock_notification_time: Option<std::time::Instant> = None;

    // Track selections in settings modal
    let mut sel_res_idx = ui::RESOLUTIONS.iter().position(|r| r.1 == args.width && r.2 == args.height).unwrap_or(1);
    let mut sel_fps_idx = ui::FPSS.iter().position(|f| f.1 == args.fps).unwrap_or(0);
    let mut sel_codec_idx = ui::CODECS.iter().position(|c| c.1 == args.codec).unwrap_or(0);
    let mut sel_bitrate_idx = ui::BITRATES.iter().position(|b| b.1 == args.bitrate).unwrap_or(2);

    let mut frame_time_accumulator = std::time::Instant::now();
    let mut frame_counter = 0u32;
    let mut rendered_fps = 0u32;
    let mut last_activity_time = std::time::Instant::now();

    // Enable alpha blend mode for translucent notch and settings overlays
    canvas.set_blend_mode(sdl2::render::BlendMode::Blend);

    info!("Starting SDL2 Event loop...");
    'running: loop {
        let mut had_activity = false;

        // Slide the menu smoothly up or down
        let target_y = if show_menu { 10 } else { -38 };
        if menu_y_offset < target_y {
            menu_y_offset += 2;
            had_activity = true;
        } else if menu_y_offset > target_y {
            menu_y_offset -= 2;
            had_activity = true;
        }

        // Auto-hide the menu after 3 seconds of mouse inactivity
        if show_menu && !show_settings && last_activity_time.elapsed() >= Duration::from_secs(3) {
            let mouse_state = event_pump.mouse_state();
            let mx = mouse_state.x();
            let my = mouse_state.y();
            let menu_rect = ui::get_menu_rect(win_w as i32, menu_y_offset);
            if !is_inside(mx, my, menu_rect) {
                show_menu = false;
            } else {
                last_activity_time = std::time::Instant::now();
            }
        }

        // Poll and process SDL2 Events
        for event in event_pump.poll_iter() {
            had_activity = true;
            match event {
                sdl2::event::Event::Quit { .. } => {
                    break 'running;
                }
                sdl2::event::Event::Window { win_event: sdl2::event::WindowEvent::SizeChanged(w, h), .. } => {
                    win_w = w as i16;
                    win_h = h as i16;
                    info!("Window resized to {}x{}", win_w, win_h);
                }
                sdl2::event::Event::MouseMotion { x, y, .. } => {
                    let mx = x;
                    let my = y;
                    
                    let menu_rect = ui::get_menu_rect(win_w as i32, menu_y_offset);
                    let trigger_rect = ui::get_trigger_rect(win_w as i32);
                    
                    if show_menu {
                        if is_inside(mx, my, menu_rect) {
                            last_activity_time = std::time::Instant::now();
                        }
                    } else {
                        if is_inside(mx, my, trigger_rect) {
                            show_menu = true;
                            last_activity_time = std::time::Instant::now();
                        }
                    }
                    
                    let k_lock = keyboard_chan.lock().unwrap();
                    let ma_lock = mouse_abs_chan.lock().unwrap();
                    let mr_lock = mouse_rel_chan.lock().unwrap();
                    
                    input::handle_sdl_event(
                        &event,
                        win_w,
                        win_h,
                        k_lock.as_ref(),
                        ma_lock.as_ref(),
                        mr_lock.as_ref(),
                        pointer_locked,
                    );
                }
                sdl2::event::Event::MouseButtonDown { x, y, .. } => {
                    let mx = x;
                    let my = y;
                    let mut click_handled = false;
                    
                    if show_settings {
                        let layout = ui::get_settings_layout(win_w as i32, win_h as i32);
                        if is_inside(mx, my, layout.cancel_btn) {
                            show_settings = false;
                            click_handled = true;
                        } else if is_inside(mx, my, layout.apply_btn) {
                            let (w, h) = (ui::RESOLUTIONS[sel_res_idx].1, ui::RESOLUTIONS[sel_res_idx].2);
                            let fps = ui::FPSS[sel_fps_idx].1;
                            let codec = ui::CODECS[sel_codec_idx].1.to_string();
                            let bitrate = ui::BITRATES[sel_bitrate_idx].1;
                            
                            let new_args = AppArgs {
                                host_id: args.host_id.clone(),
                                server_url: args.server_url.clone(),
                                token: args.token.clone(),
                                width: w,
                                height: h,
                                fps,
                                bitrate,
                                codec,
                            };
                            
                            return Ok(Some(new_args));
                        }
                        
                        for (idx, &rect) in layout.res_btns.iter().enumerate() {
                            if is_inside(mx, my, rect) {
                                sel_res_idx = idx;
                                click_handled = true;
                            }
                        }
                        for (idx, &rect) in layout.fps_btns.iter().enumerate() {
                            if is_inside(mx, my, rect) {
                                sel_fps_idx = idx;
                                click_handled = true;
                            }
                        }
                        for (idx, &rect) in layout.codec_btns.iter().enumerate() {
                            if is_inside(mx, my, rect) {
                                sel_codec_idx = idx;
                                click_handled = true;
                            }
                        }
                        for (idx, &rect) in layout.bitrate_btns.iter().enumerate() {
                            if is_inside(mx, my, rect) {
                                sel_bitrate_idx = idx;
                                click_handled = true;
                            }
                        }
                    }
                    
                    if show_settings {
                        continue;
                    }
                    
                    if show_menu {
                        let buttons = ui::get_menu_buttons(win_w as i32, menu_y_offset);
                        for &(rect, label) in &buttons {
                            if is_inside(mx, my, rect) {
                                match label {
                                    "Exit" => {
                                        return Ok(None);
                                    }
                                    "FS" => {
                                        fullscreen = !fullscreen;
                                        if fullscreen {
                                            let _ = canvas.window_mut().set_fullscreen(sdl2::video::FullscreenType::Desktop);
                                        } else {
                                            let _ = canvas.window_mut().set_fullscreen(sdl2::video::FullscreenType::Off);
                                        }
                                        click_handled = true;
                                    }
                                    "Lock" => {
                                        pointer_locked = !pointer_locked;
                                        let _ = sdl_context.mouse().set_relative_mouse_mode(pointer_locked);
                                        if pointer_locked {
                                            lock_notification_time = Some(std::time::Instant::now());
                                        } else {
                                            lock_notification_time = None;
                                        }
                                        click_handled = true;
                                    }
                                    "Stats" => {
                                        show_stats = !show_stats;
                                        click_handled = true;
                                    }
                                    "Settings" => {
                                        show_settings = true;
                                        click_handled = true;
                                    }
                                    _ => {}
                                }
                            }
                        }
                    }
                    
                    if !show_menu && menu_y_offset <= -35 {
                        let trig = ui::get_trigger_rect(win_w as i32);
                        if is_inside(mx, my, trig) {
                            show_menu = true;
                            last_activity_time = std::time::Instant::now();
                            click_handled = true;
                        }
                    }
                    
                    if !click_handled {
                        let k_lock = keyboard_chan.lock().unwrap();
                        let ma_lock = mouse_abs_chan.lock().unwrap();
                        let mr_lock = mouse_rel_chan.lock().unwrap();
                        input::handle_sdl_event(
                            &event,
                            win_w,
                            win_h,
                            k_lock.as_ref(),
                            ma_lock.as_ref(),
                            mr_lock.as_ref(),
                            pointer_locked,
                        );
                    }
                }
                sdl2::event::Event::KeyDown { keycode: Some(kc), keymod, .. } => {
                    let ctrl = keymod.contains(sdl2::keyboard::Mod::LCTRLMOD) || keymod.contains(sdl2::keyboard::Mod::RCTRLMOD);
                    let alt = keymod.contains(sdl2::keyboard::Mod::LALTMOD) || keymod.contains(sdl2::keyboard::Mod::RALTMOD);
                    if ctrl && alt && (kc == sdl2::keyboard::Keycode::Escape || kc == sdl2::keyboard::Keycode::M || kc == sdl2::keyboard::Keycode::Z) {
                        pointer_locked = false;
                        let _ = sdl_context.mouse().set_relative_mouse_mode(false);
                        lock_notification_time = None;
                        show_menu = true;
                        last_activity_time = std::time::Instant::now();
                        info!("Pointer lock released and menu opened via hotkey shortcut");
                    } else {
                        let k_lock = keyboard_chan.lock().unwrap();
                        let ma_lock = mouse_abs_chan.lock().unwrap();
                        let mr_lock = mouse_rel_chan.lock().unwrap();
                        input::handle_sdl_event(
                            &event,
                            win_w,
                            win_h,
                            k_lock.as_ref(),
                            ma_lock.as_ref(),
                            mr_lock.as_ref(),
                            pointer_locked,
                        );
                    }
                }
                other => {
                    let k_lock = keyboard_chan.lock().unwrap();
                    let ma_lock = mouse_abs_chan.lock().unwrap();
                    let mr_lock = mouse_rel_chan.lock().unwrap();
                    
                    input::handle_sdl_event(
                        &other,
                        win_w,
                        win_h,
                        k_lock.as_ref(),
                        ma_lock.as_ref(),
                        mr_lock.as_ref(),
                        pointer_locked,
                    );
                }
            }
        }

        // Pull decoded video frames from the WebRTC channel and render (drain to only render the latest frame)
        let mut latest_yuv = None;
        while let Ok(yuv) = video_frame_rx.try_recv() {
            latest_yuv = Some(yuv);
            had_activity = true;
        }

        if let Some(yuv) = latest_yuv {
            let w = yuv.width;
            let h = yuv.height;
            
            if texture.is_none() || texture_width != w || texture_height != h {
                info!("Initializing YUV hardware texture: {}x{}", w, h);
                texture = Some(
                    texture_creator
                        .create_texture_streaming(sdl2::pixels::PixelFormatEnum::IYUV, w as u32, h as u32)
                        .map_err(|e| anyhow::anyhow!("Texture creation err: {}", e))?
                );
                texture_width = w;
                texture_height = h;
            }

            if let Some(ref mut tex) = texture {
                let _ = tex.update_yuv(
                    None,
                    &yuv.y,
                    yuv.y_stride as usize,
                    &yuv.u,
                    yuv.u_stride as usize,
                    &yuv.v,
                    yuv.v_stride as usize,
                );
                
                canvas.clear();
                let _ = canvas.copy(tex, None, None);
                
                frame_counter += 1;
                if frame_time_accumulator.elapsed() >= Duration::from_secs(1) {
                    rendered_fps = frame_counter;
                    frame_counter = 0;
                    frame_time_accumulator = std::time::Instant::now();
                }

                if show_stats {
                    ui::draw_stats(&mut canvas, rendered_fps, &args.codec, args.width, args.height, args.bitrate);
                }

                ui::draw_menu(
                    &mut canvas,
                    win_w as i32,
                    menu_y_offset,
                    show_menu,
                    fullscreen,
                    pointer_locked,
                    show_stats,
                );

                if show_settings {
                    ui::draw_settings(
                        &mut canvas,
                        win_w as i32,
                        win_h as i32,
                        sel_res_idx,
                        sel_fps_idx,
                        sel_codec_idx,
                        sel_bitrate_idx,
                    );
                }

                if let Some(time) = lock_notification_time {
                    if time.elapsed() < Duration::from_secs(5) {
                        let hint = "Mouse locked. Press Ctrl+Alt+Esc to release.";
                        let text_w = hint.len() as i32 * 10;
                        let text_x = (win_w as i32 - text_w) / 2;
                        let text_y = win_h as i32 - 30;
                        canvas.set_draw_color(sdl2::pixels::Color::RGBA(15, 23, 42, 180));
                        let _ = canvas.fill_rect(sdl2::rect::Rect::new(text_x - 10, text_y - 5, (text_w + 20) as u32, 20u32));
                        canvas.set_draw_color(sdl2::pixels::Color::RGBA(139, 92, 246, 255));
                        let _ = canvas.draw_rect(sdl2::rect::Rect::new(text_x - 10, text_y - 5, (text_w + 20) as u32, 20u32));
                        ui::draw_text(&mut canvas, hint, text_x, text_y, sdl2::pixels::Color::RGBA(241, 245, 249, 255), 1);
                    } else {
                        lock_notification_time = None;
                    }
                }

                canvas.present();
            }
        }

        // Limit event loop rate dynamically to prevent CPU spinning while keeping input latency near zero
        if !had_activity {
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    // -------------------------------------------------------------------------
    // Cleanup & Close Session
    // -------------------------------------------------------------------------
    info!("Closing session...");
    let end_msg = ClientMessage::Signaling(SignalingMessage::EndSession {
        target_id: args.host_id.clone(),
    });
    let _ = outbox_tx.send(end_msg);
    let _ = peer_connection.close().await;
    
    // Give it a moment to send close packets
    std::thread::sleep(Duration::from_millis(500));
    info!("Lunaris Player Client exited cleanly.");
    Ok(None)
}
