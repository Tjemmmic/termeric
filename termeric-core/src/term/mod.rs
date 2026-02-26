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

use crate::grid::{Grid, CellAttrs, CellContent, CellFlags, Color, ColorSpec, HyperlinkRegistry};
use crate::ansi::Processor;
use crate::graphics::{GraphicsManager, KittyCommand, ImagePlacement, ImageId, StoredImage};
use crate::shell::ShellIntegration;
use crate::event::TerminalEvent;

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
    /// Current foreground color (resolved RGB)
    current_fg: Color,
    /// Current background color (resolved RGB)
    current_bg: Color,
    /// How the current foreground was specified (indexed, true color, or default)
    current_fg_spec: ColorSpec,
    /// How the current background was specified (indexed, true color, or default)
    current_bg_spec: ColorSpec,
    /// Current underline color (None means use fg color)
    current_underline_color: Option<Color>,
    /// Current hyperlink ID (if inside a hyperlink)
    current_hyperlink_id: Option<u32>,
    /// Registry of all hyperlinks
    hyperlink_registry: HyperlinkRegistry,
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
    /// Graphics manager for inline images
    graphics: GraphicsManager,
    /// Shell integration for prompt tracking
    shell: ShellIntegration,
    /// Pending events to be handled by the application
    pending_events: Vec<TerminalEvent>,
}

