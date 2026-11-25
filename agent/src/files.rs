//! File transfer sessions: a WebRTC connection of their own (like RustDesk's
//! file-transfer connection), so big transfers never queue behind or in front
//! of the stream's video and input, and files can be moved without watching
//! the screen.
//!
//! The client asks with `RequestFileSession`; the agent answers with an offer
//! holding one reliable, ordered data channel, `files`. On it, JSON requests
//! (each with an `id`, answered with the same `id`) and binary chunks of the
//! file being sent:
//!
//! - `hello` -> `hello` {home, downloads, desktop, os, sep, roots: [{name, path}]}
//! - `list` {path, hidden} -> `dir` {path, parent, entries: [{name, dir, size, mtime, hidden}]}
//! - `walk` {path} -> `walk` {path, files: [{rel, size, mtime}], dirs: [rel]} (everything under a folder)
//! - `mkdir` / `remove` {path} / `rename` {path, name} -> `ok`
//! - upload: `put` {path, size, mtime, on_exists: "rename"|"replace"|"skip"}
//!   -> `put_ready` {offset, path} (offset > 0: resuming), or `put_skip`
//!   {reason: "identical"|"exists"}; then binary chunks, then `put_end` ->
//!   `done` {path}. Data goes to `<name>.lunaris-part` (with what it will be
//!   in `.lunaris-part.json`) and becomes the file when complete, so an
//!   interrupted upload resumes where it stopped.
//! - download: `get` {path, offset} -> `get_start` {size, mtime}, binary
//!   chunks, `get_end` (with `cancelled` when stopped by `cancel`).
//! - `cancel` {id} stops an upload or download.
//! - Any failure: `error` {id, message}.
//!
//! Turned off on this computer by AgentConfig allow_file_transfer. What it
//! may reach is AgentConfig file_access ([`Access`]): by default only the
//! home folder; every path from the peer is checked against it.
//!
//! A session for a source (a computer this one shows through a connector,
//! `IncomingFileSession.source_id`) speaks the same protocol on the same
//! kind of channel, served from the connector's file session
//! (`connector/files.rs`) and never from this computer's files
//! ([`start_for_source`]). The two settings above are about this computer's
//! own files and do not apply to it: what may be done with the other
//! computer's files is that computer's to say (its own permission and
//! password, through the connector), and a path of it means nothing here.
//! Its `hello` has `"source": true`, and `home` is "" when the connector
//! cannot tell the folder it starts in.

use std::collections::HashMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::sync::mpsc;
use tracing::{info, warn};
use webrtc::data_channel::{DataChannel, DataChannelEvent, RTCDataChannelInit, RTCDataChannelMessage, RTCDataChannelState};
use webrtc::peer_connection::{
    PeerConnection, PeerConnectionBuilder, PeerConnectionEventHandler, RTCConfigurationBuilder, RTCIceServer,
    RTCPeerConnectionIceEvent, SettingEngineBuilder,
};

use crate::connector::files::{FileCommand, FileEvent, FileSession, OnExists as SourceOnExists};
use crate::connector::Target;

/// Chunks the agent sends. Receivers built on webrtc-rs (the desktop client)
/// read messages into a 65535-byte buffer, so stay below it.
pub const CHUNK: usize = 48 * 1024;
const MAX_BUFFERED: usize = 4 * 1024 * 1024;
const PART: &str = ".lunaris-part";
/// Most entries `walk` returns (a whole disk is not a sensible download).
pub(crate) const MAX_WALK: usize = 200_000;
/// Messages waiting between the data channel and the handler.
const QUEUE: usize = 64;
/// How long a connector has to open a source's files: less than the
/// viewer waits for `hello`, so that it is told why.
const SOURCE_OPEN_TIMEOUT: Duration = Duration::from_secs(12);
/// Requests of a source session taken while the connector has not taken
/// the ones before.
const SOURCE_PENDING: usize = 8;
/// The id of the listing a source session asks for itself (the home
/// folder, to see whether it has a Downloads folder); no request has it.
const SOURCE_HOME_LISTING: &str = "\u{0}home";

/// What the peer may reach (AgentConfig file_access).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Access {
    /// The whole disk ("full").
    Full,
    /// Only this folder and what is in it (canonical path).
    Root(PathBuf),
    /// Nothing: the setting names no usable folder.
    Denied(String),
}

impl Access {
    /// "home" (or empty), "full", or an absolute folder.
    pub fn from_config(value: &str) -> Access {
        let v = value.trim();
        let folder = match v {
            "full" => return Access::Full,
            "" | "home" => match dirs::home_dir() {
                Some(h) => h,
                None => return Access::Denied("file transfer: this account has no home folder".into()),
            },
            _ => PathBuf::from(v),
        };
        if !folder.is_absolute() {
            return Access::Denied(format!("file transfer: the folder in file_access ({}) is not an absolute path", v));
        }
        match canonical(&folder) {
            Ok(root) if root.is_dir() => Access::Root(root),
            _ => Access::Denied(format!("file transfer: the folder {} does not exist", folder.display())),
        }
    }

    /// The folder the peer starts in.
    pub(crate) fn start(&self) -> PathBuf {
        match self {
            Access::Root(root) => root.clone(),
            _ => home(),
        }
    }
}

fn ice_servers(servers: Option<Vec<common::RtcIceServer>>) -> Vec<RTCIceServer> {
    match servers {
        Some(list) => list
            .into_iter()
            .map(|s| RTCIceServer {
                urls: s.urls,
                username: s.username.unwrap_or_default(),
                credential: s.credential.unwrap_or_default(),
            })
            .collect(),
        None => vec![RTCIceServer {
            urls: vec!["stun:stun.l.google.com:19302".to_string()],
            ..Default::default()
        }],
    }
}

/// Sends the connection's local ICE candidates to the client (webrtc 0.21
/// delivers events to a handler object instead of `on_ice_candidate`).
struct IceToSignaling {
    tx: mpsc::UnboundedSender<common::AgentMessage>,
    client: String,
}

#[async_trait::async_trait]
impl PeerConnectionEventHandler for IceToSignaling {
    async fn on_ice_candidate(&self, event: RTCPeerConnectionIceEvent) {
        if let Ok(c) = event.candidate.to_json() {
            let _ = self.tx.send(common::AgentMessage::Signaling(common::SignalingMessage::IceCandidate {
                target_id: self.client.clone(),
                candidate: common::RtcIceCandidate {
                    candidate: c.candidate,
                    sdp_mid: c.sdp_mid,
                    sdp_mline_index: c.sdp_mline_index,
                    username_fragment: c.username_fragment,
                },
            }));
        }
    }
}

/// Creates the session's connection and its `files` channel; returns the
/// connection and the SDP offer to send.
pub async fn start(
    client_id: String,
    agent_tx: mpsc::UnboundedSender<common::AgentMessage>,
    servers: Option<Vec<common::RtcIceServer>>,
    allowed: bool,
    access: Access,
) -> anyhow::Result<(Arc<dyn PeerConnection>, String)> {
    let (pc, channel, rx) = open_channel(&client_id, &agent_tx, servers).await?;
    tokio::spawn(serve(channel, rx, allowed, access, client_id));
    offer(pc).await
}

/// Whose files a session is for: `None` for this computer's own (no source
/// was named), else the source's target, or the words for the person when
/// this computer has no source of that id (never its own files instead).
pub fn source_target(source_id: Option<&str>, sources: impl FnOnce() -> Vec<crate::connector::SourceConfig>) -> Option<Result<Target, String>> {
    let id = source_id.filter(|id| !id.is_empty())?;
    Some(match sources().into_iter().find(|source| source.id == id) {
        Some(source) => {
            info!("File session for source \"{}\" ({}) through connector {}", source.name, source.id, source.target.connector);
            Ok(source.target)
        }
        None => {
            warn!("File session asked for a source this computer does not have");
            Err("This computer no longer has that source".to_string())
        }
    })
}

/// Like [`start`], for the files of a source: the channel is served from
/// the connector's file session with `target`, which is opened now. When
/// there is no such source (`Err` with the words for the person), or the
/// connector cannot open it, every request is answered with that error:
/// this computer's own files are never served in its place.
pub async fn start_for_source(
    client_id: String,
    agent_tx: mpsc::UnboundedSender<common::AgentMessage>,
    servers: Option<Vec<common::RtcIceServer>>,
    target: Result<Target, String>,
) -> anyhow::Result<(Arc<dyn PeerConnection>, String)> {
    let (pc, channel, rx) = open_channel(&client_id, &agent_tx, servers).await?;
    let opening = async move {
        let target = target?;
        let connector = crate::connector::find(&target.connector, &crate::connector::connectors_dir()).map_err(|e| e.to_string())?;
        match tokio::time::timeout(SOURCE_OPEN_TIMEOUT, connector.open_files(&target)).await {
            Ok(session) => session.map_err(|e| e.to_string()),
            Err(_) => Err("The computer did not answer in time".to_string()),
        }
    };
    tokio::spawn(async move {
        let session = opening.await;
        serve_source(channel, rx, session, client_id).await;
    });
    offer(pc).await
}

async fn offer(pc: Arc<dyn PeerConnection>) -> anyhow::Result<(Arc<dyn PeerConnection>, String)> {
    let offer = pc.create_offer(None).await?;
    pc.set_local_description(offer.clone()).await?;
    Ok((pc, offer.sdp))
}

/// The connection of a file session, its `files` channel and what arrives on it.
async fn open_channel(
    client_id: &str,
    agent_tx: &mpsc::UnboundedSender<common::AgentMessage>,
    servers: Option<Vec<common::RtcIceServer>>,
) -> anyhow::Result<(Arc<dyn PeerConnection>, Arc<dyn DataChannel>, mpsc::Receiver<RTCDataChannelMessage>)> {
    let settings = SettingEngineBuilder::new()
        .with_include_loopback_candidate(true)
        .with_network_types(vec![
            rtc::ice::network_type::NetworkType::Udp4,
            rtc::ice::network_type::NetworkType::Tcp4,
        ])
        .build();
    let pc: Arc<dyn PeerConnection> = Arc::new(
        PeerConnectionBuilder::new()
            .with_setting_engine(settings)
            .with_configuration(RTCConfigurationBuilder::default().with_ice_servers(ice_servers(servers)).build())
            .with_handler(Arc::new(IceToSignaling { tx: agent_tx.clone(), client: client_id.to_string() }))
            // The wildcard covers every interface but not loopback (the
            // setting above only allows it): list it, as 0.17 gathered it.
            .with_udp_addrs(vec!["0.0.0.0:0", "127.0.0.1:0"])
            .build()
            .await?,
    );

    let channel = pc
        .create_data_channel("files", Some(RTCDataChannelInit { ordered: true, ..Default::default() }))
        .await?;
    // Bounded: a peer sending faster than the disk takes it waits here
    // instead of filling memory.
    let (tx, rx) = mpsc::channel::<RTCDataChannelMessage>(QUEUE);
    let reader = channel.clone();
    tokio::spawn(async move {
        while let Some(event) = reader.poll().await {
            if let DataChannelEvent::OnMessage(msg) = event {
                if tx.send(msg).await.is_err() {
                    break;
                }
            }
        }
    });
    Ok((pc, channel, rx))
}

