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
use tokio::sync::mpsc;
use tracing::{debug, error, info, warn};
use uuid::Uuid;

use crate::auth::verify_jwt;
use webrtc::ice_transport::ice_candidate::RTCIceCandidateInit;
use webrtc::peer_connection::sdp::session_description::RTCSessionDescription;

/// Both ends of an HTTP/WebSocket connection: the local address is the server
/// address that peer uses, which is where it can reach the TURN relay.
#[derive(Clone, Copy, Debug)]
pub struct ConnAddrs {
    pub local: Option<SocketAddr>,
    pub remote: SocketAddr,
}

impl Connected<axum::serve::IncomingStream<'_>> for ConnAddrs {
    fn connect_info(target: axum::serve::IncomingStream<'_>) -> Self {
        ConnAddrs {
            local: target.local_addr().ok(),
            remote: target.remote_addr(),
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
    pub agents: RwLock<HashMap<String, mpsc::UnboundedSender<ServerToAgentMessage>>>,
    // client_id -> sender to client WS
    pub clients: RwLock<HashMap<String, mpsc::UnboundedSender<ServerToClientMessage>>>,
    // client_id -> agent_id active peer mappings
    pub client_to_agent: RwLock<HashMap<String, String>>,
    // agent_id -> active connection Uuid
    pub agent_connections: RwLock<HashMap<String, Uuid>>,
    // conn_id (String) -> client_id — tracks which client owns each connection
    pub client_connections: RwLock<HashMap<String, String>>,
    // conn_id -> host_id of file-transfer connections (agent/src/files.rs)
    pub file_sessions: RwLock<HashMap<String, String>>,
    // client_id -> BridgeSession managed by server
    pub local_sessions: RwLock<HashMap<String, Arc<crate::bridge::BridgeSession>>>,
    /// Computers added by address (the server connects to them).
    pub links: crate::links::Links,
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
            local_sessions: RwLock::new(HashMap::new()),
            links: crate::links::Links::default(),
        }
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
    State(state): State<Arc<SignalingState>>,
) -> impl IntoResponse {
    let incoming_token = params.token.as_deref().unwrap_or("").trim();
    if incoming_token != state.agent_token.trim() {
        warn!(
            "Unauthorized agent connection attempt for ID {} (invalid or missing token)",
            params.id
        );
        return StatusCode::UNAUTHORIZED.into_response();
    }

    if let Some(local) = addrs.local {
        state.agent_local_ips.write().unwrap().insert(params.id.clone(), local.ip());
    }
    ws.on_upgrade(move |socket| handle_agent_socket(socket, params, state))
}

pub async fn client_ws_handler(
    ws: WebSocketUpgrade,
    Query(params): Query<ClientParams>,
    ConnectInfo(addrs): ConnectInfo<ConnAddrs>,
    State(state): State<Arc<SignalingState>>,
) -> impl IntoResponse {
    // Verify token before upgrading if possible, or inside connection.
    // Let's do it inside for simplicity with standard ws URL connection.
    match verify_jwt(&params.token) {
        Ok(claims) => {
            let local_ip = addrs.local.map(|a| a.ip());
            ws.on_upgrade(move |socket| handle_client_socket(socket, claims.sub, local_ip, state))
        }
        Err(_) => StatusCode::UNAUTHORIZED.into_response(),
    }
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
) -> (Uuid, mpsc::UnboundedReceiver<ServerToAgentMessage>) {
    let (tx, rx) = mpsc::unbounded_channel::<ServerToAgentMessage>();
    let conn_id = Uuid::new_v4();
    state.agents.write().unwrap().insert(agent_id.to_string(), tx);
    state.agent_connections.write().unwrap().insert(agent_id.to_string(), conn_id);
    state.register_host_db(agent_id, agent_name, codec_support).await;
    info!("Agent registered: {} ({})", agent_name, agent_id);
    (conn_id, rx)
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
            should_cleanup = true;
        }
    }
    if !should_cleanup {
        return;
    }
    state.set_host_status(agent_id, HostStatus::Offline).await;

    // Clean up active client sessions related to this agent
    let clients_to_disconnect: Vec<String> = state
        .client_to_agent
        .read()
        .unwrap()
        .iter()
        .filter(|(_, a_id)| a_id.as_str() == agent_id)
        .map(|(c_id, _)| c_id.clone())
        .collect();

    for client_id in clients_to_disconnect {
        state.client_to_agent.write().unwrap().remove(&client_id);
        // Find all connections for this client and send error to each
        let conn_ids: Vec<String> = {
            let conn_map = state.client_connections.read().unwrap();
            conn_map.iter()
                .filter(|(_, c_id)| *c_id == &client_id)
                .map(|(conn_id, _)| conn_id.clone())
                .collect()
        };
        let clients = state.clients.read().unwrap();
        for conn_id in conn_ids {
            if let Some(client_tx) = clients.get(&conn_id) {
                let _ = client_tx.send(ServerToClientMessage::Signaling(SignalingMessage::Error {
                    message: "Host disconnected".to_string(),
                }));
            }
        }
    }
}

