use serde::{Deserialize, Serialize};
use std::fs;

fn default_server_url() -> String {
    "ws://127.0.0.1:8090".to_string()
}

fn default_webtransport_port() -> u16 {
    55200
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct AgentConfig {
    #[serde(default)]
    pub client_unique_id: String,
    #[serde(default)]
    pub client_private_key: String,
    #[serde(default)]
    pub client_certificate: String,
    #[serde(default)]
    pub server_certificate: String,
    #[serde(default = "default_server_url")]
    pub server_url: String,
    #[serde(default)]
    pub server_token: String,
    #[serde(default = "default_webtransport_port")]
    pub webtransport_port: u16,
    #[serde(default)]
    pub autostart: bool,
    #[serde(default)]
    pub close_to_tray: bool,
    /// Direct connections: clients connect straight to this agent at
    /// `ws://<address>:<direct_port>` with `direct_password`, without a
    /// Lunaris server (LAN, VPN or a forwarded port).
    #[serde(default)]
    pub direct_enabled: bool,
    #[serde(default = "default_direct_port")]
    pub direct_port: u16,
    #[serde(default)]
    pub direct_password: String,
    /// Clipboard sharing with clients (both ways).
    #[serde(default = "default_true")]
    pub allow_clipboard: bool,
    /// Files sent by clients are saved in Downloads/Lunaris.
    #[serde(default = "default_true")]
    pub allow_file_transfer: bool,
    /// How connections are accepted: "password", "click" (the person here
    /// accepts each one) or "password-click" (either; the default), as in
    /// RustDesk.
    #[serde(default = "default_approve_mode")]
    pub approve_mode: String,
    /// Sessions that arrive through the Lunaris server (`server_url`):
    /// "trusted" (the default) lets in whoever the server lets through, for
    /// unattended access through one's own server; "ask" puts every screen
    /// or file session to the person at this computer (approval.rs) and
    /// refuses it when they decline or nobody is there to answer.
    #[serde(default = "default_server_sessions")]
    pub server_sessions: String,
    /// SHA-256 (hex) of the server's TLS certificate, pinned on the first
    /// connection when it is not valid by the system's roots (the server's
    /// own self-signed certificate), and `server_cert_host` the `host:port`
    /// it belongs to. Later connections must present the same certificate;
    /// clear both to accept a new one.
    #[serde(default)]
    pub server_cert_sha256: String,
    #[serde(default)]
    pub server_cert_host: String,
    /// What file transfer (files.rs) may reach: "home" (the home folder),
    /// "full" (the whole disk) or an absolute folder. Empty (not set) means
    /// "home"; for the Windows service's agent (LocalSystem) the home folder
    /// of the person logged on at the screen (lib.rs `file_access`; before
    /// 0.7.1 it meant the whole disk there). Only `lunaris-agent set` on
    /// this computer may widen it to "full" or a folder outside the home
    /// folder (settings.rs).
    #[serde(default)]
    pub file_access: String,
    /// This agent's own secret for the server (64 hex characters, generated
    /// when missing), sent as `key` on `/ws/agent`.
    #[serde(default)]
    pub agent_key: String,
    /// An install token from `--enroll` (the one-line installer): sent as
    /// `install_token` instead of `server_token` until the server has
    /// accepted it once, then cleared; the agent signs in with
    /// `agent_key` alone afterwards.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub install_token: String,
    /// Name shown for this computer (`--enroll --name`); empty: the host
    /// name. `--name` on the command line wins.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub name: String,
    /// RustDesk compatibility (rustdesk.rs): when set, RustDesk clients can
    /// connect to this port by IP address and see and control this computer
    /// (unencrypted, as RustDesk's direct IP access is: for a LAN or a VPN),
    /// logging in with `direct_password` or accepted here, as `approve_mode`
    /// says. Absent (the default): off. Turned on without a port: 21128
    /// (`rustdesk::DEFAULT_PORT`; RustDesk's own 21118 is often taken by
    /// RustDesk itself).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rustdesk_port: Option<u16>,
    /// RustDesk clients may also use their file manager on this computer
    /// (rustdesk/files.rs), within what `file_access` allows, when
    /// `allow_file_transfer` is on too. Off by default: without it a
    /// file-transfer connection is refused as a RustDesk host with "Enable
    /// file transfer" off refuses it.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub rustdesk_file_transfer: bool,
    /// Other computers this one shows to its viewers, through connectors
    /// (connector/mod.rs): `[{"id", "name", "connector", and the connector's
    /// fields (address, password, ...)}]`. Whoever may use this computer may
    /// use them. The fields stay in this file; viewers and the server see
    /// the id and the name.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sources: Vec<crate::connector::SourceConfig>,
    /// Screen capture on a Linux Wayland session: "auto" (the default: the
    /// capture service without a prompt when installed, else KMS, else the
    /// portal) or "portal" (always the XDG portal: the person here approves
    /// and the desktop shows the screen is shared; the service and KMS are
    /// never used). Anything else counts as "auto" (`wayland_capture_preference`).
    #[serde(default = "default_wayland_capture")]
    pub wayland_capture: String,
    /// "Allow the server to change this computer's settings": the
    /// administrators of the Lunaris server may read and change the settings
    /// above (settings.rs) and see the access password. Off by default; set
    /// here, with `lunaris-agent set server-settings allow` or
    /// `--allow-server-settings`, never from the server. While it is off the
    /// server may only read them (without the password).
    #[serde(default)]
    pub allow_server_settings: bool,
    /// Direct clients from before TLS (direct.rs) may still log in over
    /// plain `ws://` with the password in the URL, readable by anyone on the
    /// network. On by default so older apps keep working; turned off with
    /// `lunaris-agent set legacy-direct off` (only on this computer). Every
    /// such login is logged as a warning.
    #[serde(default = "default_true")]
    pub allow_legacy_plain_direct: bool,
}

fn default_wayland_capture() -> String {
    "auto".to_string()
}

/// `wayland_capture` as lunaris-media takes it, and whether the value was
/// one it knows (an unknown one is `Auto`).
pub fn wayland_capture_preference(value: &str) -> (lunaris_media::capture::WaylandCapturePreference, bool) {
    match lunaris_media::capture::WaylandCapturePreference::parse(value) {
        Some(p) => (p, true),
        None => (lunaris_media::capture::WaylandCapturePreference::Auto, false),
    }
}

impl Default for AgentConfig {
    /// Every field at its serde default (no id, no password).
    fn default() -> Self {
        serde_json::from_str("{}").expect("every field has a default")
    }
}

fn default_server_sessions() -> String {
    "trusted".to_string()
}


fn default_approve_mode() -> String {
    "password-click".to_string()
}

fn default_true() -> bool {
    true
}

pub fn default_direct_port() -> u16 {
    8091
}

/// Bytes from the OS random source.
pub fn random_bytes(n: usize) -> Vec<u8> {
    use ring::rand::SecureRandom;
    let mut buf = vec![0u8; n];
    let rng = ring::rand::SystemRandom::new();
    // It does not fail on supported systems; retry rather than use weak bytes.
    for _ in 0..8 {
        if rng.fill(&mut buf).is_ok() {
            return buf;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    // Last resort, still OS randomness (getrandom through uuid).
    buf.iter_mut()
        .zip(std::iter::repeat_with(|| *uuid::Uuid::new_v4().as_bytes()).flatten())
        .for_each(|(b, r)| *b = r);
    buf
}

pub fn to_hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{:02x}", b)).collect()
}

/// `count` characters from `alphabet`, uniformly: bytes at or above the
/// largest multiple of the alphabet size are thrown away (no modulo bias).
fn random_chars(alphabet: &[u8], count: usize) -> Vec<char> {
    let limit = 256 - 256 % alphabet.len();
    let mut out = Vec::with_capacity(count);
    while out.len() < count {
        for b in random_bytes(count * 2) {
            if (b as usize) < limit && out.len() < count {
                out.push(alphabet[b as usize % alphabet.len()] as char);
            }
        }
    }
    out
}

/// A new access password like "k7wq-3mzp-x9tr" (unambiguous lowercase letters
/// and digits, ~59 bits of entropy).
pub fn generate_direct_password() -> String {
    const ALPHABET: &[u8] = b"abcdefghjkmnpqrstuvwxyz23456789";
    let chars = random_chars(ALPHABET, 12);
    format!(
        "{}-{}-{}",
        chars[0..4].iter().collect::<String>(),
        chars[4..8].iter().collect::<String>(),
        chars[8..12].iter().collect::<String>()
    )
}

/// Gives the config a direct-access password when it has none; returns
/// whether it changed (so the caller can save it).
pub fn ensure_direct_password(config: &mut AgentConfig) -> bool {
    if config.direct_password.trim().is_empty() {
        config.direct_password = generate_direct_password();
        true
    } else {
        false
    }
}


/// Gives the config its own key for the server when it has none (or a
/// malformed one); returns whether it changed.
pub fn ensure_agent_key(config: &mut AgentConfig) -> bool {
    let key = config.agent_key.trim();
    if key.len() == 64 && key.bytes().all(|b| b.is_ascii_hexdigit()) {
        return false;
    }
    config.agent_key = to_hex(&random_bytes(32));
    true
}

static CONFIG_PATH: std::sync::OnceLock<String> = std::sync::OnceLock::new();

/// Sets the config file path for this process (from `--config`); call once at startup.
/// Gives `config` an id when it has none (a config written by hand or by an
/// older tool): an empty id is refused by servers and links. `true` when one
/// was made (the caller saves it, so the id stays the same next time).
pub fn ensure_client_id(config: &mut AgentConfig) -> bool {
    if !config.client_unique_id.trim().is_empty() {
        return false;
    }
    config.client_unique_id = uuid::Uuid::new_v4().to_string().to_uppercase();
    true
}

/// A fresh configuration for this computer: a new id, no server and a new
/// direct-access password. Direct connections stay off until the user turns
/// remote control on.
pub fn new_config() -> AgentConfig {
    let mut config: AgentConfig = serde_json::from_str("{}").expect("every field has a default");
    config.client_unique_id = uuid::Uuid::new_v4().to_string().to_uppercase();
    config.server_url = String::new();
    ensure_direct_password(&mut config);
    config
}

pub fn set_config_path(path: Option<String>) {
    let _ = CONFIG_PATH.set(path.unwrap_or_else(default_config_path));
}

/// Path of agent_config.json used by both the CLI and the GUI.
pub fn config_path() -> &'static str {
    CONFIG_PATH.get_or_init(default_config_path)
}

/// `agent_config.json` in the working directory when one exists there (dev
/// checkouts, older setups) and may be trusted (`working_dir_config_usable`);
/// otherwise the per-user config directory (`%APPDATA%\lunaris-agent`,
/// `~/.config/lunaris-agent`, ...), since the install location (e.g.
/// Program Files) is not writable. The services always pass `--config`.
fn default_config_path() -> String {
    const NAME: &str = "agent_config.json";
    let here = std::path::Path::new(NAME);
    if here.exists() {
        let per_user = dirs::config_dir().map(|d| d.join("lunaris-agent"));
        match working_dir_config_usable(here, per_user.as_deref()) {
            Ok(()) => return NAME.to_string(),
            // Before logging starts: on standard error. The file is left alone.
            Err(why) => eprintln!(
                "WARN: ignoring {} in the current folder ({}); pass --config {} to use it",
                NAME,
                why,
                std::env::current_dir().map(|d| d.join(NAME).display().to_string()).unwrap_or_else(|_| NAME.to_string())
            ),
        }
    }
    match dirs::config_dir() {
        Some(dir) => {
            let dir = dir.join("lunaris-agent");
            let _ = fs::create_dir_all(&dir);
            dir.join(NAME).to_string_lossy().into_owned()
        }
        None => NAME.to_string(),
    }
}

/// Whether `agent_config.json` found in the working directory may be used
/// without `--config`: someone else's file there would choose the server,
/// the password and the sources of this agent. Unix: a regular file (not a
/// link) owned by this user that nobody else may write. Windows: only in
/// the per-user folder itself (`per_user_dir`), whose permissions are the
/// user's; elsewhere `--config` names it.
fn working_dir_config_usable(path: &std::path::Path, per_user_dir: Option<&std::path::Path>) -> Result<(), String> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let _ = per_user_dir;
        let meta = fs::symlink_metadata(path).map_err(|e| e.to_string())?;
        if !meta.file_type().is_file() {
            return Err("not a regular file".into());
        }
        if meta.uid() != unsafe { libc::geteuid() } {
            return Err("owned by another user".into());
        }
        if meta.mode() & 0o022 != 0 {
            return Err("other users may write it".into());
        }
        Ok(())
    }
    #[cfg(windows)]
    {
        let folder = std::fs::canonicalize(path).ok().and_then(|p| p.parent().map(|d| d.to_path_buf()));
        let per_user = per_user_dir.and_then(|d| std::fs::canonicalize(d).ok());
        match (folder, per_user) {
            (Some(folder), Some(per_user)) if folder == per_user => Ok(()),
            _ => Err("not in this user's config folder".into()),
        }
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = (path, per_user_dir);
        Ok(())
    }
}

