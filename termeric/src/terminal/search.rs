//! Terminal search functionality
//!
//! Provides incremental search through terminal history and visible content.
//! Supports both literal and regex search modes.

use regex::{Regex, RegexBuilder};
use std::ops::RangeInclusive;
use std::time::{Duration, Instant};
use termeric_core::{CellContent, Grid};

/// Navigation/highlight occurrences retain a fixed recent window. Complete
/// total counts and matching-row/filter data continue across the whole grid.
pub(crate) const MAX_RETAINED_SEARCH_MATCHES: usize = 131_072;
pub(crate) const MAX_SEARCH_RESULT_INDEX_BYTES: usize = 8 * 1024 * 1024;
pub(crate) const MAX_SEARCH_QUERY_BYTES: usize = 4 * 1024;

struct SearchLine {
    text: String,
    byte_to_col: Option<Vec<usize>>,
}

#[derive(Debug)]
enum SearchRows {
    All {
        next_history: usize,
        next_visible: usize,
    },
    Candidates {
        rows: Vec<i32>,
        next: usize,
    },
}

#[derive(Debug)]
struct PendingSearch {
    cols: usize,
    history_len: usize,
    visible_rows: usize,
    rows: SearchRows,
}

impl PendingSearch {
    fn new(grid: &Grid, candidate_rows: Option<Vec<i32>>) -> Self {
        Self {
            cols: grid.cols(),
            history_len: grid.scrollback_len(),
            visible_rows: grid.rows(),
            rows: candidate_rows.map_or(
                SearchRows::All {
                    next_history: 0,
                    next_visible: 0,
                },
                |rows| SearchRows::Candidates { rows, next: 0 },
            ),
        }
    }

    fn matches_shape(&self, grid: &Grid) -> bool {
        self.cols == grid.cols()
            && self.history_len == grid.scrollback_len()
            && self.visible_rows == grid.rows()
    }

    fn next_row(&mut self) -> Option<i32> {
        match &mut self.rows {
            SearchRows::All {
                next_history,
                next_visible,
            } => {
                if *next_history < self.history_len {
                    let index = *next_history;
                    *next_history += 1;
                    return Some(-((self.history_len - index) as i32));
                }
                if *next_visible < self.visible_rows {
                    let row = *next_visible as i32;
                    *next_visible += 1;
                    return Some(row);
                }
                None
            }
            SearchRows::Candidates { rows, next } => {
                let row = rows.get(*next).copied();
                *next += usize::from(row.is_some());
                row
            }
        }
    }

    fn is_complete(&self) -> bool {
        match &self.rows {
            SearchRows::All {
                next_history,
                next_visible,
            } => *next_history >= self.history_len && *next_visible >= self.visible_rows,
            SearchRows::Candidates { rows, next } => *next >= rows.len(),
        }
    }
}

impl SearchLine {
    fn from_row(row: &termeric_core::RowView<'_>, cols: usize) -> Self {
        let mut text = String::with_capacity(cols);
        let mut byte_to_col: Option<Vec<usize>> = None;
        for col in 0..cols {
            let Some(cell) = row.get(col) else { continue };
            if cell.is_wide_char_spacer() {
                continue;
            }
            let start = text.len();
            match &cell.content {
                CellContent::Empty => text.push(' '),
                CellContent::Char(character) => text.push(*character),
                CellContent::Grapheme(grapheme) => text.push_str(grapheme),
            }
            let end = text.len();
            if byte_to_col.is_none() && (start != col || end - start != cell.width()) {
                byte_to_col = Some((0..=start).collect());
            }
            if let Some(byte_to_col) = &mut byte_to_col {
                byte_to_col.resize(end + 1, col);
                byte_to_col[start] = col;
                byte_to_col[end] = col + cell.width();
            }
        }
        Self { text, byte_to_col }
    }

    fn columns_for(&self, start: usize, end: usize) -> (usize, usize) {
        self.byte_to_col
            .as_ref()
            .map_or((start, end), |byte_to_col| {
                (
                    byte_to_col[start.min(byte_to_col.len() - 1)],
                    byte_to_col[end.min(byte_to_col.len() - 1)],
                )
            })
    }
}

