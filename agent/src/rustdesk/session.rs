//! One RustDesk client connection: the host side of RustDesk's session
//! protocol, ported from rustdesk/src/server/connection.rs (`Connection`),
//! reduced to showing the screen.

use std::net::{IpAddr, SocketAddr};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use bytes::{Bytes, BytesMut};
use lunaris_media::pipeline::{MediaEvent, MediaPipeline, PipelineCommand};
use lunaris_media::types::{FrameType, PixelFormat, StreamConfig, VideoCodec};
use prost::Message as _;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::tcp::OwnedWriteHalf;
use tokio::net::TcpStream;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tokio::time::Instant;
use tracing::{debug, error, info, warn};

use super::clipboard::{self, ClipboardLink};
use super::cursor::CursorSender;
use super::files::{self, FileHost};
use super::input::{HostOs, Injector, Input, RealInjector, RecordingInjector};
use super::qos::{self, VideoQoS};
use super::proto::back_notification::{self, BlockInputState, PrivacyModeState};
use super::proto::{BackNotification, ScreenshotResponse, VoiceCallResponse};
use super::proto::{
    login_request, login_response, message, misc, option_message::BoolOption, permission_info::Permission,
    video_frame, AudioFormat, AudioFrame, ImageQuality, DisplayInfo, EncodedVideoFrame, EncodedVideoFrames, Features, Hash,
    KeyEvent, LoginRequest, LoginResponse, Message, Misc, MouseEvent, OptionMessage, PeerInfo, PermissionInfo,
    PointerDeviceEvent, SupportedDecoding, SupportedEncoding, SupportedResolutions, SwitchDisplay, TestDelay,
    VideoFrame,
};
use super::{frame, login, Shared};
use crate::approval::{ApproveMode, Decision};
use crate::direct::{PromptSlot, UnauthSlot};

/// The RustDesk version whose protocol this follows, sent as
/// `PeerInfo.version` (rustdesk/src/server/connection.rs
/// `send_logon_response_and_keep_alive`: `version: VERSION`).
const PROTOCOL_VERSION: &str = "1.5.3";

/// The rate the capture and the encoder are opened at, and so the most
/// frames a second this host sends. How many are actually encoded follows
/// RustDesk's frame rate control (qos.rs), as a limit on the pipeline.
const CAPTURE_FPS: u32 = 60;
/// rustdesk/libs/scrap/src/common/hwcodec.rs `DEFAULT_FPS`: the frame rate
/// RustDesk opens a hardware encoder with, whatever rate frames then come
/// at. Its bitrate (`calc_bitrate`) is therefore a budget per 1/30 s frame.
const HWCODEC_DEFAULT_FPS: u32 = 30;
/// qos.rs keeps its state by connection and by display name; a session
/// here is one of each.
const QOS_ID: i32 = 1;
const QOS_DISPLAY: &str = "display";
/// The kernel's send buffer for a session's socket. RustDesk leaves it to
/// the system, whose megabytes hide a slow link from the sender for
/// seconds; kept small, a link that cannot take the stream shows at once in
/// the queue below and in the delay probes that drive the frame rate and
/// bitrate control.
const SEND_BUFFER: usize = 64 * 1024;

// rustdesk/src/server/connection.rs
/// `MAX_UNAUTHORIZED_MESSAGE`: the largest message before login.
const MAX_UNAUTHORIZED_MESSAGE: usize = 128 * 1024;
/// `LOGIN_GRACE`: a connection not logged in by then is closed.
const LOGIN_GRACE: Duration = Duration::from_secs(180);
/// `TEST_DELAY_TIMEOUT`: how often the delay is probed.
const TEST_DELAY_TIMEOUT: Duration = Duration::from_secs(1);
/// `SEC30`: a peer silent for this long is gone.
const SEC30: Duration = Duration::from_secs(30);
/// `SEND_TIMEOUT_VIDEO` (12 000 ms): a write stuck for this long ends the
/// connection.
const SEND_TIMEOUT_VIDEO: Duration = Duration::from_millis(12_000);

/// Encoded frames waiting for the socket. RustDesk's capture loop waits for
/// the connection to take each frame before it makes the next
/// (rustdesk/src/server/video_service.rs `VideoFrameController`); Lunaris's
/// pipeline runs freely, so frames a slow connection cannot take are dropped
/// here, and the picture resumes at a new key frame (as bridge.rs does).
const MAX_QUEUED_VIDEO: usize = 4;
/// Messages other than video waiting for the socket; a peer that lets this
/// many pile up is not reading.
const MAX_QUEUED_MESSAGES: usize = 256;
/// Blocks of a file that may wait for the socket (128 KiB each): the file
/// is read as fast as the client takes it, not into memory.
const MAX_QUEUED_FILE_BLOCKS: usize = 8;
/// rustdesk/src/server/connection.rs `handle_login_request`: the login
/// error of a host whose "Enable file transfer" permission is off.
pub(crate) const NO_FILE_PERMISSION: &str = "No permission of file transfer";
/// A key frame asked for and not delivered is asked for again after this.
const KEYFRAME_RETRY: Duration = Duration::from_secs(1);
/// Sound waiting for the socket (10 ms a frame): more than this is dropped
/// rather than played late (RustDesk drops audio older than a second,
/// connection.rs `Connection::start`).
const MAX_QUEUED_AUDIO: usize = 20;
/// rustdesk/src/server/input_service.rs `run_pos`: a client is not told
/// where the cursor is for this long after it moved it itself.
const PEER_INPUT_QUIET: Duration = Duration::from_millis(300);
/// rustdesk/src/server/connection.rs `send_close_reason_no_retry`: what a
/// client hears when the person at the host closes or declines it.
const CLOSED_MANUALLY: &str = "Closed manually by the peer";
/// Test switch, off unless set: input of RustDesk sessions is written to
/// this file instead of being injected, and their clipboard is the file
/// `<this>.clipboard` instead of the computer's.
pub(crate) const RECORD_INPUT_ENV: &str = "LUNARIS_RUSTDESK_RECORD_INPUT_TO";
/// How often the displays are looked at again (RustDesk: every second, in
/// its capture loop).
const DISPLAY_CHECK_INTERVAL: Duration = Duration::from_secs(3);
/// How long a login waits for the previous viewer's capture to stop (a
/// client that reconnects at once) before it is told the host is busy.
const VIEWER_WAIT: Duration = Duration::from_secs(2);

/// A display of this computer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Display {
    /// lunaris-media's id of the display.
    pub id: String,
    pub name: String,
    pub width: u32,
    pub height: u32,
    pub primary: bool,
}

/// The codecs this host sends (RustDesk `CodecFormat`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Codec {
    H264,
    H265,
}

/// rustdesk/libs/scrap/src/common/codec.rs `Encoder::update`, for one
/// connection and a host that encodes H.264 always and H.265 when
/// `host_h265`: which codecs are usable with this client (`h264_useable`,
/// `h265_useable`: the client decodes it and the host encodes it) and the
/// codec to send. The client's preference counts when it is usable; "auto"
/// is "h265 > h264". RustDesk's remaining choices (VP8, VP9, AV1) are not
/// encoded here, so a preference for one of them is treated as auto, and a
/// client that decodes neither H.264 nor H.265 gets no codec.
pub(crate) fn negotiate(decoding: Option<&SupportedDecoding>, host_h265: bool) -> (Option<Codec>, SupportedEncoding) {
    use super::proto::supported_decoding::PreferCodec;
    let h264_useable = decoding.is_some_and(|d| d.ability_h264 > 0);
    let h265_useable = decoding.is_some_and(|d| d.ability_h265 > 0) && host_h265;
    let usable = SupportedEncoding { h264: h264_useable, h265: h265_useable, ..Default::default() };
    let auto_codec = if h265_useable {
        Some(Codec::H265)
    } else if h264_useable {
        Some(Codec::H264)
    } else {
        None
    };
    let preference = decoding.and_then(|d| PreferCodec::try_from(d.prefer).ok()).unwrap_or(PreferCodec::Auto);
    let codec = match preference {
        PreferCodec::H264 if h264_useable => Some(Codec::H264),
        PreferCodec::H265 if h265_useable => Some(Codec::H265),
        _ => auto_codec,
    };
    (codec, usable)
}

const NO_CODEC: &str = "This host sends H.264 or H.265, and this client reports it can decode neither";

/// A running capture and encoder.
pub(crate) struct Video {
    pub events: mpsc::Receiver<MediaEvent>,
    pub commands: mpsc::Sender<PipelineCommand>,
    pub task: JoinHandle<()>,
}

/// A capture is not left running by a session that ends without
/// `stop_video` (its task aborted: new settings, the agent stops).
impl Drop for Video {
    fn drop(&mut self) {
        self.task.abort();
    }
}

/// What the session uses of this computer: the picture, the input
/// injector and the clipboard (lunaris-media and share.rs; replaced in
/// tests).
#[async_trait::async_trait]
pub(crate) trait Screen: Send + Sync {
    /// The displays there are; the first one marked primary is shown first.
    async fn displays(&self) -> anyhow::Result<Vec<Display>>;
    /// Whether an H.265 encoder for this display's size works here.
    async fn encodes_h265(&self, display: &Display) -> bool;
    /// Starts capture and encoder at CAPTURE_FPS with this bitrate.
    fn start(&self, display: &Display, codec: Codec, bitrate_kbps: u32) -> Video;
    /// The size the running capture found the display to have, where it is
    /// only known then (Wayland).
    fn captured_size(&self) -> Option<(u32, u32)> {
        None
    }
    /// Where mouse and keyboard go; `None` when nothing can inject here.
    fn injector(&self) -> Option<Box<dyn Injector>>;
    fn clipboard(&self) -> Option<ClipboardLink>;
}

fn record_path() -> Option<std::path::PathBuf> {
    std::env::var_os(RECORD_INPUT_ENV).filter(|p| !p.is_empty()).map(Into::into)
}

fn append_line(path: &std::path::Path, line: &str) {
    use std::io::Write;
    if let Ok(mut file) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
        // One write: the input thread and the clipboard thread both append.
        let _ = file.write_all(format!("{}\n", line).as_bytes());
    }
}

/// The stand-in clipboard of the test switch: what the client sets is
/// written to the record, and the "computer's clipboard" is the text of
/// `<record>.clipboard`, told to the client when it changes.
fn file_clipboard(record: std::path::PathBuf) -> Option<ClipboardLink> {
    let (set, set_rx) = std::sync::mpsc::channel::<String>();
    let (changed_tx, changed) = mpsc::unbounded_channel();
    let mut source = record.clone().into_os_string();
    source.push(".clipboard");
    let source = std::path::PathBuf::from(source);
    std::thread::Builder::new()
        .name("lunaris-rustdesk-test-clipboard".into())
        .spawn(move || {
            let read = || std::fs::read_to_string(&source).unwrap_or_default();
            let mut last = read();
            loop {
                match set_rx.recv_timeout(Duration::from_millis(200)) {
                    Ok(text) => append_line(&record, &format!("clipboard_set {:?}", text)),
                    Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                        let text = read();
                        if text != last && !text.is_empty() {
                            last = text.clone();
                            if changed_tx.send(text).is_err() {
                                return;
                            }
                        }
                    }
                    Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => return,
                }
            }
        })
        .ok()?;
    Some(ClipboardLink { set, changed })
}

/// The capture and hardware encoder of Lunaris's own sessions, started the
/// way bridge.rs `setup_bridge_session` starts them.
pub(crate) struct MediaScreen;

#[async_trait::async_trait]
impl Screen for MediaScreen {
    async fn displays(&self) -> anyhow::Result<Vec<Display>> {
        // A Wayland portal session shows the one screen the person here
        // chose for it (RustDesk: "wayland do not support changing display
        // for now"), and lunaris-media cannot list screens there. Its size
        // is what the last capture found, else what the desktop's layout
        // says of the first output; the capture corrects it when it starts
        // (`captured_size`).
        // With the Lunaris capture service (root KMS helper) every monitor
        // can be listed and shown, as on X11.
        #[cfg(target_os = "linux")]
        if wayland_session()
            && !tokio::task::spawn_blocking(lunaris_media::capture::drm_service::service_available)
                .await
                .unwrap_or(false)
        {
            let size = match lunaris_media::input::capture_region() {
                Some(region) => Some(region),
                None => tokio::task::spawn_blocking(|| lunaris_media::input::desktop_outputs().into_iter().next())
                    .await
                    .ok()
                    .flatten(),
            };
            let (width, height) = size
                .filter(|r| r.width > 0 && r.height > 0)
                .map(|r| (r.width as u32, r.height as u32))
                .unwrap_or((1920, 1080));
            return Ok(vec![Display { id: "default".into(), name: "Screen".into(), width, height, primary: true }]);
        }
        let displays = lunaris_media::capture::list_displays_for_capabilities()
            .await
            .map_err(|e| anyhow::anyhow!("{}", e))?;
        if displays.is_empty() {
            anyhow::bail!("No displays");
        }
        Ok(displays
            .into_iter()
            .map(|d| Display { id: d.id, name: d.name, width: d.width, height: d.height, primary: d.is_primary })
            .collect())
    }

    async fn encodes_h265(&self, display: &Display) -> bool {
        // As the agent decides it for its own clients (lib.rs
        // `probe_codec_support`): H.265 only with a hardware encoder that
        // really opens at this size.
        let (width, height) = (display.width, display.height);
        tokio::task::spawn_blocking(move || {
            let encoders = lunaris_media::encode::list_available_encoders();
            crate::bridge::codec_supported_for_realtime(
                &encoders,
                VideoCodec::H265,
                crate::bridge::hardware_encoder_opens_blocking(VideoCodec::H265, width, height, CAPTURE_FPS),
            )
        })
        .await
        .unwrap_or(false)
    }

    fn captured_size(&self) -> Option<(u32, u32)> {
        #[cfg(target_os = "linux")]
        if wayland_session() {
            return lunaris_media::input::capture_region()
                .filter(|r| r.width > 0 && r.height > 0)
                .map(|r| (r.width as u32, r.height as u32));
        }
        None
    }

    fn start(&self, display: &Display, codec: Codec, bitrate_kbps: u32) -> Video {
        let config = StreamConfig {
            width: display.width,
            height: display.height,
            fps: CAPTURE_FPS,
            codec: match codec {
                Codec::H264 => VideoCodec::H264,
                Codec::H265 => VideoCodec::H265,
            },
            bitrate_kbps,
            pixel_format: PixelFormat::NV12,
            preferred_encoder: None,
            yuv444: false,
            colorspace: lunaris_media::colorspace::from_client_config(
                lunaris_media::types::DEFAULT_ENCODER_CSC_MODE,
                0,
                false,
            ),
            microphone: false,
        };
        let (pipeline, events, commands) = MediaPipeline::new(config);
        let id = display.id.clone();
        let task = tokio::spawn(async move {
            if let Err(e) = pipeline.run(&id).await {
                error!("RustDesk session: media pipeline ended with an error: {:?}", e);
            }
        });
        Video { events, commands, task }
    }

    fn injector(&self) -> Option<Box<dyn Injector>> {
        if let Some(path) = record_path() {
            warn!("RustDesk session: {} is set: input is written to {:?}, NOT injected", RECORD_INPUT_ENV, path);
            return Some(Box::new(RecordingInjector(Box::new(move |line| append_line(&path, &line)))));
        }
        // As bridge.rs `setup_bridge_session` makes it for a session.
        let injector = RealInjector::new();
        if injector.is_none() {
            error!(
                "RustDesk session: no input backend available: remote mouse and keyboard are off \
                 (on Wayland this needs write access to /dev/uinput)"
            );
        }
        injector.map(|i| Box::new(i) as Box<dyn Injector>)
    }

    fn clipboard(&self) -> Option<ClipboardLink> {
        match record_path() {
            Some(path) => file_clipboard(path),
            None => ClipboardLink::system(),
        }
    }
}

fn wayland_session() -> bool {
    cfg!(target_os = "linux") && std::env::var("XDG_SESSION_TYPE").is_ok_and(|v| v == "wayland")
}

/// lunaris-media puts the pointer into the picture when told to
/// (`should_embed_host_cursor`); then the client must not draw it again.
fn cursor_embedded() -> bool {
    cfg!(target_os = "linux")
        && std::env::var("LUNARIS_HIDE_HOST_CURSOR").is_err()
        && std::env::var("LUNARIS_EMBED_HOST_CURSOR")
            .is_ok_and(|v| matches!(v.to_ascii_lowercase().as_str(), "1" | "true" | "yes" | "on"))
}

/// What the input thread is given.
enum InputJob {
    /// The size of the display now shown.
    Display(u32, u32),
    Mouse(MouseEvent),
    Pointer(PointerDeviceEvent),
    Key(KeyEvent),
}

/// Input is injected on a thread of its own, as bridge.rs does (typing text
/// can take a while and must not hold up the connection). The thread ends,
/// letting go of every key still held, when the sender is dropped.
///
/// One ordered path: the connection's task reads messages in the order they
/// arrived and hands each to this channel; this one thread injects them in
/// that order, each to its end (a pasted text included) before the next.
fn spawn_input(injector: Box<dyn Injector>, display: &Display) -> Option<std::sync::mpsc::Sender<(Instant, InputJob)>> {
    let (tx, rx) = std::sync::mpsc::channel::<(Instant, InputJob)>();
    let mut input = Input::new(injector, HostOs::current(), display.width, display.height);
    std::thread::Builder::new()
        .name("lunaris-rustdesk-input".into())
        .spawn(move || {
            // Time from a message leaving the socket to its injection being
            // done, in microseconds (kept for the session's summary).
            let mut latency: Vec<u32> = Vec::new();
            while let Ok((received, job)) = rx.recv() {
                match job {
                    InputJob::Display(width, height) => {
                        input.set_display(width, height);
                        continue;
                    }
                    InputJob::Mouse(evt) => input.mouse(&evt),
                    InputJob::Pointer(evt) => input.pointer(&evt),
                    InputJob::Key(evt) => input.key_event(evt),
                }
                if latency.len() < 200_000 {
                    latency.push(received.elapsed().as_micros().min(u32::MAX as u128) as u32);
                }
            }
            input.release_all();
            let stats = input.stats();
            latency.sort_unstable();
            let at = |q: f64| latency.get(((latency.len() as f64 * q) as usize).min(latency.len().saturating_sub(1))).copied().unwrap_or(0);
            info!(
                "RustDesk session input: {} mouse and {} key events, {} key events without a key here; \
                 socket to injector: median {} us, p99 {} us, max {} us",
                stats.mouse_events,
                stats.key_events,
                stats.dropped_keys,
                at(0.5),
                at(0.99),
                latency.last().copied().unwrap_or(0)
            );
        })
        .ok()?;
    Some(tx)
}

