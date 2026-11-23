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
    /// The name its agent announces, when an administrator gave it another
    /// one (`name` is then that one).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_name: Option<String>,
    /// The server's own gateway (server/src/gateway.rs): the agent the
    /// server runs itself, which shows the computers added through it and
    /// never a screen of its own. Not deleted or edited like a computer.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub gateway: bool,
    /// What its agent is (version, OS, how it was installed), as it last
    /// said; left out when not known (agents from before this).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<AgentBuild>,
}

/// Adds a computer by address: the server connects to its agent (direct
/// access port). Without the password the person at that computer accepts
/// each connection.
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct AddHostByAddressRequest {
    pub address: String,
    #[serde(default)]
    pub password: Option<String>,
    /// The name it is listed with; none or empty: the one its agent announces.
    #[serde(default)]
    pub name: Option<String>,
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

// --- A LUNARIS COMPUTER'S OWN SETTINGS (server administrators) ---

/// A secret passed through once (an access password): written on the wire,
/// never in a log (`Debug` hides it).
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Default)]
#[serde(transparent)]
pub struct SecretString(pub String);

impl std::fmt::Debug for SecretString {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("(hidden)")
    }
}

/// A Lunaris computer's own settings (its agent's config), as its agent
/// tells them. Never the access password: that is asked for on its own
/// (`SettingsRequest::ShowPassword`).
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, Default)]
pub struct AgentSettingsMsg {
    /// The person at that computer lets the server change these
    /// ("Allow the server to change this computer's settings", off by
    /// default). Never changed from the server. When off, the values are
    /// for reading only.
    #[serde(default)]
    pub remote_changes: bool,
    #[serde(default)]
    pub direct_enabled: bool,
    #[serde(default)]
    pub direct_port: u16,
    /// Whether an access password is set (it is never told here).
    #[serde(default)]
    pub has_password: bool,
    /// "password", "click" or "password-click".
    #[serde(default)]
    pub approve_mode: String,
    /// Sessions through the server: "trusted" or "ask".
    #[serde(default)]
    pub server_sessions: String,
    #[serde(default)]
    pub allow_clipboard: bool,
    #[serde(default)]
    pub allow_file_transfer: bool,
    /// What file transfer may reach: "home", "full" or an absolute folder
    /// (as in effect there).
    #[serde(default)]
    pub file_access: String,
    #[serde(default)]
    pub rustdesk_enabled: bool,
    #[serde(default)]
    pub rustdesk_port: u16,
    #[serde(default)]
    pub rustdesk_file_transfer: bool,
    /// Screen capture on a Linux Wayland session: "auto" or "portal".
    #[serde(default)]
    pub wayland_capture: String,
    /// Read only: remote keyboard and mouse can work (Linux: /dev/uinput).
    #[serde(default)]
    pub input_ready: bool,
    /// Read only: the agent's version.
    #[serde(default)]
    pub version: String,
    /// Read only: "linux", "windows", "macos".
    #[serde(default)]
    pub os: String,
    /// Read only: the change came over a server that connects to this
    /// computer by its address; its direct access (on, port, password)
    /// is then not changed from there (that would cut the server off).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub linked_by_address: bool,
}

/// What to change of a computer's settings; a field left out keeps its
/// value. There is no field for `remote_changes`: it is only ever changed
/// at that computer.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, Default)]
pub struct AgentSettingsChange {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub direct_enabled: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub direct_port: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub approve_mode: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub server_sessions: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub allow_clipboard: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub allow_file_transfer: Option<bool>,
    /// "home" or "full".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file_access: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rustdesk_enabled: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rustdesk_port: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rustdesk_file_transfer: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wayland_capture: Option<String>,
}

pub const APPROVE_MODES: [&str; 3] = ["password", "click", "password-click"];
pub const SERVER_SESSION_MODES: [&str; 2] = ["trusted", "ask"];
pub const WAYLAND_CAPTURE_MODES: [&str; 2] = ["auto", "portal"];
pub const FILE_ACCESS_CHOICES: [&str; 2] = ["home", "full"];
/// Ports a setting may use (as the apps take them).
pub const MIN_SETTINGS_PORT: u16 = 1024;
/// The fewest characters of an access password set by hand. It is checked
/// only when a password is set: one saved before (6 characters were enough
/// then) keeps working.
pub const MIN_ACCESS_PASSWORD_CHARS: usize = 12;
pub const MAX_ACCESS_PASSWORD_CHARS: usize = 128;

/// An access password set by hand: 12 to 128 characters, no control
/// characters, no space at either end.
pub fn valid_access_password(password: &str) -> bool {
    let count = password.chars().count();
    (MIN_ACCESS_PASSWORD_CHARS..=MAX_ACCESS_PASSWORD_CHARS).contains(&count)
        && password.trim() == password
        && !password.chars().any(char::is_control)
}

