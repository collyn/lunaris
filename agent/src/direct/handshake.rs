//! The direct-connection handshake, both sides (the desktop app uses the
//! client side through this crate; the server has its own copy in
//! server/src/links.rs, kept in step by the same test vectors).
//!
//! 1. TLS on the direct port with a certificate the agent makes at start.
//!    Nothing can vouch for it, so the client accepts any certificate (it
//!    still checks the handshake signature, so the peer owns that
//!    certificate's key) and relies on step 3 instead.
//! 2. WebSocket over it, without the password in the URL.
//! 3. Client: `hello`; agent: `challenge` with a random nonce and the
//!    versions it speaks (`versions`, absent from agents before v2).
//!
//! v2 (both sides new, a password given), a balanced PAKE: SPAKE2 over
//! Ed25519 (RustCrypto `spake2`), client in role A, agent in role B. Both
//! identities carry `context` = SHA-256(V2_LABEL || agent nonce || client
//! nonce || SHA-256 of the agent's certificate as the client saw it), so the
//! key is bound to this TLS channel (RFC 5929 `tls-server-end-point`) and to
//! this run.
//!    - client: `pake` = its nonce and SPAKE2 message X;
//!    - agent (unless locked out; the attempt is counted as a failure in
//!      advance, see `Gate`): `pake_reply` = its message Y and its key
//!      confirmation HMAC(K, AGENT_CONFIRM_LABEL || context || X || Y): the
//!      agent proves first;
//!    - client: checks it; `confirm` with HMAC(K, CLIENT_CONFIRM_LABEL || ...)
//!      when right, without a MAC when not (then the password is wrong or the
//!      peer is not that computer: it is never told anything that depends on
//!      the password beyond X);
//!    - agent: `ok` (`trusted`) or `refused`.
//!
//!    X and Y reveal nothing about the password; each confirmation can only
//!    be checked against one guess per run, so someone answering at the
//!    address the client dials (ARP spoofing, a reused address) gets one
//!    online guess per connection, never an offline test.
//!
//! v1 (an old agent or an old client): `proof` = client nonce and
//! HMAC-SHA256(password, CLIENT_LABEL || agent nonce || client nonce ||
//! certificate hash); agent: `ok` with HMAC over AGENT_LABEL and the same
//! data, or `refused`. The client's MAC is an offline-testable verifier for
//! whoever answers, so new clients send it only to agents that don't offer
//! v2 (with a warning); the agent still takes it from old clients.
//!
//! Without a password (the person at the computer accepts) there is no
//! binding: `proof` without a MAC, in both versions; TLS still keeps
//! passive listeners out.
//!
//! A client with a password goes on only after the agent proved it knows it
//! (its confirmation, v2, or its MAC, v1), whatever else the answer says.
//! Older agents don't speak TLS: the client then gives up (there is no plain
//! fallback an attacker could force, the password never goes in a URL); the
//! agent still takes old clients over plain ws:// with `?token=`.

use std::net::IpAddr;
use std::sync::Arc;
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use ring::hmac;
use ring::rand::{SecureRandom, SystemRandom};
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::crypto::CryptoProvider;
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer, ServerName, UnixTime};
use rustls::{DigitallySignedStruct, SignatureScheme};
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::net::TcpStream;
use tokio_tungstenite::tungstenite::{self, Message as WsMessage};
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};
use tracing::{info, warn};

pub const CLIENT_LABEL: &[u8] = b"lunaris-direct-auth-v1 client";
pub const AGENT_LABEL: &[u8] = b"lunaris-direct-auth-v1 agent";
pub const NONCE_LEN: usize = 32;
/// The v2 handshake (SPAKE2), as `hello`/`challenge` advertise it.
pub const VERSION_PAKE: u32 = 2;
pub const V2_LABEL: &[u8] = b"lunaris-direct-auth-v2";
const PAKE_CLIENT_ID: &[u8] = b"lunaris-direct-auth-v2 client";
const PAKE_AGENT_ID: &[u8] = b"lunaris-direct-auth-v2 agent";
const CLIENT_CONFIRM_LABEL: &[u8] = b"lunaris-direct-auth-v2 client confirm";
const AGENT_CONFIRM_LABEL: &[u8] = b"lunaris-direct-auth-v2 agent confirm";
/// A SPAKE2 message: the side byte and a compressed Ed25519 point.
pub const PAKE_MSG_LEN: usize = 33;
pub type PakeMsg = [u8; PAKE_MSG_LEN];
pub type Nonce = [u8; NONCE_LEN];
pub type Digest = [u8; 32];

/// Each step of a connection attempt (TCP, TLS, WebSocket, proof).
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

fn mac(label: &[u8], password: &str, agent_nonce: &Nonce, client_nonce: &Nonce, cert_hash: &Digest) -> Digest {
    let tag = hmac::sign(&mac_key(password), &mac_input(label, agent_nonce, client_nonce, cert_hash));
    tag.as_ref().try_into().expect("HMAC-SHA256 is 32 bytes")
}

/// The client's proof that it knows the password.
pub fn client_mac(password: &str, agent_nonce: &Nonce, client_nonce: &Nonce, cert_hash: &Digest) -> Digest {
    mac(CLIENT_LABEL, password, agent_nonce, client_nonce, cert_hash)
}

/// The agent's answer, proving it knows the password too.
pub fn agent_mac(password: &str, agent_nonce: &Nonce, client_nonce: &Nonce, cert_hash: &Digest) -> Digest {
    mac(AGENT_LABEL, password, agent_nonce, client_nonce, cert_hash)
}

/// Checks a received MAC in constant time (ring's HMAC verify).
fn mac_ok(label: &[u8], password: &str, agent_nonce: &Nonce, client_nonce: &Nonce, cert_hash: &Digest, tag: &[u8]) -> bool {
    hmac::verify(&mac_key(password), &mac_input(label, agent_nonce, client_nonce, cert_hash), tag).is_ok()
}

