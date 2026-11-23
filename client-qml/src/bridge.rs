use bytes::Bytes;
use cxx_qt::CxxQtType;
use std::pin::Pin;
use std::sync::{Arc, Mutex};

#[derive(Debug, Clone)]
pub enum PendingDashboardEvent {
    /// A link to start (or an empty url: show the window).
    DeepLinkReceived {
        url: String,
    },
    /// A link could not be used (its ticket was refused or the server not
    /// reached): `message` is for the user (English, translated in QML).
    LinkFailed {
        message: String,
    },
}

pub static PENDING_EVENTS: std::sync::Mutex<Vec<PendingDashboardEvent>> =
    std::sync::Mutex::new(Vec::new());

#[derive(Debug, Clone)]
pub struct AppArgs {
    pub host_id: String,
    pub server_url: String,
    pub token: String,
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    pub bitrate: u32,
    pub codec: String,
    pub app_id: Option<u32>,
    pub mouse_queue_limit: u32,
    pub host_name: String,
    pub disable_cuda: bool,
    pub render_backend: String,
    pub input_protocol: String,
    pub encoder: Option<String>,
    pub display_id: Option<String>,
    /// The source of the host to show (a computer it reaches through a
    /// connector); None = the host's own screen.
    pub source_id: Option<String>,
    /// Started from a link the user confirmed without trusting its server
    /// (or account): clipboard sync and file transfer stay off. Part of the
    /// session's own settings so that a session never runs with another
    /// one's flag.
    pub untrusted: bool,
}

pub static APP_ARGS: std::sync::OnceLock<AppArgs> = std::sync::OnceLock::new();
pub static ACTIVE_CONFIG: std::sync::Mutex<Option<AppArgs>> = std::sync::Mutex::new(None);

pub const RENDER_BACKEND_AUTO_GPU: &str = "auto_gpu";
pub const RENDER_BACKEND_NATIVE_GPU: &str = "native_gpu";
pub const RENDER_BACKEND_SOFTWARE: &str = "software";

pub fn normalize_render_backend(value: &str, disable_cuda_fallback: bool) -> String {
    let normalized = value
        .trim()
        .to_ascii_lowercase()
        .replace('-', "_")
        .replace(' ', "_");
    match normalized.as_str() {
        "auto" | "auto_gpu" | "gpu" | "hardware" | "hw" => RENDER_BACKEND_AUTO_GPU.to_string(),
        "native" | "native_gpu" | "native_render" | "gpu_native" => {
            RENDER_BACKEND_NATIVE_GPU.to_string()
        }
        "software" | "cpu" | "ffmpeg" | "qvideosink" | "cpu_present" => {
            RENDER_BACKEND_SOFTWARE.to_string()
        }
        "" => render_backend_from_disable_cuda(disable_cuda_fallback),
        _ => render_backend_from_disable_cuda(disable_cuda_fallback),
    }
}

pub fn render_backend_from_disable_cuda(disable_cuda: bool) -> String {
    if disable_cuda {
        RENDER_BACKEND_SOFTWARE.to_string()
    } else {
        RENDER_BACKEND_AUTO_GPU.to_string()
    }
}

pub fn render_backend_disables_cuda(render_backend: &str) -> bool {
    normalize_render_backend(render_backend, false) == RENDER_BACKEND_SOFTWARE
}

/// See `qt_opengl_scenegraph_available` in lib.rs: Qt 6 has OpenGL built into
/// Qt Quick; only the "software" adaptation rules it out.
fn qt_opengl_scenegraph_available_runtime() -> bool {
    !std::env::var("QT_QUICK_BACKEND")
        .map(|v| v.eq_ignore_ascii_case("software"))
        .unwrap_or(false)
}

fn qt_opengl_scenegraph_active_runtime() -> bool {
    std::env::var("QSG_RHI_BACKEND")
        .map(|v| v.eq_ignore_ascii_case("opengl"))
        .unwrap_or(false)
        && qt_opengl_scenegraph_available_runtime()
}

/// Validate that the user-chosen render backend can actually work.
/// When the user explicitly requests `native_gpu` but the required OpenGL
/// scenegraph isn't available, downgrade to `auto_gpu` with a warning.
/// The per-frame present path self-falls-back when native present fails,
/// so we only need to guard against explicitly-unmet preconditions here.
fn configure_native_present_for_stream(args: &mut AppArgs) {
    if args.effective_render_backend() != RENDER_BACKEND_NATIVE_GPU {
        return;
    }

    if cfg!(target_os = "linux") {
        if !qt_opengl_scenegraph_available_runtime() || !qt_opengl_scenegraph_active_runtime() {
            eprintln!(
                "Native GPU presentation requested, but Qt OpenGL scenegraph is \
                 unavailable or inactive. Falling back to Auto GPU decode + \
                 QVideoSink present."
            );
            args.render_backend = RENDER_BACKEND_AUTO_GPU.to_string();
            args.disable_cuda = false;
        }
    }
}

impl AppArgs {
    pub fn effective_render_backend(&self) -> String {
        normalize_render_backend(&self.render_backend, self.disable_cuda)
    }

    pub fn effective_disable_cuda(&self) -> bool {
        self.effective_render_backend() == RENDER_BACKEND_SOFTWARE
    }

    pub fn gpu_mode_enabled(&self) -> bool {
        !self.effective_disable_cuda()
    }
}

#[derive(Debug, Clone)]
pub struct StreamStats {
    pub ping: f64,
    pub decode: f64,
    pub fps: f64,
    pub bitrate: f64,
    pub requested_codec: String,
    pub negotiated_codec: String,
    pub decode_backend: String,
    pub present_backend: String,
    pub gpu_decode_enabled: bool,
    pub fallback_reason: String,
    pub packet_loss_events: u64,
    pub packets_lost: u64,
    pub queue_lag_events: u64,
    pub max_queue_lag: usize,
    pub connection_type: String,
}

pub static STREAM_STATS: std::sync::Mutex<Option<StreamStats>> = std::sync::Mutex::new(None);

fn args_codec_for_stats() -> String {
    ACTIVE_CONFIG
        .lock()
        .unwrap()
        .as_ref()
        .map(|c| c.codec.to_uppercase())
        .or_else(|| APP_ARGS.get().map(|c| c.codec.to_uppercase()))
        .unwrap_or_else(|| "Unknown".to_string())
}

fn saturating_i32(value: u64) -> i32 {
    value.min(i32::MAX as u64) as i32
}

#[derive(Debug, Clone)]
pub struct PendingHostCursor {
    pub x: i32,
    pub y: i32,
    pub visible: bool,
    pub kind: String,
    pub in_window_move_size: bool,
    /// The host's `pointer_maybe_in_picture` (absent = false): its pointer
    /// may be drawn into the picture; `pointer_in_picture` then looks.
    pub pointer_maybe_in_picture: bool,
    /// The host's `pointer_in_picture` is "always" (absent = false): its
    /// pointer is part of its picture for the whole session, and the viewer
    /// shows no pointer over the picture.
    pub pointer_always_in_picture: bool,
    pub image_data_url: String,
    pub image_width: i32,
    pub image_height: i32,
    pub image_hotspot_x: i32,
    pub image_hotspot_y: i32,
}

pub static PENDING_HOST_CURSOR: std::sync::Mutex<Option<PendingHostCursor>> =
    std::sync::Mutex::new(None);

pub static PENDING_HOST_OS: std::sync::Mutex<Option<String>> = std::sync::Mutex::new(None);

#[derive(Debug, Clone)]
pub struct PendingHostInfo {
    pub gpu_info: String,
    pub host_os: String,
}

pub static PENDING_HOST_INFO: std::sync::Mutex<Option<PendingHostInfo>> =
    std::sync::Mutex::new(None);

#[derive(Debug, Clone)]
pub struct PendingEncoderStatus {
    pub encoder: String,
    pub hw_type: String,
    pub gpu_info: String,
    pub requested_encoder: String,
    pub host_os: String,
    pub display_id: String,
    pub display_name: String,
}

pub static PENDING_ENCODER_STATUS: std::sync::Mutex<Option<PendingEncoderStatus>> =
    std::sync::Mutex::new(None);

#[derive(Debug, Clone, Default)]
pub struct PendingCapabilities {
    pub displays_json: String,
    pub encoders_json: String,
    /// The host's sources: `[{"id", "name", "connector"}]`, "[]" when it has none.
    pub sources_json: String,
    pub gpu_info: String,
    pub host_os: String,
}

pub static PENDING_CAPABILITIES: std::sync::Mutex<Option<PendingCapabilities>> =
    std::sync::Mutex::new(None);

#[derive(Clone)]
pub struct VideoSinkWrapper {
    pub sink: Arc<Mutex<Option<usize>>>,
}

unsafe impl Send for VideoSinkWrapper {}
unsafe impl Sync for VideoSinkWrapper {}

pub struct StreamBridgeRust {
    sink_wrapper: VideoSinkWrapper,
    input_senders: Arc<Mutex<Option<super::input::InputSenders>>>,
    tokio_runtime: Option<tokio::runtime::Runtime>,
    active_stream: Arc<Mutex<Option<tokio::task::JoinHandle<()>>>>,
    active_decoder: Arc<Mutex<Option<std::thread::JoinHandle<()>>>>,
}

impl Default for StreamBridgeRust {
    fn default() -> Self {
        Self {
            sink_wrapper: VideoSinkWrapper {
                sink: Arc::new(Mutex::new(None)),
            },
            input_senders: Arc::new(Mutex::new(None)),
            tokio_runtime: None,
            active_stream: Arc::new(Mutex::new(None)),
            active_decoder: Arc::new(Mutex::new(None)),
        }
    }
}

impl StreamBridgeRust {
    pub fn get_or_init_runtime(&mut self) -> &tokio::runtime::Runtime {
        if self.tokio_runtime.is_none() {
            self.tokio_runtime = Some(tokio::runtime::Runtime::new().unwrap());
        }
        self.tokio_runtime.as_ref().unwrap()
    }
}

fn get_config_path() -> std::path::PathBuf {
    let mut path = if let Ok(home) = std::env::var("HOME") {
        std::path::PathBuf::from(home)
    } else if let Ok(user_profile) = std::env::var("USERPROFILE") {
        std::path::PathBuf::from(user_profile)
    } else {
        std::path::PathBuf::from(".")
    };
    path.push(".lunaris");
    let _ = crate::secure_fs::create_private_dir(&path);
    path.push("client_config.json");
    path
}

/// A file in the app's private settings folder (`~/.lunaris`), next to
/// client_config.json.
pub(crate) fn app_config_file(name: &str) -> std::path::PathBuf {
    get_config_path().with_file_name(name)
}

/// The server's WebSocket address for `server_url`: only the scheme changes
/// (http -> ws, https -> wss; ws/wss stay), never a host or path that
/// happens to contain "http". No trailing slash.
pub(crate) fn signaling_ws_base(server_url: &str) -> String {
    let trimmed = server_url.trim();
    let Ok(mut u) = url::Url::parse(trimmed) else {
        return trimmed.trim_end_matches('/').to_string();
    };
    let scheme = match u.scheme() {
        "http" => Some("ws"),
        "https" => Some("wss"),
        _ => None,
    };
    if let Some(scheme) = scheme {
        // http(s) and ws(s) are all "special" schemes: the change is allowed.
        let _ = u.set_scheme(scheme);
    }
    u.as_str().trim_end_matches('/').to_string()
}

/// Paths or file:// URLs, one per line.
fn local_paths(text: &str) -> Vec<std::path::PathBuf> {
    text.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(|l| {
            url::Url::parse(l)
                .ok()
                .filter(|u| u.scheme() == "file")
                .and_then(|u| u.to_file_path().ok())
                .unwrap_or_else(|| std::path::PathBuf::from(l))
        })
        .collect()
}

/// The agent's "host:port" when this is a direct connection (command line
/// `--server ws://<address> --host-id direct`), None for a server.
fn direct_address<'a>(server_url: &'a str, host_id: &str) -> Option<&'a str> {
    if host_id != "direct" {
        return None;
    }
    let address = server_url.strip_prefix("ws://").or_else(|| server_url.strip_prefix("wss://"))?;
    Some(address.trim_end_matches('/'))
}

/// File of a JSON store the UI keeps (`load_store`/`save_store`).
fn store_path(name: &str) -> Option<std::path::PathBuf> {
    matches!(name, "app_settings" | "view_prefs")
        .then(|| get_config_path().with_file_name(format!("{}.json", name)))
}

/// Why the stream couldn't start, set by the stream task and reported to QML
/// by `poll_stats` (`stream_failed`), which leaves the stream view.
pub static STREAM_ERROR: std::sync::Mutex<Option<String>> = std::sync::Mutex::new(None);

/// What the host said about the source being shown (`SourceStatus`: status
/// and reason), set by the stream task, which then ends; `poll_stats` acts
/// on it (starts the session again, or tells QML).
pub static PENDING_SOURCE_STATUS: std::sync::Mutex<Option<(String, String)>> = std::sync::Mutex::new(None);

/// The displays of the source being shown and the current one's id, as the
/// host listed them on the general data channel; `poll_cursor` passes them
/// to QML (`source_displays_updated`).
pub static PENDING_SOURCE_DISPLAYS: std::sync::Mutex<Option<(String, String)>> = std::sync::Mutex::new(None);

/// The delay between the agent and the computer it shows, there and back,
/// in microseconds, as the agent last told it on the general data channel
/// (`session_info::source_latency_ms`); 0 when it told none (it shows its
/// own screen, or its connector does not measure it). Added to the ping
/// shown; put back to 0 when a session starts or stops.
pub static SOURCE_LATENCY_US: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

/// Restarts a source asked for in a row, and when the last one was.
static SOURCE_RESTARTS: std::sync::Mutex<(u32, Option<std::time::Instant>)> = std::sync::Mutex::new((0, None));

/// Restarts closer together than this are "in a row": a source that keeps
/// changing its codec as soon as the session is back.
const SOURCE_RESTART_WINDOW: std::time::Duration = std::time::Duration::from_secs(30);
const SOURCE_RESTARTS_IN_A_ROW: u32 = 3;

/// Counts a restart the source asked for; false when it is one too many in
/// a row (the session is then ended instead of started again).
fn source_restart_allowed(state: &mut (u32, Option<std::time::Instant>), now: std::time::Instant) -> bool {
    let in_a_row = state.1.is_some_and(|last| now.duration_since(last) < SOURCE_RESTART_WINDOW);
    state.0 = if in_a_row { state.0 + 1 } else { 1 };
    state.1 = Some(now);
    state.0 <= SOURCE_RESTARTS_IN_A_ROW
}

/// The codecs to name in a session request (`decode_codecs`): what this
/// client decodes, the one the user chose first ("auto" has no wish).
fn decode_codecs_for_request(decodable: Vec<String>, chosen: &str) -> Vec<String> {
    let chosen = match chosen {
        "hevc" => "h265",
        other => other,
    };
    let mut codecs = decodable;
    if let Some(at) = codecs.iter().position(|c| c == chosen) {
        let first = codecs.remove(at);
        codecs.insert(0, first);
    }
    codecs
}

/// A message for the user from a stream task error.
fn describe_stream_error(e: &anyhow::Error) -> String {
    use tokio_tungstenite::tungstenite::Error as WsError;
    // Direct connections: already full sentences (e.g. an agent too old for TLS).
    if let Some(direct) = e.downcast_ref::<agent::direct::ConnectError>() {
        return direct.to_string();
    }
    if let Some(ws) = e.downcast_ref::<WsError>() {
        match ws {
            WsError::Http(resp) => {
                return match resp.status().as_u16() {
                    401 => "Wrong password or sign-in expired (401).".to_string(),
                    429 => "Too many wrong passwords: try again in a minute.".to_string(),
                    404 => "That address is not a Lunaris server or agent (404).".to_string(),
                    code => format!("The connection was refused (HTTP {}).", code),
                };
            }
            WsError::Io(io) => return format!("Could not reach the computer: {}.", io),
            _ => {}
        }
    }
    format!("Connection failed: {}", e)
}

#[cxx_qt::bridge]

pub mod qobject {
    unsafe extern "C++" {
        include!("QtMultimedia/QVideoSink");
        include!("video_helper.h");
        include!("gpu_video_item.h");
        include!("d3d11_video_item.h");
        include!("vaapi_dmabuf_video_item.h");

        #[cxx_name = "QVideoSink"]
        type QVideoSink;

        #[rust_name = "deliver_yuv_frame"]
        unsafe fn deliver_yuv_frame(
            sink: *mut QVideoSink,
            y_data: *const u8,
            y_stride: i32,
            u_data: *const u8,
            u_stride: i32,
            v_data: *const u8,
            v_stride: i32,
            width: i32,
            height: i32,
        );

        #[rust_name = "window_screen_refresh_rate"]
        fn window_screen_refresh_rate() -> f64;

        #[rust_name = "deliver_bgrx_frame"]
        unsafe fn deliver_bgrx_frame(
            sink: *mut QVideoSink,
            data: *const u8,
            stride: i32,
            width: i32,
            height: i32,
        );

        /// Colours of the frames the renderers draw (from the stream).
        #[rust_name = "set_video_color"]
        fn set_video_color(bt709: bool, full_range: bool);

        /// The colourspace/range requested of the host (encoderCscMode), which
        /// untagged frames are assumed to have.
        #[rust_name = "set_requested_csc_mode"]
        fn set_requested_csc_mode(mode: i32);

        #[rust_name = "warp_cursor_helper"]
        fn warp_cursor_helper(x: i32, y: i32);

        #[rust_name = "set_keyboard_grab_helper"]
        fn set_keyboard_grab_helper(grab: bool);

        #[rust_name = "system_keys_support_helper"]
        fn system_keys_support_helper() -> i32;

        #[rust_name = "register_bridge_instance"]
        unsafe fn register_bridge_instance(bridge: *mut StreamBridge);

        #[rust_name = "set_pointer_locked_helper"]
        fn set_pointer_locked_helper(locked: bool);

        #[rust_name = "deliver_cuda_frame"]
        unsafe fn deliver_cuda_frame(
            cuda_ctx: u64,
            y_ptr: u64,
            y_stride: i32,
            uv_ptr: u64,
            uv_stride: i32,
            width: i32,
            height: i32,
        );

        #[rust_name = "create_widgets_application"]
        fn create_widgets_application();

        #[rust_name = "exec_widgets_application"]
        fn exec_widgets_application() -> i32;

        #[rust_name = "register_gpu_video_item_type"]
        fn register_gpu_video_item_type();

        #[rust_name = "register_native_video_item_types"]
        fn register_native_video_item_types();

        #[rust_name = "set_cuda_stream_active"]
        fn set_cuda_stream_active(active: bool);

        #[rust_name = "cuda_gl_render_failed"]
        fn cuda_gl_render_failed() -> bool;

        /// Windows: the D3D11VA hw device context FFmpeg decodes on (an
        /// AVBufferRef*, one reference for the caller), or 0.
        #[rust_name = "d3d11_setup_decoder_device"]
        fn d3d11_setup_decoder_device() -> u64;

        /// get_format helper: hw frames context for `pixel_format` on the
        /// AVCodecContext* `codec_ctx`.
        #[rust_name = "d3d11_prepare_decoder_context_in_get_format"]
        unsafe fn d3d11_prepare_decoder_context_in_get_format(
            codec_ctx: u64,
            pixel_format: i32,
        ) -> bool;

        /// Hands an AV_PIX_FMT_D3D11 AVFrame* to the renderer; true = the
        /// renderer owns (and frees) it now.
        #[rust_name = "deliver_d3d11_frame"]
        unsafe fn deliver_d3d11_frame(frame_ptr: u64) -> bool;

        #[rust_name = "d3d11_render_failed"]
        fn d3d11_render_failed() -> bool;

        #[rust_name = "set_d3d11_stream_active"]
        fn set_d3d11_stream_active(active: bool);

        #[rust_name = "deliver_dmabuf_frame"]
        unsafe fn deliver_dmabuf_frame(
            fd0: i32,
            fd1: i32,
            fourcc: u32,
            modifier: u64,
            offset0: i32,
            pitch0: i32,
            offset1: i32,
            pitch1: i32,
            width: i32,
            height: i32,
            owner: u64,
        ) -> bool;

        #[rust_name = "dmabuf_render_failed"]
        fn dmabuf_render_failed() -> bool;

        #[rust_name = "set_dmabuf_stream_active"]
        fn set_dmabuf_stream_active(active: bool);
    }

