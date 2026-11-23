
use clap::Parser;
use futures_util::{SinkExt, StreamExt};
use std::sync::Arc;
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::Message as WsMessage;
use tracing::{debug, error, info, warn};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

use common::{
    AgentMessage, RtcSdpType, RtcSessionDescription, ServerToAgentMessage, SignalingMessage,
};
use webrtc::peer_connection::{PeerConnection, RTCIceCandidateInit, RTCSessionDescription};

pub mod approval;
#[cfg(target_os = "linux")]
mod autostart;
mod cli;
mod bridge;
pub mod bridge_agent;
mod buffer;
mod build_info;
pub mod direct;
mod files;
mod game_mouse;
mod gamepad;
mod hub;
mod media_hub;
mod input;
pub mod pairing;
mod rtcp_forward;
pub mod connector;
pub mod rustdesk;
pub mod settings;
mod share;
mod single;
mod status;
mod video;
#[cfg(target_os = "windows")]
pub mod win_service;
#[cfg(target_os = "linux")]
mod capture_service;


pub static CONNECTED_TO_SERVER: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);
pub static AGENT_ACTIVE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
pub static LAST_ERROR: std::sync::Mutex<Option<String>> = std::sync::Mutex::new(None);
pub static LAST_STREAM_STOP_TIME_MS: std::sync::atomic::AtomicU64 =
    std::sync::atomic::AtomicU64::new(0);

fn get_current_time_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

use crate::bridge::{
    codec_supported_for_realtime, hardware_encoder_opens_blocking, setup_bridge_session, BridgeSession,
};
use crate::pairing::AgentConfig;

#[derive(Parser, Debug)]
#[command(
    name = "lunaris-agent",
    author,
    version,
    about = "The Lunaris agent: shares this computer with a Lunaris server (a daemon, no window)",
    long_about = "The Lunaris agent: shares this computer with a Lunaris server. It has no window: \
it runs as a service and is managed from the server's web page.\n\n\
  lunaris-agent --enroll --server URL     join a server (install token in LUNARIS_INSTALL_TOKEN)\n\
  lunaris-agent --cli                     run the agent (what the services run)\n\
  lunaris-agent status                    server, connection, addresses, settings\n\
  lunaris-agent set KEY VALUE             change a setting (then restart the agent)\n\n\
--system uses the headless system service's config (Linux: /var/lib/lunaris-agent, \
Windows: the LunarisAgentService's, %ProgramData%\\Lunaris); run it as root / administrator."
)]
struct Args {
    #[command(subcommand)]
    command: Option<Command>,

    #[arg(long, default_value = "127.0.0.1", hide = true)]
    host_ip: String,

    #[arg(long, hide = true)]
    pair: bool,

    #[arg(long, hide = true)]
    pin: Option<String>,

    /// With --enroll: the Lunaris server (https://host:port, wss://..., host:port)
    #[arg(long)]
    server: Option<String>,

    /// This computer's name on the server (default: the name saved by
    /// --enroll, else the host name)
    #[arg(long)]
    name: Option<String>,

    /// Run the agent in the foreground (what the services run). The agent
    /// has no window, so this is also what happens without any command.
    #[arg(long)]
    cli: bool,

    /// Config file (default: ~/.config/lunaris-agent/agent_config.json,
    /// %APPDATA%\lunaris-agent\agent_config.json, or ./agent_config.json
    /// when it exists)
    #[arg(long, global = true, conflicts_with = "system")]
    config: Option<String>,

    /// Use the config of the system service instead of the user's: Linux
    /// /var/lib/lunaris-agent/agent_config.json (lunaris-agent-system.service,
    /// headless machines), Windows %ProgramData%\Lunaris\agent_config.json
    /// (the LunarisAgentService service). Needs root / an elevated prompt.
    #[arg(long, global = true)]
    system: bool,

    /// Merge the settings of another agent_config.json into the config
    #[arg(long)]
    import_config: Option<String>,

    /// Join a Lunaris server with an install token and exit (what the
    /// one-line installer runs): saves --server, the token (from
    /// --install-token or LUNARIS_INSTALL_TOKEN), a new agent key and
    /// --name in the config, and sets up the start: Linux the systemd user
    /// service lunaris-agent.service (desktop session; a login autostart
    /// entry where systemd is missing), or with --system the system service
    /// lunaris-agent-system.service; on Windows the service does. The token
    /// is used once, at the first connection.
    #[arg(long, requires = "server")]
    enroll: bool,

    /// With --enroll: the install token. LUNARIS_INSTALL_TOKEN is better
    /// (a command line is visible to other users).
    #[arg(long, requires = "enroll")]
    install_token: Option<String>,

    /// With --enroll: SHA-256 (hex) of the server's TLS certificate, trusted
    /// for it (a self-signed certificate, pinned without asking)
    #[arg(long, requires = "enroll", value_name = "HEX")]
    server_cert_sha256: Option<String>,

    /// The agent window's start at login (removed): Linux and macOS run the
    /// agent as with --cli; Windows exits (the service runs the agent).
    #[arg(long, hide = true)]
    minimized: bool,

    /// Try to open each encoder at 1920x1080@60 and print which ones work, then exit
    #[arg(long)]
    probe_encoders: bool,

    /// Accept direct connections (no server) and print the addresses and the
    /// access password (saved in the config, generated on first use)
    #[arg(long)]
    direct: bool,

    /// Port for direct connections (default 8091)
    #[arg(long)]
    direct_port: Option<u16>,

    /// Allow the administrators of the Lunaris server to change this
    /// computer's settings (and see its access password); saved in the
    /// config as `allow_server_settings`. Works with --enroll too.
    #[arg(long, conflicts_with = "deny_server_settings")]
    allow_server_settings: bool,

    /// The server may only read this computer's settings (the default);
    /// saved in the config.
    #[arg(long)]
    deny_server_settings: bool,

    /// The "accept this connection?" questions go to stdout and the answers
    /// come on stdin (see approval.rs); logs go to stderr. Without it the
    /// approval helper (lunaris-approval-helper) answers, when one runs.
    #[arg(long, hide = true)]
    approval_stdio: bool,

    /// The Lunaris server's gateway (see bridge_agent.rs): listens on
    /// 127.0.0.1 only and shows sources only, never this computer.
    #[arg(long, hide = true)]
    bridge_only: bool,

    /// Windows: started by the Service Control Manager (see win_service.rs)
    #[cfg(target_os = "windows")]
    #[arg(long, hide = true)]
    service_run: bool,

    /// Windows: send Ctrl+C to the host process with this id (the service
    /// runs this program again for it, see win_service.rs)
    #[cfg(target_os = "windows")]
    #[arg(long, hide = true, value_name = "PID")]
    terminate: Option<u32>,

    /// Windows: install the service that runs the host as SYSTEM in the
    /// console session, start it and make it start at boot (needs an
    /// elevated prompt)
    #[cfg(target_os = "windows")]
    #[arg(long)]
    install_service: bool,

    /// Windows: stop and remove that service (needs an elevated prompt)
    #[cfg(target_os = "windows")]
    #[arg(long)]
    uninstall_service: bool,
}

#[derive(clap::Subcommand, Debug)]
enum Command {
    /// Show the server, whether the agent runs and is connected, the
    /// addresses for direct connections and the settings (the access
    /// password only with --show-password)
    Status {
        /// Also print the access password
        #[arg(long)]
        show_password: bool,
    },
    /// Change one setting in the config. A running agent uses it once it
    /// starts again (systemctl --user restart lunaris-agent, systemctl
    /// restart lunaris-agent-system, or restart the Windows service).
    /// The server's administrators change the same settings on its web
    /// page when this computer allows it (server-settings allow).
    Set {
        #[arg(value_enum)]
        key: cli::SettingKey,
        /// on|off for switches; `password`: `random`, or `-` to read it from
        /// stdin; `server-token`: `-` (read from stdin)
        value: String,
    },
}

/// `--probe-encoders`: open each codec's encoder for real and report it.
fn probe_encoders() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let _ = tracing_subscriber::registry()
        .with(tracing_subscriber::EnvFilter::new(
            std::env::var("RUST_LOG").unwrap_or_else(|_| "off,ffmpeg=warn,lunaris_media=error".into()),
        ))
        .with(tracing_subscriber::fmt::layer())
        .try_init();
    for codec in [
        lunaris_media::VideoCodec::H264,
        lunaris_media::VideoCodec::H265,
        lunaris_media::VideoCodec::AV1,
    ] {
        match lunaris_media::encode::probe_encoder(codec, 1920, 1080, 60) {
            Some(e) => println!("{:?}: {} ({:?})", codec, e.name, e.hw_type),
            None => println!("{:?}: no working encoder", codec),
        }
    }
    Ok(())
}

/// The config of the system service (`--system`), where there is one.
pub fn system_config_path() -> Option<std::path::PathBuf> {
    #[cfg(target_os = "linux")]
    {
        Some(std::path::PathBuf::from(SYSTEM_DATA_DIR).join("agent_config.json"))
    }
    #[cfg(target_os = "windows")]
    {
        Some(win_service::data_dir().join("agent_config.json"))
    }
    #[cfg(not(any(target_os = "linux", target_os = "windows")))]
    {
        None
    }
}

/// Linux: the data folder of `lunaris-agent-system.service` (its
/// StateDirectory), owned by the `lunaris-agent` user.
#[cfg(target_os = "linux")]
pub const SYSTEM_DATA_DIR: &str = "/var/lib/lunaris-agent";

/// Runs the agent with command-line arguments (the first one is the program
/// name): the `lunaris-agent` program.
pub fn run<I, T>(args: I) -> std::process::ExitCode
where
    I: IntoIterator<Item = T>,
    T: Into<std::ffi::OsString> + Clone,
{
    let args: Vec<std::ffi::OsString> = args.into_iter().map(Into::into).collect();
    // The root capture service (Linux, see capture_service.rs).
    #[cfg(target_os = "linux")]
    if let Some(code) = capture_service::dispatch(&args) {
        return code;
    }
    let args = match Args::try_parse_from(args) {
        Ok(args) => args,
        Err(e) => e.exit(),
    };
    // Service commands (Windows): same order as sunshinesvc.cpp main().
    #[cfg(target_os = "windows")]
    {
        if let Some(pid) = args.terminate {
            win_service::terminate_main(pid);
        }
        if args.install_service || args.uninstall_service {
            let result = if args.install_service {
                win_service::install_service(crate::pairing::config_path())
            } else {
                win_service::uninstall_service()
            };
            return match result {
                Ok(()) => std::process::ExitCode::SUCCESS,
                Err(e) => {
                    eprintln!("error: {e}");
                    std::process::ExitCode::FAILURE
                }
            };
        }
        if args.service_run {
            win_service::service_run_main();
        }
        // The start at login of the agent window that once was (HKCU Run,
        // `--minimized`): the LunarisAgentService service runs the agent
        // now, a second one in the user's session would only compete.
        if args.minimized && !args.cli {
            eprintln!("The Lunaris agent has no window any more: it runs as the LunarisAgentService service.");
            return std::process::ExitCode::SUCCESS;
        }
    }
    let runtime = match tokio::runtime::Runtime::new() {
        Ok(rt) => rt,
        Err(e) => {
            eprintln!("error: cannot start the async runtime: {e}");
            return std::process::ExitCode::FAILURE;
        }
    };
    match runtime.block_on(main_async(args)) {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e}");
            std::process::ExitCode::FAILURE
        }
    }
}

