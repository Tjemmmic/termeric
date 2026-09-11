//! Escape sequence processor

use super::handler::{Handler, MAX_OSC_SEQUENCE_BYTES};
use crate::term::Terminal;
use vte::Parser;

pub const MAX_DCS_SEQUENCE_BYTES: usize = 16 * 1024 * 1024 + 64;

/// Mirror the UTF-8 decoder's small amount of pending state so bytes in the
/// C1 range are interpreted as controls only when they are not continuations
/// of a valid or currently-invalidated UTF-8 sequence. The byte that makes a
/// sequence invalid is consumed by `vte`, so it must not be reinterpreted.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Utf8InputState {
    #[default]
    Ground,
    Tail1,
    Tail2,
    Tail3,
    E0,
    Ed,
    F0,
    F4,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum OscPayloadMode {
    /// Fold the command field while determining which payload policy applies.
    #[default]
    Command,
    /// ECMA-48 control-string data: ignore bit 8 on GR bytes.
    Fold,
    /// Preserve UTF-8 user text for supported title/link/notification paths.
    Utf8,
}

fn fold_gr_byte(byte: u8) -> u8 {
    if byte >= 0xa0 { byte & 0x7f } else { byte }
}

impl Utf8InputState {
    /// Return true when `byte` belongs to an in-progress UTF-8 decode (or is a
    /// leading byte that starts one) and must therefore bypass C1 recognition.
    fn consume(&mut self, byte: u8) -> bool {
        use Utf8InputState::{E0, Ed, F0, F4, Ground, Tail1, Tail2, Tail3};
        let prior = *self;
        *self = match prior {
            Ground => match byte {
                0xc2..=0xdf => Tail1,
                0xe0 => E0,
                0xe1..=0xec | 0xee..=0xef => Tail2,
                0xed => Ed,
                0xf0 => F0,
                0xf1..=0xf3 => Tail3,
                0xf4 => F4,
                _ => Ground,
            },
            E0 if (0xa0..=0xbf).contains(&byte) => Tail1,
            Ed if (0x80..=0x9f).contains(&byte) => Tail1,
            F0 if (0x90..=0xbf).contains(&byte) => Tail2,
            F4 if (0x80..=0x8f).contains(&byte) => Tail2,
            Tail3 if (0x80..=0xbf).contains(&byte) => Tail2,
            Tail2 if (0x80..=0xbf).contains(&byte) => Tail1,
            Tail1 if (0x80..=0xbf).contains(&byte) => Ground,
            _ => Ground,
        };
        prior != Ground || *self != Ground
    }

    fn reset(&mut self) {
        *self = Self::Ground;
    }
}

/// Processor for parsing and handling escape sequences
pub struct Processor {
    parser: Parser,
    utf8_input_state: Utf8InputState,
    escape_sequence_active: bool,
    escape_pending: bool,
    csi_active: bool,
    /// VTE canonicalizes an omitted CSI parameter and an explicit zero to the
    /// same value. Xterm's resource protocols assign different meanings to
    /// omitted and explicit zero, so retain the minimal raw syntax needed to
    /// distinguish parameterless `CSI > f` and `CSI > m`.
    csi_xterm_private: bool,
    csi_parameter_syntax_seen: bool,
    osc_active: bool,
    osc_escape_pending: bool,
    osc_bytes: usize,
    osc_discarding: bool,
    osc_payload_mode: OscPayloadMode,
    osc_command: u16,
    osc_command_valid: bool,
    apc_active: bool,
    apc_escape_pending: bool,
    apc_first_byte: bool,
    apc_is_kitty: bool,
    apc_is_graphics_relay: bool,
    dcs_active: bool,
    dcs_escape_pending: bool,
    dcs_discarding: bool,
    dcs_bytes: Vec<u8>,
}

impl Processor {
    /// Create a new processor
    pub fn new() -> Self {
        Self {
            parser: Parser::new(),
            utf8_input_state: Utf8InputState::Ground,
            escape_sequence_active: false,
            escape_pending: false,
            csi_active: false,
            csi_xterm_private: false,
            csi_parameter_syntax_seen: false,
            osc_active: false,
            osc_escape_pending: false,
            osc_bytes: 0,
            osc_discarding: false,
            osc_payload_mode: OscPayloadMode::Command,
            osc_command: 0,
            osc_command_valid: true,
            apc_active: false,
            apc_escape_pending: false,
            apc_first_byte: false,
            apc_is_kitty: false,
            apc_is_graphics_relay: false,
            dcs_active: false,
            dcs_escape_pending: false,
            dcs_discarding: false,
            dcs_bytes: Vec::new(),
        }
    }

    fn begin_dcs(&mut self) {
        self.parser = Parser::new();
        self.utf8_input_state.reset();
        self.dcs_active = true;
        self.dcs_escape_pending = false;
        self.dcs_discarding = false;
        self.dcs_bytes.clear();
        self.escape_sequence_active = false;
        self.escape_pending = false;
        self.csi_active = false;
    }

    fn begin_apc(&mut self) {
        self.parser = Parser::new();
        self.utf8_input_state.reset();
        self.apc_active = true;
        self.apc_escape_pending = false;
        self.apc_first_byte = true;
        self.apc_is_kitty = false;
        self.apc_is_graphics_relay = false;
        self.escape_sequence_active = false;
        self.escape_pending = false;
        self.csi_active = false;
    }

    fn begin_osc(&mut self, handler: &mut Handler<'_>) {
        self.parser = Parser::new();
        self.parser.advance(handler, 0x1b);
        self.parser.advance(handler, b']');
        self.utf8_input_state.reset();
        self.osc_active = true;
        self.osc_bytes = 0;
        self.osc_discarding = false;
        self.osc_escape_pending = false;
        self.osc_payload_mode = OscPayloadMode::Command;
        self.osc_command = 0;
        self.osc_command_valid = true;
        self.escape_sequence_active = false;
        self.escape_pending = false;
        self.csi_active = false;
    }

    fn normalize_osc_byte(&mut self, byte: u8) -> u8 {
        let folded = fold_gr_byte(byte);
        if self.osc_payload_mode == OscPayloadMode::Command {
            match folded {
                b'0'..=b'9' if self.osc_command_valid => {
                    self.osc_command = self
                        .osc_command
                        .checked_mul(10)
                        .and_then(|value| value.checked_add(u16::from(folded - b'0')))
                        .unwrap_or_else(|| {
                            self.osc_command_valid = false;
                            0
                        });
                }
                b';' => {
                    self.osc_payload_mode = if self.osc_command_valid
                        && matches!(self.osc_command, 0 | 1 | 2 | 8 | 9 | 66 | 99 | 777)
                    {
                        OscPayloadMode::Utf8
                    } else {
                        OscPayloadMode::Fold
                    };
                }
                _ => self.osc_payload_mode = OscPayloadMode::Fold,
            }
            return folded;
        }

        if self.osc_payload_mode == OscPayloadMode::Utf8 {
            byte
        } else {
            folded
        }
    }

    /// Dispatch one raw C1 byte by its equivalent 7-bit sequence. This is
    /// called only while no UTF-8 scalar is pending.
    fn dispatch_c1(&mut self, handler: &mut Handler<'_>, byte: u8) -> bool {
        let escape_final = match byte {
            0x84 => Some(b'D'), // IND
            0x85 => Some(b'E'), // NEL
            0x88 => Some(b'H'), // HTS
            0x8d => Some(b'M'), // RI
            0x8e => Some(b'N'), // SS2
            0x8f => Some(b'O'), // SS3
            0x96 => Some(b'V'), // SPA (recognized, currently a no-op)
            0x97 => Some(b'W'), // EPA (recognized, currently a no-op)
            0x98 => Some(b'X'), // SOS (vte discards through ST)
            0x9a => Some(b'Z'), // DECID
            0x9e => Some(b'^'), // PM (vte discards through ST)
            _ => None,
        };
        if let Some(final_byte) = escape_final {
            self.parser = Parser::new();
            self.parser.advance(handler, 0x1b);
            self.parser.advance(handler, final_byte);
            self.escape_sequence_active = false;
            self.escape_pending = false;
            self.csi_active = false;
            return true;
        }

        match byte {
            0x90 => self.begin_dcs(),
            0x9b => {
                self.parser = Parser::new();
                self.parser.advance(handler, 0x1b);
                self.parser.advance(handler, b'[');
                self.escape_sequence_active = false;
                self.escape_pending = false;
                self.csi_active = true;
                self.csi_xterm_private = false;
                self.csi_parameter_syntax_seen = false;
            }
            0x9c => {
                // ST outside a control string cancels any partial parser state.
                self.parser = Parser::new();
                self.escape_sequence_active = false;
                self.escape_pending = false;
                self.csi_active = false;
            }
            0x9d => self.begin_osc(handler),
            0x9f => self.begin_apc(),
            _ => return false,
        }
        true
    }

