//! A GameStream host inside the test process, for the tests of this connector.
//!
//! Its handlers are written from Sunshine src/nvhttp.cpp and src/crypto.cpp
//! (tag 2026.914): plain HTTP with `serverinfo` and `pair`, HTTPS with a client
//! certificate for `serverinfo`, `pair` (pairchallenge), `applist`, `launch`,
//! `resume`, `cancel`; a certificate the host has not stored at pairing gets
//! status 401. The client's side comes from Moonlight, this side from
//! Sunshine, so the two check each other.
//!
//! Use: `let host = FakeHost::start().await;`, pair against
//! `127.0.0.1:host.http_port` (see `pair_tests.rs` `pair_with`), or put a
//! client's certificate (DER) straight into `host.state.lock().paired`.
//! `host.state` shows what arrived: `launch_args` holds the query of the last
//! launch/resume (rikey, rikeyid, mode, corever ...).
//!
//! The streaming side (second half of this file) is written from Sunshine
//! src/rtsp.cpp, src/stream.cpp and src/input.cpp of the same tag: an RTSP
//! server (plain and encrypted) on the port the launch answers in
//! `sessionUrl0`, UDP sockets that learn the client's address from its
//! pings, a control server over ENet (rusty_enet's host side) that decrypts
//! and records every message and reads input packets as input.cpp does, and
//! senders that packetize frames and Opus packets with the host-side code of
//! `video_tests.rs` / `audio_tests.rs`. `host.state.lock().unwrap().stream`
//! holds its settings and everything it saw; the tests drive the senders
//! (`send_video`, `send_audio_block`, `send_control`).

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream, UdpSocket};
use tokio_rustls::rustls;

use super::http::{
    cert_der_from_pem, cert_parts, hex_decode, hex_encode, make_certificate, random_bytes,
    rsa_sign_sha256, rsa_verify_sha256,
};
use super::pair::{aes_ecb_decrypt, aes_ecb_encrypt, derive_aes_key, pairing_hash};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Phase {
    None,
    GetServerCert,
    ClientChallenge,
    ServerChallengeResp,
}

/// nvhttp.cpp pair_session_t
pub(crate) struct Session {
    pub(crate) client_cert_pem: String,
    pub(crate) salt: String,
    pub(crate) device_name: String,
    pub(crate) phase: Phase,
    /// The held answer of the first request (async_insert_pin.response).
    pub(crate) response: Option<tokio::sync::oneshot::Sender<String>>,
    pub(crate) cipher_key: Option<[u8; 16]>,
    pub(crate) server_secret: Vec<u8>,
    pub(crate) server_challenge: Vec<u8>,
    pub(crate) client_hash: Vec<u8>,
}

#[derive(Default)]
pub(crate) struct State {
    pub(crate) sessions: HashMap<String, Session>,
    /// Certificates (DER) of paired clients (client_root.named_devices).
    pub(crate) paired: Vec<Vec<u8>>,
    pub(crate) running_app: i32,
    pub(crate) launch_args: Option<HashMap<String, String>>,
    /// The client closed the connection of the first request before an answer.
    pub(crate) waiting_connection_closed: bool,
    /// Sign the pairing secret with garbage (somebody in the middle).
    pub(crate) bad_signature: bool,
    pub(crate) requests: Vec<String>,
    /// How often `/cancel` was asked.
    pub(crate) cancels: u32,
    /// The streaming side.
    pub(crate) stream: StreamHost,
}

pub(crate) struct HostKeys {
    pub(crate) cert_pem: String,
    pub(crate) key_pem: String,
}

/// One key for all tests of this process (making one is slow in a debug build).
pub(crate) fn host_keys() -> &'static HostKeys {
    static KEYS: OnceLock<HostKeys> = OnceLock::new();
    KEYS.get_or_init(|| {
        // Sunshine crypto.cpp gen_creds: random serial of up to 20 bytes, 20 years.
        let mut serial = random_bytes::<20>().unwrap();
        serial[0] &= 0x7f;
        let (cert_pem, key_pem) = make_certificate("Sunshine Gamestream Host", &serial, 20).unwrap();
        HostKeys { cert_pem, key_pem }
    })
}

pub(crate) struct FakeHost {
    pub(crate) http_port: u16,
    pub(crate) https_port: u16,
    pub(crate) rtsp_port: u16,
    pub(crate) state: Arc<Mutex<State>>,
    pub(crate) tasks: Vec<tokio::task::JoinHandle<()>>,
    video_socket: Arc<UdpSocket>,
    pub(crate) audio_socket: Arc<UdpSocket>,
}

impl Drop for FakeHost {
    fn drop(&mut self) {
        for t in &self.tasks {
            t.abort();
        }
    }
}

/// Takes any client certificate in the handshake: Sunshine decides after it
/// and answers 401 inside HTTP (nvhttp.cpp SunshineHTTPSServer: the verify
/// callback returns 1, then `verify` / `on_verify_failed`).
#[derive(Debug)]
struct AnyClientCert(rustls::crypto::WebPkiSupportedAlgorithms);

impl rustls::server::danger::ClientCertVerifier for AnyClientCert {
    fn root_hint_subjects(&self) -> &[rustls::DistinguishedName] {
        &[]
    }

