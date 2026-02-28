//! Context picker — modal UI for assembling context bundles for AI sessions
//!
//! Provides a multi-source context assembler that gathers data from git diffs,
//! file trees, terminal output, etc. and injects the assembled text into a
//! plugin tab's PTY as a single coherent context bundle.

use std::process::Command;
use std::time::Instant;

/// Animation duration in seconds
const ANIMATION_DURATION: f32 = 0.15;

/// Maximum characters per context source
const MAX_SOURCE_CHARS: usize = 10_000;

/// A context source that can be selected
#[derive(Debug, Clone)]
pub struct ContextSource {
    /// Human-readable label
    pub label: String,
    /// Whether this source is selected
    pub selected: bool,
    /// Whether this source is available (e.g., git diff only in git repos)
    pub available: bool,
    /// The kind of source
    pub kind: ContextKind,
}

/// Types of context sources
#[derive(Debug, Clone)]
pub enum ContextKind {
    /// `git diff` (unstaged changes)
    GitDiffUnstaged,
    /// `git diff --cached` (staged changes)
    GitDiffStaged,
    /// File tree from project root
    FileTree,
    /// Last N lines from any terminal tab
    TerminalOutput { lines: usize },
    /// Last detected test output (PASS/FAIL/error patterns)
    TestOutput,
    /// Specific files (file content injection)
    SpecificFiles { paths: Vec<String> },
}

/// Context picker overlay state
pub struct ContextPicker {
    /// Whether the overlay is active
    active: bool,
    /// Available context sources
    pub sources: Vec<ContextSource>,
    /// Currently highlighted source index
    pub selected_index: usize,
    /// Assembled context text (after gathering)
    pub assembled: Option<String>,
    /// Working directory for commands
    pub cwd: Option<String>,
    /// Animation start time
    animation_start: Instant,
    /// Whether animating open
    animating_open: bool,
    /// Status message (e.g., "Gathering context...")
    pub status: Option<String>,
}

impl ContextPicker {
    pub fn new() -> Self {
        Self {
            active: false,
            sources: Vec::new(),
            selected_index: 0,
            assembled: None,
            cwd: None,
            animation_start: Instant::now(),
            animating_open: false,
            status: None,
        }
    }

    /// Open the context picker with the given working directory
    pub fn open(&mut self, cwd: Option<String>) {
        self.cwd = cwd.clone();
        let in_git = is_git_repo(cwd.as_deref());
        self.sources = vec![
            ContextSource {
                label: "Git Diff (unstaged)".into(),
                selected: true,
                available: in_git,
                kind: ContextKind::GitDiffUnstaged,
            },
            ContextSource {
                label: "Git Diff (staged)".into(),
                selected: false,
                available: in_git,
                kind: ContextKind::GitDiffStaged,
            },
            ContextSource {
                label: "File Tree".into(),
                selected: false,
                available: true,
                kind: ContextKind::FileTree,
            },
            ContextSource {
                label: "Recent Terminal Output (50 lines)".into(),
                selected: false,
                available: true,
                kind: ContextKind::TerminalOutput { lines: 50 },
            },
            ContextSource {
                label: "Last Test Output".into(),
                selected: false,
                available: true,
                kind: ContextKind::TestOutput,
            },
            ContextSource {
                label: "Specific Files...".into(),
                selected: false,
                available: in_git,
                kind: ContextKind::SpecificFiles { paths: Vec::new() },
            },
        ];
        self.selected_index = 0;
        self.assembled = None;
        self.status = None;
        self.active = true;
        self.animating_open = true;
        self.animation_start = Instant::now();
    }

    /// Move selection up
    pub fn move_up(&mut self) {
        if self.selected_index > 0 {
            self.selected_index -= 1;
        }
    }

    /// Move selection down
    pub fn move_down(&mut self) {
        if self.selected_index < self.sources.len().saturating_sub(1) {
            self.selected_index += 1;
        }
    }

