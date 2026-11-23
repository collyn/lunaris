use bytes::Bytes;

/// Qt's KeypadModifier: the key is on the numeric keypad.
const QT_KEYPAD_MODIFIER: i32 = 0x2000_0000;

/// Maps a Qt key code to the Windows virtual-key code of the physical key on a
/// US keyboard, which is what the agent injects (the web client does the same
/// from `KeyboardEvent.code`). Qt reports the produced character, so shifted
/// symbols map back to their unshifted key; the Shift modifier is still held
/// on the host, which then types the symbol itself.
pub fn map_qt_key_to_vk(key: i32, modifiers_mask: i32) -> u16 {
    if (modifiers_mask & QT_KEYPAD_MODIFIER) != 0 {
        let vk = match key {
            k @ 0x30..=0x39 => Some(96 + (k - 0x30) as u16), // Numpad0..9
            0x2a => Some(106),                               // Numpad *
            0x2b => Some(107),                               // Numpad +
            0x2d => Some(109),                               // Numpad -
            0x2e | 0x2c => Some(110),                        // Numpad decimal
            0x2f => Some(111),                               // Numpad /
            _ => None, // Enter and the NumLock-off navigation keys below
        };
        if let Some(vk) = vk {
            return vk;
        }
    }

    match key {
        0x20 => 32,                       // Space
        k @ 0x30..=0x39 => k as u16,      // 0..9
        k @ 0x41..=0x5a => k as u16,      // A..Z
        0x29 => 0x30,                     // ) on 0
        0x21 => 0x31,                     // ! on 1
        0x40 => 0x32,                     // @ on 2
        0x23 => 0x33,                     // # on 3
        0x24 => 0x34,                     // $ on 4
        0x25 => 0x35,                     // % on 5
        0x5e => 0x36,                     // ^ on 6
        0x26 => 0x37,                     // & on 7
        0x2a => 0x38,                     // * on 8
        0x28 => 0x39,                     // ( on 9
        0x3b | 0x3a => 186,               // ; :
        0x3d | 0x2b => 187,               // = +
        0x2c | 0x3c => 188,               // , <
        0x2d | 0x5f => 189,               // - _
        0x2e | 0x3e => 190,               // . >
        0x2f | 0x3f => 191,               // / ?
        0x60 | 0x7e => 192,               // ` ~
        0x5b | 0x7b => 219,               // [ {
        0x5c | 0x7c => 220,               // \ |
        0x5d | 0x7d => 221,               // ] }
        0x27 | 0x22 => 222,               // ' "

        0x01000003 => 8,                  // Backspace
        0x01000001 | 0x01000002 => 9,     // Tab / Backtab (Shift+Tab)
        0x0100000b => 12,                 // Clear (keypad 5 without NumLock)
        0x01000004 | 0x01000005 => 13,    // Return / Enter
        0x01000020 => 16,                 // Shift
        0x01000021 => 17,                 // Control
        0x01000023 | 0x01001103 => 18,    // Alt / AltGr
        0x01000008 => 19,                 // Pause
        0x01000024 => 20,                 // CapsLock
        0x01000000 => 27,                 // Escape
        0x01000016 => 33,                 // PageUp
        0x01000017 => 34,                 // PageDown
        0x01000011 => 35,                 // End
        0x01000010 => 36,                 // Home
        0x01000012 => 37,                 // Left
        0x01000013 => 38,                 // Up
        0x01000014 => 39,                 // Right
        0x01000015 => 40,                 // Down
        0x01000009 => 44,                 // PrintScreen
        0x01000006 => 45,                 // Insert
        0x01000007 => 46,                 // Delete
        0x01000022 | 0x01000053 => 91,    // Meta / Super_L (Windows / Command key)
        0x01000054 => 92,                 // Super_R
        0x01000055 => 93,                 // Menu (context menu key)
        0x01000025 => 144,                // NumLock
        0x01000026 => 145,                // ScrollLock

        // F1 to F24
        k @ 0x01000030..=0x01000047 => (112 + (k - 0x01000030)) as u16,
        _ => 0,
    }
}

