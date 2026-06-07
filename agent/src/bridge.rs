use std::io::Cursor;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use std::time::{Duration, Instant};

use anyhow::Result;
use base64::Engine;
use tokio::sync::mpsc;
use tracing::{debug, error, info, trace, warn};

use webrtc::ice_transport::ice_candidate::RTCIceCandidate;

use webrtc::{
    api::{
        interceptor_registry::register_default_interceptors, media_engine::MediaEngine,
        setting_engine::SettingEngine, APIBuilder,
    },
    data_channel::{
        data_channel_init::RTCDataChannelInit, data_channel_message::DataChannelMessage,
        data_channel_state::RTCDataChannelState, RTCDataChannel,
    },
    interceptor::registry::Registry,
    media::Sample,
    peer_connection::{peer_connection_state::RTCPeerConnectionState, RTCPeerConnection},
    rtp::{
        extension::{
            abs_send_time_extension::AbsSendTimeExtension,
            playout_delay_extension::PlayoutDelayExtension, HeaderExtension,
        },
        header::Header,
        packet::Packet,
        packetizer::Payloader,
    },
    rtp_transceiver::{
        rtp_codec::{
            RTCRtpCodecCapability, RTCRtpCodecParameters, RTCRtpHeaderExtensionCapability,
            RTPCodecType,
        },
        RTCPFeedback,
    },
    sdp::extmap::ABS_SEND_TIME_URI,
    track::track_local::{
        track_local_static_rtp::TrackLocalStaticRTP,
        track_local_static_sample::TrackLocalStaticSample,
    },
};

use crate::input::{InboundPacket, KeyAction, MouseButton, MouseButtonAction, TransportChannel};
use crate::pairing::AgentConfig;
use crate::video::h264::payloader::H264Payloader;
use crate::video::h264::reader::H264Reader;
use crate::video::h265::payloader::H265Payloader;
use crate::video::h265::reader::H265Reader;
use crate::video::trim_bytes_to_range;
use webrtc::api::media_engine::{MIME_TYPE_AV1, MIME_TYPE_H264, MIME_TYPE_HEVC};
use webrtc::rtp::codecs::av1::Av1Payloader;

// We reuse the same channel IDs
use crate::input::TransportChannelId;

pub struct BridgeSession {
    pub peer_connection: Arc<RTCPeerConnection>,
    pub webtransport_port: Option<u16>,
    pub webtransport_cert_hash: Option<String>,
    pub _webtransport_endpoint:
        Option<Arc<wtransport::Endpoint<wtransport::endpoint::endpoint_side::Server>>>,
    pub webtransport_shutdown: Option<tokio::sync::oneshot::Sender<()>>,
    pub pipeline_cmd: Option<tokio::sync::mpsc::Sender<lunaris_media::pipeline::PipelineCommand>>,
    pub spawned_tasks: Vec<tokio::task::JoinHandle<()>>,
}

impl Drop for BridgeSession {
    fn drop(&mut self) {
        info!(
            "BridgeSession is being dropped, cleaning up {} spawned tasks...",
            self.spawned_tasks.len()
        );
        // Abort all spawned tasks first to release Arc references
        for handle in self.spawned_tasks.drain(..) {
            handle.abort();
        }
        if let Some(shutdown_tx) = self.webtransport_shutdown.take() {
            info!("Sending shutdown signal to WebTransport accept loop...");
            let _ = shutdown_tx.send(());
        }
        if let Some(_endpoint) = self._webtransport_endpoint.take() {
            info!("Dropping WebTransport endpoint reference...");
        }
        if let Some(cmd_tx) = self.pipeline_cmd.take() {
            info!("Stopping MediaPipeline in BridgeSession Drop...");
            let _ = cmd_tx.try_send(lunaris_media::pipeline::PipelineCommand::Stop);
        }
        // Close peer connection to release ICE, DTLS, SCTP resources
        let pc = self.peer_connection.clone();
        tokio::spawn(async move {
            if let Err(e) = pc.close().await {
                warn!("Error closing peer connection: {:?}", e);
            }
        });
    }
}

pub struct VideoFramePayload {
    pub full_frame: Vec<u8>,
    pub timestamp: u32,
}

pub enum VideoPayloader {
    H264(H264Payloader),
    H265(H265Payloader),
    Av1(Av1Payloader),
}

