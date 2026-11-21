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
mod gateway;
mod host_admin;
mod host_settings;
mod server_info;
mod import;
mod install;
mod link_tickets;
mod links;
mod mfa;
mod paths;
mod queue;
mod security;
mod service;
mod setup;
mod sources;
mod tls;
mod turn;
pub mod update_notice;
mod web;
#[cfg(windows)]
mod win_acl;

pub mod signaling;

use crate::{
    auth::{create_jwt, verify_password_blocking, AuthenticatedUser, DUMMY_HASH},
    db::init_db,
    signaling::{agent_ws_handler, client_ws_handler, SignalingState},
};

/// Where the web client is served from disk, if not the embedded copy:
/// `LUNARIS_WEB_DIR` when set (a folder with index.html); debug builds also
/// look for `web/dist` (working directory, the source tree, beside the
/// program). Release builds serve only the embedded copy otherwise: a
/// `web/dist` folder someone else could create (the build machine's path is
/// in the binary, the working directory may be anyone's) must not replace it.
fn web_dir() -> Option<std::path::PathBuf> {
    if let Some(dir) = std::env::var_os("LUNARIS_WEB_DIR").filter(|d| !d.is_empty()) {
        let dir = std::path::PathBuf::from(dir);
        if dir.join("index.html").is_file() {
            return Some(dir);
        }
        error!("LUNARIS_WEB_DIR={:?} has no index.html; serving the embedded web client", dir);
        return None;
    }
    #[cfg(debug_assertions)]
    {
        find_dist_path("web/dist")
    }
    #[cfg(not(debug_assertions))]
    {
        None
    }
}

#[cfg_attr(not(debug_assertions), allow(dead_code))]
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
            // Written by builds from before write_private: maybe 0644. The
            // token itself stays as it is (agents use it).
            paths::tighten_private(token_file);
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
  import <dir> [--replace]    Copy the accounts and computers of the desktop
                              app's built-in server (its data folder <dir>)
                              into this service's data folder; stop the
                              service first. Run as root / Administrator.
  status                      Show the service status
  help, version

