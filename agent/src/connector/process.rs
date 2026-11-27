//! External connectors: a program in `<connectors dir>/<id>/`, described by
//! the `connector.json` beside it, started for one session and spoken to over
//! its standard input and output.
//!
//! The protocol (reference: `connectors/template/README.md`): each message is
//!
//! ```text
//! u32 LE  length of the header
//! u32 LE  length of the payload
//! header  JSON object with a "type"
//! payload bytes (video, sound, cursor pixels, text), may be empty
//! ```
//!
//! and the headers are the agent's `Event`s and `Command`s, one to one
//! (`EventHeader`, `CommandHeader` below). First the agent sends `hello` with
//! its version and the connector answers `hello` with its own; then the
//! agent sends `connect` with the target and the viewer. What the program
//! writes to its standard error goes to the agent's log.
//!
//! A target's files are a session of their own, in a program started for
//! it: after `hello` the agent sends `open_files` (not `connect`), the
//! connector answers `files_ready`, and the headers are the agent's
//! `FileCommand`s and `FileEvent`s (`connector/files.rs`), one to one, as
//! `file_...` messages. Only a connector whose manifest says
//! `"files": true` in its capabilities is ever asked.
//!
//! A set-up (a pairing, done once when a target is added) is a run of its
//! own too: after `hello` the agent sends `setup` with the target, the
//! connector sends `setup_step` (a text and maybe a code to show the
//! person) as often as it wants, then `setup_done` with the values to keep
//! with the target, or `closed` with the reason. Only a connector whose
//! manifest says `"setup": true` in its capabilities is ever asked.

use std::path::{Path, PathBuf};
use std::process::Stdio;

use bytes::Bytes;
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::sync::mpsc;
use tracing::{debug, warn};

use super::files::{self, FileCommand, FileEntry, FileEvent, FileSession, FilesInfo, OnExists, Place, WalkFile};
use super::{Button, Capabilities, Codec, Command, Connector, Description, DisplayInfo, Event, Field, Session, SetupStep, Target, Viewer, VERSION};

/// A header or a payload larger than this ends the session (a 4K key frame
/// is a few megabytes).
const MAX_HEADER: usize = 64 * 1024;
const MAX_PAYLOAD: usize = 64 * 1024 * 1024;
const HELLO_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);
/// How long a connector has to log in to a target for its files.
const FILES_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

/// `connector.json`: what the connector is and how it is started.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Manifest {
    pub id: String,
    pub name: String,
    /// The program, a path inside the connector's own folder.
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub fields: Vec<Field>,
    #[serde(default)]
    pub capabilities: Capabilities,
    /// One or two plain sentences shown under the Type choice of "Add computer".
    #[serde(default)]
    pub note: String,
}

/// What the agent sends: `hello`, `connect`, then the commands.
#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum CommandHeader {
    Hello { version: u32 },
    Connect { target: Target, viewer: Viewer },
    PointerMove { x: i32, y: i32 },
    PointerMoveRelative { dx: i32, dy: i32 },
    PointerButton { button: Button, down: bool },
    Wheel { dx: i32, dy: i32 },
    Key { position: u32, down: bool },
    /// Payload: the text, UTF-8.
    Text,
    /// Payload: the text, UTF-8.
    Clipboard,
    RequestKeyFrame,
    Quality { fps: u32, bitrate_kbps: u32 },
    SwitchDisplay { index: usize },
    /// In the place of `connect`: a session for the target's files. The
    /// `file_...` commands follow, and `close`.
    OpenFiles { target: Target },
    FileList {
        id: String,
        path: String,
        #[serde(default)]
        hidden: bool,
    },
    FileStat { id: String, path: String },
    FileWalk { id: String, path: String },
    FileMkdir { id: String, path: String },
    FileRemove { id: String, path: String },
    FileRename { id: String, path: String, name: String },
    FileRead {
        id: String,
        path: String,
        #[serde(default)]
        offset: u64,
    },
    FileWrite {
        id: String,
        path: String,
        size: u64,
        #[serde(default)]
        mtime: u64,
        #[serde(default)]
        on_exists: OnExists,
    },
    /// Payload: the next bytes of the file.
    FileWriteData { id: String },
    FileWriteEnd { id: String },
    FileCancel { id: String },
    /// In the place of `connect`: the set-up of a target being added.
    Setup { target: Target },
    Close,
}

