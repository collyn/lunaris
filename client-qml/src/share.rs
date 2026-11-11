//! Clipboard and file sharing with the remote computer, over the `clipboard`
//! and `file` data channels the agent opens (see agent/src/share.rs for the
//! messages).
//!
//! - Clipboard: a thread owns this computer's clipboard during the session,
//!   sets the remote computer's text and sends local changes (polled), never
//!   echoing back what it just set. On only while the setting is on.
//! - Files: `send_files` sends files one after another in 16 KB chunks,
//!   waiting while the channel's buffer is full, and reports progress and the
//!   result through `take_status()` (shown by the UI).

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc as std_mpsc, Arc, Mutex};
use std::time::Duration;

use bytes::Bytes;
use serde_json::{json, Value};
use webrtc::data_channel::data_channel_message::DataChannelMessage;
use webrtc::data_channel::data_channel_state::RTCDataChannelState;
use webrtc::data_channel::RTCDataChannel;

const MAX_CLIPBOARD_BYTES: usize = 1024 * 1024;
const CLIPBOARD_POLL: Duration = Duration::from_millis(500);
/// webrtc-rs reads messages into a 65535-byte buffer and closes the channel
/// on anything larger.
const CHUNK: usize = 16 * 1024;
/// Stop queueing while this much is waiting to be sent.
const MAX_BUFFERED: usize = 4 * 1024 * 1024;

static CLIPBOARD_ENABLED: AtomicBool = AtomicBool::new(true);
static FILES_ENABLED: AtomicBool = AtomicBool::new(true);
static FILE_CHANNEL: Mutex<Option<Arc<RTCDataChannel>>> = Mutex::new(None);
/// Replies on the file channel, for the transfer in progress.
static FILE_REPLIES: Mutex<Option<tokio::sync::mpsc::UnboundedSender<Value>>> = Mutex::new(None);
/// Latest message for the user (progress, saved, error), taken by the UI.
static STATUS: Mutex<Option<String>> = Mutex::new(None);
static SENDING: AtomicBool = AtomicBool::new(false);

/// Settings > Clipboard and files (this device).
pub fn set_options(clipboard: bool, files: bool) {
    CLIPBOARD_ENABLED.store(clipboard, Ordering::Relaxed);
    FILES_ENABLED.store(files, Ordering::Relaxed);
}

pub fn take_status() -> Option<String> {
    STATUS.lock().unwrap().take()
}

fn set_status(text: impl Into<String>) {
    *STATUS.lock().unwrap() = Some(text.into());
}

/// The agent opened its `clipboard` channel: start syncing for this session.
pub fn attach_clipboard(channel: Arc<RTCDataChannel>) {
    let (set_tx, set_rx) = std_mpsc::channel::<String>();
    let (changed_tx, mut changed_rx) = tokio::sync::mpsc::unbounded_channel::<String>();
    if std::thread::Builder::new()
        .name("lunaris-clipboard".into())
        .spawn(move || clipboard_thread(set_rx, changed_tx))
        .is_err()
    {
        return;
    }

    let set_tx = Mutex::new(set_tx);
    channel.on_message(Box::new(move |msg: DataChannelMessage| {
        if msg.is_string {
            if let Ok(v) = serde_json::from_slice::<Value>(&msg.data) {
                match v.get("type").and_then(Value::as_str) {
                    Some("clipboard") if CLIPBOARD_ENABLED.load(Ordering::Relaxed) => {
                        if let Some(text) = v.get("text").and_then(Value::as_str) {
                            if text.len() <= MAX_CLIPBOARD_BYTES {
                                let _ = set_tx.lock().unwrap().send(text.to_string());
                            }
                        }
                    }
                    Some("clipboard_error") => {
                        if let Some(m) = v.get("message").and_then(Value::as_str) {
                            set_status(m);
                        }
                    }
                    _ => {}
                }
            }
        }
        Box::pin(async {})
    }));

    let ch = channel.clone();
    tokio::spawn(async move {
        while let Some(text) = changed_rx.recv().await {
            if !CLIPBOARD_ENABLED.load(Ordering::Relaxed) || ch.ready_state() != RTCDataChannelState::Open {
                continue;
            }
            if let Err(e) = ch.send_text(json!({"type": "clipboard", "text": text}).to_string()).await {
                eprintln!("Clipboard: could not send to the remote computer: {}", e);
            }
        }
    });
    // The thread ends when this session's handler (holding set_tx) goes.
}

