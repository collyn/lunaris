//! `lunaris-agent status` and `lunaris-agent set`: what the agent window
//! once showed and changed, on the command line. The same settings are
//! changed from the server's web page (settings.rs) when this computer
//! allows it.

use std::io::{BufRead, IsTerminal};

use common::{AgentSettingsChange, SecretString};

use crate::pairing::AgentConfig;
use crate::status::Seen;

type Res<T> = Result<T, Box<dyn std::error::Error + Send + Sync>>;

/// The settings `set` changes.
#[derive(clap::ValueEnum, Clone, Copy, Debug, PartialEq, Eq)]
pub enum SettingKey {
    /// Direct connections (no server): on|off
    Direct,
    /// Port of direct connections (1024-65535)
    DirectPort,
    /// RustDesk clients may connect: on|off
    Rustdesk,
    /// Port for RustDesk clients (turns it on)
    RustdeskPort,
    /// RustDesk's file manager: on|off
    RustdeskFileTransfer,
    /// How connections without the server are let in: password|click|password-click
    ApproveMode,
    /// Sessions through the server: trusted|ask
    ServerSessions,
    /// Clipboard sharing: on|off
    Clipboard,
    /// File transfer: on|off
    FileTransfer,
    /// What file transfer reaches: home|full
    FileAccess,
    /// Screen capture on Wayland: auto|portal
    WaylandCapture,
    /// The server's administrators may change these settings: allow|deny
    ServerSettings,
    /// Older apps may log in over plain ws:// with the password in the URL: on|off
    LegacyDirect,
    /// The server's administrators may update this agent from the web: allow|deny
    ServerUpdate,
    /// The access password: `random`, or `-` to read it from stdin
    Password,
    /// This computer's name on the server
    Name,
    /// The server address (better: --enroll with an install token)
    Server,
    /// An agent token of the server (older servers): `-`, read from stdin
    ServerToken,
}

fn on_off(value: &str) -> Res<bool> {
    match value.trim().to_ascii_lowercase().as_str() {
        "on" | "true" | "yes" | "1" | "allow" => Ok(true),
        "off" | "false" | "no" | "0" | "deny" => Ok(false),
        other => Err(format!("expected on or off, not {other:?}").into()),
    }
}

fn port(value: &str) -> Res<u16> {
    value.trim().parse::<u16>().map_err(|_| format!("not a port: {value:?}").into())
}

/// One line from stdin (a secret: never on the command line).
fn read_stdin_line(what: &str) -> Res<String> {
    if std::io::stdin().is_terminal() {
        eprintln!("{what} (then Enter):");
    }
    let mut line = String::new();
    std::io::stdin().lock().read_line(&mut line)?;
    let line = line.trim_end_matches(['\r', '\n']).to_string();
    if line.is_empty() {
        return Err(format!("no {what} on stdin").into());
    }
    Ok(line)
}

fn load_or_new(config_path: &str) -> Res<AgentConfig> {
    if std::path::Path::new(config_path).exists() {
        crate::pairing::load_config(config_path).map_err(|e| format!("cannot read {config_path}: {e}").into())
    } else {
        Ok(crate::pairing::new_config())
    }
}