    /// Process input bytes, updating terminal state
    pub fn process(&mut self, terminal: &mut Terminal, bytes: &[u8]) {
        let mut handler = Handler::new(terminal);
        for &byte in bytes {
            if self.dcs_active {
                if matches!(byte, 0x18 | 0x1a) {
                    self.parser = Parser::new();
                    self.utf8_input_state.reset();
                    self.dcs_active = false;
                    self.dcs_escape_pending = false;
                    self.dcs_discarding = false;
                    self.dcs_bytes.clear();
                    self.escape_sequence_active = false;
                    self.escape_pending = false;
                    self.csi_active = false;
                    continue;
                }
                let byte = fold_gr_byte(byte);
                let terminated = byte == 0x9c || (self.dcs_escape_pending && byte == b'\\');
                let cancelled = self.dcs_escape_pending && byte != b'\\';
                if terminated || cancelled {
                    if terminated && !self.dcs_discarding {
                        let payload = std::mem::take(&mut self.dcs_bytes);
                        handler.handle_dcs(&payload);
                    }
                    self.parser = Parser::new();
                    self.utf8_input_state.reset();
                    self.dcs_active = false;
                    self.dcs_escape_pending = false;
                    self.dcs_discarding = false;
                    self.dcs_bytes = Vec::new();
                    self.escape_sequence_active = false;
                    self.escape_pending = false;
                    self.csi_active = false;
                    if cancelled {
                        self.parser.advance(&mut handler, 0x1b);
                        self.parser.advance(&mut handler, byte);
                        self.escape_sequence_active = byte == 0x1b;
                        self.escape_pending = byte == 0x1b;
                    }
                    continue;
                }

                if self.dcs_escape_pending {
                    self.dcs_bytes.push(0x1b);
                }
                self.dcs_escape_pending = byte == 0x1b;
                if !self.dcs_escape_pending && !self.dcs_discarding {
                    self.dcs_bytes.push(byte);
                    if self.dcs_bytes.len() > MAX_DCS_SEQUENCE_BYTES {
                        self.dcs_bytes.clear();
                        self.dcs_discarding = true;
                    }
                }
                continue;
            }

            if self.apc_active {
                let byte = fold_gr_byte(byte);
                if self.apc_escape_pending {
                    if byte == b'\\' {
                        if self.apc_is_kitty {
                            handler.finish_graphics_apc();
                        } else if self.apc_is_graphics_relay {
                            handler.finish_graphics_relay_apc();
                        }
                        self.parser = Parser::new();
                        self.utf8_input_state.reset();
                        self.apc_active = false;
                        self.apc_escape_pending = false;
                        self.apc_first_byte = false;
                        self.apc_is_kitty = false;
                        self.apc_is_graphics_relay = false;
                        self.escape_sequence_active = false;
                        self.escape_pending = false;
                        self.csi_active = false;
                        continue;
                    }

                    if self.apc_is_kitty {
                        handler.cancel_graphics_apc();
                    } else if self.apc_is_graphics_relay {
                        handler.cancel_graphics_relay_apc();
                    }
                    self.parser = Parser::new();
                    self.utf8_input_state.reset();
                    self.apc_active = false;
                    self.apc_escape_pending = false;
                    self.apc_first_byte = false;
                    self.apc_is_kitty = false;
                    self.apc_is_graphics_relay = false;
                    self.escape_sequence_active = false;
                    self.escape_pending = false;
                    self.csi_active = false;
                    self.parser.advance(&mut handler, byte);
                    self.escape_sequence_active = byte == 0x1b;
                    self.escape_pending = byte == 0x1b;
                    continue;
                }

                if byte == 0x9c {
                    if self.apc_is_kitty {
                        handler.finish_graphics_apc();
                    } else if self.apc_is_graphics_relay {
                        handler.finish_graphics_relay_apc();
                    }
                    self.parser = Parser::new();
                    self.utf8_input_state.reset();
                    self.apc_active = false;
                    self.apc_first_byte = false;
                    self.apc_is_kitty = false;
                    self.apc_is_graphics_relay = false;
                    self.escape_sequence_active = false;
                    self.escape_pending = false;
                    self.csi_active = false;
                } else if byte == 0x1b {
                    self.apc_escape_pending = true;
                } else if self.apc_first_byte {
                    self.apc_first_byte = false;
                    self.apc_is_kitty = byte == b'G';
                    self.apc_is_graphics_relay = byte == b'T';
                    if self.apc_is_kitty {
                        handler.begin_graphics_apc();
                    } else if self.apc_is_graphics_relay {
                        handler.begin_graphics_relay_apc();
                    }
                } else if self.apc_is_kitty {
                    handler.push_graphics_apc_byte(byte);
                } else if self.apc_is_graphics_relay {
                    handler.push_graphics_relay_apc_byte(byte);
                }
                continue;
            }

            if self.osc_active {
                let c1_terminated = byte == 0x9c;
                let terminator_byte = fold_gr_byte(byte);
                let terminated = c1_terminated
                    || byte == 0x07
                    || (self.osc_escape_pending && terminator_byte == b'\\');
                let cancelled = self.osc_escape_pending && terminator_byte != b'\\';
                if self.osc_discarding {
                    if terminated || cancelled {
                        self.parser = Parser::new();
                        self.utf8_input_state.reset();
                        self.osc_active = false;
                        self.osc_discarding = false;
                        self.osc_escape_pending = false;
                        self.escape_sequence_active = false;
                        self.escape_pending = false;
                        self.csi_active = false;
                    } else {
                        self.osc_escape_pending = byte == 0x1b;
                    }
                    continue;
                }

                if !terminated {
                    self.osc_bytes = self.osc_bytes.saturating_add(1);
                    if self.osc_bytes > MAX_OSC_SEQUENCE_BYTES {
                        self.parser = Parser::new();
                        self.osc_discarding = true;
                        self.osc_escape_pending = byte == 0x1b;
                        continue;
                    }
                }
                if c1_terminated {
                    // vte 0.13 does not treat raw ST as OSC termination, so
                    // supply its equivalent 7-bit terminator to complete the
                    // existing bounded OSC dispatch path.
                    self.parser.advance(&mut handler, 0x1b);
                    self.parser.advance(&mut handler, b'\\');
                } else if self.osc_escape_pending && terminator_byte == b'\\' {
                    self.parser.advance(&mut handler, b'\\');
                } else {
                    let byte = self.normalize_osc_byte(byte);
                    self.parser.advance(&mut handler, byte);
                }
                if terminated || cancelled {
                    // The parser's std OSC buffer otherwise retains the peak
                    // allocation for this terminal's lifetime.
                    self.parser = Parser::new();
                    self.utf8_input_state.reset();
                    self.osc_active = false;
                    self.osc_escape_pending = false;
                    self.escape_sequence_active = false;
                    self.escape_pending = false;
                    self.csi_active = false;
                } else {
                    self.osc_escape_pending = byte == 0x1b;
                }
                continue;
            }

            if self.csi_active {
                if (0x80..=0x9f).contains(&byte) {
                    if self.dispatch_c1(&mut handler, byte) {
                        continue;
                    }
                    // Unsupported C1 functions do not become UTF-8 or GR data
                    // in the middle of a control sequence.
                    continue;
                }

                let byte = fold_gr_byte(byte);
                if matches!(byte, b'f' | b'm' | b'n')
                    && self.csi_xterm_private
                    && !self.csi_parameter_syntax_seen
                {
                    // VTE canonicalizes omitted and explicit-zero parameters.
                    // Complete parameterless all-resource resets and xterm's
                    // default resource-2 disable here so explicit resource 0
                    // remains a distinct resource-specific operation.
                    handler.handle_parameterless_xterm_key_control(byte);
                    self.parser = Parser::new();
                    self.csi_active = false;
                    self.csi_xterm_private = false;
                    self.csi_parameter_syntax_seen = false;
                    self.escape_sequence_active = false;
                    self.escape_pending = false;
                    continue;
                }
                self.parser.advance(&mut handler, byte);
                if byte == b'>' {
                    if !self.csi_parameter_syntax_seen && !self.csi_xterm_private {
                        self.csi_xterm_private = true;
                    } else {
                        // A duplicate or non-leading private marker makes the
                        // sequence ineligible for the parameterless reset.
                        self.csi_parameter_syntax_seen = true;
                    }
                } else if (0x30..=0x3f).contains(&byte) {
                    self.csi_parameter_syntax_seen = true;
                }
                match byte {
                    0x18 | 0x1a => {
                        self.csi_active = false;
                        self.escape_sequence_active = false;
                        self.escape_pending = false;
                    }
                    0x1b => {
                        self.csi_active = false;
                        self.escape_sequence_active = true;
                        self.escape_pending = true;
                    }
                    0x40..=0x7e => {
                        self.csi_active = false;
                        self.escape_sequence_active = false;
                        self.escape_pending = false;
                    }
                    _ => {}
                }
                continue;
            }

            if self.escape_sequence_active {
                if (0x80..=0x9f).contains(&byte) {
                    if self.dispatch_c1(&mut handler, byte) {
                        continue;
                    }
                    continue;
                }

                let byte = fold_gr_byte(byte);
                if self.escape_pending {
                    match byte {
                        b'P' => {
                            self.begin_dcs();
                            continue;
                        }
                        b'_' => {
                            // vte intentionally ignores SOS/PM/APC payloads,
                            // so frame APC here.
                            self.begin_apc();
                            continue;
                        }
                        b']' => {
                            self.begin_osc(&mut handler);
                            continue;
                        }
                        _ => {}
                    }
                }

                let starts_csi = self.escape_pending && byte == b'[';
                self.parser.advance(&mut handler, byte);
                if starts_csi {
                    self.csi_active = true;
                    self.csi_xterm_private = false;
                    self.csi_parameter_syntax_seen = false;
                    self.escape_sequence_active = false;
                    self.escape_pending = false;
                } else {
                    match byte {
                        0x18 | 0x1a => {
                            self.escape_sequence_active = false;
                            self.escape_pending = false;
                        }
                        0x1b => {
                            self.escape_sequence_active = true;
                            self.escape_pending = true;
                        }
                        0x20..=0x2f | 0x00..=0x17 | 0x19 | 0x1c..=0x1f | 0x7f => {
                            self.escape_pending = false;
                        }
                        _ => {
                            self.escape_sequence_active = false;
                            self.escape_pending = false;
                        }
                    }
                }
                continue;
            }

            if self.utf8_input_state.consume(byte) {
                self.parser.advance(&mut handler, byte);
                self.escape_sequence_active = false;
                self.escape_pending = false;
                continue;
            }

            if self.dispatch_c1(&mut handler, byte) {
                continue;
            }

            self.parser.advance(&mut handler, byte);
            self.escape_sequence_active = byte == 0x1b;
            self.escape_pending = byte == 0x1b;
        }
    }
}

impl Default for Processor {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::grid::{CellAttrs, Color, ColorSpec};
    use crate::term::TerminalMode;

    #[test]
    fn test_processor_creation() {
        let _processor = Processor::new();
    }

    #[test]
    fn test_simple_text() {
        let mut processor = Processor::new();
        let mut terminal = Terminal::new(80, 24);

        processor.process(&mut terminal, b"Hello");

        // Check that characters were written
        let grid = terminal.grid();
        assert_eq!(grid.cell(0, 0).unwrap().content.char(), Some('H'));
        assert_eq!(grid.cell(0, 1).unwrap().content.char(), Some('e'));
        assert_eq!(grid.cell(0, 2).unwrap().content.char(), Some('l'));
        assert_eq!(grid.cell(0, 3).unwrap().content.char(), Some('l'));
        assert_eq!(grid.cell(0, 4).unwrap().content.char(), Some('o'));
    }

    #[test]
    fn c1_input_is_utf8_safe_and_s7c1t_s8c1t_frame_every_core_reply() {
        let mut processor = Processor::new();
        let mut terminal = Terminal::new(8, 4);

        processor.process(
            &mut terminal,
            b"\x1b G\x9b5n\x90$qm\x9c\x9fGa=t,f=32,s=1,v=1,i=44;AQIDBA==\x9c",
        );
        assert!(terminal.uses_8bit_controls());
        assert_eq!(
            terminal.drain_events(),
            vec![
                crate::event::TerminalEvent::PtyWrite(b"\x9b0n".to_vec()),
                crate::event::TerminalEvent::PtyWrite(b"\x901$r0m\x9c".to_vec()),
                crate::event::TerminalEvent::PtyWrite(b"\x9fGi=44;OK\x9c".to_vec()),
            ]
        );

        // Input accepts C1 independently of reply preference. DECSTR preserves
        // the preference, S7C1T changes it, and RIS restores the 7-bit default.
        processor.process(&mut terminal, b"\x1b[!p\x9b5n\x1b F\x9b5n");
        assert_eq!(
            terminal.drain_events(),
            vec![
                crate::event::TerminalEvent::PtyWrite(b"\x9b0n".to_vec()),
                crate::event::TerminalEvent::PtyWrite(b"\x1b[0n".to_vec()),
            ]
        );
        processor.process(&mut terminal, b"\x1b G\x1bc\x9b5n");
        assert!(!terminal.uses_8bit_controls());
        assert_eq!(
            terminal.drain_events(),
            vec![crate::event::TerminalEvent::PtyWrite(b"\x1b[0n".to_vec())]
        );

        let mut unicode = Terminal::new(8, 4);
        processor = Processor::new();
        // 0x9b is the second byte of U+011B here and must not become CSI; the
        // following standalone 0x9b is a real C1 CSI.
        processor.process(&mut unicode, b"\xc4\x9b\x9b2;2HX\x85Y");
        assert_eq!(unicode.grid().cell(0, 0).unwrap().content.char(), Some('ě'));
        assert_eq!(unicode.grid().cell(1, 1).unwrap().content.char(), Some('X'));
        assert_eq!(unicode.grid().cell(2, 0).unwrap().content.char(), Some('Y'));

        processor.process(&mut unicode, b"\x9d2;c1-title\x9c");
        assert_eq!(
            unicode.drain_events(),
            vec![crate::event::TerminalEvent::TitleChanged("c1-title".into())]
        );

        let mut folded = Terminal::new(8, 4);
        processor = Processor::new();
        // ECMA-48 permits GR bytes in control bodies by ignoring bit 8. This
        // covers an ESC intermediate/final, 7/8-bit CSI, DCS/OSC payloads,
        // and high-bit final bytes in a two-byte ST.
        processor.process(
            &mut folded,
            b"\x1b\xa0\xc7\x1b[\xb5\xee\x9b\xb6\xee\x1bP\xa4\xf1\xed\x1b\xdc\x1b]\xb1\xb1\xbb\xbf\x1b\xdc",
        );
        assert!(folded.uses_8bit_controls());
        assert_eq!(
            folded.drain_events(),
            vec![
                crate::event::TerminalEvent::PtyWrite(b"\x9b0n".to_vec()),
                crate::event::TerminalEvent::PtyWrite(b"\x9b1;1R".to_vec()),
                crate::event::TerminalEvent::PtyWrite(b"\x901$r0m\x9c".to_vec()),
                crate::event::TerminalEvent::DynamicColor(
                    crate::event::DynamicColorOperation::Query {
                        target: crate::event::DynamicColorTarget::Background,
                        reply_uses_8bit_controls: true,
                    },
                ),
            ]
        );

        // User-facing OSC text remains UTF-8 even though the command prefix
        // itself may use GR equivalents. Unicode titles and hyperlink URIs
        // therefore do not regress when control-body folding is enabled.
        let mut human_osc = b"\x1b]\xb2\xbb".to_vec();
        human_osc.extend_from_slice("hé🌱".as_bytes());
        human_osc.extend_from_slice(b"\x1b\xdc\x1b]\xb8\xbb;");
        human_osc.extend_from_slice("https://例.example/路径".as_bytes());
        human_osc.extend_from_slice(b"\x1b\xdcX\x1b]8;;\x1b\\");
        processor.process(&mut folded, &human_osc);
        assert_eq!(
            folded.drain_events(),
            vec![crate::event::TerminalEvent::TitleChanged("hé🌱".into())]
        );
        let link = folded.grid().cell(0, 0).unwrap().hyperlink_id.unwrap();
        assert_eq!(
            folded.get_hyperlink_url(link),
            Some("https://例.example/路径")
        );

        let mut high_bit_apc = b"\x1b_".to_vec();
        high_bit_apc.extend(
            b"Ga=t,f=32,s=1,v=1,i=45;AQIDBA=="
                .iter()
                .map(|byte| byte | 0x80),
        );
        high_bit_apc.extend_from_slice(b"\x1b\xdc");
        processor.process(&mut folded, &high_bit_apc);
        assert_eq!(
            folded.drain_events(),
            vec![crate::event::TerminalEvent::PtyWrite(
                b"\x9fGi=45;OK\x9c".to_vec()
            )]
        );
    }

