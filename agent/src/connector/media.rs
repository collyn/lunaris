//! A connector's session as the source of an agent session: its picture
//! and sound go to the RTP packager as they came (no decoding, no
//! encoding), its cursor and clipboard to the viewer, and the viewer's
//! input and the session's requests (key frame, frame rate, bitrate, stop)
//! to the connector. Nothing of the viewer's reaches this computer's own
//! screen, input or clipboard.
//!
//! The environment variables below are for tests only; this is the one
//! place they are described.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use tokio::sync::mpsc;
use tracing::{debug, info};

use super::{Button, Codec, Command, DisplayInfo, Event, Session, SourceError, Target, Viewer};
use crate::bridge::VideoFramePayload;
use crate::input::{InboundPacket, KeyAction, MouseButton, MouseButtonAction};
use lunaris_media::pipeline::{MediaEvent, PipelineCommand};

/// Test only: the path of a JSON file with one target (`{"connector":
/// "rustdesk", "host": ..., "password": ...}`). An agent started with it
/// shows that target instead of its own screen, and injects no input.
pub const TEST_TARGET_ENV: &str = "LUNARIS_CONNECTOR_TEST_TARGET";
/// Test only: a folder to look for external connectors in, instead of
/// `connectors/` beside the agent's config.
pub const TEST_DIR_ENV: &str = "LUNARIS_CONNECTOR_TEST_DIR";

/// Test only: when set, an agent session never opens this computer's own
/// input injector (a test that must not move the user's pointer whatever
/// goes wrong sets it).
pub const TEST_NO_LOCAL_INPUT_ENV: &str = "LUNARIS_TEST_NO_LOCAL_INPUT";
/// Test only: ask the connector for a key frame every so many
/// milliseconds, to measure how long one takes.
pub const TEST_KEY_FRAME_EVERY_ENV: &str = "LUNARIS_CONNECTOR_TEST_KEY_FRAME_EVERY_MS";

/// Test only: the connector answers the other computer's delay probes this
/// many milliseconds late (to see what that computer's rate control does).
pub const TEST_DELAY_REPLY_ENV: &str = "LUNARIS_CONNECTOR_TEST_DELAY_REPLY_MS";

/// The kind of computer the source shown now is ("windows", ...), while a
/// source is shown: what a viewer is told as the host's when it asks.
static SOURCE_OS: Mutex<Option<String>> = Mutex::new(None);

/// Whether the source's pointer may be part of its picture now (see
/// `Event::CursorMaybeEmbedded`); goes out with the cursor messages.
static POINTER_MAYBE_IN_PICTURE: AtomicBool = AtomicBool::new(false);

pub fn pointer_maybe_in_picture() -> bool {
    POINTER_MAYBE_IN_PICTURE.load(Ordering::Relaxed)
}

/// Whether the source's pointer is part of its picture for the whole
/// session (see `Event::CursorAlwaysEmbedded`).
static POINTER_ALWAYS_IN_PICTURE: AtomicBool = AtomicBool::new(false);

/// `pointer_in_picture` of the cursor messages: `Some("always")` while a
/// source is shown whose pointer is always part of its picture (the viewer
/// then shows no pointer over the picture), else `None` (the field is null).
pub fn pointer_in_picture() -> Option<&'static str> {
    POINTER_ALWAYS_IN_PICTURE.load(Ordering::Relaxed).then_some("always")
}

/// `Event::CursorAlwaysEmbedded`: noted for the cursor messages of this
/// session. Nothing is known of the pointer but that it is in the picture,
/// so the cursor of the message that carries this is not visible: a viewer
/// from before `pointer_in_picture` then draws no pointer at a place nobody
/// told (it still draws its own where the person's mouse is).
fn note_pointer_always_in_picture(cursor: &mut lunaris_media::types::CursorState) {
    POINTER_ALWAYS_IN_PICTURE.store(true, Ordering::Relaxed);
    cursor.visible = false;
}

/// Whether a source is being shown now (the session's picture is another
/// computer's).
static SOURCE_ACTIVE: AtomicBool = AtomicBool::new(false);

pub fn source_active() -> bool {
    SOURCE_ACTIVE.load(Ordering::Relaxed)
}

/// The platform of the source being shown, if one is.
pub fn source_os() -> Option<String> {
    SOURCE_OS.lock().unwrap().clone()
}

/// What a session's picture did, in numbers, for the log line at its end.
#[derive(Default)]
struct Counters {
    received: u64,
    forwarded: u64,
    dropped: u64,
    keys: u64,
    key_requests: u64,
    /// Frames in each whole second since the first.
    per_second: Vec<u32>,
    second_started: Option<Instant>,
    in_second: u32,
    last_arrival: Option<Instant>,
    last_capture_ms: Option<u64>,
    /// Gaps between arrivals, in tenths of a millisecond.
    gaps: Vec<u32>,
    /// The largest difference between the gap by the source's own times
    /// and the gap between arrivals (ms).
    worst_time_mismatch_ms: i64,
    first_capture_ms: Option<u64>,
    /// Whether the RTP times are (still) made of the source's own times.
    capture_times_used: bool,
    /// Pictures kept from before the viewer's connection was up, and sent.
    early: u64,
    /// The bytes of the frames, and of those how many were filler data
    /// (bytes a constant-bitrate encoder pads with; they are not sent on).
    bytes: u64,
    filler_bytes: u64,
    /// The viewer's input events and the commands made of them (counts).
    input_events: u64,
    input_commands: u64,
}

/// The line is written when the session's task ends, however it ends (the
/// agent stops the task when the viewer leaves).
impl Drop for Counters {
    fn drop(&mut self) {
        SOURCE_ACTIVE.store(false, Ordering::Relaxed);
        POINTER_MAYBE_IN_PICTURE.store(false, Ordering::Relaxed);
        POINTER_ALWAYS_IN_PICTURE.store(false, Ordering::Relaxed);
        *SOURCE_OS.lock().unwrap() = None;
        info!("Connector session: {}; input: {} events, {} commands", self.summary(self.capture_times_used), self.input_events, self.input_commands);
    }
}

