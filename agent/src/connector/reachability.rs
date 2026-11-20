//! Whether the computers of other types (sources) are there: a computer
//! that is shut down must not be listed as online only because the Lunaris
//! computer it is reached through is.
//!
//! A source is checked where its connector would go: its address field
//! ("host" / "address") and its "port" field, or the default of the
//! connector's "port" field when it has none (RustDesk 21118, Sunshine's
//! HTTP port 47989, RDP 3389, VNC 5900; an external connector by what its
//! manifest says). The check is a plain TCP connection that is closed at
//! once: nothing is sent, no password, no login. A computer that accepts it
//! is there, one that refuses it or does not answer within [`TIMEOUT`] is
//! not. A source with no address or port to check (an external connector
//! whose fields say nothing of it) stays unknown.
//!
//! The checks run in a thread of their own every [`INTERVAL`] (jittered), a
//! few at once, for every address the sources have (sources at one address
//! share one check). An address a session is connected to now counts as
//! there and is not checked ([`in_use`]). When what is known changes, the
//! monitor's `on_change` is called once for the whole round: the agent then
//! tells the servers its sources again (`sources::announcement`), with
//! `online` for each.
//!
//! Addresses are logged at debug level only.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use tracing::{debug, info};

use super::{Description, SourceConfig, Target};

/// How often each source is checked.
pub const INTERVAL: Duration = Duration::from_secs(15);
/// How long a check waits for the computer.
pub const TIMEOUT: Duration = Duration::from_secs(2);
/// How many checks run at once.
const AT_ONCE: usize = 8;
/// The most a round starts later than [`INTERVAL`], so that checks of many
/// agents do not fall on the same moment.
const JITTER_MS: u64 = 3_000;

/// The fields that say where a computer is, by their key.
const ADDRESS_KEYS: [&str; 2] = ["host", "address"];
const PORT_KEY: &str = "port";

/// Where a source is checked.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Endpoint {
    pub host: String,
    pub port: u16,
}

/// Where `target` is checked: its address, and its port or its connector's
/// default port. `None` when either is not known.
pub fn endpoint(target: &Target, description: Option<&Description>) -> Option<Endpoint> {
    let host = ADDRESS_KEYS.iter().map(|key| target.get(key).trim()).find(|host| !host.is_empty())?;
    // An IPv6 address may be typed with its brackets.
    let host = host.strip_prefix('[').and_then(|h| h.strip_suffix(']')).unwrap_or(host);
    let port = match target.get(PORT_KEY).trim() {
        "" => description?.fields.iter().find(|field| field.key == PORT_KEY)?.default.trim().parse::<u16>().ok()?,
        port => port.parse::<u16>().ok()?,
    };
    (port != 0).then(|| Endpoint { host: host.to_lowercase(), port })
}

/// One check: whether a TCP connection to `endpoint` is accepted within
/// `timeout`. The connection is closed at once; nothing is sent.
pub async fn probe(endpoint: &Endpoint, timeout: Duration) -> bool {
    let connect = tokio::net::TcpStream::connect((endpoint.host.as_str(), endpoint.port));
    let there = matches!(tokio::time::timeout(timeout, connect).await, Ok(Ok(_)));
    debug!("Source check: {}:{} {}", endpoint.host, endpoint.port, if there { "answers" } else { "does not answer" });
    there
}

// ---- What sessions use now ----

fn in_use_counts() -> &'static Mutex<HashMap<Endpoint, usize>> {
    static IN_USE: OnceLock<Mutex<HashMap<Endpoint, usize>>> = OnceLock::new();
    IN_USE.get_or_init(Default::default)
}

/// Held while a session is connected to a source: its address counts as
/// there and is not checked.
pub struct InUse(Option<Endpoint>);

impl Drop for InUse {
    fn drop(&mut self) {
        let Some(endpoint) = self.0.take() else { return };
        let mut all = in_use_counts().lock().unwrap();
        if let Some(count) = all.get_mut(&endpoint) {
            *count -= 1;
            if *count == 0 {
                all.remove(&endpoint);
            }
        }
    }
}

