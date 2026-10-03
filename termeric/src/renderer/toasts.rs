//! Toast rendering — bottom-right stacked cards with severity-tinted border.

use super::Renderer;
use crate::ui::color::with_alpha;
use crate::ui::constants::CARD_CORNER_RADIUS;
use crate::ui::toast::{ToastInfo, ToastSeverity, toast_layout};

impl Renderer {
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

        let cell_width = self.cell_width;
        let cell_height = self.cell_height;
        let padding = cell_width;
        for placement in toast_layout(toasts, window_width, window_height, cell_width, cell_height)
        {
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
            let bg = [
                self.ui.palette_bg[0] * 0.95,
                self.ui.palette_bg[1] * 0.95,
                self.ui.palette_bg[2] * 0.95,
                0.97 * opacity,
            ];

            // Drop shadow
            self.rect_pipeline.push_rounded_rect(
                x + 2.0,
                y + 3.0,
                w,
                row_h,
                [0.0, 0.0, 0.0, 0.25 * opacity],
                CARD_CORNER_RADIUS,
            );
            // Background
            self.rect_pipeline
                .push_rounded_rect(x, y, w, row_h, bg, CARD_CORNER_RADIUS);
            // Border (outer)
            self.rect_pipeline
                .push_rounded_rect(x, y, w, row_h, border_color, CARD_CORNER_RADIUS);
            self.rect_pipeline.push_rounded_rect(
                x + 1.0,
                y + 1.0,
                w - 2.0,
                row_h - 2.0,
                bg,
                CARD_CORNER_RADIUS - 1.0,
            );
            // Left accent bar (severity indicator)
            self.rect_pipeline.push_rounded_rect(
                x + 3.0,
                y + 4.0,
                3.0,
                row_h - 8.0,
                accent_color,
                2.0,
            );

            // Message text
            let text_x = x + padding + cell_width * 0.8;
            let text_y = y + (row_h - cell_height) / 2.0;
            let text_color = with_alpha(self.ui.palette_text, opacity);
            let max_chars = ((w - padding * 2.0 - cell_width) / cell_width) as usize;
            let mut cx = text_x;
            for (i, c) in toast.message.chars().enumerate() {
                if i >= max_chars.saturating_sub(1) && max_chars > 0 {
                    // Ellipsis if we run out of room.
                    self.text_pipeline.push_char('…', cx, text_y, text_color);
                    break;
                }
                self.text_pipeline.push_char(c, cx, text_y, text_color);
                cx += cell_width;
            }
        }
    }
}
