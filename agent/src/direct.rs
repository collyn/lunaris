//! Direct connections: clients connect to the agent itself, without a Lunaris
//! server, at `ws://<agent address>:<direct_port>/ws/client?token=<password>`.
//!
//! The endpoint speaks the server's client protocol (`ClientMessage` in,
//! `ServerToClientMessage` out), so the clients only change the address they
//! connect to. Only signaling goes through this socket; video, audio and input
//! still use WebRTC, which now finds the agent at the address the client
//! already knows. Meant for LAN, VPN or a forwarded port: the socket is not
//! TLS, so the password and SDP travel in clear text (media stays encrypted
//! by DTLS-SRTP).
//!
//! Without the password (`approve_mode` "password-click" or "click", like
//! RustDesk), a client can still connect, and the person at this computer is
//! asked to accept it when it wants to see the screen or transfer files
//! (`approval.rs`); the client hears `AccessRequest` "waiting", then the
//! answer.
//!
//! A Lunaris server that has this computer added by address connects to
//! `/ws/server` (same password rules) and is spoken to like the server the
//! agent itself connects to (`AgentMessage` out, `ServerToAgentMessage` in).

use std::collections::{HashMap, HashSet};
use std::net::{IpAddr, SocketAddr};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use common::{AgentMessage, ClientMessage, ServerToAgentMessage, ServerToClientMessage, SignalingMessage};
use futures_util::{SinkExt, StreamExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::handshake::server::{ErrorResponse, Request, Response};
use tokio_tungstenite::tungstenite::http::StatusCode;
use tokio_tungstenite::tungstenite::Message as WsMessage;
use tracing::{debug, error, info, warn};

use crate::approval::{self, ApproveMode, Decision, Kind};
use crate::hub::{from_direct_client, target_of, Hub};

/// Failed logins allowed per address before it is locked out for a while.
const MAX_FAILED_LOGINS: u32 = 5;
const LOCKOUT: Duration = Duration::from_secs(60);
const PING_INTERVAL: Duration = Duration::from_secs(15);
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);

/// Wrong-password attempts per client address: (count, time of the last one).
#[derive(Default)]
struct LoginGuard {
    failures: HashMap<IpAddr, (u32, Instant)>,
}

impl LoginGuard {
    fn locked_out(&mut self, ip: IpAddr) -> bool {
        match self.failures.get(&ip) {
            Some((count, last)) if last.elapsed() < LOCKOUT => *count >= MAX_FAILED_LOGINS,
            Some(_) => {
                self.failures.remove(&ip);
                false
            }
            None => false,
        }
    }

    fn failed(&mut self, ip: IpAddr) {
        let entry = self.failures.entry(ip).or_insert((0, Instant::now()));
        if entry.1.elapsed() >= LOCKOUT {
            entry.0 = 0;
        }
        entry.0 += 1;
        entry.1 = Instant::now();
    }

    fn succeeded(&mut self, ip: IpAddr) {
        self.failures.remove(&ip);
    }
}

/// Compares in time independent of where the strings differ.
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

fn query_param(query: &str, name: &str) -> Option<String> {
    query.split('&').find_map(|pair| {
        let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
        (key == name).then(|| {
            urlencoding::decode(&value.replace('+', " "))
                .map(|v| v.into_owned())
                .unwrap_or_default()
        })
    })
}

#[derive(Debug, PartialEq)]
enum Admission {
    /// `trusted`: the right password (no need to ask the person here);
    /// `link`: a Lunaris server connecting to this agent (`/ws/server`).
    Accept { trusted: bool, link: bool },
    NotFound,
    Unauthorized,
    TooManyAttempts,
}

