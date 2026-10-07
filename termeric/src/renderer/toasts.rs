//! Toast rendering — bottom-right stacked cards with severity-tinted border.

use super::Renderer;
use crate::ui::color::{scale_rgb, with_alpha};
use crate::ui::toast::{ToastInfo, ToastPlacement, ToastSeverity, toast_at, toast_layout};

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
            // Left accent bar (severity indicator)
            self.rect_pipeline.push_rounded_rect(
                x + 3.0,
                y + 4.0,
                3.0,
                row_h - 8.0,
                accent_color,
                2.0,
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
