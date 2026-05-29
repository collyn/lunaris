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
    PairResult {
        success: bool,
        error_msg: String,
    },
    UnpairResult {
        success: bool,
        error_msg: String,
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
    pub input_protocol: String,
}

pub static APP_ARGS: std::sync::OnceLock<AppArgs> = std::sync::OnceLock::new();
pub static ACTIVE_CONFIG: std::sync::Mutex<Option<AppArgs>> = std::sync::Mutex::new(None);

#[derive(Debug, Clone)]
pub struct StreamStats {
    pub ping: f64,
    pub decode: f64,
    pub fps: f64,
    pub bitrate: f64,
    pub codec: String,
}

pub static STREAM_STATS: std::sync::Mutex<Option<StreamStats>> = std::sync::Mutex::new(None);

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

        #[rust_name = "register_gpu_video_item_type"]
        fn register_gpu_video_item_type();

        #[rust_name = "set_cuda_stream_active"]
        fn set_cuda_stream_active(active: bool);
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
            codec: QString,
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
        fn pair_result(self: Pin<&mut StreamBridge>, success: bool, error_msg: QString);

        #[qsignal]
        fn unpair_result(self: Pin<&mut StreamBridge>, success: bool, error_msg: QString);

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
        fn send_mouse_wheel(self: Pin<&mut StreamBridge>, delta: i32);

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
        fn request_settings(self: Pin<&mut StreamBridge>);

        #[qinvokable]
        fn poll_stats(self: Pin<&mut StreamBridge>);

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

        #[qinvokable]
        fn pair_host(
            self: Pin<&mut StreamBridge>,
            name: QString,
            ip: QString,
            user: QString,
            pass: QString,
        );

        #[qinvokable]
        fn unpair_host(self: Pin<&mut StreamBridge>, host_id: QString);

        #[qinvokable]
        fn fetch_apps(self: Pin<&mut StreamBridge>, host_id: QString);

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

        #[qinvokable]
        fn is_autostart_enabled(self: Pin<&mut StreamBridge>) -> bool;

        #[qinvokable]
        fn set_autostart_enabled(self: Pin<&mut StreamBridge>, enabled: bool);

        #[qinvokable]
        fn should_start_minimized(self: Pin<&mut StreamBridge>) -> bool;
    }
}

