//! `lunaris://connect?...` links: parsing with validation, and the decision
//! whether a link may connect at once. A link is data from a web page or any
//! local process, so it connects without a question only to a trusted server:
//! one the user chose to always trust (`trusted_servers.json`: "Always trust
//! this server" in the link's dialog, or a server added in the window the app
//! shows without a link). Any other server waits here until the user
//! confirms in a dialog, and its session gets no clipboard sync or file
//! transfer.
//!
//! A link carries no sign-in token (it ends up on a command line), only a
//! one-time `ticket` that the app trades with the server for a token that
//! opens only the link's computer (server/src/link_tickets.rs). The trade
//! happens only once the link may connect: never for a link still waiting
//! for the user's answer.

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Duration;
use url::Url;

use crate::bridge::{self, AppArgs};

const MAX_URL_LEN: usize = 8192;
const RES_RANGE: std::ops::RangeInclusive<u32> = 64..=8192;
const FPS_RANGE: std::ops::RangeInclusive<u32> = 1..=480;
/// kbit/s.
const BITRATE_RANGE: std::ops::RangeInclusive<u32> = 100..=500_000;
const QUEUE_RANGE: std::ops::RangeInclusive<u32> = 1..=4096;
const MAX_TEXT_LEN: usize = 256;

const CODECS: &[&str] = &["auto", "h264", "h265", "hevc", "av1", "h264_444", "h264-444"];
const INPUT_PROTOCOLS: &[&str] = &["webrtc", "webtransport"];

/// What a link ticket starts with (server/src/link_tickets.rs).
const TICKET_PREFIX: &str = "lct_";
const MAX_TICKET_LEN: usize = 128;
/// How long trading a ticket may take.
const REDEEM_TIMEOUT: Duration = Duration::from_secs(15);
/// Servers kept in the always-trusted list at most.
const MAX_TRUSTED: usize = 64;
const TRUSTED_FILE: &str = "trusted_servers.json";
/// Where the app kept its sign-in before signing in moved to the web page.
const OLD_SIGN_IN_FILE: &str = "client_config.json";

/// A checked link: the session it asks for (`args.token` still empty) and
/// the ticket to trade for its token. No Debug: the ticket stays out of logs.
pub struct Link {
    pub args: AppArgs,
    ticket: String,
}

/// A link to an untrusted server, waiting for the answer.
static PENDING: Mutex<Option<Link>> = Mutex::new(None);
/// (server, token, host_id) of the confirmed untrusted session.
static UNTRUSTED: Mutex<Option<(String, String, String)>> = Mutex::new(None);

/// What the user is asked about.
pub struct LinkInfo {
    pub server_host: String,
    pub computer: String,
}

/// (secure, host, port) of a server address; `ws` is `http`, `wss` is `https`.
fn server_key(server: &str) -> Option<(bool, String, u16)> {
    let u = Url::parse(server.trim()).ok()?;
    let secure = match u.scheme() {
        "http" | "ws" => false,
        "https" | "wss" => true,
        _ => return None,
    };
    let host = u.host_str()?.trim_end_matches('.').to_ascii_lowercase();
    if host.is_empty() {
        return None;
    }
    let port = u.port_or_known_default()?;
    Some((secure, host, port))
}

pub fn same_server(a: &str, b: &str) -> bool {
    match (server_key(a), server_key(b)) {
        (Some(x), Some(y)) => x == y,
        _ => false,
    }
}

/// The form a server is kept in the trusted list: scheme (http/https),
/// host and port, so `ws://a:80`, `http://a` and `ws://A/` are one server.
pub fn trust_key(server: &str) -> Option<String> {
    let (secure, host, port) = server_key(server)?;
    Some(format!("{}://{}:{}", if secure { "https" } else { "http" }, host, port))
}

fn trusted_list_path() -> PathBuf {
    bridge::app_config_file(TRUSTED_FILE)
}