/// Makes the change in `config` (no file involved).
fn apply(config: &mut AgentConfig, key: SettingKey, value: &str, stdin: impl FnOnce(&str) -> Res<String>) -> Res<()> {
    let channel = crate::settings::Channel::local();
    let mut change = AgentSettingsChange::default();
    let word = || value.trim().to_ascii_lowercase();
    match key {
        SettingKey::Direct => change.direct_enabled = Some(on_off(value)?),
        SettingKey::DirectPort => change.direct_port = Some(port(value)?),
        SettingKey::Rustdesk => change.rustdesk_enabled = Some(on_off(value)?),
        SettingKey::RustdeskPort => {
            change.rustdesk_enabled = Some(true);
            change.rustdesk_port = Some(port(value)?);
        }
        SettingKey::RustdeskFileTransfer => change.rustdesk_file_transfer = Some(on_off(value)?),
        SettingKey::ApproveMode => change.approve_mode = Some(word()),
        SettingKey::ServerSessions => change.server_sessions = Some(word()),
        SettingKey::Clipboard => change.allow_clipboard = Some(on_off(value)?),
        SettingKey::FileTransfer => change.allow_file_transfer = Some(on_off(value)?),
        SettingKey::FileAccess => change.file_access = Some(word()),
        SettingKey::WaylandCapture => change.wayland_capture = Some(word()),
        SettingKey::ServerSettings => {
            config.allow_server_settings = on_off(value)?;
            return Ok(());
        }
        SettingKey::ServerUpdate => {
            config.allow_server_update = on_off(value)?;
            return Ok(());
        }
        SettingKey::LegacyDirect => {
            config.allow_legacy_plain_direct = on_off(value)?;
            return Ok(());
        }
        SettingKey::Password => {
            let password = match value.trim() {
                "random" => None,
                "-" => Some(SecretString(stdin("the new access password")?)),
                _ => return Err("the password is `random` or `-` (read from stdin, not given on the command line)".into()),
            };
            crate::settings::apply_password(config, password.as_ref(), &channel).map_err(|(_, why)| why)?;
            return Ok(());
        }
        SettingKey::Name => {
            let name = value.trim();
            if name.is_empty() || name.len() > 128 {
                return Err("the name must be 1 to 128 bytes".into());
            }
            config.name = name.to_string();
            return Ok(());
        }
        SettingKey::Server => {
            let (url, _) = crate::normalize_server_url(value)?;
            let same = crate::normalize_server_url(&config.server_url).map(|(u, _)| u == url).unwrap_or(false);
            if !same {
                // A certificate pinned for another server says nothing about this one.
                config.server_cert_sha256.clear();
                config.server_cert_host.clear();
            }
            config.server_url = url;
            return Ok(());
        }
        SettingKey::ServerToken => {
            if value.trim() != "-" {
                return Err("the token is read from stdin: `set server-token -`".into());
            }
            config.server_token = stdin("the server's agent token")?.trim().to_string();
            return Ok(());
        }
    }
    crate::settings::apply_change(config, &change, &channel).map_err(|(_, why)| why)?;
    Ok(())
}

/// `lunaris-agent set KEY VALUE`.
pub fn set(config_path: &str, key: SettingKey, value: &str) -> Res<()> {
    let mut config = load_or_new(config_path)?;
    apply(&mut config, key, value, read_stdin_line)?;
    if let Some(dir) = std::path::Path::new(config_path).parent().filter(|d| !d.as_os_str().is_empty()) {
        std::fs::create_dir_all(dir)?;
    }
    crate::pairing::save_config(&config, config_path)?;
    println!("Saved in {config_path}.");
    if matches!(crate::status::read(config_path), Seen::Running(_)) {
        println!("The agent running with it uses the change once it starts again: {}", restart_hint(config_path));
    }
    Ok(())
}

/// What `status` says under the server address when the connection to it
/// is not encrypted.
fn server_warning(server_url: &str) -> Option<&'static str> {
    if server_url.trim().is_empty() {
        return None;
    }
    (crate::server_transport(&crate::server_ws_url(server_url)) == crate::ServerTransport::Plain)
        .then_some("WARNING: unencrypted connection to the server (ws://); use its wss:// address: `lunaris-agent set server wss://...`")
}

fn restart_hint(config_path: &str) -> String {
    let system = crate::system_config_path().is_some_and(|p| p.to_string_lossy() == config_path);
    if cfg!(target_os = "windows") {
        "restart the LunarisAgentService service (Restart-Service LunarisAgentService)".into()
    } else if system {
        "sudo systemctl restart lunaris-agent-system".into()
    } else {
        "systemctl --user restart lunaris-agent".into()
    }
}

fn yes(b: bool) -> &'static str {
    if b {
        "on"
    } else {
        "off"
    }
}