    #[test]
    fn test_newline() {
        let mut processor = Processor::new();
        let mut terminal = Terminal::new(80, 24);

        // Note: \n is LF (line feed), \r is CR (carriage return)
        // To start at column 0 on new line, need \r\n
        processor.process(&mut terminal, b"Line1\r\nLine2");

        let grid = terminal.grid();
        assert_eq!(grid.cell(0, 0).unwrap().content.char(), Some('L'));
        assert_eq!(grid.cell(1, 0).unwrap().content.char(), Some('L'));
    }

    #[test]
    fn test_cursor_movement() {
        let mut processor = Processor::new();
        let mut terminal = Terminal::new(80, 24);

        // Move cursor to position 5,10 (1-indexed)
        processor.process(&mut terminal, b"\x1b[5;10H");

        let cursor = terminal.grid().cursor();
        assert_eq!(cursor.row, 4);
        assert_eq!(cursor.col, 9);
    }

    #[test]
    fn test_clear_screen() {
        let mut processor = Processor::new();
        let mut terminal = Terminal::new(80, 24);

        processor.process(&mut terminal, b"Some text\x1b[2J");

        // After clear, cursor position is preserved but content is cleared
        let grid = terminal.grid();
        assert!(grid.cell(0, 0).unwrap().is_empty());
    }

    #[test]
    fn semicolon_and_colon_extended_colors_are_consumed_atomically() {
        let mut processor = Processor::new();
        let mut terminal = Terminal::new(10, 2);

        processor.process(
            &mut terminal,
            b"\x1b[38;2;12;34;56mA\x1b[48;5;196mB\x1b[38:2::90:80:70mC",
        );

        let grid = terminal.grid();
        let a = grid.cell(0, 0).unwrap();
        assert_eq!(a.fg, Color::rgb(12, 34, 56));
        assert_eq!(a.fg_spec, ColorSpec::TrueColor(Color::rgb(12, 34, 56)));
        assert!(!a.attrs.contains(CellAttrs::DIM));

        let b = grid.cell(0, 1).unwrap();
        assert_eq!(b.bg_spec, ColorSpec::Indexed(196));

        let c = grid.cell(0, 2).unwrap();
        assert_eq!(c.fg, Color::rgb(90, 80, 70));
        assert_eq!(c.fg_spec, ColorSpec::TrueColor(Color::rgb(90, 80, 70)));
    }

    #[test]
    fn alternate_screen_restores_cursor_and_rendition_state() {
        let mut processor = Processor::new();
        let mut terminal = Terminal::new(10, 2);

        processor.process(
            &mut terminal,
            b"A\x1b[?1049h\x1b[7;48;2;90;80;70mB\x1b[?1049lC",
        );

        let grid = terminal.grid();
        assert_eq!(grid.cell(0, 0).unwrap().content.char(), Some('A'));
        let c = grid.cell(0, 1).unwrap();
        assert_eq!(c.content.char(), Some('C'));
        assert_eq!(c.fg_spec, ColorSpec::Default);
        assert_eq!(c.bg_spec, ColorSpec::Default);
        assert!(!c.attrs.contains(CellAttrs::INVERSE));

        // CSI s/u, DECSET/DECRST 1048, and 1049 use the same complete DEC
        // saved-cursor state as ESC 7/8, including delayed wrap and DECOM.
        for (save, restore) in [
            (b"\x1b[s".as_slice(), b"\x1b[u".as_slice()),
            (b"\x1b[?1048h".as_slice(), b"\x1b[?1048l".as_slice()),
        ] {
            let mut alias = Terminal::new(4, 4);
            processor.process(&mut alias, b"\x1b[2;4r\x1b[?6h\x1b[2;4HX");
            processor.process(&mut alias, save);
            processor.process(&mut alias, b"\x1b[?6l\x1b[H");
            processor.process(&mut alias, restore);
            assert!(alias.mode().contains(TerminalMode::ORIGIN));
            assert_eq!(
                (alias.grid().cursor().row, alias.grid().cursor().col),
                (2, 3)
            );
            assert!(alias.grid().cursor().wrap_pending);
        }

        let mut alias_1049 = Terminal::new(4, 4);
        processor.process(
            &mut alias_1049,
            b"\x1b[2;4r\x1b[?6h\x1b[1\"q\x1b[2;4HX\x1b[?1049h\x1b[?6l\x1b[0\"q\x1b[?1049l",
        );
        assert!(alias_1049.mode().contains(TerminalMode::ORIGIN));
        assert_eq!(alias_1049.character_protection_parameter(), 1);
        assert_eq!(
            (
                alias_1049.grid().cursor().row,
                alias_1049.grid().cursor().col
            ),
            (2, 3)
        );
        assert!(alias_1049.grid().cursor().wrap_pending);
    }

    #[test]
    fn every_rendered_sgr_attribute_sets_and_resets_without_leaking() {
        let mut processor = Processor::new();
        let mut terminal = Terminal::new(32, 2);

        processor.process(
            &mut terminal,
            b"\x1b[1mA\x1b[22mB\x1b[2mC\x1b[22mD\x1b[3mE\x1b[23mF\x1b[5mG\x1b[25mH\x1b[6mI\x1b[25mJ\x1b[7mK\x1b[27mL\x1b[8mM\x1b[28mN\x1b[9mO\x1b[29mP",
        );

        for (col, attr) in [
            (0, CellAttrs::BOLD),
            (2, CellAttrs::DIM),
            (4, CellAttrs::ITALIC),
            (6, CellAttrs::BLINK),
            (8, CellAttrs::RAPID_BLINK),
            (10, CellAttrs::INVERSE),
            (12, CellAttrs::HIDDEN),
            (14, CellAttrs::STRIKE),
        ] {
            assert!(terminal.grid().cell(0, col).unwrap().attrs.contains(attr));
            assert!(
                !terminal
                    .grid()
                    .cell(0, col + 1)
                    .unwrap()
                    .attrs
                    .intersects(attr),
                "attribute {attr:?} leaked past its reset"
            );
        }

        processor.process(
            &mut terminal,
            b"\r\n\x1b[4mA\x1b[4:2mB\x1b[4:3mC\x1b[4:4mD\x1b[4:5mE\x1b[24mF",
        );
        for (col, attr) in [
            (0, CellAttrs::UNDERLINE),
            (1, CellAttrs::DOUBLE_UNDERLINE),
            (2, CellAttrs::CURLY_UNDERLINE),
            (3, CellAttrs::DOTTED_UNDERLINE),
            (4, CellAttrs::DASHED_UNDERLINE),
        ] {
            assert_eq!(terminal.grid().cell(1, col).unwrap().attrs, attr);
        }
        assert!(!terminal.grid().cell(1, 5).unwrap().attrs.has_underline());
    }

    #[test]
    fn claude_keyboard_protocol_reset_does_not_enable_underline() {
        let mut processor = Processor::new();
        let mut terminal = Terminal::new(10, 2);

        // Claude Code emits CSI > 4 m immediately after leaving its alternate
        // screen. This is an xterm modifyOtherKeys reset, not SGR underline.
        processor.process(&mut terminal, b"A\x1b[?1049hB\x1b[?1049l\x1b[>4mC");

        let grid = terminal.grid();
        assert_eq!(grid.cell(0, 0).unwrap().content.char(), Some('A'));
        let c = grid.cell(0, 1).unwrap();
        assert_eq!(c.content.char(), Some('C'));
        assert!(!c.attrs.has_underline());
    }

    #[test]
    fn osc_state_streams_across_chunks_and_releases_after_st() {
        let mut processor = Processor::new();
        let mut terminal = Terminal::new(10, 2);
        processor.process(&mut terminal, b"\x1b]2;chunk");
        processor.process(&mut terminal, b"ed\x1b");
        processor.process(&mut terminal, b"\\x");
        assert_eq!(
            terminal.drain_events(),
            vec![crate::event::TerminalEvent::TitleChanged("chunked".into())]
        );
        assert_eq!(
            terminal.grid().cell(0, 0).unwrap().content.char(),
            Some('x')
        );
    }

    #[test]
    fn osc7_directory_reports_stream_without_leaking_into_terminal_text() {
        let mut processor = Processor::new();
        let mut terminal = Terminal::new(10, 2);
        for chunk in [
            b"\x1b]7;file://local".as_slice(),
            b"host/work/a%20b;semi/%E7",
            b"%95%8C\x1b",
            b"\\x",
        ] {
            processor.process(&mut terminal, chunk);
        }
        let directory = terminal.reported_working_directory().unwrap();
        assert_eq!(directory.host(), Some("localhost"));
        assert_eq!(directory.path(), "/work/a b;semi/界");
        assert_eq!(
            terminal.grid().cell(0, 0).unwrap().content.char(),
            Some('x')
        );

        processor.process(&mut terminal, b"\x9d7;file:///tmp/c1%20cwd\x9c");
        let directory = terminal.reported_working_directory().unwrap();
        assert_eq!(directory.host(), None);
        assert_eq!(directory.path(), "/tmp/c1 cwd");
        assert!(terminal.drain_events().is_empty());
    }

    #[test]
    fn oversized_osc_is_discarded_and_following_text_is_processed() {
        let mut processor = Processor::new();
        let mut terminal = Terminal::new(10, 2);
        let mut input = b"\x1b]52;c;".to_vec();
        input.extend(std::iter::repeat_n(b'A', MAX_OSC_SEQUENCE_BYTES + 1));
        input.extend_from_slice(b"\x07ok");
        processor.process(&mut terminal, &input);
        assert!(terminal.drain_events().is_empty());
        assert_eq!(
            terminal.grid().cell(0, 0).unwrap().content.char(),
            Some('o')
        );
        assert_eq!(
            terminal.grid().cell(0, 1).unwrap().content.char(),
            Some('k')
        );
    }

    #[test]
    fn fish_startup_queries_receive_truthful_bounded_replies() {
        let mut processor = Processor::new();
        let mut terminal = Terminal::new(10, 2);
        for byte in b"\x1b[?u\x1b[>0q\x1bP+q696e646e\x1b\\\x1bP+q71756572792d6f732d6e616d65\x1b\\" {
            processor.process(&mut terminal, &[*byte]);
        }
        let replies: Vec<Vec<u8>> = terminal
            .drain_events()
            .into_iter()
            .filter_map(|event| match event {
                crate::event::TerminalEvent::PtyWrite(bytes) => Some(bytes),
                _ => None,
            })
            .collect();
        assert_eq!(replies[0], b"\x1b[?0u");
        assert_eq!(replies[1], b"\x1bP>|Termeric(1)\x1b\\");
        assert_eq!(replies[2], b"\x1bP1+r696e646e=1b5b257031256453\x1b\\");
        let expected_os = if cfg!(target_os = "linux") {
            b"4c696e7578".as_slice()
        } else if cfg!(target_os = "macos") {
            b"44617277696e".as_slice()
        } else if cfg!(target_os = "windows") {
            b"57696e646f7773".as_slice()
        } else {
            b"556e6b6e6f776e".as_slice()
        };
        let mut os_reply = b"\x1bP1+r71756572792d6f732d6e616d65=".to_vec();
        os_reply.extend_from_slice(expected_os);
        os_reply.extend_from_slice(b"\x1b\\");
        assert_eq!(replies[3], os_reply);
    }

    #[test]
    fn xtgettcap_is_bounded_multi_name_and_matches_the_native_terminfo_contract() {
        let mut processor = Processor::new();
        let mut terminal = Terminal::new(10, 2);

        // One request produces one xterm-compatible response. Processing
        // stops at the first unknown name, so data after it cannot be
        // mistaken for an affirmative capability claim.
        processor.process(
            &mut terminal,
            b"\x1bP+q544e;436f;524742;6b63757531;6b663132;756e6b6e6f776e;6b63756431\x1b\\",
        );
        assert_eq!(
            terminal.drain_events(),
            vec![crate::event::TerminalEvent::PtyWrite(
                b"\x1bP1+r544e=787465726d2d7465726d65726963;436f=323536;524742=38;6b63757531=1b4f41;6b663132=1b5b32347e\x1b\\"
                    .to_vec(),
            )]
        );

        terminal.set_term_name(crate::pty::COMPATIBILITY_TERM);
        processor.process(&mut terminal, b"\x1bP+q544e\x1b\\");
        assert_eq!(
            terminal.drain_events(),
            vec![crate::event::TerminalEvent::PtyWrite(
                b"\x1bP1+r544e=787465726d2d323536636f6c6f72\x1b\\".to_vec(),
            )]
        );

        // An invalid first name is one empty negative response, matching
        // xterm rather than echoing unrecognized data back as if parsed.
        processor.process(&mut terminal, b"\x1bP+q756e6b6e6f776e\x1b\\\x1bP+q1\x1b\\");
        assert_eq!(
            terminal.drain_events(),
            vec![
                crate::event::TerminalEvent::PtyWrite(b"\x1bP0+r\x1b\\".to_vec()),
                crate::event::TerminalEvent::PtyWrite(b"\x1bP0+r\x1b\\".to_vec()),
            ]
        );

        // Both termcap aliases and the terminal-local S8C1T reply preference
        // apply to the same bounded response path.
        processor.process(&mut terminal, b"\x1b G\x1bP+q6b3b;4632\x1b\\");
        assert_eq!(
            terminal.drain_events(),
            vec![crate::event::TerminalEvent::PtyWrite(
                b"\x901+r6b3b=1b5b32317e;4632=1b5b32347e\x9c".to_vec(),
            )]
        );

        let mut bounded_request = b"\x1bP+q".to_vec();
        for index in 0..=64 {
            if index > 0 {
                bounded_request.push(b';');
            }
            bounded_request.extend_from_slice(b"436f");
        }
        bounded_request.extend_from_slice(b"\x1b\\");
        processor.process(&mut terminal, &bounded_request);
        let events = terminal.drain_events();
        let [crate::event::TerminalEvent::PtyWrite(response)] = events.as_slice() else {
            panic!("expected one bounded XTGETTCAP response, got {events:?}");
        };
        assert_eq!(response.iter().filter(|byte| **byte == b'=').count(), 64);
        assert!(response.len() <= 8_192);
        assert_eq!(response.first(), Some(&0x90));
        assert_eq!(response.last(), Some(&0x9c));
    }

