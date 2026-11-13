//! Clipboard sharing with the remote computer, over the `clipboard` data
//! channel the agent opens (see agent/src/share.rs for the messages). Files
//! have their own connection (files.rs).
//!
//! - Clipboard: a thread owns this computer's clipboard during the session,
//!   sets the remote computer's text and sends local changes (polled), never
//!   echoing back what it just set. On only while the setting is on.
//! - Messages for the user (e.g. "turned off on the remote computer") are
//!   taken by the UI with `take_status()`.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc as std_mpsc, Arc, Mutex};
use std::time::Duration;

use serde_json::{json, Value};
use webrtc::data_channel::{DataChannel, DataChannelEvent, RTCDataChannelState};

const MAX_CLIPBOARD_BYTES: usize = 1024 * 1024;
const CLIPBOARD_POLL: Duration = Duration::from_millis(500);

static CLIPBOARD_ENABLED: AtomicBool = AtomicBool::new(true);
static FILES_ENABLED: AtomicBool = AtomicBool::new(true);
/// Latest message for the user, taken by the UI.
static STATUS: Mutex<Option<String>> = Mutex::new(None);

/// Settings > Clipboard and files (this device).
pub fn set_options(clipboard: bool, files: bool) {
    CLIPBOARD_ENABLED.store(clipboard, Ordering::Relaxed);
    FILES_ENABLED.store(files, Ordering::Relaxed);
}

/// Settings > Clipboard and files > File transfer (files.rs asks).
pub fn files_enabled() -> bool {
    FILES_ENABLED.load(Ordering::Relaxed)
}

pub fn take_status() -> Option<String> {
    STATUS.lock().unwrap().take()
}

pub(crate) fn set_status(text: impl Into<String>) {
    *STATUS.lock().unwrap() = Some(text.into());
}

/// The agent opened its `clipboard` channel: start syncing for this session.
pub fn attach_clipboard(channel: Arc<dyn DataChannel>) {
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
    // webrtc 0.21: the channel's events are polled.
    let reader = channel.clone();
    tokio::spawn(async move {
        while let Some(event) = reader.poll().await {
            let DataChannelEvent::OnMessage(msg) = event else { continue };
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
        }
    });

    let ch = channel.clone();
    tokio::spawn(async move {
        while let Some(text) = changed_rx.recv().await {
            if !CLIPBOARD_ENABLED.load(Ordering::Relaxed) || !matches!(ch.ready_state().await, Ok(RTCDataChannelState::Open)) {
                continue;
            }
            if let Err(e) = ch.send_text(&json!({"type": "clipboard", "text": text}).to_string()).await {
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
