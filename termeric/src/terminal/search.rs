//! Terminal search functionality
//!
//! Provides incremental search through terminal history and visible content.
//! Supports both literal and regex search modes.

use super::row_text::push_row_text;
use crate::ui::search_bar::SearchBarInfo;
use regex::{Regex, RegexBuilder};
use std::ops::{Range, RangeInclusive};
use std::time::{Duration, Instant};
use termeric_core::Grid;
use unicode_segmentation::UnicodeSegmentation;

/// Navigation/highlight occurrences retain a fixed recent window. Complete
/// total counts and matching-row/filter data continue across the whole grid.
pub(crate) const MAX_RETAINED_SEARCH_MATCHES: usize = 131_072;
pub(crate) const MAX_SEARCH_RESULT_INDEX_BYTES: usize = 8 * 1024 * 1024;
pub(crate) const MAX_SEARCH_QUERY_BYTES: usize = 4 * 1024;

/// Rows of one logical line searched at a time. A longer soft-wrapped line,
/// such as megabytes of output with no newline, is searched in slices of this
/// many rows, so one slice of work never grows with the line.
const MAX_LINE_SLICE_ROWS: i32 = 64;
/// How far past a slice a regex match that starts in it may reach. A literal
/// query's matches are never longer than the query, so a slice reads that far
/// ahead; a regex match may be unbounded, so it is cut off here instead.
const REGEX_MATCH_OVERLAP_CHARS: usize = 4 * 1024;

fn valid_query_text(text: &str) -> bool {
    text.len() <= MAX_SEARCH_QUERY_BYTES && !text.chars().any(crate::safe_text::breaks_single_line)
}

/// Rows of one logical line (a row and the rows it soft-wraps into) as text.
struct SearchLine {
    text: String,
    first_row: i32,
    /// Each cell's first byte, row, and columns. Absent while bytes map one
    /// to one onto the first row's columns, the common case.
    cells: Option<Vec<LineCell>>,
}

#[derive(Clone, Copy)]
struct LineCell {
    byte: usize,
    row: i32,
    start_col: usize,
    end_col: usize,
}

/// One slice of a logical line, read from `SearchLine::slice`.
struct LineSlice {
    line: SearchLine,
    /// Byte where the search starts: the first owned row, or later when an
    /// earlier slice's last match ran past it.
    search_start: usize,
    /// Byte just past the owned rows. Matches start before it; the text after
    /// it is there to finish them.
    owned_end: usize,
    last_owned_row: i32,
    /// Whether the logical line continues after the owned rows.
    continues: bool,
    rows_read: usize,
}

#[derive(Debug)]
struct PendingSearch {
    content_revision: (u64, u64),
    cols: usize,
    history_len: usize,
    visible_rows: usize,
    /// The next row to search: history rows are negative, visible rows 0 up.
    next_row: i32,
    /// While a logical line continues into `next_row`, the cell where its
    /// search goes on: the start of `next_row`, or the end of the last match
    /// when that ran further.
    line_resume: Option<(i32, usize)>,
}

impl PendingSearch {
    fn new(grid: &Grid) -> Self {
        Self {
            content_revision: grid.content_revision(),
            cols: grid.cols(),
            history_len: grid.scrollback_len(),
            visible_rows: grid.rows(),
            next_row: -(grid.scrollback_len().min(i32::MAX as usize) as i32),
            line_resume: None,
        }
    }

    fn matches_source(&self, grid: &Grid) -> bool {
        self.content_revision == grid.content_revision()
            && self.cols == grid.cols()
            && self.history_len == grid.scrollback_len()
            && self.visible_rows == grid.rows()
    }

    fn is_complete(&self) -> bool {
        self.next_row >= self.visible_rows.min(i32::MAX as usize) as i32
    }
}

impl SearchLine {
    /// The slice of a logical line that owns `row` and up to
    /// [`MAX_LINE_SLICE_ROWS`] - 1 rows it wraps into, never past
    /// `last_allowed`. Rows are searched in order, so `row` starts its line
    /// unless an earlier slice stopped before it and left `resume`. Then the
    /// row before is read too, so anchors and word boundaries see what
    /// precedes. When the line continues, the slice reads at least
    /// `overlap_chars` (and at least one) more characters of it, so a match
    /// that starts in the owned rows can finish and `$` cannot match early.
    fn slice(
        grid: &Grid,
        row: i32,
        resume: Option<(i32, usize)>,
        last_allowed: i32,
        overlap_chars: usize,
    ) -> LineSlice {
        let first_row = if resume.is_some() { row - 1 } else { row };
        let mut line = Self {
            text: String::new(),
            first_row,
            cells: None,
        };
        let mut rows_read = 0;
        let mut owned_start = 0;
        let mut current = first_row;
        let mut continues = false;
        while let Some(view) = grid.row_at_offset(current) {
            if current == row {
                owned_start = line.text.len();
            }
            line.push_row(current, &view);
            rows_read += 1;
            continues = view.is_wrapped()
                && current < last_allowed
                && grid.row_at_offset(current + 1).is_some();
            if !continues || current - row + 1 >= MAX_LINE_SLICE_ROWS {
                break;
            }
            current += 1;
        }
        let last_owned_row = current;
        let owned_end = line.text.len();
        if continues {
            let mut overlap_read = 0;
            let mut current = last_owned_row + 1;
            while overlap_read < overlap_chars.max(1)
                && let Some(view) = grid.row_at_offset(current)
            {
                let before = line.text.len();
                line.push_row(current, &view);
                rows_read += 1;
                overlap_read += line.text[before..].chars().count();
                if !view.is_wrapped() || current >= last_allowed {
                    break;
                }
                current += 1;
            }
        }
        let search_start = resume.map_or(owned_start, |cell| line.byte_at(cell).max(owned_start));
        LineSlice {
            line,
            search_start,
            owned_end,
            last_owned_row,
            continues,
            rows_read,
        }
    }

    /// The byte of the first cell at or after `(row, col)`, or the end of
    /// the text.
    fn byte_at(&self, (row, col): (i32, usize)) -> usize {
        match &self.cells {
            Some(cells) => cells
                .get(cells.partition_point(|cell| (cell.row, cell.start_col) < (row, col)))
                .map_or(self.text.len(), |cell| cell.byte),
            None if row == self.first_row => col.min(self.text.len()),
            None if row < self.first_row => 0,
            None => self.text.len(),
        }
    }

    fn push_row(&mut self, row_index: i32, row: &termeric_core::RowView<'_>) {
        let first_row = self.first_row;
        let cells = &mut self.cells;
        push_row_text(row, &mut self.text, |col, width, bytes| {
            let linear =
                row_index == first_row && bytes.start == col && bytes.len() == 1 && width == 1;
            if !linear && cells.is_none() {
                *cells = Some(
                    (0..bytes.start)
                        .map(|byte| LineCell {
                            byte,
                            row: first_row,
                            start_col: byte,
                            end_col: byte + 1,
                        })
                        .collect(),
                );
            }
            if let Some(cells) = cells {
                cells.push(LineCell {
                    byte: bytes.start,
                    row: row_index,
                    start_col: col,
                    end_col: col + width,
                });
            }
        });
    }