    #[test]
    fn xtwinops_reports_exact_text_cell_and_character_geometry_only() {
        let mut processor = Processor::new();
        let mut terminal = Terminal::new(59, 17);
        terminal.set_pixel_geometry(496, 293);

        processor.process(
            &mut terminal,
            b"\x1b[14t\x1b[16t\x1b[18t\x1b[14;2t\x1b[15t\x1b[24t",
        );
        assert_eq!(
            terminal.drain_events(),
            vec![
                crate::event::TerminalEvent::PtyWrite(b"\x1b[4;293;496t".to_vec()),
                crate::event::TerminalEvent::PtyWrite(b"\x1b[6;18;9t".to_vec()),
                crate::event::TerminalEvent::PtyWrite(b"\x1b[8;17;59t".to_vec()),
            ]
        );

        // Reports follow later grid/pixel updates and the same terminal-local
        // S8C1T framing contract as every other core CSI reply.
        terminal.resize(80, 24);
        terminal.set_pixel_geometry(640, 384);
        processor.process(&mut terminal, b"\x1b G\x1b[14t\x1b[16t\x1b[18t");
        assert_eq!(
            terminal.drain_events(),
            vec![
                crate::event::TerminalEvent::PtyWrite(b"\x9b4;384;640t".to_vec()),
                crate::event::TerminalEvent::PtyWrite(b"\x9b6;16;8t".to_vec()),
                crate::event::TerminalEvent::PtyWrite(b"\x9b8;24;80t".to_vec()),
            ]
        );
    }

    #[test]
    fn xterm_title_queries_and_stack_are_global_bounded_and_byte_exact() {
        let mut processor = Processor::new();
        let mut terminal = Terminal::new(8, 2);

        processor.process(
            &mut terminal,
            b"\x1b]1;old-icon\x1b\\\x1b]2;old-window\x1b\\\x1b[22t\
              \x1b]2;new-window\x1b\\\x1b[22;2t\
              \x1b]1;current-icon\x1b\\\x1b]2;current-window\x1b\\",
        );
        assert_eq!(
            terminal.drain_events(),
            vec![
                crate::event::TerminalEvent::TitleChanged("old-window".into()),
                crate::event::TerminalEvent::TitleChanged("new-window".into()),
                crate::event::TerminalEvent::TitleChanged("current-window".into()),
            ]
        );

        processor.process(
            &mut terminal,
            b"\x1b[#S\x1b[20t\x1b[21t\x1b[23t\x1b[20t\x1b[21t\x1b[#S",
        );
        assert_eq!(terminal.application_icon_name(), "old-icon");
        assert_eq!(terminal.application_window_title(), "new-window");
        assert_eq!(
            terminal.drain_events(),
            vec![
                crate::event::TerminalEvent::PtyWrite(b"\x1b[2;10#S".to_vec()),
                crate::event::TerminalEvent::PtyWrite(b"\x1b]Lcurrent-icon\x1b\\".to_vec()),
                crate::event::TerminalEvent::PtyWrite(b"\x1b]lcurrent-window\x1b\\".to_vec()),
                crate::event::TerminalEvent::TitleChanged("new-window".into()),
                crate::event::TerminalEvent::PtyWrite(b"\x1b]Lold-icon\x1b\\".to_vec()),
                crate::event::TerminalEvent::PtyWrite(b"\x1b]lnew-window\x1b\\".to_vec()),
                crate::event::TerminalEvent::PtyWrite(b"\x1b[1;10#S".to_vec()),
            ]
        );

        // A direct slot neither changes the push/pop position nor clears on
        // retrieval. Invalid targets, slots, extra query parameters, and
        // parameterized position reports fail closed.
        processor.process(
            &mut terminal,
            b"\x1b]0;direct\x1b\\\x1b[22;0;3t\x1b]0;other\x1b\\\
              \x1b[23;0;3t\x1b[23;0;3t\x1b[22;3t\x1b[22;0;11t\
              \x1b[20;1t\x1b[1#S\x1b[#S",
        );
        assert_eq!(terminal.application_icon_name(), "direct");
        assert_eq!(terminal.application_window_title(), "direct");
        assert_eq!(
            terminal.drain_events(),
            vec![
                crate::event::TerminalEvent::TitleChanged("direct".into()),
                crate::event::TerminalEvent::TitleChanged("other".into()),
                crate::event::TerminalEvent::TitleChanged("direct".into()),
                crate::event::TerminalEvent::PtyWrite(b"\x1b[1;10#S".to_vec()),
            ]
        );

        // Title metadata and saved slots are terminal-global across alternate
        // screens and are preserved by DECSTR/RIS. Replies follow S8C1T.
        processor.process(
            &mut terminal,
            b"\x1b[?1049h\x1b[!p\x1bc\x1b G\x1b[20t\x1b[21t\x1b[#S",
        );
        assert_eq!(terminal.application_icon_name(), "direct");
        assert_eq!(terminal.application_window_title(), "direct");
        assert_eq!(
            terminal.drain_events(),
            vec![
                crate::event::TerminalEvent::PtyWrite(b"\x9dLdirect\x9c".to_vec()),
                crate::event::TerminalEvent::PtyWrite(b"\x9dldirect\x9c".to_vec()),
                crate::event::TerminalEvent::PtyWrite(b"\x9b1;10#S".to_vec()),
            ]
        );
    }

    #[test]
    fn osc99_notifications_are_chunked_queryable_and_reset_bounded() {
        let mut processor = Processor::new();
        let mut terminal = Terminal::new(8, 2);

        processor.process(
            &mut terminal,
            b"\x1b]99;;simple \xf0\x9f\x8c\xb1\x1b\\\
              \x1b]99;i=first:d=0;build\x1b\\\
              \x1b]99;i=second:d=0;other\x1b\\\
              \x1b]99;i=first:p=body:e=1;Y29tcGxldGU=\x1b\\\
              \x1b]99;i=second; result\x1b\\",
        );
        assert_eq!(
            terminal.drain_events(),
            vec![
                crate::event::TerminalEvent::Notification(crate::event::NotificationRequest {
                    title: Some("simple 🌱".into()),
                    body: String::new(),
                    ..crate::event::NotificationRequest::default()
                }),
                crate::event::TerminalEvent::Notification(crate::event::NotificationRequest {
                    title: Some("build".into()),
                    body: "complete".into(),
                    ..crate::event::NotificationRequest::default()
                }),
                crate::event::TerminalEvent::Notification(crate::event::NotificationRequest {
                    title: Some("other result".into()),
                    body: String::new(),
                    ..crate::event::NotificationRequest::default()
                }),
            ]
        );

        processor.process(
            &mut terminal,
            b"\x1b]99;i=query-7:p=?;\x1b\\\
              \x1b G\x1b]99;i=query-8:p=?;\x1b\\\
              \x1b]99;i=bad=:p=?;\x1b\\\
              \x1b]99;c=1;ignored\x1b\\\
              \x1b]99;o=unfocused;ignored\x1b\\\
              \x1b]99;e=1;%%%\x1b\\\
              \x1b]99;p=future;ignored\x1b\\",
        );
        #[cfg(target_os = "linux")]
        let expected_query_events = vec![
            crate::event::TerminalEvent::PtyWrite(
                b"\x1b]99;i=query-7:p=?;p=title,body:o=always:u=0,1,2\x1b\\".to_vec(),
            ),
            crate::event::TerminalEvent::PtyWrite(
                b"\x9d99;i=query-8:p=?;p=title,body:o=always:u=0,1,2\x9c".to_vec(),
            ),
        ];
        #[cfg(not(target_os = "linux"))]
        let expected_query_events = vec![
            crate::event::TerminalEvent::PtyWrite(
                b"\x1b]99;i=query-7:p=?;p=title,body:o=always\x1b\\".to_vec(),
            ),
            crate::event::TerminalEvent::PtyWrite(
                b"\x9d99;i=query-8:p=?;p=title,body:o=always\x9c".to_vec(),
            ),
        ];
        assert_eq!(terminal.drain_events(), expected_query_events);

        // Incomplete state is global across a screen switch and DECSTR, but a
        // full RIS retires it so stale application text cannot be completed.
        processor.process(
            &mut terminal,
            b"\x1b]99;i=keep:d=0;before-\x1b\\\x1b[?1049h\x1b[!p\
              \x1b]99;i=keep;after\x1b\\\
              \x1b]99;i=clear:d=0;stale-\x1b\\\x1bc\
              \x1b]99;i=clear;fresh\x1b\\",
        );
        assert_eq!(
            terminal.drain_events(),
            vec![
                crate::event::TerminalEvent::Notification(crate::event::NotificationRequest {
                    title: Some("before-after".into()),
                    body: String::new(),
                    ..crate::event::NotificationRequest::default()
                }),
                crate::event::TerminalEvent::Notification(crate::event::NotificationRequest {
                    title: Some("fresh".into()),
                    body: String::new(),
                    ..crate::event::NotificationRequest::default()
                }),
            ]
        );
    }

    #[test]
    fn kitty_keyboard_negotiation_is_screen_local_stack_based_and_resettable() {
        let mut processor = Processor::new();
        let mut terminal = Terminal::new(10, 2);

        processor.process(&mut terminal, b"\x1b[=1u\x1b[?u");
        assert_eq!(
            terminal.drain_events(),
            vec![crate::event::TerminalEvent::PtyWrite(b"\x1b[?1u".to_vec())]
        );

        // Alternate-screen negotiation cannot leak into the primary screen.
        processor.process(&mut terminal, b"\x1b[?1049h\x1b[?u\x1b[>2u\x1b[?u");
        assert_eq!(
            terminal.drain_events(),
            vec![
                crate::event::TerminalEvent::PtyWrite(b"\x1b[?0u".to_vec()),
                crate::event::TerminalEvent::PtyWrite(b"\x1b[?2u".to_vec()),
            ]
        );
        processor.process(&mut terminal, b"\x1b[<u\x1b[?u\x1b[?1049l\x1b[?u");
        assert_eq!(
            terminal.drain_events(),
            vec![
                crate::event::TerminalEvent::PtyWrite(b"\x1b[?0u".to_vec()),
                crate::event::TerminalEvent::PtyWrite(b"\x1b[?1u".to_vec()),
            ]
        );

        // Add/remove modes operate on known bits only; RIS clears both stacks.
        processor.process(
            &mut terminal,
            b"\x1b[=2;2u\x1b[=1;3u\x1b[=1:1u\x1b[?u\x1bc\x1b[?u\x1b[?1049h\x1b[?u",
        );
        assert_eq!(
            terminal.drain_events(),
            vec![
                crate::event::TerminalEvent::PtyWrite(b"\x1b[?2u".to_vec()),
                crate::event::TerminalEvent::PtyWrite(b"\x1b[?0u".to_vec()),
                crate::event::TerminalEvent::PtyWrite(b"\x1b[?0u".to_vec()),
            ]
        );
    }

