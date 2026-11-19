//! One RDP connection, from the TCP connection to its end.
//!
//! The connection sequence and the session loop follow IronRDP's own client,
//! `crates/ironrdp-client/src/rdp.rs` of github.com/Devolutions/IronRDP at
//! the commit that publishes ironrdp-connector 0.12 / ironrdp-session 0.13
//! (abfc97b, November 2026): `connect_direct` and
//! `security_upgrade_and_finalize` (X.224 negotiation, TLS, CredSSP, the
//! rest of the sequence), `active_session` (frames in, input out, the
//! Deactivation-Reactivation Sequence, bitmap recovery), and the
//! `screenshot` example of the same commit for the connector's settings.
//! What differs from it is said where it differs.

use std::sync::Arc;
use std::time::Duration;

use ironrdp_connector::connection_activation::ConnectionActivationState;
use ironrdp_connector::{ClientConnector, ConnectionResult, ConnectorErrorKind, Credentials, DesktopSize, ServerName};
use ironrdp_core::WriteBuf;
use ironrdp_graphics::image_processing::PixelFormat;
use ironrdp_graphics::pointer::DecodedPointer;
use ironrdp_input::{Database, MousePosition, Operation, Scancode, WheelRotations};
use ironrdp_pdu::gcc::{ConnectionType, KeyboardType};
use ironrdp_pdu::geometry::InclusiveRectangle;
use ironrdp_pdu::input::fast_path::FastPathInputEvent;
use ironrdp_pdu::rdp::capability_sets::{InputFlags, MajorPlatformType};
use ironrdp_pdu::rdp::client_info::{CompressionType, PerformanceFlags, TimezoneInfo};
use ironrdp_session::image::DecodedImage;
use ironrdp_session::{ActiveStage, ActiveStageBuilder, ActiveStageOutput};
use ironrdp_tokio::{single_sequence_step_read, split_tokio_framed, FramedWrite, TokioFramed};
use tokio::net::TcpStream;
use tokio::sync::mpsc;
use tracing::{debug, info, warn};

use super::clipboard::{ClipboardTask, Tasks, TextBackend};
use super::{tls, Security};
use crate::connector::raw_video::{EncoderChoice, RawVideo};
use crate::connector::{keys, Button, Command, Event, Session, Target, Viewer};

pub const DEFAULT_PORT: u16 = 3389;
/// Size asked for when neither the target nor the viewer names one.
pub const FALLBACK_SIZE: (u16, u16) = (1920, 1080);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
/// The whole connection sequence (CredSSP, licensing, capabilities).
const SEQUENCE_TIMEOUT: Duration = Duration::from_secs(30);
/// Longest wait for the server to end the session after a Shutdown Request.
const SHUTDOWN_WAIT: Duration = Duration::from_secs(2);
/// A wheel notch, in rotation units: Windows' WHEEL_DELTA. ironrdp-activex
/// `control.rs` (`wheel_operations`) passes the delta of WM_MOUSEWHEEL /
/// WM_MOUSEHWHEEL as it is, so a notch up / right is +120.
const WHEEL_NOTCH: i32 = 120;

/// A target's fields, read.
#[derive(Clone)]
pub struct Options {
    pub host: String,
    pub port: u16,
    pub username: String,
    pub password: String,
    pub domain: Option<String>,
    pub security: Security,
    /// `None` = the viewer's size.
    pub size: Option<(u16, u16)>,
    /// The certificate pinned at set-up (SHA-256 hex).
    pub pinned: Option<String>,
}

impl Options {
    pub fn of(target: &Target) -> anyhow::Result<Options> {
        let host = target.get("host").trim().to_string();
        if host.is_empty() {
            anyhow::bail!("The computer's address is missing");
        }
        let port = match target.get("port").trim() {
            "" => DEFAULT_PORT,
            p => p.parse::<u16>().ok().filter(|p| *p != 0).ok_or_else(|| anyhow::anyhow!("\"{p}\" is not a port"))?,
        };
        let security = Security::of(target.get("security"))?;
        let size = match target.get("resolution").trim() {
            "" | "auto" => None,
            text => Some(parse_size(text).ok_or_else(|| anyhow::anyhow!("\"{text}\" is not a resolution (for example 1920x1080, or auto)"))?),
        };
        let domain = Some(target.get("domain").trim().to_string()).filter(|d| !d.is_empty());
        let pinned = Some(target.get(super::PIN_KEY).trim().to_string()).filter(|p| !p.is_empty());
        Ok(Options {
            host,
            port,
            username: target.get("username").trim().to_string(),
            password: target.get("password").to_string(),
            domain,
            security,
            size,
            pinned,
        })
    }