/// Asks the person at this computer to accept a connection; `None` when the
/// question was withdrawn (the receiver's sender dropped or used).
pub(crate) type Asker = Arc<
    dyn Fn(String, crate::approval::Kind, tokio::sync::oneshot::Receiver<()>) -> std::pin::Pin<Box<dyn std::future::Future<Output = Option<Decision>> + Send>>
        + Send
        + Sync,
>;

/// approval.rs `ask_unless`, as direct.rs asks for a screen session or a
/// file session.
pub(crate) fn system_asker() -> Asker {
    Arc::new(|requester, kind, withdrawn| {
        Box::pin(async move {
            crate::approval::ask_unless(&requester, kind, async {
                let _ = withdrawn.await;
            })
            .await
        })
    })
}

/// A question on this computer's screen about a client that is waiting.
struct PendingApproval {
    answer: JoinHandle<Option<Decision>>,
    /// Dropped with the session: the question is taken down.
    _withdraw: tokio::sync::oneshot::Sender<()>,
    _prompt: PromptSlot,
    lr: LoginRequest,
}

/// Aborts a task when dropped.
struct AbortOnDrop(tokio::task::AbortHandle);

impl Drop for AbortOnDrop {
    fn drop(&mut self) {
        self.0.abort();
    }
}

/// Marks the one viewer; cleared when the session ends.
struct Viewing(Arc<Shared>);

impl Viewing {
    fn take(ctx: &Arc<Shared>) -> Option<Self> {
        ctx.viewing
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_ok()
            .then(|| Viewing(ctx.clone()))
    }
}

impl Drop for Viewing {
    fn drop(&mut self) {
        self.0.viewing.store(false, Ordering::SeqCst);
    }
}

/// Stops a capture and waits until it is gone.
async fn stop_video(video: Video) {
    let mut video = video;
    // Nothing reads the events any more: the pipeline must not wait on them.
    video.events.close();
    let _ = video.commands.send(PipelineCommand::Stop).await;
    if tokio::time::timeout(Duration::from_secs(5), &mut video.task).await.is_err() {
        warn!("RustDesk session: the media pipeline did not stop in time");
    }
    // Dropping `video` aborts what is left of it.
}

/// The capture of an authorized session and what was sent of it.
struct ActiveVideo {
    video: Video,
    display: Display,
    codec: Codec,
    /// Whether the capture's own word on the display's size was heard
    /// (`Screen::captured_size`).
    size_checked: bool,
    /// What the pipeline was last told: frames a second it may encode
    /// (rustdesk/src/server/video_service.rs `spf`) and the quality ratio
    /// its bitrate was made from (`check_qos`: `ratio`).
    frame_limit: u32,
    ratio: f32,
    /// Whether the encoder takes a new bitrate while it runs
    /// (`support_changing_quality`); known once it has started.
    changes_bitrate: bool,
    /// Frames sent since `second` (`send_counter`).
    sent_this_second: usize,
    second: Instant,
    /// The clock of `EncodedVideoFrame.pts`.
    started: Instant,
    /// Frames are dropped until the next key frame: at the start, and after
    /// a frame was dropped (the ones after it refer to it).
    waiting_for_key: bool,
    key_requested: Option<Instant>,
    sent: u64,
    sent_keys: u64,
    dropped: u64,
    _viewing: Viewing,
}

impl ActiveVideo {
    fn request_keyframe(&mut self) {
        let _ = self.video.commands.try_send(PipelineCommand::RequestKeyframe);
        self.key_requested = Some(Instant::now());
    }
}

/// What waits for the socket.
enum Outgoing {
    Message(Bytes),
    Video(Bytes),
    Audio(Bytes),
}

/// How much video and sound waits for the socket.
#[derive(Default)]
struct Queued {
    video: AtomicUsize,
    audio: AtomicUsize,
    /// Told when the socket took a video frame.
    video_taken: tokio::sync::Notify,
}

/// One `Message` in its frame (hbb_common/src/tcp.rs `FramedStream::send`:
/// the protobuf bytes through `BytesCodec`; no encryption, see rustdesk.rs).
fn encode(msg: &Message) -> Bytes {
    let len = msg.encoded_len();
    let mut buf = BytesMut::with_capacity(len + 4);
    // A message of this host is far below the 1 GiB a header can describe.
    frame::put_header(len, &mut buf).expect("message fits a frame");
    msg.encode(&mut buf).expect("the buffer grows");
    buf.freeze()
}

/// Writes queued frames to the socket, each within SEND_TIMEOUT_VIDEO
/// (rustdesk/src/server/connection.rs: `conn.stream.set_send_timeout`), and
/// closes the sending side when the queue ends.
async fn write_loop(mut socket: OwnedWriteHalf, mut rx: mpsc::Receiver<Outgoing>, queued: Arc<Queued>) {
    while let Some(out) = rx.recv().await {
        let (bytes, counter) = match out {
            Outgoing::Message(bytes) => (bytes, None),
            Outgoing::Video(bytes) => (bytes, Some(&queued.video)),
            Outgoing::Audio(bytes) => (bytes, Some(&queued.audio)),
        };
        let written = tokio::time::timeout(SEND_TIMEOUT_VIDEO, socket.write_all(&bytes)).await;
        if let Some(counter) = counter {
            counter.fetch_sub(1, Ordering::Relaxed);
            if std::ptr::eq(counter, &queued.video) {
                queued.video_taken.notify_one();
            }
        }
        match written {
            Ok(Ok(())) => {}
            Ok(Err(e)) => {
                debug!("RustDesk connection: write failed: {}", e);
                return;
            }
            Err(_) => {
                debug!("RustDesk connection: write timed out");
                return;
            }
        }
    }
    let _ = socket.shutdown().await;
}

// hbb_common/src/lib.rs `is_ipv4_str`, `is_ipv6_str`, `is_domain_port_str`:
// the same expressions.
fn is_ip_str(id: &str) -> bool {
    static V4: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    static V6: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    let v4 = V4.get_or_init(|| {
        regex::Regex::new(
            r"^(?:25[0-5]|2[0-4][0-9]|[01]?[0-9][0-9]?)\.(?:25[0-5]|2[0-4][0-9]|[01]?[0-9][0-9]?)\.(?:25[0-5]|2[0-4][0-9]|[01]?[0-9][0-9]?)\.(?:25[0-5]|2[0-4][0-9]|[01]?[0-9][0-9]?)(:\d+)?$",
        )
        .expect("a valid expression")
    });
    let v6 = V6.get_or_init(|| {
        regex::Regex::new(
            r"^((([a-fA-F0-9]{1,4}:{1,2})+[a-fA-F0-9]{1,4})|(\[([a-fA-F0-9]{1,4}:{1,2})+[a-fA-F0-9]{1,4}\]:\d+))$",
        )
        .expect("a valid expression")
    });
    v4.is_match(id) || v6.is_match(id)
}

fn is_domain_port_str(id: &str) -> bool {
    static RE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    RE.get_or_init(|| {
        regex::Regex::new(r"(?i)^([a-z0-9]([a-z0-9-]{0,61}[a-z0-9])?\.)+[a-z][a-z-]{0,61}[a-z]:\d{1,5}$")
            .expect("a valid expression")
    })
    .is_match(id)
}

/// rustdesk/src/common.rs `is_direct_ip_access`: the peer the client asked
/// for is an address, not a RustDesk id.
fn is_direct_ip_access(peer: &str) -> bool {
    is_ip_str(peer) || is_domain_port_str(peer)
}

fn misc_message(union: misc::Union) -> Message {
    Message { union: Some(message::Union::Misc(Misc { union: Some(union) })) }
}

/// rustdesk/src/server/connection.rs `send_login_error`.
fn login_error(text: &str) -> Message {
    Message {
        union: Some(message::Union::LoginResponse(LoginResponse {
            union: Some(login_response::Union::Error(text.to_string())),
            ..Default::default()
        })),
    }
}

/// rustdesk/src/server/connection.rs `send_permission`.
fn permission(permission: Permission, enabled: bool) -> Message {
    misc_message(misc::Union::PermissionInfo(PermissionInfo { permission: permission as i32, enabled }))
}

/// The `VideoFrame` of one access unit
/// (rustdesk/libs/scrap/src/common/hwcodec.rs `encode_to_message`:
/// `h264s` or `h265s { frames: [{ data, pts, key }] }`;
/// rustdesk/src/server/video_service.rs `handle_one_frame` sets `display`).
/// `data` is the encoder's Annex-B output as it is, which is what the client
/// hands to its decoder (rustdesk/libs/scrap/src/common/codec.rs
/// `handle_hwram_video_frame`).
fn video_message(codec: Codec, display: usize, data: Vec<u8>, key: bool, pts_ms: i64) -> Message {
    // A constant-bitrate encoder pads its stream with filler data, which the
    // client's decoder throws away: it is not sent (the WebRTC path leaves
    // it out too, in its packager).
    let data = crate::video::annexb::without_filler(data, codec == Codec::H265);
    let frames = EncodedVideoFrames { frames: vec![EncodedVideoFrame { data: Bytes::from(data), key, pts: pts_ms }] };
    Message {
        union: Some(message::Union::VideoFrame(VideoFrame {
            display: display as i32,
            union: Some(match codec {
                Codec::H264 => video_frame::Union::H264s(frames),
                Codec::H265 => video_frame::Union::H265s(frames),
            }),
        })),
    }
}

/// rustdesk/src/server/display_service.rs `update_sync_displays`: the
/// displays as the client is told them. lunaris-media does not say where a
/// display lies on the desktop, so each is at 0,0: positions the client
/// sends are within the display it is shown.
fn display_infos(displays: &[Display]) -> Vec<DisplayInfo> {
    displays
        .iter()
        .map(|display| DisplayInfo {
            x: 0,
            y: 0,
            width: display.width as i32,
            height: display.height as i32,
            name: display.name.clone(),
            online: true,
            // The client draws the cursor from `CursorData`, unless
            // lunaris-media was told to put it into the picture.
            cursor_embedded: cursor_embedded(),
            original_resolution: None,
            scale: 1.0,
        })
        .collect()
}

/// rustdesk/src/server/display_service.rs `displays_to_msg`: the displays
/// again, when they changed.
fn displays_message(displays: &[Display]) -> Message {
    Message {
        union: Some(message::Union::PeerInfo(PeerInfo { displays: display_infos(displays), ..Default::default() })),
    }
}

/// rustdesk/src/server/video_service.rs `make_display_changed_msg`.
fn display_changed_message(index: usize, display: &Display) -> Message {
    misc_message(misc::Union::SwitchDisplay(SwitchDisplay {
        display: index as i32,
        x: 0,
        y: 0,
        width: display.width as i32,
        height: display.height as i32,
        cursor_embedded: cursor_embedded(),
        resolutions: Some(SupportedResolutions::default()),
        original_resolution: None,
    }))
}

fn platform() -> &'static str {
    // What RustDesk sends (`hbb_common::whoami::platform().to_string()`).
    match std::env::consts::OS {
        "linux" => "Linux",
        "windows" => "Windows",
        "macos" => "Mac OS",
        other => other,
    }
}

/// rustdesk/src/server/connection.rs `send_logon_response_and_keep_alive`:
/// the `PeerInfo` of a remote-desktop session.
fn peer_info(name: &str, displays: &[Display], current: usize, host_h265: bool) -> PeerInfo {
    let mut additions = serde_json::Map::new();
    // "is_wayland" as RustDesk reports it (`platform_additions`).
    if wayland_session() {
        additions.insert("is_wayland".into(), serde_json::json!(true));
    }
    PeerInfo {
        username: std::env::var("USER").or_else(|_| std::env::var("USERNAME")).unwrap_or_default(),
        hostname: name.to_string(),
        platform: platform().to_string(),
        displays: display_infos(displays),
        current_display: current as i32,
        sas_enabled: false,
        version: PROTOCOL_VERSION.to_string(),
        features: Some(Features::default()),
        // rustdesk/libs/scrap/src/common/codec.rs `Encoder::supported_encoding`:
        // what this host can encode.
        encoding: Some(SupportedEncoding { h264: true, h265: host_h265, ..Default::default() }),
        resolutions: Some(SupportedResolutions::default()),
        platform_additions: if additions.is_empty() {
            String::new()
        } else {
            serde_json::to_string(&additions).unwrap_or_default()
        },
        windows_sessions: None,
    }
}

struct Session {
    ctx: Arc<Shared>,
    ip: IpAddr,
    tx: mpsc::Sender<Outgoing>,
    queued: Arc<Queued>,
    hash: Hash,
    /// Counted as not logged in until dropped (at login or at the end).
    slot: Option<UnauthSlot>,
    authorized: bool,
    /// rustdesk/src/server/connection.rs `last_test_delay`, `network_delay`.
    last_test_delay: Option<Instant>,
    network_delay: u32,
    /// rustdesk/src/server/video_service.rs `VIDEO_QOS`.
    qos: VideoQoS,
    video: Option<ActiveVideo>,
    /// The displays the client was told, and the one it is shown.
    displays: Vec<Display>,
    current_display: usize,
    displays_checked: Instant,
    /// What the client decodes (`SupportedDecoding`, scrap `PEER_DECODINGS`),
    /// whether this host encodes H.265, and the usable codecs the client
    /// was last told (connection.rs `last_supported_encoding`).
    decoding: Option<SupportedDecoding>,
    host_h265: bool,
    last_supported_encoding: Option<SupportedEncoding>,
    /// The client's version and platform (`LoginRequest`), which decide the
    /// clipboard message it understands.
    peer_version: String,
    peer_platform: String,
    /// rustdesk/src/server/connection.rs `disable_keyboard`,
    /// `disable_clipboard`, `disable_audio`, `show_remote_cursor`: what the
    /// client switched off or on for itself (`OptionMessage`).
    disable_keyboard: bool,
    disable_clipboard: bool,
    disable_audio: bool,
    show_remote_cursor: bool,
    /// The input thread; `None` before login and when nothing can inject.
    input: Option<std::sync::mpsc::Sender<(Instant, InputJob)>>,
    last_peer_input: Option<Instant>,
    clipboard: Option<ClipboardLink>,
    cursor: CursorSender,
    /// The `AudioFormat` the client was told.
    audio_format: Option<(u32, u32)>,
    approval: Option<PendingApproval>,
    /// This computer's files, on a file-transfer connection (files.rs).
    files: Option<FileHost>,
    /// Why the connection ends, for the log.
    reason: String,
}

impl Session {
    /// Queues a message; false when the peer is not reading (the connection
    /// ends).
    fn send(&mut self, msg: Message) -> bool {
        if self.tx.try_send(Outgoing::Message(encode(&msg))).is_err() {
            self.reason = "the client does not read".into();
            return false;
        }
        true
    }

    /// rustdesk/src/server/connection.rs `send_close_reason_no_retry`: tells
    /// the client why it is being closed.
    fn close_with(&mut self, reason: &str) -> bool {
        self.send(misc_message(misc::Union::CloseReason(reason.to_string())));
        self.reason = reason.to_string();
        false
    }

    /// rustdesk/src/server/connection.rs `peer_keyboard_enabled`: this host
    /// allows control (it can inject) and the client did not switch it off.
    fn keyboard_enabled(&self) -> bool {
        self.input.is_some() && !self.disable_keyboard
    }

    /// `clipboard_enabled`: the agent's `allow_clipboard`, and the client's
    /// own switch.
    fn clipboard_enabled(&self) -> bool {
        self.clipboard.is_some() && !self.disable_clipboard
    }

    /// `peer_keyboard_enabled() || show_remote_cursor`: when a client is
    /// told the cursor's shape.
    fn cursor_wanted(&self) -> bool {
        !cursor_embedded() && (self.keyboard_enabled() || self.show_remote_cursor)
    }

    /// Tells the client the cursor's shape when it should know it and does
    /// not yet.
    fn send_cursor_shape(&mut self) -> bool {
        if !self.cursor_wanted() {
            return true;
        }
        match self.cursor.shape_message() {
            Some(msg) => self.send(msg),
            None => true,
        }
    }

    fn inject(&mut self, job: InputJob) {
        if !self.keyboard_enabled() {
            return;
        }
        let now = Instant::now();
        self.last_peer_input = Some(now);
        if let Some(input) = self.input.as_ref() {
            let _ = input.send((now, job));
        }
    }

    /// Puts the question on this computer's screen (RustDesk:
    /// `try_start_cm`), unless one is up for this connection, nobody is
    /// there to answer, or too many are up (direct.rs `PromptSlot`).
    fn start_approval(&mut self, lr: &LoginRequest) {
        if self.approval.is_some() || !(self.ctx.attended)() {
            return;
        }
        let Some(prompt) = PromptSlot::take(&self.ctx.limits, self.ip) else {
            debug!("RustDesk login from {}: not asked here, too many questions are up", self.ip);
            return;
        };
        let requester = crate::direct::requester_label(&lr.my_name, self.ip, false);
        // A file-transfer connection is asked about as one.
        let kind = match lr.union {
            Some(login_request::Union::FileTransfer(_)) => crate::approval::Kind::Files,
            _ => crate::approval::Kind::Control,
        };
        let (withdraw, withdrawn) = tokio::sync::oneshot::channel();
        self.approval = Some(PendingApproval {
            answer: tokio::spawn((self.ctx.asker)(requester, kind, withdrawn)),
            _withdraw: withdraw,
            _prompt: prompt,
            lr: lr.clone(),
        });
    }

    /// The person here answered (rustdesk/src/server/connection.rs
    /// `Connection::start`: `ipc::Data::Authorize` lets the client in,
    /// `ipc::Data::Close` sends it the manual close reason). Returns false
    /// when the connection ends.
    async fn on_approval(&mut self, decision: Option<Decision>) -> bool {
        let Some(pending) = self.approval.take() else { return true };
        match decision {
            Some(Decision::Accepted) => {
                info!("RustDesk client from {} accepted at this computer", self.ip);
                self.authorize(pending.lr).await
            }
            Some(Decision::Denied) | Some(Decision::Timeout) => {
                // As in direct.rs: a refusal counts like a wrong password,
                // so a stranger cannot keep putting questions up.
                if decision == Some(Decision::Denied) {
                    self.ctx.limits.lock().unwrap().logins.failed(self.ip);
                }
                self.close_with(CLOSED_MANUALLY)
            }
            // Nobody could be asked after all: the password remains.
            Some(Decision::Unattended) | None => true,
        }
    }