    #[test]
    fn osc22_pointer_shapes_are_bounded_queryable_screen_local_and_resettable() {
        use crate::event::TerminalEvent;
        use crate::term::{POINTER_SHAPE_STACK_DEPTH, PointerShape};

        let mut processor = Processor::new();
        let mut terminal = Terminal::new(10, 2);

        processor.process(
            &mut terminal,
            b"\x1b]22;text\x1b\\\x1b]22;>pointer,wait\x1b\\\x1b]22;?__current__,text,no-such-name\x1b\\",
        );
        assert_eq!(terminal.pointer_shape(), Some(PointerShape::Wait));
        assert_eq!(
            terminal.drain_events(),
            vec![
                TerminalEvent::PointerShapeChanged,
                TerminalEvent::PtyWrite(b"\x1b]22;wait,1,0\x1b\\".to_vec()),
            ]
        );

        // Set replaces only the current entry, so pop restores the entry
        // below it. Repeating a value emits no redundant native event.
        processor.process(
            &mut terminal,
            b"\x1b]22;=progress\x1b\\\x1b]22;progress\x1b\\\x1b]22;<ignored,names\x1b\\\x1b]22;?__current__\x1b\\",
        );
        assert_eq!(terminal.pointer_shape(), Some(PointerShape::Pointer));
        assert_eq!(
            terminal.drain_events(),
            vec![
                TerminalEvent::PointerShapeChanged,
                TerminalEvent::PointerShapeChanged,
                TerminalEvent::PtyWrite(b"\x1b]22;pointer\x1b\\".to_vec()),
            ]
        );

        // Invalid set/push/query operations are rejected atomically.
        let mut invalid = b"\x1b]22;>text,no-such-name\x1b\\\x1b]22;Pointer\x1b\\\x1b]22;?text,,wait\x1b\\\x1b]22;>".to_vec();
        invalid.extend(std::iter::repeat_n(
            b'x',
            crate::term::MAX_POINTER_SHAPE_PAYLOAD_BYTES + 1,
        ));
        invalid.extend_from_slice(b"\x1b\\");
        processor.process(&mut terminal, &invalid);
        assert_eq!(terminal.pointer_shape(), Some(PointerShape::Pointer));
        assert!(terminal.drain_events().is_empty());

        // Overflow evicts the oldest entry from the fixed 16-slot stack.
        processor.process(&mut terminal, b"\x1b]22;\x1b\\");
        assert_eq!(
            terminal.drain_events(),
            vec![TerminalEvent::PointerShapeChanged]
        );
        let names = PointerShape::ALL[..=POINTER_SHAPE_STACK_DEPTH]
            .iter()
            .map(|shape| shape.name())
            .collect::<Vec<_>>()
            .join(",");
        terminal.process(format!("\x1b]22;>{names}\x1b\\\x1b]22;<\x1b\\").as_bytes());
        assert_eq!(
            terminal.pointer_shape(),
            Some(PointerShape::ALL[POINTER_SHAPE_STACK_DEPTH - 1])
        );
        assert_eq!(
            terminal.drain_events(),
            vec![
                TerminalEvent::PointerShapeChanged,
                TerminalEvent::PointerShapeChanged,
            ]
        );

        // Primary and alternate stacks remain independent across switches.
        terminal.process(b"\x1b]22;text\x1b\\");
        terminal.drain_events();
        terminal.process(
            b"\x1b[?1049h\x1b]22;?__current__\x1b\\\x1b]22;wait\x1b\\\x1b[?1049l\x1b]22;?__current__\x1b\\",
        );
        assert_eq!(terminal.pointer_shape(), Some(PointerShape::Text));
        assert_eq!(
            terminal.drain_events(),
            vec![
                TerminalEvent::PtyWrite(b"\x1b]22;0\x1b\\".to_vec()),
                TerminalEvent::PointerShapeChanged,
                TerminalEvent::PointerShapeChanged,
                TerminalEvent::PtyWrite(b"\x1b]22;text\x1b\\".to_vec()),
            ]
        );

        // DECSTR is a terminal reset for this protocol and clears both
        // stacks without changing the active screen. RIS does the same and
        // returns to the primary screen.
        terminal.process(
            b"\x1b[?1049h\x1b[!p\x1b]22;?__current__\x1b\\\x1b[?1049l\x1b]22;?__current__\x1b\\",
        );
        assert_eq!(
            terminal.drain_events(),
            vec![
                TerminalEvent::PointerShapeChanged,
                TerminalEvent::PointerShapeChanged,
                TerminalEvent::PtyWrite(b"\x1b]22;0\x1b\\".to_vec()),
                TerminalEvent::PtyWrite(b"\x1b]22;0\x1b\\".to_vec()),
            ]
        );
        terminal.process(b"\x1b]22;zoom-in\x1b\\\x1bc\x1b]22;?__current__\x1b\\");
        assert_eq!(
            terminal.drain_events(),
            vec![
                TerminalEvent::PointerShapeChanged,
                TerminalEvent::PointerShapeChanged,
                TerminalEvent::PtyWrite(b"\x1b]22;0\x1b\\".to_vec()),
            ]
        );

        // Special policy queries and ordinary support queries share the
        // terminal-local S8C1T framing contract.
        terminal.process(b"\x1b G\x1b]22;?__default__,__grabbed__,pointer,missing\x1b\\");
        assert_eq!(
            terminal.drain_events(),
            vec![TerminalEvent::PtyWrite(
                b"\x9d22;text,default,1,0\x9c".to_vec()
            )]
        );

        terminal.process(b"\x1b[?1000h");
        assert_eq!(terminal.effective_pointer_shape(), PointerShape::Default);
        assert_eq!(
            terminal.drain_events(),
            vec![TerminalEvent::PointerShapeChanged]
        );
        terminal.process(b"\x1b[?1000l");
        assert_eq!(terminal.effective_pointer_shape(), PointerShape::Text);
        assert_eq!(
            terminal.drain_events(),
            vec![TerminalEvent::PointerShapeChanged]
        );
    }

    #[test]
    fn xterm_modify_other_keys_is_bounded_queryable_and_resettable() {
        let mut processor = Processor::new();
        let mut terminal = Terminal::new(10, 2);

        processor.process(&mut terminal, b"\x1b[?4m\x1b[>4;2m\x1b[?4m");
        assert_eq!(terminal.xterm_modify_other_keys().level(), 2);
        assert_eq!(terminal.xterm_modify_other_keys().modifier_mask(), 0);
        assert_eq!(
            terminal.drain_events(),
            vec![
                crate::event::TerminalEvent::PtyWrite(b"\x1b[>4;0m".to_vec()),
                crate::event::TerminalEvent::PtyWrite(b"\x1b[>4;2m".to_vec()),
            ]
        );

        // The optional modifier mask is bounded, malformed levels are
        // atomic, and screen-buffer switches do not change terminal-local
        // xterm resource state.
        processor.process(
            &mut terminal,
            b"\x1b[>4;2:2m\x1b[>4:1m\x1b[>4;4m\x1b[?1049h\x1b[?4m",
        );
        assert_eq!(terminal.xterm_modify_other_keys().level(), 2);
        assert_eq!(terminal.xterm_modify_other_keys().modifier_mask(), 1);
        assert_eq!(
            terminal.drain_events(),
            vec![crate::event::TerminalEvent::PtyWrite(
                b"\x1b[>4;2m".to_vec()
            )]
        );

        // Query replies follow S8C1T; both xterm reset forms and DECSTR
        // restore the disabled startup profile without affecting text.
        processor.process(
            &mut terminal,
            b"\x1b G\x1b[?4m\x1b[>4n\x1b[>4;3m\x1b[!p\x1b[?4mX",
        );
        assert_eq!(terminal.xterm_modify_other_keys().level(), 0);
        assert_eq!(
            terminal.grid().cell(0, 0).unwrap().content.char(),
            Some('X')
        );
        assert_eq!(
            terminal.drain_events(),
            vec![
                crate::event::TerminalEvent::PtyWrite(b"\x9b>4;2m".to_vec()),
                crate::event::TerminalEvent::PtyWrite(b"\x9b>4;0m".to_vec()),
            ]
        );
    }

    #[test]
    fn xterm_keyboard_resource_zero_is_exact_queryable_and_resettable() {
        use crate::term::{XtermKeyFormat, XtermKeyMode, XtermKeyboardResource};

        let mut processor = Processor::new();
        let mut terminal = Terminal::new(10, 2);

        processor.process(
            &mut terminal,
            b"\x1b[?0m\x1b[?0g\x1b[>0;15m\x1b[>0;42f\x1b[?0m\x1b[?0g",
        );
        assert_eq!(
            terminal.xterm_keyboard_resource(),
            XtermKeyboardResource::new(15, 42)
        );
        assert_eq!(
            terminal.drain_events(),
            vec![
                crate::event::TerminalEvent::PtyWrite(b"\x1b[>0;0m".to_vec()),
                crate::event::TerminalEvent::PtyWrite(b"\x1b[>0;0f".to_vec()),
                crate::event::TerminalEvent::PtyWrite(b"\x1b[>0;15m".to_vec()),
                crate::event::TerminalEvent::PtyWrite(b"\x1b[>0;42f".to_vec()),
            ]
        );

        // Xterm treats both resource-zero values as complete integer resource
        // state. Subparameters, plural forms, and unknown resource 8 remain
        // invalid; switching screens does not fork the terminal-local state.
        processor.process(
            &mut terminal,
            b"\x1b[>0:1m\x1b[>0;7;1m\x1b[>0:1f\x1b[>0;7;1f\x1b[>8;1m\x1b[>8;1f\x1b[?1049h\x1b[?0m\x1b[?0g",
        );
        assert_eq!(
            terminal.xterm_keyboard_resource(),
            XtermKeyboardResource::new(15, 42)
        );
        assert_eq!(
            terminal.drain_events(),
            vec![
                crate::event::TerminalEvent::PtyWrite(b"\x1b[>0;15m".to_vec()),
                crate::event::TerminalEvent::PtyWrite(b"\x1b[>0;42f".to_vec()),
            ]
        );

        // The disable spelling stores xterm's signed -1 through the unsigned
        // CSI path. Resource-only reset does not cross either the mode/format
        // family boundary or another resource; parameterless reset does.
        processor.process(&mut terminal, b"\x1b[>0n\x1b[?0m");
        assert_eq!(terminal.xterm_keyboard_resource().modify(), u16::MAX);
        assert_eq!(
            terminal.drain_events(),
            vec![crate::event::TerminalEvent::PtyWrite(
                b"\x1b[>0;65535m".to_vec()
            )]
        );

        processor.process(
            &mut terminal,
            b"\x1b[>0;9m\x1b[>0;17f\x1b[>1;4m\x1b[>1;1f\x1b[>0m",
        );
        assert_eq!(terminal.xterm_keyboard_resource().modify(), 0);
        assert_eq!(terminal.xterm_keyboard_resource().format(), 17);
        assert_eq!(terminal.xterm_cursor_keys().mode(), XtermKeyMode::Extended);
        assert_eq!(terminal.xterm_cursor_keys().format(), XtermKeyFormat::CsiU);
        processor.process(&mut terminal, b"\x1b[>0f");
        assert_eq!(
            terminal.xterm_keyboard_resource(),
            XtermKeyboardResource::default()
        );
        assert_eq!(terminal.xterm_cursor_keys().mode(), XtermKeyMode::Extended);
        assert_eq!(terminal.xterm_cursor_keys().format(), XtermKeyFormat::CsiU);

        processor.process(&mut terminal, b"\x1b[>0;9m\x1b[>0;17f\x1b[>m\x1b[>f");
        assert_eq!(
            terminal.xterm_keyboard_resource(),
            XtermKeyboardResource::default()
        );
        assert_eq!(terminal.xterm_cursor_keys().mode(), XtermKeyMode::Parameter);
        assert_eq!(
            terminal.xterm_cursor_keys().format(),
            XtermKeyFormat::FunctionKey27
        );

        // DECRQSS returns restorable bodies, S8C1T controls direct replies,
        // and both DECSTR and RIS restore the startup values without leaking
        // protocol bytes into terminal text.
        processor.process(
            &mut terminal,
            b"\x1b[>0;11m\x1b[>0;99f\x1bP$q>0m\x1b\\\x1bP$q>0f\x1b\\\x1b G\x1b[?0m\x1b[!p\x1b[?0mX",
        );
        assert_eq!(
            terminal.xterm_keyboard_resource(),
            XtermKeyboardResource::default()
        );
        assert_eq!(
            terminal.grid().cell(0, 0).unwrap().content.char(),
            Some('X')
        );
        assert_eq!(
            terminal.drain_events(),
            vec![
                crate::event::TerminalEvent::PtyWrite(b"\x1bP1$r>0;11m\x1b\\".to_vec()),
                crate::event::TerminalEvent::PtyWrite(b"\x1bP1$r>0;99f\x1b\\".to_vec()),
                crate::event::TerminalEvent::PtyWrite(b"\x9b>0;11m".to_vec()),
                crate::event::TerminalEvent::PtyWrite(b"\x9b>0;0m".to_vec()),
            ]
        );

        processor.process(&mut terminal, b"\x1b[>0;15m\x1b[>0;42f\x1bc");
        assert_eq!(
            terminal.xterm_keyboard_resource(),
            XtermKeyboardResource::default()
        );
    }