Environment: SERVER_PORT (default 8090), LUNARIS_HTTPS_PORT (default 8443,
off to disable), LUNARIS_TLS_CERT + LUNARIS_TLS_KEY (PEM; default: a
self-signed certificate kept in the data directory), LUNARIS_DATA_DIR (database, token,
sign-in key, first-start setup code; default: working directory), LUNARIS_TOKEN, JWT_SECRET,
LUNARIS_ADMIN_PASSWORD (first start only: makes the `admin` account and skips
the setup page), DATABASE_URL, RUST_LOG, LUNARIS_GATEWAY (off|on|<path of
lunaris-agent>: the server gateway, which runs lunaris-agent to reach
RustDesk, Sunshine, RDP and VNC computers through this server; on by default
when lunaris-agent is installed, switched on the web page's Server tab),
LUNARIS_GATEWAY_LOG (its log level).
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
        Some("import") => import::run(rest),
        #[cfg(windows)]
        Some("prepare-data-dir") => service::prepare_data_dir_command(),
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
    let pool = init_db(&database_url).await?;
    // The first admin: the setup page with a one-time code, or LUNARIS_ADMIN_PASSWORD.
    setup::prepare(&pool, &paths::data_file(setup::CODE_FILE), std::env::var("LUNARIS_ADMIN_PASSWORD").ok()).await?;

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
    // The agent the server runs itself, when it is on and found (gateway.rs).
    gateway::start(&signaling_state).await;
    let gateway_state = signaling_state.clone();

    let cors = cors_layer(cors_origins_from_env());

    // The web client from disk (LUNARIS_WEB_DIR, development), else embedded.
    let react_path = web_dir();

    // Build routes
    let mut app = Router::new()
        .route("/api/auth/login", post(login_handler))
        .route("/api/auth/me", get(admin::get_current_user))
        // Sign out everywhere: every session of the signed-in user ends.
        .route("/api/auth/logout-all", post(admin::logout_all))
        // First start: the first admin, made with the setup code (setup.rs).
        .route("/api/setup", get(setup::status).post(setup::complete))
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
        // The name a computer is shown with; a computer added by address (host_admin.rs).
        .route("/api/admin/hosts/{id}/name", axum::routing::put(host_admin::rename_host))
        .route("/api/admin/hosts/{id}/address", axum::routing::put(host_admin::update_address_host))
        // Version, update, ports and data folder (server_info.rs).
        .route("/api/server/info", get(server_info::info))
        // The server gateway: its state and its switch (gateway.rs).
        .route("/api/admin/server/gateway", get(gateway::get_gateway).put(gateway::put_gateway))
        .route(
            "/api/admin/hosts/{id}/settings",
            get(host_settings::get_settings).put(host_settings::put_settings),
        )
        .route("/api/admin/hosts/{id}/settings/password", post(host_settings::password))
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
        // lunaris://connect links carry a one-time ticket, not the sign-in
        // token (link_tickets.rs).
        .route("/api/links/ticket", post(link_tickets::create_ticket))
        .route("/api/links/redeem", post(link_tickets::redeem_ticket))
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
        .route("/api/admin/turn-servers/builtin", get(admin::builtin_turn_status))
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
        .layer(axum::middleware::map_response(html_security_headers))
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
    // Its standard input closes, then it is killed if it is still there.
    gateway_state.gateway.shutdown().await;
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

/// The Content-Security-Policy of the web client's pages. `connect-src`
/// stays open: the web client talks to whatever server address the user
/// types at sign-in.
pub(crate) const CONTENT_SECURITY_POLICY: &str = "default-src 'self'; script-src 'self'; style-src 'self' 'unsafe-inline'; \
img-src 'self' data: blob:; media-src 'self' blob:; font-src 'self' data:; connect-src * ws: wss: data: blob:; \
worker-src 'self' blob:; frame-ancestors 'none'; base-uri 'none'; form-action 'self'";

/// HTML responses (the web client's pages) get the CSP.
async fn html_security_headers(mut response: axum::response::Response) -> axum::response::Response {
    let html = response
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.trim_start().to_ascii_lowercase().starts_with("text/html"));
    if html && !response.headers().contains_key(header::CONTENT_SECURITY_POLICY) {
        response
            .headers_mut()
            .insert(header::CONTENT_SECURITY_POLICY, axum::http::HeaderValue::from_static(CONTENT_SECURITY_POLICY));
    }
    response
}

/// Which other web origins may call the API from a browser.
#[derive(Debug, Clone, PartialEq, Eq)]
enum CorsOrigins {
    /// `LUNARIS_CORS_ORIGINS=*`, and debug builds (the Vite dev server).
    Any,
    /// The listed origins, and pages of this same server on another port
    /// or scheme (http :8090 and https :8443).
    Listed(Vec<String>),
}

fn parse_cors_origins(value: Option<&str>) -> CorsOrigins {
    match value.map(str::trim) {
        Some("*") => CorsOrigins::Any,
        Some(list) => CorsOrigins::Listed(
            list.split(',')
                .map(|o| o.trim().trim_end_matches('/').to_ascii_lowercase())
                .filter(|o| !o.is_empty())
                .collect(),
        ),
        None if cfg!(debug_assertions) => CorsOrigins::Any,
        None => CorsOrigins::Listed(Vec::new()),
    }
}

fn cors_origins_from_env() -> CorsOrigins {
    let origins = parse_cors_origins(std::env::var("LUNARIS_CORS_ORIGINS").ok().as_deref());
    if let CorsOrigins::Listed(list) = &origins {
        if !list.is_empty() {
            info!("Cross-origin API calls allowed from {}", list.join(", "));
        }
    }
    origins
}

/// The host name of "scheme://host:port" or "host:port" (brackets kept off IPv6).
fn host_name_of(authority_or_origin: &str) -> &str {
    let rest = authority_or_origin.split_once("://").map(|(_, r)| r).unwrap_or(authority_or_origin);
    let rest = rest.split('/').next().unwrap_or("");
    if let Some(v6) = rest.strip_prefix('[') {
        return v6.split(']').next().unwrap_or("");
    }
    rest.rsplit_once(':').map(|(h, _)| h).unwrap_or(rest)
}

