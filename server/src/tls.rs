//! HTTPS for the web client, next to plain HTTP.
//!
//! Browsers only give secure pages (https, or localhost) game controllers
//! (Gamepad API), the clipboard API and WebTransport, so the server also
//! listens with TLS on LUNARIS_HTTPS_PORT (default 8443; "0" or "off" turns
//! it off). Agents keep using ws:// on the HTTP port.
//!
//! Certificate: LUNARIS_TLS_CERT + LUNARIS_TLS_KEY (PEM files, e.g. from
//! Let's Encrypt); otherwise a self-signed one is made once and kept in the
//! data directory (browsers ask to accept it the first time).

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

use axum::extract::ConnectInfo;
use axum::Router;
use hyper_util::rt::{TokioExecutor, TokioIo};
use tokio_rustls::rustls;
use tower::ServiceExt;
use tracing::{debug, error, info, warn};

use crate::paths;
use crate::signaling::ConnAddrs;

pub const DEFAULT_HTTPS_PORT: u16 = 8443;

/// In the extensions of every request that came in on this HTTPS listener
/// (not through a reverse proxy): install.rs tells agents to use it.
#[derive(Clone, Copy, Debug)]
pub struct ViaTls;

/// SHA-256 (lowercase hex) of the certificate this listener serves, as the
/// agent computes it for pinning (lib.rs `server_tls::fingerprint`).
static SERVED_CERT_SHA256: std::sync::OnceLock<String> = std::sync::OnceLock::new();

pub fn served_cert_sha256() -> Option<&'static str> {
    SERVED_CERT_SHA256.get().map(String::as_str)
}

/// Whether HTTPS is on with the server's own self-signed certificate (no
/// LUNARIS_TLS_CERT): browsers, curl and PowerShell don't trust it.
pub fn self_signed() -> bool {
    https_port().is_some() && cert_paths().2
}

pub fn cert_sha256(der: &[u8]) -> String {
    use sha2::Digest;
    hex::encode(sha2::Sha256::digest(der))
}

/// The HTTPS port, or None when turned off.
pub fn https_port() -> Option<u16> {
    match std::env::var("LUNARIS_HTTPS_PORT") {
        Ok(v) if v.trim() == "0" || v.trim().eq_ignore_ascii_case("off") => None,
        Ok(v) => v.trim().parse().ok().or(Some(DEFAULT_HTTPS_PORT)),
        Err(_) => Some(DEFAULT_HTTPS_PORT),
    }
}

fn cert_paths() -> (PathBuf, PathBuf, bool) {
    match (std::env::var("LUNARIS_TLS_CERT"), std::env::var("LUNARIS_TLS_KEY")) {
        (Ok(c), Ok(k)) if !c.is_empty() && !k.is_empty() => (PathBuf::from(c), PathBuf::from(k), false),
        _ => (paths::data_file("tls_cert.pem"), paths::data_file("tls_key.pem"), true),
    }
}

/// Names the self-signed certificate is made for: localhost, this computer's
/// name and its addresses.
fn self_signed_names() -> Vec<String> {
    let mut names = vec!["localhost".to_string(), "127.0.0.1".to_string(), "::1".to_string()];
    if let Ok(h) = hostname::get() {
        let h = h.to_string_lossy().to_string();
        if !h.is_empty() {
            names.push(h.clone());
            names.push(format!("{}.local", h));
        }
    }
    // The address used to reach other machines (no packet is sent).
    if let Ok(sock) = std::net::UdpSocket::bind("0.0.0.0:0") {
        if sock.connect("192.0.2.1:9").is_ok() {
            if let Ok(a) = sock.local_addr() {
                names.push(a.ip().to_string());
            }
        }
    }
    names.dedup();
    names
}

fn make_self_signed(cert: &PathBuf, key: &PathBuf) -> Result<(), String> {
    let names = self_signed_names();
    let made = rcgen::generate_simple_self_signed(names.clone()).map_err(|e| e.to_string())?;
    if let Some(dir) = cert.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    std::fs::write(cert, made.cert.pem()).map_err(|e| format!("{}: {}", cert.display(), e))?;
    // Created owner-only: never readable by others, not even briefly.
    paths::write_private(key, &made.signing_key.serialize_pem()).map_err(|e| format!("{}: {}", key.display(), e))?;
    info!("Made a self-signed HTTPS certificate for {}", names.join(", "));
    Ok(())
}

