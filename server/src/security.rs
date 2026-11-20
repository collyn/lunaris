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

/// The address a request comes from, for rate limits: behind a reverse proxy
/// on this machine (TCP peer is loopback) the right-most `X-Forwarded-For`
/// entry, which that proxy appended; otherwise the TCP peer. Left-most
/// entries are client-supplied and not trusted.
pub fn client_ip(peer: std::net::IpAddr, headers: &axum::http::HeaderMap) -> std::net::IpAddr {
    let loopback = match peer {
        std::net::IpAddr::V4(v4) => v4.is_loopback(),
        std::net::IpAddr::V6(v6) => v6.is_loopback() || v6.to_ipv4_mapped().is_some_and(|v4| v4.is_loopback()),
    };
    if !loopback {
        return peer;
    }
    headers
        .get_all("x-forwarded-for")
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(','))
        .last()
        .and_then(|last| last.trim().parse().ok())
        .unwrap_or(peer)
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
pub struct FailureLimiter {
    max_failures: u32,
    window: Duration,
    max_entries: usize,
    entries: Mutex<HashMap<String, (u32, Instant)>>,
}

impl FailureLimiter {
    pub fn new(max_failures: u32, window: Duration, max_entries: usize) -> Self {
        Self { max_failures, window, max_entries, entries: Mutex::new(HashMap::new()) }
    }

    /// Ten failures in five minutes, for logins and agent tokens.
    pub fn standard() -> Self {
        Self::new(10, Duration::from_secs(300), 10_000)
    }

    pub fn is_blocked(&self, key: &str, now: Instant) -> bool {
        let entries = self.entries.lock().unwrap();
        matches!(entries.get(key), Some((count, start))
            if *count >= self.max_failures && now.duration_since(*start) < self.window)
    }

    pub fn record_failure(&self, key: &str, now: Instant) {
        let mut entries = self.entries.lock().unwrap();
        if entries.len() >= self.max_entries && !entries.contains_key(key) {
            let window = self.window;
            entries.retain(|_, (_, start)| now.duration_since(*start) < window);
            // Still full (many addresses at once): forget the oldest.
            if entries.len() >= self.max_entries {
                if let Some(oldest) = entries.iter().min_by_key(|(_, (_, s))| *s).map(|(k, _)| k.clone()) {
                    entries.remove(&oldest);
                }
            }
        }
        let entry = entries.entry(key.to_string()).or_insert((0, now));
        if now.duration_since(entry.1) >= self.window {
            *entry = (0, now);
        }
        entry.0 += 1;
    }

    pub fn reset(&self, key: &str) {
        self.entries.lock().unwrap().remove(key);
    }

    #[cfg(test)]
    fn len(&self) -> usize {
        self.entries.lock().unwrap().len()
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
