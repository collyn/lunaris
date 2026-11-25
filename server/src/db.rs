use sqlx::{
    sqlite::{SqliteConnectOptions, SqlitePoolOptions},
    SqlitePool,
};
use std::fs;
use std::str::FromStr;

/// Opens (and migrates) the database. It makes no account: the first admin
/// comes from the setup page or LUNARIS_ADMIN_PASSWORD (setup.rs).
pub async fn init_db(database_url: &str) -> Result<SqlitePool, sqlx::Error> {
    // If it's a file database, ensure parent directories exist
    if database_url.starts_with("sqlite://") {
        let path = database_url.trim_start_matches("sqlite://");
        let path = path.split('?').next().unwrap_or(path);
        if path != ":memory:" && !path.is_empty() {
            if let Some(parent) = std::path::Path::new(path).parent() {
                if !parent.as_os_str().is_empty() {
                    fs::create_dir_all(parent).unwrap_or_default();
                }
            }
            make_private(std::path::Path::new(path));
        }
    }

    let options = SqliteConnectOptions::from_str(database_url)?.create_if_missing(true);

    let pool = SqlitePoolOptions::new()
        .max_connections(5)
        .connect_with(options)
        .await?;

    // Enable foreign keys
    sqlx::query("PRAGMA foreign_keys = ON;")
        .execute(&pool)
        .await?;

    // Create tables
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS users (
            id TEXT PRIMARY KEY,
            username TEXT UNIQUE NOT NULL,
            password_hash TEXT NOT NULL
        );",
    )
    .execute(&pool)
    .await?;

    sqlx::query(
        "CREATE TABLE IF NOT EXISTS hosts (
            id TEXT PRIMARY KEY,
            name TEXT NOT NULL,
            status TEXT NOT NULL,
            ip_address TEXT,
            owner_id TEXT,
            client_unique_id TEXT,
            client_private_key TEXT,
            client_certificate TEXT,
            server_certificate TEXT,
            server_codec_mode_support INTEGER DEFAULT 0,
            FOREIGN KEY(owner_id) REFERENCES users(id)
        );",
    )
    .execute(&pool)
    .await?;

    sqlx::query(
        "CREATE TABLE IF NOT EXISTS groups (
            id TEXT PRIMARY KEY,
            name TEXT UNIQUE NOT NULL,
            note TEXT DEFAULT '',
            created_at TEXT DEFAULT CURRENT_TIMESTAMP
        );",
    )
    .execute(&pool)
    .await?;

    sqlx::query(
        "CREATE TABLE IF NOT EXISTS user_groups (
            user_id TEXT NOT NULL,
            group_id TEXT NOT NULL,
            PRIMARY KEY (user_id, group_id),
            FOREIGN KEY(user_id) REFERENCES users(id) ON DELETE CASCADE,
            FOREIGN KEY(group_id) REFERENCES groups(id) ON DELETE CASCADE
        );",
    )
    .execute(&pool)
    .await?;

    sqlx::query(
        "CREATE TABLE IF NOT EXISTS host_groups (
            host_id TEXT NOT NULL,
            group_id TEXT NOT NULL,
            PRIMARY KEY (host_id, group_id),
            FOREIGN KEY(host_id) REFERENCES hosts(id) ON DELETE CASCADE,
            FOREIGN KEY(group_id) REFERENCES groups(id) ON DELETE CASCADE
        );",
    )
    .execute(&pool)
    .await?;

    sqlx::query(
        "CREATE TABLE IF NOT EXISTS turn_servers (
            id TEXT PRIMARY KEY,
            urls TEXT NOT NULL,
            username TEXT,
            credential TEXT,
            created_at TEXT DEFAULT CURRENT_TIMESTAMP
        );",
    )
    .execute(&pool)
    .await?;

    // Computers added by address (links.rs): the server connects to them.
    // The password is the agent's access password, kept to reconnect.
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS address_hosts (
            address TEXT PRIMARY KEY,
            password TEXT NOT NULL DEFAULT '',
            host_id TEXT,
            created_at TEXT DEFAULT CURRENT_TIMESTAMP
        );",
    )
    .execute(&pool)
    .await?;

    // The name given when it was added (or edited); '' when none.
    let address_columns: Vec<String> = sqlx::query_scalar("SELECT name FROM pragma_table_info('address_hosts')")
        .fetch_all(&pool)
        .await?;
    if !address_columns.iter().any(|c| c == "name") {
        sqlx::query("ALTER TABLE address_hosts ADD COLUMN name TEXT NOT NULL DEFAULT '';")
            .execute(&pool)
            .await?;
    }
    // The SHA-256 (hex) of the agent's TLS certificate, pinned on the first
    // connection (links.rs); NULL until then (rows from before: pinned on
    // their next connection). An admin's key reset clears it.
    if !address_columns.iter().any(|c| c == "cert_sha256") {
        sqlx::query("ALTER TABLE address_hosts ADD COLUMN cert_sha256 TEXT;")
            .execute(&pool)
            .await?;
    }
    // The newest direct-handshake version the agent spoke (2: SPAKE2),
    // links.rs; NULL until then. Once 2, an answer with only v1 is refused
    // like another certificate; an admin's key reset clears it too.
    if !address_columns.iter().any(|c| c == "pake_version") {
        sqlx::query("ALTER TABLE address_hosts ADD COLUMN pake_version INTEGER;")
            .execute(&pool)
            .await?;
    }

    // Computers of other types that a Lunaris computer shows (sources.rs):
    // what its agent announced. Nothing of how they are reached is here.
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS host_sources (
            host_id TEXT NOT NULL REFERENCES hosts(id) ON DELETE CASCADE,
            source_id TEXT NOT NULL,
            name TEXT NOT NULL,
            connector TEXT NOT NULL,
            connector_name TEXT NOT NULL DEFAULT '',
            PRIMARY KEY (host_id, source_id)
        );",
    )
    .execute(&pool)
    .await?;

    // Sources removed from the list while their agent was offline: the agent
    // still has them and is asked to remove them when it is back.
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS source_removals (
            host_id TEXT NOT NULL,
            source_id TEXT NOT NULL,
            PRIMARY KEY (host_id, source_id)
        );",
    )
    .execute(&pool)
    .await?;

    // Which computer a source is, when that computer is listed in other
    // ways too (sources.rs): an opaque word its agent announced.
    let source_columns: Vec<String> = sqlx::query_scalar("SELECT name FROM pragma_table_info('host_sources')")
        .fetch_all(&pool)
        .await?;
    if !source_columns.iter().any(|c| c == "source_group") {
        sqlx::query("ALTER TABLE host_sources ADD COLUMN source_group TEXT;")
            .execute(&pool)
            .await?;
    }
    // The fields of its form that are not secrets (JSON object), as its
    // agent announced them: for its card and an edit form.
    if !source_columns.iter().any(|c| c == "source_values") {
        sqlx::query("ALTER TABLE host_sources ADD COLUMN source_values TEXT;")
            .execute(&pool)
            .await?;
    }

    // Perform migrations for hosts table if columns don't exist
    let columns: Vec<String> = sqlx::query_scalar("SELECT name FROM pragma_table_info('hosts')")
        .fetch_all(&pool)
        .await?;

    if !columns.iter().any(|c| c == "client_unique_id") {
        sqlx::query("ALTER TABLE hosts ADD COLUMN client_unique_id TEXT;")
            .execute(&pool)
            .await?;
    }
    if !columns.iter().any(|c| c == "client_private_key") {
        sqlx::query("ALTER TABLE hosts ADD COLUMN client_private_key TEXT;")
            .execute(&pool)
            .await?;
    }
    if !columns.iter().any(|c| c == "client_certificate") {
        sqlx::query("ALTER TABLE hosts ADD COLUMN client_certificate TEXT;")
            .execute(&pool)
            .await?;
    }
    if !columns.iter().any(|c| c == "server_certificate") {
        sqlx::query("ALTER TABLE hosts ADD COLUMN server_certificate TEXT;")
            .execute(&pool)
            .await?;
    }
    if !columns.iter().any(|c| c == "server_codec_mode_support") {
        sqlx::query("ALTER TABLE hosts ADD COLUMN server_codec_mode_support INTEGER DEFAULT 0;")
            .execute(&pool)
            .await?;
    }
    // SHA-256 (hex) of the agent's own key: only that agent may use the host id.
    if !columns.iter().any(|c| c == "agent_key_hash") {
        sqlx::query("ALTER TABLE hosts ADD COLUMN agent_key_hash TEXT;")
            .execute(&pool)
            .await?;
    }

    // Joined with an install token (install.rs): such a host signs in with
    // its key alone, and only while it is in this table with that key.
    if !columns.iter().any(|c| c == "enrolled") {
        sqlx::query("ALTER TABLE hosts ADD COLUMN enrolled INTEGER NOT NULL DEFAULT 0;")
            .execute(&pool)
            .await?;
    }
    // The computer's own addresses (JSON array), as its agent last said them.
    if !columns.iter().any(|c| c == "addresses") {
        sqlx::query("ALTER TABLE hosts ADD COLUMN addresses TEXT;")
            .execute(&pool)
            .await?;
    }
    // What its agent is (common::AgentBuild: version, OS, how it was
    // installed), as it last said; NULL for agents from before this.
    for (column, add) in [
        ("agent_version", "ALTER TABLE hosts ADD COLUMN agent_version TEXT;"),
        ("agent_os", "ALTER TABLE hosts ADD COLUMN agent_os TEXT;"),
        ("agent_install", "ALTER TABLE hosts ADD COLUMN agent_install TEXT;"),
        // Windows: whether its USB game mouse can be plugged in (common::GAME_MOUSE_*).
        ("agent_game_mouse", "ALTER TABLE hosts ADD COLUMN agent_game_mouse TEXT;"),
        // Whether it can be updated from the web (host_update.rs): 1, 0, or NULL (not said).
        ("agent_remote_update", "ALTER TABLE hosts ADD COLUMN agent_remote_update INTEGER;"),
    ] {
        if !columns.iter().any(|c| c == column) {
            sqlx::query(add).execute(&pool).await?;
        }
    }
    // A name an administrator gave the computer; NULL: the name its agent
    // announces (`name`) is shown.
    if !columns.iter().any(|c| c == "display_name") {
        sqlx::query("ALTER TABLE hosts ADD COLUMN display_name TEXT;")
            .execute(&pool)
            .await?;
    }
    if !columns.iter().any(|c| c == "install_token_id") {
        sqlx::query("ALTER TABLE hosts ADD COLUMN install_token_id TEXT;")
            .execute(&pool)
            .await?;
    }

    // Install tokens (install.rs): the one-line agent installer joins
    // computers with one of these, never with the server's agent token. Only
    // the token's SHA-256 (hex) is kept. Times as datetime('now') (UTC).
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS install_tokens (
            id TEXT PRIMARY KEY,
            token_hash TEXT UNIQUE NOT NULL,
            label TEXT NOT NULL DEFAULT '',
            created_by TEXT,
            created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
            expires_at TEXT NOT NULL,
            max_uses INTEGER,
            uses INTEGER NOT NULL DEFAULT 0,
            revoked INTEGER NOT NULL DEFAULT 0
        );",
    )
    .execute(&pool)
    .await?;
    // The token itself, kept for tokens made from 0.8.4 on so that an
    // administrator can be shown its command again (GET
    // .../install-tokens/{id}/command); NULL for older ones, which were kept
    // as a hash only.
    let token_columns: Vec<String> = sqlx::query_scalar("SELECT name FROM pragma_table_info('install_tokens')").fetch_all(&pool).await?;
    if !token_columns.iter().any(|c| c == "token_text") {
        sqlx::query("ALTER TABLE install_tokens ADD COLUMN token_text TEXT;").execute(&pool).await?;
    }


    // Perform migrations for users table if columns don't exist
    let user_columns: Vec<String> =
        sqlx::query_scalar("SELECT name FROM pragma_table_info('users')")
            .fetch_all(&pool)
            .await?;

    if !user_columns.iter().any(|c| c == "role") {
        sqlx::query("ALTER TABLE users ADD COLUMN role TEXT NOT NULL DEFAULT 'user';")
            .execute(&pool)
            .await?;
    }
    if !user_columns.iter().any(|c| c == "is_active") {
        sqlx::query("ALTER TABLE users ADD COLUMN is_active INTEGER NOT NULL DEFAULT 1;")
            .execute(&pool)
            .await?;
    }
    if !user_columns.iter().any(|c| c == "created_at") {
        sqlx::query("ALTER TABLE users ADD COLUMN created_at TEXT;")
            .execute(&pool)
            .await?;
    }
    // Sign-in tokens carry this number (auth.rs); raising it (new password,
    // two-factor turned off) ends every session of the user.
    if !user_columns.iter().any(|c| c == "token_version") {
        sqlx::query("ALTER TABLE users ADD COLUMN token_version INTEGER NOT NULL DEFAULT 0;")
            .execute(&pool)
            .await?;
    }

    // Two-factor sign-in (mfa.rs): the authenticator secret, encrypted with
    // the key in mfa_key.txt; enabled = 0 while it is being set up;
    // last_step = the time step of the last code accepted (each is used once).
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS user_mfa (
            user_id TEXT PRIMARY KEY REFERENCES users(id) ON DELETE CASCADE,
            secret TEXT NOT NULL,
            enabled INTEGER NOT NULL DEFAULT 0,
            last_step INTEGER,
            created_at TEXT,
            enabled_at TEXT
        );",
    )
    .execute(&pool)
    .await?;
    // One-time recovery codes, as HMAC-SHA-256 (keyed like the secret).
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS user_recovery_codes (
            user_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
            code_hash TEXT NOT NULL,
            created_at TEXT,
            used_at TEXT,
            PRIMARY KEY (user_id, code_hash)
        );",
    )
    .execute(&pool)
    .await?;

    // One-time tickets for lunaris://connect links (link_tickets.rs): the
    // desktop app trades one for a token that opens only `host_id` (or only
    // its source `source_id`). Only the ticket's SHA-256 (hex) is kept;
    // expires_at in Unix seconds.
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS link_tickets (
            ticket_hash TEXT PRIMARY KEY,
            user_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
            host_id TEXT NOT NULL,
            source_id TEXT,
            token_version INTEGER NOT NULL,
            expires_at INTEGER NOT NULL
        );",
    )
    .execute(&pool)
    .await?;

    // The first-start setup code (setup.rs): only its SHA-256 (hex), one row
    // while there is no account.
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS setup_code (
            id INTEGER PRIMARY KEY CHECK (id = 1),
            code_hash TEXT NOT NULL,
            created_at TEXT
        );",
    )
    .execute(&pool)
    .await?;

    // The server's own settings changed on its web page (key, value): the
    // server gateway's switch and its computer id (gateway.rs).
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS server_settings (
            key TEXT PRIMARY KEY,
            value TEXT NOT NULL
        );",
    )
    .execute(&pool)
    .await?;

    Ok(pool)
}

