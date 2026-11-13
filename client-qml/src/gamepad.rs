//! Game controllers of this computer (gilrs: evdev on Linux, Windows.Gaming.Input
//! on Windows, IOKit on macOS).
//!
//! A thread owns gilrs for the whole app: it keeps the controllers' state for
//! the Controllers settings page (`pads_json`) and, during a session, sends
//! each controller's state on the agent's `gamepad` channel, where it becomes
//! a virtual Xbox 360 controller (packet format: agent/src/gamepad.rs).

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc as std_mpsc, Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use bytes::Bytes;
use serde_json::{json, Value};
use webrtc::data_channel::{DataChannel, DataChannelEvent, RTCDataChannelState};

const MAX_PADS: usize = 8;
/// The remote side unplugs a controller after 5 s of silence.
const KEEPALIVE: Duration = Duration::from_secs(1);

mod bits {
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
}

#[derive(Clone, Copy, Default, PartialEq, Eq)]
struct State {
    buttons: u32,
    lt: u8,
    rt: u8,
    lx: i16,
    ly: i16,
    rx: i16,
    ry: i16,
}

#[derive(Clone)]
struct Pad {
    slot: u8,
    name: String,
    kind: &'static str,
    rumble: bool,
    state: State,
}

enum Cmd {
    Vibrate(u8),
}

static ENABLED: AtomicBool = AtomicBool::new(true);
static PADS: Mutex<Vec<Pad>> = Mutex::new(Vec::new());
static CHANNEL: Mutex<Option<(Arc<dyn DataChannel>, tokio::runtime::Handle)>> = Mutex::new(None);
static CMDS: OnceLock<Mutex<std_mpsc::Sender<Cmd>>> = OnceLock::new();
static UNAVAILABLE: Mutex<Option<String>> = Mutex::new(None);

/// Starts watching controllers (once).
pub fn start() {
    CMDS.get_or_init(|| {
        let (tx, rx) = std_mpsc::channel();
        if let Err(e) = std::thread::Builder::new().name("lunaris-gamepads".into()).spawn(move || run(rx)) {
            *UNAVAILABLE.lock().unwrap() = Some(format!("controllers unavailable: {}", e));
        }
        Mutex::new(tx)
    });
}

pub fn set_enabled(on: bool) {
    ENABLED.store(on, Ordering::Relaxed);
}

pub fn vibrate(slot: i32) {
    start();
    if let (Some(tx), Ok(slot)) = (CMDS.get(), u8::try_from(slot)) {
        let _ = tx.lock().unwrap().send(Cmd::Vibrate(slot));
    }
}

/// Controllers for the settings page:
/// `{"error": "...", "pads": [{slot, name, kind, rumble, buttons, lt, rt, lx, ly, rx, ry}]}`.
pub fn pads_json() -> String {
    start();
    let pads: Vec<Value> = PADS
        .lock()
        .unwrap()
        .iter()
        .map(|p| {
            json!({
                "slot": p.slot, "name": p.name, "kind": p.kind, "rumble": p.rumble,
                "buttons": p.state.buttons, "lt": p.state.lt, "rt": p.state.rt,
                "lx": p.state.lx, "ly": p.state.ly, "rx": p.state.rx, "ry": p.state.ry,
            })
        })
        .collect();
    json!({ "error": UNAVAILABLE.lock().unwrap().clone().unwrap_or_default(), "pads": pads }).to_string()
}

/// The agent opened its `gamepad` channel (in the session's runtime).
pub fn attach(channel: Arc<dyn DataChannel>) {
    start();
    // webrtc 0.21: the channel's events are polled (from the session's runtime).
    let reader = channel.clone();
    if let Ok(handle) = tokio::runtime::Handle::try_current() {
        handle.spawn(async move {
            while let Some(event) = reader.poll().await {
                let DataChannelEvent::OnMessage(msg) = event else { continue };
                if msg.is_string {
                    if let Ok(v) = serde_json::from_slice::<Value>(&msg.data) {
                        if v.get("type").and_then(Value::as_str) == Some("gamepad_error") {
                            if let Some(m) = v.get("message").and_then(Value::as_str) {
                                crate::share::set_status(format!("Controllers: {}", m));
                            }
                        }
                    }
                }
            }
        });
    }
    if let Ok(handle) = tokio::runtime::Handle::try_current() {
        *CHANNEL.lock().unwrap() = Some((channel, handle));
    }
}

