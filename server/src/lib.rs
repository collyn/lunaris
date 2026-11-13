use axum::{
    extract::State,
    http::StatusCode,
    response::IntoResponse,
    routing::{delete, get, post},
    Json, Router,
};
use common::{AuthResponse, HostInfo, HostStatus, LoginRequest};
use std::sync::Arc;
use tower_http::cors::{Any, CorsLayer};
use tower_http::services::{ServeDir, ServeFile};
use tracing::{error, info};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

mod admin;
mod auth;
pub mod bridge;
pub mod buffer;
mod db;
pub mod input;
mod links;
mod paths;
mod service;
mod tls;
mod turn;
mod web;

pub mod signaling;
pub mod video;

use crate::{
    auth::{create_jwt, verify_password, AuthenticatedUser},
    db::init_db,
    signaling::{agent_ws_handler, client_ws_handler, SignalingState},
};

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

    let token_file = paths::data_file("server_token.txt");
    let token_file = token_file.as_path();
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

    if let Err(e) = paths::write_private(token_file, &generated) {
        error!("Failed to write server_token.txt: {:?}", e);
    }

    generated
}

fn find_database_url() -> String {
    if let Ok(url) = std::env::var("DATABASE_URL") {
        return url;
    }
    if let Some(dir) = paths::data_dir() {
        return format!("sqlite://{}", dir.join("lunaris.db").to_string_lossy());
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

const USAGE: &str = "\
Lunaris server — accounts, host directory, signaling and the web client.

Usage: lunaris-server [command]

Commands:
  run                         Run in the foreground (default)
  install [--port <port>]     Install and start as a system service
                              (systemd on Linux, Windows service on Windows)
  uninstall [--purge]         Remove the service (--purge also deletes data)
  status                      Show the service status
  help, version

Environment: SERVER_PORT (default 8090), LUNARIS_HTTPS_PORT (default 8443,
off to disable), LUNARIS_TLS_CERT + LUNARIS_TLS_KEY (PEM; default: a
self-signed certificate kept in the data directory), LUNARIS_DATA_DIR (database, token,
initial admin password; default: working directory), LUNARIS_TOKEN,
LUNARIS_ADMIN_PASSWORD (first start only), DATABASE_URL, RUST_LOG.
";

/// Runs the server command line (without the program name), as the `server`
/// binary and `lunaris server` do.
pub fn run(args: Vec<String>) -> std::process::ExitCode {
    let rest = args.get(1..).unwrap_or(&[]);
    let result: Result<(), String> = match args.first().map(String::as_str) {
        None | Some("run") => run_foreground(),
        Some("install") => service::install(rest),
        Some("uninstall") => service::uninstall(rest),
        Some("status") => service::status(),
        #[cfg(windows)]
        Some("service") => service::run_dispatcher(),
        Some("help" | "-h" | "--help") => {
            print!("{USAGE}");
            Ok(())
        }
        Some("version" | "-V" | "--version") => {
            println!("lunaris-server {}", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
        Some(other) => Err(format!("unknown command: {other}\n\n{USAGE}")),
    };
    match result {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e}");
            std::process::ExitCode::FAILURE
        }
    }
}

/// Sets up logging to stderr, or to `log_file` (the Windows service has no console).
pub(crate) fn init_logging(log_file: Option<&std::path::Path>) {
    let filter = tracing_subscriber::EnvFilter::new(
        std::env::var("RUST_LOG").unwrap_or_else(|_| "info,server=debug,webrtc_sctp=off,rtc_sctp=off".into()),
    );
    let file = log_file.and_then(|p| {
        std::fs::OpenOptions::new().create(true).append(true).open(p).ok()
    });
    match file {
        Some(f) => tracing_subscriber::registry()
            .with(filter)
            .with(
                tracing_subscriber::fmt::layer()
                    .with_ansi(false)
                    .with_writer(std::sync::Mutex::new(f)),
            )
            .init(),
        None => tracing_subscriber::registry()
            .with(filter)
            .with(tracing_subscriber::fmt::layer())
            .init(),
    }
}

fn run_foreground() -> Result<(), String> {
    init_logging(None);
    let rt = tokio::runtime::Runtime::new().map_err(|e| e.to_string())?;
    rt.block_on(run_server(shutdown_signal()))
        .map_err(|e| e.to_string())
}

/// Ctrl+C, or SIGTERM from systemd / docker stop.
async fn shutdown_signal() {
    #[cfg(unix)]
    {
        let mut term =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
                .expect("install SIGTERM handler");
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {}
            _ = term.recv() => {}
        }
    }
    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
    }
    info!("Shutting down");
}

