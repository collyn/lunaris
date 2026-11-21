//! Small security helpers: random secrets, constant-time comparison, agent
//! keys, failure rate limits and the "may this user use this host" rule.

use std::collections::{HashMap, HashSet};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use sha2::{Digest, Sha256};
use sqlx::SqlitePool;
use subtle::ConstantTimeEq;

/// `bytes` random bytes from the OS, as lowercase hex.
pub fn random_hex(bytes: usize) -> String {
    let mut buf = vec![0u8; bytes];
    getrandom::fill(&mut buf).expect("OS random source unavailable");
    hex::encode(buf)
}

/// Equality that doesn't leak where two secrets differ.
pub fn ct_eq(a: &str, b: &str) -> bool {
    a.as_bytes().ct_eq(b.as_bytes()).into()
}

pub fn sha256_hex(text: &str) -> String {
    hex::encode(Sha256::digest(text.as_bytes()))
}

/// An address or network in `LUNARIS_TRUSTED_PROXIES` ("10.0.0.0/8",
/// "172.17.0.1", "fd00::/8").
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cidr {
    net: std::net::IpAddr,
    prefix: u8,
}

impl Cidr {
    pub fn parse(text: &str) -> Option<Self> {
        let text = text.trim();
        let (ip, prefix) = match text.split_once('/') {
            Some((ip, p)) => (ip.trim().parse::<std::net::IpAddr>().ok()?, Some(p.trim().parse::<u8>().ok()?)),
            None => (text.parse::<std::net::IpAddr>().ok()?, None),
        };
        let ip = canonical_ip(ip);
        let max = if ip.is_ipv4() { 32 } else { 128 };
        let prefix = prefix.unwrap_or(max);
        (prefix <= max).then_some(Cidr { net: ip, prefix })
    }

    pub fn contains(&self, ip: std::net::IpAddr) -> bool {
        match (self.net, canonical_ip(ip)) {
            (std::net::IpAddr::V4(net), std::net::IpAddr::V4(ip)) => {
                let mask = if self.prefix == 0 { 0 } else { u32::MAX << (32 - self.prefix as u32) };
                (u32::from(net) & mask) == (u32::from(ip) & mask)
            }
            (std::net::IpAddr::V6(net), std::net::IpAddr::V6(ip)) => {
                let mask = if self.prefix == 0 { 0 } else { u128::MAX << (128 - self.prefix as u32) };
                (u128::from(net) & mask) == (u128::from(ip) & mask)
            }
            _ => false,
        }
    }
}

/// IPv4-mapped IPv6 (a dual-stack socket) as plain IPv4.
pub fn canonical_ip(ip: std::net::IpAddr) -> std::net::IpAddr {
    match ip {
        std::net::IpAddr::V6(v6) => v6.to_ipv4_mapped().map(std::net::IpAddr::V4).unwrap_or(ip),
        v4 => v4,
    }
}

/// `LUNARIS_TRUSTED_PROXIES`: comma-separated addresses or networks of
/// reverse proxies (e.g. a Docker bridge) whose `X-Forwarded-For` is
/// believed, besides loopback. Invalid entries are logged and skipped.
pub fn parse_trusted_proxies(text: &str) -> Vec<Cidr> {
    text.split(',')
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .filter_map(|t| {
            let c = Cidr::parse(t);
            if c.is_none() {
                tracing::warn!("LUNARIS_TRUSTED_PROXIES: {:?} is not an address or network; ignored", t);
            }
            c
        })
        .collect()
}

static TRUSTED_PROXIES: once_cell::sync::Lazy<Vec<Cidr>> = once_cell::sync::Lazy::new(|| {
    std::env::var("LUNARIS_TRUSTED_PROXIES").map(|v| parse_trusted_proxies(&v)).unwrap_or_default()
});

fn is_trusted_proxy(ip: std::net::IpAddr, trusted: &[Cidr]) -> bool {
    canonical_ip(ip).is_loopback() || trusted.iter().any(|c| c.contains(ip))
}

/// The address a request comes from, for rate limits: behind a reverse proxy
/// (TCP peer on loopback, or in `LUNARIS_TRUSTED_PROXIES`) the right-most
/// `X-Forwarded-For` entry that is not itself a trusted proxy (the one the
/// outermost trusted proxy saw); otherwise the TCP peer. Entries left of
/// that are client-supplied and not trusted.
pub fn client_ip(peer: std::net::IpAddr, headers: &axum::http::HeaderMap) -> std::net::IpAddr {
    client_ip_with(peer, headers, &TRUSTED_PROXIES)
}

