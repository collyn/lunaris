//! The connection to the agent: connects, answers for the person here
//! (pending.rs), and tries again every few seconds while there is no agent
//! or after it went away.

use std::sync::atomic::{AtomicBool, Ordering};

use tracing::{info, warn};

/// Whether an agent lets this helper answer now (the tray shows it).
pub static CONNECTED: AtomicBool = AtomicBool::new(false);

/// Connects to the agent and answers for the person here, again and again;
/// `on_ready` each time the agent lets this helper answer.
pub async fn run(mut on_ready: impl FnMut()) {
    let mut last_error = String::new();
    loop {
        match approval_channel::connect().await {
            Ok((reader, writer)) => {
                let (tx, mut answers) = tokio::sync::mpsc::unbounded_channel();
                let result = approval_channel::run_answerer(
                    reader,
                    writer,
                    || {
                        crate::pending::set_remote(Some(tx));
                        CONNECTED.store(true, Ordering::SeqCst);
                        info!("Connected to the agent: its connection requests are shown here");
                        on_ready();
                    },
                    crate::pending::on_event,
                    &mut answers,
                )
                .await;
                if CONNECTED.swap(false, Ordering::SeqCst) {
                    crate::pending::set_remote(None);
                    info!("The agent went away");
                    last_error.clear();
                }
                if let Err(e) = result {
                    log_once(&mut last_error, e);
                }
            }
            Err(e) => log_once(&mut last_error, e),
        }
        tokio::time::sleep(approval_channel::RETRY).await;
    }
}

/// Logs why the agent cannot be reached, when that changed (not every few
/// seconds while there is no agent).
fn log_once(last: &mut String, e: std::io::Error) {
    let text = e.to_string();
    if *last != text {
        match e.kind() {
            std::io::ErrorKind::PermissionDenied | std::io::ErrorKind::InvalidData => warn!("Approval helper: {}", text),
            _ => info!("Approval helper: no agent to answer for yet ({})", text),
        }
        *last = text;
    }
}
