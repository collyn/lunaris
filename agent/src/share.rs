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
use std::time::{Duration, Instant};

use serde::Deserialize;
use serde_json::json;
use tracing::warn;
use webrtc::data_channel::{DataChannel, DataChannelEvent, RTCDataChannelInit, RTCDataChannelState};
use webrtc::peer_connection::PeerConnection;

/// Clipboard text larger than this isn't sent (it would hold up the channel).
const MAX_CLIPBOARD_BYTES: usize = 1024 * 1024;
const CLIPBOARD_POLL: Duration = Duration::from_millis(500);

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum ClipboardMsg {
    Clipboard { text: String },
}

/// Opens the `clipboard` channel on a new session's connection.
pub async fn attach(pc: &Arc<dyn PeerConnection>, allow_clipboard: bool) -> anyhow::Result<Vec<tokio::task::JoinHandle<()>>> {
    let reliable = RTCDataChannelInit { ordered: true, ..Default::default() };
    let clipboard = pc.create_data_channel("clipboard", Some(reliable)).await?;
    Ok(setup_clipboard(clipboard, allow_clipboard))
}

// ---------------------------------------------------------------- clipboard

fn setup_clipboard(channel: Arc<dyn DataChannel>, allowed: bool) -> Vec<tokio::task::JoinHandle<()>> {
    if !allowed {
        // webrtc 0.21: events are polled, not delivered to callbacks.
        let ch = channel.clone();
        return vec![tokio::spawn(async move {
            while let Some(event) = ch.poll().await {
                if let DataChannelEvent::OnMessage(_) = event {
                    let _ = ch
                        .send_text(&json!({"type": "clipboard_error", "message": "Clipboard sharing is turned off on this computer"}).to_string())
                        .await;
                }
            }
        })];
    }

    // Text from the client -> the clipboard thread; local changes -> client.
    let (set_tx, set_rx) = std_mpsc::channel::<String>();
    let (changed_tx, mut changed_rx) = tokio::sync::mpsc::unbounded_channel::<String>();
    if std::thread::Builder::new()
        .name("lunaris-clipboard".into())
        .spawn(move || clipboard_thread(set_rx, changed_tx))
        .is_err()
    {
        return Vec::new();
    }

    let set_tx = Mutex::new(set_tx);
    let reader = channel.clone();
    let on_message = tokio::spawn(async move {
        while let Some(event) = reader.poll().await {
            let DataChannelEvent::OnMessage(msg) = event else { continue };
            if msg.is_string {
                match serde_json::from_slice::<ClipboardMsg>(&msg.data) {
                    Ok(ClipboardMsg::Clipboard { text }) if text.len() <= MAX_CLIPBOARD_BYTES => {
                        let _ = set_tx.lock().unwrap().send(text);
                    }
                    Ok(_) => warn!("Clipboard text from the client is too large; ignored"),
                    Err(e) => warn!("Bad clipboard message: {}", e),
                }
            }
        }
    });

    let ch = channel.clone();
    let sender = tokio::spawn(async move {
        while let Some(text) = changed_rx.recv().await {
            if !matches!(ch.ready_state().await, Ok(RTCDataChannelState::Open)) {
                continue;
            }
            let payload = json!({"type": "clipboard", "text": text}).to_string();
            if let Err(e) = ch.send_text(&payload).await {
                warn!("Could not send the clipboard to the client: {}", e);
            }
        }
    });
    vec![on_message, sender]
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
                    if text != last && is_own_clipboard(&text) {
                        // Put there to type text (type_text below), not copied.
                        last = text;
                    } else if text != last && !text.is_empty() && text.len() <= MAX_CLIPBOARD_BYTES {
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

// ------------------------------------------------------------- typing text

/// Text from the client's keyboard (phones send characters, not keys). On
/// Wayland the input device can only press keys of the compositor's layout,
/// so characters it doesn't have (Vietnamese, …) are pasted: put on the
/// clipboard, Ctrl+V, and the previous clipboard is put back once typing
/// pauses. Everything else goes through the injector.
pub fn type_text(injector: &lunaris_media::input::InputInjector, text: &str) {
    let wayland = std::env::var("XDG_SESSION_TYPE").unwrap_or_default() == "wayland";
    if !wayland || text.is_ascii() {
        injector.type_text(text);
        return;
    }
    // ASCII runs as keys, the rest pasted, in order.
    let mut run = String::new();
    let mut run_ascii = true;
    for ch in text.chars() {
        if !run.is_empty() && ch.is_ascii() != run_ascii {
            type_run(injector, &run, run_ascii);
            run.clear();
        }
        run_ascii = ch.is_ascii();
        run.push(ch);
    }
    if !run.is_empty() {
        type_run(injector, &run, run_ascii);
    }
}

fn type_run(injector: &lunaris_media::input::InputInjector, run: &str, ascii: bool) {
    if ascii {
        injector.type_text(run);
    } else if !paste(injector, run) {
        injector.type_text(run);
    }
}

/// Clipboard contents the agent set itself (to paste, or restored after),
/// which the clipboard thread must not report to the client.
static OWN_CLIPBOARD: Mutex<Vec<(String, Instant)>> = Mutex::new(Vec::new());
const OWN_CLIPBOARD_FOR: Duration = Duration::from_secs(5);

fn note_own_clipboard(text: &str) {
    if let Ok(mut own) = OWN_CLIPBOARD.lock() {
        own.retain(|(_, at)| at.elapsed() < OWN_CLIPBOARD_FOR);
        own.push((text.to_string(), Instant::now()));
    }
}

fn is_own_clipboard(text: &str) -> bool {
    OWN_CLIPBOARD
        .lock()
        .map(|own| own.iter().any(|(t, at)| t == text && at.elapsed() < OWN_CLIPBOARD_FOR))
        .unwrap_or(false)
}

struct Paster {
    clipboard: arboard::Clipboard,
    /// What was on the clipboard before typing started (put back later).
    saved: Option<String>,
    last_paste: Instant,
}

static PASTER: Mutex<Option<Paster>> = Mutex::new(None);
/// Puts the clipboard back this long after the last paste.
const RESTORE_AFTER: Duration = Duration::from_millis(700);

fn paste(injector: &lunaris_media::input::InputInjector, text: &str) -> bool {
    let Ok(mut guard) = PASTER.lock() else { return false };
    if guard.is_none() {
        match arboard::Clipboard::new() {
            Ok(clipboard) => {
                *guard = Some(Paster { clipboard, saved: None, last_paste: Instant::now() });
                spawn_clipboard_restorer();
            }
            Err(e) => {
                warn!("Cannot paste typed text: clipboard unavailable: {}", e);
                return false;
            }
        }
    }
    let p = guard.as_mut().unwrap();
    if p.saved.is_none() {
        p.saved = Some(p.clipboard.get_text().unwrap_or_default());
    }
    note_own_clipboard(text);
    if let Err(e) = p.clipboard.set_text(text.to_string()) {
        warn!("Cannot paste typed text: {}", e);
        return false;
    }
    p.last_paste = Instant::now();
    drop(guard);
    // Let the clipboard reach the compositor before the app asks for it.
    std::thread::sleep(Duration::from_millis(30));
    injector.keyboard_key(17, true); // Ctrl
    injector.keyboard_key(86, true); // V
    injector.keyboard_key(86, false);
    injector.keyboard_key(17, false);
    injector.flush();
    true
}

fn spawn_clipboard_restorer() {
    let _ = std::thread::Builder::new().name("lunaris-paste-restore".into()).spawn(|| loop {
        std::thread::sleep(Duration::from_millis(200));
        let Ok(mut guard) = PASTER.lock() else { return };
        let Some(p) = guard.as_mut() else { return };
        if p.saved.is_some() && p.last_paste.elapsed() >= RESTORE_AFTER {
            let saved = p.saved.take().unwrap_or_default();
            note_own_clipboard(&saved);
            if saved.is_empty() {
                let _ = p.clipboard.clear();
            } else if let Err(e) = p.clipboard.set_text(saved) {
                warn!("Could not put the clipboard back after typing: {}", e);
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn own_clipboard_text_is_recognised() {
        note_own_clipboard("ệ");
        assert!(is_own_clipboard("ệ"));
        assert!(!is_own_clipboard("something the user copied"));
    }
}
