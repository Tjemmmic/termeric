//! Shared animation state machines for UI overlays and panels
//!
//! Provides two reusable animation types:
//! - `OverlayAnimation` — for modal overlays (fade in/out)
//! - `PanelAnimation` — for slide-in panels with seamless mid-animation reversal

use std::time::Instant;

/// Default duration for overlay open/close animations (seconds).
pub const DEFAULT_OVERLAY_DURATION: f32 = 0.15;

/// Default duration for panel slide animations (seconds).
pub const DEFAULT_PANEL_DURATION: f32 = 0.2;

/// Threshold below which animation is considered complete.
const ANIM_EPSILON: f32 = 0.001;

/// Ease-out cubic: fast start, decelerating finish.
pub fn ease_out_cubic(t: f32) -> f32 {
    1.0 - (1.0 - t).powi(3)
}

// ── OverlayAnimation ──────────────────────────────────────────────────

/// Simple time-based overlay animation for modal overlays that fade in/out.
///
/// Used by: InfoOverlay, DiffReviewOverlay, ContextPicker, GuardrailOverlay,
/// CommandPalette.
pub struct OverlayAnimation {
    active: bool,
    animation_start: Instant,
    animating_open: bool,
    duration: f32,
}

impl OverlayAnimation {
    /// Create a new overlay animation with the default duration.
    pub fn new() -> Self {
        Self {
            active: false,
            animation_start: Instant::now(),
            animating_open: false,
            duration: DEFAULT_OVERLAY_DURATION,
        }
    }

    /// Create a new overlay animation with a custom duration.
    pub fn with_duration(duration: f32) -> Self {
        Self {
            duration,
            ..Self::new()
        }
    }

    /// Start the open animation.
    pub fn open(&mut self) {
        self.active = true;
        self.animating_open = true;
        self.animation_start = Instant::now();
    }

    /// Start the close animation.
    pub fn close(&mut self) {
        self.animating_open = false;
        self.animation_start = Instant::now();
    }

    /// Whether the overlay is logically active (accepting input).
    pub fn is_active(&self) -> bool {
        self.active
    }

    /// Whether the overlay is visible (active or still animating out).
    pub fn is_visible(&self) -> bool {
        self.active || self.progress() > ANIM_EPSILON
    }

    /// Immediately deactivate without close animation.
    pub fn close_immediately(&mut self) {
        self.active = false;
        self.animating_open = false;
    }

    /// Linear animation progress (0.0 = hidden, 1.0 = fully visible).
    pub fn progress(&self) -> f32 {
        if !self.active {
            return 0.0;
        }
        let elapsed = self.animation_start.elapsed().as_secs_f32();
        let t = (elapsed / self.duration).min(1.0);
        let raw = if self.animating_open { t } else { 1.0 - t };
        raw.clamp(0.0, 1.0)
    }

    /// Animation progress with ease-out cubic applied.
    pub fn eased(&self) -> f32 {
        ease_out_cubic(self.progress())
    }

    /// Update animation state. Returns true if still animating (needs redraw).
    pub fn update(&mut self) -> bool {
        let anim = self.progress();
        if !self.animating_open && anim < ANIM_EPSILON {
            self.active = false;
            return false;
        }
        self.animation_start.elapsed().as_secs_f32() < self.duration
    }
}

// ── PanelAnimation ────────────────────────────────────────────────────

/// Panel animation supporting seamless mid-animation direction reversal.
///
/// Used by: SettingsPanel, SessionPanel, CommandsPanel.
/// These are slide-in panels that may be toggled rapidly, requiring the
/// animation to reverse smoothly from its current position.
#[derive(Debug, Clone)]
pub struct PanelAnimation {
    open: bool,
    animation_start: Instant,
    animating_open: bool,
    animation_base: f32,
    duration: f32,
}

impl PanelAnimation {
    /// Create a new panel animation with the default duration.
    pub fn new() -> Self {
        Self {
            open: false,
            animation_start: Instant::now(),
            animating_open: false,
            animation_base: 0.0,
            duration: DEFAULT_PANEL_DURATION,
        }
    }

    /// Create a new panel animation with a custom duration.
    pub fn with_duration(duration: f32) -> Self {
        Self {
            duration,
            ..Self::new()
        }
    }

    /// Start the open animation, capturing the current position for seamless reversal.
    pub fn open(&mut self) {
        self.animation_base = self.raw_progress();
        self.open = true;
        self.animating_open = true;
        self.animation_start = Instant::now();
    }

