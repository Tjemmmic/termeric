//! Terminal grid management
//!
//! The grid module provides the data structures for representing the terminal display
//! as a 2D grid of cells.

mod cell;
mod hyperlink;
mod row;
mod storage;

pub use cell::{
    Cell, CellAttrs, CellContent, CellFlags, Color, ColorSpec, MultiCell, MultiCellAlignment,
};
pub use hyperlink::{Hyperlink, HyperlinkRegistry};
pub use row::Row;
pub use storage::{CellRef, DEFAULT_SCROLLBACK_MEMORY_BYTES, GridMemoryUsage, RowView};

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicU64, Ordering};
use storage::GridStorage;

static NEXT_GRID_ID: AtomicU64 = AtomicU64::new(1);

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
        let blank = Cell {
            flags: CellFlags::DIRTY,
            ..Cell::default()
        };
        snapshot.repair_multicells(&blank);

        snapshot.cursor = self.cursor;
        // A terminal cursor is not presented while the user is viewing
        // history. The snapshot has no history of its own, so preserve that
        // visual rule explicitly.
        snapshot.cursor.visible &= self.scroll_offset == 0;
        snapshot
    }

    /// Resize the grid to new dimensions, reflowing text when columns change
    pub fn resize(&mut self, cols: usize, rows: usize) {
        if self.cols != cols || self.rows != rows {
            self.mark_rows_changed();
        }
        let (new_row, new_col) = self
            .storage
            .resize(cols, rows, self.cursor.row, self.cursor.col);
        self.cols = cols;
        self.rows = rows;
        self.cursor.row = new_row;
        self.cursor.col = new_col;
        self.cursor.wrap_pending = false;

        // Clamp scroll offset
        self.scroll_offset = self.scroll_offset.min(self.storage.scrollback_len());
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
    pub fn cell_mut(&mut self, row: usize, col: usize) -> Option<&mut Cell> {
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
                cell.flags.insert(CellFlags::DIRTY);
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
                        cell.flags.insert(CellFlags::DIRTY);
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
                cell.flags.insert(CellFlags::DIRTY);
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
                    cell.flags.insert(CellFlags::DIRTY);
                }
            }
        }
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
    pub fn set_wrap_pending(&mut self, pending: bool) {
        self.cursor.wrap_pending = pending;
    }

    /// Set cursor visibility
    pub fn set_cursor_visible(&mut self, visible: bool) {
        self.cursor.visible = visible;
    }

    /// Scroll the grid up by one line, pushing the top line to scrollback
    pub fn scroll_up(&mut self) -> usize {
        self.mark_rows_changed();
        let old_len = self.storage.scrollback_len();
        let was_at_max = self.scroll_offset > 0 && self.scroll_offset >= old_len;

        let evicted = self.storage.scroll_up();
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
        evicted
    }

    /// Copy the active viewport into compact history without moving it.
    pub(crate) fn archive_visible(&mut self) -> usize {
        self.mark_rows_changed();
        let old_len = self.storage.scrollback_len();
        let was_at_max = self.scroll_offset > 0 && self.scroll_offset >= old_len;
        let evicted = self.storage.archive_visible();
        let new_len = self.storage.scrollback_len();
        if self.scroll_offset > 0 {
            if was_at_max {
                self.scroll_offset = new_len;
            } else {
                self.scroll_offset = self
                    .scroll_offset
                    .saturating_add(new_len.saturating_sub(old_len))
                    .min(new_len);
            }
        }
        evicted
    }

    /// Scroll the grid down by one line (reverse scroll)
    pub fn scroll_down(&mut self) {
        self.mark_rows_changed();
        self.storage.scroll_down();
    }

    /// Scroll a region up by one line
    pub fn scroll_region_up(&mut self, top: usize, bottom: usize) -> usize {
        self.mark_rows_changed();
        self.storage.scroll_region_up(top, bottom)
    }

    /// Scroll a region down by one line
    pub fn scroll_region_down(&mut self, top: usize, bottom: usize) {
        self.mark_rows_changed();
        self.storage.scroll_region_down(top, bottom);
    }

    /// Scroll a partial-width rectangle up by one row.
    pub fn scroll_rect_up(
        &mut self,
        top: usize,
        bottom: usize,
        left: usize,
        right: usize,
        blank: &Cell,
    ) {
        self.storage
            .scroll_rect_up(top, bottom, left, right, blank, |_, _| {});
        self.mark_rows_changed();
    }

    /// Scroll a partial-width rectangle down by one row.
    pub fn scroll_rect_down(
        &mut self,
        top: usize,
        bottom: usize,
        left: usize,
        right: usize,
        blank: &Cell,
    ) {
        self.storage
            .scroll_rect_down(top, bottom, left, right, blank, |_, _| {});
        self.mark_rows_changed();
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

    /// Clear a specific row
    pub fn clear_row(&mut self, row: usize) {
        if row < self.rows {
            self.content_revision = self.content_revision.wrapping_add(1);
        }
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
    pub fn set_scrollback_byte_limit(&mut self, limit: usize) {
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

    /// Iterate over all visible cells
    pub fn iter_cells(&self) -> impl Iterator<Item = (usize, usize, &Cell)> {
        (0..self.rows).flat_map(move |row| {
            (0..self.cols).filter_map(move |col| self.cell(row, col).map(|cell| (row, col, cell)))
        })
    }

    /// Export scrollback + visible rows as text with ANSI color sequences for session persistence
    ///
    /// Returns all scrollback lines followed by visible lines, with ANSI escape
    /// sequences to preserve colors and attributes. Trailing empty lines and
    /// prompt-like lines are trimmed to avoid double prompts on restore.
    pub fn export_scrollback_text(&self) -> Vec<String> {
        let mut lines: Vec<String> = self
            .storage
            .iter_scrollback()
            .map(|row| row.to_ansi_string())
            .collect();
        lines.extend(self.storage.iter_visible().map(|row| row.to_ansi_string()));

        // Trim trailing empty lines
        while lines.last().is_some_and(|l| l.is_empty()) {
            lines.pop();
        }

        // Trim trailing prompt-like or logout/exit lines to avoid duplication on restore.
        // Strip ANSI escapes before checking so we match the visible text content.
        while let Some(last) = lines.last() {
            let plain = strip_ansi_escapes(last);
            let trimmed = plain.trim();
            if trimmed.is_empty()
                || trimmed == "logout"
                || trimmed == "exit"
                || is_bare_prompt(trimmed)
            {
                lines.pop();
            } else {
                break;
            }
        }

        lines
    }

    /// Iterate over scrollback rows (oldest first) for persistence
    pub fn scrollback_rows(&self) -> impl Iterator<Item = RowView<'_>> {
        self.storage.iter_scrollback()
    }
}

/// Strip ANSI escape sequences from a string, returning plain text.
fn strip_ansi_escapes(s: &str) -> String {
    let mut result = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\x1b' {
            // Skip ESC [ ... m sequences
            if chars.peek() == Some(&'[') {
                chars.next(); // consume '['
                // Consume until we hit a letter (the final byte of the sequence)
                while let Some(&next) = chars.peek() {
                    chars.next();
                    if next.is_ascii_alphabetic() {
                        break;
                    }
                }
            }
        } else {
            result.push(c);
        }
    }
    result
}