    /// The encoder's bitrate (kbps) for a quality ratio
    /// (rustdesk/libs/scrap/src/common/hwcodec.rs `HwRamEncoder::bitrate`:
    /// `calc_bitrate(width, height, ratio, h264)`). RustDesk gives that
    /// number to an encoder opened at 30 fps and then feeds it frames at
    /// the session's rate, so it is a budget per frame; this encoder is
    /// opened at CAPTURE_FPS and gets the same budget per frame.
    fn bitrates(display: &Display, codec: Codec, ratio: f32) -> (u32, u32) {
        let rustdesk = qos::calc_bitrate(display.width as usize, display.height as usize, ratio, codec == Codec::H264);
        (rustdesk, (rustdesk as u64 * CAPTURE_FPS as u64 / HWCODEC_DEFAULT_FPS as u64) as u32)
    }

    /// rustdesk/src/server/video_service.rs `check_qos`: what the frame
    /// rate and bitrate control decided is passed to the pipeline. The
    /// frame rate is a limit on how many captured frames are encoded
    /// (`spf`); nothing is restarted for it.
    async fn check_qos(&mut self) {
        let Some(active) = self.video.as_mut() else { return };
        let fps = self.qos.fps().min(CAPTURE_FPS);
        if fps != active.frame_limit {
            debug!("RustDesk session: frame rate {} -> {}", active.frame_limit, fps);
            active.frame_limit = fps;
            let _ = active.video.commands.try_send(PipelineCommand::SetFrameLimit(fps));
        }
        let ratio = self.qos.ratio();
        let mut restart = false;
        if ratio != active.ratio {
            let (rustdesk_kbps, encoder_kbps) = Self::bitrates(&active.display, active.codec, ratio);
            if active.changes_bitrate {
                info!(
                    "RustDesk session: quality ratio {:.2} -> {:.2}: {} kbps at 30 fps ({} kbps for the encoder)",
                    active.ratio, ratio, rustdesk_kbps, encoder_kbps
                );
                active.ratio = ratio;
                let _ = active.video.commands.try_send(PipelineCommand::SetBitrate(encoder_kbps));
                self.qos.store_bitrate(rustdesk_kbps);
            } else if !self.qos.in_vbr_state() && !self.qos.latest_quality().is_custom() {
                // "Now only vaapi doesn't support changing quality ...
                // switch to change quality": the encoder is made anew.
                active.ratio = ratio;
                restart = true;
            }
        }
        if active.second.elapsed() > Duration::from_secs(1) {
            active.second = Instant::now();
            self.qos.update_display_data(QOS_DISPLAY, active.sent_this_second);
            active.sent_this_second = 0;
        }
        if restart {
            self.restart_video().await;
        }
    }

    /// Starts capture and encoder again with the session's current display,
    /// codec and quality (rustdesk/src/server/video_service.rs: "SWITCH",
    /// the capturer and encoder are made anew).
    async fn restart_video(&mut self) {
        let Some(mut active) = self.video.take() else { return };
        let (rustdesk_kbps, encoder_kbps) = Self::bitrates(&active.display, active.codec, active.ratio);
        info!(
            "RustDesk session: restarting the picture: '{}' {}x{}, {:?}, quality ratio {:.2} ({} kbps at 30 fps)",
            active.display.name, active.display.width, active.display.height, active.codec, active.ratio, rustdesk_kbps
        );
        stop_video(active.video).await;
        active.video = self.ctx.screen.start(&active.display, active.codec, encoder_kbps);
        let _ = active.video.commands.try_send(PipelineCommand::SetFrameLimit(active.frame_limit));
        self.qos.store_bitrate(rustdesk_kbps);
        active.waiting_for_key = true;
        active.key_requested = None;
        self.video = Some(active);
    }

    /// connection.rs `update_supported_encoding`: the client is told which
    /// codecs are usable with it when that changed.
    fn send_supported_encoding(&mut self, usable: SupportedEncoding) -> bool {
        let changed = self
            .last_supported_encoding
            .as_ref()
            .is_some_and(|last| last.h264 != usable.h264 || last.h265 != usable.h265);
        if !changed {
            return true;
        }
        self.last_supported_encoding = Some(usable.clone());
        self.send(misc_message(misc::Union::SupportedEncoding(usable)))
    }

    /// The client said (again) what it decodes and prefers
    /// (`Encoder::update(EncodingUpdate::Update(..))`); the picture changes
    /// codec when the choice does. Returns false when the connection ends.
    async fn update_decoding(&mut self, decoding: &SupportedDecoding) -> bool {
        self.decoding = Some(decoding.clone());
        let (codec, usable) = negotiate(self.decoding.as_ref(), self.host_h265);
        let Some(codec) = codec else {
            return self.close_with(NO_CODEC);
        };
        if !self.send_supported_encoding(usable) {
            return false;
        }
        if let Some(active) = self.video.as_mut() {
            if active.codec != codec {
                info!("RustDesk session: codec {:?} -> {:?}", active.codec, codec);
                active.codec = codec;
                self.restart_video().await;
            }
        }
        true
    }

    /// The picture moves to another display (connection.rs
    /// `switch_display_to`); an index that does not exist is ignored.
    /// Returns whether it moved.
    async fn show_display(&mut self, index: usize) -> bool {
        if index == self.current_display || index >= self.displays.len() {
            return false;
        }
        self.current_display = index;
        let display = self.displays[index].clone();
        // Another size may have another answer.
        self.host_h265 = self.ctx.screen.encodes_h265(&display).await;
        let codec = negotiate(self.decoding.as_ref(), self.host_h265).0;
        if let Some(input) = self.input.as_ref() {
            let _ = input.send((Instant::now(), InputJob::Display(display.width, display.height)));
        }
        if let Some(active) = self.video.as_mut() {
            active.display = display;
            active.codec = codec.unwrap_or(active.codec);
            active.size_checked = false;
            self.restart_video().await;
        }
        true
    }

    /// rustdesk/src/server/video_service.rs `try_broadcast_display_changed`
    /// / display_service.rs `check_display_changed`: the displays are
    /// looked at again every few seconds; a changed list is told to the
    /// client, and a display shown whose size changed is shown anew.
    async fn check_displays(&mut self) -> bool {
        if self.video.is_none() || self.displays_checked.elapsed() < DISPLAY_CHECK_INTERVAL {
            return true;
        }
        self.displays_checked = Instant::now();
        let Ok(mut displays) = self.ctx.screen.displays().await else { return true };
        // Where only the capture knows the size (Wayland), its word stands.
        if let (Some(active), Some((width, height)), [only]) =
            (self.video.as_ref().filter(|a| a.size_checked), self.ctx.screen.captured_size(), displays.as_mut_slice())
        {
            let _ = active;
            only.width = width;
            only.height = height;
        }
        if displays == self.displays {
            return true;
        }
        self.apply_displays(displays).await
    }

    /// The client is told the displays as they are now, and the picture
    /// follows when the display shown changed.
    async fn apply_displays(&mut self, displays: Vec<Display>) -> bool {
        info!("RustDesk session: the displays changed ({} now)", displays.len());
        self.displays = displays;
        if !self.send(displays_message(&self.displays)) {
            return false;
        }
        if self.current_display >= self.displays.len() {
            self.current_display = self.displays.iter().position(|d| d.primary).unwrap_or(0);
        }
        let display = self.displays[self.current_display].clone();
        let changed = self.video.as_ref().is_some_and(|active| active.display != display);
        if changed {
            if !self.send(display_changed_message(self.current_display, &display)) {
                return false;
            }
            if let Some(input) = self.input.as_ref() {
                let _ = input.send((Instant::now(), InputJob::Display(display.width, display.height)));
            }
            if let Some(active) = self.video.as_mut() {
                active.display = display;
            }
            self.restart_video().await;
        }
        true
    }

    /// rustdesk/src/server/connection.rs `update_options`: the frame rate,
    /// the codecs the client decodes, and what it switches off or on for
    /// itself. Image quality is not followed (the bitrate is fixed); the
    /// remaining options concern what is not offered.
    async fn update_options(&mut self, o: &OptionMessage) -> bool {
        debug!("RustDesk session: option update: {:?}", o);
        // The client's image quality: Best, Balanced or Low, or with
        // NotSet a custom percentage (`user_image_quality`).
        let image_quality = match ImageQuality::try_from(o.image_quality) {
            Ok(ImageQuality::NotSet) => {
                if o.custom_image_quality > 0 {
                    o.custom_image_quality
                } else {
                    -1
                }
            }
            Ok(quality) => quality as i32,
            Err(_) => -1,
        };
        if image_quality > 0 {
            self.qos.user_image_quality(QOS_ID, image_quality);
        }
        if o.custom_fps > 0 {
            self.qos.user_custom_fps(QOS_ID, o.custom_fps as u32);
        }
        if let Some(decoding) = o.supported_decoding.as_ref() {
            if !self.update_decoding(decoding).await {
                return false;
            }
        }
        let switch = |value: i32| match BoolOption::try_from(value) {
            Ok(BoolOption::Yes) => Some(true),
            Ok(BoolOption::No) => Some(false),
            _ => None,
        };
        // `block_input` without the permission: "No permission"
        // (`send_block_input_error`).
        if let Some(on) = switch(o.block_input) {
            let state = if on { BlockInputState::BlkOnFailed } else { BlockInputState::BlkOffFailed };
            if !self.send(misc_message(misc::Union::BackNotification(BackNotification {
                details: "No permission".into(),
                impl_key: String::new(),
                union: Some(back_notification::Union::BlockInputState(state as i32)),
            }))) {
                return false;
            }
        }
        let cursor_was_wanted = self.cursor_wanted();
        if let Some(on) = switch(o.show_remote_cursor) {
            self.show_remote_cursor = on;
        }
        if let Some(off) = switch(o.disable_audio) {
            self.disable_audio = off;
            if off {
                // The format is told again when sound comes back.
                self.audio_format = None;
            }
        }
        if let Some(off) = switch(o.disable_clipboard) {
            self.disable_clipboard = off;
        }
        if let Some(off) = switch(o.disable_keyboard) {
            // The client's "view only".
            self.disable_keyboard = off;
        }
        // The cursor service is (un)subscribed with these options; a client
        // that starts to want the shape gets the current one.
        if self.cursor_wanted() && !cursor_was_wanted {
            self.cursor.forget_current();
        }
        if !self.send_cursor_shape() {
            return false;
        }
        self.check_qos().await;
        true
    }

    /// rustdesk/src/server/connection.rs `on_message`, the `LoginRequest`
    /// branch. Returns false when the connection ends.
    async fn on_login_request(&mut self, lr: LoginRequest) -> bool {
        // Remote desktop is offered, and file transfer when the agent's
        // `rustdesk_file_transfer` is on (`Self::permission(
        // keys::OPTION_ENABLE_FILE_TRANSFER, ..)`); the refusals are
        // RustDesk's own for a host that does not permit the other kinds.
        let refused = match lr.union {
            Some(login_request::Union::FileTransfer(_)) if self.ctx.config.file_transfer => None,
            Some(login_request::Union::FileTransfer(_)) => Some(NO_FILE_PERMISSION),
            Some(login_request::Union::ViewCamera(_)) => Some("No permission of viewing camera"),
            Some(login_request::Union::Terminal(_)) => Some("No permission of terminal"),
            Some(login_request::Union::PortForward(_)) => Some("No permission of IP tunneling"),
            None => None,
        };
        if let Some(text) = refused {
            self.send(login_error(text));
            tokio::time::sleep(Duration::from_secs(1)).await;
            self.reason = text.into();
            return false;
        }
        // This host has no RustDesk id: only clients that asked for an
        // address belong here.
        if !is_direct_ip_access(&lr.username) {
            self.send(login_error(login::LOGIN_MSG_OFFLINE));
            self.reason = "asked for a RustDesk id".into();
            return false;
        }
        let ctx = self.ctx.clone();
        let config = &ctx.config;
        // `approve_mode` "click" (RustDesk `ApproveMode::Click`): the
        // password is not used; the person here is asked (`try_start_cm`)
        // and the client told to wait for that (clients from 1.2.0 on, the
        // only ones considered here, show "Please wait for the remote side
        // to accept your session request...").
        if config.approve_mode == ApproveMode::Click {
            self.start_approval(&lr);
            return self.send(login_error(login::LOGIN_MSG_NO_PASSWORD_ACCESS));
        }
        // No password yet: the client is asking its user for it and logs in
        // again on this connection (rustdesk/src/client.rs `handle_hash`).
        // Meanwhile, with "password-click", the person here may accept it
        // (`try_start_cm(lr.my_id, lr.my_name, false)`).
        if lr.password.is_empty() {
            if config.approve_mode == ApproveMode::PasswordClick {
                self.start_approval(&lr);
            }
            return true;
        }
        // rustdesk/src/server/connection.rs `check_failure`: too many wrong
        // passwords from this address (counted by direct.rs `LoginGuard`).
        if ctx.limits.lock().unwrap().logins.locked_out(self.ip) {
            warn!("RustDesk login from {} refused: too many wrong passwords", self.ip);
            return self.send(login_error(login::LOGIN_MSG_TRY_LATER));
        }
        if !login::verify(config.password.trim(), &self.hash.salt, &self.hash.challenge, &lr.password) {
            ctx.limits.lock().unwrap().logins.failed(self.ip);
            warn!("RustDesk login from {} refused: wrong password", self.ip);
            // RustDesk also `try_start_cm`s here: the person at the host may
            // still accept.
            if config.approve_mode == ApproveMode::PasswordClick {
                self.start_approval(&lr);
            }
            return self.send(login_error(login::LOGIN_MSG_PASSWORD_WRONG));
        }
        ctx.limits.lock().unwrap().logins.succeeded(self.ip);
        self.authorize(lr).await
    }

