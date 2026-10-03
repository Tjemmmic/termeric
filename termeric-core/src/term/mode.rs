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

        /// Convert wheel events to cursor keys while the alternate screen is active
        const ALTERNATE_SCROLL = 0b1000_0000_0000_0000_0000;

        /// Left/right margin mode (DECLRMM) - CSI s selects horizontal margins
        const LEFT_RIGHT_MARGIN = 0b0001_0000_0000_0000_0000_0000;
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
}
