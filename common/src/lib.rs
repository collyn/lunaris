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
    /// The computer's own IPv4 addresses (LAN, VPN), as its agent last said
    /// them; kept while it is away. Left out when not known (agents from
    /// before this, computers added by address).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub addresses: Vec<String>,
    /// Added by address: the server connects to the computer itself
    /// ("host:port"), instead of the computer connecting to the server.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub address: Option<String>,
    /// Why the server can't reach a computer added by address.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub link_error: Option<String>,
    /// Added by address without its password: each connection has to be
    /// accepted by the person at that computer.
    #[serde(default)]
    pub needs_approval: bool,
}

/// Adds a computer by address: the server connects to its agent (direct
/// access port). Without the password the person at that computer accepts
/// each connection.
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct AddHostByAddressRequest {
    pub address: String,
    #[serde(default)]
    pub password: Option<String>,
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
    /// Two-factor sign-in (authenticator app) is on.
    #[serde(default)]
    pub mfa_enabled: bool,
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
    /// Members (filled by the group list; empty in create/update answers).
    #[serde(default)]
    pub users: Vec<UserBrief>,
    #[serde(default)]
    pub hosts: Vec<HostBrief>,
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

/// A computer a host can show besides its own screen. Only the name: what
/// it takes to connect to it (address, password) stays at the host.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct SourceInfoMsg {
    pub id: String,
    pub name: String,
    /// The connector's id ("rustdesk", ...), for an icon or a label.
    #[serde(default)]
    pub connector: String,
    /// Which computer it is, when it is one that is listed in other ways
    /// too ([`valid_source_group`]): sources with the same group are one
    /// computer, and `host:<id>` is the Lunaris computer with that id. An
    /// opaque word: nothing of where the computer is. Left out when it has
    /// none, and by peers from before groups.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group: Option<String>,
    /// Whether the computer itself is there, as its agent last checked
    /// (a connection to its port): `Some(false)` when it did not answer.
    /// Left out when that is not known (not checked yet, nothing to check),
    /// and by agents from before the checks: then it is listed as online
    /// while its agent is.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub online: Option<bool>,
    /// The fields of its form that are not secrets ("host", "port", "app",
    /// ...), as stored there: shown on its card and in an edit form. A
    /// password, and anything a set-up stored, is never among them. Left out
    /// by agents from before this, and where they are not to be told (a
    /// viewer's list of sources).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub values: Option<std::collections::BTreeMap<String, String>>,
}

/// The longest group of a source, in characters.
pub const MAX_SOURCE_GROUP: usize = 64;
/// How the group of a source that is a Lunaris computer begins; the rest is
/// that computer's id.
pub const SOURCE_GROUP_HOST: &str = "host:";

/// Whether `group` can be the group of a source: 1 to 64 of the letters
/// a-z and A-Z, the digits and `. _ : -`. It comes from other computers
/// and from forms, and is stored and shown to clients as it is.
pub fn valid_source_group(group: &str) -> bool {
    !group.is_empty()
        && group.len() <= MAX_SOURCE_GROUP
        && group.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b':' | b'-'))
}

/// What a connector can do, for the clients to offer only that (a Files
/// button only where there are files).
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ConnectorCapabilitiesMsg {
    #[serde(default)]
    pub audio: bool,
    #[serde(default)]
    pub input: bool,
    #[serde(default)]
    pub clipboard: bool,
    #[serde(default)]
    pub cursor: bool,
    #[serde(default)]
    pub files: bool,
    #[serde(default)]
    pub setup: bool,
}

/// A kind of computer a host can show through a connector, and what it
/// takes to add one (the fields of the form).
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct ConnectorInfoMsg {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub fields: Vec<ConnectorFieldMsg>,
    /// One or two plain sentences shown under the Type choice of "Add
    /// computer" (empty: nothing). Peers from before it leave it out.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub note: String,
    /// What it can do. Left out by agents from before this: then it is not
    /// known, and a client offers what it offered before (files included).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub capabilities: Option<ConnectorCapabilitiesMsg>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct ConnectorFieldMsg {
    pub key: String,
    pub label: String,
    /// "text", "port", "secret" or "choice" (one of `options`; a client
    /// from before choices shows it as text, and its value is typed).
    pub kind: String,
    #[serde(default)]
    pub default: String,
    #[serde(default)]
    pub required: bool,
    /// What a "choice" is chosen from. Left out when there is nothing to
    /// choose from, and by peers from before choices.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub options: Vec<FieldOptionMsg>,
}

/// One thing a "choice" field can be: the value kept for it, and what the
/// person reads.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct FieldOptionMsg {
    pub value: String,
    pub label: String,
}