fn admit(
    path: &str,
    query: &str,
    password: &str,
    mode: ApproveMode,
    ip: IpAddr,
    guard: &std::sync::Mutex<LoginGuard>,
) -> Admission {
    let link = match path.trim_end_matches('/') {
        "/ws/client" => false,
        "/ws/server" => true,
        _ => return Admission::NotFound,
    };
    let mut guard = guard.lock().unwrap();
    if guard.locked_out(ip) {
        return Admission::TooManyAttempts;
    }
    let token = query_param(query, "token").unwrap_or_default();
    let token = token.trim();
    let right = !password.trim().is_empty() && constant_time_eq(token.as_bytes(), password.trim().as_bytes());
    match mode {
        // Everyone is asked; the password is not used.
        ApproveMode::Click => Admission::Accept { trusted: false, link },
        _ if right => {
            guard.succeeded(ip);
            Admission::Accept { trusted: true, link }
        }
        // No password given: the person here may accept.
        ApproveMode::PasswordClick if token.is_empty() => Admission::Accept { trusted: false, link },
        _ => {
            guard.failed(ip);
            Admission::Unauthorized
        }
    }
}

/// What the person here is asked about, for a request; None for messages
/// that need no permission.
fn approval_kind(sig: &SignalingMessage) -> Option<Kind> {
    match sig {
        SignalingMessage::RequestSession { .. } | SignalingMessage::IncomingSession { .. } => Some(Kind::Control),
        SignalingMessage::RequestFileSession { .. } | SignalingMessage::IncomingFileSession { .. } => Some(Kind::Files),
        _ => None,
    }
}

/// Messages anyone connected may send before being accepted: questions about
/// the computer, nothing that shows or changes it.
fn allowed_before_approval(sig: &SignalingMessage) -> bool {
    matches!(sig, SignalingMessage::GetCapabilities { .. } | SignalingMessage::GetAppList { .. })
}

/// This computer's IPv4 addresses a direct client could use (LAN, VPN),
/// without loopback and link-local ones, sorted.
pub async fn local_addresses() -> Vec<String> {
    let net = webrtc_util::vnet::net::Net::new(None);
    let mut addrs: Vec<String> = net
        .get_interfaces()
        .await
        .iter()
        // Container/VM bridges and tunnels to them are no use to a client.
        .filter(|iface| {
            let name = iface.name();
            !["docker", "br-", "veth", "virbr", "lxcbr", "lxdbr", "podman", "cni", "vmnet", "vboxnet"]
                .iter()
                .any(|prefix| name.starts_with(prefix))
        })
        .flat_map(|iface| iface.addrs().iter().map(|net| net.addr()).collect::<Vec<_>>())
        .filter_map(|addr| match addr {
            IpAddr::V4(v4) if !v4.is_loopback() && !v4.is_link_local() && !v4.is_unspecified() => {
                Some(v4.to_string())
            }
            _ => None,
        })
        .collect();
    addrs.sort();
    addrs.dedup();
    addrs
}

/// Settings of the direct-connection listener.
#[derive(Clone)]
pub struct DirectConfig {
    pub port: u16,
    pub password: String,
    pub approve_mode: ApproveMode,
    /// This computer's name, told to servers that connect (`/ws/server`).
    pub name: String,
}

/// Accepts direct clients on `port` until the task is aborted.
pub async fn run_listener(
    config: DirectConfig,
    hub: Arc<Hub>,
    inbound_tx: mpsc::UnboundedSender<SignalingMessage>,
) -> anyhow::Result<()> {
    let listener = TcpListener::bind(("0.0.0.0", config.port)).await?;
    info!(
        "Direct connections enabled: clients can connect to ws://<this computer's address>:{} ({:?})",
        config.port, config.approve_mode
    );
    let guard = Arc::new(std::sync::Mutex::new(LoginGuard::default()));
    let config = Arc::new(config);
    loop {
        let (stream, peer) = match listener.accept().await {
            Ok(conn) => conn,
            Err(e) => {
                warn!("Direct connection accept failed: {}", e);
                tokio::time::sleep(Duration::from_millis(200)).await;
                continue;
            }
        };
        let _ = stream.set_nodelay(true);
        let hub = hub.clone();
        let inbound_tx = inbound_tx.clone();
        let config = config.clone();
        let guard = guard.clone();
        tokio::spawn(async move {
            if let Err(e) = handle_connection(stream, peer, config, guard, hub, inbound_tx).await {
                debug!("Direct connection from {} ended: {}", peer, e);
            }
        });
    }
}

