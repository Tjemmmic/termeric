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
        // - Scrollback is stored oldest-first: index 0 is oldest, index (len-1) is newest
        //
        // With scroll_offset = N:
        // - Rows 0..(N-1) show scrollback content (if available)
        // - Rows N.. show active buffer content
        //
        // Example: scroll_offset = 3, scrollback_len = 100
        // - row 0: 3 lines back from current → scrollback[97]
        // - row 1: 2 lines back → scrollback[98]
        // - row 2: 1 line back → scrollback[99] (newest)
        // - row 3: active buffer row 0
        // - row 4: active buffer row 1

        if row < self.scroll_offset {
            // This row shows scrollback content
            let lines_back = self.scroll_offset - row;
            if lines_back <= scrollback_len {
                let scrollback_idx = scrollback_len - lines_back;
                self.storage.scrollback_row(scrollback_idx)
                    .and_then(|r| r.get(col))
            } else {
                // Scrolled past available scrollback (shouldn't happen with clamping)
                None
            }
        } else {
            // This row shows active buffer content
            let active_row = row - self.scroll_offset;
            if active_row < self.rows {
                self.storage.cell(active_row, col)
            } else {
                None
            }
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
        let old_len = self.storage.scrollback_len();
        let was_at_max = self.scroll_offset > 0 && self.scroll_offset >= old_len;

        self.storage.scroll_up();
        let new_len = self.storage.scrollback_len();

        // If user is scrolled into history, maintain their view position
        if self.scroll_offset > 0 {
            if was_at_max {
                // User was at max scroll - keep them at max (viewing oldest content)
                self.scroll_offset = new_len;
            } else if new_len > old_len {
                // Scrollback grew - increment offset to maintain position
                self.scroll_offset += 1;
            }
            // If scrollback didn't grow (at capacity), content shifts but we can't
            // maintain the exact same view since the oldest line was deleted
        }
    }

    /// Scroll the grid down by one line (reverse scroll)
    pub fn scroll_down(&mut self) {
        self.storage.scroll_down();
    }

    /// Scroll a region up by one line
    pub fn scroll_region_up(&mut self, top: usize, bottom: usize) {
        self.storage.scroll_region_up(top, bottom);
    }

    /// Scroll a region down by one line
    pub fn scroll_region_down(&mut self, top: usize, bottom: usize) {
        self.storage.scroll_region_down(top, bottom);
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

    /// Set the scrollback limit
    pub fn set_scrollback_limit(&mut self, limit: usize) {
        self.storage.set_scrollback_limit(limit);
        self.scroll_offset = self.scroll_offset.min(self.storage.scrollback_len());
    }

    /// Get a row from the grid (accounting for scroll offset)
    pub fn row(&self, row: usize) -> Option<&Row> {
        self.storage.row(row)
    }

    /// Get a row from the grid using a scrollback-aware offset
    /// 0..rows-1 for visible rows, negative for scrollback (e.g. -1 is newest scrollback)
    pub fn row_at_offset(&self, offset: i32) -> Option<&Row> {
        self.storage.row_at_offset(offset)
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