/// Ports the server listens on: agents use ws:// on `http`, browsers may
/// prefer `https` (controllers and the clipboard need a secure page).
async fn ports_handler() -> axum::Json<serde_json::Value> {
    let http: u16 = std::env::var("SERVER_PORT").ok().and_then(|p| p.parse().ok()).unwrap_or(8090);
    axum::Json(serde_json::json!({ "http": http, "https": tls::https_port() }))
}

/// Runs the server until `shutdown` completes.
pub(crate) async fn run_server(
    shutdown: impl std::future::Future<Output = ()> + Send + 'static,
) -> Result<(), Box<dyn std::error::Error>> {
    let database_url = find_database_url();

    info!("Initializing SQLite database: {}", database_url);
    let pool = init_db(&database_url, &paths::data_file("initial_admin_password.txt")).await?;

    let agent_token = get_or_generate_agent_token();
    info!("==================================================");
    info!(" [Security] Agent Connection Token: {}", agent_token);
    info!("==================================================");

    let turn = turn::TurnService::start_from_env().await;
    let signaling_state = Arc::new(SignalingState::new(pool.clone(), agent_token, turn));
    links::start_saved(&signaling_state).await;

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
        .route("/api/hosts/:id", delete(delete_host_handler))
        .route("/api/hosts/address", post(add_host_by_address_handler))
        .route("/api/agent/token", get(agent_token_handler))
        .route("/api/server/ports", get(ports_handler))
        .route(
            "/api/admin/users",
            get(admin::list_users).post(admin::create_user),
        )
        .route(
            "/api/admin/users/:id",
            axum::routing::put(admin::update_user).delete(admin::delete_user),
        )
        .route(
            "/api/admin/groups",
            get(admin::list_groups).post(admin::create_group),
        )
        .route(
            "/api/admin/groups/:id",
            axum::routing::put(admin::update_group).delete(admin::delete_group),
        )
        .route(
            "/api/admin/turn-servers",
            get(admin::list_turn_servers).post(admin::create_turn_server),
        )
        .route(
            "/api/admin/turn-servers/:id",
            delete(admin::delete_turn_server),
        )
        .route("/ws/agent", get(agent_ws_handler))
        .route("/ws/client", get(client_ws_handler));

    if let Some(ref path) = react_path {
        info!("Serving React client from {:?}", path);
        let serve_react =
            ServeDir::new(path).not_found_service(ServeFile::new(path.join("index.html")));
        // Revalidate (Last-Modified → 304): without it browsers guess a cache
        // lifetime and keep showing an old index.html after an update.
        let serve_react = Router::new().fallback_service(serve_react).layer(
            tower_http::set_header::SetResponseHeaderLayer::if_not_present(
                axum::http::header::CACHE_CONTROL,
                axum::http::HeaderValue::from_static("no-cache"),
            ),
        );
        app = app.fallback_service(serve_react);
    } else if web::is_embedded() {
        info!("Serving the embedded React client");
        app = app.fallback(web::serve);
    } else {
        error!("No web client: 'web/dist' not found and none embedded. Build it with 'npm run build' in 'web/'.");
    }

    let app = app.layer(cors).with_state(signaling_state);

    let port = std::env::var("SERVER_PORT").unwrap_or_else(|_| "8090".to_string());
    let addr = format!("0.0.0.0:{}", port);
    let listener = tokio::net::TcpListener::bind(&addr).await?;

    // HTTPS next to HTTP (tls.rs); stopped with the HTTP server.
    let https = tls::https_port().map(|p| tokio::spawn(tls::serve(app.clone(), p)));

    info!("Lunaris Server running on http://{}", addr);
    let served = axum::serve(
        listener,
        app.into_make_service_with_connect_info::<signaling::ConnAddrs>(),
    )
    .with_graceful_shutdown(shutdown)
    .await;
    if let Some(task) = https {
        task.abort();
    }
    served?;

    Ok(())
}

