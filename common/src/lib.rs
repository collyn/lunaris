use serde::{Deserialize, Serialize};

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
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct PairHostRequest {
    pub name: String,
    pub ip_address: String,
    pub sunshine_username: Option<String>,
    pub sunshine_password: Option<String>,
}



#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct HostInfo {
    pub id: String,
    pub name: String,
    pub status: HostStatus,
    pub ip_address: Option<String>,
    pub server_codec_mode_support: Option<u32>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub enum HostStatus {
    Online,
    Offline,
    Busy,
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
    },
    // Server notifying agent about session request
    IncomingSession {
        client_id: String,
        width: Option<u32>,
        height: Option<u32>,
        fps: Option<u32>,
        bitrate: Option<u32>,
        codec: Option<String>,
    },
    // SDP / ICE Exchange
    Sdp { target_id: String, sdp: RtcSessionDescription },
    IceCandidate { target_id: String, candidate: RtcIceCandidate },
    // Session termination
    EndSession { target_id: String },
    // Sunshine Config Exchange
    GetSunshineConfig { target_id: String },
    SunshineConfigResponse { target_id: String, config: String },
    UpdateSunshineConfig { target_id: String, config: String },
    UpdateSunshineConfigResponse { target_id: String, success: bool, error: Option<String> },
    // Errors
    Error { message: String },
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
