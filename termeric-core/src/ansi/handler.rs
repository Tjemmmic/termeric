//! VTE escape sequence handler

use vte::{Params, Perform};
use crate::grid::{CellAttrs, Color};
use crate::term::{Terminal, TerminalMode};
use super::color::{ansi_color, ansi_bright_color, parse_extended_color};

/// Handler for VTE callbacks that updates terminal state
pub struct Handler<'a> {
    terminal: &'a mut Terminal,
}

impl<'a> Handler<'a> {
    pub fn new(terminal: &'a mut Terminal) -> Self {
        Self { terminal }
    }
}

impl Perform for Handler<'_> {
    /// Print a character to the terminal
    fn print(&mut self, c: char) {
        self.terminal.write_char(c);
    }

    /// Execute a C0/C1 control character
    fn execute(&mut self, byte: u8) {
        match byte {
            0x00 => {} // NUL - ignore
            0x07 => {
                // BEL - bell
                log::debug!("Bell");
            }
            0x08 => {
                // BS - backspace
                self.terminal.cursor_backward(1);
            }
            0x09 => {
                // HT - horizontal tab
                self.terminal.horizontal_tab();
            }
            0x0A | 0x0B | 0x0C => {
                // LF, VT, FF - line feed
                self.terminal.linefeed();
            }
            0x0D => {
                // CR - carriage return
                self.terminal.carriage_return();
            }
            0x0E => {
                // SO - shift out (select G1 charset)
                log::trace!("Shift out");
            }
            0x0F => {
                // SI - shift in (select G0 charset)
                log::trace!("Shift in");
            }
            _ => {
                log::trace!("Unknown C0 control: 0x{:02X}", byte);
            }
        }
    }

    /// Handle a CSI sequence
    fn csi_dispatch(&mut self, params: &Params, intermediates: &[u8], _ignore: bool, action: char) {
        let params: Vec<u16> = params.iter()
            .flat_map(|p| p.iter().copied())
            .collect();

        let private = intermediates.first() == Some(&b'?');

        macro_rules! param {
            ($idx:expr, $default:expr) => {
                params.get($idx).copied().filter(|&v| v != 0).unwrap_or($default)
            };
        }

        match action {
            // Cursor movement
            'A' => {
                // CUU - Cursor Up
                let n = param!(0, 1) as usize;
                self.terminal.cursor_up(n);
            }
            'B' => {
                // CUD - Cursor Down
                let n = param!(0, 1) as usize;
                self.terminal.cursor_down(n);
            }
            'C' => {
                // CUF - Cursor Forward
                let n = param!(0, 1) as usize;
                self.terminal.cursor_forward(n);
            }
            'D' => {
                // CUB - Cursor Back
                let n = param!(0, 1) as usize;
                self.terminal.cursor_backward(n);
            }
            'E' => {
                // CNL - Cursor Next Line
                let n = param!(0, 1) as usize;
                self.terminal.cursor_down(n);
                self.terminal.carriage_return();
            }
            'F' => {
                // CPL - Cursor Previous Line
                let n = param!(0, 1) as usize;
                self.terminal.cursor_up(n);
                self.terminal.carriage_return();
            }
            'G' => {
                // CHA - Cursor Horizontal Absolute
                let col = param!(0, 1) as usize;
                let row = self.terminal.grid().cursor().row + 1;
                self.terminal.set_cursor_position(row, col);
            }
            'H' | 'f' => {
                // CUP/HVP - Cursor Position
                let row = param!(0, 1) as usize;
                let col = param!(1, 1) as usize;
                self.terminal.set_cursor_position(row, col);
            }
            'd' => {
                // VPA - Vertical Position Absolute
                let row = param!(0, 1) as usize;
                let col = self.terminal.grid().cursor().col + 1;
                self.terminal.set_cursor_position(row, col);
            }

            // Erasing
            'J' => {
                // ED - Erase in Display
                let mode = param!(0, 0);
                self.terminal.erase_in_display(mode);
            }
            'K' => {
                // EL - Erase in Line
                let mode = param!(0, 0);
                self.terminal.erase_in_line(mode);
            }

            // Scrolling
            'S' => {
                // SU - Scroll Up
                let n = param!(0, 1) as usize;
                self.terminal.scroll_up(n);
            }
            'T' => {
                // SD - Scroll Down
                let n = param!(0, 1) as usize;
                self.terminal.scroll_down(n);
            }

            // Modes
            'h' => {
                // SM - Set Mode
                self.handle_mode(&params, private, true);
            }
            'l' => {
                // RM - Reset Mode
                self.handle_mode(&params, private, false);
            }

            // SGR - Select Graphic Rendition
            'm' => {
                self.handle_sgr(&params);
            }

            // Cursor save/restore
            's' => {
                // SCOSC - Save Cursor Position
                self.terminal.save_cursor();
            }
            'u' => {
                // SCORC - Restore Cursor Position
                self.terminal.restore_cursor();
            }

            // Device status report
            'n' => {
                if private {
                    // DECRQM - Request Mode
                    log::trace!("Request mode: {:?}", params);
                } else {
                    match param!(0, 0) {
                        5 => {
                            // DSR - Device Status Report
                            log::trace!("Device status report requested");
                            // Would respond with ESC [ 0 n (OK)
                        }
                        6 => {
                            // CPR - Cursor Position Report
                            log::trace!("Cursor position report requested");
                            // Would respond with ESC [ row ; col R
                        }
                        _ => {}
                    }
                }
            }

            // Primary/Secondary device attributes
            'c' => {
                log::trace!("Device attributes requested");
                // Would respond with device identification
            }

            // Insert/Delete
            '@' => {
                // ICH - Insert Characters
                let n = param!(0, 1) as usize;
                log::trace!("Insert {} characters", n);
            }
            'P' => {
                // DCH - Delete Characters
                let n = param!(0, 1) as usize;
                log::trace!("Delete {} characters", n);
            }
            'L' => {
                // IL - Insert Lines
                let n = param!(0, 1) as usize;
                log::trace!("Insert {} lines", n);
            }
            'M' => {
                // DL - Delete Lines
                let n = param!(0, 1) as usize;
                log::trace!("Delete {} lines", n);
            }

            // Scroll region
            'r' => {
                // DECSTBM - Set Top and Bottom Margins
                let top = param!(0, 1) as usize;
                let bottom = param!(1, self.terminal.rows() as u16) as usize;
                log::trace!("Set scroll region: {} to {}", top, bottom);
            }

            _ => {
                log::trace!(
                    "Unhandled CSI: params={:?}, intermediates={:?}, action='{}'",
                    params, intermediates, action
                );
            }
        }
    }

    /// Handle an escape sequence
    fn esc_dispatch(&mut self, intermediates: &[u8], _ignore: bool, byte: u8) {
        match (byte, intermediates) {
            // RIS - Full Reset
            (b'c', []) => {
                self.terminal.reset();
            }

            // DECSC - Save Cursor
            (b'7', []) => {
                self.terminal.save_cursor();
            }

            // DECRC - Restore Cursor
            (b'8', []) => {
                self.terminal.restore_cursor();
            }

            // IND - Index (move down, possibly scroll)
            (b'D', []) => {
                self.terminal.linefeed();
            }

            // NEL - Next Line
            (b'E', []) => {
                self.terminal.carriage_return();
                self.terminal.linefeed();
            }

            // HTS - Horizontal Tab Set
            (b'H', []) => {
                log::trace!("Set tab stop");
            }

            // RI - Reverse Index (move up, possibly scroll)
            (b'M', []) => {
                self.terminal.cursor_up(1);
            }

            // DECKPAM - Application Keypad Mode
            (b'=', []) => {
                self.terminal.set_mode(TerminalMode::KEYPAD_APPLICATION, true);
            }

            // DECKPNM - Normal Keypad Mode
            (b'>', []) => {
                self.terminal.set_mode(TerminalMode::KEYPAD_APPLICATION, false);
            }

            // Character set designation
            (b'B', [b'(']) => {
                log::trace!("Set G0 to US ASCII");
            }
            (b'0', [b'(']) => {
                log::trace!("Set G0 to DEC Special Graphics");
            }

            _ => {
                log::trace!(
                    "Unhandled ESC: intermediates={:?}, byte=0x{:02X} ('{}')",
                    intermediates, byte, byte as char
                );
            }
        }
    }

    /// Handle an OSC (Operating System Command) sequence
    fn osc_dispatch(&mut self, params: &[&[u8]], _bell_terminated: bool) {
        if params.is_empty() {
            return;
        }

        let command = std::str::from_utf8(params[0])
            .ok()
            .and_then(|s| s.parse::<u32>().ok())
            .unwrap_or(0);

        match command {
            // Set window title
            0 | 2 => {
                if params.len() > 1 {
                    if let Ok(title) = std::str::from_utf8(params[1]) {
                        log::debug!("Set window title: {}", title);
                    }
                }
            }

            // Set icon name
            1 => {
                if params.len() > 1 {
                    if let Ok(name) = std::str::from_utf8(params[1]) {
                        log::trace!("Set icon name: {}", name);
                    }
                }
            }

            // Set color palette
            4 => {
                log::trace!("Set color palette: {:?}", params);
            }

            // Hyperlink
            8 => {
                log::trace!("Hyperlink: {:?}", params);
            }

            // Clipboard
            52 => {
                log::trace!("Clipboard operation: {:?}", params);
            }

            _ => {
                log::trace!("Unhandled OSC {}: {:?}", command, params);
            }
        }
    }

    fn hook(&mut self, _params: &Params, _intermediates: &[u8], _ignore: bool, _action: char) {
        // DCS hook - start of device control string
    }

    fn put(&mut self, _byte: u8) {
        // DCS data
    }

    fn unhook(&mut self) {
        // DCS unhook - end of device control string
    }
}