impl Counters {
    fn frame(&mut self, now: Instant, capture_time_ms: Option<u64>, key: bool) {
        self.received += 1;
        self.keys += key as u64;
        match self.second_started {
            None => self.second_started = Some(now),
            Some(started) if now.duration_since(started) >= Duration::from_secs(1) => {
                self.per_second.push(self.in_second);
                self.in_second = 0;
                // Whole seconds without a frame count as such.
                let mut next = started + Duration::from_secs(1);
                while now.duration_since(next) >= Duration::from_secs(1) && self.per_second.len() < 100_000 {
                    self.per_second.push(0);
                    next += Duration::from_secs(1);
                }
                self.second_started = Some(next);
            }
            Some(_) => {}
        }
        self.in_second += 1;
        if let Some(last) = self.last_arrival {
            let gap = now.duration_since(last);
            if self.gaps.len() < 1_000_000 {
                self.gaps.push((gap.as_micros() / 100).min(u32::MAX as u128) as u32);
            }
            if let (Some(capture), Some(last_capture)) = (capture_time_ms, self.last_capture_ms) {
                let mismatch = (capture as i64 - last_capture as i64) - gap.as_millis() as i64;
                self.worst_time_mismatch_ms = self.worst_time_mismatch_ms.max(mismatch.abs());
            }
        }
        self.last_arrival = Some(now);
        self.last_capture_ms = capture_time_ms.or(self.last_capture_ms);
        self.first_capture_ms = self.first_capture_ms.or(capture_time_ms);
    }

    /// One line of numbers: nothing of what was shown or typed.
    fn summary(&self, capture_times_used: bool) -> String {
        let mut seconds = self.per_second.clone();
        seconds.sort_unstable();
        let mut gaps = self.gaps.clone();
        gaps.sort_unstable();
        let gap = |p: f64| gaps.get(((gaps.len() as f64 * p) as usize).min(gaps.len().saturating_sub(1))).copied().unwrap_or(0) as f64 / 10.0;
        let by_source = match (self.first_capture_ms, self.last_capture_ms) {
            (Some(first), Some(last)) if last > first && self.received > 1 => format!("{:.1}", (self.received - 1) as f64 * 1000.0 / (last - first) as f64),
            _ => "-".to_string(),
        };
        format!(
            "frames: {} received, {} forwarded, {} dropped at the packager, {} key, {} key frames asked for, {} kept from before the connection; {} bytes a frame on average, {} % of the bytes filler; frames per second min {} median {} max {} over {} s ({} by the source's own times); gap between frames ms median {:.1} p95 {:.1} p99 {:.1} max {:.1}; source times {} (worst mismatch with arrival {} ms)",
            self.received,
            self.forwarded,
            self.dropped,
            self.keys,
            self.key_requests,
            self.early,
            self.bytes.checked_div(self.received).unwrap_or(0),
            (self.filler_bytes * 100).checked_div(self.bytes).unwrap_or(0),
            seconds.first().copied().unwrap_or(0),
            seconds.get(seconds.len() / 2).copied().unwrap_or(0),
            seconds.last().copied().unwrap_or(0),
            seconds.len(),
            by_source,
            gap(0.5),
            gap(0.95),
            gap(0.99),
            gaps.last().copied().unwrap_or(0) as f64 / 10.0,
            if capture_times_used { "used for RTP" } else { "not used (arrival times)" },
            self.worst_time_mismatch_ms
        )
    }
}

/// How long a frame waits for room in the packager's queue before it is
/// dropped (and a key frame asked for).
const PACKAGER_WAIT: Duration = Duration::from_millis(50);

/// How long the first picture may take after the login.
const FIRST_FRAME_TIMEOUT: Duration = Duration::from_secs(15);

/// The test target, when the variable names one. A file that cannot be
/// read is an error, never "no target": the session must not fall back to
/// this computer's own screen and input.
pub fn test_target() -> anyhow::Result<Option<Target>> {
    let Some(path) = std::env::var(TEST_TARGET_ENV).ok().filter(|p| !p.is_empty()) else { return Ok(None) };
    let text = std::fs::read_to_string(&path).map_err(|e| anyhow::anyhow!("{}={}: {}", TEST_TARGET_ENV, path, e))?;
    let target = serde_json::from_str::<Target>(&text).map_err(|e| anyhow::anyhow!("{}={}: {}", TEST_TARGET_ENV, path, e))?;
    Ok(Some(target))
}

fn connectors_dir() -> std::path::PathBuf {
    std::env::var(TEST_DIR_ENV).ok().filter(|p| !p.is_empty()).map(Into::into).unwrap_or_else(super::connectors_dir)
}

pub fn codec_from_name(name: &str) -> Option<Codec> {
    Some(match name {
        "h264" => Codec::H264,
        "h265" => Codec::H265,
        "vp8" => Codec::Vp8,
        "vp9" => Codec::Vp9,
        "av1" => Codec::Av1,
        _ => return None,
    })
}

pub fn codec_name(codec: Codec) -> &'static str {
    match codec {
        Codec::H264 => "h264",
        Codec::H265 => "h265",
        Codec::Vp8 => "vp8",
        Codec::Vp9 => "vp9",
        Codec::Av1 => "av1",
    }
}

/// What the connector is told the viewer takes: the codec asked for
/// first, then the others it decodes. A client from before it said what it
/// decodes (`decodes` None) is taken to decode what every WebRTC browser
/// does, and H.265 only when it asked for it.
pub fn viewer_codecs(requested: &str, decodes: Option<&[String]>) -> Vec<Codec> {
    let mut codecs = Vec::new();
    let known: Vec<Codec> = match decodes {
        Some(names) => names.iter().filter_map(|name| codec_from_name(&name.to_lowercase())).collect(),
        None => vec![Codec::H264, Codec::Vp9, Codec::Vp8, Codec::Av1],
    };
    if let Some(first) = codec_from_name(requested) {
        if decodes.is_none() || known.contains(&first) {
            codecs.push(first);
        }
    }
    for codec in known {
        if !codecs.contains(&codec) {
            codecs.push(codec);
        }
    }
    codecs
}

/// A target logged in to, with its first picture: the codec is known.
pub struct Source {
    pub codec: Codec,
    session: Session,
    first: Option<Event>,
    pending: Vec<Event>,
    /// While the session lasts the computer counts as there (`reachability`).
    in_use: super::reachability::InUse,
}