/// Constant-time comparison of two fixed-length digests.
pub fn digest_eq(a: &Digest, b: &Digest) -> bool {
    a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

// ---- v2: SPAKE2 ----

/// What both PAKE identities carry: this run (both nonces) and this TLS
/// channel (the certificate hash).
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

pub type Pake = spake2::Spake2<spake2::Ed25519Group>;

/// The client's side (role A): its state and message X.
pub fn pake_client(password: &str, context: &Digest, rng: impl rand_core::CryptoRng) -> (Pake, PakeMsg) {
    let (a, b) = pake_ids(context);
    let (state, msg) = Pake::start_a_with_rng(&spake2::Password::new(password.trim().as_bytes()), &a, &b, rng);
    (state, msg.try_into().expect("SPAKE2 Ed25519 messages are 33 bytes"))
}

/// The agent's side (role B): its state and message Y.
pub fn pake_agent(password: &str, context: &Digest, rng: impl rand_core::CryptoRng) -> (Pake, PakeMsg) {
    let (a, b) = pake_ids(context);
    let (state, msg) = Pake::start_b_with_rng(&spake2::Password::new(password.trim().as_bytes()), &a, &b, rng);
    (state, msg.try_into().expect("SPAKE2 Ed25519 messages are 33 bytes"))
}

/// The two key confirmations of a run: (agent's, client's).
pub fn confirmations(key: &[u8], context: &Digest, client_msg: &PakeMsg, agent_msg: &PakeMsg) -> (Digest, Digest) {
    let key = hmac::Key::new(hmac::HMAC_SHA256, key);
    let tag = |label: &[u8]| -> Digest {
        let input = [label, &context[..], &client_msg[..], &agent_msg[..]].concat();
        hmac::sign(&key, &input).as_ref().try_into().expect("HMAC-SHA256 is 32 bytes")
    };
    (tag(AGENT_CONFIRM_LABEL), tag(CLIENT_CONFIRM_LABEL))
}

/// The confirmations once the peer's message came; None when it is not a
/// valid SPAKE2 message of the other role.
fn finish(state: Pake, context: &Digest, client_msg: &PakeMsg, agent_msg: &PakeMsg, peer_msg: &PakeMsg) -> Option<(Digest, Digest)> {
    let key = state.finish(peer_msg).ok()?;
    Some(confirmations(&key, context, client_msg, agent_msg))
}

pub fn random_nonce() -> Nonce {
    let mut nonce = [0u8; NONCE_LEN];
    SystemRandom::new().fill(&mut nonce).expect("the OS random source failed");
    nonce
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

/// Handshake messages (JSON text frames, before any `ClientMessage`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "auth", rename_all = "snake_case")]
pub enum AuthMessage {
    /// `version`: the newest the client speaks (old agents ignore it).
    Hello { version: u32 },
    Challenge {
        nonce: String,
        /// The handshake versions this agent speaks besides v1 (absent from
        /// agents before v2; old clients ignore it).
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        versions: Vec<u32>,
    },
    Proof {
        client_nonce: String,
        /// Absent without a password (asks the person at the computer).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        mac: Option<String>,
    },
    Ok {
        /// The password was right; `mac` then proves the agent knows it.
        trusted: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        mac: Option<String>,
    },
    /// "unauthorized" or "too_many_attempts".
    Refused { reason: String },
    /// v2, client: its nonce and SPAKE2 message (hex).
    Pake { client_nonce: String, msg: String },
    /// v2, agent: its SPAKE2 message and key confirmation (hex).
    PakeReply { msg: String, confirm: String },
    /// v2, client: its key confirmation; absent when the agent's did not
    /// check out.
    Confirm {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        mac: Option<String>,
    },
}

pub const REFUSED_UNAUTHORIZED: &str = "unauthorized";
pub const REFUSED_TOO_MANY: &str = "too_many_attempts";

async fn send_auth<S: AsyncRead + AsyncWrite + Unpin>(ws: &mut WebSocketStream<S>, msg: &AuthMessage) -> Result<(), tungstenite::Error> {
    let json = serde_json::to_string(msg).expect("handshake messages serialize");
    ws.send(WsMessage::Text(json.into())).await
}

/// The next handshake message; None when the peer closed or sent something else.
async fn recv_auth<S: AsyncRead + AsyncWrite + Unpin>(ws: &mut WebSocketStream<S>) -> Option<AuthMessage> {
    loop {
        match ws.next().await? {
            Ok(WsMessage::Text(text)) => return serde_json::from_str(&text).ok(),
            Ok(WsMessage::Ping(_) | WsMessage::Pong(_)) => continue,
            _ => return None,
        }
    }
}

// ---- Agent side ----

/// What a peer showed of the password.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Presented {
    Nothing,
    Right,
    Wrong,
}

/// TLS of the direct listener: a self-signed certificate, kept across
/// starts (`load_or_make_tls_server`) or made fresh (`make_tls_server`).
pub struct TlsServer {
    pub acceptor: tokio_rustls::TlsAcceptor,
    pub cert_hash: Digest,
}

pub fn make_tls_server() -> anyhow::Result<TlsServer> {
    let made = rcgen::generate_simple_self_signed(vec!["lunaris-agent".to_string()])?;
    let cert: CertificateDer<'static> = made.cert.der().clone();
    let cert_hash = sha256(&cert);
    let key = PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(made.signing_key.serialize_der()));
    let config = rustls::ServerConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
        .with_safe_default_protocol_versions()?
        .with_no_client_auth()
        .with_single_cert(vec![cert], key)?;
    Ok(TlsServer { acceptor: tokio_rustls::TlsAcceptor::from(Arc::new(config)), cert_hash })
}

/// Files of the listener's kept certificate, next to the agent's config.
pub const TLS_CERT_FILE: &str = "direct_tls_cert.der";
pub const TLS_KEY_FILE: &str = "direct_tls_key.der";

fn tls_server_from(cert: Vec<u8>, key: Vec<u8>) -> anyhow::Result<TlsServer> {
    let cert = CertificateDer::from(cert);
    let cert_hash = sha256(&cert);
    let key = PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(key));
    let config = rustls::ServerConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
        .with_safe_default_protocol_versions()?
        .with_no_client_auth()
        .with_single_cert(vec![cert], key)?;
    Ok(TlsServer { acceptor: tokio_rustls::TlsAcceptor::from(Arc::new(config)), cert_hash })
}

/// The listener's certificate kept in `dir` (made and saved there the first
/// time): servers that added this computer by address pin it, so it must
/// not change when the agent or its listener starts again. A pair that
/// can't be read or used is replaced (those servers then need a key reset);
/// one that can't be saved is used for this start only.
pub fn load_or_make_tls_server(dir: &std::path::Path) -> anyhow::Result<TlsServer> {
    let (cert_path, key_path) = (dir.join(TLS_CERT_FILE), dir.join(TLS_KEY_FILE));
    match (std::fs::read(&cert_path), std::fs::read(&key_path)) {
        (Ok(cert), Ok(key)) => match tls_server_from(cert, key) {
            Ok(tls) => return Ok(tls),
            Err(e) => warn!("The kept certificate in {} can't be used ({}): making a new one", dir.display(), e),
        },
        (Err(e), _) | (_, Err(e)) if e.kind() != std::io::ErrorKind::NotFound => {
            warn!("The kept certificate in {} can't be read ({}): making a new one", dir.display(), e)
        }
        _ => {}
    }
    let made = rcgen::generate_simple_self_signed(vec!["lunaris-agent".to_string()])?;
    let (cert, key) = (made.cert.der().to_vec(), made.signing_key.serialize_der());
    // The key first: a certificate is never kept without its key.
    let saved = crate::pairing::write_private(&key_path, &key).and_then(|()| crate::pairing::write_private(&cert_path, &cert));
    match saved {
        Ok(()) => info!("Made the direct listener's certificate, kept in {}", dir.display()),
        Err(e) => warn!(
            "The direct listener's certificate could not be kept in {} ({}): a new one comes at each start, and servers that \
             added this computer by address will need a key reset after it",
            dir.display(),
            e
        ),
    }
    tls_server_from(cert, key)
}

/// How the listener judges an attempt (direct.rs, around its lockout).
pub(super) trait Gate {
    /// v2, before the agent's PAKE message, which lets whoever is on the
    /// other end test one guess: false when the address is locked out;
    /// otherwise the attempt counts as a failure in advance, so a client
    /// that hangs up (or several at once) can't guess for free. `decide`
    /// settles it.
    fn charge(&mut self) -> bool;
    /// The admission for what the client showed (failures counted);
    /// `charged`: `charge` counted one for this attempt already.
    fn decide(&mut self, presented: Presented, charged: bool) -> super::Admission;
}