async fn main_async(args: Args) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    if args.probe_encoders {
        return probe_encoders();
    }
    let config_arg = if args.system {
        let path = system_config_path().ok_or("--system: there is no system service on this platform")?;
        Some(path.to_string_lossy().into_owned())
    } else {
        args.config.clone()
    };
    crate::pairing::set_config_path(config_arg);
    let config_path = crate::pairing::config_path().to_string();

    // If import_config is specified, we perform the import first
    if let Some(ref import_path) = args.import_config {
        let _ = tracing_subscriber::registry()
            .with(tracing_subscriber::EnvFilter::new(
                std::env::var("RUST_LOG").unwrap_or_else(|_| "info,agent=info,webrtc_sctp=off,dtls=error,rtc_sctp=off,rtc_dtls=error".into()),
            ))
            .with(tracing_subscriber::fmt::layer())
            .try_init();

        info!(
            "Importing configuration from {} into {}...",
            import_path, config_path
        );
        if let Err(e) = crate::pairing::import_config_file(import_path, &config_path) {
            error!("Failed to import configuration: {:?}", e);
            std::process::exit(1);
        }
        info!("Configuration imported and merged successfully!");
    }

    match &args.command {
        Some(Command::Status { show_password }) => return cli::status(&config_path, *show_password).await,
        Some(Command::Set { key, value }) => return cli::set(&config_path, *key, value),
        None => {}
    }

    if args.enroll {
        return enroll(&args, &config_path);
    }

    // The agent itself (`--cli`, or nothing given: there is no window).
    let _ = tracing_subscriber::registry()
        .with(tracing_subscriber::EnvFilter::new(
            std::env::var("RUST_LOG").unwrap_or_else(|_| {
                "info,agent=info,webrtc_sctp=off,dtls=error,rtc_sctp=off,rtc_dtls=error".into()
            }),
        ))
        .with(tracing_subscriber::fmt::layer().with_writer(if args.approval_stdio || args.bridge_only {
            tracing_subscriber::fmt::writer::BoxMakeWriter::new(std::io::stderr)
        } else {
            tracing_subscriber::fmt::writer::BoxMakeWriter::new(std::io::stdout)
        }))
        .try_init();
    if args.approval_stdio {
        approval::attach_stdio();
    }
    // One agent per config (single.rs); a login entry that started a
    // second one beside the systemd service is fixed for the next login.
    crate::single::hold(&config_path).await;
    #[cfg(target_os = "linux")]
    if !args.system && !args.bridge_only {
        autostart::repair_login_entry();
    }

    if args.minimized && !args.cli {
        info!("Started by the agent window's old login entry (--minimized): running the agent without a window");
    }
    info!("Launching Lunaris Agent in CLI mode...");

    // --name, else the name saved by --enroll, else the host name.
    let saved_name = crate::pairing::load_config(&config_path).map(|c| c.name).unwrap_or_default();
    let name = args.name.clone().or_else(|| Some(saved_name.trim().to_string()).filter(|n| !n.is_empty())).unwrap_or_else(|| {
        hostname::get()
            .map(|h| h.to_string_lossy().into_owned())
            .unwrap_or_else(|_| "LunarisHost".to_string())
    });

    if args.pair {
        info!("lunaris-media is the streaming backend.");
        return Ok(());
    }

    if args.bridge_only {
        // Its own config file, made on the first start (the id it is known
        // by stays the same, and the sources added through it are kept there).
        let config = bridge_agent::load_or_make_config(&config_path)?;
        return bridge_agent::run(config, name).await;
    }

    // Load the config; a first start without one makes a new one (no
    // server: direct connections only, once turned on), as the agent
    // window did. One that exists but can't be read is left alone.
    let mut config = if std::path::Path::new(&config_path).exists() {
        match crate::pairing::load_config(&config_path) {
            Ok(c) => c,
            Err(e) => {
                error!("Failed to load agent config from {}: {:?}", config_path, e);
                std::process::exit(1);
            }
        }
    } else {
        let config = crate::pairing::new_config();
        if let Some(dir) = std::path::Path::new(&config_path).parent().filter(|d| !d.as_os_str().is_empty()) {
            let _ = std::fs::create_dir_all(dir);
        }
        match crate::pairing::save_config(&config, &config_path) {
            Ok(()) => info!("No config yet: made a new one in {} (join a server with --enroll)", config_path),
            Err(e) => warn!("Could not save a new config in {}: {}", config_path, e),
        }
        config
    };

    if let Some(allow) = server_settings_flag(&args) {
        config.allow_server_settings = allow;
        match crate::pairing::update_config_file(&config_path, &config, |c| c.allow_server_settings = allow) {
            Ok(()) => info!("Changes of this computer's settings from the server: {}", if allow { "allowed" } else { "not allowed" }),
            Err(e) => warn!("Could not save allow_server_settings to {}: {}", config_path, e),
        }
    }

    if args.direct || args.direct_port.is_some() {
        config.direct_enabled = true;
        if let Some(port) = args.direct_port {
            config.direct_port = port;
        }
        if crate::pairing::ensure_direct_password(&mut config) {
            if let Err(e) = crate::pairing::save_config(&config, &config_path) {
                warn!("Could not save the direct-access password to {}: {}", config_path, e);
            }
        }
    }
    if config.direct_enabled {
        println!("Direct connections — connect a client to one of:");
        for addr in direct::local_addresses().await {
            println!("    {}:{}", addr, config.direct_port);
        }
        // Printed, not logged, and only to a terminal: when stdout is a file
        // or a pipe (the Windows service's log, the journal) it would be
        // stored. `lunaris-agent status --show-password` shows it later.
        use std::io::IsTerminal;
        if std::io::stdout().is_terminal() {
            println!("Access password: {}", config.direct_password);
        } else {
            println!("Access password: not shown here (output is not a terminal); see `lunaris-agent status --show-password`");
        }
    }

    // A daemon: the person here answers through the approval helper, if
    // one connects (approval.rs); `--approval-stdio` answers on stdio.
    if !args.approval_stdio {
        approval::spawn_helper_listener();
    }

    // A package upgrade or a new AppImage does not restart this headless
    // agent: it starts the new program itself once its file was replaced.
    // Not with approval on stdio: whoever runs it restarts it.
    #[cfg(target_os = "linux")]
    if !args.approval_stdio {
        updater::restart::watch_executable(std::time::Duration::from_secs(30));
    }

    // For `lunaris-agent status`: whether this agent runs and is connected.
    let _status = status::RunState::start(&config_path, &config.server_url);

    run_agent(config, name).await
}

/// Windows: DLLs loaded later by name (the encoders' runtimes: NVENC's
/// nvEncodeAPI64.dll, AMF's amfrt64.dll, FFmpeg's own loads) are looked
/// for only in System32 and in this program's folder (FFmpeg's DLLs and the
/// C runtime are installed there), no longer in the current folder or the
/// folders of the PATH, where a user may be able to plant one that the
/// service, running as SYSTEM, would load. DLLs this program imports
/// directly are found by the loader before main runs and are not affected.
/// One loader that may be affected: lunaris-media's native Quick Sync
/// encoder loads the oneVPL dispatcher `libvpl.dll`, which the installer
/// does not ship; one found only through the PATH (an Intel oneAPI install)
/// is no longer used, the FFmpeg encoders remain.
#[cfg(target_os = "windows")]
pub fn harden_dll_search() -> bool {
    use windows::Win32::System::LibraryLoader::{
        SetDefaultDllDirectories, LOAD_LIBRARY_SEARCH_APPLICATION_DIR, LOAD_LIBRARY_SEARCH_SYSTEM32,
    };
    // Before logging is set up: the outcome is only returned.
    unsafe { SetDefaultDllDirectories(LOAD_LIBRARY_SEARCH_SYSTEM32 | LOAD_LIBRARY_SEARCH_APPLICATION_DIR).is_ok() }
}

/// The `ws://`/`wss://` address the agent connects to for a server
/// address as configured: `http://` is `ws://`, `https://` is `wss://`, and
/// one without a scheme (`host:port`) is `wss://` (before 0.7.1 it was
/// `ws://`: unencrypted unless asked for).
pub(crate) fn server_ws_url(given: &str) -> String {
    let given = given.trim();
    if let Some(rest) = given.strip_prefix("http://") {
        format!("ws://{rest}")
    } else if let Some(rest) = given.strip_prefix("https://") {
        format!("wss://{rest}")
    } else if given.starts_with("ws://") || given.starts_with("wss://") {
        given.to_string()
    } else {
        format!("wss://{given}")
    }
}

/// How the connection to a server (a `server_ws_url`) is protected.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ServerTransport {
    Tls,
    /// Plain ws:// to this computer itself: nothing crosses a network.
    PlainLoopback,
    /// Plain ws:// over the network.
    Plain,
}

pub(crate) fn server_transport(ws_url: &str) -> ServerTransport {
    let Some(rest) = ws_url.trim().strip_prefix("ws://") else { return ServerTransport::Tls };
    let authority = rest.split(['/', '?']).next().unwrap_or("");
    let host = match authority.strip_prefix('[') {
        Some(v6) => v6.split(']').next().unwrap_or(""),
        None => authority.rsplit_once(':').map(|(h, _)| h).unwrap_or(authority),
    };
    let loopback = host.eq_ignore_ascii_case("localhost") || host.parse::<std::net::IpAddr>().is_ok_and(|ip| ip.is_loopback());
    if loopback {
        ServerTransport::PlainLoopback
    } else {
        ServerTransport::Plain
    }
}

/// The WebSocket request to `/ws/agent`: the secrets go both in the query
/// (`token`, `install_token`, `key`; what servers before 0.7.1 read) and as
/// the headers `X-Lunaris-Token`, `X-Lunaris-Install-Token` and
/// `X-Lunaris-Agent-Key`, which newer servers read and which no proxy or
/// access log records the way it records a URL. The query copy can go once
/// servers older than 0.7.1 are no longer supported.
fn server_request(
    url: &str,
    token: &str,
    agent_key: &str,
    install_token: &str,
) -> Result<tokio_tungstenite::tungstenite::handshake::client::Request, Box<dyn std::error::Error + Send + Sync>> {
    use tokio_tungstenite::tungstenite::client::IntoClientRequest;
    use tokio_tungstenite::tungstenite::http::HeaderValue;
    let mut request = url.into_client_request()?;
    for (name, value) in [("X-Lunaris-Token", token), ("X-Lunaris-Agent-Key", agent_key), ("X-Lunaris-Install-Token", install_token)] {
        let value = value.trim();
        if value.is_empty() {
            continue;
        }
        // One a header cannot carry still goes in the query, as before.
        let Ok(mut header) = HeaderValue::from_str(value) else { continue };
        header.set_sensitive(true);
        request.headers_mut().insert(name, header);
    }
    Ok(request)
}

/// `ws://`/`wss://` address of a server as given (`http(s)://` and a bare
/// `host:port`, which is `wss://`, accepted), checked: Err when it isn't one.
fn normalize_server_url(given: &str) -> Result<(String, String), String> {
    let given = given.trim().trim_end_matches('/');
    let url = server_ws_url(given);
    let uri: tokio_tungstenite::tungstenite::http::Uri =
        url.parse().map_err(|e| format!("not a server address: {given} ({e})"))?;
    let tls = uri.scheme_str() == Some("wss");
    let host = uri
        .host()
        .filter(|h| !h.is_empty())
        .ok_or_else(|| format!("no host in the server address {given}"))?
        .trim_start_matches('[')
        .trim_end_matches(']')
        .to_string();
    if uri.path() != "/" && !uri.path().is_empty() || uri.query().is_some() {
        return Err(format!("the server address has a path or query: {given}"));
    }
    // As server_stream says it (the key of a pinned certificate).
    let host_port = format!("{}:{}", host, uri.port_u16().unwrap_or(if tls { 443 } else { 80 }));
    Ok((url, host_port))
}

/// `--enroll`: joins this computer to a server with an install token (see
/// the flag). Never prints the token.
fn enroll(args: &Args, config_path: &str) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    #[cfg(target_os = "linux")]
    if args.system {
        // The service's folder, made for its user before the token goes in.
        autostart::prepare_system_dir()?;
    }
    let server_url = enroll_config(args, config_path)?;
    println!("Saved the server {} in {}.", server_url, config_path);
    #[cfg(target_os = "linux")]
    {
        let started = if args.system {
            autostart::enable_system_service()
        } else {
            autostart::enable_user_start(config_path)
        };
        match started {
            Ok(note) => println!("{note}"),
            Err(e) => eprintln!("warning: could not set up the start of the agent: {e}"),
        }
    }
    #[cfg(target_os = "macos")]
    crate::pairing::enable_macos_login_start();
    #[cfg(target_os = "windows")]
    println!(
        "The LunarisAgentService service runs the agent with {}; restart it to join now (`{} --install-service` from an elevated prompt installs and restarts it).",
        system_config_path().map(|p| p.display().to_string()).unwrap_or_default(),
        std::env::current_exe().map(|p| p.display().to_string()).unwrap_or_else(|_| "lunaris-agent.exe".into()),
    );
    println!("This computer joins the server the next time the agent starts (the install token is used then).");
    Ok(())
}

/// The config part of `--enroll`; returns the server address saved.
fn enroll_config(args: &Args, config_path: &str) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
    let (server_url, host_port) = normalize_server_url(args.server.as_deref().unwrap_or_default())?;
    let token = args
        .install_token
        .clone()
        .or_else(|| std::env::var("LUNARIS_INSTALL_TOKEN").ok())
        .map(|t| t.trim().to_string())
        .filter(|t| !t.is_empty())
        .ok_or("no install token: set LUNARIS_INSTALL_TOKEN (or --install-token)")?;
    let token_ok = token.strip_prefix("lit_").is_some_and(|rest| {
        (16..=128).contains(&rest.len()) && rest.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
    });
    if !token_ok {
        return Err("this is not an install token (they start with lit_)".into());
    }
    let pin = match args.server_cert_sha256.as_deref().map(str::trim).filter(|p| !p.is_empty()) {
        Some(p) if p.len() == 64 && p.bytes().all(|b| b.is_ascii_hexdigit()) => Some(p.to_ascii_lowercase()),
        Some(_) => return Err("--server-cert-sha256 takes 64 hex characters".into()),
        None => None,
    };
    if let Some(name) = &args.name {
        if name.trim().is_empty() || name.len() > 128 {
            return Err("--name must be 1 to 128 bytes".into());
        }
    }

    let path = std::path::Path::new(config_path);
    if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
        #[cfg(target_os = "windows")]
        {
            // The service's folder: made for SYSTEM and administrators only
            // before the config with the token and key goes in.
            let service_dir = win_service::data_dir();
            if dir.to_string_lossy().eq_ignore_ascii_case(&service_dir.to_string_lossy()) {
                win_service::secure_data_dir()?;
            }
        }
        std::fs::create_dir_all(dir)?;
    }
    // An existing config keeps its id, key and settings; one that can't be
    // read is not overwritten.
    let mut config = if path.exists() {
        crate::pairing::load_config(config_path).map_err(|e| format!("cannot read {config_path}: {e}"))?
    } else {
        crate::pairing::new_config()
    };
    if config.client_unique_id.trim().is_empty() {
        config.client_unique_id = uuid::Uuid::new_v4().to_string().to_uppercase();
    }
    let same_server = normalize_server_url(&config.server_url).map(|(u, _)| u == server_url).unwrap_or(false);
    if !same_server {
        // A certificate pinned for another server says nothing about this one.
        config.server_cert_sha256.clear();
        config.server_cert_host.clear();
    }
    if let Some(pin) = pin {
        config.server_cert_sha256 = pin;
        config.server_cert_host = host_port;
    }
    config.server_url = server_url.clone();
    // The install token replaces the server's agent token: this computer
    // signs in with its own key once joined.
    config.server_token.clear();
    config.install_token = token;
    crate::pairing::ensure_agent_key(&mut config);
    if let Some(name) = &args.name {
        config.name = name.trim().to_string();
    }
    config.autostart = true;
    if let Some(allow) = server_settings_flag(args) {
        config.allow_server_settings = allow;
    }
    crate::pairing::save_config(&config, config_path)?;
    Ok(server_url)
}

/// `--allow-server-settings` / `--deny-server-settings`, when given.
fn server_settings_flag(args: &Args) -> Option<bool> {
    if args.allow_server_settings {
        Some(true)
    } else if args.deny_server_settings {
        Some(false)
    } else {
        None
    }
}

