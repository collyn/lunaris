use bytes::Bytes;

pub fn map_qt_key_to_vk(key: i32) -> u16 {
    // Alphanumeric keys and symbols in the ASCII range
    if key >= 0x20 && key <= 0x7e {
        return key as u16;
    }
    
    match key {
        0x01000003 => 8,   // Backspace
        0x01000001 => 9,   // Tab
        0x01000004 | 0x01000005 => 13,  // Return / Enter
        0x01000020 => 16,  // Shift
        0x01000021 => 17,  // Control
        0x01000023 => 18,  // Alt
        0x01000008 => 19,  // Pause
        0x01000024 => 20,  // CapsLock
        0x01000000 => 27,  // Escape
        0x01000016 => 33,  // PageUp
        0x01000017 => 34,  // PageDown
        0x01000011 => 35,  // End
        0x01000010 => 36,  // Home
        0x01000012 => 37,  // Left
        0x01000013 => 38,  // Up
        0x01000014 => 39,  // Right
        0x01000015 => 40,  // Down
        0x01000009 => 44,  // PrintScreen
        0x01000006 => 45,  // Insert
        0x01000007 => 46,  // Delete
        0x01000022 => 91,  // Meta (Windows / Command key)
        
        // F1 to F12
        k if k >= 0x01000030 && k <= 0x0100003b => {
            (112 + (k - 0x01000030)) as u16
        }
        _ => 0,
    }
}

pub struct InputSenders {
    pub keyboard: tokio::sync::mpsc::UnboundedSender<Bytes>,
    pub mouse_abs: tokio::sync::mpsc::UnboundedSender<Bytes>,
    pub mouse_rel: tokio::sync::mpsc::UnboundedSender<Bytes>,
}

pub fn handle_key_event(
    key: i32,
    modifiers_mask: i32,
    is_down: bool,
    senders: &InputSenders,
) {
    let vk = map_qt_key_to_vk(key);
    if vk > 0 {
        let mut modifiers = 0u8;
        // Qt modifiers mapping:
        // ShiftModifier = 0x02000000
        // ControlModifier = 0x04000000
        // AltModifier = 0x08000000
        // MetaModifier = 0x10000000
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
        
        let mut buf = vec![0u8; 5];
        buf[0] = 0; // Type 0: Key Event
        buf[1] = if is_down { 1 } else { 0 };
        buf[2] = modifiers;
        buf[3] = (vk >> 8) as u8;
        buf[4] = (vk & 0xFF) as u8;
        let _ = senders.keyboard.send(Bytes::from(buf));
    }
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
    if !pointer_locked {
        let mut buf = vec![0u8; 9];
        buf[0] = 1; // Type 1: Absolute Mouse Position
        buf[1..3].copy_from_slice(&(x as i16).to_be_bytes());
        buf[3..5].copy_from_slice(&(y as i16).to_be_bytes());
        buf[5..7].copy_from_slice(&(width as i16).to_be_bytes());
        buf[7..9].copy_from_slice(&(height as i16).to_be_bytes());
        let _ = senders.mouse_abs.send(Bytes::from(buf));
    } else {
        let mut buf = vec![0u8; 5];
        buf[0] = 0; // Type 0: Relative Mouse Move
        buf[1..3].copy_from_slice(&(rx as i16).to_be_bytes());
        buf[3..5].copy_from_slice(&(ry as i16).to_be_bytes());
        let _ = senders.mouse_rel.send(Bytes::from(buf));
    }
}

pub fn handle_mouse_click(
    button: i32,
    is_down: bool,
    senders: &InputSenders,
) {
    if button > 0 {
        let mut buf = vec![0u8; 3];
        buf[0] = 2; // Type 2: Mouse Button Event
        buf[1] = if is_down { 1 } else { 0 };
        buf[2] = button as u8;
        let _ = senders.mouse_rel.send(Bytes::from(buf));
    }
}

pub fn handle_mouse_wheel(
    delta: i32,
    senders: &InputSenders,
) {
    let mut buf = vec![0u8; 3];
    buf[0] = 4; // Type 4: Scroll Event
    buf[1] = 0;
    buf[2] = ((delta / 120) as i8) as u8; // Normal delta is 120 per click
    let _ = senders.mouse_rel.send(Bytes::from(buf));
}
