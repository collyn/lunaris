//! Computers added by address: instead of their agent connecting to this
//! server, the server connects to the agent (its direct-access port,
//! `wss://<address>/ws/server`, agent/src/direct.rs) and then treats that
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
use tokio_tungstenite::tungstenite::Message as WsMessage;
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
    let link = spawn(state, address.clone(), password, host_id, ADDED_BY_ADDRESS);
    state.links.by_address.lock().unwrap().insert(address, link);
}

/// How the log names a computer added by address.
const ADDED_BY_ADDRESS: &str = "added by address";

/// A link that is kept up until the returned `Link` is dropped, not in the
/// list of computers added by address (the server gateway, gateway.rs).
/// `what` names it in the log.
pub(crate) fn spawn(state: &Arc<SignalingState>, address: String, password: String, host_id: Option<String>, what: &'static str) -> Link {
    let error = Arc::new(Mutex::new(None));
    let known_id = Arc::new(Mutex::new(host_id));
    let needs_approval = password.trim().is_empty();
    let task = tokio::spawn(run(state.clone(), address, password, error.clone(), known_id.clone(), what));
    Link { task, error, host_id: known_id, needs_approval }
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
    what: &'static str,
) {
    loop {
        let wait = match connect_once(&state, &address, &password, &error, &host_id, what).await {
            Ok(()) => {
                info!("Lost the connection to {} ({}); reconnecting", address, what);
                RETRY
            }
            Err((message, refused)) => {
                let changed = error.lock().unwrap().as_deref() != Some(message.as_str());
                if changed {
                    warn!("Cannot connect to {} ({}): {}", address, what, message);
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
    what: &str,
) -> Result<(), (String, bool)> {
    let name_query = format!("name={}", urlencoding::encode(&server_name()));
    // A computer added by address that offered v2 (SPAKE2) once must keep
    // to it: an old-style (v1) answer could be someone at its address
    // fishing for a MAC to test passwords against offline, or (without a
    // password) passing as an older agent whose certificate isn't pinned.
    let pake_seen = what == ADDED_BY_ADDRESS && pake_seen(state, address).await?;
    // (Whether v2 was used is not needed here: what matters for the pin is
    // that it was offered.)
    let (mut ws, local_ip, cert_hash, _pake, offered_v2) =
        match direct_auth::connect(address, "/ws/server", password, &name_query, pake_seen).await {
            Ok(c) => (c.ws, c.local_ip, direct_auth::to_hex(&c.cert_hash), c.pake, c.offered_v2),
            Err(e) => {
                // Also slower when TLS fails or the agent can't prove itself:
                // retrying fast won't change it.
                use direct_auth::ConnectError as E;
                if e == E::Downgraded {
                    warn!(
                        "{} ({}) offered only the old password proof (v1) although it spoke v2 before: refused, no password proof sent; \
                         if its agent was really downgraded, reset its key on the web page",
                        address, what
                    );
                }
                let refused = matches!(e, E::WrongPassword | E::TooManyAttempts | E::NotSecure(_) | E::NotProven | E::Downgraded);
                return Err((e.to_string(), refused));
            }
        };
    // Trust on first use: a computer added by address keeps the certificate
    // it first connected with. Without a password nothing else tells it
    // from someone on the network answering at its address. Only agents
    // that offered v2 keep their certificate across restarts (older ones
    // make a new one at every start): the pin holds once one did, and an
    // answer without v2 from there is refused (`pake_seen` → Downgraded).
    let pinned = if what == ADDED_BY_ADDRESS && pake_seen { pinned_certificate(state, address).await? } else { None };
    if let Some(pin) = &pinned {
        if !certificate_ok(Some(pin), &cert_hash) {
            let _ = ws.close(None).await;
            warn!(
                "{} ({}) answered with another certificate ({}, pinned {}): refused; if it was reinstalled, reset its key on the web page",
                address, what, cert_hash, pin
            );
            return Err((CERT_CHANGED.into(), true));
        }
    }
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
    // The id is only what the other end says: it may not take over another
    // computer's (signaling::link_may_register), nor a live connection.
    let known = host_id.lock().unwrap().clone();
    if let Err(why) = crate::signaling::link_may_register(state, &id, &name, known.as_deref()).await {
        // `run` logs it and retries slowly.
        return Err((why, true));
    }
    let Some((conn_id, mut rx)) = crate::signaling::link_connected(state, &id, &name, codec_support).await else {
        // Also briefly while its previous connection is being closed.
        return Err(("a computer with this id is already connected to the server".into(), false));
    };
    *error.lock().unwrap() = None;
    if what == ADDED_BY_ADDRESS && offered_v2 && !pake_seen {
        // The first answer from an agent that keeps its certificate: pin it
        // (replacing what an older agent there showed, which changed at
        // each start anyway) and from now on refuse answers without v2
        // (until a key reset). Only while not yet marked: a reset in
        // between is not undone by an older connection.
        let _ = sqlx::query("UPDATE address_hosts SET cert_sha256 = ?, pake_version = ? WHERE address = ? AND pake_version IS NULL")
            .bind(&cert_hash)
            .bind(direct_auth::VERSION_PAKE as i64)
            .bind(address)
            .execute(&state.db)
            .await;
        info!("Pinned the certificate of {} ({}); it offers the v2 password check, older answers from there are refused from now on", address, cert_hash);
    } else if what == ADDED_BY_ADDRESS && pake_seen && pinned.is_none() {
        // Marked but not pinned (an older version's row): pin now.
        let _ = sqlx::query("UPDATE address_hosts SET cert_sha256 = ? WHERE address = ? AND cert_sha256 IS NULL")
            .bind(&cert_hash)
            .bind(address)
            .execute(&state.db)
            .await;
        info!("Pinned the certificate of {} ({})", address, cert_hash);
    } else if what == ADDED_BY_ADDRESS && !offered_v2 {
        info!(
            "{} runs an older Lunaris agent, which makes a new certificate at each start: it can't be pinned until that agent is updated",
            address
        );
    }
    if known.is_none() {
        *host_id.lock().unwrap() = Some(id.clone());
        let _ = sqlx::query("UPDATE address_hosts SET host_id = ? WHERE address = ?")
            .bind(&id)
            .bind(address)
            .execute(&state.db)
            .await;
        // The name it was added with becomes its shown name (everywhere a
        // host's name is read: sources, groups), unless it has one.
        let _ = sqlx::query(
            "UPDATE hosts SET display_name = (SELECT NULLIF(name, '') FROM address_hosts WHERE address = ?)
             WHERE id = ? AND display_name IS NULL",
        )
        .bind(address)
        .bind(&id)
        .execute(&state.db)
        .await;
    }
    if let Some(ip) = local_ip {
        state.agent_local_ips.write().unwrap().insert(id.clone(), ip);
    }
    info!("Connected to {} ({}), {} {}", name, id, what, address);

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
                    if write.send(WsMessage::Text(json.into())).await.is_err() {
                        break;
                    }
                }
                _ = ping.tick() => {
                    if write.send(WsMessage::Ping(Default::default())).await.is_err() {
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

/// What the host list shows for a computer added by address that answers
/// with another certificate than the one pinned.
pub const CERT_CHANGED: &str =
    "this address answers with another certificate than before (reinstalled, or someone else?); refused. If it was reinstalled, reset its key";

/// What the host list shows for a computer added by address that spoke the
/// v2 password check before and now answers with only v1.
pub const DOWNGRADED: &str = "this address answers like an older agent (without the secure password check) although it had the new one before \
     (downgraded, or someone else?); refused, no password proof was sent. If its agent was really downgraded, reset its key";

/// Whether the computer added by `address` spoke v2 before (then v1 is refused).
async fn pake_seen(state: &SignalingState, address: &str) -> Result<bool, (String, bool)> {
    sqlx::query_scalar::<_, Option<i64>>("SELECT pake_version FROM address_hosts WHERE address = ?")
        .bind(address)
        .fetch_optional(&state.db)
        .await
        .map(|v| v.flatten().is_some_and(|v| v >= direct_auth::VERSION_PAKE as i64))
        .map_err(|e| (format!("server database error: {e}"), false))
}

/// Whether `seen` (hex SHA-256) may connect under the pin (None: not pinned yet).
fn certificate_ok(pinned: Option<&str>, seen: &str) -> bool {
    pinned.is_none_or(|pin| crate::security::ct_eq(&pin.to_ascii_lowercase(), &seen.to_ascii_lowercase()))
}

/// The pinned certificate of a computer added by address (None: not yet).
async fn pinned_certificate(state: &SignalingState, address: &str) -> Result<Option<String>, (String, bool)> {
    sqlx::query_scalar::<_, Option<String>>("SELECT cert_sha256 FROM address_hosts WHERE address = ?")
        .bind(address)
        .fetch_optional(&state.db)
        .await
        .map(Option::flatten)
        .map_err(|e| (format!("server database error: {e}"), false))
}

/// An admin reset the key of a host added by address (admin.rs): its pin
/// goes (and the memory that it spoke v2), and the link connects again at
/// once to pin what answers now.
/// `host_id` is the host's id, or "address:<address>". Returns whether it
/// is such a host.
pub async fn reset_pin(state: &Arc<SignalingState>, host_id: &str) -> Result<bool, sqlx::Error> {
    let row: Option<(String, String, Option<String>)> = match host_id.strip_prefix("address:") {
        Some(address) => sqlx::query_as("SELECT address, password, host_id FROM address_hosts WHERE address = ?")
            .bind(address)
            .fetch_optional(&state.db)
            .await?,
        None => sqlx::query_as("SELECT address, password, host_id FROM address_hosts WHERE host_id = ?")
            .bind(host_id)
            .fetch_optional(&state.db)
            .await?,
    };
    let Some((address, password, known_id)) = row else { return Ok(false) };
    sqlx::query("UPDATE address_hosts SET cert_sha256 = NULL, pake_version = NULL WHERE address = ?")
        .bind(&address)
        .execute(&state.db)
        .await?;
    info!("The certificate pin of {} was reset", address);
    start(state, address, password, known_id);
    Ok(true)
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

/// Client side of the agent's direct-connection handshake (the agent's side
/// and the desktop app's copy are agent/src/direct/handshake.rs, where the
/// protocol is described; the test vectors below are the same there, so the
/// two cannot drift apart).
///
/// TLS first, accepting the agent's self-signed certificate: there is
/// nothing to check it against. What stops a man in the middle is the
/// password check, bound to the certificate seen here (the
/// `tls-server-end-point` channel binding, RFC 5929):
/// - v2 (agents that offer it): SPAKE2 over Ed25519, both identities carrying
///   SHA-256(label || both nonces || certificate hash); the agent confirms
///   the key first, we confirm ours only when its confirmation checked out.
///   Someone answering at the address learns nothing to test passwords
///   against offline.
/// - v1 (older agents): HMAC-SHA256(password, label || agent nonce || client
///   nonce || certificate hash) from us, the agent's over another label
///   back. Whoever answers could test passwords against our MAC offline, so
///   a computer that spoke v2 once is refused when it answers v1
///   (`require_pake`, `address_hosts.pake_version`).
///
/// The password itself never crosses the network. With a password, only an
/// answer proving the agent knows it is accepted. Older agents don't speak
/// TLS: no connection then (no plain fallback that would expose the password).
mod direct_auth {
    use std::net::IpAddr;
    use std::sync::Arc;
    use std::time::Duration;

    use futures_util::{SinkExt, StreamExt};
    use ring::hmac;
    use ring::rand::{SecureRandom, SystemRandom};
    use serde::{Deserialize, Serialize};
    use tokio::io::{AsyncRead, AsyncWrite};
    use tokio::net::TcpStream;
    use tokio_rustls::rustls;
    use tokio_rustls::rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
    use tokio_rustls::rustls::crypto::CryptoProvider;
    use tokio_rustls::rustls::pki_types::{CertificateDer, ServerName, UnixTime};
    use tokio_rustls::rustls::{DigitallySignedStruct, SignatureScheme};
    use tokio_tungstenite::tungstenite::{self, Message as WsMessage};
    use tokio_tungstenite::WebSocketStream;

    pub const CLIENT_LABEL: &[u8] = b"lunaris-direct-auth-v1 client";
    pub const AGENT_LABEL: &[u8] = b"lunaris-direct-auth-v1 agent";
    pub type Nonce = [u8; 32];
    pub type Digest = [u8; 32];
    /// The v2 handshake (SPAKE2), as `hello`/`challenge` advertise it.
    pub const VERSION_PAKE: u32 = 2;
    pub const V2_LABEL: &[u8] = b"lunaris-direct-auth-v2";
    const PAKE_CLIENT_ID: &[u8] = b"lunaris-direct-auth-v2 client";
    const PAKE_AGENT_ID: &[u8] = b"lunaris-direct-auth-v2 agent";
    const CLIENT_CONFIRM_LABEL: &[u8] = b"lunaris-direct-auth-v2 client confirm";
    const AGENT_CONFIRM_LABEL: &[u8] = b"lunaris-direct-auth-v2 agent confirm";
    pub const PAKE_MSG_LEN: usize = 33;
    pub type PakeMsg = [u8; PAKE_MSG_LEN];
    pub type Pake = spake2::Spake2<spake2::Ed25519Group>;

    const STEP_TIMEOUT: Duration = Duration::from_secs(8);

    pub fn sha256(data: &[u8]) -> Digest {
        ring::digest::digest(&ring::digest::SHA256, data).as_ref().try_into().expect("SHA-256 is 32 bytes")
    }

    fn mac_input(label: &[u8], agent_nonce: &Nonce, client_nonce: &Nonce, cert_hash: &Digest) -> Vec<u8> {
        [label, &agent_nonce[..], &client_nonce[..], &cert_hash[..]].concat()
    }

    fn mac_key(password: &str) -> hmac::Key {
        hmac::Key::new(hmac::HMAC_SHA256, password.trim().as_bytes())
    }

    pub fn mac(label: &[u8], password: &str, agent_nonce: &Nonce, client_nonce: &Nonce, cert_hash: &Digest) -> Digest {
        let tag = hmac::sign(&mac_key(password), &mac_input(label, agent_nonce, client_nonce, cert_hash));
        tag.as_ref().try_into().expect("HMAC-SHA256 is 32 bytes")
    }

    /// Constant time (ring's HMAC verify).
    pub fn mac_ok(label: &[u8], password: &str, agent_nonce: &Nonce, client_nonce: &Nonce, cert_hash: &Digest, tag: &[u8]) -> bool {
        hmac::verify(&mac_key(password), &mac_input(label, agent_nonce, client_nonce, cert_hash), tag).is_ok()
    }

    /// What both PAKE identities carry: this run and this TLS channel.
    pub fn v2_context(agent_nonce: &Nonce, client_nonce: &Nonce, cert_hash: &Digest) -> Digest {
        sha256(&[V2_LABEL, &agent_nonce[..], &client_nonce[..], &cert_hash[..]].concat())
    }

    fn pake_ids(context: &Digest) -> (spake2::Identity, spake2::Identity) {
        (
            spake2::Identity::new(&[PAKE_CLIENT_ID, &context[..]].concat()),
            spake2::Identity::new(&[PAKE_AGENT_ID, &context[..]].concat()),
        )
    }

    /// The OS random source (ring's) for SPAKE2.
    pub struct OsRng;

    impl rand_core::TryRng for OsRng {
        type Error = std::convert::Infallible;
        fn try_next_u32(&mut self) -> Result<u32, Self::Error> {
            let mut b = [0u8; 4];
            self.try_fill_bytes(&mut b)?;
            Ok(u32::from_le_bytes(b))
        }
        fn try_next_u64(&mut self) -> Result<u64, Self::Error> {
            let mut b = [0u8; 8];
            self.try_fill_bytes(&mut b)?;
            Ok(u64::from_le_bytes(b))
        }
        fn try_fill_bytes(&mut self, dst: &mut [u8]) -> Result<(), Self::Error> {
            SystemRandom::new().fill(dst).expect("the OS random source failed");
            Ok(())
        }
    }

    impl rand_core::TryCryptoRng for OsRng {}

    /// Our side (role A): the state and message X.
    pub fn pake_client(password: &str, context: &Digest, rng: impl rand_core::CryptoRng) -> (Pake, PakeMsg) {
        let (a, b) = pake_ids(context);
        let (state, msg) = Pake::start_a_with_rng(&spake2::Password::new(password.trim().as_bytes()), &a, &b, rng);
        (state, msg.try_into().expect("SPAKE2 Ed25519 messages are 33 bytes"))
    }

    /// The agent's side (role B), for the tests' fake agents.
    #[cfg(test)]
    pub fn pake_agent(password: &str, context: &Digest, rng: impl rand_core::CryptoRng) -> (Pake, PakeMsg) {
        let (a, b) = pake_ids(context);
        let (state, msg) = Pake::start_b_with_rng(&spake2::Password::new(password.trim().as_bytes()), &a, &b, rng);
        (state, msg.try_into().expect("SPAKE2 Ed25519 messages are 33 bytes"))
    }

    /// The two key confirmations of a run: (agent's, ours).
    pub fn confirmations(key: &[u8], context: &Digest, client_msg: &PakeMsg, agent_msg: &PakeMsg) -> (Digest, Digest) {
        let key = hmac::Key::new(hmac::HMAC_SHA256, key);
        let tag = |label: &[u8]| -> Digest {
            let input = [label, &context[..], &client_msg[..], &agent_msg[..]].concat();
            hmac::sign(&key, &input).as_ref().try_into().expect("HMAC-SHA256 is 32 bytes")
        };
        (tag(AGENT_CONFIRM_LABEL), tag(CLIENT_CONFIRM_LABEL))
    }

    /// Constant-time comparison of two digests.
    fn digest_eq(a: &Digest, b: &Digest) -> bool {
        a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
    }

    /// Whether the agent's `pake_reply` checks out: then our confirmation.
    fn judge_pake_reply(state: Pake, context: &Digest, client_msg: &PakeMsg, reply_msg: &str, reply_confirm: &str) -> Option<Digest> {
        let agent_msg = from_hex::<PAKE_MSG_LEN>(reply_msg)?;
        let key = state.finish(&agent_msg).ok()?;
        let (agent_confirm, client_confirm) = confirmations(&key, context, client_msg, &agent_msg);
        let tag = from_hex::<32>(reply_confirm)?;
        digest_eq(&tag, &agent_confirm).then_some(client_confirm)
    }

    /// The agent's answer to our `confirm`; `proven`: its confirmation
    /// checked out.
    fn judge_v2_answer(answer: Option<AuthMessage>, proven: bool) -> Result<bool, ConnectError> {
        match answer {
            Some(AuthMessage::Refused { reason }) => Err(refusal(&reason)),
            Some(AuthMessage::Ok { trusted, .. }) if proven => Ok(trusted),
            Some(AuthMessage::Ok { .. }) => Err(ConnectError::NotProven),
            _ if !proven => Err(ConnectError::NotProven),
            _ => Err(ConnectError::Protocol("unexpected answer to the password check".into())),
        }
    }

    pub fn to_hex(bytes: &[u8]) -> String {
        bytes.iter().map(|b| format!("{:02x}", b)).collect()
    }

    pub fn from_hex<const N: usize>(text: &str) -> Option<[u8; N]> {
        if text.len() != 2 * N || !text.bytes().all(|c| c.is_ascii_hexdigit()) {
            return None;
        }
        let mut out = [0u8; N];
        for (i, byte) in out.iter_mut().enumerate() {
            *byte = u8::from_str_radix(&text[2 * i..2 * i + 2], 16).ok()?;
        }
        Some(out)
    }

    #[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
    #[serde(tag = "auth", rename_all = "snake_case")]
    pub enum AuthMessage {
        Hello { version: u32 },
        Challenge {
            nonce: String,
            /// Versions besides v1; absent from agents before v2.
            #[serde(default, skip_serializing_if = "Vec::is_empty")]
            versions: Vec<u32>,
        },
        Proof {
            client_nonce: String,
            #[serde(default, skip_serializing_if = "Option::is_none")]
            mac: Option<String>,
        },
        Ok {
            trusted: bool,
            #[serde(default, skip_serializing_if = "Option::is_none")]
            mac: Option<String>,
        },
        Refused { reason: String },
        Pake { client_nonce: String, msg: String },
        PakeReply { msg: String, confirm: String },
        Confirm {
            #[serde(default, skip_serializing_if = "Option::is_none")]
            mac: Option<String>,
        },
    }

    /// The agent's self-signed certificate is accepted (see the module
    /// comment); handshake signatures are still checked, so the peer owns
    /// the key of the certificate the proof is bound to.
    #[derive(Debug)]
    struct AcceptAnyCert(Arc<CryptoProvider>);

    impl ServerCertVerifier for AcceptAnyCert {
        fn verify_server_cert(
            &self,
            _end_entity: &CertificateDer<'_>,
            _intermediates: &[CertificateDer<'_>],
            _server_name: &ServerName<'_>,
            _ocsp_response: &[u8],
            _now: UnixTime,
        ) -> Result<ServerCertVerified, rustls::Error> {
            Ok(ServerCertVerified::assertion())
        }

        fn verify_tls12_signature(
            &self,
            message: &[u8],
            cert: &CertificateDer<'_>,
            dss: &DigitallySignedStruct,
        ) -> Result<HandshakeSignatureValid, rustls::Error> {
            rustls::crypto::verify_tls12_signature(message, cert, dss, &self.0.signature_verification_algorithms)
        }

        fn verify_tls13_signature(
            &self,
            message: &[u8],
            cert: &CertificateDer<'_>,
            dss: &DigitallySignedStruct,
        ) -> Result<HandshakeSignatureValid, rustls::Error> {
            rustls::crypto::verify_tls13_signature(message, cert, dss, &self.0.signature_verification_algorithms)
        }

        fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
            self.0.signature_verification_algorithms.supported_schemes()
        }
    }

    fn server_name(address: &str) -> ServerName<'static> {
        let host = if let Some(rest) = address.strip_prefix('[') {
            rest.split(']').next().unwrap_or("")
        } else {
            address.rsplit_once(':').map(|(h, _)| h).unwrap_or(address)
        };
        ServerName::try_from(host.to_string())
            .unwrap_or_else(|_| ServerName::try_from("lunaris-agent").expect("valid DNS name"))
    }

    pub trait Io: AsyncRead + AsyncWrite + Unpin + Send {}
    impl<T: AsyncRead + AsyncWrite + Unpin + Send> Io for T {}

    pub struct Connected {
        pub ws: WebSocketStream<Box<dyn Io>>,
        pub local_ip: Option<IpAddr>,
        /// SHA-256 of the agent's TLS certificate (pinned by `connect_once`).
        pub cert_hash: Digest,
        /// The password was checked with v2 (SPAKE2); false with an older
        /// agent or without a password.
        pub pake: bool,
        /// The agent offered v2: it is new enough to keep its certificate
        /// across restarts (older agents make a new one at every start, so
        /// their certificate can't be pinned).
        pub offered_v2: bool,
    }

    #[derive(Debug, Clone, PartialEq)]
    pub enum ConnectError {
        Unreachable(String),
        /// TLS failed: an agent from before secure direct connections (or
        /// someone in the way). The password is never sent without TLS.
        NotSecure(String),
        WrongPassword,
        /// With a password: no valid MAC from the agent in the answer.
        NotProven,
        TooManyAttempts,
        /// No v2 offered by an agent that offered it before (`require_pake`);
        /// nothing that depends on the password was sent.
        Downgraded,
        NotFound,
        Refused(String),
        Protocol(String),
    }

    impl std::fmt::Display for ConnectError {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            match self {
                ConnectError::Unreachable(e) => write!(f, "not reachable ({})", e),
                ConnectError::NotSecure(e) => write!(
                    f,
                    "cannot connect securely (TLS failed: {}); if it runs a Lunaris agent older than {}, update that agent",
                    e,
                    env!("CARGO_PKG_VERSION")
                ),
                ConnectError::WrongPassword => write!(f, "wrong password"),
                ConnectError::NotProven => write!(f, "wrong password, or this is not the computer you expect"),
                ConnectError::TooManyAttempts => write!(f, "too many wrong passwords; retrying later"),
                ConnectError::Downgraded => write!(f, "{}", super::DOWNGRADED),
                ConnectError::NotFound => write!(f, "not a Lunaris agent, or one too old for this (update it)"),
                ConnectError::Refused(e) => write!(f, "refused ({})", e),
                ConnectError::Protocol(e) => write!(f, "{}", e),
            }
        }
    }

    fn http_error(e: tungstenite::Error) -> ConnectError {
        match e {
            tungstenite::Error::Http(resp) => match resp.status().as_u16() {
                401 => ConnectError::WrongPassword,
                429 => ConnectError::TooManyAttempts,
                404 => ConnectError::NotFound,
                code => ConnectError::Refused(format!("HTTP {}", code)),
            },
            e => ConnectError::Unreachable(e.to_string()),
        }
    }

    async fn tcp(address: &str) -> Result<TcpStream, ConnectError> {
        match tokio::time::timeout(STEP_TIMEOUT, TcpStream::connect(address)).await {
            Ok(Ok(s)) => {
                let _ = s.set_nodelay(true);
                Ok(s)
            }
            Ok(Err(e)) => Err(ConnectError::Unreachable(e.to_string())),
            Err(_) => Err(ConnectError::Unreachable("timed out".into())),
        }
    }

    async fn upgrade(url: String, stream: Box<dyn Io>) -> Result<WebSocketStream<Box<dyn Io>>, ConnectError> {
        match tokio::time::timeout(STEP_TIMEOUT, tokio_tungstenite::client_async(url, stream)).await {
            Ok(r) => r.map(|(ws, _)| ws).map_err(http_error),
            Err(_) => Err(ConnectError::Unreachable("timed out".into())),
        }
    }

    /// `address` "host:port", `path` "/ws/server", `query` extra parameters
    /// (never the password). `require_pake`: an agent that does not offer
    /// v2 is refused (`ConnectError::Downgraded`), with or without a
    /// password, before anything that depends on the password is sent.
    pub async fn connect(address: &str, path: &str, password: &str, query: &str, require_pake: bool) -> Result<Connected, ConnectError> {
        let query = query.trim_start_matches(['&', '?']);
        let stream = tcp(address).await?;
        let local_ip = stream.local_addr().ok().map(|a| a.ip());
        let provider = Arc::new(rustls::crypto::ring::default_provider());
        let config = rustls::ClientConfig::builder_with_provider(provider.clone())
            .with_safe_default_protocol_versions()
            .map_err(|e| ConnectError::Protocol(e.to_string()))?
            .dangerous()
            .with_custom_certificate_verifier(Arc::new(AcceptAnyCert(provider)))
            .with_no_client_auth();
        let connector = tokio_rustls::TlsConnector::from(Arc::new(config));
        // TLS only: no plain fallback, which anyone on the network could
        // force (the retry loop would hand them the password every time).
        let tls = match tokio::time::timeout(STEP_TIMEOUT, connector.connect(server_name(address), stream)).await {
            Ok(Ok(tls)) => tls,
            Ok(Err(e)) => return Err(ConnectError::NotSecure(e.to_string())),
            Err(_) => return Err(ConnectError::NotSecure("timed out".into())),
        };
        let cert_hash = tls
            .get_ref()
            .1
            .peer_certificates()
            .and_then(|certs| certs.first())
            .map(|cert| sha256(cert))
            .ok_or_else(|| ConnectError::Protocol("no certificate".into()))?;
        let sep = if query.is_empty() { "" } else { "?" };
        let mut ws = upgrade(format!("wss://{}{}{}{}", address, path, sep, query), Box::new(tls)).await?;
        // Untrusted (no password): the person at the computer accepts each
        // session; `Link::needs_approval` already says so.
        let (_trusted, pake, offered_v2) =
            match tokio::time::timeout(STEP_TIMEOUT, client_auth(&mut ws, password, &cert_hash, require_pake, address)).await {
                Ok(r) => r?,
                Err(_) => return Err(ConnectError::Protocol("no answer to the password proof".into())),
            };
        Ok(Connected { ws, local_ip, cert_hash, pake, offered_v2 })
    }

    async fn send_auth(ws: &mut WebSocketStream<Box<dyn Io>>, msg: &AuthMessage) -> Result<(), ConnectError> {
        let json = serde_json::to_string(msg).expect("handshake messages serialize");
        ws.send(WsMessage::Text(json.into())).await.map_err(|e| ConnectError::Unreachable(e.to_string()))
    }

    async fn recv_auth(ws: &mut WebSocketStream<Box<dyn Io>>) -> Option<AuthMessage> {
        loop {
            match ws.next().await? {
                Ok(WsMessage::Text(text)) => return serde_json::from_str(&text).ok(),
                Ok(WsMessage::Ping(_) | WsMessage::Pong(_)) => continue,
                _ => return None,
            }
        }
    }

    fn refusal(reason: &str) -> ConnectError {
        match reason {
            "unauthorized" => ConnectError::WrongPassword,
            "too_many_attempts" => ConnectError::TooManyAttempts,
            other => ConnectError::Refused(other.chars().filter(|c| !c.is_control()).take(100).collect()),
        }
    }

    /// Ok((trusted, v2 used, v2 offered)); trusted false: the person at the
    /// computer will be asked. `require_pake`: this address offered v2
    /// before, so an answer without it is refused whether or not there is a
    /// password (an impostor would otherwise pass as an older agent, whose
    /// certificate isn't pinned).
    async fn client_auth(
        ws: &mut WebSocketStream<Box<dyn Io>>,
        password: &str,
        cert_hash: &Digest,
        require_pake: bool,
        address: &str,
    ) -> Result<(bool, bool, bool), ConnectError> {
        send_auth(ws, &AuthMessage::Hello { version: VERSION_PAKE }).await?;
        let (agent_nonce, versions) = match recv_auth(ws).await {
            Some(AuthMessage::Challenge { nonce, versions }) => (from_hex::<32>(&nonce), versions),
            Some(AuthMessage::Refused { reason }) => return Err(refusal(&reason)),
            _ => (None, Vec::new()),
        };
        let agent_nonce = agent_nonce.ok_or_else(|| ConnectError::Protocol("unexpected answer (not a Lunaris agent?)".into()))?;
        let mut client_nonce = [0u8; 32];
        SystemRandom::new()
            .fill(&mut client_nonce)
            .map_err(|_| ConnectError::Protocol("no random numbers".into()))?;
        let has_password = !password.trim().is_empty();
        let offered = versions.contains(&VERSION_PAKE);
        if require_pake && !offered {
            return Err(ConnectError::Downgraded);
        }
        if has_password && offered {
            let context = v2_context(&agent_nonce, &client_nonce, cert_hash);
            let (state, client_msg) = pake_client(password, &context, OsRng);
            send_auth(ws, &AuthMessage::Pake { client_nonce: to_hex(&client_nonce), msg: to_hex(&client_msg) }).await?;
            let client_confirm = match recv_auth(ws).await {
                Some(AuthMessage::PakeReply { msg, confirm }) => judge_pake_reply(state, &context, &client_msg, &msg, &confirm),
                Some(AuthMessage::Refused { reason }) => return Err(refusal(&reason)),
                Some(AuthMessage::Ok { .. }) => return Err(ConnectError::NotProven),
                _ => return Err(ConnectError::Protocol("unexpected answer to the password check".into())),
            };
            let proven = client_confirm.is_some();
            send_auth(ws, &AuthMessage::Confirm { mac: client_confirm.map(|c| to_hex(&c)) }).await?;
            return judge_v2_answer(recv_auth(ws).await, proven).map(|trusted| (trusted, true, true));
        }
        if has_password {
            tracing::warn!(
                "{} runs an older Lunaris agent (password proof v1): whoever answers at that address could test \
                 passwords against it offline; update that agent",
                address
            );
        }
        let mac = has_password.then(|| to_hex(&mac(CLIENT_LABEL, password, &agent_nonce, &client_nonce, cert_hash)));
        send_auth(ws, &AuthMessage::Proof { client_nonce: to_hex(&client_nonce), mac }).await?;
        judge_answer(recv_auth(ws).await, password, &agent_nonce, &client_nonce, cert_hash).map(|trusted| (trusted, false, offered))
    }

    /// Ok(trusted). With a password, only an answer carrying the agent's
    /// valid MAC goes on: "ok, untrusted" without it could come from an
    /// impostor with its own certificate.
    fn judge_answer(
        answer: Option<AuthMessage>,
        password: &str,
        agent_nonce: &Nonce,
        client_nonce: &Nonce,
        cert_hash: &Digest,
    ) -> Result<bool, ConnectError> {
        let has_password = !password.trim().is_empty();
        match answer {
            Some(AuthMessage::Refused { reason }) => Err(refusal(&reason)),
            Some(AuthMessage::Ok { trusted, mac }) if has_password => {
                let tag = mac.as_deref().and_then(from_hex::<32>);
                let proven = tag.is_some_and(|t| mac_ok(AGENT_LABEL, password, agent_nonce, client_nonce, cert_hash, &t));
                // Proven but untrusted: an agent in click mode, it asks there.
                if proven { Ok(trusted) } else { Err(ConnectError::NotProven) }
            }
            Some(AuthMessage::Ok { trusted: false, .. }) => Ok(false),
            Some(AuthMessage::Ok { trusted: true, .. }) => Err(ConnectError::NotProven),
            _ => Err(ConnectError::Protocol("unexpected answer to the password proof".into())),
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        // The same vectors are in agent/src/direct/handshake.rs; computed
        // independently (Python hmac/hashlib).
        const PASSWORD: &str = "abcd-efgh-ijkl";
        const CERT_HASH_HEX: &str = "fc105ce3018116bb697b777e09fd7ce7039e628a31ad7aab6c9bbe02c3bf0d1d";
        const CLIENT_MAC_HEX: &str = "5443f1e19d8c91e2256950e29b0476d5f90f9e9bba6ee2d799e271af115739cd";
        const AGENT_MAC_HEX: &str = "a389673baba901cf246ec3b4b83db77a97e466384fc8c76897ddd2d3abe497e8";

        #[test]
        fn mac_test_vectors() {
            let cert_hash = sha256(b"lunaris test certificate");
            assert_eq!(to_hex(&cert_hash), CERT_HASH_HEX);
            let (an, cn) = ([0x11u8; 32], [0x22u8; 32]);
            assert_eq!(to_hex(&mac(CLIENT_LABEL, PASSWORD, &an, &cn, &cert_hash)), CLIENT_MAC_HEX);
            assert_eq!(to_hex(&mac(AGENT_LABEL, PASSWORD, &an, &cn, &cert_hash)), AGENT_MAC_HEX);
            assert_eq!(to_hex(&mac(CLIENT_LABEL, " abcd-efgh-ijkl\n", &an, &cn, &cert_hash)), CLIENT_MAC_HEX);
            let tag = from_hex::<32>(AGENT_MAC_HEX).unwrap();
            assert!(mac_ok(AGENT_LABEL, PASSWORD, &an, &cn, &cert_hash, &tag));
            assert!(!mac_ok(CLIENT_LABEL, PASSWORD, &an, &cn, &cert_hash, &tag));
            assert!(!mac_ok(AGENT_LABEL, PASSWORD, &an, &cn, &sha256(b"other"), &tag));
        }

        #[test]
        fn message_test_vectors() {
            let json = |m: &AuthMessage| serde_json::to_string(m).unwrap();
            assert_eq!(json(&AuthMessage::Hello { version: 1 }), r#"{"auth":"hello","version":1}"#);
            assert_eq!(json(&AuthMessage::Challenge { nonce: "ab".into(), versions: vec![] }), r#"{"auth":"challenge","nonce":"ab"}"#);
            assert_eq!(
                json(&AuthMessage::Proof { client_nonce: "cd".into(), mac: Some("ef".into()) }),
                r#"{"auth":"proof","client_nonce":"cd","mac":"ef"}"#
            );
            assert_eq!(json(&AuthMessage::Proof { client_nonce: "cd".into(), mac: None }), r#"{"auth":"proof","client_nonce":"cd"}"#);
            assert_eq!(json(&AuthMessage::Ok { trusted: false, mac: None }), r#"{"auth":"ok","trusted":false}"#);
            assert_eq!(
                json(&AuthMessage::Refused { reason: "too_many_attempts".into() }),
                r#"{"auth":"refused","reason":"too_many_attempts"}"#
            );
            assert_eq!(refusal("unauthorized"), ConnectError::WrongPassword);
        }

        #[test]
        fn with_a_password_only_the_agents_valid_mac_is_accepted() {
            let (an, cn, ch) = ([1u8; 32], [2u8; 32], sha256(b"cert"));
            let good = Some(to_hex(&mac(AGENT_LABEL, "pw", &an, &cn, &ch)));
            let ok = |trusted: bool, mac: Option<String>| Some(AuthMessage::Ok { trusted, mac });
            assert_eq!(judge_answer(ok(true, good.clone()), "pw", &an, &cn, &ch), Ok(true));
            assert_eq!(judge_answer(ok(false, good.clone()), "pw", &an, &cn, &ch), Ok(false));
            assert_eq!(judge_answer(ok(false, None), "pw", &an, &cn, &ch), Err(ConnectError::NotProven));
            assert_eq!(judge_answer(ok(true, None), "pw", &an, &cn, &ch), Err(ConnectError::NotProven));
            let client_side = Some(to_hex(&mac(CLIENT_LABEL, "pw", &an, &cn, &ch)));
            assert_eq!(judge_answer(ok(true, client_side), "pw", &an, &cn, &ch), Err(ConnectError::NotProven));
            let other_cert = Some(to_hex(&mac(AGENT_LABEL, "pw", &an, &cn, &sha256(b"other"))));
            assert_eq!(judge_answer(ok(true, other_cert), "pw", &an, &cn, &ch), Err(ConnectError::NotProven));
            assert_eq!(judge_answer(ok(false, None), "", &an, &cn, &ch), Ok(false));
            assert_eq!(judge_answer(ok(true, good), "", &an, &cn, &ch), Err(ConnectError::NotProven));
        }

        pub(crate) struct FixedRng(pub u8);

        impl rand_core::TryRng for FixedRng {
            type Error = std::convert::Infallible;
            fn try_next_u32(&mut self) -> Result<u32, Self::Error> {
                let mut b = [0u8; 4];
                self.try_fill_bytes(&mut b)?;
                Ok(u32::from_le_bytes(b))
            }
            fn try_next_u64(&mut self) -> Result<u64, Self::Error> {
                let mut b = [0u8; 8];
                self.try_fill_bytes(&mut b)?;
                Ok(u64::from_le_bytes(b))
            }
            fn try_fill_bytes(&mut self, dst: &mut [u8]) -> Result<(), Self::Error> {
                for (i, b) in dst.iter_mut().enumerate() {
                    *b = self.0.wrapping_add(i as u8);
                }
                Ok(())
            }
        }

        impl rand_core::TryCryptoRng for FixedRng {}

        // The same v2 vectors as agent/src/direct/handshake.rs (context and
        // confirmations also computed with Python hashlib/hmac).
        const CONTEXT_HEX: &str = "f10f7c03fdf9223a967c8f2159ac7db893d605a749896ad6713c179e3b9d3b3b";
        const X_HEX: &str = "41d1ec4c004e3ddfff0d121a1e00dbf012ca79514cd0de9c890cb9fa80f3601d98";
        const Y_HEX: &str = "4292015d892636f03b4a6ebe39b18cd673d90d10fed452f6e925b043695a97e482";
        const KEY_HEX: &str = "2f27a9b7db0666500a3e7f454a4c830f02f752d0fe2ac3a3894afa066553b07b";
        const AGENT_CONFIRM_HEX: &str = "a91983da23e1ea1a612989a6d2a32684fd850d639e12327f25e9d55dced4df67";
        const CLIENT_CONFIRM_HEX: &str = "62debb0ceb6a09e145c9c016219a15111aa3e276f59d4136be70c469c6879244";

        #[test]
        fn pake_test_vectors() {
            let cert_hash = sha256(b"lunaris test certificate");
            let (an, cn) = ([0x11u8; 32], [0x22u8; 32]);
            let context = v2_context(&an, &cn, &cert_hash);
            assert_eq!(to_hex(&context), CONTEXT_HEX);
            let (a, x) = pake_client(PASSWORD, &context, FixedRng(1));
            let (b, y) = pake_agent(PASSWORD, &context, FixedRng(2));
            assert_eq!((to_hex(&x).as_str(), to_hex(&y).as_str()), (X_HEX, Y_HEX));
            let key = a.finish(&y).unwrap();
            assert_eq!(key, b.finish(&x).unwrap());
            assert_eq!(to_hex(&key), KEY_HEX);
            let (ac, cc) = confirmations(&key, &context, &x, &y);
            assert_eq!((to_hex(&ac).as_str(), to_hex(&cc).as_str()), (AGENT_CONFIRM_HEX, CLIENT_CONFIRM_HEX));
            let (_, x2) = pake_client(" abcd-efgh-ijkl\n", &context, FixedRng(1));
            assert_eq!(x2, x);
        }

        #[test]
        fn v2_message_test_vectors() {
            let json = |m: &AuthMessage| serde_json::to_string(m).unwrap();
            assert_eq!(json(&AuthMessage::Hello { version: 2 }), r#"{"auth":"hello","version":2}"#);
            assert_eq!(
                json(&AuthMessage::Challenge { nonce: "ab".into(), versions: vec![2] }),
                r#"{"auth":"challenge","nonce":"ab","versions":[2]}"#
            );
            assert_eq!(
                json(&AuthMessage::Pake { client_nonce: "cd".into(), msg: "41".into() }),
                r#"{"auth":"pake","client_nonce":"cd","msg":"41"}"#
            );
            assert_eq!(
                json(&AuthMessage::PakeReply { msg: "42".into(), confirm: "ef".into() }),
                r#"{"auth":"pake_reply","msg":"42","confirm":"ef"}"#
            );
            assert_eq!(json(&AuthMessage::Confirm { mac: Some("01".into()) }), r#"{"auth":"confirm","mac":"01"}"#);
            assert_eq!(json(&AuthMessage::Confirm { mac: None }), r#"{"auth":"confirm"}"#);
            let old: AuthMessage = serde_json::from_str(r#"{"auth":"challenge","nonce":"ab"}"#).unwrap();
            assert_eq!(old, AuthMessage::Challenge { nonce: "ab".into(), versions: vec![] });
        }

        #[test]
        fn a_v2_reply_checks_out_only_with_the_password_and_this_certificate() {
            let ch = sha256(b"cert");
            let ctx = v2_context(&[1; 32], &[2; 32], &ch);
            let reply = |pw: &str, ctx: &Digest, x: &PakeMsg| {
                let (b, y) = pake_agent(pw, ctx, OsRng);
                let key = b.finish(x).unwrap();
                let (ac, cc) = confirmations(&key, ctx, x, &y);
                (y, ac, cc)
            };
            let (a, x) = pake_client("pw", &ctx, OsRng);
            let (y, ac, cc) = reply("pw", &ctx, &x);
            assert_eq!(judge_pake_reply(a, &ctx, &x, &to_hex(&y), &to_hex(&ac)), Some(cc));
            let (a, x) = pake_client("pw", &ctx, OsRng);
            let (y, ac, _) = reply("other", &ctx, &x);
            assert_eq!(judge_pake_reply(a, &ctx, &x, &to_hex(&y), &to_hex(&ac)), None);
            let (a, x) = pake_client("pw", &ctx, OsRng);
            let (y, ac, _) = reply("pw", &v2_context(&[1; 32], &[2; 32], &sha256(b"relay")), &x);
            assert_eq!(judge_pake_reply(a, &ctx, &x, &to_hex(&y), &to_hex(&ac)), None);
            let (a, x) = pake_client("pw", &ctx, OsRng);
            let (mut y, ac, _) = reply("pw", &ctx, &x);
            y[3] ^= 1;
            assert_eq!(judge_pake_reply(a, &ctx, &x, &to_hex(&y), &to_hex(&ac)), None);
            let (a, x) = pake_client("pw", &ctx, OsRng);
            let (y, _, cc) = reply("pw", &ctx, &x);
            assert_eq!(judge_pake_reply(a, &ctx, &x, &to_hex(&y), &to_hex(&cc)), None);
            assert_eq!(judge_v2_answer(Some(AuthMessage::Ok { trusted: true, mac: None }), false), Err(ConnectError::NotProven));
            assert_eq!(judge_v2_answer(Some(AuthMessage::Ok { trusted: true, mac: None }), true), Ok(true));
        }

        /// v2, v1 first contact and a refused downgrade against fake agents.
        #[tokio::test]
        async fn new_and_old_agents_and_a_refused_downgrade() {
            let v2 = super::super::fake_agent::start("pw", true, None).await;
            let c = connect(&v2.addr, "/ws/server", "pw", "", true).await.unwrap();
            assert!(c.pake);
            assert_eq!(c.cert_hash, v2.cert_hash);
            assert_eq!(connect(&v2.addr, "/ws/server", "nope", "", false).await.err(), Some(ConnectError::WrongPassword));
            // No password: nothing proven, the person there accepts.
            assert!(!connect(&v2.addr, "/ws/server", "", "", true).await.unwrap().pake);
            let v1 = super::super::fake_agent::start("pw", false, None).await;
            let c = connect(&v1.addr, "/ws/server", "pw", "", false).await.unwrap();
            assert!(!c.pake);
            assert_eq!(v1.proofs(), 1);
            assert_eq!(connect(&v1.addr, "/ws/server", "pw", "", true).await.err(), Some(ConnectError::Downgraded));
            tokio::time::sleep(Duration::from_millis(200)).await;
            assert_eq!(v1.proofs(), 1, "a v1 proof was sent although v2 was required");
        }

        /// An agent that never sends its confirmation (or a wrong one): no
        /// connection, and our confirmation is not sent.
        #[tokio::test]
        async fn an_impostor_offering_v2_gets_nothing() {
            let fake = super::super::fake_agent::start("not-the-password", true, None).await;
            assert_eq!(connect(&fake.addr, "/ws/server", "pw", "", true).await.err(), Some(ConnectError::WrongPassword));
            assert_eq!(fake.client_confirms(), 0);
        }

        /// An agent from before TLS: after the failed TLS hello, nothing
        /// else (no plain request, no password) reaches it.
        #[tokio::test]
        async fn nothing_goes_over_plain_after_a_tls_failure() {
            use std::sync::atomic::{AtomicUsize, Ordering};
            use std::sync::Mutex;
            use tokio::io::AsyncReadExt;
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let addr = listener.local_addr().unwrap().to_string();
            let seen: Arc<Mutex<Vec<u8>>> = Arc::default();
            let conns = Arc::new(AtomicUsize::new(0));
            let (seen2, conns2) = (seen.clone(), conns.clone());
            tokio::spawn(async move {
                loop {
                    let (mut stream, _) = listener.accept().await.unwrap();
                    conns2.fetch_add(1, Ordering::SeqCst);
                    let seen = seen2.clone();
                    tokio::spawn(async move {
                        let mut buf = [0u8; 4096];
                        if let Ok(n) = stream.read(&mut buf).await {
                            seen.lock().unwrap().extend_from_slice(&buf[..n]);
                        }
                    });
                }
            });
            let err = connect(&addr, "/ws/server", "secret-pw", "name=srv", false).await.err().unwrap();
            assert!(matches!(err, ConnectError::NotSecure(_)), "{:?}", err);
            assert!(err.to_string().contains("update that agent"));
            tokio::time::sleep(Duration::from_millis(300)).await;
            assert_eq!(conns.load(Ordering::SeqCst), 1);
            let text = String::from_utf8_lossy(&seen.lock().unwrap()).to_string();
            assert!(!text.contains("token=") && !text.contains("secret-pw") && !text.contains("GET "), "{}", text);
        }
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

#[cfg(test)]
mod pin_tests {
    use super::*;

    #[test]
    fn a_pinned_certificate_must_match() {
        let a = "ab".repeat(32);
        let b = "cd".repeat(32);
        assert!(certificate_ok(None, &a), "not pinned yet: pinned on this connection");
        assert!(certificate_ok(Some(&a), &a));
        assert!(certificate_ok(Some(&a.to_uppercase()), &a));
        assert!(!certificate_ok(Some(&a), &b));
    }

    /// Rows from before the pin pin on their next connection; an admin's
    /// key reset (by host id or by "address:") clears the pin.
    #[tokio::test]
    async fn reset_key_clears_the_pin() {
        let db = crate::security::tests::test_db().await;
        let state = Arc::new(SignalingState::new(db, "t".into(), None));
        let pin = "ab".repeat(32);
        sqlx::query("INSERT INTO address_hosts (address, password, host_id, name, cert_sha256) VALUES ('192.0.2.50:8091', '', 'h-link', '', ?)")
            .bind(&pin)
            .execute(&state.db)
            .await
            .unwrap();
        sqlx::query("INSERT INTO address_hosts (address, password, host_id, name) VALUES ('192.0.2.51:8091', '', NULL, '')")
            .execute(&state.db)
            .await
            .unwrap();
        assert_eq!(pinned_certificate(&state, "192.0.2.50:8091").await.unwrap(), Some(pin.clone()));
        assert_eq!(pinned_certificate(&state, "192.0.2.51:8091").await.unwrap(), None);
        assert!(reset_pin(&state, "h-link").await.unwrap());
        assert_eq!(pinned_certificate(&state, "192.0.2.50:8091").await.unwrap(), None);
        sqlx::query("UPDATE address_hosts SET cert_sha256 = 'x'").execute(&state.db).await.unwrap();
        assert!(reset_pin(&state, "address:192.0.2.51:8091").await.unwrap());
        assert_eq!(pinned_certificate(&state, "192.0.2.51:8091").await.unwrap(), None);
        assert!(!reset_pin(&state, "not-a-link").await.unwrap());
        // Through the admin route too (the host has no agent key row).
        sqlx::query("UPDATE address_hosts SET cert_sha256 = 'x'").execute(&state.db).await.unwrap();
        let admin = crate::auth::AdminUser { user_id: "a".into(), username: "a".into() };
        let r = crate::admin::reset_host_key(admin, axum::extract::State(state.clone()), axum::extract::Path("h-link".into())).await;
        assert!(r.is_ok());
        assert_eq!(pinned_certificate(&state, "192.0.2.50:8091").await.unwrap(), None);
        state.links.stop("192.0.2.50:8091");
        state.links.stop("192.0.2.51:8091");
    }
}

/// A fake agent behind TLS speaking v2 (or only v1, like agents before it),
/// with a password, then registering like `/ws/server` of direct.rs.
#[cfg(test)]
mod fake_agent {
    use super::direct_auth::*;
    use futures_util::{SinkExt, StreamExt};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;
    use tokio_rustls::rustls;
    use tokio_rustls::rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
    use tokio_tungstenite::tungstenite::Message as WsMessage;

    pub struct Fake {
        pub addr: String,
        pub cert_hash: Digest,
        v1_proofs: Arc<AtomicUsize>,
        confirms: Arc<AtomicUsize>,
    }

    impl Fake {
        /// v1 proofs with a MAC it received.
        pub fn proofs(&self) -> usize {
            self.v1_proofs.load(Ordering::SeqCst)
        }
        /// v2 client confirmations with a MAC it received.
        pub fn client_confirms(&self) -> usize {
            self.confirms.load(Ordering::SeqCst)
        }
    }

    pub async fn start(password: &'static str, v2: bool, register: Option<&'static str>) -> Fake {
        let made = rcgen::generate_simple_self_signed(vec!["lunaris-agent".to_string()]).unwrap();
        let cert: CertificateDer<'static> = made.cert.der().clone();
        let cert_hash = sha256(&cert);
        let key = PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(made.signing_key.serialize_der()));
        let config = rustls::ServerConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
            .with_safe_default_protocol_versions()
            .unwrap()
            .with_no_client_auth()
            .with_single_cert(vec![cert], key)
            .unwrap();
        let acceptor = tokio_rustls::TlsAcceptor::from(Arc::new(config));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap().to_string();
        let (v1_proofs, confirms) = (Arc::new(AtomicUsize::new(0)), Arc::new(AtomicUsize::new(0)));
        let (p, c) = (v1_proofs.clone(), confirms.clone());
        tokio::spawn(async move {
            loop {
                let (stream, _) = listener.accept().await.unwrap();
                let (acceptor, p, c) = (acceptor.clone(), p.clone(), c.clone());
                tokio::spawn(async move {
                    let Ok(tls) = acceptor.accept(stream).await else { return };
                    let Ok(mut ws) = tokio_tungstenite::accept_async(tls).await else { return };
                    let let_in = serve(&mut ws, password, &cert_hash, v2, &p, &c).await;
                    if let (Some(true), Some(id)) = (let_in, register) {
                        let reg = common::AgentMessage::Register { id: id.into(), name: "Fake".into(), codec_support: None };
                        let _ = ws.send(WsMessage::Text(serde_json::to_string(&reg).unwrap().into())).await;
                        // Keep the link up a moment, then end it.
                        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
                    }
                    let _ = ws.close(None).await;
                });
            }
        });
        Fake { addr, cert_hash, v1_proofs, confirms }
    }

    async fn send<S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin>(
        ws: &mut tokio_tungstenite::WebSocketStream<S>,
        m: &AuthMessage,
    ) -> Option<()> {
        ws.send(WsMessage::Text(serde_json::to_string(m).unwrap().into())).await.ok()
    }

    async fn recv<S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin>(
        ws: &mut tokio_tungstenite::WebSocketStream<S>,
    ) -> Option<AuthMessage> {
        loop {
            match ws.next().await? {
                Ok(WsMessage::Text(t)) => return serde_json::from_str(&t).ok(),
                Ok(WsMessage::Ping(_) | WsMessage::Pong(_)) => continue,
                _ => return None,
            }
        }
    }

    /// The agent's side; Some(let in).
    async fn serve<S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin>(
        ws: &mut tokio_tungstenite::WebSocketStream<S>,
        password: &str,
        cert_hash: &Digest,
        v2: bool,
        v1_proofs: &AtomicUsize,
        confirms: &AtomicUsize,
    ) -> Option<bool> {
        let Some(AuthMessage::Hello { .. }) = recv(ws).await else { return None };
        let mut agent_nonce = [0u8; 32];
        agent_nonce[0] = 9;
        let versions = if v2 { vec![VERSION_PAKE] } else { vec![] };
        send(ws, &AuthMessage::Challenge { nonce: to_hex(&agent_nonce), versions }).await?;
        match recv(ws).await? {
            AuthMessage::Proof { client_nonce, mac } => {
                let cn = from_hex::<32>(&client_nonce)?;
                let Some(tag) = mac.as_deref().and_then(from_hex::<32>) else {
                    send(ws, &AuthMessage::Ok { trusted: false, mac: None }).await?;
                    return Some(true);
                };
                v1_proofs.fetch_add(1, Ordering::SeqCst);
                if mac_ok(CLIENT_LABEL, password, &agent_nonce, &cn, cert_hash, &tag) {
                    let tag = Some(to_hex(&super::direct_auth::mac(AGENT_LABEL, password, &agent_nonce, &cn, cert_hash)));
                    send(ws, &AuthMessage::Ok { trusted: true, mac: tag }).await?;
                    Some(true)
                } else {
                    send(ws, &AuthMessage::Refused { reason: "unauthorized".into() }).await?;
                    Some(false)
                }
            }
            AuthMessage::Pake { client_nonce, msg } if v2 => {
                let (cn, x) = (from_hex::<32>(&client_nonce)?, from_hex::<PAKE_MSG_LEN>(&msg)?);
                let ctx = v2_context(&agent_nonce, &cn, cert_hash);
                let (b, y) = pake_agent(password, &ctx, OsRng);
                let key = b.finish(&x).ok()?;
                let (ac, cc) = confirmations(&key, &ctx, &x, &y);
                send(ws, &AuthMessage::PakeReply { msg: to_hex(&y), confirm: to_hex(&ac) }).await?;
                let Some(AuthMessage::Confirm { mac }) = recv(ws).await else { return None };
                if mac.is_some() {
                    confirms.fetch_add(1, Ordering::SeqCst);
                }
                let right = mac.as_deref().and_then(from_hex::<32>) == Some(cc);
                let answer = if right {
                    AuthMessage::Ok { trusted: true, mac: None }
                } else {
                    AuthMessage::Refused { reason: "unauthorized".into() }
                };
                send(ws, &answer).await?;
                Some(right)
            }
            _ => None,
        }
    }
}

/// `connect_once` as the server runs it for computers added by address: v2
/// is remembered, a later v1-only answer refused like another certificate,
/// and a key reset forgets both.
#[cfg(test)]
mod downgrade_tests {
    use super::*;

    async fn once(state: &Arc<SignalingState>, address: &str) -> Result<(), (String, bool)> {
        once_with(state, address, "pw").await
    }

    async fn once_with(state: &Arc<SignalingState>, address: &str, password: &str) -> Result<(), (String, bool)> {
        let (error, host_id) = (Arc::new(Mutex::new(None)), Arc::new(Mutex::new(None)));
        connect_once(state, address, password, &error, &host_id, ADDED_BY_ADDRESS).await
    }

    async fn row(state: &SignalingState, address: &str, password: &str, pin: Option<&str>, pake: Option<i64>) {
        sqlx::query("INSERT INTO address_hosts (address, password, host_id, name, cert_sha256, pake_version) VALUES (?, ?, NULL, '', ?, ?)")
            .bind(address)
            .bind(password)
            .bind(pin)
            .bind(pake)
            .execute(&state.db)
            .await
            .unwrap();
    }

    /// Older agents make a new certificate at each start: their pin is not
    /// held (it would refuse them after every restart); the first answer
    /// with v2 pins the certificate, which then holds.
    #[tokio::test]
    async fn only_agents_that_keep_their_certificate_are_held_to_the_pin() {
        let db = crate::security::tests::test_db().await;
        let state = Arc::new(SignalingState::new(db, "t".into(), None));
        // An old agent, a pin from an earlier start of it: still let in.
        let old = fake_agent::start("pw", false, Some("old")).await;
        row(&state, &old.addr, "pw", Some("00"), None).await;
        assert_eq!(once(&state, &old.addr).await, Ok(()));
        assert_eq!(pake_version(&state, &old.addr).await, None);
        // The same computer after its update: its kept certificate replaces
        // the old pin, and it is marked.
        let updated = fake_agent::start("pw", true, Some("updated")).await;
        row(&state, &updated.addr, "pw", Some("00"), None).await;
        assert_eq!(once(&state, &updated.addr).await, Ok(()));
        assert_eq!(pinned_certificate(&state, &updated.addr).await.unwrap(), Some(direct_auth::to_hex(&updated.cert_hash)));
        assert_eq!(pake_version(&state, &updated.addr).await, Some(2));
        // Marked, another certificate: refused.
        let other = fake_agent::start("pw", true, Some("other")).await;
        row(&state, &other.addr, "pw", Some("00"), Some(2)).await;
        assert_eq!(once(&state, &other.addr).await, Err((CERT_CHANGED.to_string(), true)));
    }

    /// Without a password nothing but the pin tells the computer apart, so
    /// someone at a marked address can't pass as an older agent.
    #[tokio::test]
    async fn without_a_password_a_marked_address_must_still_offer_v2() {
        let db = crate::security::tests::test_db().await;
        let state = Arc::new(SignalingState::new(db, "t".into(), None));
        let impostor = fake_agent::start("", false, Some("impostor")).await;
        row(&state, &impostor.addr, "", None, Some(2)).await;
        assert_eq!(once_with(&state, &impostor.addr, "").await, Err((DOWNGRADED.to_string(), true)));
        // Unmarked (an older agent there): allowed, nothing pinned.
        let old = fake_agent::start("", false, Some("old-nopw")).await;
        row(&state, &old.addr, "", None, None).await;
        assert_eq!(once_with(&state, &old.addr, "").await, Ok(()));
        assert_eq!(pinned_certificate(&state, &old.addr).await.unwrap(), None);
    }

    async fn pake_version(state: &SignalingState, address: &str) -> Option<i64> {
        sqlx::query_scalar("SELECT pake_version FROM address_hosts WHERE address = ?")
            .bind(address)
            .fetch_one(&state.db)
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn v2_is_remembered_and_a_downgrade_refused_until_a_key_reset() {
        let db = crate::security::tests::test_db().await;
        let state = Arc::new(SignalingState::new(db, "t".into(), None));
        let v2 = fake_agent::start("pw", true, Some("fake-v2")).await;
        let v1 = fake_agent::start("pw", false, Some("fake-v1")).await;
        for addr in [&v2.addr, &v1.addr] {
            sqlx::query("INSERT INTO address_hosts (address, password, host_id, name) VALUES (?, 'pw', NULL, '')")
                .bind(addr)
                .execute(&state.db)
                .await
                .unwrap();
        }
        // A new agent: v2, remembered with the certificate pin.
        assert_eq!(once(&state, &v2.addr).await, Ok(()));
        assert_eq!(pake_version(&state, &v2.addr).await, Some(2));
        assert_eq!(pinned_certificate(&state, &v2.addr).await.unwrap(), Some(direct_auth::to_hex(&v2.cert_hash)));
        assert!(pake_seen(&state, &v2.addr).await.unwrap());
        // An old agent at first contact: allowed (with a warning), not remembered.
        assert_eq!(once(&state, &v1.addr).await, Ok(()));
        assert_eq!(v1.proofs(), 1);
        assert_eq!(pake_version(&state, &v1.addr).await, None);
        // As if the v1 answer came where v2 was seen: refused, no proof sent.
        sqlx::query("UPDATE address_hosts SET pake_version = 2, cert_sha256 = NULL WHERE address = ?")
            .bind(&v1.addr)
            .execute(&state.db)
            .await
            .unwrap();
        assert_eq!(once(&state, &v1.addr).await, Err((DOWNGRADED.to_string(), true)));
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert_eq!(v1.proofs(), 1);
        // The admin's key reset forgets it with the pin.
        assert!(reset_pin(&state, &format!("address:{}", v1.addr)).await.unwrap());
        state.links.stop(&v1.addr);
        assert_eq!(pake_version(&state, &v1.addr).await, None);
        assert!(!pake_seen(&state, &v1.addr).await.unwrap());
    }
}
