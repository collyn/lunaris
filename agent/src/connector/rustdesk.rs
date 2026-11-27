//! The RustDesk connector: the agent as a RustDesk *client*, connecting to a
//! RustDesk host by its address on the LAN ("direct IP access" on the host,
//! TCP port 21118).
//!
//! Ported from RustDesk 1.5.3 (AGPL-3.0, https://github.com/rustdesk/rustdesk),
//! the client's side: `src/client.rs` and `src/client/io_loop.rs`. Each step
//! names the function it follows. The framing, the messages and the login
//! proof are the ones the agent's RustDesk listener uses
//! (`crate::rustdesk::{frame, proto, login}`).
//!
//! It carries the picture, the sound, the cursor, clipboard text both ways,
//! the pointer, keys (map mode, to Windows and Linux hosts), typed text and
//! the choice of the display. Its files are a connection of their own:
//! `rustdesk_files.rs`.

use std::time::Duration;

use bytes::{Bytes, BytesMut};
use prost::Message as _;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::sync::mpsc;
use tracing::{debug, info, warn};

use super::{keys, Button, Capabilities, Codec, Command, Connector, Description, DisplayInfo, Event, Field, Session, Target, Viewer};
use crate::rustdesk::proto::{
    key_event, login_response, message, misc, permission_info, supported_decoding::PreferCodec, video_frame, ControlKey, CursorData,
    EncodedVideoFrames, Hash, KeyEvent, KeyboardMode, LoginRequest, Message, Misc, MouseEvent, OptionMessage, PeerInfo,
    SupportedDecoding, SwitchDisplay,
};
use crate::rustdesk::{clipboard, cursor, frame, login, qos};

/// hbb_common/src/config.rs: `RELAY_PORT` + 1, the port of direct access
/// (client.rs `Client::_start`: `check_port(peer, RELAY_PORT + 1)`).
pub const DEFAULT_PORT: u16 = 21118;
/// hbb_common/src/config.rs `CONNECT_TIMEOUT` and `READ_TIMEOUT`.
pub(super) const CONNECT_TIMEOUT: Duration = Duration::from_millis(18_000);
pub(super) const READ_TIMEOUT: Duration = Duration::from_millis(18_000);
/// The version of the client this follows; a host chooses what it sends by it.
const CLIENT_VERSION: &str = "1.5.3";
/// common.rs `MIN_VER_MULTI_UI_SESSION`.
const MIN_VER_MULTI_UI_SESSION: &str = "1.2.4";

pub struct RustdeskConnector;

#[async_trait::async_trait]
impl Connector for RustdeskConnector {
    fn describe(&self) -> Description {
        let field = |key: &str, label: &str, kind: &str, default: &str, required: bool| Field {
            key: key.into(),
            label: label.into(),
            kind: kind.into(),
            default: default.into(),
            required,
            options: Vec::new(),
        };
        Description {
            id: "rustdesk".into(),
            name: "RustDesk (by IP address)".into(),
            fields: vec![
                field("host", "IP address", "text", "", true),
                field("port", "Port", "port", &DEFAULT_PORT.to_string(), false),
                field("password", "Password", "secret", "", true),
            ],
            capabilities: Capabilities {
                video: vec![Codec::H264, Codec::H265, Codec::Vp9, Codec::Vp8, Codec::Av1],
                audio: true,
                input: true,
                clipboard: true,
                cursor: true,
                files: true,
                setup: false,
            },
            note: String::new(),
        }
    }

    /// The computer's files: `rustdesk_files.rs`.
    async fn open_files(&self, target: &Target) -> anyhow::Result<super::files::FileSession> {
        super::rustdesk_files::open(target).await
    }

    async fn connect(&self, target: &Target, viewer: &Viewer) -> anyhow::Result<Session> {
        let host = target.get("host").trim();
        if host.is_empty() {
            anyhow::bail!("The computer's IP address is missing");
        }
        let port = match target.get("port").trim() {
            "" => DEFAULT_PORT,
            port => port.parse::<u16>().map_err(|_| anyhow::anyhow!("\"{}\" is not a port", port))?,
        };
        if viewer.codecs.is_empty() {
            anyhow::bail!("The viewer decodes no video codec");
        }
        let address = format!("{}:{}", host, port);

        // client.rs Client::_start, the `is_ip_str` branch: a plain TCP
        // connection, no rendezvous and no secure_connection (key exchange).
        // The errors are for the viewer, who is not told where the computer
        // is: the address stays in this computer's log.
        let stream = match tokio::time::timeout(CONNECT_TIMEOUT, TcpStream::connect(&address)).await {
            Ok(Ok(stream)) => stream,
            Ok(Err(e)) => {
                warn!("RustDesk connector: {} cannot be reached: {}", address, e);
                anyhow::bail!("The RustDesk computer cannot be reached (is it on, and is direct IP access switched on there?)");
            }
            Err(_) => {
                warn!("RustDesk connector: {} does not answer", address);
                anyhow::bail!("The RustDesk computer does not answer");
            }
        };
        let _ = stream.set_nodelay(true);
        let mut conn = Conn { stream, decoder: frame::Decoder::new(frame::MAX_FRAME_LENGTH), buf: BytesMut::with_capacity(64 * 1024) };

        // io_loop.rs handle_msg_from_peer: the host's Hash, then our
        // LoginRequest, then its LoginResponse.
        // connection.rs: what the host does not allow is said before its
        // LoginResponse (`Misc::PermissionInfo`).
        let mut clipboard_allowed = true;
        let peer_info = loop {
            let msg = tokio::time::timeout(READ_TIMEOUT, conn.next())
                .await
                .map_err(|_| anyhow::anyhow!("The RustDesk computer did not answer the login"))?
                .map_err(|e| anyhow::anyhow!("The connection to the RustDesk computer broke: {}", e.kind()))?
                .ok_or_else(|| anyhow::anyhow!("The RustDesk computer closed the connection"))?;
            match msg.union {
                Some(message::Union::Hash(hash)) => {
                    conn.send(&login_message(&address, target.get("password"), &hash, viewer)).await?;
                }
                Some(message::Union::Misc(Misc { union: Some(misc::Union::PermissionInfo(info)) })) => {
                    if info.permission == permission_info::Permission::Clipboard as i32 {
                        clipboard_allowed = info.enabled;
                    }
                }
                Some(message::Union::LoginResponse(response)) => match response.union {
                    Some(login_response::Union::Error(error)) => anyhow::bail!("{}", login_error(&error)),
                    Some(login_response::Union::PeerInfo(info)) => break info,
                    None => {}
                },
                // client.rs handle_test_delay.
                Some(message::Union::TestDelay(delay)) if !delay.from_client => {
                    conn.send(&Message { union: Some(message::Union::TestDelay(delay)) }).await?;
                }
                // A host that asks someone at the computer first says so
                // (io_loop.rs: Misc::CloseReason ends the session).
                Some(message::Union::Misc(Misc { union: Some(misc::Union::CloseReason(reason)) })) => {
                    anyhow::bail!("{}", if reason.is_empty() { "The computer closed the connection".to_string() } else { reason })
                }
                _ => {}
            }
        };
        info!(
            "RustDesk connector: logged in to {} (version {}, platform {}, {} displays)",
            address,
            peer_info.version,
            peer_info.platform,
            peer_info.displays.len()
        );

        let (event_tx, events) = mpsc::channel::<Event>(64);
        let (commands, command_rx) = mpsc::channel::<Command>(64);
        let _ = event_tx.try_send(Event::Platform { os: platform_of(&peer_info) });
        let _ = event_tx.try_send(displays_event(&peer_info));
        let wanted = viewer.codecs.clone();
        let fps_percent = target.get(FPS_PERCENT_KEY).trim().parse::<u32>().ok().filter(|p| (100..=200).contains(p)).unwrap_or(DEFAULT_FPS_PERCENT);
        tokio::spawn(async move {
            let reason = run(conn, peer_info, wanted, fps_percent, clipboard_allowed, &event_tx, command_rx).await;
            debug!("RustDesk connector: session with {} ended: {}", address, reason);
            let _ = event_tx.send(Event::Closed { reason }).await;
        });
        Ok(Session { events, commands })
    }
}

