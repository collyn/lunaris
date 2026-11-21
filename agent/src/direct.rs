//! Direct connections: clients connect to the agent itself, without a Lunaris
//! server, at `wss://<agent address>:<direct_port>/ws/client`.
//!
//! The endpoint speaks the server's client protocol (`ClientMessage` in,
//! `ServerToClientMessage` out), so the clients only change the address they
//! connect to. Only signaling goes through this socket; video, audio and input
//! still use WebRTC, which now finds the agent at the address the client
//! already knows. Meant for LAN, VPN or a forwarded port.
//!
//! The port speaks TLS with a self-signed certificate made at start, and the
//! password never crosses the network: the client proves it knows it with a
//! MAC bound to that certificate (`handshake.rs`). Clients from before that
//! still connect over plain `ws://...?token=<password>` on the same port (the
//! first byte tells them apart).
//!
//! Without the password (`approve_mode` "password-click" or "click", like
//! RustDesk), a client can still connect, and the person at this computer is
//! asked to accept it when it wants to see the screen or transfer files
//! (`approval.rs`), once per kind of access; the client hears
//! `AccessRequest` "waiting", then the answer.
//!
//! A Lunaris server that has this computer added by address connects to
//! `/ws/server` (same password rules) and is spoken to like the server the
//! agent itself connects to (`AgentMessage` out, `ServerToAgentMessage` in).

pub mod handshake;

use std::collections::{HashMap, HashSet};
use std::hash::Hash;
use std::net::{IpAddr, Ipv6Addr, SocketAddr};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use common::{AgentMessage, ClientMessage, ServerToAgentMessage, ServerToClientMessage, SignalingMessage};
use futures_util::{SinkExt, StreamExt};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::handshake::server::{ErrorResponse, Request, Response};
use tokio_tungstenite::tungstenite::http::StatusCode;
use tokio_tungstenite::tungstenite::Message as WsMessage;
use tokio_tungstenite::WebSocketStream;
use tracing::{debug, error, info, warn};

use crate::approval::{self, ApproveMode, Decision, Kind};
use crate::hub::{from_direct_client, link_client_id, target_of, with_target, Hub};
use handshake::Presented;

pub use handshake::{connect, ConnectError, Connected};

/// Failed logins allowed per address before it is locked out for a while.
const MAX_FAILED_LOGINS: u32 = 5;
const LOCKOUT: Duration = Duration::from_secs(60);
/// What a failure costs toward MAX_FAILED_LOGINS, in half failures: a wrong
/// password or a refusal by the person here a whole one, a question nobody
/// answered (it timed out) half of one.
const FULL_FAILURE: u32 = 2;
const HALF_FAILURE: u32 = 1;
/// Failures from all addresses together allowed per GLOBAL_WINDOW (direct
/// connections and RustDesk logins of this process); beyond it only the
/// addresses that logged in with the password lately (KNOWN_GOOD) may try,
/// so guessing from many addresses at once gets no further than from one.
const GLOBAL_MAX_FAILURES: usize = 30;
const GLOBAL_WINDOW: Duration = Duration::from_secs(60);
const KNOWN_GOOD: Duration = Duration::from_secs(24 * 3600);
const MAX_KNOWN_GOOD: usize = 256;
const PING_INTERVAL: Duration = Duration::from_secs(15);
/// TLS, WebSocket upgrade and password proof together.
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);
/// Connections not (yet) let in by the password or the person here.
const MAX_UNAUTH_PER_ADDR: usize = 8;
const MAX_UNAUTH_TOTAL: usize = 64;
/// Questions on screen at once: one per address, a few in all.
const MAX_PROMPTS: usize = 3;
/// After a question from an address was refused or not answered, the next
/// one from there waits this long, doubled for each further one (up to
/// LOCKOUT), so a few addresses cannot keep the screen full of questions.
const PROMPT_BACKOFF: Duration = Duration::from_secs(5);
/// Addresses remembered for lockouts and query limits.
const MAX_TRACKED_ADDRS: usize = 4096;
/// Capability and app-list queries before being let in: one of each per
/// this interval, per connection and per address.
const QUERY_INTERVAL: Duration = Duration::from_secs(2);
const MAX_NAME_CHARS: usize = 64;
/// The largest WebSocket message (and frame) a peer of the direct listener
/// may send. tungstenite keeps the limit it is given at the upgrade for the
/// life of the connection, so it must fit what comes after the password
/// too: signaling (an SDP answer is a few KB, ICE candidates, queries) and,
/// over `/ws/server`, settings and source changes (a form's values), all far
/// below this. Before authentication only the small handshake messages of
/// handshake.rs come; this bounds what 64 connections not let in can make
/// the agent hold (tungstenite's own default is 64 MiB per message).
const MAX_WS_MESSAGE: usize = 256 * 1024;

/// The WebSocket limits of the direct listener (MAX_WS_MESSAGE).
fn ws_config() -> tokio_tungstenite::tungstenite::protocol::WebSocketConfig {
    tokio_tungstenite::tungstenite::protocol::WebSocketConfig::default()
        .max_message_size(Some(MAX_WS_MESSAGE))
        .max_frame_size(Some(MAX_WS_MESSAGE))
}

/// The address limits are kept by: IPv4 as is, IPv6 by its /64 (one host
/// usually owns a whole /64).
fn addr_key(ip: IpAddr) -> IpAddr {
    match ip {
        IpAddr::V4(_) => ip,
        IpAddr::V6(v6) => match v6.to_ipv4_mapped() {
            Some(v4) => IpAddr::V4(v4),
            None => {
                let s = v6.segments();
                IpAddr::V6(Ipv6Addr::new(s[0], s[1], s[2], s[3], 0, 0, 0, 0))
            }
        },
    }
}

/// Failed attempts (wrong passwords, refused or unanswered questions) per
/// client address: (cost in half failures, time of the last one); and of
/// all addresses together (`recent`).
#[derive(Default)]
pub(crate) struct LoginGuard {
    failures: HashMap<IpAddr, (u32, Instant)>,
    /// The times of the last GLOBAL_MAX_FAILURES failures, oldest first.
    recent: std::collections::VecDeque<Instant>,
    /// Addresses that logged in with the password, and when.
    known_good: HashMap<IpAddr, Instant>,
}

impl LoginGuard {
    /// This address is locked out by its own failures.
    fn blocked(&self, key: &IpAddr) -> bool {
        matches!(self.failures.get(key), Some((cost, last)) if last.elapsed() < LOCKOUT && *cost >= MAX_FAILED_LOGINS * FULL_FAILURE)
    }

    /// Too many failures from everyone in the last GLOBAL_WINDOW.
    fn global_exhausted(&self) -> bool {
        self.recent.len() >= GLOBAL_MAX_FAILURES && self.recent.front().is_some_and(|t| t.elapsed() < GLOBAL_WINDOW)
    }

    fn known(&self, key: &IpAddr) -> bool {
        self.known_good.get(key).is_some_and(|t| t.elapsed() < KNOWN_GOOD)
    }

    pub(crate) fn locked_out(&mut self, ip: IpAddr) -> bool {
        let key = addr_key(ip);
        if self.blocked(&key) {
            return true;
        }
        if self.failures.get(&key).is_some_and(|(_, last)| last.elapsed() >= LOCKOUT) {
            self.failures.remove(&key);
        }
        self.global_exhausted() && !self.known(&key)
    }

    /// A wrong password (or a refusal by the person here).
    pub(crate) fn failed(&mut self, ip: IpAddr) {
        self.add_failure(ip, FULL_FAILURE);
    }

    /// A question nobody answered: half a failure.
    pub(crate) fn timed_out(&mut self, ip: IpAddr) {
        self.add_failure(ip, HALF_FAILURE);
    }

    fn add_failure(&mut self, ip: IpAddr, cost: u32) {
        let now = Instant::now();
        self.recent.push_back(now);
        while self.recent.len() > GLOBAL_MAX_FAILURES {
            self.recent.pop_front();
        }
        let key = addr_key(ip);
        if !self.failures.contains_key(&key) && self.failures.len() >= MAX_TRACKED_ADDRS {
            self.failures.retain(|_, (_, last)| last.elapsed() < LOCKOUT);
            if self.failures.len() >= MAX_TRACKED_ADDRS {
                // Still full of fresh entries: forget the oldest one that
                // is not locked out (a lockout is never undone by others'
                // failures). All locked out: this address is not
                // remembered, the global budget still counts it.
                let oldest = self
                    .failures
                    .iter()
                    .filter(|(k, _)| !self.blocked(k))
                    .min_by_key(|(_, (_, last))| *last)
                    .map(|(k, _)| *k);
                match oldest {
                    Some(oldest) => {
                        self.failures.remove(&oldest);
                    }
                    None => return,
                }
            }
        }
        let entry = self.failures.entry(key).or_insert((0, now));
        if entry.1.elapsed() >= LOCKOUT {
            entry.0 = 0;
        }
        entry.0 += cost;
        entry.1 = now;
    }

    /// Takes back one failure counted in advance (`handshake::Gate::charge`)
    /// for `ip`, before the attempt is judged for what it was.
    fn refund(&mut self, ip: IpAddr) {
        self.recent.pop_back();
        let key = addr_key(ip);
        if let Some(entry) = self.failures.get_mut(&key) {
            entry.0 = entry.0.saturating_sub(FULL_FAILURE);
            if entry.0 == 0 {
                self.failures.remove(&key);
            }
        }
    }

    pub(crate) fn succeeded(&mut self, ip: IpAddr) {
        let key = addr_key(ip);
        self.failures.remove(&key);
        if !self.known_good.contains_key(&key) && self.known_good.len() >= MAX_KNOWN_GOOD {
            if let Some(oldest) = self.known_good.iter().min_by_key(|(_, t)| **t).map(|(k, _)| *k) {
                self.known_good.remove(&oldest);
            }
        }
        self.known_good.insert(key, Instant::now());
    }
}

/// Everything the listeners limit across connections: the direct listener
/// and the RustDesk one share it (`process_limits`), so the password gets
/// one budget of guesses per address, not one per protocol.
#[derive(Default)]
pub(crate) struct Limits {
    pub(crate) logins: LoginGuard,
    /// Connections not let in yet, per address.
    unauth: HashMap<IpAddr, usize>,
    unauth_total: usize,
    /// Addresses with a question on screen.
    prompts: HashSet<IpAddr>,
    /// Questions refused or not answered lately, per address: (how many,
    /// when the last one ended).
    prompt_strikes: HashMap<IpAddr, (u32, Instant)>,
    /// Last capability/app-list query per (address, query kind).
    queries: HashMap<(IpAddr, u8), Instant>,
}

/// How long an address waits for its next question after `strikes`
/// refused or unanswered ones.
fn prompt_backoff(strikes: u32) -> Duration {
    if strikes == 0 {
        return Duration::ZERO;
    }
    PROMPT_BACKOFF.saturating_mul(1 << (strikes - 1).min(8)).min(LOCKOUT)
}