    #[test]
    fn xterm_cursor_key_resources_are_bounded_queryable_and_resettable() {
        use crate::term::{XtermKeyFormat, XtermKeyMode};

        let mut processor = Processor::new();
        let mut terminal = Terminal::new(10, 2);

        processor.process(
            &mut terminal,
            b"\x1b[?1m\x1b[?1g\x1b[>1;4m\x1b[>1;1f\x1b[?1m\x1b[?1g",
        );
        assert_eq!(terminal.xterm_cursor_keys().mode(), XtermKeyMode::Extended);
        assert_eq!(terminal.xterm_cursor_keys().format(), XtermKeyFormat::CsiU);
        assert_eq!(
            terminal.drain_events(),
            vec![
                crate::event::TerminalEvent::PtyWrite(b"\x1b[>1;2m".to_vec()),
                crate::event::TerminalEvent::PtyWrite(b"\x1b[>1;0f".to_vec()),
                crate::event::TerminalEvent::PtyWrite(b"\x1b[>1;4m".to_vec()),
                crate::event::TerminalEvent::PtyWrite(b"\x1b[>1;1f".to_vec()),
            ]
        );

        // Invalid values, subparameters, and plural shapes are atomic. The
        // state is terminal-local rather than screen-local.
        processor.process(
            &mut terminal,
            b"\x1b[>1;5m\x1b[>1;2f\x1b[>1:0m\x1b[>1;4;0m\x1b[?1049h\x1b[?1m",
        );
        assert_eq!(terminal.xterm_cursor_keys().mode(), XtermKeyMode::Extended);
        assert_eq!(terminal.xterm_cursor_keys().format(), XtermKeyFormat::CsiU);
        assert_eq!(
            terminal.drain_events(),
            vec![crate::event::TerminalEvent::PtyWrite(
                b"\x1b[>1;4m".to_vec()
            )]
        );

        // Disable uses xterm's unsigned -1 report. Parameterless modifier and
        // format resets cover all supported resources, while explicit
        // resource zero resets only its own independent state.
        processor.process(
            &mut terminal,
            b"\x1b[>1n\x1b[?1m\x1b[>4;2m\x1b[>4;1f\x1b[>m\x1b[>0m",
        );
        assert_eq!(terminal.xterm_cursor_keys().mode(), XtermKeyMode::Parameter);
        assert_eq!(terminal.xterm_cursor_keys().format(), XtermKeyFormat::CsiU);
        assert_eq!(terminal.xterm_modify_other_keys().level(), 0);
        assert_eq!(
            terminal.xterm_modify_other_keys().format(),
            XtermKeyFormat::CsiU
        );
        assert_eq!(
            terminal.drain_events(),
            vec![crate::event::TerminalEvent::PtyWrite(
                b"\x1b[>1;65535m".to_vec()
            )]
        );

        processor.process(&mut terminal, b"\x1b[>f\x1b[>0f");
        assert_eq!(
            terminal.xterm_modify_other_keys().format(),
            XtermKeyFormat::FunctionKey27
        );
        assert_eq!(
            terminal.xterm_cursor_keys().format(),
            XtermKeyFormat::FunctionKey27
        );

        // DECRQSS returns the precise negotiated bodies. S8C1T applies to
        // direct queries, and DECSTR/RIS restore startup state.
        processor.process(
            &mut terminal,
            b"\x1b[>1;3m\x1b[>1;1f\x1bP$q>1m\x1b\\\x1bP$q>1f\x1b\\\x1b G\x1b[?1m\x1b[!p\x1b[?1mX",
        );
        assert_eq!(
            terminal.xterm_cursor_keys(),
            crate::term::XtermCursorKeys::default()
        );
        assert_eq!(
            terminal.grid().cell(0, 0).unwrap().content.char(),
            Some('X')
        );
        assert_eq!(
            terminal.drain_events(),
            vec![
                crate::event::TerminalEvent::PtyWrite(b"\x1bP1$r>1;3m\x1b\\".to_vec()),
                crate::event::TerminalEvent::PtyWrite(b"\x1bP1$r>1;1f\x1b\\".to_vec()),
                crate::event::TerminalEvent::PtyWrite(b"\x9b>1;3m".to_vec()),
                crate::event::TerminalEvent::PtyWrite(b"\x9b>1;2m".to_vec()),
            ]
        );

        processor.process(&mut terminal, b"\x1b[>1;4m\x1b[>1;1f\x1bc\x1b[?1m\x1b[?1g");
        assert_eq!(
            terminal.xterm_cursor_keys(),
            crate::term::XtermCursorKeys::default()
        );
        assert_eq!(
            terminal.drain_events(),
            vec![
                crate::event::TerminalEvent::PtyWrite(b"\x1b[>1;2m".to_vec()),
                crate::event::TerminalEvent::PtyWrite(b"\x1b[>1;0f".to_vec()),
            ]
        );
    }

    #[test]
    fn xterm_function_key_resources_are_bounded_queryable_and_resettable() {
        use crate::term::{XtermFunctionKeys, XtermKeyFormat, XtermKeyMode};

        let mut processor = Processor::new();
        let mut terminal = Terminal::new(10, 2);

        processor.process(
            &mut terminal,
            b"\x1b[?2m\x1b[?2g\x1b[>2;4m\x1b[>2;1f\x1b[?2m\x1b[?2g",
        );
        assert_eq!(
            terminal.xterm_function_keys().mode(),
            XtermKeyMode::Extended
        );
        assert_eq!(
            terminal.xterm_function_keys().format(),
            XtermKeyFormat::CsiU
        );
        assert_eq!(
            terminal.drain_events(),
            vec![
                crate::event::TerminalEvent::PtyWrite(b"\x1b[>2;2m".to_vec()),
                crate::event::TerminalEvent::PtyWrite(b"\x1b[>2;0f".to_vec()),
                crate::event::TerminalEvent::PtyWrite(b"\x1b[>2;4m".to_vec()),
                crate::event::TerminalEvent::PtyWrite(b"\x1b[>2;1f".to_vec()),
            ]
        );

        processor.process(
            &mut terminal,
            b"\x1b[>2;5m\x1b[>2;2f\x1b[>2:0m\x1b[>2;4;0m\x1b[?1049h\x1b[?2m",
        );
        assert_eq!(
            terminal.xterm_function_keys().mode(),
            XtermKeyMode::Extended
        );
        assert_eq!(
            terminal.xterm_function_keys().format(),
            XtermKeyFormat::CsiU
        );
        assert_eq!(
            terminal.drain_events(),
            vec![crate::event::TerminalEvent::PtyWrite(
                b"\x1b[>2;4m".to_vec()
            )]
        );

        processor.process(&mut terminal, b"\x1b[>2n\x1b[?2m");
        assert_eq!(
            terminal.xterm_function_keys().mode(),
            XtermKeyMode::Disabled
        );
        assert_eq!(
            terminal.drain_events(),
            vec![crate::event::TerminalEvent::PtyWrite(
                b"\x1b[>2;65535m".to_vec()
            )]
        );

        processor.process(&mut terminal, b"\x1b[>2;4m\x1b[>0n\x1b[?2m\x1b[>n\x1b[?2m");
        assert_eq!(
            terminal.xterm_function_keys().mode(),
            XtermKeyMode::Disabled
        );
        assert_eq!(
            terminal.drain_events(),
            vec![
                crate::event::TerminalEvent::PtyWrite(b"\x1b[>2;4m".to_vec()),
                crate::event::TerminalEvent::PtyWrite(b"\x1b[>2;65535m".to_vec()),
            ]
        );

        // Parameterless modifier and formatter resets cover every supported
        // resource without crossing the modifier/format family boundary.
        processor.process(
            &mut terminal,
            b"\x1b[>1;3m\x1b[>1;1f\x1b[>2;4m\x1b[>2;1f\x1b[>4;2m\x1b[>4;1f\x1b[>65535m\x1b[>>m",
        );
        assert_eq!(terminal.xterm_cursor_keys().mode(), XtermKeyMode::Private);
        assert_eq!(
            terminal.xterm_function_keys().mode(),
            XtermKeyMode::Extended
        );
        assert_eq!(terminal.xterm_modify_other_keys().level(), 2);

        processor.process(&mut terminal, b"\x1b[>m\x1b[>0m");
        assert_eq!(terminal.xterm_cursor_keys().mode(), XtermKeyMode::Parameter);
        assert_eq!(
            terminal.xterm_function_keys().mode(),
            XtermKeyMode::Parameter
        );
        assert_eq!(terminal.xterm_modify_other_keys().level(), 0);
        assert_eq!(terminal.xterm_cursor_keys().format(), XtermKeyFormat::CsiU);
        assert_eq!(
            terminal.xterm_function_keys().format(),
            XtermKeyFormat::CsiU
        );
        assert_eq!(
            terminal.xterm_modify_other_keys().format(),
            XtermKeyFormat::CsiU
        );

        processor.process(&mut terminal, b"\x1b[>65535f");
        assert_eq!(terminal.xterm_cursor_keys().format(), XtermKeyFormat::CsiU);
        assert_eq!(
            terminal.xterm_function_keys().format(),
            XtermKeyFormat::CsiU
        );
        assert_eq!(
            terminal.xterm_modify_other_keys().format(),
            XtermKeyFormat::CsiU
        );

        processor.process(&mut terminal, b"\x1b[>f\x1b[>0f");
        assert_eq!(
            terminal.xterm_cursor_keys().format(),
            XtermKeyFormat::FunctionKey27
        );
        assert_eq!(
            terminal.xterm_function_keys().format(),
            XtermKeyFormat::FunctionKey27
        );
        assert_eq!(
            terminal.xterm_modify_other_keys().format(),
            XtermKeyFormat::FunctionKey27
        );

        processor.process(
            &mut terminal,
            b"\x1b[>2;3m\x1b[>2;1f\x1bP$q>2m\x1b\\\x1bP$q>2f\x1b\\\x1b G\x1b[?2m\x1b[!p\x1b[?2mX",
        );
        assert_eq!(terminal.xterm_function_keys(), XtermFunctionKeys::default());
        assert_eq!(
            terminal.grid().cell(0, 0).unwrap().content.char(),
            Some('X')
        );
        assert_eq!(
            terminal.drain_events(),
            vec![
                crate::event::TerminalEvent::PtyWrite(b"\x1bP1$r>2;3m\x1b\\".to_vec()),
                crate::event::TerminalEvent::PtyWrite(b"\x1bP1$r>2;1f\x1b\\".to_vec()),
                crate::event::TerminalEvent::PtyWrite(b"\x9b>2;3m".to_vec()),
                crate::event::TerminalEvent::PtyWrite(b"\x9b>2;2m".to_vec()),
            ]
        );

        processor.process(&mut terminal, b"\x1b[>2;4m\x1b[>2;1f\x1bc\x1b[?2m\x1b[?2g");
        assert_eq!(terminal.xterm_function_keys(), XtermFunctionKeys::default());
        assert_eq!(
            terminal.drain_events(),
            vec![
                crate::event::TerminalEvent::PtyWrite(b"\x1b[>2;2m".to_vec()),
                crate::event::TerminalEvent::PtyWrite(b"\x1b[>2;0f".to_vec()),
            ]
        );
    }

    #[test]
    fn xterm_keypad_key_resources_are_bounded_queryable_and_resettable() {
        use crate::term::{XtermKeyFormat, XtermKeyMode, XtermKeypadKeys};

        let mut processor = Processor::new();
        let mut terminal = Terminal::new(10, 2);

        processor.process(
            &mut terminal,
            b"\x1b[?3m\x1b[?3g\x1b[>3;4m\x1b[>3;1f\x1b[?3m\x1b[?3g",
        );
        assert_eq!(terminal.xterm_keypad_keys().mode(), XtermKeyMode::Extended);
        assert_eq!(terminal.xterm_keypad_keys().format(), XtermKeyFormat::CsiU);
        assert_eq!(
            terminal.drain_events(),
            vec![
                crate::event::TerminalEvent::PtyWrite(b"\x1b[>3;0m".to_vec()),
                crate::event::TerminalEvent::PtyWrite(b"\x1b[>3;0f".to_vec()),
                crate::event::TerminalEvent::PtyWrite(b"\x1b[>3;4m".to_vec()),
                crate::event::TerminalEvent::PtyWrite(b"\x1b[>3;1f".to_vec()),
            ]
        );

        processor.process(
            &mut terminal,
            b"\x1b[>3;5m\x1b[>3;2f\x1b[>3:0m\x1b[>3;4;0m\x1b[?1049h\x1b[?3m",
        );
        assert_eq!(terminal.xterm_keypad_keys().mode(), XtermKeyMode::Extended);
        assert_eq!(terminal.xterm_keypad_keys().format(), XtermKeyFormat::CsiU);
        assert_eq!(
            terminal.drain_events(),
            vec![crate::event::TerminalEvent::PtyWrite(
                b"\x1b[>3;4m".to_vec()
            )]
        );

        processor.process(&mut terminal, b"\x1b[>3n\x1b[?3m");
        assert_eq!(terminal.xterm_keypad_keys().mode(), XtermKeyMode::Disabled);
        assert_eq!(
            terminal.drain_events(),
            vec![crate::event::TerminalEvent::PtyWrite(
                b"\x1b[>3;65535m".to_vec()
            )]
        );

        processor.process(
            &mut terminal,
            b"\x1b[>3;4m\x1b[>3;1f\x1b[>65535m\x1b[>>m\x1b[>65535f\x1b[>>f",
        );
        assert_eq!(terminal.xterm_keypad_keys().mode(), XtermKeyMode::Extended);
        assert_eq!(terminal.xterm_keypad_keys().format(), XtermKeyFormat::CsiU);

        processor.process(&mut terminal, b"\x1b[>m");
        assert_eq!(terminal.xterm_keypad_keys().mode(), XtermKeyMode::Original);
        assert_eq!(terminal.xterm_keypad_keys().format(), XtermKeyFormat::CsiU);

        processor.process(&mut terminal, b"\x1b[>3;4m\x1b[>f");
        assert_eq!(terminal.xterm_keypad_keys().mode(), XtermKeyMode::Extended);
        assert_eq!(
            terminal.xterm_keypad_keys().format(),
            XtermKeyFormat::FunctionKey27
        );

        processor.process(&mut terminal, b"\x1b[>3m\x1b[>3f");
        assert_eq!(terminal.xterm_keypad_keys(), XtermKeypadKeys::default());

        processor.process(
            &mut terminal,
            b"\x1b[>3;4m\x1b[>3;1f\x1bP$q>3m\x1b\\\x1bP$q>3f\x1b\\\x1b G\x1b[?3m\x1b[!p\x1b[?3mX",
        );
        assert_eq!(terminal.xterm_keypad_keys(), XtermKeypadKeys::default());
        assert_eq!(
            terminal.grid().cell(0, 0).unwrap().content.char(),
            Some('X')
        );
        assert_eq!(
            terminal.drain_events(),
            vec![
                crate::event::TerminalEvent::PtyWrite(b"\x1bP1$r>3;4m\x1b\\".to_vec()),
                crate::event::TerminalEvent::PtyWrite(b"\x1bP1$r>3;1f\x1b\\".to_vec()),
                crate::event::TerminalEvent::PtyWrite(b"\x9b>3;4m".to_vec()),
                crate::event::TerminalEvent::PtyWrite(b"\x9b>3;0m".to_vec()),
            ]
        );

        processor.process(&mut terminal, b"\x1b[>3;4m\x1b[>3;1f\x1bc\x1b[?3m\x1b[?3g");
        assert_eq!(terminal.xterm_keypad_keys(), XtermKeypadKeys::default());
        assert_eq!(
            terminal.drain_events(),
            vec![
                crate::event::TerminalEvent::PtyWrite(b"\x1b[>3;0m".to_vec()),
                crate::event::TerminalEvent::PtyWrite(b"\x1b[>3;0f".to_vec()),
            ]
        );
    }