impl Handler<'_> {
    /// Handle SGR (Select Graphic Rendition) parameters
    fn handle_sgr(&mut self, params: &[u16]) {
        if params.is_empty() {
            self.terminal.reset_attrs();
            return;
        }

        let mut iter = params.iter().peekable();

        while let Some(&param) = iter.next() {
            match param {
                // Reset
                0 => {
                    self.terminal.reset_attrs();
                }

                // Attributes
                1 => self.add_attr(CellAttrs::BOLD),
                2 => self.add_attr(CellAttrs::DIM),
                3 => self.add_attr(CellAttrs::ITALIC),
                4 => self.add_attr(CellAttrs::UNDERLINE),
                5 => self.add_attr(CellAttrs::BLINK),
                7 => self.add_attr(CellAttrs::INVERSE),
                8 => self.add_attr(CellAttrs::HIDDEN),
                9 => self.add_attr(CellAttrs::STRIKE),

                // Reset attributes
                22 => self.remove_attr(CellAttrs::BOLD | CellAttrs::DIM),
                23 => self.remove_attr(CellAttrs::ITALIC),
                24 => self.remove_attr(CellAttrs::UNDERLINE),
                25 => self.remove_attr(CellAttrs::BLINK),
                27 => self.remove_attr(CellAttrs::INVERSE),
                28 => self.remove_attr(CellAttrs::HIDDEN),
                29 => self.remove_attr(CellAttrs::STRIKE),

                // Normal foreground colors (30-37)
                30..=37 => {
                    self.terminal.set_fg(ansi_color(param - 30));
                }

                // Extended foreground color
                38 => {
                    if let Some(color) = parse_extended_color(&mut iter) {
                        self.terminal.set_fg(color);
                    }
                }

                // Default foreground
                39 => {
                    self.terminal.set_fg(Color::default_fg());
                }

                // Normal background colors (40-47)
                40..=47 => {
                    self.terminal.set_bg(ansi_color(param - 40));
                }

                // Extended background color
                48 => {
                    if let Some(color) = parse_extended_color(&mut iter) {
                        self.terminal.set_bg(color);
                    }
                }

                // Default background
                49 => {
                    self.terminal.set_bg(Color::default_bg());
                }

                // Bright foreground colors (90-97)
                90..=97 => {
                    self.terminal.set_fg(ansi_bright_color(param - 90));
                }

                // Bright background colors (100-107)
                100..=107 => {
                    self.terminal.set_bg(ansi_bright_color(param - 100));
                }

                _ => {
                    log::trace!("Unknown SGR parameter: {}", param);
                }
            }
        }
    }

    /// Handle mode setting/resetting
    fn handle_mode(&mut self, params: &[u16], private: bool, enable: bool) {
        for &mode in params {
            if private {
                // DEC private modes
                match mode {
                    1 => {
                        // DECCKM - Application cursor keys
                        self.terminal.set_mode(TerminalMode::CURSOR_KEYS_APPLICATION, enable);
                    }
                    7 => {
                        // DECAWM - Autowrap
                        self.terminal.set_mode(TerminalMode::AUTOWRAP, enable);
                    }
                    12 => {
                        // ATT610 - Cursor blinking
                        self.terminal.set_mode(TerminalMode::CURSOR_BLINKING, enable);
                    }
                    25 => {
                        // DECTCEM - Cursor visible
                        self.terminal.set_cursor_visible(enable);
                    }
                    1000 => {
                        // X10 mouse tracking
                        self.terminal.set_mode(TerminalMode::MOUSE_X10, enable);
                    }
                    1002 => {
                        // Button event mouse tracking
                        self.terminal.set_mode(TerminalMode::MOUSE_BUTTON_EVENT, enable);
                    }
                    1003 => {
                        // Any event mouse tracking
                        self.terminal.set_mode(TerminalMode::MOUSE_ANY_EVENT, enable);
                    }
                    1004 => {
                        // Focus events
                        self.terminal.set_mode(TerminalMode::FOCUS_EVENTS, enable);
                    }
                    1005 => {
                        // UTF-8 mouse encoding
                        self.terminal.set_mode(TerminalMode::MOUSE_UTF8, enable);
                    }
                    1006 => {
                        // SGR mouse encoding
                        self.terminal.set_mode(TerminalMode::MOUSE_SGR, enable);
                    }
                    1049 => {
                        // Alternate screen buffer
                        if enable {
                            self.terminal.enter_alt_screen();
                        } else {
                            self.terminal.exit_alt_screen();
                        }
                    }
                    2004 => {
                        // Bracketed paste mode
                        self.terminal.set_mode(TerminalMode::BRACKETED_PASTE, enable);
                    }
                    _ => {
                        log::trace!("Unknown private mode {}: {}", mode, enable);
                    }
                }
            } else {
                // ANSI modes
                match mode {
                    4 => {
                        // IRM - Insert mode
                        self.terminal.set_mode(TerminalMode::INSERT, enable);
                    }
                    20 => {
                        // LNM - Linefeed/Newline mode
                        self.terminal.set_mode(TerminalMode::LINEFEED_NEWLINE, enable);
                    }
                    _ => {
                        log::trace!("Unknown ANSI mode {}: {}", mode, enable);
                    }
                }
            }
        }
    }

    fn add_attr(&mut self, attr: CellAttrs) {
        // This is a simplified implementation - in reality we'd need to
        // get the current attrs, modify them, and set them back
        // For now, we just log that we would add this attribute
        log::trace!("Add attribute: {:?}", attr);
    }

    fn remove_attr(&mut self, attr: CellAttrs) {
        log::trace!("Remove attribute: {:?}", attr);
    }
}