/// Whether a browser page from `origin` may read the API's answers to a
/// request with these headers.
fn cors_allows(origins: &CorsOrigins, origin: &str, host: Option<&str>) -> bool {
    let origin = origin.trim().trim_end_matches('/').to_ascii_lowercase();
    match origins {
        CorsOrigins::Any => true,
        CorsOrigins::Listed(list) => {
            list.iter().any(|o| *o == origin)
                || host.is_some_and(|h| !h.is_empty() && host_name_of(&origin).eq_ignore_ascii_case(host_name_of(h)))
        }
    }
}

fn cors_layer(origins: CorsOrigins) -> CorsLayer {
    let allow = match origins {
        CorsOrigins::Any => tower_http::cors::AllowOrigin::any(),
        origins => tower_http::cors::AllowOrigin::predicate(move |origin, parts| {
            let host = parts.headers.get(header::HOST).and_then(|h| h.to_str().ok());
            origin.to_str().is_ok_and(|o| cors_allows(&origins, o, host))
        }),
    };
    CorsLayer::new().allow_origin(allow).allow_methods(Any).allow_headers(Any)
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
    let ip_key = security::client_limit_key(addrs.remote.ip(), &headers);
    // Per name *and* address: guessing from one address can't lock the
    // account for everyone else.
    let user_key = format!("{}\n{}", payload.username, ip_key);
    // Attempts are taken before anything is awaited, so requests sent at
    // once count too (security.rs `try_begin`).
    let Some(ip_attempt) = state.login_ip_limit.try_begin(&ip_key, now) else {
        return Err(too_many_attempts());
    };
    let Some(user_attempt) = state.login_user_limit.try_begin(&user_key, now) else {
        return Err(too_many_attempts());
    };
    // Per name across all addresses (guessing from many addresses): a
    // window, not for good, and never from an address the user signed in
    // from lately, so the owner is not locked out by someone else's guesses.
    let known_address = state.login_known.knows(&payload.username, &ip_key, now);
    let name_attempt = if known_address {
        None
    } else {
        match state.login_name_limit.try_begin(&payload.username, now) {
            Some(a) => Some(a),
            None => return Err(too_many_attempts()),
        }
    };

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
        ip_attempt.fail();
        user_attempt.fail();
        if let Some(a) = name_attempt {
            a.fail();
        }
        return Err((
            StatusCode::UNAUTHORIZED,
            Json(serde_json::json!({ "error": "Invalid username or password" })),
        ));
    };
    ip_attempt.succeed();
    user_attempt.succeed();
    if let Some(a) = name_attempt {
        a.succeed();
    }
    state.login_user_limit.reset(&user_key);
    state.login_known.remember(&payload.username, &ip_key, now);

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
    #[allow(clippy::type_complexity)]
    let all: Vec<(String, String, String, Option<String>, Option<i64>, Option<String>, Option<String>)> =
        sqlx::query_as("SELECT id, name, status, ip_address, server_codec_mode_support, addresses, display_name FROM hosts")
            .fetch_all(&state.db)
            .await
            .map_err(db_error)?;
    #[allow(clippy::type_complexity)]
    let rows: Vec<(String, String, String, Option<String>, Option<i64>, Option<String>, Option<String>)> = all
            .into_iter()
            .filter(|(id, ..)| access.allows(id))
            .collect();
    // The names computers added by address were given, by address.
    let address_names: std::collections::HashMap<String, String> =
        sqlx::query_as::<_, (String, String)>("SELECT address, name FROM address_hosts WHERE name <> ''")
            .fetch_all(&state.db)
            .await
            .map_err(db_error)?
            .into_iter()
            .collect();

    let active_host_ids: std::collections::HashSet<String> = {
        let sessions = state.local_sessions.read().unwrap();
        sessions.keys().cloned().collect()
    };

    let mut futures = Vec::new();

    for (id, agent_name, status_str, ip_address, server_codec_mode_support, addresses, display_name) in rows {
        // An administrator's name for it, else the one its agent announces.
        let display_name = display_name.filter(|n| !n.trim().is_empty());
        let name = display_name.clone().unwrap_or_else(|| agent_name.clone());
        let agent_name = display_name.is_some().then_some(agent_name);
        let addresses: Vec<String> = addresses.and_then(|a| serde_json::from_str(&a).ok()).unwrap_or_default();
        let ip_address_clone = ip_address.clone();
        let db_pool = state.db.clone();
        let is_currently_busy = active_host_ids.contains(&id);
        let state_clone = state.clone();
        let is_gateway = state.gateway.is_gateway(&id);

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
                addresses,
                server_codec_mode_support: server_codec_mode_support.map(|v| v as u32),
                agent_connected: is_agent_online,
                address: None,
                link_error: None,
                needs_approval: false,
                agent_name,
                gateway: is_gateway,
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
                // The name it was added with, until an administrator renames it.
                if h.agent_name.is_none() {
                    if let Some(given) = address_names.get(&link.address) {
                        h.agent_name = Some(std::mem::replace(&mut h.name, given.clone()));
                    }
                }
            }
            None if access.is_all() => hosts.push(HostInfo {
                id: format!("address:{}", link.address),
                name: address_names.get(&link.address).cloned().unwrap_or_else(|| link.address.clone()),
                agent_name: None,
                status: HostStatus::Offline,
                ip_address: None,
                addresses: Vec::new(),
                server_codec_mode_support: None,
                agent_connected: false,
                address: Some(link.address.clone()),
                link_error: link.error.clone().or_else(|| Some("connecting…".into())),
                needs_approval: link.needs_approval,
                gateway: false,
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
    let name = match req.name.as_deref().map(host_admin::clean_name).transpose() {
        Ok(name) => name.flatten().unwrap_or_default(),
        Err(why) => return (StatusCode::BAD_REQUEST, Json(serde_json::json!({ "error": why }))).into_response(),
    };
    // Again: a new password, and a new name when one is given.
    let saved = sqlx::query(
        "INSERT INTO address_hosts (address, password, name) VALUES (?, ?, ?)
         ON CONFLICT(address) DO UPDATE SET password = excluded.password,
           name = CASE WHEN excluded.name <> '' THEN excluded.name ELSE address_hosts.name END",
    )
    .bind(&address)
    .bind(&password)
    .bind(&name)
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
    // The server's own gateway goes when it is turned off, not here (it
    // would come back at once); once off, its entry may be removed.
    if state.gateway.is_gateway(&host_id) && state.gateway.enabled() {
        return (
            StatusCode::CONFLICT,
            Json(serde_json::json!({ "error": "This is the server gateway: turn it off on the Server page instead" })),
        )
            .into_response();
    }
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

    /// The list shows an administrator's name first, then the name a
    /// computer was added by address with, then its agent's; the agent's
    /// name is given beside a name that is not its own.
    #[tokio::test]
    async fn hosts_list_uses_display_names() {
        let state = state_with_users().await;
        sqlx::query("UPDATE users SET role = 'admin' WHERE id = 'u1'").execute(&state.db).await.unwrap();
        for sql in [
            "INSERT INTO hosts (id, name, status, display_name) VALUES ('h1', 'DESKTOP-1', 'Offline', 'Office')",
            "INSERT INTO hosts (id, name, status) VALUES ('h2', 'DESKTOP-2', 'Offline')",
            "INSERT INTO hosts (id, name, status) VALUES ('h3', 'DESKTOP-3', 'Offline')",
            "INSERT INTO address_hosts (address, password, host_id, name) VALUES ('192.0.2.3:8091', '', 'h3', 'Den')",
            "INSERT INTO address_hosts (address, password, host_id, name) VALUES ('192.0.2.4:8091', '', NULL, 'Lab')",
        ] {
            sqlx::query(sql).execute(&state.db).await.unwrap();
        }
        links::start(&state, "192.0.2.3:8091".into(), String::new(), Some("h3".into()));
        links::start(&state, "192.0.2.4:8091".into(), String::new(), None);
        let user = AuthenticatedUser { user_id: "u1".into(), username: "alice".into(), role: "admin".into() };
        let r = hosts_handler(user, State(state.clone())).await.ok().unwrap().into_response();
        let bytes = axum::body::to_bytes(r.into_body(), 1 << 20).await.unwrap();
        let list: Vec<serde_json::Value> = serde_json::from_slice(&bytes).unwrap();
        let find = |id: &str| list.iter().find(|h| h["id"] == id).cloned().unwrap();
        assert_eq!(find("h1")["name"], "Office");
        assert_eq!(find("h1")["agent_name"], "DESKTOP-1");
        assert_eq!(find("h2")["name"], "DESKTOP-2");
        assert!(find("h2").get("agent_name").is_none());
        assert_eq!(find("h3")["name"], "Den");
        assert_eq!(find("h3")["agent_name"], "DESKTOP-3");
        assert_eq!(find("address:192.0.2.4:8091")["name"], "Lab");
        state.links.stop("192.0.2.3:8091");
        state.links.stop("192.0.2.4:8091");
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

    /// Wrong passwords sent at once: no more than the limit get checked.
    #[tokio::test]
    async fn concurrent_logins_are_limited() {
        let state = state_with_users().await;
        let tries = (0..30).map(|_| {
            let state = state.clone();
            async move { login(&state, "10.0.0.20", "alice", "wrong").await }
        });
        let codes = futures_util::future::join_all(tries).await;
        let checked = codes.iter().filter(|c| **c == StatusCode::UNAUTHORIZED).count();
        assert!(checked <= 10, "{checked} passwords were checked");
        assert_eq!(login(&state, "10.0.0.20", "alice", "right-pw").await, StatusCode::TOO_MANY_REQUESTS);
    }

    /// One name guessed from many addresses waits out a window; its owner,
    /// from an address they signed in from before, is not locked out.
    #[tokio::test]
    async fn a_name_guessed_from_many_addresses() {
        let state = state_with_users().await;
        assert_eq!(login(&state, "10.0.9.1", "alice", "right-pw").await, StatusCode::OK);
        for i in 0..20 {
            assert_eq!(login(&state, &format!("10.1.0.{i}"), "alice", "wrong").await, StatusCode::UNAUTHORIZED);
        }
        assert_eq!(login(&state, "10.2.0.1", "alice", "right-pw").await, StatusCode::TOO_MANY_REQUESTS);
        assert_eq!(login(&state, "10.9.9.9", "carol", "wrong").await, StatusCode::UNAUTHORIZED, "other names are not held back");
        assert_eq!(login(&state, "10.0.9.1", "alice", "right-pw").await, StatusCode::OK, "the owner's own address");
    }

    /// IPv6 addresses of one /64 count as one address.
    #[tokio::test]
    async fn ipv6_logins_count_per_64() {
        let state = state_with_users().await;
        for i in 0..10 {
            let ip = format!("[2001:db8:5:6::{:x}]", i + 1);
            let addrs = signaling::ConnAddrs { local: None, remote: format!("{ip}:5000").parse().unwrap() };
            let req = LoginRequest { username: "carol".into(), password: "wrong".into() };
            let r = login_handler(State(state.clone()), ConnectInfo(addrs), Default::default(), Json(req)).await;
            assert_eq!(r.err().unwrap().0, StatusCode::UNAUTHORIZED);
        }
        let addrs = signaling::ConnAddrs { local: None, remote: "[2001:db8:5:6::ffff]:5000".parse().unwrap() };
        let req = LoginRequest { username: "alice".into(), password: "right-pw".into() };
        let r = login_handler(State(state.clone()), ConnectInfo(addrs), Default::default(), Json(req)).await;
        assert_eq!(r.err().unwrap().0, StatusCode::TOO_MANY_REQUESTS);
    }

    /// Passwords from before the 10-character rule still sign in.
    #[tokio::test]
    async fn an_existing_short_password_still_signs_in() {
        let state = state_with_users().await;
        let old = bcrypt::hash("abc123", 4).unwrap();
        sqlx::query("INSERT INTO users (id, username, password_hash, role, is_active) VALUES ('u9', 'old', ?, 'user', 1)")
            .bind(old)
            .execute(&state.db)
            .await
            .unwrap();
        assert_eq!(login(&state, "10.0.0.1", "old", "abc123").await, StatusCode::OK);
    }

    #[tokio::test]
    async fn logout_all_ends_every_session() {
        let state = state_with_users().await;
        let (tx, mut rx) = crate::queue::channel::<common::ServerToClientMessage>();
        state.clients.write().unwrap().insert("c1".into(), tx);
        state.client_connections.write().unwrap().insert("c1".into(), "u1".into());
        let token = auth::create_jwt("u1", "alice", "user", 0).unwrap();
        let user = AuthenticatedUser { user_id: "u1".into(), username: "alice".into(), role: "user".into() };
        let r = admin::logout_all(user, State(state.clone())).await;
        assert_eq!(r.ok().unwrap().into_response().status(), StatusCode::OK);
        assert!(rx.recv().await.is_none(), "the connection is closed");
        let mut parts = axum::http::Request::builder()
            .header("Authorization", format!("Bearer {token}"))
            .body(())
            .unwrap()
            .into_parts()
            .0;
        assert!(AuthenticatedUser::from_request_parts(&mut parts, &state).await.is_err());
        assert_eq!(login(&state, "10.0.0.1", "alice", "right-pw").await, StatusCode::OK);
    }

    #[tokio::test]
    async fn html_gets_the_content_security_policy() {
        let html = axum::response::Response::builder()
            .header(header::CONTENT_TYPE, "text/html; charset=utf-8")
            .body(axum::body::Body::empty())
            .unwrap();
        let r = html_security_headers(html).await;
        let csp = r.headers().get(header::CONTENT_SECURITY_POLICY).unwrap().to_str().unwrap();
        assert!(csp.contains("frame-ancestors 'none'") && csp.contains("connect-src * ws: wss:") && csp.contains("script-src 'self';"));
        let json = axum::response::Response::builder()
            .header(header::CONTENT_TYPE, "application/json")
            .body(axum::body::Body::empty())
            .unwrap();
        assert!(html_security_headers(json).await.headers().get(header::CONTENT_SECURITY_POLICY).is_none());
        // The embedded index.html as served.
        let page = web::serve("/".parse().unwrap()).await;
        if page.status() == StatusCode::OK {
            assert!(html_security_headers(page).await.headers().contains_key(header::CONTENT_SECURITY_POLICY));
        }
    }

    #[test]
    fn cors_only_for_listed_origins_and_this_server() {
        let none = parse_cors_origins(Some(""));
        assert_eq!(none, CorsOrigins::Listed(vec![]));
        assert!(!cors_allows(&none, "https://evil.example", Some("lunaris.lan:8090")));
        // The same server on another port or scheme (http :8090, https :8443).
        assert!(cors_allows(&none, "https://lunaris.lan:8443", Some("lunaris.lan:8090")));
        assert!(cors_allows(&none, "http://[fd00::5]:8090", Some("[fd00::5]:8443")));
        assert!(!cors_allows(&none, "http://lunaris.lan.evil:8090", Some("lunaris.lan:8090")));
        let listed = parse_cors_origins(Some("https://web.example/, http://other:5173"));
        assert!(cors_allows(&listed, "https://web.example", Some("api:8090")));
        assert!(cors_allows(&listed, "http://OTHER:5173", None));
        assert!(!cors_allows(&listed, "https://web.example.evil", Some("api:8090")));
        assert_eq!(parse_cors_origins(Some("*")), CorsOrigins::Any);
        assert!(cors_allows(&CorsOrigins::Any, "https://anything", None));
        #[cfg(not(debug_assertions))]
        assert_eq!(parse_cors_origins(None), CorsOrigins::Listed(vec![]));
    }

    /// LUNARIS_WEB_DIR names the folder served from disk; without it a
    /// release build serves only the embedded copy (`web_dir` = None).
    #[test]
    fn the_web_client_folder() {
        let dir = std::env::temp_dir().join(format!("lunaris-web-{}", security::random_hex(6)));
        std::fs::create_dir_all(&dir).unwrap();
        // SAFETY: tests touching LUNARIS_WEB_DIR are only this one.
        unsafe { std::env::set_var("LUNARIS_WEB_DIR", &dir) };
        assert_eq!(web_dir(), None, "no index.html: the embedded copy");
        std::fs::write(dir.join("index.html"), "<html></html>").unwrap();
        assert_eq!(web_dir(), Some(dir.clone()));
        unsafe { std::env::remove_var("LUNARIS_WEB_DIR") };
        #[cfg(not(debug_assertions))]
        assert_eq!(web_dir(), None);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