pub fn client_ip_with(peer: std::net::IpAddr, headers: &axum::http::HeaderMap, trusted: &[Cidr]) -> std::net::IpAddr {
    if !is_trusted_proxy(peer, trusted) {
        return peer;
    }
    let entries: Vec<&str> = headers
        .get_all("x-forwarded-for")
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(','))
        .collect();
    let mut found = None;
    for entry in entries.iter().rev() {
        let Ok(ip) = entry.trim().parse::<std::net::IpAddr>() else {
            // Garbage where the proxy's entry should be: don't guess further left.
            break;
        };
        found = Some(ip);
        if !is_trusted_proxy(ip, trusted) {
            break;
        }
    }
    found.unwrap_or(peer)
}

/// The key a client address counts under in the failure limits: IPv4 as
/// is, IPv6 by its /64 (one subscriber usually has a whole /64, so one
/// address each would be no limit at all).
pub fn limit_key(ip: std::net::IpAddr) -> String {
    match canonical_ip(ip) {
        std::net::IpAddr::V4(v4) => v4.to_string(),
        std::net::IpAddr::V6(v6) => {
            let s = v6.segments();
            format!("{:x}:{:x}:{:x}:{:x}::/64", s[0], s[1], s[2], s[3])
        }
    }
}

/// `client_ip` as a failure-limit key (`limit_key`).
pub fn client_limit_key(peer: std::net::IpAddr, headers: &axum::http::HeaderMap) -> String {
    limit_key(client_ip(peer, headers))
}

/// Longest agent id / name accepted (bytes).
pub const MAX_AGENT_FIELD: usize = 128;

/// An agent's own key: at least 32 hex characters (and not absurdly long).
pub fn valid_agent_key(key: &str) -> bool {
    (32..=256).contains(&key.len()) && key.bytes().all(|b| b.is_ascii_hexdigit())
}

/// Checks the id and name an agent registers with.
pub fn validate_agent_identity(id: &str, name: &str) -> Result<(), &'static str> {
    if id.trim().is_empty() {
        return Err("empty agent id");
    }
    if id.len() > MAX_AGENT_FIELD || name.len() > MAX_AGENT_FIELD {
        return Err("agent id or name too long");
    }
    Ok(())
}

/// What an agent's registration does with its key.
#[derive(Debug, PartialEq, Eq)]
pub enum AgentKeyDecision {
    Accept,
    /// First key seen for this host: remember it.
    Store,
    Reject,
}

/// `stored`: SHA-256 hex kept for the host; `given`: the key the agent sent.
pub fn agent_key_decision(stored: Option<&str>, given: Option<&str>) -> AgentKeyDecision {
    match (stored, given) {
        (Some(hash), Some(key)) if ct_eq(hash, &sha256_hex(key)) => AgentKeyDecision::Accept,
        (Some(_), _) => AgentKeyDecision::Reject,
        (None, Some(_)) => AgentKeyDecision::Store,
        // Agents from before per-agent keys.
        (None, None) => AgentKeyDecision::Accept,
    }
}

/// Counts failures per key (client address, username) in a fixed window and
/// blocks the key once it reaches the limit, until the window has passed.
///
/// Checks that await (database, bcrypt) take an attempt first with
/// `try_begin`: attempts still being checked count like failures, under the
/// same lock, so many requests sent at once can't all pass `is_blocked`
/// before the first failure is written down. A full table never forgets a
/// blocked key or one with an attempt under way: the oldest other entry goes,
/// and when there is none, new keys are refused (fail closed) until the
/// window frees entries.
pub struct FailureLimiter {
    max_failures: u32,
    window: Duration,
    max_entries: usize,
    entries: Mutex<HashMap<String, LimitEntry>>,
}

#[derive(Debug, Clone, Copy)]
struct LimitEntry {
    failures: u32,
    start: Instant,
    in_flight: u32,
}

/// An attempt taken with `FailureLimiter::try_begin`: `fail` counts it as a
/// failure, `succeed` (or dropping it, e.g. on a database error) gives it back.
#[must_use]
pub struct Attempt<'a> {
    limiter: &'a FailureLimiter,
    key: String,
    done: bool,
}