/// Why a settings change is not valid (the same rules at the server and the
/// agent), in words for the person; `current_direct_port` is the port in
/// effect when the change leaves it out.
pub fn settings_change_error(change: &AgentSettingsChange, current_direct_port: u16) -> Option<String> {
    let port_ok = |p: u16| p >= MIN_SETTINGS_PORT;
    if change.direct_port.is_some_and(|p| !port_ok(p)) || change.rustdesk_port.is_some_and(|p| !port_ok(p)) {
        return Some("Use a port from 1024 to 65535".into());
    }
    if let (Some(rustdesk), true) = (change.rustdesk_port, change.rustdesk_enabled != Some(false)) {
        if rustdesk == change.direct_port.unwrap_or(current_direct_port) {
            return Some("That port is the one for direct connections".into());
        }
    }
    let one_of = |value: &Option<String>, allowed: &[&str]| value.as_deref().is_none_or(|v| allowed.contains(&v));
    if !one_of(&change.approve_mode, &APPROVE_MODES) {
        return Some("Unknown way of accepting connections".into());
    }
    if !one_of(&change.server_sessions, &SERVER_SESSION_MODES) {
        return Some("Unknown choice for sessions through the server".into());
    }
    if !one_of(&change.wayland_capture, &WAYLAND_CAPTURE_MODES) {
        return Some("Unknown Wayland capture mode".into());
    }
    if !one_of(&change.file_access, &FILE_ACCESS_CHOICES) {
        return Some("File access is \"home\" or \"full\"".into());
    }
    None
}

/// What a server asks of a computer's own settings (server administrators).
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum SettingsRequest {
    /// The settings (`AgentSettingsMsg`).
    Get,
    /// Change some (only when the person there allows it).
    Change { change: AgentSettingsChange },
    /// The access password, to show (only when the person there allows
    /// changes from the server).
    ShowPassword,
    /// A new access password: this one, or a new random one when left out.
    /// Answered with the settings, not the password.
    NewPassword {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        password: Option<SecretString>,
    },
}

/// Why an agent did not do what was asked of its settings.
pub const SETTINGS_NOT_ALLOWED: &str = "not_allowed";
pub const SETTINGS_INVALID: &str = "invalid";
pub const SETTINGS_FAILED: &str = "failed";

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
        /// The picture's size. A decoder may give a taller (or wider) frame:
        /// H.265 encoders round it up to their block size (1080 -> 1088)
        /// and say what to crop, which Chrome's D3D11 H.265 decoder on
        /// Windows ignores, showing the extra lines (green). The viewer
        /// shows only this much. None from older agents and connectors.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        width: Option<u32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        height: Option<u32>,
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

/// An agent's version and how it was installed, so the server can show it
/// and say how to update it.
#[derive(Serialize, Deserialize, Debug, Clone, Default, PartialEq, Eq)]
pub struct AgentBuild {
    pub version: String,
    /// "linux", "windows", "macos".
    pub os: String,
    /// How it was installed (see the `INSTALL_*` values).
    pub install: String,
    /// Windows: whether its game mouse can be plugged in (`GAME_MOUSE_*`),
    /// for mouse input that anti-cheats accept; absent elsewhere and from
    /// agents before 0.7.8.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub game_mouse: Option<String>,
}

/// usbip-win2 is installed: the agent plugs its USB game mouse in.
pub const GAME_MOUSE_USBIP: &str = "usbip";
/// usbip-win2 is not installed: injected input only.
pub const GAME_MOUSE_NONE: &str = "none";