impl Limits {
    fn open_unauth(&mut self, ip: IpAddr) -> bool {
        let key = addr_key(ip);
        let n = self.unauth.get(&key).copied().unwrap_or(0);
        if n >= MAX_UNAUTH_PER_ADDR || self.unauth_total >= MAX_UNAUTH_TOTAL {
            return false;
        }
        *self.unauth.entry(key).or_insert(0) += 1;
        self.unauth_total += 1;
        true
    }

    fn close_unauth(&mut self, ip: IpAddr) {
        let key = addr_key(ip);
        if let Some(n) = self.unauth.get_mut(&key) {
            *n -= 1;
            if *n == 0 {
                self.unauth.remove(&key);
            }
        }
        self.unauth_total = self.unauth_total.saturating_sub(1);
    }

    /// One question at a time per address, MAX_PROMPTS in all, none from an
    /// address that is locked out or waiting after refused ones.
    fn start_prompt(&mut self, ip: IpAddr) -> bool {
        let key = addr_key(ip);
        if self.logins.locked_out(ip) {
            return false;
        }
        if let Some((strikes, last)) = self.prompt_strikes.get(&key).copied() {
            if last.elapsed() >= LOCKOUT.max(prompt_backoff(strikes)) {
                self.prompt_strikes.remove(&key);
            } else if last.elapsed() < prompt_backoff(strikes) {
                return false;
            }
        }
        self.prompts.len() < MAX_PROMPTS && self.prompts.insert(key)
    }

    fn end_prompt(&mut self, ip: IpAddr) {
        self.prompts.remove(&addr_key(ip));
    }

    /// The person here answered a question from `ip` (or it timed out): a
    /// refusal counts like a wrong password, no answer like half of one,
    /// and either makes the next question from there wait.
    pub(crate) fn prompt_answered(&mut self, ip: IpAddr, decision: Decision) {
        let key = addr_key(ip);
        match decision {
            Decision::Accepted => {
                self.prompt_strikes.remove(&key);
                return;
            }
            Decision::Denied => self.logins.failed(ip),
            Decision::Timeout => self.logins.timed_out(ip),
            Decision::Unattended => return,
        }
        if !self.prompt_strikes.contains_key(&key) && self.prompt_strikes.len() >= MAX_TRACKED_ADDRS {
            self.prompt_strikes.retain(|_, (strikes, last)| last.elapsed() < prompt_backoff(*strikes).max(LOCKOUT));
            if self.prompt_strikes.len() >= MAX_TRACKED_ADDRS {
                return;
            }
        }
        let entry = self.prompt_strikes.entry(key).or_insert((0, Instant::now()));
        entry.0 = entry.0.saturating_add(1);
        entry.1 = Instant::now();
    }

    fn allow_query(&mut self, ip: IpAddr, kind: u8, now: Instant) -> bool {
        allow_at(&mut self.queries, (addr_key(ip), kind), now)
    }
}

/// Rate limit: true (and remembered) when `key` was last allowed at least
/// QUERY_INTERVAL ago. The map is kept under MAX_TRACKED_ADDRS entries.
fn allow_at<K: Hash + Eq>(map: &mut HashMap<K, Instant>, key: K, now: Instant) -> bool {
    if let Some(last) = map.get(&key) {
        if now.saturating_duration_since(*last) < QUERY_INTERVAL {
            return false;
        }
    } else if map.len() >= MAX_TRACKED_ADDRS {
        map.retain(|_, last| now.saturating_duration_since(*last) < QUERY_INTERVAL);
        if map.len() >= MAX_TRACKED_ADDRS {
            return false;
        }
    }
    map.insert(key, now);
    true
}

/// Counts a connection as not let in until dropped or `authenticated`.
pub(crate) struct UnauthSlot {
    limits: Arc<Mutex<Limits>>,
    ip: IpAddr,
}

impl UnauthSlot {
    pub(crate) fn take(limits: &Arc<Mutex<Limits>>, ip: IpAddr) -> Option<Self> {
        limits.lock().unwrap().open_unauth(ip).then(|| UnauthSlot { limits: limits.clone(), ip })
    }
}

impl Drop for UnauthSlot {
    fn drop(&mut self) {
        self.limits.lock().unwrap().close_unauth(self.ip);
    }
}

/// A question on screen, for the limits.
pub(crate) struct PromptSlot {
    limits: Arc<Mutex<Limits>>,
    ip: IpAddr,
}

impl PromptSlot {
    pub(crate) fn take(limits: &Arc<Mutex<Limits>>, ip: IpAddr) -> Option<Self> {
        limits.lock().unwrap().start_prompt(ip).then(|| PromptSlot { limits: limits.clone(), ip })
    }
}

impl Drop for PromptSlot {
    fn drop(&mut self) {
        self.limits.lock().unwrap().end_prompt(self.ip);
    }
}

/// The limits of the direct listeners of this process: kept while the
/// listener starts again with new settings (and while the agent is
/// restarted by its window), so a lockout for wrong passwords is not undone
/// by a restart.
pub(crate) fn process_limits() -> Arc<Mutex<Limits>> {
    static LIMITS: std::sync::OnceLock<Arc<Mutex<Limits>>> = std::sync::OnceLock::new();
    LIMITS.get_or_init(Arc::default).clone()
}

/// The Lunaris servers connected to this agent by its address
/// (`/ws/server`) during one run of the agent (`run_agent`, which closes
/// them all when it stops: `close_all`). They outlive the listener when it
/// starts again with new settings, unless those would not let them in any
/// more (`keep_admitted`).
#[derive(Clone, Default)]
pub struct Links(Arc<Mutex<LinkTasks>>);

#[derive(Default)]
struct LinkTasks {
    /// Let in with the access password.
    trusted: tokio::task::JoinSet<()>,
    /// Let in without it: their clients are accepted by the person here.
    untrusted: tokio::task::JoinSet<()>,
}

impl Links {
    fn spawn(&self, trusted: bool, link: impl std::future::Future<Output = ()> + Send + 'static) {
        let mut tasks = self.0.lock().unwrap();
        while tasks.trusted.try_join_next().is_some() {}
        while tasks.untrusted.try_join_next().is_some() {}
        if trusted {
            tasks.trusted.spawn(link);
        } else {
            tasks.untrusted.spawn(link);
        }
    }

    /// After a change of the settings: closes the links the listener would
    /// not let in now. One that proved the old password is closed when the
    /// password changed (its server connects again and must prove the new
    /// one; the change itself never comes over a link, settings.rs), or when
    /// the password no longer lets anyone in ("click"); one let in without
    /// it when nobody may be let in that way any more ("password"). All of
    /// them when direct access is turned off.
    pub fn keep_admitted(&self, direct_enabled: bool, password_changed: bool, mode: ApproveMode) -> usize {
        let keep_trusted = direct_enabled && !password_changed && mode != ApproveMode::Click;
        let keep_untrusted = direct_enabled && mode != ApproveMode::Password;
        let mut tasks = self.0.lock().unwrap();
        let mut closed = 0;
        if !keep_trusted {
            closed += tasks.trusted.len();
            tasks.trusted.abort_all();
        }
        if !keep_untrusted {
            closed += tasks.untrusted.len();
            tasks.untrusted.abort_all();
        }
        closed
    }

    /// Closes every link (the agent stops).
    pub fn close_all(&self) {
        let mut tasks = self.0.lock().unwrap();
        tasks.trusted.abort_all();
        tasks.untrusted.abort_all();
    }

    #[cfg(test)]
    fn open(&self) -> usize {
        let mut tasks = self.0.lock().unwrap();
        while tasks.trusted.try_join_next().is_some() {}
        while tasks.untrusted.try_join_next().is_some() {}
        tasks.trusted.len() + tasks.untrusted.len()
    }
}

/// Compares in constant time: both sides are hashed first, so the lengths
/// compared are always the same.
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    handshake::digest_eq(&handshake::sha256(a), &handshake::sha256(b))
}

fn query_param(query: &str, name: &str) -> Option<String> {
    query.split('&').find_map(|pair| {
        let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
        (key == name).then(|| {
            urlencoding::decode(&value.replace('+', " "))
                .map(|v| v.into_owned())
                .unwrap_or_default()
        })
    })
}

/// A name a peer gives itself, safe to show: no control or direction
/// characters (which could fake the rest of the question), trimmed, short.
pub(crate) fn clean_name(name: &str) -> String {
    let bidi = |c: char| matches!(c, '\u{200E}' | '\u{200F}' | '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}');
    let cleaned: String = name.chars().filter(|&c| !c.is_control() && !bidi(c)).take(MAX_NAME_CHARS).collect();
    cleaned.trim().to_string()
}

/// Who is asking, as shown to the person here: the address it really comes
/// from, and the name it gave only marked as its claim.
pub(crate) fn requester_label(claimed: &str, ip: IpAddr, via_server: bool) -> String {
    let name = clean_name(claimed);
    match (name.is_empty(), via_server) {
        (true, false) => ip.to_string(),
        (true, true) => format!("a client via the server at {}", ip),
        (false, false) => format!("\"{}\" (unverified name) from {}", name, ip),
        (false, true) => format!("\"{}\" (unverified name) via the server at {}", name, ip),
    }
}

#[derive(Debug, PartialEq)]
enum Admission {
    /// `trusted`: the right password (no need to ask the person here);
    /// `link`: a Lunaris server connecting to this agent (`/ws/server`).
    Accept { trusted: bool, link: bool },
    NotFound,
    Unauthorized,
    TooManyAttempts,
}

fn link_path(path: &str) -> Option<bool> {
    match path.trim_end_matches('/') {
        "/ws/client" => Some(false),
        "/ws/server" => Some(true),
        _ => None,
    }
}

/// The decision for what a peer showed of the password, with failures
/// counted toward the lockout.
fn judge(presented: Presented, mode: ApproveMode, link: bool, ip: IpAddr, guard: &mut LoginGuard) -> Admission {
    if guard.locked_out(ip) {
        return Admission::TooManyAttempts;
    }
    match mode {
        // Everyone is asked; the password is not used.
        ApproveMode::Click => Admission::Accept { trusted: false, link },
        _ if presented == Presented::Right => {
            guard.succeeded(ip);
            Admission::Accept { trusted: true, link }
        }
        // No password given: the person here may accept.
        ApproveMode::PasswordClick if presented == Presented::Nothing => Admission::Accept { trusted: false, link },
        _ => {
            guard.failed(ip);
            Admission::Unauthorized
        }
    }
}

/// The lockout around the TLS handshake (handshake.rs `agent_auth`).
struct LoginGate<'a> {
    limits: &'a Mutex<Limits>,
    mode: ApproveMode,
    link: bool,
    ip: IpAddr,
}

impl handshake::Gate for LoginGate<'_> {
    fn charge(&mut self) -> bool {
        let logins = &mut self.limits.lock().unwrap().logins;
        if logins.locked_out(self.ip) {
            return false;
        }
        logins.failed(self.ip);
        true
    }

    fn decide(&mut self, presented: Presented, charged: bool) -> Admission {
        let logins = &mut self.limits.lock().unwrap().logins;
        if charged {
            logins.refund(self.ip);
        }
        judge(presented, self.mode, self.link, self.ip, logins)
    }
}