pub(super) struct Conn {
    pub(super) stream: TcpStream,
    pub(super) decoder: frame::Decoder,
    pub(super) buf: BytesMut,
}

impl Conn {
    /// hbb_common/src/bytes_codec.rs: one framed protobuf `Message`.
    pub(super) async fn send(&mut self, msg: &Message) -> std::io::Result<()> {
        let len = msg.encoded_len();
        let mut buf = BytesMut::with_capacity(len + 4);
        frame::put_header(len, &mut buf)?;
        msg.encode(&mut buf).map_err(std::io::Error::other)?;
        self.stream.write_all(&buf).await
    }

    /// The next message; `None` when the host closed the connection.
    pub(super) async fn next(&mut self) -> std::io::Result<Option<Message>> {
        loop {
            if let Some(payload) = self.decoder.decode(&mut self.buf)? {
                match Message::decode(payload.freeze()) {
                    Ok(msg) => return Ok(Some(msg)),
                    // io_loop.rs handle_msg_from_peer: what does not parse is skipped.
                    Err(_) => continue,
                }
            }
            if self.stream.read_buf(&mut self.buf).await? == 0 {
                return Ok(None);
            }
        }
    }
}

/// scrap codec.rs `Decoder::supported_decodings`, for a viewer that is a
/// WebRTC receiver: the abilities are the codecs it negotiated, the
/// preference its first.
fn supported_decoding(viewer: &Viewer) -> SupportedDecoding {
    let has = |codec| viewer.codecs.contains(&codec) as i32;
    SupportedDecoding {
        ability_vp8: has(Codec::Vp8),
        ability_vp9: has(Codec::Vp9),
        ability_av1: has(Codec::Av1),
        ability_h264: has(Codec::H264),
        ability_h265: has(Codec::H265),
        prefer: match viewer.codecs.first() {
            Some(Codec::H264) => PreferCodec::H264,
            Some(Codec::H265) => PreferCodec::H265,
            Some(Codec::Vp8) => PreferCodec::Vp8,
            Some(Codec::Vp9) => PreferCodec::Vp9,
            Some(Codec::Av1) => PreferCodec::Av1,
            None => PreferCodec::Auto,
        } as i32,
        ..Default::default()
    }
}

/// client.rs `handle_hash` with a preset password, then
/// `handle_login_from_ui` and `LoginConfigHandler::create_login_msg`:
/// the password goes as sha256(sha256(password + salt) + challenge).
pub(super) fn login_message(address: &str, password: &str, hash: &Hash, viewer: &Viewer) -> Message {
    let proof = login::proof(&login::h1(password, &hash.salt), &hash.challenge);
    Message {
        union: Some(message::Union::LoginRequest(LoginRequest {
            // create_login_msg: `username` is the peer's id, here its address.
            username: address.to_string(),
            password: Bytes::copy_from_slice(&proof[..]),
            my_id: "lunaris".into(),
            my_name: "Lunaris".into(),
            my_platform: match std::env::consts::OS {
                "windows" => "Windows",
                "macos" => "Mac OS",
                _ => "Linux",
            }
            .into(),
            version: CLIENT_VERSION.into(),
            session_id: u64::from_le_bytes(crate::pairing::random_bytes(8).try_into().unwrap_or_default()),
            // client.rs get_option_message(true): the frame rate wanted and
            // what can be decoded.
            option: Some(OptionMessage {
                custom_fps: viewer.fps as i32,
                supported_decoding: Some(supported_decoding(viewer)),
                ..Default::default()
            }),
            ..Default::default()
        })),
    }
}

/// client.rs `handle_login_error`: the host's words, with what they mean
/// for someone who set the target up.
pub(super) fn login_error(error: &str) -> String {
    match error {
        login::LOGIN_MSG_PASSWORD_WRONG => "Wrong password for the RustDesk computer".to_string(),
        login::LOGIN_MSG_NO_PASSWORD_ACCESS => "The RustDesk computer does not accept a password (only a click at the computer)".to_string(),
        other => other.to_string(),
    }
}

fn displays_event(info: &PeerInfo) -> Event {
    Event::Displays {
        displays: info
            .displays
            .iter()
            .map(|d| DisplayInfo { name: d.name.clone(), width: d.width.max(0) as u32, height: d.height.max(0) as u32 })
            .collect(),
        current: info.current_display.max(0) as usize,
    }
}

/// How long a session waits for the first picture before it asks for one,
/// and how often it asks again.
const FIRST_PICTURE_WAIT: Duration = Duration::from_millis(1000);
const FIRST_PICTURE_RETRY: Duration = Duration::from_millis(2000);

fn misc(union: misc::Union) -> Message {
    Message { union: Some(message::Union::Misc(Misc { union: Some(union) })) }
}

/// ui_session_interface.rs `refresh_video`: `LoginConfigHandler::refresh_display`
/// for a peer from 1.2.4 on, else `LoginConfigHandler::refresh`.
fn refresh_message(peer_version: &str, display: i32) -> Message {
    use crate::rustdesk::clipboard::get_version_number;
    if get_version_number(peer_version) >= get_version_number(MIN_VER_MULTI_UI_SESSION) {
        misc(misc::Union::RefreshVideoDisplay(display))
    } else {
        misc(misc::Union::RefreshVideo(true))
    }
}

/// The frames of a `VideoFrame` and their codec; `None` for raw pictures
/// (rgb, yuv), which no host sends to a client of this age.
fn video_frames(union: video_frame::Union) -> Option<(Codec, EncodedVideoFrames)> {
    Some(match union {
        video_frame::Union::H264s(frames) => (Codec::H264, frames),
        video_frame::Union::H265s(frames) => (Codec::H265, frames),
        video_frame::Union::Vp8s(frames) => (Codec::Vp8, frames),
        video_frame::Union::Vp9s(frames) => (Codec::Vp9, frames),
        video_frame::Union::Av1s(frames) => (Codec::Av1, frames),
        _ => return None,
    })
}

// common.rs `input`: the mouse event's mask is `buttons << 3 | type`.
const MOUSE_TYPE_MOVE: i32 = 0;
const MOUSE_TYPE_DOWN: i32 = 1;
const MOUSE_TYPE_UP: i32 = 2;
const MOUSE_TYPE_WHEEL: i32 = 3;
const MOUSE_TYPE_MOVE_RELATIVE: i32 = 5;
const MOUSE_BUTTON_LEFT: i32 = 0x01;
const MOUSE_BUTTON_RIGHT: i32 = 0x02;
const MOUSE_BUTTON_WHEEL: i32 = 0x04;
const MOUSE_BUTTON_BACK: i32 = 0x08;
const MOUSE_BUTTON_FORWARD: i32 = 0x10;

/// io_loop.rs `decode_cursor_data`: the largest cursor taken.
const MAX_CURSOR_SIZE: i32 = 512;
/// Cursor shapes kept to answer a `CursorId` (the host sends each once).
const MAX_KEPT_CURSORS: usize = 64;
/// Whether the image quality is to be changed from `last` to `wanted` (per
/// cent), `since` the last change. Each change makes the host set its
/// encoder's bitrate anew and start its own rate control over, so the
/// viewer's estimate is not followed in every wobble: down by 15 % or more,
/// 2 s after the last change at the earliest; up by 25 % or more, 10 s after.
fn quality_change_due(last: i32, wanted: i32, since: Duration) -> bool {
    if wanted < last {
        (last - wanted) * 100 >= last * 15 && since >= Duration::from_secs(2)
    } else {
        (wanted - last) * 100 >= last * 25 && since >= Duration::from_secs(10)
    }
}

