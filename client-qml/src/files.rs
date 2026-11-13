//! File transfer with a remote computer on a connection of its own (its own
//! signaling WebSocket and WebRTC connection, like RustDesk's file-transfer
//! connection): the stream's video and input never wait behind a transfer,
//! and files can be moved without a stream. Protocol: agent/src/files.rs.
//!
//! The file manager page (FileManager.qml) drives it through the bridge and
//! reads `state_json()` (versioned, so it only re-reads on changes): the
//! connection, the remote folder shown, and the transfer queue. Transfers run
//! one after another, both ways, folders included; identical files are
//! skipped and interrupted ones resume (`.lunaris-part` on either side).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant, UNIX_EPOCH};

use bytes::Bytes;
use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::Message as WsMessage;
use webrtc::data_channel::{DataChannel, DataChannelEvent, RTCDataChannelMessage, RTCDataChannelState};
use webrtc::peer_connection::{
    PeerConnection, PeerConnectionBuilder, PeerConnectionEventHandler, RTCConfigurationBuilder, RTCIceCandidateInit,
    RTCIceServer, RTCPeerConnectionIceEvent, RTCSessionDescription,
};

use common::{ClientMessage, RtcIceCandidate, RtcSdpType, RtcSessionDescription, ServerToClientMessage, SignalingMessage};

/// webrtc-rs (the agent) reads messages into a 65535-byte buffer.
const CHUNK: usize = 48 * 1024;
const MAX_BUFFERED: usize = 4 * 1024 * 1024;
const PART: &str = ".lunaris-part";

fn runtime() -> &'static tokio::runtime::Runtime {
    static RT: OnceLock<tokio::runtime::Runtime> = OnceLock::new();
    RT.get_or_init(|| {
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .thread_name("lunaris-files")
            .enable_all()
            .build()
            .expect("file transfer runtime")
    })
}

// ---------------------------------------------------------------- state

#[derive(Clone, Copy, PartialEq, Eq)]
enum JobState {
    Queued,
    Running,
    Done,
    Skipped,
    Failed,
    Cancelled,
}

impl JobState {
    fn as_str(self) -> &'static str {
        match self {
            JobState::Queued => "queued",
            JobState::Running => "running",
            JobState::Done => "done",
            JobState::Skipped => "skipped",
            JobState::Failed => "failed",
            JobState::Cancelled => "cancelled",
        }
    }
}

struct Job {
    id: String,
    up: bool,
    name: String,
    local: PathBuf,
    remote: String,
    size: u64,
    mtime: u64,
    done: u64,
    state: JobState,
    message: String,
    started: Option<Instant>,
    cancel: Arc<AtomicBool>,
}

#[derive(Default)]
struct Shared {
    host_name: String,
    status: String,
    error: String,
    connected: bool,
    hello: Value,
    remote: Value,
    jobs: Vec<Job>,
}

static STATE: Mutex<Option<Shared>> = Mutex::new(None);
static VERSION: AtomicU64 = AtomicU64::new(1);
static CONN: Mutex<Option<Arc<Conn>>> = Mutex::new(None);
static WORKER: AtomicBool = AtomicBool::new(false);
static SEQ: AtomicU64 = AtomicU64::new(0);

fn with_state<R>(f: impl FnOnce(&mut Shared) -> R) -> R {
    let mut guard = STATE.lock().unwrap();
    let s = guard.get_or_insert_with(Shared::default);
    let r = f(s);
    VERSION.fetch_add(1, Ordering::Relaxed);
    r
}

pub fn version() -> u64 {
    VERSION.load(Ordering::Relaxed)
}

fn new_id() -> String {
    let n = SEQ.fetch_add(1, Ordering::Relaxed);
    let t = std::time::SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis()).unwrap_or(0);
    format!("c{:x}{:x}", t, n)
}

/// Everything the page shows.
pub fn state_json() -> String {
    let guard = STATE.lock().unwrap();
    let Some(s) = guard.as_ref() else {
        return json!({"connected": false, "status": "", "error": "", "jobs": []}).to_string();
    };
    let jobs: Vec<Value> = s
        .jobs
        .iter()
        .rev()
        .map(|j| {
            let speed = match (j.state, j.started) {
                (JobState::Running, Some(t)) if t.elapsed().as_secs_f64() > 0.5 => (j.done as f64 / t.elapsed().as_secs_f64()) as u64,
                _ => 0,
            };
            json!({
                "id": j.id, "up": j.up, "name": j.name, "remote": j.remote, "local": j.local.to_string_lossy(),
                "size": j.size, "done": j.done, "state": j.state.as_str(), "message": j.message, "speed": speed,
            })
        })
        .collect();
    json!({
        "host": s.host_name, "connected": s.connected, "status": s.status, "error": s.error,
        "hello": s.hello, "remote": s.remote, "jobs": jobs,
        "active": s.jobs.iter().filter(|j| matches!(j.state, JobState::Queued | JobState::Running)).count(),
    })
    .to_string()
}

fn set_error(e: impl Into<String>) {
    let e = e.into();
    with_state(|s| s.error = e);
}

// ---------------------------------------------------------------- connection

struct Conn {
    ch: Arc<dyn DataChannel>,
    pc: Arc<dyn PeerConnection>,
    waiting: Mutex<HashMap<String, mpsc::UnboundedSender<Value>>>,
    /// Chunks of the download in progress.
    sink: Mutex<Option<mpsc::UnboundedSender<Bytes>>>,
    /// The remote computer's path separator.
    sep: Mutex<String>,
}

