//! Diff review overlay — shows detected diffs with color-coded +/- lines
//!
//! A scrollable overlay for reviewing unified diffs detected in plugin tabs.
//! Supports scrolling, copy-to-clipboard, and dismiss actions.

use super::animation::OverlayAnimation;
use crate::terminal::diff_detector::DetectedDiff;

/// Diff review overlay state
pub struct DiffReviewOverlay {
    /// Animation state
    animation: OverlayAnimation,
    /// The diff being reviewed
    pub diff: Option<DetectedDiff>,
    /// Scroll offset (in lines)
    pub scroll_offset: usize,
    /// Index of the diff in the tab's list (for marking as reviewed)
    pub diff_tab_index: Option<usize>,
}

impl DiffReviewOverlay {
    pub fn new() -> Self {
        Self {
            animation: OverlayAnimation::new(),
            diff: None,
            scroll_offset: 0,
            diff_tab_index: None,
        }
    }

    /// Show a diff for review
    pub fn show(&mut self, diff: DetectedDiff, tab_index: usize) {
        self.diff = Some(diff);
        self.scroll_offset = 0;
        self.diff_tab_index = Some(tab_index);
        self.animation.open();
    }

    /// Dismiss the overlay
    pub fn dismiss(&mut self) {
        self.animation.close();
    }

    /// Whether the overlay is active (accepting input)
    pub fn is_active(&self) -> bool {
        self.animation.is_active()
    }

    /// Whether the overlay is visible (active or animating out)
    pub fn is_visible(&self) -> bool {
        self.animation.is_visible()
    }

    /// Scroll up
    pub fn scroll_up(&mut self, lines: usize) {
        self.scroll_offset = self.scroll_offset.saturating_sub(lines);
    }

    /// Scroll down
    pub fn scroll_down(&mut self, lines: usize) {
        self.scroll_offset += lines;
        // Clamp to content length
        if let Some(ref diff) = self.diff {
            let total_lines = self.total_lines(diff);
            if total_lines > 0 {
                self.scroll_offset = self.scroll_offset.min(total_lines.saturating_sub(1));
            }
        }
    }

    /// Total number of display lines for a diff
    fn total_lines(&self, diff: &DetectedDiff) -> usize {
        let mut count = 2; // filename header + blank line
        for hunk in &diff.hunks {
            count += 1; // hunk header
            count += hunk.lines.len();
            count += 1; // blank between hunks
        }
        count
    }

    /// Get the raw diff text for copying
    pub fn raw_text(&self) -> Option<&str> {
        self.diff.as_ref().map(|d| d.raw.as_str())
    }

    /// Get animation progress (0.0 = hidden, 1.0 = fully visible)
    pub fn animation(&self) -> f32 {
        self.animation.progress()
    }

    /// Update animation state. Returns true if still animating.
    pub fn update_animation(&mut self) -> bool {
        let still_animating = self.animation.update();
        // Clear diff data when close animation completes
        if !self.animation.is_active() && !self.animation.is_visible() {
            self.diff = None;
        }
        still_animating
    }
}