    unsafe extern "C++" {
        include!("cxx-qt-lib/qstring.h");
        type QString = cxx_qt_lib::QString;
    }

    unsafe extern "RustQt" {
        #[qobject]
        #[qml_element]
        type StreamBridge = super::StreamBridgeRust;

        #[qsignal]
        fn stats_updated(
            self: Pin<&mut StreamBridge>,
            ping: f64,
            decode: f64,
            fps: f64,
            bitrate: f64,
            requested_codec: QString,
            negotiated_codec: QString,
            decode_backend: QString,
            present_backend: QString,
            gpu_decode_enabled: bool,
            fallback_reason: QString,
            packet_loss_events: i32,
            packets_lost: i32,
            queue_lag_events: i32,
            max_queue_lag: i32,
            connection_type: QString,
        );

        #[qsignal]
        fn host_cursor_updated(
            self: Pin<&mut StreamBridge>,
            x: i32,
            y: i32,
            visible: bool,
            kind: QString,
            in_window_move_size: bool,
        );

        /// `maybe`: the host says its pointer may be drawn into the picture
        /// (information only). `found`: the client saw it there during this
        /// drag: hide the viewer's own pointer as for `in_window_move_size`.
        #[qsignal]
        fn pointer_in_picture_updated(self: Pin<&mut StreamBridge>, maybe: bool, found: bool);

        /// With every cursor message: whether the host says its pointer is
        /// part of its picture for the whole session (`pointer_in_picture`
        /// is "always"). The viewer then shows no pointer over the picture.
        #[qsignal]
        fn pointer_always_in_picture_updated(self: Pin<&mut StreamBridge>, always: bool);

        #[qsignal]
        fn host_cursor_image_updated(
            self: Pin<&mut StreamBridge>,
            kind: QString,
            source: QString,
            width: i32,
            height: i32,
            hotspot_x: i32,
            hotspot_y: i32,
        );

        #[qsignal]
        fn host_os_updated(self: Pin<&mut StreamBridge>, host_os: QString);

        #[qsignal]
        fn host_info_updated(self: Pin<&mut StreamBridge>, gpu_info: QString, host_os: QString);

        #[qsignal]
        fn encoder_status_updated(
            self: Pin<&mut StreamBridge>,
            encoder: QString,
            hw_type: QString,
            gpu_info: QString,
            requested_encoder: QString,
            host_os: QString,
            display_id: QString,
            display_name: QString,
        );

        #[qsignal]
        fn capabilities_updated(
            self: Pin<&mut StreamBridge>,
            displays_json: QString,
            encoders_json: QString,
            gpu_info: QString,
            host_os: QString,
            sources_json: QString,
        );

        #[qsignal]
        fn local_cursor_delta(self: Pin<&mut StreamBridge>, rx: i32, ry: i32);

        #[qsignal]
        fn settings_loaded(
            self: Pin<&mut StreamBridge>,
            res: QString,
            fps: i32,
            codec: QString,
            bitrate: i32,
            mouse_queue_limit: i32,
            host_name: QString,
            disable_cuda: bool,
            input_protocol: QString,
            render_backend: QString,
        );

        #[qsignal]
        fn deeplink_received(self: Pin<&mut StreamBridge>, url: QString);

        /// A lunaris:// link could not be used (message for the user).
        #[qsignal]
        fn link_failed(self: Pin<&mut StreamBridge>, message: QString);

        /// The stream could not start or ended with an error (message for the user).
        #[qsignal]
        fn stream_failed(self: Pin<&mut StreamBridge>, message: QString);

        /// The source being shown could not be connected ("failed") or closed
        /// ("ended"); `reason` is the host's text for the user, may be "".
        /// The stream is over either way.
        #[qsignal]
        fn source_status(self: Pin<&mut StreamBridge>, status: QString, reason: QString);

        /// The displays of the source being shown (a computer shown through
        /// a connector): `[{id, name, width, height}]` and the id of the one
        /// shown now. The id is what a session asks for as its display.
        #[qsignal]
        fn source_displays_updated(self: Pin<&mut StreamBridge>, displays_json: QString, current: QString);

        /// Clipboard/file sharing news for the user (progress, saved, error).
        #[qsignal]
        fn share_status(self: Pin<&mut StreamBridge>, message: QString);

        #[qinvokable]
        unsafe fn set_video_sink(self: Pin<&mut StreamBridge>, sink: *mut QVideoSink);

        #[qinvokable]
        fn start_stream(self: Pin<&mut StreamBridge>);

        #[qinvokable]
        fn stop_stream(self: Pin<&mut StreamBridge>);

        #[qinvokable]
        fn send_mouse_move(
            self: Pin<&mut StreamBridge>,
            x: i32,
            y: i32,
            width: i32,
            height: i32,
            rx: i32,
            ry: i32,
            pointer_locked: bool,
        );

        #[qinvokable]
        fn send_mouse_click(self: Pin<&mut StreamBridge>, button: i32, is_down: bool);

        /// Turns the remote computer's game mouse on or off (mouse packet
        /// type 5, agent/src/game_mouse); main.qml says it again every 2 s
        /// while it is on.
        #[qinvokable]
        fn send_game_mouse(self: Pin<&mut StreamBridge>, on: bool);

        /// The id of the computer shown ("" before a stream), for what is
        /// kept per computer (ViewPrefs.gameMouseHosts).
        #[qinvokable]
        fn current_host_id(self: &StreamBridge) -> QString;

        #[qinvokable]
        fn send_mouse_wheel(self: Pin<&mut StreamBridge>, angle_x: i32, angle_y: i32);

        #[qinvokable]
        fn send_key_event(self: Pin<&mut StreamBridge>, key: i32, modifiers: i32, is_down: bool);

        /// A key event with the key's position (Qt's `nativeScanCode`) and
        /// whether it is the key repeating.
        #[qinvokable]
        fn send_key_event_at(
            self: Pin<&mut StreamBridge>,
            key: i32,
            modifiers: i32,
            native_scan_code: i32,
            is_down: bool,
            is_auto_repeat: bool,
        );

        /// Lets go of every key held on the remote computer (the stream
        /// view stopped getting the keyboard).
        #[qinvokable]
        fn release_all_keys(self: Pin<&mut StreamBridge>);

        /// How the window system gives the window the system's shortcuts:
        /// "compositor" (Wayland: for as long as it is asked, it may ask
        /// the user), "grab" (X11), "grab-maybe" (XWayland: the desktop may
        /// ignore the grab), "window" (inside the app only: no system
        /// shortcut is taken), "no".
        #[qinvokable]
        fn system_keys_support(self: Pin<&mut StreamBridge>) -> QString;

        #[qinvokable]
        fn warp_cursor(self: Pin<&mut StreamBridge>, x: i32, y: i32);

        #[qinvokable]
        fn set_keyboard_grab(self: Pin<&mut StreamBridge>, grab: bool);

        #[qinvokable]
        fn set_pointer_locked(self: Pin<&mut StreamBridge>, locked: bool);

        #[qinvokable]
        fn update_stream_config(
            self: Pin<&mut StreamBridge>,
            res: QString,
            fps: i32,
            codec: QString,
            bitrate: i32,
            mouse_queue_limit: i32,
            disable_cuda: bool,
            input_protocol: QString,
        );

        #[qinvokable]
        fn update_stream_config_with_backend(
            self: Pin<&mut StreamBridge>,
            res: QString,
            fps: i32,
            codec: QString,
            bitrate: i32,
            mouse_queue_limit: i32,
            disable_cuda: bool,
            render_backend: QString,
            input_protocol: QString,
        );

        #[qinvokable]
        fn request_settings(self: Pin<&mut StreamBridge>);

        #[qinvokable]
        fn poll_stats(self: Pin<&mut StreamBridge>);

        #[qinvokable]
        fn poll_cursor(self: Pin<&mut StreamBridge>);

        #[qinvokable]
        fn has_connection_args(self: Pin<&mut StreamBridge>) -> bool;

        /// A lunaris:// link to a server the user is not signed in to waits
        /// for an answer: JSON {"server":..,"computer":..}, "" if none.
        #[qinvokable]
        fn pending_link_info(self: Pin<&mut StreamBridge>) -> QString;

        /// Answer to that question (`trust_always`: keep the server in the
        /// trusted list). True when there was a link; its stream starts
        /// with `deeplink_received` once its ticket is traded.
        #[qinvokable]
        fn answer_pending_link(self: Pin<&mut StreamBridge>, accept: bool, trust_always: bool) -> bool;

        #[qinvokable]
        fn poll_events(self: Pin<&mut StreamBridge>);

        /// Stream sound on/off and volume (0-100), applied right away.
        #[qinvokable]
        fn set_audio(self: Pin<&mut StreamBridge>, enabled: bool, volume: i32);

        /// Ask the remote computer to record its microphone too (from the
        /// next session request).
        #[qinvokable]
        fn set_record_microphone(self: Pin<&mut StreamBridge>, on: bool);

        /// Clipboard sync and file transfer on/off (this device's settings).
        #[qinvokable]
        fn set_share_options(self: Pin<&mut StreamBridge>, clipboard: bool, files: bool);

        /// This computer's controllers for the Controllers page (JSON, see
        /// gamepad.rs), whether to send them in sessions, and a rumble test.
        #[qinvokable]
        fn gamepads_json(self: Pin<&mut StreamBridge>) -> QString;
        #[qinvokable]
        fn set_gamepad_enabled(self: Pin<&mut StreamBridge>, enabled: bool);
        #[qinvokable]
        fn vibrate_gamepad(self: Pin<&mut StreamBridge>, slot: i32);

        #[qinvokable]
        fn files_open_current(self: Pin<&mut StreamBridge>) -> bool;
        #[qinvokable]
        fn files_close(self: Pin<&mut StreamBridge>);
        /// Changes when files_state changes.
        #[qinvokable]
        fn files_version(self: Pin<&mut StreamBridge>) -> i32;
        #[qinvokable]
        fn files_state(self: Pin<&mut StreamBridge>) -> QString;
        #[qinvokable]
        fn files_list_remote(self: Pin<&mut StreamBridge>, path: QString, hidden: bool);
        /// "mkdir" | "remove" | "rename" (name: the new name).
        #[qinvokable]
        fn files_remote_op(self: Pin<&mut StreamBridge>, op: QString, path: QString, name: QString);
        #[qinvokable]
        fn files_local_places(self: Pin<&mut StreamBridge>) -> QString;
        #[qinvokable]
        fn files_list_local(self: Pin<&mut StreamBridge>, path: QString, hidden: bool) -> QString;
        /// Returns "" or an error message.
        #[qinvokable]
        fn files_local_op(self: Pin<&mut StreamBridge>, op: QString, path: QString, name: QString) -> QString;
        /// Local paths or file:// URLs (one per line) -> the remote folder.
        #[qinvokable]
        fn files_upload(self: Pin<&mut StreamBridge>, paths: QString, remote_dir: QString);
        /// Files dropped on the stream -> the remote Downloads/Lunaris.
        #[qinvokable]
        fn files_upload_dropped(self: Pin<&mut StreamBridge>, paths: QString);
        /// Remote names (one per line) in remote_dir -> the local folder.
        #[qinvokable]
        fn files_download(self: Pin<&mut StreamBridge>, names: QString, remote_dir: QString, local_dir: QString);
        #[qinvokable]
        fn files_cancel(self: Pin<&mut StreamBridge>, id: QString);
        #[qinvokable]
        fn files_clear_finished(self: Pin<&mut StreamBridge>);

        /// A small JSON store of the app ("app_settings", "peers"); "" if none.
        #[qinvokable]
        fn load_store(self: Pin<&mut StreamBridge>, name: QString) -> QString;

        #[qinvokable]
        fn save_store(self: Pin<&mut StreamBridge>, name: QString, json: QString);

        /// Refresh rate in Hz of the screen the window is on, 0 when unknown.
        #[qinvokable]
        fn screen_refresh_rate(self: Pin<&mut StreamBridge>) -> f64;

    }
}

use cxx_qt_lib::QString;
use qobject::{deliver_bgrx_frame, deliver_yuv_frame};

impl qobject::StreamBridge {
    pub unsafe fn set_video_sink(mut self: Pin<&mut Self>, sink: *mut qobject::QVideoSink) {
        {
            let binding = self.as_ref();
            let mut lock = binding.rust().sink_wrapper.sink.lock().unwrap();
            if sink.is_null() {
                *lock = None;
            } else {
                *lock = Some(sink as usize);
            }
        }
        println!("QVideoSink pointer registered successfully: {:?}", sink);

        let bridge_raw = self.as_mut().get_unchecked_mut() as *mut qobject::StreamBridge;
        qobject::register_bridge_instance(bridge_raw);
    }

    pub fn start_stream(mut self: Pin<&mut Self>) {
        println!("Starting WebRTC streaming pipeline...");
        self.as_mut().stop_stream();
        super::pointer_in_picture::reset();
        *STREAM_STATS.lock().unwrap() = Some(StreamStats {
            ping: 0.0,
            decode: 0.0,
            fps: 0.0,
            bitrate: 0.0,
            requested_codec: args_codec_for_stats(),
            negotiated_codec: String::new(),
            decode_backend: "Unknown".to_string(),
            present_backend: "Unknown".to_string(),
            gpu_decode_enabled: false,
            fallback_reason: "Waiting for video track".to_string(),
            packet_loss_events: 0,
            packets_lost: 0,
            queue_lag_events: 0,
            max_queue_lag: 0,
            connection_type: "P2P (Direct)".to_string(),
        });
        // Load active config, if None, initialize from APP_ARGS
        let mut active_config_lock = ACTIVE_CONFIG.lock().unwrap();
        if active_config_lock.is_none() {
            if let Some(args) = APP_ARGS.get() {
                *active_config_lock = Some(args.clone());
            }
        }
        let mut args = match &*active_config_lock {
            Some(a) => a.clone(),
            None => {
                eprintln!("AppArgs static configuration not initialized!");
                return;
            }
        };
        configure_native_present_for_stream(&mut args);
        *active_config_lock = Some(args.clone());
        drop(active_config_lock);

        qobject::set_cuda_stream_active(true);
        qobject::set_d3d11_stream_active(true);
        qobject::set_dmabuf_stream_active(true);

        // Create tokio runtime if not exists
        let rt = match tokio::runtime::Runtime::new() {
            Ok(r) => r,
            Err(e) => {
                eprintln!("Failed to initialize tokio runtime: {:?}", e);
                return;
            }
        };

        let host_id = args.host_id.clone();
        let server_url = args.server_url.clone();
        let token = args.token.clone();
        let width = args.width;
        let height = args.height;
        let fps = args.fps;
        let bitrate = args.bitrate;
        let codec_str = args.codec.clone();
        let app_id = args.app_id;
        let mouse_queue_limit = args.mouse_queue_limit;
        let input_protocol = args.input_protocol.clone();
        let encoder = args.encoder.clone();
        let display_id = args.display_id.clone();
        let source_id = args.source_id.clone();

        let sink_wrapper = self.as_ref().rust().sink_wrapper.clone();
        let input_senders = self.as_ref().rust().input_senders.clone();
        let active_decoder = self.as_ref().rust().active_decoder.clone();

        // Spawn signaling connection and media threads
        let handle = rt.spawn(async move {
            if let Err(e) = run_webrtc_client_task(
                host_id,
                server_url,
                token,
                width,
                height,
                fps,
                bitrate,
                codec_str,
                app_id,
                mouse_queue_limit,
                input_protocol,
                encoder,
                display_id,
                source_id,
                sink_wrapper,
                input_senders,
                active_decoder,
            )
            .await
            {
                eprintln!("Error in WebRTC client task: {:?}", e);
                *STREAM_ERROR.lock().unwrap() = Some(describe_stream_error(&e));
            }
        });

        self.as_mut().rust_mut().tokio_runtime = Some(rt);
        *self.as_mut().rust_mut().active_stream.lock().unwrap() = Some(handle);
    }

    pub fn stop_stream(mut self: Pin<&mut Self>) {
        crate::gamepad::detach();
        println!("Stopping stream and releasing signaling runtime...");
        qobject::set_cuda_stream_active(false);
        qobject::set_d3d11_stream_active(false);
        qobject::set_dmabuf_stream_active(false);
        let handle = self
            .as_mut()
            .rust_mut()
            .active_stream
            .lock()
            .unwrap()
            .take();
        if let Some(h) = handle {
            h.abort();
        }
        self.as_mut().rust_mut().tokio_runtime = None;
        *self.as_mut().rust_mut().input_senders.lock().unwrap() = None;
        // What the last session's source said of its delay is not the next
        // one's (after the runtime went: nothing of the old session sets it).
        SOURCE_LATENCY_US.store(0, std::sync::atomic::Ordering::Relaxed);
        // Nothing is held on a computer that is no longer shown (the host
        // lets the keys go itself when the session ends).
        super::input::forget_pressed_keys();

        let decoder_handle = self
            .as_mut()
            .rust_mut()
            .active_decoder
            .lock()
            .unwrap()
            .take();
        if let Some(h) = decoder_handle {
            println!("Waiting for old decoder thread to exit...");
            if let Err(e) = h.join() {
                eprintln!("Error joining decoder thread: {:?}", e);
            }
            println!("Old decoder thread exited successfully.");
        }

        super::decoder::clear_active_cuda_frame();
    }

    pub fn send_mouse_move(
        mut self: Pin<&mut Self>,
        x: i32,
        y: i32,
        width: i32,
        height: i32,
        rx: i32,
        ry: i32,
        pointer_locked: bool,
    ) {
        if pointer_locked && (rx != 0 || ry != 0) {
            self.as_mut().local_cursor_delta(rx, ry);
        }

        // Use try_lock to avoid blocking the main/QML thread when the
        // WebRTC send path holds the lock.  Dropping a mouse-move packet
        // is preferable to a frame of cursor lag.
        if let Ok(guard) = self.as_ref().rust().input_senders.try_lock() {
            if let Some(ref s) = *guard {
                super::input::handle_mouse_move(x, y, width, height, rx, ry, pointer_locked, s);
                // The absolute positions sent are where the host's pointer
                // is looked for in the picture.
                if !pointer_locked {
                    super::pointer_in_picture::note_position(x, y, width, height);
                }
            }
        }
    }

    pub fn send_game_mouse(self: Pin<&mut Self>, on: bool) {
        if let Ok(guard) = self.as_ref().rust().input_senders.try_lock() {
            if let Some(ref s) = *guard {
                super::input::send_game_mouse(on, s);
            }
        }
    }

    pub fn current_host_id(&self) -> cxx_qt_lib::QString {
        let id = ACTIVE_CONFIG.lock().unwrap().as_ref().map(|c| c.host_id.clone()).unwrap_or_default();
        cxx_qt_lib::QString::from(&id)
    }

    pub fn send_mouse_click(self: Pin<&mut Self>, button: i32, is_down: bool) {
        // Button 1 is the left one (main.qml getButtonCode).
        if button == 1 {
            super::pointer_in_picture::note_left_button(is_down);
        }
        let binding = self.as_ref();
        let senders = binding.rust().input_senders.lock().unwrap();
        if let Some(ref s) = *senders {
            super::input::handle_mouse_click(button, is_down, s);
        }
    }

    pub fn send_mouse_wheel(self: Pin<&mut Self>, angle_x: i32, angle_y: i32) {
        let binding = self.as_ref();
        let senders = binding.rust().input_senders.lock().unwrap();
        if let Some(ref s) = *senders {
            super::input::handle_mouse_wheel(angle_x, angle_y, s);
        }
    }

    pub fn send_key_event(self: Pin<&mut Self>, key: i32, modifiers: i32, is_down: bool) {
        let binding = self.as_ref();
        let senders = binding.rust().input_senders.lock().unwrap();
        if let Some(ref s) = *senders {
            super::input::handle_key_event(key, modifiers, is_down, s);
        }
    }

    pub fn send_key_event_at(
        self: Pin<&mut Self>,
        key: i32,
        modifiers: i32,
        native_scan_code: i32,
        is_down: bool,
        is_auto_repeat: bool,
    ) {
        let binding = self.as_ref();
        let senders = binding.rust().input_senders.lock().unwrap();
        if let Some(ref s) = *senders {
            super::input::handle_key_event_at(
                key,
                modifiers,
                native_scan_code.max(0) as u32,
                is_down,
                is_auto_repeat,
                s,
            );
        }
    }

