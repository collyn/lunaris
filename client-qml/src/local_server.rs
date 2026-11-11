//! "Lunaris server on this computer": the app runs the server itself
//! (`<this program> server run`), like the agent (see host.rs), with its data
//! (database, agent token, initial admin password) in the per-user data
//! directory. Other computers then sign in at http://<this computer>:<port>
//! and the web console opens at the same address.

use std::net::{Ipv4Addr, SocketAddr, TcpStream};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::Mutex;
use std::time::Duration;

use serde::{Deserialize, Serialize};

static SERVER_CHILD: Mutex<Option<Child>> = Mutex::new(None);

const DEFAULT_PORT: u16 = 8090;

/// What the app remembers about its server (~/.lunaris/local_server.json).
#[derive(Serialize, Deserialize)]
struct Settings {
    enabled: bool,
    port: u16,
}

impl Default for Settings {
    fn default() -> Self {
        Self { enabled: false, port: DEFAULT_PORT }
    }
}

#[derive(Serialize)]
struct ServerStatus {
    enabled: bool,
    /// Something answers on `port` (our server, or one started elsewhere).
    running: bool,
    /// The app started it (so it can stop it).
    ours: bool,
    port: u16,
    /// For this computer's browser and the app's own sign-in.
    local_url: String,
    /// For other computers (this computer's main address).
    lan_url: String,
    /// The first admin password, while the server still has it on file.
    admin_password: String,
    data_dir: String,
    error: String,
}

fn settings_path() -> PathBuf {
    let home = std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    let dir = home.join(".lunaris");
    let _ = std::fs::create_dir_all(&dir);
    dir.join("local_server.json")
}

fn load_settings() -> Settings {
    std::fs::read_to_string(settings_path())
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

fn save_settings(settings: &Settings) -> Result<(), String> {
    let json = serde_json::to_string_pretty(settings).map_err(|e| e.to_string())?;
    std::fs::write(settings_path(), json).map_err(|e| e.to_string())
}

/// Database, agent token and admin password of the app's server.
pub fn data_dir() -> PathBuf {
    let dir = dirs::data_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("lunaris-server");
    let _ = std::fs::create_dir_all(&dir);
    dir
}

fn read_trimmed(path: PathBuf) -> String {
    std::fs::read_to_string(path).map(|s| s.trim().to_string()).unwrap_or_default()
}

fn port_open(port: u16) -> bool {
    TcpStream::connect_timeout(
        &SocketAddr::from((Ipv4Addr::LOCALHOST, port)),
        Duration::from_millis(150),
    )
    .is_ok()
}

fn child_alive() -> bool {
    let mut lock = SERVER_CHILD.lock().unwrap();
    match lock.as_mut().map(|c| c.try_wait()) {
        Some(Ok(None)) => true,
        Some(_) => {
            *lock = None;
            false
        }
        None => false,
    }
}

pub fn status_json() -> String {
    let settings = load_settings();
    let ours = child_alive();
    let dir = data_dir();
    let lan = crate::host::primary_address().unwrap_or_else(|| "127.0.0.1".into());
    let status = ServerStatus {
        enabled: settings.enabled,
        running: ours || port_open(settings.port),
        ours,
        port: settings.port,
        local_url: format!("http://127.0.0.1:{}", settings.port),
        lan_url: format!("http://{}:{}", lan, settings.port),
        admin_password: read_trimmed(dir.join("initial_admin_password.txt")),
        data_dir: dir.to_string_lossy().into_owned(),
        error: String::new(),
    };
    serde_json::to_string(&status).unwrap_or_default()
}

/// Starts the server (unless something already serves the port) and
/// remembers it as on.
pub fn start() -> Result<(), String> {
    let mut settings = load_settings();
    if !settings.enabled {
        settings.enabled = true;
        save_settings(&settings)?;
    }
    if child_alive() {
        return Ok(());
    }
    if port_open(settings.port) {
        return Err(format!(
            "Port {} is already in use (another Lunaris server?). Choose another port in Settings > Network.",
            settings.port
        ));
    }
    spawn(settings.port)?;
    // This computer's agent registers with its own server (when it has no
    // server yet) once the server has written its agent token.
    std::thread::spawn(move || connect_local_agent(settings.port));
    Ok(())
}

fn spawn(port: u16) -> Result<(), String> {
    let dir = data_dir();
    let log = std::fs::File::create(dir.join("server.log")).map_err(|e| e.to_string())?;
    let log_err = log.try_clone().map_err(|e| e.to_string())?;
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let mut cmd = Command::new(exe);
    cmd.arg("server")
        .arg("run")
        .env("SERVER_PORT", port.to_string())
        .env("LUNARIS_DATA_DIR", &dir)
        .current_dir(&dir)
        .stdin(Stdio::null())
        .stdout(log)
        .stderr(log_err);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    crate::host::stop_with_parent(&mut cmd);
    let child = cmd.spawn().map_err(|e| format!("cannot start the server: {}", e))?;
    *SERVER_CHILD.lock().unwrap() = Some(child);
    Ok(())
}

/// Points this computer's agent at the app's server when it has none.
fn connect_local_agent(port: u16) {
    let token_file = data_dir().join("server_token.txt");
    for _ in 0..50 {
        let token = read_trimmed(token_file.clone());
        if !token.is_empty() {
            if let Err(e) = crate::host::use_server_if_none(&format!("ws://127.0.0.1:{}", port), &token) {
                tracing::warn!("Could not register this computer with its server: {}", e);
            }
            return;
        }
        std::thread::sleep(Duration::from_millis(200));
    }
}

/// Stops the app's server and remembers it as off.
pub fn stop() -> Result<(), String> {
    let mut settings = load_settings();
    if settings.enabled {
        settings.enabled = false;
        save_settings(&settings)?;
    }
    kill_child();
    Ok(())
}

pub fn set_port(port: u16) -> Result<(), String> {
    if port < 1024 {
        return Err("Use a port from 1024 to 65535".into());
    }
    let mut settings = load_settings();
    settings.port = port;
    save_settings(&settings)?;
    if child_alive() {
        kill_child();
        spawn(port)?;
    }
    Ok(())
}

fn kill_child() {
    if let Some(mut child) = SERVER_CHILD.lock().unwrap().take() {
        let _ = child.kill();
        let _ = child.wait();
    }
}

pub fn start_if_enabled() {
    if load_settings().enabled {
        if let Err(e) = start() {
            tracing::warn!("Lunaris server: {}", e);
        }
    }
}

pub fn shutdown() {
    kill_child();
}