/// What the client knows of the host and of its own keyboard.
struct Remote {
    version: String,
    /// keyboard.rs: the peer's platform, lower case without spaces
    /// ("windows", "linux", "macos", "android").
    platform: String,
    displays: Vec<crate::rustdesk::proto::DisplayInfo>,
    display: i32,
    codec: Option<Codec>,
    /// The modifier keys held (by position), for the mouse events.
    held: Vec<u32>,
    /// The viewer's lock keys as this client counts them: off and on at
    /// the start, toggled by the viewer's own CapsLock and NumLock keys.
    caps_lock: bool,
    num_lock: bool,
    cursors: Vec<(u64, Event)>,
    quality_sent: Option<(i32, std::time::Instant)>,
    keys_unsendable_logged: bool,
    /// Where the viewer's pointer is (display pixels), and its left button.
    pointer: (i32, i32),
    left_down: bool,
    /// Where the left button went down, and whether the cursor was the
    /// arrow then.
    press: (i32, i32),
    press_with_arrow: bool,
    /// Whether the host's cursor is the arrow now.
    cursor_is_arrow: bool,
    /// Whether the viewer was told the pointer is in the picture.
    embedded: bool,
    /// Events a command or a cursor message gave rise to.
    pending: Vec<Event>,
    /// The host's delay probes: how long each answer took here (µs), and
    /// what the host last said of the delay it measures (ms) and of its
    /// bitrate (kbit/s).
    /// The frame rate asked of the host, in per cent of the viewer's (see
    /// `FPS_PERCENT_KEY`).
    fps_percent: u32,
    /// Whether the host lets its clipboard be used, and whether the viewer
    /// was told that it does not.
    clipboard_allowed: bool,
    clipboard_refusal_told: bool,
    probe_answers_us: Vec<u32>,
    probes_logged: bool,
    host_delay_ms: u32,
    host_bitrate_kbps: u32,
    /// Whether a delay probe of the host was seen in this session, and the
    /// delay last told to the viewer (`latency_event`).
    probe_seen: bool,
    /// In tenths of a millisecond.
    latency_told: Option<u32>,
    /// How often the delay told came from the kernel's figure for the
    /// connection, and how often from the host's (for the log).
    latency_from_tcp: u32,
    latency_from_host: u32,
    tcp_rtt_us: Option<u32>,
}

/// The kernel's round trip time of a TCP connection, in microseconds.
///
/// Linux: `getsockopt(TCP_INFO)`, `tcp_info.tcpi_rtt` (tcp(7); kernel
/// include/uapi/linux/tcp.h `struct tcp_info`, filled by net/ipv4/tcp.c
/// `tcp_get_info`: `info->tcpi_rtt = tp->srtt_us >> 3`): the smoothed time
/// between a segment this side sent and its acknowledgement (RFC 6298).
/// It is measured below the application, so neither the time a message
/// waits behind pictures in the host's send queue nor the time the two
/// programs take is in it. Elsewhere `None` (Windows has `SIO_TCP_INFO`;
/// it is not read here, and the host's figure is used there).
///
/// Any socket of a TCP connection will do (the RDP connector passes a
/// duplicate of its socket's descriptor, rdp/session.rs).
#[cfg(target_os = "linux")]
pub(crate) fn tcp_round_trip_us<S: std::os::fd::AsRawFd + ?Sized>(stream: &S) -> Option<u32> {
    let mut info: libc::tcp_info = unsafe { std::mem::zeroed() };
    let mut len = std::mem::size_of::<libc::tcp_info>() as libc::socklen_t;
    let result = unsafe {
        libc::getsockopt(stream.as_raw_fd(), libc::IPPROTO_TCP, libc::TCP_INFO, (&mut info as *mut libc::tcp_info).cast(), &mut len)
    };
    (result == 0 && info.tcpi_rtt > 0).then_some(info.tcpi_rtt)
}

#[cfg(not(target_os = "linux"))]
pub(crate) fn tcp_round_trip_us<S: ?Sized>(_stream: &S) -> Option<u32> {
    None
}

/// Where a delay to the computer came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DelaySource {
    Tcp,
    Host,
}

/// The round trip to the computer, in milliseconds, of the two figures
/// there are for it: the host's (`host_ms`: `TestDelay.last_delay`, see
/// `Remote::latency_event`; `None` while it has measured nothing) and the
/// kernel's for the connection (`tcp_us`: `tcp_round_trip_us`).
///
/// Lunaris's own rule. Each figure can be too high for a reason of its own:
/// the host's holds the time its probe waited behind pictures in the same
/// stream and the time this program took to answer; the kernel's holds the
/// time the host's system kept an acknowledgement back when it had nothing
/// to send with it (delayed ACK). Neither is too low for long, except that
/// the host's is cut to whole milliseconds (`as_millis`). So the kernel's is
/// taken, finer and free of queueing, unless it is above what the host can
/// have measured (its figure plus the millisecond that was cut off): then
/// the kernel's is the inflated one and the host's is told.
fn target_round_trip_ms(host_ms: Option<u32>, tcp_us: Option<u32>) -> Option<(f32, DelaySource)> {
    let tcp_ms = tcp_us.map(|us| us as f32 / 1000.0);
    match (host_ms, tcp_ms) {
        (Some(host), Some(tcp)) if tcp > host as f32 + 1.0 => Some((host as f32, DelaySource::Host)),
        (_, Some(tcp)) => Some((tcp, DelaySource::Tcp)),
        (Some(host), None) => Some((host as f32, DelaySource::Host)),
        (None, None) => None,
    }
}

/// However the session ends, its numbers are written.
impl Drop for Remote {
    fn drop(&mut self) {
        self.log_probes();
    }
}

/// The platform of a host as the viewers are told it ("windows", "linux",
/// "macos", "android"): `PeerInfo.platform` in lower case without spaces.
pub(super) fn platform_of(info: &PeerInfo) -> String {
    let mut platform = info.platform.to_lowercase();
    platform.retain(|c| !c.is_whitespace());
    platform
}

/// The frame rate asked of the host, in per cent of the viewer's.
///
/// A RustDesk host paces its capture by sleeping the rest of each frame's
/// time (server/video_service.rs `run`: `c.frame(spf)`, the work, then
/// `sleep(spf - elapsed)`), so each round takes the frame's time plus what
/// the sleep overshoots, and it delivers somewhat fewer frames than it was
/// asked for. Measured on a Windows host (RustDesk 1.5.3): asked for 60 it
/// gave 58.4 a second at best, asked for 72 it gave 69.2 to 69.8: about
/// 97 % either way. Asking for 3 % more (rounded up: 62 for 60) lands on
/// the viewer's rate.
///
/// More is not asked for by default, because what comes cannot be thinned
/// out here: RustDesk's encoders make one chain of frames that each depend
/// on the one before (hwcodec.rs `DEFAULT_GOP` is `i32::MAX`, and neither
/// it nor the VP8/VP9/AV1 encoders use temporal layers), so leaving a
/// frame out would break the picture until the next key frame. A host
/// that keeps its rate exactly gives the viewer 62 for 60, which it takes.
const DEFAULT_FPS_PERCENT: u32 = 103;

/// A value of a source in the config file that no form shows (for trying
/// out): the per cent above, 100 to 200.
pub const FPS_PERCENT_KEY: &str = "fps_percent";

/// The frame rate to ask the host for (client.rs `set_custom_fps` takes
/// what the person set; the host takes 1 to 120, video_qos.rs `MAX_FPS`).
fn asked_fps(viewer_fps: u32, percent: u32) -> u32 {
    (viewer_fps * percent).div_ceil(100).clamp(1, 120)
}

const CLIPBOARD_NOT_ALLOWED: &str = "The other computer does not allow its clipboard to be used";
const CLIPBOARD_TOO_LARGE: &str = "The text copied on the other computer is too large to bring over (more than 1 MB)";