/// The answer that ends the handshake; `mac` only in v1.
fn final_answer(admission: &super::Admission, v1_mac: impl FnOnce(bool) -> Option<String>) -> AuthMessage {
    use super::Admission;
    match admission {
        Admission::Accept { trusted, .. } => AuthMessage::Ok { trusted: *trusted, mac: v1_mac(*trusted) },
        Admission::TooManyAttempts => AuthMessage::Refused { reason: REFUSED_TOO_MANY.into() },
        Admission::Unauthorized | Admission::NotFound => AuthMessage::Refused { reason: REFUSED_UNAUTHORIZED.into() },
    }
}

/// Runs the agent side of step 3: the admission and the version the client
/// used (1 or 2). Failures are counted through `gate`; the client is told
/// the outcome.
pub(super) async fn agent_auth<S: AsyncRead + AsyncWrite + Unpin>(
    ws: &mut WebSocketStream<S>,
    password: &str,
    cert_hash: &Digest,
    gate: &mut impl Gate,
) -> anyhow::Result<(super::Admission, u32)> {
    use super::Admission;
    match recv_auth(ws).await {
        Some(AuthMessage::Hello { .. }) => {}
        other => anyhow::bail!("expected hello, got {:?}", other),
    }
    let agent_nonce = random_nonce();
    send_auth(ws, &AuthMessage::Challenge { nonce: to_hex(&agent_nonce), versions: vec![VERSION_PAKE] }).await?;
    match recv_auth(ws).await {
        Some(AuthMessage::Proof { client_nonce, mac }) => {
            let Some(client_nonce) = from_hex::<NONCE_LEN>(&client_nonce) else {
                anyhow::bail!("bad client nonce");
            };
            let presented = match mac.as_deref() {
                None | Some("") => Presented::Nothing,
                Some(_) if password.trim().is_empty() => Presented::Wrong,
                Some(tag) => {
                    let tag = from_hex::<32>(tag).unwrap_or([0; 32]);
                    if mac_ok(CLIENT_LABEL, password, &agent_nonce, &client_nonce, cert_hash, &tag) {
                        Presented::Right
                    } else {
                        Presented::Wrong
                    }
                }
            };
            let admission = gate.decide(presented, false);
            // Click mode with the right password: still asked here, but the
            // client, which holds a password, needs our MAC to go on.
            let answer = final_answer(&admission, |_| {
                (presented == Presented::Right).then(|| to_hex(&agent_mac(password, &agent_nonce, &client_nonce, cert_hash)))
            });
            send_auth(ws, &answer).await?;
            Ok((admission, 1))
        }
        Some(AuthMessage::Pake { client_nonce, msg }) => {
            let (Some(client_nonce), Some(client_msg)) = (from_hex::<NONCE_LEN>(&client_nonce), from_hex::<PAKE_MSG_LEN>(&msg)) else {
                anyhow::bail!("bad PAKE message");
            };
            if password.trim().is_empty() {
                // Nothing to run the PAKE with: a password here is wrong.
                let admission = gate.decide(Presented::Wrong, false);
                send_auth(ws, &final_answer(&admission, |_| None)).await?;
                return Ok((admission, VERSION_PAKE));
            }
            if !gate.charge() {
                send_auth(ws, &AuthMessage::Refused { reason: REFUSED_TOO_MANY.into() }).await?;
                return Ok((Admission::TooManyAttempts, VERSION_PAKE));
            }
            let context = v2_context(&agent_nonce, &client_nonce, cert_hash);
            let (state, agent_msg) = pake_agent(password, &context, OsRng);
            let Some((agent_confirm, client_confirm)) = finish(state, &context, &client_msg, &agent_msg, &client_msg) else {
                // Not a valid role-A message: like a wrong password.
                let admission = gate.decide(Presented::Wrong, true);
                send_auth(ws, &final_answer(&admission, |_| None)).await?;
                return Ok((admission, VERSION_PAKE));
            };
            send_auth(ws, &AuthMessage::PakeReply { msg: to_hex(&agent_msg), confirm: to_hex(&agent_confirm) }).await?;
            // Anything but `confirm` (a hang-up, a timeout): the failure
            // counted by `charge` stays.
            let presented = match recv_auth(ws).await {
                Some(AuthMessage::Confirm { mac }) => {
                    let tag = mac.as_deref().and_then(from_hex::<32>);
                    if tag.is_some_and(|t| digest_eq(&t, &client_confirm)) { Presented::Right } else { Presented::Wrong }
                }
                other => anyhow::bail!("expected confirm, got {:?}", other),
            };
            let admission = gate.decide(presented, true);
            send_auth(ws, &final_answer(&admission, |_| None)).await?;
            Ok((admission, VERSION_PAKE))
        }
        other => anyhow::bail!("expected proof, got {:?}", other),
    }
}

// ---- Client side ----

/// Accepts the agent's self-signed certificate: there is nothing to check it
/// against. The password proof is bound to this certificate's hash, which is
/// what stops a man in the middle. Handshake signatures are still verified,
/// so the peer must own the key of the certificate it shows.
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

fn client_tls_config() -> Result<Arc<rustls::ClientConfig>, rustls::Error> {
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let config = rustls::ClientConfig::builder_with_provider(provider.clone())
        .with_safe_default_protocol_versions()?
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(AcceptAnyCert(provider)))
        .with_no_client_auth();
    Ok(Arc::new(config))
}

/// The host part of "host:port" / "[v6]:port", for SNI (not checked).
fn server_name(address: &str) -> ServerName<'static> {
    let host = if let Some(rest) = address.strip_prefix('[') {
        rest.split(']').next().unwrap_or("")
    } else {
        address.rsplit_once(':').map(|(h, _)| h).unwrap_or(address)
    };
    ServerName::try_from(host.to_string())
        .unwrap_or_else(|_| ServerName::try_from("lunaris-agent").expect("valid DNS name"))
}

/// A direct connection, handshake done (always TLS).
pub struct Connected {
    pub ws: WebSocketStream<MaybeTlsStream<TcpStream>>,
    /// The password was accepted and the agent proved it knows it. False:
    /// connected without a password, the person at the computer will be asked.
    pub trusted: bool,
    /// This side's address of the connection.
    pub local_ip: Option<IpAddr>,
    /// The handshake used: VERSION_PAKE, or 1 with an older agent (or
    /// without a password, where nothing is proven either way).
    pub version: u32,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ConnectError {
    Unreachable(String),
    /// TLS failed: an agent from before secure direct connections, or
    /// someone in the way. The password is never sent without TLS.
    NotSecure(String),
    WrongPassword,
    /// With a password: the answer did not carry the agent's proof. Either
    /// the password is wrong, or the peer is not that computer.
    NotProven,
    TooManyAttempts,
    /// The agent offered only the old password proof (v1) where the new
    /// one (v2) was required: nothing was sent that depends on the password.
    Downgraded,
    /// No such path: not a Lunaris agent, or one too old.
    NotFound,
    Refused(String),
    Protocol(String),
}

/// Full sentences: the desktop app shows them as they are.
impl std::fmt::Display for ConnectError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ConnectError::Unreachable(e) => write!(f, "Could not reach the computer ({}).", e),
            ConnectError::NotSecure(e) => write!(
                f,
                "Could not connect to the computer securely (TLS failed: {}). If it runs a Lunaris agent \
                 older than {}, update that agent; the password is not sent without encryption.",
                e,
                env!("CARGO_PKG_VERSION")
            ),
            ConnectError::WrongPassword => write!(f, "Wrong password."),
            ConnectError::NotProven => {
                write!(f, "Wrong password, or this is not the computer you expect (it could not prove it knows the password).")
            }
            ConnectError::TooManyAttempts => write!(f, "Too many wrong passwords: try again in a minute."),
            ConnectError::Downgraded => write!(
                f,
                "The computer answered like an old Lunaris agent (without the secure password check) although it had \
                 the new one before: not connecting, someone else may be answering at that address."
            ),
            ConnectError::NotFound => write!(f, "That address is not a Lunaris agent, or its agent is too old (update it)."),
            ConnectError::Refused(e) => write!(f, "The computer refused the connection ({}).", e),
            ConnectError::Protocol(e) => write!(f, "The connection failed: {}.", e),
        }
    }
}

