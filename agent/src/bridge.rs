use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use std::time::{Duration, Instant};

use anyhow::Result;
use base64::Engine;
use tokio::sync::mpsc;
use tracing::{debug, error, info, trace, warn};

use rtc::interceptor::{FlexFec03SendBuilder, Slot};
use rtc::media::Sample;
use rtc::media_stream::MediaStreamTrack;
use rtc::peer_connection::configuration::media_engine::{
    MIME_TYPE_AV1, MIME_TYPE_FLEX_FEC03, MIME_TYPE_H264, MIME_TYPE_HEVC, MIME_TYPE_OPUS,
    MIME_TYPE_VP8, MIME_TYPE_VP9,
};
use rtc::rtp::codec::vp8::Vp8Payloader;
use rtc::rtp::{
    extension::{
        abs_send_time_extension::AbsSendTimeExtension,
        playout_delay_extension::PlayoutDelayExtension, HeaderExtension,
    },
    header::Header,
    packet::Packet,
    packetizer::Payloader,
};
use rtc::rtp_transceiver::rtp_sender::{
    RTCPFeedback, RTCRtpCodec, RTCRtpCodecParameters, RTCRtpCodingParameters,
    RTCRtpEncodingParameters, RTCRtpFecParameters, RTCRtpHeaderExtensionCapability, RtpCodecKind,
};
use rtc::sdp::extmap::ABS_SEND_TIME_URI;
use webrtc::data_channel::{
    DataChannel, DataChannelEvent, RTCDataChannelInit, RTCDataChannelState,
};
use webrtc::media_stream::track_local::{
    static_rtp::TrackLocalStaticRTP, static_sample::TrackLocalStaticSample, TrackLocal,
    TrackLocalEvent,
};
use webrtc::peer_connection::{
    register_default_interceptors, MediaEngine, PeerConnection, PeerConnectionBuilder,
    PeerConnectionEventHandler, RTCConfigurationBuilder, RTCIceConnectionState,
    RTCIceGatheringState, RTCIceServer, RTCPeerConnectionIceEvent, RTCPeerConnectionState,
    Registry, SettingEngineBuilder,
};

use crate::input::{InboundPacket, KeyAction, MouseButton, MouseButtonAction, TransportChannel};
use crate::pairing::AgentConfig;
use crate::video::av1::LunarisAv1Payloader;
use crate::video::h264::payloader::H264Payloader;
use crate::video::h264::reader::H264Reader;
use crate::video::h265::payloader::H265Payloader;
use crate::video::h265::reader::H265Reader;
use crate::video::vp9::LunarisVp9Payloader;
use crate::video::{frame_buffer, trim_bytes_to_range};

// We reuse the same channel IDs
use crate::input::TransportChannelId;

/// Payload type of FlexFEC-03, as the reference project registers it; it does
/// not collide with the 96/98/102/111 this host registers.
const FLEX_FEC_PAYLOAD_TYPE: u8 = 49;

fn random_ssrc() -> u32 {
    (uuid::Uuid::new_v4().as_u128() & 0xFFFF_FFFF) as u32
}

type PipelineCmdSlot =
    Arc<std::sync::Mutex<Option<tokio::sync::mpsc::Sender<lunaris_media::pipeline::PipelineCommand>>>>;

/// Asks the encoder for a key frame after a frame was dropped before it was
/// packetised, at most once per `KEYFRAME_AFTER_DROP_INTERVAL` so a backlog
/// does not ask for one per frame.
struct KeyframeRequester {
    pipeline_cmd: PipelineCmdSlot,
    last_request: Option<Instant>,
}

const KEYFRAME_AFTER_DROP_INTERVAL: Duration = Duration::from_millis(500);

impl KeyframeRequester {
    fn new(pipeline_cmd: PipelineCmdSlot) -> Self {
        Self { pipeline_cmd, last_request: None }
    }

    /// Whether a request is due now (and records it).
    fn due(&mut self, now: Instant) -> bool {
        if self
            .last_request
            .is_some_and(|last| now.duration_since(last) < KEYFRAME_AFTER_DROP_INTERVAL)
        {
            return false;
        }
        self.last_request = Some(now);
        true
    }

    fn request(&mut self) {
        if !self.due(Instant::now()) {
            return;
        }
        if let Some(ref cmd_tx) = *self.pipeline_cmd.lock().unwrap() {
            let _ = cmd_tx.try_send(lunaris_media::pipeline::PipelineCommand::RequestKeyframe);
        }
    }
}

/// The payload type of FlexFEC-03 in an SDP answer, when the answer accepted
/// it: an `a=rtpmap:<pt> flexfec-03/90000` line of the video section whose
/// payload type is also listed on a non-rejected `m=video` line.
pub(crate) fn flexfec_in_answer(sdp: &str) -> Option<u8> {
    let mut in_video = false;
    let mut formats: Vec<u8> = Vec::new();
    let mut fec_pt: Option<u8> = None;
    for line in sdp.lines().map(str::trim) {
        if let Some(rest) = line.strip_prefix("m=") {
            in_video = rest.starts_with("video ");
            if in_video {
                // m=video <port> <proto> <pt>...; port 0 = rejected section.
                let mut parts = rest.split_whitespace().skip(1);
                if parts.next() == Some("0") {
                    in_video = false;
                    continue;
                }
                formats = parts.skip(1).filter_map(|pt| pt.parse().ok()).collect();
            }
        } else if in_video {
            if let Some(map) = line.strip_prefix("a=rtpmap:") {
                let mut it = map.splitn(2, ' ');
                let (pt, codec) = (it.next(), it.next());
                if let (Some(pt), Some(codec)) = (pt.and_then(|p| p.parse::<u8>().ok()), codec) {
                    if codec.to_ascii_lowercase().starts_with("flexfec-03/") {
                        fec_pt = Some(pt);
                    }
                }
            }
        }
    }
    fec_pt.filter(|pt| formats.contains(pt))
}

/// Events of a session's connection (webrtc 0.21 delivers them to a handler
/// object, 0.17 to callbacks).
struct SessionEventHandler {
    ws_tx: mpsc::UnboundedSender<common::AgentMessage>,
    client_id: String,
    webrtc_connected: Arc<AtomicBool>,
    pipeline_cmd_tx:
        Arc<std::sync::Mutex<Option<tokio::sync::mpsc::Sender<lunaris_media::pipeline::PipelineCommand>>>>,
}

#[async_trait::async_trait]
impl PeerConnectionEventHandler for SessionEventHandler {
    async fn on_ice_candidate(&self, event: RTCPeerConnectionIceEvent) {
        if let Ok(json_cand) = event.candidate.to_json() {
            let msg = common::AgentMessage::Signaling(common::SignalingMessage::IceCandidate {
                target_id: self.client_id.clone(),
                candidate: common::RtcIceCandidate {
                    candidate: json_cand.candidate,
                    sdp_mid: json_cand.sdp_mid,
                    sdp_mline_index: json_cand.sdp_mline_index,
                    username_fragment: json_cand.username_fragment,
                },
            });
            let _ = self.ws_tx.send(msg);
        }
    }

    // ICE Gathering state change — helps diagnose ICE failures in production logs
    async fn on_ice_gathering_state_change(&self, state: RTCIceGatheringState) {
        warn!("[ICE] Gathering state changed: {:?}", state);
    }

    // ICE Connection state change — logs actual ICE connectivity progress
    async fn on_ice_connection_state_change(&self, state: RTCIceConnectionState) {
        warn!("[ICE] Connection state changed: {:?}", state);
    }

    // Peer Connection state changes clean up the session
    async fn on_connection_state_change(&self, state: RTCPeerConnectionState) {
        info!("WebRTC connection state changed to: {}", state);
        if state == RTCPeerConnectionState::Connected {
            self.webrtc_connected.store(true, Ordering::SeqCst);
            if let Some(ref cmd_tx) = *self.pipeline_cmd_tx.lock().unwrap() {
                info!("WebRTC connected! Requesting immediate IDR keyframe from media pipeline.");
                let _ = cmd_tx.try_send(lunaris_media::pipeline::PipelineCommand::RequestKeyframe);
            }
        } else if state == RTCPeerConnectionState::Closed
            || state == RTCPeerConnectionState::Failed
            || state == RTCPeerConnectionState::Disconnected
        {
            self.webrtc_connected.store(false, Ordering::SeqCst);

            // Stop MediaPipeline if it is active
            if let Some(ref cmd_tx) = *self.pipeline_cmd_tx.lock().unwrap() {
                info!("WebRTC connection state is inactive ({}). Stopping MediaPipeline...", state);
                let _ = cmd_tx.try_send(lunaris_media::pipeline::PipelineCommand::Stop);
            }
        }
    }
}

pub struct BridgeSession {
    pub peer_connection: Arc<dyn PeerConnection>,
    pub webtransport_port: Option<u16>,
    pub webtransport_cert_hash: Option<String>,
    /// Secret the client puts in the WebTransport URL path (`/<token>`).
    pub webtransport_token: Option<String>,
    pub _webtransport_endpoint:
        Option<Arc<wtransport::Endpoint<wtransport::endpoint::endpoint_side::Server>>>,
    pub webtransport_shutdown: Option<tokio::sync::oneshot::Sender<()>>,
    pub pipeline_cmd: Option<tokio::sync::mpsc::Sender<lunaris_media::pipeline::PipelineCommand>>,
    pub spawned_tasks: Vec<tokio::task::JoinHandle<()>>,
    /// This viewer's place in the display's shared pipeline (none for a
    /// connector's source); leaving it stops the pipeline when it was the
    /// last viewer.
    _media: Option<crate::media_hub::Membership>,
    _live: LiveSession,
}

/// Sessions that exist now (several viewers at once).
static LIVE_SESSIONS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// Counts a session in `LIVE_SESSIONS` for as long as it lives.
struct LiveSession;

impl LiveSession {
    fn new() -> Self {
        LIVE_SESSIONS.fetch_add(1, Ordering::SeqCst);
        LiveSession
    }
}

impl Drop for LiveSession {
    fn drop(&mut self) {
        LIVE_SESSIONS.fetch_sub(1, Ordering::SeqCst);
    }
}

/// The codec of lunaris-media named as the sessions name it.
fn codec_name_of(codec: lunaris_media::types::VideoCodec) -> &'static str {
    match codec {
        lunaris_media::types::VideoCodec::H265 => "h265",
        lunaris_media::types::VideoCodec::AV1 => "av1",
        _ => "h264",
    }
}

/// The codec of lunaris-media a session's codec name stands for.
fn media_codec_of(codec: &str) -> lunaris_media::types::VideoCodec {
    match codec {
        "h265" => lunaris_media::types::VideoCodec::H265,
        "av1" => lunaris_media::types::VideoCodec::AV1,
        _ => lunaris_media::types::VideoCodec::H264,
    }
}

/// Whether a viewer can take a running stream encoded as `running`: it
/// decodes its codec (the one it would get anyway, or one it says it
/// decodes), and 4:4:4 only when it asked for 4:4:4 itself.
pub(crate) fn viewer_decodes(
    running: &lunaris_media::types::StreamConfig,
    own_codec: &str,
    yuv444: bool,
    decode_codecs: Option<&[String]>,
) -> bool {
    let name = codec_name_of(running.codec);
    let decodes = name == own_codec || decode_codecs.is_some_and(|d| d.iter().any(|c| c.eq_ignore_ascii_case(name)));
    decodes && (!running.yuv444 || yuv444)
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
    Av1(LunarisAv1Payloader),
    /// Only for a picture a connector brings (the host itself does not encode VP8 or VP9).
    Vp8(Vp8Payloader),
    Vp9(LunarisVp9Payloader),
}

