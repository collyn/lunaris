//! First start: the first administrator is made in the web console with a
//! one-time setup code, not with a password the server picks.
//!
//! While there is no account the server keeps a setup code: its SHA-256 in
//! the database (`setup_code`), the code itself in `setup_code.txt` in the
//! data directory (owner only; on the Linux packages /var/lib/lunaris, on
//! Windows %ProgramData%\Lunaris\Server). The log names the file, never the
//! code (logs and apt's term.log get shared). Whoever can read that file
//! (root, Administrators) proves it in the web console and picks the first
//! admin's name and password: `GET /api/setup` says whether that is still
//! needed, `POST /api/setup` does it once. Once any account exists, setup is
//! closed for good.
//!
//! Headless installs (Docker) set `LUNARIS_ADMIN_PASSWORD` instead: the
//! `admin` account is made with it on the first start and there is no setup.

use axum::{
    extract::{ConnectInfo, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use serde::Deserialize;
use sqlx::SqlitePool;
use std::path::Path;
use std::sync::Arc;
use tracing::{error, info, warn};

use crate::{
    security,
    signaling::{ConnAddrs, SignalingState},
};

/// The code's file in the data directory.
pub const CODE_FILE: &str = "setup_code.txt";

/// Shortest password accepted (as for accounts made by an admin, admin.rs).
const MIN_PASSWORD: usize = 6;
const MAX_USERNAME: usize = 64;

async fn has_users(db: &SqlitePool) -> Result<bool, sqlx::Error> {
    let n: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM users").fetch_one(db).await?;
    Ok(n > 0)
}

/// The code as typed: spaces, dashes and case don't matter.
fn normalize(code: &str) -> String {
    code.chars().filter(|c| c.is_ascii_alphanumeric()).collect::<String>().to_ascii_lowercase()
}

/// At start, after the database is open: nothing when accounts exist (a
/// leftover code is dropped); the `admin` account from `env_password`
/// (LUNARIS_ADMIN_PASSWORD) when set; otherwise a setup code, kept across
/// restarts while its file still matches.
pub async fn prepare(db: &SqlitePool, code_file: &Path, env_password: Option<String>) -> Result<(), sqlx::Error> {
    if has_users(db).await? {
        close(db, code_file).await?;
        return Ok(());
    }
    if let Some(password) = env_password.filter(|p| !p.trim().is_empty()) {
        let hashed = crate::auth::hash_password_blocking(password)
            .await
            .map_err(|e| sqlx::Error::Protocol(format!("Hash error: {e}")))?;
        sqlx::query(
            "INSERT INTO users (id, username, password_hash, role, is_active, created_at)
             SELECT ?, 'admin', ?, 'admin', 1, datetime('now') WHERE NOT EXISTS (SELECT 1 FROM users)",
        )
        .bind(uuid::Uuid::new_v4().to_string())
        .bind(&hashed)
        .execute(db)
        .await?;
        close(db, code_file).await?;
        info!("Created the admin account 'admin' with the password from LUNARIS_ADMIN_PASSWORD");
        return Ok(());
    }

    let stored: Option<String> = sqlx::query_scalar("SELECT code_hash FROM setup_code WHERE id = 1")
        .fetch_optional(db)
        .await?;
    let on_file = std::fs::read_to_string(code_file).ok().map(|c| normalize(&c)).filter(|c| !c.is_empty());
    let still_valid = matches!((&stored, &on_file), (Some(hash), Some(code)) if security::ct_eq(hash, &security::sha256_hex(code)));
    if !still_valid {
        let code = security::random_hex(16);
        if let Err(e) = crate::paths::write_private(code_file, &format!("{code}\n")) {
            error!("Cannot write the setup code to {:?}: {} (set LUNARIS_ADMIN_PASSWORD to make the admin account instead)", code_file, e);
        }
        sqlx::query("INSERT OR REPLACE INTO setup_code (id, code_hash, created_at) VALUES (1, ?, datetime('now'))")
            .bind(security::sha256_hex(&code))
            .execute(db)
            .await?;
    }
    // The code stays out of the log; it is in the file only.
    info!("==================================================");
    info!(" No account yet. Open the web console and create the first admin");
    info!(" with the setup code in {:?}", code_file);
    info!("==================================================");
    Ok(())
}

/// Setup is over: the code goes (row and file).
async fn close(db: &SqlitePool, code_file: &Path) -> Result<(), sqlx::Error> {
    sqlx::query("DELETE FROM setup_code").execute(db).await?;
    if code_file.exists() {
        if let Err(e) = std::fs::remove_file(code_file) {
            warn!("Cannot remove {:?}: {}", code_file, e);
        }
    }
    Ok(())
}

fn error(status: StatusCode, message: &str) -> Response {
    (status, Json(serde_json::json!({ "error": message }))).into_response()
}

fn db_error(e: sqlx::Error) -> Response {
    error!("Database error: {}", e);
    error(StatusCode::INTERNAL_SERVER_ERROR, "Internal Database Error")
}

const CLOSED: &str = "Setup is already done. Sign in instead.";

/// GET /api/setup → `{needed}`: no account exists yet.
pub async fn status(State(state): State<Arc<SignalingState>>) -> Response {
    match has_users(&state.db).await {
        Ok(users) => Json(serde_json::json!({ "needed": !users })).into_response(),
        Err(e) => db_error(e),
    }
}

#[derive(Deserialize)]
pub struct SetupRequest {
    pub code: String,
    pub username: String,
    pub password: String,
}

/// POST /api/setup `{code, username, password}` → a sign-in like
/// /api/auth/login's, for the new admin.
pub async fn complete(
    State(state): State<Arc<SignalingState>>,
    ConnectInfo(addrs): ConnectInfo<ConnAddrs>,
    headers: axum::http::HeaderMap,
    Json(req): Json<SetupRequest>,
) -> Response {
    let ip = security::client_ip(addrs.remote.ip(), &headers).to_string();
    complete_with(&state, &ip, req, &crate::paths::data_file(CODE_FILE)).await
}

async fn complete_with(state: &SignalingState, ip: &str, req: SetupRequest, code_file: &Path) -> Response {
    let now = std::time::Instant::now();
    // Wrong codes count as failed sign-ins from this address.
    if state.login_ip_limit.is_blocked(ip, now) {
        return error(StatusCode::TOO_MANY_REQUESTS, "Too many failed sign-in attempts. Try again in a few minutes.");
    }
    match has_users(&state.db).await {
        Ok(true) => return error(StatusCode::CONFLICT, CLOSED),
        Ok(false) => {}
        Err(e) => return db_error(e),
    }
    let code_hash = security::sha256_hex(&normalize(&req.code));
    let stored: Option<String> = match sqlx::query_scalar("SELECT code_hash FROM setup_code WHERE id = 1")
        .fetch_optional(&state.db)
        .await
    {
        Ok(s) => s,
        Err(e) => return db_error(e),
    };
    if !matches!(&stored, Some(hash) if security::ct_eq(hash, &code_hash)) {
        state.login_ip_limit.record_failure(ip, now);
        return error(StatusCode::UNAUTHORIZED, "Wrong setup code");
    }

    let username = req.username.trim().to_string();
    if username.is_empty() {
        return error(StatusCode::BAD_REQUEST, "Username cannot be empty");
    }
    if username.chars().count() > MAX_USERNAME || username.chars().any(char::is_control) {
        return error(StatusCode::BAD_REQUEST, "Choose a shorter username without special characters");
    }
    if req.password.len() < MIN_PASSWORD {
        return error(StatusCode::BAD_REQUEST, "Password must be at least 6 characters");
    }
    let hashed = match crate::auth::hash_password_blocking(req.password).await {
        Ok(h) => h,
        Err(e) => {
            error!("Hashing error: {}", e);
            return error(StatusCode::INTERNAL_SERVER_ERROR, "Internal Hashing Error");
        }
    };

    // One transaction: the code is used up and the account made together,
    // and only while there is no account. Its first statement writes, so a
    // second request waits for this one and then finds the code gone.
    let user_id = uuid::Uuid::new_v4().to_string();
    let made = async {
        let mut tx = state.db.begin().await?;
        let used = sqlx::query("DELETE FROM setup_code WHERE id = 1 AND code_hash = ?")
            .bind(&code_hash)
            .execute(&mut *tx)
            .await?
            .rows_affected();
        if used != 1 {
            return Ok::<bool, sqlx::Error>(false);
        }
        let inserted = sqlx::query(
            "INSERT INTO users (id, username, password_hash, role, is_active, created_at)
             SELECT ?, ?, ?, 'admin', 1, datetime('now') WHERE NOT EXISTS (SELECT 1 FROM users)",
        )
        .bind(&user_id)
        .bind(&username)
        .bind(&hashed)
        .execute(&mut *tx)
        .await?
        .rows_affected();
        if inserted != 1 {
            return Ok(false);
        }
        tx.commit().await?;
        Ok(true)
    }
    .await;
    match made {
        Ok(true) => {}
        Ok(false) => return error(StatusCode::CONFLICT, CLOSED),
        Err(e) => return db_error(e),
    }
    state.login_ip_limit.reset(ip);
    if let Err(e) = std::fs::remove_file(code_file) {
        if e.kind() != std::io::ErrorKind::NotFound {
            warn!("Cannot remove {:?}: {}", code_file, e);
        }
    }
    info!("Setup done: created the admin account '{}'", username);

    match crate::auth::create_jwt(&user_id, &username, "admin", 0) {
        Ok(token) => (
            StatusCode::OK,
            Json(common::AuthResponse { token, username, role: "admin".into() }),
        )
            .into_response(),
        Err(e) => {
            error!("Token error: {}", e);
            error(StatusCode::INTERNAL_SERVER_ERROR, "Failed to generate token")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    struct Env {
        state: Arc<SignalingState>,
        dir: PathBuf,
    }

    impl Env {
        fn code_file(&self) -> PathBuf {
            self.dir.join(CODE_FILE)
        }
        fn code(&self) -> String {
            std::fs::read_to_string(self.code_file()).unwrap().trim().to_string()
        }
        async fn users(&self) -> Vec<(String, String)> {
            sqlx::query_as("SELECT username, role FROM users").fetch_all(&self.state.db).await.unwrap()
        }
        async fn setup(&self, ip: &str, code: &str, user: &str, pw: &str) -> (StatusCode, serde_json::Value) {
            let req = SetupRequest { code: code.into(), username: user.into(), password: pw.into() };
            body(complete_with(&self.state, ip, req, &self.code_file()).await).await
        }
        async fn needed(&self) -> bool {
            let (status, b) = body(status(State(self.state.clone())).await).await;
            assert_eq!(status, StatusCode::OK);
            b["needed"].as_bool().unwrap()
        }
    }

    impl Drop for Env {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    async fn env(admin_password: Option<&str>) -> Env {
        let dir = std::env::temp_dir().join(format!("lunaris-setup-{}", security::random_hex(8)));
        std::fs::create_dir_all(&dir).unwrap();
        let db = crate::db::init_db(&format!("sqlite://{}", dir.join("lunaris.db").display())).await.unwrap();
        prepare(&db, &dir.join(CODE_FILE), admin_password.map(String::from)).await.unwrap();
        Env { state: Arc::new(SignalingState::new(db, "t".into(), None)), dir }
    }

    async fn body(r: Response) -> (StatusCode, serde_json::Value) {
        let status = r.status();
        let bytes = axum::body::to_bytes(r.into_body(), 1 << 20).await.unwrap();
        (status, serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null))
    }

    #[tokio::test]
    async fn first_start_makes_a_code_and_no_account() {
        let env = env(None).await;
        assert!(env.users().await.is_empty());
        assert!(env.needed().await);
        let code = env.code();
        assert_eq!(code.len(), 32);
        // Only the hash is in the database.
        let stored: Vec<String> = sqlx::query_scalar("SELECT code_hash FROM setup_code").fetch_all(&env.state.db).await.unwrap();
        assert_eq!(stored, vec![security::sha256_hex(&code)]);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(std::fs::metadata(env.code_file()).unwrap().permissions().mode() & 0o777, 0o600);
        }
        // A restart keeps the same code while its file matches...
        prepare(&env.state.db, &env.code_file(), None).await.unwrap();
        assert_eq!(env.code(), code);
        // ...and makes a new one when the file is gone.
        std::fs::remove_file(env.code_file()).unwrap();
        prepare(&env.state.db, &env.code_file(), None).await.unwrap();
        assert_ne!(env.code(), code);
        // No first-admin password file any more.
        assert!(!env.dir.join("initial_admin_password.txt").exists());
    }

    #[tokio::test]
    async fn the_code_makes_the_first_admin_once() {
        let env = env(None).await;
        let code = env.code();
        // Typed with spaces and capitals.
        let typed = format!(" {} {} ", code[..16].to_uppercase(), &code[16..]);
        let (status, b) = env.setup("192.0.2.1", &typed, "  boss ", "secret1").await;
        assert_eq!(status, StatusCode::OK, "{b}");
        assert_eq!(b["username"], "boss");
        assert_eq!(b["role"], "admin");
        let claims = crate::auth::verify_jwt(b["token"].as_str().unwrap()).unwrap();
        assert_eq!(claims.username, "boss");
        assert_eq!(claims.role, "admin");
        assert_eq!(env.users().await, vec![("boss".to_string(), "admin".to_string())]);
        assert!(!env.code_file().exists(), "the code file goes");
        let left: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM setup_code").fetch_one(&env.state.db).await.unwrap();
        assert_eq!(left, 0);
        // The password works for sign-in.
        let hash: String = sqlx::query_scalar("SELECT password_hash FROM users").fetch_one(&env.state.db).await.unwrap();
        assert!(crate::auth::verify_password("secret1", &hash));

        // Closed for good.
        assert!(!env.needed().await);
        assert_eq!(env.setup("192.0.2.1", &code, "other", "secret2").await.0, StatusCode::CONFLICT);
        // A restart makes no new code.
        prepare(&env.state.db, &env.code_file(), None).await.unwrap();
        assert!(!env.code_file().exists());
        assert_eq!(env.users().await.len(), 1);
    }

    #[tokio::test]
    async fn bad_input_is_refused_without_using_the_code() {
        let env = env(None).await;
        let code = env.code();
        assert_eq!(env.setup("192.0.2.1", &code, "   ", "secret1").await.0, StatusCode::BAD_REQUEST);
        assert_eq!(env.setup("192.0.2.1", &code, "boss", "12345").await.0, StatusCode::BAD_REQUEST);
        assert_eq!(env.setup("192.0.2.1", &code, &"x".repeat(65), "secret1").await.0, StatusCode::BAD_REQUEST);
        assert!(env.users().await.is_empty());
        assert_eq!(env.setup("192.0.2.1", &code, "boss", "secret1").await.0, StatusCode::OK);
    }

    #[tokio::test]
    async fn wrong_codes_are_limited_per_address() {
        let env = env(None).await;
        let code = env.code();
        for _ in 0..10 {
            assert_eq!(env.setup("192.0.2.9", "00000000000000000000000000000000", "boss", "secret1").await.0, StatusCode::UNAUTHORIZED);
        }
        // Blocked now, even with the right code.
        assert_eq!(env.setup("192.0.2.9", &code, "boss", "secret1").await.0, StatusCode::TOO_MANY_REQUESTS);
        assert!(env.users().await.is_empty());
        // Another address is not.
        assert_eq!(env.setup("192.0.2.10", &code, "boss", "secret1").await.0, StatusCode::OK);
    }

    #[tokio::test]
    async fn two_setups_at_once_make_one_admin() {
        let env = env(None).await;
        let code = env.code();
        let (a, b) = tokio::join!(
            env.setup("192.0.2.1", &code, "first", "secret1"),
            env.setup("192.0.2.2", &code, "second", "secret2"),
        );
        let mut statuses = [a.0, b.0];
        statuses.sort();
        assert_eq!(statuses, [StatusCode::OK, StatusCode::CONFLICT], "{a:?} {b:?}");
        assert_eq!(env.users().await.len(), 1);
    }

    #[tokio::test]
    async fn the_env_password_makes_the_admin_and_skips_setup() {
        let env = env(Some("from-env-1")).await;
        assert_eq!(env.users().await, vec![("admin".to_string(), "admin".to_string())]);
        let hash: String = sqlx::query_scalar("SELECT password_hash FROM users").fetch_one(&env.state.db).await.unwrap();
        assert!(crate::auth::verify_password("from-env-1", &hash));
        assert!(!env.needed().await);
        assert!(!env.code_file().exists());
        assert!(!env.dir.join("initial_admin_password.txt").exists());
        // Only on the first start: a later one changes nothing.
        prepare(&env.state.db, &env.code_file(), Some("other-pass".into())).await.unwrap();
        let again: String = sqlx::query_scalar("SELECT password_hash FROM users").fetch_one(&env.state.db).await.unwrap();
        assert_eq!(hash, again);
    }

    #[tokio::test]
    async fn an_existing_install_is_left_alone() {
        let dir = std::env::temp_dir().join(format!("lunaris-setup-old-{}", security::random_hex(8)));
        std::fs::create_dir_all(&dir).unwrap();
        let db = crate::db::init_db(&format!("sqlite://{}", dir.join("lunaris.db").display())).await.unwrap();
        sqlx::query("INSERT INTO users (id, username, password_hash, role, is_active) VALUES ('a', 'admin', 'x', 'admin', 1)")
            .execute(&db)
            .await
            .unwrap();
        // An older version's password file is the owner's: kept.
        std::fs::write(dir.join("initial_admin_password.txt"), "old\n").unwrap();
        prepare(&db, &dir.join(CODE_FILE), None).await.unwrap();
        assert!(!dir.join(CODE_FILE).exists());
        assert!(dir.join("initial_admin_password.txt").exists());
        let n: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM users").fetch_one(&db).await.unwrap();
        assert_eq!(n, 1);
        db.close().await;
        let _ = std::fs::remove_dir_all(&dir);
    }
}
