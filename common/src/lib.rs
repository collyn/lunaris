use serde::{Deserialize, Serialize};

pub use base64;

// --- REST API TYPES ---

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct RegisterRequest {
    pub username: String,
    pub password: String,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct LoginRequest {
    pub username: String,
    pub password: String,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct AuthResponse {
    pub token: String,
    pub username: String,
    pub role: String,
}


#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct HostInfo {
    pub id: String,
    pub name: String,
    pub status: HostStatus,
    pub ip_address: Option<String>,
    pub server_codec_mode_support: Option<u32>,
    pub agent_connected: bool,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub enum HostStatus {
    Online,
    Offline,
    Busy,
}

// --- ADMIN API TYPES ---

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct UserInfo {
    pub id: String,
    pub username: String,
    pub role: String,
    pub is_active: bool,
    pub groups: Vec<GroupBrief>,
    pub created_at: Option<String>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct GroupBrief {
    pub id: String,
    pub name: String,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct GroupInfo {
    pub id: String,
    pub name: String,
    pub note: String,
    pub user_count: i64,
    pub host_count: i64,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct GroupDetail {
    pub id: String,
    pub name: String,
    pub note: String,
    pub users: Vec<UserBrief>,
    pub hosts: Vec<HostBrief>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct UserBrief {
    pub id: String,
    pub username: String,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct HostBrief {
    pub id: String,
    pub name: String,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct CreateUserRequest {
    pub username: String,
    pub password: String,
    pub role: Option<String>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct UpdateUserRequest {
    pub role: Option<String>,
    pub is_active: Option<bool>,
    pub password: Option<String>,
    pub group_ids: Option<Vec<String>>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct CreateGroupRequest {
    pub name: String,
    pub note: Option<String>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct UpdateGroupRequest {
    pub name: Option<String>,
    pub note: Option<String>,
    pub user_ids: Option<Vec<String>>,
    pub host_ids: Option<Vec<String>>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct RtcIceServer {
    pub urls: Vec<String>,
    pub username: Option<String>,
    pub credential: Option<String>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct TurnServer {
    pub id: String,
    pub urls: String,
    pub username: Option<String>,
    pub credential: Option<String>,
    pub created_at: Option<String>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct CreateTurnServerRequest {
    pub urls: String,
    pub username: Option<String>,
    pub credential: Option<String>,
}

// --- WEBSOCKET SIGNALING TYPES ---

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum RtcSdpType {
    Offer,
    Answer,
    Pranswer,
    Rollback,
    Unspecified,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct RtcSessionDescription {
    pub ty: RtcSdpType,
    pub sdp: String,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct RtcIceCandidate {
    pub candidate: String,
    pub sdp_mid: Option<String>,
    pub sdp_mline_index: Option<u16>,
    pub username_fragment: Option<String>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct AppInfo {
    pub id: u32,
    pub title: String,
    pub icon_base64: Option<String>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct DisplayInfoMsg {
    pub id: String,
    pub name: String,
    pub width: u32,
    pub height: u32,
    pub refresh_rate: f64,
    pub is_primary: bool,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(tag = "type", content = "payload")]
pub enum SignalingMessage {
    RequestSession {
        host_id: String,
        width: Option<u32>,
        height: Option<u32>,
        fps: Option<u32>,
        bitrate: Option<u32>,
        codec: Option<String>,
        app_id: Option<u32>,
        encoder: Option<String>,
        display_id: Option<String>,
        virtual_display: Option<bool>,
    },
    // Server notifying agent about session request
    IncomingSession {
        client_id: String,
        width: Option<u32>,
        height: Option<u32>,
        fps: Option<u32>,
        bitrate: Option<u32>,
        codec: Option<String>,
        app_id: Option<u32>,
        encoder: Option<String>,
        display_id: Option<String>,
        virtual_display: Option<bool>,
        ice_servers: Option<Vec<RtcIceServer>>,
    },
    // SDP / ICE Exchange
    Sdp {
        target_id: String,
        sdp: RtcSessionDescription,
        ice_servers: Option<Vec<RtcIceServer>>,
        webtransport_port: Option<u16>,
        webtransport_cert_hash: Option<String>,
    },
    IceCandidate {
        target_id: String,
        candidate: RtcIceCandidate,
    },
    // Session termination
    EndSession {
        target_id: String,
    },
    // App List Query
    GetAppList {
        target_id: String,
    },
    AppListResponse {
        target_id: String,
        apps: Vec<AppInfo>,
        current_game_id: u32,
    },
    // Stop Session/Stream
    StopActiveStream {
        target_id: String,
    },
    StopActiveStreamResponse {
        target_id: String,
        success: bool,
        error: Option<String>,
    },
    // Host Capabilities Query
    GetCapabilities {
        target_id: String,
    },
    CapabilitiesResponse {
        target_id: String,
        displays: Vec<DisplayInfoMsg>,
        encoders: Vec<String>,
        gpu_info: Option<String>,
    },
    EncoderStatus {
        target_id: String,
        encoder: String,
        hw_type: String,
        gpu_info: Option<String>,
        requested_encoder: Option<String>,
    },
    // Errors
    Error {
        message: String,
    },
}

// Message from Agent to Server via WebSocket
#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(tag = "event", content = "data")]
pub enum AgentMessage {
    Register { id: String, name: String },
    StatusUpdate { status: HostStatus },
    Signaling(SignalingMessage),
}

// Message from Server to Agent via WebSocket
#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(tag = "event", content = "data")]
pub enum ServerToAgentMessage {
    Registered { success: bool },
    Signaling(SignalingMessage),
}

// Message from Client to Server via WebSocket
#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(tag = "event", content = "data")]
pub enum ClientMessage {
    Signaling(SignalingMessage),
}

// Message from Server to Client via WebSocket
#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(tag = "event", content = "data")]
pub enum ServerToClientMessage {
    Signaling(SignalingMessage),
}