    /// The size asked of the server: the target's, else the viewer's, else
    /// 1920x1080; fitted as ironrdp-displaycontrol
    /// `MonitorLayoutEntry::adjust_display_size` fits a size (MS-RDPEDISP
    /// 2.2.2.2.1: 200 to 8192, the width even), which keeps it valid for the
    /// Client Core Data's desktopWidth / desktopHeight too (MS-RDPBCGR
    /// 2.2.1.3.2: at most 8192).
    pub fn desktop_size(&self, viewer: &Viewer) -> DesktopSize {
        let (w, h) = self.size.unwrap_or_else(|| {
            if viewer.width > 0 && viewer.height > 0 {
                (viewer.width.min(8192) as u16, viewer.height.min(8192) as u16)
            } else {
                FALLBACK_SIZE
            }
        });
        let (w, h) = ironrdp_displaycontrol::pdu::MonitorLayoutEntry::adjust_display_size(w as u32, h as u32);
        DesktopSize { width: w as u16, height: h as u16 }
    }
}

pub fn parse_size(text: &str) -> Option<(u16, u16)> {
    let (w, h) = text.trim().split_once(['x', 'X', '*'])?;
    let w = w.trim().parse::<u16>().ok()?;
    let h = h.trim().parse::<u16>().ok()?;
    (w >= 200 && h >= 200 && w <= 8192 && h <= 8192).then_some((w, h))
}

/// The connector's settings (ironrdp `screenshot.rs` `build_config`, with
/// the pointer drawn by the viewer: `enable_server_pointer` and no software
/// rendering, as ironrdp-client's own default).
pub fn connector_config(options: &Options, size: DesktopSize) -> ironrdp_connector::Config {
    let (enable_tls, enable_credssp) = match options.security {
        Security::Nla => (false, true),
        Security::Tls => (true, false),
        Security::Any => (true, true),
    };
    ironrdp_connector::Config {
        credentials: Credentials::UsernamePassword { username: options.username.clone(), password: options.password.clone() },
        domain: options.domain.clone(),
        enable_tls,
        enable_credssp,
        enable_standard_rdp_security: false,
        keyboard_type: KeyboardType::IBM_ENHANCED,
        keyboard_subtype: 0,
        // 0: the server's own layout (keys are sent by scan code, where they are).
        keyboard_layout: 0,
        keyboard_functional_keys_count: 12,
        connection_type: ConnectionType::Lan,
        ime_file_name: String::new(),
        dig_product_id: String::new(),
        desktop_size: size,
        monitor_layout: None,
        bitmap: None,
        client_build: 0,
        client_name: "Lunaris".to_owned(),
        client_dir: "C:\\Windows\\System32\\mstscax.dll".to_owned(),
        #[cfg(windows)]
        platform: MajorPlatformType::WINDOWS,
        #[cfg(target_os = "macos")]
        platform: MajorPlatformType::MACINTOSH,
        #[cfg(not(any(windows, target_os = "macos")))]
        platform: MajorPlatformType::UNIX,
        enable_server_pointer: true,
        request_data: None,
        // The password given is used to log on (INFO_AUTOLOGON, MS-RDPBCGR
        // 2.2.1.11.1.1); without one the server shows its logon screen.
        autologon: !options.password.is_empty(),
        enable_audio_playback: false,
        enable_audio_capture: false,
        // The screenshot example's default (its compression level 3).
        compression_type: Some(CompressionType::Rdp61),
        pointer_software_rendering: false,
        multitransport_flags: None,
        support_dyn_vc_gfx_protocol: false,
        performance_flags: PerformanceFlags::default(),
        desktop_scale_factor: 0,
        desktop_physical_size: None,
        desktop_orientation: None,
        device_scale_factor: None,
        monitor_extended_layout: None,
        relative_mouse_input: false,
        hardware_id: None,
        license_cache: None,
        timezone_info: TimezoneInfo::default(),
        alternate_shell: String::new(),
        work_dir: String::new(),
        remote_application_mode: false,
        rail_support_level: ironrdp_pdu::rdp::capability_sets::RailSupportLevel::empty(),
    }
}

/// CredSSP may ask for Kerberos tickets through this (ironrdp-client uses
/// a reqwest client to reach a KDC). Lunaris has no Kerberos configuration:
/// every request fails, and sspi's Negotiate uses NTLM.
pub struct NoKerberos;

impl ironrdp_tokio::NetworkClient for NoKerberos {
    async fn send(&mut self, _request: &ironrdp_connector::sspi::generator::NetworkRequest) -> ironrdp_connector::ConnectorResult<Vec<u8>> {
        Err(ironrdp_connector::general_err!("Kerberos (KDC) is not supported; NTLM is used"))
    }
}

