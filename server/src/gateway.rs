//! The server gateway: a Lunaris agent the server runs itself, so computers
//! of other types (RustDesk, Sunshine, RDP, VNC) can be added and reached
//! through the server without installing an agent anywhere else (a server
//! on a VPS, a box in the LAN).
//!
//! The server starts `lunaris-agent --cli --bridge-only` as a child process
//! (agent/src/bridge_agent.rs), without a shell, with a new random password
//! in its environment at each start. The agent listens on 127.0.0.1 only,
//! says its port on standard output, and ends when its standard input
//! closes. The server then connects to it like to a computer added by
//! address (links.rs, with that password: its sources can be managed), so
//! it is one host in the list and in /api/connectors. Its id is the
//! server's (kept in the database and in the agent's config file), so it
//! stays the same from one start to the next.
//!
//! The bridge-only agent never shows this machine: no screen, input,
//! clipboard or files of it; only the computers added through it.
//!
//! On when a `lunaris-agent` that knows `--bridge-only` is found (beside the
//! server's program, in PATH, or where its packages install it), unless an
//! administrator turned it off on the Server page (kept in the database,
//! `server_settings`). `LUNARIS_GATEWAY=off|on|<path of lunaris-agent>`
//! overrides: off and on fix the switch, a path names the program.
//!
//! Its config and sources: `<data dir>/gateway/agent_config.json`. Its log
//! goes to the server's, each line marked `gateway |`.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::Json;
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;
use tokio::sync::watch;
use tracing::{info, warn};

use crate::signaling::SignalingState;

/// The environment variable that overrides the switch or names the program.
pub const ENV: &str = "LUNARIS_GATEWAY";
/// Its log level (the agent's RUST_LOG), when set.
const LOG_ENV: &str = "LUNARIS_GATEWAY_LOG";
/// As in agent/src/bridge_agent.rs (the server does not link the agent).
const PASSWORD_ENV: &str = "LUNARIS_BRIDGE_PASSWORD";
const PORT_LINE: &str = "@lunaris-bridge port=";

const KEY_ENABLED: &str = "gateway_enabled";
const KEY_HOST_ID: &str = "gateway_host_id";

/// What the log calls its link (links.rs).
const LINK_NAME: &str = "the server gateway";

#[cfg(windows)]
const PROGRAM: &str = "lunaris-agent.exe";
#[cfg(not(windows))]
const PROGRAM: &str = "lunaris-agent";

/// What `LUNARIS_GATEWAY` says.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EnvSetting {
    Unset,
    Off,
    On,
    /// The `lunaris-agent` to run (and the switch is the page's).
    Program(PathBuf),
}

pub fn env_setting(value: Option<&str>) -> EnvSetting {
    match value.map(str::trim).unwrap_or("") {
        "" => EnvSetting::Unset,
        v if ["off", "0", "false", "no"].contains(&v.to_ascii_lowercase().as_str()) => EnvSetting::Off,
        v if ["on", "1", "true", "yes"].contains(&v.to_ascii_lowercase().as_str()) => EnvSetting::On,
        v => EnvSetting::Program(PathBuf::from(v)),
    }
}

/// Whether the gateway should run: the environment first, then the switch
/// saved from the Server page (on unless turned off).
fn wanted(env: &EnvSetting, saved: Option<&str>) -> bool {
    match env {
        EnvSetting::Off => false,
        EnvSetting::On => true,
        _ => saved != Some("off"),
    }
}

/// Where the gateway is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Phase {
    /// Turned off (the Server page, or LUNARIS_GATEWAY=off).
    Disabled,
    /// No lunaris-agent found.
    NotFound,
    /// The lunaris-agent found is too old (no `--bridge-only`).
    Unsupported,
    /// Being started, or started and not connected yet.
    Starting,
    Running,
    /// It ended or could not start; started again after a while.
    Failing,
}

#[derive(Debug, Clone)]
struct Status {
    phase: Phase,
    error: Option<String>,
    program: Option<PathBuf>,
    /// How often in a row it ended soon or could not start.
    failures: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Wish {
    Run,
    Stop,
    /// The server stops.
    Exit,
}

/// How long the steps may take (shorter in tests).
#[derive(Debug, Clone, Copy)]
pub(crate) struct Timing {
    probe: Duration,
    port: Duration,
    stop: Duration,
    /// After "not found" or "too old": look again.
    recheck: Duration,
    first_backoff: Duration,
    max_backoff: Duration,
    /// Ran this long: the next failure starts the backoff again.
    stable: Duration,
}

impl Default for Timing {
    fn default() -> Self {
        Timing {
            probe: Duration::from_secs(15),
            port: Duration::from_secs(30),
            stop: Duration::from_secs(5),
            recheck: Duration::from_secs(60),
            first_backoff: Duration::from_secs(2),
            max_backoff: Duration::from_secs(120),
            stable: Duration::from_secs(120),
        }
    }
}

pub struct Gateway {
    env: EnvSetting,
    status: Mutex<Status>,
    wish: watch::Sender<Wish>,
    host_id: Mutex<Option<String>>,
    /// The link to the running agent (dropping it closes it).
    link: Mutex<Option<crate::links::Link>>,
    task: Mutex<Option<tokio::task::JoinHandle<()>>>,
}

impl Gateway {
    pub fn new(env: EnvSetting) -> Self {
        Gateway {
            env,
            status: Mutex::new(Status { phase: Phase::Disabled, error: None, program: None, failures: 0 }),
            wish: watch::channel(Wish::Stop).0,
            host_id: Mutex::new(None),
            link: Mutex::new(None),
            task: Mutex::new(None),
        }
    }