// ---------------------------------------------------------------- requests

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum Request {
    Hello { #[serde(default)] id: String },
    List { id: String, path: String, #[serde(default)] hidden: bool },
    Walk { id: String, path: String },
    Mkdir { id: String, path: String },
    Remove { id: String, path: String },
    Rename { id: String, path: String, name: String },
    Put { id: String, path: String, size: u64, #[serde(default)] mtime: u64, #[serde(default)] on_exists: OnExists },
    PutEnd { id: String },
    Get { id: String, path: String, #[serde(default)] offset: u64 },
    Cancel { id: String },
}

#[derive(Deserialize, Default, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum OnExists {
    #[default]
    Rename,
    Replace,
    Skip,
}

#[derive(Serialize, Deserialize, PartialEq, Eq, Debug)]
struct PartInfo {
    size: u64,
    mtime: u64,
}

struct Upload {
    id: String,
    target: PathBuf,
    part: PathBuf,
    file: std::fs::File,
    size: u64,
    mtime: u64,
    received: u64,
    /// The allowed folder, when there is one: the target's folder is
    /// checked again just before the finished file is moved there.
    root: Option<PathBuf>,
}

impl Upload {
    /// One chunk; never more than the announced size.
    fn write(&mut self, data: &[u8]) -> Result<(), String> {
        if self.received.saturating_add(data.len() as u64) > self.size {
            return Err(format!("more data than the announced {} bytes: transfer refused", self.size));
        }
        self.file.write_all(data).map_err(|e| format!("could not write the file: {}", e))?;
        self.received += data.len() as u64;
        Ok(())
    }
}

async fn send_json(ch: &Arc<dyn DataChannel>, v: Value) {
    if let Err(e) = ch.send_text(&v.to_string()).await {
        warn!("Files: could not answer: {}", e);
    }
}

fn err(id: &str, message: impl std::fmt::Display) -> Value {
    json!({"type": "error", "id": id, "message": message.to_string()})
}

async fn serve(channel: Arc<dyn DataChannel>, mut rx: mpsc::Receiver<RTCDataChannelMessage>, allowed: bool, access: Access, client: String) {
    let mut upload: Option<Upload> = None;
    let downloads: Arc<Mutex<HashMap<String, Arc<AtomicBool>>>> = Arc::default();
    info!("File session with {}", client);

    while let Some(msg) = rx.recv().await {
        if !msg.is_string {
            // A chunk of the file being uploaded.
            let Some(up) = upload.as_mut() else { continue };
            if let Err(e) = up.write(&msg.data) {
                let id = up.id.clone();
                warn!("Files: upload {}: {}", id, e);
                upload = None;
                send_json(&channel, err(&id, e)).await;
            }
            continue;
        }
        let req: Request = match serde_json::from_slice(&msg.data) {
            Ok(r) => r,
            Err(e) => {
                warn!("Files: bad request: {}", e);
                continue;
            }
        };
        let refusal = if !allowed {
            Some("File transfer is turned off on this computer".to_string())
        } else if let Access::Denied(reason) = &access {
            Some(reason.clone())
        } else {
            None
        };
        if let Some(refusal) = refusal {
            let id = match &req {
                Request::Hello { id } => id.clone(),
                _ => String::new(),
            };
            send_json(&channel, err(&id, refusal)).await;
            continue;
        }
        let reply = match req {
            Request::Hello { id } => Some(hello(&id, &access)),
            Request::List { id, path, hidden } => Some(list(&id, &access, &path, hidden).unwrap_or_else(|e| err(&id, e))),
            Request::Walk { id, path } => match resolve(&access, &path) {
                Ok(p) => {
                    let access = access.clone();
                    Some(tokio::task::spawn_blocking(move || walk(&id, &access, &p).unwrap_or_else(|e| err(&id, e))).await.unwrap_or(Value::Null))
                }
                Err(e) => Some(err(&id, e)),
            },
            Request::Mkdir { id, path } => Some(match resolve(&access, &path) {
                Ok(p) => simple(&id, std::fs::create_dir_all(p)),
                Err(e) => err(&id, e),
            }),
            Request::Remove { id, path } => Some(match resolve_entry(&access, &path) {
                Ok(p) => {
                    // Not following a link: the link itself goes.
                    let r = match std::fs::symlink_metadata(&p) {
                        Ok(m) if m.is_dir() => std::fs::remove_dir_all(&p),
                        Ok(_) => std::fs::remove_file(&p),
                        Err(e) => Err(e),
                    };
                    simple(&id, r)
                }
                Err(e) => err(&id, e),
            }),
            Request::Rename { id, path, name } => Some(rename(&id, &access, &path, &name)),
            Request::Put { id, path, size, mtime, on_exists } => {
                // A new upload replaces an unfinished one (its part stays for resuming).
                upload = None;
                match resolve(&access, &path).and_then(|p| begin_upload(&id, &access, &p, size, mtime, on_exists)) {
                    Ok((reply, up)) => {
                        upload = up;
                        Some(reply)
                    }
                    Err(e) => Some(err(&id, e)),
                }
            }
            Request::PutEnd { id } => match upload.take() {
                Some(up) if up.id == id => Some(finish_upload(up).unwrap_or_else(|e| err(&id, e))),
                Some(other) => {
                    upload = Some(other);
                    None
                }
                None => Some(err(&id, "no upload in progress")),
            },
            Request::Get { id, path, offset } => {
                let path = match resolve(&access, &path) {
                    Ok(p) => p,
                    Err(e) => {
                        send_json(&channel, err(&id, e)).await;
                        continue;
                    }
                };
                let flag = Arc::new(AtomicBool::new(false));
                downloads.lock().unwrap().insert(id.clone(), flag.clone());
                let ch = channel.clone();
                let downloads = downloads.clone();
                let access = access.clone();
                // Downloads run beside the requests, one chunk after another.
                tokio::spawn(async move {
                    if let Err(e) = download(&ch, &id, &access, &path, offset, &flag).await {
                        send_json(&ch, err(&id, e)).await;
                    }
                    downloads.lock().unwrap().remove(&id);
                });
                None
            }
            Request::Cancel { id } => {
                if upload.as_ref().map(|u| u.id == id).unwrap_or(false) {
                    upload = None;
                }
                if let Some(flag) = downloads.lock().unwrap().get(&id) {
                    flag.store(true, Ordering::SeqCst);
                }
                None
            }
        };
        if let Some(reply) = reply {
            send_json(&channel, reply).await;
        }
    }
    info!("File session with {} closed", client);
}

// ---------------------------------------------------------------- sources

/// A request of the channel's protocol as a command for a connector's file
/// session; `None` for `hello` (answered here).
fn source_command(req: Request) -> Option<FileCommand> {
    Some(match req {
        Request::Hello { .. } => return None,
        Request::List { id, path, hidden } => FileCommand::List { id, path, hidden },
        Request::Walk { id, path } => FileCommand::Walk { id, path },
        Request::Mkdir { id, path } => FileCommand::Mkdir { id, path },
        Request::Remove { id, path } => FileCommand::Remove { id, path },
        Request::Rename { id, path, name } => FileCommand::Rename { id, path, name },
        Request::Put { id, path, size, mtime, on_exists } => FileCommand::Write {
            id,
            path,
            size,
            mtime,
            on_exists: match on_exists {
                OnExists::Rename => SourceOnExists::Rename,
                OnExists::Replace => SourceOnExists::Replace,
                OnExists::Skip => SourceOnExists::Skip,
            },
        },
        Request::PutEnd { id } => FileCommand::WriteEnd { id },
        Request::Get { id, path, offset } => FileCommand::Read { id, path, offset },
        Request::Cancel { id } => FileCommand::Cancel { id },
    })
}

/// A connector's event as the channel's message; `None` for a file's data
/// (sent as binary chunks) and for what the protocol has no message for.
fn source_reply(event: &FileEvent) -> Option<Value> {
    Some(match event {
        FileEvent::Dir { id, path, parent, entries } => json!({"type": "dir", "id": id, "path": path, "parent": parent, "entries": entries}),
        FileEvent::Walk { id, path, files, dirs } => json!({"type": "walk", "id": id, "path": path, "files": files, "dirs": dirs}),
        FileEvent::Ok { id } => json!({"type": "ok", "id": id}),
        FileEvent::ReadStart { id, size, mtime, offset } => json!({"type": "get_start", "id": id, "size": size, "mtime": mtime, "offset": offset}),
        FileEvent::ReadEnd { id, cancelled: true } => json!({"type": "get_end", "id": id, "cancelled": true}),
        FileEvent::ReadEnd { id, cancelled: false } => json!({"type": "get_end", "id": id}),
        FileEvent::WriteReady { id, offset, path } => json!({"type": "put_ready", "id": id, "offset": offset, "path": path}),
        FileEvent::WriteSkip { id, reason, path } => json!({"type": "put_skip", "id": id, "reason": reason, "path": path}),
        FileEvent::WriteDone { id, path } => json!({"type": "done", "id": id, "path": path}),
        FileEvent::Error { id, message } => err(id, message),
        FileEvent::Stat { .. } | FileEvent::ReadData { .. } | FileEvent::Closed { .. } => return None,
    })
}

/// `hello` of a source: what the connector knows of the computer. The
/// folder files dropped on the stream go to is its Downloads folder when
/// the home folder has one by that name, else the home folder.
fn source_hello(id: &str, info: &crate::connector::files::FilesInfo, downloads: Option<&str>) -> Value {
    let home = info.home.clone().unwrap_or_default();
    json!({
        "type": "hello", "id": id,
        "home": home,
        "downloads": downloads.unwrap_or(&home),
        "os": info.os,
        "sep": info.sep,
        "roots": info.roots,
        "source": true,
    })
}

/// Serves the channel's protocol from a connector's file session (or, when
/// there is none, answers every request with why).
async fn serve_source(
    channel: Arc<dyn DataChannel>,
    mut rx: mpsc::Receiver<RTCDataChannelMessage>,
    session: Result<FileSession, String>,
    client: String,
) {
    info!("File session with {} for a source", client);
    let mut session = match session {
        Ok(session) => session,
        Err(reason) => {
            warn!("Files: the source's files could not be opened: {}", reason);
            // The viewer is told with its `hello`, and with whatever else it asks.
            while let Some(msg) = rx.recv().await {
                if !msg.is_string {
                    continue;
                }
                let id = serde_json::from_slice::<Value>(&msg.data).ok().and_then(|v| v.get("id").and_then(Value::as_str).map(str::to_string));
                send_json(&channel, err(id.as_deref().unwrap_or(""), &reason)).await;
            }
            info!("File session with {} closed", client);
            return;
        }
    };
    let info = session.info.clone();
    let sep = info.sep.clone();
    // Commands the connector has not taken yet (it is never waited for
    // while its events wait: neither side can hold the other up).
    let mut pending: std::collections::VecDeque<FileCommand> = Default::default();
    // The file data on its way to the viewer, and whose it is.
    let mut data = bytes::Bytes::new();
    let mut download: Option<String> = None;
    let mut upload: Option<String> = None;
    // `hello` waiting for the home folder's listing, and what it showed.
    let mut hello_waiting: Option<String> = None;
    let mut downloads: Option<String> = None;
    let mut closed: Option<String> = None;

    loop {
        // File data first, as much as the channel takes.
        let mut blocked = false;
        while !data.is_empty() {
            if channel.outstanding_bytes().await.unwrap_or(0) > MAX_BUFFERED {
                if !matches!(channel.ready_state().await, Ok(RTCDataChannelState::Open)) {
                    data.clear();
                }
                blocked = !data.is_empty();
                break;
            }
            let chunk = data.split_to(data.len().min(CHUNK));
            if channel.send(bytes::BytesMut::from(&chunk[..])).await.is_err() {
                data.clear();
            }
        }
        tokio::select! {
            permit = session.commands.reserve(), if !pending.is_empty() && closed.is_none() => {
                match permit {
                    Ok(permit) => permit.send(pending.pop_front().expect("not empty")),
                    Err(_) => closed = Some("The connection to the computer ended".into()),
                }
            }
            msg = rx.recv(), if pending.len() < SOURCE_PENDING => {
                let Some(msg) = msg else { break };
                if !msg.is_string {
                    // A chunk of the file being uploaded.
                    if let Some(id) = upload.clone() {
                        pending.push_back(FileCommand::WriteData { id, data: msg.data.freeze() });
                    }
                    continue;
                }
                let req: Request = match serde_json::from_slice(&msg.data) {
                    Ok(r) => r,
                    Err(e) => {
                        warn!("Files: bad request: {}", e);
                        continue;
                    }
                };
                if let Some(reason) = &closed {
                    let id = serde_json::from_slice::<Value>(&msg.data).ok().and_then(|v| v.get("id").and_then(Value::as_str).map(str::to_string));
                    send_json(&channel, err(id.as_deref().unwrap_or(""), reason)).await;
                    continue;
                }
                match &req {
                    Request::Hello { id } => {
                        match (&info.home, &downloads) {
                            (Some(home), None) => {
                                hello_waiting = Some(id.clone());
                                pending.push_back(FileCommand::List { id: SOURCE_HOME_LISTING.into(), path: home.clone(), hidden: false });
                            }
                            _ => send_json(&channel, source_hello(id, &info, downloads.as_deref())).await,
                        }
                        continue;
                    }
                    Request::Put { id, .. } => upload = Some(id.clone()),
                    // One download at a time: a second one is refused by the
                    // connector, and the first goes on.
                    Request::Get { id, .. } => {
                        if download.is_none() {
                            download = Some(id.clone());
                        }
                    }
                    Request::Cancel { id } => {
                        if upload.as_deref() == Some(id.as_str()) {
                            upload = None;
                        }
                        // Nothing more of a cancelled download is sent.
                        if download.as_deref() == Some(id.as_str()) {
                            download = None;
                            data.clear();
                        }
                    }
                    _ => {}
                }
                if let Some(command) = source_command(req) {
                    pending.push_back(command);
                }
            }
            event = session.events.recv(), if data.is_empty() && closed.is_none() => {
                let event = event.unwrap_or(FileEvent::Closed { reason: "The connection to the computer ended".into() });
                match event {
                    // The listing asked for here: is there a Downloads folder?
                    FileEvent::Dir { ref id, ref path, ref entries, .. } if id == SOURCE_HOME_LISTING => {
                        let found = entries.iter().find(|e| e.dir && e.name.eq_ignore_ascii_case("Downloads"));
                        let dir = match found {
                            Some(e) if path.ends_with(['/', '\\']) => format!("{}{}", path, e.name),
                            Some(e) => format!("{}{}{}", path, sep, e.name),
                            None => path.clone(),
                        };
                        downloads = Some(dir);
                        if let Some(id) = hello_waiting.take() {
                            send_json(&channel, source_hello(&id, &info, downloads.as_deref())).await;
                        }
                    }
                    FileEvent::Error { ref id, .. } if id == SOURCE_HOME_LISTING => {
                        downloads = info.home.clone();
                        if let Some(id) = hello_waiting.take() {
                            send_json(&channel, source_hello(&id, &info, downloads.as_deref())).await;
                        }
                    }
                    FileEvent::ReadData { id, data: bytes } => {
                        if download.as_deref() == Some(id.as_str()) {
                            data = bytes;
                        }
                    }
                    FileEvent::Closed { reason } => {
                        // What was under way ends with the reason; so does
                        // whatever is asked from now on.
                        for id in hello_waiting.take().into_iter().chain(download.take()).chain(upload.take()) {
                            send_json(&channel, err(&id, &reason)).await;
                        }
                        pending.clear();
                        closed = Some(reason);
                    }
                    event => {
                        match &event {
                            FileEvent::ReadEnd { id, .. } if download.as_deref() == Some(id.as_str()) => download = None,
                            FileEvent::WriteSkip { id, .. } | FileEvent::WriteDone { id, .. } if upload.as_deref() == Some(id.as_str()) => upload = None,
                            FileEvent::Error { id, .. } => {
                                if upload.as_deref() == Some(id.as_str()) {
                                    upload = None;
                                }
                                if download.as_deref() == Some(id.as_str()) {
                                    download = None;
                                }
                            }
                            _ => {}
                        }
                        if let Some(reply) = source_reply(&event) {
                            send_json(&channel, reply).await;
                        }
                    }
                }
            }
            _ = tokio::time::sleep(Duration::from_millis(5)), if blocked => {}
        }
    }
    let _ = session.commands.try_send(FileCommand::Close);
    info!("File session with {} closed", client);
}

fn simple(id: &str, r: std::io::Result<()>) -> Value {
    match r {
        Ok(()) => json!({"type": "ok", "id": id}),
        Err(e) => err(id, e),
    }
}

fn home() -> PathBuf {
    dirs::home_dir().unwrap_or_else(|| PathBuf::from("/"))
}

/// The path as the peer means it: "" or "~" is the starting folder (home,
/// or the allowed folder), "~/x" is in it.
fn expand(access: &Access, path: &str) -> PathBuf {
    let p = path.trim();
    if p.is_empty() || p == "~" {
        return access.start();
    }
    if let Some(rest) = p.strip_prefix("~/").or_else(|| p.strip_prefix("~\\")) {
        return access.start().join(rest);
    }
    PathBuf::from(p)
}

/// `fs::canonicalize`, without the `\\?\` prefix Windows adds to a drive
/// path (the peer shows these paths).
pub(crate) fn canonical(path: impl AsRef<Path>) -> std::io::Result<PathBuf> {
    let real = std::fs::canonicalize(path)?;
    #[cfg(windows)]
    {
        let s = real.to_string_lossy();
        let b = s.as_bytes();
        if s.starts_with(r"\\?\") && b.len() >= 7 && b[4].is_ascii_alphabetic() && b[5] == b':' && b[6] == b'\\' {
            return Ok(PathBuf::from(&s[4..]));
        }
    }
    Ok(real)
}

fn outside(root: &Path) -> String {
    format!(
        "Not allowed: outside {}, the only folder file transfer may use on this computer",
        root.display()
    )
}

/// The real location of `path`, which need not exist yet: the deepest part
/// that exists is canonicalized (links and `..` resolved by the system), the
/// rest must be plain names that do not exist (so no `..` and no link,
/// broken or not, can lead elsewhere later).
fn real_path(path: &Path) -> Result<PathBuf, String> {
    let mut existing = path.to_path_buf();
    let mut rest: Vec<std::ffi::OsString> = Vec::new();
    loop {
        match canonical(&existing) {
            Ok(real) => {
                let mut out = real;
                for (i, name) in rest.iter().rev().enumerate() {
                    out.push(name);
                    // A broken link (or anything unreadable) in the new part.
                    if i == 0 && std::fs::symlink_metadata(&out).is_ok() {
                        return Err(format!("{}: a broken link or an unreadable entry", out.display()));
                    }
                }
                return Ok(out);
            }
            Err(_) => {
                let name = match existing.components().next_back() {
                    Some(std::path::Component::Normal(n)) => n.to_os_string(),
                    Some(std::path::Component::CurDir) => std::ffi::OsString::new(),
                    _ => return Err(format!("{}: no such folder", path.display())),
                };
                if !existing.pop() {
                    return Err(format!("{}: no such folder", path.display()));
                }
                if !name.is_empty() {
                    rest.push(name);
                }
            }
        }
    }
}

/// Names Windows opens as devices, with or without an extension or what
/// follows a colon (`NUL.txt`, `COM1:9600`; Windows reads the name up to
/// the first of either), the ports with a superscript digit too (`COM¹`).
fn is_device_name(name: &str) -> bool {
    let stem = name.split(['.', ':']).next().unwrap_or("").trim_end_matches(' ').to_ascii_uppercase();
    if matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL" | "CONIN$" | "CONOUT$") {
        return true;
    }
    let mut rest = stem.strip_prefix("COM").or_else(|| stem.strip_prefix("LPT")).unwrap_or("").chars();
    matches!((rest.next(), rest.next()), (Some('1'..='9' | '¹' | '²' | '³'), None))
}

/// Checks on the text of a peer path, before the file system sees it (on
/// every system, as both separators): no device namespace (`\\.\`,
/// `\\?\`, `\??\`), no network path (`\\server\share`) unless the whole
/// disk is allowed, and no name Windows would read differently from Rust:
/// one ending in a dot or a space ("..." is the folder itself there) or a
/// device name (CON, NUL, COM1, ...). "." and ".." are kept as they are.
fn check_path_text(path: &str, network: bool) -> Result<(), String> {
    let p = path.trim().replace('/', "\\");
    for prefix in [r"\\.", r"\\?", r"\??"] {
        if p == prefix || p.starts_with(&format!("{}\\", prefix)) {
            return Err(format!("{}: device paths are not allowed", path.trim()));
        }
    }
    if p.starts_with(r"\\") && !network {
        return Err(format!("{}: network paths are not allowed", path.trim()));
    }
    for name in p.split('\\') {
        if name.is_empty() || name == "." || name == ".." {
            continue;
        }
        if name.trim_end_matches(['.', ' ']).len() != name.len() {
            return Err(format!("{:?}: a name may not end in a dot or a space", name));
        }
        if is_device_name(name) {
            return Err(format!("{:?}: a reserved device name", name));
        }
    }
    Ok(())
}

/// `path` with "." and ".." worked out on the text alone, if it stays
/// inside `root` (relative paths are taken relative to it).
fn lexically_inside(root: &Path, path: &Path) -> Option<PathBuf> {
    let joined = if path.is_absolute() { path.to_path_buf() } else { root.join(path) };
    let mut out = PathBuf::new();
    for c in joined.components() {
        match c {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                if !out.pop() {
                    return None;
                }
            }
            other => out.push(other.as_os_str()),
        }
    }
    out.starts_with(root).then_some(out)
}

/// A path from the peer, checked against `access` (links followed).
pub(crate) fn resolve(access: &Access, path: &str) -> Result<PathBuf, String> {
    match access {
        Access::Full => {
            check_path_text(path, true)?;
            Ok(expand(access, path))
        }
        Access::Denied(reason) => Err(reason.clone()),
        Access::Root(root) => {
            check_path_text(path, false)?;
            // On the text first: nothing outside the folder is opened, not
            // even to find out where it leads.
            let p = lexically_inside(root, &expand(access, path)).ok_or_else(|| outside(root))?;
            let real = real_path(&p)?;
            if real.starts_with(root) {
                Ok(real)
            } else {
                Err(outside(root))
            }
        }
    }
}

/// Like [`resolve`] for an entry that is removed or renamed itself: its
/// folder is resolved, the last name is kept (a link is acted on, not
/// followed). The allowed folder itself is not such an entry.
pub(crate) fn resolve_entry(access: &Access, path: &str) -> Result<PathBuf, String> {
    let Access::Root(root) = access else {
        return resolve(access, path);
    };
    check_path_text(path, false)?;
    let p = lexically_inside(root, &expand(access, path)).ok_or_else(|| outside(root))?;
    let (Some(parent), Some(std::path::Component::Normal(name))) = (p.parent(), p.components().next_back()) else {
        return Err(outside(root));
    };
    let dir = resolve(access, &parent.to_string_lossy())?;
    let entry = dir.join(name);
    let not_here = || format!("{} cannot be changed from here", root.display());
    if entry == *root {
        return Err(not_here());
    }
    // Whatever the name says, the entry must not be the folder or above it.
    if let Ok(real) = canonical(&entry) {
        if root.starts_with(&real) {
            return Err(not_here());
        }
    }
    Ok(entry)
}

/// Opens `path` (already checked against `access` by [`resolve`]) for what
/// `options` say, and makes sure, once it is open, that what was opened is
/// inside the allowed folder: the folders on the way may have been
/// replaced by links between the check and the open. On Unix the last name
/// is never followed if it is a link (`O_NOFOLLOW`), and the open file's
/// real location is read back (Linux: `/proc/self/fd`; elsewhere the path
/// is resolved again and must be the same file). On Windows the path is
/// resolved again after the open. Outside: refused, the file is closed.
pub(crate) fn open_inside(access: &Access, path: &Path, options: &mut std::fs::OpenOptions) -> Result<std::fs::File, String> {
    let Access::Root(root) = access else {
        return options.open(path).map_err(|e| format!("{}: {}", path.display(), e));
    };
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW);
    }
    let file = options.open(path).map_err(|e| {
        #[cfg(unix)]
        if e.raw_os_error() == Some(libc::ELOOP) {
            return format!("{} is a link: not followed", path.display());
        }
        format!("{}: {}", path.display(), e)
    })?;
    match opened_location(&file, path) {
        Some(real) if real.starts_with(root) => Ok(file),
        _ => {
            warn!("Files: {} led outside {} when it was opened: refused", path.display(), root.display());
            Err(outside(root))
        }
    }
}

