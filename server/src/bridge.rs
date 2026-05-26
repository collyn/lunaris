use std::io::Cursor;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use std::time::Duration;

use anyhow::Result;
use bytes::Bytes;
use tokio::sync::mpsc;
use tracing::{debug, error, info, trace, warn};

use webrtc::ice_transport::ice_candidate::RTCIceCandidate;

use webrtc::{
    api::{
        interceptor_registry::register_default_interceptors,
        media_engine::{MediaEngine, MIME_TYPE_AV1, MIME_TYPE_H264, MIME_TYPE_HEVC},
        setting_engine::SettingEngine,
        APIBuilder,
    },
    data_channel::{
        data_channel_init::RTCDataChannelInit,
        data_channel_message::DataChannelMessage,
        RTCDataChannel,
    },
    interceptor::registry::Registry,
    media::Sample,
    peer_connection::{RTCPeerConnection, peer_connection_state::RTCPeerConnectionState},
    rtp::{header::Header, packet::Packet, packetizer::Payloader},
    rtp_transceiver::{
        rtp_codec::{RTCRtpCodecCapability, RTCRtpCodecParameters, RTPCodecType},
        RTCPFeedback,
    },
    track::track_local::{
        track_local_static_rtp::TrackLocalStaticRTP,
        track_local_static_sample::TrackLocalStaticSample,
        TrackLocalWriter,
    },
};

use moonlight_common::{
    crypto::openssl::OpenSSLCryptoBackend,
    high::tokio::MoonlightHost,
    http::{client::tokio_hyper::TokioHyperClient, ClientIdentifier, ClientSecret, ServerIdentifier},
    stream::{
        audio::{AudioConfig, AudioDecoder, AudioSample, OpusMultistreamConfig},
        c::{bindings::{Stage, ConnectionStatus}, connection::ConnectionListenerC, MoonlightInstance, MoonlightStream},
        connection::ConnectionListener,
        control::ActiveGamepads,
        video::{
            ColorRange, ColorSpace, DecodeResult, SupportedVideoFormats, VideoCapabilities,
            VideoDecodeUnit, VideoDecoder, VideoSetup, VideoFormat,
        },
        AesIv, AesKey, EncryptionFlags, MoonlightStreamSettings, StreamingConfig,
    },
};

use crate::input::{InboundPacket, TransportChannel};
use crate::pairing::AgentConfig;
use crate::video::h264::payloader::H264Payloader;
use crate::video::h264::reader::H264Reader;
use crate::video::h265::payloader::H265Payloader;
use crate::video::h265::reader::H265Reader;
use crate::video::trim_bytes_to_range;
use webrtc::rtp::codecs::av1::Av1Payloader;

// We reuse the same channel IDs
use crate::input::TransportChannelId;

pub struct BridgeSession {
    pub peer_connection: Arc<RTCPeerConnection>,
    pub moonlight_stream: Arc<std::sync::RwLock<Option<MoonlightStream>>>,
}

pub enum VideoPayloader {
    H264(H264Payloader),
    H265(H265Payloader),
    Av1(Av1Payloader),
}

pub struct VideoFramePayload {
    pub full_frame: Vec<u8>,
    pub timestamp: u32,
}

pub struct StreamVideoDecoder {
    supported_formats: SupportedVideoFormats,
    need_idr: Arc<AtomicBool>,
    webrtc_connected: Arc<AtomicBool>,
    video_frame_tx: tokio::sync::mpsc::Sender<VideoFramePayload>,
    frame_count: u64,
    start_time: std::time::Instant,
    format: Option<VideoFormat>,
    last_idr_time: std::time::Instant,
}

impl VideoDecoder for StreamVideoDecoder {
    fn setup(&mut self, setup: VideoSetup) -> i32 {
        info!("Video setup called from Moonlight: {:?}", setup);
        self.format = Some(setup.format);
        0
    }

    fn start(&mut self) {}
    fn stop(&mut self) {}

