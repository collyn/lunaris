//! The signed checksums of this server's release, for the install scripts.
//!
//! The scripts install the agent from the GitHub release of this server's
//! version as root / SYSTEM. Before they run anything they check the file
//! against the release's `SHA256SUMS`, whose Ed25519 signature (CI,
//! `SHA256SUMS.sig`) is checked with the key built into this program
//! (updater/src/verify.rs). Where the server can fetch and check them itself
//! (it reaches GitHub), the expected SHA-256 of each file is written into the
//! script: then the computer being installed needs no tool that knows
//! Ed25519 (Windows PowerShell 5.1 has none). Otherwise the script fetches
//! and checks the list itself (see agent.sh / agent.ps1).
//!
//! Fetched at most once per process when it works; a failure is retried
//! after [`RETRY_AFTER`]. `LUNARIS_NO_UPDATE_CHECK` (no calls to GitHub from
//! this server) turns it off.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio_rustls::rustls;
use tracing::{debug, info, warn};

use updater::verify;

/// How long the script request waits for GitHub at most.
const FETCH_TIMEOUT: Duration = Duration::from_secs(8);
const RETRY_AFTER: Duration = Duration::from_secs(600);
const MAX_REDIRECTS: usize = 4;
/// SHA256SUMS lists every release file: a few kB.
const SUMS_CAP: usize = 256 << 10;
const SIG_CAP: usize = 1 << 10;

/// The verified SHA256SUMS of this version, or when the last try failed.
enum Cached {
    Verified(Arc<Vec<u8>>),
    Failed(Instant),
}

static CACHE: Mutex<Option<Cached>> = Mutex::new(None);

/// The release's SHA256SUMS, its signature checked against the built-in key
/// and its `# lunaris <version>` line against `version`; None when it can't
/// be had (offline, no signing key in this build, a bad signature).
pub async fn verified_sums(version: &str) -> Option<Arc<Vec<u8>>> {
    if std::env::var_os("LUNARIS_NO_UPDATE_CHECK").is_some() || !verify::signing_configured(verify::RELEASE_SIGNING_PUBLIC_KEY_B64) {
        return None;
    }
    match &*CACHE.lock().unwrap_or_else(|p| p.into_inner()) {
        Some(Cached::Verified(s)) => return Some(s.clone()),
        Some(Cached::Failed(when)) if when.elapsed() < RETRY_AFTER => return None,
        _ => {}
    }
    let result = tokio::time::timeout(FETCH_TIMEOUT, fetch_and_verify(version))
        .await
        .unwrap_or_else(|_| Err("timed out".into()));
    let mut cache = CACHE.lock().unwrap_or_else(|p| p.into_inner());
    match result {
        Ok(sums) => {
            info!("Install scripts: release {version} checksums fetched and verified");
            let sums = Arc::new(sums);
            *cache = Some(Cached::Verified(sums.clone()));
            Some(sums)
        }
        Err(e) => {
            // A dev build has no release: not worth a warning.
            debug!("Install scripts: release {version} checksums not available ({e}); the scripts check them themselves");
            *cache = Some(Cached::Failed(Instant::now()));
            None
        }
    }
}

async fn fetch_and_verify(version: &str) -> Result<Vec<u8>, String> {
    let base = format!("{}v{version}/", updater::DOWNLOAD_PREFIX);
    let sums = get(&format!("{base}{}", verify::SUMS_NAME), SUMS_CAP).await?;
    let sig = get(&format!("{base}{}", verify::SIG_NAME), SIG_CAP).await?;
    check(&sums, &sig, version).inspect_err(|e| warn!("Install scripts: release {version} checksums refused: {e}"))?;
    Ok(sums)
}

/// The signature with the built-in key, then the version line.
fn check(sums: &[u8], sig: &[u8], version: &str) -> Result<(), String> {
    check_with_key(verify::RELEASE_SIGNING_PUBLIC_KEY_B64, sums, sig, version)
}

fn check_with_key(key_b64: &str, sums: &[u8], sig: &[u8], version: &str) -> Result<(), String> {
    verify::verify_sums_signature(key_b64, sums, sig).map_err(|e| e.to_string())?;
    verify::check_sums_version(sums, version).map_err(|e| e.to_string())?;
    verify::parse_sums(sums).map_err(|e| e.to_string())?;
    Ok(())
}

/// The lowercase hex SHA-256 the verified list gives for `name`.
pub fn sha256_of(sums: &[u8], name: &str) -> Option<String> {
    verify::expected_digest(sums, name).ok().map(hex::encode)
}