/// What a server asks an agent to do with its sources. The fields of a
/// source (address, password) pass through the server once, in this
/// message, and are kept only by the agent.
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum SourceChange {
    /// `group`: the computer it is ([`SourceInfoMsg::group`]). Left out:
    /// the agent groups it with a source it has at the same address, if
    /// there is one; empty: with none.
    Add {
        name: String,
        connector: String,
        values: std::collections::BTreeMap<String, String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        group: Option<String>,
    },
    /// A field left out or empty keeps its value. `group` left out keeps
    /// the group too; empty takes the source out of its group.
    Update {
        id: String,
        name: String,
        values: std::collections::BTreeMap<String, String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        group: Option<String>,
    },
    Remove { id: String },
    /// Where the set-up that an `Add` started is: answered like `Add`
    /// (`setup` while it waits, `id` when the source is added, `error` when
    /// it failed). Only for agents that know set-ups (an older one cannot
    /// have started one).
    SetupState { setup: String },
    /// Stops that set-up.
    SetupCancel { setup: String },
}

/// What to show the person while the set-up of a source being added waits
/// for them (a PIN to type at the other computer).
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq)]
pub struct SetupInfo {
    /// The set-up's id, to ask about it and to cancel it.
    pub setup: String,
    /// In the connector's words.
    pub message: String,
    /// A code to show large (a PIN), when there is one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
}

// The code is for the person's eyes: it is never printed.
impl std::fmt::Debug for SetupInfo {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "SetupInfo {{ setup: {:?}, code: {} }}", self.setup, if self.code.is_some() { "(hidden)" } else { "none" })
    }
}

// The values include a password: they are never printed.
impl std::fmt::Debug for SourceChange {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SourceChange::Add { connector, values, .. } => write!(f, "Add {{ connector: {:?}, fields: {:?} }}", connector, values.keys().collect::<Vec<_>>()),
            SourceChange::Update { id, values, .. } => write!(f, "Update {{ id: {:?}, fields: {:?} }}", id, values.keys().collect::<Vec<_>>()),
            SourceChange::Remove { id } => write!(f, "Remove {{ id: {:?} }}", id),
            SourceChange::SetupState { setup } => write!(f, "SetupState {{ setup: {:?} }}", setup),
            SourceChange::SetupCancel { setup } => write!(f, "SetupCancel {{ setup: {:?} }}", setup),
        }
    }
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
        /// Chroma subsampling: "420" (default) or "444".
        #[serde(default, skip_serializing_if = "Option::is_none")]
        chroma: Option<String>,
        /// GameStream `encoderCscMode`: bit 0 = full range, bits 1.. = colourspace
        /// (0 Rec. 601, 1 Rec. 709, 2 Rec. 2020). None = the host's default.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        encoder_csc_mode: Option<i32>,
        /// Also record the host's microphone into the stream's sound.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        microphone: Option<bool>,
        /// The source to show: one of the host's `sources` (a computer it
        /// shows through a connector). None = the host's own screen.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        source_id: Option<String>,
        /// The video codecs the client can decode ("h264", "h265", "av1",
        /// "vp8", "vp9"). None = a client from before this field.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        decode_codecs: Option<Vec<String>>,
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
        /// Chroma subsampling requested by the client: "420" (default) or "444".
        #[serde(default, skip_serializing_if = "Option::is_none")]
        chroma: Option<String>,
        /// GameStream `encoderCscMode`: bit 0 = full range, bits 1.. = colourspace
        /// (0 Rec. 601, 1 Rec. 709, 2 Rec. 2020). None = the host's default.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        encoder_csc_mode: Option<i32>,
        /// Also record the host's microphone into the stream's sound.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        microphone: Option<bool>,
        /// The source to show: one of the host's `sources` (a computer it
        /// shows through a connector). None = the host's own screen.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        source_id: Option<String>,
        /// The video codecs the client can decode ("h264", "h265", "av1",
        /// "vp8", "vp9"). None = a client from before this field.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        decode_codecs: Option<Vec<String>>,
        ice_servers: Option<Vec<RtcIceServer>>,
        /// Who asks (user name, computer name), shown to the person at the
        /// host when the connection must be accepted there.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        requester: Option<String>,
    },
    /// Client: a file-transfer connection to `host_id`, separate from any
    /// stream (agent/src/files.rs). Sent on its own signaling connection.
    RequestFileSession {
        host_id: String,
        /// The source whose files are wanted: one of the host's `sources`
        /// (a computer it shows through a connector). None = the host's
        /// own files.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        source_id: Option<String>,
    },
    /// Server/hub -> agent: a client wants a file-transfer connection.
    IncomingFileSession {
        client_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        ice_servers: Option<Vec<RtcIceServer>>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        requester: Option<String>,
        /// The source whose files are wanted; None = the host's own files.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        source_id: Option<String>,
    },
    // SDP / ICE Exchange
    Sdp {
        target_id: String,
        sdp: RtcSessionDescription,
        ice_servers: Option<Vec<RtcIceServer>>,
        webtransport_port: Option<u16>,
        webtransport_cert_hash: Option<String>,
        /// Agent -> client: the secret path of the WebTransport URL
        /// (`https://host:port/<token>`); agents without it accept any path.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        webtransport_token: Option<String>,
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
        host_os: Option<String>,
        /// The other computers this host can show (through connectors).
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        sources: Vec<SourceInfoMsg>,
    },
    EncoderStatus {
        target_id: String,
        encoder: String,
        hw_type: String,
        gpu_info: Option<String>,
        requested_encoder: Option<String>,
        host_os: Option<String>,
        display_id: Option<String>,
        display_name: Option<String>,
    },
    /// Agent -> client: the connection waits for the person at the host to
    /// accept it (`status` "waiting"), or they did not ("denied", "timeout",
    /// "unattended": nobody there can accept, a password is needed).
    AccessRequest {
        target_id: String,
        status: String,
    },
    /// Agent -> client, about the source of the session (a computer shown
    /// through a connector): `status` is "failed" (it could not be
    /// connected to; no session follows) or "ended" (it closed; the session
    /// is over). `reason` is a text for the person. Clients, servers and
    /// agents from before this message drop it.
    SourceStatus {
        target_id: String,
        status: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reason: Option<String>,
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
    Register {
        id: String,
        name: String,
        /// Codecs the agent encodes (bits, as `codec_support` of /ws/agent).
        /// Sent by agents the server connects to (`/ws/server` of direct.rs).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        codec_support: Option<u32>,
    },
    StatusUpdate { status: HostStatus },
    Signaling(SignalingMessage),
    /// The other computers this agent shows and the kinds it can show, when
    /// it connects and whenever they change. Servers from before this
    /// message drop it.
    Sources {
        sources: Vec<SourceInfoMsg>,
        connectors: Vec<ConnectorInfoMsg>,
        /// Whether sources can be changed from the server (not on a
        /// computer that asks at its screen before each connection).
        #[serde(default)]
        manageable: bool,
        /// This computer's own IPv4 addresses (LAN, VPN), to show beside
        /// its name. Left out by agents from before this.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        addresses: Vec<String>,
    },
    /// The answer to `ServerToAgentMessage::ChangeSource`.
    SourceChanged {
        request_id: String,
        /// The source's id, when it worked.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        id: Option<String>,
        /// Why not, in words for the person, when it did not.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        error: Option<String>,
        /// The source's set-up waits for the person: what to show them.
        /// Servers from before set-ups do not read it (they then say the
        /// computer did not say whether it worked).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        setup: Option<SetupInfo>,
    },
}

