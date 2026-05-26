use sqlx::{sqlite::{SqliteConnectOptions, SqlitePoolOptions}, SqlitePool};
use std::fs;
use std::str::FromStr;

pub async fn init_db(database_url: &str) -> Result<SqlitePool, sqlx::Error> {
    // If it's a file database, ensure parent directories exist
    if database_url.starts_with("sqlite://") {
        let path = database_url.trim_start_matches("sqlite://");
        if path != ":memory:" {
            if let Some(parent) = std::path::Path::new(path).parent() {
                if !parent.as_os_str().is_empty() {
                    fs::create_dir_all(parent).unwrap_or_default();
                }
            }
        }
    }

    let options = SqliteConnectOptions::from_str(database_url)?
        .create_if_missing(true);

    let pool = SqlitePoolOptions::new()
        .max_connections(5)
        .connect_with(options)
        .await?;

    // Enable foreign keys
    sqlx::query("PRAGMA foreign_keys = ON;").execute(&pool).await?;

    // Create tables
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS users (
            id TEXT PRIMARY KEY,
            username TEXT UNIQUE NOT NULL,
            password_hash TEXT NOT NULL
        );"
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
        );"
    )
    .execute(&pool)
    .await?;

    sqlx::query(
        "CREATE TABLE IF NOT EXISTS groups (
            id TEXT PRIMARY KEY,
            name TEXT UNIQUE NOT NULL,
            note TEXT DEFAULT '',
            created_at TEXT DEFAULT CURRENT_TIMESTAMP
        );"
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
        );"
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
        );"
    )
    .execute(&pool)
    .await?;

    // Perform migrations for hosts table if columns don't exist
    let columns: Vec<String> = sqlx::query_scalar("SELECT name FROM pragma_table_info('hosts')")
        .fetch_all(&pool)
        .await?;

    if !columns.iter().any(|c| c == "client_unique_id") {
        sqlx::query("ALTER TABLE hosts ADD COLUMN client_unique_id TEXT;").execute(&pool).await?;
    }
    if !columns.iter().any(|c| c == "client_private_key") {
        sqlx::query("ALTER TABLE hosts ADD COLUMN client_private_key TEXT;").execute(&pool).await?;
    }
    if !columns.iter().any(|c| c == "client_certificate") {
        sqlx::query("ALTER TABLE hosts ADD COLUMN client_certificate TEXT;").execute(&pool).await?;
    }
    if !columns.iter().any(|c| c == "server_certificate") {
        sqlx::query("ALTER TABLE hosts ADD COLUMN server_certificate TEXT;").execute(&pool).await?;
    }
    if !columns.iter().any(|c| c == "server_codec_mode_support") {
        sqlx::query("ALTER TABLE hosts ADD COLUMN server_codec_mode_support INTEGER DEFAULT 0;").execute(&pool).await?;
    }

    // Perform migrations for users table if columns don't exist
    let user_columns: Vec<String> = sqlx::query_scalar("SELECT name FROM pragma_table_info('users')")
        .fetch_all(&pool)
        .await?;

    if !user_columns.iter().any(|c| c == "role") {
        sqlx::query("ALTER TABLE users ADD COLUMN role TEXT NOT NULL DEFAULT 'user';").execute(&pool).await?;
    }
    if !user_columns.iter().any(|c| c == "is_active") {
        sqlx::query("ALTER TABLE users ADD COLUMN is_active INTEGER NOT NULL DEFAULT 1;").execute(&pool).await?;
    }
    if !user_columns.iter().any(|c| c == "created_at") {
        sqlx::query("ALTER TABLE users ADD COLUMN created_at TEXT;").execute(&pool).await?;
    }

    // Bootstrap default admin user if no users exist
    let user_count: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM users")
        .fetch_one(&pool)
        .await?;
    if user_count.0 == 0 {
        use crate::auth::hash_password;
        let admin_id = uuid::Uuid::new_v4().to_string();
        let hashed = hash_password("admin123").map_err(|e| sqlx::Error::Protocol(format!("Hash error: {}", e)))?;
        sqlx::query("INSERT INTO users (id, username, password_hash, role, is_active, created_at) VALUES (?, 'admin', ?, 'admin', 1, datetime('now'))")
            .bind(&admin_id)
            .bind(&hashed)
            .execute(&pool)
            .await?;
        tracing::info!("Created default admin user (username: admin, password: admin123)");
    }

    Ok(pool)
}
