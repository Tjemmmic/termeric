//! Input handling - keyboard and mouse

use winit::event::{ElementState, KeyEvent};
use winit::keyboard::{Key, NamedKey};

/// Convert a key event to bytes to send to the PTY
pub fn key_to_bytes(event: &KeyEvent) -> Option<Vec<u8>> {
    // Only handle key presses
    if event.state != ElementState::Pressed {
        return None;
    }

    match &event.logical_key {
        Key::Character(c) => {
            // Regular character input
            Some(c.as_bytes().to_vec())
        }

        Key::Named(named) => {
            let bytes = match named {
                // Basic control keys
                NamedKey::Enter => b"\r".to_vec(),
                NamedKey::Tab => b"\t".to_vec(),
                NamedKey::Backspace => b"\x7f".to_vec(),
                NamedKey::Escape => b"\x1b".to_vec(),
                NamedKey::Space => b" ".to_vec(),

                // Arrow keys
                NamedKey::ArrowUp => b"\x1b[A".to_vec(),
                NamedKey::ArrowDown => b"\x1b[B".to_vec(),
                NamedKey::ArrowRight => b"\x1b[C".to_vec(),
                NamedKey::ArrowLeft => b"\x1b[D".to_vec(),

                // Navigation keys
                NamedKey::Home => b"\x1b[H".to_vec(),
                NamedKey::End => b"\x1b[F".to_vec(),
                NamedKey::PageUp => b"\x1b[5~".to_vec(),
                NamedKey::PageDown => b"\x1b[6~".to_vec(),
                NamedKey::Insert => b"\x1b[2~".to_vec(),
                NamedKey::Delete => b"\x1b[3~".to_vec(),

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

/// Action that can be triggered by a key binding
#[derive(Clone, Debug)]
pub enum Action {
    /// Copy selected text to clipboard
    Copy,
    /// Paste from clipboard
    Paste,
    /// Scroll up by N lines
    ScrollUp(usize),
    /// Scroll down by N lines
    ScrollDown(usize),
    /// Scroll to top of scrollback
    ScrollToTop,
    /// Scroll to bottom (current output)
    ScrollToBottom,
    /// Increase font size
    IncreaseFontSize,
    /// Decrease font size
    DecreaseFontSize,
    /// Reset font size to default
    ResetFontSize,
    /// No action (pass through to PTY)
    None,
}