    /// The cells under bytes `start..end` of the text.
    fn span(&self, start: usize, end: usize) -> SearchMatch {
        let Some(cells) = &self.cells else {
            return SearchMatch {
                row: self.first_row,
                start_col: start,
                end_row: self.first_row,
                end_col: end,
            };
        };
        let cell_at =
            |byte: usize| cells[cells.partition_point(|cell| cell.byte <= byte).max(1) - 1];
        let first = cell_at(start);
        if end <= start {
            return SearchMatch {
                row: first.row,
                start_col: first.start_col,
                end_row: first.row,
                end_col: first.start_col,
            };
        }
        let last = cell_at(end - 1);
        SearchMatch {
            row: first.row,
            start_col: first.start_col,
            end_row: last.row,
            end_col: last.end_col,
        }
    }
}

/// A search match in the terminal. It continues across soft-wrapped rows,
/// from `(row, start_col)` up to `(end_row, end_col)`.
#[derive(Clone, Debug, PartialEq)]
pub struct SearchMatch {
    /// Row of the first matched cell (negative for scrollback, 0+ for visible grid)
    pub row: i32,
    /// Column of the first matched cell
    pub start_col: usize,
    /// Row of the last matched cell
    pub end_row: i32,
    /// Column just past the last matched cell, on `end_row`
    pub end_col: usize,
}

impl SearchMatch {
    /// Check if a position is within this match
    pub fn contains(&self, row: i32, col: usize) -> bool {
        (row, col) >= (self.row, self.start_col) && (row, col) < (self.end_row, self.end_col)
    }
}

/// Search state and functionality
pub struct Search {
    /// Current search query
    query: String,
    /// Whether search is case-sensitive
    case_sensitive: bool,
    /// Whether to use regex mode
    use_regex: bool,
    /// Whether filter mode is enabled (show only matching lines)
    filter_mode: bool,
    /// Compiled matcher for the current regex or escaped literal query.
    compiled_regex: Option<Regex>,
    /// Regex compilation error (if any)
    regex_error: Option<String>,
    /// All matches found
    matches: Vec<SearchMatch>,
    /// Physical index of the oldest retained occurrence when `matches` is a
    /// full circular buffer. Zero until the retention cap is reached.
    retained_match_start: usize,
    /// Exact occurrence count across every scanned row, including occurrences
    /// older than the retained navigation/highlight window.
    total_match_count: usize,
    /// Logical index within the retained recent-match window.
    current_match: Option<usize>,
    /// Whether search is active
    active: bool,
    /// Rows that contain matches (for filter mode) - sorted and deduplicated
    matching_rows: Vec<i32>,
    /// First matching-row index shown by virtualized filter rendering.
    filter_row_offset: usize,
    /// Optional inclusive row scope (used by Harvest's Search Output).
    row_scope: Option<RangeInclusive<i32>>,
    /// In-progress scan. Work is consumed in bounded slices by
    /// the event-driven render loop so keyboard dispatch never walks all history.
    pending_search: Option<PendingSearch>,
    /// Identity of the matching-row set. Scrolling that set does not change it.
    result_generation: u64,
}

impl Default for Search {
    fn default() -> Self {
        Self::new()
    }
}

impl Search {
    /// Create a new search state
    pub fn new() -> Self {
        Self {
            query: String::new(),
            case_sensitive: false,
            use_regex: false,
            filter_mode: false,
            compiled_regex: None,
            regex_error: None,
            matches: Vec::new(),
            retained_match_start: 0,
            total_match_count: 0,
            current_match: None,
            active: false,
            matching_rows: Vec::new(),
            filter_row_offset: 0,
            row_scope: None,
            pending_search: None,
            result_generation: 0,
        }
    }

    /// Start a new search
    pub fn start(&mut self) {
        self.start_with_scope(None);
    }

    pub fn start_in_rows(&mut self, first_row: i32, last_row: i32) {
        let (first, last) = if first_row <= last_row {
            (first_row, last_row)
        } else {
            (last_row, first_row)
        };
        self.start_with_scope(Some(first..=last));
    }

    pub fn is_scoped(&self) -> bool {
        self.row_scope.is_some()
    }

    /// A new search keeps filter mode, which the user may want to continue.
    fn start_with_scope(&mut self, row_scope: Option<RangeInclusive<i32>>) {
        self.clear_query(row_scope);
        self.active = true;
    }

    /// Cancel the search
    pub fn cancel(&mut self) {
        self.clear_query(None);
        self.active = false;
        self.filter_mode = false;
        // Release the result indexes, which can be large, rather than keep
        // their capacity for a search that may never come.
        self.matches = Vec::new();
        self.matching_rows = Vec::new();
    }

    fn clear_query(&mut self, row_scope: Option<RangeInclusive<i32>>) {
        self.query.clear();
        self.compiled_regex = None;
        self.regex_error = None;
        self.row_scope = row_scope;
        self.clear_results();
    }

    fn clear_results(&mut self) {
        self.result_generation = self.result_generation.wrapping_add(1);
        self.matches.clear();
        self.retained_match_start = 0;
        self.total_match_count = 0;
        self.matching_rows.clear();
        self.filter_row_offset = 0;
        self.current_match = None;
        self.pending_search = None;
    }

    /// Confirm the search (keep highlights, exit search input mode)
    pub fn confirm(&mut self) {
        self.active = false;
        // Keep matches, current_match, and filter_mode for continued highlighting/filtering
    }

    /// Re-enter a visible query without resetting its scope, results, or scan.
    /// The application owns the caret; refuse stale or non-grapheme positions.
    pub fn resume_input_at(&mut self, cursor: usize) -> bool {
        if !(self.active || self.has_results() || self.is_searching())
            || (cursor != self.query.len()
                && !self
                    .query
                    .grapheme_indices(true)
                    .any(|(byte, _)| byte == cursor))
        {
            return false;
        }
        self.active = true;
        true
    }

    /// Toggle filter mode (show only matching lines)
    pub fn toggle_filter(&mut self) {
        self.result_generation = self.result_generation.wrapping_add(1);
        self.filter_mode = !self.filter_mode;
        if self.filter_mode {
            self.sync_filter_to_current();
        }
    }

    /// Is filter mode enabled?
    pub fn is_filter_mode(&self) -> bool {
        self.filter_mode
    }

    /// Exact ordered source rows for an active, completed filter projection.
    /// Borrow the existing index; selections must not clone full history indexes.
    pub fn filtered_matching_rows(&self) -> &[i32] {
        if self.filter_mode && !self.is_searching() {
            &self.matching_rows
        } else {
            &[]
        }
    }

    pub fn filter_generation(&self) -> Option<u64> {
        (!self.filtered_matching_rows().is_empty()).then_some(self.result_generation)
    }

