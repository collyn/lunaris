use axum::{
    extract::{
        connect_info::Connected,
        ws::{Message, WebSocket, WebSocketUpgrade},
        ConnectInfo, Query, State,
    },
    http::StatusCode,
    response::IntoResponse,
};
use common::{
    AgentMessage, ClientMessage, HostStatus, ServerToAgentMessage, ServerToClientMessage,
    SignalingMessage,
};
use futures_util::{SinkExt, StreamExt};
use sqlx::SqlitePool;
use std::{
    collections::HashMap,
    net::{IpAddr, SocketAddr},
    sync::{Arc, RwLock},
};
use tracing::{debug, error, info, warn};
use uuid::Uuid;

use crate::auth::verify_jwt;
use crate::queue::{Inbox, Outbox};
use crate::security::{self, AgentKeyDecision, FailureLimiter};

/// Largest WebSocket message accepted from a client: SDP answers and ICE
/// candidates are a few KiB, so this leaves ample headroom.
pub const CLIENT_MAX_MESSAGE: usize = 1024 * 1024;
/// From an agent: app lists may carry base64 icons (none today), so more room.
pub const AGENT_MAX_MESSAGE: usize = 8 * 1024 * 1024;
/// Open client WebSockets per user.
pub const MAX_CLIENT_SOCKETS_PER_USER: usize = 32;
use webrtc::peer_connection::RTCIceCandidateInit;
use webrtc::peer_connection::RTCSessionDescription;

/// Both ends of an HTTP/WebSocket connection: the local address is the server
/// address that peer uses, which is where it can reach the TURN relay.
#[derive(Clone, Copy, Debug)]
pub struct ConnAddrs {
    pub local: Option<SocketAddr>,
    pub remote: SocketAddr,
}

impl Connected<axum::serve::IncomingStream<'_, tokio::net::TcpListener>> for ConnAddrs {
    fn connect_info(target: axum::serve::IncomingStream<'_, tokio::net::TcpListener>) -> Self {
        ConnAddrs {
            local: target.io().local_addr().ok(),
            remote: *target.remote_addr(),
        }
    }
}

pub struct SignalingState {
    pub db: SqlitePool,
    /// Built-in TURN relay (None when disabled).
    pub turn: Option<Arc<crate::turn::TurnService>>,
    // agent_id / client conn_id -> server address that peer connected to
    pub agent_local_ips: RwLock<HashMap<String, IpAddr>>,
    pub client_local_ips: RwLock<HashMap<String, IpAddr>>,
    pub agent_token: String,
    // agent_id -> sender to agent WS
    pub agents: RwLock<HashMap<String, Outbox<ServerToAgentMessage>>>,
    // conn_id -> sender to client WS
    pub clients: RwLock<HashMap<String, Outbox<ServerToClientMessage>>>,
    // conn_id -> agent_id of the streaming session started on that connection
    // (agents address clients by conn_id)
    pub client_to_agent: RwLock<HashMap<String, String>>,
    // agent_id -> active connection Uuid
    pub agent_connections: RwLock<HashMap<String, Uuid>>,
    // conn_id (String) -> client_id — tracks which client owns each connection
    pub client_connections: RwLock<HashMap<String, String>>,
    // conn_id -> host_id of file-transfer connections (agent/src/files.rs)
    pub file_sessions: RwLock<HashMap<String, String>>,
    /// (conn_id, host_id): the connection asked that host something outside a
    /// session (app list, capabilities, stop the stream), so the host's
    /// answers may reach it. Agents' messages reach only connections with a
    /// session, a file session or a question with them.
    pub client_asked: RwLock<std::collections::HashSet<(String, String)>>,
    // client_id -> BridgeSession managed by server
    pub local_sessions: RwLock<HashMap<String, Arc<crate::bridge::BridgeSession>>>,
    /// Computers added by address (the server connects to them).
    pub links: crate::links::Links,
    /// The agent the server runs itself (gateway.rs).
    pub gateway: crate::gateway::Gateway,
    /// Computers of other types that agents show (sources.rs).
    pub sources: crate::sources::Registry,
    /// Requests about computers' own settings waiting for their agents
    /// (host_settings.rs).
    pub host_settings: crate::host_settings::Pending,
    /// Failed sign-ins per client address and per (username, address).
    pub login_ip_limit: FailureLimiter,
    pub login_user_limit: FailureLimiter,
    /// Wrong agent tokens per client address.
    pub agent_token_limit: FailureLimiter,
    /// Sign-ins waiting for their two-factor code (mfa.rs).
    pub mfa: crate::mfa::MfaState,
    /// conn_id -> the one host (and source of it, or None for its own
    /// screen) a connection signed in with a link token may address
    /// (`Claims::host`, link_tickets.rs).
    pub client_scopes: RwLock<HashMap<String, (String, Option<String>)>>,
    /// Unknown link tickets per client address.
    pub link_ticket_limit: FailureLimiter,
}

impl SignalingState {
    pub fn new(db: SqlitePool, agent_token: String, turn: Option<Arc<crate::turn::TurnService>>) -> Self {
        Self {
            db,
            turn,
            agent_local_ips: RwLock::new(HashMap::new()),
            client_local_ips: RwLock::new(HashMap::new()),
            agent_token,
            agents: RwLock::new(HashMap::new()),
            clients: RwLock::new(HashMap::new()),
            client_to_agent: RwLock::new(HashMap::new()),
            agent_connections: RwLock::new(HashMap::new()),
            client_connections: RwLock::new(HashMap::new()),
            file_sessions: RwLock::new(HashMap::new()),
            client_asked: RwLock::new(std::collections::HashSet::new()),
            local_sessions: RwLock::new(HashMap::new()),
            links: crate::links::Links::default(),
            gateway: crate::gateway::Gateway::from_env(),
            sources: crate::sources::Registry::default(),
            host_settings: crate::host_settings::Pending::default(),
            login_ip_limit: FailureLimiter::standard(),
            login_user_limit: FailureLimiter::standard(),
            // Higher: a stale agent retrying every few seconds must not lock
            // out every agent behind its address; the token is long and random.
            agent_token_limit: FailureLimiter::new(300, std::time::Duration::from_secs(300), 10_000),
            mfa: crate::mfa::MfaState::default(),
            client_scopes: RwLock::new(HashMap::new()),
            link_ticket_limit: FailureLimiter::standard(),
        }
    }

    /// Closes every client WebSocket of a user (deactivated or deleted).
    pub fn close_user_connections(&self, user_id: &str) {
        let conn_ids: Vec<String> = self
            .client_connections
            .read()
            .unwrap()
            .iter()
            .filter(|(_, uid)| uid.as_str() == user_id)
            .map(|(conn, _)| conn.clone())
            .collect();
        let clients = self.clients.read().unwrap();
        for conn in &conn_ids {
            if let Some(tx) = clients.get(conn) {
                tx.close();
            }
        }
        if !conn_ids.is_empty() {
            info!("Closed {} connection(s) of user {}", conn_ids.len(), user_id);
        }
    }

    fn send_to_conn(&self, conn_id: &str, msg: ServerToClientMessage) {
        if let Some(tx) = self.clients.read().unwrap().get(conn_id) {
            let _ = tx.send(msg);
        }
    }

    /// Whether agent `agent_id` may send to client connection `conn_id`: it
    /// has a session or file session with that agent, or asked it something.
    fn conn_talks_to(&self, conn_id: &str, agent_id: &str) -> bool {
        self.client_to_agent.read().unwrap().get(conn_id).is_some_and(|a| a == agent_id)
            || self.file_sessions.read().unwrap().get(conn_id).is_some_and(|a| a == agent_id)
            || self
                .client_asked
                .read()
                .unwrap()
                .contains(&(conn_id.to_string(), agent_id.to_string()))
    }

    /// A user's name, shown to the person at a computer asked to accept
    /// that user's connection.
    pub async fn username(&self, user_id: &str) -> Option<String> {
        sqlx::query_scalar("SELECT username FROM users WHERE id = ?")
            .bind(user_id)
            .fetch_optional(&self.db)
            .await
            .ok()
            .flatten()
    }

    pub async fn set_host_status(&self, host_id: &str, status: HostStatus) {
        let status_str = match status {
            HostStatus::Online => "Online",
            HostStatus::Offline => "Offline",
            HostStatus::Busy => "Busy",
        };
        let _ = sqlx::query("UPDATE hosts SET status = ? WHERE id = ?")
            .bind(status_str)
            .bind(host_id)
            .execute(&self.db)
            .await;
    }

    pub async fn register_host_db(
        &self,
        host_id: &str,
        host_name: &str,
        codec_support: Option<u32>,
    ) {
        let _ = sqlx::query(
            "INSERT INTO hosts (id, name, status, server_codec_mode_support) VALUES (?, ?, 'Online', ?)
             ON CONFLICT(id) DO UPDATE SET name = excluded.name, status = 'Online', server_codec_mode_support = excluded.server_codec_mode_support;"
        )
        .bind(host_id)
        .bind(host_name)
        .bind(codec_support.map(|c| c as i64))
        .execute(&self.db)
        .await;
    }

    /// ICE servers for an agent: the configured ones plus the built-in relay
    /// at the address the agent reaches the server on.
    pub async fn ice_servers_for_agent(&self, agent_id: &str) -> Vec<common::RtcIceServer> {
        let mut servers = self.fetch_ice_servers().await;
        if let Some(turn) = &self.turn {
            let agent_ip = self.agent_local_ips.read().unwrap().get(agent_id).copied();
            servers.extend(turn.ice_server_for(agent_ip, None));
        }
        servers
    }

    /// ICE servers for a client connection; falls back to the agent's
    /// address for the relay when the client came in on loopback.
    pub async fn ice_servers_for_client(&self, conn_id: &str, agent_id: &str) -> Vec<common::RtcIceServer> {
        let mut servers = self.fetch_ice_servers().await;
        if let Some(turn) = &self.turn {
            let client_ip = self.client_local_ips.read().unwrap().get(conn_id).copied();
            let agent_ip = self.agent_local_ips.read().unwrap().get(agent_id).copied();
            servers.extend(turn.ice_server_for(client_ip, agent_ip));
        }
        servers
    }

    pub async fn fetch_ice_servers(&self) -> Vec<common::RtcIceServer> {
        let rows: Result<Vec<(String, Option<String>, Option<String>)>, _> =
            sqlx::query_as("SELECT urls, username, credential FROM turn_servers")
                .fetch_all(&self.db)
                .await;

        match rows {
            Ok(rows) if !rows.is_empty() => rows
                .into_iter()
                .map(|(urls, username, credential)| {
                    let urls_vec = urls
                        .split(',')
                        .map(|s| s.trim().to_string())
                        .filter(|s| !s.is_empty())
                        .collect();
                    common::RtcIceServer {
                        urls: urls_vec,
                        username,
                        credential,
                    }
                })
                .collect(),
            _ => {
                vec![
                    common::RtcIceServer {
                        urls: vec!["stun:stun.l.google.com:19302".to_string()],
                        username: None,
                        credential: None,
                    },
                    common::RtcIceServer {
                        urls: vec!["stun:stun1.l.google.com:19302".to_string()],
                        username: None,
                        credential: None,
                    },
                ]
            }
        }
    }
}

// WS Agent query params
#[derive(serde::Deserialize)]
pub struct AgentParams {
    pub id: String,
    pub name: String,
    pub codec_support: Option<u32>,
    pub token: Option<String>,
    /// The agent's own key (hex): once a host id has one, only that agent may
    /// register with the id.
    pub key: Option<String>,
    /// An install token (install.rs), sent instead of `token` by an agent
    /// joining with the one-line installer.
    pub install_token: Option<String>,
}

