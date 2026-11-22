//! Linux: how the agent gets started, set up by `--enroll`.
//!
//! - A desktop computer (`--enroll` as the desktop user): the systemd *user*
//!   service `lunaris-agent.service`, started with the graphical session
//!   (`graphical-session.target`, which GNOME, KDE Plasma and other systemd
//!   sessions start once their display variables are in the user manager),
//!   so the agent captures that session. Enabled for this user only, as
//!   `systemctl --user enable` does (a link in
//!   ~/.config/systemd/user/graphical-session.target.wants), never for every
//!   user: the package ships the unit, not a preset. The packaged unit
//!   (/usr/lib/systemd/user) is used for /usr/bin/lunaris-agent with the
//!   default config; otherwise (AppImage, another --config) a unit of the
//!   user's own in ~/.config/systemd/user. Desktops that start no
//!   graphical-session.target (Xfce, MATE, ...) run the login autostart
//!   entry ~/.config/autostart/lunaris-agent.desktop instead: it carries
//!   `X-GNOME-HiddenUnderSystemd=true`, which systemd's autostart generator
//!   (and gnome-session) skip, so one agent runs either way. Without
//!   systemd only that entry is written.
//! - A headless computer (`--enroll --system`, as root): the *system*
//!   service `lunaris-agent-system.service`, user `lunaris-agent`, data in
//!   /var/lib/lunaris-agent.

use std::path::{Path, PathBuf};
use std::process::Command;

const USER_UNIT: &str = "lunaris-agent.service";
const SYSTEM_UNIT: &str = "lunaris-agent-system.service";
const SYSTEM_USER: &str = "lunaris-agent";
/// The packaged program, which the packaged user unit starts.
const PACKAGED_EXE: &str = "/usr/bin/lunaris-agent";

fn systemd_runs() -> bool {
    Path::new("/run/systemd/system").is_dir()
}

fn packaged_user_unit() -> Option<PathBuf> {
    ["/usr/lib/systemd/user", "/lib/systemd/user"]
        .iter()
        .map(|d| Path::new(d).join(USER_UNIT))
        .find(|p| p.is_file())
}

fn canonical(p: &str) -> String {
    std::fs::canonicalize(p).map(|p| p.to_string_lossy().into_owned()).unwrap_or_else(|_| p.to_string())
}

/// The program to start: the AppImage file itself (current_exe points into
/// its temporary mount), else this executable.
fn this_exe() -> std::io::Result<String> {
    match std::env::var("APPIMAGE").ok().filter(|p| !p.is_empty()) {
        Some(appimage) => Ok(appimage),
        None => Ok(std::env::current_exe()?.to_string_lossy().into_owned()),
    }
}

fn home() -> std::io::Result<PathBuf> {
    std::env::var_os("HOME")
        .filter(|h| !h.is_empty())
        .map(PathBuf::from)
        .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::NotFound, "HOME is not set"))
}

/// ~/.config (XDG_CONFIG_HOME).
fn config_home() -> std::io::Result<PathBuf> {
    match std::env::var_os("XDG_CONFIG_HOME").filter(|d| !d.is_empty()) {
        Some(d) => Ok(PathBuf::from(d)),
        None => Ok(home()?.join(".config")),
    }
}

