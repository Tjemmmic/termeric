//! Diff detector — watches terminal output for unified diff patterns
//!
//! Scans lines of terminal output for unified diff format (`--- a/`, `+++ b/`,
//! `@@ ... @@`), collects complete diffs, and stores them for review.
//!
//! The scanner operates as a simple state machine that can handle both
//! standard `diff -u` output and `git diff` output with color escape sequences.

/// A single line in a diff hunk
#[derive(Debug, Clone, PartialEq)]
pub enum DiffLine {
    /// Unchanged context line
    Context(String),
    /// Added line (+)
    Added(String),
    /// Removed line (-)
    Removed(String),
}

/// A single hunk within a diff
#[derive(Debug, Clone)]
pub struct DiffHunk {
    /// The @@ header line (e.g., "@@ -1,3 +1,4 @@")
    pub header: String,
    /// Lines within this hunk
    pub lines: Vec<DiffLine>,
}

/// A complete detected diff for one file
#[derive(Debug, Clone)]
pub struct DetectedDiff {
    /// The filename being diffed (from the +++ line)
    pub filename: String,
    /// All hunks in this diff
    pub hunks: Vec<DiffHunk>,
    /// Complete raw diff text (for copy/apply)
    pub raw: String,
    /// Whether the user has reviewed this diff
    pub reviewed: bool,
}

/// Internal scanner state
#[derive(Debug)]
enum ScanState {
    /// Waiting for a `---` line
    Idle,
    /// Saw `--- a/file` line, expecting `+++ b/file`
    SeenMinus { minus_line: String },
    /// Inside a diff, collecting hunks
    InDiff {
        minus_line: String,
        plus_line: String,
        hunks: Vec<DiffHunk>,
        current_hunk: Option<DiffHunk>,
        raw_lines: Vec<String>,
    },
}

/// Watches terminal output lines and detects unified diffs
#[derive(Debug)]
pub struct DiffDetector {
    state: ScanState,
    /// Completed diffs ready for review
    pub detected: Vec<DetectedDiff>,
}

impl DiffDetector {
    pub fn new() -> Self {
        Self {
            state: ScanState::Idle,
            detected: Vec::new(),
        }
    }

    /// Feed a line of terminal output to the detector.
    ///
    /// The line should have ANSI escape sequences stripped before calling.
    pub fn feed_line(&mut self, line: &str) {
        let trimmed = line.trim_end();

        match std::mem::replace(&mut self.state, ScanState::Idle) {
            ScanState::Idle => {
                if is_minus_line(trimmed) {
                    self.state = ScanState::SeenMinus {
                        minus_line: trimmed.to_string(),
                    };
                }
                // else stay Idle
            }

            ScanState::SeenMinus { minus_line } => {
                if is_plus_line(trimmed) {
                    // Valid --- / +++ pair, enter diff mode
                    self.state = ScanState::InDiff {
                        minus_line: minus_line.clone(),
                        plus_line: trimmed.to_string(),
                        hunks: Vec::new(),
                        current_hunk: None,
                        raw_lines: vec![minus_line, trimmed.to_string()],
                    };
                } else if is_minus_line(trimmed) {
                    // Another --- line, restart
                    self.state = ScanState::SeenMinus {
                        minus_line: trimmed.to_string(),
                    };
                } else {
                    // Not a valid pair, reset
                    self.state = ScanState::Idle;
                }
            }

            ScanState::InDiff {
                minus_line,
                plus_line,
                mut hunks,
                mut current_hunk,
                mut raw_lines,
            } => {
                raw_lines.push(trimmed.to_string());

                if is_hunk_header(trimmed) {
                    // Finish previous hunk if any
                    if let Some(hunk) = current_hunk.take() {
                        if !hunk.lines.is_empty() {
                            hunks.push(hunk);
                        }
                    }
                    // Start new hunk
                    current_hunk = Some(DiffHunk {
                        header: trimmed.to_string(),
                        lines: Vec::new(),
                    });
                    self.state = ScanState::InDiff {
                        minus_line,
                        plus_line,
                        hunks,
                        current_hunk,
                        raw_lines,
                    };
                } else if let Some(ref mut hunk) = current_hunk {
                    // Inside a hunk — classify the line
                    // IMPORTANT: check is_minus_line BEFORE strip_prefix('-')
                    // so that '--- a/file.rs' triggers a new diff instead of
                    // being misclassified as a removed line.
                    if is_minus_line(trimmed) {
                        // New diff starts — finalize current one
                        if let Some(hunk) = current_hunk.take() {
                            if !hunk.lines.is_empty() {
                                hunks.push(hunk);
                            }
                        }
                        if !hunks.is_empty() {
                            raw_lines.pop(); // Don't include the new --- line in old diff
                            self.finalize_diff(&plus_line, hunks, &raw_lines);
                        }
                        self.state = ScanState::SeenMinus {
                            minus_line: trimmed.to_string(),
                        };
                    } else if is_hunk_header(trimmed) {
                        // New hunk within same diff — finish previous hunk
                        if let Some(h) = current_hunk.take() {
                            if !h.lines.is_empty() {
                                hunks.push(h);
                            }
                        }
                        current_hunk = Some(DiffHunk {
                            header: trimmed.to_string(),
                            lines: Vec::new(),
                        });
                        self.state = ScanState::InDiff {
                            minus_line,
                            plus_line,
                            hunks,
                            current_hunk,
                            raw_lines,
                        };
                    } else if let Some(content) = trimmed.strip_prefix('+') {
                        hunk.lines.push(DiffLine::Added(content.to_string()));
                        self.state = ScanState::InDiff {
                            minus_line,
                            plus_line,
                            hunks,
                            current_hunk,
                            raw_lines,
                        };
                    } else if let Some(content) = trimmed.strip_prefix('-') {
                        hunk.lines.push(DiffLine::Removed(content.to_string()));
                        self.state = ScanState::InDiff {
                            minus_line,
                            plus_line,
                            hunks,
                            current_hunk,
                            raw_lines,
                        };
                    } else if trimmed.starts_with(' ') || trimmed.is_empty() {
                        // Context line
                        let ctx = if trimmed.starts_with(' ') {
                            &trimmed[1..]
                        } else {
                            trimmed
                        };
                        hunk.lines.push(DiffLine::Context(ctx.to_string()));
                        self.state = ScanState::InDiff {
                            minus_line,
                            plus_line,
                            hunks,
                            current_hunk,
                            raw_lines,
                        };
                    } else {
                        // Non-diff line — finalize current diff
                        if let Some(hunk) = current_hunk.take() {
                            if !hunk.lines.is_empty() {
                                hunks.push(hunk);
                            }
                        }
                        if !hunks.is_empty() {
                            self.finalize_diff(&plus_line, hunks, &raw_lines);
                        }
                        // state is already Idle
                    }
                } else {
                    // No current hunk yet, expecting @@ header
                    if is_minus_line(trimmed) {
                        // New diff starts, old one had no hunks
                        self.state = ScanState::SeenMinus {
                            minus_line: trimmed.to_string(),
                        };
                    }
                    // else stay Idle (no valid hunk header after +++ line)
                }
            }
        }
    }

