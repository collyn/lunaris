//! Two-factor sign-in with an authenticator app (Google Authenticator,
//! Microsoft Authenticator, Aegis, 1Password…): RFC 6238 time-based codes,
//! SHA-1, 6 digits, 30 seconds, one step of clock difference either way. A
//! code is accepted once: the last accepted time step is kept per user.
//!
//! The 160-bit secret is kept encrypted (ChaCha20-Poly1305, bound to the
//! user id) with a key in `mfa_key.txt` beside the database, and is only
//! shown while it is being set up. Recovery codes are kept as HMAC-SHA-256
//! with the same key.
//!
//! Sign-in with two-factor turned on: the password answers with a
//! short-lived, single-use `mfa_token` (random, kept in memory here; not a
//! JWT, so it can never be used as a session), and `/api/auth/login/mfa`
//! exchanges it and a code for the usual sign-in answer.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::{
    extract::{ConnectInfo, Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use once_cell::sync::Lazy;
use ring::{aead, hmac};
use serde::Deserialize;
use sqlx::SqlitePool;
use tracing::{error, info, warn};

use crate::{
    auth::{AdminUser, AuthenticatedUser},
    security::{self, FailureLimiter},
    signaling::{ConnAddrs, SignalingState},
};

pub const ISSUER: &str = "Lunaris";
/// Recovery codes given when two-factor sign-in is turned on.
pub const RECOVERY_CODE_COUNT: usize = 10;
/// How long the step between password and code may take.
pub const PENDING_TTL: Duration = Duration::from_secs(300);
/// Wrong codes one pending sign-in may take before it has to start over.
const PENDING_MAX_FAILURES: u32 = 5;
const PENDING_MAX_ENTRIES: usize = 10_000;
/// Sign-ins one user may have waiting for their code at once: a new one
/// past this replaces that user's oldest (each would allow
/// `PENDING_MAX_FAILURES` more guesses otherwise).
const PENDING_MAX_PER_USER: usize = 3;
/// Crockford's base 32 (no I, L, O, U): 256 is a multiple of 32, so taking
/// a byte modulo 32 is unbiased.
const RECOVERY_ALPHABET: &[u8; 32] = b"0123456789abcdefghjkmnpqrstvwxyz";

static MFA_KEY: Lazy<[u8; 32]> = Lazy::new(|| {
    #[cfg(test)]
    {
        random_key()
    }
    #[cfg(not(test))]
    {
        load_key(&crate::paths::data_file("mfa_key.txt"))
    }
});

fn random_key() -> [u8; 32] {
    let mut key = [0u8; 32];
    getrandom::fill(&mut key).expect("OS random source unavailable");
    key
}

/// The key in `file`, or a new one saved there. A file that is not a key is
/// moved aside (`.invalid`), never overwritten.
#[cfg_attr(test, allow(dead_code))]
fn load_key(file: &std::path::Path) -> [u8; 32] {
    if let Ok(text) = std::fs::read_to_string(file) {
        crate::paths::tighten_private(file);
        if let Ok(bytes) = hex::decode(text.trim()) {
            if let Ok(key) = <[u8; 32]>::try_from(bytes.as_slice()) {
                return key;
            }
        }
        let aside = file.with_extension("txt.invalid");
        error!("{:?} is not a two-factor key; moved to {:?}. Two-factor sign-in has to be set up again.", file, aside);
        let _ = std::fs::rename(file, &aside);
    }
    let key = random_key();
    if let Err(e) = crate::paths::write_private(file, &hex::encode(key)) {
        error!("Cannot save the two-factor key to {:?} (two-factor sign-in stops working at restart): {}", file, e);
    }
    key
}

// ---- Secrets at rest ----

fn aead_key(key: &[u8; 32]) -> aead::LessSafeKey {
    aead::LessSafeKey::new(aead::UnboundKey::new(&aead::CHACHA20_POLY1305, key).expect("32-byte key"))
}

/// "v1:" + hex(nonce ‖ ciphertext ‖ tag), bound to `user_id`.
fn seal(key: &[u8; 32], user_id: &str, plain: &[u8]) -> String {
    let mut nonce = [0u8; aead::NONCE_LEN];
    getrandom::fill(&mut nonce).expect("OS random source unavailable");
    let mut buf = plain.to_vec();
    aead_key(key)
        .seal_in_place_append_tag(
            aead::Nonce::assume_unique_for_key(nonce),
            aead::Aad::from(user_id.as_bytes()),
            &mut buf,
        )
        .expect("seal");
    format!("v1:{}{}", hex::encode(nonce), hex::encode(buf))
}

fn open(key: &[u8; 32], user_id: &str, stored: &str) -> Option<Vec<u8>> {
    let raw = hex::decode(stored.strip_prefix("v1:")?).ok()?;
    if raw.len() < aead::NONCE_LEN + aead::CHACHA20_POLY1305.tag_len() {
        return None;
    }
    let (nonce, rest) = raw.split_at(aead::NONCE_LEN);
    let nonce = aead::Nonce::try_assume_unique_for_key(nonce).ok()?;
    let mut buf = rest.to_vec();
    let plain = aead_key(key)
        .open_in_place(nonce, aead::Aad::from(user_id.as_bytes()), &mut buf)
        .ok()?;
    Some(plain.to_vec())
}

// ---- Codes ----

fn totp(secret: &[u8], account: &str) -> Option<totp_rs::Totp> {
    // The label may not hold a colon (issuer:account).
    let account = account.replace(':', "_");
    let account = if account.trim().is_empty() { "user".to_string() } else { account };
    totp_rs::Builder::new()
        .with_algorithm(totp_rs::Algorithm::SHA1)
        .with_digits(6)
        .with_skew(1)
        .with_step_duration(30)
        .with_secret(secret.to_vec())
        .with_issuer(Some(ISSUER))
        .with_account_name(account)
        .build()
        .ok()
}

/// The time step `code` belongs to at `now` (Unix seconds), within one step
/// either way, and only when it is later than `last_step` (a code is used
/// once).
pub fn check_code(secret: &[u8], code: &str, now: u64, last_step: Option<i64>) -> Option<u64> {
    let code: String = code.chars().filter(|c| !c.is_whitespace()).collect();
    if code.len() != 6 || !code.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let step = totp(secret, "user")?.check(&code, now)?;
    match last_step {
        Some(last) if (step as i64) <= last => None,
        _ => Some(step),
    }
}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// A recovery code as typed: case, dashes and spaces don't matter, and the
/// letters people confuse with digits are read as those digits.
pub fn normalize_recovery_code(code: &str) -> String {
    code.chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .map(|c| match c.to_ascii_lowercase() {
            'o' => '0',
            'i' | 'l' => '1',
            c => c,
        })
        .collect()
}

fn new_recovery_code() -> String {
    let mut bytes = [0u8; 12];
    getrandom::fill(&mut bytes).expect("OS random source unavailable");
    let chars: Vec<char> = bytes.iter().map(|b| RECOVERY_ALPHABET[(*b % 32) as usize] as char).collect();
    chars.chunks(4).map(|c| c.iter().collect::<String>()).collect::<Vec<_>>().join("-")
}

fn recovery_hash(key: &[u8; 32], user_id: &str, code: &str) -> String {
    let k = hmac::Key::new(hmac::HMAC_SHA256, key);
    let msg = format!("{}\n{}", user_id, normalize_recovery_code(code));
    hex::encode(hmac::sign(&k, msg.as_bytes()).as_ref())
}

/// An SVG picture of `text` as a QR code, as a data: URL (black on white
/// with the quiet zone, which every scanner reads, in dark mode too).
pub fn qr_data_url(text: &str) -> Option<String> {
    use common::base64::Engine;
    use std::fmt::Write;
    let qr = qrcodegen::QrCode::encode_text(text, qrcodegen::QrCodeEcc::Medium).ok()?;
    let border = 4;
    let dim = qr.size() + 2 * border;
    let mut path = String::new();
    for y in 0..qr.size() {
        for x in 0..qr.size() {
            if qr.get_module(x, y) {
                let _ = write!(path, "M{},{}h1v1h-1z", x + border, y + border);
            }
        }
    }
    let svg = format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 {dim} {dim}\" shape-rendering=\"crispEdges\">\
         <rect width=\"{dim}\" height=\"{dim}\" fill=\"#ffffff\"/><path d=\"{path}\" fill=\"#000000\"/></svg>"
    );
    Some(format!(
        "data:image/svg+xml;base64,{}",
        common::base64::engine::general_purpose::STANDARD.encode(svg)
    ))
}