/// Says that a session is connected to `target` until the guard is dropped.
pub fn in_use(target: &Target, description: Option<&Description>) -> InUse {
    let endpoint = endpoint(target, description);
    if let Some(endpoint) = &endpoint {
        *in_use_counts().lock().unwrap().entry(endpoint.clone()).or_insert(0) += 1;
    }
    InUse(endpoint)
}

fn is_in_use(endpoint: &Endpoint) -> bool {
    in_use_counts().lock().unwrap().contains_key(endpoint)
}

// ---- The monitor ----

/// Checks `endpoints`, [`AT_ONCE`] at a time, with `check`; those in use
/// are there without a check.
pub async fn check_all<F, Fut>(endpoints: HashSet<Endpoint>, check: F) -> HashMap<Endpoint, bool>
where
    F: Fn(Endpoint) -> Fut,
    Fut: std::future::Future<Output = bool>,
{
    use futures_util::StreamExt;
    let (used, unused): (Vec<Endpoint>, Vec<Endpoint>) = endpoints.into_iter().partition(is_in_use);
    let mut known: HashMap<Endpoint, bool> = used.into_iter().map(|endpoint| (endpoint, true)).collect();
    let checked: Vec<(Endpoint, bool)> = futures_util::stream::iter(unused)
        .map(|endpoint| {
            let checking = check(endpoint.clone());
            async move { (endpoint, checking.await) }
        })
        .buffer_unordered(AT_ONCE)
        .collect()
        .await;
    known.extend(checked);
    known
}

/// What loads the sources to check, and the connectors there are.
pub type Load = Box<dyn Fn() -> (Vec<SourceConfig>, Vec<Description>) + Send + Sync>;
/// Called once after a round in which what is known changed.
pub type OnChange = Box<dyn Fn() + Send + Sync>;

struct Shared {
    known: Mutex<HashMap<Endpoint, bool>>,
    kick: tokio::sync::Notify,
}

/// Checks the sources that `load` gives in a thread of its own, as long as
/// the program runs.
#[derive(Clone)]
pub struct Monitor(Arc<Shared>);

impl Monitor {
    pub fn start(load: Load, on_change: OnChange) -> Monitor {
        Self::start_with(load, on_change, INTERVAL, TIMEOUT)
    }

    pub fn start_with(load: Load, on_change: OnChange, interval: Duration, timeout: Duration) -> Monitor {
        let shared = Arc::new(Shared { known: Mutex::new(HashMap::new()), kick: tokio::sync::Notify::new() });
        let monitor = Monitor(shared.clone());
        let started = std::thread::Builder::new().name("source-checks".into()).spawn(move || {
            let runtime = match tokio::runtime::Builder::new_current_thread().enable_all().build() {
                Ok(runtime) => runtime,
                Err(e) => {
                    tracing::warn!("Source checks cannot run: {}", e);
                    return;
                }
            };
            runtime.block_on(run(shared, load, on_change, interval, timeout));
        });
        if let Err(e) = started {
            tracing::warn!("Source checks cannot run: {}", e);
        }
        monitor
    }

    /// Whether the source `target` is there: `Some(true)` when it answered
    /// the last check or a session uses it, `Some(false)` when it did not,
    /// `None` when it has not been checked (yet) or cannot be. A target not
    /// checked yet is checked at once.
    pub fn status(&self, target: &Target, description: Option<&Description>) -> Option<bool> {
        let endpoint = endpoint(target, description)?;
        if is_in_use(&endpoint) {
            return Some(true);
        }
        let known = self.0.known.lock().unwrap().get(&endpoint).copied();
        if known.is_none() {
            self.0.kick.notify_one();
        }
        known
    }

    /// Checks again now (the sources were changed).
    pub fn kick(&self) {
        self.0.kick.notify_one();
    }
}

