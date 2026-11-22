//! One agent per config file. Two agents with the same config are the same
//! computer to the server: each connection pushes the other's off, every
//! few seconds, and nobody can connect (KDE Plasma ran the login entry of
//! autostart.rs beside the systemd service). The second one waits here
//! until the first stops, then starts.

/// Holds `<config>.lock` for the life of the process (unix: flock, released
/// by the kernel when the process ends; not inherited across exec, the
/// descriptor is close-on-exec). Waits while another agent holds it.
#[cfg(unix)]
pub async fn hold(config_path: &str) {
    use std::os::unix::io::AsRawFd;
    let path = format!("{config_path}.lock");
    if let Some(dir) = std::path::Path::new(&path).parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let file = match std::fs::OpenOptions::new().create(true).truncate(false).write(true).open(&path) {
        Ok(f) => f,
        Err(e) => {
            tracing::warn!("Cannot open {}: {} (a second agent with this config is not kept out)", path, e);
            return;
        }
    };
    if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
        tracing::warn!(
            "Another Lunaris agent runs with {}: waiting until it stops (two would push each other off the server)",
            config_path
        );
        let file = match tokio::task::spawn_blocking(move || {
            let held = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX) } == 0;
            (file, held)
        })
        .await
        {
            Ok((file, true)) => file,
            _ => return,
        };
        tracing::info!("The other agent stopped: starting");
        std::mem::forget(file);
        return;
    }
    std::mem::forget(file);
}

#[cfg(not(unix))]
pub async fn hold(_config_path: &str) {}

#[cfg(all(test, unix))]
mod tests {
    #[tokio::test]
    async fn a_second_agent_waits_for_the_first() {
        use std::os::unix::io::AsRawFd;
        let dir = std::env::temp_dir().join(format!("lunaris-single-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let config = dir.join("agent_config.json").to_string_lossy().into_owned();
        // The first agent (another open file description, as another process has).
        let first = std::fs::OpenOptions::new().create(true).truncate(false).write(true).open(format!("{config}.lock")).unwrap();
        assert_eq!(unsafe { libc::flock(first.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) }, 0);
        let second = tokio::spawn({
            let config = config.clone();
            async move { super::hold(&config).await }
        });
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
        assert!(!second.is_finished(), "the second must wait");
        drop(first);
        tokio::time::timeout(std::time::Duration::from_secs(5), second).await.expect("starts once the first stops").unwrap();
        let _ = std::fs::remove_dir_all(&dir);
    }
}