pub struct InputSenders {
    pub keyboard: tokio::sync::mpsc::UnboundedSender<Bytes>,
    pub mouse_abs: tokio::sync::mpsc::UnboundedSender<Bytes>,
    pub mouse_rel: tokio::sync::mpsc::UnboundedSender<Bytes>,
    pub stream_width: u32,
    pub stream_height: u32,
}

/// The key at a physical position, from Qt's `nativeScanCode`. On Linux
/// (xcb and Wayland) that is the kernel's key code plus 8, so the position
/// is known whatever layout this computer has, which is what the web client
/// sends from `KeyboardEvent.code`. The table is the agent's own
/// (`connector/keys.rs`, turned round by `connector/sunshine/keymap.rs`):
/// Shift, Control and Alt come with their side, the keypad's Enter is Return.
/// 0 when the position is not known (other systems: Qt's code is used then).
///
/// The three volume keys are left to this computer: a Windows host presses
/// keys by `MapVirtualKeyW(vk)` without the extended flag for them
/// (lunaris-media `keyboard_key`, `vk_is_extended`), which are the scan
/// codes of other keys.
pub fn map_native_scan_to_vk(native_scan_code: u32) -> u16 {
    #[cfg(target_os = "linux")]
    {
        if native_scan_code < 9 {
            return 0;
        }
        match agent::connector::sunshine::keymap::vk_of_position(native_scan_code - 8) {
            Some(0xAD..=0xAF) | None => 0,
            Some(vk) => vk as u16,
        }
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = native_scan_code;
        0
    }
}

/// Alt, with or without a side (VK_MENU, VK_LMENU, VK_RMENU).
fn is_alt(vk: u16) -> bool {
    matches!(vk, 0x12 | 0xA4 | 0xA5)
}

/// The keys this viewer has pressed on the remote computer and not let go.
/// They are let go there when the viewer stops getting the keyboard (another
/// window is activated, the menu opens, the stream ends), because the
/// releases then never arrive here: moonlight-qt does the same with
/// `m_KeysDown` and `raiseAllKeys()` (app/streaming/input/keyboard.cpp
/// 521-537, called from input.cpp notifyFocusLost 273-285).
#[derive(Default)]
pub struct PressedKeys {
    down: Vec<u16>,
}

impl PressedKeys {
    /// Notes a key event; false when it is not to be sent.
    ///
    /// A release that is part of the key repeating is not sent: Qt reports a
    /// held key as release and press pairs, which would let the key go on
    /// the remote computer many times a second. The repeated presses are
    /// sent, as the web client sends its repeated `keydown`.
    pub fn note(&mut self, vk: u16, is_down: bool, is_auto_repeat: bool) -> bool {
        if is_down {
            if !self.down.contains(&vk) {
                self.down.push(vk);
            }
            true
        } else if is_auto_repeat {
            false
        } else {
            self.down.retain(|k| *k != vk);
            true
        }
    }

    /// The events that let every pressed key go, last pressed first, as
    /// (virtual key, is_down). After Alt, one more press and release of it:
    /// RustDesk does so (src/keyboard.rs release_remote_keys_for_events,
    /// 791-807: "the controlled side may hold the alt state, if local window
    /// is switched by [Alt + Tab]") - a lone Alt opens the window's menu on
    /// Windows, the second one closes it.
    pub fn release_all(&mut self) -> Vec<(u16, bool)> {
        let mut events = Vec::new();
        for vk in std::mem::take(&mut self.down).into_iter().rev() {
            events.push((vk, false));
            if is_alt(vk) {
                events.push((vk, true));
                events.push((vk, false));
            }
        }
        events
    }

    pub fn is_empty(&self) -> bool {
        self.down.is_empty()
    }
}

static PRESSED_KEYS: std::sync::Mutex<PressedKeys> = std::sync::Mutex::new(PressedKeys { down: Vec::new() });