/// The always-trusted servers in `path` (none when it is missing or bad).
fn load_trusted(path: &Path) -> Vec<String> {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|data| serde_json::from_str::<Vec<String>>(&data).ok())
        .unwrap_or_default()
        .into_iter()
        .filter_map(|s| trust_key(&s))
        .collect()
}

/// Adds `server` to the list in `path` (owner-only file). The newest stay
/// when the list is full.
fn add_trusted(path: &Path, server: &str) -> std::io::Result<()> {
    let Some(key) = trust_key(server) else {
        return Err(std::io::Error::new(std::io::ErrorKind::InvalidInput, "not a server address"));
    };
    let mut list = load_trusted(path);
    list.retain(|k| *k != key);
    list.push(key);
    if list.len() > MAX_TRUSTED {
        list.drain(..list.len() - MAX_TRUSTED);
    }
    let data = serde_json::to_string_pretty(&list).map_err(std::io::Error::other)?;
    crate::secure_fs::write_private(path, data)
}

fn in_trusted_list(path: &Path, server: &str) -> bool {
    trust_key(server).is_some_and(|key| load_trusted(path).contains(&key))
}

fn is_trusted(server: &str) -> bool {
    in_trusted_list(&trusted_list_path(), server)
}

/// Takes `server` off the list in `path`; false when it was not on it.
fn remove_trusted_at(path: &Path, server: &str) -> std::io::Result<bool> {
    let Some(key) = trust_key(server) else { return Ok(false) };
    let mut list = load_trusted(path);
    let before = list.len();
    list.retain(|k| *k != key);
    if list.len() == before {
        return Ok(false);
    }
    let data = serde_json::to_string_pretty(&list).map_err(std::io::Error::other)?;
    crate::secure_fs::write_private(path, data).map(|_| true)
}

/// A server's address as the user types it: a scheme is optional
/// (https:// when none is given); the path is dropped. None when it is
/// not the address of a web server.
fn typed_server(address: &str) -> Option<String> {
    let address = address.trim();
    if address.is_empty() || address.len() > MAX_TEXT_LEN {
        return None;
    }
    let with_scheme = if address.contains("://") { address.to_string() } else { format!("https://{}", address) };
    http_base(&with_scheme)
}

/// The address shown for a trusted server: its web page's, without the
/// scheme's own port.
fn web_address(key: &str) -> Option<String> {
    let (secure, host, port) = server_key(key)?;
    let host = if host.contains(':') && !host.starts_with('[') { format!("[{host}]") } else { host };
    let scheme = if secure { "https" } else { "http" };
    Some(if port == if secure { 443 } else { 80 } {
        format!("{scheme}://{host}")
    } else {
        format!("{scheme}://{host}:{port}")
    })
}

fn trusted_servers_at(path: &Path) -> Vec<(String, String)> {
    load_trusted(path)
        .into_iter()
        .rev()
        .filter_map(|key| web_address(&key).map(|web| (key, web)))
        .collect()
}

/// The always-trusted servers, the newest first: (as kept, web page address).
pub fn trusted_servers() -> Vec<(String, String)> {
    trusted_servers_at(&trusted_list_path())
}

/// Adds a server the user typed to the trusted list; its web page's
/// address, or None when it is not a server address.
pub fn add_typed_server(address: &str) -> Option<String> {
    let server = typed_server(address)?;
    if let Err(e) = add_trusted(&trusted_list_path(), &server) {
        tracing::warn!("Could not save the trusted server: {}", e);
        return None;
    }
    web_address(&server)
}

/// Takes a server off the trusted list: its links ask again.
pub fn remove_trusted_server(server: &str) -> bool {
    remove_trusted_at(&trusted_list_path(), server).unwrap_or_else(|e| {
        tracing::warn!("Could not change the trusted servers: {}", e);
        false
    })
}