impl std::error::Error for ConnectError {}

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

/// Connects to an agent's direct port: `address` "host:port", `path`
/// "/ws/client" or "/ws/server", `query` extra URL parameters (e.g.
/// "name=..."), never the password. TLS only: when it fails there is no
/// plain fallback, which anyone on the network could force to read the
/// password.
pub async fn connect(address: &str, path: &str, password: &str, query: &str) -> Result<Connected, ConnectError> {
    connect_with(address, path, password, query, false).await
}

/// As `connect`; `require_pake`: with a password, refuse an agent that offers
/// only v1 (`ConnectError::Downgraded`) instead of sending it the v1 MAC.
pub async fn connect_with(
    address: &str,
    path: &str,
    password: &str,
    query: &str,
    require_pake: bool,
) -> Result<Connected, ConnectError> {
    let query = query.trim_start_matches(['&', '?']);
    let tcp = match tokio::time::timeout(STEP_TIMEOUT, TcpStream::connect(address)).await {
        Ok(Ok(tcp)) => tcp,
        Ok(Err(e)) => return Err(ConnectError::Unreachable(e.to_string())),
        Err(_) => return Err(ConnectError::Unreachable("timed out".into())),
    };
    let _ = tcp.set_nodelay(true);
    let local_ip = tcp.local_addr().ok().map(|a| a.ip());
    let tls_config = client_tls_config().map_err(|e| ConnectError::Protocol(e.to_string()))?;
    let connector = tokio_rustls::TlsConnector::from(tls_config);
    let tls = match tokio::time::timeout(STEP_TIMEOUT, connector.connect(server_name(address), tcp)).await {
        Ok(Ok(tls)) => tls,
        Ok(Err(e)) => {
            warn!("TLS with {} failed ({}): not connecting (an older agent?)", address, e);
            return Err(ConnectError::NotSecure(e.to_string()));
        }
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
    let url = format!("wss://{}{}{}{}", address, path, sep, query);
    let (mut ws, _) =
        match tokio::time::timeout(STEP_TIMEOUT, tokio_tungstenite::client_async(url, MaybeTlsStream::Rustls(tls))).await {
            Ok(r) => r.map_err(http_error)?,
            Err(_) => return Err(ConnectError::Unreachable("WebSocket: timed out".into())),
        };
    let (trusted, version) =
        match tokio::time::timeout(STEP_TIMEOUT, client_auth(&mut ws, password, &cert_hash, require_pake, address)).await {
            Ok(r) => r?,
            Err(_) => return Err(ConnectError::Protocol("no answer to the password proof".into())),
        };
    info!(
        "Connected to {} over TLS ({}, handshake v{})",
        address,
        if trusted { "password" } else { "to be accepted there" },
        version
    );
    Ok(Connected { ws, trusted, local_ip, version })
}

/// What the client makes of the agent's answer to its proof; Ok(trusted).
/// With a password, only an answer carrying the agent's valid MAC goes on:
/// anything else (including "ok, untrusted") could come from an impostor
/// with its own certificate.
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
        // No password: click-to-accept, nothing to prove either way.
        Some(AuthMessage::Ok { trusted: false, .. }) => Ok(false),
        Some(AuthMessage::Ok { trusted: true, .. }) => Err(ConnectError::NotProven),
        _ => Err(ConnectError::Protocol("unexpected answer to the password proof".into())),
    }
}

/// What the client makes of the agent's v2 `pake_reply`: whether its
/// confirmation checks out, and the client's own to send then.
fn judge_pake_reply(
    state: Pake,
    context: &Digest,
    client_msg: &PakeMsg,
    reply_msg: &str,
    reply_confirm: &str,
) -> Option<Digest> {
    let agent_msg = from_hex::<PAKE_MSG_LEN>(reply_msg)?;
    let (agent_confirm, client_confirm) = finish(state, context, client_msg, &agent_msg, &agent_msg)?;
    let tag = from_hex::<32>(reply_confirm)?;
    digest_eq(&tag, &agent_confirm).then_some(client_confirm)
}

/// The end of a v2 run: the agent's answer to our `confirm`. `proven`: its
/// confirmation checked out (we sent ours).
fn judge_v2_answer(answer: Option<AuthMessage>, proven: bool) -> Result<bool, ConnectError> {
    match answer {
        Some(AuthMessage::Refused { reason }) => Err(refusal(&reason)),
        Some(AuthMessage::Ok { trusted, .. }) if proven => Ok(trusted),
        // Not proven: whatever it says, it is not let in on (click mode with
        // a wrong password, or an impostor).
        Some(AuthMessage::Ok { .. }) => Err(ConnectError::NotProven),
        _ if !proven => Err(ConnectError::NotProven),
        _ => Err(ConnectError::Protocol("unexpected answer to the password check".into())),
    }
}