// Message from Server to Agent via WebSocket
#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(tag = "event", content = "data")]
pub enum ServerToAgentMessage {
    Registered { success: bool },
    Signaling(SignalingMessage),
    /// Add, change or remove a source of this agent; answered with
    /// `AgentMessage::SourceChanged`. Agents from before this message drop
    /// it (the server then tells the person the computer is too old).
    ChangeSource {
        request_id: String,
        change: SourceChange,
        /// Who asked (user name), for the agent's log.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        requester: Option<String>,
    },
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_group_of_a_source_is_a_short_plain_word() {
        for good in ["g-1a2b3c4d5e6f", "host:0b7c1f0e-3d7a-4c57-9a0e-6f0f4c1d2e3f", "a", "Den_PC.2", &"x".repeat(MAX_SOURCE_GROUP)] {
            assert!(valid_source_group(good), "{}", good);
        }
        for bad in ["", " ", "a b", "<b>", "a\"b", "a'b", "a;b", "a/b", "a\nb", "nhóm", "a%b", &"x".repeat(MAX_SOURCE_GROUP + 1)] {
            assert!(!valid_source_group(bad), "{:?}", bad);
        }
    }

    // (How these messages look on the wire is tested where JSON is at hand:
    // server/src/sources.rs.)
    #[test]
    fn a_set_up_is_never_printed_with_its_code() {
        let info = SetupInfo { setup: "s1".into(), message: "Enter this PIN".into(), code: Some("4711".into()) };
        let answer = AgentMessage::SourceChanged { request_id: "r".into(), id: None, error: None, setup: Some(info.clone()) };
        let ask = ServerToAgentMessage::ChangeSource { request_id: "r".into(), change: SourceChange::SetupState { setup: "s1".into() }, requester: None };
        let cancel = SourceChange::SetupCancel { setup: "s1".into() };
        let printed = format!("{:?} {:?} {:?} {:?}", info, answer, ask, cancel);
        assert!(!printed.contains("4711") && !printed.contains("Enter this PIN"), "{}", printed);
        // Which set-up it is about can be read in a log.
        assert!(printed.contains("SetupState { setup: \"s1\" }") && printed.contains("SetupCancel { setup: \"s1\" }"), "{}", printed);
        assert!(format!("{:?}", SetupInfo { code: None, ..info }).contains("code: none"));
    }
}