/// The left button held and the pointer moved this far (pixels) is a drag.
const DRAG_PIXELS: i32 = 4;
/// A cursor whose hot spot is this near its top left corner is the arrow.
const ARROW_HOT_SPOT: u32 = 2;

impl Remote {
    fn new(info: &PeerInfo) -> Self {
        let mut platform = info.platform.to_lowercase();
        platform.retain(|c| !c.is_whitespace());
        Remote {
            version: info.version.clone(),
            platform,
            displays: info.displays.clone(),
            display: info.current_display,
            codec: None,
            held: Vec::new(),
            caps_lock: false,
            num_lock: true,
            cursors: Vec::new(),
            quality_sent: None,
            keys_unsendable_logged: false,
            pointer: (0, 0),
            left_down: false,
            press: (0, 0),
            press_with_arrow: false,
            cursor_is_arrow: true,
            embedded: false,
            pending: Vec::new(),
            fps_percent: DEFAULT_FPS_PERCENT,
            clipboard_allowed: true,
            clipboard_refusal_told: false,
            probe_answers_us: Vec::new(),
            probes_logged: false,
            host_delay_ms: 0,
            host_bitrate_kbps: 0,
            probe_seen: false,
            latency_told: None,
            latency_from_tcp: 0,
            latency_from_host: 0,
            tcp_rtt_us: None,
        }
    }

    /// `Event::Latency` for a delay probe of the host carrying `last_delay`,
    /// when there is something new to tell.
    ///
    /// What `last_delay` is, from the RustDesk source:
    /// - src/server/connection.rs, `Connection::start`, the
    ///   `test_delay_timer.tick()` branch (every `TEST_DELAY_TIMEOUT` = 1 s,
    ///   when no probe is unanswered): the host notes
    ///   `conn.last_test_delay = Some(Instant::now())` and sends
    ///   `TestDelay { last_delay: conn.network_delay, target_bitrate, .. }`.
    /// - src/client.rs `handle_test_delay` (called from
    ///   src/client/io_loop.rs): the client sends the probe back as it came.
    /// - src/server/connection.rs, `Connection::on_message`, the `TestDelay`
    ///   branch with `!t.from_client`:
    ///   `new_delay = tm.elapsed().as_millis() as u32` (`tm` = the noted
    ///   `last_test_delay`), `self.network_delay = new_delay`.
    /// - protos/message.proto of hbb_common (libs/base/protos in the
    ///   checkout read): `message TestDelay { .. uint32 last_delay = 3; .. }`.
    ///
    /// So it is a round trip (host -> client -> host), in milliseconds,
    /// measured by the host with its own clock, of the probe *before* this
    /// one (a second old), and it holds the time the probe waited behind
    /// pictures in the same stream and the time this connector took to send
    /// it back. It is told as it is: already there and back, not doubled.
    ///
    /// `network_delay` starts at 0 (connection.rs `Connection::start`,
    /// `network_delay: 0`), so the first probe carries a 0 that measured
    /// nothing. Lunaris's own choice: the first probe seen in a session does
    /// not count as a figure of the host (a later 0 is a measurement of
    /// under a millisecond and does).
    ///
    /// `tcp_us`: the kernel's figure for the connection at this moment
    /// (`tcp_round_trip_us`); which of the two is told: `target_round_trip_ms`.
    /// Told in tenths of a millisecond, when that changed.
    fn latency_event(&mut self, last_delay: u32, tcp_us: Option<u32>) -> Option<Event> {
        let host_ms = std::mem::replace(&mut self.probe_seen, true).then_some(last_delay);
        self.tcp_rtt_us = tcp_us;
        let (ms, source) = target_round_trip_ms(host_ms, tcp_us)?;
        match source {
            DelaySource::Tcp => self.latency_from_tcp += 1,
            DelaySource::Host => self.latency_from_host += 1,
        }
        let tenths = (ms * 10.0).round() as u32;
        if self.latency_told == Some(tenths) {
            return None;
        }
        self.latency_told = Some(tenths);
        Some(Event::Latency { ms: tenths as f32 / 10.0 })
    }

    /// Where the display shown is on the host's desktop, and its size.
    fn shown(&self) -> (i32, i32, i32, i32) {
        self.displays.get(self.display.max(0) as usize).map(|d| (d.x, d.y, d.width, d.height)).unwrap_or((0, 0, 0, 0))
    }

    fn displays_event(&self) -> Event {
        Event::Displays {
            displays: self
                .displays
                .iter()
                .map(|d| DisplayInfo { name: d.name.clone(), width: d.width.max(0) as u32, height: d.height.max(0) as u32 })
                .collect(),
            current: self.display.max(0) as usize,
        }
    }

    /// client.rs `send_mouse`: the modifiers held go with every mouse event.
    fn mouse(&self, mask: i32, x: i32, y: i32) -> Message {
        let held = |keys: [u32; 2]| self.held.iter().any(|k| keys.contains(k));
        let mut modifiers = Vec::new();
        if held([56, 100]) {
            modifiers.push(ControlKey::Alt as i32);
        }
        if held([42, 54]) {
            modifiers.push(ControlKey::Shift as i32);
        }
        if held([29, 97]) {
            modifiers.push(ControlKey::Control as i32);
        }
        if held([125, 126]) {
            modifiers.push(ControlKey::Meta as i32);
        }
        Message { union: Some(message::Union::MouseEvent(MouseEvent { mask, x, y, modifiers })) }
    }

    /// keyboard.rs `event_to_key_events` in map mode: `map_keyboard_mode`
    /// (`windows_peer_special_key`, `_map_keyboard_mode` as a Linux client
    /// has it) and `parse_add_lock_modes_modifiers`.
    fn key(&mut self, position: u32, down: bool) -> Option<Message> {
        const MODIFIERS: [u32; 8] = [29, 97, 42, 54, 56, 100, 125, 126];
        if MODIFIERS.contains(&position) {
            self.held.retain(|k| *k != position);
            if down {
                self.held.push(position);
            }
        }
        if down && position == keys::KEY_CAPSLOCK {
            self.caps_lock = !self.caps_lock;
        }
        if down && position == keys::KEY_NUMLOCK {
            self.num_lock = !self.num_lock;
        }
        let mut event = KeyEvent { down, mode: KeyboardMode::Map as i32, ..Default::default() };
        // windows_peer_special_key: Windows has no scan code for Pause.
        const KEY_PAUSE: u32 = 119;
        if self.platform == "windows" && position == KEY_PAUSE {
            event.mode = KeyboardMode::Legacy as i32;
            event.union = Some(key_event::Union::ControlKey(ControlKey::Pause as i32));
            return Some(Message { union: Some(message::Union::KeyEvent(event)) });
        }
        let keycode = match self.platform.as_str() {
            // rdev::linux_code_to_win_scancode
            "windows" => keys::win_scancode_of(position)?,
            // The X keycode (`event.position_code` of a Linux client).
            "linux" => position + 8,
            // macOS and Android hosts take their own key codes
            // (rdev::linux_code_to_macos_code, ..._to_android_key_code),
            // whose tables are not here yet.
            _ => {
                if !self.keys_unsendable_logged {
                    self.keys_unsendable_logged = true;
                    warn!("RustDesk connector: keys are not sent to a {} computer yet", self.platform);
                }
                return None;
            }
        };
        event.union = Some(key_event::Union::Chr(keycode));
        if (keys::is_letter(position) || matches!(position, 26 | 27 | 39 | 40 | 51 | 52)) && self.caps_lock {
            event.modifiers.push(ControlKey::CapsLock as i32);
        }
        if keys::is_numpad(position) && self.num_lock {
            event.modifiers.push(ControlKey::NumLock as i32);
        }
        Some(Message { union: Some(message::Union::KeyEvent(event)) })
    }

