//! The game mouse: a USB mouse the agent plugs into its own Windows computer.
//!
//! Games with a kernel anti-cheat drop injected mouse moves: in a League of
//! Legends match (Riot Vanguard) `SendInput` refuses relative moves and
//! ignores absolute ones, while a USB mouse works. So the agent is also a
//! USB mouse (hid.rs), served over USB/IP on the loopback address
//! (usbip.rs) and plugged in with usbip-win2, a USB/IP client for Windows
//! that the computer's owner installs (BSD-2-Clause, Microsoft-signed
//! drivers; https://github.com/vadimgrn/usbip-win2). Windows drives it with
//! its own mouse driver, as any mouse.
//!
//! It carries what the viewer sends while its pointer is locked ("Lock
//! Pointer": relative moves, packet type 0) - games take only relative
//! moves - with the buttons and wheel that come with them. Absolute
//! positions (the pointer not locked) keep going through `SendInput`, which
//! puts the pointer exactly where it is aimed; they also switch the buttons
//! back to it.
//!
//! Without usbip-win2, or before the mouse is plugged in, everything goes
//! through `SendInput` as before.

// Served on Windows only (the tests run everywhere).
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
pub mod hid;
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
mod usbip;
#[cfg(target_os = "windows")]
mod attach;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};

/// The loopback port the USB/IP server listens on (another one is taken
/// when it is in use).
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
const PORT: u16 = 37240;

static DEVICE: OnceLock<Arc<usbip::Device>> = OnceLock::new();
/// The viewer's last move was relative (its pointer is locked).
static RELATIVE: AtomicBool = AtomicBool::new(false);

/// What the agent's computer offers, for the server's page of the computer
/// (`AgentBuild::game_mouse`): `"usbip"` with usbip-win2 installed, `"none"`
/// without; `None` where no game mouse is needed (Linux: uinput is a
/// device already).
pub fn availability() -> Option<&'static str> {
    #[cfg(target_os = "windows")]
    {
        Some(if attach::usbip_exe().is_some() { common::GAME_MOUSE_USBIP } else { common::GAME_MOUSE_NONE })
    }
    #[cfg(not(target_os = "windows"))]
    {
        None
    }
}

/// Starts the game mouse once (later calls do nothing): its USB/IP server,
/// then plugs it in. Needs a tokio runtime. Windows only.
pub fn start() {
    #[cfg(target_os = "windows")]
    {
        let mut first = false;
        let device = DEVICE.get_or_init(|| {
            first = true;
            usbip::Device::new()
        });
        if !first {
            return;
        }
        let Some(exe) = attach::usbip_exe() else {
            log::info!(
                "Game mouse: usbip-win2 is not installed, so games with an anti-cheat \
                 (League of Legends, Valorant) may ignore the mouse. Install it from \
                 https://github.com/vadimgrn/usbip-win2/releases and restart the agent"
            );
            return;
        };
        let device = device.clone();
        tokio::spawn(async move {
            let listener = match tokio::net::TcpListener::bind(("127.0.0.1", PORT)).await {
                Ok(l) => l,
                Err(_) => match tokio::net::TcpListener::bind(("127.0.0.1", 0)).await {
                    Ok(l) => l,
                    Err(e) => {
                        log::warn!("Game mouse: no loopback port for USB/IP: {e}");
                        return;
                    }
                },
            };
            let port = listener.local_addr().map(|a| a.port()).unwrap_or(PORT);
            tokio::spawn(usbip::serve(listener, device));
            match tokio::task::spawn_blocking(move || attach::plug_in(&exe, port, usbip::BUS_ID)).await {
                Ok(Ok(hub_port)) => log::info!("Game mouse: plugged in (USB/IP port {hub_port}, TCP {port})"),
                Ok(Err(e)) => log::warn!("Game mouse: could not plug it in: {e}"),
                Err(e) => log::warn!("Game mouse: {e}"),
            }
        });
    }
}

fn device_in_use() -> Option<&'static Arc<usbip::Device>> {
    DEVICE.get().filter(|d| d.in_use())
}

/// A relative move: through the game mouse when it is plugged in (`true`),
/// else the caller injects it.
pub fn relative_move(dx: i32, dy: i32) -> bool {
    RELATIVE.store(true, Ordering::Relaxed);
    match device_in_use() {
        Some(d) => {
            d.update(|q| q.move_by(dx, dy));
            true
        }
        None => false,
    }
}

/// An absolute position: injected by the caller; the buttons go back to it.
pub fn absolute_move() {
    if RELATIVE.swap(false, Ordering::Relaxed) {
        // A button held while the pointer was locked is let go by the mouse
        // that pressed it.
        release_all();
    }
}

/// A button (0 left, 1 right, 2 middle, 3 back, 4 forward): through the game
/// mouse while the viewer's pointer is locked (`true`).
pub fn button(bit: u8, down: bool) -> bool {
    if !RELATIVE.load(Ordering::Relaxed) {
        return false;
    }
    match device_in_use() {
        Some(d) => {
            d.update(|q| q.button(bit, down));
            true
        }
        None => false,
    }
}

/// Wheel clicks (vertical: up is positive; horizontal: right is positive),
/// as `button`.
pub fn wheel(clicks: i32, vertical: bool) -> bool {
    if !RELATIVE.load(Ordering::Relaxed) || clicks == 0 {
        return false;
    }
    match device_in_use() {
        Some(d) => {
            d.update(|q| q.wheel(clicks, vertical));
            true
        }
        None => false,
    }
}

/// Lets go of the game mouse's buttons (the viewer left).
pub fn release_all() {
    if let Some(d) = DEVICE.get() {
        d.update(|q| q.release_all());
    }
}