    #[test]
    fn xterm_modifier_key_resources_are_bounded_queryable_and_resettable() {
        use crate::term::{XtermKeyFormat, XtermKeyMode, XtermModifierKeys};

        let mut processor = Processor::new();
        let mut terminal = Terminal::new(10, 2);

        processor.process(
            &mut terminal,
            b"\x1b[?6m\x1b[?6g\x1b[>6;4m\x1b[>6;1f\x1b[?6m\x1b[?6g",
        );
        assert_eq!(
            terminal.xterm_modifier_keys().mode(),
            XtermKeyMode::Extended
        );
        assert_eq!(
            terminal.xterm_modifier_keys().format(),
            XtermKeyFormat::CsiU
        );
        assert_eq!(
            terminal.drain_events(),
            vec![
                crate::event::TerminalEvent::PtyWrite(b"\x1b[>6;0m".to_vec()),
                crate::event::TerminalEvent::PtyWrite(b"\x1b[>6;0f".to_vec()),
                crate::event::TerminalEvent::PtyWrite(b"\x1b[>6;4m".to_vec()),
                crate::event::TerminalEvent::PtyWrite(b"\x1b[>6;1f".to_vec()),
            ]
        );

        // Values outside xterm's -1/0..4 mode domain, invalid formats,
        // subparameters, plural shapes, and unknown resource 8 are atomic.
        // The valid state remains terminal-local across the alternate screen.
        processor.process(
            &mut terminal,
            b"\x1b[>6;5m\x1b[>6;2f\x1b[>6:0m\x1b[>6;4;0m\x1b[>8;4m\x1b[?8m\x1b[?1049h\x1b[?6m",
        );
        assert_eq!(
            terminal.xterm_modifier_keys().mode(),
            XtermKeyMode::Extended
        );
        assert_eq!(
            terminal.xterm_modifier_keys().format(),
            XtermKeyFormat::CsiU
        );
        assert_eq!(
            terminal.drain_events(),
            vec![crate::event::TerminalEvent::PtyWrite(
                b"\x1b[>6;4m".to_vec()
            )]
        );

        processor.process(&mut terminal, b"\x1b[>6n\x1b[?6m");
        assert_eq!(
            terminal.xterm_modifier_keys().mode(),
            XtermKeyMode::Disabled
        );
        assert_eq!(
            terminal.drain_events(),
            vec![crate::event::TerminalEvent::PtyWrite(
                b"\x1b[>6;65535m".to_vec()
            )]
        );

        // Resource-zero and malformed private prefixes cannot alias
        // parameterless all-resource resets. Modifier and format resets do not
        // cross their family boundary.
        processor.process(
            &mut terminal,
            b"\x1b[>1;3m\x1b[>2;4m\x1b[>3;4m\x1b[>4;2m\x1b[>6;4m\x1b[>1;1f\x1b[>2;1f\x1b[>3;1f\x1b[>4;1f\x1b[>6;1f\x1b[>65535m\x1b[>>m\x1b[>0m",
        );
        assert_eq!(
            terminal.xterm_modifier_keys().mode(),
            XtermKeyMode::Extended
        );
        processor.process(&mut terminal, b"\x1b[>m");
        assert_eq!(
            terminal.xterm_modifier_keys().mode(),
            XtermKeyMode::Original
        );
        assert_eq!(
            terminal.xterm_modifier_keys().format(),
            XtermKeyFormat::CsiU
        );
        assert_eq!(terminal.xterm_cursor_keys().mode(), XtermKeyMode::Parameter);
        assert_eq!(
            terminal.xterm_function_keys().mode(),
            XtermKeyMode::Parameter
        );
        assert_eq!(terminal.xterm_keypad_keys().mode(), XtermKeyMode::Original);
        assert_eq!(terminal.xterm_modify_other_keys().level(), 0);

        processor.process(&mut terminal, b"\x1b[>65535f\x1b[>>f\x1b[>0f");
        assert_eq!(
            terminal.xterm_modifier_keys().format(),
            XtermKeyFormat::CsiU
        );
        processor.process(&mut terminal, b"\x1b[>f");
        assert_eq!(
            terminal.xterm_modifier_keys().format(),
            XtermKeyFormat::FunctionKey27
        );
        assert_eq!(
            terminal.xterm_cursor_keys().format(),
            XtermKeyFormat::FunctionKey27
        );
        assert_eq!(
            terminal.xterm_function_keys().format(),
            XtermKeyFormat::FunctionKey27
        );
        assert_eq!(
            terminal.xterm_keypad_keys().format(),
            XtermKeyFormat::FunctionKey27
        );
        assert_eq!(
            terminal.xterm_modify_other_keys().format(),
            XtermKeyFormat::FunctionKey27
        );

        processor.process(
            &mut terminal,
            b"\x1b[>6;4m\x1b[>6;1f\x1bP$q>6m\x1b\\\x1bP$q>6f\x1b\\\x1b G\x1b[?6m\x1b[!p\x1b[?6mX",
        );
        assert_eq!(terminal.xterm_modifier_keys(), XtermModifierKeys::default());
        assert_eq!(
            terminal.grid().cell(0, 0).unwrap().content.char(),
            Some('X')
        );
        assert_eq!(
            terminal.drain_events(),
            vec![
                crate::event::TerminalEvent::PtyWrite(b"\x1bP1$r>6;4m\x1b\\".to_vec()),
                crate::event::TerminalEvent::PtyWrite(b"\x1bP1$r>6;1f\x1b\\".to_vec()),
                crate::event::TerminalEvent::PtyWrite(b"\x9b>6;4m".to_vec()),
                crate::event::TerminalEvent::PtyWrite(b"\x9b>6;0m".to_vec()),
            ]
        );

        processor.process(&mut terminal, b"\x1b[>6;4m\x1b[>6;1f\x1bc\x1b[?6m\x1b[?6g");
        assert_eq!(terminal.xterm_modifier_keys(), XtermModifierKeys::default());
        assert_eq!(
            terminal.drain_events(),
            vec![
                crate::event::TerminalEvent::PtyWrite(b"\x1b[>6;0m".to_vec()),
                crate::event::TerminalEvent::PtyWrite(b"\x1b[>6;0f".to_vec()),
            ]
        );
    }

    #[test]
    fn xterm_special_key_resources_are_bounded_queryable_and_resettable() {
        use crate::term::{XtermKeyFormat, XtermKeyMode, XtermSpecialKeys};

        let mut processor = Processor::new();
        let mut terminal = Terminal::new(10, 2);

        processor.process(
            &mut terminal,
            b"\x1b[?7m\x1b[?7g\x1b[>7;4m\x1b[>7;1f\x1b[?7m\x1b[?7g",
        );
        assert_eq!(terminal.xterm_special_keys().mode(), XtermKeyMode::Extended);
        assert_eq!(terminal.xterm_special_keys().format(), XtermKeyFormat::CsiU);
        assert_eq!(
            terminal.drain_events(),
            vec![
                crate::event::TerminalEvent::PtyWrite(b"\x1b[>7;0m".to_vec()),
                crate::event::TerminalEvent::PtyWrite(b"\x1b[>7;0f".to_vec()),
                crate::event::TerminalEvent::PtyWrite(b"\x1b[>7;4m".to_vec()),
                crate::event::TerminalEvent::PtyWrite(b"\x1b[>7;1f".to_vec()),
            ]
        );

        // Invalid values, subparameters, plural shapes, and unknown resource
        // 8 are atomic. State persists across screen changes.
        processor.process(
            &mut terminal,
            b"\x1b[>7;5m\x1b[>7;2f\x1b[>7:0m\x1b[>7;4;0m\x1b[>8;4m\x1b[?8m\x1b[?1049h\x1b[?7m",
        );
        assert_eq!(terminal.xterm_special_keys().mode(), XtermKeyMode::Extended);
        assert_eq!(terminal.xterm_special_keys().format(), XtermKeyFormat::CsiU);
        assert_eq!(
            terminal.drain_events(),
            vec![crate::event::TerminalEvent::PtyWrite(
                b"\x1b[>7;4m".to_vec()
            )]
        );

        processor.process(&mut terminal, b"\x1b[>7n\x1b[?7m");
        assert_eq!(terminal.xterm_special_keys().mode(), XtermKeyMode::Disabled);
        assert_eq!(
            terminal.drain_events(),
            vec![crate::event::TerminalEvent::PtyWrite(
                b"\x1b[>7;65535m".to_vec()
            )]
        );

        // Resource-zero and malformed private prefixes cannot alias
        // the parameterless family resets, which do not cross mode/format.
        processor.process(
            &mut terminal,
            b"\x1b[>7;4m\x1b[>7;1f\x1b[>65535m\x1b[>>m\x1b[>0m",
        );
        assert_eq!(terminal.xterm_special_keys().mode(), XtermKeyMode::Extended);
        processor.process(&mut terminal, b"\x1b[>m");
        assert_eq!(terminal.xterm_special_keys().mode(), XtermKeyMode::Original);
        assert_eq!(terminal.xterm_special_keys().format(), XtermKeyFormat::CsiU);
        processor.process(&mut terminal, b"\x1b[>65535f\x1b[>>f\x1b[>0f");
        assert_eq!(terminal.xterm_special_keys().format(), XtermKeyFormat::CsiU);
        processor.process(&mut terminal, b"\x1b[>f");
        assert_eq!(
            terminal.xterm_special_keys().format(),
            XtermKeyFormat::FunctionKey27
        );

        processor.process(
            &mut terminal,
            b"\x1b[>7;4m\x1b[>7;1f\x1bP$q>7m\x1b\\\x1bP$q>7f\x1b\\\x1b G\x1b[?7m\x1b[!p\x1b[?7mX",
        );
        assert_eq!(terminal.xterm_special_keys(), XtermSpecialKeys::default());
        assert_eq!(
            terminal.grid().cell(0, 0).unwrap().content.char(),
            Some('X')
        );
        assert_eq!(
            terminal.drain_events(),
            vec![
                crate::event::TerminalEvent::PtyWrite(b"\x1bP1$r>7;4m\x1b\\".to_vec()),
                crate::event::TerminalEvent::PtyWrite(b"\x1bP1$r>7;1f\x1b\\".to_vec()),
                crate::event::TerminalEvent::PtyWrite(b"\x9b>7;4m".to_vec()),
                crate::event::TerminalEvent::PtyWrite(b"\x9b>7;0m".to_vec()),
            ]
        );

        processor.process(&mut terminal, b"\x1b[>7;4m\x1b[>7;1f\x1bc\x1b[?7m\x1b[?7g");
        assert_eq!(terminal.xterm_special_keys(), XtermSpecialKeys::default());
        assert_eq!(
            terminal.drain_events(),
            vec![
                crate::event::TerminalEvent::PtyWrite(b"\x1b[>7;0m".to_vec()),
                crate::event::TerminalEvent::PtyWrite(b"\x1b[>7;0f".to_vec()),
            ]
        );
    }

