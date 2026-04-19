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
}