    /// The custom image quality (per cent) at which the host's encoder is
    /// given about `kbps`: the inverse of video_qos.rs `user_image_quality`
    /// (ratio = per cent * 2 / 100, within BR_MIN..BR_MAX) and of the
    /// encoders' bitrate from the ratio (hwcodec.rs `calc_bitrate` for
    /// H.264 and H.265; `base_bitrate * ratio` for the others).
    fn quality_for(&self, kbps: u32) -> i32 {
        let (_, _, width, height) = self.shown();
        let (width, height) = (width.max(1) as usize, height.max(1) as usize);
        let bitrate_at = |percent: i32| -> u32 {
            let ratio = percent as f32 * 2.0 / 100.0;
            match self.codec {
                Some(Codec::H264) => qos::calc_bitrate(width, height, ratio, true),
                Some(Codec::H265) => qos::calc_bitrate(width, height, ratio, false),
                _ => (qos::base_bitrate(width as u32, height as u32) as f32 * ratio) as u32,
            }
        };
        // The slider of the client: 10 to 2000 per cent; the bitrate grows with it.
        let (mut low, mut high) = (10, 2000);
        while low < high {
            let middle = (low + high + 1) / 2;
            if bitrate_at(middle) <= kbps {
                low = middle;
            } else {
                high = middle - 1;
            }
        }
        low
    }

    /// The pointer in the picture while a window is moved on Windows.
    ///
    /// While a window is dragged by its title bar, Windows draws the
    /// pointer into the desktop picture itself (so that pointer and window
    /// move together), and the picture a host captures then has the pointer
    /// in it. A Lunaris host says so (`in_window_move_size`, from
    /// `GUI_INMOVESIZE`), and its viewers do not draw their own pointer
    /// meanwhile. RustDesk's host says nothing of it: its cursor service
    /// (server/input_service.rs `run_cursor`) sends a shape when the cursor's
    /// handle changes and nothing when the pointer becomes part of the
    /// picture, and its displays' `cursor_embedded` is fixed per display.
    /// What is known here is the viewer's own input: the left button held
    /// and the pointer dragged, begun while the cursor was the arrow, is
    /// what moving a window looks like. It is a likeness, not knowledge
    /// (a scroll bar or a selection frame on the desktop looks the same), so
    /// it is told to the viewer as "the pointer may be in the picture": the
    /// viewer keeps its own pointer, looks for the computer's in the
    /// picture, and hides its own only when it finds it there.
    fn pointer_moved(&mut self) {
        if self.platform != "windows" || !self.left_down || !self.press_with_arrow || self.embedded {
            return;
        }
        let (dx, dy) = (self.pointer.0 - self.press.0, self.pointer.1 - self.press.1);
        if dx.abs() >= DRAG_PIXELS || dy.abs() >= DRAG_PIXELS {
            self.embedded = true;
            self.pending.push(Event::CursorMaybeEmbedded { maybe: true });
        }
    }

    fn left_button(&mut self, down: bool) {
        self.left_down = down;
        if down {
            self.press = self.pointer;
            self.press_with_arrow = self.cursor_is_arrow;
        } else if self.embedded {
            self.embedded = false;
            self.pending.push(Event::CursorMaybeEmbedded { maybe: false });
        }
    }

    fn note_cursor(&mut self, shape: &Event) {
        if let Event::CursorShape { hot_x, hot_y, .. } = shape {
            self.cursor_is_arrow = *hot_x <= ARROW_HOT_SPOT && *hot_y <= ARROW_HOT_SPOT;
        }
    }

    /// io_loop.rs `handle_msg_from_peer` for `CursorData`, with
    /// `decode_cursor_data`: the size, the hot spot and the length are
    /// checked before the pixels are taken.
    fn cursor_shape(&mut self, data: CursorData) -> Option<Event> {
        if !(1..=MAX_CURSOR_SIZE).contains(&data.width) || !(1..=MAX_CURSOR_SIZE).contains(&data.height) {
            return None;
        }
        if !(0..data.width).contains(&data.hotx) || !(0..data.height).contains(&data.hoty) {
            return None;
        }
        let expected = data.width as usize * data.height as usize * 4;
        let rgba = cursor::decompress(&data.colors, expected)?;
        if rgba.len() != expected {
            return None;
        }
        let event = Event::CursorShape {
            width: data.width as u32,
            height: data.height as u32,
            hot_x: data.hotx as u32,
            hot_y: data.hoty as u32,
            rgba: Bytes::from(rgba),
        };
        self.cursors.retain(|(id, _)| *id != data.id);
        if self.cursors.len() >= MAX_KEPT_CURSORS {
            self.cursors.remove(0);
        }
        self.cursors.push((data.id, event.clone()));
        self.note_cursor(&event);
        Some(event)
    }

    /// The messages of a command; none for what the host cannot be told.
    fn command(&mut self, command: Command) -> Vec<Message> {
        let (origin_x, origin_y, _, _) = self.shown();
        match command {
            // flutter input_model.dart: the position is on the host's
            // desktop (`x += rect.left`), the display's origin added.
            Command::PointerMove { x, y } => {
                self.pointer = (x, y);
                self.pointer_moved();
                vec![self.mouse(MOUSE_TYPE_MOVE, x + origin_x, y + origin_y)]
            }
            Command::PointerMoveRelative { dx, dy } => {
                self.pointer = (self.pointer.0 + dx, self.pointer.1 + dy);
                self.pointer_moved();
                vec![self.mouse(MOUSE_TYPE_MOVE_RELATIVE, dx, dy)]
            }
            Command::PointerButton { button, down } => {
                if button == Button::Left {
                    self.left_button(down);
                }
                let button = match button {
                    Button::Left => MOUSE_BUTTON_LEFT,
                    Button::Right => MOUSE_BUTTON_RIGHT,
                    Button::Middle => MOUSE_BUTTON_WHEEL,
                    Button::Back => MOUSE_BUTTON_BACK,
                    Button::Forward => MOUSE_BUTTON_FORWARD,
                };
                vec![self.mouse((button << 3) | if down { MOUSE_TYPE_DOWN } else { MOUSE_TYPE_UP }, 0, 0)]
            }
            // server/input_service.rs handle_mouse_: y > 0 turns the wheel
            // up, x > 0 to the left.
            Command::Wheel { dx, dy } => vec![self.mouse(MOUSE_TYPE_WHEEL, -dx, dy)],
            Command::Key { position, down } => self.key(position, down).into_iter().collect(),
            // ui_session_interface.rs input_string.
            Command::Text { text } => vec![Message {
                union: Some(message::Union::KeyEvent(KeyEvent { union: Some(key_event::Union::Seq(text)), ..Default::default() })),
            }],
            // clipboard.rs: the text, as `MultiClipboards` to hosts that know it.
            Command::Clipboard { text } => {
                // A host that does not allow its clipboard takes no text
                // (connection.rs drops it); the viewer is told, once.
                if !self.clipboard_allowed {
                    if !std::mem::replace(&mut self.clipboard_refusal_told, true) {
                        self.pending.push(Event::ClipboardError { text: CLIPBOARD_NOT_ALLOWED.into() });
                    }
                    return Vec::new();
                }
                vec![clipboard::text_message(&text, &self.version, &self.platform_name())]
            }
            Command::RequestKeyFrame => vec![refresh_message(&self.version, self.display)],
            Command::Quality { fps, bitrate_kbps } => {
                let mut out = Vec::new();
                // client.rs LoginConfigHandler::set_custom_fps.
                if fps > 0 {
                    let fps = asked_fps(fps, self.fps_percent);
                    out.push(misc(misc::Union::Option(OptionMessage { custom_fps: fps as i32, ..Default::default() })));
                }
                // The protocol has no message for a bitrate. What a person
                // does on a slow connection is lower the image quality:
                // client.rs LoginConfigHandler::save_custom_image_quality.
                if bitrate_kbps > 0 {
                    let percent = self.quality_for(bitrate_kbps);
                    let now = std::time::Instant::now();
                    let due = match self.quality_sent {
                        None => true,
                        Some((last, at)) => quality_change_due(last, percent, now.duration_since(at)),
                    };
                    if due {
                        self.quality_sent = Some((percent, now));
                        debug!("RustDesk connector: image quality {} % for {} kbit/s", percent, bitrate_kbps);
                        out.push(misc(misc::Union::Option(OptionMessage { custom_image_quality: percent << 8, ..Default::default() })));
                    }
                }
                out
            }
            // ui_session_interface.rs switch_display, with capture_displays
            // as a client that does not render textures sends it.
            Command::SwitchDisplay { index } => {
                if index >= self.displays.len() {
                    return Vec::new();
                }
                let display = index as i32;
                let mut out = vec![misc(misc::Union::SwitchDisplay(SwitchDisplay { display, ..Default::default() }))];
                if clipboard::get_version_number(&self.version) >= clipboard::get_version_number(MIN_VER_MULTI_UI_SESSION) {
                    out.push(misc(misc::Union::CaptureDisplays(crate::rustdesk::proto::CaptureDisplays {
                        set: vec![display],
                        ..Default::default()
                    })));
                }
                out
            }
            // RustDesk's protocol has no game controllers.
            Command::Close | Command::Gamepad { .. } => Vec::new(),
        }
    }