    pub fn from_env() -> Self {
        Self::new(env_setting(std::env::var(ENV).ok().as_deref()))
    }

    /// The gateway's computer id (known once the server started it, also
    /// while it is off).
    pub fn host_id(&self) -> Option<String> {
        self.host_id.lock().unwrap().clone()
    }

    pub fn is_gateway(&self, host_id: &str) -> bool {
        self.host_id.lock().unwrap().as_deref() == Some(host_id)
    }

    /// Whether it is switched on (it may still be not found or failing).
    pub fn enabled(&self) -> bool {
        *self.wish.borrow() == Wish::Run
    }

    fn set_wish(&self, wish: Wish) {
        self.wish.send_replace(wish);
    }

    fn set(&self, phase: Phase, error: Option<String>) {
        let mut status = self.status.lock().unwrap();
        if phase == Phase::Failing {
            status.failures += 1;
        } else if phase == Phase::Disabled {
            status.failures = 0;
        }
        status.phase = phase;
        status.error = error;
    }

    fn set_program(&self, program: Option<PathBuf>) {
        self.status.lock().unwrap().program = program;
    }

    /// For the Server page (administrators).
    pub fn status_json(&self, online: bool) -> serde_json::Value {
        let status = self.status.lock().unwrap().clone();
        let link_error = self.link.lock().unwrap().as_ref().and_then(|l| l.error.lock().unwrap().clone());
        // Started, but the server is not connected to it (yet).
        let (phase, error) = match status.phase {
            Phase::Running if !online => (Phase::Starting, link_error.or(status.error)),
            phase => (phase, status.error),
        };
        serde_json::json!({
            "enabled": self.enabled(),
            "forced": match self.env { EnvSetting::On => Some("on"), EnvSetting::Off => Some("off"), _ => None },
            "state": phase,
            "error": error,
            "program": status.program.map(|p| p.to_string_lossy().into_owned()),
            "host_id": self.host_id(),
            "failures": status.failures,
        })
    }

    /// Stops the agent (server shutdown): its standard input is closed,
    /// then it is killed if it is still there.
    pub async fn shutdown(&self) {
        self.set_wish(Wish::Exit);
        let task = self.task.lock().unwrap().take();
        if let Some(mut task) = task {
            if tokio::time::timeout(Duration::from_secs(10), &mut task).await.is_err() {
                task.abort();
            }
        }
        self.link.lock().unwrap().take();
    }
}

/// The gateway's data folder.
fn gateway_dir() -> PathBuf {
    crate::paths::data_file("gateway")
}

/// The name it registers with.
fn gateway_name() -> String {
    let host = hostname::get().ok().and_then(|h| h.into_string().ok()).unwrap_or_default();
    let host = host.trim();
    if host.is_empty() {
        "Lunaris server (gateway)".to_string()
    } else {
        format!("{} (gateway)", host)
    }
}

/// Starts the gateway at server start (it runs when it is on and found).
pub async fn start(state: &Arc<SignalingState>) {
    let finder_env = state.gateway.env.clone();
    start_with(state, gateway_dir(), gateway_name(), Box::new(move || find_program(&finder_env)), Timing::default()).await;
}

type Finder = Box<dyn Fn() -> Option<PathBuf> + Send + Sync>;

pub(crate) async fn start_with(state: &Arc<SignalingState>, dir: PathBuf, name: String, find: Finder, timing: Timing) {
    let gw = &state.gateway;
    match host_id(&state.db, &dir).await {
        Ok(id) => *gw.host_id.lock().unwrap() = Some(id),
        Err(e) => {
            warn!("Server gateway: {}", e);
            gw.set(Phase::Failing, Some(e));
            return;
        }
    }
    let saved = crate::db::setting(&state.db, KEY_ENABLED).await.ok().flatten();
    let on = wanted(&gw.env, saved.as_deref());
    match &gw.env {
        EnvSetting::Off => info!("Server gateway: off ({}=off)", ENV),
        EnvSetting::On => info!("Server gateway: on ({}=on)", ENV),
        _ if !on => info!("Server gateway: turned off on the Server page"),
        _ => {}
    }
    gw.set_wish(if on { Wish::Run } else { Wish::Stop });
    let task = tokio::spawn(supervise(state.clone(), dir, name, find, timing));
    *gw.task.lock().unwrap() = Some(task);
}

/// The gateway's id: the one kept in the database, else the one in its
/// config file (a database made again), else a new one; kept in the database.
async fn host_id(db: &sqlx::SqlitePool, dir: &Path) -> Result<String, String> {
    if let Some(id) = crate::db::setting(db, KEY_HOST_ID).await.map_err(|e| format!("database: {e}"))? {
        if !id.trim().is_empty() {
            return Ok(id);
        }
    }
    let from_file = std::fs::read_to_string(dir.join("agent_config.json"))
        .ok()
        .and_then(|text| serde_json::from_str::<serde_json::Value>(&text).ok())
        .and_then(|v| v.get("client_unique_id").and_then(|id| id.as_str()).map(str::to_owned))
        .filter(|id| !id.trim().is_empty());
    let id = from_file.unwrap_or_else(|| uuid::Uuid::new_v4().to_string().to_uppercase());
    crate::db::set_setting(db, KEY_HOST_ID, &id).await.map_err(|e| format!("database: {e}"))?;
    Ok(id)
}

/// The agent's config file, with the gateway's id: made when there is none,
/// its id put right when it has another (its sources are kept).
fn prepare_config(dir: &Path, host_id: &str) -> Result<PathBuf, String> {
    std::fs::create_dir_all(dir).map_err(|e| format!("cannot make {}: {}", dir.display(), e))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700));
    }
    let path = dir.join("agent_config.json");
    let mut config = match std::fs::read_to_string(&path) {
        Ok(text) => match serde_json::from_str::<serde_json::Value>(&text) {
            Ok(serde_json::Value::Object(map)) => map,
            _ => return Err(format!("{} cannot be read; move it away to start the gateway with a new one", path.display())),
        },
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => serde_json::Map::new(),
        Err(e) => return Err(format!("cannot read {}: {}", path.display(), e)),
    };
    if config.get("client_unique_id").and_then(|v| v.as_str()) != Some(host_id) {
        config.insert("client_unique_id".into(), host_id.into());
        let text = serde_json::to_string_pretty(&serde_json::Value::Object(config)).unwrap_or_default();
        crate::paths::write_private(&path, &text).map_err(|e| format!("cannot write {}: {}", path.display(), e))?;
    }
    Ok(path)
}

