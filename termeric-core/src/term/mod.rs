//! Terminal state machine
//!
//! This module contains the main terminal state including modes,
//! cursor handling, and the interface for processing escape sequences.

mod cursor;
mod mode;
mod selection;

pub use cursor::SavedCursor;
pub use mode::TerminalMode;
pub use selection::{Selection, SelectionType, Point};

use crate::grid::{Grid, Cell, CellAttrs, CellContent, Color};
use crate::ansi::Processor;

/// Tab stop interval
const TAB_INTERVAL: usize = 8;

/// The main terminal state
pub struct Terminal {
    /// Primary screen grid
    grid: Grid,
    /// Alternate screen grid (for vim, less, etc.)
    alt_grid: Grid,
    /// Whether we're using the alternate screen
    alt_screen_active: bool,
    /// Terminal mode flags
    mode: TerminalMode,
    /// Tab stop positions
    tabs: Vec<bool>,
    /// Saved cursor state for primary screen
    saved_cursor: Option<SavedCursor>,
    /// Saved cursor state for alternate screen
    alt_saved_cursor: Option<SavedCursor>,
    /// Current text attributes for new characters
    current_attrs: CellAttrs,
    /// Current foreground color
    current_fg: Color,
    /// Current background color
    current_bg: Color,
    /// Current selection
    selection: Option<Selection>,
    /// Scroll region top (inclusive, 0-indexed)
    scroll_top: usize,
    /// Scroll region bottom (exclusive, 0-indexed)
    scroll_bottom: usize,
    /// Escape sequence processor
    processor: Processor,
    /// Number of columns
    cols: usize,
    /// Number of rows
    rows: usize,
}

impl Terminal {
    /// Create a new terminal with the specified dimensions
    pub fn new(cols: usize, rows: usize) -> Self {
        let mut tabs = vec![false; cols];
        // Set default tab stops every 8 columns
        for i in (0..cols).step_by(TAB_INTERVAL) {
            tabs[i] = true;
        }

        Self {
            grid: Grid::new(cols, rows),
            alt_grid: Grid::new(cols, rows),
            alt_screen_active: false,
            mode: TerminalMode::default(),
            tabs,
            saved_cursor: None,
            alt_saved_cursor: None,
            current_attrs: CellAttrs::empty(),
            current_fg: Color::default_fg(),
            current_bg: Color::default_bg(),
            selection: None,
            scroll_top: 0,
            scroll_bottom: rows,
            processor: Processor::new(),
            cols,
            rows,
        }
    }

    /// Get the active grid
    pub fn grid(&self) -> &Grid {
        if self.alt_screen_active {
            &self.alt_grid
        } else {
            &self.grid
        }
    }

    /// Get the active grid mutably
    pub fn grid_mut(&mut self) -> &mut Grid {
        if self.alt_screen_active {
            &mut self.alt_grid
        } else {
            &mut self.grid
        }
    }

    /// Get terminal mode flags
    pub fn mode(&self) -> TerminalMode {
        self.mode
    }

    /// Get the current selection
    pub fn selection(&self) -> Option<&Selection> {
        self.selection.as_ref()
    }

    /// Set the selection
    pub fn set_selection(&mut self, selection: Option<Selection>) {
        self.selection = selection;
    }

    /// Get the number of columns
    pub fn cols(&self) -> usize {
        self.cols
    }

    /// Get the number of rows
    pub fn rows(&self) -> usize {
        self.rows
    }

    /// Process input bytes from PTY
    pub fn process(&mut self, bytes: &[u8]) {
        // Take the processor temporarily to avoid borrow conflict
        let mut processor = std::mem::take(&mut self.processor);
        processor.process(self, bytes);
        self.processor = processor;
    }

    /// Resize the terminal
    pub fn resize(&mut self, cols: usize, rows: usize) {
        self.grid.resize(cols, rows);
        self.alt_grid.resize(cols, rows);

        // Update tab stops
        self.tabs.resize(cols, false);
        for i in (0..cols).step_by(TAB_INTERVAL) {
            self.tabs[i] = true;
        }

        // Update scroll region
        self.scroll_top = 0;
        self.scroll_bottom = rows;

        self.cols = cols;
        self.rows = rows;
    }

