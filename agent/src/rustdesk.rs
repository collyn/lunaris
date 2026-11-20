//! RustDesk compatibility: an unmodified RustDesk client connects by IP
//! address ("direct IP access") and sees and controls this computer, fed by
//! the same capture, H.264 encoder, input injector and clipboard as
//! Lunaris's own sessions.
//!
//! Off unless `rustdesk_port` is set in the agent's config. Offered: the
//! screen (one display, one viewer at a time), mouse and keyboard
//! (`input.rs`), the cursor's shape (`cursor.rs`), clipboard text
//! (`clipboard.rs`, when the agent's `allow_clipboard` is on), sound, and
//! this computer's files for the client's file manager (`files.rs`, when the
//! agent's `rustdesk_file_transfer` is on; held to the agent's `file_access`).
//! Not offered: RustDesk ids, rendezvous and relay servers.
//!
//! The wire protocol is ported from RustDesk 1.5.3 (AGPL-3.0); every step
//! names the function it follows. In short, as a RustDesk host does for a
//! direct-IP connection:
//!
//! 1. TCP, default port 21118 (rustdesk/src/rendezvous_mediator.rs
//!    `direct_server`, `get_direct_port`), frames as in `frame.rs`.
//! 2. No key exchange and no encryption: `direct_server` calls
//!    `create_tcp_connection(.., secure = false, ..)`, so
//!    rustdesk/src/server.rs `identity_handshake` sends no `SignedId`, and the
//!    client (rustdesk/src/client.rs `Client::_start`, the `is_ip_str` branch)
//!    returns the plain stream without calling `secure_connection`. The same
//!    in 1.4.9. **Everything, the picture and what is typed included,
//!    crosses the network in the clear**; only the password is protected, by
//!    the challenge hash below. For a LAN or a VPN.
//! 3. The host sends `Hash { salt, challenge }`; the client answers with
//!    `LoginRequest` (`login.rs`); the host replies `LoginResponse` with an
//!    error text or `PeerInfo`, then sends `VideoFrame`s (`session.rs`).
//!    With `approve_mode` "click" or "password-click" the person at this
//!    computer is asked as for Lunaris's own direct connections
//!    (approval.rs).

pub(crate) mod clipboard;
pub(crate) mod cursor;
pub(crate) mod files;
pub mod frame;
mod input;
pub(crate) mod qos;
pub mod login;
pub mod proto;
mod session;

use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::net::TcpListener;
use tracing::{debug, info, warn};

use crate::approval::ApproveMode;
use crate::direct::{Limits, UnauthSlot};

/// This process's RustDesk login limits (as direct.rs `process_limits`, but
/// apart from the direct listener's).
fn process_limits() -> Arc<Mutex<Limits>> {
    static LIMITS: std::sync::OnceLock<Arc<Mutex<Limits>>> = std::sync::OnceLock::new();
    LIMITS.get_or_init(Arc::default).clone()
}

/// rustdesk/src/rendezvous_mediator.rs `get_direct_port`: RENDEZVOUS_PORT + 2
/// (hbb_common/src/config.rs `RENDEZVOUS_PORT` = 21116). It is also the port
/// the client adds when only an address is typed (rustdesk/src/client.rs
/// `Client::_start`: `check_port(peer, RELAY_PORT + 1)`).
pub const DEFAULT_PORT: u16 = 21118;

/// Settings of the RustDesk-compatible listener.
#[derive(Clone)]
pub struct RustdeskConfig {
    pub port: u16,
    /// The agent's access password (`direct_password`): what the RustDesk
    /// client must type.
    pub password: String,
    pub approve_mode: ApproveMode,
    /// The agent's `allow_clipboard`: clipboard text is shared both ways.
    pub allow_clipboard: bool,
    /// The agent's id; the login salt is derived from it.
    pub agent_id: String,
    /// This computer's name, shown by the client.
    pub name: String,
    /// The agent's `rustdesk_file_transfer` (and `allow_file_transfer`):
    /// RustDesk clients may open their file manager on this computer.
    pub file_transfer: bool,
    /// The agent's `file_access`: what their file manager may reach.
    pub file_access: crate::files::Access,
    /// The folder a file manager starts in; `None` for the one
    /// `file_access` gives (the home folder, or the allowed folder).
    pub file_home: Option<std::path::PathBuf>,
}

