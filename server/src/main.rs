use axum::{
    extract::State,
    http::StatusCode,
    response::IntoResponse,
    routing::{get, post, delete},
    Json, Router,
};
use common::{AuthResponse, HostInfo, HostStatus, LoginRequest, PairHostRequest};
use std::sync::Arc;
use tower_http::cors::{Any, CorsLayer};
use tower_http::services::{ServeDir, ServeFile};
use tracing::{error, info};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};


mod admin;
mod auth;
mod db;
pub mod signaling;
pub mod pairing;
pub mod buffer;
pub mod input;
pub mod video;
pub mod bridge;

use crate::{
    auth::{create_jwt, verify_password, AuthenticatedUser},
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

fn find_dist_path(dir_name: &str) -> Option<std::path::PathBuf> {
    // Candidate 1: Current Working Directory
    let cwd_path = std::path::PathBuf::from(dir_name);
    if cwd_path.exists() {
        return Some(cwd_path);
    }

    // Candidate 2: Compile-time manifest directory parent (workspace root on dev machine)
    let manifest_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    if let Some(workspace_dir) = manifest_dir.parent() {
        let path = workspace_dir.join(dir_name);
        if path.exists() {
            return Some(path);
        }
    }

    // Candidate 3: Relative to Executable Path
    if let Ok(exe_path) = std::env::current_exe() {
        if let Some(exe_dir) = exe_path.parent() {
            let path = exe_dir.join(dir_name);
            if path.exists() {
                return Some(path);
            }
            
            // If executable is in target/release/ or target/debug/
            if let Some(target_dir) = exe_dir.parent() {
                if let Some(workspace_dir) = target_dir.parent() {
                    let path = workspace_dir.join(dir_name);
                    if path.exists() {
                        return Some(path);
                    }
                }
            }
        }
    }

    None
}

fn get_or_generate_agent_token() -> String {
    if let Ok(token) = std::env::var("LUNARIS_TOKEN") {
        if !token.trim().is_empty() {
            return token.trim().to_string();
        }
    }

    let token_file = std::path::Path::new("server_token.txt");
    if token_file.exists() {
        if let Ok(token) = std::fs::read_to_string(token_file) {
            let trimmed = token.trim();
            if !trimmed.is_empty() {
                return trimmed.to_string();
            }
        }
    }

    // Generate a random token based on UUID
    let generated = uuid::Uuid::new_v4().to_string().replace("-", "")[..16].to_string();

    if let Err(e) = std::fs::write(token_file, &generated) {
        error!("Failed to write server_token.txt: {:?}", e);
    }

    generated
}

fn find_database_url() -> String {
    if let Ok(url) = std::env::var("DATABASE_URL") {
        return url;
    }

    // Default to "sqlite://lunaris.db"
    // If started from target/release or target/debug, walk up to use root workspace directory
    if let Ok(exe_path) = std::env::current_exe() {
        if let Some(exe_dir) = exe_path.parent() {
            if let Some(target_dir) = exe_dir.parent() {
                if let Some(workspace_dir) = target_dir.parent() {
                    let path = workspace_dir.join("lunaris.db");
                    if workspace_dir.join("Cargo.toml").exists() {
                        return format!("sqlite://{}", path.to_string_lossy());
                    }
                }
            }
        }
    }

    "sqlite://lunaris.db".to_string()
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

    let database_url = find_database_url();

    info!("Initializing SQLite database: {}", database_url);
    let pool = init_db(&database_url).await?;

    let agent_token = get_or_generate_agent_token();
    info!("==================================================");
    info!(" [Security] Agent Connection Token: {}", agent_token);
    info!("==================================================");

    let signaling_state = Arc::new(SignalingState::new(pool.clone(), agent_token));

    // Configure CORS
    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_methods(Any)
        .allow_headers(Any);

    // Find React build directory
    let react_path = find_dist_path("web/dist");

    // Build routes
    let mut app = Router::new()
        .route("/api/auth/login", post(login_handler))
        .route("/api/auth/me", get(admin::get_current_user))
        .route("/api/hosts", get(hosts_handler))
        .route("/api/hosts/pair", post(pair_host_handler))
        .route("/api/hosts/:id", delete(unpair_host_handler))
        .route("/api/agent/token", get(agent_token_handler))
        .route("/api/admin/users", get(admin::list_users).post(admin::create_user))
        .route("/api/admin/users/:id", axum::routing::put(admin::update_user).delete(admin::delete_user))
        .route("/api/admin/groups", get(admin::list_groups).post(admin::create_group))
        .route("/api/admin/groups/:id", axum::routing::put(admin::update_group).delete(admin::delete_group))
        .route("/api/admin/turn-servers", get(admin::list_turn_servers).post(admin::create_turn_server))
        .route("/api/admin/turn-servers/:id", delete(admin::delete_turn_server))
        .route("/ws/agent", get(agent_ws_handler))
        .route("/ws/client", get(client_ws_handler));

    if let Some(ref path) = react_path {
        info!("Serving React client from {:?}", path);
        let serve_react = ServeDir::new(path)
            .not_found_service(ServeFile::new(path.join("index.html")));
        app = app.fallback_service(serve_react);
    } else {
        error!("Directory 'web/dist' not found! Build the React client via 'npm run build' inside the 'web' directory.");
    }

    let app = app.layer(cors)
        .with_state(signaling_state);

    let port = std::env::var("SERVER_PORT").unwrap_or_else(|_| "8080".to_string());
    let addr = format!("0.0.0.0:{}", port);
    let listener = tokio::net::TcpListener::bind(&addr).await?;

    info!("Lunaris Server running on http://{}", addr);
    axum::serve(listener, app).await?;

    Ok(())
}

async fn login_handler(
    State(state): State<Arc<SignalingState>>,
    Json(payload): Json<LoginRequest>,
) -> Result<impl IntoResponse, (StatusCode, Json<serde_json::Value>)> {
    let row: Option<(String, String, String)> =
        sqlx::query_as("SELECT id, password_hash, COALESCE(role, 'user') as role FROM users WHERE username = ?")
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

    let (user_id, hash, role) = match row {
        Some((id, hash, role)) => (id, hash, role),
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

    let token = create_jwt(&user_id, &payload.username, &role).map_err(|e| {
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
            role,
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
    user: AuthenticatedUser,
    State(state): State<Arc<SignalingState>>,
) -> Result<impl IntoResponse, (StatusCode, Json<serde_json::Value>)> {
    let rows: Vec<(String, String, String, Option<String>, Option<i64>)> = if user.role == "admin" {
        // Admin sees all hosts
        sqlx::query_as("SELECT id, name, status, ip_address, server_codec_mode_support FROM hosts")
            .fetch_all(&state.db)
            .await
            .map_err(|e| {
                error!("Database error: {}", e);
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(serde_json::json!({ "error": "Internal Database Error" })),
                )
            })?
    } else {
        // Regular user only sees hosts in their groups
        sqlx::query_as(
            "SELECT DISTINCT h.id, h.name, h.status, h.ip_address, h.server_codec_mode_support 
             FROM hosts h
             INNER JOIN host_groups hg ON h.id = hg.host_id
             INNER JOIN user_groups ug ON hg.group_id = ug.group_id
             WHERE ug.user_id = ?"
        )
        .bind(&user.user_id)
        .fetch_all(&state.db)
        .await
        .map_err(|e| {
            error!("Database error: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({ "error": "Internal Database Error" })),
            )
        })?
    };

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
                agent_connected: is_agent_online,
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

async fn agent_token_handler(
    _user: crate::auth::AuthenticatedUser,
    State(state): State<Arc<SignalingState>>,
) -> impl IntoResponse {
    Json(serde_json::json!({ "token": state.agent_token }))
}