    pub fn release_all_keys(self: Pin<&mut Self>) {
        let binding = self.as_ref();
        let senders = binding.rust().input_senders.lock().unwrap();
        match *senders {
            Some(ref s) => super::input::release_all_keys(s),
            None => super::input::forget_pressed_keys(),
        }
    }

    pub fn system_keys_support(self: Pin<&mut Self>) -> QString {
        QString::from(match qobject::system_keys_support_helper() {
            1 => "compositor",
            2 => "grab",
            3 => "grab-maybe",
            4 => "window",
            _ => "no",
        })
    }

    pub fn warp_cursor(self: Pin<&mut Self>, x: i32, y: i32) {
        qobject::warp_cursor_helper(x, y);
    }

    pub fn set_keyboard_grab(self: Pin<&mut Self>, grab: bool) {
        qobject::set_keyboard_grab_helper(grab);
    }

    pub fn set_pointer_locked(self: Pin<&mut Self>, locked: bool) {
        qobject::set_pointer_locked_helper(locked);
    }

    pub fn update_stream_config(
        self: Pin<&mut Self>,
        res: QString,
        fps: i32,
        codec: QString,
        bitrate: i32,
        mouse_queue_limit: i32,
        disable_cuda: bool,
        input_protocol: QString,
    ) {
        let render_backend =
            cxx_qt_lib::QString::from(&render_backend_from_disable_cuda(disable_cuda));
        self.update_stream_config_with_backend(
            res,
            fps,
            codec,
            bitrate,
            mouse_queue_limit,
            disable_cuda,
            render_backend,
            input_protocol,
        );
    }

    pub fn update_stream_config_with_backend(
        mut self: Pin<&mut Self>,
        res: QString,
        fps: i32,
        codec: QString,
        bitrate: i32,
        mouse_queue_limit: i32,
        disable_cuda: bool,
        render_backend: QString,
        input_protocol: QString,
    ) {
        let res_str = res.to_string();
        let codec_str = codec.to_string().to_lowercase();
        let input_proto_str = input_protocol.to_string().to_lowercase();
        let render_backend_str =
            normalize_render_backend(&render_backend.to_string(), disable_cuda);
        let effective_disable_cuda = render_backend_disables_cuda(&render_backend_str);
        println!("Updating stream configuration: res={}, fps={}, codec={}, bitrate={}, mouse_queue_limit={}, render_backend={}, disable_cuda={}, input_protocol={}", res_str, fps, codec_str, bitrate, mouse_queue_limit, render_backend_str, effective_disable_cuda, input_proto_str);

        // Parse resolution (e.g. "1920x1080" or "720p")
        let mut width = 1280;
        let mut height = 720;
        if res_str.contains('x') {
            let parts: Vec<&str> = res_str.split('x').collect();
            if parts.len() == 2 {
                if let (Ok(w), Ok(h)) = (parts[0].parse::<u32>(), parts[1].parse::<u32>()) {
                    width = w;
                    height = h;
                }
            }
        } else if res_str.contains("1080") {
            width = 1920;
            height = 1080;
        } else if res_str.contains("720") {
            width = 1280;
            height = 720;
        } else if res_str.contains("540") {
            width = 960;
            height = 540;
        }

        {
            let mut active_config_lock = ACTIVE_CONFIG.lock().unwrap();
            if let Some(ref mut config) = *active_config_lock {
                config.width = width;
                config.height = height;
                config.fps = fps as u32;
                config.codec = codec_str;
                config.bitrate = bitrate as u32;
                config.mouse_queue_limit = mouse_queue_limit as u32;
                config.disable_cuda = effective_disable_cuda;
                config.render_backend = render_backend_str.clone();
                config.input_protocol = input_proto_str;
            } else if let Some(args) = APP_ARGS.get() {
                let mut new_config = args.clone();
                new_config.width = width;
                new_config.height = height;
                new_config.fps = fps as u32;
                new_config.codec = codec_str;
                new_config.bitrate = bitrate as u32;
                new_config.mouse_queue_limit = mouse_queue_limit as u32;
                new_config.disable_cuda = effective_disable_cuda;
                new_config.render_backend = render_backend_str.clone();
                new_config.input_protocol = input_proto_str;
                *active_config_lock = Some(new_config);
            }
        }

        // Restart stream. start_stream() owns cleanup of the previous session.
        self.as_mut().start_stream();
    }

    pub fn poll_stats(mut self: Pin<&mut Self>) {
        let failure = STREAM_ERROR.lock().unwrap().take();
        if let Some(message) = failure {
            self.as_mut().stream_failed(QString::from(&message));
            return;
        }
        let source_status = PENDING_SOURCE_STATUS.lock().unwrap().take();
        if let Some((mut status, mut reason)) = source_status {
            if status == "restart" {
                // The source changed its video codec: the same session again,
                // the way a change of the stream settings restarts it.
                let again = source_restart_allowed(&mut SOURCE_RESTARTS.lock().unwrap(), std::time::Instant::now());
                if again {
                    println!("The source asked for a restart: starting the same session again");
                    self.as_mut().start_stream();
                    return;
                }
                println!("The source asked for a restart too many times in a row: ending the session");
                status = "ended".to_string();
                reason = String::new();
            }
            self.as_mut().source_status(QString::from(&status), QString::from(&reason));
            return;
        }
        if let Some(message) = crate::share::take_status() {
            self.as_mut().share_status(QString::from(&message));
        }
        let stats = { STREAM_STATS.lock().unwrap().clone() };
        if let Some(s) = stats {
            let requested_codec_qstring = cxx_qt_lib::QString::from(&s.requested_codec);
            let negotiated_codec_qstring = cxx_qt_lib::QString::from(&s.negotiated_codec);
            let decode_backend_qstring = cxx_qt_lib::QString::from(&s.decode_backend);
            let present_backend_qstring = cxx_qt_lib::QString::from(&s.present_backend);
            let fallback_reason_qstring = cxx_qt_lib::QString::from(&s.fallback_reason);
            let conn_type_qstring = cxx_qt_lib::QString::from(&s.connection_type);
            self.as_mut().stats_updated(
                s.ping,
                s.decode,
                s.fps,
                s.bitrate,
                requested_codec_qstring,
                negotiated_codec_qstring,
                decode_backend_qstring,
                present_backend_qstring,
                s.gpu_decode_enabled,
                fallback_reason_qstring,
                saturating_i32(s.packet_loss_events),
                saturating_i32(s.packets_lost),
                saturating_i32(s.queue_lag_events),
                s.max_queue_lag.min(i32::MAX as usize) as i32,
                conn_type_qstring,
            );
        }
    }

    pub fn poll_cursor(mut self: Pin<&mut Self>) {
        let cursor = { PENDING_HOST_CURSOR.lock().unwrap().take() };
        if let Some(cursor) = cursor {
            // First: what follows is not drawn when the pointer is in the picture.
            self.as_mut().pointer_always_in_picture_updated(cursor.pointer_always_in_picture);
            let kind_qstring = cxx_qt_lib::QString::from(&cursor.kind);
            self.as_mut().host_cursor_updated(
                cursor.x,
                cursor.y,
                cursor.visible,
                kind_qstring.clone(),
                cursor.in_window_move_size,
            );
            if !cursor.image_data_url.is_empty() {
                self.as_mut().host_cursor_image_updated(
                    kind_qstring,
                    cxx_qt_lib::QString::from(&cursor.image_data_url),
                    cursor.image_width,
                    cursor.image_height,
                    cursor.image_hotspot_x,
                    cursor.image_hotspot_y,
                );
            }
        }

        if let Some((maybe, found)) = super::pointer_in_picture::take_update() {
            self.as_mut().pointer_in_picture_updated(maybe, found);
        }

        let host_os = { PENDING_HOST_OS.lock().unwrap().take() };
        if let Some(host_os) = host_os {
            self.as_mut()
                .host_os_updated(cxx_qt_lib::QString::from(&host_os));
        }

        let host_info = { PENDING_HOST_INFO.lock().unwrap().take() };
        if let Some(info) = host_info {
            self.as_mut().host_info_updated(
                cxx_qt_lib::QString::from(&info.gpu_info),
                cxx_qt_lib::QString::from(&info.host_os),
            );
        }

        let encoder_status = { PENDING_ENCODER_STATUS.lock().unwrap().take() };
        if let Some(status) = encoder_status {
            self.as_mut().encoder_status_updated(
                cxx_qt_lib::QString::from(&status.encoder),
                cxx_qt_lib::QString::from(&status.hw_type),
                cxx_qt_lib::QString::from(&status.gpu_info),
                cxx_qt_lib::QString::from(&status.requested_encoder),
                cxx_qt_lib::QString::from(&status.host_os),
                cxx_qt_lib::QString::from(&status.display_id),
                cxx_qt_lib::QString::from(&status.display_name),
            );
        }

        let capabilities = { PENDING_CAPABILITIES.lock().unwrap().take() };
        if let Some(caps) = capabilities {
            self.as_mut().capabilities_updated(
                cxx_qt_lib::QString::from(&caps.displays_json),
                cxx_qt_lib::QString::from(&caps.encoders_json),
                cxx_qt_lib::QString::from(&caps.gpu_info),
                cxx_qt_lib::QString::from(&caps.host_os),
                cxx_qt_lib::QString::from(&caps.sources_json),
            );
        }

        let source_displays = { PENDING_SOURCE_DISPLAYS.lock().unwrap().take() };
        if let Some((displays_json, current)) = source_displays {
            self.as_mut().source_displays_updated(
                cxx_qt_lib::QString::from(&displays_json),
                cxx_qt_lib::QString::from(&current),
            );
        }
    }

    pub fn request_settings(mut self: Pin<&mut Self>) {
        let mut active_config_lock = ACTIVE_CONFIG.lock().unwrap();
        if active_config_lock.is_none() {
            if let Some(args) = APP_ARGS.get() {
                *active_config_lock = Some(args.clone());
            }
        }
        if let Some(ref config) = *active_config_lock {
            let res = format!("{}x{}", config.width, config.height);
            let res_qstring = cxx_qt_lib::QString::from(&res);
            let codec_qstring = cxx_qt_lib::QString::from(&config.codec);
            let host_name_qstring = cxx_qt_lib::QString::from(&config.host_name);
            let input_proto_qstring = cxx_qt_lib::QString::from(&config.input_protocol);
            let render_backend_qstring =
                cxx_qt_lib::QString::from(&config.effective_render_backend());
            self.as_mut().settings_loaded(
                res_qstring,
                config.fps as i32,
                codec_qstring,
                config.bitrate as i32,
                config.mouse_queue_limit as i32,
                host_name_qstring,
                config.effective_disable_cuda(),
                input_proto_qstring,
                render_backend_qstring,
            );
        }
    }

    pub fn has_connection_args(self: Pin<&mut Self>) -> bool {
        APP_ARGS.get().is_some()
    }

    pub fn pending_link_info(self: Pin<&mut Self>) -> QString {
        match crate::deeplink::pending_info() {
            Some(i) => QString::from(
                &serde_json::json!({
                    "server": i.server_host,
                    "computer": i.computer,
                    "encrypted": i.encrypted,
                    "account": i.account.unwrap_or_default(),
                    "knownAccount": i.known_account.unwrap_or_default(),
                })
                .to_string(),
            ),
            None => QString::from(""),
        }
    }

    /// The answer to a link to an untrusted server. Yes: the session starts
    /// once the link's ticket is traded (`deeplinkReceived`), not here.
    pub fn answer_pending_link(self: Pin<&mut Self>, accept: bool, trust_always: bool) -> bool {
        if accept {
            crate::deeplink::confirm_pending(trust_always)
        } else {
            crate::deeplink::cancel_pending();
            false
        }
    }

    pub fn poll_events(mut self: Pin<&mut Self>) {
        let events = {
            let mut lock = PENDING_EVENTS.lock().unwrap();
            std::mem::take(&mut *lock)
        };
        for event in events {
            match event {
                PendingDashboardEvent::DeepLinkReceived { url } => {
                    let url_qstr = QString::from(&url);
                    self.as_mut().deeplink_received(url_qstr);
                }
                PendingDashboardEvent::LinkFailed { message } => {
                    self.as_mut().link_failed(QString::from(&message));
                }
            }
        }
    }

    pub fn set_audio(self: Pin<&mut Self>, enabled: bool, volume: i32) {
        crate::audio::set_output(enabled, volume.clamp(0, 100) as u32);
    }

    pub fn set_record_microphone(self: Pin<&mut Self>, on: bool) {
        crate::audio::set_record_microphone(on);
    }

    pub fn set_share_options(self: Pin<&mut Self>, clipboard: bool, files: bool) {
        crate::share::set_options(clipboard, files);
    }

    pub fn gamepads_json(self: Pin<&mut Self>) -> QString {
        QString::from(&crate::gamepad::pads_json())
    }

    pub fn set_gamepad_enabled(self: Pin<&mut Self>, enabled: bool) {
        crate::gamepad::set_enabled(enabled);
        crate::gamepad::start();
    }

    pub fn vibrate_gamepad(self: Pin<&mut Self>, slot: i32) {
        crate::gamepad::vibrate(slot);
    }

    pub fn files_open_current(self: Pin<&mut Self>) -> bool {
        let args = ACTIVE_CONFIG.lock().unwrap().clone().or_else(|| APP_ARGS.get().cloned());
        match args {
            // The files of what is being shown: the computer's own, or the
            // source's (a computer the app added for itself is one).
            Some(a) if !a.server_url.is_empty() => {
                crate::files::open(a.server_url, a.token, a.host_id, a.host_name, a.source_id);
                true
            }
            _ => false,
        }
    }

    pub fn files_close(self: Pin<&mut Self>) {
        crate::files::close();
    }

    pub fn files_version(self: Pin<&mut Self>) -> i32 {
        crate::files::version() as i32
    }

    pub fn files_state(self: Pin<&mut Self>) -> QString {
        QString::from(&crate::files::state_json())
    }

    pub fn files_list_remote(self: Pin<&mut Self>, path: QString, hidden: bool) {
        crate::files::list_remote(path.to_string(), hidden);
    }

    pub fn files_remote_op(self: Pin<&mut Self>, op: QString, path: QString, name: QString) {
        crate::files::remote_op(&op.to_string(), path.to_string(), name.to_string());
    }

    pub fn files_local_places(self: Pin<&mut Self>) -> QString {
        QString::from(&crate::files::local_places())
    }

    pub fn files_list_local(self: Pin<&mut Self>, path: QString, hidden: bool) -> QString {
        QString::from(&crate::files::list_local(&path.to_string(), hidden))
    }

    pub fn files_local_op(self: Pin<&mut Self>, op: QString, path: QString, name: QString) -> QString {
        QString::from(&crate::files::local_op(&op.to_string(), &path.to_string(), &name.to_string()))
    }

    pub fn files_upload(self: Pin<&mut Self>, paths: QString, remote_dir: QString) {
        crate::files::upload(local_paths(&paths.to_string()), remote_dir.to_string());
    }

    pub fn files_upload_dropped(self: Pin<&mut Self>, paths: QString) {
        crate::files::upload_to_downloads(local_paths(&paths.to_string()));
    }

    pub fn files_download(self: Pin<&mut Self>, names: QString, remote_dir: QString, local_dir: QString) {
        let names = names.to_string().lines().map(str::to_string).filter(|n| !n.is_empty()).collect();
        crate::files::download(names, remote_dir.to_string(), std::path::PathBuf::from(local_dir.to_string()));
    }

    pub fn files_cancel(self: Pin<&mut Self>, id: QString) {
        crate::files::cancel(&id.to_string());
    }

    pub fn files_clear_finished(self: Pin<&mut Self>) {
        crate::files::clear_finished();
    }

    pub fn load_store(self: Pin<&mut Self>, name: QString) -> QString {
        let data = store_path(&name.to_string())
            .and_then(|path| std::fs::read_to_string(path).ok())
            .unwrap_or_default();
        QString::from(&data)
    }

    pub fn save_store(self: Pin<&mut Self>, name: QString, json: QString) {
        let Some(path) = store_path(&name.to_string()) else { return };
        let json = json.to_string();
        if serde_json::from_str::<serde_json::Value>(&json).is_err() {
            return;
        }
        // "peers" may hold access passwords: owner-only.
        let _ = crate::secure_fs::write_private(&path, json);
    }

    pub fn screen_refresh_rate(self: Pin<&mut Self>) -> f64 {
        qobject::window_screen_refresh_rate()
    }

}

// -----------------------------------------------------------------------------
// WebRTC and network handling logic (Tokio Task)
// -----------------------------------------------------------------------------
use common::{
    ClientMessage, RtcSdpType, RtcSessionDescription, ServerToClientMessage, SignalingMessage,
};
use futures_util::{SinkExt, StreamExt};
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::protocol::Message as WsMessage;
use rtc::interceptor::{
    FlexFec03ReceiveBuilder, NackGeneratorBuilder, NackResponderBuilder, Slot,
};
use rtc::peer_connection::configuration::media_engine::{
    MIME_TYPE_AV1, MIME_TYPE_FLEX_FEC03, MIME_TYPE_H264, MIME_TYPE_HEVC,
};
use rtc::rtp_transceiver::rtp_sender::{
    RTCPFeedback, RTCRtpCodec, RTCRtpCodecParameters, RTCRtpHeaderExtensionCapability, RtpCodecKind,
};
use webrtc::data_channel::{DataChannel, DataChannelEvent};
use webrtc::media_stream::track_remote::{TrackRemote, TrackRemoteEvent};
use webrtc::peer_connection::{
    configure_rtcp_reports, configure_twcc_receiver_only, MediaEngine, PeerConnection,
    PeerConnectionBuilder, PeerConnectionEventHandler, RTCConfigurationBuilder, RTCIceCandidateInit,
    RTCIceConnectionState, RTCIceServer, RTCPeerConnectionIceEvent, RTCPeerConnectionState,
    RTCSessionDescription, Registry, RTCIceCandidateType, RTCStatsReportEntry, StatsSelector,
};
use wtransport::{ClientConfig, Endpoint};

fn normalize_host_cursor_kind(kind: &str) -> &str {
    match kind {
        "arrow" | "ibeam" | "hand" | "cross" | "move" | "resize_ns" | "resize_ew"
        | "resize_nesw" | "resize_nwse" | "unavailable" | "unknown" => kind,
        _ => "arrow",
    }
}

fn encode_cursor_png_data_url(image: &serde_json::Value) -> Option<(String, i32, i32, i32, i32)> {
    use common::base64::Engine;

    let width = image.get("width")?.as_u64()? as u32;
    let height = image.get("height")?.as_u64()? as u32;
    let hotspot_x = image.get("hotspot_x")?.as_i64()? as i32;
    let hotspot_y = image.get("hotspot_y")?.as_i64()? as i32;
    let rgba_b64 = image.get("rgba")?.as_str()?;
    if width == 0 || height == 0 || width > 256 || height > 256 {
        return None;
    }

    let rgba = common::base64::engine::general_purpose::STANDARD
        .decode(rgba_b64)
        .ok()?;
    if rgba.len() != width as usize * height as usize * 4 {
        return None;
    }

    let mut png_bytes = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut png_bytes, width, height);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder.write_header().ok()?;
        writer.write_image_data(&rgba).ok()?;
    }

    let encoded = common::base64::engine::general_purpose::STANDARD.encode(png_bytes);
    Some((
        format!("data:image/png;base64,{}", encoded),
        width as i32,
        height as i32,
        hotspot_x,
        hotspot_y,
    ))
}

/// The cursor image of a `host_cursor` message as pixels (RGBA, straight
/// alpha), for `pointer_in_picture`.
fn host_cursor_image(image: &serde_json::Value) -> Option<super::pointer_in_picture::CursorImage> {
    use common::base64::Engine;

    let width = image.get("width")?.as_u64()?;
    let height = image.get("height")?.as_u64()?;
    if width == 0 || height == 0 || width > 256 || height > 256 {
        return None;
    }
    let rgba = common::base64::engine::general_purpose::STANDARD
        .decode(image.get("rgba")?.as_str()?)
        .ok()?;
    if rgba.len() != width as usize * height as usize * 4 {
        return None;
    }
    Some(super::pointer_in_picture::CursorImage {
        width: width as i32,
        height: height as i32,
        hot_x: image.get("hotspot_x")?.as_i64()? as i32,
        hot_y: image.get("hotspot_y")?.as_i64()? as i32,
        rgba,
    })
}