/// Session over.
pub fn detach() {
    *CHANNEL.lock().unwrap() = None;
}

fn kind_of(vendor: Option<u16>, name: &str) -> &'static str {
    let n = name.to_lowercase();
    match vendor {
        Some(0x054c) => "playstation",
        Some(0x045e) => "xbox",
        Some(0x057e) => "nintendo",
        _ if n.contains("playstation") || n.contains("dualsense") || n.contains("dualshock") || n.contains("ps4") || n.contains("ps5") => "playstation",
        _ if n.contains("xbox") || n.contains("x-box") || n.contains("xinput") => "xbox",
        _ if n.contains("nintendo") || n.contains("pro controller") || n.contains("joy-con") => "nintendo",
        _ => "generic",
    }
}

fn kind_code(kind: &str) -> u8 {
    match kind {
        "xbox" => 1,
        "playstation" => 2,
        "nintendo" => 3,
        _ => 0,
    }
}

fn read(gp: &gilrs::Gamepad) -> State {
    use gilrs::{Axis, Button};
    const MAP: [(Button, u32); 15] = [
        (Button::South, bits::A),
        (Button::East, bits::B),
        (Button::West, bits::X),
        (Button::North, bits::Y),
        (Button::DPadUp, bits::UP),
        (Button::DPadDown, bits::DOWN),
        (Button::DPadLeft, bits::LEFT),
        (Button::DPadRight, bits::RIGHT),
        (Button::LeftTrigger, bits::LB),
        (Button::RightTrigger, bits::RB),
        (Button::Start, bits::START),
        (Button::Select, bits::BACK),
        (Button::LeftThumb, bits::LS),
        (Button::RightThumb, bits::RS),
        (Button::Mode, bits::GUIDE),
    ];
    let buttons = MAP.iter().filter(|(b, _)| gp.is_pressed(*b)).fold(0, |acc, (_, bit)| acc | bit);
    let trigger = |b: Button| {
        let v = gp.button_data(b).map(|d| d.value()).unwrap_or(0.0);
        // Digital triggers (some controllers) report no value: full when pressed.
        let v = if gp.is_pressed(b) && v == 0.0 { 1.0 } else { v };
        (v.clamp(0.0, 1.0) * 255.0).round() as u8
    };
    let axis = |a: Axis, flip: bool| {
        let v = gp.value(a);
        let v = if flip { -v } else { v };
        (v.clamp(-1.0, 1.0) * 32767.0).round() as i16
    };
    State {
        buttons,
        lt: trigger(Button::LeftTrigger2),
        rt: trigger(Button::RightTrigger2),
        lx: axis(Axis::LeftStickX, false),
        // gilrs: up is positive; the protocol: down is positive.
        ly: axis(Axis::LeftStickY, true),
        rx: axis(Axis::RightStickX, false),
        ry: axis(Axis::RightStickY, true),
    }
}

fn encode(slot: u8, kind: &str, s: &State) -> Bytes {
    let mut p = Vec::with_capacity(17);
    p.push(0u8);
    p.push(slot);
    p.push(kind_code(kind));
    p.extend_from_slice(&s.buttons.to_be_bytes());
    p.push(s.lt);
    p.push(s.rt);
    for v in [s.lx, s.ly, s.rx, s.ry] {
        p.extend_from_slice(&v.to_be_bytes());
    }
    Bytes::from(p)
}

fn send(data: Bytes) {
    let Some((ch, handle)) = CHANNEL.lock().unwrap().clone() else { return };
    handle.spawn(async move {
        // (0.17 checked the state here, synchronously.)
        if !matches!(ch.ready_state().await, Ok(RTCDataChannelState::Open)) {
            return;
        }
        let _ = ch.send(bytes::BytesMut::from(&data[..])).await;
    });
}