    #[test]
    fn xterm_other_key_format_is_bounded_queryable_and_resettable() {
        use crate::term::XtermKeyFormat;

        let mut processor = Processor::new();
        let mut terminal = Terminal::new(10, 2);

        processor.process(&mut terminal, b"\x1b[?4g\x1b[>4;1f\x1b[?4g");
        assert_eq!(
            terminal.xterm_modify_other_keys().format(),
            XtermKeyFormat::CsiU
        );
        assert_eq!(
            terminal.drain_events(),
            vec![
                crate::event::TerminalEvent::PtyWrite(b"\x1b[>4;0f".to_vec()),
                crate::event::TerminalEvent::PtyWrite(b"\x1b[>4;1f".to_vec()),
            ]
        );

        // Unsupported resources, values, plural shapes, and subparameters
        // are atomic and cannot fall through to HVP cursor movement.
        processor.process(
            &mut terminal,
            b"\x1b[>4;2f\x1b[>99;1f\x1b[>0f\x1b[>4;1;0f\x1b[>4:0f\x1b[?1049h",
        );
        assert_eq!(
            terminal.xterm_modify_other_keys().format(),
            XtermKeyFormat::CsiU
        );
        assert_eq!(terminal.grid().cursor().row, 0);
        assert_eq!(terminal.grid().cursor().col, 0);

        // The resource is terminal-local across screen changes. Queries use
        // the selected S8C1T framing, and both resource-only and all-resource
        // reset forms restore the default formatter.
        processor.process(
            &mut terminal,
            b"\x1b G\x1b[?4g\x1b[>4f\x1b[?4g\x1b[>4;1f\x1b[>f\x1b[?4g",
        );
        assert_eq!(
            terminal.xterm_modify_other_keys().format(),
            XtermKeyFormat::FunctionKey27
        );
        assert_eq!(
            terminal.drain_events(),
            vec![
                crate::event::TerminalEvent::PtyWrite(b"\x9b>4;1f".to_vec()),
                crate::event::TerminalEvent::PtyWrite(b"\x9b>4;0f".to_vec()),
                crate::event::TerminalEvent::PtyWrite(b"\x9b>4;0f".to_vec()),
            ]
        );

        processor.process(&mut terminal, b"\x1b[>4;1f\x1b[!p\x1b[?4g\x1bP$q>4f\x1b\\X");
        assert_eq!(
            terminal.xterm_modify_other_keys().format(),
            XtermKeyFormat::FunctionKey27
        );
        assert_eq!(
            terminal.grid().cell(0, 0).unwrap().content.char(),
            Some('X')
        );
        assert_eq!(
            terminal.drain_events(),
            vec![
                crate::event::TerminalEvent::PtyWrite(b"\x9b>4;0f".to_vec()),
                crate::event::TerminalEvent::PtyWrite(b"\x901$r>4;0f\x9c".to_vec()),
            ]
        );

        processor.process(&mut terminal, b"\x1b[>4;1f\x1bc\x1b[?4g");
        assert_eq!(
            terminal.xterm_modify_other_keys().format(),
            XtermKeyFormat::FunctionKey27
        );
        assert_eq!(
            terminal.drain_events(),
            vec![crate::event::TerminalEvent::PtyWrite(
                b"\x1b[>4;0f".to_vec()
            )]
        );
    }

    #[test]
    fn decrqss_reports_rendition_margins_and_cursor_style_and_recovers_from_cancel() {
        let mut processor = Processor::new();
        let mut terminal = Terminal::new(20, 6);

        for byte in b"\x1bP$qm\x1b\\\x1bP$qr\x1b\\\x1bP$qs\x1b\\\x1bP$q q\x1b\\\x1bP$q\"q\x1b\\\x1bP$q\"p\x1b\\\x1bP$q*x\x1b\\\x1bP$qbogus\x1b\\"
        {
            processor.process(&mut terminal, &[*byte]);
        }
        assert_eq!(
            terminal.drain_events(),
            vec![
                crate::event::TerminalEvent::PtyWrite(b"\x1bP1$r0m\x1b\\".to_vec()),
                crate::event::TerminalEvent::PtyWrite(b"\x1bP1$r1;6r\x1b\\".to_vec()),
                crate::event::TerminalEvent::PtyWrite(b"\x1bP1$r1;20s\x1b\\".to_vec()),
                crate::event::TerminalEvent::PtyWrite(b"\x1bP1$r0 q\x1b\\".to_vec()),
                crate::event::TerminalEvent::PtyWrite(b"\x1bP1$r0\"q\x1b\\".to_vec()),
                crate::event::TerminalEvent::PtyWrite(b"\x1bP1$r64;1\"p\x1b\\".to_vec()),
                crate::event::TerminalEvent::PtyWrite(b"\x1bP1$r1*x\x1b\\".to_vec()),
                crate::event::TerminalEvent::PtyWrite(b"\x1bP0$r\x1b\\".to_vec()),
            ]
        );

        processor.process(
            &mut terminal,
            b"\x1b[1;3;4:3;6;7;8;9;38;5;196;48;2;1;2;3;58;2;4;5;6m\x1b[2;5r\x1b[4 q\x1b[9 q\x1b[1\"q\x1b[2*x",
        );
        processor.process(
            &mut terminal,
            b"\x1bP$qm\x1b\\\x1bP$qr\x1b\\\x1bP$q q\x1b\\\x1bP$q\"q\x1b\\\x1bP$q*x\x1b\\",
        );
        assert_eq!(
            terminal.drain_events(),
            vec![
                crate::event::TerminalEvent::PtyWrite(
                    b"\x1bP1$r1;3;4:3;6;7;8;9;38;5;196;48;2;1;2;3;58;2;4;5;6m\x1b\\".to_vec(),
                ),
                crate::event::TerminalEvent::PtyWrite(b"\x1bP1$r2;5r\x1b\\".to_vec()),
                crate::event::TerminalEvent::PtyWrite(b"\x1bP1$r4 q\x1b\\".to_vec()),
                crate::event::TerminalEvent::PtyWrite(b"\x1bP1$r1\"q\x1b\\".to_vec()),
                crate::event::TerminalEvent::PtyWrite(b"\x1bP1$r2*x\x1b\\".to_vec()),
            ]
        );

        // CAN/SUB cancel without replies or payload leakage. An ESC that is
        // not ST cancels DCS and begins the following escape sequence.
        processor.process(
            &mut terminal,
            b"\x1bP$qm\x18A\x1bP$qr\x1aB\x1bP$q q\x1b[31mC",
        );
        assert!(terminal.drain_events().is_empty());
        assert_eq!(terminal.grid().row(0).unwrap().to_string(), "ABC");
        assert_eq!(
            terminal.grid().cell(0, 2).unwrap().fg_spec,
            ColorSpec::Indexed(1)
        );

        processor.process(
            &mut terminal,
            b"\x1bc\x1bP$qm\x1b\\\x1bP$qr\x1b\\\x1bP$q q\x1b\\\x1bP$q\"q\x1b\\\x1bP$q\"p\x1b\\\x1bP$q*x\x1b\\",
        );
        assert_eq!(
            terminal.drain_events(),
            vec![
                crate::event::TerminalEvent::PtyWrite(b"\x1bP1$r0m\x1b\\".to_vec()),
                crate::event::TerminalEvent::PtyWrite(b"\x1bP1$r1;6r\x1b\\".to_vec()),
                crate::event::TerminalEvent::PtyWrite(b"\x1bP1$r0 q\x1b\\".to_vec()),
                crate::event::TerminalEvent::PtyWrite(b"\x1bP1$r0\"q\x1b\\".to_vec()),
                crate::event::TerminalEvent::PtyWrite(b"\x1bP1$r64;1\"p\x1b\\".to_vec()),
                crate::event::TerminalEvent::PtyWrite(b"\x1bP1$r1*x\x1b\\".to_vec()),
            ]
        );
    }

    #[test]
    fn decrqm_reports_only_authoritative_standard_and_private_mode_state() {
        let mut processor = Processor::new();
        let mut terminal = Terminal::new(20, 6);
        let standard = [4_u16, 20];
        let private = [
            1_u16, 6, 7, 9, 12, 25, 69, 1000, 1002, 1003, 1004, 1005, 1006, 1007, 1060, 1061, 2004,
            2026,
        ];

        // Fragment every request to prove that private markers and the `$`
        // intermediate survive arbitrary PTY read boundaries.
        let mut requests = Vec::new();
        for mode in standard {
            requests.extend_from_slice(format!("\x1b[{mode}$p").as_bytes());
        }
        for mode in private {
            requests.extend_from_slice(format!("\x1b[?{mode}$p").as_bytes());
        }
        requests.extend_from_slice(b"\x1b[?9999$p");
        for byte in requests {
            processor.process(&mut terminal, &[byte]);
        }

        let replies: Vec<Vec<u8>> = terminal
            .drain_events()
            .into_iter()
            .filter_map(|event| match event {
                crate::event::TerminalEvent::PtyWrite(bytes) => Some(bytes),
                _ => None,
            })
            .collect();
        let mut expected = Vec::new();
        for mode in standard {
            expected.push(format!("\x1b[{mode};2$y").into_bytes());
        }
        for mode in private {
            let status = if matches!(mode, 7 | 25) { 1 } else { 2 };
            expected.push(format!("\x1b[?{mode};{status}$y").into_bytes());
        }
        expected.push(b"\x1b[?9999;0$y".to_vec());
        assert_eq!(replies, expected);

        let standard_set = standard
            .iter()
            .map(u16::to_string)
            .collect::<Vec<_>>()
            .join(";");
        let private_set = private
            .iter()
            .map(u16::to_string)
            .collect::<Vec<_>>()
            .join(";");
        processor.process(
            &mut terminal,
            format!("\x1b[{standard_set}h\x1b[?{private_set}h").as_bytes(),
        );
        for mode in standard {
            processor.process(&mut terminal, format!("\x1b[{mode}$p").as_bytes());
        }
        for mode in private {
            processor.process(&mut terminal, format!("\x1b[?{mode}$p").as_bytes());
        }
        let replies: Vec<Vec<u8>> = terminal
            .drain_events()
            .into_iter()
            .filter_map(|event| match event {
                crate::event::TerminalEvent::PtyWrite(bytes) => Some(bytes),
                _ => None,
            })
            .collect();
        let expected: Vec<Vec<u8>> = standard
            .iter()
            .map(|mode| format!("\x1b[{mode};1$y").into_bytes())
            .chain(private.iter().map(|mode| {
                let status = if *mode == 1060 { 2 } else { 1 };
                format!("\x1b[?{mode};{status}$y").into_bytes()
            }))
            .collect();
        assert_eq!(replies, expected);
        assert_eq!(terminal.cursor_blink_override(), Some(true));
        assert!(terminal.grid().cursor().visible);

        // DECRQM has one scalar Ps. Malformed plural/subparameter requests and
        // the unrelated private DSR form do not produce misleading replies.
        processor.process(&mut terminal, b"\x1b[?7;25$p\x1b[?7:25$p\x1b[?6n");
        assert!(terminal.drain_events().is_empty());

        processor.process(&mut terminal, b"\x1b[?25l\x1b[?1049h");
        assert!(!terminal.grid().cursor().visible);
        processor.process(&mut terminal, b"\x1b[?25h\x1b[?1049l");
        assert!(terminal.grid().cursor().visible);

        processor.process(&mut terminal, b"\x1b[?12l\x1b[?25l\x1bc");
        assert_eq!(terminal.cursor_blink_override(), None);
        assert!(terminal.grid().cursor().visible);
        assert!(
            terminal
                .mode()
                .contains(crate::term::TerminalMode::CURSOR_VISIBLE)
        );
        assert!(
            !terminal
                .mode()
                .contains(crate::term::TerminalMode::CURSOR_BLINKING)
        );
    }

    #[test]
    fn unknown_and_oversized_dcs_are_negative_or_discarded_without_leaking_text() {
        let mut processor = Processor::new();
        let mut terminal = Terminal::new(10, 2);
        processor.process(&mut terminal, b"\x1bP+q756e6b6e6f776e\x1b\\");
        assert_eq!(
            terminal.drain_events(),
            vec![crate::event::TerminalEvent::PtyWrite(
                b"\x1bP0+r\x1b\\".to_vec()
            )]
        );

        let mut oversized = b"\x1bP+q".to_vec();
        oversized.extend(std::iter::repeat_n(b'a', MAX_DCS_SEQUENCE_BYTES + 1));
        oversized.extend_from_slice(b"\x1b\\ok");
        processor.process(&mut terminal, &oversized);
        assert!(terminal.drain_events().is_empty());
        assert_eq!(terminal.grid().row(0).unwrap().to_string(), "ok");
    }
}
