use axum::{
    extract::FromRequestParts,
    http::{request::Parts, StatusCode},
};
use jsonwebtoken::{decode, encode, DecodingKey, EncodingKey, Header, Validation};
use once_cell::sync::Lazy;
use serde::{Deserialize, Serialize};

/// The secret older builds fell back to; anyone could sign tokens with it.
const OLD_DEFAULT_SECRET: &str = "lunaris-dev-secret-change-in-production";

static JWT_SECRET: Lazy<Vec<u8>> = Lazy::new(|| {
    #[cfg(test)]
    {
        crate::security::random_hex(32).into_bytes()
    }
    #[cfg(not(test))]
    {
        choose_secret(std::env::var("JWT_SECRET").ok(), &crate::paths::data_file("jwt_secret.txt"))
    }
});

/// Shortest `JWT_SECRET` accepted (bytes): a short one can be guessed offline
/// from any token.
pub const MIN_JWT_SECRET: usize = 32;

/// Checked at start (lib.rs run_server): a `JWT_SECRET` that is set but too
/// short stops the server with this message, instead of signing with it.
pub fn check_jwt_secret_env() -> Result<(), String> {
    jwt_secret_problem(std::env::var("JWT_SECRET").ok().as_deref()).map_or(Ok(()), Err)
}

fn jwt_secret_problem(env: Option<&str>) -> Option<String> {
    let secret = env.map(str::trim).filter(|s| !s.is_empty() && *s != OLD_DEFAULT_SECRET)?;
    (secret.len() < MIN_JWT_SECRET).then(|| {
        format!(
            "JWT_SECRET is too short ({} bytes; at least {} needed). Use a long random value \
             (e.g. `openssl rand -hex 32`), or unset it to let the server keep its own key.",
            secret.len(),
            MIN_JWT_SECRET
        )
    })
}

/// `JWT_SECRET` when set (never the old default, never a short one: the
/// server refuses to start with that), otherwise a random secret kept in
/// `file` so sign-ins survive a restart.
fn choose_secret(env: Option<String>, file: &std::path::Path) -> Vec<u8> {
    if let Some(secret) = env.map(|s| s.trim().to_string()) {
        if secret.len() >= MIN_JWT_SECRET && secret != OLD_DEFAULT_SECRET {
            return secret.into_bytes();
        }
        if secret == OLD_DEFAULT_SECRET {
            tracing::warn!("JWT_SECRET is the old public default; ignoring it");
        }
    }
    if let Ok(saved) = std::fs::read_to_string(file) {
        let saved = saved.trim();
        if saved.len() >= 32 && saved != OLD_DEFAULT_SECRET {
            return saved.as_bytes().to_vec();
        }
    }
    let secret = crate::security::random_hex(32);
    if let Err(e) = crate::paths::write_private(file, &secret) {
        tracing::error!("Cannot save the sign-in key to {:?} (sign-ins end at restart): {}", file, e);
    }
    secret.into_bytes()
}

/// A valid bcrypt hash (cost 12) checked for unknown usernames, so they take
/// as long as wrong passwords.
pub const DUMMY_HASH: &str = "$2b$12$rhTXdG3k9pjCa6u1.pNncOA9kj.STXQU8E5kun0D6dJtQRVnVtVR.";

#[derive(Debug, Serialize, Deserialize)]
pub struct Claims {
    pub sub: String, // User ID
    pub username: String,
    pub role: String,
    pub exp: u64,
    /// The user's `token_version` when the token was made; a token from
    /// before the last bump is refused. Tokens from before this claim count
    /// as version 0.
    #[serde(default)]
    pub ver: i64,
    /// Set on tokens from a `lunaris://connect` link ticket (link_tickets.rs):
    /// the one computer they may open over the client WebSocket. Such a token
    /// is refused by every HTTP API (`AuthenticatedUser`), and the signaling
    /// socket lets it address no other host. None: a normal sign-in.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host: Option<String>,
    /// With `host`: the source of that host the link is for (sources.rs);
    /// None = the host's own screen. Only that one may be opened.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
}

pub fn hash_password(password: &str) -> Result<String, bcrypt::BcryptError> {
    bcrypt::hash(password, bcrypt::DEFAULT_COST)
}

