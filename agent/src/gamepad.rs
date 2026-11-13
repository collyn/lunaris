//! Game controllers: the client's controllers appear on this computer as
//! virtual Xbox 360 controllers (the layout every game understands), one per
//! client controller.
//!
//! - Linux: uinput devices that look like the `xpad` driver's
//!   (045e:028e, same buttons and axes), so SDL/Steam/Proton map them.
//! - Windows: ViGEmBus (must be installed); without it the client is told.
//!
//! The client sends full controller states on the `gamepad` data channel
//! (unordered, no retransmits: a lost state is replaced by the next one; the
//! client also repeats the state every second). Format, big-endian:
//!
//! - state: `0, id u8, kind u8, buttons u32, lt u8, rt u8, lx i16, ly i16,
//!   rx i16, ry i16` (17 bytes; sticks -32768..32767, y grows downwards);
//! - removed: `1, id u8`.
//!
//! A controller that hasn't been heard from for a few seconds is unplugged
//! (client gone or channel lost); all are unplugged when the session ends.

use std::collections::HashMap;
use std::sync::mpsc as std_mpsc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::json;
use tracing::{info, warn};
use webrtc::data_channel::{DataChannelEvent, RTCDataChannelInit};
use webrtc::peer_connection::PeerConnection;

/// Most controllers one client may plug in.
const MAX_PADS: u8 = 8;
/// Unplugged after this long without a state (the client repeats every 1 s).
const IDLE_UNPLUG: Duration = Duration::from_secs(5);