    fn submit_decode_unit(&mut self, unit: VideoDecodeUnit<'_>) -> DecodeResult {
        if !self.webrtc_connected.load(Ordering::SeqCst) {
            return DecodeResult::Ok;
        }

        if self.need_idr.load(Ordering::SeqCst) {
            let now = std::time::Instant::now();
            if now.duration_since(self.last_idr_time) >= std::time::Duration::from_millis(1000) {
                self.need_idr.store(false, Ordering::SeqCst);
                self.last_idr_time = now;
                info!("Forcing keyframe request (DR_NEED_IDR) due to PLI/queue full");
                return DecodeResult::NeedIdr;
            }
        }

        let frame_num = self.frame_count;
        self.frame_count += 1;

        let elapsed = self.start_time.elapsed();
        let timestamp = (elapsed.as_nanos() * 90000 / 1_000_000_000) as u32;

        let mut full_frame = Vec::new();
        for buffer in unit.buffers {
            full_frame.extend_from_slice(buffer.data);
        }

        if frame_num % 120 == 0 {
            trace!(
                "submit_decode_unit (enqueued): frame {}, timestamp {}, buffers {}, format: {:?}",
                frame_num,
                timestamp,
                unit.buffers.len(),
                self.format
            );
        }

        match self.video_frame_tx.try_send(VideoFramePayload {
            full_frame,
            timestamp,
        }) {
            Ok(_) => DecodeResult::Ok,
            Err(e) => {
                warn!("Video frame queue full or closed, dropping frame: {:?}", e);
                self.need_idr.store(true, Ordering::SeqCst);
                
                let now = std::time::Instant::now();
                if now.duration_since(self.last_idr_time) >= std::time::Duration::from_millis(1000) {
                    self.need_idr.store(false, Ordering::SeqCst);
                    self.last_idr_time = now;
                    DecodeResult::NeedIdr
                } else {
                    DecodeResult::Ok
                }
            }
        }
    }

    fn supported_formats(&self) -> SupportedVideoFormats {
        self.supported_formats
    }

    fn capabilities(&self) -> VideoCapabilities {
        VideoCapabilities::default()
    }
}


pub struct StreamAudioDecoder {
    sample_rate: u32,
    samples_per_frame: u32,
    audio_tx: tokio::sync::mpsc::UnboundedSender<Sample>,
}

impl AudioDecoder for StreamAudioDecoder {
    fn setup(&mut self, _audio_config: AudioConfig, stream_config: OpusMultistreamConfig) -> i32 {
        info!("Audio setup called: {:?}", stream_config);
        self.sample_rate = stream_config.sample_rate;
        self.samples_per_frame = stream_config.samples_per_frame;
        0
    }

    fn start(&mut self) {}
    fn stop(&mut self) {}

    fn decode_and_play_sample(&mut self, sample: AudioSample) {
        let duration = Duration::from_secs_f64(self.samples_per_frame as f64 / self.sample_rate as f64);
        let sample_webrtc = Sample {
            data: Bytes::copy_from_slice(&sample.buffer),
            duration,
            ..Default::default()
        };
        let _ = self.audio_tx.send(sample_webrtc);
    }

    fn config(&self) -> AudioConfig {
        AudioConfig::STEREO
    }
}

pub struct StreamConnectionListener;

impl ConnectionListener for StreamConnectionListener {
    fn set_hdr_mode(&mut self, _hdr_enabled: bool) {}
    fn controller_rumble(&mut self, _controller_number: u16, _low_frequency_motor: u16, _high_frequency_motor: u16) {}
    fn controller_rumble_triggers(&mut self, _controller_number: u16, _left_trigger_motor: u16, _right_trigger_motor: u16) {}
    fn controller_set_motion_event_state(&mut self, _controller_number: u16, _motion_type: u8, _report_rate_hz: u16) {}
    fn controller_set_adaptive_triggers(&mut self, _controller_number: u16, _event_flags: u8, _type_left: u8, _type_right: u8, _left: &mut u8, _right: &mut u8) {}
    fn controller_set_led(&mut self, _controller_number: u16, _r: u8, _g: u8, _b: u8) {}
}

impl ConnectionListenerC for StreamConnectionListener {
    fn stage_starting(&mut self, stage: Stage) {
        info!("Stage starting: {:?}", stage.name());
    }

    fn stage_complete(&mut self, stage: Stage) {
        info!("Stage complete: {:?}", stage.name());
    }

