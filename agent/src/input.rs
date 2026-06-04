use crate::buffer::ByteBuffer;
use num_derive::FromPrimitive;
use num_traits::FromPrimitive;
use tracing::warn;

// ── Input protocol types (extracted from moonlight-common) ──────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MouseButtonAction {
    Press,
    Release,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, FromPrimitive)]
pub enum MouseButton {
    Left = 1,
    Middle = 2,
    Right = 3,
    X1 = 4,
    X2 = 5,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyAction {
    Down,
    Up,
}

bitflags::bitflags! {
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct KeyModifiers: i8 {
        const SHIFT = 0x01;
        const CTRL  = 0x02;
        const ALT   = 0x04;
        const META  = 0x08;
    }
}

bitflags::bitflags! {
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct KeyFlags: i8 {
        const NONE = 0;
    }
}


#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TouchEventType {
    Down,
    Move,
    Cancel,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, FromPrimitive)]
pub enum ControllerType {
    Unknown = 0,
    Xbox = 1,
    PlayStation = 2,
    Nintendo = 3,
}

bitflags::bitflags! {
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct ControllerButtons: u32 {
        const A              = 0x0001;
        const B              = 0x0002;
        const X              = 0x0004;
        const Y              = 0x0008;
        const UP             = 0x0010;
        const DOWN           = 0x0020;
        const LEFT           = 0x0040;
        const RIGHT          = 0x0080;
        const LB             = 0x0100;
        const RB             = 0x0200;
        const START          = 0x0400;
        const BACK           = 0x0800;
        const LS_CLICK       = 0x1000;
        const RS_CLICK       = 0x2000;
        const SPECIAL        = 0x4000;
    }
}

bitflags::bitflags! {
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct ControllerCapabilities: u32 {
        const ANALOG_TRIGGERS = 0x01;
        const RUMBLE          = 0x02;
        const TRIGGER_RUMBLE  = 0x04;
        const TOUCHPAD        = 0x08;
        const ACCEL           = 0x10;
        const GYRO            = 0x20;
        const BATTERY         = 0x40;
        const RGB_LED         = 0x80;
    }
}

// ── Transport Channel IDs ───────────────────────────────────────────────

#[allow(dead_code)]
pub struct TransportChannelId;

