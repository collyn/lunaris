use axum::{
    extract::State,
    http::StatusCode,
    response::IntoResponse,
    routing::{get, post, delete},
    Json, Router,
};
use common::{AuthResponse, HostInfo, HostStatus, LoginRequest, RegisterRequest, PairHostRequest};
use std::sync::Arc;
use tower_http::cors::{Any, CorsLayer};
use tracing::{error, info};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};
use uuid::Uuid;

mod auth;
mod db;
pub mod signaling;
pub mod pairing;
pub mod buffer;
pub mod input;
pub mod video;
pub mod bridge;

use crate::{
    auth::{create_jwt, hash_password, verify_password, AuthenticatedUser},
    db::init_db,
    signaling::{agent_ws_handler, client_ws_handler, SignalingState},
};


#[cfg(all(target_os = "macos", target_arch = "x86_64"))]
#[no_mangle]
#[allow(non_upper_case_globals)]
pub static mut __cpu_model: [u32; 4] = [0; 4];

#[cfg(all(target_os = "macos", target_arch = "x86_64"))]
#[no_mangle]
pub extern "C" fn __cpu_indicator_init() -> i32 {
    0
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Init logger
    tracing_subscriber::registry()
        .with(tracing_subscriber::EnvFilter::new(
            std::env::var("RUST_LOG").unwrap_or_else(|_| "info,server=debug".into()),
        ))
        .with(tracing_subscriber::fmt::layer())
        .init();

    let database_url = std::env::var("DATABASE_URL")
        .unwrap_or_else(|_| "sqlite://lunaris.db".to_string());

    info!("Initializing SQLite database: {}", database_url);
    let pool = init_db(&database_url).await?;

    let signaling_state = Arc::new(SignalingState::new(pool.clone()));

    // Configure CORS
    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_methods(Any)
        .allow_headers(Any);

    // Build routes
    let app = Router::new()
        .route("/api/auth/register", post(register_handler))
        .route("/api/auth/login", post(login_handler))
        .route("/api/hosts", get(hosts_handler))
        .route("/api/hosts/pair", post(pair_host_handler))
        .route("/api/hosts/:id", delete(unpair_host_handler))
        .route("/ws/agent", get(agent_ws_handler))
        .route("/ws/client", get(client_ws_handler))
        .layer(cors)
        .with_state(signaling_state);

    let port = std::env::var("SERVER_PORT").unwrap_or_else(|_| "8080".to_string());
    let addr = format!("0.0.0.0:{}", port);
    let listener = tokio::net::TcpListener::bind(&addr).await?;

    info!("Lunaris Server running on http://{}", addr);
    axum::serve(listener, app).await?;

    Ok(())
}

async fn register_handler(
    State(state): State<Arc<SignalingState>>,
    Json(payload): Json<RegisterRequest>,
) -> Result<impl IntoResponse, (StatusCode, Json<serde_json::Value>)> {
    if payload.username.trim().is_empty() || payload.password.len() < 6 {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": "Invalid username or password must be at least 6 characters" })),
        ));
    }

    // Check if user already exists
    let existing: Option<String> = sqlx::query_scalar("SELECT id FROM users WHERE username = ?")
        .bind(&payload.username)
        .fetch_optional(&state.db)
        .await
        .map_err(|e| {
            error!("Database error: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({ "error": "Internal Database Error" })),
            )
        })?;

    if existing.is_some() {
        return Err((
            StatusCode::CONFLICT,
            Json(serde_json::json!({ "error": "Username already taken" })),
        ));
    }

    let hashed = hash_password(&payload.password).map_err(|e| {
        error!("Hashing error: {}", e);
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": "Internal Hashing Error" })),
        )
    })?;

    let user_id = Uuid::new_v4().to_string();

    sqlx::query("INSERT INTO users (id, username, password_hash) VALUES (?, ?, ?)")
        .bind(&user_id)
        .bind(&payload.username)
        .bind(&hashed)
        .execute(&state.db)
        .await
        .map_err(|e| {
            error!("Database error: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({ "error": "Failed to create user" })),
            )
        })?;

    let token = create_jwt(&user_id, &payload.username).map_err(|e| {
        error!("Token error: {}", e);
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": "Failed to generate token" })),
        )
    })?;

    Ok((
        StatusCode::CREATED,
        Json(AuthResponse {
            token,
            username: payload.username,
        }),
    ))
}