pub async fn run_agent(
    mut config: AgentConfig,
    name: String,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    CONNECTED_TO_SERVER.store(false, std::sync::atomic::Ordering::SeqCst);
    if crate::pairing::ensure_client_id(&mut config) {
        let id = config.client_unique_id.clone();
        warn!("The config had no computer id; using a new one: {}", id);
        let saved = crate::pairing::update_config_file(crate::pairing::config_path(), &config, |c| {
            if c.client_unique_id.trim().is_empty() {
                c.client_unique_id = id.clone();
            }
        });
        if let Err(e) = saved {
            warn!("Could not save the new computer id to {}: {}", crate::pairing::config_path(), e);
        }
    }

    let hub = hub::Hub::new(config.client_unique_id.clone());
    let (agent_tx, router) = hub::spawn_router(hub.clone());
    let (inbound_tx, mut inbound_rx) = mpsc::unbounded_channel::<SignalingMessage>();
    let mut tasks = AbortOnDrop(vec![router]);
    // A set-up of a computer being added (a pairing that waits for its PIN)
    // does not outlive the agent that started it.
    let _setups = crate::connector::sources::StopSetupsOnDrop(crate::pairing::config_path().to_string());
    // The config as the local parts run with it, for the server connection
    // (which stays when they start again after a change from the server).
    let (config_tx, config_rx) = tokio::sync::watch::channel(config.clone());
    // Servers connected to this computer by its address: they stay while the
    // local parts start again, and end with this agent.
    let links = direct::Links::default();
    let _links_end = CloseLinksOnDrop(links.clone());
    // Changes from the server saved from now on.
    let mut settings_changes = crate::settings::changes();

    if config.server_url.trim().is_empty() {
        info!("No Lunaris server configured: direct connections only.");
    } else {
        if crate::pairing::ensure_agent_key(&mut config) {
            let key = config.agent_key.clone();
            if let Err(e) = crate::pairing::update_config_file(crate::pairing::config_path(), &config, |c| c.agent_key = key) {
                // A key the server stores but this agent forgets on restart
                // would lock it out: register without one (as older agents do).
                error!(
                    "Could not save the agent's server key to {} ({}): connecting without a key until it can be saved",
                    crate::pairing::config_path(),
                    e
                );
                config.agent_key.clear();
            }
        }
        tasks.0.push(tokio::spawn(run_server_connection(
            config.clone(),
            name.clone(),
            hub.clone(),
            inbound_tx.clone(),
            config_rx,
        )));
    }

    info!("Starting Host Agent: {} ({})", name, config.client_unique_id);
    loop {
        // Before any session can create a capture backend. Like the other
        // settings, a change applies when the agent (re)starts (after
        // `lunaris-agent set`, the service is restarted); a change from the
        // server (settings.rs) starts these local parts again.
        apply_wayland_capture(&config);
        let local = start_local_services(&mut config, &name, &hub, &inbound_tx, &links);
        // `inbound_tx` stays alive here, so the session logic runs until
        // this future is dropped (GUI stop) even when nothing is connected.
        let restart = tokio::select! {
            _ = run_session_logic(config.clone(), agent_tx.clone(), &mut inbound_rx) => false,
            _ = settings_changes.changed() => true,
        };
        local.stop().await;
        if !restart {
            break;
        }
        // Seen now: the config read below has it (and any made meanwhile
        // wakes the next round).
        settings_changes.borrow_and_update();
        match crate::pairing::load_config(crate::pairing::config_path()) {
            Ok(saved) => {
                let password_changed = saved.direct_password != config.direct_password;
                config = with_saved_settings(config, saved);
                let closed = links.keep_admitted(
                    config.direct_enabled,
                    password_changed,
                    approval::ApproveMode::parse(&config.approve_mode),
                );
                if closed > 0 {
                    info!("Settings changed: {} server link(s) let in under the old ones closed; they connect again", closed);
                }
            }
            Err(e) => warn!("The changed settings could not be read again ({}); going on with the ones before", e),
        }
        config_tx.send_replace(config.clone());
        info!("Settings changed: direct connections, RustDesk compatibility and sessions start again with them");
    }
    drop(inbound_tx);
    drop(tasks);
    Ok(())
}

/// `running` with the settings of `saved` (the file after a change from the
/// server): its identity and server stay as they started.
fn with_saved_settings(running: AgentConfig, saved: AgentConfig) -> AgentConfig {
    AgentConfig {
        client_unique_id: running.client_unique_id,
        server_url: running.server_url,
        server_token: running.server_token,
        agent_key: running.agent_key,
        install_token: running.install_token,
        server_cert_sha256: running.server_cert_sha256,
        server_cert_host: running.server_cert_host,
        name: running.name,
        ..saved
    }
}

/// The agent's local listeners (direct connections, RustDesk
/// compatibility), started again when the settings change. Dropped (the
/// GUI drops `run_agent` to stop the agent), they are aborted: they must
/// not go on with the old settings and keep their ports.
struct LocalServices(Vec<tokio::task::JoinHandle<()>>);

impl LocalServices {
    /// Stops them and waits until their ports are free again.
    async fn stop(mut self) {
        let tasks = std::mem::take(&mut self.0);
        for task in &tasks {
            task.abort();
        }
        for task in tasks {
            let _ = task.await;
        }
    }
}

impl Drop for LocalServices {
    fn drop(&mut self) {
        for task in &self.0 {
            task.abort();
        }
    }
}

/// Closes the agent's server links when `run_agent` stops.
struct CloseLinksOnDrop(direct::Links);

impl Drop for CloseLinksOnDrop {
    fn drop(&mut self) {
        self.0.close_all();
    }
}

/// The folder of the agent's config file (files kept with it, such as the
/// direct listener's certificate).
fn config_dir() -> std::path::PathBuf {
    match std::path::Path::new(crate::pairing::config_path()).parent() {
        Some(dir) if !dir.as_os_str().is_empty() => dir.to_path_buf(),
        _ => std::path::PathBuf::from("."),
    }
}

fn start_local_services(
    config: &mut AgentConfig,
    name: &str,
    hub: &Arc<hub::Hub>,
    inbound_tx: &mpsc::UnboundedSender<SignalingMessage>,
    links: &direct::Links,
) -> LocalServices {
    let mut tasks = Vec::new();
    if config.direct_enabled {
        if crate::pairing::ensure_direct_password(config) {
            if let Err(e) = crate::pairing::save_config(config, crate::pairing::config_path()) {
                warn!("Could not save the new direct-access password: {}", e);
            }
        }
        let port = config.direct_port;
        let direct_config = direct::DirectConfig {
            port,
            password: config.direct_password.clone(),
            approve_mode: approval::ApproveMode::parse(&config.approve_mode),
            name: name.to_string(),
            links: links.clone(),
            allow_legacy_plain: config.allow_legacy_plain_direct,
            // Next to the config: servers pin the certificate. Not in unit
            // tests, whose config path is the real user's.
            tls_dir: (!cfg!(test)).then(config_dir),
        };
        let hub = hub.clone();
        let inbound_tx = inbound_tx.clone();
        tasks.push(tokio::spawn(async move {
            if let Err(e) = direct::run_listener(direct_config, hub, inbound_tx).await {
                let msg = format!("Direct connections unavailable: cannot listen on port {}: {}", port, e);
                error!("{}", msg);
                if let Ok(mut last) = LAST_ERROR.lock() {
                    *last = Some(msg);
                }
            }
        }));
    }

    if let Some(port) = config.rustdesk_port {
        // The RustDesk client logs in with the access password.
        if crate::pairing::ensure_direct_password(config) {
            if let Err(e) = crate::pairing::save_config(config, crate::pairing::config_path()) {
                warn!("Could not save the new access password: {}", e);
            }
        }
        let rustdesk_config = rustdesk::RustdeskConfig {
            port,
            password: config.direct_password.clone(),
            approve_mode: approval::ApproveMode::parse(&config.approve_mode),
            allow_clipboard: config.allow_clipboard,
            agent_id: config.client_unique_id.clone(),
            name: name.to_string(),
            // RustDesk's file manager: its own switch, and never when file
            // transfer is off on this computer; held to `file_access`.
            file_transfer: config.rustdesk_file_transfer && config.allow_file_transfer,
            file_access: file_access(config),
            // Resolved again for each file manager: for the Windows
            // service's agent, whoever is logged on at the screen then.
            file_access_setting: Some(file_access_setting(config)),
            file_home: None,
        };
        tasks.push(tokio::spawn(async move {
            if let Err(e) = rustdesk::run_listener(rustdesk_config).await {
                error!("RustDesk compatibility unavailable: cannot listen on port {}: {}", port, e);
            }
        }));
    }
    LocalServices(tasks)
}

/// Hands the config's `wayland_capture` to lunaris-media, which reads it
/// whenever it creates a screen capture backend on a Wayland session.
fn apply_wayland_capture(config: &AgentConfig) {
    let (preference, known) = crate::pairing::wayland_capture_preference(&config.wayland_capture);
    if !known {
        warn!(
            "wayland_capture {:?} is not \"auto\" or \"portal\"; using \"auto\"",
            config.wayland_capture
        );
    }
    lunaris_media::capture::set_wayland_capture_preference(preference);
    info!("Wayland screen capture: {}", preference.as_str());
}

/// Aborts the agent's background tasks when `run_agent` stops.
struct AbortOnDrop(Vec<tokio::task::JoinHandle<()>>);

impl Drop for AbortOnDrop {
    fn drop(&mut self) {
        for task in &self.0 {
            task.abort();
        }
    }
}

/// Codecs this computer encodes in real time, as the server's
/// `codec_support` bits. H.264 software is a practical fallback; H.265/AV1
/// software is too slow for interactive remote desktop, so only advertise
/// those when a realtime-capable hardware encoder exists.
pub(crate) fn codec_support_bits() -> u32 {
    // Probing the encoders takes a while: once per run.
    static BITS: std::sync::OnceLock<u32> = std::sync::OnceLock::new();
    *BITS.get_or_init(probe_codec_support)
}

fn probe_codec_support() -> u32 {
    let encoders = lunaris_media::encode::list_available_encoders();
    let h264 = codec_supported_for_realtime(&encoders, lunaris_media::VideoCodec::H264, false);
    // The cached probe (1080p60, as `--probe-encoders`) decides H.265 and AV1.
    let h265 = codec_supported_for_realtime(
        &encoders,
        lunaris_media::VideoCodec::H265,
        hardware_encoder_opens_blocking(lunaris_media::VideoCodec::H265, 1920, 1080, 60),
    );
    let av1 = codec_supported_for_realtime(
        &encoders,
        lunaris_media::VideoCodec::AV1,
        hardware_encoder_opens_blocking(lunaris_media::VideoCodec::AV1, 1920, 1080, 60),
    );
    let mut bits: u32 = 0;
    if h264 {
        bits |= 262145;
    }
    if h265 {
        bits |= 1573632;
    }
    if av1 {
        bits |= 6488064;
    }
    bits
}

/// The server refused the agent's WebSocket handshake with this HTTP status.
#[derive(Debug)]
struct ServerRefused {
    status: u16,
    message: String,
}