    fn verify_client_cert(
        &self,
        _end_entity: &rustls::pki_types::CertificateDer<'_>,
        _intermediates: &[rustls::pki_types::CertificateDer<'_>],
        _now: rustls::pki_types::UnixTime,
    ) -> Result<rustls::server::danger::ClientCertVerified, rustls::Error> {
        Ok(rustls::server::danger::ClientCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &rustls::pki_types::CertificateDer<'_>,
        dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(message, cert, dss, &self.0)
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &rustls::pki_types::CertificateDer<'_>,
        dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(message, cert, dss, &self.0)
    }

    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        self.0.supported_schemes()
    }
}

fn percent_decode(text: &str) -> String {
    let b = text.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 3 <= b.len() {
            if let Some(v) = hex_decode(&text[i + 1..i + 3]) {
                out.push(v[0]);
                i += 3;
                continue;
            }
        }
        out.push(if b[i] == b'+' { b' ' } else { b[i] });
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn xml(status: u32, message: Option<&str>, elements: &[(&str, String)]) -> String {
    let mut out = String::from("<?xml version=\"1.0\" encoding=\"utf-8\"?>\n<root status_code=\"");
    out.push_str(&status.to_string());
    out.push('"');
    if let Some(m) = message {
        out.push_str(&format!(" status_message=\"{m}\""));
    }
    out.push('>');
    for (name, value) in elements {
        out.push_str(&format!("<{name}>{value}</{name}>"));
    }
    out.push_str("</root>");
    out
}

/// nvhttp.cpp fail_pair
fn fail_pair(message: &str) -> String {
    xml(400, Some(message), &[("paired", "0".into())])
}

/// nvhttp.cpp getservercert
fn getservercert(sess: &mut Session, pin: &str) -> (String, bool) {
    if sess.phase != Phase::None {
        return (fail_pair("Out of order call to getservercert"), true);
    }
    sess.phase = Phase::GetServerCert;
    if sess.salt.len() < 32 {
        return (fail_pair("Salt too short"), true);
    }
    let salt: [u8; 16] = hex_decode(&sess.salt[..32]).unwrap().try_into().unwrap();
    sess.cipher_key = Some(derive_aes_key(&salt, pin, true));
    let plaincert = hex_encode(host_keys().cert_pem.as_bytes());
    (xml(200, None, &[("paired", "1".into()), ("plaincert", plaincert)]), false)
}

/// nvhttp.cpp clientchallenge
fn clientchallenge(sess: &mut Session, challenge: &[u8]) -> (String, bool) {
    if sess.phase != Phase::GetServerCert {
        return (fail_pair("Out of order call to clientchallenge"), true);
    }
    sess.phase = Phase::ClientChallenge;
    let Some(key) = sess.cipher_key else {
        return (fail_pair("Cipher key not set"), true);
    };
    let mut decrypted = aes_ecb_decrypt(&key, challenge);
    let sign = cert_parts(&cert_der_from_pem(&host_keys().cert_pem).unwrap()).unwrap().signature;
    let server_secret = random_bytes::<16>().unwrap().to_vec();
    decrypted.extend_from_slice(&sign);
    decrypted.extend_from_slice(&server_secret);
    let hash = pairing_hash(true, &decrypted);
    let server_challenge = random_bytes::<16>().unwrap().to_vec();
    let mut plaintext = hash;
    plaintext.extend_from_slice(&server_challenge);
    let encrypted = aes_ecb_encrypt(&key, &plaintext);
    sess.server_secret = server_secret;
    sess.server_challenge = server_challenge;
    (xml(200, None, &[("paired", "1".into()), ("challengeresponse", hex_encode(&encrypted))]), false)
}

/// nvhttp.cpp serverchallengeresp
fn serverchallengeresp(sess: &mut Session, encrypted: &[u8], bad_signature: bool) -> (String, bool) {
    if sess.phase != Phase::ClientChallenge {
        return (fail_pair("Out of order call to serverchallengeresp"), true);
    }
    sess.phase = Phase::ServerChallengeResp;
    let Some(key) = sess.cipher_key else {
        return (fail_pair("Cipher key or serversecret not set"), true);
    };
    sess.client_hash = aes_ecb_decrypt(&key, encrypted);
    let mut secret = sess.server_secret.clone();
    let mut sign = rsa_sign_sha256(&host_keys().key_pem, &secret).unwrap();
    if bad_signature {
        sign[7] ^= 0x40;
    }
    secret.extend_from_slice(&sign);
    (xml(200, None, &[("pairingsecret", hex_encode(&secret)), ("paired", "1".into())]), false)
}

/// nvhttp.cpp clientpairingsecret; the certificate to authorize when it holds.
fn clientpairingsecret(sess: &mut Session, pairing_secret: &[u8]) -> (String, Option<Vec<u8>>) {
    if sess.phase != Phase::ServerChallengeResp {
        return (fail_pair("Out of order call to clientpairingsecret"), None);
    }
    if pairing_secret.len() <= 16 {
        return (fail_pair("Client pairing secret too short"), None);
    }
    let (secret, sign) = pairing_secret.split_at(16);
    let Some(parts) = cert_der_from_pem(&sess.client_cert_pem).and_then(|der| cert_parts(&der)) else {
        return (fail_pair("Invalid client certificate"), None);
    };
    let mut data = sess.server_challenge.clone();
    data.extend_from_slice(&parts.signature);
    data.extend_from_slice(secret);
    let hash = pairing_hash(true, &data);
    let same_hash = hash == sess.client_hash;
    let verify = rsa_verify_sha256(&sess.client_cert_pem, secret, sign);
    // add_authorized_client stores only what OpenSSL can read and re-check.
    let paired = same_hash && verify && super::http::cert_is_self_signed(&sess.client_cert_pem);
    let cert = paired.then(|| cert_der_from_pem(&sess.client_cert_pem).unwrap());
    (xml(200, None, &[("paired", if paired { "1" } else { "0" }.into())]), cert)
}

enum Reply {
    Now(String),
    /// The answer comes when the PIN is entered.
    Held(tokio::sync::oneshot::Receiver<String>),
}

/// nvhttp.cpp pair
fn handle_pair(state: &Mutex<State>, args: &HashMap<String, String>) -> Reply {
    let Some(unique_id) = args.get("uniqueid") else {
        return Reply::Now(xml(400, Some("Missing uniqueid parameter"), &[]));
    };
    let mut st = state.lock().unwrap();
    match args.get("phrase").map(String::as_str) {
        Some("getservercert") => {
            // start_pairing_session / insert_pair_session
            if st.sessions.contains_key(unique_id) {
                return Reply::Now(xml(
                    409,
                    Some("A pairing session with this uniqueid already exists"),
                    &[("paired", "0".into())],
                ));
            }
            let (tx, rx) = tokio::sync::oneshot::channel();
            st.sessions.insert(
                unique_id.clone(),
                Session {
                    client_cert_pem: String::from_utf8(hex_decode(&args["clientcert"]).unwrap()).unwrap(),
                    salt: args["salt"].clone(),
                    device_name: args["devicename"].clone(),
                    phase: Phase::None,
                    response: Some(tx),
                    cipher_key: None,
                    server_secret: Vec::new(),
                    server_challenge: Vec::new(),
                    client_hash: Vec::new(),
                },
            );
            return Reply::Held(rx);
        }
        Some("pairchallenge") => {
            return Reply::Now(xml(200, None, &[("paired", "1".into())]));
        }
        _ => {}
    }
    let bad_signature = st.bad_signature;
    let Some(sess) = st.sessions.get_mut(unique_id) else {
        return Reply::Now(xml(400, Some("Invalid uniqueid"), &[]));
    };
    let mut authorize = None;
    let (answer, drop_session) = if let Some(hex) = args.get("clientchallenge") {
        clientchallenge(sess, &hex_decode(hex).unwrap())
    } else if let Some(hex) = args.get("serverchallengeresp") {
        serverchallengeresp(sess, &hex_decode(hex).unwrap(), bad_signature)
    } else if let Some(hex) = args.get("clientpairingsecret") {
        let (answer, cert) = clientpairingsecret(sess, &hex_decode(hex).unwrap());
        authorize = cert;
        (answer, true)
    } else {
        (fail_pair("Invalid pairing request"), true)
    };
    if drop_session {
        st.sessions.remove(unique_id);
    }
    if let Some(cert) = authorize {
        st.paired.push(cert);
    }
    Reply::Now(answer)
}

/// nvhttp.cpp serverinfo
fn handle_serverinfo(host: &HostPorts, state: &Mutex<State>, args: &HashMap<String, String>, https: bool) -> String {
    let st = state.lock().unwrap();
    let pair_status = if https && args.contains_key("uniqueid") { 1 } else { 0 };
    xml(
        200,
        None,
        &[
            ("hostname", "fake-host".into()),
            ("appversion", "7.1.431.-1".into()),
            ("GfeVersion", "3.23.0.74".into()),
            ("uniqueid", "0A0B0C0D-0000-0000-0000-000000000001".into()),
            ("HttpsPort", host.https.to_string()),
            ("ExternalPort", host.http.to_string()),
            ("MaxLumaPixelsHEVC", "1869449984".into()),
            ("mac", if https { "02:00:00:00:00:01" } else { "00:00:00:00:00:00" }.into()),
            ("LocalIP", "127.0.0.1".into()),
            ("ServerCodecModeSupport", st.stream.codec_mode_support().to_string()),
            ("PairStatus", pair_status.to_string()),
            ("currentgame", st.running_app.to_string()),
            (
                "state",
                if st.running_app > 0 { "SUNSHINE_SERVER_BUSY" } else { "SUNSHINE_SERVER_FREE" }.into(),
            ),
        ],
    )
}

pub(crate) const DESKTOP_ID: i32 = 881448767;

/// nvhttp.cpp applist, launch, resume, cancel
fn handle_https(host: &HostPorts, state: &Mutex<State>, path: &str, args: &HashMap<String, String>) -> String {
    match path {
        "/serverinfo" => handle_serverinfo(host, state, args, true),
        "/applist" => format!(
            "<?xml version=\"1.0\" encoding=\"utf-8\"?>\n<root status_code=\"200\"><App><IsHdrSupported>0</IsHdrSupported><AppTitle>Desktop</AppTitle><ID>{DESKTOP_ID}</ID></App><App><IsHdrSupported>0</IsHdrSupported><AppTitle>Steam &amp; Big Picture</AppTitle><ID>1093255277</ID></App></root>"
        ),
        "/launch" => {
            let mut st = state.lock().unwrap();
            if !["rikey", "rikeyid", "localAudioPlayMode", "appid"].iter().all(|k| args.contains_key(*k)) {
                return xml(400, Some("Missing a required launch parameter"), &[("resume", "0".into())]);
            }
            if st.running_app > 0 {
                return xml(400, Some("An app is already running on this host"), &[("resume", "0".into())]);
            }
            let appid: i32 = args["appid"].parse().unwrap();
            if appid > 0 && appid != DESKTOP_ID {
                return xml(404, Some("Failed to start the specified application"), &[("gamesession", "0".into())]);
            }
            st.running_app = appid;
            st.launch_args = Some(args.clone());
            let scheme = st.stream.launch_session_raise(args);
            xml(200, None, &[("sessionUrl0", format!("{scheme}://127.0.0.1:{}", st.stream.rtsp_port)), ("gamesession", "1".into())])
        }
        "/resume" => {
            let mut st = state.lock().unwrap();
            if st.running_app == 0 {
                return xml(503, Some("No running app to resume"), &[("resume", "0".into())]);
            }
            if !["rikey", "rikeyid"].iter().all(|k| args.contains_key(*k)) {
                return xml(400, Some("Missing a required resume parameter"), &[("resume", "0".into())]);
            }
            st.launch_args = Some(args.clone());
            let scheme = st.stream.launch_session_raise(args);
            xml(200, None, &[("sessionUrl0", format!("{scheme}://127.0.0.1:{}", st.stream.rtsp_port)), ("resume", "1".into())])
        }
        "/cancel" => {
            // nvhttp.cpp cancel: rtsp_stream::terminate_sessions(), proc::proc.terminate()
            let mut st = state.lock().unwrap();
            st.running_app = 0;
            st.cancels += 1;
            st.stream.terminate_sessions();
            xml(200, None, &[("cancel", "1".into())])
        }
        "/pair" => match handle_pair(state, args) {
            Reply::Now(answer) => answer,
            Reply::Held(_) => xml(400, Some("not in this test"), &[]),
        },
        _ => xml(404, None, &[]),
    }
}

/// Where the host listens on 127.0.0.1; 0 = any free port (what the tests use).
#[derive(Clone, Copy, Default)]
pub(crate) struct FixedPorts {
    pub(crate) http: u16,
    pub(crate) https: u16,
    pub(crate) rtsp: u16,
    pub(crate) video: u16,
    pub(crate) audio: u16,
    pub(crate) control: u16,
}

impl FixedPorts {
    /// Sunshine's port family around its HTTP port (network.h map_port over
    /// `config::sunshine.port`, whose default 47989 is the HTTP port: HTTPS
    /// -5, video +9, control +10, audio +11, RTSP +21).
    pub(crate) fn family(http: u16) -> FixedPorts {
        FixedPorts { http, https: http - 5, rtsp: http + 21, video: http + 9, audio: http + 11, control: http + 10 }
    }
}

#[derive(Clone, Copy)]
pub(crate) struct HostPorts {
    pub(crate) http: u16,
    pub(crate) https: u16,
}

async fn read_request<S: AsyncRead + Unpin>(stream: &mut S) -> Option<(String, HashMap<String, String>)> {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 4096];
    while !buf.windows(4).any(|w| w == b"\r\n\r\n") {
        let n = stream.read(&mut chunk).await.ok()?;
        if n == 0 {
            return None;
        }
        buf.extend_from_slice(&chunk[..n]);
    }
    let head = String::from_utf8_lossy(&buf).into_owned();
    let mut words = head.lines().next()?.split(' ');
    if words.next()? != "GET" {
        return None;
    }
    let target = words.next()?;
    let (path, query) = target.split_once('?').unwrap_or((target, ""));
    let mut args = HashMap::new();
    for pair in query.split('&').filter(|p| !p.is_empty()) {
        let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
        args.insert(percent_decode(k), percent_decode(v));
    }
    Some((path.to_string(), args))
}

async fn write_answer<S: AsyncWrite + Unpin>(stream: &mut S, body: &str) {
    // Simple-Web-Server's answer for `response->write(text)`.
    let head = format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n", body.len());
    let _ = stream.write_all(head.as_bytes()).await;
    let _ = stream.write_all(body.as_bytes()).await;
    let _ = stream.shutdown().await;
}

impl FakeHost {
    pub(crate) async fn start() -> FakeHost {
        Self::start_on(FixedPorts::default()).await
    }