async fn login_handler(
    State(state): State<Arc<SignalingState>>,
    Json(payload): Json<LoginRequest>,
) -> Result<impl IntoResponse, (StatusCode, Json<serde_json::Value>)> {
    let row: Option<(String, String, String)> = sqlx::query_as(
        "SELECT id, password_hash, COALESCE(role, 'user') as role FROM users WHERE username = ?",
    )
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
        match tokio::time::timeout(
            std::time::Duration::from_millis(500),
            tokio::net::TcpStream::connect(&socket_addr),
        )
        .await
        {
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
             WHERE ug.user_id = ?",
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
                address: None,
                link_error: None,
                needs_approval: false,
            }
        };
        futures.push(fut);
    }

    let mut hosts: Vec<HostInfo> = futures_util::future::join_all(futures).await;

    // Computers added by address: where they are, and why they can't be
    // reached. Admins also see the ones that never answered yet.
    for link in state.links.list() {
        match hosts.iter_mut().find(|h| link.host_id.as_deref() == Some(h.id.as_str())) {
            Some(h) => {
                h.address = Some(link.address.clone());
                h.link_error = link.error.clone();
                h.needs_approval = link.needs_approval;
            }
            None if user.role == "admin" => hosts.push(HostInfo {
                id: format!("address:{}", link.address),
                name: link.address.clone(),
                status: HostStatus::Offline,
                ip_address: None,
                server_codec_mode_support: None,
                agent_connected: false,
                address: Some(link.address.clone()),
                link_error: link.error.clone().or_else(|| Some("connecting…".into())),
                needs_approval: link.needs_approval,
            }),
            None => {}
        }
    }

    Ok((StatusCode::OK, Json(hosts)))
}

/// Adds a computer by address (admins): the server connects to its agent.
/// Adding the same address again changes its password.
async fn add_host_by_address_handler(
    _admin: crate::auth::AdminUser,
    State(state): State<Arc<SignalingState>>,
    Json(req): Json<common::AddHostByAddressRequest>,
) -> impl IntoResponse {
    let Some(address) = links::normalize_address(&req.address) else {
        return (StatusCode::BAD_REQUEST, Json(serde_json::json!({ "error": "Not a valid address" }))).into_response();
    };
    let password = req.password.unwrap_or_default().trim().to_string();
    let saved = sqlx::query(
        "INSERT INTO address_hosts (address, password) VALUES (?, ?)
         ON CONFLICT(address) DO UPDATE SET password = excluded.password",
    )
    .bind(&address)
    .bind(&password)
    .execute(&state.db)
    .await;
    if let Err(e) = saved {
        error!("Database error: {}", e);
        return (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({ "error": "Internal Database Error" }))).into_response();
    }
    let host_id: Option<String> = sqlx::query_scalar("SELECT host_id FROM address_hosts WHERE address = ?")
        .bind(&address)
        .fetch_optional(&state.db)
        .await
        .ok()
        .flatten()
        .flatten();
    info!("Computer added by address: {} ({})", address, if password.is_empty() { "accepted there" } else { "password" });
    links::start(&state, address.clone(), password, host_id);
    (StatusCode::OK, Json(serde_json::json!({ "address": address }))).into_response()
}

async fn delete_host_handler(
    _admin: crate::auth::AdminUser,
    host_id_path: axum::extract::Path<String>,
    State(state): State<Arc<SignalingState>>,
) -> impl IntoResponse {
    let host_id = host_id_path.0;
    info!("Unpairing host: {}", host_id);

    // A computer added by address: stop connecting to it.
    let address = host_id.strip_prefix("address:").map(str::to_owned).or_else(|| state.links.address_of(&host_id));
    if let Some(address) = address {
        state.links.stop(&address);
        let _ = sqlx::query("DELETE FROM address_hosts WHERE address = ?")
            .bind(&address)
            .execute(&state.db)
            .await;
        if host_id.starts_with("address:") {
            return StatusCode::OK.into_response();
        }
    }

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
        )
            .into_response();
    }

    StatusCode::OK.into_response()
}

async fn agent_token_handler(
    _user: crate::auth::AuthenticatedUser,
    State(state): State<Arc<SignalingState>>,
) -> impl IntoResponse {
    Json(serde_json::json!({ "token": state.agent_token }))
}