    /// io_loop.rs: `Clipboard` and `MultiClipboards` both end in the
    /// client's clipboard. Text is what is carried (as between Lunaris
    /// computers); other formats (rich text, HTML, pictures, files) are
    /// not. Text too large to carry is not cut short: the viewer is told.
    fn clipboard_from(&mut self, clipboards: &[crate::rustdesk::proto::Clipboard]) -> Option<Event> {
        if let Some(text) = clipboard::text_from(clipboards) {
            return Some(Event::Clipboard { text });
        }
        clipboard::has_oversized_text(clipboards).then(|| Event::ClipboardError { text: CLIPBOARD_TOO_LARGE.into() })
    }

    /// The delay probes of the session, in numbers, for the log (once).
    fn log_probes(&mut self) {
        if std::mem::replace(&mut self.probes_logged, true) {
            return;
        }
        let mut answers = std::mem::take(&mut self.probe_answers_us);
        if answers.is_empty() {
            info!("RustDesk connector: no delay probe came from the computer");
            return;
        }
        answers.sort_unstable();
        info!(
            "RustDesk connector: {} delay probes answered, each within median {} us, max {} us of being read; the computer last measured a delay of {} ms and aimed at {} kbit/s; the connection's own round trip was last {:?} us; the delay told the viewer came {} times from the connection, {} times from the computer",
            answers.len(),
            answers[answers.len() / 2],
            answers[answers.len() - 1],
            self.host_delay_ms,
            self.host_bitrate_kbps,
            self.tcp_rtt_us,
            self.latency_from_tcp,
            self.latency_from_host
        );
    }

    /// The platform as the host named it ("Windows", "Linux", "Mac OS").
    fn platform_name(&self) -> String {
        match self.platform.as_str() {
            "windows" => "Windows".into(),
            "macos" => "Mac OS".into(),
            "android" => "Android".into(),
            _ => "Linux".into(),
        }
    }
}

