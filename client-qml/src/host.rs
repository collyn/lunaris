//! "This computer": the agent that lets other computers control this one.
//!
//! The app runs it as `<this program> agent --cli --config <agent config>`, a
//! child process, so a capture/encoder failure can't take the app down. It
//! shares the standalone agent's configuration file (address, password,
//! server), so both are the same computer to the clients.

use std::net::{Ipv4Addr, SocketAddr, TcpStream};
use std::process::{Child, Command, Stdio};
use std::sync::Mutex;
use std::time::Duration;

use agent::pairing::{self, AgentConfig};
use serde::Serialize;

static HOST_CHILD: Mutex<Option<Child>> = Mutex::new(None);

#[derive(Serialize)]
struct HostStatus {
    /// Remote control is turned on (started with the app).
    enabled: bool,
    /// An agent is accepting direct connections on `port` (ours or one
    /// started outside the app).
    running: bool,
    port: u16,
    password: String,
    addresses: Vec<String>,
    hostname: String,
    /// Remote keyboard/mouse can work (Linux: /dev/uinput is writable).
    input_ready: bool,
    error: String,
}

fn load_or_create_config() -> Result<AgentConfig, String> {
    let path = pairing::config_path();
    match pairing::load_config(path) {
        Ok(mut config) => {
            if pairing::ensure_direct_password(&mut config) {
                pairing::save_config(&config, path).map_err(|e| e.to_string())?;
            }
            Ok(config)
        }
        Err(_) if !std::path::Path::new(path).exists() => {
            let config = pairing::new_config();
            pairing::save_config(&config, path).map_err(|e| format!("cannot save {}: {}", path, e))?;
            Ok(config)
        }
        Err(e) => Err(format!("cannot read {}: {}", path, e)),
    }
}

fn save(config: &AgentConfig) -> Result<(), String> {
    pairing::save_config(config, pairing::config_path()).map_err(|e| e.to_string())
}

fn child_alive() -> bool {
    let mut lock = HOST_CHILD.lock().unwrap();
    match lock.as_mut().map(|c| c.try_wait()) {
        Some(Ok(None)) => true,
        Some(_) => {
            *lock = None;
            false
        }
        None => false,
    }
}

fn port_open(port: u16) -> bool {
    TcpStream::connect_timeout(
        &SocketAddr::from((Ipv4Addr::LOCALHOST, port)),
        Duration::from_millis(150),
    )
    .is_ok()
}

/// This computer's LAN/VPN addresses. `local_addresses` is async; run it on a
/// thread of its own (the caller may be inside a tokio runtime).
fn addresses() -> Vec<String> {
    let mut list: Vec<String> = std::thread::spawn(|| {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map(|rt| rt.block_on(agent::direct::local_addresses()))
            .unwrap_or_default()
    })
    .join()
    .unwrap_or_default();
    // The main address first: it's the one other computers most likely use.
    if let Some(primary) = primary_address() {
        if let Some(i) = list.iter().position(|a| *a == primary) {
            let main = list.remove(i);
            list.insert(0, main);
        }
    }
    list
}

fn hostname() -> String {
    hostname::get()
        .ok()
        .and_then(|h| h.into_string().ok())
        .unwrap_or_default()
}

/// Whether the agent can inject keyboard/mouse input. On Linux it needs
/// write access to /dev/uinput (udev rule from the package or --setup-host).
fn input_ready() -> bool {
    #[cfg(target_os = "linux")]
    {
        std::fs::OpenOptions::new().write(true).open("/dev/uinput").is_ok()
    }
    #[cfg(not(target_os = "linux"))]
    {
        true
    }
}

/// Status of remote control as JSON for the UI.
pub fn status_json() -> String {
    let status = match load_or_create_config() {
        Ok(config) => HostStatus {
            enabled: config.direct_enabled,
            running: child_alive() || port_open(config.direct_port),
            port: config.direct_port,
            password: config.direct_password.clone(),
            addresses: addresses(),
            hostname: hostname(),
            input_ready: input_ready(),
            error: String::new(),
        },
        Err(error) => HostStatus {
            enabled: false,
            running: false,
            port: pairing::default_direct_port(),
            password: String::new(),
            addresses: addresses(),
            hostname: hostname(),
            input_ready: input_ready(),
            error,
        },
    };
    serde_json::to_string(&status).unwrap_or_default()
}

