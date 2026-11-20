//! One-time tickets for `lunaris://connect` links. The web page asks for a
//! ticket for one computer and puts it in the link instead of its sign-in
//! token (a link ends up on a command line, where other processes and logs
//! see it). The desktop app trades the ticket for a token that opens only
//! that computer (`auth::create_host_jwt`) and lasts one sitting.
//!
//! A ticket lasts a minute and works once; only its SHA-256 is stored.

use std::sync::Arc;

use axum::{
    extract::{ConnectInfo, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use serde::Deserialize;
use sqlx::SqlitePool;
use tracing::{error, info};

use crate::{
    auth::AuthenticatedUser,
    security,
    signaling::{ConnAddrs, SignalingState},
};

pub const TICKET_PREFIX: &str = "lct_";
/// Seconds a ticket can wait for the desktop app.
pub const TICKET_TTL_SECS: i64 = 60;
/// Unused tickets one user may have at a time.
const MAX_LIVE_TICKETS_PER_USER: i64 = 20;
/// Longer input is not a ticket (prefix + 64 hex characters).
const MAX_TICKET_LEN: usize = 128;

const EXPIRED: &str = "This link has expired or was already used. Open it again from the web page.";

#[derive(Deserialize)]
pub struct TicketRequest {
    pub host_id: String,
    /// A source of that host (sources.rs); none = the host's own screen.
    #[serde(default)]
    pub source_id: Option<String>,
}

#[derive(Deserialize)]
pub struct RedeemRequest {
    pub ticket: String,
}

fn error(status: StatusCode, message: &str) -> Response {
    (status, Json(serde_json::json!({ "error": message }))).into_response()
}

fn db_error(e: sqlx::Error) -> Response {
    error!("Database error: {}", e);
    error(StatusCode::INTERNAL_SERVER_ERROR, "Internal Database Error")
}

fn now_secs() -> i64 {
    chrono::Utc::now().timestamp()
}

/// A new ticket: the prefix and 32 random bytes as hex.
fn new_ticket() -> String {
    format!("{TICKET_PREFIX}{}", security::random_hex(32))
}

async fn purge_expired(db: &SqlitePool, now: i64) -> Result<(), sqlx::Error> {
    sqlx::query("DELETE FROM link_tickets WHERE expires_at <= ?").bind(now).execute(db).await?;
    Ok(())
}

/// POST /api/links/ticket `{host_id, source_id?}` → `{ticket}`: for a
/// computer the signed-in user may use, or one of its sources.
pub async fn create_ticket(
    user: AuthenticatedUser,
    State(state): State<Arc<SignalingState>>,
    Json(req): Json<TicketRequest>,
) -> Response {
    let host_id = req.host_id.trim();
    if host_id.is_empty() || host_id.len() > security::MAX_AGENT_FIELD {
        return error(StatusCode::BAD_REQUEST, "Not a valid computer");
    }
    let source_id = req.source_id.as_deref().map(str::trim).filter(|s| !s.is_empty());
    if source_id.is_some_and(|s| s.len() > security::MAX_AGENT_FIELD) {
        return error(StatusCode::BAD_REQUEST, "Not a valid computer");
    }
    if !security::may_use_host(&state.db, &user.user_id, host_id).await {
        return error(StatusCode::FORBIDDEN, "You don't have access to this computer.");
    }
    // A source the host has (its sources are open to whoever may use it).
    if let Some(source_id) = source_id {
        let known: Result<Option<i64>, _> =
            sqlx::query_scalar("SELECT 1 FROM host_sources WHERE host_id = ? AND source_id = ?")
                .bind(host_id)
                .bind(source_id)
                .fetch_optional(&state.db)
                .await;
        match known {
            Ok(Some(_)) => {}
            Ok(None) => return error(StatusCode::NOT_FOUND, "No such computer"),
            Err(e) => return db_error(e),
        }
    }
    match issue(&state.db, &user.user_id, host_id, source_id, now_secs()).await {
        Ok(Some(ticket)) => (StatusCode::OK, Json(serde_json::json!({ "ticket": ticket }))).into_response(),
        Ok(None) => error(StatusCode::TOO_MANY_REQUESTS, "Too many links opened at once. Try again in a minute."),
        Err(e) => db_error(e),
    }
}

/// Stores a ticket for `user_id` and `host_id` (`source_id` of it); None
/// when the user already has too many unused ones.
async fn issue(
    db: &SqlitePool,
    user_id: &str,
    host_id: &str,
    source_id: Option<&str>,
    now: i64,
) -> Result<Option<String>, sqlx::Error> {
    purge_expired(db, now).await?;
    let live: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM link_tickets WHERE user_id = ?")
        .bind(user_id)
        .fetch_one(db)
        .await?;
    if live >= MAX_LIVE_TICKETS_PER_USER {
        return Ok(None);
    }
    let version = crate::auth::token_version(db, user_id).await?;
    let ticket = new_ticket();
    sqlx::query(
        "INSERT INTO link_tickets (ticket_hash, user_id, host_id, source_id, token_version, expires_at)
         VALUES (?, ?, ?, ?, ?, ?)",
    )
    .bind(security::sha256_hex(&ticket))
    .bind(user_id)
    .bind(host_id)
    .bind(source_id)
    .bind(version)
    .bind(now + TICKET_TTL_SECS)
    .execute(db)
    .await?;
    Ok(Some(ticket))
}

/// What a good ticket was for.
#[derive(Debug, PartialEq, Eq)]
struct Redeemed {
    user_id: String,
    host_id: String,
    source_id: Option<String>,
    token_version: i64,
}

/// Uses up a ticket (whether or not it is still good: it is gone either way).
async fn consume(db: &SqlitePool, ticket: &str, now: i64) -> Result<Option<Redeemed>, sqlx::Error> {
    purge_expired(db, now).await?;
    let row: Option<(String, String, Option<String>, i64, i64)> = sqlx::query_as(
        "DELETE FROM link_tickets WHERE ticket_hash = ? RETURNING user_id, host_id, source_id, token_version, expires_at",
    )
    .bind(security::sha256_hex(ticket))
    .fetch_optional(db)
    .await?;
    Ok(row
        .filter(|(.., expires_at)| *expires_at > now)
        .map(|(user_id, host_id, source_id, token_version, _)| Redeemed { user_id, host_id, source_id, token_version }))
}

/// POST /api/links/redeem `{ticket}` → `{token}` (no sign-in: the ticket is
/// the proof). The token opens only the ticket's computer (or source).
pub async fn redeem_ticket(
    State(state): State<Arc<SignalingState>>,
    ConnectInfo(addrs): ConnectInfo<ConnAddrs>,
    headers: axum::http::HeaderMap,
    Json(req): Json<RedeemRequest>,
) -> Response {
    let instant = std::time::Instant::now();
    let ip_key = security::client_ip(addrs.remote.ip(), &headers).to_string();
    if state.link_ticket_limit.is_blocked(&ip_key, instant) {
        return error(StatusCode::TOO_MANY_REQUESTS, "Too many tries. Try again in a few minutes.");
    }
    let ticket = req.ticket.trim();
    let well_formed = ticket.len() <= MAX_TICKET_LEN && ticket.starts_with(TICKET_PREFIX);
    let redeemed = if well_formed {
        match consume(&state.db, ticket, now_secs()).await {
            Ok(r) => r,
            Err(e) => return db_error(e),
        }
    } else {
        None
    };
    let Some(r) = redeemed else {
        state.link_ticket_limit.record_failure(&ip_key, instant);
        return error(StatusCode::UNAUTHORIZED, EXPIRED);
    };

    // The account as login checks it: it must still exist and be active,
    // and nothing may have ended its sessions (new password, two-factor
    // turned off or reset) since the ticket was made. Two-factor sign-in
    // itself was passed by the session that asked for the ticket.
    let user: Option<(String, String, i64, i64)> = match sqlx::query_as(
        "SELECT username, COALESCE(role, 'user'), is_active, token_version FROM users WHERE id = ?",
    )
    .bind(&r.user_id)
    .fetch_optional(&state.db)
    .await
    {
        Ok(u) => u,
        Err(e) => return db_error(e),
    };
    let Some((username, role, _, version)) =
        user.filter(|(_, _, active, version)| *active != 0 && *version == r.token_version)
    else {
        return error(StatusCode::UNAUTHORIZED, EXPIRED);
    };
    // Access to the computer may have been taken away in the meantime.
    if !security::may_use_host(&state.db, &r.user_id, &r.host_id).await {
        return error(StatusCode::FORBIDDEN, "You don't have access to this computer.");
    }
    match crate::auth::create_host_jwt(&r.user_id, &username, &role, version, &r.host_id, r.source_id.as_deref()) {
        Ok(token) => {
            info!("Link ticket used by {} for computer {}", username, r.host_id);
            (StatusCode::OK, Json(serde_json::json!({ "token": token }))).into_response()
        }
        Err(e) => {
            error!("Token error: {}", e);
            error(StatusCode::INTERNAL_SERVER_ERROR, "Failed to generate token")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::extract::FromRequestParts;

    async fn state() -> Arc<SignalingState> {
        let db = security::tests::test_db().await;
        for (id, name, role, active) in [("u1", "alice", "user", 1), ("u2", "carol", "user", 1), ("boss", "boss", "admin", 1)] {
            sqlx::query("INSERT INTO users (id, username, password_hash, role, is_active) VALUES (?, ?, 'x', ?, ?)")
                .bind(id)
                .bind(name)
                .bind(role)
                .bind(active)
                .execute(&db)
                .await
                .unwrap();
        }
        for sql in [
            "INSERT INTO hosts (id, name, status) VALUES ('h1', 'H1', 'Online'), ('h2', 'H2', 'Online')",
            "INSERT INTO groups (id, name) VALUES ('g1', 'G1')",
            "INSERT INTO host_groups (host_id, group_id) VALUES ('h1', 'g1')",
            "INSERT INTO user_groups (user_id, group_id) VALUES ('u1', 'g1')",
            "INSERT INTO host_sources (host_id, source_id, name, connector, connector_name) VALUES ('h1', 's1', 'S1', 'rdp', 'RDP')",
        ] {
            sqlx::query(sql).execute(&db).await.unwrap();
        }
        Arc::new(SignalingState::new(db, "t".into(), None))
    }

    fn user(id: &str) -> AuthenticatedUser {
        AuthenticatedUser { user_id: id.into(), username: id.into(), role: "user".into() }
    }

    async fn ask(state: &Arc<SignalingState>, uid: &str, host: &str) -> (StatusCode, serde_json::Value) {
        ask_source(state, uid, host, None).await
    }

    async fn ask_source(state: &Arc<SignalingState>, uid: &str, host: &str, source: Option<&str>) -> (StatusCode, serde_json::Value) {
        let req = TicketRequest { host_id: host.into(), source_id: source.map(str::to_string) };
        body(create_ticket(user(uid), State(state.clone()), Json(req)).await).await
    }

    async fn body(r: Response) -> (StatusCode, serde_json::Value) {
        let status = r.status();
        let bytes = axum::body::to_bytes(r.into_body(), 1 << 20).await.unwrap();
        (status, serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null))
    }

    async fn redeem_from(state: &Arc<SignalingState>, ip: &str, ticket: &str) -> (StatusCode, serde_json::Value) {
        let addrs = ConnAddrs { local: None, remote: format!("{ip}:5000").parse().unwrap() };
        let r = redeem_ticket(
            State(state.clone()),
            ConnectInfo(addrs),
            axum::http::HeaderMap::new(),
            Json(RedeemRequest { ticket: ticket.into() }),
        )
        .await;
        body(r).await
    }

    async fn redeem(state: &Arc<SignalingState>, ticket: &str) -> (StatusCode, serde_json::Value) {
        redeem_from(state, "192.0.2.1", ticket).await
    }

    async fn ticket_for(state: &Arc<SignalingState>, uid: &str, host: &str) -> String {
        let (status, b) = ask(state, uid, host).await;
        assert_eq!(status, StatusCode::OK, "{b}");
        b["ticket"].as_str().unwrap().to_string()
    }

    #[tokio::test]
    async fn issue_stores_only_a_hash_and_redeems_once() {
        let state = state().await;
        let ticket = ticket_for(&state, "u1", "h1").await;
        assert!(ticket.starts_with(TICKET_PREFIX));
        assert_eq!(ticket.len(), TICKET_PREFIX.len() + 64);
        assert!(ticket.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_'), "URL-safe");
        let stored: Vec<String> = sqlx::query_scalar("SELECT ticket_hash FROM link_tickets").fetch_all(&state.db).await.unwrap();
        assert_eq!(stored, vec![security::sha256_hex(&ticket)]);

        let (status, b) = redeem(&state, &ticket).await;
        assert_eq!(status, StatusCode::OK, "{b}");
        let claims = crate::auth::verify_jwt(b["token"].as_str().unwrap()).unwrap();
        assert_eq!(claims.sub, "u1");
        assert_eq!(claims.host.as_deref(), Some("h1"));
        assert_eq!(claims.source, None);

        // Single use.
        assert_eq!(redeem(&state, &ticket).await.0, StatusCode::UNAUTHORIZED);
        let left: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM link_tickets").fetch_one(&state.db).await.unwrap();
        assert_eq!(left, 0);
    }

    #[tokio::test]
    async fn a_source_ticket_names_that_source() {
        let state = state().await;
        let (status, b) = ask_source(&state, "u1", "h1", Some("s1")).await;
        assert_eq!(status, StatusCode::OK, "{b}");
        let (status, b) = redeem(&state, b["ticket"].as_str().unwrap()).await;
        assert_eq!(status, StatusCode::OK, "{b}");
        let claims = crate::auth::verify_jwt(b["token"].as_str().unwrap()).unwrap();
        assert_eq!((claims.host.as_deref(), claims.source.as_deref()), (Some("h1"), Some("s1")));
        // A source the host doesn't have, or of a host the user may not use.
        assert_eq!(ask_source(&state, "u1", "h1", Some("s9")).await.0, StatusCode::NOT_FOUND);
        assert_eq!(ask_source(&state, "u2", "h1", Some("s1")).await.0, StatusCode::FORBIDDEN);
        assert_eq!(ask_source(&state, "u1", "h1", Some(&"s".repeat(200))).await.0, StatusCode::BAD_REQUEST);
        // Blank = the host's own screen.
        assert_eq!(ask_source(&state, "u1", "h1", Some(" ")).await.0, StatusCode::OK);
    }

    #[tokio::test]
    async fn only_for_computers_the_user_may_use() {
        let state = state().await;
        assert_eq!(ask(&state, "u1", "h2").await.0, StatusCode::FORBIDDEN);
        assert_eq!(ask(&state, "u2", "h1").await.0, StatusCode::FORBIDDEN);
        assert_eq!(ask(&state, "u1", "").await.0, StatusCode::BAD_REQUEST);
        assert_eq!(ask(&state, "u1", &"h".repeat(200)).await.0, StatusCode::BAD_REQUEST);
        // Access taken away before the ticket is used.
        let ticket = ticket_for(&state, "u1", "h1").await;
        sqlx::query("DELETE FROM user_groups WHERE user_id = 'u1'").execute(&state.db).await.unwrap();
        assert_eq!(redeem(&state, &ticket).await.0, StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn expired_tickets_fail_and_are_purged() {
        let state = state().await;
        let now = now_secs();
        let old = issue(&state.db, "u1", "h1", None, now - TICKET_TTL_SECS - 1).await.unwrap().unwrap();
        let fresh = issue(&state.db, "u1", "h1", None, now).await.unwrap().unwrap();
        // Exactly at expiry it is no longer good.
        assert_eq!(consume(&state.db, &fresh, now + TICKET_TTL_SECS).await.unwrap(), None);
        assert_eq!(redeem(&state, &old).await.0, StatusCode::UNAUTHORIZED);
        let left: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM link_tickets").fetch_one(&state.db).await.unwrap();
        assert_eq!(left, 0);
        // A good one just before expiry.
        let t = issue(&state.db, "u1", "h1", Some("s1"), now).await.unwrap().unwrap();
        let r = consume(&state.db, &t, now + TICKET_TTL_SECS - 1).await.unwrap().unwrap();
        assert_eq!(r, Redeemed { user_id: "u1".into(), host_id: "h1".into(), source_id: Some("s1".into()), token_version: 0 });
    }

    #[tokio::test]
    async fn disabled_or_signed_out_users_cannot_redeem() {
        let state = state().await;
        let t = ticket_for(&state, "u1", "h1").await;
        sqlx::query("UPDATE users SET is_active = 0 WHERE id = 'u1'").execute(&state.db).await.unwrap();
        assert_eq!(redeem(&state, &t).await.0, StatusCode::UNAUTHORIZED);
        sqlx::query("UPDATE users SET is_active = 1 WHERE id = 'u1'").execute(&state.db).await.unwrap();
        // Sessions ended (new password, two-factor off) after the ticket was made.
        let t = ticket_for(&state, "u1", "h1").await;
        crate::auth::revoke_sessions(&state, "u1").await.unwrap();
        assert_eq!(redeem(&state, &t).await.0, StatusCode::UNAUTHORIZED);
        // Deleted user: the ticket goes with it.
        let t = ticket_for(&state, "u1", "h1").await;
        sqlx::query("DELETE FROM users WHERE id = 'u1'").execute(&state.db).await.unwrap();
        assert_eq!(redeem(&state, &t).await.0, StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn issuing_and_guessing_are_limited() {
        let state = state().await;
        for _ in 0..MAX_LIVE_TICKETS_PER_USER {
            ticket_for(&state, "u1", "h1").await;
        }
        assert_eq!(ask(&state, "u1", "h1").await.0, StatusCode::TOO_MANY_REQUESTS);
        // Others are not affected.
        ticket_for(&state, "boss", "h2").await;

        let good = ticket_for(&state, "boss", "h1").await;
        for _ in 0..10 {
            assert_eq!(redeem_from(&state, "192.0.2.9", "lct_nope").await.0, StatusCode::UNAUTHORIZED);
        }
        assert_eq!(redeem_from(&state, "192.0.2.9", &good).await.0, StatusCode::TOO_MANY_REQUESTS);
        assert_eq!(redeem_from(&state, "192.0.2.10", &"x".repeat(5000)).await.0, StatusCode::UNAUTHORIZED);
        assert_eq!(redeem_from(&state, "192.0.2.10", &good).await.0, StatusCode::OK);
    }

    #[tokio::test]
    async fn a_host_token_opens_no_http_api() {
        let state = state().await;
        let t = ticket_for(&state, "boss", "h1").await;
        let token = redeem(&state, &t).await.1["token"].as_str().unwrap().to_string();
        let mut parts = axum::http::Request::builder()
            .header("Authorization", format!("Bearer {token}"))
            .body(())
            .unwrap()
            .into_parts()
            .0;
        let refused = AuthenticatedUser::from_request_parts(&mut parts, &state).await.err().unwrap();
        assert_eq!(refused.0, StatusCode::FORBIDDEN);
        assert!(crate::auth::AdminUser::from_request_parts(&mut parts, &state).await.is_err());
        // And it can't make more tickets (that takes an AuthenticatedUser).
    }
}