/// Clients from before TLS: the password in the URL (`?token=`).
fn admit(path: &str, query: &str, password: &str, mode: ApproveMode, ip: IpAddr, limits: &Mutex<Limits>) -> Admission {
    let Some(link) = link_path(path) else { return Admission::NotFound };
    let token = query_param(query, "token").unwrap_or_default();
    let token = token.trim();
    let presented = if token.is_empty() {
        Presented::Nothing
    } else if !password.trim().is_empty() && constant_time_eq(token.as_bytes(), password.trim().as_bytes()) {
        Presented::Right
    } else {
        Presented::Wrong
    };
    judge(presented, mode, link, ip, &mut limits.lock().unwrap().logins)
}

/// What the person here is asked about, for a request; None for messages
/// that need no permission.
fn approval_kind(sig: &SignalingMessage) -> Option<Kind> {
    match sig {
        SignalingMessage::RequestSession { .. } | SignalingMessage::IncomingSession { .. } => Some(Kind::Control),
        SignalingMessage::RequestFileSession { .. } | SignalingMessage::IncomingFileSession { .. } => Some(Kind::Files),
        _ => None,
    }
}

/// Messages anyone connected may send before being accepted: questions about
/// the computer, nothing that shows or changes it. Rate-limited (`query_kind`).
#[cfg(test)]
fn allowed_before_approval(sig: &SignalingMessage) -> bool {
    query_kind(sig).is_some()
}

/// Which rate limit a pre-approval query falls under; None for messages
/// not allowed before approval.
fn query_kind(sig: &SignalingMessage) -> Option<u8> {
    match sig {
        SignalingMessage::GetCapabilities { .. } => Some(0),
        SignalingMessage::GetAppList { .. } => Some(1),
        _ => None,
    }
}

/// The kinds of access the person here granted one client.
#[derive(Debug, Default, Clone, Copy, PartialEq)]
struct Kinds(u8);

impl Kinds {
    fn bit(kind: Kind) -> u8 {
        match kind {
            Kind::Control => 1,
            Kind::Files => 2,
        }
    }
    fn has(self, kind: Kind) -> bool {
        self.0 & Self::bit(kind) != 0
    }
    fn add(&mut self, kind: Kind) {
        self.0 |= Self::bit(kind);
    }
    fn any(self) -> bool {
        self.0 != 0
    }
    /// Whether a message that is not a request may pass: anything once some
    /// access was granted, except stopping the stream (anyone's, not only
    /// this client's), which needs screen control.
    fn covers(self, sig: &SignalingMessage) -> bool {
        match sig {
            SignalingMessage::StopActiveStream { .. } => self.has(Kind::Control),
            _ => self.any(),
        }
    }
}

/// The addresses `refresh_local_addresses` found last.
static KNOWN_ADDRESSES: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());

/// Looks up this computer's addresses again (`local_addresses`), for
/// `known_local_addresses`: done when a connection to a server starts.
pub async fn refresh_local_addresses() {
    let addresses = local_addresses().await;
    *KNOWN_ADDRESSES.lock().unwrap() = addresses;
}

/// This computer's addresses as last looked up (empty before the first).
pub fn known_local_addresses() -> Vec<String> {
    KNOWN_ADDRESSES.lock().unwrap().clone()
}

/// This computer's IPv4 addresses a direct client could use (LAN, VPN),
/// without loopback and link-local ones, sorted.
pub async fn local_addresses() -> Vec<String> {
    let net = webrtc_util::vnet::net::Net::new(None);
    let mut addrs: Vec<String> = net
        .get_interfaces()
        .await
        .iter()
        // Container/VM bridges and tunnels to them are no use to a client.
        .filter(|iface| {
            let name = iface.name();
            !["docker", "br-", "veth", "virbr", "lxcbr", "lxdbr", "podman", "cni", "vmnet", "vboxnet"]
                .iter()
                .any(|prefix| name.starts_with(prefix))
        })
        .flat_map(|iface| iface.addrs().iter().map(|net| net.addr()).collect::<Vec<_>>())
        .filter_map(|addr| match addr {
            IpAddr::V4(v4) if !v4.is_loopback() && !v4.is_link_local() && !v4.is_unspecified() => {
                Some(v4.to_string())
            }
            _ => None,
        })
        .collect();
    addrs.sort();
    addrs.dedup();
    addrs
}

/// Settings of the direct-connection listener.
#[derive(Clone)]
pub struct DirectConfig {
    pub port: u16,
    pub password: String,
    pub approve_mode: ApproveMode,
    /// This computer's name, told to servers that connect (`/ws/server`).
    pub name: String,
    /// Where the servers that connect (`/ws/server`) are kept.
    pub links: Links,
    /// Clients from before TLS may still log in over plain `ws://` with the
    /// password in the URL (`allow_legacy_plain_direct` of the config).
    pub allow_legacy_plain: bool,
    /// Where the TLS certificate is kept across starts (servers pin it);
    /// None: a new one per start (tests, the server gateway's local agent).
    pub tls_dir: Option<std::path::PathBuf>,
}

/// Accepts direct clients on `port` until the task is aborted.
pub async fn run_listener(
    config: DirectConfig,
    hub: Arc<Hub>,
    inbound_tx: mpsc::UnboundedSender<SignalingMessage>,
) -> anyhow::Result<()> {
    // All interfaces on purpose: direct clients come from the LAN or a VPN.
    let listener = TcpListener::bind(("0.0.0.0", config.port)).await?;
    let asker: Asker = Arc::new(|requester, kind| Box::pin(async move { approval::ask(&requester, kind).await }));
    serve_listener(listener, config, hub, inbound_tx, asker, process_limits()).await
}

/// As `run_listener`, on a listener the caller bound (the bridge-only
/// agent's, on 127.0.0.1).
pub(crate) async fn run_listener_on(
    listener: TcpListener,
    config: DirectConfig,
    hub: Arc<Hub>,
    inbound_tx: mpsc::UnboundedSender<SignalingMessage>,
) -> anyhow::Result<()> {
    let asker: Asker = Arc::new(|requester, kind| Box::pin(async move { approval::ask(&requester, kind).await }));
    serve_listener(listener, config, hub, inbound_tx, asker, process_limits()).await
}

/// Asks the person here (approval::ask; replaced in tests).
type Asker = Arc<dyn Fn(String, Kind) -> std::pin::Pin<Box<dyn std::future::Future<Output = Decision> + Send>> + Send + Sync>;

async fn serve_listener(
    listener: TcpListener,
    config: DirectConfig,
    hub: Arc<Hub>,
    inbound_tx: mpsc::UnboundedSender<SignalingMessage>,
    asker: Asker,
    limits: Arc<Mutex<Limits>>,
) -> anyhow::Result<()> {
    let made = match config.tls_dir.as_deref() {
        Some(dir) => handshake::load_or_make_tls_server(dir),
        None => handshake::make_tls_server(),
    };
    let tls = match made {
        Ok(tls) => Some(Arc::new(tls)),
        Err(e) => {
            error!("Direct connections: no TLS certificate ({}); only plain ws:// is offered", e);
            None
        }
    };
    let local = listener.local_addr().map(|a| a.to_string()).unwrap_or_default();
    info!(
        "Direct connections listening on {} ({}, older clients over plain ws://; {:?})",
        local,
        if tls.is_some() { "TLS on" } else { "TLS OFF" },
        config.approve_mode
    );
    let config = Arc::new(config);
    // The connections end with the listener (its settings changed: a client
    // let in with the old password is not kept); a server link goes on
    // (`handle_connection`, `Links`).
    let mut connections = tokio::task::JoinSet::new();
    loop {
        while connections.try_join_next().is_some() {}
        let (stream, peer) = match listener.accept().await {
            Ok(conn) => conn,
            Err(e) => {
                warn!("Direct connection accept failed: {}", e);
                tokio::time::sleep(Duration::from_millis(200)).await;
                continue;
            }
        };
        let Some(slot) = UnauthSlot::take(&limits, peer.ip()) else {
            debug!("Direct connection from {} refused: too many connections not let in yet", peer);
            continue;
        };
        let _ = stream.set_nodelay(true);
        let ctx = Shared {
            config: config.clone(),
            tls: tls.clone(),
            limits: limits.clone(),
            hub: hub.clone(),
            inbound_tx: inbound_tx.clone(),
            asker: asker.clone(),
        };
        connections.spawn(async move {
            if let Err(e) = handle_connection(stream, peer, slot, ctx).await {
                debug!("Direct connection from {} ended: {}", peer, e);
            }
        });
    }
}

/// What every connection of the listener shares.
struct Shared {
    config: Arc<DirectConfig>,
    tls: Option<Arc<handshake::TlsServer>>,
    limits: Arc<Mutex<Limits>>,
    hub: Arc<Hub>,
    inbound_tx: mpsc::UnboundedSender<SignalingMessage>,
    asker: Asker,
}

/// A TLS or plain stream, after the first byte decided which.
trait Io: AsyncRead + AsyncWrite + Unpin + Send {}
impl<T: AsyncRead + AsyncWrite + Unpin + Send> Io for T {}
type Ws = WebSocketStream<Box<dyn Io>>;

/// Writes queued messages to the socket, with pings, until the queue closes.
fn spawn_writer<T: serde::Serialize + Send + 'static>(
    mut ws_write: futures_util::stream::SplitSink<Ws, WsMessage>,
    mut rx: mpsc::UnboundedReceiver<T>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        // First ping after a while: the first message must be the peer's.
        let mut ping = tokio::time::interval_at(tokio::time::Instant::now() + PING_INTERVAL, PING_INTERVAL);
        ping.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tokio::select! {
                msg = rx.recv() => {
                    let Some(msg) = msg else { break };
                    match serde_json::to_string(&msg) {
                        Ok(json) => {
                            if ws_write.send(WsMessage::Text(json.into())).await.is_err() {
                                break;
                            }
                        }
                        Err(e) => error!("Failed to serialize a direct message: {}", e),
                    }
                }
                _ = ping.tick() => {
                    if ws_write.send(WsMessage::Ping(Default::default())).await.is_err() {
                        break;
                    }
                }
            }
        }
        let _ = ws_write.close().await;
    })
}


/// A connection past the handshake, let in or to be asked about.
struct Opened {
    ws: Ws,
    trusted: bool,
    link: bool,
    name: String,
    tls: bool,
}

fn log_refusal(ip: IpAddr, admission: &Admission) {
    match admission {
        Admission::Unauthorized => warn!("Direct connection from {} rejected: wrong password", ip),
        Admission::TooManyAttempts => {
            warn!("Direct connection from {} rejected: too many wrong passwords, try again later", ip)
        }
        _ => {}
    }
}