    /// Get the number of matching lines (distinct rows with matches)
    pub fn matching_line_count(&self) -> usize {
        if self.is_searching() {
            0
        } else {
            self.matching_rows.len()
        }
    }

    /// Return only the matching rows that fit the current viewport. Filter
    /// mode remains complete without making render work proportional to every
    /// matching line on each frame.
    pub fn filtered_rows(&self, viewport_rows: usize) -> &[i32] {
        if self.is_searching() || !self.filter_mode || viewport_rows == 0 {
            return &[];
        }
        let start = self
            .filter_row_offset
            .min(self.matching_rows.len().saturating_sub(viewport_rows));
        let end = start
            .saturating_add(viewport_rows)
            .min(self.matching_rows.len());
        &self.matching_rows[start..end]
    }

    /// Scroll the virtualized matching-line viewport. Positive wheel deltas
    /// move toward older rows, matching ordinary terminal scroll direction.
    pub fn scroll_filter(&mut self, delta: f64, viewport_rows: usize) {
        if !self.filter_mode || self.is_searching() || self.matching_rows.is_empty() {
            return;
        }
        let lines = delta.abs().max(1.0) as usize;
        let max_offset = self.matching_rows.len().saturating_sub(viewport_rows);
        self.filter_row_offset = if delta > 0.0 {
            self.filter_row_offset.saturating_sub(lines)
        } else {
            self.filter_row_offset.saturating_add(lines).min(max_offset)
        };
    }

    /// Check if search is active
    pub fn is_active(&self) -> bool {
        self.active
    }

    /// Check if we have search results to display (even when input closed)
    pub fn has_results(&self) -> bool {
        !self.is_searching() && self.total_match_count > 0
    }

    /// Get the current query
    pub fn query(&self) -> &str {
        &self.query
    }

    /// Whether a complete result set is still being assembled.
    pub fn is_searching(&self) -> bool {
        self.pending_search.is_some()
    }

    /// Type a character at the end of the query, as the keyboard does.
    #[cfg(test)]
    pub fn push_char(&mut self, c: char, grid: &Grid) {
        assert!(self.insert_text(self.query.len(), c.encode_utf8(&mut [0; 4]), grid));
    }

    /// Insert committed platform text at a UTF-8 cursor boundary.
    pub fn insert_text(&mut self, cursor: usize, text: &str, grid: &Grid) -> bool {
        if cursor <= self.query.len()
            && self.query.is_char_boundary(cursor)
            && self.query.len().saturating_add(text.len()) <= MAX_SEARCH_QUERY_BYTES
            && valid_query_text(text)
        {
            self.query.insert_str(cursor, text);
            self.compile_regex_if_needed();
            self.begin_update(grid);
            true
        } else {
            false
        }
    }

    /// Whole-grapheme query editing. Cursor-only and no-op edits preserve
    /// completed results, selection, and progress through a pending scan.
    pub fn edit_query(
        &mut self,
        mut cursor: usize,
        edit: crate::ui::text_input::TextInputEdit,
        grid: &Grid,
    ) -> Option<usize> {
        if !self.query.is_char_boundary(cursor) {
            return None;
        }
        if crate::ui::text_input::edit_grapheme(&mut self.query, &mut cursor, edit) {
            self.compile_regex_if_needed();
            self.begin_update(grid);
        }
        Some(cursor)
    }

    /// Replace the complete query from an accessibility text action.
    pub fn set_query(&mut self, query: &str, grid: &Grid) -> bool {
        if !valid_query_text(query) {
            return false;
        }
        self.query.clear();
        self.query.push_str(query);
        self.compile_regex_if_needed();
        self.begin_update(grid);
        true
    }

    /// Toggle case sensitivity
    pub fn toggle_case_sensitive(&mut self, grid: &Grid) {
        self.case_sensitive = !self.case_sensitive;
        self.compile_regex_if_needed();
        self.begin_update(grid);
    }

    /// Toggle regex mode
    pub fn toggle_regex(&mut self, grid: &Grid) {
        self.use_regex = !self.use_regex;
        self.compile_regex_if_needed();
        self.begin_update(grid);
    }

    /// Re-run the current query against a different terminal grid while
    /// preserving the search controls and input state.
    pub fn refresh(&mut self, grid: &Grid) {
        // Content changes do not alter the query or its compile/error state.
        self.begin_update(grid);
    }

    /// Get current match count
    pub fn match_count(&self) -> usize {
        if self.is_searching() {
            0
        } else {
            self.total_match_count
        }
    }

    /// Number of recent occurrences available for navigation/highlighting.
    pub fn retained_match_count(&self) -> usize {
        if self.is_searching() {
            0
        } else {
            self.matches.len()
        }
    }

    /// Whether older occurrences were omitted from navigation/highlighting.
    pub fn results_truncated(&self) -> bool {
        !self.is_searching() && self.total_match_count > self.matches.len()
    }

    /// Deterministic bytes reserved by occurrence and matching-row indexes.
    #[cfg(test)]
    pub(crate) fn result_index_memory_bytes(&self) -> usize {
        self.matches
            .capacity()
            .saturating_mul(std::mem::size_of::<SearchMatch>())
            .saturating_add(
                self.matching_rows
                    .capacity()
                    .saturating_mul(std::mem::size_of::<i32>()),
            )
    }

    /// Get current match index (1-based for display)
    pub fn current_match_index(&self) -> Option<usize> {
        if self.is_searching() {
            return None;
        }
        let omitted = self.total_match_count.saturating_sub(self.matches.len());
        self.current_match
            .map(|index| omitted.saturating_add(index).saturating_add(1))
    }

    /// Get current match
    pub fn current(&self) -> Option<&SearchMatch> {
        if self.is_searching() {
            None
        } else {
            self.current_match
                .and_then(|index| self.retained_match(index))
        }
    }

    /// Move to next match
    pub fn next(&mut self) {
        if self.is_searching() || self.matches.is_empty() {
            return;
        }

        self.current_match = Some(match self.current_match {
            Some(i) => (i + 1) % self.matches.len(),
            None => 0,
        });
        self.sync_filter_to_current();
    }

    /// Move to previous match
    pub fn prev(&mut self) {
        if self.is_searching() || self.matches.is_empty() {
            return;
        }

        self.current_match = Some(match self.current_match {
            Some(0) | None => self.matches.len().saturating_sub(1),
            Some(i) => i - 1,
        });
        self.sync_filter_to_current();
    }

    /// Check if a cell position is part of a match
    pub fn is_match(&self, row: i32, col: usize) -> bool {
        if self.is_searching() {
            return false;
        }
        let (newer, older) = self.retained_match_slices();
        match_in_sorted_slice(older, row, col) || match_in_sorted_slice(newer, row, col)
    }

    /// Check if a cell position is part of the current match
    pub fn is_current_match(&self, row: i32, col: usize) -> bool {
        self.current()
            .map(|m| m.contains(row, col))
            .unwrap_or(false)
    }