impl Conn {
    fn sep(&self) -> String {
        self.sep.lock().unwrap().clone()
    }

    fn on_message(&self, msg: RTCDataChannelMessage) {
        if !msg.is_string {
            if let Some(tx) = self.sink.lock().unwrap().as_ref() {
                let _ = tx.send(msg.data.freeze());
            }
            return;
        }
        let Ok(v) = serde_json::from_slice::<Value>(&msg.data) else { return };
        if let Some(id) = v.get("id").and_then(Value::as_str) {
            if let Some(tx) = self.waiting.lock().unwrap().get(id) {
                let _ = tx.send(v.clone());
            }
        }
    }

    /// Sends a request; replies with its id arrive on the returned receiver.
    async fn start(&self, mut body: Value) -> Result<(String, mpsc::UnboundedReceiver<Value>), String> {
        let id = body.get("id").and_then(Value::as_str).map(str::to_string).unwrap_or_else(new_id);
        body["id"] = json!(id);
        let (tx, rx) = mpsc::unbounded_channel();
        self.waiting.lock().unwrap().insert(id.clone(), tx);
        if let Err(e) = self.ch.send_text(&body.to_string()).await {
            self.waiting.lock().unwrap().remove(&id);
            return Err(e.to_string());
        }
        Ok((id, rx))
    }

    async fn request(&self, body: Value, secs: u64) -> Result<Value, String> {
        let (id, mut rx) = self.start(body).await?;
        let r = tokio::time::timeout(Duration::from_secs(secs), rx.recv()).await;
        self.waiting.lock().unwrap().remove(&id);
        match r {
            Ok(Some(v)) if v.get("type").and_then(Value::as_str) == Some("error") => {
                Err(v.get("message").and_then(Value::as_str).unwrap_or("error").to_string())
            }
            Ok(Some(v)) => Ok(v),
            Ok(None) => Err("connection closed".into()),
            Err(_) => Err("no answer from the remote computer".into()),
        }
    }