/// `pointer_in_picture` of a `host_cursor` message: only "always" says that
/// the host's pointer is part of its picture for the whole session. Hosts
/// from before the field do not send it; other hosts send null.
fn pointer_always_in_picture(message: &serde_json::Value) -> bool {
    message.get("pointer_in_picture").and_then(|v| v.as_str()) == Some("always")
}

fn handle_host_cursor_message(data: &[u8]) {
    let Ok(text) = std::str::from_utf8(data) else {
        return;
    };
    let Ok(value) = serde_json::from_str::<serde_json::Value>(text) else {
        return;
    };
    if value.get("type").and_then(|v| v.as_str()) != Some("host_cursor") {
        return;
    }

    let Some(x) = value.get("x").and_then(|v| v.as_f64()) else {
        return;
    };
    let Some(y) = value.get("y").and_then(|v| v.as_f64()) else {
        return;
    };
    let visible = value
        .get("visible")
        .and_then(|v| v.as_bool())
        .unwrap_or(true);
    let kind = value
        .get("kind")
        .and_then(|v| v.as_str())
        .map(normalize_host_cursor_kind)
        .unwrap_or("arrow")
        .to_string();
    let in_window_move_size = value
        .get("in_window_move_size")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let pointer_maybe_in_picture = value
        .get("pointer_maybe_in_picture")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let pointer_always_in_picture = pointer_always_in_picture(&value);
    let (image_data_url, image_width, image_height, image_hotspot_x, image_hotspot_y) = value
        .get("image")
        .and_then(encode_cursor_png_data_url)
        .unwrap_or_else(|| (String::new(), 0, 0, 0, 0));
    // The matcher looks for the cursor image last delivered.
    if let Some(cursor) = value.get("image").and_then(host_cursor_image) {
        super::pointer_in_picture::note_cursor(cursor);
    }
    super::pointer_in_picture::note_maybe(pointer_maybe_in_picture);

    *PENDING_HOST_CURSOR.lock().unwrap() = Some(PendingHostCursor {
        x: x.round() as i32,
        y: y.round() as i32,
        visible,
        kind,
        in_window_move_size,
        pointer_maybe_in_picture,
        pointer_always_in_picture,
        image_data_url,
        image_width,
        image_height,
        image_hotspot_x,
        image_hotspot_y,
    });
}

/// The certificate hash WebTransport input is pinned to, or why WebTransport
/// is not used. Never without a hash: an agent that sends none (older
/// agents) would need an unchecked certificate, so anyone on the path could
/// take the input; such a session keeps its input on the WebRTC data
/// channels.
fn webtransport_pinned_digest(cert_hash: Option<&str>, token: Option<&str>) -> Result<[u8; 32], &'static str> {
    match (cert_hash.and_then(parse_sha256_hex), token.is_some()) {
        (Some(digest), _) => Ok(digest),
        (None, true) => Err("the agent sent a token but no valid certificate hash"),
        (None, false) => Err("the agent sent no certificate hash (an older agent), its certificate cannot be checked"),
    }
}

/// Splits the client's codec choice into the codec and chroma to request:
/// "h264_444" -> ("h264", Some("444")), anything else -> (codec, None).
/// Input over WebTransport (QUIC datagrams): once ICE is connected, connect to
/// the agent at the address ICE found reachable (the selected candidate pair,
/// unless it is a TURN relay), then the signaling server's host, checking the
/// agent's self-signed certificate against the hash from its offer.
async fn connect_webtransport_input(
    pc: Arc<dyn PeerConnection>,
    pc_state: ConnState,
    server_url: String,
    port: u16,
    cert_hash: Option<String>,
    token: Option<String>,
    wt_conn: Arc<Mutex<Option<wtransport::Connection>>>,
    wt_connected: Arc<std::sync::atomic::AtomicBool>,
) {
    use webrtc::peer_connection::{RTCIceCandidateType, RTCStatsReportEntry, StatsSelector};

    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
    while !pc_state.ice_connected() {
        if std::time::Instant::now() > deadline || pc_state.closed() {
            eprintln!("WebTransport: ICE not connected, input stays on WebRTC");
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }

    let mut hosts: Vec<String> = Vec::new();
    // The selected candidate pair (0.17: `get_selected_candidate_pair`) is the
    // nominated pair of the stats report in 0.21.
    let stats = pc.get_stats(std::time::Instant::now(), StatsSelector::None).await;
    let selected = stats.candidate_pairs().find(|pair| pair.nominated);
    if let Some(pair) = selected {
        if let Some(RTCStatsReportEntry::RemoteCandidate(remote)) = stats.get(&pair.remote_candidate_id) {
            if remote.candidate_type != RTCIceCandidateType::Relay {
                if let Some(address) = remote.address.as_ref().filter(|a| !a.is_empty()) {
                    hosts.push(address.clone());
                }
            }
        }
    }
    if let Some(host) = url::Url::parse(&server_url).ok().and_then(|u| u.host_str().map(str::to_string)) {
        if !hosts.contains(&host) {
            hosts.push(host);
        }
    }

    let digest = match webtransport_pinned_digest(cert_hash.as_deref(), token.as_deref()) {
        Ok(d) => d,
        Err(why) => {
            // The input keeps going over the WebRTC data channels.
            eprintln!("WebTransport: {}, input stays on WebRTC", why);
            return;
        }
    };
    let config = ClientConfig::builder()
        .with_bind_default()
        .with_server_certificate_hashes([wtransport::tls::Sha256Digest::new(digest)])
        .build();
    // Newer agents accept only the session's token as the URL path.
    let path = webtransport_path(token.as_deref());
    let endpoint = match Endpoint::client(config) {
        Ok(e) => e,
        Err(e) => {
            eprintln!("WebTransport: cannot create the endpoint: {:?}", e);
            return;
        }
    };
    for host in hosts {
        let host_part = if host.contains(':') { format!("[{}]", host) } else { host.clone() };
        let url = format!("https://{}:{}{}", host_part, port, path);
        println!("WebTransport: connecting to https://{}:{}", host_part, port);
        match tokio::time::timeout(std::time::Duration::from_secs(4), endpoint.connect(&url)).await {
            Ok(Ok(connection)) => {
                println!("WebTransport: connected to {}:{}; input now goes over QUIC datagrams", host_part, port);
                *wt_conn.lock().unwrap() = Some(connection);
                wt_connected.store(true, std::sync::atomic::Ordering::Release);
                return;
            }
            Ok(Err(e)) => eprintln!("WebTransport: {}:{} failed: {:?}", host_part, port, e),
            Err(_) => eprintln!("WebTransport: {}:{} timed out", host_part, port),
        }
    }
    eprintln!("WebTransport: no agent address answered, input stays on WebRTC");
}

/// The URL path of the WebTransport request: `/<token>`, or none (the root,
/// as before) for agents that send no token. The agent's token is hex;
/// anything else is dropped rather than put into the URL.
fn webtransport_path(token: Option<&str>) -> String {
    match token.filter(|t| !t.is_empty() && t.bytes().all(|b| b.is_ascii_hexdigit())) {
        Some(token) => format!("/{token}"),
        None => String::new(),
    }
}

/// A SHA-256 digest from hex (the agent's certificate hash).
fn parse_sha256_hex(hex: &str) -> Option<[u8; 32]> {
    let hex = hex.trim();
    if hex.len() != 64 {
        return None;
    }
    let mut out = [0u8; 32];
    for (i, byte) in out.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16).ok()?;
    }
    Some(out)
}

/// Codec for "auto": H.265 when the host has a hardware HEVC encoder for its
/// GPU, else H.264. (The host lists every encoder its FFmpeg has, usable or
/// not, so the encoder must match the GPU; a software H.265 encode would be
/// slow.) AV1 isn't picked: hardware AV1 decoding here isn't guaranteed.
fn auto_codec_for_host(encoders: &[String], gpu_info: Option<&str>) -> &'static str {
    let gpu = gpu_info.unwrap_or_default().to_ascii_lowercase();
    let has = |name: &str| encoders.iter().any(|e| e.eq_ignore_ascii_case(name));
    let nvidia = gpu.contains("nvidia") || gpu.contains("geforce") || gpu.contains("quadro");
    let intel = gpu.contains("intel");
    let amd = gpu.contains("amd") || gpu.contains("radeon");
    let hw_hevc = (nvidia && has("hevc_nvenc"))
        || ((intel || amd) && (has("hevc_vaapi") || has("hevc_qsv")))
        || (amd && has("hevc_amf"));
    if hw_hevc {
        "h265"
    } else {
        "h264"
    }
}

/// Mirrors `lunaris_media::types::DEFAULT_ENCODER_CSC_MODE` (client-qml does
/// not depend on lunaris-media): Rec. 709, limited range.
pub(crate) const HOST_DEFAULT_ENCODER_CSC_MODE: i32 = 2;

/// Composition of `requested_encoder_csc_mode` from the two override values
/// (None = unset or not an integer).
fn compose_encoder_csc_mode(space: Option<i32>, range: Option<i32>) -> Option<i32> {
    if space.is_none() && range.is_none() {
        return None;
    }
    // An absent one takes the host default's part.
    let space = space.unwrap_or(HOST_DEFAULT_ENCODER_CSC_MODE >> 1);
    let range = range.unwrap_or(HOST_DEFAULT_ENCODER_CSC_MODE & 1);
    Some((space << 1) | range)
}

/// Value of an integer environment variable; unset or not an integer is
/// None, as `qEnvironmentVariableIntValue(name, &ok)` with ok false.
fn env_int(name: &str) -> Option<i32> {
    std::env::var(name).ok()?.trim().parse::<i32>().ok()
}

/// Port of moonlight-qt session.cpp:540-556 (overrides only; the
/// per-renderer request arrives with decoder selection, Phase 8).
/// COLOR_SPACE_OVERRIDE: 0 Rec. 601, 1 Rec. 709, 2 Rec. 2020;
/// COLOR_RANGE_OVERRIDE: 0 limited, 1 full. encoderCscMode =
/// (colorspace << 1) | range, the inverse of Sunshine
/// video_colorspace.cpp:59 and :82.
fn requested_encoder_csc_mode() -> Option<i32> {
    compose_encoder_csc_mode(env_int("COLOR_SPACE_OVERRIDE"), env_int("COLOR_RANGE_OVERRIDE"))
}

fn split_chroma_codec(codec: &str) -> (String, Option<String>) {
    let codec = codec.trim().to_ascii_lowercase();
    match codec.as_str() {
        "h264_444" | "h264-444" | "h264 4:4:4" => ("h264".to_string(), Some("444".to_string())),
        _ => (codec, None),
    }
}

/// True when the negotiated H.264 track is High 4:4:4 Predictive
/// (profile_idc 244 = 0xf4), which VAAPI and NVDEC can't decode.
fn is_h264_444_fmtp(fmtp: &str) -> bool {
    fmtp.to_ascii_lowercase()
        .split(';')
        .filter_map(|kv| kv.trim().strip_prefix("profile-level-id="))
        .any(|plid| plid.starts_with("f4"))
}

fn select_video_codec_for_offer(requested_codec: &str, offer_sdp: &str) -> String {
    let requested = requested_codec.trim().to_ascii_lowercase();
    let sdp = offer_sdp.to_ascii_lowercase();
    let has_h264 = sdp.contains(" h264/90000") || sdp.contains("h264/90000");
    let has_h265 =
        sdp.contains(" h265/90000") || sdp.contains("hevc/90000") || sdp.contains("h265/90000");
    let has_av1 = sdp.contains(" av1/90000") || sdp.contains("av1/90000");

    match requested.as_str() {
        "h265" | "hevc" if has_h265 => "h265".to_string(),
        "av1" if has_av1 => "av1".to_string(),
        "h264" if has_h264 => "h264".to_string(),
        _ => {
            if has_h264 {
                "h264".to_string()
            } else if has_h265 {
                "h265".to_string()
            } else if has_av1 {
                "av1".to_string()
            } else {
                requested
            }
        }
    }
}

/// Connection state the event handler records (webrtc 0.21 has no state
/// getters on the connection).
#[derive(Clone)]
struct ConnState(Arc<std::sync::Mutex<(RTCIceConnectionState, RTCPeerConnectionState)>>);

impl ConnState {
    fn ice_connected(&self) -> bool {
        matches!(
            self.0.lock().unwrap().0,
            RTCIceConnectionState::Connected | RTCIceConnectionState::Completed
        )
    }

    fn closed(&self) -> bool {
        self.0.lock().unwrap().1 == RTCPeerConnectionState::Closed
    }
}

/// Events of the stream connection (webrtc 0.21 delivers them to a handler
/// object, 0.17 to callbacks).
struct ClientEventHandler {
    outbox_tx: tokio::sync::mpsc::UnboundedSender<ClientMessage>,
    host_id: String,
    sink_wrapper: VideoSinkWrapper,
    kb_chan_ref: Arc<Mutex<Option<Arc<dyn DataChannel>>>>,
    ma_chan_ref: Arc<Mutex<Option<Arc<dyn DataChannel>>>>,
    mr_chan_ref: Arc<Mutex<Option<Arc<dyn DataChannel>>>>,
    active_decoder: Arc<Mutex<Option<std::thread::JoinHandle<()>>>>,
    state: ConnState,
}

#[async_trait::async_trait]
impl PeerConnectionEventHandler for ClientEventHandler {
    async fn on_ice_connection_state_change(&self, state: RTCIceConnectionState) {
        self.state.0.lock().unwrap().0 = state;
        println!("Client ICE connection state changed: {:?}", state);
    }

    async fn on_connection_state_change(&self, state: RTCPeerConnectionState) {
        self.state.0.lock().unwrap().1 = state;
        println!("Client WebRTC peer connection state changed: {:?}", state);
    }

    // ICE candidate gathering
    async fn on_ice_candidate(&self, event: RTCPeerConnectionIceEvent) {
        if let Ok(json_cand) = event.candidate.to_json() {
            let msg = ClientMessage::Signaling(SignalingMessage::IceCandidate {
                target_id: self.host_id.clone(),
                candidate: common::RtcIceCandidate {
                    candidate: json_cand.candidate,
                    sdp_mid: json_cand.sdp_mid,
                    sdp_mline_index: json_cand.sdp_mline_index,
                    username_fragment: json_cand.username_fragment,
                },
            });
            let _ = self.outbox_tx.send(msg);
        }
    }

    // Video / audio track handler
    async fn on_track(&self, track: Arc<dyn TrackRemote>) {
        tokio::spawn(handle_remote_track(
            track,
            self.sink_wrapper.clone(),
            self.active_decoder.clone(),
        ));
    }

    // Data channel callbacks
    async fn on_data_channel(&self, d: Arc<dyn DataChannel>) {
        let label = d.label().await.unwrap_or_default();
        println!("Remote host created DataChannel: {}", label);
        let channel_ref = Arc::clone(&d);
        match label.as_str() {
            "keyboard" => {
                *self.kb_chan_ref.lock().unwrap() = Some(channel_ref);
            }
            "mouse_absolute" => {
                *self.ma_chan_ref.lock().unwrap() = Some(channel_ref);
            }
            "mouse_relative" => {
                *self.mr_chan_ref.lock().unwrap() = Some(channel_ref);
            }
            "clipboard" => crate::share::attach_clipboard(channel_ref),
            "gamepad" => crate::gamepad::attach(channel_ref),
            "general" | "cursor" => {
                // webrtc 0.21: the channel's events are polled.
                tokio::spawn(async move {
                    while let Some(event) = d.poll().await {
                        match event {
                            DataChannelEvent::OnMessage(msg) => {
                                // A source lists its displays here too.
                                if let Some(displays) = crate::session_info::source_displays(&msg.data) {
                                    *PENDING_SOURCE_DISPLAYS.lock().unwrap() = Some(displays);
                                } else if let Some(ms) = crate::session_info::source_latency_ms(&msg.data) {
                                    // And how far it is from the computer it shows.
                                    SOURCE_LATENCY_US.store((ms * 1000.0).round() as u32, std::sync::atomic::Ordering::Relaxed);
                                } else {
                                    handle_host_cursor_message(&msg.data);
                                }
                            }
                            DataChannelEvent::OnClose => {
                                *PENDING_HOST_CURSOR.lock().unwrap() = Some(PendingHostCursor {
                                    x: 0,
                                    y: 0,
                                    visible: false,
                                    kind: "arrow".to_string(),
                                    in_window_move_size: false,
                                    pointer_maybe_in_picture: false,
                                    pointer_always_in_picture: false,
                                    image_data_url: String::new(),
                                    image_width: 0,
                                    image_height: 0,
                                    image_hotspot_x: 0,
                                    image_hotspot_y: 0,
                                });
                            }
                            _ => {}
                        }
                    }
                });
            }
            _ => {}
        }
    }
}