pub async fn setup_bridge_session(
    agent_config: AgentConfig,
    client_id: String,
    ws_tx: mpsc::UnboundedSender<common::AgentMessage>,
    width: Option<u32>,
    height: Option<u32>,
    fps: Option<u32>,
    bitrate: Option<u32>,
    codec: Option<String>,
    encoder: Option<String>,
    display_id: Option<String>,
    virtual_display: Option<bool>,
    ice_servers: Option<Vec<common::RtcIceServer>>,
) -> Result<Arc<BridgeSession>> {
    info!("Media library active: host cursor visible by default (toggle with Ctrl+Alt+Shift+N)");
    // Do NOT hide host cursor by default — keeping cursor visible ensures:
    // 1. NvFBC captures cursor movement as new frames → maintains 60 FPS on static screens
    // 2. Desktop browser users can see their cursor in the stream
    // Users can still toggle with Ctrl+Alt+Shift+N when needed (e.g. mobile trackpad mode)

    let (input_tx, mut input_rx) =
        tokio::sync::mpsc::unbounded_channel::<(InboundPacket, Arc<std::sync::atomic::AtomicU32>)>(
        );
    let (key_input_tx, mut key_input_rx) =
        tokio::sync::mpsc::unbounded_channel::<(InboundPacket, Arc<std::sync::atomic::AtomicU32>)>(
        );

    let resolution_width = width.unwrap_or(1920);
    let resolution_height = height.unwrap_or(1080);
    let stream_fps = fps.unwrap_or(60);
    let stream_bitrate = bitrate.unwrap_or(8000);
    let mut codec_str = codec.as_deref().unwrap_or("h264").to_lowercase();

    // 1. Setup WebRTC PeerConnection
    let mut api_settings = SettingEngine::default();
    api_settings.set_include_loopback_candidate(true);
    api_settings.set_network_types(vec![
        webrtc::ice::network_type::NetworkType::Udp4,
        webrtc::ice::network_type::NetworkType::Tcp4,
    ]);

    let mut api_media = MediaEngine::default();

    // Register H264
    api_media.register_codec(
        RTCRtpCodecParameters {
            capability: RTCRtpCodecCapability {
                mime_type: MIME_TYPE_H264.to_string(),
                clock_rate: 90000,
                channels: 0,
                sdp_fmtp_line:
                    // Use 42002a (Baseline, Level 4.2) to support both CBP (Constrained Baseline)
                    // and AMD AMF standard Baseline (which outputs profile_idc=66, constraints=0x40).
                    // Chrome's WebRTC decoder supports 42002a natively and will accept the 42402a
                    // AMD stream without rejecting it or entering an infinite PLI loop.
                    "level-asymmetry-allowed=1;packetization-mode=1;profile-level-id=42002a"
                        .to_string(),
                rtcp_feedback: vec![
                    RTCPFeedback {
                        typ: "nack".to_string(),
                        parameter: "".to_string(),
                    },
                    RTCPFeedback {
                        typ: "nack".to_string(),
                        parameter: "pli".to_string(),
                    },
                    RTCPFeedback {
                        typ: "goog-remb".to_string(),
                        parameter: "".to_string(),
                    },
                ],
            },
            payload_type: 96,
            ..Default::default()
        },
        RTPCodecType::Video,
    )?;

    // Register HEVC (H265)
    api_media.register_codec(
        RTCRtpCodecParameters {
            capability: RTCRtpCodecCapability {
                mime_type: MIME_TYPE_HEVC.to_string(),
                clock_rate: 90000,
                channels: 0,
                sdp_fmtp_line: "profile-id=1;tier-flag=0;level-id=120;tx-mode=SRST".to_string(),
                rtcp_feedback: vec![
                    RTCPFeedback {
                        typ: "nack".to_string(),
                        parameter: "".to_string(),
                    },
                    RTCPFeedback {
                        typ: "nack".to_string(),
                        parameter: "pli".to_string(),
                    },
                    RTCPFeedback {
                        typ: "goog-remb".to_string(),
                        parameter: "".to_string(),
                    },
                ],
            },
            payload_type: 98,
            ..Default::default()
        },
        RTPCodecType::Video,
    )?;

    // Register AV1
    api_media.register_codec(
        RTCRtpCodecParameters {
            capability: RTCRtpCodecCapability {
                mime_type: MIME_TYPE_AV1.to_string(),
                clock_rate: 90000,
                channels: 0,
                sdp_fmtp_line: "profile=0".to_string(),
                rtcp_feedback: vec![
                    RTCPFeedback {
                        typ: "nack".to_string(),
                        parameter: "".to_string(),
                    },
                    RTCPFeedback {
                        typ: "nack".to_string(),
                        parameter: "pli".to_string(),
                    },
                    RTCPFeedback {
                        typ: "goog-remb".to_string(),
                        parameter: "".to_string(),
                    },
                ],
            },
            payload_type: 102,
            ..Default::default()
        },
        RTPCodecType::Video,
    )?;

    // Register Opus
    api_media.register_codec(
        RTCRtpCodecParameters {
            capability: RTCRtpCodecCapability {
                mime_type: "audio/opus".to_string(),
                clock_rate: 48000,
                channels: 2,
                sdp_fmtp_line: "minptime=10;useinbandfec=1".to_string(),
                rtcp_feedback: vec![],
            },
            payload_type: 111,
            ..Default::default()
        },
        RTPCodecType::Audio,
    )?;

    // Register RTP header extensions — critical for low-latency video:
    // PlayoutDelay(0,0): tells browser jitter buffer to use ZERO playout delay.
    //   Without this, Chrome's jitter buffer grows over time, adding increasing latency.
    //   This is the root cause of "mouse gets slower over time".
    // AbsSendTime: helps browser's congestion control (REMB) estimate bandwidth.
    const PLAYOUT_DELAY_URI: &str = "http://www.webrtc.org/experiments/rtp-hdrext/playout-delay";
    api_media.register_header_extension(
        RTCRtpHeaderExtensionCapability {
            uri: PLAYOUT_DELAY_URI.to_string(),
        },
        RTPCodecType::Video,
        None,
    )?;
    api_media.register_header_extension(
        RTCRtpHeaderExtensionCapability {
            uri: ABS_SEND_TIME_URI.to_string(),
        },
        RTPCodecType::Video,
        None,
    )?;

    let mut registry = Registry::new();
    registry = register_default_interceptors(registry, &mut api_media)?;

    let api = APIBuilder::new()
        .with_setting_engine(api_settings)
        .with_media_engine(api_media)
        .with_interceptor_registry(registry)
        .build();

    let webrtc_ice_servers = if let Some(servers) = ice_servers {
        servers
            .into_iter()
            .map(|s| webrtc::ice_transport::ice_server::RTCIceServer {
                urls: s.urls,
                username: s.username.unwrap_or_default(),
                credential: s.credential.unwrap_or_default(),
                ..Default::default()
            })
            .collect()
    } else {
        vec![
            webrtc::ice_transport::ice_server::RTCIceServer {
                urls: vec!["stun:stun.l.google.com:19302".to_string()],
                ..Default::default()
            },
            webrtc::ice_transport::ice_server::RTCIceServer {
                urls: vec!["stun:stun1.l.google.com:19302".to_string()],
                ..Default::default()
            },
        ]
    };

    let rtc_config = webrtc::peer_connection::configuration::RTCConfiguration {
        ice_servers: webrtc_ice_servers,
        ..Default::default()
    };
    let peer_connection = Arc::new(api.new_peer_connection(rtc_config).await?);

    // Resolve codec settings with host capability checks
    let encoders = lunaris_media::encode::list_available_encoders();
    let hevc_supported = encoders.iter().any(|e| {
        e.supported_codecs
            .contains(&lunaris_media::VideoCodec::H265)
    });
    let av1_supported = encoders
        .iter()
        .any(|e| e.supported_codecs.contains(&lunaris_media::VideoCodec::AV1));
    if codec_str == "av1" && !av1_supported {
        info!("AV1 requested but host doesn't support it, falling back to H.264");
        codec_str = "h264".to_string();
    }
    if codec_str == "h265" && !hevc_supported {
        info!("H.265 requested but host doesn't support it, falling back to H.264");
        codec_str = "h264".to_string();
    }

    let (mime_type, sdp_fmtp_line, payload_type, payloader) = match codec_str.as_str() {
        "h265" => (
            MIME_TYPE_HEVC.to_string(),
            "profile-id=1;tier-flag=0;level-id=120;tx-mode=SRST".to_string(),
            98,
            VideoPayloader::H265(H265Payloader::default()),
        ),
        "av1" => (
            MIME_TYPE_AV1.to_string(),
            "profile=0".to_string(),
            102,
            VideoPayloader::Av1(Av1Payloader::default()),
        ),
        _ => (
            MIME_TYPE_H264.to_string(),
            "level-asymmetry-allowed=1;packetization-mode=1;profile-level-id=42002a".to_string(),
            96,
            VideoPayloader::H264(H264Payloader::default()),
        ),
    };

    info!(
        "Configured WebRTC video track codec: {} (Mime: {}, PT: {})",
        codec_str, mime_type, payload_type
    );

    // Create video and audio tracks
    let video_track = Arc::new(TrackLocalStaticRTP::new(
        RTCRtpCodecCapability {
            mime_type,
            clock_rate: 90000,
            sdp_fmtp_line,
            ..Default::default()
        },
        "video".to_string(),
        "lunaris".to_string(),
    ));

    let audio_track = Arc::new(TrackLocalStaticSample::new(
        RTCRtpCodecCapability {
            mime_type: "audio/opus".to_string(),
            clock_rate: 48000,
            channels: 2,
            ..Default::default()
        },
        "audio".to_string(),
        "lunaris".to_string(),
    ));

    let video_sender = peer_connection
        .add_track(
            video_track.clone() as Arc<dyn webrtc::track::track_local::TrackLocal + Send + Sync>
        )
        .await?;
    peer_connection
        .add_track(
            audio_track.clone() as Arc<dyn webrtc::track::track_local::TrackLocal + Send + Sync>
        )
        .await?;

    let pipeline_cmd_tx_shared = Arc::new(std::sync::Mutex::new(
        None::<tokio::sync::mpsc::Sender<lunaris_media::pipeline::PipelineCommand>>,
    ));
    let mut spawned_tasks: Vec<tokio::task::JoinHandle<()>> = Vec::new();
    let pipeline_cmd_tx_for_pli = pipeline_cmd_tx_shared.clone();

    let need_idr_flag = Arc::new(AtomicBool::new(true));
    let need_idr_clone = need_idr_flag.clone();
    let video_sender_clone = video_sender.clone();
    let pli_task = tokio::spawn(async move {
        let mut buf = vec![0u8; 1500];
        let mut last_bitrate_kbps: u32 = stream_bitrate;
        let mut last_bitrate_change = Instant::now();
        let remb_started = Instant::now();
        let mut low_remb_samples: u32 = 0;
        // PLI debounce: only generate one IDR per second from PLI requests.
        // Without debouncing, on slow networks (low REMB), PLI every 200ms causes
        // 5 large IDR floods/sec, saturating the network and making REMB unable to climb.
        let mut last_pli_idr_time = std::time::Instant::now()
            .checked_sub(std::time::Duration::from_secs(60))
            .unwrap_or_else(std::time::Instant::now);
        while let Ok((packets, _)) = video_sender_clone.read(&mut buf).await {
            for packet in packets {
                let packet_any = packet.as_any();
                if packet_any.is::<webrtc::rtcp::payload_feedbacks::picture_loss_indication::PictureLossIndication>() {
                    let now = std::time::Instant::now();
                    let elapsed_ms = now.duration_since(last_pli_idr_time).as_millis();
                    if elapsed_ms >= 1000 {
                        // Enough time has passed — honor this PLI and generate IDR.
                        info!("Received PLI request from browser, requesting IDR frame (last IDR {}ms ago)", elapsed_ms);
                        last_pli_idr_time = now;
                        need_idr_clone.store(true, Ordering::SeqCst);
                        if let Some(ref cmd_tx) = *pipeline_cmd_tx_for_pli.lock().unwrap() {
                            let _ = cmd_tx.try_send(lunaris_media::pipeline::PipelineCommand::RequestKeyframe);
                        }
                    } else {
                        // Too soon — suppress this PLI to avoid IDR flood on slow networks.
                        debug!("PLI from browser suppressed: last IDR only {}ms ago (debounce=1000ms)", elapsed_ms);
                    }
                } else if let Some(remb) = packet_any.downcast_ref::<webrtc::rtcp::payload_feedbacks::receiver_estimated_maximum_bitrate::ReceiverEstimatedMaximumBitrate>() {
                    let estimated_bitrate_kbps = (remb.bitrate / 1000.0) as u32;

                    // Chrome can report very low REMB during startup before the decoder/jitter
                    // buffer settles. For realtime desktop streaming, avoid collapsing 1080p60
                    // into sub-megabit mode unless several low samples arrive after warmup.
                    let realtime_floor = if stream_fps >= 50 && resolution_width >= 1920 {
                        6_000
                    } else if stream_fps >= 50 {
                        3_000
                    } else {
                        1_500
                    };
                    let bitrate_floor = stream_bitrate
                        .min((stream_bitrate / 3).max(realtime_floor).max(1_000));
                    let clamped_bitrate_kbps = estimated_bitrate_kbps.clamp(bitrate_floor, stream_bitrate);
                    let now = Instant::now();
                    if now.duration_since(remb_started) < Duration::from_secs(5)
                        && estimated_bitrate_kbps < bitrate_floor
                    {
                        debug!(
                            "Ignoring startup REMB below realtime floor: estimated={} kbps, floor={} kbps, max={} kbps",
                            estimated_bitrate_kbps, bitrate_floor, stream_bitrate
                        );
                        continue;
                    }
                    if clamped_bitrate_kbps < last_bitrate_kbps
                        && clamped_bitrate_kbps < stream_bitrate.saturating_mul(2) / 3
                    {
                        low_remb_samples += 1;
                        if low_remb_samples < 3 {
                            debug!(
                                "Waiting for stable low REMB before bitrate drop: sample {}/3, estimated={} kbps, target={} kbps",
                                low_remb_samples, estimated_bitrate_kbps, clamped_bitrate_kbps
                            );
                            continue;
                        }
                    } else if clamped_bitrate_kbps >= last_bitrate_kbps {
                        low_remb_samples = 0;
                    }

                    // Rate-limit bitrate changes: only apply if value changed AND at least 2 seconds since last change.
                    // Rapid bitrate oscillation destabilizes hardware encoders and can cause video corruption.
                    if clamped_bitrate_kbps != last_bitrate_kbps && now.duration_since(last_bitrate_change).as_secs() >= 2 {
                        info!(
                            "REMB bitrate change: {} kbps -> {} kbps (estimated={} kbps, floor={}, max={})",
                            last_bitrate_kbps, clamped_bitrate_kbps, estimated_bitrate_kbps, bitrate_floor, stream_bitrate
                        );
                        if let Some(ref cmd_tx) = *pipeline_cmd_tx_for_pli.lock().unwrap() {
                            let _ = cmd_tx.try_send(lunaris_media::pipeline::PipelineCommand::SetBitrate(clamped_bitrate_kbps));
                        }
                        last_bitrate_kbps = clamped_bitrate_kbps;
                        last_bitrate_change = now;
                        low_remb_samples = 0;
                    }
                }
            }
        }
    });
    spawned_tasks.push(pli_task);

    let webrtc_connected = Arc::new(AtomicBool::new(false));
    let webrtc_connected_clone = webrtc_connected.clone();
    // Handle Peer Connection state changes to clean up session
    let pipeline_cmd_tx_for_state_change = pipeline_cmd_tx_shared.clone();
    peer_connection.on_peer_connection_state_change(Box::new(
        move |state: RTCPeerConnectionState| {
            let webrtc_connected = webrtc_connected_clone.clone();
            let pipeline_cmd_tx = pipeline_cmd_tx_for_state_change.clone();
            Box::pin(async move {
                info!("WebRTC connection state changed to: {}", state);
                if state == RTCPeerConnectionState::Connected {
                    webrtc_connected.store(true, Ordering::SeqCst);
                    if let Some(ref cmd_tx) = *pipeline_cmd_tx.lock().unwrap() {
                        info!("WebRTC connected! Requesting immediate IDR keyframe from media pipeline.");
                        let _ = cmd_tx.try_send(lunaris_media::pipeline::PipelineCommand::RequestKeyframe);
                    }
                } else if state == RTCPeerConnectionState::Closed
                    || state == RTCPeerConnectionState::Failed
                    || state == RTCPeerConnectionState::Disconnected
                {
                    webrtc_connected.store(false, Ordering::SeqCst);

                    // Stop MediaPipeline if it is active
                    if let Some(ref cmd_tx) = *pipeline_cmd_tx.lock().unwrap() {
                        info!("WebRTC connection state is inactive ({}). Stopping MediaPipeline...", state);
                        let _ = cmd_tx.try_send(lunaris_media::pipeline::PipelineCommand::Stop);
                    }
                }
            })
        },
    ));

    // Create Data Channels
    let general_channel = peer_connection.create_data_channel("general", None).await?;
    let general_channel_for_cursor = general_channel.clone();

    let cursor_channel_init = RTCDataChannelInit {
        ordered: Some(false),
        max_retransmits: Some(0),
        ..Default::default()
    };
    let cursor_channel = peer_connection
        .create_data_channel("cursor", Some(cursor_channel_init))
        .await?;
    let cursor_channel_for_cursor = cursor_channel.clone();
    let latest_host_cursor_payload = Arc::new(tokio::sync::Mutex::new(None::<String>));
    let latest_host_cursor_on_open = latest_host_cursor_payload.clone();
    let cursor_channel_on_open = cursor_channel.clone();
    cursor_channel.on_open(Box::new(move || {
        let latest_host_cursor_on_open = latest_host_cursor_on_open.clone();
        let cursor_channel_on_open = cursor_channel_on_open.clone();
        Box::pin(async move {
            if let Some(payload) = latest_host_cursor_on_open.lock().await.clone() {
                if let Err(err) = cursor_channel_on_open.send_text(payload).await {
                    trace!("Failed to replay cached host cursor update: {:?}", err);
                }
            }
        })
    }));

    let mouse_reliable_init = RTCDataChannelInit {
        ordered: Some(false),
        ..Default::default()
    };
    let mouse_reliable_channel = peer_connection
        .create_data_channel("mouse_reliable", Some(mouse_reliable_init))
        .await?;

    let mouse_absolute_init = RTCDataChannelInit {
        ordered: Some(false),
        max_retransmits: Some(0),
        ..Default::default()
    };
    let mouse_absolute_channel = peer_connection
        .create_data_channel("mouse_absolute", Some(mouse_absolute_init))
        .await?;

    let mouse_relative_init = RTCDataChannelInit {
        ordered: Some(false),
        max_retransmits: Some(0),
        ..Default::default()
    };
    let mouse_relative_channel = peer_connection
        .create_data_channel("mouse_relative", Some(mouse_relative_init))
        .await?;
    let keyboard_channel = peer_connection
        .create_data_channel("keyboard", None)
        .await?;

    // Handle messages on Data Channels
    setup_data_channel_handler(
        general_channel,
        TransportChannel(TransportChannelId::GENERAL),
        Some(input_tx.clone()),
        Some(pipeline_cmd_tx_shared.clone()),
    );
    setup_data_channel_handler(
        mouse_reliable_channel,
        TransportChannel(TransportChannelId::MOUSE_RELIABLE),
        Some(input_tx.clone()),
        None,
    );
    setup_data_channel_handler(
        mouse_absolute_channel,
        TransportChannel(TransportChannelId::MOUSE_ABSOLUTE),
        Some(input_tx.clone()),
        None,
    );
    setup_data_channel_handler(
        mouse_relative_channel,
        TransportChannel(TransportChannelId::MOUSE_RELATIVE),
        Some(input_tx.clone()),
        None,
    );
    setup_data_channel_handler(
        keyboard_channel,
        TransportChannel(TransportChannelId::KEYBOARD),
        Some(key_input_tx.clone()),
        None,
    );

    // ICE Candidate callback
    let ws_tx_clone = ws_tx.clone();
    let client_id_clone = client_id.clone();
    peer_connection.on_ice_candidate(Box::new(move |candidate: Option<RTCIceCandidate>| {
        let ws_tx = ws_tx_clone.clone();
        let client_id = client_id_clone.clone();
        Box::pin(async move {
            if let Some(cand) = candidate {
                if let Ok(json_cand) = cand.to_json() {
                    let msg =
                        common::AgentMessage::Signaling(common::SignalingMessage::IceCandidate {
                            target_id: client_id,
                            candidate: common::RtcIceCandidate {
                                candidate: json_cand.candidate,
                                sdp_mid: json_cand.sdp_mid,
                                sdp_mline_index: json_cand.sdp_mline_index,
                                username_fragment: json_cand.username_fragment,
                            },
                        });
                    let _ = ws_tx.send(msg);
                }
            }
        })
    }));

    // ICE Gathering state change callback — helps diagnose ICE failures in production logs
    peer_connection.on_ice_gathering_state_change(Box::new(
        move |state: webrtc::ice_transport::ice_gatherer_state::RTCIceGathererState| {
            Box::pin(async move {
                warn!("[ICE] Gathering state changed: {:?}", state);
            })
        },
    ));

    // ICE Connection state change callback — logs actual ICE connectivity progress
    peer_connection.on_ice_connection_state_change(Box::new(
        move |state: webrtc::ice_transport::ice_connection_state::RTCIceConnectionState| {
            Box::pin(async move {
                warn!("[ICE] Connection state changed: {:?}", state);
            })
        },
    ));

    let (video_frame_tx, mut video_frame_rx) = tokio::sync::mpsc::channel::<VideoFramePayload>(4);
    let (audio_tx, mut audio_rx) = tokio::sync::mpsc::channel::<Sample>(256);

    let audio_track_clone = audio_track.clone();
    let audio_writer_task = tokio::spawn(async move {
        while let Some(sample) = audio_rx.recv().await {
            if let Err(e) = audio_track_clone.write_sample(&sample).await {
                debug!("Failed to write audio sample: {:?}", e);
            }
        }
    });
    spawned_tasks.push(audio_writer_task);

    let media_codec = match codec_str.as_str() {
        "h265" => lunaris_media::types::VideoCodec::H265,
        "av1" => lunaris_media::types::VideoCodec::AV1,
        _ => lunaris_media::types::VideoCodec::H264,
    };

    let media_config = lunaris_media::types::StreamConfig {
        width: resolution_width,
        height: resolution_height,
        fps: stream_fps,
        codec: media_codec,
        bitrate_kbps: stream_bitrate,
        pixel_format: lunaris_media::types::PixelFormat::NV12,
        preferred_encoder: encoder.clone(),
        virtual_display: virtual_display.unwrap_or(false),
    };

    let (pipeline, mut media_event_rx, pipeline_cmd_tx) =
        lunaris_media::pipeline::MediaPipeline::new(media_config);

    *pipeline_cmd_tx_shared.lock().unwrap() = Some(pipeline_cmd_tx);

    let video_frame_tx_clone = video_frame_tx.clone();
    let audio_tx_clone = audio_tx.clone();

    let pipeline_task = tokio::spawn(async move {
        info!("Running lunaris-media pipeline...");
        let display = display_id.clone().unwrap_or_else(|| "default".to_string());
        if let Err(e) = pipeline.run(&display).await {
            error!("Media pipeline exited with error: {:?}", e);
        }
    });
    spawned_tasks.push(pipeline_task);

    if let Some(injector) = lunaris_media::input::InputInjector::new() {
        let injector = std::sync::Arc::new(injector);
        let injector_clone = injector.clone();

        // Spawn separate task for mouse inputs
        let input_task = tokio::spawn(async move {
            info!("InputInjector: started mouse input processing task");
            while let Some(first_packet) = input_rx.recv().await {
                process_mouse_input_batch(first_packet, &mut input_rx, &injector);
                injector.flush();
            }
            info!("InputInjector: stopped mouse input processing task");
        });
        spawned_tasks.push(input_task);

        // Spawn separate task for keyboard inputs
        let key_input_task = tokio::spawn(async move {
            info!("InputInjector: started keyboard input processing task");
            while let Some((packet, last_timestamp)) = key_input_rx.recv().await {
                handle_inbound_packet(packet, &injector_clone, &last_timestamp);
            }
            info!("InputInjector: stopped keyboard input processing task");
        });
        spawned_tasks.push(key_input_task);
    } else {
        error!("InputInjector: failed to initialize X11 input injector");
    }

    let start_time = Instant::now();
    let webrtc_connected_for_media = webrtc_connected.clone();
    let ws_tx_for_media = ws_tx.clone();
    let client_id_for_media = client_id.clone();
    let cursor_channel_for_media = cursor_channel_for_cursor.clone();
    let general_channel_for_media = general_channel_for_cursor.clone();
    let latest_host_cursor_for_media = latest_host_cursor_payload.clone();
    let media_event_task = tokio::spawn(async move {
        let mut metrics_started = Instant::now();
        let mut forwarded_frames: u64 = 0;
        let mut dropped_frames: u64 = 0;
        let mut forwarded_bytes: u64 = 0;
        let mut latest_cursor_image: Option<serde_json::Value> = None;
        while let Some(event) = media_event_rx.recv().await {
            match event {
                lunaris_media::pipeline::MediaEvent::EncoderStarted {
                    encoder,
                    gpu_name,
                    requested_encoder,
                } => {
                    info!(
                        "Active media encoder: {} ({}) on {} requested={}",
                        encoder.name,
                        encoder.hw_type,
                        gpu_name.as_deref().unwrap_or("unknown GPU"),
                        requested_encoder.as_deref().unwrap_or("auto")
                    );
                    let _ = ws_tx_for_media.send(common::AgentMessage::Signaling(
                        common::SignalingMessage::EncoderStatus {
                            target_id: client_id_for_media.clone(),
                            encoder: encoder.name,
                            hw_type: encoder.hw_type.to_string(),
                            gpu_info: gpu_name,
                            requested_encoder,
                            host_os: Some(std::env::consts::OS.to_string()),
                        },
                    ));
                }
                lunaris_media::pipeline::MediaEvent::VideoFrame(frame) => {
                    // Don't queue video frames before WebRTC is connected
                    if !webrtc_connected_for_media.load(Ordering::Relaxed) {
                        continue;
                    }
                    let elapsed = start_time.elapsed();
                    let timestamp = (elapsed.as_nanos() * 90000 / 1_000_000_000) as u32;
                    let payload = VideoFramePayload {
                        full_frame: frame.data,
                        timestamp,
                    };
                    let frame_size = payload.full_frame.len() as u64;
                    trace!(
                        "Received VideoFrame from media pipeline: size={}, timestamp={}",
                        frame_size,
                        timestamp
                    );
                    if let Err(e) = video_frame_tx_clone.try_send(payload) {
                        dropped_frames += 1;
                        warn!("Failed to forward video frame to packager: {:?}", e);
                    } else {
                        forwarded_frames += 1;
                        forwarded_bytes += frame_size;
                    }
                    let elapsed = metrics_started.elapsed();
                    if elapsed >= Duration::from_secs(1) {
                        let secs = elapsed.as_secs_f64();
                        info!(
                            "Agent media ingress: forwarded={:.1}/s dropped={} bitrate={:.2}Mbps packager_queue={}",
                            forwarded_frames as f64 / secs,
                            dropped_frames,
                            (forwarded_bytes as f64 * 8.0 / secs) / 1_000_000.0,
                            video_frame_tx_clone.max_capacity() - video_frame_tx_clone.capacity()
                        );
                        metrics_started = Instant::now();
                        forwarded_frames = 0;
                        dropped_frames = 0;
                        forwarded_bytes = 0;
                    }
                }
                lunaris_media::pipeline::MediaEvent::AudioFrame(frame) => {
                    let duration = Duration::from_micros(frame.duration);
                    let sample = Sample {
                        data: bytes::Bytes::copy_from_slice(&frame.data),
                        duration,
                        ..Default::default()
                    };
                    let _ = audio_tx_clone.try_send(sample);
                }
                lunaris_media::pipeline::MediaEvent::CursorUpdate(cursor) => {
                    let cursor_kind = cursor.kind.as_str();
                    let image = cursor.image.as_ref().map(|image| {
                        serde_json::json!({
                            "width": image.width,
                            "height": image.height,
                            "hotspot_x": image.hotspot_x,
                            "hotspot_y": image.hotspot_y,
                            "rgba": base64::engine::general_purpose::STANDARD.encode(&image.rgba_data),
                        })
                    });
                    if let Some(image) = image.as_ref() {
                        latest_cursor_image = Some(image.clone());
                    }
                    let realtime_payload = serde_json::json!({
                        "type": "host_cursor",
                        "x": cursor.x,
                        "y": cursor.y,
                        "visible": cursor.visible,
                        "kind": cursor_kind,
                        "image": image,
                    })
                    .to_string();
                    let cached_payload = serde_json::json!({
                        "type": "host_cursor",
                        "x": cursor.x,
                        "y": cursor.y,
                        "visible": cursor.visible,
                        "kind": cursor_kind,
                        "image": latest_cursor_image.clone(),
                    })
                    .to_string();
                    *latest_host_cursor_for_media.lock().await = Some(cached_payload.clone());

                    if image.is_some()
                        && general_channel_for_media.ready_state() == RTCDataChannelState::Open
                    {
                        if let Err(err) = general_channel_for_media.send_text(cached_payload).await
                        {
                            trace!(
                                "Failed to send reliable host cursor image update: {:?}",
                                err
                            );
                        }
                    }

                    if cursor_channel_for_media.ready_state() == RTCDataChannelState::Open {
                        if let Err(err) = cursor_channel_for_media.send_text(realtime_payload).await
                        {
                            trace!("Failed to send host cursor update: {:?}", err);
                        }
                    }
                }
                lunaris_media::pipeline::MediaEvent::Started => {
                    info!("Media pipeline started successfully");
                }
                lunaris_media::pipeline::MediaEvent::Stopped => {
                    info!("Media pipeline stopped");
                }
                lunaris_media::pipeline::MediaEvent::Error(err_str) => {
                    warn!("Media pipeline warning/error: {}", err_str);
                }
            }
        }
    });
    spawned_tasks.push(media_event_task);
    let video_track_clone = video_track.clone();
    let need_idr_clone_worker = need_idr_flag.clone();
    let pipeline_cmd_tx_for_writer = pipeline_cmd_tx_shared.clone();

    let video_packager_task = tokio::spawn(async move {
        let mut frame_count: u64 = 0;
        let mut payloader = payloader;

        // Bounded channel for complete RTP frames. Keep this small so the browser sees
        // current frames instead of a delayed backlog when networking briefly stalls.
        const RTP_FRAME_QUEUE_CAPACITY: usize = 12;
        const RTP_FRAME_QUEUE_DROP_THRESHOLD: usize = 6;
        let (packet_tx, mut packet_rx) =
            tokio::sync::mpsc::channel::<Vec<Packet>>(RTP_FRAME_QUEUE_CAPACITY);

        let need_idr_clone_writer = need_idr_clone_worker.clone();
        let pipeline_cmd_tx_for_writer = pipeline_cmd_tx_for_writer.clone();
        tokio::spawn(async move {
            let mut seq: u16 = 0;
            let mut metrics_started = Instant::now();
            let mut sent_frames: u64 = 0;
            let mut sent_packets: u64 = 0;
            let mut discarded_frames: u64 = 0;
            while let Some(mut packets) = packet_rx.recv().await {
                let queue_len = packet_rx.len();
                if queue_len > RTP_FRAME_QUEUE_DROP_THRESHOLD {
                    let mut discarded_count = 0;
                    while let Ok(next_packets) = packet_rx.try_recv() {
                        packets = next_packets;
                        discarded_count += 1;
                    }
                    if discarded_count > 0 {
                        discarded_frames += discarded_count;
                        warn!(
                            "Discarding {} queued video frames to prevent latency buildup (queue_len={})",
                            discarded_count,
                            queue_len
                        );
                        need_idr_clone_writer.store(true, Ordering::SeqCst);
                        if let Some(ref cmd_tx) = *pipeline_cmd_tx_for_writer.lock().unwrap() {
                            let _ = cmd_tx.try_send(
                                lunaris_media::pipeline::PipelineCommand::RequestKeyframe,
                            );
                        }
                    }
                }

                // Compute AbsSendTime for congestion control (REMB)
                let now = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default();
                let now_secs = now.as_secs() as f64 + now.subsec_nanos() as f64 * 1e-9;
                let abs_send_time: u64 = (now_secs * 262_144.0) as u64;

                let extensions = [
                    // PlayoutDelay(0, 0): min=0, max=0 playout delay.
                    // Tells Chrome to use ZERO jitter buffer delay — critical for
                    // preventing latency accumulation that makes mouse feel slower.
                    HeaderExtension::PlayoutDelay(PlayoutDelayExtension::new(0, 0)),
                    HeaderExtension::AbsSendTime(AbsSendTimeExtension {
                        timestamp: abs_send_time,
                    }),
                ];

                let packet_count = packets.len() as u64;
                trace!(
                    "Writer task sending {} RTP packets (first seq={})",
                    packets.len(),
                    seq
                );
                for packet in &mut packets {
                    packet.header.sequence_number = seq;
                    seq = seq.wrapping_add(1);
                    if let Err(e) = video_track_clone
                        .write_rtp_with_extensions(packet, &extensions)
                        .await
                    {
                        debug!("Failed to write video RTP packet: {:?}", e);
                    }
                }
                sent_frames += 1;
                sent_packets += packet_count;
                let elapsed = metrics_started.elapsed();
                if elapsed >= Duration::from_secs(1) {
                    let secs = elapsed.as_secs_f64();
                    info!(
                        "RTP writer metrics: sent_frames={:.1}/s sent_packets={:.1}/s discarded={} queue={}",
                        sent_frames as f64 / secs,
                        sent_packets as f64 / secs,
                        discarded_frames,
                        packet_rx.len()
                    );
                    metrics_started = Instant::now();
                    sent_frames = 0;
                    sent_packets = 0;
                    discarded_frames = 0;
                }
                trace!("Writer task successfully sent RTP packets");
            }
        });

        while let Some(VideoFramePayload {
            full_frame,
            timestamp,
        }) = video_frame_rx.recv().await
        {
            trace!(
                "Enqueuer received frame from rx: size={}, timestamp={}",
                full_frame.len(),
                timestamp
            );
            let mut samples = Vec::new();
            match &mut payloader {
                VideoPayloader::H264(_) => {
                    let mut reader = H264Reader::new(Cursor::new(full_frame), 0);
                    while let Ok(Some(nal)) = reader.next_nal() {
                        if nal.header.nal_unit_type == crate::video::h264::NalUnitType::FillerData {
                            continue;
                        }
                        let data = trim_bytes_to_range(
                            nal.full,
                            nal.header_range.start..nal.payload_range.end,
                        );
                        samples.push(data);
                    }
                }
                VideoPayloader::H265(_) => {
                    let mut reader = H265Reader::new(Cursor::new(full_frame), 0);
                    while let Ok(Some(nal)) = reader.next_nal() {
                        if nal.header.nal_unit_type
                            == crate::video::h265::reader::NalUnitType::FdNut
                        {
                            continue;
                        }
                        let data = trim_bytes_to_range(
                            nal.full,
                            nal.header_range.start..nal.payload_range.end,
                        );
                        samples.push(data);
                    }
                }
                VideoPayloader::Av1(_) => {
                    let mut reader =
                        crate::video::annexb::AnnexBSplitter::new(Cursor::new(full_frame), 0);
                    while let Ok(Some(obu)) = reader.next() {
                        let data = trim_bytes_to_range(
                            obu.full,
                            obu.payload_range.start..obu.payload_range.end,
                        );
                        samples.push(data);
                    }
                }
            }

            if frame_count % 120 == 0 {
                trace!(
                    "Background video processor: frame {}, timestamp {}, samples {}",
                    frame_count,
                    timestamp,
                    samples.len()
                );
            }

            let mut peekable = samples.drain(..).peekable();
            let mut packets = Vec::new();

            while let Some(sample) = peekable.next() {
                let payloads = match &mut payloader {
                    VideoPayloader::H264(p) => p.payload(1200 - 12, &sample.freeze()),
                    VideoPayloader::H265(p) => p.payload(1200 - 12, &sample.freeze()),
                    VideoPayloader::Av1(p) => p.payload(1200 - 12, &sample.freeze()),
                };

                let payloads = match payloads {
                    Ok(p) => p,
                    Err(e) => {
                        warn!("Failed to packetize frame: {:?}", e);
                        continue;
                    }
                };

                let len = payloads.len();
                for (i, payload) in payloads.into_iter().enumerate() {
                    let packet = Packet {
                        header: Header {
                            version: 2,
                            padding: false,
                            extension: false,
                            marker: peekable.peek().is_none() && i == len - 1,
                            sequence_number: 0, // Assigned dynamically by the writer task
                            timestamp,
                            payload_type,
                            ..Default::default()
                        },
                        payload,
                    };
                    packets.push(packet);
                }
            }

            let total_packets = packets.len();
            trace!(
                "Enqueuer finished packetizing: {} RTP packets",
                total_packets
            );
            if let Err(e) = packet_tx.try_send(packets) {
                warn!("RTP frame queue full, dropping frame: {:?}", e);
                need_idr_clone_worker.store(true, Ordering::SeqCst);
            }

            if frame_count % 120 == 0 {
                trace!(
                    "Background video processor: frame {}, sent {} RTP packets",
                    frame_count,
                    total_packets
                );
            }
            frame_count += 1;
        }
    });
    spawned_tasks.push(video_packager_task);

    // Start WebTransport Server
    let mut webtransport_port = None;
    let mut webtransport_cert_hash = None;
    let mut webtransport_endpoint = None;
    let mut webtransport_shutdown = None;

    match wtransport::Identity::self_signed(&["localhost", "127.0.0.1", "::1"]) {
        Ok(identity) => {
            let cert_hash = identity.certificate_chain().as_slice()[0]
                .hash()
                .as_ref()
                .iter()
                .map(|b| format!("{:02x}", b))
                .collect::<String>();

            let config = wtransport::ServerConfig::builder()
                .with_bind_default(agent_config.webtransport_port)
                .with_identity(identity)
                .build();

            match wtransport::Endpoint::server(config) {
                Ok(endpoint) => {
                    if let Ok(addr) = endpoint.local_addr() {
                        let port = addr.port();
                        info!("WebTransport server listening on port {}", port);
                        webtransport_port = Some(port);
                        webtransport_cert_hash = Some(cert_hash);

                        let endpoint_arc = Arc::new(endpoint);
                        webtransport_endpoint = Some(endpoint_arc.clone());

                        let (shutdown_tx, mut shutdown_rx) = tokio::sync::oneshot::channel::<()>();
                        webtransport_shutdown = Some(shutdown_tx);

                        let input_tx_clone = input_tx.clone();
                        tokio::spawn(async move {
                            loop {
                                tokio::select! {
                                    incoming = endpoint_arc.accept() => {
                                        let input_tx = input_tx_clone.clone();
                                        tokio::spawn(async move {
                                            info!(
                                                "Incoming WebTransport session from {}",
                                                incoming.remote_address()
                                            );
                                            let session_request = match incoming.await {
                                                Ok(req) => req,
                                                Err(e) => {
                                                    error!(
                                                        "WebTransport session handshake failed: {:?}",
                                                        e
                                                    );
                                                    return;
                                                }
                                            };
                                            let connection = match session_request.accept().await {
                                                Ok(conn) => conn,
                                                Err(e) => {
                                                    error!("WebTransport session accept failed: {:?}", e);
                                                    return;
                                                }
                                            };
                                            info!("WebTransport session accepted successfully");
                                            let last_timestamp = Arc::new(std::sync::atomic::AtomicU32::new(0));
                                            loop {
                                                match connection.receive_datagram().await {
                                                    Ok(datagram) => {
                                                        if datagram.len() < 1 {
                                                            continue;
                                                        }
                                                        let channel_id = datagram[0];
                                                        let payload = &datagram[1..];
                                                        let Some(packet) = InboundPacket::deserialize(
                                                            TransportChannel(channel_id),
                                                            payload,
                                                        ) else {
                                                            warn!(
                                                                "WebTransport: Failed to deserialize packet on channel {}, payload len: {}",
                                                                channel_id,
                                                                payload.len()
                                                            );
                                                            continue;
                                                        };
                                                        let _ = input_tx.send((packet, last_timestamp.clone()));
                                                    }
                                                    Err(e) => {
                                                        info!("WebTransport connection closed: {:?}", e);
                                                        break;
                                                    }
                                                }
                                            }
                                        });
                                    }
                                    _ = &mut shutdown_rx => {
                                        info!("WebTransport accept loop received shutdown signal, exiting.");
                                        break;
                                    }
                                }
                            }
                        });
                    }
                }
                Err(e) => {
                    error!("Failed to start WebTransport endpoint: {:?}", e);
                }
            }
        }
        Err(e) => {
            error!("Failed to generate self-signed certificate: {:?}", e);
        }
    }

    let pipeline_cmd = pipeline_cmd_tx_shared.lock().unwrap().clone();

    Ok(Arc::new(BridgeSession {
        peer_connection,
        webtransport_port,
        webtransport_cert_hash,
        _webtransport_endpoint: webtransport_endpoint,
        webtransport_shutdown,
        pipeline_cmd,
        spawned_tasks,
    }))
}