async fn login_handler(
    State(state): State<Arc<SignalingState>>,
    Json(payload): Json<LoginRequest>,
) -> Result<impl IntoResponse, (StatusCode, Json<serde_json::Value>)> {
    let row: Option<(String, String)> =
        sqlx::query_as("SELECT id, password_hash FROM users WHERE username = ?")
            .bind(&payload.username)
            .fetch_optional(&state.db)
            .await
            .map_err(|e| {
                error!("Database error: {}", e);
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(serde_json::json!({ "error": "Internal Database Error" })),
                )
            })?;

    let (user_id, hash) = match row {
        Some((id, hash)) => (id, hash),
        None => {
            return Err((
                StatusCode::UNAUTHORIZED,
                Json(serde_json::json!({ "error": "Invalid username or password" })),
            ));
        }
    };

    if !verify_password(&payload.password, &hash) {
        return Err((
            StatusCode::UNAUTHORIZED,
            Json(serde_json::json!({ "error": "Invalid username or password" })),
        ));
    }

    let token = create_jwt(&user_id, &payload.username).map_err(|e| {
        error!("Token error: {}", e);
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": "Failed to generate token" })),
        )
    })?;

    Ok((
        StatusCode::OK,
        Json(AuthResponse {
            token,
            username: payload.username,
        }),
    ))
}

async fn check_host_online(ip: &str, port: u16) -> bool {
    let addr_str = format!("{}:{}", ip, port);
    if let Ok(socket_addr) = addr_str.parse::<std::net::SocketAddr>() {
        match tokio::time::timeout(std::time::Duration::from_millis(500), tokio::net::TcpStream::connect(&socket_addr)).await {
            Ok(Ok(_)) => true,
            _ => false,
        }
    } else {
        false
    }
}

async fn hosts_handler(
    _user: AuthenticatedUser,
    State(state): State<Arc<SignalingState>>,
) -> Result<impl IntoResponse, (StatusCode, Json<serde_json::Value>)> {
    let rows: Vec<(String, String, String, Option<String>, Option<i64>)> =
        sqlx::query_as("SELECT id, name, status, ip_address, server_codec_mode_support FROM hosts")
            .fetch_all(&state.db)
            .await
            .map_err(|e| {
                error!("Database error: {}", e);
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(serde_json::json!({ "error": "Internal Database Error" })),
                )
            })?;

    let active_host_ids: std::collections::HashSet<String> = {
        let sessions = state.local_sessions.read().unwrap();
        sessions.keys().cloned().collect()
    };

    let mut futures = Vec::new();

    for (id, name, status_str, ip_address, server_codec_mode_support) in rows {
        let ip_address_clone = ip_address.clone();
        let db_pool = state.db.clone();
        let is_currently_busy = active_host_ids.contains(&id);
        let state_clone = state.clone();
        
        let fut = async move {
            let is_agent_online = state_clone.agents.read().unwrap().contains_key(&id);
            let status = if is_currently_busy {
                HostStatus::Busy
            } else if is_agent_online {
                HostStatus::Online
            } else {
                if let Some(ref ip) = ip_address_clone {
                    if check_host_online(ip, 47989).await {
                        HostStatus::Online
                    } else {
                        HostStatus::Offline
                    }
                } else {
                    HostStatus::Offline
                }
            };

            let status_db_str = match status {
                HostStatus::Online => "Online",
                HostStatus::Offline => "Offline",
                HostStatus::Busy => "Busy",
            };
            
            if status_db_str != status_str {
                let _ = sqlx::query("UPDATE hosts SET status = ? WHERE id = ?")
                    .bind(status_db_str)
                    .bind(&id)
                    .execute(&db_pool)
                    .await;
            }

            HostInfo {
                id,
                name,
                status,
                ip_address: ip_address_clone,
                server_codec_mode_support: server_codec_mode_support.map(|v| v as u32),
            }
        };
        futures.push(fut);
    }

    let hosts: Vec<HostInfo> = futures_util::future::join_all(futures).await;

    Ok((StatusCode::OK, Json(hosts)))
}