    /// rustdesk/src/server/connection.rs `send_logon_response_and_keep_alive`:
    /// the connection is let in, hears what it is connected to and gets the
    /// picture.
    async fn authorize(&mut self, lr: LoginRequest) -> bool {
        if let Some(login_request::Union::FileTransfer(ft)) = &lr.union {
            let (dir, show_hidden) = (ft.dir.clone(), ft.show_hidden);
            return self.authorize_files(lr, &dir, show_hidden).await;
        }
        let waiting = Instant::now();
        let viewing = loop {
            if let Some(viewing) = Viewing::take(&self.ctx) {
                break viewing;
            }
            if waiting.elapsed() >= VIEWER_WAIT {
                self.send(login_error("Another RustDesk client is already viewing this computer"));
                self.reason = "another viewer is connected".into();
                return false;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        };
        // rustdesk/src/server/display_service.rs
        // `update_get_sync_displays_on_login`; its error goes to the client.
        let displays = match self.ctx.screen.displays().await {
            Ok(displays) if !displays.is_empty() => displays,
            Ok(_) => {
                self.send(login_error("No displays"));
                self.reason = "no display".into();
                return false;
            }
            Err(e) => {
                self.send(login_error(&e.to_string()));
                self.reason = format!("no display: {}", e);
                return false;
            }
        };
        let current = displays.iter().position(|d| d.primary).unwrap_or(0);
        let shown = displays[current].clone();
        // `update_codec_on_login`: the codec for what this client decodes.
        let host_h265 = self.ctx.screen.encodes_h265(&shown).await;
        let decoding = lr.option.as_ref().and_then(|o| o.supported_decoding.clone());
        let (codec, usable) = negotiate(decoding.as_ref(), host_h265);
        let Some(codec) = codec else {
            self.send(login_error(NO_CODEC));
            self.reason = "no codec in common".into();
            return false;
        };
        self.displays = displays;
        self.current_display = current;
        self.displays_checked = Instant::now();
        self.decoding = decoding;
        self.host_h265 = host_h265;
        self.authorized = true;
        self.slot = None;
        // Let in: a question still on screen is taken down.
        self.approval = None;
        self.peer_version = lr.version.clone();
        self.peer_platform = lr.my_platform.clone();
        // Control and the clipboard, as far as this computer allows them.
        // A RustDesk host tells a connection what it may not do
        // (`send_permission`); what could not be known before login is told
        // now.
        self.input = self.ctx.screen.injector().and_then(|injector| spawn_input(injector, &shown));
        if self.input.is_none() && !self.send(permission(Permission::Keyboard, false)) {
            return false;
        }
        if self.ctx.config.allow_clipboard {
            self.clipboard = self.ctx.screen.clipboard();
        }
        info!(
            "RustDesk client logged in from {}: id={:?} name={:?} platform={:?} version={:?}; control {}, clipboard {}",
            self.ip,
            lr.my_id,
            lr.my_name,
            lr.my_platform,
            lr.version,
            if self.input.is_some() { "on" } else { "off" },
            if self.clipboard.is_some() { "on" } else { "off" },
        );
        let response = Message {
            union: Some(message::Union::LoginResponse(LoginResponse {
                union: Some(login_response::Union::PeerInfo(peer_info(
                    &self.ctx.config.name,
                    &self.displays,
                    current,
                    host_h265,
                ))),
                ..Default::default()
            })),
        };
        if !self.send(response) {
            return false;
        }
        // `last_supported_encoding` starts as what the host encodes; the
        // codecs usable with this client follow when they are fewer.
        self.last_supported_encoding = Some(SupportedEncoding { h264: true, h265: host_h265, ..Default::default() });
        if !self.send_supported_encoding(usable) {
            return false;
        }
        // `AuthedConnID::new`: the connection joins the frame rate and
        // bitrate control.
        self.qos.on_connection_open(QOS_ID);
        self.qos.new_display(QOS_DISPLAY.to_string());
        // `update_scoped_login_options`: the options sent with the login.
        if let Some(option) = lr.option.as_ref() {
            if !self.update_options(option).await {
                return false;
            }
        }
        // `try_sub_monitor_services`: the picture starts, at the quality the
        // client chose and the frame rate a new connection starts with
        // (qos.rs `INIT_FPS`, raised by the first delay replies).
        let ratio = self.qos.ratio();
        // The login's options were applied above; the codec they chose
        // stands.
        let codec = negotiate(self.decoding.as_ref(), self.host_h265).0.unwrap_or(codec);
        let (rustdesk_kbps, encoder_kbps) = Self::bitrates(&shown, codec, ratio);
        self.qos.store_bitrate(rustdesk_kbps);
        let frame_limit = self.qos.fps().min(CAPTURE_FPS);
        info!(
            "RustDesk session: showing '{}' {}x{} ({} of {} displays), {:?}, quality ratio {:.2}: {} kbps at 30 fps ({} kbps for the encoder at {} fps)",
            shown.name,
            shown.width,
            shown.height,
            current + 1,
            self.displays.len(),
            codec,
            ratio,
            rustdesk_kbps,
            encoder_kbps,
            CAPTURE_FPS
        );
        let video = self.ctx.screen.start(&shown, codec, encoder_kbps);
        let _ = video.commands.try_send(PipelineCommand::SetFrameLimit(frame_limit));
        self.video = Some(ActiveVideo {
            video,
            display: shown,
            codec,
            size_checked: false,
            frame_limit,
            ratio,
            changes_bitrate: true,
            sent_this_second: 0,
            second: Instant::now(),
            started: Instant::now(),
            waiting_for_key: true,
            key_requested: None,
            sent: 0,
            sent_keys: 0,
            dropped: 0,
            _viewing: viewing,
        });
        true
    }

    /// rustdesk/src/server/connection.rs `send_logon_response_and_keep_alive`
    /// for a file-transfer connection (`self.file_transfer.is_some()`): the
    /// client hears what it is connected to (`res.set_peer_info(pi)`: no
    /// displays and no codecs), then the listing of the folder it starts in
    /// (`self.read_dir(dir, show_hidden)`). No picture, no input and no
    /// clipboard on this connection, and it does not take the viewer's
    /// place.
    async fn authorize_files(&mut self, lr: LoginRequest, dir: &str, show_hidden: bool) -> bool {
        let config = &self.ctx.config;
        let host = match FileHost::new(config.file_access.clone(), config.file_home.clone(), &lr.version) {
            Ok(host) => host,
            Err(reason) => {
                warn!("RustDesk file transfer from {} refused: {}", self.ip, reason);
                self.send(login_error(&reason));
                self.reason = "no folder for file transfer".into();
                return false;
            }
        };
        self.authorized = true;
        self.slot = None;
        // Let in: a question still on screen is taken down.
        self.approval = None;
        self.peer_version = lr.version.clone();
        self.peer_platform = lr.my_platform.clone();
        info!(
            "RustDesk client logged in from {} for file transfer: id={:?} name={:?} platform={:?} version={:?}",
            self.ip, lr.my_id, lr.my_name, lr.my_platform, lr.version
        );
        let info = PeerInfo { encoding: None, resolutions: None, ..peer_info(&config.name, &[], 0, false) };
        let response = Message {
            union: Some(message::Union::LoginResponse(LoginResponse {
                union: Some(login_response::Union::PeerInfo(info)),
                ..Default::default()
            })),
        };
        if !self.send(response) {
            return false;
        }
        let listing = host.first_listing(dir, show_hidden).await;
        self.files = Some(host);
        self.send(listing)
    }

    /// rustdesk/src/server/connection.rs `Connection::start`, the
    /// `file_timer` branch (libs/base/src/fs.rs `handle_read_jobs`): the
    /// next block of a file the client downloads. RustDesk's timer ticks
    /// every millisecond while there is a job and waits for the socket with
    /// each block; here a block goes as soon as the socket took the ones
    /// before, and `idle` (wait a tick) is set when nothing could be sent.
    /// Returns false when the connection ends.
    fn on_file_timer(&mut self, idle: &mut bool) -> bool {
        *idle = true;
        // Not more than a few blocks wait for the socket.
        if self.tx.capacity() + MAX_QUEUED_FILE_BLOCKS < MAX_QUEUED_MESSAGES {
            return true;
        }
        let Some(files) = self.files.as_mut() else { return true };
        match files.next_read() {
            files::Step::Send(msg) => {
                *idle = false;
                self.send(msg)
            }
            files::Step::Idle => true,
        }
    }

    /// rustdesk/src/server/connection.rs `on_message`. Returns false when
    /// the connection ends.
    async fn on_message(&mut self, msg: Message) -> bool {
        // A close reason is heeded before login too.
        if let Some(message::Union::Misc(Misc { union: Some(misc::Union::CloseReason(reason)) })) = &msg.union {
            debug!("RustDesk client closes: {}", reason);
            self.reason = "closed by the client".into();
            return false;
        }
        match msg.union {
            Some(message::Union::LoginRequest(lr)) => {
                if self.authorized {
                    return true;
                }
                self.on_login_request(lr).await
            }
            Some(message::Union::TestDelay(t)) => {
                if t.from_client {
                    // The client's own probe: sent back as it came.
                    self.send(Message { union: Some(message::Union::TestDelay(t)) })
                } else {
                    // The answer to this host's probe.
                    if let Some(sent) = self.last_test_delay.take() {
                        self.network_delay = sent.elapsed().as_millis() as u32;
                        if self.authorized {
                            // `VIDEO_QOS.user_network_delay`.
                            self.qos.user_network_delay(QOS_ID, self.network_delay);
                            self.check_qos().await;
                        }
                        debug!(
                            "RustDesk session: delay {} ms, {} fps, quality ratio {:.2}, {} kbps at 30 fps",
                            self.network_delay,
                            self.qos.fps(),
                            self.qos.ratio(),
                            self.qos.bitrate()
                        );
                    }
                    true
                }
            }
            // Nothing else is heard before login.
            _ if !self.authorized => true,
            Some(message::Union::Misc(misc)) => match misc.union {
                Some(misc::Union::Option(o)) => self.update_options(&o).await,
                // `refresh_video_display`: the client wants a whole picture.
                Some(misc::Union::RefreshVideo(true)) | Some(misc::Union::RefreshVideoDisplay(_)) => {
                    debug!("RustDesk session: refresh asked");
                    if let Some(active) = self.video.as_mut() {
                        active.request_keyframe();
                    }
                    true
                }
                // `handle_switch_display`: another display, then the
                // display-changed message.
                Some(misc::Union::SwitchDisplay(s)) => {
                    if s.display >= 0 && self.show_display(s.display as usize).await {
                        let display = self.displays[self.current_display].clone();
                        return self.send(display_changed_message(self.current_display, &display));
                    }
                    true
                }
                // `capture_displays`: the displays a client of 1.2.4 or
                // later wants pictures of. RustDesk captures each of them;
                // this host shows one at a time: the first valid one asked
                // for (`add`, else `set`).
                Some(misc::Union::CaptureDisplays(wanted)) => {
                    let pick = |list: &[i32]| {
                        list.iter().copied().find(|d| *d >= 0 && (*d as usize) < self.displays.len()).map(|d| d as usize)
                    };
                    if let Some(index) = pick(&wanted.add).or_else(|| pick(&wanted.set)) {
                        self.show_display(index).await;
                    }
                    true
                }
                // rustdesk/src/server/video_qos.rs `user_auto_adjust_fps`.
                Some(misc::Union::AutoAdjustFps(fps)) => {
                    self.qos.user_auto_adjust_fps(QOS_ID, fps);
                    self.check_qos().await;
                    true
                }
                // `toggle_privacy_mode`: not offered. Turning it on is
                // denied (`turn_on_privacy` without the permission), turning
                // it off answered as where it is not supported.
                Some(misc::Union::TogglePrivacyMode(t)) => {
                    let (state, details) = if t.on {
                        (PrivacyModeState::PrvOnFailedDenied, String::new())
                    } else {
                        (
                            PrivacyModeState::PrvNotSupported,
                            "Unsupported. 1 Multi-screen is not supported. 2 Please confirm the license is activated.".to_string(),
                        )
                    };
                    self.send(misc_message(misc::Union::BackNotification(BackNotification {
                        details,
                        impl_key: t.impl_key,
                        union: Some(back_notification::Union::PrivacyModeState(state as i32)),
                    })))
                }
                // Restart is not permitted (`if self.restart`), a chat has
                // nobody to show it to, and the rest concerns what is not
                // offered. Only the kind is logged, never the content.
                other => {
                    debug!(
                        "RustDesk session: ignored misc message: {}",
                        format!("{:?}", other).split(['(', ' ', '{']).nth(1).unwrap_or("?")
                    );
                    true
                }
            },
            // rustdesk/src/server/connection.rs `handle_voice_call(false)`:
            // a voice call is declined, as when the person at a RustDesk
            // host declines it.
            Some(message::Union::VoiceCallRequest(request)) => {
                if !request.is_connect {
                    return true;
                }
                self.send(Message {
                    union: Some(message::Union::VoiceCallResponse(VoiceCallResponse {
                        accepted: false,
                        req_timestamp: request.req_timestamp,
                        ack_timestamp: std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .map(|d| d.as_millis() as i64)
                            .unwrap_or(0),
                    })),
                })
            }
            // A screenshot is answered with an error text, so the client
            // does not wait for one (`ScreenshotResponse.msg`: "empty if
            // success").
            Some(message::Union::ScreenshotRequest(request)) => self.send(Message {
                union: Some(message::Union::ScreenshotResponse(ScreenshotResponse {
                    sid: request.sid,
                    msg: "Screenshots are not available on this host".into(),
                    data: Bytes::new(),
                })),
            }),
            // Mouse, touch and keys go to the input thread while control is
            // on (`peer_keyboard_enabled`), and nowhere otherwise.
            Some(message::Union::MouseEvent(evt)) => {
                self.inject(InputJob::Mouse(evt));
                true
            }
            Some(message::Union::PointerDeviceEvent(evt)) => {
                self.inject(InputJob::Pointer(evt));
                true
            }
            Some(message::Union::KeyEvent(evt)) => {
                self.inject(InputJob::Key(evt));
                true
            }
            // The client's clipboard: its text is put on this computer's
            // (`update_clipboard`), when both sides allow it.
            Some(message::Union::Clipboard(cb)) => {
                self.set_clipboard(&[cb]);
                true
            }
            Some(message::Union::MultiClipboards(mcb)) => {
                self.set_clipboard(&mcb.clipboards);
                true
            }
            // `let mut handle_fa = self.file_transfer.is_some()`: file
            // actions are served on a file-transfer connection (files.rs),
            // and ignored on any other.
            Some(message::Union::FileAction(action)) => {
                let Some(files) = self.files.as_mut() else {
                    debug!("RustDesk session: file action ignored (not a file-transfer connection)");
                    return true;
                };
                let replies = files.on_action(action).await;
                replies.into_iter().all(|reply| self.send(reply))
            }
            // The data of an upload (`ipc::FS::WriteBlock`, ...).
            Some(message::Union::FileResponse(response)) => {
                let Some(files) = self.files.as_mut() else { return true };
                let replies = files.on_response(response);
                replies.into_iter().all(|reply| self.send(reply))
            }
            // Copying files through the clipboard is not offered.
            Some(message::Union::Cliprdr(_)) => {
                debug!("RustDesk session: file clipboard message ignored");
                true
            }
            other => {
                debug!("RustDesk session: ignored message: {}", describe(&other));
                true
            }
        }
    }

    /// One event of the pipeline. Returns false when the connection ends.
    fn on_media(&mut self, event: Option<MediaEvent>) -> bool {
        let Some(event) = event else {
            return self.close_with("The screen capture stopped");
        };
        let Some(active) = self.video.as_mut() else { return true };
        match event {
            MediaEvent::VideoFrame(frame) => {
                let key = frame.frame_type == FrameType::Key;
                if active.waiting_for_key && !key {
                    active.dropped += 1;
                    if active.key_requested.is_none_or(|at| at.elapsed() >= KEYFRAME_RETRY) {
                        active.request_keyframe();
                    }
                    return true;
                }
                if self.queued.video.load(Ordering::Relaxed) >= MAX_QUEUED_VIDEO {
                    // The socket is behind: this frame is not sent, so the
                    // ones after it cannot be either.
                    active.dropped += 1;
                    active.waiting_for_key = true;
                    active.request_keyframe();
                    return true;
                }
                // rustdesk/src/server/video_service.rs `run`: `ms`, the time
                // since the capture started.
                let pts = active.started.elapsed().as_millis() as i64;
                let bytes = encode(&video_message(active.codec, self.current_display, frame.data, key, pts));
                self.queued.video.fetch_add(1, Ordering::Relaxed);
                if self.tx.try_send(Outgoing::Video(bytes)).is_err() {
                    self.queued.video.fetch_sub(1, Ordering::Relaxed);
                    self.reason = "the client does not read".into();
                    return false;
                }
                active.waiting_for_key = false;
                active.key_requested = None;
                active.sent_this_second += 1;
                active.sent += 1;
                active.sent_keys += key as u64;
                true
            }
            MediaEvent::EncoderStarted { encoder, gpu_name, .. } => {
                // `set_support_changing_quality`: lunaris-media changes the
                // bitrate of a running VAAPI encoder (it reopens it). The
                // other encoders are treated as RustDesk treats one that
                // cannot: their bitrate changes with a restart only.
                active.changes_bitrate = encoder.hw_type == lunaris_media::types::HwAccelType::Vaapi;
                self.qos.set_support_changing_quality(QOS_DISPLAY, active.changes_bitrate);
                info!(
                    "RustDesk session: encoder {} ({}) on {}",
                    encoder.name,
                    encoder.hw_type,
                    gpu_name.as_deref().unwrap_or("unknown GPU")
                );
                true
            }
            MediaEvent::Error(e) => {
                warn!("RustDesk session: media pipeline: {}", e);
                true
            }
            MediaEvent::Started => {
                active.size_checked = true;
                // Looked at on the next turn of the display check.
                self.displays_checked = Instant::now() - DISPLAY_CHECK_INTERVAL;
                true
            }
            // rustdesk/src/server/audio_service.rs: the format once
            // (`create_format_msg`), then each Opus packet as an
            // `AudioFrame` (`send_f32`). RustDesk encodes 10 ms of 48 kHz
            // stereo with Opus in low-delay mode, which is what
            // lunaris-media's audio capture produces.
            MediaEvent::AudioFrame(frame) => {
                if self.disable_audio {
                    return true;
                }
                let format = (frame.sample_rate, frame.channels as u32);
                if self.audio_format != Some(format) {
                    self.audio_format = Some(format);
                    let msg = misc_message(misc::Union::AudioFormat(AudioFormat { sample_rate: format.0, channels: format.1 }));
                    if !self.send(msg) {
                        return false;
                    }
                }
                if self.queued.audio.load(Ordering::Relaxed) >= MAX_QUEUED_AUDIO {
                    return true;
                }
                let msg = Message { union: Some(message::Union::AudioFrame(AudioFrame { data: Bytes::from(frame.data) })) };
                self.queued.audio.fetch_add(1, Ordering::Relaxed);
                if self.tx.try_send(Outgoing::Audio(encode(&msg))).is_err() {
                    self.queued.audio.fetch_sub(1, Ordering::Relaxed);
                    self.reason = "the client does not read".into();
                    return false;
                }
                true
            }
            // rustdesk/src/server/input_service.rs `run_cursor`, `run_pos`.
            MediaEvent::CursorUpdate(state) => {
                if let Some(image) = state.image.as_ref() {
                    self.cursor.set_shape(image);
                }
                if !self.send_cursor_shape() {
                    return false;
                }
                let quiet = self.last_peer_input.is_some_and(|at| at.elapsed() < PEER_INPUT_QUIET);
                if self.show_remote_cursor && !cursor_embedded() && !quiet {
                    if let Some(msg) = self.cursor.position_message(state.x, state.y) {
                        return self.send(msg);
                    }
                }
                true
            }
            _ => true,
        }
    }

    /// rustdesk/src/clipboard.rs `update_clipboard`, for text.
    fn set_clipboard(&mut self, clipboards: &[super::proto::Clipboard]) {
        if !self.clipboard_enabled() {
            return;
        }
        let Some(text) = clipboard::text_from(clipboards) else {
            debug!("RustDesk session: clipboard message without usable text ignored");
            return;
        };
        if let Some(link) = self.clipboard.as_ref() {
            let _ = link.set.send(text);
        }
    }

    /// This computer's clipboard changed
    /// (rustdesk/src/server/clipboard_service.rs; sent to a connection that
    /// `can_sub_clipboard_service`: clipboard and control both on).
    fn on_clipboard(&mut self, text: String) -> bool {
        if !(self.clipboard_enabled() && self.keyboard_enabled()) {
            return true;
        }
        let msg = clipboard::text_message(&text, &self.peer_version, &self.peer_platform);
        self.send(msg)
    }

    /// rustdesk/src/server/connection.rs `Connection::start`, the
    /// `test_delay_timer` branch. Returns false when the connection ends.
    fn on_test_delay_timer(&mut self, last_recv: Instant) -> bool {
        if last_recv.elapsed() >= SEC30 {
            self.reason = "Timeout".into();
            return false;
        }
        // `VIDEO_QOS.user_delay_response_elapsed`: a probe that stays out
        // brakes the frame rate.
        if self.authorized {
            if let Some(sent) = self.last_test_delay {
                self.qos.user_delay_response_elapsed(QOS_ID, sent.elapsed().as_millis());
            }
        }
        if self.last_test_delay.is_none() {
            self.last_test_delay = Some(Instant::now());
            let probe = TestDelay {
                last_delay: self.network_delay,
                // rustdesk/src/server/video_qos.rs `bitrate()`: kbps.
                target_bitrate: self.qos.bitrate(),
                ..Default::default()
            };
            return self.send(Message { union: Some(message::Union::TestDelay(probe)) });
        }
        true
    }
}

/// The kind of a message, for the log (never its content).
fn describe(union: &Option<message::Union>) -> String {
    match union {
        None => "empty".into(),
        Some(u) => {
            let text = format!("{:?}", u);
            text.split(['(', ' ', '{']).next().unwrap_or_default().to_string()
        }
    }
}

async fn next_media(video: &mut Option<ActiveVideo>) -> Option<MediaEvent> {
    match video {
        Some(active) => active.video.events.recv().await,
        None => std::future::pending().await,
    }
}

async fn next_clipboard(link: &mut Option<ClipboardLink>) -> Option<String> {
    match link {
        Some(link) => match link.changed.recv().await {
            Some(text) => Some(text),
            // The clipboard is gone: nothing more will come.
            None => std::future::pending().await,
        },
        None => std::future::pending().await,
    }
}

async fn next_approval(approval: &mut Option<PendingApproval>) -> Option<Decision> {
    match approval {
        Some(pending) => (&mut pending.answer).await.unwrap_or(None),
        None => std::future::pending().await,
    }
}

/// rustdesk/src/server/connection.rs `file_timer` (`MILLI1` while a file is
/// read): a tick when there was nothing to send, else at once, after the
/// connection's other events had their turn.
async fn file_tick(idle: bool) {
    if idle {
        tokio::time::sleep(Duration::from_millis(1)).await
    } else {
        tokio::task::yield_now().await
    }
}

/// rustdesk/src/server/connection.rs `login_deadline`.
async fn login_deadline(authorized: bool, started: Instant) {
    if authorized {
        return std::future::pending().await;
    }
    tokio::time::sleep_until(started + LOGIN_GRACE).await
}

/// Serves one accepted connection to its end; returns why it ended
/// (rustdesk/src/server.rs `create_tcp_connection` with `secure` false, then
/// rustdesk/src/server/connection.rs `Connection::start`).
pub(crate) async fn serve(stream: TcpStream, peer: SocketAddr, slot: UnauthSlot, ctx: Arc<Shared>) -> String {
    // hbb_common/src/tcp.rs sets TCP_NODELAY and nothing else on a
    // connection's socket (rustdesk.rs sets it at accept, as `direct_server`
    // does). The send buffer is this host's own choice, see SEND_BUFFER.
    if let Err(e) = socket2::SockRef::from(&stream).set_send_buffer_size(SEND_BUFFER) {
        debug!("RustDesk connection from {}: send buffer not set: {}", peer, e);
    }
    let (mut reader, writer) = stream.into_split();
    let (tx, rx) = mpsc::channel(MAX_QUEUED_MESSAGES);
    let queued = Arc::new(Queued::default());
    let mut writer = tokio::spawn(write_loop(writer, rx, queued.clone()));
    // The writer ends with the session also when the session's task is
    // aborted (the listener starts again or stops); the capture does too
    // (`Video`'s drop), and the viewer's place is freed (`Viewing`).
    let _writer_ends = AbortOnDrop(writer.abort_handle());

    let mut session = Session {
        hash: Hash { salt: login::salt_for(&ctx.config.agent_id), challenge: login::new_challenge() },
        ctx,
        ip: peer.ip(),
        tx,
        queued,
        slot: Some(slot),
        authorized: false,
        last_test_delay: None,
        network_delay: 0,
        qos: VideoQoS::default(),
        video: None,
        displays: Vec::new(),
        current_display: 0,
        displays_checked: Instant::now(),
        decoding: None,
        host_h265: false,
        last_supported_encoding: None,
        peer_version: String::new(),
        peer_platform: String::new(),
        disable_keyboard: false,
        disable_clipboard: false,
        disable_audio: false,
        show_remote_cursor: false,
        input: None,
        last_peer_input: None,
        clipboard: None,
        cursor: CursorSender::default(),
        audio_format: None,
        approval: None,
        files: None,
        reason: String::new(),
    };

    // `create_tcp_connection`: bounded reads until login. With `secure`
    // false `identity_handshake` exchanges nothing.
    let mut decoder = frame::Decoder::new(MAX_UNAUTHORIZED_MESSAGE);
    let mut buf = BytesMut::with_capacity(8 * 1024);
    let started = Instant::now();
    let mut last_recv = Instant::now();

    // `Connection::on_open`: the salt and the challenge of the login.
    let mut open = session.send(Message { union: Some(message::Union::Hash(session.hash.clone())) });
    // `Connection::start`: a RustDesk host tells a connection what it may
    // not do, and says nothing of what it may. Mouse, keyboard and sound are
    // offered; the clipboard when the agent's `allow_clipboard` is on;
    // files when its `rustdesk_file_transfer` is on (`if !conn.file`);
    // restart, recording, blocking input and privacy mode are not.
    let mut denied = vec![
        Permission::Restart,
        Permission::Recording,
        Permission::BlockInput,
        Permission::PrivacyMode,
    ];
    if !session.ctx.config.file_transfer {
        denied.insert(0, Permission::File);
    }
    if !session.ctx.config.allow_clipboard {
        denied.insert(0, Permission::Clipboard);
    }
    for p in denied {
        open = open && session.send(permission(p, false));
    }

    let mut test_delay_timer = tokio::time::interval_at(Instant::now(), TEST_DELAY_TIMEOUT);
    test_delay_timer.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    // Whether the last turn of the file timer had nothing to send.
    let mut files_idle = false;

    'connection: while open {
        tokio::select! {
            _ = login_deadline(session.authorized, started) => {
                session.reason = "Timeout".into();
                break;
            }
            read = reader.read_buf(&mut buf) => {
                match read {
                    Ok(0) => {
                        if session.reason.is_empty() {
                            session.reason = "closed by the client".into();
                        }
                        break;
                    }
                    Ok(_) => {}
                    Err(e) => {
                        session.reason = format!("read failed: {}", e);
                        break;
                    }
                }
                loop {
                    let payload = match decoder.decode(&mut buf) {
                        Ok(Some(payload)) => payload,
                        Ok(None) => break,
                        Err(e) => {
                            session.reason = format!("bad frame: {}", e);
                            break 'connection;
                        }
                    };
                    last_recv = Instant::now();
                    let Ok(msg) = Message::decode(payload.freeze()) else {
                        debug!("RustDesk connection from {}: a message that does not parse is ignored", peer);
                        continue;
                    };
                    if !session.on_message(msg).await {
                        break 'connection;
                    }
                    if session.authorized {
                        // `send_logon_response_and_keep_alive`: the bound on
                        // reads is lifted at login.
                        decoder.set_max_packet_length(usize::MAX);
                    }
                }
            }
            decision = next_approval(&mut session.approval) => {
                if !session.on_approval(decision).await {
                    break;
                }
                if session.authorized {
                    decoder.set_max_packet_length(usize::MAX);
                }
            }
            text = next_clipboard(&mut session.clipboard) => {
                if let Some(text) = text {
                    if !session.on_clipboard(text) {
                        break;
                    }
                }
            }
            _ = test_delay_timer.tick() => {
                if !session.on_test_delay_timer(last_recv) {
                    break;
                }
                session.check_qos().await;
                if !session.check_displays().await {
                    break;
                }
            }
            // RustDesk's capture loop waits for the connection to take a
            // frame before it makes the next (video_service.rs
            // `VideoFrameController::try_wait_next`). Here: while the socket
            // has not taken what is queued, the pipeline's events are left
            // where they are, and the pipeline stops encoding when they
            // pile up (lunaris-media "Pipeline backpressure"). Nothing
            // encoded is thrown away, so no key frame is needed to go on.
            _ = session.queued.video_taken.notified(), if session.queued.video.load(Ordering::Relaxed) >= MAX_QUEUED_VIDEO => {
                // Room again: the pipeline's events are read on the next turn.
            }
            event = next_media(&mut session.video), if session.queued.video.load(Ordering::Relaxed) < MAX_QUEUED_VIDEO => {
                if !session.on_media(event) {
                    break;
                }
            }
            // `conn.file_timer`: while the client downloads.
            _ = file_tick(files_idle), if session.files.as_ref().is_some_and(|files| files.has_read_jobs()) => {
                if !session.on_file_timer(&mut files_idle) {
                    break;
                }
            }
            _ = &mut writer => {
                if session.reason.is_empty() {
                    session.reason = "the connection broke while sending".into();
                }
                break;
            }
        }
    }