// ---- Database ----

/// Whether `user_id` has two-factor sign-in turned on.
pub async fn is_enabled(db: &SqlitePool, user_id: &str) -> Result<bool, sqlx::Error> {
    let n: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM user_mfa WHERE user_id = ? AND enabled = 1")
        .bind(user_id)
        .fetch_one(db)
        .await?;
    Ok(n > 0)
}

async fn secret_row(db: &SqlitePool, user_id: &str, enabled: bool) -> Result<Option<(String, Option<i64>)>, sqlx::Error> {
    sqlx::query_as("SELECT secret, last_step FROM user_mfa WHERE user_id = ? AND enabled = ?")
        .bind(user_id)
        .bind(enabled as i64)
        .fetch_optional(db)
        .await
}

async fn recovery_codes_left(db: &SqlitePool, user_id: &str) -> Result<i64, sqlx::Error> {
    sqlx::query_scalar("SELECT COUNT(*) FROM user_recovery_codes WHERE user_id = ? AND used_at IS NULL")
        .bind(user_id)
        .fetch_one(db)
        .await
}

/// New recovery codes for `user_id` (the old ones stop working).
async fn replace_recovery_codes(db: &SqlitePool, user_id: &str) -> Result<Vec<String>, sqlx::Error> {
    let codes: Vec<String> = (0..RECOVERY_CODE_COUNT).map(|_| new_recovery_code()).collect();
    let mut tx = db.begin().await?;
    sqlx::query("DELETE FROM user_recovery_codes WHERE user_id = ?").bind(user_id).execute(&mut *tx).await?;
    for code in &codes {
        sqlx::query("INSERT INTO user_recovery_codes (user_id, code_hash, created_at) VALUES (?, ?, datetime('now'))")
            .bind(user_id)
            .bind(recovery_hash(&MFA_KEY, user_id, code))
            .execute(&mut *tx)
            .await?;
    }
    tx.commit().await?;
    Ok(codes)
}

async fn remove(db: &SqlitePool, user_id: &str) -> Result<bool, sqlx::Error> {
    let mut tx = db.begin().await?;
    sqlx::query("DELETE FROM user_recovery_codes WHERE user_id = ?").bind(user_id).execute(&mut *tx).await?;
    let done = sqlx::query("DELETE FROM user_mfa WHERE user_id = ? AND enabled = 1").bind(user_id).execute(&mut *tx).await?;
    sqlx::query("DELETE FROM user_mfa WHERE user_id = ?").bind(user_id).execute(&mut *tx).await?;
    tx.commit().await?;
    Ok(done.rows_affected() > 0)
}

/// Checks a code from the app (and uses it up) for a user with two-factor
/// sign-in on.
async fn use_code(db: &SqlitePool, user_id: &str, code: &str) -> Result<bool, sqlx::Error> {
    let Some((stored, last_step)) = secret_row(db, user_id, true).await? else {
        return Ok(false);
    };
    let Some(secret) = open(&MFA_KEY, user_id, &stored) else {
        error!("Cannot read the two-factor secret of user {} (mfa_key.txt changed?); an admin can reset it", user_id);
        return Ok(false);
    };
    let Some(step) = check_code(&secret, code, now_secs(), last_step) else {
        return Ok(false);
    };
    // Only one request can move the step forward: the same code twice at
    // once is still accepted once.
    let done = sqlx::query(
        "UPDATE user_mfa SET last_step = ? WHERE user_id = ? AND enabled = 1 AND (last_step IS NULL OR last_step < ?)",
    )
    .bind(step as i64)
    .bind(user_id)
    .bind(step as i64)
    .execute(db)
    .await?;
    Ok(done.rows_affected() == 1)
}

async fn use_recovery_code(db: &SqlitePool, user_id: &str, code: &str) -> Result<bool, sqlx::Error> {
    if normalize_recovery_code(code).is_empty() || !is_enabled(db, user_id).await? {
        return Ok(false);
    }
    let done = sqlx::query(
        "UPDATE user_recovery_codes SET used_at = datetime('now') WHERE user_id = ? AND code_hash = ? AND used_at IS NULL",
    )
    .bind(user_id)
    .bind(recovery_hash(&MFA_KEY, user_id, code))
    .execute(db)
    .await?;
    Ok(done.rows_affected() == 1)
}

/// The second factor in a request: a code from the app, or (where allowed)
/// a recovery code.
async fn use_factor(
    db: &SqlitePool,
    user_id: &str,
    code: Option<&str>,
    recovery_code: Option<&str>,
) -> Result<bool, sqlx::Error> {
    match (code.filter(|c| !c.trim().is_empty()), recovery_code.filter(|c| !c.trim().is_empty())) {
        (Some(code), _) => use_code(db, user_id, code).await,
        (None, Some(recovery)) => use_recovery_code(db, user_id, recovery).await,
        (None, None) => Ok(false),
    }
}

// ---- Sign-ins waiting for their code ----

struct Pending {
    user_id: String,
    expires: Instant,
    failures: u32,
}

/// Sign-ins between password and code, and the limits on wrong codes.
pub struct MfaState {
    /// SHA-256 of the mfa_token → the sign-in it continues.
    pending: Mutex<HashMap<String, Pending>>,
    /// Wrong codes per client address, and per user.
    pub ip_limit: FailureLimiter,
    pub user_limit: FailureLimiter,
}

impl Default for MfaState {
    fn default() -> Self {
        Self {
            pending: Mutex::new(HashMap::new()),
            ip_limit: FailureLimiter::standard(),
            user_limit: FailureLimiter::standard(),
        }
    }
}

impl MfaState {
    /// A new mfa_token for `user_id`, valid for `PENDING_TTL`.
    pub fn issue(&self, user_id: &str, now: Instant) -> String {
        let token = security::random_hex(32);
        let mut pending = self.pending.lock().unwrap();
        pending.retain(|_, p| p.expires > now);
        loop {
            let mine: Vec<(&String, &Pending)> = pending.iter().filter(|(_, p)| p.user_id == user_id).collect();
            if mine.len() < PENDING_MAX_PER_USER {
                break;
            }
            let oldest = mine.iter().min_by_key(|(_, p)| p.expires).map(|(k, _)| (*k).clone());
            match oldest {
                Some(k) => {
                    pending.remove(&k);
                }
                None => break,
            }
        }
        if pending.len() >= PENDING_MAX_ENTRIES {
            if let Some(oldest) = pending.iter().min_by_key(|(_, p)| p.expires).map(|(k, _)| k.clone()) {
                pending.remove(&oldest);
            }
        }
        pending.insert(
            security::sha256_hex(&token),
            Pending { user_id: user_id.to_string(), expires: now + PENDING_TTL, failures: 0 },
        );
        token
    }