/// What every connection of the listener shares.
pub(crate) struct Shared {
    pub(crate) config: RustdeskConfig,
    pub(crate) limits: Arc<Mutex<Limits>>,
    pub(crate) screen: Arc<dyn session::Screen>,
    /// Asks the person here to accept a connection (approval.rs), and
    /// whether anyone can be asked.
    pub(crate) asker: session::Asker,
    pub(crate) attended: Arc<dyn Fn() -> bool + Send + Sync>,
    /// One viewer at a time (one capture, one encoder).
    pub(crate) viewing: AtomicBool,
}

/// Accepts RustDesk clients on `config.port` until the task is aborted.
pub async fn run_listener(config: RustdeskConfig) -> anyhow::Result<()> {
    // All interfaces, as direct.rs and RustDesk's `direct_server`
    // (`hbb_common::tcp::listen_any`) do.
    let listener = TcpListener::bind(("0.0.0.0", config.port)).await?;
    if std::env::var_os(session::RECORD_INPUT_ENV).is_some() {
        tracing::warn!(
            "{} is set: RustDesk sessions record their input and use a stand-in clipboard (a test mode)",
            session::RECORD_INPUT_ENV
        );
    }
    serve_listener(
        listener,
        config,
        Arc::new(session::MediaScreen),
        session::system_asker(),
        Arc::new(crate::approval::attended),
        process_limits(),
    )
    .await
}

/// rustdesk/src/rendezvous_mediator.rs `direct_server`: accept, no delay on
/// the socket, one task per connection.
pub(crate) async fn serve_listener(
    listener: TcpListener,
    config: RustdeskConfig,
    screen: Arc<dyn session::Screen>,
    asker: session::Asker,
    attended: Arc<dyn Fn() -> bool + Send + Sync>,
    limits: Arc<Mutex<Limits>>,
) -> anyhow::Result<()> {
    let local = listener.local_addr().map(|a| a.to_string()).unwrap_or_default();
    info!(
        "RustDesk-compatible listener on {} (unencrypted; {:?}; clipboard {}; file transfer {})",
        local,
        config.approve_mode,
        if config.allow_clipboard { "on" } else { "off" },
        if config.file_transfer { "on" } else { "off" }
    );
    let ctx = Arc::new(Shared {
        config,
        // Kept while the listener starts again (`run_listener`): a lockout
        // is not undone by new settings.
        limits,
        screen,
        asker,
        attended,
        viewing: AtomicBool::new(false),
    });
    // The connections end with the listener (its settings changed).
    let mut connections = tokio::task::JoinSet::new();
    loop {
        while connections.try_join_next().is_some() {}
        let (stream, peer) = match listener.accept().await {
            Ok(conn) => conn,
            Err(e) => {
                warn!("RustDesk connection accept failed: {}", e);
                tokio::time::sleep(Duration::from_millis(200)).await;
                continue;
            }
        };
        // rustdesk/src/server.rs `create_tcp_connection`: a connection takes
        // its place among the unauthorized ones before anything is read
        // (`admit_unauthorized`); here with the limits of direct.rs.
        let Some(slot) = UnauthSlot::take(&ctx.limits, peer.ip()) else {
            debug!("RustDesk connection from {} refused: too many connections not logged in yet", peer);
            continue;
        };
        let _ = stream.set_nodelay(true);
        debug!("RustDesk direct access from {}", peer);
        let ctx = ctx.clone();
        connections.spawn(async move {
            let reason = session::serve(stream, peer, slot, ctx).await;
            // A session's start and end are logged in session.rs; this also
            // covers every port scan.
            debug!("RustDesk connection from {} closed: {}", peer, reason);
        });
    }
}