    fn stage_failed(&mut self, stage: Stage, error: i32) {
        error!("Stage failed: {:?} with error: {}", stage.name(), error);
    }

    fn connection_started(&mut self) {
        info!("Connection started!");
    }

    fn connection_terminated(&mut self, error_code: i32) {
        info!("Connection terminated: {}", error_code);
    }

    fn log_message(&mut self, message: &str) {
        info!("Moonlight C log: {}", message);
    }

    fn connection_status_update(&mut self, status: ConnectionStatus) {
        info!("Connection status update: {:?}", status);
    }
}

pub async fn setup_bridge_session(
    agent_config: AgentConfig,
    _client_id: String,
    sunshine_ip: String,
    sunshine_port: u16,
    client_tx: mpsc::UnboundedSender<common::ServerToClientMessage>,
    host_id: String,
    width: Option<u32>,
    height: Option<u32>,
    fps: Option<u32>,
    bitrate: Option<u32>,
    codec: Option<String>,
    app_id: Option<u32>,
) -> Result<Arc<BridgeSession>> {
    // 1. Setup WebRTC PeerConnection
    let mut api_settings = SettingEngine::default();
    api_settings.set_include_loopback_candidate(true);

    let mut api_media = MediaEngine::default();
    
    // Register H264
    api_media.register_codec(
        RTCRtpCodecParameters {
            capability: RTCRtpCodecCapability {
                mime_type: MIME_TYPE_H264.to_string(),
                clock_rate: 90000,
                channels: 0,
                sdp_fmtp_line: "level-asymmetry-allowed=1;packetization-mode=1;profile-level-id=42e01f".to_string(),
                rtcp_feedback: vec![
                    RTCPFeedback { typ: "nack".to_string(), parameter: "".to_string() },
                    RTCPFeedback { typ: "nack".to_string(), parameter: "pli".to_string() },
                    RTCPFeedback { typ: "goog-remb".to_string(), parameter: "".to_string() },
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
                sdp_fmtp_line: "profile-id=1;tier-flag=0;level-id=93;tx-mode=SRST".to_string(),
                rtcp_feedback: vec![
                    RTCPFeedback { typ: "nack".to_string(), parameter: "".to_string() },
                    RTCPFeedback { typ: "nack".to_string(), parameter: "pli".to_string() },
                    RTCPFeedback { typ: "goog-remb".to_string(), parameter: "".to_string() },
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
                    RTCPFeedback { typ: "nack".to_string(), parameter: "".to_string() },
                    RTCPFeedback { typ: "nack".to_string(), parameter: "pli".to_string() },
                    RTCPFeedback { typ: "goog-remb".to_string(), parameter: "".to_string() },
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

    let mut registry = Registry::new();
    registry = register_default_interceptors(registry, &mut api_media)?;

    let api = APIBuilder::new()
        .with_setting_engine(api_settings)
        .with_media_engine(api_media)
        .with_interceptor_registry(registry)
        .build();

    let rtc_config = webrtc::peer_connection::configuration::RTCConfiguration {
        ice_servers: vec![
            webrtc::ice_transport::ice_server::RTCIceServer {
                urls: vec!["stun:stun.l.google.com:19302".to_string()],
                ..Default::default()
            },
            webrtc::ice_transport::ice_server::RTCIceServer {
                urls: vec!["stun:stun1.l.google.com:19302".to_string()],
                ..Default::default()
            },
        ],
        ..Default::default()
    };
    let peer_connection = Arc::new(api.new_peer_connection(rtc_config).await?);

    let webrtc_connected = Arc::new(AtomicBool::new(false));
    let webrtc_connected_clone = webrtc_connected.clone();
    peer_connection.on_peer_connection_state_change(Box::new(move |state: RTCPeerConnectionState| {
        let webrtc_connected = webrtc_connected_clone.clone();
        Box::pin(async move {
            info!("WebRTC connection state changed to: {}", state);
            if state == RTCPeerConnectionState::Connected {
                webrtc_connected.store(true, Ordering::SeqCst);
            } else if state == RTCPeerConnectionState::Closed || state == RTCPeerConnectionState::Failed || state == RTCPeerConnectionState::Disconnected {
                webrtc_connected.store(false, Ordering::SeqCst);
            }
        })
    }));

    // Resolve codec settings with host capability checks
    let host_support = agent_config.server_codec_mode_support;
    let _h264_supported = host_support == 0 || (host_support & 262145) != 0;
    let hevc_supported = host_support == 0 || (host_support & 1573632) != 0;
    let av1_supported = host_support != 0 && (host_support & 6488064) != 0;

    let mut codec_str = codec.as_deref().unwrap_or("h264").to_lowercase();
    if codec_str == "av1" && !av1_supported {
        info!("AV1 requested but host doesn't support it, falling back to H.264");
        codec_str = "h264".to_string();
    }
    if codec_str == "h265" && !hevc_supported {
        info!("H.265 requested but host doesn't support it, falling back to H.264");
        codec_str = "h264".to_string();
    }

    let (mime_type, sdp_fmtp_line, payload_type, supported_formats, payloader) = match codec_str.as_str() {
        "h265" => (
            MIME_TYPE_HEVC.to_string(),
            "profile-id=1;tier-flag=0;level-id=93;tx-mode=SRST".to_string(),
            98,
            SupportedVideoFormats::H265,
            VideoPayloader::H265(H265Payloader::default()),
        ),
        "av1" => (
            MIME_TYPE_AV1.to_string(),
            "profile=0".to_string(),
            102,
            SupportedVideoFormats::AV1_MAIN8 | SupportedVideoFormats::AV1_MAIN10,
            VideoPayloader::Av1(Av1Payloader::default()),
        ),
        _ => (
            MIME_TYPE_H264.to_string(),
            "level-asymmetry-allowed=1;packetization-mode=1;profile-level-id=42e01f".to_string(),
            96,
            SupportedVideoFormats::H264,
            VideoPayloader::H264(H264Payloader::default()),
        ),
    };

    info!("Configured WebRTC video track codec: {} (Mime: {}, PT: {})", codec_str, mime_type, payload_type);

    // Create video and audio tracks
    let video_track = Arc::new(TrackLocalStaticRTP::new(
        RTCRtpCodecCapability {
            mime_type,
            clock_rate: 90000,
            sdp_fmtp_line,
            ..Default::default()
        },
        "video".to_string(),
        "moonlight".to_string(),
    ));

    let audio_track = Arc::new(TrackLocalStaticSample::new(
        RTCRtpCodecCapability {
            mime_type: "audio/opus".to_string(),
            clock_rate: 48000,
            channels: 2,
            ..Default::default()
        },
        "audio".to_string(),
        "moonlight".to_string(),
    ));

    let video_sender = peer_connection.add_track(video_track.clone() as Arc<dyn webrtc::track::track_local::TrackLocal + Send + Sync>).await?;
    peer_connection.add_track(audio_track.clone() as Arc<dyn webrtc::track::track_local::TrackLocal + Send + Sync>).await?;

    let need_idr_flag = Arc::new(AtomicBool::new(true));
    let need_idr_clone = need_idr_flag.clone();
    let video_sender_clone = video_sender.clone();
    tokio::spawn(async move {
        let mut buf = vec![0u8; 1500];
        while let Ok((packets, _)) = video_sender_clone.read(&mut buf).await {
            for packet in packets {
                if packet.as_any().is::<webrtc::rtcp::payload_feedbacks::picture_loss_indication::PictureLossIndication>() {
                    info!("Received PLI request from browser, requesting IDR frame");
                    need_idr_clone.store(true, Ordering::SeqCst);
                }
            }
        }
    });

    let moonlight_stream_rwlock = Arc::new(std::sync::RwLock::new(None));
    let ml_stream_clone = moonlight_stream_rwlock.clone();

    // Create Data Channels
    let general_channel = peer_connection.create_data_channel("general", None).await?;
    let mouse_reliable_channel = peer_connection.create_data_channel("mouse_reliable", None).await?;

    let mouse_absolute_init = RTCDataChannelInit {
        ordered: Some(false),
        max_retransmits: Some(0),
        ..Default::default()
    };
    let mouse_absolute_channel = peer_connection.create_data_channel("mouse_absolute", Some(mouse_absolute_init)).await?;

    let mouse_relative_init = RTCDataChannelInit {
        ordered: Some(false),
        max_retransmits: Some(0),
        ..Default::default()
    };
    let mouse_relative_channel = peer_connection.create_data_channel("mouse_relative", Some(mouse_relative_init)).await?;
    let keyboard_channel = peer_connection.create_data_channel("keyboard", None).await?;

    // Handle messages on Data Channels
    setup_data_channel_handler(general_channel, TransportChannel(TransportChannelId::GENERAL), ml_stream_clone.clone());
    setup_data_channel_handler(mouse_reliable_channel, TransportChannel(TransportChannelId::MOUSE_RELIABLE), ml_stream_clone.clone());
    setup_data_channel_handler(mouse_absolute_channel, TransportChannel(TransportChannelId::MOUSE_ABSOLUTE), ml_stream_clone.clone());
    setup_data_channel_handler(mouse_relative_channel, TransportChannel(TransportChannelId::MOUSE_RELATIVE), ml_stream_clone.clone());
    setup_data_channel_handler(keyboard_channel, TransportChannel(TransportChannelId::KEYBOARD), ml_stream_clone.clone());

    // ICE Candidate callback
    let client_tx_clone = client_tx.clone();
    let host_id_clone = host_id.clone();
    peer_connection.on_ice_candidate(Box::new(move |candidate: Option<RTCIceCandidate>| {
         let client_tx = client_tx_clone.clone();
         let host_id = host_id_clone.clone();
         Box::pin(async move {
             if let Some(cand) = candidate {
                 if let Ok(json_cand) = cand.to_json() {
                     let msg = common::ServerToClientMessage::Signaling(common::SignalingMessage::IceCandidate {
                         target_id: host_id,
                         candidate: common::RtcIceCandidate {
                             candidate: json_cand.candidate,
                             sdp_mid: json_cand.sdp_mid,
                             sdp_mline_index: json_cand.sdp_mline_index,
                             username_fragment: json_cand.username_fragment,
                         },
                     });
                     let _ = client_tx.send(msg);
                 }
             }
         })
     }));

    // Start Moonlight connection to Sunshine
    info!("Starting Moonlight host connection to {}:{}", sunshine_ip, sunshine_port);
    let host = MoonlightHost::<TokioHyperClient>::new(sunshine_ip, sunshine_port, Some(agent_config.client_unique_id))?;
    
    let client_cert_pem = pem::parse(&agent_config.client_certificate)?;
    let client_key_pem = pem::parse(&agent_config.client_private_key)?;
    let server_cert_pem = pem::parse(&agent_config.server_certificate)?;

    host.set_identity(
        ClientIdentifier::from_pem(client_cert_pem),
        ClientSecret::from_pem(client_key_pem),
        ServerIdentifier::from_pem(server_cert_pem),
    )
    .await?;

    let server_version = host.version().await?;
    let server_gfe_version = host.gfe_version().await?;
    let server_codec_mode_support = host.server_codec_mode_support().await?;

    let resolution_width = width.unwrap_or(1920); // Default to 1080p if not specified
    let resolution_height = height.unwrap_or(1080);
    let stream_fps = fps.unwrap_or(60);
    let stream_bitrate = bitrate.unwrap_or(8000); // Default 8Mbps

    let mut settings = MoonlightStreamSettings {
        width: resolution_width,
        height: resolution_height,
        fps: stream_fps,
        fps_x100: stream_fps * 100,
        bitrate: stream_bitrate,
        packet_size: 1392,
        encryption_flags: EncryptionFlags::ALL,
        streaming_remotely: StreamingConfig::Auto,
        sops: true,
        hdr: false,
        supported_video_formats: supported_formats,
        color_space: ColorSpace::Rec709,
        color_range: ColorRange::Limited,
        local_audio_play_mode: false,
        audio_config: AudioConfig::STEREO,
        gamepads_attached: ActiveGamepads::empty(),
        gamepads_persist_after_disconnect: false,
    };

    settings.adjust_for_server(
        server_version,
        &server_gfe_version,
        server_codec_mode_support,
    )?;

    let aes_key = AesKey::new_random(&OpenSSLCryptoBackend)?;
    let aes_iv = AesIv::new_random(&OpenSSLCryptoBackend)?;

    let mut resolved_app_id = app_id.unwrap_or(0);
    if resolved_app_id == 0 {
        match host.app_list().await {
            Ok(apps) => {
                info!("Retrieved app list from host: {:?}", apps);
                if let Some(desktop_app) = apps.iter().find(|app| app.title.to_lowercase().contains("desktop")) {
                    info!("Found desktop app: {} (ID: {})", desktop_app.title, desktop_app.id);
                    resolved_app_id = desktop_app.id;
                } else if let Some(first_app) = apps.first() {
                    info!("No desktop app found. Falling back to the first available app: {} (ID: {})", first_app.title, first_app.id);
                    resolved_app_id = first_app.id;
                } else {
                    warn!("App list is empty. Falling back to App ID 1");
                    resolved_app_id = 1;
                }
            }
            Err(e) => {
                warn!("Failed to retrieve app list: {:?}. Falling back to App ID 1", e);
                resolved_app_id = 1;
            }
        }
    }

    let stream_config = host.start_stream(
        resolved_app_id,
        &settings,
        aes_key,
        aes_iv,
        "",
    )
    .await?;

    let (video_frame_tx, mut video_frame_rx) = tokio::sync::mpsc::channel::<VideoFramePayload>(24);
    let video_track_clone = video_track.clone();
    let payload_type_clone = payload_type;
    let need_idr_clone_worker = need_idr_flag.clone();

    tokio::spawn(async move {
        let mut frame_count: u64 = 0;
        let mut payloader = payloader;

        // Bounded channel for complete frames (each represented as Vec<Packet>).
        // Capacity of 30 frames handles keyframe bursts cleanly without dropping frames.
        let (packet_tx, mut packet_rx) = tokio::sync::mpsc::channel::<Vec<Packet>>(30);

        // Spawn a dedicated writer task to write the RTP packets.
        // This task assigns RTP sequence numbers to ensure no gaps ever occur on drops.
        tokio::spawn(async move {
            let mut seq: u16 = 0;
            while let Some(mut packets) = packet_rx.recv().await {
                for packet in &mut packets {
                    packet.header.sequence_number = seq;
                    seq = seq.wrapping_add(1);
                    if let Err(e) = video_track_clone.write_rtp(packet).await {
                        debug!("Failed to write video RTP packet: {:?}", e);
                    }
                }
            }
        });

        while let Some(VideoFramePayload { full_frame, timestamp }) = video_frame_rx.recv().await {
            let mut samples = Vec::new();
            match &mut payloader {
                VideoPayloader::H264(_) => {
                    let mut reader = H264Reader::new(Cursor::new(full_frame), 0);
                    while let Ok(Some(nal)) = reader.next_nal() {
                        if nal.header.nal_unit_type == crate::video::h264::NalUnitType::FillerData {
                            continue;
                        }
                        let data = trim_bytes_to_range(nal.full, nal.header_range.start..nal.payload_range.end);
                        samples.push(data);
                    }
                }
                VideoPayloader::H265(_) => {
                    let mut reader = H265Reader::new(Cursor::new(full_frame), 0);
                    while let Ok(Some(nal)) = reader.next_nal() {
                        if nal.header.nal_unit_type == crate::video::h265::reader::NalUnitType::FdNut {
                            continue;
                        }
                        let data = trim_bytes_to_range(nal.full, nal.header_range.start..nal.payload_range.end);
                        samples.push(data);
                    }
                }
                VideoPayloader::Av1(_) => {
                    let mut reader = crate::video::annexb::AnnexBSplitter::new(Cursor::new(full_frame), 0);
                    while let Ok(Some(obu)) = reader.next() {
                        let data = trim_bytes_to_range(obu.full, obu.payload_range.start..obu.payload_range.end);
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
                            payload_type: payload_type_clone,
                            ..Default::default()
                        },
                        payload,
                    };
                    packets.push(packet);
                }
            }

            let total_packets = packets.len();
            if let Err(e) = packet_tx.try_send(packets) {
                warn!("RTP frame queue full, dropping frame: {:?}", e);
                need_idr_clone_worker.store(true, Ordering::SeqCst);
            }

            if frame_count % 120 == 0 {
                trace!("Background video processor: frame {}, sent {} RTP packets", frame_count, total_packets);
            }
            frame_count += 1;
        }
    });

    let (audio_tx, mut audio_rx) = tokio::sync::mpsc::unbounded_channel::<Sample>();
    let audio_track_clone = audio_track.clone();
    tokio::spawn(async move {
        while let Some(sample) = audio_rx.recv().await {
            if let Err(e) = audio_track_clone.write_sample(&sample).await {
                debug!("Failed to write audio sample: {:?}", e);
            }
        }
    });

    let video_decoder = StreamVideoDecoder {
        supported_formats,
        need_idr: need_idr_flag.clone(),
        webrtc_connected: webrtc_connected.clone(),
        video_frame_tx,
        frame_count: 0,
        start_time: std::time::Instant::now(),
        format: None,
        last_idr_time: std::time::Instant::now() - std::time::Duration::from_secs(5),
    };

    let audio_decoder = StreamAudioDecoder {
        sample_rate: 48000,
        samples_per_frame: 480,
        audio_tx,
    };

    let connection_listener = StreamConnectionListener;
    let connection_listener_c = StreamConnectionListener;

    let moonlight = MoonlightInstance::global().expect("failed to find moonlight");
    
    // Start C connection in background thread since it is blocking
    let ml_stream_rwlock_clone = moonlight_stream_rwlock.clone();
    std::thread::spawn(move || {
        match moonlight.start_connection(
            stream_config,
            settings,
            connection_listener,
            connection_listener_c,
            video_decoder,
            audio_decoder,
        ) {
            Ok(stream) => {
                info!("Moonlight C connection successfully started!");
                let mut lock = ml_stream_rwlock_clone.write().unwrap();
                *lock = Some(stream);
            }
            Err(e) => {
                error!("Failed to start Moonlight connection: {:?}", e);
            }
        }
    });

    Ok(Arc::new(BridgeSession {
        peer_connection,
        moonlight_stream: moonlight_stream_rwlock,
    }))
}

fn setup_data_channel_handler(
    data_channel: Arc<RTCDataChannel>,
    channel: TransportChannel,
    ml_stream: Arc<std::sync::RwLock<Option<MoonlightStream>>>,
) {
    data_channel.on_message(Box::new(move |msg: DataChannelMessage| {
        let ml_stream = ml_stream.clone();
        let channel = channel;
        Box::pin(async move {
            let Some(packet) = InboundPacket::deserialize(channel, &msg.data) else {
                return;
            };

            let stream_guard = ml_stream.read().unwrap();
            let Some(stream) = stream_guard.as_ref() else {
                return;
            };

            match packet {
                InboundPacket::GeneralStop => {
                    info!("Received stop message on general channel");
                }
                InboundPacket::MouseMove { delta_x, delta_y } => {
                    let _ = stream.send_mouse_move(delta_x, delta_y);
                }
                InboundPacket::MousePosition { x, y, reference_width, reference_height } => {
                    let _ = stream.send_mouse_position(x, y, reference_width, reference_height);
                }
                InboundPacket::MouseButton { action, button } => {
                    let _ = stream.send_mouse_button(action, button);
                }
                InboundPacket::Scroll { delta_x, delta_y } => {
                    if delta_y != 0 {
                        let _ = stream.send_scroll(delta_y);
                    }
                    if delta_x != 0 {
                        let _ = stream.send_horizontal_scroll(delta_x);
                    }
                }
                InboundPacket::HighResScroll { delta_x, delta_y } => {
                    if delta_y != 0 {
                        let _ = stream.send_high_res_scroll(delta_y);
                    }
                    if delta_x != 0 {
                        let _ = stream.send_high_res_horizontal_scroll(delta_x);
                    }
                }
                InboundPacket::Key { action, modifiers, key, flags } => {
                    let _ = stream.send_keyboard_event_non_standard(key as i16, action, modifiers, flags);
                }
                InboundPacket::Text { text } => {
                    let _ = stream.send_text(&text);
                }
                _ => {}
            }
        })
    }));
}
