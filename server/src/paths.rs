//! Where the server keeps its state (database, agent token, initial admin password).
//!
//! `LUNARIS_DATA_DIR` selects the directory; the installed service sets it
//! (`/var/lib/lunaris` on Linux, `%ProgramData%\Lunaris` on Windows). Without it the
//! server keeps the historical behaviour of using the working directory.

use std::path::PathBuf;

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

/// Writes a file only the owner can read (secrets: token, admin password).
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
            .open(path)?;
        f.write_all(contents.as_bytes())
    }
    #[cfg(not(unix))]
    {
        std::fs::write(path, contents)
    }
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