/// The built-in release key as the base64 body of a PEM public key
/// (SubjectPublicKeyInfo of an Ed25519 key), for `openssl pkeyutl` in
/// agent.sh; empty when this build has no key.
pub fn release_key_spki_b64() -> String {
    const ED25519_SPKI_PREFIX: [u8; 12] = [0x30, 0x2a, 0x30, 0x05, 0x06, 0x03, 0x2b, 0x65, 0x70, 0x03, 0x21, 0x00];
    match verify::public_key(verify::RELEASE_SIGNING_PUBLIC_KEY_B64) {
        Ok(raw) => {
            let mut der = ED25519_SPKI_PREFIX.to_vec();
            der.extend_from_slice(&raw);
            base64(&der)
        }
        Err(_) => String::new(),
    }
}

/// Standard base64 with padding.
pub fn base64(data: &[u8]) -> String {
    const ABC: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let b = [chunk[0], *chunk.get(1).unwrap_or(&0), *chunk.get(2).unwrap_or(&0)];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        out.push(ABC[(n >> 18) as usize & 63] as char);
        out.push(ABC[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 { ABC[(n >> 6) as usize & 63] as char } else { '=' });
        out.push(if chunk.len() > 2 { ABC[n as usize & 63] as char } else { '=' });
    }
    out
}

// ---- A small HTTPS GET to GitHub (release downloads redirect to its CDN) ----

/// Hosts a release file may come from.
fn allowed_host(host: &str) -> bool {
    host == "github.com" || host.ends_with(".githubusercontent.com")
}

/// `https://host/path` split; anything else (another scheme, a port, user
/// info) is refused.
fn split_url(url: &str) -> Result<(String, String), String> {
    let rest = url.strip_prefix("https://").ok_or("not https")?;
    let (host, path) = match rest.find('/') {
        Some(i) => (&rest[..i], &rest[i..]),
        None => (rest, "/"),
    };
    if host.is_empty() || !host.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'.') {
        return Err(format!("bad host in {url}"));
    }
    let host = host.to_ascii_lowercase();
    if !allowed_host(&host) {
        return Err(format!("not a GitHub host: {host}"));
    }
    if path.bytes().any(|b| b.is_ascii_control() || b == b' ') {
        return Err("bad path".into());
    }
    Ok((host, path.to_string()))
}

async fn get(url: &str, cap: usize) -> Result<Vec<u8>, String> {
    let mut url = url.to_string();
    for _ in 0..=MAX_REDIRECTS {
        let (host, path) = split_url(&url)?;
        let response = request(&host, &path, cap).await?;
        match parse_response(&response)? {
            Answer::Body(body) => return Ok(body),
            Answer::Redirect(location) => {
                url = if location.starts_with('/') { format!("https://{host}{location}") } else { location };
            }
        }
    }
    Err("too many redirects".into())
}

async fn request(host: &str, path: &str, cap: usize) -> Result<Vec<u8>, String> {
    let mut roots = rustls::RootCertStore::empty();
    for cert in rustls_native_certs::load_native_certs().certs {
        let _ = roots.add(cert);
    }
    if roots.is_empty() {
        return Err("no CA certificates on this system".into());
    }
    let config = rustls::ClientConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
        .with_safe_default_protocol_versions()
        .map_err(|e| e.to_string())?
        .with_root_certificates(roots)
        .with_no_client_auth();
    let tcp = tokio::net::TcpStream::connect((host, 443)).await.map_err(|e| e.to_string())?;
    let name = rustls::pki_types::ServerName::try_from(host.to_string()).map_err(|e| e.to_string())?;
    let mut tls = tokio_rustls::TlsConnector::from(Arc::new(config))
        .connect(name, tcp)
        .await
        .map_err(|e| e.to_string())?;
    let req = format!(
        "GET {path} HTTP/1.1\r\nHost: {host}\r\nUser-Agent: {}\r\nAccept: */*\r\nConnection: close\r\n\r\n",
        updater::USER_AGENT
    );
    tls.write_all(req.as_bytes()).await.map_err(|e| e.to_string())?;
    let mut response = Vec::new();
    // Headers (GitHub's are a few kB) and the body.
    (&mut tls).take((cap + (64 << 10)) as u64).read_to_end(&mut response).await.map_err(|e| e.to_string())?;
    Ok(response)
}

#[derive(Debug, PartialEq, Eq)]
enum Answer {
    Body(Vec<u8>),
    Redirect(String),
}

