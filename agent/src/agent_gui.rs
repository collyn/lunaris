#![cfg(feature = "gui")]

use std::pin::Pin;
use std::sync::atomic::Ordering;

use cxx_qt::CxxQtType;
use cxx_qt_lib::{QGuiApplication, QQmlApplicationEngine, QString, QUrl};

use crate::pairing::{load_config, save_config as save_config_file, AgentConfig, import_config_file};
use crate::{run_agent_loop, AGENT_ACTIVE, CONNECTED_TO_SERVER, LAST_ERROR};

// ---------------------------------------------------------------------------
// Log channel (identical to the old gui.rs — consumed by main.rs)
// ---------------------------------------------------------------------------
pub static LOG_CHANNEL: once_cell::sync::Lazy<(
    std::sync::Mutex<std::sync::mpsc::Sender<String>>,
    std::sync::Mutex<std::sync::mpsc::Receiver<String>>,
)> = once_cell::sync::Lazy::new(|| {
    let (tx, rx) = std::sync::mpsc::channel();
    (std::sync::Mutex::new(tx), std::sync::Mutex::new(rx))
});

pub struct ChannelWriter;

impl std::io::Write for ChannelWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        if let Ok(s) = std::str::from_utf8(buf) {
            if let Ok(tx) = LOG_CHANNEL.0.lock() {
                let _ = tx.send(s.to_string());
            }
        }
        let _ = std::io::stdout().write(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        std::io::stdout().flush()
    }
}

pub struct ChannelMakeWriter;

impl<'a> tracing_subscriber::fmt::writer::MakeWriter<'a> for ChannelMakeWriter {
    type Writer = ChannelWriter;
    fn make_writer(&self) -> Self::Writer {
        ChannelWriter
    }
}

// ---------------------------------------------------------------------------
// Pending event queues (cross-thread communication from tokio tasks to QML)
// ---------------------------------------------------------------------------
pub static PENDING_LOGS: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());

pub(crate) struct UpdateInfo {
    version: String,
    url: String,
}
pub(crate) static PENDING_UPDATE: std::sync::Mutex<Option<UpdateInfo>> = std::sync::Mutex::new(None);

// ---------------------------------------------------------------------------
// cxx-qt bridge
// ---------------------------------------------------------------------------
#[cxx_qt::bridge]
pub mod qobject {
    unsafe extern "C++" {
        include!("cxx-qt-lib/qstring.h");
        include!("agent_gui.h");
        type QString = cxx_qt_lib::QString;

        #[rust_name = "open_url_helper"]
        unsafe fn open_url_helper(url: &QString);
        #[rust_name = "pick_import_file"]
        unsafe fn pick_import_file() -> QString;
    }

    unsafe extern "RustQt" {
        #[qobject]
        #[qml_element]
        type AgentBridge = super::AgentBridgeRust;

        // ---- Signals (Rust → QML) ----
        #[qsignal]
        fn status_changed(self: Pin<&mut AgentBridge>, agent_active: bool, connected: bool);

        #[qsignal]
        fn log_message(self: Pin<&mut AgentBridge>, message: QString);

        #[qsignal]
        fn config_loaded(
            self: Pin<&mut AgentBridge>,
            server_url: QString,
            server_token: QString,
            agent_name: QString,
            client_unique_id: QString,
            autostart: bool,
            close_to_tray: bool,
        );

        #[qsignal]
        fn update_available(
            self: Pin<&mut AgentBridge>,
            latest_version: QString,
            release_url: QString,
        );

        #[qsignal]
        fn config_saved(self: Pin<&mut AgentBridge>, success: bool, error_msg: QString);

        #[qsignal]
        fn import_completed(self: Pin<&mut AgentBridge>, success: bool, error_msg: QString);

        // ---- Invokables (QML → Rust) ----
        #[qinvokable]
        fn load_config(self: Pin<&mut AgentBridge>);

        #[qinvokable]
        fn save_config(
            self: Pin<&mut AgentBridge>,
            server_url: QString,
            server_token: QString,
            autostart: bool,
            close_to_tray: bool,
        );

        #[qinvokable]
        fn import_config(self: Pin<&mut AgentBridge>);

        #[qinvokable]
        fn start_agent(self: Pin<&mut AgentBridge>);

        #[qinvokable]
        fn stop_agent(self: Pin<&mut AgentBridge>);

        #[qinvokable]
        fn poll_status(self: Pin<&mut AgentBridge>);

        #[qinvokable]
        fn poll_logs(self: Pin<&mut AgentBridge>);

        #[qinvokable]
        fn clear_logs(self: Pin<&mut AgentBridge>);

        #[qinvokable]
        fn check_for_updates(self: Pin<&mut AgentBridge>);

        #[qinvokable]
        fn open_url(self: Pin<&mut AgentBridge>, url: QString);

        #[qinvokable]
        fn launch_native_client(
            self: Pin<&mut AgentBridge>,
            host_id: QString,
            server_url: QString,
            token: QString,
            res: QString,
            fps: QString,
            bitrate: QString,
            codec: QString,
            app_id: i32,
            mouse_queue_limit: QString,
            host_name: QString,
            encoder: QString,
            display_id: QString,
            virtual_display: bool,
            input_protocol: QString,
        );

        #[qinvokable]
        fn is_agent_active(self: &AgentBridge) -> bool;
    }
}