pub mod buttons {
    pub const A: u32 = 0x0001;
    pub const B: u32 = 0x0002;
    pub const X: u32 = 0x0004;
    pub const Y: u32 = 0x0008;
    pub const UP: u32 = 0x0010;
    pub const DOWN: u32 = 0x0020;
    pub const LEFT: u32 = 0x0040;
    pub const RIGHT: u32 = 0x0080;
    pub const LB: u32 = 0x0100;
    pub const RB: u32 = 0x0200;
    pub const START: u32 = 0x0400;
    pub const BACK: u32 = 0x0800;
    pub const LS: u32 = 0x1000;
    pub const RS: u32 = 0x2000;
    pub const GUIDE: u32 = 0x4000;
    /// PlayStation touchpad click: Back on an Xbox controller (as Steam does).
    pub const TOUCHPAD: u32 = 0x8000;
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PadState {
    pub buttons: u32,
    pub lt: u8,
    pub rt: u8,
    pub lx: i16,
    pub ly: i16,
    pub rx: i16,
    pub ry: i16,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PadMsg {
    State { id: u8, kind: u8, state: PadState },
    Removed { id: u8 },
}

pub fn parse(data: &[u8]) -> Option<PadMsg> {
    match *data.first()? {
        0 if data.len() >= 17 => {
            let i16_at = |i: usize| i16::from_be_bytes([data[i], data[i + 1]]);
            Some(PadMsg::State {
                id: data[1],
                kind: data[2],
                state: PadState {
                    buttons: u32::from_be_bytes([data[3], data[4], data[5], data[6]]),
                    lt: data[7],
                    rt: data[8],
                    lx: i16_at(9),
                    ly: i16_at(11),
                    rx: i16_at(13),
                    ry: i16_at(15),
                },
            })
        }
        1 if data.len() >= 2 => Some(PadMsg::Removed { id: data[1] }),
        _ => None,
    }
}

enum Event {
    Msg(PadMsg),
    Closed,
}

/// Opens the `gamepad` channel on a new session's connection.
pub async fn attach(pc: &Arc<dyn PeerConnection>) -> anyhow::Result<()> {
    let init = RTCDataChannelInit { ordered: false, max_retransmits: Some(0), ..Default::default() };
    let channel = pc.create_data_channel("gamepad", Some(init)).await?;

    let (tx, rx) = std_mpsc::channel::<Event>();
    let reply_channel = channel.clone();
    // The pads thread reports problems (no ViGEmBus...) to the client once.
    let (err_tx, mut err_rx) = tokio::sync::mpsc::unbounded_channel::<String>();
    tokio::spawn(async move {
        while let Some(message) = err_rx.recv().await {
            let _ = reply_channel.send_text(&json!({"type": "gamepad_error", "message": message}).to_string()).await;
        }
    });
    std::thread::Builder::new()
        .name("lunaris-gamepads".into())
        .spawn(move || pads_thread(rx, err_tx))?;

    let tx = Arc::new(Mutex::new(tx));
    // webrtc 0.21: events are polled; one task reads the messages and the close.
    let on_msg_tx = tx.clone();
    let reader = channel.clone();
    tokio::spawn(async move {
        while let Some(event) = reader.poll().await {
            match event {
                DataChannelEvent::OnMessage(msg) => {
                    if !msg.is_string {
                        if let Some(m) = parse(&msg.data) {
                            let _ = on_msg_tx.lock().unwrap().send(Event::Msg(m));
                        }
                    }
                }
                DataChannelEvent::OnClose => {
                    let _ = on_msg_tx.lock().unwrap().send(Event::Closed);
                }
                _ => {}
            }
        }
    });
    // If the connection dies without closing the channel, IDLE_UNPLUG does it.
    Ok(())
}

fn pads_thread(rx: std_mpsc::Receiver<Event>, errors: tokio::sync::mpsc::UnboundedSender<String>) {
    let mut pads: HashMap<u8, (Box<dyn VirtualPad>, Instant)> = HashMap::new();
    let mut reported = false;
    loop {
        match rx.recv_timeout(Duration::from_secs(1)) {
            Ok(Event::Msg(PadMsg::State { id, kind, state })) => {
                if id >= MAX_PADS {
                    continue;
                }
                if !pads.contains_key(&id) {
                    match new_pad(id) {
                        Ok(pad) => {
                            info!("Controller {} connected (client kind {}): virtual Xbox 360 controller", id, kind);
                            pads.insert(id, (pad, Instant::now()));
                        }
                        Err(e) => {
                            if !reported {
                                warn!("Could not create a virtual controller: {}", e);
                                let _ = errors.send(e);
                                reported = true;
                            }
                            continue;
                        }
                    }
                }
                if let Some((pad, seen)) = pads.get_mut(&id) {
                    *seen = Instant::now();
                    if let Err(e) = pad.update(&state) {
                        warn!("Controller {}: {}", id, e);
                    }
                }
            }
            Ok(Event::Msg(PadMsg::Removed { id })) => {
                if pads.remove(&id).is_some() {
                    info!("Controller {} disconnected", id);
                }
            }
            Ok(Event::Closed) | Err(std_mpsc::RecvTimeoutError::Disconnected) => break,
            Err(std_mpsc::RecvTimeoutError::Timeout) => {}
        }
        pads.retain(|id, (_, seen)| {
            let keep = seen.elapsed() < IDLE_UNPLUG;
            if !keep {
                info!("Controller {} silent for {:?}: unplugged", id, IDLE_UNPLUG);
            }
            keep
        });
    }
    if !pads.is_empty() {
        info!("Session over: unplugging {} controller(s)", pads.len());
    }
}

trait VirtualPad {
    fn update(&mut self, state: &PadState) -> Result<(), String>;
}

// ---------------------------------------------------------------- Linux

#[cfg(target_os = "linux")]
fn new_pad(id: u8) -> Result<Box<dyn VirtualPad>, String> {
    linux::XpadDevice::new(id).map(|d| Box::new(d) as Box<dyn VirtualPad>)
}

#[cfg(target_os = "linux")]
mod linux {
    use super::{buttons as b, PadState, VirtualPad};
    use evdev::uinput::VirtualDevice;
    use evdev::{AbsInfo, AbsoluteAxisCode as Abs, AttributeSet, BusType, EventType, InputEvent, InputId, KeyCode, UinputAbsSetup};

    /// Buttons in the order and codes of the kernel's xpad driver.
    const KEYS: [(u32, KeyCode); 11] = [
        (b::A, KeyCode::BTN_SOUTH),
        (b::B, KeyCode::BTN_EAST),
        // xpad reports X as BTN_X (= BTN_NORTH) and Y as BTN_Y (= BTN_WEST);
        // SDL's mapping for 045e:028e expects exactly that.
        (b::X, KeyCode::BTN_NORTH),
        (b::Y, KeyCode::BTN_WEST),
        (b::LB, KeyCode::BTN_TL),
        (b::RB, KeyCode::BTN_TR),
        (b::BACK | b::TOUCHPAD, KeyCode::BTN_SELECT),
        (b::START, KeyCode::BTN_START),
        (b::GUIDE, KeyCode::BTN_MODE),
        (b::LS, KeyCode::BTN_THUMBL),
        (b::RS, KeyCode::BTN_THUMBR),
    ];

    pub struct XpadDevice {
        dev: VirtualDevice,
        last: Option<PadState>,
    }

    impl XpadDevice {
        pub fn new(id: u8) -> Result<Self, String> {
            let err = |e: std::io::Error| {
                format!("cannot create a virtual controller ({}); is /dev/uinput writable? (Lunaris installs a udev rule for it)", e)
            };
            let mut keys = AttributeSet::<KeyCode>::new();
            for (_, k) in KEYS {
                keys.insert(k);
            }
            let stick = AbsInfo::new(0, -32768, 32767, 16, 128, 0);
            let trigger = AbsInfo::new(0, 0, 255, 0, 0, 0);
            let hat = AbsInfo::new(0, -1, 1, 0, 0, 0);
            let dev = VirtualDevice::builder()
                .map_err(err)?
                .name(&format!("Microsoft X-Box 360 pad {}", id))
                .input_id(InputId::new(BusType::BUS_USB, 0x045e, 0x028e, 0x0114))
                .with_keys(&keys)
                .map_err(err)?
                .with_absolute_axis(&UinputAbsSetup::new(Abs::ABS_X, stick))
                .map_err(err)?
                .with_absolute_axis(&UinputAbsSetup::new(Abs::ABS_Y, stick))
                .map_err(err)?
                .with_absolute_axis(&UinputAbsSetup::new(Abs::ABS_Z, trigger))
                .map_err(err)?
                .with_absolute_axis(&UinputAbsSetup::new(Abs::ABS_RX, stick))
                .map_err(err)?
                .with_absolute_axis(&UinputAbsSetup::new(Abs::ABS_RY, stick))
                .map_err(err)?
                .with_absolute_axis(&UinputAbsSetup::new(Abs::ABS_RZ, trigger))
                .map_err(err)?
                .with_absolute_axis(&UinputAbsSetup::new(Abs::ABS_HAT0X, hat))
                .map_err(err)?
                .with_absolute_axis(&UinputAbsSetup::new(Abs::ABS_HAT0Y, hat))
                .map_err(err)?
                .build()
                .map_err(err)?;
            Ok(Self { dev, last: None })
        }
    }

    fn hat(neg: bool, pos: bool) -> i32 {
        match (neg, pos) {
            (true, false) => -1,
            (false, true) => 1,
            _ => 0,
        }
    }

    impl VirtualPad for XpadDevice {
        fn update(&mut self, s: &PadState) -> Result<(), String> {
            let old = self.last.unwrap_or_default();
            let mut events = Vec::with_capacity(16);
            for (mask, key) in KEYS {
                let now = s.buttons & mask != 0;
                if self.last.is_none() || now != (old.buttons & mask != 0) {
                    events.push(InputEvent::new(EventType::KEY.0, key.0, now as i32));
                }
            }
            let axes = [
                (Abs::ABS_X, s.lx as i32, old.lx as i32),
                (Abs::ABS_Y, s.ly as i32, old.ly as i32),
                (Abs::ABS_RX, s.rx as i32, old.rx as i32),
                (Abs::ABS_RY, s.ry as i32, old.ry as i32),
                (Abs::ABS_Z, s.lt as i32, old.lt as i32),
                (Abs::ABS_RZ, s.rt as i32, old.rt as i32),
                (
                    Abs::ABS_HAT0X,
                    hat(s.buttons & b::LEFT != 0, s.buttons & b::RIGHT != 0),
                    hat(old.buttons & b::LEFT != 0, old.buttons & b::RIGHT != 0),
                ),
                (
                    Abs::ABS_HAT0Y,
                    hat(s.buttons & b::UP != 0, s.buttons & b::DOWN != 0),
                    hat(old.buttons & b::UP != 0, old.buttons & b::DOWN != 0),
                ),
            ];
            for (axis, now, before) in axes {
                if self.last.is_none() || now != before {
                    events.push(InputEvent::new(EventType::ABSOLUTE.0, axis.0, now));
                }
            }
            self.last = Some(*s);
            if events.is_empty() {
                return Ok(());
            }
            // emit() appends the SYN_REPORT.
            self.dev.emit(&events).map_err(|e| e.to_string())
        }
    }
}

// ---------------------------------------------------------------- Windows

#[cfg(target_os = "windows")]
fn new_pad(_id: u8) -> Result<Box<dyn VirtualPad>, String> {
    windows::ViGEmPad::new().map(|d| Box::new(d) as Box<dyn VirtualPad>)
}

#[cfg(target_os = "windows")]
mod windows {
    use super::{buttons as b, PadState, VirtualPad};
    use vigem_client::{Client, TargetId, XButtons, XGamepad, Xbox360Wired};

    pub struct ViGEmPad {
        target: Xbox360Wired<Client>,
    }

    impl ViGEmPad {
        pub fn new() -> Result<Self, String> {
            let client = Client::connect().map_err(|e| {
                format!("controllers need the ViGEmBus driver on the remote computer (https://github.com/nefarius/ViGEmBus/releases): {}", e)
            })?;
            let mut target = Xbox360Wired::new(client, TargetId::XBOX360_WIRED);
            target.plugin().map_err(|e| format!("cannot plug in a virtual controller: {}", e))?;
            target.wait_ready().map_err(|e| format!("virtual controller not ready: {}", e))?;
            Ok(Self { target })
        }
    }

    /// XInput button bits.
    const MAP: [(u32, u16); 16] = [
        (b::UP, 0x0001),
        (b::DOWN, 0x0002),
        (b::LEFT, 0x0004),
        (b::RIGHT, 0x0008),
        (b::START, 0x0010),
        (b::BACK, 0x0020),
        (b::TOUCHPAD, 0x0020),
        (b::LS, 0x0040),
        (b::RS, 0x0080),
        (b::LB, 0x0100),
        (b::RB, 0x0200),
        (b::GUIDE, 0x0400),
        (b::A, 0x1000),
        (b::B, 0x2000),
        (b::X, 0x4000),
        (b::Y, 0x8000),
    ];

    /// XInput's y axes grow upwards.
    fn flip(v: i16) -> i16 {
        (-(v as i32)).clamp(i16::MIN as i32, i16::MAX as i32) as i16
    }

    impl VirtualPad for ViGEmPad {
        fn update(&mut self, s: &PadState) -> Result<(), String> {
            let raw = MAP.iter().filter(|(m, _)| s.buttons & m != 0).fold(0u16, |acc, (_, x)| acc | x);
            let pad = XGamepad {
                buttons: XButtons { raw },
                left_trigger: s.lt,
                right_trigger: s.rt,
                thumb_lx: s.lx,
                thumb_ly: flip(s.ly),
                thumb_rx: s.rx,
                thumb_ry: flip(s.ry),
            };
            self.target.update(&pad).map_err(|e| e.to_string())
        }
    }

    impl Drop for ViGEmPad {
        fn drop(&mut self) {
            let _ = self.target.unplug();
        }
    }
}

// ---------------------------------------------------------------- others

#[cfg(not(any(target_os = "linux", target_os = "windows")))]
fn new_pad(_id: u8) -> Result<Box<dyn VirtualPad>, String> {
    Err("controllers aren't supported on this computer's system yet".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_states_and_removals() {
        let mut p = vec![0u8, 2, 1];
        p.extend_from_slice(&(buttons::A | buttons::UP).to_be_bytes());
        p.extend_from_slice(&[10, 255]);
        for v in [-32768i16, 32767, 0, -1] {
            p.extend_from_slice(&v.to_be_bytes());
        }
        assert_eq!(
            parse(&p),
            Some(PadMsg::State {
                id: 2,
                kind: 1,
                state: PadState { buttons: buttons::A | buttons::UP, lt: 10, rt: 255, lx: -32768, ly: 32767, rx: 0, ry: -1 },
            })
        );
        assert_eq!(parse(&[1, 3]), Some(PadMsg::Removed { id: 3 }));
        assert_eq!(parse(&[0, 1, 2]), None);
        assert_eq!(parse(&[]), None);
    }
}