impl std::fmt::Display for ServerRefused {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for ServerRefused {}

const RETRY_FIRST: std::time::Duration = std::time::Duration::from_secs(3);
/// Refused (401/403/429): every refusal counts toward the server's limit
/// for the address, which all agents behind it share.
const RETRY_MAX_REFUSED: std::time::Duration = std::time::Duration::from_secs(300);
/// Unreachable: the server comes back on its own, so not too slow.
const RETRY_MAX_UNREACHABLE: std::time::Duration = std::time::Duration::from_secs(30);

/// Wait before attempt `failures + 1` after `failures` failed attempts in a
/// row: 3 s doubling up to the cap (5 min when refused, 30 s otherwise), ±20 %
/// with `jitter` (0..1) so agents behind one address don't retry in step.
fn retry_delay(failures: u32, refused: bool, jitter: f64) -> std::time::Duration {
    let cap = if refused { RETRY_MAX_REFUSED } else { RETRY_MAX_UNREACHABLE };
    let doubled = RETRY_FIRST.as_secs_f64() * 2f64.powi(failures.saturating_sub(1).min(20) as i32);
    let base = doubled.min(cap.as_secs_f64());
    std::time::Duration::from_secs_f64(base * (0.8 + 0.4 * jitter.clamp(0.0, 1.0)))
}

fn random_unit() -> f64 {
    let b = crate::pairing::random_bytes(2);
    f64::from(u16::from_le_bytes([b[0], b[1]])) / f64::from(u16::MAX)
}

/// Keeps a connection to the signaling server, reconnecting after 3 s, and
/// backing off while the server refuses this agent or can't be reached.
async fn run_server_connection(
    mut config: AgentConfig,
    name: String,
    hub: Arc<hub::Hub>,
    inbound_tx: mpsc::UnboundedSender<SignalingMessage>,
    mut settings: tokio::sync::watch::Receiver<AgentConfig>,
) {
    let mut last_error = String::new();
    let mut failures: u32 = 0;
    loop {
        let wait = match connect_server_once(&mut config, &name, &hub, &inbound_tx, &mut settings).await {
            Ok(()) => {
                info!("Disconnected from the signaling server. Reconnecting in 3 seconds...");
                last_error.clear();
                failures = 0;
                RETRY_FIRST
            }
            Err(e) => {
                failures = failures.saturating_add(1);
                let refused = e
                    .downcast_ref::<ServerRefused>()
                    .is_some_and(|r| matches!(r.status, 401 | 403 | 429));
                let wait = retry_delay(failures, refused, random_unit());
                let msg = format!("Signaling server connection failed: {}", e);
                // Report each new error once instead of on every attempt.
                if msg != last_error {
                    error!("{}. Retrying in {} s (longer each time it fails)...", msg, wait.as_secs());
                    if let Ok(mut last) = LAST_ERROR.lock() {
                        *last = Some(msg.clone());
                    }
                    last_error = msg;
                } else {
                    debug!("{} (retrying in {} s)", msg, wait.as_secs());
                }
                wait
            }
        };
        hub.set_server(None);
        CONNECTED_TO_SERVER.store(false, std::sync::atomic::Ordering::SeqCst);
        tokio::time::sleep(wait).await;
    }
}

/// How long the server may stay silent before the connection counts as dead
/// (it pings every 15 s).
const SERVER_SILENCE_LIMIT: std::time::Duration = std::time::Duration::from_secs(60);

async fn connect_server_once(
    config: &mut AgentConfig,
    name: &str,
    hub: &Arc<hub::Hub>,
    inbound_tx: &mpsc::UnboundedSender<SignalingMessage>,
    settings: &mut tokio::sync::watch::Receiver<AgentConfig>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let codec_support = Some(codec_support_bits());

    let base_ws_url = server_ws_url(&config.server_url);
    if server_transport(&base_ws_url) == ServerTransport::Plain {
        // Kept working (existing installs name ws:// servers), said every time.
        warn!(
            "Unencrypted connection to the server {}: the agent's key and the signaling cross the network \
             readable by anyone on the way. Use the server's wss:// (https://) address: `lunaris-agent set server wss://...`",
            base_ws_url
        );
    }

    // Joining with an install token (`--enroll`): sent instead of the
    // server's agent token, until the server has taken it once.
    let install_token = config.install_token.trim().to_string();
    let token = if install_token.is_empty() { config.server_token.clone() } else { String::new() };

    // Connect to Signaling Server
    let mut server_ws_url = if let Some(support) = codec_support {
        format!(
            "{}/ws/agent?id={}&name={}&codec_support={}&multi_viewer=true&token={}",
            base_ws_url.trim_end_matches('/'),
            config.client_unique_id,
            urlencoding::encode(name),
            support,
            urlencoding::encode(&token)
        )
    } else {
        format!(
            "{}/ws/agent?id={}&name={}&multi_viewer=true&token={}",
            base_ws_url.trim_end_matches('/'),
            config.client_unique_id,
            urlencoding::encode(name),
            urlencoding::encode(&token)
        )
    };
    if !install_token.is_empty() {
        server_ws_url.push_str("&install_token=");
        server_ws_url.push_str(&urlencoding::encode(&install_token));
    }

    // The agent's own key (servers that don't know it ignore it).
    if !config.agent_key.is_empty() {
        server_ws_url.push_str("&key=");
        server_ws_url.push_str(&urlencoding::encode(&config.agent_key));
    }

    // Never log server_ws_url itself: it carries the agent token and key.
    debug!("Connecting to signaling server at {}", base_ws_url);
    // TLS first (the certificate is checked or pinned before the token is
    // sent), then the WebSocket handshake over it.
    let stream = server_stream(config, &base_ws_url).await?;
    let request = server_request(&server_ws_url, &token, &config.agent_key, &install_token)?;
    let (ws_stream, _) = tokio_tungstenite::client_async(request, stream).await.map_err(
        |e| -> Box<dyn std::error::Error + Send + Sync> {
            let status = match &e {
                tokio_tungstenite::tungstenite::Error::Http(resp) => Some(resp.status().as_u16()),
                _ => None,
            };
            let hint = refusal_hint(status, !install_token.is_empty(), !token.is_empty());
            let message = format!("WebSocket handshake with {} failed: {}{}", base_ws_url, e, hint);
            match status {
                // The retry loop backs off on refusals.
                Some(status) => Box::new(ServerRefused { status, message }),
                None => anyhow::anyhow!(message).into(),
            }
        },
    )?;
    info!("Connected to signaling server!");
    CONNECTED_TO_SERVER.store(true, std::sync::atomic::Ordering::SeqCst);
    if !install_token.is_empty() {
        // Joined: from now on the key alone signs in.
        info!("Joined the server with the install token; it is no longer needed");
        config.install_token.clear();
        if let Err(e) = crate::pairing::update_config_file(crate::pairing::config_path(), config, |c| c.install_token.clear()) {
            // Harmless: the server accepts the token again for this key
            // without counting a use.
            warn!("Could not remove the used install token from {}: {}", crate::pairing::config_path(), e);
        }
    }

    let (mut ws_write, mut ws_read) = ws_stream.split();
    let (server_tx, mut server_rx) = mpsc::unbounded_channel::<AgentMessage>();
    hub.set_server(Some(server_tx.clone()));
    // The server is told which other computers this one shows, and which
    // kinds it can show (again whenever it changes them).
    // `server_sessions` as the agent's local parts run with it now (a
    // change from the server applies here too, without a new connection).
    let mut ask = settings.borrow_and_update().server_sessions.trim() == "ask";
    let mut manageable = !ask;
    let server_tx_for_sources = server_tx.clone();
    // Again when a set-up (a pairing) has added one, whoever still asks about it.
    let mut _announcing = crate::connector::sources::announce_changes(server_tx.clone(), manageable);
    crate::direct::refresh_local_addresses().await;
    let _ = server_tx.send(crate::connector::sources::announcement(manageable));
    let mut gate = ServerGate::new(ask, server_tx.clone(), inbound_tx.clone());

    // Outbound: the hub routes the session logic's replies for server clients here.
    let write_task = tokio::spawn(async move {
        while let Some(msg) = server_rx.recv().await {
            match serde_json::to_string(&msg) {
                Ok(json) => {
                    if let Err(e) = ws_write.send(WsMessage::Text(json.into())).await {
                        error!("WebSocket write error: {:?}", e);
                        break;
                    }
                }
                Err(e) => error!("Failed to serialize outbound message: {:?}", e),
            }
        }
    });

    loop {
        let message_result = tokio::select! {
            // The server pings every 15 s: silence means the connection died
            // without closing (this computer slept or changed network), and
            // waiting on it would keep this computer away until TCP gives up.
            message = tokio::time::timeout(SERVER_SILENCE_LIMIT, ws_read.next()) => match message {
                Ok(Some(message)) => message,
                Ok(None) => break,
                Err(_) => {
                    warn!("Nothing from the signaling server for {} s: reconnecting", SERVER_SILENCE_LIMIT.as_secs());
                    break;
                }
            },
            changed = settings.changed() => {
                if changed.is_err() {
                    // The agent stops.
                    break;
                }
                let now_ask = settings.borrow_and_update().server_sessions.trim() == "ask";
                if now_ask != ask {
                    // The sessions ended with the change: what the person
                    // here granted before goes with them.
                    ask = now_ask;
                    manageable = !ask;
                    gate = ServerGate::new(ask, server_tx.clone(), inbound_tx.clone());
                    _announcing = crate::connector::sources::announce_changes(server_tx.clone(), manageable);
                    let _ = server_tx.send(crate::connector::sources::announcement(manageable));
                }
                continue;
            }
        };
        let text = match message_result {
            Ok(WsMessage::Text(text)) => text,
            Ok(_) => continue,
            Err(e) => {
                error!("WebSocket read error: {:?}", e);
                break;
            }
        };
        match serde_json::from_str::<ServerToAgentMessage>(&text) {
            Ok(ServerToAgentMessage::Registered { success }) => {
                info!("Registration success: {}", success);
            }
            Ok(ServerToAgentMessage::Signaling(sig)) => gate.pass(sig),
            Ok(ServerToAgentMessage::ChangeSource { request_id, change, requester }) => {
                // A computer that asks at its screen before each connection
                // is not changed from the server: its sources are added in
                // the Lunaris app on it. Adding can wait a few seconds for a
                // set-up's first step, so not on this loop.
                let tx = server_tx_for_sources.clone();
                tokio::task::spawn_blocking(move || {
                    for message in crate::connector::sources::reply(request_id, &change, requester.as_deref(), "the server", manageable) {
                        let _ = tx.send(message);
                    }
                });
            }
            Ok(ServerToAgentMessage::Settings { request_id, request, requester }) => {
                let tx = server_tx_for_sources.clone();
                tokio::task::spawn_blocking(move || {
                    let answer = crate::settings::reply(
                        request_id,
                        &request,
                        requester.as_deref(),
                        &crate::settings::Channel::server(),
                        crate::pairing::config_path(),
                    );
                    let _ = tx.send(answer);
                });
            }
            // What a newer server may send is not written out: it can hold
            // what a person typed.
            Err(e) => error!("A message from the server was not understood: {}", e),
        }
    }

    write_task.abort();
    hub.set_server(None);
    CONNECTED_TO_SERVER.store(false, std::sync::atomic::Ordering::SeqCst);
    Ok(())
}

/// Sessions that arrive through the server: passed on as they come
/// (`server_sessions` "trusted"), or, with "ask", each client connection
/// that wants the screen or files is put to the person here first, the way
/// direct.rs does for a server linked without the password.
struct ServerGate {
    ask: bool,
    server_tx: mpsc::UnboundedSender<AgentMessage>,
    inbound_tx: mpsc::UnboundedSender<SignalingMessage>,
    /// What the person here granted each client connection (server
    /// connection id): the screen and files are asked for separately.
    accepted: Arc<std::sync::Mutex<std::collections::HashSet<(String, approval::Kind)>>>,
    /// Questions open, each withdrawn through its Notify when the client
    /// leaves first.
    asking: Arc<std::sync::Mutex<std::collections::HashMap<(String, approval::Kind), Arc<tokio::sync::Notify>>>>,
    ask_fn: AskFn,
}

/// Puts a question to the person here: None when withdrawn (the Notify).
type AskFn = Arc<
    dyn Fn(String, approval::Kind, Arc<tokio::sync::Notify>) -> std::pin::Pin<Box<dyn std::future::Future<Output = Option<approval::Decision>> + Send>>
        + Send
        + Sync,
>;

impl ServerGate {
    fn new(
        ask: bool,
        server_tx: mpsc::UnboundedSender<AgentMessage>,
        inbound_tx: mpsc::UnboundedSender<SignalingMessage>,
    ) -> Self {
        if ask {
            info!("Sessions through the server are accepted at this computer (server_sessions = \"ask\")");
        }
        let ask_fn: AskFn = Arc::new(|requester, kind, cancel| {
            Box::pin(async move { approval::ask_unless(&requester, kind, cancel.notified()).await })
        });
        Self { ask, server_tx, inbound_tx, accepted: Arc::default(), asking: Arc::default(), ask_fn }
    }

    fn pass(&self, sig: SignalingMessage) {
        if !self.ask {
            let _ = self.inbound_tx.send(sig);
            return;
        }
        let Some(client) = hub::target_of(&sig).map(str::to_owned) else {
            let _ = self.inbound_tx.send(sig);
            return;
        };
        if let SignalingMessage::EndSession { .. } = sig {
            // The client is gone: withdraw its questions, forget its grants.
            // Under the lock, so an answer arriving now is ordered before or
            // after this, never in between.
            let mut asking = self.asking.lock().unwrap();
            asking.retain(|(c, _), cancel| {
                if *c == client {
                    cancel.notify_one();
                    false
                } else {
                    true
                }
            });
            self.accepted.lock().unwrap().retain(|(c, _)| *c != client);
            let _ = self.inbound_tx.send(sig);
            return;
        }
        let kind = approval::kind_of(&sig);
        let pass = {
            let accepted = self.accepted.lock().unwrap();
            match (kind, &sig) {
                (Some(kind), _) => accepted.contains(&(client.clone(), kind)),
                // Stopping the stream is for a client allowed to see it.
                (None, SignalingMessage::StopActiveStream { .. }) => {
                    accepted.contains(&(client.clone(), approval::Kind::Control))
                }
                // Answers, candidates and the like: from a client granted something.
                (None, _) => approval::allowed_before_approval(&sig) || accepted.iter().any(|(c, _)| *c == client),
            }
        };
        if pass {
            let _ = self.inbound_tx.send(sig);
            return;
        }
        let Some(kind) = kind else {
            debug!("Server client {} not accepted yet: dropping a {:?} message", client, std::mem::discriminant(&sig));
            return;
        };
        let key = (client.clone(), kind);
        let cancel = Arc::new(tokio::sync::Notify::new());
        {
            let mut asking = self.asking.lock().unwrap();
            if asking.contains_key(&key) {
                return;
            }
            asking.insert(key.clone(), cancel.clone());
        }
        let requester = server_requester_label(&sig);
        let status = |client: &str, s: &str| {
            AgentMessage::Signaling(SignalingMessage::AccessRequest { target_id: client.to_string(), status: s.to_string() })
        };
        let _ = self.server_tx.send(status(&client, "waiting"));
        let (server_tx, inbound_tx) = (self.server_tx.clone(), self.inbound_tx.clone());
        let (accepted, asking) = (self.accepted.clone(), self.asking.clone());
        let question = (self.ask_fn)(requester, kind, cancel.clone());
        tokio::spawn(async move {
            let decision = question.await;
            let mut asking = asking.lock().unwrap();
            // Gone meanwhile (its EndSession withdrew the question): nothing
            // to deliver, nobody to tell.
            let still_here = matches!(asking.get(&key), Some(c) if Arc::ptr_eq(c, &cancel));
            let Some(decision) = decision.filter(|_| still_here) else { return };
            asking.remove(&key);
            let _ = server_tx.send(status(&client, decision.status()));
            if decision == approval::Decision::Accepted {
                accepted.lock().unwrap().insert(key);
                let _ = inbound_tx.send(sig);
            } else if kind == approval::Kind::Control {
                // The server marked the computer busy for this request.
                let _ = server_tx.send(AgentMessage::Signaling(SignalingMessage::EndSession { target_id: client }));
            }
        });
    }
}

/// Who asks through the server, as shown to the person here: the name the
/// server gives, cleaned like a direct client's (no control or direction
/// characters that could fake the rest of the question, not too long).
fn server_requester_label(sig: &SignalingMessage) -> String {
    let name = match sig {
        SignalingMessage::IncomingSession { requester: Some(r), .. }
        | SignalingMessage::IncomingFileSession { requester: Some(r), .. } => crate::direct::clean_name(r),
        _ => String::new(),
    };
    if name.is_empty() {
        "a user of the Lunaris server".to_string()
    } else {
        format!("{} (through the Lunaris server)", name)
    }
}

impl Drop for ServerGate {
    /// The server connection is gone: its questions are withdrawn.
    fn drop(&mut self) {
        for (_, cancel) in self.asking.lock().unwrap().drain() {
            cancel.notify_one();
        }
        self.accepted.lock().unwrap().clear();
    }
}

/// What a refused registration means, from what was sent (the server only
/// answers 401/429; its log has the reason).
fn refusal_hint(status: Option<u16>, sent_install_token: bool, sent_server_token: bool) -> &'static str {
    match status {
        Some(401) if sent_install_token => {
            " (the server refused the install token: it is expired, revoked, used up or not from this server, \
             or this computer's id is taken. Ask for a new install token and run the installer again)"
        }
        Some(401) if sent_server_token => " (wrong agent connection token, or another key is registered for this computer's id)",
        Some(401) => {
            " (the server does not accept this computer's key: it was removed from the server or its key was reset. \
             Run the installer again with a new install token)"
        }
        Some(429) => " (too many refused attempts from this address: the server waits a few minutes)",
        _ => "",
    }
}