// ---------------------------------------------------------------------------
// Rust backing struct
// ---------------------------------------------------------------------------
#[derive(Default)]
pub struct AgentBridgeRust {
    shutdown_tx: std::sync::Mutex<Option<tokio::sync::oneshot::Sender<()>>>,
    tokio_runtime: Option<tokio::runtime::Runtime>,
}

impl AgentBridgeRust {
    fn get_or_init_runtime(&mut self) -> &tokio::runtime::Runtime {
        if self.tokio_runtime.is_none() {
            self.tokio_runtime = Some(
                tokio::runtime::Runtime::new()
                    .expect("Failed to create tokio runtime for agent"),
            );
        }
        self.tokio_runtime.as_ref().unwrap()
    }
}

// ---------------------------------------------------------------------------
// Helper: build config from default + common fields
// ---------------------------------------------------------------------------
fn make_default_config() -> AgentConfig {
    AgentConfig {
        client_unique_id: uuid::Uuid::new_v4().to_string().to_uppercase(),
        client_private_key: String::new(),
        client_certificate: String::new(),
        server_certificate: String::new(),
        server_url: "ws://127.0.0.1:8080".to_string(),
        server_token: String::new(),
        webtransport_port: 55200,
        autostart: false,
        close_to_tray: false,
        virtual_display_output: None,
    }
}

fn agent_hostname() -> String {
    hostname::get()
        .map(|h| h.to_string_lossy().into_owned())
        .unwrap_or_else(|_| "LunarisHost".to_string())
}

// ---------------------------------------------------------------------------
// Invokable implementations
// ---------------------------------------------------------------------------
impl qobject::AgentBridge {
    pub fn load_config(mut self: Pin<&mut Self>) {
        let config = load_config("agent_config.json").unwrap_or_else(|_| make_default_config());
        let name = agent_hostname();
        self.as_mut().config_loaded(
            QString::from(&config.server_url),
            QString::from(&config.server_token),
            QString::from(&name),
            QString::from(&config.client_unique_id),
            config.autostart,
            config.close_to_tray,
        );
    }

    pub fn save_config(
        mut self: Pin<&mut Self>,
        server_url: QString,
        server_token: QString,
        autostart: bool,
        close_to_tray: bool,
    ) {
        let mut config = load_config("agent_config.json").unwrap_or_else(|_| make_default_config());
        config.server_url = server_url.to_string();
        config.server_token = server_token.to_string();
        config.autostart = autostart;
        config.close_to_tray = close_to_tray;

        crate::pairing::set_autostart_enabled_impl(autostart);

        match save_config_file(&config, "agent_config.json") {
            Ok(()) => self.as_mut().config_saved(true, QString::from("")),
            Err(e) => self.as_mut().config_saved(
                false,
                QString::from(&format!("Failed to save config: {}", e)),
            ),
        }
    }

    pub fn import_config(mut self: Pin<&mut Self>) {
        let path = unsafe { qobject::pick_import_file() };
        if path.is_empty() {
            self.as_mut().import_completed(false, QString::from("Cancelled"));
            return;
        }
        let path_str = path.to_string();
        match import_config_file(&path_str, "agent_config.json") {
            Ok(()) => {
                log::info!("Successfully imported configuration from {:?}", path);
                self.as_mut().import_completed(true, QString::from(""));
            }
            Err(e) => {
                self.as_mut().import_completed(
                    false,
                    QString::from(&format!("Failed to import config: {}", e)),
                );
            }
        }
    }