    async fn drain(&self) -> Result<(), String> {
        // 0.21 has no bufferedAmount: the bytes SCTP has not released yet.
        while self.ch.outstanding_bytes().await.unwrap_or(0) > MAX_BUFFERED {
            if !matches!(self.ch.ready_state().await, Ok(RTCDataChannelState::Open)) {
                return Err("connection closed".into());
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        Ok(())
    }
}

fn conn() -> Result<Arc<Conn>, String> {
    CONN.lock().unwrap().clone().ok_or_else(|| "not connected".to_string())
}

/// Opens the file connection to `host_id` (closing any other).
pub fn open(server_url: String, token: String, host_id: String, host_name: String) {
    close();
    with_state(|s| {
        *s = Shared { host_name, status: "Connecting…".into(), ..Default::default() };
    });
    runtime().spawn(async move {
        if let Err(e) = connect(server_url, token, host_id).await {
            with_state(|s| {
                s.status.clear();
                s.error = e;
                s.connected = false;
            });
        }
    });
}

pub fn close() {
    if let Some(c) = CONN.lock().unwrap().take() {
        let pc = c.pc.clone();
        runtime().spawn(async move {
            let _ = pc.close().await;
        });
    }
    with_state(|s| {
        for j in &s.jobs {
            j.cancel.store(true, Ordering::SeqCst);
        }
        s.connected = false;
    });
}

/// Events of the file connection (webrtc 0.21: a handler object instead of
/// `on_ice_candidate` / `on_data_channel` callbacks).
struct FilesHandler {
    ice_out: mpsc::UnboundedSender<SignalingMessage>,
    target: String,
    ready: mpsc::UnboundedSender<Arc<dyn DataChannel>>,
}

#[async_trait::async_trait]
impl PeerConnectionEventHandler for FilesHandler {
    async fn on_ice_candidate(&self, event: RTCPeerConnectionIceEvent) {
        if let Ok(c) = event.candidate.to_json() {
            let _ = self.ice_out.send(SignalingMessage::IceCandidate {
                target_id: self.target.clone(),
                candidate: RtcIceCandidate { candidate: c.candidate, sdp_mid: c.sdp_mid, sdp_mline_index: c.sdp_mline_index, username_fragment: c.username_fragment },
            });
        }
    }

    // Called once the remote channel is open (0.17 called it when the channel
    // was announced, and `on_open` followed).
    async fn on_data_channel(&self, data_channel: Arc<dyn DataChannel>) {
        if data_channel.label().await.map(|l| l == "files").unwrap_or(false) {
            let _ = self.ready.send(data_channel);
        }
    }
}

async fn connect(server_url: String, token: String, host_id: String) -> Result<(), String> {
    let encoded: String = url::form_urlencoded::byte_serialize(token.as_bytes()).collect();
    let ws_url = format!("{}/ws/client?token={}{}", server_url.trim_end_matches('/').replacen("http", "ws", 1), encoded, crate::host::client_name_query());
    let (ws, _) = tokio::time::timeout(Duration::from_secs(10), tokio_tungstenite::connect_async(ws_url.as_str()))
        .await
        .map_err(|_| "cannot reach the server".to_string())?
        .map_err(|e| format!("cannot reach the server: {}", e))?;
    let (mut ws_tx, mut ws_rx) = ws.split();
    let (out_tx, mut out_rx) = mpsc::unbounded_channel::<SignalingMessage>();
    tokio::spawn(async move {
        while let Some(sig) = out_rx.recv().await {
            let Ok(text) = serde_json::to_string(&ClientMessage::Signaling(sig)) else { continue };
            if ws_tx.send(WsMessage::Text(text.into())).await.is_err() {
                break;
            }
        }
        let _ = ws_tx.close().await;
    });
    let _ = out_tx.send(SignalingMessage::RequestFileSession { host_id: host_id.clone() });

    let mut pc: Option<Arc<dyn PeerConnection>> = None;
    let (ready_tx, mut ready_rx) = mpsc::unbounded_channel::<Arc<dyn DataChannel>>();
    let mut deadline = tokio::time::Instant::now() + Duration::from_secs(25);
    let channel = loop {
        tokio::select! {
            msg = ws_rx.next() => {
                let text = match msg {
                    Some(Ok(WsMessage::Text(t))) => t,
                    Some(Ok(_)) => continue,
                    _ => return Err("connection to the server closed".into()),
                };
                let Ok(ServerToClientMessage::Signaling(sig)) = serde_json::from_str::<ServerToClientMessage>(&text) else { continue };
                match sig {
                    SignalingMessage::Error { message } => return Err(message),
                    SignalingMessage::AccessRequest { status, .. } => {
                        if let Some(refusal) = crate::host::access_refusal(&status) {
                            return Err(refusal);
                        }
                        if status == "waiting" {
                            // The person there has 30 s to answer.
                            deadline = tokio::time::Instant::now() + Duration::from_secs(50);
                            with_state(|s| s.status = "Waiting for the person at the remote computer to accept…".into());
                        } else {
                            with_state(|s| s.status = "Connecting…".into());
                        }
                    }
                    SignalingMessage::Sdp { sdp, ice_servers, .. } if sdp.ty == RtcSdpType::Offer => {
                        let servers = ice_servers.unwrap_or_default().into_iter().map(|s| RTCIceServer {
                            urls: s.urls, username: s.username.unwrap_or_default(), credential: s.credential.unwrap_or_default(),
                        }).collect::<Vec<_>>();
                        let handler = Arc::new(FilesHandler { ice_out: out_tx.clone(), target: host_id.clone(), ready: ready_tx.clone() });
                        let p: Arc<dyn PeerConnection> = Arc::new(PeerConnectionBuilder::new()
                            .with_configuration(RTCConfigurationBuilder::default()
                                .with_ice_servers(if servers.is_empty() { vec![RTCIceServer { urls: vec!["stun:stun.l.google.com:19302".into()], ..Default::default() }] } else { servers })
                                .build())
                            .with_handler(handler)
                            .with_udp_addrs(vec!["0.0.0.0:0"])
                            .build()
                            .await
                            .map_err(|e| e.to_string())?);
                        p.set_remote_description(RTCSessionDescription::offer(sdp.sdp).map_err(|e| e.to_string())?).await.map_err(|e| e.to_string())?;
                        let answer = p.create_answer(None).await.map_err(|e| e.to_string())?;
                        p.set_local_description(answer.clone()).await.map_err(|e| e.to_string())?;
                        let _ = out_tx.send(SignalingMessage::Sdp {
                            target_id: host_id.clone(),
                            sdp: RtcSessionDescription { ty: RtcSdpType::Answer, sdp: answer.sdp },
                            ice_servers: None, webtransport_port: None, webtransport_cert_hash: None,
                        });
                        pc = Some(p);
                    }
                    SignalingMessage::IceCandidate { candidate, .. } => {
                        if let Some(p) = &pc {
                            let _ = p.add_ice_candidate(RTCIceCandidateInit {
                                candidate: candidate.candidate, sdp_mid: candidate.sdp_mid,
                                sdp_mline_index: candidate.sdp_mline_index, username_fragment: candidate.username_fragment,
                                ..Default::default()
                            }).await;
                        }
                    }
                    _ => {}
                }
            }
            Some(ch) = ready_rx.recv() => break ch,
            _ = tokio::time::sleep_until(deadline) => return Err("the remote computer did not answer".into()),
        }
    };
    let pc = pc.ok_or("no connection")?;

    // Keep relaying candidates while the connection lives.
    let pc_for_ice = pc.clone();
    tokio::spawn(async move {
        while let Some(Ok(msg)) = ws_rx.next().await {
            let WsMessage::Text(text) = msg else { continue };
            if let Ok(ServerToClientMessage::Signaling(SignalingMessage::IceCandidate { candidate, .. })) = serde_json::from_str(&text) {
                let _ = pc_for_ice
                    .add_ice_candidate(RTCIceCandidateInit {
                        candidate: candidate.candidate,
                        sdp_mid: candidate.sdp_mid,
                        sdp_mline_index: candidate.sdp_mline_index,
                        username_fragment: candidate.username_fragment,
                        ..Default::default()
                    })
                    .await;
            }
        }
        drop(out_tx);
    });

    let conn = Arc::new(Conn { ch: channel.clone(), pc: pc.clone(), waiting: Mutex::new(HashMap::new()), sink: Mutex::new(None), sep: Mutex::new("/".into()) });
    let weak = Arc::downgrade(&conn);
    // webrtc 0.21: the channel's events are polled.
    let reader = channel.clone();
    tokio::spawn(async move {
        while let Some(event) = reader.poll().await {
            match event {
                DataChannelEvent::OnMessage(m) => {
                    if let Some(c) = weak.upgrade() {
                        c.on_message(m);
                    }
                }
                DataChannelEvent::OnClose => with_state(|s| {
                    s.connected = false;
                    s.error = "connection closed".into();
                }),
                _ => {}
            }
        }
    });

    let hello = conn.request(json!({"type": "hello"}), 15).await?;
    *conn.sep.lock().unwrap() = hello.get("sep").and_then(Value::as_str).unwrap_or("/").to_string();
    *CONN.lock().unwrap() = Some(conn.clone());
    let home = hello.get("home").and_then(Value::as_str).unwrap_or("").to_string();
    with_state(|s| {
        s.connected = true;
        s.status.clear();
        s.error.clear();
        s.hello = hello;
    });
    list_remote_now(&conn, &home, false).await;
    // Uploads queued before the connection was up.
    kick();
    Ok(())
}

async fn list_remote_now(conn: &Conn, path: &str, hidden: bool) {
    match conn.request(json!({"type": "list", "path": path, "hidden": hidden}), 30).await {
        Ok(v) => with_state(|s| {
            s.remote = v;
            s.error.clear();
        }),
        Err(e) => set_error(e),
    }
}

pub fn list_remote(path: String, hidden: bool) {
    runtime().spawn(async move {
        match conn() {
            Ok(c) => list_remote_now(&c, &path, hidden).await,
            Err(e) => set_error(e),
        }
    });
}

fn refresh_remote() {
    let path = STATE
        .lock()
        .unwrap()
        .as_ref()
        .and_then(|s| s.remote.get("path").and_then(Value::as_str).map(str::to_string));
    if let Some(p) = path {
        list_remote(p, false);
    }
}

/// mkdir / remove / rename on the remote computer, then a fresh listing.
pub fn remote_op(op: &str, path: String, name: String) {
    let body = match op {
        "mkdir" => json!({"type": "mkdir", "path": path}),
        "remove" => json!({"type": "remove", "path": path}),
        "rename" => json!({"type": "rename", "path": path, "name": name}),
        _ => return,
    };
    runtime().spawn(async move {
        match conn() {
            Ok(c) => {
                if let Err(e) = c.request(body, 120).await {
                    set_error(e);
                }
                refresh_remote();
            }
            Err(e) => set_error(e),
        }
    });
}

// ---------------------------------------------------------------- local files

fn mtime_of(m: &std::fs::Metadata) -> u64 {
    m.modified().ok().and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map(|d| d.as_secs()).unwrap_or(0)
}

fn hidden_name(name: &str, _m: Option<&std::fs::Metadata>) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        if let Some(m) = _m {
            if m.file_attributes() & 0x2 != 0 {
                return true;
            }
        }
    }
    name.starts_with('.')
}