pub fn verify_password(password: &str, hash: &str) -> bool {
    bcrypt::verify(password, hash).unwrap_or(false)
}

/// bcrypt is slow on purpose: run it off the async workers.
pub async fn hash_password_blocking(password: String) -> Result<String, String> {
    match tokio::task::spawn_blocking(move || hash_password(&password)).await {
        Ok(r) => r.map_err(|e| e.to_string()),
        Err(e) => Err(e.to_string()),
    }
}

pub async fn verify_password_blocking(password: String, hash: String) -> bool {
    tokio::task::spawn_blocking(move || verify_password(&password, &hash))
        .await
        .unwrap_or(false)
}

pub fn create_jwt(
    user_id: &str,
    username: &str,
    role: &str,
    token_version: i64,
) -> Result<String, jsonwebtoken::errors::Error> {
    sign(user_id, username, role, token_version, chrono::Duration::days(7), None)
}

/// How long a token from a link ticket lasts: one sitting at the computer.
/// The desktop app opens a new client WebSocket with it for every (re)start
/// of the stream (new settings, a dropped connection), so it can't be
/// minutes; an open WebSocket is not cut when it expires.
pub const HOST_TOKEN_HOURS: i64 = 12;

/// A token that opens only `host_id`, or only its source `source_id` (see
/// `Claims::host`).
pub fn create_host_jwt(
    user_id: &str,
    username: &str,
    role: &str,
    token_version: i64,
    host_id: &str,
    source_id: Option<&str>,
) -> Result<String, jsonwebtoken::errors::Error> {
    sign(
        user_id,
        username,
        role,
        token_version,
        chrono::Duration::hours(HOST_TOKEN_HOURS),
        Some((host_id.to_string(), source_id.map(str::to_string))),
    )
}

fn sign(
    user_id: &str,
    username: &str,
    role: &str,
    token_version: i64,
    lifetime: chrono::Duration,
    scope: Option<(String, Option<String>)>,
) -> Result<String, jsonwebtoken::errors::Error> {
    let (host, source) = match scope {
        Some((host, source)) => (Some(host), source),
        None => (None, None),
    };
    let expiration = chrono::Utc::now()
        .checked_add_signed(lifetime)
        .expect("valid timestamp")
        .timestamp() as u64;

    let claims = Claims {
        sub: user_id.to_string(),
        username: username.to_string(),
        role: role.to_string(),
        exp: expiration,
        ver: token_version,
        host,
        source,
    };

    encode(
        &Header::default(),
        &claims,
        &EncodingKey::from_secret(&JWT_SECRET),
    )
}

pub fn verify_jwt(token: &str) -> Result<Claims, jsonwebtoken::errors::Error> {
    let token_data = decode::<Claims>(
        token,
        &DecodingKey::from_secret(&JWT_SECRET),
        &Validation::default(),
    )?;
    Ok(token_data.claims)
}

use crate::signaling::SignalingState;
use std::sync::Arc;

/// Why a valid-looking session token is not accepted.
#[derive(Debug, PartialEq, Eq)]
pub enum SessionCheck {
    /// Good: the user's current role (from the database, not the token).
    Ok(String),
    Disabled,
    UnknownUser,
    /// Made before the user's last password change or two-factor reset.
    Revoked,
}

/// Whether the session `claims` stand for is still good.
pub async fn check_session(db: &sqlx::SqlitePool, claims: &Claims) -> Result<SessionCheck, sqlx::Error> {
    let row: Option<(String, i64, i64)> =
        sqlx::query_as("SELECT COALESCE(role, 'user'), is_active, token_version FROM users WHERE id = ?")
            .bind(&claims.sub)
            .fetch_optional(db)
            .await?;
    Ok(match row {
        None => SessionCheck::UnknownUser,
        Some((_, 0, _)) => SessionCheck::Disabled,
        Some((_, _, version)) if version != claims.ver => SessionCheck::Revoked,
        Some((role, _, _)) => SessionCheck::Ok(role),
    })
}