    pub fn start_agent(self: Pin<&mut Self>) {
        if AGENT_ACTIVE.load(Ordering::SeqCst) {
            return;
        }

        let config = match load_config("agent_config.json") {
            Ok(c) => c,
            Err(e) => {
                log::error!("Failed to load config: {:?}", e);
                return;
            }
        };

        let name = agent_hostname();

        let (tx, rx) = tokio::sync::oneshot::channel::<()>();
        let mut rust_obj = self.rust_mut();
        *rust_obj.shutdown_tx.lock().unwrap() = Some(tx);

        AGENT_ACTIVE.store(true, Ordering::SeqCst);
        log::info!("Starting Host Agent loop from Qt GUI...");

        let rt = rust_obj.get_or_init_runtime();
        rt.spawn(async move {
            tokio::select! {
                _ = rx => {
                    log::info!("Host Agent loop stopped via GUI command.");
                }
                res = run_agent_loop(
                    config,
                    name,
                    "agent_config.json".to_string(),
                ) => {
                    if let Err(e) = res {
                        let err_str = format!("Agent loop error: {:?}", e);
                        log::error!("{}", err_str);
                        if let Ok(mut err_lock) = LAST_ERROR.lock() {
                            *err_lock = Some(err_str);
                        }
                    }
                }
            }
            AGENT_ACTIVE.store(false, Ordering::SeqCst);
            CONNECTED_TO_SERVER.store(false, Ordering::SeqCst);
        });
    }

    pub fn stop_agent(self: Pin<&mut Self>) {
        if !AGENT_ACTIVE.load(Ordering::SeqCst) {
            return;
        }
        let rust_obj = self.rust_mut();
        if let Ok(mut lock) = rust_obj.shutdown_tx.lock() {
            if let Some(tx) = lock.take() {
                let _ = tx.send(());
            }
        }
        AGENT_ACTIVE.store(false, Ordering::SeqCst);
        CONNECTED_TO_SERVER.store(false, Ordering::SeqCst);
    }

    pub fn poll_status(mut self: Pin<&mut Self>) {
        let active = AGENT_ACTIVE.load(Ordering::SeqCst);
        let connected = CONNECTED_TO_SERVER.load(Ordering::SeqCst);
        self.as_mut().status_changed(active, connected);

        if let Ok(mut err_lock) = LAST_ERROR.lock() {
            if let Some(err) = err_lock.take() {
                self.as_mut()
                    .log_message(QString::from(&format!("[ERROR] {}", err)));
            }
        }
    }

    pub fn poll_logs(mut self: Pin<&mut Self>) {
        if let Ok(mut logs) = PENDING_LOGS.lock() {
            let drained: Vec<String> = logs.drain(..).collect();
            for msg in drained {
                self.as_mut().log_message(QString::from(&msg));
            }
        }
        // Check for pending update result
        if let Ok(mut opt) = PENDING_UPDATE.lock() {
            if let Some(info) = opt.take() {
                self.as_mut().update_available(
                    QString::from(&info.version),
                    QString::from(&info.url),
                );
            }
        }
    }

    pub fn clear_logs(self: Pin<&mut Self>) {
        if let Ok(mut logs) = PENDING_LOGS.lock() {
            logs.clear();
        }
    }