/// The app used to sign in to one server (`client_config.json`: its address
/// and a sign-in token) and connected that server's links without asking.
/// Signing in is the web page's now: that server joins the trusted list, so
/// its links still connect at once, and the unused token is deleted.
fn migrate_signed_in_server_at(config: &Path, list: &Path) {
    let Ok(data) = std::fs::read_to_string(config) else { return };
    let server = serde_json::from_str::<serde_json::Value>(&data)
        .ok()
        .and_then(|v| v.get("server_url").and_then(|s| s.as_str()).map(str::to_string))
        .unwrap_or_default();
    if !server.is_empty() && trust_key(&server).is_some() {
        if let Err(e) = add_trusted(list, &server) {
            // Kept for the next start.
            tracing::warn!("Could not keep the signed-in server as trusted: {}", e);
            return;
        }
    }
    let _ = std::fs::remove_file(config);
}

/// See `migrate_signed_in_server_at`; once, at start.
pub fn migrate_signed_in_server() {
    migrate_signed_in_server_at(&bridge::app_config_file(OLD_SIGN_IN_FILE), &trusted_list_path());
}

/// The server's web address for a ws(s):// link address: `ws` is `http`,
/// `wss` is `https` (http(s) stay). Path and query are dropped.
fn http_base(server: &str) -> Option<String> {
    let (secure, host, port) = server_key(server)?;
    let host = if host.contains(':') && !host.starts_with('[') { format!("[{host}]") } else { host };
    Some(format!("{}://{}:{}", if secure { "https" } else { "http" }, host, port))
}

fn valid_ticket(ticket: &str) -> bool {
    ticket.len() <= MAX_TICKET_LEN
        && ticket.len() > TICKET_PREFIX.len()
        && ticket.starts_with(TICKET_PREFIX)
        && ticket.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

/// Trades a link's ticket for its session token (blocking; call it off the
/// UI thread). The error is for the log and never holds the ticket or token.
fn redeem(server: &str, ticket: &str) -> Result<String, String> {
    let url = format!("{}/api/links/redeem", http_base(server).ok_or("not a server address")?);
    let ticket = ticket.to_string();
    // Its own thread and runtime: a caller's thread may run a runtime
    // already (which can't block_on another).
    std::thread::spawn(move || -> Result<String, String> {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|e| e.to_string())?;
        rt.block_on(async move {
            let client = reqwest::Client::builder()
                .timeout(REDEEM_TIMEOUT)
                .build()
                .map_err(|e| e.to_string())?;
            let resp = client
                .post(&url)
                .json(&serde_json::json!({ "ticket": ticket }))
                .send()
                .await
                .map_err(|e| format!("cannot reach the server: {}", e.without_url()))?;
            let status = resp.status();
            let body: serde_json::Value = resp.json().await.unwrap_or(serde_json::Value::Null);
            match body.get("token").and_then(|t| t.as_str()) {
                Some(token) if status.is_success() && !token.is_empty() => Ok(token.to_string()),
                _ => {
                    let why = body.get("error").and_then(|e| e.as_str()).unwrap_or("no token");
                    Err(format!("{}: {}", status.as_u16(), why.chars().take(200).collect::<String>()))
                }
            }
        })
    })
    .join()
    .unwrap_or_else(|_| Err("the request failed".into()))
}

/// The link's session with its token, or why not.
fn redeem_link(link: Link) -> Result<AppArgs, String> {
    let mut args = link.args;
    args.token = redeem(&args.server_url, &link.ticket)?;
    Ok(args)
}

fn bounded(v: &str, range: &std::ops::RangeInclusive<u32>) -> Option<u32> {
    v.trim().parse::<u32>().ok().filter(|n| range.contains(n))
}

fn text(v: &str) -> String {
    v.chars().filter(|c| !c.is_control()).take(MAX_TEXT_LEN).collect()
}

