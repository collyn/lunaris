//! Asking the person at this computer to accept a connection made without
//! the access password, like RustDesk's "accept sessions via click" and
//! AnyDesk's accept window.
//!
//! Whoever shows the question is attached here, one of:
//!
//! - the Lunaris app, which runs the agent as a child: started with
//!   `--approval-stdio`, the agent writes each question as one line on
//!   stdout and reads the answers from stdin ([`attach_stdio`]):
//!
//!   ```text
//!   agent -> app   @lunaris-approval {"id":1,"requester":"huy (server x)","kind":"control","timeout_s":30}
//!   agent -> app   @lunaris-approval-cancel {"id":1}
//!   app -> agent   {"id":1,"accept":true}
//!   ```
//!
//! - the agent's own window, which runs the agent in its process: it is
//!   handed each [`Event`] and answers with [`answer`] ([`attach`]).
//!
//! - the approval helper (`lunaris-approval-helper`, an optional tray
//!   program), for the agent running as a daemon (`--cli`, the Windows
//!   service): the same lines over a local channel only the person at this
//!   computer can open (the approval-channel crate, [`spawn_helper_listener`]).
//!
//! Without any of them ("unattended"), only the password lets anyone in.

use std::collections::HashMap;
use std::io::{BufRead, Write};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::sync::oneshot;
use tracing::{info, warn};

/// How long the question stays up before it counts as declined.
pub const TIMEOUT: Duration = Duration::from_secs(30);

/// How connections without the right password are treated (`approve_mode`
/// in the config), the same choices as RustDesk's.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApproveMode {
    /// Only the password lets anyone in.
    Password,
    /// Every connection is accepted at this computer, password or not.
    Click,
    /// The password lets one in; without it, the person here can accept.
    PasswordClick,
}

