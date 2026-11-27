//! Connectors: the agent shows a computer that speaks another protocol (a
//! RustDesk host on the LAN, ...) to Lunaris clients, through the agent's
//! normal session. A connector logs in to that computer and hands the agent
//! what it sends, already encoded: the agent packs it into RTP as it is.
//!
//! One small interface, the same for every protocol:
//!
//! - [`Connector::describe`]: who it is, what a target needs (address,
//!   password, ...) and what it can deliver.
//! - [`Connector::connect`]: log in to a [`Target`] for a [`Viewer`]; gives a
//!   [`Session`].
//! - [`Event`]: what the session tells the agent (video frames, sound, the
//!   cursor, clipboard text, the displays, the end).
//! - [`Command`]: what the agent tells the session (pointer, keys, text,
//!   clipboard, "send a key frame", quality, display, close).
//!
//! Two ways to provide one:
//!
//! - built in: a Rust module implementing [`Connector`] (`rustdesk.rs`);
//! - external: a program in `<agent data dir>/connectors/<id>/` with a
//!   `connector.json` beside it, spoken to over its standard input and
//!   output (`process.rs`; the messages are these same events and commands,
//!   one to one). `connectors/template/` in the repository is a minimal one
//!   and its README the protocol's reference.
//!
//! Files are a part of their own (`files.rs`): [`Connector::open_files`]
//! logs in to a target for its files and gives a [`files::FileSession`],
//! which the agent's file sessions serve to the Lunaris file manager. A
//! connector that has none leaves it out.
//!
//! Adding a target can have a set-up of its own ([`Connector::setup`]: a
//! pairing, with a PIN the person types at the other computer). What it
//! returns is kept with the target and never shown (`sources.rs`).
//!
//! An external connector is a program the user chose to run with their own
//! rights. Nothing installs or downloads one: it exists when the user put
//! its folder there, and it is started only for a target that names it.

pub mod cursor_contrast;
pub mod files;
pub mod keys;
pub mod media;
pub mod process;
pub mod raw_video;
pub mod rdp;
pub mod reachability;
pub mod rustdesk;
pub mod rustdesk_files;
pub mod sources;
pub mod sunshine;
pub mod vnc;

use std::collections::BTreeMap;
use std::sync::Arc;

use bytes::Bytes;
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;

/// Version of the interface (and of the external connectors' protocol).
pub const VERSION: u32 = 1;

/// A video codec a connector can deliver and a viewer can decode.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Codec {
    H264,
    H265,
    Vp8,
    Vp9,
    Av1,
}

/// One thing a target is described by, for a form.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Field {
    /// The key in [`Target::values`].
    pub key: String,
    pub label: String,
    /// "text", "port", "secret" (a secret is never shown or logged) or
    /// "choice": one of `options`, shown as that many options to press. Its
    /// value is text like any other (an older client shows a text box for
    /// it), and it is the connector that says what it takes.
    pub kind: String,
    #[serde(default)]
    pub default: String,
    #[serde(default)]
    pub required: bool,
    /// What a "choice" is chosen from: the value kept and its label.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub options: Vec<common::FieldOptionMsg>,
}

/// What a connector can deliver and take.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Capabilities {
    pub video: Vec<Codec>,
    #[serde(default)]
    pub audio: bool,
    #[serde(default)]
    pub input: bool,
    #[serde(default)]
    pub clipboard: bool,
    #[serde(default)]
    pub cursor: bool,
    /// The target's files can be browsed and copied ([`Connector::open_files`]).
    #[serde(default)]
    pub files: bool,
    /// Adding a target has a set-up of its own ([`Connector::setup`]).
    #[serde(default)]
    pub setup: bool,
}

/// Something to show the person while a set-up waits for them.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SetupStep {
    pub message: String,
    /// A code to show large (a PIN), when there is one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Description {
    /// Short and stable: letters, digits, `-` and `_`. Targets name it.
    pub id: String,
    pub name: String,
    pub fields: Vec<Field>,
    pub capabilities: Capabilities,
    /// One or two plain sentences for the person, shown under the Type
    /// choice of "Add computer" (what must be there before adding one).
    #[serde(default)]
    pub note: String,
}

/// A computer to show: which connector, and its values for the
/// connector's fields (address, port, password, ...).
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Target {
    pub connector: String,
    #[serde(flatten)]
    pub values: BTreeMap<String, String>,
}

impl Target {
    pub fn get(&self, key: &str) -> &str {
        self.values.get(key).map(String::as_str).unwrap_or("")
    }
}

// The values include the password: only the keys are ever printed.
impl std::fmt::Debug for Target {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Target {{ connector: {:?}, fields: {:?} }}", self.connector, self.values.keys().collect::<Vec<_>>())
    }
}