#[allow(dead_code)]
impl TransportChannelId {
    pub const GENERAL: u8 = 0;
    pub const STATS: u8 = 1;
    pub const HOST_VIDEO: u8 = 2;
    pub const HOST_AUDIO: u8 = 3;
    pub const MOUSE_RELIABLE: u8 = 4;
    pub const MOUSE_ABSOLUTE: u8 = 5;
    pub const MOUSE_RELATIVE: u8 = 6;
    pub const KEYBOARD: u8 = 7;
    pub const TOUCH: u8 = 8;
    pub const CONTROLLERS: u8 = 9;
    pub const CONTROLLER0: u8 = 10;
    pub const CONTROLLER1: u8 = 11;
    pub const CONTROLLER2: u8 = 12;
    pub const CONTROLLER3: u8 = 13;
    pub const CONTROLLER4: u8 = 14;
    pub const CONTROLLER5: u8 = 15;
    pub const CONTROLLER6: u8 = 16;
    pub const CONTROLLER7: u8 = 17;
    pub const CONTROLLER8: u8 = 18;
    pub const CONTROLLER9: u8 = 19;
    pub const CONTROLLER10: u8 = 20;
    pub const CONTROLLER11: u8 = 21;
    pub const CONTROLLER12: u8 = 22;
    pub const CONTROLLER13: u8 = 23;
    pub const CONTROLLER14: u8 = 24;
    pub const CONTROLLER15: u8 = 25;
    pub const RTT: u8 = 26;
    pub const MOUSE_RAW: u8 = 27;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TransportChannel(pub u8);

// ── Inbound Packet ──────────────────────────────────────────────────────

#[derive(Debug)]
#[allow(dead_code)]
pub enum InboundPacket {
    GeneralStop,
    MouseMove {
        delta_x: i16,
        delta_y: i16,
        timestamp: u32,
    },
    MousePosition {
        x: i16,
        y: i16,
        reference_width: i16,
        reference_height: i16,
        timestamp: u32,
    },
    MouseButton {
        action: MouseButtonAction,
        button: MouseButton,
    },
    HighResScroll {
        delta_x: i16,
        delta_y: i16,
    },
    Scroll {
        delta_x: i8,
        delta_y: i8,
    },
    Key {
        action: KeyAction,
        modifiers: KeyModifiers,
        key: u16,
        flags: KeyFlags,
    },
    Text {
        text: String,
    },
    Touch {
        pointer_id: u32,
        x: f32,
        y: f32,
        pressure_or_distance: f32,
        contact_area_major: f32,
        contact_area_minor: f32,
        rotation: Option<u16>,
        event_type: TouchEventType,
    },
    ControllerConnected {
        id: u8,
        ty: ControllerType,
        supported_buttons: ControllerButtons,
        capabilities: ControllerCapabilities,
    },
    ControllerDisconnected {
        id: u8,
    },
    ControllerState {
        id: u8,
        buttons: ControllerButtons,
        left_trigger: u8,
        right_trigger: u8,
        left_stick_x: i16,
        left_stick_y: i16,
        right_stick_x: i16,
        right_stick_y: i16,
    },
    RequestVideoIdr,
    /// Dynamic bitrate change from web client.
    SetBitrate { kbps: u32 },
    /// Dynamic FPS change from web client.
    SetFps { fps: u32 },
}

#[allow(dead_code)]
impl InboundPacket {
    pub const CONTROLLER_CHANNELS: [u8; 16] = [
        TransportChannelId::CONTROLLER0,
        TransportChannelId::CONTROLLER1,
        TransportChannelId::CONTROLLER2,
        TransportChannelId::CONTROLLER3,
        TransportChannelId::CONTROLLER4,
        TransportChannelId::CONTROLLER5,
        TransportChannelId::CONTROLLER6,
        TransportChannelId::CONTROLLER7,
        TransportChannelId::CONTROLLER8,
        TransportChannelId::CONTROLLER9,
        TransportChannelId::CONTROLLER10,
        TransportChannelId::CONTROLLER11,
        TransportChannelId::CONTROLLER12,
        TransportChannelId::CONTROLLER13,
        TransportChannelId::CONTROLLER14,
        TransportChannelId::CONTROLLER15,
    ];