/// A search match in the terminal
#[derive(Clone, Debug, PartialEq)]
pub struct SearchMatch {
    /// Row of the match (negative for scrollback, 0+ for visible grid)
    pub row: i32,
    /// Starting column
    pub start_col: usize,
    /// Ending column (exclusive)
    pub end_col: usize,
}

impl SearchMatch {
    /// Check if a position is within this match
    pub fn contains(&self, row: i32, col: usize) -> bool {
        self.row == row && col >= self.start_col && col < self.end_col
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
    /// In-progress full/candidate scan. Work is consumed in bounded slices by
    /// the event-driven render loop so keyboard dispatch never walks all history.
    pending_search: Option<PendingSearch>,
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

    fn start_with_scope(&mut self, row_scope: Option<RangeInclusive<i32>>) {
        self.active = true;
        self.query.clear();
        self.matches.clear();
        self.retained_match_start = 0;
        self.total_match_count = 0;
        self.matching_rows.clear();
        self.filter_row_offset = 0;
        self.current_match = None;
        self.compiled_regex = None;
        self.regex_error = None;
        self.row_scope = row_scope;
        self.pending_search = None;
        // Keep filter_mode state - user might want to continue filtering
    }

    /// Cancel the search
    pub fn cancel(&mut self) {
        self.active = false;
        self.filter_mode = false; // Disable filter when canceling
        self.query.clear();
        self.matches = Vec::new();
        self.retained_match_start = 0;
        self.total_match_count = 0;
        self.matching_rows = Vec::new();
        self.filter_row_offset = 0;
        self.current_match = None;
        self.compiled_regex = None;
        self.regex_error = None;
        self.row_scope = None;
        self.pending_search = None;
    }

    /// Confirm the search (keep highlights, exit search input mode)
    pub fn confirm(&mut self) {
        self.active = false;
        // Keep matches, current_match, and filter_mode for continued highlighting/filtering
    }

    /// Toggle filter mode (show only matching lines)
    pub fn toggle_filter(&mut self) {
        self.filter_mode = !self.filter_mode;
        if self.filter_mode {
            self.sync_filter_to_current();
        }
    }

    /// Is filter mode enabled?
    pub fn is_filter_mode(&self) -> bool {
        self.filter_mode
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

    /// Add a character to the query
    #[cfg(test)]
    pub fn push_char(&mut self, c: char, grid: &Grid) {
        let candidate_rows =
            (!self.use_regex && !self.query.is_empty() && self.pending_search.is_none())
                .then(|| std::mem::take(&mut self.matching_rows));
        self.query.push(c);
        self.compile_regex_if_needed();
        self.begin_update(grid, candidate_rows);
    }

    /// Insert committed platform text at a UTF-8 cursor boundary.
    pub fn insert_text(&mut self, cursor: usize, text: &str, grid: &Grid) -> bool {
        if cursor <= self.query.len()
            && self.query.is_char_boundary(cursor)
            && self.query.len().saturating_add(text.len()) <= MAX_SEARCH_QUERY_BYTES
        {
            self.query.insert_str(cursor, text);
            self.compile_regex_if_needed();
            self.begin_update(grid, None);
            true
        } else {
            false
        }
    }

    /// Remove the character immediately before a UTF-8 cursor and return the
    /// new cursor position. This keeps editing behavior independent from the
    /// physical end of the query.
    pub fn remove_char_before(&mut self, cursor: usize, grid: &Grid) -> Option<usize> {
        if cursor == 0 || cursor > self.query.len() || !self.query.is_char_boundary(cursor) {
            return None;
        }
        let previous = self.query[..cursor].char_indices().next_back()?.0;
        self.query.drain(previous..cursor);
        self.compile_regex_if_needed();
        self.begin_update(grid, None);
        Some(previous)
    }

    /// Replace the complete query from an accessibility text action.
    pub fn set_query(&mut self, query: &str, grid: &Grid) -> bool {
        if query.len() > MAX_SEARCH_QUERY_BYTES {
            return false;
        }
        self.query.clear();
        self.query.push_str(query);
        self.compile_regex_if_needed();
        self.begin_update(grid, None);
        true
    }

    /// Toggle case sensitivity
    pub fn toggle_case_sensitive(&mut self, grid: &Grid) {
        self.case_sensitive = !self.case_sensitive;
        self.compile_regex_if_needed();
        self.begin_update(grid, None);
    }

    /// Toggle regex mode
    pub fn toggle_regex(&mut self, grid: &Grid) {
        self.use_regex = !self.use_regex;
        self.compile_regex_if_needed();
        self.begin_update(grid, None);
    }

    /// Re-run the current query against a different terminal grid while
    /// preserving the search controls and input state.
    pub fn refresh(&mut self, grid: &Grid) {
        self.compile_regex_if_needed();
        self.begin_update(grid, None);
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

        let pattern = if self.case_sensitive {
            if self.use_regex {
                self.query.clone()
            } else {
                regex::escape(&self.query)
            }
        } else if self.use_regex {
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
                self.regex_error = Some(e.to_string());
            }
        }
    }

    fn begin_update(&mut self, grid: &Grid, candidate_rows: Option<Vec<i32>>) {
        self.matches.clear();
        self.retained_match_start = 0;
        self.total_match_count = 0;
        self.matching_rows.clear();
        self.filter_row_offset = 0;
        self.current_match = None;
        self.pending_search = None;

        if self.query.is_empty() || self.compiled_regex.is_none() {
            return;
        }

        self.pending_search = Some(PendingSearch::new(grid, candidate_rows));
    }

    /// Consume at most `max_rows` rows of pending search work.
    ///
    /// Returns `true` while more work remains. A grid geometry/history change
    /// invalidates partial results and restarts a full scan against the new
    /// shape so row offsets cannot be mixed across layouts.
    pub fn advance_rows(&mut self, grid: &Grid, max_rows: usize) -> bool {
        let Some(pending) = self.pending_search.as_ref() else {
            return false;
        };
        if !pending.matches_shape(grid) {
            self.begin_update(grid, None);
        }

        let Some(mut pending) = self.pending_search.take() else {
            return false;
        };
        for _ in 0..max_rows {
            let Some(row_index) = pending.next_row() else {
                break;
            };
            if !self.row_is_in_scope(row_index) {
                continue;
            }
            if let Some(row) = grid.row_at_offset(row_index) {
                let line = SearchLine::from_row(&row, grid.cols());
                self.find_matches_in_line(&line, row_index);
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

    /// Find all matches in a line and add them to self.matches
    fn find_matches_in_line(&mut self, line: &SearchLine, row: i32) {
        let previous_total = self.total_match_count;
        if !self.use_regex && self.case_sensitive {
            for (start, found) in line.text.match_indices(&self.query) {
                let (start_col, end_col) = line.columns_for(start, start + found.len());
                record_retained_match(
                    &mut self.matches,
                    &mut self.retained_match_start,
                    &mut self.total_match_count,
                    SearchMatch {
                        row,
                        start_col,
                        end_col,
                    },
                );
            }
        } else if !self.use_regex && self.query.is_ascii() && line.text.is_ascii() {
            let query = self.query.as_bytes();
            let mut search_start = 0;
            while let Some(relative_start) = line.text.as_bytes()[search_start..]
                .windows(query.len())
                .position(|candidate| candidate.eq_ignore_ascii_case(query))
            {
                let start = search_start + relative_start;
                let end = start + query.len();
                let (start_col, end_col) = line.columns_for(start, end);
                record_retained_match(
                    &mut self.matches,
                    &mut self.retained_match_start,
                    &mut self.total_match_count,
                    SearchMatch {
                        row,
                        start_col,
                        end_col,
                    },
                );
                search_start = end;
            }
        } else if let Some(regex) = &self.compiled_regex {
            for found in regex.find_iter(&line.text) {
                let (start_col, end_col) = line.columns_for(found.start(), found.end());
                record_retained_match(
                    &mut self.matches,
                    &mut self.retained_match_start,
                    &mut self.total_match_count,
                    SearchMatch {
                        row,
                        start_col,
                        end_col,
                    },
                );
            }
        }

        if self.total_match_count > previous_total {
            if self.matching_rows.last() != Some(&row) {
                self.matching_rows.push(row);
            }
        }
    }
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
    let index = matches.partition_point(|search_match| {
        search_match.row < row || (search_match.row == row && search_match.end_col <= col)
    });
    matches
        .get(index)
        .is_some_and(|search_match| search_match.contains(row, col))
}

/// Information needed to render the search bar
#[derive(Clone, Debug)]
pub struct SearchBarInfo {
    /// Current search query
    pub query: String,
    /// Whether case-sensitive mode is enabled
    pub case_sensitive: bool,
    /// Whether regex mode is enabled
    pub use_regex: bool,
    /// Whether filter mode is enabled (show only matching lines)
    pub filter_mode: bool,
    /// Current match index (1-based, None if no matches)
    pub current_match: Option<usize>,
    /// Total match count
    pub match_count: usize,
    /// Recent occurrences retained for navigation and highlighting
    pub retained_match_count: usize,
    /// Whether older occurrences fell outside the retained recent window
    pub results_truncated: bool,
    /// Whether the complete result set is still being assembled
    pub searching: bool,
    /// Number of lines containing matches (for filter mode display)
    pub matching_line_count: usize,
    /// Regex error message if any
    pub regex_error: Option<String>,
    /// Cursor position in query
    pub cursor_pos: usize,
    /// Whether the query currently owns keyboard input.
    pub is_active: bool,
}

impl SearchBarInfo {
    pub fn count_text(&self) -> String {
        if self.regex_error.is_some() {
            "Invalid regex".to_string()
        } else if self.searching {
            "Searching…".to_string()
        } else if self.match_count > 0 && self.results_truncated {
            format!(
                "{}/{} · recent",
                self.current_match.unwrap_or(0),
                self.match_count
            )
        } else if self.match_count > 0 {
            format!("{}/{}", self.current_match.unwrap_or(0), self.match_count)
        } else if !self.query.is_empty() {
            "No matches".to_string()
        } else {
            String::new()
        }
    }

    pub fn accessible_status(&self) -> String {
        if let Some(error) = &self.regex_error {
            return format!("Regular expression error: {error}");
        }
        if self.searching {
            return "Searching terminal history".to_string();
        }
        if self.match_count == 0 {
            return if self.query.is_empty() {
                "Enter a terminal search query".to_string()
            } else {
                "No matches".to_string()
            };
        }
        let current = self.current_match.unwrap_or(0);
        let suffix = if self.results_truncated {
            ", navigation retains only the most recent matches"
        } else {
            ""
        };
        format!(
            "Match {current} of {}; {} matching lines{suffix}",
            self.match_count, self.matching_line_count
        )
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct SearchBarRect {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

impl SearchBarRect {
    pub fn contains(self, x: f32, y: f32) -> bool {
        x >= self.x && x < self.x + self.width && y >= self.y && y < self.y + self.height
    }
}

/// One shared geometry contract for painting, pointer hit-testing, and the
/// native accessibility tree.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SearchBarLayout {
    pub bar: SearchBarRect,
    pub input: SearchBarRect,
    pub filter: SearchBarRect,
    pub case_sensitive: SearchBarRect,
    pub regex: SearchBarRect,
    pub status: SearchBarRect,
    pub previous: Option<SearchBarRect>,
    pub next: Option<SearchBarRect>,
    pub close: SearchBarRect,
    pub text_y: f32,
}

impl SearchBarLayout {
    pub fn calculate(
        info: &SearchBarInfo,
        bar_x: f32,
        bar_width: f32,
        bar_y: f32,
        cell_width: f32,
        cell_height: f32,
    ) -> Self {
        let padding = 8.0;
        let bar_height = cell_height + padding * 2.0;
        let mut right = bar_x + bar_width - padding;
        let rect_from_right = |right: &mut f32, width: f32| {
            *right -= width;
            SearchBarRect {
                x: *right,
                y: bar_y + padding,
                width,
                height: cell_height,
            }
        };

        let close = rect_from_right(&mut right, cell_width);
        right -= cell_width * 0.5;
        let (next, previous) = if info.retained_match_count > 0 {
            let next = rect_from_right(&mut right, cell_width);
            let previous = rect_from_right(&mut right, cell_width);
            right -= cell_width * 0.5;
            (Some(next), Some(previous))
        } else {
            (None, None)
        };

        let count_width = info.count_text().chars().count() as f32 * cell_width;
        let status = rect_from_right(&mut right, count_width);
        if count_width > 0.0 {
            right -= cell_width;
        }
        right -= cell_width * 0.5;
        let regex = rect_from_right(&mut right, 4.0 * cell_width);
        right -= cell_width;
        right -= cell_width * 0.5;
        let case_sensitive = rect_from_right(&mut right, 4.0 * cell_width);
        right -= cell_width;
        let filter_chars = if info.filter_mode {
            format!("[⊜{}]", info.matching_line_count).chars().count()
        } else {
            3
        };
        right -= cell_width * 0.5;
        let filter = rect_from_right(&mut right, filter_chars as f32 * cell_width);

        let input_x = bar_x + padding;
        let input_right = (filter.x - cell_width * 0.5).max(input_x);
        Self {
            bar: SearchBarRect {
                x: bar_x,
                y: bar_y,
                width: bar_width,
                height: bar_height,
            },
            input: SearchBarRect {
                x: input_x,
                y: bar_y + padding,
                width: (input_right - input_x).max(0.0),
                height: cell_height,
            },
            filter,
            case_sensitive,
            regex,
            status,
            previous,
            next,
            close,
            text_y: bar_y + padding,
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

    // Note: These tests would require a mock Grid implementation
    // For now, we just test the basic functionality

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
                start_col: 0,
                end_col: 4,
            },
            SearchMatch {
                row: 1,
                start_col: 5,
                end_col: 9,
            },
            SearchMatch {
                row: 2,
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

        // Should compile with (?i) prefix
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
        assert_eq!(search.remove_char_before(4, terminal.grid()), Some(1));
        assert_eq!(search.query(), "ac");

        let oversized = "x".repeat(MAX_SEARCH_QUERY_BYTES + 1);
        assert!(!search.set_query(&oversized, terminal.grid()));
        assert_eq!(search.query(), "ac");
        assert!(!search.insert_text(1, &oversized, terminal.grid()));
        assert_eq!(search.query(), "ac");
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
                start_col: 0,
                end_col: 6,
            })
        );
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
    fn completed_literal_extension_moves_candidate_rows_without_copying() {
        let mut terminal = termeric_core::Terminal::new(40, 2);
        terminal.grid_mut().set_scrollback_limit(1_000);
        for line in 0..200 {
            terminal.process(format!("{line:03} needle content\r\n").as_bytes());
        }

        let mut search = Search::new();
        search.start();
        search.push_char('n', terminal.grid());
        search.finish(terminal.grid());
        let candidate_count = search.matching_rows.len();
        let candidate_ptr = search.matching_rows.as_ptr();

        search.push_char('e', terminal.grid());
        let Some(PendingSearch {
            rows: SearchRows::Candidates { rows, .. },
            ..
        }) = search.pending_search.as_ref()
        else {
            panic!("literal extension should narrow completed candidate rows");
        };
        assert_eq!(rows.len(), candidate_count);
        assert_eq!(rows.as_ptr(), candidate_ptr);

        search.finish(terminal.grid());
        assert_eq!(search.match_count(), 200);
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
        let line = SearchLine {
            text: "a".repeat(100),
            byte_to_col: None,
        };
        for row in 0..3_000 {
            search.find_matches_in_line(&line, row);
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