    /// Compile one matcher per query change, never once per searched row.
    fn compile_regex_if_needed(&mut self) {
        if self.query.is_empty() {
            self.compiled_regex = None;
            self.regex_error = None;
            return;
        }

        let pattern = if self.use_regex {
            self.query.clone()
        } else {
            regex::escape(&self.query)
        };

        match RegexBuilder::new(&pattern)
            .case_insensitive(!self.case_sensitive)
            .build()
        {
            Ok(re) => {
                self.compiled_regex = Some(re);
                self.regex_error = None;
            }
            Err(e) => {
                self.compiled_regex = None;
                self.regex_error = Some(describe_regex_error(&e));
            }
        }
    }

    fn begin_update(&mut self, grid: &Grid) {
        self.clear_results();
        if self.query.is_empty() || self.compiled_regex.is_none() {
            return;
        }

        self.pending_search = Some(PendingSearch::new(grid));
    }

    /// Consume about `max_rows` rows of pending search work. A slice of a
    /// soft-wrapped line is read whole, so the last one may run past
    /// `max_rows` by up to [`MAX_LINE_SLICE_ROWS`] and the rows it reads
    /// around them.
    ///
    /// Returns `true` while more work remains. A grid content/geometry/history change
    /// invalidates partial results and restarts a full scan against the new
    /// shape so row offsets cannot be mixed across layouts.
    pub fn advance_rows(&mut self, grid: &Grid, max_rows: usize) -> bool {
        let Some(pending) = self.pending_search.as_ref() else {
            return false;
        };
        if !pending.matches_source(grid) {
            self.begin_update(grid);
        }

        let Some(mut pending) = self.pending_search.take() else {
            return false;
        };
        let last_allowed = self
            .row_scope
            .as_ref()
            .map_or(i32::MAX, |scope| *scope.end());
        let overlap_chars = if self.use_regex {
            REGEX_MATCH_OVERLAP_CHARS
        } else {
            self.query.chars().count()
        };
        let mut rows_read = 0;
        while rows_read < max_rows && !pending.is_complete() {
            let row = pending.next_row;
            let resume = pending.line_resume.take();
            if resume.is_none() && (!self.row_is_in_scope(row) || grid.row_at_offset(row).is_none())
            {
                pending.next_row += 1;
                rows_read += 1;
                continue;
            }
            let slice = SearchLine::slice(grid, row, resume, last_allowed, overlap_chars);
            rows_read += slice.rows_read;
            let last_match_end =
                self.find_matches_in_line(&slice.line, slice.search_start..slice.owned_end);
            pending.next_row = slice.last_owned_row + 1;
            if slice.continues {
                let next_slice = (pending.next_row, 0);
                pending.line_resume = [Some(next_slice), resume, last_match_end]
                    .into_iter()
                    .flatten()
                    .max();
            }
        }

        if pending.is_complete() {
            self.finish_update();
            false
        } else {
            self.pending_search = Some(pending);
            true
        }
    }

    /// Consume pending work until the time budget expires.
    ///
    /// Work is grouped into small row batches to avoid a clock read per row.
    /// At least one batch is attempted whenever work is pending.
    pub fn advance_for(&mut self, grid: &Grid, budget: Duration) -> bool {
        let started = Instant::now();
        loop {
            if !self.advance_rows(grid, 16) {
                return false;
            }
            if started.elapsed() >= budget {
                return true;
            }
        }
    }

    fn finish_update(&mut self) {
        // Set current match to first if we have matches
        if !self.matches.is_empty() {
            self.current_match = Some(0);
        }
        if self.filter_mode {
            self.sync_filter_to_current();
        }
    }

    #[cfg(test)]
    pub(crate) fn finish(&mut self, grid: &Grid) {
        while self.advance_rows(grid, 4096) {}
    }

    fn row_is_in_scope(&self, row: i32) -> bool {
        self.row_scope
            .as_ref()
            .is_none_or(|scope| scope.contains(&row))
    }

    fn retained_match(&self, logical_index: usize) -> Option<&SearchMatch> {
        if logical_index >= self.matches.len() {
            return None;
        }
        let physical_index = (self.retained_match_start + logical_index) % self.matches.len();
        self.matches.get(physical_index)
    }

    /// Return chronological slices for the circular retained-match window.
    fn retained_match_slices(&self) -> (&[SearchMatch], &[SearchMatch]) {
        if self.retained_match_start == 0 {
            (&self.matches, &[])
        } else {
            let (newer, older) = self.matches.split_at(self.retained_match_start);
            (older, newer)
        }
    }

    fn sync_filter_to_current(&mut self) {
        let Some(row) = self.current().map(|search_match| search_match.row) else {
            self.filter_row_offset = 0;
            return;
        };
        if let Ok(index) = self.matching_rows.binary_search(&row) {
            self.filter_row_offset = index;
        }
    }

    /// Records the matches in `line` that start in `starts`, and returns
    /// the end of the last one.
    fn find_matches_in_line(
        &mut self,
        line: &SearchLine,
        starts: Range<usize>,
    ) -> Option<(i32, usize)> {
        let text = &line.text;
        let mut found = Vec::new();
        if !self.use_regex && self.case_sensitive {
            found.extend(
                text[starts.start..]
                    .match_indices(&self.query)
                    .map(|(start, text)| (starts.start + start, starts.start + start + text.len()))
                    .take_while(|(start, _)| *start < starts.end),
            );
        } else if !self.use_regex && self.query.is_ascii() && text.is_ascii() {
            let query = self.query.as_bytes();
            let mut search_start = starts.start;
            while let Some(relative_start) = text.as_bytes()[search_start..]
                .windows(query.len())
                .position(|candidate| candidate.eq_ignore_ascii_case(query))
            {
                let start = search_start + relative_start;
                if start >= starts.end {
                    break;
                }
                found.push((start, start + query.len()));
                search_start = start + query.len();
            }
        } else if let Some(regex) = &self.compiled_regex {
            found.extend(regex_matches(regex, text, starts));
        }
        let mut last_end = None;
        for (start, end) in found {
            let search_match = line.span(start, end);
            for row in search_match.row..=search_match.end_row {
                if self.matching_rows.last() != Some(&row) {
                    self.matching_rows.push(row);
                }
            }
            last_end = Some((search_match.end_row, search_match.end_col));
            record_retained_match(
                &mut self.matches,
                &mut self.retained_match_start,
                &mut self.total_match_count,
                search_match,
            );
        }
        last_end
    }
}