/// `lunaris-agent status`.
pub async fn status(config_path: &str, show_password: bool) -> Res<()> {
    println!("Lunaris agent {}", env!("CARGO_PKG_VERSION"));
    println!("Config:          {config_path}");
    if !std::path::Path::new(config_path).exists() {
        println!("Not set up: no config yet. Join a server with `lunaris-agent --enroll --server URL` (install token in LUNARIS_INSTALL_TOKEN).");
        return Ok(());
    }
    let config = crate::pairing::load_config(config_path).map_err(|e| format!("cannot read {config_path}: {e}"))?;
    let name = Some(config.name.trim().to_string()).filter(|n| !n.is_empty()).unwrap_or_else(|| {
        hostname::get().map(|h| h.to_string_lossy().into_owned()).unwrap_or_default() + " (host name)"
    });
    println!("Name:            {name}");
    println!("Computer id:     {}", config.client_unique_id);
    let server = config.server_url.trim();
    if server.is_empty() {
        println!("Server:          none (direct connections only)");
    } else if !config.install_token.trim().is_empty() {
        println!("Server:          {server} (joins with its install token when the agent starts)");
    } else {
        println!("Server:          {server}");
    }
    if let Some(warning) = server_warning(server) {
        println!("                 {warning}");
    }
    match crate::status::read(config_path) {
        Seen::Running(s) => {
            let state = if server.is_empty() {
                "".to_string()
            } else if s.connected {
                ", connected to the server".to_string()
            } else {
                match &s.last_error {
                    Some(e) => format!(", not connected to the server ({e})"),
                    None => ", not connected to the server yet".to_string(),
                }
            };
            println!("Agent:           running (pid {}, version {}){state}", s.pid, s.version);
        }
        Seen::Stale(s) => println!("Agent:           not running (pid {} has stopped)", s.pid),
        Seen::NotRunning => println!("Agent:           not running"),
    }
    if config.direct_enabled {
        let legacy = if config.allow_legacy_plain_direct {
            "older apps over plain ws:// allowed: `set legacy-direct off`"
        } else {
            "encrypted only"
        };
        println!("Direct access:   on, port {} ({legacy})", config.direct_port);
        for addr in crate::direct::local_addresses().await {
            println!("                 {}:{}", addr, config.direct_port);
        }
    } else {
        println!("Direct access:   off");
    }
    match config.rustdesk_port {
        Some(p) => println!("RustDesk:        on, port {p}, file manager {}", yes(config.rustdesk_file_transfer)),
        None => println!("RustDesk:        off"),
    }
    if show_password {
        if config.direct_password.is_empty() {
            println!("Access password: none yet (made when direct access or RustDesk is turned on)");
        } else {
            println!("Access password: {}", config.direct_password);
        }
    } else {
        println!("Access password: {} (--show-password shows it)", if config.direct_password.is_empty() { "none yet" } else { "set" });
    }
    let view = crate::settings::view(&config, &crate::settings::Channel::server());
    println!("Accepting:       {} (sessions through the server: {})", view.approve_mode, view.server_sessions);
    println!("Clipboard:       {}", yes(config.allow_clipboard));
    println!("File transfer:   {} (reaches: {})", yes(config.allow_file_transfer), view.file_access);
    #[cfg(target_os = "linux")]
    println!("Wayland capture: {}", view.wayland_capture);
    #[cfg(target_os = "linux")]
    println!("Remote input:    {}", if view.input_ready { "ready (/dev/uinput)" } else { "not available (/dev/uinput is not writable)" });
    println!(
        "Server changes:  {}",
        if config.allow_server_settings { "allowed (the server's administrators may change these settings)" } else { "not allowed (the server may only read them; `set server-settings allow`)" }
    );
    println!(
        "Server updates:  {}",
        if config.allow_server_update { "allowed (the server's administrators may update this agent to a signed release; `set server-update deny`)" } else { "not allowed (`set server-update allow`)" }
    );
    if !config.sources.is_empty() {
        println!("Sources:         {} computer(s) of other types reached through this one", config.sources.len());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn no_stdin(_: &str) -> Res<String> {
        panic!("stdin read")
    }

    #[test]
    fn switches_and_ports() {
        let mut c = crate::pairing::new_config();
        c.direct_password.clear();
        apply(&mut c, SettingKey::Direct, "on", no_stdin).unwrap();
        assert!(c.direct_enabled);
        assert!(!c.direct_password.is_empty(), "direct access gets a password");
        apply(&mut c, SettingKey::DirectPort, "9000", no_stdin).unwrap();
        assert_eq!(c.direct_port, 9000);
        assert!(apply(&mut c, SettingKey::DirectPort, "80", no_stdin).is_err());
        apply(&mut c, SettingKey::RustdeskPort, "21200", no_stdin).unwrap();
        assert_eq!(c.rustdesk_port, Some(21200));
        assert!(apply(&mut c, SettingKey::RustdeskPort, "9000", no_stdin).is_err(), "the direct port");
        apply(&mut c, SettingKey::Rustdesk, "off", no_stdin).unwrap();
        assert_eq!(c.rustdesk_port, None);
        apply(&mut c, SettingKey::WaylandCapture, "Portal", no_stdin).unwrap();
        assert_eq!(c.wayland_capture, "portal");
        assert!(apply(&mut c, SettingKey::WaylandCapture, "kms", no_stdin).is_err());
        assert!(apply(&mut c, SettingKey::Clipboard, "maybe", no_stdin).is_err());
        apply(&mut c, SettingKey::ServerSettings, "allow", no_stdin).unwrap();
        assert!(c.allow_server_settings);
    }

    #[test]
    fn secrets_only_from_stdin() {
        let mut c = crate::pairing::new_config();
        let before = c.direct_password.clone();
        apply(&mut c, SettingKey::Password, "random", no_stdin).unwrap();
        assert_ne!(c.direct_password, before);
        assert!(apply(&mut c, SettingKey::Password, "hunter22", no_stdin).is_err());
        apply(&mut c, SettingKey::Password, "-", |_| Ok("a-new-one-of-mine".into())).unwrap();
        assert_eq!(c.direct_password, "a-new-one-of-mine");
        assert!(apply(&mut c, SettingKey::Password, "-", |_| Ok("only11chars".into())).is_err(), "12 at least now");
        assert!(apply(&mut c, SettingKey::Password, "-", |_| Ok("abc".into())).is_err(), "too short");
        assert!(apply(&mut c, SettingKey::ServerToken, "tok", no_stdin).is_err());
        apply(&mut c, SettingKey::ServerToken, "-", |_| Ok(" tok ".into())).unwrap();
        assert_eq!(c.server_token, "tok");
    }

    #[test]
    fn server_change_drops_the_pin() {
        let mut c = crate::pairing::new_config();
        c.server_url = "wss://a:8443".into();
        c.server_cert_sha256 = "ab".repeat(32);
        apply(&mut c, SettingKey::Server, "https://a:8443/", no_stdin).unwrap();
        assert_eq!(c.server_cert_sha256.len(), 64, "same server keeps it");
        apply(&mut c, SettingKey::Server, "b:8090", no_stdin).unwrap();
        // No scheme: encrypted.
        assert_eq!(c.server_url, "wss://b:8090");
        assert!(c.server_cert_sha256.is_empty());
        // An explicit ws:// is kept as asked.
        apply(&mut c, SettingKey::Server, "ws://b:8090", no_stdin).unwrap();
        assert_eq!(c.server_url, "ws://b:8090");
    }

    #[test]
    fn status_warns_about_an_unencrypted_server() {
        assert!(server_warning("ws://srv.lan:8090").is_some());
        assert!(server_warning("http://10.0.0.5:8090").is_some());
        for fine in ["wss://srv.lan:8443", "https://srv", "srv.lan:8090", "ws://127.0.0.1:8090", "ws://localhost:8090", "ws://[::1]:8090", ""] {
            assert!(server_warning(fine).is_none(), "{fine}");
        }
    }

    #[test]
    fn legacy_direct_is_switched_here_only() {
        let mut c = crate::pairing::new_config();
        assert!(c.allow_legacy_plain_direct, "on by default: older apps keep working");
        apply(&mut c, SettingKey::LegacyDirect, "off", no_stdin).unwrap();
        assert!(!c.allow_legacy_plain_direct);
        apply(&mut c, SettingKey::LegacyDirect, "on", no_stdin).unwrap();
        assert!(c.allow_legacy_plain_direct);
        assert!(c.allow_server_update, "on by default");
        apply(&mut c, SettingKey::ServerUpdate, "deny", no_stdin).unwrap();
        assert!(!c.allow_server_update);
        apply(&mut c, SettingKey::ServerUpdate, "allow", no_stdin).unwrap();
        assert!(c.allow_server_update);
    }

    #[test]
    fn widening_is_allowed_here() {
        let mut c = crate::pairing::new_config();
        c.server_sessions = "ask".into();
        apply(&mut c, SettingKey::FileAccess, "full", no_stdin).unwrap();
        apply(&mut c, SettingKey::ServerSessions, "trusted", no_stdin).unwrap();
        assert_eq!((c.file_access.as_str(), c.server_sessions.as_str()), ("full", "trusted"));
    }
}