impl ApproveMode {
    pub fn parse(s: &str) -> Self {
        match s.trim() {
            "password" => ApproveMode::Password,
            "click" => ApproveMode::Click,
            _ => ApproveMode::PasswordClick,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Kind {
    /// See and control the screen.
    Control,
    /// Transfer files.
    Files,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    Accepted,
    Denied,
    Timeout,
    /// Nobody can answer here.
    Unattended,
}

impl Decision {
    /// `status` of `SignalingMessage::AccessRequest`.
    pub fn status(self) -> &'static str {
        match self {
            Decision::Accepted => "accepted",
            Decision::Denied => "denied",
            Decision::Timeout => "timeout",
            Decision::Unattended => "unattended",
        }
    }
}

pub use approval_channel::{answer_line, event_line, parse_answer_line, parse_event_line, Event, Question};

type Callback = Arc<dyn Fn(Event) + Send + Sync>;

/// Where questions go.
#[derive(Clone)]
enum Sink {
    Stdio,
    Callback(Callback),
}

static ATTACHED: AtomicBool = AtomicBool::new(false);
/// Counts [`attach`]es: [`detach_if`] leaves a newer one attached.
static GENERATION: AtomicU64 = AtomicU64::new(0);
static NEXT_ID: AtomicU64 = AtomicU64::new(1);
static PENDING: Mutex<Option<HashMap<u64, oneshot::Sender<bool>>>> = Mutex::new(None);
static STDOUT: Mutex<()> = Mutex::new(());
static SINK: Mutex<Option<Sink>> = Mutex::new(None);

/// Lets the app answer: questions on stdout, answers on stdin.
pub fn attach_stdio() {
    if ATTACHED.swap(true, Ordering::SeqCst) {
        return;
    }
    *SINK.lock().unwrap() = Some(Sink::Stdio);
    std::thread::Builder::new()
        .name("approval-stdin".into())
        .spawn(|| {
            let stdin = std::io::stdin();
            for line in stdin.lock().lines() {
                let Ok(line) = line else { break };
                match parse_answer_line(&line) {
                    Ok(Some((id, accept))) => answer(id, accept),
                    Ok(None) => {}
                    Err(e) => warn!("approval: ignoring {:?}: {}", line, e),
                }
            }
            // The app is gone: nobody can answer any more.
            detach();
        })
        .ok();
}

/// Lets a window in this process answer: `on_event` is handed each
/// question and each withdrawal (from the agent's tasks: it must not
/// block), and the window answers with [`answer`]. Replaces whoever
/// answered before. Returns what [`detach_if`] takes.
pub fn attach(on_event: impl Fn(Event) + Send + Sync + 'static) -> u64 {
    let mut sink = SINK.lock().unwrap();
    *sink = Some(Sink::Callback(Arc::new(on_event)));
    ATTACHED.store(true, Ordering::SeqCst);
    GENERATION.fetch_add(1, Ordering::SeqCst) + 1
}

/// [`detach`], unless someone attached after the [`attach`] that returned
/// `generation`.
pub fn detach_if(generation: u64) {
    let mut sink = SINK.lock().unwrap();
    if GENERATION.load(Ordering::SeqCst) == generation {
        clear(&mut sink);
    }
}

/// Nobody can answer any more: the questions waiting count as declined,
/// and the next ones are unattended.
pub fn detach() {
    clear(&mut SINK.lock().unwrap());
}

/// [`detach`] under the lock of `SINK`.
fn clear(sink: &mut Option<Sink>) {
    GENERATION.fetch_add(1, Ordering::SeqCst);
    ATTACHED.store(false, Ordering::SeqCst);
    *sink = None;
    if let Some(map) = PENDING.lock().unwrap().as_mut() {
        for (_, tx) in map.drain() {
            let _ = tx.send(false);
        }
    }
}

/// Whether someone can be asked.
pub fn attended() -> bool {
    ATTACHED.load(Ordering::SeqCst)
}

fn emit(event: Event) {
    // Not under the lock while the callback runs.
    let sink = SINK.lock().unwrap().clone();
    match sink {
        Some(Sink::Stdio) => {
            let line = event_line(&event);
            let _guard = STDOUT.lock().unwrap();
            let mut out = std::io::stdout().lock();
            let _ = writeln!(out, "{}", line);
            let _ = out.flush();
        }
        Some(Sink::Callback(f)) => f(event),
        None => {}
    }
}

/// The person here answered question `id` (unknown or past ids are ignored).
pub fn answer(id: u64, accept: bool) {
    let tx = PENDING.lock().unwrap().as_mut().and_then(|m| m.remove(&id));
    if let Some(tx) = tx {
        let _ = tx.send(accept);
    }
}

/// The agent's questions, for the approval helper's channel.
struct HelperQuestions;

impl approval_channel::Answerer for HelperQuestions {
    fn attach(&self, sink: Box<dyn Fn(Event) + Send + Sync>) -> u64 {
        attach(sink)
    }
    fn answer(&self, id: u64, accept: bool) {
        answer(id, accept)
    }
    fn detach(&self, token: u64) {
        detach_if(token)
    }
}

/// The agent running as a daemon: the person here answers through the
/// approval helper, when one connects (in the background; in a tokio
/// runtime). Without one, nothing changes: only the password lets anyone in.
pub fn spawn_helper_listener() {
    approval_channel::spawn_listener(Arc::new(HelperQuestions));
}

/// What the person here is asked about for a signaling message; None for
/// messages that need no permission.
pub fn kind_of(sig: &common::SignalingMessage) -> Option<Kind> {
    use common::SignalingMessage::*;
    match sig {
        RequestSession { .. } | IncomingSession { .. } => Some(Kind::Control),
        RequestFileSession { .. } | IncomingFileSession { .. } => Some(Kind::Files),
        _ => None,
    }
}

/// Messages a client may send before it is accepted: questions about the
/// computer, and the end of its own connection; nothing that shows or
/// changes anything.
pub fn allowed_before_approval(sig: &common::SignalingMessage) -> bool {
    use common::SignalingMessage::*;
    matches!(sig, GetCapabilities { .. } | GetAppList { .. } | EndSession { .. })
}

/// Asks the person here to let `requester` in; waits up to [`TIMEOUT`].
pub async fn ask(requester: &str, kind: Kind) -> Decision {
    ask_unless(requester, kind, std::future::pending::<()>()).await.unwrap_or(Decision::Denied)
}

/// Like [`ask`], but withdrawn when `cancel` completes first (the one
/// asking went away): the question is taken down and None returned.
pub async fn ask_unless(requester: &str, kind: Kind, cancel: impl std::future::Future<Output = ()>) -> Option<Decision> {
    ask_within(requester, kind, cancel, TIMEOUT).await
}

/// A question on screen; taken down when dropped unless accepted, also
/// when the task asking is dropped (the agent stopped) mid-question.
struct Shown {
    id: u64,
    accepted: bool,
}

impl Drop for Shown {
    fn drop(&mut self) {
        if !self.accepted {
            PENDING.lock().unwrap().as_mut().map(|m| m.remove(&self.id));
            emit(Event::Cancel(self.id));
        }
    }
}

async fn ask_within(
    requester: &str,
    kind: Kind,
    cancel: impl std::future::Future<Output = ()>,
    timeout: Duration,
) -> Option<Decision> {
    if !attended() {
        return Some(Decision::Unattended);
    }
    let id = NEXT_ID.fetch_add(1, Ordering::SeqCst);
    let (tx, rx) = oneshot::channel();
    PENDING.lock().unwrap().get_or_insert_with(HashMap::new).insert(id, tx);
    let question = Question {
        id,
        requester: requester.to_string(),
        kind: match kind {
            Kind::Control => "control",
            Kind::Files => "files",
        },
        timeout_s: timeout.as_secs(),
    };
    info!("Asking to accept a connection from {:?} ({:?})", requester, kind);
    let mut shown = Shown { id, accepted: false };
    emit(Event::Ask(question));
    let decision = tokio::select! {
        answer = tokio::time::timeout(timeout, rx) => Some(match answer {
            Ok(Ok(true)) => Decision::Accepted,
            Ok(_) => Decision::Denied,
            Err(_) => Decision::Timeout,
        }),
        _ = cancel => None,
    };
    shown.accepted = decision == Some(Decision::Accepted);
    drop(shown);
    match decision {
        Some(d) => info!("Connection from {:?}: {}", requester, d.status()),
        None => info!("Connection from {:?}: withdrawn before an answer", requester),
    }
    decision
}

/// The tests of this state (here and in approval_channel.rs) run one at a time.
#[cfg(test)]
pub(crate) static TEST_SERIAL: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn modes() {
        assert_eq!(ApproveMode::parse("password"), ApproveMode::Password);
        assert_eq!(ApproveMode::parse("click"), ApproveMode::Click);
        assert_eq!(ApproveMode::parse("password-click"), ApproveMode::PasswordClick);
        assert_eq!(ApproveMode::parse(""), ApproveMode::PasswordClick);
    }

    use super::TEST_SERIAL as SERIAL;

    /// Attaches a window that records what it is told.
    fn record() -> Arc<Mutex<Vec<Event>>> {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let s = seen.clone();
        attach(move |e| s.lock().unwrap().push(e));
        seen
    }

    async fn asked(seen: &Arc<Mutex<Vec<Event>>>) -> Question {
        loop {
            if let Some(Event::Ask(q)) = seen.lock().unwrap().iter().rev().find(|e| matches!(e, Event::Ask(_))) {
                return q.clone();
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    }

    #[tokio::test]
    async fn asking() {
        let _serial = SERIAL.lock().await;
        detach();
        assert_eq!(ask("x", Kind::Control).await, Decision::Unattended);
        ATTACHED.store(true, Ordering::SeqCst);
        let task = tokio::spawn(async { ask("someone", Kind::Files).await });
        // Wait for the question to be registered, then accept it.
        let id = loop {
            if let Some(id) = PENDING.lock().unwrap().as_ref().and_then(|m| m.keys().next().copied()) {
                break id;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        };
        answer(id, true);
        assert_eq!(task.await.unwrap(), Decision::Accepted);
        detach();
    }

    #[tokio::test]
    async fn window_accepts_and_declines() {
        let _serial = SERIAL.lock().await;
        let seen = record();
        let task = tokio::spawn(async { ask("huy (direct)", Kind::Control).await });
        let q = asked(&seen).await;
        assert_eq!((q.requester.as_str(), q.kind, q.timeout_s), ("huy (direct)", "control", 30));
        answer(q.id, true);
        assert_eq!(task.await.unwrap(), Decision::Accepted);
        // Accepted: the window took it down itself, nothing withdrawn.
        assert!(!seen.lock().unwrap().contains(&Event::Cancel(q.id)));

        seen.lock().unwrap().clear();
        let task = tokio::spawn(async { ask("someone", Kind::Files).await });
        let q = asked(&seen).await;
        assert_eq!(q.kind, "files");
        // An answer for another question changes nothing.
        answer(q.id + 1000, true);
        answer(q.id, false);
        assert_eq!(task.await.unwrap(), Decision::Denied);
        assert_eq!(seen.lock().unwrap().last(), Some(&Event::Cancel(q.id)));
        detach();
    }

    #[tokio::test]
    async fn unanswered_times_out_as_declined() {
        let _serial = SERIAL.lock().await;
        let seen = record();
        let d = ask_within("someone", Kind::Control, std::future::pending(), Duration::from_millis(50)).await;
        assert_eq!(d, Some(Decision::Timeout));
        let events = seen.lock().unwrap().clone();
        let Event::Ask(q) = &events[0] else { panic!("{:?}", events) };
        assert_eq!(events[1], Event::Cancel(q.id));
        // A late answer is ignored.
        answer(q.id, true);
        assert!(PENDING.lock().unwrap().as_ref().is_none_or(|m| m.is_empty()));
        detach();
    }

    #[tokio::test]
    async fn withdrawn_or_dropped_question_is_taken_down() {
        let _serial = SERIAL.lock().await;
        let seen = record();
        // The one asking went away.
        let d = ask_within("someone", Kind::Control, async {}, TIMEOUT).await;
        assert_eq!(d, None);
        assert!(matches!(seen.lock().unwrap().last(), Some(Event::Cancel(_))));

        // The agent stopped while asking (its task dropped).
        seen.lock().unwrap().clear();
        let task = tokio::spawn(async { ask("someone", Kind::Files).await });
        let q = asked(&seen).await;
        task.abort();
        let _ = task.await;
        assert_eq!(seen.lock().unwrap().last(), Some(&Event::Cancel(q.id)));
        detach();
    }

    #[tokio::test]
    async fn detach_declines_waiting_questions() {
        let _serial = SERIAL.lock().await;
        let seen = record();
        let task = tokio::spawn(async { ask("someone", Kind::Control).await });
        asked(&seen).await;
        detach();
        assert_eq!(task.await.unwrap(), Decision::Denied);
        assert_eq!(ask("x", Kind::Control).await, Decision::Unattended);
    }

    #[tokio::test]
    async fn detach_if_spares_a_newer_answerer() {
        let _serial = SERIAL.lock().await;
        let old = attach(|_| {});
        let new = attach(|_| {});
        detach_if(old);
        assert!(attended());
        detach_if(new);
        assert!(!attended());
    }

    /// The approval helper's channel against this state: questions go out
    /// as lines, answers come back, and the helper leaving declines what
    /// waits and leaves the agent unattended.
    #[tokio::test]
    async fn helper_over_the_channel() {
        use tokio::io::{AsyncBufReadExt, AsyncWriteExt};
        let _serial = SERIAL.lock().await;
        detach();
        let (agent_end, helper_end) = tokio::io::duplex(4096);
        let (ar, aw) = tokio::io::split(agent_end);
        let served = tokio::spawn(async move { approval_channel::serve_answerer(ar, aw, &HelperQuestions).await });
        let (hr, mut hw) = tokio::io::split(helper_end);
        let mut hr = tokio::io::BufReader::new(hr).lines();
        assert_eq!(hr.next_line().await.unwrap().unwrap(), approval_channel::READY_LINE);
        while !attended() {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }

        let task = tokio::spawn(async { ask("huy (direct)", Kind::Control).await });
        let Some(Event::Ask(q)) = parse_event_line(&hr.next_line().await.unwrap().unwrap()) else { panic!() };
        assert_eq!((q.requester.as_str(), q.kind), ("huy (direct)", "control"));
        hw.write_all(format!("{}\n", answer_line(q.id, true)).as_bytes()).await.unwrap();
        assert_eq!(task.await.unwrap(), Decision::Accepted);

        let task = tokio::spawn(async { ask("someone", Kind::Files).await });
        let Some(Event::Ask(q)) = parse_event_line(&hr.next_line().await.unwrap().unwrap()) else { panic!() };
        hw.write_all(format!("{}\n", answer_line(q.id, false)).as_bytes()).await.unwrap();
        assert_eq!(task.await.unwrap(), Decision::Denied);
        assert_eq!(parse_event_line(&hr.next_line().await.unwrap().unwrap()), Some(Event::Cancel(q.id)));

        let task = tokio::spawn(async { ask("late", Kind::Control).await });
        hr.next_line().await.unwrap().unwrap();
        drop(hw);
        drop(hr);
        assert_eq!(task.await.unwrap(), Decision::Denied);
        served.await.unwrap().unwrap();
        assert_eq!(ask("x", Kind::Control).await, Decision::Unattended);
    }

    #[test]
    fn stdio_json_unchanged() {
        let q = Question { id: 1, requester: "huy (server x)".into(), kind: "control", timeout_s: 30 };
        assert_eq!(
            serde_json::to_string(&q).unwrap(),
            r#"{"id":1,"requester":"huy (server x)","kind":"control","timeout_s":30}"#
        );
    }
}