/// The connection to the server: plain TCP for ws://; for wss:// a TLS
/// handshake whose certificate is checked before anything is sent: valid
/// for the host by the system's roots, or else the one pinned in the config
/// (`server_cert_sha256`), pinned now on the first connection.
async fn server_stream(
    config: &mut AgentConfig,
    ws_url: &str,
) -> Result<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>, Box<dyn std::error::Error + Send + Sync>> {
    let uri: tokio_tungstenite::tungstenite::http::Uri = ws_url
        .parse()
        .map_err(|e| anyhow::anyhow!("bad server address {}: {}", ws_url, e))?;
    let tls = match uri.scheme_str() {
        Some("wss") => true,
        Some("ws") => false,
        _ => return Err(anyhow::anyhow!("bad server address {}", ws_url).into()),
    };
    let host = uri
        .host()
        .ok_or_else(|| anyhow::anyhow!("no host in the server address {}", ws_url))?
        .trim_start_matches('[')
        .trim_end_matches(']')
        .to_string();
    let port = uri.port_u16().unwrap_or(if tls { 443 } else { 80 });
    let tcp = tokio::time::timeout(
        std::time::Duration::from_secs(15),
        tokio::net::TcpStream::connect((host.as_str(), port)),
    )
    .await
    .map_err(|_| anyhow::anyhow!("connecting to {}:{} timed out", host, port))??;
    let _ = tcp.set_nodelay(true);
    if !tls {
        return Ok(tokio_tungstenite::MaybeTlsStream::Plain(tcp));
    }

    let verifier = Arc::new(server_tls::RecordingVerifier::new());
    let tls_config = server_tls::client_config(verifier.clone())?;
    let name = rustls::pki_types::ServerName::try_from(host.clone())
        .map_err(|e| anyhow::anyhow!("bad server host name {}: {}", host, e))?;
    let stream = tokio_rustls::TlsConnector::from(Arc::new(tls_config))
        .connect(name, tcp)
        .await
        .map_err(|e| anyhow::anyhow!("TLS handshake with {}:{} failed: {}", host, port, e))?;
    let seen = verifier
        .take()
        .ok_or_else(|| anyhow::anyhow!("{}:{} sent no certificate", host, port))?;
    let fingerprint = server_tls::fingerprint(&seen.der);
    let host_port = format!("{}:{}", host, port);
    match server_tls::decide(seen.valid, &fingerprint, &host_port, &config.server_cert_sha256, &config.server_cert_host) {
        server_tls::Decision::Valid | server_tls::Decision::PinMatches => {}
        server_tls::Decision::Pin => {
            warn!(
                "The Lunaris server {} has a certificate the system does not trust (self-signed?): \
                 trusting it from now on (pinned SHA-256 {}). Compare it with the server's if in doubt.",
                host_port, fingerprint
            );
            config.server_cert_sha256 = fingerprint.clone();
            config.server_cert_host = host_port.clone();
            let (sha, for_host) = (fingerprint.clone(), host_port.clone());
            if let Err(e) = crate::pairing::update_config_file(crate::pairing::config_path(), config, |c| {
                c.server_cert_sha256 = sha;
                c.server_cert_host = for_host;
            }) {
                warn!("Could not save the pinned server certificate: {}", e);
            }
        }
        server_tls::Decision::Mismatch => {
            return Err(anyhow::anyhow!(
                "the TLS certificate of the Lunaris server {} is not the one trusted before \
                 (pinned SHA-256 {}, now {}): not connecting, the connection may be intercepted. \
                 If the server's certificate was replaced on purpose, empty server_cert_sha256 in {} \
                 and restart the agent",
                host_port,
                config.server_cert_sha256,
                fingerprint,
                crate::pairing::config_path()
            )
            .into());
        }
    }
    Ok(tokio_tungstenite::MaybeTlsStream::Rustls(stream))
}

/// Trust on first use for the server's TLS certificate.
mod server_tls {
    use std::sync::{Arc, Mutex, OnceLock};

    use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
    use rustls::client::WebPkiServerVerifier;
    use rustls::crypto::{CryptoProvider, WebPkiSupportedAlgorithms};
    use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
    use rustls::{DigitallySignedStruct, RootCertStore, SignatureScheme};

    pub struct Seen {
        pub der: Vec<u8>,
        /// Valid for the host name by the system's roots.
        pub valid: bool,
    }

    /// Lets the handshake finish with any certificate (its signatures over
    /// the handshake are still verified, so the server holds its key) and
    /// records it; the caller decides before sending anything.
    pub struct RecordingVerifier {
        roots: Option<Arc<WebPkiServerVerifier>>,
        algorithms: WebPkiSupportedAlgorithms,
        seen: Mutex<Option<Seen>>,
    }

    impl std::fmt::Debug for RecordingVerifier {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str("RecordingVerifier")
        }
    }

    fn provider() -> Arc<CryptoProvider> {
        Arc::new(rustls::crypto::ring::default_provider())
    }

    /// The system's root certificates, loaded once.
    fn system_roots() -> Option<Arc<RootCertStore>> {
        static ROOTS: OnceLock<Option<Arc<RootCertStore>>> = OnceLock::new();
        ROOTS
            .get_or_init(|| {
                let mut store = RootCertStore::empty();
                store.add_parsable_certificates(rustls_native_certs::load_native_certs().certs);
                (!store.is_empty()).then(|| Arc::new(store))
            })
            .clone()
    }

    impl RecordingVerifier {
        pub fn new() -> Self {
            let provider = provider();
            let roots = system_roots()
                .and_then(|r| WebPkiServerVerifier::builder_with_provider(r, provider.clone()).build().ok());
            Self { roots, algorithms: provider.signature_verification_algorithms, seen: Mutex::new(None) }
        }

        pub fn take(&self) -> Option<Seen> {
            self.seen.lock().unwrap().take()
        }
    }

    impl ServerCertVerifier for RecordingVerifier {
        fn verify_server_cert(
            &self,
            end_entity: &CertificateDer<'_>,
            intermediates: &[CertificateDer<'_>],
            server_name: &ServerName<'_>,
            ocsp_response: &[u8],
            now: UnixTime,
        ) -> Result<ServerCertVerified, rustls::Error> {
            let valid = self
                .roots
                .as_ref()
                .map(|v| v.verify_server_cert(end_entity, intermediates, server_name, ocsp_response, now).is_ok())
                .unwrap_or(false);
            *self.seen.lock().unwrap() = Some(Seen { der: end_entity.as_ref().to_vec(), valid });
            Ok(ServerCertVerified::assertion())
        }

        fn verify_tls12_signature(
            &self,
            message: &[u8],
            cert: &CertificateDer<'_>,
            dss: &DigitallySignedStruct,
        ) -> Result<HandshakeSignatureValid, rustls::Error> {
            rustls::crypto::verify_tls12_signature(message, cert, dss, &self.algorithms)
        }

        fn verify_tls13_signature(
            &self,
            message: &[u8],
            cert: &CertificateDer<'_>,
            dss: &DigitallySignedStruct,
        ) -> Result<HandshakeSignatureValid, rustls::Error> {
            rustls::crypto::verify_tls13_signature(message, cert, dss, &self.algorithms)
        }

        fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
            self.algorithms.supported_schemes()
        }
    }

    pub fn client_config(verifier: Arc<RecordingVerifier>) -> Result<rustls::ClientConfig, rustls::Error> {
        Ok(rustls::ClientConfig::builder_with_provider(provider())
            .with_safe_default_protocol_versions()?
            .dangerous()
            .with_custom_certificate_verifier(verifier)
            .with_no_client_auth())
    }

    pub fn fingerprint(der: &[u8]) -> String {
        crate::pairing::to_hex(ring::digest::digest(&ring::digest::SHA256, der).as_ref())
    }

    #[derive(Debug, PartialEq, Eq)]
    pub enum Decision {
        /// Valid by the system's roots.
        Valid,
        /// The pinned certificate.
        PinMatches,
        /// Nothing pinned for this server: pin this one.
        Pin,
        /// Another certificate than the pinned one: refuse.
        Mismatch,
    }

    /// `pinned_host` empty: the pin holds for any server address.
    pub fn decide(valid: bool, fingerprint: &str, host: &str, pinned: &str, pinned_host: &str) -> Decision {
        let pinned = pinned.trim();
        if valid {
            Decision::Valid
        } else if pinned.is_empty() || (!pinned_host.is_empty() && pinned_host != host) {
            Decision::Pin
        } else if pinned.eq_ignore_ascii_case(fingerprint) {
            Decision::PinMatches
        } else {
            Decision::Mismatch
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn trust_on_first_use() {
            let fp = fingerprint(b"cert");
            assert_eq!(fp.len(), 64);
            assert_eq!(decide(true, &fp, "h:1", "", ""), Decision::Valid);
            assert_eq!(decide(true, &fp, "h:1", "00", "h:1"), Decision::Valid);
            assert_eq!(decide(false, &fp, "h:1", "", ""), Decision::Pin);
            assert_eq!(decide(false, &fp, "h:1", &fp, "h:1"), Decision::PinMatches);
            assert_eq!(decide(false, &fp, "h:1", &fp.to_uppercase(), ""), Decision::PinMatches);
            assert_eq!(decide(false, &fp, "h:1", &fingerprint(b"other"), "h:1"), Decision::Mismatch);
            assert_eq!(decide(false, &fp, "h:1", &fingerprint(b"other"), ""), Decision::Mismatch);
            // Pinned for another server: this one is new.
            assert_eq!(decide(false, &fp, "h:1", &fingerprint(b"other"), "g:1"), Decision::Pin);
        }
    }
}

/// `file_access` as it applies here: as set, "home" when not set. (Before
/// 0.7.1 an agent running as LocalSystem, the Windows service's, took "not
/// set" for the whole disk; it now means the home folder of the person
/// logged on at the screen, `file_access`. A config that says "full" keeps
/// it.)
pub(crate) fn file_access_setting(config: &AgentConfig) -> String {
    let value = config.file_access.trim();
    if value.is_empty() {
        "home".to_string()
    } else {
        value.to_string()
    }
}

/// What file transfer may reach, from the config: `file_access_setting`,
/// with "home" resolved for the account the agent runs as, or, for the
/// Windows service's agent (LocalSystem, whose own home is the system
/// profile), for the person logged on at the screen; nobody logged on:
/// refused.
pub(crate) fn file_access(config: &AgentConfig) -> files::Access {
    file_access_for(&file_access_setting(config))
}

/// `file_access` for a `file_access_setting` value.
pub(crate) fn file_access_for(setting: &str) -> files::Access {
    let setting = setting.trim();
    if setting == "home" || setting.is_empty() {
        if let Some(home) = service_home() {
            return match home {
                Ok(dir) => files::Access::from_config(&dir.to_string_lossy()),
                Err(reason) => files::Access::Denied(reason),
            };
        }
    }
    files::Access::from_config(setting)
}

/// The home folder "home" means when it is not this account's own: `None`
/// for an ordinary agent; for the Windows service's agent (LocalSystem) the
/// profile folder of the user logged on at the console, or why there is
/// none.
#[allow(clippy::unnecessary_wraps)]
fn service_home() -> Option<Result<std::path::PathBuf, String>> {
    #[cfg(target_os = "windows")]
    if win_service::running_as_local_system() {
        return Some(win_service::console_user_profile_dir().ok_or_else(|| {
            "file transfer: nobody is logged on at this computer's screen, so there is no home folder to offer \
             (the agent runs as the Windows service). Log on, or set file_access to a folder with \
             `lunaris-agent set file-access <folder>`"
                .to_string()
        }));
    }
    None
}

/// Whether `value` (a `file_access` value) keeps file transfer inside the
/// home folder: "home" (or not set), or a folder in it. "full" and a folder
/// elsewhere are wider: only set on this computer (settings.rs).
pub(crate) fn file_access_within_home(value: &str) -> bool {
    let value = value.trim();
    if value.is_empty() || value == "home" {
        return true;
    }
    if value == "full" {
        return false;
    }
    let home = match file_access_for("home") {
        files::Access::Root(root) => root,
        _ => return false,
    };
    matches!(files::Access::from_config(value), files::Access::Root(root) if root.starts_with(&home))
}

/// Stream parameters from the client, bounded to what the capture and
/// encoders handle (lunaris-media divides by the frame rate, and a few
/// encoders take the bitrate in 16- or 32-bit fields).
fn clamp_stream_request(
    width: Option<u32>,
    height: Option<u32>,
    fps: Option<u32>,
    bitrate: Option<u32>,
) -> (Option<u32>, Option<u32>, Option<u32>, Option<u32>) {
    // Even sizes: 4:2:0 encoders need them.
    let size = |v: u32| v.clamp(64, 8192) & !1;
    (
        width.map(size),
        height.map(size),
        // lunaris-media keeps the frame rate within 1..=240 when it changes it.
        fps.map(|v| v.clamp(1, 240)),
        // kbps; the clients offer up to 150 Mbps.
        bitrate.map(|v| v.clamp(500, 500_000)),
    )
}

/// Ends client `client`'s stream session, if it has one.
async fn close_stream(sessions: &mut std::collections::HashMap<String, Arc<BridgeSession>>, client: &str) -> bool {
    let Some(session) = sessions.remove(client) else { return false };
    let _ = session.peer_connection.close().await;
    if sessions.is_empty() {
        // The encoder is let go when its last viewer leaves.
        LAST_STREAM_STOP_TIME_MS.store(get_current_time_ms(), std::sync::atomic::Ordering::SeqCst);
    }
    info!("Stream session of {} ended ({} still streaming)", client, sessions.len());
    true
}

