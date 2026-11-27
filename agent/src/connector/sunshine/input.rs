//! Mouse, keyboard and text for a GameStream host: the packets of
//! moonlight-common-c `Input.h`, built as `InputStream.c` builds them for a
//! generation-7 host, and the bookkeeping that turns Lunaris' commands into
//! them.
//!
//! How they travel (InputStream.c sendInputPacket, `encryptedControlStream`):
//! each packet as it is, as the payload of control message 0x0206, reliable,
//! on the keyboard or the mouse channel of the control stream, whose
//! encryption covers it ([`super::control::Control::send_input`]). There is
//! no input encryption of its own any more for these hosts.
//!
//! The host's reading of every packet: Sunshine `src/input.cpp`
//! (`validate_input_packet`: the size field must be exact; the `passthrough`
//! overloads), tag v2026.914 and master alike.
//!
//! Layout of every packet: size (big-endian u32, counted without itself),
//! magic (little-endian u32), then the fields; they are big-endian except the
//! key code.
//!
//! Game controllers: as moonlight-common-c sends them to Sunshine — an
//! arrival (`SS_CONTROLLER_ARRIVAL_PACKET`, type and buttons) when one is
//! first seen, then whole states (`NV_MULTI_CONTROLLER_PACKET`, generation
//! 5), each controller on its own channel (`CTRL_CHANNEL_GAMEPAD_BASE` + its
//! number); an unplugging is a state whose `activeGamepadMask` no longer has
//! its bit (Sunshine input.cpp frees the controller). No rumble, motion or
//! controller touch pad.
//!
//! Limits: no touch, no pen (their packets are not built).
//!
//! Nothing here logs a key, a text or a position.

use super::super::{Button, Command};
use super::control::{CTRL_CHANNEL_KEYBOARD, CTRL_CHANNEL_MOUSE};
use super::keymap;

// Input.h
const KEY_DOWN_EVENT_MAGIC: u32 = 0x03;
const KEY_UP_EVENT_MAGIC: u32 = 0x04;
const MOUSE_MOVE_ABS_MAGIC: u32 = 0x05;
const MOUSE_MOVE_REL_MAGIC_GEN5: u32 = 0x07;
const MOUSE_BUTTON_DOWN_EVENT_MAGIC_GEN5: u32 = 0x08;
const MOUSE_BUTTON_UP_EVENT_MAGIC_GEN5: u32 = 0x09;
const SCROLL_MAGIC_GEN5: u32 = 0x0A;
const UTF8_TEXT_EVENT_MAGIC: u32 = 0x17;
const SS_HSCROLL_MAGIC: u32 = 0x5500_0001;
const MULTI_CONTROLLER_MAGIC_GEN5: u32 = 0x0C;
const SS_CONTROLLER_ARRIVAL_MAGIC: u32 = 0x5500_0004;
// NV_MULTI_CONTROLLER_PACKET's fixed fields.
const MC_HEADER_B: u16 = 0x001A;
const MC_MID_B: u16 = 0x0014;
const MC_TAIL_A: u16 = 0x009C;
const MC_TAIL_B: u16 = 0x0055;
/// Limelight-internal.h: controller n goes on channel 0x10 + n.
const CTRL_CHANNEL_GAMEPAD_BASE: u8 = 0x10;
/// Sunshine's most controllers (bits of `activeGamepadMask`).
pub const MAX_GAMEPADS: u8 = 16;
// Limelight.h button flags (the Xbox ones are XInput's bits).
const TOUCHPAD_FLAG: u32 = 0x10_0000;
/// Every Xbox 360 button: A B X Y, the pad, LB RB, Start Back, the sticks, Guide.
const XBOX_BUTTONS: u32 = 0xF7FF;
const LI_CTYPE_PS: u8 = 0x02;
const LI_CCAP_ANALOG_TRIGGERS: u16 = 0x01;
/// The most text bytes one packet holds.
pub const UTF8_TEXT_EVENT_MAX_COUNT: usize = 32;

// Limelight.h
pub const BUTTON_LEFT: u8 = 0x01;
pub const BUTTON_MIDDLE: u8 = 0x02;
pub const BUTTON_RIGHT: u8 = 0x03;
pub const BUTTON_X1: u8 = 0x04;
pub const BUTTON_X2: u8 = 0x05;
pub const MODIFIER_SHIFT: u8 = 0x01;
pub const MODIFIER_CTRL: u8 = 0x02;
pub const MODIFIER_ALT: u8 = 0x04;
pub const MODIFIER_META: u8 = 0x08;
/// InputStream.c LI_WHEEL_DELTA: one notch of a wheel.
pub const LI_WHEEL_DELTA: i32 = 120;

