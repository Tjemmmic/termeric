//! Toast rendering — bottom-right stacked cards with severity-tinted border.

use super::Renderer;
use crate::ui::color::{scale_rgb, with_alpha};
use crate::ui::constants::{BORDER_WIDTH, CARD_CORNER_RADIUS};
use crate::ui::geometry::Bounds;
use crate::ui::toast::{ToastInfo, ToastPlacement, ToastSeverity, toast_at, toast_layout};

/// The severity bar's width and its distance from the card's left edge, in
/// logical pixels.
const ACCENT_BAR_WIDTH: f32 = 3.0;
const ACCENT_BAR_GAP: f32 = 3.0;

impl Renderer {
    /// Where each toast's card sits: in the terminal area's corner, clear
    /// of the dock. Drawing and the hit test both place toasts here.
    fn toast_placements(
        &self,
        toasts: &[ToastInfo],
        window_width: f32,
        window_height: f32,
    ) -> Vec<ToastPlacement> {
        toast_layout(
            toasts,
            self.content_right_for(window_width),
            window_height,
            self.cell_width(),
            self.cell_height(),
        )
    }

    /// The ID of the toast the last frame drew under `(x, y)`.
    pub fn toast_hit_test(&self, x: f32, y: f32) -> Option<u64> {
        let (window_width, window_height) = self.window_size();
        let placements =
            self.toast_placements(&self.toasts, window_width as f32, window_height as f32);
        toast_at(&self.toasts, &placements, x, y)
    }

    /// Draw the active toast stack in the bottom-right corner, newest at
    /// the bottom. Each toast fades in, holds, then fades out.
    pub(super) fn build_toasts(
        &mut self,
        toasts: &[ToastInfo],
        window_width: f32,
        window_height: f32,
    ) {
        if toasts.is_empty() {
            return;
        }

        let cell_width = self.cell_width();
        let padding = cell_width;
        let label_size = self.ui_label_px();
        for placement in self.toast_placements(toasts, window_width, window_height) {
            let toast = &toasts[placement.toast_index];
            let x = placement.rect.x;
            let y = placement.rect.y;
            let w = placement.rect.width;
            let row_h = placement.rect.height;
            let opacity = toast.opacity;

            // Severity colors — pull from the theme palette.
            let (border_color, accent_color) = match toast.severity {
                ToastSeverity::Info => (
                    with_alpha(self.ui.palette_border, 0.45 * opacity),
                    with_alpha(self.ui.palette_accent, opacity),
                ),
                ToastSeverity::Success => (
                    with_alpha(self.ui.success, 0.45 * opacity),
                    with_alpha(self.ui.success, opacity),
                ),
                ToastSeverity::Warning => (
                    with_alpha(self.ui.warning, 0.55 * opacity),
                    with_alpha(self.ui.warning, opacity),
                ),
                ToastSeverity::Error => (
                    with_alpha(self.ui.error, 0.65 * opacity),
                    with_alpha(self.ui.error, opacity),
                ),
            };

            // Card background — slightly darker than the palette background
            // so the toast stays legible over both terminal content and
            // modal overlays.
            let bg = with_alpha(scale_rgb(self.ui.palette_bg, 0.95), 0.97 * opacity);

            self.draw_card_shadow(placement.rect, 0.2 * opacity, 2);
            self.draw_card(placement.rect, bg, border_color);
            let bar = accent_bar(
                placement.rect,
                self.px(CARD_CORNER_RADIUS),
                self.px(BORDER_WIDTH),
                self.px(ACCENT_BAR_GAP),
                self.px(ACCENT_BAR_WIDTH),
            );
            self.rect_pipeline.push_rounded_rect(
                bar.x,
                bar.y,
                bar.width,
                bar.height,
                accent_color,
                bar.width / 2.0,
            );

            let text_x = x + padding + cell_width * 0.8;
            self.push_ui_label(
                &toast.message,
                text_x,
                y,
                row_h,
                x + w - padding - text_x,
                label_size,
                with_alpha(self.ui.palette_text, opacity),
            );
        }
    }
}

/// The severity bar along a toast card's left edge: `width` wide, `gap` in
/// from the edge, and as tall as it can be while its rounded ends stay
/// inside the card's border where the corners curve. Lengths are physical.
fn accent_bar(card: Bounds, corner_radius: f32, border: f32, gap: f32, width: f32) -> Bounds {
    let cap = width / 2.0;
    // Each end is a semicircle; it fits while its centre is no farther
    // from the corner's centre than the inner edge's radius less its own.
    let reach = (corner_radius - border - cap).max(0.0);
    let across = (corner_radius - gap - cap).max(0.0);
    let down = (reach * reach - across * across).max(0.0).sqrt();
    let inset = (corner_radius - cap - down).max(border).ceil();
    Bounds::new(
        card.x + gap,
        card.y + inset,
        width,
        (card.height - inset * 2.0).max(0.0),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Whether `(x, y)` lies inside `bounds` rounded by `radius`.
    fn inside_rounded(bounds: Bounds, radius: f32, x: f32, y: f32) -> bool {
        let nearest_x = x.clamp(bounds.x + radius, bounds.right() - radius);
        let nearest_y = y.clamp(bounds.y + radius, bounds.bottom() - radius);
        bounds.contains(x, y) && (x - nearest_x).hypot(y - nearest_y) <= radius + 1e-3
    }

    #[test]
    fn the_accent_bar_stays_inside_the_cards_rounded_border() {
        for scale in [1.0_f32, 1.25, 1.5, 2.0, 3.0] {
            let px = |logical: f32| (logical * scale).round().max(1.0);
            let (radius, border) = (px(CARD_CORNER_RADIUS), px(BORDER_WIDTH));
            let width = px(ACCENT_BAR_WIDTH);
            for height in [32.0, 40.0, 64.0] {
                let card = Bounds::new(10.0, 20.0, 300.0, height * scale);
                let bar = accent_bar(card, radius, border, px(ACCENT_BAR_GAP), width);
                assert!(
                    bar.height > card.height / 2.0,
                    "{scale}: {bar:?} in {card:?}"
                );
                let inner = card.inset(border, border);
                let cap = width / 2.0;
                // The bar's outline: its straight sides and its round ends.
                let outline = (0..=64).flat_map(|step| {
                    let angle = std::f32::consts::TAU * step as f32 / 64.0;
                    let (sin, cos) = angle.sin_cos();
                    let center_x = bar.x + cap;
                    let end_y = if sin < 0.0 {
                        bar.y + cap
                    } else {
                        bar.bottom() - cap
                    };
                    [(center_x + cap * cos, end_y + cap * sin)]
                });
                for (x, y) in outline {
                    assert!(
                        inside_rounded(inner, radius - border, x, y),
                        "{scale}: ({x}, {y}) of {bar:?} leaves {card:?}"
                    );
                }
            }
        }
    }
}