/// Where the open `file` really is (`path` is the name it was opened by).
fn opened_location(file: &std::fs::File, path: &Path) -> Option<PathBuf> {
    #[cfg(target_os = "linux")]
    {
        use std::os::unix::io::AsRawFd;
        if let Ok(real) = std::fs::read_link(format!("/proc/self/fd/{}", file.as_raw_fd())) {
            return Some(real);
        }
    }
    let real = canonical(path).ok()?;
    #[cfg(unix)]
    {
        // The same file, not one put there since.
        use std::os::unix::fs::MetadataExt;
        let (opened, now) = (file.metadata().ok()?, std::fs::metadata(&real).ok()?);
        if (opened.dev(), opened.ino()) != (now.dev(), now.ino()) {
            return None;
        }
    }
    #[cfg(not(unix))]
    let _ = file;
    Some(real)
}

/// Whether `path` (a link perhaps) stays inside the allowed folder.
pub(crate) fn inside(access: &Access, path: &Path) -> bool {
    match access {
        Access::Root(root) => canonical(path).map(|r| r.starts_with(root)).unwrap_or(false),
        _ => true,
    }
}

fn rename(id: &str, access: &Access, path: &str, name: &str) -> Value {
    let p = match resolve_entry(access, path) {
        Ok(p) => p,
        Err(e) => return err(id, e),
    };
    let name = safe_name(name);
    let Some(dir) = p.parent() else { return err(id, "cannot rename this") };
    // Both ends are checked: the new name lands in the same (allowed) folder.
    let dest = match resolve(access, &dir.join(&name).to_string_lossy()) {
        Ok(d) => d,
        Err(e) => return err(id, e),
    };
    if std::fs::symlink_metadata(&dest).is_ok() {
        return err(id, format!("{} already exists", name));
    }
    simple(id, std::fs::rename(&p, dest))
}