/// What the connector sends: `hello`, then the events.
#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum EventHeader {
    Hello { version: u32 },
    /// Payload: the access unit.
    Video {
        codec: Codec,
        key: bool,
        #[serde(default)]
        capture_time_ms: Option<u64>,
    },
    /// Payload: the Opus packet.
    Audio { channels: u8 },
    /// Payload: RGBA pixels.
    CursorShape { width: u32, height: u32, hot_x: u32, hot_y: u32 },
    CursorPosition { x: i32, y: i32, visible: bool },
    CursorEmbedded { embedded: bool },
    CursorMaybeEmbedded { maybe: bool },
    CursorAlwaysEmbedded,
    Platform { os: String },
    /// The delay to the computer and back, in milliseconds, whole or with
    /// a fraction (`Event::Latency`).
    Latency { ms: f32 },
    /// Payload: the text, UTF-8.
    Clipboard,
    /// Payload: the reason, UTF-8.
    ClipboardError,
    Displays { displays: Vec<DisplayInfo>, current: usize },
    /// The answer to `open_files`: the target's files can be asked for.
    FilesReady {
        #[serde(default)]
        os: String,
        sep: String,
        #[serde(default)]
        home: Option<String>,
        #[serde(default)]
        roots: Vec<Place>,
    },
    FileDir {
        id: String,
        path: String,
        #[serde(default)]
        parent: Option<String>,
        entries: Vec<FileEntry>,
    },
    FileStat {
        id: String,
        #[serde(default)]
        entry: Option<FileEntry>,
    },
    FileWalk {
        id: String,
        path: String,
        files: Vec<WalkFile>,
        #[serde(default)]
        dirs: Vec<String>,
    },
    FileOk { id: String },
    FileReadStart {
        id: String,
        size: u64,
        #[serde(default)]
        mtime: u64,
        #[serde(default)]
        offset: u64,
    },
    /// Payload: the next bytes of the file.
    FileReadData { id: String },
    FileReadEnd {
        id: String,
        #[serde(default)]
        cancelled: bool,
    },
    FileWriteReady {
        id: String,
        #[serde(default)]
        offset: u64,
        path: String,
    },
    FileWriteSkip { id: String, reason: String, path: String },
    FileWriteDone { id: String, path: String },
    FileError { id: String, message: String },
    /// Of a set-up: something to show the person (a PIN as `code`).
    SetupStep {
        message: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        code: Option<String>,
    },
    /// The set-up's end: the values to keep with the target.
    SetupDone {
        #[serde(default)]
        values: std::collections::BTreeMap<String, String>,
    },
    Closed { reason: String },
}

/// `None` for what the wire has no word for (game controllers).
pub fn command_to_wire(command: &Command) -> Option<(CommandHeader, Bytes)> {
    Some(match command {
        Command::PointerMove { x, y } => (CommandHeader::PointerMove { x: *x, y: *y }, Bytes::new()),
        Command::PointerMoveRelative { dx, dy } => (CommandHeader::PointerMoveRelative { dx: *dx, dy: *dy }, Bytes::new()),
        Command::PointerButton { button, down } => (CommandHeader::PointerButton { button: *button, down: *down }, Bytes::new()),
        Command::Wheel { dx, dy } => (CommandHeader::Wheel { dx: *dx, dy: *dy }, Bytes::new()),
        Command::Key { position, down } => (CommandHeader::Key { position: *position, down: *down }, Bytes::new()),
        Command::Text { text } => (CommandHeader::Text, Bytes::copy_from_slice(text.as_bytes())),
        Command::Clipboard { text } => (CommandHeader::Clipboard, Bytes::copy_from_slice(text.as_bytes())),
        Command::RequestKeyFrame => (CommandHeader::RequestKeyFrame, Bytes::new()),
        Command::Quality { fps, bitrate_kbps } => (CommandHeader::Quality { fps: *fps, bitrate_kbps: *bitrate_kbps }, Bytes::new()),
        Command::SwitchDisplay { index } => (CommandHeader::SwitchDisplay { index: *index }, Bytes::new()),
        Command::Close => (CommandHeader::Close, Bytes::new()),
        Command::Gamepad { .. } => return None,
    })
}

/// The event of a message from a connector; `None` for `hello` (not an
/// event) and for text that is not UTF-8.
pub fn event_from_wire(header: EventHeader, payload: Bytes) -> Option<Event> {
    Some(match header {
        EventHeader::Hello { .. } => return None,
        EventHeader::Video { codec, key, capture_time_ms } => Event::Video { codec, key, capture_time_ms, data: payload },
        EventHeader::Audio { channels } => Event::Audio { channels, data: payload },
        EventHeader::CursorShape { width, height, hot_x, hot_y } => {
            if payload.len() as u64 != width as u64 * height as u64 * 4 {
                return None;
            }
            Event::CursorShape { width, height, hot_x, hot_y, rgba: payload }
        }
        EventHeader::CursorPosition { x, y, visible } => Event::CursorPosition { x, y, visible },
        EventHeader::CursorEmbedded { embedded } => Event::CursorEmbedded { embedded },
        EventHeader::CursorMaybeEmbedded { maybe } => Event::CursorMaybeEmbedded { maybe },
        EventHeader::CursorAlwaysEmbedded => Event::CursorAlwaysEmbedded,
        EventHeader::Platform { os } => Event::Platform { os },
        // A delay is a number that is not negative.
        EventHeader::Latency { ms } => {
            if !ms.is_finite() || ms < 0.0 {
                return None;
            }
            Event::Latency { ms }
        }
        EventHeader::Clipboard => Event::Clipboard { text: String::from_utf8(payload.to_vec()).ok()? },
        EventHeader::ClipboardError => Event::ClipboardError { text: String::from_utf8(payload.to_vec()).ok()? },
        EventHeader::Displays { displays, current } => Event::Displays { displays, current },
        EventHeader::Closed { reason } => Event::Closed { reason },
        // Of a file session (`file_event_from_wire`), not of a picture.
        EventHeader::FilesReady { .. }
        | EventHeader::FileDir { .. }
        | EventHeader::FileStat { .. }
        | EventHeader::FileWalk { .. }
        | EventHeader::FileOk { .. }
        | EventHeader::FileReadStart { .. }
        | EventHeader::FileReadData { .. }
        | EventHeader::FileReadEnd { .. }
        | EventHeader::FileWriteReady { .. }
        | EventHeader::FileWriteSkip { .. }
        | EventHeader::FileWriteDone { .. }
        | EventHeader::FileError { .. } => return None,
        // Of a set-up (`setup_over`).
        EventHeader::SetupStep { .. } | EventHeader::SetupDone { .. } => return None,
    })
}