    // The capture stops with the session, as a dropped BridgeSession's does.
    if let Some(active) = session.video.take() {
        info!(
            "RustDesk session ended: {} frames sent ({} key), {} dropped, {:.1} s",
            active.sent,
            active.sent_keys,
            active.dropped,
            active.started.elapsed().as_secs_f64()
        );
        stop_video(active.video).await;
        // Only now may the next viewer start its own capture.
        drop(active._viewing);
    }
    // What is queued (a login error, a close reason) still goes out.
    let Session { tx, reason, .. } = session;
    drop(tx);
    if !writer.is_finished() && tokio::time::timeout(Duration::from_secs(2), &mut writer).await.is_err() {
        writer.abort();
    }
    if reason.is_empty() {
        "closed".into()
    } else {
        reason
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rustdesk::input::tests::{recorder, Record};
    use crate::rustdesk::proto::{key_event, supported_decoding, Clipboard, KeyboardMode, MultiClipboards, SupportedDecoding};
    use crate::rustdesk::{serve_listener, RustdeskConfig};
    use lunaris_media::types::{CursorImage, CursorKind, CursorState, EncodedAudioFrame, EncodedVideoFrame as MediaFrame};
    use crate::rustdesk::qos;
    use std::sync::Mutex;
    use tokio::net::TcpListener;

    const PASSWORD: &str = "correct horse";

    /// The far ends of the fake pipeline's channels.
    type Pipeline = (mpsc::Sender<MediaEvent>, mpsc::Receiver<PipelineCommand>);

    /// The far ends of the fake clipboard: what the client set, and a way
    /// to say the computer's clipboard changed.
    type FakeClipboard = (std::sync::mpsc::Receiver<String>, mpsc::UnboundedSender<String>);

    struct FakeScreen {
        started: Mutex<Vec<(u32, Pipeline)>>,
        /// Every start: the display's id, the codec and the bitrate.
        starts: Mutex<Vec<(String, Codec, u32)>>,
        displays: Mutex<Vec<Display>>,
        /// Whether this computer encodes H.265.
        h265: bool,
        /// Whether this computer can inject input.
        can_inject: bool,
        /// Every injector call of every session.
        injected: Record,
        clipboards: Mutex<Vec<FakeClipboard>>,
        /// The answers the person here gives, in order; `None` leaves the
        /// question up.
        answers: Mutex<Vec<Option<Decision>>>,
        asked: Mutex<Vec<String>>,
    }

    impl Default for FakeScreen {
        fn default() -> Self {
            FakeScreen {
                started: Mutex::default(),
                starts: Mutex::default(),
                displays: Mutex::new(vec![Display {
                    id: "default".into(),
                    name: "Fake".into(),
                    width: 1920,
                    height: 1080,
                    primary: true,
                }]),
                h265: false,
                can_inject: true,
                injected: Record::default(),
                clipboards: Mutex::default(),
                answers: Mutex::default(),
                asked: Mutex::default(),
            }
        }
    }

    impl FakeScreen {
        fn injected(&self) -> Vec<String> {
            std::mem::take(&mut *self.injected.lock().unwrap())
        }

        /// What was injected once `count` calls arrived (the input thread
        /// runs on its own).
        async fn injected_after(&self, count: usize) -> Vec<String> {
            for _ in 0..200 {
                if self.injected.lock().unwrap().len() >= count {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            self.injected()
        }
    }

    #[async_trait::async_trait]
    impl Screen for FakeScreen {
        async fn displays(&self) -> anyhow::Result<Vec<Display>> {
            Ok(self.displays.lock().unwrap().clone())
        }

        async fn encodes_h265(&self, _display: &Display) -> bool {
            self.h265
        }

        fn start(&self, display: &Display, codec: Codec, bitrate_kbps: u32) -> Video {
            self.starts.lock().unwrap().push((display.id.clone(), codec, bitrate_kbps));
            let (event_tx, events) = mpsc::channel(64);
            let (commands, mut command_rx) = mpsc::channel(16);
            let (seen_tx, seen_rx) = mpsc::channel(16);
            // Ends on Stop, as the real pipeline does.
            let task = tokio::spawn(async move {
                while let Some(command) = command_rx.recv().await {
                    let stop = matches!(command, PipelineCommand::Stop);
                    let _ = seen_tx.send(command).await;
                    if stop {
                        break;
                    }
                }
            });
            self.started.lock().unwrap().push((bitrate_kbps, (event_tx, seen_rx)));
            Video { events, commands, task }
        }

        fn injector(&self) -> Option<Box<dyn Injector>> {
            if !self.can_inject {
                return None;
            }
            let (_, own) = recorder();
            let all = self.injected.clone();
            drop(own);
            Some(Box::new(RecordingInjector(Box::new(move |line| all.lock().unwrap().push(line)))))
        }

        fn clipboard(&self) -> Option<ClipboardLink> {
            let (set, set_rx) = std::sync::mpsc::channel();
            let (changed_tx, changed) = mpsc::unbounded_channel();
            self.clipboards.lock().unwrap().push((set_rx, changed_tx));
            Some(ClipboardLink { set, changed })
        }
    }

    async fn start_host(mode: &str) -> (SocketAddr, Arc<FakeScreen>) {
        start_host_with(mode, true, FakeScreen::default()).await
    }

    async fn start_host_with(mode: &str, allow_clipboard: bool, screen: FakeScreen) -> (SocketAddr, Arc<FakeScreen>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let screen = Arc::new(screen);
        let config = RustdeskConfig {
            port: addr.port(),
            password: PASSWORD.into(),
            approve_mode: ApproveMode::parse(mode),
            allow_clipboard,
            agent_id: "test-agent".into(),
            name: "testhost".into(),
            file_transfer: false,
            file_access: crate::files::Access::Denied("not in this test".into()),
            file_home: None,
        };
        // The person here: answers from `answers`, or never.
        let person = screen.clone();
        let asker: Asker = Arc::new(move |requester, _kind, withdrawn| {
            person.asked.lock().unwrap().push(requester);
            let answer = {
                let mut answers = person.answers.lock().unwrap();
                if answers.is_empty() { None } else { answers.remove(0) }
            };
            Box::pin(async move {
                match answer {
                    Some(decision) => {
                        tokio::time::sleep(Duration::from_millis(50)).await;
                        Some(decision)
                    }
                    None => {
                        let _ = withdrawn.await;
                        None
                    }
                }
            })
        });
        tokio::spawn(serve_listener(listener, config, screen.clone(), asker, Arc::new(|| true), Arc::default()));
        (addr, screen)
    }

    /// The client's side of the connection, as rustdesk/src/client.rs and
    /// rustdesk/src/client/io_loop.rs speak it.
    struct Client {
        stream: TcpStream,
        decoder: frame::Decoder,
        buf: BytesMut,
        hash: Hash,
        /// A probe of the host that `next_reply` read past, not answered
        /// yet (the host's first one may come before its LoginResponse).
        skipped_probe: Option<TestDelay>,
    }

    impl Client {
        /// Connects and reads the `Hash` the host opens with
        /// (io_loop.rs `handle_msg_from_peer`, the `Hash` branch).
        async fn connect(addr: SocketAddr) -> Client {
            let stream = TcpStream::connect(addr).await.unwrap();
            let mut client =
                Client { stream, decoder: frame::Decoder::new(usize::MAX), buf: BytesMut::new(), hash: Hash::default(), skipped_probe: None };
            match client.next().await.union {
                Some(message::Union::Hash(hash)) => client.hash = hash,
                other => panic!("expected Hash, got {:?}", other),
            }
            client
        }

        async fn send(&mut self, msg: Message) {
            self.stream.write_all(&encode(&msg)).await.unwrap();
        }

        async fn try_next(&mut self) -> Option<Message> {
            loop {
                if let Some(payload) = self.decoder.decode(&mut self.buf).unwrap() {
                    return Some(Message::decode(payload.freeze()).unwrap());
                }
                if self.stream.read_buf(&mut self.buf).await.unwrap() == 0 {
                    return None;
                }
            }
        }

        async fn next(&mut self) -> Message {
            tokio::time::timeout(Duration::from_secs(5), self.try_next())
                .await
                .expect("a message in time")
                .expect("the connection is open")
        }

        /// The next message that is not a permission notice or a delay probe.
        async fn next_reply(&mut self) -> Message {
            loop {
                let msg = self.next().await;
                match &msg.union {
                    Some(message::Union::TestDelay(t)) => {
                        if !t.from_client {
                            self.skipped_probe = Some(t.clone());
                        }
                    }
                    Some(message::Union::Misc(Misc { union: Some(misc::Union::PermissionInfo(_)) })) => {}
                    _ => return msg,
                }
            }
        }

        /// client.rs `handle_login_from_ui` then `create_login_msg`.
        fn login_request(&self, password: &str, h264: bool, custom_fps: i32) -> Message {
            let proof = if password.is_empty() {
                Bytes::new()
            } else {
                Bytes::copy_from_slice(&login::proof(&login::h1(password, &self.hash.salt), &self.hash.challenge))
            };
            Message {
                union: Some(message::Union::LoginRequest(LoginRequest {
                    username: "127.0.0.1".into(),
                    password: proof,
                    my_id: "123456789".into(),
                    my_name: "Tester".into(),
                    my_platform: "Linux".into(),
                    version: "1.5.3".into(),
                    option: Some(OptionMessage {
                        custom_fps,
                        supported_decoding: Some(SupportedDecoding {
                            ability_vp8: 1,
                            ability_vp9: 1,
                            ability_h264: h264 as i32,
                            prefer: supported_decoding::PreferCodec::Auto as i32,
                            ..Default::default()
                        }),
                        ..Default::default()
                    }),
                    ..Default::default()
                })),
            }
        }

        async fn login(&mut self, password: &str) -> Result<PeerInfo, String> {
            let request = self.login_request(password, true, 0);
            self.login_with(request).await
        }

        async fn login_with(&mut self, request: Message) -> Result<PeerInfo, String> {
            self.send(request).await;
            match self.next_reply().await.union {
                Some(message::Union::LoginResponse(LoginResponse { union: Some(union), .. })) => match union {
                    login_response::Union::PeerInfo(info) => Ok(info),
                    login_response::Union::Error(error) => Err(error),
                },
                other => panic!("expected LoginResponse, got {:?}", other),
            }
        }
    }

    fn frame(key: bool, fill: u8) -> MediaEvent {
        MediaEvent::VideoFrame(MediaFrame {
            data: vec![0, 0, 0, 1, fill, fill],
            frame_type: if key { FrameType::Key } else { FrameType::Inter },
            pts: 0,
            duration: 16_666,
            codec: VideoCodec::H264,
        })
    }

    fn take_pipeline(screen: &FakeScreen) -> (u32, Pipeline) {
        screen.started.lock().unwrap().pop().expect("the pipeline was started")
    }

    /// The pipeline started in place of a stopped one.
    async fn restarted(screen: &FakeScreen) -> (u32, Pipeline) {
        for _ in 0..100 {
            if let Some(pipeline) = screen.started.lock().unwrap().pop() {
                return pipeline;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        panic!("the pipeline was not started again");
    }

    async fn next_command(commands: &mut mpsc::Receiver<PipelineCommand>) -> PipelineCommand {
        tokio::time::timeout(Duration::from_secs(5), commands.recv()).await.expect("a command in time").unwrap()
    }

    fn h265(msg: Message) -> (i32, EncodedVideoFrame) {
        match msg.union {
            Some(message::Union::VideoFrame(VideoFrame { display, union: Some(video_frame::Union::H265s(mut frames)) })) => {
                assert_eq!(frames.frames.len(), 1);
                (display, frames.frames.remove(0))
            }
            other => panic!("expected an H.265 VideoFrame, got {:?}", other),
        }
    }

    fn decoding(h264: bool, h265: bool, prefer: supported_decoding::PreferCodec) -> SupportedDecoding {
        SupportedDecoding {
            ability_vp8: 1,
            ability_vp9: 1,
            ability_av1: 1,
            ability_h264: h264 as i32,
            ability_h265: h265 as i32,
            prefer: prefer as i32,
            ..Default::default()
        }
    }

    // rustdesk/libs/scrap/src/common/codec.rs `Encoder::update`.
    #[test]
    fn the_codec_is_chosen_as_rustdesk_chooses_between_h264_and_h265() {
        use supported_decoding::PreferCodec::*;
        let pick = |h264, h265, prefer, host_h265| negotiate(Some(&decoding(h264, h265, prefer)), host_h265).0;
        // auto: h265 > h264, each only when the client decodes it and the
        // host encodes it.
        assert_eq!(pick(true, true, Auto, true), Some(Codec::H265));
        assert_eq!(pick(true, true, Auto, false), Some(Codec::H264));
        assert_eq!(pick(true, false, Auto, true), Some(Codec::H264));
        assert_eq!(pick(false, true, Auto, true), Some(Codec::H265));
        // A usable preference wins; an unusable one falls back to auto.
        assert_eq!(pick(true, true, H264, true), Some(Codec::H264));
        assert_eq!(pick(true, true, H265, true), Some(Codec::H265));
        assert_eq!(pick(true, true, H265, false), Some(Codec::H264));
        assert_eq!(pick(false, true, H264, true), Some(Codec::H265));
        // The codecs this host does not have: as auto.
        assert_eq!(pick(true, true, Vp9, true), Some(Codec::H265));
        assert_eq!(pick(true, false, Av1, true), Some(Codec::H264));
        // Nothing in common.
        assert_eq!(pick(false, false, Auto, true), None);
        assert_eq!(pick(false, true, Auto, false), None);
        assert_eq!(negotiate(None, true).0, None);
        let usable = negotiate(Some(&decoding(true, true, Auto)), false).1;
        assert!(usable.h264 && !usable.h265 && !usable.vp8 && !usable.av1);
    }

    fn with_decoding(mut request: Message, decoding: SupportedDecoding) -> Message {
        if let Some(message::Union::LoginRequest(lr)) = request.union.as_mut() {
            lr.option.as_mut().unwrap().supported_decoding = Some(decoding);
        }
        request
    }

    #[tokio::test]
    async fn h265_is_sent_when_both_sides_can_and_follows_the_clients_preference() {
        use supported_decoding::PreferCodec;
        let (addr, screen) = start_host_with("password", true, FakeScreen { h265: true, ..Default::default() }).await;
        let mut client = Client::connect(addr).await;
        let request = with_decoding(client.login_request(PASSWORD, true, 0), decoding(true, true, PreferCodec::Auto));
        let info = client.login_with(request).await.expect("logged in");
        let encoding = info.encoding.unwrap();
        assert!(encoding.h264 && encoding.h265);
        // Auto: H.265, at RustDesk's bitrate for it.
        let (bitrate, (events, mut commands)) = take_pipeline(&screen);
        assert_eq!(bitrate, qos::calc_bitrate(1920, 1080, qos::BR_BALANCED, false) * 2);
        assert_eq!(screen.starts.lock().unwrap().last().unwrap().1, Codec::H265);
        events.send(frame(true, 1)).await.unwrap();
        assert!(h265(client.next_reply().await).1.key);

        // The client now prefers H.264: the encoder is made anew.
        client
            .send(option(OptionMessage { supported_decoding: Some(decoding(true, true, PreferCodec::H264)), ..Default::default() }))
            .await;
        loop {
            if matches!(next_command(&mut commands).await, PipelineCommand::Stop) {
                break;
            }
        }
        let (bitrate, (events, mut commands)) = restarted(&screen).await;
        assert_eq!(bitrate, balanced_kbps());
        assert_eq!(screen.starts.lock().unwrap().last().unwrap().1, Codec::H264);
        // The new stream starts at a key frame.
        events.send(frame(false, 2)).await.unwrap();
        events.send(frame(true, 3)).await.unwrap();
        let (_, first) = h264(client.next_reply().await);
        assert!(first.key);

        // And back.
        client
            .send(option(OptionMessage { supported_decoding: Some(decoding(true, true, PreferCodec::H265)), ..Default::default() }))
            .await;
        loop {
            if matches!(next_command(&mut commands).await, PipelineCommand::Stop) {
                break;
            }
        }
        let (_, (events, _commands)) = restarted(&screen).await;
        assert_eq!(screen.starts.lock().unwrap().last().unwrap().1, Codec::H265);
        events.send(frame(true, 4)).await.unwrap();
        assert!(h265(client.next_reply().await).1.key);
        drop(client);

        // A client without an H.265 decoder: H.264, and it is told which
        // codecs are usable with it.
        let mut client = Client::connect(addr).await;
        let request = with_decoding(client.login_request(PASSWORD, true, 0), decoding(true, false, PreferCodec::H265));
        let mut tries = 0;
        while let Err(e) = client.login_with(request.clone()).await {
            tries += 1;
            assert!(tries < 50, "still refused: {}", e);
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        match client.next_reply().await.union {
            Some(message::Union::Misc(Misc { union: Some(misc::Union::SupportedEncoding(usable)) })) => {
                assert!(usable.h264 && !usable.h265)
            }
            other => panic!("expected SupportedEncoding, got {:?}", other),
        }
        restarted(&screen).await;
        assert_eq!(screen.starts.lock().unwrap().last().unwrap().1, Codec::H264);
    }

    #[tokio::test]
    async fn every_display_is_listed_and_the_picture_moves_between_them() {
        let screen = FakeScreen::default();
        *screen.displays.lock().unwrap() = vec![
            Display { id: "left".into(), name: "DP-1".into(), width: 2560, height: 1440, primary: false },
            Display { id: "right".into(), name: "HDMI-1".into(), width: 1920, height: 1080, primary: true },
        ];
        let (addr, screen) = start_host_with("password", true, screen).await;
        let mut client = Client::connect(addr).await;
        let info = client.login(PASSWORD).await.expect("logged in");
        assert_eq!(info.displays.len(), 2);
        assert_eq!((info.displays[0].name.as_str(), info.displays[0].width, info.displays[0].height), ("DP-1", 2560, 1440));
        assert_eq!((info.displays[1].name.as_str(), info.displays[1].width, info.displays[1].height), ("HDMI-1", 1920, 1080));
        // The primary display is shown first.
        assert_eq!(info.current_display, 1);
        let (_, (events, mut commands)) = take_pipeline(&screen);
        assert_eq!(screen.starts.lock().unwrap().last().unwrap().0, "right");
        events.send(frame(true, 1)).await.unwrap();
        assert_eq!(h264(client.next_reply().await).0, 1);

        // `SwitchDisplay`: the other one, at its own size and bitrate; the
        // client hears the display-changed message.
        client.send(misc_message(misc::Union::SwitchDisplay(SwitchDisplay { display: 0, ..Default::default() }))).await;
        loop {
            if matches!(next_command(&mut commands).await, PipelineCommand::Stop) {
                break;
            }
        }
        let (bitrate, (events, mut commands)) = restarted(&screen).await;
        assert_eq!(screen.starts.lock().unwrap().last().unwrap().0, "left");
        assert_eq!(bitrate, qos::calc_bitrate(2560, 1440, qos::BR_BALANCED, true) * 2);
        match client.next_reply().await.union {
            Some(message::Union::Misc(Misc { union: Some(misc::Union::SwitchDisplay(s)) })) => {
                assert_eq!((s.display, s.width, s.height), (0, 2560, 1440))
            }
            other => panic!("expected SwitchDisplay, got {:?}", other),
        }
        events.send(frame(true, 2)).await.unwrap();
        assert_eq!(h264(client.next_reply().await).0, 0);
        // The pointer's positions are now within that display.
        client.send(mouse(0, 2000, 1000)).await;
        assert_eq!(screen.injected_after(1).await, ["move_abs 2000 1000 of 2560x1440"]);
        // A display that does not exist is ignored.
        client.send(misc_message(misc::Union::SwitchDisplay(SwitchDisplay { display: 7, ..Default::default() }))).await;

        // `CaptureDisplays` (clients from 1.2.4): the display asked for.
        client
            .send(misc_message(misc::Union::CaptureDisplays(crate::rustdesk::proto::CaptureDisplays { set: vec![1], ..Default::default() })))
            .await;
        loop {
            if matches!(next_command(&mut commands).await, PipelineCommand::Stop) {
                break;
            }
        }
        let (_, (events, mut commands)) = restarted(&screen).await;
        assert_eq!(screen.starts.lock().unwrap().last().unwrap().0, "right");
        events.send(frame(true, 3)).await.unwrap();
        assert_eq!(h264(client.next_reply().await).0, 1);

        // The display shown changes its resolution: the client is told the
        // displays again and the change, and the picture starts anew.
        screen.displays.lock().unwrap()[1] =
            Display { id: "right".into(), name: "HDMI-1".into(), width: 1280, height: 720, primary: true };
        let told = loop {
            if let Some(message::Union::PeerInfo(info)) = client.next().await.union {
                break info;
            }
        };
        assert_eq!((told.displays[1].width, told.displays[1].height), (1280, 720));
        match client.next_reply().await.union {
            Some(message::Union::Misc(Misc { union: Some(misc::Union::SwitchDisplay(s)) })) => {
                assert_eq!((s.display, s.width, s.height), (1, 1280, 720))
            }
            other => panic!("expected SwitchDisplay, got {:?}", other),
        }
        loop {
            if matches!(next_command(&mut commands).await, PipelineCommand::Stop) {
                break;
            }
        }
        let (bitrate, _) = restarted(&screen).await;
        assert_eq!(bitrate, qos::calc_bitrate(1280, 720, qos::BR_BALANCED, true) * 2);
    }

    fn h264(msg: Message) -> (i32, EncodedVideoFrame) {
        match msg.union {
            Some(message::Union::VideoFrame(VideoFrame { display, union: Some(video_frame::Union::H264s(mut frames)) })) => {
                assert_eq!(frames.frames.len(), 1);
                (display, frames.frames.remove(0))
            }
            other => panic!("expected an H.264 VideoFrame, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn the_right_password_gets_peer_info_and_the_picture_from_a_key_frame() {
        let (addr, screen) = start_host("password").await;
        let mut client = Client::connect(addr).await;
        assert!(!client.hash.salt.is_empty() && !client.hash.challenge.is_empty());

        let info = client.login(PASSWORD).await.expect("logged in");
        assert_eq!(info.hostname, "testhost");
        assert_eq!(info.version, PROTOCOL_VERSION);
        assert_eq!(info.displays.len(), 1);
        assert_eq!((info.displays[0].width, info.displays[0].height), (1920, 1080));
        assert_eq!(info.displays[0].scale, 1.0);
        assert_eq!(info.current_display, 0);
        assert!(info.encoding.as_ref().unwrap().h264);

        // A client that asked for nothing gets RustDesk's Balanced quality:
        // its hardware H.264 bitrate for 1080p (2777 kbps for an encoder
        // at 30 fps), as the same budget per frame for one at 60 fps.
        let (bitrate, (events, mut commands)) = take_pipeline(&screen);
        assert_eq!(bitrate, balanced_kbps());
        assert_eq!(bitrate, 2777 * 2);
        // And RustDesk's default frame rate, as a limit on the pipeline.
        assert!(matches!(next_command(&mut commands).await, PipelineCommand::SetFrameLimit(30)));
        // Frames before the first key frame cannot be decoded: not sent.
        events.send(frame(false, 1)).await.unwrap();
        events.send(frame(true, 2)).await.unwrap();
        events.send(frame(false, 3)).await.unwrap();
        let (display, first) = h264(client.next_reply().await);
        assert_eq!(display, 0);
        assert!(first.key);
        assert_eq!(&first.data[..], &[0, 0, 0, 1, 2, 2]);
        let (_, second) = h264(client.next_reply().await);
        assert!(!second.key);
        assert_eq!(&second.data[..], &[0, 0, 0, 1, 3, 3]);
        assert!(second.pts >= first.pts);
    }

    #[tokio::test]
    async fn a_wrong_password_is_refused_and_can_be_corrected_on_the_same_connection() {
        let (addr, screen) = start_host("password").await;
        let mut client = Client::connect(addr).await;
        // The client first logs in without a password while it asks its user.
        let empty = client.login_request("", true, 0);
        client.send(empty).await;
        assert_eq!(client.login("wrong").await.unwrap_err(), login::LOGIN_MSG_PASSWORD_WRONG);
        assert!(screen.started.lock().unwrap().is_empty());
        client.login(PASSWORD).await.expect("logged in");
        // A proof for another connection's challenge is worth nothing here.
        let other = Client::connect(addr).await;
        let mut third = Client::connect(addr).await;
        let replayed = other.login_request(PASSWORD, true, 0);
        assert_eq!(third.login_with(replayed).await.unwrap_err(), login::LOGIN_MSG_PASSWORD_WRONG);
    }

    #[tokio::test]
    async fn repeated_wrong_passwords_lock_the_address_out() {
        let (addr, _screen) = start_host("password").await;
        let mut client = Client::connect(addr).await;
        for _ in 0..5 {
            assert_eq!(client.login("wrong").await.unwrap_err(), login::LOGIN_MSG_PASSWORD_WRONG);
        }
        // Now even the right password is turned away, on any connection.
        assert_eq!(client.login(PASSWORD).await.unwrap_err(), login::LOGIN_MSG_TRY_LATER);
        let mut other = Client::connect(addr).await;
        assert_eq!(other.login(PASSWORD).await.unwrap_err(), login::LOGIN_MSG_TRY_LATER);
    }

    #[tokio::test]
    async fn nothing_is_acted_on_before_login() {
        let (addr, screen) = start_host("password").await;
        let mut client = Client::connect(addr).await;
        client.send(misc_message(misc::Union::RefreshVideo(true))).await;
        client
            .send(misc_message(misc::Union::Option(OptionMessage { custom_fps: 60, ..Default::default() })))
            .await;
        client.send(Message { union: Some(message::Union::MouseEvent(Default::default())) }).await;
        client
            .send(misc_message(misc::Union::Option(OptionMessage { image_quality: ImageQuality::Best as i32, ..Default::default() })))
            .await;
        // The options sent before login are not remembered.
        client.login(PASSWORD).await.expect("logged in");
        let (bitrate, _) = take_pipeline(&screen);
        assert_eq!(bitrate, balanced_kbps());
    }

    fn balanced_kbps() -> u32 {
        qos::calc_bitrate(1920, 1080, qos::BR_BALANCED, true) * CAPTURE_FPS / HWCODEC_DEFAULT_FPS
    }

    /// Answers the host's next delay probe, as the client does.
    async fn answer_probe(client: &mut Client) {
        // The host sends one probe and waits for its answer: the one read
        // past during the login is the one to answer.
        if let Some(t) = client.skipped_probe.take() {
            client.send(Message { union: Some(message::Union::TestDelay(t)) }).await;
            return;
        }
        loop {
            if let Some(message::Union::TestDelay(t)) = client.next().await.union {
                if !t.from_client {
                    client.send(Message { union: Some(message::Union::TestDelay(t)) }).await;
                    return;
                }
            }
        }
    }

    #[tokio::test]
    async fn the_client_chooses_quality_and_asks_for_key_frames() {
        let (addr, screen) = start_host("password").await;
        let mut client = Client::connect(addr).await;
        // The quality configured in the client comes with the login.
        let mut request = client.login_request(PASSWORD, true, 60);
        if let Some(message::Union::LoginRequest(lr)) = request.union.as_mut() {
            lr.option.as_mut().unwrap().image_quality = ImageQuality::Best as i32;
        }
        client.login_with(request).await.expect("logged in");
        let (bitrate, (events, mut commands)) = take_pipeline(&screen);
        let best = qos::calc_bitrate(1920, 1080, qos::BR_BEST, true);
        assert_eq!(best, 4583);
        assert_eq!(bitrate, best * 2);
        assert!(matches!(next_command(&mut commands).await, PipelineCommand::SetFrameLimit(30)));

        client.send(misc_message(misc::Union::RefreshVideo(true))).await;
        assert!(matches!(next_command(&mut commands).await, PipelineCommand::RequestKeyframe));
        client.send(misc_message(misc::Union::RefreshVideoDisplay(0))).await;
        assert!(matches!(next_command(&mut commands).await, PipelineCommand::RequestKeyframe));

        // Another quality while connected: the running encoder is given the
        // new bitrate; nothing is restarted.
        client
            .send(misc_message(misc::Union::Option(OptionMessage { image_quality: ImageQuality::Low as i32, ..Default::default() })))
            .await;
        let low = qos::calc_bitrate(1920, 1080, qos::BR_SPEED, true);
        match next_command(&mut commands).await {
            PipelineCommand::SetBitrate(kbps) => assert_eq!(kbps, low * 2),
            other => panic!("expected SetBitrate, got {:?}", other),
        }
        // A custom quality: the percentage is in the second byte
        // (`custom_image_quality = quality << 8`), 100 % is ratio 2.0.
        client
            .send(misc_message(misc::Union::Option(OptionMessage { custom_image_quality: 100 << 8, ..Default::default() })))
            .await;
        match next_command(&mut commands).await {
            PipelineCommand::SetBitrate(kbps) => assert_eq!(kbps, qos::calc_bitrate(1920, 1080, 2.0, true) * 2),
            other => panic!("expected SetBitrate, got {:?}", other),
        }

        // The first delay reply starts the frame rate control: a new
        // connection begins low (`INIT_FPS`) and is raised from there.
        answer_probe(&mut client).await;
        match next_command(&mut commands).await {
            PipelineCommand::SetFrameLimit(fps) => assert!((1..30).contains(&fps), "{}", fps),
            other => panic!("expected SetFrameLimit, got {:?}", other),
        }

        events.send(frame(true, 9)).await.unwrap();
        assert!(h264(client.next_reply().await).1.key);
        assert!(screen.started.lock().unwrap().is_empty(), "nothing was restarted");

        // Closing stops the pipeline.
        client.send(misc_message(misc::Union::CloseReason(String::new()))).await;
        loop {
            if matches!(next_command(&mut commands).await, PipelineCommand::Stop) {
                break;
            }
        }
    }

    #[tokio::test]
    async fn an_encoder_that_cannot_change_its_bitrate_is_restarted_for_a_new_quality() {
        let (addr, screen) = start_host("password").await;
        let mut client = Client::connect(addr).await;
        client.login(PASSWORD).await.expect("logged in");
        let (_, (events, mut commands)) = take_pipeline(&screen);
        events
            .send(MediaEvent::EncoderStarted {
                encoder: lunaris_media::EncoderInfo {
                    name: "libx264".into(),
                    hw_type: lunaris_media::types::HwAccelType::Software,
                    supported_codecs: vec![],
                },
                gpu_name: None,
                requested_encoder: None,
            })
            .await
            .unwrap();
        events.send(frame(true, 1)).await.unwrap();
        h264(client.next_reply().await);
        client
            .send(misc_message(misc::Union::Option(OptionMessage { image_quality: ImageQuality::Best as i32, ..Default::default() })))
            .await;
        loop {
            match next_command(&mut commands).await {
                PipelineCommand::Stop => break,
                PipelineCommand::SetBitrate(_) => panic!("this encoder was not to be told a bitrate"),
                _ => {}
            }
        }
        let (bitrate, _) = restarted(&screen).await;
        assert_eq!(bitrate, qos::calc_bitrate(1920, 1080, qos::BR_BEST, true) * 2);
    }

    // A client that stops reading: the host must not pile frames up. What
    // the socket does not take stays with the pipeline, which then stops
    // encoding (here: the fake pipeline's channel fills).
    #[tokio::test]
    async fn a_reader_that_stalls_holds_the_pipeline_back() {
        let (addr, screen) = start_host("password").await;
        let mut client = Client::connect(addr).await;
        // A small receive buffer, like a slow link right behind the host.
        socket2::SockRef::from(&client.stream).set_recv_buffer_size(64 * 1024).unwrap();
        client.login(PASSWORD).await.expect("logged in");
        let (_, (events, _commands)) = take_pipeline(&screen);
        let big = |key: bool, n: u32| {
            let mut data = vec![0u8; 32 * 1024];
            data[..4].copy_from_slice(&n.to_be_bytes());
            MediaEvent::VideoFrame(MediaFrame {
                data,
                frame_type: if key { FrameType::Key } else { FrameType::Inter },
                pts: 0,
                duration: 16_666,
                codec: VideoCodec::H264,
            })
        };
        // 600 frames (19 MB) offered at once while the client reads nothing.
        // Each is numbered by how many were taken before it.
        let mut accepted = 0u32;
        for _ in 0..600u32 {
            if events.try_send(big(accepted == 0, accepted)).is_ok() {
                accepted += 1;
            } else {
                tokio::time::sleep(Duration::from_millis(2)).await;
            }
        }
        // Taken: the two socket buffers, the session's few frames and the
        // pipeline's channel; far from all of them.
        assert!(accepted < 300, "{} frames were taken from the pipeline", accepted);
        // What was taken arrives whole and in order once the client reads.
        let mut expected = 0u32;
        while expected < accepted {
            let (_, got) = h264(client.next_reply().await);
            assert_eq!(u32::from_be_bytes(got.data[..4].try_into().unwrap()), expected);
            assert_eq!(got.key, expected == 0);
            expected += 1;
        }
    }

    #[tokio::test]
    async fn the_delay_is_probed_and_the_clients_probe_is_echoed() {
        let (addr, _screen) = start_host("password").await;
        let mut client = Client::connect(addr).await;
        // The host's probe comes before login too; answered as the client
        // does (client.rs `handle_test_delay`): sent back unchanged.
        let probe = loop {
            if let Some(message::Union::TestDelay(t)) = client.next().await.union {
                break t;
            }
        };
        assert!(!probe.from_client);
        assert_eq!(probe.last_delay, 0);
        tokio::time::sleep(Duration::from_millis(30)).await;
        client.send(Message { union: Some(message::Union::TestDelay(probe)) }).await;
        // The next probe reports the delay measured with the first.
        let second = loop {
            if let Some(message::Union::TestDelay(t)) = client.next().await.union {
                break t;
            }
        };
        assert!(second.last_delay >= 30, "delay {}", second.last_delay);

        let mine = TestDelay { time: 42, from_client: true, ..Default::default() };
        client.send(Message { union: Some(message::Union::TestDelay(mine)) }).await;
        let echoed = loop {
            match client.next().await.union {
                Some(message::Union::TestDelay(t)) if t.from_client => break t,
                _ => {}
            }
        };
        assert_eq!(echoed, mine);
    }

    #[tokio::test]
    async fn what_is_not_offered_is_refused() {
        let (addr, screen) = start_host("password").await;

        // A client without an H.264 decoder.
        let mut client = Client::connect(addr).await;
        let request = client.login_request(PASSWORD, false, 0);
        assert!(client.login_with(request).await.unwrap_err().contains("H.264"));

        // File transfer, however right the password.
        let mut client = Client::connect(addr).await;
        let mut request = client.login_request(PASSWORD, true, 0);
        if let Some(message::Union::LoginRequest(lr)) = request.union.as_mut() {
            lr.union = Some(login_request::Union::FileTransfer(Default::default()));
        }
        assert_eq!(client.login_with(request).await.unwrap_err(), "No permission of file transfer");

        // A client that asked for a RustDesk id, not an address.
        let mut client = Client::connect(addr).await;
        let mut request = client.login_request(PASSWORD, true, 0);
        if let Some(message::Union::LoginRequest(lr)) = request.union.as_mut() {
            lr.username = "123456789".into();
        }
        assert_eq!(client.login_with(request).await.unwrap_err(), login::LOGIN_MSG_OFFLINE);
        assert!(screen.started.lock().unwrap().is_empty());

        // A second viewer while one is connected.
        let mut first = Client::connect(addr).await;
        first.login(PASSWORD).await.expect("logged in");
        let mut second = Client::connect(addr).await;
        assert!(second.login(PASSWORD).await.unwrap_err().contains("already viewing"));
        // The place is free again once the first one left.
        drop(first);
        let mut again = Client::connect(addr).await;
        let mut tries = 0;
        while let Err(e) = again.login(PASSWORD).await {
            tries += 1;
            assert!(tries < 50, "still refused: {}", e);
            tokio::time::sleep(Duration::from_millis(50)).await;
            again = Client::connect(addr).await;
        }
    }

    #[tokio::test]
    async fn click_only_hosts_say_the_password_does_not_let_in() {
        let (addr, screen) = start_host("click").await;
        let mut client = Client::connect(addr).await;
        assert_eq!(client.login(PASSWORD).await.unwrap_err(), login::LOGIN_MSG_NO_PASSWORD_ACCESS);
        assert!(screen.started.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn an_oversized_message_before_login_ends_the_connection() {
        let (addr, _screen) = start_host("password").await;
        let mut client = Client::connect(addr).await;
        let mut header = BytesMut::new();
        frame::put_header(MAX_UNAUTHORIZED_MESSAGE + 1, &mut header).unwrap();
        client.stream.write_all(&header).await.unwrap();
        let closed = tokio::time::timeout(Duration::from_secs(5), async {
            while client.try_next().await.is_some() {}
        })
        .await;
        assert!(closed.is_ok(), "the host closed the connection");
    }

    #[tokio::test]
    async fn connections_not_logged_in_are_capped_per_address() {
        let (addr, _screen) = start_host("password").await;
        let mut held = Vec::new();
        // direct.rs MAX_UNAUTH_PER_ADDR
        for _ in 0..8 {
            held.push(Client::connect(addr).await);
        }
        // One more is accepted by the kernel and dropped by the listener.
        let mut extra = TcpStream::connect(addr).await.unwrap();
        let mut byte = [0u8; 1];
        let read = tokio::time::timeout(Duration::from_secs(5), extra.read(&mut byte)).await.expect("closed in time");
        assert!(matches!(read, Ok(0) | Err(_)));
        // Logging in gives the place back.
        held[0].login(PASSWORD).await.expect("logged in");
        Client::connect(addr).await;
    }

    fn option(o: OptionMessage) -> Message {
        misc_message(misc::Union::Option(o))
    }

    fn mouse(mask: i32, x: i32, y: i32) -> Message {
        Message { union: Some(message::Union::MouseEvent(MouseEvent { mask, x, y, ..Default::default() })) }
    }

    fn map_key(code: u32, down: bool) -> Message {
        Message {
            union: Some(message::Union::KeyEvent(KeyEvent {
                down,
                mode: KeyboardMode::Map as i32,
                union: Some(key_event::Union::Chr(code)),
                ..Default::default()
            })),
        }
    }

    fn cursor(x: i32, y: i32, fill: Option<u8>) -> MediaEvent {
        MediaEvent::CursorUpdate(CursorState {
            x,
            y,
            visible: true,
            kind: CursorKind::Arrow,
            image: fill.map(|fill| CursorImage { width: 2, height: 2, hotspot_x: 0, hotspot_y: 1, rgba_data: vec![fill; 16] }),
            in_window_move_size: false,
        })
    }

    fn audio(fill: u8) -> MediaEvent {
        MediaEvent::AudioFrame(EncodedAudioFrame {
            data: vec![fill; 40],
            pts: 0,
            duration: 10_000,
            sample_rate: 48_000,
            channels: 2,
        })
    }

    /// Logs in and returns the permissions the host switched off on the way.
    async fn login_with_permissions(client: &mut Client) -> (Vec<Permission>, PeerInfo) {
        let request = client.login_request(PASSWORD, true, 0);
        client.send(request).await;
        let mut off = Vec::new();
        loop {
            match client.next().await.union {
                Some(message::Union::Misc(Misc { union: Some(misc::Union::PermissionInfo(p)) })) => {
                    assert!(!p.enabled);
                    off.push(Permission::try_from(p.permission).unwrap());
                }
                Some(message::Union::LoginResponse(LoginResponse { union: Some(login_response::Union::PeerInfo(info)), .. })) => {
                    return (off, info)
                }
                Some(message::Union::TestDelay(_)) => {}
                other => panic!("unexpected {:?}", other),
            }
        }
    }

    async fn clipboard_set(set: &std::sync::mpsc::Receiver<String>) -> Option<String> {
        for _ in 0..100 {
            if let Ok(text) = set.try_recv() {
                return Some(text);
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        None
    }

    fn clipboard_text(msg: Message) -> String {
        match msg.union {
            Some(message::Union::MultiClipboards(mcb)) => clipboard::text_from(&mcb.clipboards).expect("text"),
            other => panic!("expected MultiClipboards, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn the_host_says_what_a_connection_may_not_do() {
        // Control, clipboard and sound on: only what is never offered is off.
        let (addr, _screen) = start_host("password").await;
        let mut client = Client::connect(addr).await;
        let (off, _) = login_with_permissions(&mut client).await;
        assert_eq!(
            off,
            [Permission::File, Permission::Restart, Permission::Recording, Permission::BlockInput, Permission::PrivacyMode]
        );
        // The agent's clipboard switch off, and a computer that cannot inject.
        let screen = FakeScreen { can_inject: false, ..Default::default() };
        let (addr, screen) = start_host_with("password", false, screen).await;
        let mut client = Client::connect(addr).await;
        let (off, _) = login_with_permissions(&mut client).await;
        assert_eq!(
            off,
            [
                Permission::Clipboard,
                Permission::File,
                Permission::Restart,
                Permission::Recording,
                Permission::BlockInput,
                Permission::PrivacyMode,
                Permission::Keyboard
            ]
        );
        // No clipboard was opened, and clipboard messages change nothing.
        assert!(screen.clipboards.lock().unwrap().is_empty());
        let text = Clipboard { content: Bytes::from_static(b"ignored"), ..Default::default() };
        client.send(Message { union: Some(message::Union::Clipboard(text)) }).await;
        // Without control the cursor's shape is of no use to the client,
        // until it asks to see the remote cursor.
        let (_, (events, _commands)) = take_pipeline(&screen);
        events.send(cursor(1, 1, Some(7))).await.unwrap();
        client.send(option(OptionMessage { show_remote_cursor: BoolOption::Yes as i32, ..Default::default() })).await;
        assert!(matches!(client.next_reply().await.union, Some(message::Union::CursorData(_))));
        client.send(mouse(0, 5, 5)).await;
        events.send(cursor(2, 3, None)).await.unwrap();
        assert_eq!(
            client.next_reply().await.union,
            Some(message::Union::CursorPosition(crate::rustdesk::proto::CursorPosition { x: 2, y: 3 }))
        );
        assert!(screen.injected().is_empty());
    }

    #[tokio::test]
    async fn mouse_and_keys_are_injected_only_while_control_is_on() {
        let (addr, screen) = start_host("password").await;
        let mut client = Client::connect(addr).await;
        // Before login: nothing.
        client.send(mouse(0, 1, 1)).await;
        client.send(map_key(38, true)).await;
        client.login(PASSWORD).await.expect("logged in");
        // Move, left button down and up (mask: type | button << 3), the key
        // of 'a' in map mode (X keycode 38).
        client.send(mouse(0, 10, 20)).await;
        client.send(mouse(1 | (1 << 3), 10, 20)).await;
        client.send(mouse(2 | (1 << 3), 10, 20)).await;
        client.send(map_key(38, true)).await;
        // A scan code on Windows, the kernel's key code on Linux; the
        // key-down stays down until the client's key-up.
        let position = if cfg!(target_os = "windows") { 38 } else { 30 };
        let mut expected = vec!["move_abs 10 20 of 1920x1080".to_string(), "button 1 down".into(), "button 1 up".into()];
        expected.push(format!("position {} down", position));
        assert_eq!(screen.injected_after(expected.len()).await, expected);
        client.send(map_key(38, false)).await;
        assert_eq!(screen.injected_after(1).await, [format!("position {} up", position)]);

        // The client's "view only" (`disable_keyboard`): input is dropped
        // until it is switched back.
        client.send(option(OptionMessage { disable_keyboard: BoolOption::Yes as i32, ..Default::default() })).await;
        client.send(mouse(0, 99, 99)).await;
        client.send(map_key(38, true)).await;
        client.send(option(OptionMessage { disable_keyboard: BoolOption::No as i32, ..Default::default() })).await;
        client.send(mouse(0, 30, 40)).await;
        assert_eq!(screen.injected_after(1).await, ["move_abs 30 40 of 1920x1080"]);

        // A modifier left down is released when the client goes away.
        let ctrl = if cfg!(target_os = "windows") { 0x1D } else { 37 };
        client.send(map_key(ctrl, true)).await;
        assert_eq!(screen.injected_after(1).await, ["position 29 down"]);
        drop(client);
        assert_eq!(screen.injected_after(1).await, ["position 29 up"]);
    }

    #[tokio::test]
    async fn clipboard_text_goes_both_ways_when_both_sides_allow_it() {
        let (addr, screen) = start_host("password").await;
        let mut client = Client::connect(addr).await;
        client.login(PASSWORD).await.expect("logged in");
        let (set, changed) = screen.clipboards.lock().unwrap().pop().expect("a clipboard was opened");

        // Client to host: the single message of older clients, and the
        // list of newer ones (text among other formats, compressed).
        let plain = Clipboard { content: Bytes::from_static(b"from the client"), ..Default::default() };
        client.send(Message { union: Some(message::Union::Clipboard(plain)) }).await;
        assert_eq!(clipboard_set(&set).await.as_deref(), Some("from the client"));
        let long = "Tiếng Việt ".repeat(100);
        let Some(message::Union::MultiClipboards(mut list)) = clipboard::text_message(&long, "1.5.3", "Linux").union else {
            panic!("expected MultiClipboards");
        };
        assert!(list.clipboards[0].compress);
        list.clipboards.insert(0, Clipboard { format: 22, content: Bytes::from_static(b"png"), ..Default::default() });
        client.send(Message { union: Some(message::Union::MultiClipboards(MultiClipboards { clipboards: list.clipboards })) }).await;
        assert_eq!(clipboard_set(&set).await.as_deref(), Some(long.as_str()));

        // Host to client.
        changed.send("Tiếng Việt".into()).unwrap();
        assert_eq!(clipboard_text(client.next_reply().await), "Tiếng Việt");

        // The client switches the clipboard off for itself: nothing either
        // way, until it switches it back on.
        client.send(option(OptionMessage { disable_clipboard: BoolOption::Yes as i32, ..Default::default() })).await;
        let ignored = Clipboard { content: Bytes::from_static(b"ignored"), ..Default::default() };
        client.send(Message { union: Some(message::Union::Clipboard(ignored)) }).await;
        // (Read something to be sure the option arrived before the change.)
        client.send(Message { union: Some(message::Union::TestDelay(TestDelay { time: 1, from_client: true, ..Default::default() })) }).await;
        loop {
            if let Some(message::Union::TestDelay(t)) = client.next().await.union {
                if t.from_client {
                    break;
                }
            }
        }
        changed.send("not sent".into()).unwrap();
        client.send(option(OptionMessage { disable_clipboard: BoolOption::No as i32, ..Default::default() })).await;
        client.send(Message { union: Some(message::Union::TestDelay(TestDelay { time: 2, from_client: true, ..Default::default() })) }).await;
        loop {
            if let Some(message::Union::TestDelay(t)) = client.next().await.union {
                if t.from_client {
                    break;
                }
            }
        }
        changed.send("sent again".into()).unwrap();
        assert_eq!(clipboard_text(client.next_reply().await), "sent again");
        assert_eq!(clipboard_set(&set).await, None);
    }

    #[tokio::test]
    async fn the_cursor_is_sent_as_a_shape_once_then_by_its_id() {
        let (addr, screen) = start_host("password").await;
        let mut client = Client::connect(addr).await;
        client.login(PASSWORD).await.expect("logged in");
        let (_, (events, _commands)) = take_pipeline(&screen);
        events.send(cursor(5, 6, Some(7))).await.unwrap();
        let Some(message::Union::CursorData(first)) = client.next_reply().await.union else { panic!("expected CursorData") };
        assert_eq!((first.width, first.height, first.hotx, first.hoty), (2, 2, 0, 1));
        assert_eq!(zstd::bulk::decompress(&first.colors, 16).unwrap(), vec![7u8; 16]);
        // Moves without a new shape say nothing unless the client shows the
        // remote cursor.
        events.send(cursor(8, 8, None)).await.unwrap();
        events.send(cursor(8, 9, Some(8))).await.unwrap();
        let Some(message::Union::CursorData(second)) = client.next_reply().await.union else { panic!("expected CursorData") };
        assert_ne!(second.id, first.id);
        events.send(cursor(8, 9, Some(7))).await.unwrap();
        assert_eq!(client.next_reply().await.union, Some(message::Union::CursorId(first.id)));

        client.send(option(OptionMessage { show_remote_cursor: BoolOption::Yes as i32, ..Default::default() })).await;
        client.send(Message { union: Some(message::Union::TestDelay(TestDelay { time: 1, from_client: true, ..Default::default() })) }).await;
        loop {
            if let Some(message::Union::TestDelay(t)) = client.next().await.union {
                if t.from_client {
                    break;
                }
            }
        }
        events.send(cursor(100, 200, None)).await.unwrap();
        assert_eq!(
            client.next_reply().await.union,
            Some(message::Union::CursorPosition(crate::rustdesk::proto::CursorPosition { x: 100, y: 200 }))
        );
    }

    #[tokio::test]
    async fn sound_is_the_format_then_opus_packets() {
        let (addr, screen) = start_host("password").await;
        let mut client = Client::connect(addr).await;
        client.login(PASSWORD).await.expect("logged in");
        let (_, (events, _commands)) = take_pipeline(&screen);
        events.send(audio(1)).await.unwrap();
        events.send(audio(2)).await.unwrap();
        assert_eq!(
            client.next_reply().await,
            misc_message(misc::Union::AudioFormat(AudioFormat { sample_rate: 48_000, channels: 2 }))
        );
        for fill in [1u8, 2] {
            let Some(message::Union::AudioFrame(frame)) = client.next_reply().await.union else { panic!("expected AudioFrame") };
            assert_eq!(&frame.data[..], &[fill; 40]);
        }
        // Muted by the client, then on again: the format is told again.
        client.send(option(OptionMessage { disable_audio: BoolOption::Yes as i32, ..Default::default() })).await;
        client.send(Message { union: Some(message::Union::TestDelay(TestDelay { time: 1, from_client: true, ..Default::default() })) }).await;
        loop {
            if let Some(message::Union::TestDelay(t)) = client.next().await.union {
                if t.from_client {
                    break;
                }
            }
        }
        events.send(audio(3)).await.unwrap();
        client.send(option(OptionMessage { disable_audio: BoolOption::No as i32, ..Default::default() })).await;
        client.send(Message { union: Some(message::Union::TestDelay(TestDelay { time: 2, from_client: true, ..Default::default() })) }).await;
        loop {
            if let Some(message::Union::TestDelay(t)) = client.next().await.union {
                if t.from_client {
                    break;
                }
            }
        }
        events.send(audio(4)).await.unwrap();
        assert!(matches!(client.next_reply().await.union, Some(message::Union::Misc(_))));
        let Some(message::Union::AudioFrame(frame)) = client.next_reply().await.union else { panic!("expected AudioFrame") };
        assert_eq!(&frame.data[..], &[4u8; 40]);
    }

    #[tokio::test]
    async fn what_is_not_offered_inside_a_session_is_answered_and_never_hangs() {
        use crate::rustdesk::proto::{ChatMessage, ScreenshotRequest, TogglePrivacyMode, VoiceCallRequest};
        let (addr, screen) = start_host("password").await;
        let mut client = Client::connect(addr).await;
        client.login(PASSWORD).await.expect("logged in");
        let (_, (events, _commands)) = take_pipeline(&screen);
        let notification = |msg: Message| match msg.union {
            Some(message::Union::Misc(Misc { union: Some(misc::Union::BackNotification(n)) })) => n,
            other => panic!("expected BackNotification, got {:?}", other),
        };
        // Privacy mode: denied.
        client
            .send(misc_message(misc::Union::TogglePrivacyMode(TogglePrivacyMode { impl_key: "k".into(), on: true })))
            .await;
        let n = notification(client.next_reply().await);
        assert_eq!(n.union, Some(back_notification::Union::PrivacyModeState(PrivacyModeState::PrvOnFailedDenied as i32)));
        assert_eq!(n.impl_key, "k");
        // Blocking the host's own input: no permission.
        client.send(option(OptionMessage { block_input: BoolOption::Yes as i32, ..Default::default() })).await;
        let n = notification(client.next_reply().await);
        assert_eq!(n.union, Some(back_notification::Union::BlockInputState(BlockInputState::BlkOnFailed as i32)));
        assert_eq!(n.details, "No permission");
        // A voice call: declined.
        client
            .send(Message {
                union: Some(message::Union::VoiceCallRequest(VoiceCallRequest { req_timestamp: 77, is_connect: true })),
            })
            .await;
        match client.next_reply().await.union {
            Some(message::Union::VoiceCallResponse(r)) => assert!(!r.accepted && r.req_timestamp == 77),
            other => panic!("expected VoiceCallResponse, got {:?}", other),
        }
        // A screenshot: an error text.
        client
            .send(Message {
                union: Some(message::Union::ScreenshotRequest(ScreenshotRequest { display: 0, sid: "s".into() })),
            })
            .await;
        match client.next_reply().await.union {
            Some(message::Union::ScreenshotResponse(r)) => assert!(r.sid == "s" && !r.msg.is_empty() && r.data.is_empty()),
            other => panic!("expected ScreenshotResponse, got {:?}", other),
        }
        // Restart, chat and switching sides: nothing happens, the session
        // goes on.
        client.send(misc_message(misc::Union::RestartRemoteDevice(true))).await;
        client.send(misc_message(misc::Union::ChatMessage(ChatMessage { text: "hello".into() }))).await;
        client.send(misc_message(misc::Union::SwitchSidesRequest(Default::default()))).await;
        events.send(frame(true, 1)).await.unwrap();
        assert!(h264(client.next_reply().await).1.key);
    }

    #[tokio::test]
    async fn click_mode_asks_the_person_here() {
        // Accepted: the client is told to wait, then let in.
        let screen = FakeScreen::default();
        screen.answers.lock().unwrap().push(Some(Decision::Accepted));
        let (addr, screen) = start_host_with("click", true, screen).await;
        let mut client = Client::connect(addr).await;
        assert_eq!(client.login("whatever").await.unwrap_err(), login::LOGIN_MSG_NO_PASSWORD_ACCESS);
        match client.next_reply().await.union {
            Some(message::Union::LoginResponse(LoginResponse { union: Some(login_response::Union::PeerInfo(_)), .. })) => {}
            other => panic!("expected PeerInfo, got {:?}", other),
        }
        let asked = screen.asked.lock().unwrap().clone();
        assert_eq!(asked.len(), 1);
        assert!(asked[0].contains("Tester") && asked[0].contains("127.0.0.1"), "{}", asked[0]);
        drop(client);

        // Declined: closed with RustDesk's reason for it.
        screen.answers.lock().unwrap().push(Some(Decision::Denied));
        let mut client = Client::connect(addr).await;
        let mut tries = 0;
        // (The first viewer's place is given back a moment after it left.)
        while client.login("whatever").await.unwrap_err() != login::LOGIN_MSG_NO_PASSWORD_ACCESS {
            tries += 1;
            assert!(tries < 5);
        }
        assert_eq!(client.next_reply().await.union, misc_message(misc::Union::CloseReason(CLOSED_MANUALLY.into())).union);
    }

    #[tokio::test]
    async fn password_click_mode_lets_in_by_either() {
        // No password typed yet, and the person here accepts.
        let screen = FakeScreen::default();
        screen.answers.lock().unwrap().push(Some(Decision::Accepted));
        let (addr, screen) = start_host_with("password-click", true, screen).await;
        let mut client = Client::connect(addr).await;
        let empty = client.login_request("", true, 0);
        client.login_with(empty).await.expect("accepted at the host");
        drop(client);

        // The question is up and unanswered; the password still works, and
        // only one question was asked for the connection.
        let mut client = Client::connect(addr).await;
        let empty = client.login_request("", true, 0);
        client.send(empty.clone()).await;
        client.send(empty).await;
        let mut tries = 0;
        while let Err(e) = client.login(PASSWORD).await {
            tries += 1;
            assert!(tries < 50, "still refused: {}", e);
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        assert_eq!(screen.asked.lock().unwrap().len(), 2);
        // "password" mode never asks.
        let (addr, screen) = start_host("password").await;
        let mut client = Client::connect(addr).await;
        let empty = client.login_request("", true, 0);
        client.send(empty).await;
        client.login(PASSWORD).await.expect("logged in");
        assert!(screen.asked.lock().unwrap().is_empty());
    }

    #[test]
    fn addresses_are_told_from_ids_as_rustdesk_does() {
        for peer in ["192.168.1.20", "192.168.1.20:21118", "fe80::1", "[fe80::1]:21118", "host.example.com:21118"] {
            assert!(is_direct_ip_access(peer), "{}", peer);
        }
        for peer in ["123456789", "host.example.com", "my-pc", ""] {
            assert!(!is_direct_ip_access(peer), "{}", peer);
        }
    }

    #[test]
    fn frames_are_message_bytes_behind_the_length_header() {
        let msg = video_message(Codec::H264, 0, vec![0xAB; 100], true, 7);
        let bytes = encode(&msg);
        let mut buf = BytesMut::from(&bytes[..]);
        let payload = frame::Decoder::new(usize::MAX).decode(&mut buf).unwrap().unwrap();
        assert!(buf.is_empty());
        assert_eq!(Message::decode(payload.freeze()).unwrap(), msg);
    }

    // --- the RustDesk connector (the agent as a client) against this listener ---

    fn connector_target(addr: SocketAddr, password: &str) -> crate::connector::Target {
        crate::connector::Target {
            connector: "rustdesk".into(),
            values: [("host", addr.ip().to_string()), ("port", addr.port().to_string()), ("password", password.to_string())]
                .into_iter()
                .map(|(key, value)| (key.to_string(), value))
                .collect(),
        }
    }

    async fn connector_event(session: &mut crate::connector::Session) -> crate::connector::Event {
        tokio::time::timeout(Duration::from_secs(5), session.events.recv()).await.expect("an event in time").expect("the session is open")
    }

    /// The delay the viewer adds to its ping, against this agent's own
    /// RustDesk host on the loopback: told within the first probes, and a
    /// fraction of a millisecond there (on Linux the connection's own round
    /// trip; elsewhere the host's figure, 0 or 1 ms).
    #[tokio::test]
    async fn the_connector_tells_the_round_trip_to_a_rustdesk_host() {
        use crate::connector::{Codec as Wanted, Connector, Event, Viewer};
        let (addr, _screen) = start_host_with("password", true, FakeScreen::default()).await;
        let connector = crate::connector::rustdesk::RustdeskConnector;
        let viewer = Viewer { codecs: vec![Wanted::H264], fps: 60, width: 0, height: 0, bitrate_kbps: 0 };
        let mut session = connector.connect(&connector_target(addr, PASSWORD), &viewer).await.expect("logged in");
        let started = Instant::now();
        let mut told = Vec::new();
        while started.elapsed() < Duration::from_millis(3_500) {
            if let Ok(Some(Event::Latency { ms })) = tokio::time::timeout(Duration::from_millis(200), session.events.recv()).await {
                told.push(ms);
            }
        }
        println!("round trips to the RustDesk host on the loopback: {:?} ms", told);
        assert!(!told.is_empty());
        assert!(told.iter().all(|ms| (0.0..5.0).contains(ms)), "{told:?}");
    }

    #[tokio::test]
    async fn the_connector_gets_the_picture_and_the_sound_of_a_rustdesk_host() {
        use crate::connector::{Codec as Wanted, Command, Connector, Event, Viewer};
        let (addr, screen) = start_host_with("password", true, FakeScreen { h265: true, ..Default::default() }).await;
        let connector = crate::connector::rustdesk::RustdeskConnector;
        let viewer = Viewer { codecs: vec![Wanted::H265, Wanted::H264], fps: 60, width: 0, height: 0, bitrate_kbps: 0 };
        let mut session = connector.connect(&connector_target(addr, PASSWORD), &viewer).await.expect("logged in");

        // What kind of computer it is, then the displays of the PeerInfo.
        match connector_event(&mut session).await {
            Event::Platform { os } => assert!(["linux", "windows", "macos"].contains(&os.as_str()), "{}", os),
            other => panic!("the platform first, not {:?}", other),
        }
        match connector_event(&mut session).await {
            Event::Displays { displays, current } => {
                assert_eq!(current, 0);
                assert_eq!((displays[0].width, displays[0].height), (1920, 1080));
            }
            other => panic!("displays first, not {:?}", other),
        }
        // The host encodes what the viewer prefers and can take.
        let (_, (events, mut commands)) = restarted(&screen).await;
        assert_eq!(screen.starts.lock().unwrap().last().unwrap().1, Codec::H265);

        events.send(frame(true, 7)).await.unwrap();
        events.send(audio(9)).await.unwrap();
        events.send(frame(false, 8)).await.unwrap();
        let mut video = Vec::new();
        let mut sound = Vec::new();
        while video.len() < 2 || sound.is_empty() {
            match connector_event(&mut session).await {
                Event::Video { codec, key, data, capture_time_ms } => {
                    assert_eq!(codec, Wanted::H265);
                    assert!(capture_time_ms.is_some());
                    video.push((key, data.to_vec()));
                }
                Event::Audio { channels, data } => sound.push((channels, data.to_vec())),
                // Whenever the host's second delay probe happens to come.
                Event::Latency { .. } => {}
                other => panic!("unexpected {:?}", other),
            }
        }
        assert_eq!(video, vec![(true, vec![0, 0, 0, 1, 7, 7]), (false, vec![0, 0, 0, 1, 8, 8])]);
        assert_eq!(sound, vec![(2, vec![9; 40])]);

        // A key frame asked for reaches the host's encoder (the client's refresh).
        while commands.try_recv().is_ok() {}
        session.commands.send(Command::RequestKeyFrame).await.unwrap();
        loop {
            if matches!(next_command(&mut commands).await, PipelineCommand::RequestKeyframe) {
                break;
            }
        }
        // The frame rate wanted goes as the client's custom_fps, a little
        // raised (a RustDesk host delivers about 97 % of what it is asked for).
        session.commands.send(Command::Quality { fps: 30, bitrate_kbps: 0 }).await.unwrap();
        loop {
            if matches!(next_command(&mut commands).await, PipelineCommand::SetFrameLimit(31)) {
                break;
            }
        }
        // The viewer's input reaches the computer shown (here: the fake
        // host's injector), as the RustDesk client sends it.
        assert!(screen.injected().is_empty());
        for command in [
            Command::PointerMove { x: 30, y: 40 },
            Command::PointerButton { button: crate::connector::Button::Left, down: true },
            Command::PointerButton { button: crate::connector::Button::Left, down: false },
            Command::Wheel { dx: 0, dy: -1 },
            Command::Key { position: 30, down: true },
            Command::Key { position: 30, down: false },
        ] {
            session.commands.send(command).await.unwrap();
        }
        let injected = screen.injected_after(6).await;
        assert_eq!(injected[0], "move_abs 30 40 of 1920x1080", "{:?}", injected);
        assert!(injected.iter().filter(|line| line.starts_with("button")).count() >= 3, "{:?}", injected);
        // Map mode: the key at the same place of the keyboard (the test
        // host is this platform; the connector sends what a Linux client does).
        if cfg!(target_os = "linux") {
            assert!(injected.contains(&"position 30 down".to_string()) && injected.contains(&"position 30 up".to_string()), "{:?}", injected);
        }
        // Text typed as it is.
        session.commands.send(Command::Text { text: "ô".into() }).await.unwrap();
        assert!(!screen.injected_after(1).await.is_empty());

        // The clipboard, both ways.
        let (set, changed) = screen.clipboards.lock().unwrap().pop().expect("a clipboard was opened");
        session.commands.send(Command::Clipboard { text: "from the viewer".into() }).await.unwrap();
        assert_eq!(clipboard_set(&set).await.as_deref(), Some("from the viewer"));
        changed.send("from the computer".into()).unwrap();
        // The cursor's shape.
        events.send(cursor(5, 6, Some(7))).await.unwrap();
        let (mut clipboard_text, mut shape) = (None, None);
        while clipboard_text.is_none() || shape.is_none() {
            match connector_event(&mut session).await {
                Event::Clipboard { text } => clipboard_text = Some(text),
                Event::CursorShape { width, height, hot_x, hot_y, rgba } => shape = Some((width, height, hot_x, hot_y, rgba.to_vec())),
                Event::CursorPosition { .. } => {}
                // The host's delay probes, answered: the round trip it
                // measured (under a millisecond on the loopback).
                Event::Latency { ms } => {
                    println!("round trip to the RustDesk host on the loopback: {} ms", ms);
                    assert!((0.0..50.0).contains(&ms), "{} ms", ms)
                }
                other => panic!("unexpected {:?}", other),
            }
        }
        assert_eq!(clipboard_text.as_deref(), Some("from the computer"));
        assert_eq!(shape, Some((2, 2, 0, 1, vec![7; 16])));

        // The viewer's bitrate becomes the client's image quality (once it
        // differs enough), which the host's encoder follows.
        session.commands.send(Command::Quality { fps: 0, bitrate_kbps: 1500 }).await.unwrap();

        session.commands.send(Command::Close).await.unwrap();
        loop {
            if let Event::Closed { reason } = connector_event(&mut session).await {
                assert_eq!(reason, "Closed");
                break;
            }
        }
        // The host stops its pipeline when the client has left.
        loop {
            if matches!(next_command(&mut commands).await, PipelineCommand::Stop) {
                break;
            }
        }
    }

    #[tokio::test]
    async fn the_connector_tells_the_viewer_once_when_the_host_keeps_its_clipboard() {
        use crate::connector::{Codec as Wanted, Command, Connector, Event, Viewer};
        // The host does not share its clipboard (it says so before its LoginResponse).
        let (addr, screen) = start_host_with("password", false, FakeScreen::default()).await;
        let connector = crate::connector::rustdesk::RustdeskConnector;
        let viewer = Viewer { codecs: vec![Wanted::H264], fps: 60, width: 0, height: 0, bitrate_kbps: 0 };
        let mut session = connector.connect(&connector_target(addr, PASSWORD), &viewer).await.expect("logged in");
        session.commands.send(Command::Clipboard { text: "first".into() }).await.unwrap();
        session.commands.send(Command::Clipboard { text: "second".into() }).await.unwrap();
        // A pointer move after them, to know both were dealt with.
        session.commands.send(Command::PointerMove { x: 1, y: 1 }).await.unwrap();
        let mut told = 0;
        let injected = loop {
            match tokio::time::timeout(Duration::from_millis(600), session.events.recv()).await {
                Ok(Some(Event::ClipboardError { text })) => {
                    assert!(text.contains("does not allow its clipboard"), "{}", text);
                    told += 1;
                }
                Ok(Some(_)) => {}
                Ok(None) => panic!("the session ended"),
                Err(_) => break screen.injected(),
            }
        };
        assert_eq!(told, 1, "told once, not for every text");
        assert!(!injected.is_empty(), "the pointer still works");
        // The host opened no clipboard for this client.
        assert!(screen.clipboards.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn the_connector_says_when_the_password_is_wrong_or_nobody_is_there() {
        use crate::connector::{Codec as Wanted, Connector, Viewer};
        let (addr, _screen) = start_host("password").await;
        let connector = crate::connector::rustdesk::RustdeskConnector;
        let viewer = Viewer { codecs: vec![Wanted::H264], fps: 60, width: 0, height: 0, bitrate_kbps: 0 };
        let error = connector.connect(&connector_target(addr, "not the password"), &viewer).await.err().expect("refused");
        assert_eq!(error.to_string(), "Wrong password for the RustDesk computer");

        // A port nobody listens on.
        let free = TcpListener::bind("127.0.0.1:0").await.unwrap().local_addr().unwrap();
        let error = connector.connect(&connector_target(free, PASSWORD), &viewer).await.err().expect("refused");
        assert!(error.to_string().contains("cannot be reached"), "{}", error);

        // The target's password is never in what is printed of it.
        let target = connector_target(addr, "s3cret-value");
        assert!(!format!("{:?}", target).contains("s3cret-value"));
    }
}