/// The agent's session logic, driven by signaling from server and direct
/// clients alike (addressed with the client's connection id). Replies go to
/// `agent_tx`, which the hub routes.
///
/// Any number of clients stream at once, as with RustDesk: viewers of the
/// same display share its capture and encoder (media_hub.rs), each with its
/// own connection and input. A source shown through a connector is the one
/// exception: what is known of it is the agent's (connector/media.rs), so
/// its session is alone; it replaces the others, and another session
/// replaces it.
async fn run_session_logic(
    config: AgentConfig,
    agent_tx: mpsc::UnboundedSender<AgentMessage>,
    inbound: &mut mpsc::UnboundedReceiver<SignalingMessage>,
) {
    // Stream sessions, by client connection.
    let mut sessions: std::collections::HashMap<String, Arc<BridgeSession>> = std::collections::HashMap::new();
    // The client whose session shows a connector's source.
    let mut source_client: Option<String> = None;
    // File-transfer connections (files.rs), by client connection; any number,
    // beside the stream.
    let mut file_sessions: std::collections::HashMap<String, Arc<dyn PeerConnection>> =
        std::collections::HashMap::new();

    while let Some(sig) = inbound.recv().await {
        // Answers, candidates and ends of file sessions go to them.
        let sig = match sig {
            SignalingMessage::IncomingFileSession { client_id, ref source_id, .. }
                if bridge_agent::is_bridge_only() && source_id.as_deref().unwrap_or("").is_empty() =>
            {
                // The bridge-only agent has no files of its own to give
                // (the files of a source it shows are that computer's).
                warn!("File transfer asked of the bridge-only agent by {}: refused", client_id);
                continue;
            }
            SignalingMessage::IncomingFileSession { client_id, ice_servers, source_id, .. } => {
                info!("File transfer session requested by {}", client_id);
                if let Some(old) = file_sessions.remove(&client_id) {
                    let _ = old.close().await;
                }
                // The files of a source are served from its connector, never
                // from this computer; a source that is not there is an error.
                let started = match files::source_target(source_id.as_deref(), crate::connector::configured_sources) {
                    Some(target) => files::start_for_source(client_id.clone(), agent_tx.clone(), ice_servers, target).await,
                    None => {
                        let access = file_access(&config);
                        if let files::Access::Denied(reason) = &access {
                            warn!("{}", reason);
                        }
                        files::start(client_id.clone(), agent_tx.clone(), ice_servers, config.allow_file_transfer, access).await
                    }
                };
                match started {
                    Ok((pc, sdp)) => {
                        file_sessions.insert(client_id.clone(), pc);
                        let _ = agent_tx.send(AgentMessage::Signaling(SignalingMessage::Sdp {
                            target_id: client_id,
                            sdp: RtcSessionDescription { ty: RtcSdpType::Offer, sdp },
                            ice_servers: None,
                            webtransport_port: None,
                            webtransport_cert_hash: None,
                            webtransport_token: None,
                        }));
                    }
                    Err(e) => {
                        error!("Could not start a file session: {:?}", e);
                        let _ = agent_tx.send(AgentMessage::Signaling(SignalingMessage::Error {
                            message: format!("Could not start file transfer: {}", e),
                        }));
                    }
                }
                continue;
            }
            SignalingMessage::Sdp { ref target_id, ref sdp, .. } if file_sessions.contains_key(target_id) => {
                let pc = file_sessions[target_id].clone();
                match RTCSessionDescription::answer(sdp.sdp.clone()) {
                    Ok(answer) => {
                        if let Err(e) = pc.set_remote_description(answer).await {
                            error!("File session: bad answer: {:?}", e);
                        }
                    }
                    Err(e) => error!("File session: bad answer: {:?}", e),
                }
                continue;
            }
            SignalingMessage::IceCandidate { ref target_id, ref candidate } if file_sessions.contains_key(target_id) => {
                let pc = file_sessions[target_id].clone();
                let _ = pc
                    .add_ice_candidate(RTCIceCandidateInit {
                        candidate: candidate.candidate.clone(),
                        sdp_mid: candidate.sdp_mid.clone(),
                        sdp_mline_index: candidate.sdp_mline_index,
                        username_fragment: candidate.username_fragment.clone(),
                        ..Default::default()
                    })
                    .await;
                continue;
            }
            SignalingMessage::EndSession { ref target_id } if file_sessions.contains_key(target_id) => {
                info!("File transfer session ended by {}", target_id);
                if let Some(pc) = file_sessions.remove(target_id) {
                    let _ = pc.close().await;
                }
                continue;
            }
            other => other,
        };
        match sig {
            SignalingMessage::IncomingSession {
                client_id,
                width,
                height,
                fps,
                bitrate,
                codec,
                app_id: _,
                encoder,
                display_id,
                chroma,
                encoder_csc_mode,
                microphone,
                ice_servers,
                source_id,
                decode_codecs,
                ..
            } => {
                let (width, height, fps, bitrate) = clamp_stream_request(width, height, fps, bitrate);
                // A source asked for must be one this computer has: its own
                // screen is never shown in place of one that is not there.
                let source = match source_id.as_deref().filter(|id| !id.is_empty()) {
                    // The bridge-only agent shows other computers, never this one.
                    None if bridge_agent::is_bridge_only() => {
                        warn!("Session without a source asked of the bridge-only agent: refused");
                        let _ = agent_tx.send(AgentMessage::Signaling(SignalingMessage::SourceStatus {
                            target_id: client_id.clone(),
                            status: "failed".into(),
                            reason: Some("This is a gateway with no screen of its own: connect to a computer added through it".into()),
                        }));
                        continue;
                    }
                    None => None,
                    Some(id) => match crate::connector::configured_sources().into_iter().find(|s| s.id == id) {
                        Some(found) => {
                            info!("Session for source \"{}\" ({}) through connector {}", found.name, found.id, found.target.connector);
                            Some(found.target)
                        }
                        None => {
                            warn!("Session asked for a source this computer does not have");
                            let _ = agent_tx.send(AgentMessage::Signaling(SignalingMessage::SourceStatus {
                                target_id: client_id.clone(),
                                status: "failed".into(),
                                reason: Some("This computer no longer has that source".into()),
                            }));
                            continue;
                        }
                    },
                };
                info!(
                    "Incoming session request from client: {} codec={:?} encoder={:?} res={:?}x{:?} fps={:?} bitrate={:?} display={:?} chroma={:?} encoder_csc_mode={:?} microphone={:?}",
                    client_id, codec, encoder, width, height, fps, bitrate, display_id, chroma, encoder_csc_mode, microphone
                );

                // This client's previous session; and the sessions this one
                // cannot run beside (a source's, see above).
                if close_stream(&mut sessions, &client_id).await {
                    info!("Replacing the previous session of {}", client_id);
                }
                let alone: Vec<String> = if source.is_some() {
                    sessions.keys().cloned().collect()
                } else {
                    source_client.iter().cloned().collect()
                };
                for other in alone {
                    if close_stream(&mut sessions, &other).await {
                        info!("Session of {} replaced by {}'s (a connector's source is shown alone)", other, client_id);
                        let _ = agent_tx.send(AgentMessage::Signaling(SignalingMessage::EndSession { target_id: other }));
                    }
                }
                if source_client.as_deref().is_some_and(|c| !sessions.contains_key(c)) {
                    source_client = None;
                }
                let shows_source = source.is_some();

                // Wait if the stream was stopped very recently (let encoder release resources)
                let last_stop = LAST_STREAM_STOP_TIME_MS.load(std::sync::atomic::Ordering::SeqCst);
                if last_stop > 0 {
                    let now = get_current_time_ms();
                    let elapsed = now.saturating_sub(last_stop);
                    if elapsed < 1200 {
                        let sleep_ms = 1200 - elapsed;
                        info!("Previous stream was stopped recently ({}ms ago). Delaying new stream setup by {}ms to prevent encoder conflicts...", elapsed, sleep_ms);
                        tokio::time::sleep(std::time::Duration::from_millis(sleep_ms)).await;
                    }
                }

                // Initialize bridge session
                let session = match setup_bridge_session(
                    config.clone(),
                    client_id.clone(),
                    agent_tx.clone(),
                    width,
                    height,
                    fps,
                    bitrate,
                    codec,
                    encoder,
                    display_id,
                    chroma.as_deref() == Some("444"),
                    encoder_csc_mode,
                    microphone.unwrap_or(false),
                    ice_servers,
                    source,
                    decode_codecs,
                )
                .await
                {
                    Ok(s) => s,
                    Err(e) => {
                        // A source that could not be shown: the viewer is
                        // told why, in the connector's words.
                        if let Some(source_error) = e.downcast_ref::<crate::connector::SourceError>() {
                            warn!("The source could not be shown: {}", source_error);
                            let _ = agent_tx.send(AgentMessage::Signaling(SignalingMessage::SourceStatus {
                                target_id: client_id.clone(),
                                status: "failed".into(),
                                reason: Some(source_error.to_string()),
                            }));
                            continue;
                        }
                        error!("Failed to setup WebRTC bridge session: {:?}", e);
                        let _ = agent_tx.send(AgentMessage::Signaling(
                            SignalingMessage::Error {
                                message: format!(
                                    "Failed to bridge connection: {:?}",
                                    e
                                ),
                            },
                        ));
                        continue;
                    }
                };

                // Generate SDP Offer
                let offer = match session.peer_connection.create_offer(None).await {
                    Ok(o) => o,
                    Err(e) => {
                        error!("Failed to create SDP Offer: {:?}", e);
                        continue;
                    }
                };

                if let Err(e) = session
                    .peer_connection
                    .set_local_description(offer.clone())
                    .await
                {
                    error!("Failed to set local description: {:?}", e);
                    continue;
                }

                // Send Offer to Client
                let sdp_msg = AgentMessage::Signaling(SignalingMessage::Sdp {
                    target_id: client_id.clone(),
                    sdp: RtcSessionDescription {
                        ty: RtcSdpType::Offer,
                        sdp: offer.sdp,
                    },
                    ice_servers: None,
                    webtransport_port: session.webtransport_port,
                    webtransport_cert_hash: session.webtransport_cert_hash.clone(),
                    webtransport_token: session.webtransport_token.clone(),
                });

                // CRITICAL: Store session BEFORE sending the offer to avoid a race
                // condition where the client's SDP answer arrives before the session
                // is stored, causing it to be silently dropped (=> no ICE candidate
                // pairs => connection timeout after ~7s).
                sessions.insert(client_id.clone(), session);
                if shows_source {
                    source_client = Some(client_id.clone());
                }
                info!("{} stream session(s) now", sessions.len());

                let _ = agent_tx.send(sdp_msg);
                warn!("[SIGNALING] SDP Offer sent to client: {}", client_id);
            }
            SignalingMessage::Sdp { target_id, .. } if !sessions.contains_key(&target_id) => {
                // Only a client an offer went to answers it.
                warn!("[SIGNALING] Ignoring an SDP answer from {} (no session of its own)", target_id);
            }
            SignalingMessage::Sdp { target_id, sdp, .. } => {
                warn!("[SIGNALING] Received SDP answer from client: {}", target_id);
                if let Some(session) = sessions.get(&target_id) {
                    // Force Level 4.2 in SDP Answer to prevent browser decoder from silently rejecting Level 4.0/4.2 AMF GPU streams.
                    let forced_sdp = sdp
                        .sdp
                        .replace("profile-level-id=42001f", "profile-level-id=42002a")
                        // Same for H.264 High 4:4:4 (YUV 4:4:4 sessions).
                        .replace("profile-level-id=f4001f", "profile-level-id=f4002a");
                    match RTCSessionDescription::answer(forced_sdp.clone()) {
                        Ok(rtc_sdp) => {
                            if let Err(e) = session
                                .peer_connection
                                .set_remote_description(rtc_sdp)
                                .await
                            {
                                error!("[SIGNALING] Failed to set remote description: {:?}", e);
                            } else {
                                warn!("[SIGNALING] SDP Answer set successfully! WebRTC negotiation in progress.");
                                match bridge::flexfec_in_answer(&forced_sdp) {
                                    Some(pt) => info!("FlexFEC-03 (20%): accepted by the client's answer (payload type {}), the video is protected by forward error correction", pt),
                                    None => info!("FlexFEC-03: not in use (offered only with LUNARIS_FEC=1, and the client has to accept it); lost packets are repaired by NACK"),
                                }
                            }
                        }
                        Err(e) => {
                            error!("[SIGNALING] Failed to parse SDP Answer: {:?}", e);
                        }
                    }
                }
            }
            SignalingMessage::IceCandidate { target_id, .. } if !sessions.contains_key(&target_id) => {
                debug!("Ignoring an ICE candidate from {} (no session of its own)", target_id);
            }
            SignalingMessage::IceCandidate {
                target_id,
                candidate,
            } => {
                if let Some(session) = sessions.get(&target_id) {
                    let rtc_cand = RTCIceCandidateInit {
                        candidate: candidate.candidate,
                        sdp_mid: candidate.sdp_mid,
                        sdp_mline_index: candidate.sdp_mline_index,
                        username_fragment: candidate.username_fragment,
                        ..Default::default()
                    };
                    if let Err(e) =
                        session.peer_connection.add_ice_candidate(rtc_cand).await
                    {
                        debug!("Failed to add ICE candidate: {:?}", e);
                    }
                }
            }
            SignalingMessage::EndSession { target_id } => {
                // A client ends its own session only: another one (e.g. a
                // direct connection that only asked for the display list)
                // disconnecting must not cut anyone's stream.
                if !close_stream(&mut sessions, &target_id).await {
                    debug!("Ignoring EndSession from {} (no session of its own)", target_id);
                    continue;
                }
                info!("Session ended by client: {}", target_id);
                if source_client.as_deref() == Some(target_id.as_str()) {
                    source_client = None;
                }
            }
            SignalingMessage::GetAppList { target_id } => {
                info!("Received GetAppList request from target: {}", target_id);
                let agent_tx_clone = agent_tx.clone();
                let apps = vec![common::AppInfo {
                    id: 0,
                    title: "Desktop".to_string(),
                    icon_base64: None,
                }];
                let resp = AgentMessage::Signaling(
                    SignalingMessage::AppListResponse {
                        target_id,
                        apps,
                        current_game_id: 0,
                    },
                );
                let _ = agent_tx_clone.send(resp);
            }
            SignalingMessage::GetCapabilities { target_id } => {
                info!("Received GetCapabilities request from target: {}", target_id);
                let agent_tx_clone = agent_tx.clone();

                // 1. List displays. Uses the lightweight enumeration
                // (NvFBC status only, no CUDA context) so opening the
                // client settings no longer allocates/leaks GPU memory.
                let mut displays = Vec::new();
                // The bridge-only agent has nothing of this computer to tell:
                // no displays, no encoders; only its sources.
                // Nor while another computer is being shown: the viewer asks
                // about what it sees, and that computer's displays reach it
                // with the picture.
                let own_screen = !bridge_agent::is_bridge_only() && !crate::connector::media::source_active();
                match if own_screen { lunaris_media::capture::list_displays_for_capabilities().await } else { Ok(Vec::new()) } {
                    Ok(display_list) => {
                        info!("[GetCapabilities] Discovered {} display(s)", display_list.len());
                        for d in &display_list {
                            info!("  Display: {} ({}) {}x{} @{}Hz primary={}", d.id, d.name, d.width, d.height, d.refresh_rate, d.is_primary);
                            displays.push(common::DisplayInfoMsg {
                                id: d.id.clone(),
                                name: d.name.clone(),
                                width: d.width,
                                height: d.height,
                                refresh_rate: d.refresh_rate,
                                is_primary: d.is_primary,
                            });
                        }
                    }
                    Err(e) => error!("Failed to list displays: {:?}", e),
                }

                // 3. Query encoders (fast, non-blocking)
                info!("[GetCapabilities] Querying encoders...");
                let mut encoders = Vec::new();
                encoders.push("native".to_string());
                encoders.push("ffmpeg".to_string());
                let encoder_list = if own_screen { lunaris_media::encode::list_available_encoders() } else { Vec::new() };
                for enc in encoder_list {
                    if !encoders.contains(&enc.name) {
                        encoders.push(enc.name.clone());
                    }
                    let hw_name = format!("{}", enc.hw_type).to_lowercase();
                    if !encoders.contains(&hw_name) {
                        encoders.push(hw_name);
                    }
                }
                if !encoders.contains(&"software".to_string()) {
                    encoders.push("software".to_string());
                }

                // 4. Send response
                info!("[GetCapabilities] Sending response: {} displays, {} encoders", displays.len(), encoders.len());
                let resp = AgentMessage::Signaling(
                    SignalingMessage::CapabilitiesResponse {
                        target_id,
                        displays,
                        encoders,
                        gpu_info: if own_screen { lunaris_media::encode::describe_host_gpu(None) } else { None },
                        // While another computer is shown, the viewer treats the
                        // pointer by what that computer is.
                        host_os: Some(crate::connector::media::source_os().unwrap_or_else(|| std::env::consts::OS.to_string())),
                        // The other computers this one shows: id and name only.
                        // (Not their groups: a viewer picks one by its name.)
                        sources: crate::connector::configured_sources()
                            .into_iter()
                            .map(|s| common::SourceInfoMsg { id: s.id, name: s.name, connector: s.target.connector, group: None, online: None, values: None })
                            .collect(),
                    },
                );
                if let Err(e) = agent_tx_clone.send(resp) {
                    error!("Failed to send CapabilitiesResponse: {:?}", e);
                } else {
                    info!("[GetCapabilities] Response sent!");
                }
            }
            SignalingMessage::StopActiveStream { target_id } => {
                info!(
                    "Received StopActiveStream request from target: {}",
                    target_id
                );
                // Every stream here stops; their clients are told.
                let clients: Vec<String> = sessions.keys().cloned().collect();
                let success = !clients.is_empty();
                for client in clients {
                    close_stream(&mut sessions, &client).await;
                    let _ = agent_tx.send(AgentMessage::Signaling(SignalingMessage::EndSession { target_id: client }));
                }
                source_client = None;
                let resp = AgentMessage::Signaling(
                    SignalingMessage::StopActiveStreamResponse {
                        target_id,
                        success,
                        error: if success { None } else { Some("No active session".to_string()) },
                    },
                );
                let _ = agent_tx.send(resp);
            }
            _ => {}
        }
    }

    for (client, session) in sessions.drain() {
        info!("Stopping the stream session of {} on agent exit...", client);
        let _ = session.peer_connection.close().await;
    }
}

