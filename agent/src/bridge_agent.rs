//! The bridge-only agent: a helper that shows the computers of other types
//! (RustDesk, Sunshine, RDP, VNC) it was given, and nothing of the computer
//! it runs on. The Lunaris server starts it as its "server gateway" (a child
//! process, server/src/gateway.rs) and connects to it as it would to any
//! computer added by address, so those computers can be added and reached
//! through the server itself.
//!
//! It is not a way in to this computer:
//!
//! - it listens on 127.0.0.1 only, on a port of its own choosing, and takes
//!   one password, made by its parent for this run and handed over in the
//!   environment (never on the command line, never stored);
//! - it shows sources only: a session that names no source is refused, so
//!   this computer's screen, input and clipboard are never used (a session
//!   with a source uses none of them, bridge.rs);
//! - no files of this computer, no server connection of its own, no
//!   RustDesk listener;
//! - it ends when its parent does (its standard input closes).
//!
//! Its sources are in the config file it is started with (`--config`),
//! made on the first start, which the agent that shares this computer
//! never reads.

use std::sync::atomic::{AtomicBool, Ordering};

use tokio::sync::mpsc;
use tracing::{error, info};

use crate::pairing::AgentConfig;

/// The environment variable the app hands the run's password over in.
pub const PASSWORD_ENV: &str = "LUNARIS_BRIDGE_PASSWORD";
/// The line on standard output that says which port was taken:
/// `@lunaris-bridge port=<port>`.
pub const PORT_LINE: &str = "@lunaris-bridge port=";
/// A password shorter than this is refused.
const MIN_PASSWORD_CHARS: usize = 24;

static BRIDGE_ONLY: AtomicBool = AtomicBool::new(false);

/// Whether this process is a bridge-only agent.
pub fn is_bridge_only() -> bool {
    BRIDGE_ONLY.load(Ordering::Relaxed)
}

/// The arguments to start one with (after the program's own name; the
/// Lunaris app's program takes `agent` first).
pub fn spawn_args(config_path: &str, name: &str) -> Vec<String> {
    ["--cli", "--bridge-only", "--config", config_path, "--name", name].iter().map(|s| s.to_string()).collect()
}

/// The port in a line of the child's standard output, if it is that line.
pub fn port_of_line(line: &str) -> Option<u16> {
    line.trim().strip_prefix(PORT_LINE)?.parse().ok()
}

/// The config at `path`; a new one (a new id, no sources) is saved there
/// when there is none yet, so the id stays the same from one start to the
/// next and sources can be added. One that is there but cannot be read is
/// left alone: an error, not an empty agent under a new id.
pub fn load_or_make_config(path: &str) -> Result<AgentConfig, Box<dyn std::error::Error + Send + Sync>> {
    let file = std::path::Path::new(path);
    if file.exists() {
        return crate::pairing::load_config(path).map_err(|e| format!("cannot read {}: {:?}", path, e).into());
    }
    if let Some(dir) = file.parent().filter(|d| !d.as_os_str().is_empty()) {
        std::fs::create_dir_all(dir)?;
    }
    let config = crate::pairing::new_config();
    crate::pairing::save_config(&config, path).map_err(|e| format!("cannot save {}: {:?}", path, e))?;
    info!("Bridge-only agent: made a new config in {}", path);
    Ok(config)
}

pub(crate) async fn run(config: AgentConfig, name: String) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    BRIDGE_ONLY.store(true, Ordering::SeqCst);
    let password = std::env::var(PASSWORD_ENV).unwrap_or_default();
    // Not left in the environment for anything this process starts (a connector's program).
    std::env::remove_var(PASSWORD_ENV);
    if password.chars().count() < MIN_PASSWORD_CHARS {
        error!("Bridge-only agent: no password was handed over in {}", PASSWORD_ENV);
        return Err("no bridge password".into());
    }

    let hub = crate::hub::Hub::new(config.client_unique_id.clone());
    let (agent_tx, router) = crate::hub::spawn_router(hub.clone());
    let (inbound_tx, mut inbound_rx) = mpsc::unbounded_channel::<common::SignalingMessage>();

    // This computer only.
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0)).await?;
    let port = listener.local_addr()?.port();
    let direct_config = crate::direct::DirectConfig {
        port,
        password,
        approve_mode: crate::approval::ApproveMode::Password,
        name: name.clone(),
        links: crate::direct::Links::default(),
        // The app connects with TLS; this listener is on 127.0.0.1 only.
        allow_legacy_plain: true,
        // Not pinned: the server starts it with a new password each time.
        tls_dir: None,
    };
    let listening = tokio::spawn(crate::direct::run_listener_on(listener, direct_config, hub.clone(), inbound_tx.clone()));

    // The app reads this line; everything else this process says goes to
    // standard error.
    {
        use std::io::Write;
        let mut out = std::io::stdout();
        let _ = writeln!(out, "{}{}", PORT_LINE, port);
        let _ = out.flush();
    }
    info!("Bridge-only agent on 127.0.0.1:{}: shows other computers only, never this one", port);

    // The app holds our standard input: when it goes, so do we.
    std::thread::Builder::new()
        .name("lunaris-bridge-parent".into())
        .spawn(|| {
            use std::io::Read;
            let mut buf = [0u8; 64];
            let mut stdin = std::io::stdin();
            loop {
                match stdin.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(_) => {}
                }
            }
            std::process::exit(0);
        })
        .ok();

    crate::run_session_logic(config, agent_tx, &mut inbound_rx).await;
    drop(inbound_tx);
    listening.abort();
    router.abort();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_port_line_and_the_arguments() {
        assert_eq!(port_of_line("@lunaris-bridge port=40123\n"), Some(40123));
        assert_eq!(port_of_line("@lunaris-bridge port="), None);
        assert_eq!(port_of_line("something else"), None);
        let args = spawn_args("/x/bridge_config.json", "pc");
        assert!(args.contains(&"--bridge-only".to_string()));
        // The password is not among them.
        assert_eq!(args, ["--cli", "--bridge-only", "--config", "/x/bridge_config.json", "--name", "pc"]);
    }

    #[test]
    fn the_config_is_made_once_and_then_kept() {
        let dir = std::env::temp_dir().join(format!("lunaris-bridge-config-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("gateway").join("agent_config.json");
        let path = path.to_str().unwrap();
        let first = load_or_make_config(path).unwrap();
        assert!(!first.client_unique_id.is_empty());
        assert!(std::path::Path::new(path).exists());
        // The same id on the next start.
        assert_eq!(load_or_make_config(path).unwrap().client_unique_id, first.client_unique_id);
        // One that cannot be read is not replaced.
        std::fs::write(path, "not json").unwrap();
        assert!(load_or_make_config(path).is_err());
        assert_eq!(std::fs::read_to_string(path).unwrap(), "not json");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