/// This computer's places (home, desktop, ... drives) for the local pane.
pub fn local_places() -> String {
    let mut roots = Vec::new();
    let mut add = |name: &str, p: Option<PathBuf>| {
        if let Some(p) = p {
            roots.push(json!({"name": name, "path": p.to_string_lossy()}));
        }
    };
    add("Home", dirs::home_dir());
    add("Desktop", dirs::desktop_dir());
    add("Documents", dirs::document_dir());
    add("Downloads", dirs::download_dir());
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
        "roots": roots,
        "home": dirs::home_dir().map(|p| p.to_string_lossy().into_owned()).unwrap_or_default(),
        "sep": std::path::MAIN_SEPARATOR.to_string(),
    })
    .to_string()
}

/// A folder of this computer, in the same form as the remote listing.
pub fn list_local(path: &str, hidden: bool) -> String {
    let dir = if path.trim().is_empty() { dirs::home_dir().unwrap_or_else(|| PathBuf::from("/")) } else { PathBuf::from(path) };
    let read = match std::fs::read_dir(&dir) {
        Ok(r) => r,
        Err(e) => return json!({"error": format!("{}: {}", dir.display(), e)}).to_string(),
    };
    let mut entries = Vec::new();
    for e in read.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        if name.ends_with(PART) {
            continue;
        }
        let meta = std::fs::metadata(e.path()).or_else(|_| e.metadata()).ok();
        let hide = hidden_name(&name, meta.as_ref());
        if hide && !hidden {
            continue;
        }
        let is_dir = meta.as_ref().map(|m| m.is_dir()).unwrap_or(false);
        entries.push(json!({
            "name": name, "dir": is_dir, "hidden": hide,
            "size": meta.as_ref().filter(|m| !m.is_dir()).map(|m| m.len()).unwrap_or(0),
            "mtime": meta.as_ref().map(mtime_of).unwrap_or(0),
        }));
    }
    json!({"path": dir.to_string_lossy(), "parent": dir.parent().map(|p| p.to_string_lossy().into_owned()), "entries": entries}).to_string()
}

/// mkdir / remove / rename on this computer; "" or an error message.
pub fn local_op(op: &str, path: &str, name: &str) -> String {
    let p = PathBuf::from(path);
    let r = match op {
        "mkdir" => std::fs::create_dir_all(&p),
        "remove" => match std::fs::symlink_metadata(&p) {
            Ok(m) if m.is_dir() => std::fs::remove_dir_all(&p),
            Ok(_) => std::fs::remove_file(&p),
            Err(e) => Err(e),
        },
        "rename" => match p.parent() {
            Some(dir) if dir.join(name).exists() => return format!("{} already exists", name),
            Some(dir) => std::fs::rename(&p, dir.join(name)),
            None => return "cannot rename this".into(),
        },
        _ => return "unknown operation".into(),
    };
    r.err().map(|e| e.to_string()).unwrap_or_default()
}

