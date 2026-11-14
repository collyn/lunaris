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
    /// Clients may share the clipboard / send files to this computer.
    allow_clipboard: bool,
    allow_file_transfer: bool,
    /// "password", "click" or "password-click" (agent/src/approval.rs).
    approve_mode: String,
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
            allow_clipboard: config.allow_clipboard,
            allow_file_transfer: config.allow_file_transfer,
            approve_mode: config.approve_mode.clone(),
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
            allow_clipboard: true,
            allow_file_transfer: true,
            approve_mode: "password-click".into(),
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

/// The log of the agent this app starts: `agent.log` next to its
/// configuration, the previous run's kept as `agent.log.1`. (It used to be
/// thrown away, which left nothing to look at after a bad session.)
pub fn agent_log_path() -> std::path::PathBuf {
    std::path::Path::new(pairing::config_path()).with_file_name("agent.log")
}

fn agent_log_file() -> Option<std::fs::File> {
    let path = agent_log_path();
    if path.exists() {
        let mut previous = path.clone().into_os_string();
        previous.push(".1");
        let _ = std::fs::rename(&path, previous);
    }
    std::fs::File::create(&path).ok()
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
        // Questions to accept a connection come on stdout, answers go on
        // stdin (agent/src/approval.rs); the logs go to stderr, which is
        // kept in agent.log next to the configuration.
        .arg("--approval-stdio")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(agent_log_file().map(Stdio::from).unwrap_or_else(Stdio::null));
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    stop_with_parent(&mut cmd);
    let mut child = spawn_child(cmd).map_err(|e| format!("cannot start the agent: {}", e))?;
    APPROVALS.lock().unwrap().clear();
    *AGENT_STDIN.lock().unwrap() = child.stdin.take();
    if let Some(stdout) = child.stdout.take() {
        std::thread::Builder::new()
            .name("agent-approvals".into())
            .spawn(move || read_approvals(stdout))
            .ok();
    }
    *HOST_CHILD.lock().unwrap() = Some(child);
    Ok(())
}

// ---- Connections that the person at the other computer accepts ----

/// `&name=…` for a client connection: who connects, as the person at the
/// remote computer is asked about (agents ignore it with the password).
pub fn client_name_query() -> String {
    let user = std::env::var("USER").or_else(|_| std::env::var("USERNAME")).unwrap_or_default();
    let who = if user.is_empty() { hostname() } else { format!("{}@{}", user, hostname()) };
    format!("&name={}", url::form_urlencoded::byte_serialize(who.as_bytes()).collect::<String>())
}

/// What to tell the user for an `AccessRequest` status: None while the
/// connection may go on ("waiting", "accepted").
pub fn access_refusal(status: &str) -> Option<String> {
    match status {
        "waiting" | "accepted" => None,
        "denied" => Some("The person at the remote computer declined the connection.".into()),
        "timeout" => Some("Nobody accepted the connection at the remote computer in time.".into()),
        "unattended" => Some("Nobody can accept connections at the remote computer: enter its password.".into()),
        _ => Some("The remote computer refused the connection.".into()),
    }
}

// ---- Accepting connections made without the password ----

/// A question from the agent: let `requester` in? (approval.rs)
#[derive(serde::Deserialize, Serialize, Clone)]
struct Approval {
    id: u64,
    requester: String,
    /// "control" or "files"
    kind: String,
    timeout_s: u64,
}

static APPROVALS: Mutex<Vec<Approval>> = Mutex::new(Vec::new());
static AGENT_STDIN: Mutex<Option<std::process::ChildStdin>> = Mutex::new(None);

fn read_approvals(stdout: std::process::ChildStdout) {
    use std::io::BufRead;
    for line in std::io::BufReader::new(stdout).lines() {
        let Ok(line) = line else { break };
        if let Some(json) = line.strip_prefix("@lunaris-approval ") {
            if let Ok(q) = serde_json::from_str::<Approval>(json) {
                APPROVALS.lock().unwrap().push(q);
            }
        } else if let Some(json) = line.strip_prefix("@lunaris-approval-cancel ") {
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(json) {
                let id = v.get("id").and_then(|i| i.as_u64()).unwrap_or(0);
                APPROVALS.lock().unwrap().retain(|q| q.id != id);
            }
        }
    }
    // The agent stopped: nothing left to answer.
    APPROVALS.lock().unwrap().clear();
}

/// The questions waiting for an answer, as JSON (oldest first).
pub fn approvals_json() -> String {
    serde_json::to_string(&*APPROVALS.lock().unwrap()).unwrap_or_else(|_| "[]".into())
}

/// Answers question `id`.
pub fn answer_approval(id: u64, accept: bool) {
    use std::io::Write;
    APPROVALS.lock().unwrap().retain(|q| q.id != id);
    if let Some(stdin) = AGENT_STDIN.lock().unwrap().as_mut() {
        let _ = writeln!(stdin, "{{\"id\":{},\"accept\":{}}}", id, accept);
        let _ = stdin.flush();
    }
}

