//! What `lunaris-agent status` says about the agent that runs: the running
//! agent keeps a small file next to its config (`agent_status.json`): its
//! process id, the server and whether it is connected, the last error. No
//! secrets are in it. The file is written when something changes and once a
//! minute otherwise, and removed when the agent ends normally; a reader
//! treats it as stale when that process is gone or it was not written for a
//! while (a killed agent cannot remove it).

use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::time::Duration;

use serde::{Deserialize, Serialize};

/// Written at least this often while the agent runs.
const HEARTBEAT: Duration = Duration::from_secs(60);
/// Older than this, the file says nothing any more.
const STALE_AFTER_S: u64 = 180;

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct Snapshot {
    pub pid: u32,
    pub version: String,
    /// Unix seconds.
    pub started: u64,
    pub updated: u64,
    pub server_url: String,
    pub connected: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_error: Option<String>,
}

pub fn path_for(config_path: &str) -> PathBuf {
    let config = Path::new(config_path);
    match config.parent() {
        Some(dir) if !dir.as_os_str().is_empty() => dir.join("agent_status.json"),
        _ => PathBuf::from("agent_status.json"),
    }
}

fn now_s() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

/// The running agent's file; removed on drop.
pub struct RunState {
    path: PathBuf,
    task: tokio::task::JoinHandle<()>,
}

impl RunState {
    /// Starts keeping the file (in the current tokio runtime).
    pub fn start(config_path: &str, server_url: &str) -> Self {
        let path = path_for(config_path);
        let mut snapshot = Snapshot {
            pid: std::process::id(),
            version: env!("CARGO_PKG_VERSION").to_string(),
            started: now_s(),
            updated: 0,
            server_url: server_url.trim().to_string(),
            connected: false,
            last_error: None,
        };
        let file = path.clone();
        let task = tokio::spawn(async move {
            let mut written: Option<(bool, Option<String>)> = None;
            let mut last_write = std::time::Instant::now();
            loop {
                snapshot.connected = crate::CONNECTED_TO_SERVER.load(Ordering::SeqCst);
                snapshot.last_error = crate::LAST_ERROR.lock().ok().and_then(|e| e.clone());
                let now = (snapshot.connected, snapshot.last_error.clone());
                if written.as_ref() != Some(&now) || last_write.elapsed() >= HEARTBEAT {
                    snapshot.updated = now_s();
                    if let Ok(json) = serde_json::to_vec_pretty(&snapshot) {
                        if crate::pairing::write_private(&file, &json).is_ok() {
                            written = Some(now);
                            last_write = std::time::Instant::now();
                        }
                    }
                }
                tokio::time::sleep(Duration::from_secs(2)).await;
            }
        });
        Self { path, task }
    }
}

impl Drop for RunState {
    fn drop(&mut self) {
        self.task.abort();
        let _ = std::fs::remove_file(&self.path);
    }
}

/// What a reader makes of the file.
#[derive(Debug, PartialEq, Eq)]
pub enum Seen {
    /// No file: no agent ran with this config (or it ended normally).
    NotRunning,
    /// A file whose agent is gone or that is too old.
    Stale(Snapshot),
    Running(Snapshot),
}

pub fn read(config_path: &str) -> Seen {
    let Ok(data) = std::fs::read(path_for(config_path)) else {
        return Seen::NotRunning;
    };
    let Ok(snapshot) = serde_json::from_slice::<Snapshot>(&data) else {
        return Seen::NotRunning;
    };
    judge(snapshot, now_s(), process_alive)
}

fn judge(snapshot: Snapshot, now: u64, alive: impl Fn(u32) -> Option<bool>) -> Seen {
    if now.saturating_sub(snapshot.updated) > STALE_AFTER_S || alive(snapshot.pid) == Some(false) {
        Seen::Stale(snapshot)
    } else {
        Seen::Running(snapshot)
    }
}

/// Whether process `pid` exists; None where that can't be told.
fn process_alive(pid: u32) -> Option<bool> {
    #[cfg(unix)]
    {
        let Ok(pid) = libc::pid_t::try_from(pid) else { return Some(false) };
        // Signal 0: only the check. EPERM: it exists, as another user.
        let r = unsafe { libc::kill(pid, 0) };
        Some(r == 0 || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM))
    }
    #[cfg(not(unix))]
    {
        let _ = pid;
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snap(updated: u64) -> Snapshot {
        Snapshot {
            pid: 42,
            version: "1".into(),
            started: 1,
            updated,
            server_url: "wss://x".into(),
            connected: true,
            last_error: None,
        }
    }

    #[test]
    fn stale_when_old_or_gone() {
        assert!(matches!(judge(snap(1000), 1010, |_| Some(true)), Seen::Running(_)));
        assert!(matches!(judge(snap(1000), 1010, |_| None), Seen::Running(_)));
        assert!(matches!(judge(snap(1000), 1010, |_| Some(false)), Seen::Stale(_)));
        assert!(matches!(judge(snap(1000), 1000 + STALE_AFTER_S + 1, |_| Some(true)), Seen::Stale(_)));
    }

    #[test]
    fn file_sits_next_to_the_config() {
        assert_eq!(path_for("/var/lib/lunaris-agent/agent_config.json"), PathBuf::from("/var/lib/lunaris-agent/agent_status.json"));
        assert_eq!(path_for("agent_config.json"), PathBuf::from("agent_status.json"));
    }

    #[tokio::test]
    async fn running_agent_writes_and_removes_it() {
        let dir = std::env::temp_dir().join(format!("lunaris-status-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let config = dir.join("agent_config.json").to_string_lossy().into_owned();
        let state = RunState::start(&config, " wss://srv:8443 ");
        let mut seen = Seen::NotRunning;
        for _ in 0..50 {
            tokio::time::sleep(Duration::from_millis(20)).await;
            seen = read(&config);
            if matches!(seen, Seen::Running(_)) {
                break;
            }
        }
        match seen {
            Seen::Running(s) => {
                assert_eq!(s.pid, std::process::id());
                assert_eq!(s.server_url, "wss://srv:8443");
            }
            other => panic!("{other:?}"),
        }
        drop(state);
        assert_eq!(read(&config), Seen::NotRunning);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