pub fn parse_deeplink_url(url_str: &str) -> Option<Link> {
    if url_str.len() > MAX_URL_LEN || !url_str.starts_with("lunaris://") {
        return None;
    }
    let parsed_url = Url::parse(url_str).ok()?;

    let mut host_id = String::new();
    let mut server_url = String::new();
    let mut ticket = String::new();
    let mut source_id: Option<String> = None;

    let mut width = 1920; // Default resolution
    let mut height = 1080;
    let mut fps = 60;
    let mut bitrate = 8000;
    let mut codec = "h264".to_string();
    let mut app_id: Option<u32> = None;
    let mut mouse_queue_limit = 256;
    let mut host_name = "Desktop • Host".to_string();
    let mut disable_cuda = false;
    let mut input_protocol = "webrtc".to_string();
    let mut encoder: Option<String> = None;
    let mut display_id: Option<String> = None;
    let mut render_backend: Option<String> = None;

    for (k, v) in parsed_url.query_pairs() {
        match k.as_ref() {
            "host_id" => host_id = text(&v),
            "server" => server_url = v.trim().to_string(),
            // Never `token`: a link carries no sign-in token any more.
            "ticket" => ticket = v.trim().to_string(),
            "source_id" => {
                let value = text(&v);
                if !value.trim().is_empty() {
                    source_id = Some(value);
                }
            }
            "host_name" => {
                let t = text(&v);
                if !t.trim().is_empty() {
                    host_name = t;
                }
            }
            "input_protocol" => {
                let value = v.trim().to_lowercase();
                if INPUT_PROTOCOLS.contains(&value.as_str()) {
                    input_protocol = value;
                }
            }
            "encoder" => {
                let value = text(&v).to_lowercase();
                if !value.is_empty() && value != "auto" {
                    encoder = Some(value);
                }
            }
            "display" | "display_id" => {
                let value = text(&v);
                if !value.is_empty() && value != "default" {
                    display_id = Some(value);
                }
            }
            "render_backend" | "gpu_backend" | "decode_backend" => {
                render_backend = Some(text(&v));
            }
            "app_id" => {
                if let Ok(id) = v.parse::<u32>() {
                    app_id = Some(id);
                }
            }
            "res" => {
                if let Some((w, h)) = v.split_once('x') {
                    if let (Some(w), Some(h)) = (bounded(w, &RES_RANGE), bounded(h, &RES_RANGE)) {
                        width = w;
                        height = h;
                    }
                }
            }
            "fps" => {
                if let Some(f) = bounded(&v, &FPS_RANGE) {
                    fps = f;
                }
            }
            "bitrate" => {
                if let Some(b) = bounded(&v, &BITRATE_RANGE) {
                    bitrate = b;
                }
            }
            "codec" => {
                let value = v.trim().to_lowercase();
                if CODECS.contains(&value.as_str()) {
                    codec = value;
                }
            }
            "mouse_queue_limit" => {
                if let Some(limit) = bounded(&v, &QUEUE_RANGE) {
                    mouse_queue_limit = limit;
                }
            }
            "disable_cuda" => {
                if let Ok(val) = v.parse::<bool>() {
                    disable_cuda = val;
                } else if v.as_ref() == "1" || v.as_ref() == "true" {
                    disable_cuda = true;
                }
            }
            _ => {}
        }
    }

    if server_key(&server_url).is_none() {
        return None;
    }

    let render_backend = render_backend
        .map(|value| bridge::normalize_render_backend(&value, disable_cuda))
        .unwrap_or_else(|| bridge::render_backend_from_disable_cuda(disable_cuda));
    disable_cuda = bridge::render_backend_disables_cuda(&render_backend);

    if host_id.is_empty() || !valid_ticket(&ticket) {
        return None;
    }
    let args = AppArgs {
        host_id,
        server_url,
        token: String::new(),
        width,
        height,
        fps,
        bitrate,
        codec,
        app_id,
        mouse_queue_limit,
        host_name,
        disable_cuda,
        render_backend,
        input_protocol,
        encoder,
        display_id,
        source_id,
    };
    Some(Link { args, ticket })
}