/// TLS (first byte 0x16, a TLS handshake record) with the password proof,
/// or the old plain WebSocket with `?token=`.
async fn open(stream: TcpStream, ip: IpAddr, ctx: &Shared) -> anyhow::Result<Opened> {
    let mut first = [0u8; 1];
    if stream.peek(&mut first).await? == 0 {
        anyhow::bail!("closed before sending anything");
    }
    let config = &ctx.config;
    let mut name = String::new();
    if first[0] == 0x16 {
        let Some(tls) = ctx.tls.as_ref() else { anyhow::bail!("TLS requested but not available") };
        let tls_stream = tls.acceptor.accept(stream).await?;
        // The password comes after the upgrade; only path and lockout here.
        let mut link = None;
        let mut locked = false;
        let callback = |request: &Request, response: Response| -> Result<Response, ErrorResponse> {
            let uri = request.uri();
            name = query_param(uri.query().unwrap_or(""), "name").unwrap_or_default();
            link = link_path(uri.path());
            locked = ctx.limits.lock().unwrap().logins.locked_out(ip);
            let status = match (link, locked) {
                (Some(_), false) => return Ok(response),
                (None, _) => StatusCode::NOT_FOUND,
                (Some(_), true) => StatusCode::TOO_MANY_REQUESTS,
            };
            let mut error = ErrorResponse::new(Some(status.canonical_reason().unwrap_or("").to_string()));
            *error.status_mut() = status;
            Err(error)
        };
        let result = tokio_tungstenite::accept_hdr_async_with_config(Box::new(tls_stream) as Box<dyn Io>, callback, Some(ws_config())).await;
        let mut ws = match result {
            Ok(ws) => ws,
            Err(e) => {
                if locked {
                    log_refusal(ip, &Admission::TooManyAttempts);
                }
                anyhow::bail!("WebSocket handshake: {}", e);
            }
        };
        let link = link.unwrap_or(false);
        let mut gate = LoginGate { limits: &ctx.limits, mode: config.approve_mode, link, ip };
        let (admission, version) = handshake::agent_auth(&mut ws, &config.password, &tls.cert_hash, &mut gate).await?;
        if version < handshake::VERSION_PAKE && matches!(admission, Admission::Accept { trusted: true, .. }) {
            info!(
                "Direct connection from {} used the old password proof (v1, an older app or server): update it, \
                 an impostor could test the password it sends offline",
                ip
            );
        }
        let Admission::Accept { trusted, link } = admission else {
            log_refusal(ip, &admission);
            let _ = ws.close(None).await;
            anyhow::bail!("refused: {:?}", admission);
        };
        Ok(Opened { ws, trusted, link, name, tls: true })
    } else {
        let mut admission = Admission::NotFound;
        let mut refused_legacy = false;
        let callback = |request: &Request, response: Response| -> Result<Response, ErrorResponse> {
            if !config.allow_legacy_plain {
                // Turned off here: nothing of the request (the password in
                // its URL) is looked at.
                refused_legacy = true;
                let mut error = ErrorResponse::new(Some(
                    "This computer accepts only encrypted connections: update the Lunaris app".to_string(),
                ));
                *error.status_mut() = StatusCode::UPGRADE_REQUIRED;
                return Err(error);
            }
            let uri = request.uri();
            let query = uri.query().unwrap_or("");
            admission = admit(uri.path(), query, &config.password, config.approve_mode, ip, &ctx.limits);
            name = query_param(query, "name").unwrap_or_default();
            let status = match admission {
                Admission::Accept { .. } => return Ok(response),
                Admission::NotFound => StatusCode::NOT_FOUND,
                Admission::Unauthorized => StatusCode::UNAUTHORIZED,
                Admission::TooManyAttempts => StatusCode::TOO_MANY_REQUESTS,
            };
            let mut error = ErrorResponse::new(Some(status.canonical_reason().unwrap_or("").to_string()));
            *error.status_mut() = status;
            Err(error)
        };
        let result = tokio_tungstenite::accept_hdr_async_with_config(Box::new(stream) as Box<dyn Io>, callback, Some(ws_config())).await;
        match (result, admission) {
            (Ok(ws), Admission::Accept { trusted, link }) => Ok(Opened { ws, trusted, link, name, tls: false }),
            (Ok(_), admission) => anyhow::bail!("unexpected admission {:?}", admission),
            (Err(e), admission) => {
                if refused_legacy {
                    warn!("Direct connection from {} refused: plain ws:// (older client) is turned off here (legacy-direct off)", ip);
                }
                log_refusal(ip, &admission);
                anyhow::bail!("WebSocket handshake: {}", e)
            }
        }
    }
}

async fn handle_connection(stream: TcpStream, peer: SocketAddr, slot: UnauthSlot, ctx: Shared) -> anyhow::Result<()> {
    let ip = peer.ip();
    // A connection that never completes the handshake (plain HTTP, a stalled
    // client) must not hold a socket forever.
    let opened = match tokio::time::timeout(HANDSHAKE_TIMEOUT, open(stream, ip, &ctx)).await {
        Ok(opened) => opened?,
        Err(_) => {
            debug!("Direct connection from {} timed out during the handshake", ip);
            return Ok(());
        }
    };
    let Opened { ws, trusted, link, name, tls } = opened;
    if !tls {
        // Its password (if any) crossed the network readable by anyone.
        warn!(
            "Direct connection from {} is a legacy one: plain ws://, password in the URL, unencrypted (older client). \
             Update that app; `lunaris-agent set legacy-direct off` refuses these",
            ip
        );
    }
    let conn = Conn {
        ip,
        requester: requester_label(&name, ip, false),
        trusted,
        // The password let it in: it no longer counts as unauthenticated.
        slot: Mutex::new((!trusted).then_some(slot)),
        limits: ctx.limits,
        asker: ctx.asker,
        hub: ctx.hub,
        inbound_tx: ctx.inbound_tx,
    };
    if link {
        // Not with the listener: a server that connects to this computer
        // stays connected while the listener starts again with new
        // settings (`Links::keep_admitted`), until the agent stops.
        let links = ctx.config.links.clone();
        links.spawn(trusted, serve_link(ws, ctx.config, conn));
    } else {
        serve_client(ws, conn).await;
    }
    Ok(())
}

/// One accepted connection.
struct Conn {
    ip: IpAddr,
    /// Who connected, as shown to the person here.
    requester: String,
    /// Came with the right password: nobody needs to be asked.
    trusted: bool,
    /// Held until the password or the person here let it in.
    slot: Mutex<Option<UnauthSlot>>,
    limits: Arc<Mutex<Limits>>,
    asker: Asker,
    hub: Arc<Hub>,
    inbound_tx: mpsc::UnboundedSender<SignalingMessage>,
}

impl Conn {
    /// Asks the person here; a refusal counts like a wrong password and no
    /// answer like half of one, and the next question from this address
    /// waits (`Limits::prompt_answered`), so a stranger can't keep popping
    /// questions up.
    async fn ask(&self, requester: &str, kind: Kind) -> Decision {
        let decision = (self.asker)(requester.to_string(), kind).await;
        self.limits.lock().unwrap().prompt_answered(self.ip, decision);
        decision
    }

    /// The person here let it in (for something).
    fn let_in(&self) {
        self.slot.lock().unwrap().take();
    }

    /// A capability/app-list query from a peer not let in yet: at most one
    /// of each per QUERY_INTERVAL from this peer (`per_peer`, keyed by `key`)
    /// and from its address.
    fn allow_query<K: Hash + Eq>(&self, per_peer: &mut HashMap<K, Instant>, key: impl FnOnce(u8) -> K, sig: &SignalingMessage) -> bool {
        let Some(kind) = query_kind(sig) else { return false };
        let now = Instant::now();
        allow_at(per_peer, key(kind), now) && self.limits.lock().unwrap().allow_query(self.ip, kind, now)
    }
}

fn access_status(target: &str, status: &str) -> SignalingMessage {
    SignalingMessage::AccessRequest { target_id: target.to_string(), status: status.to_string() }
}

/// Status sent when a question can't be put on screen now.
const STATUS_BUSY: &str = "busy";

/// A client (app, browser) connected straight to this agent.
async fn serve_client(ws: Ws, conn: Conn) {
    let conn_id = format!("direct-{}", uuid::Uuid::new_v4());
    info!("Direct client connected: {} ({}, {})", conn.requester, conn_id,
        if conn.trusted { "password" } else { "to be accepted" });
    let (ws_write, mut ws_read) = ws.split();
    let (tx, rx) = mpsc::unbounded_channel::<ServerToClientMessage>();
    conn.hub.add_direct_client(conn_id.clone(), tx.clone());
    let conn = Arc::new(conn);
    // Also when the listener's task is aborted (new settings, agent stops).
    let _cleanup = ClientGone { conn: conn.clone(), conn_id: conn_id.clone(), writer: spawn_writer(ws_write, rx) };
    // What the person here accepted, per kind, for as long as this connection lasts.
    let accepted = Arc::new(Mutex::new(Kinds::default()));
    let asking = Arc::new(AtomicBool::new(false));
    let mut queries: HashMap<u8, Instant> = HashMap::new();
    let agent_id = conn.hub.agent_id().to_string();

    while let Some(frame) = ws_read.next().await {
        let text = match frame {
            Ok(WsMessage::Text(text)) => text,
            Ok(WsMessage::Close(_)) => break,
            Ok(_) => continue,
            Err(e) => {
                debug!("Direct client {} read error: {}", conn_id, e);
                break;
            }
        };
        let ClientMessage::Signaling(sig) = match serde_json::from_str::<ClientMessage>(&text) {
            Ok(msg) => msg,
            Err(e) => {
                warn!("Ignoring malformed message from direct client {}: {}", conn_id, e);
                continue;
            }
        };
        let Some(sig) = from_direct_client(sig, &conn_id) else { continue };
        let granted = *accepted.lock().unwrap();
        let pass = match approval_kind(&sig) {
            Some(kind) => conn.trusted || granted.has(kind),
            None => conn.trusted || granted.covers(&sig) || conn.allow_query(&mut queries, |k| k, &sig),
        };
        if pass {
            if conn.inbound_tx.send(sig).is_err() {
                break;
            }
            continue;
        }
        let Some(kind) = approval_kind(&sig) else {
            debug!("Direct client {} not accepted yet (or asking too often): dropping {:?}", conn_id, sig);
            continue;
        };
        if asking.swap(true, Ordering::SeqCst) {
            continue;
        }
        let Some(prompt) = PromptSlot::take(&conn.limits, conn.ip) else {
            asking.store(false, Ordering::SeqCst);
            info!("Direct client {} ({}) refused: other questions are already waiting", conn_id, conn.requester);
            let _ = tx.send(ServerToClientMessage::Signaling(access_status(&agent_id, STATUS_BUSY)));
            continue;
        };
        let _ = tx.send(ServerToClientMessage::Signaling(access_status(&agent_id, "waiting")));
        let (conn, accepted, asking, tx) = (conn.clone(), accepted.clone(), asking.clone(), tx.clone());
        let agent_id = agent_id.clone();
        tokio::spawn(async move {
            let decision = conn.ask(&conn.requester, kind).await;
            drop(prompt);
            asking.store(false, Ordering::SeqCst);
            let _ = tx.send(ServerToClientMessage::Signaling(access_status(&agent_id, decision.status())));
            if decision == Decision::Accepted {
                // Only this kind: another one asks again.
                accepted.lock().unwrap().add(kind);
                conn.let_in();
                let _ = conn.inbound_tx.send(sig);
            }
        });
    }

}

