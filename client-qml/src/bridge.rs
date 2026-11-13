use bytes::Bytes;
use cxx_qt::CxxQtType;
use std::pin::Pin;
use std::sync::{Arc, Mutex};

#[derive(Debug, Clone)]
pub enum PendingDashboardEvent {
    LoginResult {
        success: bool,
        error_msg: String,
        token: String,
        username: String,
        server: String,
    },

    HostsResult {
        success: bool,
        error_msg: String,
        hosts_json: String,
    },

    AppsResult {
        success: bool,
        error_msg: String,
        host_id: String,
        apps_json: String,
    },
    CredentialsLoaded {
        success: bool,
        server: String,
        token: String,
        username: String,
    },
    AgentTokenResult {
        success: bool,
        error_msg: String,
        token: String,
    },
    DeepLinkReceived {
        url: String,
    },
    UpdateAvailable {
        latest_version: String,
        release_url: String,
    },
    /// Answer to `api_request`: HTTP status (0 = no connection) and body.
    ApiResult {
        req_id: String,
        status: i32,
        body: String,
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
    pub virtual_display: bool,
}

pub static APP_ARGS: std::sync::OnceLock<AppArgs> = std::sync::OnceLock::new();
/// Saved computers that answered the last check (`probe_peer`).
static PEER_ONLINE: std::sync::LazyLock<std::sync::Mutex<std::collections::HashMap<String, bool>>> =
    std::sync::LazyLock::new(Default::default);
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

#[derive(serde::Serialize, serde::Deserialize, Clone, Debug)]
struct SavedSettings {
    server_url: String,
    token: String,
    username: String,
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
    let _ = std::fs::create_dir_all(&path);
    path.push("client_config.json");
    path
}

fn load_settings() -> Option<SavedSettings> {
    let path = get_config_path();
    if !path.exists() {
        return None;
    }
    let data = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&data).ok()
}

fn save_settings(server_url: &str, token: &str, username: &str) {
    let path = get_config_path();
    let settings = SavedSettings {
        server_url: server_url.to_string(),
        token: token.to_string(),
        username: username.to_string(),
    };
    if let Ok(data) = serde_json::to_string_pretty(&settings) {
        let _ = std::fs::write(path, data);
    }
}

fn clear_settings() {
    let path = get_config_path();
    let _ = std::fs::remove_file(path);
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

/// The last direct connection (no server), remembered like the login.
#[derive(serde::Serialize, serde::Deserialize, Default)]
struct DirectTarget {
    address: String,
    password: String,
}

fn direct_target_path() -> std::path::PathBuf {
    get_config_path().with_file_name("direct_connection.json")
}

/// File of a JSON store the UI keeps (`load_store`/`save_store`).
fn store_path(name: &str) -> Option<std::path::PathBuf> {
    matches!(name, "app_settings" | "peers" | "view_prefs")
        .then(|| get_config_path().with_file_name(format!("{}.json", name)))
}

fn load_direct_target() -> Option<DirectTarget> {
    serde_json::from_str(&std::fs::read_to_string(direct_target_path()).ok()?).ok()
}

fn save_direct_target_file(target: &DirectTarget) {
    let path = direct_target_path();
    if let Ok(data) = serde_json::to_string_pretty(target) {
        if std::fs::write(&path, data).is_ok() {
            // Holds an access password: owner-only.
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
            }
        }
    }
}

/// Why the stream couldn't start, set by the stream task and reported to QML
/// by `poll_stats` (`stream_failed`), which leaves the stream view.
pub static STREAM_ERROR: std::sync::Mutex<Option<String>> = std::sync::Mutex::new(None);

