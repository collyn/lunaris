//! This computer's own settings, read and changed by the administrators of
//! the Lunaris server it is connected to (`ServerToAgentMessage::Settings`).
//!
//! Only when the person here allows it: "Allow the server to change this
//! computer's settings" (`allow_server_settings` of the config, off by
//! default) is set on the computer with `lunaris-agent set server-settings
//! allow` or `--allow-server-settings`, and never from the server. While it is off the
//! server gets the settings to read (never the access password) and every
//! change is refused. The password is sent only when asked for on its own
//! (`ShowPassword`), and only while changes are allowed; a new one passes
//! the other way once. Neither is ever written to the log.
//!
//! A change is saved in the config file and applied as the apps apply
//! theirs, by starting the agent's local parts again (direct connections,
//! RustDesk compatibility, the session logic: sessions that run end), while
//! the connection to the server stays (`run_agent`). Through a server that
//! connects to this computer by its address (direct.rs `/ws/server`) direct
//! access is not turned off, moved or given a new password: that server
//! could not come back.

use common::{AgentMessage, AgentSettingsChange, AgentSettingsMsg, SecretString, SettingsRequest};
use tracing::{info, warn};

use crate::pairing::AgentConfig;

/// Counts the changes saved: `run_agent` watches it (`changes`) and starts
/// its local parts again with the config as saved. A count, not a wake-up
/// permit: a change made while no agent waits is not left behind for the
/// next one (which reads the config anyway), and one made while the agent
/// restarts is still seen.
fn saved_changes() -> &'static tokio::sync::watch::Sender<u64> {
    static CHANGED: std::sync::OnceLock<tokio::sync::watch::Sender<u64>> = std::sync::OnceLock::new();
    CHANGED.get_or_init(|| tokio::sync::watch::channel(0).0)
}

/// For `run_agent`: changes saved from now on.
pub(crate) fn changes() -> tokio::sync::watch::Receiver<u64> {
    saved_changes().subscribe()
}

/// The way a request came.
#[derive(Clone, Debug)]
pub struct Channel {
    /// For the log ("the server", "server link ...").
    pub via: String,
    /// A server connected to this computer by its address (direct.rs).
    pub link: bool,
    /// That server proved the access password (always for the server this
    /// agent connects to itself).
    pub trusted: bool,
    /// On this computer (`lunaris-agent set`), not from any server: only
    /// this may widen what remote people reach (`remote_widening`).
    pub local: bool,
}

impl Channel {
    pub fn server() -> Self {
        Self { via: "the server".into(), link: false, trusted: true, local: false }
    }

    /// A server that connected by this computer's address (direct.rs).
    pub fn link(via: String, trusted: bool) -> Self {
        Self { via, link: true, trusted, local: false }
    }

    /// `lunaris-agent set` on this computer.
    pub fn local() -> Self {
        Self { via: "this computer".into(), link: false, trusted: true, local: true }
    }
}

/// Remote keyboard and mouse can work (Linux: /dev/uinput is writable).
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

fn allowed(config: &AgentConfig, channel: &Channel) -> bool {
    config.allow_server_settings && (!channel.link || channel.trusted)
}

/// The settings as the server is told them (no password).
pub fn view(config: &AgentConfig, channel: &Channel) -> AgentSettingsMsg {
    AgentSettingsMsg {
        remote_changes: allowed(config, channel),
        direct_enabled: config.direct_enabled,
        direct_port: config.direct_port,
        has_password: !config.direct_password.trim().is_empty(),
        // As approval.rs reads it: anything else is "password-click".
        approve_mode: match config.approve_mode.trim() {
            mode @ ("password" | "click") => mode,
            _ => "password-click",
        }
        .to_string(),
        server_sessions: if config.server_sessions.trim() == "ask" { "ask" } else { "trusted" }.to_string(),
        allow_clipboard: config.allow_clipboard,
        allow_file_transfer: config.allow_file_transfer,
        file_access: crate::file_access_setting(config),
        rustdesk_enabled: config.rustdesk_port.is_some(),
        rustdesk_port: config.rustdesk_port.unwrap_or(crate::rustdesk::DEFAULT_PORT),
        rustdesk_file_transfer: config.rustdesk_file_transfer,
        wayland_capture: crate::pairing::wayland_capture_preference(&config.wayland_capture).0.as_str().to_string(),
        input_ready: input_ready(),
        version: env!("CARGO_PKG_VERSION").to_string(),
        os: std::env::consts::OS.to_string(),
        linked_by_address: channel.link,
    }
}

