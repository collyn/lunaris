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
    std::fs::write(key, made.key_pair.serialize_pem()).map_err(|e| format!("{}: {}", key.display(), e))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(key, std::fs::Permissions::from_mode(0o600));
    }
    info!("Made a self-signed HTTPS certificate for {}", names.join(", "));
    Ok(())
}

fn load_config() -> Result<rustls::ServerConfig, String> {
    let (cert_path, key_path, own) = cert_paths();
    if own && (!cert_path.exists() || !key_path.exists()) {
        make_self_signed(&cert_path, &key_path)?;
    }
    let certs = rustls_pemfile::certs(&mut std::io::BufReader::new(
        std::fs::File::open(&cert_path).map_err(|e| format!("{}: {}", cert_path.display(), e))?,
    ))
    .collect::<Result<Vec<_>, _>>()
    .map_err(|e| format!("{}: {}", cert_path.display(), e))?;
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
    Ok(config)
}

/// Serves `app` over HTTPS until the task is dropped/aborted.
pub async fn serve(app: Router, port: u16) {
    let config = match load_config() {
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