/// How connections are accepted ("password", "click", "password-click");
/// restarts the app's agent.
pub fn set_approve_mode(mode: &str) -> Result<(), String> {
    if !matches!(mode, "password" | "click" | "password-click") {
        return Err(format!("unknown mode {}", mode));
    }
    let mut config = load_or_create_config()?;
    config.approve_mode = mode.to_string();
    save(&config)?;
    restart_if_ours()
}

/// Starts a child process from a thread that lives as long as the app.
/// PR_SET_PDEATHSIG (stop_with_parent) fires when the *thread* that started
/// the child ends, so a child started from a short-lived thread (e.g. the
/// one waiting for the server's token) would be stopped right away.
pub fn spawn_child(cmd: Command) -> std::io::Result<Child> {
    use std::sync::mpsc;
    type Job = (Command, mpsc::Sender<std::io::Result<Child>>);
    static SPAWNER: Mutex<Option<mpsc::Sender<Job>>> = Mutex::new(None);
    let tx = {
        let mut lock = SPAWNER.lock().unwrap();
        lock.get_or_insert_with(|| {
            let (tx, rx) = mpsc::channel::<Job>();
            std::thread::Builder::new()
                .name("lunaris-spawner".into())
                .spawn(move || {
                    for (mut cmd, reply) in rx {
                        let _ = reply.send(cmd.spawn());
                    }
                })
                .expect("spawner thread");
            tx
        })
        .clone()
    };
    let (reply_tx, reply_rx) = mpsc::channel();
    tx.send((cmd, reply_tx))
        .map_err(|_| std::io::Error::new(std::io::ErrorKind::Other, "spawner thread gone"))?;
    reply_rx
        .recv()
        .map_err(|_| std::io::Error::new(std::io::ErrorKind::Other, "spawner thread gone"))?
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

/// Registers this computer with the server the app is signed in to
/// (`server` as the app knows it, http(s)://host:port; `token` its agent
/// token), so it shows up in that server's computer list while remote control
/// is on. Doesn't move an agent already registered with another server.
/// Returns "registered", "already" or "elsewhere <server>".
pub fn use_server(server: &str, token: &str) -> Result<String, String> {
    let ws_url = agent_server_url(server).ok_or_else(|| format!("unexpected server address {}", server))?;
    let mut config = load_or_create_config()?;
    let current = config.server_url.trim().trim_end_matches('/').to_string();
    let same = |a: &str, b: &str| {
        let norm = |u: &str| u.replace("://localhost", "://127.0.0.1").to_ascii_lowercase();
        norm(a) == norm(b)
    };
    // The app's own server only got the agent because nothing else had it:
    // the server the user signed in to takes over.
    let own_server = same(&current, &crate::local_server::agent_url());
    if !current.is_empty() && !config.server_token.trim().is_empty() && !same(&current, &ws_url) && !own_server {
        return Ok(format!("elsewhere {}", current));
    }
    if same(&current, &ws_url) && config.server_token == token {
        return Ok("already".into());
    }
    config.server_url = ws_url;
    config.server_token = token.to_string();
    save(&config)?;
    restart_if_ours()?;
    Ok("registered".into())
}

/// The agent's signaling address for a server the app knows as
/// http(s)://host:port.
fn agent_server_url(server: &str) -> Option<String> {
    let s = server.trim().trim_end_matches('/');
    if let Some(rest) = s.strip_prefix("https://") {
        Some(format!("wss://{}", rest))
    } else if let Some(rest) = s.strip_prefix("http://") {
        Some(format!("ws://{}", rest))
    } else if s.starts_with("ws://") || s.starts_with("wss://") {
        Some(s.to_string())
    } else if !s.is_empty() && !s.contains("://") {
        Some(format!("ws://{}", s))
    } else {
        None
    }
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

/// What clients may do with this computer: share its clipboard, send it
/// files. Restarts the app's agent so new sessions follow.
pub fn set_sharing(clipboard: bool, files: bool) -> Result<(), String> {
    let mut config = load_or_create_config()?;
    config.allow_clipboard = clipboard;
    config.allow_file_transfer = files;
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

#[cfg(test)]
mod tests {
    use super::agent_server_url;

    #[test]
    fn agent_server_url_from_the_app_server_address() {
        assert_eq!(agent_server_url("http://192.168.1.5:8090/").as_deref(), Some("ws://192.168.1.5:8090"));
        assert_eq!(agent_server_url("https://lunaris.example.com").as_deref(), Some("wss://lunaris.example.com"));
        assert_eq!(agent_server_url("ws://10.0.0.2:8090").as_deref(), Some("ws://10.0.0.2:8090"));
        assert_eq!(agent_server_url("10.0.0.2:8090").as_deref(), Some("ws://10.0.0.2:8090"));
        assert_eq!(agent_server_url("ftp://x").as_deref(), None);
        assert_eq!(agent_server_url("").as_deref(), None);
    }
}