    pub fn check_for_updates(self: Pin<&mut Self>) {
        let mut rust_obj = self.rust_mut();
        let rt = rust_obj.get_or_init_runtime();
        rt.spawn(async move {
            let client = match reqwest::Client::builder()
                .user_agent("lunaris-agent")
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
                Ok(r) => r,
                Err(_) => return,
            };
            let current = env!("CARGO_PKG_VERSION");
            // Simple semver check
            let current_clean = current.trim_start_matches('v');
            let latest_clean = release.tag_name.trim_start_matches('v');
            if latest_clean != current_clean {
                let newer = compare_versions(latest_clean, current_clean);
                if newer {
                    if let Ok(mut opt) = PENDING_UPDATE.lock() {
                        *opt = Some(UpdateInfo {
                            version: release.tag_name,
                            url: release.html_url,
                        });
                    }
                }
            }
        });
    }

    pub fn open_url(self: Pin<&mut Self>, url: QString) {
        unsafe { qobject::open_url_helper(&url); }
    }

    #[allow(clippy::too_many_arguments)]
    pub fn launch_native_client(
        self: Pin<&mut Self>,
        host_id: QString,
        server_url: QString,
        token: QString,
        res: QString,
        fps: QString,
        bitrate: QString,
        codec: QString,
        app_id: i32,
        mouse_queue_limit: QString,
        host_name: QString,
        encoder: QString,
        display_id: QString,
        virtual_display: bool,
        input_protocol: QString,
    ) {
        let ws_server = {
            let s = server_url.to_string();
            if s.starts_with("https://") {
                s.replacen("https://", "wss://", 1)
            } else if s.starts_with("http://") {
                s.replacen("http://", "ws://", 1)
            } else {
                s
            }
        };
        let encoder_val = if encoder.to_string().is_empty() {
            "auto".to_string()
        } else {
            encoder.to_string()
        };
        let display_val = if display_id.to_string().is_empty() {
            "default".to_string()
        } else {
            display_id.to_string()
        };
        let input_val = if input_protocol.to_string().is_empty() {
            "webrtc".to_string()
        } else {
            input_protocol.to_string()
        };

        let url = format!(
            "lunaris://connect?host_id={}&server={}&token={}&res={}&fps={}&bitrate={}&codec={}&mouse_queue_limit={}&host_name={}&app_id={}&encoder={}&display={}&virtual_display={}&input_protocol={}",
            urlencoding::encode(&host_id.to_string()),
            urlencoding::encode(&ws_server),
            urlencoding::encode(&token.to_string()),
            urlencoding::encode(&res.to_string()),
            urlencoding::encode(&fps.to_string()),
            urlencoding::encode(&bitrate.to_string()),
            urlencoding::encode(&codec.to_string()),
            urlencoding::encode(&mouse_queue_limit.to_string()),
            urlencoding::encode(&host_name.to_string()),
            app_id,
            urlencoding::encode(&encoder_val),
            urlencoding::encode(&display_val),
            virtual_display,
            urlencoding::encode(&input_val),
        );
        let s = url.to_string();
        let qs = QString::from(&s);
        unsafe { qobject::open_url_helper(&qs); }
    }

    pub fn is_agent_active(&self) -> bool {
        AGENT_ACTIVE.load(Ordering::SeqCst)
    }
}

// ---------------------------------------------------------------------------
// GUI entry point
// ---------------------------------------------------------------------------
pub fn run_gui(minimized: bool) {
    // Spawn background thread that reads from LOG_CHANNEL and pushes to PENDING_LOGS
    std::thread::spawn(move || {
        if let Ok(rx) = LOG_CHANNEL.1.lock() {
            while let Ok(msg) = rx.recv() {
                if let Ok(mut logs) = PENDING_LOGS.lock() {
                    logs.push(msg.trim_end().to_string());
                    // Keep max 500 lines (match old JS behavior)
                    if logs.len() > 500 {
                        logs.remove(0);
                    }
                }
            }
        }
    });

    // Create Qt application and load QML
    let mut app = QGuiApplication::new();
    let mut engine = QQmlApplicationEngine::new();

    if let Some(engine_mut) = engine.as_mut() {
        let qurl = QUrl::from("qrc:/AgentWindow.qml");
        engine_mut.load(&qurl);
    }

    if let Some(app_mut) = app.as_mut() {
        let _ = minimized; // QML handles initial visibility
        app_mut.exec();
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------
fn compare_versions(latest: &str, current: &str) -> bool {
    let latest_parts: Vec<&str> = latest.split('.').collect();
    let current_parts: Vec<&str> = current.split('.').collect();
    for i in 0..std::cmp::max(latest_parts.len(), current_parts.len()) {
        let ln = latest_parts.get(i).and_then(|s| s.parse::<u32>().ok()).unwrap_or(0);
        let cn = current_parts.get(i).and_then(|s| s.parse::<u32>().ok()).unwrap_or(0);
        if ln > cn {
            return true;
        } else if cn > ln {
            return false;
        }
    }
    false
}
