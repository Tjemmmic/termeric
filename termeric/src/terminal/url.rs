//! URL detection and handling
//!
//! This module provides functionality for detecting URLs in terminal output
//! and handling click-to-open functionality.

use std::path::Path;
use std::sync::Arc;

#[path = "url/opener.rs"]
mod opener;

use super::row_text::push_row_text;

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

    /// Detect URLs in a line of text. Columns here are UTF-8 byte offsets;
    /// the grid cache maps them to display cells (including wide graphemes).
    pub fn detect_in_line(&self, text: &str, row: usize) -> Vec<DetectedUrl> {
        let mut urls = Vec::new();

        for scheme in &self.schemes {
            let mut search_start = 0;
            let first = scheme.as_bytes()[0] as char;

            // URI schemes are ASCII case-insensitive (RFC 3986 §3.1).
            // Search original bytes: lowercasing the line would allocate and
            // could change Unicode lengths or case-sensitive paths/queries.
            while let Some((start, _)) = text[search_start..]
                .match_indices([first, first.to_ascii_uppercase()])
                .find(|(start, _)| {
                    text.as_bytes()[search_start + start..]
                        .get(..scheme.len())
                        .is_some_and(|prefix| prefix.eq_ignore_ascii_case(scheme.as_bytes()))
                })
            {
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
                'a'..='z'
                | 'A'..='Z'
                | '0'..='9'
                | '-'
                | '.'
                | '_'
                | '~'
                | ':'
                | '/'
                | '?'
                | '#'
                | '@'
                | '!'
                | '$'
                | '&'
                | '\''
                | '*'
                | '+'
                | ','
                | ';'
                | '='
                | '%' => {}
                '(' => paren_depth += 1,
                ')' if paren_depth > 0 => paren_depth -= 1,
                '[' => bracket_depth += 1,
                ']' if bracket_depth > 0 => bracket_depth -= 1,
                _ if Self::is_unicode_url_char(c) => {}
                // End URL at whitespace or other characters
                _ => break,
            }
            // Advance only after accepting a whole scalar. In particular an
            // unmatched closing bracket must not become part of the target.
            end = i + c.len_utf8();
        }

        // Strip trailing punctuation that's likely not part of the URL
        let url_text = &text[..end];
        let trimmed = url_text.trim_end_matches(['.', ',', ':', ';', '!', '?']);
        trimmed.len()
    }

    fn is_unicode_url_char(character: char) -> bool {
        // RFC 3987 section 2.2 ucschar, with conservative prose boundaries.
        // This is link detection, not IRI validation/normalization. Whitespace
        // and directional/word-separation formatting end an inferred target;
        // ZWNJ/ZWJ remain valid for joined text and emoji sequences.
        if character.is_whitespace()
            || matches!(character,
                '\u{ad}' | '\u{61c}' | '\u{200b}' | '\u{200e}' | '\u{200f}'
                | '\u{202a}'..='\u{202e}' | '\u{2060}' | '\u{2066}'..='\u{2069}'
                | '\u{feff}')
        {
            return false;
        }
        match character as u32 {
            0xA0..=0xD7FF | 0xF900..=0xFDCF | 0xFDF0..=0xFFEF | 0xE1000..=0xEFFFD => true,
            0x10000..=0xDFFFD => (character as u32 & 0xFFFF) <= 0xFFFD,
            _ => false,
        }
    }

    /// Open a URL or path relative to the focused terminal's trusted local
    /// working directory. The target remains one argument and is never parsed
    /// by a command shell. Relative filesystem targets require an absolute
    /// trusted directory; they never inherit the application's directory.
    pub fn open_url_in_directory(
        url: &str,
        working_directory: Option<&Path>,
    ) -> std::io::Result<()> {
        opener::launch(Self::opener_command(url, working_directory)?)
    }

    /// Leave URI interpretation to the desktop handler. In particular, a file
    /// authority must never be discarded or its encoded path treated as a
    /// literal local filename. Building separately permits side-effect-free
    /// argument and working-directory tests of the actual launch path.
    fn opener_command(
        url: &str,
        working_directory: Option<&Path>,
    ) -> std::io::Result<std::process::Command> {
        if url.is_empty() || url.contains('\0') {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "opener target must be nonempty and contain no NUL",
            ));
        }
        // A single argument is not an option boundary. xdg-open has no
        // portable end-of-options contract; do not let terminal-controlled
        // text select a helper flag instead of a resource.
        if url.starts_with('-') {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "opener target must not start with '-'; use an explicit path such as './-name'",
            ));
        }
        let directory = if Self::is_relative_path(url) {
            let directory = working_directory.ok_or_else(|| std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "relative path requires a trusted local terminal directory; enable local OSC 7 reporting or use an absolute path/file URI",
            ))?;
            if !directory.is_absolute() {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    "trusted terminal directory must be absolute",
                ));
            }
            Some(directory)
        } else {
            None
        };
        let mut command = std::process::Command::new("xdg-open");
        command.arg(url);
        if let Some(directory) = directory {
            command.current_dir(directory);
        }
        Ok(command)
    }

    fn is_relative_path(target: &str) -> bool {
        if Path::new(target).is_absolute() {
            return false;
        }
        // A scheme is ASCII alpha followed by ASCII alphanumeric/+/./-.
        // URI interpretation remains the desktop handler's responsibility.
        !target.split_once(':').is_some_and(|(scheme, _)| {
            let mut bytes = scheme.bytes();
            bytes.next().is_some_and(|byte| byte.is_ascii_alphabetic())
                && bytes
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'+' | b'.' | b'-'))
        })
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
    linear: bool,
}