/// The 0.17 `on_track` callback's body: starts the receive pipeline of a
/// remote track (video: reader task -> decode thread; audio: Opus). Runs in
/// its own task because webrtc 0.21 waits for the event handler to return.
async fn handle_remote_track(
    track: Arc<dyn TrackRemote>,
    sink_inner: VideoSinkWrapper,
    active_decoder_inner: Arc<Mutex<Option<std::thread::JoinHandle<()>>>>,
) {
    let track_clone = Arc::clone(&track);
    let Some(media_ssrc) = track.ssrcs().await.first().copied() else {
        eprintln!("client-qml on_track: the track has no SSRC");
        return;
    };
    let Some(codec) = track.codec(media_ssrc).await else {
        eprintln!("client-qml on_track: no codec for SSRC {}", media_ssrc);
        return;
    };
        let mime = codec.mime_type.to_lowercase();
        println!("client-qml on_track called: kind={}, mime_type={}", track_clone.kind().await, mime);
        let is_video = mime == "video/h264" || mime == "video/h265" || mime == "video/hevc" || mime == "video/av1";
        
        if is_video {
            println!("Starting video receiver for: {}", mime);
            let codec_type = match mime.as_str() {
                "video/h264" => super::decoder::CodecType::H264,
                "video/h265" | "video/hevc" => super::decoder::CodecType::H265,
                "video/av1" => super::decoder::CodecType::AV1,
                _ => unreachable!(),
            };
            
            let (disable_cuda, requested_codec) = if let Some(ref config) = *ACTIVE_CONFIG.lock().unwrap() {
                (config.effective_disable_cuda(), config.codec.to_uppercase())
            } else {
                (false, "Unknown".to_string())
            };
            // H.264 4:4:4 has no GPU decoder (VAAPI/NVDEC): decode it in
            // software straight away instead of failing over per frame.
            let h264_444 = matches!(codec_type, super::decoder::CodecType::H264)
                && is_h264_444_fmtp(&codec.sdp_fmtp_line);
            if h264_444 {
                println!("H.264 4:4:4 stream: decoding in software (multi-threaded)");
            }
            let decoder = match super::decoder::HardwareDecoder::new(codec_type, disable_cuda || h264_444) {
                Ok(d) => d,
                Err(e) => {
                    eprintln!("Failed to initialize hardware decoder: {:?}", e);
                    return;
                }
            };
            
            let annex_b_buf = Vec::<u8>::new();
            let av1_assembler = crate::av1_rtp::Av1Assembler::default();
            
            let frame_count = 0;
            let byte_count = 0;
            let last_stats_time = std::time::Instant::now();
            let total_decode_time_ms = 0.0;
            let decode_count = 0;
            
            let has_decoded = Arc::new(std::sync::atomic::AtomicBool::new(false));
            let has_decoded_clone = Arc::clone(&has_decoded);
                    let pli_track = Arc::clone(&track_clone);
            
            tokio::spawn(async move {
                use rtc::rtcp::payload_feedbacks::picture_loss_indication::PictureLossIndication;
                println!("PLI requester task started for video track SSRC: {}", media_ssrc);
                
                while !has_decoded_clone.load(std::sync::atomic::Ordering::SeqCst) {
                    let pli = PictureLossIndication {
                        sender_ssrc: 0,
                        media_ssrc,
                    };
                    if let Err(e) = pli_track.write_rtcp(vec![Box::new(pli)]).await {
                        eprintln!("Failed to send PLI request: {:?}", e);
                    }
                    tokio::time::sleep(std::time::Duration::from_millis(1000)).await;
                }
                println!("First frame decoded, stopping periodic PLI requests.");
            });
            
            let pending_packets = Arc::new(std::sync::atomic::AtomicUsize::new(0));
            let pending_packets_reader = Arc::clone(&pending_packets);
            let pending_packets_decoder = Arc::clone(&pending_packets);

            // Room for a whole keyframe burst: at 20 Mbps a 1080p keyframe
            // is a few hundred packets arriving at once. With 256 the
            // queue overflowed on every keyframe, which dropped packets,
            // requested another (large) keyframe and froze the stream.
            const RTP_QUEUE_LIMIT: usize = 4096;
            // Flush only when the decoder is really behind (~1 s of video
            // at 20 Mbps), not on a keyframe burst.
            const RTP_QUEUE_LAG_FLUSH: usize = 2048;
            // A sequence gap is treated as loss once a NACK retransmission
            // had time to arrive, or when the reorder buffer gets this big.
            const RTP_GAP_TIMEOUT: std::time::Duration = std::time::Duration::from_millis(120);
            const RTP_REORDER_LIMIT: usize = 1024;
            let (rtp_tx, mut rtp_rx) = tokio::sync::mpsc::channel(RTP_QUEUE_LIMIT);
            let track_clone_reader = Arc::clone(&track_clone);
            tokio::spawn(async move {
                while let Some(event) = track_clone_reader.poll().await {
                    let rtp_packet = match event {
                        TrackRemoteEvent::OnRtpPacket(p) => p,
                        TrackRemoteEvent::OnEnded => break,
                        _ => continue,
                    };
                    if rtp_packet.payload.is_empty() {
                        continue;
                    }
                    match rtp_tx.try_send(rtp_packet) {
                        Ok(()) => {
                            pending_packets_reader.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                        }
                        Err(tokio::sync::mpsc::error::TrySendError::Full(_)) => {
                            pending_packets_reader.store(RTP_QUEUE_LIMIT, std::sync::atomic::Ordering::SeqCst);
                        }
                        Err(tokio::sync::mpsc::error::TrySendError::Closed(_)) => break,
                    }
                }
            });

            let (pli_tx, mut pli_rx) = tokio::sync::mpsc::unbounded_channel::<()>();
            let pli_track2 = Arc::clone(&track_clone);
            tokio::spawn(async move {
                let mut last_pli_time = std::time::Instant::now();
                while let Some(_) = pli_rx.recv().await {
                    let now = std::time::Instant::now();
                    if now.duration_since(last_pli_time) > std::time::Duration::from_millis(500) {
                        let pli = rtc::rtcp::payload_feedbacks::picture_loss_indication::PictureLossIndication {
                            sender_ssrc: 0,
                            media_ssrc,
                        };
                        if let Err(e) = pli_track2.write_rtcp(vec![Box::new(pli)]).await {
                            eprintln!("Failed to send PLI request: {:?}", e);
                        }
                        last_pli_time = now;
                    }
                }
            });

            let thread_handle = std::thread::spawn(move || {
                let mut decoder = decoder;
                let mut annex_b_buf = annex_b_buf;
                let mut av1_assembler = av1_assembler;
                let mut frame_count = frame_count;
                let mut byte_count = byte_count;
                let mut last_stats_time = last_stats_time;
                let mut total_decode_time_ms = total_decode_time_ms;
                let mut decode_count = decode_count;
                let sink_inner = sink_inner;
                let has_decoded = has_decoded;
                
                let mut next_seq_num: Option<u64> = None;
                let mut gap_since: Option<std::time::Instant> = None;
                let mut packet_buffer = std::collections::BTreeMap::<u64, rtc::rtp::Packet>::new();
                let mut h264_seen_config = false;
                let mut h264_config_annexb = Vec::<u8>::new();
                let mut h264_wait_for_idr = true;
                let mut h264_dropping_fragment = false;
                let mut h264_fragment_is_idr = false;
                let mut h265_seen_config = false;
                let mut h265_config_annexb = Vec::<u8>::new();
                let mut h265_wait_for_irap = true;
                let mut h265_dropping_fragment = false;
                let mut h265_fragment_is_irap = false;
                let mut last_packet_loss_log = std::time::Instant::now()
                    .checked_sub(std::time::Duration::from_secs(60))
                    .unwrap_or_else(std::time::Instant::now);
                let mut packet_loss_events: u64 = 0;
                let mut packets_lost: u64 = 0;
                let mut queue_lag_events: u64 = 0;
                let mut max_queue_lag: usize = 0;
                let mut last_present_log_label = String::new();
                // LUNARIS_CLIENT_STATS=1: one line per second with the
                // interval between decoded frames (stutter diagnosis).
                let stats_log = std::env::var("LUNARIS_CLIENT_STATS").is_ok();
                let stats_frame_log = std::env::var("LUNARIS_CLIENT_STATS").as_deref() == Ok("2");
                let stats_started = std::time::Instant::now();
                let mut stats_first_ts: Option<u32> = None;
                let mut stats_frame_packets: Vec<(u16, f64)> = Vec::new();
                let mut stats_first_at = stats_started;
                let mut stats_prev_frames: usize = 0;
                let mut stats_last_frame_at: Option<std::time::Instant> = None;
                let mut stats_gap_min = f64::MAX;
                let mut stats_gap_max = 0.0f64;
                let mut stats_gap_late: u32 = 0;
                let mut stats_fill_count: u32 = 0;
                let mut stats_fill_max = 0.0f64;
                let mut stats_fill_sum = 0.0f64;

                while let Some(rtp_packet) = rtp_rx.blocking_recv() {
                    let _ = pending_packets_decoder.fetch_update(
                        std::sync::atomic::Ordering::SeqCst,
                        std::sync::atomic::Ordering::SeqCst,
                        |value| Some(value.saturating_sub(1)),
                    );

                    // Check for backlog (decoder lagging)
                    let pending_val = pending_packets_decoder.load(std::sync::atomic::Ordering::SeqCst);
                    if pending_val > RTP_QUEUE_LAG_FLUSH {
                        queue_lag_events = queue_lag_events.saturating_add(1);
                        max_queue_lag = max_queue_lag.max(pending_val);
                        eprintln!("Video receiver queue lag detected ({} packets). Flushing queue...", pending_val);
                        while let Ok(_) = rtp_rx.try_recv() {}
                        pending_packets_decoder.store(0, std::sync::atomic::Ordering::SeqCst);
                        packet_buffer.clear();
                        annex_b_buf.clear();
                        av1_assembler.reset();
                        h264_wait_for_idr = true;
                        h264_dropping_fragment = false;
                        h264_fragment_is_idr = false;
                        h265_wait_for_irap = true;
                        h265_dropping_fragment = false;
                        h265_fragment_is_irap = false;
                        let _ = pli_tx.send(());
                        next_seq_num = None;
                        gap_since = None;
                        continue;
                    }

                    let stats_rtp_ts = rtp_packet.header.timestamp;
                    if stats_frame_log {
                        if stats_first_ts != Some(stats_rtp_ts) {
                            // First packet of a new frame taken off the queue.
                            stats_first_ts = Some(stats_rtp_ts);
                            stats_first_at = std::time::Instant::now();
                            stats_frame_packets.clear();
                        }
                        stats_frame_packets.push((
                            rtp_packet.header.sequence_number,
                            stats_first_at.elapsed().as_secs_f64() * 1000.0,
                        ));
                    }
                    let seq = rtp_packet.header.sequence_number;
                    let ext_seq = match next_seq_num {
                        None => {
                            next_seq_num = Some(seq as u64);
                            seq as u64
                        }
                        Some(next_seq) => {
                            let next_seq_16 = next_seq as u16;
                            let diff = seq.wrapping_sub(next_seq_16) as i16;
                            (next_seq as i64 + diff as i64) as u64
                        }
                    };

                    if ext_seq < next_seq_num.unwrap() {
                        // Old duplicate packet, discard
                        continue;
                    }

                    packet_buffer.insert(ext_seq, rtp_packet);

                    // A gap is open while the next expected packet is missing.
                    // Give NACK retransmissions time to fill it before giving
                    // up on the frame (the old 64-packet limit fired within
                    // one keyframe burst, before any retransmission arrived).
                    let gap_open = next_seq_num
                        .map(|expected| !packet_buffer.contains_key(&expected))
                        .unwrap_or(false);
                    if !gap_open {
                        if let Some(since) = gap_since {
                            // A gap closed: the missing packet arrived
                            // (reordering or a NACK retransmission).
                            let ms = since.elapsed().as_secs_f64() * 1000.0;
                            stats_fill_count += 1;
                            stats_fill_max = stats_fill_max.max(ms);
                            stats_fill_sum += ms;
                        }
                        gap_since = None;
                    } else if gap_since.is_none() {
                        gap_since = Some(std::time::Instant::now());
                    }
                    let gap_expired = gap_since
                        .map(|since| since.elapsed() >= RTP_GAP_TIMEOUT)
                        .unwrap_or(false);

                    if gap_open && (gap_expired || packet_buffer.len() > RTP_REORDER_LIMIT) {
                        gap_since = None;
                        let &first_seq = packet_buffer.keys().next().unwrap();
                        let missing_start = next_seq_num.unwrap();
                        let missing_end = first_seq - 1;
                        let now = std::time::Instant::now();
                        packet_loss_events = packet_loss_events.saturating_add(1);
                        if missing_end >= missing_start {
                            packets_lost = packets_lost.saturating_add(missing_end - missing_start + 1);
                        }
                        if now.duration_since(last_packet_loss_log) >= std::time::Duration::from_secs(1) {
                            eprintln!(
                                "RTP packet loss detected: missing sequence from {} to {}",
                                missing_start,
                                missing_end
                            );
                            last_packet_loss_log = now;
                        }
                        
                        // Clear corrupted frame buffer and request keyframe
                        annex_b_buf.clear();
                        av1_assembler.reset();
                        h264_wait_for_idr = true;
                        h264_dropping_fragment = false;
                        h264_fragment_is_idr = false;
                        h265_wait_for_irap = true;
                        h265_dropping_fragment = false;
                        h265_fragment_is_irap = false;
                        let _ = pli_tx.send(());
                        
                        next_seq_num = Some(first_seq);
                    }

                    // Process all in-order packets
                    while let Some(current_expected) = next_seq_num {
                        if !packet_buffer.contains_key(&current_expected) {
                            break;
                        }
                        let pkt = packet_buffer.remove(&current_expected).unwrap();
                        
                        let payload = &pkt.payload;
                        if payload.is_empty() {
                            next_seq_num = Some(current_expected + 1);
                            continue;
                        }
                        
                        byte_count += payload.len();
                        
                        let mut process_and_deliver = |annex_b_data: &[u8], decoder_ref: &mut super::decoder::HardwareDecoder, decode_count_ref: &mut usize, total_decode_time_ref: &mut f64, frame_count_ref: &mut usize, has_decoded_ref: &Arc<std::sync::atomic::AtomicBool>, sink_ref: &VideoSinkWrapper| -> Result<(), anyhow::Error> {
                            let start_decode = std::time::Instant::now();
                            let decoded_frames = decoder_ref.decode(annex_b_data)?;
                            let decode_time_ms = start_decode.elapsed().as_secs_f64() * 1000.0;
                            
                            if !decoded_frames.is_empty() {
                                if *decode_count_ref == 0 {
                                    has_decoded_ref.store(true, std::sync::atomic::Ordering::SeqCst);
                                }
                                *decode_count_ref += decoded_frames.len();
                                *total_decode_time_ref += decode_time_ms;
                                *frame_count_ref += decoded_frames.len();
                            }
                            
                            for frame in decoded_frames {
                                match frame {
                                    super::decoder::DecodedFrame::NativePresented => {
                                        let label = decoder_ref.presentation_mode_label();
                                        if last_present_log_label != label {
                                            println!("Decoded frame via {}", label);
                                            last_present_log_label = label;
                                        }
                                    }
                                    super::decoder::DecodedFrame::CpuBgrx(frame) => {
                                        let sink_lock = sink_ref.sink.lock().unwrap();
                                        let label = "software decode + BGRX present (4:4:4)".to_string();
                                        if last_present_log_label != label {
                                            println!("Decoded frame via {}: {}x{}", label, frame.width, frame.height);
                                            last_present_log_label = label;
                                        }
                                        if let Some(sink_ptr_val) = *sink_lock {
                                            unsafe {
                                                deliver_bgrx_frame(
                                                    sink_ptr_val as *mut qobject::QVideoSink,
                                                    frame.data.as_ptr(),
                                                    frame.stride,
                                                    frame.width,
                                                    frame.height,
                                                );
                                            }
                                        }
                                    }
                                    super::decoder::DecodedFrame::CpuYuv(frame) => {
                                        let sink_lock = sink_ref.sink.lock().unwrap();
                                        let label = decoder_ref.presentation_mode_label();
                                        if last_present_log_label != label {
                                            println!(
                                                "Decoded frame via {}: {}x{}, sink={:?}",
                                                label,
                                                frame.width,
                                                frame.height,
                                                *sink_lock
                                            );
                                            last_present_log_label = label;
                                        }
                                        if let Some(sink_ptr_val) = *sink_lock {
                                            unsafe {
                                                deliver_yuv_frame(
                                                    sink_ptr_val as *mut qobject::QVideoSink,
                                                    frame.y.as_ptr(), frame.y_stride,
                                                    frame.u.as_ptr(), frame.u_stride,
                                                    frame.v.as_ptr(), frame.v_stride,
                                                    frame.width, frame.height,
                                                );
                                            }
                                        }
                                    }
                                }
                            }
                            Ok(())
                        };
                        
                        match codec_type {
                            super::decoder::CodecType::H264 => {
                                let nal_type = payload[0] & 0x1F;
                                if nal_type >= 1 && nal_type <= 23 {
                                    // Single NAL unit
                                    let is_config = nal_type == 7 || nal_type == 8;
                                    let is_vcl = nal_type >= 1 && nal_type <= 5;
                                    let is_idr = nal_type == 5;

                                    if is_config {
                                        if nal_type == 7 {
                                            h264_config_annexb.clear();
                                        }
                                        h264_seen_config = true;
                                        h264_config_annexb.extend_from_slice(&[0, 0, 0, 1]);
                                        h264_config_annexb.extend_from_slice(payload);
                                    }

                                    annex_b_buf.extend_from_slice(&[0, 0, 0, 1]);
                                    annex_b_buf.extend_from_slice(payload);

                                    if is_vcl {
                                        if h264_wait_for_idr && !is_idr {
                                            // Still no key frame: ask again (the sender task limits this to
                                            // one PLI per 500 ms). Without it a lost or ignored request left
                                            // the stream frozen until some later packet loss.
                                            let _ = pli_tx.send(());
                                            annex_b_buf.clear();
                                            next_seq_num = Some(current_expected + 1);
                                            continue;
                                        }
                                        if is_idr && !h264_seen_config {
                                            annex_b_buf.clear();
                                            h264_wait_for_idr = true;
                                            let _ = pli_tx.send(());
                                            next_seq_num = Some(current_expected + 1);
                                            continue;
                                        }

                                        let h264_decode_buf;
                                        let h264_payload = if is_idr
                                            && !h264_config_annexb.is_empty()
                                            && !annex_b_buf.starts_with(&h264_config_annexb)
                                        {
                                            h264_decode_buf = [h264_config_annexb.as_slice(), annex_b_buf.as_slice()].concat();
                                            h264_decode_buf.as_slice()
                                        } else {
                                            annex_b_buf.as_slice()
                                        };

                                        if let Err(e) = process_and_deliver(h264_payload, &mut decoder, &mut decode_count, &mut total_decode_time_ms, &mut frame_count, &has_decoded, &sink_inner) {
                                            eprintln!("Decoder error: {:?}. Requesting keyframe...", e);
                                            h264_wait_for_idr = true;
                                            let _ = pli_tx.send(());
                                        } else {
                                            h264_wait_for_idr = false;
                                        }
                                        annex_b_buf.clear();
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
                                            let inner_nal_type = nalu_data[0] & 0x1F;
                                            let is_config = inner_nal_type == 7 || inner_nal_type == 8;
                                            let is_vcl = inner_nal_type >= 1 && inner_nal_type <= 5;
                                            let is_idr = inner_nal_type == 5;

                                            if is_config {
                                                if inner_nal_type == 7 {
                                                    h264_config_annexb.clear();
                                                }
                                                h264_seen_config = true;
                                                h264_config_annexb.extend_from_slice(&[0, 0, 0, 1]);
                                                h264_config_annexb.extend_from_slice(nalu_data);
                                            }

                                            annex_b_buf.extend_from_slice(&[0, 0, 0, 1]);
                                            annex_b_buf.extend_from_slice(nalu_data);

                                            if is_vcl {
                                                if h264_wait_for_idr && !is_idr {
                                                    // Still no key frame: ask again (the sender task limits this to
                                                    // one PLI per 500 ms). Without it a lost or ignored request left
                                                    // the stream frozen until some later packet loss.
                                                    let _ = pli_tx.send(());
                                                    annex_b_buf.clear();
                                                    continue;
                                                }
                                                if is_idr && !h264_seen_config {
                                                    annex_b_buf.clear();
                                                    h264_wait_for_idr = true;
                                                    let _ = pli_tx.send(());
                                                    continue;
                                                }

                                                let h264_decode_buf;
                                                let h264_payload = if is_idr
                                                    && !h264_config_annexb.is_empty()
                                                    && !annex_b_buf.starts_with(&h264_config_annexb)
                                                {
                                                    h264_decode_buf = [h264_config_annexb.as_slice(), annex_b_buf.as_slice()].concat();
                                                    h264_decode_buf.as_slice()
                                                } else {
                                                    annex_b_buf.as_slice()
                                                };

                                                if let Err(e) = process_and_deliver(h264_payload, &mut decoder, &mut decode_count, &mut total_decode_time_ms, &mut frame_count, &has_decoded, &sink_inner) {
                                                    eprintln!("Decoder error: {:?}. Requesting keyframe...", e);
                                                    h264_wait_for_idr = true;
                                                    let _ = pli_tx.send(());
                                                } else {
                                                    h264_wait_for_idr = false;
                                                }
                                                annex_b_buf.clear();
                                            }
                                        }
                                    }
                                } else if nal_type == 28 {
                                    // FU-A Fragmentation Unit
                                    if payload.len() < 2 {
                                        next_seq_num = Some(current_expected + 1);
                                        continue;
                                    }
                                    let fu_indicator = payload[0];
                                    let fu_header = payload[1];
                                    let start_bit = (fu_header & 0x80) != 0;
                                    let end_bit = (fu_header & 0x40) != 0;
                                    let inner_nal_type = fu_header & 0x1F;
                                    let reconstructed_header = (fu_indicator & 0xE0) | inner_nal_type;

                                    if start_bit {
                                        let is_idr = inner_nal_type == 5;
                                        h264_fragment_is_idr = is_idr;
                                        h264_dropping_fragment = false;
                                        annex_b_buf.clear();

                                        if h264_wait_for_idr && !is_idr {
                                            // Still no key frame: ask again (one PLI per 500 ms at most).
                                            let _ = pli_tx.send(());
                                            h264_dropping_fragment = true;
                                            next_seq_num = Some(current_expected + 1);
                                            continue;
                                        }
                                        if is_idr && !h264_seen_config {
                                            h264_dropping_fragment = true;
                                            h264_wait_for_idr = true;
                                            let _ = pli_tx.send(());
                                            next_seq_num = Some(current_expected + 1);
                                            continue;
                                        }

                                        annex_b_buf.extend_from_slice(&[0, 0, 0, 1, reconstructed_header]);
                                        annex_b_buf.extend_from_slice(&payload[2..]);
                                    } else if h264_dropping_fragment || annex_b_buf.is_empty() {
                                        if end_bit {
                                            h264_dropping_fragment = false;
                                        }
                                        next_seq_num = Some(current_expected + 1);
                                        continue;
                                    } else {
                                        annex_b_buf.extend_from_slice(&payload[2..]);
                                    }

                                    if end_bit {
                                        h264_dropping_fragment = false;
                                        let h264_decode_buf;
                                        let h264_payload = if h264_fragment_is_idr
                                            && !h264_config_annexb.is_empty()
                                            && !annex_b_buf.starts_with(&h264_config_annexb)
                                        {
                                            h264_decode_buf = [h264_config_annexb.as_slice(), annex_b_buf.as_slice()].concat();
                                            h264_decode_buf.as_slice()
                                        } else {
                                            annex_b_buf.as_slice()
                                        };

                                        if let Err(e) = process_and_deliver(h264_payload, &mut decoder, &mut decode_count, &mut total_decode_time_ms, &mut frame_count, &has_decoded, &sink_inner) {
                                            eprintln!("Decoder error: {:?}. Requesting keyframe...", e);
                                            h264_wait_for_idr = true;
                                            let _ = pli_tx.send(());
                                        } else {
                                            h264_wait_for_idr = false;
                                        }
                                        annex_b_buf.clear();
                                    }
                                }
                            }
                            super::decoder::CodecType::H265 => {
                                let nal_type = (payload[0] & 0x7E) >> 1;
                                let is_h265_config = |t: u8| t == 32 || t == 33 || t == 34;
                                let is_h265_irap = |t: u8| (16..=23).contains(&t);

                                if nal_type <= 47 {
                                    // Single NAL unit
                                    let is_vcl = nal_type <= 31;
                                    let is_irap = is_h265_irap(nal_type);

                                    if is_h265_config(nal_type) {
                                        if nal_type == 32 {
                                            h265_config_annexb.clear();
                                        }
                                        h265_seen_config = true;
                                        h265_config_annexb.extend_from_slice(&[0, 0, 0, 1]);
                                        h265_config_annexb.extend_from_slice(payload);
                                    }

                                    annex_b_buf.extend_from_slice(&[0, 0, 0, 1]);
                                    annex_b_buf.extend_from_slice(payload);

                                    if is_vcl {
                                        if h265_wait_for_irap && !is_irap {
                                            // Still no key frame: ask again (see the H.264 path).
                                            let _ = pli_tx.send(());
                                            annex_b_buf.clear();
                                            next_seq_num = Some(current_expected + 1);
                                            continue;
                                        }
                                        if is_irap && !h265_seen_config {
                                            annex_b_buf.clear();
                                            h265_wait_for_irap = true;
                                            let _ = pli_tx.send(());
                                            next_seq_num = Some(current_expected + 1);
                                            continue;
                                        }

                                        let h265_decode_buf;
                                        let h265_payload = if is_irap
                                            && !h265_config_annexb.is_empty()
                                            && !annex_b_buf.starts_with(&h265_config_annexb)
                                        {
                                            h265_decode_buf = [h265_config_annexb.as_slice(), annex_b_buf.as_slice()].concat();
                                            h265_decode_buf.as_slice()
                                        } else {
                                            annex_b_buf.as_slice()
                                        };

                                        if let Err(e) = process_and_deliver(h265_payload, &mut decoder, &mut decode_count, &mut total_decode_time_ms, &mut frame_count, &has_decoded, &sink_inner) {
                                            eprintln!("Decoder error: {:?}. Requesting keyframe...", e);
                                            h265_wait_for_irap = true;
                                            let _ = pli_tx.send(());
                                        } else {
                                            h265_wait_for_irap = false;
                                        }
                                        annex_b_buf.clear();
                                    }
                                } else if nal_type == 48 {
                                    // AP (Aggregation Packet)
                                    let mut offset = 2; // HEVC payload header is 2 bytes
                                    while offset + 2 <= payload.len() {
                                        let nalu_size = ((payload[offset] as usize) << 8) | (payload[offset + 1] as usize);
                                        offset += 2;
                                        if offset + nalu_size > payload.len() {
                                            break;
                                        }
                                        let nalu_data = &payload[offset..offset + nalu_size];
                                        offset += nalu_size;

                                        if nalu_data.len() >= 2 {
                                            let inner_nal_type = (nalu_data[0] & 0x7E) >> 1;
                                            let is_vcl = inner_nal_type <= 31;
                                            let is_irap = is_h265_irap(inner_nal_type);

                                            if is_h265_config(inner_nal_type) {
                                                if inner_nal_type == 32 {
                                                    h265_config_annexb.clear();
                                                }
                                                h265_seen_config = true;
                                                h265_config_annexb.extend_from_slice(&[0, 0, 0, 1]);
                                                h265_config_annexb.extend_from_slice(nalu_data);
                                            }

                                            annex_b_buf.extend_from_slice(&[0, 0, 0, 1]);
                                            annex_b_buf.extend_from_slice(nalu_data);

                                            if is_vcl {
                                                if h265_wait_for_irap && !is_irap {
                                                    // Still no key frame: ask again (see the H.264 path).
                                                    let _ = pli_tx.send(());
                                                    annex_b_buf.clear();
                                                    continue;
                                                }
                                                if is_irap && !h265_seen_config {
                                                    annex_b_buf.clear();
                                                    h265_wait_for_irap = true;
                                                    let _ = pli_tx.send(());
                                                    continue;
                                                }

                                                let h265_decode_buf;
                                                let h265_payload = if is_irap
                                                    && !h265_config_annexb.is_empty()
                                                    && !annex_b_buf.starts_with(&h265_config_annexb)
                                                {
                                                    h265_decode_buf = [h265_config_annexb.as_slice(), annex_b_buf.as_slice()].concat();
                                                    h265_decode_buf.as_slice()
                                                } else {
                                                    annex_b_buf.as_slice()
                                                };

                                                if let Err(e) = process_and_deliver(h265_payload, &mut decoder, &mut decode_count, &mut total_decode_time_ms, &mut frame_count, &has_decoded, &sink_inner) {
                                                    eprintln!("Decoder error: {:?}. Requesting keyframe...", e);
                                                    h265_wait_for_irap = true;
                                                    let _ = pli_tx.send(());
                                                } else {
                                                    h265_wait_for_irap = false;
                                                }
                                                annex_b_buf.clear();
                                            }
                                        }
                                    }
                                } else if nal_type == 49 {
                                    // FU (Fragmentation Unit)
                                    if payload.len() < 3 {
                                        next_seq_num = Some(current_expected + 1);
                                        continue;
                                    }
                                    let fu_indicator_1 = payload[0];
                                    let fu_indicator_2 = payload[1];
                                    let fu_header = payload[2];
                                    let start_bit = (fu_header & 0x80) != 0;
                                    let end_bit = (fu_header & 0x40) != 0;
                                    let original_nal_type = fu_header & 0x3F;
                                    let is_irap = is_h265_irap(original_nal_type);

                                    let reconstructed_header_1 = (fu_indicator_1 & 0x81) | (original_nal_type << 1);
                                    let reconstructed_header_2 = fu_indicator_2;

                                    if start_bit {
                                        h265_fragment_is_irap = is_irap;
                                        h265_dropping_fragment = false;
                                        annex_b_buf.clear();

                                        if h265_wait_for_irap && !is_irap {
                                            // Still no key frame: ask again (see the H.264 path).
                                            let _ = pli_tx.send(());
                                            h265_dropping_fragment = true;
                                            next_seq_num = Some(current_expected + 1);
                                            continue;
                                        }
                                        if is_irap && !h265_seen_config {
                                            h265_dropping_fragment = true;
                                            h265_wait_for_irap = true;
                                            let _ = pli_tx.send(());
                                            next_seq_num = Some(current_expected + 1);
                                            continue;
                                        }

                                        annex_b_buf.extend_from_slice(&[0, 0, 0, 1, reconstructed_header_1, reconstructed_header_2]);
                                        annex_b_buf.extend_from_slice(&payload[3..]);
                                    } else if h265_dropping_fragment || annex_b_buf.is_empty() {
                                        if end_bit {
                                            h265_dropping_fragment = false;
                                        }
                                        next_seq_num = Some(current_expected + 1);
                                        continue;
                                    } else {
                                        annex_b_buf.extend_from_slice(&payload[3..]);
                                    }

                                    if end_bit {
                                        h265_dropping_fragment = false;
                                        let h265_decode_buf;
                                        let h265_payload = if h265_fragment_is_irap
                                            && !h265_config_annexb.is_empty()
                                            && !annex_b_buf.starts_with(&h265_config_annexb)
                                        {
                                            h265_decode_buf = [h265_config_annexb.as_slice(), annex_b_buf.as_slice()].concat();
                                            h265_decode_buf.as_slice()
                                        } else {
                                            annex_b_buf.as_slice()
                                        };

                                        if let Err(e) = process_and_deliver(h265_payload, &mut decoder, &mut decode_count, &mut total_decode_time_ms, &mut frame_count, &has_decoded, &sink_inner) {
                                            eprintln!("Decoder error: {:?}. Requesting keyframe...", e);
                                            h265_wait_for_irap = true;
                                            let _ = pli_tx.send(());
                                        } else {
                                            h265_wait_for_irap = false;
                                        }
                                        annex_b_buf.clear();
                                    }
                                }
                            }
                            super::decoder::CodecType::AV1 => {
                                // The packets of a frame become one temporal
                                // unit (av1_rtp.rs); the decoder gets it whole.
                                if let Some(unit) = av1_assembler.push(current_expected, pkt.header.timestamp, pkt.header.marker, payload) {
                                    if let Err(e) = process_and_deliver(&unit, &mut decoder, &mut decode_count, &mut total_decode_time_ms, &mut frame_count, &has_decoded, &sink_inner) {
                                        eprintln!("Decoder error: {:?}. Requesting keyframe...", e);
                                        let _ = pli_tx.send(());
                                    }
                                }
                            }
                        }
                        next_seq_num = Some(current_expected + 1);
                    }
                    
                    let now = std::time::Instant::now();
                    if frame_count != stats_prev_frames {
                        if frame_count > stats_prev_frames {
                            if let Some(prev) = stats_last_frame_at {
                                let gap = now.duration_since(prev).as_secs_f64() * 1000.0;
                                stats_gap_min = stats_gap_min.min(gap);
                                stats_gap_max = stats_gap_max.max(gap);
                                if gap > 25.0 {
                                    stats_gap_late += 1;
                                }
                            }
                            stats_last_frame_at = Some(now);
                            // LUNARIS_CLIENT_STATS=2: one line per decoded
                            // frame with its RTP timestamp (90 kHz, the
                            // host's clock) and the local time in ms, to tell
                            // uneven sending from uneven arrival.
                            if stats_frame_log {
                                eprintln!(
                                    "Frame: rtp_ts={} at_ms={:.2} first_ms={:.2}",
                                    stats_rtp_ts,
                                    now.duration_since(stats_started).as_secs_f64() * 1000.0,
                                    stats_first_at.duration_since(stats_started).as_secs_f64() * 1000.0
                                );
                                if now.duration_since(stats_first_at).as_secs_f64() * 1000.0 > 12.0 {
                                    // A frame that took long from its first packet: every
                                    // packet's sequence number and ms after the first.
                                    let detail: Vec<String> = stats_frame_packets
                                        .iter()
                                        .map(|(seq, ms)| format!("{}@{:.1}", seq, ms))
                                        .collect();
                                    eprintln!("Late frame packets: {}", detail.join(" "));
                                }
                            }
                        }
                        stats_prev_frames = frame_count;
                    }
                    let elapsed = now.duration_since(last_stats_time).as_secs_f64();
                    if elapsed >= 1.0 {
                        let fps = frame_count as f64 / elapsed;
                        if stats_log {
                            println!(
                                "Client stats: fps={:.1} frame_gap_ms(min/max)={:.1}/{:.1} gaps_over_25ms={} decode_ms={:.2} lost_packets={} loss_events={} queue_lag_events={} gaps_filled={} fill_ms(avg/max)={:.0}/{:.0}",
                                fps,
                                if stats_gap_min == f64::MAX { 0.0 } else { stats_gap_min },
                                stats_gap_max,
                                stats_gap_late,
                                if decode_count > 0 { total_decode_time_ms / decode_count as f64 } else { 0.0 },
                                packets_lost,
                                packet_loss_events,
                                queue_lag_events,
                                stats_fill_count,
                                if stats_fill_count > 0 { stats_fill_sum / stats_fill_count as f64 } else { 0.0 },
                                stats_fill_max
                            );
                        }
                        stats_fill_count = 0;
                        stats_fill_max = 0.0;
                        stats_fill_sum = 0.0;
                        stats_gap_min = f64::MAX;
                        stats_gap_max = 0.0;
                        stats_gap_late = 0;
                        stats_prev_frames = 0;
                        let bitrate_kbps = (byte_count as f64 * 8.0) / 1000.0 / elapsed;
                        let avg_decode_ms = if decode_count > 0 {
                            total_decode_time_ms / decode_count as f64
                        } else {
                            0.0
                        };
                        
                        let negotiated_codec = match codec_type {
                            super::decoder::CodecType::H264 => "H264",
                            super::decoder::CodecType::H265 => "H265",
                            super::decoder::CodecType::AV1 => "AV1",
                        }.to_string();
                        
                        let decode_backend = decoder.decode_backend_label().to_string();
                        let present_backend = decoder.present_backend_label().to_string();
                        let gpu_decode_enabled = decoder.gpu_decode_enabled();
                        let fallback_reason = decoder.fallback_reason();
                        let mut stats_lock = STREAM_STATS.lock().unwrap();
                        if let Some(ref mut s) = *stats_lock {
                            s.decode = avg_decode_ms;
                            s.fps = fps;
                            s.bitrate = bitrate_kbps;
                            s.requested_codec = requested_codec.clone();
                            s.negotiated_codec = negotiated_codec;
                            s.decode_backend = decode_backend;
                            s.present_backend = present_backend;
                            s.gpu_decode_enabled = gpu_decode_enabled;
                            s.fallback_reason = fallback_reason;
                            s.packet_loss_events = packet_loss_events;
                            s.packets_lost = packets_lost;
                            s.queue_lag_events = queue_lag_events;
                            s.max_queue_lag = max_queue_lag;
                        } else {
                            *stats_lock = Some(StreamStats {
                                ping: 0.0,
                                decode: avg_decode_ms,
                                fps,
                                bitrate: bitrate_kbps,
                                requested_codec: requested_codec.clone(),
                                negotiated_codec,
                                decode_backend,
                                present_backend,
                                gpu_decode_enabled,
                                fallback_reason,
                                packet_loss_events,
                                packets_lost,
                                queue_lag_events,
                                max_queue_lag,
                                connection_type: "P2P (Direct)".to_string(),
                            });
                        }
                        
                        frame_count = 0;
                        byte_count = 0;
                        decode_count = 0;
                        total_decode_time_ms = 0.0;
                        last_stats_time = now;
                    }
                }
            });
            *active_decoder_inner.lock().unwrap() = Some(thread_handle);
        } else if mime == "audio/opus" {
            println!("Starting audio receiver for: {}", mime);
            // Read audio on its own task: webrtc-rs waits for this on_track
            // handler to return before dispatching the next track, so a read
            // loop here would starve the video track (black screen).
            tokio::spawn(async move {
                if let Some(audio_tx) = super::audio::setup_audio() {
                    let mut decoder = match opus::Decoder::new(48000, opus::Channels::Stereo) {
                        Ok(d) => d,
                        Err(e) => {
                            eprintln!("Failed to create Opus decoder: {:?}", e);
                            return;
                        }
                    };
                    let mut pcm_output = vec![0.0f32; 1920 * 2];

                    while let Some(event) = track_clone.poll().await {
                        let rtp_packet = match event {
                            TrackRemoteEvent::OnRtpPacket(p) => p,
                            TrackRemoteEvent::OnEnded => break,
                            _ => continue,
                        };
                        if let Ok(num_samples) = decoder.decode_float(&rtp_packet.payload, &mut pcm_output, false) {
                            let stereo_samples = pcm_output[..num_samples * 2].to_vec();
                            let _ = audio_tx.send(stereo_samples);
                        }
                    }
                }
            });
        }
}