    /// As [`FakeHost::start`], on the given ports of 127.0.0.1 (0: any free one).
    pub(crate) async fn start_on(fixed: FixedPorts) -> FakeHost {
        let keys = host_keys();
        let http = TcpListener::bind(("127.0.0.1", fixed.http)).await.unwrap();
        let https = TcpListener::bind(("127.0.0.1", fixed.https)).await.unwrap();
        let ports = HostPorts {
            http: http.local_addr().unwrap().port(),
            https: https.local_addr().unwrap().port(),
        };
        let state = Arc::new(Mutex::new(State::default()));
        let (stream_tasks, video_socket, audio_socket) = start_stream_side(&state, &fixed).await;
        let rtsp_port = state.lock().unwrap().stream.rtsp_port;

        let st = state.clone();
        let http_task = tokio::spawn(async move {
            loop {
                let Ok((mut stream, _)) = http.accept().await else { return };
                let st = st.clone();
                tokio::spawn(async move {
                    let Some((path, args)) = read_request(&mut stream).await else { return };
                    st.lock().unwrap().requests.push(format!("http {path}"));
                    let answer = match path.as_str() {
                        "/serverinfo" => handle_serverinfo(&ports, &st, &args, false),
                        "/pair" => match handle_pair(&st, &args) {
                            Reply::Now(answer) => answer,
                            Reply::Held(rx) => {
                                let mut probe = [0u8; 1];
                                tokio::select! {
                                    answer = rx => match answer {
                                        Ok(answer) => answer,
                                        // The session went away without an answer.
                                        Err(_) => return,
                                    },
                                    _ = stream.read(&mut probe) => {
                                        st.lock().unwrap().waiting_connection_closed = true;
                                        return;
                                    }
                                }
                            }
                        },
                        // nvhttp.cpp not_found: no /unpair, no /applist over HTTP.
                        _ => xml(404, None, &[]),
                    };
                    write_answer(&mut stream, &answer).await;
                });
            }
        });

        let provider = Arc::new(rustls::crypto::ring::default_provider());
        let verifier = AnyClientCert(provider.signature_verification_algorithms);
        let (_, key_der) = super::http::pem_decode(&keys.key_pem).unwrap();
        let config = rustls::ServerConfig::builder_with_provider(provider)
            .with_safe_default_protocol_versions()
            .unwrap()
            .with_client_cert_verifier(Arc::new(verifier))
            .with_single_cert(
                vec![cert_der_from_pem(&keys.cert_pem).unwrap().into()],
                rustls::pki_types::PrivateKeyDer::Pkcs8(key_der.into()),
            )
            .unwrap();
        let acceptor = tokio_rustls::TlsAcceptor::from(Arc::new(config));
        let st = state.clone();
        let https_task = tokio::spawn(async move {
            loop {
                let Ok((stream, _)) = https.accept().await else { return };
                let st = st.clone();
                let acceptor = acceptor.clone();
                tokio::spawn(async move {
                    let Ok(mut stream) = acceptor.accept(stream).await else { return };
                    let peer = stream
                        .get_ref()
                        .1
                        .peer_certificates()
                        .and_then(|c| c.first())
                        .map(|c| c.as_ref().to_vec());
                    let Some((path, args)) = read_request(&mut stream).await else { return };
                    st.lock().unwrap().requests.push(format!("https {path}"));
                    // nvhttp.cpp https_server.verify: only a stored certificate passes.
                    let verified = peer.is_some_and(|p| st.lock().unwrap().paired.contains(&p));
                    let answer = if verified {
                        handle_https(&ports, &st, &path, &args)
                    } else {
                        // nvhttp.cpp on_verify_failed
                        format!(
                            "<?xml version=\"1.0\" encoding=\"utf-8\"?>\n<root status_code=\"401\" query=\"{path}\" status_message=\"The client is not authorized. Certificate verification failed.\"/>"
                        )
                    };
                    write_answer(&mut stream, &answer).await;
                });
            }
        });

        let mut tasks = vec![http_task, https_task];
        tasks.extend(stream_tasks);
        FakeHost { http_port: ports.http, https_port: ports.https, rtsp_port, state, tasks, video_socket, audio_socket }
    }