use cxx_qt_lib::QString;
use qobject::deliver_yuv_frame;

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
        qobject::set_cuda_stream_active(true);

        // Load active config, if None, initialize from APP_ARGS
        let mut active_config_lock = ACTIVE_CONFIG.lock().unwrap();
        if active_config_lock.is_none() {
            if let Some(args) = APP_ARGS.get() {
                *active_config_lock = Some(args.clone());
            }
        }
        let args = match &*active_config_lock {
            Some(a) => a.clone(),
            None => {
                eprintln!("AppArgs static configuration not initialized!");
                return;
            }
        };
        // Drop lock before async tokio spawning
        drop(active_config_lock);

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
                sink_wrapper,
                input_senders,
                active_decoder,
            )
            .await
            {
                eprintln!("Error in WebRTC client task: {:?}", e);
            }
        });

        self.as_mut().rust_mut().tokio_runtime = Some(rt);
        *self.as_mut().rust_mut().active_stream.lock().unwrap() = Some(handle);
    }

    pub fn stop_stream(mut self: Pin<&mut Self>) {
        println!("Stopping stream and releasing signaling runtime...");
        qobject::set_cuda_stream_active(false);
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
        self: Pin<&mut Self>,
        x: i32,
        y: i32,
        width: i32,
        height: i32,
        rx: i32,
        ry: i32,
        pointer_locked: bool,
    ) {
        let binding = self.as_ref();
        let senders = binding.rust().input_senders.lock().unwrap();
        if let Some(ref s) = *senders {
            super::input::handle_mouse_move(x, y, width, height, rx, ry, pointer_locked, s);
        }
    }

    pub fn send_mouse_click(self: Pin<&mut Self>, button: i32, is_down: bool) {
        let binding = self.as_ref();
        let senders = binding.rust().input_senders.lock().unwrap();
        if let Some(ref s) = *senders {
            super::input::handle_mouse_click(button, is_down, s);
        }
    }

    pub fn send_mouse_wheel(self: Pin<&mut Self>, delta: i32) {
        let binding = self.as_ref();
        let senders = binding.rust().input_senders.lock().unwrap();
        if let Some(ref s) = *senders {
            super::input::handle_mouse_wheel(delta, s);
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
        mut self: Pin<&mut Self>,
        res: QString,
        fps: i32,
        codec: QString,
        bitrate: i32,
        mouse_queue_limit: i32,
        disable_cuda: bool,
        input_protocol: QString,
    ) {
        let res_str = res.to_string();
        let codec_str = codec.to_string().to_lowercase();
        let input_proto_str = input_protocol.to_string().to_lowercase();
        println!("Updating stream configuration: res={}, fps={}, codec={}, bitrate={}, mouse_queue_limit={}, disable_cuda={}, input_protocol={}", res_str, fps, codec_str, bitrate, mouse_queue_limit, disable_cuda, input_proto_str);

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
                config.disable_cuda = disable_cuda;
                config.input_protocol = input_proto_str;
            } else if let Some(args) = APP_ARGS.get() {
                let mut new_config = args.clone();
                new_config.width = width;
                new_config.height = height;
                new_config.fps = fps as u32;
                new_config.codec = codec_str;
                new_config.bitrate = bitrate as u32;
                new_config.mouse_queue_limit = mouse_queue_limit as u32;
                new_config.disable_cuda = disable_cuda;
                new_config.input_protocol = input_proto_str;
                *active_config_lock = Some(new_config);
            }
        }

        // Restart stream
        self.as_mut().stop_stream();
        self.as_mut().start_stream();
    }

    pub fn poll_stats(mut self: Pin<&mut Self>) {
        let stats = { STREAM_STATS.lock().unwrap().clone() };
        if let Some(s) = stats {
            let codec_qstring = cxx_qt_lib::QString::from(&s.codec);
            self.as_mut()
                .stats_updated(s.ping, s.decode, s.fps, s.bitrate, codec_qstring);
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
            self.as_mut().settings_loaded(
                res_qstring,
                config.fps as i32,
                codec_qstring,
                config.bitrate as i32,
                config.mouse_queue_limit as i32,
                host_name_qstring,
                config.disable_cuda,
                input_proto_qstring,
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

    pub fn pair_host(
        self: Pin<&mut Self>,
        name: QString,
        ip: QString,
        user: QString,
        pass: QString,
    ) {
        let name_str = name.to_string();
        let ip_str = ip.to_string();
        let user_str = user.to_string();
        let pass_str = pass.to_string();

        let settings = match load_settings() {
            Some(s) => s,
            None => {
                PENDING_EVENTS
                    .lock()
                    .unwrap()
                    .push(PendingDashboardEvent::PairResult {
                        success: false,
                        error_msg: "Not authenticated".to_string(),
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
            let url = format!("{}/api/hosts/pair", server_str);

            let pair_req = common::PairHostRequest {
                name: name_str,
                ip_address: ip_str,
                sunshine_username: if user_str.is_empty() {
                    None
                } else {
                    Some(user_str)
                },
                sunshine_password: if pass_str.is_empty() {
                    None
                } else {
                    Some(pass_str)
                },
            };

            let res = client
                .post(&url)
                .header("Authorization", format!("Bearer {}", token_str))
                .json(&pair_req)
                .send()
                .await;

            match res {
                Ok(resp) => {
                    if resp.status().is_success() {
                        PENDING_EVENTS
                            .lock()
                            .unwrap()
                            .push(PendingDashboardEvent::PairResult {
                                success: true,
                                error_msg: "".to_string(),
                            });
                    } else {
                        let err_msg = if let Ok(err_data) = resp.json::<serde_json::Value>().await {
                            err_data
                                .get("error")
                                .and_then(|v| v.as_str())
                                .unwrap_or("Pairing failed")
                                .to_string()
                        } else {
                            "Pairing failed".to_string()
                        };
                        PENDING_EVENTS
                            .lock()
                            .unwrap()
                            .push(PendingDashboardEvent::PairResult {
                                success: false,
                                error_msg: err_msg,
                            });
                    }
                }
                Err(e) => {
                    let err_msg = format!("Connection failed: {}", e);
                    PENDING_EVENTS
                        .lock()
                        .unwrap()
                        .push(PendingDashboardEvent::PairResult {
                            success: false,
                            error_msg: err_msg,
                        });
                }
            }
        });
    }

    pub fn unpair_host(self: Pin<&mut Self>, host_id: QString) {
        let host_id_str = host_id.to_string();

        let settings = match load_settings() {
            Some(s) => s,
            None => {
                PENDING_EVENTS
                    .lock()
                    .unwrap()
                    .push(PendingDashboardEvent::UnpairResult {
                        success: false,
                        error_msg: "Not authenticated".to_string(),
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
            let url = format!("{}/api/hosts/{}", server_str, host_id_str);

            let res = client
                .delete(&url)
                .header("Authorization", format!("Bearer {}", token_str))
                .send()
                .await;

            match res {
                Ok(resp) => {
                    if resp.status().is_success() {
                        PENDING_EVENTS
                            .lock()
                            .unwrap()
                            .push(PendingDashboardEvent::UnpairResult {
                                success: true,
                                error_msg: "".to_string(),
                            });
                    } else {
                        PENDING_EVENTS
                            .lock()
                            .unwrap()
                            .push(PendingDashboardEvent::UnpairResult {
                                success: false,
                                error_msg: "Failed to unpair host".to_string(),
                            });
                    }
                }
                Err(e) => {
                    let err_msg = format!("Connection failed: {}", e);
                    PENDING_EVENTS
                        .lock()
                        .unwrap()
                        .push(PendingDashboardEvent::UnpairResult {
                            success: false,
                            error_msg: err_msg,
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
            println!("fetch_apps connecting to ws at: {}", ws_url);

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
                if let Err(e) = ws_stream.send(WsMessage::Text(text)).await {
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

    pub fn start_game_session(
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
        input_protocol: QString,
    ) {
        let server_str = server.to_string();
        let token_str = token.to_string();
        let host_id_str = host_id.to_string();
        let host_name_str = host_name.to_string();
        let res_str = res.to_string();
        let codec_str = codec.to_string().to_lowercase();
        let input_proto_str = input_protocol.to_string().to_lowercase();

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
            disable_cuda,
            input_protocol: input_proto_str,
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
                PendingDashboardEvent::PairResult { success, error_msg } => {
                    let err_qstr = QString::from(&error_msg);
                    self.as_mut().pair_result(success, err_qstr);
                }
                PendingDashboardEvent::UnpairResult { success, error_msg } => {
                    let err_qstr = QString::from(&error_msg);
                    self.as_mut().unpair_result(success, err_qstr);
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
                PendingDashboardEvent::DeepLinkReceived { url } => {
                    let url_qstr = QString::from(&url);
                    self.as_mut().deeplink_received(url_qstr);
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
use wtransport::{ClientConfig, Endpoint};
use webrtc::api::media_engine::MediaEngine;
use webrtc::api::APIBuilder;
use webrtc::data_channel::RTCDataChannel;
use webrtc::ice_transport::ice_candidate::{RTCIceCandidate, RTCIceCandidateInit};
use webrtc::ice_transport::ice_server::RTCIceServer;
use webrtc::peer_connection::configuration::RTCConfiguration;
use webrtc::peer_connection::sdp::session_description::RTCSessionDescription;
use webrtc::peer_connection::RTCPeerConnection;
use webrtc::track::track_remote::TrackRemote;

async fn setup_peer_connection(
    ice_servers: Option<Vec<common::RtcIceServer>>,
    outbox_tx: tokio::sync::mpsc::UnboundedSender<ClientMessage>,
    host_id: String,
    sink_wrapper: VideoSinkWrapper,
    kb_chan_ref: Arc<Mutex<Option<Arc<RTCDataChannel>>>>,
    ma_chan_ref: Arc<Mutex<Option<Arc<RTCDataChannel>>>>,
    mr_chan_ref: Arc<Mutex<Option<Arc<RTCDataChannel>>>>,
    active_decoder: Arc<Mutex<Option<std::thread::JoinHandle<()>>>>,
) -> Result<Arc<RTCPeerConnection>, anyhow::Error> {
    // WebRTC connection setup
    let mut media_engine = MediaEngine::default();
    media_engine.register_default_codecs()?;
    let api = APIBuilder::new().with_media_engine(media_engine).build();

    let webrtc_ice_servers = if let Some(servers) = ice_servers {
        servers
            .into_iter()
            .map(|s| RTCIceServer {
                urls: s.urls,
                username: s.username.unwrap_or_default(),
                credential: s.credential.unwrap_or_default(),
                ..Default::default()
            })
            .collect()
    } else {
        vec![RTCIceServer {
            urls: vec!["stun:stun.l.google.com:19302".to_string()],
            ..Default::default()
        }]
    };

    let config = RTCConfiguration {
        ice_servers: webrtc_ice_servers,
        ..Default::default()
    };

    let peer_connection = Arc::new(api.new_peer_connection(config).await?);

    // Register ICE candidate gathering callback
    let outbox_tx_clone = outbox_tx.clone();
    let host_id_clone = host_id.clone();
    peer_connection.on_ice_candidate(Box::new(move |candidate: Option<RTCIceCandidate>| {
        let outbox_tx = outbox_tx_clone.clone();
        let host_id = host_id_clone.clone();
        Box::pin(async move {
            if let Some(cand) = candidate {
                if let Ok(json_cand) = cand.to_json() {
                    let msg = ClientMessage::Signaling(SignalingMessage::IceCandidate {
                        target_id: host_id,
                        candidate: common::RtcIceCandidate {
                            candidate: json_cand.candidate,
                            sdp_mid: json_cand.sdp_mid,
                            sdp_mline_index: json_cand.sdp_mline_index,
                            username_fragment: json_cand.username_fragment,
                        },
                    });
                    let _ = outbox_tx.send(msg);
                }
            }
        })
    }));

    // Setup video track handler
    let sink_clone = sink_wrapper.clone();
    let pc_clone = Arc::clone(&peer_connection);
    let active_decoder_clone = active_decoder.clone();
    peer_connection.on_track(Box::new(move |track: Arc<TrackRemote>, _receiver, _| {
        let track_clone = Arc::clone(&track);
        let codec = track.codec();
        let sink_inner = sink_clone.clone();
        let pc_clone_inner = Arc::clone(&pc_clone);
        let active_decoder_inner = active_decoder_clone.clone();
        
        Box::pin(async move {
            let mime = codec.capability.mime_type.to_lowercase();
            let is_video = mime == "video/h264" || mime == "video/h265" || mime == "video/hevc" || mime == "video/av1";
            
            if is_video {
                println!("Starting video receiver for: {}", mime);
                let codec_type = match mime.as_str() {
                    "video/h264" => super::decoder::CodecType::H264,
                    "video/h265" | "video/hevc" => super::decoder::CodecType::H265,
                    "video/av1" => super::decoder::CodecType::AV1,
                    _ => unreachable!(),
                };
                
                let disable_cuda = if let Some(ref config) = *ACTIVE_CONFIG.lock().unwrap() {
                    config.disable_cuda
                } else {
                    false
                };
                let decoder = match super::decoder::HardwareDecoder::new(codec_type, disable_cuda) {
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
                let media_ssrc = track_clone.ssrc();
                let pc_clone_deep = Arc::clone(&pc_clone_inner);
                
                tokio::spawn(async move {
                    use webrtc::rtcp::payload_feedbacks::picture_loss_indication::PictureLossIndication;
                    println!("PLI requester task started for video track SSRC: {}", media_ssrc);
                    
                    while !has_decoded_clone.load(std::sync::atomic::Ordering::SeqCst) {
                        let pli = PictureLossIndication {
                            sender_ssrc: 0,
                            media_ssrc,
                        };
                        if let Err(e) = pc_clone_deep.write_rtcp(&[Box::new(pli)]).await {
                            eprintln!("Failed to send PLI request: {:?}", e);
                        }
                        tokio::time::sleep(std::time::Duration::from_millis(1000)).await;
                    }
                    println!("First frame decoded, stopping periodic PLI requests.");
                });
                
                let pending_packets = Arc::new(std::sync::atomic::AtomicUsize::new(0));
                let pending_packets_reader = Arc::clone(&pending_packets);
                let pending_packets_decoder = Arc::clone(&pending_packets);

                let (rtp_tx, mut rtp_rx) = tokio::sync::mpsc::unbounded_channel();
                let track_clone_reader = Arc::clone(&track_clone);
                tokio::spawn(async move {
                    while let Ok((rtp_packet, _)) = track_clone_reader.read_rtp().await {
                        if rtp_packet.payload.is_empty() {
                            continue;
                        }
                        pending_packets_reader.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                        if let Err(_) = rtp_tx.send(rtp_packet) {
                            break;
                        }
                    }
                });

                let (pli_tx, mut pli_rx) = tokio::sync::mpsc::unbounded_channel::<()>();
                let pc_clone_deep2 = Arc::clone(&pc_clone_inner);
                tokio::spawn(async move {
                    let mut last_pli_time = std::time::Instant::now();
                    while let Some(_) = pli_rx.recv().await {
                        let now = std::time::Instant::now();
                        if now.duration_since(last_pli_time) > std::time::Duration::from_millis(500) {
                            let pli = webrtc::rtcp::payload_feedbacks::picture_loss_indication::PictureLossIndication {
                                sender_ssrc: 0,
                                media_ssrc,
                            };
                            if let Err(e) = pc_clone_deep2.write_rtcp(&[Box::new(pli)]).await {
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
                    let mut packet_buffer = std::collections::BTreeMap::<u64, webrtc::rtp::packet::Packet>::new();

                    while let Some(rtp_packet) = rtp_rx.blocking_recv() {
                        pending_packets_decoder.fetch_sub(1, std::sync::atomic::Ordering::SeqCst);

                        // Check for backlog (decoder lagging)
                        let pending_val = pending_packets_decoder.load(std::sync::atomic::Ordering::SeqCst);
                        if pending_val > 150 {
                            eprintln!("Video receiver queue lag detected ({} packets). Flushing queue...", pending_val);
                            while let Ok(_) = rtp_rx.try_recv() {
                                pending_packets_decoder.fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
                            }
                            packet_buffer.clear();
                            annex_b_buf.clear();
                            av1_obu_buf.clear();
                            let _ = pli_tx.send(());
                            next_seq_num = None;
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

                        // If buffer is too large, we have packet loss
                        if packet_buffer.len() > 64 {
                            let &first_seq = packet_buffer.keys().next().unwrap();
                            let missing_start = next_seq_num.unwrap();
                            let missing_end = first_seq - 1;
                            eprintln!("RTP packet loss detected: missing sequence from {} to {}", missing_start, missing_end);
                            
                            // Clear corrupted frame buffer and request keyframe
                            annex_b_buf.clear();
                            av1_obu_buf.clear();
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
                            
                            let process_and_deliver = |annex_b_data: &[u8], decoder_ref: &mut super::decoder::HardwareDecoder, decode_count_ref: &mut usize, total_decode_time_ref: &mut f64, frame_count_ref: &mut usize, has_decoded_ref: &Arc<std::sync::atomic::AtomicBool>, sink_ref: &VideoSinkWrapper| -> Result<(), anyhow::Error> {
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
                                    if frame.width == 0 {
                                        continue;
                                    }
                                    let sink_lock = sink_ref.sink.lock().unwrap();
                                    if *frame_count_ref % 60 == 0 {
                                        println!("Decoded software frame: {}x{}, sink={:?}", frame.width, frame.height, *sink_lock);
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
                                Ok(())
                            };
                            
                            match codec_type {
                                super::decoder::CodecType::H264 => {
                                    let nal_type = payload[0] & 0x1F;
                                    if nal_type >= 1 && nal_type <= 23 {
                                        // Single NAL unit
                                        let is_vcl = nal_type >= 1 && nal_type <= 5;
                                        annex_b_buf.extend_from_slice(&[0, 0, 0, 1]);
                                        annex_b_buf.extend_from_slice(payload);
                                        
                                        if is_vcl {
                                            if let Err(e) = process_and_deliver(&annex_b_buf, &mut decoder, &mut decode_count, &mut total_decode_time_ms, &mut frame_count, &has_decoded, &sink_inner) {
                                                eprintln!("Decoder error: {:?}. Requesting keyframe...", e);
                                                let _ = pli_tx.send(());
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
                                                let is_vcl = inner_nal_type >= 1 && inner_nal_type <= 5;
                                                annex_b_buf.extend_from_slice(&[0, 0, 0, 1]);
                                                annex_b_buf.extend_from_slice(nalu_data);
                                                
                                                if is_vcl {
                                                    if let Err(e) = process_and_deliver(&annex_b_buf, &mut decoder, &mut decode_count, &mut total_decode_time_ms, &mut frame_count, &has_decoded, &sink_inner) {
                                                        eprintln!("Decoder error: {:?}. Requesting keyframe...", e);
                                                        let _ = pli_tx.send(());
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
                                            annex_b_buf.extend_from_slice(&[0, 0, 0, 1, reconstructed_header]);
                                            annex_b_buf.extend_from_slice(&payload[2..]);
                                        } else {
                                            annex_b_buf.extend_from_slice(&payload[2..]);
                                        }
                                        
                                        if end_bit {
                                            if let Err(e) = process_and_deliver(&annex_b_buf, &mut decoder, &mut decode_count, &mut total_decode_time_ms, &mut frame_count, &has_decoded, &sink_inner) {
                                                eprintln!("Decoder error: {:?}. Requesting keyframe...", e);
                                                let _ = pli_tx.send(());
                                            }
                                            annex_b_buf.clear();
                                        }
                                    }
                                }
                                super::decoder::CodecType::H265 => {
                                    let nal_type = (payload[0] & 0x7E) >> 1;
                                    if nal_type <= 47 {
                                        // Single NAL unit
                                        let is_vcl = nal_type <= 31;
                                        annex_b_buf.extend_from_slice(&[0, 0, 0, 1]);
                                        annex_b_buf.extend_from_slice(payload);
                                        
                                        if is_vcl {
                                            if let Err(e) = process_and_deliver(&annex_b_buf, &mut decoder, &mut decode_count, &mut total_decode_time_ms, &mut frame_count, &has_decoded, &sink_inner) {
                                                eprintln!("Decoder error: {:?}. Requesting keyframe...", e);
                                                let _ = pli_tx.send(());
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
                                            
                                            if !nalu_data.is_empty() {
                                                let inner_nal_type = (nalu_data[0] & 0x7E) >> 1;
                                                let is_vcl = inner_nal_type <= 31;
                                                annex_b_buf.extend_from_slice(&[0, 0, 0, 1]);
                                                annex_b_buf.extend_from_slice(nalu_data);
                                                
                                                if is_vcl {
                                                    if let Err(e) = process_and_deliver(&annex_b_buf, &mut decoder, &mut decode_count, &mut total_decode_time_ms, &mut frame_count, &has_decoded, &sink_inner) {
                                                        eprintln!("Decoder error: {:?}. Requesting keyframe...", e);
                                                        let _ = pli_tx.send(());
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
                                        
                                        let reconstructed_header_1 = (fu_indicator_1 & 0x81) | (original_nal_type << 1);
                                        let reconstructed_header_2 = fu_indicator_2;
                                        
                                        if start_bit {
                                            annex_b_buf.extend_from_slice(&[0, 0, 0, 1, reconstructed_header_1, reconstructed_header_2]);
                                            annex_b_buf.extend_from_slice(&payload[3..]);
                                        } else {
                                            annex_b_buf.extend_from_slice(&payload[3..]);
                                        }
                                        
                                        if end_bit {
                                            if let Err(e) = process_and_deliver(&annex_b_buf, &mut decoder, &mut decode_count, &mut total_decode_time_ms, &mut frame_count, &has_decoded, &sink_inner) {
                                                eprintln!("Decoder error: {:?}. Requesting keyframe...", e);
                                                let _ = pli_tx.send(());
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
                                    let process_fragment = |element_data: &[u8], is_first_elem: bool, is_last_elem: bool, av1_obu_buf_ref: &mut Vec<u8>, decoder_ref: &mut super::decoder::HardwareDecoder, decode_count_ref: &mut usize, total_decode_time_ref: &mut f64, frame_count_ref: &mut usize, has_decoded_ref: &Arc<std::sync::atomic::AtomicBool>, sink_ref: &VideoSinkWrapper| {
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
                        let elapsed = now.duration_since(last_stats_time).as_secs_f64();
                        if elapsed >= 1.0 {
                            let fps = frame_count as f64 / elapsed;
                            let bitrate_kbps = (byte_count as f64 * 8.0) / 1000.0 / elapsed;
                            let avg_decode_ms = if decode_count > 0 {
                                total_decode_time_ms / decode_count as f64
                            } else {
                                0.0
                            };
                            
                            let codec_name = match codec_type {
                                super::decoder::CodecType::H264 => "H264",
                                super::decoder::CodecType::H265 => "H265",
                                super::decoder::CodecType::AV1 => "AV1",
                            }.to_string();
                            
                            *STREAM_STATS.lock().unwrap() = Some(StreamStats {
                                ping: 2.1,
                                decode: avg_decode_ms,
                                fps,
                                bitrate: bitrate_kbps,
                                codec: codec_name,
                            });
                            
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
                if let Some(audio_tx) = super::audio::setup_audio() {
                    let mut decoder = match opus::Decoder::new(48000, opus::Channels::Stereo) {
                        Ok(d) => d,
                        Err(e) => {
                            eprintln!("Failed to create Opus decoder: {:?}", e);
                            return;
                        }
                    };
                    let mut pcm_output = vec![0.0f32; 1920 * 2];
                    
                    while let Ok((rtp_packet, _)) = track_clone.read_rtp().await {
                        if let Ok(num_samples) = decoder.decode_float(&rtp_packet.payload, &mut pcm_output, false) {
                            let stereo_samples = pcm_output[..num_samples * 2].to_vec();
                            let _ = audio_tx.send(stereo_samples);
                        }
                    }
                }
            }
        })
    }));

    // Setup data channel callbacks
    let k_c = Arc::clone(&kb_chan_ref);
    let ma_c = Arc::clone(&ma_chan_ref);
    let mr_c = Arc::clone(&mr_chan_ref);

    peer_connection.on_data_channel(Box::new(move |d: Arc<RTCDataChannel>| {
        let label = d.label().to_string();
        println!("Remote host created DataChannel: {}", label);
        let channel_ref = Arc::clone(&d);
        match label.as_str() {
            "keyboard" => {
                *k_c.lock().unwrap() = Some(channel_ref);
            }
            "mouse_absolute" => {
                *ma_c.lock().unwrap() = Some(channel_ref);
            }
            "mouse_relative" => {
                *mr_c.lock().unwrap() = Some(channel_ref);
            }
            _ => {}
        }
        Box::pin(async {})
    }));

    Ok(peer_connection)
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
    sink_wrapper: VideoSinkWrapper,
    input_senders: Arc<Mutex<Option<super::input::InputSenders>>>,
    active_decoder: Arc<Mutex<Option<std::thread::JoinHandle<()>>>>,
) -> Result<(), anyhow::Error> {
    let ws_url = format!(
        "{}/ws/client?token={}",
        server_url.replace("http", "ws"),
        token
    );
    println!("Connecting to signaling server at: {}", ws_url);

    let (ws_stream, _) = connect_async(url::Url::parse(&ws_url)?).await?;
    let (mut ws_write, mut ws_read) = ws_stream.split();

    let (outbox_tx, mut outbox_rx) = tokio::sync::mpsc::unbounded_channel::<ClientMessage>();

    let mut peer_connection: Option<Arc<RTCPeerConnection>> = None;

    // Setup input channels
    let (kb_tx, mut kb_rx) = tokio::sync::mpsc::unbounded_channel::<Bytes>();
    let (ma_tx, mut ma_rx) = tokio::sync::mpsc::unbounded_channel::<Bytes>();
    let (mr_tx, mut mr_rx) = tokio::sync::mpsc::unbounded_channel::<Bytes>();

    let senders = super::input::InputSenders {
        keyboard: kb_tx,
        mouse_abs: ma_tx,
        mouse_rel: mr_tx,
    };
    *input_senders.lock().unwrap() = Some(senders);

    // Setup data channel callbacks
    let kb_chan_ref: Arc<Mutex<Option<Arc<RTCDataChannel>>>> = Arc::new(Mutex::new(None));
    let ma_chan_ref: Arc<Mutex<Option<Arc<RTCDataChannel>>>> = Arc::new(Mutex::new(None));
    let mr_chan_ref: Arc<Mutex<Option<Arc<RTCDataChannel>>>> = Arc::new(Mutex::new(None));

    let wt_conn_ref: Arc<Mutex<Option<wtransport::Connection>>> = Arc::new(Mutex::new(None));

    // Spawn input senders tasks
    let k_c = Arc::clone(&kb_chan_ref);
    let wt_c = Arc::clone(&wt_conn_ref);
    tokio::spawn(async move {
        while let Some(buf) = kb_rx.recv().await {
            let wt_sent = {
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
            };

            if !wt_sent {
                let chan = { k_c.lock().unwrap().clone() };
                if let Some(chan) = chan {
                    let _ = chan.send(&buf).await;
                }
            }
        }
    });

    let ma_c = Arc::clone(&ma_chan_ref);
    let wt_c = Arc::clone(&wt_conn_ref);
    tokio::spawn(async move {
        let mut last_check = std::time::Instant::now();
        let mut pkts_since_check = 0;
        let mut cached_buffer_ok = true;
        while let Some(buf) = ma_rx.recv().await {
            let chan = { ma_c.lock().unwrap().clone() };
            let mut final_buf = buf;
            while let Ok(next_buf) = ma_rx.try_recv() {
                final_buf = next_buf;
            }

            let wt_sent = {
                let lock = wt_c.lock().unwrap();
                if let Some(ref conn) = *lock {
                    let mut wt_buf = vec![0u8; final_buf.len() + 1];
                    wt_buf[0] = 5; // Channel 5: mouse_absolute
                    wt_buf[1..].copy_from_slice(&final_buf);
                    if let Err(e) = conn.send_datagram(&wt_buf) {
                        eprintln!("WebTransport send_datagram mouse_abs failed: {:?}", e);
                        false
                    } else {
                        true
                    }
                } else {
                    false
                }
            };

            if !wt_sent {
                if let Some(chan) = chan {
                    if mouse_queue_limit > 0 {
                        let now = std::time::Instant::now();
                        pkts_since_check += 1;
                        if pkts_since_check >= 16 || now.duration_since(last_check).as_millis() >= 50 {
                            let amt = chan.buffered_amount().await;
                            cached_buffer_ok = amt < mouse_queue_limit as usize;
                            last_check = now;
                            pkts_since_check = 0;
                        }
                    } else {
                        cached_buffer_ok = true;
                    }

                    if cached_buffer_ok {
                        let _ = chan.send(&final_buf).await;
                    }
                }
            }
        }
    });

    let mr_c = Arc::clone(&mr_chan_ref);
    let wt_c = Arc::clone(&wt_conn_ref);
    tokio::spawn(async move {
        let mut last_check = std::time::Instant::now();
        let mut pkts_since_check = 0;
        let mut cached_buffer_ok = true;
        while let Some(buf) = mr_rx.recv().await {
            let chan = { mr_c.lock().unwrap().clone() };
            let mut final_buf = buf;

            // If this is a relative mouse motion event (Type 0), coalesce it with consecutive motions in the queue
            if final_buf[0] == 0 {
                let mut dx = i16::from_be_bytes([final_buf[1], final_buf[2]]);
                let mut dy = i16::from_be_bytes([final_buf[3], final_buf[4]]);
                let mut ts = u32::from_be_bytes([
                    final_buf[5],
                    final_buf[6],
                    final_buf[7],
                    final_buf[8],
                ]);
                let mut coalesced = false;

                while let Ok(next_buf) = mr_rx.try_recv() {
                    if next_buf[0] == 0 {
                        dx = dx.wrapping_add(i16::from_be_bytes([next_buf[1], next_buf[2]]));
                        dy = dy.wrapping_add(i16::from_be_bytes([next_buf[3], next_buf[4]]));
                        ts = u32::from_be_bytes([
                            next_buf[5],
                            next_buf[6],
                            next_buf[7],
                            next_buf[8],
                        ]);
                        coalesced = true;
                    } else {
                        // Non-motion event (e.g. click, scroll) - send current accumulated motion first
                        let mut motion_buf = vec![0u8; 9];
                        motion_buf[0] = 0;
                        motion_buf[1..3].copy_from_slice(&dx.to_be_bytes());
                        motion_buf[3..5].copy_from_slice(&dy.to_be_bytes());
                        motion_buf[5..9].copy_from_slice(&ts.to_be_bytes());

                        let wt_sent = {
                            let lock = wt_c.lock().unwrap();
                            if let Some(ref conn) = *lock {
                                let mut wt_buf = vec![0u8; motion_buf.len() + 1];
                                wt_buf[0] = 6; // Channel 6: mouse_relative
                                wt_buf[1..].copy_from_slice(&motion_buf);
                                if let Err(e) = conn.send_datagram(&wt_buf) {
                                    eprintln!("WebTransport send_datagram mouse_rel motion failed: {:?}", e);
                                    false
                                } else {
                                    true
                                }
                            } else {
                                false
                            }
                        };

                        if !wt_sent {
                            if let Some(ref c) = chan {
                                if mouse_queue_limit > 0 {
                                    let now = std::time::Instant::now();
                                    pkts_since_check += 1;
                                    if pkts_since_check >= 16
                                        || now.duration_since(last_check).as_millis() >= 50
                                    {
                                        let amt = c.buffered_amount().await;
                                        cached_buffer_ok = amt < mouse_queue_limit as usize;
                                        last_check = now;
                                        pkts_since_check = 0;
                                    }
                                } else {
                                    cached_buffer_ok = true;
                                }

                                if cached_buffer_ok {
                                    let _ = c.send(&Bytes::from(motion_buf)).await;
                                }
                            }
                        }

                        // Now transition final_buf to this non-motion event
                        final_buf = next_buf;
                        coalesced = false;
                        break;
                    }
                }

                if coalesced {
                    let mut motion_buf = vec![0u8; 9];
                    motion_buf[0] = 0;
                    motion_buf[1..3].copy_from_slice(&dx.to_be_bytes());
                    motion_buf[3..5].copy_from_slice(&dy.to_be_bytes());
                    motion_buf[5..9].copy_from_slice(&ts.to_be_bytes());
                    final_buf = Bytes::from(motion_buf);
                }
            }

            let wt_sent = {
                let lock = wt_c.lock().unwrap();
                if let Some(ref conn) = *lock {
                    let mut wt_buf = vec![0u8; final_buf.len() + 1];
                    wt_buf[0] = 6; // Channel 6: mouse_relative
                    wt_buf[1..].copy_from_slice(&final_buf);
                    if let Err(e) = conn.send_datagram(&wt_buf) {
                        eprintln!("WebTransport send_datagram mouse_rel final failed: {:?}", e);
                        false
                    } else {
                        true
                    }
                } else {
                    false
                }
            };

            if !wt_sent {
                if let Some(ref c) = chan {
                    if mouse_queue_limit > 0 {
                        let now = std::time::Instant::now();
                        pkts_since_check += 1;
                        if pkts_since_check >= 16 || now.duration_since(last_check).as_millis() >= 50 {
                            let amt = c.buffered_amount().await;
                            cached_buffer_ok = amt < mouse_queue_limit as usize;
                            last_check = now;
                            pkts_since_check = 0;
                        }
                    } else {
                        cached_buffer_ok = true;
                    }

                    if cached_buffer_ok {
                        let _ = c.send(&final_buf).await;
                    }
                }
            }
        }
    });

    // Write outgoing WS messages
    tokio::spawn(async move {
        while let Some(msg) = outbox_rx.recv().await {
            if let Ok(text) = serde_json::to_string(&msg) {
                if let Err(e) = ws_write.send(WsMessage::Text(text)).await {
                    eprintln!("WS write error: {:?}", e);
                    break;
                }
            }
        }
    });

    // Send RequestSession command
    let req_msg = ClientMessage::Signaling(SignalingMessage::RequestSession {
        host_id: host_id.clone(),
        width: Some(width),
        height: Some(height),
        fps: Some(fps),
        bitrate: Some(bitrate),
        codec: Some(codec_str.clone()),
        app_id,
    });
    outbox_tx.send(req_msg)?;

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
                    SignalingMessage::Sdp {
                        sdp, ice_servers, webtransport_port, ..
                    } => {
                        if sdp.ty == RtcSdpType::Offer {
                            if input_protocol == "webtransport" {
                                if let Some(port) = webtransport_port {
                                    let wt_conn_c = Arc::clone(&wt_conn_ref);
                                    let server_url_c = server_url.clone();
                                    tokio::spawn(async move {
                                        if let Ok(parsed_url) = url::Url::parse(&server_url_c) {
                                            if let Some(host) = parsed_url.host_str() {
                                                println!("WebTransport: Connecting to https://{}:{}", host, port);
                                                let config = ClientConfig::builder()
                                                    .with_bind_default()
                                                    .with_no_cert_validation()
                                                    .build();
                                                match Endpoint::client(config) {
                                                    Ok(endpoint) => {
                                                        match endpoint.connect(format!("https://{}:{}", host, port)).await {
                                                            Ok(connection) => {
                                                                println!("WebTransport connected successfully!");
                                                                *wt_conn_c.lock().unwrap() = Some(connection);
                                                            }
                                                            Err(e) => {
                                                                eprintln!("WebTransport connection to {}:{} failed: {:?}", host, port, e);
                                                            }
                                                        }
                                                    }
                                                    Err(e) => {
                                                        eprintln!("Failed to construct WebTransport endpoint: {:?}", e);
                                                    }
                                                }
                                            }
                                        }
                                    });
                                }
                            }

                            let pc = match setup_peer_connection(
                                ice_servers,
                                outbox_tx.clone(),
                                host_id.clone(),
                                sink_wrapper.clone(),
                                kb_chan_ref.clone(),
                                ma_chan_ref.clone(),
                                mr_chan_ref.clone(),
                                active_decoder.clone(),
                            )
                            .await
                            {
                                Ok(pc) => pc,
                                Err(e) => {
                                    eprintln!("Failed to setup peer connection: {:?}", e);
                                    continue;
                                }
                            };

                            if let Ok(rtc_sdp) = RTCSessionDescription::offer(sdp.sdp) {
                                if let Err(e) = pc.set_remote_description(rtc_sdp).await {
                                    eprintln!("Failed to set remote description: {:?}", e);
                                    continue;
                                }

                                if let Ok(answer) = pc.create_answer(None).await {
                                    if let Err(e) = pc.set_local_description(answer.clone()).await {
                                        eprintln!("Failed to set local description: {:?}", e);
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
                                    let _ = outbox_tx.send(answer_msg);
                                    println!("SDP Answer created and sent successfully.");
                                }
                            }
                            peer_connection = Some(pc);
                        }
                    }
                    SignalingMessage::IceCandidate { candidate, .. } => {
                        if let Some(ref pc) = peer_connection {
                            let rtc_cand = RTCIceCandidateInit {
                                candidate: candidate.candidate,
                                sdp_mid: candidate.sdp_mid,
                                sdp_mline_index: candidate.sdp_mline_index,
                                username_fragment: candidate.username_fragment,
                            };
                            let _ = pc.add_ice_candidate(rtc_cand).await;
                        } else {
                            eprintln!("Warning: Received ICE candidate before peer connection was initialized.");
                        }
                    }
                    SignalingMessage::EndSession { .. } => {
                        println!("Stream session ended by remote host.");
                        break;
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