/// The byte ranges of `regex`'s matches in `text` that start in `starts`,
/// as `find_iter` would yield them. Searching from `starts.start` rather
/// than slicing the text keeps what precedes it as context for anchors and
/// word boundaries.
fn regex_matches(regex: &Regex, text: &str, starts: Range<usize>) -> Vec<(usize, usize)> {
    let next_char = |at: usize| text[at..].chars().next().map(|c| at + c.len_utf8());
    let mut found = Vec::new();
    let mut at = Some(starts.start);
    let mut last_end = None;
    while let Some(search_from) = at
        && let Some(matched) = regex.find_at(text, search_from)
        && matched.start() < starts.end
    {
        // Like find_iter, skip an empty match right where the last one ended.
        if matched.is_empty() && last_end == Some(matched.end()) {
            at = next_char(matched.end());
            continue;
        }
        found.push((matched.start(), matched.end()));
        last_end = Some(matched.end());
        at = if matched.is_empty() {
            next_char(matched.end())
        } else {
            Some(matched.end())
        };
    }
    found
}

fn record_retained_match(
    matches: &mut Vec<SearchMatch>,
    retained_match_start: &mut usize,
    total_match_count: &mut usize,
    search_match: SearchMatch,
) {
    *total_match_count = total_match_count.saturating_add(1);
    if matches.len() < MAX_RETAINED_SEARCH_MATCHES {
        matches.push(search_match);
        debug_assert!(
            matches
                .capacity()
                .saturating_mul(std::mem::size_of::<SearchMatch>())
                <= MAX_SEARCH_RESULT_INDEX_BYTES
        );
        return;
    }

    matches[*retained_match_start] = search_match;
    *retained_match_start = (*retained_match_start + 1) % matches.len();
}

fn match_in_sorted_slice(matches: &[SearchMatch], row: i32, col: usize) -> bool {
    let index = matches
        .partition_point(|search_match| (search_match.end_row, search_match.end_col) <= (row, col));
    matches
        .get(index)
        .is_some_and(|search_match| search_match.contains(row, col))
}

/// One line explaining why a pattern is invalid. The regex crate's syntax
/// errors span several lines (the pattern, a caret marker, then
/// `error: …`); only the final explanation fits the find box and reads well
/// aloud.
fn describe_regex_error(error: &regex::Error) -> String {
    match error {
        regex::Error::CompiledTooBig(_) => "Pattern is too large".to_string(),
        _ => {
            let text = error.to_string();
            let line = text
                .lines()
                .rev()
                .map(str::trim)
                .find(|line| !line.is_empty())
                .unwrap_or_default();
            let reason = line.strip_prefix("error:").map_or(line, str::trim);
            let mut chars = reason.chars();
            chars.next().map_or_else(String::new, |first| {
                first.to_uppercase().chain(chars).collect()
            })
        }
    }
}