impl Attempt<'_> {
    pub fn fail(mut self) {
        self.done = true;
        self.limiter.finish(&self.key, true, Instant::now());
    }

    pub fn succeed(mut self) {
        self.done = true;
        self.limiter.finish(&self.key, false, Instant::now());
    }
}

impl Drop for Attempt<'_> {
    fn drop(&mut self) {
        if !self.done {
            self.limiter.finish(&self.key, false, Instant::now());
        }
    }
}

impl FailureLimiter {
    pub fn new(max_failures: u32, window: Duration, max_entries: usize) -> Self {
        Self { max_failures, window, max_entries, entries: Mutex::new(HashMap::new()) }
    }

    /// Ten failures in five minutes, for logins and agent tokens.
    pub fn standard() -> Self {
        Self::new(10, Duration::from_secs(300), 10_000)
    }

    fn fresh(&self, e: &LimitEntry, now: Instant) -> bool {
        now.duration_since(e.start) < self.window
    }

    fn blocked(&self, e: &LimitEntry, now: Instant) -> bool {
        self.fresh(e, now) && e.failures >= self.max_failures
    }

    pub fn is_blocked(&self, key: &str, now: Instant) -> bool {
        let entries = self.entries.lock().unwrap();
        entries.get(key).is_some_and(|e| self.blocked(e, now))
    }

    /// Makes room for a new key; false when the table is full of entries
    /// that must be kept (blocked, or with attempts under way).
    fn make_room(&self, entries: &mut HashMap<String, LimitEntry>, now: Instant) -> bool {
        if entries.len() < self.max_entries {
            return true;
        }
        entries.retain(|_, e| self.fresh(e, now) || e.in_flight > 0);
        if entries.len() < self.max_entries {
            return true;
        }
        // Still full (many addresses at once): forget the oldest entry that
        // neither blocks nor has an attempt under way.
        let oldest = entries
            .iter()
            .filter(|(_, e)| !self.blocked(e, now) && e.in_flight == 0)
            .min_by_key(|(_, e)| e.start)
            .map(|(k, _)| k.clone());
        match oldest {
            Some(k) => {
                entries.remove(&k);
                true
            }
            None => false,
        }
    }

    /// Takes an attempt for `key`, or None when the key is blocked: its
    /// failures and the attempts still under way reached the limit.
    pub fn try_begin(&self, key: &str, now: Instant) -> Option<Attempt<'_>> {
        let mut entries = self.entries.lock().unwrap();
        if !entries.contains_key(key) && !self.make_room(&mut entries, now) {
            tracing::warn!("Failure limit table full of blocked addresses: refusing new ones for now");
            return None;
        }
        let entry = entries.entry(key.to_string()).or_insert(LimitEntry { failures: 0, start: now, in_flight: 0 });
        if !self.fresh(entry, now) {
            entry.failures = 0;
            entry.start = now;
        }
        if entry.failures + entry.in_flight >= self.max_failures {
            return None;
        }
        entry.in_flight += 1;
        Some(Attempt { limiter: self, key: key.to_string(), done: false })
    }

    fn finish(&self, key: &str, failed: bool, now: Instant) {
        let mut entries = self.entries.lock().unwrap();
        let Some(entry) = entries.get_mut(key) else { return };
        entry.in_flight = entry.in_flight.saturating_sub(1);
        if failed {
            if !self.fresh(entry, now) {
                entry.failures = 0;
                entry.start = now;
            }
            entry.failures += 1;
        }
    }

    pub fn record_failure(&self, key: &str, now: Instant) {
        let mut entries = self.entries.lock().unwrap();
        if !entries.contains_key(key) && !self.make_room(&mut entries, now) {
            return;
        }
        let entry = entries.entry(key.to_string()).or_insert(LimitEntry { failures: 0, start: now, in_flight: 0 });
        if !self.fresh(entry, now) {
            entry.failures = 0;
            entry.start = now;
        }
        entry.failures += 1;
    }

    /// Forgets the failures of `key` (a success); attempts under way stay counted.
    pub fn reset(&self, key: &str) {
        let mut entries = self.entries.lock().unwrap();
        if let Some(e) = entries.get_mut(key) {
            if e.in_flight == 0 {
                entries.remove(key);
            } else {
                e.failures = 0;
            }
        }
    }

    #[cfg(test)]
    fn len(&self) -> usize {
        self.entries.lock().unwrap().len()
    }
}

