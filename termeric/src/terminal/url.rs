//! URL detection and handling
//!
//! This module provides functionality for detecting URLs in terminal output
//! and handling click-to-open functionality.

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
        let trimmed = url_text.trim_end_matches(|c| matches!(c, '.' | ',' | ':' | ';' | '!' | '?'));
        trimmed.len()
    }

    /// Open a URL using the system default handler
    pub fn open_url(url: &str) -> std::io::Result<()> {
        // Handle file:// URLs specially - extract the path portion
        let effective_url = if url.starts_with("file://") {
            Self::normalize_file_url(url)
        } else {
            url.to_string()
        };

        #[cfg(target_os = "linux")]
        {
            std::process::Command::new("xdg-open")
                .arg(&effective_url)
                .spawn()?;
        }

        #[cfg(target_os = "macos")]
        {
            std::process::Command::new("open")
                .arg(&effective_url)
                .spawn()?;
        }

        #[cfg(target_os = "windows")]
        {
            std::process::Command::new("cmd")
                .args(["/C", "start", "", &effective_url])
                .spawn()?;
        }

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
}

/// URL cache for efficient lookup during rendering
pub struct UrlCache {
    /// Detected URLs indexed by row
    urls_by_row: Vec<Vec<DetectedUrl>>,
    /// Number of rows in the cache
    rows: usize,
}

impl UrlCache {
    /// Create a new URL cache
    pub fn new(rows: usize) -> Self {
        Self {
            urls_by_row: vec![Vec::new(); rows],
            rows,
        }
    }

    /// Clear the cache
    pub fn clear(&mut self) {
        for row in &mut self.urls_by_row {
            row.clear();
        }
    }

    /// Resize the cache
    pub fn resize(&mut self, rows: usize) {
        self.urls_by_row.resize(rows, Vec::new());
        self.rows = rows;
    }

    /// Add URLs for a row
    pub fn set_row(&mut self, row: usize, urls: Vec<DetectedUrl>) {
        if row < self.rows {
            self.urls_by_row[row] = urls;
        }
    }

    /// Get URL at a specific position
    pub fn url_at(&self, row: usize, col: usize) -> Option<&DetectedUrl> {
        if row < self.rows {
            self.urls_by_row[row]
                .iter()
                .find(|url| url.contains(row, col))
        } else {
            None
        }
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
}
