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
//! Turned off on this computer by AgentConfig allow_file_transfer.

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

/// Chunks the agent sends. Receivers built on webrtc-rs (the desktop client)
/// read messages into a 65535-byte buffer, so stay below it.
pub const CHUNK: usize = 48 * 1024;
const MAX_BUFFERED: usize = 4 * 1024 * 1024;
const PART: &str = ".lunaris-part";
/// Most entries `walk` returns (a whole disk is not a sensible download).
const MAX_WALK: usize = 200_000;

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
) -> anyhow::Result<(Arc<dyn PeerConnection>, String)> {
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
            .with_handler(Arc::new(IceToSignaling { tx: agent_tx.clone(), client: client_id.clone() }))
            // The wildcard covers every interface but not loopback (the
            // setting above only allows it): list it, as 0.17 gathered it.
            .with_udp_addrs(vec!["0.0.0.0:0", "127.0.0.1:0"])
            .build()
            .await?,
    );

    let channel = pc
        .create_data_channel("files", Some(RTCDataChannelInit { ordered: true, ..Default::default() }))
        .await?;
    let (tx, rx) = mpsc::unbounded_channel::<RTCDataChannelMessage>();
    let reader = channel.clone();
    tokio::spawn(async move {
        while let Some(event) = reader.poll().await {
            if let DataChannelEvent::OnMessage(msg) = event {
                let _ = tx.send(msg);
            }
        }
    });
    tokio::spawn(serve(channel, rx, allowed, client_id));

    let offer = pc.create_offer(None).await?;
    pc.set_local_description(offer.clone()).await?;
    Ok((pc, offer.sdp))
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
}

async fn send_json(ch: &Arc<dyn DataChannel>, v: Value) {
    if let Err(e) = ch.send_text(&v.to_string()).await {
        warn!("Files: could not answer: {}", e);
    }
}

fn err(id: &str, message: impl std::fmt::Display) -> Value {
    json!({"type": "error", "id": id, "message": message.to_string()})
}

