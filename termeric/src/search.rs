//! Terminal search functionality
//!
//! Provides incremental search through terminal history and visible content.

use termeric_core::{Grid, CellContent};

/// A search match in the terminal
#[derive(Clone, Debug, PartialEq)]
pub struct SearchMatch {
    /// Row of the match (can be negative for scrollback)
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
    /// All matches found
    matches: Vec<SearchMatch>,
    /// Index of current match (-1 if no current match)
    current_match: Option<usize>,
    /// Whether search is active
    active: bool,
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
            matches: Vec::new(),
            current_match: None,
            active: false,
        }
    }

    /// Start a new search
    pub fn start(&mut self) {
        self.active = true;
        self.query.clear();
        self.matches.clear();
        self.current_match = None;
    }

    /// Cancel the search
    pub fn cancel(&mut self) {
        self.active = false;
        self.query.clear();
        self.matches.clear();
        self.current_match = None;
    }

    /// Check if search is active
    pub fn is_active(&self) -> bool {
        self.active
    }

    /// Get the current query
    pub fn query(&self) -> &str {
        &self.query
    }

    /// Set the search query and update matches
    pub fn set_query(&mut self, query: &str, grid: &Grid) {
        self.query = query.to_string();
        self.update_matches(grid);
    }

    /// Add a character to the query
    pub fn push_char(&mut self, c: char, grid: &Grid) {
        self.query.push(c);
        self.update_matches(grid);
    }

    /// Remove last character from query
    pub fn pop_char(&mut self, grid: &Grid) {
        self.query.pop();
        self.update_matches(grid);
    }

    /// Toggle case sensitivity
    pub fn toggle_case_sensitive(&mut self, grid: &Grid) {
        self.case_sensitive = !self.case_sensitive;
        self.update_matches(grid);
    }

    /// Is case sensitive?
    pub fn is_case_sensitive(&self) -> bool {
        self.case_sensitive
    }

    /// Get current match count
    pub fn match_count(&self) -> usize {
        self.matches.len()
    }

    /// Get current match index (1-based for display)
    pub fn current_match_index(&self) -> Option<usize> {
        self.current_match.map(|i| i + 1)
    }

    /// Get all matches
    pub fn matches(&self) -> &[SearchMatch] {
        &self.matches
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

    /// Update matches based on current query
    fn update_matches(&mut self, grid: &Grid) {
        self.matches.clear();
        self.current_match = None;

        if self.query.is_empty() {
            return;
        }

        let query = if self.case_sensitive {
            self.query.clone()
        } else {
            self.query.to_lowercase()
        };

        // Search visible rows
        for row in 0..grid.rows() {
            let line = self.extract_line_text(grid, row);
            let search_line = if self.case_sensitive {
                line.clone()
            } else {
                line.to_lowercase()
            };

            let mut search_start = 0;
            while let Some(pos) = search_line[search_start..].find(&query) {
                let start_col = search_start + pos;
                let end_col = start_col + query.len();

                self.matches.push(SearchMatch {
                    row: row as i32,
                    start_col,
                    end_col,
                });

                search_start = end_col;
            }
        }

        // TODO: Search scrollback buffer

        // Set current match to first if we have matches
        if !self.matches.is_empty() {
            self.current_match = Some(0);
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
            SearchMatch { row: 0, start_col: 0, end_col: 4 },
            SearchMatch { row: 1, start_col: 5, end_col: 9 },
            SearchMatch { row: 2, start_col: 0, end_col: 4 },
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
}