async fn handle_agent_socket(socket: WebSocket, params: AgentParams, state: Arc<SignalingState>) {
    let (mut ws_sender, mut ws_receiver) = socket.split();
    let agent_id = params.id.clone();
    let (conn_id, mut rx) = agent_connected(&state, &agent_id, &params.name, params.codec_support).await;

    // Spawn a task to handle outbound messages to the agent (with heartbeat to prevent idle timeouts)
    let agent_id_clone = agent_id.clone();
    let send_task = tokio::spawn(async move {
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
                            if let Err(e) = ws_sender.send(Message::Text(json)).await {
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

    // Handle inbound messages from agent
    while let Some(result) = ws_receiver.next().await {
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
    local_ip: Option<IpAddr>,
    state: Arc<SignalingState>,
) {
    let (mut ws_sender, mut ws_receiver) = socket.split();
    let (tx, mut rx) = mpsc::unbounded_channel::<ServerToClientMessage>();
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
    info!("Client WebSocket connected: {} (conn: {})", client_id, conn_id_str);

    // Spawn a task to handle outbound messages to the client (with heartbeat to prevent idle timeouts)
    let client_id_clone = client_id.clone();
    let send_task = tokio::spawn(async move {
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
                            if let Err(e) = ws_sender.send(Message::Text(json)).await {
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

    // Handle inbound messages from client
    while let Some(result) = ws_receiver.next().await {
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

    // Only clean up session if this was the last connection for this client
    let has_other_connections = {
        let conn_map = state.client_connections.read().unwrap();
        conn_map.values().any(|cid| cid == &client_id)
    };
    let should_cleanup = !has_other_connections;

    if should_cleanup {
        let host_id_opt = state.client_to_agent.write().unwrap().remove(&client_id);
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
                        target_id: client_id.clone(),
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
    match sig {
        SignalingMessage::Sdp {
            target_id,
            sdp,
            ice_servers: _,
            webtransport_port,
            webtransport_cert_hash,
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
            state.client_to_agent.write().unwrap().remove(&target_id);
            if let Some(client_tx) = state.clients.read().unwrap().get(&target_id) {
                let _ = client_tx.send(ServerToClientMessage::Signaling(
                    SignalingMessage::EndSession {
                        target_id: agent_id.to_string(),
                    },
                ));
            }
            state.set_host_status(agent_id, HostStatus::Online).await;
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
        SignalingMessage::CapabilitiesResponse { target_id, displays, encoders, gpu_info, host_os } => {
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
                });
                let _ = client_tx.send(msg);
            } else {
                warn!("Server: Client {} NOT found in clients map! Cannot forward CapabilitiesResponse", target_id);
            }
        }
        SignalingMessage::VirtualDisplayConfigured { target_id, success, message, displays } => {
            info!("Server: VirtualDisplayConfigured from agent {} for client {} (success={}, {} displays)", agent_id, target_id, success, displays.len());
            let clients = state.clients.read().unwrap();
            if let Some(client_tx) = clients.get(&target_id) {
                let msg = ServerToClientMessage::Signaling(SignalingMessage::VirtualDisplayConfigured {
                    target_id: agent_id.to_string(),
                    success,
                    message,
                    displays,
                });
                let _ = client_tx.send(msg);
            } else {
                warn!("Server: Client {} NOT found in clients map! Cannot forward VirtualDisplayConfigured", target_id);
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
    match sig {
        // A file-transfer connection (on its own client WebSocket): routed
        // like a session, without touching the host's streaming state.
        SignalingMessage::RequestFileSession { host_id } => {
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
        SignalingMessage::EndSession { ref target_id } if state.file_sessions.read().unwrap().contains_key(conn_id) => {
            state.file_sessions.write().unwrap().remove(conn_id);
            if let Some(agent_tx) = state.agents.read().unwrap().get(target_id).cloned() {
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
            virtual_display,
            chroma,
            microphone,
        } => {
            info!("Client {} requested session for host {} with settings: w={:?}, h={:?}, fps={:?}, bitrate={:?}, codec={:?}, app_id={:?}, virtual_display={:?}, chroma={:?}", client_id, host_id, width, height, fps, bitrate, codec, app_id, virtual_display, chroma);

            let previous_host = state.client_to_agent.write().unwrap().remove(client_id);
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
                        virtual_display,
                        chroma: chroma.clone(),
                        microphone,
                        ice_servers: Some(ice_servers),
                        requester,
                    });
                if let Err(e) = agent_tx.send(incoming_msg) {
                    error!(
                        "Failed to send IncomingSession to agent {}: {:?}",
                        host_id, e
                    );
                    // Fall through to fallback Agent-less code below
                } else {
                    // Store client-to-host routing and update status to Busy
                    state
                        .client_to_agent
                        .write()
                        .unwrap()
                        .insert(client_id.to_string(), host_id.clone());
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
                state.client_to_agent.write().unwrap().remove(client_id);
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
            state.client_to_agent.write().unwrap().remove(client_id);
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
        SignalingMessage::ConfigureVirtualDisplay { target_id, enable, width, height, refresh_hz } => {
            info!("ConfigureVirtualDisplay from client {} for host {} (enable={}, {}x{}@{}Hz, conn: {})", client_id, target_id, enable, width, height, refresh_hz, conn_id);
            let agents = state.agents.read().unwrap();
            if let Some(agent_tx) = agents.get(&target_id) {
                let msg = ServerToAgentMessage::Signaling(SignalingMessage::ConfigureVirtualDisplay {
                    target_id: conn_id.to_string(),
                    enable,
                    width,
                    height,
                    refresh_hz,
                });
                if let Err(e) = agent_tx.send(msg) {
                    error!("Failed to forward ConfigureVirtualDisplay to agent {}: {:?}", target_id, e);
                } else {
                    info!("ConfigureVirtualDisplay forwarded to agent {} with conn_id {}", target_id, conn_id);
                }
            } else {
                warn!("No agent found for host {} when handling ConfigureVirtualDisplay", target_id);
            }
        }
        _ => {}
    }
}


