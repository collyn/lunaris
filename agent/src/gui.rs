#![cfg(feature = "gui")]

use crate::pairing::{load_config, save_config as save_config_file, AgentConfig};
use crate::{run_agent_loop, AGENT_ACTIVE, CONNECTED_TO_SERVER, LAST_ERROR};
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
    pub close_to_tray: std::sync::atomic::AtomicBool,
}

#[derive(serde::Serialize, serde::Deserialize, Clone)]
pub struct ConfigResponse {
    pub client_unique_id: String,
    pub server_url: String,
    pub agent_name: String,
    pub server_token: String,
    pub autostart: bool,
    pub close_to_tray: bool,
}

#[derive(serde::Serialize, serde::Deserialize, Clone)]
pub struct UpdateResponse {
    pub latest_version: String,
    pub release_url: String,
}

#[derive(serde::Serialize, Clone)]
pub struct StatusResponse {
    pub agent_active: bool,
    pub connected_to_server: bool,
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
        webtransport_port: 55200,
        autostart: false,
        close_to_tray: false,
    });

    // Extract name from environment or host name
    let agent_name = hostname::get()
        .map(|h| h.to_string_lossy().into_owned())
        .unwrap_or_else(|_| "LunarisHost".to_string());

    Ok(ConfigResponse {
        client_unique_id: config.client_unique_id,
        server_url: config.server_url,
        agent_name,
        server_token: config.server_token,
        autostart: config.autostart,
        close_to_tray: config.close_to_tray,
    })
}

#[tauri::command]
fn save_config(
    state: State<'_, AppState>,
    server_url: String,
    _agent_name: String,
    server_token: String,
    autostart: bool,
    close_to_tray: bool,
) -> Result<(), String> {
    let mut config = load_config("agent_config.json").unwrap_or_else(|_| AgentConfig {
        client_unique_id: uuid::Uuid::new_v4().to_string().to_uppercase(),
        client_private_key: "".to_string(),
        client_certificate: "".to_string(),
        server_certificate: "".to_string(),
        server_url: "ws://127.0.0.1:8080".to_string(),
        server_token: "".to_string(),
        webtransport_port: 55200,
        autostart: false,
        close_to_tray: false,
    });

    config.server_url = server_url;
    config.server_token = server_token;
    config.autostart = autostart;
    config.close_to_tray = close_to_tray;

    // Apply autostart settings to OS
    crate::pairing::set_autostart_enabled_impl(autostart);

    // Sync close_to_tray in AppState
    state.close_to_tray.store(close_to_tray, Ordering::SeqCst);

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

    Ok(())
}

#[tauri::command]
fn get_status() -> Result<StatusResponse, String> {
    let agent_active = AGENT_ACTIVE.load(Ordering::SeqCst);
    let connected_to_server = CONNECTED_TO_SERVER.load(Ordering::SeqCst);

    let last_error = if let Ok(mut err_lock) = LAST_ERROR.lock() {
        err_lock.take()
    } else {
        None
    };

    Ok(StatusResponse {
        agent_active,
        connected_to_server,
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

fn is_newer_version(current: &str, latest: &str) -> bool {
    let current_clean = current.trim_start_matches('v');
    let latest_clean = latest.trim_start_matches('v');
    
    let current_parts: Vec<&str> = current_clean.split('.').collect();
    let latest_parts: Vec<&str> = latest_clean.split('.').collect();
    
    for i in 0..std::cmp::max(current_parts.len(), latest_parts.len()) {
        let current_num = current_parts.get(i).and_then(|s| s.parse::<u32>().ok()).unwrap_or(0);
        let latest_num = latest_parts.get(i).and_then(|s| s.parse::<u32>().ok()).unwrap_or(0);
        
        if latest_num > current_num {
            return true;
        } else if current_num > latest_num {
            return false;
        }
    }
    false
}

#[tauri::command]
async fn check_for_updates() -> Result<Option<UpdateResponse>, String> {
    let client = reqwest::Client::builder()
        .user_agent("lunaris-agent")
        .build()
        .map_err(|e| e.to_string())?;
        
    let res = client.get("https://api.github.com/repos/collyn/lunaris/releases/latest")
        .send()
        .await
        .map_err(|e| e.to_string())?;
        
    if !res.status().is_success() {
        return Ok(None);
    }
    
    #[derive(serde::Deserialize)]
    struct GithubRelease {
        tag_name: String,
        html_url: String,
    }
    
    let release: GithubRelease = res.json().await.map_err(|e| e.to_string())?;
    
    let current_version = env!("CARGO_PKG_VERSION");
    if is_newer_version(current_version, &release.tag_name) {
        Ok(Some(UpdateResponse {
            latest_version: release.tag_name,
            release_url: release.html_url,
        }))
    } else {
        Ok(None)
    }
}

#[tauri::command]
fn open_url(url: String) -> Result<(), String> {
    #[cfg(target_os = "windows")]
    let _ = std::process::Command::new("cmd").args(&["/C", "start", &url]).status();
    #[cfg(target_os = "macos")]
    let _ = std::process::Command::new("open").arg(&url).status();
    #[cfg(target_os = "linux")]
    let _ = std::process::Command::new("xdg-open").arg(&url).status();
    Ok(())
}

// -------------------------------------------------------------------------
// GUI Entrypoint
// -------------------------------------------------------------------------
pub fn run_gui(minimized: bool) {
    let config = load_config("agent_config.json").unwrap_or_else(|_| AgentConfig {
        client_unique_id: "".to_string(),
        client_private_key: "".to_string(),
        client_certificate: "".to_string(),
        server_certificate: "".to_string(),
        server_url: "ws://127.0.0.1:8080".to_string(),
        server_token: "".to_string(),
        webtransport_port: 55200,
        autostart: false,
        close_to_tray: false,
    });

    let close_to_tray_val = config.close_to_tray;

    tauri::Builder::default()
        .manage(AppState {
            shutdown_tx: Mutex::new(None),
            close_to_tray: std::sync::atomic::AtomicBool::new(close_to_tray_val),
        })
        .setup(move |app| {
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

            // If not starting minimized, show the window
            if !minimized {
                if let Some(window) = app.get_webview_window("main") {
                    let _ = window.show();
                }
            }

            Ok(())
        })
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                let state = window.state::<AppState>();
                if state.close_to_tray.load(Ordering::SeqCst) {
                    api.prevent_close();
                    let _ = window.hide();
                }
            }
        })
        .invoke_handler(tauri::generate_handler![
            get_config,
            save_config,
            import_config,
            start_agent,
            stop_agent,
            get_status,
            clear_last_error,
            check_for_updates,
            open_url
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
