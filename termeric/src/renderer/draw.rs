//! Shared drawing helpers for UI rendering
//!
//! Provides `DrawCtx`, a context struct that borrows the rect and text pipelines
//! along with layout metrics and theme colors. Helper methods on `DrawCtx`
//! eliminate duplicated scrim, card, text, scrollbar, and button rendering
//! across 17+ `build_*` functions in the renderer.

use super::UiPalette;
use super::rect::RectPipeline;
use super::text::TextPipeline;
use crate::ui::color::{scale_alpha, with_alpha};
use crate::ui::constants::*;

/// Drawing context — borrows the pipelines and carries layout metrics.
///
/// Created per-frame (or per-overlay) to avoid borrow conflicts when
/// splitting `Renderer` methods into separate files.
pub struct DrawCtx<'a> {
    pub rect: &'a mut RectPipeline,
    pub text: &'a mut TextPipeline,
    pub cell_width: f32,
    pub cell_height: f32,
    pub ui: &'a UiPalette,
}

impl<'a> DrawCtx<'a> {
    // ── Scrim ──────────────────────────────────────────────────

    /// Draw a full-window scrim (semi-transparent overlay).
    ///
    /// For slide-in panels that only dim the area left of the panel,
    /// pass the panel x position as `width`.
    pub fn draw_scrim(&mut self, width: f32, height: f32, color: [f32; 4], anim: f32) {
        self.rect
            .push_rect(0.0, 0.0, width, height, scale_alpha(color, anim));
    }

    // ── Card ───────────────────────────────────────────────────

    /// Draw a rounded card background with a 4-side border.
    pub fn draw_card(&mut self, x: f32, y: f32, w: f32, h: f32, bg: [f32; 4], border: [f32; 4]) {
        // Background
        self.rect
            .push_rounded_rect(x, y, w, h, bg, CARD_CORNER_RADIUS);
        // 4-side border (inset from corners to avoid overlap with rounded edges)
        self.draw_card_border(x, y, w, h, border, CARD_CORNER_RADIUS);
    }

    /// Draw a 4-side border around a rounded rect.
    pub fn draw_card_border(
        &mut self,
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        color: [f32; 4],
        radius: f32,
    ) {
        // Top / bottom
        self.rect
            .push_rect(x + radius, y, w - radius * 2.0, BORDER_WIDTH, color);
        self.rect.push_rect(
            x + radius,
            y + h - BORDER_WIDTH,
            w - radius * 2.0,
            BORDER_WIDTH,
            color,
        );
        // Left / right
        self.rect
            .push_rect(x, y + radius, BORDER_WIDTH, h - radius * 2.0, color);
        self.rect.push_rect(
            x + w - BORDER_WIDTH,
            y + radius,
            BORDER_WIDTH,
            h - radius * 2.0,
            color,
        );
    }

    /// Draw layered shadow behind a card.
    pub fn draw_card_shadow(&mut self, x: f32, y: f32, w: f32, h: f32, alpha: f32, layers: u32) {
        for i in 0..layers {
            let spread = (i as f32 + 1.0) * 4.0;
            let a = alpha / (i as f32 + 1.0);
            let shadow = [0.0, 0.0, 0.0, a];
            self.rect.push_rounded_rect(
                x - spread,
                y - spread * 0.5 + spread * 0.8,
                w + spread * 2.0,
                h + spread * 1.5,
                shadow,
                CARD_CORNER_RADIUS + spread,
            );
        }
    }

    // ── Separator ──────────────────────────────────────────────

    /// Draw a horizontal separator line.
    pub fn draw_separator(&mut self, x: f32, y: f32, width: f32, color: [f32; 4]) {
        self.rect.push_rect(x, y, width, BORDER_WIDTH, color);
    }

    // ── Text ───────────────────────────────────────────────────

    /// Draw a string of text character-by-character.
    ///
    /// Returns the x position after the last character (i.e., the string width
    /// added to the start `x`). Uses `self.cell_width` as char advance.
    pub fn draw_text(&mut self, text: &str, x: f32, y: f32, color: [f32; 4]) -> f32 {
        let mut cx = x;
        for c in text.chars() {
            self.text.push_char(c, cx, y, color);
            cx += self.cell_width;
        }
        cx
    }

    /// Draw text at a custom character scale (e.g., `CONTENT_CHAR_SCALE * cell_width`).
    pub fn draw_text_scaled(
        &mut self,
        text: &str,
        x: f32,
        y: f32,
        color: [f32; 4],
        char_width: f32,
    ) -> f32 {
        let mut cx = x;
        for c in text.chars() {
            if !c.is_whitespace() {
                self.text.push_char(c, cx, y, color);
            }
            cx += char_width;
        }
        cx
    }

    /// Draw text centered horizontally around `center_x`.
    pub fn draw_centered_text(&mut self, text: &str, center_x: f32, y: f32, color: [f32; 4]) {
        let w = text.chars().count() as f32 * self.cell_width;
        self.draw_text(text, center_x - w / 2.0, y, color);
    }