async fn serve(channel: Arc<dyn DataChannel>, mut rx: mpsc::UnboundedReceiver<RTCDataChannelMessage>, allowed: bool, client: String) {
    let mut upload: Option<Upload> = None;
    let downloads: Arc<Mutex<HashMap<String, Arc<AtomicBool>>>> = Arc::default();
    info!("File session with {}", client);

    while let Some(msg) = rx.recv().await {
        if !msg.is_string {
            // A chunk of the file being uploaded.
            let Some(up) = upload.as_mut() else { continue };
            if let Err(e) = up.file.write_all(&msg.data) {
                let id = up.id.clone();
                upload = None;
                send_json(&channel, err(&id, format!("could not write the file: {}", e))).await;
                continue;
            }
            up.received += msg.data.len() as u64;
            continue;
        }
        let req: Request = match serde_json::from_slice(&msg.data) {
            Ok(r) => r,
            Err(e) => {
                warn!("Files: bad request: {}", e);
                continue;
            }
        };
        if !allowed {
            let id = match &req {
                Request::Hello { id } => id.clone(),
                _ => String::new(),
            };
            send_json(&channel, err(&id, "File transfer is turned off on this computer")).await;
            continue;
        }
        let reply = match req {
            Request::Hello { id } => Some(hello(&id)),
            Request::List { id, path, hidden } => Some(list(&id, &path, hidden).unwrap_or_else(|e| err(&id, e))),
            Request::Walk { id, path } => {
                let p = resolve(&path);
                Some(tokio::task::spawn_blocking(move || walk(&id, &p).unwrap_or_else(|e| err(&id, e))).await.unwrap_or(Value::Null))
            }
            Request::Mkdir { id, path } => Some(simple(&id, std::fs::create_dir_all(resolve(&path)))),
            Request::Remove { id, path } => {
                let p = resolve(&path);
                let r = match std::fs::symlink_metadata(&p) {
                    Ok(m) if m.is_dir() => std::fs::remove_dir_all(&p),
                    Ok(_) => std::fs::remove_file(&p),
                    Err(e) => Err(e),
                };
                Some(simple(&id, r))
            }
            Request::Rename { id, path, name } => {
                let p = resolve(&path);
                let name = safe_name(&name);
                Some(match p.parent() {
                    Some(dir) if dir.join(&name).exists() => err(&id, format!("{} already exists", name)),
                    Some(dir) => simple(&id, std::fs::rename(&p, dir.join(&name))),
                    None => err(&id, "cannot rename this"),
                })
            }
            Request::Put { id, path, size, mtime, on_exists } => {
                // A new upload replaces an unfinished one (its part stays for resuming).
                upload = None;
                match begin_upload(&id, &resolve(&path), size, mtime, on_exists) {
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
                let flag = Arc::new(AtomicBool::new(false));
                downloads.lock().unwrap().insert(id.clone(), flag.clone());
                let ch = channel.clone();
                let downloads = downloads.clone();
                // Downloads run beside the requests, one chunk after another.
                tokio::spawn(async move {
                    if let Err(e) = download(&ch, &id, &resolve(&path), offset, &flag).await {
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

fn simple(id: &str, r: std::io::Result<()>) -> Value {
    match r {
        Ok(()) => json!({"type": "ok", "id": id}),
        Err(e) => err(id, e),
    }
}

fn home() -> PathBuf {
    dirs::home_dir().unwrap_or_else(|| PathBuf::from("/"))
}

/// "" or "~" is the home folder; "~/x" is in it.
fn resolve(path: &str) -> PathBuf {
    let p = path.trim();
    if p.is_empty() || p == "~" {
        return home();
    }
    if let Some(rest) = p.strip_prefix("~/") {
        return home().join(rest);
    }
    PathBuf::from(p)
}

fn safe_name(name: &str) -> String {
    let base = name.rsplit(['/', '\\']).next().unwrap_or("").trim();
    let cleaned: String = base
        .chars()
        .map(|c| if c.is_control() || matches!(c, ':' | '*' | '?' | '"' | '<' | '>' | '|') { '_' } else { c })
        .collect();
    let cleaned = cleaned.trim_matches('.').trim().to_string();
    if cleaned.is_empty() { "file".into() } else { cleaned }
}

fn mtime_of(m: &std::fs::Metadata) -> u64 {
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

fn hello(id: &str) -> Value {
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

fn list(id: &str, path: &str, hidden: bool) -> Result<Value, String> {
    let dir = resolve(path);
    let read = std::fs::read_dir(&dir).map_err(|e| format!("{}: {}", dir.display(), e))?;
    let mut entries = Vec::new();
    for entry in read.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.ends_with(PART) || name.ends_with(".lunaris-part.json") {
            continue;
        }
        // Follow links so a link to a folder opens like a folder.
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
        "parent": dir.parent().map(path_str),
        "entries": entries,
    }))
}

fn walk(id: &str, root: &Path) -> Result<Value, String> {
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

fn begin_upload(id: &str, path: &Path, size: u64, mtime: u64, on_exists: OnExists) -> Result<(Value, Option<Upload>), String> {
    let dir = path.parent().ok_or("no folder")?;
    let name = safe_name(&path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default());
    std::fs::create_dir_all(dir).map_err(|e| format!("cannot create {}: {}", dir.display(), e))?;
    let mut target = dir.join(&name);

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
    let resume = std::fs::read(&info_path)
        .ok()
        .and_then(|b| serde_json::from_slice::<PartInfo>(&b).ok())
        .filter(|i| *i == PartInfo { size, mtime } && mtime != 0)
        .and_then(|_| std::fs::metadata(&part).ok())
        .map(|m| m.len())
        .filter(|&len| len <= size)
        .unwrap_or(0);
    let file = if resume > 0 {
        std::fs::OpenOptions::new().append(true).open(&part)
    } else {
        std::fs::File::create(&part)
    }
    .map_err(|e| format!("cannot write {}: {}", part.display(), e))?;
    let _ = std::fs::write(&info_path, serde_json::to_vec(&PartInfo { size, mtime }).unwrap_or_default());
    if resume > 0 {
        info!("Resuming {} at {} of {} bytes", target.display(), resume, size);
    } else {
        info!("Receiving {} ({} bytes)", target.display(), size);
    }
    let up = Upload { id: id.to_string(), target: target.clone(), part, file, size, mtime, received: resume };
    Ok((json!({"type": "put_ready", "id": id, "offset": resume, "path": path_str(&target)}), Some(up)))
}

fn finish_upload(up: Upload) -> Result<Value, String> {
    let Upload { id, target, part, file, size, mtime, received } = up;
    let _ = file.sync_all();
    if received != size {
        return Err(format!("incomplete: {} of {} bytes (it can be resumed)", received, size));
    }
    if mtime > 0 {
        let _ = file.set_modified(UNIX_EPOCH + Duration::from_secs(mtime));
    }
    drop(file);
    std::fs::rename(&part, &target).map_err(|e| format!("cannot save {}: {}", target.display(), e))?;
    let _ = std::fs::remove_file(part_paths(&target).1);
    info!("Received {}", target.display());
    Ok(json!({"type": "done", "id": id, "path": path_str(&target)}))
}

async fn download(ch: &Arc<dyn DataChannel>, id: &str, path: &Path, offset: u64, cancel: &AtomicBool) -> Result<(), String> {
    use tokio::io::{AsyncReadExt, AsyncSeekExt};
    let meta = tokio::fs::metadata(path).await.map_err(|e| format!("{}: {}", path.display(), e))?;
    if !meta.is_file() {
        return Err(format!("{} is not a file", path.display()));
    }
    let size = meta.len();
    let offset = offset.min(size);
    let mut file = tokio::fs::File::open(path).await.map_err(|e| format!("{}: {}", path.display(), e))?;
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
        let (reply, up) = begin_upload("1", &target, 6, 1_700_000_000, OnExists::Rename).unwrap();
        assert_eq!(reply["offset"], 0);
        let mut up = up.unwrap();
        up.file.write_all(b"abc").unwrap();
        up.received = 3;
        drop(up);
        // The second resumes at 3 and completes.
        let (reply, up) = begin_upload("2", &target, 6, 1_700_000_000, OnExists::Rename).unwrap();
        assert_eq!(reply["offset"], 3);
        let mut up = up.unwrap();
        up.file.write_all(b"def").unwrap();
        up.received += 3;
        let done = finish_upload(up).unwrap();
        assert_eq!(done["type"], "done");
        assert_eq!(std::fs::read(&target).unwrap(), b"abcdef");
        assert!(!part_paths(&target).0.exists() && !part_paths(&target).1.exists());
        // Same size and time: skipped. Different: a new name.
        let (reply, _) = begin_upload("3", &target, 6, 1_700_000_000, OnExists::Rename).unwrap();
        assert_eq!(reply["type"], "put_skip");
        let (reply, _) = begin_upload("4", &target, 7, 1_700_000_000, OnExists::Rename).unwrap();
        assert_eq!(reply["path"], path_str(&d.join("a (2).bin")));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn walks_folders_and_hides_parts() {
        let d = tmp("walk");
        std::fs::create_dir_all(d.join("x/y")).unwrap();
        std::fs::write(d.join("x/y/f.txt"), b"1").unwrap();
        std::fs::write(d.join("g.txt.lunaris-part"), b"1").unwrap();
        let v = walk("1", &d).unwrap();
        assert_eq!(v["files"].as_array().unwrap().len(), 1);
        assert_eq!(v["files"][0]["rel"], "x/y/f.txt");
        let l = list("2", &path_str(&d), false).unwrap();
        assert_eq!(l["entries"].as_array().unwrap().len(), 1);
        let _ = std::fs::remove_dir_all(&d);
    }
}
