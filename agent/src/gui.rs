#![cfg(feature = "gui")]

use crate::pairing::{load_config, save_config as save_config_file, AgentConfig};
use crate::{run_agent_loop, AGENT_ACTIVE, CONNECTED_TO_SERVER, LAST_ERROR, SUNSHINE_PID};
use std::sync::atomic::Ordering;
use std::sync::Mutex;
use tauri::{
    menu::{Menu, MenuItem},
    tray::{MouseButton, TrayIconBuilder, TrayIconEvent},
    Emitter, Manager, State,
};
use tracing::{error, info};

// Shared state for Tauri
pub struct AppState {
    pub shutdown_tx: Mutex<Option<tokio::sync::oneshot::Sender<()>>>,
}

#[derive(serde::Serialize, serde::Deserialize, Clone)]
pub struct ConfigResponse {
    pub client_unique_id: String,
    pub server_url: String,
    pub agent_name: String,
    pub no_auto_start_sunshine: bool,
    pub server_token: String,
}

#[derive(serde::Serialize, Clone)]
pub struct StatusResponse {
    pub agent_active: bool,
    pub connected_to_server: bool,
    pub sunshine_running: bool,
    pub sunshine_pid: u32,
    pub last_error: Option<String>,
}

// -------------------------------------------------------------------------
// Thread-Safe Log Channel for UI Streaming
// -------------------------------------------------------------------------
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

// -------------------------------------------------------------------------
// Tauri Commands
// -------------------------------------------------------------------------

#[tauri::command]
fn get_config() -> Result<ConfigResponse, String> {
    let config = load_config("agent_config.json").unwrap_or_else(|_| AgentConfig {
        client_unique_id: "N/A".to_string(),
        client_private_key: "".to_string(),
        client_certificate: "".to_string(),
        server_certificate: "".to_string(),
        server_url: "ws://127.0.0.1:8080".to_string(),
        server_token: "".to_string(),
    });

    // Extract name from environment or host name
    let agent_name = hostname::get()
        .map(|h| h.to_string_lossy().into_owned())
        .unwrap_or_else(|_| "LunarisHost".to_string());

    Ok(ConfigResponse {
        client_unique_id: config.client_unique_id,
        server_url: config.server_url,
        agent_name,
        no_auto_start_sunshine: false, // Default
        server_token: config.server_token,
    })
}

#[tauri::command]
fn save_config(
    server_url: String,
    _agent_name: String,
    _no_auto_start_sunshine: bool,
    server_token: String,
) -> Result<(), String> {
    let mut config = load_config("agent_config.json").unwrap_or_else(|_| AgentConfig {
        client_unique_id: uuid::Uuid::new_v4().to_string().to_uppercase(),
        client_private_key: "".to_string(),
        client_certificate: "".to_string(),
        server_certificate: "".to_string(),
        server_url: "ws://127.0.0.1:8080".to_string(),
        server_token: "".to_string(),
    });

    config.server_url = server_url;
    config.server_token = server_token;
    // We can save other fields or settings in agent_config.json if desired.
    if let Err(e) = save_config_file(&config, "agent_config.json") {
        return Err(format!("Failed to save config: {}", e));
    }
    Ok(())
}

#[tauri::command]
fn import_config() -> Result<bool, String> {
    // Open native file dialog
    let file_path = rfd::FileDialog::new()
        .set_title("Select agent_config.json")
        .add_filter("JSON Config", &["json"])
        .pick_file();

    if let Some(path) = file_path {
        let path_str = path.to_string_lossy();
        crate::pairing::import_config_file(&path_str, "agent_config.json")
            .map_err(|e| format!("Failed to import config: {}", e))?;

        info!("Successfully imported configuration from {:?}", path);
        Ok(true)
    } else {
        Ok(false)
    }
}

