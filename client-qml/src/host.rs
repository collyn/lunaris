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
    std::thread::spawn(|| {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map(|rt| rt.block_on(agent::direct::local_addresses()))
            .unwrap_or_default()
    })
    .join()
    .unwrap_or_default()
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
    // The agent goes when the app goes, even if the app is killed or crashes
    // (otherwise it keeps the port and "Turn off" can't reach it).
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
    let child = cmd.spawn().map_err(|e| format!("cannot start the agent: {}", e))?;
    *HOST_CHILD.lock().unwrap() = Some(child);
    Ok(())
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