fn packet(magic: u32, fields: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(8 + fields.len());
    out.extend_from_slice(&(4 + fields.len() as u32).to_be_bytes());
    out.extend_from_slice(&magic.to_le_bytes());
    out.extend_from_slice(fields);
    out
}

/// NV_REL_MOUSE_MOVE_PACKET (InputStream.c LiSendMouseMoveEvent).
pub fn mouse_move_relative(dx: i16, dy: i16) -> Vec<u8> {
    let mut fields = [0u8; 4];
    fields[0..2].copy_from_slice(&dx.to_be_bytes());
    fields[2..4].copy_from_slice(&dy.to_be_bytes());
    packet(MOUSE_MOVE_REL_MAGIC_GEN5, &fields)
}

/// A movement of any size as the packets it takes (InputStream.c
/// inputSendThreadProc: "Send as many packets as it takes to get the entire
/// delta through"). Nothing for no movement.
pub fn mouse_move_relative_split(mut dx: i32, mut dy: i32) -> Vec<Vec<u8>> {
    let mut out = Vec::new();
    while dx != 0 || dy != 0 {
        let step_x = dx.clamp(i16::MIN as i32, i16::MAX as i32);
        let step_y = dy.clamp(i16::MIN as i32, i16::MAX as i32);
        out.push(mouse_move_relative(step_x as i16, step_y as i16));
        dx -= step_x;
        dy -= step_y;
    }
    out
}

/// NV_ABS_MOUSE_MOVE_PACKET (InputStream.c LiSendMousePositionEvent): a
/// position within a reference of `width` x `height`; the host scales it to
/// the display it shows (input.cpp passthrough(PNV_ABS_MOUSE_MOVE_PACKET)).
/// One is taken off both dimensions, as the reference does for a rounding
/// error at the far edge.
pub fn mouse_position(x: i16, y: i16, width: i16, height: i16) -> Vec<u8> {
    let mut fields = [0u8; 10];
    fields[0..2].copy_from_slice(&x.to_be_bytes());
    fields[2..4].copy_from_slice(&y.to_be_bytes());
    // fields[4..6]: unused
    fields[6..8].copy_from_slice(&(width - 1).to_be_bytes());
    fields[8..10].copy_from_slice(&(height - 1).to_be_bytes());
    packet(MOUSE_MOVE_ABS_MAGIC, &fields)
}

/// NV_MOUSE_BUTTON_PACKET (InputStream.c LiSendMouseButtonEvent: the action
/// 0x07 / 0x08 plus one from generation 5 on).
pub fn mouse_button(button: u8, down: bool) -> Vec<u8> {
    let magic = if down { MOUSE_BUTTON_DOWN_EVENT_MAGIC_GEN5 } else { MOUSE_BUTTON_UP_EVENT_MAGIC_GEN5 };
    packet(magic, &[button])
}

/// NV_SCROLL_PACKET (InputStream.c LiSendHighResScrollEvent): 120 is one
/// notch away from the person (up).
pub fn scroll(amount: i16) -> Vec<u8> {
    let mut fields = [0u8; 6];
    fields[0..2].copy_from_slice(&amount.to_be_bytes());
    fields[2..4].copy_from_slice(&amount.to_be_bytes());
    packet(SCROLL_MAGIC_GEN5, &fields)
}

/// SS_HSCROLL_PACKET (InputStream.c LiSendHighResHScrollEvent, a Sunshine
/// extension): 120 is one notch to the right.
pub fn horizontal_scroll(amount: i16) -> Vec<u8> {
    packet(SS_HSCROLL_MAGIC, &amount.to_be_bytes())
}

/// NV_KEYBOARD_PACKET (InputStream.c LiSendKeyboardEvent2). `vk`: the Windows
/// virtual-key code; it goes with 0x80 in the high byte as moonlight-qt sends
/// it (app/streaming/input/keyboard.cpp `keyCode |= 0x8000`; Sunshine reads
/// only the low byte). `flags` 0: the code names the key by its place on a US
/// keyboard; `SS_KBE_FLAG_NON_NORMALIZED`: by itself (see keymap.rs).
pub fn keyboard(vk: u8, down: bool, modifiers: u8, flags: u8) -> Vec<u8> {
    let key_code = 0x8000u16 | vk as u16;
    let fields = [flags, key_code as u8, (key_code >> 8) as u8, modifiers, 0, 0];
    packet(if down { KEY_DOWN_EVENT_MAGIC } else { KEY_UP_EVENT_MAGIC }, &fields)
}

