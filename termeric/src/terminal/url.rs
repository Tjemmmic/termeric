//! URL detection and handling
//!
//! This module provides functionality for detecting URLs in terminal output
//! and handling click-to-open functionality.

use std::path::Path;
use std::sync::Arc;

/// A detected URL in the terminal
#[derive(Clone, Debug)]
pub struct DetectedUrl {
    /// The URL string
    pub url: String,
    /// Row where the URL starts
    pub start_row: usize,
    /// Column where the URL starts
    pub start_col: usize,
    /// Row where the URL ends
    pub end_row: usize,
    /// Column where the URL ends
    pub end_col: usize,
}

impl DetectedUrl {
    /// Check if a position is within this URL
    pub fn contains(&self, row: usize, col: usize) -> bool {
        if row < self.start_row || row > self.end_row {
            return false;
        }

        if self.start_row == self.end_row {
            // Single line URL
            return col >= self.start_col && col <= self.end_col;
        }

        if row == self.start_row {
            return col >= self.start_col;
        }

        if row == self.end_row {
            return col <= self.end_col;
        }

        // Middle row
        true
    }
}

/// URL detector for terminal content
pub struct UrlDetector {
    /// Common URL schemes to detect
    schemes: Vec<&'static str>,
}

impl Default for UrlDetector {
    fn default() -> Self {
        Self::new()
    }
}

impl UrlDetector {
    /// Create a new URL detector
    pub fn new() -> Self {
        Self {
            schemes: vec![
                "http://", "https://", "ftp://", "ftps://", "file://", "mailto:", "ssh://",
                "git://",
            ],
        }
    }

    /// Detect URLs in a line of text
    pub fn detect_in_line(&self, text: &str, row: usize) -> Vec<DetectedUrl> {
        let mut urls = Vec::new();

        for scheme in &self.schemes {
            let mut search_start = 0;

            while let Some(start) = text[search_start..].find(scheme) {
                let abs_start = search_start + start;

                // Find the end of the URL
                let url_start = &text[abs_start..];
                let end = self.find_url_end(url_start);

                if end > scheme.len() {
                    let url_str = &url_start[..end];

                    urls.push(DetectedUrl {
                        url: url_str.to_string(),
                        start_row: row,
                        start_col: abs_start,
                        end_row: row,
                        end_col: abs_start + end - 1,
                    });
                }

                search_start = abs_start + end;
            }
        }

        urls
    }

    /// Find the end of a URL starting from the beginning of the string
    fn find_url_end(&self, text: &str) -> usize {
        let mut end = 0;
        let mut paren_depth = 0;
        let mut bracket_depth = 0;

        for (i, c) in text.char_indices() {
            match c {
                // Valid URL characters
                'a'..='z' | 'A'..='Z' | '0'..='9' => end = i + c.len_utf8(),
                '-' | '.' | '_' | '~' | ':' | '/' | '?' | '#' | '[' | ']' | '@' | '!' | '$'
                | '&' | '\'' | '*' | '+' | ',' | ';' | '=' | '%' => {
                    end = i + c.len_utf8();
                    match c {
                        '(' => paren_depth += 1,
                        ')' => {
                            if paren_depth > 0 {
                                paren_depth -= 1;
                            } else {
                                break;
                            }
                        }
                        '[' => bracket_depth += 1,
                        ']' => {
                            if bracket_depth > 0 {
                                bracket_depth -= 1;
                            } else {
                                break;
                            }
                        }
                        _ => {}
                    }
                }
                '(' => {
                    paren_depth += 1;
                    end = i + 1;
                }
                ')' => {
                    if paren_depth > 0 {
                        paren_depth -= 1;
                        end = i + 1;
                    } else {
                        break;
                    }
                }
                // End URL at whitespace or other characters
                _ => break,
            }
        }

        // Strip trailing punctuation that's likely not part of the URL
        let url_text = &text[..end];
        let trimmed = url_text.trim_end_matches(['.', ',', ':', ';', '!', '?']);
        trimmed.len()
    }