/// Writes queued messages to the socket, with pings, until the queue closes.
fn spawn_writer<T: serde::Serialize + Send + 'static>(
    mut ws_write: futures_util::stream::SplitSink<tokio_tungstenite::WebSocketStream<TcpStream>, WsMessage>,
    mut rx: mpsc::UnboundedReceiver<T>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        // First ping after a while: the first message must be the peer's.
        let mut ping = tokio::time::interval_at(tokio::time::Instant::now() + PING_INTERVAL, PING_INTERVAL);
        ping.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tokio::select! {
                msg = rx.recv() => {
                    let Some(msg) = msg else { break };
                    match serde_json::to_string(&msg) {
                        Ok(json) => {
                            if ws_write.send(WsMessage::Text(json.into())).await.is_err() {
                                break;
                            }
                        }
                        Err(e) => error!("Failed to serialize a direct message: {}", e),
                    }
                }
                _ = ping.tick() => {
                    if ws_write.send(WsMessage::Ping(Default::default())).await.is_err() {
                        break;
                    }
                }
            }
        }
        let _ = ws_write.close().await;
    })
}

async fn handle_connection(
    stream: TcpStream,
    peer: SocketAddr,
    config: Arc<DirectConfig>,
    guard: Arc<std::sync::Mutex<LoginGuard>>,
    hub: Arc<Hub>,
    inbound_tx: mpsc::UnboundedSender<SignalingMessage>,
) -> anyhow::Result<()> {
    let ip = peer.ip();
    let mut admission = Admission::NotFound;
    let mut peer_name = String::new();
    let callback = |request: &Request, response: Response| -> Result<Response, ErrorResponse> {
        let uri = request.uri();
        let query = uri.query().unwrap_or("");
        admission = admit(uri.path(), query, &config.password, config.approve_mode, ip, &guard);
        peer_name = query_param(query, "name").unwrap_or_default().chars().take(64).collect();
        let status = match admission {
            Admission::Accept { .. } => return Ok(response),
            Admission::NotFound => StatusCode::NOT_FOUND,
            Admission::Unauthorized => StatusCode::UNAUTHORIZED,
            Admission::TooManyAttempts => StatusCode::TOO_MANY_REQUESTS,
        };
        let mut error = ErrorResponse::new(Some(status.canonical_reason().unwrap_or("").to_string()));
        *error.status_mut() = status;
        Err(error)
    };
    // A connection that never completes the handshake (plain HTTP, a stalled
    // client) must not hold a socket forever.
    let handshake = match tokio::time::timeout(
        HANDSHAKE_TIMEOUT,
        tokio_tungstenite::accept_hdr_async(stream, callback),
    )
    .await
    {
        Ok(result) => result,
        Err(_) => {
            debug!("Direct connection from {} timed out during the handshake", ip);
            return Ok(());
        }
    };
    let ws = match handshake {
        Ok(ws) => ws,
        Err(e) => {
            match admission {
                Admission::Unauthorized => warn!("Direct connection from {} rejected: wrong password", ip),
                Admission::TooManyAttempts => warn!("Direct connection from {} rejected: too many wrong passwords, try again later", ip),
                _ => debug!("Direct connection handshake from {} failed: {}", ip, e),
            }
            return Ok(());
        }
    };
    let Admission::Accept { trusted, link } = admission else { return Ok(()) };
    let requester = if peer_name.trim().is_empty() {
        ip.to_string()
    } else {
        format!("{} ({})", peer_name.trim(), ip)
    };
    let ctx = Conn { ip, requester, trusted, guard, hub, inbound_tx };
    if link {
        serve_link(ws, config, ctx).await;
    } else {
        serve_client(ws, ctx).await;
    }
    Ok(())
}