    /// Write a character at the current cursor position
    pub fn write_char(&mut self, c: char) {
        let cursor = self.grid().cursor();
        let cols = self.cols;

        // Handle line wrap
        if cursor.col >= cols {
            if self.mode.contains(TerminalMode::AUTOWRAP) {
                self.grid_mut().set_cursor(cursor.row, 0);
                self.linefeed();
            } else {
                self.grid_mut().set_cursor(cursor.row, cols - 1);
            }
        }

        let cursor = self.grid().cursor();
        let fg = self.current_fg;
        let bg = self.current_bg;
        let attrs = self.current_attrs;

        // Write the character
        if let Some(cell) = self.grid_mut().cell_mut(cursor.row, cursor.col) {
            cell.content = CellContent::Char(c);
            cell.fg = fg;
            cell.bg = bg;
            cell.attrs = attrs;
        }

        // Move cursor forward
        self.grid_mut().set_cursor(cursor.row, cursor.col + 1);
    }

    /// Perform a carriage return (move cursor to column 0)
    pub fn carriage_return(&mut self) {
        let cursor = self.grid().cursor();
        self.grid_mut().set_cursor(cursor.row, 0);
    }

    /// Perform a line feed (move cursor down, scroll if at bottom)
    pub fn linefeed(&mut self) {
        let cursor = self.grid().cursor();

        if cursor.row + 1 >= self.scroll_bottom {
            // At bottom of scroll region, scroll up
            self.scroll_up(1);
        } else {
            self.grid_mut().set_cursor(cursor.row + 1, cursor.col);
        }
    }

    /// Scroll the screen up by n lines
    pub fn scroll_up(&mut self, n: usize) {
        for _ in 0..n {
            self.grid_mut().scroll_up();
        }
    }

    /// Scroll the screen down by n lines
    pub fn scroll_down(&mut self, n: usize) {
        for _ in 0..n {
            self.grid_mut().scroll_down();
        }
    }

    /// Move to the next tab stop
    pub fn horizontal_tab(&mut self) {
        let cursor = self.grid().cursor();
        let cols = self.cols;
        let mut col = cursor.col + 1;

        while col < cols && !self.tabs.get(col).copied().unwrap_or(false) {
            col += 1;
        }

        let new_col = col.min(cols.saturating_sub(1));
        self.grid_mut().set_cursor(cursor.row, new_col);
    }

    /// Move cursor up
    pub fn cursor_up(&mut self, n: usize) {
        let cursor = self.grid().cursor();
        self.grid_mut().set_cursor(cursor.row.saturating_sub(n), cursor.col);
    }

    /// Move cursor down
    pub fn cursor_down(&mut self, n: usize) {
        let cursor = self.grid().cursor();
        let new_row = (cursor.row + n).min(self.rows - 1);
        self.grid_mut().set_cursor(new_row, cursor.col);
    }

    /// Move cursor forward (right)
    pub fn cursor_forward(&mut self, n: usize) {
        let cursor = self.grid().cursor();
        let new_col = (cursor.col + n).min(self.cols - 1);
        self.grid_mut().set_cursor(cursor.row, new_col);
    }

    /// Move cursor backward (left)
    pub fn cursor_backward(&mut self, n: usize) {
        let cursor = self.grid().cursor();
        self.grid_mut().set_cursor(cursor.row, cursor.col.saturating_sub(n));
    }

    /// Set cursor position (1-indexed input, converted to 0-indexed)
    pub fn set_cursor_position(&mut self, row: usize, col: usize) {
        let row = row.saturating_sub(1).min(self.rows - 1);
        let col = col.saturating_sub(1).min(self.cols - 1);
        self.grid_mut().set_cursor(row, col);
    }

    /// Erase in display
    pub fn erase_in_display(&mut self, mode: u16) {
        let cursor = self.grid().cursor();

        match mode {
            0 => {
                // Erase from cursor to end of screen
                self.erase_in_line(0);
                for row in (cursor.row + 1)..self.rows {
                    self.grid_mut().clear_row(row);
                }
            }
            1 => {
                // Erase from start of screen to cursor
                for row in 0..cursor.row {
                    self.grid_mut().clear_row(row);
                }
                self.erase_in_line(1);
            }
            2 => {
                // Erase entire screen
                self.grid_mut().clear();
            }
            3 => {
                // Erase entire screen and scrollback
                self.grid_mut().clear();
                self.grid_mut().clear_scrollback();
            }
            _ => {}
        }
    }

    /// Erase in line
    pub fn erase_in_line(&mut self, mode: u16) {
        let cursor = self.grid().cursor();

        let (start, end) = match mode {
            0 => (cursor.col, self.cols),
            1 => (0, cursor.col + 1),
            2 => (0, self.cols),
            _ => return,
        };

        for col in start..end {
            if let Some(cell) = self.grid_mut().cell_mut(cursor.row, col) {
                cell.clear();
            }
        }
    }

    /// Set text attributes (SGR)
    pub fn set_attrs(&mut self, attrs: CellAttrs) {
        self.current_attrs = attrs;
    }

    /// Set foreground color
    pub fn set_fg(&mut self, color: Color) {
        self.current_fg = color;
    }

