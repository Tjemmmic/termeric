//! Terminal search functionality
//!
//! Provides incremental search through terminal history and visible content.
//! Supports both literal and regex search modes.

use regex::Regex;
use std::ops::RangeInclusive;
use termeric_core::{CellContent, Grid};

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
    /// Compiled regex (if in regex mode and valid)
    compiled_regex: Option<Regex>,
    /// Regex compilation error (if any)
    regex_error: Option<String>,
    /// All matches found
    matches: Vec<SearchMatch>,
    /// Index of current match
    current_match: Option<usize>,
    /// Whether search is active
    active: bool,
    /// Rows that contain matches (for filter mode) - sorted and deduplicated
    matching_rows: Vec<i32>,
    /// Optional inclusive row scope (used by Harvest's Search Output).
    row_scope: Option<RangeInclusive<i32>>,
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
            current_match: None,
            active: false,
            matching_rows: Vec::new(),
            row_scope: None,
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
        self.matching_rows.clear();
        self.current_match = None;
        self.compiled_regex = None;
        self.regex_error = None;
        self.row_scope = row_scope;
        // Keep filter_mode state - user might want to continue filtering
    }

    /// Cancel the search
    pub fn cancel(&mut self) {
        self.active = false;
        self.filter_mode = false; // Disable filter when canceling
        self.query.clear();
        self.matches.clear();
        self.matching_rows.clear();
        self.current_match = None;
        self.compiled_regex = None;
        self.regex_error = None;
        self.row_scope = None;
    }

    /// Confirm the search (keep highlights, exit search input mode)
    pub fn confirm(&mut self) {
        self.active = false;
        // Keep matches, current_match, and filter_mode for continued highlighting/filtering
    }

    /// Toggle filter mode (show only matching lines)
    pub fn toggle_filter(&mut self) {
        self.filter_mode = !self.filter_mode;
    }

    /// Is filter mode enabled?
    pub fn is_filter_mode(&self) -> bool {
        self.filter_mode
    }

    /// Get the rows that contain matches (for filter mode)
    /// Returns sorted, deduplicated list of row indices
    pub fn matching_rows(&self) -> &[i32] {
        &self.matching_rows
    }

    /// Get the number of matching lines (distinct rows with matches)
    pub fn matching_line_count(&self) -> usize {
        self.matching_rows.len()
    }

    /// Check if search is active
    pub fn is_active(&self) -> bool {
        self.active
    }

    /// Check if we have search results to display (even when input closed)
    pub fn has_results(&self) -> bool {
        !self.matches.is_empty()
    }

    /// Get the current query
    pub fn query(&self) -> &str {
        &self.query
    }

    /// Add a character to the query
    pub fn push_char(&mut self, c: char, grid: &Grid) {
        self.query.push(c);
        self.compile_regex_if_needed();
        self.update_matches(grid);
    }

    /// Remove last character from query
    pub fn pop_char(&mut self, grid: &Grid) {
        self.query.pop();
        self.compile_regex_if_needed();
        self.update_matches(grid);
    }

    /// Toggle case sensitivity
    pub fn toggle_case_sensitive(&mut self, grid: &Grid) {
        self.case_sensitive = !self.case_sensitive;
        self.compile_regex_if_needed();
        self.update_matches(grid);
    }

    /// Toggle regex mode
    pub fn toggle_regex(&mut self, grid: &Grid) {
        self.use_regex = !self.use_regex;
        self.compile_regex_if_needed();
        self.update_matches(grid);
    }

    /// Get current match count
    pub fn match_count(&self) -> usize {
        self.matches.len()
    }

    /// Get current match index (1-based for display)
    pub fn current_match_index(&self) -> Option<usize> {
        self.current_match.map(|i| i + 1)
    }

    /// Get current match
    pub fn current(&self) -> Option<&SearchMatch> {
        self.current_match.and_then(|i| self.matches.get(i))
    }

    /// Move to next match
    pub fn next(&mut self) {
        if self.matches.is_empty() {
            return;
        }

        self.current_match = Some(match self.current_match {
            Some(i) => (i + 1) % self.matches.len(),
            None => 0,
        });
    }

    /// Move to previous match
    pub fn prev(&mut self) {
        if self.matches.is_empty() {
            return;
        }

        self.current_match = Some(match self.current_match {
            Some(0) | None => self.matches.len().saturating_sub(1),
            Some(i) => i - 1,
        });
    }

    /// Check if a cell position is part of a match
    pub fn is_match(&self, row: i32, col: usize) -> bool {
        self.matches.iter().any(|m| m.contains(row, col))
    }

    /// Check if a cell position is part of the current match
    pub fn is_current_match(&self, row: i32, col: usize) -> bool {
        self.current()
            .map(|m| m.contains(row, col))
            .unwrap_or(false)
    }

    /// Compile regex pattern if in regex mode
    fn compile_regex_if_needed(&mut self) {
        if !self.use_regex || self.query.is_empty() {
            self.compiled_regex = None;
            self.regex_error = None;
            return;
        }

        let pattern = if self.case_sensitive {
            self.query.clone()
        } else {
            format!("(?i){}", self.query)
        };

        match Regex::new(&pattern) {
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

    /// Update matches based on current query
    fn update_matches(&mut self, grid: &Grid) {
        self.matches.clear();
        self.matching_rows.clear();
        self.current_match = None;

        if self.query.is_empty() {
            return;
        }

        // If regex mode and we have an error, don't search
        if self.use_regex && self.regex_error.is_some() {
            return;
        }

        // Search scrollback buffer first (oldest to newest)
        // Scrollback rows are indexed as negative: -scrollback_len to -1
        let scrollback_len = grid.scrollback_len();
        for i in 0..scrollback_len {
            // Convert to negative row index: scrollback[0] is oldest, becomes row -(scrollback_len)
            let row_index = -((scrollback_len - i) as i32);
            if !self.row_is_in_scope(row_index) {
                continue;
            }
            if let Some(row) = grid.row_at_offset(row_index) {
                let line = Self::extract_row_text(row, grid.cols());
                self.find_matches_in_line(&line, row_index);
            }
        }

        // Search visible rows (0 to rows-1)
        for row in 0..grid.rows() {
            if !self.row_is_in_scope(row as i32) {
                continue;
            }
            let line = self.extract_line_text(grid, row);
            self.find_matches_in_line(&line, row as i32);
        }

        // Set current match to first if we have matches
        if !self.matches.is_empty() {
            self.current_match = Some(0);
        }

        // Build the matching_rows list (sorted, deduplicated)
        self.rebuild_matching_rows();
    }

    fn row_is_in_scope(&self, row: i32) -> bool {
        self.row_scope
            .as_ref()
            .is_none_or(|scope| scope.contains(&row))
    }

    /// Rebuild the matching_rows list from current matches
    fn rebuild_matching_rows(&mut self) {
        self.matching_rows.clear();
        for m in &self.matches {
            if self.matching_rows.last() != Some(&m.row) {
                self.matching_rows.push(m.row);
            }
        }
        // Matches are already in row order from search, so matching_rows is sorted
    }

    /// Find all matches in a line and add them to self.matches
    fn find_matches_in_line(&mut self, line: &str, row: i32) {
        if self.use_regex {
            if let Some(ref re) = self.compiled_regex {
                for mat in re.find_iter(line) {
                    self.matches.push(SearchMatch {
                        row,
                        start_col: mat.start(),
                        end_col: mat.end(),
                    });
                }
            }
        } else {
            // Literal search
            let (search_line, query) = if self.case_sensitive {
                (line.to_string(), self.query.clone())
            } else {
                (line.to_lowercase(), self.query.to_lowercase())
            };

            let mut search_start = 0;
            while let Some(pos) = search_line[search_start..].find(&query) {
                let start_col = search_start + pos;
                let end_col = start_col + query.len();

                self.matches.push(SearchMatch {
                    row,
                    start_col,
                    end_col,
                });

                search_start = end_col;
            }
        }
    }

    /// Extract text content from a grid row
    fn extract_line_text(&self, grid: &Grid, row: usize) -> String {
        let mut text = String::with_capacity(grid.cols());

        for col in 0..grid.cols() {
            if let Some(cell) = grid.cell(row, col) {
                match &cell.content {
                    CellContent::Empty => text.push(' '),
                    CellContent::Char(c) => text.push(*c),
                    CellContent::Grapheme(g) => text.push_str(g),
                }
            } else {
                text.push(' ');
            }
        }

        text
    }

    /// Extract text from a Row directly
    fn extract_row_text(row: &termeric_core::Row, cols: usize) -> String {
        let mut text = String::with_capacity(cols);

        for col in 0..cols {
            if let Some(cell) = row.get(col) {
                match &cell.content {
                    CellContent::Empty => text.push(' '),
                    CellContent::Char(c) => text.push(*c),
                    CellContent::Grapheme(g) => text.push_str(g),
                }
            } else {
                text.push(' ');
            }
        }

        text
    }
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
    /// Number of lines containing matches (for filter mode display)
    pub matching_line_count: usize,
    /// Regex error message if any
    pub regex_error: Option<String>,
    /// Cursor position in query
    pub cursor_pos: usize,
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
            matching_line_count: self.matching_line_count(),
            regex_error: self.regex_error.clone(),
            cursor_pos,
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
}
