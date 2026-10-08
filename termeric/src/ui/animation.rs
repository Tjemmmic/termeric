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

/// Time-based animation for modal overlays that fade in and out. Owners call
/// `update()` every frame so the close animation can end their input.
pub struct OverlayAnimation {
    active: bool,
    animation_start: Instant,
    animating_open: bool,
    duration: f32,
    reduced_motion: bool,
}

impl OverlayAnimation {
    /// Create a new overlay animation with the default duration.
    pub fn new() -> Self {
        Self {
            active: false,
            animation_start: Instant::now(),
            animating_open: false,
            duration: DEFAULT_OVERLAY_DURATION,
            reduced_motion: false,
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
        if self.reduced_motion {
            self.close_immediately();
            return;
        }
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
        if self.reduced_motion {
            return if self.active && self.animating_open {
                1.0
            } else {
                0.0
            };
        }
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
        if self.reduced_motion {
            if !self.animating_open {
                self.active = false;
            }
            return false;
        }
        let anim = self.progress();
        if !self.animating_open && anim < ANIM_EPSILON {
            self.active = false;
            return false;
        }
        self.animation_start.elapsed().as_secs_f32() < self.duration
    }

    /// Disable non-essential transition frames while preserving the logical
    /// open/closed state and immediate input revocation on close.
    pub fn set_reduced_motion(&mut self, reduced_motion: bool) {
        self.reduced_motion = reduced_motion;
        if reduced_motion && !self.animating_open {
            self.active = false;
        }
    }
}

// ── PanelAnimation ────────────────────────────────────────────────────

/// Animation for slide-in panels, which may be toggled rapidly and so
/// reverse smoothly from wherever the animation is.
#[derive(Debug, Clone)]
pub struct PanelAnimation {
    open: bool,
    animation_start: Instant,
    animation_base: f32,
    duration: f32,
    reduced_motion: bool,
}

impl PanelAnimation {
    /// Create a new panel animation with the default duration.
    pub fn new() -> Self {
        Self {
            open: false,
            animation_start: Instant::now(),
            animation_base: 0.0,
            duration: DEFAULT_PANEL_DURATION,
            reduced_motion: false,
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
        self.animation_start = Instant::now();
    }

    /// Start the close animation, capturing the current position for seamless reversal.
    /// `is_open()` is false at once, so the panel stops taking input; the close
    /// animation still plays out through `raw_progress()` and `is_visible()`.
    pub fn close(&mut self) {
        self.animation_base = self.raw_progress();
        self.open = false;
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
        if self.reduced_motion {
            return if self.open { 1.0 } else { 0.0 };
        }
        let elapsed = self.animation_start.elapsed().as_secs_f32();
        let linear = (elapsed / self.duration).min(1.0);

        if self.open {
            self.animation_base + (1.0 - self.animation_base) * linear
        } else {
            self.animation_base * (1.0 - linear)
        }
    }

    /// Animation progress with ease-out cubic applied.
    pub fn progress(&self) -> f32 {
        ease_out_cubic(self.raw_progress())
    }

    /// Disable non-essential transition frames. Open panels remain fully
    /// visible and closed panels become fully hidden immediately.
    pub fn set_reduced_motion(&mut self, reduced_motion: bool) {
        self.reduced_motion = reduced_motion;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    /// Move an animation's start into the past, as if `elapsed` had passed
    /// since it began, so timing is tested without sleeping.
    fn rewind(start: &mut Instant, elapsed: Duration) {
        *start = start.checked_sub(elapsed).unwrap();
    }

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
        // Still active and visible during the close animation
        assert!(anim.is_active());
        assert!(anim.is_visible());

        rewind(&mut anim.animation_start, Duration::from_millis(200));
        assert!(!anim.update());
        assert!(!anim.is_active());
        assert!(!anim.is_visible());
    }

    #[test]
    fn test_overlay_progress_bounds() {
        let mut anim = OverlayAnimation::new();
        anim.open();
        let p = anim.progress();
        assert!((0.0..=1.0).contains(&p));

        rewind(&mut anim.animation_start, Duration::from_millis(200));
        assert_eq!(anim.progress(), 1.0);
        assert_eq!(anim.eased(), 1.0);
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
        rewind(&mut panel.animation_start, Duration::from_millis(250));
        assert_eq!(panel.raw_progress(), 1.0);
        assert!(!panel.needs_redraw());
        assert!(panel.is_open());
        assert!(panel.is_visible());
        panel.close();
        // No longer "open" but still visible during close animation
        assert!(!panel.is_open());
        assert!(panel.is_visible());
        assert!(!panel.is_closed());

        rewind(&mut panel.animation_start, Duration::from_millis(250));
        assert!(!panel.is_visible());
        assert!(panel.is_closed());
    }

    #[test]
    fn test_panel_seamless_reversal() {
        let mut panel = PanelAnimation::new();
        panel.open();
        // A quarter of the way through the 200ms open animation
        rewind(&mut panel.animation_start, Duration::from_millis(50));
        let mid_progress = panel.raw_progress();
        assert!(mid_progress >= 0.25);
        panel.close();
        // The close starts from where the open had reached
        assert!(panel.animation_base >= mid_progress);
        assert!(panel.raw_progress() <= panel.animation_base);

        rewind(&mut panel.animation_start, Duration::from_millis(200));
        assert_eq!(panel.raw_progress(), 0.0);
        assert!(panel.is_closed());
    }

    #[test]
    fn test_ease_out_cubic() {
        assert!((ease_out_cubic(0.0)).abs() < f32::EPSILON);
        assert!((ease_out_cubic(1.0) - 1.0).abs() < f32::EPSILON);
        // Mid-point should be > 0.5 (fast start, slow finish)
        assert!(ease_out_cubic(0.5) > 0.5);
    }

    #[test]
    fn reduced_motion_makes_overlay_and_panel_transitions_immediate() {
        let mut overlay = OverlayAnimation::new();
        overlay.set_reduced_motion(true);
        overlay.open();
        assert_eq!(overlay.progress(), 1.0);
        assert!(!overlay.update());
        overlay.close();
        assert!(!overlay.is_active());
        assert!(!overlay.is_visible());

        let mut panel = PanelAnimation::new();
        panel.set_reduced_motion(true);
        panel.open();
        assert_eq!(panel.raw_progress(), 1.0);
        assert!(!panel.needs_redraw());
        panel.close();
        assert_eq!(panel.raw_progress(), 0.0);
        assert!(panel.is_closed());
    }
}