/// Where a `lunaris-agent` may be: `LUNARIS_GATEWAY=<path>` alone when it
/// is set; else beside the server's program, in PATH, then where the
/// agent's packages and installer put it.
pub fn candidates(env: &EnvSetting) -> Vec<PathBuf> {
    if let EnvSetting::Program(path) = env {
        return vec![path.clone()];
    }
    let mut out = Vec::new();
    if let Some(dir) = std::env::current_exe().ok().and_then(|exe| exe.parent().map(Path::to_path_buf)) {
        out.push(dir.join(PROGRAM));
    }
    if let Some(path) = std::env::var_os("PATH") {
        out.extend(std::env::split_paths(&path).filter(|d| d.is_absolute()).map(|d| d.join(PROGRAM)));
    }
    #[cfg(windows)]
    {
        // agent-installer.nsi: the folder chosen there (its registry
        // value), else its default "$PROGRAMFILES64\Lunaris Agent".
        if let Some(dir) = agent_install_dir() {
            out.push(dir.join(PROGRAM));
        }
        for var in ["ProgramW6432", "ProgramFiles"] {
            if let Some(dir) = std::env::var_os(var) {
                out.push(PathBuf::from(dir).join("Lunaris Agent").join(PROGRAM));
            }
        }
    }
    #[cfg(not(windows))]
    {
        out.push(PathBuf::from("/usr/bin").join(PROGRAM));
        out.push(PathBuf::from("/usr/local/bin").join(PROGRAM));
    }
    out
}

/// HKLM\Software\Lunaris Agent\InstallDir (agent-installer.nsi), read
/// with reg.exe.
#[cfg(windows)]
fn agent_install_dir() -> Option<PathBuf> {
    use std::os::windows::process::CommandExt;
    let system = std::env::var_os("SystemRoot").map(PathBuf::from).unwrap_or_else(|| PathBuf::from(r"C:\Windows"));
    let out = std::process::Command::new(system.join("System32").join("reg.exe"))
        // The agent's installer is 32-bit NSIS without SetRegView: its key
        // is in the 32-bit view (WOW6432Node).
        .args(["query", r"HKLM\Software\Lunaris Agent", "/v", "InstallDir", "/reg:32"])
        .creation_flags(0x0800_0000)
        .output()
        .ok()?;
    reg_value(&String::from_utf8_lossy(&out.stdout)).map(PathBuf::from)
}

/// The value in `reg query ... /v InstallDir` output.
#[cfg_attr(not(windows), allow(dead_code))]
fn reg_value(text: &str) -> Option<String> {
    text.lines().find_map(|line| {
        let line = line.trim();
        let rest = line.strip_prefix("InstallDir")?.trim_start();
        let value = rest.strip_prefix("REG_SZ")?.trim();
        (!value.is_empty()).then(|| value.to_string())
    })
}

pub fn find_program(env: &EnvSetting) -> Option<PathBuf> {
    candidates(env).into_iter().find(|p| p.is_file())
}

