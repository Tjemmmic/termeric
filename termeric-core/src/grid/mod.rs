//! Terminal grid management
//!
//! The grid module provides the data structures for representing the terminal display
//! as a 2D grid of cells.

mod cell;
mod row;
mod storage;

pub use cell::{Cell, CellContent, CellAttrs, CellFlags, Color};
pub use row::Row;

use storage::GridStorage;

/// Cursor position and state
#[derive(Clone, Copy, Debug, Default)]
pub struct Cursor {
    /// Current row (0-indexed)
    pub row: usize,
    /// Current column (0-indexed)
    pub col: usize,
    /// Whether the cursor is visible
    pub visible: bool,
    /// Whether the cursor should blink
    pub blinking: bool,
}

/// The terminal grid
///
/// Represents the visible terminal area as a 2D grid of cells.
/// Also manages the scrollback buffer for historical lines.
pub struct Grid {
    /// Grid storage (active screen + scrollback)
    storage: GridStorage,
    /// Number of columns
    cols: usize,
    /// Number of visible rows
    rows: usize,
    /// Cursor state
    cursor: Cursor,
    /// Current scroll offset into scrollback (0 = at bottom)
    scroll_offset: usize,
}

impl Grid {
    /// Create a new grid with the specified dimensions
    pub fn new(cols: usize, rows: usize) -> Self {
        Self {
            storage: GridStorage::new(cols, rows),
            cols,
            rows,
            cursor: Cursor {
                visible: true,
                blinking: true,
                ..Default::default()
            },
            scroll_offset: 0,
        }
    }

    /// Resize the grid to new dimensions
    pub fn resize(&mut self, cols: usize, rows: usize) {
        self.storage.resize(cols, rows);
        self.cols = cols;
        self.rows = rows;

        // Clamp cursor position
        self.cursor.col = self.cursor.col.min(cols.saturating_sub(1));
        self.cursor.row = self.cursor.row.min(rows.saturating_sub(1));
    }

    /// Get the number of columns
    pub fn cols(&self) -> usize {
        self.cols
    }

    /// Get the number of visible rows
    pub fn rows(&self) -> usize {
        self.rows
    }

    /// Get a reference to a cell at the given position
    pub fn cell(&self, row: usize, col: usize) -> Option<&Cell> {
        self.storage.cell(row, col)
    }

    /// Get a reference to a cell accounting for current scroll offset
    /// When scrolled up, this returns cells from scrollback buffer
    pub fn cell_scrolled(&self, row: usize, col: usize) -> Option<&Cell> {
        if self.scroll_offset == 0 {
            // Not scrolled - return visible cell from active buffer
            return self.storage.cell(row, col);
        }

        let scrollback_len = self.storage.scrollback_len();
        if scrollback_len == 0 {
            return self.storage.cell(row, col);
        }

        // When scrolled up by N lines:
        // - Screen row 0 shows what was N lines above current view
        // - If scroll_offset <= scrollback_len, we're viewing scrollback
        //
        // Scrollback is stored oldest-first: index 0 is oldest, index (len-1) is newest
        // When scroll_offset = 1, row 0 should show the newest scrollback line (index len-1)
        // When scroll_offset = scrollback_len, row 0 should show oldest (index 0)

        // How many lines into scrollback does this row fall?
        // Row 0 with offset N is N lines back
        // Row 1 with offset N is N-1 lines back, etc.
        let lines_back = self.scroll_offset.saturating_sub(row);

        if lines_back > 0 && lines_back <= scrollback_len {
            // This row is in scrollback
            // lines_back=1 means newest scrollback (index scrollback_len-1)
            // lines_back=scrollback_len means oldest (index 0)
            let scrollback_idx = scrollback_len - lines_back;
            self.storage.scrollback_row(scrollback_idx)
                .and_then(|r| r.get(col))
        } else if lines_back == 0 {
            // This row is in active buffer
            // scroll_offset = 3, scrollback_len = 5, rows = 10
            // row 0: lines_back = 3, in scrollback at idx 5-3=2
            // row 1: lines_back = 2, in scrollback at idx 5-2=3
            // row 2: lines_back = 1, in scrollback at idx 5-1=4 (newest)
            // row 3: lines_back = 0, active buffer row 0
            // row 4: lines_back = 0, active buffer row 1
            let active_row = row - self.scroll_offset.min(scrollback_len);
            if active_row < self.rows {
                self.storage.cell(active_row, col)
            } else {
                None
            }
        } else {
            // lines_back > scrollback_len - we've scrolled past all scrollback (shouldn't happen with proper clamping)
            None
        }
    }

