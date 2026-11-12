//! Computers added by address: instead of their agent connecting to this
//! server, the server connects to the agent (its direct-access port,
//! `ws://<address>/ws/server`, agent/src/direct.rs) and then treats that
//! connection exactly like an agent's own (`signaling::agent_connected`), so
//! they show in the computer list and work from every client.
//!
//! Added with the agent's access password, sessions start at once; without
//! it, the person at that computer accepts each one (RustDesk's "click").
//! The connection is kept up, retried every few seconds while unreachable.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use common::{AgentMessage, ServerToAgentMessage};
use futures_util::{SinkExt, StreamExt};
use tokio_tungstenite::tungstenite::{self, Message as WsMessage};
use tracing::{debug, info, warn};

use crate::signaling::SignalingState;

const RETRY: Duration = Duration::from_secs(5);
/// After a refused password: slower, so the agent's lockout isn't hit.
const RETRY_REFUSED: Duration = Duration::from_secs(60);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(8);
const PING_INTERVAL: Duration = Duration::from_secs(15);

/// One computer added by address.
pub struct Link {
    task: tokio::task::JoinHandle<()>,
    /// Why it isn't connected (None while connected or not tried yet).
    pub error: Arc<Mutex<Option<String>>>,
    /// The agent's id, once it has told it.
    pub host_id: Arc<Mutex<Option<String>>>,
    pub needs_approval: bool,
}

impl Drop for Link {
    fn drop(&mut self) {
        self.task.abort();
    }
}

#[derive(Default)]
pub struct Links {
    by_address: Mutex<HashMap<String, Link>>,
}

/// What the host list shows for one address.
pub struct LinkInfo {
    pub address: String,
    pub host_id: Option<String>,
    pub error: Option<String>,
    pub needs_approval: bool,
}

impl Links {
    pub fn list(&self) -> Vec<LinkInfo> {
        self.by_address
            .lock()
            .unwrap()
            .iter()
            .map(|(address, l)| LinkInfo {
                address: address.clone(),
                host_id: l.host_id.lock().unwrap().clone(),
                error: l.error.lock().unwrap().clone(),
                needs_approval: l.needs_approval,
            })
            .collect()
    }

    /// Stops a link; returns whether there was one.
    pub fn stop(&self, address: &str) -> bool {
        self.by_address.lock().unwrap().remove(address).is_some()
    }

    /// The address a host was added with, if any.
    pub fn address_of(&self, host_id: &str) -> Option<String> {
        self.by_address
            .lock()
            .unwrap()
            .iter()
            .find(|(_, l)| l.host_id.lock().unwrap().as_deref() == Some(host_id))
            .map(|(a, _)| a.clone())
    }
}

/// "host" or "host:port" (port 8091 when missing), without scheme or path;
/// None when it can't be an address.
pub fn normalize_address(text: &str) -> Option<String> {
    let mut addr = text.trim();
    for scheme in ["ws://", "wss://", "http://", "https://"] {
        if let Some(rest) = addr.strip_prefix(scheme) {
            addr = rest;
        }
    }
    let addr = addr.split('/').next().unwrap_or("").trim();
    if addr.is_empty() || addr.chars().any(|c| c.is_whitespace() || "?#@\"'<>\\".contains(c)) {
        return None;
    }
    let has_port = if addr.starts_with('[') { addr.contains("]:") } else { addr.contains(':') };
    let addr = if has_port { addr.to_string() } else { format!("{}:8091", addr) };
    let port_ok = addr.rsplit(':').next().and_then(|p| p.parse::<u16>().ok()).is_some_and(|p| p > 0);
    port_ok.then_some(addr)
}

/// Starts (or restarts, with a new password) the link to `address`.
pub fn start(state: &Arc<SignalingState>, address: String, password: String, host_id: Option<String>) {
    let error = Arc::new(Mutex::new(None));
    let known_id = Arc::new(Mutex::new(host_id));
    let task = tokio::spawn(run(state.clone(), address.clone(), password.clone(), error.clone(), known_id.clone()));
    let link = Link { task, error, host_id: known_id, needs_approval: password.trim().is_empty() };
    state.links.by_address.lock().unwrap().insert(address, link);
}

/// Starts the links saved in the database (server start).
pub async fn start_saved(state: &Arc<SignalingState>) {
    let rows: Vec<(String, String, Option<String>)> =
        sqlx::query_as("SELECT address, password, host_id FROM address_hosts")
            .fetch_all(&state.db)
            .await
            .unwrap_or_default();
    for (address, password, host_id) in rows {
        start(state, address, password, host_id);
    }
}

/// This server's name, as the person at the computer sees it.
fn server_name() -> String {
    let host = hostname::get().ok().and_then(|h| h.into_string().ok()).unwrap_or_default();
    format!("Lunaris server {}", host).trim().to_string()
}

async fn run(
    state: Arc<SignalingState>,
    address: String,
    password: String,
    error: Arc<Mutex<Option<String>>>,
    host_id: Arc<Mutex<Option<String>>>,
) {
    loop {
        let wait = match connect_once(&state, &address, &password, &error, &host_id).await {
            Ok(()) => {
                info!("Lost the connection to {} (added by address); reconnecting", address);
                RETRY
            }
            Err((message, refused)) => {
                let changed = error.lock().unwrap().as_deref() != Some(message.as_str());
                if changed {
                    warn!("Cannot connect to {} (added by address): {}", address, message);
                } else {
                    debug!("Cannot connect to {}: {}", address, message);
                }
                *error.lock().unwrap() = Some(message);
                if refused { RETRY_REFUSED } else { RETRY }
            }
        };
        tokio::time::sleep(wait).await;
    }
}