#[cfg(test)]
mod stream_limit_tests {
    use super::clamp_stream_request;

    #[test]
    fn stream_parameters_are_bounded() {
        assert_eq!(clamp_stream_request(None, None, None, None), (None, None, None, None));
        assert_eq!(
            clamp_stream_request(Some(1920), Some(1080), Some(60), Some(20_000)),
            (Some(1920), Some(1080), Some(60), Some(20_000))
        );
        assert_eq!(
            clamp_stream_request(Some(0), Some(u32::MAX), Some(0), Some(u32::MAX)),
            (Some(64), Some(8192), Some(1), Some(500_000))
        );
        assert_eq!(clamp_stream_request(Some(1365), Some(767), Some(100_000), Some(0)), (Some(1364), Some(766), Some(240), Some(500)));
    }
}

#[cfg(test)]
mod enroll_tests {
    use super::*;

    const TOKEN: &str = "lit_00112233445566778899aabbccddeeff00112233445566778899aabbccddeeff";

    fn args(extra: &[&str]) -> Args {
        let mut v = vec!["agent", "--enroll", "--server", "https://srv.lan:8443/", "--install-token", TOKEN];
        v.extend_from_slice(extra);
        Args::try_parse_from(v).unwrap()
    }

    #[test]
    fn refusals_are_explained() {
        assert!(refusal_hint(Some(401), true, false).contains("install token"));
        assert!(refusal_hint(Some(401), false, true).contains("connection token"));
        assert!(refusal_hint(Some(401), false, false).contains("key"));
        assert!(refusal_hint(Some(429), true, false).contains("too many"));
        assert_eq!(refusal_hint(Some(500), true, false), "");
        assert_eq!(refusal_hint(None, true, false), "");
    }

    #[test]
    fn the_connection_to_the_server_is_encrypted_unless_ws_is_named() {
        assert_eq!(server_ws_url("srv.lan:8090"), "wss://srv.lan:8090");
        assert_eq!(server_ws_url(" https://srv "), "wss://srv");
        assert_eq!(server_ws_url("http://srv:8090"), "ws://srv:8090");
        // An existing ws:// server keeps working, as it was.
        assert_eq!(server_ws_url("ws://10.0.0.5:8090"), "ws://10.0.0.5:8090");
        assert_eq!(server_transport("ws://10.0.0.5:8090"), ServerTransport::Plain);
        assert_eq!(server_transport("ws://srv.lan"), ServerTransport::Plain);
        assert_eq!(server_transport("ws://127.0.0.1:8090"), ServerTransport::PlainLoopback);
        assert_eq!(server_transport("ws://localhost:8090/"), ServerTransport::PlainLoopback);
        assert_eq!(server_transport("ws://[::1]:8090"), ServerTransport::PlainLoopback);
        assert_eq!(server_transport("ws://[2001:db8::1]:8090"), ServerTransport::Plain);
        assert_eq!(server_transport("wss://srv:8443"), ServerTransport::Tls);
        // The config default (no server_url in the file) is this computer.
        assert_eq!(server_transport(&server_ws_url(&AgentConfig::default().server_url)), ServerTransport::PlainLoopback);
    }

    #[test]
    fn secrets_go_in_headers_as_well_as_the_query() {
        let url = "wss://srv/ws/agent?id=X&token=tok&install_token=lit_1&key=k";
        let req = server_request(url, "tok", "k", "lit_1").unwrap();
        assert_eq!(req.uri().to_string(), url, "old servers still read the query");
        let h = req.headers();
        assert_eq!(h["X-Lunaris-Token"], "tok");
        assert_eq!(h["X-Lunaris-Agent-Key"], "k");
        assert_eq!(h["X-Lunaris-Install-Token"], "lit_1");
        assert!(h["X-Lunaris-Agent-Key"].is_sensitive());
        // Empty ones are left out.
        let req = server_request("wss://srv/ws/agent?id=X&token=", "", "k", "").unwrap();
        assert!(!req.headers().contains_key("X-Lunaris-Token") && !req.headers().contains_key("X-Lunaris-Install-Token"));
        // One a header cannot carry stays in the query only.
        let req = server_request("wss://srv/ws/agent?token=a%0Ab", "a\nb", "", "").unwrap();
        assert!(!req.headers().contains_key("X-Lunaris-Token"));
    }

    #[test]
    fn requesters_through_the_server_are_shown_cleaned() {
        let sig = |r: Option<&str>| SignalingMessage::IncomingFileSession {
            client_id: "c".into(),
            ice_servers: None,
            requester: r.map(str::to_string),
            source_id: None,
        };
        assert_eq!(server_requester_label(&sig(Some("bob"))), "bob (through the Lunaris server)");
        assert_eq!(
            server_requester_label(&sig(Some("Admin\r\nAccepted\u{202E}"))),
            "AdminAccepted (through the Lunaris server)"
        );
        assert_eq!(server_requester_label(&sig(Some(&"x".repeat(500)))).chars().count(), 64 + " (through the Lunaris server)".len());
        assert_eq!(server_requester_label(&sig(Some(" \u{7} "))), "a user of the Lunaris server");
        assert_eq!(server_requester_label(&sig(None)), "a user of the Lunaris server");
    }

    #[test]
    fn file_access_defaults_to_the_home_folder() {
        let mut c = AgentConfig::default();
        assert_eq!(file_access_setting(&c), "home");
        // Not running as LocalSystem here: this account's home.
        if let Some(home) = dirs::home_dir().and_then(|h| files::canonical(h).ok()) {
            assert_eq!(file_access(&c), files::Access::Root(home));
        }
        c.file_access = "full".into();
        assert_eq!(file_access_setting(&c), "full");
        assert_eq!(file_access(&c), files::Access::Full);
        assert!(file_access_within_home("home") && file_access_within_home(""));
        assert!(!file_access_within_home("full"));
        if let Some(home) = dirs::home_dir() {
            let inside = home.join(format!(".lunaris-within-{}", std::process::id()));
            std::fs::create_dir_all(&inside).unwrap();
            assert!(file_access_within_home(&inside.to_string_lossy()));
            let _ = std::fs::remove_dir_all(&inside);
        }
        let outside = std::env::temp_dir().join(format!("lunaris-outside-{}", std::process::id()));
        std::fs::create_dir_all(&outside).unwrap();
        if dirs::home_dir().is_some_and(|h| !outside.starts_with(h)) {
            assert!(!file_access_within_home(&outside.to_string_lossy()));
        }
        let _ = std::fs::remove_dir_all(&outside);
    }

