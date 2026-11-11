//! Clipboard and file sharing with the viewing client, over two reliable,
//! ordered data channels the agent opens with each session:
//!
//! - `clipboard`: text both ways, `{"type":"clipboard","text":"..."}`. A
//!   thread owns this computer's clipboard for the session (on X11 the owner
//!   must stay alive for others to paste), sets what the client sends and
//!   reports local changes (polled), never echoing back what it just set.
//! - `file`: files from the client, saved in Downloads/Lunaris:
//!   `{"type":"file_start","id","name","size"}`, binary chunks, then
//!   `{"type":"file_end","id"}`; the agent answers `file_saved` (with the
//!   path) or `file_error`.
//!
//! Both can be turned off on this computer (AgentConfig allow_clipboard /
//! allow_file_transfer); the client is told why when it tries.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{mpsc as std_mpsc, Arc, Mutex};
use std::time::Duration;

use serde::Deserialize;
use serde_json::json;
use tracing::{info, warn};
use webrtc::data_channel::data_channel_init::RTCDataChannelInit;
use webrtc::data_channel::data_channel_message::DataChannelMessage;
use webrtc::data_channel::RTCDataChannel;
use webrtc::peer_connection::RTCPeerConnection;

/// Clipboard text larger than this isn't sent (it would hold up the channel).
const MAX_CLIPBOARD_BYTES: usize = 1024 * 1024;
const CLIPBOARD_POLL: Duration = Duration::from_millis(500);

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum ClipboardMsg {
    Clipboard { text: String },
}

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum FileMsg {
    FileStart { id: String, name: String, size: u64 },
    FileEnd { id: String },
    FileCancel { id: String },
}

/// Opens the `clipboard` and `file` channels on a new session's connection.
pub async fn attach(
    pc: &Arc<RTCPeerConnection>,
    allow_clipboard: bool,
    allow_files: bool,
) -> anyhow::Result<Vec<tokio::task::JoinHandle<()>>> {
    let reliable = || RTCDataChannelInit { ordered: Some(true), ..Default::default() };
    let clipboard = pc.create_data_channel("clipboard", Some(reliable())).await?;
    let file = pc.create_data_channel("file", Some(reliable())).await?;
    let mut tasks = Vec::new();
    if let Some(task) = setup_clipboard(clipboard, allow_clipboard) {
        tasks.push(task);
    }
    setup_files(file, allow_files);
    Ok(tasks)
}

// ---------------------------------------------------------------- clipboard

fn setup_clipboard(channel: Arc<RTCDataChannel>, allowed: bool) -> Option<tokio::task::JoinHandle<()>> {
    if !allowed {
        let ch = channel.clone();
        channel.on_message(Box::new(move |_msg: DataChannelMessage| {
            let ch = ch.clone();
            Box::pin(async move {
                let _ = ch
                    .send_text(json!({"type": "clipboard_error", "message": "Clipboard sharing is turned off on this computer"}).to_string())
                    .await;
            })
        }));
        return None;
    }

    // Text from the client -> the clipboard thread; local changes -> client.
    let (set_tx, set_rx) = std_mpsc::channel::<String>();
    let (changed_tx, mut changed_rx) = tokio::sync::mpsc::unbounded_channel::<String>();
    std::thread::Builder::new()
        .name("lunaris-clipboard".into())
        .spawn(move || clipboard_thread(set_rx, changed_tx))
        .ok()?;

    let set_tx = Mutex::new(set_tx);
    channel.on_message(Box::new(move |msg: DataChannelMessage| {
        if msg.is_string {
            match serde_json::from_slice::<ClipboardMsg>(&msg.data) {
                Ok(ClipboardMsg::Clipboard { text }) if text.len() <= MAX_CLIPBOARD_BYTES => {
                    let _ = set_tx.lock().unwrap().send(text);
                }
                Ok(_) => warn!("Clipboard text from the client is too large; ignored"),
                Err(e) => warn!("Bad clipboard message: {}", e),
            }
        }
        Box::pin(async {})
    }));

    let ch = channel.clone();
    Some(tokio::spawn(async move {
        while let Some(text) = changed_rx.recv().await {
            if ch.ready_state() != webrtc::data_channel::data_channel_state::RTCDataChannelState::Open {
                continue;
            }
            let payload = json!({"type": "clipboard", "text": text}).to_string();
            if let Err(e) = ch.send_text(payload).await {
                warn!("Could not send the clipboard to the client: {}", e);
            }
        }
    }))
}

/// Owns this computer's clipboard for the session: sets the client's text,
/// reports local changes. Ends when the session's sender is dropped.
fn clipboard_thread(set_rx: std_mpsc::Receiver<String>, changed_tx: tokio::sync::mpsc::UnboundedSender<String>) {
    let mut clipboard = match arboard::Clipboard::new() {
        Ok(c) => c,
        Err(e) => {
            warn!("Clipboard unavailable on this computer: {}", e);
            // Keep draining so the session doesn't notice.
            while set_rx.recv().is_ok() {}
            return;
        }
    };
    // Don't send what was already there when the session started.
    let mut last = clipboard.get_text().unwrap_or_default();
    loop {
        match set_rx.recv_timeout(CLIPBOARD_POLL) {
            Ok(text) => {
                if text != last {
                    match clipboard.set_text(text.clone()) {
                        Ok(()) => last = text,
                        Err(e) => warn!("Could not set the clipboard: {}", e),
                    }
                }
            }
            Err(std_mpsc::RecvTimeoutError::Timeout) => {
                if let Ok(text) = clipboard.get_text() {
                    if text != last && !text.is_empty() && text.len() <= MAX_CLIPBOARD_BYTES {
                        last = text.clone();
                        if changed_tx.send(text).is_err() {
                            return;
                        }
                    }
                }
            }
            Err(std_mpsc::RecvTimeoutError::Disconnected) => return,
        }
    }
}