/// The listener's TLS configuration and its certificate's SHA-256.
fn load_config() -> Result<(rustls::ServerConfig, Option<String>), String> {
    let (cert_path, key_path, own) = cert_paths();
    if own && (!cert_path.exists() || !key_path.exists()) {
        make_self_signed(&cert_path, &key_path)?;
    }
    if own {
        // A key from before write_private may be readable by others.
        paths::tighten_private(&key_path);
    }
    let certs = rustls_pemfile::certs(&mut std::io::BufReader::new(
        std::fs::File::open(&cert_path).map_err(|e| format!("{}: {}", cert_path.display(), e))?,
    ))
    .collect::<Result<Vec<_>, _>>()
    .map_err(|e| format!("{}: {}", cert_path.display(), e))?;
    let fingerprint = certs.first().map(|leaf| cert_sha256(leaf.as_ref()));
    let key = rustls_pemfile::private_key(&mut std::io::BufReader::new(
        std::fs::File::open(&key_path).map_err(|e| format!("{}: {}", key_path.display(), e))?,
    ))
    .map_err(|e| format!("{}: {}", key_path.display(), e))?
    .ok_or_else(|| format!("{}: no private key", key_path.display()))?;

    let mut config = rustls::ServerConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
        .with_safe_default_protocol_versions()
        .map_err(|e| e.to_string())?
        .with_no_client_auth()
        .with_single_cert(certs, key)
        .map_err(|e| e.to_string())?;
    config.alpn_protocols = vec![b"h2".to_vec(), b"http/1.1".to_vec()];
    Ok((config, fingerprint))
}

/// `Strict-Transport-Security` for this listener: off unless
/// `LUNARIS_HSTS=1`. HSTS holds for the host name on every port, so a
/// browser would then also refuse plain http://host:8090 (the HTTP port
/// speaks no TLS), and with a self-signed certificate it removes the
/// "continue anyway" button: only for servers whose HTTPS is the one way
/// in, with a trusted certificate. Short and without `preload`.
pub fn hsts_header(env: Option<&str>) -> Option<&'static str> {
    matches!(env.map(str::trim), Some("1" | "true" | "on" | "yes")).then_some("max-age=86400")
}

/// Serves `app` over HTTPS until the task is dropped/aborted.
pub async fn serve(app: Router, port: u16) {
    let app = match hsts_header(std::env::var("LUNARIS_HSTS").ok().as_deref()) {
        Some(value) => app.layer(tower_http::set_header::SetResponseHeaderLayer::if_not_present(
            axum::http::header::STRICT_TRANSPORT_SECURITY,
            axum::http::HeaderValue::from_static(value),
        )),
        None => app,
    };
    let (config, fingerprint) = match load_config() {
        Ok(c) => c,
        Err(e) => {
            error!("HTTPS disabled: {}", e);
            return;
        }
    };
    let acceptor = tokio_rustls::TlsAcceptor::from(Arc::new(config));
    let addr = SocketAddr::from(([0, 0, 0, 0], port));
    let listener = match tokio::net::TcpListener::bind(addr).await {
        Ok(l) => l,
        Err(e) => {
            warn!("HTTPS disabled: cannot listen on {}: {}", addr, e);
            return;
        }
    };
    // Only now: install.rs hands out wss:// with this pin only while the
    // HTTPS listener really listens.
    if let Some(fingerprint) = fingerprint {
        let _ = SERVED_CERT_SHA256.set(fingerprint);
    }
    info!("Lunaris Server running on https://{} (web client with controllers and clipboard)", addr);

    loop {
        let (tcp, remote) = match listener.accept().await {
            Ok(x) => x,
            Err(e) => {
                debug!("HTTPS accept: {}", e);
                continue;
            }
        };
        let local = tcp.local_addr().ok();
        let acceptor = acceptor.clone();
        let app = app.clone();
        tokio::spawn(async move {
            let tls = match acceptor.accept(tcp).await {
                Ok(s) => s,
                // Browsers drop the first connection while the user decides
                // about a self-signed certificate: not worth a warning.
                Err(e) => {
                    debug!("TLS handshake with {}: {}", remote, e);
                    return;
                }
            };
            let service = hyper::service::service_fn(move |mut req: hyper::Request<hyper::body::Incoming>| {
                req.extensions_mut().insert(ConnectInfo(ConnAddrs { local, remote }));
                req.extensions_mut().insert(ViaTls);
                app.clone().oneshot(req)
            });
            if let Err(e) = hyper_util::server::conn::auto::Builder::new(TokioExecutor::new())
                .serve_connection_with_upgrades(TokioIo::new(tls), service)
                .await
            {
                debug!("HTTPS connection with {}: {}", remote, e);
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hsts_only_when_asked() {
        assert_eq!(hsts_header(None), None);
        assert_eq!(hsts_header(Some("0")), None);
        assert_eq!(hsts_header(Some("")), None);
        assert_eq!(hsts_header(Some("1")), Some("max-age=86400"));
        assert!(!hsts_header(Some("on")).unwrap().contains("preload"));
    }
}
