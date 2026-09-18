//! Keep live selections consistent with text writes and explicit erasure.

use super::Terminal;
use crate::grid::Cell;
use std::ops::Range;

impl Terminal {
    /// Apply the shared text erase policy and retire selections using actual
    /// damage, not the requested rectangle (protected glyphs can survive it).
    /// The callbacks add no damage queue, grid clone, or text/history scan.
    pub(super) fn erase_cells_in_region(
        &mut self,
        rows: Range<usize>,
        cols: Range<usize>,
        selective: bool,
    ) {
        let rows = rows.start.min(self.rows)..rows.end.min(self.rows);
        let cols = cols.start.min(self.cols)..cols.end.min(self.cols);
        if rows.is_empty() || cols.is_empty() {
            return;
        }
        let blank = self.erase_cell();
        let total_cols = self.cols;
        let grid = if self.alt_screen_active {
            &mut self.alt_grid
        } else {
            &mut self.grid
        };
        let selection = &mut self.selection;
        let mut damage = |rows, cols| {
            if selection
                .as_ref()
                .is_some_and(|selection| selection.intersects_region(rows, cols, total_cols))
            {
                *selection = None;
            }
        };
        grid.erase_multicells_with_damage(
            rows.clone(),
            cols.clone(),
            &blank,
            selective,
            &mut damage,
        );
        for row in rows {
            if let Some(cells) = grid.row_mut(row) {
                if selective {
                    cells.selective_erase_range_with_damage(cols.start, cols.end, &blank, |cols| {
                        damage(row..row + 1, cols)
                    });
                } else {
                    cells.erase_range_with_damage(cols.start, cols.end, &blank, |cols| {
                        damage(row..row + 1, cols)
                    });
                }
            }
        }
    }

    pub(super) fn invalidate_selection_in_region(
        &mut self,
        rows: Range<usize>,
        cols: Range<usize>,
    ) {
        // Output mutates live state only. The synchronized snapshot's visible
        // selection and text remain unchanged until that snapshot is released.
        if self
            .selection
            .as_ref()
            .is_some_and(|selection| selection.intersects_region(rows, cols, self.cols))
        {
            self.selection = None;
        }
    }

    /// Account for complete old glyphs erased by a row write. The direct range
    /// check is O(1); only existing OSC 66 descriptors in the written span need
    /// inspection when a selection survives outside that span. No text/history
    /// copy or selection-sized scan is required, and no-selection writes return
    /// before any grid lookups.
    pub(super) fn invalidate_selection_for_write(&mut self, row: usize, start: usize, end: usize) {
        if self.selection.is_none() || row >= self.rows {
            return;
        }
        let mut start = start.min(self.cols);
        let mut end = end.min(self.cols);
        if start >= end {
            return;
        }
        if self
            .grid()
            .cell(row, start)
            .is_some_and(Cell::is_wide_char_spacer)
        {
            start = start.saturating_sub(1);
        }
        if self
            .grid()
            .cell(row, end - 1)
            .is_some_and(Cell::is_wide_char)
        {
            end = end.saturating_add(1).min(self.cols);
        }
        self.invalidate_selection_in_region(row..row + 1, start..end);
        if self
            .selection
            .as_ref()
            .is_none_or(|selection| selection.bounds().1.row < 0)
        {
            return;
        }
        // Insertion or a width-changing write can erase an entire multicell
        // block whose selected leader/covered cells lie outside this row span.
        let mut col = start;
        while col < end && self.selection.is_some() {
            if let Some(region) = self.grid().multicell_region_at(row, col) {
                self.invalidate_selection_in_region(
                    region.top..region.bottom(),
                    region.left..region.right(),
                );
                col = region.right();
            } else {
                col += 1;
            }
        }
    }
}
