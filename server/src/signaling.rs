use axum::{
    extract::{
        ws::{Message, WebSocket, WebSocketUpgrade},
        Query, State,
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
    sync::{Arc, RwLock},
};
use tokio::sync::mpsc;
use tracing::{debug, error, info, warn};
use uuid::Uuid;

use crate::auth::verify_jwt;
use webrtc::ice_transport::ice_candidate::RTCIceCandidateInit;
use webrtc::peer_connection::sdp::session_description::RTCSessionDescription;

pub struct SignalingState {
    pub db: SqlitePool,
    pub agent_token: String,
    // agent_id -> sender to agent WS
    pub agents: RwLock<HashMap<String, mpsc::UnboundedSender<ServerToAgentMessage>>>,
    // client_id -> sender to client WS
    pub clients: RwLock<HashMap<String, mpsc::UnboundedSender<ServerToClientMessage>>>,
    // client_id -> agent_id active peer mappings
    pub client_to_agent: RwLock<HashMap<String, String>>,
    // agent_id -> active connection Uuid
    pub agent_connections: RwLock<HashMap<String, Uuid>>,
    // client_id -> active connection Uuid
    pub client_connections: RwLock<HashMap<String, Uuid>>,
    // client_id -> BridgeSession managed by server
    pub local_sessions: RwLock<HashMap<String, Arc<crate::bridge::BridgeSession>>>,
}

impl SignalingState {
    pub fn new(db: SqlitePool, agent_token: String) -> Self {
        Self {
            db,
            agent_token,
            agents: RwLock::new(HashMap::new()),
            clients: RwLock::new(HashMap::new()),
            client_to_agent: RwLock::new(HashMap::new()),
            agent_connections: RwLock::new(HashMap::new()),
            client_connections: RwLock::new(HashMap::new()),
            local_sessions: RwLock::new(HashMap::new()),
        }
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

    ws.on_upgrade(move |socket| handle_agent_socket(socket, params, state))
}

pub async fn client_ws_handler(
    ws: WebSocketUpgrade,
    Query(params): Query<ClientParams>,
    State(state): State<Arc<SignalingState>>,
) -> impl IntoResponse {
    // Verify token before upgrading if possible, or inside connection.
    // Let's do it inside for simplicity with standard ws URL connection.
    match verify_jwt(&params.token) {
        Ok(claims) => ws.on_upgrade(move |socket| handle_client_socket(socket, claims.sub, state)),
        Err(_) => StatusCode::UNAUTHORIZED.into_response(),
    }
}

// --- Agent Connection Handler ---
async fn handle_agent_socket(socket: WebSocket, params: AgentParams, state: Arc<SignalingState>) {
    let (mut ws_sender, mut ws_receiver) = socket.split();
    let (tx, mut rx) = mpsc::unbounded_channel::<ServerToAgentMessage>();

    let agent_id = params.id.clone();
    let agent_name = params.name.clone();
    let conn_id = Uuid::new_v4();

    // Register active agent sender
    {
        state.agents.write().unwrap().insert(agent_id.clone(), tx);
        state
            .agent_connections
            .write()
            .unwrap()
            .insert(agent_id.clone(), conn_id);
    }

    // Register host in DB as Online
    state
        .register_host_db(&agent_id, &agent_name, params.codec_support)
        .await;
    info!("Agent registered: {} ({})", agent_name, agent_id);

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
            let agent_msg: AgentMessage = match serde_json::from_str(&text) {
                Ok(m) => m,
                Err(e) => {
                    error!("Failed to parse agent message: {}", e);
                    continue;
                }
            };

            match agent_msg {
                AgentMessage::Register { .. } => {
                    // Handled upon connection
                }
                AgentMessage::StatusUpdate { status } => {
                    state.set_host_status(&agent_id, status).await;
                }
                AgentMessage::Signaling(sig) => {
                    handle_agent_signaling(sig, &agent_id, state.clone()).await;
                }
            }
        }
    }

    // Connection closed
    info!("Agent disconnected: {} ({})", agent_name, agent_id);
    let mut should_cleanup = false;
    {
        let mut conn_map = state.agent_connections.write().unwrap();
        if conn_map.get(&agent_id) == Some(&conn_id) {
            conn_map.remove(&agent_id);
            state.agents.write().unwrap().remove(&agent_id);
            should_cleanup = true;
        }
    }

    if should_cleanup {
        state.set_host_status(&agent_id, HostStatus::Offline).await;

        // Clean up active client sessions related to this agent
        let clients_to_disconnect: Vec<String> = state
            .client_to_agent
            .read()
            .unwrap()
            .iter()
            .filter(|(_, a_id)| **a_id == agent_id)
            .map(|(c_id, _)| c_id.clone())
            .collect();

        for client_id in clients_to_disconnect {
            state.client_to_agent.write().unwrap().remove(&client_id);
            if let Some(client_tx) = state.clients.read().unwrap().get(&client_id) {
                let _ = client_tx.send(ServerToClientMessage::Signaling(SignalingMessage::Error {
                    message: "Host disconnected".to_string(),
                }));
            }
        }
    }

    send_task.abort();
}