    /// Toggle the selected source
    pub fn toggle_selected(&mut self) {
        if let Some(source) = self.sources.get_mut(self.selected_index) {
            if source.available {
                source.selected = !source.selected;
            }
        }
    }

    /// Gather all selected context sources and assemble the text
    pub fn gather(&mut self, terminal_output: Option<String>) -> String {
        let mut parts = Vec::new();

        for source in &self.sources {
            if !source.selected || !source.available {
                continue;
            }
            match &source.kind {
                ContextKind::GitDiffUnstaged => {
                    if let Some(diff) = run_git_diff(self.cwd.as_deref(), false) {
                        if !diff.trim().is_empty() {
                            parts.push(format!("=== Git Diff (Unstaged) ===\n{}", truncate(&diff, MAX_SOURCE_CHARS)));
                        }
                    }
                }
                ContextKind::GitDiffStaged => {
                    if let Some(diff) = run_git_diff(self.cwd.as_deref(), true) {
                        if !diff.trim().is_empty() {
                            parts.push(format!("=== Git Diff (Staged) ===\n{}", truncate(&diff, MAX_SOURCE_CHARS)));
                        }
                    }
                }
                ContextKind::FileTree => {
                    if let Some(tree) = run_file_tree(self.cwd.as_deref()) {
                        parts.push(format!("=== File Tree ===\n{}", truncate(&tree, 5_000)));
                    }
                }
                ContextKind::TerminalOutput { .. } => {
                    if let Some(ref output) = terminal_output {
                        if !output.trim().is_empty() {
                            parts.push(format!("=== Recent Terminal Output ===\n{}", truncate(output, MAX_SOURCE_CHARS)));
                        }
                    }
                }
                ContextKind::TestOutput => {
                    if let Some(ref output) = terminal_output {
                        if let Some(test_output) = extract_test_output(output) {
                            parts.push(format!("=== Test Output ===\n{}", truncate(&test_output, MAX_SOURCE_CHARS)));
                        }
                    }
                }
                ContextKind::SpecificFiles { paths } => {
                    for path in paths {
                        if let Ok(content) = std::fs::read_to_string(path) {
                            let truncated = truncate(&content, MAX_SOURCE_CHARS);
                            parts.push(format!("=== File: {} ===\n{}", path, truncated));
                        }
                    }
                }
            }
        }

        let assembled = if parts.is_empty() {
            "(No context sources produced output)".to_string()
        } else {
            parts.join("\n\n")
        };

        self.assembled = Some(assembled.clone());
        assembled
    }

    /// Dismiss the overlay
    pub fn dismiss(&mut self) {
        self.animating_open = false;
        self.animation_start = Instant::now();
    }

    /// Whether the overlay is active (accepting input)
    pub fn is_active(&self) -> bool {
        self.active
    }

    /// Whether the overlay is visible (active or animating out)
    pub fn is_visible(&self) -> bool {
        self.active || self.animation() > 0.001
    }

    /// Get animation progress
    pub fn animation(&self) -> f32 {
        let elapsed = self.animation_start.elapsed().as_secs_f32();
        let t = (elapsed / ANIMATION_DURATION).min(1.0);
        let raw = if self.animating_open { t } else { 1.0 - t };
        raw.clamp(0.0, 1.0)
    }

    /// Update animation state. Returns true if still animating.
    pub fn update_animation(&mut self) -> bool {
        let anim = self.animation();
        if !self.animating_open && anim < 0.001 {
            self.active = false;
            return false;
        }
        self.animation_start.elapsed().as_secs_f32() < ANIMATION_DURATION
    }

    /// Whether any source is selected
    pub fn has_selection(&self) -> bool {
        self.sources.iter().any(|s| s.selected && s.available)
    }
}

