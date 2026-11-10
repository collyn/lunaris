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

use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use std::time::{Duration, Instant};

use common::{ClientMessage, ServerToClientMessage, SignalingMessage};
use futures_util::{SinkExt, StreamExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::handshake::server::{ErrorResponse, Request, Response};
use tokio_tungstenite::tungstenite::http::StatusCode;
use tokio_tungstenite::tungstenite::Message as WsMessage;
use tracing::{debug, error, info, warn};

use crate::hub::{from_direct_client, Hub};

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
    Accept,
    NotFound,
    Unauthorized,
    TooManyAttempts,
}

fn admit(path: &str, query: &str, password: &str, ip: IpAddr, guard: &std::sync::Mutex<LoginGuard>) -> Admission {
    if path.trim_end_matches('/') != "/ws/client" {
        return Admission::NotFound;
    }
    let mut guard = guard.lock().unwrap();
    if guard.locked_out(ip) {
        return Admission::TooManyAttempts;
    }
    let token = query_param(query, "token").unwrap_or_default();
    if !password.is_empty() && constant_time_eq(token.trim().as_bytes(), password.trim().as_bytes()) {
        guard.succeeded(ip);
        Admission::Accept
    } else {
        guard.failed(ip);
        Admission::Unauthorized
    }
}

/// This computer's IPv4 addresses a direct client could use (LAN, VPN),
/// without loopback and link-local ones, sorted.
pub async fn local_addresses() -> Vec<String> {
    let net = webrtc::util::vnet::net::Net::new(None);
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

/// Accepts direct clients on `port` until the task is aborted.
pub async fn run_listener(
    port: u16,
    password: String,
    hub: Arc<Hub>,
    inbound_tx: mpsc::UnboundedSender<SignalingMessage>,
) -> anyhow::Result<()> {
    let listener = TcpListener::bind(("0.0.0.0", port)).await?;
    info!(
        "Direct connections enabled: clients can connect to ws://<this computer's address>:{}",
        port
    );
    let guard = Arc::new(std::sync::Mutex::new(LoginGuard::default()));
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
        let password = password.clone();
        let guard = guard.clone();
        tokio::spawn(async move {
            if let Err(e) = handle_connection(stream, peer, password, guard, hub, inbound_tx).await {
                debug!("Direct connection from {} ended: {}", peer, e);
            }
        });
    }
}

async fn handle_connection(
    stream: TcpStream,
    peer: SocketAddr,
    password: String,
    guard: Arc<std::sync::Mutex<LoginGuard>>,
    hub: Arc<Hub>,
    inbound_tx: mpsc::UnboundedSender<SignalingMessage>,
) -> anyhow::Result<()> {
    let ip = peer.ip();
    let mut admission = Admission::NotFound;
    let callback = |request: &Request, response: Response| -> Result<Response, ErrorResponse> {
        let uri = request.uri();
        admission = admit(uri.path(), uri.query().unwrap_or(""), &password, ip, &guard);
        let status = match admission {
            Admission::Accept => return Ok(response),
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

    let conn_id = format!("direct-{}", uuid::Uuid::new_v4());
    info!("Direct client connected from {} ({})", peer, conn_id);
    let (mut ws_write, mut ws_read) = ws.split();
    let (tx, mut rx) = mpsc::unbounded_channel::<ServerToClientMessage>();
    hub.add_direct_client(conn_id.clone(), tx);

    let writer = tokio::spawn(async move {
        let mut ping = tokio::time::interval(PING_INTERVAL);
        ping.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tokio::select! {
                msg = rx.recv() => {
                    let Some(msg) = msg else { break };
                    match serde_json::to_string(&msg) {
                        Ok(json) => {
                            if ws_write.send(WsMessage::Text(json)).await.is_err() {
                                break;
                            }
                        }
                        Err(e) => error!("Failed to serialize message for a direct client: {}", e),
                    }
                }
                _ = ping.tick() => {
                    if ws_write.send(WsMessage::Ping(Vec::new())).await.is_err() {
                        break;
                    }
                }
            }
        }
        let _ = ws_write.close().await;
    });

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
        if let Some(sig) = from_direct_client(sig, &conn_id) {
            if inbound_tx.send(sig).is_err() {
                break;
            }
        }
    }

    info!("Direct client {} disconnected", conn_id);
    hub.remove_direct_client(&conn_id);
    // Like the server on a client disconnect: end that client's session (the
    // session logic ignores this when the client had none).
    let _ = inbound_tx.send(SignalingMessage::EndSession { target_id: conn_id });
    writer.abort();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ip() -> IpAddr {
        "192.168.1.20".parse().unwrap()
    }

    #[test]
    fn admits_only_the_right_password_on_the_client_path() {
        let guard = std::sync::Mutex::new(LoginGuard::default());
        assert_eq!(admit("/ws/client", "token=abcd-efgh", "abcd-efgh", ip(), &guard), Admission::Accept);
        assert_eq!(admit("/ws/client", "token=abcd%2Defgh", "abcd-efgh", ip(), &guard), Admission::Accept);
        assert_eq!(admit("/ws/client", "token=wrong", "abcd-efgh", ip(), &guard), Admission::Unauthorized);
        assert_eq!(admit("/ws/client", "", "abcd-efgh", ip(), &guard), Admission::Unauthorized);
        assert_eq!(admit("/ws/agent", "token=abcd-efgh", "abcd-efgh", ip(), &guard), Admission::NotFound);
        // An empty password never admits anyone.
        assert_eq!(admit("/ws/client", "token=", "", ip(), &guard), Admission::Unauthorized);
    }

    #[test]
    fn locks_out_after_repeated_wrong_passwords() {
        let guard = std::sync::Mutex::new(LoginGuard::default());
        for _ in 0..MAX_FAILED_LOGINS {
            assert_eq!(admit("/ws/client", "token=x", "secret", ip(), &guard), Admission::Unauthorized);
        }
        // Even the right password is refused during the lockout...
        assert_eq!(admit("/ws/client", "token=secret", "secret", ip(), &guard), Admission::TooManyAttempts);
        // ...but other addresses are not affected.
        let other: IpAddr = "192.168.1.21".parse().unwrap();
        assert_eq!(admit("/ws/client", "token=secret", "secret", other, &guard), Admission::Accept);
    }
}
