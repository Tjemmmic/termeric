//! Terminal grid management
//!
//! The grid module provides the data structures for representing the terminal display
//! as a 2D grid of cells.

mod cell;
mod hyperlink;
mod resize_tracking;
mod row;
mod storage;

pub use cell::{
    Cell, CellAttrs, CellContent, CellFlags, Color, ColorSpec, MultiCell, MultiCellAlignment,
};
pub use hyperlink::Hyperlink;
pub(crate) use hyperlink::HyperlinkRegistry;
pub use resize_tracking::ResizeTrackingToken;
pub use row::Row;
pub(crate) use storage::GridMemoryUsage;
pub(crate) use storage::ResizeOutcome;
pub use storage::{CellRef, DEFAULT_SCROLLBACK_MEMORY_BYTES, RowView};

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicU64, Ordering};
use storage::GridStorage;

static NEXT_GRID_ID: AtomicU64 = AtomicU64::new(1);

/// How many caller points a resize carries through a reflow: a selection's
/// two ends and the start of a command still being typed.
pub(crate) const REFLOW_POINTS: usize = 3;

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
    /// Whether the next character should trigger a line wrap.
    ///
    /// Set when writing to the last column — the cursor stays at `cols-1`
    /// but the next printable character will wrap to the beginning of the
    /// next line before being placed. Cleared by any explicit cursor
    /// movement (CR, CUP, CUF, etc.).
    pub wrap_pending: bool,
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
    /// Instance identity prevents reset/replacement from reusing a revision.
    identity: u64,
    content_revision: u64,
    row_mapping_revision: u64,
    resize_tracking: Option<resize_tracking::ResizeTracking>,
}

/// Validated rectangle occupied by one OSC 66 multicell text block.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MultiCellRegion {
    pub top: usize,
    pub left: usize,
    pub descriptor: MultiCell,
}

impl MultiCellRegion {
    pub fn bottom(self) -> usize {
        self.top + self.descriptor.rows()
    }

    pub fn right(self) -> usize {
        self.left + self.descriptor.columns()
    }