    /// A config as written by 0.7.0, every field set: read, changed by every
    /// fix of this version that touches the config, saved and read again,
    /// nothing of it is lost or changed.
    #[test]
    fn a_config_in_the_current_format_loads_and_round_trips_unchanged() {
        let json = r#"{
  "client_unique_id": "8E0B1C1E-1111-4222-8333-944455556666",
  "client_private_key": "PRIVATE KEY",
  "client_certificate": "CERT",
  "server_certificate": "SERVER CERT",
  "server_url": "ws://192.168.1.10:8090",
  "server_token": "agent-token",
  "webtransport_port": 55201,
  "autostart": true,
  "close_to_tray": true,
  "direct_enabled": true,
  "direct_port": 9101,
  "direct_password": "short1",
  "allow_clipboard": false,
  "allow_file_transfer": true,
  "approve_mode": "click",
  "server_sessions": "ask",
  "server_cert_sha256": "abababababababababababababababababababababababababababababababab",
  "server_cert_host": "192.168.1.10:8090",
  "file_access": "full",
  "agent_key": "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
  "install_token": "lit_abc",
  "name": "Den PC",
  "rustdesk_port": 21130,
  "rustdesk_file_transfer": true,
  "sources": [
    {"id": "nas-ab12", "name": "NAS", "connector": "vnc", "address": "10.0.0.7", "port": "5900", "password": "vnc-secret"},
    {"id": "win-cd34", "name": "Office", "connector": "rdp", "address": "10.0.0.8", "username": "huy", "password": "rdp-secret", "group": "g1"}
  ],
  "wayland_capture": "portal",
  "allow_server_settings": true
}"#;
        let dir = std::env::temp_dir().join(format!("lunaris-compat-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("agent_config.json");
        std::fs::write(&path, json).unwrap();
        let path = path.to_str().unwrap();
        let before: serde_json::Value = serde_json::from_str(json).unwrap();

        let mut c = crate::pairing::load_config(path).unwrap();
        // Nothing regenerated or reset.
        assert!(!crate::pairing::ensure_client_id(&mut c));
        assert!(!crate::pairing::ensure_agent_key(&mut c));
        assert!(!crate::pairing::ensure_direct_password(&mut c));
        assert_eq!(c.direct_password, "short1", "a password saved before the 12-character rule is kept");
        assert_eq!(file_access_setting(&c), "full", "an explicit \"full\" stays");
        assert_eq!(server_ws_url(&c.server_url), "ws://192.168.1.10:8090", "an existing ws:// server is kept");
        assert!(c.allow_legacy_plain_direct, "older apps keep working after the upgrade");
        assert_eq!(c.sources.len(), 2);

        // A change from the server that is refused now leaves the file as it was.
        let server = crate::settings::Channel::server();
        let widen = common::SettingsRequest::Change {
            change: common::AgentSettingsChange { server_sessions: Some("trusted".into()), ..Default::default() },
        };
        let _ = crate::settings::reply("r".into(), &widen, None, &server, path);
        // An allowed one changes only what it names.
        let narrow = common::SettingsRequest::Change {
            change: common::AgentSettingsChange { allow_clipboard: Some(true), ..Default::default() },
        };
        let _ = crate::settings::reply("r".into(), &narrow, None, &server, path);

        let after: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
        let mut expected = before.clone();
        expected["allow_clipboard"] = serde_json::Value::Bool(true);
        // The one new field, at its default.
        expected["allow_legacy_plain_direct"] = serde_json::Value::Bool(true);
        assert_eq!(after, expected);

        // And once more through save/load.
        let c = crate::pairing::load_config(path).unwrap();
        crate::pairing::save_config(&c, path).unwrap();
        let again: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
        assert_eq!(again, expected);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn server_addresses() {
        assert_eq!(normalize_server_url("https://srv.lan:8443/").unwrap(), ("wss://srv.lan:8443".into(), "srv.lan:8443".into()));
        // No scheme: encrypted (it was ws:// before 0.7.1).
        assert_eq!(normalize_server_url("srv.lan:8090").unwrap().0, "wss://srv.lan:8090");
        assert_eq!(normalize_server_url("ws://srv.lan:8090").unwrap().0, "ws://srv.lan:8090");
        assert_eq!(normalize_server_url("wss://example.com").unwrap().1, "example.com:443");
        assert_eq!(normalize_server_url("ws://[::1]:8090").unwrap().1, "::1:8090");
        assert!(normalize_server_url("").is_err());
        assert!(normalize_server_url("ws://srv/ws/agent?token=x").is_err());
    }

    #[test]
    fn enroll_writes_the_config_and_keeps_the_identity() {
        let dir = std::env::temp_dir().join(format!("lunaris-enroll-{}", crate::pairing::to_hex(&crate::pairing::random_bytes(6))));
        let path = dir.join("sub").join("agent_config.json");
        let path_s = path.to_string_lossy().to_string();
        // An existing config with a token for another server and a pin.
        let mut old = crate::pairing::new_config();
        old.server_url = "ws://other:8090".into();
        old.server_token = "main-token".into();
        old.server_cert_sha256 = "aa".repeat(32);
        old.direct_enabled = true;
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        crate::pairing::save_config(&old, &path_s).unwrap();

        let pin = "AB".repeat(32);
        let url = enroll_config(&args(&["--name", "Lab 1", "--server-cert-sha256", &pin]), &path_s).unwrap();
        assert_eq!(url, "wss://srv.lan:8443");
        let c = crate::pairing::load_config(&path_s).unwrap();
        assert_eq!(c.client_unique_id, old.client_unique_id);
        assert_eq!(c.server_url, "wss://srv.lan:8443");
        assert_eq!(c.install_token, TOKEN);
        assert!(c.server_token.is_empty(), "the main token is not kept");
        assert_eq!(c.agent_key.len(), 64);
        assert_eq!(c.name, "Lab 1");
        assert_eq!(c.server_cert_sha256, "ab".repeat(32));
        assert_eq!(c.server_cert_host, "srv.lan:8443");
        assert!(c.autostart && c.direct_enabled);

        // Again (the installer run twice): same id and key.
        enroll_config(&args(&[]), &path_s).unwrap();
        let again = crate::pairing::load_config(&path_s).unwrap();
        assert_eq!((again.client_unique_id, again.agent_key), (c.client_unique_id, c.agent_key));
        // Same server: the pin stays.
        assert_eq!(again.server_cert_sha256, "ab".repeat(32));
        assert_eq!(again.name, "Lab 1");
        // The server may not change the settings unless the installer says so.
        assert!(!again.allow_server_settings);
        enroll_config(&args(&["--allow-server-settings"]), &path_s).unwrap();
        assert!(crate::pairing::load_config(&path_s).unwrap().allow_server_settings);
        enroll_config(&args(&[]), &path_s).unwrap();
        assert!(crate::pairing::load_config(&path_s).unwrap().allow_server_settings, "kept when not given");
        enroll_config(&args(&["--deny-server-settings"]), &path_s).unwrap();
        assert!(!crate::pairing::load_config(&path_s).unwrap().allow_server_settings);
        assert!(Args::try_parse_from(["agent", "--allow-server-settings", "--deny-server-settings"]).is_err());

        // Bad input changes nothing.
        let bad = Args::try_parse_from(["agent", "--enroll", "--server", "srv", "--install-token", "nope"]).unwrap();
        assert!(enroll_config(&bad, &path_s).is_err());
        assert!(enroll_config(&args(&["--server-cert-sha256", "xyz"]), &path_s).is_err());
        assert!(Args::try_parse_from(["agent", "--enroll"]).is_err(), "--enroll needs --server");
        let _ = std::fs::remove_dir_all(&dir);
    }
}

#[cfg(test)]
mod local_services_tests {
    use super::*;

    fn free_port() -> u16 {
        std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap().local_addr().unwrap().port()
    }

    async fn listening(port: u16) -> bool {
        tokio::net::TcpStream::connect(("127.0.0.1", port)).await.is_ok()
    }

    async fn until(port: u16, open: bool) {
        for _ in 0..200 {
            if listening(port).await == open {
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        panic!("port {} is not {}", port, if open { "open" } else { "closed" });
    }

    /// A change from the server starts the listeners again with the new
    /// settings: the old port is freed at once (so the same port can be
    /// taken again), the new one listens.
    #[tokio::test]
    async fn the_listeners_start_again_with_new_settings() {
        let hub = hub::Hub::new("AGENT".into());
        let (inbound_tx, _inbound_rx) = mpsc::unbounded_channel::<SignalingMessage>();
        let links = direct::Links::default();
        let mut config = crate::pairing::new_config();
        config.direct_enabled = true;
        config.direct_port = free_port();
        let first = config.direct_port;
        let services = start_local_services(&mut config, "Test", &hub, &inbound_tx, &links);
        until(first, true).await;
        services.stop().await;
        assert!(!listening(first).await, "stopped means the port is free");
        // The same port again (a change that kept it), then another one.
        let services = start_local_services(&mut config, "Test", &hub, &inbound_tx, &links);
        until(first, true).await;
        services.stop().await;
        config.direct_port = free_port();
        let services = start_local_services(&mut config, "Test", &hub, &inbound_tx, &links);
        until(config.direct_port, true).await;
        assert!(!listening(first).await);
        services.stop().await;
        // Direct access off: nothing listens.
        config.direct_enabled = false;
        let services = start_local_services(&mut config, "Test", &hub, &inbound_tx, &links);
        assert!(services.0.is_empty());
    }

    /// Dropping `run_agent` stops the agent (as the bridge and tests do):
    /// its listeners go with it and free their ports, so the next start can
    /// listen there again.
    #[tokio::test]
    async fn dropped_listeners_free_their_ports() {
        let hub = hub::Hub::new("AGENT".into());
        let (inbound_tx, _inbound_rx) = mpsc::unbounded_channel::<SignalingMessage>();
        let links = direct::Links::default();
        let mut config = crate::pairing::new_config();
        config.direct_enabled = true;
        config.direct_password = "a-password".into();
        config.direct_port = free_port();
        config.rustdesk_port = Some(free_port());
        let (direct_port, rustdesk_port) = (config.direct_port, config.rustdesk_port.unwrap());
        let services = start_local_services(&mut config, "Test", &hub, &inbound_tx, &links);
        until(direct_port, true).await;
        until(rustdesk_port, true).await;
        drop(services);
        until(direct_port, false).await;
        until(rustdesk_port, false).await;
        // Taken again by the next start.
        let services = start_local_services(&mut config, "Test", &hub, &inbound_tx, &links);
        until(direct_port, true).await;
        services.stop().await;
    }

    #[test]
    fn a_reload_keeps_the_identity_and_the_server() {
        let mut running = crate::pairing::new_config();
        running.server_url = "wss://server.example:8443".into();
        running.agent_key = "k".repeat(64);
        let mut saved = crate::pairing::new_config();
        saved.direct_port = 9100;
        saved.allow_clipboard = false;
        let merged = with_saved_settings(running.clone(), saved);
        assert_eq!((merged.client_unique_id, merged.server_url, merged.agent_key), (running.client_unique_id, running.server_url, running.agent_key));
        assert_eq!(merged.direct_port, 9100);
        assert!(!merged.allow_clipboard);
    }
}

#[cfg(test)]
mod server_gate_tests {
    use super::*;

    fn incoming(client: &str) -> SignalingMessage {
        SignalingMessage::IncomingFileSession { client_id: client.into(), ice_servers: None, requester: Some("bob".into()), source_id: None }
    }

    fn answer(client: &str) -> SignalingMessage {
        SignalingMessage::IceCandidate {
            target_id: client.into(),
            candidate: common::RtcIceCandidate {
                candidate: String::new(),
                sdp_mid: None,
                sdp_mline_index: None,
                username_fragment: None,
            },
        }
    }

    #[tokio::test]
    async fn trusted_passes_everything_ask_waits_for_the_person_here() {
        let (server_tx, mut server_rx) = mpsc::unbounded_channel();
        let (inbound_tx, mut inbound_rx) = mpsc::unbounded_channel();
        let gate = ServerGate::new(false, server_tx.clone(), inbound_tx.clone());
        gate.pass(incoming("c1"));
        assert!(matches!(inbound_rx.try_recv(), Ok(SignalingMessage::IncomingFileSession { .. })));

        let gate = ServerGate::new(true, server_tx, inbound_tx);
        gate.pass(SignalingMessage::GetCapabilities { target_id: "c1".into() });
        assert!(matches!(inbound_rx.try_recv(), Ok(SignalingMessage::GetCapabilities { .. })));
        // Nothing from a client that was not accepted reaches the session.
        gate.pass(answer("c1"));
        gate.pass(incoming("c1"));
        assert!(inbound_rx.try_recv().is_err());
        match server_rx.try_recv() {
            Ok(AgentMessage::Signaling(SignalingMessage::AccessRequest { target_id, status })) => {
                assert_eq!((target_id.as_str(), status.as_str()), ("c1", "waiting"));
            }
            other => panic!("{:?}", other),
        }
        // Once granted, that kind and the messages that follow pass.
        gate.accepted.lock().unwrap().insert(("c1".into(), approval::Kind::Files));
        gate.pass(answer("c1"));
        assert!(matches!(inbound_rx.try_recv(), Ok(SignalingMessage::IceCandidate { .. })));
        gate.pass(answer("c2"));
        assert!(inbound_rx.try_recv().is_err());
    }

    fn session(client: &str) -> SignalingMessage {
        serde_json::from_value(serde_json::json!({
            "type": "IncomingSession",
            "payload": {"client_id": client, "width": null, "height": null, "fps": null, "bitrate": null,
                        "codec": null, "app_id": null, "encoder": null, "display_id": null, "ice_servers": null}
        }))
        .unwrap()
    }

    /// A gate whose questions are answered "accept" when `answer` is notified.
    fn gate_with_answer() -> (
        ServerGate,
        Arc<tokio::sync::Notify>,
        mpsc::UnboundedReceiver<AgentMessage>,
        mpsc::UnboundedReceiver<SignalingMessage>,
    ) {
        let (server_tx, server_rx) = mpsc::unbounded_channel();
        let (inbound_tx, inbound_rx) = mpsc::unbounded_channel();
        let mut gate = ServerGate::new(true, server_tx, inbound_tx);
        let answer = Arc::new(tokio::sync::Notify::new());
        let a = answer.clone();
        gate.ask_fn = Arc::new(move |_, _, cancel| {
            let a = a.clone();
            Box::pin(async move {
                tokio::select! {
                    _ = a.notified() => Some(approval::Decision::Accepted),
                    _ = cancel.notified() => None,
                }
            })
        });
        (gate, answer, server_rx, inbound_rx)
    }

    async fn settle() {
        for _ in 0..20 {
            tokio::task::yield_now().await;
        }
    }

    #[tokio::test]
    async fn a_client_that_left_while_asked_gets_nothing() {
        let (gate, answer, mut server_rx, mut inbound_rx) = gate_with_answer();
        gate.pass(session("c1"));
        assert!(matches!(server_rx.try_recv(), Ok(AgentMessage::Signaling(SignalingMessage::AccessRequest { .. }))));
        gate.pass(SignalingMessage::EndSession { target_id: "c1".into() });
        assert!(matches!(inbound_rx.try_recv(), Ok(SignalingMessage::EndSession { .. })));
        assert!(gate.asking.lock().unwrap().is_empty());
        // Accepted after the client left: the request is not delivered.
        answer.notify_waiters();
        settle().await;
        assert!(inbound_rx.try_recv().is_err());
        assert!(server_rx.try_recv().is_err());
    }

    #[tokio::test]
    async fn grants_end_with_the_client_and_stop_needs_control() {
        let (gate, answer, _server_rx, mut inbound_rx) = gate_with_answer();
        gate.pass(session("c1"));
        settle().await;
        answer.notify_waiters();
        settle().await;
        assert!(matches!(inbound_rx.try_recv(), Ok(SignalingMessage::IncomingSession { .. })));
        assert!(gate.accepted.lock().unwrap().contains(&("c1".to_string(), approval::Kind::Control)));
        // Only a client with screen control stops the stream.
        gate.accepted.lock().unwrap().insert(("c2".into(), approval::Kind::Files));
        gate.pass(SignalingMessage::StopActiveStream { target_id: "c2".into() });
        assert!(inbound_rx.try_recv().is_err());
        gate.pass(SignalingMessage::StopActiveStream { target_id: "c1".into() });
        assert!(matches!(inbound_rx.try_recv(), Ok(SignalingMessage::StopActiveStream { .. })));
        // Its end prunes the grants: later messages from it are dropped.
        gate.pass(SignalingMessage::EndSession { target_id: "c1".into() });
        assert!(matches!(inbound_rx.try_recv(), Ok(SignalingMessage::EndSession { .. })));
        assert!(!gate.accepted.lock().unwrap().iter().any(|(c, _)| c == "c1"));
        gate.pass(answer_msg("c1"));
        assert!(inbound_rx.try_recv().is_err());
    }

    fn answer_msg(client: &str) -> SignalingMessage {
        answer(client)
    }
}

#[cfg(test)]
mod retry_tests {
    use super::*;
    use std::time::Duration;

    fn secs(failures: u32, refused: bool, jitter: f64) -> f64 {
        retry_delay(failures, refused, jitter).as_secs_f64()
    }

    #[test]
    fn refused_agents_back_off_to_five_minutes() {
        // Without jitter (0.5 is the middle): 3, 6, 12, ... capped at 300 s.
        let seq: Vec<f64> = (1..=9).map(|n| secs(n, true, 0.5)).collect();
        assert_eq!(seq, vec![3.0, 6.0, 12.0, 24.0, 48.0, 96.0, 192.0, 300.0, 300.0]);
        assert_eq!(secs(u32::MAX, true, 0.5), 300.0);
        // Unreachable servers: the same start, capped at 30 s.
        let seq: Vec<f64> = (1..=6).map(|n| secs(n, false, 0.5)).collect();
        assert_eq!(seq, vec![3.0, 6.0, 12.0, 24.0, 30.0, 30.0]);
        // Jitter: ±20 %, never past it even for odd input.
        assert!((secs(1, true, 0.0) - 2.4).abs() < 1e-9);
        assert!((secs(1, true, 1.0) - 3.6).abs() < 1e-9);
        assert!(retry_delay(20, true, 7.0) <= Duration::from_secs(360));
        assert!(retry_delay(20, true, -1.0) >= Duration::from_secs(240));
        let j = random_unit();
        assert!((0.0..=1.0).contains(&j));
    }

    /// A refused agent stays well under the server's per-address limit
    /// (300 refusals in 5 minutes): after the first few, about one a 5 min.
    #[test]
    fn refusals_per_five_minutes_stay_low() {
        let mut t = 0.0;
        let mut attempts = 0;
        let mut n = 0;
        while t < 3600.0 {
            n += 1;
            t += secs(n, true, 0.0);
            attempts += 1;
        }
        assert!(attempts < 30, "{attempts} attempts in an hour");
    }

    #[test]
    fn refusals_are_told_apart_from_network_errors() {
        let refused: Box<dyn std::error::Error + Send + Sync> = Box::new(ServerRefused { status: 401, message: "no".into() });
        assert!(refused.downcast_ref::<ServerRefused>().is_some_and(|r| r.status == 401));
        assert_eq!(refused.to_string(), "no");
        let other: Box<dyn std::error::Error + Send + Sync> = anyhow::anyhow!("connection refused").into();
        assert!(other.downcast_ref::<ServerRefused>().is_none());
    }
}