/// Words for the person, from an error of the connection sequence. Never
/// holds the password (IronRDP's errors do not carry it).
pub fn explain(error: &ironrdp_connector::ConnectorError, security: Security) -> String {
    match error.kind() {
        ConnectorErrorKind::Credssp(_) => "The user name or password is not correct, or this account may not sign in remotely (NLA)".to_string(),
        ConnectorErrorKind::AccessDenied => "The computer denied access to this account".to_string(),
        ConnectorErrorKind::Negotiation(failure) => {
            let text = failure.to_string();
            let hint = match security {
                Security::Nla => " The computer may not offer NLA: try Security = TLS.",
                Security::Tls => " The computer may require NLA: try Security = NLA.",
                Security::Any => "",
            };
            format!("The computer refused the security asked for ({text}).{hint}")
        }
        _ => format!("The RDP connection failed ({error})"),
    }
}

/// Whether to offer the graphics pipeline (MS-RDPEGFX). On unless
/// `LUNARIS_RDP_GFX=0` (bitmap updates only, as before; for comparing).
fn graphics_pipeline_wanted() -> bool {
    std::env::var("LUNARIS_RDP_GFX").map(|v| v.trim() != "0").unwrap_or(true)
}

/// Says in the log what the graphics pipeline agreed on.
struct PipelineLog;

impl ironrdp_egfx::client::GraphicsPipelineHandler for PipelineLog {
    fn on_capabilities_confirmed(&mut self, caps: &ironrdp_egfx::pdu::CapabilitySet) {
        info!("RDP: graphics pipeline in use ({caps:?})");
    }

    fn on_reset_graphics(&mut self, width: u32, height: u32) {
        debug!("RDP: graphics pipeline reset to {width}x{height}");
    }
}

/// The connection sequence up to the active session (ironrdp-client
/// `connect_direct` + `security_upgrade_and_finalize`).
pub async fn connect_sequence(
    options: &Options,
    size: DesktopSize,
    clipboard: Option<Tasks>,
) -> anyhow::Result<(ConnectionResult, TokioFramed<tokio_rustls::client::TlsStream<TcpStream>>, RttFd, String)> {
    let address = format!("{}:{}", options.host, options.port);
    let stream = match tokio::time::timeout(CONNECT_TIMEOUT, TcpStream::connect(&address)).await {
        Ok(Ok(stream)) => stream,
        Ok(Err(e)) => anyhow::bail!("The computer could not be reached ({e})"),
        Err(_) => anyhow::bail!("The computer did not answer"),
    };
    let _ = stream.set_nodelay(true);
    let client_addr = stream.local_addr()?;
    #[cfg(unix)]
    let rtt_fd = {
        use std::os::fd::AsFd;
        stream.as_fd().try_clone_to_owned().ok()
    };
    #[cfg(not(unix))]
    let rtt_fd: RttFd = None;

    let mut config = connector_config(options, size);
    let gfx = graphics_pipeline_wanted();
    // MS-RDPEGFX 1.5.1: the flag is set only with the channel registered.
    config.support_dyn_vc_gfx_protocol = gfx;
    let mut connector = ClientConnector::new(config, client_addr);
    if let Some(tasks) = clipboard {
        connector.attach_static_channel(ironrdp_cliprdr::CliprdrClient::new(Box::new(TextBackend::new(tasks))));
    }
    if gfx {
        // The graphics pipeline: the server sends whole frames
        // (StartFrame..EndFrame) and ActiveStage composites each one into
        // the picture when it is complete, so a change never shows half
        // drawn. No H.264 decoder is given: the client then offers only the
        // capability sets without AVC, and the server uses RemoteFX /
        // progressive / planar / ClearCodec, which ironrdp-egfx decodes. A
        // server that does not open the channel keeps sending bitmaps.
        let pipeline = ironrdp_egfx::client::GraphicsPipelineClient::new(Box::new(PipelineLog), None);
        connector.attach_static_channel(ironrdp_dvc::DrdynvcClient::new().with_dynamic_channel(pipeline));
    }
    let mut framed = TokioFramed::new(stream);
    let should_upgrade = ironrdp_tokio::connect_begin(&mut framed, &mut connector).await.map_err(|e| anyhow::anyhow!(explain(&e, options.security)))?;
    // The X.224 negotiation is over; no RDP byte may be left over before TLS
    // (ironrdp `screenshot.rs`: `into_inner_no_leftover`).
    let initial = framed.into_inner_no_leftover();
    let pinned = options.pinned.as_deref().ok_or_else(|| {
        anyhow::anyhow!("This computer has no pinned certificate: remove it here and add it again")
    })?;
    let upgraded = tls::upgrade(initial, &options.host, Some(pinned)).await.map_err(|e| anyhow::anyhow!("{e}"))?;
    let fingerprint = upgraded.fingerprint.clone();
    let upgraded_marker = ironrdp_tokio::mark_as_upgraded(should_upgrade, &mut connector);
    let mut framed = TokioFramed::new(upgraded.stream);
    let outcome = ironrdp_tokio::connect_finalize(
        upgraded_marker,
        connector,
        &mut framed,
        &mut NoKerberos,
        ServerName::new(options.host.clone()),
        upgraded.public_key,
        None,
    )
    .await
    .map_err(|e| anyhow::anyhow!(explain(&e, options.security)))?;
    let result = outcome.into_connected().map_err(|_| anyhow::anyhow!("The computer redirects the connection to another server, which Lunaris does not follow"))?;
    Ok((result, framed, rtt_fd, fingerprint))
}

