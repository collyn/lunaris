//! Clipboard sharing with the viewing client, over a reliable, ordered
//! `clipboard` data channel the agent opens with each session: text both
//! ways, `{"type":"clipboard","text":"..."}`. A thread owns this computer's
//! clipboard for the session (on X11 the owner must stay alive for others to
//! paste), sets what the client sends and reports local changes (polled),
//! never echoing back what it just set.
//!
//! Can be turned off on this computer (AgentConfig allow_clipboard); the
//! client is told why when it tries. Files have their own connection
//! (files.rs).

use std::sync::{mpsc as std_mpsc, Arc, Mutex};
use std::time::Duration;

use serde::Deserialize;
use serde_json::json;
use tracing::warn;
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

/// Opens the `clipboard` channel on a new session's connection.
pub async fn attach(pc: &Arc<RTCPeerConnection>, allow_clipboard: bool) -> anyhow::Result<Vec<tokio::task::JoinHandle<()>>> {
    let reliable = RTCDataChannelInit { ordered: Some(true), ..Default::default() };
    let clipboard = pc.create_data_channel("clipboard", Some(reliable)).await?;
    Ok(setup_clipboard(clipboard, allow_clipboard).into_iter().collect())
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