/// The environment the agent gets: what a program needs to run, never the
/// server's own settings (its keys, tokens, database).
fn child_env() -> Vec<(String, String)> {
    const KEEP: &[&str] = &[
        "PATH", "HOME", "USER", "LANG", "LC_ALL", "LC_CTYPE", "TZ", "TMPDIR",
        // Windows
        "SystemRoot", "SystemDrive", "windir", "TEMP", "TMP", "USERPROFILE", "APPDATA", "LOCALAPPDATA",
        "ProgramData", "ProgramFiles", "ProgramW6432", "CommonProgramFiles", "PATHEXT", "COMSPEC", "COMPUTERNAME",
        "NUMBER_OF_PROCESSORS", "PROCESSOR_ARCHITECTURE",
    ];
    let mut env: Vec<(String, String)> = std::env::vars()
        .filter(|(k, _)| KEEP.iter().any(|keep| keep.eq_ignore_ascii_case(k)))
        .collect();
    if let Ok(level) = std::env::var(LOG_ENV) {
        env.push(("RUST_LOG".into(), level));
    }
    env.push(("NO_COLOR".into(), "1".into()));
    env
}

fn command(program: &Path) -> Command {
    let mut cmd = Command::new(program);
    cmd.env_clear().envs(child_env()).kill_on_drop(true);
    #[cfg(windows)]
    {
        // No console window of its own.
        cmd.creation_flags(0x0800_0000);
    }
    cmd
}

/// Whether `program` can be the gateway: it knows `--bridge-only`.
async fn probe(program: &Path, timeout: Duration) -> Result<(), String> {
    let mut cmd = command(program);
    cmd.args(["--bridge-only", "--help"])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    let status = match tokio::time::timeout(timeout, async { cmd.status().await }).await {
        Ok(Ok(status)) => status,
        Ok(Err(e)) => return Err(format!("cannot run {}: {}", program.display(), e)),
        Err(_) => return Err(format!("{} did not answer", program.display())),
    };
    if status.success() {
        Ok(())
    } else {
        Err(format!("{} is too old for the server gateway (update lunaris-agent)", program.display()))
    }
}

/// The line without terminal colours and control characters.
fn clean_line(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' {
            if chars.peek() == Some(&'[') {
                chars.next();
                for c in chars.by_ref() {
                    if c.is_ascii_alphabetic() {
                        break;
                    }
                }
            }
            continue;
        }
        if !c.is_control() || c == '\t' {
            out.push(c);
        }
    }
    out.trim_end().to_string()
}

fn port_of_line(line: &str) -> Option<u16> {
    line.trim().strip_prefix(PORT_LINE)?.parse().ok().filter(|p| *p > 0)
}

/// The line without the agent's own time stamp (the server's log has one).
fn without_time(line: &str) -> &str {
    match line.split_once(' ') {
        Some((first, rest)) if first.len() >= 20 && first.as_bytes()[0].is_ascii_digit() && first.contains('T') && first.ends_with('Z') => rest.trim_start(),
        _ => line,
    }
}

fn log_line(line: &str) {
    let line = clean_line(line);
    let line = without_time(&line);
    if !line.is_empty() {
        info!(target: "lunaris_gateway", "gateway | {}", line);
    }
}

/// Waits `dur`, or less when the wish changes. False: the server stops.
async fn wait(rx: &mut watch::Receiver<Wish>, dur: Duration) -> bool {
    tokio::select! {
        _ = tokio::time::sleep(dur) => true,
        changed = rx.changed() => changed.is_ok() && *rx.borrow() != Wish::Exit,
    }
}

enum Outcome {
    /// Turned off, or the server stops.
    Stopped,
    /// Why, and how long it ran.
    Failed(String, Duration),
}

async fn supervise(state: Arc<SignalingState>, dir: PathBuf, name: String, find: Finder, timing: Timing) {
    let gw = &state.gateway;
    let mut rx = gw.wish.subscribe();
    let mut backoff = timing.first_backoff;
    loop {
        let wish = *rx.borrow_and_update();
        match wish {
            Wish::Exit => break,
            Wish::Stop => {
                gw.set(Phase::Disabled, None);
                if rx.changed().await.is_err() {
                    break;
                }
                continue;
            }
            Wish::Run => {}
        }
        let Some(program) = find() else {
            if gw.status.lock().unwrap().phase != Phase::NotFound {
                info!("Server gateway: no lunaris-agent found (install it beside the server to add RustDesk, Sunshine, RDP and VNC computers through the server)");
            }
            gw.set_program(None);
            gw.set(Phase::NotFound, None);
            if !wait(&mut rx, timing.recheck).await {
                break;
            }
            continue;
        };
        gw.set_program(Some(program.clone()));
        if let Err(why) = probe(&program, timing.probe).await {
            if gw.status.lock().unwrap().error.as_deref() != Some(why.as_str()) {
                warn!("Server gateway: {}", why);
            }
            gw.set(Phase::Unsupported, Some(why));
            if !wait(&mut rx, timing.recheck).await {
                break;
            }
            continue;
        }
        gw.set(Phase::Starting, None);
        match run_once(&state, &program, &dir, &name, &mut rx, timing).await {
            Outcome::Stopped => {
                backoff = timing.first_backoff;
                gw.status.lock().unwrap().failures = 0;
            }
            Outcome::Failed(why, ran) => {
                if ran >= timing.stable {
                    backoff = timing.first_backoff;
                    gw.status.lock().unwrap().failures = 0;
                }
                warn!("Server gateway: {}; starting it again in {} s", why, backoff.as_secs().max(1));
                gw.set(Phase::Failing, Some(why));
                if !wait(&mut rx, backoff).await {
                    break;
                }
                backoff = (backoff * 2).min(timing.max_backoff);
            }
        }
    }
    gw.link.lock().unwrap().take();
}