/// A setting from `server_settings`, None when it is not set.
pub async fn setting(db: &SqlitePool, key: &str) -> Result<Option<String>, sqlx::Error> {
    sqlx::query_scalar("SELECT value FROM server_settings WHERE key = ?").bind(key).fetch_optional(db).await
}

pub async fn set_setting(db: &SqlitePool, key: &str, value: &str) -> Result<(), sqlx::Error> {
    sqlx::query("INSERT INTO server_settings (key, value) VALUES (?, ?) ON CONFLICT(key) DO UPDATE SET value = excluded.value")
        .bind(key)
        .bind(value)
        .execute(db)
        .await
        .map(|_| ())
}

/// The database holds password hashes and keys: only the owner may read it.
/// Creates the file with mode 0600 (SQLite gives its journal files the same
/// mode) and tightens an existing one.
fn make_private(path: &std::path::Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
        if path.exists() {
            if let Err(e) = fs::set_permissions(path, fs::Permissions::from_mode(0o600)) {
                tracing::warn!("Cannot restrict permissions of {:?}: {}", path, e);
            }
        } else if let Err(e) = fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(path) {
            tracing::warn!("Cannot create {:?}: {}", path, e);
        }
    }
    #[cfg(not(unix))]
    let _ = path;
}

#[cfg(all(test, unix))]
mod tests {
    /// A database from the previous release (address_hosts without
    /// cert_sha256, with a row) opens: the column is added, nothing is
    /// lost, and opening it again changes nothing.
    #[tokio::test]
    async fn an_existing_database_gains_the_certificate_pin_column() {
        let dir = std::env::temp_dir().join(format!("lunaris-db-old-{}", crate::security::random_hex(8)));
        std::fs::create_dir_all(&dir).unwrap();
        let url = format!("sqlite://{}", dir.join("lunaris.db").display());
        let pool = super::init_db(&url).await.unwrap();
        // Back to the old shape: the table as the previous release made it.
        for sql in [
            "DROP TABLE address_hosts",
            "CREATE TABLE address_hosts (address TEXT PRIMARY KEY, password TEXT NOT NULL DEFAULT '', host_id TEXT, created_at TEXT DEFAULT CURRENT_TIMESTAMP, name TEXT NOT NULL DEFAULT '')",
            "INSERT INTO address_hosts (address, password, host_id, name) VALUES ('pc.lan:8091', 'pw', 'h-old', 'Old PC')",
            "INSERT INTO users (id, username, password_hash, role, is_active, token_version) VALUES ('u1', 'short', 'hash', 'admin', 1, 3)",
        ] {
            sqlx::query(sql).execute(&pool).await.unwrap();
        }
        pool.close().await;
        for _ in 0..2 {
            let pool = super::init_db(&url).await.unwrap();
            let row: (String, String, Option<String>, String, Option<String>) =
                sqlx::query_as("SELECT address, password, host_id, name, cert_sha256 FROM address_hosts")
                    .fetch_one(&pool)
                    .await
                    .unwrap();
            assert_eq!(row, ("pc.lan:8091".into(), "pw".into(), Some("h-old".into()), "Old PC".into(), None));
            let pake: Option<i64> = sqlx::query_scalar("SELECT pake_version FROM address_hosts").fetch_one(&pool).await.unwrap();
            assert_eq!(pake, None);
            let user: (String, i64) = sqlx::query_as("SELECT password_hash, token_version FROM users WHERE id = 'u1'")
                .fetch_one(&pool)
                .await
                .unwrap();
            assert_eq!(user, ("hash".into(), 3));
            pool.close().await;
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn database_file_is_private() {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("lunaris-db-{}", crate::security::random_hex(8)));
        let db = dir.join("lunaris.db");
        let pool = super::init_db(&format!("sqlite://{}", db.display())).await.unwrap();
        let cols: Vec<String> = sqlx::query_scalar("SELECT name FROM pragma_table_info('hosts')")
            .fetch_all(&pool)
            .await
            .unwrap();
        assert!(cols.iter().any(|c| c == "agent_key_hash"));
        assert_eq!(std::fs::metadata(&db).unwrap().permissions().mode() & 0o777, 0o600);
        // No account and no password file: the first admin comes from setup.rs.
        let users: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM users").fetch_one(&pool).await.unwrap();
        assert_eq!(users, 0);
        assert!(!dir.join("initial_admin_password.txt").exists());
        pool.close().await;
        let _ = std::fs::remove_dir_all(&dir);
    }
}

/// The name a host is shown with, in SQL over `hosts` aliased `h`: the one
/// an administrator gave it, else the one its agent announces.
/// A macro, so that it can be part of a `concat!` (sqlx takes only static SQL).
macro_rules! host_name_sql {
    () => {
        "COALESCE(NULLIF(h.display_name, ''), h.name)"
    };
}
pub(crate) use host_name_sql;