    /// Get a mutable reference to a cell at the given position
    pub fn cell_mut(&mut self, row: usize, col: usize) -> Option<&mut Cell> {
        self.storage.cell_mut(row, col)
    }

    /// Get the current cursor state
    pub fn cursor(&self) -> Cursor {
        self.cursor
    }

    /// Set the cursor position
    pub fn set_cursor(&mut self, row: usize, col: usize) {
        self.cursor.row = row.min(self.rows.saturating_sub(1));
        self.cursor.col = col.min(self.cols.saturating_sub(1));
    }

    /// Set cursor visibility
    pub fn set_cursor_visible(&mut self, visible: bool) {
        self.cursor.visible = visible;
    }

    /// Scroll the grid up by one line, pushing the top line to scrollback
    pub fn scroll_up(&mut self) {
        self.storage.scroll_up();
    }

    /// Scroll the grid down by one line (reverse scroll)
    pub fn scroll_down(&mut self) {
        self.storage.scroll_down();
    }

    /// Clear all cells in the visible grid
    pub fn clear(&mut self) {
        self.storage.clear();
        self.cursor = Cursor {
            visible: self.cursor.visible,
            blinking: self.cursor.blinking,
            ..Default::default()
        };
    }

    /// Clear a specific row
    pub fn clear_row(&mut self, row: usize) {
        self.storage.clear_row(row);
    }

    /// Get the scrollback length
    pub fn scrollback_len(&self) -> usize {
        self.storage.scrollback_len()
    }

    /// Set the scroll offset (for viewing scrollback)
    pub fn set_scroll_offset(&mut self, offset: usize) {
        self.scroll_offset = offset.min(self.scrollback_len());
    }

    /// Get the current scroll offset
    pub fn scroll_offset(&self) -> usize {
        self.scroll_offset
    }

    /// Clear the scrollback buffer
    pub fn clear_scrollback(&mut self) {
        self.storage.clear_scrollback();
        self.scroll_offset = 0;
    }

    /// Get a row from the grid (accounting for scroll offset)
    pub fn row(&self, row: usize) -> Option<&Row> {
        self.storage.row(row)
    }

    /// Get a mutable row from the grid
    pub fn row_mut(&mut self, row: usize) -> Option<&mut Row> {
        self.storage.row_mut(row)
    }

    /// Iterate over all visible cells
    pub fn iter_cells(&self) -> impl Iterator<Item = (usize, usize, &Cell)> {
        (0..self.rows).flat_map(move |row| {
            (0..self.cols).filter_map(move |col| {
                self.cell(row, col).map(|cell| (row, col, cell))
            })
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_grid_creation() {
        let grid = Grid::new(80, 24);
        assert_eq!(grid.cols(), 80);
        assert_eq!(grid.rows(), 24);
    }

    #[test]
    fn test_cursor_movement() {
        let mut grid = Grid::new(80, 24);
        grid.set_cursor(10, 20);
        let cursor = grid.cursor();
        assert_eq!(cursor.row, 10);
        assert_eq!(cursor.col, 20);
    }

    #[test]
    fn test_cursor_clamping() {
        let mut grid = Grid::new(80, 24);
        grid.set_cursor(100, 100);
        let cursor = grid.cursor();
        assert_eq!(cursor.row, 23);
        assert_eq!(cursor.col, 79);
    }
}