pub fn file_command_to_wire(command: &FileCommand) -> (CommandHeader, Bytes) {
    let header = match command.clone() {
        FileCommand::List { id, path, hidden } => CommandHeader::FileList { id, path, hidden },
        FileCommand::Stat { id, path } => CommandHeader::FileStat { id, path },
        FileCommand::Walk { id, path } => CommandHeader::FileWalk { id, path },
        FileCommand::Mkdir { id, path } => CommandHeader::FileMkdir { id, path },
        FileCommand::Remove { id, path } => CommandHeader::FileRemove { id, path },
        FileCommand::Rename { id, path, name } => CommandHeader::FileRename { id, path, name },
        FileCommand::Read { id, path, offset } => CommandHeader::FileRead { id, path, offset },
        FileCommand::Write { id, path, size, mtime, on_exists } => CommandHeader::FileWrite { id, path, size, mtime, on_exists },
        FileCommand::WriteData { id, data } => return (CommandHeader::FileWriteData { id }, data),
        FileCommand::WriteEnd { id } => CommandHeader::FileWriteEnd { id },
        FileCommand::Cancel { id } => CommandHeader::FileCancel { id },
        FileCommand::Close => CommandHeader::Close,
    };
    (header, Bytes::new())
}

/// The file event of a message from a connector; `None` for what is not one.
pub fn file_event_from_wire(header: EventHeader, payload: Bytes) -> Option<FileEvent> {
    Some(match header {
        EventHeader::FileDir { id, path, parent, entries } => FileEvent::Dir { id, path, parent, entries },
        EventHeader::FileStat { id, entry } => FileEvent::Stat { id, entry },
        EventHeader::FileWalk { id, path, files, dirs } => FileEvent::Walk { id, path, files, dirs },
        EventHeader::FileOk { id } => FileEvent::Ok { id },
        EventHeader::FileReadStart { id, size, mtime, offset } => FileEvent::ReadStart { id, size, mtime, offset },
        EventHeader::FileReadData { id } => FileEvent::ReadData { id, data: payload },
        EventHeader::FileReadEnd { id, cancelled } => FileEvent::ReadEnd { id, cancelled },
        EventHeader::FileWriteReady { id, offset, path } => FileEvent::WriteReady { id, offset, path },
        EventHeader::FileWriteSkip { id, reason, path } => FileEvent::WriteSkip { id, reason, path },
        EventHeader::FileWriteDone { id, path } => FileEvent::WriteDone { id, path },
        EventHeader::FileError { id, message } => FileEvent::Error { id, message },
        EventHeader::Closed { reason } => FileEvent::Closed { reason },
        _ => return None,
    })
}

/// A file session with a connector that said `hello`, over its input and
/// output: `open_files`, its `files_ready` (or `closed` with the reason:
/// wrong password, not reachable, no file transfer), then the commands and
/// events. `child` is the program, stopped when the session ends.
pub async fn file_session_over<R, W>(
    mut from_connector: R,
    mut to_connector: W,
    target: &Target,
    name: &str,
    child: Option<tokio::process::Child>,
) -> anyhow::Result<FileSession>
where
    R: AsyncRead + Unpin + Send + 'static,
    W: AsyncWrite + Unpin + Send + 'static,
{
    write_message(&mut to_connector, &CommandHeader::OpenFiles { target: target.clone() }, &[])
        .await
        .map_err(|e| anyhow::anyhow!("Connector \"{}\" ended: {}", name, e))?;
    let until = tokio::time::Instant::now() + FILES_TIMEOUT;
    let info = loop {
        let message = tokio::time::timeout_at(until, read_message(&mut from_connector))
            .await
            .map_err(|_| anyhow::anyhow!("Connector \"{}\" did not answer", name))??;
        let Some((header, _)) = message else { anyhow::bail!("Connector \"{}\" ended", name) };
        match serde_json::from_slice::<EventHeader>(&header) {
            Ok(EventHeader::FilesReady { os, sep, home, roots }) => break FilesInfo { os, sep, home, roots },
            Ok(EventHeader::Closed { reason }) => anyhow::bail!("{}", reason),
            _ => {}
        }
    };

    let (event_tx, events) = mpsc::channel::<FileEvent>(files::QUEUE);
    let (commands, mut command_rx) = mpsc::channel::<FileCommand>(files::QUEUE);
    // Commands to the program; the end of the commands ends it.
    tokio::spawn(async move {
        while let Some(command) = command_rx.recv().await {
            let close = command == FileCommand::Close;
            let (header, payload) = file_command_to_wire(&command);
            if write_message(&mut to_connector, &header, &payload).await.is_err() || close {
                break;
            }
        }
        drop(to_connector);
        if let Some(mut child) = child {
            if tokio::time::timeout(std::time::Duration::from_secs(2), child.wait()).await.is_err() {
                let _ = child.kill().await;
            }
        }
    });
    // Events from the program.
    let name = name.to_string();
    tokio::spawn(async move {
        let reason = loop {
            let (header, payload) = match read_message(&mut from_connector).await {
                Ok(Some(message)) => message,
                Ok(None) => break "The connector ended".to_string(),
                Err(e) => break format!("The connector's output is broken: {}", e),
            };
            let header = match serde_json::from_slice::<EventHeader>(&header) {
                Ok(header) => header,
                Err(e) => {
                    warn!("connector {}: a message that is not understood: {}", name, e);
                    continue;
                }
            };
            let Some(event) = file_event_from_wire(header, payload) else { continue };
            let closed = matches!(event, FileEvent::Closed { .. });
            if event_tx.send(event).await.is_err() || closed {
                return;
            }
        };
        let _ = event_tx.send(FileEvent::Closed { reason }).await;
    });
    Ok(FileSession { info, events, commands })
}