pub async fn connect(target: &Target, viewer: &Viewer, choice: EncoderChoice) -> anyhow::Result<Session> {
    let options = Options::of(target)?;
    if options.security == Security::Nla && options.username.is_empty() {
        anyhow::bail!("NLA needs a user name: enter it in this computer's settings, or choose Security = TLS");
    }
    let size = options.desktop_size(viewer);
    let tasks: Tasks = Default::default();
    let began = std::time::Instant::now();
    let (result, framed, rtt_fd, _) = tokio::time::timeout(SEQUENCE_TIMEOUT, connect_sequence(&options, size, Some(tasks.clone())))
        .await
        .map_err(|_| anyhow::anyhow!("The computer did not finish the RDP connection sequence"))??;
    info!(
        "RDP: connected ({:?}, {}x{} asked, {}x{} given, {} ms)",
        options.security,
        size.width,
        size.height,
        result.desktop_size.width,
        result.desktop_size.height,
        began.elapsed().as_millis()
    );

    let (events_tx, events_rx) = mpsc::channel(64);
    let (commands_tx, commands_rx) = mpsc::channel(256);
    let video = RawVideo::start(viewer, events_tx.clone(), "RDP", choice);
    video.resize(result.desktop_size.width as u32, result.desktop_size.height as u32);
    tokio::spawn(run(result, framed, video, events_tx, commands_rx, tasks, rtt_fd));
    Ok(Session { events: events_rx, commands: commands_tx })
}

/// A built-in arrow for the server's "default pointer" (MS-RDPBCGR
/// 2.2.9.1.1.4.3, SYSPTR_DEFAULT: the client shows its own default pointer;
/// Lunaris's viewer has none of the other computer's, so this one): 11x16,
/// black with a white outline, hot spot at the tip.
pub fn default_arrow() -> Event {
    const ROWS: [&str; 16] = [
        "X..........",
        "XX.........",
        "XOX........",
        "XOOX.......",
        "XOOOX......",
        "XOOOOX.....",
        "XOOOOOX....",
        "XOOOOOOX...",
        "XOOOOOOOX..",
        "XOOOOOOOOX.",
        "XOOOOOXXXXX",
        "XOOXOOX....",
        "XOX.XOOX...",
        "XX..XOOX...",
        "X....XOOX..",
        ".....XXXX..",
    ];
    let mut rgba = Vec::with_capacity(11 * 16 * 4);
    for row in ROWS {
        for c in row.bytes() {
            rgba.extend_from_slice(match c {
                b'X' => &[255, 255, 255, 255],
                b'O' => &[0, 0, 0, 255],
                _ => &[0, 0, 0, 0],
            });
        }
    }
    Event::CursorShape { width: 11, height: 16, hot_x: 0, hot_y: 0, rgba: rgba.into() }
}

fn pointer_shape(pointer: &DecodedPointer) -> Event {
    Event::CursorShape {
        width: pointer.width as u32,
        height: pointer.height as u32,
        hot_x: pointer.hotspot_x as u32,
        hot_y: pointer.hotspot_y as u32,
        // Accelerated target: RGBA, not premultiplied (ironrdp-graphics
        // pointer.rs `PointerBitmapTarget::Accelerated`).
        rgba: pointer.bitmap_data.clone().into(),
    }
}

