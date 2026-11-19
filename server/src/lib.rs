use axum::{
    extract::{ConnectInfo, State},
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
mod db;
mod install;
mod links;
mod mfa;
mod paths;
mod queue;
mod security;
mod service;
mod sources;
mod tls;
mod turn;
pub mod update_notice;
mod web;

pub mod signaling;

use crate::{
    auth::{create_jwt, verify_password_blocking, AuthenticatedUser, DUMMY_HASH},
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

    // New installs: 128 bits from the OS random source. An existing
    // server_token.txt (older, shorter tokens) keeps working above.
    let generated = security::random_hex(16);

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
sign-in key, initial admin password; default: working directory), LUNARIS_TOKEN, JWT_SECRET,
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
    rt.block_on(async {
        update_notice::spawn();
        run_server(shutdown_signal()).await
    })
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
async fn ports_handler(via_tls: Option<axum::Extension<tls::ViaTls>>) -> axum::Json<serde_json::Value> {
    let http: u16 = std::env::var("SERVER_PORT").ok().and_then(|p| p.parse().ok()).unwrap_or(8090);
    axum::Json(ports_json(http, via_tls.is_some()))
}

/// https_self_signed: this page came over the server's own HTTPS listener
/// with its own certificate, which curl and PowerShell refuse (the web then
/// starts the install command with the self-signed option). Not through a
/// reverse proxy, whose certificate is usually trusted: `curl -k` there would
/// only throw away the check on a script that runs as root.
fn ports_json(http: u16, via_tls: bool) -> serde_json::Value {
    serde_json::json!({ "http": http, "https": tls::https_port(), "https_self_signed": via_tls && tls::self_signed() })
}

/// Runs the server until `shutdown` completes.
pub(crate) async fn run_server(
    shutdown: impl std::future::Future<Output = ()> + Send + 'static,
) -> Result<(), Box<dyn std::error::Error>> {
    // Before anything listens: a weak sign-in key is a configuration error.
    auth::check_jwt_secret_env()?;
    let database_url = find_database_url();

    info!("Initializing SQLite database: {}", database_url);
    let pool = init_db(&database_url, &paths::data_file("initial_admin_password.txt")).await?;

    let agent_token = get_or_generate_agent_token();
    // The token itself stays out of the log (logs get shared and collected).
    if std::env::var("LUNARIS_TOKEN").map(|t| !t.trim().is_empty()).unwrap_or(false) {
        info!("Agent connection token: set by LUNARIS_TOKEN");
    } else {
        info!("Agent connection token: in {:?}", paths::data_file("server_token.txt"));
    }

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
        // Two-factor sign-in with an authenticator app (mfa.rs).
        .route("/api/auth/login/mfa", post(mfa::login_mfa_handler))
        .route("/api/auth/mfa", get(mfa::status))
        .route("/api/auth/mfa/setup", post(mfa::setup))
        .route("/api/auth/mfa/enable", post(mfa::enable))
        .route("/api/auth/mfa/disable", post(mfa::disable))
        .route("/api/auth/mfa/recovery-codes", post(mfa::regenerate_recovery_codes))
        .route("/api/admin/users/{id}/mfa", delete(mfa::admin_reset))
        .route("/api/hosts", get(hosts_handler))
        .route("/api/hosts/{id}", delete(delete_host_handler))
        .route("/api/admin/hosts/{id}/reset-key", post(admin::reset_host_key))
        .route("/api/hosts/address", post(add_host_by_address_handler))
        // Computers of other types, shown through a Lunaris computer (sources.rs).
        .route("/api/sources", get(sources::list_sources))
        .route("/api/connectors", get(sources::list_connectors))
        .route("/api/hosts/{id}/sources", post(sources::add_source))
        .route(
            "/api/hosts/{id}/sources/{source_id}",
            axum::routing::put(sources::update_source).delete(sources::remove_source),
        )
        // The set-up of one being added (a pairing that waits for its PIN).
        .route(
            "/api/hosts/{id}/sources/setup/{setup}",
            get(sources::setup_state).delete(sources::cancel_setup),
        )
        .route("/api/agent/token", get(agent_token_handler))
        .route("/api/server/ports", get(ports_handler))
        .route(
            "/api/admin/users",
            get(admin::list_users).post(admin::create_user),
        )
        .route(
            "/api/admin/users/{id}",
            axum::routing::put(admin::update_user).delete(admin::delete_user),
        )
        .route(
            "/api/admin/groups",
            get(admin::list_groups).post(admin::create_group),
        )
        .route(
            "/api/admin/groups/{id}",
            axum::routing::put(admin::update_group).delete(admin::delete_group),
        )
        .route(
            "/api/admin/turn-servers",
            get(admin::list_turn_servers).post(admin::create_turn_server),
        )
        .route(
            "/api/admin/turn-servers/{id}",
            delete(admin::delete_turn_server),
        )
        // One-line agent installation (install.rs).
        .route(
            "/api/admin/install-tokens",
            get(install::list_tokens).post(install::create_token),
        )
        .route("/api/admin/install-tokens/{id}", delete(install::revoke_token))
        .route("/install/agent.sh", get(install::agent_sh))
        .route("/install/agent.ps1", get(install::agent_ps1))
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

    let app = app
        .layer(cors)
        .layer(security_header(header::X_FRAME_OPTIONS, "DENY"))
        .layer(security_header(header::X_CONTENT_TYPE_OPTIONS, "nosniff"))
        .layer(security_header(header::REFERRER_POLICY, "no-referrer"))
        .with_state(signaling_state);

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

use axum::http::header;

fn security_header(
    name: header::HeaderName,
    value: &'static str,
) -> tower_http::set_header::SetResponseHeaderLayer<axum::http::HeaderValue> {
    tower_http::set_header::SetResponseHeaderLayer::if_not_present(
        name,
        axum::http::HeaderValue::from_static(value),
    )
}

fn too_many_attempts() -> (StatusCode, Json<serde_json::Value>) {
    (
        StatusCode::TOO_MANY_REQUESTS,
        Json(serde_json::json!({ "error": "Too many failed sign-in attempts. Try again in a few minutes." })),
    )
}

async fn login_handler(
    State(state): State<Arc<SignalingState>>,
    ConnectInfo(addrs): ConnectInfo<signaling::ConnAddrs>,
    headers: axum::http::HeaderMap,
    Json(payload): Json<LoginRequest>,
) -> Result<axum::response::Response, (StatusCode, Json<serde_json::Value>)> {
    let now = std::time::Instant::now();
    let ip_key = security::client_ip(addrs.remote.ip(), &headers).to_string();
    // Per name *and* address: guessing from one address can't lock the
    // account for everyone else.
    let user_key = format!("{}\n{}", payload.username, ip_key);
    if state.login_ip_limit.is_blocked(&ip_key, now) || state.login_user_limit.is_blocked(&user_key, now) {
        return Err(too_many_attempts());
    }

    let row: Option<(String, String, String, i64, i64)> = sqlx::query_as(
        "SELECT id, password_hash, COALESCE(role, 'user') as role, is_active, token_version FROM users WHERE username = ?",
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

    // Unknown users are checked against a dummy hash: same time as a wrong password.
    let (found, hash) = match &row {
        Some((_, hash, ..)) => (true, hash.clone()),
        None => (false, DUMMY_HASH.to_string()),
    };
    let password_ok = verify_password_blocking(payload.password.clone(), hash).await;
    // A disabled account answers like a wrong password (and counts as a
    // failure): the answer must not tell whether a guessed password was right.
    let Some((user_id, _, role, _, token_version)) =
        row.filter(|(_, _, _, active, _)| found && password_ok && *active != 0)
    else {
        state.login_ip_limit.record_failure(&ip_key, now);
        state.login_user_limit.record_failure(&user_key, now);
        return Err((
            StatusCode::UNAUTHORIZED,
            Json(serde_json::json!({ "error": "Invalid username or password" })),
        ));
    };
    state.login_user_limit.reset(&user_key);

    // Two-factor sign-in on: no session yet, only the step that asks for the code.
    match mfa::is_enabled(&state.db, &user_id).await {
        Ok(true) => return Ok(mfa::challenge(&state, &user_id, &payload.username)),
        Ok(false) => {}
        Err(e) => {
            error!("Database error: {}", e);
            return Err((
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({ "error": "Internal Database Error" })),
            ));
        }
    }

    let token = create_jwt(&user_id, &payload.username, &role, token_version).map_err(|e| {
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
    )
        .into_response())
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
    let db_error = |e: sqlx::Error| {
        error!("Database error: {}", e);
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": "Internal Database Error" })),
        )
    };
    // Admins see every host, others the hosts of their groups (the same rule
    // the signaling WebSocket applies).
    let access = security::host_access(&state.db, &user.user_id).await.map_err(db_error)?;
    let all: Vec<(String, String, String, Option<String>, Option<i64>)> =
        sqlx::query_as("SELECT id, name, status, ip_address, server_codec_mode_support FROM hosts")
            .fetch_all(&state.db)
            .await
            .map_err(db_error)?;
    let rows: Vec<(String, String, String, Option<String>, Option<i64>)> = all
            .into_iter()
            .filter(|(id, ..)| access.allows(id))
            .collect();

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
            None if access.is_all() => hosts.push(HostInfo {
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

    // Its group memberships go explicitly (foreign keys are not on for every
    // pooled connection): an agent registering this id again starts with none.
    let _ = sqlx::query("DELETE FROM host_groups WHERE host_id = ?")
        .bind(&host_id)
        .execute(&state.db)
        .await;
    let _ = sqlx::query("DELETE FROM source_removals WHERE host_id = ?")
        .bind(&host_id)
        .execute(&state.db)
        .await;
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
    // The agent connected as this host is disconnected, its sessions end.
    signaling::disconnect_agent(&state, &host_id).await;

    StatusCode::OK.into_response()
}

/// The shared agent registration token: admins only (it lets a computer
/// join as a host).
async fn agent_token_handler(
    _admin: crate::auth::AdminUser,
    State(state): State<Arc<SignalingState>>,
) -> impl IntoResponse {
    Json(serde_json::json!({ "token": state.agent_token }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::extract::FromRequestParts;

    async fn state_with_users() -> Arc<SignalingState> {
        let db = security::tests::test_db().await;
        let hash = bcrypt::hash("right-pw", 4).unwrap();
        for (id, name, active) in [("u1", "alice", 1), ("u2", "carol", 0)] {
            sqlx::query("INSERT INTO users (id, username, password_hash, role, is_active) VALUES (?, ?, ?, 'user', ?)")
                .bind(id)
                .bind(name)
                .bind(&hash)
                .bind(active)
                .execute(&db)
                .await
                .unwrap();
        }
        Arc::new(SignalingState::new(db, "t".into(), None))
    }

    async fn login_via(state: &Arc<SignalingState>, peer: &str, xff: Option<&str>, user: &str, pw: &str) -> StatusCode {
        let addrs = signaling::ConnAddrs { local: None, remote: format!("{peer}:5000").parse().unwrap() };
        let mut headers = axum::http::HeaderMap::new();
        if let Some(x) = xff {
            headers.insert("x-forwarded-for", x.parse().unwrap());
        }
        let req = LoginRequest { username: user.into(), password: pw.into() };
        match login_handler(State(state.clone()), ConnectInfo(addrs), headers, Json(req)).await {
            Ok(r) => r.into_response().status(),
            Err((code, _)) => code,
        }
    }

    async fn login(state: &Arc<SignalingState>, ip: &str, user: &str, pw: &str) -> StatusCode {
        login_via(state, ip, None, user, pw).await
    }

    #[tokio::test]
    async fn login_limit_and_inactive_users() {
        let state = state_with_users().await;
        assert_eq!(login(&state, "10.0.0.1", "alice", "right-pw").await, StatusCode::OK);
        // A disabled account answers like a wrong password (no password oracle).
        assert_eq!(login(&state, "10.0.0.1", "carol", "right-pw").await, StatusCode::UNAUTHORIZED);
        assert_eq!(login(&state, "10.0.0.1", "carol", "wrong").await, StatusCode::UNAUTHORIZED);
        for _ in 0..10 {
            assert_eq!(login(&state, "10.0.0.2", "alice", "wrong").await, StatusCode::UNAUTHORIZED);
        }
        // The guessing address is blocked, even with the right password and
        // for other names.
        assert_eq!(login(&state, "10.0.0.2", "alice", "right-pw").await, StatusCode::TOO_MANY_REQUESTS);
        assert_eq!(login(&state, "10.0.0.2", "carol", "right-pw").await, StatusCode::TOO_MANY_REQUESTS);
        // The account itself is not locked for everyone else.
        assert_eq!(login(&state, "10.0.0.3", "alice", "right-pw").await, StatusCode::OK);
    }

    #[tokio::test]
    async fn login_limit_behind_local_proxy() {
        let state = state_with_users().await;
        for _ in 0..10 {
            let code = login_via(&state, "127.0.0.1", Some("6.6.6.6, 203.0.113.9"), "alice", "wrong").await;
            assert_eq!(code, StatusCode::UNAUTHORIZED);
        }
        let blocked = login_via(&state, "127.0.0.1", Some("203.0.113.9"), "alice", "right-pw").await;
        assert_eq!(blocked, StatusCode::TOO_MANY_REQUESTS);
        // Another client through the same proxy is not affected.
        let other = login_via(&state, "127.0.0.1", Some("203.0.113.10"), "alice", "right-pw").await;
        assert_eq!(other, StatusCode::OK);
    }

    #[tokio::test]
    async fn admin_resets_a_host_key() {
        let state = state_with_users().await;
        sqlx::query("INSERT INTO hosts (id, name, status, agent_key_hash) VALUES ('h1', 'H1', 'Offline', 'abc')")
            .execute(&state.db)
            .await
            .unwrap();
        let admin = auth::AdminUser { user_id: "u1".into(), username: "alice".into() };
        let r = admin::reset_host_key(admin, State(state.clone()), axum::extract::Path("h1".into())).await;
        assert_eq!(r.into_response().status(), StatusCode::OK);
        let hash: Option<String> = sqlx::query_scalar("SELECT agent_key_hash FROM hosts WHERE id = 'h1'")
            .fetch_one(&state.db)
            .await
            .unwrap();
        assert!(hash.is_none());
        let admin = auth::AdminUser { user_id: "u1".into(), username: "alice".into() };
        let r = admin::reset_host_key(admin, State(state.clone()), axum::extract::Path("nope".into())).await;
        assert_eq!(r.into_response().status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn inactive_user_token_is_rejected() {
        let state = state_with_users().await;
        let parts = |token: String| {
            axum::http::Request::builder()
                .header("Authorization", format!("Bearer {token}"))
                .body(())
                .unwrap()
                .into_parts()
                .0
        };
        let mut ok = parts(auth::create_jwt("u1", "alice", "user", 0).unwrap());
        assert!(AuthenticatedUser::from_request_parts(&mut ok, &state).await.is_ok());
        let mut off = parts(auth::create_jwt("u2", "carol", "user", 0).unwrap());
        assert!(AuthenticatedUser::from_request_parts(&mut off, &state).await.is_err());
        // The role claimed in the token is not trusted.
        let mut forged_role = parts(auth::create_jwt("u1", "alice", "admin", 0).unwrap());
        assert!(auth::AdminUser::from_request_parts(&mut forged_role, &state).await.is_err());
    }

    /// The self-signed hint is only for pages served by the server's own
    /// HTTPS listener, not for plain HTTP or a reverse proxy in front.
    #[test]
    fn self_signed_hint_only_on_own_https() {
        if std::env::var_os("LUNARIS_TLS_CERT").is_some() || tls::https_port().is_none() {
            return;
        }
        assert_eq!(ports_json(8090, true)["https_self_signed"], true);
        assert_eq!(ports_json(8090, false)["https_self_signed"], false);
        assert_eq!(ports_json(8090, false)["http"], 8090);
    }
}