/// The .deb package (apt): `agent.sh --update` updates it.
pub const INSTALL_DEB: &str = "deb";
/// The AppImage the Linux installer put in /opt/lunaris-agent: `agent.sh --update`.
pub const INSTALL_APPIMAGE: &str = "appimage";
/// The Windows installer (Program Files, the service): `agent.ps1 -Update`.
pub const INSTALL_WINDOWS: &str = "windows";
/// Another package manager (e.g. Arch): updated with it.
pub const INSTALL_PACKAGE: &str = "package";
/// Run from wherever it was put (an AppImage elsewhere, a build): updated by hand.
pub const INSTALL_MANUAL: &str = "manual";

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
        /// The agent streams to several clients at once (each its own
        /// session); older agents run one, which a new session replaces.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        multi_viewer: Option<bool>,
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
        /// What this agent is: its version and how it was installed. Left
        /// out by agents from before this.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        build: Option<AgentBuild>,
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
    /// The answer to `ServerToAgentMessage::Settings`: the settings (after
    /// a change, as they are now), the password when it was asked for, or
    /// why not (`error`, and `code`: `SETTINGS_NOT_ALLOWED`,
    /// `SETTINGS_INVALID`, `SETTINGS_FAILED`). Servers from before this
    /// message drop it.
    SettingsAnswer {
        request_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        settings: Option<AgentSettingsMsg>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        password: Option<SecretString>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        error: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        code: Option<String>,
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
    /// Read or change this computer's own settings; answered with
    /// `AgentMessage::SettingsAnswer`. Agents from before this message
    /// drop it (the server then times out and says the agent is too old).
    Settings {
        request_id: String,
        request: SettingsRequest,
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
    #[test]
    fn settings_messages_on_the_wire_and_from_older_peers() {
        // An older agent's view of a newer server's message: unknown, so it
        // fails to parse (the agent logs it without the text and goes on).
        let ask = ServerToAgentMessage::Settings { request_id: "r".into(), request: SettingsRequest::Get, requester: None };
        let wire = serde_json::to_string(&ask).unwrap();
        assert_eq!(wire, r#"{"event":"Settings","data":{"request_id":"r","request":{"op":"get"}}}"#);
        let back: ServerToAgentMessage = serde_json::from_str(&wire).unwrap();
        assert!(matches!(back, ServerToAgentMessage::Settings { request: SettingsRequest::Get, .. }));

        // A change with only some fields; unknown fields (a newer server's,
        // or one trying `remote_changes`) are ignored.
        let wire = r#"{"event":"Settings","data":{"request_id":"r","request":{"op":"change","change":{"direct_port":9000,"remote_changes":true,"future":1}}}}"#;
        let ServerToAgentMessage::Settings { request: SettingsRequest::Change { change }, .. } = serde_json::from_str(wire).unwrap() else { panic!() };
        assert_eq!(change, AgentSettingsChange { direct_port: Some(9000), ..Default::default() });

        // An answer from an agent that knows fewer fields of the settings.
        let wire = r#"{"event":"SettingsAnswer","data":{"request_id":"r","settings":{"direct_enabled":true}}}"#;
        let AgentMessage::SettingsAnswer { settings: Some(s), password: None, error: None, .. } = serde_json::from_str(wire).unwrap() else { panic!() };
        assert!(s.direct_enabled && !s.remote_changes && s.version.is_empty());

        // Older messages still read the same.
        let wire = r#"{"event":"SourceChanged","data":{"request_id":"r","id":"x"}}"#;
        assert!(matches!(serde_json::from_str::<AgentMessage>(wire).unwrap(), AgentMessage::SourceChanged { .. }));
    }

    #[test]
    fn a_password_is_never_printed() {
        let set = ServerToAgentMessage::Settings {
            request_id: "r".into(),
            request: SettingsRequest::NewPassword { password: Some(SecretString("hunter22".into())) },
            requester: Some("admin".into()),
        };
        let answer = AgentMessage::SettingsAnswer { request_id: "r".into(), settings: None, password: Some(SecretString("hunter22".into())), error: None, code: None };
        let printed = format!("{:?} {:?}", set, answer);
        assert!(!printed.contains("hunter22"), "{}", printed);
        // But it is on the wire.
        assert!(serde_json::to_string(&set).unwrap().contains(r#""password":"hunter22""#));
    }

    #[test]
    fn settings_changes_are_checked() {
        let ok = AgentSettingsChange { direct_port: Some(9000), approve_mode: Some("click".into()), ..Default::default() };
        assert_eq!(settings_change_error(&ok, 8091), None);
        for bad in [
            AgentSettingsChange { direct_port: Some(80), ..Default::default() },
            AgentSettingsChange { rustdesk_port: Some(0), ..Default::default() },
            AgentSettingsChange { rustdesk_port: Some(8091), ..Default::default() },
            AgentSettingsChange { direct_port: Some(9000), rustdesk_port: Some(9000), rustdesk_enabled: Some(true), ..Default::default() },
            AgentSettingsChange { approve_mode: Some("always".into()), ..Default::default() },
            AgentSettingsChange { server_sessions: Some("never".into()), ..Default::default() },
            AgentSettingsChange { wayland_capture: Some("kms".into()), ..Default::default() },
            AgentSettingsChange { file_access: Some("/etc".into()), ..Default::default() },
        ] {
            assert!(settings_change_error(&bad, 8091).is_some(), "{:?}", bad);
        }
        // RustDesk turned off: its port is not in the way.
        assert_eq!(settings_change_error(&AgentSettingsChange { rustdesk_port: Some(8091), rustdesk_enabled: Some(false), ..Default::default() }, 8091), None);
        assert!(valid_access_password("abcd-efgh-jkm"));
        assert!(valid_access_password(&"x".repeat(12)) && valid_access_password(&"x".repeat(128)));
        // What 6 characters used to allow is too short for a new password now.
        for bad in ["short", "abc-def", &"x".repeat(11), " padded-password ", "tab\there-and-more", &"x".repeat(129)] {
            assert!(!valid_access_password(bad), "{:?}", bad);
        }
    }
}
