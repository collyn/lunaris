//! The Lunaris server's own certificate, accepted by its fingerprint.
//!
//! A server on its own HTTPS port with the self-signed certificate it made
//! (server/src/tls.rs) is not trusted by the system. The web page was opened
//! over it (the browser was told to accept it), and the link it makes for
//! this app carries that certificate's SHA-256 (`cert_sha256`, from the
//! server's link ticket). Here the server is accepted when the system
//! trusts its certificate, or when the certificate's SHA-256 is that one;
//! never otherwise (the handshake's signatures are always checked, so the
//! server holds the key). Without it the app refused the server ("UnknownCA")
//! and said the link had expired (2026-11-25).

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::client::WebPkiServerVerifier;
use rustls::crypto::{CryptoProvider, WebPkiSupportedAlgorithms};
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::{DigitallySignedStruct, RootCertStore, SignatureScheme};

/// `cert_sha256` of a link: 64 hex digits, as lower case; anything else is
/// not a fingerprint.
pub fn parse_fingerprint(text: &str) -> Option<String> {
    let t = text.trim();
    (t.len() == 64 && t.bytes().all(|b| b.is_ascii_hexdigit())).then(|| t.to_ascii_lowercase())
}

pub fn fingerprint(der: &[u8]) -> String {
    ring::digest::digest(&ring::digest::SHA256, der).as_ref().iter().map(|b| format!("{b:02x}")).collect()
}

/// The fingerprints links gave, by the server's host and port.
fn pins() -> &'static Mutex<HashMap<(String, u16), String>> {
    static PINS: OnceLock<Mutex<HashMap<(String, u16), String>>> = OnceLock::new();
    PINS.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Host (lower case) and port of a server address (`wss://h:p`, `https://h:p`).
fn server_of(url: &str) -> Option<(String, u16)> {
    let u = url::Url::parse(url.trim()).ok()?;
    Some((u.host_str()?.trim_end_matches('.').to_ascii_lowercase(), u.port_or_known_default()?))
}

/// A link to `server` said its certificate's SHA-256 is `pin`: connections
/// to that host and port accept it from now on (for this run of the app).
pub fn remember(server: &str, pin: &str) {
    if let Some(key) = server_of(server) {
        pins().lock().unwrap_or_else(|e| e.into_inner()).insert(key, pin.to_string());
    }
}

/// The fingerprint a link gave for the server of `url`.
pub fn pin_for(url: &str) -> Option<String> {
    let key = server_of(url)?;
    pins().lock().unwrap_or_else(|e| e.into_inner()).get(&key).cloned()
}

fn provider() -> Arc<CryptoProvider> {
    Arc::new(rustls::crypto::ring::default_provider())
}

fn system_roots() -> Option<Arc<RootCertStore>> {
    static ROOTS: OnceLock<Option<Arc<RootCertStore>>> = OnceLock::new();
    ROOTS
        .get_or_init(|| {
            let mut store = RootCertStore::empty();
            store.add_parsable_certificates(rustls_native_certs::load_native_certs().certs);
            (!store.is_empty()).then(|| Arc::new(store))
        })
        .clone()
}

/// Trusted by the system, or this exact certificate.
#[derive(Debug)]
struct PinnedVerifier {
    roots: Option<Arc<WebPkiServerVerifier>>,
    algorithms: WebPkiSupportedAlgorithms,
    pin: String,
}

impl ServerCertVerifier for PinnedVerifier {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        intermediates: &[CertificateDer<'_>],
        server_name: &ServerName<'_>,
        ocsp_response: &[u8],
        now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        if fingerprint(end_entity.as_ref()) == self.pin {
            return Ok(ServerCertVerified::assertion());
        }
        match &self.roots {
            Some(roots) => roots.verify_server_cert(end_entity, intermediates, server_name, ocsp_response, now),
            None => Err(rustls::Error::InvalidCertificate(rustls::CertificateError::UnknownIssuer)),
        }
    }

    fn verify_tls12_signature(&self, message: &[u8], cert: &CertificateDer<'_>, dss: &DigitallySignedStruct) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(message, cert, dss, &self.algorithms)
    }

    fn verify_tls13_signature(&self, message: &[u8], cert: &CertificateDer<'_>, dss: &DigitallySignedStruct) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(message, cert, dss, &self.algorithms)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.algorithms.supported_schemes()
    }
}