fn encoder_supports_codec(
    encoder: &lunaris_media::EncoderInfo,
    codec: lunaris_media::VideoCodec,
) -> bool {
    encoder.supported_codecs.contains(&codec)
}

/// Codecs this host offers for realtime streaming. H.264 may be software
/// (existing Lunaris policy: it is the fallback everywhere). H.265 and AV1
/// are offered only when a HARDWARE encoder for them really opens
/// (`hardware_opens`, from [`hardware_encoder_opens_blocking`]); software
/// HEVC/AV1 are too slow for interactive use. `hardware_opens` is ignored for
/// H.264.
pub(crate) fn codec_supported_for_realtime(
    encoders: &[lunaris_media::EncoderInfo],
    codec: lunaris_media::VideoCodec,
    hardware_opens: bool,
) -> bool {
    match codec {
        lunaris_media::VideoCodec::H264 => {
            encoders.iter().any(|e| encoder_supports_codec(e, codec))
        }
        lunaris_media::VideoCodec::H265 | lunaris_media::VideoCodec::AV1 => hardware_opens,
    }
}

/// Whether a hardware encoder for `codec` actually opens on this machine at
/// this size, on every platform: FFmpeg listing e.g. `hevc_vaapi` does not
/// mean the driver accepts it, and offering such a codec ends in a black
/// stream. The probe opens an encoder (tens to hundreds of ms), so the result
/// is cached per codec and size; call this from a blocking context.
pub(crate) fn hardware_encoder_opens_blocking(
    codec: lunaris_media::VideoCodec,
    width: u32,
    height: u32,
    fps: u32,
) -> bool {
    use std::collections::HashMap;
    use std::sync::{Mutex, OnceLock};
    static CACHE: OnceLock<Mutex<HashMap<String, bool>>> = OnceLock::new();
    let cache = CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    let key = format!("{:?}-{}x{}", codec, width, height);
    if let Some(ok) = cache.lock().unwrap().get(&key) {
        return *ok;
    }
    let probed = lunaris_media::encode::probe_hardware_encoder(codec, width, height, fps);
    let ok = probed
        .as_ref()
        .is_some_and(|e| e.hw_type != lunaris_media::HwAccelType::Software);
    info!(
        "Encoder probe {:?} {}x{}: {}",
        codec,
        width,
        height,
        probed.map(|e| format!("{} ({:?})", e.name, e.hw_type)).unwrap_or_else(|| "none".into())
    );
    cache.lock().unwrap().insert(key, ok);
    ok
}

