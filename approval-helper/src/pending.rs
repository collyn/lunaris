//! The agent's "accept this connection?" questions waiting here, for the
//! dialog to show (it polls [`pending_json`] and answers with [`answer`]);
//! the answers go back over the channel ([`set_remote`]).

use std::sync::Mutex;

use approval_channel::{Event, Question};
use tokio::sync::mpsc::UnboundedSender;

/// The questions waiting for an answer, oldest first.
static PENDING: Mutex<Vec<Question>> = Mutex::new(Vec::new());

/// Where the answers go: the agent's channel while connected.
static REMOTE: Mutex<Option<UnboundedSender<(u64, bool)>>> = Mutex::new(None);

/// A question or withdrawal from the agent.
pub fn on_event(event: Event) {
    let mut pending = PENDING.lock().unwrap_or_else(|e| e.into_inner());
    match event {
        Event::Ask(q) => {
            pending.retain(|p| p.id != q.id);
            pending.push(q);
        }
        Event::Cancel(id) => pending.retain(|p| p.id != id),
    }
}

/// The questions waiting, as JSON: `[{"id":1,"requester":"…","kind":"control","timeout_s":30}]`.
pub fn pending_json() -> String {
    let pending = PENDING.lock().unwrap_or_else(|e| e.into_inner());
    serde_json::to_string(&*pending).unwrap_or_else(|_| "[]".into())
}

/// The person here answered question `id` (dropped when no agent is
/// connected: it declined the question when the helper went away).
pub fn answer(id: u64, accept: bool) {
    PENDING.lock().unwrap_or_else(|e| e.into_inner()).retain(|q| q.id != id);
    if let Some(remote) = REMOTE.lock().unwrap_or_else(|e| e.into_inner()).as_ref() {
        let _ = remote.send((id, accept));
    }
}

/// The answers go to `remote` from now on (None: the agent went away).
/// The questions shown are dropped either way.
pub fn set_remote(remote: Option<UnboundedSender<(u64, bool)>>) {
    *REMOTE.lock().unwrap_or_else(|e| e.into_inner()) = remote;
    PENDING.lock().unwrap_or_else(|e| e.into_inner()).clear();
}

#[cfg(test)]
mod tests {
    use super::*;

    fn listed() -> Vec<serde_json::Value> {
        serde_json::from_str(&pending_json()).unwrap()
    }

    fn q(id: u64, kind: &'static str) -> Question {
        Question { id, requester: "huy".into(), kind, timeout_s: 30 }
    }

    #[tokio::test]
    async fn questions_listed_and_answers_sent() {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        set_remote(Some(tx));
        on_event(Event::Ask(q(5, "control")));
        on_event(Event::Ask(q(6, "files")));
        on_event(Event::Cancel(6));
        let list = listed();
        assert_eq!(list.len(), 1);
        assert_eq!((list[0]["id"].as_u64(), list[0]["kind"].as_str()), (Some(5), Some("control")));
        answer(5, true);
        assert_eq!(rx.recv().await, Some((5, true)));
        assert!(listed().is_empty());
        // The agent went away: what was shown goes, late answers go nowhere.
        on_event(Event::Ask(q(7, "files")));
        set_remote(None);
        assert!(listed().is_empty());
        answer(7, true);
        assert!(rx.recv().await.is_none());
    }
}
