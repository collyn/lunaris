use sdl2::event::Event;
use sdl2::keyboard::Keycode;
use sdl2::mouse::MouseButton as SdlMouseButton;
use bytes::Bytes;

pub fn map_keycode_to_vk(keycode: Keycode) -> u16 {
    match keycode {
        Keycode::Backspace => 8,
        Keycode::Tab => 9,
        Keycode::Return => 13,
        Keycode::LShift | Keycode::RShift => 16,
        Keycode::LCtrl | Keycode::RCtrl => 17,
        Keycode::LAlt | Keycode::RAlt => 18,
        Keycode::Pause => 19,
        Keycode::CapsLock => 20,
        Keycode::Escape => 27,
        Keycode::Space => 32,
        Keycode::PageUp => 33,
        Keycode::PageDown => 34,
        Keycode::End => 35,
        Keycode::Home => 36,
        Keycode::Left => 37,
        Keycode::Up => 38,
        Keycode::Right => 39,
        Keycode::Down => 40,
        Keycode::PrintScreen => 44,
        Keycode::Insert => 45,
        Keycode::Delete => 46,
        
        Keycode::Num0 => 48,
        Keycode::Num1 => 49,
        Keycode::Num2 => 50,
        Keycode::Num3 => 51,
        Keycode::Num4 => 52,
        Keycode::Num5 => 53,
        Keycode::Num6 => 54,
        Keycode::Num7 => 55,
        Keycode::Num8 => 56,
        Keycode::Num9 => 57,
        
        Keycode::A => 65, Keycode::B => 66, Keycode::C => 67, Keycode::D => 68,
        Keycode::E => 69, Keycode::F => 70, Keycode::G => 71, Keycode::H => 72,
        Keycode::I => 73, Keycode::J => 74, Keycode::K => 75, Keycode::L => 76,
        Keycode::M => 77, Keycode::N => 78, Keycode::O => 79, Keycode::P => 80,
        Keycode::Q => 81, Keycode::R => 82, Keycode::S => 83, Keycode::T => 84,
        Keycode::U => 85, Keycode::V => 86, Keycode::W => 87, Keycode::X => 88,
        Keycode::Y => 89, Keycode::Z => 90,
        
        Keycode::LGui => 91, // MetaLeft
        Keycode::RGui => 92, // MetaRight
        
        Keycode::Kp0 => 96, Keycode::Kp1 => 97, Keycode::Kp2 => 98, Keycode::Kp3 => 99,
        Keycode::Kp4 => 100, Keycode::Kp5 => 101, Keycode::Kp6 => 102, Keycode::Kp7 => 103,
        Keycode::Kp8 => 104, Keycode::Kp9 => 105,
        Keycode::KpMultiply => 106,
        Keycode::KpPlus => 107,
        Keycode::KpMinus => 109,
        Keycode::KpPeriod => 110,
        Keycode::KpDivide => 111,
        
        Keycode::F1 => 112, Keycode::F2 => 113, Keycode::F3 => 114, Keycode::F4 => 115,
        Keycode::F5 => 116, Keycode::F6 => 117, Keycode::F7 => 118, Keycode::F8 => 119,
        Keycode::F9 => 120, Keycode::F10 => 121, Keycode::F11 => 122, Keycode::F12 => 123,
        
        Keycode::NumLockClear => 144,
        Keycode::ScrollLock => 145,
        
        Keycode::Semicolon => 186,
        Keycode::Equals => 187,
        Keycode::Comma => 188,
        Keycode::Minus => 189,
        Keycode::Period => 190,
        Keycode::Slash => 191,
        Keycode::Backquote => 192,
        Keycode::LeftBracket => 219,
        Keycode::Backslash => 220,
        Keycode::RightBracket => 221,
        Keycode::Quote => 222,
        _ => 0,
    }
}

pub struct InputSenders {
    pub keyboard: tokio::sync::mpsc::UnboundedSender<Bytes>,
    pub mouse_abs: tokio::sync::mpsc::UnboundedSender<Bytes>,
    pub mouse_rel: tokio::sync::mpsc::UnboundedSender<Bytes>,
}