/// Why not: (code, words for the person).
type Refusal = (&'static str, String);

fn invalid(text: &str) -> Refusal {
    (common::SETTINGS_INVALID, text.to_string())
}

const LINKED: &str = "This computer was added to the server by its address: direct access (on, port, password) cannot be changed from there, or the server could not reach it again. Change it on the computer.";

const LOCAL_ONLY_FILES: &str = "File transfer can be given the whole disk only on that computer (`lunaris-agent set file-access full`)";
const LOCAL_ONLY_SESSIONS: &str = "Sessions through the server are asked about at that computer: only the person there can change that (`lunaris-agent set server-sessions trusted`)";

/// A change from a server that would give remote people more than the
/// person here chose: the whole disk (or a folder outside the home folder)
/// for file transfer, or sessions through the server let in without asking
/// when the person here asked to be asked. Values that stay as they are
/// pass (the web page sends the whole form).
fn remote_widening(config: &AgentConfig, change: &AgentSettingsChange) -> Option<&'static str> {
    if let Some(access) = change.file_access.as_deref().map(str::trim) {
        let current = crate::file_access_setting(config);
        if access != current && !crate::file_access_within_home(access) {
            return Some(LOCAL_ONLY_FILES);
        }
    }
    if let Some(mode) = change.server_sessions.as_deref().map(str::trim) {
        if config.server_sessions.trim() == "ask" && mode != "ask" {
            return Some(LOCAL_ONLY_SESSIONS);
        }
    }
    None
}

/// Makes `change` in `config`, checked as the apps check it.
pub fn apply_change(config: &mut AgentConfig, change: &AgentSettingsChange, channel: &Channel) -> Result<(), Refusal> {
    if let Some(error) = common::settings_change_error(change, config.direct_port) {
        return Err(invalid(&error));
    }
    if !channel.local {
        if let Some(why) = remote_widening(config, change) {
            return Err((common::SETTINGS_NOT_ALLOWED, why.to_string()));
        }
    }
    if channel.link
        && (change.direct_enabled == Some(false) || change.direct_port.is_some_and(|port| port != config.direct_port))
    {
        return Err(invalid(LINKED));
    }
    // The RustDesk port against the direct port as they will be.
    let direct_port = change.direct_port.unwrap_or(config.direct_port);
    let rustdesk_port = match change.rustdesk_enabled {
        Some(false) => None,
        Some(true) => Some(change.rustdesk_port.or(config.rustdesk_port).unwrap_or(crate::rustdesk::DEFAULT_PORT)),
        None => config.rustdesk_port.map(|port| change.rustdesk_port.unwrap_or(port)),
    };
    if rustdesk_port == Some(direct_port) {
        return Err(invalid("That port is the one for direct connections"));
    }
    if let Some(on) = change.direct_enabled {
        config.direct_enabled = on;
    }
    config.direct_port = direct_port;
    config.rustdesk_port = rustdesk_port;
    if let Some(mode) = &change.approve_mode {
        config.approve_mode = mode.clone();
    }
    if let Some(mode) = &change.server_sessions {
        config.server_sessions = mode.clone();
    }
    if let Some(on) = change.allow_clipboard {
        config.allow_clipboard = on;
    }
    if let Some(on) = change.allow_file_transfer {
        config.allow_file_transfer = on;
    }
    if let Some(access) = &change.file_access {
        config.file_access = access.clone();
    }
    if let Some(on) = change.rustdesk_file_transfer {
        config.rustdesk_file_transfer = on;
    }
    if let Some(mode) = &change.wayland_capture {
        config.wayland_capture = mode.clone();
    }
    // Direct and RustDesk clients log in with the access password.
    if config.direct_enabled || config.rustdesk_port.is_some() {
        crate::pairing::ensure_direct_password(config);
    }
    Ok(())
}

/// A new access password: `password`, or a new random one.
pub fn apply_password(config: &mut AgentConfig, password: Option<&SecretString>, channel: &Channel) -> Result<(), Refusal> {
    if channel.link {
        return Err(invalid(LINKED));
    }
    config.direct_password = match password {
        Some(SecretString(password)) if common::valid_access_password(password) => password.clone(),
        Some(_) => {
            return Err(invalid(&format!(
                "The password needs {} to {} characters, without spaces at either end",
                common::MIN_ACCESS_PASSWORD_CHARS,
                common::MAX_ACCESS_PASSWORD_CHARS
            )))
        }
        None => crate::pairing::generate_direct_password(),
    };
    Ok(())
}