/// Copies the changed rectangle of IronRDP's picture into ours.
fn copy_region(image: &DecodedImage, video: &RawVideo, rect: &InclusiveRectangle) {
    let (iw, ih) = (image.width() as u32, image.height() as u32);
    let left = rect.left as u32;
    let top = rect.top as u32;
    if left >= iw || top >= ih {
        return;
    }
    let right = (rect.right as u32).min(iw - 1);
    let bottom = (rect.bottom as u32).min(ih - 1);
    let w = right + 1 - left;
    let stride = image.stride();
    let data = image.data();
    video.update(|fb| {
        if fb.width != iw || fb.height != ih {
            return;
        }
        let fb_stride = fb.stride();
        for y in top..=bottom {
            let src = y as usize * stride + left as usize * 4;
            let dst = y as usize * fb_stride + left as usize * 4;
            fb.data[dst..dst + w as usize * 4].copy_from_slice(&data[src..src + w as usize * 4]);
        }
        fb.mark_dirty();
    });
}

/// The input a command makes (ironrdp-input keeps what is held, so a
/// release is sent only for what was pressed and `release_all` lets
/// everything go at the end).
fn input_events(
    command: &Command,
    database: &mut Database,
    size: (u16, u16),
    horizontal_wheel: bool,
) -> Vec<FastPathInputEvent> {
    let clamp = |x: i64, y: i64| MousePosition {
        x: x.clamp(0, size.0.saturating_sub(1) as i64) as u16,
        y: y.clamp(0, size.1.saturating_sub(1) as i64) as u16,
    };
    let button = |b: Button| match b {
        Button::Left => ironrdp_input::MouseButton::Left,
        Button::Middle => ironrdp_input::MouseButton::Middle,
        Button::Right => ironrdp_input::MouseButton::Right,
        Button::Back => ironrdp_input::MouseButton::X1,
        Button::Forward => ironrdp_input::MouseButton::X2,
    };
    let ops: Vec<Operation> = match command {
        Command::PointerMove { x, y } => vec![Operation::MouseMove(clamp(*x as i64, *y as i64))],
        Command::PointerMoveRelative { dx, dy } => {
            let at = database.mouse_position();
            vec![Operation::MouseMove(clamp(at.x as i64 + *dx as i64, at.y as i64 + *dy as i64))]
        }
        Command::PointerButton { button: b, down: true } => vec![Operation::MouseButtonPressed(button(*b))],
        Command::PointerButton { button: b, down: false } => vec![Operation::MouseButtonReleased(button(*b))],
        Command::Wheel { dx, dy } => {
            let mut ops = Vec::new();
            if *dy != 0 {
                ops.push(Operation::WheelRotations(WheelRotations { is_vertical: true, rotation_units: (dy * WHEEL_NOTCH).clamp(-255, 255) as i16 }));
            }
            // MS-RDPBCGR 2.2.8.1.1.3.1.1.3: horizontal wheel events only
            // when the server's Input Capability Set has TS_MOUSE_HWHEEL
            // (ironrdp-server capabilities.rs notes the same).
            if *dx != 0 && horizontal_wheel {
                ops.push(Operation::WheelRotations(WheelRotations { is_vertical: false, rotation_units: (dx * WHEEL_NOTCH).clamp(-255, 255) as i16 }));
            }
            ops
        }
        Command::Key { position, down } => match keys::win_scancode_of(*position) {
            // 0: a key with no plain scan code (Pause, which is a sequence).
            Some(code) if code != 0 => {
                let scancode = Scancode::from_u16(code as u16);
                vec![if *down { Operation::KeyPressed(scancode) } else { Operation::KeyReleased(scancode) }]
            }
            _ => Vec::new(),
        },
        // Unicode keyboard events (MS-RDPBCGR 2.2.8.1.2.2.2), a press and a
        // release for each character.
        Command::Text { text } => text.chars().flat_map(|c| [Operation::UnicodeKeyPressed(c), Operation::UnicodeKeyReleased(c)]).collect(),
        _ => Vec::new(),
    };
    database.apply(ops).into_iter().collect()
}

/// A duplicate of the connection's socket, for the kernel's round trip
/// (TLS owns the socket itself).
#[cfg(unix)]
pub type RttFd = Option<std::os::fd::OwnedFd>;
#[cfg(not(unix))]
pub type RttFd = Option<()>;

#[cfg(unix)]
fn round_trip_ms(fd: &RttFd) -> Option<f32> {
    crate::connector::rustdesk::tcp_round_trip_us(fd.as_ref()?).map(|us| us as f32 / 1000.0)
}

#[cfg(not(unix))]
fn round_trip_ms(_fd: &RttFd) -> Option<f32> {
    None
}

type Reader = TokioFramed<tokio::io::ReadHalf<tokio_rustls::client::TlsStream<TcpStream>>>;
type Writer = TokioFramed<tokio::io::WriteHalf<tokio_rustls::client::TlsStream<TcpStream>>>;