/// Logs in to `target` and waits for the first picture. The codec of an
/// agent session is fixed when it is offered to the viewer, and which codec
/// a computer sends is only known when it sends: so this comes first.
/// Every failure is a [`SourceError`] with words for the viewer.
pub async fn open(
    target: &Target,
    requested_codec: &str,
    decodes: Option<&[String]>,
    fps: u32,
    // The size (pixels) and the bitrate (kbit/s) the viewer asked for; 0 = no wish.
    size: (u32, u32),
    bitrate_kbps: u32,
) -> Result<Source, SourceError> {
    let fail = |e: anyhow::Error| SourceError(e.to_string());
    let connector = super::find(&target.connector, &connectors_dir()).map_err(fail)?;
    let viewer = Viewer { codecs: viewer_codecs(requested_codec, decodes), fps, width: size.0, height: size.1, bitrate_kbps };
    if viewer.codecs.is_empty() {
        return Err(SourceError("This viewer decodes no video the computer could send".into()));
    }
    info!("Connector {}: connecting ({:?}) for a viewer that takes {:?}", target.connector, target, viewer.codecs);
    let mut session = connector.connect(target, &viewer).await.map_err(fail)?;
    let in_use = super::reachability::in_use(target, Some(&connector.describe()));
    let mut pending = Vec::new();
    let deadline = tokio::time::Instant::now() + FIRST_FRAME_TIMEOUT;
    loop {
        let event = tokio::time::timeout_at(deadline, session.events.recv())
            .await
            .map_err(|_| SourceError("The computer sent no picture".into()))?
            .ok_or_else(|| SourceError("The connector ended".into()))?;
        match event {
            Event::Video { codec, .. } => {
                if !viewer.codecs.contains(&codec) {
                    return Err(SourceError(format!("The computer sends {:?} video, which this viewer cannot show", codec)));
                }
                info!("Connector {}: the computer sends {:?}", target.connector, codec);
                return Ok(Source { codec, session, first: Some(event), pending, in_use });
            }
            Event::Closed { reason } => return Err(SourceError(reason)),
            // Sound before the first picture is of no use.
            Event::Audio { .. } => {}
            other => pending.push(other),
        }
    }
}

/// RTP time (90 kHz) of the frames of one session. A connector's capture
/// time is used while it is sane: moving forward, and in step with the
/// clock here (a computer whose times jump, stand still or run at another
/// speed gets the times of arrival instead, for the rest of the session).
pub struct Timeline {
    start: Instant,
    /// (capture time, arrival) of the first frame.
    origin: Option<(u64, Duration)>,
    last_capture_ms: u64,
    use_capture: bool,
}

/// How far a capture time may be from where the arrival clock puts it.
const MAX_CAPTURE_DRIFT_MS: i64 = 1_000;

impl Timeline {
    pub fn new(start: Instant) -> Self {
        Timeline { start, origin: None, last_capture_ms: 0, use_capture: true }
    }

    /// Whether the source's own times are (still) what the RTP times are made of.
    pub fn uses_capture_times(&self) -> bool {
        self.use_capture && self.origin.is_some()
    }

    /// The RTP time of a frame that arrived at `now`.
    pub fn rtp(&mut self, capture_time_ms: Option<u64>, now: Instant) -> u32 {
        let arrival = now.saturating_duration_since(self.start);
        let by_arrival = (arrival.as_micros() * 90 / 1000) as u32;
        let Some(capture) = capture_time_ms.filter(|_| self.use_capture) else {
            self.use_capture = false;
            return by_arrival;
        };
        let (first_capture, first_arrival) = *self.origin.get_or_insert((capture, arrival));
        let media_ms = capture as i64 - first_capture as i64;
        let clock_ms = (arrival.as_millis() as i64) - (first_arrival.as_millis() as i64);
        if capture < self.last_capture_ms || (media_ms - clock_ms).abs() > MAX_CAPTURE_DRIFT_MS {
            debug!("Connector: capture times are not usable ({} ms against {} ms of the clock); using arrival times", media_ms, clock_ms);
            self.use_capture = false;
            return by_arrival;
        }
        self.last_capture_ms = capture;
        ((first_arrival.as_micros() as u64 * 90 / 1000) as u32).wrapping_add((media_ms as u64 * 90) as u32)
    }
}

/// The length of an Opus packet (RFC 6716, section 3.1: the TOC byte gives
/// the frame size, its low two bits the number of frames).
pub fn opus_duration(packet: &[u8]) -> Option<Duration> {
    let toc = *packet.first()?;
    let config = toc >> 3;
    let frame_us: u64 = match config {
        0..=11 => [10_000, 20_000, 40_000, 60_000][(config % 4) as usize],
        12..=15 => [10_000, 20_000][(config % 2) as usize],
        _ => [2_500, 5_000, 10_000, 20_000][(config % 4) as usize],
    };
    let frames: u64 = match toc & 3 {
        0 => 1,
        1 | 2 => 2,
        _ => (*packet.get(1)? & 0x3f) as u64,
    };
    Some(Duration::from_micros(frame_us * frames))
}

// --- measurements of the hop (arrival from the connector -> RTP sent) ---

static MEASURING: AtomicBool = AtomicBool::new(false);
static ARRIVALS: Mutex<VecDeque<(u32, Instant)>> = Mutex::new(VecDeque::new());
static HOP: Mutex<Vec<f32>> = Mutex::new(Vec::new());

/// Called by the RTP writer when the last packet of the frame with this RTP
/// time was written. Does nothing unless a connector is the source.
pub fn frame_sent(rtp_time: u32) {
    if !MEASURING.load(Ordering::Relaxed) {
        return;
    }
    let now = Instant::now();
    let mut arrivals = ARRIVALS.lock().unwrap();
    while let Some((time, arrived)) = arrivals.pop_front() {
        if time == rtp_time {
            HOP.lock().unwrap().push(now.duration_since(arrived).as_secs_f32() * 1000.0);
            break;
        }
    }
}

fn hop_report() -> Option<String> {
    let mut hop = std::mem::take(&mut *HOP.lock().unwrap());
    if hop.is_empty() {
        return None;
    }
    hop.sort_by(|a, b| a.total_cmp(b));
    let at = |p: f32| hop[((hop.len() as f32 * p) as usize).min(hop.len() - 1)];
    Some(format!("hop ms median={:.2} p95={:.2} p99={:.2} max={:.2} (n={})", at(0.5), at(0.95), at(0.99), hop[hop.len() - 1], hop.len()))
}

/// What the viewer sends that is for the computer shown.
pub enum ViewerInput {
    Packet(InboundPacket),
    /// The viewer's clipboard text changed.
    Clipboard(String),
}

/// The viewer's input as connector commands. Positions are scaled to the
/// display shown; wheel steps finer than a notch are added up.
#[derive(Default)]
pub struct InputMapper {
    /// The size of the display shown, in pixels (0: not known yet).
    pub width: u32,
    pub height: u32,
    wheel_rest: (i32, i32),
}