fn join_remote(dir: &str, rel: &str, sep: &str) -> String {
    let rel = if sep == "\\" { rel.replace('/', "\\") } else { rel.to_string() };
    if dir.ends_with('/') || dir.ends_with('\\') {
        format!("{}{}", dir, rel)
    } else {
        format!("{}{}{}", dir, sep, rel)
    }
}

// ---------------------------------------------------------------- transfers

fn push_jobs(jobs: Vec<Job>) {
    if jobs.is_empty() {
        return;
    }
    if !crate::share::files_enabled() {
        set_error("File transfer is turned off in Settings");
        return;
    }
    with_state(|s| s.jobs.extend(jobs));
    kick();
}

fn job(up: bool, name: String, local: PathBuf, remote: String, size: u64, mtime: u64) -> Job {
    Job {
        id: new_id(),
        up,
        name,
        local,
        remote,
        size,
        mtime,
        done: 0,
        state: JobState::Queued,
        message: String::new(),
        started: None,
        cancel: Arc::new(AtomicBool::new(false)),
    }
}

/// Files and folders of this computer -> the remote folder `remote_dir`.
pub fn upload(paths: Vec<PathBuf>, remote_dir: String) {
    let sep = CONN.lock().unwrap().as_ref().map(|c| c.sep()).or_else(|| {
        STATE.lock().unwrap().as_ref().and_then(|s| s.hello.get("sep").and_then(Value::as_str).map(str::to_string))
    });
    let sep = sep.unwrap_or_else(|| "/".into());
    let mut jobs = Vec::new();
    for p in paths {
        let Ok(meta) = std::fs::metadata(&p) else { continue };
        let base = p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "file".into());
        if meta.is_dir() {
            let mut stack = vec![(p.clone(), base.clone())];
            while let Some((dir, rel)) = stack.pop() {
                let Ok(read) = std::fs::read_dir(&dir) else { continue };
                for e in read.flatten() {
                    let name = e.file_name().to_string_lossy().into_owned();
                    let r = format!("{}/{}", rel, name);
                    let Ok(ft) = e.file_type() else { continue };
                    if ft.is_dir() {
                        stack.push((e.path(), r));
                    } else if let Ok(m) = std::fs::metadata(e.path()) {
                        if m.is_file() && !name.ends_with(PART) {
                            jobs.push(job(true, r.clone(), e.path(), join_remote(&remote_dir, &r, &sep), m.len(), mtime_of(&m)));
                        }
                    }
                }
            }
        } else if meta.is_file() {
            jobs.push(job(true, base.clone(), p.clone(), join_remote(&remote_dir, &base, &sep), meta.len(), mtime_of(&meta)));
        }
    }
    push_jobs(jobs);
}

/// Files dropped on the stream: into the remote Downloads/Lunaris.
pub fn upload_to_downloads(paths: Vec<PathBuf>) {
    let downloads = STATE
        .lock()
        .unwrap()
        .as_ref()
        .and_then(|s| s.hello.get("downloads").and_then(Value::as_str).map(str::to_string));
    match downloads {
        Some(d) => {
            let sep = CONN.lock().unwrap().as_ref().map(|c| c.sep()).unwrap_or_else(|| "/".into());
            upload(paths, join_remote(&d, "Lunaris", &sep));
        }
        None => {
            // Not connected yet: wait for hello, then queue.
            runtime().spawn(async move {
                for _ in 0..250 {
                    if CONN.lock().unwrap().is_some() {
                        upload_to_downloads(paths);
                        return;
                    }
                    tokio::time::sleep(Duration::from_millis(100)).await;
                }
            });
        }
    }
}

/// Remote entries (names in `remote_dir`) -> the local folder `local_dir`.
pub fn download(names: Vec<String>, remote_dir: String, local_dir: PathBuf) {
    runtime().spawn(async move {
        let c = match conn() {
            Ok(c) => c,
            Err(e) => return set_error(e),
        };
        let entries = STATE
            .lock()
            .unwrap()
            .as_ref()
            .and_then(|s| s.remote.get("entries").cloned())
            .unwrap_or(Value::Null);
        let mut jobs = Vec::new();
        for name in names {
            let info = entries.as_array().and_then(|a| a.iter().find(|e| e.get("name").and_then(Value::as_str) == Some(&name)).cloned());
            let is_dir = info.as_ref().and_then(|e| e.get("dir")).and_then(Value::as_bool).unwrap_or(false);
            let remote = join_remote(&remote_dir, &name, &c.sep());
            if is_dir {
                match c.request(json!({"type": "walk", "path": remote}), 120).await {
                    Ok(tree) => {
                        if let Some(ds) = tree.get("dirs").and_then(Value::as_array) {
                            for d in ds.iter().filter_map(Value::as_str) {
                                let _ = std::fs::create_dir_all(local_dir.join(&name).join(d));
                            }
                        }
                        let _ = std::fs::create_dir_all(local_dir.join(&name));
                        for f in tree.get("files").and_then(Value::as_array).cloned().unwrap_or_default() {
                            let rel = f.get("rel").and_then(Value::as_str).unwrap_or("").to_string();
                            let size = f.get("size").and_then(Value::as_u64).unwrap_or(0);
                            let mtime = f.get("mtime").and_then(Value::as_u64).unwrap_or(0);
                            let local = rel.split('/').fold(local_dir.join(&name), |p, part| p.join(part));
                            jobs.push(job(false, format!("{}/{}", name, rel), local, join_remote(&remote, &rel, &c.sep()), size, mtime));
                        }
                    }
                    Err(e) => set_error(e),
                }
            } else {
                let size = info.as_ref().and_then(|e| e.get("size")).and_then(Value::as_u64).unwrap_or(0);
                let mtime = info.as_ref().and_then(|e| e.get("mtime")).and_then(Value::as_u64).unwrap_or(0);
                jobs.push(job(false, name.clone(), local_dir.join(&name), remote, size, mtime));
            }
        }
        push_jobs(jobs);
    });
}

