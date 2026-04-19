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

        // Handle pending line wrap: if the previous character was written to
        // the last column, wrap_pending is set. Now that a new printable char
        // arrives, perform the actual wrap (mark the row as soft-wrapped,
        // carriage-return, and linefeed).
        if cursor.wrap_pending {
            if self.mode.contains(TerminalMode::AUTOWRAP) {
                // Mark the row we're leaving as soft-wrapped
                if let Some(row) = self.grid_mut().row_mut(cursor.row) {
                    row.set_wrapped(true);
                }
                self.grid_mut().set_cursor(cursor.row, 0);
                self.linefeed();
            }
            // If not autowrap, cursor stays at last column (overwrite mode)
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

        // Advance cursor. If we just wrote to the last column, set
        // wrap_pending instead of advancing past the edge (which would
        // be clamped by set_cursor anyway).
        if cursor.col + 1 >= cols {
            // Stay at last column but flag that a wrap is pending
            self.grid_mut().set_wrap_pending(true);
        } else {
            self.grid_mut().set_cursor(cursor.row, cursor.col + 1);
        }
    }

    /// Perform a carriage return (move cursor to column 0)
    pub fn carriage_return(&mut self) {
        let cursor = self.grid().cursor();
        // Clear wrapped flag — explicit CR means this row ends here
        if let Some(row) = self.grid_mut().row_mut(cursor.row) {
            row.set_wrapped(false);
        }
        self.grid_mut().set_cursor(cursor.row, 0);
    }

    /// Perform a line feed (move cursor down, scroll if at bottom)
    pub fn linefeed(&mut self) {
        let cursor = self.grid().cursor();

        if cursor.row + 1 >= self.scroll_bottom {
            // At bottom of scroll region, scroll up
            self.scroll_up(1);
            // Clear wrap_pending — we've moved to a new logical position
            self.grid_mut().set_wrap_pending(false);
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
                // Emit the response as Kitty graphics ack back to the PTY,
                // so apps probing for image-protocol support (e.g. `icat`
                // in Kitty-compatible mode) get the expected reply.
                let bytes = response.to_escape_sequence().into_bytes();
                self.push_event(crate::event::TerminalEvent::PtyWrite(bytes));
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

    /// Extract the text on rows `[start_row..=end_row_inclusive]` from the
    /// grid (scrollback-aware). Rows are row-at-offset indices: 0..rows-1
    /// are visible, negative values reach into scrollback. Trailing
    /// whitespace on each row is stripped.
    pub fn rows_text(&self, start_row: i32, end_row_inclusive: i32) -> String {
        if end_row_inclusive < start_row {
            return String::new();
        }
        let mut out = String::new();
        for row in start_row..=end_row_inclusive {
            if let Some(grid_row) = self.grid().row_at_offset(row) {
                let mut line = String::new();
                for col in 0..self.cols {
                    if let Some(cell) = grid_row.get(col) {
                        match &cell.content {
                            CellContent::Empty => line.push(' '),
                            CellContent::Char(c) => line.push(*c),
                            CellContent::Grapheme(g) => line.push_str(g),
                        }
                    }
                }
                out.push_str(line.trim_end());
            }
            if row != end_row_inclusive {
                out.push('\n');
            }
        }
        out
    }

    /// Expand a click point to its word boundaries (for double-click semantic
    /// selection). Returns `(start, end_inclusive)`. If the clicked cell is
    /// whitespace or empty, returns `(point, point)` unchanged so the caller
    /// falls back to a zero-width selection.
    pub fn word_at(&self, point: Point) -> (Point, Point) {
        let Some(row) = self.grid().row_at_offset(point.row) else {
            return (point, point);
        };
        let cell_char = |col: usize| -> Option<char> {
            row.get(col).and_then(|c| c.content.char())
        };

        // Anchor must itself be a word char, otherwise don't expand.
        match cell_char(point.col) {
            Some(c) if is_word_char(c) => {}
            _ => return (point, point),
        }

        let max_col = self.cols.saturating_sub(1);
        // Walk left
        let mut start_col = point.col;
        while start_col > 0 {
            match cell_char(start_col - 1) {
                Some(c) if is_word_char(c) => start_col -= 1,
                _ => break,
            }
        }
        // Walk right
        let mut end_col = point.col;
        while end_col < max_col {
            match cell_char(end_col + 1) {
                Some(c) if is_word_char(c) => end_col += 1,
                _ => break,
            }
        }

        (
            Point::new(point.row, start_col),
            Point::new(point.row, end_col),
        )
    }

    /// Expand a click point to its full line (for triple-click line selection).
    /// Trailing whitespace is excluded so the selection doesn't drag past the
    /// visible end of the line.
    pub fn line_at(&self, point: Point) -> (Point, Point) {
        let Some(row) = self.grid().row_at_offset(point.row) else {
            return (point, point);
        };
        let max_col = self.cols.saturating_sub(1);
        let mut end_col = max_col;
        while end_col > 0 {
            let ch = row.get(end_col).and_then(|c| c.content.char());
            match ch {
                Some(c) if !c.is_whitespace() => break,
                _ => end_col -= 1,
            }
        }
        (Point::new(point.row, 0), Point::new(point.row, end_col))
    }

    /// Get text content for selection. Honors the selection type:
    /// - Simple/Semantic: contiguous range, newline between rows.
    /// - Lines: full rows from start to end.
    /// - Block: rectangular region (one newline between rows).
    pub fn get_selection_text(&self) -> Option<String> {
        let selection = self.selection.as_ref()?;
        let (start, end) = selection.bounds();

        let max_col = self.cols.saturating_sub(1);
        let mut text = String::new();

        for row in start.row..=end.row {
            let Some(grid_row) = self.grid().row_at_offset(row) else { continue };

            // Resolve the column range based on selection type.
            let (start_col, end_col) = match selection.ty {
                SelectionType::Simple | SelectionType::Semantic => {
                    let s = if row == start.row { start.col } else { 0 };
                    let e = if row == end.row { end.col } else { max_col };
                    (s.min(max_col), e.min(max_col))
                }
                SelectionType::Lines => (0, max_col),
                SelectionType::Block => {
                    let s = start.col.min(end.col);
                    let e = start.col.max(end.col);
                    (s.min(max_col), e.min(max_col))
                }
            };

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

        Some(text.trim_end().to_string())
    }
}

/// Whether `c` counts as part of a word for double-click expansion.
/// Covers alphanumerics plus the common path / URL / identifier punctuation
/// so filenames, URLs, and code identifiers select as a single unit.
fn is_word_char(c: char) -> bool {
    c.is_alphanumeric() || matches!(c, '_' | '-' | '.' | '/' | ':' | '@' | '+' | '~' | '%' | '?' | '&' | '=' | '#')
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

    fn write_line(term: &mut Terminal, s: &str) {
        for c in s.chars() {
            term.write_char(c);
        }
    }

    #[test]
    fn test_word_at_simple_word() {
        let mut term = Terminal::new(80, 24);
        write_line(&mut term, "hello world");
        let (start, end) = term.word_at(Point::new(0, 2)); // inside "hello"
        assert_eq!(start, Point::new(0, 0));
        assert_eq!(end, Point::new(0, 4));
    }

    #[test]
    fn test_word_at_whitespace_is_noop() {
        let mut term = Terminal::new(80, 24);
        write_line(&mut term, "hello world");
        let (start, end) = term.word_at(Point::new(0, 5)); // the space
        assert_eq!(start, Point::new(0, 5));
        assert_eq!(end, Point::new(0, 5));
    }

    #[test]
    fn test_word_at_url_like_token() {
        // path- and URL-like punctuation is part of the word.
        let mut term = Terminal::new(80, 24);
        write_line(&mut term, "cat /tmp/foo.txt done");
        let (start, end) = term.word_at(Point::new(0, 8)); // inside "/tmp/foo.txt"
        assert_eq!(start, Point::new(0, 4));
        assert_eq!(end, Point::new(0, 15));
    }

    #[test]
    fn test_line_at_trims_trailing_space() {
        let mut term = Terminal::new(80, 24);
        write_line(&mut term, "hi   ");
        let (start, end) = term.line_at(Point::new(0, 3));
        assert_eq!(start, Point::new(0, 0));
        assert_eq!(end, Point::new(0, 1)); // "hi" ends at col 1
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

    /// Helper: collect all non-empty characters from scrollback + visible area.
    /// This is a content-only check — ignores line boundaries.
    fn extract_all_chars(term: &Terminal) -> String {
        let grid = term.grid();
        let mut chars = String::new();

        // Scrollback
        for row in grid.scrollback_rows() {
            for cell in row.iter() {
                if let Some(c) = cell.content.char() {
                    chars.push(c);
                }
            }
        }

        // Visible area
        for r in 0..grid.rows() {
            if let Some(row) = grid.row(r) {
                for cell in row.iter() {
                    if let Some(c) = cell.content.char() {
                        chars.push(c);
                    }
                }
            }
        }

        chars
    }

    #[test]
    fn test_process_and_resize_preserves_content() {
        // Process a few lines of text at 40 cols, then shrink to 10 cols.
        // All characters must still be present.
        let mut term = Terminal::new(40, 24);
        term.process(b"Hello World\r\n");
        term.process(b"ABCDEFGHIJKLMNOPQRSTUVWXYZ\r\n");
        term.process(b"1234567890\r\n");

        let before = extract_all_chars(&term);

        // Shrink to 10 columns
        term.resize(10, 24);

        let after = extract_all_chars(&term);
        assert_eq!(before, after, "Characters lost after shrink to 10 cols");

        // Grow back to 40 columns
        term.resize(40, 24);

        let after_grow = extract_all_chars(&term);
        assert_eq!(before, after_grow, "Characters lost after grow back to 40 cols");
    }

    #[test]
    fn test_process_long_line_reflow_to_small_width() {
        // Write a 50-char line in a 50-col terminal, then shrink to 5 cols.
        // All 50 characters must be preserved.
        let mut term = Terminal::new(50, 24);
        let text = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuv";
        term.process(text.as_bytes());
        term.process(b"\r\n");

        let before = extract_all_chars(&term);
        assert_eq!(before.len(), text.len());

        term.resize(5, 24);

        let after = extract_all_chars(&term);
        assert_eq!(before, after, "Characters lost after shrink to 5 cols");
    }

    #[test]
    fn test_process_resize_to_very_small_height() {
        // Write multiple lines, then shrink to 3 rows.
        // Content should go to scrollback, not be lost.
        let mut term = Terminal::new(20, 10);
        for i in 0..8 {
            term.process(format!("Line {}\r\n", i).as_bytes());
        }

        let before = extract_all_chars(&term);

        term.resize(20, 3);

        let after = extract_all_chars(&term);
        assert_eq!(before, after, "Characters lost after shrink to 3 rows");
    }

    #[test]
    fn test_process_resize_both_dimensions_small() {
        // Start at 40x10, write content, shrink to 5x3.
        // This is the scenario the user reported as corrupting text.
        let mut term = Terminal::new(40, 10);
        term.process(b"Hello World\r\n");
        term.process(b"ABCDEFGHIJKLMNOPQRSTUVWXYZ\r\n");
        term.process(b"$ ");

        let before = extract_all_chars(&term);

        term.resize(5, 3);

        let after = extract_all_chars(&term);
        assert_eq!(before, after, "Characters lost after shrink to 5x3");
    }

    #[test]
    fn test_autowrap_sets_wrapped_flag() {
        // Write enough chars to fill exactly one row and trigger autowrap.
        // The first row should be marked as wrapped.
        let mut term = Terminal::new(10, 5);
        // Write exactly 10 chars to fill row 0
        term.process(b"ABCDEFGHIJ");
        // The cursor is now at col 10 (past end). Write one more char to trigger autowrap.
        term.process(b"K");

        // Row 0 should be wrapped
        assert!(
            term.grid().row(0).unwrap().is_wrapped(),
            "Row 0 should be marked as wrapped after autowrap"
        );
        // Row 1 should have 'K'
        assert_eq!(
            term.grid().cell(1, 0).unwrap().content.char(),
            Some('K'),
            "Character after autowrap should be on row 1"
        );
    }

    #[test]
    fn test_autowrap_reflow_round_trip() {
        // Write 20 chars in a 10-col terminal (autowrap creates 2 rows).
        // Grow to 20 cols — should merge back to 1 row.
        // Shrink back to 10 cols — should split to 2 rows again.
        let mut term = Terminal::new(10, 5);
        term.process(b"ABCDEFGHIJKLMNOPQRST"); // 20 chars, fills 2 rows

        assert!(term.grid().row(0).unwrap().is_wrapped());

        let original_chars = extract_all_chars(&term);
        assert_eq!(original_chars.len(), 20);

        // Grow to 20 cols — should merge into 1 row
        term.resize(20, 5);
        let after_grow = extract_all_chars(&term);
        assert_eq!(original_chars, after_grow, "Chars lost on grow");

        let row0_text = term.grid().row(0).unwrap().to_string();
        assert_eq!(row0_text, "ABCDEFGHIJKLMNOPQRST");

        // Shrink back to 10
        term.resize(10, 5);
        let after_shrink = extract_all_chars(&term);
        assert_eq!(original_chars, after_shrink, "Chars lost on shrink back");
    }

    #[test]
    fn test_initial_small_terminal_no_corruption() {
        // Create a very small terminal and write text that exceeds its width.
        // This tests the scenario where the pane is "too small to begin with."
        let mut term = Terminal::new(5, 3);
        term.process(b"Hello World!\r\n");
        term.process(b"ABCDEFGHIJ\r\n");

        let chars = extract_all_chars(&term);
        // "Hello World!" is 12 chars, "ABCDEFGHIJ" is 10 chars — total 22
        assert_eq!(chars.len(), 22, "Expected 22 chars, got: '{}'", chars);
        assert!(chars.contains("Hello"), "Missing 'Hello' in output");
        assert!(chars.contains("World"), "Missing 'World' in output");
        assert!(chars.contains("ABCDEFGHIJ"), "Missing 'ABCDEFGHIJ' in output");
    }

    #[test]
    fn test_small_terminal_then_grow() {
        // Write into a 5x3 terminal, then grow to 40x24.
        // All content must be preserved (in scrollback or visible).
        let mut term = Terminal::new(5, 3);
        term.process(b"ABCDEFGHIJKLMNO\r\n"); // 15 chars
        term.process(b"XYZ\r\n");

        let before = extract_all_chars(&term);

        term.resize(40, 24);

        let after = extract_all_chars(&term);
        assert_eq!(before, after, "Characters lost after growing from 5x3 to 40x24");
    }

    #[test]
    fn test_pane_close_reflow_simulation() {
        // Simulate the pane lifecycle: start at full width, shrink to pane width,
        // write text that wraps, then grow back to full width.
        // This mirrors what happens when a pane is created and then closed.
        let mut term = Terminal::new(80, 24);

        // Phase 1: Resize to pane width (simulating split)
        term.resize(40, 24);
        assert_eq!(term.cols(), 40);

        // Phase 2: Write text that wraps at 40 cols
        // A 60-char line will wrap: 40 chars on row 0 (wrapped), 20 chars on row 1
        let long_line = "ABCDEFGHIJ" .repeat(6); // 60 chars
        term.process(long_line.as_bytes());
        term.process(b"\r\n");
        term.process(b"Short line\r\n");
        term.process(b"$ "); // prompt

        let before = extract_all_chars(&term);
        assert!(before.contains(&long_line), "Long line should be fully present");

        // Phase 3: Resize back to full width (simulating pane close)
        term.resize(80, 24);

        let after = extract_all_chars(&term);
        assert_eq!(before, after, "Characters lost after expanding from 40 to 80 cols");

        // The 60-char line should now fit on a single row (no wrapping needed at 80 cols)
        // Check that row 0 has the full content (either in scrollback or visible)
        let grid = term.grid();
        let mut found_full_line = false;
        for r in 0..grid.rows() {
            if let Some(row) = grid.row(r) {
                let text = row.to_string();
                if text == long_line {
                    found_full_line = true;
                    assert!(!row.is_wrapped(), "60-char line should not be wrapped at 80 cols");
                    break;
                }
            }
        }
        // Also check scrollback
        for row in grid.scrollback_rows() {
            let text = row.to_string();
            if text == long_line {
                found_full_line = true;
                assert!(!row.is_wrapped(), "60-char line in scrollback should not be wrapped at 80 cols");
                break;
            }
        }
        assert!(found_full_line, "Should find the full 60-char line on a single row after resize to 80 cols");
    }

    #[test]
    fn test_pane_reflow_multiple_wrapped_lines() {
        // Multiple auto-wrapped lines should all reflow when growing.
        let mut term = Terminal::new(20, 24);

        // Write 3 lines that wrap at 20 cols
        let line1 = "A".repeat(30); // wraps to 2 rows
        let line2 = "B".repeat(25); // wraps to 2 rows
        let line3 = "C".repeat(40); // wraps to 2 rows

        term.process(line1.as_bytes());
        term.process(b"\r\n");
        term.process(line2.as_bytes());
        term.process(b"\r\n");
        term.process(line3.as_bytes());
        term.process(b"\r\n");

        let before = extract_all_chars(&term);

        // Grow to 80 cols — all lines should unwrap
        term.resize(80, 24);

        let after = extract_all_chars(&term);
        assert_eq!(before, after, "Characters lost after expanding from 20 to 80 cols");
    }
}
