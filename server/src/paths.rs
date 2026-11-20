//! Where the server keeps its state (database, agent token, keys, the
//! first-start setup code).
//!
//! `LUNARIS_DATA_DIR` selects the directory; the installed service sets it
//! (`/var/lib/lunaris` on Linux, `%ProgramData%\Lunaris\Server` on Windows). Without it the
//! server keeps the historical behaviour of using the working directory.

use std::path::{Path, PathBuf};

/// Every file the server keeps in its data directory, by the names the code
/// uses: lib.rs (lunaris.db, server_token.txt), auth.rs (jwt_secret.txt),
/// mfa.rs (mfa_key.txt), tls.rs (tls_cert.pem, tls_key.pem), setup.rs
/// (setup_code.txt), the Windows service (server.env, server.log) and older
/// versions (initial_admin_password.txt). SQLite's own files go with the
/// database. Moving the Windows data (service.rs) takes these and nothing else.
pub const SERVER_FILES: &[&str] = &[
    "lunaris.db",
    "lunaris.db-wal",
    "lunaris.db-shm",
    "lunaris.db-journal",
    "server_token.txt",
    "jwt_secret.txt",
    "mfa_key.txt",
    "tls_cert.pem",
    "tls_key.pem",
    "setup_code.txt",
    "initial_admin_password.txt",
    "server.env",
    "server.log",
];

/// The configured data directory, created on first use.
pub fn data_dir() -> Option<PathBuf> {
    let dir = std::env::var_os("LUNARIS_DATA_DIR").filter(|v| !v.is_empty())?;
    let dir = PathBuf::from(dir);
    if let Err(e) = std::fs::create_dir_all(&dir) {
        tracing::error!("Cannot create data directory {:?}: {}", dir, e);
    }
    Some(dir)
}

/// `name` inside the data directory, or relative to the working directory.
pub fn data_file(name: &str) -> PathBuf {
    data_dir().map(|d| d.join(name)).unwrap_or_else(|| PathBuf::from(name))
}

/// Writes a file only the owner can read (secrets: token, keys, setup code).
/// On Windows: only SYSTEM, Administrators and the file's owner (win_acl.rs),
/// written to a new file that then replaces `path`. On Unix never through a
/// link at `path` (`O_NOFOLLOW`: one planted in a data folder another
/// account owns can't send root's write elsewhere).
pub fn write_private(path: &std::path::Path, contents: &str) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW)
            .open(path)?;
        // `mode` applies to a new file only: tighten one that was there.
        use std::os::unix::fs::PermissionsExt;
        f.set_permissions(std::fs::Permissions::from_mode(0o600))?;
        f.write_all(contents.as_bytes())
    }
    #[cfg(windows)]
    {
        crate::win_acl::write_private(path, contents.as_bytes())
    }
    #[cfg(not(any(unix, windows)))]
    {
        std::fs::write(path, contents)
    }
}

/// Moves the server's files (`SERVER_FILES`) from `old` to `new` when `new`
/// has no database yet and `old` has one: the Windows service's data moved
/// from `%ProgramData%\Lunaris` (shared with the agent) to its own folder.
/// Nothing else in `old` is touched; a file already in `new`, a link, or a
/// file `trusted` refuses (someone else's) stays where it is. Returns the
/// names moved.
#[cfg_attr(not(windows), allow(dead_code))]
pub fn move_server_files(old: &Path, new: &Path, trusted: impl Fn(&Path) -> bool) -> Vec<String> {
    let mut moved = Vec::new();
    if new.join("lunaris.db").exists() || !is_plain_file(&old.join("lunaris.db")) {
        return moved;
    }
    for name in SERVER_FILES {
        let from = old.join(name);
        let to = new.join(name);
        if !is_plain_file(&from) || to.exists() {
            continue;
        }
        if !trusted(&from) {
            tracing::warn!("Not moving {:?}: it is not owned by SYSTEM or Administrators", from);
            continue;
        }
        let done = std::fs::rename(&from, &to).is_ok()
            || (std::fs::copy(&from, &to).is_ok() && std::fs::remove_file(&from).is_ok());
        if done {
            moved.push(name.to_string());
        } else {
            tracing::warn!("Could not move {:?} to {:?}", from, to);
        }
    }
    moved
}

/// A regular file, not a link (nor a folder).
fn is_plain_file(path: &Path) -> bool {
    std::fs::symlink_metadata(path).map(|m| m.file_type().is_file()).unwrap_or(false)
}