/// A source of this agent: a computer it shows besides its own screen. In
/// the agent's config as `{"id": ..., "name": ..., "connector": ..., and the
/// connector's fields}`. Only the id, the name and the group leave this computer.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceConfig {
    pub id: String,
    pub name: String,
    /// Which computer it is, when that computer is listed in other ways
    /// too (`common::SourceInfoMsg::group`); an opaque word, told with the
    /// id and the name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group: Option<String>,
    #[serde(flatten)]
    pub target: Target,
}

/// Why a source could not be shown, in words for the viewer (the
/// connector's own: wrong password, not reachable, ...).
#[derive(Debug)]
pub struct SourceError(pub String);

impl std::fmt::Display for SourceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for SourceError {}

/// What the viewer at the other end of the agent's session can take.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Viewer {
    /// The codecs it decodes, the preferred one first. The connector must
    /// deliver one of them (the agent does not transcode).
    pub codecs: Vec<Codec>,
    pub fps: u32,
    /// The picture's size the viewer asked for, in pixels; 0 = no wish. For
    /// a connector whose computer sends in the size a client asks for.
    #[serde(default)]
    pub width: u32,
    #[serde(default)]
    pub height: u32,
    /// The bitrate the viewer asked for, in kbit/s; 0 = no wish. For a
    /// connector whose protocol fixes it when the session starts (later
    /// wishes come as `Command::Quality`).
    #[serde(default)]
    pub bitrate_kbps: u32,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DisplayInfo {
    pub name: String,
    pub width: u32,
    pub height: u32,
}

/// A mouse button.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Button {
    Left,
    Middle,
    Right,
    Back,
    Forward,
}

/// What a session tells the agent.
#[derive(Clone, Debug, PartialEq)]
pub enum Event {
    /// One encoded picture (an access unit; H.264 and H.265 as Annex-B with
    /// parameter sets in key frames). `capture_time_ms` is the protocol's
    /// own time for it when it has one, on a clock that only has to move
    /// steadily forward.
    Video { codec: Codec, key: bool, capture_time_ms: Option<u64>, data: Bytes },
    /// One Opus packet (48 kHz).
    Audio { channels: u8, data: Bytes },
    /// The cursor's picture: RGBA, `width * height * 4` bytes.
    CursorShape { width: u32, height: u32, hot_x: u32, hot_y: u32, rgba: Bytes },
    CursorPosition { x: i32, y: i32, visible: bool },
    /// The pointer is part of the picture now (`true`), so a viewer that
    /// draws a pointer of its own would show two; or no longer (`false`).
    CursorEmbedded { embedded: bool },
    /// The pointer *may* be part of the picture now (`true`): the connector
    /// cannot tell, only that it looks like a moment in which it would be
    /// (a window being moved). The viewer keeps its own pointer and looks
    /// for the computer's in the picture; it hides its own only if it finds
    /// it. `false`: the moment is over.
    CursorMaybeEmbedded { maybe: bool },
    /// The pointer is part of the picture for the whole session, and no
    /// shape or position of it will come (a computer that always draws its
    /// pointer into what it sends). Said once, at the start. The viewer
    /// shows no pointer at all over the picture, neither one it draws nor
    /// its own system's: the only pointer is the one in the picture.
    /// Unlike `CursorEmbedded` it does not depend on the kind of computer
    /// or on a button being held.
    CursorAlwaysEmbedded,
    /// What kind of computer it is: "windows", "linux", "macos", "android"
    /// (viewers treat the pointer differently by it).
    Platform { os: String },
    /// The delay between the agent and the computer, there and back, in
    /// milliseconds (fractions for what is under a millisecond), as the
    /// connector's protocol measures it (a round trip: a protocol that only
    /// knows one way doubles it). Told when it is known and again when it
    /// changes, about once a second at most. The viewer adds it to its own
    /// round trip to the agent and shows the sum as the ping. Never told =
    /// nothing is added (Lunaris's own event; no protocol is asked to have
    /// it). Never negative, never not a number.
    Latency { ms: f32 },
    /// The remote computer's clipboard text changed.
    Clipboard { text: String },
    /// The clipboard cannot be shared (now): the reason, for the viewer
    /// (the other computer does not allow it, the text is too large).
    ClipboardError { text: String },
    /// The displays there are and the one shown.
    Displays { displays: Vec<DisplayInfo>, current: usize },
    /// The end; nothing follows.
    Closed { reason: String },
}