// --- Client Connection Handler ---
async fn handle_client_socket(socket: WebSocket, client_id: String, state: Arc<SignalingState>) {
    let (mut ws_sender, mut ws_receiver) = socket.split();
    let (tx, mut rx) = mpsc::unbounded_channel::<ServerToClientMessage>();
    let conn_id = Uuid::new_v4();

    // Register active client sender
    {
        state.clients.write().unwrap().insert(client_id.clone(), tx);
        state
            .client_connections
            .write()
            .unwrap()
            .insert(client_id.clone(), conn_id);
    }
    info!("Client WebSocket connected: {}", client_id);

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
                warn!("WebSocket read error from client {}: {}", client_id, e);
                break;
            }
        };

        if let Message::Text(text) = msg {
            let client_msg: ClientMessage = match serde_json::from_str(&text) {
                Ok(m) => m,
                Err(e) => {
                    error!("Failed to parse client message: {}", e);
                    continue;
                }
            };

            match client_msg {
                ClientMessage::Signaling(sig) => {
                    handle_client_signaling(sig, &client_id, state.clone()).await;
                }
            }
        }
    }

    // Connection closed
    info!("Client WebSocket disconnected: {}", client_id);
    let mut should_cleanup = false;
    {
        let mut conn_map = state.client_connections.write().unwrap();
        if conn_map.get(&client_id) == Some(&conn_id) {
            conn_map.remove(&client_id);
            state.clients.write().unwrap().remove(&client_id);
            should_cleanup = true;
        }
    }

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
                let ice_servers = state.fetch_ice_servers().await;
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
            let clients = state.clients.read().unwrap();
            if let Some(client_tx) = clients.get(&target_id) {
                let msg = ServerToClientMessage::Signaling(SignalingMessage::CapabilitiesResponse {
                    target_id: agent_id.to_string(),
                    displays,
                    encoders,
                    gpu_info,
                    host_os,
                });
                let _ = client_tx.send(msg);
            }
        }
        SignalingMessage::EncoderStatus { target_id, encoder, hw_type, gpu_info, requested_encoder, host_os } => {
            let clients = state.clients.read().unwrap();
            if let Some(client_tx) = clients.get(&target_id) {
                let msg = ServerToClientMessage::Signaling(SignalingMessage::EncoderStatus {
                    target_id: agent_id.to_string(),
                    encoder,
                    hw_type,
                    gpu_info,
                    requested_encoder,
                    host_os,
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
    state: Arc<SignalingState>,
) {
    match sig {
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
        } => {
            info!("Client {} requested session for host {} with settings: w={:?}, h={:?}, fps={:?}, bitrate={:?}, codec={:?}, app_id={:?}, virtual_display={:?}", client_id, host_id, width, height, fps, bitrate, codec, app_id, virtual_display);

            let previous_host = state.client_to_agent.write().unwrap().remove(client_id);
            let mut stopped_previous_agent_session = false;
            if let Some(previous_host_id) = previous_host {
                if let Some(agent_tx) = state.agents.read().unwrap().get(&previous_host_id).cloned() {
                    let _ = agent_tx.send(ServerToAgentMessage::Signaling(
                        SignalingMessage::EndSession {
                            target_id: client_id.to_string(),
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
                let ice_servers = state.fetch_ice_servers().await;
                let incoming_msg =
                    ServerToAgentMessage::Signaling(SignalingMessage::IncomingSession {
                        client_id: client_id.to_string(),
                        width,
                        height,
                        fps,
                        bitrate,
                        codec: codec.clone(),
                        app_id,
                        encoder: encoder.clone(),
                        display_id: display_id.clone(),
                        virtual_display,
                        ice_servers: Some(ice_servers),
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
            if let Some(client_tx) = state.clients.read().unwrap().get(client_id) {
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
                    target_id: client_id.to_string(),
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
                        target_id: client_id.to_string(),
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
                        target_id: client_id.to_string(),
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
                        target_id: client_id.to_string(),
                    },
                ));
            } else {
                warn!("No agent connected for host {}, cannot get app list", target_id);
                if let Some(client_tx) = state.clients.read().unwrap().get(client_id) {
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
                        target_id: client_id.to_string(),
                    },
                ));
            } else {
                warn!("No agent connected for host {}, cannot stop stream", target_id);
                if let Some(client_tx) = state.clients.read().unwrap().get(client_id) {
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
            let agents = state.agents.read().unwrap();
            if let Some(agent_tx) = agents.get(&target_id) {
                let msg = ServerToAgentMessage::Signaling(SignalingMessage::GetCapabilities {
                    target_id: client_id.to_string(),
                });
                let _ = agent_tx.send(msg);
            }
        }
        _ => {}
    }
}