async fn setup_peer_connection(
    ice_servers: Option<Vec<common::RtcIceServer>>,
    outbox_tx: tokio::sync::mpsc::UnboundedSender<ClientMessage>,
    host_id: String,
    sink_wrapper: VideoSinkWrapper,
    kb_chan_ref: Arc<Mutex<Option<Arc<dyn DataChannel>>>>,
    ma_chan_ref: Arc<Mutex<Option<Arc<dyn DataChannel>>>>,
    mr_chan_ref: Arc<Mutex<Option<Arc<dyn DataChannel>>>>,
    active_decoder: Arc<Mutex<Option<std::thread::JoinHandle<()>>>>,
    codec: &str,
) -> Result<(Arc<dyn PeerConnection>, ConnState), anyhow::Error> {
    // WebRTC connection setup
    let mut media_engine = MediaEngine::default();

    // Register Opus
    media_engine.register_codec(
        RTCRtpCodecParameters {
            rtp_codec: RTCRtpCodec {
                mime_type: "audio/opus".to_string(),
                clock_rate: 48000,
                channels: 2,
                sdp_fmtp_line: "minptime=10;useinbandfec=1".to_string(),
                rtcp_feedback: vec![],
            },
            payload_type: 111,
        },
        RtpCodecKind::Audio,
    )?;

    // Register only the requested video codec to avoid SDP answer mismatch
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

    match codec {
        "h265" | "hevc" => {
            media_engine.register_codec(
                RTCRtpCodecParameters {
                    rtp_codec: RTCRtpCodec {
                        mime_type: MIME_TYPE_HEVC.to_string(),
                        clock_rate: 90000,
                        channels: 0,
                        sdp_fmtp_line: "profile-id=1;tier-flag=0;level-id=120;tx-mode=SRST"
                            .to_string(),
                        rtcp_feedback: video_rtcp_feedback.clone(),
                    },
                    payload_type: 98,
                },
                RtpCodecKind::Video,
            )?;
        }
        "av1" => {
            media_engine.register_codec(
                RTCRtpCodecParameters {
                    rtp_codec: RTCRtpCodec {
                        mime_type: MIME_TYPE_AV1.to_string(),
                        clock_rate: 90000,
                        channels: 0,
                        sdp_fmtp_line: "profile=0".to_string(),
                        rtcp_feedback: video_rtcp_feedback.clone(),
                    },
                    payload_type: 102,
                },
                RtpCodecKind::Video,
            )?;
        }
        _ => {
            // Default to H264
            media_engine.register_codec(
                RTCRtpCodecParameters {
                    rtp_codec: RTCRtpCodec {
                        mime_type: MIME_TYPE_H264.to_string(),
                        clock_rate: 90000,
                        channels: 0,
                        sdp_fmtp_line:
                            "level-asymmetry-allowed=1;packetization-mode=1;profile-level-id=42002a"
                                .to_string(),
                        rtcp_feedback: video_rtcp_feedback.clone(),
                    },
                    payload_type: 96,
                },
                RtpCodecKind::Video,
            )?;
            // High 4:4:4 Predictive, offered by the agent for chroma=444
            // sessions; without it the answer has no matching video codec.
            media_engine.register_codec(
                RTCRtpCodecParameters {
                    rtp_codec: RTCRtpCodec {
                        mime_type: MIME_TYPE_H264.to_string(),
                        clock_rate: 90000,
                        channels: 0,
                        sdp_fmtp_line:
                            "level-asymmetry-allowed=1;packetization-mode=1;profile-level-id=f4002a"
                                .to_string(),
                        rtcp_feedback: video_rtcp_feedback.clone(),
                    },
                    payload_type: 97,
                },
                RtpCodecKind::Video,
            )?;
        }
    }

    // FlexFEC-03: accept the host's repair stream (payload type and fmtp as
    // the host offers them, see agent/src/bridge.rs; the reference project
    // registers it the same way) so that the answer keeps it.
    media_engine.register_codec(
        RTCRtpCodecParameters {
            rtp_codec: RTCRtpCodec {
                mime_type: MIME_TYPE_FLEX_FEC03.to_owned(),
                clock_rate: 90000,
                channels: 0,
                sdp_fmtp_line: "repair-window=10000000".to_owned(),
                rtcp_feedback: video_rtcp_feedback,
            },
            payload_type: 49,
        },
        RtpCodecKind::Video,
    )?;

    // Register RTP header extensions — critical for low-latency video and codec matching
    const PLAYOUT_DELAY_URI: &str = "http://www.webrtc.org/experiments/rtp-hdrext/playout-delay";
    media_engine.register_header_extension(
        RTCRtpHeaderExtensionCapability {
            uri: PLAYOUT_DELAY_URI.to_string(),
        },
        RtpCodecKind::Video,
        None,
    )?;
    media_engine.register_header_extension(
        RTCRtpHeaderExtensionCapability {
            uri: rtc::sdp::extmap::ABS_SEND_TIME_URI.to_string(),
        },
        RtpCodecKind::Video,
        None,
    )?;

    let mut registry = Registry::new();
    // What register_default_interceptors does (webrtc 0.17
    // api/interceptor_registry: configure_nack, configure_rtcp_reports,
    // configure_twcc_receiver_only), except that lost packets are asked for
    // again every 20 ms instead of the library's 100 ms. With 100 ms a
    // retransmission arrives 100-110 ms after the loss, at the edge of
    // RTP_GAP_TIMEOUT below: measured over a tunnel with ~0.5% loss, the
    // client then dropped the frame, asked for a key frame whose own packets
    // were lost again, and never recovered, while Chrome on the same path
    // kept 60 fps. 20 ms is the period of libwebrtc's NackRequester
    // (kUpdateInterval).
    {
        for parameter in ["", "pli"] {
            media_engine.register_feedback(
                RTCPFeedback { typ: "nack".to_owned(), parameter: parameter.to_owned() },
                RtpCodecKind::Video,
            );
        }
        registry = registry
            .with(Slot::NackResponder, NackResponderBuilder::new().build())
            .with(
                Slot::NackGenerator,
                NackGeneratorBuilder::new()
                    .with_interval(std::time::Duration::from_millis(20))
                    .build(),
            );
    }
    registry = configure_rtcp_reports(registry);
    registry = configure_twcc_receiver_only(registry, &mut media_engine)?;
    // FlexFEC-03 receive side (slot 6_000, before the NACK generator: a
    // packet it rebuilds counts as arrived and is not asked for again).
    // Recovered packets re-enter the chain as ordinary media RTP, so they
    // reach `TrackRemote::poll` as `OnRtpPacket`; repair packets never do.
    registry = registry.with(Slot::FecDecoder, FlexFec03ReceiveBuilder::new().build());

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
        vec![RTCIceServer {
            urls: vec!["stun:stun.l.google.com:19302".to_string()],
            ..Default::default()
        }]
    };

    let config = RTCConfigurationBuilder::default()
        .with_ice_servers(webrtc_ice_servers)
        .build();

    let state = ConnState(Arc::new(std::sync::Mutex::new((
        RTCIceConnectionState::New,
        RTCPeerConnectionState::New,
    ))));
    let handler = Arc::new(ClientEventHandler {
        outbox_tx,
        host_id,
        sink_wrapper,
        kb_chan_ref,
        ma_chan_ref,
        mr_chan_ref,
        active_decoder,
        state: state.clone(),
    });

    let peer_connection: Arc<dyn PeerConnection> = Arc::new(
        PeerConnectionBuilder::new()
            .with_configuration(config)
            .with_media_engine(media_engine)
            .with_interceptor_registry(registry)
            // The library's SRTP replay window is 64 packets; at 20 Mbit/s
            // that is about 30 ms of video, so a packet the host sent again
            // after a NACK (20 ms interval + round trip) was rejected as
            // "duplicated" and the loss was never repaired. libwebrtc uses
            // 1024 (pc/srtp_session.cc, `policy.window_size = 1024`).
            .with_setting_engine(
                rtc::peer_connection::configuration::setting_engine::SettingEngineBuilder::new()
                    .with_srtp_replay_protection_window(1024)
                    // The ping shown is the round trip of ICE's check of the
                    // pair in use: one STUN request and its answer, timed
                    // (rtc-ice candidate_pair.rs `on_response_received` sets
                    // `current_round_trip_time`), sent every keepalive
                    // interval (agent/mod.rs `check_keepalive`), which is 2 s
                    // unless said otherwise (agent_config.rs
                    // `DEFAULT_KEEPALIVE_INTERVAL`). Every second here, so
                    // that the ping is a second old at most. The other two
                    // times stay the library's (None).
                    .with_ice_timeouts(None, None, Some(std::time::Duration::from_secs(1)))
                    .build(),
            )
            .with_handler(handler)
            // Every IPv4 interface (0.17 gathered them all, no loopback).
            .with_udp_addrs(vec!["0.0.0.0:0"])
            .build()
            .await?,
    );

    Ok((peer_connection, state))
}