    /// The user a token continues the sign-in of, while it is valid.
    fn peek(&self, token: &str, now: Instant) -> Option<String> {
        let key = security::sha256_hex(token);
        let mut pending = self.pending.lock().unwrap();
        match pending.get(&key) {
            Some(p) if p.expires > now => Some(p.user_id.clone()),
            Some(_) => {
                pending.remove(&key);
                None
            }
            None => None,
        }
    }

    /// A wrong code: after a few the token is gone.
    fn fail(&self, token: &str) {
        let key = security::sha256_hex(token);
        let mut pending = self.pending.lock().unwrap();
        if let Some(p) = pending.get_mut(&key) {
            p.failures += 1;
            if p.failures >= PENDING_MAX_FAILURES {
                pending.remove(&key);
            }
        }
    }

    /// Uses the token up; false when it was already used (or expired).
    fn take(&self, token: &str, now: Instant) -> bool {
        let key = security::sha256_hex(token);
        matches!(self.pending.lock().unwrap().remove(&key), Some(p) if p.expires > now)
    }
}

/// The answer to a right password when the account has two-factor sign-in on.
pub fn challenge(state: &SignalingState, user_id: &str, username: &str) -> Response {
    let token = state.mfa.issue(user_id, Instant::now());
    (
        StatusCode::OK,
        Json(serde_json::json!({
            "mfa_required": true,
            "mfa_token": token,
            "username": username,
            "expires_in": PENDING_TTL.as_secs(),
        })),
    )
        .into_response()
}

// ---- HTTP ----

type ApiError = (StatusCode, Json<serde_json::Value>);

fn err(status: StatusCode, msg: &str) -> ApiError {
    (status, Json(serde_json::json!({ "error": msg })))
}

fn db_error(e: sqlx::Error) -> ApiError {
    error!("Database error: {}", e);
    err(StatusCode::INTERNAL_SERVER_ERROR, "Internal Database Error")
}

fn too_many() -> ApiError {
    err(StatusCode::TOO_MANY_REQUESTS, "Too many wrong codes. Try again in a few minutes.")
}

fn user_key(user_id: &str) -> String {
    format!("mfa\n{user_id}")
}

#[derive(Deserialize)]
pub struct MfaLoginRequest {
    pub mfa_token: String,
    #[serde(default)]
    pub code: Option<String>,
    #[serde(default)]
    pub recovery_code: Option<String>,
}

#[derive(Deserialize)]
pub struct CodeRequest {
    #[serde(default)]
    pub code: Option<String>,
}

#[derive(Deserialize)]
pub struct DisableRequest {
    #[serde(default)]
    pub password: String,
    #[serde(default)]
    pub code: Option<String>,
    #[serde(default)]
    pub recovery_code: Option<String>,
}

// POST /api/auth/login/mfa
/// The second step of signing in: the mfa_token from the password step and
/// a code from the app (or a recovery code) → the usual sign-in answer.
pub async fn login_mfa_handler(
    State(state): State<Arc<SignalingState>>,
    ConnectInfo(addrs): ConnectInfo<ConnAddrs>,
    headers: axum::http::HeaderMap,
    Json(req): Json<MfaLoginRequest>,
) -> Result<Response, ApiError> {
    let now = Instant::now();
    let ip_key = security::client_limit_key(addrs.remote.ip(), &headers);
    // Taken before anything is awaited: requests sent at once count too.
    let Some(ip_attempt) = state.mfa.ip_limit.try_begin(&ip_key, now) else {
        return Err(too_many());
    };
    let Some(user_id) = state.mfa.peek(&req.mfa_token, now) else {
        ip_attempt.fail();
        return Err(err(StatusCode::UNAUTHORIZED, "This sign-in has expired. Sign in again."));
    };
    // Per user, over all of that user's pending sign-ins.
    let Some(user_attempt) = state.mfa.user_limit.try_begin(&user_key(&user_id), now) else {
        return Err(too_many());
    };
    let ok = use_factor(&state.db, &user_id, req.code.as_deref(), req.recovery_code.as_deref())
        .await
        .map_err(db_error)?;
    if !ok {
        state.mfa.fail(&req.mfa_token);
        ip_attempt.fail();
        user_attempt.fail();
        let msg = if req.code.as_deref().is_some_and(|c| !c.trim().is_empty()) { "Wrong code" } else { "Wrong recovery code" };
        return Err(err(StatusCode::UNAUTHORIZED, msg));
    }
    ip_attempt.succeed();
    user_attempt.succeed();
    if !state.mfa.take(&req.mfa_token, now) {
        return Err(err(StatusCode::UNAUTHORIZED, "This sign-in has expired. Sign in again."));
    }
    state.mfa.user_limit.reset(&user_key(&user_id));
    if req.code.as_deref().is_none_or(|c| c.trim().is_empty()) {
        info!("User {} signed in with a recovery code", user_id);
    }

    // The account as it is now (it may have been disabled in between).
    let row: Option<(String, String, i64, i64)> = sqlx::query_as(
        "SELECT username, COALESCE(role, 'user'), is_active, token_version FROM users WHERE id = ?",
    )
    .bind(&user_id)
    .fetch_optional(&state.db)
    .await
    .map_err(db_error)?;
    let Some((username, role, _, version)) = row.filter(|(_, _, active, _)| *active != 0) else {
        return Err(err(StatusCode::UNAUTHORIZED, "Invalid username or password"));
    };
    let token = crate::auth::create_jwt(&user_id, &username, &role, version).map_err(|e| {
        error!("Token error: {}", e);
        err(StatusCode::INTERNAL_SERVER_ERROR, "Failed to generate token")
    })?;
    Ok((StatusCode::OK, Json(common::AuthResponse { token, username, role })).into_response())
}

// GET /api/auth/mfa
pub async fn status(user: AuthenticatedUser, State(state): State<Arc<SignalingState>>) -> Result<Response, ApiError> {
    let enabled = is_enabled(&state.db, &user.user_id).await.map_err(db_error)?;
    let left = if enabled { recovery_codes_left(&state.db, &user.user_id).await.map_err(db_error)? } else { 0 };
    Ok(Json(serde_json::json!({ "enabled": enabled, "recovery_codes_left": left })).into_response())
}

#[derive(Deserialize)]
pub struct SetupRequest {
    /// The account's password, entered again: a session left open
    /// somewhere can't bind someone else's authenticator to the account.
    #[serde(default)]
    pub password: String,
}

/// The wrong account password where it is asked again: 403 with
/// `code: "wrong_password"` (not 401, which the web client reads as a
/// session that ended).
fn wrong_password() -> ApiError {
    (
        StatusCode::FORBIDDEN,
        Json(serde_json::json!({ "error": "Wrong password", "code": "wrong_password" })),
    )
}