/// Starts the agent (when it isn't running yet) and remembers remote control
/// as on.
pub fn start() -> Result<(), String> {
    let mut config = load_or_create_config()?;
    if !config.direct_enabled {
        config.direct_enabled = true;
        save(&config)?;
    }
    if child_alive() {
        return Ok(());
    }
    if port_open(config.direct_port) {
        // Another agent (e.g. the standalone one) already serves this port.
        return Ok(());
    }
    spawn_agent()
}

fn spawn_agent() -> Result<(), String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let mut cmd = Command::new(exe);
    cmd.arg("agent")
        .arg("--cli")
        .arg("--config")
        .arg(pairing::config_path())
        .arg("--name")
        .arg(hostname())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    stop_with_parent(&mut cmd);
    let child = cmd.spawn().map_err(|e| format!("cannot start the agent: {}", e))?;
    *HOST_CHILD.lock().unwrap() = Some(child);
    Ok(())
}

/// Makes a child process stop when the app does, even if the app is killed
/// or crashes (otherwise it would keep its port, out of the app's reach).
/// Linux only; elsewhere the app stops its children when it exits.
pub fn stop_with_parent(cmd: &mut Command) {
    #[cfg(target_os = "linux")]
    unsafe {
        use std::os::unix::process::CommandExt;
        let app_pid = libc::getpid();
        cmd.pre_exec(move || {
            if libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGTERM) != 0 {
                return Err(std::io::Error::last_os_error());
            }
            // The app may already have exited between fork and prctl.
            if libc::getppid() != app_pid {
                libc::_exit(0);
            }
            Ok(())
        });
    }
    #[cfg(not(target_os = "linux"))]
    let _ = cmd;
}

/// Registers this computer with `server_url` (the app's own server) unless
/// it already uses a server; restarts the app's agent to connect.
pub fn use_server_if_none(server_url: &str, token: &str) -> Result<(), String> {
    let mut config = load_or_create_config()?;
    if !config.server_url.trim().is_empty() && !config.server_token.trim().is_empty() {
        return Ok(());
    }
    config.server_url = server_url.to_string();
    config.server_token = token.to_string();
    save(&config)?;
    restart_if_ours()
}

/// The address this computer uses to reach other networks (the interface of
/// the default route), found without sending anything.
pub fn primary_address() -> Option<String> {
    let socket = std::net::UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0)).ok()?;
    socket.connect((Ipv4Addr::new(192, 0, 2, 1), 9)).ok()?;
    match socket.local_addr().ok()?.ip() {
        std::net::IpAddr::V4(ip) if !ip.is_loopback() && !ip.is_unspecified() => Some(ip.to_string()),
        _ => None,
    }
}

/// Stops the agent the app started and remembers remote control as off.
pub fn stop() -> Result<(), String> {
    let mut config = load_or_create_config()?;
    if config.direct_enabled {
        config.direct_enabled = false;
        save(&config)?;
    }
    kill_child();
    Ok(())
}

/// Stops the app's agent when the app exits (remote control stays "on" and
/// starts again with the app).
pub fn shutdown() {
    kill_child();
}

fn kill_child() {
    if let Some(mut child) = HOST_CHILD.lock().unwrap().take() {
        let _ = child.kill();
        let _ = child.wait();
    }
}

/// Starts remote control at app start when it was left on.
pub fn start_if_enabled() {
    if let Ok(config) = load_or_create_config() {
        if config.direct_enabled {
            if let Err(e) = start() {
                tracing::warn!("Remote control: {}", e);
            }
        }
    }
}

/// Sets the access password (`None`: a new random one) and restarts the
/// app's agent so it takes effect.
pub fn set_password(password: Option<String>) -> Result<(), String> {
    let mut config = load_or_create_config()?;
    config.direct_password = match password {
        Some(p) => {
            let p = p.trim().to_string();
            if p.chars().count() < 6 {
                return Err("The password needs at least 6 characters".into());
            }
            p
        }
        None => pairing::generate_direct_password(),
    };
    save(&config)?;
    restart_if_ours()
}

/// Sets the port for direct connections and restarts the app's agent.
pub fn set_port(port: u16) -> Result<(), String> {
    if port < 1024 {
        return Err("Use a port from 1024 to 65535".into());
    }
    let mut config = load_or_create_config()?;
    config.direct_port = port;
    save(&config)?;
    restart_if_ours()
}

fn restart_if_ours() -> Result<(), String> {
    if child_alive() {
        kill_child();
        spawn_agent()?;
    }
    Ok(())
}