    pub fn deserialize(channel: TransportChannel, bytes: &[u8]) -> Option<Self> {
        let mut buffer = ByteBuffer::new(bytes);

        match channel {
            TransportChannel(TransportChannelId::GENERAL) => {
                if buffer.remaining() < 2 {
                    return None;
                }
                let len = buffer.get_u16();
                let text = match buffer.get_utf8_raw(len as usize) {
                    Ok(t) => t,
                    Err(err) => {
                        warn!("Failed to read general message: {}", err);
                        return None;
                    }
                };
                if text.contains("\"stop\"") || text.contains("Stop") {
                    Some(Self::GeneralStop)
                } else if let Some(value) = Self::parse_json_command(&text, "set_bitrate") {
                    Some(Self::SetBitrate { kbps: value })
                } else if let Some(value) = Self::parse_json_command(&text, "set_fps") {
                    Some(Self::SetFps { fps: value })
                } else {
                    None
                }
            }
            TransportChannel(TransportChannelId::HOST_VIDEO) => {
                if buffer.remaining() < 1 {
                    return None;
                }
                let ty = buffer.get_u8();
                if ty == 0 {
                    Some(InboundPacket::RequestVideoIdr)
                } else {
                    None
                }
            }
            TransportChannel(
                TransportChannelId::MOUSE_ABSOLUTE
                | TransportChannelId::MOUSE_RELIABLE
                | TransportChannelId::MOUSE_RELATIVE
                | TransportChannelId::MOUSE_RAW,
            ) => {
                if buffer.remaining() < 1 {
                    return None;
                }
                let ty = buffer.get_u8();
                if ty == 0 {
                    if buffer.remaining() < 4 {
                        return None;
                    }
                    let delta_x = buffer.get_i16();
                    let delta_y = buffer.get_i16();
                    let timestamp = if buffer.remaining() >= 4 {
                        buffer.get_u32()
                    } else {
                        0
                    };
                    Some(InboundPacket::MouseMove {
                        delta_x,
                        delta_y,
                        timestamp,
                    })
                } else if ty == 1 {
                    if buffer.remaining() < 8 {
                        return None;
                    }
                    let x = buffer.get_i16();
                    let y = buffer.get_i16();
                    let reference_width = buffer.get_i16();
                    let reference_height = buffer.get_i16();
                    let timestamp = if buffer.remaining() >= 4 {
                        buffer.get_u32()
                    } else {
                        0
                    };
                    Some(InboundPacket::MousePosition {
                        x,
                        y,
                        reference_width,
                        reference_height,
                        timestamp,
                    })
                } else if ty == 2 {
                    if buffer.remaining() < 2 {
                        return None;
                    }
                    let action = if buffer.get_bool() {
                        MouseButtonAction::Press
                    } else {
                        MouseButtonAction::Release
                    };
                    let button = MouseButton::from_u8(buffer.get_u8())?;
                    Some(InboundPacket::MouseButton { action, button })
                } else if ty == 3 {
                    if buffer.remaining() < 4 {
                        return None;
                    }
                    let delta_x = buffer.get_i16();
                    let delta_y = buffer.get_i16();
                    Some(InboundPacket::HighResScroll { delta_x, delta_y })
                } else if ty == 4 {
                    if buffer.remaining() < 2 {
                        return None;
                    }
                    let delta_x = buffer.get_i8();
                    let delta_y = buffer.get_i8();
                    Some(InboundPacket::Scroll { delta_x, delta_y })
                } else {
                    None
                }
            }
            TransportChannel(TransportChannelId::KEYBOARD) => {
                if buffer.remaining() < 1 {
                    return None;
                }
                let ty = buffer.get_u8();
                if ty == 0 {
                    if buffer.remaining() < 4 {
                        return None;
                    }
                    let action = if buffer.get_bool() {
                        KeyAction::Down
                    } else {
                        KeyAction::Up
                    };
                    let modifiers = KeyModifiers::from_bits(buffer.get_u8() as i8)?;
                    let key = buffer.get_u16();
                    Some(InboundPacket::Key {
                        action,
                        modifiers,
                        key,
                        flags: KeyFlags::empty(),
                    })
                } else if ty == 1 {
                    if buffer.remaining() < 1 {
                        return None;
                    }
                    let len = buffer.get_u8();
                    let text = buffer.get_utf8_raw(len as usize).ok()?.to_owned();
                    Some(InboundPacket::Text { text })
                } else {
                    None
                }
            }
            TransportChannel(TransportChannelId::TOUCH) => {
                if buffer.remaining() < 27 {
                    return None;
                }
                let event_type = match buffer.get_u8() {
                    0 => TouchEventType::Down,
                    1 => TouchEventType::Move,
                    2 => TouchEventType::Cancel,
                    _ => return None,
                };
                let pointer_id = buffer.get_u32();
                let x = buffer.get_f32();
                let y = buffer.get_f32();
                let pressure_or_distance = buffer.get_f32();
                let contact_area_major = buffer.get_f32();
                let contact_area_minor = buffer.get_f32();
                let rotation = buffer.get_u16();

                Some(InboundPacket::Touch {
                    pointer_id,
                    x,
                    y,
                    pressure_or_distance,
                    contact_area_major,
                    contact_area_minor,
                    rotation: Some(rotation),
                    event_type,
                })
            }
            _ => None,
        }
    }

    /// Lightweight JSON command parser for dynamic settings.
    /// Parses commands like: {"type":"set_bitrate","value":8000}
    /// Returns the value as u32 if the command matches.
    fn parse_json_command(text: &str, command_type: &str) -> Option<u32> {
        // Check if this JSON contains the right command type
        let type_pattern = format!("\"type\":\"{}\"", command_type);
        let type_pattern_spaced = format!("\"type\": \"{}\"", command_type);
        if !text.contains(&type_pattern) && !text.contains(&type_pattern_spaced) {
            return None;
        }

        // Extract the value field
        // Look for "value": followed by a number
        let value_str = if let Some(idx) = text.find("\"value\":") {
            &text[idx + 8..]
        } else if let Some(idx) = text.find("\"value\": ") {
            &text[idx + 9..]
        } else {
            return None;
        };

        // Parse the numeric value (trim whitespace, stop at non-digit)
        let value_str = value_str.trim();
        let num_str: String = value_str.chars().take_while(|c| c.is_ascii_digit()).collect();

        num_str.parse::<u32>().ok()
    }
}