/// Client addresses a user recently signed in from (in memory only): the
/// per-username sign-in limit (lib.rs `login_handler`) does not lock the
/// user out from those, so someone guessing one account's password from
/// many addresses can't keep its owner out.
pub struct KnownAddresses {
    ttl: Duration,
    per_user: usize,
    max_users: usize,
    entries: Mutex<HashMap<String, Vec<(String, Instant)>>>,
}

impl KnownAddresses {
    pub fn new(ttl: Duration, per_user: usize, max_users: usize) -> Self {
        Self { ttl, per_user, max_users, entries: Mutex::new(HashMap::new()) }
    }

    pub fn remember(&self, user: &str, addr: &str, now: Instant) {
        let mut entries = self.entries.lock().unwrap();
        if !entries.contains_key(user) && entries.len() >= self.max_users {
            let ttl = self.ttl;
            entries.retain(|_, list| {
                list.retain(|(_, t)| now.duration_since(*t) < ttl);
                !list.is_empty()
            });
            if entries.len() >= self.max_users {
                return;
            }
        }
        let list = entries.entry(user.to_string()).or_default();
        list.retain(|(a, t)| a != addr && now.duration_since(*t) < self.ttl);
        list.push((addr.to_string(), now));
        if list.len() > self.per_user {
            list.remove(0);
        }
    }

    pub fn knows(&self, user: &str, addr: &str, now: Instant) -> bool {
        self.entries
            .lock()
            .unwrap()
            .get(user)
            .is_some_and(|list| list.iter().any(|(a, t)| a == addr && now.duration_since(*t) < self.ttl))
    }
}

/// Hosts a user may see and use.
#[derive(Debug)]
pub enum HostAccess {
    /// Admins: every host.
    All,
    /// Hosts in one of the user's groups.
    Some(HashSet<String>),
    /// Unknown or deactivated user.
    None,
}

impl HostAccess {
    pub fn allows(&self, host_id: &str) -> bool {
        match self {
            HostAccess::All => true,
            HostAccess::Some(ids) => ids.contains(host_id),
            HostAccess::None => false,
        }
    }

    pub fn is_all(&self) -> bool {
        matches!(self, HostAccess::All)
    }
}

/// The user's current role and active flag (from the database, not the token).
pub async fn user_status(db: &SqlitePool, user_id: &str) -> Result<Option<(String, bool)>, sqlx::Error> {
    let row: Option<(String, i64)> =
        sqlx::query_as("SELECT COALESCE(role, 'user'), is_active FROM users WHERE id = ?")
            .bind(user_id)
            .fetch_optional(db)
            .await?;
    Ok(row.map(|(role, active)| (role, active != 0)))
}

/// Which hosts `user_id` may use: admins all of them, others the hosts of
/// their groups.
pub async fn host_access(db: &SqlitePool, user_id: &str) -> Result<HostAccess, sqlx::Error> {
    match user_status(db, user_id).await? {
        Some((_, false)) | None => Ok(HostAccess::None),
        Some((role, true)) if role == "admin" => Ok(HostAccess::All),
        Some(_) => {
            let ids: Vec<String> = sqlx::query_scalar(
                "SELECT DISTINCT hg.host_id FROM host_groups hg
                 INNER JOIN user_groups ug ON hg.group_id = ug.group_id
                 WHERE ug.user_id = ?",
            )
            .bind(user_id)
            .fetch_all(db)
            .await?;
            Ok(HostAccess::Some(ids.into_iter().collect()))
        }
    }
}