impl InputMapper {
    pub fn commands(&mut self, packet: InboundPacket) -> Vec<Command> {
        match packet {
            InboundPacket::MousePosition { x, y, reference_width, reference_height, .. } => {
                if self.width == 0 || self.height == 0 || reference_width <= 0 || reference_height <= 0 {
                    return Vec::new();
                }
                let scale = |v: i16, reference: i16, size: u32| {
                    ((v.max(0) as i64 * size as i64) / reference as i64).clamp(0, size as i64 - 1) as i32
                };
                vec![Command::PointerMove { x: scale(x, reference_width, self.width), y: scale(y, reference_height, self.height) }]
            }
            InboundPacket::MouseMove { delta_x, delta_y, .. } => vec![Command::PointerMoveRelative { dx: delta_x as i32, dy: delta_y as i32 }],
            InboundPacket::MouseButton { action, button } => vec![Command::PointerButton {
                button: match button {
                    MouseButton::Left => Button::Left,
                    MouseButton::Middle => Button::Middle,
                    MouseButton::Right => Button::Right,
                    MouseButton::X1 => Button::Back,
                    MouseButton::X2 => Button::Forward,
                },
                down: matches!(action, MouseButtonAction::Press),
            }],
            // As the local injector takes them: delta_y > 0 is up, delta_x > 0 right.
            InboundPacket::Scroll { delta_x, delta_y } => self.wheel(delta_x as i32 * 120, delta_y as i32 * 120),
            InboundPacket::HighResScroll { delta_x, delta_y } => self.wheel(delta_x as i32, delta_y as i32),
            InboundPacket::Key { action, key, .. } => match super::keys::position_of_vk(key) {
                Some(position) => vec![Command::Key { position, down: matches!(action, KeyAction::Down) }],
                None => Vec::new(),
            },
            InboundPacket::Text { text } => vec![Command::Text { text }],
            // Touch, controllers and the rest have no place in the interface.
            _ => Vec::new(),
        }
    }

    fn wheel(&mut self, dx: i32, dy: i32) -> Vec<Command> {
        self.wheel_rest.0 += dx;
        self.wheel_rest.1 += dy;
        let (notches_x, notches_y) = (self.wheel_rest.0 / 120, self.wheel_rest.1 / 120);
        self.wheel_rest.0 %= 120;
        self.wheel_rest.1 %= 120;
        if notches_x == 0 && notches_y == 0 {
            return Vec::new();
        }
        vec![Command::Wheel { dx: notches_x, dy: notches_y }]
    }
}

/// The pictures that came while the viewer's connection was not up yet:
/// the last key frame and what followed it, while that is little.
#[derive(Default)]
struct Early {
    frames: Vec<(bytes::Bytes, u32)>,
    bytes: usize,
    /// Too much came before the connection was up: nothing is kept until
    /// the next key frame.
    gave_up: bool,
}

/// More than this is not kept (a quarter to half a second of pictures):
/// the viewer would see them run by before the present one.
const EARLY_FRAMES: usize = 16;
/// The pictures kept are handed on a little apart (the key frame gets
/// more): all at once they would be thrown away further on as a backlog
/// (the RTP writer keeps a few frames at most).
const EARLY_KEY_SPACING: Duration = Duration::from_millis(10);
const EARLY_SPACING: Duration = Duration::from_millis(3);
const EARLY_BYTES: usize = 4 * 1024 * 1024;

impl Early {
    fn keep(&mut self, key: bool, data: bytes::Bytes, timestamp: u32) {
        if key {
            self.frames.clear();
            self.bytes = 0;
            self.gave_up = false;
        }
        if self.gave_up || (self.frames.is_empty() && !key) {
            return;
        }
        self.bytes += data.len();
        self.frames.push((data, timestamp));
        if self.frames.len() > EARLY_FRAMES || self.bytes > EARLY_BYTES {
            debug!("Connector: too many pictures before the viewer's connection is up; a key frame will be asked for");
            self.frames.clear();
            self.bytes = 0;
            self.gave_up = true;
        }
    }

    /// Sends what was kept; whether there was anything (a whole picture).
    async fn flush(&mut self, video: &mpsc::Sender<VideoFramePayload>, counters: &mut Counters) -> bool {
        if self.frames.is_empty() {
            return false;
        }
        self.bytes = 0;
        let kept = std::mem::take(&mut self.frames);
        let last = kept.len() - 1;
        for (n, (data, timestamp)) in kept.into_iter().enumerate() {
            counters.early += 1;
            if video.send_timeout(VideoFramePayload { full_frame: data.to_vec(), timestamp }, PACKAGER_WAIT).await.is_err() {
                // The packager did not take it: the pictures after it are no
                // use, and a key frame is asked for as before.
                return false;
            }
            if n < last {
                tokio::time::sleep(if n == 0 { EARLY_KEY_SPACING } else { EARLY_SPACING }).await;
            }
        }
        true
    }
}

/// The ends of the agent session a source is connected to.
pub struct ViewerLink {
    pub video: mpsc::Sender<VideoFramePayload>,
    pub audio: mpsc::Sender<rtc::media::Sample>,
    /// Cursor updates, in the form the session already forwards.
    pub media_events: mpsc::Sender<MediaEvent>,
    /// The session's requests of its pipeline (key frame, rate, stop).
    pub pipeline_commands: mpsc::Receiver<PipelineCommand>,
    pub input: mpsc::UnboundedReceiver<ViewerInput>,
    /// Clipboard text (or why there is none) for the viewer.
    pub clipboard: mpsc::UnboundedSender<crate::share::ClipboardOut>,
    pub general: Arc<dyn webrtc::data_channel::DataChannel>,
    pub signaling: mpsc::UnboundedSender<common::AgentMessage>,
    pub client_id: String,
    pub peer_connection: Arc<dyn webrtc::peer_connection::PeerConnection>,
    pub connected: Arc<AtomicBool>,
    /// What the viewer asked for at the start.
    pub fps: u32,
    pub bitrate_kbps: u32,
    /// The display of the source to show (its index), when the viewer chose one.
    pub display: Option<usize>,
}

fn displays_message(displays: &[DisplayInfo], current: usize) -> String {
    serde_json::json!({
        "type": "source_displays",
        "current": current.to_string(),
        "displays": displays.iter().enumerate().map(|(index, d)| serde_json::json!({
            "id": index.to_string(),
            "name": if d.name.is_empty() { format!("Display {}", index + 1) } else { d.name.clone() },
            "width": d.width,
            "height": d.height,
        })).collect::<Vec<_>>(),
    })
    .to_string()
}

/// The message that tells the viewer the delay between this agent and the
/// computer shown, there and back, on the general data channel:
/// `{"type":"source_latency","ms":12.3}`, in tenths of a millisecond (a
/// JSON number, `0.4` for 400 microseconds). The viewer adds it to its own
/// round trip to the agent and shows the sum as the ping. Lunaris's own
/// message; viewers from before it skip it (both look at `type` on that
/// channel and take only `source_displays` and `host_cursor`).
fn latency_message(tenths: u32) -> String {
    serde_json::json!({ "type": "source_latency", "ms": tenths as f64 / 10.0 }).to_string()
}