fn setup_data_channel_handler(
    data_channel: Arc<RTCDataChannel>,
    channel: TransportChannel,
    input_tx: Option<
        tokio::sync::mpsc::UnboundedSender<(InboundPacket, Arc<std::sync::atomic::AtomicU32>)>,
    >,
    pipeline_cmd_tx: Option<
        Arc<
            std::sync::Mutex<
                Option<tokio::sync::mpsc::Sender<lunaris_media::pipeline::PipelineCommand>>,
            >,
        >,
    >,
) {
    let last_timestamp = Arc::new(std::sync::atomic::AtomicU32::new(0));
    let label = data_channel.label().to_string();
    info!(
        "setup_data_channel_handler: label={}, channel_id={}, has_input_tx={}",
        label,
        channel.0,
        input_tx.is_some()
    );
    data_channel.on_message(Box::new(move |msg: DataChannelMessage| {
        let input_tx = input_tx.clone();
        let channel = channel;
        let last_timestamp = last_timestamp.clone();
        let pipeline_cmd_tx = pipeline_cmd_tx.clone();
        Box::pin(async move {
            let Some(packet) = InboundPacket::deserialize(channel, &msg.data) else {
                // Only warn for non-general channels (general may have unrecognized JSON commands)
                if channel.0 != 0 {
                    warn!(
                        "Failed to deserialize inbound packet on channel {}",
                        channel.0
                    );
                }
                return;
            };

            // Handle dynamic pipeline commands (SetBitrate, SetFps) directly
            if let Some(ref pipeline_cmd) = pipeline_cmd_tx {
                match &packet {
                    InboundPacket::SetBitrate { kbps } => {
                        info!("Dynamic bitrate change via data channel: {} kbps", kbps);
                        if let Some(ref cmd_tx) = *pipeline_cmd.lock().unwrap() {
                            let _ = cmd_tx.try_send(
                                lunaris_media::pipeline::PipelineCommand::SetBitrate(*kbps),
                            );
                        }
                        return;
                    }
                    InboundPacket::SetFps { fps } => {
                        info!("Dynamic FPS change via data channel: {} fps", fps);
                        if let Some(ref cmd_tx) = *pipeline_cmd.lock().unwrap() {
                            let _ = cmd_tx
                                .try_send(lunaris_media::pipeline::PipelineCommand::SetFps(*fps));
                        }
                        return;
                    }
                    _ => {}
                }
            }

            if let Some(ref tx) = input_tx {
                let _ = tx.send((packet, last_timestamp));
            }
        })
    }));
}