async fn hardware_encoder_opens(
    codec: lunaris_media::VideoCodec,
    width: u32,
    height: u32,
    fps: u32,
) -> bool {
    tokio::task::spawn_blocking(move || hardware_encoder_opens_blocking(codec, width, height, fps))
        .await
        .unwrap_or(false)
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
    yuv444: bool,
    encoder_csc_mode: Option<i32>,
    microphone: bool,
    ice_servers: Option<Vec<common::RtcIceServer>>,
    // The computer to show instead of this one's screen (a source of the
    // agent's config, through its connector).
    source: Option<crate::connector::Target>,
    // The codecs the client says it decodes; None from older clients.
    decode_codecs: Option<Vec<String>>,
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

    let mut resolution_width = width.unwrap_or(1920);
    let mut resolution_height = height.unwrap_or(1080);
    let mut stream_fps = fps.unwrap_or(60);
    let mut stream_bitrate = bitrate.unwrap_or(8000);
    let mut yuv444 = yuv444;
    let requested_codec = codec.as_deref().unwrap_or("h264").to_lowercase();
    let mut codec_str = requested_codec.clone();

    // Sunshine video.cpp: colorspace_from_client_config(config, hdr_display).
    // dynamicRange 0 and no HDR display until Phase 5.
    let media_colorspace = lunaris_media::colorspace::from_client_config(
        encoder_csc_mode.unwrap_or(lunaris_media::types::DEFAULT_ENCODER_CSC_MODE),
        0,
        false,
    );

    info!(
        "Session media request: client={} res={}x{} fps={} bitrate={}kbps codec={} encoder={} display={} yuv444={} csc={} full_range={}",
        client_id,
        resolution_width,
        resolution_height,
        stream_fps,
        stream_bitrate,
        requested_codec,
        encoder.as_deref().unwrap_or("auto"),
        display_id.as_deref().unwrap_or("default"),
        yuv444,
        media_colorspace.colorspace.label(),
        media_colorspace.full_range
    );

    // Resolve codec settings before registering WebRTC codecs. The SDP offer
    // must only advertise the codec the host can actually encode; otherwise
    // the answer can select H.265 while the RTP track is configured for H.264.
    // A source: the picture and the sound come from another computer,
    // through a connector, as it encodes them. Nothing is encoded here
    // then, and no encoder is tried. (LUNARIS_CONNECTOR_TEST_TARGET names
    // one for tests; see connector/media.rs.) A source that cannot be shown
    // is an error of the session: this computer's own screen is never shown
    // in its place.
    let connector_target = match crate::connector::media::test_target().map_err(|e| crate::connector::SourceError(e.to_string()))? {
        Some(target) => Some(target),
        None => source,
    };
    let encoders = if connector_target.is_some() { Vec::new() } else { lunaris_media::encode::list_available_encoders() };
    // Listed encoders are only candidates: H.265 and AV1 are supported when
    // a hardware encoder for them really opens (cached per codec and size).
    let hevc_supported = codec_supported_for_realtime(
        &encoders,
        lunaris_media::VideoCodec::H265,
        connector_target.is_none()
            && hardware_encoder_opens(lunaris_media::VideoCodec::H265, resolution_width, resolution_height, stream_fps).await,
    );
    let av1_supported = codec_supported_for_realtime(
        &encoders,
        lunaris_media::VideoCodec::AV1,
        connector_target.is_none()
            && hardware_encoder_opens(lunaris_media::VideoCodec::AV1, resolution_width, resolution_height, stream_fps).await,
    );
    if codec_str == "av1" && !av1_supported {
        info!("AV1 requested but host doesn't support it, falling back to H.264");
        codec_str = "h264".to_string();
    }
    if codec_str == "h265" && !hevc_supported {
        info!("H.265 requested but host doesn't support it, falling back to H.264");
        codec_str = "h264".to_string();
    }
    // YUV 4:4:4 is offered as H.264 High 4:4:4 Predictive (browsers decode
    // profile-level-id f4…; HEVC/AV1 4:4:4 are not available here).
    if yuv444 && codec_str != "h264" {
        info!("YUV 4:4:4 requested with {} — switching to H.264 High 4:4:4", codec_str);
        codec_str = "h264".to_string();
    }
    // The other computer is logged in to first: the codec offered to the
    // viewer is the one it sends.
    let connector_source = match connector_target {
        Some(target) => {
            // What the viewer itself asked for (not this function's defaults).
            let source = crate::connector::media::open(
                &target,
                &requested_codec,
                decode_codecs.as_deref(),
                stream_fps,
                (width.unwrap_or(0), height.unwrap_or(0)),
                bitrate.unwrap_or(0),
            )
            .await?;
            codec_str = crate::connector::media::codec_name(source.codec).to_string();
            Some(source)
        }
        None => None,
    };
    let from_connector = connector_source.is_some();

    // This computer's screen: one capture and encoder per display, shared by
    // its viewers (media_hub.rs). A viewer that can decode what a display's
    // running stream encodes joins it, and that stream's size, frame rate
    // and codec are then this session's.
    let media = if from_connector {
        None
    } else {
        let wanted = lunaris_media::types::StreamConfig {
            width: resolution_width,
            height: resolution_height,
            fps: stream_fps,
            codec: media_codec_of(&codec_str),
            bitrate_kbps: stream_bitrate,
            pixel_format: lunaris_media::types::PixelFormat::NV12,
            preferred_encoder: encoder.clone(),
            yuv444,
            colorspace: media_colorspace,
            microphone,
        };
        let screen = display_id.clone().unwrap_or_else(|| "default".to_string());
        let joined = crate::media_hub::join(&screen, wanted, |running| {
            viewer_decodes(running, &codec_str, yuv444, decode_codecs.as_deref())
        });
        if joined.joined_running {
            resolution_width = joined.config.width;
            resolution_height = joined.config.height;
            stream_fps = joined.config.fps;
            stream_bitrate = joined.config.bitrate_kbps;
            yuv444 = joined.config.yuv444;
            codec_str = codec_name_of(joined.config.codec).to_string();
            info!(
                "Session joins the running stream of display {}: {}x{} {} fps {} kbps codec={} yuv444={}",
                screen, resolution_width, resolution_height, stream_fps, stream_bitrate, codec_str, yuv444
            );
        }
        Some(joined)
    };
    // Level 4.2 (0x2a) covers 1080p60; 0xf4 = High 4:4:4 Predictive.
    let h264_fmtp = if yuv444 {
        "level-asymmetry-allowed=1;packetization-mode=1;profile-level-id=f4002a"
    } else {
        "level-asymmetry-allowed=1;packetization-mode=1;profile-level-id=42002a"
    };

    // 1. Setup WebRTC PeerConnection
    let api_settings = SettingEngineBuilder::new()
        .with_include_loopback_candidate(true)
        .with_network_types(vec![
            rtc::ice::network_type::NetworkType::Udp4,
            rtc::ice::network_type::NetworkType::Tcp4,
        ])
        .build();

    let mut api_media = MediaEngine::default();

    let video_rtcp_feedback = vec![
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
    ];

    // The video codec of this session: what is offered, and what the track
    // sends (mime, fmtp, payload type, payloader).
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
            VideoPayloader::Av1(LunarisAv1Payloader::new()),
        ),
        "vp9" => (
            MIME_TYPE_VP9.to_string(),
            "profile-id=0".to_string(),
            100,
            VideoPayloader::Vp9(LunarisVp9Payloader::new()),
        ),
        "vp8" => (
            MIME_TYPE_VP8.to_string(),
            String::new(),
            104,
            VideoPayloader::Vp8(Vp8Payloader::default()),
        ),
        _ => (
            MIME_TYPE_H264.to_string(),
            h264_fmtp.to_string(),
            96,
            VideoPayloader::H264(H264Payloader::default()),
        ),
    };
    let video_codec = RTCRtpCodec {
        mime_type: mime_type.clone(),
        clock_rate: 90000,
        channels: 0,
        sdp_fmtp_line: sdp_fmtp_line.clone(),
        rtcp_feedback: video_rtcp_feedback.clone(),
    };
    api_media.register_codec(
        RTCRtpCodecParameters {
            rtp_codec: video_codec.clone(),
            payload_type,
        },
        RtpCodecKind::Video,
    )?;

    // FlexFEC-03 (20%: 1 repair packet per 5 media packets, as Sunshine/
    // Moonlight do), the same registration as the reference project
    // (moonlight-web-stream src/api/stream/webrtc/mod.rs). The host offers,
    // and Chrome accepts flexfec-03 only when it is offered to it. Payload
    // type 49 does not collide with 96/97/98/102/111 used here. NACK stays
    // (video_rtcp_feedback above).
    // Off unless LUNARIS_FEC=1. With FlexFEC, Chrome on Windows stopped
    // decoding for 3 s after some losses although it kept receiving complete
    // frames (framesReceived 60/s, framesDecoded 0, then about 190 frames at
    // once; no key frame request): Linux host -> Chrome on Windows over the
    // WireGuard tunnel, 2-5 such freezes per 160 s with FEC, none in 150 s
    // without (42 losses, all repaired by NACK). The desktop client never
    // used the repair packets. Why the recovered packets upset Chrome's
    // decoder is not known yet.
    if std::env::var("LUNARIS_FEC").map(|v| v == "1").unwrap_or(false) {
    api_media.register_codec(
        RTCRtpCodecParameters {
            rtp_codec: RTCRtpCodec {
                mime_type: MIME_TYPE_FLEX_FEC03.to_owned(),
                clock_rate: 90000,
                channels: 0,
                sdp_fmtp_line: "repair-window=10000000".to_owned(),
                rtcp_feedback: video_rtcp_feedback.clone(),
            },
            payload_type: FLEX_FEC_PAYLOAD_TYPE,
        },
        RtpCodecKind::Video,
    )?;
    }

    // Register Opus
    let opus_codec = RTCRtpCodec {
        mime_type: MIME_TYPE_OPUS.to_string(),
        clock_rate: 48000,
        channels: 2,
        sdp_fmtp_line: "minptime=10;useinbandfec=1".to_string(),
        rtcp_feedback: vec![],
    };
    api_media.register_codec(
        RTCRtpCodecParameters {
            rtp_codec: opus_codec.clone(),
            payload_type: 111,
        },
        RtpCodecKind::Audio,
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
        RtpCodecKind::Video,
        None,
    )?;
    api_media.register_header_extension(
        RTCRtpHeaderExtensionCapability {
            uri: ABS_SEND_TIME_URI.to_string(),
        },
        RtpCodecKind::Video,
        None,
    )?;

    // Shared with the connection's event handler (state changes), which has
    // to exist before the connection does.
    let pipeline_cmd_tx_shared = Arc::new(std::sync::Mutex::new(
        None::<tokio::sync::mpsc::Sender<lunaris_media::pipeline::PipelineCommand>>,
    ));
    let webrtc_connected = Arc::new(AtomicBool::new(false));

    // register_default_interceptors adds NACK (responder + generator),
    // sender/receiver reports and the TWCC receiver, as in 0.17. The FEC
    // encoder (slot 5_000) and the RTCP forwarder (application-most, see
    // rtcp_forward.rs) are the 0.21 additions.
    let registry = register_default_interceptors(
        Registry::new()
            .with(
                Slot::FecEncoder,
                FlexFec03SendBuilder::default()
                    // 1 to 5 ratio = 20% fec
                    .with_num_media_packets(5)
                    .with_num_fec_packets(1)
                    .build(),
            )
            .with(
                Slot::from(14_000),
                crate::rtcp_forward::RtcpForwarderInterceptor::default(),
            ),
        &mut api_media,
    )?;

    let webrtc_ice_servers = if let Some(servers) = ice_servers {
        servers
            .into_iter()
            .map(|s| RTCIceServer {
                urls: s.urls,
                username: s.username.unwrap_or_default(),
                credential: s.credential.unwrap_or_default(),
            })
            .collect()
    } else {
        vec![
            RTCIceServer {
                urls: vec!["stun:stun.l.google.com:19302".to_string()],
                ..Default::default()
            },
            RTCIceServer {
                urls: vec!["stun:stun1.l.google.com:19302".to_string()],
                ..Default::default()
            },
        ]
    };

    let rtc_config = RTCConfigurationBuilder::default()
        .with_ice_servers(webrtc_ice_servers)
        .build();
    let handler = Arc::new(SessionEventHandler {
        ws_tx: ws_tx.clone(),
        client_id: client_id.clone(),
        webrtc_connected: webrtc_connected.clone(),
        pipeline_cmd_tx: pipeline_cmd_tx_shared.clone(),
    });
    let peer_connection: Arc<dyn PeerConnection> = Arc::new(
        PeerConnectionBuilder::new()
            .with_configuration(rtc_config)
            .with_setting_engine(api_settings)
            .with_media_engine(api_media)
            .with_interceptor_registry(registry)
            .with_handler(handler)
            // The wildcard covers every interface but not loopback (allowed
            // by the setting above): list it, as 0.17 gathered it.
            .with_udp_addrs(vec!["0.0.0.0:0", "127.0.0.1:0"])
            .build()
            .await?,
    );

    info!(
        "Configured WebRTC video track codec: {} (requested={}, Mime: {}, PT: {}, host_h265={}, host_av1={})",
        codec_str, requested_codec, mime_type, payload_type, hevc_supported, av1_supported
    );

    // Create video and audio tracks. The packets are written with the SSRC
    // and payload type set here (0.17 rewrote them per binding).
    let video_ssrc: u32 = random_ssrc();
    let video_fec_ssrc: u32 = loop {
        let candidate: u32 = random_ssrc();
        if candidate != video_ssrc {
            break candidate;
        }
    };
    let audio_ssrc: u32 = loop {
        let candidate: u32 = random_ssrc();
        if candidate != video_ssrc && candidate != video_fec_ssrc {
            break candidate;
        }
    };
    let video_track = Arc::new(TrackLocalStaticRTP::new(MediaStreamTrack::new(
        "lunaris".to_string(),
        "video".to_string(),
        "video".to_string(),
        RtpCodecKind::Video,
        vec![RTCRtpEncodingParameters {
            rtp_coding_parameters: RTCRtpCodingParameters {
                ssrc: Some(video_ssrc),
                // The repair stream of the FEC encoder.
                fec: Some(RTCRtpFecParameters { ssrc: video_fec_ssrc }),
                ..Default::default()
            },
            codec: video_codec.clone(),
            ..Default::default()
        }],
    )));

    let audio_track = Arc::new(TrackLocalStaticSample::new(
        Instant::now(),
        MediaStreamTrack::new(
            "lunaris".to_string(),
            "audio".to_string(),
            "audio".to_string(),
            RtpCodecKind::Audio,
            vec![RTCRtpEncodingParameters {
                rtp_coding_parameters: RTCRtpCodingParameters {
                    ssrc: Some(audio_ssrc),
                    ..Default::default()
                },
                codec: opus_codec.clone(),
                ..Default::default()
            }],
        ),
    )?);

    peer_connection
        .add_track(video_track.clone() as Arc<dyn TrackLocal>)
        .await?;
    peer_connection
        .add_track(audio_track.clone() as Arc<dyn TrackLocal>)
        .await?;

    let mut spawned_tasks: Vec<tokio::task::JoinHandle<()>> = Vec::new();
    let pipeline_cmd_tx_for_pli = pipeline_cmd_tx_shared.clone();

    let video_track_for_rtcp = video_track.clone();
    // The client's RTCP about the video stream (PLI, FIR, NACK, REMB), which
    // 0.17 returned from `RTCRtpSender::read`, arrives as track events (see
    // rtcp_forward.rs).
    let pli_task = tokio::spawn(async move {
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
        let mut empty_polls = 0u32;
        loop {
            let packets = match video_track_for_rtcp.poll().await {
                Some(TrackLocalEvent::OnRtcpPacket(packets)) => {
                    empty_polls = 0;
                    packets
                }
                Some(_) => continue,
                None => {
                    // Not bound yet, or the connection is gone.
                    empty_polls += 1;
                    if empty_polls >= 20 {
                        break;
                    }
                    tokio::time::sleep(Duration::from_millis(250)).await;
                    continue;
                }
            };
            for packet in packets {
                let packet_any = packet.as_any();
                // A PLI or a FIR asks for an IDR frame, as GameStream's
                // IDX_REQUEST_IDR_FRAME does in Sunshine (stream.cpp:1221-1225).
                if packet_any.is::<rtc::rtcp::payload_feedbacks::picture_loss_indication::PictureLossIndication>()
                    || packet_any.is::<rtc::rtcp::payload_feedbacks::full_intra_request::FullIntraRequest>()
                {
                    let now = std::time::Instant::now();
                    let elapsed_ms = now.duration_since(last_pli_idr_time).as_millis();
                    if elapsed_ms >= 1000 {
                        // Enough time has passed — honor this PLI and generate IDR.
                        info!("Received PLI/FIR from the client, requesting IDR frame (last IDR {}ms ago)", elapsed_ms);
                        last_pli_idr_time = now;
                        if let Some(ref cmd_tx) = *pipeline_cmd_tx_for_pli.lock().unwrap() {
                            let _ = cmd_tx.try_send(lunaris_media::pipeline::PipelineCommand::RequestKeyframe);
                        }
                    } else {
                        // Too soon — suppress this PLI to avoid IDR flood on slow networks.
                        debug!("PLI/FIR suppressed: last IDR only {}ms ago (debounce=1000ms)", elapsed_ms);
                    }
                } else if let Some(nack) = packet_any.downcast_ref::<rtc::rtcp::transport_feedbacks::transport_layer_nack::TransportLayerNack>() {
                    // Diagnosis of loss recovery (debug log): which packets the
                    // client asks for again. The same number asked for twice
                    // means the first retransmission did not help.
                    if tracing::enabled!(tracing::Level::DEBUG) {
                        let mut seqs: Vec<u16> = Vec::new();
                        for pair in &nack.nacks {
                            seqs.extend(pair.packet_list());
                        }
                        debug!("NACK from the client: {} packet(s) {:?}", seqs.len(), &seqs[..seqs.len().min(12)]);
                    }
                } else if let Some(remb) = packet_any.downcast_ref::<rtc::rtcp::payload_feedbacks::receiver_estimated_maximum_bitrate::ReceiverEstimatedMaximumBitrate>() {
                    let estimated_bitrate_kbps = (remb.bitrate / 1000.0) as u32;

                    // Chrome can report very low REMB during startup before the decoder/jitter
                    // buffer settles. For realtime desktop streaming, avoid collapsing 1080p60
                    // into sub-megabit mode unless several low samples arrive after warmup.
                    let realtime_floor = if stream_fps >= 50 && resolution_width >= 1920 {
                        12_000
                    } else if stream_fps >= 50 {
                        6_000
                    } else {
                        2_000
                    };
                    // Use stream_bitrate / 2 as the floor (was / 3) to keep quality
                    // acceptable during high-motion scenes. The previous floor let
                    // REMB drop too low during window dragging, causing visible lag.
                    let bitrate_floor = stream_bitrate
                        .min((stream_bitrate / 2).max(realtime_floor).max(2_000));
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

                    // Rate-limit bitrate changes: only apply if value changed AND at
                    // least 1 second since last change. Rapid bitrate oscillation
                    // destabilizes hardware encoders, but a 2 s cooldown was too slow
                    // to recover from REMB dips during high-motion scenes.
                    if clamped_bitrate_kbps != last_bitrate_kbps
                        && now.duration_since(last_bitrate_change).as_millis() >= 1000
                    {
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

    // Create Data Channels
    let general_channel = peer_connection.create_data_channel("general", None).await?;
    let general_channel_for_cursor = general_channel.clone();

    let cursor_channel_init = RTCDataChannelInit {
        ordered: false,
        max_retransmits: Some(0),
        ..Default::default()
    };
    let cursor_channel = peer_connection
        .create_data_channel("cursor", Some(cursor_channel_init))
        .await?;
    let cursor_channel_for_cursor = cursor_channel.clone();
    let latest_host_cursor_payload = Arc::new(tokio::sync::Mutex::new(None::<String>));
    let latest_host_cursor_on_cursor_open = latest_host_cursor_payload.clone();
    let cursor_channel_on_open = cursor_channel.clone();
    // webrtc 0.21: channel events are polled. The cursor channel only needs
    // its open event (the general channel's open is handled together with its
    // messages, in setup_data_channel_handler).
    spawned_tasks.push(tokio::spawn(async move {
        while let Some(event) = cursor_channel_on_open.poll().await {
            if let DataChannelEvent::OnOpen = event {
                if let Some(payload) = latest_host_cursor_on_cursor_open.lock().await.clone() {
                    if let Err(err) = cursor_channel_on_open.send_text(&payload).await {
                        trace!(
                            "Failed to replay cached host cursor update on cursor channel: {:?}",
                            err
                        );
                    }
                }
            }
        }
    }));

    let mouse_reliable_init = RTCDataChannelInit {
        ordered: false,
        ..Default::default()
    };
    let mouse_reliable_channel = peer_connection
        .create_data_channel("mouse_reliable", Some(mouse_reliable_init))
        .await?;

    let mouse_absolute_init = RTCDataChannelInit {
        ordered: false,
        max_retransmits: Some(0),
        ..Default::default()
    };
    let mouse_absolute_channel = peer_connection
        .create_data_channel("mouse_absolute", Some(mouse_absolute_init))
        .await?;

    let mouse_relative_init = RTCDataChannelInit {
        ordered: false,
        max_retransmits: Some(0),
        ..Default::default()
    };
    let mouse_relative_channel = peer_connection
        .create_data_channel("mouse_relative", Some(mouse_relative_init))
        .await?;
    let keyboard_channel = peer_connection
        .create_data_channel("keyboard", None)
        .await?;

    // Clipboard sharing (share.rs); files have their own connection (files.rs).
    // With a source, the viewer's clipboard text and input are for the
    // computer shown: they go to the connector (connector/media.rs), and
    // this computer's clipboard, pointer, keyboard and controllers are left
    // alone.
    let (viewer_input_tx, viewer_input_rx) = tokio::sync::mpsc::unbounded_channel::<crate::connector::media::ViewerInput>();
    let (source_clipboard_tx, source_clipboard_rx) = tokio::sync::mpsc::unbounded_channel::<crate::share::ClipboardOut>();
    if from_connector {
        let (set_tx, set_rx) = std::sync::mpsc::channel::<String>();
        let to_source = viewer_input_tx.clone();
        std::thread::Builder::new()
            .name("lunaris-source-clipboard".into())
            .spawn(move || {
                while let Ok(text) = set_rx.recv() {
                    if to_source.send(crate::connector::media::ViewerInput::Clipboard(text)).is_err() {
                        break;
                    }
                }
            })
            .expect("spawn the clipboard forwarder");
        match crate::share::attach_link(&peer_connection, set_tx, source_clipboard_rx).await {
            Ok(tasks) => spawned_tasks.extend(tasks),
            Err(e) => warn!("Clipboard channel unavailable: {}", e),
        }
    } else {
        drop(source_clipboard_rx);
        match crate::share::attach(&peer_connection, agent_config.allow_clipboard).await {
            Ok(tasks) => spawned_tasks.extend(tasks),
            Err(e) => warn!("Clipboard channel unavailable: {}", e),
        }
        // Game controllers (gamepad.rs).
        if let Err(e) = crate::gamepad::attach(&peer_connection).await {
            warn!("Controller channel unavailable: {}", e);
        }
    }

    // Handle messages on Data Channels
    spawned_tasks.push(
        setup_data_channel_handler(
            general_channel,
            TransportChannel(TransportChannelId::GENERAL),
            Some(input_tx.clone()),
            Some(pipeline_cmd_tx_shared.clone()),
            Some(latest_host_cursor_payload.clone()),
        )
        .await,
    );
    spawned_tasks.push(
        setup_data_channel_handler(
            mouse_reliable_channel,
            TransportChannel(TransportChannelId::MOUSE_RELIABLE),
            Some(input_tx.clone()),
            None,
            None,
        )
        .await,
    );
    spawned_tasks.push(
        setup_data_channel_handler(
            mouse_absolute_channel,
            TransportChannel(TransportChannelId::MOUSE_ABSOLUTE),
            Some(input_tx.clone()),
            None,
            None,
        )
        .await,
    );
    spawned_tasks.push(
        setup_data_channel_handler(
            mouse_relative_channel,
            TransportChannel(TransportChannelId::MOUSE_RELATIVE),
            Some(input_tx.clone()),
            None,
            None,
        )
        .await,
    );
    spawned_tasks.push(
        setup_data_channel_handler(
            keyboard_channel,
            TransportChannel(TransportChannelId::KEYBOARD),
            Some(key_input_tx.clone()),
            None,
            None,
        )
        .await,
    );

    let (video_frame_tx, mut video_frame_rx) = tokio::sync::mpsc::channel::<VideoFramePayload>(4);
    let (audio_tx, mut audio_rx) = tokio::sync::mpsc::channel::<Sample>(256);

    let audio_track_clone = audio_track.clone();
    let audio_writer_task = tokio::spawn(async move {
        while let Some(sample) = audio_rx.recv().await {
            // Opus payload type 111 (registered above).
            if let Err(e) = audio_track_clone
                .write_sample(audio_ssrc, 111, &sample, &[])
                .await
            {
                debug!("Failed to write audio sample: {:?}", e);
            }
        }
    });
    spawned_tasks.push(audio_writer_task);

    // The display's shared pipeline (media_hub.rs): its events for this
    // viewer, and this viewer's commands to it. A connector replaces both
    // below.
    #[allow(unused_mut)]
    let (mut media_event_rx, media_membership) = match media {
        Some(joined) => {
            *pipeline_cmd_tx_shared.lock().unwrap() = Some(joined.commands);
            (joined.events, Some(joined.membership))
        }
        None => (tokio::sync::mpsc::channel(1).1, None),
    };

    let video_frame_tx_clone = video_frame_tx.clone();
    let audio_tx_clone = audio_tx.clone();

    // With a connector as the source the pipeline is never run (nothing is
    // captured or encoded here) and what the session asks of it (key frame,
    // frame rate, stop) goes to the connector.
    if let Some(source) = connector_source {
        let (connector_cmd_tx, connector_cmd_rx) = tokio::sync::mpsc::channel(16);
        *pipeline_cmd_tx_shared.lock().unwrap() = Some(connector_cmd_tx);
        // The source's cursor goes the way the pipeline's does.
        let (source_events_tx, source_events_rx) = tokio::sync::mpsc::channel(64);
        media_event_rx = source_events_rx;
        // The viewer's pointer and keys: to the connector, in the order
        // they came (the two channels are read by one task each).
        for mut packets in [std::mem::replace(&mut input_rx, tokio::sync::mpsc::unbounded_channel().1), std::mem::replace(&mut key_input_rx, tokio::sync::mpsc::unbounded_channel().1)] {
            let to_source = viewer_input_tx.clone();
            spawned_tasks.push(tokio::spawn(async move {
                while let Some((packet, _)) = packets.recv().await {
                    if to_source.send(crate::connector::media::ViewerInput::Packet(packet)).is_err() {
                        break;
                    }
                }
            }));
        }
        spawned_tasks.push(tokio::spawn(crate::connector::media::run(
            source,
            crate::connector::media::ViewerLink {
                video: video_frame_tx.clone(),
                audio: audio_tx.clone(),
                media_events: source_events_tx,
                pipeline_commands: connector_cmd_rx,
                input: viewer_input_rx,
                clipboard: source_clipboard_tx,
                general: general_channel_for_cursor.clone(),
                signaling: ws_tx.clone(),
                client_id: client_id.clone(),
                peer_connection: peer_connection.clone(),
                connected: webrtc_connected.clone(),
                fps: stream_fps,
                bitrate_kbps: stream_bitrate,
                display: display_id.as_deref().and_then(|id| id.parse::<usize>().ok()),
            },
        )));
    } else {
        drop((viewer_input_rx, source_clipboard_tx));
    }
    drop(viewer_input_tx);


    // The viewer's input is for the computer shown: with a connector as the
    // source it must never reach this one (the connectors do not take input
    // yet, so it is dropped).
    let no_local_input = from_connector || std::env::var_os(crate::connector::media::TEST_NO_LOCAL_INPUT_ENV).is_some();
    let local_injector = if no_local_input { None } else { lunaris_media::input::InputInjector::new() };
    if let Some(injector) = local_injector {
        // Windows: the USB mouse the viewer may turn on (game_mouse.rs).
        crate::game_mouse::init();
        let injector = std::sync::Arc::new(injector);
        let injector_clone = injector.clone();
        // The keys the viewer holds, let go when its input ends (below).
        let held_keys = std::sync::Arc::new(HeldKeys::default());
        let held_keys_clone = held_keys.clone();

        // Spawn a DEDICATED OS THREAD for mouse inputs — tokio::spawn can
        // starve the mouse task when the pipeline is CPU-heavy (e.g. encoding
        // 60 fps high-motion), causing progressive input lag. A dedicated thread
        // guarantees the OS scheduler gives mouse injection its own time slice.
        // The input threads are detached: they end by themselves when every
        // sender of their channel is gone (the session's data channel tasks
        // and WebTransport connections), which aborting a task could not
        // force anyway. Waiting for them in a task would hold a runtime
        // worker for the whole session.
        std::thread::Builder::new()
            .name("lunaris-mouse-input".into())
            .spawn(move || {
                info!("InputInjector: started mouse input processing thread");
                while let Some(first_packet) = input_rx.blocking_recv() {
                    process_mouse_input_batch(first_packet, &mut input_rx, &injector, &held_keys);
                    injector.flush();
                }
                // Keys may come this way too (WebTransport input).
                release_held_keys(&injector, &held_keys);
                // The viewer left: its game mouse is unplugged.
                crate::game_mouse::set_wanted(false);
                info!("InputInjector: stopped mouse input processing thread");
            })
            .expect("Failed to spawn mouse input thread");

        // Dedicated OS thread for keyboard — same reasoning as mouse.
        std::thread::Builder::new()
            .name("lunaris-key-input".into())
            .spawn(move || {
                info!("InputInjector: started keyboard input processing thread");
                while let Some((packet, last_timestamp)) = key_input_rx.blocking_recv() {
                    handle_inbound_packet(packet, &injector_clone, &last_timestamp, &held_keys_clone);
                }
                // The viewer is gone (closed, lost its connection): the
                // releases of what it held will not come.
                release_held_keys(&injector_clone, &held_keys_clone);
                info!("InputInjector: stopped keyboard input processing thread");
            })
            .expect("Failed to spawn keyboard input thread");
    } else if from_connector {
        info!("Showing another computer through a connector: the viewer's input is not injected here");
    } else {
        error!(
            "InputInjector: no input backend available — remote mouse/keyboard disabled. \
             On Wayland this needs write access to /dev/uinput (udev rule or the 'input' group); \
             on X11 it also needs a reachable DISPLAY with the XTest extension"
        );
    }

    let start_time = Instant::now();
    let webrtc_connected_for_media = webrtc_connected.clone();
    let ws_tx_for_media = ws_tx.clone();
    let client_id_for_media = client_id.clone();
    let cursor_channel_for_media = cursor_channel_for_cursor.clone();
    let general_channel_for_media = general_channel_for_cursor.clone();
    let latest_host_cursor_for_media = latest_host_cursor_payload.clone();
    let active_display_id_for_media = display_id.clone();
    let mut keyframe_after_ingress_drop = KeyframeRequester::new(pipeline_cmd_tx_shared.clone());
    let media_event_task = tokio::spawn(async move {
        let mut metrics_started = Instant::now();
        let mut forwarded_frames: u64 = 0;
        let mut dropped_frames: u64 = 0;
        let mut forwarded_bytes: u64 = 0;
        let mut latest_cursor_image: Option<serde_json::Value> = None;
        // Numbers the host_cursor messages: the cursor channel is unordered,
        // and a viewer drops a position older than one it has (with_seq).
        let mut cursor_seq: u64 = 0;
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
                            display_id: active_display_id_for_media.clone(),
                            display_name: None, // Could be resolved from display list if needed
                            // What the encoder was given (the running
                            // stream's size for a viewer that joined it).
                            width: Some(resolution_width),
                            height: Some(resolution_height),
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
                        // The next frames reference the dropped one.
                        keyframe_after_ingress_drop.request();
                    } else {
                        forwarded_frames += 1;
                        forwarded_bytes += frame_size;
                    }
                    let elapsed = metrics_started.elapsed();
                    if elapsed >= Duration::from_secs(1) {
                        let secs = elapsed.as_secs_f64();
                        debug!(
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
                        ..Sample::new(Instant::now())
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
                    if let Some(image_json) = image.as_ref() {
                        if let Some(media_image) = cursor.image.as_ref() {
                            debug!(
                                "Host cursor native image update: kind={} size={}x{} hotspot={},{} bytes={}",
                                cursor_kind,
                                media_image.width,
                                media_image.height,
                                media_image.hotspot_x,
                                media_image.hotspot_y,
                                media_image.rgba_data.len()
                            );
                        }
                        latest_cursor_image = Some(image_json.clone());
                    }
                    // What a source says of its pointer and its picture
                    // (connector/media.rs).
                    let maybe_in_picture = crate::connector::media::pointer_maybe_in_picture();
                    let in_picture = crate::connector::media::pointer_in_picture();
                    cursor_seq += 1;
                    let realtime_payload = with_seq(host_cursor_message(&cursor, image.as_ref(), maybe_in_picture, in_picture), cursor_seq);
                    let cached_payload = with_seq(host_cursor_message(&cursor, latest_cursor_image.as_ref(), maybe_in_picture, in_picture), cursor_seq);
                    *latest_host_cursor_for_media.lock().await = Some(cached_payload.clone());

                    if image.is_some()
                        && matches!(
                            general_channel_for_media.ready_state().await,
                            Ok(RTCDataChannelState::Open)
                        )
                    {
                        if let Err(err) = general_channel_for_media.send_text(&cached_payload).await
                        {
                            trace!(
                                "Failed to send reliable host cursor image update: {:?}",
                                err
                            );
                        }
                    }

                    if matches!(
                        cursor_channel_for_media.ready_state().await,
                        Ok(RTCDataChannelState::Open)
                    ) {
                        if let Err(err) = cursor_channel_for_media.send_text(&realtime_payload).await
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

        let mut keyframe_after_queue_drop = KeyframeRequester::new(pipeline_cmd_tx_for_writer.clone());
        let pipeline_cmd_tx_for_writer = pipeline_cmd_tx_for_writer.clone();
        // Send pacing. Structure of Sunshine's video broadcast loop
        // (stream.cpp:1740-1900): a frame's packets leave in 1 ms groups, each
        // group waiting for its due time, and the next frame does not start
        // before the previous one's last group. Sunshine's rate is a fixed
        // 80% of 1 Gbit/s because its transport has FEC; over WebRTC the rate
        // is libwebrtc's pacing factor, 2.5 x the stream bitrate
        // (kDefaultPaceMultiplier), raised when needed so that one frame is
        // spread over at most a quarter of the frame interval (see below). Measured without pacing over a
        // WireGuard tunnel at 15 Mbit/s: about 9 retransmission requests per
        // second from Chrome, late frames every few seconds.
        const RTP_PACKET_BYTES: u64 = 1200;
        // A source's bitrate is its own, not the one asked for here: its
        // frames are only spread over their share of the frame interval.
        // Its ordinary frames (a few packets) leave at once; only a large one
        // (a key frame) is spread, at 12 packets a millisecond.
        let pacing_packets_in_1ms: u64 = if from_connector {
            12
        } else {
            ((stream_bitrate as u64 * 1000 * 25 / 10) / 8 / RTP_PACKET_BYTES / 1000).max(1)
        };
        let pacing_frame_interval_ms: u64 = (1000 / stream_fps.max(1) as u64).max(1);
        // A frame leaves within a quarter of the frame interval. The desktop
        // client shows a frame when it arrives (no jitter buffer, like
        // Moonlight), so the time a frame takes to leave is added to its delay:
        // at 2.5 x the bitrate an average frame took 40% of the interval and a
        // large one all of it. Measured dragging a window fast (Windows host,
        // 60 fps at 15 Mbit/s over the WireGuard tunnel, 12 s): frames arriving
        // more than 25% off the interval 21.8% -> 8.1%, more than 1.5 intervals
        // apart 23 -> 5, no retransmissions. 1/8 was no better and 1/32 (as
        // good as unpaced) brought retransmissions back.
        // LUNARIS_PACING_FRAME_FRACTION overrides the 4 (1 = the whole interval).
        let pacing_frame_fraction: u64 = std::env::var("LUNARIS_PACING_FRAME_FRACTION")
            .ok()
            .and_then(|v| v.parse().ok())
            .filter(|v| *v >= 1)
            .unwrap_or(4);
        // The writer runs on its own thread: its waits are 1 ms and shorter,
        // which the async runtime's timer cannot keep (whole milliseconds,
        // and on Windows the system tick). Sunshine sleeps here with its
        // high-precision timer (stream.cpp:1857-1860); a thread sleep is the
        // equivalent (a high-resolution waitable timer on Windows).
        let writer_runtime = tokio::runtime::Handle::current();
        std::thread::Builder::new().name("rtp-writer".into()).spawn(move || writer_runtime.block_on(async move {
            let mut seq: u16 = 0;
            let mut ratecontrol_next_frame_start = Instant::now();
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
                        if let Some(ref cmd_tx) = *pipeline_cmd_tx_for_writer.lock().unwrap() {
                            let _ = cmd_tx.try_send(
                                lunaris_media::pipeline::PipelineCommand::RequestKeyframe,
                            );
                        }
                    }
                }

                let writer_frame_started = Instant::now();
                let mut writer_sleep = Duration::ZERO;
                let mut writer_sleeps = 0u32;
                let mut writer_write_max = Duration::ZERO;
                let packet_count = packets.len() as u64;
                let frame_rtp_time = packets.first().map(|packet| packet.header.timestamp);
                trace!(
                    "Writer task sending {} RTP packets (first seq={})",
                    packets.len(),
                    seq
                );
                // stream.cpp:1740: packets per 1 ms group; here also at least
                // enough to finish the frame within its share of the interval.
                let packets_in_1ms =
                    pacing_packets_in_1ms.max((packet_count * pacing_frame_fraction).div_ceil(pacing_frame_interval_ms));
                // stream.cpp:1753: "Don't ignore the last ratecontrol group of
                // the previous frame".
                let ratecontrol_frame_start = ratecontrol_next_frame_start.max(Instant::now());
                let mut ratecontrol_frame_packets_sent: u64 = 0;
                let mut ratecontrol_group_packets_sent: u64 = 0;
                let mut extensions = [
                    HeaderExtension::PlayoutDelay(PlayoutDelayExtension::new(0, 0)),
                    HeaderExtension::AbsSendTime(AbsSendTimeExtension { timestamp: 0 }),
                ];
                for mut packet in packets {
                    // stream.cpp:1853-1864: pacing within the frame, and
                    // before its first group.
                    if ratecontrol_group_packets_sent >= packets_in_1ms
                        || ratecontrol_frame_packets_sent == 0
                    {
                        let due = ratecontrol_frame_start
                            + Duration::from_nanos(
                                1_000_000 * ratecontrol_frame_packets_sent / packets_in_1ms,
                            );
                        let now = Instant::now();
                        if now < due {
                            std::thread::sleep(due - now);
                            writer_sleep += now.elapsed();
                            writer_sleeps += 1;
                        }
                        ratecontrol_group_packets_sent = 0;

                        // AbsSendTime of this group, for the receiver's
                        // bandwidth estimate (REMB): the time it really leaves.
                        let now = std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .unwrap_or_default();
                        let now_secs = now.as_secs() as f64 + now.subsec_nanos() as f64 * 1e-9;
                        extensions = [
                            // PlayoutDelay(0, 0): min=0, max=0 playout delay.
                            // Tells Chrome to use ZERO jitter buffer delay — critical for
                            // preventing latency accumulation that makes mouse feel slower.
                            HeaderExtension::PlayoutDelay(PlayoutDelayExtension::new(0, 0)),
                            HeaderExtension::AbsSendTime(AbsSendTimeExtension {
                                timestamp: (now_secs * 262_144.0) as u64,
                            }),
                        ];
                    }
                    packet.header.sequence_number = seq;
                    seq = seq.wrapping_add(1);
                    let write_started = Instant::now();
                    if let Err(e) = video_track_clone
                        .write_rtp_with_extensions(packet, &extensions)
                        .await
                    {
                        debug!("Failed to write video RTP packet: {:?}", e);
                    }
                    writer_write_max = writer_write_max.max(write_started.elapsed());
                    ratecontrol_group_packets_sent += 1;
                    ratecontrol_frame_packets_sent += 1;
                }
                if let Some(rtp_time) = frame_rtp_time {
                    crate::connector::media::frame_sent(rtp_time);
                }
                if writer_frame_started.elapsed() > Duration::from_millis(9) {
                    debug!(
                        "Slow RTP write: {} packets took {:.1} ms: {} sleeps {:.1} ms, longest write {:.2} ms (pacing {} per ms, queue {})",
                        packet_count,
                        writer_frame_started.elapsed().as_secs_f64() * 1000.0,
                        writer_sleeps,
                        writer_sleep.as_secs_f64() * 1000.0,
                        writer_write_max.as_secs_f64() * 1000.0,
                        packets_in_1ms,
                        packet_rx.len()
                    );
                }
                // stream.cpp:1899-1901.
                ratecontrol_next_frame_start = ratecontrol_frame_start
                    + Duration::from_nanos(1_000_000 * ratecontrol_frame_packets_sent / packets_in_1ms);
                sent_frames += 1;
                sent_packets += packet_count;
                let elapsed = metrics_started.elapsed();
                if elapsed >= Duration::from_secs(1) {
                    let secs = elapsed.as_secs_f64();
                    debug!(
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
        })).expect("spawn rtp-writer thread");

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
                    let mut reader = H264Reader::new(frame_buffer(full_frame));
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
                    let mut reader = H265Reader::new(frame_buffer(full_frame));
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
                    // AV1 from lunaris-media/FFmpeg is a raw OBU temporal unit, not an
                    // Annex-B NAL stream. The WebRTC AV1 payloader parses the OBU
                    // sequence itself, so pass the complete encoded frame through.
                    if !full_frame.is_empty() {
                        samples.push(frame_buffer(full_frame));
                    }
                }
                VideoPayloader::Vp8(_) | VideoPayloader::Vp9(_) => {
                    // One VP8 or VP9 frame is one sample.
                    if !full_frame.is_empty() {
                        samples.push(frame_buffer(full_frame));
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
                    VideoPayloader::Vp8(p) => p.payload(1200 - 12, &sample.freeze()),
                    VideoPayloader::Vp9(p) => p.payload(1200 - 12, &sample.freeze()),
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
                            // 0.17 filled in the SSRC when writing; 0.21 sends what it gets.
                            ssrc: video_ssrc,
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
                // Sequence numbers are assigned by the writer, so the client
                // sees no gap: it needs a key frame to recover.
                keyframe_after_queue_drop.request();
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
    let mut webtransport_token = None;

    // The token in the WebTransport URL path proves a connection comes from
    // this session's client (it travels in the signaling offer). Without a
    // random source there is no endpoint: input stays on the data channels.
    let token = match new_webtransport_token() {
        Ok(token) => Some(token),
        Err(e) => {
            error!("WebTransport disabled for this session: {}", e);
            None
        }
    };

    match token.map(|token| (token, wtransport::Identity::self_signed(&["localhost", "127.0.0.1", "::1"]))) {
        None => {}
        Some((_, Err(e))) => {
            error!("Failed to generate self-signed certificate: {:?}", e);
        }
        Some((token, Ok(identity))) => {
            let cert_hash = identity.certificate_chain().as_slice()[0]
                .hash()
                .as_ref()
                .iter()
                .map(|b| format!("{:02x}", b))
                .collect::<String>();

            // The previous session's endpoint may still hold the port for a
            // moment: retry, then take any free port (the offer carries the
            // port, so clients follow it).
            // With other sessions alive the port is theirs: one try, then
            // any free port, without waiting.
            let attempts: u32 = if LIVE_SESSIONS.load(Ordering::SeqCst) > 0 { 1 } else { 10 };
            let mut bound = None;
            for attempt in 0..=attempts {
                let port = if attempt < attempts { agent_config.webtransport_port } else { 0 };
                let config = wtransport::ServerConfig::builder()
                    .with_bind_default(port)
                    .with_identity(identity.clone_identity())
                    .build();
                match wtransport::Endpoint::server(config) {
                    Ok(endpoint) => {
                        bound = Some(Ok(endpoint));
                        break;
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::AddrInUse && attempt < attempts => {
                        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
                    }
                    Err(e) => {
                        bound = Some(Err(e));
                        break;
                    }
                }
            }

            match bound.expect("the loop always sets it") {
                Ok(endpoint) => {
                    if let Ok(addr) = endpoint.local_addr() {
                        let port = addr.port();
                        info!("WebTransport server listening on port {}", port);
                        webtransport_port = Some(port);
                        webtransport_cert_hash = Some(cert_hash);
                        webtransport_token = Some(token.clone());

                        let endpoint_arc = Arc::new(endpoint);
                        webtransport_endpoint = Some(endpoint_arc.clone());

                        let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel::<()>();
                        webtransport_shutdown = Some(shutdown_tx);

                        let gate = Arc::new(WebTransportGate::new(token));
                        tokio::spawn(serve_webtransport_input(
                            endpoint_arc,
                            gate,
                            input_tx.clone(),
                            shutdown_rx,
                        ));
                    }
                }
                Err(e) => {
                    error!("Failed to start WebTransport endpoint: {:?}", e);
                }
            }
        }
    }

    let pipeline_cmd = pipeline_cmd_tx_shared.lock().unwrap().clone();

    Ok(Arc::new(BridgeSession {
        peer_connection,
        webtransport_port,
        webtransport_cert_hash,
        webtransport_token,
        _webtransport_endpoint: webtransport_endpoint,
        webtransport_shutdown,
        pipeline_cmd,
        spawned_tasks,
        _media: media_membership,
        _live: LiveSession::new(),
    }))
}

/// Failed WebTransport handshakes (bad token, broken handshake) from one
/// address after which a session's endpoint accepts no more connections
/// from it. Other addresses (the session's own client) are still served.
const WEBTRANSPORT_MAX_FAILED_HANDSHAKES: u32 = 20;
/// Addresses whose failures a session's endpoint remembers; beyond them a
/// new address is not counted (the 256-bit token is what keeps it out).
const WEBTRANSPORT_MAX_TRACKED: usize = 1024;

/// A random per-session secret, 32 bytes as hex, from the OS random source
/// (through rustls' ring provider, which wtransport already uses).
fn new_webtransport_token() -> Result<String, String> {
    let mut bytes = [0u8; 32];
    wtransport::tls::rustls::crypto::ring::default_provider()
        .secure_random
        .fill(&mut bytes)
        .map_err(|_| "no OS random source for the WebTransport token".to_string())?;
    Ok(bytes.iter().map(|b| format!("{:02x}", b)).collect())
}

/// Compares in time independent of where the inputs differ.
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// Who may feed input through a session's WebTransport endpoint: a request
/// whose path is `/<token>`, one connection at a time, and no address from
/// which too many handshakes failed (so a stranger's failures do not lock
/// the session's own client out).
pub(crate) struct WebTransportGate {
    expected_path: String,
    connected: AtomicBool,
    /// Failed handshakes per remote address (IP; any port).
    failures: std::sync::Mutex<std::collections::HashMap<std::net::IpAddr, u32>>,
}

impl WebTransportGate {
    pub(crate) fn new(token: String) -> Self {
        Self {
            expected_path: format!("/{token}"),
            connected: AtomicBool::new(false),
            failures: std::sync::Mutex::new(std::collections::HashMap::new()),
        }
    }

    fn failures_from(&self, ip: std::net::IpAddr) -> u32 {
        self.failures.lock().unwrap().get(&ip).copied().unwrap_or(0)
    }

    fn locked(&self, from: std::net::SocketAddr) -> bool {
        self.failures_from(from.ip()) >= WEBTRANSPORT_MAX_FAILED_HANDSHAKES
    }

    fn record_failure(&self, from: std::net::SocketAddr, what: &str) {
        let failures = {
            let mut map = self.failures.lock().unwrap();
            if !map.contains_key(&from.ip()) && map.len() >= WEBTRANSPORT_MAX_TRACKED {
                warn!("WebTransport: {} from {} (not counted: too many addresses failed)", what, from);
                return;
            }
            let count = map.entry(from.ip()).or_insert(0);
            *count += 1;
            *count
        };
        warn!("WebTransport: {} from {} ({} failed from there)", what, from, failures);
        if failures == WEBTRANSPORT_MAX_FAILED_HANDSHAKES {
            warn!(
                "WebTransport: {} failed handshakes from {}, refusing its further connections for this session",
                failures,
                from.ip()
            );
        }
    }

    fn path_allowed(&self, path: &str) -> bool {
        constant_time_eq(path.as_bytes(), self.expected_path.as_bytes())
    }

    /// Takes the session's single connection slot.
    fn try_claim(&self) -> Option<WebTransportSlot<'_>> {
        self.connected
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .ok()
            .map(|_| WebTransportSlot(self))
    }
}

/// The connection slot, given back when the connection ends.
struct WebTransportSlot<'a>(&'a WebTransportGate);

impl Drop for WebTransportSlot<'_> {
    fn drop(&mut self) {
        self.0.connected.store(false, Ordering::SeqCst);
    }
}

/// Completes the handshake of an incoming connection and accepts it only
/// when the request carries the session's token and no other connection is
/// alive; everything else is refused before any datagram is read.
async fn authenticate_webtransport<'a>(
    incoming: wtransport::endpoint::IncomingSession,
    gate: &'a WebTransportGate,
) -> Option<(wtransport::Connection, WebTransportSlot<'a>)> {
    let remote = incoming.remote_address();
    if gate.locked(remote) {
        incoming.refuse();
        return None;
    }
    let request = match incoming.await {
        Ok(request) => request,
        Err(e) => {
            gate.record_failure(remote, &format!("handshake failed: {e}"));
            return None;
        }
    };
    if !gate.path_allowed(request.path()) {
        gate.record_failure(remote, "request without the session token");
        request.forbidden().await;
        return None;
    }
    let Some(slot) = gate.try_claim() else {
        info!("WebTransport: refusing a second connection from {} while one is open", remote);
        request.too_many_requests().await;
        return None;
    };
    match request.accept().await {
        Ok(connection) => Some((connection, slot)),
        Err(e) => {
            error!("WebTransport session accept failed: {:?}", e);
            None
        }
    }
}

/// The accept loop of a session's WebTransport endpoint: authenticated
/// connections' datagrams go to the input queue, until `shutdown_rx` fires.
pub(crate) async fn serve_webtransport_input(
    endpoint: Arc<wtransport::Endpoint<wtransport::endpoint::endpoint_side::Server>>,
    gate: Arc<WebTransportGate>,
    input_tx: mpsc::UnboundedSender<InputQueueItem>,
    mut shutdown_rx: tokio::sync::oneshot::Receiver<()>,
) {
    loop {
        tokio::select! {
            incoming = endpoint.accept() => {
                if gate.locked(incoming.remote_address()) {
                    incoming.refuse();
                    continue;
                }
                let input_tx = input_tx.clone();
                let gate = gate.clone();
                tokio::spawn(async move {
                    let remote = incoming.remote_address();
                    debug!("Incoming WebTransport connection from {}", remote);
                    let Some((connection, _slot)) = authenticate_webtransport(incoming, &gate).await else {
                        return;
                    };
                    info!("WebTransport session from {} accepted", remote);
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
                // Close the session's connections too: an open
                // connection keeps the UDP port until its idle
                // timeout, and the next session then has to take
                // a random port.
                endpoint.close(wtransport::VarInt::from_u32(0), b"session ended");
                break;
            }
        }
    }
}

async fn setup_data_channel_handler(
    data_channel: Arc<dyn DataChannel>,
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
    // The latest host cursor payload, replayed when the channel opens.
    replay_on_open: Option<Arc<tokio::sync::Mutex<Option<String>>>>,
) -> tokio::task::JoinHandle<()> {
    let last_timestamp = Arc::new(std::sync::atomic::AtomicU32::new(0));
    let label = data_channel.label().await.unwrap_or_default();
    info!(
        "setup_data_channel_handler: label={}, channel_id={}, has_input_tx={}",
        label,
        channel.0,
        input_tx.is_some()
    );
    // Diagnosis of input stalls (debug log): a pause of 60 ms to 1 s between
    // two mouse messages that follows a run of closely spaced ones.
    let mouse_gap_label = label.clone();
    let mouse_gap_state = Arc::new(std::sync::Mutex::new((None::<Instant>, false)));
    // webrtc 0.21: the channel's events are polled by this task.
    tokio::spawn(async move {
        while let Some(event) = data_channel.poll().await {
            let msg = match event {
                DataChannelEvent::OnMessage(msg) => msg,
                DataChannelEvent::OnOpen => {
                    if let Some(cache) = replay_on_open.as_ref() {
                        if let Some(payload) = cache.lock().await.clone() {
                            if let Err(err) = data_channel.send_text(&payload).await {
                                trace!(
                                    "Failed to replay cached host cursor update on {} channel: {:?}",
                                    mouse_gap_label,
                                    err
                                );
                            }
                        }
                    }
                    continue;
                }
                _ => continue,
            };
            if mouse_gap_label.starts_with("mouse") {
                let now = Instant::now();
                let mut st = mouse_gap_state.lock().unwrap();
                if let Some(prev) = st.0 {
                    let gap = now.duration_since(prev);
                    if st.1 && gap >= Duration::from_millis(60) && gap <= Duration::from_secs(1) {
                        debug!("Input gap: {} ms without a message on {}", gap.as_millis(), mouse_gap_label);
                    }
                    st.1 = gap < Duration::from_millis(40);
                }
                st.0 = Some(now);
            }
            let input_tx = input_tx.clone();
            let last_timestamp = last_timestamp.clone();
            let pipeline_cmd_tx = pipeline_cmd_tx.clone();
            // The body of the 0.17 `on_message` callback, unchanged.
            async {
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
                        // Sunshine stream.cpp:1221-1225 (IDX_REQUEST_IDR_FRAME):
                        // the client asks for an IDR frame.
                        InboundPacket::RequestVideoIdr => {
                            debug!("IDR frame requested via data channel");
                            if let Some(ref cmd_tx) = *pipeline_cmd.lock().unwrap() {
                                let _ = cmd_tx.try_send(
                                    lunaris_media::pipeline::PipelineCommand::RequestKeyframe,
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
            }
            .await;
        }
    })
}

type InputQueueItem = (InboundPacket, Arc<std::sync::atomic::AtomicU32>);

fn process_mouse_input_batch(
    first: InputQueueItem,
    input_rx: &mut mpsc::UnboundedReceiver<InputQueueItem>,
    injector: &lunaris_media::input::InputInjector,
    held_keys: &HeldKeys,
) {
    let mut pending = Some(first);
    while let Ok(next) = input_rx.try_recv() {
        if let Some(current) = pending.take() {
            match coalesce_mouse_input(current, next) {
                Ok(coalesced) => pending = Some(coalesced),
                Err((current, next)) => {
                    handle_inbound_packet(current.0, injector, &current.1, held_keys);
                    pending = Some(next);
                }
            }
        } else {
            pending = Some(next);
        }
    }

    if let Some((packet, last_timestamp)) = pending {
        handle_inbound_packet(packet, injector, &last_timestamp, held_keys);
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

/// The keys a viewer has pressed here and not let go (virtual-key codes).
/// When the viewer's input ends with keys still down - it closed, or lost
/// its connection, in the middle of a combination - they are let go, or the
/// key would stay held on this computer for whoever uses it next
/// (moonlight-qt raises its keys the same way from the viewer's side,
/// keyboard.cpp raiseAllKeys 521-537; the host cannot count on that).
#[derive(Default)]
struct HeldKeys(std::sync::Mutex<Vec<u16>>);

impl HeldKeys {
    fn note(&self, key: u16, is_press: bool) {
        let mut held = self.0.lock().unwrap();
        held.retain(|k| *k != key);
        if is_press {
            held.push(key);
        }
    }

    /// The keys still held, last pressed first; none are held afterwards.
    fn take(&self) -> Vec<u16> {
        let mut held = std::mem::take(&mut *self.0.lock().unwrap());
        held.reverse();
        held
    }
}

/// Keys the Linux injector has no entry for when named by virtual-key code
/// (lunaris-media `vk_to_linux_keycode` and `vk_to_keysym`: Pause, Caps
/// Lock, Print Screen, the menu key, F13 to F24, the key beside the left
/// Shift of ISO keyboards, the volume keys), as the kernel's key code from
/// `connector/keys.rs`. They are pressed by position instead
/// (`InputInjector::key_position`); before, they were dropped on a Linux
/// host. None for every other key.
#[cfg(any(target_os = "linux", test))]
fn linux_position_of_unmapped_vk(key: u16) -> Option<u32> {
    match key {
        0x13 | 0x14 | 0x2C | 0x5D | 0x7C..=0x87 | 0xE2 | 0xAD..=0xAF => {
            crate::connector::keys::position_of_vk(key)
        }
        _ => None,
    }
}

fn inject_key(injector: &lunaris_media::input::InputInjector, key: u16, is_press: bool) {
    #[cfg(target_os = "linux")]
    if let Some(position) = linux_position_of_unmapped_vk(key) {
        injector.key_position(position, is_press);
        return;
    }
    injector.keyboard_key(key, is_press);
}

fn release_held_keys(injector: &lunaris_media::input::InputInjector, held_keys: &HeldKeys) {
    let held = held_keys.take();
    if held.is_empty() {
        return;
    }
    // How many, never which (privacy).
    info!("The viewer's input ended with {} key(s) held: let go", held.len());
    for key in held {
        inject_key(injector, key, false);
    }
    injector.flush();
}

fn handle_inbound_packet(
    packet: InboundPacket,
    injector: &lunaris_media::input::InputInjector,
    last_timestamp: &std::sync::atomic::AtomicU32,
    held_keys: &HeldKeys,
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
                crate::game_mouse::absolute_move();
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
            // A locked pointer: through the game mouse when it is plugged in
            // (game_mouse.rs: anti-cheats drop injected moves).
            let through_game_mouse = crate::game_mouse::relative_move(delta_x as i32, delta_y as i32);
            if !through_game_mouse {
                injector.move_mouse_relative(delta_x as i32, delta_y as i32);
            }
        }
        InboundPacket::MouseButton { action, button } => {
            let is_press = match action {
                MouseButtonAction::Press => true,
                MouseButtonAction::Release => false,
            };
            let (x11_btn, hid_bit) = match button {
                MouseButton::Left => (1, 0),
                MouseButton::Middle => (2, 2),
                MouseButton::Right => (3, 1),
                MouseButton::X1 => (8, 3),
                MouseButton::X2 => (9, 4),
            };
            if !crate::game_mouse::button(hid_bit, is_press) {
                injector.mouse_button(x11_btn, is_press);
            }
        }
        InboundPacket::GameMouse { on } => crate::game_mouse::set_wanted(on),
        InboundPacket::Scroll { delta_y, delta_x } => {
            let (delta_y, delta_x) = (
                if crate::game_mouse::wheel(delta_y as i32, true) { 0 } else { delta_y },
                if crate::game_mouse::wheel(delta_x as i32, false) { 0 } else { delta_x },
            );
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
            let click_y = if crate::game_mouse::wheel(click_y as i32, true) { 0 } else { click_y };
            let click_x = if crate::game_mouse::wheel(click_x as i32, false) { 0 } else { click_x };

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

            held_keys.note(key, is_press);
            inject_key(injector, key, is_press);
        }
        InboundPacket::Text { text } => {
            crate::share::type_text(injector, &text);
        }
        _ => {}
    }
}

/// The `host_cursor` message for the viewers. Viewers read every field but
/// `type`, `x` and `y` as optional, and fields they do not know not at all.
/// `image`: the cursor's picture as JSON (it goes with the message that
/// brings a new one, and with the message kept for a viewer that opens its
/// channel later).
/// `maybe_in_picture`: a source that cannot know whether its pointer is in
/// its picture now; the viewer looks for it in the picture itself (viewers
/// from before this field keep their own pointer).
/// `in_picture`: `Some("always")` for a source whose pointer is part of its
/// picture for the whole session; the viewer shows no pointer over the
/// picture (viewers from before this field keep their own pointer).
/// Otherwise null.
fn host_cursor_message(
    cursor: &lunaris_media::types::CursorState,
    image: Option<&serde_json::Value>,
    maybe_in_picture: bool,
    in_picture: Option<&str>,
) -> String {
    serde_json::json!({
        "type": "host_cursor",
        "x": cursor.x,
        "y": cursor.y,
        "visible": cursor.visible,
        "kind": cursor.kind.as_str(),
        "image": image,
        "in_window_move_size": cursor.in_window_move_size,
        "pointer_maybe_in_picture": maybe_in_picture,
        "pointer_in_picture": in_picture,
    })
    .to_string()
}

#[cfg(test)]
mod held_keys_tests {
    use super::{linux_position_of_unmapped_vk, HeldKeys};

    #[test]
    fn keys_still_down_are_the_ones_to_let_go() {
        let held = HeldKeys::default();
        held.note(0xA2, true); // Ctrl
        held.note(0x12, true); // Alt
        held.note(0x09, true); // Tab
        held.note(0x09, false);
        held.note(0x12, true); // the key repeating
        assert_eq!(held.take(), vec![0x12, 0xA2]);
        assert!(held.take().is_empty());
        // A release without a press holds nothing.
        held.note(0x41, false);
        assert!(held.take().is_empty());
    }

    #[test]
    fn keys_the_linux_injector_lacks_go_by_position() {
        assert_eq!(linux_position_of_unmapped_vk(0x14), Some(58)); // Caps Lock
        assert_eq!(linux_position_of_unmapped_vk(0x2C), Some(99)); // Print Screen
        assert_eq!(linux_position_of_unmapped_vk(0x13), Some(119)); // Pause
        assert_eq!(linux_position_of_unmapped_vk(0x5D), Some(127)); // menu key
        assert_eq!(linux_position_of_unmapped_vk(0x7C), Some(183)); // F13
        assert_eq!(linux_position_of_unmapped_vk(0xE2), Some(86)); // ISO key
        // The keys it has stay with it.
        assert_eq!(linux_position_of_unmapped_vk(0x41), None);
        assert_eq!(linux_position_of_unmapped_vk(0x5B), None);
        assert_eq!(linux_position_of_unmapped_vk(0x7B), None); // F12
        assert_eq!(linux_position_of_unmapped_vk(0xA5), None);
    }
}

/// A `host_cursor` message with its number (`seq`): positions come on an
/// unordered channel, so a viewer keeps the newest one it has seen. Viewers
/// from before this field ignore it.
fn with_seq(message: String, seq: u64) -> String {
    match serde_json::from_str::<serde_json::Value>(&message) {
        Ok(serde_json::Value::Object(mut fields)) => {
            fields.insert("seq".into(), seq.into());
            serde_json::Value::Object(fields).to_string()
        }
        _ => message,
    }
}

#[cfg(test)]
mod host_cursor_message_tests {
    use super::host_cursor_message;
    use lunaris_media::types::{CursorKind, CursorState};

    fn cursor() -> CursorState {
        CursorState { x: 10, y: 20, visible: true, kind: CursorKind::Arrow, image: None, in_window_move_size: false }
    }

    fn parsed(text: &str) -> serde_json::Value {
        serde_json::from_str(text).unwrap()
    }

    /// An ordinary host and a source that says nothing of its picture: the
    /// message is what it was before `pointer_in_picture`, with that field null.
    #[test]
    fn nothing_said_of_the_picture() {
        let message = parsed(&host_cursor_message(&cursor(), None, false, None));
        assert_eq!(
            message,
            serde_json::json!({
                "type": "host_cursor", "x": 10, "y": 20, "visible": true, "kind": "arrow", "image": null,
                "in_window_move_size": false, "pointer_maybe_in_picture": false, "pointer_in_picture": null,
            })
        );
    }

    /// The three things a host can say are three fields; none stands for another.
    #[test]
    fn always_in_the_picture_is_its_own_field() {
        let message = parsed(&host_cursor_message(&cursor(), None, false, Some("always")));
        assert_eq!(message["pointer_in_picture"], "always");
        assert_eq!(message["in_window_move_size"], false);
        assert_eq!(message["pointer_maybe_in_picture"], false);

        let mut moved = cursor();
        moved.in_window_move_size = true;
        let message = parsed(&host_cursor_message(&moved, None, true, None));
        assert_eq!(message["in_window_move_size"], true);
        assert_eq!(message["pointer_maybe_in_picture"], true);
        assert!(message["pointer_in_picture"].is_null());
    }

    #[test]
    fn a_message_carries_its_number() {
        let message = parsed(&super::with_seq(host_cursor_message(&cursor(), None, false, None), 42));
        assert_eq!(message["seq"], 42);
        assert_eq!(message["x"], 10);
    }

    #[test]
    fn the_cursors_picture_goes_with_it() {
        let image = serde_json::json!({"width": 1, "height": 1, "hotspot_x": 0, "hotspot_y": 0, "rgba": "AAAAAA=="});
        let message = parsed(&host_cursor_message(&cursor(), Some(&image), false, None));
        assert_eq!(message["image"], image);
    }
}

#[cfg(test)]
mod codec_support_tests {
    use super::*;
    use lunaris_media::{EncoderInfo, HwAccelType, VideoCodec};

    #[test]
    fn a_viewer_joins_a_stream_it_decodes() {
        let running = |codec, yuv444| lunaris_media::types::StreamConfig { codec, yuv444, ..Default::default() };
        let names = |n: &[&str]| n.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        // The codec it would get anyway, or one it says it decodes.
        assert!(viewer_decodes(&running(VideoCodec::H264, false), "h264", false, None));
        assert!(viewer_decodes(&running(VideoCodec::H265, false), "h264", false, Some(&names(&["h264", "H265"]))));
        // Not one it does not name (older clients name none).
        assert!(!viewer_decodes(&running(VideoCodec::H265, false), "h264", false, None));
        assert!(!viewer_decodes(&running(VideoCodec::AV1, false), "h264", false, Some(&names(&["h264", "h265"]))));
        // 4:4:4 only for a viewer that asked for it; 4:2:0 for anyone.
        assert!(!viewer_decodes(&running(VideoCodec::H264, true), "h264", false, None));
        assert!(viewer_decodes(&running(VideoCodec::H264, true), "h264", true, None));
        assert!(viewer_decodes(&running(VideoCodec::H264, false), "h264", true, None));
    }

    fn listed(codecs: &[VideoCodec], hw_type: HwAccelType) -> Vec<EncoderInfo> {
        vec![EncoderInfo { name: "test".into(), hw_type, supported_codecs: codecs.to_vec() }]
    }

    #[test]
    fn h264_follows_the_encoder_list() {
        let sw = listed(&[VideoCodec::H264], HwAccelType::Software);
        assert!(codec_supported_for_realtime(&sw, VideoCodec::H264, false));
        assert!(!codec_supported_for_realtime(&[], VideoCodec::H264, true));
    }

    #[test]
    fn h265_and_av1_need_a_hardware_probe_success() {
        let hw = listed(&[VideoCodec::H264, VideoCodec::H265, VideoCodec::AV1], HwAccelType::Amf);
        for codec in [VideoCodec::H265, VideoCodec::AV1] {
            assert!(codec_supported_for_realtime(&hw, codec, true));
            // Listed but the encoder does not open: not offered.
            assert!(!codec_supported_for_realtime(&hw, codec, false));
            // Probe success does not depend on the list.
            assert!(codec_supported_for_realtime(&[], codec, true));
        }
    }
}

#[cfg(test)]
mod fec_answer_tests {
    use super::flexfec_in_answer;

    const HEAD: &str = "v=0\r\nm=audio 9 UDP/TLS/RTP/SAVPF 111\r\na=rtpmap:111 opus/48000/2\r\n";

    #[test]
    fn finds_an_accepted_flexfec() {
        let sdp = format!("{HEAD}m=video 9 UDP/TLS/RTP/SAVPF 96 49\r\na=rtpmap:96 H264/90000\r\na=rtpmap:49 flexfec-03/90000\r\na=fmtp:49 repair-window=10000000\r\n");
        assert_eq!(flexfec_in_answer(&sdp), Some(49));
    }

    #[test]
    fn flexfec_dropped_from_the_answer() {
        let sdp = format!("{HEAD}m=video 9 UDP/TLS/RTP/SAVPF 96\r\na=rtpmap:96 H264/90000\r\n");
        assert_eq!(flexfec_in_answer(&sdp), None);
        // Described but not in the format list: not accepted.
        let sdp = format!("{HEAD}m=video 9 UDP/TLS/RTP/SAVPF 96\r\na=rtpmap:49 flexfec-03/90000\r\n");
        assert_eq!(flexfec_in_answer(&sdp), None);
        // Rejected video section.
        let sdp = format!("{HEAD}m=video 0 UDP/TLS/RTP/SAVPF 96 49\r\na=rtpmap:49 flexfec-03/90000\r\n");
        assert_eq!(flexfec_in_answer(&sdp), None);
    }
}

#[cfg(test)]
mod webtransport_auth_tests {
    use super::*;
    use wtransport::{ClientConfig, Endpoint, Identity, ServerConfig};

    struct Server {
        port: u16,
        cert_hash: wtransport::tls::Sha256Digest,
        token: String,
        gate: Arc<WebTransportGate>,
        input_rx: mpsc::UnboundedReceiver<InputQueueItem>,
        _shutdown: tokio::sync::oneshot::Sender<()>,
    }

    async fn start_server() -> Server {
        let identity = Identity::self_signed(["localhost", "127.0.0.1"]).unwrap();
        let cert_hash = identity.certificate_chain().as_slice()[0].hash();
        let config = ServerConfig::builder()
            .with_bind_address("127.0.0.1:0".parse().unwrap())
            .with_identity(identity)
            .build();
        let endpoint = Arc::new(Endpoint::server(config).unwrap());
        let port = endpoint.local_addr().unwrap().port();
        let token = new_webtransport_token().unwrap();
        let gate = Arc::new(WebTransportGate::new(token.clone()));
        let (input_tx, input_rx) = mpsc::unbounded_channel();
        let (shutdown, shutdown_rx) = tokio::sync::oneshot::channel();
        tokio::spawn(serve_webtransport_input(endpoint, gate.clone(), input_tx, shutdown_rx));
        Server { port, cert_hash, token, gate, input_rx, _shutdown: shutdown }
    }

    const LOCAL: std::net::IpAddr = std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST);

    async fn connect(server: &Server, path: &str) -> Result<wtransport::Connection, String> {
        connect_from(server, LOCAL, path).await
    }

    async fn connect_from(server: &Server, from: std::net::IpAddr, path: &str) -> Result<wtransport::Connection, String> {
        let config = ClientConfig::builder()
            .with_bind_address(std::net::SocketAddr::new(from, 0))
            .with_server_certificate_hashes([server.cert_hash.clone()])
            .build();
        let client = Endpoint::client(config).unwrap();
        let url = format!("https://127.0.0.1:{}{}", server.port, path);
        match tokio::time::timeout(Duration::from_secs(10), client.connect(url)).await {
            Ok(Ok(connection)) => Ok(connection),
            Ok(Err(e)) => Err(format!("{e:?}")),
            Err(_) => Err("timed out".into()),
        }
    }

    /// A relative mouse move on the MOUSE_RELATIVE channel.
    fn mouse_move_datagram() -> Vec<u8> {
        vec![TransportChannelId::MOUSE_RELATIVE, 0, 0, 5, 0, 7]
    }

    async fn next_input(server: &mut Server) -> Option<InboundPacket> {
        tokio::time::timeout(Duration::from_secs(2), server.input_rx.recv())
            .await
            .ok()
            .flatten()
            .map(|(packet, _)| packet)
    }

    #[test]
    fn token_is_32_random_bytes_as_hex() {
        let a = new_webtransport_token().unwrap();
        let b = new_webtransport_token().unwrap();
        assert_eq!(a.len(), 64);
        assert!(a.bytes().all(|c| c.is_ascii_hexdigit()));
        assert_ne!(a, b);
    }

    #[test]
    fn path_check_needs_the_exact_token() {
        let gate = WebTransportGate::new("ab12".into());
        assert!(gate.path_allowed("/ab12"));
        for path in ["/", "", "ab12", "/ab1", "/ab123", "/AB12", "/ab12/", "/ab12?x=1", "//ab12"] {
            assert!(!gate.path_allowed(path), "{path}");
        }
    }

    #[test]
    fn old_offers_without_a_token_still_parse() {
        let json = r#"{"type":"Sdp","payload":{"target_id":"c","sdp":{"ty":"offer","sdp":"v=0"},"ice_servers":null,"webtransport_port":55200,"webtransport_cert_hash":"00"}}"#;
        match serde_json::from_str::<common::SignalingMessage>(json) {
            Ok(common::SignalingMessage::Sdp { webtransport_token, .. }) => assert!(webtransport_token.is_none()),
            other => panic!("unexpected {:?}", other.map(|_| ())),
        }
        // And a new offer carries it.
        let offer = common::SignalingMessage::Sdp {
            target_id: "c".into(),
            sdp: common::RtcSessionDescription { ty: common::RtcSdpType::Offer, sdp: "v=0".into() },
            ice_servers: None,
            webtransport_port: Some(1),
            webtransport_cert_hash: None,
            webtransport_token: Some("ab".into()),
        };
        assert!(serde_json::to_string(&offer).unwrap().contains(r#""webtransport_token":"ab""#));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn right_token_is_accepted_and_its_input_arrives() {
        let mut server = start_server().await;
        let path = format!("/{}", server.token);
        let connection = connect(&server, &path).await.expect("accepted");
        connection.send_datagram(mouse_move_datagram()).unwrap();
        assert!(matches!(next_input(&mut server).await, Some(InboundPacket::MouseMove { .. })));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn wrong_or_missing_token_is_refused() {
        let mut server = start_server().await;
        let wrong = format!("/{}", "0".repeat(64));
        for path in ["", "/", wrong.as_str()] {
            assert!(connect(&server, path).await.is_err(), "path {path:?} accepted");
        }
        assert_eq!(server.gate.failures_from(LOCAL), 3);
        assert!(next_input(&mut server).await.is_none());
        // The right token still works after a few failures.
        let path = format!("/{}", server.token);
        assert!(connect(&server, &path).await.is_ok());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn one_connection_at_a_time_and_reconnect_after_close() {
        let server = start_server().await;
        let path = format!("/{}", server.token);
        let first = connect(&server, &path).await.expect("first accepted");
        assert!(connect(&server, &path).await.is_err(), "second connection accepted");
        // Not counted as a failed handshake: it had the token.
        assert_eq!(server.gate.failures_from(LOCAL), 0);
        first.close(wtransport::VarInt::from_u32(0), b"bye");
        drop(first);
        // The server notices the close, then the slot is free again.
        let mut reconnected = None;
        for _ in 0..50 {
            tokio::time::sleep(Duration::from_millis(100)).await;
            if let Ok(c) = connect(&server, &path).await {
                reconnected = Some(c);
                break;
            }
        }
        assert!(reconnected.is_some(), "reconnect after close refused");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn too_many_failures_lock_out_that_address_only() {
        let server = start_server().await;
        // A stranger on another (loopback) address.
        let stranger: std::net::IpAddr = "127.0.0.2".parse().unwrap();
        for _ in 0..WEBTRANSPORT_MAX_FAILED_HANDSHAKES {
            assert!(connect_from(&server, stranger, "/nope").await.is_err());
        }
        assert!(server.gate.locked(std::net::SocketAddr::new(stranger, 1)));
        let path = format!("/{}", server.token);
        assert!(connect_from(&server, stranger, &path).await.is_err(), "accepted after the limit");
        assert_eq!(server.gate.failures_from(stranger), WEBTRANSPORT_MAX_FAILED_HANDSHAKES);
        // The session's own client is still served.
        assert!(!server.gate.locked(std::net::SocketAddr::new(LOCAL, 1)));
        assert!(connect(&server, &path).await.is_ok(), "the legitimate address was locked out too");
    }

    #[test]
    fn the_failure_map_stays_bounded() {
        let gate = WebTransportGate::new("t".into());
        for i in 0..(WEBTRANSPORT_MAX_TRACKED as u32 + 50) {
            gate.record_failure(std::net::SocketAddr::new(std::net::Ipv4Addr::from(0x0a00_0000 + i).into(), 9), "test");
        }
        assert_eq!(gate.failures.lock().unwrap().len(), WEBTRANSPORT_MAX_TRACKED);
    }

    #[test]
    fn keyframe_requests_after_drops_are_rate_limited() {
        let mut requester = KeyframeRequester::new(Arc::new(std::sync::Mutex::new(None)));
        let t0 = Instant::now();
        assert!(requester.due(t0));
        assert!(!requester.due(t0 + Duration::from_millis(100)));
        assert!(!requester.due(t0 + Duration::from_millis(499)));
        assert!(requester.due(t0 + Duration::from_millis(500)));
        assert!(!requester.due(t0 + Duration::from_millis(900)));
        assert!(requester.due(t0 + Duration::from_millis(1000)));
    }
}
