//! The host settings every GUI terminal runs under.

use termeric_core::Terminal;
use termeric_core::term::XtermKeyboardType;

/// Host settings applied to every terminal a tab manager owns, and inherited
/// by each terminal it creates later: new tabs, splits, and restored panes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct TerminalPolicy {
    pub(crate) scrollback_lines: usize,
    pub(crate) scrollback_bytes: usize,
    pub(crate) screen_checksum_reporting: bool,
    /// Whether Kitty file transfers may read files on this host.
    pub(crate) kitty_file_transmission: bool,
    pub(crate) xterm_keyboard_type: XtermKeyboardType,
}

impl TerminalPolicy {
    /// Scrollback budgets with every other setting at its default.
    pub(crate) fn with_scrollback(lines: usize, bytes: usize) -> Self {
        Self {
            scrollback_lines: lines,
            scrollback_bytes: bytes,
            screen_checksum_reporting: false,
            kitty_file_transmission: false,
            xterm_keyboard_type: XtermKeyboardType::Normal,
        }
    }

    pub(crate) fn from_config(config: &crate::config::Config) -> Self {
        Self {
            screen_checksum_reporting: config.security.allow_screen_checksum_reports,
            kitty_file_transmission: config.security.allow_kitty_file_transfers,
            xterm_keyboard_type: config.keyboard.xterm_type.terminal_type(),
            ..Self::with_scrollback(
                config.scrollback.lines,
                config.scrollback.max_memory_bytes(),
            )
        }
    }

    /// The settings an adopted terminal already runs under, so the panes
    /// split from it inherit them.
    pub(crate) fn inherited_from(
        terminal: &Terminal,
        scrollback_lines: usize,
        scrollback_bytes: usize,
    ) -> Self {
        Self {
            screen_checksum_reporting: terminal.screen_checksum_reporting(),
            kitty_file_transmission: terminal.kitty_file_transmission(),
            xterm_keyboard_type: terminal.initial_xterm_keyboard_type(),
            ..Self::with_scrollback(scrollback_lines, scrollback_bytes)
        }
    }

    /// The policy for a live-domain relay. Its bytes come from the daemon's
    /// PTY, so a Kitty `t=f` path names a daemon file and must never read a
    /// same-named file on this host.
    pub(crate) fn for_relay(self) -> Self {
        Self {
            kitty_file_transmission: false,
            ..self
        }
    }

    /// Apply every setting to `terminal`. `apply_keyboard_type` also switches
    /// its current keyboard type, which a running application may have
    /// chosen itself; otherwise only the type it resets to changes.
    pub(crate) fn apply(&self, terminal: &mut Terminal, apply_keyboard_type: bool) {
        terminal.set_scrollback_limits(self.scrollback_lines, self.scrollback_bytes);
        terminal.set_screen_checksum_reporting(self.screen_checksum_reporting);
        terminal.set_kitty_file_transmission(self.kitty_file_transmission);
        terminal.configure_xterm_keyboard_type(self.xterm_keyboard_type, apply_keyboard_type);
    }
}