pub fn cancel(id: &str) {
    with_state(|s| {
        if let Some(j) = s.jobs.iter_mut().find(|j| j.id == id) {
            j.cancel.store(true, Ordering::SeqCst);
            if j.state == JobState::Queued {
                j.state = JobState::Cancelled;
            }
        }
    });
}

pub fn clear_finished() {
    with_state(|s| s.jobs.retain(|j| matches!(j.state, JobState::Queued | JobState::Running)));
}

/// Starts the worker unless it runs.
fn kick() {
    if CONN.lock().unwrap().is_none() || WORKER.swap(true, Ordering::SeqCst) {
        return;
    }
    runtime().spawn(async {
        loop {
            let next = with_state(|s| {
                s.jobs.iter_mut().find(|j| j.state == JobState::Queued).map(|j| {
                    j.state = JobState::Running;
                    j.started = Some(Instant::now());
                    (j.id.clone(), j.up, j.local.clone(), j.remote.clone(), j.size, j.mtime, j.cancel.clone())
                })
            });
            let Some((id, up, local, remote, size, mtime, cancel)) = next else { break };
            let result = match conn() {
                Ok(c) if up => upload_one(&c, &id, &local, &remote, size, mtime, &cancel).await,
                Ok(c) => download_one(&c, &id, &remote, &local, size, mtime, &cancel).await,
                Err(e) => Err(e),
            };
            let up_done = up;
            with_state(|s| {
                if let Some(j) = s.jobs.iter_mut().find(|j| j.id == id) {
                    match result {
                        Ok((state, message)) => {
                            j.state = state;
                            j.message = message;
                            if state == JobState::Done || state == JobState::Skipped {
                                j.done = j.size;
                            }
                        }
                        Err(e) => {
                            j.state = JobState::Failed;
                            j.message = e;
                        }
                    }
                }
            });
            let more_uploads = STATE.lock().unwrap().as_ref().map(|s| s.jobs.iter().any(|j| j.up && j.state == JobState::Queued)).unwrap_or(false);
            if up_done && !more_uploads {
                refresh_remote();
            }
        }
        WORKER.store(false, Ordering::SeqCst);
    });
}

fn progress(id: &str, done: u64) {
    static LAST: Mutex<Option<Instant>> = Mutex::new(None);
    let mut last = LAST.lock().unwrap();
    if last.map(|t| t.elapsed() < Duration::from_millis(150)).unwrap_or(false) {
        // Keep the number fresh without bumping the version too often.
        if let Some(s) = STATE.lock().unwrap().as_mut() {
            if let Some(j) = s.jobs.iter_mut().find(|j| j.id == id) {
                j.done = done;
            }
        }
        return;
    }
    *last = Some(Instant::now());
    drop(last);
    with_state(|s| {
        if let Some(j) = s.jobs.iter_mut().find(|j| j.id == id) {
            j.done = done;
        }
    });
}

fn set_remote_path(id: &str, path: &str) {
    with_state(|s| {
        if let Some(j) = s.jobs.iter_mut().find(|j| j.id == id) {
            j.remote = path.to_string();
        }
    });
}

async fn upload_one(c: &Conn, id: &str, local: &Path, remote: &str, size: u64, mtime: u64, cancel: &AtomicBool) -> Result<(JobState, String), String> {
    use tokio::io::{AsyncReadExt, AsyncSeekExt};
    let (_, mut rx) = c
        .start(json!({"type": "put", "id": id, "path": remote, "size": size, "mtime": mtime, "on_exists": "rename"}))
        .await?;
    let ready = tokio::time::timeout(Duration::from_secs(30), rx.recv())
        .await
        .map_err(|_| "no answer from the remote computer".to_string())?
        .ok_or("connection closed")?;
    match ready.get("type").and_then(Value::as_str) {
        Some("put_skip") => {
            c.waiting.lock().unwrap().remove(id);
            let why = if ready.get("reason").and_then(Value::as_str) == Some("identical") { "Already there (same file)" } else { "Already exists" };
            return Ok((JobState::Skipped, why.into()));
        }
        Some("put_ready") => {}
        _ => {
            c.waiting.lock().unwrap().remove(id);
            return Err(ready.get("message").and_then(Value::as_str).unwrap_or("refused").to_string());
        }
    }
    if let Some(p) = ready.get("path").and_then(Value::as_str) {
        set_remote_path(id, p);
    }
    let mut sent = ready.get("offset").and_then(Value::as_u64).unwrap_or(0);
    let mut file = tokio::fs::File::open(local).await.map_err(|e| format!("{}: {}", local.display(), e))?;
    if sent > 0 {
        file.seek(std::io::SeekFrom::Start(sent)).await.map_err(|e| e.to_string())?;
    }
    let mut buf = vec![0u8; CHUNK];
    loop {
        if cancel.load(Ordering::SeqCst) {
            let _ = c.ch.send_text(&json!({"type": "cancel", "id": id}).to_string()).await;
            c.waiting.lock().unwrap().remove(id);
            return Ok((JobState::Cancelled, String::new()));
        }
        let n = file.read(&mut buf).await.map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        c.drain().await?;
        c.ch.send(bytes::BytesMut::from(&buf[..n])).await.map_err(|e| e.to_string())?;
        sent += n as u64;
        progress(id, sent);
    }
    c.ch.send_text(&json!({"type": "put_end", "id": id}).to_string()).await.map_err(|e| e.to_string())?;
    let done = tokio::time::timeout(Duration::from_secs(600), rx.recv())
        .await
        .map_err(|_| "no answer from the remote computer".to_string())?
        .ok_or("connection closed")?;
    c.waiting.lock().unwrap().remove(id);
    match done.get("type").and_then(Value::as_str) {
        Some("done") => {
            if let Some(p) = done.get("path").and_then(Value::as_str) {
                set_remote_path(id, p);
            }
            Ok((JobState::Done, String::new()))
        }
        _ => Err(done.get("message").and_then(Value::as_str).unwrap_or("failed").to_string()),
    }
}