pub fn load_config(path: &str) -> Result<AgentConfig, anyhow::Error> {
    let content = fs::read_to_string(path)?;
    let config: AgentConfig = serde_json::from_str(&content)?;
    Ok(config)
}

/// Saves the config. It holds the access password, the server token and
/// the agent key, so on unix it is created owner-only (0600) from the start,
/// written to a new file next to it and renamed over the old one; on Windows
/// it gets the folder's permissions (win_service.rs locks the service's).
pub fn save_config(config: &AgentConfig, path: &str) -> Result<(), anyhow::Error> {
    let content = serde_json::to_string_pretty(config)?;
    write_private(std::path::Path::new(path), content.as_bytes())?;
    Ok(())
}

/// Writes `data` to `path` owner-only (unix 0600), through a new file
/// renamed over the old one. Run as root on a file of another user (the
/// system service's config, `--system`) the file keeps that owner, or gets
/// the folder's owner when it is new, so the agent running as that user can
/// still read it.
pub(crate) fn write_private(path: &std::path::Path, data: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    let dir = match path.parent() {
        Some(d) if !d.as_os_str().is_empty() => d,
        _ => std::path::Path::new("."),
    };
    let name = path
        .file_name()
        .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::InvalidInput, "no file name"))?;
    let tmp = dir.join(format!(".{}.{}.tmp", name.to_string_lossy(), to_hex(&random_bytes(6))));
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let result = (|| {
        let mut file = options.open(&tmp)?;
        #[cfg(unix)]
        keep_owner(&file, path, dir)?;
        file.write_all(data)?;
        file.sync_all()?;
        drop(file);
        fs::rename(&tmp, path)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    result
}

/// See [`write_private`]: only root can give a file away.
#[cfg(unix)]
fn keep_owner(file: &fs::File, path: &std::path::Path, dir: &std::path::Path) -> std::io::Result<()> {
    use std::os::unix::fs::MetadataExt;
    use std::os::unix::io::AsRawFd;
    if unsafe { libc::geteuid() } != 0 {
        return Ok(());
    }
    // Never through a link: one planted by the folder's owner would hand
    // the file to whoever owns the link's target.
    let own = match fs::symlink_metadata(path) {
        Ok(m) if !m.file_type().is_symlink() => Ok(m),
        _ => fs::symlink_metadata(dir),
    };
    let Ok(meta) = own else {
        return Ok(());
    };
    if meta.uid() == 0 {
        return Ok(());
    }
    if unsafe { libc::fchown(file.as_raw_fd(), meta.uid(), meta.gid()) } != 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(())
}

/// Changes the config file on disk: read again first, so what the app or
/// another process saved meanwhile is kept. `current` is used when there is
/// no file yet.
pub fn update_config_file(
    path: &str,
    current: &AgentConfig,
    change: impl FnOnce(&mut AgentConfig),
) -> Result<(), anyhow::Error> {
    let mut config = if std::path::Path::new(path).exists() {
        load_config(path)?
    } else {
        current.clone()
    };
    change(&mut config);
    save_config(&config, path)
}

pub fn import_config_file(imported_path: &str, target_path: &str) -> Result<(), anyhow::Error> {
    let content = fs::read_to_string(imported_path)?;
    let mut imported: AgentConfig = serde_json::from_str(&content)?;

    // Load existing config if available to preserve keys/certificates
    if let Ok(ext) = load_config(target_path) {
        if imported.client_unique_id.is_empty() {
            imported.client_unique_id = ext.client_unique_id;
        }
        if imported.client_private_key.is_empty() {
            imported.client_private_key = ext.client_private_key;
        }
        if imported.client_certificate.is_empty() {
            imported.client_certificate = ext.client_certificate;
        }
        if imported.server_certificate.is_empty() {
            imported.server_certificate = ext.server_certificate;
        }
    }

    save_config(&imported, target_path)?;
    Ok(())
}



/// macOS: starts the agent (`--cli`) at login with the default config
/// (`--enroll`), as a LaunchAgent.
#[cfg(target_os = "macos")]
pub fn enable_macos_login_start() {
    let exe = match std::env::current_exe() {
        Ok(path) => path.to_string_lossy().into_owned(),
        Err(_) => return,
    };
    let Some(home) = std::env::var_os("HOME") else { return };
    let dir = std::path::Path::new(&home).join("Library").join("LaunchAgents");
    let _ = std::fs::create_dir_all(&dir);
    let content = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>com.lunaris.agent</string>
    <key>ProgramArguments</key>
    <array>
        <string>{}</string>
        <string>--cli</string>
    </array>
    <key>RunAtLoad</key>
    <true/>
</dict>
</plist>"#,
        exe
    );
    let _ = std::fs::write(dir.join("com.lunaris.agent.plist"), content);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn passwords_use_the_alphabet_and_differ() {
        let a = generate_direct_password();
        let b = generate_direct_password();
        assert_ne!(a, b);
        assert_eq!(a.len(), 14);
        let parts: Vec<&str> = a.split('-').collect();
        assert_eq!(parts.len(), 3);
        assert!(parts.iter().all(|p| p.len() == 4));
        assert!(a.bytes().filter(|&c| c != b'-').all(|c| b"abcdefghjkmnpqrstuvwxyz23456789".contains(&c)));
    }

    #[test]
    fn agent_key_is_64_hex_and_kept() {
        let mut c = AgentConfig::default();
        assert!(ensure_agent_key(&mut c));
        assert_eq!(c.agent_key.len(), 64);
        assert!(c.agent_key.bytes().all(|b| b.is_ascii_hexdigit()));
        let key = c.agent_key.clone();
        assert!(!ensure_agent_key(&mut c));
        assert_eq!(c.agent_key, key);
        c.agent_key = "short".into();
        assert!(ensure_agent_key(&mut c));
    }

    #[test]
    fn install_token_and_name_stay_out_of_the_file_when_empty() {
        let c = new_config();
        let json = serde_json::to_string(&c).unwrap();
        assert!(!json.contains("install_token") && !json.contains("\"name\""));
        let mut c = c;
        c.install_token = "lit_x".into();
        let back: AgentConfig = serde_json::from_str(&serde_json::to_string(&c).unwrap()).unwrap();
        assert_eq!(back.install_token, "lit_x");
    }

    #[test]
    fn defaults_for_new_fields() {
        let c: AgentConfig = serde_json::from_str(r#"{"server_url":"x"}"#).unwrap();
        assert_eq!(c.server_sessions, "trusted");
        assert_eq!(c.file_access, "");
        assert!(c.server_cert_sha256.is_empty() && c.agent_key.is_empty());
        assert_eq!(c.wayland_capture, "auto");
        assert_eq!(AgentConfig::default().wayland_capture, "auto");
        assert_eq!(new_config().wayland_capture, "auto");
    }

    #[test]
    fn legacy_direct_is_on_unless_turned_off() {
        let c: AgentConfig = serde_json::from_str(r#"{"direct_enabled":true}"#).unwrap();
        assert!(c.allow_legacy_plain_direct);
        let c: AgentConfig = serde_json::from_str(r#"{"allow_legacy_plain_direct":false}"#).unwrap();
        assert!(!c.allow_legacy_plain_direct);
        let back: AgentConfig = serde_json::from_str(&serde_json::to_string(&c).unwrap()).unwrap();
        assert!(!back.allow_legacy_plain_direct);
    }

    #[cfg(unix)]
    #[test]
    fn a_config_in_the_working_folder_is_used_only_when_it_is_ours_alone() {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("lunaris-cwd-config-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("agent_config.json");
        fs::write(&path, "{}").unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
        assert!(working_dir_config_usable(&path, None).is_ok());
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
        assert!(working_dir_config_usable(&path, None).is_ok(), "readable by others is the user's choice");
        fs::set_permissions(&path, fs::Permissions::from_mode(0o666)).unwrap();
        assert!(working_dir_config_usable(&path, None).is_err());
        fs::set_permissions(&path, fs::Permissions::from_mode(0o620)).unwrap();
        assert!(working_dir_config_usable(&path, None).is_err());
        // A link is not followed, wherever it leads.
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
        let link = dir.join("linked.json");
        std::os::unix::fs::symlink(&path, &link).unwrap();
        assert!(working_dir_config_usable(&link, None).is_err());
        // Owned by someone else (root's file, when not running as root).
        if unsafe { libc::geteuid() } != 0 && std::path::Path::new("/etc/hostname").exists() {
            assert!(working_dir_config_usable(std::path::Path::new("/etc/hostname"), None).is_err());
        }
        // The file itself is never touched.
        assert_eq!(fs::read_to_string(&path).unwrap(), "{}");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn wayland_capture_round_trips() {
        let mut c = AgentConfig::default();
        c.wayland_capture = "portal".into();
        let back: AgentConfig = serde_json::from_str(&serde_json::to_string(&c).unwrap()).unwrap();
        assert_eq!(back.wayland_capture, "portal");
        use lunaris_media::capture::WaylandCapturePreference as P;
        assert_eq!(wayland_capture_preference(&back.wayland_capture), (P::Portal, true));
        assert_eq!(wayland_capture_preference("auto"), (P::Auto, true));
    }

    #[test]
    fn missing_id_is_made_once() {
        let mut c: AgentConfig = serde_json::from_str("{}").unwrap();
        assert!(ensure_client_id(&mut c));
        let id = c.client_unique_id.clone();
        assert_eq!(id.len(), 36);
        assert!(!ensure_client_id(&mut c));
        assert_eq!(c.client_unique_id, id);
    }

    #[test]
    fn unknown_wayland_capture_is_auto() {
        use lunaris_media::capture::WaylandCapturePreference as P;
        let c: AgentConfig = serde_json::from_str(r#"{"wayland_capture":"service"}"#).unwrap();
        // Kept in the file as written, used as auto.
        assert_eq!(c.wayland_capture, "service");
        assert_eq!(wayland_capture_preference(&c.wayland_capture), (P::Auto, false));
        assert_eq!(wayland_capture_preference(""), (P::Auto, false));
    }

    #[cfg(unix)]
    #[test]
    fn config_is_saved_owner_only() {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("lunaris-pairing-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("agent_config.json");
        fs::write(&path, "{}").unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
        let mut c = AgentConfig::default();
        c.direct_password = "secret".into();
        save_config(&c, path.to_str().unwrap()).unwrap();
        let mode = fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
        assert_eq!(load_config(path.to_str().unwrap()).unwrap().direct_password, "secret");
        // No temporary file left behind.
        assert_eq!(fs::read_dir(&dir).unwrap().count(), 1);
        update_config_file(path.to_str().unwrap(), &c, |c| c.agent_key = "k".into()).unwrap();
        let back = load_config(path.to_str().unwrap()).unwrap();
        assert_eq!((back.agent_key.as_str(), back.direct_password.as_str()), ("k", "secret"));
        let _ = fs::remove_dir_all(&dir);
    }
}