fn clipboard_thread(set_rx: std_mpsc::Receiver<String>, changed_tx: tokio::sync::mpsc::UnboundedSender<String>) {
    let mut clipboard = match arboard::Clipboard::new() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("Clipboard unavailable here: {}", e);
            while set_rx.recv().is_ok() {}
            return;
        }
    };
    let mut last = clipboard.get_text().unwrap_or_default();
    loop {
        match set_rx.recv_timeout(CLIPBOARD_POLL) {
            Ok(text) => {
                if text != last {
                    match clipboard.set_text(text.clone()) {
                        Ok(()) => last = text,
                        Err(e) => eprintln!("Clipboard: could not set: {}", e),
                    }
                }
            }
            Err(std_mpsc::RecvTimeoutError::Timeout) => {
                if !CLIPBOARD_ENABLED.load(Ordering::Relaxed) {
                    continue;
                }
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

/// The agent opened its `file` channel.
pub fn attach_files(channel: Arc<RTCDataChannel>) {
    channel.on_message(Box::new(move |msg: DataChannelMessage| {
        if msg.is_string {
            if let Ok(v) = serde_json::from_slice::<Value>(&msg.data) {
                if let Some(tx) = FILE_REPLIES.lock().unwrap().as_ref() {
                    let _ = tx.send(v);
                }
            }
        }
        Box::pin(async {})
    }));
    *FILE_CHANNEL.lock().unwrap() = Some(channel);
}

/// Session over.
pub fn detach() {
    *FILE_CHANNEL.lock().unwrap() = None;
}

fn human_size(bytes: u64) -> String {
    match bytes {
        b if b >= 1 << 30 => format!("{:.1} GB", b as f64 / (1u64 << 30) as f64),
        b if b >= 1 << 20 => format!("{:.1} MB", b as f64 / (1u64 << 20) as f64),
        b if b >= 1 << 10 => format!("{:.0} KB", b as f64 / 1024.0),
        b => format!("{} B", b),
    }
}

/// Sends local files to the remote computer's Downloads/Lunaris.
pub async fn send_files(paths: Vec<std::path::PathBuf>) {
    if !FILES_ENABLED.load(Ordering::Relaxed) {
        set_status("File transfer is turned off in Settings");
        return;
    }
    let Some(channel) = FILE_CHANNEL.lock().unwrap().clone() else {
        set_status("Connect to a computer to send files");
        return;
    };
    if SENDING.swap(true, Ordering::SeqCst) {
        set_status("Already sending files; try again when it's done");
        return;
    }
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<Value>();
    *FILE_REPLIES.lock().unwrap() = Some(tx);

    let total = paths.len();
    let mut sent = 0;
    for (n, path) in paths.iter().enumerate() {
        match send_one(&channel, &mut rx, path, n + 1, total).await {
            Ok(saved) => {
                sent += 1;
                set_status(format!("Sent {} → {}", file_name(path), saved));
            }
            Err(e) => {
                set_status(format!("{}: {}", file_name(path), e));
                break;
            }
        }
    }
    if total > 1 && sent == total {
        set_status(format!("Sent {} files", total));
    }
    *FILE_REPLIES.lock().unwrap() = None;
    SENDING.store(false, Ordering::SeqCst);
}

fn file_name(path: &std::path::Path) -> String {
    path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "file".into())
}

async fn wait_reply(rx: &mut tokio::sync::mpsc::UnboundedReceiver<Value>, id: &str, secs: u64) -> Result<Value, String> {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(secs);
    loop {
        let v = tokio::time::timeout_at(deadline, rx.recv())
            .await
            .map_err(|_| "no answer from the remote computer".to_string())?
            .ok_or_else(|| "connection closed".to_string())?;
        if v.get("id").and_then(Value::as_str) == Some(id) {
            return Ok(v);
        }
    }
}

async fn send_one(
    channel: &Arc<RTCDataChannel>,
    rx: &mut tokio::sync::mpsc::UnboundedReceiver<Value>,
    path: &std::path::Path,
    index: usize,
    total: usize,
) -> Result<String, String> {
    use tokio::io::AsyncReadExt;
    let meta = tokio::fs::metadata(path).await.map_err(|e| e.to_string())?;
    if !meta.is_file() {
        return Err("not a file".into());
    }
    let size = meta.len();
    let name = file_name(path);
    let id = uuid_like();

    channel
        .send_text(json!({"type": "file_start", "id": id, "name": name, "size": size}).to_string())
        .await
        .map_err(|e| e.to_string())?;
    let reply = wait_reply(rx, &id, 15).await?;
    if reply.get("type").and_then(Value::as_str) == Some("file_error") {
        return Err(reply.get("message").and_then(Value::as_str).unwrap_or("refused").to_string());
    }

    let mut file = tokio::fs::File::open(path).await.map_err(|e| e.to_string())?;
    let mut buf = vec![0u8; CHUNK];
    let mut done: u64 = 0;
    let mut last_report = std::time::Instant::now();
    loop {
        let n = file.read(&mut buf).await.map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        while channel.buffered_amount().await > MAX_BUFFERED {
            if channel.ready_state() != RTCDataChannelState::Open {
                return Err("connection closed".into());
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        channel.send(&Bytes::copy_from_slice(&buf[..n])).await.map_err(|e| e.to_string())?;
        done += n as u64;
        if last_report.elapsed() > Duration::from_millis(400) {
            last_report = std::time::Instant::now();
            let pct = if size > 0 { done * 100 / size } else { 100 };
            let prefix = if total > 1 { format!("({}/{}) ", index, total) } else { String::new() };
            set_status(format!("{}Sending {}: {}% of {}", prefix, name, pct, human_size(size)));
        }
    }
    channel
        .send_text(json!({"type": "file_end", "id": id}).to_string())
        .await
        .map_err(|e| e.to_string())?;
    // Big files take a while to drain from the buffer.
    let reply = wait_reply(rx, &id, 600).await?;
    match reply.get("type").and_then(Value::as_str) {
        Some("file_saved") => Ok(reply.get("path").and_then(Value::as_str).unwrap_or("").to_string()),
        _ => Err(reply.get("message").and_then(Value::as_str).unwrap_or("failed").to_string()),
    }
}

fn uuid_like() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let nanos = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
    format!("f{:x}{:x}", nanos, std::process::id())
}
