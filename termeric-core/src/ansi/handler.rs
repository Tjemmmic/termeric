//! VTE escape sequence handler

use super::color::{ansi_bright_color, ansi_color};
use crate::event::{NotificationRequest, TerminalEvent};
use crate::grid::{CellAttrs, Color, ColorSpec};
use crate::term::{Terminal, TerminalMode};
use vte::{Params, Perform};

/// Handler for VTE callbacks that updates terminal state
pub struct Handler<'a> {
    terminal: &'a mut Terminal,
    /// Accumulator for APC/DCS data
    apc_data: Vec<u8>,
    /// Whether we're in an APC sequence
    in_apc: bool,
    /// APC action character (for Kitty graphics, this is 'G')
    apc_action: char,
}

impl<'a> Handler<'a> {
    pub fn new(terminal: &'a mut Terminal) -> Self {
        Self {
            terminal,
            apc_data: Vec::new(),
            in_apc: false,
            apc_action: '\0',
        }
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
                self.terminal.push_event(TerminalEvent::Bell);
            }
            0x08 => {
                // BS - backspace
                self.terminal.cursor_backward(1);
            }
            0x09 => {
                // HT - horizontal tab
                self.terminal.horizontal_tab();
            }
            0x0A..=0x0C => {
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
    fn csi_dispatch(
        &mut self,
        raw_params: &Params,
        intermediates: &[u8],
        _ignore: bool,
        action: char,
    ) {
        let params: Vec<u16> = raw_params.iter().flat_map(|p| p.iter().copied()).collect();

        let private = intermediates.first() == Some(&b'?');

        macro_rules! param {
            ($idx:expr, $default:expr) => {
                params
                    .get($idx)
                    .copied()
                    .filter(|&v| v != 0)
                    .unwrap_or($default)
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
                // CHA - Cursor Horizontal Absolute.
                // Only the column is addressed; the row stays where it is.
                // Re-routing through `set_cursor_position` would double-
                // offset the row under ORIGIN mode (scroll_top would be
                // added again to an already-absolute row), so set the
                // grid cursor directly.
                let col = param!(0, 1)
                    .saturating_sub(1)
                    .min(self.terminal.cols() as u16 - 1) as usize;
                let row = self.terminal.grid().cursor().row;
                self.terminal.grid_mut().set_cursor(row, col);
            }
            'H' | 'f' => {
                // CUP/HVP - Cursor Position
                let row = param!(0, 1) as usize;
                let col = param!(1, 1) as usize;
                self.terminal.set_cursor_position(row, col);
            }
            'd' => {
                // VPA - Vertical Position Absolute.
                // The row parameter is addressed through the normal
                // ORIGIN-mode path; the column stays where it is. Preserve
                // column directly to avoid the off-by-one round-trip.
                let row = param!(0, 1) as usize;
                let col = self.terminal.grid().cursor().col;
                // Go through set_cursor_position for the row (so ORIGIN
                // mode is honored), but keep the column we just captured.
                self.terminal.set_cursor_position(row, col + 1);
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
                // SGR has no private/intermediate prefix. Xterm keyboard
                // protocols also use final `m` (for example `CSI > 4 m` to
                // reset modifyOtherKeys); treating those parameters as SGR
                // would incorrectly enable underline on application exit.
                if intermediates.is_empty() {
                    self.handle_sgr_raw(raw_params);
                } else {
                    log::trace!(
                        "Unhandled private CSI m: params={:?}, intermediates={:?}",
                        params,
                        intermediates
                    );
                }
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
                            // DSR - Device Status Report: respond with "OK"
                            log::trace!("Device status report requested");
                            self.terminal
                                .push_event(crate::event::TerminalEvent::PtyWrite(
                                    b"\x1b[0n".to_vec(),
                                ));
                        }
                        6 => {
                            // CPR - Cursor Position Report
                            let cursor = self.terminal.grid().cursor();
                            let row = cursor.row + 1; // 1-indexed
                            let col = cursor.col + 1; // 1-indexed
                            let response = format!("\x1b[{};{}R", row, col);
                            log::trace!("Cursor position report: row={}, col={}", row, col);
                            self.terminal
                                .push_event(crate::event::TerminalEvent::PtyWrite(
                                    response.into_bytes(),
                                ));
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
                self.terminal.insert_chars(n);
            }
            'P' => {
                // DCH - Delete Characters
                let n = param!(0, 1) as usize;
                self.terminal.delete_chars(n);
            }
            'L' => {
                // IL - Insert Lines
                let n = param!(0, 1) as usize;
                self.terminal.insert_lines(n);
            }
            'M' => {
                // DL - Delete Lines
                let n = param!(0, 1) as usize;
                self.terminal.delete_lines(n);
            }

            // Scroll region
            'r' => {
                // DECSTBM - Set Top and Bottom Margins
                let top = param!(0, 1) as usize;
                let bottom = param!(1, self.terminal.rows() as u16) as usize;
                self.terminal.set_scroll_region(top, bottom);
            }

            _ => {
                log::trace!(
                    "Unhandled CSI: params={:?}, intermediates={:?}, action='{}'",
                    params,
                    intermediates,
                    action
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
                self.terminal.set_tab_stop();
            }

            // RI - Reverse Index (move up, possibly scroll)
            (b'M', []) => {
                self.terminal.reverse_index();
            }

            // DECKPAM - Application Keypad Mode
            (b'=', []) => {
                self.terminal
                    .set_mode(TerminalMode::KEYPAD_APPLICATION, true);
            }

            // DECKPNM - Normal Keypad Mode
            (b'>', []) => {
                self.terminal
                    .set_mode(TerminalMode::KEYPAD_APPLICATION, false);
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
                    intermediates,
                    byte,
                    byte as char
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
                if params.len() > 1
                    && let Ok(title) = std::str::from_utf8(params[1])
                {
                    log::debug!("Set window title: {}", title);
                }
            }

            // Set icon name
            1 => {
                if params.len() > 1
                    && let Ok(name) = std::str::from_utf8(params[1])
                {
                    log::trace!("Set icon name: {}", name);
                }
            }

            // Set color palette
            4 => {
                log::trace!("Set color palette: {:?}", params);
            }

            // Hyperlink (OSC 8)
            // Format: OSC 8 ; params ; uri ST
            // params may contain id=value pairs separated by colons
            // Empty uri ends the hyperlink
            8 => {
                log::trace!("Hyperlink OSC 8: {:?}", params);
                // Strip the command "8" from params before passing
                if params.len() > 1 {
                    self.handle_osc_hyperlink(&params[1..]);
                }
            }

            // Clipboard (OSC 52)
            // Format: OSC 52 ; <selection> ; <data> ST
            // selection: c = clipboard, p = primary, etc.
            // data: base64 encoded content, or ? to query
            52 => {
                log::trace!("Clipboard operation: {:?}", params);
                // OSC 52 clipboard handling is done at the app level,
                // since it requires platform clipboard access
            }

            // Desktop notification (OSC 9) - iTerm2 style
            // Format: OSC 9 ; message ST
            9 => {
                if params.len() > 1
                    && let Ok(message) = std::str::from_utf8(params[1])
                {
                    log::info!("Desktop notification: {}", message);
                    self.terminal
                        .push_event(TerminalEvent::Notification(NotificationRequest {
                            title: None,
                            body: message.to_string(),
                        }));
                }
            }

            // Shell integration (OSC 133)
            // Used for prompt detection, command tracking
            133 => {
                // params[0] is "133", the rest are the actual parameters
                if params.len() > 1 {
                    self.terminal.handle_osc_133(&params[1..]);
                }
            }

            // Desktop notification (OSC 777) - rxvt-unicode style
            // Format: OSC 777 ; notify ; title ; body ST
            777 => {
                if params.len() >= 2 {
                    let subcommand = std::str::from_utf8(params[1]).unwrap_or("");
                    if subcommand == "notify" {
                        let title = if params.len() > 2 {
                            std::str::from_utf8(params[2]).ok().map(String::from)
                        } else {
                            None
                        };
                        let body = if params.len() > 3 {
                            std::str::from_utf8(params[3]).unwrap_or("").to_string()
                        } else {
                            String::new()
                        };
                        log::info!("Desktop notification: {:?} - {}", title, body);
                        self.terminal.push_event(TerminalEvent::Notification(
                            NotificationRequest { title, body },
                        ));
                    }
                }
            }

            _ => {
                log::trace!("Unhandled OSC {}: {:?}", command, params);
            }
        }
    }

    fn hook(&mut self, _params: &Params, intermediates: &[u8], _ignore: bool, action: char) {
        // DCS/APC hook - start of device control string or application program command
        // For Kitty graphics: APC (ESC _) followed by 'G'
        // VTE uses intermediates to identify the sequence type
        // APC starts with ESC _ which VTE represents with intermediates containing '_'
        if action == 'G' {
            // Kitty graphics command
            self.in_apc = true;
            self.apc_action = 'G';
            self.apc_data.clear();
            log::trace!("Starting Kitty graphics APC sequence");
        } else {
            log::trace!(
                "DCS hook: intermediates={:?}, action='{}'",
                intermediates,
                action
            );
        }
    }

    fn put(&mut self, byte: u8) {
        // DCS/APC data
        if self.in_apc {
            self.apc_data.push(byte);
        }
    }

    fn unhook(&mut self) {
        // DCS/APC unhook - end of device control string or application program command
        if self.in_apc {
            log::trace!(
                "Kitty graphics APC sequence received, {} bytes",
                self.apc_data.len()
            );
            // Process the accumulated data
            let data = std::mem::take(&mut self.apc_data);
            self.terminal.handle_graphics_command(&data);
            self.in_apc = false;
            self.apc_action = '\0';
        }
    }
}

impl Handler<'_> {
    /// Handle SGR with raw VTE Params (preserves colon-separated subparameters)
    fn handle_sgr_raw(&mut self, params: &Params) {
        // Check for empty params (reset all)
        if params.iter().next().is_none() {
            self.terminal.reset_attrs();
            return;
        }

        let groups: Vec<Vec<u16>> = params.iter().map(|group| group.to_vec()).collect();
        let mut group_index = 0;

        // Process each semicolon-separated parameter group. Extended colors
        // consume following groups in the common `38;2;r;g;b` / `48;5;n`
        // forms, while colon forms stay within one group.
        while group_index < groups.len() {
            let subparams = &groups[group_index];
            // Each subparams is a colon-separated group (e.g., [4, 3] for "4:3")
            let values = subparams.as_slice();

            if values.is_empty() {
                group_index += 1;
                continue;
            }

            let primary = values[0];

            match primary {
                // Reset
                0 => self.terminal.reset_attrs(),

                // Basic attributes
                1 => self.add_attr(CellAttrs::BOLD),
                2 => self.add_attr(CellAttrs::DIM),
                3 => self.add_attr(CellAttrs::ITALIC),

                // Underline with optional style subparameter
                4 => {
                    if values.len() > 1 {
                        // Has subparameter: 4:0 (none), 4:1 (single), 4:2 (double), 4:3 (curly), etc.
                        self.terminal.set_underline_style(values[1]);
                    } else {
                        // Plain "4" means single underline
                        self.terminal.set_underline_style(1);
                    }
                }

                5 => self.add_attr(CellAttrs::BLINK),
                7 => self.add_attr(CellAttrs::INVERSE),
                8 => self.add_attr(CellAttrs::HIDDEN),
                9 => self.add_attr(CellAttrs::STRIKE),

                // Double underline (alternate code)
                21 => self.terminal.set_underline_style(2),

                // Reset attributes
                22 => self.remove_attr(CellAttrs::BOLD | CellAttrs::DIM),
                23 => self.remove_attr(CellAttrs::ITALIC),
                24 => {
                    // Reset all underline styles
                    self.remove_attr(CellAttrs::ALL_UNDERLINES);
                }
                25 => self.remove_attr(CellAttrs::BLINK),
                27 => self.remove_attr(CellAttrs::INVERSE),
                28 => self.remove_attr(CellAttrs::HIDDEN),
                29 => self.remove_attr(CellAttrs::STRIKE),

                // Foreground colors (30-37)
                30..=37 => self.terminal.set_fg(
                    ansi_color(primary - 30),
                    ColorSpec::Indexed((primary - 30) as u8),
                ),

                // Extended foreground color (38:2:R:G:B or 38:5:N)
                38 => {
                    let parsed = if values.len() > 1 {
                        self.parse_extended_color_spec_from_subparams(&values[1..])
                            .map(|color| (color, 0))
                    } else {
                        self.parse_semicolon_extended_color(&groups, group_index)
                    };
                    if let Some(((color, spec), consumed)) = parsed {
                        self.terminal.set_fg(color, spec);
                        group_index += consumed;
                    }
                }

                // Default foreground
                39 => self
                    .terminal
                    .set_fg(Color::default_fg(), ColorSpec::Default),

                // Background colors (40-47)
                40..=47 => self.terminal.set_bg(
                    ansi_color(primary - 40),
                    ColorSpec::Indexed((primary - 40) as u8),
                ),

                // Extended background color (48:2:R:G:B or 48:5:N)
                48 => {
                    let parsed = if values.len() > 1 {
                        self.parse_extended_color_spec_from_subparams(&values[1..])
                            .map(|color| (color, 0))
                    } else {
                        self.parse_semicolon_extended_color(&groups, group_index)
                    };
                    if let Some(((color, spec), consumed)) = parsed {
                        self.terminal.set_bg(color, spec);
                        group_index += consumed;
                    }
                }

                // Default background
                49 => self
                    .terminal
                    .set_bg(Color::default_bg(), ColorSpec::Default),

                // Extended underline color (58:2:R:G:B or 58:5:N)
                58 => {
                    let parsed = if values.len() > 1 {
                        self.parse_extended_color_spec_from_subparams(&values[1..])
                            .map(|color| (color, 0))
                    } else {
                        self.parse_semicolon_extended_color(&groups, group_index)
                    };
                    if let Some(((color, _), consumed)) = parsed {
                        self.terminal.set_underline_color(color);
                        group_index += consumed;
                    }
                }

                // Reset underline color
                59 => self.terminal.reset_underline_color(),

                // Bright foreground colors (90-97)
                90..=97 => self.terminal.set_fg(
                    ansi_bright_color(primary - 90),
                    ColorSpec::Indexed((primary - 90 + 8) as u8),
                ),

                // Bright background colors (100-107)
                100..=107 => self.terminal.set_bg(
                    ansi_bright_color(primary - 100),
                    ColorSpec::Indexed((primary - 100 + 8) as u8),
                ),

                _ => {
                    log::trace!("Unknown SGR parameter: {}", primary);
                }
            }
            group_index += 1;
        }
    }

    /// Parse the groups following a semicolon-form 38/48/58 parameter.
    /// Returns the color and number of additional groups consumed.
    fn parse_semicolon_extended_color(
        &self,
        groups: &[Vec<u16>],
        start: usize,
    ) -> Option<((Color, ColorSpec), usize)> {
        let mode = *groups.get(start + 1)?.first()?;
        match mode {
            2 => {
                let r = *groups.get(start + 2)?.first()?;
                let g = *groups.get(start + 3)?.first()?;
                let b = *groups.get(start + 4)?.first()?;
                if r > 255 || g > 255 || b > 255 {
                    return None;
                }
                let color = Color::rgb(r as u8, g as u8, b as u8);
                Some(((color, ColorSpec::TrueColor(color)), 4))
            }
            5 => {
                let index = *groups.get(start + 2)?.first()?;
                if index > 255 {
                    return None;
                }
                let index = index as u8;
                Some((
                    (super::color::color_256(index), ColorSpec::Indexed(index)),
                    2,
                ))
            }
            _ => None,
        }
    }

    /// Parse extended color with its spec from colon-separated subparameters
    fn parse_extended_color_spec_from_subparams(
        &self,
        subparams: &[u16],
    ) -> Option<(Color, ColorSpec)> {
        if subparams.is_empty() {
            return None;
        }

        match subparams[0] {
            2 => {
                // RGB color: 2:R:G:B (may have additional empty : separators)
                // Some terminals send 2::R:G:B (with colorspace placeholder)
                let rgb = if subparams.len() >= 5 {
                    &subparams[subparams.len() - 3..]
                } else {
                    subparams.get(1..4)?
                };
                if rgb.iter().any(|component| *component > 255) {
                    return None;
                }
                let color = Color::rgb(rgb[0] as u8, rgb[1] as u8, rgb[2] as u8);
                Some((color, ColorSpec::TrueColor(color)))
            }
            5 => {
                // 256-color palette: 5:N
                if subparams.len() > 1 {
                    if subparams[1] > 255 {
                        return None;
                    }
                    let idx = subparams[1] as u8;
                    Some((super::color::color_256(idx), ColorSpec::Indexed(idx)))
                } else {
                    None
                }
            }
            _ => None,
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
                        self.terminal
                            .set_mode(TerminalMode::CURSOR_KEYS_APPLICATION, enable);
                    }
                    6 => {
                        // DECOM - Origin mode
                        self.terminal.set_mode(TerminalMode::ORIGIN, enable);
                        self.terminal.set_cursor_position(1, 1);
                    }
                    7 => {
                        // DECAWM - Autowrap
                        self.terminal.set_mode(TerminalMode::AUTOWRAP, enable);
                    }
                    12 => {
                        // ATT610 - Cursor blinking
                        self.terminal
                            .set_mode(TerminalMode::CURSOR_BLINKING, enable);
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
                        self.terminal
                            .set_mode(TerminalMode::MOUSE_BUTTON_EVENT, enable);
                    }
                    1003 => {
                        // Any event mouse tracking
                        self.terminal
                            .set_mode(TerminalMode::MOUSE_ANY_EVENT, enable);
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
                    47 | 1047 => {
                        // Alternate screen buffer without cursor save/restore.
                        if enable {
                            self.terminal.enter_alt_screen();
                        } else {
                            self.terminal.exit_alt_screen();
                        }
                    }
                    1048 => {
                        // Save/restore cursor and rendition state.
                        if enable {
                            self.terminal.save_cursor();
                        } else {
                            self.terminal.restore_cursor();
                        }
                    }
                    1049 => {
                        // Save cursor/rendition, use a cleared alternate screen,
                        // then restore the primary state when leaving.
                        if enable {
                            self.terminal.save_cursor();
                            self.terminal.enter_alt_screen();
                        } else {
                            self.terminal.exit_alt_screen();
                            self.terminal.restore_cursor();
                        }
                    }
                    2004 => {
                        // Bracketed paste mode
                        self.terminal
                            .set_mode(TerminalMode::BRACKETED_PASTE, enable);
                    }
                    2026 => {
                        // Synchronized output mode
                        // When enabled, terminal should buffer output until disabled
                        self.terminal
                            .set_mode(TerminalMode::SYNCHRONIZED_OUTPUT, enable);
                        log::trace!("Synchronized output mode: {}", enable);
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
                        self.terminal
                            .set_mode(TerminalMode::LINEFEED_NEWLINE, enable);
                    }
                    _ => {
                        log::trace!("Unknown ANSI mode {}: {}", mode, enable);
                    }
                }
            }
        }
    }

    fn add_attr(&mut self, attr: CellAttrs) {
        self.terminal.add_attrs(attr);
    }

    fn remove_attr(&mut self, attr: CellAttrs) {
        self.terminal.remove_attrs(attr);
    }

    /// Handle OSC 8 hyperlink
    ///
    /// Format: OSC 8 ; params ; uri ST
    /// - params may contain id=value pairs separated by colons
    /// - Empty uri ends the hyperlink
    fn handle_osc_hyperlink(&mut self, params: &[&[u8]]) {
        // OSC 8 should have at least 2 parameters after the command:
        // params[0] is "8" (command), params[1] is params, params[2] is URI
        // But the params slice we receive has the command already stripped
        // So params[0] is params (may be empty), params[1] is URI

        if params.len() < 2 {
            log::trace!("OSC 8: Invalid format, need at least 2 params");
            return;
        }

        let params_str = std::str::from_utf8(params[0]).unwrap_or("");
        let uri_str = std::str::from_utf8(params[1]).unwrap_or("");

        // If URI is empty, end the hyperlink
        if uri_str.is_empty() {
            log::trace!("OSC 8: End hyperlink");
            self.terminal.reset_hyperlink();
            return;
        }

        // Parse the id parameter from params string
        // Format: key=value:key=value:...
        let mut link_id: Option<String> = None;
        if !params_str.is_empty() {
            for param in params_str.split(':') {
                if let Some(value) = param.strip_prefix("id=") {
                    link_id = Some(value.to_string());
                    break;
                }
            }
        }

        log::trace!("OSC 8: Set hyperlink url={} id={:?}", uri_str, link_id);
        self.terminal.set_hyperlink(uri_str.to_string(), link_id);
    }
}