    /// Waits until a pairing request is held, as the PIN page of Sunshine
    /// would list it (nvhttp.cpp get_pending_pairings); its device name.
    pub(crate) async fn wait_pending(&self) -> String {
        for _ in 0..500 {
            if let Some(s) = self.state.lock().unwrap().sessions.values().find(|s| s.phase == Phase::None) {
                return s.device_name.clone();
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!("no pairing request arrived");
    }

    /// The person types a PIN at the host (nvhttp.cpp pin).
    pub(crate) fn enter_pin(&self, pin: &str) {
        let mut st = self.state.lock().unwrap();
        let sess = st.sessions.values_mut().find(|s| s.phase == Phase::None).expect("a waiting session");
        let (answer, _) = getservercert(sess, pin);
        sess.response.take().unwrap().send(answer).unwrap();
    }

    /// The person turns the request down (nvhttp.cpp cancel_pairing).
    pub(crate) fn refuse(&self) {
        let mut st = self.state.lock().unwrap();
        let id = st.sessions.iter().find(|(_, s)| s.phase == Phase::None).map(|(id, _)| id.clone()).unwrap();
        let mut sess = st.sessions.remove(&id).unwrap();
        let answer = xml(400, Some("Pairing request cancelled by operator"), &[("paired", "0".into())]);
        sess.response.take().unwrap().send(answer).unwrap();
    }

    pub(crate) fn session_count(&self) -> usize {
        self.state.lock().unwrap().sessions.len()
    }

    pub(crate) fn paired_count(&self) -> usize {
        self.state.lock().unwrap().paired.len()
    }

    pub(crate) async fn wait_no_session(&self) {
        for _ in 0..500 {
            if self.session_count() == 0 {
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!("the host still holds a pairing session");
    }
}


// ===========================================================================
// The streaming side: Sunshine src/rtsp.cpp, src/stream.cpp, src/input.cpp
// (tag 2026.914), and the keyboard tables of libvirtualhid at the commit
// that tag pins (53e1a94).
// ===========================================================================

use aes_gcm::aead::consts::U16;
use aes_gcm::{AeadInOut, Aes128Gcm, AesGcm, KeyInit};
use rusty_enet as enet;
use std::net::SocketAddr;

use super::audio::tests::Host as AudioHost;
use super::control::EnetSocket;
use super::video::tests::Host as VideoHost;

pub(crate) const SS_ENC_CONTROL_V2: u32 = 0x01;
pub(crate) const SS_ENC_VIDEO: u32 = 0x02;
pub(crate) const SS_ENC_AUDIO: u32 = 0x04;
const ML_FF_SESSION_ID_V1: u32 = 0x02;

/// config.h `lan_encryption_mode` / `wan_encryption_mode` as
/// net::encryption_mode_for_address gives it for the client's address.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub(crate) enum EncryptionMode {
    /// ENCRYPTION_MODE_NEVER: Sunshine's default on a private network.
    #[default]
    Never,
    /// ENCRYPTION_MODE_OPPORTUNISTIC: its default elsewhere.
    Opportunistic,
    Mandatory,
}

/// nvhttp.cpp make_launch_session
#[derive(Clone, Debug)]
pub(crate) struct LaunchSession {
    pub(crate) key: [u8; 16],
    /// `iv[0..4]` read big-endian (stream.cpp session::alloc `audio.avRiKeyId`).
    pub(crate) rikeyid: u32,
    /// `rtsp_cipher` is set (`corever` >= 1).
    pub(crate) rtsp_encrypted: bool,
    pub(crate) rtsp_iv_counter: u32,
    /// 8 random bytes as 16 hex digits.
    pub(crate) av_ping_payload: String,
    pub(crate) control_connect_data: u32,
}

/// One RTSP request as it arrived.
#[derive(Clone, Debug)]
pub(crate) struct RtspSeen {
    pub(crate) method: String,
    pub(crate) target: String,
    pub(crate) headers: Vec<(String, String)>,
    pub(crate) payload: String,
    pub(crate) encrypted: bool,
    /// Bytes on the wire, encryption header included.
    pub(crate) wire_len: usize,
    /// The sequence number of the encryption header.
    pub(crate) seq: u32,
}

impl RtspSeen {
    pub(crate) fn header(&self, name: &str) -> Option<&str> {
        self.headers.iter().find(|(n, _)| n == name).map(|(_, v)| v.as_str())
    }
}

/// stream.cpp config_t as rtsp.cpp cmd_announce fills it.
#[derive(Clone, Debug, Default)]
pub(crate) struct StreamConfig {
    /// Every `a=` line of the SDP, with the defaults cmd_announce adds.
    pub(crate) args: HashMap<String, String>,
    /// The `s=` line.
    pub(crate) client: String,
    pub(crate) packetsize: usize,
    pub(crate) min_required_fec_packets: usize,
    pub(crate) ml_feature_flags: u32,
    pub(crate) control_protocol_type: i64,
    pub(crate) encryption_flags_enabled: u32,
    pub(crate) width: i64,
    pub(crate) height: i64,
    pub(crate) framerate: i64,
    pub(crate) framerate_x100: i64,
    pub(crate) bitrate: i64,
    pub(crate) configured_bitrate_kbps: i64,
    pub(crate) slices_per_frame: i64,
    pub(crate) num_ref_frames: i64,
    pub(crate) encoder_csc_mode: i64,
    pub(crate) video_format: i64,
    pub(crate) dynamic_range: i64,
    pub(crate) chroma_sampling_type: i64,
    pub(crate) enable_intra_refresh: i64,
    pub(crate) audio_channels: i64,
    pub(crate) audio_mask: i64,
    pub(crate) audio_packet_duration: i64,
    pub(crate) audio_high_quality: bool,
    pub(crate) audio_qos_type: i64,
    pub(crate) video_qos_type: i64,
}

/// An input packet as input.cpp reads it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum InputSeen {
    /// passthrough(PNV_REL_MOUSE_MOVE_PACKET): platf::move_mouse(deltaX, deltaY)
    MoveRelative { dx: i16, dy: i16 },
    /// passthrough(PNV_ABS_MOUSE_MOVE_PACKET): x, y within width x height
    MoveAbsolute { x: i16, y: i16, width: i16, height: i16 },
    /// passthrough(PNV_MOUSE_BUTTON_PACKET)
    Button { button: u8, release: bool },
    /// passthrough(PNV_SCROLL_PACKET): scrollAmt1
    Scroll { amount: i16 },
    /// passthrough(PSS_HSCROLL_PACKET)
    HScroll { amount: i16 },
    /// passthrough(PNV_KEYBOARD_PACKET): `keyCode & 0x00FF`
    Key { vk: u8, release: bool, modifiers: u8, flags: u8 },
    /// passthrough(const NV_UNICODE_PACKET *)
    Text(String),
    /// passthrough(PSS_CONTROLLER_ARRIVAL_PACKET)
    ControllerArrival { number: u8, kind: u8, capabilities: u16, buttons: u32 },
    /// passthrough(PNV_MULTI_CONTROLLER_PACKET): `bf | (bf2 << 16)`, the
    /// sticks as the host's platform gets them (y upwards).
    Controller { number: i16, active_mask: u16, buttons: u32, lt: u8, rt: u8, sticks: [i16; 4] },
    Other { magic: u32 },
}

/// One control message after decryption.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ControlSeen {
    pub(crate) channel: u8,
    pub(crate) ptype: u16,
    pub(crate) payload: Vec<u8>,
    /// The sequence number of the encrypted envelope.
    pub(crate) seq: u32,
}

/// stream.cpp session_t, as far as the tests need it.
pub(crate) struct StreamSession {
    pub(crate) launch: LaunchSession,
    pub(crate) config: StreamConfig,
    pub(crate) video_peer: Option<SocketAddr>,
    pub(crate) audio_peer: Option<SocketAddr>,
    pub(crate) video: VideoHost,
    pub(crate) audio: AudioHost,
    /// control.seq
    control_seq: u32,
    control_peer: Option<enet::PeerID>,
    /// session::stop was called.
    stopping: bool,
}

pub(crate) struct StreamHost {
    // What the host is set to.
    pub(crate) encryption_mode: EncryptionMode,
    /// video::active_hevc_mode != 1 / active_av1_mode != 1
    pub(crate) hevc: bool,
    pub(crate) av1: bool,
    pub(crate) rtsp_port: u16,
    pub(crate) video_port: u16,
    pub(crate) audio_port: u16,
    pub(crate) control_port: u16,
    /// The largest IP packet the way to the client lets through: a video
    /// datagram that needs more does not arrive. Default 1500 (the tests
    /// reach this host over the loopback interface, which `net.rs` finds to
    /// be the local link); a test of a narrow path sets it lower.
    /// The first real host was behind a tunnel of 1420, and 1392-byte video
    /// packets, as Moonlight asks for on a private address, were lost there.
    pub(crate) path_mtu: usize,
    /// globals.cpp `display_cursor`: the pointer is drawn into the picture.
    pub(crate) display_cursor: bool,
    /// input.cpp `input_t::shortcutFlags` (CTRL 1, ALT 2, SHIFT 4).
    pub(crate) shortcut_flags: u8,
    /// How often Ctrl+Alt+Shift+N was applied.
    pub(crate) cursor_toggles: u32,
    /// Video datagrams that did not fit `path_mtu`.
    pub(crate) video_too_big: u64,
    // What happened.
    /// rtsp_server_t::launch_event
    pub(crate) launch: Option<LaunchSession>,
    pub(crate) rtsp: Vec<RtspSeen>,
    pub(crate) session: Option<StreamSession>,
    /// Sessions that were started (cmd_announce) so far.
    pub(crate) sessions_started: u32,
    pub(crate) control: Vec<ControlSeen>,
    pub(crate) input: Vec<InputSeen>,
    /// "Dropping malformed input packet"
    pub(crate) malformed_input: u32,
    /// IDX_REQUEST_IDR_FRAME (0x0302) arrivals: `idr_events->raise(true)`.
    pub(crate) idr_requests: u32,
    pub(crate) periodic_pings: u32,
    pub(crate) start_b: u32,
    pub(crate) fec_status: Vec<Vec<u8>>,
    /// "Dropping unencrypted message on encrypted control stream" and failed tags.
    pub(crate) control_rejected: u32,
    pub(crate) video_pings: u32,
    pub(crate) audio_pings: u32,
    /// The counter of the last video ping (SS_PING.sequenceNumber).
    pub(crate) last_video_ping_seq: u32,
    pub(crate) control_connects: u32,
    pub(crate) control_refused: u32,
    pub(crate) control_disconnects: u32,
    /// Control messages waiting to go to the client: (type, payload).
    outgoing: Vec<(u16, Vec<u8>)>,
}

impl Default for StreamHost {
    fn default() -> Self {
        StreamHost {
            encryption_mode: EncryptionMode::Never,
            hevc: true,
            av1: false,
            rtsp_port: 0,
            video_port: 0,
            audio_port: 0,
            control_port: 0,
            path_mtu: 1500,
            display_cursor: true,
            shortcut_flags: 0,
            cursor_toggles: 0,
            video_too_big: 0,
            launch: None,
            rtsp: Vec::new(),
            session: None,
            sessions_started: 0,
            control: Vec::new(),
            input: Vec::new(),
            malformed_input: 0,
            idr_requests: 0,
            periodic_pings: 0,
            start_b: 0,
            fec_status: Vec::new(),
            control_rejected: 0,
            video_pings: 0,
            audio_pings: 0,
            last_video_ping_seq: 0,
            control_connects: 0,
            control_refused: 0,
            control_disconnects: 0,
            outgoing: Vec::new(),
        }
    }
}

impl StreamHost {
    /// nvhttp.cpp serverinfo: SCM_H264, + SCM_HEVC, + SCM_AV1_MAIN8 as the encoders allow.
    fn codec_mode_support(&self) -> u32 {
        let mut mode = 0x1;
        if self.hevc {
            mode |= 0x100 | 0x200;
        }
        if self.av1 {
            mode |= 0x10000;
        }
        mode
    }

    /// nvhttp.cpp make_launch_session + rtsp_stream::launch_session_raise;
    /// the scheme of the session URL.
    fn launch_session_raise(&mut self, args: &HashMap<String, String>) -> &'static str {
        let key: [u8; 16] = hex_decode(&args["rikey"]).and_then(|k| k.try_into().ok()).expect("rikey of 16 bytes");
        let rikeyid = args["rikeyid"].parse::<i64>().expect("rikeyid") as i32 as u32;
        let rtsp_encrypted = args.get("corever").map_or(0, |v| v.parse().unwrap_or(0)) >= 1;
        let session = LaunchSession {
            key,
            rikeyid,
            rtsp_encrypted,
            rtsp_iv_counter: 0,
            av_ping_payload: hex_encode(&random_bytes::<8>().unwrap()).to_uppercase(),
            control_connect_data: u32::from_le_bytes(random_bytes::<4>().unwrap()) | 1,
        };
        // "If a launch event is still pending, don't overwrite it."
        if self.launch.is_none() {
            self.launch = Some(session);
        }
        if rtsp_encrypted {
            "rtspenc"
        } else {
            "rtsp"
        }
    }