/// What a link asks for: connect now (it is for a trusted server), or
/// wait for the user's answer.
pub enum Verdict {
    Connect(AppArgs),
    /// For a trusted server; its ticket is being traded in the background
    /// and `DeepLinkReceived` starts the session (or `LinkFailed` says why
    /// not).
    Redeeming,
    NeedsConfirmation,
    Invalid,
}

/// Shown to the user when a link's ticket was not accepted (or the server
/// could not be asked): a ticket is short-lived and used once.
pub const LINK_FAILED: &str = "This link could not be used: it may have expired. Open it again from the web page.";

/// Judge a link. Same rules for the command line and the single-instance
/// port. A link needing confirmation is stored and replaces an older one.
/// A link to a trusted server has its ticket traded: here (`blocking`, up
/// to `REDEEM_TIMEOUT`; at start, for the one session the app was started
/// for), or in the background (`Redeeming`; the single-instance listener,
/// which must stay free for the next message). One whose ticket the server
/// refuses is Invalid, and the user is told.
pub fn judge(url: &str, blocking: bool) -> Verdict {
    let Some(link) = parse_deeplink_url(url) else {
        return Verdict::Invalid;
    };
    if is_trusted(&link.args.server_url) {
        *PENDING.lock().unwrap_or_else(|e| e.into_inner()) = None;
        if !blocking {
            redeem_in_background(link, true);
            return Verdict::Redeeming;
        }
        match redeem_link(link) {
            Ok(args) => Verdict::Connect(args),
            Err(why) => {
                link_failed(&why);
                Verdict::Invalid
            }
        }
    } else {
        *PENDING.lock().unwrap_or_else(|e| e.into_inner()) = Some(link);
        Verdict::NeedsConfirmation
    }
}

/// A ticket was not accepted: logged (`why` holds neither ticket nor
/// token) and told to the user.
fn link_failed(why: &str) {
    tracing::warn!("The link's ticket was not accepted: {}", why);
    bridge::PENDING_EVENTS
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .push(bridge::PendingDashboardEvent::LinkFailed { message: LINK_FAILED.to_string() });
}

/// Trades `link`'s ticket off the calling thread; then its session becomes
/// the one to start (`trusted`: a normal one; otherwise without clipboard
/// sync and file transfer) and `DeepLinkReceived` starts it.
fn redeem_in_background(link: Link, trusted: bool) {
    std::thread::spawn(move || match redeem_link(link) {
        Ok(args) => {
            *UNTRUSTED.lock().unwrap_or_else(|e| e.into_inner()) = if trusted {
                None
            } else {
                Some((args.server_url.clone(), args.token.clone(), args.host_id.clone()))
            };
            *bridge::ACTIVE_CONFIG.lock().unwrap_or_else(|e| e.into_inner()) = Some(args);
            bridge::PENDING_EVENTS
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .push(bridge::PendingDashboardEvent::DeepLinkReceived { url: "lunaris://connect".to_string() });
        }
        Err(why) => link_failed(&why),
    });
}

/// The link waiting for an answer, for the dialog.
pub fn pending_info() -> Option<LinkInfo> {
    let lock = PENDING.lock().unwrap_or_else(|e| e.into_inner());
    let a = &lock.as_ref()?.args;
    let (secure, host, port) = server_key(&a.server_url)?;
    let default_port = if secure { 443 } else { 80 };
    let server_host = if port == default_port { host } else { format!("{}:{}", host, port) };
    Some(LinkInfo { server_host, computer: a.host_name.clone() })
}

/// The user said yes: the link's ticket is traded in the background, then
/// its session becomes the one to start and `DeepLinkReceived` starts it.
/// Without `trust_always` that session has clipboard sync and file transfer
/// off; with it the server joins the trusted list and the session is a
/// normal one. Returns whether there was a link to answer.
pub fn confirm_pending(trust_always: bool) -> bool {
    let Some(link) = PENDING.lock().unwrap_or_else(|e| e.into_inner()).take() else {
        return false;
    };
    if trust_always {
        if let Err(e) = add_trusted(&trusted_list_path(), &link.args.server_url) {
            tracing::warn!("Could not save the trusted server: {}", e);
        }
    }
    redeem_in_background(link, trust_always);
    true
}