    /// Draw text centered at a custom scale.
    pub fn draw_centered_text_scaled(
        &mut self,
        text: &str,
        center_x: f32,
        y: f32,
        color: [f32; 4],
        char_width: f32,
    ) {
        let w = text.chars().count() as f32 * char_width;
        self.draw_text_scaled(text, center_x - w / 2.0, y, color, char_width);
    }

    /// Draw text clipped to a maximum x boundary.
    ///
    /// Stops rendering characters that would exceed `max_x`.
    /// Returns the ending x position.
    pub fn draw_clipped_text(
        &mut self,
        text: &str,
        x: f32,
        y: f32,
        color: [f32; 4],
        max_x: f32,
    ) -> f32 {
        let mut cx = x;
        for c in text.chars() {
            if cx + self.cell_width > max_x {
                break;
            }
            self.text.push_char(c, cx, y, color);
            cx += self.cell_width;
        }
        cx
    }

    /// Draw text clipped at a custom scale.
    pub fn draw_clipped_text_scaled(
        &mut self,
        text: &str,
        x: f32,
        y: f32,
        color: [f32; 4],
        char_width: f32,
        max_x: f32,
    ) -> f32 {
        let mut cx = x;
        for c in text.chars() {
            if cx + char_width > max_x {
                break;
            }
            if !c.is_whitespace() {
                self.text.push_char(c, cx, y, color);
            }
            cx += char_width;
        }
        cx
    }

    // ── Scrollbar ──────────────────────────────────────────────

    /// Draw a vertical scrollbar (track + thumb).
    ///
    /// - `x, y, height` — track position and size
    /// - `total` — total number of items
    /// - `visible` — number of visible items
    /// - `offset` — current scroll offset
    /// - `anim` — animation multiplier (for fading in/out)
    /// - `width` — scrollbar width in pixels
    pub fn draw_scrollbar(
        &mut self,
        x: f32,
        y: f32,
        height: f32,
        total: usize,
        visible: usize,
        offset: usize,
        anim: f32,
        width: f32,
    ) {
        if total <= visible || visible == 0 {
            return;
        }

        // Track
        let track_color = with_alpha(self.ui.palette_border, 0.15 * anim);
        self.rect
            .push_rounded_rect(x, y, width, height, track_color, 2.0);

        // Thumb
        let thumb_frac = visible as f32 / total as f32;
        let thumb_h = (height * thumb_frac).max(self.cell_height).min(height);
        let max_scroll = total.saturating_sub(visible);
        let scroll_frac = if max_scroll > 0 {
            offset as f32 / max_scroll as f32
        } else {
            0.0
        };
        let thumb_y = y + scroll_frac * (height - thumb_h);
        let thumb_color = with_alpha(self.ui.palette_accent, 0.5 * anim);
        self.rect
            .push_rounded_rect(x, thumb_y, width, thumb_h, thumb_color, 2.0);
    }

    // ── Button ─────────────────────────────────────────────────

    /// Draw a themed button with optional hover/press states.
    ///
    /// Uses `palette_accent` for border and text colors.
    /// `primary` makes the button slightly more prominent.
    pub fn draw_button(
        &mut self,
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        label: &str,
        primary: bool,
        hovered: bool,
        pressed: bool,
        anim: f32,
    ) {
        // Background alpha varies by state
        let base_alpha = if primary { 0.12 } else { 0.05 };
        let bg_alpha = if pressed {
            base_alpha + 0.18
        } else if hovered {
            base_alpha + 0.10
        } else {
            base_alpha
        };

        // Press inset effect
        let inset = if pressed { 1.0 } else { 0.0 };
        let bx = x + inset;
        let by = y + inset;
        let bw = w - inset * 2.0;
        let bh = h - inset * 2.0;

        // Background
        let btn_bg = with_alpha(self.ui.palette_accent, bg_alpha * anim);
        self.rect
            .push_rounded_rect(bx, by, bw, bh, btn_bg, BUTTON_CORNER_RADIUS);

        // Border
        let border_alpha = if hovered { 0.65 } else { 0.35 };
        let btn_border = with_alpha(self.ui.palette_accent, border_alpha * anim);
        self.draw_card_border(bx, by, bw, bh, btn_border, BUTTON_CORNER_RADIUS);

        // Hover glow
        if hovered && !pressed {
            let glow = with_alpha(self.ui.palette_accent, 0.06 * anim);
            self.rect.push_rounded_rect(
                bx - 2.0,
                by - 2.0,
                bw + 4.0,
                bh + 4.0,
                glow,
                BUTTON_CORNER_RADIUS + 2.0,
            );
        }

        // Centered text
        let text_alpha = if hovered { 1.0 } else { 0.8 };
        let text_color = with_alpha(self.ui.palette_accent, text_alpha * anim);
        let text_x = bx + (bw - label.chars().count() as f32 * self.cell_width) / 2.0;
        let text_y = by + (bh - self.cell_height) / 2.0;
        self.draw_text(label, text_x, text_y, text_color);
    }
}