fn parse_response(response: &[u8]) -> Result<Answer, String> {
    let split = response.windows(4).position(|w| w == b"\r\n\r\n").ok_or("incomplete answer")?;
    let head = String::from_utf8_lossy(&response[..split]).to_string();
    let body = &response[split + 4..];
    let mut lines = head.lines();
    let status = lines.next().unwrap_or("");
    let code = status.split_whitespace().nth(1).unwrap_or("");
    let header = |name: &str| {
        head.lines().skip(1).find_map(|l| {
            let (k, v) = l.split_once(':')?;
            k.trim().eq_ignore_ascii_case(name).then(|| v.trim().to_string())
        })
    };
    match code {
        "200" => {
            if header("transfer-encoding").is_some_and(|v| v.to_ascii_lowercase().contains("chunked")) {
                dechunk(body).map(Answer::Body)
            } else {
                Ok(Answer::Body(body.to_vec()))
            }
        }
        "301" | "302" | "303" | "307" | "308" => header("location").map(Answer::Redirect).ok_or_else(|| "redirect without a location".into()),
        _ => Err(format!("GitHub answered {}", status.trim())),
    }
}

fn dechunk(mut data: &[u8]) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    loop {
        let line_end = data.windows(2).position(|w| w == b"\r\n").ok_or("bad chunk")?;
        let size_text = String::from_utf8_lossy(&data[..line_end]);
        let size = usize::from_str_radix(size_text.split(';').next().unwrap_or("").trim(), 16).map_err(|_| "bad chunk size")?;
        data = &data[line_end + 2..];
        if size == 0 {
            return Ok(out);
        }
        if data.len() < size + 2 {
            return Err("truncated chunk".into());
        }
        out.extend_from_slice(&data[..size]);
        data = &data[size + 2..];
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ring::signature::{Ed25519KeyPair, KeyPair};

    #[test]
    fn base64_matches_the_standard() {
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foo"), "Zm9v");
        assert_eq!(base64(b"foobar"), "Zm9vYmFy");
        assert_eq!(base64(&[0xfb, 0xff]), "+/8=");
    }

    #[test]
    fn release_key_pem_body_is_an_ed25519_spki() {
        let b64 = release_key_spki_b64();
        // 12 bytes of prefix + 32 of key = 44 bytes = 60 base64 characters.
        assert_eq!(b64.len(), 60);
        assert!(b64.starts_with("MCowBQYDK2VwAyEA"), "{b64}");
        // Its key part is the built-in key.
        let raw = verify::public_key(verify::RELEASE_SIGNING_PUBLIC_KEY_B64).unwrap();
        let mut der = vec![0x30, 0x2a, 0x30, 0x05, 0x06, 0x03, 0x2b, 0x65, 0x70, 0x03, 0x21, 0x00];
        der.extend_from_slice(&raw);
        assert_eq!(b64, base64(&der));
    }

    #[test]
    fn only_github_urls_are_fetched() {
        assert!(split_url("https://github.com/collyn/lunaris/releases/download/v1/SHA256SUMS").is_ok());
        assert!(split_url("https://release-assets.githubusercontent.com/x?y=1").is_ok());
        for bad in [
            "http://github.com/x",
            "https://evil.example/x",
            "https://github.com.evil.example/x",
            "https://github.com:444/x",
            "https://user@github.com/x",
            "https://githubusercontent.com.evil/x",
        ] {
            assert!(split_url(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn answers_are_read() {
        assert_eq!(parse_response(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nhi").unwrap(), Answer::Body(b"hi".to_vec()));
        assert_eq!(
            parse_response(b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n2\r\nhi\r\n1\r\n!\r\n0\r\n\r\n").unwrap(),
            Answer::Body(b"hi!".to_vec())
        );
        assert_eq!(
            parse_response(b"HTTP/1.1 302 Found\r\nlocation: https://objects.githubusercontent.com/a\r\n\r\n").unwrap(),
            Answer::Redirect("https://objects.githubusercontent.com/a".into())
        );
        assert!(parse_response(b"HTTP/1.1 404 Not Found\r\n\r\n").is_err());
    }

    #[test]
    fn a_list_is_taken_only_signed_and_for_this_version() {
        let rng = ring::rand::SystemRandom::new();
        let pkcs8 = Ed25519KeyPair::generate_pkcs8(&rng).unwrap();
        let pair = Ed25519KeyPair::from_pkcs8(pkcs8.as_ref()).unwrap();
        let key = base64(pair.public_key().as_ref());
        let sums = format!("# lunaris 1.2.3\n{}  lunaris-agent_1.2.3_amd64.deb\n", "ab".repeat(32)).into_bytes();
        let sig = pair.sign(&sums);
        assert!(check_with_key(&key, &sums, sig.as_ref(), "1.2.3").is_ok());
        assert!(check_with_key(&key, &sums, sig.as_ref(), "1.2.4").is_err(), "another version");
        let mut changed = sums.clone();
        changed[20] ^= 1;
        assert!(check_with_key(&key, &changed, sig.as_ref(), "1.2.3").is_err(), "changed list");
        assert_eq!(sha256_of(&sums, "lunaris-agent_1.2.3_amd64.deb"), Some("ab".repeat(32)));
        assert_eq!(sha256_of(&sums, "other"), None);
    }
}