    /// Open a URL using the system default handler
    pub fn open_url(url: &str) -> std::io::Result<()> {
        Self::open_url_in_directory(url, None)
    }

    /// Open a URL or path relative to the focused terminal's trusted local
    /// working directory. The target remains one argument and is never parsed
    /// by a command shell.
    pub fn open_url_in_directory(
        url: &str,
        working_directory: Option<&Path>,
    ) -> std::io::Result<()> {
        // Handle file:// URLs specially - extract the path portion
        let effective_url = if url.starts_with("file://") {
            Self::normalize_file_url(url)
        } else {
            url.to_string()
        };

        #[cfg(target_os = "linux")]
        let mut command = std::process::Command::new("xdg-open");

        #[cfg(target_os = "macos")]
        let mut command = std::process::Command::new("open");

        #[cfg(target_os = "windows")]
        let mut command = std::process::Command::new("explorer.exe");

        command.arg(&effective_url);
        if let Some(working_directory) = working_directory
            && Self::is_explicit_relative_path(&effective_url)
        {
            command.current_dir(working_directory);
        }
        command.spawn()?;

        Ok(())
    }

    /// Normalize a file:// URL to extract the local path
    ///
    /// Handles URLs like:
    /// - file:///path/to/file -> /path/to/file
    /// - file://localhost/path/to/file -> /path/to/file
    /// - file://hostname/path/to/file -> /path/to/file (for local files)
    fn normalize_file_url(url: &str) -> String {
        let without_scheme = url.strip_prefix("file://").unwrap_or(url);

        // Check if it starts with a path (three slashes total: file:///path)
        if without_scheme.starts_with('/') {
            // Already a proper local path: file:///path/to/file
            return without_scheme.to_string();
        }

        // Has a hostname: file://hostname/path/to/file
        // Find the first slash after the hostname
        if let Some(slash_pos) = without_scheme.find('/') {
            let hostname = &without_scheme[..slash_pos];
            let path = &without_scheme[slash_pos..];

            log::debug!(
                "Normalized file URL: hostname='{}', path='{}'",
                hostname,
                path
            );
            path.to_string()
        } else {
            // No path portion - malformed URL, return as-is
            format!("file://{}", without_scheme)
        }
    }

    fn is_explicit_relative_path(target: &str) -> bool {
        ["./", "../", ".\\", "..\\"]
            .iter()
            .any(|prefix| target.starts_with(prefix))
    }
}

/// URL cache for efficient lookup during rendering
#[derive(Clone)]
pub struct UrlCache {
    /// Detected URLs indexed by row
    urls_by_row: Vec<Vec<Arc<DetectedUrl>>>,
    /// Number of rows in the cache
    rows: usize,
    /// Content fingerprint used to skip unchanged rows.
    row_fingerprints: Vec<Option<u64>>,
    detection_scans: u64,
    cache_hits: u64,
}

#[derive(Hash)]
struct UrlCellSpan {
    byte_start: usize,
    byte_end: usize,
    row: usize,
    start_col: usize,
    end_col: usize,
}

fn sort_disjoint_urls(urls: &mut Vec<DetectedUrl>) {
    urls.sort_unstable_by(|left, right| {
        left.start_col
            .cmp(&right.start_col)
            .then_with(|| right.end_col.cmp(&left.end_col))
    });
    let mut last_end = None;
    urls.retain(|url| match last_end {
        Some(end) if url.start_col <= end => false,
        _ => {
            last_end = Some(url.end_col);
            true
        }
    });
}

impl UrlCache {
    /// Create a new URL cache
    pub fn new(rows: usize) -> Self {
        Self {
            urls_by_row: vec![Vec::new(); rows],
            rows,
            row_fingerprints: vec![None; rows],
            detection_scans: 0,
            cache_hits: 0,
        }
    }

    /// Resize the cache
    pub fn resize(&mut self, rows: usize) {
        self.urls_by_row.resize(rows, Vec::new());
        self.row_fingerprints.resize(rows, None);
        self.rows = rows;
    }