    /// Start the close animation, capturing the current position for seamless reversal.
    /// Sets `open` to false immediately so that `is_open()` returns false right away,
    /// matching the original CommandsPanel behavior where toggle() did `self.open = !self.open`.
    /// The close animation still plays out via `raw_progress()` / `is_visible()`.
    pub fn close(&mut self) {
        self.animation_base = self.raw_progress();
        self.open = false;
        self.animating_open = false;
        self.animation_start = Instant::now();
    }

    /// Toggle between open and close.
    pub fn toggle(&mut self) {
        if self.open {
            self.close();
        } else {
            self.open();
        }
    }

    /// Whether the panel is logically open.
    pub fn is_open(&self) -> bool {
        self.open
    }

    /// Whether the panel is fully closed (animation complete).
    pub fn is_closed(&self) -> bool {
        !self.open && self.raw_progress() <= 0.0
    }

    /// Whether the panel is visible (open or animating).
    pub fn is_visible(&self) -> bool {
        self.open || self.raw_progress() > ANIM_EPSILON
    }

    /// Whether animation is still in progress (needs redraw).
    pub fn needs_redraw(&self) -> bool {
        if self.open {
            self.raw_progress() < 1.0
        } else {
            self.raw_progress() > 0.0
        }
    }

    /// Raw linear progress (0.0 = closed, 1.0 = open), accounting for
    /// the animation base for seamless reversal.
    pub fn raw_progress(&self) -> f32 {
        let elapsed = self.animation_start.elapsed().as_secs_f32();
        let linear = (elapsed / self.duration).min(1.0);

        if self.animating_open {
            self.animation_base + (1.0 - self.animation_base) * linear
        } else {
            self.animation_base * (1.0 - linear)
        }
    }

    /// Animation progress with ease-out cubic applied.
    pub fn progress(&self) -> f32 {
        ease_out_cubic(self.raw_progress())
    }

    /// Update animation state. Returns true if still animating (needs redraw).
    pub fn update(&mut self) -> bool {
        let anim = self.raw_progress();
        if !self.animating_open && anim < ANIM_EPSILON {
            self.open = false;
            return false;
        }
        self.animation_start.elapsed().as_secs_f32() < self.duration
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread;
    use std::time::Duration;

    #[test]
    fn test_overlay_starts_inactive() {
        let anim = OverlayAnimation::new();
        assert!(!anim.is_active());
        assert!(!anim.is_visible());
    }

    #[test]
    fn test_overlay_open_close() {
        let mut anim = OverlayAnimation::new();
        anim.open();
        assert!(anim.is_active());
        assert!(anim.is_visible());

        anim.close();
        // Still visible during close animation
        assert!(!anim.is_active() || anim.is_visible());

        // Wait for animation to complete
        thread::sleep(Duration::from_millis(200));
        anim.update();
        assert!(!anim.is_active());
    }

    #[test]
    fn test_overlay_progress_bounds() {
        let mut anim = OverlayAnimation::new();
        anim.open();
        let p = anim.progress();
        assert!((0.0..=1.0).contains(&p));

        thread::sleep(Duration::from_millis(200));
        let p = anim.progress();
        assert!((p - 1.0).abs() < 0.01);
    }

    #[test]
    fn test_panel_starts_closed() {
        let panel = PanelAnimation::new();
        assert!(!panel.is_open());
        assert!(panel.is_closed());
    }

    #[test]
    fn test_panel_toggle() {
        let mut panel = PanelAnimation::new();
        panel.toggle();
        assert!(panel.is_open());
        panel.toggle();
        // Immediately no longer "open" after close
        assert!(!panel.is_open());
    }

    #[test]
    fn test_panel_visible_during_close() {
        let mut panel = PanelAnimation::new();
        panel.open();
        // Let the open animation complete
        thread::sleep(Duration::from_millis(250));
        assert!(panel.is_open());
        assert!(panel.is_visible());
        panel.close();
        // No longer "open" but still visible during close animation
        assert!(!panel.is_open());
        assert!(panel.is_visible());
    }

    #[test]
    fn test_panel_seamless_reversal() {
        let mut panel = PanelAnimation::new();
        panel.open();
        thread::sleep(Duration::from_millis(50));
        // Mid-animation, toggle should capture current position
        let mid_progress = panel.raw_progress();
        assert!(mid_progress > 0.0);
        panel.close();
        // After reversal, progress should start from mid_progress
        let post_toggle = panel.raw_progress();
        assert!(post_toggle <= mid_progress + 0.1); // Allow small timing variance
    }

    #[test]
    fn test_ease_out_cubic() {
        assert!((ease_out_cubic(0.0)).abs() < f32::EPSILON);
        assert!((ease_out_cubic(1.0) - 1.0).abs() < f32::EPSILON);
        // Mid-point should be > 0.5 (fast start, slow finish)
        assert!(ease_out_cubic(0.5) > 0.5);
    }
}