/// A TLS config that accepts the system's trusted certificates and the one
/// with SHA-256 `pin` (lower-case hex, `parse_fingerprint`).
pub fn client_config(pin: &str) -> Result<rustls::ClientConfig, rustls::Error> {
    let provider = provider();
    let roots = system_roots().and_then(|r| WebPkiServerVerifier::builder_with_provider(r, provider.clone()).build().ok());
    let verifier = Arc::new(PinnedVerifier { roots, algorithms: provider.signature_verification_algorithms, pin: pin.to_string() });
    Ok(rustls::ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()?
        .dangerous()
        .with_custom_certificate_verifier(verifier)
        .with_no_client_auth())
}

/// Opens the signaling WebSocket `url`: over TLS that also accepts the
/// certificate a link gave for its server; otherwise as before.
pub async fn connect_ws(
    url: &str,
) -> Result<tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>, tokio_tungstenite::tungstenite::Error> {
    match pin_for(url) {
        Some(pin) if url.starts_with("wss://") => {
            let config = client_config(&pin).map_err(|e| tokio_tungstenite::tungstenite::Error::Io(std::io::Error::other(e)))?;
            let connector = tokio_tungstenite::Connector::Rustls(Arc::new(config));
            Ok(tokio_tungstenite::connect_async_tls_with_config(url, None, false, Some(connector)).await?.0)
        }
        _ => Ok(tokio_tungstenite::connect_async(url).await?.0),
    }
}

/// POSTs `body` (JSON) to `https://host:port/path` over TLS that also accepts
/// the certificate `pin`; answers the status and the body. A small
/// HTTP/1.1 request of its own: the HTTP client the app uses elsewhere
/// cannot accept one certificate by its fingerprint.
pub async fn post_json(host: &str, port: u16, path: &str, pin: &str, body: &serde_json::Value, timeout: std::time::Duration) -> Result<(u16, serde_json::Value), String> {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let work = async {
        let config = client_config(pin).map_err(|e| e.to_string())?;
        let name_text = host.trim_start_matches('[').trim_end_matches(']').to_string();
        let server_name = ServerName::try_from(name_text).map_err(|e| e.to_string())?;
        let tcp = tokio::net::TcpStream::connect((host.trim_start_matches('[').trim_end_matches(']'), port)).await.map_err(|e| e.to_string())?;
        let mut tls = tokio_rustls::TlsConnector::from(Arc::new(config)).connect(server_name, tcp).await.map_err(|e| e.to_string())?;
        let body = body.to_string();
        let request = format!(
            "POST {path} HTTP/1.1\r\nHost: {host}:{port}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\nUser-Agent: lunaris-client\r\n\r\n{body}",
            body.len()
        );
        tls.write_all(request.as_bytes()).await.map_err(|e| e.to_string())?;
        let mut answer = Vec::new();
        // The server may end TLS without close_notify: what came is enough.
        let _ = tls.take(1024 * 1024).read_to_end(&mut answer).await;
        parse_response(&answer)
    };
    tokio::time::timeout(timeout, work).await.map_err(|_| "timed out".to_string())?
}