type InputQueueItem = (InboundPacket, Arc<std::sync::atomic::AtomicU32>);

fn process_mouse_input_batch(
    first: InputQueueItem,
    input_rx: &mut mpsc::UnboundedReceiver<InputQueueItem>,
    injector: &lunaris_media::input::InputInjector,
) {
    let mut pending = Some(first);
    while let Ok(next) = input_rx.try_recv() {
        if let Some(current) = pending.take() {
            match coalesce_mouse_input(current, next) {
                Ok(coalesced) => pending = Some(coalesced),
                Err((current, next)) => {
                    handle_inbound_packet(current.0, injector, &current.1);
                    pending = Some(next);
                }
            }
        } else {
            pending = Some(next);
        }
    }

    if let Some((packet, last_timestamp)) = pending {
        handle_inbound_packet(packet, injector, &last_timestamp);
    }
}

fn coalesce_mouse_input(
    current: InputQueueItem,
    next: InputQueueItem,
) -> Result<InputQueueItem, (InputQueueItem, InputQueueItem)> {
    let (current_packet, current_timestamp) = current;
    let (next_packet, next_timestamp) = next;
    let same_channel = Arc::ptr_eq(&current_timestamp, &next_timestamp);

    match (current_packet, next_packet) {
        (
            InboundPacket::MousePosition { .. },
            InboundPacket::MousePosition {
                x,
                y,
                reference_width,
                reference_height,
                timestamp,
            },
        ) if same_channel => Ok((
            InboundPacket::MousePosition {
                x,
                y,
                reference_width,
                reference_height,
                timestamp,
            },
            next_timestamp,
        )),
        (
            InboundPacket::MouseMove {
                delta_x: ax,
                delta_y: ay,
                timestamp: at,
            },
            InboundPacket::MouseMove {
                delta_x: bx,
                delta_y: by,
                timestamp: bt,
            },
        ) if same_channel => {
            let delta_x =
                ((ax as i32) + (bx as i32)).clamp(i16::MIN as i32, i16::MAX as i32) as i16;
            let delta_y =
                ((ay as i32) + (by as i32)).clamp(i16::MIN as i32, i16::MAX as i32) as i16;
            Ok((
                InboundPacket::MouseMove {
                    delta_x,
                    delta_y,
                    timestamp: if bt != 0 { bt } else { at },
                },
                next_timestamp,
            ))
        }
        (current_packet, next_packet) => Err((
            (current_packet, current_timestamp),
            (next_packet, next_timestamp),
        )),
    }
}