fn run(cmds: std_mpsc::Receiver<Cmd>) {
    let mut gilrs = match gilrs::Gilrs::new() {
        Ok(g) => g,
        Err(gilrs::Error::NotImplemented(g)) => {
            *UNAVAILABLE.lock().unwrap() = Some("controllers aren't supported on this system".into());
            g
        }
        Err(e) => {
            *UNAVAILABLE.lock().unwrap() = Some(format!("controllers unavailable: {}", e));
            return;
        }
    };
    // Sent state and when, per slot (to repeat it and to report removals).
    let mut sent: [Option<(State, Instant)>; MAX_PADS] = Default::default();
    let mut effects: Vec<(gilrs::ff::Effect, Instant)> = Vec::new();
    let mut was_attached = false;

    loop {
        // Wait for input (or a short timeout to repeat states).
        let _ = gilrs.next_event_blocking(Some(Duration::from_millis(20)));
        while gilrs.next_event().is_some() {}

        while let Ok(cmd) = cmds.try_recv() {
            match cmd {
                Cmd::Vibrate(slot) => {
                    let target = gilrs.gamepads().find(|(id, _)| usize::from(*id) == slot as usize).map(|(id, _)| id);
                    if let Some(id) = target {
                        use gilrs::ff::{BaseEffect, BaseEffectType, EffectBuilder, Replay, Ticks};
                        let play = Replay { play_for: Ticks::from_ms(600), ..Default::default() };
                        let built = EffectBuilder::new()
                            .add_effect(BaseEffect { kind: BaseEffectType::Strong { magnitude: 60_000 }, scheduling: play, ..Default::default() })
                            .add_effect(BaseEffect { kind: BaseEffectType::Weak { magnitude: 60_000 }, scheduling: play, ..Default::default() })
                            .gamepads(&[id])
                            .finish(&mut gilrs);
                        match built {
                            Ok(effect) => {
                                let _ = effect.play();
                                effects.push((effect, Instant::now()));
                            }
                            Err(e) => eprintln!("Controller {}: cannot vibrate: {}", slot, e),
                        }
                    }
                }
            }
        }
        effects.retain(|(_, at)| at.elapsed() < Duration::from_secs(2));

        let pads: Vec<Pad> = gilrs
            .gamepads()
            .filter(|(id, gp)| gp.is_connected() && usize::from(*id) < MAX_PADS)
            .map(|(id, gp)| {
                let name = gp.name().to_string();
                Pad {
                    slot: usize::from(id) as u8,
                    kind: kind_of(gp.vendor_id(), &name),
                    name,
                    rumble: gp.is_ff_supported(),
                    state: read(&gp),
                }
            })
            .collect();

        let attached = CHANNEL.lock().unwrap().is_some();
        if attached && !was_attached {
            sent = Default::default();
        }
        was_attached = attached;
        if attached {
            let enabled = ENABLED.load(Ordering::Relaxed);
            let mut present = [false; MAX_PADS];
            if enabled {
                for p in &pads {
                    let i = p.slot as usize;
                    present[i] = true;
                    let due = match sent[i] {
                        Some((s, at)) => s != p.state || at.elapsed() >= KEEPALIVE,
                        None => true,
                    };
                    if due {
                        send(encode(p.slot, p.kind, &p.state));
                        sent[i] = Some((p.state, Instant::now()));
                    }
                }
            }
            for i in 0..MAX_PADS {
                if !present[i] && sent[i].take().is_some() {
                    send(Bytes::from(vec![1u8, i as u8]));
                }
            }
        }
        *PADS.lock().unwrap() = pads;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encodes_like_the_agent_reads() {
        let s = State { buttons: bits::A | bits::GUIDE, lt: 1, rt: 255, lx: -32768, ly: 32767, rx: 5, ry: -5 };
        let b = encode(3, "playstation", &s);
        assert_eq!(b.len(), 17);
        assert_eq!(&b[..3], &[0, 3, 2]);
        assert_eq!(u32::from_be_bytes([b[3], b[4], b[5], b[6]]), bits::A | bits::GUIDE);
        assert_eq!(i16::from_be_bytes([b[9], b[10]]), -32768);
        assert_eq!(i16::from_be_bytes([b[15], b[16]]), -5);
    }

    #[test]
    fn kinds_from_vendor_or_name() {
        assert_eq!(kind_of(Some(0x054c), "x"), "playstation");
        assert_eq!(kind_of(None, "Xbox Wireless Controller"), "xbox");
        assert_eq!(kind_of(None, "8BitDo"), "generic");
    }
}
