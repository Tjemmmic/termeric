//! Info overlay — centered card for displaying version/about information
//!
//! A simple dismissable overlay for showing app info, version, etc.

use super::animation::OverlayAnimation;

/// What kind of info to show
#[derive(Debug, Clone)]
pub enum InfoKind {
    About,
    Version,
}

/// Info overlay state
pub struct InfoOverlay {
    /// Animation state
    animation: OverlayAnimation,
    /// Title text
    pub title: String,
    /// Content lines
    pub lines: Vec<String>,
}

impl InfoOverlay {
    pub fn new() -> Self {
        Self {
            animation: OverlayAnimation::new(),
            title: String::new(),
            lines: Vec::new(),
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
        self.animation.open();
    }

    /// Show a compact version overlay
    pub fn show_version(&mut self) {
        self.title = "Termeric".into();
        self.lines = vec![
            format!("v{}", env!("CARGO_PKG_VERSION")),
        ];
        self.animation.open();
    }

    /// Show a custom info overlay with the given title and lines
    pub fn show(&mut self, title: impl Into<String>, lines: Vec<String>) {
        self.title = title.into();
        self.lines = lines;
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

    /// Get animation progress (0.0 = hidden, 1.0 = fully visible)
    pub fn animation(&self) -> f32 {
        self.animation.progress()
    }

    /// Update animation state. Returns true if still animating.
    pub fn update_animation(&mut self) -> bool {
        self.animation.update()
    }
}