pub fn handle_sdl_event(
    event: &Event,
    window_width: i16,
    window_height: i16,
    senders: &InputSenders,
    pointer_locked: bool,
) {
    match event {
        Event::KeyDown { keycode: Some(kc), keymod, .. } => {
            let vk = map_keycode_to_vk(*kc);
            if vk > 0 {
                let mut modifiers = 0u8;
                if keymod.contains(sdl2::keyboard::Mod::LSHIFTMOD) || keymod.contains(sdl2::keyboard::Mod::RSHIFTMOD) {
                    modifiers |= 1;
                }
                if keymod.contains(sdl2::keyboard::Mod::LCTRLMOD) || keymod.contains(sdl2::keyboard::Mod::RCTRLMOD) {
                    modifiers |= 2;
                }
                if keymod.contains(sdl2::keyboard::Mod::LALTMOD) || keymod.contains(sdl2::keyboard::Mod::RALTMOD) {
                    modifiers |= 4;
                }
                if keymod.contains(sdl2::keyboard::Mod::LGUIMOD) || keymod.contains(sdl2::keyboard::Mod::RGUIMOD) {
                    modifiers |= 8;
                }
                
                let mut buf = vec![0u8; 5];
                buf[0] = 0; // Type 0: Key Event
                buf[1] = 1; // 1 = Down
                buf[2] = modifiers;
                buf[3] = (vk >> 8) as u8;
                buf[4] = (vk & 0xFF) as u8;
                let _ = senders.keyboard.send(Bytes::from(buf));
            }
        }
        Event::KeyUp { keycode: Some(kc), keymod, .. } => {
            let vk = map_keycode_to_vk(*kc);
            if vk > 0 {
                let mut modifiers = 0u8;
                if keymod.contains(sdl2::keyboard::Mod::LSHIFTMOD) || keymod.contains(sdl2::keyboard::Mod::RSHIFTMOD) {
                    modifiers |= 1;
                }
                if keymod.contains(sdl2::keyboard::Mod::LCTRLMOD) || keymod.contains(sdl2::keyboard::Mod::RCTRLMOD) {
                    modifiers |= 2;
                }
                if keymod.contains(sdl2::keyboard::Mod::LALTMOD) || keymod.contains(sdl2::keyboard::Mod::RALTMOD) {
                    modifiers |= 4;
                }
                if keymod.contains(sdl2::keyboard::Mod::LGUIMOD) || keymod.contains(sdl2::keyboard::Mod::RGUIMOD) {
                    modifiers |= 8;
                }
                
                let mut buf = vec![0u8; 5];
                buf[0] = 0; // Type 0: Key Event
                buf[1] = 0; // 0 = Up
                buf[2] = modifiers;
                buf[3] = (vk >> 8) as u8;
                buf[4] = (vk & 0xFF) as u8;
                let _ = senders.keyboard.send(Bytes::from(buf));
            }
        }
        Event::MouseMotion { x, y, xrel, yrel, .. } => {
            // Send absolute coordinates (only if pointer is not locked)
            if !pointer_locked {
                let mut buf = vec![0u8; 9];
                buf[0] = 1; // Type 1: Absolute Mouse Position
                buf[1..3].copy_from_slice(&(*x as i16).to_be_bytes());
                buf[3..5].copy_from_slice(&(*y as i16).to_be_bytes());
                buf[5..7].copy_from_slice(&window_width.to_be_bytes());
                buf[7..9].copy_from_slice(&window_height.to_be_bytes());
                let _ = senders.mouse_abs.send(Bytes::from(buf));
            }
            // Send relative coordinates (only if pointer is locked)
            if pointer_locked {
                let mut buf = vec![0u8; 5];
                buf[0] = 0; // Type 0: Relative Mouse Move
                buf[1..3].copy_from_slice(&(*xrel as i16).to_be_bytes());
                buf[3..5].copy_from_slice(&(*yrel as i16).to_be_bytes());
                let _ = senders.mouse_rel.send(Bytes::from(buf));
            }
        }
        Event::MouseButtonDown { mouse_btn, .. } => {
            let button_id = match mouse_btn {
                SdlMouseButton::Left => 1,
                SdlMouseButton::Middle => 2,
                SdlMouseButton::Right => 3,
                SdlMouseButton::X1 => 4,
                SdlMouseButton::X2 => 5,
                _ => 0,
            };
            if button_id > 0 {
                let mut buf = vec![0u8; 3];
                buf[0] = 2; // Type 2: Mouse Button Event
                buf[1] = 1; // 1 = Press
                buf[2] = button_id;
                let _ = senders.mouse_rel.send(Bytes::from(buf));
            }
        }
        Event::MouseButtonUp { mouse_btn, .. } => {
            let button_id = match mouse_btn {
                SdlMouseButton::Left => 1,
                SdlMouseButton::Middle => 2,
                SdlMouseButton::Right => 3,
                SdlMouseButton::X1 => 4,
                SdlMouseButton::X2 => 5,
                _ => 0,
            };
            if button_id > 0 {
                let mut buf = vec![0u8; 3];
                buf[0] = 2; // Type 2: Mouse Button Event
                buf[1] = 0; // 0 = Release
                buf[2] = button_id;
                let _ = senders.mouse_rel.send(Bytes::from(buf));
            }
        }
        Event::MouseWheel { x, y, .. } => {
            let mut buf = vec![0u8; 3];
            buf[0] = 4; // Type 4: Scroll Event
            buf[1] = *x as i8 as u8;
            buf[2] = *y as i8 as u8;
            let _ = senders.mouse_rel.send(Bytes::from(buf));
        }
        _ => {}
    }
}