/// Checks `password` against the user's own, counted in `mfa.user_limit`.
async fn check_own_password(state: &SignalingState, user_id: &str, password: &str) -> Result<(), ApiError> {
    let Some(attempt) = state.mfa.user_limit.try_begin(&user_key(user_id), Instant::now()) else {
        return Err(too_many());
    };
    let hash: Option<String> = sqlx::query_scalar("SELECT password_hash FROM users WHERE id = ?")
        .bind(user_id)
        .fetch_optional(&state.db)
        .await
        .map_err(db_error)?;
    let ok = !password.is_empty()
        && crate::auth::verify_password_blocking(password.to_string(), hash.unwrap_or_else(|| crate::auth::DUMMY_HASH.to_string())).await;
    if ok {
        attempt.succeed();
        Ok(())
    } else {
        attempt.fail();
        Err(wrong_password())
    }
}

// POST /api/auth/mfa/setup {password}
/// A new secret to add to the app; two-factor sign-in is on only once a
/// code from it is confirmed. Starting again replaces an unconfirmed secret.
/// The account password is asked again (`wrong_password` when wrong).
pub async fn setup(
    user: AuthenticatedUser,
    State(state): State<Arc<SignalingState>>,
    Json(req): Json<SetupRequest>,
) -> Result<Response, ApiError> {
    if is_enabled(&state.db, &user.user_id).await.map_err(db_error)? {
        return Err(err(StatusCode::CONFLICT, "Two-factor sign-in is already on"));
    }
    check_own_password(&state, &user.user_id, &req.password).await?;
    let mut secret = [0u8; 20];
    getrandom::fill(&mut secret).expect("OS random source unavailable");
    let username: Option<String> = sqlx::query_scalar("SELECT username FROM users WHERE id = ?")
        .bind(&user.user_id)
        .fetch_optional(&state.db)
        .await
        .map_err(db_error)?;
    let username = username.unwrap_or(user.username.clone());
    let Some(totp) = totp(&secret, &username) else {
        return Err(err(StatusCode::INTERNAL_SERVER_ERROR, "Cannot make a secret"));
    };
    let url = totp.get_url_or_err()?;
    sqlx::query(
        "INSERT INTO user_mfa (user_id, secret, enabled, last_step, created_at) VALUES (?, ?, 0, NULL, datetime('now'))
         ON CONFLICT(user_id) DO UPDATE SET secret = excluded.secret, last_step = NULL, created_at = excluded.created_at
         WHERE user_mfa.enabled = 0",
    )
    .bind(&user.user_id)
    .bind(seal(&MFA_KEY, &user.user_id, &secret))
    .execute(&state.db)
    .await
    .map_err(db_error)?;
    Ok(Json(serde_json::json!({
        "secret": totp.secret().to_base32(),
        "otpauth_url": url,
        "qr": qr_data_url(&url),
        "issuer": ISSUER,
        "account": username,
    }))
    .into_response())
}

trait UrlOrErr {
    fn get_url_or_err(&self) -> Result<String, ApiError>;
}

impl UrlOrErr for totp_rs::Totp {
    fn get_url_or_err(&self) -> Result<String, ApiError> {
        self.to_url().map_err(|e| {
            error!("otpauth link: {}", e);
            err(StatusCode::INTERNAL_SERVER_ERROR, "Cannot make a secret")
        })
    }
}

// POST /api/auth/mfa/enable {code}
/// Confirms the secret from `setup` with a code from the app: two-factor
/// sign-in is on, and the recovery codes are shown (only this once).
pub async fn enable(
    user: AuthenticatedUser,
    State(state): State<Arc<SignalingState>>,
    Json(req): Json<CodeRequest>,
) -> Result<Response, ApiError> {
    let now = Instant::now();
    let key = user_key(&user.user_id);
    let Some(attempt) = state.mfa.user_limit.try_begin(&key, now) else {
        return Err(too_many());
    };
    let Some((stored, _)) = secret_row(&state.db, &user.user_id, false).await.map_err(db_error)? else {
        if is_enabled(&state.db, &user.user_id).await.map_err(db_error)? {
            return Err(err(StatusCode::CONFLICT, "Two-factor sign-in is already on"));
        }
        return Err(err(StatusCode::BAD_REQUEST, "Start setting up two-factor sign-in first"));
    };
    let step = open(&MFA_KEY, &user.user_id, &stored)
        .and_then(|secret| check_code(&secret, req.code.as_deref().unwrap_or(""), now_secs(), None));
    let Some(step) = step else {
        attempt.fail();
        return Err(err(StatusCode::BAD_REQUEST, "Wrong code"));
    };
    attempt.succeed();
    let done = sqlx::query(
        "UPDATE user_mfa SET enabled = 1, last_step = ?, enabled_at = datetime('now') WHERE user_id = ? AND enabled = 0 AND secret = ?",
    )
    .bind(step as i64)
    .bind(&user.user_id)
    .bind(&stored)
    .execute(&state.db)
    .await
    .map_err(db_error)?;
    if done.rows_affected() != 1 {
        return Err(err(StatusCode::CONFLICT, "Start setting up two-factor sign-in first"));
    }
    let codes = replace_recovery_codes(&state.db, &user.user_id).await.map_err(db_error)?;
    state.mfa.user_limit.reset(&key);
    info!("User {} turned on two-factor sign-in", user.username);
    Ok(Json(serde_json::json!({ "enabled": true, "recovery_codes": codes })).into_response())
}

// POST /api/auth/mfa/disable {password, code | recovery_code}
pub async fn disable(
    user: AuthenticatedUser,
    State(state): State<Arc<SignalingState>>,
    Json(req): Json<DisableRequest>,
) -> Result<Response, ApiError> {
    let now = Instant::now();
    let key = user_key(&user.user_id);
    let Some(attempt) = state.mfa.user_limit.try_begin(&key, now) else {
        return Err(too_many());
    };
    if !is_enabled(&state.db, &user.user_id).await.map_err(db_error)? {
        return Err(err(StatusCode::BAD_REQUEST, "Two-factor sign-in is not on"));
    }
    let hash: Option<String> = sqlx::query_scalar("SELECT password_hash FROM users WHERE id = ?")
        .bind(&user.user_id)
        .fetch_optional(&state.db)
        .await
        .map_err(db_error)?;
    let password_ok = crate::auth::verify_password_blocking(
        req.password.clone(),
        hash.unwrap_or_else(|| crate::auth::DUMMY_HASH.to_string()),
    )
    .await;
    // The factor is only spent once the password is right.
    let factor_ok = password_ok
        && use_factor(&state.db, &user.user_id, req.code.as_deref(), req.recovery_code.as_deref())
            .await
            .map_err(db_error)?;
    if !factor_ok {
        attempt.fail();
        return Err(err(StatusCode::BAD_REQUEST, "Wrong password or code"));
    }
    attempt.succeed();
    remove(&state.db, &user.user_id).await.map_err(db_error)?;
    state.mfa.user_limit.reset(&key);
    // Every session ends (one may have been signed in by whoever had the
    // second factor); this one goes on with the new token in the answer.
    let version = crate::auth::revoke_sessions(&state, &user.user_id).await.map_err(db_error)?;
    let token = crate::auth::create_jwt(&user.user_id, &user.username, &user.role, version).map_err(|e| {
        error!("Token error: {}", e);
        err(StatusCode::INTERNAL_SERVER_ERROR, "Failed to generate token")
    })?;
    info!("User {} turned off two-factor sign-in; their other sessions were ended", user.username);
    Ok(Json(serde_json::json!({ "enabled": false, "token": token })).into_response())
}

