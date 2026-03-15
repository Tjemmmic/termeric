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

    /// Resize the grid with text reflow when columns change.
    /// Accepts cursor position and returns the new cursor position after reflow.
    pub fn resize(&mut self, new_cols: usize, new_rows: usize, cursor_row: usize, cursor_col: usize) -> (usize, usize) {
        let old_cols = self.cols;
        let mut new_cursor = (cursor_row, cursor_col);

        if new_cols != old_cols {
            // Reflow scrollback (no cursor tracking needed)
            let old_scrollback: Vec<Row> = std::mem::take(&mut self.scrollback).into();
            self.scrollback = Self::reflow_rows(old_scrollback, new_cols, None).into();

            // Reflow visible rows with cursor tracking
            let old_visible = std::mem::take(&mut self.rows);
            let cursor_info = Some((cursor_row, cursor_col));
            let (reflowed, tracked_cursor) = Self::reflow_rows_with_cursor(old_visible, new_cols, cursor_info);
            self.rows = reflowed;
            if let Some(pos) = tracked_cursor {
                new_cursor = pos;
            }
        }

        // Adjust number of visible rows
        while self.rows.len() < new_rows {
            self.rows.push(Row::new(new_cols));
        }

        // When shrinking, remove excess rows in three phases to keep the
        // cursor visible and preserve as much content as possible:
        //   1. Trim trailing empty rows (no data loss, no cursor impact)
        //   2. Push rows from the top to scrollback, but stop before
        //      pushing the cursor row itself (keeps cursor on-screen)
        //   3. If still too many, pop from the bottom (content below cursor)
        let mut rows_pushed_top = 0usize;

        // Phase 1: trim trailing empty rows
        while self.rows.len() > new_rows {
            if self.rows.last().map(|r| r.to_string().is_empty()).unwrap_or(false) {
                self.rows.pop();
            } else {
                break;
            }
        }

        // Phase 2: push from top to scrollback, preserving cursor visibility
        if self.rows.len() > new_rows {
            let excess = self.rows.len() - new_rows;
            // Don't push more rows than the cursor position — that would
            // push the cursor into scrollback and lose its correct position.
            let max_push = excess.min(new_cursor.0);
            for _ in 0..max_push {
                let row = self.rows.remove(0);
                if self.scrollback.len() >= self.scrollback_limit {
                    self.scrollback.pop_front();
                }
                self.scrollback.push_back(row);
                rows_pushed_top += 1;
            }
        }

        // Phase 3: if still too many, pop from bottom (content below cursor)
        while self.rows.len() > new_rows {
            self.rows.pop();
        }

        // Adjust cursor for rows pushed to scrollback
        if rows_pushed_top > 0 {
            new_cursor.0 = new_cursor.0.saturating_sub(rows_pushed_top);
        }

        // Clamp cursor to visible area
        new_cursor.0 = new_cursor.0.min(new_rows.saturating_sub(1));
        new_cursor.1 = new_cursor.1.min(new_cols.saturating_sub(1));

        // Enforce scrollback limit after reflow (reflow can change line count)
        while self.scrollback.len() > self.scrollback_limit {
            self.scrollback.pop_front();
        }

        self.cols = new_cols;
        new_cursor
    }

    /// Reflow a sequence of rows to a new column width.
    ///
    /// Groups consecutive wrapped rows into logical lines, flattens their cells,
    /// trims trailing empties, then re-wraps at the new width.
    /// `_cursor` is unused — this is a convenience wrapper for scrollback.
    fn reflow_rows(rows: Vec<Row>, new_cols: usize, _cursor: Option<(usize, usize)>) -> Vec<Row> {
        Self::reflow_rows_with_cursor(rows, new_cols, None).0
    }

    /// Reflow rows with optional cursor tracking.
    ///
    /// If `cursor` is Some((row, col)), tracks that position through the reflow
    /// and returns the new (row, col) in the output. This is critical for keeping
    /// the cursor at the correct position after resize, which prevents the shell's
    /// SIGWINCH response from corrupting content.
    fn reflow_rows_with_cursor(
        rows: Vec<Row>,
        new_cols: usize,
        cursor: Option<(usize, usize)>,
    ) -> (Vec<Row>, Option<(usize, usize)>) {
        let mut result: Vec<Row> = Vec::new();
        let mut new_cursor: Option<(usize, usize)> = None;

        // Group rows into logical lines using the `wrapped` flag.
        let mut line_cells: Vec<Cell> = Vec::new();
        let mut in_logical_line = false;
        // Track cursor: which cell index in the current logical line corresponds to the cursor?
        let mut cursor_cell_offset: Option<usize> = None;

        for (row_idx, row) in rows.into_iter().enumerate() {
            let is_wrapped = row.is_wrapped();
            let row_len = row.len();
            let cells = row.into_cells();

            if in_logical_line {
                // Check if cursor is in this row of the logical line
                if let Some((cr, cc)) = cursor {
                    if cr == row_idx && cursor_cell_offset.is_none() {
                        cursor_cell_offset = Some(line_cells.len() + cc.min(row_len.saturating_sub(1)));
                    }
                }
                line_cells.extend(cells);
            } else {
                // Check if cursor is in this row
                if let Some((cr, cc)) = cursor {
                    if cr == row_idx {
                        cursor_cell_offset = Some(cc.min(row_len.saturating_sub(1)));
                    }
                }
                line_cells = cells;
            }

            if is_wrapped {
                in_logical_line = true;
            } else {
                // End of logical line — reflow and emit
                let output_start = result.len();
                Self::rewrap_line(&mut result, line_cells, new_cols);

                // If cursor was in this logical line, compute its new position
                if let Some(cell_offset) = cursor_cell_offset.take() {
                    new_cursor = Some(Self::find_cursor_in_output(&result, output_start, cell_offset, new_cols));
                }

                line_cells = Vec::new();
                in_logical_line = false;
            }
        }

        // Flush any remaining logical line
        if !line_cells.is_empty() {
            let output_start = result.len();
            Self::rewrap_line(&mut result, line_cells, new_cols);

            if let Some(cell_offset) = cursor_cell_offset.take() {
                new_cursor = Some(Self::find_cursor_in_output(&result, output_start, cell_offset, new_cols));
            }
        }

        (result, new_cursor)
    }

    /// Find cursor position in the output rows given a cell offset within a logical line.
    ///
    /// `output_start` is the index of the first row in `output` that belongs to this logical line.
    /// `cell_offset` is the cursor's position as a cell index within the flattened logical line.
    fn find_cursor_in_output(
        output: &[Row],
        output_start: usize,
        cell_offset: usize,
        new_cols: usize,
    ) -> (usize, usize) {
        // The cursor was at position `cell_offset` in the flat cell buffer.
        // After re-wrapping at new_cols, row = cell_offset / new_cols, col = cell_offset % new_cols.
        // But we need to account for the trimming of trailing empty cells and
        // wide char boundary adjustments. Use a simple approach: row/col from offset.
        let row_within_line = cell_offset / new_cols;
        let col_within_line = cell_offset % new_cols;
        let output_row = output_start + row_within_line;
        // Clamp to actual output bounds
        let clamped_row = output_row.min(output.len().saturating_sub(1));
        let clamped_col = if clamped_row < output.len() {
            col_within_line.min(output[clamped_row].len().saturating_sub(1))
        } else {
            col_within_line
        };
        (clamped_row, clamped_col)
    }

    /// Re-wrap a single logical line (flat cell buffer) at the given column width
    /// and append the resulting rows to `output`.
    fn rewrap_line(output: &mut Vec<Row>, mut cells: Vec<Cell>, cols: usize) {
        use super::cell::CellFlags;

        // Trim trailing empty cells so padding doesn't inflate the line
        while cells.last().is_some_and(|c| c.is_empty() && !c.is_wide_char_spacer()) {
            cells.pop();
        }

        // Empty logical line → emit one empty row
        if cells.is_empty() {
            output.push(Row::new(cols));
            return;
        }

        // Re-wrap the cells at new_cols
        let mut pos = 0;
        while pos < cells.len() {
            let mut row_cells: Vec<Cell> = Vec::with_capacity(cols);

            while row_cells.len() < cols && pos < cells.len() {
                let cell = &cells[pos];

                if cell.flags.contains(CellFlags::WIDE_CHAR) {
                    // Wide char needs 2 cells. If it can never fit (cols < 2),
                    // skip it entirely to avoid an infinite loop.
                    if cols < 2 {
                        pos += 1;
                        if pos < cells.len() && cells[pos].is_wide_char_spacer() {
                            pos += 1;
                        }
                        continue;
                    }
                    // Only 1 slot left on this row — pad and break to next row.
                    if row_cells.len() + 2 > cols {
                        row_cells.resize_with(cols, Cell::default);
                        break;
                    }
                    row_cells.push(cells[pos].clone());
                    pos += 1;
                    // Push the spacer if it exists
                    if pos < cells.len() && cells[pos].flags.contains(CellFlags::WIDE_CHAR_SPACER) {
                        row_cells.push(cells[pos].clone());
                        pos += 1;
                    } else {
                        // Create spacer if missing
                        let mut spacer = Cell::default();
                        spacer.flags.insert(CellFlags::WIDE_CHAR_SPACER);
                        row_cells.push(spacer);
                    }
                } else if cell.is_wide_char_spacer() {
                    // Orphaned spacer (wide char was on previous row) — skip
                    pos += 1;
                } else {
                    row_cells.push(cells[pos].clone());
                    pos += 1;
                }
            }

            let is_last = pos >= cells.len();
            // Pad row to full width
            row_cells.resize_with(cols, Cell::default);
            // All rows except the last are wrapped
            output.push(Row::from_cells(row_cells, !is_last));
        }
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

}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_storage_creation() {
        let storage = GridStorage::new(80, 24);
        assert_eq!(storage.iter_visible().count(), 24);
        assert_eq!(storage.row(0).unwrap().len(), 80);
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
        storage.resize(100, 30, 0, 0);
        assert_eq!(storage.iter_visible().count(), 30);
        assert_eq!(storage.row(0).unwrap().len(), 100);
    }

    /// Helper: fill a row with characters starting from a given char
    fn fill_row(storage: &mut GridStorage, row_idx: usize, text: &str) {
        for (col, ch) in text.chars().enumerate() {
            if let Some(cell) = storage.cell_mut(row_idx, col) {
                cell.set_char(ch);
            }
        }
    }

    /// Helper: extract text content from a row
    fn row_text(storage: &GridStorage, row_idx: usize) -> String {
        storage.row(row_idx).unwrap().to_string()
    }

    #[test]
    fn test_reflow_shrink() {
        // 10-col row with "ABCDEFGHIJ" → shrink to 5 cols → 2 wrapped rows
        let mut storage = GridStorage::new(10, 1);
        fill_row(&mut storage, 0, "ABCDEFGHIJ");

        // Simulate autowrap: mark the row as wrapped since it was full
        storage.row_mut(0).unwrap().set_wrapped(true);

        // Actually we need a non-wrapped row to test shrink reflow.
        // A single non-wrapped 10-char row shrunk to 5 should produce 2 rows.
        storage.row_mut(0).unwrap().set_wrapped(false);

        storage.resize(5, 3, 0, 0);

        // Should have 3 visible rows (padded), first two contain content
        assert_eq!(storage.iter_visible().count(), 3);
        assert_eq!(row_text(&storage, 0), "ABCDE");
        assert_eq!(row_text(&storage, 1), "FGHIJ");
        // First row should be wrapped, second should not
        assert!(storage.row(0).unwrap().is_wrapped());
        assert!(!storage.row(1).unwrap().is_wrapped());
    }

    #[test]
    fn test_reflow_grow() {
        // Two 5-col wrapped rows → grow to 10 cols → single row
        let mut storage = GridStorage::new(5, 2);
        fill_row(&mut storage, 0, "ABCDE");
        fill_row(&mut storage, 1, "FGHIJ");
        storage.row_mut(0).unwrap().set_wrapped(true);

        storage.resize(10, 2, 0, 0);

        assert_eq!(row_text(&storage, 0), "ABCDEFGHIJ");
        assert!(!storage.row(0).unwrap().is_wrapped());
    }

    #[test]
    fn test_reflow_round_trip() {
        // 10 chars → shrink to 5 → grow back to 10 → same content
        let mut storage = GridStorage::new(10, 1);
        fill_row(&mut storage, 0, "ABCDEFGHIJ");

        storage.resize(5, 3, 0, 0);
        storage.resize(10, 1, 0, 0);

        assert_eq!(row_text(&storage, 0), "ABCDEFGHIJ");
    }

    #[test]
    fn test_reflow_hard_break_preserved() {
        // Two separate non-wrapped rows should stay as two rows
        let mut storage = GridStorage::new(10, 2);
        fill_row(&mut storage, 0, "Hello");
        fill_row(&mut storage, 1, "World");

        storage.resize(5, 4, 0, 0);

        assert_eq!(row_text(&storage, 0), "Hello");
        assert_eq!(row_text(&storage, 1), "World");
        assert!(!storage.row(0).unwrap().is_wrapped());
        assert!(!storage.row(1).unwrap().is_wrapped());
    }

    #[test]
    fn test_reflow_empty_rows_preserved() {
        // Empty row between content rows should be preserved
        let mut storage = GridStorage::new(10, 3);
        fill_row(&mut storage, 0, "AAA");
        // row 1 is empty
        fill_row(&mut storage, 2, "BBB");

        storage.resize(5, 5, 0, 0);

        assert_eq!(row_text(&storage, 0), "AAA");
        assert_eq!(row_text(&storage, 1), "");
        assert_eq!(row_text(&storage, 2), "BBB");
    }

    #[test]
    fn test_reflow_scrollback() {
        // Content in scrollback should also reflow
        let mut storage = GridStorage::new(10, 1);
        fill_row(&mut storage, 0, "ABCDEFGHIJ");
        storage.scroll_up(); // moves row to scrollback

        storage.resize(5, 1, 0, 0);

        // Scrollback should now have 2 rows
        assert_eq!(storage.scrollback_len(), 2);
        assert_eq!(storage.scrollback_row(0).unwrap().to_string(), "ABCDE");
        assert_eq!(storage.scrollback_row(1).unwrap().to_string(), "FGHIJ");
    }

    #[test]
    fn test_cursor_not_pushed_to_scrollback() {
        // When shrinking to very few rows, cursor must stay visible.
        // 10 rows of content, cursor at row 9 (bottom). Shrink to 3 rows.
        let mut storage = GridStorage::new(10, 10);
        for i in 0..10 {
            fill_row(&mut storage, i, &format!("LINE{}", i));
        }

        let (new_row, new_col) = storage.resize(10, 3, 9, 0);

        // Cursor should be at the bottom of the 3-row visible area
        assert_eq!(new_row, 2);
        assert_eq!(new_col, 0);
        // The visible area should contain the last 3 rows of content
        assert_eq!(row_text(&storage, 2), "LINE9");
    }

    #[test]
    fn test_cursor_preserved_on_small_reflow() {
        // Reflow 80-col content to 10 cols with only 3 visible rows.
        // Cursor at prompt on last row must stay visible.
        let mut storage = GridStorage::new(20, 5);
        fill_row(&mut storage, 0, "ABCDEFGHIJKLMNOPQRST"); // 20 chars
        fill_row(&mut storage, 1, "UVWXYZ1234567890abcd"); // 20 chars
        // rows 2-3 empty
        fill_row(&mut storage, 4, "$ "); // prompt with cursor

        let (new_row, _new_col) = storage.resize(5, 3, 4, 2);

        // Cursor must be in the visible area [0..2]
        assert!(new_row < 3, "cursor row {} is outside visible area", new_row);
    }
}