impl UrlCellSpan {
    fn start_column(&self, byte: usize) -> usize {
        if self.linear {
            self.start_col + byte - self.byte_start
        } else {
            self.start_col
        }
    }

    fn end_column(&self, byte: usize) -> usize {
        if self.linear {
            self.start_col + byte - self.byte_start
        } else {
            self.end_col
        }
    }
}

fn append_url_row(
    row: &termeric_core::RowView<'_>,
    visual_row: usize,
    text: &mut String,
    spans: &mut Vec<UrlCellSpan>,
) {
    let last_col = row.len().saturating_sub(1);
    push_row_text(row, text, |col, width, bytes| {
        let end_col = col.saturating_add(width.saturating_sub(1)).min(last_col);
        // Exactly one UTF-8 byte and one display cell is an affine mapping.
        // Never merge across rows, skipped cells, or variable-width graphemes.
        let linear = bytes.len() == 1 && width == 1;
        if linear
            && let Some(previous) = spans.last_mut()
            && previous.linear
            && previous.row == visual_row
            && previous.byte_end == bytes.start
            && previous.end_col.checked_add(1) == Some(col)
        {
            previous.byte_end = bytes.end;
            previous.end_col = end_col;
        } else {
            spans.push(UrlCellSpan {
                byte_start: bytes.start,
                byte_end: bytes.end,
                row: visual_row,
                start_col: col,
                end_col,
                linear,
            });
        }
    });
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
        self.update_projected_grid(grid, &[], detector);
    }

    /// Index displayed source rows by visual row. An empty projection uses the
    /// ordinary viewport, matching search's pending/empty-filter fallback.
    /// Only adjacent source rows may continue a soft-wrapped URL.
    pub fn update_projected_grid(
        &mut self,
        grid: &termeric_core::Grid,
        projected_rows: &[i32],
        detector: &UrlDetector,
    ) {
        let rows = if projected_rows.is_empty() {
            grid.rows()
        } else {
            projected_rows.len().min(grid.rows())
        };
        self.resize(rows);
        let offset = i32::try_from(grid.scroll_offset()).unwrap_or(i32::MAX);
        let source_row = |visual_row: usize| {
            projected_rows
                .get(visual_row)
                .copied()
                .unwrap_or_else(|| (visual_row as i32).saturating_sub(offset))
        };
        let mut text = String::new();
        let mut spans = Vec::new();
        let mut first = 0;
        for visual_row in 0..rows {
            let source = source_row(visual_row);
            let row = grid.row_at_offset(source);
            if let Some(row) = &row {
                append_url_row(row, visual_row, &mut text, &mut spans);
            }
            let adjacent =
                visual_row + 1 < rows && source.checked_add(1) == Some(source_row(visual_row + 1));
            if row.is_none_or(|row| !row.is_wrapped()) || !adjacent {
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
            url.start_col = start.start_column(url.start_col);
            url.end_row = end.row;
            url.end_col = end.end_column(url.end_col);
            let url = Arc::new(url);
            for row in url.start_row..=url.end_row {
                self.urls_by_row[row].push(Arc::clone(&url));
            }
        }
        self.detection_scans += 1;
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
    fn unicode_detected_target_preserves_utf8_and_byte_boundaries() {
        let detector = UrlDetector::new();
        for target in [
            "https://例え.test/路径/e\u{301}/👩\u{200d}💻?名称=值#片段",
            "mailto:élève@例.test",
            "file:///tmp/資料/é.txt",
            "https://[::1]/界",
        ] {
            let prefix = "前言 [";
            let text = format!("{prefix}{target}] next");
            let urls = detector.detect_in_line(&text, 7);
            assert_eq!(urls.len(), 1);
            assert_eq!(urls[0].url, target);
            assert_eq!((urls[0].start_row, urls[0].end_row), (7, 7));
            assert_eq!(urls[0].start_col, prefix.len());
            assert_eq!(urls[0].end_col + 1, prefix.len() + target.len());
            assert_eq!(&text[urls[0].start_col..=urls[0].end_col], target);
        }
    }

    #[test]
    fn unmatched_url_brackets_are_not_part_of_the_target() {
        let detector = UrlDetector::new();
        for (text, expected) in [
            ("[https://example.test]", "https://example.test"),
            ("[https://[::1]/path]", "https://[::1]/path"),
            ("https://e.test/a[b[c]]d]tail", "https://e.test/a[b[c]]d"),
            ("(https://e.test/a(b(c)))", "https://e.test/a(b(c))"),
        ] {
            let urls = detector.detect_in_line(text, 0);
            assert_eq!(urls.len(), 1);
            assert_eq!(urls[0].url, expected);
        }
    }

    #[test]
    fn unicode_url_boundaries_exclude_separators_controls_and_noncharacters() {
        let detector = UrlDetector::new();
        for separator in [
            ' ',
            '\t',
            '\n',
            '\u{7f}',
            '\u{85}',
            '\u{a0}',
            '\u{ad}',
            '\u{61c}',
            '\u{2000}',
            '\u{200b}',
            '\u{200e}',
            '\u{200f}',
            '\u{2028}',
            '\u{2029}',
            '\u{202a}',
            '\u{202e}',
            '\u{202f}',
            '\u{2060}',
            '\u{2066}',
            '\u{2069}',
            '\u{3000}',
            '\u{e000}',
            '\u{f8ff}',
            '\u{fdd0}',
            '\u{fdef}',
            '\u{feff}',
            '\u{fff0}',
            '\u{ffff}',
            '\u{1fffe}',
            '\u{affff}',
            '\u{e0001}',
            '\u{effff}',
            '\u{f0000}',
            '\u{10ffff}',
        ] {
            let text = format!("https://例.test/界{separator}tail https://next.test");
            let urls = detector.detect_in_line(&text, 0);
            assert_eq!(urls.len(), 2, "separator={separator:?}");
            assert_eq!(urls[0].url, "https://例.test/界", "separator={separator:?}");
            assert_eq!(urls[1].url, "https://next.test");
        }
        for character in [
            '\u{a1}',
            '\u{301}',
            '\u{200c}',
            '\u{200d}',
            '\u{d7ff}',
            '\u{f900}',
            '\u{fdcf}',
            '\u{fdf0}',
            '\u{ffef}',
            '\u{10000}',
            '\u{1fffd}',
            '\u{20000}',
            '\u{dfffd}',
            '\u{e1000}',
            '\u{efffd}',
        ] {
            let target = format!("https://例.test/{character}/end");
            assert_eq!(detector.detect_in_line(&target, 0)[0].url, target);
        }
    }

    #[test]
    fn unicode_url_cache_maps_wrapped_graphemes_history_and_reflow() {
        let target = "https://例.test/e\u{301}/👩\u{200d}💻/界";
        let detector = UrlDetector::new();
        let mut terminal = termeric_core::Terminal::new(7, 32);
        terminal.process(format!("前 [{target}] ").as_bytes());
        let mut cache = UrlCache::new(0);
        for cols in [7, 4, 16, 80] {
            terminal.resize(cols, 32);
            let history = terminal.grid().scrollback_len();
            terminal.grid_mut().set_scroll_offset(history);
            cache.update_grid(terminal.grid(), &detector);
            let mut checked = [false; 5];
            for visual_row in 0..32 {
                let row = terminal
                    .grid()
                    .row_at_offset(visual_row as i32 - terminal.grid().scroll_offset() as i32)
                    .unwrap();
                for col in row.reflow_columns() {
                    let cell = row.get(col).unwrap();
                    let index = match &cell.content {
                        termeric_core::CellContent::Char('例') => Some(0),
                        termeric_core::CellContent::Grapheme(value)
                            if value.as_ref() == "e\u{301}" =>
                        {
                            Some(1)
                        }
                        termeric_core::CellContent::Grapheme(value)
                            if value.as_ref() == "👩\u{200d}💻" =>
                        {
                            Some(2)
                        }
                        termeric_core::CellContent::Char('界') => Some(3),
                        termeric_core::CellContent::Char(']') => {
                            assert!(cache.url_at(visual_row, col).is_none());
                            checked[4] = true;
                            None
                        }
                        _ => None,
                    };
                    if let Some(index) = index {
                        checked[index] = true;
                        for column in col..col + cell.width() {
                            assert_eq!(cache.url_at(visual_row, column).unwrap().url, target);
                        }
                    }
                }
            }
            assert_eq!(checked, [true; 5], "cols={cols}");
            let (scans, hits) = cache.metrics();
            cache.update_grid(terminal.grid(), &detector);
            assert_eq!(cache.metrics().0, scans);
            assert!(cache.metrics().1 > hits);
            // Move this same content into compact history before next resize.
            terminal.process(&b"\r\n".repeat(35));
        }
    }

    #[test]
    fn ascii_url_mapping_retains_one_span_per_physical_row() {
        let mut terminal = termeric_core::Terminal::new(100, 30);
        terminal.process("a".repeat(3_000).as_bytes());
        let mut text = String::new();
        let mut spans = Vec::new();
        for row in 0..30 {
            append_url_row(
                &terminal.grid().row_at_offset(row).unwrap(),
                row as usize,
                &mut text,
                &mut spans,
            );
        }
        assert_eq!(text.len(), 3_000);
        assert_eq!(spans.len(), 30);
        for (row, span) in spans.iter().enumerate() {
            assert!(span.linear);
            assert_eq!(span.row, row);
            assert_eq!(span.start_column(row * 100 + 17), 17);
            assert_eq!(span.end_column(row * 100 + 83), 83);
        }
    }

    #[test]
    fn coalesced_url_mapping_matches_dense_cell_mapping_for_every_byte() {
        for cols in [1, 3, 9, 32] {
            let mut terminal = termeric_core::Terminal::new(cols, 8);
            terminal.process("a界 e\u{301} 👩\u{200d}💻 https://e.test/x\r\n".as_bytes());
            terminal.process(b"\x1b]66;s=2:w=2;title\x07! https://b.test/y\r\nend");
            for offset in [0, terminal.grid().scrollback_len()] {
                terminal.grid_mut().set_scroll_offset(offset);
                let mut text = String::new();
                let mut spans = Vec::new();
                let mut reference = String::new();
                let mut expected = Vec::new();
                for visual_row in 0..8 {
                    let row = terminal
                        .grid()
                        .row_at_offset(visual_row as i32 - offset as i32)
                        .unwrap();
                    append_url_row(&row, visual_row, &mut text, &mut spans);
                    // Independent pre-coalescing cell traversal, including
                    // skipped physical columns and non-linear glyph spans.
                    for col in 0..row.len() {
                        let cell = row.get(col).unwrap();
                        if cell.is_wide_char_spacer()
                            || (col + 1 == row.len() && row.has_wide_wrap_padding())
                            || (cell.multicell.is_present() && !cell.multicell.is_leader())
                        {
                            continue;
                        }
                        let start = reference.len();
                        match &cell.content {
                            termeric_core::CellContent::Empty => reference.push(' '),
                            termeric_core::CellContent::Char(ch) => reference.push(*ch),
                            termeric_core::CellContent::Grapheme(grapheme) => {
                                reference.push_str(grapheme)
                            }
                        }
                        let end_col =
                            (col + cell.width().saturating_sub(1)).min(row.len().saturating_sub(1));
                        expected.extend(
                            (start..reference.len()).map(|byte| (byte, visual_row, col, end_col)),
                        );
                    }
                }
                assert_eq!(text, reference, "cols={cols} offset={offset}");
                for (byte, row, start_col, end_col) in expected {
                    let span = &spans[spans.partition_point(|span| span.byte_end <= byte)];
                    assert_eq!(
                        (span.row, span.start_column(byte), span.end_column(byte)),
                        (row, start_col, end_col),
                        "cols={cols} offset={offset} byte={byte}"
                    );
                }
            }
        }
    }

    #[test]
    fn projected_url_cache_maps_history_and_unicode_to_visual_rows() {
        let detector = UrlDetector::new();
        let mut terminal = termeric_core::Terminal::new(40, 6);
        terminal.process(
            "界e\u{301} https://first.test\r\nhttps://hidden.test\r\nhttps://last.test".as_bytes(),
        );
        terminal.process(b"\r\n".repeat(10).as_slice());
        let first = -(terminal.grid().scrollback_len() as i32);
        assert!(first < 0);
        let projection = [first, first + 2];
        let mut cache = UrlCache::new(0);
        cache.update_projected_grid(terminal.grid(), &projection, &detector);
        assert!(cache.url_at(0, 3).is_none());
        let first_url = cache.url_at(0, 4).unwrap();
        assert_eq!(first_url.url, "https://first.test");
        assert_eq!((first_url.start_row, first_url.start_col), (0, 4));
        assert_eq!(cache.url_at(1, 0).unwrap().url, "https://last.test");
        assert!(cache.url_at(2, 0).is_none());
        let (scans, hits) = cache.metrics();
        cache.update_projected_grid(terminal.grid(), &projection, &detector);
        assert_eq!(cache.metrics(), (scans, hits + 2));

        cache.update_projected_grid(terminal.grid(), &[first + 2], &detector);
        assert_eq!(cache.url_at(0, 0).unwrap().url, "https://last.test");
        assert!(cache.url_at(1, 0).is_none());
        // Empty/pending filters return to the ordinary viewport, not old hits.
        cache.update_projected_grid(terminal.grid(), &[], &detector);
        assert!((0..6).all(|row| cache.url_at(row, 0).is_none()));
        terminal.grid_mut().set_scroll_offset((-first) as usize);
        cache.update_grid(terminal.grid(), &detector);
        assert_eq!(cache.url_at(1, 0).unwrap().url, "https://hidden.test");
    }

    #[test]
    fn projected_url_cache_joins_only_adjacent_source_wraps() {
        let detector = UrlDetector::new();
        let mut terminal = termeric_core::Terminal::new(8, 5);
        let target = "https://a.test/omitted/tail";
        terminal.process(target.as_bytes());
        let mut cache = UrlCache::new(0);
        cache.update_projected_grid(terminal.grid(), &[0, 1, 2, 3], &detector);
        assert_eq!(cache.url_at(0, 0).unwrap().url, target);
        assert!(Arc::ptr_eq(
            &cache.urls_by_row[0][0],
            &cache.urls_by_row[3][0]
        ));
        // Omitting row one must not create the synthetic https://mitted/tail.
        cache.update_projected_grid(terminal.grid(), &[0, 2, 3], &detector);
        assert!((0..5).all(|row| cache.url_at(row, 0).is_none()));
        cache.update_projected_grid(terminal.grid(), &[0, 1, 2, 3], &detector);
        assert_eq!(cache.url_at(3, 0).unwrap().url, target);
        // Projection is still bounded to viewport height.
        cache.update_projected_grid(terminal.grid(), &[0, 1, 2, 3, 4, 0], &detector);
        assert!(cache.url_at(5, 0).is_none());
    }

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
    fn detected_scheme_casing_preserves_every_target_byte() {
        let detector = UrlDetector::new();
        for scheme in &detector.schemes {
            let letters = scheme.bytes().filter(u8::is_ascii_alphabetic).count();
            for mask in 0..(1 << letters) {
                let mut bit = 0;
                let variant: String = scheme
                    .chars()
                    .map(|ch| {
                        if !ch.is_ascii_alphabetic() {
                            return ch;
                        }
                        let upper = mask & (1 << bit) != 0;
                        bit += 1;
                        if upper { ch.to_ascii_uppercase() } else { ch }
                    })
                    .collect();
                let target = format!("{variant}Example.test/Case/界/%2f?q=VaLue#Frag");
                let text = format!("界 ({target}). after");
                let urls = detector.detect_in_line(&text, 7);
                assert_eq!(urls.len(), 1, "{text}");
                assert_eq!(urls[0].url, target);
                assert_eq!((urls[0].start_row, urls[0].end_row), (7, 7));
                assert_eq!(urls[0].start_col, "界 (".len());
                assert_eq!(urls[0].end_col, "界 (".len() + target.len() - 1);
            }
        }
    }

    #[test]
    fn detected_scheme_casing_is_ascii_only_and_requires_the_existing_separator() {
        let detector = UrlDetector::new();
        for text in [
            "HTTP:example.test",
            "HTTPS:/example.test",
            "HTTPＳ://example.test",
            "ＦTP://example.test",
            "MAİLTO:user@example.test",
            "unknown://example.test",
            "HTTP://",
            "MAILTO:",
            "H界HTTP:/",
            "界F",
        ] {
            assert!(detector.detect_in_line(text, 0).is_empty(), "{text}");
        }
    }

    #[test]
    fn mixed_case_wrapped_url_cache_preserves_outer_target_and_reuses_rows() {
        let detector = UrlDetector::new();
        let mut terminal = termeric_core::Terminal::new(20, 8);
        let target = "HtTpS://例.test/Case?next=HTTP://Inner.test/%2f";
        terminal.process(format!("界 {target} END").as_bytes());
        let mut cache = UrlCache::new(0);
        cache.update_grid(terminal.grid(), &detector);
        let url = cache.url_at(0, 3).unwrap();
        assert_eq!(url.url, target);
        assert!(url.end_row > 0);
        for row in url.start_row..=url.end_row {
            let start = if row == url.start_row {
                url.start_col
            } else {
                0
            };
            let end = if row == url.end_row { url.end_col } else { 19 };
            for col in start..=end {
                assert_eq!(cache.url_at(row, col).unwrap().url, target);
            }
        }
        assert!(cache.url_at(0, 2).is_none());
        assert!(cache.url_at(url.end_row, url.end_col + 1).is_none());
        let scans = cache.metrics().0;
        cache.update_grid(terminal.grid(), &detector);
        assert_eq!(cache.metrics().0, scans);
        terminal.resize(16, 8);
        cache.update_grid(terminal.grid(), &detector);
        assert_eq!(cache.url_at(0, 3).unwrap().url, target);
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

    /// A one-row terminal showing `text`, and a cache scanned from it.
    fn scanned_row(text: &str) -> (termeric_core::Terminal, UrlCache) {
        let mut terminal = termeric_core::Terminal::new(80, 1);
        terminal.process(text.as_bytes());
        let mut cache = UrlCache::new(1);
        cache.update_grid(terminal.grid(), &UrlDetector::new());
        (terminal, cache)
    }

    #[test]
    fn cache_scans_only_changed_rows_and_replaces_old_urls() {
        let detector = UrlDetector::new();
        let (mut terminal, mut cache) = scanned_row("https://example.com");
        cache.update_grid(terminal.grid(), &detector);
        assert_eq!(cache.metrics(), (1, 1));
        assert!(cache.is_url(0, 0));

        terminal.process(b"\r\x1b[2Kplain text");
        cache.update_grid(terminal.grid(), &detector);
        assert!(!cache.is_url(0, 0));
        assert_eq!(cache.metrics(), (2, 1));
    }

    #[test]
    fn cache_uses_terminal_columns_after_wide_graphemes() {
        // 界 takes columns 0 and 1, so the URL starts at column 3.
        let (_terminal, cache) = scanned_row("界 https://example.com");
        let url = cache.url_at(0, 3).unwrap();
        assert_eq!(url.start_col, 3);
        assert_eq!(url.url, "https://example.com");
        assert!(cache.url_at(0, 2).is_none());
    }

    #[test]
    fn cached_urls_are_position_sorted_for_logarithmic_cell_lookup() {
        let text = "https://first.example then http://second.example then ftp://third.example";
        let (_terminal, cache) = scanned_row(text);

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
        let text = "https://outer.example/?next=http://inner.example";
        let (_terminal, cache) = scanned_row(text);

        assert_eq!(cache.urls_by_row[0].len(), 1);
        assert_eq!(cache.url_at(0, 0).unwrap().url, text);
        assert_eq!(cache.url_at(0, text.len() - 1).unwrap().url, text);
    }

    #[test]
    fn relative_opener_targets_require_a_trusted_absolute_terminal_directory() {
        for target in [
            "./file",
            "../file",
            ".",
            "..",
            "report.txt",
            "src/main.rs",
            ".\\file",
            "..\\file",
            "界.txt",
        ] {
            let error = UrlDetector::opener_command(target, None).unwrap_err();
            assert_eq!(error.kind(), std::io::ErrorKind::PermissionDenied);
            // Public rejection must happen before launching a desktop helper.
            assert_eq!(
                UrlDetector::open_url_in_directory(target, None)
                    .unwrap_err()
                    .kind(),
                std::io::ErrorKind::PermissionDenied
            );
            assert!(
                error
                    .to_string()
                    .contains("trusted local terminal directory")
            );
            let error =
                UrlDetector::opener_command(target, Some(Path::new("relative/cwd"))).unwrap_err();
            assert_eq!(error.kind(), std::io::ErrorKind::InvalidInput);
        }
    }

    #[test]
    fn opener_rejects_empty_targets_and_keeps_absolute_targets_context_free() {
        for target in ["", "bad\0target"] {
            assert_eq!(
                UrlDetector::open_url_in_directory(target, None)
                    .unwrap_err()
                    .kind(),
                std::io::ErrorKind::InvalidInput
            );
        }
        let absolute = "/absolute/file.txt";
        let command = UrlDetector::opener_command(absolute, None).unwrap();
        assert!(command.get_current_dir().is_none());
        assert_eq!(
            command.get_args().collect::<Vec<_>>(),
            [std::ffi::OsStr::new(absolute)]
        );
        for target in [
            "mailto:user@example.test",
            "custom+app.v2:payload",
            "https://example.test",
            "file:///tmp/a%20b",
        ] {
            let command = UrlDetector::opener_command(target, None).unwrap();
            assert!(command.get_current_dir().is_none());
            assert_eq!(
                command.get_args().collect::<Vec<_>>(),
                [std::ffi::OsStr::new(target)]
            );
        }
    }

    #[test]
    fn opener_rejects_option_shaped_targets() {
        for target in [
            "-",
            "--",
            "--help",
            "--version",
            "-a",
            "--application=other",
        ] {
            let error = UrlDetector::opener_command(target, None).unwrap_err();
            assert_eq!(error.kind(), std::io::ErrorKind::InvalidInput);
            assert!(error.to_string().contains("./-name"));
            // The public entry point must fail before attempting to launch a
            // helper, even when a trusted terminal cwd was supplied.
            let error = UrlDetector::open_url_in_directory(target, Some(Path::new("/trusted/cwd")))
                .unwrap_err();
            assert_eq!(error.kind(), std::io::ErrorKind::InvalidInput);
        }
    }

    #[test]
    fn opener_preserves_file_uri_authorities() {
        for target in [
            "file://remote.example/home/user/Downloads",
            "file://localhost/home/user/file.txt",
            "file://[::1]/tmp/file",
            "FILE://Remote.Example/share/file.txt",
            "file://remote.example",
        ] {
            let command =
                UrlDetector::opener_command(target, Some(Path::new("/trusted/cwd"))).unwrap();
            assert_eq!(
                command.get_args().collect::<Vec<_>>(),
                [std::ffi::OsStr::new(target)]
            );
            assert!(command.get_current_dir().is_none());
        }
    }

    #[test]
    fn opener_preserves_encoded_local_file_uris() {
        for target in [
            "file:///tmp/file.txt",
            "file:///tmp/a%20b%23c%2520.txt",
            "file://localhost/tmp/%E7%95%8C.txt",
            "file:/tmp/a%3Fb.txt",
            "file:///C:/Users/Example/a%20b.txt",
        ] {
            let command = UrlDetector::opener_command(target, None).unwrap();
            assert_eq!(
                command.get_args().collect::<Vec<_>>(),
                [std::ffi::OsStr::new(target)]
            );
        }
    }

    #[test]
    fn opener_keeps_literal_arguments_and_only_rebases_relative_paths() {
        let cwd = Path::new("/trusted/cwd");
        for target in [
            "./a b;$(not-a-command).txt",
            "report.txt",
            "src/main.rs",
            "a b;$(not-a-command).txt",
            "./-report.txt",
            "../--help",
            ".\\-report.txt",
            "/absolute/--help",
            "file:///tmp/--help",
            "../file",
            ".\\file",
            "..\\file",
            "/absolute/a b.txt",
            "https://example.test/?x=a%20b&y=2",
        ] {
            let command = UrlDetector::opener_command(target, Some(cwd)).unwrap();
            assert_eq!(
                command.get_args().collect::<Vec<_>>(),
                [std::ffi::OsStr::new(target)]
            );
            assert_eq!(
                command.get_current_dir(),
                UrlDetector::is_relative_path(target).then_some(cwd)
            );
            assert_eq!(command.get_program(), "xdg-open");
        }
    }

    #[test]
    fn all_filesystem_relative_paths_require_the_terminal_directory() {
        for target in [
            "./file",
            "../file",
            ".\\file",
            "..\\file",
            "PROJ-17",
            "src/main.rs",
            "report.txt",
            ".",
            "..",
            "./name:stream",
            "dir/name:stream",
            "界:note",
        ] {
            assert!(UrlDetector::is_relative_path(target), "{target}");
        }
        for target in [
            "https://example.test",
            "file:///tmp/file",
            "mailto:user@example.test",
            "ssh://example.test",
            "custom+app.v2:payload",
        ] {
            assert!(!UrlDetector::is_relative_path(target), "{target}");
        }
    }
}