/// Whether `user_id` may use `host_id` (false on database errors).
pub async fn may_use_host(db: &SqlitePool, user_id: &str, host_id: &str) -> bool {
    match host_access(db, user_id).await {
        Ok(access) => access.allows(host_id),
        Err(e) => {
            tracing::error!("Database error checking host access: {}", e);
            false
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A fresh database file with the server's schema (pooled connections
    /// would each get their own `:memory:` database).
    pub(crate) async fn test_db() -> SqlitePool {
        let dir = std::env::temp_dir().join(format!("lunaris-test-{}", random_hex(8)));
        std::fs::create_dir_all(&dir).unwrap();
        let url = format!("sqlite://{}", dir.join("test.db").display());
        crate::db::init_db(&url).await.unwrap()
    }

    async fn add_user(db: &SqlitePool, id: &str, role: &str, active: bool) {
        sqlx::query("INSERT INTO users (id, username, password_hash, role, is_active) VALUES (?, ?, 'x', ?, ?)")
            .bind(id)
            .bind(format!("u-{id}"))
            .bind(role)
            .bind(active as i64)
            .execute(db)
            .await
            .unwrap();
    }

    #[test]
    fn agent_keys() {
        let key = "0123456789abcdef0123456789ABCDEF";
        assert!(valid_agent_key(key));
        assert!(!valid_agent_key("0123456789abcdef"));
        assert!(!valid_agent_key("zz23456789abcdef0123456789abcdef"));
        let hash = sha256_hex(key);
        assert_eq!(agent_key_decision(None, None), AgentKeyDecision::Accept);
        assert_eq!(agent_key_decision(None, Some(key)), AgentKeyDecision::Store);
        assert_eq!(agent_key_decision(Some(&hash), Some(key)), AgentKeyDecision::Accept);
        assert_eq!(
            agent_key_decision(Some(&hash), Some("ffffffffffffffffffffffffffffffff")),
            AgentKeyDecision::Reject
        );
        // A host with a key can't be taken over by an agent without one.
        assert_eq!(agent_key_decision(Some(&hash), None), AgentKeyDecision::Reject);
    }

    #[test]
    fn client_ip_behind_local_proxy() {
        use axum::http::{HeaderMap, HeaderValue};
        let mut h = HeaderMap::new();
        h.append("x-forwarded-for", HeaderValue::from_static("6.6.6.6, 203.0.113.9"));
        let lo: std::net::IpAddr = "127.0.0.1".parse().unwrap();
        let remote: std::net::IpAddr = "198.51.100.4".parse().unwrap();
        // Local proxy: the address it appended (right-most), not the spoofable left one.
        assert_eq!(client_ip(lo, &h).to_string(), "203.0.113.9");
        assert_eq!(client_ip("::ffff:127.0.0.1".parse().unwrap(), &h).to_string(), "203.0.113.9");
        // Several header lines: the last line's last entry.
        h.append("x-forwarded-for", HeaderValue::from_static("192.0.2.7"));
        assert_eq!(client_ip(lo, &h).to_string(), "192.0.2.7");
        // Not from loopback: the header is ignored.
        assert_eq!(client_ip(remote, &h), remote);
        // Loopback without (or with garbage in) the header: the peer.
        assert_eq!(client_ip(lo, &HeaderMap::new()), lo);
        let mut bad = HeaderMap::new();
        bad.insert("x-forwarded-for", HeaderValue::from_static("nonsense"));
        assert_eq!(client_ip(lo, &bad), lo);
    }

    #[test]
    fn agent_identity_limits() {
        assert!(validate_agent_identity("pc-1", "PC").is_ok());
        assert!(validate_agent_identity("", "PC").is_err());
        assert!(validate_agent_identity("  ", "PC").is_err());
        assert!(validate_agent_identity(&"a".repeat(129), "PC").is_err());
        assert!(validate_agent_identity("pc", &"n".repeat(129)).is_err());
        assert!(validate_agent_identity(&"a".repeat(128), &"n".repeat(128)).is_ok());
    }

    #[test]
    fn constant_time_eq_and_random() {
        assert!(ct_eq("abc", "abc"));
        assert!(!ct_eq("abc", "abd"));
        assert!(!ct_eq("abc", "abcd"));
        let a = random_hex(16);
        assert_eq!(a.len(), 32);
        assert_ne!(a, random_hex(16));
    }

    #[test]
    fn failure_limit() {
        let limiter = FailureLimiter::new(10, Duration::from_secs(300), 100);
        let t0 = Instant::now();
        for _ in 0..9 {
            limiter.record_failure("ip:1.2.3.4", t0);
        }
        assert!(!limiter.is_blocked("ip:1.2.3.4", t0));
        limiter.record_failure("ip:1.2.3.4", t0);
        assert!(limiter.is_blocked("ip:1.2.3.4", t0 + Duration::from_secs(299)));
        assert!(!limiter.is_blocked("ip:5.6.7.8", t0));
        // The window passed: allowed again, and counting starts over.
        assert!(!limiter.is_blocked("ip:1.2.3.4", t0 + Duration::from_secs(300)));
        limiter.record_failure("ip:1.2.3.4", t0 + Duration::from_secs(301));
        assert!(!limiter.is_blocked("ip:1.2.3.4", t0 + Duration::from_secs(301)));
        limiter.reset("ip:1.2.3.4");
        assert!(!limiter.is_blocked("ip:1.2.3.4", t0));
    }

    #[test]
    fn failure_limit_is_bounded() {
        let limiter = FailureLimiter::new(3, Duration::from_secs(60), 10);
        let t0 = Instant::now();
        for i in 0..50 {
            limiter.record_failure(&format!("k{i}"), t0 + Duration::from_millis(i));
        }
        assert!(limiter.len() <= 10);
        // Old entries make room once their window passed.
        limiter.record_failure("late", t0 + Duration::from_secs(120));
        assert!(limiter.len() <= 10);
    }

    /// Attempts under way count like failures: requests sent at once can't
    /// all get past the check before the first failure is written down.
    #[test]
    fn attempts_under_way_count() {
        let limiter = FailureLimiter::new(3, Duration::from_secs(60), 100);
        let t0 = Instant::now();
        let a = limiter.try_begin("k", t0).unwrap();
        let b = limiter.try_begin("k", t0).unwrap();
        let c = limiter.try_begin("k", t0).unwrap();
        assert!(limiter.try_begin("k", t0).is_none(), "three under way: the fourth waits");
        assert!(limiter.try_begin("other", t0).is_some());
        // A success (or an error: dropped) gives the attempt back.
        a.succeed();
        let d = limiter.try_begin("k", t0).unwrap();
        b.fail();
        c.fail();
        d.fail();
        assert!(limiter.is_blocked("k", t0));
        assert!(limiter.try_begin("k", t0).is_none());
        // The window passed: allowed again.
        assert!(limiter.try_begin("k", t0 + Duration::from_secs(61)).is_some());
    }

    #[tokio::test]
    async fn concurrent_attempts_at_most_the_limit_are_evaluated() {
        let limiter = std::sync::Arc::new(FailureLimiter::new(10, Duration::from_secs(300), 100));
        let evaluated = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let tasks = (0..50).map(|_| {
            let (limiter, evaluated) = (limiter.clone(), evaluated.clone());
            async move {
                let Some(attempt) = limiter.try_begin("ip", Instant::now()) else { return };
                evaluated.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                tokio::time::sleep(Duration::from_millis(20)).await;
                attempt.fail();
            }
        });
        futures_util::future::join_all(tasks).await;
        assert_eq!(evaluated.load(std::sync::atomic::Ordering::SeqCst), 10);
        assert!(limiter.is_blocked("ip", Instant::now()));
    }

    /// A full table never forgets a blocked key (that would unblock it).
    #[test]
    fn full_table_keeps_blocked_keys() {
        let limiter = FailureLimiter::new(2, Duration::from_secs(60), 3);
        let t0 = Instant::now();
        for k in ["a", "b", "c"] {
            limiter.record_failure(k, t0);
            limiter.record_failure(k, t0);
        }
        // Full of blocked keys: a new one is refused, the blocked stay blocked.
        assert!(limiter.try_begin("d", t0).is_none());
        limiter.record_failure("d", t0);
        for k in ["a", "b", "c"] {
            assert!(limiter.is_blocked(k, t0), "{k}");
        }
        // One of them not blocked: that one makes room.
        limiter.reset("b");
        limiter.record_failure("b", t0);
        assert!(limiter.try_begin("d", t0).is_some());
        assert!(limiter.is_blocked("a", t0) && limiter.is_blocked("c", t0));
    }

    #[test]
    fn ipv6_counts_by_64() {
        let a: std::net::IpAddr = "2001:db8:1:2:aaaa::1".parse().unwrap();
        let b: std::net::IpAddr = "2001:db8:1:2:ffff::9".parse().unwrap();
        let c: std::net::IpAddr = "2001:db8:1:3::1".parse().unwrap();
        assert_eq!(limit_key(a), limit_key(b));
        assert_ne!(limit_key(a), limit_key(c));
        assert_eq!(limit_key("10.1.2.3".parse().unwrap()), "10.1.2.3");
        assert_eq!(limit_key("::ffff:10.1.2.3".parse().unwrap()), "10.1.2.3");
    }

    #[test]
    fn trusted_proxies() {
        use axum::http::{HeaderMap, HeaderValue};
        let trusted = parse_trusted_proxies("172.17.0.0/16, 10.9.9.9, bogus, fd00::/8");
        assert_eq!(trusted.len(), 3);
        let mut h = HeaderMap::new();
        h.insert("x-forwarded-for", HeaderValue::from_static("6.6.6.6, 203.0.113.9"));
        let docker: std::net::IpAddr = "172.17.0.1".parse().unwrap();
        // A proxy on the Docker bridge is believed when listed...
        assert_eq!(client_ip_with(docker, &h, &trusted).to_string(), "203.0.113.9");
        // ...not otherwise.
        assert_eq!(client_ip_with(docker, &h, &[]), docker);
        // Proxies in a chain are skipped from the right; the spoofable
        // left-most entry is never reached.
        let mut chain = HeaderMap::new();
        chain.insert("x-forwarded-for", HeaderValue::from_static("6.6.6.6, 203.0.113.9, 10.9.9.9"));
        assert_eq!(client_ip_with("127.0.0.1".parse().unwrap(), &chain, &trusted).to_string(), "203.0.113.9");
        assert!(Cidr::parse("10.0.0.0/33").is_none());
        assert!(Cidr::parse("0.0.0.0/0").unwrap().contains("8.8.8.8".parse().unwrap()));
        assert!(Cidr::parse("fd00::/8").unwrap().contains("fd12::1".parse().unwrap()));
        assert!(!Cidr::parse("fd00::/8").unwrap().contains("fe80::1".parse().unwrap()));
    }

    #[test]
    fn known_addresses_expire_and_are_bounded() {
        let known = KnownAddresses::new(Duration::from_secs(100), 2, 2);
        let t0 = Instant::now();
        known.remember("alice", "1.1.1.1", t0);
        known.remember("alice", "2.2.2.2", t0);
        known.remember("alice", "3.3.3.3", t0);
        assert!(!known.knows("alice", "1.1.1.1", t0), "only the latest two");
        assert!(known.knows("alice", "3.3.3.3", t0));
        assert!(!known.knows("alice", "3.3.3.3", t0 + Duration::from_secs(100)));
        known.remember("bob", "1.1.1.1", t0);
        known.remember("carol", "1.1.1.1", t0);
        assert!(!known.knows("carol", "1.1.1.1", t0), "table full");
    }

    #[tokio::test]
    async fn host_access_rule() {
        let db = test_db().await;
        add_user(&db, "member", "user", true).await;
        add_user(&db, "outsider", "user", true).await;
        add_user(&db, "boss", "admin", true).await;
        add_user(&db, "gone", "user", false).await;
        add_user(&db, "gone-admin", "admin", false).await;
        sqlx::query("INSERT INTO hosts (id, name, status) VALUES ('h1', 'H1', 'Online'), ('h2', 'H2', 'Online')")
            .execute(&db)
            .await
            .unwrap();
        sqlx::query("INSERT INTO groups (id, name) VALUES ('g1', 'G1')").execute(&db).await.unwrap();
        sqlx::query("INSERT INTO host_groups (host_id, group_id) VALUES ('h1', 'g1')").execute(&db).await.unwrap();
        sqlx::query("INSERT INTO user_groups (user_id, group_id) VALUES ('member', 'g1'), ('gone', 'g1')")
            .execute(&db)
            .await
            .unwrap();

        assert!(may_use_host(&db, "member", "h1").await);
        assert!(!may_use_host(&db, "member", "h2").await);
        assert!(!may_use_host(&db, "outsider", "h1").await);
        assert!(may_use_host(&db, "boss", "h1").await);
        assert!(may_use_host(&db, "boss", "h2").await);
        assert!(may_use_host(&db, "boss", "address:pc.lan:8091").await);
        assert!(!may_use_host(&db, "member", "address:pc.lan:8091").await);
        // Deactivated or unknown users: nothing.
        assert!(!may_use_host(&db, "gone", "h1").await);
        assert!(!may_use_host(&db, "gone-admin", "h1").await);
        assert!(!may_use_host(&db, "nobody", "h1").await);
    }
}