/// `--enroll` on a desktop: see the module. Returns what was done, for the
/// person.
pub fn enable_user_start(config: &str) -> Result<String, String> {
    let exe = this_exe().map_err(|e| e.to_string())?;
    let config = canonical(config);
    let default_config = config_home()
        .map(|d| canonical(&d.join("lunaris-agent").join("agent_config.json").to_string_lossy()))
        .ok();
    let config_home = config_home().map_err(|e| e.to_string())?;
    let is_default = default_config.as_deref() == Some(config.as_str());

    // The login entry: the fallback for desktops without
    // graphical-session.target (and the only start without systemd).
    let autostart_dir = config_home.join("autostart");
    std::fs::create_dir_all(&autostart_dir).map_err(|e| format!("{}: {e}", autostart_dir.display()))?;
    let entry = autostart_dir.join("lunaris-agent.desktop");
    let systemd = systemd_runs();
    std::fs::write(&entry, autostart_entry(&exe, &config, systemd)).map_err(|e| format!("{}: {e}", entry.display()))?;
    // An entry an earlier --enroll wrote beside the agent window's.
    let old = autostart_dir.join("lunaris-agent-cli.desktop");
    if std::fs::read_to_string(&old).is_ok_and(|e| e.contains("--cli --config")) {
        let _ = std::fs::remove_file(&old);
    }
    if !systemd {
        return Ok(format!("The agent starts when you log in to the desktop ({}).", entry.display()));
    }

    // The systemd user service.
    let unit_dir = config_home.join("systemd").join("user");
    let own_unit = unit_dir.join(USER_UNIT);
    let packaged = packaged_user_unit().filter(|_| is_default && canonical(&exe) == PACKAGED_EXE);
    let unit = match packaged {
        Some(unit) => {
            // A unit of our own from an earlier enroll would hide the packaged one.
            if std::fs::read_to_string(&own_unit).is_ok_and(|u| u.contains(OWN_UNIT_MARK)) {
                let _ = std::fs::remove_file(&own_unit);
            }
            unit
        }
        None => {
            std::fs::create_dir_all(&unit_dir).map_err(|e| format!("{}: {e}", unit_dir.display()))?;
            std::fs::write(&own_unit, user_unit(&exe, &config)).map_err(|e| format!("{}: {e}", own_unit.display()))?;
            own_unit.clone()
        }
    };
    let wants = unit_dir.join("graphical-session.target.wants");
    std::fs::create_dir_all(&wants).map_err(|e| format!("{}: {e}", wants.display()))?;
    let link = wants.join(USER_UNIT);
    let _ = std::fs::remove_file(&link);
    std::os::unix::fs::symlink(&unit, &link).map_err(|e| format!("{}: {e}", link.display()))?;

    let mut note = format!(
        "The agent starts with your desktop session: systemd user service {USER_UNIT} ({}), or {} on desktops without systemd sessions.",
        unit.display(),
        entry.display()
    );
    match start_user_unit_now() {
        Some(true) => note.push_str("\nStarted it now in your desktop session (systemctl --user status lunaris-agent)."),
        Some(false) => note.push_str("\nIt starts at your next desktop login (or now, in the desktop: systemctl --user start lunaris-agent)."),
        None => {}
    }
    Ok(note)
}

/// When this user's systemd manager runs (an open session): reloads it and,
/// when a graphical session is active, (re)starts the agent with the new
/// config. Some(started), None when there is no manager to ask.
fn start_user_unit_now() -> Option<bool> {
    let runtime = std::env::var_os("XDG_RUNTIME_DIR")
        .filter(|d| !d.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(format!("/run/user/{}", unsafe { libc::getuid() })));
    if !runtime.join("systemd").join("private").exists() {
        return None;
    }
    let systemctl = |args: &[&str]| {
        Command::new("systemctl")
            .arg("--user")
            .args(args)
            .env("XDG_RUNTIME_DIR", &runtime)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .is_ok_and(|s| s.success())
    };
    if !systemctl(&["daemon-reload"]) {
        return None;
    }
    if systemctl(&["is-active", "--quiet", "graphical-session.target"]) {
        Some(systemctl(&["restart", USER_UNIT]))
    } else {
        Some(false)
    }
}

/// In a unit `--enroll` wrote (not the packaged one).
const OWN_UNIT_MARK: &str = "# Written by lunaris-agent --enroll";