/// The modifier bits of a key packet from Qt's modifiers
/// (ShiftModifier 0x02000000, ControlModifier 0x04000000, AltModifier
/// 0x08000000, MetaModifier 0x10000000).
fn packet_modifiers(modifiers_mask: i32) -> u8 {
    let mut modifiers = 0u8;
    if (modifiers_mask & 0x02000000) != 0 {
        modifiers |= 1;
    }
    if (modifiers_mask & 0x04000000) != 0 {
        modifiers |= 2;
    }
    if (modifiers_mask & 0x08000000) != 0 {
        modifiers |= 4;
    }
    if (modifiers_mask & 0x10000000) != 0 {
        modifiers |= 8;
    }
    modifiers
}

/// Type 0 key packet: [0, down, modifiers, vk (u16 big-endian)].
fn key_packet(vk: u16, is_down: bool, modifiers: u8) -> Bytes {
    Bytes::from(vec![0u8, if is_down { 1 } else { 0 }, modifiers, (vk >> 8) as u8, (vk & 0xFF) as u8])
}

pub fn handle_key_event(key: i32, modifiers_mask: i32, is_down: bool, senders: &InputSenders) {
    handle_key_event_at(key, modifiers_mask, 0, is_down, false, senders);
}

/// A key event of the stream view. `native_scan_code` names the key's
/// position when the window system gives it ([`map_native_scan_to_vk`]);
/// Qt's key code is used otherwise.
pub fn handle_key_event_at(
    key: i32,
    modifiers_mask: i32,
    native_scan_code: u32,
    is_down: bool,
    is_auto_repeat: bool,
    senders: &InputSenders,
) {
    let mut vk = map_native_scan_to_vk(native_scan_code);
    if vk == 0 {
        vk = map_qt_key_to_vk(key, modifiers_mask);
    }
    if vk > 0 {
        if !PRESSED_KEYS.lock().unwrap().note(vk, is_down, is_auto_repeat) {
            return;
        }
        let _ = senders.keyboard.send(key_packet(vk, is_down, packet_modifiers(modifiers_mask)));
    }
}

/// Lets go, on the remote computer, of every key this viewer holds there.
pub fn release_all_keys(senders: &InputSenders) {
    for (vk, is_down) in PRESSED_KEYS.lock().unwrap().release_all() {
        let _ = senders.keyboard.send(key_packet(vk, is_down, 0));
    }
}

/// The stream ended: nothing is held any more (there is nowhere to send to).
pub fn forget_pressed_keys() {
    PRESSED_KEYS.lock().unwrap().release_all();
}

pub fn get_client_timestamp() -> u32 {
    static SEQ: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    SEQ.fetch_add(1, std::sync::atomic::Ordering::SeqCst)
}

pub fn handle_mouse_move(
    x: i32,
    y: i32,
    width: i32,
    height: i32,
    rx: i32,
    ry: i32,
    pointer_locked: bool,
    senders: &InputSenders,
) {
    let ts = get_client_timestamp();
    if !pointer_locked {
        let mut buf = vec![0u8; 13];
        buf[0] = 1; // Type 1: Absolute Mouse Position
        buf[1..3].copy_from_slice(&(x as i16).to_be_bytes());
        buf[3..5].copy_from_slice(&(y as i16).to_be_bytes());
        buf[5..7].copy_from_slice(&(width as i16).to_be_bytes());
        buf[7..9].copy_from_slice(&(height as i16).to_be_bytes());
        buf[9..13].copy_from_slice(&ts.to_be_bytes());
        let _ = senders.mouse_abs.send(Bytes::from(buf));
    } else {
        let clamp_i16 =
            |value: f64| -> i16 { value.round().clamp(i16::MIN as f64, i16::MAX as f64) as i16 };
        let scaled_rx = if senders.stream_width > 0 && width > 0 {
            clamp_i16(rx as f64 * (senders.stream_width as f64 / width as f64))
        } else {
            rx.clamp(i16::MIN as i32, i16::MAX as i32) as i16
        };
        let scaled_ry = if senders.stream_height > 0 && height > 0 {
            clamp_i16(ry as f64 * (senders.stream_height as f64 / height as f64))
        } else {
            ry.clamp(i16::MIN as i32, i16::MAX as i32) as i16
        };

        let mut buf = vec![0u8; 5];
        buf[0] = 0; // Type 0: Relative Mouse Move (without timestamp)
        buf[1..3].copy_from_slice(&scaled_rx.to_be_bytes());
        buf[3..5].copy_from_slice(&scaled_ry.to_be_bytes());
        let _ = senders.mouse_rel.send(Bytes::from(buf));
    }
}