// POST /api/auth/mfa/recovery-codes {code}
/// New recovery codes (the old ones stop working), with a code from the app.
pub async fn regenerate_recovery_codes(
    user: AuthenticatedUser,
    State(state): State<Arc<SignalingState>>,
    Json(req): Json<CodeRequest>,
) -> Result<Response, ApiError> {
    let now = Instant::now();
    let key = user_key(&user.user_id);
    let Some(attempt) = state.mfa.user_limit.try_begin(&key, now) else {
        return Err(too_many());
    };
    if !is_enabled(&state.db, &user.user_id).await.map_err(db_error)? {
        return Err(err(StatusCode::BAD_REQUEST, "Two-factor sign-in is not on"));
    }
    if !use_code(&state.db, &user.user_id, req.code.as_deref().unwrap_or("")).await.map_err(db_error)? {
        attempt.fail();
        return Err(err(StatusCode::BAD_REQUEST, "Wrong code"));
    }
    attempt.succeed();
    let codes = replace_recovery_codes(&state.db, &user.user_id).await.map_err(db_error)?;
    state.mfa.user_limit.reset(&key);
    info!("User {} made new recovery codes", user.username);
    Ok(Json(serde_json::json!({ "recovery_codes": codes })).into_response())
}

// DELETE /api/admin/users/{id}/mfa
/// An admin turns off another user's two-factor sign-in (a lost phone).
pub async fn admin_reset(
    admin: AdminUser,
    State(state): State<Arc<SignalingState>>,
    Path(user_id): Path<String>,
) -> Result<Response, ApiError> {
    let username: Option<String> = sqlx::query_scalar("SELECT username FROM users WHERE id = ?")
        .bind(&user_id)
        .fetch_optional(&state.db)
        .await
        .map_err(db_error)?;
    let Some(username) = username else {
        return Err(err(StatusCode::NOT_FOUND, "User not found"));
    };
    if !remove(&state.db, &user_id).await.map_err(db_error)? {
        return Err(err(StatusCode::NOT_FOUND, "Two-factor sign-in is not on for this user"));
    }
    state.mfa.user_limit.reset(&user_key(&user_id));
    crate::auth::revoke_sessions(&state, &user_id).await.map_err(db_error)?;
    warn!(
        "AUDIT: admin {} ({}) reset two-factor sign-in of user {} ({}); their sessions were ended",
        admin.username, admin.user_id, username, user_id
    );
    Ok(Json(serde_json::json!({ "enabled": false })).into_response())
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::to_bytes;

    // RFC 6238 appendix B, SHA-1 secret; the 8-digit 94287082 at T=59 is
    // 287082 with 6 digits.
    const RFC_SECRET: &[u8] = b"12345678901234567890";

    fn code_at(secret: &[u8], t: u64) -> String {
        totp(secret, "x").unwrap().generate(t).to_string()
    }

    #[test]
    fn rfc6238_vector_and_window() {
        assert_eq!(code_at(RFC_SECRET, 59), "287082");
        assert_eq!(check_code(RFC_SECRET, "287082", 59, None), Some(1));
        assert_eq!(check_code(RFC_SECRET, "287 082", 59, None), Some(1));
        let now = 1_700_000_000u64;
        let step = now / 30;
        // One step either way is accepted, two are not.
        assert_eq!(check_code(RFC_SECRET, &code_at(RFC_SECRET, now - 30), now, None), Some(step - 1));
        assert_eq!(check_code(RFC_SECRET, &code_at(RFC_SECRET, now + 30), now, None), Some(step + 1));
        assert_eq!(check_code(RFC_SECRET, &code_at(RFC_SECRET, now - 60), now, None), None);
        assert_eq!(check_code(RFC_SECRET, &code_at(RFC_SECRET, now + 60), now, None), None);
        // Not six digits.
        assert_eq!(check_code(RFC_SECRET, "12345", now, None), None);
        assert_eq!(check_code(RFC_SECRET, "abcdef", now, None), None);
        assert_eq!(check_code(RFC_SECRET, "", now, None), None);
    }

    #[test]
    fn a_code_is_used_once() {
        let now = 1_700_000_000u64;
        let step = (now / 30) as i64;
        let code = code_at(RFC_SECRET, now);
        assert_eq!(check_code(RFC_SECRET, &code, now, Some(step - 1)), Some(step as u64));
        assert_eq!(check_code(RFC_SECRET, &code, now, Some(step)), None);
        // An older code after a newer one was used: refused too.
        let older = code_at(RFC_SECRET, now - 30);
        assert_eq!(check_code(RFC_SECRET, &older, now, Some(step)), None);
    }

    #[test]
    fn secret_sealed_per_user() {
        let key = random_key();
        let sealed = seal(&key, "u1", b"secret-bytes-20-long");
        assert!(sealed.starts_with("v1:"));
        assert!(!sealed.contains(&hex::encode(b"secret-bytes-20-long")));
        assert_eq!(open(&key, "u1", &sealed).unwrap(), b"secret-bytes-20-long");
        // Another user's row, another key, or a changed byte: nothing.
        assert!(open(&key, "u2", &sealed).is_none());
        assert!(open(&random_key(), "u1", &sealed).is_none());
        let mut bad = sealed.clone();
        let last = bad.pop().unwrap();
        bad.push(if last == '0' { '1' } else { '0' });
        assert!(open(&key, "u1", &bad).is_none());
        assert!(open(&key, "u1", "garbage").is_none());
    }

    #[test]
    fn recovery_codes_format_and_typing() {
        let code = new_recovery_code();
        assert_eq!(code.len(), 14);
        assert_eq!(code.matches('-').count(), 2);
        assert_ne!(code, new_recovery_code());
        let key = random_key();
        let typed = format!(" {} ", code.to_uppercase().replace('-', " "));
        assert_eq!(recovery_hash(&key, "u1", &code), recovery_hash(&key, "u1", &typed));
        assert_ne!(recovery_hash(&key, "u1", &code), recovery_hash(&key, "u2", &code));
        assert_eq!(normalize_recovery_code("O1-Il"), "0111");
    }

    #[test]
    fn otpauth_link_and_qr() {
        let t = totp(&[7u8; 20], "ali:ce").unwrap();
        let url = t.to_url().unwrap();
        assert!(url.starts_with("otpauth://totp/Lunaris:ali_ce?secret="), "{url}");
        assert!(url.contains("issuer=Lunaris"));
        assert_eq!(t.secret().to_base32().len(), 32);
        let qr = qr_data_url(&url).unwrap();
        assert!(qr.starts_with("data:image/svg+xml;base64,"));
    }

    #[test]
    fn pending_tokens_expire_and_are_single_use() {
        let state = MfaState::default();
        let t0 = Instant::now();
        let token = state.issue("u1", t0);
        assert_eq!(state.peek(&token, t0).as_deref(), Some("u1"));
        assert!(state.peek("other", t0).is_none());
        assert!(state.take(&token, t0));
        assert!(!state.take(&token, t0));
        assert!(state.peek(&token, t0).is_none());
        // Expired.
        let token = state.issue("u1", t0);
        assert!(state.peek(&token, t0 + PENDING_TTL).is_none());
        // Too many wrong codes.
        let token = state.issue("u1", t0);
        for _ in 0..PENDING_MAX_FAILURES {
            assert!(state.peek(&token, t0).is_some());
            state.fail(&token);
        }
        assert!(state.peek(&token, t0).is_none());
        // Not a session token.
        assert!(crate::auth::verify_jwt(&state.issue("u1", t0)).is_err());
    }

    // ---- Through the handlers ----

    async fn state_with_alice() -> Arc<SignalingState> {
        let db = crate::security::tests::test_db().await;
        let hash = bcrypt::hash("right-pw", 4).unwrap();
        for (id, name, role) in [("u1", "alice", "user"), ("a1", "boss", "admin")] {
            sqlx::query("INSERT INTO users (id, username, password_hash, role, is_active) VALUES (?, ?, ?, ?, 1)")
                .bind(id)
                .bind(name)
                .bind(&hash)
                .bind(role)
                .execute(&db)
                .await
                .unwrap();
        }
        Arc::new(SignalingState::new(db, "t".into(), None))
    }

    fn alice() -> AuthenticatedUser {
        AuthenticatedUser { user_id: "u1".into(), username: "alice".into(), role: "user".into() }
    }

    fn addrs(ip: &str) -> ConnAddrs {
        ConnAddrs { local: None, remote: format!("{ip}:5000").parse().unwrap() }
    }

    async fn body(r: Response) -> (StatusCode, serde_json::Value) {
        let status = r.status();
        let bytes = to_bytes(r.into_body(), 1 << 20).await.unwrap();
        (status, serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null))
    }

    async fn res(r: Result<Response, ApiError>) -> (StatusCode, serde_json::Value) {
        match r {
            Ok(r) => body(r).await,
            Err((s, Json(v))) => (s, v),
        }
    }

    async fn password_login(state: &Arc<SignalingState>, ip: &str, pw: &str) -> (StatusCode, serde_json::Value) {
        let req = common::LoginRequest { username: "alice".into(), password: pw.into() };
        res(crate::login_handler(State(state.clone()), ConnectInfo(addrs(ip)), Default::default(), Json(req)).await).await
    }

    async fn mfa_login(
        state: &Arc<SignalingState>,
        ip: &str,
        token: &str,
        code: Option<&str>,
        recovery: Option<&str>,
    ) -> (StatusCode, serde_json::Value) {
        let req = MfaLoginRequest {
            mfa_token: token.into(),
            code: code.map(Into::into),
            recovery_code: recovery.map(Into::into),
        };
        res(login_mfa_handler(State(state.clone()), ConnectInfo(addrs(ip)), Default::default(), Json(req)).await).await
    }

    /// Sets alice up with two-factor sign-in; returns her secret and recovery codes.
    async fn turn_on(state: &Arc<SignalingState>) -> (Vec<u8>, Vec<String>) {
        let (s, v) = res(setup(alice(), State(state.clone()), Json(SetupRequest { password: "right-pw".into() })).await).await;
        assert_eq!(s, StatusCode::OK);
        let secret = totp_rs::Secret::try_from_base32(v["secret"].as_str().unwrap()).unwrap().as_bytes().to_vec();
        assert_eq!(secret.len(), 20);
        assert!(v["otpauth_url"].as_str().unwrap().contains("issuer=Lunaris"));
        assert!(v["qr"].as_str().unwrap().starts_with("data:image/svg+xml"));
        // Not on before the code is confirmed.
        assert!(!is_enabled(&state.db, "u1").await.unwrap());
        let (s, _) = res(enable(alice(), State(state.clone()), Json(CodeRequest { code: Some("000000".into()) })).await).await;
        // (000000 is right one time in a million.)
        assert_eq!(s, StatusCode::BAD_REQUEST);
        let code = code_at(&secret, now_secs());
        let (s, v) = res(enable(alice(), State(state.clone()), Json(CodeRequest { code: Some(code) })).await).await;
        assert_eq!(s, StatusCode::OK);
        let codes: Vec<String> = v["recovery_codes"].as_array().unwrap().iter().map(|c| c.as_str().unwrap().to_string()).collect();
        assert_eq!(codes.len(), RECOVERY_CODE_COUNT);
        assert!(is_enabled(&state.db, "u1").await.unwrap());
        // Stored encrypted, never as the secret itself.
        let stored: String = sqlx::query_scalar("SELECT secret FROM user_mfa WHERE user_id = 'u1'").fetch_one(&state.db).await.unwrap();
        assert!(!stored.contains(&hex::encode(&secret)));
        (secret, codes)
    }

    /// Lets the next code be used again (as if 30 s passed).
    async fn next_step(state: &Arc<SignalingState>) {
        sqlx::query("UPDATE user_mfa SET last_step = last_step - 5").execute(&state.db).await.unwrap();
    }

    #[tokio::test]
    async fn login_without_two_factor_is_unchanged() {
        let state = state_with_alice().await;
        let (s, v) = password_login(&state, "10.0.0.1", "right-pw").await;
        assert_eq!(s, StatusCode::OK);
        assert!(v["token"].is_string());
        assert_eq!(v["username"], "alice");
        assert_eq!(v["role"], "user");
        assert!(v.get("mfa_required").is_none());
        let (s, v) = res(status(alice(), State(state.clone())).await).await;
        assert_eq!(s, StatusCode::OK);
        assert_eq!(v["enabled"], false);
    }

    #[tokio::test]
    async fn login_with_code() {
        let state = state_with_alice().await;
        let (secret, _) = turn_on(&state).await;
        // The password alone gives no session.
        let (s, v) = password_login(&state, "10.0.0.1", "right-pw").await;
        assert_eq!(s, StatusCode::OK);
        assert_eq!(v["mfa_required"], true);
        assert!(v.get("token").is_none());
        let mfa_token = v["mfa_token"].as_str().unwrap().to_string();
        // ... and the mfa_token is no session token.
        assert!(crate::auth::verify_jwt(&mfa_token).is_err());
        // The code that turned it on can't be used again.
        let code = code_at(&secret, now_secs());
        let (s, _) = mfa_login(&state, "10.0.0.1", &mfa_token, Some(&code), None).await;
        assert_eq!(s, StatusCode::UNAUTHORIZED);
        next_step(&state).await;
        let (s, v) = mfa_login(&state, "10.0.0.1", &mfa_token, Some(&code), None).await;
        assert_eq!(s, StatusCode::OK, "{v}");
        let jwt = v["token"].as_str().unwrap();
        assert_eq!(crate::auth::verify_jwt(jwt).unwrap().sub, "u1");
        assert_eq!(v["username"], "alice");
        // The same code with a new sign-in: already used.
        let (_, v) = password_login(&state, "10.0.0.1", "right-pw").await;
        let t2 = v["mfa_token"].as_str().unwrap();
        let (s, _) = mfa_login(&state, "10.0.0.1", t2, Some(&code), None).await;
        assert_eq!(s, StatusCode::UNAUTHORIZED);
        // The mfa_token is used up (even with a code that would be good
        // again); a session token is not an mfa_token.
        next_step(&state).await;
        let (s, _) = mfa_login(&state, "10.0.0.1", &mfa_token, Some(&code), None).await;
        assert_eq!(s, StatusCode::UNAUTHORIZED);
        let (s, _) = mfa_login(&state, "10.0.0.1", jwt, Some(&code), None).await;
        assert_eq!(s, StatusCode::UNAUTHORIZED);
        // Wrong password: no mfa step at all.
        let (s, v) = password_login(&state, "10.0.0.1", "wrong").await;
        assert_eq!(s, StatusCode::UNAUTHORIZED);
        assert!(v.get("mfa_token").is_none());
    }

    #[tokio::test]
    async fn login_with_recovery_code_once() {
        let state = state_with_alice().await;
        let (_, codes) = turn_on(&state).await;
        let (_, v) = password_login(&state, "10.0.0.1", "right-pw").await;
        let t = v["mfa_token"].as_str().unwrap().to_string();
        let (s, _) = mfa_login(&state, "10.0.0.1", &t, None, Some("zzzz-zzzz-zzzz")).await;
        assert_eq!(s, StatusCode::UNAUTHORIZED);
        let typed = codes[0].to_uppercase();
        let (s, v) = mfa_login(&state, "10.0.0.1", &t, None, Some(&typed)).await;
        assert_eq!(s, StatusCode::OK, "{v}");
        let (_, v) = res(status(alice(), State(state.clone())).await).await;
        assert_eq!(v["recovery_codes_left"], RECOVERY_CODE_COUNT as i64 - 1);
        let (_, v) = password_login(&state, "10.0.0.1", "right-pw").await;
        let t = v["mfa_token"].as_str().unwrap().to_string();
        let (s, _) = mfa_login(&state, "10.0.0.1", &t, None, Some(&codes[0])).await;
        assert_eq!(s, StatusCode::UNAUTHORIZED);
        // Neither given: refused.
        let (s, _) = mfa_login(&state, "10.0.0.1", &t, None, None).await;
        assert_eq!(s, StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn wrong_codes_are_limited() {
        let state = state_with_alice().await;
        let (secret, _) = turn_on(&state).await;
        next_step(&state).await;
        // Per address: ten wrong codes over several sign-ins.
        let mut sent = 0;
        while sent < 10 {
            let (_, v) = password_login(&state, "10.0.0.9", "right-pw").await;
            let t = v["mfa_token"].as_str().unwrap().to_string();
            for _ in 0..PENDING_MAX_FAILURES {
                if sent == 10 {
                    break;
                }
                let (s, _) = mfa_login(&state, "10.0.0.9", &t, Some("000000"), None).await;
                assert_ne!(s, StatusCode::OK);
                sent += 1;
            }
            // After five wrong codes the token is gone.
        }
        let (_, v) = password_login(&state, "10.0.0.9", "right-pw").await;
        let t = v["mfa_token"].as_str().unwrap().to_string();
        let good = code_at(&secret, now_secs());
        let (s, _) = mfa_login(&state, "10.0.0.9", &t, Some(&good), None).await;
        assert_eq!(s, StatusCode::TOO_MANY_REQUESTS);
        // The user is limited from other addresses too (the codes were for her).
        let (_, v) = password_login(&state, "10.0.0.10", "right-pw").await;
        let t = v["mfa_token"].as_str().unwrap().to_string();
        let (s, _) = mfa_login(&state, "10.0.0.10", &t, Some(&good), None).await;
        assert_eq!(s, StatusCode::TOO_MANY_REQUESTS);
        // Guessing tokens counts per address.
        for _ in 0..10 {
            let (s, _) = mfa_login(&state, "10.0.0.11", "not-a-token", Some(&good), None).await;
            assert_eq!(s, StatusCode::UNAUTHORIZED);
        }
        let (s, _) = mfa_login(&state, "10.0.0.11", "not-a-token", Some(&good), None).await;
        assert_eq!(s, StatusCode::TOO_MANY_REQUESTS);
    }

    #[tokio::test]
    async fn disable_needs_password_and_code() {
        let state = state_with_alice().await;
        let (secret, codes) = turn_on(&state).await;
        next_step(&state).await;
        let code = code_at(&secret, now_secs());
        let off = |pw: &str, code: Option<&str>, rec: Option<&str>| DisableRequest {
            password: pw.into(),
            code: code.map(Into::into),
            recovery_code: rec.map(Into::into),
        };
        let (s, _) = res(disable(alice(), State(state.clone()), Json(off("right-pw", None, None))).await).await;
        assert_eq!(s, StatusCode::BAD_REQUEST);
        let (s, _) = res(disable(alice(), State(state.clone()), Json(off("wrong", Some(&code), None))).await).await;
        assert_eq!(s, StatusCode::BAD_REQUEST);
        // The wrong password did not spend the code.
        let (s, _) = res(disable(alice(), State(state.clone()), Json(off("right-pw", Some("000000"), None))).await).await;
        assert_eq!(s, StatusCode::BAD_REQUEST);
        assert!(is_enabled(&state.db, "u1").await.unwrap());
        // New recovery codes need a code; the old ones stop working.
        let (s, v) = res(regenerate_recovery_codes(alice(), State(state.clone()), Json(CodeRequest { code: Some(code.clone()) })).await).await;
        assert_eq!(s, StatusCode::OK);
        let fresh: Vec<String> = v["recovery_codes"].as_array().unwrap().iter().map(|c| c.as_str().unwrap().into()).collect();
        assert_eq!(fresh.len(), RECOVERY_CODE_COUNT);
        let (s, _) = res(regenerate_recovery_codes(alice(), State(state.clone()), Json(CodeRequest { code: Some(code.clone()) })).await).await;
        assert_eq!(s, StatusCode::BAD_REQUEST, "a code is used once");
        let (s, _) = res(disable(alice(), State(state.clone()), Json(off("right-pw", None, Some(&codes[1])))).await).await;
        assert_eq!(s, StatusCode::BAD_REQUEST);
        let (s, _) = res(disable(alice(), State(state.clone()), Json(off("right-pw", None, Some(&fresh[0])))).await).await;
        assert_eq!(s, StatusCode::OK);
        assert!(!is_enabled(&state.db, "u1").await.unwrap());
        let n: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM user_recovery_codes WHERE user_id = 'u1'").fetch_one(&state.db).await.unwrap();
        assert_eq!(n, 0);
        // Signing in is one step again.
        let (_, v) = password_login(&state, "10.0.0.1", "right-pw").await;
        assert!(v["token"].is_string());
    }

    #[tokio::test]
    async fn admin_resets_two_factor() {
        let state = state_with_alice().await;
        turn_on(&state).await;
        let (_, v) = password_login(&state, "10.0.0.1", "right-pw").await;
        let pending = v["mfa_token"].as_str().unwrap().to_string();
        let boss = || AdminUser { user_id: "a1".into(), username: "boss".into() };
        let (s, _) = res(admin_reset(boss(), State(state.clone()), Path("nobody".into())).await).await;
        assert_eq!(s, StatusCode::NOT_FOUND);
        let (s, _) = res(admin_reset(boss(), State(state.clone()), Path("u1".into())).await).await;
        assert_eq!(s, StatusCode::OK);
        assert!(!is_enabled(&state.db, "u1").await.unwrap());
        let (s, _) = res(admin_reset(boss(), State(state.clone()), Path("u1".into())).await).await;
        assert_eq!(s, StatusCode::NOT_FOUND);
        // A sign-in waiting for its code can't finish with no secret left.
        let (s, _) = mfa_login(&state, "10.0.0.1", &pending, Some("123456"), None).await;
        assert_eq!(s, StatusCode::UNAUTHORIZED);
        let (_, v) = password_login(&state, "10.0.0.1", "right-pw").await;
        assert!(v["token"].is_string());
        // Only admins may reset: the route's AdminUser extractor (auth.rs)
        // refuses others; the user list shows who has it on.
        turn_on(&state).await;
        let users = crate::admin::list_users(boss(), State(state.clone())).await.ok().unwrap().into_response();
        let (_, v) = body(users).await;
        let alice = v.as_array().unwrap().iter().find(|u| u["username"] == "alice").unwrap();
        assert_eq!(alice["mfa_enabled"], true);
        let boss_row = v.as_array().unwrap().iter().find(|u| u["username"] == "boss").unwrap();
        assert_eq!(boss_row["mfa_enabled"], false);
    }

    /// Whether `token` passes the session extractor (auth.rs).
    async fn accepted(state: &Arc<SignalingState>, token: &str) -> bool {
        use axum::extract::FromRequestParts;
        let mut parts = axum::http::Request::builder()
            .header("Authorization", format!("Bearer {token}"))
            .body(())
            .unwrap()
            .into_parts()
            .0;
        AuthenticatedUser::from_request_parts(&mut parts, state).await.is_ok()
    }

    async fn session(state: &Arc<SignalingState>) -> String {
        let v = crate::auth::token_version(&state.db, "u1").await.unwrap();
        crate::auth::create_jwt("u1", "alice", "user", v).unwrap()
    }

    #[tokio::test]
    async fn sessions_end_on_new_password_and_two_factor_off() {
        let state = state_with_alice().await;
        // An mfa_token never passes as a session.
        turn_on(&state).await;
        let (_, v) = password_login(&state, "10.0.0.1", "right-pw").await;
        assert!(!accepted(&state, v["mfa_token"].as_str().unwrap()).await);

        // Turning two-factor off ends the other sessions; the answer carries
        // a new token for this one.
        let old = session(&state).await;
        assert!(accepted(&state, &old).await);
        let stored: String = sqlx::query_scalar("SELECT secret FROM user_mfa WHERE user_id = 'u1'").fetch_one(&state.db).await.unwrap();
        let secret = open(&MFA_KEY, "u1", &stored).unwrap();
        next_step(&state).await;
        let req = DisableRequest { password: "right-pw".into(), code: Some(code_at(&secret, now_secs())), recovery_code: None };
        let (s, v) = res(disable(alice(), State(state.clone()), Json(req)).await).await;
        assert_eq!(s, StatusCode::OK);
        assert!(!accepted(&state, &old).await);
        assert!(accepted(&state, v["token"].as_str().unwrap()).await);

        // An admin's reset of two-factor ends them too.
        turn_on(&state).await;
        let before = session(&state).await;
        let boss = AdminUser { user_id: "a1".into(), username: "boss".into() };
        let (s, _) = res(admin_reset(boss, State(state.clone()), Path("u1".into())).await).await;
        assert_eq!(s, StatusCode::OK);
        assert!(!accepted(&state, &before).await);

        // So does a new password set by an admin; a new sign-in works.
        let before = session(&state).await;
        assert!(accepted(&state, &before).await);
        let boss = AdminUser { user_id: "a1".into(), username: "boss".into() };
        let change = common::UpdateUserRequest { role: None, is_active: None, password: Some("new-password".into()), group_ids: None };
        let r = crate::admin::update_user(boss, State(state.clone()), Path("u1".into()), Json(change)).await;
        assert!(r.is_ok());
        assert!(!accepted(&state, &before).await);
        let (s, v) = password_login(&state, "10.0.0.1", "new-password").await;
        assert_eq!(s, StatusCode::OK);
        assert!(accepted(&state, v["token"].as_str().unwrap()).await);
        // Tokens from before the version claim existed are version 0: still
        // good for users whose version was never raised.
        assert!(accepted(&state, &crate::auth::create_jwt("a1", "boss", "admin", 0).unwrap()).await);
    }

    #[tokio::test]
    async fn setup_again_replaces_an_unconfirmed_secret_only() {
        let state = state_with_alice().await;
        let (_, v1) = res(setup(alice(), State(state.clone()), Json(SetupRequest { password: "right-pw".into() })).await).await;
        let (_, v2) = res(setup(alice(), State(state.clone()), Json(SetupRequest { password: "right-pw".into() })).await).await;
        assert_ne!(v1["secret"], v2["secret"]);
        // A code from the first secret no longer turns it on.
        let first = totp_rs::Secret::try_from_base32(v1["secret"].as_str().unwrap()).unwrap().as_bytes().to_vec();
        let (s, _) = res(enable(alice(), State(state.clone()), Json(CodeRequest { code: Some(code_at(&first, now_secs())) })).await).await;
        assert_eq!(s, StatusCode::BAD_REQUEST);
        let state2 = state_with_alice().await;
        turn_on(&state2).await;
        let (s, _) = res(setup(alice(), State(state2.clone()), Json(SetupRequest { password: "right-pw".into() })).await).await;
        assert_eq!(s, StatusCode::CONFLICT);
    }

    /// Starting two-factor set-up asks for the account password again; a
    /// wrong one is 403 `wrong_password` (not 401: that means "signed out").
    #[tokio::test]
    async fn setup_needs_the_password() {
        let state = state_with_alice().await;
        for pw in ["", "wrong"] {
            let (s, v) = res(setup(alice(), State(state.clone()), Json(SetupRequest { password: pw.into() })).await).await;
            assert_eq!(s, StatusCode::FORBIDDEN);
            assert_eq!(v["code"], "wrong_password");
        }
        let n: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM user_mfa").fetch_one(&state.db).await.unwrap();
        assert_eq!(n, 0, "no secret made");
        // Wrong passwords are limited like wrong codes.
        for _ in 0..8 {
            let _ = setup(alice(), State(state.clone()), Json(SetupRequest { password: "wrong".into() })).await;
        }
        let (s, _) = res(setup(alice(), State(state.clone()), Json(SetupRequest { password: "right-pw".into() })).await).await;
        assert_eq!(s, StatusCode::TOO_MANY_REQUESTS);
    }

    /// Codes sent at once, over several pending sign-ins of one user: at
    /// most the per-user limit of them are checked.
    #[tokio::test]
    async fn codes_at_once_are_limited_per_user() {
        let state = state_with_alice().await;
        turn_on(&state).await;
        let mut tokens = Vec::new();
        for i in 0..3 {
            let (_, v) = password_login(&state, &format!("10.0.1.{i}"), "right-pw").await;
            tokens.push(v["mfa_token"].as_str().unwrap().to_string());
        }
        let tries = (0..60).map(|i| {
            let (state, token) = (state.clone(), tokens[i % 3].clone());
            async move { mfa_login(&state, &format!("10.0.2.{i}"), &token, Some("000000"), None).await.0 }
        });
        let codes = futures_util::future::join_all(tries).await;
        let checked = codes.iter().filter(|c| **c == StatusCode::UNAUTHORIZED).count();
        assert!(checked <= 10, "{checked} codes were checked");
    }

    #[test]
    fn pending_sign_ins_per_user_are_capped() {
        let state = MfaState::default();
        let t0 = Instant::now();
        let tokens: Vec<String> = (0..PENDING_MAX_PER_USER + 2).map(|i| state.issue("u1", t0 + Duration::from_millis(i as u64))).collect();
        let live = tokens.iter().filter(|t| state.peek(t, t0).is_some()).count();
        assert_eq!(live, PENDING_MAX_PER_USER);
        // The newest are the ones kept; other users are not touched.
        assert!(state.peek(tokens.last().unwrap(), t0).is_some());
        assert!(state.peek(&tokens[0], t0).is_none());
        let other = state.issue("u2", t0);
        assert!(state.peek(&other, t0).is_some());
    }
}
