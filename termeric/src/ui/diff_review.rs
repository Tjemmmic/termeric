//! Diff review overlay — shows detected diffs with color-coded +/- lines
//!
//! A scrollable overlay for reviewing unified diffs detected in plugin tabs.
//! Supports scrolling, copy-to-clipboard, and dismiss actions.

use std::time::Instant;

use crate::terminal::diff_detector::DetectedDiff;

/// Animation duration in seconds
const ANIMATION_DURATION: f32 = 0.15;

/// Diff review overlay state
pub struct DiffReviewOverlay {
    /// Whether the overlay is currently active
    active: bool,
    /// The diff being reviewed
    pub diff: Option<DetectedDiff>,
    /// Scroll offset (in lines)
    pub scroll_offset: usize,
    /// Animation start time
    animation_start: Instant,
    /// Whether animating open
    animating_open: bool,
    /// Index of the diff in the tab's list (for marking as reviewed)
    pub diff_tab_index: Option<usize>,
}

impl DiffReviewOverlay {
    pub fn new() -> Self {
        Self {
            active: false,
            diff: None,
            scroll_offset: 0,
            animation_start: Instant::now(),
            animating_open: false,
            diff_tab_index: None,
        }
    }

    /// Show a diff for review
    pub fn show(&mut self, diff: DetectedDiff, tab_index: usize) {
        self.diff = Some(diff);
        self.scroll_offset = 0;
        self.diff_tab_index = Some(tab_index);
        self.active = true;
        self.animating_open = true;
        self.animation_start = Instant::now();
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
        let elapsed = self.animation_start.elapsed().as_secs_f32();
        let t = (elapsed / ANIMATION_DURATION).min(1.0);

        let raw = if self.animating_open {
            t
        } else {
            1.0 - t
        };

        raw.clamp(0.0, 1.0)
    }

    /// Update animation state. Returns true if still animating.
    pub fn update_animation(&mut self) -> bool {
        let anim = self.animation();
        if !self.animating_open && anim < 0.001 {
            self.active = false;
            self.diff = None;
            return false;
        }
        self.animation_start.elapsed().as_secs_f32() < ANIMATION_DURATION
    }
}