/// What the agent tells a session.
#[derive(Clone, Debug, PartialEq)]
pub enum Command {
    /// The pointer, in pixels of the display shown.
    PointerMove { x: i32, y: i32 },
    /// The pointer moved by this much (a viewer that holds the pointer, as
    /// games want it).
    PointerMoveRelative { dx: i32, dy: i32 },
    PointerButton { button: Button, down: bool },
    /// Wheel notches: `dy` > 0 up, `dx` > 0 right.
    Wheel { dx: i32, dy: i32 },
    /// A key by where it is on the keyboard: the Linux input key code
    /// (`KEY_A` = 30, linux/input-event-codes.h).
    Key { position: u32, down: bool },
    /// Text to type as it is.
    Text { text: String },
    /// The viewer's clipboard text changed.
    Clipboard { text: String },
    /// The viewer cannot go on without a whole picture.
    RequestKeyFrame,
    /// What the viewer would like; 0 = no wish.
    Quality { fps: u32, bitrate_kbps: u32 },
    SwitchDisplay { index: usize },
    /// A game controller of the viewer, its whole state (gamepad.rs);
    /// `state` = `None`: unplugged. `kind`: 0 unknown, 1 Xbox,
    /// 2 PlayStation, 3 Nintendo. Connectors without controllers ignore it.
    Gamepad { id: u8, kind: u8, state: Option<crate::gamepad::PadState> },
    Close,
}

/// A connection to a target. Dropping it (or `Command::Close`) ends it.
pub struct Session {
    pub events: mpsc::Receiver<Event>,
    pub commands: mpsc::Sender<Command>,
}

#[async_trait::async_trait]
pub trait Connector: Send + Sync {
    fn describe(&self) -> Description;

    /// Logs in to `target`. An error is a text for the person who asked
    /// (wrong password, not reachable, ...).
    async fn connect(&self, target: &Target, viewer: &Viewer) -> anyhow::Result<Session>;

    /// Logs in to `target` for its files: a connection of its own, beside
    /// any picture session. An error is a text for the person who asked. A
    /// connector that has no files leaves this as it is.
    async fn open_files(&self, _target: &Target) -> anyhow::Result<files::FileSession> {
        anyhow::bail!(files::NO_FILES)
    }

    /// Done once when a target is added, when `capabilities.setup` is true:
    /// may need the person (a PIN to type at the other computer). Each step
    /// to show goes to `steps`. Returns values to keep with the target
    /// (secrets included; they never leave the agent). An error is a text
    /// for the person who asked.
    async fn setup(
        &self,
        _target: &Target,
        _steps: mpsc::Sender<SetupStep>,
    ) -> anyhow::Result<BTreeMap<String, String>> {
        Ok(BTreeMap::new())
    }
}

/// The connectors built into the agent.
pub fn builtin() -> Vec<Arc<dyn Connector>> {
    vec![
        Arc::new(rustdesk::RustdeskConnector),
        Arc::new(sunshine::SunshineConnector::new()),
        Arc::new(rdp::RdpConnector::new()),
        Arc::new(vnc::VncConnector::new()),
    ]
}

/// Where external connectors live: `connectors/` beside the agent's config.
pub fn connectors_dir() -> std::path::PathBuf {
    std::path::Path::new(crate::pairing::config_path())
        .parent()
        .map(|dir| dir.join("connectors"))
        .unwrap_or_else(|| "connectors".into())
}

/// The connector a target names: a built-in one, else the external one in
/// `dir/<id>/`. Nothing is run by looking one up.
pub fn find(id: &str, dir: &std::path::Path) -> anyhow::Result<Arc<dyn Connector>> {
    if let Some(connector) = builtin().into_iter().find(|c| c.describe().id == id) {
        return Ok(connector);
    }
    Ok(Arc::new(process::ProcessConnector::load(dir, id)?))
}

/// The sources in the agent's config file, as it is now (so that one added
/// in the settings is there for the next session without a restart).
pub fn configured_sources() -> Vec<SourceConfig> {
    crate::pairing::load_config(crate::pairing::config_path())
        .map(|config| config.sources)
        .unwrap_or_default()
        .into_iter()
        .filter(|source| !source.id.is_empty())
        .collect()
}

/// Every connector there is to choose from: the built-in ones and the ones
/// installed in `dir` (their manifests are read; nothing is run).
pub fn available(dir: &std::path::Path) -> Vec<Description> {
    let mut all: Vec<Description> = builtin().iter().map(|c| c.describe()).collect();
    let mut installed: Vec<Description> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|entry| entry.file_name().into_string().ok())
        .filter_map(|id| process::ProcessConnector::load(dir, &id).ok())
        .map(|connector| connector.describe())
        .filter(|d| !all.iter().any(|known| known.id == d.id))
        .collect();
    installed.sort_by(|a, b| a.id.cmp(&b.id));
    all.extend(installed);
    all
}

/// Whether `id` is fit to be a folder's name and nothing else.
pub fn valid_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 64 && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}