/// What ends with a direct client, however it ends.
struct ClientGone {
    conn: Arc<Conn>,
    conn_id: String,
    writer: tokio::task::JoinHandle<()>,
}

impl Drop for ClientGone {
    fn drop(&mut self) {
        info!("Direct client {} disconnected", self.conn_id);
        self.conn.hub.remove_direct_client(&self.conn_id);
        // Like the server on a client disconnect: end that client's session
        // (the session logic ignores this when the client had none).
        let _ = self.conn.inbound_tx.send(SignalingMessage::EndSession { target_id: self.conn_id.clone() });
        self.writer.abort();
    }
}

/// A Lunaris server that connected to this agent (the computer was added to
/// it by address): spoken to exactly like the server the agent connects to,
/// so its clients reach this agent through it. Without the password, each
/// client connection that wants to see the screen or transfer files is
/// accepted by the person here, per kind of access.
async fn serve_link(ws: Ws, config: Arc<DirectConfig>, conn: Conn) {
    let link_id = format!("link-{}", uuid::Uuid::new_v4());
    info!("Lunaris server connected: {} ({}, {})", conn.requester, link_id,
        if conn.trusted { "password" } else { "connections to be accepted" });
    let (ws_write, mut ws_read) = ws.split();
    let (tx, rx) = mpsc::unbounded_channel::<AgentMessage>();
    conn.hub.add_link(link_id.clone(), tx.clone());
    // Also when the link is closed from here (`Links`).
    let _cleanup = LinkGone {
        hub: conn.hub.clone(),
        inbound_tx: conn.inbound_tx.clone(),
        link_id: link_id.clone(),
        writer: spawn_writer(ws_write, rx),
    };
    let codec_support = tokio::task::spawn_blocking(crate::codec_support_bits).await.ok();
    let _ = tx.send(AgentMessage::Register {
        id: conn.hub.agent_id().to_string(),
        name: config.name.clone(),
        codec_support,
        multi_viewer: Some(true),
    });
    // The other computers this one shows, and the kinds it can show. A
    // server let in without the password hears only what its clients need
    // to choose one (no addresses, ports or user names of them, nor this
    // computer's own addresses): until the person here accepts a client,
    // it is anyone who knows this address.
    refresh_local_addresses().await;
    let _ = tx.send(crate::connector::sources::announcement_with(conn.trusted, conn.trusted));
    // Again when a set-up (a pairing) has added one, whoever still asks about it.
    let _announcing = crate::connector::sources::announce_changes_with(tx.clone(), conn.trusted, conn.trusted);
    let conn = Arc::new(conn);
    // Client connections (on that server) the person here accepted, per kind.
    let accepted: Arc<Mutex<HashMap<String, Kinds>>> = Arc::default();
    let asking: Arc<Mutex<HashSet<String>>> = Arc::default();
    let mut queries: HashMap<(String, u8), Instant> = HashMap::new();

    while let Some(frame) = ws_read.next().await {
        let text = match frame {
            Ok(WsMessage::Text(text)) => text,
            Ok(WsMessage::Close(_)) => break,
            Ok(_) => continue,
            Err(e) => {
                debug!("Server link {} read error: {}", link_id, e);
                break;
            }
        };
        let sig = match serde_json::from_str::<ServerToAgentMessage>(&text) {
            Ok(ServerToAgentMessage::Signaling(sig)) => sig,
            Ok(ServerToAgentMessage::Registered { .. }) => continue,
            Ok(ServerToAgentMessage::ChangeSource { request_id, change, requester }) => {
                // Only a server that proved the password may change the
                // sources; one whose connections are accepted here one by
                // one may not. Adding can wait a few seconds for a set-up's
                // first step, so not on this loop.
                let (tx, trusted, via) = (tx.clone(), conn.trusted, format!("server link {}", link_id));
                tokio::task::spawn_blocking(move || {
                    for message in crate::connector::sources::reply(request_id, &change, requester.as_deref(), &via, trusted) {
                        let _ = tx.send(message);
                    }
                });
                continue;
            }
            Ok(ServerToAgentMessage::Settings { request_id, request, requester }) => {
                // This computer's own settings (settings.rs): changed only by
                // a server that proved the password, and never so that this
                // link is cut.
                let channel = crate::settings::Channel::link(format!("server link {}", link_id), conn.trusted);
                let tx = tx.clone();
                tokio::task::spawn_blocking(move || {
                    let _ = tx.send(crate::settings::reply(request_id, &request, requester.as_deref(), &channel, crate::pairing::config_path()));
                });
                continue;
            }
            Err(e) => {
                warn!("Ignoring malformed message from server link {}: {}", link_id, e);
                continue;
            }
        };
        let Some(client) = target_of(&sig).map(str::to_owned) else { continue };
        // The session logic knows this link's clients by ids of their own
        // (`link_client_id`): a link cannot name a client of another link,
        // of the server or a direct one and get its replies. The hub gives
        // the link its own ids back.
        let sig = with_target(sig, &link_client_id(&link_id, &client));
        let granted = accepted.lock().unwrap().get(&client).copied().unwrap_or_default();
        let pass = match approval_kind(&sig) {
            Some(kind) => conn.trusted || granted.has(kind),
            None => conn.trusted || granted.covers(&sig) || conn.allow_query(&mut queries, |k| (client.clone(), k), &sig),
        };
        if pass {
            // Only clients something is forwarded for are routed back here.
            conn.hub.link_client(&link_client_id(&link_id, &client), &client, &link_id);
            if conn.inbound_tx.send(sig).is_err() {
                break;
            }
            continue;
        }
        let Some(kind) = approval_kind(&sig) else {
            debug!("Server client {} not accepted yet (or asking too often): dropping {:?}", client, sig);
            continue;
        };
        if !asking.lock().unwrap().insert(client.clone()) {
            continue;
        }
        let Some(prompt) = PromptSlot::take(&conn.limits, conn.ip) else {
            asking.lock().unwrap().remove(&client);
            info!("Server client {} via {} refused: other questions are already waiting", client, conn.ip);
            let _ = tx.send(AgentMessage::Signaling(access_status(&client, STATUS_BUSY)));
            continue;
        };
        let requester = match &sig {
            SignalingMessage::IncomingSession { requester: Some(r), .. }
            | SignalingMessage::IncomingFileSession { requester: Some(r), .. } => requester_label(r, conn.ip, true),
            _ => conn.requester.clone(),
        };
        let _ = tx.send(AgentMessage::Signaling(access_status(&client, "waiting")));
        let (conn, accepted, asking, tx, link_id) = (conn.clone(), accepted.clone(), asking.clone(), tx.clone(), link_id.clone());
        tokio::spawn(async move {
            let decision = conn.ask(&requester, kind).await;
            drop(prompt);
            asking.lock().unwrap().remove(&client);
            let _ = tx.send(AgentMessage::Signaling(access_status(&client, decision.status())));
            if decision == Decision::Accepted {
                accepted.lock().unwrap().entry(client.clone()).or_default().add(kind);
                conn.let_in();
                conn.hub.link_client(&link_client_id(&link_id, &client), &client, &link_id);
                let _ = conn.inbound_tx.send(sig);
            }
        });
    }

}

/// What ends with a server link, however it ends.
struct LinkGone {
    hub: Arc<Hub>,
    inbound_tx: mpsc::UnboundedSender<SignalingMessage>,
    link_id: String,
    writer: tokio::task::JoinHandle<()>,
}