/// The user's current token version (0 for unknown users).
pub async fn token_version(db: &sqlx::SqlitePool, user_id: &str) -> Result<i64, sqlx::Error> {
    let v: Option<i64> = sqlx::query_scalar("SELECT token_version FROM users WHERE id = ?")
        .bind(user_id)
        .fetch_optional(db)
        .await?;
    Ok(v.unwrap_or(0))
}

/// Ends every session of a user (new password, two-factor turned off):
/// their tokens stop working and their open connections are closed. Returns
/// the new token version.
pub async fn revoke_sessions(state: &SignalingState, user_id: &str) -> Result<i64, sqlx::Error> {
    sqlx::query("UPDATE users SET token_version = token_version + 1 WHERE id = ?")
        .bind(user_id)
        .execute(&state.db)
        .await?;
    state.close_user_connections(user_id);
    token_version(&state.db, user_id).await
}

#[allow(dead_code)]
pub struct AuthenticatedUser {
    pub user_id: String,
    pub username: String,
    pub role: String,
}

impl FromRequestParts<Arc<SignalingState>> for AuthenticatedUser {
    type Rejection = (StatusCode, &'static str);

    async fn from_request_parts(
        parts: &mut Parts,
        state: &Arc<SignalingState>,
    ) -> Result<Self, Self::Rejection> {
        let auth_header = parts
            .headers
            .get("Authorization")
            .and_then(|value| value.to_str().ok())
            .ok_or((StatusCode::UNAUTHORIZED, "Missing Authorization Header"))?;

        if !auth_header.starts_with("Bearer ") {
            return Err((
                StatusCode::UNAUTHORIZED,
                "Invalid Authorization Header Format",
            ));
        }

        let token = &auth_header[7..];
        let claims = verify_jwt(token).map_err(|_| (StatusCode::UNAUTHORIZED, "Invalid Token"))?;
        // A token from a link opens one computer over the client WebSocket
        // and nothing else: no computer list, no settings, no admin pages.
        if claims.host.is_some() {
            return Err((StatusCode::FORBIDDEN, "This sign-in only opens one computer"));
        }

        // The user must still exist and be active, and the token must be
        // from after their last password change; the role comes from the database.
        let status = check_session(&state.db, &claims).await.map_err(|e| {
            tracing::error!("Database verification error: {:?}", e);
            (StatusCode::INTERNAL_SERVER_ERROR, "Database Error")
        })?;

        match status {
            SessionCheck::Ok(role) => Ok(AuthenticatedUser {
                user_id: claims.sub,
                username: claims.username,
                role,
            }),
            SessionCheck::Disabled => Err((StatusCode::UNAUTHORIZED, "Account is disabled")),
            SessionCheck::UnknownUser => Err((StatusCode::UNAUTHORIZED, "User does not exist in database")),
            SessionCheck::Revoked => Err((StatusCode::UNAUTHORIZED, "Session ended. Sign in again.")),
        }
    }
}

#[allow(dead_code)]
pub struct AdminUser {
    pub user_id: String,
    pub username: String,
}

impl FromRequestParts<Arc<SignalingState>> for AdminUser {
    type Rejection = (StatusCode, &'static str);

    async fn from_request_parts(
        parts: &mut Parts,
        state: &Arc<SignalingState>,
    ) -> Result<Self, Self::Rejection> {
        let user = AuthenticatedUser::from_request_parts(parts, state).await?;
        if user.role != "admin" {
            return Err((StatusCode::FORBIDDEN, "Admin access required"));
        }
        Ok(AdminUser {
            user_id: user.user_id,
            username: user.username,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Hash produced by bcrypt 0.15 (before the upgrade) for "correct horse".
    const OLD_HASH: &str = "$2b$12$rhTXdG3k9pjCa6u1.pNncOA9kj.STXQU8E5kun0D6dJtQRVnVtVR.";

    #[test]
    fn old_bcrypt_hash_still_verifies() {
        assert!(verify_password("correct horse", OLD_HASH));
        assert!(!verify_password("wrong horse", OLD_HASH));
    }

    #[test]
    fn new_hash_roundtrip_and_format() {
        let h = hash_password("pw").unwrap();
        assert!(h.starts_with("$2b$12$"), "{h}");
        assert!(verify_password("pw", &h));
    }

    #[test]
    fn token_signed_with_old_default_secret_is_rejected() {
        let claims = Claims {
            sub: "id1".into(),
            username: "admin".into(),
            role: "admin".into(),
            exp: (chrono::Utc::now().timestamp() + 3600) as u64,
            ver: 0,
            host: None,
            source: None,
        };
        let forged = encode(
            &Header::default(),
            &claims,
            &EncodingKey::from_secret(OLD_DEFAULT_SECRET.as_bytes()),
        )
        .unwrap();
        assert!(verify_jwt(&forged).is_err());
    }

    #[test]
    fn secret_never_the_old_default() {
        let dir = std::env::temp_dir().join(format!("lunaris-jwt-{}", crate::security::random_hex(8)));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("jwt_secret.txt");
        // The old default in the environment is ignored; a random one is saved.
        let s1 = choose_secret(Some(OLD_DEFAULT_SECRET.into()), &file);
        assert_ne!(s1, OLD_DEFAULT_SECRET.as_bytes());
        assert_eq!(s1.len(), 64);
        // Kept across restarts.
        assert_eq!(choose_secret(None, &file), s1);
        // An explicit secret wins.
        let own = "my-own-secret-of-at-least-32-bytes!";
        assert_eq!(choose_secret(Some(own.into()), &file), own.as_bytes());
        // A short one is never used (and stops the server at start).
        assert_eq!(choose_secret(Some("my-own-secret".into()), &file), s1);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&file).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn short_jwt_secret_stops_the_server() {
        assert!(jwt_secret_problem(None).is_none());
        assert!(jwt_secret_problem(Some("  ")).is_none());
        // The old default is ignored with a warning, not fatal.
        assert!(jwt_secret_problem(Some(OLD_DEFAULT_SECRET)).is_none());
        let msg = jwt_secret_problem(Some("short-secret")).unwrap();
        assert!(msg.contains("too short") && !msg.contains("short-secret"), "{msg}");
        assert!(jwt_secret_problem(Some(&"x".repeat(31))).is_some());
        assert!(jwt_secret_problem(Some(&"x".repeat(32))).is_none());
    }

    #[test]
    fn dummy_hash_is_valid_bcrypt() {
        assert!(!verify_password("anything", DUMMY_HASH));
        assert!(bcrypt::verify("x", DUMMY_HASH).is_ok());
    }

    #[test]
    fn jwt_roundtrip_hs256() {
        // A token from before the version claim reads as version 0.
        #[derive(Serialize)]
        struct OldClaims<'a> { sub: &'a str, username: &'a str, role: &'a str, exp: u64 }
        let old = encode(
            &Header::default(),
            &OldClaims { sub: "id1", username: "bob", role: "user", exp: (chrono::Utc::now().timestamp() + 3600) as u64 },
            &EncodingKey::from_secret(&JWT_SECRET),
        )
        .unwrap();
        assert_eq!(verify_jwt(&old).unwrap().ver, 0);
        assert_eq!(verify_jwt(&create_jwt("id1", "bob", "user", 3).unwrap()).unwrap().ver, 3);

        let t = create_jwt("id1", "bob", "admin", 0).unwrap();
        let header = jsonwebtoken::decode_header(&t).unwrap();
        assert_eq!(header.alg, jsonwebtoken::Algorithm::HS256);
        let c = verify_jwt(&t).unwrap();
        assert_eq!(c.sub, "id1");
        assert_eq!(c.host, None);
    }

    #[test]
    fn host_token_names_its_host_and_lasts_hours() {
        let c = verify_jwt(&create_host_jwt("id1", "bob", "user", 2, "h1", None).unwrap()).unwrap();
        assert_eq!(c.host.as_deref(), Some("h1"));
        assert_eq!(c.source, None);
        let c = verify_jwt(&create_host_jwt("id1", "bob", "user", 2, "h1", Some("s1")).unwrap()).unwrap();
        assert_eq!((c.host.as_deref(), c.source.as_deref()), (Some("h1"), Some("s1")));
        assert_eq!(c.ver, 2);
        let left = c.exp as i64 - chrono::Utc::now().timestamp();
        assert!(left > (HOST_TOKEN_HOURS - 1) * 3600 && left <= HOST_TOKEN_HOURS * 3600, "{left}");
    }
}