/// Check if a trimmed plain-text line looks like a bare shell prompt.
///
/// Matches common patterns like `$`, `[~]$`, `user@host:~$`, `#`, `%`, `>`
/// but NOT lines that contain actual command output after the prompt character.
fn is_bare_prompt(trimmed: &str) -> bool {
    if trimmed.is_empty() {
        return false;
    }
    // Must end with a common prompt character
    let ends_with_prompt = trimmed.ends_with('$')
        || trimmed.ends_with('#')
        || trimmed.ends_with('%')
        || trimmed.ends_with('>');

    if !ends_with_prompt {
        return false;
    }

    // Prompts are short — if the line is very long, it's likely command output
    // Allow generous length for prompts like "user@hostname:/very/long/path $"
    trimmed.len() <= 120
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grid_revisions_cover_mutation_entry_points_and_separate_row_movement() {
        type Mutation = fn(&mut Grid);
        let mutations: &[(&str, Mutation, bool)] = &[
            (
                "cell",
                |grid| grid.cell_mut(0, 0).unwrap().set_char('x'),
                false,
            ),
            (
                "row",
                |grid| grid.row_mut(0).unwrap().set_wrapped(true),
                false,
            ),
            ("clear row", |grid| grid.clear_row(0), false),
            ("clear", Grid::clear, false),
            (
                "scroll up",
                |grid| {
                    grid.scroll_up();
                },
                true,
            ),
            ("scroll down", Grid::scroll_down, true),
            (
                "region up",
                |grid| {
                    grid.scroll_region_up(0, 1);
                },
                true,
            ),
            ("region down", |grid| grid.scroll_region_down(0, 1), true),
            (
                "rectangle up",
                |grid| grid.scroll_rect_up(0, 1, 0, 1, &Cell::default()),
                true,
            ),
            (
                "rectangle down",
                |grid| grid.scroll_rect_down(0, 1, 0, 1, &Cell::default()),
                true,
            ),
            (
                "archive",
                |grid| {
                    grid.archive_visible();
                },
                true,
            ),
            ("clear history", Grid::clear_scrollback, true),
            ("row limit", |grid| grid.set_scrollback_limit(0), true),
            ("byte limit", |grid| grid.set_scrollback_byte_limit(0), true),
            ("resize", |grid| grid.resize(3, 3), true),
        ];
        for &(name, mutate, moves_rows) in mutations {
            let mut grid = Grid::new(4, 2);
            grid.cell_mut(0, 0).unwrap().set_char('a');
            grid.scroll_up();
            assert_eq!(grid.scrollback_len(), 1);
            let content = grid.content_revision();
            let mapping = grid.row_mapping_revision();
            mutate(&mut grid);
            assert_ne!(grid.content_revision(), content, "{name}");
            assert_eq!(grid.row_mapping_revision() != mapping, moves_rows, "{name}");
        }
    }

    #[test]
    fn grid_revisions_ignore_navigation_and_distinguish_replacement_and_snapshot() {
        let mut grid = Grid::new(4, 2);
        grid.scroll_up();
        let content = grid.content_revision();
        let mapping = grid.row_mapping_revision();
        grid.set_cursor(1, 2);
        grid.set_cursor_visible(false);
        grid.set_wrap_pending(true);
        grid.set_scroll_offset(1);
        grid.set_scrollback_limit(100);
        grid.set_scrollback_byte_limit(1024 * 1024);
        grid.resize(4, 2);
        assert!(grid.cell_mut(100, 100).is_none());
        assert!(grid.row_mut(100).is_none());
        assert_eq!(grid.content_revision(), content);
        assert_eq!(grid.row_mapping_revision(), mapping);
        let snapshot = grid.visible_snapshot();
        assert_ne!(snapshot.content_revision(), content);
        let frozen = snapshot.content_revision();
        grid.cell_mut(0, 0).unwrap().set_char('x');
        assert_eq!(snapshot.content_revision(), frozen);
        assert_ne!(
            Grid::new(4, 2).content_revision(),
            Grid::new(4, 2).content_revision()
        );
    }

    #[test]
    fn multicell_repair_leaves_ordinary_rows_clean_and_unchanged() {
        let mut grid = Grid::new(8, 4);
        for row in 0..4 {
            grid.cell_mut(row, 0).unwrap().set_grapheme("e\u{301}");
            grid.cell_mut(row, 1).unwrap().set_char('界');
            grid.cell_mut(row, 1)
                .unwrap()
                .flags
                .insert(CellFlags::WIDE_CHAR);
            grid.cell_mut(row, 2)
                .unwrap()
                .flags
                .insert(CellFlags::WIDE_CHAR_SPACER);
            grid.row_mut(row).unwrap().set_clean();
        }
        let before: Vec<_> = (0..4)
            .map(|row| grid.row(row).unwrap().iter().cloned().collect::<Vec<_>>())
            .collect();
        grid.repair_multicells(&Cell::default());
        for (row, cells) in before.iter().enumerate() {
            assert!(!grid.row(row).unwrap().is_dirty());
            assert_eq!(
                grid.row(row).unwrap().iter().collect::<Vec<_>>(),
                cells.iter().collect::<Vec<_>>()
            );
        }
    }

    #[test]
    fn multicell_repair_rejects_foreign_descriptors_sharing_a_valid_anchor() {
        let leader = MultiCell::new(
            2,
            1,
            0,
            0,
            MultiCellAlignment::Start,
            MultiCellAlignment::Start,
            0,
            0,
        )
        .unwrap();
        let blank = Cell {
            bg: Color::rgb(17, 31, 59),
            ..Cell::default()
        };
        for (scale, width) in [(3, 1), (2, 2), (3, 2)] {
            let foreign = MultiCell::new(
                scale,
                width,
                1,
                2,
                MultiCellAlignment::Center,
                MultiCellAlignment::End,
                0,
                0,
            )
            .unwrap();
            for relative_row in 0..foreign.rows() {
                for relative_col in 0..foreign.columns() {
                    if relative_row < leader.rows() && relative_col < leader.columns() {
                        continue;
                    }
                    for stray_text in [false, true] {
                        let mut grid = Grid::new(10, 6);
                        let (top, left) = (1, 2);
                        for row in 0..leader.rows() {
                            for col in 0..leader.columns() {
                                let cell = grid.cell_mut(top + row, left + col).unwrap();
                                cell.multicell = leader.at(col, row).unwrap();
                                if row == 0 && col == 0 {
                                    cell.set_grapheme("large");
                                    cell.multicell = leader;
                                }
                            }
                        }
                        let (row, col) = (top + relative_row, left + relative_col);
                        let cell = grid.cell_mut(row, col).unwrap();
                        if stray_text {
                            cell.set_char('x');
                        }
                        cell.multicell = foreign.at(relative_col, relative_row).unwrap();
                        assert!(grid.multicell_region_at(top, left).is_some());
                        assert!(grid.multicell_region_at(row, col).is_none());
                        // Exercise cached presence and ensure repair preserves all
                        // unrelated cells, not just the legitimate leader.
                        for row in 0..grid.rows() {
                            grid.row_mut(row).unwrap().has_multicells();
                            grid.row_mut(row).unwrap().set_clean();
                        }
                        let before: Vec<_> = grid.iter_cells().map(|(_, _, c)| c.clone()).collect();
                        // Synchronized-output capture uses the same repair path
                        // on newly copied rows, with cold descriptor caches.
                        let snapshot = grid.visible_snapshot();
                        assert!(!snapshot.cell(row, col).unwrap().multicell.is_present());
                        assert!(snapshot.cell(row, col).unwrap().content.is_empty());
                        assert!(snapshot.multicell_region_at(top, left).is_some());
                        assert!(grid.cell(row, col).unwrap().multicell.is_present());
                        grid.repair_multicells(&blank);
                        let mut erased = blank.clone();
                        erased.flags.insert(CellFlags::DIRTY);
                        assert_eq!(
                            grid.cell(row, col),
                            Some(&erased),
                            "foreign={foreign:?} row={row} col={col} text={stray_text}"
                        );
                        for (r, c, cell) in grid.iter_cells() {
                            if (r, c) != (row, col) {
                                assert_eq!(cell, &before[r * grid.cols() + c]);
                            }
                        }
                        for r in 0..grid.rows() {
                            assert_eq!(grid.row(r).unwrap().is_dirty(), r == row);
                        }
                        assert!(grid.multicell_region_at(top, left).is_some());
                    }
                }
            }
        }
    }

    #[test]
    fn multicell_repair_matches_two_pass_reference_for_every_block_subset() {
        // Frozen pre-optimization algorithm. Every subset of a 4x2 block
        // includes empty grids, intact blocks, missing leaders, and holes.
        fn reference(grid: &mut Grid, blank: &Cell) {
            let mut valid = HashSet::new();
            for row in 0..grid.rows {
                for col in 0..grid.cols {
                    if grid
                        .cell(row, col)
                        .is_some_and(|cell| cell.multicell.is_leader())
                        && let Some(region) = grid.multicell_region_at(row, col)
                    {
                        valid.insert((region.top, region.left));
                    }
                }
            }
            for row in 0..grid.rows {
                for col in 0..grid.cols {
                    let keep = grid.cell(row, col).is_none_or(|cell| {
                        !cell.multicell.is_present()
                            || row
                                .checked_sub(cell.multicell.relative_row())
                                .zip(col.checked_sub(cell.multicell.relative_col()))
                                .is_some_and(|anchor| valid.contains(&anchor))
                    });
                    if !keep && let Some(cell) = grid.cell_mut(row, col) {
                        *cell = blank.clone();
                        cell.flags.insert(CellFlags::DIRTY);
                    }
                }
            }
        }
        let descriptor = MultiCell::new(
            2,
            2,
            0,
            0,
            MultiCellAlignment::Start,
            MultiCellAlignment::Start,
            0,
            0,
        )
        .unwrap();
        let blank = Cell {
            bg: Color::rgb(17, 31, 59),
            ..Cell::default()
        };
        for mask in 0..256 {
            for empty_leader in [false, true] {
                let mut actual = Grid::new(8, 4);
                let mut expected = Grid::new(8, 4);
                for grid in [&mut actual, &mut expected] {
                    // Prime cached absence before descriptor insertion so the
                    // parity check also covers cache invalidation on edits.
                    for row in 0..4 {
                        assert!(!grid.row_mut(row).unwrap().has_multicells());
                    }
                    grid.cell_mut(0, 0).unwrap().set_char('x');
                    for offset in 0..8 {
                        if mask & (1 << offset) != 0 {
                            let cell = grid.cell_mut(1 + offset / 4, 2 + offset % 4).unwrap();
                            if offset == 0 && !empty_leader {
                                cell.set_grapheme("large");
                            }
                            cell.multicell = descriptor.at(offset % 4, offset / 4).unwrap();
                        }
                    }
                    for row in 0..4 {
                        grid.row_mut(row).unwrap().set_clean();
                    }
                }
                actual.repair_multicells(&blank);
                reference(&mut expected, &blank);
                for row in 0..4 {
                    assert_eq!(
                        actual.row(row).unwrap().is_dirty(),
                        expected.row(row).unwrap().is_dirty()
                    );
                    for col in 0..8 {
                        assert_eq!(
                            actual.cell(row, col),
                            expected.cell(row, col),
                            "mask={mask} empty_leader={empty_leader} row={row} col={col}"
                        );
                    }
                }
            }
        }
    }

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

    #[test]
    fn multicell_regions_validate_and_clear_atomically() {
        let mut grid = Grid::new(8, 4);
        let leader = MultiCell::new(
            2,
            2,
            1,
            2,
            MultiCellAlignment::Center,
            MultiCellAlignment::End,
            0,
            0,
        )
        .unwrap();
        for relative_row in 0..leader.rows() {
            for relative_col in 0..leader.columns() {
                let cell = grid.cell_mut(1 + relative_row, 2 + relative_col).unwrap();
                cell.multicell = leader.at(relative_col, relative_row).unwrap();
            }
        }
        grid.cell_mut(1, 2).unwrap().set_grapheme("large");
        grid.cell_mut(1, 2).unwrap().multicell = leader;

        let region = grid.multicell_region_at(2, 5).unwrap();
        assert_eq!((region.top, region.left), (1, 2));
        assert_eq!((region.bottom(), region.right()), (3, 6));
        assert!(region.contains(1, 2));
        assert!(region.contains(2, 5));
        assert!(!region.contains(3, 5));

        let blank = Cell::default();
        assert!(grid.clear_multicell_at(1, 4, &blank, true));
        for row in 1..3 {
            for col in 2..6 {
                let cell = grid.cell(row, col).unwrap();
                assert_eq!(cell.content, CellContent::Char(' '));
                assert!(!cell.multicell.is_present());
            }
        }
    }

    #[test]
    fn malformed_multicell_descriptor_cannot_clear_neighbors() {
        let mut grid = Grid::new(4, 2);
        grid.cell_mut(0, 0).unwrap().set_char('a');
        grid.cell_mut(0, 1).unwrap().set_char('b');
        grid.cell_mut(0, 1).unwrap().multicell = MultiCell::new(
            2,
            2,
            0,
            0,
            MultiCellAlignment::Start,
            MultiCellAlignment::Start,
            1,
            0,
        )
        .unwrap();

        assert!(grid.multicell_region_at(0, 1).is_none());
        assert!(!grid.clear_multicell_at(0, 1, &Cell::default(), false));
        assert_eq!(grid.cell(0, 0).unwrap().content, CellContent::Char('a'));
        assert!(grid.cell(0, 1).unwrap().is_empty());
    }

    #[test]
    fn visible_snapshot_copies_the_scrolled_view_without_history() {
        let mut grid = Grid::new(4, 2);
        for (col, character) in "old!".chars().enumerate() {
            grid.cell_mut(0, col).unwrap().set_char(character);
        }
        grid.scroll_up();
        for (col, character) in "live".chars().enumerate() {
            grid.cell_mut(1, col).unwrap().set_char(character);
        }
        grid.set_scroll_offset(1);

        let snapshot = grid.visible_snapshot();

        assert_eq!(snapshot.scrollback_len(), 0);
        assert_eq!(snapshot.scroll_offset(), 0);
        assert_eq!(snapshot.row(0).unwrap().to_string(), "old!");
        assert!(!snapshot.cursor().visible);
    }
}