/// One accepted connection.
struct Conn {
    ip: IpAddr,
    /// Who connected, as shown to the person here.
    requester: String,
    /// Came with the right password: nobody needs to be asked.
    trusted: bool,
    guard: Arc<std::sync::Mutex<LoginGuard>>,
    hub: Arc<Hub>,
    inbound_tx: mpsc::UnboundedSender<SignalingMessage>,
}

impl Conn {
    /// Asks the person here; a refusal counts like a wrong password, so a
    /// stranger can't keep popping questions up.
    async fn ask(&self, requester: &str, kind: Kind) -> Decision {
        let decision = approval::ask(requester, kind).await;
        if matches!(decision, Decision::Denied) {
            self.guard.lock().unwrap().failed(self.ip);
        }
        decision
    }
}

/// A client (app, browser) connected straight to this agent.
async fn serve_client(ws: tokio_tungstenite::WebSocketStream<TcpStream>, conn: Conn) {
    let conn_id = format!("direct-{}", uuid::Uuid::new_v4());
    info!("Direct client connected from {} ({}, {})", conn.requester, conn_id,
        if conn.trusted { "password" } else { "to be accepted" });
    let (ws_write, mut ws_read) = ws.split();
    let (tx, rx) = mpsc::unbounded_channel::<ServerToClientMessage>();
    conn.hub.add_direct_client(conn_id.clone(), tx.clone());
    let writer = spawn_writer(ws_write, rx);
    let conn = Arc::new(conn);
    let accepted = Arc::new(AtomicBool::new(conn.trusted));
    let asking = Arc::new(AtomicBool::new(false));
    let agent_id = conn.hub.agent_id().to_string();

    while let Some(frame) = ws_read.next().await {
        let text = match frame {
            Ok(WsMessage::Text(text)) => text,
            Ok(WsMessage::Close(_)) => break,
            Ok(_) => continue,
            Err(e) => {
                debug!("Direct client {} read error: {}", conn_id, e);
                break;
            }
        };
        let ClientMessage::Signaling(sig) = match serde_json::from_str::<ClientMessage>(&text) {
            Ok(msg) => msg,
            Err(e) => {
                warn!("Ignoring malformed message from direct client {}: {}", conn_id, e);
                continue;
            }
        };
        let Some(sig) = from_direct_client(sig, &conn_id) else { continue };
        if accepted.load(Ordering::SeqCst) || allowed_before_approval(&sig) {
            if conn.inbound_tx.send(sig).is_err() {
                break;
            }
            continue;
        }
        let Some(kind) = approval_kind(&sig) else {
            debug!("Direct client {} not accepted yet: dropping {:?}", conn_id, sig);
            continue;
        };
        if asking.swap(true, Ordering::SeqCst) {
            continue;
        }
        let status = |s: &str| ServerToClientMessage::Signaling(SignalingMessage::AccessRequest {
            target_id: agent_id.clone(),
            status: s.to_string(),
        });
        let _ = tx.send(status("waiting"));
        let (conn, accepted, asking, tx) = (conn.clone(), accepted.clone(), asking.clone(), tx.clone());
        let agent_id = agent_id.clone();
        tokio::spawn(async move {
            let decision = conn.ask(&conn.requester, kind).await;
            asking.store(false, Ordering::SeqCst);
            let _ = tx.send(ServerToClientMessage::Signaling(SignalingMessage::AccessRequest {
                target_id: agent_id,
                status: decision.status().to_string(),
            }));
            if decision == Decision::Accepted {
                // Accepted for as long as this connection lasts.
                accepted.store(true, Ordering::SeqCst);
                let _ = conn.inbound_tx.send(sig);
            }
        });
    }

    info!("Direct client {} disconnected", conn_id);
    conn.hub.remove_direct_client(&conn_id);
    // Like the server on a client disconnect: end that client's session (the
    // session logic ignores this when the client had none).
    let _ = conn.inbound_tx.send(SignalingMessage::EndSession { target_id: conn_id });
    writer.abort();
}