/// A word of a unit's command line (systemd.service(5) "Command lines"):
/// double quotes, `\` and `"` escaped, `%` and `$` doubled.
fn unit_quote(s: &str) -> String {
    let mut out = String::from("\"");
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '%' => out.push_str("%%"),
            '$' => out.push_str("$$"),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// The user's own unit: as the packaged one (scripts/packaging/linux/
/// lunaris-agent.service), with this program and config.
fn user_unit(exe: &str, config: &str) -> String {
    format!(
        "{OWN_UNIT_MARK} (this program or config is not the packaged one's).\n\
         [Unit]\n\
         Description=Lunaris agent (shares this desktop session)\n\
         PartOf=graphical-session.target\n\
         After=graphical-session.target\n\
         \n\
         [Service]\n\
         Type=simple\n\
         ExecStart={} --cli --config {}\n\
         Restart=always\n\
         RestartSec=5\n\
         \n\
         [Install]\n\
         WantedBy=graphical-session.target\n",
        unit_quote(exe),
        unit_quote(config)
    )
}

/// The login autostart entry: arguments quoted as the Desktop Entry spec
/// says. `hidden_under_systemd`: systemd sessions start the user unit
/// instead.
fn autostart_entry(exe: &str, config: &str, hidden_under_systemd: bool) -> String {
    let quote = |s: &str| {
        let mut out = String::from("\"");
        for c in s.chars() {
            if matches!(c, '"' | '`' | '$' | '\\') {
                out.push('\\');
            }
            out.push(c);
        }
        out.push('"');
        // Then the string escape (a backslash twice), and `%` is a field code.
        out.replace('\\', "\\\\").replace('%', "%%")
    };
    format!(
        "[Desktop Entry]\nType=Application\nName=Lunaris Agent\nExec={} --cli --config {}\nTerminal=false\nNoDisplay=true\nX-GNOME-Autostart-enabled=true\n{}",
        quote(exe),
        quote(config),
        if hidden_under_systemd { "X-GNOME-HiddenUnderSystemd=true\n" } else { "" }
    )
}

/// The `lunaris-agent` user: (uid, gid).
fn system_user() -> Option<(libc::uid_t, libc::gid_t)> {
    let name = std::ffi::CString::new(SYSTEM_USER).ok()?;
    let pw = unsafe { libc::getpwnam(name.as_ptr()) };
    if pw.is_null() {
        return None;
    }
    Some(unsafe { ((*pw).pw_uid, (*pw).pw_gid) })
}

/// `--enroll --system`, before the config is written: /var/lib/lunaris-agent
/// exists, is the `lunaris-agent` user's and closed to others (the config
/// written there by root then gets that owner, pairing.rs write_private).
pub fn prepare_system_dir() -> Result<(), String> {
    let (uid, gid) = system_user().ok_or_else(|| {
        format!(
            "there is no `{SYSTEM_USER}` user for the system service: install the lunaris-agent package, \
             or create it: useradd --system --home-dir {} --shell /usr/sbin/nologin {SYSTEM_USER}",
            crate::SYSTEM_DATA_DIR
        )
    })?;
    let euid = unsafe { libc::geteuid() };
    if euid != 0 && euid != uid {
        return Err("--system: run it as root (sudo lunaris-agent --system ...)".into());
    }
    let dir = Path::new(crate::SYSTEM_DATA_DIR);
    std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    if euid == 0 {
        use std::os::unix::fs::PermissionsExt;
        let c = std::ffi::CString::new(crate::SYSTEM_DATA_DIR).map_err(|e| e.to_string())?;
        if unsafe { libc::chown(c.as_ptr(), uid, gid) } != 0 {
            return Err(format!("{}: {}", dir.display(), std::io::Error::last_os_error()));
        }
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700)).map_err(|e| format!("{}: {e}", dir.display()))?;
    }
    Ok(())
}

/// `--enroll --system`: enables and (re)starts the system service.
pub fn enable_system_service() -> Result<String, String> {
    if !systemd_runs() {
        return Err(format!(
            "systemd is not running: start `lunaris-agent --cli --config {}/agent_config.json` as the {SYSTEM_USER} user with your init system",
            crate::SYSTEM_DATA_DIR
        ));
    }
    if unsafe { libc::geteuid() } != 0 {
        return Err(format!("not root: run `systemctl enable --now {SYSTEM_UNIT}` as root"));
    }
    let run = |args: &[&str]| {
        Command::new("systemctl").args(args).status().map_err(|e| e.to_string()).and_then(|s| {
            if s.success() {
                Ok(())
            } else {
                Err(format!("systemctl {} failed ({s})", args.join(" ")))
            }
        })
    };
    run(&["enable", SYSTEM_UNIT])?;
    run(&["restart", SYSTEM_UNIT])?;
    Ok(format!(
        "The system service {SYSTEM_UNIT} runs the agent now and at every boot (systemctl status {SYSTEM_UNIT}, journalctl -u {SYSTEM_UNIT})."
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn autostart_entry_quotes() {
        let e = autostart_entry("/opt/lunaris agent/x.AppImage", "/home/a/.config/lunaris-agent/agent_config.json", false);
        assert!(e.contains("Exec=\"/opt/lunaris agent/x.AppImage\" --cli --config \"/home/a/.config/lunaris-agent/agent_config.json\"\n"));
        assert!(!e.contains("HiddenUnderSystemd"));
        let e = autostart_entry("/usr/bin/lunaris-agent", "/h/$x\"%", true);
        assert!(e.contains("--config \"/h/\\\\$x\\\\\"%%\""), "{e}");
        assert!(e.ends_with("X-GNOME-HiddenUnderSystemd=true\n"));
    }

    #[test]
    fn user_unit_quotes() {
        let u = user_unit("/opt/my apps/Lunaris.AppImage", "/home/a/50%/$c\"x\\y");
        assert!(u.starts_with(OWN_UNIT_MARK));
        assert!(
            u.contains("ExecStart=\"/opt/my apps/Lunaris.AppImage\" --cli --config \"/home/a/50%%/$$c\\\"x\\\\y\"\n"),
            "{u}"
        );
        assert!(u.contains("WantedBy=graphical-session.target"));
    }
}