#[tauri::command]
fn start_agent(state: State<'_, AppState>) -> Result<(), String> {
    if AGENT_ACTIVE.load(Ordering::SeqCst) {
        return Ok(());
    }

    let config = match load_config("agent_config.json") {
        Ok(c) => c,
        Err(e) => {
            let err_msg = format!("Failed to load config: {:?}", e);
            error!("{}", err_msg);
            return Err(err_msg);
        }
    };

    let name = hostname::get()
        .map(|h| h.to_string_lossy().into_owned())
        .unwrap_or_else(|_| "LunarisHost".to_string());

    let (tx, rx) = tokio::sync::oneshot::channel::<()>();
    if let Ok(mut lock) = state.shutdown_tx.lock() {
        *lock = Some(tx);
    }

    AGENT_ACTIVE.store(true, Ordering::SeqCst);
    info!("Starting Host Agent loop from GUI context...");

    // Run the agent loop in a separate thread/task
    tokio::spawn(async move {
        let agent_future = run_agent_loop(
            config,
            name,
            "127.0.0.1".to_string(),
            47989,
            false, // Default
            "sunshine".to_string(),
            "agent_config.json".to_string(),
        );

        tokio::select! {
            _ = rx => {
                info!("Host Agent loop stopped via GUI command.");
            }
            res = agent_future => {
                if let Err(e) = res {
                    let err_str = format!("Agent loop error: {:?}", e);
                    error!("{}", err_str);
                    if let Ok(mut err_lock) = LAST_ERROR.lock() {
                        *err_lock = Some(err_str);
                    }
                }
            }
        }
        AGENT_ACTIVE.store(false, Ordering::SeqCst);
        CONNECTED_TO_SERVER.store(false, Ordering::SeqCst);
        SUNSHINE_PID.store(0, Ordering::SeqCst);
    });

    Ok(())
}

#[tauri::command]
fn stop_agent(state: State<'_, AppState>) -> Result<(), String> {
    if !AGENT_ACTIVE.load(Ordering::SeqCst) {
        return Ok(());
    }

    if let Ok(mut lock) = state.shutdown_tx.lock() {
        if let Some(tx) = lock.take() {
            let _ = tx.send(());
        }
    }

    AGENT_ACTIVE.store(false, Ordering::SeqCst);
    CONNECTED_TO_SERVER.store(false, Ordering::SeqCst);
    SUNSHINE_PID.store(0, Ordering::SeqCst);

    Ok(())
}

#[tauri::command]
fn get_status() -> Result<StatusResponse, String> {
    let agent_active = AGENT_ACTIVE.load(Ordering::SeqCst);
    let connected_to_server = CONNECTED_TO_SERVER.load(Ordering::SeqCst);
    let sunshine_pid = SUNSHINE_PID.load(Ordering::SeqCst);

    // Check if Sunshine is running on localhost port 47989
    let sunshine_running =
        sunshine_pid > 0 || std::net::TcpStream::connect("127.0.0.1:47989").is_ok();

    let last_error = if let Ok(mut err_lock) = LAST_ERROR.lock() {
        err_lock.take()
    } else {
        None
    };

    Ok(StatusResponse {
        agent_active,
        connected_to_server,
        sunshine_running,
        sunshine_pid,
        last_error,
    })
}

#[tauri::command]
fn clear_last_error() -> Result<(), String> {
    if let Ok(mut err_lock) = LAST_ERROR.lock() {
        *err_lock = None;
    }
    Ok(())
}

// -------------------------------------------------------------------------
// GUI Entrypoint
// -------------------------------------------------------------------------
pub fn run_gui() {
    tauri::Builder::default()
        .manage(AppState {
            shutdown_tx: Mutex::new(None),
        })
        .setup(|app| {
            // Setup log listener that pipes std::sync::mpsc logs to UI
            let app_handle = app.handle().clone();
            std::thread::spawn(move || {
                if let Ok(rx) = LOG_CHANNEL.1.lock() {
                    while let Ok(msg) = rx.recv() {
                        let _ = app_handle.emit("log-message", msg.trim_end().to_string());
                    }
                }
            });

            // Create System Tray Icon
            let quit_i = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;
            let show_i = MenuItem::with_id(app, "show", "Show Dashboard", true, None::<&str>)?;
            let menu = Menu::with_items(app, &[&show_i, &quit_i])?;

            let _tray = TrayIconBuilder::new()
                .menu(&menu)
                .show_menu_on_left_click(false)
                .icon(app.default_window_icon().unwrap().clone())
                .on_tray_icon_event(|tray, event| {
                    if let TrayIconEvent::Click {
                        button: MouseButton::Left,
                        ..
                    } = event
                    {
                        let app = tray.app_handle();
                        if let Some(window) = app.get_webview_window("main") {
                            if window.is_visible().unwrap_or(false) {
                                let _ = window.hide();
                            } else {
                                let _ = window.show();
                                let _ = window.set_focus();
                            }
                        }
                    }
                })
                .on_menu_event(|app, event| match event.id.as_ref() {
                    "quit" => {
                        app.exit(0);
                    }
                    "show" => {
                        if let Some(window) = app.get_webview_window("main") {
                            let _ = window.show();
                            let _ = window.set_focus();
                        }
                    }
                    _ => {}
                })
                .build(app)?;

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            get_config,
            save_config,
            import_config,
            start_agent,
            stop_agent,
            get_status,
            clear_last_error
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