pub fn cancel_pending() {
    *PENDING.lock().unwrap_or_else(|e| e.into_inner()) = None;
}

/// A link to a trusted server starts a normal session.
pub fn mark_trusted_session() {
    *UNTRUSTED.lock().unwrap_or_else(|e| e.into_inner()) = None;
}

/// Is the running session the one started from an unconfirmed-server link?
/// (A session the user starts from the app has other credentials.)
pub fn session_is_untrusted() -> bool {
    let u = UNTRUSTED.lock().unwrap_or_else(|e| e.into_inner());
    let Some((server, token, host)) = u.as_ref() else { return false };
    let active = bridge::ACTIVE_CONFIG.lock().unwrap_or_else(|e| e.into_inner());
    match active.as_ref() {
        Some(a) => a.server_url == *server && a.token == *token && a.host_id == *host,
        None => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TICKET: &str = "lct_0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
    const BASE: &str =
        "lunaris://connect?host_id=h1&ticket=lct_0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef&server=";

    fn p(extra: &str) -> Option<AppArgs> {
        parse_deeplink_url(&format!("{}wss%3A%2F%2Fsrv.example%3A8443{}", BASE, extra)).map(|l| l.args)
    }

    #[test]
    fn takes_the_ticket_not_a_token() {
        let link = parse_deeplink_url(&format!("{}wss%3A%2F%2Fsrv.example", BASE)).unwrap();
        assert_eq!(link.ticket, TICKET);
        // The token comes only from trading the ticket.
        assert_eq!(link.args.token, "");
        // A token in the link is not used, and alone it is no link at all.
        let link = parse_deeplink_url(&format!("{}wss%3A%2F%2Fsrv.example&token=secret", BASE)).unwrap();
        assert_eq!(link.args.token, "");
        assert!(parse_deeplink_url("lunaris://connect?host_id=h1&token=t&server=wss%3A%2F%2Fsrv.example").is_none());
        // Not a ticket.
        let long = format!("lct_{}", "a".repeat(200));
        for bad in ["", "lct_", "abc", "lct_a%20b", "lct_%3Cx%3E", long.as_str()] {
            let url = format!("lunaris://connect?host_id=h1&ticket={bad}&server=wss%3A%2F%2Fsrv.example");
            assert!(parse_deeplink_url(&url).is_none(), "{bad}");
        }
    }

    #[test]
    fn source_id_is_read_like_other_text() {
        assert_eq!(p("").unwrap().source_id, None);
        assert_eq!(p("&source_id=").unwrap().source_id, None);
        assert_eq!(p("&source_id=rdp-1").unwrap().source_id.as_deref(), Some("rdp-1"));
        // Control characters dropped, length capped.
        assert_eq!(p("&source_id=a%0Ab").unwrap().source_id.as_deref(), Some("ab"));
        let long = p(&format!("&source_id={}", "s".repeat(1000))).unwrap().source_id.unwrap();
        assert_eq!(long.len(), MAX_TEXT_LEN);
    }

    #[test]
    fn chroma_comes_as_the_444_codec() {
        assert_eq!(p("&codec=h264_444").unwrap().codec, "h264_444");
    }

    #[test]
    fn web_address_of_a_link_server() {
        assert_eq!(http_base("wss://srv.example:8443").as_deref(), Some("https://srv.example:8443"));
        assert_eq!(http_base("ws://srv.example").as_deref(), Some("http://srv.example:80"));
        assert_eq!(http_base("wss://SRV.example/ws?x=1").as_deref(), Some("https://srv.example:443"));
        assert_eq!(http_base("ws://192.0.2.4:8090").as_deref(), Some("http://192.0.2.4:8090"));
        assert_eq!(http_base("ws://[2001:db8::1]:8090").as_deref(), Some("http://[2001:db8::1]:8090"));
        assert_eq!(http_base("file:///etc"), None);
    }

    fn temp_list() -> PathBuf {
        let n = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
        std::env::temp_dir().join(format!("lunaris-trusted-{}-{n}", std::process::id())).join(TRUSTED_FILE)
    }

    #[test]
    fn trusted_list_keeps_normalised_servers() {
        let path = temp_list();
        assert!(load_trusted(&path).is_empty());
        assert!(!in_trusted_list(&path, "wss://srv.example:8443"));
        add_trusted(&path, "wss://SRV.example:8443/").unwrap();
        // The same server however it is written; not another port or scheme.
        assert!(in_trusted_list(&path, "https://srv.example:8443"));
        assert!(in_trusted_list(&path, "wss://srv.example.:8443/ws"));
        assert!(!in_trusted_list(&path, "ws://srv.example:8443"));
        assert!(!in_trusted_list(&path, "wss://srv.example"));
        assert!(!in_trusted_list(&path, "wss://other.example:8443"));
        // Adding it again keeps one entry.
        add_trusted(&path, "https://srv.example:8443").unwrap();
        assert_eq!(load_trusted(&path), vec!["https://srv.example:8443".to_string()]);
        assert!(add_trusted(&path, "javascript:alert(1)").is_err());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
        }
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn trusted_list_is_bounded_and_ignores_garbage() {
        let path = temp_list();
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "not json").unwrap();
        assert!(load_trusted(&path).is_empty());
        std::fs::write(&path, r#"["ftp://x.example", "wss://a.example"]"#).unwrap();
        assert_eq!(load_trusted(&path), vec!["https://a.example:443".to_string()]);
        for i in 0..(MAX_TRUSTED + 5) {
            add_trusted(&path, &format!("wss://h{i}.example")).unwrap();
        }
        let list = load_trusted(&path);
        assert_eq!(list.len(), MAX_TRUSTED);
        // The newest stay.
        assert!(in_trusted_list(&path, &format!("wss://h{}.example", MAX_TRUSTED + 4)));
        assert!(!in_trusted_list(&path, "wss://a.example"));
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn accepts_a_good_link() {
        let a = p("&res=1280x720&fps=30&bitrate=5000&codec=AV1&host_name=PC").unwrap();
        assert_eq!((a.width, a.height, a.fps, a.bitrate), (1280, 720, 30, 5000));
        assert_eq!(a.codec, "av1");
        assert_eq!(a.host_name, "PC");
        assert_eq!(a.server_url, "wss://srv.example:8443");
    }

    #[test]
    fn rejects_bad_servers() {
        for s in ["file%3A%2F%2F%2Fetc", "javascript%3Aalert(1)", "ws%3A%2F%2F", "", "ftp%3A%2F%2Fa"] {
            assert!(parse_deeplink_url(&format!("{}{}", BASE, s)).is_none(), "{}", s);
        }
        assert!(parse_deeplink_url(&format!("http://x/?host_id=h&ticket={TICKET}&server=ws://a")).is_none());
        assert!(parse_deeplink_url("lunaris://connect?server=ws%3A%2F%2Fa").is_none());
    }

    #[test]
    fn out_of_range_numbers_keep_defaults() {
        let a = p("&res=99999x1&fps=0&bitrate=4294967295&mouse_queue_limit=0").unwrap();
        assert_eq!((a.width, a.height, a.fps, a.bitrate, a.mouse_queue_limit), (1920, 1080, 60, 8000, 256));
        let a = p("&fps=481").unwrap();
        assert_eq!(a.fps, 60);
        let a = p("&fps=480&res=64x8192&bitrate=500000").unwrap();
        assert_eq!((a.fps, a.width, a.height, a.bitrate), (480, 64, 8192, 500_000));
    }

    #[test]
    fn unknown_enums_fall_back() {
        let a = p("&codec=evil&input_protocol=x&render_backend=zzz").unwrap();
        assert_eq!(a.codec, "h264");
        assert_eq!(a.input_protocol, "webrtc");
        assert_eq!(a.render_backend, bridge::render_backend_from_disable_cuda(false));
    }

    #[test]
    fn oversized_and_garbage_do_not_panic() {
        assert!(parse_deeplink_url(&format!("lunaris://x?{}", "a".repeat(20000))).is_none());
        assert!(parse_deeplink_url("lunaris://\u{0}\u{ffff}%zz?&&==").is_none());
        assert!(parse_deeplink_url("").is_none());
    }

    #[test]
    fn a_refused_ticket_is_told_to_the_user() {
        link_failed("410: expired");
        let events = bridge::PENDING_EVENTS.lock().unwrap_or_else(|e| e.into_inner());
        assert!(events.iter().any(|e| matches!(e, bridge::PendingDashboardEvent::LinkFailed { message } if message == LINK_FAILED)));
    }

    #[test]
    fn same_server_normalises() {
        assert!(same_server("http://Host:8080", "ws://host:8080/ws"));
        assert!(same_server("https://a.example", "wss://a.example:443"));
        assert!(!same_server("http://a.example", "https://a.example"));
        assert!(!same_server("http://a.example:8080", "http://a.example:8081"));
        assert!(!same_server("http://a.example", "http://b.example"));
        assert!(!same_server("nonsense", "nonsense"));
    }

    #[test]
    fn typed_servers_and_their_web_address() {
        assert_eq!(typed_server("lunaris.example.com").as_deref(), Some("https://lunaris.example.com:443"));
        assert_eq!(typed_server(" http://10.0.0.2:8080/dash ").as_deref(), Some("http://10.0.0.2:8080"));
        assert_eq!(typed_server("srv.example:9443").as_deref(), Some("https://srv.example:9443"));
        assert_eq!(typed_server(""), None);
        assert_eq!(typed_server("javascript:alert(1)"), None);
        assert_eq!(typed_server("ftp://x.example"), None);
        assert_eq!(web_address("https://a.example:443").as_deref(), Some("https://a.example"));
        assert_eq!(web_address("http://a.example:8080").as_deref(), Some("http://a.example:8080"));
    }

    #[test]
    fn trusted_servers_are_listed_newest_first_and_removed() {
        let path = temp_list();
        add_trusted(&path, "https://a.example").unwrap();
        add_trusted(&path, "ws://b.example:8080").unwrap();
        let listed = trusted_servers_at(&path);
        assert_eq!(listed[0], ("http://b.example:8080".to_string(), "http://b.example:8080".to_string()));
        assert_eq!(listed[1], ("https://a.example:443".to_string(), "https://a.example".to_string()));
        assert!(remove_trusted_at(&path, "https://A.example/").unwrap());
        assert!(!remove_trusted_at(&path, "https://a.example").unwrap());
        assert!(!in_trusted_list(&path, "wss://a.example"));
        assert!(in_trusted_list(&path, "http://b.example:8080"));
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn the_signed_in_server_becomes_trusted() {
        let list = temp_list();
        let dir = list.parent().unwrap().to_path_buf();
        std::fs::create_dir_all(&dir).unwrap();
        let config = dir.join(OLD_SIGN_IN_FILE);
        std::fs::write(&config, r#"{"server_url":"https://srv.example","token":"t","username":"u"}"#).unwrap();
        migrate_signed_in_server_at(&config, &list);
        assert!(in_trusted_list(&list, "wss://srv.example"));
        assert!(!config.exists(), "the old sign-in is deleted");
        // Nothing to do the next time.
        migrate_signed_in_server_at(&config, &list);
        assert_eq!(load_trusted(&list).len(), 1);
        let _ = std::fs::remove_dir_all(dir);
    }

}