/// At most this often is the viewer told a new delay.
const LATENCY_MESSAGE_INTERVAL: Duration = Duration::from_secs(1);

/// What the connector said of the delay to its computer (`Event::Latency`)
/// and what the viewer was told of it: told once the channel is open, then
/// when it changed, at most once in `LATENCY_MESSAGE_INTERVAL`.
#[derive(Default)]
struct LatencyOut {
    /// In tenths of a millisecond: what the viewer shows at most.
    known: Option<u32>,
    sent: Option<(u32, Instant)>,
}

impl LatencyOut {
    fn note(&mut self, ms: f32) {
        if ms.is_finite() && ms >= 0.0 {
            self.known = Some((ms as f64 * 10.0).round().min(u32::MAX as f64) as u32);
        }
    }

    /// The delay to tell at `now`, if there is one to tell.
    fn due(&self, now: Instant) -> Option<u32> {
        let known = self.known?;
        match self.sent {
            None => Some(known),
            Some((sent, at)) => (sent != known && now.duration_since(at) >= LATENCY_MESSAGE_INTERVAL).then_some(known),
        }
    }

    /// The message for `now` (the channel is open and takes it).
    fn message(&mut self, now: Instant) -> Option<String> {
        let ms = self.due(now)?;
        self.sent = Some((ms, now));
        Some(latency_message(ms))
    }
}