    /// Flush any in-progress diff (e.g., when output stops)
    pub fn flush(&mut self) {
        if let ScanState::InDiff {
            plus_line,
            mut hunks,
            current_hunk,
            raw_lines,
            ..
        } = std::mem::replace(&mut self.state, ScanState::Idle)
        {
            if let Some(hunk) = current_hunk {
                if !hunk.lines.is_empty() {
                    hunks.push(hunk);
                }
            }
            if !hunks.is_empty() {
                self.finalize_diff(&plus_line, hunks, &raw_lines);
            }
        }
    }

    /// Get count of unreviewed diffs
    pub fn unreviewed_count(&self) -> usize {
        self.detected.iter().filter(|d| !d.reviewed).count()
    }

    /// Mark all diffs as reviewed
    pub fn mark_all_reviewed(&mut self) {
        for diff in &mut self.detected {
            diff.reviewed = true;
        }
    }

    /// Get the most recent unreviewed diff
    pub fn latest_unreviewed(&self) -> Option<&DetectedDiff> {
        self.detected.iter().rev().find(|d| !d.reviewed)
    }

    fn finalize_diff(&mut self, plus_line: &str, hunks: Vec<DiffHunk>, raw_lines: &[String]) {
        let filename = extract_filename(plus_line);
        let raw = raw_lines.join("\n");
        self.detected.push(DetectedDiff {
            filename,
            hunks,
            raw,
            reviewed: false,
        });
    }
}

/// Check if a line looks like `--- a/file` or `--- file`
fn is_minus_line(line: &str) -> bool {
    line.starts_with("--- a/")
        || line.starts_with("--- b/")
        || (line.starts_with("--- ") && !line.starts_with("--- /dev/null") && line.len() > 5)
}

/// Check if a line looks like `+++ b/file` or `+++ file`
fn is_plus_line(line: &str) -> bool {
    line.starts_with("+++ a/")
        || line.starts_with("+++ b/")
        || (line.starts_with("+++ ") && line.len() > 5)
}

/// Check if a line is a hunk header: `@@ -start,count +start,count @@`
fn is_hunk_header(line: &str) -> bool {
    line.starts_with("@@ ") && line.contains(" @@")
}

/// Extract filename from a +++ line
fn extract_filename(plus_line: &str) -> String {
    let path = plus_line.trim_start_matches("+++ ");
    let path = path.strip_prefix("b/").unwrap_or(path);
    // Strip any trailing tab + timestamp (from standard diff)
    path.split('\t').next().unwrap_or(path).to_string()
}