/// A Lunaris server that connected to this agent (the computer was added to
/// it by address): spoken to exactly like the server the agent connects to,
/// so its clients reach this agent through it. Without the password, each
/// client connection that wants to see the screen or transfer files is
/// accepted by the person here.
async fn serve_link(ws: tokio_tungstenite::WebSocketStream<TcpStream>, config: Arc<DirectConfig>, conn: Conn) {
    let link_id = format!("link-{}", uuid::Uuid::new_v4());
    info!("Lunaris server connected from {} ({}, {})", conn.ip, link_id,
        if conn.trusted { "password" } else { "connections to be accepted" });
    let (ws_write, mut ws_read) = ws.split();
    let (tx, rx) = mpsc::unbounded_channel::<AgentMessage>();
    conn.hub.add_link(link_id.clone(), tx.clone());
    let writer = spawn_writer(ws_write, rx);
    let codec_support = tokio::task::spawn_blocking(crate::codec_support_bits).await.ok();
    let _ = tx.send(AgentMessage::Register {
        id: conn.hub.agent_id().to_string(),
        name: config.name.clone(),
        codec_support,
    });
    let conn = Arc::new(conn);
    // Client connections (on that server) the person here accepted.
    let accepted: Arc<std::sync::Mutex<HashSet<String>>> = Arc::default();
    let asking: Arc<std::sync::Mutex<HashSet<String>>> = Arc::default();

    while let Some(frame) = ws_read.next().await {
        let text = match frame {
            Ok(WsMessage::Text(text)) => text,
            Ok(WsMessage::Close(_)) => break,
            Ok(_) => continue,
            Err(e) => {
                debug!("Server link {} read error: {}", link_id, e);
                break;
            }
        };
        let sig = match serde_json::from_str::<ServerToAgentMessage>(&text) {
            Ok(ServerToAgentMessage::Signaling(sig)) => sig,
            Ok(ServerToAgentMessage::Registered { .. }) => continue,
            Err(e) => {
                warn!("Ignoring malformed message from server link {}: {}", link_id, e);
                continue;
            }
        };
        let Some(client) = target_of(&sig).map(str::to_owned) else { continue };
        conn.hub.link_client(&client, &link_id);
        if conn.trusted || allowed_before_approval(&sig) || accepted.lock().unwrap().contains(&client) {
            if conn.inbound_tx.send(sig).is_err() {
                break;
            }
            continue;
        }
        let Some(kind) = approval_kind(&sig) else {
            debug!("Server client {} not accepted yet: dropping {:?}", client, sig);
            continue;
        };
        if !asking.lock().unwrap().insert(client.clone()) {
            continue;
        }
        let requester = match &sig {
            SignalingMessage::IncomingSession { requester: Some(r), .. }
            | SignalingMessage::IncomingFileSession { requester: Some(r), .. } if !r.trim().is_empty() => {
                format!("{} (via {})", r.trim(), conn.ip)
            }
            _ => conn.requester.clone(),
        };
        let status = |s: &str| AgentMessage::Signaling(SignalingMessage::AccessRequest {
            target_id: client.clone(),
            status: s.to_string(),
        });
        let _ = tx.send(status("waiting"));
        let (conn, accepted, asking, tx) = (conn.clone(), accepted.clone(), asking.clone(), tx.clone());
        tokio::spawn(async move {
            let decision = conn.ask(&requester, kind).await;
            asking.lock().unwrap().remove(&client);
            let _ = tx.send(AgentMessage::Signaling(SignalingMessage::AccessRequest {
                target_id: client.clone(),
                status: decision.status().to_string(),
            }));
            if decision == Decision::Accepted {
                accepted.lock().unwrap().insert(client);
                let _ = conn.inbound_tx.send(sig);
            }
        });
    }

    info!("Server link {} closed", link_id);
    for client in conn.hub.remove_link(&link_id) {
        let _ = conn.inbound_tx.send(SignalingMessage::EndSession { target_id: client });
    }
    writer.abort();
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ip() -> IpAddr {
        "192.168.1.20".parse().unwrap()
    }

    const PW: ApproveMode = ApproveMode::Password;
    const BOTH: ApproveMode = ApproveMode::PasswordClick;
    const CLICK: ApproveMode = ApproveMode::Click;
    const TRUSTED: Admission = Admission::Accept { trusted: true, link: false };
    const ASK: Admission = Admission::Accept { trusted: false, link: false };

    #[test]
    fn admits_only_the_right_password_on_the_client_path() {
        let guard = std::sync::Mutex::new(LoginGuard::default());
        assert_eq!(admit("/ws/client", "token=abcd-efgh", "abcd-efgh", PW, ip(), &guard), TRUSTED);
        assert_eq!(admit("/ws/client", "token=abcd%2Defgh", "abcd-efgh", PW, ip(), &guard), TRUSTED);
        assert_eq!(admit("/ws/client", "token=wrong", "abcd-efgh", PW, ip(), &guard), Admission::Unauthorized);
        assert_eq!(admit("/ws/client", "", "abcd-efgh", PW, ip(), &guard), Admission::Unauthorized);
        assert_eq!(admit("/ws/agent", "token=abcd-efgh", "abcd-efgh", PW, ip(), &guard), Admission::NotFound);
        // An empty password never admits anyone.
        assert_eq!(admit("/ws/client", "token=", "", PW, ip(), &guard), Admission::Unauthorized);
    }

    #[test]
    fn without_the_password_the_person_here_is_asked() {
        let guard = std::sync::Mutex::new(LoginGuard::default());
        assert_eq!(admit("/ws/client", "token=pw", "pw", BOTH, ip(), &guard), TRUSTED);
        assert_eq!(admit("/ws/client", "", "pw", BOTH, ip(), &guard), ASK);
        assert_eq!(admit("/ws/client", "token=", "pw", BOTH, ip(), &guard), ASK);
        // A wrong password is still refused (it isn't a request to be asked).
        assert_eq!(admit("/ws/client", "token=nope", "pw", BOTH, ip(), &guard), Admission::Unauthorized);
        // Click: everyone is asked, even with the password.
        assert_eq!(admit("/ws/client", "token=pw", "pw", CLICK, ip(), &guard), ASK);
        // Servers connect on their own path, with the same rules.
        assert_eq!(admit("/ws/server", "token=pw", "pw", BOTH, ip(), &guard), Admission::Accept { trusted: true, link: true });
        assert_eq!(admit("/ws/server", "", "pw", BOTH, ip(), &guard), Admission::Accept { trusted: false, link: true });
        assert_eq!(admit("/ws/server", "", "pw", PW, ip(), &guard), Admission::Unauthorized);
    }

    #[test]
    fn only_requests_need_approval() {
        let req = SignalingMessage::RequestFileSession { host_id: "h".into() };
        assert_eq!(approval_kind(&req), Some(Kind::Files));
        assert!(!allowed_before_approval(&req));
        let caps = SignalingMessage::GetCapabilities { target_id: "h".into() };
        assert_eq!(approval_kind(&caps), None);
        assert!(allowed_before_approval(&caps));
        let stop = SignalingMessage::StopActiveStream { target_id: "h".into() };
        assert!(approval_kind(&stop).is_none() && !allowed_before_approval(&stop));
    }

    #[test]
    fn locks_out_after_repeated_wrong_passwords() {
        let guard = std::sync::Mutex::new(LoginGuard::default());
        for _ in 0..MAX_FAILED_LOGINS {
            assert_eq!(admit("/ws/client", "token=x", "secret", PW, ip(), &guard), Admission::Unauthorized);
        }
        // Even the right password is refused during the lockout...
        assert_eq!(admit("/ws/client", "token=secret", "secret", PW, ip(), &guard), Admission::TooManyAttempts);
        // ...but other addresses are not affected.
        let other: IpAddr = "192.168.1.21".parse().unwrap();
        assert_eq!(admit("/ws/client", "token=secret", "secret", PW, other, &guard), TRUSTED);
    }
}