/// Runs the source for one agent session: until the connector's session
/// ends, the agent's session stops it, or the packager is gone. When the
/// source ends by itself the viewer is told and the session closed.
pub async fn run(source: Source, link: ViewerLink) {
    let Source { codec: session_codec, session, first, pending, in_use: _in_use } = source;
    let Session { mut events, commands } = session;
    let ViewerLink {
        video,
        audio,
        media_events,
        mut pipeline_commands,
        mut input,
        clipboard,
        general,
        signaling,
        client_id,
        peer_connection,
        connected,
        fps,
        bitrate_kbps,
        display,
    } = link;
    let mut timeline = Timeline::new(Instant::now());
    // Said at once, whatever the connector tells later: a connector that
    // never names the computer's system (`Event::Platform`, which says this
    // again with the system) would leave the viewers showing the GPU of this
    // computer, which encodes nothing here.
    let _ = signaling.send(common::AgentMessage::Signaling(common::SignalingMessage::EncoderStatus {
        target_id: client_id.clone(),
        encoder: "remote".into(),
        hw_type: "connector".into(),
        gpu_info: Some("Unknown (encoded on the remote computer)".into()),
        requested_encoder: None,
        host_os: None,
        display_id: None,
        display_name: None,
        width: None,
        height: None,
    }));
    let mut queued: VecDeque<Event> = pending.into_iter().chain(first).collect();
    let mut last_key_request: Option<Instant> = None;
    // A key frame was asked for at this time and has not come yet.
    let mut key_awaited: Option<Instant> = None;
    let test_key_every = std::env::var(TEST_KEY_FRAME_EVERY_ENV).ok().and_then(|v| v.parse::<u64>().ok()).filter(|ms| *ms >= 100);
    let mut test_key_tick = tokio::time::interval(Duration::from_millis(test_key_every.unwrap_or(3_600_000)));
    test_key_tick.tick().await;
    let mut report_at = Instant::now() + Duration::from_secs(5);
    let (mut frames, mut keys, mut bytes, mut dropped) = (0u64, 0u64, 0u64, 0u64);
    let mut counters = Counters::default();
    // The text last passed either way, and how many texts went each way.
    let mut clipboard_last: Option<String> = None;
    let (mut clipboard_to_source, mut clipboard_to_viewer) = (0u64, 0u64);
    let mut mapper = InputMapper::default();
    let mut cursor = lunaris_media::types::CursorState {
        x: 0,
        y: 0,
        visible: true,
        kind: lunaris_media::types::CursorKind::Arrow,
        image: None,
        in_window_move_size: false,
    };
    // The source's displays, to tell the viewer once its channel is open.
    let mut displays_unsent: Option<String> = None;
    // The delay to the computer, for the viewer's ping.
    let mut latency = LatencyOut::default();
    let mut channel_poll = tokio::time::interval(Duration::from_millis(250));
    // (viewer input events, commands made of them): kinds only, never which key.
    let (mut input_events, mut input_commands) = (0u64, 0u64);
    ARRIVALS.lock().unwrap().clear();
    HOP.lock().unwrap().clear();
    MEASURING.store(true, Ordering::Relaxed);
    SOURCE_ACTIVE.store(true, Ordering::Relaxed);
    // The pictures that came before the viewer's connection was up, from
    // the last key frame on: sent the moment it is, so that the first
    // picture does not wait for another key frame from the computer.
    let mut early = Early::default();

    // What the viewer set: the frame rate and the bitrate it would like.
    let _ = commands.try_send(Command::Quality { fps, bitrate_kbps });
    if let Some(index) = display {
        let _ = commands.try_send(Command::SwitchDisplay { index });
    }

    // (why it ended, whether the viewer is to be told, the status to tell)
    let (reason, by_source, status) = loop {
        let event = match queued.pop_front() {
            Some(event) => event,
            None => tokio::select! {
                event = events.recv() => match event {
                    Some(event) => event,
                    None => break ("The connector ended".to_string(), true, "ended"),
                },
                _ = test_key_tick.tick(), if test_key_every.is_some() => {
                    key_awaited.get_or_insert_with(Instant::now);
                    let _ = commands.try_send(Command::RequestKeyFrame);
                    continue;
                }
                _ = channel_poll.tick(), if displays_unsent.is_some() || latency.due(Instant::now()).is_some() => {
                    if matches!(general.ready_state().await, Ok(webrtc::data_channel::RTCDataChannelState::Open)) {
                        if let Some(message) = displays_unsent.take() {
                            let _ = general.send_text(&message).await;
                        }
                        if let Some(message) = latency.message(Instant::now()) {
                            let _ = general.send_text(&message).await;
                        }
                    }
                    continue;
                }
                viewer = input.recv() => {
                    match viewer {
                        Some(ViewerInput::Packet(packet)) => {
                            input_events += 1;
                            counters.input_events = input_events;
                            for command in mapper.commands(packet) {
                                input_commands += 1;
                                counters.input_commands = input_commands;
                                // Input is never dropped silently behind a slow
                                // source: it waits its turn.
                                if commands.send(command).await.is_err() {
                                    break;
                                }
                            }
                        }
                        Some(ViewerInput::Clipboard(text)) => {
                            // Text that just came from the computer and was put
                            // on the viewer's clipboard is not sent back to it.
                            if clipboard_last.as_deref() != Some(text.as_str()) {
                                clipboard_last = Some(text.clone());
                                clipboard_to_source += 1;
                                let _ = commands.send(Command::Clipboard { text }).await;
                            }
                        }
                        // The session's channels are gone: the viewer left.
                        None => {
                            let _ = commands.send(Command::Close).await;
                            break ("The viewer left".to_string(), false, "");
                        }
                    }
                    continue;
                }
                command = pipeline_commands.recv() => {
                    let command = match command {
                        None | Some(PipelineCommand::Stop) => {
                            let _ = commands.send(Command::Close).await;
                            break ("The viewer left".to_string(), false, "");
                        }
                        Some(PipelineCommand::RequestKeyframe) => {
                            // The session asks for a whole picture when the
                            // viewer's connection is up: the pictures kept
                            // from before start with one.
                            if connected.load(Ordering::Relaxed) && early.flush(&video, &mut counters).await {
                                info!("Connector: the first picture is one kept from before the viewer's connection was up");
                                continue;
                            }
                            key_awaited.get_or_insert_with(Instant::now);
                            counters.key_requests += 1;
                            Command::RequestKeyFrame
                        }
                        Some(PipelineCommand::SetFps(fps)) | Some(PipelineCommand::SetFrameLimit(fps)) => Command::Quality { fps, bitrate_kbps: 0 },
                        Some(PipelineCommand::SetBitrate(kbps)) => Command::Quality { fps: 0, bitrate_kbps: kbps },
                    };
                    let _ = commands.try_send(command);
                    continue;
                }
            },
        };
        match event {
            Event::Video { codec, key, capture_time_ms, data } => {
                if codec != session_codec {
                    // The viewer was offered one codec and another cannot
                    // go on the same track: the viewer is asked to start
                    // the session again, which finds the new codec.
                    let _ = commands.send(Command::Close).await;
                    break (format!("The computer changed its video to {:?}", codec), true, "restart");
                }
                let now = Instant::now();
                let timestamp = timeline.rtp(capture_time_ms, now);
                if key {
                    if let Some(asked) = key_awaited.take() {
                        info!("Connector: key frame {:.0} ms after it was asked for ({} bytes)", now.duration_since(asked).as_secs_f64() * 1000.0, data.len());
                    }
                }
                if !connected.load(Ordering::Relaxed) {
                    early.keep(key, data, timestamp);
                    continue;
                }
                // (If the frame got here before the session's request did.)
                early.flush(&video, &mut counters).await;
                counters.frame(now, capture_time_ms, key);
                counters.capture_times_used = timeline.uses_capture_times();
                counters.bytes += data.len() as u64;
                if matches!(codec, Codec::H264 | Codec::H265) {
                    counters.filler_bytes += crate::video::annexb::filler_bytes(&data, codec == Codec::H265) as u64;
                }
                frames += 1;
                keys += key as u64;
                bytes += data.len() as u64;
                {
                    let mut arrivals = ARRIVALS.lock().unwrap();
                    if arrivals.len() >= 256 {
                        arrivals.pop_front();
                    }
                    arrivals.push_back((timestamp, now));
                }
                // Frames from another computer come over a network, a few at
                // once after a short stall: the packager gets a moment to
                // take them (it works several times faster than they are
                // shown) before one is given up. A stall longer than that is
                // given up on, for a fresh whole picture.
                let payload = VideoFramePayload { full_frame: data.to_vec(), timestamp };
                if video.send_timeout(payload, PACKAGER_WAIT).await.is_err() {
                    if video.is_closed() {
                        let _ = commands.send(Command::Close).await;
                        break ("The viewer left".to_string(), false, "");
                    }
                    // The frames after a dropped one refer to it.
                    dropped += 1;
                    counters.dropped += 1;
                    if last_key_request.is_none_or(|at| now.duration_since(at) >= Duration::from_millis(500)) {
                        last_key_request = Some(now);
                        counters.key_requests += 1;
                        key_awaited.get_or_insert(now);
                        let _ = commands.try_send(Command::RequestKeyFrame);
                    }
                } else {
                    counters.forwarded += 1;
                }
                if now >= report_at {
                    info!(
                        "Connector media: {:.1} frames/s, {} key, {:.2} Mbit/s, {} dropped; {}; input: {} events, {} commands",
                        frames as f64 / 5.0,
                        keys,
                        bytes as f64 * 8.0 / 5.0 / 1e6,
                        dropped,
                        hop_report().unwrap_or_else(|| "no frame sent".into()),
                        input_events,
                        input_commands
                    );
                    (frames, keys, bytes, dropped) = (0, 0, 0, 0);
                    report_at = now + Duration::from_secs(5);
                }
            }
            Event::Audio { data, .. } => {
                let Some(duration) = opus_duration(&data) else { continue };
                let _ = audio.try_send(rtc::media::Sample { data, duration, ..rtc::media::Sample::new(Instant::now()) });
            }
            Event::Displays { displays, current } => {
                info!("Connector: {} displays, showing {} ({:?})", displays.len(), current, displays.get(current));
                if let Some(shown) = displays.get(current) {
                    mapper.width = shown.width;
                    mapper.height = shown.height;
                }
                displays_unsent = Some(displays_message(&displays, current));
            }
            Event::CursorShape { width, height, hot_x, hot_y, rgba } => {
                // A pointer that is all light (Windows' text cursor as a
                // host sends it) would vanish over a white text field.
                let (width, height, hot_x, hot_y, rgba_data) =
                    super::cursor_contrast::with_dark_outline(width, height, hot_x, hot_y, &rgba)
                        .unwrap_or_else(|| (width, height, hot_x, hot_y, rgba.to_vec()));
                cursor.image = Some(lunaris_media::types::CursorImage {
                    width,
                    height,
                    hotspot_x: hot_x,
                    hotspot_y: hot_y,
                    rgba_data,
                });
                let _ = media_events.try_send(MediaEvent::CursorUpdate(cursor.clone()));
                // The picture goes once; positions repeat without it.
                cursor.image = None;
            }
            Event::CursorPosition { x, y, visible } => {
                cursor.x = x;
                cursor.y = y;
                cursor.visible = visible;
                let _ = media_events.try_send(MediaEvent::CursorUpdate(cursor.clone()));
            }
            Event::Clipboard { text } => {
                // The same text again (the computer telling back what the
                // viewer just set there) goes no further: no bouncing.
                if clipboard_last.as_deref() != Some(text.as_str()) {
                    clipboard_last = Some(text.clone());
                    clipboard_to_viewer += 1;
                    let _ = clipboard.send(crate::share::ClipboardOut::Text(text));
                }
            }
            Event::ClipboardError { text } => {
                info!("Connector: clipboard: {}", text);
                let _ = clipboard.send(crate::share::ClipboardOut::Error(text));
            }
            // The viewer treats the pointer by the kind of computer shown
            // (it learns it from the encoder status, as of a Lunaris host).
            Event::Platform { os } => {
                info!("Connector: the computer is {}", os);
                *SOURCE_OS.lock().unwrap() = Some(os.clone());
                let _ = signaling.send(common::AgentMessage::Signaling(common::SignalingMessage::EncoderStatus {
                    target_id: client_id.clone(),
                    encoder: "remote".into(),
                    hw_type: "connector".into(),
                    // Said, not left out: the viewers keep the last GPU they
                    // heard of otherwise, which is this computer's (the one
                    // bridging), not the GPU of the computer shown.
                    gpu_info: Some("Unknown (encoded on the remote computer)".into()),
                    requested_encoder: None,
                    host_os: Some(os),
                    display_id: None,
                    display_name: None,
                    width: None,
                    height: None,
                }));
            }
            // The pointer is in the picture: what a Lunaris host on Windows
            // reports while a window is moved (`in_window_move_size`), and
            // the viewers then do not draw their own.
            // Not known, only likely: the viewer is told so in the same
            // message (`pointer_maybe_in_picture`, bridge.rs) and decides by
            // looking at the picture.
            // Goes to the viewer when the channel takes it (`channel_poll`).
            Event::Latency { ms } => latency.note(ms),
            Event::CursorMaybeEmbedded { maybe } => {
                POINTER_MAYBE_IN_PICTURE.store(maybe, Ordering::Relaxed);
                let _ = media_events.try_send(MediaEvent::CursorUpdate(cursor.clone()));
            }
            Event::CursorEmbedded { embedded } => {
                cursor.in_window_move_size = embedded;
                let _ = media_events.try_send(MediaEvent::CursorUpdate(cursor.clone()));
            }
            // In the picture for the whole session, whatever the computer
            // is and whether or not a button is held: its own field of the
            // cursor message (`pointer_in_picture`, bridge.rs), not the
            // window-move one, which viewers honour for a Windows computer
            // while a button is held only.
            Event::CursorAlwaysEmbedded => {
                info!("Connector: the pointer is part of the picture for the whole session");
                note_pointer_always_in_picture(&mut cursor);
                let _ = media_events.try_send(MediaEvent::CursorUpdate(cursor.clone()));
            }
            Event::Closed { reason } => break (reason, true, "ended"),
        }
    };
    MEASURING.store(false, Ordering::Relaxed);
    counters.input_events = input_events;
    counters.input_commands = input_commands;
    info!("Connector: the source ended: {}; clipboard texts: {} to the computer, {} to the viewer", reason, clipboard_to_source, clipboard_to_viewer);
    if by_source {
        // The viewer is told why, and the session is closed: no frozen picture.
        let _ = signaling.send(common::AgentMessage::Signaling(common::SignalingMessage::SourceStatus {
            target_id: client_id,
            status: status.to_string(),
            reason: Some(reason),
        }));
        let _ = peer_connection.close().await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_delay_to_the_computer_is_told_at_once_then_on_change_once_a_second() {
        let parsed: serde_json::Value = serde_json::from_str(&latency_message(123)).unwrap();
        assert_eq!(parsed, serde_json::json!({ "type": "source_latency", "ms": 12.3 }));
        assert_eq!(serde_json::from_str::<serde_json::Value>(&latency_message(4)).unwrap()["ms"], 0.4);
        assert_eq!(serde_json::from_str::<serde_json::Value>(&latency_message(120)).unwrap()["ms"], 12.0);

        let start = Instant::now();
        let at = |ms: u64| start + Duration::from_millis(ms);
        let mut out = LatencyOut::default();
        // A connector that never says it: nothing is ever sent.
        assert_eq!(out.message(at(0)), None);
        out.note(12.0);
        // The first goes as soon as the channel takes it.
        assert_eq!(out.message(at(100)), Some(latency_message(120)));
        // The same again (to the tenth of a millisecond): nothing, however long after.
        out.note(12.04);
        assert_eq!(out.message(at(300)), None);
        // A change within the second waits for it to pass, and the newest goes.
        out.note(20.0);
        assert_eq!(out.due(at(600)), None);
        out.note(0.37);
        assert_eq!(out.due(at(1_050)), None);
        assert_eq!(out.message(at(1_100)), Some(latency_message(4)));
        // Back to what it was a while ago is a change too.
        out.note(12.0);
        assert_eq!(out.message(at(2_200)), Some(latency_message(120)));
        assert_eq!(out.message(at(9_000)), None);
        // What is not a delay is not taken.
        out.note(f32::NAN);
        out.note(-1.0);
        assert_eq!(out.message(at(20_000)), None);
    }

    #[test]
    fn capture_times_are_used_while_they_follow_the_clock() {
        let start = Instant::now();
        let mut timeline = Timeline::new(start);
        let at = |ms: u64| start + Duration::from_millis(ms);
        // The first frame arrives 100 ms in; its capture time is on the host's own clock.
        assert_eq!(timeline.rtp(Some(5_000), at(100)), 9_000);
        // 16 ms later by the host, 30 ms later here (network jitter): the host's spacing is kept.
        assert_eq!(timeline.rtp(Some(5_016), at(130)), 9_000 + 16 * 90);
        assert_eq!(timeline.rtp(Some(5_033), at(134)), 9_000 + 33 * 90);
    }

    #[test]
    fn capture_times_that_jump_or_go_back_are_dropped_for_good() {
        let start = Instant::now();
        let at = |ms: u64| start + Duration::from_millis(ms);
        let mut jump = Timeline::new(start);
        jump.rtp(Some(0), at(0));
        assert_eq!(jump.rtp(Some(60_000), at(16)), 16 * 90);
        assert_eq!(jump.rtp(Some(60_016), at(32)), 32 * 90);
        let mut back = Timeline::new(start);
        back.rtp(Some(1_000), at(0));
        back.rtp(Some(1_016), at(16));
        assert_eq!(back.rtp(Some(1_000), at(32)), 32 * 90);
        let mut none = Timeline::new(start);
        assert_eq!(none.rtp(None, at(50)), 50 * 90);
    }

    /// A session's end (its `Counters` going) resets what the cursor
    /// messages say of the source: tests that look at that take this.
    static SOURCE_FLAGS: Mutex<()> = Mutex::new(());

    #[test]
    fn a_pointer_always_in_the_picture_is_said_until_the_session_ends() {
        let _flags = SOURCE_FLAGS.lock().unwrap_or_else(|e| e.into_inner());
        assert_eq!(pointer_in_picture(), None);
        let mut cursor = lunaris_media::types::CursorState {
            x: 0,
            y: 0,
            visible: true,
            kind: lunaris_media::types::CursorKind::Arrow,
            image: None,
            in_window_move_size: false,
        };
        {
            let _session = Counters::default();
            note_pointer_always_in_picture(&mut cursor);
            assert_eq!(pointer_in_picture(), Some("always"));
            // Its own flag: neither the window-move one nor "maybe".
            assert!(!cursor.in_window_move_size);
            assert!(!pointer_maybe_in_picture());
            // Nothing is known of where the pointer is.
            assert!(!cursor.visible);
        }
        // The next session (another source, or this computer itself) starts without it.
        assert_eq!(pointer_in_picture(), None);
    }

    #[tokio::test]
    async fn the_first_pictures_are_kept_until_the_viewer_is_connected() {
        let _flags = SOURCE_FLAGS.lock().unwrap_or_else(|e| e.into_inner());
        let frame = |n: u8| bytes::Bytes::from(vec![n; 10]);
        let (video, mut packager) = mpsc::channel::<VideoFramePayload>(16);
        let mut counters = Counters::default();
        let mut early = Early::default();
        // Nothing kept: nothing to send, a key frame has to be asked for.
        assert!(!early.flush(&video, &mut counters).await);
        // Frames before a key frame are no use; from the key frame on they are kept.
        early.keep(false, frame(1), 10);
        early.keep(true, frame(2), 20);
        early.keep(false, frame(3), 30);
        // A newer key frame replaces what was kept.
        early.keep(true, frame(4), 40);
        early.keep(false, frame(5), 50);
        assert!(early.flush(&video, &mut counters).await);
        let sent: Vec<(u8, u32)> = std::iter::from_fn(|| packager.try_recv().ok()).map(|p| (p.full_frame[0], p.timestamp)).collect();
        assert_eq!(sent, [(4, 40), (5, 50)]);
        assert_eq!(counters.early, 2);
        // Sent once.
        assert!(!early.flush(&video, &mut counters).await);
        // Too many before the connection is up: given up, until the next key frame.
        early.keep(true, frame(6), 60);
        for n in 0..EARLY_FRAMES as u8 {
            early.keep(false, frame(7 + n), 70);
        }
        assert!(!early.flush(&video, &mut counters).await);
        early.keep(false, frame(20), 80);
        assert!(!early.flush(&video, &mut counters).await);
        early.keep(true, frame(21), 90);
        assert!(early.flush(&video, &mut counters).await);
        assert_eq!(packager.try_recv().unwrap().full_frame[0], 21);
        // (The counters write their line when dropped; nothing to check here.)
    }

    #[test]
    fn opus_packet_lengths() {
        // CELT fullband 10 ms, one frame (config 30); 20 ms (config 31); two 20 ms frames.
        assert_eq!(opus_duration(&[30 << 3]), Some(Duration::from_millis(10)));
        assert_eq!(opus_duration(&[31 << 3]), Some(Duration::from_millis(20)));
        assert_eq!(opus_duration(&[(31 << 3) | 1]), Some(Duration::from_millis(40)));
        // SILK 60 ms (config 3), three frames by the count byte.
        assert_eq!(opus_duration(&[(3 << 3) | 3, 3]), Some(Duration::from_millis(180)));
        assert_eq!(opus_duration(&[]), None);
    }

    #[test]
    fn the_viewer_is_offered_what_it_asked_for_first() {
        // A client from before it said what it decodes: today's guess.
        assert_eq!(viewer_codecs("h265", None), vec![Codec::H265, Codec::H264, Codec::Vp9, Codec::Vp8, Codec::Av1]);
        assert_eq!(viewer_codecs("vp9", None)[0], Codec::Vp9);
        assert!(!viewer_codecs("h264", None).contains(&Codec::H265));
        // A client that says: only what it names, the one asked for first.
        let names = |list: &[&str]| list.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(viewer_codecs("av1", Some(&names(&["H264", "av1", "vp8", "theora"]))), vec![Codec::Av1, Codec::H264, Codec::Vp8]);
        // Asked for something it does not decode: not offered.
        assert_eq!(viewer_codecs("h265", Some(&names(&["h264"]))), vec![Codec::H264]);
        assert!(viewer_codecs("h264", Some(&[])).is_empty());
    }

    #[test]
    fn the_viewers_input_becomes_commands_for_the_display_shown() {
        use crate::input::{KeyFlags, KeyModifiers};
        let mut mapper = InputMapper::default();
        let position = |x, y| InboundPacket::MousePosition { x, y, reference_width: 1000, reference_height: 500, timestamp: 0 };
        // Before the display's size is known a position means nothing.
        assert!(mapper.commands(position(500, 250)).is_empty());
        mapper.width = 1920;
        mapper.height = 1080;
        assert_eq!(mapper.commands(position(500, 250)), vec![Command::PointerMove { x: 960, y: 540 }]);
        assert_eq!(mapper.commands(position(1000, 500)), vec![Command::PointerMove { x: 1919, y: 1079 }]);
        assert_eq!(
            mapper.commands(InboundPacket::MouseMove { delta_x: -3, delta_y: 4, timestamp: 0 }),
            vec![Command::PointerMoveRelative { dx: -3, dy: 4 }]
        );
        assert_eq!(
            mapper.commands(InboundPacket::MouseButton { action: MouseButtonAction::Press, button: MouseButton::X1 }),
            vec![Command::PointerButton { button: Button::Back, down: true }]
        );
        // Whole notches at once; fine steps add up to one.
        assert_eq!(mapper.commands(InboundPacket::Scroll { delta_x: 0, delta_y: -2 }), vec![Command::Wheel { dx: 0, dy: -2 }]);
        assert!(mapper.commands(InboundPacket::HighResScroll { delta_x: 0, delta_y: 60 }).is_empty());
        assert_eq!(mapper.commands(InboundPacket::HighResScroll { delta_x: 0, delta_y: 60 }), vec![Command::Wheel { dx: 0, dy: 1 }]);
        // A key goes by its place on the keyboard (VK_A is KEY_A, 30).
        let key = |action| InboundPacket::Key { action, modifiers: KeyModifiers::empty(), key: 0x41, flags: KeyFlags::empty() };
        assert_eq!(mapper.commands(key(KeyAction::Down)), vec![Command::Key { position: 30, down: true }]);
        assert_eq!(mapper.commands(key(KeyAction::Up)), vec![Command::Key { position: 30, down: false }]);
        assert_eq!(mapper.commands(InboundPacket::Text { text: "ố".into() }), vec![Command::Text { text: "ố".into() }]);
        assert!(mapper.commands(InboundPacket::GeneralStop).is_empty());
    }
}