    /// Scan only visible logical lines, sharing wrapped URL payloads between
    /// their row indices. Hard breaks and viewport edges bound each scan.
    pub fn update_grid(&mut self, grid: &termeric_core::Grid, detector: &UrlDetector) {
        self.resize(grid.rows());
        let offset = i32::try_from(grid.scroll_offset()).unwrap_or(i32::MAX);
        let mut text = String::new();
        let mut spans = Vec::new();
        let mut first = 0;
        for visual_row in 0..grid.rows() {
            let row = grid.row_at_offset((visual_row as i32).saturating_sub(offset));
            if let Some(row) = &row {
                for col in row.reflow_columns() {
                    let Some(cell) = row.get(col) else { continue };
                    if cell.multicell.is_present() && !cell.multicell.is_leader() {
                        continue;
                    }
                    let byte_start = text.len();
                    match &cell.content {
                        termeric_core::CellContent::Empty => text.push(' '),
                        termeric_core::CellContent::Char(character) => text.push(*character),
                        termeric_core::CellContent::Grapheme(grapheme) => text.push_str(grapheme),
                    }
                    spans.push(UrlCellSpan {
                        byte_start,
                        byte_end: text.len(),
                        row: visual_row,
                        start_col: col,
                        end_col: col
                            .saturating_add(cell.width().saturating_sub(1))
                            .min(grid.cols().saturating_sub(1)),
                    });
                }
            }
            if row.is_none_or(|row| !row.is_wrapped()) || visual_row + 1 == grid.rows() {
                self.update_logical_line(first, visual_row, &text, &spans, detector);
                first = visual_row + 1;
                text.clear();
                spans.clear();
            }
        }
    }

    fn update_logical_line(
        &mut self,
        first: usize,
        last: usize,
        text: &str,
        spans: &[UrlCellSpan],
        detector: &UrlDetector,
    ) {
        use std::hash::{DefaultHasher, Hash, Hasher};

        let mut hasher = DefaultHasher::new();
        first.hash(&mut hasher);
        last.hash(&mut hasher);
        text.hash(&mut hasher);
        spans.hash(&mut hasher);
        let fingerprint = hasher.finish();
        if self.row_fingerprints[first..=last]
            .iter()
            .all(|old| *old == Some(fingerprint))
        {
            self.cache_hits += 1;
            return;
        }
        for row in first..=last {
            self.urls_by_row[row].clear();
            self.row_fingerprints[row] = Some(fingerprint);
        }
        let mut urls = detector.detect_in_line(text, first);
        // Disambiguate in byte coordinates before mapping to multiple rows.
        sort_disjoint_urls(&mut urls);
        for mut url in urls {
            let start = spans.get(spans.partition_point(|span| span.byte_end <= url.start_col));
            let end = spans.get(spans.partition_point(|span| span.byte_end <= url.end_col));
            let (Some(start), Some(end)) = (start, end) else {
                continue;
            };
            url.start_row = start.row;
            url.start_col = start.start_col;
            url.end_row = end.row;
            url.end_col = end.end_col;
            let url = Arc::new(url);
            for row in url.start_row..=url.end_row {
                self.urls_by_row[row].push(Arc::clone(&url));
            }
        }
        self.detection_scans += 1;
    }

    /// Detect URLs only when this visual row's text changed.
    #[cfg(test)]
    pub fn update_row(&mut self, row: usize, text: &str, detector: &UrlDetector) -> bool {
        self.update_row_inner(row, text, None, detector)
    }

    /// Detect URLs using byte boundaries mapped back to terminal columns.
    #[cfg(test)]
    pub fn update_row_mapped(
        &mut self,
        row: usize,
        text: &str,
        byte_to_col: &[usize],
        detector: &UrlDetector,
    ) -> bool {
        self.update_row_inner(row, text, Some(byte_to_col), detector)
    }