/// Connects, serves the agent until the connection ends. Err: a message for
/// the user, and whether the agent refused us (wrong password, locked out).
async fn connect_once(
    state: &Arc<SignalingState>,
    address: &str,
    password: &str,
    error: &Arc<Mutex<Option<String>>>,
    host_id: &Arc<Mutex<Option<String>>>,
) -> Result<(), (String, bool)> {
    let url = format!(
        "ws://{}/ws/server?token={}&name={}",
        address,
        urlencoding::encode(password.trim()),
        urlencoding::encode(&server_name())
    );
    let (ws, _) = match tokio::time::timeout(CONNECT_TIMEOUT, tokio_tungstenite::connect_async(url.as_str())).await {
        Err(_) => return Err(("not reachable (timed out)".into(), false)),
        Ok(Err(tungstenite::Error::Http(resp))) => {
            return Err(match resp.status().as_u16() {
                401 => ("wrong password".into(), true),
                429 => ("too many wrong passwords; retrying later".into(), true),
                404 => ("not a Lunaris agent, or one too old for this (update it)".into(), false),
                code => (format!("refused (HTTP {})", code), false),
            });
        }
        Ok(Err(e)) => return Err((format!("not reachable ({})", e), false)),
        Ok(Ok(c)) => c,
    };
    let local_ip = match ws.get_ref() {
        tokio_tungstenite::MaybeTlsStream::Plain(tcp) => tcp.local_addr().ok().map(|a| a.ip()),
        _ => None,
    };
    let (mut write, mut read) = ws.split();

    // The agent says who it is first (pings may come before).
    let first = tokio::time::timeout(CONNECT_TIMEOUT, async {
        loop {
            match read.next().await {
                Some(Ok(WsMessage::Text(text))) => return Some(text),
                Some(Ok(_)) => continue,
                _ => return None,
            }
        }
    })
    .await;
    let (id, name, codec_support) = match first {
        Ok(Some(text)) => match serde_json::from_str::<AgentMessage>(&text) {
            Ok(AgentMessage::Register { id, name, codec_support }) => (id, name, codec_support),
            _ => return Err(("unexpected answer (not a Lunaris agent?)".into(), false)),
        },
        _ => return Err(("no answer from the agent".into(), false)),
    };
    *error.lock().unwrap() = None;
    *host_id.lock().unwrap() = Some(id.clone());
    let _ = sqlx::query("UPDATE address_hosts SET host_id = ? WHERE address = ?")
        .bind(&id)
        .bind(address)
        .execute(&state.db)
        .await;
    if let Some(ip) = local_ip {
        state.agent_local_ips.write().unwrap().insert(id.clone(), ip);
    }
    info!("Connected to {} ({}) added by address {}", name, id, address);

    let (conn_id, mut rx) = crate::signaling::agent_connected(state, &id, &name, codec_support).await;
    // Also when the link is stopped (removed, new password) mid-way.
    let _registered = Registered { state: state.clone(), id: id.clone(), conn_id };
    let writer = tokio::spawn(async move {
        let mut ping = tokio::time::interval(PING_INTERVAL);
        ping.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tokio::select! {
                msg = rx.recv() => {
                    let Some(msg): Option<ServerToAgentMessage> = msg else { break };
                    let Ok(json) = serde_json::to_string(&msg) else { continue };
                    if write.send(WsMessage::Text(json)).await.is_err() {
                        break;
                    }
                }
                _ = ping.tick() => {
                    if write.send(WsMessage::Ping(Vec::new())).await.is_err() {
                        break;
                    }
                }
            }
        }
        let _ = write.close().await;
    });
    while let Some(frame) = read.next().await {
        match frame {
            Ok(WsMessage::Text(text)) => match serde_json::from_str::<AgentMessage>(&text) {
                Ok(msg) => crate::signaling::agent_message(state, &id, msg).await,
                Err(e) => warn!("Bad message from {}: {}", address, e),
            },
            Ok(WsMessage::Close(_)) => break,
            Ok(_) => {}
            Err(e) => {
                debug!("Connection to {} failed: {}", address, e);
                break;
            }
        }
    }
    writer.abort();
    Ok(())
}

/// Takes the host offline when its connection ends, however it ends.
struct Registered {
    state: Arc<SignalingState>,
    id: String,
    conn_id: uuid::Uuid,
}

impl Drop for Registered {
    fn drop(&mut self) {
        let (state, id, conn_id) = (self.state.clone(), self.id.clone(), self.conn_id);
        tokio::spawn(async move { crate::signaling::agent_disconnected(&state, &id, conn_id).await });
    }
}

#[cfg(test)]
mod tests {
    use super::normalize_address;

    #[test]
    fn addresses() {
        assert_eq!(normalize_address("192.168.1.20").as_deref(), Some("192.168.1.20:8091"));
        assert_eq!(normalize_address(" ws://pc.lan:9000/ws ").as_deref(), Some("pc.lan:9000"));
        assert_eq!(normalize_address("[fe80::1]:8091").as_deref(), Some("[fe80::1]:8091"));
        assert_eq!(normalize_address("pc:0"), None);
        assert_eq!(normalize_address("pc:x"), None);
        assert_eq!(normalize_address(""), None);
        assert_eq!(normalize_address("a b"), None);
        assert_eq!(normalize_address("evil@host"), None);
    }
}