/// Check if a directory is inside a git repository
fn is_git_repo(cwd: Option<&str>) -> bool {
    let mut cmd = Command::new("git");
    cmd.args(["rev-parse", "--git-dir"]);
    if let Some(dir) = cwd {
        cmd.current_dir(dir);
    }
    cmd.output().map(|o| o.status.success()).unwrap_or(false)
}

/// Run git diff (staged or unstaged)
fn run_git_diff(cwd: Option<&str>, staged: bool) -> Option<String> {
    let mut cmd = Command::new("git");
    cmd.arg("diff");
    if staged {
        cmd.arg("--cached");
    }
    if let Some(dir) = cwd {
        cmd.current_dir(dir);
    }
    cmd.output().ok().and_then(|o| {
        if o.status.success() {
            String::from_utf8(o.stdout).ok()
        } else {
            None
        }
    })
}

/// Run file tree listing (respects .gitignore via git ls-files)
fn run_file_tree(cwd: Option<&str>) -> Option<String> {
    let mut cmd = Command::new("git");
    cmd.args(["ls-files"]);
    if let Some(dir) = cwd {
        cmd.current_dir(dir);
    }
    let output = cmd.output().ok()?;
    if output.status.success() {
        String::from_utf8(output.stdout).ok()
    } else {
        // Fallback: find
        let mut cmd = Command::new("find");
        cmd.args([".", "-type", "f", "-not", "-path", "./.git/*"]);
        if let Some(dir) = cwd {
            cmd.current_dir(dir);
        }
        cmd.output().ok().and_then(|o| String::from_utf8(o.stdout).ok())
    }
}

/// Extract test-related output from terminal text.
///
/// Looks for patterns that indicate test output: PASS, FAIL, error[E,
/// "panicked at", "test result:", and captures the surrounding block.
pub fn extract_test_output(output: &str) -> Option<String> {
    let test_patterns = [
        "test result:",
        "PASS",
        "FAIL",
        "panicked at",
        "error[E",
        "PASSED",
        "FAILED",
        "tests passed",
        "tests failed",
    ];

    let lines: Vec<&str> = output.lines().collect();
    let mut test_lines = Vec::new();
    let mut in_test_block = false;
    let mut last_test_line = 0;

    for (i, line) in lines.iter().enumerate() {
        let is_test_line = test_patterns.iter().any(|p| line.contains(p));
        if is_test_line {
            // Include 5 lines of context before the first match
            if !in_test_block {
                let start = i.saturating_sub(5);
                for prev in &lines[start..i] {
                    test_lines.push(*prev);
                }
            }
            in_test_block = true;
            last_test_line = i;
            test_lines.push(line);
        } else if in_test_block && i - last_test_line < 3 {
            // Include a few lines after test output
            test_lines.push(line);
        } else {
            in_test_block = false;
        }
    }

    if test_lines.is_empty() {
        None
    } else {
        Some(test_lines.join("\n"))
    }
}

/// Truncate a string to a maximum number of characters
fn truncate(s: &str, max: usize) -> &str {
    if s.len() <= max {
        s
    } else {
        // Find a safe char boundary
        let mut end = max;
        while !s.is_char_boundary(end) && end > 0 {
            end -= 1;
        }
        &s[..end]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_test_output() {
        let output = "some random output\ncompiling...\ntest foo ... ok\ntest bar ... FAIL\ntest result: 1 passed; 1 failed\nmore stuff";
        let extracted = extract_test_output(output).unwrap();
        assert!(extracted.contains("FAIL"));
        assert!(extracted.contains("test result:"));
    }

    #[test]
    fn test_extract_test_output_none() {
        let output = "hello world\nno test results here";
        assert!(extract_test_output(output).is_none());
    }

    #[test]
    fn test_extract_test_output_rust_panic() {
        let output = "running tests\nthread 'main' panicked at 'assertion failed'\nnote: run with RUST_BACKTRACE=1";
        let extracted = extract_test_output(output).unwrap();
        assert!(extracted.contains("panicked at"));
    }
}
