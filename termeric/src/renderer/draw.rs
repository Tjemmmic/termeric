//! Shared pieces of the chrome's cards and dialogs: shadows, card surfaces,
//! scrollbars, buttons, and focus rings.

use super::Renderer;
use crate::ui::color::with_alpha;
use crate::ui::constants::*;
use crate::ui::geometry::Bounds;

impl Renderer {
    /// A soft shadow under a card, `layers` rounded rectangles spreading
    /// down and out with falling alpha.
    pub(super) fn draw_card_shadow(&mut self, card: Bounds, alpha: f32, layers: u32) {
        for layer in 0..layers {
            let spread = (layer as f32 + 1.0) * 4.0;
            self.rect_pipeline.push_rounded_rect(
                card.x - spread,
                card.y - spread * 0.5 + spread * 0.8,
                card.width + spread * 2.0,
                card.height + spread * 1.5,
                [0.0, 0.0, 0.0, alpha / (layer as f32 + 1.0)],
                CARD_CORNER_RADIUS + spread,
            );
        }
    }

    /// A rounded card surface with a hairline border.
    pub(super) fn draw_card(&mut self, card: Bounds, background: [f32; 4], border: [f32; 4]) {
        self.rect_pipeline.push_rounded_rect(
            card.x,
            card.y,
            card.width,
            card.height,
            background,
            CARD_CORNER_RADIUS,
        );
        self.rect_pipeline.push_rounded_outline(
            card.x,
            card.y,
            card.width,
            card.height,
            border,
            CARD_CORNER_RADIUS,
            BORDER_WIDTH,
        );
    }

    /// A vertical scrollbar track and thumb for `visible` of `total` items
    /// scrolled by `offset`; nothing while everything fits.
    pub(super) fn draw_scrollbar(
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
        let track_color = with_alpha(self.ui.palette_border, 0.15 * anim);
        self.rect_pipeline
            .push_rounded_rect(x, y, width, height, track_color, 2.0);

        let thumb_height = (height * visible as f32 / total as f32)
            .max(self.cell_height())
            .min(height);
        let max_scroll = total - visible;
        let scroll_fraction = offset as f32 / max_scroll as f32;
        let thumb_y = y + scroll_fraction * (height - thumb_height);
        let thumb_color = with_alpha(self.ui.palette_accent, 0.5 * anim);
        self.rect_pipeline
            .push_rounded_rect(x, thumb_y, width, thumb_height, thumb_color, 2.0);
    }

    /// A dialog button in the accent color, its label centred in the UI
    /// face. `primary` raises the surface; hover and press tint it further.
    pub(super) fn draw_button(
        &mut self,
        button: Bounds,
        label: &str,
        primary: bool,
        hovered: bool,
        pressed: bool,
        anim: f32,
    ) {
        let base_alpha = if primary { 0.12 } else { 0.05 };
        let background_alpha = if pressed {
            base_alpha + 0.18
        } else if hovered {
            base_alpha + 0.10
        } else {
            base_alpha
        };
        // A press sinks the button by a pixel on every side.
        let button = if pressed {
            button.inset(1.0, 1.0)
        } else {
            button
        };
        let accent = self.ui.palette_accent;
        self.rect_pipeline.push_rounded_rect(
            button.x,
            button.y,
            button.width,
            button.height,
            with_alpha(accent, background_alpha * anim),
            BUTTON_CORNER_RADIUS,
        );
        let border_alpha = if hovered { 0.65 } else { 0.35 };
        self.rect_pipeline.push_rounded_outline(
            button.x,
            button.y,
            button.width,
            button.height,
            with_alpha(accent, border_alpha * anim),
            BUTTON_CORNER_RADIUS,
            BORDER_WIDTH,
        );
        if hovered && !pressed {
            self.rect_pipeline.push_rounded_rect(
                button.x - 2.0,
                button.y - 2.0,
                button.width + 4.0,
                button.height + 4.0,
                with_alpha(accent, 0.06 * anim),
                BUTTON_CORNER_RADIUS + 2.0,
            );
        }
        let text_alpha = if hovered { 1.0 } else { 0.8 };
        let size = self.ui_label_px();
        self.push_ui_label_centered(
            label,
            button.center()[0],
            button.y,
            button.height,
            button.width,
            size,
            with_alpha(accent, text_alpha * anim),
        );
    }

    /// A two-pixel keyboard focus ring just inside `bounds`.
    pub(super) fn draw_focus_ring(&mut self, bounds: Bounds, color: [f32; 4]) {
        self.rect_pipeline.push_rounded_outline(
            bounds.x,
            bounds.y,
            bounds.width,
            bounds.height,
            color,
            BUTTON_CORNER_RADIUS,
            2.0,
        );
    }
}