/// Mouse packet type 5: the remote computer's game mouse on or off (the
/// web client's src/gameMouse.ts sends the same). On the relative channel,
/// which does not resend: main.qml says it again every 2 s while it is on.
pub fn game_mouse_packet(on: bool) -> [u8; 2] {
    [5, u8::from(on)]
}

pub fn send_game_mouse(on: bool, senders: &InputSenders) {
    let _ = senders.mouse_rel.send(Bytes::copy_from_slice(&game_mouse_packet(on)));
}

pub fn handle_mouse_click(button: i32, is_down: bool, senders: &InputSenders) {
    if button > 0 {
        let mut buf = vec![0u8; 3];
        buf[0] = 2; // Type 2: Mouse Button Event
        buf[1] = if is_down { 1 } else { 0 };
        buf[2] = button as u8;
        let _ = senders.mouse_rel.send(Bytes::from(buf));
    }
}

/// Wheel deltas not yet sent, in Qt angle units (120 = one notch).
static WHEEL_REMAINDER: std::sync::Mutex<(i32, i32)> = std::sync::Mutex::new((0, 0));

/// Turns accumulated Qt wheel deltas into whole notches. Touchpads (two-finger
/// scrolling) report many small deltas (a few units each); dividing each one by
/// 120 on its own always gave 0, so touchpad scrolling did nothing.
fn take_wheel_notches(remainder: &mut (i32, i32), angle_x: i32, angle_y: i32) -> (i32, i32) {
    remainder.0 += angle_x;
    remainder.1 += angle_y;
    let notches = (remainder.0 / 120, remainder.1 / 120);
    remainder.0 -= notches.0 * 120;
    remainder.1 -= notches.1 * 120;
    notches
}

/// `angle_x`/`angle_y` are QWheelEvent::angleDelta (positive y = away from
/// the user, positive x = to the left).
pub fn handle_mouse_wheel(angle_x: i32, angle_y: i32, senders: &InputSenders) {
    let (notches_x, notches_y) = {
        let mut remainder = WHEEL_REMAINDER.lock().unwrap();
        take_wheel_notches(&mut remainder, angle_x, angle_y)
    };
    if notches_x == 0 && notches_y == 0 {
        return;
    }
    // Type 4 scroll packet: [4, dx, dy] in notches; the agent scrolls right
    // for dx > 0 and up for dy > 0 (the web client sends the same).
    let dx = (-notches_x).clamp(-127, 127) as i8;
    let dy = notches_y.clamp(-127, 127) as i8;
    let buf = vec![4u8, dx as u8, dy as u8];
    let _ = senders.mouse_rel.send(Bytes::from(buf));
}

#[cfg(test)]
mod tests {
    use super::{game_mouse_packet, map_native_scan_to_vk, map_qt_key_to_vk, take_wheel_notches, PressedKeys};

    #[test]
    fn held_keys_are_let_go_when_the_keyboard_is_lost() {
        let mut keys = PressedKeys::default();
        assert!(keys.note(0xA2, true, false)); // Ctrl
        assert!(keys.note(0x43, true, false)); // C
        assert!(keys.note(0x43, false, false));
        assert!(keys.note(0x5B, true, false)); // Win
        assert_eq!(keys.release_all(), vec![(0x5B, false), (0xA2, false)]);
        assert!(keys.is_empty());
        assert!(keys.release_all().is_empty());
    }

    /// Alt+Tab taken by this computer: Alt was sent, its release never comes.
    #[test]
    fn alt_is_let_go_and_tapped_once_more() {
        let mut keys = PressedKeys::default();
        keys.note(0xA4, true, false);
        assert_eq!(keys.release_all(), vec![(0xA4, false), (0xA4, true), (0xA4, false)]);
    }