async fn run_once(
    state: &Arc<SignalingState>,
    program: &Path,
    dir: &Path,
    name: &str,
    rx: &mut watch::Receiver<Wish>,
    timing: Timing,
) -> Outcome {
    let started = Instant::now();
    let failed = |why: String| Outcome::Failed(why, started.elapsed());
    let gw = &state.gateway;
    let Some(host_id) = gw.host_id() else { return failed("no id".into()) };
    let config = match prepare_config(dir, &host_id) {
        Ok(path) => path,
        Err(e) => return failed(e),
    };
    // 32 random bytes, new for each start; only in the agent's environment.
    let password = crate::security::random_hex(32);
    let mut cmd = command(program);
    cmd.arg("--cli")
        .arg("--bridge-only")
        .arg("--config")
        .arg(&config)
        .arg("--name")
        .arg(name)
        .env(PASSWORD_ENV, &password)
        .current_dir(dir)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    let mut child = match cmd.spawn() {
        Ok(child) => child,
        Err(e) => return failed(format!("cannot start {}: {}", program.display(), e)),
    };
    info!("Server gateway: started {} (pid {})", program.display(), child.id().unwrap_or(0));
    let stdin = child.stdin.take();
    let mut stdout = BufReader::new(child.stdout.take().expect("piped")).lines();
    let stderr = child.stderr.take().map(|err| {
        tokio::spawn(async move {
            let mut lines = BufReader::new(err).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                log_line(&line);
            }
        })
    });
    let _stderr = AbortOnDrop(stderr);

    // Its port, on standard output.
    let port = tokio::time::timeout(timing.port, async {
        while let Ok(Some(line)) = stdout.next_line().await {
            if let Some(port) = port_of_line(&line) {
                return Some(port);
            }
            log_line(&line);
        }
        None
    })
    .await;
    let port = match port {
        Ok(Some(port)) => port,
        Ok(None) => {
            let status = stop(child, stdin, timing.stop).await;
            return failed(format!("the gateway agent ended before it was ready ({})", status));
        }
        Err(_) => {
            stop(child, stdin, timing.stop).await;
            return failed("the gateway agent did not say its port in time".into());
        }
    };
    let _stdout = AbortOnDrop(Some(tokio::spawn(async move {
        while let Ok(Some(line)) = stdout.next_line().await {
            log_line(&line);
        }
    })));
    info!("Server gateway: listening on 127.0.0.1:{}", port);
    let link = crate::links::spawn(state, format!("127.0.0.1:{}", port), password, Some(host_id), LINK_NAME);
    *gw.link.lock().unwrap() = Some(link);
    gw.set(Phase::Running, None);

    let outcome = tokio::select! {
        status = child.wait() => {
            let status = status.map(|s| s.to_string()).unwrap_or_else(|e| e.to_string());
            drop(stdin);
            failed(format!("the gateway agent ended ({})", status))
        }
        _ = async {
            loop {
                if rx.changed().await.is_err() || *rx.borrow() != Wish::Run {
                    break;
                }
            }
        } => {
            gw.link.lock().unwrap().take();
            let status = stop(child, stdin, timing.stop).await;
            info!("Server gateway: stopped ({})", status);
            Outcome::Stopped
        }
    };
    gw.link.lock().unwrap().take();
    outcome
}

/// Closes its standard input (it ends by itself then), kills it when it is
/// still there after `grace`. Says how it ended.
async fn stop(mut child: tokio::process::Child, stdin: Option<tokio::process::ChildStdin>, grace: Duration) -> String {
    drop(stdin);
    match tokio::time::timeout(grace, child.wait()).await {
        Ok(Ok(status)) => status.to_string(),
        Ok(Err(e)) => e.to_string(),
        Err(_) => {
            let _ = child.kill().await;
            "killed".into()
        }
    }
}

struct AbortOnDrop(Option<tokio::task::JoinHandle<()>>);

impl Drop for AbortOnDrop {
    fn drop(&mut self) {
        if let Some(task) = self.0.take() {
            task.abort();
        }
    }
}

#[derive(Deserialize)]
pub struct Switch {
    enabled: bool,
}

/// GET /api/admin/server/gateway: where the gateway is.
pub async fn get_gateway(_admin: crate::auth::AdminUser, State(state): State<Arc<SignalingState>>) -> impl IntoResponse {
    Json(status(&state))
}