impl Drop for LinkGone {
    fn drop(&mut self) {
        info!("Server link {} closed", self.link_id);
        for client in self.hub.remove_link(&self.link_id) {
            let _ = self.inbound_tx.send(SignalingMessage::EndSession { target_id: client });
        }
        self.writer.abort();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ip() -> IpAddr {
        "192.168.1.20".parse().unwrap()
    }

    const PW: ApproveMode = ApproveMode::Password;
    const BOTH: ApproveMode = ApproveMode::PasswordClick;
    const CLICK: ApproveMode = ApproveMode::Click;
    const TRUSTED: Admission = Admission::Accept { trusted: true, link: false };
    const ASK: Admission = Admission::Accept { trusted: false, link: false };

    fn limits() -> Mutex<Limits> {
        Mutex::new(Limits::default())
    }

    #[test]
    fn admits_only_the_right_password_on_the_client_path() {
        let guard = limits();
        assert_eq!(admit("/ws/client", "token=abcd-efgh", "abcd-efgh", PW, ip(), &guard), TRUSTED);
        assert_eq!(admit("/ws/client", "token=abcd%2Defgh", "abcd-efgh", PW, ip(), &guard), TRUSTED);
        assert_eq!(admit("/ws/client", "token=wrong", "abcd-efgh", PW, ip(), &guard), Admission::Unauthorized);
        assert_eq!(admit("/ws/client", "token=abcd-efg", "abcd-efgh", PW, ip(), &guard), Admission::Unauthorized);
        assert_eq!(admit("/ws/client", "", "abcd-efgh", PW, ip(), &guard), Admission::Unauthorized);
        assert_eq!(admit("/ws/agent", "token=abcd-efgh", "abcd-efgh", PW, ip(), &guard), Admission::NotFound);
        // An empty password never admits anyone.
        assert_eq!(admit("/ws/client", "token=", "", PW, ip(), &guard), Admission::Unauthorized);
    }

    #[test]
    fn without_the_password_the_person_here_is_asked() {
        let guard = limits();
        assert_eq!(admit("/ws/client", "token=pw", "pw", BOTH, ip(), &guard), TRUSTED);
        assert_eq!(admit("/ws/client", "", "pw", BOTH, ip(), &guard), ASK);
        assert_eq!(admit("/ws/client", "token=", "pw", BOTH, ip(), &guard), ASK);
        // A wrong password is still refused (it isn't a request to be asked).
        assert_eq!(admit("/ws/client", "token=nope", "pw", BOTH, ip(), &guard), Admission::Unauthorized);
        // Click: everyone is asked, even with the password.
        assert_eq!(admit("/ws/client", "token=pw", "pw", CLICK, ip(), &guard), ASK);
        // Servers connect on their own path, with the same rules.
        assert_eq!(admit("/ws/server", "token=pw", "pw", BOTH, ip(), &guard), Admission::Accept { trusted: true, link: true });
        assert_eq!(admit("/ws/server", "", "pw", BOTH, ip(), &guard), Admission::Accept { trusted: false, link: true });
        assert_eq!(admit("/ws/server", "", "pw", PW, ip(), &guard), Admission::Unauthorized);
    }

    #[test]
    fn only_requests_need_approval() {
        let req = SignalingMessage::RequestFileSession { host_id: "h".into(), source_id: None };
        assert_eq!(approval_kind(&req), Some(Kind::Files));
        assert!(!allowed_before_approval(&req));
        let caps = SignalingMessage::GetCapabilities { target_id: "h".into() };
        assert_eq!(approval_kind(&caps), None);
        assert!(allowed_before_approval(&caps));
        let stop = SignalingMessage::StopActiveStream { target_id: "h".into() };
        assert!(approval_kind(&stop).is_none() && !allowed_before_approval(&stop));
    }

    #[test]
    fn locks_out_after_repeated_wrong_passwords() {
        let guard = limits();
        for _ in 0..MAX_FAILED_LOGINS {
            assert_eq!(admit("/ws/client", "token=x", "secret", PW, ip(), &guard), Admission::Unauthorized);
        }
        // Even the right password is refused during the lockout...
        assert_eq!(admit("/ws/client", "token=secret", "secret", PW, ip(), &guard), Admission::TooManyAttempts);
        // ...but other addresses are not affected.
        let other: IpAddr = "192.168.1.21".parse().unwrap();
        assert_eq!(admit("/ws/client", "token=secret", "secret", PW, other, &guard), TRUSTED);
    }

    #[test]
    fn ipv6_addresses_count_by_their_64() {
        let a: IpAddr = "2001:db8:1:2::1".parse().unwrap();
        let b: IpAddr = "2001:db8:1:2:ffff::9".parse().unwrap();
        let c: IpAddr = "2001:db8:1:3::1".parse().unwrap();
        assert_eq!(addr_key(a), addr_key(b));
        assert_ne!(addr_key(a), addr_key(c));
        let mapped: IpAddr = "::ffff:192.168.1.20".parse().unwrap();
        assert_eq!(addr_key(mapped), ip());
        let guard = limits();
        for _ in 0..MAX_FAILED_LOGINS {
            admit("/ws/client", "token=x", "secret", PW, a, &guard);
        }
        assert_eq!(admit("/ws/client", "token=secret", "secret", PW, b, &guard), Admission::TooManyAttempts);
        assert_eq!(admit("/ws/client", "token=secret", "secret", PW, c, &guard), TRUSTED);
    }

    #[test]
    fn the_lockout_map_stays_bounded() {
        let mut guard = LoginGuard::default();
        for i in 0..(MAX_TRACKED_ADDRS as u32 + 100) {
            guard.failed(IpAddr::V4(std::net::Ipv4Addr::from(0x0a00_0000 + i)));
        }
        assert!(guard.failures.len() <= MAX_TRACKED_ADDRS);
    }

    fn v4(i: u32) -> IpAddr {
        IpAddr::V4(std::net::Ipv4Addr::from(0x0a00_0000 + i))
    }

    #[test]
    fn locked_out_addresses_are_not_evicted_by_others() {
        let mut guard = LoginGuard::default();
        let attacker = v4(1_000_000);
        for _ in 0..MAX_FAILED_LOGINS {
            guard.failed(attacker);
        }
        for i in 0..(MAX_TRACKED_ADDRS as u32 + 100) {
            guard.failed(v4(i));
        }
        assert!(guard.failures.len() <= MAX_TRACKED_ADDRS);
        assert!(guard.blocked(&addr_key(attacker)), "a lockout was undone by other addresses' failures");
    }

    #[test]
    fn too_many_failures_from_everyone_stop_new_addresses() {
        let mut guard = LoginGuard::default();
        let regular = v4(9_999);
        guard.succeeded(regular);
        for i in 0..GLOBAL_MAX_FAILURES as u32 {
            assert!(!guard.locked_out(v4(i)));
            guard.failed(v4(i));
        }
        // None of them is locked out by its own failures...
        assert!(!guard.blocked(&addr_key(v4(0))));
        // ...but together they are too many: a new address waits,
        assert!(guard.locked_out(v4(5_000)));
        // one that logged in with the password lately does not.
        assert!(!guard.locked_out(regular));
    }

    #[test]
    fn unanswered_questions_count_half_and_make_the_next_one_wait() {
        let mut limits = Limits::default();
        // Each timeout makes the next question from there wait.
        assert!(limits.start_prompt(ip()));
        limits.end_prompt(ip());
        limits.prompt_answered(ip(), Decision::Timeout);
        assert!(!limits.start_prompt(ip()), "asked again at once after a timeout");
        // Others are still asked.
        assert!(limits.start_prompt(v4(1)));
        // Half a failure each: twice as many timeouts as wrong passwords lock out.
        let mut guard = LoginGuard::default();
        for _ in 0..(2 * MAX_FAILED_LOGINS - 1) {
            guard.timed_out(ip());
        }
        assert!(!guard.locked_out(ip()));
        guard.timed_out(ip());
        assert!(guard.locked_out(ip()));
        // A refusal is a whole one; an answer clears the wait.
        let mut limits = Limits::default();
        limits.prompt_answered(ip(), Decision::Denied);
        assert_eq!(limits.logins.failures[&addr_key(ip())].0, FULL_FAILURE);
        limits.prompt_answered(ip(), Decision::Accepted);
        assert!(limits.start_prompt(ip()));
        // Nobody to ask is nobody's fault.
        let mut limits = Limits::default();
        limits.prompt_answered(ip(), Decision::Unattended);
        assert!(limits.start_prompt(ip()) && limits.logins.failures.is_empty());
    }

    #[test]
    fn the_wait_after_refused_questions_grows_to_the_lockout() {
        assert_eq!(prompt_backoff(0), Duration::ZERO);
        assert_eq!(prompt_backoff(1), PROMPT_BACKOFF);
        assert_eq!(prompt_backoff(2), PROMPT_BACKOFF * 2);
        assert_eq!(prompt_backoff(30), LOCKOUT);
    }

    #[test]
    fn a_locked_out_address_gets_no_question() {
        let mut limits = Limits::default();
        for _ in 0..MAX_FAILED_LOGINS {
            limits.logins.failed(ip());
        }
        assert!(!limits.start_prompt(ip()));
    }

    #[test]
    fn names_are_cleaned_and_marked_as_claims() {
        assert_eq!(clean_name("  bob@pc\n\u{7}"), "bob@pc");
        assert_eq!(clean_name("evil\u{202E}cod.exe"), "evilcod.exe");
        assert_eq!(clean_name(&"x".repeat(500)).chars().count(), MAX_NAME_CHARS);
        assert_eq!(requester_label("", ip(), false), "192.168.1.20");
        assert_eq!(requester_label("Admin\r\nAccepted", ip(), false), "\"AdminAccepted\" (unverified name) from 192.168.1.20");
        assert!(requester_label("bob", ip(), true).ends_with("via the server at 192.168.1.20"));
    }

    #[test]
    fn unauthenticated_connections_and_prompts_are_capped() {
        let limits = Arc::new(limits());
        let slots: Vec<_> = (0..MAX_UNAUTH_PER_ADDR).map(|_| UnauthSlot::take(&limits, ip()).unwrap()).collect();
        assert!(UnauthSlot::take(&limits, ip()).is_none());
        drop(slots);
        assert!(UnauthSlot::take(&limits, ip()).is_some());
        let mut all = Vec::new();
        for i in 0..MAX_UNAUTH_TOTAL as u32 {
            all.push(UnauthSlot::take(&limits, IpAddr::V4(std::net::Ipv4Addr::from(0x0a00_0000 + i))).unwrap());
        }
        assert!(UnauthSlot::take(&limits, "10.9.9.9".parse().unwrap()).is_none());
        drop(all);
        assert_eq!(limits.lock().unwrap().unauth_total, 0);

        let one = PromptSlot::take(&limits, ip()).unwrap();
        // One question per address...
        assert!(PromptSlot::take(&limits, ip()).is_none());
        // ...and a few in all.
        let more: Vec<_> = (1..MAX_PROMPTS as u32)
            .map(|i| PromptSlot::take(&limits, IpAddr::V4(std::net::Ipv4Addr::from(0x0a00_0000 + i))).unwrap())
            .collect();
        assert!(PromptSlot::take(&limits, "10.9.9.9".parse().unwrap()).is_none());
        drop(one);
        assert!(PromptSlot::take(&limits, ip()).is_some());
        drop(more);
    }

    #[test]
    fn queries_are_rate_limited() {
        let mut map = HashMap::new();
        let t0 = Instant::now();
        assert!(allow_at(&mut map, 0u8, t0));
        assert!(!allow_at(&mut map, 0u8, t0 + Duration::from_millis(500)));
        assert!(allow_at(&mut map, 1u8, t0 + Duration::from_millis(500)));
        assert!(allow_at(&mut map, 0u8, t0 + QUERY_INTERVAL));
    }

    #[test]
    fn kinds_are_granted_one_by_one() {
        let mut k = Kinds::default();
        assert!(!k.any());
        k.add(Kind::Files);
        assert!(k.any() && k.has(Kind::Files) && !k.has(Kind::Control));
        k.add(Kind::Control);
        assert!(k.has(Kind::Control));
    }

    // ---- The listener on 127.0.0.1, with the client side of handshake.rs ----

    use tokio_tungstenite::tungstenite;

    async fn start_agent(mode: ApproveMode, password: &str, asker: Asker) -> (String, mpsc::UnboundedReceiver<SignalingMessage>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap().to_string();
        let (tx, rx) = mpsc::unbounded_channel();
        let config = DirectConfig { port: 0, password: password.into(), approve_mode: mode, name: "test".into(), links: Links::default(), allow_legacy_plain: true, tls_dir: None };
        tokio::spawn(serve_listener(listener, config, Hub::new("AGENT".into()), tx, asker, Arc::default()));
        (addr, rx)
    }

    /// Links stay across new settings that would let them in again, and
    /// all end with the agent.
    #[tokio::test]
    async fn links_are_kept_only_while_the_settings_let_them_in() {
        let links = Links::default();
        let forever = || std::future::pending::<()>();
        let open = |trusted: bool| {
            links.spawn(trusted, forever());
        };
        open(true);
        open(false);
        assert_eq!(links.keep_admitted(true, false, ApproveMode::PasswordClick), 0);
        assert_eq!(links.open(), 2);
        // A new password: the one that proved the old one goes.
        assert_eq!(links.keep_admitted(true, true, ApproveMode::PasswordClick), 1);
        tokio::task::yield_now().await;
        assert_eq!(links.open(), 1);
        // Password only: nobody is let in without it any more.
        open(true);
        assert_eq!(links.keep_admitted(true, false, ApproveMode::Password), 1);
        tokio::task::yield_now().await;
        assert_eq!(links.open(), 1);
        // Everyone asked: the password lets nobody in by itself.
        open(false);
        assert_eq!(links.keep_admitted(true, false, ApproveMode::Click), 1);
        tokio::task::yield_now().await;
        assert_eq!(links.open(), 1);
        // Direct access off, then the agent stops.
        assert_eq!(links.keep_admitted(false, false, ApproveMode::Click), 1);
        open(true);
        open(false);
        links.close_all();
        tokio::task::yield_now().await;
        assert_eq!(links.open(), 0);
    }

    fn never_asked() -> Asker {
        Arc::new(|_, _| Box::pin(async { panic!("nobody should be asked") }))
    }

    async fn send<S: AsyncRead + AsyncWrite + Unpin>(ws: &mut WebSocketStream<S>, sig: SignalingMessage) {
        let json = serde_json::to_string(&ClientMessage::Signaling(sig)).unwrap();
        ws.send(WsMessage::Text(json.into())).await.unwrap();
    }

    async fn inbound(rx: &mut mpsc::UnboundedReceiver<SignalingMessage>) -> SignalingMessage {
        tokio::time::timeout(Duration::from_secs(5), rx.recv()).await.expect("nothing reached the session logic").unwrap()
    }

    async fn quiet(rx: &mut mpsc::UnboundedReceiver<SignalingMessage>) -> bool {
        tokio::time::sleep(Duration::from_millis(300)).await;
        rx.try_recv().is_err()
    }

    /// The next AccessRequest status the client hears.
    async fn status<S: AsyncRead + AsyncWrite + Unpin>(ws: &mut WebSocketStream<S>) -> String {
        loop {
            let frame = tokio::time::timeout(Duration::from_secs(5), ws.next()).await.unwrap().unwrap().unwrap();
            if let WsMessage::Text(text) = frame {
                if let Ok(ServerToClientMessage::Signaling(SignalingMessage::AccessRequest { status, .. })) =
                    serde_json::from_str(&text)
                {
                    return status;
                }
            }
        }
    }

    fn session_request() -> SignalingMessage {
        SignalingMessage::RequestSession {
            host_id: "AGENT".into(),
            width: None,
            height: None,
            fps: None,
            bitrate: None,
            codec: None,
            app_id: None,
            encoder: None,
            display_id: None,
            chroma: None,
            encoder_csc_mode: None,
            microphone: None,
            source_id: None,
            decode_codecs: None,
        }
    }

    #[tokio::test]
    async fn new_client_and_agent_use_tls_and_the_password_proof() {
        let (addr, mut rx) = start_agent(PW, "abcd-efgh-ijkl", never_asked()).await;
        let mut c = connect(&addr, "/ws/client", "abcd-efgh-ijkl", "name=tester").await.unwrap();
        assert!(c.trusted);
        assert_eq!(c.version, handshake::VERSION_PAKE);
        // Trusted: the session request goes through without asking anyone.
        send(&mut c.ws, session_request()).await;
        assert!(matches!(inbound(&mut rx).await, SignalingMessage::IncomingSession { .. }));
    }

    #[tokio::test]
    async fn wrong_password_is_refused_over_tls_then_locked_out() {
        let (addr, mut rx) = start_agent(PW, "abcd-efgh-ijkl", never_asked()).await;
        for _ in 0..MAX_FAILED_LOGINS {
            assert_eq!(connect(&addr, "/ws/client", "wrong", "").await.err(), Some(ConnectError::WrongPassword));
        }
        // No password at all isn't let in either (password-only mode).
        assert_eq!(
            connect(&addr, "/ws/client", "abcd-efgh-ijkl", "").await.err(),
            Some(ConnectError::TooManyAttempts)
        );
        assert!(quiet(&mut rx).await);
    }

    /// The v2 guess is counted before the agent's reply: hanging up after
    /// it (an attacker holding its one guess) still locks out.
    #[tokio::test]
    async fn v2_guesses_count_even_when_the_client_hangs_up() {
        use handshake::{from_hex, pake_client, to_hex, v2_context, AuthMessage, OsRng};
        let (addr, _rx) = start_agent(PW, "abcd-efgh-ijkl", never_asked()).await;
        let text = |m: &AuthMessage| WsMessage::Text(serde_json::to_string(m).unwrap().into());
        for _ in 0..MAX_FAILED_LOGINS {
            let (mut ws, cert) = handshake::open_for_test(&addr, "/ws/client").await;
            ws.send(text(&AuthMessage::Hello { version: 2 })).await.unwrap();
            let Some(Ok(WsMessage::Text(t))) = ws.next().await else { panic!() };
            let Ok(AuthMessage::Challenge { nonce, versions }) = serde_json::from_str(&t) else { panic!("{}", t) };
            assert_eq!(versions, vec![handshake::VERSION_PAKE]);
            let ctx = v2_context(&from_hex::<32>(&nonce).unwrap(), &[7; 32], &cert);
            let (_, x) = pake_client("a-guess", &ctx, OsRng);
            ws.send(text(&AuthMessage::Pake { client_nonce: to_hex(&[7; 32]), msg: to_hex(&x) })).await.unwrap();
            let Some(Ok(WsMessage::Text(t))) = ws.next().await else { panic!() };
            assert!(matches!(serde_json::from_str(&t), Ok(AuthMessage::PakeReply { .. })), "{}", t);
            drop(ws);
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert_eq!(connect(&addr, "/ws/client", "abcd-efgh-ijkl", "").await.err(), Some(ConnectError::TooManyAttempts));
    }

    /// Failures counted in advance are taken back for the right password:
    /// right after a few wrong ones it still gets in.
    #[tokio::test]
    async fn the_right_password_after_wrong_ones_still_gets_in() {
        let (addr, _rx) = start_agent(PW, "abcd-efgh-ijkl", never_asked()).await;
        for _ in 0..MAX_FAILED_LOGINS - 1 {
            assert_eq!(connect(&addr, "/ws/client", "wrong", "").await.err(), Some(ConnectError::WrongPassword));
        }
        assert!(connect(&addr, "/ws/client", "abcd-efgh-ijkl", "").await.unwrap().trusted);
        // Success forgot the failures.
        for _ in 0..MAX_FAILED_LOGINS - 1 {
            assert_eq!(connect(&addr, "/ws/client", "wrong", "").await.err(), Some(ConnectError::WrongPassword));
        }
        assert!(connect(&addr, "/ws/client", "abcd-efgh-ijkl", "").await.unwrap().trusted);
    }

    /// An app or server from before v2 (the exact old messages): the v1
    /// proof is still taken, wrong ones still count.
    #[tokio::test]
    async fn an_old_client_logs_in_to_a_new_agent() {
        let (addr, mut rx) = start_agent(PW, "abcd-efgh-ijkl", never_asked()).await;
        let (mut ws, cert) = handshake::open_for_test(&addr, "/ws/client").await;
        assert_eq!(handshake::v1::client_auth(&mut ws, "abcd-efgh-ijkl", &cert).await, Ok(true));
        send(&mut ws, session_request()).await;
        assert!(matches!(inbound(&mut rx).await, SignalingMessage::IncomingSession { .. }));
        for _ in 0..MAX_FAILED_LOGINS {
            let (mut ws, cert) = handshake::open_for_test(&addr, "/ws/client").await;
            assert_eq!(handshake::v1::client_auth(&mut ws, "wrong", &cert).await, Err(ConnectError::WrongPassword));
        }
        assert_eq!(connect(&addr, "/ws/client", "abcd-efgh-ijkl", "").await.err(), Some(ConnectError::TooManyAttempts));
    }

    /// An agent from before v2 behind TLS: the new client falls back to v1
    /// (with a warning), unless v2 is required (then nothing is sent).
    #[tokio::test]
    async fn a_new_client_with_an_old_agent() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap().to_string();
        let proofs = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let proofs2 = proofs.clone();
        tokio::spawn(async move {
            let tls = handshake::make_tls_server().unwrap();
            loop {
                let (stream, _) = listener.accept().await.unwrap();
                let Ok(tls_stream) = tls.acceptor.accept(stream).await else { continue };
                let Ok(mut ws) = tokio_tungstenite::accept_async(tls_stream).await else { continue };
                if handshake::v1::agent_auth(&mut ws, "old-pw", &tls.cert_hash).await.is_some() {
                    proofs2.fetch_add(1, Ordering::SeqCst);
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        });
        let c = connect(&addr, "/ws/client", "old-pw", "").await.unwrap();
        assert!(c.trusted);
        assert_eq!(c.version, 1);
        assert_eq!(connect(&addr, "/ws/client", "nope", "").await.err(), Some(ConnectError::WrongPassword));
        assert_eq!(proofs.load(Ordering::SeqCst), 2);
        assert_eq!(handshake::connect_with(&addr, "/ws/client", "old-pw", "", true).await.err(), Some(ConnectError::Downgraded));
        tokio::time::sleep(Duration::from_millis(200)).await;
        assert_eq!(proofs.load(Ordering::SeqCst), 2, "a proof went to the old agent although v2 was required");
    }

    #[tokio::test]
    async fn no_password_in_password_mode_is_refused() {
        let (addr, _rx) = start_agent(PW, "abcd-efgh-ijkl", never_asked()).await;
        assert_eq!(connect(&addr, "/ws/client", "", "").await.err(), Some(ConnectError::WrongPassword));
    }

    #[tokio::test]
    async fn old_style_token_is_still_accepted() {
        let (addr, mut rx) = start_agent(PW, "pw", never_asked()).await;
        let (mut ws, _) = tokio_tungstenite::connect_async(format!("ws://{}/ws/client?token=pw&name=old", addr)).await.unwrap();
        send(&mut ws, session_request()).await;
        assert!(matches!(inbound(&mut rx).await, SignalingMessage::IncomingSession { .. }));
        match tokio_tungstenite::connect_async(format!("ws://{}/ws/client?token=nope", addr)).await {
            Err(tungstenite::Error::Http(resp)) => assert_eq!(resp.status().as_u16(), 401),
            other => panic!("expected 401, got {:?}", other.map(|_| ())),
        }
    }

    /// An agent from before TLS: records every byte any connection sends it.
    async fn old_agent() -> (String, Arc<Mutex<Vec<u8>>>, Arc<std::sync::atomic::AtomicUsize>) {
        use tokio::io::AsyncReadExt;
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap().to_string();
        let seen: Arc<Mutex<Vec<u8>>> = Arc::default();
        let conns = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let (seen2, conns2) = (seen.clone(), conns.clone());
        tokio::spawn(async move {
            loop {
                let (mut stream, _) = listener.accept().await.unwrap();
                conns2.fetch_add(1, Ordering::SeqCst);
                let seen = seen2.clone();
                tokio::spawn(async move {
                    // Like tungstenite there: the TLS hello is no HTTP request, hang up.
                    let mut buf = [0u8; 4096];
                    if let Ok(n) = stream.read(&mut buf).await {
                        seen.lock().unwrap().extend_from_slice(&buf[..n]);
                    }
                });
            }
        });
        (addr, seen, conns)
    }

    #[tokio::test]
    async fn nothing_goes_over_plain_after_a_tls_failure() {
        let (addr, seen, conns) = old_agent().await;
        let err = connect(&addr, "/ws/client", "secret-pw", "name=x").await.err().unwrap();
        assert!(matches!(err, ConnectError::NotSecure(_)), "{:?}", err);
        assert!(err.to_string().contains("update that agent"), "{}", err);
        tokio::time::sleep(Duration::from_millis(300)).await;
        // One connection (the TLS attempt), no second plain one.
        assert_eq!(conns.load(Ordering::SeqCst), 1);
        let seen = seen.lock().unwrap();
        let text = String::from_utf8_lossy(&seen);
        assert!(!text.contains("token=") && !text.contains("secret-pw") && !text.contains("GET "), "{}", text);
    }

    #[tokio::test]
    async fn password_click_refuses_a_wrong_password_and_counts_it() {
        let (addr, _rx) = start_agent(BOTH, "right-pw", never_asked()).await;
        for _ in 0..MAX_FAILED_LOGINS {
            assert_eq!(connect(&addr, "/ws/client", "wrong", "").await.err(), Some(ConnectError::WrongPassword));
        }
        assert_eq!(connect(&addr, "/ws/client", "right-pw", "").await.err(), Some(ConnectError::TooManyAttempts));
    }

    #[tokio::test]
    async fn password_click_without_a_password_is_asked_and_with_it_trusted() {
        let (addr, _rx) = start_agent(BOTH, "right-pw", never_asked()).await;
        assert!(!connect(&addr, "/ws/client", "", "").await.unwrap().trusted);
        assert!(connect(&addr, "/ws/client", "right-pw", "").await.unwrap().trusted);
    }

    #[tokio::test]
    async fn click_mode_proves_itself_to_a_client_with_the_password() {
        let (addr, _rx) = start_agent(CLICK, "right-pw", never_asked()).await;
        // The right password: the agent proves it, the person there decides.
        assert!(!connect(&addr, "/ws/client", "right-pw", "").await.unwrap().trusted);
        // A wrong one: the agent can't prove anything, the client stops.
        assert_eq!(connect(&addr, "/ws/client", "wrong", "").await.err(), Some(ConnectError::NotProven));
    }

    /// A fake agent with its own certificate that waves a password client
    /// through as "ok, untrusted".
    #[tokio::test]
    async fn an_impostor_without_the_password_is_not_accepted() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap().to_string();
        tokio::spawn(async move {
            let tls = handshake::make_tls_server().unwrap();
            loop {
                let (stream, _) = listener.accept().await.unwrap();
                let Ok(tls_stream) = tls.acceptor.accept(stream).await else { continue };
                let Ok(mut ws) = tokio_tungstenite::accept_async(tls_stream).await else { continue };
                let _ = ws.next().await;
                let challenge = r#"{"auth":"challenge","nonce":"0000000000000000000000000000000000000000000000000000000000000000"}"#;
                let _ = ws.send(WsMessage::Text(challenge.into())).await;
                let _ = ws.next().await;
                let _ = ws.send(WsMessage::Text(r#"{"auth":"ok","trusted":false}"#.into())).await;
                tokio::time::sleep(Duration::from_millis(200)).await;
            }
        });
        assert_eq!(connect(&addr, "/ws/client", "pw", "").await.err(), Some(ConnectError::NotProven));
        // Without a password that answer is the normal click-to-accept one.
        assert!(!connect(&addr, "/ws/client", "", "").await.unwrap().trusted);
    }

    #[tokio::test]
    async fn each_kind_of_access_is_asked_for_separately() {
        let asked: Arc<Mutex<Vec<Kind>>> = Arc::default();
        let log = asked.clone();
        let asker: Asker = Arc::new(move |requester, kind| {
            assert!(requester.contains("(unverified name) from 127.0.0.1"), "{}", requester);
            log.lock().unwrap().push(kind);
            // Files yes, the screen no.
            Box::pin(async move { if kind == Kind::Files { Decision::Accepted } else { Decision::Denied } })
        });
        let (addr, mut rx) = start_agent(CLICK, "", asker).await;
        let mut c = connect(&addr, "/ws/client", "", "name=someone").await.unwrap();
        assert!(!c.trusted);

        // Before being let in: queries are rate-limited.
        let caps = SignalingMessage::GetCapabilities { target_id: "AGENT".into() };
        send(&mut c.ws, caps.clone()).await;
        send(&mut c.ws, caps.clone()).await;
        assert!(matches!(inbound(&mut rx).await, SignalingMessage::GetCapabilities { .. }));
        assert!(quiet(&mut rx).await);

        send(&mut c.ws, SignalingMessage::RequestFileSession { host_id: "AGENT".into(), source_id: None }).await;
        assert_eq!(status(&mut c.ws).await, "waiting");
        assert_eq!(status(&mut c.ws).await, "accepted");
        assert!(matches!(inbound(&mut rx).await, SignalingMessage::IncomingFileSession { .. }));
        // File access doesn't let it stop the screen stream (someone else's).
        send(&mut c.ws, SignalingMessage::StopActiveStream { target_id: "AGENT".into() }).await;
        assert!(quiet(&mut rx).await);

        // Accepted for files is not accepted for the screen: asked again.
        send(&mut c.ws, session_request()).await;
        assert_eq!(status(&mut c.ws).await, "waiting");
        assert_eq!(status(&mut c.ws).await, "denied");
        assert!(quiet(&mut rx).await);
        assert_eq!(*asked.lock().unwrap(), vec![Kind::Files, Kind::Control]);

        // Let in for something: queries are no longer limited.
        send(&mut c.ws, caps.clone()).await;
        send(&mut c.ws, caps).await;
        inbound(&mut rx).await;
        inbound(&mut rx).await;
    }

    async fn start_agent_with(config: DirectConfig, asker: Asker) -> (String, mpsc::UnboundedReceiver<SignalingMessage>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap().to_string();
        let (tx, rx) = mpsc::unbounded_channel();
        tokio::spawn(serve_listener(listener, config, Hub::new("AGENT".into()), tx, asker, Arc::default()));
        (addr, rx)
    }

    fn config(mode: ApproveMode, password: &str) -> DirectConfig {
        DirectConfig { port: 0, password: password.into(), approve_mode: mode, name: "test".into(), links: Links::default(), allow_legacy_plain: true, tls_dir: None }
    }

    #[tokio::test]
    async fn legacy_plain_logins_can_be_turned_off() {
        let mut off = config(PW, "a-long-password");
        off.allow_legacy_plain = false;
        let (addr, mut rx) = start_agent_with(off, never_asked()).await;
        match tokio_tungstenite::connect_async(format!("ws://{}/ws/client?token=a-long-password", addr)).await {
            Err(tungstenite::Error::Http(resp)) => assert_eq!(resp.status().as_u16(), 426),
            other => panic!("expected 426, got {:?}", other.map(|_| ())),
        }
        // TLS clients are not affected.
        let mut c = connect(&addr, "/ws/client", "a-long-password", "").await.unwrap();
        assert!(c.trusted);
        send(&mut c.ws, session_request()).await;
        assert!(matches!(inbound(&mut rx).await, SignalingMessage::IncomingSession { .. }));
    }

    #[tokio::test]
    async fn big_messages_are_refused() {
        let (addr, mut rx) = start_agent(PW, "a-long-password", never_asked()).await;
        let mut c = connect(&addr, "/ws/client", "a-long-password", "").await.unwrap();
        // An SDP of a realistic size passes...
        let sdp = |len: usize| SignalingMessage::Sdp {
            target_id: "AGENT".into(),
            sdp: common::RtcSessionDescription { ty: common::RtcSdpType::Answer, sdp: "a".repeat(len) },
            ice_servers: None,
            webtransport_port: None,
            webtransport_cert_hash: None,
            webtransport_token: None,
        };
        send(&mut c.ws, sdp(32 * 1024)).await;
        assert!(matches!(inbound(&mut rx).await, SignalingMessage::Sdp { .. }));
        // ...one beyond the limit ends the connection, unread.
        let _ = c.ws.send(WsMessage::Text(serde_json::to_string(&ClientMessage::Signaling(sdp(MAX_WS_MESSAGE + 1))).unwrap().into())).await;
        // Only the end of the client's session (it is gone) follows.
        assert!(matches!(inbound(&mut rx).await, SignalingMessage::EndSession { .. }));
        let closed = tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                match c.ws.next().await {
                    None | Some(Err(_)) | Some(Ok(WsMessage::Close(_))) => return true,
                    Some(Ok(_)) => continue,
                }
            }
        })
        .await
        .unwrap_or(false);
        assert!(closed, "the connection stayed open");
    }

    /// The next message of `kind` a link hears from the agent.
    async fn link_message<S: AsyncRead + AsyncWrite + Unpin>(ws: &mut WebSocketStream<S>, want: impl Fn(&AgentMessage) -> bool) -> AgentMessage {
        loop {
            let frame = tokio::time::timeout(Duration::from_secs(10), ws.next()).await.expect("nothing from the agent").unwrap().unwrap();
            if let WsMessage::Text(text) = frame {
                if let Ok(msg) = serde_json::from_str::<AgentMessage>(&text) {
                    if want(&msg) {
                        return msg;
                    }
                }
            }
        }
    }

    async fn send_link<S: AsyncRead + AsyncWrite + Unpin>(ws: &mut WebSocketStream<S>, msg: ServerToAgentMessage) {
        ws.send(WsMessage::Text(serde_json::to_string(&msg).unwrap().into())).await.unwrap();
    }

    #[tokio::test]
    async fn a_link_without_the_password_hears_no_details() {
        let (addr, _rx) = start_agent(BOTH, "a-long-password", never_asked()).await;
        let mut link = connect(&addr, "/ws/server", "", "name=srv").await.unwrap();
        assert!(!link.trusted);
        // Registered first: the server needs that.
        assert!(matches!(link_message(&mut link.ws, |_| true).await, AgentMessage::Register { .. }));
        match link_message(&mut link.ws, |m| matches!(m, AgentMessage::Sources { .. })).await {
            AgentMessage::Sources { sources, addresses, manageable, .. } => {
                assert!(addresses.is_empty() && !manageable);
                assert!(sources.iter().all(|s| s.values.is_none()));
            }
            _ => unreachable!(),
        }
        send_link(&mut link.ws, ServerToAgentMessage::Settings { request_id: "q".into(), request: common::SettingsRequest::Get, requester: None }).await;
        match link_message(&mut link.ws, |m| matches!(m, AgentMessage::SettingsAnswer { .. })).await {
            AgentMessage::SettingsAnswer { settings, code, .. } => {
                assert!(settings.is_none());
                assert_eq!(code.as_deref(), Some(common::SETTINGS_NOT_ALLOWED));
            }
            _ => unreachable!(),
        }
    }

    #[tokio::test]
    async fn a_links_clients_get_ids_of_their_own_and_replies_come_back() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap().to_string();
        let (tx, mut rx) = mpsc::unbounded_channel();
        let hub = Hub::new("AGENT".into());
        tokio::spawn(serve_listener(listener, config(PW, "a-long-password"), hub.clone(), tx, never_asked(), Arc::default()));
        let mut link = connect(&addr, "/ws/server", "a-long-password", "").await.unwrap();
        assert!(link.trusted);
        link_message(&mut link.ws, |m| matches!(m, AgentMessage::Register { .. })).await;
        send_link(&mut link.ws, ServerToAgentMessage::Signaling(SignalingMessage::GetCapabilities { target_id: "conn-1".into() })).await;
        let seen = inbound(&mut rx).await;
        let id = target_of(&seen).unwrap().to_string();
        assert!(id.starts_with("link-") && id.ends_with("/conn-1"), "{}", id);
        // The reply reaches the link, addressed as its server knows the client.
        hub.route(AgentMessage::Signaling(access_status(&id, "x")));
        match link_message(&mut link.ws, |m| matches!(m, AgentMessage::Signaling(SignalingMessage::AccessRequest { .. }))).await {
            AgentMessage::Signaling(sig) => assert_eq!(target_of(&sig), Some("conn-1")),
            _ => unreachable!(),
        }
    }
}