    /// rtsp_stream::terminate_sessions
    fn terminate_sessions(&mut self) {
        if let Some(session) = &mut self.session {
            session.stopping = true;
        }
    }
}

impl StreamSession {
    /// stream.cpp session::stop: the control thread then drops the client's
    /// ENet peer without a message.
    pub(crate) fn stop(&mut self) {
        self.stopping = true;
    }
}

// --- crypto as the host does it (crypto.cpp cipher::gcm_t with a given IV) --

fn gcm_encrypt(key: &[u8; 16], iv: &[u8], plaintext: &[u8]) -> ([u8; 16], Vec<u8>) {
    let mut data = plaintext.to_vec();
    let tag: [u8; 16] = if iv.len() == 12 {
        Aes128Gcm::new(key.into()).encrypt_inout_detached(iv.try_into().unwrap(), &[], data.as_mut_slice().into()).unwrap().into()
    } else {
        AesGcm::<aes_gcm::aes::Aes128, U16>::new(key.into())
            .encrypt_inout_detached(iv.try_into().unwrap(), &[], data.as_mut_slice().into())
            .unwrap()
            .into()
    };
    (tag, data)
}

fn gcm_decrypt(key: &[u8; 16], iv: &[u8], tag: &[u8], ciphertext: &[u8]) -> Option<Vec<u8>> {
    let mut data = ciphertext.to_vec();
    let tag: [u8; 16] = tag.try_into().ok()?;
    if iv.len() == 12 {
        Aes128Gcm::new(key.into()).decrypt_inout_detached(iv.try_into().unwrap(), &[], data.as_mut_slice().into(), &tag.into()).ok()?;
    } else {
        AesGcm::<aes_gcm::aes::Aes128, U16>::new(key.into())
            .decrypt_inout_detached(iv.try_into().unwrap(), &[], data.as_mut_slice().into(), &tag.into())
            .ok()?;
    }
    Some(data)
}

// --- RTSP (rtsp.cpp) --------------------------------------------------------

/// rtsp.cpp socket_t::msg_buf
const RTSP_MSG_BUF: usize = 2048;

struct RtspRequest {
    method: String,
    target: String,
    headers: Vec<(String, String)>,
    payload: Vec<u8>,
    cseq: i64,
}

/// moonlight-common-c RtspParser.c parseRtspMessage, as far as a request needs it.
fn parse_rtsp_request(plain: &[u8]) -> Option<RtspRequest> {
    let head_end = plain.windows(4).position(|w| w == b"\r\n\r\n")?;
    let head = std::str::from_utf8(&plain[..head_end]).ok()?;
    let mut lines = head.split("\r\n");
    let mut words = lines.next()?.split(' ');
    let (method, target, protocol) = (words.next()?, words.next()?, words.next()?);
    if !protocol.starts_with("RTSP/") {
        return None;
    }
    let mut headers = Vec::new();
    for line in lines {
        let (name, value) = line.split_once(':')?;
        headers.push((name.to_string(), value.trim_start().to_string()));
    }
    let cseq = headers.iter().find(|(n, _)| n == "CSeq").and_then(|(_, v)| v.parse().ok()).unwrap_or(0);
    Some(RtspRequest { method: method.into(), target: target.into(), headers, payload: plain[head_end + 4..].to_vec(), cseq })
}

/// rtsp.cpp respond(): the serialized answer, sealed when the launch asked for it.
fn rtsp_respond(state: &Mutex<State>, code: u32, text: &str, options: &[(&str, String)], payload: &str) -> Vec<u8> {
    let mut plain = format!("RTSP/1.0 {code} {text}\r\n");
    for (name, value) in options {
        plain.push_str(&format!("{name}: {value}\r\n"));
    }
    plain.push_str("\r\n");
    plain.push_str(payload);
    let mut st = state.lock().unwrap();
    match st.stream.launch.as_mut() {
        Some(launch) if launch.rtsp_encrypted => {
            // "session.rtsp_iv_counter++; ... iv[10] = 'H'; iv[11] = 'R';"
            launch.rtsp_iv_counter += 1;
            let mut iv = [0u8; 12];
            iv[..4].copy_from_slice(&launch.rtsp_iv_counter.to_le_bytes());
            iv[10] = b'H';
            iv[11] = b'R';
            let (tag, ciphertext) = gcm_encrypt(&launch.key, &iv, plain.as_bytes());
            let mut message = (0x8000_0000u32 + plain.len() as u32).to_be_bytes().to_vec();
            message.extend_from_slice(&launch.rtsp_iv_counter.to_be_bytes());
            message.extend_from_slice(&tag);
            message.extend_from_slice(&ciphertext);
            message
        }
        _ => plain.into_bytes(),
    }
}

/// rtsp.cpp cmd_describe
fn rtsp_describe(stream: &StreamHost) -> String {
    let mut supported = SS_ENC_CONTROL_V2 | SS_ENC_AUDIO;
    let mut requested = SS_ENC_CONTROL_V2;
    if stream.encryption_mode != EncryptionMode::Never {
        supported |= SS_ENC_VIDEO;
        if stream.encryption_mode == EncryptionMode::Mandatory {
            requested |= SS_ENC_VIDEO | SS_ENC_AUDIO;
        }
    }
    let mut out = String::new();
    out.push_str("a=x-ss-general.featureFlags:3\n");
    out.push_str(&format!("a=x-ss-general.encryptionSupported:{supported}\n"));
    out.push_str(&format!("a=x-ss-general.encryptionRequested:{requested}\n"));
    out.push_str("a=x-nv-video[0].refPicInvalidation:1\n");
    if stream.hevc {
        out.push_str("sprop-parameter-sets=AAAAAU\n");
    }
    if stream.av1 {
        out.push_str("a=rtpmap:98 AV1/90000\n");
    }
    // audio::stream_configs: the stereo lines (the surround ones follow in a
    // real host; a stereo client does not read any of them).
    out.push_str("a=fmtp:97 surround-params=21101\n");
    out.push_str("a=fmtp:97 surround-params=21101\n");
    out
}

/// rtsp.cpp cmd_announce: the config, or the status to refuse with.
fn rtsp_announce(stream: &StreamHost, request: &RtspRequest) -> Result<StreamConfig, (u32, &'static str)> {
    let payload = String::from_utf8_lossy(&request.payload).into_owned();
    let mut args: HashMap<String, String> = HashMap::new();
    let mut client = String::new();
    for line in payload.split(['\r', '\n']).filter(|l| !l.is_empty()) {
        if let Some(rest) = line.strip_prefix("s=") {
            client = rest.to_string();
        } else if let Some(rest) = line.strip_prefix("a=") {
            let Some((name, value)) = rest.split_once(':') else { continue };
            // "if (val[val.size() - 1] == ' ') val = val.substr(0, val.size() - 1);"
            let value = value.strip_suffix(' ').unwrap_or(value);
            args.entry(name.to_string()).or_insert_with(|| value.to_string());
        }
    }
    // "Initialize any omitted parameters to defaults"
    for (name, value) in [
        ("x-nv-video[0].encoderCscMode", "0"),
        ("x-nv-vqos[0].bitStreamFormat", "0"),
        ("x-nv-video[0].dynamicRangeMode", "0"),
        ("x-nv-aqos.packetDuration", "5"),
        ("x-nv-general.useReliableUdp", "1"),
        ("x-nv-vqos[0].fec.minRequiredFecPackets", "0"),
        ("x-nv-general.featureFlags", "135"),
        ("x-ml-general.featureFlags", "0"),
        ("x-nv-vqos[0].qosTrafficType", "5"),
        ("x-nv-aqos.qosTrafficType", "4"),
        ("x-ml-video.configuredBitrateKbps", "0"),
        ("x-ss-general.encryptionEnabled", "0"),
        ("x-ss-video[0].chromaSamplingType", "0"),
        ("x-ss-video[0].intraRefresh", "0"),
        ("x-nv-video[0].clientRefreshRateX100", "0"),
    ] {
        args.entry(name.to_string()).or_insert_with(|| value.to_string());
    }
    // util::from_view over args.at(...): a missing one is std::out_of_range -> 400.
    let at = |name: &str| -> Result<i64, (u32, &'static str)> {
        let value = args.get(name).ok_or((400, "BAD REQUEST"))?;
        let end = value.find(|c: char| !c.is_ascii_digit() && c != '-').unwrap_or(value.len());
        Ok(value[..end].parse().unwrap_or(0))
    };
    let mut config = StreamConfig {
        audio_channels: at("x-nv-audio.surround.numChannels")?,
        audio_mask: at("x-nv-audio.surround.channelMask")?,
        audio_packet_duration: at("x-nv-aqos.packetDuration")?,
        audio_high_quality: at("x-nv-audio.surround.AudioQuality")? != 0,
        control_protocol_type: at("x-nv-general.useReliableUdp")?,
        packetsize: at("x-nv-video[0].packetSize")? as usize,
        min_required_fec_packets: at("x-nv-vqos[0].fec.minRequiredFecPackets")? as usize,
        ml_feature_flags: at("x-ml-general.featureFlags")? as u32,
        audio_qos_type: at("x-nv-aqos.qosTrafficType")?,
        video_qos_type: at("x-nv-vqos[0].qosTrafficType")?,
        encryption_flags_enabled: at("x-ss-general.encryptionEnabled")? as u32,
        height: at("x-nv-video[0].clientViewportHt")?,
        width: at("x-nv-video[0].clientViewportWd")?,
        framerate: at("x-nv-video[0].maxFPS")?,
        framerate_x100: at("x-nv-video[0].clientRefreshRateX100")?,
        bitrate: at("x-nv-vqos[0].bw.maximumBitrateKbps")?,
        slices_per_frame: at("x-nv-video[0].videoEncoderSlicesPerFrame")?,
        num_ref_frames: at("x-nv-video[0].maxNumReferenceFrames")?,
        encoder_csc_mode: at("x-nv-video[0].encoderCscMode")?,
        video_format: at("x-nv-vqos[0].bitStreamFormat")?,
        dynamic_range: at("x-nv-video[0].dynamicRangeMode")?,
        chroma_sampling_type: at("x-ss-video[0].chromaSamplingType")?,
        enable_intra_refresh: at("x-ss-video[0].intraRefresh")?,
        configured_bitrate_kbps: at("x-ml-video.configuredBitrateKbps")?,
        ..Default::default()
    };
    // "Legacy clients use nvFeatureFlags to indicate support for audio encryption"
    if at("x-nv-general.featureFlags")? & 0x20 != 0 {
        config.encryption_flags_enabled |= SS_ENC_AUDIO;
    }
    // "Discard framerateX100 if the derived fps is not within 1% of framerate."
    if config.framerate_x100 > 0 {
        let ratio = config.framerate_x100 as f64 / 100.0 / config.framerate as f64;
        if !(0.99..=1.01).contains(&ratio) {
            config.framerate_x100 = 0;
        }
    }
    // "the audio quality is (strangely) indicated by whether the Host field
    // in the RTSP message matches a local interface's IP address"
    if config.audio_channels == 2 {
        if let Some((_, host)) = request.headers.iter().find(|(n, _)| n == "Host") {
            config.audio_high_quality = !host.contains("0.0.0.0");
        }
    }
    if config.video_format == 1 && !stream.hevc {
        return Err((400, "BAD REQUEST"));
    }
    if config.video_format == 2 && !stream.av1 {
        return Err((400, "BAD REQUEST"));
    }
    // "Rejecting client that cannot comply with mandatory encryption requirement"
    if stream.encryption_mode == EncryptionMode::Mandatory
        && config.encryption_flags_enabled & (SS_ENC_VIDEO | SS_ENC_AUDIO) != (SS_ENC_VIDEO | SS_ENC_AUDIO)
    {
        return Err((403, "Forbidden"));
    }
    config.args = args;
    config.client = client;
    Ok(config)
}

/// rtsp.cpp socket_t::read and its handlers: one request, or `Err` with
/// what the host answers to a bad one.
async fn rtsp_read(stream: &mut TcpStream, launch: &LaunchSession) -> Result<(Vec<u8>, usize, u32), ()> {
    if launch.rtsp_encrypted {
        // handle_read_encrypted_header
        let mut header = [0u8; 24];
        stream.read_exact(&mut header).await.map_err(|_| ())?;
        let type_and_length = u32::from_be_bytes(header[0..4].try_into().unwrap());
        if type_and_length & 0x8000_0000 == 0 {
            return Err(()); // "Rejecting unencrypted RTSP message"
        }
        let length = (type_and_length & !0x8000_0000) as usize;
        if 24 + length >= RTSP_MSG_BUF {
            return Err(()); // "Exceeded maximum rtsp packet size"
        }
        // handle_read_encrypted_message
        let mut ciphertext = vec![0u8; length];
        stream.read_exact(&mut ciphertext).await.map_err(|_| ())?;
        let seq = u32::from_be_bytes(header[4..8].try_into().unwrap());
        let mut iv = [0u8; 12];
        iv[..4].copy_from_slice(&seq.to_le_bytes());
        iv[10] = b'C';
        iv[11] = b'R';
        let plain = gcm_decrypt(&launch.key, &iv, &header[8..24], &ciphertext).ok_or(())?; // "Failed to verify RTSP message tag"
        Ok((plain, 24 + length, seq))
    } else {
        // handle_read_plaintext / handle_plaintext_payload
        let mut buf = Vec::new();
        let mut chunk = [0u8; RTSP_MSG_BUF];
        loop {
            if buf.len() >= RTSP_MSG_BUF {
                return Err(());
            }
            let room = RTSP_MSG_BUF - buf.len();
            let n = stream.read(&mut chunk[..room]).await.map_err(|_| ())?;
            if n == 0 {
                return Err(());
            }
            buf.extend_from_slice(&chunk[..n]);
            let Some(end) = buf.windows(4).position(|w| w == b"\r\n\r\n") else { continue };
            let head = String::from_utf8_lossy(&buf[..end]).into_owned();
            // `"Content-length"sv == option->option`: this exact spelling.
            let content_length: usize = head
                .split("\r\n")
                .find_map(|l| l.strip_prefix("Content-length:"))
                .and_then(|v| v.trim().parse().ok())
                .unwrap_or(0);
            if buf.len() - (end + 4) >= content_length {
                let len = buf.len();
                return Ok((buf, len, 0));
            }
        }
    }
}

async fn rtsp_connection(state: Arc<Mutex<State>>, mut stream: TcpStream) {
    // handle_accept: "If there is no session pending, close the connection immediately"
    let Some(launch) = state.lock().unwrap().stream.launch.clone() else { return };
    let answer = match rtsp_read(&mut stream, &launch).await {
        Err(()) => rtsp_respond(&state, 400, "BAD REQUEST", &[], ""),
        Ok((plain, wire_len, seq)) => match parse_rtsp_request(&plain) {
            None => rtsp_respond(&state, 400, "BAD REQUEST", &[], ""),
            Some(request) => {
                let cseq = ("CSeq", request.cseq.to_string());
                let mut st = state.lock().unwrap();
                st.stream.rtsp.push(RtspSeen {
                    method: request.method.clone(),
                    target: request.target.clone(),
                    headers: request.headers.clone(),
                    payload: String::from_utf8_lossy(&request.payload).into_owned(),
                    encrypted: launch.rtsp_encrypted,
                    wire_len,
                    seq,
                });
                let (code, text, options, payload): (u32, &str, Vec<(&str, String)>, String) = match request.method.as_str() {
                    "OPTIONS" | "PLAY" => (200, "OK", vec![cseq], String::new()),
                    "DESCRIBE" => (200, "OK", vec![cseq], rtsp_describe(&st.stream)),
                    "SETUP" => {
                        // cmd_setup: the word between '=' and '/'
                        let kind = request.target.split_once('=').map(|(_, r)| r.split('/').next().unwrap_or("")).unwrap_or("");
                        let port = match kind {
                            "audio" => Some(st.stream.audio_port),
                            "video" => Some(st.stream.video_port),
                            "control" => Some(st.stream.control_port),
                            _ => None,
                        };
                        match port {
                            None => (404, "NOT FOUND", vec![], String::new()),
                            Some(port) => {
                                let extra = if kind == "control" {
                                    ("X-SS-Connect-Data", launch.control_connect_data.to_string())
                                } else {
                                    ("X-SS-Ping-Payload", launch.av_ping_payload.clone())
                                };
                                let options = vec![
                                    cseq,
                                    ("Session", "DEADBEEFCAFE;timeout = 90".to_string()),
                                    ("Transport", format!("server_port={port}")),
                                    extra,
                                ];
                                (200, "OK", options, String::new())
                            }
                        }
                    }
                    "ANNOUNCE" => match rtsp_announce(&st.stream, &request) {
                        Err((code, text)) => (code, text, vec![cseq], String::new()),
                        Ok(config) => {
                            // stream::session::alloc + start
                            let video_key = (config.encryption_flags_enabled & SS_ENC_VIDEO != 0).then_some(launch.key);
                            let audio_encrypted = config.encryption_flags_enabled & SS_ENC_AUDIO != 0;
                            st.stream.session = Some(StreamSession {
                                video: VideoHost::with_key(config.packetsize, video_key, config.min_required_fec_packets),
                                audio: AudioHost::with_key(audio_encrypted, launch.key, launch.rikeyid),
                                launch: launch.clone(),
                                config,
                                video_peer: None,
                                audio_peer: None,
                                control_seq: 0,
                                control_peer: None,
                                stopping: false,
                            });
                            st.stream.sessions_started += 1;
                            (200, "OK", vec![cseq], String::new())
                        }
                    },
                    _ => (404, "NOT FOUND", vec![], String::new()),
                };
                drop(st);
                rtsp_respond(&state, code, text, &options, &payload)
            }
        },
    };
    let _ = stream.write_all(&answer).await;
    // handle_msg: sock.shutdown(shutdown_both)
    let _ = stream.shutdown().await;
}

// --- input (input.cpp) ------------------------------------------------------

/// input.cpp validate_input_packet + the passthrough overloads.
pub(crate) fn read_input_packet(packet: &[u8]) -> Result<InputSeen, ()> {
    if packet.len() < 8 {
        return Err(());
    }
    let declared = u32::from_be_bytes(packet[0..4].try_into().unwrap()) as usize;
    let magic = u32::from_le_bytes(packet[4..8].try_into().unwrap());
    if declared < 4 || declared > packet.len() - 4 {
        return Err(());
    }
    // validate_fixed_input_packet<Packet>: sizeof(Packet) by Input.h
    let fixed = |size_of: usize| if declared == size_of - 4 && packet.len() >= size_of { Ok(()) } else { Err(()) };
    let be16 = |at: usize| i16::from_be_bytes([packet[at], packet[at + 1]]);
    match magic {
        0x07 => {
            fixed(12)?;
            Ok(InputSeen::MoveRelative { dx: be16(8), dy: be16(10) })
        }
        0x05 => {
            fixed(18)?;
            Ok(InputSeen::MoveAbsolute { x: be16(8), y: be16(10), width: be16(14), height: be16(16) })
        }
        0x08 | 0x09 => {
            fixed(9)?;
            Ok(InputSeen::Button { button: packet[8], release: magic == 0x09 })
        }
        0x0A => {
            fixed(14)?;
            Ok(InputSeen::Scroll { amount: be16(8) })
        }
        0x5500_0001 => {
            fixed(10)?;
            Ok(InputSeen::HScroll { amount: be16(8) })
        }
        0x03 | 0x04 => {
            fixed(14)?;
            // "auto keyCode = packet->keyCode & 0x00FF;" (little-endian on the wire)
            Ok(InputSeen::Key { vk: packet[9], release: magic == 0x04, modifiers: packet[11], flags: packet[8] })
        }
        0x17 => {
            if declared - 4 > 32 {
                return Err(());
            }
            Ok(InputSeen::Text(String::from_utf8_lossy(&packet[8..4 + declared]).into_owned()))
        }
        // The controller packets are read as the host's (little-endian) structs.
        0x5500_0004 => {
            fixed(16)?;
            Ok(InputSeen::ControllerArrival {
                number: packet[8],
                kind: packet[9],
                capabilities: u16::from_le_bytes([packet[10], packet[11]]),
                buttons: u32::from_le_bytes(packet[12..16].try_into().unwrap()),
            })
        }
        0x0C => {
            fixed(34)?;
            let le16 = |at: usize| u16::from_le_bytes([packet[at], packet[at + 1]]);
            Ok(InputSeen::Controller {
                number: le16(10) as i16,
                active_mask: le16(12),
                buttons: le16(16) as u32 | ((le16(30) as u32) << 16),
                lt: packet[18],
                rt: packet[19],
                sticks: [le16(20) as i16, le16(22) as i16, le16(24) as i16, le16(26) as i16],
            })
        }
        other => Ok(InputSeen::Other { magic: other }),
    }
}

/// libvirtualhid src/platform/linux/uhid_backend.cpp key_code_to_linux: what
/// a Linux host of this version presses for a Windows virtual-key code.
pub(crate) fn linux_key_of_vk(vk: u8) -> Option<u32> {
    const SPECIAL: &[(u8, u32)] = &[
        (0x08, 14), (0x09, 15), (0x0D, 28), (0x10, 42), (0xA0, 42), (0x11, 29), (0xA2, 29), (0x12, 56), (0xA4, 56),
        (0x14, 58), (0x1B, 1), (0x20, 57), (0x21, 104), (0x22, 109), (0x23, 107), (0x24, 102), (0x25, 105),
        (0x26, 103), (0x27, 106), (0x28, 108), (0x2C, 99), (0x2D, 110), (0x2E, 111), (0x5B, 125), (0x5C, 126),
        (0x6A, 55), (0x6B, 78), (0x6D, 74), (0x6E, 83), (0x6F, 98), (0x90, 69), (0x91, 70), (0xA1, 54), (0xA3, 97),
        (0xA5, 100), (0xBA, 39), (0xBB, 13), (0xBC, 51), (0xBD, 12), (0xBE, 52), (0xBF, 53), (0xC0, 41), (0xDB, 26),
        (0xDC, 43), (0xDD, 27), (0xDE, 40), (0xE2, 86),
    ];
    // KEY_0, KEY_1..KEY_9
    const DIGITS: [u32; 10] = [11, 2, 3, 4, 5, 6, 7, 8, 9, 10];
    // KEY_A..KEY_Z
    const LETTERS: [u32; 26] = [30, 48, 46, 32, 18, 33, 34, 35, 23, 36, 37, 38, 50, 49, 24, 25, 16, 19, 31, 20, 22, 47, 17, 45, 21, 44];
    // KEY_KP0..KEY_KP9
    const KEYPAD: [u32; 10] = [82, 79, 80, 81, 75, 76, 77, 71, 72, 73];
    // KEY_F1..KEY_F24
    const FUNCTION: [u32; 24] = [59, 60, 61, 62, 63, 64, 65, 66, 67, 68, 87, 88, 183, 184, 185, 186, 187, 188, 189, 190, 191, 192, 193, 194];
    if let Some((_, key)) = SPECIAL.iter().find(|(code, _)| *code == vk) {
        return Some(*key);
    }
    match vk {
        0x30..=0x39 => Some(DIGITS[(vk - 0x30) as usize]),
        0x41..=0x5A => Some(LETTERS[(vk - 0x41) as usize]),
        0x60..=0x69 => Some(KEYPAD[(vk - 0x60) as usize]),
        0x70..=0x87 => Some(FUNCTION[(vk - 0x70) as usize]),
        _ => None,
    }
}

/// libvirtualhid src/platform/windows/keylayout.hpp
/// windows_us_english_virtual_key_to_scan_code.
#[rustfmt::skip]
const WINDOWS_US_SCAN_CODES: [u8; 256] = [
    0, 0, 0, 70, 0, 0, 0, 0, 14, 15, 0, 0, 76, 28, 0, 0,
    42, 29, 56, 0, 58, 0, 0, 0, 0, 0, 0, 1, 0, 0, 0, 0,
    57, 73, 81, 79, 71, 75, 72, 77, 80, 0, 0, 0, 84, 82, 83, 99,
    11, 2, 3, 4, 5, 6, 7, 8, 9, 10, 0, 0, 0, 0, 0, 0,
    0, 30, 48, 46, 32, 18, 33, 34, 35, 23, 36, 37, 38, 50, 49, 24,
    25, 16, 19, 31, 20, 22, 47, 17, 45, 21, 44, 91, 92, 93, 0, 95,
    82, 79, 80, 81, 75, 76, 77, 71, 72, 73, 55, 78, 0, 74, 83, 53,
    59, 60, 61, 62, 63, 64, 65, 66, 67, 68, 87, 88, 100, 101, 102, 103,
    104, 105, 106, 107, 108, 109, 110, 118, 0, 0, 0, 0, 0, 0, 0, 0,
    69, 70, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    42, 54, 29, 29, 56, 56, 106, 105, 103, 104, 101, 102, 50, 32, 46, 48,
    25, 16, 36, 34, 108, 109, 107, 33, 0, 0, 39, 13, 51, 12, 52, 53,
    41, 115, 126, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 26, 43, 27, 40, 0,
    0, 0, 86, 0, 0, 0, 0, 0, 0, 113, 92, 123, 0, 111, 90, 0,
    0, 91, 0, 95, 0, 94, 0, 0, 0, 93, 0, 98, 0, 0, 0, 0,
];

/// libvirtualhid src/platform/windows/windows_backend.cpp
/// WindowsKeyboard::submit for an event without a scan code of its own:
/// `(scan code, extended, by virtual key)`: with `normalized` (the packet's
/// flags lack SS_KBE_FLAG_NON_NORMALIZED) the US table's scan code, sent
/// with KEYEVENTF_SCANCODE; without it (and with the host's
/// `always_send_scancodes` setting off), or when the table has none, the
/// virtual-key code itself. Extended as extended_key() says.
pub(crate) fn windows_key_of_vk(vk: u8, normalized: bool) -> (u8, bool, bool) {
    let scan = if normalized { WINDOWS_US_SCAN_CODES[vk as usize] } else { 0 };
    // VK_LWIN, VK_RWIN, VK_RMENU, VK_RCONTROL, VK_INSERT, VK_DELETE, VK_HOME, VK_END,
    // VK_PRIOR, VK_NEXT, VK_UP, VK_DOWN, VK_LEFT, VK_RIGHT, VK_DIVIDE, VK_APPS
    let extended = matches!(vk, 0x5B | 0x5C | 0xA5 | 0xA3 | 0x2D | 0x2E | 0x24 | 0x23 | 0x21 | 0x22 | 0x26 | 0x28 | 0x25 | 0x27 | 0x6F | 0x5D);
    (scan, extended, scan == 0)
}

// --- control (stream.cpp) and the UDP sockets -------------------------------

/// stream.cpp IDX_ENCRYPTED handler: (seq, type, payload after the inner header).
fn control_decrypt(session: &StreamSession, packet: &[u8]) -> Option<(u32, u16, Vec<u8>)> {
    // "auto header = (control_encrypted_p) (payload.data() - 2);"
    if packet.len() < 8 {
        return None;
    }
    let length = u16::from_le_bytes([packet[2], packet[3]]) as usize;
    let seq = u32::from_le_bytes(packet[4..8].try_into().unwrap());
    if length < 16 + 4 + 4 || packet.len() < 4 + length {
        return None; // "Control: Runt packet"
    }
    let tagged = &packet[8..4 + length];
    let iv: Vec<u8> = if session.config.encryption_flags_enabled & SS_ENC_CONTROL_V2 != 0 {
        let mut iv = vec![0u8; 12];
        iv[..4].copy_from_slice(&seq.to_le_bytes());
        iv[10] = b'C';
        iv[11] = b'C';
        iv
    } else {
        let mut iv = vec![0u8; 16];
        iv[0] = seq as u8;
        iv
    };
    let plain = gcm_decrypt(&session.launch.key, &iv, &tagged[..16], &tagged[16..])?;
    let ptype = u16::from_le_bytes([plain[0], plain[1]]);
    Some((seq, ptype, plain[4..].to_vec()))
}

/// stream.cpp encode_control
fn control_encrypt(session: &mut StreamSession, ptype: u16, payload: &[u8]) -> Vec<u8> {
    let seq = session.control_seq;
    session.control_seq += 1;
    let iv: Vec<u8> = if session.config.encryption_flags_enabled & SS_ENC_CONTROL_V2 != 0 {
        let mut iv = vec![0u8; 12];
        iv[..4].copy_from_slice(&seq.to_le_bytes());
        iv[10] = b'H';
        iv[11] = b'C';
        iv
    } else {
        let mut iv = vec![0u8; 16];
        iv[0] = seq as u8;
        iv
    };
    // control_header_v2 { type, payloadLength } + payload
    let mut plain = ptype.to_le_bytes().to_vec();
    plain.extend_from_slice(&(payload.len() as u16).to_le_bytes());
    plain.extend_from_slice(payload);
    let (tag, ciphertext) = gcm_encrypt(&session.launch.key, &iv, &plain);
    let length = (ciphertext.len() + 16 + 4) as u16;
    let mut out = 0x0001u16.to_le_bytes().to_vec();
    out.extend_from_slice(&length.to_le_bytes());
    out.extend_from_slice(&seq.to_le_bytes());
    out.extend_from_slice(&tag);
    out.extend_from_slice(&ciphertext);
    out
}

/// stream.cpp control_server_t::iterate / call and the handlers of
/// controlBroadcastThread for one received packet.
fn control_received(stream: &mut StreamHost, channel: u8, packet: &[u8]) {
    let Some(session) = stream.session.as_ref() else { return };
    if packet.len() < 2 {
        return;
    }
    let outer = u16::from_le_bytes([packet[0], packet[1]]);
    // "Dropping unencrypted message on encrypted control stream"
    if session.config.control_protocol_type == 13 && outer != 0x0001 {
        stream.control_rejected += 1;
        return;
    }
    let Some((seq, ptype, payload)) = control_decrypt(session, packet) else {
        // "Failed to verify tag" -> session::stop
        stream.control_rejected += 1;
        return;
    };
    stream.control.push(ControlSeen { channel, ptype, payload: payload.clone(), seq });
    match ptype {
        0x0200 => stream.periodic_pings += 1,
        0x0302 => stream.idr_requests += 1,
        0x0307 => stream.start_b += 1,
        0x5502 => stream.fec_status.push(payload),
        // "IDX_INPUT_DATA callback will attempt to decrypt unencrypted data, therefore we need pass it directly"
        0x0206 => match read_input_packet(&payload) {
            Ok(seen) => {
                // input.cpp passthrough(PNV_KEYBOARD_PACKET), update_shortcutFlags
                // and apply_shortcut: with Ctrl, Alt and Shift held, a new
                // press of N turns `display_cursor` (globals.cpp, one for the
                // whole program, never put back by a session's end) over.
                if let InputSeen::Key { vk, release, .. } = &seen {
                    let bit = match *vk {
                        0x10 | 0xA0 | 0xA1 => 0x4, // input_t::SHIFT
                        0x11 | 0xA2 | 0xA3 => 0x1, // input_t::CTRL
                        0x12 | 0xA4 | 0xA5 => 0x2, // input_t::ALT
                        _ => 0,
                    };
                    if !*release && bit == 0 && stream.shortcut_flags == 0x7 && *vk == 0x4E {
                        stream.display_cursor = !stream.display_cursor;
                        stream.cursor_toggles += 1;
                    }
                    if *release {
                        stream.shortcut_flags &= !bit;
                    } else {
                        stream.shortcut_flags |= bit;
                    }
                }
                stream.input.push(seen)
            }
            Err(()) => stream.malformed_input += 1,
        },
        _ => {}
    }
}

async fn control_server(state: Arc<Mutex<State>>, socket: Arc<UdpSocket>) {
    // network.cpp host_create: enet_host_create(af, &addr, 128, 0, 0, 0)
    let mut host = enet::Host::new(
        EnetSocket::new(socket.clone(), false),
        enet::HostSettings { peer_limit: 128, channel_limit: 255, ..Default::default() },
    )
    .unwrap();
    // cgutman's enet.h ENET_HOST_DEFAULT_MTU
    let _ = host.set_mtu(900);
    loop {
        loop {
            let event = match host.service() {
                Ok(Some(event)) => event,
                Ok(None) => break,
                Err(_) => return,
            };
            let mut st = state.lock().unwrap();
            match event {
                enet::Event::Connect { peer, data } => {
                    // control_server_t::get_session: by connect data when the
                    // client declared ML_FF_SESSION_ID_V1 (this host has no
                    // other way: every test client is 127.0.0.1).
                    let fits = st.stream.session.as_ref().is_some_and(|s| {
                        s.control_peer.is_none()
                            && s.config.ml_feature_flags & ML_FF_SESSION_ID_V1 != 0
                            && s.launch.control_connect_data == data
                    });
                    if fits {
                        st.stream.session.as_mut().unwrap().control_peer = Some(peer.id());
                        st.stream.control_connects += 1;
                        // "Once the control stream connection is established, RTSP session state can be torn down"
                        st.stream.launch = None;
                    } else {
                        // "Rejected connection from [...]: it's not properly set up"
                        st.stream.control_refused += 1;
                        peer.disconnect_now(0);
                    }
                }
                enet::Event::Receive { channel_id, packet, .. } => {
                    // (Whether it came reliably cannot be told here: an ENet
                    // host learns it from the command, and rusty_enet 0.4's
                    // Packet::kind() answers "reliable" for every packet.)
                    control_received(&mut st.stream, channel_id, packet.data());
                }
                enet::Event::Disconnect { .. } => {
                    // "CLIENT DISCONNECTED" -> session::stop
                    st.stream.control_disconnects += 1;
                    st.stream.session = None;
                }
            }
        }
        {
            let mut st = state.lock().unwrap();
            let stream = &mut st.stream;
            let outgoing = std::mem::take(&mut stream.outgoing);
            if let Some(session) = stream.session.as_mut() {
                if let Some(peer) = session.control_peer {
                    for (ptype, payload) in outgoing {
                        // control_server_t::send: channel 0, reliable
                        let message = control_encrypt(session, ptype, &payload);
                        let _ = host.peer_mut(peer).send(0, &enet::Packet::reliable(message.as_slice()));
                    }
                    // controlBroadcastThread at its end: the termination
                    // messages, `server->flush()`, and soon after the peers
                    // are dropped (network.cpp free_host).
                    host.flush();
                    if session.stopping {
                        // controlBroadcastThread: a STOPPING session is erased and its peer dropped
                        host.peer_mut(peer).disconnect_now(0);
                        stream.session = None;
                    }
                } else if session.stopping {
                    stream.session = None;
                }
            }
            host.flush();
        }
        let _ = tokio::time::timeout(Duration::from_millis(2), socket.readable()).await;
    }
}

/// stream.cpp recvThread: a ping tells where a session's video or audio goes.
async fn ping_receiver(state: Arc<Mutex<State>>, socket: Arc<UdpSocket>, video: bool) {
    let mut buf = [0u8; 2048];
    loop {
        let Ok((n, from)) = socket.recv_from(&mut buf).await else { continue };
        let mut st = state.lock().unwrap();
        let stream = &mut st.stream;
        let Some(session) = stream.session.as_mut() else { continue };
        let v1 = session.config.ml_feature_flags & ML_FF_SESSION_ID_V1 != 0;
        // "For legacy PING packets" (4 bytes, only without ML_FF_SESSION_ID_V1) /
        // "For new PING packets that include a client identifier, search by payload."
        let fits = if n == 4 {
            !v1 && &buf[..4] == b"PING"
        } else {
            n >= 20 && buf[..16] == *session.launch.av_ping_payload.as_bytes()
        };
        if !fits {
            continue;
        }
        if video {
            session.video_peer = Some(from);
            stream.video_pings += 1;
            if n >= 20 {
                stream.last_video_ping_seq = u32::from_be_bytes(buf[16..20].try_into().unwrap());
            }
        } else {
            session.audio_peer = Some(from);
            stream.audio_pings += 1;
        }
    }
}

async fn start_stream_side(
    state: &Arc<Mutex<State>>,
    fixed: &FixedPorts,
) -> (Vec<tokio::task::JoinHandle<()>>, Arc<UdpSocket>, Arc<UdpSocket>) {
    let rtsp = TcpListener::bind(("127.0.0.1", fixed.rtsp)).await.unwrap();
    let video = Arc::new(UdpSocket::bind(("127.0.0.1", fixed.video)).await.unwrap());
    let audio = Arc::new(UdpSocket::bind(("127.0.0.1", fixed.audio)).await.unwrap());
    let control = Arc::new(UdpSocket::bind(("127.0.0.1", fixed.control)).await.unwrap());
    {
        let mut st = state.lock().unwrap();
        st.stream.rtsp_port = rtsp.local_addr().unwrap().port();
        st.stream.video_port = video.local_addr().unwrap().port();
        st.stream.audio_port = audio.local_addr().unwrap().port();
        st.stream.control_port = control.local_addr().unwrap().port();
    }
    let st = state.clone();
    let rtsp_task = tokio::spawn(async move {
        loop {
            let Ok((stream, _)) = rtsp.accept().await else { return };
            tokio::spawn(rtsp_connection(st.clone(), stream));
        }
    });
    let tasks = vec![
        rtsp_task,
        tokio::spawn(control_server(state.clone(), control)),
        tokio::spawn(ping_receiver(state.clone(), video.clone(), true)),
        tokio::spawn(ping_receiver(state.clone(), audio.clone(), false)),
    ];
    (tasks, video, audio)
}

impl FakeHost {
    /// Makes the host know the tests' shared client without the pairing
    /// exchange (as if it had been paired before): the identity to use and
    /// the host's certificate.
    pub(crate) fn pair_directly(&self, unique_id: &str) -> (super::http::Identity, String) {
        let mut identity = super::http::tests::test_identity().clone();
        identity.unique_id = unique_id.to_string();
        let der = cert_der_from_pem(&identity.client_cert_pem).unwrap();
        self.state.lock().unwrap().paired.push(der);
        (identity, host_keys().cert_pem.clone())
    }