/// NV_UNICODE_PACKET (InputStream.c LiSendUtf8TextEvent and the splitting in
/// inputSendThreadProc for Sunshine: as many whole characters as fit in 32
/// bytes per packet). The host types the text itself, whatever its layout.
pub fn utf8_text(text: &str) -> Vec<Vec<u8>> {
    let mut out = Vec::new();
    let mut chunk = String::new();
    for c in text.chars() {
        if chunk.len() + c.len_utf8() > UTF8_TEXT_EVENT_MAX_COUNT {
            out.push(packet(UTF8_TEXT_EVENT_MAGIC, chunk.as_bytes()));
            chunk.clear();
        }
        chunk.push(c);
    }
    if !chunk.is_empty() {
        out.push(packet(UTF8_TEXT_EVENT_MAGIC, chunk.as_bytes()));
    }
    out
}

/// SS_CONTROLLER_ARRIVAL_PACKET (InputStream.c LiSendControllerArrivalEvent):
/// fields little-endian.
pub fn controller_arrival(number: u8, kind: u8, capabilities: u16, buttons: u32) -> Vec<u8> {
    let mut fields = Vec::with_capacity(8);
    fields.push(number);
    fields.push(kind);
    fields.extend_from_slice(&capabilities.to_le_bytes());
    fields.extend_from_slice(&buttons.to_le_bytes());
    packet(SS_CONTROLLER_ARRIVAL_MAGIC, &fields)
}

/// NV_MULTI_CONTROLLER_PACKET for a generation 5+ host
/// (InputStream.c sendControllerEventInternal): fields little-endian,
/// sticks with y growing upwards, the buttons above 16 bits in
/// `buttonFlags2` (a Sunshine extension).
#[allow(clippy::too_many_arguments)]
pub fn multi_controller(
    number: u8,
    active_mask: u16,
    buttons: u32,
    left_trigger: u8,
    right_trigger: u8,
    sticks: [i16; 4],
) -> Vec<u8> {
    let mut fields = Vec::with_capacity(22);
    let le16 = |fields: &mut Vec<u8>, v: u16| fields.extend_from_slice(&v.to_le_bytes());
    le16(&mut fields, MC_HEADER_B);
    le16(&mut fields, number as u16);
    le16(&mut fields, active_mask);
    le16(&mut fields, MC_MID_B);
    le16(&mut fields, buttons as u16);
    fields.push(left_trigger);
    fields.push(right_trigger);
    for v in sticks {
        le16(&mut fields, v as u16);
    }
    le16(&mut fields, MC_TAIL_A);
    le16(&mut fields, (buttons >> 16) as u16);
    le16(&mut fields, MC_TAIL_B);
    packet(MULTI_CONTROLLER_MAGIC_GEN5, &fields)
}

/// Lunaris' controller buttons (gamepad.rs) as Limelight.h flags (XInput's
/// bits for the Xbox ones).
fn controller_buttons(buttons: u32) -> u32 {
    use crate::gamepad::buttons as b;
    const MAP: [(u32, u32); 16] = [
        (b::UP, 0x0001),
        (b::DOWN, 0x0002),
        (b::LEFT, 0x0004),
        (b::RIGHT, 0x0008),
        (b::START, 0x0010),
        (b::BACK, 0x0020),
        (b::LS, 0x0040),
        (b::RS, 0x0080),
        (b::LB, 0x0100),
        (b::RB, 0x0200),
        (b::GUIDE, 0x0400),
        (b::A, 0x1000),
        (b::B, 0x2000),
        (b::X, 0x4000),
        (b::Y, 0x8000),
        (b::TOUCHPAD, TOUCHPAD_FLAG),
    ];
    MAP.iter().filter(|(ours, _)| buttons & ours != 0).fold(0, |flags, (_, theirs)| flags | theirs)
}

/// y grows downwards here and upwards in the protocol (XInput's way).
fn flip_y(v: i16) -> i16 {
    clamp16(-(v as i32))
}

/// One input packet and the control stream channel it goes on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InputPacket {
    pub channel: u8,
    pub data: Vec<u8>,
}

