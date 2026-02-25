//! Grid storage with scrollback buffer

use super::cell::Cell;
use super::row::Row;
use std::collections::VecDeque;

/// Default maximum scrollback lines
const DEFAULT_SCROLLBACK_LIMIT: usize = 10_000;

/// Grid storage manages the visible grid and scrollback buffer
pub struct GridStorage {
    /// Visible rows
    rows: Vec<Row>,
    /// Scrollback buffer (historical lines)
    scrollback: VecDeque<Row>,
    /// Maximum scrollback lines
    scrollback_limit: usize,
    /// Number of columns
    cols: usize,
}

impl GridStorage {
    /// Create new grid storage
    pub fn new(cols: usize, rows: usize) -> Self {
        Self {
            rows: (0..rows).map(|_| Row::new(cols)).collect(),
            scrollback: VecDeque::new(),
            scrollback_limit: DEFAULT_SCROLLBACK_LIMIT,
            cols,
        }
    }

    /// Get the number of visible rows
    pub fn visible_rows(&self) -> usize {
        self.rows.len()
    }

    /// Get the number of columns
    pub fn cols(&self) -> usize {
        self.cols
    }

    /// Set the scrollback limit
    pub fn set_scrollback_limit(&mut self, limit: usize) {
        self.scrollback_limit = limit;
        while self.scrollback.len() > limit {
            self.scrollback.pop_front();
        }
    }

    /// Get a reference to a cell
    pub fn cell(&self, row: usize, col: usize) -> Option<&Cell> {
        self.rows.get(row)?.get(col)
    }

    /// Get a mutable reference to a cell
    pub fn cell_mut(&mut self, row: usize, col: usize) -> Option<&mut Cell> {
        self.rows.get_mut(row)?.get_mut(col)
    }

    /// Get a reference to a row
    pub fn row(&self, row: usize) -> Option<&Row> {
        self.rows.get(row)
    }

    /// Get a mutable reference to a row
    pub fn row_mut(&mut self, row: usize) -> Option<&mut Row> {
        self.rows.get_mut(row)
    }

    /// Scroll the grid up by one line
    ///
    /// The top line is pushed to scrollback, and a new empty line appears at the bottom.
    pub fn scroll_up(&mut self) {
        if self.rows.is_empty() {
            return;
        }

        // Push top row to scrollback
        let top_row = self.rows.remove(0);
        if self.scrollback.len() >= self.scrollback_limit {
            self.scrollback.pop_front();
        }
        self.scrollback.push_back(top_row);

        // Add new empty row at bottom
        self.rows.push(Row::new(self.cols));
    }

    /// Scroll the grid down by one line (reverse scroll)
    ///
    /// A new empty line appears at the top, and the bottom line is discarded.
    pub fn scroll_down(&mut self) {
        if self.rows.is_empty() {
            return;
        }

        // Remove bottom row (discarded)
        self.rows.pop();

        // Add new empty row at top
        self.rows.insert(0, Row::new(self.cols));
    }

    /// Scroll a region up by one line
    pub fn scroll_region_up(&mut self, top: usize, bottom: usize) {
        if top >= bottom || bottom > self.rows.len() {
            return;
        }

        // Remove the top line of the region
        let removed = self.rows.remove(top);

        // Insert new line at bottom of region
        self.rows.insert(bottom - 1, Row::new(self.cols));

        // Optionally push to scrollback if top == 0
        if top == 0 {
            if self.scrollback.len() >= self.scrollback_limit {
                self.scrollback.pop_front();
            }
            self.scrollback.push_back(removed);
        }
    }

    /// Scroll a region down by one line
    pub fn scroll_region_down(&mut self, top: usize, bottom: usize) {
        if top >= bottom || bottom > self.rows.len() {
            return;
        }

        // Remove the bottom line of the region
        self.rows.remove(bottom - 1);

        // Insert new line at top of region
        self.rows.insert(top, Row::new(self.cols));
    }

    /// Clear all visible cells
    pub fn clear(&mut self) {
        for row in &mut self.rows {
            row.clear();
        }
    }

    /// Clear a specific row
    pub fn clear_row(&mut self, row: usize) {
        if let Some(r) = self.rows.get_mut(row) {
            r.clear();
        }
    }

    /// Resize the grid
    pub fn resize(&mut self, cols: usize, rows: usize) {
        // Resize existing rows
        for row in &mut self.rows {
            row.resize(cols);
        }

        // Resize scrollback rows
        for row in &mut self.scrollback {
            row.resize(cols);
        }

        // Adjust number of rows
        while self.rows.len() < rows {
            self.rows.push(Row::new(cols));
        }
        while self.rows.len() > rows {
            // Push extra rows to scrollback
            if let Some(row) = self.rows.first().cloned() {
                if self.scrollback.len() >= self.scrollback_limit {
                    self.scrollback.pop_front();
                }
                self.scrollback.push_back(row);
            }
            self.rows.remove(0);
        }

        self.cols = cols;
    }

    /// Get the number of lines in scrollback
    pub fn scrollback_len(&self) -> usize {
        self.scrollback.len()
    }

    /// Get a row from scrollback (0 = oldest)
    pub fn scrollback_row(&self, idx: usize) -> Option<&Row> {
        self.scrollback.get(idx)
    }

    /// Clear the scrollback buffer
    pub fn clear_scrollback(&mut self) {
        self.scrollback.clear();
    }

    /// Get a row, accounting for scroll offset
    ///
    /// - Positive offset: visible grid (0 = top visible row)
    /// - Negative offset: scrollback (-1 = most recent scrollback line)
    pub fn row_at_offset(&self, offset: i32) -> Option<&Row> {
        if offset >= 0 {
            self.rows.get(offset as usize)
        } else {
            let scrollback_idx = self.scrollback.len() as i32 + offset;
            if scrollback_idx >= 0 {
                self.scrollback.get(scrollback_idx as usize)
            } else {
                None
            }
        }
    }

    /// Iterate over all scrollback rows (oldest first)
    pub fn iter_scrollback(&self) -> impl Iterator<Item = &Row> {
        self.scrollback.iter()
    }

    /// Iterate over all visible rows
    pub fn iter_visible(&self) -> impl Iterator<Item = &Row> {
        self.rows.iter()
    }

    /// Iterate over all visible rows mutably
    pub fn iter_visible_mut(&mut self) -> impl Iterator<Item = &mut Row> {
        self.rows.iter_mut()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_storage_creation() {
        let storage = GridStorage::new(80, 24);
        assert_eq!(storage.visible_rows(), 24);
        assert_eq!(storage.cols(), 80);
        assert_eq!(storage.scrollback_len(), 0);
    }

    #[test]
    fn test_scroll_up() {
        let mut storage = GridStorage::new(80, 3);

        // Set content in first row
        if let Some(cell) = storage.cell_mut(0, 0) {
            cell.set_char('A');
        }

        storage.scroll_up();

        // First row should now be in scrollback
        assert_eq!(storage.scrollback_len(), 1);

        // New first row should be empty
        assert!(storage.cell(0, 0).unwrap().is_empty());
    }

    #[test]
    fn test_scrollback_limit() {
        let mut storage = GridStorage::new(80, 1);
        storage.set_scrollback_limit(3);

        for _ in 0..5 {
            storage.scroll_up();
        }

        assert_eq!(storage.scrollback_len(), 3);
    }

    #[test]
    fn test_resize() {
        let mut storage = GridStorage::new(80, 24);
        storage.resize(100, 30);
        assert_eq!(storage.visible_rows(), 30);
        assert_eq!(storage.cols(), 100);
    }
}