/// Loads `KEY=value` lines into the environment without overriding variables
/// that are already set. Used for the Windows service's `server.env`
/// (systemd reads the Linux one itself through `EnvironmentFile=`).
#[cfg_attr(not(windows), allow(dead_code))]
pub fn load_env_file(path: &std::path::Path) {
    let Ok(text) = std::fs::read_to_string(path) else {
        return;
    };
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some((key, value)) = line.split_once('=') {
            let key = key.trim();
            let value = value.trim().trim_matches('"');
            if !key.is_empty() && std::env::var_os(key).is_none() {
                std::env::set_var(key, value);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("lunaris-paths-{}-{}", name, crate::security::random_hex(6)));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[cfg(unix)]
    #[test]
    fn write_private_never_writes_through_a_link() {
        let dir = temp("link");
        std::fs::write(dir.join("victim"), "untouched").unwrap();
        std::os::unix::fs::symlink(dir.join("victim"), dir.join("secret.txt")).unwrap();
        assert!(write_private(&dir.join("secret.txt"), "secret").is_err());
        assert_eq!(std::fs::read_to_string(dir.join("victim")).unwrap(), "untouched");
        // A plain file is still written (and made owner-only).
        write_private(&dir.join("plain.txt"), "a").unwrap();
        write_private(&dir.join("plain.txt"), "b").unwrap();
        assert_eq!(std::fs::read_to_string(dir.join("plain.txt")).unwrap(), "b");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn moves_only_the_servers_files() {
        let old = temp("old");
        let new = old.join("Server");
        std::fs::create_dir_all(&new).unwrap();
        for f in ["lunaris.db", "server_token.txt", "jwt_secret.txt", "tls_key.pem", "server.env"] {
            std::fs::write(old.join(f), f).unwrap();
        }
        // The agent's files stay.
        for f in ["agent_config.json", "agent-service.log", "bridge_config.json"] {
            std::fs::write(old.join(f), "agent").unwrap();
        }
        // A file already in the new folder is kept, not overwritten.
        std::fs::write(new.join("server.env"), "new").unwrap();
        let mut moved = move_server_files(&old, &new, |_| true);
        moved.sort();
        assert_eq!(moved, ["jwt_secret.txt", "lunaris.db", "server_token.txt", "tls_key.pem"]);
        assert_eq!(std::fs::read_to_string(new.join("lunaris.db")).unwrap(), "lunaris.db");
        assert_eq!(std::fs::read_to_string(new.join("server.env")).unwrap(), "new");
        assert!(old.join("server.env").exists());
        for f in ["agent_config.json", "agent-service.log", "bridge_config.json"] {
            assert!(old.join(f).exists());
            assert!(!new.join(f).exists());
        }
        let _ = std::fs::remove_dir_all(&old);
    }

    #[test]
    fn no_move_once_the_new_folder_has_a_database_or_without_an_old_one() {
        let old = temp("old2");
        let new = temp("new2");
        std::fs::write(old.join("server_token.txt"), "x").unwrap();
        assert!(move_server_files(&old, &new, |_| true).is_empty(), "no old database");
        std::fs::write(old.join("lunaris.db"), "old").unwrap();
        std::fs::write(new.join("lunaris.db"), "new").unwrap();
        assert!(move_server_files(&old, &new, |_| true).is_empty(), "new database exists");
        assert_eq!(std::fs::read_to_string(new.join("lunaris.db")).unwrap(), "new");
        assert!(old.join("server_token.txt").exists());
        let _ = std::fs::remove_dir_all(&old);
        let _ = std::fs::remove_dir_all(&new);
    }

    #[test]
    fn untrusted_files_and_links_stay() {
        let old = temp("old3");
        let new = temp("new3");
        std::fs::write(old.join("lunaris.db"), "db").unwrap();
        std::fs::write(old.join("jwt_secret.txt"), "planted").unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink("/etc/hostname", old.join("server_token.txt")).unwrap();
        let moved = move_server_files(&old, &new, |p| !p.ends_with("jwt_secret.txt"));
        assert_eq!(moved, ["lunaris.db"]);
        assert!(old.join("jwt_secret.txt").exists());
        assert!(!new.join("server_token.txt").exists());
        let _ = std::fs::remove_dir_all(&old);
        let _ = std::fs::remove_dir_all(&new);
    }
}