fn mouse(data: Vec<u8>) -> InputPacket {
    InputPacket { channel: CTRL_CHANNEL_MOUSE, data }
}

fn key(data: Vec<u8>) -> InputPacket {
    InputPacket { channel: CTRL_CHANNEL_KEYBOARD, data }
}

fn clamp16(value: i32) -> i16 {
    value.clamp(i16::MIN as i32, i16::MAX as i32) as i16
}

/// Lunaris' input commands as GameStream packets, with what has to be
/// remembered between them: the stream's size (the reference of absolute
/// positions) and which modifier keys are held.
pub struct InputEncoder {
    width: i16,
    height: i16,
    /// The modifier keys this side holds down, as `MODIFIER_*`, one bit per
    /// physical key so that two Shift keys do not cancel each other.
    held: u8,
    /// The viewer's controllers the host was told of, one bit each.
    gamepads: u16,
}

// One bit per modifier key (left, right).
const HELD_LSHIFT: u8 = 0x01;
const HELD_RSHIFT: u8 = 0x02;
const HELD_LCTRL: u8 = 0x04;
const HELD_RCTRL: u8 = 0x08;
const HELD_LALT: u8 = 0x10;
const HELD_RALT: u8 = 0x20;
const HELD_LMETA: u8 = 0x40;
const HELD_RMETA: u8 = 0x80;

impl InputEncoder {
    /// `width`, `height`: the size the stream was asked in, which is what the
    /// viewer's pointer positions are measured in.
    pub fn new(width: u32, height: u32) -> InputEncoder {
        InputEncoder {
            width: width.clamp(2, i16::MAX as u32) as i16,
            height: height.clamp(2, i16::MAX as u32) as i16,
            held: 0,
            gamepads: 0,
        }
    }

    /// The packets of a controller's state or unplugging.
    fn gamepad(&mut self, id: u8, kind: u8, state: Option<&crate::gamepad::PadState>) -> Vec<InputPacket> {
        if id >= MAX_GAMEPADS {
            return Vec::new();
        }
        let bit = 1u16 << id;
        let channel = CTRL_CHANNEL_GAMEPAD_BASE + id;
        let Some(state) = state else {
            if self.gamepads & bit == 0 {
                return Vec::new();
            }
            self.gamepads &= !bit;
            // The host frees a controller whose bit is gone from the mask.
            return vec![InputPacket { channel, data: multi_controller(id, self.gamepads, 0, 0, 0, [0; 4]) }];
        };
        let mut out = Vec::with_capacity(2);
        if self.gamepads & bit == 0 {
            self.gamepads |= bit;
            let supported = if kind == LI_CTYPE_PS { XBOX_BUTTONS | TOUCHPAD_FLAG } else { XBOX_BUTTONS };
            out.push(InputPacket { channel, data: controller_arrival(id, kind, LI_CCAP_ANALOG_TRIGGERS, supported) });
        }
        let sticks = [state.lx, flip_y(state.ly), state.rx, flip_y(state.ry)];
        out.push(InputPacket {
            channel,
            data: multi_controller(id, self.gamepads, controller_buttons(state.buttons), state.lt, state.rt, sticks),
        });
        out
    }

    /// The `modifiers` byte of a keyboard packet: Sunshine presses and
    /// releases around a key whatever of Shift, Control and Alt this names
    /// and it has not seen pressed (input.cpp passthrough(PNV_KEYBOARD_PACKET),
    /// `synthetic_modifiers`), so it must name exactly what is held.
    fn modifiers(&self) -> u8 {
        let mut m = 0;
        if self.held & (HELD_LSHIFT | HELD_RSHIFT) != 0 {
            m |= MODIFIER_SHIFT;
        }
        if self.held & (HELD_LCTRL | HELD_RCTRL) != 0 {
            m |= MODIFIER_CTRL;
        }
        if self.held & (HELD_LALT | HELD_RALT) != 0 {
            m |= MODIFIER_ALT;
        }
        if self.held & (HELD_LMETA | HELD_RMETA) != 0 {
            m |= MODIFIER_META;
        }
        m
    }