pub(crate) fn safe_name(name: &str) -> String {
    let base = name.rsplit(['/', '\\']).next().unwrap_or("").trim();
    let cleaned: String = base
        .chars()
        .map(|c| if c.is_control() || matches!(c, ':' | '*' | '?' | '"' | '<' | '>' | '|') { '_' } else { c })
        .collect();
    // Leading dots go (no hidden files, no ".."); trailing dots and spaces
    // too, as Windows drops them.
    let cleaned = cleaned.trim_start_matches(['.', ' ']).trim_end_matches(['.', ' ']).trim().to_string();
    if cleaned.is_empty() {
        "file".into()
    } else if is_device_name(&cleaned) {
        format!("_{}", cleaned)
    } else {
        cleaned
    }
}

pub(crate) fn mtime_of(m: &std::fs::Metadata) -> u64 {
    m.modified().ok().and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map(|d| d.as_secs()).unwrap_or(0)
}

fn is_hidden(name: &str, _meta: Option<&std::fs::Metadata>) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        if let Some(m) = _meta {
            if m.file_attributes() & 0x2 != 0 {
                return true;
            }
        }
    }
    name.starts_with('.')
}

fn path_str(p: &Path) -> String {
    p.to_string_lossy().into_owned()
}

fn hello(id: &str, access: &Access) -> Value {
    if let Access::Root(root) = access {
        // Only the allowed folder is offered.
        let name = if Some(root) == dirs::home_dir().and_then(|h| canonical(h).ok()).as_ref() {
            "Home".to_string()
        } else {
            root.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| path_str(root))
        };
        let downloads = dirs::download_dir()
            .and_then(|d| canonical(d).ok())
            .filter(|d| d.starts_with(root))
            .unwrap_or_else(|| root.clone());
        return json!({
            "type": "hello", "id": id,
            "home": path_str(root),
            "downloads": path_str(&downloads),
            "os": std::env::consts::OS,
            "sep": std::path::MAIN_SEPARATOR.to_string(),
            "roots": [{"name": name, "path": path_str(root)}],
        });
    }
    let mut roots = vec![json!({"name": "Home", "path": path_str(&home())})];
    if let Some(d) = dirs::desktop_dir() {
        roots.push(json!({"name": "Desktop", "path": path_str(&d)}));
    }
    if let Some(d) = dirs::document_dir() {
        roots.push(json!({"name": "Documents", "path": path_str(&d)}));
    }
    if let Some(d) = dirs::download_dir() {
        roots.push(json!({"name": "Downloads", "path": path_str(&d)}));
    }
    #[cfg(windows)]
    for letter in b'A'..=b'Z' {
        let drive = format!("{}:\\", letter as char);
        if Path::new(&drive).exists() {
            roots.push(json!({"name": format!("{}:", letter as char), "path": drive}));
        }
    }
    #[cfg(not(windows))]
    roots.push(json!({"name": "/", "path": "/"}));
    json!({
        "type": "hello", "id": id,
        "home": path_str(&home()),
        "downloads": dirs::download_dir().map(|d| path_str(&d)).unwrap_or_else(|| path_str(&home())),
        "os": std::env::consts::OS,
        "sep": std::path::MAIN_SEPARATOR.to_string(),
        "roots": roots,
    })
}

fn list(id: &str, access: &Access, path: &str, hidden: bool) -> Result<Value, String> {
    let dir = resolve(access, path)?;
    let read = std::fs::read_dir(&dir).map_err(|e| format!("{}: {}", dir.display(), e))?;
    let mut entries = Vec::new();
    for entry in read.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.ends_with(PART) || name.ends_with(".lunaris-part.json") {
            continue;
        }
        // Follow links so a link to a folder opens like a folder.
        if entry.file_type().map(|t| t.is_symlink()).unwrap_or(false) && !inside(access, &entry.path()) {
            continue;
        }
        let meta = std::fs::metadata(entry.path()).or_else(|_| entry.metadata()).ok();
        let hide = is_hidden(&name, meta.as_ref());
        if hide && !hidden {
            continue;
        }
        let is_dir = meta.as_ref().map(|m| m.is_dir()).unwrap_or(false);
        entries.push(json!({
            "name": name,
            "dir": is_dir,
            "size": meta.as_ref().filter(|m| !m.is_dir()).map(|m| m.len()).unwrap_or(0),
            "mtime": meta.as_ref().map(mtime_of).unwrap_or(0),
            "hidden": hide,
        }));
    }
    Ok(json!({
        "type": "dir", "id": id,
        "path": path_str(&dir),
        // Not above the allowed folder.
        "parent": match access {
            Access::Root(root) if dir == *root => None,
            _ => dir.parent().map(path_str),
        },
        "entries": entries,
    }))
}

