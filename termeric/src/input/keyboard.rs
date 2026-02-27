//! Keyboard input to PTY byte conversion

use winit::event::{ElementState, KeyEvent};
use winit::keyboard::{Key, ModifiersState, NamedKey};

/// Convert a key event to bytes to send to the PTY
pub fn key_to_bytes(event: &KeyEvent) -> Option<Vec<u8>> {
    key_to_bytes_with_modifiers(event, ModifiersState::empty())
}

/// Convert a key event to bytes, taking modifiers into account
pub fn key_to_bytes_with_modifiers(event: &KeyEvent, modifiers: ModifiersState) -> Option<Vec<u8>> {
    // Only handle key presses
    if event.state != ElementState::Pressed {
        return None;
    }

    let ctrl = modifiers.control_key();
    let _alt = modifiers.alt_key();

    match &event.logical_key {
        Key::Character(c) => {
            // Handle Ctrl+letter combinations - send control characters to PTY
            if ctrl {
                let c_lower = c.to_lowercase();
                let ch = c_lower.chars().next()?;
                if ch.is_ascii_lowercase() {
                    // Ctrl+a = 0x01, Ctrl+b = 0x02, etc.
                    let ctrl_code = (ch as u8) - b'a' + 1;
                    return Some(vec![ctrl_code]);
                }
                // For non-letter Ctrl combos, don't send anything
                // (they're likely handled by check_keybinding)
                return None;
            }
            // Regular character input
            Some(c.as_bytes().to_vec())
        }

        Key::Named(named) => {
            let bytes = match named {
                // Basic control keys
                NamedKey::Enter => b"\r".to_vec(),
                NamedKey::Tab => b"\t".to_vec(),
                NamedKey::Backspace => {
                    if ctrl {
                        b"\x17".to_vec() // Ctrl+W: delete previous word
                    } else {
                        b"\x7f".to_vec()
                    }
                }
                NamedKey::Escape => b"\x1b".to_vec(),
                NamedKey::Space => b" ".to_vec(),

                // Arrow keys (with modifier support: Ctrl=5, Shift=2, Alt=3)
                NamedKey::ArrowUp => if ctrl { b"\x1b[1;5A".to_vec() } else { b"\x1b[A".to_vec() },
                NamedKey::ArrowDown => if ctrl { b"\x1b[1;5B".to_vec() } else { b"\x1b[B".to_vec() },
                NamedKey::ArrowRight => if ctrl { b"\x1b[1;5C".to_vec() } else { b"\x1b[C".to_vec() },
                NamedKey::ArrowLeft => if ctrl { b"\x1b[1;5D".to_vec() } else { b"\x1b[D".to_vec() },

                // Navigation keys
                NamedKey::Home => b"\x1b[H".to_vec(),
                NamedKey::End => b"\x1b[F".to_vec(),
                NamedKey::PageUp => b"\x1b[5~".to_vec(),
                NamedKey::PageDown => b"\x1b[6~".to_vec(),
                NamedKey::Insert => b"\x1b[2~".to_vec(),
                NamedKey::Delete => if ctrl { b"\x1b[3;5~".to_vec() } else { b"\x1b[3~".to_vec() },

                // Function keys
                NamedKey::F1 => b"\x1bOP".to_vec(),
                NamedKey::F2 => b"\x1bOQ".to_vec(),
                NamedKey::F3 => b"\x1bOR".to_vec(),
                NamedKey::F4 => b"\x1bOS".to_vec(),
                NamedKey::F5 => b"\x1b[15~".to_vec(),
                NamedKey::F6 => b"\x1b[17~".to_vec(),
                NamedKey::F7 => b"\x1b[18~".to_vec(),
                NamedKey::F8 => b"\x1b[19~".to_vec(),
                NamedKey::F9 => b"\x1b[20~".to_vec(),
                NamedKey::F10 => b"\x1b[21~".to_vec(),
                NamedKey::F11 => b"\x1b[23~".to_vec(),
                NamedKey::F12 => b"\x1b[24~".to_vec(),

                _ => return None,
            };
            Some(bytes)
        }

        _ => None,
    }
}