    /// The packets of one command; none for a command that is not input, a
    /// key the protocol has no name for, or a movement of nothing.
    pub fn encode(&mut self, command: &Command) -> Vec<InputPacket> {
        match command {
            Command::PointerMove { x, y } => {
                let x = (*x).clamp(0, self.width as i32 - 1) as i16;
                let y = (*y).clamp(0, self.height as i32 - 1) as i16;
                vec![mouse(mouse_position(x, y, self.width, self.height))]
            }
            Command::PointerMoveRelative { dx, dy } => {
                mouse_move_relative_split(*dx, *dy).into_iter().map(mouse).collect()
            }
            Command::PointerButton { button, down } => {
                let button = match button {
                    Button::Left => BUTTON_LEFT,
                    Button::Middle => BUTTON_MIDDLE,
                    Button::Right => BUTTON_RIGHT,
                    Button::Back => BUTTON_X1,
                    Button::Forward => BUTTON_X2,
                };
                vec![mouse(mouse_button(button, *down))]
            }
            Command::Wheel { dx, dy } => {
                // `dy` > 0 is up and `dx` > 0 is right on both sides:
                // moonlight-qt sends SDL's wheel numbers times 120
                // (app/streaming/input/mouse.cpp; SDL: y away from the
                // person and x to the right are positive), and the host gives
                // them to the system's wheel as they are (libvirtualhid
                // uhid_backend.cpp: REL_WHEEL / REL_HWHEEL).
                let mut out = Vec::new();
                if *dy != 0 {
                    out.push(mouse(scroll(clamp16(dy.saturating_mul(LI_WHEEL_DELTA)))));
                }
                if *dx != 0 {
                    out.push(mouse(horizontal_scroll(clamp16(dx.saturating_mul(LI_WHEEL_DELTA)))));
                }
                out
            }
            Command::Key { position, down } => {
                let Some(vk) = keymap::vk_of_position(*position) else {
                    return Vec::new();
                };
                let bit = match vk {
                    keymap::VK_LSHIFT => HELD_LSHIFT,
                    keymap::VK_RSHIFT => HELD_RSHIFT,
                    keymap::VK_LCONTROL => HELD_LCTRL,
                    keymap::VK_RCONTROL => HELD_RCTRL,
                    keymap::VK_LMENU => HELD_LALT,
                    keymap::VK_RMENU => HELD_RALT,
                    keymap::VK_LWIN => HELD_LMETA,
                    keymap::VK_RWIN => HELD_RMETA,
                    _ => 0,
                };
                if *down {
                    self.held |= bit;
                } else {
                    self.held &= !bit;
                }
                vec![key(keyboard(vk, *down, self.modifiers(), keymap::flags_of_vk(vk)))]
            }
            Command::Text { text } => utf8_text(text).into_iter().map(key).collect(),
            Command::Gamepad { id, kind, state } => self.gamepad(*id, *kind, state.as_ref()),
            _ => Vec::new(),
        }
    }
}

/// Makes less of a run of commands that came at once, as InputStream.c does
/// for what queues up within its batching interval: of absolute positions in
/// a row only the last counts ("we should only send the latest"), relative
/// movements in a row add up. Anything else ends a run, so clicks and keys
/// stay in order with the movements around them.
pub fn coalesce(commands: Vec<Command>) -> Vec<Command> {
    let mut out: Vec<Command> = Vec::with_capacity(commands.len());
    for command in commands {
        match (out.last_mut(), &command) {
            (Some(Command::PointerMove { x, y }), Command::PointerMove { x: nx, y: ny }) => {
                *x = *nx;
                *y = *ny;
            }
            (Some(Command::PointerMoveRelative { dx, dy }), Command::PointerMoveRelative { dx: ndx, dy: ndy }) => {
                *dx = dx.saturating_add(*ndx);
                *dy = dy.saturating_add(*ndy);
            }
            // InputStream.c batches a controller's states while its buttons
            // stay the same: only the sticks and triggers of the last count.
            (
                Some(Command::Gamepad { id, state: Some(last), .. }),
                Command::Gamepad { id: nid, state: Some(next), .. },
            ) if id == nid && last.buttons == next.buttons => {
                *last = *next;
            }
            _ => out.push(command),
        }
    }
    out
}

/// Whether a command is input (what the test switch
/// `LUNARIS_CONNECTOR_TEST_NO_INPUT` holds back).
pub fn is_input(command: &Command) -> bool {
    matches!(
        command,
        Command::PointerMove { .. }
            | Command::PointerMoveRelative { .. }
            | Command::PointerButton { .. }
            | Command::Wheel { .. }
            | Command::Key { .. }
            | Command::Text { .. }
            | Command::Gamepad { .. }
    )
}

#[cfg(test)]
#[path = "input_tests.rs"]
mod tests;
