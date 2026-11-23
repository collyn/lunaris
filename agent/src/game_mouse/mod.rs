//! The game mouse: a USB mouse the agent plugs into its own Windows computer.
//!
//! Games with a kernel anti-cheat drop injected mouse moves: in a League of
//! Legends match (Riot Vanguard) `SendInput` refuses relative moves and
//! ignores absolute ones, while a USB mouse works. So the agent can also be
//! a USB mouse (hid.rs), served over USB/IP on the loopback address
//! (usbip.rs) and plugged in with usbip-win2, a USB/IP client for Windows
//! that the computer's owner installs (BSD-2-Clause, Microsoft-signed
//! drivers; https://github.com/vadimgrn/usbip-win2). Windows drives it with
//! its own mouse driver, as any mouse.
//!
//! The viewer chooses it ("Game mouse" in the stream's settings, packet type
//! 5 on a mouse channel): only then is the mouse plugged in, and it is
//! unplugged when the viewer turns it off or leaves. While it is on, what
//! the viewer sends with its pointer locked (relative moves, packet type 0;
//! games take only relative moves) goes through it, with the buttons and
//! the wheel. Absolute positions (the pointer not locked) keep going through
//! `SendInput`, which puts the pointer exactly where it is aimed, and switch
//! the buttons back to it.
//!
//! Off, without usbip-win2, or before the mouse is plugged in, everything
//! goes through `SendInput` as before.

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
/// The viewer turned the game mouse on.
static WANTED: AtomicBool = AtomicBool::new(false);
/// The viewer's last move was relative (its pointer is locked).
static RELATIVE: AtomicBool = AtomicBool::new(false);
/// The runtime that plugs the mouse in and out (the input threads that turn
/// it on and off are not in one).
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
static RUNTIME: OnceLock<tokio::runtime::Handle> = OnceLock::new();

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

/// Keeps the runtime the mouse is plugged in and out from (called in a
/// session's set-up, which runs in it).
pub fn init() {
    if let Ok(handle) = tokio::runtime::Handle::try_current() {
        let _ = RUNTIME.set(handle);
    }
}

/// The viewer turns the game mouse on or off (or leaves: off).
pub fn set_wanted(on: bool) {
    let was = WANTED.swap(on, Ordering::Relaxed);
    if was == on {
        return;
    }
    log::info!("Game mouse: turned {} by the viewer", if on { "on" } else { "off" });
    if !on {
        release_all();
    }
    #[cfg(target_os = "windows")]
    if let Some(runtime) = RUNTIME.get() {
        runtime.spawn(plugging::reconcile());
    }
}

/// Plugs the mouse in or out to match `WANTED`, one change at a time.
#[cfg(target_os = "windows")]
mod plugging {
    use super::*;

    #[derive(Default)]
    struct Plug {
        /// The TCP port of the USB/IP server, once it listens.
        port: Option<u16>,
        /// The hub port the mouse is plugged into.
        hub_port: Option<String>,
    }

    static PLUG: tokio::sync::Mutex<Plug> = tokio::sync::Mutex::const_new(Plug { port: None, hub_port: None });

    pub async fn reconcile() {
        let mut plug = PLUG.lock().await;
        let wanted = WANTED.load(Ordering::Relaxed);
        let Some(exe) = attach::usbip_exe() else {
            if wanted {
                log::info!(
                    "Game mouse: usbip-win2 is not installed. Install it from \
                     https://github.com/vadimgrn/usbip-win2/releases and restart the agent"
                );
            }
            return;
        };
        if wanted && plug.hub_port.is_none() {
            let device = DEVICE.get_or_init(usbip::Device::new).clone();
            let port = match plug.port {
                Some(p) => p,
                None => {
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
                    let p = listener.local_addr().map(|a| a.port()).unwrap_or(PORT);
                    tokio::spawn(usbip::serve(listener, device));
                    plug.port = Some(p);
                    p
                }
            };
            match tokio::task::spawn_blocking(move || attach::plug_in(&exe, port, usbip::BUS_ID)).await {
                Ok(Ok(hub_port)) => {
                    log::info!("Game mouse: plugged in (USB/IP port {hub_port}, TCP {port})");
                    plug.hub_port = Some(hub_port);
                }
                Ok(Err(e)) => log::warn!("Game mouse: could not plug it in: {e}"),
                Err(e) => log::warn!("Game mouse: {e}"),
            }
        } else if !wanted {
            if let Some(hub_port) = plug.hub_port.take() {
                match tokio::task::spawn_blocking(move || attach::unplug(&exe, &hub_port)).await {
                    Ok(Ok(())) => log::info!("Game mouse: unplugged"),
                    Ok(Err(e)) => log::warn!("Game mouse: could not unplug it: {e}"),
                    Err(e) => log::warn!("Game mouse: {e}"),
                }
            }
        }
    }
}

fn device_in_use() -> Option<&'static Arc<usbip::Device>> {
    if !WANTED.load(Ordering::Relaxed) {
        return None;
    }
    DEVICE.get().filter(|d| d.in_use())
}

/// A relative move: through the game mouse when it is on and plugged in
/// (`true`), else the caller injects it.
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
/// mouse while it is on and the viewer's pointer is locked (`true`).
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

/// Lets go of the game mouse's buttons.
pub fn release_all() {
    if let Some(d) = DEVICE.get() {
        d.update(|q| q.release_all());
    }
}
