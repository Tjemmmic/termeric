//! Info overlay — centered card for displaying version/about information
//!
//! A simple dismissable overlay for showing app info, version, etc.

use super::animation::OverlayAnimation;
use super::constants::CONTENT_CHAR_SCALE;

/// Geometry for the centered info overlay card — single source of truth
/// shared by the renderer and the click hit-test in `event_loop`.
#[derive(Debug, Clone, Copy)]
#[allow(dead_code)] // Several fields are exposed for future callers.
pub struct InfoOverlayLayout {
    pub card_x: f32,
    pub card_y: f32,
    pub card_width: f32,
    pub card_height: f32,
    pub padding: f32,
    pub title_height: f32,
    pub content_start_y: f32,
    pub content_height: f32,
    pub line_height: f32,
    pub char_scale: f32,
    pub cell_width: f32,
    pub cell_height: f32,
    // Button row
    pub btn_y: f32,
    pub btn_h: f32,
    pub btn_h_padding: f32,
    pub copy_btn_x: f32,
    pub copy_btn_w: f32,
    pub continue_btn_x: f32,
    pub continue_btn_w: f32,
}

impl InfoOverlayLayout {
    pub fn calculate(
        window_width: f32,
        window_height: f32,
        cell_width: f32,
        cell_height: f32,
        animation: f32,
        title: &str,
        lines: &[String],
    ) -> Self {
        let char_scale = cell_width * CONTENT_CHAR_SCALE;
        let padding = cell_width * 2.5;
        let line_height = cell_height * 1.6;
        let title_height = cell_height * 2.8;

        // Match the renderer: hint string determines a min width so the
        // dismiss hint never wraps.
        let hint_chars = 37_usize; // "Esc / Enter to dismiss  ·  C to copy"
        let max_line_chars = lines.iter().map(|l| l.chars().count()).max().unwrap_or(20);
        let content_w = max_line_chars.max(hint_chars) as f32 * char_scale;
        let title_w = title.chars().count() as f32 * cell_width;
        let inner_w = content_w.max(title_w);
        let card_width = (inner_w + padding * 2.0 + cell_width * 2.0)
            .max(380.0)
            .min(window_width * 0.85);

        let content_height = lines.len() as f32 * line_height;
        let btn_area_height = cell_height * 4.5;
        let card_height = padding + title_height + content_height + btn_area_height + padding * 0.5;
        let card_x = (window_width - card_width) / 2.0;
        // Slide-in from above with the shared -30 offset.
        let card_y = (window_height - card_height) / 2.0 + (1.0 - animation) * -30.0;

        let title_y = card_y + padding;
        let content_start_y = title_y + title_height;
        let btn_sep_y = content_start_y + content_height + cell_height * 0.5;
        let btn_h = cell_height * 1.8;
        let btn_h_padding = cell_width * 2.0;
        let btn_gap = cell_width * 1.5;
        let btn_y = btn_sep_y + cell_height * 0.8;

        let copy_btn_w = 4.0 * cell_width + btn_h_padding * 2.0;
        let continue_btn_w = 8.0 * cell_width + btn_h_padding * 2.0;
        let total_btns_w = copy_btn_w + btn_gap + continue_btn_w;
        let copy_btn_x = card_x + (card_width - total_btns_w) / 2.0;
        let continue_btn_x = copy_btn_x + copy_btn_w + btn_gap;

        Self {
            card_x,
            card_y,
            card_width,
            card_height,
            padding,
            title_height,
            content_start_y,
            content_height,
            line_height,
            char_scale,
            cell_width,
            cell_height,
            btn_y,
            btn_h,
            btn_h_padding,
            copy_btn_x,
            copy_btn_w,
            continue_btn_x,
            continue_btn_w,
        }
    }

    /// Hit-test for the Copy button.
    pub fn copy_button_contains(&self, x: f32, y: f32) -> bool {
        x >= self.copy_btn_x
            && x <= self.copy_btn_x + self.copy_btn_w
            && y >= self.btn_y
            && y <= self.btn_y + self.btn_h
    }

    /// Hit-test for the Continue button.
    pub fn continue_button_contains(&self, x: f32, y: f32) -> bool {
        x >= self.continue_btn_x
            && x <= self.continue_btn_x + self.continue_btn_w
            && y >= self.btn_y
            && y <= self.btn_y + self.btn_h
    }
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
        self.lines = vec![format!("v{}", env!("CARGO_PKG_VERSION"))];
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

    /// Get eased animation progress (0.0 = hidden, 1.0 = fully visible)
    pub fn animation(&self) -> f32 {
        self.animation.eased()
    }

    /// Update animation state. Returns true if still animating.
    pub fn update_animation(&mut self) -> bool {
        self.animation.update()
    }
}