/// A set-up with a connector that said `hello`, over its input and output:
/// `setup` with the target, its `setup_step`s (each goes to `steps`), then
/// `setup_done` with the values to keep, or `closed` with the reason. How
/// long it may take is the caller's to decide (it waits for a person).
pub async fn setup_over<R, W>(
    mut from_connector: R,
    mut to_connector: W,
    target: &Target,
    name: &str,
    steps: mpsc::Sender<SetupStep>,
) -> anyhow::Result<std::collections::BTreeMap<String, String>>
where
    R: AsyncRead + Unpin + Send,
    W: AsyncWrite + Unpin + Send,
{
    write_message(&mut to_connector, &CommandHeader::Setup { target: target.clone() }, &[])
        .await
        .map_err(|e| anyhow::anyhow!("Connector \"{}\" ended: {}", name, e))?;
    loop {
        let Some((header, _)) = read_message(&mut from_connector).await? else { anyhow::bail!("Connector \"{}\" ended", name) };
        // What is not understood is skipped (and not written out: a step holds a code).
        match serde_json::from_slice::<EventHeader>(&header) {
            Ok(EventHeader::SetupStep { message, code }) => {
                let _ = steps.send(SetupStep { message, code }).await;
            }
            Ok(EventHeader::SetupDone { values }) => return Ok(values),
            Ok(EventHeader::Closed { reason }) => anyhow::bail!("{}", reason),
            _ => {}
        }
    }
}

pub async fn write_message<W: AsyncWrite + Unpin>(out: &mut W, header: &impl Serialize, payload: &[u8]) -> std::io::Result<()> {
    let header = serde_json::to_vec(header).map_err(std::io::Error::other)?;
    let mut buf = Vec::with_capacity(8 + header.len());
    buf.extend_from_slice(&(header.len() as u32).to_le_bytes());
    buf.extend_from_slice(&(payload.len() as u32).to_le_bytes());
    buf.extend_from_slice(&header);
    out.write_all(&buf).await?;
    out.write_all(payload).await?;
    out.flush().await
}

/// The next message; `None` at the end of the stream.
pub async fn read_message<R: AsyncRead + Unpin>(input: &mut R) -> std::io::Result<Option<(Vec<u8>, Bytes)>> {
    let mut lengths = [0u8; 8];
    match input.read_exact(&mut lengths).await {
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(e) => return Err(e),
    }
    let header_len = u32::from_le_bytes(lengths[..4].try_into().unwrap()) as usize;
    let payload_len = u32::from_le_bytes(lengths[4..].try_into().unwrap()) as usize;
    if header_len > MAX_HEADER || payload_len > MAX_PAYLOAD {
        return Err(std::io::Error::new(std::io::ErrorKind::InvalidData, "message too large"));
    }
    let mut header = vec![0u8; header_len];
    input.read_exact(&mut header).await?;
    let mut payload = vec![0u8; payload_len];
    input.read_exact(&mut payload).await?;
    Ok(Some((header, Bytes::from(payload))))
}

/// An external connector, as its manifest describes it.
pub struct ProcessConnector {
    manifest: Manifest,
    program: PathBuf,
    dir: PathBuf,
}

impl ProcessConnector {
    /// Reads `dir/<id>/connector.json`. Nothing is run.
    pub fn load(dir: &Path, id: &str) -> anyhow::Result<Self> {
        if !super::valid_id(id) {
            anyhow::bail!("\"{}\" is not a connector's name", id);
        }
        let folder = dir.join(id);
        let text = std::fs::read_to_string(folder.join("connector.json"))
            .map_err(|e| anyhow::anyhow!("No connector \"{}\" is installed ({}: {})", id, folder.display(), e))?;
        let manifest: Manifest = serde_json::from_str(&text).map_err(|e| anyhow::anyhow!("connector.json of \"{}\": {}", id, e))?;
        if manifest.id != id {
            anyhow::bail!("connector.json in {} is of connector \"{}\"", folder.display(), manifest.id);
        }
        // The program is a file of the connector's own folder: a manifest
        // cannot point the agent at something else on the computer.
        let folder = folder.canonicalize()?;
        let program = folder.join(&manifest.command).canonicalize().map_err(|e| {
            anyhow::anyhow!("The program of connector \"{}\" ({}) is not there: {}", id, manifest.command, e)
        })?;
        if !program.starts_with(&folder) || !program.is_file() {
            anyhow::bail!("The program of connector \"{}\" must be a file in its own folder", id);
        }
        Ok(ProcessConnector { manifest, program, dir: folder })
    }
}

#[async_trait::async_trait]
impl Connector for ProcessConnector {
    fn describe(&self) -> Description {
        Description {
            id: self.manifest.id.clone(),
            name: self.manifest.name.clone(),
            fields: self.manifest.fields.clone(),
            capabilities: self.manifest.capabilities.clone(),
            note: self.manifest.note.clone(),
        }
    }

