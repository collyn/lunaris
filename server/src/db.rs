use sqlx::{sqlite::SqlitePoolOptions, SqlitePool};
use std::fs;

pub async fn init_db(database_url: &str) -> Result<SqlitePool, sqlx::Error> {
    // If it's a file database, ensure parent directories exist
    if database_url.starts_with("sqlite://") {
        let path = database_url.trim_start_matches("sqlite://");
        if path != ":memory:" {
            if let Some(parent) = std::path::Path::new(path).parent() {
                fs::create_dir_all(parent).unwrap_or_default();
            }
        }
    }

    let pool = SqlitePoolOptions::new()
        .max_connections(5)
        .connect(database_url)
        .await?;

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

    // Perform migrations if columns don't exist in existing database
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

    Ok(pool)
}

