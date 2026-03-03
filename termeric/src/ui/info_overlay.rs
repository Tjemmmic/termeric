//! Info overlay — centered card for displaying version/about information
//!
//! A simple dismissable overlay for showing app info, version, etc.

use std::time::Instant;

/// Animation duration in seconds
const ANIMATION_DURATION: f32 = 0.15;

/// What kind of info to show
#[derive(Debug, Clone)]
pub enum InfoKind {
    About,
    Version,
}

/// Info overlay state
pub struct InfoOverlay {
    /// Whether the overlay is currently active
    active: bool,
    /// Title text
    pub title: String,
    /// Content lines
    pub lines: Vec<String>,
    /// Animation start time
    animation_start: Instant,
    /// Whether animating open
    animating_open: bool,
}

impl InfoOverlay {
    pub fn new() -> Self {
        Self {
            active: false,
            title: String::new(),
            lines: Vec::new(),
            animation_start: Instant::now(),
            animating_open: false,
        }
    }

    /// Show the "About Termeric" overlay
    pub fn show_about(&mut self) {
        self.title = "About Termeric".into();
        self.lines = vec![
            format!("Version {}", env!("CARGO_PKG_VERSION")),
            String::new(),
            "A GPU-accelerated terminal emulator".into(),
            "Built with wgpu + winit".into(),
            String::new(),
            "github.com/termeric-terminal/termeric".into(),
        ];
        self.active = true;
        self.animating_open = true;
        self.animation_start = Instant::now();
    }

    /// Show a compact version overlay
    pub fn show_version(&mut self) {
        self.title = "Termeric".into();
        self.lines = vec![
            format!("v{}", env!("CARGO_PKG_VERSION")),
        ];
        self.active = true;
        self.animating_open = true;
        self.animation_start = Instant::now();
    }

    /// Show a custom info overlay with the given title and lines
    pub fn show(&mut self, title: impl Into<String>, lines: Vec<String>) {
        self.title = title.into();
        self.lines = lines;
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

    /// Get animation progress (0.0 = hidden, 1.0 = fully visible)
    pub fn animation(&self) -> f32 {
        let elapsed = self.animation_start.elapsed().as_secs_f32();
        let t = (elapsed / ANIMATION_DURATION).min(1.0);

        let raw = if self.animating_open {
            t
        } else {
            1.0 - t
        };

        // Update active state when close animation completes
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
}