async fn pair_host_handler(
    user: AuthenticatedUser,
    State(state): State<Arc<SignalingState>>,
    Json(payload): Json<PairHostRequest>,
) -> Result<impl IntoResponse, (StatusCode, Json<serde_json::Value>)> {
    info!("Pairing host: {} at {}", payload.name, payload.ip_address);
    
    let username = match payload.sunshine_username.as_deref() {
        Some(u) if !u.trim().is_empty() => u,
        _ => {
            return Err((
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({ "error": "Sunshine Web UI username is required" })),
            ));
        }
    };
    
    let password = match payload.sunshine_password.as_deref() {
        Some(p) if !p.is_empty() => p,
        _ => {
            return Err((
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({ "error": "Sunshine Web UI password is required" })),
            ));
        }
    };

    // 1. Perform pairing handshake using Moonlight client implementation
    let config = crate::pairing::perform_pairing(
        &payload.ip_address,
        47989, // Sunshine default port
        username,
        password,
        &payload.name,
    )
    .await
    .map_err(|e| {
        error!("Pairing failed: {}", e);
        (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": format!("Pairing failed: {}", e) })),
        )
    })?;

    // 2. Insert host into DB
    sqlx::query(
        "INSERT INTO hosts (id, name, status, ip_address, owner_id, client_unique_id, client_private_key, client_certificate, server_certificate, server_codec_mode_support)
         VALUES (?, ?, 'Online', ?, ?, ?, ?, ?, ?, ?)
         ON CONFLICT(id) DO UPDATE SET
            name = excluded.name,
            ip_address = excluded.ip_address,
            owner_id = excluded.owner_id,
            client_unique_id = excluded.client_unique_id,
            client_private_key = excluded.client_private_key,
            client_certificate = excluded.client_certificate,
            server_certificate = excluded.server_certificate,
            server_codec_mode_support = excluded.server_codec_mode_support,
            status = 'Online';"
    )
    .bind(&config.client_unique_id)
    .bind(&payload.name)
    .bind(&payload.ip_address)
    .bind(&user.user_id)
    .bind(&config.client_unique_id)
    .bind(&config.client_private_key)
    .bind(&config.client_certificate)
    .bind(&config.server_certificate)
    .bind(config.server_codec_mode_support as i64)
    .execute(&state.db)
    .await
    .map_err(|e| {
        error!("Database error: {}", e);
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": "Failed to save host to database" })),
        )
    })?;

    Ok((StatusCode::CREATED, Json(serde_json::json!({ "id": config.client_unique_id }))))
}

async fn unpair_host_handler(
    host_id_path: axum::extract::Path<String>,
    State(state): State<Arc<SignalingState>>,
) -> impl IntoResponse {
    let host_id = host_id_path.0;
    info!("Unpairing host: {}", host_id);
    
    // Check if there are active local sessions and end them
    let session = {
        let mut sessions = state.local_sessions.write().unwrap();
        sessions.remove(&host_id)
    };
    if let Some(session) = session {
        let _ = session.peer_connection.close().await;
    }

    if let Err(e) = sqlx::query("DELETE FROM hosts WHERE id = ?")
        .bind(&host_id)
        .execute(&state.db)
        .await
    {
        error!("Database error: {}", e);
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": "Failed to delete host from database" })),
        ).into_response();
    }

    StatusCode::OK.into_response()
}