fn walk(id: &str, access: &Access, root: &Path) -> Result<Value, String> {
    let meta = std::fs::metadata(root).map_err(|e| format!("{}: {}", root.display(), e))?;
    if !meta.is_dir() {
        return Err(format!("{} is not a folder", root.display()));
    }
    let mut files = Vec::new();
    let mut dirs = Vec::new();
    let mut stack = vec![PathBuf::new()];
    while let Some(rel) = stack.pop() {
        let Ok(read) = std::fs::read_dir(root.join(&rel)) else { continue };
        for entry in read.flatten() {
            if files.len() + dirs.len() > MAX_WALK {
                return Err(format!("more than {} files in {}", MAX_WALK, root.display()));
            }
            let name = entry.file_name();
            let r = rel.join(&name);
            let rel_str = r.to_string_lossy().replace('\\', "/");
            // Don't follow links into folders (loops); linked files are copied.
            let Ok(ft) = entry.file_type() else { continue };
            if ft.is_symlink() && !inside(access, &entry.path()) {
                continue;
            }
            if ft.is_dir() {
                dirs.push(json!(rel_str));
                stack.push(r);
            } else if let Ok(m) = std::fs::metadata(entry.path()) {
                if m.is_file() && !name.to_string_lossy().ends_with(PART) {
                    files.push(json!({"rel": rel_str, "size": m.len(), "mtime": mtime_of(&m)}));
                }
            }
        }
    }
    Ok(json!({"type": "walk", "id": id, "path": path_str(root), "files": files, "dirs": dirs}))
}

/// `name (2).ext`, `name (3).ext`... next to `path`.
fn free_name(path: &Path) -> PathBuf {
    let dir = path.parent().unwrap_or(Path::new("."));
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "file".into());
    let (stem, ext) = match name.rfind('.') {
        Some(i) if i > 0 => (name[..i].to_string(), name[i..].to_string()),
        _ => (name.clone(), String::new()),
    };
    (2..10_000)
        .map(|n| dir.join(format!("{} ({}){}", stem, n, ext)))
        .find(|p| !p.exists())
        .unwrap_or_else(|| path.to_path_buf())
}

fn part_paths(target: &Path) -> (PathBuf, PathBuf) {
    let mut part = target.as_os_str().to_owned();
    part.push(PART);
    let part = PathBuf::from(part);
    let mut info = part.as_os_str().to_owned();
    info.push(".json");
    (part, PathBuf::from(info))
}

/// Same size and modification time (within 2 s: FAT and some copies round it).
fn identical(meta: &std::fs::Metadata, size: u64, mtime: u64) -> bool {
    meta.len() == size && mtime != 0 && mtime_of(meta).abs_diff(mtime) <= 2
}

/// `path` is already checked against `access`; the names made from it
/// (renamed copy, part files) are checked too.
fn begin_upload(id: &str, access: &Access, path: &Path, size: u64, mtime: u64, on_exists: OnExists) -> Result<(Value, Option<Upload>), String> {
    let dir = path.parent().ok_or("no folder")?;
    let name = safe_name(&path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default());
    std::fs::create_dir_all(dir).map_err(|e| format!("cannot create {}: {}", dir.display(), e))?;
    let mut target = resolve(access, &dir.join(&name).to_string_lossy())?;

    if let Ok(meta) = std::fs::metadata(&target) {
        if meta.is_dir() {
            return Err(format!("{} is a folder", target.display()));
        }
        if identical(&meta, size, mtime) {
            return Ok((json!({"type": "put_skip", "id": id, "reason": "identical", "path": path_str(&target)}), None));
        }
        match on_exists {
            OnExists::Skip => return Ok((json!({"type": "put_skip", "id": id, "reason": "exists", "path": path_str(&target)}), None)),
            OnExists::Rename => target = free_name(&target),
            OnExists::Replace => {}
        }
    }

    // Resume when an unfinished upload of the same file is there.
    let (part, info_path) = part_paths(&target);
    if let Access::Root(_) = access {
        for p in [&target, &part, &info_path] {
            if std::fs::symlink_metadata(p).map(|m| m.file_type().is_symlink()).unwrap_or(false) {
                return Err(format!("{} is a link: not written through", p.display()));
            }
        }
    }
    let resume = std::fs::read(&info_path)
        .ok()
        .and_then(|b| serde_json::from_slice::<PartInfo>(&b).ok())
        .filter(|i| *i == PartInfo { size, mtime } && mtime != 0)
        .and_then(|_| std::fs::metadata(&part).ok())
        .map(|m| m.len())
        .filter(|&len| len <= size)
        .unwrap_or(0);
    let file = if resume > 0 {
        open_inside(access, &part, std::fs::OpenOptions::new().append(true))
    } else {
        open_inside(access, &part, std::fs::OpenOptions::new().write(true).create(true).truncate(true))
    }
    .map_err(|e| format!("cannot write {}", e))?;
    if let Ok(mut info) = open_inside(access, &info_path, std::fs::OpenOptions::new().write(true).create(true).truncate(true)) {
        let _ = info.write_all(&serde_json::to_vec(&PartInfo { size, mtime }).unwrap_or_default());
    }
    if resume > 0 {
        info!("Resuming {} at {} of {} bytes", target.display(), resume, size);
    } else {
        info!("Receiving {} ({} bytes)", target.display(), size);
    }
    let root = match access {
        Access::Root(root) => Some(root.clone()),
        _ => None,
    };
    let up = Upload { id: id.to_string(), target: target.clone(), part, file, size, mtime, received: resume, root };
    Ok((json!({"type": "put_ready", "id": id, "offset": resume, "path": path_str(&target)}), Some(up)))
}

fn finish_upload(up: Upload) -> Result<Value, String> {
    let Upload { id, target, part, file, size, mtime, received, root } = up;
    let _ = file.sync_all();
    if received != size {
        return Err(format!("incomplete: {} of {} bytes (it can be resumed)", received, size));
    }
    if mtime > 0 {
        let _ = file.set_modified(UNIX_EPOCH + Duration::from_secs(mtime));
    }
    drop(file);
    if let Some(root) = &root {
        // Its folder may have been replaced by a link since the upload began.
        let dir = target.parent().and_then(|d| canonical(d).ok());
        if !dir.is_some_and(|d| d.starts_with(root)) || std::fs::symlink_metadata(&part).map(|m| m.file_type().is_symlink()).unwrap_or(true) {
            warn!("Files: the folder of {} is no longer inside {}: not saved", target.display(), root.display());
            return Err(outside(root));
        }
    }
    std::fs::rename(&part, &target).map_err(|e| format!("cannot save {}: {}", target.display(), e))?;
    let _ = std::fs::remove_file(part_paths(&target).1);
    info!("Received {}", target.display());
    Ok(json!({"type": "done", "id": id, "path": path_str(&target)}))
}

