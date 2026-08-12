//! Escape sequence processor

use super::handler::Handler;
use crate::term::Terminal;
use vte::Parser;

/// Processor for parsing and handling escape sequences
pub struct Processor {
    parser: Parser,
}

impl Processor {
    /// Create a new processor
    pub fn new() -> Self {
        Self {
            parser: Parser::new(),
        }
    }

    /// Process input bytes, updating terminal state
    pub fn process(&mut self, terminal: &mut Terminal, bytes: &[u8]) {
        let mut handler = Handler::new(terminal);
        for byte in bytes {
            self.parser.advance(&mut handler, *byte);
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
}