struct Loop {
    stage: ActiveStage,
    image: DecodedImage,
    writer: Writer,
    video: Arc<RawVideo>,
    events: mpsc::Sender<Event>,
    database: Database,
    input_flags: InputFlags,
    refresh_rect_support: bool,
    suppress_output_support: bool,
    tasks: Tasks,
    local_text: Option<String>,
    remote_text_asked: bool,
    pointer_hidden: bool,
    shutdown_sent: bool,
}

impl Loop {
    async fn write(&mut self, frame: &[u8]) -> anyhow::Result<()> {
        if !frame.is_empty() {
            self.writer.write_all(frame).await.map_err(|e| anyhow::anyhow!("The RDP connection failed ({e})"))?;
        }
        Ok(())
    }

    fn size(&self) -> (u16, u16) {
        (self.image.width(), self.image.height())
    }

    /// Sends what a cliprdr operation made.
    async fn send_svc(&mut self, messages: ironrdp_cliprdr::CliprdrSvcMessages<ironrdp_cliprdr::Client>) -> anyhow::Result<()> {
        let frame = self.stage.process_svc_processor_messages(messages).map_err(|e| anyhow::anyhow!("clipboard: {e}"))?;
        self.write(&frame).await
    }

    /// Does what the clipboard channel queued.
    async fn clipboard_tasks(&mut self) -> anyhow::Result<()> {
        loop {
            let task = self.tasks.lock().unwrap().pop_front();
            let Some(task) = task else { return Ok(()) };
            let Some(cliprdr) = self.stage.get_svc_processor_mut::<ironrdp_cliprdr::CliprdrClient>() else { return Ok(()) };
            let messages = match task {
                ClipboardTask::Ready | ClipboardTask::SendFormats => {
                    // Our formats: text when the viewer gave some, else none
                    // (MS-RDPECLIP 2.2.3.1: an empty list says so).
                    let formats = if self.local_text.is_some() { vec![super::clipboard::unicode_text_format()] } else { Vec::new() };
                    cliprdr.initiate_copy(&formats)
                }
                ClipboardTask::FetchRemoteText => {
                    self.remote_text_asked = true;
                    cliprdr.initiate_paste(ironrdp_cliprdr::pdu::ClipboardFormatId::CF_UNICODETEXT)
                }
                ClipboardTask::SendLocalText => {
                    let text = self.local_text.clone().unwrap_or_default();
                    cliprdr.submit_format_data(ironrdp_cliprdr::pdu::FormatDataResponse::new_unicode_string(&text))
                }
                ClipboardTask::RemoteText(text) => {
                    self.remote_text_asked = false;
                    if self.events.send(Event::Clipboard { text }).await.is_err() {
                        anyhow::bail!("The viewer left");
                    }
                    continue;
                }
                ClipboardTask::RemoteTextFailed => {
                    self.remote_text_asked = false;
                    continue;
                }
            };
            let messages = messages.map_err(|e| anyhow::anyhow!("clipboard: {e}"))?;
            self.send_svc(messages).await?;
        }
    }