async fn send_relative_packet(
    packet: &[u8],
    mr_c: &Arc<Mutex<Option<Arc<dyn DataChannel>>>>,
    cached_chan: &mut Option<Arc<dyn DataChannel>>,
    wt_c: &Arc<Mutex<Option<wtransport::Connection>>>,
    has_wt: &Arc<std::sync::atomic::AtomicBool>,
    mouse_queue_limit: u32,
) {
    if cached_chan.is_none() {
        *cached_chan = mr_c.lock().unwrap().clone();
    }

    let wt_sent = if has_wt.load(std::sync::atomic::Ordering::Acquire) {
        let lock = wt_c.lock().unwrap();
        if let Some(ref conn) = *lock {
            let mut wt_buf = vec![0u8; packet.len() + 1];
            wt_buf[0] = 6; // Channel 6: mouse_relative
            wt_buf[1..].copy_from_slice(packet);
            if let Err(e) = conn.send_datagram(&wt_buf) {
                eprintln!("WebTransport send_datagram mouse_rel failed: {:?}", e);
                false
            } else {
                true
            }
        } else {
            false
        }
    } else {
        false
    };

    if !wt_sent {
        if let Some(ref chan) = cached_chan {
            let mut cached_buffer_ok = true;
            if mouse_queue_limit > 0 {
                static LAST_CHECK: std::sync::Mutex<Option<std::time::Instant>> =
                    std::sync::Mutex::new(None);
                static LAST_RESULT: std::sync::atomic::AtomicBool =
                    std::sync::atomic::AtomicBool::new(true);
                let now = std::time::Instant::now();
                let should_check = {
                    let mut guard = LAST_CHECK.lock().unwrap();
                    match *guard {
                        Some(last) if now.duration_since(last).as_millis() < 100 => false,
                        _ => {
                            *guard = Some(now);
                            true
                        }
                    }
                };
                if should_check {
                    // 0.21 has no bufferedAmount: the bytes SCTP has not released yet.
                    let amt = chan.outstanding_bytes().await.unwrap_or(0);
                    let limit = if mouse_queue_limit < 10240 {
                        65536
                    } else {
                        mouse_queue_limit as usize
                    };
                    let ok = amt < limit;
                    LAST_RESULT.store(ok, std::sync::atomic::Ordering::Relaxed);
                    cached_buffer_ok = ok;
                } else {
                    cached_buffer_ok = LAST_RESULT.load(std::sync::atomic::Ordering::Relaxed);
                }
            }

            if cached_buffer_ok {
                let _ = chan.send(bytes::BytesMut::from(packet)).await;
            }
        }
    }
}

async fn run_webrtc_client_task(
    host_id: String,
    server_url: String,
    token: String,
    width: u32,
    height: u32,
    fps: u32,
    bitrate: u32,
    codec_str: String,
    app_id: Option<u32>,
    mouse_queue_limit: u32,
    input_protocol: String,
    encoder: Option<String>,
    display_id: Option<String>,
    source_id: Option<String>,
    sink_wrapper: VideoSinkWrapper,
    input_senders: Arc<Mutex<Option<super::input::InputSenders>>>,
    active_decoder: Arc<Mutex<Option<std::thread::JoinHandle<()>>>>,
) -> Result<(), anyhow::Error> {
    let ws_stream = if let Some(address) = direct_address(&server_url, &host_id) {
        // Straight to the agent: TLS and a password proof, no password in the URL.
        println!("Connecting directly to {}", address);
        agent::direct::connect(address, "/ws/client", &token, &crate::session_info::client_name_query()).await?.ws
    } else {
        let encoded_token: String = url::form_urlencoded::byte_serialize(token.as_bytes()).collect();
        let ws_base = signaling_ws_base(&server_url);
        let ws_url = format!(
            "{}/ws/client?token={}{}",
            ws_base,
            encoded_token,
            crate::session_info::client_name_query()
        );
        // Never log ws_url itself: it carries the session token.
        println!("Connecting to signaling server at: {}/ws/client", ws_base);
        connect_async(ws_url.as_str()).await?.0
    };
    let (mut ws_write, mut ws_read) = ws_stream.split();

    let (outbox_tx, mut outbox_rx) = tokio::sync::mpsc::unbounded_channel::<ClientMessage>();

    let mut peer_connection: Option<Arc<dyn PeerConnection>> = None;
    let mut pending_remote_ice: Vec<RTCIceCandidateInit> = Vec::new();

    // Setup input channels
    let (kb_tx, mut kb_rx) = tokio::sync::mpsc::unbounded_channel::<Bytes>();
    let (ma_tx, mut ma_rx) = tokio::sync::mpsc::unbounded_channel::<Bytes>();
    let (mr_tx, mut mr_rx) = tokio::sync::mpsc::unbounded_channel::<Bytes>();

    let senders = super::input::InputSenders {
        keyboard: kb_tx,
        mouse_abs: ma_tx,
        mouse_rel: mr_tx,
        stream_width: width,
        stream_height: height,
    };
    *input_senders.lock().unwrap() = Some(senders);

    // Setup data channel callbacks
    let kb_chan_ref: Arc<Mutex<Option<Arc<dyn DataChannel>>>> = Arc::new(Mutex::new(None));
    let ma_chan_ref: Arc<Mutex<Option<Arc<dyn DataChannel>>>> = Arc::new(Mutex::new(None));
    let mr_chan_ref: Arc<Mutex<Option<Arc<dyn DataChannel>>>> = Arc::new(Mutex::new(None));

    let wt_conn_ref: Arc<Mutex<Option<wtransport::Connection>>> = Arc::new(Mutex::new(None));
    let wt_connected: Arc<std::sync::atomic::AtomicBool> =
        Arc::new(std::sync::atomic::AtomicBool::new(false));

    // Spawn input senders tasks
    let k_c = Arc::clone(&kb_chan_ref);
    let wt_c = Arc::clone(&wt_conn_ref);
    let has_wt = Arc::clone(&wt_connected);
    tokio::spawn(async move {
        while let Some(buf) = kb_rx.recv().await {
            // Keys go on the reliable, ordered data channel whenever there is
            // one: a WebTransport datagram may be lost or arrive out of
            // order, and a lost release leaves the key held on the host.
            let chan = { k_c.lock().unwrap().clone() };
            if let Some(chan) = chan {
                if chan.send(bytes::BytesMut::from(&buf[..])).await.is_ok() {
                    continue;
                }
            }
            let wt_sent = if has_wt.load(std::sync::atomic::Ordering::Acquire) {
                let lock = wt_c.lock().unwrap();
                if let Some(ref conn) = *lock {
                    let mut wt_buf = vec![0u8; buf.len() + 1];
                    wt_buf[0] = 7; // Channel 7: keyboard
                    wt_buf[1..].copy_from_slice(&buf);
                    if let Err(e) = conn.send_datagram(&wt_buf) {
                        eprintln!("WebTransport send_datagram keyboard failed: {:?}", e);
                        false
                    } else {
                        true
                    }
                } else {
                    false
                }
            } else {
                false
            };

            if !wt_sent {
                let chan = { k_c.lock().unwrap().clone() };
                if let Some(chan) = chan {
                    let _ = chan.send(bytes::BytesMut::from(&buf[..])).await;
                }
            }
        }
    });

    let ma_c = Arc::clone(&ma_chan_ref);
    let wt_c = Arc::clone(&wt_conn_ref);
    let has_wt = Arc::clone(&wt_connected);
    tokio::spawn(async move {
        let mut cached_chan: Option<Arc<dyn DataChannel>> = None;
        loop {
            let first_buf = match ma_rx.recv().await {
                Some(buf) => buf,
                None => break,
            };

            let mut latest_buf = first_buf;
            // Drain any pending absolute mouse events to get the freshest one
            while let Ok(buf) = ma_rx.try_recv() {
                latest_buf = buf;
            }

            if cached_chan.is_none() {
                cached_chan = ma_c.lock().unwrap().clone();
            }

            let wt_sent = if has_wt.load(std::sync::atomic::Ordering::Acquire) {
                let lock = wt_c.lock().unwrap();
                if let Some(ref conn) = *lock {
                    let mut wt_buf = vec![0u8; latest_buf.len() + 1];
                    wt_buf[0] = 5; // Channel 5: mouse_absolute
                    wt_buf[1..].copy_from_slice(&latest_buf);
                    if let Err(e) = conn.send_datagram(&wt_buf) {
                        eprintln!("WebTransport send_datagram mouse_abs failed: {:?}", e);
                        false
                    } else {
                        true
                    }
                } else {
                    false
                }
            } else {
                false
            };

            if !wt_sent {
                if let Some(ref chan) = cached_chan {
                    let mut cached_buffer_ok = true;
                    if mouse_queue_limit > 0 {
                        static LAST_CHECK: std::sync::Mutex<Option<std::time::Instant>> =
                            std::sync::Mutex::new(None);
                        static LAST_RESULT: std::sync::atomic::AtomicBool =
                            std::sync::atomic::AtomicBool::new(true);
                        let now = std::time::Instant::now();
                        let should_check = {
                            let mut guard = LAST_CHECK.lock().unwrap();
                            match *guard {
                                Some(last) if now.duration_since(last).as_millis() < 100 => false,
                                _ => {
                                    *guard = Some(now);
                                    true
                                }
                            }
                        };
                        if should_check {
                            // 0.21 has no bufferedAmount: the bytes SCTP has not released yet.
                    let amt = chan.outstanding_bytes().await.unwrap_or(0);
                            let limit = if mouse_queue_limit < 10240 {
                                65536
                            } else {
                                mouse_queue_limit as usize
                            };
                            let ok = amt < limit;
                            LAST_RESULT.store(ok, std::sync::atomic::Ordering::Relaxed);
                            cached_buffer_ok = ok;
                        } else {
                            cached_buffer_ok =
                                LAST_RESULT.load(std::sync::atomic::Ordering::Relaxed);
                        }
                    }

                    if cached_buffer_ok {
                        let _ = chan.send(bytes::BytesMut::from(&latest_buf[..])).await;
                    }
                }
            }
        }
    });

    let mr_c = Arc::clone(&mr_chan_ref);
    let wt_c = Arc::clone(&wt_conn_ref);
    let has_wt = Arc::clone(&wt_connected);
    tokio::spawn(async move {
        let mut cached_chan: Option<Arc<dyn DataChannel>> = None;
        loop {
            let first_buf = match mr_rx.recv().await {
                Some(buf) => buf,
                None => break,
            };

            let mut accumulated_dx: i16 = 0;
            let mut accumulated_dy: i16 = 0;
            let mut has_motion = false;

            // Process a packet and return it if it's not a relative motion event
            let process_packet = |buf: bytes::Bytes,
                                  acc_dx: &mut i16,
                                  acc_dy: &mut i16,
                                  has_mot: &mut bool|
             -> Option<bytes::Bytes> {
                if buf[0] == 0 {
                    *acc_dx = acc_dx.wrapping_add(i16::from_be_bytes([buf[1], buf[2]]));
                    *acc_dy = acc_dy.wrapping_add(i16::from_be_bytes([buf[3], buf[4]]));
                    *has_mot = true;
                    None
                } else {
                    Some(buf)
                }
            };

            let mut non_motion_buf = process_packet(
                first_buf,
                &mut accumulated_dx,
                &mut accumulated_dy,
                &mut has_motion,
            );

            // Drain any pending relative mouse packets in queue to aggregate them
            while non_motion_buf.is_none() {
                match mr_rx.try_recv() {
                    Ok(buf) => {
                        non_motion_buf = process_packet(
                            buf,
                            &mut accumulated_dx,
                            &mut accumulated_dy,
                            &mut has_motion,
                        );
                    }
                    Err(_) => break,
                }
            }

            if has_motion && (accumulated_dx != 0 || accumulated_dy != 0) {
                let mut motion_buf = vec![0u8; 5];
                motion_buf[0] = 0;
                motion_buf[1..3].copy_from_slice(&accumulated_dx.to_be_bytes());
                motion_buf[3..5].copy_from_slice(&accumulated_dy.to_be_bytes());
                send_relative_packet(
                    &motion_buf,
                    &mr_c,
                    &mut cached_chan,
                    &wt_c,
                    &has_wt,
                    mouse_queue_limit,
                )
                .await;
            }

            if let Some(buf) = non_motion_buf {
                send_relative_packet(
                    &buf,
                    &mr_c,
                    &mut cached_chan,
                    &wt_c,
                    &has_wt,
                    mouse_queue_limit,
                )
                .await;
            }
        }
    });

    // Write outgoing WS messages
    tokio::spawn(async move {
        while let Some(msg) = outbox_rx.recv().await {
            if let Ok(text) = serde_json::to_string(&msg) {
                if let Err(e) = ws_write.send(WsMessage::Text(text.into())).await {
                    eprintln!("WS write error: {:?}", e);
                    break;
                }
            }
        }
    });

    // "h264_444" (menu choice "H.264 4:4:4") is H.264 with full chroma, which
    // the agent encodes as High 4:4:4 when its encoder can (like the web
    // client's chroma setting).
    let (request_codec, request_chroma) = split_chroma_codec(&codec_str);
    let mut codec_str = codec_str;

    // Colourspace and range asked of the host (None: the host default); the
    // untagged-frame fallbacks of the renderers assume what was asked.
    let requested_csc = requested_encoder_csc_mode();
    if let Some(mode) = requested_csc {
        println!("Requesting encoderCscMode {} (colour space {}, {} range)", mode, mode >> 1, if mode & 1 != 0 { "full" } else { "limited" });
    }
    crate::decoder::set_requested_encoder_csc_mode(requested_csc.unwrap_or(HOST_DEFAULT_ENCODER_CSC_MODE));

    // The session request, sent with the codec to use.
    let request_template = SignalingMessage::RequestSession {
        host_id: host_id.clone(),
        width: Some(width),
        height: Some(height),
        fps: Some(fps),
        bitrate: Some(bitrate),
        codec: Some(request_codec.clone()),
        app_id,
        encoder,
        display_id,
        chroma: request_chroma,
        encoder_csc_mode: requested_csc,
        microphone: crate::audio::record_microphone().then_some(true),
        source_id,
        // What this client decodes: a source is passed on as it is encoded,
        // so the host has to know what it may send.
        decode_codecs: Some(decode_codecs_for_request(crate::decoder::decodable_codecs(), &request_codec)),
    };
    let request_with_codec = move |codec: &str| -> ClientMessage {
        let mut sig = request_template.clone();
        if let SignalingMessage::RequestSession { codec: c, .. } = &mut sig {
            *c = Some(codec.to_string());
        }
        ClientMessage::Signaling(sig)
    };

    // "auto": pick the codec from what the host can encode in hardware
    // (asked first; H.264 if it doesn't answer in time).
    let auto_pending = Arc::new(std::sync::atomic::AtomicBool::new(request_codec == "auto"));
    if request_codec == "auto" {
        outbox_tx.send(ClientMessage::Signaling(SignalingMessage::GetCapabilities {
            target_id: host_id.clone(),
        }))?;
        let pending = auto_pending.clone();
        let outbox = outbox_tx.clone();
        let fallback = request_with_codec("h264");
        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_secs(3)).await;
            if pending.swap(false, std::sync::atomic::Ordering::SeqCst) {
                println!("Codec auto: no capabilities from the host in time, using H.264");
                let _ = outbox.send(fallback);
            }
        });
    } else {
        outbox_tx.send(request_with_codec(&request_codec))?;
    }

    // Read incoming WS messages
    while let Some(msg_res) = ws_read.next().await {
        let ws_msg = match msg_res {
            Ok(m) => m,
            Err(e) => {
                eprintln!("WS read error: {:?}", e);
                break;
            }
        };

        if let WsMessage::Text(text) = ws_msg {
            let server_msg: ServerToClientMessage = match serde_json::from_str(&text) {
                Ok(m) => m,
                Err(e) => {
                    eprintln!("Failed to parse server message: {}", e);
                    continue;
                }
            };

            match server_msg {
                ServerToClientMessage::Signaling(sig) => match sig {
                    SignalingMessage::CapabilitiesResponse {
                        displays,
                        encoders,
                        gpu_info,
                        host_os,
                        sources,
                        ..
                    } => {
                        if auto_pending.swap(false, std::sync::atomic::Ordering::SeqCst) {
                            let chosen = auto_codec_for_host(&encoders, gpu_info.as_deref());
                            println!("Codec auto: {} (host GPU {:?})", chosen, gpu_info);
                            codec_str = chosen.to_string();
                            outbox_tx.send(request_with_codec(chosen))?;
                        }
                        let host_os_str = host_os.clone().unwrap_or_default();
                        let host_os_lower = host_os_str.to_ascii_lowercase();
                        if !host_os_lower.is_empty() {
                            *PENDING_HOST_OS.lock().unwrap() = Some(host_os_lower.clone());
                        }
                        *PENDING_HOST_INFO.lock().unwrap() = Some(PendingHostInfo {
                            gpu_info: gpu_info.clone().unwrap_or_default(),
                            host_os: host_os_lower,
                        });
                        let displays_json =
                            serde_json::to_string(&displays).unwrap_or_else(|_| "[]".to_string());
                        let encoders_json =
                            serde_json::to_string(&encoders).unwrap_or_else(|_| "[]".to_string());
                        *PENDING_CAPABILITIES.lock().unwrap() = Some(PendingCapabilities {
                            displays_json,
                            encoders_json,
                            sources_json: serde_json::to_string(&sources).unwrap_or_else(|_| "[]".to_string()),
                            gpu_info: gpu_info.unwrap_or_default(),
                            host_os: host_os_str,
                        });
                    }
                    SignalingMessage::EncoderStatus {
                        encoder,
                        hw_type,
                        gpu_info,
                        requested_encoder,
                        host_os,
                        display_id,
                        display_name,
                        ..
                    } => {
                        let host_os = host_os.unwrap_or_default().to_ascii_lowercase();
                        if !host_os.is_empty() {
                            *PENDING_HOST_OS.lock().unwrap() = Some(host_os.clone());
                        }
                        *PENDING_ENCODER_STATUS.lock().unwrap() = Some(PendingEncoderStatus {
                            encoder,
                            hw_type,
                            gpu_info: gpu_info.unwrap_or_default(),
                            requested_encoder: requested_encoder
                                .unwrap_or_else(|| "auto".to_string()),
                            host_os,
                            display_id: display_id.unwrap_or_default(),
                            display_name: display_name.unwrap_or_default(),
                        });
                    }
                    SignalingMessage::Sdp {
                        sdp,
                        ice_servers,
                        webtransport_port,
                        webtransport_cert_hash,
                        webtransport_token,
                        ..
                    } => {
                        if sdp.ty == RtcSdpType::Offer {
                            // WebTransport input starts once ICE is connected (below).
                            let wt_target = if input_protocol == "webtransport" {
                                webtransport_port.map(|port| {
                                    (port, webtransport_cert_hash.clone(), webtransport_token.clone())
                                })
                            } else {
                                None
                            };

                            let offer_codec = select_video_codec_for_offer(&codec_str, &sdp.sdp);
                            if offer_codec != codec_str {
                                println!(
                                    "Client codec preference adjusted to match SDP offer: requested={}, offer={}",
                                    codec_str, offer_codec
                                );
                            }

                            // A new offer (agent restarted the session) replaces the
                            // previous connection: close it, or its ICE agent keeps
                            // checking and competing with the new one.
                            if let Some(old_pc) = peer_connection.take() {
                                println!("New SDP offer: closing the previous peer connection");
                                let _ = old_pc.close().await;
                            }

                            let (pc, pc_state) = match setup_peer_connection(
                                ice_servers,
                                outbox_tx.clone(),
                                host_id.clone(),
                                sink_wrapper.clone(),
                                kb_chan_ref.clone(),
                                ma_chan_ref.clone(),
                                mr_chan_ref.clone(),
                                active_decoder.clone(),
                                &offer_codec,
                            )
                            .await
                            {
                                Ok(pair) => pair,
                                Err(e) => {
                                    eprintln!("Failed to setup peer connection: {:?}", e);
                                    continue;
                                }
                            };

                            if let Some((port, cert_hash, token)) = wt_target {
                                tokio::spawn(connect_webtransport_input(
                                    Arc::clone(&pc),
                                    pc_state.clone(),
                                    server_url.clone(),
                                    port,
                                    cert_hash,
                                    token,
                                    Arc::clone(&wt_conn_ref),
                                    Arc::clone(&wt_connected),
                                ));
                            }

                            match RTCSessionDescription::offer(sdp.sdp.clone()) {
                                Ok(rtc_sdp) => {
                                    println!("SDP Offer received:\n{}", sdp.sdp);
                                    if let Err(e) = pc.set_remote_description(rtc_sdp).await {
                                        eprintln!("Failed to set remote description: {:?}", e);
                                        continue;
                                    }

                                    if !pending_remote_ice.is_empty() {
                                        println!(
                                            "Adding {} queued remote ICE candidates",
                                            pending_remote_ice.len()
                                        );
                                        for rtc_cand in pending_remote_ice.drain(..) {
                                            if let Err(e) = pc.add_ice_candidate(rtc_cand).await {
                                                eprintln!(
                                                    "Failed to add queued ICE candidate: {:?}",
                                                    e
                                                );
                                            }
                                        }
                                    }

                                    match pc.create_answer(None).await {
                                        Ok(answer) => {
                                            println!("SDP Answer created:\n{}", answer.sdp);
                                            if let Err(e) =
                                                pc.set_local_description(answer.clone()).await
                                            {
                                                eprintln!(
                                                    "Failed to set local description: {:?}",
                                                    e
                                                );
                                                continue;
                                            }

                                            let answer_msg =
                                                ClientMessage::Signaling(SignalingMessage::Sdp {
                                                    target_id: host_id.clone(),
                                                    sdp: RtcSessionDescription {
                                                        ty: RtcSdpType::Answer,
                                                        sdp: answer.sdp,
                                                    },
                                                    ice_servers: None,
                                                    webtransport_port: None,
                                                    webtransport_cert_hash: None,
                                                    webtransport_token: None,
                                                });
                                            if let Err(e) = outbox_tx.send(answer_msg) {
                                                eprintln!("Failed to queue SDP answer for websocket send: {:?}", e);
                                                continue;
                                            }
                                            println!("SDP Answer created and queued successfully.");
                                        }
                                        Err(e) => {
                                            eprintln!("Failed to create SDP answer: {:?}", e);
                                            continue;
                                        }
                                    }
                                }
                                Err(e) => {
                                    eprintln!("Failed to parse SDP offer: {:?}", e);
                                    continue;
                                }
                            }
                            let pc_clone = Arc::clone(&pc);
                            let pc_state = pc_state.clone();
                            tokio::spawn(async move {
                                loop {
                                    tokio::time::sleep(std::time::Duration::from_secs(1)).await;
                                    // Stop with the connection: holding `pc_clone` forever
                                    // would keep a replaced connection alive.
                                    if pc_state.closed() {
                                        break;
                                    }
                                    let stats = pc_clone
                                        .get_stats(std::time::Instant::now(), StatsSelector::None)
                                        .await;

                                    let mut conn_type = "P2P (Direct)".to_string();
                                    let mut dynamic_ping = 0.0;
                                    let mut selected_pair = None;

                                    for pair_stats in stats.candidate_pairs() {
                                        if pair_stats.nominated {
                                            selected_pair = Some(pair_stats);
                                            break;
                                        }
                                    }

                                    if let Some(pair_stats) = selected_pair {
                                        dynamic_ping = pair_stats.current_round_trip_time * 1000.0;

                                        let local_cand = stats.get(&pair_stats.local_candidate_id);
                                        let remote_cand = stats.get(&pair_stats.remote_candidate_id);

                                        let is_local_relay = if let Some(
                                            RTCStatsReportEntry::LocalCandidate(cand_stats),
                                        ) = local_cand
                                        {
                                            cand_stats.candidate_type == RTCIceCandidateType::Relay
                                        } else {
                                            false
                                        };

                                        let is_remote_relay = if let Some(
                                            RTCStatsReportEntry::RemoteCandidate(cand_stats),
                                        ) = remote_cand
                                        {
                                            cand_stats.candidate_type == RTCIceCandidateType::Relay
                                        } else {
                                            false
                                        };

                                        if is_local_relay || is_remote_relay {
                                            conn_type = "TURN Relay".to_string();
                                        }
                                    }

                                    if let Some(ref mut s) = *STREAM_STATS.lock().unwrap() {
                                        // To the agent and back, plus from the agent to the
                                        // computer it shows and back when it shows another.
                                        s.ping = crate::session_info::shown_ping_ms(
                                            dynamic_ping,
                                            SOURCE_LATENCY_US.load(std::sync::atomic::Ordering::Relaxed) as f64 / 1000.0,
                                        );
                                        s.connection_type = conn_type;
                                    }
                                }
                            });
                            peer_connection = Some(pc);
                        }
                    }
                    SignalingMessage::IceCandidate { candidate, .. } => {
                        let rtc_cand = RTCIceCandidateInit {
                            candidate: candidate.candidate,
                            sdp_mid: candidate.sdp_mid,
                            sdp_mline_index: candidate.sdp_mline_index,
                            username_fragment: candidate.username_fragment,
                            ..Default::default()
                        };

                        if let Some(ref pc) = peer_connection {
                            if let Err(e) = pc.add_ice_candidate(rtc_cand).await {
                                eprintln!("Failed to add remote ICE candidate: {:?}", e);
                            }
                        } else {
                            pending_remote_ice.push(rtc_cand);
                            println!(
                                "Queued remote ICE candidate before peer connection was initialized (pending={})",
                                pending_remote_ice.len()
                            );
                        }
                    }
                    SignalingMessage::EndSession { .. } => {
                        println!("Stream session ended by remote host.");
                        break;
                    }
                    SignalingMessage::SourceStatus { status, reason, .. } => {
                        // "failed": the source could not be connected, no
                        // session follows. "ended": it closed. "restart": it
                        // changed its codec and the same session is to be
                        // asked for again. This connection is over in all
                        // three; `poll_stats` does the rest.
                        println!("Source status from the host: {}", status);
                        *PENDING_SOURCE_STATUS.lock().unwrap() = Some((status, reason.unwrap_or_default()));
                        break;
                    }
                    SignalingMessage::AccessRequest { status, .. } => {
                        if let Some(refusal) = crate::session_info::access_refusal(&status) {
                            return Err(anyhow::anyhow!(refusal));
                        }
                        if status == "waiting" {
                            crate::share::set_status("Waiting for the person at the remote computer to accept…");
                        }
                    }
                    SignalingMessage::Error { message } if peer_connection.is_none() => {
                        // e.g. "No agent connected" or the agent could not
                        // start capture/encoding: nothing will stream.
                        return Err(anyhow::anyhow!(message));
                    }
                    _ => {}
                },
            }
        }
    }

    println!("Cleaning up WebRTC connection...");
    if let Some(pc) = peer_connection {
        let _ = pc.close().await;
    }
    Ok(())
}