impl Search {
    /// Build info struct for rendering
    pub fn build_info(&self, cursor_pos: usize) -> SearchBarInfo {
        SearchBarInfo {
            query: self.query.clone(),
            case_sensitive: self.case_sensitive,
            use_regex: self.use_regex,
            filter_mode: self.filter_mode,
            current_match: self.current_match_index(),
            match_count: self.match_count(),
            retained_match_count: self.retained_match_count(),
            results_truncated: self.results_truncated(),
            searching: self.is_searching(),
            matching_line_count: self.matching_line_count(),
            regex_error: self.regex_error.clone(),
            cursor_pos,
            is_active: self.is_active(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn regex_errors_are_one_plain_line() {
        // Invalid on purpose; held in variables so they are parsed at run time.
        let [unclosed, reversed] = ["Cargo(", "a{2,1}"];
        let error = Regex::new(unclosed).unwrap_err();
        assert!(error.to_string().lines().count() > 1);
        assert_eq!(describe_regex_error(&error), "Unclosed group");
        let error = Regex::new(reversed).unwrap_err();
        let described = describe_regex_error(&error);
        assert!(!described.contains('\n') && !described.starts_with("error"));
        assert_eq!(
            describe_regex_error(&regex::Error::CompiledTooBig(1)),
            "Pattern is too large"
        );
    }

    #[test]
    fn test_search_state() {
        let mut search = Search::new();
        assert!(!search.is_active());

        search.start();
        assert!(search.is_active());
        assert!(search.query().is_empty());

        search.cancel();
        assert!(!search.is_active());
    }

    #[test]
    fn resume_query_rejects_invalid_carets_and_preserves_scope_and_filter() {
        let terminal = termeric_core::Terminal::new(40, 2);
        let mut search = Search::new();
        assert!(!search.resume_input_at(0));
        search.start_in_rows(0, 1);
        search.toggle_filter();
        search.set_query("e\u{301}👩\u{200d}💻", terminal.grid());
        search.confirm();
        for cursor in [1, 2, 4, usize::MAX] {
            assert!(!search.resume_input_at(cursor));
            assert!(!search.is_active());
        }
        assert!(search.resume_input_at("e\u{301}".len()));
        assert_eq!(search.row_scope, Some(0..=1));
        assert!(search.filter_mode);
        assert_eq!(search.query(), "e\u{301}👩\u{200d}💻");
        search.cancel();
        assert!(!search.resume_input_at(0));
    }

    #[test]
    fn scoped_search_normalizes_and_clears_its_row_range() {
        let mut search = Search::new();
        search.start_in_rows(8, 3);
        assert!(search.row_is_in_scope(3));
        assert!(search.row_is_in_scope(8));
        assert!(!search.row_is_in_scope(2));
        search.start();
        assert!(search.row_is_in_scope(-100));
    }

    #[test]
    fn test_match_contains() {
        let m = SearchMatch {
            row: 5,
            end_row: 5,
            start_col: 10,
            end_col: 15,
        };

        assert!(m.contains(5, 10));
        assert!(m.contains(5, 14));
        assert!(!m.contains(5, 15));
        assert!(!m.contains(5, 9));
        assert!(!m.contains(4, 12));
    }

    #[test]
    fn test_navigation() {
        let mut search = Search::new();
        search.matches = vec![
            SearchMatch {
                row: 0,
                end_row: 0,
                start_col: 0,
                end_col: 4,
            },
            SearchMatch {
                row: 1,
                end_row: 1,
                start_col: 5,
                end_col: 9,
            },
            SearchMatch {
                row: 2,
                end_row: 2,
                start_col: 0,
                end_col: 4,
            },
        ];
        search.current_match = Some(0);

        search.next();
        assert_eq!(search.current_match, Some(1));

        search.next();
        assert_eq!(search.current_match, Some(2));

        search.next();
        assert_eq!(search.current_match, Some(0)); // Wraps around

        search.prev();
        assert_eq!(search.current_match, Some(2)); // Wraps around backwards
    }

    #[test]
    fn test_regex_mode() {
        let mut search = Search::new();
        search.use_regex = true;
        search.query = r"\d+".to_string();
        search.compile_regex_if_needed();

        assert!(search.compiled_regex.is_some());
        assert!(search.regex_error.is_none());
    }

    #[test]
    fn test_invalid_regex() {
        let mut search = Search::new();
        search.use_regex = true;
        search.query = r"[invalid".to_string();
        search.compile_regex_if_needed();

        assert!(search.compiled_regex.is_none());
        assert!(search.regex_error.is_some());
    }

    #[test]
    fn test_case_insensitive_regex() {
        let mut search = Search::new();
        search.use_regex = true;
        search.case_sensitive = false;
        search.query = "test".to_string();
        search.compile_regex_if_needed();

        assert!(search.compiled_regex.is_some());
        let re = search.compiled_regex.as_ref().unwrap();
        assert!(re.is_match("TEST"));
        assert!(re.is_match("Test"));
        assert!(re.is_match("test"));
    }

    #[test]
    fn per_row_intervals_preserve_match_boundaries() {
        let mut terminal = termeric_core::Terminal::new(80, 2);
        terminal.process(b"needle gap needle");
        let mut search = Search::new();
        search.start();
        for character in "needle".chars() {
            search.push_char(character, terminal.grid());
        }
        search.finish(terminal.grid());

        assert!(search.is_match(0, 0));
        assert!(search.is_match(0, 5));
        assert!(!search.is_match(0, 6));
        assert!(search.is_match(0, 11));
        assert!(search.is_match(0, 16));
        assert!(!search.is_match(1, 0));
    }

    #[test]
    fn unicode_search_maps_utf8_matches_to_terminal_columns() {
        let mut terminal = termeric_core::Terminal::new(40, 2);
        terminal.process("界 e\u{301} 👩\u{200d}💻 needle".as_bytes());

        let mut search = Search::new();
        search.start();
        for character in "needle".chars() {
            search.push_char(character, terminal.grid());
        }
        search.finish(terminal.grid());
        let found = search.current().unwrap();
        // 界(2) + space(1) + é(1) + space(1) + emoji(2) + space(1)
        assert_eq!((found.start_col, found.end_col), (8, 14));

        search.start();
        for character in "👩\u{200d}💻".chars() {
            search.push_char(character, terminal.grid());
        }
        search.finish(terminal.grid());
        let found = search.current().unwrap();
        assert_eq!((found.start_col, found.end_col), (5, 7));
    }

    #[test]
    fn utf8_query_editing_honors_the_cursor_and_size_bound() {
        let terminal = termeric_core::Terminal::new(20, 2);
        let mut search = Search::new();
        search.start();
        assert!(search.insert_text(0, "ac", terminal.grid()));
        assert!(search.insert_text(1, "界", terminal.grid()));
        assert_eq!(search.query(), "a界c");
        assert_eq!(
            search.edit_query(
                4,
                crate::ui::text_input::TextInputEdit::Backspace,
                terminal.grid()
            ),
            Some(1)
        );
        assert_eq!(search.query(), "ac");

        let oversized = "x".repeat(MAX_SEARCH_QUERY_BYTES + 1);
        assert!(!search.set_query(&oversized, terminal.grid()));
        assert_eq!(search.query(), "ac");
        assert!(!search.insert_text(1, &oversized, terminal.grid()));
        assert_eq!(search.query(), "ac");
    }

    #[test]
    fn query_text_rejects_controls_atomically_for_replacement_and_insertion() {
        let mut terminal = termeric_core::Terminal::new(40, 2);
        terminal.process(b"needle needle");
        let mut search = Search::new();
        search.start();
        assert!(search.set_query("needle", terminal.grid()));
        search.finish(terminal.grid());
        let selected = search.current().cloned();
        for invalid in [
            "a\nb", "a\rb", "a\tb", "\0", "\x1b", "\u{85}", "\u{2028}", "\u{2029}",
        ] {
            assert!(!search.set_query(invalid, terminal.grid()), "{invalid:?}");
            assert!(
                !search.insert_text(1, invalid, terminal.grid()),
                "{invalid:?}"
            );
            assert_eq!(search.query(), "needle");
            assert_eq!(search.current(), selected.as_ref());
            assert_eq!(search.match_count(), 2);
            assert!(!search.is_searching());
        }
        let maximum = "🫚".repeat(MAX_SEARCH_QUERY_BYTES / 4);
        assert!(search.set_query(&maximum, terminal.grid()));
        assert!(!search.insert_text(0, "a", terminal.grid()));
        assert_eq!(search.query(), maximum);
        assert!(search.set_query("", terminal.grid()));
        assert!(search.insert_text(0, "e\u{301}👩\u{200d}💻", terminal.grid()));
    }

    #[test]
    fn hidden_terminal_content_remains_searchable() {
        let mut terminal = termeric_core::Terminal::new(20, 2);
        terminal.process(b"\x1b[8msecret\x1b[28m");
        let mut search = Search::new();
        search.start();
        for character in "secret".chars() {
            search.push_char(character, terminal.grid());
        }
        search.finish(terminal.grid());

        assert_eq!(
            search.current(),
            Some(&SearchMatch {
                row: 0,
                end_row: 0,
                start_col: 0,
                end_col: 6,
            })
        );
    }

    #[test]
    fn query_edits_preserve_pending_progress_and_completed_selection_until_text_changes() {
        use crate::ui::text_input::TextInputEdit::*;
        let mut terminal = termeric_core::Terminal::new(40, 2);
        terminal.grid_mut().set_scrollback_limit(1_000);
        for _ in 0..200 {
            terminal.process(b"needle\r\n");
        }
        let mut search = Search::new();
        search.start();
        assert!(search.set_query("needle", terminal.grid()));
        assert!(search.advance_rows(terminal.grid(), 10));
        let progress = format!("{:?}", search.pending_search);
        let mut cursor = 0;
        for edit in [
            Home,
            Backspace,
            BackspaceWord,
            Right,
            Left,
            WordRight,
            WordLeft,
            End,
            Delete,
            DeleteWord,
        ] {
            cursor = search.edit_query(cursor, edit, terminal.grid()).unwrap();
            search.confirm();
            assert!(search.resume_input_at(cursor));
            assert_eq!(format!("{:?}", search.pending_search), progress);
            assert_eq!(search.query(), "needle");
        }
        search.finish(terminal.grid());
        search.next();
        let selected = search.current_match_index();
        let matches = search.matches.clone();
        for edit in [
            End,
            Delete,
            DeleteWord,
            Home,
            Backspace,
            BackspaceWord,
            Right,
            Left,
            WordRight,
            WordLeft,
        ] {
            cursor = search.edit_query(cursor, edit, terminal.grid()).unwrap();
            search.confirm();
            assert!(search.resume_input_at(cursor));
            assert!(!search.is_searching());
            assert_eq!(search.current_match_index(), selected);
            assert_eq!(search.matches, matches);
        }
        assert_eq!(search.edit_query(0, Delete, terminal.grid()), Some(0));
        assert_eq!(search.query(), "eedle");
        assert!(search.is_searching());
        assert_eq!(search.match_count(), 0);
        assert!(search.current().is_none());
        search.finish(terminal.grid());
        assert_eq!(search.match_count(), 200);
    }

    #[test]
    fn query_edits_handle_joined_clusters_invalid_cursors_and_regex_recompilation() {
        use crate::ui::text_input::TextInputEdit::*;
        let terminal = termeric_core::Terminal::new(40, 2);
        let mut search = Search::new();
        for edit in [Backspace, Delete] {
            assert!(search.set_query("\u{301}x", terminal.grid()));
            assert!(search.insert_text(0, "e", terminal.grid()));
            assert_eq!(search.edit_query(1, edit, terminal.grid()), Some(0));
            assert_eq!(search.query(), "x");
        }
        assert!(search.set_query("界", terminal.grid()));
        for cursor in [1, 2, 4, usize::MAX] {
            for edit in [
                Home,
                End,
                Left,
                Right,
                Backspace,
                Delete,
                WordLeft,
                WordRight,
                BackspaceWord,
                DeleteWord,
            ] {
                assert_eq!(search.edit_query(cursor, edit, terminal.grid()), None);
                assert_eq!(search.query(), "界");
            }
        }
        search.toggle_regex(terminal.grid());
        for edit in [Delete, DeleteWord] {
            assert!(search.set_query("[a", terminal.grid()));
            assert!(search.regex_error.is_some());
            assert_eq!(search.edit_query(0, edit, terminal.grid()), Some(0));
            assert_eq!(search.query(), "a");
            assert!(search.regex_error.is_none());
            assert!(search.is_searching());
        }
    }

    #[test]
    fn full_history_search_is_bounded_cancellable_and_hides_partial_results() {
        let mut terminal = termeric_core::Terminal::new(40, 2);
        terminal.grid_mut().set_scrollback_limit(1_000);
        for line in 0..200 {
            terminal.process(format!("{line:03} needle searchable\r\n").as_bytes());
        }

        let mut search = Search::new();
        search.start();
        search.push_char('n', terminal.grid());
        assert!(search.is_searching());
        assert_eq!(search.match_count(), 0);
        assert!(search.current().is_none());
        assert!(search.build_info(1).searching);

        assert!(search.advance_rows(terminal.grid(), 10));
        assert!(search.is_searching());
        assert_eq!(search.match_count(), 0, "partial results must stay hidden");
        assert_eq!(search.matching_line_count(), 0);
        assert!(search.advance_for(terminal.grid(), Duration::ZERO));

        // Mutating the query cancels the partial scan and starts fresh. Typing
        // while that replacement scan is pending must remain correct too.
        for character in "eedle".chars() {
            search.push_char(character, terminal.grid());
        }
        search.finish(terminal.grid());
        assert!(!search.is_searching());
        assert_eq!(search.match_count(), 200);
        assert_eq!(search.current_match_index(), Some(1));

        search.push_char('x', terminal.grid());
        assert!(search.is_searching());
        search.cancel();
        assert!(!search.is_searching());
        assert_eq!(search.match_count(), 0);
    }

    #[test]
    fn pending_search_restarts_after_same_shape_edits_and_grid_replacement() {
        let mut terminal = termeric_core::Terminal::new(30, 4);
        terminal.process(b"needle\r\nneedle");
        let mut search = Search::new();
        search.set_query("needle", terminal.grid());
        assert!(search.advance_rows(terminal.grid(), 1));
        terminal.process(b"\x1b[H\x1b[2Kerased");
        search.finish(terminal.grid());
        assert_eq!(search.match_count(), 1);
        assert_eq!(search.current().unwrap().row, 1);

        search.refresh(terminal.grid());
        assert!(search.advance_rows(terminal.grid(), 2));
        let replacement = termeric_core::Terminal::new(30, 4);
        search.finish(replacement.grid());
        assert_eq!(search.match_count(), 0);
    }

    #[test]
    fn pending_search_restarts_when_capped_history_rotates_without_changing_shape() {
        let mut terminal = termeric_core::Terminal::new(30, 3);
        terminal.grid_mut().set_scrollback_limit(4);
        for index in 0..6 {
            terminal.process(format!("old{index} needle\r\n").as_bytes());
        }
        assert_eq!(terminal.grid().scrollback_len(), 4);
        let mut search = Search::new();
        search.set_query("old0", terminal.grid());
        assert!(search.advance_rows(terminal.grid(), 1));
        terminal.process(b"newest\r\n");
        assert_eq!(terminal.grid().scrollback_len(), 4);
        search.finish(terminal.grid());
        assert_eq!(search.match_count(), 0);
    }

    #[test]
    fn grid_shape_change_restarts_a_pending_scan() {
        let mut terminal = termeric_core::Terminal::new(40, 2);
        terminal.grid_mut().set_scrollback_limit(1_000);
        for line in 0..200 {
            terminal.process(format!("{line:03} needle content\r\n").as_bytes());
        }

        let mut search = Search::new();
        search.start();
        for character in "needle".chars() {
            search.push_char(character, terminal.grid());
        }
        assert!(search.advance_rows(terminal.grid(), 10));
        terminal.resize(20, 3);
        search.finish(terminal.grid());

        assert!(!search.is_searching());
        assert_eq!(search.match_count(), 200);
    }

    #[test]
    fn dense_results_keep_exact_totals_and_filter_rows_with_bounded_recent_matches() {
        let mut search = Search::new();
        search.start();
        search.query = "a".to_string();
        search.compile_regex_if_needed();
        for row in 0..3_000 {
            let line = SearchLine {
                text: "a".repeat(100),
                first_row: row,
                cells: None,
            };
            search.find_matches_in_line(&line, 0..100);
        }
        search.finish_update();

        assert_eq!(search.match_count(), 300_000);
        assert_eq!(search.retained_match_count(), MAX_RETAINED_SEARCH_MATCHES);
        assert!(search.results_truncated());
        assert_eq!(search.matching_line_count(), 3_000);
        assert!(search.result_index_memory_bytes() <= MAX_SEARCH_RESULT_INDEX_BYTES);

        let omitted = 300_000 - MAX_RETAINED_SEARCH_MATCHES;
        assert_eq!(search.current_match_index(), Some(omitted + 1));
        assert_eq!(search.current().unwrap().row, (omitted / 100) as i32);
        assert!(
            !search.is_match(0, 0),
            "omitted old matches are not highlighted"
        );
        assert!(search.is_match(2_999, 99));

        search.toggle_filter();
        let initial_rows = search.filtered_rows(10);
        assert_eq!(initial_rows.len(), 10);
        assert_eq!(initial_rows[0], (omitted / 100) as i32);
        search.scroll_filter(-5.0, 10);
        assert_eq!(search.filtered_rows(10)[0], (omitted / 100 + 5) as i32);
        search.scroll_filter(5.0, 10);
        assert_eq!(search.filtered_rows(10)[0], (omitted / 100) as i32);

        search.prev();
        assert_eq!(search.current_match_index(), Some(300_000));
        assert_eq!(search.current().unwrap().row, 2_999);
        search.next();
        assert_eq!(search.current_match_index(), Some(omitted + 1));
    }

    #[test]
    fn maximum_configured_history_and_retained_window_fit_the_index_budget() {
        let mut search = Search::new();
        for index in 0..MAX_RETAINED_SEARCH_MATCHES {
            record_retained_match(
                &mut search.matches,
                &mut search.retained_match_start,
                &mut search.total_match_count,
                SearchMatch {
                    row: index as i32,
                    end_row: index as i32,
                    start_col: 0,
                    end_col: 1,
                },
            );
        }
        // Config validation caps retained history at one million lines.
        search.matching_rows.resize(1_000_000, 0);

        assert!(search.result_index_memory_bytes() <= MAX_SEARCH_RESULT_INDEX_BYTES);
        search.cancel();
        assert_eq!(search.result_index_memory_bytes(), 0);
    }
}
#[test]
fn multicell_text_is_indexed_once_across_its_full_column_span() {
    let mut terminal = termeric_core::Terminal::new(10, 3);
    terminal.process(b"\x1b]66;s=2:w=2;title\x07!");
    let line = SearchLine::slice(terminal.grid(), 0, None, i32::MAX, 1).line;
    assert_eq!(line.text.trim_end(), "title!");
    assert_eq!((line.span(0, 5).start_col, line.span(0, 5).end_col), (0, 4));
    assert_eq!((line.span(5, 6).start_col, line.span(5, 6).end_col), (4, 5));
}

#[test]
fn matches_continue_across_soft_wraps_but_not_hard_line_breaks() {
    let search_for = |output: &[u8], query: &str| {
        let mut terminal = termeric_core::Terminal::new(8, 4);
        terminal.process(output);
        let mut search = Search::new();
        search.start();
        search.set_query(query, terminal.grid());
        search.finish(terminal.grid());
        search
    };

    // "hello" wraps after its third letter.
    let search = search_for(b"xxxxxhello world", "hello");
    assert_eq!(search.match_count(), 1);
    assert_eq!(
        search.current(),
        Some(&SearchMatch {
            row: 0,
            start_col: 5,
            end_row: 1,
            end_col: 2,
        })
    );
    for (row, col) in [(0, 5), (0, 7), (1, 0), (1, 1)] {
        assert!(search.is_match(row, col) && search.is_current_match(row, col));
    }
    assert!(!search.is_match(0, 4) && !search.is_match(1, 2));
    // Filter mode shows both rows the match touches.
    assert_eq!(search.matching_rows, [0, 1]);
    // The rest of the wrapped line is searched once, not again from row 1.
    assert_eq!(search_for(b"xxxxxhello hello", "hello").match_count(), 2);

    assert_eq!(search_for(b"xxxxxhel\r\nlo", "hello").match_count(), 0);

    // A match that starts on a wrapped line's second row is placed there.
    let search = search_for(b"abcdefghijhello!", "hello!");
    assert_eq!(search.match_count(), 1);
    assert_eq!(search.matching_rows, [1]);
    assert_eq!(
        search.current().map(|found| (found.row, found.start_col)),
        Some((1, 2))
    );

    // A wide character pushed to the next row leaves padding that isn't text.
    let search = search_for("abcdefg界x".as_bytes(), "g界");
    assert_eq!(search.match_count(), 1);
    assert_eq!(
        search.current().map(|found| (found.end_row, found.end_col)),
        Some((1, 2))
    );
}

#[test]
fn long_soft_wrapped_lines_are_searched_in_bounded_slices() {
    const COLS: usize = 20;
    const ROWS: usize = 300;
    let slice_chars = MAX_LINE_SLICE_ROWS as usize * COLS;
    // A needle longer than two rows, so a match that starts just before a
    // slice ends needs more than the next row to finish.
    let needle = format!("needle{}", "z".repeat(2 * COLS));
    // Needles across the first two slice ends and one near the end of the
    // line, which takes several slices to reach.
    let starts = [
        slice_chars - 3,
        2 * slice_chars - 3,
        COLS * ROWS - needle.len() - 1,
    ];
    let mut line = "x".repeat(COLS * ROWS);
    for start in starts {
        line.replace_range(start..start + needle.len(), &needle);
    }
    let mut terminal = termeric_core::Terminal::new(COLS, 4);
    terminal.grid_mut().set_scrollback_limit(1_000);
    terminal.process(line.as_bytes());
    terminal.process(format!("\r\n{needle}").as_bytes());
    let first_row = -(terminal.grid().scrollback_len() as i32);

    let mut search = Search::new();
    search.start();
    search.set_query(&needle, terminal.grid());
    let mut slices = 0;
    loop {
        let before = search.pending_search.as_ref().unwrap().next_row;
        if !search.advance_rows(terminal.grid(), 1) {
            break;
        }
        let after = search.pending_search.as_ref().unwrap().next_row;
        assert!(after - before <= MAX_LINE_SLICE_ROWS, "{before}..{after}");
        slices += 1;
    }
    assert!(slices >= ROWS / MAX_LINE_SLICE_ROWS as usize, "{slices}");

    let cell = |index: usize| (first_row + (index / COLS) as i32, index % COLS);
    let found = |start: usize| {
        let (row, start_col) = cell(start);
        let (end_row, last_col) = cell(start + needle.len() - 1);
        SearchMatch {
            row,
            start_col,
            end_row,
            end_col: last_col + 1,
        }
    };
    let mut expected = starts.map(found).to_vec();
    // The next line holds one more, past its first row.
    expected.push(found(COLS * ROWS));
    assert_eq!(search.matches, expected);
}

#[test]
fn slices_of_a_long_line_find_what_one_whole_line_search_would() {
    const COLS: usize = 20;
    let chars = COLS * (3 * MAX_LINE_SLICE_ROWS as usize + 5);
    let mut terminal = termeric_core::Terminal::new(COLS, 4);
    terminal.grid_mut().set_scrollback_limit(1_000);
    terminal.process("a".repeat(chars).as_bytes());
    terminal.process(b"\r\n");
    let grid = terminal.grid();
    let search_for = |query: &str, regex: bool, case_sensitive: bool| {
        let mut search = Search::new();
        search.start();
        if regex {
            search.toggle_regex(grid);
        }
        if case_sensitive {
            search.toggle_case_sensitive(grid);
        }
        assert!(search.set_query(query, grid));
        search.finish(grid);
        search
    };

    // A slice holds 1,280 characters, so matches of three straddle every
    // slice end; each one starts where the last ended, as in one search.
    for (regex, case_sensitive) in [(false, false), (false, true), (true, false)] {
        let search = search_for("aaa", regex, case_sensitive);
        assert_eq!(search.match_count(), chars / 3, "{regex} {case_sensitive}");
        let last = search.matches.last().unwrap();
        let last_start = (chars / 3 - 1) * 3;
        assert_eq!(
            (last.row - search.matches[0].row, last.start_col),
            ((last_start / COLS) as i32, last_start % COLS)
        );
    }
    assert_eq!(search_for("a{50}", true, false).match_count(), chars / 50);
    // Anchors and word boundaries see the text around a slice, so they
    // match only at the true ends of the line.
    for anchored in ["^a", "a$", r"\ba", r"a\b"] {
        assert_eq!(
            search_for(anchored, true, false).match_count(),
            1,
            "{anchored}"
        );
    }
}