    pub fn contains(self, row: usize, col: usize) -> bool {
        row >= self.top && row < self.bottom() && col >= self.left && col < self.right()
    }
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
            identity: NEXT_GRID_ID.fetch_add(1, Ordering::Relaxed),
            content_revision: 0,
            row_mapping_revision: 0,
            resize_tracking: None,
        }
    }

    /// O(1) stamp for text/attribute/width changes and row movement. Mutable
    /// cell/row access conservatively invalidates it, even for a no-op write.
    /// Cursor movement and viewport scrolling do not change searchable text.
    pub fn content_revision(&self) -> (u64, u64) {
        (self.identity, self.content_revision)
    }

    /// O(1) stamp for operations that can move or remove signed source rows.
    /// In-place cell edits preserve coordinates; reset creates a new identity.
    pub fn row_mapping_revision(&self) -> (u64, u64) {
        (self.identity, self.row_mapping_revision)
    }

    fn mark_rows_changed(&mut self) {
        self.content_revision = self.content_revision.wrapping_add(1);
        self.row_mapping_revision = self.row_mapping_revision.wrapping_add(1);
    }

    /// Capture the cells exactly as they are currently presented, without
    /// copying scrollback history. This keeps synchronized-output snapshots
    /// bounded by the visible viewport rather than the history limit.
    pub(crate) fn visible_snapshot(&self) -> Self {
        let mut snapshot = Self::new(self.cols, self.rows);
        for row in 0..self.rows {
            for col in 0..self.cols {
                let (Some(source), Some(target)) =
                    (self.cell_scrolled(row, col), snapshot.cell_mut(row, col))
                else {
                    continue;
                };
                *target = (*source).clone();
            }
            let offset = i32::try_from(row)
                .ok()
                .zip(i32::try_from(self.scroll_offset).ok())
                .and_then(|(row, scroll)| row.checked_sub(scroll));
            if let Some(source) = offset.and_then(|offset| self.storage.row_at_offset(offset))
                && let Some(target) = snapshot.row_mut(row)
            {
                target.set_wrapped(source.is_wrapped());
                if source.has_wide_wrap_padding() {
                    target.mark_wide_wrap_padding();
                }
            }
        }
        snapshot.repair_multicells(&Cell::default());

        snapshot.cursor = self.cursor;
        // A terminal cursor is not presented while the user is viewing
        // history. The snapshot has no history of its own, so preserve that
        // visual rule explicitly.
        snapshot.cursor.visible &= self.scroll_offset == 0;
        snapshot
    }

    /// Resize the grid to new dimensions, reflowing text when columns change
    pub fn resize(&mut self, cols: usize, rows: usize) {
        self.resize_with_row_shift(cols, rows);
    }

    /// Return the exact number of viewport rows archived during resize. This
    /// is a selection translation only when the column count is unchanged.
    pub(crate) fn resize_with_row_shift(&mut self, cols: usize, rows: usize) -> usize {
        self.resize_with_points(cols, rows, &mut [None; REFLOW_POINTS])
            .archived_rows
    }

    /// Resize, carrying each of `points` (row, column) to where its cell
    /// went, or `None` when the cell was not retained.
    pub(crate) fn resize_with_points(
        &mut self,
        cols: usize,
        rows: usize,
        points: &mut [Option<(i32, usize)>; REFLOW_POINTS],
    ) -> ResizeOutcome {
        let mut tracking = self.take_valid_resize_tracking();
        if self.cols != cols
            && let Some(tracking) = &mut tracking
            && matches!(
                tracking.selection_type,
                crate::SelectionType::Block | crate::SelectionType::Lines
            )
        {
            tracking.points[1] = None;
        }
        if self.cols != cols || self.rows != rows {
            self.mark_rows_changed();
        }
        let ((new_row, new_col), outcome) = if let Some(tracking) = &mut tracking {
            let mut combined = [
                points[0],
                points[1],
                points[2],
                tracking.points[0],
                tracking.points[1],
            ];
            let result = self.storage.resize_with_points(
                cols,
                rows,
                self.cursor.row,
                self.cursor.col,
                &mut combined,
            );
            *points = [combined[0], combined[1], combined[2]];
            tracking.points = [combined[3], combined[4]];
            result
        } else {
            self.storage
                .resize_with_points(cols, rows, self.cursor.row, self.cursor.col, points)
        };
        self.cols = cols;
        self.rows = rows;
        self.cursor.row = new_row;
        self.cursor.col = new_col;
        self.cursor.wrap_pending = false;

        // Clamp scroll offset
        self.scroll_offset = self.scroll_offset.min(self.storage.scrollback_len());
        self.restore_resize_tracking(tracking);
        outcome
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
    pub fn cell_scrolled(&self, row: usize, col: usize) -> Option<CellRef<'_>> {
        if self.scroll_offset == 0 {
            // Not scrolled - return visible cell from active buffer
            return self.storage.cell(row, col).map(CellRef::Borrowed);
        }

        let scrollback_len = self.storage.scrollback_len();
        if scrollback_len == 0 {
            return self.storage.cell(row, col).map(CellRef::Borrowed);
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
                self.storage.scrollback_cell(scrollback_idx, col)
            } else {
                // Scrolled past available scrollback (shouldn't happen with clamping)
                None
            }
        } else {
            // This row shows active buffer content
            let active_row = row - self.scroll_offset;
            if active_row < self.rows {
                self.storage.cell(active_row, col).map(CellRef::Borrowed)
            } else {
                None
            }
        }
    }

    /// Get a mutable reference to a cell at the given position
    pub(crate) fn cell_mut(&mut self, row: usize, col: usize) -> Option<&mut Cell> {
        let cell = self.storage.cell_mut(row, col)?;
        self.content_revision = self.content_revision.wrapping_add(1);
        Some(cell)
    }

    /// Resolve and validate the complete active-screen OSC 66 block covering
    /// one cell. Malformed or clipped descriptors do not gain authority over
    /// neighboring cells.
    pub fn multicell_region_at(&self, row: usize, col: usize) -> Option<MultiCellRegion> {
        let descriptor = self.cell(row, col)?.multicell;
        if !descriptor.is_present() {
            return None;
        }
        let top = row.checked_sub(descriptor.relative_row())?;
        let left = col.checked_sub(descriptor.relative_col())?;
        let leader = self.cell(top, left)?.multicell;
        if !leader.is_leader()
            || !leader.has_same_layout(descriptor)
            || leader.rows() > self.rows.saturating_sub(top)
            || leader.columns() > self.cols.saturating_sub(left)
            || self.cell(top, left)?.content.is_empty()
        {
            return None;
        }
        for relative_row in 0..leader.rows() {
            for relative_col in 0..leader.columns() {
                let expected = leader.at(relative_col, relative_row)?;
                let cell = self.cell(top + relative_row, left + relative_col)?;
                if cell.multicell != expected
                    || ((relative_row != 0 || relative_col != 0) && !cell.content.is_empty())
                {
                    return None;
                }
            }
        }
        Some(MultiCellRegion {
            top,
            left,
            descriptor: leader,
        })
    }

    /// Clear one complete validated multicell block. A top-row partial
    /// overwrite can request ordinary space content, while leader overwrites
    /// and erasures use the supplied BCE blank.
    pub(crate) fn clear_multicell_at(
        &mut self,
        row: usize,
        col: usize,
        blank: &Cell,
        replace_with_spaces: bool,
    ) -> bool {
        self.clear_multicell_at_with_damage(row, col, blank, replace_with_spaces, |_, _| {})
    }

    pub(crate) fn clear_multicell_at_with_damage(
        &mut self,
        row: usize,
        col: usize,
        blank: &Cell,
        replace_with_spaces: bool,
        mut damage: impl FnMut(std::ops::Range<usize>, std::ops::Range<usize>),
    ) -> bool {
        let Some(region) = self.multicell_region_at(row, col) else {
            if let Some(cell) = self.cell_mut(row, col)
                && cell.multicell.is_present()
            {
                damage(row..row + 1, col..col + 1);
                *cell = blank.clone();
            }
            return false;
        };
        damage(region.top..region.bottom(), region.left..region.right());
        for target_row in region.top..region.bottom() {
            for target_col in region.left..region.right() {
                if let Some(cell) = self.cell_mut(target_row, target_col) {
                    if replace_with_spaces {
                        cell.set_char(' ');
                    } else {
                        *cell = blank.clone();
                    }
                }
            }
        }
        true
    }

    /// Erase every complete multicell block intersecting an active-screen
    /// rectangle. The rectangle is clipped to the grid; malformed descriptors
    /// inside it are cleared individually and never authorize neighboring
    /// mutations.
    /// Report complete blocks (or individual malformed descriptors) that the
    /// existing erase policy removes, without retaining a second damage list.
    pub(crate) fn erase_multicells_with_damage(
        &mut self,
        rows: std::ops::Range<usize>,
        cols: std::ops::Range<usize>,
        blank: &Cell,
        preserve_protected: bool,
        mut damage: impl FnMut(std::ops::Range<usize>, std::ops::Range<usize>),
    ) -> usize {
        let (top, bottom, left, right) = (rows.start, rows.end, cols.start, cols.end);
        let bottom = bottom.min(self.rows);
        let right = right.min(self.cols);
        if top >= bottom || left >= right {
            return 0;
        }

        let mut regions = HashSet::new();
        let mut malformed = Vec::new();
        for row in top..bottom {
            for col in left..right {
                let Some(cell) = self.cell(row, col) else {
                    continue;
                };
                if !cell.multicell.is_present() {
                    continue;
                }
                if let Some(region) = self.multicell_region_at(row, col) {
                    if !preserve_protected
                        || !self
                            .cell(region.top, region.left)
                            .is_some_and(|cell| cell.flags.contains(CellFlags::PROTECTED))
                    {
                        regions.insert((region.top, region.left));
                    }
                } else if !preserve_protected || !cell.flags.contains(CellFlags::PROTECTED) {
                    malformed.push((row, col));
                }
            }
        }

        for (row, col) in malformed {
            damage(row..row + 1, col..col + 1);
            if let Some(cell) = self.cell_mut(row, col) {
                *cell = blank.clone();
            }
        }
        for (row, col) in regions.iter().copied() {
            self.clear_multicell_at_with_damage(row, col, blank, false, &mut damage);
        }
        regions.len()
    }

    /// Remove descriptors made invalid by row-local edits, clipping, or
    /// resize. Complete blocks are retained; every other descriptor becomes a
    /// normal BCE blank so a later cell can never resolve into stale content.
    pub(crate) fn repair_multicells(&mut self, blank: &Cell) {
        self.repair_multicells_with_damage(blank, |_, _| {});
    }

    pub(crate) fn repair_multicells_with_damage(
        &mut self,
        blank: &Cell,
        mut damage: impl FnMut(std::ops::Range<usize>, std::ops::Range<usize>),
    ) {
        let mut tracking = self.take_valid_resize_tracking();
        let mut valid = HashMap::new();
        let mut has_descriptors = false;
        for row in 0..self.rows {
            if !self.row_mut(row).is_some_and(Row::has_multicells) {
                continue;
            }
            let Some(cells) = self.row(row) else {
                continue;
            };
            for (col, cell) in cells.iter_enumerated().take(self.cols) {
                has_descriptors |= cell.multicell.is_present();
                if cell.multicell.is_leader()
                    && let Some(region) = self.multicell_region_at(row, col)
                {
                    valid.insert((region.top, region.left), region.descriptor);
                }
            }
        }
        // Ordinary scrolling calls this after every line. If the first pass
        // found no descriptors, there is nothing to repair: avoid a second
        // full-screen scan. Do not use `valid.is_empty()` here, since orphaned
        // or malformed descriptors must still be erased even without a leader.
        if !has_descriptors {
            self.restore_resize_tracking(tracking);
            return;
        }
        for row in 0..self.rows {
            for col in 0..self.cols {
                let keep = self.cell(row, col).is_none_or(|cell| {
                    if !cell.multicell.is_present() {
                        return true;
                    }
                    row.checked_sub(cell.multicell.relative_row())
                        .zip(col.checked_sub(cell.multicell.relative_col()))
                        .and_then(|anchor| valid.get(&anchor))
                        .is_some_and(|leader| {
                            // An orphan from a larger/different block can point
                            // back to an otherwise valid leader. Anchor identity
                            // alone does not make it part of that rectangle.
                            leader.at(cell.multicell.relative_col(), cell.multicell.relative_row())
                                == Some(cell.multicell)
                        })
                });
                if !keep && let Some(cell) = self.cell_mut(row, col) {
                    damage(row..row + 1, col..col + 1);
                    *cell = blank.clone();
                    if let Some(tracking) = &mut tracking {
                        tracking.damage(row..row + 1, col..col + 1, self.cols);
                    }
                }
            }
        }
        self.restore_resize_tracking(tracking);
    }

    /// Erase blocks split by a horizontal structural-edit boundary.
    pub(crate) fn erase_multicells_crossing_row(
        &mut self,
        boundary: usize,
        left: usize,
        right: usize,
        blank: &Cell,
        mut damage: impl FnMut(std::ops::Range<usize>, std::ops::Range<usize>),
    ) {
        if boundary == 0 || boundary >= self.rows || left >= right {
            return;
        }
        let mut regions = HashSet::new();
        for row in [boundary - 1, boundary] {
            for col in left..right.min(self.cols) {
                if let Some(region) = self.multicell_region_at(row, col)
                    && region.top < boundary
                    && region.bottom() > boundary
                    && region.left < right
                    && region.right() > left
                {
                    regions.insert((region.top, region.left));
                }
            }
        }
        for (row, col) in regions {
            self.clear_multicell_at_with_damage(row, col, blank, false, &mut damage);
        }
    }

    /// Get the current cursor state
    pub fn cursor(&self) -> Cursor {
        self.cursor
    }

    /// Set the cursor position
    ///
    /// Any explicit cursor movement clears the pending-wrap state.
    pub fn set_cursor(&mut self, row: usize, col: usize) {
        self.cursor.row = row.min(self.rows.saturating_sub(1));
        self.cursor.col = col.min(self.cols.saturating_sub(1));
        self.cursor.wrap_pending = false;
    }

    /// Set the wrap-pending flag on the cursor
    pub(crate) fn set_wrap_pending(&mut self, pending: bool) {
        self.cursor.wrap_pending = pending;
    }

    /// Set cursor visibility
    pub(crate) fn set_cursor_visible(&mut self, visible: bool) {
        self.cursor.visible = visible;
    }

    /// Scroll the grid up by one line, pushing the top line to scrollback
    pub fn scroll_up(&mut self) -> usize {
        self.mark_rows_changed();
        let evicted = self.storage.scroll_up();
        self.preserve_history_view_after_append(usize::from(self.rows > 0));
        evicted
    }

    /// Copy the active viewport into compact history without moving it.
    pub(crate) fn archive_visible(&mut self) -> usize {
        self.mark_rows_changed();
        let evicted = self.storage.archive_visible();
        self.preserve_history_view_after_append(self.rows);
        evicted
    }

    /// History coordinates are relative to its newest row, even when an
    /// append evicts old rows and the total length stays constant or shrinks.
    /// Follow a retained viewing anchor; only eviction clamps it to the oldest
    /// remaining row. Offset zero deliberately continues following live output.
    fn preserve_history_view_after_append(&mut self, appended: usize) {
        if self.scroll_offset > 0 {
            self.scroll_offset = self
                .scroll_offset
                .saturating_add(appended)
                .min(self.storage.scrollback_len());
        }
    }

    /// Scroll the grid down by one line (reverse scroll)
    pub fn scroll_down(&mut self) {
        self.mark_rows_changed();
        self.storage.scroll_down();
    }

    /// Scroll a region up by one line
    pub fn scroll_region_up(&mut self, top: usize, bottom: usize) -> usize {
        self.mark_rows_changed();
        let evicted = self.storage.scroll_region_up(top, bottom);
        if top == 0 && bottom > 0 && bottom <= self.rows {
            self.preserve_history_view_after_append(1);
        }
        evicted
    }

    /// Scroll a region down by one line
    pub fn scroll_region_down(&mut self, top: usize, bottom: usize) {
        self.mark_rows_changed();
        self.storage.scroll_region_down(top, bottom);
    }

    pub(crate) fn scroll_rect_with_damage(
        &mut self,
        rows: std::ops::Range<usize>,
        cols: std::ops::Range<usize>,
        blank: &Cell,
        down: bool,
        mut damage: impl FnMut(std::ops::Range<usize>, std::ops::Range<usize>),
    ) {
        let report = |row, cols| damage(row..row + 1, cols);
        if down {
            self.storage
                .scroll_rect_down(rows.start, rows.end, cols.start, cols.end, blank, report);
        } else {
            self.storage
                .scroll_rect_up(rows.start, rows.end, cols.start, cols.end, blank, report);
        }
        self.mark_rows_changed();
    }

    /// Clear all cells in the visible grid
    pub fn clear(&mut self) {
        self.content_revision = self.content_revision.wrapping_add(1);
        self.storage.clear();
        self.cursor = Cursor {
            visible: self.cursor.visible,
            blinking: self.cursor.blinking,
            ..Default::default()
        };
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
        if self.scrollback_len() > 0 {
            self.mark_rows_changed();
        }
        self.storage.clear_scrollback();
        self.scroll_offset = 0;
    }

    /// Set the scrollback limit
    pub fn set_scrollback_limit(&mut self, limit: usize) {
        let previous_len = self.scrollback_len();
        self.storage.set_scrollback_limit(limit);
        if self.scrollback_len() != previous_len {
            self.mark_rows_changed();
        }
        self.scroll_offset = self.scroll_offset.min(self.storage.scrollback_len());
    }

    /// Set the maximum estimated memory owned by archived scrollback rows.
    pub(crate) fn set_scrollback_byte_limit(&mut self, limit: usize) {
        let previous_len = self.scrollback_len();
        self.storage.set_scrollback_byte_limit(limit);
        if self.scrollback_len() != previous_len {
            self.mark_rows_changed();
        }
        self.scroll_offset = self.scroll_offset.min(self.storage.scrollback_len());
    }

    /// Return memory diagnostics for visible and archived row storage.
    pub fn memory_usage(&self) -> GridMemoryUsage {
        self.storage.memory_usage()
    }

    pub(crate) fn collect_hyperlink_ids(&self, identifiers: &mut HashSet<u32>) {
        self.storage.collect_hyperlink_ids(identifiers);
    }

    /// Get a row from the grid (accounting for scroll offset)
    pub fn row(&self, row: usize) -> Option<&Row> {
        self.storage.row(row)
    }

    /// Get a row from the grid using a scrollback-aware offset
    /// 0..rows-1 for visible rows, negative for scrollback (e.g. -1 is newest scrollback)
    pub fn row_at_offset(&self, offset: i32) -> Option<RowView<'_>> {
        self.storage.row_at_offset(offset)
    }

    /// Get a mutable row from the grid
    pub fn row_mut(&mut self, row: usize) -> Option<&mut Row> {
        let row = self.storage.row_mut(row)?;
        self.content_revision = self.content_revision.wrapping_add(1);
        Some(row)
    }

    #[cfg(test)]
    pub(crate) fn iter_cells(&self) -> impl Iterator<Item = (usize, usize, &Cell)> {
        (0..self.rows).flat_map(move |row| {
            (0..self.cols).filter_map(move |col| self.cell(row, col).map(|cell| (row, col, cell)))
        })
    }

    /// Scrollback then visible rows, one line per row, with SGR and other
    /// escapes that reproduce each row's styling when replayed. Blank rows
    /// are kept, so the last `rows()` lines are exactly the screen.
    pub fn styled_lines(&self) -> Vec<String> {
        self.storage
            .iter_all()
            .map(|row| row.to_ansi_string())
            .collect()
    }

    /// Scrollback then visible rows as plain text, one line per row without
    /// trailing spaces, and without the blank rows below the last text.
    pub fn plain_lines(&self) -> Vec<String> {
        let mut lines: Vec<String> = self
            .storage
            .iter_all()
            .map(|row| row.plain_text(self.cols).trim_end().to_owned())
            .collect();
        while lines.last().is_some_and(String::is_empty) {
            lines.pop();
        }
        lines
    }

    /// Iterate over scrollback rows (oldest first) for persistence
    pub fn scrollback_rows(&self) -> impl Iterator<Item = RowView<'_>> {
        self.storage.iter_scrollback()
    }
}

#[cfg(test)]
mod tests;