    /// Waits (up to 10 s) until `done` holds for the host's state.
    pub(crate) async fn wait(&self, what: &str, done: impl Fn(&State) -> bool) {
        for _ in 0..2000 {
            if done(&self.state.lock().unwrap()) {
                return;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        panic!("the host waited in vain for: {what}");
    }

    /// stream.cpp videoBroadcastThread for one encoded frame; the datagrams
    /// whose index is in `lose` are not sent. Returns how many there were.
    pub(crate) async fn send_video(&self, frame: &[u8], idr: bool, lose: &[usize]) -> usize {
        let (datagrams, peer, path_mtu) = {
            let mut st = self.state.lock().unwrap();
            let path_mtu = st.stream.path_mtu;
            let session = st.stream.session.as_mut().expect("a streaming session");
            (session.video.send(frame, idr).all(), session.video_peer.expect("a video ping"), path_mtu)
        };
        for (index, datagram) in datagrams.iter().enumerate() {
            // 20 bytes of IPv4 and 8 of UDP around it.
            if datagram.len() + 28 > path_mtu {
                self.state.lock().unwrap().stream.video_too_big += 1;
                continue;
            }
            if !lose.contains(&index) {
                self.video_socket.send_to(datagram, peer).await.unwrap();
            }
        }
        datagrams.len()
    }

    /// stream.cpp audioBroadcastThread for four Opus packets (one FEC block
    /// with its two parity packets); the Opus packets as given to it.
    pub(crate) async fn send_audio_block(&self, seed: u8, size: usize) -> Vec<Vec<u8>> {
        let mut packets = Vec::new();
        let mut datagrams = Vec::new();
        let peer = {
            let mut st = self.state.lock().unwrap();
            let session = st.stream.session.as_mut().expect("a streaming session");
            for i in 0..4u8 {
                let mut opus: Vec<u8> = (0..size).map(|n| (n as u8).wrapping_mul(31).wrapping_add(seed).wrapping_add(i)).collect();
                opus[0] = 0xFC; // an Opus TOC byte
                datagrams.extend(session.audio.send(&opus));
                packets.push(opus);
            }
            session.audio_peer.expect("an audio ping")
        };
        for datagram in &datagrams {
            self.audio_socket.send_to(datagram, peer).await.unwrap();
        }
        packets
    }

    /// A control message to the client (stream.cpp encode_control + control_server_t::send).
    pub(crate) fn send_control(&self, ptype: u16, payload: &[u8]) {
        self.state.lock().unwrap().stream.outgoing.push((ptype, payload.to_vec()));
    }
}


#[cfg(test)]
#[path = "fake_host_e2e.rs"]
mod e2e;