    /// A run of the program for the set-up alone: started, greeted, asked
    /// with `setup`, and stopped when it is over or given up on (dropping
    /// this stops the program). A connector whose manifest does not say it
    /// has a set-up is not started: there is nothing to do.
    async fn setup(&self, target: &Target, steps: mpsc::Sender<SetupStep>) -> anyhow::Result<std::collections::BTreeMap<String, String>> {
        if !self.manifest.capabilities.setup {
            return Ok(Default::default());
        }
        let id = self.manifest.id.clone();
        let mut child = tokio::process::Command::new(&self.program)
            .args(&self.manifest.args)
            .current_dir(&self.dir)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .map_err(|e| anyhow::anyhow!("Connector \"{}\" could not be started: {}", id, e))?;
        let mut stdin = child.stdin.take().expect("piped");
        let mut stdout = child.stdout.take().expect("piped");
        if let Some(stderr) = child.stderr.take() {
            let id = id.clone();
            tokio::spawn(async move {
                use tokio::io::AsyncBufReadExt;
                let mut lines = tokio::io::BufReader::new(stderr).lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    debug!("connector {}: {}", id, line);
                }
            });
        }
        write_message(&mut stdin, &CommandHeader::Hello { version: VERSION }, &[]).await?;
        let hello = tokio::time::timeout(HELLO_TIMEOUT, read_message(&mut stdout))
            .await
            .map_err(|_| anyhow::anyhow!("Connector \"{}\" did not answer", id))??;
        match hello.and_then(|(header, _)| serde_json::from_slice::<EventHeader>(&header).ok()) {
            Some(EventHeader::Hello { version }) if version == VERSION => {}
            Some(EventHeader::Hello { version }) => {
                anyhow::bail!("Connector \"{}\" speaks version {} of the protocol, this agent version {}", id, version, VERSION)
            }
            _ => anyhow::bail!("Connector \"{}\" did not say hello", id),
        }
        let result = setup_over(&mut stdout, &mut stdin, target, &id, steps).await;
        // It has a moment to leave by itself, then it is stopped.
        drop(stdin);
        if tokio::time::timeout(std::time::Duration::from_secs(2), child.wait()).await.is_err() {
            let _ = child.kill().await;
        }
        result
    }

    /// A program of its own for the files, as for a picture session: started,
    /// greeted, then asked with `open_files`. A connector whose manifest
    /// does not say it has files is not started for them.
    async fn open_files(&self, target: &Target) -> anyhow::Result<FileSession> {
        if !self.manifest.capabilities.files {
            anyhow::bail!(files::NO_FILES);
        }
        let id = self.manifest.id.clone();
        let mut child = tokio::process::Command::new(&self.program)
            .args(&self.manifest.args)
            .current_dir(&self.dir)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .map_err(|e| anyhow::anyhow!("Connector \"{}\" could not be started: {}", id, e))?;
        let mut stdin = child.stdin.take().expect("piped");
        let mut stdout = child.stdout.take().expect("piped");
        if let Some(stderr) = child.stderr.take() {
            let id = id.clone();
            tokio::spawn(async move {
                use tokio::io::AsyncBufReadExt;
                let mut lines = tokio::io::BufReader::new(stderr).lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    debug!("connector {}: {}", id, line);
                }
            });
        }
        write_message(&mut stdin, &CommandHeader::Hello { version: VERSION }, &[]).await?;
        let hello = tokio::time::timeout(HELLO_TIMEOUT, read_message(&mut stdout))
            .await
            .map_err(|_| anyhow::anyhow!("Connector \"{}\" did not answer", id))??;
        match hello.and_then(|(header, _)| serde_json::from_slice::<EventHeader>(&header).ok()) {
            Some(EventHeader::Hello { version }) if version == VERSION => {}
            Some(EventHeader::Hello { version }) => {
                anyhow::bail!("Connector \"{}\" speaks version {} of the protocol, this agent version {}", id, version, VERSION)
            }
            _ => anyhow::bail!("Connector \"{}\" did not say hello", id),
        }
        file_session_over(stdout, stdin, target, &id, Some(child)).await
    }

    async fn connect(&self, target: &Target, viewer: &Viewer) -> anyhow::Result<Session> {
        let id = self.manifest.id.clone();
        let mut child = tokio::process::Command::new(&self.program)
            .args(&self.manifest.args)
            .current_dir(&self.dir)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .map_err(|e| anyhow::anyhow!("Connector \"{}\" could not be started: {}", id, e))?;
        let mut stdin = child.stdin.take().expect("piped");
        let mut stdout = child.stdout.take().expect("piped");
        if let Some(stderr) = child.stderr.take() {
            let id = id.clone();
            tokio::spawn(async move {
                use tokio::io::AsyncBufReadExt;
                let mut lines = tokio::io::BufReader::new(stderr).lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    debug!("connector {}: {}", id, line);
                }
            });
        }

        // hello, both ways: the versions must be the same.
        write_message(&mut stdin, &CommandHeader::Hello { version: VERSION }, &[]).await?;
        let hello = tokio::time::timeout(HELLO_TIMEOUT, read_message(&mut stdout))
            .await
            .map_err(|_| anyhow::anyhow!("Connector \"{}\" did not answer", id))??;
        match hello.and_then(|(header, _)| serde_json::from_slice::<EventHeader>(&header).ok()) {
            Some(EventHeader::Hello { version }) if version == VERSION => {}
            Some(EventHeader::Hello { version }) => {
                anyhow::bail!("Connector \"{}\" speaks version {} of the protocol, this agent version {}", id, version, VERSION)
            }
            _ => anyhow::bail!("Connector \"{}\" did not say hello", id),
        }
        write_message(&mut stdin, &CommandHeader::Connect { target: target.clone(), viewer: viewer.clone() }, &[]).await?;

        let (event_tx, events) = mpsc::channel::<Event>(64);
        let (commands, mut command_rx) = mpsc::channel::<Command>(64);
        // Commands to the program; the end of the commands ends it.
        tokio::spawn(async move {
            while let Some(command) = command_rx.recv().await {
                let close = command == Command::Close;
                let Some((header, payload)) = command_to_wire(&command) else { continue };
                if write_message(&mut stdin, &header, &payload).await.is_err() || close {
                    break;
                }
            }
            drop(stdin);
            // It has a moment to leave by itself, then it is stopped.
            if tokio::time::timeout(std::time::Duration::from_secs(2), child.wait()).await.is_err() {
                let _ = child.kill().await;
            }
        });
        // Events from the program.
        tokio::spawn(async move {
            let reason = loop {
                let (header, payload) = match read_message(&mut stdout).await {
                    Ok(Some(message)) => message,
                    Ok(None) => break "The connector ended".to_string(),
                    Err(e) => break format!("The connector's output is broken: {}", e),
                };
                let header = match serde_json::from_slice::<EventHeader>(&header) {
                    Ok(header) => header,
                    Err(e) => {
                        warn!("connector {}: a message that is not understood: {}", id, e);
                        continue;
                    }
                };
                let Some(event) = event_from_wire(header, payload) else { continue };
                let closed = matches!(event, Event::Closed { .. });
                if event_tx.send(event).await.is_err() || closed {
                    return;
                }
            };
            let _ = event_tx.send(Event::Closed { reason }).await;
        });
        Ok(Session { events, commands })
    }
}