    #[test]
    fn a_repeating_key_stays_down() {
        let mut keys = PressedKeys::default();
        assert!(keys.note(0x57, true, false));
        // Qt: release and press pairs while the key is held.
        assert!(!keys.note(0x57, false, true));
        assert!(keys.note(0x57, true, true));
        assert!(!keys.is_empty());
        assert!(keys.note(0x57, false, false));
        assert!(keys.is_empty());
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn positions_from_the_native_scan_code() {
        // The kernel's key code plus 8.
        assert_eq!(map_native_scan_to_vk(30 + 8), 0x41); // KEY_A
        assert_eq!(map_native_scan_to_vk(16 + 8), 0x51); // KEY_Q, whatever the layout types there
        assert_eq!(map_native_scan_to_vk(42 + 8), 0xA0); // left Shift
        assert_eq!(map_native_scan_to_vk(54 + 8), 0xA1); // right Shift
        assert_eq!(map_native_scan_to_vk(97 + 8), 0xA3); // right Ctrl
        assert_eq!(map_native_scan_to_vk(100 + 8), 0xA5); // right Alt (AltGr)
        assert_eq!(map_native_scan_to_vk(125 + 8), 0x5B); // Super
        assert_eq!(map_native_scan_to_vk(127 + 8), 0x5D); // menu key
        assert_eq!(map_native_scan_to_vk(99 + 8), 0x2C); // Print Screen
        assert_eq!(map_native_scan_to_vk(96 + 8), 0x0D); // keypad Enter
        assert_eq!(map_native_scan_to_vk(86 + 8), 0xE2); // the key beside left Shift (ISO)
        assert_eq!(map_native_scan_to_vk(15 + 8), 0x09); // Tab
        // Not sent: the volume keys, and no position at all.
        assert_eq!(map_native_scan_to_vk(115 + 8), 0);
        assert_eq!(map_native_scan_to_vk(0), 0);
        assert_eq!(map_native_scan_to_vk(8), 0);
        assert_eq!(map_native_scan_to_vk(5000), 0);
    }

    #[test]
    fn the_game_mouse_packet_is_the_web_client_s() {
        assert_eq!(game_mouse_packet(true), [5, 1]);
        assert_eq!(game_mouse_packet(false), [5, 0]);
    }

    #[test]
    fn touchpad_deltas_add_up_to_notches() {
        let mut rem = (0, 0);
        let mut total = 0;
        for _ in 0..24 {
            total += take_wheel_notches(&mut rem, 0, 15).1; // small touchpad steps
        }
        assert_eq!(total, 3);
        assert_eq!(take_wheel_notches(&mut rem, 0, -120), (0, -1));
        assert_eq!(take_wheel_notches(&mut rem, 240, 0), (2, 0));
    }

    const SHIFT: i32 = 0x0200_0000;
    const KEYPAD: i32 = 0x2000_0000;

    #[test]
    fn symbols_map_to_their_physical_key() {
        assert_eq!(map_qt_key_to_vk(0x2d, 0), 189); // - (was Insert)
        assert_eq!(map_qt_key_to_vk(0x2e, 0), 190); // . (was Delete)
        assert_eq!(map_qt_key_to_vk(0x21, SHIFT), 0x31); // ! is Shift+1 (was PageUp)
        assert_eq!(map_qt_key_to_vk(0x40, SHIFT), 0x32); // @
        assert_eq!(map_qt_key_to_vk(0x7b, SHIFT), 219); // {
        assert_eq!(map_qt_key_to_vk(0x22, SHIFT), 222); // "
        assert_eq!(map_qt_key_to_vk(0x41, 0), 0x41); // A
        assert_eq!(map_qt_key_to_vk(0x01000002, SHIFT), 9); // Backtab
    }

    #[test]
    fn keypad_keys() {
        assert_eq!(map_qt_key_to_vk(0x37, KEYPAD), 103); // Numpad7
        assert_eq!(map_qt_key_to_vk(0x2d, KEYPAD), 109); // Numpad -
        assert_eq!(map_qt_key_to_vk(0x01000005, KEYPAD), 13); // Numpad Enter
        assert_eq!(map_qt_key_to_vk(0x01000010, KEYPAD), 36); // Home (NumLock off)
    }
}