// ---------------------------------------------------------------- files

struct Incoming {
    id: String,
    path: PathBuf,
    file: std::fs::File,
    expected: u64,
    received: u64,
}

/// Where received files go: Downloads/Lunaris (home directory otherwise).
fn receive_dir() -> PathBuf {
    let base = dirs::download_dir()
        .or_else(dirs::home_dir)
        .unwrap_or_else(|| PathBuf::from("."));
    base.join("Lunaris")
}

/// A file name without directories or characters file systems reject.
fn safe_file_name(name: &str) -> String {
    let base = name.rsplit(['/', '\\']).next().unwrap_or("").trim();
    let cleaned: String = base
        .chars()
        .map(|c| if c.is_control() || matches!(c, ':' | '*' | '?' | '"' | '<' | '>' | '|') { '_' } else { c })
        .collect();
    let cleaned = cleaned.trim_matches('.').trim().to_string();
    if cleaned.is_empty() { "file".to_string() } else { cleaned }
}

/// `name`, or `name (2)`, `name (3)`... when it exists already.
fn unique_path(dir: &Path, name: &str) -> PathBuf {
    let first = dir.join(name);
    if !first.exists() {
        return first;
    }
    let (stem, ext) = match name.rfind('.') {
        Some(i) if i > 0 => (&name[..i], &name[i..]),
        _ => (name, ""),
    };
    (2..10_000)
        .map(|n| dir.join(format!("{} ({}){}", stem, n, ext)))
        .find(|p| !p.exists())
        .unwrap_or(first)
}

fn setup_files(channel: Arc<RTCDataChannel>, allowed: bool) {
    let state: Arc<Mutex<Option<Incoming>>> = Arc::new(Mutex::new(None));
    let ch = channel.clone();
    channel.on_message(Box::new(move |msg: DataChannelMessage| {
        let reply = handle_file_message(&state, msg, allowed);
        let ch = ch.clone();
        Box::pin(async move {
            if let Some(reply) = reply {
                let _ = ch.send_text(reply.to_string()).await;
            }
        })
    }));
}

fn handle_file_message(state: &Mutex<Option<Incoming>>, msg: DataChannelMessage, allowed: bool) -> Option<serde_json::Value> {
    let mut current = state.lock().unwrap();
    if !msg.is_string {
        // A chunk of the file being received.
        let incoming = current.as_mut()?;
        if let Err(e) = incoming.file.write_all(&msg.data) {
            let id = incoming.id.clone();
            let path = incoming.path.clone();
            *current = None;
            let _ = std::fs::remove_file(&path);
            return Some(json!({"type": "file_error", "id": id, "message": format!("Could not write the file: {}", e)}));
        }
        incoming.received += msg.data.len() as u64;
        return None;
    }
    match serde_json::from_slice::<FileMsg>(&msg.data) {
        Ok(FileMsg::FileStart { id, name, size }) => {
            if !allowed {
                return Some(json!({"type": "file_error", "id": id, "message": "File transfer is turned off on this computer"}));
            }
            if let Some(old) = current.take() {
                let _ = std::fs::remove_file(&old.path);
            }
            let dir = receive_dir();
            if let Err(e) = std::fs::create_dir_all(&dir) {
                return Some(json!({"type": "file_error", "id": id, "message": format!("Could not create {}: {}", dir.display(), e)}));
            }
            let path = unique_path(&dir, &safe_file_name(&name));
            match std::fs::File::create(&path) {
                Ok(file) => {
                    info!("Receiving {} ({} bytes) into {}", name, size, path.display());
                    *current = Some(Incoming { id: id.clone(), path, file, expected: size, received: 0 });
                    Some(json!({"type": "file_accepted", "id": id}))
                }
                Err(e) => Some(json!({"type": "file_error", "id": id, "message": format!("Could not create the file: {}", e)})),
            }
        }
        Ok(FileMsg::FileEnd { id }) => {
            let incoming = current.take()?;
            if incoming.id != id {
                return None;
            }
            let _ = incoming.file.sync_all();
            if incoming.received != incoming.expected {
                let _ = std::fs::remove_file(&incoming.path);
                return Some(json!({"type": "file_error", "id": id, "message": format!("Incomplete: {} of {} bytes", incoming.received, incoming.expected)}));
            }
            info!("Received {}", incoming.path.display());
            Some(json!({"type": "file_saved", "id": id, "path": incoming.path.to_string_lossy()}))
        }
        Ok(FileMsg::FileCancel { id }) => {
            if current.as_ref().map(|i| i.id == id).unwrap_or(false) {
                if let Some(old) = current.take() {
                    let _ = std::fs::remove_file(&old.path);
                }
            }
            None
        }
        Err(e) => {
            warn!("Bad file message: {}", e);
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_names_cannot_leave_the_folder() {
        assert_eq!(safe_file_name("../../etc/passwd"), "passwd");
        assert_eq!(safe_file_name("C:\\Users\\x\\report.pdf"), "report.pdf");
        assert_eq!(safe_file_name("a:b*c?.txt"), "a_b_c_.txt");
        assert_eq!(safe_file_name(".."), "file");
        assert_eq!(safe_file_name(""), "file");
    }

    #[test]
    fn existing_names_get_a_number() {
        let dir = std::env::temp_dir().join(format!("lunaris-share-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("a.txt"), b"x").unwrap();
        assert_eq!(unique_path(&dir, "a.txt"), dir.join("a (2).txt"));
        assert_eq!(unique_path(&dir, "b.txt"), dir.join("b.txt"));
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