#[cfg(test)]
mod pointer_wire_tests {
    use super::*;

    fn event(header: &str) -> Option<Event> {
        event_from_wire(serde_json::from_str(header).expect("a known header"), Bytes::new())
    }

    /// The three things a connector can say of the pointer and the picture
    /// are three messages, and stay three events.
    #[test]
    fn what_a_connector_says_of_the_pointer_in_the_picture() {
        assert_eq!(event(r#"{"type":"cursor_always_embedded"}"#), Some(Event::CursorAlwaysEmbedded));
        assert_eq!(event(r#"{"type":"cursor_embedded","embedded":true}"#), Some(Event::CursorEmbedded { embedded: true }));
        assert_eq!(event(r#"{"type":"cursor_maybe_embedded","maybe":true}"#), Some(Event::CursorMaybeEmbedded { maybe: true }));
        // As the connector writes it.
        assert_eq!(serde_json::to_string(&EventHeader::CursorAlwaysEmbedded).unwrap(), r#"{"type":"cursor_always_embedded"}"#);
    }

    /// The delay to the computer and back, as a connector writes it.
    #[test]
    fn the_delay_to_the_computer_is_an_event() {
        // Whole milliseconds or a fraction of one: both are numbers of JSON.
        assert_eq!(event(r#"{"type":"latency","ms":12}"#), Some(Event::Latency { ms: 12.0 }));
        assert_eq!(event(r#"{"type":"latency","ms":0.4}"#), Some(Event::Latency { ms: 0.4 }));
        assert_eq!(serde_json::to_string(&EventHeader::Latency { ms: 12.5 }).unwrap(), r#"{"type":"latency","ms":12.5}"#);
        // A delay is not negative.
        assert_eq!(event(r#"{"type":"latency","ms":-1}"#), None);
        // Not a number of milliseconds: not a known header at all.
        assert!(serde_json::from_str::<EventHeader>(r#"{"type":"latency","ms":"12"}"#).is_err());
        assert!(serde_json::from_str::<EventHeader>(r#"{"type":"latency"}"#).is_err());
    }
}

#[cfg(test)]
mod file_tests {
    use super::*;

    fn target() -> Target {
        serde_json::from_str(r#"{"connector":"thử","host":"192.0.2.9"}"#).unwrap()
    }

    /// A connector's end of the pipes, in this process: what it reads and
    /// writes are the bytes a program would.
    struct Far {
        input: tokio::io::DuplexStream,
        output: tokio::io::DuplexStream,
    }

    impl Far {
        async fn read(&mut self) -> (serde_json::Value, Bytes) {
            let (header, payload) = read_message(&mut self.input).await.unwrap().expect("a message");
            (serde_json::from_slice(&header).unwrap(), payload)
        }

        async fn write(&mut self, header: serde_json::Value, payload: &[u8]) {
            write_message(&mut self.output, &header, payload).await.unwrap();
        }
    }

    fn pipes() -> (Far, tokio::io::DuplexStream, tokio::io::DuplexStream) {
        let (to_connector, input) = tokio::io::duplex(1 << 16);
        let (output, from_connector) = tokio::io::duplex(1 << 16);
        (Far { input, output }, from_connector, to_connector)
    }

    #[tokio::test]
    async fn a_file_session_with_an_external_connector() {
        use serde_json::json;
        let (mut far, from_connector, to_connector) = pipes();
        let connector = tokio::spawn(async move {
            // open_files with the target, as `connect` has it.
            let (header, _) = far.read().await;
            assert_eq!(header, json!({"type": "open_files", "target": {"connector": "thử", "host": "192.0.2.9"}}));
            far.write(json!({"type": "files_ready", "os": "windows", "sep": "\\", "home": "C:\\Users\\An", "roots": [{"name": "C:", "path": "C:\\"}]}), &[]).await;
            let mut seen = Vec::new();
            loop {
                let (header, payload) = far.read().await;
                let id = header["id"].as_str().unwrap_or("").to_string();
                match header["type"].as_str().unwrap() {
                    "file_list" => {
                        assert_eq!(header, json!({"type": "file_list", "id": "1", "path": "C:\\Users\\An", "hidden": true}));
                        far.write(json!({"type": "file_dir", "id": id, "path": "C:\\Users\\An", "parent": "C:\\Users",
                            "entries": [{"name": "Tài liệu", "dir": true}, {"name": "ảnh.png", "dir": false, "size": 7, "mtime": 1700000000, "hidden": true}]}), &[]).await;
                    }
                    "file_stat" => far.write(json!({"type": "file_stat", "id": id, "entry": null}), &[]).await,
                    "file_walk" => far.write(json!({"type": "file_walk", "id": id, "path": header["path"], "files": [{"rel": "a/b.txt", "size": 2}], "dirs": ["a"]}), &[]).await,
                    "file_mkdir" | "file_rename" => far.write(json!({"type": "file_ok", "id": id}), &[]).await,
                    "file_remove" => far.write(json!({"type": "file_error", "id": id, "message": "Không xoá được: tệp đang mở"}), &[]).await,
                    "file_read" => {
                        assert_eq!(header["offset"], 3);
                        far.write(json!({"type": "file_read_start", "id": id, "size": 10, "mtime": 5, "offset": 3}), &[]).await;
                        far.write(json!({"type": "file_read_data", "id": id}), b"defg").await;
                        far.write(json!({"type": "file_read_data", "id": id}), b"hij").await;
                        far.write(json!({"type": "file_read_end", "id": id}), &[]).await;
                    }
                    "file_write" => {
                        assert_eq!(header, json!({"type": "file_write", "id": "8", "path": "C:\\x.bin", "size": 6, "mtime": 9, "on_exists": "replace"}));
                        far.write(json!({"type": "file_write_ready", "id": id, "offset": 0, "path": "C:\\x.bin"}), &[]).await;
                    }
                    "file_write_data" => seen.extend_from_slice(&payload),
                    "file_write_end" => {
                        assert_eq!(seen, b"abcdef");
                        far.write(json!({"type": "file_write_done", "id": id, "path": "C:\\x.bin"}), &[]).await;
                    }
                    "file_cancel" => far.write(json!({"type": "file_read_end", "id": id, "cancelled": true}), &[]).await,
                    "close" => {
                        far.write(json!({"type": "closed", "reason": "Closed"}), &[]).await;
                        return;
                    }
                    other => panic!("{}", other),
                }
            }
        });

        let mut session = file_session_over(from_connector, to_connector, &target(), "thử", None).await.unwrap();
        assert_eq!(
            session.info,
            FilesInfo { os: "windows".into(), sep: "\\".into(), home: Some("C:\\Users\\An".into()), roots: vec![Place { name: "C:".into(), path: "C:\\".into() }] }
        );
        let id = |n: u32| n.to_string();
        let commands = [
            FileCommand::List { id: id(1), path: "C:\\Users\\An".into(), hidden: true },
            FileCommand::Stat { id: id(2), path: "C:\\none".into() },
            FileCommand::Walk { id: id(3), path: "C:\\w".into() },
            FileCommand::Mkdir { id: id(4), path: "C:\\m".into() },
            FileCommand::Rename { id: id(5), path: "C:\\m".into(), name: "n".into() },
            FileCommand::Remove { id: id(6), path: "C:\\n".into() },
            FileCommand::Read { id: id(7), path: "C:\\r".into(), offset: 3 },
            FileCommand::Write { id: id(8), path: "C:\\x.bin".into(), size: 6, mtime: 9, on_exists: OnExists::Replace },
            FileCommand::WriteData { id: id(8), data: Bytes::from_static(b"abc") },
            FileCommand::WriteData { id: id(8), data: Bytes::from_static(b"def") },
            FileCommand::WriteEnd { id: id(8) },
            FileCommand::Cancel { id: id(9) },
            FileCommand::Close,
        ];
        for command in commands {
            session.commands.send(command).await.unwrap();
        }
        let mut events = Vec::new();
        while let Some(event) = session.events.recv().await {
            events.push(event);
        }
        let entry = |name: &str, dir, size, mtime, hidden| FileEntry { name: name.into(), dir, size, mtime, hidden };
        assert_eq!(
            events,
            [
                FileEvent::Dir {
                    id: id(1),
                    path: "C:\\Users\\An".into(),
                    parent: Some("C:\\Users".into()),
                    entries: vec![entry("Tài liệu", true, 0, 0, false), entry("ảnh.png", false, 7, 1_700_000_000, true)],
                },
                FileEvent::Stat { id: id(2), entry: None },
                FileEvent::Walk { id: id(3), path: "C:\\w".into(), files: vec![WalkFile { rel: "a/b.txt".into(), size: 2, mtime: 0 }], dirs: vec!["a".into()] },
                FileEvent::Ok { id: id(4) },
                FileEvent::Ok { id: id(5) },
                FileEvent::Error { id: id(6), message: "Không xoá được: tệp đang mở".into() },
                FileEvent::ReadStart { id: id(7), size: 10, mtime: 5, offset: 3 },
                FileEvent::ReadData { id: id(7), data: Bytes::from_static(b"defg") },
                FileEvent::ReadData { id: id(7), data: Bytes::from_static(b"hij") },
                FileEvent::ReadEnd { id: id(7), cancelled: false },
                FileEvent::WriteReady { id: id(8), offset: 0, path: "C:\\x.bin".into() },
                FileEvent::WriteDone { id: id(8), path: "C:\\x.bin".into() },
                FileEvent::ReadEnd { id: id(9), cancelled: true },
                FileEvent::Closed { reason: "Closed".into() },
            ]
        );
        connector.await.unwrap();
    }

    #[tokio::test]
    async fn a_connector_that_cannot_open_the_files_says_why() {
        use serde_json::json;
        // It answers `closed` with its reason: that is the error.
        let (mut far, from_connector, to_connector) = pipes();
        tokio::spawn(async move {
            far.read().await;
            far.write(json!({"type": "closed", "reason": "Sai mật khẩu"}), &[]).await;
        });
        let error = file_session_over(from_connector, to_connector, &target(), "thử", None).await.err().unwrap();
        assert_eq!(error.to_string(), "Sai mật khẩu");
        // It ends without a word.
        let (far, from_connector, to_connector) = pipes();
        drop(far);
        let error = file_session_over(from_connector, to_connector, &target(), "thử", None).await.err().unwrap();
        assert!(error.to_string().contains("thử"), "{}", error);
    }

    #[tokio::test]
    async fn a_connector_without_files_is_not_started_for_them() {
        let dir = std::env::temp_dir().join(format!("lunaris-connector-nofiles-{}", std::process::id()));
        let folder = dir.join("plain");
        std::fs::create_dir_all(&folder).unwrap();
        // Not a program: starting it would fail with another error.
        std::fs::write(folder.join("program"), b"not a program").unwrap();
        std::fs::write(
            folder.join("connector.json"),
            r#"{"id":"plain","name":"Plain","command":"program","capabilities":{"video":["vp8"]}}"#,
        )
        .unwrap();
        let connector = ProcessConnector::load(&dir, "plain").unwrap();
        assert!(!connector.describe().capabilities.files);
        let error = connector.open_files(&target()).await.err().unwrap();
        assert_eq!(error.to_string(), files::NO_FILES);
        assert_eq!(error.to_string(), "This kind of computer has no file transfer");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn a_set_up_with_an_external_connector() {
        use serde_json::json;
        let (mut far, from_connector, to_connector) = pipes();
        let connector = tokio::spawn(async move {
            let (header, _) = far.read().await;
            assert_eq!(header, json!({"type": "setup", "target": {"connector": "thử", "host": "192.0.2.9"}}));
            far.write(json!({"type": "setup_step", "message": "Nhập mã này ở máy kia", "code": "4711"}), &[]).await;
            // A message of a later protocol is skipped; a step need not have a code.
            far.write(json!({"type": "setup_progress", "percent": 50}), &[]).await;
            far.write(json!({"type": "setup_step", "message": "Almost there"}), &[]).await;
            far.write(json!({"type": "setup_done", "values": {"client_key": "KEY", "unique_id": "0123"}}), &[]).await;
        });
        let (steps_tx, mut steps) = mpsc::channel(8);
        let values = setup_over(from_connector, to_connector, &target(), "thử", steps_tx).await.unwrap();
        assert_eq!(values.get("client_key").map(String::as_str), Some("KEY"));
        assert_eq!(values.len(), 2);
        assert_eq!(steps.recv().await, Some(SetupStep { message: "Nhập mã này ở máy kia".into(), code: Some("4711".into()) }));
        assert_eq!(steps.recv().await, Some(SetupStep { message: "Almost there".into(), code: None }));
        assert_eq!(steps.recv().await, None);
        connector.await.unwrap();

        // `closed` is the reason it failed; `setup_done` without values is nothing to keep.
        let (mut far, from_connector, to_connector) = pipes();
        tokio::spawn(async move {
            far.read().await;
            far.write(json!({"type": "closed", "reason": "Sai mã PIN"}), &[]).await;
        });
        let error = setup_over(from_connector, to_connector, &target(), "thử", mpsc::channel(1).0).await.err().unwrap();
        assert_eq!(error.to_string(), "Sai mã PIN");
        let (mut far, from_connector, to_connector) = pipes();
        tokio::spawn(async move {
            far.read().await;
            far.write(json!({"type": "setup_done"}), &[]).await;
        });
        assert!(setup_over(from_connector, to_connector, &target(), "thử", mpsc::channel(1).0).await.unwrap().is_empty());
        // It ends without a word.
        let (far, from_connector, to_connector) = pipes();
        drop(far);
        let error = setup_over(from_connector, to_connector, &target(), "thử", mpsc::channel(1).0).await.err().unwrap();
        assert!(error.to_string().contains("thử"), "{}", error);
    }

    #[tokio::test]
    async fn a_connector_without_a_set_up_is_not_started_for_one() {
        let dir = std::env::temp_dir().join(format!("lunaris-connector-nosetup-{}", std::process::id()));
        let folder = dir.join("plain");
        std::fs::create_dir_all(&folder).unwrap();
        // Not a program: starting it would fail.
        std::fs::write(folder.join("program"), b"not a program").unwrap();
        // A manifest from before set-ups: no "setup" in its capabilities.
        std::fs::write(folder.join("connector.json"), r#"{"id":"plain","name":"Plain","command":"program","capabilities":{"video":["vp8"]}}"#).unwrap();
        let connector = ProcessConnector::load(&dir, "plain").unwrap();
        assert!(!connector.describe().capabilities.setup);
        assert!(connector.setup(&target(), mpsc::channel(1).0).await.unwrap().is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