    #[cfg(test)]
    fn update_row_inner(
        &mut self,
        row: usize,
        text: &str,
        byte_to_col: Option<&[usize]>,
        detector: &UrlDetector,
    ) -> bool {
        if row >= self.rows {
            return false;
        }
        let column = |byte: usize| byte_to_col.map_or(byte, |map| map[byte.min(map.len() - 1)]);
        let spans: Vec<_> = text
            .char_indices()
            .map(|(byte_start, character)| {
                let byte_end = byte_start + character.len_utf8();
                UrlCellSpan {
                    byte_start,
                    byte_end,
                    row,
                    start_col: column(byte_start),
                    end_col: column(byte_end).saturating_sub(1),
                }
            })
            .collect();
        let scans = self.detection_scans;
        self.update_logical_line(row, row, text, &spans, detector);
        self.detection_scans != scans
    }

    pub fn metrics(&self) -> (u64, u64) {
        (self.detection_scans, self.cache_hits)
    }

    /// Get URL at a specific position
    pub fn url_at(&self, row: usize, col: usize) -> Option<&DetectedUrl> {
        let urls = self.urls_by_row.get(row)?;
        let candidate = urls
            .partition_point(|url| url.start_row < row || url.start_col <= col)
            .checked_sub(1)?;
        urls.get(candidate)
            .map(Arc::as_ref)
            .filter(|url| url.contains(row, col))
    }