#[cfg(test)]
mod source_tests {
    use super::{decode_codecs_for_request, source_restart_allowed};
    use crate::av1_rtp::av1_obu_with_size;
    use std::time::{Duration, Instant};

    #[test]
    fn three_restarts_in_a_row_then_no_more() {
        let start = Instant::now();
        let mut state = (0, None);
        for i in 0..3 {
            assert!(source_restart_allowed(&mut state, start + Duration::from_secs(i * 5)));
        }
        assert!(!source_restart_allowed(&mut state, start + Duration::from_secs(15)));
        // After a session that lasted, the count begins anew.
        assert!(source_restart_allowed(&mut state, start + Duration::from_secs(120)));
    }

    #[test]
    fn the_chosen_codec_comes_first() {
        let all = || vec!["h264".to_string(), "h265".to_string(), "av1".to_string()];
        assert_eq!(decode_codecs_for_request(all(), "auto"), ["h264", "h265", "av1"]);
        assert_eq!(decode_codecs_for_request(all(), "av1"), ["av1", "h264", "h265"]);
        assert_eq!(decode_codecs_for_request(all(), "hevc"), ["h265", "h264", "av1"]);
        // One this client has no decoder for is not added.
        assert_eq!(decode_codecs_for_request(vec!["h264".to_string()], "av1"), ["h264"]);
    }

    #[test]
    fn an_av1_obu_from_rtp_gets_its_size_field() {
        // OBU_FRAME (type 6), no extension, no size: header 0x30.
        let mut obu = vec![0x30];
        obu.extend(std::iter::repeat(7u8).take(200));
        let out = av1_obu_with_size(&obu).unwrap();
        assert_eq!(out[0], 0x32);
        assert_eq!(&out[1..3], &[0xC8, 0x01], "200 as LEB128");
        assert_eq!(&out[3..], &obu[1..]);
        // With an extension byte: the size comes after it.
        let out = av1_obu_with_size(&[0x34, 0x08, 1, 2, 3]).unwrap();
        assert_eq!(out, [0x36, 0x08, 3, 1, 2, 3]);
        // Has its size already: unchanged.
        assert_eq!(av1_obu_with_size(&[0x0A, 1, 9]).unwrap(), [0x0A, 1, 9]);
        // A sequence header alone, payload of one byte.
        assert_eq!(av1_obu_with_size(&[0x08, 5]).unwrap(), [0x0A, 1, 5]);
        assert!(av1_obu_with_size(&[]).is_none());
        assert!(av1_obu_with_size(&[0x80, 1]).is_none());
        assert!(av1_obu_with_size(&[0x34]).is_none());
    }
}

#[cfg(test)]
mod csc_request_tests {
    use super::compose_encoder_csc_mode;

    #[test]
    fn composes_overrides() {
        // Neither set: the host default applies.
        assert_eq!(compose_encoder_csc_mode(None, None), None);
        // Both set: (space << 1) | range.
        assert_eq!(compose_encoder_csc_mode(Some(2), Some(1)), Some(5));
        assert_eq!(compose_encoder_csc_mode(Some(0), Some(0)), Some(0));
        // One absent: the default's part (Rec. 709, limited).
        assert_eq!(compose_encoder_csc_mode(Some(0), None), Some(0));
        assert_eq!(compose_encoder_csc_mode(Some(2), None), Some(4));
        assert_eq!(compose_encoder_csc_mode(None, Some(1)), Some(3));
        assert_eq!(compose_encoder_csc_mode(None, Some(0)), Some(2));
    }
}

#[cfg(test)]
mod chroma_tests {
    use super::{auto_codec_for_host, is_h264_444_fmtp, split_chroma_codec};

    #[test]
    fn auto_codec_needs_a_hardware_hevc_encoder_for_the_gpu() {
        let list = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        let all = list(&["h264_nvenc", "hevc_nvenc", "h264_vaapi", "hevc_vaapi", "libx264", "libx265"]);
        assert_eq!(auto_codec_for_host(&all, Some("NVIDIA GeForce RTX 2070")), "h265");
        assert_eq!(auto_codec_for_host(&all, Some("Intel Iris Xe Graphics")), "h265");
        // nvenc compiled in but no NVIDIA GPU, no VAAPI HEVC: software only.
        assert_eq!(auto_codec_for_host(&list(&["hevc_nvenc", "libx265", "libx264"]), Some("Intel UHD 620")), "h264");
        assert_eq!(auto_codec_for_host(&all, None), "h264");
    }

    #[test]
    fn splits_444_choice() {
        assert_eq!(split_chroma_codec("h264_444"), ("h264".into(), Some("444".into())));
        assert_eq!(split_chroma_codec("H265"), ("h265".into(), None));
    }

    #[test]
    fn detects_high_444_profile() {
        assert!(is_h264_444_fmtp("level-asymmetry-allowed=1;packetization-mode=1;profile-level-id=f4002a"));
        assert!(!is_h264_444_fmtp("level-asymmetry-allowed=1;packetization-mode=1;profile-level-id=42002a"));
    }
}

#[cfg(test)]
mod host_cursor_tests {
    use super::{handle_host_cursor_message, pointer_always_in_picture, PENDING_HOST_CURSOR};

    /// The pending message is one place for the whole process: the tests
    /// that put one there and take it do so one at a time.
    static TAKING: std::sync::Mutex<()> = std::sync::Mutex::new(());

    /// `pointer_maybe_in_picture` is optional: hosts from before it, and
    /// hosts that know when their pointer is in the picture, do not send it.
    #[test]
    fn pointer_maybe_in_picture_is_optional() {
        let take = |json: &str| {
            let _one = TAKING.lock().unwrap_or_else(|e| e.into_inner());
            handle_host_cursor_message(json.as_bytes());
            PENDING_HOST_CURSOR.lock().unwrap().take().expect("a host_cursor message")
        };
        let cursor = take(r#"{"type":"host_cursor","x":10,"y":20,"visible":true,"kind":"arrow","in_window_move_size":true}"#);
        assert!(cursor.in_window_move_size);
        assert!(!cursor.pointer_maybe_in_picture);
        let cursor = take(r#"{"type":"host_cursor","x":10,"y":20,"pointer_maybe_in_picture":true}"#);
        assert!(cursor.pointer_maybe_in_picture);
        assert!(!cursor.in_window_move_size);
        let cursor = take(r#"{"type":"host_cursor","x":10,"y":20,"pointer_maybe_in_picture":false}"#);
        assert!(!cursor.pointer_maybe_in_picture);
    }

    /// `pointer_in_picture` is its own field: only "always" counts, and
    /// neither of the other two flags stands for it (nor it for them).
    #[test]
    fn pointer_always_in_picture_is_its_own_field() {
        let says = |json: &str| pointer_always_in_picture(&serde_json::from_str(json).unwrap());
        // As a Sunshine source's agent sends it.
        assert!(says(
            r#"{"type":"host_cursor","x":0,"y":0,"visible":false,"kind":"arrow","image":null,"in_window_move_size":false,"pointer_maybe_in_picture":false,"pointer_in_picture":"always"}"#
        ));
        // Hosts from before the field, hosts that send null, anything else.
        assert!(!says(r#"{"type":"host_cursor","x":10,"y":20}"#));
        assert!(!says(r#"{"type":"host_cursor","x":10,"y":20,"pointer_in_picture":null}"#));
        assert!(!says(r#"{"type":"host_cursor","x":10,"y":20,"pointer_in_picture":true}"#));
        assert!(!says(r#"{"type":"host_cursor","x":10,"y":20,"pointer_in_picture":"never"}"#));
        assert!(!says(r#"{"type":"host_cursor","x":10,"y":20,"in_window_move_size":true,"pointer_maybe_in_picture":true}"#));
    }

    /// The whole message, as the UI thread takes it.
    #[test]
    fn a_message_of_a_host_whose_pointer_is_always_in_its_picture() {
        // One lock for the two tests of this module that take the pending message.
        let take = |json: &str| {
            let _one = TAKING.lock().unwrap_or_else(|e| e.into_inner());
            handle_host_cursor_message(json.as_bytes());
            PENDING_HOST_CURSOR.lock().unwrap().take().expect("a host_cursor message")
        };
        let cursor = take(r#"{"type":"host_cursor","x":0,"y":0,"visible":false,"kind":"arrow","image":null,"in_window_move_size":false,"pointer_maybe_in_picture":false,"pointer_in_picture":"always"}"#);
        assert!(cursor.pointer_always_in_picture);
        assert!(!cursor.in_window_move_size && !cursor.pointer_maybe_in_picture && !cursor.visible);
        let cursor = take(r#"{"type":"host_cursor","x":10,"y":20,"visible":true,"kind":"arrow","in_window_move_size":true,"pointer_maybe_in_picture":true}"#);
        assert!(!cursor.pointer_always_in_picture);
        assert!(cursor.in_window_move_size && cursor.pointer_maybe_in_picture);
    }
}


#[cfg(test)]
mod security_fix_tests {
    use super::*;

    #[test]
    fn signaling_address_changes_only_the_scheme() {
        assert_eq!(signaling_ws_base("https://srv.example"), "wss://srv.example");
        assert_eq!(signaling_ws_base("http://srv.example:8080/"), "ws://srv.example:8080");
        // A host or path with "http" in it stays as it is.
        assert_eq!(signaling_ws_base("wss://httpd.example"), "wss://httpd.example");
        assert_eq!(signaling_ws_base("https://httpd.example/http-base"), "wss://httpd.example/http-base");
        assert_eq!(signaling_ws_base("http://myhttp.example"), "ws://myhttp.example");
        assert_eq!(signaling_ws_base("ws://[2001:db8::1]:8090"), "ws://[2001:db8::1]:8090");
    }

    #[test]
    fn webtransport_is_used_only_with_a_pinned_certificate() {
        let hash = "ab".repeat(32);
        assert_eq!(webtransport_pinned_digest(Some(&hash), None), Ok([0xab; 32]));
        assert_eq!(webtransport_pinned_digest(Some(&hash), Some("t")), Ok([0xab; 32]));
        // Older agents (no hash): no WebTransport, input stays on WebRTC.
        assert!(webtransport_pinned_digest(None, None).is_err());
        assert!(webtransport_pinned_digest(Some("zz"), None).is_err());
        assert!(webtransport_pinned_digest(None, Some("t")).is_err());
    }
}