/// A message for the user from a stream task error.
fn describe_stream_error(e: &anyhow::Error) -> String {
    use tokio_tungstenite::tungstenite::Error as WsError;
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

pub static LOCAL_AGENT_CHILD: std::sync::Mutex<Option<std::process::Child>> =
    std::sync::Mutex::new(None);

fn get_agent_config_path() -> std::path::PathBuf {
    let mut path = if let Ok(home) = std::env::var("HOME") {
        std::path::PathBuf::from(home)
    } else if let Ok(user_profile) = std::env::var("USERPROFILE") {
        std::path::PathBuf::from(user_profile)
    } else {
        std::path::PathBuf::from(".")
    };
    path.push(".lunaris");
    let _ = std::fs::create_dir_all(&path);
    path.push("agent_config.json");
    path
}

fn prepare_agent_config(server_url: &str, server_token: &str) -> std::path::PathBuf {
    let path = get_agent_config_path();
    let mut config_json = if path.exists() {
        if let Ok(content) = std::fs::read_to_string(&path) {
            serde_json::from_str::<serde_json::Value>(&content)
                .unwrap_or_else(|_| serde_json::json!({}))
        } else {
            serde_json::json!({})
        }
    } else {
        serde_json::json!({})
    };

    config_json["server_url"] = serde_json::json!(server_url);
    config_json["server_token"] = serde_json::json!(server_token);

    if let Ok(data) = serde_json::to_string_pretty(&config_json) {
        let _ = std::fs::write(&path, data);
    }
    path
}

#[allow(dead_code)]
fn get_autostart_path_linux() -> Option<std::path::PathBuf> {
    let home = std::env::var("HOME").ok()?;
    let mut path = std::path::PathBuf::from(home);
    path.push(".config");
    path.push("autostart");
    let _ = std::fs::create_dir_all(&path);
    path.push("lunaris-client.desktop");
    Some(path)
}

#[allow(dead_code)]
fn get_autostart_path_macos() -> Option<std::path::PathBuf> {
    let home = std::env::var("HOME").ok()?;
    let mut path = std::path::PathBuf::from(home);
    path.push("Library");
    path.push("LaunchAgents");
    let _ = std::fs::create_dir_all(&path);
    path.push("com.lunaris.client.plist");
    Some(path)
}

fn is_autostart_enabled_impl() -> bool {
    #[cfg(target_os = "linux")]
    {
        if let Some(path) = get_autostart_path_linux() {
            return path.exists();
        }
    }
    #[cfg(target_os = "macos")]
    {
        if let Some(path) = get_autostart_path_macos() {
            return path.exists();
        }
    }
    #[cfg(target_os = "windows")]
    {
        let output = std::process::Command::new("reg")
            .args(&[
                "query",
                "HKCU\\Software\\Microsoft\\Windows\\CurrentVersion\\Run",
                "/v",
                "LunarisClient",
            ])
            .output();
        if let Ok(out) = output {
            return out.status.success();
        }
    }
    false
}

fn set_autostart_enabled_impl(enabled: bool) {
    let exe_path = match std::env::current_exe() {
        Ok(path) => path.to_string_lossy().to_string(),
        Err(_) => return,
    };

    if enabled {
        #[cfg(target_os = "linux")]
        {
            if let Some(path) = get_autostart_path_linux() {
                let content = format!(
                    "[Desktop Entry]\nType=Application\nName=Lunaris Client\nExec=\"{}\" --minimized\nIcon=lunaris-client\nX-GNOME-Autostart-enabled=true\n",
                    exe_path
                );
                let _ = std::fs::write(path, content);
            }
        }
        #[cfg(target_os = "macos")]
        {
            if let Some(path) = get_autostart_path_macos() {
                let content = format!(
                    r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>com.lunaris.client</string>
    <key>ProgramArguments</key>
    <array>
        <string>{}</string>
        <string>--minimized</string>
    </array>
    <key>RunAtLoad</key>
    <true/>
</dict>
</plist>"#,
                    exe_path
                );
                let _ = std::fs::write(path, content);
            }
        }
        #[cfg(target_os = "windows")]
        {
            let val = format!("\"{}\" --minimized", exe_path);
            let _ = std::process::Command::new("reg")
                .args(&[
                    "add",
                    "HKCU\\Software\\Microsoft\\Windows\\CurrentVersion\\Run",
                    "/v",
                    "LunarisClient",
                    "/t",
                    "REG_SZ",
                    "/d",
                    &val,
                    "/f",
                ])
                .output();
        }
    } else {
        #[cfg(target_os = "linux")]
        {
            if let Some(path) = get_autostart_path_linux() {
                let _ = std::fs::remove_file(path);
            }
        }
        #[cfg(target_os = "macos")]
        {
            if let Some(path) = get_autostart_path_macos() {
                let _ = std::fs::remove_file(path);
            }
        }
        #[cfg(target_os = "windows")]
        {
            let _ = std::process::Command::new("reg")
                .args(&[
                    "delete",
                    "HKCU\\Software\\Microsoft\\Windows\\CurrentVersion\\Run",
                    "/v",
                    "LunarisClient",
                    "/f",
                ])
                .output();
        }
    }
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
        );

        #[qsignal]
        fn local_cursor_delta(self: Pin<&mut StreamBridge>, rx: i32, ry: i32);

        #[qsignal]
        fn new_version_available(
            self: Pin<&mut StreamBridge>,
            latest_version: QString,
            release_url: QString,
        );

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
        fn login_result(
            self: Pin<&mut StreamBridge>,
            success: bool,
            error_msg: QString,
            token: QString,
            username: QString,
            server: QString,
        );

        #[qsignal]
        fn hosts_result(
            self: Pin<&mut StreamBridge>,
            success: bool,
            error_msg: QString,
            hosts_json: QString,
        );

        #[qsignal]
        fn apps_result(
            self: Pin<&mut StreamBridge>,
            success: bool,
            error_msg: QString,
            host_id: QString,
            apps_json: QString,
        );

        #[qsignal]
        fn credentials_loaded(
            self: Pin<&mut StreamBridge>,
            success: bool,
            server: QString,
            token: QString,
            username: QString,
        );

        #[qsignal]
        fn agent_token_result(
            self: Pin<&mut StreamBridge>,
            success: bool,
            error_msg: QString,
            token: QString,
        );

        #[qsignal]
        fn deeplink_received(self: Pin<&mut StreamBridge>, url: QString);

        /// Answer to `api_request` (status 0 = the server could not be reached).
        #[qsignal]
        fn api_result(self: Pin<&mut StreamBridge>, req_id: QString, status: i32, body: QString);

        /// The stream could not start or ended with an error (message for the user).
        #[qsignal]
        fn stream_failed(self: Pin<&mut StreamBridge>, message: QString);

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

        #[qinvokable]
        fn send_mouse_wheel(self: Pin<&mut StreamBridge>, angle_x: i32, angle_y: i32);

        #[qinvokable]
        fn send_key_event(self: Pin<&mut StreamBridge>, key: i32, modifiers: i32, is_down: bool);

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

        #[qinvokable]
        fn load_saved_credentials(self: Pin<&mut StreamBridge>);

        #[qinvokable]
        fn login(
            self: Pin<&mut StreamBridge>,
            server: QString,
            username: QString,
            password: QString,
        );

        #[qinvokable]
        fn logout(self: Pin<&mut StreamBridge>);

        #[qinvokable]
        fn fetch_hosts(self: Pin<&mut StreamBridge>);

        /// Calls the signed-in server's REST API (`path` like "/api/admin/users")
        /// with the saved token; the answer comes back as `api_result(req_id, …)`.
        #[qinvokable]
        fn api_request(
            self: Pin<&mut StreamBridge>,
            req_id: QString,
            method: QString,
            path: QString,
            body: QString,
        );

        #[qinvokable]
        fn delete_host(self: Pin<&mut StreamBridge>, host_id: QString);

        #[qinvokable]
        fn fetch_apps(self: Pin<&mut StreamBridge>, host_id: QString);

        #[qinvokable]
        fn fetch_capabilities(self: Pin<&mut StreamBridge>, server: QString, token: QString, host_id: QString);

        #[qinvokable]
        fn configure_virtual_display(
            self: Pin<&mut StreamBridge>,
            server: QString,
            token: QString,
            host_id: QString,
            enable: bool,
            width: i32,
            height: i32,
            refresh_hz: i32,
        );

        #[qinvokable]
        fn start_game_session(
            self: Pin<&mut StreamBridge>,
            server: QString,
            token: QString,
            host_id: QString,
            host_name: QString,
            app_id: i32,
            res: QString,
            fps: i32,
            codec: QString,
            bitrate: i32,
            mouse_queue_limit: i32,
            disable_cuda: bool,
            input_protocol: QString,
            encoder: QString,
            display_id: QString,
            virtual_display: bool,
        );

        #[qinvokable]
        fn start_game_session_with_backend(
            self: Pin<&mut StreamBridge>,
            server: QString,
            token: QString,
            host_id: QString,
            host_name: QString,
            app_id: i32,
            res: QString,
            fps: i32,
            codec: QString,
            bitrate: i32,
            mouse_queue_limit: i32,
            disable_cuda: bool,
            render_backend: QString,
            input_protocol: QString,
            encoder: QString,
            display_id: QString,
            virtual_display: bool,
        );

        #[qinvokable]
        fn poll_events(self: Pin<&mut StreamBridge>);

        #[qinvokable]
        fn fetch_agent_token(self: Pin<&mut StreamBridge>, server: QString, token: QString);

        #[qinvokable]
        fn start_local_agent(
            self: Pin<&mut StreamBridge>,
            server: QString,
            token: QString,
            name: QString,
        );

        #[qinvokable]
        fn stop_local_agent(self: Pin<&mut StreamBridge>);

        #[qinvokable]
        fn is_local_agent_running(self: Pin<&mut StreamBridge>) -> bool;

        #[qinvokable]
        fn get_local_hostname(self: Pin<&mut StreamBridge>) -> QString;

        /// Remote control of this computer, as JSON: enabled, running, port,
        /// password, addresses, hostname, error.
        #[qinvokable]
        fn host_status(self: Pin<&mut StreamBridge>) -> QString;

        /// Turns remote control on; returns an error message or "".
        #[qinvokable]
        fn host_start(self: Pin<&mut StreamBridge>) -> QString;

        /// Turns remote control off; returns an error message or "".
        #[qinvokable]
        fn host_stop(self: Pin<&mut StreamBridge>) -> QString;

        /// Sets the access password ("" = a new random one); returns an
        /// error message or "".
        #[qinvokable]
        fn host_set_password(self: Pin<&mut StreamBridge>, password: QString) -> QString;

        /// Registers this computer with the signed-in server (`server`
        /// http(s)://host:port, `token` its agent token). Returns
        /// "registered", "already", "elsewhere <server>" or "error: ...".
        #[qinvokable]
        fn host_use_server(self: Pin<&mut StreamBridge>, server: QString, token: QString) -> QString;

        /// Whether clients may share the clipboard / send files to this
        /// computer; returns an error message or "".
        #[qinvokable]
        fn host_set_sharing(self: Pin<&mut StreamBridge>, clipboard: bool, files: bool) -> QString;

        /// How connections are accepted: "password", "click" or
        /// "password-click"; returns an error message or "".
        #[qinvokable]
        fn host_set_approve_mode(self: Pin<&mut StreamBridge>, mode: QString) -> QString;

        /// Connections waiting to be accepted here, as JSON:
        /// [{id, requester, kind, timeout_s}].
        #[qinvokable]
        fn host_approvals(self: Pin<&mut StreamBridge>) -> QString;

        #[qinvokable]
        fn host_answer_approval(self: Pin<&mut StreamBridge>, id: i32, accept: bool);

        /// Sets the port for direct connections; returns an error message or "".
        #[qinvokable]
        fn host_set_port(self: Pin<&mut StreamBridge>, port: i32) -> QString;

        /// The app's own Lunaris server, as JSON: enabled, running, ours,
        /// port, local_url, lan_url, admin_password, data_dir, error.
        #[qinvokable]
        fn server_status(self: Pin<&mut StreamBridge>) -> QString;

        /// Starts the app's server; returns an error message or "".
        #[qinvokable]
        fn server_start(self: Pin<&mut StreamBridge>) -> QString;

        /// Stops the app's server; returns an error message or "".
        #[qinvokable]
        fn server_stop(self: Pin<&mut StreamBridge>) -> QString;

        /// Sets the server's port; returns an error message or "".
        #[qinvokable]
        fn server_set_port(self: Pin<&mut StreamBridge>, port: i32) -> QString;

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

        /// File transfer on its own connection (files.rs) for the file
        /// manager page. Open for a computer, or for the one being streamed.
        #[qinvokable]
        fn files_open(self: Pin<&mut StreamBridge>, server_url: QString, token: QString, host_id: QString, host_name: QString);
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

        #[qinvokable]
        fn app_version(self: Pin<&mut StreamBridge>) -> QString;

        /// Last direct connection (address "host:port" and password), "" if none.
        #[qinvokable]
        fn saved_direct_address(self: Pin<&mut StreamBridge>) -> QString;

        #[qinvokable]
        fn saved_direct_password(self: Pin<&mut StreamBridge>) -> QString;

        #[qinvokable]
        fn save_direct_target(self: Pin<&mut StreamBridge>, address: QString, password: QString);

        /// Checks in the background whether a saved computer ("host:port")
        /// answers; `peer_online` gives the result.
        #[qinvokable]
        fn probe_peer(self: Pin<&mut StreamBridge>, address: QString);

        /// 1 reachable, 0 not, -1 not checked yet.
        #[qinvokable]
        fn peer_online(self: Pin<&mut StreamBridge>, address: QString) -> i32;

        #[qinvokable]
        fn is_autostart_enabled(self: Pin<&mut StreamBridge>) -> bool;

        #[qinvokable]
        fn set_autostart_enabled(self: Pin<&mut StreamBridge>, enabled: bool);

        #[qinvokable]
        fn should_start_minimized(self: Pin<&mut StreamBridge>) -> bool;

        #[qinvokable]
        fn check_for_updates(self: Pin<&mut StreamBridge>);
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
        let virtual_display = args.virtual_display;

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
                virtual_display,
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
            }
        }
    }

    pub fn send_mouse_click(self: Pin<&mut Self>, button: i32, is_down: bool) {
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

    pub fn load_saved_credentials(self: Pin<&mut Self>) {
        if let Some(settings) = load_settings() {
            PENDING_EVENTS
                .lock()
                .unwrap()
                .push(PendingDashboardEvent::CredentialsLoaded {
                    success: true,
                    server: settings.server_url,
                    token: settings.token,
                    username: settings.username,
                });
        } else {
            PENDING_EVENTS
                .lock()
                .unwrap()
                .push(PendingDashboardEvent::CredentialsLoaded {
                    success: false,
                    server: "".to_string(),
                    token: "".to_string(),
                    username: "".to_string(),
                });
        }
    }

    pub fn login(self: Pin<&mut Self>, server: QString, username: QString, password: QString) {
        let server_str = server.to_string();
        let username_str = username.to_string();
        let password_str = password.to_string();

        let mut rust_obj = self.rust_mut();
        let rt = rust_obj.get_or_init_runtime();

        rt.spawn(async move {
            let client = reqwest::Client::new();
            let url = format!("{}/api/auth/login", server_str);
            let res = client
                .post(&url)
                .json(&serde_json::json!({
                    "username": username_str,
                    "password": password_str
                }))
                .send()
                .await;

            match res {
                Ok(resp) => {
                    let status = resp.status();
                    let text = resp.text().await.unwrap_or_default();
                    if status.is_success() {
                        if let Ok(data) = serde_json::from_str::<common::AuthResponse>(&text) {
                            save_settings(&server_str, &data.token, &data.username);

                            PENDING_EVENTS.lock().unwrap().push(
                                PendingDashboardEvent::LoginResult {
                                    success: true,
                                    error_msg: "".to_string(),
                                    token: data.token,
                                    username: data.username,
                                    server: server_str,
                                },
                            );
                            return;
                        }
                    }

                    let err_msg =
                        if let Ok(err_data) = serde_json::from_str::<serde_json::Value>(&text) {
                            err_data
                                .get("error")
                                .and_then(|v| v.as_str())
                                .unwrap_or("Login failed")
                                .to_string()
                        } else {
                            "Login failed".to_string()
                        };

                    PENDING_EVENTS
                        .lock()
                        .unwrap()
                        .push(PendingDashboardEvent::LoginResult {
                            success: false,
                            error_msg: err_msg,
                            token: "".to_string(),
                            username: "".to_string(),
                            server: "".to_string(),
                        });
                }
                Err(e) => {
                    let err_msg = format!("Connection failed: {}", e);
                    PENDING_EVENTS
                        .lock()
                        .unwrap()
                        .push(PendingDashboardEvent::LoginResult {
                            success: false,
                            error_msg: err_msg,
                            token: "".to_string(),
                            username: "".to_string(),
                            server: "".to_string(),
                        });
                }
            }
        });
    }

    pub fn logout(self: Pin<&mut Self>) {
        clear_settings();
    }

    pub fn fetch_hosts(self: Pin<&mut Self>) {
        let settings = match load_settings() {
            Some(s) => s,
            None => {
                PENDING_EVENTS
                    .lock()
                    .unwrap()
                    .push(PendingDashboardEvent::HostsResult {
                        success: false,
                        error_msg: "Not authenticated".to_string(),
                        hosts_json: "".to_string(),
                    });
                return;
            }
        };

        let server_str = settings.server_url;
        let token_str = settings.token;

        let mut rust_obj = self.rust_mut();
        let rt = rust_obj.get_or_init_runtime();

        rt.spawn(async move {
            let client = reqwest::Client::new();
            let url = format!("{}/api/hosts", server_str);
            let res = client
                .get(&url)
                .header("Authorization", format!("Bearer {}", token_str))
                .send()
                .await;

            match res {
                Ok(resp) => {
                    if resp.status().is_success() {
                        if let Ok(hosts) = resp.json::<Vec<common::HostInfo>>().await {
                            if let Ok(hosts_json) = serde_json::to_string(&hosts) {
                                PENDING_EVENTS.lock().unwrap().push(
                                    PendingDashboardEvent::HostsResult {
                                        success: true,
                                        error_msg: "".to_string(),
                                        hosts_json,
                                    },
                                );
                                return;
                            }
                        }
                    } else if resp.status() == 401 {
                        PENDING_EVENTS
                            .lock()
                            .unwrap()
                            .push(PendingDashboardEvent::HostsResult {
                                success: false,
                                error_msg: "Unauthorized".to_string(),
                                hosts_json: "".to_string(),
                            });
                        return;
                    }
                    PENDING_EVENTS
                        .lock()
                        .unwrap()
                        .push(PendingDashboardEvent::HostsResult {
                            success: false,
                            error_msg: "Failed to fetch host list".to_string(),
                            hosts_json: "".to_string(),
                        });
                }
                Err(e) => {
                    let err_msg = format!("Connection failed: {}", e);
                    PENDING_EVENTS
                        .lock()
                        .unwrap()
                        .push(PendingDashboardEvent::HostsResult {
                            success: false,
                            error_msg: err_msg,
                            hosts_json: "".to_string(),
                        });
                }
            }
        });
    }

    pub fn api_request(
        self: Pin<&mut Self>,
        req_id: QString,
        method: QString,
        path: QString,
        body: QString,
    ) {
        let req_id = req_id.to_string();
        let push = |req_id: String, status: i32, body: String| {
            PENDING_EVENTS
                .lock()
                .unwrap()
                .push(PendingDashboardEvent::ApiResult {
                    req_id,
                    status,
                    body,
                });
        };
        let Some(settings) = load_settings() else {
            push(req_id, 401, r#"{"error":"Not signed in"}"#.to_string());
            return;
        };
        let path = path.to_string();
        if !path.starts_with("/api/") {
            push(req_id, 400, r#"{"error":"Bad path"}"#.to_string());
            return;
        }
        let method = match method.to_string().to_ascii_uppercase().as_str() {
            "GET" => reqwest::Method::GET,
            "POST" => reqwest::Method::POST,
            "PUT" => reqwest::Method::PUT,
            "DELETE" => reqwest::Method::DELETE,
            _ => {
                push(req_id, 400, r#"{"error":"Bad method"}"#.to_string());
                return;
            }
        };
        let body = body.to_string();
        let url = format!("{}{}", settings.server_url.trim_end_matches('/'), path);
        let token = settings.token;

        let mut rust_obj = self.rust_mut();
        let rt = rust_obj.get_or_init_runtime();
        rt.spawn(async move {
            let client = reqwest::Client::new();
            let mut req = client
                .request(method, &url)
                .header("Authorization", format!("Bearer {}", token))
                .timeout(std::time::Duration::from_secs(30));
            if !body.is_empty() {
                req = req.header("Content-Type", "application/json").body(body);
            }
            match req.send().await {
                Ok(resp) => {
                    let status = resp.status().as_u16() as i32;
                    let text = resp.text().await.unwrap_or_default();
                    push(req_id, status, text);
                }
                Err(e) => push(
                    req_id,
                    0,
                    serde_json::json!({ "error": format!("Connection failed: {}", e) })
                        .to_string(),
                ),
            }
        });
    }

    pub fn delete_host(self: Pin<&mut Self>, host_id: QString) {
        let host_id_str = host_id.to_string();
        let settings = match load_settings() {
            Some(s) => s,
            None => {
                PENDING_EVENTS
                    .lock()
                    .unwrap()
                    .push(PendingDashboardEvent::HostsResult {
                        success: false,
                        error_msg: "Not authenticated".to_string(),
                        hosts_json: "".to_string(),
                    });
                return;
            }
        };

        let server_str = settings.server_url;
        let token_str = settings.token;

        let mut rust_obj = self.rust_mut();
        let rt = rust_obj.get_or_init_runtime();

        rt.spawn(async move {
            let client = reqwest::Client::new();
            let delete_url = format!("{}/api/hosts/{}", server_str, host_id_str);
            let delete_res = client
                .delete(&delete_url)
                .header("Authorization", format!("Bearer {}", token_str))
                .send()
                .await;

            match delete_res {
                Ok(resp) if resp.status().is_success() => {
                    let hosts_url = format!("{}/api/hosts", server_str);
                    match client
                        .get(&hosts_url)
                        .header("Authorization", format!("Bearer {}", token_str))
                        .send()
                        .await
                    {
                        Ok(hosts_resp) if hosts_resp.status().is_success() => {
                            if let Ok(hosts) = hosts_resp.json::<Vec<common::HostInfo>>().await {
                                if let Ok(hosts_json) = serde_json::to_string(&hosts) {
                                    PENDING_EVENTS.lock().unwrap().push(
                                        PendingDashboardEvent::HostsResult {
                                            success: true,
                                            error_msg: "".to_string(),
                                            hosts_json,
                                        },
                                    );
                                    return;
                                }
                            }
                            PENDING_EVENTS.lock().unwrap().push(
                                PendingDashboardEvent::HostsResult {
                                    success: false,
                                    error_msg: "Host deleted, but failed to refresh host list"
                                        .to_string(),
                                    hosts_json: "".to_string(),
                                },
                            );
                        }
                        Ok(hosts_resp) if hosts_resp.status() == 401 => {
                            PENDING_EVENTS.lock().unwrap().push(
                                PendingDashboardEvent::HostsResult {
                                    success: false,
                                    error_msg: "Unauthorized".to_string(),
                                    hosts_json: "".to_string(),
                                },
                            );
                        }
                        Ok(_) => {
                            PENDING_EVENTS.lock().unwrap().push(
                                PendingDashboardEvent::HostsResult {
                                    success: false,
                                    error_msg: "Host deleted, but failed to refresh host list"
                                        .to_string(),
                                    hosts_json: "".to_string(),
                                },
                            );
                        }
                        Err(e) => {
                            PENDING_EVENTS.lock().unwrap().push(
                                PendingDashboardEvent::HostsResult {
                                    success: false,
                                    error_msg: format!("Host deleted, refresh failed: {}", e),
                                    hosts_json: "".to_string(),
                                },
                            );
                        }
                    }
                }
                Ok(resp) if resp.status() == 401 => {
                    PENDING_EVENTS
                        .lock()
                        .unwrap()
                        .push(PendingDashboardEvent::HostsResult {
                            success: false,
                            error_msg: "Unauthorized".to_string(),
                            hosts_json: "".to_string(),
                        });
                }
                Ok(_) => {
                    PENDING_EVENTS
                        .lock()
                        .unwrap()
                        .push(PendingDashboardEvent::HostsResult {
                            success: false,
                            error_msg: "Failed to delete host".to_string(),
                            hosts_json: "".to_string(),
                        });
                }
                Err(e) => {
                    PENDING_EVENTS
                        .lock()
                        .unwrap()
                        .push(PendingDashboardEvent::HostsResult {
                            success: false,
                            error_msg: format!("Connection failed: {}", e),
                            hosts_json: "".to_string(),
                        });
                }
            }
        });
    }

    pub fn fetch_apps(self: Pin<&mut Self>, host_id: QString) {
        let host_id_str = host_id.to_string();

        let settings = match load_settings() {
            Some(s) => s,
            None => {
                PENDING_EVENTS
                    .lock()
                    .unwrap()
                    .push(PendingDashboardEvent::AppsResult {
                        success: false,
                        error_msg: "Not authenticated".to_string(),
                        host_id: host_id_str,
                        apps_json: "".to_string(),
                    });
                return;
            }
        };

        let server_str = settings.server_url;
        let token_str = settings.token;

        let mut rust_obj = self.rust_mut();
        let rt = rust_obj.get_or_init_runtime();

        rt.spawn(async move {
            let encoded_token: String =
                url::form_urlencoded::byte_serialize(token_str.as_bytes()).collect();
            let ws_url = format!(
                "{}/ws/client?token={}",
                server_str.replace("http", "ws"),
                encoded_token
            );
            // Never log ws_url itself: it carries the session token.
            println!("fetch_apps connecting to ws at: {}/ws/client", server_str.replace("http", "ws"));

            let connect_result = connect_async(&ws_url).await;
            let (mut ws_stream, _) = match connect_result {
                Ok(c) => c,
                Err(e) => {
                    let err_msg = format!("WebSocket connection failed: {}", e);
                    PENDING_EVENTS
                        .lock()
                        .unwrap()
                        .push(PendingDashboardEvent::AppsResult {
                            success: false,
                            error_msg: err_msg,
                            host_id: host_id_str,
                            apps_json: "".to_string(),
                        });
                    return;
                }
            };

            let get_app_msg = ClientMessage::Signaling(SignalingMessage::GetAppList {
                target_id: host_id_str.clone(),
            });

            if let Ok(text) = serde_json::to_string(&get_app_msg) {
                if let Err(e) = ws_stream.send(WsMessage::Text(text.into())).await {
                    let err_msg = format!("Failed to send GetAppList request: {}", e);
                    PENDING_EVENTS
                        .lock()
                        .unwrap()
                        .push(PendingDashboardEvent::AppsResult {
                            success: false,
                            error_msg: err_msg,
                            host_id: host_id_str,
                            apps_json: "".to_string(),
                        });
                    return;
                }
            }

            let result = tokio::time::timeout(std::time::Duration::from_secs(5), async {
                while let Some(msg_result) = ws_stream.next().await {
                    match msg_result {
                        Ok(WsMessage::Text(text)) => {
                            if let Ok(server_msg) =
                                serde_json::from_str::<common::ServerToClientMessage>(&text)
                            {
                                match server_msg {
                                    common::ServerToClientMessage::Signaling(sig) => match sig {
                                        SignalingMessage::AppListResponse { apps, .. } => {
                                            return Ok(apps);
                                        }
                                        SignalingMessage::Error { message } => {
                                            return Err(message);
                                        }
                                        _ => {}
                                    },
                                }
                            }
                        }
                        Ok(_) => {}
                        Err(e) => {
                            return Err(format!("WebSocket error: {}", e));
                        }
                    }
                }
                Err("Connection closed before response received".to_string())
            })
            .await;

            let _ = ws_stream.close(None).await;

            match result {
                Ok(Ok(apps)) => {
                    if let Ok(apps_json) = serde_json::to_string(&apps) {
                        PENDING_EVENTS
                            .lock()
                            .unwrap()
                            .push(PendingDashboardEvent::AppsResult {
                                success: true,
                                error_msg: "".to_string(),
                                host_id: host_id_str,
                                apps_json,
                            });
                    } else {
                        PENDING_EVENTS
                            .lock()
                            .unwrap()
                            .push(PendingDashboardEvent::AppsResult {
                                success: false,
                                error_msg: "Failed to serialize apps".to_string(),
                                host_id: host_id_str,
                                apps_json: "".to_string(),
                            });
                    }
                }
                Ok(Err(e)) => {
                    PENDING_EVENTS
                        .lock()
                        .unwrap()
                        .push(PendingDashboardEvent::AppsResult {
                            success: false,
                            error_msg: e,
                            host_id: host_id_str,
                            apps_json: "".to_string(),
                        });
                }
                Err(_) => {
                    PENDING_EVENTS
                        .lock()
                        .unwrap()
                        .push(PendingDashboardEvent::AppsResult {
                            success: false,
                            error_msg: "Timed out waiting for application list".to_string(),
                            host_id: host_id_str,
                            apps_json: "".to_string(),
                        });
                }
            }
        });
    }

    pub fn fetch_capabilities(self: Pin<&mut Self>, server: QString, token: QString, host_id: QString) {
        let server_str = server.to_string();
        let token_str = token.to_string();
        let host_id_str = host_id.to_string();

        let mut rust_obj = self.rust_mut();
        let rt = rust_obj.get_or_init_runtime();

        rt.spawn(async move {
            let encoded_token: String =
                url::form_urlencoded::byte_serialize(token_str.as_bytes()).collect();
            let ws_url = format!(
                "{}/ws/client?token={}",
                server_str.replace("http", "ws"),
                encoded_token
            );

            let connect_result = connect_async(&ws_url).await;
            let (mut ws_stream, _) = match connect_result {
                Ok(c) => c,
                Err(e) => {
                    eprintln!("fetch_capabilities: WebSocket connection failed: {}", e);
                    return;
                }
            };

            let get_caps_msg = ClientMessage::Signaling(SignalingMessage::GetCapabilities {
                target_id: host_id_str.clone(),
            });

            if let Ok(text) = serde_json::to_string(&get_caps_msg) {
                if let Err(e) = ws_stream.send(WsMessage::Text(text.into())).await {
                    eprintln!("fetch_capabilities: Failed to send GetCapabilities: {}", e);
                    return;
                }
            }

            let result = tokio::time::timeout(std::time::Duration::from_secs(5), async {
                while let Some(msg_result) = ws_stream.next().await {
                    match msg_result {
                        Ok(WsMessage::Text(text)) => {
                            if let Ok(server_msg) =
                                serde_json::from_str::<common::ServerToClientMessage>(&text)
                            {
                                match server_msg {
                                    common::ServerToClientMessage::Signaling(sig) => match sig {
                                        SignalingMessage::CapabilitiesResponse {
                                            displays,
                                            encoders,
                                            gpu_info,
                                            host_os,
                                            ..
                                        } => {
                                            return Ok((displays, encoders, gpu_info, host_os));
                                        }
                                        SignalingMessage::Error { message } => {
                                            return Err(message);
                                        }
                                        _ => {}
                                    },
                                }
                            }
                        }
                        Ok(_) => {}
                        Err(e) => {
                            return Err(format!("WebSocket error: {}", e));
                        }
                    }
                }
                Err("Connection closed before response received".to_string())
            })
            .await;

            let _ = ws_stream.close(None).await;

            match result {
                Ok(Ok((displays, encoders, gpu_info, host_os))) => {
                    let displays_json =
                        serde_json::to_string(&displays).unwrap_or_else(|_| "[]".to_string());
                    let encoders_json =
                        serde_json::to_string(&encoders).unwrap_or_else(|_| "[]".to_string());
                    *PENDING_CAPABILITIES.lock().unwrap() = Some(PendingCapabilities {
                        displays_json,
                        encoders_json,
                        gpu_info: gpu_info.unwrap_or_default(),
                        host_os: host_os.unwrap_or_default(),
                    });
                }
                Ok(Err(e)) => {
                    eprintln!("fetch_capabilities error: {}", e);
                }
                Err(_) => {
                    eprintln!("fetch_capabilities: Timed out waiting for CapabilitiesResponse");
                }
            }
        });
    }

    pub fn configure_virtual_display(
        self: Pin<&mut Self>,
        server: QString,
        token: QString,
        host_id: QString,
        enable: bool,
        width: i32,
        height: i32,
        refresh_hz: i32,
    ) {
        let server_str = server.to_string();
        let token_str = token.to_string();
        let host_id_str = host_id.to_string();
        let width = width.max(0) as u32;
        let height = height.max(0) as u32;
        let refresh_hz = refresh_hz.max(0) as u32;

        let mut rust_obj = self.rust_mut();
        let rt = rust_obj.get_or_init_runtime();

        rt.spawn(async move {
            let encoded_token: String =
                url::form_urlencoded::byte_serialize(token_str.as_bytes()).collect();
            let ws_url = format!(
                "{}/ws/client?token={}",
                server_str.replace("http", "ws"),
                encoded_token
            );

            let (mut ws_stream, _) = match connect_async(&ws_url).await {
                Ok(c) => c,
                Err(e) => {
                    eprintln!("configure_virtual_display: WebSocket connection failed: {}", e);
                    return;
                }
            };

            // 1. Send the configure command.
            let cfg_msg = ClientMessage::Signaling(SignalingMessage::ConfigureVirtualDisplay {
                target_id: host_id_str.clone(),
                enable,
                width,
                height,
                refresh_hz,
            });
            if let Ok(text) = serde_json::to_string(&cfg_msg) {
                if let Err(e) = ws_stream.send(WsMessage::Text(text.into())).await {
                    eprintln!("configure_virtual_display: send failed: {}", e);
                    return;
                }
            }

            // 2. Wait for the result.
            let outcome = tokio::time::timeout(std::time::Duration::from_secs(8), async {
                while let Some(msg_result) = ws_stream.next().await {
                    if let Ok(WsMessage::Text(text)) = msg_result {
                        if let Ok(common::ServerToClientMessage::Signaling(sig)) =
                            serde_json::from_str::<common::ServerToClientMessage>(&text)
                        {
                            match sig {
                                SignalingMessage::VirtualDisplayConfigured {
                                    success,
                                    message,
                                    displays,
                                    ..
                                } => return Ok((success, message, displays)),
                                SignalingMessage::Error { message } => return Err(message),
                                _ => {}
                            }
                        }
                    }
                }
                Err("connection closed before VirtualDisplayConfigured".to_string())
            })
            .await;

            match outcome {
                Ok(Ok((success, message, displays))) => {
                    eprintln!("configure_virtual_display: success={} msg={}", success, message);
                    // Encoders don't change when toggling a display; re-query full
                    // capabilities so the encoder list isn't cleared in the UI.
                    let mut encoders_json = "[]".to_string();
                    let mut gpu_info = String::new();
                    let mut host_os = String::new();
                    let get_caps = ClientMessage::Signaling(SignalingMessage::GetCapabilities {
                        target_id: host_id_str.clone(),
                    });
                    if let Ok(text) = serde_json::to_string(&get_caps) {
                        if ws_stream.send(WsMessage::Text(text.into())).await.is_ok() {
                            let caps = tokio::time::timeout(
                                std::time::Duration::from_secs(5),
                                async {
                                    while let Some(m) = ws_stream.next().await {
                                        if let Ok(WsMessage::Text(t)) = m {
                                            if let Ok(common::ServerToClientMessage::Signaling(
                                                SignalingMessage::CapabilitiesResponse {
                                                    encoders,
                                                    gpu_info,
                                                    host_os,
                                                    ..
                                                },
                                            )) = serde_json::from_str::<common::ServerToClientMessage>(&t)
                                            {
                                                return Some((encoders, gpu_info, host_os));
                                            }
                                        }
                                    }
                                    None
                                },
                            )
                            .await
                            .ok()
                            .flatten();
                            if let Some((encoders, gi, ho)) = caps {
                                encoders_json = serde_json::to_string(&encoders)
                                    .unwrap_or_else(|_| "[]".to_string());
                                gpu_info = gi.unwrap_or_default();
                                host_os = ho.unwrap_or_default();
                            }
                        }
                    }
                    let displays_json =
                        serde_json::to_string(&displays).unwrap_or_else(|_| "[]".to_string());
                    *PENDING_CAPABILITIES.lock().unwrap() = Some(PendingCapabilities {
                        displays_json,
                        encoders_json,
                        gpu_info,
                        host_os,
                    });
                }
                Ok(Err(e)) => eprintln!("configure_virtual_display error: {}", e),
                Err(_) => eprintln!("configure_virtual_display: timed out"),
            }

            let _ = ws_stream.close(None).await;
        });
    }

    pub fn start_game_session(
        self: Pin<&mut Self>,
        server: QString,
        token: QString,
        host_id: QString,
        host_name: QString,
        app_id: i32,
        res: QString,
        fps: i32,
        codec: QString,
        bitrate: i32,
        mouse_queue_limit: i32,
        disable_cuda: bool,
        input_protocol: QString,
        encoder: QString,
        display_id: QString,
        virtual_display: bool,
    ) {
        let render_backend =
            cxx_qt_lib::QString::from(&render_backend_from_disable_cuda(disable_cuda));
        self.start_game_session_with_backend(
            server,
            token,
            host_id,
            host_name,
            app_id,
            res,
            fps,
            codec,
            bitrate,
            mouse_queue_limit,
            disable_cuda,
            render_backend,
            input_protocol,
            encoder,
            display_id,
            virtual_display,
        );
    }

    pub fn start_game_session_with_backend(
        mut self: Pin<&mut Self>,
        server: QString,
        token: QString,
        host_id: QString,
        host_name: QString,
        app_id: i32,
        res: QString,
        fps: i32,
        codec: QString,
        bitrate: i32,
        mouse_queue_limit: i32,
        disable_cuda: bool,
        render_backend: QString,
        input_protocol: QString,
        encoder: QString,
        display_id: QString,
        virtual_display: bool,
    ) {
        let server_str = server.to_string();
        let token_str = token.to_string();
        let host_id_str = host_id.to_string();
        let host_name_str = host_name.to_string();
        let res_str = res.to_string();
        let codec_str = codec.to_string().to_lowercase();
        let input_proto_str = input_protocol.to_string().to_lowercase();
        let render_backend_str =
            normalize_render_backend(&render_backend.to_string(), disable_cuda);
        let effective_disable_cuda = render_backend_disables_cuda(&render_backend_str);
        let encoder_str = encoder.to_string().to_lowercase();
        let display_str = display_id.to_string();

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
        }

        let app_id_opt = if app_id < 0 {
            None
        } else {
            Some(app_id as u32)
        };

        let args = AppArgs {
            host_id: host_id_str,
            server_url: server_str,
            token: token_str,
            width,
            height,
            fps: fps as u32,
            bitrate: bitrate as u32,
            codec: codec_str,
            app_id: app_id_opt,
            mouse_queue_limit: mouse_queue_limit as u32,
            host_name: host_name_str,
            disable_cuda: effective_disable_cuda,
            render_backend: render_backend_str,
            input_protocol: input_proto_str,
            encoder: if encoder_str.is_empty() || encoder_str == "auto" {
                None
            } else {
                Some(encoder_str)
            },
            display_id: if display_str.is_empty() || display_str == "default" {
                None
            } else {
                Some(display_str)
            },
            virtual_display,
        };

        {
            let mut active_config_lock = ACTIVE_CONFIG.lock().unwrap();
            *active_config_lock = Some(args);
        }

        println!("Configuring session from dashboard. Launching stream...");
        self.as_mut().start_stream();
    }

    pub fn poll_events(mut self: Pin<&mut Self>) {
        let events = {
            let mut lock = PENDING_EVENTS.lock().unwrap();
            std::mem::take(&mut *lock)
        };
        for event in events {
            match event {
                PendingDashboardEvent::LoginResult {
                    success,
                    error_msg,
                    token,
                    username,
                    server,
                } => {
                    let err_qstr = QString::from(&error_msg);
                    let tok_qstr = QString::from(&token);
                    let user_qstr = QString::from(&username);
                    let srv_qstr = QString::from(&server);
                    self.as_mut()
                        .login_result(success, err_qstr, tok_qstr, user_qstr, srv_qstr);
                }

                PendingDashboardEvent::HostsResult {
                    success,
                    error_msg,
                    hosts_json,
                } => {
                    let err_qstr = QString::from(&error_msg);
                    let hosts_qstr = QString::from(&hosts_json);
                    self.as_mut().hosts_result(success, err_qstr, hosts_qstr);
                }

                PendingDashboardEvent::AppsResult {
                    success,
                    error_msg,
                    host_id,
                    apps_json,
                } => {
                    let err_qstr = QString::from(&error_msg);
                    let host_qstr = QString::from(&host_id);
                    let apps_qstr = QString::from(&apps_json);
                    self.as_mut()
                        .apps_result(success, err_qstr, host_qstr, apps_qstr);
                }
                PendingDashboardEvent::CredentialsLoaded {
                    success,
                    server,
                    token,
                    username,
                } => {
                    let srv_qstr = QString::from(&server);
                    let tok_qstr = QString::from(&token);
                    let user_qstr = QString::from(&username);
                    self.as_mut()
                        .credentials_loaded(success, srv_qstr, tok_qstr, user_qstr);
                }
                PendingDashboardEvent::AgentTokenResult {
                    success,
                    error_msg,
                    token,
                } => {
                    let err_qstr = QString::from(&error_msg);
                    let tok_qstr = QString::from(&token);
                    self.as_mut()
                        .agent_token_result(success, err_qstr, tok_qstr);
                }
                PendingDashboardEvent::ApiResult {
                    req_id,
                    status,
                    body,
                } => {
                    self.as_mut().api_result(
                        QString::from(&req_id),
                        status,
                        QString::from(&body),
                    );
                }
                PendingDashboardEvent::DeepLinkReceived { url } => {
                    let url_qstr = QString::from(&url);
                    self.as_mut().deeplink_received(url_qstr);
                }
                PendingDashboardEvent::UpdateAvailable {
                    latest_version,
                    release_url,
                } => {
                    let ver_qstr = QString::from(&latest_version);
                    let url_qstr = QString::from(&release_url);
                    self.as_mut().new_version_available(ver_qstr, url_qstr);
                }
            }
        }
    }

    pub fn fetch_agent_token(self: Pin<&mut Self>, server: QString, token: QString) {
        let server_str = server.to_string();
        let token_str = token.to_string();

        let mut rust_obj = self.rust_mut();
        let rt = rust_obj.get_or_init_runtime();

        rt.spawn(async move {
            let client = reqwest::Client::new();
            let url = format!("{}/api/agent/token", server_str.trim_end_matches('/'));
            let res = client
                .get(&url)
                .header("Authorization", format!("Bearer {}", token_str))
                .send()
                .await;

            match res {
                Ok(resp) => {
                    let status = resp.status();
                    let text = resp.text().await.unwrap_or_default();
                    if status.is_success() {
                        if let Ok(data) = serde_json::from_str::<serde_json::Value>(&text) {
                            if let Some(agent_tok) = data.get("token").and_then(|t| t.as_str()) {
                                PENDING_EVENTS.lock().unwrap().push(
                                    PendingDashboardEvent::AgentTokenResult {
                                        success: true,
                                        error_msg: "".to_string(),
                                        token: agent_tok.to_string(),
                                    },
                                );
                                return;
                            }
                        }
                    }

                    let err_msg =
                        if let Ok(err_data) = serde_json::from_str::<serde_json::Value>(&text) {
                            err_data
                                .get("error")
                                .and_then(|v| v.as_str())
                                .unwrap_or("Failed to fetch agent token")
                                .to_string()
                        } else {
                            "Failed to fetch agent token".to_string()
                        };

                    PENDING_EVENTS
                        .lock()
                        .unwrap()
                        .push(PendingDashboardEvent::AgentTokenResult {
                            success: false,
                            error_msg: err_msg,
                            token: "".to_string(),
                        });
                }
                Err(e) => {
                    let err_msg = format!("Connection failed: {}", e);
                    PENDING_EVENTS
                        .lock()
                        .unwrap()
                        .push(PendingDashboardEvent::AgentTokenResult {
                            success: false,
                            error_msg: err_msg,
                            token: "".to_string(),
                        });
                }
            }
        });
    }

    pub fn start_local_agent(self: Pin<&mut Self>, server: QString, token: QString, name: QString) {
        let server_str = server.to_string();
        let token_str = token.to_string();
        let name_str = name.to_string();

        // 1. Prepare agent config json
        let config_path = prepare_agent_config(&server_str, &token_str);

        // 2. Locate agent executable
        let exe_dir = std::env::current_exe().ok().and_then(|mut path| {
            path.pop();
            Some(path)
        });

        if let Some(mut path) = exe_dir {
            #[cfg(target_os = "windows")]
            let agent_name = "agent.exe";
            #[cfg(not(target_os = "windows"))]
            let agent_name = "agent";
            path.push(agent_name);

            if path.exists() {
                let config_path_str = config_path.to_string_lossy().to_string();

                // Stop any running local agent first
                stop_local_agent();

                // Spawn subprocess
                #[cfg(target_os = "windows")]
                use std::os::windows::process::CommandExt;

                let mut cmd = std::process::Command::new(path);
                cmd.arg("--config")
                    .arg(config_path_str)
                    .arg("--name")
                    .arg(&name_str)
                    .arg("--server")
                    .arg(&server_str)
                    .arg("--cli")
                    .stdout(std::process::Stdio::null())
                    .stderr(std::process::Stdio::null());

                #[cfg(target_os = "windows")]
                cmd.creation_flags(0x08000000); // CREATE_NO_WINDOW

                let child = cmd.spawn();

                match child {
                    Ok(c) => {
                        let mut lock = LOCAL_AGENT_CHILD.lock().unwrap();
                        *lock = Some(c);
                        println!("Spawned local agent process successfully.");
                    }
                    Err(e) => {
                        eprintln!("Failed to spawn local agent: {:?}", e);
                    }
                }
            } else {
                eprintln!("Local agent executable not found at: {:?}", path);
            }
        }
    }

    pub fn stop_local_agent(self: Pin<&mut Self>) {
        stop_local_agent();
    }

    pub fn is_local_agent_running(self: Pin<&mut Self>) -> bool {
        let mut lock = LOCAL_AGENT_CHILD.lock().unwrap();
        if let Some(child) = lock.as_mut() {
            match child.try_wait() {
                Ok(None) => true,
                _ => {
                    *lock = None;
                    false
                }
            }
        } else {
            false
        }
    }

    pub fn host_status(self: Pin<&mut Self>) -> QString {
        QString::from(&crate::host::status_json())
    }

    pub fn host_start(self: Pin<&mut Self>) -> QString {
        QString::from(&crate::host::start().err().unwrap_or_default())
    }

    pub fn host_stop(self: Pin<&mut Self>) -> QString {
        QString::from(&crate::host::stop().err().unwrap_or_default())
    }

    pub fn host_set_password(self: Pin<&mut Self>, password: QString) -> QString {
        let password = password.to_string();
        let password = (!password.trim().is_empty()).then_some(password);
        QString::from(&crate::host::set_password(password).err().unwrap_or_default())
    }

    pub fn host_use_server(self: Pin<&mut Self>, server: QString, token: QString) -> QString {
        let result = crate::host::use_server(&server.to_string(), &token.to_string());
        QString::from(&result.unwrap_or_else(|e| format!("error: {}", e)))
    }

    pub fn host_set_sharing(self: Pin<&mut Self>, clipboard: bool, files: bool) -> QString {
        QString::from(&crate::host::set_sharing(clipboard, files).err().unwrap_or_default())
    }

    pub fn host_set_approve_mode(self: Pin<&mut Self>, mode: QString) -> QString {
        QString::from(&crate::host::set_approve_mode(&mode.to_string()).err().unwrap_or_default())
    }

    pub fn host_approvals(self: Pin<&mut Self>) -> QString {
        QString::from(&crate::host::approvals_json())
    }

    pub fn host_answer_approval(self: Pin<&mut Self>, id: i32, accept: bool) {
        crate::host::answer_approval(id.max(0) as u64, accept);
    }

    pub fn host_set_port(self: Pin<&mut Self>, port: i32) -> QString {
        let result = u16::try_from(port)
            .map_err(|_| "Use a port from 1024 to 65535".to_string())
            .and_then(crate::host::set_port);
        QString::from(&result.err().unwrap_or_default())
    }

    pub fn server_status(self: Pin<&mut Self>) -> QString {
        QString::from(&crate::local_server::status_json())
    }

    pub fn server_start(self: Pin<&mut Self>) -> QString {
        QString::from(&crate::local_server::start().err().unwrap_or_default())
    }

    pub fn server_stop(self: Pin<&mut Self>) -> QString {
        QString::from(&crate::local_server::stop().err().unwrap_or_default())
    }

    pub fn server_set_port(self: Pin<&mut Self>, port: i32) -> QString {
        let result = u16::try_from(port)
            .map_err(|_| "Use a port from 1024 to 65535".to_string())
            .and_then(crate::local_server::set_port);
        QString::from(&result.err().unwrap_or_default())
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

    pub fn files_open(self: Pin<&mut Self>, server_url: QString, token: QString, host_id: QString, host_name: QString) {
        crate::files::open(server_url.to_string(), token.to_string(), host_id.to_string(), host_name.to_string());
    }

    pub fn files_open_current(self: Pin<&mut Self>) -> bool {
        let args = ACTIVE_CONFIG.lock().unwrap().clone().or_else(|| APP_ARGS.get().cloned());
        match args {
            Some(a) if !a.server_url.is_empty() => {
                crate::files::open(a.server_url, a.token, a.host_id, a.host_name);
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
        if std::fs::write(&path, json).is_ok() {
            // "peers" may hold access passwords: owner-only.
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
            }
        }
    }

    pub fn app_version(self: Pin<&mut Self>) -> QString {
        QString::from(env!("CARGO_PKG_VERSION"))
    }

    pub fn saved_direct_address(self: Pin<&mut Self>) -> QString {
        QString::from(&load_direct_target().map(|t| t.address).unwrap_or_default())
    }

    pub fn saved_direct_password(self: Pin<&mut Self>) -> QString {
        QString::from(&load_direct_target().map(|t| t.password).unwrap_or_default())
    }

    pub fn probe_peer(self: Pin<&mut Self>, address: QString) {
        let address = address.to_string();
        let mut rust_obj = self.rust_mut();
        let rt = rust_obj.get_or_init_runtime();
        rt.spawn(async move {
            let ok = matches!(
                tokio::time::timeout(
                    std::time::Duration::from_millis(1500),
                    tokio::net::TcpStream::connect(address.as_str()),
                )
                .await,
                Ok(Ok(_))
            );
            PEER_ONLINE.lock().unwrap().insert(address, ok);
        });
    }

    pub fn peer_online(self: Pin<&mut Self>, address: QString) -> i32 {
        match PEER_ONLINE.lock().unwrap().get(&address.to_string()) {
            Some(true) => 1,
            Some(false) => 0,
            None => -1,
        }
    }

    pub fn save_direct_target(self: Pin<&mut Self>, address: QString, password: QString) {
        save_direct_target_file(&DirectTarget {
            address: address.to_string(),
            password: password.to_string(),
        });
    }

    pub fn get_local_hostname(self: Pin<&mut Self>) -> QString {
        let name = hostname::get()
            .ok()
            .and_then(|s| s.into_string().ok())
            .unwrap_or_else(|| "Local Host".to_string());
        QString::from(&name)
    }

    pub fn is_autostart_enabled(self: Pin<&mut Self>) -> bool {
        is_autostart_enabled_impl()
    }

    pub fn set_autostart_enabled(self: Pin<&mut Self>, enabled: bool) {
        set_autostart_enabled_impl(enabled);
    }

    pub fn should_start_minimized(self: Pin<&mut Self>) -> bool {
        std::env::args().any(|arg| arg == "--minimized")
    }

    pub fn check_for_updates(self: Pin<&mut Self>) {
        let mut rust_obj = self.rust_mut();
        let rt = rust_obj.get_or_init_runtime();

        rt.spawn(async move {
            let client = match reqwest::Client::builder()
                .user_agent("lunaris-client")
                .build()
            {
                Ok(c) => c,
                Err(_) => return,
            };

            let res = match client
                .get("https://api.github.com/repos/collyn/lunaris/releases/latest")
                .send()
                .await
            {
                Ok(r) => r,
                Err(_) => return,
            };

            if !res.status().is_success() {
                return;
            }

            #[derive(serde::Deserialize)]
            struct GithubRelease {
                tag_name: String,
                html_url: String,
            }

            let release: GithubRelease = match res.json().await {
                Ok(rel) => rel,
                Err(_) => return,
            };

            let current_version = env!("CARGO_PKG_VERSION");
            if is_newer_version(current_version, &release.tag_name) {
                let mut lock = PENDING_EVENTS.lock().unwrap();
                lock.push(PendingDashboardEvent::UpdateAvailable {
                    latest_version: release.tag_name,
                    release_url: release.html_url,
                });
            }
        });
    }
}

fn is_newer_version(current: &str, latest: &str) -> bool {
    let current_clean = current.trim_start_matches('v');
    let latest_clean = latest.trim_start_matches('v');

    let current_parts: Vec<&str> = current_clean.split('.').collect();
    let latest_parts: Vec<&str> = latest_clean.split('.').collect();

    for i in 0..std::cmp::max(current_parts.len(), latest_parts.len()) {
        let current_num = current_parts
            .get(i)
            .and_then(|s| s.parse::<u32>().ok())
            .unwrap_or(0);
        let latest_num = latest_parts
            .get(i)
            .and_then(|s| s.parse::<u32>().ok())
            .unwrap_or(0);

        if latest_num > current_num {
            return true;
        } else if current_num > latest_num {
            return false;
        }
    }
    false
}

pub fn stop_local_agent() {
    let mut lock = LOCAL_AGENT_CHILD.lock().unwrap();
    if let Some(mut child) = lock.take() {
        let _ = child.kill();
        let _ = child.wait();
        println!("Local agent stopped.");
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
    let (image_data_url, image_width, image_height, image_hotspot_x, image_hotspot_y) = value
        .get("image")
        .and_then(encode_cursor_png_data_url)
        .unwrap_or_else(|| (String::new(), 0, 0, 0, 0));

    *PENDING_HOST_CURSOR.lock().unwrap() = Some(PendingHostCursor {
        x: x.round() as i32,
        y: y.round() as i32,
        visible,
        kind,
        in_window_move_size,
        image_data_url,
        image_width,
        image_height,
        image_hotspot_x,
        image_hotspot_y,
    });
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

    let hash = cert_hash.as_deref().and_then(parse_sha256_hex);
    let config = match hash {
        Some(digest) => ClientConfig::builder()
            .with_bind_default()
            .with_server_certificate_hashes([wtransport::tls::Sha256Digest::new(digest)])
            .build(),
        // Older agents send no hash: accept their self-signed certificate.
        None => ClientConfig::builder().with_bind_default().with_no_cert_validation().build(),
    };
    let endpoint = match Endpoint::client(config) {
        Ok(e) => e,
        Err(e) => {
            eprintln!("WebTransport: cannot create the endpoint: {:?}", e);
            return;
        }
    };
    for host in hosts {
        let host_part = if host.contains(':') { format!("[{}]", host) } else { host.clone() };
        let url = format!("https://{}:{}", host_part, port);
        println!("WebTransport: connecting to {}", url);
        match tokio::time::timeout(std::time::Duration::from_secs(4), endpoint.connect(&url)).await {
            Ok(Ok(connection)) => {
                println!("WebTransport: connected to {}; input now goes over QUIC datagrams", url);
                *wt_conn.lock().unwrap() = Some(connection);
                wt_connected.store(true, std::sync::atomic::Ordering::Release);
                return;
            }
            Ok(Err(e)) => eprintln!("WebTransport: {} failed: {:?}", url, e),
            Err(_) => eprintln!("WebTransport: {} timed out", url),
        }
    }
    eprintln!("WebTransport: no agent address answered, input stays on WebRTC");
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
                                handle_host_cursor_message(&msg.data);
                            }
                            DataChannelEvent::OnClose => {
                                *PENDING_HOST_CURSOR.lock().unwrap() = Some(PendingHostCursor {
                                    x: 0,
                                    y: 0,
                                    visible: false,
                                    kind: "arrow".to_string(),
                                    in_window_move_size: false,
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
            let av1_obu_buf = Vec::<u8>::new();
            
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
                let mut av1_obu_buf = av1_obu_buf;
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
                        av1_obu_buf.clear();
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
                        av1_obu_buf.clear();
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
                                let h = payload[0];
                                let z = (h & 0x80) != 0;
                                let y = (h & 0x40) != 0;
                                let w = (h & 0x30) >> 4;
                                
                                let mut offset = 1;
                                
                                let read_leb128 = |off: &mut usize| -> Option<usize> {
                                    let mut value = 0;
                                    let mut shift = 0;
                                    while *off < payload.len() {
                                        let b = payload[*off];
                                        *off += 1;
                                        value |= ((b & 0x7F) as usize) << shift;
                                        if (b & 0x80) == 0 {
                                            return Some(value);
                                        }
                                        shift += 7;
                                        if shift >= 35 {
                                            return None;
                                        }
                                    }
                                    None
                                };
                                
                                let mut first = true;
                                
                                // Process AV1 OBU fragment
                                let mut process_fragment = |element_data: &[u8], is_first_elem: bool, is_last_elem: bool, av1_obu_buf_ref: &mut Vec<u8>, decoder_ref: &mut super::decoder::HardwareDecoder, decode_count_ref: &mut usize, total_decode_time_ref: &mut f64, frame_count_ref: &mut usize, has_decoded_ref: &Arc<std::sync::atomic::AtomicBool>, sink_ref: &VideoSinkWrapper| {
                                    if is_first_elem && z {
                                        av1_obu_buf_ref.extend_from_slice(element_data);
                                    } else {
                                        if !av1_obu_buf_ref.is_empty() {
                                            if let Err(e) = process_and_deliver(av1_obu_buf_ref, decoder_ref, decode_count_ref, total_decode_time_ref, frame_count_ref, has_decoded_ref, sink_ref) {
                                                eprintln!("Decoder error: {:?}. Requesting keyframe...", e);
                                                av1_obu_buf_ref.clear();
                                                let _ = pli_tx.send(());
                                            }
                                        }
                                        av1_obu_buf_ref.extend_from_slice(element_data);
                                    }
                                    
                                    if is_last_elem && y {
                                        // Fragment continues in next packet
                                    } else {
                                        if let Err(e) = process_and_deliver(av1_obu_buf_ref, decoder_ref, decode_count_ref, total_decode_time_ref, frame_count_ref, has_decoded_ref, sink_ref) {
                                            eprintln!("Decoder error: {:?}. Requesting keyframe...", e);
                                            av1_obu_buf_ref.clear();
                                            let _ = pli_tx.send(());
                                        }
                                    }
                                };
                                
                                if w == 0 {
                                    while offset < payload.len() {
                                        if let Some(size) = read_leb128(&mut offset) {
                                            if offset + size <= payload.len() {
                                                let element_data = &payload[offset..offset + size];
                                                offset += size;
                                                let is_last = offset >= payload.len();
                                                process_fragment(element_data, first, is_last, &mut av1_obu_buf, &mut decoder, &mut decode_count, &mut total_decode_time_ms, &mut frame_count, &has_decoded, &sink_inner);
                                                first = false;
                                            } else {
                                                break;
                                            }
                                        } else {
                                            break;
                                        }
                                    }
                                } else {
                                    for i in 0..w {
                                        let is_first = i == 0;
                                        let is_last = i == w - 1;
                                        if !is_last {
                                            if let Some(size) = read_leb128(&mut offset) {
                                                if offset + size <= payload.len() {
                                                    let element_data = &payload[offset..offset + size];
                                                    offset += size;
                                                    process_fragment(element_data, is_first, is_last, &mut av1_obu_buf, &mut decoder, &mut decode_count, &mut total_decode_time_ms, &mut frame_count, &has_decoded, &sink_inner);
                                                } else {
                                                    break;
                                                }
                                            } else {
                                                break;
                                            }
                                        } else {
                                            if offset < payload.len() {
                                                let element_data = &payload[offset..];
                                                process_fragment(element_data, is_first, is_last, &mut av1_obu_buf, &mut decoder, &mut decode_count, &mut total_decode_time_ms, &mut frame_count, &has_decoded, &sink_inner);
                                            }
                                        }
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
    virtual_display: bool,
    sink_wrapper: VideoSinkWrapper,
    input_senders: Arc<Mutex<Option<super::input::InputSenders>>>,
    active_decoder: Arc<Mutex<Option<std::thread::JoinHandle<()>>>>,
) -> Result<(), anyhow::Error> {
    let encoded_token: String = url::form_urlencoded::byte_serialize(token.as_bytes()).collect();
    let ws_url = format!(
        "{}/ws/client?token={}{}",
        server_url.replace("http", "ws"),
        encoded_token,
        crate::host::client_name_query()
    );
    // Never log ws_url itself: it carries the session token.
    println!("Connecting to signaling server at: {}/ws/client", server_url.replace("http", "ws"));

    let (ws_stream, _) = connect_async(ws_url.as_str()).await?;
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
        virtual_display: if virtual_display { Some(true) } else { None },
        chroma: request_chroma,
        encoder_csc_mode: requested_csc,
        microphone: crate::audio::record_microphone().then_some(true),
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
                        ..
                    } => {
                        if sdp.ty == RtcSdpType::Offer {
                            // WebTransport input starts once ICE is connected (below).
                            let wt_target = if input_protocol == "webtransport" {
                                webtransport_port.map(|port| (port, webtransport_cert_hash.clone()))
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

                            if let Some((port, cert_hash)) = wt_target {
                                tokio::spawn(connect_webtransport_input(
                                    Arc::clone(&pc),
                                    pc_state.clone(),
                                    server_url.clone(),
                                    port,
                                    cert_hash,
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
                                        s.ping = dynamic_ping;
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
                    SignalingMessage::AccessRequest { status, .. } => {
                        if let Some(refusal) = crate::host::access_refusal(&status) {
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