/// Client side of step 3; Ok((trusted, version)).
async fn client_auth<S: AsyncRead + AsyncWrite + Unpin>(
    ws: &mut WebSocketStream<S>,
    password: &str,
    cert_hash: &Digest,
    require_pake: bool,
    address: &str,
) -> Result<(bool, u32), ConnectError> {
    let lost = |e: tungstenite::Error| ConnectError::Unreachable(e.to_string());
    send_auth(ws, &AuthMessage::Hello { version: VERSION_PAKE }).await.map_err(lost)?;
    let (agent_nonce, versions) = match recv_auth(ws).await {
        Some(AuthMessage::Challenge { nonce, versions }) => (from_hex::<NONCE_LEN>(&nonce), versions),
        Some(AuthMessage::Refused { reason }) => return Err(refusal(&reason)),
        _ => (None, Vec::new()),
    };
    let agent_nonce = agent_nonce.ok_or_else(|| ConnectError::Protocol("unexpected answer (not a Lunaris agent?)".into()))?;
    let client_nonce = random_nonce();
    let has_password = !password.trim().is_empty();
    if has_password && versions.contains(&VERSION_PAKE) {
        let context = v2_context(&agent_nonce, &client_nonce, cert_hash);
        let (state, client_msg) = pake_client(password, &context, OsRng);
        send_auth(ws, &AuthMessage::Pake { client_nonce: to_hex(&client_nonce), msg: to_hex(&client_msg) })
            .await
            .map_err(lost)?;
        let client_confirm = match recv_auth(ws).await {
            Some(AuthMessage::PakeReply { msg, confirm }) => judge_pake_reply(state, &context, &client_msg, &msg, &confirm),
            Some(AuthMessage::Refused { reason }) => return Err(refusal(&reason)),
            // "ok" without a reply: an agent without a password (click
            // mode), or an impostor.
            Some(AuthMessage::Ok { .. }) => return Err(ConnectError::NotProven),
            _ => return Err(ConnectError::Protocol("unexpected answer to the password check".into())),
        };
        let proven = client_confirm.is_some();
        send_auth(ws, &AuthMessage::Confirm { mac: client_confirm.map(|c| to_hex(&c)) }).await.map_err(lost)?;
        return judge_v2_answer(recv_auth(ws).await, proven).map(|trusted| (trusted, VERSION_PAKE));
    }
    if has_password {
        if require_pake {
            warn!("{} offered only the old password proof (v1) where it used the new one before: refused", address);
            return Err(ConnectError::Downgraded);
        }
        warn!(
            "{} runs an older Lunaris agent (password proof v1): whoever answers at that address could test \
             passwords against it offline; update that agent",
            address
        );
    }
    let mac = has_password.then(|| to_hex(&client_mac(password, &agent_nonce, &client_nonce, cert_hash)));
    send_auth(ws, &AuthMessage::Proof { client_nonce: to_hex(&client_nonce), mac }).await.map_err(lost)?;
    judge_answer(recv_auth(ws).await, password, &agent_nonce, &client_nonce, cert_hash).map(|trusted| (trusted, 1))
}

