//! Terminal mode flags

use bitflags::bitflags;

bitflags! {
    /// Terminal mode flags
    #[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
    pub struct TerminalMode: u32 {
        // Standard ANSI modes

        /// Insert mode (IRM) - characters are inserted, not overwritten
        const INSERT = 0b0000_0000_0000_0001;

        // DEC private modes

        /// Application cursor keys (DECCKM)
        const CURSOR_KEYS_APPLICATION = 0b0000_0000_0000_0010;

        /// Reverse video mode (DECSCNM)
        const REVERSE_VIDEO = 0b0000_0000_0000_0100;

        /// Origin mode (DECOM) - cursor positioning relative to scroll region
        const ORIGIN = 0b0000_0000_0000_1000;

        /// Autowrap mode (DECAWM) - wrap at end of line
        const AUTOWRAP = 0b0000_0000_0001_0000;

        /// Cursor blinking (ATT610)
        const CURSOR_BLINKING = 0b0000_0000_0010_0000;

        /// Cursor visible (DECTCEM)
        const CURSOR_VISIBLE = 0b0000_0000_0100_0000;

        // Mouse modes

        /// X10 mouse reporting (button press only)
        const MOUSE_X10 = 0b0000_0000_1000_0000;

        /// Normal mouse tracking (button press/release)
        const MOUSE_TRACKING = 0b0000_0001_0000_0000;

        /// Button event mouse tracking (press/release + motion while pressed)
        const MOUSE_BUTTON_EVENT = 0b0000_0010_0000_0000;

        /// Any event mouse tracking (all motion)
        const MOUSE_ANY_EVENT = 0b0000_0100_0000_0000;

        /// UTF-8 mouse encoding
        const MOUSE_UTF8 = 0b0000_1000_0000_0000;

        /// SGR mouse encoding
        const MOUSE_SGR = 0b0001_0000_0000_0000;

        // Focus events

        /// Send focus in/out events
        const FOCUS_EVENTS = 0b0010_0000_0000_0000;

        // Alternate screen

        /// Alternate screen buffer active
        const ALT_SCREEN = 0b0100_0000_0000_0000;

        // Bracketed paste

        /// Bracketed paste mode
        const BRACKETED_PASTE = 0b1000_0000_0000_0000;

        // Application keypad

        /// Application keypad mode (DECKPAM)
        const KEYPAD_APPLICATION = 0b0001_0000_0000_0000_0000;

        /// Linefeed/Newline mode - LF also does CR
        const LINEFEED_NEWLINE = 0b0010_0000_0000_0000_0000;

        /// Synchronized output mode (Mode 2026) - buffer output until disabled
        const SYNCHRONIZED_OUTPUT = 0b0100_0000_0000_0000_0000;
    }
}

impl TerminalMode {
    /// Check if any mouse mode is active
    pub fn has_mouse_mode(&self) -> bool {
        self.intersects(
            Self::MOUSE_X10
                | Self::MOUSE_TRACKING
                | Self::MOUSE_BUTTON_EVENT
                | Self::MOUSE_ANY_EVENT,
        )
    }

    /// Get the active mouse mode for encoding
    pub fn mouse_mode(&self) -> MouseMode {
        if self.contains(Self::MOUSE_ANY_EVENT) {
            MouseMode::AnyEvent
        } else if self.contains(Self::MOUSE_BUTTON_EVENT) {
            MouseMode::ButtonEvent
        } else if self.contains(Self::MOUSE_TRACKING) {
            MouseMode::Normal
        } else if self.contains(Self::MOUSE_X10) {
            MouseMode::X10
        } else {
            MouseMode::None
        }
    }

    /// Get the mouse encoding format
    pub fn mouse_encoding(&self) -> MouseEncoding {
        if self.contains(Self::MOUSE_SGR) {
            MouseEncoding::Sgr
        } else if self.contains(Self::MOUSE_UTF8) {
            MouseEncoding::Utf8
        } else {
            MouseEncoding::Default
        }
    }
}

/// Mouse tracking mode
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum MouseMode {
    /// No mouse tracking
    #[default]
    None,
    /// X10 mode - button press only
    X10,
    /// Normal mode - button press and release
    Normal,
    /// Button event mode - press/release + motion while pressed
    ButtonEvent,
    /// Any event mode - all motion reported
    AnyEvent,
}

/// Mouse encoding format
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum MouseEncoding {
    /// Default X10 encoding (limited to 223 columns/rows)
    #[default]
    Default,
    /// UTF-8 encoding
    Utf8,
    /// SGR encoding (unlimited coordinates)
    Sgr,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_mode() {
        let mode = TerminalMode::default();
        assert!(!mode.contains(TerminalMode::INSERT));
        assert!(!mode.has_mouse_mode());
    }

    #[test]
    fn test_autowrap() {
        let mut mode = TerminalMode::default();
        mode.insert(TerminalMode::AUTOWRAP);
        assert!(mode.contains(TerminalMode::AUTOWRAP));
    }

    #[test]
    fn test_mouse_mode() {
        let mut mode = TerminalMode::default();
        assert_eq!(mode.mouse_mode(), MouseMode::None);

        mode.insert(TerminalMode::MOUSE_TRACKING);
        assert_eq!(mode.mouse_mode(), MouseMode::Normal);

        mode.insert(TerminalMode::MOUSE_ANY_EVENT);
        assert_eq!(mode.mouse_mode(), MouseMode::AnyEvent);
    }

    #[test]
    fn test_mouse_encoding() {
        let mut mode = TerminalMode::default();
        assert_eq!(mode.mouse_encoding(), MouseEncoding::Default);

        mode.insert(TerminalMode::MOUSE_SGR);
        assert_eq!(mode.mouse_encoding(), MouseEncoding::Sgr);
    }
}