    /// Set background color
    pub fn set_bg(&mut self, color: Color) {
        self.current_bg = color;
    }

    /// Reset all attributes to default
    pub fn reset_attrs(&mut self) {
        self.current_attrs = CellAttrs::empty();
        self.current_fg = Color::default_fg();
        self.current_bg = Color::default_bg();
    }

    /// Switch to alternate screen buffer
    pub fn enter_alt_screen(&mut self) {
        if !self.alt_screen_active {
            self.alt_screen_active = true;
            self.alt_grid.clear();
        }
    }

    /// Switch back to primary screen buffer
    pub fn exit_alt_screen(&mut self) {
        if self.alt_screen_active {
            self.alt_screen_active = false;
        }
    }

    /// Save cursor position
    pub fn save_cursor(&mut self) {
        let cursor = self.grid().cursor();
        let saved = SavedCursor {
            row: cursor.row,
            col: cursor.col,
            attrs: self.current_attrs,
            fg: self.current_fg,
            bg: self.current_bg,
        };

        if self.alt_screen_active {
            self.alt_saved_cursor = Some(saved);
        } else {
            self.saved_cursor = Some(saved);
        }
    }

    /// Restore cursor position
    pub fn restore_cursor(&mut self) {
        let saved = if self.alt_screen_active {
            self.alt_saved_cursor.take()
        } else {
            self.saved_cursor.take()
        };

        if let Some(saved) = saved {
            self.grid_mut().set_cursor(saved.row, saved.col);
            self.current_attrs = saved.attrs;
            self.current_fg = saved.fg;
            self.current_bg = saved.bg;
        }
    }

    /// Set cursor visibility
    pub fn set_cursor_visible(&mut self, visible: bool) {
        self.grid_mut().set_cursor_visible(visible);
    }

    /// Set a terminal mode
    pub fn set_mode(&mut self, mode: TerminalMode, enable: bool) {
        if enable {
            self.mode.insert(mode);
        } else {
            self.mode.remove(mode);
        }
    }

    /// Full reset (RIS)
    pub fn reset(&mut self) {
        self.grid.clear();
        self.alt_grid.clear();
        self.alt_screen_active = false;
        self.mode = TerminalMode::default();
        self.reset_attrs();
        self.saved_cursor = None;
        self.alt_saved_cursor = None;
        self.scroll_top = 0;
        self.scroll_bottom = self.rows;
        self.selection = None;

        // Reset tab stops
        for (i, tab) in self.tabs.iter_mut().enumerate() {
            *tab = i % TAB_INTERVAL == 0;
        }
    }

    /// Get text content for selection
    pub fn get_selection_text(&self) -> Option<String> {
        let selection = self.selection.as_ref()?;
        let (start, end) = selection.bounds();

        let mut text = String::new();

        for row in start.row..=end.row {
            if row < 0 {
                // Scrollback - not yet implemented
                continue;
            }

            let row = row as usize;
            if let Some(grid_row) = self.grid().row(row) {
                let start_col = if row == start.row as usize { start.col } else { 0 };
                let end_col = if row == end.row as usize { end.col } else { self.cols - 1 };

                for col in start_col..=end_col {
                    if let Some(cell) = grid_row.get(col) {
                        match &cell.content {
                            CellContent::Empty => text.push(' '),
                            CellContent::Char(c) => text.push(*c),
                            CellContent::Grapheme(g) => text.push_str(g),
                        }
                    }
                }

                if row != end.row as usize {
                    text.push('\n');
                }
            }
        }

        Some(text.trim_end().to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_terminal_creation() {
        let term = Terminal::new(80, 24);
        assert_eq!(term.cols(), 80);
        assert_eq!(term.rows(), 24);
    }

    #[test]
    fn test_write_char() {
        let mut term = Terminal::new(80, 24);
        term.write_char('A');
        let cursor = term.grid().cursor();
        assert_eq!(cursor.col, 1);

        let cell = term.grid().cell(0, 0).unwrap();
        assert_eq!(cell.content.char(), Some('A'));
    }

    #[test]
    fn test_cursor_movement() {
        let mut term = Terminal::new(80, 24);
        term.set_cursor_position(5, 10);
        let cursor = term.grid().cursor();
        assert_eq!(cursor.row, 4);
        assert_eq!(cursor.col, 9);
    }

    #[test]
    fn test_alt_screen() {
        let mut term = Terminal::new(80, 24);
        term.write_char('A');
        term.enter_alt_screen();
        assert!(term.grid().cell(0, 0).unwrap().is_empty());
        term.exit_alt_screen();
        assert_eq!(term.grid().cell(0, 0).unwrap().content.char(), Some('A'));
    }
}