/// The status and the JSON body of an HTTP/1.1 answer (a whole body, or a
/// chunked one).
fn parse_response(answer: &[u8]) -> Result<(u16, serde_json::Value), String> {
    let split = answer.windows(4).position(|w| w == b"\r\n\r\n").ok_or("no answer")?;
    let head = std::str::from_utf8(&answer[..split]).map_err(|_| "not HTTP")?;
    let status: u16 = head.split(' ').nth(1).and_then(|s| s.parse().ok()).ok_or("not HTTP")?;
    let chunked = head.lines().any(|l| l.to_ascii_lowercase().starts_with("transfer-encoding:") && l.to_ascii_lowercase().contains("chunked"));
    let raw = &answer[split + 4..];
    let body = if chunked {
        let mut out = Vec::new();
        let mut rest = raw;
        loop {
            let line_end = rest.windows(2).position(|w| w == b"\r\n").ok_or("bad chunk")?;
            let size = usize::from_str_radix(std::str::from_utf8(&rest[..line_end]).map_err(|_| "bad chunk")?.split(';').next().unwrap_or("").trim(), 16)
                .map_err(|_| "bad chunk")?;
            rest = &rest[line_end + 2..];
            if size == 0 || rest.len() < size {
                break;
            }
            out.extend_from_slice(&rest[..size]);
            rest = &rest[(size + 2).min(rest.len())..];
        }
        out
    } else {
        raw.to_vec()
    };
    Ok((status, serde_json::from_slice(&body).unwrap_or(serde_json::Value::Null)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_fingerprint_is_64_hex_digits() {
        let f = "AB".repeat(32);
        assert_eq!(parse_fingerprint(&f), Some("ab".repeat(32)));
        assert_eq!(parse_fingerprint(&format!(" {} ", "0f".repeat(32))), Some("0f".repeat(32)));
        for bad in ["", "ab", &"zz".repeat(32), &"ab".repeat(33), &format!("{}:", "a".repeat(63))] {
            assert_eq!(parse_fingerprint(bad), None, "{bad}");
        }
        assert_eq!(fingerprint(b"cert").len(), 64);
    }

    #[test]
    fn a_fingerprint_holds_for_its_server_only() {
        remember("wss://Srv.Example:8443", &"ab".repeat(32));
        assert_eq!(pin_for("https://srv.example:8443/api/links/redeem"), Some("ab".repeat(32)));
        assert_eq!(pin_for("wss://srv.example:8443/ws/client?token=x"), Some("ab".repeat(32)));
        assert_eq!(pin_for("wss://srv.example:9443/ws/client"), None, "another port");
        assert_eq!(pin_for("wss://other.example:8443/ws/client"), None, "another host");
    }

    #[test]
    fn answers_are_read_whole_or_chunked() {
        let whole = b"HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: 15\r\n\r\n{\"token\":\"abc\"}";
        assert_eq!(parse_response(whole).unwrap(), (200, serde_json::json!({ "token": "abc" })));
        let chunked = b"HTTP/1.1 401 Unauthorized\r\ntransfer-encoding: chunked\r\n\r\n7\r\n{\"error\r\n8\r\n\":\"x\"}  \r\n0\r\n\r\n";
        assert_eq!(parse_response(chunked).unwrap(), (401, serde_json::json!({ "error": "x" })));
        assert!(parse_response(b"garbage").is_err());
    }

    /// Against a server with a self-signed certificate (the test's own):
    /// refused without the pin, accepted with it, refused with another.
    #[tokio::test]
    async fn the_pinned_certificate_is_accepted_and_no_other() {
        let rcgen::CertifiedKey { cert, signing_key } = rcgen::generate_simple_self_signed(vec!["localhost".into()]).unwrap();
        let der = cert.der().to_vec();
        let key = rustls::pki_types::PrivateKeyDer::try_from(signing_key.serialize_der()).unwrap();
        let server_config = rustls::ServerConfig::builder_with_provider(provider())
            .with_safe_default_protocol_versions()
            .unwrap()
            .with_no_client_auth()
            .with_single_cert(vec![CertificateDer::from(der.clone())], key)
            .unwrap();
        let acceptor = tokio_rustls::TlsAcceptor::from(Arc::new(server_config));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            use tokio::io::{AsyncReadExt, AsyncWriteExt};
            loop {
                let (tcp, _) = listener.accept().await.unwrap();
                let acceptor = acceptor.clone();
                tokio::spawn(async move {
                    let Ok(mut tls) = acceptor.accept(tcp).await else { return };
                    let mut buf = vec![0u8; 4096];
                    let _ = tls.read(&mut buf).await;
                    let body = "{\"token\":\"t1\"}";
                    let _ = tls.write_all(format!("HTTP/1.1 200 OK\r\ncontent-length: {}\r\n\r\n{body}", body.len()).as_bytes()).await;
                    let _ = tls.shutdown().await;
                });
            }
        });
        let pin = fingerprint(&der);
        let body = serde_json::json!({ "ticket": "x" });
        let t = std::time::Duration::from_secs(5);
        let ok = post_json("localhost", port, "/api/links/redeem", &pin, &body, t).await.unwrap();
        assert_eq!(ok, (200, serde_json::json!({ "token": "t1" })));
        let other = fingerprint(b"another certificate");
        assert!(post_json("localhost", port, "/api/links/redeem", &other, &body, t).await.is_err());
    }
}