/// What the agent answers: done in the config file at `path` (read again
/// first, so what the apps saved meanwhile is kept). A change wakes
/// `run_agent` (`changes`).
pub fn reply(request_id: String, request: &SettingsRequest, requester: Option<&str>, channel: &Channel, path: &str) -> AgentMessage {
    let answer = |settings: Option<AgentSettingsMsg>, password: Option<SecretString>, refusal: Option<Refusal>| {
        let (code, error) = match refusal {
            Some((code, error)) => (Some(code.to_string()), Some(error)),
            None => (None, None),
        };
        AgentMessage::SettingsAnswer { request_id: request_id.clone(), settings, password, error, code }
    };
    // A server let in without the password is anyone who knows this
    // address: it is not told this computer's settings (nor may it change
    // them, `allowed`). Before the file is even read.
    if channel.link && !channel.trusted {
        info!(
            "Settings: {:?} asked for by {} through {}: refused (that server did not prove the access password)",
            request,
            requester.unwrap_or("someone"),
            channel.via
        );
        return answer(
            None,
            None,
            Some((common::SETTINGS_NOT_ALLOWED, "This computer's settings are shown only to a server that has its access password".into())),
        );
    }
    // Read, change and save under the lock sources.rs writes the same file
    // with, so a source change at the same moment is not lost.
    let _writing = crate::connector::sources::CONFIG_WRITES.lock().unwrap_or_else(|e| e.into_inner());
    let mut config = match crate::pairing::load_config(path) {
        Ok(config) => config,
        Err(e) => {
            warn!("Settings: {} could not be read: {}", path, e);
            return answer(None, None, Some((common::SETTINGS_FAILED, "That computer's settings could not be read".into())));
        }
    };
    let who = requester.unwrap_or("someone");
    let changed = match request {
        SettingsRequest::Get => return answer(Some(view(&config, channel)), None, None),
        _ if !allowed(&config, channel) => {
            info!("Settings: {:?} asked for by {} through {}: refused (changes from the server are not allowed here)", request, who, channel.via);
            return answer(
                None,
                None,
                Some((
                    common::SETTINGS_NOT_ALLOWED,
                    "The person at that computer has not allowed the server to change its settings".into(),
                )),
            );
        }
        SettingsRequest::ShowPassword => {
            info!("Settings: the access password was shown to {} through {}", who, channel.via);
            let password = SecretString(config.direct_password.clone());
            return answer(Some(view(&config, channel)), Some(password), None);
        }
        SettingsRequest::Change { change } => apply_change(&mut config, change, channel),
        SettingsRequest::NewPassword { password } => apply_password(&mut config, password.as_ref(), channel),
    };
    if let Err(refusal) = changed {
        info!("Settings: {:?} asked for by {} through {}: refused ({})", request, who, channel.via, refusal.1);
        return answer(None, None, Some(refusal));
    }
    if let Err(e) = crate::pairing::save_config(&config, path) {
        warn!("Settings: {} could not be saved: {}", path, e);
        return answer(None, None, Some((common::SETTINGS_FAILED, "That computer's settings could not be saved".into())));
    }
    info!("Settings: {:?} made by {} through {}; local services start again with it", request, who, channel.via);
    saved_changes().send_modify(|count| *count += 1);
    answer(Some(view(&config, channel)), None, None)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_config(name: &str, config: &AgentConfig) -> (std::path::PathBuf, String) {
        let dir = std::env::temp_dir().join(format!("lunaris-settings-{}-{}", name, std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("agent_config.json");
        crate::pairing::save_config(config, path.to_str().unwrap()).unwrap();
        let p = path.to_str().unwrap().to_string();
        (dir, p)
    }

    fn base(allowed: bool) -> AgentConfig {
        let mut c = crate::pairing::new_config();
        c.direct_password = "old-password".into();
        c.allow_server_settings = allowed;
        c
    }

    fn unpack(message: AgentMessage) -> (Option<AgentSettingsMsg>, Option<SecretString>, Option<String>) {
        match message {
            AgentMessage::SettingsAnswer { settings, password, code, .. } => (settings, password, code),
            other => panic!("{:?}", other),
        }
    }

    #[test]
    fn switch_is_off_by_default_and_the_values_are_read_only_then() {
        assert!(!AgentConfig::default().allow_server_settings);
        assert!(!crate::pairing::new_config().allow_server_settings);
        let (dir, path) = temp_config("off", &base(false));
        let (settings, password, code) = unpack(reply("r".into(), &SettingsRequest::Get, Some("admin"), &Channel::server(), &path));
        let settings = settings.unwrap();
        assert!(!settings.remote_changes && settings.has_password && password.is_none() && code.is_none());
        assert_eq!(settings.version, env!("CARGO_PKG_VERSION"));
        for request in [
            SettingsRequest::ShowPassword,
            SettingsRequest::NewPassword { password: None },
            SettingsRequest::Change { change: AgentSettingsChange { allow_clipboard: Some(false), ..Default::default() } },
        ] {
            let (settings, password, code) = unpack(reply("r".into(), &request, None, &Channel::server(), &path));
            assert!(settings.is_none() && password.is_none());
            assert_eq!(code.as_deref(), Some(common::SETTINGS_NOT_ALLOWED));
        }
        let after = crate::pairing::load_config(&path).unwrap();
        assert!(after.allow_clipboard && after.direct_password == "old-password");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn allowed_changes_are_saved_and_the_password_only_shown_when_asked() {
        let (dir, path) = temp_config("on", &base(true));
        let change = AgentSettingsChange {
            direct_enabled: Some(true),
            direct_port: Some(9100),
            approve_mode: Some("click".into()),
            server_sessions: Some("ask".into()),
            allow_clipboard: Some(false),
            file_access: Some("home".into()),
            rustdesk_enabled: Some(true),
            rustdesk_port: Some(21120),
            wayland_capture: Some("portal".into()),
            ..Default::default()
        };
        let (settings, password, code) = unpack(reply("r".into(), &SettingsRequest::Change { change }, Some("admin"), &Channel::server(), &path));
        let settings = settings.unwrap();
        assert!(code.is_none() && password.is_none());
        assert!(settings.remote_changes && settings.direct_enabled && !settings.allow_clipboard);
        assert_eq!((settings.direct_port, settings.rustdesk_port), (9100, 21120));
        assert_eq!((settings.approve_mode.as_str(), settings.server_sessions.as_str()), ("click", "ask"));
        assert_eq!((settings.file_access.as_str(), settings.wayland_capture.as_str()), ("home", "portal"));
        let saved = crate::pairing::load_config(&path).unwrap();
        assert_eq!((saved.direct_port, saved.rustdesk_port, saved.file_access.as_str()), (9100, Some(21120), "home"));
        // The switch itself stays as the person set it.
        assert!(saved.allow_server_settings);

        let (_, password, _) = unpack(reply("r".into(), &SettingsRequest::ShowPassword, None, &Channel::server(), &path));
        assert_eq!(password.unwrap().0, "old-password");
        let (settings, password, _) = unpack(reply("r".into(), &SettingsRequest::NewPassword { password: None }, None, &Channel::server(), &path));
        assert!(settings.is_some() && password.is_none());
        let random = crate::pairing::load_config(&path).unwrap().direct_password;
        assert!(random != "old-password" && random.len() == 14);
        let set = SettingsRequest::NewPassword { password: Some(SecretString("my own password".into())) };
        assert!(unpack(reply("r".into(), &set, None, &Channel::server(), &path)).2.is_none());
        assert_eq!(crate::pairing::load_config(&path).unwrap().direct_password, "my own password");
        for short in ["abc", "my own pw", "elevenchars"] {
            let short = SettingsRequest::NewPassword { password: Some(SecretString(short.into())) };
            assert_eq!(unpack(reply("r".into(), &short, None, &Channel::server(), &path)).2.as_deref(), Some(common::SETTINGS_INVALID));
        }
        assert_eq!(crate::pairing::load_config(&path).unwrap().direct_password, "my own password");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn invalid_changes_are_refused_and_nothing_is_saved() {
        let mut c = base(true);
        for change in [
            AgentSettingsChange { direct_port: Some(80), ..Default::default() },
            AgentSettingsChange { approve_mode: Some("always".into()), ..Default::default() },
            AgentSettingsChange { rustdesk_enabled: Some(true), rustdesk_port: Some(c.direct_port), ..Default::default() },
            AgentSettingsChange { file_access: Some("/".into()), ..Default::default() },
        ] {
            assert_eq!(apply_change(&mut c, &change, &Channel::server()).unwrap_err().0, common::SETTINGS_INVALID, "{:?}", change);
        }
        // Moving the direct port onto RustDesk's.
        c.rustdesk_port = Some(21118);
        let onto = AgentSettingsChange { direct_port: Some(21118), ..Default::default() };
        assert!(apply_change(&mut c, &onto, &Channel::server()).is_err());
        assert_eq!(c.direct_port, crate::pairing::default_direct_port());
    }

    #[test]
    fn a_server_linked_by_address_cannot_cut_itself_off() {
        let mut c = base(true);
        c.direct_enabled = true;
        let link = Channel::link("link".into(), true);
        for change in [
            AgentSettingsChange { direct_enabled: Some(false), ..Default::default() },
            AgentSettingsChange { direct_port: Some(9999), ..Default::default() },
        ] {
            assert!(apply_change(&mut c, &change, &link).is_err());
        }
        assert!(apply_password(&mut c, None, &link).is_err());
        assert!(apply_change(&mut c, &AgentSettingsChange { allow_clipboard: Some(false), ..Default::default() }, &link).is_ok());
        assert!(c.direct_enabled && !c.allow_clipboard);
        // One let in without the password may only read.
        let untrusted = Channel::link("link".into(), false);
        assert!(!view(&c, &untrusted).remote_changes && view(&c, &link).remote_changes);
    }

    #[test]
    fn a_saved_short_password_keeps_working() {
        // Saved when 6 characters were enough: loaded and kept as it is.
        let (dir, path) = temp_config("short-pw", &base(true));
        let c = crate::pairing::load_config(&path).unwrap();
        assert_eq!(c.direct_password, "old-password");
        let mut short = c.clone();
        short.direct_password = "abc123".into();
        crate::pairing::save_config(&short, &path).unwrap();
        // Other changes leave it alone.
        let change = SettingsRequest::Change { change: AgentSettingsChange { allow_clipboard: Some(false), ..Default::default() } };
        assert!(unpack(reply("r".into(), &change, None, &Channel::server(), &path)).2.is_none());
        assert_eq!(crate::pairing::load_config(&path).unwrap().direct_password, "abc123");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_server_cannot_widen_what_remote_people_reach() {
        let mut c = base(true);
        let server = Channel::server();
        let full = AgentSettingsChange { file_access: Some("full".into()), ..Default::default() };
        let (code, _) = apply_change(&mut c, &full, &server).unwrap_err();
        assert_eq!(code, common::SETTINGS_NOT_ALLOWED);
        assert_eq!(c.file_access, "");
        // Narrowing, or sending back what is set, is fine.
        c.file_access = "full".into();
        apply_change(&mut c, &full, &server).unwrap();
        assert_eq!(c.file_access, "full", "an existing \"full\" stays");
        apply_change(&mut c, &AgentSettingsChange { file_access: Some("home".into()), ..Default::default() }, &server).unwrap();
        assert_eq!(c.file_access, "home");

        // "ask" is changed back only here.
        c.server_sessions = "ask".into();
        let trusted = AgentSettingsChange { server_sessions: Some("trusted".into()), ..Default::default() };
        assert_eq!(apply_change(&mut c, &trusted, &server).unwrap_err().0, common::SETTINGS_NOT_ALLOWED);
        assert_eq!(c.server_sessions, "ask");
        apply_change(&mut c, &AgentSettingsChange { server_sessions: Some("ask".into()), ..Default::default() }, &server).unwrap();
        let mut t = base(true);
        apply_change(&mut t, &AgentSettingsChange { server_sessions: Some("ask".into()), ..Default::default() }, &server).unwrap();
        assert_eq!(t.server_sessions, "ask", "asking more is allowed from the server");
        // A trusted link is a server too.
        let link = Channel::link("link".into(), true);
        assert!(apply_change(&mut c, &trusted, &link).is_err());
        // On this computer both are possible.
        apply_change(&mut c, &trusted, &Channel::local()).unwrap();
        apply_change(&mut c, &full, &Channel::local()).unwrap();
        assert_eq!((c.server_sessions.as_str(), c.file_access.as_str()), ("trusted", "full"));
    }

    #[test]
    fn a_link_without_the_password_is_not_told_the_settings() {
        let (dir, path) = temp_config("untrusted", &base(true));
        let untrusted = Channel::link("link".into(), false);
        let (settings, password, code) = unpack(reply("r".into(), &SettingsRequest::Get, None, &untrusted, &path));
        assert!(settings.is_none() && password.is_none());
        assert_eq!(code.as_deref(), Some(common::SETTINGS_NOT_ALLOWED));
        // One with the password still is.
        let (settings, _, code) = unpack(reply("r".into(), &SettingsRequest::Get, None, &Channel::link("link".into(), true), &path));
        assert!(settings.is_some() && code.is_none());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn turning_direct_access_on_gives_it_a_password() {
        let mut c = base(true);
        c.direct_password.clear();
        apply_change(&mut c, &AgentSettingsChange { direct_enabled: Some(true), ..Default::default() }, &Channel::server()).unwrap();
        assert!(!c.direct_password.is_empty());
    }
}