fn part_of(p: &Path) -> PathBuf {
    let mut s = p.as_os_str().to_owned();
    s.push(PART);
    PathBuf::from(s)
}

async fn download_one(c: &Conn, id: &str, remote: &str, local: &Path, size: u64, mtime: u64, cancel: &AtomicBool) -> Result<(JobState, String), String> {
    use tokio::io::AsyncWriteExt;
    // The same file is here already: nothing to do.
    if let Ok(m) = std::fs::metadata(local) {
        if m.is_file() && m.len() == size && mtime != 0 && mtime_of(&m).abs_diff(mtime) <= 2 {
            return Ok((JobState::Skipped, "Already there (same file)".into()));
        }
    }
    if let Some(dir) = local.parent() {
        tokio::fs::create_dir_all(dir).await.map_err(|e| format!("{}: {}", dir.display(), e))?;
    }
    // A different file with that name stays: the download gets a new name.
    let local = if local.exists() { free_name(local) } else { local.to_path_buf() };
    let part = part_of(&local);
    // Resume an unfinished download of the same size.
    let offset = std::fs::metadata(&part).map(|m| m.len()).ok().filter(|&l| l < size).unwrap_or(0);
    let mut file = if offset > 0 {
        tokio::fs::OpenOptions::new().append(true).open(&part).await
    } else {
        tokio::fs::File::create(&part).await
    }
    .map_err(|e| format!("{}: {}", part.display(), e))?;

    // Downloads run one at a time: every chunk until get_end is this one's
    // (the sink is set before asking, so the first chunks can't be missed).
    let (chunk_tx, mut chunk_rx) = mpsc::unbounded_channel::<Bytes>();
    *c.sink.lock().unwrap() = Some(chunk_tx);
    let (_, mut rx) = match c.start(json!({"type": "get", "id": id, "path": remote, "offset": offset})).await {
        Ok(x) => x,
        Err(e) => {
            *c.sink.lock().unwrap() = None;
            return Err(e);
        }
    };
    let mut done = offset;
    let mut cancel_sent = false;
    let end: Value = loop {
        tokio::select! {
            reply = rx.recv() => {
                let Some(r) = reply else {
                    *c.sink.lock().unwrap() = None;
                    return Err("connection closed".into());
                };
                if r.get("type").and_then(Value::as_str) != Some("get_start") {
                    // get_end or error: the chunks before it are queued already.
                    *c.sink.lock().unwrap() = None;
                    break r;
                }
            }
            Some(chunk) = chunk_rx.recv() => {
                if cancel.load(Ordering::SeqCst) {
                    if !cancel_sent {
                        cancel_sent = true;
                        let _ = c.ch.send_text(&json!({"type": "cancel", "id": id}).to_string()).await;
                    }
                    continue;
                }
                file.write_all(&chunk).await.map_err(|e| e.to_string())?;
                done += chunk.len() as u64;
                progress(id, done);
            }
        }
    };
    c.waiting.lock().unwrap().remove(id);
    // Chunks that arrived before get_end.
    while let Ok(chunk) = chunk_rx.try_recv() {
        if cancel_sent || cancel.load(Ordering::SeqCst) {
            break;
        }
        file.write_all(&chunk).await.map_err(|e| e.to_string())?;
        done += chunk.len() as u64;
    }
    file.flush().await.map_err(|e| e.to_string())?;
    if end.get("cancelled").and_then(Value::as_bool).unwrap_or(false) || cancel_sent {
        return Ok((JobState::Cancelled, String::new()));
    }
    if end.get("type").and_then(Value::as_str) != Some("get_end") {
        return Err(end.get("message").and_then(Value::as_str).unwrap_or("failed").to_string());
    }
    if done != size {
        return Err(format!("incomplete: {} of {} bytes", done, size));
    }
    let file = file.into_std().await;
    if mtime > 0 {
        let _ = file.set_modified(UNIX_EPOCH + Duration::from_secs(mtime));
    }
    drop(file);
    std::fs::rename(&part, &local).map_err(|e| format!("{}: {}", local.display(), e))?;
    Ok((JobState::Done, local.to_string_lossy().into_owned()))
}

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remote_paths_use_the_remote_separator() {
        assert_eq!(join_remote("C:\\Users\\a", "x/y.txt", "\\"), "C:\\Users\\a\\x\\y.txt");
        assert_eq!(join_remote("/home/a/", "y.txt", "/"), "/home/a/y.txt");
        assert_eq!(join_remote("/", "y", "/"), "/y");
    }

    #[test]
    fn local_listing_hides_parts() {
        let d = std::env::temp_dir().join(format!("lunaris-cfiles-{}", std::process::id()));
        std::fs::create_dir_all(d.join("sub")).unwrap();
        std::fs::write(d.join("a.txt"), b"1").unwrap();
        std::fs::write(d.join("b.txt.lunaris-part"), b"1").unwrap();
        let v: Value = serde_json::from_str(&list_local(&d.to_string_lossy(), false)).unwrap();
        let names: Vec<&str> = v["entries"].as_array().unwrap().iter().filter_map(|e| e["name"].as_str()).collect();
        assert!(names.contains(&"a.txt") && names.contains(&"sub") && !names.iter().any(|n| n.ends_with(PART)));
        let _ = std::fs::remove_dir_all(&d);
    }
}