async fn download(ch: &Arc<dyn DataChannel>, id: &str, access: &Access, path: &Path, offset: u64, cancel: &AtomicBool) -> Result<(), String> {
    use tokio::io::{AsyncReadExt, AsyncSeekExt};
    // Opened first, then asked what it is: the file that is sent is the one
    // that was checked (`open_inside`).
    let std_file = open_inside(access, path, std::fs::OpenOptions::new().read(true))?;
    let meta = std_file.metadata().map_err(|e| format!("{}: {}", path.display(), e))?;
    if !meta.is_file() {
        return Err(format!("{} is not a file", path.display()));
    }
    let size = meta.len();
    let offset = offset.min(size);
    let mut file = tokio::fs::File::from_std(std_file);
    if offset > 0 {
        file.seek(std::io::SeekFrom::Start(offset)).await.map_err(|e| e.to_string())?;
    }
    send_json(ch, json!({"type": "get_start", "id": id, "size": size, "mtime": mtime_of(&meta), "offset": offset})).await;
    let mut buf = vec![0u8; CHUNK];
    loop {
        if cancel.load(Ordering::SeqCst) {
            // Nothing of this download follows (the channel is ordered).
            send_json(ch, json!({"type": "get_end", "id": id, "cancelled": true})).await;
            return Ok(());
        }
        let n = file.read(&mut buf).await.map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        // 0.21 has no bufferedAmount; the bytes SCTP has not released yet
        // (queued or unacknowledged) are what bounds the send buffer.
        while ch.outstanding_bytes().await.unwrap_or(0) > MAX_BUFFERED {
            if !matches!(ch.ready_state().await, Ok(RTCDataChannelState::Open)) {
                return Ok(());
            }
            if cancel.load(Ordering::SeqCst) {
                send_json(ch, json!({"type": "get_end", "id": id, "cancelled": true})).await;
                return Ok(());
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        ch.send(bytes::BytesMut::from(&buf[..n])).await.map_err(|e| e.to_string())?;
    }
    send_json(ch, json!({"type": "get_end", "id": id})).await;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("lunaris-files-{}-{}", name, std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn uploads_resume_and_skip_identical_files() {
        let d = tmp("up");
        let target = d.join("a.bin");
        // First attempt stops after 3 bytes.
        let (reply, up) = begin_upload("1", &Access::Full, &target, 6, 1_700_000_000, OnExists::Rename).unwrap();
        assert_eq!(reply["offset"], 0);
        let mut up = up.unwrap();
        up.file.write_all(b"abc").unwrap();
        up.received = 3;
        drop(up);
        // The second resumes at 3 and completes.
        let (reply, up) = begin_upload("2", &Access::Full, &target, 6, 1_700_000_000, OnExists::Rename).unwrap();
        assert_eq!(reply["offset"], 3);
        let mut up = up.unwrap();
        up.file.write_all(b"def").unwrap();
        up.received += 3;
        let done = finish_upload(up).unwrap();
        assert_eq!(done["type"], "done");
        assert_eq!(std::fs::read(&target).unwrap(), b"abcdef");
        assert!(!part_paths(&target).0.exists() && !part_paths(&target).1.exists());
        // Same size and time: skipped. Different: a new name.
        let (reply, _) = begin_upload("3", &Access::Full, &target, 6, 1_700_000_000, OnExists::Rename).unwrap();
        assert_eq!(reply["type"], "put_skip");
        let (reply, _) = begin_upload("4", &Access::Full, &target, 7, 1_700_000_000, OnExists::Rename).unwrap();
        assert_eq!(reply["path"], path_str(&d.join("a (2).bin")));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn walks_folders_and_hides_parts() {
        let d = tmp("walk");
        std::fs::create_dir_all(d.join("x/y")).unwrap();
        std::fs::write(d.join("x/y/f.txt"), b"1").unwrap();
        std::fs::write(d.join("g.txt.lunaris-part"), b"1").unwrap();
        let v = walk("1", &Access::Full, &d).unwrap();
        assert_eq!(v["files"].as_array().unwrap().len(), 1);
        assert_eq!(v["files"][0]["rel"], "x/y/f.txt");
        let l = list("2", &Access::Full, &path_str(&d), false).unwrap();
        assert_eq!(l["entries"].as_array().unwrap().len(), 1);
        let _ = std::fs::remove_dir_all(&d);
    }

    /// A folder `root` inside `d` as the allowed one, and a file beside it.
    fn restricted(name: &str) -> (PathBuf, Access, PathBuf) {
        let d = tmp(name);
        std::fs::create_dir_all(d.join("root/sub")).unwrap();
        std::fs::write(d.join("secret.txt"), b"s").unwrap();
        std::fs::write(d.join("root/a.txt"), b"a").unwrap();
        let access = Access::from_config(&path_str(&d.join("root")));
        let Access::Root(root) = &access else { panic!("{:?}", access) };
        let root = root.clone();
        (d, access, root)
    }

    #[test]
    fn paths_stay_inside_the_allowed_folder() {
        let (d, access, root) = restricted("access");
        assert_eq!(resolve(&access, "").unwrap(), root);
        assert_eq!(resolve(&access, "~/a.txt").unwrap(), root.join("a.txt"));
        assert_eq!(resolve(&access, &path_str(&root.join("sub"))).unwrap(), root.join("sub"));
        // Traversal with `..`, existing and not.
        assert!(resolve(&access, &path_str(&root.join("../secret.txt"))).is_err());
        assert!(resolve(&access, &path_str(&root.join("sub/../../secret.txt"))).is_err());
        assert!(resolve(&access, &path_str(&root.join("new/../../secret.txt"))).is_err());
        assert!(resolve(&access, "~/../secret.txt").is_err());
        // Absolute paths elsewhere, relative paths.
        assert!(resolve(&access, &path_str(&d.join("secret.txt"))).is_err());
        assert!(resolve(&access, "/").is_err());
        // Relative paths are inside the folder.
        assert_eq!(resolve(&access, "a.txt").unwrap(), root.join("a.txt"));
        assert!(resolve(&access, "../secret.txt").is_err());
        // A file that does not exist yet (upload target), in a new folder too.
        assert_eq!(resolve(&access, &path_str(&root.join("new.bin"))).unwrap(), root.join("new.bin"));
        assert_eq!(resolve(&access, &path_str(&root.join("x/y/new.bin"))).unwrap(), root.join("x/y/new.bin"));
        // Listing stops at the allowed folder; roots offer only it.
        let l = list("1", &access, "", false).unwrap();
        assert!(l["parent"].is_null());
        let h = hello("2", &access);
        assert_eq!(h["roots"].as_array().unwrap().len(), 1);
        assert_eq!(h["roots"][0]["path"], path_str(&root));
        // The folder itself cannot be removed or renamed.
        assert!(resolve_entry(&access, "").is_err());
        assert!(resolve_entry(&access, &path_str(&root)).is_err());
        assert_eq!(resolve_entry(&access, "~/a.txt").unwrap(), root.join("a.txt"));
        // Rename checks both ends; a name can't leave the folder.
        let r = rename("3", &access, &path_str(&root.join("a.txt")), "../../secret2.txt");
        assert_eq!(r["type"], "ok");
        assert!(root.join("secret2.txt").exists() && !d.join("secret2.txt").exists());
        let r = rename("4", &access, &path_str(&d.join("secret.txt")), "mine.txt");
        assert_eq!(r["type"], "error");
        assert!(d.join("secret.txt").exists());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[cfg(unix)]
    #[test]
    fn links_cannot_lead_outside() {
        let (d, access, root) = restricted("links");
        std::os::unix::fs::symlink(d.join("secret.txt"), root.join("link.txt")).unwrap();
        std::os::unix::fs::symlink(&d, root.join("up")).unwrap();
        std::os::unix::fs::symlink(d.join("missing.txt"), root.join("dangling.txt")).unwrap();
        std::os::unix::fs::symlink(root.join("a.txt"), root.join("ok.txt")).unwrap();
        assert!(resolve(&access, &path_str(&root.join("link.txt"))).is_err());
        assert!(resolve(&access, &path_str(&root.join("up/secret.txt"))).is_err());
        assert!(resolve(&access, &path_str(&root.join("up/new.bin"))).is_err());
        // A broken link would be written through on upload.
        assert!(resolve(&access, &path_str(&root.join("dangling.txt"))).is_err());
        let put = begin_upload("1", &access, &root.join("dangling.txt"), 1, 0, OnExists::Replace);
        assert!(put.is_err());
        assert!(!d.join("missing.txt").exists());
        // Links inside are fine.
        assert_eq!(resolve(&access, &path_str(&root.join("ok.txt"))).unwrap(), root.join("a.txt"));
        // Links that lead out are not listed, and walk skips them.
        let l = list("2", &access, "", false).unwrap();
        let names: Vec<&str> = l["entries"].as_array().unwrap().iter().map(|e| e["name"].as_str().unwrap()).collect();
        assert!(!names.contains(&"link.txt") && !names.contains(&"up") && names.contains(&"ok.txt"));
        let w = walk("3", &access, &root).unwrap();
        assert!(!w["files"].to_string().contains("link.txt"));
        // Removing a link removes the link, not what it points to.
        assert_eq!(resolve_entry(&access, &path_str(&root.join("link.txt"))).unwrap(), root.join("link.txt"));
        let _ = std::fs::remove_dir_all(&d);
    }

    /// A folder on the way replaced by a link after the check (as a racing
    /// local user could): what is opened is checked again, and refused.
    #[cfg(unix)]
    #[test]
    fn a_link_put_in_after_the_check_is_not_opened() {
        let (d, access, root) = restricted("toctou");
        // Checked while it is a real folder...
        std::fs::create_dir_all(root.join("box")).unwrap();
        let checked = resolve(&access, &path_str(&root.join("box/secret.txt"))).unwrap();
        // ...then swapped for a link to the folder outside.
        std::fs::remove_dir_all(root.join("box")).unwrap();
        std::os::unix::fs::symlink(&d, root.join("box")).unwrap();
        let err = open_inside(&access, &checked, std::fs::OpenOptions::new().read(true)).unwrap_err();
        assert!(err.contains("outside"), "{err}");
        // Writing through it is refused too (the file may be made, nothing is written to it).
        let made = root.join("box/new.bin");
        assert!(open_inside(&access, &made, std::fs::OpenOptions::new().write(true).create(true).truncate(true)).is_err());
        // A link as the last name is not followed at all.
        std::os::unix::fs::symlink(d.join("secret.txt"), root.join("last.txt")).unwrap();
        let err = open_inside(&access, &root.join("last.txt"), std::fs::OpenOptions::new().read(true)).unwrap_err();
        assert!(err.contains("link"), "{err}");
        // A plain file inside opens.
        assert!(open_inside(&access, &root.join("a.txt"), std::fs::OpenOptions::new().read(true)).is_ok());
        // With the whole disk allowed, links are followed as before.
        assert!(open_inside(&Access::Full, &root.join("last.txt"), std::fs::OpenOptions::new().read(true)).is_ok());
        let _ = std::fs::remove_dir_all(&d);
    }

    /// The upload's folder replaced by a link before it is finished: the
    /// file is not moved there.
    #[cfg(unix)]
    #[test]
    fn a_finished_upload_is_not_moved_through_a_new_link() {
        let (d, access, root) = restricted("toctou-up");
        std::fs::create_dir_all(root.join("up")).unwrap();
        let target = resolve(&access, &path_str(&root.join("up/f.bin"))).unwrap();
        let (_, up) = begin_upload("1", &access, &target, 3, 1_700_000_000, OnExists::Replace).unwrap();
        let mut up = up.unwrap();
        up.write(b"abc").unwrap();
        // The part file stays where it was made; its folder is swapped.
        std::fs::rename(root.join("up"), root.join("up-moved")).unwrap();
        std::os::unix::fs::symlink(&d, root.join("up")).unwrap();
        assert!(finish_upload(up).unwrap_err().contains("outside"));
        assert!(!d.join("f.bin").exists());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn uploads_stop_at_the_announced_size() {
        let d = tmp("size");
        let (_, up) = begin_upload("1", &Access::Full, &d.join("f.bin"), 4, 0, OnExists::Replace).unwrap();
        let mut up = up.unwrap();
        up.write(b"abc").unwrap();
        assert!(up.write(b"de").is_err());
        assert_eq!(up.received, 3);
        up.write(b"d").unwrap();
        assert!(up.write(b"e").is_err());
        assert_eq!(finish_upload(up).unwrap()["type"], "done");
        assert_eq!(std::fs::read(d.join("f.bin")).unwrap(), b"abcd");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn names_windows_reads_differently_are_refused() {
        let (d, access, root) = restricted("names");
        // "..." is the folder itself on Windows: removing it would remove the root.
        for p in ["~\\...", "~/...", "~/. .", "~/a.txt.", "~/a /b", "~/sub/../...", "~/..../x"] {
            assert!(resolve(&access, p).is_err(), "{}", p);
            assert!(resolve_entry(&access, p).is_err(), "{}", p);
            assert!(resolve(&Access::Full, p).is_err(), "{}", p);
        }
        for p in ["~/CON", "~/nul.txt", "~/sub/com1", "~/LPT9.log", "~/aux "] {
            assert!(resolve(&access, p).is_err(), "{}", p);
        }
        assert!(resolve(&access, "~/console.txt").is_ok());
        assert!(resolve(&access, "~/com10").is_ok());
        assert_eq!(safe_name("..."), "file");
        assert_eq!(safe_name("a. . "), "a");
        assert_eq!(safe_name("CON"), "_CON");
        assert_eq!(safe_name("nul.txt"), "_nul.txt");
        // Renaming into such a name gives a safe one inside the folder.
        let r = rename("1", &access, "~/a.txt", "...");
        assert_eq!(r["type"], "ok");
        assert!(root.join("file").exists());
        // An entry whose real place is the folder or above is not changed.
        assert!(resolve_entry(&access, "~/sub/..").is_err());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn device_and_network_paths_are_refused_before_any_lookup() {
        let (d, access, _root) = restricted("unc");
        for p in [r"\\.\pipe\x", r"\\?\C:\x", r"\??\C:\x", "//./pipe/x", "//?/C:/x", r"\\.", r"\\?"] {
            assert!(check_path_text(p, true).is_err(), "{}", p);
            assert!(resolve(&access, p).is_err(), "{}", p);
            assert!(resolve(&Access::Full, p).is_err(), "{}", p);
        }
        for p in [r"\\server\share\x", "//server/share/x"] {
            assert!(check_path_text(p, false).is_err(), "{}", p);
            assert!(check_path_text(p, true).is_ok(), "{}", p);
            assert!(resolve(&access, p).is_err(), "{}", p);
        }
        assert!(lexically_inside(Path::new("/r"), Path::new("/r/a/../b")).is_some());
        assert!(lexically_inside(Path::new("/r"), Path::new("/r/../x")).is_none());
        assert!(lexically_inside(Path::new("/r"), Path::new("a/../../x")).is_none());
        assert_eq!(lexically_inside(Path::new("/r"), Path::new("a/b")), Some(PathBuf::from("/r/a/b")));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn a_session_names_its_source_or_this_computers_own_files() {
        let sources = || -> Vec<crate::connector::SourceConfig> {
            vec![serde_json::from_str(r#"{"id":"office-pc","name":"Máy văn phòng","connector":"rustdesk","host":"192.0.2.5","password":"x"}"#).unwrap()]
        };
        // No source: this computer's own files (and the list is not even read).
        assert!(source_target(None, || panic!("not asked")).is_none());
        assert!(source_target(Some(""), || panic!("not asked")).is_none());
        // A source it has: that computer, through its connector.
        let target = source_target(Some("office-pc"), sources).unwrap().unwrap();
        assert_eq!((target.connector.as_str(), target.get("host")), ("rustdesk", "192.0.2.5"));
        // One it does not have: an error, never its own files.
        assert_eq!(source_target(Some("gone"), sources).unwrap().unwrap_err(), "This computer no longer has that source");
        assert_eq!(source_target(Some("gone"), Vec::new).unwrap().unwrap_err(), "This computer no longer has that source");
    }

    #[test]
    fn a_sources_requests_and_answers_are_the_channels_protocol() {
        let req = |text: &str| source_command(serde_json::from_str(text).unwrap());
        assert_eq!(req(r#"{"type":"hello","id":"1"}"#), None);
        assert_eq!(req(r#"{"type":"list","id":"2","path":"C:\\Users","hidden":true}"#), Some(FileCommand::List { id: "2".into(), path: "C:\\Users".into(), hidden: true }));
        assert_eq!(req(r#"{"type":"walk","id":"3","path":"/a"}"#), Some(FileCommand::Walk { id: "3".into(), path: "/a".into() }));
        assert_eq!(req(r#"{"type":"mkdir","id":"4","path":"/a"}"#), Some(FileCommand::Mkdir { id: "4".into(), path: "/a".into() }));
        assert_eq!(req(r#"{"type":"remove","id":"5","path":"/a"}"#), Some(FileCommand::Remove { id: "5".into(), path: "/a".into() }));
        assert_eq!(req(r#"{"type":"rename","id":"6","path":"/a","name":"b"}"#), Some(FileCommand::Rename { id: "6".into(), path: "/a".into(), name: "b".into() }));
        assert_eq!(
            req(r#"{"type":"put","id":"7","path":"/a","size":9,"mtime":3,"on_exists":"replace"}"#),
            Some(FileCommand::Write { id: "7".into(), path: "/a".into(), size: 9, mtime: 3, on_exists: SourceOnExists::Replace })
        );
        // As the clients send it when they say nothing: kept beside the one there.
        assert_eq!(
            req(r#"{"type":"put","id":"7","path":"/a","size":9}"#),
            Some(FileCommand::Write { id: "7".into(), path: "/a".into(), size: 9, mtime: 0, on_exists: SourceOnExists::Rename })
        );
        assert_eq!(req(r#"{"type":"put_end","id":"7"}"#), Some(FileCommand::WriteEnd { id: "7".into() }));
        assert_eq!(req(r#"{"type":"get","id":"8","path":"/a","offset":5}"#), Some(FileCommand::Read { id: "8".into(), path: "/a".into(), offset: 5 }));
        assert_eq!(req(r#"{"type":"cancel","id":"8"}"#), Some(FileCommand::Cancel { id: "8".into() }));

        let entry = crate::connector::files::FileEntry { name: "ảnh.png".into(), dir: false, size: 7, mtime: 9, hidden: true };
        let reply = |event: FileEvent| source_reply(&event).unwrap();
        assert_eq!(
            reply(FileEvent::Dir { id: "2".into(), path: "C:\\".into(), parent: None, entries: vec![entry] }),
            json!({"type": "dir", "id": "2", "path": "C:\\", "parent": null, "entries": [{"name": "ảnh.png", "dir": false, "size": 7, "mtime": 9, "hidden": true}]})
        );
        assert_eq!(
            reply(FileEvent::Walk { id: "3".into(), path: "/a".into(), files: vec![crate::connector::files::WalkFile { rel: "x/y".into(), size: 1, mtime: 2 }], dirs: vec!["x".into()] }),
            json!({"type": "walk", "id": "3", "path": "/a", "files": [{"rel": "x/y", "size": 1, "mtime": 2}], "dirs": ["x"]})
        );
        assert_eq!(reply(FileEvent::Ok { id: "4".into() }), json!({"type": "ok", "id": "4"}));
        assert_eq!(reply(FileEvent::ReadStart { id: "8".into(), size: 10, mtime: 2, offset: 5 }), json!({"type": "get_start", "id": "8", "size": 10, "mtime": 2, "offset": 5}));
        assert_eq!(reply(FileEvent::ReadEnd { id: "8".into(), cancelled: false }), json!({"type": "get_end", "id": "8"}));
        assert_eq!(reply(FileEvent::ReadEnd { id: "8".into(), cancelled: true }), json!({"type": "get_end", "id": "8", "cancelled": true}));
        assert_eq!(reply(FileEvent::WriteReady { id: "7".into(), offset: 4, path: "/a".into() }), json!({"type": "put_ready", "id": "7", "offset": 4, "path": "/a"}));
        assert_eq!(reply(FileEvent::WriteSkip { id: "7".into(), reason: "identical".into(), path: "/a".into() }), json!({"type": "put_skip", "id": "7", "reason": "identical", "path": "/a"}));
        assert_eq!(reply(FileEvent::WriteDone { id: "7".into(), path: "/a (2)".into() }), json!({"type": "done", "id": "7", "path": "/a (2)"}));
        assert_eq!(reply(FileEvent::Error { id: "5".into(), message: "Không được phép".into() }), json!({"type": "error", "id": "5", "message": "Không được phép"}));
        assert!(source_reply(&FileEvent::ReadData { id: "8".into(), data: bytes::Bytes::new() }).is_none());

        // hello: what the connector knows; no home when it cannot tell.
        let mut info = crate::connector::files::FilesInfo {
            os: "windows".into(),
            sep: "\\".into(),
            home: Some("C:\\Users\\An".into()),
            roots: vec![crate::connector::files::Place { name: "C:".into(), path: "C:\\".into() }],
        };
        assert_eq!(
            source_hello("1", &info, Some("C:\\Users\\An\\Downloads")),
            json!({"type": "hello", "id": "1", "home": "C:\\Users\\An", "downloads": "C:\\Users\\An\\Downloads", "os": "windows", "sep": "\\", "roots": [{"name": "C:", "path": "C:\\"}], "source": true})
        );
        info.home = None;
        let hello = source_hello("1", &info, None);
        assert_eq!((hello["home"].as_str(), hello["downloads"].as_str()), (Some(""), Some("")));
    }

    /// The whole way of a source's file session: the viewer's end of the
    /// `files` channel (as the clients have it), this agent's session for a
    /// source, the RustDesk connector, and a RustDesk-protocol host over a
    /// folder (`connector::rustdesk_files::tests::start_host`).
    mod source_session {
        use super::*;
        use crate::connector::rustdesk_files::tests::{places, says_not_there, start_host, target, text, tmp, HostOptions};
        use webrtc::peer_connection::{RTCIceCandidateInit, RTCSessionDescription};

        struct ViewerEvents {
            ice: mpsc::UnboundedSender<RTCIceCandidateInit>,
            ready: mpsc::UnboundedSender<Arc<dyn DataChannel>>,
        }

        #[async_trait::async_trait]
        impl PeerConnectionEventHandler for ViewerEvents {
            async fn on_ice_candidate(&self, event: RTCPeerConnectionIceEvent) {
                if let Ok(c) = event.candidate.to_json() {
                    let _ = self.ice.send(c);
                }
            }

            async fn on_data_channel(&self, data_channel: Arc<dyn DataChannel>) {
                let _ = self.ready.send(data_channel);
            }
        }

        struct Viewer {
            channel: Arc<dyn DataChannel>,
            messages: mpsc::UnboundedReceiver<RTCDataChannelMessage>,
            _connections: (Arc<dyn PeerConnection>, Arc<dyn PeerConnection>),
        }

        impl Viewer {
            /// Connects to a file session for `source` on this computer only.
            async fn connect(source: Result<Target, String>) -> Viewer {
                let (agent_tx, mut agent_rx) = mpsc::unbounded_channel::<common::AgentMessage>();
                let (agent_pc, offer) = start_for_source("viewer".into(), agent_tx, Some(Vec::new()), source).await.unwrap();
                let (ice_tx, mut ice_rx) = mpsc::unbounded_channel();
                let (ready_tx, mut ready_rx) = mpsc::unbounded_channel();
                let settings = SettingEngineBuilder::new()
                    .with_include_loopback_candidate(true)
                    .with_network_types(vec![rtc::ice::network_type::NetworkType::Udp4])
                    .build();
                let pc: Arc<dyn PeerConnection> = Arc::new(
                    PeerConnectionBuilder::new()
                        .with_setting_engine(settings)
                        .with_configuration(RTCConfigurationBuilder::default().with_ice_servers(Vec::new()).build())
                        .with_handler(Arc::new(ViewerEvents { ice: ice_tx, ready: ready_tx }))
                        .with_udp_addrs(vec!["127.0.0.1:0"])
                        .build()
                        .await
                        .unwrap(),
                );
                pc.set_remote_description(RTCSessionDescription::offer(offer).unwrap()).await.unwrap();
                let answer = pc.create_answer(None).await.unwrap();
                pc.set_local_description(answer.clone()).await.unwrap();
                agent_pc.set_remote_description(RTCSessionDescription::answer(answer.sdp).unwrap()).await.unwrap();
                // Candidates both ways, as signaling would carry them.
                let (to_viewer, to_agent) = (pc.clone(), agent_pc.clone());
                tokio::spawn(async move {
                    while let Some(common::AgentMessage::Signaling(sig)) = agent_rx.recv().await {
                        if let common::SignalingMessage::IceCandidate { candidate: c, .. } = sig {
                            let _ = to_viewer
                                .add_ice_candidate(RTCIceCandidateInit {
                                    candidate: c.candidate,
                                    sdp_mid: c.sdp_mid,
                                    sdp_mline_index: c.sdp_mline_index,
                                    username_fragment: c.username_fragment,
                                    ..Default::default()
                                })
                                .await;
                        }
                    }
                });
                tokio::spawn(async move {
                    while let Some(candidate) = ice_rx.recv().await {
                        let _ = to_agent.add_ice_candidate(candidate).await;
                    }
                });
                let channel = tokio::time::timeout(Duration::from_secs(30), ready_rx.recv()).await.expect("the channel opens").unwrap();
                let (tx, messages) = mpsc::unbounded_channel();
                let reader = channel.clone();
                tokio::spawn(async move {
                    while let Some(event) = reader.poll().await {
                        if let DataChannelEvent::OnMessage(msg) = event {
                            if tx.send(msg).is_err() {
                                break;
                            }
                        }
                    }
                });
                Viewer { channel, messages, _connections: (pc, agent_pc) }
            }

            async fn message(&mut self) -> RTCDataChannelMessage {
                tokio::time::timeout(Duration::from_secs(30), self.messages.recv()).await.expect("an answer in time").expect("the channel is open")
            }

            /// A request and its answer.
            async fn ask(&mut self, request: Value) -> Value {
                self.channel.send_text(&request.to_string()).await.unwrap();
                self.answer().await
            }

            async fn answer(&mut self) -> Value {
                let msg = self.message().await;
                assert!(msg.is_string, "an answer, not file data");
                serde_json::from_slice(&msg.data).unwrap()
            }
        }

        #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
        async fn the_file_manager_works_on_a_rustdesk_computer() {
            let home = tmp("agent-source");
            std::fs::create_dir_all(home.join("Downloads")).unwrap();
            std::fs::create_dir_all(home.join("Tài liệu/con")).unwrap();
            let words = "Một con ngựa đau cả tàu bỏ cỏ. ".repeat(10_000).into_bytes();
            std::fs::write(home.join("Tài liệu/tục ngữ.txt"), &words).unwrap();
            std::fs::write(home.join("Tài liệu/con/a.bin"), b"abc").unwrap();
            let host = start_host(&home, HostOptions::default()).await;
            // The other computer is one of this system here: its paths,
            // its separator, its places ("/", or the drives of Windows).
            let home_text = text(&home);
            let at = |name: &str| text(&home.join(name));
            let mut v = Viewer::connect(Ok(target(host.address, &host.password))).await;

            // hello: the other computer's home, its Downloads folder, its places.
            let hello = v.ask(json!({"type": "hello", "id": "h"})).await;
            assert_eq!(
                hello,
                json!({
                    "type": "hello", "id": "h", "home": home_text, "downloads": at("Downloads"),
                    "os": std::env::consts::OS, "sep": std::path::MAIN_SEPARATOR_STR, "source": true,
                    "roots": places(&home),
                })
            );
            // The folder it starts in, as the clients ask for it.
            let dir = v.ask(json!({"type": "list", "id": "l1", "path": hello["home"], "hidden": false})).await;
            assert_eq!((dir["type"].as_str(), dir["path"].as_str(), dir["parent"].as_str()), (Some("dir"), Some(home_text.as_str()), home.parent().and_then(|p| p.to_str())));
            let mut names: Vec<&str> = dir["entries"].as_array().unwrap().iter().map(|e| e["name"].as_str().unwrap()).collect();
            names.sort();
            assert_eq!(names, ["Downloads", "Tài liệu"]);
            assert!(dir["entries"].as_array().unwrap().iter().all(|e| e["dir"] == true));

            // An upload in the clients' chunks, then the same file again.
            let data: Vec<u8> = (0..1_000_000u32).map(|i| (i.wrapping_mul(2654435761) >> 13) as u8).collect();
            let put = json!({"type": "put", "id": "u1", "path": at("Downloads/lên.bin"), "size": data.len(), "mtime": 1_700_000_000u64, "on_exists": "rename"});
            assert_eq!(v.ask(put.clone()).await, json!({"type": "put_ready", "id": "u1", "offset": 0, "path": at("Downloads/lên.bin")}));
            for chunk in data.chunks(CHUNK) {
                v.channel.send(bytes::BytesMut::from(chunk)).await.unwrap();
            }
            assert_eq!(v.ask(json!({"type": "put_end", "id": "u1"})).await, json!({"type": "done", "id": "u1", "path": at("Downloads/lên.bin")}));
            assert!(std::fs::read(home.join("Downloads/lên.bin")).unwrap() == data);
            let mut again = put;
            again["id"] = json!("u2");
            assert_eq!(v.ask(again).await, json!({"type": "put_skip", "id": "u2", "reason": "identical", "path": at("Downloads/lên.bin")}));

            // A download, whole and from an offset: get_start, chunks no
            // larger than the channel takes, get_end.
            for offset in [0usize, 123_456] {
                v.channel.send_text(&json!({"type": "get", "id": "d1", "path": at("Tài liệu/tục ngữ.txt"), "offset": offset}).to_string()).await.unwrap();
                let start = v.answer().await;
                assert_eq!((start["type"].as_str(), start["size"].as_u64(), start["offset"].as_u64()), (Some("get_start"), Some(words.len() as u64), Some(offset as u64)));
                let mut got = Vec::new();
                let end = loop {
                    let msg = v.message().await;
                    if msg.is_string {
                        break serde_json::from_slice::<Value>(&msg.data).unwrap();
                    }
                    assert!(msg.data.len() <= CHUNK);
                    got.extend_from_slice(&msg.data);
                };
                assert_eq!(end, json!({"type": "get_end", "id": "d1"}));
                assert!(got == words[offset..]);
            }

            // Everything under a folder; a new folder; a new name; gone.
            let walk = v.ask(json!({"type": "walk", "id": "w", "path": at("Tài liệu")})).await;
            let mut files: Vec<&str> = walk["files"].as_array().unwrap().iter().map(|f| f["rel"].as_str().unwrap()).collect();
            files.sort();
            assert_eq!((walk["type"].as_str(), files, &walk["dirs"]), (Some("walk"), vec!["con/a.bin", "tục ngữ.txt"], &json!(["con"])));
            assert_eq!(v.ask(json!({"type": "mkdir", "id": "m", "path": at("Mới")})).await, json!({"type": "ok", "id": "m"}));
            assert_eq!(v.ask(json!({"type": "rename", "id": "r", "path": at("Mới"), "name": "Cũ"})).await, json!({"type": "ok", "id": "r"}));
            assert!(home.join("Cũ").is_dir() && !home.join("Mới").exists());
            assert_eq!(v.ask(json!({"type": "remove", "id": "x", "path": at("Tài liệu")})).await, json!({"type": "ok", "id": "x"}));
            assert!(!home.join("Tài liệu").exists());
            // The other computer's words for what goes wrong.
            let error = v.ask(json!({"type": "list", "id": "l2", "path": at("không có"), "hidden": false})).await;
            assert_eq!((error["type"].as_str(), error["id"].as_str()), (Some("error"), Some("l2")));
            assert!(says_not_there(error["message"].as_str().unwrap()), "{}", error);
            let _ = std::fs::remove_dir_all(&home);
        }

        #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
        async fn a_source_that_cannot_be_opened_is_an_error_not_this_computers_files() {
            let home = tmp("agent-source-refused");
            // A source this computer does not have (any more).
            let mut v = Viewer::connect(Err("This computer no longer has that source".into())).await;
            assert_eq!(v.ask(json!({"type": "hello", "id": "h"})).await, json!({"type": "error", "id": "h", "message": "This computer no longer has that source"}));
            let listing = v.ask(json!({"type": "list", "id": "l", "path": "", "hidden": false})).await;
            assert_eq!(listing, json!({"type": "error", "id": "l", "message": "This computer no longer has that source"}));
            // A computer that does not give its files, and a wrong password.
            let closed = start_host(&home, HostOptions { file_permission: false, ..Default::default() }).await;
            let mut v = Viewer::connect(Ok(target(closed.address, &closed.password))).await;
            let hello = v.ask(json!({"type": "hello", "id": "h"})).await;
            assert!(hello["type"] == "error" && hello["message"].as_str().unwrap().contains("File transfer is switched off on the RustDesk computer"), "{}", hello);
            let host = start_host(&home, HostOptions::default()).await;
            let mut v = Viewer::connect(Ok(target(host.address, "sai"))).await;
            assert_eq!(v.ask(json!({"type": "hello", "id": "h"})).await, json!({"type": "error", "id": "h", "message": "Wrong password for the RustDesk computer"}));
            // A connector without files.
            let none: Target = serde_json::from_value(json!({"connector": "no-such-connector-installed"})).unwrap();
            let mut v = Viewer::connect(Ok(none)).await;
            let hello = v.ask(json!({"type": "hello", "id": "h"})).await;
            assert_eq!(hello["type"], "error", "{}", hello);
            let _ = std::fs::remove_dir_all(&home);
        }
    }

    #[test]
    fn access_settings() {
        assert_eq!(Access::from_config("full"), Access::Full);
        assert!(matches!(Access::from_config("relative/dir"), Access::Denied(_)));
        assert!(matches!(Access::from_config("/no/such/lunaris/folder"), Access::Denied(_)));
        if dirs::home_dir().is_some() {
            assert!(matches!(Access::from_config("home"), Access::Root(_)));
            assert!(matches!(Access::from_config(""), Access::Root(_)));
        }
    }
}