// WS Client query params
#[derive(serde::Deserialize)]
pub struct ClientParams {
    pub token: String,
}

pub async fn agent_ws_handler(
    ws: WebSocketUpgrade,
    Query(params): Query<AgentParams>,
    ConnectInfo(addrs): ConnectInfo<ConnAddrs>,
    headers: axum::http::HeaderMap,
    State(state): State<Arc<SignalingState>>,
) -> impl IntoResponse {
    let now = std::time::Instant::now();
    let client_ip = security::client_ip(addrs.remote.ip(), &headers);
    let ip_key = client_ip.to_string();
    if state.agent_token_limit.is_blocked(&ip_key, now) {
        return StatusCode::TOO_MANY_REQUESTS.into_response();
    }
    let incoming_token = params.token.as_deref().unwrap_or("").trim();
    if incoming_token.is_empty() {
        // Without the server's agent token: an install token, or the key
        // alone of a computer that joined with one (install.rs).
        return match agent_without_token(&state, &params, &ip_key, now).await {
            Ok(()) => {
                if let Some(local) = addrs.local {
                    state.agent_local_ips.write().unwrap().insert(params.id.clone(), local.ip());
                }
                ws.max_message_size(AGENT_MAX_MESSAGE)
                    .on_upgrade(move |socket| handle_agent_socket(socket, params, state))
            }
            Err(code) => code.into_response(),
        };
    }
    if !security::ct_eq(incoming_token, state.agent_token.trim()) {
        state.agent_token_limit.record_failure(&ip_key, now);
        warn!(
            "Unauthorized agent connection attempt from {} (invalid or missing token)",
            client_ip
        );
        return StatusCode::UNAUTHORIZED.into_response();
    }
    if let Err(why) = security::validate_agent_identity(&params.id, &params.name) {
        warn!("Agent connection from {} refused: {}", client_ip, why);
        return StatusCode::BAD_REQUEST.into_response();
    }
    let key = params.key.as_deref().map(str::trim).filter(|k| !k.is_empty());
    if key.is_some_and(|k| !security::valid_agent_key(k)) {
        warn!("Agent {} sent a malformed key", params.id);
        return StatusCode::BAD_REQUEST.into_response();
    }
    match check_agent_key(&state, &params.id, &params.name, key).await {
        Ok(true) => {}
        Ok(false) => {
            warn!(
                "Agent connection from {} refused: wrong key for host id {} (or the id is in use)",
                client_ip,
                params.id
            );
            return StatusCode::UNAUTHORIZED.into_response();
        }
        Err(e) => {
            error!("Database error checking agent key: {}", e);
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
    }

    if let Some(local) = addrs.local {
        state.agent_local_ips.write().unwrap().insert(params.id.clone(), local.ip());
    }
    ws.max_message_size(AGENT_MAX_MESSAGE)
        .on_upgrade(move |socket| handle_agent_socket(socket, params, state))
}

/// Registration with an install token or the key alone (install.rs
/// `authorize`); failures count against the address like wrong agent tokens.
async fn agent_without_token(
    state: &SignalingState,
    params: &AgentParams,
    ip_key: &str,
    now: std::time::Instant,
) -> Result<(), StatusCode> {
    if let Err(why) = security::validate_agent_identity(&params.id, &params.name) {
        warn!("Agent connection from {} refused: {}", ip_key, why);
        return Err(StatusCode::BAD_REQUEST);
    }
    let key = params.key.as_deref().map(str::trim).unwrap_or("");
    if !security::valid_agent_key(key) {
        state.agent_token_limit.record_failure(ip_key, now);
        warn!("Unauthorized agent connection attempt from {} (no token and no valid key)", ip_key);
        return Err(StatusCode::UNAUTHORIZED);
    }
    let install_token = params.install_token.as_deref().map(str::trim).filter(|t| !t.is_empty());
    let keyless_online = state.agents.read().unwrap().contains_key(&params.id);
    match crate::install::authorize(&state.db, &params.id, &params.name, key, install_token, keyless_online).await {
        Ok(crate::install::Outcome::Accepted) => Ok(()),
        Ok(crate::install::Outcome::Enrolled { counted }) => {
            info!(
                "Agent {} joined with an install token{}",
                params.id,
                if counted { "" } else { " (already known: no use counted)" }
            );
            Ok(())
        }
        Ok(crate::install::Outcome::Rejected(why)) => {
            state.agent_token_limit.record_failure(ip_key, now);
            warn!("Agent connection from {} for host id {} refused: {}", ip_key, params.id, why);
            Err(StatusCode::UNAUTHORIZED)
        }
        Err(e) => {
            error!("Database error checking an agent: {}", e);
            Err(StatusCode::INTERNAL_SERVER_ERROR)
        }
    }
}

/// Whether an agent may register as `host_id`: a host that has a key accepts
/// only that key; the first key given for a host is stored.
async fn check_agent_key(
    state: &SignalingState,
    host_id: &str,
    name: &str,
    key: Option<&str>,
) -> Result<bool, sqlx::Error> {
    let stored: Option<String> = sqlx::query_scalar("SELECT agent_key_hash FROM hosts WHERE id = ?")
        .bind(host_id)
        .fetch_optional(&state.db)
        .await?
        .flatten();
    match security::agent_key_decision(stored.as_deref(), key) {
        AgentKeyDecision::Accept => Ok(true),
        AgentKeyDecision::Reject => Ok(false),
        AgentKeyDecision::Store => {
            // An agent without a key is online with this id: a newcomer with
            // the shared token must not claim the id and lock it out.
            if state.agents.read().unwrap().contains_key(host_id) {
                return Ok(false);
            }
            let hash = security::sha256_hex(key.unwrap_or_default());
            // Only fills an empty slot, so two agents racing for a new id
            // can't both win: re-read what was kept.
            sqlx::query(
                "INSERT INTO hosts (id, name, status, agent_key_hash) VALUES (?, ?, 'Offline', ?)
                 ON CONFLICT(id) DO UPDATE SET agent_key_hash = excluded.agent_key_hash
                 WHERE hosts.agent_key_hash IS NULL",
            )
            .bind(host_id)
            .bind(name)
            .bind(&hash)
            .execute(&state.db)
            .await?;
            let kept: Option<String> = sqlx::query_scalar("SELECT agent_key_hash FROM hosts WHERE id = ?")
                .bind(host_id)
                .fetch_optional(&state.db)
                .await?
                .flatten();
            if kept.as_deref().is_some_and(|k| security::ct_eq(k, &hash)) {
                info!("Stored the key of agent {}", host_id);
                Ok(true)
            } else {
                Ok(false)
            }
        }
    }
}

pub async fn client_ws_handler(
    ws: WebSocketUpgrade,
    Query(params): Query<ClientParams>,
    ConnectInfo(addrs): ConnectInfo<ConnAddrs>,
    State(state): State<Arc<SignalingState>>,
) -> impl IntoResponse {
    let Ok(claims) = verify_jwt(&params.token) else {
        return StatusCode::UNAUTHORIZED.into_response();
    };
    // A valid token is not enough: the account must still exist and be
    // active, and the token be from after the last password change.
    match crate::auth::check_session(&state.db, &claims).await {
        Ok(crate::auth::SessionCheck::Ok(_)) => {}
        Ok(_) => return StatusCode::UNAUTHORIZED.into_response(),
        Err(e) => {
            error!("Database error checking user: {}", e);
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
    }
    let open = state
        .client_connections
        .read()
        .unwrap()
        .values()
        .filter(|uid| **uid == claims.sub)
        .count();
    if open >= MAX_CLIENT_SOCKETS_PER_USER {
        warn!("User {} has too many open connections", claims.sub);
        return StatusCode::TOO_MANY_REQUESTS.into_response();
    }
    let local_ip = addrs.local.map(|a| a.ip());
    ws.max_message_size(CLIENT_MAX_MESSAGE)
        .on_upgrade(move |socket| handle_client_socket(socket, claims.sub, claims.host.map(|h| (h, claims.source)), local_ip, state))
}

// --- Agent Connection Handler ---

/// An agent connected (to /ws/agent, or the server to it: links.rs): it
/// becomes the host's connection and shows online. Messages for it arrive on
/// the returned queue; the id tells `agent_disconnected` which connection ended.
pub(crate) async fn agent_connected(
    state: &Arc<SignalingState>,
    agent_id: &str,
    agent_name: &str,
    codec_support: Option<u32>,
) -> (Uuid, Inbox<ServerToAgentMessage>) {
    let (conn_id, rx) = add_agent_connection(state, agent_id, false).expect("replacing is always possible");
    state.register_host_db(agent_id, agent_name, codec_support).await;
    info!("Agent registered: {} ({})", agent_name, agent_id);
    (conn_id, rx)
}

/// Makes a new connection the host's; with `only_if_free`, not when the host
/// already has one (checked and done under the same locks). Lock order as in
/// `agent_disconnected`: agent_connections, then agents.
fn add_agent_connection(
    state: &SignalingState,
    agent_id: &str,
    only_if_free: bool,
) -> Option<(Uuid, Inbox<ServerToAgentMessage>)> {
    let mut conn_map = state.agent_connections.write().unwrap();
    let mut agents = state.agents.write().unwrap();
    if only_if_free && (agents.contains_key(agent_id) || conn_map.contains_key(agent_id)) {
        return None;
    }
    let (tx, rx) = crate::queue::channel::<ServerToAgentMessage>();
    let conn_id = Uuid::new_v4();
    agents.insert(agent_id.to_string(), tx);
    conn_map.insert(agent_id.to_string(), conn_id);
    Some((conn_id, rx))
}

/// Whether a computer added by address (links.rs) may register as `id`. The
/// id is only what the remote end says, so it must not take over another
/// computer's: the link keeps the id it first registered with (`known`), a
/// new link only an id the server doesn't know, and never one whose agent
/// proves itself with its own key. Err: why not, for the admin.
pub(crate) async fn link_may_register(
    state: &SignalingState,
    id: &str,
    name: &str,
    known: Option<&str>,
) -> Result<(), String> {
    if let Err(why) = security::validate_agent_identity(id, name) {
        return Err(format!("refused: {why}"));
    }
    if id.starts_with("address:") {
        return Err("refused: not a valid computer id".into());
    }
    if let Some(known) = known.filter(|k| *k != id) {
        return Err(format!(
            "this address now answers as another computer ({id}, it was {known}); remove it and add it again if that is right"
        ));
    }
    let row: Option<Option<String>> = sqlx::query_scalar("SELECT agent_key_hash FROM hosts WHERE id = ?")
        .bind(id)
        .fetch_optional(&state.db)
        .await
        .map_err(|e| {
            error!("Database error checking a computer added by address: {}", e);
            "server database error".to_string()
        })?;
    match row {
        // Its agent signs in with its own key, which a link can't show.
        Some(Some(_)) => Err(format!(
            "the computer {id} already signs in to this server with its own key; it cannot also be added by address"
        )),
        Some(None) if known.is_none() => Err(format!(
            "a computer with the id {id} is already known to this server; remove it first to add it by address"
        )),
        _ => Ok(()),
    }
}

/// A computer added by address connected (links.rs): like `agent_connected`,
/// but never in place of another live connection. None: the host has one.
pub(crate) async fn link_connected(
    state: &Arc<SignalingState>,
    agent_id: &str,
    agent_name: &str,
    codec_support: Option<u32>,
) -> Option<(Uuid, Inbox<ServerToAgentMessage>)> {
    let (conn_id, rx) = add_agent_connection(state, agent_id, true)?;
    state.register_host_db(agent_id, agent_name, codec_support).await;
    info!("Agent registered (added by address): {} ({})", agent_name, agent_id);
    Some((conn_id, rx))
}

/// Ends the sessions and file sessions client connections have with a host
/// and forgets their questions to it; the clients get `message`. File
/// connections (one WebSocket each) are closed. Returns the stream
/// connections whose session ended.
fn end_host_sessions(state: &SignalingState, host_id: &str, message: &str) -> Vec<String> {
    let streams: Vec<String> = {
        let mut map = state.client_to_agent.write().unwrap();
        let conns: Vec<String> = map.iter().filter(|(_, h)| h.as_str() == host_id).map(|(c, _)| c.clone()).collect();
        for conn in &conns {
            map.remove(conn);
        }
        conns
    };
    let files: Vec<String> = {
        let mut map = state.file_sessions.write().unwrap();
        let conns: Vec<String> = map.iter().filter(|(_, h)| h.as_str() == host_id).map(|(c, _)| c.clone()).collect();
        for conn in &conns {
            map.remove(conn);
        }
        conns
    };
    state.client_asked.write().unwrap().retain(|(_, h)| h != host_id);
    for conn in &streams {
        state.send_to_conn(conn, ServerToClientMessage::Signaling(SignalingMessage::Error { message: message.to_string() }));
    }
    let clients = state.clients.read().unwrap();
    for conn in &files {
        if let Some(tx) = clients.get(conn) {
            let _ = tx.send(ServerToClientMessage::Signaling(SignalingMessage::EndSession { target_id: host_id.to_string() }));
            tx.close();
        }
    }
    streams
}

/// Disconnects the agent connected as `host_id` now (the host was removed,
/// or its key reset) and ends its clients' sessions.
pub(crate) async fn disconnect_agent(state: &Arc<SignalingState>, host_id: &str) {
    let agent_tx = {
        let mut conn_map = state.agent_connections.write().unwrap();
        let mut agents = state.agents.write().unwrap();
        conn_map.remove(host_id);
        agents.remove(host_id)
    };
    state.agent_local_ips.write().unwrap().remove(host_id);
    crate::sources::agent_gone(state, host_id);
    if let Some(tx) = &agent_tx {
        // Its socket task sees the queue close and ends the connection;
        // `agent_disconnected` then finds nothing left to do.
        tx.close();
        info!("Disconnected the agent of host {}", host_id);
    }
    end_host_sessions(state, host_id, "Host disconnected");
    state.set_host_status(host_id, HostStatus::Offline).await;
}

/// After an access change (group members or computers, a role, a deleted
/// group): every session, file session and open question of a client
/// connection whose user may no longer use that host ends now. The agent is
/// told to end it, and the connection is closed.
pub(crate) async fn end_sessions_without_access(state: &Arc<SignalingState>) {
    let owners: HashMap<String, String> = state.client_connections.read().unwrap().clone();
    let streams: Vec<(String, String)> =
        state.client_to_agent.read().unwrap().iter().map(|(c, h)| (c.clone(), h.clone())).collect();
    let files: Vec<(String, String)> =
        state.file_sessions.read().unwrap().iter().map(|(c, h)| (c.clone(), h.clone())).collect();
    let asked: Vec<(String, String)> = state.client_asked.read().unwrap().iter().cloned().collect();

    let mut access: HashMap<String, security::HostAccess> = HashMap::new();
    let mut lost: Vec<(String, String, &'static str)> = Vec::new();
    for (list, kind) in [(&streams, "stream"), (&files, "file"), (&asked, "asked")] {
        for (conn, host) in list.iter() {
            let Some(user) = owners.get(conn) else { continue };
            if !access.contains_key(user) {
                // Fails closed: a database error ends the session.
                let a = security::host_access(&state.db, user).await.unwrap_or_else(|e| {
                    error!("Database error checking host access: {}", e);
                    security::HostAccess::None
                });
                access.insert(user.clone(), a);
            }
            if !access[user].allows(host) {
                lost.push((conn.clone(), host.clone(), kind));
            }
        }
    }

    for (conn, host, kind) in lost {
        let ended = match kind {
            "stream" => {
                let mut map = state.client_to_agent.write().unwrap();
                let ours = map.get(&conn).is_some_and(|h| *h == host);
                if ours {
                    map.remove(&conn);
                }
                ours
            }
            "file" => {
                let mut map = state.file_sessions.write().unwrap();
                let ours = map.get(&conn).is_some_and(|h| *h == host);
                if ours {
                    map.remove(&conn);
                }
                ours
            }
            _ => {
                state.client_asked.write().unwrap().remove(&(conn.clone(), host.clone()));
                false
            }
        };
        if !ended {
            continue;
        }
        info!("Access to host {} was removed: ending the {} session of connection {}", host, kind, conn);
        if let Some(agent_tx) = state.agents.read().unwrap().get(&host).cloned() {
            let _ = agent_tx.send(ServerToAgentMessage::Signaling(SignalingMessage::EndSession { target_id: conn.clone() }));
        }
        if kind == "stream" {
            state.set_host_status(&host, HostStatus::Online).await;
        }
        if let Some(tx) = state.clients.read().unwrap().get(&conn) {
            let _ = tx.send(ServerToClientMessage::Signaling(SignalingMessage::Error {
                message: "Your access to this computer was removed.".to_string(),
            }));
            tx.close();
        }
    }
}

/// One message from a connected agent.
pub(crate) async fn agent_message(state: &Arc<SignalingState>, agent_id: &str, msg: AgentMessage) {
    match msg {
        AgentMessage::Register { .. } => {
            // Handled upon connection
        }
        AgentMessage::StatusUpdate { status } => {
            state.set_host_status(agent_id, status).await;
        }
        AgentMessage::Signaling(sig) => {
            handle_agent_signaling(sig, agent_id, state.clone()).await;
        }
        AgentMessage::Sources { sources, connectors, manageable, addresses } => {
            crate::sources::addresses_announced(state, agent_id, addresses).await;
            crate::sources::announced(state, agent_id, sources, connectors, manageable).await;
        }
        AgentMessage::SourceChanged { request_id, id, error, setup } => {
            crate::sources::changed(state, agent_id, &request_id, id, error, setup);
        }
        AgentMessage::SettingsAnswer { request_id, settings, password, error, code } => {
            crate::host_settings::answered(state, agent_id, &request_id, settings, password, error, code);
        }
    }
}

/// The agent connection `conn_id` ended: unless a newer one replaced it, the
/// host goes offline and its clients are told.
pub(crate) async fn agent_disconnected(state: &Arc<SignalingState>, agent_id: &str, conn_id: Uuid) {
    info!("Agent disconnected: {}", agent_id);
    let mut should_cleanup = false;
    {
        let mut conn_map = state.agent_connections.write().unwrap();
        if conn_map.get(agent_id) == Some(&conn_id) {
            conn_map.remove(agent_id);
            state.agents.write().unwrap().remove(agent_id);
            state.agent_local_ips.write().unwrap().remove(agent_id);
            crate::sources::agent_gone(state, agent_id);
            should_cleanup = true;
        }
    }
    if !should_cleanup {
        return;
    }
    state.set_host_status(agent_id, HostStatus::Offline).await;

    // Tell the connections that had a session with this agent.
    end_host_sessions(state, agent_id, "Host disconnected");
}

async fn handle_agent_socket(socket: WebSocket, params: AgentParams, state: Arc<SignalingState>) {
    let (mut ws_sender, mut ws_receiver) = socket.split();
    let agent_id = params.id.clone();
    let (conn_id, mut rx) = agent_connected(&state, &agent_id, &params.name, params.codec_support).await;

    // Spawn a task to handle outbound messages to the agent (with heartbeat to prevent idle timeouts)
    let agent_id_clone = agent_id.clone();
    let mut send_task = tokio::spawn(async move {
        let mut ping_interval = tokio::time::interval(std::time::Duration::from_secs(15));
        ping_interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tokio::select! {
                maybe_msg = rx.recv() => {
                    match maybe_msg {
                        Some(msg) => {
                            let json = match serde_json::to_string(&msg) {
                                Ok(j) => j,
                                Err(e) => {
                                    error!("Failed to serialize message to agent: {}", e);
                                    continue;
                                }
                            };
                            if let Err(e) = ws_sender.send(Message::Text(json.into())).await {
                                debug!("Error sending to agent {}: {}", agent_id_clone, e);
                                break;
                            }
                        }
                        None => break,
                    }
                }
                _ = ping_interval.tick() => {
                    if let Err(e) = ws_sender.send(Message::Ping(Vec::new().into())).await {
                        debug!("Error sending ping to agent {}: {}", agent_id_clone, e);
                        break;
                    }
                }
            }
        }
    });

    // Handle inbound messages from agent, until it or the sender (queue full) stops
    loop {
        let next = tokio::select! {
            next = ws_receiver.next() => next,
            _ = &mut send_task => break,
        };
        let Some(result) = next else { break };
        let msg = match result {
            Ok(m) => m,
            Err(e) => {
                warn!("WebSocket read error from agent {}: {}", agent_id, e);
                break;
            }
        };
        if let Message::Text(text) = msg {
            match serde_json::from_str::<AgentMessage>(&text) {
                Ok(agent_msg) => agent_message(&state, &agent_id, agent_msg).await,
                Err(e) => error!("Failed to parse agent message: {}", e),
            }
        }
    }

    agent_disconnected(&state, &agent_id, conn_id).await;
    send_task.abort();
}

// --- Client Connection Handler ---
async fn handle_client_socket(
    socket: WebSocket,
    client_id: String,
    // The one host (and source) it may address: a token from a link ticket.
    scope: Option<(String, Option<String>)>,
    local_ip: Option<IpAddr>,
    state: Arc<SignalingState>,
) {
    let (mut ws_sender, mut ws_receiver) = socket.split();
    let (tx, mut rx) = crate::queue::channel::<ServerToClientMessage>();
    let conn_id = Uuid::new_v4();

    // Register active client sender (keyed by conn_id to support multiple connections per client)
    let conn_id_str = conn_id.to_string();
    {
        state.clients.write().unwrap().insert(conn_id_str.clone(), tx);
        state
            .client_connections
            .write()
            .unwrap()
            .insert(conn_id_str.clone(), client_id.clone());
    }
    if let Some(ip) = local_ip {
        state.client_local_ips.write().unwrap().insert(conn_id_str.clone(), ip);
    }
    if let Some(scope) = scope {
        state.client_scopes.write().unwrap().insert(conn_id_str.clone(), scope);
    }
    info!("Client WebSocket connected: {} (conn: {})", client_id, conn_id_str);

    // Spawn a task to handle outbound messages to the client (with heartbeat to prevent idle timeouts)
    let client_id_clone = client_id.clone();
    let mut send_task = tokio::spawn(async move {
        let mut ping_interval = tokio::time::interval(std::time::Duration::from_secs(15));
        ping_interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tokio::select! {
                maybe_msg = rx.recv() => {
                    match maybe_msg {
                        Some(msg) => {
                            let json = match serde_json::to_string(&msg) {
                                Ok(j) => j,
                                Err(e) => {
                                    error!("Failed to serialize message to client: {}", e);
                                    continue;
                                }
                            };
                            if let Err(e) = ws_sender.send(Message::Text(json.into())).await {
                                debug!("Error sending to client {}: {}", client_id_clone, e);
                                break;
                            }
                        }
                        None => break,
                    }
                }
                _ = ping_interval.tick() => {
                    if let Err(e) = ws_sender.send(Message::Ping(Vec::new().into())).await {
                        debug!("Error sending ping to client {}: {}", client_id_clone, e);
                        break;
                    }
                }
            }
        }
    });

    // Handle inbound messages from client, until it or the sender (queue full,
    // user deactivated) stops
    loop {
        let next = tokio::select! {
            next = ws_receiver.next() => next,
            _ = &mut send_task => break,
        };
        let Some(result) = next else { break };
        let msg = match result {
            Ok(m) => m,
            Err(e) => {
                warn!("WebSocket read error from client {} (conn: {}): {}", client_id, conn_id, e);
                break;
            }
        };

        match &msg {
            Message::Text(_) => {},
            Message::Close(frame) => {
                info!("Client {} (conn: {}) sent close frame: {:?}", client_id, conn_id, frame);
                break;
            }
            _ => {
                debug!("Client {} (conn: {}) sent non-text message: {:?}", client_id, conn_id, msg);
            }
        }

        if let Message::Text(text) = msg {
            let client_msg: ClientMessage = match serde_json::from_str(&text) {
                Ok(m) => m,
                Err(e) => {
                    error!("Failed to parse client message from {} (conn: {}): {}", client_id, conn_id, e);
                    continue;
                }
            };

            match client_msg {
                ClientMessage::Signaling(sig) => {
                    handle_client_signaling(sig, &client_id, &conn_id.to_string(), state.clone()).await;
                }
            }
        }
    }

    // Connection closed
    info!("Client WebSocket disconnected: {} (conn: {})", client_id, conn_id);
    // Remove this specific connection
    state.clients.write().unwrap().remove(&conn_id_str);
    state.client_connections.write().unwrap().remove(&conn_id_str);
    state.client_local_ips.write().unwrap().remove(&conn_id_str);
    state.client_scopes.write().unwrap().remove(&conn_id_str);
    state.client_asked.write().unwrap().retain(|(c, _)| *c != conn_id_str);

    // A file-transfer connection: its agent closes that connection only.
    let file_host = state.file_sessions.write().unwrap().remove(&conn_id_str);
    if let Some(host_id) = file_host {
        if let Some(agent_tx) = state.agents.read().unwrap().get(&host_id).cloned() {
            let _ = agent_tx.send(ServerToAgentMessage::Signaling(SignalingMessage::EndSession {
                target_id: conn_id_str.clone(),
            }));
        }
        return;
    }

    // The session started on this connection ends with it (agents know the
    // client by its conn_id).
    {
        let host_id_opt = state.client_to_agent.write().unwrap().remove(&conn_id_str);
        if let Some(host_id) = host_id_opt {
            info!(
                "Cleaning up active session for host {} due to client disconnect",
                host_id
            );
            // Send EndSession to the Agent if agent is registered
            let agent_tx_opt = {
                let agents = state.agents.read().unwrap();
                agents.get(&host_id).cloned()
            };
            if let Some(agent_tx) = agent_tx_opt {
                let _ = agent_tx.send(ServerToAgentMessage::Signaling(
                    SignalingMessage::EndSession {
                        target_id: conn_id_str.clone(),
                    },
                ));
            }

            let session = {
                let mut sessions = state.local_sessions.write().unwrap();
                sessions.remove(&host_id)
            };
            if let Some(session) = session {
                let _ = session.peer_connection.close().await;
            }
            state.set_host_status(&host_id, HostStatus::Online).await;
        }
    }

    send_task.abort();
}

// --- Route Signaling from Agent -> Client ---
async fn handle_agent_signaling(sig: SignalingMessage, agent_id: &str, state: Arc<SignalingState>) {
    // Only to a connection that has something to do with this agent: no
    // agent can push messages into other users' connections.
    if let Some(target) = agent_reply_target(&sig) {
        if !state.conn_talks_to(target, agent_id) {
            debug!("Agent {} wrote to connection {} it has nothing to do with; dropped", agent_id, target);
            return;
        }
    }
    match sig {
        SignalingMessage::Sdp {
            target_id,
            sdp,
            ice_servers: _,
            webtransport_port,
            webtransport_cert_hash,
            webtransport_token,
        } => {
            let client_tx_opt = state.clients.read().unwrap().get(&target_id).cloned();
            if let Some(client_tx) = client_tx_opt {
                let ice_servers = state.ice_servers_for_client(&target_id, agent_id).await;
                let _ = client_tx.send(ServerToClientMessage::Signaling(SignalingMessage::Sdp {
                    target_id: agent_id.to_string(),
                    sdp,
                    ice_servers: Some(ice_servers),
                    webtransport_port,
                    webtransport_cert_hash,
                    webtransport_token,
                }));
            }
        }
        SignalingMessage::IceCandidate {
            target_id,
            candidate,
        } => {
            if let Some(client_tx) = state.clients.read().unwrap().get(&target_id) {
                let _ = client_tx.send(ServerToClientMessage::Signaling(
                    SignalingMessage::IceCandidate {
                        target_id: agent_id.to_string(),
                        candidate,
                    },
                ));
            }
        }
        SignalingMessage::EndSession { target_id } => {
            // Only for a connection that has a session with this agent.
            let stream = {
                let mut map = state.client_to_agent.write().unwrap();
                let ours = map.get(&target_id).is_some_and(|a| a == agent_id);
                if ours {
                    map.remove(&target_id);
                }
                ours
            };
            let file = {
                let mut map = state.file_sessions.write().unwrap();
                let ours = map.get(&target_id).is_some_and(|a| a == agent_id);
                if ours {
                    map.remove(&target_id);
                }
                ours
            };
            if !stream && !file {
                debug!("Agent {} ended a session it does not have ({}); ignored", agent_id, target_id);
                return;
            }
            state.send_to_conn(
                &target_id,
                ServerToClientMessage::Signaling(SignalingMessage::EndSession {
                    target_id: agent_id.to_string(),
                }),
            );
            if stream {
                state.set_host_status(agent_id, HostStatus::Online).await;
            }
        }

        SignalingMessage::AppListResponse {
            target_id,
            apps,
            current_game_id,
        } => {
            if let Some(client_tx) = state.clients.read().unwrap().get(&target_id) {
                let _ = client_tx.send(ServerToClientMessage::Signaling(
                    SignalingMessage::AppListResponse {
                        target_id: agent_id.to_string(),
                        apps,
                        current_game_id,
                    },
                ));
            }
        }
        SignalingMessage::StopActiveStreamResponse {
            target_id,
            success,
            error,
        } => {
            if let Some(client_tx) = state.clients.read().unwrap().get(&target_id) {
                let _ = client_tx.send(ServerToClientMessage::Signaling(
                    SignalingMessage::StopActiveStreamResponse {
                        target_id: agent_id.to_string(),
                        success,
                        error,
                    },
                ));
            }
        }
        SignalingMessage::CapabilitiesResponse { target_id, displays, encoders, gpu_info, host_os, sources } => {
            info!("Server: CapabilitiesResponse from agent {} for client {} ({} displays, {} encoders)", agent_id, target_id, displays.len(), encoders.len());
            let clients = state.clients.read().unwrap();
            if let Some(client_tx) = clients.get(&target_id) {
                info!("Server: Found client {} in clients map, forwarding CapabilitiesResponse", target_id);
                let msg = ServerToClientMessage::Signaling(SignalingMessage::CapabilitiesResponse {
                    target_id: agent_id.to_string(),
                    displays,
                    encoders,
                    gpu_info,
                    host_os,
                    sources,
                });
                let _ = client_tx.send(msg);
            } else {
                warn!("Server: Client {} NOT found in clients map! Cannot forward CapabilitiesResponse", target_id);
            }
        }
        SignalingMessage::SourceStatus { target_id, status, reason } => {
            if let Some(client_tx) = state.clients.read().unwrap().get(&target_id) {
                let _ = client_tx.send(ServerToClientMessage::Signaling(SignalingMessage::SourceStatus {
                    target_id: agent_id.to_string(),
                    status,
                    reason,
                }));
            }
        }
        SignalingMessage::AccessRequest { target_id, status } => {
            if let Some(client_tx) = state.clients.read().unwrap().get(&target_id) {
                let _ = client_tx.send(ServerToClientMessage::Signaling(SignalingMessage::AccessRequest {
                    target_id: agent_id.to_string(),
                    status,
                }));
            }
        }
        SignalingMessage::EncoderStatus { target_id, encoder, hw_type, gpu_info, requested_encoder, host_os, display_id, display_name } => {
            let clients = state.clients.read().unwrap();
            if let Some(client_tx) = clients.get(&target_id) {
                let msg = ServerToClientMessage::Signaling(SignalingMessage::EncoderStatus {
                    target_id: agent_id.to_string(),
                    encoder,
                    hw_type,
                    gpu_info,
                    requested_encoder,
                    host_os,
                    display_id,
                    display_name,
                });
                let _ = client_tx.send(msg);
            }
        }
        _ => {}
    }
}

// --- Route Signaling from Client -> Agent ---
async fn handle_client_signaling(
    sig: SignalingMessage,
    client_id: &str,
    conn_id: &str,
    state: Arc<SignalingState>,
) {
    if let Some(host_id) = addressed_host(&sig) {
        if !client_may_address(&state, client_id, conn_id, &sig, host_id).await {
            warn!("User {} refused access to host {}", client_id, host_id);
            state.send_to_conn(
                conn_id,
                ServerToClientMessage::Signaling(SignalingMessage::Error {
                    message: "You don't have access to this computer.".to_string(),
                }),
            );
            return;
        }
    }
    // Questions outside a session: the host's answers may come back here.
    if let SignalingMessage::GetAppList { target_id }
    | SignalingMessage::GetCapabilities { target_id }
    | SignalingMessage::StopActiveStream { target_id } = &sig
    {
        state.client_asked.write().unwrap().insert((conn_id.to_string(), target_id.clone()));
    }
    match sig {
        // A file-transfer connection (on its own client WebSocket): routed
        // like a session, without touching the host's streaming state.
        SignalingMessage::RequestFileSession { host_id, source_id } => {
            let agent_tx = state.agents.read().unwrap().get(&host_id).cloned();
            match agent_tx {
                Some(agent_tx) => {
                    info!("Client {} requested file transfer with host {}", client_id, host_id);
                    let ice_servers = state.ice_servers_for_agent(&host_id).await;
                    state.file_sessions.write().unwrap().insert(conn_id.to_string(), host_id.clone());
                    let requester = state.username(client_id).await;
                    let _ = agent_tx.send(ServerToAgentMessage::Signaling(SignalingMessage::IncomingFileSession {
                        client_id: conn_id.to_string(),
                        ice_servers: Some(ice_servers),
                        requester,
                        // The files of one of the host's sources, when named.
                        source_id,
                    }));
                }
                None => {
                    if let Some(client_tx) = state.clients.read().unwrap().get(conn_id) {
                        let _ = client_tx.send(ServerToClientMessage::Signaling(SignalingMessage::Error {
                            message: "No agent connected for this host. Please ensure the agent is running.".to_string(),
                        }));
                    }
                }
            }
        }
        SignalingMessage::EndSession { .. } if state.file_sessions.read().unwrap().contains_key(conn_id) => {
            let host_id = state.file_sessions.write().unwrap().remove(conn_id).unwrap_or_default();
            if let Some(agent_tx) = state.agents.read().unwrap().get(&host_id).cloned() {
                let _ = agent_tx.send(ServerToAgentMessage::Signaling(SignalingMessage::EndSession {
                    target_id: conn_id.to_string(),
                }));
            }
        }
        SignalingMessage::RequestSession {
            host_id,
            width,
            height,
            fps,
            bitrate,
            codec,
            app_id,
            encoder,
            display_id,
            chroma,
            encoder_csc_mode,
            microphone,
            source_id,
            decode_codecs,
        } => {
            info!("Client {} requested session for host {} with settings: w={:?}, h={:?}, fps={:?}, bitrate={:?}, codec={:?}, app_id={:?}, chroma={:?}, encoder_csc_mode={:?}", client_id, host_id, width, height, fps, bitrate, codec, app_id, chroma, encoder_csc_mode);

            let previous_host = state.client_to_agent.write().unwrap().remove(conn_id);
            let mut stopped_previous_agent_session = false;
            if let Some(previous_host_id) = previous_host {
                if let Some(agent_tx) = state.agents.read().unwrap().get(&previous_host_id).cloned() {
                    let _ = agent_tx.send(ServerToAgentMessage::Signaling(
                        SignalingMessage::EndSession {
                            target_id: conn_id.to_string(),
                        },
                    ));
                    stopped_previous_agent_session = true;
                    info!(
                        "Stopped previous session for client {} on host {} before starting a new session",
                        client_id, previous_host_id
                    );
                }
                state.set_host_status(&previous_host_id, HostStatus::Online).await;
            }
            if stopped_previous_agent_session {
                tokio::time::sleep(std::time::Duration::from_millis(1300)).await;
            }

            // Check if there is an active session for host_id, if so terminate it first
            let old_session = {
                let mut sessions = state.local_sessions.write().unwrap();
                sessions.remove(&host_id)
            };
            if let Some(session) = old_session {
                let _ = session.peer_connection.close().await;
            }

            // Check if there is an online Agent registered for this host_id
            let agent_tx_opt = {
                let agents = state.agents.read().unwrap();
                agents.get(&host_id).cloned()
            };

            if let Some(agent_tx) = agent_tx_opt {
                let ice_servers = state.ice_servers_for_agent(&host_id).await;
                let requester = state.username(client_id).await;
                let incoming_msg =
                    ServerToAgentMessage::Signaling(SignalingMessage::IncomingSession {
                        client_id: conn_id.to_string(),
                        width,
                        height,
                        fps,
                        bitrate,
                        codec: codec.clone(),
                        app_id,
                        encoder: encoder.clone(),
                        display_id: display_id.clone(),
                        chroma: chroma.clone(),
                        encoder_csc_mode,
                        microphone,
                        source_id: source_id.clone(),
                        decode_codecs: decode_codecs.clone(),
                        ice_servers: Some(ice_servers),
                        requester,
                    });
                // Client-to-host routing first, so the agent's first answer
                // finds it. The agent runs one session: it replaces any
                // other connection's, which then must not end it or the status.
                let replaced: Vec<String> = {
                    let mut map = state.client_to_agent.write().unwrap();
                    let others: Vec<String> =
                        map.iter().filter(|(c, h)| **h == host_id && c.as_str() != conn_id).map(|(c, _)| c.clone()).collect();
                    for c in &others {
                        map.remove(c);
                    }
                    map.insert(conn_id.to_string(), host_id.clone());
                    others
                };
                if let Err(e) = agent_tx.send(incoming_msg) {
                    error!(
                        "Failed to send IncomingSession to agent {}: {:?}",
                        host_id, e
                    );
                    let mut map = state.client_to_agent.write().unwrap();
                    map.remove(conn_id);
                    for c in replaced {
                        map.insert(c, host_id.clone());
                    }
                    // Fall through to fallback Agent-less code below
                } else {
                    state.set_host_status(&host_id, HostStatus::Busy).await;
                    info!(
                        "IncomingSession successfully forwarded to agent {} for client {}",
                        host_id, client_id
                    );
                    return;
                }
            }

            // No agent available for this host
            error!("No agent connected for host {}", host_id);
            if let Some(client_tx) = state.clients.read().unwrap().get(conn_id) {
                let _ = client_tx.send(ServerToClientMessage::Signaling(SignalingMessage::Error {
                    message: "No agent connected for this host. Please ensure the agent is running.".to_string(),
                }));
            }
        }
        SignalingMessage::Sdp {
            target_id,
            sdp,
            ice_servers: _,
            webtransport_port,
            webtransport_cert_hash,
            webtransport_token,
        } => {
            // target_id is the host_id
            info!(
                "Received SDP Answer from client {} for host {}",
                client_id, target_id
            );

            // Check if there is an online Agent registered for this host_id
            let agent_tx_opt = {
                let agents = state.agents.read().unwrap();
                agents.get(&target_id).cloned()
            };

            if let Some(agent_tx) = agent_tx_opt {
                // Forward the SDP Answer to the agent
                let _ = agent_tx.send(ServerToAgentMessage::Signaling(SignalingMessage::Sdp {
                    target_id: conn_id.to_string(),
                    sdp,
                    ice_servers: None,
                    webtransport_port,
                    webtransport_cert_hash,
                    webtransport_token,
                }));
                info!("SDP Answer forwarded to agent {}", target_id);
                return;
            }

            // Fallback: local session
            let session = {
                let sessions = state.local_sessions.read().unwrap();
                sessions.get(&target_id).cloned()
            };

            if let Some(session) = session {
                match RTCSessionDescription::answer(sdp.sdp) {
                    Ok(rtc_sdp) => {
                        if let Err(e) = session
                            .peer_connection
                            .set_remote_description(rtc_sdp)
                            .await
                        {
                            error!("Failed to set remote description: {:?}", e);
                        } else {
                            info!(
                                "SDP Answer set successfully on local session for host {}",
                                target_id
                            );
                        }
                    }
                    Err(e) => {
                        error!("Failed to parse SDP Answer: {:?}", e);
                    }
                }
            } else {
                warn!(
                    "Received SDP without active local session for host {}",
                    target_id
                );
            }
        }
        SignalingMessage::IceCandidate {
            target_id,
            candidate,
        } => {
            // target_id is the host_id

            // Check if there is an online Agent registered for this host_id
            let agent_tx_opt = {
                let agents = state.agents.read().unwrap();
                agents.get(&target_id).cloned()
            };

            if let Some(agent_tx) = agent_tx_opt {
                // Forward IceCandidate to the agent
                let _ = agent_tx.send(ServerToAgentMessage::Signaling(
                    SignalingMessage::IceCandidate {
                        target_id: conn_id.to_string(),
                        candidate,
                    },
                ));
                return;
            }

            // Fallback: local session
            let session = {
                let sessions = state.local_sessions.read().unwrap();
                sessions.get(&target_id).cloned()
            };

            if let Some(session) = session {
                let rtc_cand = RTCIceCandidateInit {
                    candidate: candidate.candidate,
                    sdp_mid: candidate.sdp_mid,
                    sdp_mline_index: candidate.sdp_mline_index,
                    username_fragment: candidate.username_fragment,
                    ..Default::default()
                };
                if let Err(e) = session.peer_connection.add_ice_candidate(rtc_cand).await {
                    debug!("Failed to add ICE candidate on local session: {:?}", e);
                }
            }
        }
        SignalingMessage::EndSession { target_id } => {
            // target_id is the host_id
            info!(
                "Session ended by client {} for host {}",
                client_id, target_id
            );

            // Only the connection that holds the host's session ends it;
            // otherwise another stream on that host would be marked idle.
            let ours = {
                let mut map = state.client_to_agent.write().unwrap();
                let ours = map.get(conn_id).is_some_and(|h| *h == target_id);
                if ours {
                    map.remove(conn_id);
                }
                ours
            };
            if !ours {
                debug!("EndSession from {} for {} without a session there; ignored", conn_id, target_id);
                return;
            }

            // Check if there is an online Agent registered for this host_id
            let agent_tx_opt = {
                let agents = state.agents.read().unwrap();
                agents.get(&target_id).cloned()
            };

            if let Some(agent_tx) = agent_tx_opt {
                // Forward EndSession to the agent
                let _ = agent_tx.send(ServerToAgentMessage::Signaling(
                    SignalingMessage::EndSession {
                        target_id: conn_id.to_string(),
                    },
                ));
                state.set_host_status(&target_id, HostStatus::Online).await;
                info!("EndSession forwarded to agent {}", target_id);
                return;
            }

            // Fallback: local session
            let session = {
                let mut sessions = state.local_sessions.write().unwrap();
                sessions.remove(&target_id)
            };
            if let Some(session) = session {
                let _ = session.peer_connection.close().await;
            }
            state.set_host_status(&target_id, HostStatus::Online).await;
        }

        SignalingMessage::GetAppList { target_id } => {
            let agent_tx_opt = {
                let agents = state.agents.read().unwrap();
                agents.get(&target_id).cloned()
            };
            if let Some(agent_tx) = agent_tx_opt {
                let _ = agent_tx.send(ServerToAgentMessage::Signaling(
                    SignalingMessage::GetAppList {
                        target_id: conn_id.to_string(),
                    },
                ));
            } else {
                warn!("No agent connected for host {}, cannot get app list", target_id);
                if let Some(client_tx) = state.clients.read().unwrap().get(conn_id) {
                    let _ = client_tx.send(ServerToClientMessage::Signaling(
                        SignalingMessage::Error {
                            message: "No agent connected for this host.".to_string(),
                        },
                    ));
                }
            }
        }
        SignalingMessage::StopActiveStream { target_id } => {
            let agent_tx_opt = {
                let agents = state.agents.read().unwrap();
                agents.get(&target_id).cloned()
            };
            if let Some(agent_tx) = agent_tx_opt {
                let _ = agent_tx.send(ServerToAgentMessage::Signaling(
                    SignalingMessage::StopActiveStream {
                        target_id: conn_id.to_string(),
                    },
                ));
            } else {
                warn!("No agent connected for host {}, cannot stop stream", target_id);
                if let Some(client_tx) = state.clients.read().unwrap().get(conn_id) {
                    let _ = client_tx.send(ServerToClientMessage::Signaling(
                        SignalingMessage::StopActiveStreamResponse {
                            target_id: target_id.clone(),
                            success: false,
                            error: Some("No agent connected for this host.".to_string()),
                        },
                    ));
                }
            }
        }
        SignalingMessage::GetCapabilities { target_id } => {
            info!("GetCapabilities from client {} for host {} (conn: {})", client_id, target_id, conn_id);
            let agents = state.agents.read().unwrap();
            if let Some(agent_tx) = agents.get(&target_id) {
                let msg = ServerToAgentMessage::Signaling(SignalingMessage::GetCapabilities {
                    target_id: conn_id.to_string(),
                });
                if let Err(e) = agent_tx.send(msg) {
                    error!("Failed to forward GetCapabilities to agent {}: {:?}", target_id, e);
                } else {
                    info!("GetCapabilities forwarded to agent {} with conn_id {}", target_id, conn_id);
                }
            } else {
                warn!("No agent found for host {} when handling GetCapabilities", target_id);
            }
        }
        _ => {}
    }
}

/// The host a client message is about, if any.
fn addressed_host(sig: &SignalingMessage) -> Option<&str> {
    match sig {
        SignalingMessage::RequestSession { host_id, .. }
        | SignalingMessage::RequestFileSession { host_id, .. } => Some(host_id),
        SignalingMessage::Sdp { target_id, .. }
        | SignalingMessage::IceCandidate { target_id, .. }
        | SignalingMessage::EndSession { target_id }
        | SignalingMessage::GetAppList { target_id }
        | SignalingMessage::StopActiveStream { target_id }
        | SignalingMessage::GetCapabilities { target_id } => Some(target_id),
        _ => None,
    }
}

/// The client connection an agent's message is for (EndSession has its own
/// check), if it is one the server passes on.
fn agent_reply_target(sig: &SignalingMessage) -> Option<&str> {
    match sig {
        SignalingMessage::Sdp { target_id, .. }
        | SignalingMessage::IceCandidate { target_id, .. }
        | SignalingMessage::AppListResponse { target_id, .. }
        | SignalingMessage::StopActiveStreamResponse { target_id, .. }
        | SignalingMessage::CapabilitiesResponse { target_id, .. }
        | SignalingMessage::SourceStatus { target_id, .. }
        | SignalingMessage::AccessRequest { target_id, .. }
        | SignalingMessage::EncoderStatus { target_id, .. } => Some(target_id),
        _ => None,
    }
}

/// Whether a client connection may send `sig` to `host_id`: the user may use
/// the host (same rule as the host list), or it is ending its own session.
/// A connection signed in with a link token addresses only its link's host,
/// and of it only the link's source (or, without one, its own screen).
/// Such a connection may ask for the host's capabilities (the stream window
/// does, to pick a codec, for a source too), but not for its app list
/// unless the link is for the screen; and it may stop the host's screen
/// stream (`StopActiveStream`, which stops whichever one runs there) only
/// when that is its own: never with a source's link.
async fn client_may_address(
    state: &SignalingState,
    user_id: &str,
    conn_id: &str,
    sig: &SignalingMessage,
    host_id: &str,
) -> bool {
    let own_stream = || state.client_to_agent.read().unwrap().get(conn_id).is_some_and(|h| h == host_id);
    let scope = state.client_scopes.read().unwrap().get(conn_id).cloned();
    if let Some((scope_host, scope_source)) = scope {
        let source = match sig {
            SignalingMessage::RequestSession { source_id, .. }
            | SignalingMessage::RequestFileSession { source_id, .. } => source_id.as_ref(),
            _ => scope_source.as_ref(),
        };
        if scope_host != host_id || source != scope_source.as_ref() {
            return false;
        }
        match sig {
            SignalingMessage::StopActiveStream { .. } if scope_source.is_some() || !own_stream() => return false,
            SignalingMessage::GetAppList { .. } if scope_source.is_some() => return false,
            _ => {}
        }
    }
    if matches!(sig, SignalingMessage::EndSession { .. }) {
        let own_file = state.file_sessions.read().unwrap().contains_key(conn_id);
        if own_stream() || own_file {
            return true;
        }
    }
    security::may_use_host(&state.db, user_id, host_id).await
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn state() -> Arc<SignalingState> {
        let db = crate::security::tests::test_db().await;
        Arc::new(SignalingState::new(db, "t".into(), None))
    }

    #[tokio::test]
    async fn closing_a_users_connections() {
        let state = state().await;
        let (tx, mut rx) = crate::queue::channel::<ServerToClientMessage>();
        let (tx2, mut rx2) = crate::queue::channel::<ServerToClientMessage>();
        state.clients.write().unwrap().insert("c1".into(), tx);
        state.clients.write().unwrap().insert("c2".into(), tx2);
        state.client_connections.write().unwrap().insert("c1".into(), "alice".into());
        state.client_connections.write().unwrap().insert("c2".into(), "bob".into());
        state.close_user_connections("alice");
        assert!(rx.recv().await.is_none());
        state.send_to_conn("c2", ServerToClientMessage::Signaling(SignalingMessage::Error { message: "x".into() }));
        assert!(rx2.recv().await.is_some());
    }

    #[tokio::test]
    async fn agent_ends_only_its_own_sessions() {
        let state = state().await;
        let (tx, mut rx) = crate::queue::channel::<ServerToClientMessage>();
        state.clients.write().unwrap().insert("c1".into(), tx);
        state.client_to_agent.write().unwrap().insert("c1".into(), "host-a".into());
        // Another agent can't end it.
        handle_agent_signaling(SignalingMessage::EndSession { target_id: "c1".into() }, "host-b", state.clone()).await;
        assert_eq!(state.client_to_agent.read().unwrap().get("c1").map(String::as_str), Some("host-a"));
        // Its own agent can, and the client hears about it.
        handle_agent_signaling(SignalingMessage::EndSession { target_id: "c1".into() }, "host-a", state.clone()).await;
        assert!(state.client_to_agent.read().unwrap().get("c1").is_none());
        assert!(matches!(
            rx.recv().await,
            Some(ServerToClientMessage::Signaling(SignalingMessage::EndSession { .. }))
        ));
    }

    #[tokio::test]
    async fn client_messages_need_host_access() {
        let state = state().await;
        sqlx::query("INSERT INTO users (id, username, password_hash, role, is_active) VALUES ('u1', 'u1', 'x', 'user', 1)")
            .execute(&state.db)
            .await
            .unwrap();
        let (agent_tx, mut agent_rx) = crate::queue::channel::<ServerToAgentMessage>();
        state.agents.write().unwrap().insert("h1".into(), agent_tx);
        let (tx, mut rx) = crate::queue::channel::<ServerToClientMessage>();
        state.clients.write().unwrap().insert("c1".into(), tx);

        handle_client_signaling(SignalingMessage::GetCapabilities { target_id: "h1".into() }, "u1", "c1", state.clone()).await;
        assert!(matches!(
            rx.recv().await,
            Some(ServerToClientMessage::Signaling(SignalingMessage::Error { .. }))
        ));

        // In a group with the host: forwarded.
        sqlx::query("INSERT INTO hosts (id, name, status) VALUES ('h1', 'H1', 'Online')").execute(&state.db).await.unwrap();
        sqlx::query("INSERT INTO groups (id, name) VALUES ('g', 'G')").execute(&state.db).await.unwrap();
        sqlx::query("INSERT INTO host_groups VALUES ('h1', 'g')").execute(&state.db).await.unwrap();
        sqlx::query("INSERT INTO user_groups VALUES ('u1', 'g')").execute(&state.db).await.unwrap();
        handle_client_signaling(SignalingMessage::GetCapabilities { target_id: "h1".into() }, "u1", "c1", state.clone()).await;
        assert!(matches!(
            agent_rx.recv().await,
            Some(ServerToAgentMessage::Signaling(SignalingMessage::GetCapabilities { .. }))
        ));
    }

    /// A connection signed in with a link token (link_tickets.rs) reaches
    /// only its link's host, even for an admin who may use every host.
    #[tokio::test]
    async fn a_link_token_connection_reaches_only_its_host() {
        let state = state().await;
        exec(&state, "INSERT INTO users (id, username, password_hash, role, is_active) VALUES ('boss', 'boss', 'x', 'admin', 1)").await;
        let mut h1 = agent(&state, "h1");
        let mut h2 = agent(&state, "h2");
        let mut rx = client(&state, "c1", "boss");
        state.client_scopes.write().unwrap().insert("c1".into(), ("h1".into(), None));

        handle_client_signaling(SignalingMessage::GetCapabilities { target_id: "h2".into() }, "boss", "c1", state.clone()).await;
        assert!(matches!(rx.rx_try(), Some(ServerToClientMessage::Signaling(SignalingMessage::Error { .. }))));
        assert!(h2.rx_try().is_none());
        handle_client_signaling(
            SignalingMessage::RequestFileSession { host_id: "h2".into(), source_id: None },
            "boss",
            "c1",
            state.clone(),
        )
        .await;
        assert!(matches!(rx.rx_try(), Some(ServerToClientMessage::Signaling(SignalingMessage::Error { .. }))));
        assert!(h2.rx_try().is_none());

        // Not a source of its host either: links name the host's own screen.
        handle_client_signaling(
            SignalingMessage::RequestFileSession { host_id: "h1".into(), source_id: Some("s1".into()) },
            "boss",
            "c1",
            state.clone(),
        )
        .await;
        assert!(matches!(rx.rx_try(), Some(ServerToClientMessage::Signaling(SignalingMessage::Error { .. }))));
        assert!(h1.rx_try().is_none());

        handle_client_signaling(SignalingMessage::GetCapabilities { target_id: "h1".into() }, "boss", "c1", state.clone()).await;
        assert!(matches!(
            h1.rx_try(),
            Some(ServerToAgentMessage::Signaling(SignalingMessage::GetCapabilities { .. }))
        ));

        // Scoped to a source of h1: that source only, not h1's own screen.
        let mut rx3 = client(&state, "c3", "boss");
        state.client_scopes.write().unwrap().insert("c3".into(), ("h1".into(), Some("s1".into())));
        handle_client_signaling(
            SignalingMessage::RequestFileSession { host_id: "h1".into(), source_id: None },
            "boss",
            "c3",
            state.clone(),
        )
        .await;
        assert!(matches!(rx3.rx_try(), Some(ServerToClientMessage::Signaling(SignalingMessage::Error { .. }))));
        handle_client_signaling(
            SignalingMessage::RequestFileSession { host_id: "h1".into(), source_id: Some("s2".into()) },
            "boss",
            "c3",
            state.clone(),
        )
        .await;
        assert!(matches!(rx3.rx_try(), Some(ServerToClientMessage::Signaling(SignalingMessage::Error { .. }))));
        assert!(h1.rx_try().is_none());
        handle_client_signaling(
            SignalingMessage::RequestFileSession { host_id: "h1".into(), source_id: Some("s1".into()) },
            "boss",
            "c3",
            state.clone(),
        )
        .await;
        assert!(matches!(
            h1.rx_try(),
            Some(ServerToAgentMessage::Signaling(SignalingMessage::IncomingFileSession { .. }))
        ));
        // Capabilities (the stream window picks its codec), but not the
        // app list, and never stopping the host's own screen stream.
        handle_client_signaling(SignalingMessage::GetCapabilities { target_id: "h1".into() }, "boss", "c3", state.clone()).await;
        assert!(matches!(h1.rx_try(), Some(ServerToAgentMessage::Signaling(SignalingMessage::GetCapabilities { .. }))));
        for sig in [
            SignalingMessage::GetAppList { target_id: "h1".into() },
            SignalingMessage::StopActiveStream { target_id: "h1".into() },
        ] {
            handle_client_signaling(sig, "boss", "c3", state.clone()).await;
            assert!(matches!(rx3.rx_try(), Some(ServerToClientMessage::Signaling(SignalingMessage::Error { .. }))));
            assert!(h1.rx_try().is_none());
        }
        // The screen's link stops only a stream that is its own.
        handle_client_signaling(SignalingMessage::StopActiveStream { target_id: "h1".into() }, "boss", "c1", state.clone()).await;
        assert!(matches!(rx.rx_try(), Some(ServerToClientMessage::Signaling(SignalingMessage::Error { .. }))));
        assert!(h1.rx_try().is_none());
        state.client_to_agent.write().unwrap().insert("c1".into(), "h1".into());
        handle_client_signaling(SignalingMessage::StopActiveStream { target_id: "h1".into() }, "boss", "c1", state.clone()).await;
        assert!(matches!(h1.rx_try(), Some(ServerToAgentMessage::Signaling(SignalingMessage::StopActiveStream { .. }))));
        state.client_to_agent.write().unwrap().remove("c1");

        // Another connection of the same user without the scope: every host.
        let _rx2 = client(&state, "c2", "boss");
        handle_client_signaling(SignalingMessage::GetCapabilities { target_id: "h2".into() }, "boss", "c2", state.clone()).await;
        assert!(matches!(
            h2.rx_try(),
            Some(ServerToAgentMessage::Signaling(SignalingMessage::GetCapabilities { .. }))
        ));
    }

    #[tokio::test]
    async fn agent_key_is_stored_then_required() {
        let state = state().await;
        let key = "00112233445566778899aabbccddeeff";
        // Old agent, new id: accepted, nothing stored.
        assert!(check_agent_key(&state, "h1", "PC", None).await.unwrap());
        // First key: stored.
        assert!(check_agent_key(&state, "h1", "PC", Some(key)).await.unwrap());
        assert!(check_agent_key(&state, "h1", "PC", Some(key)).await.unwrap());
        // Someone else with the shared token can't take the id over.
        assert!(!check_agent_key(&state, "h1", "PC", Some("ffffffffffffffffffffffffffffffff")).await.unwrap());
        assert!(!check_agent_key(&state, "h1", "PC", None).await.unwrap());
    }

    #[tokio::test]
    async fn key_cannot_claim_an_online_keyless_agent() {
        let state = state().await;
        let key = "00112233445566778899aabbccddeeff";
        // An old agent (no key) is online as h1.
        let (tx, _rx) = crate::queue::channel::<ServerToAgentMessage>();
        state.agents.write().unwrap().insert("h1".into(), tx);
        assert!(!check_agent_key(&state, "h1", "PC", Some(key)).await.unwrap());
        let hash: Option<String> = sqlx::query_scalar("SELECT agent_key_hash FROM hosts WHERE id = 'h1'")
            .fetch_optional(&state.db)
            .await
            .unwrap()
            .flatten();
        assert!(hash.is_none());
        // The old agent itself may still reconnect.
        assert!(check_agent_key(&state, "h1", "PC", None).await.unwrap());
        // Offline: the first key is stored as before.
        state.agents.write().unwrap().remove("h1");
        assert!(check_agent_key(&state, "h1", "PC", Some(key)).await.unwrap());
    }

    #[tokio::test]
    async fn agent_token_limit_tolerates_retrying_agents() {
        let state = state().await;
        let limit = &state.agent_token_limit;
        let now = std::time::Instant::now();
        // A stale agent retrying every 3 s for 5 minutes.
        for _ in 0..100 {
            limit.record_failure("10.0.0.9", now);
        }
        assert!(!limit.is_blocked("10.0.0.9", now));
        for _ in 0..200 {
            limit.record_failure("10.0.0.9", now);
        }
        assert!(limit.is_blocked("10.0.0.9", now));
    }

    async fn host_status(state: &SignalingState, id: &str) -> String {
        sqlx::query_scalar("SELECT status FROM hosts WHERE id = ?").bind(id).fetch_one(&state.db).await.unwrap()
    }

    #[tokio::test]
    async fn only_the_sessions_connection_ends_it() {
        let state = state().await;
        sqlx::query("INSERT INTO users (id, username, password_hash, role, is_active) VALUES ('a', 'a', 'x', 'admin', 1)")
            .execute(&state.db)
            .await
            .unwrap();
        sqlx::query("INSERT INTO hosts (id, name, status) VALUES ('h1', 'H1', 'Online')").execute(&state.db).await.unwrap();
        let (agent_tx, mut agent_rx) = crate::queue::channel::<ServerToAgentMessage>();
        state.agents.write().unwrap().insert("h1".into(), agent_tx);
        for conn in ["c1", "c2"] {
            let (tx, _rx) = crate::queue::channel::<ServerToClientMessage>();
            state.clients.write().unwrap().insert(conn.into(), tx);
        }
        let request = || SignalingMessage::RequestSession {
            host_id: "h1".into(),
            width: None,
            height: None,
            fps: None,
            bitrate: None,
            codec: None,
            app_id: None,
            encoder: None,
            display_id: None,
            chroma: None,
            encoder_csc_mode: None,
            microphone: None,
            source_id: None,
            decode_codecs: None,
        };
        handle_client_signaling(request(), "a", "c1", state.clone()).await;
        // A second connection takes the host over (the agent runs one session).
        handle_client_signaling(request(), "a", "c2", state.clone()).await;
        assert!(state.client_to_agent.read().unwrap().get("c1").is_none());
        assert_eq!(host_status(&state, "h1").await, "Busy");
        while agent_rx.rx_try().is_some() {}

        // c1 ending "its" session: nothing reaches the agent, h1 stays busy.
        handle_client_signaling(SignalingMessage::EndSession { target_id: "h1".into() }, "a", "c1", state.clone()).await;
        assert!(agent_rx.rx_try().is_none());
        assert_eq!(host_status(&state, "h1").await, "Busy");

        // c2 ends it for real.
        handle_client_signaling(SignalingMessage::EndSession { target_id: "h1".into() }, "a", "c2", state.clone()).await;
        assert!(matches!(
            agent_rx.rx_try(),
            Some(ServerToAgentMessage::Signaling(SignalingMessage::EndSession { .. }))
        ));
        assert_eq!(host_status(&state, "h1").await, "Online");
    }

    /// A file session for one of a host's sources names it to the agent;
    /// clients and agents from before the field mean, and are told, "the
    /// host's own files" exactly as before.
    #[tokio::test]
    async fn a_file_session_names_its_source_and_older_peers_still_work() {
        let state = state().await;
        sqlx::query("INSERT INTO users (id, username, password_hash, role, is_active) VALUES ('a', 'a', 'x', 'admin', 1)")
            .execute(&state.db)
            .await
            .unwrap();
        sqlx::query("INSERT INTO hosts (id, name, status) VALUES ('h1', 'H1', 'Online')").execute(&state.db).await.unwrap();
        let (agent_tx, mut agent_rx) = crate::queue::channel::<ServerToAgentMessage>();
        state.agents.write().unwrap().insert("h1".into(), agent_tx);
        let (client_tx, _client_rx) = crate::queue::channel::<ServerToClientMessage>();
        state.clients.write().unwrap().insert("c1".into(), client_tx);

        // A new client, as it is on the wire.
        let new_client: SignalingMessage =
            serde_json::from_str(r#"{"type":"RequestFileSession","payload":{"host_id":"h1","source_id":"office-pc"}}"#).unwrap();
        handle_client_signaling(new_client, "a", "c1", state.clone()).await;
        match agent_rx.rx_try() {
            Some(message @ ServerToAgentMessage::Signaling(SignalingMessage::IncomingFileSession { .. })) => {
                let ServerToAgentMessage::Signaling(SignalingMessage::IncomingFileSession { client_id, source_id, .. }) = &message else { unreachable!() };
                assert_eq!((client_id.as_str(), source_id.as_deref()), ("c1", Some("office-pc")));
                assert!(serde_json::to_string(&message).unwrap().contains(r#""source_id":"office-pc""#));
            }
            _ => panic!("the agent got no IncomingFileSession"),
        }
        // The file session did not touch the host's streaming state.
        assert_eq!(host_status(&state, "h1").await, "Online");

        // A client from before the field.
        let old_client: SignalingMessage = serde_json::from_str(r#"{"type":"RequestFileSession","payload":{"host_id":"h1"}}"#).unwrap();
        assert!(matches!(&old_client, SignalingMessage::RequestFileSession { source_id: None, .. }));
        handle_client_signaling(old_client, "a", "c1", state.clone()).await;
        match agent_rx.rx_try() {
            Some(message @ ServerToAgentMessage::Signaling(SignalingMessage::IncomingFileSession { .. })) => {
                assert!(matches!(&message, ServerToAgentMessage::Signaling(SignalingMessage::IncomingFileSession { source_id: None, .. })));
                // What an agent from before the field is sent has no trace of it.
                assert!(!serde_json::to_string(&message).unwrap().contains("source_id"));
            }
            _ => panic!("the agent got no IncomingFileSession"),
        }
        // What an old server sends an agent (no field) still parses, as "own files";
        // and a new client's request without a source is the old request, byte for byte.
        let from_old_server: SignalingMessage =
            serde_json::from_str(r#"{"type":"IncomingFileSession","payload":{"client_id":"c1","requester":"bob"}}"#).unwrap();
        assert!(matches!(from_old_server, SignalingMessage::IncomingFileSession { source_id: None, .. }));
        assert_eq!(
            serde_json::to_string(&SignalingMessage::RequestFileSession { host_id: "h1".into(), source_id: None }).unwrap(),
            r#"{"type":"RequestFileSession","payload":{"host_id":"h1"}}"#
        );
    }

    /// The source asked for and the codecs the client decodes reach the
    /// agent; the agent's sources and what it says of a source reach the
    /// client. Messages from clients and agents of before these fields
    /// still parse, and mean "no source".
    #[tokio::test]
    async fn sources_pass_through_and_older_peers_still_work() {
        let state = state().await;
        sqlx::query("INSERT INTO users (id, username, password_hash, role, is_active) VALUES ('a', 'a', 'x', 'admin', 1)")
            .execute(&state.db)
            .await
            .unwrap();
        sqlx::query("INSERT INTO hosts (id, name, status) VALUES ('h1', 'H1', 'Online')").execute(&state.db).await.unwrap();
        let (agent_tx, mut agent_rx) = crate::queue::channel::<ServerToAgentMessage>();
        state.agents.write().unwrap().insert("h1".into(), agent_tx);
        let (client_tx, mut client_rx) = crate::queue::channel::<ServerToClientMessage>();
        state.clients.write().unwrap().insert("c1".into(), client_tx);

        // A new client: the request as it is on the wire.
        let new_client: SignalingMessage = serde_json::from_str(
            r#"{"type":"RequestSession","payload":{"host_id":"h1","width":1280,"height":720,"fps":60,"bitrate":8000,"codec":"h264","app_id":null,"encoder":null,"display_id":null,"source_id":"office-pc","decode_codecs":["h264","vp8","av1"]}}"#,
        )
        .unwrap();
        handle_client_signaling(new_client, "a", "c1", state.clone()).await;
        match agent_rx.rx_try() {
            Some(ServerToAgentMessage::Signaling(SignalingMessage::IncomingSession { source_id, decode_codecs, client_id, .. })) => {
                assert_eq!(client_id, "c1");
                assert_eq!(source_id.as_deref(), Some("office-pc"));
                assert_eq!(decode_codecs, Some(vec!["h264".to_string(), "vp8".to_string(), "av1".to_string()]));
            }
            _ => panic!("the agent got no IncomingSession"),
        }

        // A client from before the fields: the same request without them.
        let old_client: SignalingMessage = serde_json::from_str(
            r#"{"type":"RequestSession","payload":{"host_id":"h1","width":1280,"height":720,"fps":60,"bitrate":8000,"codec":"h264","app_id":null,"encoder":null,"display_id":null}}"#,
        )
        .unwrap();
        handle_client_signaling(old_client, "a", "c1", state.clone()).await;
        while let Some(message) = agent_rx.rx_try() {
            if let ServerToAgentMessage::Signaling(SignalingMessage::IncomingSession { source_id, decode_codecs, .. }) = &message {
                assert!(source_id.is_none() && decode_codecs.is_none());
                // What an agent from before the fields is sent has no trace of them.
                let wire = serde_json::to_string(&message).unwrap();
                assert!(!wire.contains("source_id") && !wire.contains("decode_codecs"), "{}", wire);
            }
        }
        while client_rx.rx_try().is_some() {}

        // The agent's capabilities: with sources (new agent) and without (old agent).
        let new_agent: SignalingMessage = serde_json::from_str(
            r#"{"type":"CapabilitiesResponse","payload":{"target_id":"c1","displays":[],"encoders":["native"],"gpu_info":null,"host_os":"linux","sources":[{"id":"office-pc","name":"Office PC","connector":"rustdesk"}]}}"#,
        )
        .unwrap();
        handle_agent_signaling(new_agent, "h1", state.clone()).await;
        match client_rx.rx_try() {
            Some(ServerToClientMessage::Signaling(SignalingMessage::CapabilitiesResponse { target_id, sources, .. })) => {
                assert_eq!(target_id, "h1");
                assert_eq!(sources, vec![common::SourceInfoMsg { id: "office-pc".into(), name: "Office PC".into(), connector: "rustdesk".into(), group: None, online: None, values: None }]);
            }
            _ => panic!("the client got no CapabilitiesResponse"),
        }
        let old_agent: SignalingMessage = serde_json::from_str(
            r#"{"type":"CapabilitiesResponse","payload":{"target_id":"c1","displays":[],"encoders":["native"],"gpu_info":null,"host_os":"linux"}}"#,
        )
        .unwrap();
        handle_agent_signaling(old_agent, "h1", state.clone()).await;
        match client_rx.rx_try() {
            Some(message @ ServerToClientMessage::Signaling(SignalingMessage::CapabilitiesResponse { .. })) => {
                // A client from before the field is not sent an empty list either.
                assert!(!serde_json::to_string(&message).unwrap().contains("sources"));
            }
            _ => panic!("the client got no CapabilitiesResponse"),
        }

        // What the agent says of a source (it failed, it ended) reaches the client.
        handle_agent_signaling(
            SignalingMessage::SourceStatus { target_id: "c1".into(), status: "failed".into(), reason: Some("Wrong password".into()) },
            "h1",
            state.clone(),
        )
        .await;
        match client_rx.rx_try() {
            Some(ServerToClientMessage::Signaling(SignalingMessage::SourceStatus { target_id, status, reason })) => {
                assert_eq!((target_id.as_str(), status.as_str(), reason.as_deref()), ("h1", "failed", Some("Wrong password")));
            }
            _ => panic!("the client got no SourceStatus"),
        }

        // A message type this build does not know (a newer peer) does not parse,
        // which the socket loops log and skip.
        assert!(serde_json::from_str::<SignalingMessage>(r#"{"type":"SomethingNewer","payload":{"target_id":"c1"}}"#).is_err());
    }

    async fn exec(state: &SignalingState, sql: &'static str) {
        sqlx::query(sql).execute(&state.db).await.unwrap();
    }

    fn client(state: &SignalingState, conn: &str, user: &str) -> Inbox<ServerToClientMessage> {
        let (tx, rx) = crate::queue::channel::<ServerToClientMessage>();
        state.clients.write().unwrap().insert(conn.into(), tx);
        state.client_connections.write().unwrap().insert(conn.into(), user.into());
        rx
    }

    fn agent(state: &SignalingState, id: &str) -> Inbox<ServerToAgentMessage> {
        let (tx, rx) = crate::queue::channel::<ServerToAgentMessage>();
        state.agents.write().unwrap().insert(id.into(), tx);
        state.agent_connections.write().unwrap().insert(id.into(), Uuid::new_v4());
        rx
    }

    /// A computer added by address announces its id itself: it can't take
    /// over another computer's id, nor replace a live connection, while the
    /// link itself can always reconnect with the id it registered first.
    #[tokio::test]
    async fn a_link_cannot_take_over_another_hosts_id() {
        let state = state().await;
        let key_hash = security::sha256_hex("00112233445566778899aabbccddeeff");
        sqlx::query("INSERT INTO hosts (id, name, status, agent_key_hash) VALUES ('h1', 'Real', 'Online', ?)")
            .bind(&key_hash)
            .execute(&state.db)
            .await
            .unwrap();
        exec(&state, "INSERT INTO hosts (id, name, status) VALUES ('old', 'Old keyless', 'Offline')").await;
        let mut real = agent(&state, "h1");

        // An impostor link claiming h1 (keyed, online), on first contact or
        // after it had registered as something else.
        assert!(link_may_register(&state, "h1", "IMPOSTOR", None).await.is_err());
        assert!(link_may_register(&state, "h1", "IMPOSTOR", Some("pc-x")).await.is_err());
        // Even with h1's own id remembered: h1 signs in with its key.
        assert!(link_may_register(&state, "h1", "IMPOSTOR", Some("h1")).await.is_err());
        // An id the server already knows (no key) is not for a new link either.
        assert!(link_may_register(&state, "old", "IMPOSTOR", None).await.is_err());
        assert!(link_may_register(&state, "address:x:1", "x", None).await.is_err());
        // And no link replaces a live connection.
        assert!(link_connected(&state, "h1", "IMPOSTOR", None).await.is_none());
        assert!(real.rx_try().is_none());
        let real_tx_still_there = state.agents.read().unwrap().get("h1").is_some();
        assert!(real_tx_still_there);
        let name: String = sqlx::query_scalar("SELECT name FROM hosts WHERE id = 'h1'").fetch_one(&state.db).await.unwrap();
        assert_eq!(name, "Real");

        // A new computer: accepted, and again when it reconnects.
        link_may_register(&state, "pc-new", "PC", None).await.unwrap();
        let (conn, _rx) = link_connected(&state, "pc-new", "PC", None).await.unwrap();
        // Its connection is live: a second one can't replace it.
        assert!(link_connected(&state, "pc-new", "PC", None).await.is_none());
        agent_disconnected(&state, "pc-new", conn).await;
        link_may_register(&state, "pc-new", "PC", Some("pc-new")).await.unwrap();
        assert!(link_connected(&state, "pc-new", "PC", None).await.is_some());
        // The same link later announcing another id: refused.
        assert!(link_may_register(&state, "pc-other", "PC", Some("pc-new")).await.is_err());
    }

    async fn user_with_group(state: &SignalingState, user: &str, group: &str, hosts: &[&str]) {
        sqlx::query("INSERT INTO users (id, username, password_hash, role, is_active) VALUES (?, ?, 'x', 'user', 1)")
            .bind(user)
            .bind(user)
            .execute(&state.db)
            .await
            .unwrap();
        sqlx::query("INSERT OR IGNORE INTO groups (id, name) VALUES (?, ?)").bind(group).bind(group).execute(&state.db).await.unwrap();
        sqlx::query("INSERT INTO user_groups VALUES (?, ?)").bind(user).bind(group).execute(&state.db).await.unwrap();
        for h in hosts {
            sqlx::query("INSERT OR IGNORE INTO hosts (id, name, status) VALUES (?, ?, 'Online')").bind(h).bind(h).execute(&state.db).await.unwrap();
            sqlx::query("INSERT OR IGNORE INTO host_groups VALUES (?, ?)").bind(h).bind(group).execute(&state.db).await.unwrap();
        }
    }

    /// Access taken away: the user's live session, file session and open
    /// questions on that host end at once; others' don't.
    #[tokio::test]
    async fn removed_access_ends_live_sessions() {
        let state = state().await;
        user_with_group(&state, "alice", "g", &["h1"]).await;
        user_with_group(&state, "bob", "g2", &["h1"]).await;
        let mut h1 = agent(&state, "h1");
        let mut stream = client(&state, "c-stream", "alice");
        let mut files = client(&state, "c-files", "alice");
        let mut bobs = client(&state, "c-bob", "bob");
        state.client_to_agent.write().unwrap().insert("c-stream".into(), "h1".into());
        state.file_sessions.write().unwrap().insert("c-files".into(), "h1".into());
        state.client_asked.write().unwrap().insert(("c-stream".into(), "h1".into()));
        state.file_sessions.write().unwrap().insert("c-bob".into(), "h1".into());

        // Nothing changed: nothing ends.
        end_sessions_without_access(&state).await;
        assert!(h1.rx_try().is_none());
        assert_eq!(state.client_to_agent.read().unwrap().len(), 1);

        exec(&state, "DELETE FROM user_groups WHERE user_id = 'alice'").await;
        end_sessions_without_access(&state).await;
        let mut ended = Vec::new();
        while let Some(ServerToAgentMessage::Signaling(SignalingMessage::EndSession { target_id })) = h1.rx_try() {
            ended.push(target_id);
        }
        ended.sort();
        assert_eq!(ended, vec!["c-files".to_string(), "c-stream".to_string()]);
        assert!(state.client_to_agent.read().unwrap().is_empty());
        assert!(!state.file_sessions.read().unwrap().contains_key("c-files"));
        assert!(state.client_asked.read().unwrap().is_empty());
        for rx in [&mut stream, &mut files] {
            assert!(matches!(rx.rx_try(), Some(ServerToClientMessage::Signaling(SignalingMessage::Error { .. }))));
            assert!(rx.recv().await.is_none(), "the connection is closed");
        }
        // Bob keeps his.
        assert!(state.file_sessions.read().unwrap().contains_key("c-bob"));
        assert!(bobs.rx_try().is_none());
    }

    /// An agent's answers reach only connections that have a session with
    /// it or asked it something.
    #[tokio::test]
    async fn agent_messages_reach_only_its_own_clients() {
        let state = state().await;
        user_with_group(&state, "alice", "g", &["h1", "h2"]).await;
        let _h2 = agent(&state, "h2");
        let mut c1 = client(&state, "c1", "alice");
        let caps = || SignalingMessage::CapabilitiesResponse {
            target_id: "c1".into(),
            displays: vec![],
            encoders: vec![],
            gpu_info: None,
            host_os: None,
            sources: vec![],
        };
        let sdp = SignalingMessage::Sdp {
            target_id: "c1".into(),
            sdp: common::RtcSessionDescription { ty: common::RtcSdpType::Offer, sdp: "v=0".into() },
            ice_servers: None,
            webtransport_port: None,
            webtransport_cert_hash: None,
            webtransport_token: None,
        };
        // h1 has nothing to do with c1.
        handle_agent_signaling(caps(), "h1", state.clone()).await;
        handle_agent_signaling(sdp.clone(), "h1", state.clone()).await;
        handle_agent_signaling(
            SignalingMessage::AccessRequest { target_id: "c1".into(), status: "accepted".into() },
            "h1",
            state.clone(),
        )
        .await;
        assert!(c1.rx_try().is_none());
        // c1 asks h2: h2's answer comes through, h1's still doesn't.
        handle_client_signaling(SignalingMessage::GetCapabilities { target_id: "h2".into() }, "alice", "c1", state.clone()).await;
        handle_agent_signaling(caps(), "h1", state.clone()).await;
        assert!(c1.rx_try().is_none());
        handle_agent_signaling(caps(), "h2", state.clone()).await;
        assert!(matches!(c1.rx_try(), Some(ServerToClientMessage::Signaling(SignalingMessage::CapabilitiesResponse { .. }))));
        // A session with h1: its offer comes through.
        state.client_to_agent.write().unwrap().insert("c1".into(), "h1".into());
        handle_agent_signaling(sdp, "h1", state.clone()).await;
        assert!(matches!(c1.rx_try(), Some(ServerToClientMessage::Signaling(SignalingMessage::Sdp { .. }))));
    }

    /// A host removed or its key reset: the agent connected as it goes now,
    /// and its clients' sessions end.
    #[tokio::test]
    async fn disconnecting_an_agent_ends_its_sessions() {
        let state = state().await;
        exec(&state, "INSERT INTO hosts (id, name, status) VALUES ('h1', 'H1', 'Busy')").await;
        let mut h1 = agent(&state, "h1");
        let mut c1 = client(&state, "c1", "alice");
        let mut c2 = client(&state, "c2", "alice");
        state.client_to_agent.write().unwrap().insert("c1".into(), "h1".into());
        state.file_sessions.write().unwrap().insert("c2".into(), "h1".into());
        disconnect_agent(&state, "h1").await;
        assert!(h1.recv().await.is_none(), "the agent's connection is closed");
        assert!(!state.agents.read().unwrap().contains_key("h1"));
        assert!(!state.agent_connections.read().unwrap().contains_key("h1"));
        assert!(state.client_to_agent.read().unwrap().is_empty() && state.file_sessions.read().unwrap().is_empty());
        assert!(matches!(c1.rx_try(), Some(ServerToClientMessage::Signaling(SignalingMessage::Error { .. }))));
        assert!(matches!(c2.rx_try(), Some(ServerToClientMessage::Signaling(SignalingMessage::EndSession { .. }))));
        assert!(c2.recv().await.is_none());
        assert_eq!(host_status(&state, "h1").await, "Offline");
    }
}