/// Against a running server and agent:
/// LUNARIS_TEST_SERVER=http://127.0.0.1:8093 LUNARIS_TEST_TOKEN=... LUNARIS_TEST_HOST=<id>
/// LUNARIS_TEST_DIR=<scratch dir> cargo test -p client-qml files_e2e -- --ignored
#[cfg(test)]
mod e2e {
    use super::*;

    fn wait(what: &str, secs: u64, f: impl Fn(&Value) -> bool) -> Value {
        let t = Instant::now();
        loop {
            let v: Value = serde_json::from_str(&state_json()).unwrap();
            if f(&v) {
                return v;
            }
            assert!(t.elapsed() < Duration::from_secs(secs), "timeout waiting for {}: {}", what, v);
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    #[test]
    #[ignore]
    fn files_e2e() {
        let env = |k: &str| std::env::var(k).unwrap_or_else(|_| panic!("{} not set", k));
        let dir = PathBuf::from(env("LUNARIS_TEST_DIR"));
        let remote = dir.join("remote-c");
        let local = dir.join("local-c");
        let _ = std::fs::remove_dir_all(&remote);
        let _ = std::fs::remove_dir_all(&local);
        std::fs::create_dir_all(remote.join("folder/inner")).unwrap();
        std::fs::create_dir_all(&local).unwrap();
        std::fs::write(remote.join("folder/inner/x.txt"), b"inner file").unwrap();
        let big: Vec<u8> = (0..3_000_000u32).map(|i| (i.wrapping_mul(2654435761) >> 13) as u8).collect();
        std::fs::write(local.join("up.bin"), &big).unwrap();

        open(env("LUNARIS_TEST_SERVER"), env("LUNARIS_TEST_TOKEN"), env("LUNARIS_TEST_HOST"), "test".into());
        let t0 = Instant::now();
        wait("connected", 30, |v| v["connected"] == true);
        println!("connected in {:?}", t0.elapsed());

        let remote_s = remote.to_string_lossy().to_string();
        list_remote(remote_s.clone(), false);
        wait("listing", 10, |v| v["remote"]["path"] == remote_s.as_str());

        // Upload, then again (identical: skipped).
        upload(vec![local.join("up.bin")], remote_s.clone());
        wait("upload", 60, |v| v["active"] == 0 && v["jobs"].as_array().map(|a| a.len() == 1).unwrap_or(false));
        assert_eq!(std::fs::read(remote.join("up.bin")).unwrap(), big);
        upload(vec![local.join("up.bin")], remote_s.clone());
        let v = wait("second upload", 30, |v| v["active"] == 0 && v["jobs"].as_array().map(|a| a.len() == 2).unwrap_or(false));
        assert_eq!(v["jobs"][0]["state"], "skipped", "{}", v["jobs"][0]);

        // Download a file and a folder.
        list_remote(remote_s.clone(), false);
        wait("listing 2", 10, |v| v["remote"]["entries"].as_array().map(|a| a.len() == 2).unwrap_or(false));
        let dl = local.join("dl");
        std::fs::create_dir_all(&dl).unwrap();
        download(vec!["up.bin".into(), "folder".into()], remote_s.clone(), dl.clone());
        let v = wait("downloads", 60, |v| v["active"] == 0 && v["jobs"].as_array().map(|a| a.len() == 4).unwrap_or(false));
        for j in v["jobs"].as_array().unwrap() {
            assert!(j["state"] == "done" || j["state"] == "skipped", "{}", j);
        }
        assert_eq!(std::fs::read(dl.join("up.bin")).unwrap(), big);
        assert_eq!(std::fs::read(dl.join("folder/inner/x.txt")).unwrap(), b"inner file");
        let a = std::fs::metadata(remote.join("up.bin")).unwrap().modified().unwrap();
        let b = std::fs::metadata(dl.join("up.bin")).unwrap().modified().unwrap();
        assert!(a.duration_since(b).unwrap_or_default().as_secs() <= 2 && b.duration_since(a).unwrap_or_default().as_secs() <= 2);
        close();
        println!("ok in {:?}", t0.elapsed());
    }
}
