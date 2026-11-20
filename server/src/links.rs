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
    let (ws, local_ip) = match direct_auth::connect(address, "/ws/server", password, &name_query).await {
        Ok(c) => (c.ws, c.local_ip),
        Err(e) => {
            // Also slower when TLS fails or the agent can't prove itself:
            // retrying fast won't change it.
            use direct_auth::ConnectError as E;
            let refused = matches!(e, E::WrongPassword | E::TooManyAttempts | E::NotSecure(_) | E::NotProven);
            return Err((e.to_string(), refused));
        }
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
/// and the desktop app's copy are agent/src/direct/handshake.rs; the test
/// vectors below are the same there, so the two cannot drift apart).
///
/// TLS first, accepting the agent's self-signed certificate: there is
/// nothing to check it against. What stops a man in the middle is the
/// password proof, bound to the certificate seen here: HMAC-SHA256(password,
/// label || agent nonce || client nonce || SHA-256 of the certificate), sent
/// by us, and the agent's answer over another label, checked here (the
/// `tls-server-end-point` channel binding of SCRAM, RFC 5929). The password
/// itself never crosses the network. With a password, only an answer with
/// the agent's valid MAC is accepted. Older agents don't speak TLS: no
/// connection then (no plain fallback that would expose the password).
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
        Challenge { nonce: String },
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
    /// (never the password).
    pub async fn connect(address: &str, path: &str, password: &str, query: &str) -> Result<Connected, ConnectError> {
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
        let _trusted = match tokio::time::timeout(STEP_TIMEOUT, client_auth(&mut ws, password, &cert_hash)).await {
            Ok(r) => r?,
            Err(_) => return Err(ConnectError::Protocol("no answer to the password proof".into())),
        };
        Ok(Connected { ws, local_ip })
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

    /// Ok(trusted): false when the person at the computer will be asked.
    async fn client_auth(ws: &mut WebSocketStream<Box<dyn Io>>, password: &str, cert_hash: &Digest) -> Result<bool, ConnectError> {
        send_auth(ws, &AuthMessage::Hello { version: 1 }).await?;
        let agent_nonce = match recv_auth(ws).await {
            Some(AuthMessage::Challenge { nonce }) => from_hex::<32>(&nonce),
            Some(AuthMessage::Refused { reason }) => return Err(refusal(&reason)),
            _ => None,
        }
        .ok_or_else(|| ConnectError::Protocol("unexpected answer (not a Lunaris agent?)".into()))?;
        let mut client_nonce = [0u8; 32];
        SystemRandom::new()
            .fill(&mut client_nonce)
            .map_err(|_| ConnectError::Protocol("no random numbers".into()))?;
        let has_password = !password.trim().is_empty();
        let mac = has_password.then(|| to_hex(&mac(CLIENT_LABEL, password, &agent_nonce, &client_nonce, cert_hash)));
        send_auth(ws, &AuthMessage::Proof { client_nonce: to_hex(&client_nonce), mac }).await?;
        judge_answer(recv_auth(ws).await, password, &agent_nonce, &client_nonce, cert_hash)
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
            assert_eq!(json(&AuthMessage::Challenge { nonce: "ab".into() }), r#"{"auth":"challenge","nonce":"ab"}"#);
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
            let err = connect(&addr, "/ws/server", "secret-pw", "name=srv").await.err().unwrap();
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