fn handle_inbound_packet(
    packet: InboundPacket,
    injector: &lunaris_media::input::InputInjector,
    last_timestamp: &std::sync::atomic::AtomicU32,
) {
    match packet {
        InboundPacket::MousePosition {
            x,
            y,
            reference_width,
            reference_height,
            timestamp,
        } => {
            let last_ts = last_timestamp.load(std::sync::atomic::Ordering::SeqCst);
            if last_ts == 0 || (timestamp.wrapping_sub(last_ts) as i32) >= 0 {
                last_timestamp.store(timestamp, std::sync::atomic::Ordering::SeqCst);
                injector.move_mouse_absolute(
                    x as i32,
                    y as i32,
                    reference_width as i32,
                    reference_height as i32,
                );
            }
        }
        InboundPacket::MouseMove {
            delta_x, delta_y, ..
        } => {
            injector.move_mouse_relative(delta_x as i32, delta_y as i32);
        }
        InboundPacket::MouseButton { action, button } => {
            let is_press = match action {
                MouseButtonAction::Press => true,
                MouseButtonAction::Release => false,
            };
            let x11_btn = match button {
                MouseButton::Left => 1,
                MouseButton::Middle => 2,
                MouseButton::Right => 3,
                MouseButton::X1 => 8,
                MouseButton::X2 => 9,
            };
            injector.mouse_button(x11_btn, is_press);
        }
        InboundPacket::Scroll { delta_y, delta_x } => {
            if delta_y > 0 {
                for _ in 0..delta_y {
                    injector.mouse_button(4, true);
                    injector.mouse_button(4, false);
                }
            } else if delta_y < 0 {
                for _ in 0..-delta_y {
                    injector.mouse_button(5, true);
                    injector.mouse_button(5, false);
                }
            }
            if delta_x > 0 {
                for _ in 0..delta_x {
                    injector.mouse_button(7, true);
                    injector.mouse_button(7, false);
                }
            } else if delta_x < 0 {
                for _ in 0..-delta_x {
                    injector.mouse_button(6, true);
                    injector.mouse_button(6, false);
                }
            }
        }
        InboundPacket::HighResScroll { delta_y, delta_x } => {
            let click_y = if delta_y > 0 {
                (delta_y + 119) / 120
            } else {
                -((-delta_y + 119) / 120)
            };
            let click_x = if delta_x > 0 {
                (delta_x + 119) / 120
            } else {
                -((-delta_x + 119) / 120)
            };

            if click_y > 0 {
                for _ in 0..click_y {
                    injector.mouse_button(4, true);
                    injector.mouse_button(4, false);
                }
            } else if click_y < 0 {
                for _ in 0..-click_y {
                    injector.mouse_button(5, true);
                    injector.mouse_button(5, false);
                }
            }
            if click_x > 0 {
                for _ in 0..click_x {
                    injector.mouse_button(7, true);
                    injector.mouse_button(7, false);
                }
            } else if click_x < 0 {
                for _ in 0..-click_x {
                    injector.mouse_button(6, true);
                    injector.mouse_button(6, false);
                }
            }
        }
        InboundPacket::Key {
            action,
            key,
            modifiers,
            ..
        } => {
            let is_press = match action {
                KeyAction::Down => true,
                KeyAction::Up => false,
            };

            // Intercept Ctrl + Alt + Shift + N (toggle cursor hide)
            // Ctrl (2) + Alt (4) + Shift (1) = 7
            if key == 78 && modifiers.bits() == 7 {
                if is_press {
                    if std::env::var("LUNARIS_HIDE_HOST_CURSOR").is_ok() {
                        info!("Intercepted Ctrl+Alt+Shift+N: Showing host cursor");
                        std::env::remove_var("LUNARIS_HIDE_HOST_CURSOR");
                    } else {
                        info!("Intercepted Ctrl+Alt+Shift+N: Hiding host cursor");
                        std::env::set_var("LUNARIS_HIDE_HOST_CURSOR", "1");
                    }
                }
            }

            injector.keyboard_key(key, is_press);
        }
        _ => {}
    }
}