fn refusal(reason: &str) -> ConnectError {
    match reason {
        REFUSED_UNAUTHORIZED => ConnectError::WrongPassword,
        REFUSED_TOO_MANY => ConnectError::TooManyAttempts,
        other => ConnectError::Refused(other.chars().filter(|c| !c.is_control()).take(100).collect()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // The same vectors are in server/src/links.rs; computed independently
    // (Python hmac/hashlib).
    const PASSWORD: &str = "abcd-efgh-ijkl";
    const CERT_HASH_HEX: &str = "fc105ce3018116bb697b777e09fd7ce7039e628a31ad7aab6c9bbe02c3bf0d1d";
    const CLIENT_MAC_HEX: &str = "5443f1e19d8c91e2256950e29b0476d5f90f9e9bba6ee2d799e271af115739cd";
    const AGENT_MAC_HEX: &str = "a389673baba901cf246ec3b4b83db77a97e466384fc8c76897ddd2d3abe497e8";

    fn cert_dir(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("lunaris-tls-{}-{}", name, crate::pairing::to_hex(&crate::pairing::random_bytes(6))));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn the_kept_certificate_is_the_same_at_every_start() {
        let dir = cert_dir("kept");
        let first = load_or_make_tls_server(&dir).unwrap().cert_hash;
        let again = load_or_make_tls_server(&dir).unwrap().cert_hash;
        assert_eq!(first, again);
        // Fresh ones (tests, the gateway's agent) differ.
        assert_ne!(make_tls_server().unwrap().cert_hash, first);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(dir.join(TLS_KEY_FILE)).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600);
        }
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_broken_kept_certificate_is_replaced_and_kept_again() {
        let dir = cert_dir("broken");
        let first = load_or_make_tls_server(&dir).unwrap().cert_hash;
        std::fs::write(dir.join(TLS_KEY_FILE), b"not a key").unwrap();
        let replaced = load_or_make_tls_server(&dir).unwrap().cert_hash;
        assert_ne!(replaced, first);
        assert_eq!(load_or_make_tls_server(&dir).unwrap().cert_hash, replaced);
        // A certificate without its key: made again.
        std::fs::remove_file(dir.join(TLS_KEY_FILE)).unwrap();
        let remade = load_or_make_tls_server(&dir).unwrap().cert_hash;
        assert_ne!(remade, replaced);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_folder_that_cannot_keep_it_still_gives_a_certificate() {
        let missing = std::env::temp_dir().join(format!("lunaris-tls-missing-{}", crate::pairing::to_hex(&crate::pairing::random_bytes(6))));
        assert!(load_or_make_tls_server(&missing).is_ok());
        assert!(!missing.exists());
    }

    #[test]
    fn mac_test_vectors() {
        let cert_hash = sha256(b"lunaris test certificate");
        assert_eq!(to_hex(&cert_hash), CERT_HASH_HEX);
        let (an, cn) = ([0x11u8; 32], [0x22u8; 32]);
        assert_eq!(to_hex(&client_mac(PASSWORD, &an, &cn, &cert_hash)), CLIENT_MAC_HEX);
        assert_eq!(to_hex(&agent_mac(PASSWORD, &an, &cn, &cert_hash)), AGENT_MAC_HEX);
        // The password is trimmed, like everywhere else.
        assert_eq!(to_hex(&client_mac(" abcd-efgh-ijkl\n", &an, &cn, &cert_hash)), CLIENT_MAC_HEX);
        let tag = from_hex::<32>(CLIENT_MAC_HEX).unwrap();
        assert!(mac_ok(CLIENT_LABEL, PASSWORD, &an, &cn, &cert_hash, &tag));
        assert!(!mac_ok(AGENT_LABEL, PASSWORD, &an, &cn, &cert_hash, &tag));
        // Another certificate (a man in the middle) gives another MAC.
        assert!(!mac_ok(CLIENT_LABEL, PASSWORD, &an, &cn, &sha256(b"other"), &tag));
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
            json(&AuthMessage::Refused { reason: REFUSED_TOO_MANY.into() }),
            r#"{"auth":"refused","reason":"too_many_attempts"}"#
        );
    }

    #[test]
    fn with_a_password_only_the_agents_valid_mac_is_accepted() {
        let (an, cn, ch) = ([1u8; 32], [2u8; 32], sha256(b"cert"));
        let good = Some(to_hex(&agent_mac("pw", &an, &cn, &ch)));
        let ok = |trusted: bool, mac: Option<String>| Some(AuthMessage::Ok { trusted, mac });
        assert_eq!(judge_answer(ok(true, good.clone()), "pw", &an, &cn, &ch), Ok(true));
        // A click-mode agent that proved it knows the password: asked there.
        assert_eq!(judge_answer(ok(false, good.clone()), "pw", &an, &cn, &ch), Ok(false));
        // An impostor: no MAC, a wrong one, or one for another certificate.
        assert_eq!(judge_answer(ok(false, None), "pw", &an, &cn, &ch), Err(ConnectError::NotProven));
        assert_eq!(judge_answer(ok(true, None), "pw", &an, &cn, &ch), Err(ConnectError::NotProven));
        let client_side = Some(to_hex(&client_mac("pw", &an, &cn, &ch)));
        assert_eq!(judge_answer(ok(true, client_side), "pw", &an, &cn, &ch), Err(ConnectError::NotProven));
        let other_cert = Some(to_hex(&agent_mac("pw", &an, &cn, &sha256(b"other"))));
        assert_eq!(judge_answer(ok(true, other_cert), "pw", &an, &cn, &ch), Err(ConnectError::NotProven));
        // Without a password: click-to-accept as before, "trusted" can't be.
        assert_eq!(judge_answer(ok(false, None), "", &an, &cn, &ch), Ok(false));
        assert_eq!(judge_answer(ok(true, good), "", &an, &cn, &ch), Err(ConnectError::NotProven));
        let refused = Some(AuthMessage::Refused { reason: REFUSED_UNAUTHORIZED.into() });
        assert_eq!(judge_answer(refused, "pw", &an, &cn, &ch), Err(ConnectError::WrongPassword));
    }

    #[test]
    fn hex_round_trip_and_rejects() {
        let n = random_nonce();
        assert_eq!(from_hex::<32>(&to_hex(&n)), Some(n));
        assert_ne!(random_nonce(), n);
        assert_eq!(from_hex::<2>("+f00"), None);
        assert_eq!(from_hex::<2>("abc"), None);
        assert_eq!(from_hex::<1>("zz"), None);
    }

    #[test]
    fn digests_compare_equal_only_when_equal() {
        let a = sha256(b"a");
        assert!(digest_eq(&a, &sha256(b"a")));
        assert!(!digest_eq(&a, &sha256(b"b")));
    }

    #[test]
    fn server_names() {
        assert_eq!(server_name("192.168.1.2:8091"), ServerName::try_from("192.168.1.2").unwrap());
        assert_eq!(server_name("[fe80::1]:8091"), ServerName::try_from("fe80::1").unwrap());
        assert_eq!(server_name("pc.lan:8091"), ServerName::try_from("pc.lan").unwrap());
    }

    // ---- v2 ----

    /// Deterministic "randomness" for the test vectors.
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

    // The same vectors are in server/src/links.rs. CONTEXT_HEX and the
    // confirmations from KEY_HEX were computed independently (Python
    // hashlib/hmac); X, Y and the key by this code with FixedRng(1)/(2).
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
        let (a, x) = pake_client(PASSWORD, &context, FixedRng(1));
        let (b, y) = pake_agent(PASSWORD, &context, FixedRng(2));
        let key_a = a.finish(&y).unwrap();
        let key_b = b.finish(&x).unwrap();
        let (ac, cc) = confirmations(&key_a, &context, &x, &y);
        assert_eq!(key_a, key_b);
        assert_eq!(to_hex(&context), CONTEXT_HEX);
        assert_eq!(to_hex(&x), X_HEX);
        assert_eq!(to_hex(&y), Y_HEX);
        assert_eq!(to_hex(&key_a), KEY_HEX);
        assert_eq!(to_hex(&ac), AGENT_CONFIRM_HEX);
        assert_eq!(to_hex(&cc), CLIENT_CONFIRM_HEX);
        // The password is trimmed, like everywhere else.
        let (_, x2) = pake_client(" abcd-efgh-ijkl\n", &context, FixedRng(1));
        assert_eq!(x2, x);
        // Role bytes: 'A' from the client, 'B' from the agent.
        assert_eq!((x[0], y[0]), (b'A', b'B'));
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
        // An old agent's challenge (no versions) still reads.
        let old: AuthMessage = serde_json::from_str(r#"{"auth":"challenge","nonce":"ab"}"#).unwrap();
        assert_eq!(old, AuthMessage::Challenge { nonce: "ab".into(), versions: vec![] });
    }

    /// The new messages read by the v1 shapes: an old client takes a new
    /// agent's challenge, an old agent a new client's hello.
    #[test]
    fn old_peers_read_the_new_messages() {
        let challenge = serde_json::to_string(&AuthMessage::Challenge { nonce: "ab".into(), versions: vec![2] }).unwrap();
        assert_eq!(serde_json::from_str::<v1::AuthMessage>(&challenge).unwrap(), v1::AuthMessage::Challenge { nonce: "ab".into() });
        let hello = serde_json::to_string(&AuthMessage::Hello { version: VERSION_PAKE }).unwrap();
        assert!(matches!(serde_json::from_str::<v1::AuthMessage>(&hello).unwrap(), v1::AuthMessage::Hello { .. }));
    }

    #[test]
    fn tampered_or_wrong_runs_do_not_confirm() {
        let ch = sha256(b"cert");
        let ctx = v2_context(&[1; 32], &[2; 32], &ch);
        let reply = |pw: &str, ctx: &Digest, x: &PakeMsg| {
            let (b, y) = pake_agent(pw, ctx, OsRng);
            let (ac, cc) = finish(b, ctx, x, &y, x).unwrap();
            (y, ac, cc)
        };
        // Right password: the agent's confirmation checks out, ours matches.
        let (a, x) = pake_client("pw-right", &ctx, OsRng);
        let (y, ac, cc) = reply("pw-right", &ctx, &x);
        assert_eq!(judge_pake_reply(a, &ctx, &x, &to_hex(&y), &to_hex(&ac)), Some(cc));
        // Wrong password on the agent's side (or an impostor guessing).
        let (a, x) = pake_client("pw-right", &ctx, OsRng);
        let (y, ac, _) = reply("pw-wrong", &ctx, &x);
        assert_eq!(judge_pake_reply(a, &ctx, &x, &to_hex(&y), &to_hex(&ac)), None);
        // Another certificate on the agent's side (a relay): another context.
        let (a, x) = pake_client("pw-right", &ctx, OsRng);
        let (y, ac, _) = reply("pw-right", &v2_context(&[1; 32], &[2; 32], &sha256(b"relay")), &x);
        assert_eq!(judge_pake_reply(a, &ctx, &x, &to_hex(&y), &to_hex(&ac)), None);
        // Tampered Y, tampered confirmation, our own message reflected, junk.
        let (a, x) = pake_client("pw-right", &ctx, OsRng);
        let (mut y, ac, _) = reply("pw-right", &ctx, &x);
        y[5] ^= 1;
        assert_eq!(judge_pake_reply(a, &ctx, &x, &to_hex(&y), &to_hex(&ac)), None);
        let (a, x) = pake_client("pw-right", &ctx, OsRng);
        let (y, mut ac, _) = reply("pw-right", &ctx, &x);
        ac[0] ^= 1;
        assert_eq!(judge_pake_reply(a, &ctx, &x, &to_hex(&y), &to_hex(&ac)), None);
        let (a, x) = pake_client("pw-right", &ctx, OsRng);
        let (_, ac, _) = reply("pw-right", &ctx, &x);
        assert_eq!(judge_pake_reply(a, &ctx, &x, &to_hex(&x), &to_hex(&ac)), None);
        let (a, x) = pake_client("pw-right", &ctx, OsRng);
        assert_eq!(judge_pake_reply(a, &ctx, &x, "zz", "00"), None);
        // The agent's confirmation is not the client's (no reflection).
        let (a, x) = pake_client("pw-right", &ctx, OsRng);
        let (y, _, cc) = reply("pw-right", &ctx, &x);
        assert_eq!(judge_pake_reply(a, &ctx, &x, &to_hex(&y), &to_hex(&cc)), None);
    }

    #[test]
    fn the_v2_answer_counts_only_when_the_agent_proved_itself() {
        let ok = |trusted| Some(AuthMessage::Ok { trusted, mac: None });
        assert_eq!(judge_v2_answer(ok(true), true), Ok(true));
        assert_eq!(judge_v2_answer(ok(false), true), Ok(false));
        assert_eq!(judge_v2_answer(ok(true), false), Err(ConnectError::NotProven));
        assert_eq!(judge_v2_answer(None, false), Err(ConnectError::NotProven));
        let refused = Some(AuthMessage::Refused { reason: REFUSED_UNAUTHORIZED.into() });
        assert_eq!(judge_v2_answer(refused, false), Err(ConnectError::WrongPassword));
    }

    // ---- Both sides over an in-memory WebSocket ----

    use super::super::Admission;
    use tokio_tungstenite::tungstenite::protocol::Role;

    /// Password-only mode, recording what it was asked.
    #[derive(Default)]
    struct TestGate {
        charges: u32,
        decided: Vec<(Presented, bool)>,
        locked: bool,
    }

    impl Gate for TestGate {
        fn charge(&mut self) -> bool {
            self.charges += 1;
            !self.locked
        }
        fn decide(&mut self, presented: Presented, charged: bool) -> Admission {
            self.decided.push((presented, charged));
            match presented {
                Presented::Right => Admission::Accept { trusted: true, link: false },
                _ => Admission::Unauthorized,
            }
        }
    }

    type Duplex = WebSocketStream<tokio::io::DuplexStream>;

    async fn ws_pair() -> (Duplex, Duplex) {
        let (a, b) = tokio::io::duplex(64 * 1024);
        (
            WebSocketStream::from_raw_socket(a, Role::Client, None).await,
            WebSocketStream::from_raw_socket(b, Role::Server, None).await,
        )
    }

    /// One run: (client's result, agent's result, the gate).
    async fn run(
        client_pw: &str,
        agent_pw: &str,
        client_cert: Digest,
        agent_cert: Digest,
        require: bool,
        gate: TestGate,
    ) -> (Result<(bool, u32), ConnectError>, anyhow::Result<(Admission, u32)>, TestGate) {
        let (mut c, mut a) = ws_pair().await;
        let mut gate = gate;
        let (cr, ar) = tokio::join!(client_auth(&mut c, client_pw, &client_cert, require, "test"), async {
            let r = agent_auth(&mut a, agent_pw, &agent_cert, &mut gate).await;
            // The client may still read the answer.
            drop(a);
            r
        });
        (cr, ar, gate)
    }

    #[tokio::test]
    async fn v2_with_the_right_password() {
        let ch = sha256(b"cert");
        let (c, a, gate) = run("pw", " pw ", ch, ch, false, TestGate::default()).await;
        assert_eq!(c, Ok((true, VERSION_PAKE)));
        assert_eq!(a.unwrap(), (Admission::Accept { trusted: true, link: false }, VERSION_PAKE));
        assert_eq!((gate.charges, gate.decided), (1, vec![(Presented::Right, true)]));
    }

    #[tokio::test]
    async fn v2_with_a_wrong_password_both_sides_fail_and_it_counts() {
        let ch = sha256(b"cert");
        let (c, a, gate) = run("pw-wrong", "pw", ch, ch, false, TestGate::default()).await;
        assert_eq!(c, Err(ConnectError::WrongPassword));
        assert_eq!(a.unwrap(), (Admission::Unauthorized, VERSION_PAKE));
        assert_eq!((gate.charges, gate.decided), (1, vec![(Presented::Wrong, true)]));
    }

    /// A relay: the client sees another certificate than the agent's own.
    #[tokio::test]
    async fn v2_bound_to_the_certificate() {
        let (c, a, gate) = run("pw", "pw", sha256(b"relay"), sha256(b"agent"), false, TestGate::default()).await;
        assert_eq!(c, Err(ConnectError::WrongPassword));
        assert_eq!(a.unwrap().0, Admission::Unauthorized);
        assert_eq!(gate.decided, vec![(Presented::Wrong, true)]);
    }

    /// Locked out: no PAKE message at all (no guess to test).
    #[tokio::test]
    async fn v2_locked_out_runs_no_pake() {
        let ch = sha256(b"cert");
        let (c, a, gate) = run("pw", "pw", ch, ch, false, TestGate { locked: true, ..Default::default() }).await;
        assert_eq!(c, Err(ConnectError::TooManyAttempts));
        assert_eq!(a.unwrap().0, Admission::TooManyAttempts);
        assert!(gate.decided.is_empty());
    }

    /// Without a password: the v1 `proof` without a MAC, as before.
    #[tokio::test]
    async fn without_a_password_nothing_is_proven() {
        let ch = sha256(b"cert");
        let (c, a, gate) = run("", "pw", ch, ch, true, TestGate::default()).await;
        assert_eq!(c, Err(ConnectError::WrongPassword));
        assert_eq!(a.unwrap(), (Admission::Unauthorized, 1));
        assert_eq!((gate.charges, gate.decided), (0, vec![(Presented::Nothing, false)]));
    }

    /// A client that hangs up after the agent's reply (it got its one
    /// guess): the failure counted in advance stays.
    #[tokio::test]
    async fn hanging_up_after_the_reply_still_counts() {
        let (mut c, mut a) = ws_pair().await;
        let ch = sha256(b"cert");
        let mut gate = TestGate::default();
        let client = async {
            send_auth(&mut c, &AuthMessage::Hello { version: 2 }).await.unwrap();
            let Some(AuthMessage::Challenge { nonce, .. }) = recv_auth(&mut c).await else { panic!() };
            let an = from_hex::<32>(&nonce).unwrap();
            let ctx = v2_context(&an, &[3; 32], &ch);
            let (_, x) = pake_client("guess", &ctx, OsRng);
            send_auth(&mut c, &AuthMessage::Pake { client_nonce: to_hex(&[3; 32]), msg: to_hex(&x) }).await.unwrap();
            assert!(matches!(recv_auth(&mut c).await, Some(AuthMessage::PakeReply { .. })));
            drop(c);
        };
        let (_, r) = tokio::join!(client, agent_auth(&mut a, "pw", &ch, &mut gate));
        assert!(r.is_err());
        assert_eq!((gate.charges, gate.decided.len()), (1, 0));
    }

    /// A tampered client message (a man in the middle flipping a bit):
    /// the agent's confirmation fails at the client, nothing is let in.
    #[tokio::test]
    async fn a_tampered_client_message_fails_both_sides() {
        let (mut c, mut a) = ws_pair().await;
        let ch = sha256(b"cert");
        let mut gate = TestGate::default();
        let client = async {
            send_auth(&mut c, &AuthMessage::Hello { version: 2 }).await.unwrap();
            let Some(AuthMessage::Challenge { nonce, .. }) = recv_auth(&mut c).await else { panic!() };
            let an = from_hex::<32>(&nonce).unwrap();
            let ctx = v2_context(&an, &[3; 32], &ch);
            let (state, x) = pake_client("pw", &ctx, OsRng);
            let mut sent = x;
            sent[9] ^= 0x40;
            send_auth(&mut c, &AuthMessage::Pake { client_nonce: to_hex(&[3; 32]), msg: to_hex(&sent) }).await.unwrap();
            match recv_auth(&mut c).await {
                Some(AuthMessage::PakeReply { msg, confirm }) => {
                    assert_eq!(judge_pake_reply(state, &ctx, &x, &msg, &confirm), None);
                    send_auth(&mut c, &AuthMessage::Confirm { mac: None }).await.unwrap();
                    recv_auth(&mut c).await
                }
                // Not a point on the curve: refused at once, the last word
                // (waiting for another would wait forever).
                refused @ Some(AuthMessage::Refused { .. }) => refused,
                other => panic!("{:?}", other),
            }
        };
        let (_, r) = tokio::join!(client, agent_auth(&mut a, "pw", &ch, &mut gate));
        assert_eq!(r.unwrap().0, Admission::Unauthorized);
        assert_eq!(gate.decided, vec![(Presented::Wrong, true)]);
    }

    /// An agent before v2, with the right and a wrong password.
    #[tokio::test]
    async fn a_new_client_falls_back_to_v1_with_an_old_agent() {
        let ch = sha256(b"cert");
        for (pw, want) in [("pw", Ok((true, 1))), ("nope", Err(ConnectError::WrongPassword))] {
            let (mut c, mut a) = ws_pair().await;
            let (cr, ar) = tokio::join!(client_auth(&mut c, pw, &ch, false, "test"), v1::agent_auth(&mut a, "pw", &ch));
            assert_eq!(cr, want);
            assert_eq!(ar, Some(pw == "pw"));
        }
    }

    /// Where v2 was seen before: an old-style answer gets no MAC at all.
    #[tokio::test]
    async fn a_downgrade_is_refused_before_anything_is_sent() {
        let ch = sha256(b"cert");
        let (mut c, mut a) = ws_pair().await;
        let (cr, ar) = tokio::join!(
            async {
                let r = client_auth(&mut c, "pw", &ch, true, "test").await;
                drop(c);
                r
            },
            v1::agent_auth(&mut a, "pw", &ch)
        );
        assert_eq!(cr, Err(ConnectError::Downgraded));
        // The fake old agent never got a proof.
        assert_eq!(ar, None);
    }

    /// A client before v2 against this agent: the v1 proof still works.
    #[tokio::test]
    async fn an_old_client_still_logs_in_with_v1() {
        let ch = sha256(b"cert");
        for (pw, trusted) in [("pw", true), ("nope", false)] {
            let (mut c, mut a) = ws_pair().await;
            let mut gate = TestGate::default();
            let (cr, ar) = tokio::join!(
                v1::client_auth(&mut c, pw, &ch),
                async {
                    let r = agent_auth(&mut a, "pw", &ch, &mut gate).await;
                    drop(a);
                    r
                }
            );
            assert_eq!(cr, if trusted { Ok(true) } else { Err(ConnectError::WrongPassword) });
            assert_eq!(ar.unwrap().1, 1);
            assert_eq!(gate.charges, 0);
            assert_eq!(gate.decided, vec![(if trusted { Presented::Right } else { Presented::Wrong }, false)]);
        }
    }
}

/// TLS and the WebSocket upgrade without the password step: for tests that
/// speak the handshake themselves (old clients). Returns the certificate hash.
#[cfg(test)]
pub(crate) async fn open_for_test(address: &str, path: &str) -> (WebSocketStream<MaybeTlsStream<TcpStream>>, Digest) {
    let tcp = TcpStream::connect(address).await.unwrap();
    let connector = tokio_rustls::TlsConnector::from(client_tls_config().unwrap());
    let tls = connector.connect(server_name(address), tcp).await.unwrap();
    let cert_hash = sha256(tls.get_ref().1.peer_certificates().unwrap().first().unwrap());
    let (ws, _) = tokio_tungstenite::client_async(format!("wss://{}{}", address, path), MaybeTlsStream::Rustls(tls)).await.unwrap();
    (ws, cert_hash)
}

/// The v1 handshake exactly as agents and clients before v2 spoke it (the
/// message shapes and both sides), to test against.
#[cfg(test)]
pub(crate) mod v1 {
    use super::*;

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

    async fn send<S: AsyncRead + AsyncWrite + Unpin>(ws: &mut WebSocketStream<S>, msg: &AuthMessage) -> Option<()> {
        ws.send(WsMessage::Text(serde_json::to_string(msg).unwrap().into())).await.ok()
    }

    async fn recv<S: AsyncRead + AsyncWrite + Unpin>(ws: &mut WebSocketStream<S>) -> Option<AuthMessage> {
        loop {
            match ws.next().await? {
                Ok(WsMessage::Text(text)) => return serde_json::from_str(&text).ok(),
                Ok(WsMessage::Ping(_) | WsMessage::Pong(_)) => continue,
                _ => return None,
            }
        }
    }

    /// The old client: Ok(trusted).
    pub async fn client_auth<S: AsyncRead + AsyncWrite + Unpin>(
        ws: &mut WebSocketStream<S>,
        password: &str,
        cert_hash: &Digest,
    ) -> Result<bool, ConnectError> {
        let lost = || ConnectError::Unreachable("lost".into());
        send(ws, &AuthMessage::Hello { version: 1 }).await.ok_or_else(lost)?;
        let agent_nonce = match recv(ws).await {
            Some(AuthMessage::Challenge { nonce }) => from_hex::<NONCE_LEN>(&nonce),
            Some(AuthMessage::Refused { reason }) => return Err(refusal(&reason)),
            _ => None,
        }
        .ok_or_else(|| ConnectError::Protocol("not a Lunaris agent".into()))?;
        let client_nonce = random_nonce();
        let mac = (!password.trim().is_empty()).then(|| to_hex(&client_mac(password, &agent_nonce, &client_nonce, cert_hash)));
        send(ws, &AuthMessage::Proof { client_nonce: to_hex(&client_nonce), mac }).await.ok_or_else(lost)?;
        let answer = match recv(ws).await {
            Some(AuthMessage::Ok { trusted, mac }) => Some(super::AuthMessage::Ok { trusted, mac }),
            Some(AuthMessage::Refused { reason }) => Some(super::AuthMessage::Refused { reason }),
            _ => None,
        };
        judge_answer(answer, password, &agent_nonce, &client_nonce, cert_hash)
    }

    /// The old agent in password mode: Some(whether the proof was right),
    /// None when no proof came.
    pub async fn agent_auth<S: AsyncRead + AsyncWrite + Unpin>(
        ws: &mut WebSocketStream<S>,
        password: &str,
        cert_hash: &Digest,
    ) -> Option<bool> {
        let Some(AuthMessage::Hello { .. }) = recv(ws).await else { return None };
        let agent_nonce = random_nonce();
        send(ws, &AuthMessage::Challenge { nonce: to_hex(&agent_nonce) }).await?;
        let Some(AuthMessage::Proof { client_nonce, mac }) = recv(ws).await else { return None };
        let client_nonce = from_hex::<NONCE_LEN>(&client_nonce)?;
        let right = mac
            .as_deref()
            .and_then(from_hex::<32>)
            .is_some_and(|t| mac_ok(CLIENT_LABEL, password, &agent_nonce, &client_nonce, cert_hash, &t));
        let answer = if right {
            AuthMessage::Ok { trusted: true, mac: Some(to_hex(&agent_mac(password, &agent_nonce, &client_nonce, cert_hash))) }
        } else {
            AuthMessage::Refused { reason: REFUSED_UNAUTHORIZED.into() }
        };
        send(ws, &answer).await?;
        Some(right)
    }
}