/// Strip ANSI escape sequences from a string
pub fn strip_ansi(input: &str) -> String {
    let mut result = String::with_capacity(input.len());
    let mut chars = input.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\x1b' {
            // Skip ESC[...m sequences
            if chars.peek() == Some(&'[') {
                chars.next(); // consume '['
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_simple_diff() {
        let mut detector = DiffDetector::new();
        let lines = [
            "--- a/src/main.rs",
            "+++ b/src/main.rs",
            "@@ -1,3 +1,4 @@",
            " fn main() {",
            "-    println!(\"hello\");",
            "+    println!(\"hello, world!\");",
            "+    println!(\"goodbye\");",
            " }",
        ];
        for line in &lines {
            detector.feed_line(line);
        }
        detector.flush();

        assert_eq!(detector.detected.len(), 1);
        let diff = &detector.detected[0];
        assert_eq!(diff.filename, "src/main.rs");
        assert_eq!(diff.hunks.len(), 1);
        assert_eq!(diff.hunks[0].lines.len(), 5);
        assert_eq!(
            diff.hunks[0].lines[0],
            DiffLine::Context("fn main() {".to_string())
        );
        assert_eq!(
            diff.hunks[0].lines[1],
            DiffLine::Removed("    println!(\"hello\");".to_string())
        );
        assert_eq!(
            diff.hunks[0].lines[2],
            DiffLine::Added("    println!(\"hello, world!\");".to_string())
        );
        assert_eq!(
            diff.hunks[0].lines[3],
            DiffLine::Added("    println!(\"goodbye\");".to_string())
        );
        assert_eq!(diff.hunks[0].lines[4], DiffLine::Context("}".to_string()));
    }

    #[test]
    fn test_multiple_hunks() {
        let mut detector = DiffDetector::new();
        let lines = [
            "--- a/file.txt",
            "+++ b/file.txt",
            "@@ -1,2 +1,2 @@",
            "-old line",
            "+new line",
            "@@ -10,2 +10,2 @@",
            "-old line 10",
            "+new line 10",
        ];
        for line in &lines {
            detector.feed_line(line);
        }
        detector.flush();

        assert_eq!(detector.detected.len(), 1);
        assert_eq!(detector.detected[0].hunks.len(), 2);
    }

    #[test]
    fn test_multiple_files() {
        let mut detector = DiffDetector::new();
        let lines = [
            "--- a/file1.rs",
            "+++ b/file1.rs",
            "@@ -1,1 +1,1 @@",
            "-old",
            "+new",
            "--- a/file2.rs",
            "+++ b/file2.rs",
            "@@ -1,1 +1,1 @@",
            "-old2",
            "+new2",
        ];
        for line in &lines {
            detector.feed_line(line);
        }
        detector.flush();

        assert_eq!(detector.detected.len(), 2);
        assert_eq!(detector.detected[0].filename, "file1.rs");
        assert_eq!(detector.detected[1].filename, "file2.rs");
    }

    #[test]
    fn test_no_diff() {
        let mut detector = DiffDetector::new();
        detector.feed_line("just a normal line");
        detector.feed_line("nothing to see here");
        detector.flush();

        assert_eq!(detector.detected.len(), 0);
    }

    #[test]
    fn test_incomplete_diff() {
        let mut detector = DiffDetector::new();
        detector.feed_line("--- a/file.rs");
        detector.feed_line("some other line"); // Not a +++ line
        detector.flush();

        assert_eq!(detector.detected.len(), 0);
    }

    #[test]
    fn test_unreviewed_count() {
        let mut detector = DiffDetector::new();
        let lines = [
            "--- a/file.rs",
            "+++ b/file.rs",
            "@@ -1,1 +1,1 @@",
            "-old",
            "+new",
        ];
        for line in &lines {
            detector.feed_line(line);
        }
        detector.flush();

        assert_eq!(detector.unreviewed_count(), 1);
        detector.mark_all_reviewed();
        assert_eq!(detector.unreviewed_count(), 0);
    }

    #[test]
    fn test_strip_ansi() {
        assert_eq!(strip_ansi("\x1b[31m--- a/file.rs\x1b[0m"), "--- a/file.rs");
        assert_eq!(strip_ansi("normal text"), "normal text");
        assert_eq!(strip_ansi("\x1b[1;32m+added\x1b[0m"), "+added");
    }

    #[test]
    fn test_git_diff_with_noise() {
        let mut detector = DiffDetector::new();
        let lines = [
            "diff --git a/src/main.rs b/src/main.rs",
            "index abc123..def456 100644",
            "--- a/src/main.rs",
            "+++ b/src/main.rs",
            "@@ -5,3 +5,4 @@",
            " use std::io;",
            "+use std::fs;",
            " ",
            " fn main() {",
            "some random output after the diff",
        ];
        for line in &lines {
            detector.feed_line(line);
        }
        detector.flush();

        assert_eq!(detector.detected.len(), 1);
        assert_eq!(detector.detected[0].filename, "src/main.rs");
    }

    #[test]
    fn test_raw_text_preserved() {
        let mut detector = DiffDetector::new();
        let lines = [
            "--- a/file.rs",
            "+++ b/file.rs",
            "@@ -1,1 +1,1 @@",
            "-old",
            "+new",
        ];
        for line in &lines {
            detector.feed_line(line);
        }
        detector.flush();

        let raw = &detector.detected[0].raw;
        assert!(raw.contains("--- a/file.rs"));
        assert!(raw.contains("+new"));
    }
}