impl Terminal {
    /// Create a new terminal with the specified dimensions
    pub fn new(cols: usize, rows: usize) -> Self {
        let mut tabs = vec![false; cols];
        // Set default tab stops every 8 columns
        for i in (0..cols).step_by(TAB_INTERVAL) {
            tabs[i] = true;
        }
        let mut mode = TerminalMode::default();
        mode.insert(TerminalMode::AUTOWRAP);

        Self {
            grid: Grid::new(cols, rows),
            alt_grid: Grid::new(cols, rows),
            alt_screen_active: false,
            mode,
            tabs,
            saved_cursor: None,
            alt_saved_cursor: None,
            current_attrs: CellAttrs::empty(),
            current_fg: Color::default_fg(),
            current_bg: Color::default_bg(),
            current_fg_spec: ColorSpec::Default,
            current_bg_spec: ColorSpec::Default,
            current_underline_color: None,
            current_hyperlink_id: None,
            hyperlink_registry: HyperlinkRegistry::new(),
            selection: None,
            scroll_top: 0,
            scroll_bottom: rows,
            processor: Processor::new(),
            cols,
            rows,
            graphics: GraphicsManager::new(),
            shell: ShellIntegration::new(),
            pending_events: Vec::new(),
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

    /// Push a terminal event to be handled by the application
    pub fn push_event(&mut self, event: TerminalEvent) {
        self.pending_events.push(event);
    }

    /// Drain all pending events
    pub fn drain_events(&mut self) -> Vec<TerminalEvent> {
        std::mem::take(&mut self.pending_events)
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
        let fg_spec = self.current_fg_spec;
        let bg_spec = self.current_bg_spec;
        let attrs = self.current_attrs;

        if self.mode.contains(TerminalMode::INSERT) {
            if let Some(row) = self.grid_mut().row_mut(cursor.row) {
                row.insert_cells(cursor.col, 1);
            }
        }

        // Write the character
        let underline_color = self.current_underline_color;
        let hyperlink_id = self.current_hyperlink_id;
        if let Some(cell) = self.grid_mut().cell_mut(cursor.row, cursor.col) {
            cell.content = CellContent::Char(c);
            cell.fg = fg;
            cell.bg = bg;
            cell.fg_spec = fg_spec;
            cell.bg_spec = bg_spec;
            cell.underline_color = underline_color;
            cell.attrs = attrs;
            cell.hyperlink_id = hyperlink_id;
            // Set hyperlink flag if this cell is part of a hyperlink
            if hyperlink_id.is_some() {
                cell.flags.insert(CellFlags::HYPERLINK);
            } else {
                cell.flags.remove(CellFlags::HYPERLINK);
            }
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

        if self.mode.contains(TerminalMode::LINEFEED_NEWLINE) {
            self.carriage_return();
        }
    }

    /// Scroll the screen up by n lines
    pub fn scroll_up(&mut self, n: usize) {
        let scroll_top = self.scroll_top;
        let scroll_bottom = self.scroll_bottom;
        for _ in 0..n {
            if scroll_top == 0 && scroll_bottom == self.rows {
                self.grid_mut().scroll_up();
            } else {
                self.grid_mut().scroll_region_up(scroll_top, scroll_bottom);
            }
        }
    }

    /// Scroll the screen down by n lines
    pub fn scroll_down(&mut self, n: usize) {
        let scroll_top = self.scroll_top;
        let scroll_bottom = self.scroll_bottom;
        for _ in 0..n {
            if scroll_top == 0 && scroll_bottom == self.rows {
                self.grid_mut().scroll_down();
            } else {
                self.grid_mut().scroll_region_down(scroll_top, scroll_bottom);
            }
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
        let mut row = row.saturating_sub(1);
        if self.mode.contains(TerminalMode::ORIGIN) {
            row = row.saturating_add(self.scroll_top);
        }
        let row = row.min(self.rows - 1);
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

    /// Set foreground color with its original specification
    pub fn set_fg(&mut self, color: Color, spec: ColorSpec) {
        self.current_fg = color;
        self.current_fg_spec = spec;
    }

    /// Set background color with its original specification
    pub fn set_bg(&mut self, color: Color, spec: ColorSpec) {
        self.current_bg = color;
        self.current_bg_spec = spec;
    }

    /// Reset all attributes to default
    pub fn reset_attrs(&mut self) {
        self.current_attrs = CellAttrs::empty();
        self.current_fg = Color::default_fg();
        self.current_bg = Color::default_bg();
        self.current_fg_spec = ColorSpec::Default;
        self.current_bg_spec = ColorSpec::Default;
        self.current_underline_color = None;
    }

    /// Add text attributes to the current set
    pub fn add_attrs(&mut self, attrs: CellAttrs) {
        self.current_attrs.insert(attrs);
    }

    /// Remove text attributes from the current set
    pub fn remove_attrs(&mut self, attrs: CellAttrs) {
        self.current_attrs.remove(attrs);
    }

    /// Set underline color
    pub fn set_underline_color(&mut self, color: Color) {
        self.current_underline_color = Some(color);
    }

    /// Reset underline color (use fg color)
    pub fn reset_underline_color(&mut self) {
        self.current_underline_color = None;
    }

    /// Set underline style (clears all underline flags and sets the specified style)
    pub fn set_underline_style(&mut self, style: u16) {
        // Clear all underline-related flags first
        self.current_attrs.remove(CellAttrs::ALL_UNDERLINES);

        // Set the specified style
        match style {
            0 => {} // No underline
            1 => self.current_attrs.insert(CellAttrs::UNDERLINE),
            2 => self.current_attrs.insert(CellAttrs::DOUBLE_UNDERLINE),
            3 => self.current_attrs.insert(CellAttrs::CURLY_UNDERLINE),
            4 => self.current_attrs.insert(CellAttrs::DOTTED_UNDERLINE),
            5 => self.current_attrs.insert(CellAttrs::DASHED_UNDERLINE),
            _ => self.current_attrs.insert(CellAttrs::UNDERLINE), // Default to single
        }
    }

    /// Set the current hyperlink (OSC 8)
    ///
    /// All characters written after this will be part of this hyperlink
    /// until `reset_hyperlink` is called.
    pub fn set_hyperlink(&mut self, url: String, id: Option<String>) {
        let hyperlink_id = self.hyperlink_registry.get_or_insert(url, id);
        self.current_hyperlink_id = Some(hyperlink_id);
    }

    /// Clear the current hyperlink
    ///
    /// Characters written after this will not be part of any hyperlink.
    pub fn reset_hyperlink(&mut self) {
        self.current_hyperlink_id = None;
    }

    /// Get the URL for a hyperlink ID
    pub fn get_hyperlink_url(&self, id: u32) -> Option<&str> {
        self.hyperlink_registry.get_url(id)
    }

    /// Get access to the hyperlink registry
    pub fn hyperlink_registry(&self) -> &HyperlinkRegistry {
        &self.hyperlink_registry
    }

    // === Graphics methods ===

    /// Handle a Kitty graphics APC command
    pub fn handle_graphics_command(&mut self, payload: &[u8]) {
        if let Some(cmd) = KittyCommand::parse(payload) {
            // Record cursor position for placement
            let cursor = self.grid().cursor();
            let mut cmd = cmd;
            if let Some(ref mut placement) = cmd.placement {
                // If no explicit position, use cursor position
                if placement.col.is_none() {
                    placement.col = Some(cursor.col as u32);
                }
                if placement.row.is_none() {
                    placement.row = Some(cursor.row as u32);
                }
            }

            if let Some(response) = self.graphics.handle_command(cmd) {
                log::trace!("Graphics response: {:?}", response);
                // TODO: Send response back to PTY
            }
        }
    }

    /// Get all image placements for rendering
    pub fn graphics_placements(&self) -> &[ImagePlacement] {
        self.graphics.placements()
    }

    /// Get image data for rendering
    pub fn get_image(&self, id: ImageId) -> Option<&StoredImage> {
        self.graphics.get_image(id)
    }

    /// Check if there are images to render
    pub fn has_images(&self) -> bool {
        self.graphics.has_images()
    }

    // === Shell integration methods ===

    /// Handle OSC 133 shell integration sequence
    pub fn handle_osc_133(&mut self, params: &[&[u8]]) {
        let cursor_row = self.grid().cursor().row;
        self.shell.handle_osc_133(params, cursor_row);
    }

    /// Get the previous prompt row from the current position
    pub fn previous_prompt(&self) -> Option<usize> {
        let current_row = self.grid().cursor().row as i32;
        self.shell.previous_prompt(current_row)
    }

    /// Get the next prompt row from the current position
    pub fn next_prompt(&self) -> Option<usize> {
        let current_row = self.grid().cursor().row as i32;
        self.shell.next_prompt(current_row)
    }

    /// Get shell integration state
    pub fn shell_integration(&self) -> &ShellIntegration {
        &self.shell
    }

    /// Check if synchronized output mode is enabled
    pub fn is_synchronized_output(&self) -> bool {
        self.mode.contains(TerminalMode::SYNCHRONIZED_OUTPUT)
    }

    /// Set a tab stop at the current column
    pub fn set_tab_stop(&mut self) {
        let cursor = self.grid().cursor();
        if let Some(tab) = self.tabs.get_mut(cursor.col) {
            *tab = true;
        }
    }

    /// Clear all tab stops
    pub fn clear_tab_stops(&mut self) {
        for tab in &mut self.tabs {
            *tab = false;
        }
    }

    /// Set the scroll region (1-indexed, inclusive) for DECSTBM
    pub fn set_scroll_region(&mut self, top: usize, bottom: usize) {
        let top = top.saturating_sub(1);
        let bottom = bottom.min(self.rows);
        if bottom <= top || bottom == 0 {
            self.scroll_top = 0;
            self.scroll_bottom = self.rows;
        } else {
            self.scroll_top = top.min(self.rows.saturating_sub(1));
            self.scroll_bottom = bottom;
        }
        self.set_cursor_position(1, 1);
    }

    /// Reverse index (move up, scroll down if at top of region)
    pub fn reverse_index(&mut self) {
        let cursor = self.grid().cursor();
        if cursor.row <= self.scroll_top {
            self.scroll_down(1);
        } else {
            self.cursor_up(1);
        }
    }

    /// Insert blank characters at cursor position
    pub fn insert_chars(&mut self, count: usize) {
        let cursor = self.grid().cursor();
        if let Some(row) = self.grid_mut().row_mut(cursor.row) {
            row.insert_cells(cursor.col, count);
        }
    }

    /// Delete characters at cursor position
    pub fn delete_chars(&mut self, count: usize) {
        let cursor = self.grid().cursor();
        if let Some(row) = self.grid_mut().row_mut(cursor.row) {
            row.delete_cells(cursor.col, count);
        }
    }

    /// Insert blank lines at cursor row within the scroll region
    pub fn insert_lines(&mut self, count: usize) {
        let cursor = self.grid().cursor();
        let scroll_bottom = self.scroll_bottom;
        if cursor.row < scroll_bottom {
            for _ in 0..count {
                self.grid_mut().scroll_region_down(cursor.row, scroll_bottom);
            }
        }
    }

    /// Delete lines at cursor row within the scroll region
    pub fn delete_lines(&mut self, count: usize) {
        let cursor = self.grid().cursor();
        let scroll_bottom = self.scroll_bottom;
        if cursor.row < scroll_bottom {
            for _ in 0..count {
                self.grid_mut().scroll_region_up(cursor.row, scroll_bottom);
            }
        }
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
            fg_spec: self.current_fg_spec,
            bg_spec: self.current_bg_spec,
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
            self.current_fg_spec = saved.fg_spec;
            self.current_bg_spec = saved.bg_spec;
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
        self.mode.insert(TerminalMode::AUTOWRAP);
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
            if let Some(grid_row) = self.grid().row_at_offset(row) {
                let start_col = if row == start.row { start.col } else { 0 };
                let end_col = if row == end.row { end.col } else { self.cols - 1 };
                let start_col = start_col.min(self.cols.saturating_sub(1));
                let end_col = end_col.min(self.cols.saturating_sub(1));

                for col in start_col..=end_col {
                    if let Some(cell) = grid_row.get(col) {
                        match &cell.content {
                            CellContent::Empty => text.push(' '),
                            CellContent::Char(c) => text.push(*c),
                            CellContent::Grapheme(g) => text.push_str(g),
                        }
                    }
                }

                if row != end.row {
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