    /// Handles what the active stage gave. `Some(reason)`: the end.
    async fn outputs(&mut self, reader: &mut Reader, activation: &ironrdp_connector::connection_activation::ConnectionActivationFactory, outputs: Vec<ActiveStageOutput>) -> anyhow::Result<Option<String>> {
        for output in outputs {
            match output {
                ActiveStageOutput::ResponseFrame(frame) => self.write(&frame).await?,
                ActiveStageOutput::GraphicsUpdate(rect) => copy_region(&self.image, &self.video, &rect),
                ActiveStageOutput::PointerDefault => {
                    let _ = self.events.send(default_arrow()).await;
                    self.show_pointer().await;
                }
                ActiveStageOutput::PointerHidden => {
                    self.pointer_hidden = true;
                    let at = self.database.mouse_position();
                    let _ = self.events.send(Event::CursorPosition { x: at.x as i32, y: at.y as i32, visible: false }).await;
                }
                ActiveStageOutput::PointerPosition { x, y } => {
                    let _ = self.events.send(Event::CursorPosition { x: x as i32, y: y as i32, visible: !self.pointer_hidden }).await;
                }
                ActiveStageOutput::PointerBitmap(pointer) => {
                    let _ = self.events.send(pointer_shape(&pointer)).await;
                    self.show_pointer().await;
                }
                ActiveStageOutput::Terminate(reason) => return Ok(Some(reason.description())),
                ActiveStageOutput::DeactivateAll => {
                    // Deactivation-Reactivation Sequence (MS-RDPBCGR 1.3.1.3),
                    // as ironrdp-client `active_session` drives it.
                    debug!("RDP: deactivation-reactivation");
                    let mut sequence = activation.create();
                    let mut buf = WriteBuf::new();
                    loop {
                        let written = single_sequence_step_read(reader, &mut sequence, &mut buf).await.map_err(|e| anyhow::anyhow!("The RDP reactivation failed ({e})"))?;
                        if written.size().is_some() {
                            let filled = buf.filled().to_vec();
                            self.write(&filled).await?;
                        }
                        if let ConnectionActivationState::Redirected { .. } = sequence.connection_activation_state() {
                            return Ok(Some("The computer redirects the connection to another server, which Lunaris does not follow".into()));
                        }
                        if let ConnectionActivationState::Finalized {
                            desktop_size,
                            share_id,
                            input_flags,
                            enable_server_pointer,
                            pointer_software_rendering,
                            static_channel_chunk_size,
                            refresh_rect_support,
                            suppress_output_support,
                            window_support_level,
                            ..
                        } = sequence.connection_activation_state()
                        {
                            info!("RDP: the desktop is now {}x{}", desktop_size.width, desktop_size.height);
                            self.image = DecodedImage::new(PixelFormat::BgrX32, desktop_size.width, desktop_size.height);
                            if !self.stage.reactivate(sequence.io_channel_id(), sequence.user_channel_id(), share_id, enable_server_pointer, pointer_software_rendering, static_channel_chunk_size) {
                                anyhow::bail!("The RDP reactivation failed (invalid static channel chunk size)");
                            }
                            self.stage.set_window_support_level(window_support_level);
                            self.input_flags = input_flags;
                            self.refresh_rect_support = refresh_rect_support;
                            self.suppress_output_support = suppress_output_support;
                            self.video.resize(desktop_size.width as u32, desktop_size.height as u32);
                            break;
                        }
                    }
                }
                // Not used by this client: monitor layouts (one display),
                // RemoteApp, multitransport (TCP only), auto-reconnect,
                // session notifications, redirection after logon.
                ActiveStageOutput::ServerRedirection(_) => {
                    return Ok(Some("The computer redirects the connection to another server, which Lunaris does not follow".into()));
                }
                ActiveStageOutput::LogonErrors(_) => debug!("RDP: logon notification"),
                _ => {}
            }
        }
        Ok(None)
    }

    async fn show_pointer(&mut self) {
        if self.pointer_hidden {
            self.pointer_hidden = false;
            let at = self.database.mouse_position();
            let _ = self.events.send(Event::CursorPosition { x: at.x as i32, y: at.y as i32, visible: true }).await;
        }
    }
}