pub fn status(state: &SignalingState) -> serde_json::Value {
    let online = state.gateway.host_id().is_some_and(|id| state.agents.read().unwrap().contains_key(&id));
    state.gateway.status_json(online)
}

/// PUT /api/admin/server/gateway {"enabled": bool}: the Server page's
/// switch, kept in the database. Not while LUNARIS_GATEWAY=on|off fixes it.
pub async fn put_gateway(
    admin: crate::auth::AdminUser,
    State(state): State<Arc<SignalingState>>,
    Json(switch): Json<Switch>,
) -> impl IntoResponse {
    let gw = &state.gateway;
    if matches!(gw.env, EnvSetting::On | EnvSetting::Off) {
        let error = format!("{} is set in the server's environment, which decides this; remove it to use this switch", ENV);
        return (StatusCode::CONFLICT, Json(serde_json::json!({ "error": error }))).into_response();
    }
    let value = if switch.enabled { "on" } else { "off" };
    if let Err(e) = crate::db::set_setting(&state.db, KEY_ENABLED, value).await {
        tracing::error!("Database error: {}", e);
        return (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({ "error": "Internal Database Error" }))).into_response();
    }
    info!("Server gateway: turned {} by {}", value, admin.username);
    gw.set_wish(if switch.enabled { Wish::Run } else { Wish::Stop });
    // The answer says where it is then: a few seconds for it to stop, or
    // to be started (or found missing).
    for _ in 0..30 {
        let phase = gw.status.lock().unwrap().phase;
        let settled = if switch.enabled { phase != Phase::Disabled && phase != Phase::Starting } else { phase == Phase::Disabled };
        if settled || gw.task.lock().unwrap().is_none() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    Json(status(&state)).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_environment_decides_first() {
        assert_eq!(env_setting(None), EnvSetting::Unset);
        assert_eq!(env_setting(Some(" ")), EnvSetting::Unset);
        assert_eq!(env_setting(Some("OFF")), EnvSetting::Off);
        assert_eq!(env_setting(Some("on")), EnvSetting::On);
        assert_eq!(env_setting(Some("/opt/x/lunaris-agent")), EnvSetting::Program("/opt/x/lunaris-agent".into()));
        assert!(wanted(&EnvSetting::Unset, None));
        assert!(wanted(&EnvSetting::Unset, Some("on")));
        assert!(!wanted(&EnvSetting::Unset, Some("off")));
        assert!(!wanted(&EnvSetting::Program("x".into()), Some("off")));
        assert!(wanted(&EnvSetting::On, Some("off")));
        assert!(!wanted(&EnvSetting::Off, Some("on")));
        // A path is the only place looked at.
        assert_eq!(candidates(&EnvSetting::Program("/x/agent".into())), vec![PathBuf::from("/x/agent")]);
        let all = candidates(&EnvSetting::Unset);
        assert!(all.iter().all(|p| p.ends_with(PROGRAM)));
        #[cfg(not(windows))]
        assert!(all.contains(&PathBuf::from("/usr/bin/lunaris-agent")));
    }

    #[test]
    fn the_agent_installers_folder() {
        let out = "\r\nHKEY_LOCAL_MACHINE\\Software\\Lunaris Agent\r\n    InstallDir    REG_SZ    D:\\Apps\\Lunaris Agent\r\n\r\n";
        assert_eq!(reg_value(out).as_deref(), Some("D:\\Apps\\Lunaris Agent"));
        assert_eq!(reg_value("ERROR: The system was unable to find the specified registry key"), None);
    }

    #[test]
    fn lines_and_ports() {
        assert_eq!(port_of_line("@lunaris-bridge port=40123\n"), Some(40123));
        assert_eq!(port_of_line("@lunaris-bridge port=0"), None);
        assert_eq!(port_of_line("INFO something"), None);
        assert_eq!(clean_line("\u{1b}[2m2026\u{1b}[0m \u{1b}[32m INFO\u{1b}[0m hi\r"), "2026  INFO hi");
        assert_eq!(without_time("2026-11-20T10:39:15.014201Z  INFO agent: hi"), "INFO agent: hi");
        assert_eq!(without_time("INFO agent: hi"), "INFO agent: hi");
    }

    #[test]
    fn the_child_gets_no_server_secrets() {
        let env = child_env();
        for (k, _) in &env {
            assert!(!k.contains("TOKEN") && !k.contains("SECRET") && !k.contains("PASSWORD") && k != "DATABASE_URL", "{}", k);
        }
        assert!(env.iter().any(|(k, v)| k == "NO_COLOR" && v == "1"));
    }

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("lunaris-gw-{}-{}", name, crate::security::random_hex(6)));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn the_config_keeps_its_sources_and_takes_the_id() {
        let dir = temp_dir("config");
        let path = prepare_config(&dir, "GW-1").unwrap();
        let read = || serde_json::from_str::<serde_json::Value>(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(read()["client_unique_id"], "GW-1");
        std::fs::write(&path, r#"{"client_unique_id":"OTHER","sources":[{"id":"a"}]}"#).unwrap();
        prepare_config(&dir, "GW-1").unwrap();
        assert_eq!(read()["client_unique_id"], "GW-1");
        assert_eq!(read()["sources"][0]["id"], "a");
        std::fs::write(&path, "garbage").unwrap();
        assert!(prepare_config(&dir, "GW-1").is_err());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "garbage");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn the_id_is_kept() {
        let db = crate::security::tests::test_db().await;
        let dir = temp_dir("id");
        let first = host_id(&db, &dir).await.unwrap();
        assert_eq!(host_id(&db, &dir).await.unwrap(), first);
        // A new database takes the id of the config file that is there.
        let db2 = crate::security::tests::test_db().await;
        prepare_config(&dir, &first).unwrap();
        assert_eq!(host_id(&db2, &dir).await.unwrap(), first);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A stand-in for lunaris-agent: answers the probe, notes each start
    /// and what it was given, says a port, and ends when its standard input
    /// closes (`stay`) or at once.
    #[cfg(unix)]
    fn fake_agent(dir: &Path, stay: bool) -> PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let log = dir.join("starts");
        let script = format!(
            "#!/bin/sh\n\
             if [ \"$1\" = --bridge-only ] && [ \"$2\" = --help ]; then exit 0; fi\n\
             echo \"start $* pw=${{#LUNARIS_BRIDGE_PASSWORD}} manifest=${{CARGO_MANIFEST_DIR:-none}}\" >> '{log}'\n\
             echo 'a line on stderr' >&2\n\
             echo '@lunaris-bridge port=9'\n\
             {rest}\n",
            log = log.display(),
            rest = if stay { format!("cat >/dev/null\necho stopped >> '{}'", log.display()) } else { "exit 3".into() },
        );
        let path = dir.join(if stay { "fake-agent" } else { "fake-agent-exits" });
        std::fs::write(&path, script).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        path
    }

    #[cfg(unix)]
    fn quick() -> Timing {
        Timing {
            probe: Duration::from_secs(5),
            port: Duration::from_secs(5),
            stop: Duration::from_secs(3),
            recheck: Duration::from_millis(200),
            first_backoff: Duration::from_millis(100),
            max_backoff: Duration::from_millis(400),
            stable: Duration::from_secs(60),
        }
    }

    #[cfg(unix)]
    async fn until(what: impl Fn() -> bool) -> bool {
        for _ in 0..100 {
            if what() {
                return true;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        false
    }

    #[cfg(unix)]
    fn phase(state: &SignalingState) -> Phase {
        state.gateway.status.lock().unwrap().phase
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn runs_links_stops_and_starts_again() {
        let dir = temp_dir("run");
        let program = fake_agent(&dir, true);
        let db = crate::security::tests::test_db().await;
        let state = Arc::new(SignalingState::new(db, "t".into(), None));
        let p = program.clone();
        start_with(&state, dir.join("data"), "srv (gateway)".into(), Box::new(move || Some(p.clone())), quick()).await;
        assert!(until(|| phase(&state) == Phase::Running).await, "{:?}", phase(&state));
        let log = || std::fs::read_to_string(dir.join("starts")).unwrap_or_default();
        let id = state.gateway.host_id().unwrap();
        // Started without a shell, with its config, its name and a long
        // password, and none of the server's environment.
        assert!(log().contains("start --cli --bridge-only --config "), "{}", log());
        assert!(log().contains("agent_config.json --name srv (gateway) pw=64 manifest=none"), "{}", log());
        let config: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(dir.join("data/agent_config.json")).unwrap()).unwrap();
        assert_eq!(config["client_unique_id"], id.as_str());
        // The link to it (nothing answers on port 9: not online).
        assert!(state.gateway.link.lock().unwrap().is_some());
        assert_eq!(state.gateway.link.lock().unwrap().as_ref().unwrap().host_id.lock().unwrap().as_deref(), Some(id.as_str()));
        assert!(until(|| status(&state)["error"].is_string()).await);
        assert_eq!(status(&state)["state"], "starting");
        assert!(state.links.list().is_empty(), "not one of the computers added by address");

        // Turned off: its standard input closes and it ends.
        state.gateway.set_wish(Wish::Stop);
        assert!(until(|| log().contains("stopped")).await, "{}", log());
        assert!(until(|| phase(&state) == Phase::Disabled).await);
        assert!(state.gateway.link.lock().unwrap().is_none());
        assert_eq!(status(&state)["enabled"], false);

        // On again: a new start, the same id.
        state.gateway.set_wish(Wish::Run);
        assert!(until(|| log().matches("start ").count() == 2).await, "{}", log());
        assert!(until(|| phase(&state) == Phase::Running).await);
        assert_eq!(state.gateway.host_id().unwrap(), id);

        // Server shutdown stops it too.
        state.gateway.shutdown().await;
        assert!(until(|| log().matches("stopped").count() == 2).await, "{}", log());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn one_that_ends_is_started_again_and_one_missing_is_looked_for() {
        let dir = temp_dir("exits");
        let program = fake_agent(&dir, false);
        let db = crate::security::tests::test_db().await;
        let state = Arc::new(SignalingState::new(db, "t".into(), None));
        let found = Arc::new(Mutex::new(None::<PathBuf>));
        let f = found.clone();
        start_with(&state, dir.join("data"), "gw".into(), Box::new(move || f.lock().unwrap().clone()), quick()).await;
        assert!(until(|| phase(&state) == Phase::NotFound).await);
        assert_eq!(status(&state)["state"], "not_found");
        *found.lock().unwrap() = Some(program);
        let log = || std::fs::read_to_string(dir.join("starts")).unwrap_or_default();
        assert!(until(|| log().matches("start ").count() >= 3).await, "{}", log());
        assert!(until(|| phase(&state) == Phase::Failing).await);
        let s = status(&state);
        assert!(s["error"].as_str().unwrap().contains("ended"), "{}", s);
        assert!(s["failures"].as_u64().unwrap() >= 2);
        state.gateway.shutdown().await;
        let _ = std::fs::remove_dir_all(&dir);
    }

    async fn admin_state(env: EnvSetting) -> Arc<SignalingState> {
        let db = crate::security::tests::test_db().await;
        sqlx::query("INSERT INTO users (id, username, password_hash, role, is_active) VALUES ('a1', 'root', 'x', 'admin', 1)")
            .execute(&db)
            .await
            .unwrap();
        let mut state = SignalingState::new(db, "t".into(), None);
        state.gateway = Gateway::new(env);
        Arc::new(state)
    }

    fn admin() -> crate::auth::AdminUser {
        crate::auth::AdminUser { user_id: "a1".into(), username: "root".into() }
    }

    async fn body(r: axum::response::Response) -> serde_json::Value {
        serde_json::from_slice(&axum::body::to_bytes(r.into_body(), 1 << 20).await.unwrap()).unwrap()
    }

    /// The switch is kept in the database; LUNARIS_GATEWAY=on|off fixes it.
    #[tokio::test]
    async fn the_switch() {
        let state = admin_state(EnvSetting::Unset).await;
        state.gateway.set_wish(Wish::Run);
        let r = put_gateway(admin(), State(state.clone()), Json(Switch { enabled: false })).await.into_response();
        assert_eq!(r.status(), StatusCode::OK);
        assert_eq!(body(r).await["enabled"], false);
        assert!(!state.gateway.enabled());
        assert_eq!(crate::db::setting(&state.db, KEY_ENABLED).await.unwrap().as_deref(), Some("off"));
        let r = put_gateway(admin(), State(state.clone()), Json(Switch { enabled: true })).await.into_response();
        assert_eq!(body(r).await["enabled"], true);
        assert_eq!(crate::db::setting(&state.db, KEY_ENABLED).await.unwrap().as_deref(), Some("on"));

        let fixed = admin_state(EnvSetting::Off).await;
        let r = put_gateway(admin(), State(fixed.clone()), Json(Switch { enabled: true })).await.into_response();
        assert_eq!(r.status(), StatusCode::CONFLICT);
        assert!(!fixed.gateway.enabled());
        let r = get_gateway(admin(), State(fixed.clone())).await.into_response();
        assert_eq!(body(r).await["forced"], "off");
    }

    /// In the list it is marked; it is not deleted while it is on.
    #[tokio::test]
    async fn listed_as_the_gateway_and_not_deleted_while_on() {
        let state = admin_state(EnvSetting::Unset).await;
        sqlx::query("INSERT INTO hosts (id, name, status) VALUES ('GW', 'srv (gateway)', 'Offline'), ('h2', 'pc', 'Offline')")
            .execute(&state.db)
            .await
            .unwrap();
        *state.gateway.host_id.lock().unwrap() = Some("GW".into());
        state.gateway.set_wish(Wish::Run);
        let user = crate::auth::AuthenticatedUser { user_id: "a1".into(), username: "root".into(), role: "admin".into() };
        let r = crate::hosts_handler(user, State(state.clone())).await.ok().unwrap().into_response();
        let list = body(r).await;
        let find = |id: &str| list.as_array().unwrap().iter().find(|h| h["id"] == id).cloned().unwrap();
        assert_eq!(find("GW")["gateway"], true);
        assert!(find("h2").get("gateway").is_none());

        let r = crate::delete_host_handler(admin(), axum::extract::Path("GW".into()), State(state.clone())).await.into_response();
        assert_eq!(r.status(), StatusCode::CONFLICT);
        let left: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM hosts WHERE id = 'GW'").fetch_one(&state.db).await.unwrap();
        assert_eq!(left, 1);
        // Turned off, its entry may go.
        state.gateway.set_wish(Wish::Stop);
        let r = crate::delete_host_handler(admin(), axum::extract::Path("GW".into()), State(state.clone())).await.into_response();
        assert_eq!(r.status(), StatusCode::OK);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn an_agent_without_bridge_only_is_too_old() {
        use std::os::unix::fs::PermissionsExt;
        let dir = temp_dir("old");
        let program = dir.join("old-agent");
        std::fs::write(&program, "#!/bin/sh\nexit 2\n").unwrap();
        std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).unwrap();
        let err = probe(&program, Duration::from_secs(5)).await.unwrap_err();
        assert!(err.contains("too old"), "{}", err);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