async fn run(shared: Arc<Shared>, load: Load, on_change: OnChange, interval: Duration, timeout: Duration) {
    loop {
        let (sources, connectors) = load();
        let endpoints: HashSet<Endpoint> = sources
            .iter()
            .filter_map(|source| endpoint(&source.target, connectors.iter().find(|c| c.id == source.target.connector)))
            .collect();
        let known = check_all(endpoints, |endpoint| async move { probe(&endpoint, timeout).await }).await;
        let changed = {
            let mut previous = shared.known.lock().unwrap();
            let changed = *previous != known;
            if changed {
                let there = known.values().filter(|there| **there).count();
                info!("Sources: {} of {} address(es) answer", there, known.len());
            }
            *previous = known;
            changed
        };
        if changed {
            on_change();
        }
        let jitter = Duration::from_millis(std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|t| t.subsec_nanos() as u64).unwrap_or(0) % (JITTER_MS + 1));
        let wait = if interval >= Duration::from_secs(5) { interval + jitter } else { interval };
        tokio::select! {
            _ = tokio::time::sleep(wait) => {}
            _ = shared.kick.notified() => {}
        }
    }
}

// ---- This agent's monitor ----

/// The monitor of this agent's sources (its config file), started the
/// first time it is asked for; also in the bridge-only agent (the server
/// gateway: the server shows whether its computers are there). Not in unit
/// tests (they must not reach out to the addresses in a config).
pub fn agent_monitor() -> Option<&'static Monitor> {
    static MONITOR: OnceLock<Option<Monitor>> = OnceLock::new();
    MONITOR
        .get_or_init(|| {
            if cfg!(test) || std::env::var_os("LUNARIS_NO_SOURCE_CHECKS").is_some() {
                return None;
            }
            Some(Monitor::start(
                Box::new(|| (super::configured_sources(), super::available(&super::connectors_dir()))),
                Box::new(super::sources::note_sources_changed),
            ))
        })
        .as_ref()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::connector::Field;
    use std::collections::BTreeMap;
    use std::time::Instant;

    fn target(connector: &str, values: &[(&str, &str)]) -> Target {
        Target { connector: connector.into(), values: values.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect::<BTreeMap<_, _>>() }
    }

    fn described(id: &str, default_port: &str) -> Description {
        let field = |key: &str, default: &str| Field { key: key.into(), label: key.into(), kind: "text".into(), default: default.into(), required: false, options: Vec::new() };
        Description { id: id.into(), name: id.into(), fields: vec![field("host", ""), field("port", default_port)], capabilities: Default::default(), note: String::new() }
    }

    fn at(port: u16) -> Endpoint {
        Endpoint { host: "127.0.0.1".into(), port }
    }

    /// A port nothing listens on (just closed).
    fn closed_port() -> u16 {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        listener.local_addr().unwrap().port()
    }

    #[test]
    fn where_a_source_is_checked() {
        let rustdesk = described("rustdesk", "21118");
        assert_eq!(endpoint(&target("rustdesk", &[("host", " 10.0.0.5 ")]), Some(&rustdesk)), Some(Endpoint { host: "10.0.0.5".into(), port: 21118 }));
        assert_eq!(endpoint(&target("rustdesk", &[("host", "Den-PC"), ("port", "4000")]), Some(&rustdesk)), Some(Endpoint { host: "den-pc".into(), port: 4000 }));
        assert_eq!(endpoint(&target("x", &[("address", "[::1]"), ("port", "5900")]), None), Some(Endpoint { host: "::1".into(), port: 5900 }));
        // Unknown: no address, no port and no connector to say one, a port that is none.
        assert_eq!(endpoint(&target("rustdesk", &[("port", "21118")]), Some(&rustdesk)), None);
        assert_eq!(endpoint(&target("ext", &[("host", "a")]), None), None);
        assert_eq!(endpoint(&target("ext", &[("host", "a")]), Some(&described("ext", ""))), None);
        assert_eq!(endpoint(&target("rustdesk", &[("host", "a"), ("port", "0")]), Some(&rustdesk)), None);
        assert_eq!(endpoint(&target("rustdesk", &[("host", "a"), ("port", "x")]), Some(&rustdesk)), None);
    }

    #[test]
    fn the_built_in_connectors_say_their_ports() {
        let all = crate::connector::available(std::path::Path::new("/nonexistent-lunaris-connectors"));
        let port = |id: &str| endpoint(&target(id, &[("host", "h")]), all.iter().find(|d| d.id == id)).map(|e| e.port);
        assert_eq!(port("rustdesk"), Some(21118));
        assert_eq!(port("sunshine"), Some(47989));
        assert_eq!(port("rdp"), Some(3389));
        assert_eq!(port("vnc"), Some(5900));
    }

    #[tokio::test]
    async fn an_open_port_answers_and_a_closed_one_does_not() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let open = listener.local_addr().unwrap().port();
        assert!(probe(&at(open), TIMEOUT).await);
        assert!(!probe(&at(closed_port()), TIMEOUT).await);
    }

    #[tokio::test]
    async fn a_computer_that_does_not_answer_times_out() {
        // A listener whose queue is full: the kernel drops further
        // connections without an answer, as a computer that is off does.
        let socket = tokio::net::TcpSocket::new_v4().unwrap();
        socket.bind("127.0.0.1:0".parse().unwrap()).unwrap();
        let port = socket.local_addr().unwrap().port();
        let _listener = socket.listen(1).unwrap();
        let mut held = Vec::new();
        let mut full = false;
        for _ in 0..32 {
            match tokio::time::timeout(Duration::from_millis(300), tokio::net::TcpStream::connect(("127.0.0.1", port))).await {
                Ok(Ok(stream)) => held.push(stream),
                _ => {
                    full = true;
                    break;
                }
            }
        }
        assert!(full, "the listener's queue did not fill");
        let started = Instant::now();
        assert!(!probe(&at(port), Duration::from_millis(400)).await);
        let took = started.elapsed();
        assert!(took >= Duration::from_millis(350) && took < Duration::from_secs(2), "{:?}", took);
    }

    #[tokio::test]
    async fn an_address_in_use_is_not_checked() {
        let port = closed_port();
        let used = target("rustdesk", &[("host", "127.0.0.1"), ("port", &port.to_string())]);
        let guard = in_use(&used, None);
        let checked = Arc::new(Mutex::new(Vec::new()));
        let seen = checked.clone();
        let other = at(closed_port());
        let known = check_all([at(port), other.clone()].into_iter().collect(), move |endpoint| {
            seen.lock().unwrap().push(endpoint);
            async { false }
        })
        .await;
        assert_eq!(known.get(&at(port)), Some(&true));
        assert_eq!(known.get(&other), Some(&false));
        assert_eq!(*checked.lock().unwrap(), vec![other]);
        drop(guard);
        assert!(!is_in_use(&at(port)));
    }

    #[test]
    fn the_monitor_tells_what_answers_and_when_it_changes() {
        let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        let listener = runtime.block_on(tokio::net::TcpListener::bind("127.0.0.1:0")).unwrap();
        let open_port = listener.local_addr().unwrap().port();
        let shut_port = closed_port();
        let source = |id: &str, port: u16| SourceConfig { id: id.into(), name: id.into(), group: None, target: target("rustdesk", &[("host", "127.0.0.1"), ("port", &port.to_string())]) };
        let sources = vec![source("on", open_port), source("off", shut_port)];
        let loaded = sources.clone();
        let changes = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let counted = changes.clone();
        let monitor = Monitor::start_with(
            Box::new(move || (loaded.clone(), Vec::new())),
            Box::new(move || {
                counted.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            }),
            Duration::from_millis(100),
            Duration::from_millis(500),
        );
        let deadline = Instant::now() + Duration::from_secs(10);
        while monitor.status(&sources[1].target, None).is_none() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
        }
        assert_eq!(monitor.status(&sources[0].target, None), Some(true));
        assert_eq!(monitor.status(&sources[1].target, None), Some(false));
        // Nothing to check: unknown.
        assert_eq!(monitor.status(&target("ext", &[("host", "127.0.0.1")]), None), None);
        // The first round changed what is known; the rounds after it did not.
        std::thread::sleep(Duration::from_millis(500));
        assert_eq!(changes.load(std::sync::atomic::Ordering::SeqCst), 1);
        // The computer goes away: the next round says so, once.
        drop(listener);
        let deadline = Instant::now() + Duration::from_secs(10);
        while monitor.status(&sources[0].target, None) != Some(false) && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
        }
        assert_eq!(monitor.status(&sources[0].target, None), Some(false));
        std::thread::sleep(Duration::from_millis(300));
        assert_eq!(changes.load(std::sync::atomic::Ordering::SeqCst), 2);
    }
}