async fn run(
    result: ConnectionResult,
    framed: TokioFramed<tokio_rustls::client::TlsStream<TcpStream>>,
    video: RawVideo,
    events: mpsc::Sender<Event>,
    mut commands: mpsc::Receiver<Command>,
    tasks: Tasks,
    rtt_fd: RttFd,
) {
    let (mut reader, writer) = split_tokio_framed(framed);
    let size = result.desktop_size;
    let activation = result.activation_factory;
    let stage = ActiveStageBuilder {
        static_channels: result.static_channels,
        user_channel_id: result.user_channel_id,
        io_channel_id: result.io_channel_id,
        message_channel_id: result.message_channel_id,
        share_id: result.share_id,
        compression_type: result.compression_type,
        enable_server_pointer: result.enable_server_pointer,
        pointer_software_rendering: result.pointer_software_rendering,
    }
    .build();
    let mut state = Loop {
        stage,
        image: DecodedImage::new(PixelFormat::BgrX32, size.width, size.height),
        writer,
        video: Arc::new(video),
        events: events.clone(),
        database: Database::new(),
        input_flags: result.input_flags,
        refresh_rect_support: result.refresh_rect_support,
        suppress_output_support: result.suppress_output_support,
        tasks,
        local_text: None,
        remote_text_asked: false,
        pointer_hidden: false,
        shutdown_sent: false,
    };
    let mut latency_tick = tokio::time::interval(Duration::from_secs(1));
    // ironrdp-client drives the clipboard's lock time-outs this often.
    let mut clipboard_tick = tokio::time::interval(Duration::from_secs(5));
    let mut latency_told: Option<u32> = None;
    let mut shutdown_deadline: Option<tokio::time::Instant> = None;
    let mut input_events_sent = 0u64;

    let result: anyhow::Result<String> = async {
        loop {
            let deadline = shutdown_deadline;
            tokio::select! {
                frame = reader.read_pdu() => {
                    let (action, payload) = match frame {
                        Ok(frame) => frame,
                        Err(_) if state.shutdown_sent => return Ok("Closed".to_string()),
                        Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => return Ok("The computer closed the RDP connection".to_string()),
                        Err(e) => anyhow::bail!("The RDP connection failed ({e})"),
                    };
                    let mut outputs = state.stage.process(&mut state.image, action, &payload, reader.last_read_at()).map_err(|e| anyhow::anyhow!("The RDP session failed ({e})"))?;
                    // ironrdp-client: a malformed bitmap was dropped; ask for
                    // the whole picture again where the server allows it.
                    if state.stage.take_bitmap_recovery_request() {
                        let (w, h) = state.size();
                        let frames = state.stage.request_full_redraw(w, h, state.refresh_rect_support, state.suppress_output_support).map_err(|e| anyhow::anyhow!("{e}"))?;
                        outputs.extend(frames.into_iter().map(ActiveStageOutput::ResponseFrame));
                    }
                    if let Some(reason) = state.outputs(&mut reader, &activation, outputs).await? {
                        return Ok(if state.shutdown_sent { "Closed".to_string() } else { reason });
                    }
                    state.clipboard_tasks().await?;
                }
                command = commands.recv(), if !state.shutdown_sent => {
                    let Some(command) = command else {
                        // The viewer left without a word: the same as Close.
                        return Ok("The viewer left".to_string());
                    };
                    match command {
                        Command::Close => {
                            // Nothing stays held, then the client-initiated
                            // shutdown (MS-RDPBCGR 1.3.1.4.1; ActiveStage
                            // `graceful_shutdown`), waiting a moment for the
                            // server to end the connection.
                            let release = state.database.release_all();
                            let frames = state.stage.process_fastpath_input(&mut state.image, &release).map_err(|e| anyhow::anyhow!("{e}"))?;
                            for out in frames.into_iter().chain(state.stage.graceful_shutdown().map_err(|e| anyhow::anyhow!("{e}"))?) {
                                if let ActiveStageOutput::ResponseFrame(frame) = out {
                                    state.write(&frame).await?;
                                }
                            }
                            state.shutdown_sent = true;
                            shutdown_deadline = Some(tokio::time::Instant::now() + SHUTDOWN_WAIT);
                        }
                        Command::RequestKeyFrame => state.video.request_key_frame(),
                        Command::Quality { fps, bitrate_kbps } => state.video.set_quality(fps, bitrate_kbps),
                        Command::SwitchDisplay { .. } => {}
                        Command::Clipboard { text } => {
                            state.local_text = Some(text);
                            state.tasks.lock().unwrap().push_back(ClipboardTask::SendFormats);
                            state.clipboard_tasks().await?;
                        }
                        other => {
                            let horizontal = state.input_flags.contains(InputFlags::TS_MOUSE_HWHEEL);
                            let size = state.size();
                            let events = input_events(&other, &mut state.database, size, horizontal);
                            if !events.is_empty() {
                                input_events_sent += events.len() as u64;
                                let outputs = state.stage.process_fastpath_input(&mut state.image, &events).map_err(|e| anyhow::anyhow!("{e}"))?;
                                for out in outputs {
                                    if let ActiveStageOutput::ResponseFrame(frame) = out {
                                        state.write(&frame).await?;
                                    }
                                }
                            }
                        }
                    }
                }
                _ = async { tokio::time::sleep_until(deadline.unwrap()).await }, if deadline.is_some() => {
                    return Ok("Closed".to_string());
                }
                _ = clipboard_tick.tick() => {
                    if let Some(cliprdr) = state.stage.get_svc_processor_mut::<ironrdp_cliprdr::CliprdrClient>() {
                        if let Ok(messages) = cliprdr.drive_timeouts() {
                            state.send_svc(messages).await?;
                        }
                    }
                }
                _ = latency_tick.tick() => {
                    if let Some(ms) = round_trip_ms(&rtt_fd) {
                        let tenths = (ms * 10.0).round() as u32;
                        if latency_told != Some(tenths) {
                            latency_told = Some(tenths);
                            let _ = state.events.try_send(Event::Latency { ms });
                        }
                    }
                }
            }
        }
    }
    .await;
    let reason = match result {
        Ok(reason) => reason,
        Err(e) => e.to_string(),
    };
    debug!("RDP: {} input events sent", input_events_sent);
    drop(reader);
    drop(state.writer);
    let video = state.video;
    let _ = tokio::task::spawn_blocking(move || drop(video)).await;
    if reason == "Closed" {
        info!("RDP: session closed");
    } else {
        warn!("RDP: session ended: {reason}");
    }
    let _ = events.send(Event::Closed { reason }).await;
}