/// The session after the login (io_loop.rs `io_loop` and
/// `handle_msg_from_peer`). Returns why it ended.
async fn run(
    mut conn: Conn,
    peer_info: PeerInfo,
    wanted: Vec<Codec>,
    fps_percent: u32,
    clipboard_allowed: bool,
    events: &mpsc::Sender<Event>,
    mut commands: mpsc::Receiver<Command>,
) -> String {
    let mut remote = Remote::new(&peer_info);
    remote.fps_percent = fps_percent;
    remote.clipboard_allowed = clipboard_allowed;
    let mut channels: u8 = 2;
    // Test only (see connector/media.rs): answer the host's delay probes late.
    let test_reply_delay = std::env::var(super::media::TEST_DELAY_REPLY_ENV).ok().and_then(|v| v.parse::<u64>().ok()).map(Duration::from_millis);
    // No picture yet: ask for one. A host whose video service is still
    // running from an earlier connection (video_service.rs: a new subscriber
    // is served from `handle_one_frame`, that is, with the next captured
    // frame) sends nothing while its screen is still; the "Refresh" of
    // RustDesk's own client (ui_session_interface.rs `refresh_video`) makes
    // it start over and send a key frame. This safety net is Lunaris's own:
    // RustDesk's client leaves that to the person at the "waiting for image"
    // dialog.
    let mut first_picture = tokio::time::interval_at(tokio::time::Instant::now() + FIRST_PICTURE_WAIT, FIRST_PICTURE_RETRY);
    loop {
        tokio::select! {
            _ = first_picture.tick(), if remote.codec.is_none() => {
                info!("RustDesk connector: no picture yet, asking the computer for one");
                if conn.send(&refresh_message(&remote.version, remote.display)).await.is_err() {
                    return "The connection to the computer broke".into();
                }
            }
            msg = conn.next() => {
                let msg = match msg {
                    Ok(Some(msg)) => msg,
                    Ok(None) => return "The computer closed the connection".into(),
                    Err(e) => return format!("The connection to the computer broke: {}", e),
                };
                // What is not a picture must not wait behind pictures, and
                // must not be lost: it is sent with a wait.
                let mut other: Option<Event> = None;
                match msg.union {
                    Some(message::Union::VideoFrame(video)) => {
                        let Some((codec, frames)) = video.union.and_then(video_frames) else { continue };
                        if !wanted.contains(&codec) {
                            // scrap codec.rs Encoder::update decides by the
                            // abilities sent; a host without an encoder for
                            // them falls back to VP9.
                            warn!("RustDesk connector: the computer sends {:?}, which the viewer did not ask for", codec);
                            return format!("The computer can only send {:?} video, which this viewer cannot show", codec);
                        }
                        remote.codec = Some(codec);
                        for f in frames.frames {
                            let event = Event::Video {
                                codec,
                                key: f.key,
                                capture_time_ms: u64::try_from(f.pts).ok(),
                                data: f.data,
                            };
                            // A full queue holds the reading back: the host
                            // sees it as a slow network and lowers its rate.
                            if events.send(event).await.is_err() {
                                return "The viewer left".into();
                            }
                        }
                    }
                    // client.rs AudioHandler::handle_format / handle_frame:
                    // the format, then one Opus packet per AudioFrame.
                    Some(message::Union::Misc(Misc { union: Some(misc::Union::AudioFormat(format)) })) => {
                        if format.sample_rate != 48000 {
                            warn!("RustDesk connector: sound at {} Hz is not forwarded", format.sample_rate);
                            channels = 0;
                        } else {
                            channels = format.channels.clamp(1, 2) as u8;
                        }
                    }
                    Some(message::Union::AudioFrame(audio)) => {
                        if channels != 0 && events.try_send(Event::Audio { channels, data: audio.data }).is_err() && events.is_closed() {
                            return "The viewer left".into();
                        }
                    }
                    // client.rs handle_test_delay: the host's probe goes back as it came.
                    // The host measures the way there and back with it and
                    // sets its frame rate by what it measures (video_qos.rs
                    // `user_network_delay`): answered at once, before
                    // anything else is done with what was read.
                    Some(message::Union::TestDelay(delay)) => {
                        if !delay.from_client {
                            let read_at = std::time::Instant::now();
                            if let Some(wait) = test_reply_delay {
                                tokio::time::sleep(wait).await;
                            }
                            let last_delay = delay.last_delay;
                            remote.host_delay_ms = last_delay;
                            remote.host_bitrate_kbps = delay.target_bitrate;
                            if conn.send(&Message { union: Some(message::Union::TestDelay(delay)) }).await.is_err() {
                                return "The connection to the computer broke".into();
                            }
                            remote.probe_answers_us.push(read_at.elapsed().as_micros().min(u32::MAX as u128) as u32);
                            // After the answer went: the viewer's ping (see `latency_event`).
                            other = remote.latency_event(last_delay, tcp_round_trip_us(&conn.stream));
                        }
                    }
                    Some(message::Union::Misc(Misc { union: Some(misc::Union::CloseReason(reason)) })) => {
                        remote.log_probes();
                        return if reason.is_empty() { "The computer closed the connection".into() } else { reason };
                    }
                    // io_loop.rs: the host shows another display, or the
                    // one shown changed its size.
                    Some(message::Union::Misc(Misc { union: Some(misc::Union::SwitchDisplay(switch)) })) => {
                        remote.display = switch.display;
                        if let Some(display) = remote.displays.get_mut(switch.display.max(0) as usize) {
                            display.x = switch.x;
                            display.y = switch.y;
                            if switch.width > 0 && switch.height > 0 {
                                display.width = switch.width;
                                display.height = switch.height;
                            }
                        }
                        other = Some(remote.displays_event());
                    }
                    // io_loop.rs handle_peer_info: the displays changed.
                    Some(message::Union::PeerInfo(info)) => {
                        if !info.displays.is_empty() {
                            remote.displays = info.displays;
                            remote.display = remote.display.clamp(0, remote.displays.len() as i32 - 1);
                            other = Some(remote.displays_event());
                        }
                    }
                    Some(message::Union::CursorData(data)) => other = remote.cursor_shape(data),
                    Some(message::Union::CursorId(id)) => {
                        other = remote.cursors.iter().find(|(known, _)| *known == id).map(|(_, shape)| shape.clone());
                        if let Some(shape) = other.as_ref() {
                            remote.note_cursor(shape);
                        }
                    }
                    Some(message::Union::CursorPosition(position)) => {
                        let (origin_x, origin_y, _, _) = remote.shown();
                        // The newest position is enough: not waited for.
                        let _ = events.try_send(Event::CursorPosition { x: position.x - origin_x, y: position.y - origin_y, visible: true });
                    }
                    // io_loop.rs: `Clipboard` and `MultiClipboards` both end
                    // in the client's clipboard; text is what is carried.
                    Some(message::Union::Clipboard(one)) => other = remote.clipboard_from(&[one]),
                    Some(message::Union::MultiClipboards(many)) => other = remote.clipboard_from(&many.clipboards),
                    // connection.rs: a permission switched at the computer
                    // while connected.
                    Some(message::Union::Misc(Misc { union: Some(misc::Union::PermissionInfo(info)) })) => {
                        if info.permission == permission_info::Permission::Clipboard as i32 {
                            remote.clipboard_allowed = info.enabled;
                            remote.clipboard_refusal_told = false;
                        }
                    }
                    _ => {}
                }
                if let Some(event) = other {
                    if events.send(event).await.is_err() {
                        return "The viewer left".into();
                    }
                }
                for event in std::mem::take(&mut remote.pending) {
                    if events.send(event).await.is_err() {
                        return "The viewer left".into();
                    }
                }
            }
            command = commands.recv() => {
                let command = match command {
                    None | Some(Command::Close) => {
                        // io_loop.rs send_close_reason, then the connection ends.
                        let _ = conn.send(&misc(misc::Union::CloseReason(String::new()))).await;
                        let _ = conn.stream.shutdown().await;
                        remote.log_probes();
                        return "Closed".into();
                    }
                    Some(command) => command,
                };
                for out in remote.command(command) {
                    if conn.send(&out).await.is_err() {
                        return "The connection to the computer broke".into();
                    }
                }
                for event in std::mem::take(&mut remote.pending) {
                    if events.send(event).await.is_err() {
                        return "The viewer left".into();
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn remote(platform: &str) -> Remote {
        let mut display = crate::rustdesk::proto::DisplayInfo::default();
        display.width = 1920;
        display.height = 1080;
        Remote::new(&PeerInfo { platform: platform.into(), version: "1.4.9".into(), displays: vec![display], ..Default::default() })
    }

    fn shape(id: u64, hot: (i32, i32)) -> CursorData {
        let rgba = vec![255u8; 8 * 8 * 4];
        CursorData { id, hotx: hot.0, hoty: hot.1, width: 8, height: 8, colors: Bytes::from(cursor::compress(&rgba)), ..Default::default() }
    }

    fn left(down: bool) -> Command {
        Command::PointerButton { button: Button::Left, down }
    }

    /// What the viewer is told of the pointer being in the picture.
    fn told(remote: &mut Remote) -> Vec<bool> {
        std::mem::take(&mut remote.pending)
            .into_iter()
            .filter_map(|event| match event {
                Event::CursorMaybeEmbedded { maybe } => Some(maybe),
                _ => None,
            })
            .collect()
    }

    /// The host's delay probes become the delay the viewer adds to its ping.
    #[test]
    fn the_delay_the_host_measured_is_told_as_a_round_trip_when_it_changes() {
        // Without the kernel's figure (not Linux): the host's, as it is.
        let mut r = remote("Windows");
        // The first probe carries the host's starting 0: no measurement.
        assert_eq!(r.latency_event(0, None), None);
        // From the second on it is the round trip of the probe before.
        assert_eq!(r.latency_event(23, None), Some(Event::Latency { ms: 23.0 }));
        assert_eq!(r.latency_event(23, None), None, "the same again says nothing");
        assert_eq!(r.latency_event(31, None), Some(Event::Latency { ms: 31.0 }));
        // Under a millisecond is a measurement too.
        assert_eq!(r.latency_event(0, None), Some(Event::Latency { ms: 0.0 }));
        // A host whose first probe already measured something: still skipped once.
        let mut r = remote("Linux");
        assert_eq!(r.latency_event(40, None), None);
        assert_eq!(r.latency_event(40, None), Some(Event::Latency { ms: 40.0 }));
        assert_eq!((r.latency_from_tcp, r.latency_from_host), (0, 1));
    }

    #[test]
    fn the_connections_own_round_trip_is_told_when_it_is_the_better_figure() {
        use DelaySource::{Host, Tcp};
        // Only one of the two.
        assert_eq!(target_round_trip_ms(None, None), None);
        assert_eq!(target_round_trip_ms(Some(7), None), Some((7.0, Host)));
        assert_eq!(target_round_trip_ms(None, Some(350)), Some((0.35, Tcp)));
        // Both, and they agree: the finer one (the host cuts 0.35 ms to 0, 12.8 to 12).
        assert_eq!(target_round_trip_ms(Some(0), Some(350)), Some((0.35, Tcp)));
        assert_eq!(target_round_trip_ms(Some(12), Some(12_800)), Some((12.8, Tcp)));
        // The host's probe waited behind pictures: the connection's.
        assert_eq!(target_round_trip_ms(Some(85), Some(12_800)), Some((12.8, Tcp)));
        // The acknowledgements were kept back at the host (40 ms and more):
        // more than the host can have measured, so the host's.
        assert_eq!(target_round_trip_ms(Some(12), Some(52_000)), Some((12.0, Host)));
        assert_eq!(target_round_trip_ms(Some(12), Some(13_001)), Some((12.0, Host)));
        assert_eq!(target_round_trip_ms(Some(12), Some(13_000)), Some((13.0, Tcp)));

        // In a session: the connection's figure from the first probe on,
        // in tenths of a millisecond, told when it changed.
        let mut r = remote("Windows");
        assert_eq!(r.latency_event(0, Some(420)), Some(Event::Latency { ms: 0.4 }));
        assert_eq!(r.latency_event(0, Some(440)), None, "still 0.4 ms");
        assert_eq!(r.latency_event(0, Some(460)), Some(Event::Latency { ms: 0.5 }));
        assert_eq!(r.latency_event(3, Some(47_000)), Some(Event::Latency { ms: 3.0 }));
        assert_eq!((r.latency_from_tcp, r.latency_from_host), (3, 1));
    }

    #[test]
    fn dragging_with_the_arrow_on_windows_is_reported_as_the_pointer_in_the_picture() {
        let mut r = remote("Windows");
        // Before any shape came the cursor is taken for the arrow.
        r.command(Command::PointerMove { x: 500, y: 20 });
        r.command(left(true));
        // Pressed, not moved yet: a click is not a drag.
        r.command(Command::PointerMove { x: 502, y: 21 });
        assert!(told(&mut r).is_empty());
        r.command(Command::PointerMove { x: 540, y: 60 });
        r.command(Command::PointerMove { x: 600, y: 90 });
        assert_eq!(told(&mut r), [true], "told once, when the drag begins");
        r.command(left(false));
        assert_eq!(told(&mut r), [false], "and that it is over when the button is let go");
        // Moving without the button says nothing; nor does another button.
        r.command(Command::PointerMove { x: 100, y: 100 });
        r.command(Command::PointerButton { button: Button::Right, down: true });
        r.command(Command::PointerMove { x: 300, y: 300 });
        r.command(Command::PointerButton { button: Button::Right, down: false });
        assert!(told(&mut r).is_empty());
        // A viewer that holds the pointer sends how far it moved.
        r.command(left(true));
        r.command(Command::PointerMoveRelative { dx: 3, dy: 0 });
        assert!(told(&mut r).is_empty());
        r.command(Command::PointerMoveRelative { dx: 3, dy: 0 });
        assert_eq!(told(&mut r), [true]);
        r.command(left(false));
        assert_eq!(told(&mut r), [false]);
    }

    #[test]
    fn a_drag_that_begins_on_another_cursor_or_on_another_system_is_not() {
        let mut r = remote("Windows");
        // The text cursor (hot spot in its middle), as a shape and later by its id.
        assert!(r.cursor_shape(shape(1, (0, 0))).is_some());
        assert!(r.cursor_is_arrow);
        assert!(r.cursor_shape(shape(2, (4, 4))).is_some());
        assert!(!r.cursor_is_arrow);
        r.command(left(true));
        r.command(Command::PointerMove { x: 200, y: 200 });
        r.command(left(false));
        assert!(told(&mut r).is_empty(), "selecting text keeps the viewer's pointer");
        // Back to the arrow by its id (the host sends a shape once): a drag counts again.
        let arrow = r.cursors.iter().find(|(id, _)| *id == 1).map(|(_, s)| s.clone()).unwrap();
        r.note_cursor(&arrow);
        r.command(left(true));
        r.command(Command::PointerMove { x: 260, y: 260 });
        assert_eq!(told(&mut r), [true]);
        // The cursor changing during the drag (a window being moved keeps its
        // state until the button is let go).
        assert!(r.cursor_shape(shape(3, (4, 4))).is_some());
        r.command(Command::PointerMove { x: 300, y: 300 });
        assert!(told(&mut r).is_empty());
        r.command(left(false));
        assert_eq!(told(&mut r), [false]);

        // A host that is not Windows draws no pointer into its picture this way.
        for platform in ["Linux", "Mac OS", "Android"] {
            let mut r = remote(platform);
            r.command(left(true));
            r.command(Command::PointerMove { x: 400, y: 400 });
            r.command(left(false));
            assert!(told(&mut r).is_empty(), "{}", platform);
        }
        assert_eq!(platform_of(&PeerInfo { platform: "Mac OS".into(), ..Default::default() }), "macos");
        assert_eq!(platform_of(&PeerInfo { platform: "Windows".into(), ..Default::default() }), "windows");
    }

    #[test]
    fn the_clipboard_is_text_and_what_cannot_be_carried_is_said() {
        use crate::rustdesk::proto::{Clipboard, ClipboardFormat};
        let mut r = remote("Windows");
        let text = |s: &str| match clipboard::text_message(s, "1.4.9", "Windows").union {
            Some(message::Union::MultiClipboards(many)) => many.clipboards,
            Some(message::Union::Clipboard(one)) => vec![one],
            _ => unreachable!(),
        };
        // Vietnamese text, short and long (the long one goes compressed).
        let short = "Tiếng Việt có dấu: ắ ằ ẳ ẵ ặ";
        assert_eq!(r.clipboard_from(&text(short)), Some(Event::Clipboard { text: short.into() }));
        let long = "Xin chào thế giới. ".repeat(20_000);
        let sent = text(&long);
        assert!(sent[0].compress && sent[0].content.len() < long.len() / 10);
        assert_eq!(r.clipboard_from(&sent), Some(Event::Clipboard { text: long.clone() }));
        // Text among other formats: the text is taken, the rest left.
        let html = Clipboard { format: ClipboardFormat::Html as i32, content: Bytes::from_static(b"<b>x</b>"), ..Default::default() };
        let mut mixed = vec![html.clone()];
        mixed.extend(text("plain"));
        assert_eq!(r.clipboard_from(&mixed), Some(Event::Clipboard { text: "plain".into() }));
        // Only another format: nothing, and nothing said.
        assert_eq!(r.clipboard_from(&[html]), None);
        // More than a megabyte of text: not cut short, the viewer is told.
        let huge = "0123456789".repeat(110_000);
        assert_eq!(r.clipboard_from(&text(&huge)), Some(Event::ClipboardError { text: CLIPBOARD_TOO_LARGE.into() }));
        let plain_huge = Clipboard { format: ClipboardFormat::Text as i32, content: Bytes::from(huge.into_bytes()), ..Default::default() };
        assert_eq!(r.clipboard_from(&[plain_huge]), Some(Event::ClipboardError { text: CLIPBOARD_TOO_LARGE.into() }));

        // To the host: as a real client sends it, unless the host does not allow it.
        assert_eq!(r.command(Command::Clipboard { text: short.into() }).len(), 1);
        assert!(r.pending.is_empty());
        r.clipboard_allowed = false;
        assert!(r.command(Command::Clipboard { text: "a".into() }).is_empty());
        assert!(r.command(Command::Clipboard { text: "b".into() }).is_empty());
        assert_eq!(std::mem::take(&mut r.pending), [Event::ClipboardError { text: CLIPBOARD_NOT_ALLOWED.into() }], "told once");
    }

    #[test]
    fn the_image_quality_follows_the_viewers_bitrate_calmly() {
        let s = Duration::from_secs;
        // Down: by 15 % or more, 2 s after the last change at the earliest.
        assert!(quality_change_due(300, 250, s(2)));
        assert!(!quality_change_due(300, 250, s(1)));
        assert!(!quality_change_due(300, 270, s(60)));
        // Up: by 25 % or more, 10 s after.
        assert!(quality_change_due(200, 250, s(10)));
        assert!(!quality_change_due(200, 250, s(9)));
        assert!(!quality_change_due(200, 240, s(60)));
        assert!(!quality_change_due(200, 200, s(60)));

        // As commands: the first wish is sent, a wobble is not, a real drop is (later).
        let mut r = remote("Windows");
        r.codec = Some(Codec::H264);
        let sent = |r: &mut Remote, kbps: u32| !r.command(Command::Quality { fps: 0, bitrate_kbps: kbps }).is_empty();
        assert!(sent(&mut r, 15_000));
        assert!(!sent(&mut r, 14_000));
        assert!(!sent(&mut r, 6_000), "not within 2 s of the last change");
        r.quality_sent = r.quality_sent.map(|(percent, at)| (percent, at - s(3)));
        assert!(sent(&mut r, 6_000));
        // The frame rate wanted is asked for every time, a little raised
        // (the host delivers about 97 % of what it is asked for).
        assert_eq!(r.command(Command::Quality { fps: 60, bitrate_kbps: 0 }).len(), 1);
        assert_eq!((asked_fps(60, DEFAULT_FPS_PERCENT), asked_fps(30, DEFAULT_FPS_PERCENT), asked_fps(144, DEFAULT_FPS_PERCENT)), (62, 31, 120));
        assert_eq!((asked_fps(60, 100), asked_fps(60, 120), asked_fps(0, 103)), (60, 72, 1));
    }
}