    /// Check if a position is within any URL
    pub fn is_url(&self, row: usize, col: usize) -> bool {
        self.url_at(row, col).is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn logical_url_cache_shares_payloads_and_invalidates_the_whole_line() {
        let detector = UrlDetector::new();
        let mut terminal = termeric_core::Terminal::new(8, 5);
        terminal.process(b"xx https://example.test/x yy");
        let mut cache = UrlCache::new(0);
        cache.update_grid(terminal.grid(), &detector);
        assert!(Arc::ptr_eq(
            &cache.urls_by_row[0][0],
            &cache.urls_by_row[3][0]
        ));
        let (scans, hits) = cache.metrics();
        cache.update_grid(terminal.grid(), &detector);
        assert_eq!(cache.metrics(), (scans, hits + 2));

        terminal.process(b"\x1b[4;1Hy");
        cache.update_grid(terminal.grid(), &detector);
        assert_eq!(cache.metrics().0, scans + 1);
        assert_eq!(cache.url_at(0, 3).unwrap().url, "https://example.test/y");
        assert_eq!(cache.url_at(3, 0).unwrap().url, "https://example.test/y");

        // Identical bytes with a new hard boundary must not reuse the group.
        terminal.grid_mut().row_mut(0).unwrap().set_wrapped(false);
        cache.update_grid(terminal.grid(), &detector);
        assert!(cache.url_at(0, 3).is_none());
        assert!(cache.url_at(3, 0).is_none());
    }

    #[test]
    fn logical_url_cache_maps_wide_prefix_padding_and_resize() {
        let detector = UrlDetector::new();
        let mut terminal = termeric_core::Terminal::new(2, 12);
        terminal.process("a界 https://e.test/x".as_bytes());
        let mut cache = UrlCache::new(0);
        cache.update_grid(terminal.grid(), &detector);
        assert!(cache.url_at(0, 1).is_none()); // Synthetic wrap padding.
        assert!(cache.url_at(1, 1).is_none()); // Wide spacer.
        assert!(cache.url_at(2, 0).is_none()); // Real space.
        assert_eq!(cache.url_at(2, 1).unwrap().url, "https://e.test/x");
        terminal.resize(8, 12);
        cache.update_grid(terminal.grid(), &detector);
        assert!(cache.url_at(0, 3).is_none());
        assert_eq!(cache.url_at(0, 4).unwrap().url, "https://e.test/x");
        assert_eq!(cache.url_at(2, 3).unwrap().url, "https://e.test/x");
        assert!(cache.url_at(2, 4).is_none());
        assert!(cache.url_at(10, 0).is_none());
    }

    #[test]
    fn logical_url_cache_reads_visible_history_and_keeps_outer_intervals() {
        let detector = UrlDetector::new();
        let mut terminal = termeric_core::Terminal::new(12, 6);
        let text = "https://a.test/?next=http://b.test ftp://c.test";
        terminal.process(text.as_bytes());
        terminal.process(b"\r\nx\r\ny\r\nz\r\nw\r\nq\r\nr");
        let mut cache = UrlCache::new(0);
        cache.update_grid(terminal.grid(), &detector);
        assert!(cache.url_at(0, 0).is_none());
        let history = terminal.grid().scrollback_len();
        terminal.grid_mut().set_scroll_offset(history);
        cache.update_grid(terminal.grid(), &detector);
        let outer = "https://a.test/?next=http://b.test";
        for index in 0..outer.len() {
            assert_eq!(cache.url_at(index / 12, index % 12).unwrap().url, outer);
        }
        let gap = outer.len();
        assert!(cache.url_at(gap / 12, gap % 12).is_none());
        for index in gap + 1..text.len() {
            assert_eq!(
                cache.url_at(index / 12, index % 12).unwrap().url,
                "ftp://c.test"
            );
        }
        terminal.grid_mut().set_scroll_offset(0);
        cache.update_grid(terminal.grid(), &detector);
        assert!(cache.url_at(0, 0).is_none());
    }

    #[test]
    fn test_detect_simple_url() {
        let detector = UrlDetector::new();
        let urls = detector.detect_in_line("Check out https://example.com for more", 0);

        assert_eq!(urls.len(), 1);
        assert_eq!(urls[0].url, "https://example.com");
        assert_eq!(urls[0].start_col, 10);
    }

    #[test]
    fn test_detect_url_with_path() {
        let detector = UrlDetector::new();
        let urls = detector.detect_in_line("Visit https://example.com/path/to/page.html", 0);

        assert_eq!(urls.len(), 1);
        assert_eq!(urls[0].url, "https://example.com/path/to/page.html");
    }

    #[test]
    fn test_detect_url_with_query() {
        let detector = UrlDetector::new();
        let urls = detector.detect_in_line("Link: https://example.com/search?q=test&page=1", 0);

        assert_eq!(urls.len(), 1);
        assert_eq!(urls[0].url, "https://example.com/search?q=test&page=1");
    }

    #[test]
    fn test_detect_multiple_urls() {
        let detector = UrlDetector::new();
        let urls = detector.detect_in_line("See http://a.com and https://b.com", 0);

        assert_eq!(urls.len(), 2);
        assert_eq!(urls[0].url, "http://a.com");
        assert_eq!(urls[1].url, "https://b.com");
    }

    #[test]
    fn test_detect_url_with_parens() {
        let detector = UrlDetector::new();
        let urls = detector.detect_in_line(
            "Wikipedia: https://en.wikipedia.org/wiki/Rust_(programming_language)",
            0,
        );

        assert_eq!(urls.len(), 1);
        assert_eq!(
            urls[0].url,
            "https://en.wikipedia.org/wiki/Rust_(programming_language)"
        );
    }

    #[test]
    fn test_strip_trailing_punctuation() {
        let detector = UrlDetector::new();
        let urls = detector.detect_in_line("Check https://example.com.", 0);

        assert_eq!(urls.len(), 1);
        assert_eq!(urls[0].url, "https://example.com");
    }

    #[test]
    fn cache_scans_only_changed_rows_and_replaces_old_urls() {
        let detector = UrlDetector::new();
        let mut cache = UrlCache::new(1);
        assert!(cache.update_row(0, "https://example.com", &detector));
        assert!(!cache.update_row(0, "https://example.com", &detector));
        assert_eq!(cache.metrics(), (1, 1));
        assert!(cache.is_url(0, 0));

        assert!(cache.update_row(0, "plain text", &detector));
        assert!(!cache.is_url(0, 0));
        assert_eq!(cache.metrics(), (2, 1));
    }

    #[test]
    fn mapped_cache_uses_terminal_columns_after_wide_graphemes() {
        let detector = UrlDetector::new();
        let mut cache = UrlCache::new(1);
        let text = "界 https://example.com";
        let mut map = vec![0; text.len() + 1];
        // 界 occupies bytes 0..3 but columns 0..2; the following space is col 2.
        map[0..3].fill(0);
        map[3] = 2;
        map[4] = 3;
        for (byte, value) in map.iter_mut().enumerate().skip(4) {
            *value = byte - 1;
        }
        assert!(cache.update_row_mapped(0, text, &map, &detector));
        let url = cache.url_at(0, 3).unwrap();
        assert_eq!(url.start_col, 3);
        assert_eq!(url.url, "https://example.com");
        assert!(cache.url_at(0, 2).is_none());
    }

    #[test]
    fn mapped_cache_invalidates_when_only_terminal_columns_change() {
        let detector = UrlDetector::new();
        let mut cache = UrlCache::new(1);
        let text = "x https://example.com";
        let identity_map: Vec<_> = (0..=text.len()).collect();
        assert!(cache.update_row_mapped(0, text, &identity_map, &detector));
        assert_eq!(cache.url_at(0, 2).unwrap().start_col, 2);

        let shifted_map: Vec<_> = (0..=text.len()).map(|byte| byte + 3).collect();
        assert!(cache.update_row_mapped(0, text, &shifted_map, &detector));
        assert!(cache.url_at(0, 2).is_none());
        assert_eq!(cache.url_at(0, 5).unwrap().start_col, 5);
        assert_eq!(cache.metrics(), (2, 0));
    }

    #[test]
    fn cached_urls_are_position_sorted_for_logarithmic_cell_lookup() {
        let detector = UrlDetector::new();
        let mut cache = UrlCache::new(1);
        let text = "https://first.example then http://second.example then ftp://third.example";
        assert!(cache.update_row(0, text, &detector));

        let urls = &cache.urls_by_row[0];
        assert_eq!(urls.len(), 3);
        assert!(
            urls.windows(2)
                .all(|pair| pair[0].start_col < pair[1].start_col)
        );
        for url in urls {
            assert_eq!(cache.url_at(0, url.start_col).unwrap().url, url.url);
            assert_eq!(cache.url_at(0, url.end_col).unwrap().url, url.url);
        }
        assert!(cache.url_at(0, text.len()).is_none());
    }

    #[test]
    fn cached_lookup_prefers_an_outer_url_over_a_nested_scheme() {
        let detector = UrlDetector::new();
        let mut cache = UrlCache::new(1);
        let text = "https://outer.example/?next=http://inner.example";
        assert!(cache.update_row(0, text, &detector));

        assert_eq!(cache.urls_by_row[0].len(), 1);
        assert_eq!(cache.url_at(0, 0).unwrap().url, text);
        assert_eq!(cache.url_at(0, text.len() - 1).unwrap().url, text);
    }

    #[test]
    fn test_normalize_file_url_with_hostname() {
        let result = UrlDetector::normalize_file_url("file://Emrakul/home/zilawon/Downloads");
        assert_eq!(result, "/home/zilawon/Downloads");
    }

    #[test]
    fn test_normalize_file_url_with_localhost() {
        let result = UrlDetector::normalize_file_url("file://localhost/home/user/file.txt");
        assert_eq!(result, "/home/user/file.txt");
    }

    #[test]
    fn test_normalize_file_url_triple_slash() {
        let result = UrlDetector::normalize_file_url("file:///home/user/file.txt");
        assert_eq!(result, "/home/user/file.txt");
    }

    #[test]
    fn only_explicit_relative_paths_inherit_the_terminal_directory() {
        for target in ["./file", "../file", ".\\file", "..\\file"] {
            assert!(UrlDetector::is_explicit_relative_path(target));
        }
        for target in [
            "/tmp/file",
            "https://example.test",
            "file:///tmp/file",
            "PROJ-17",
        ] {
            assert!(!UrlDetector::is_explicit_relative_path(target));
        }
    }
}
