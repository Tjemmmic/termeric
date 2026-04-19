//! Toast rendering — bottom-right stacked cards with severity-tinted border.

use super::{Renderer, adjust_alpha};
use crate::ui::toast::{ToastInfo, ToastSeverity};
use crate::ui::constants::CARD_CORNER_RADIUS;

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
        let padding = cell_width * 1.0;
        let row_h = cell_height * 2.0;
        let gap = 6.0;
        // Cap toast width at 50% of the window, but respect a sensible minimum
        // so very short messages don't render in a cramped pill.
        let max_w = (window_width * 0.5).min(cell_width * 48.0);
        let min_w = (cell_width * 18.0).min(max_w);

        // Stack from the bottom up, newest (last in iter) at the bottom.
        let right_margin = cell_width * 1.5;
        let bottom_margin = cell_height * 1.0;
        let mut y = window_height - bottom_margin;

        for toast in toasts.iter().rev() {
            y -= row_h;
            let opacity = toast.opacity;
            // Width from message length, clamped.
            let text_w = toast.message.chars().count() as f32 * cell_width;
            let w = (text_w + padding * 2.0 + cell_width * 2.0)
                .clamp(min_w, max_w);
            let x = window_width - w - right_margin;

            // Slide-in: offset horizontally until fully faded in.
            let slide = (1.0 - opacity) * (cell_width * 2.0);
            let tx = x + slide;

            // Severity colors — pull from the theme palette.
            let (border_color, accent_color) = match toast.severity {
                ToastSeverity::Info => (
                    adjust_alpha(self.ui.palette_border, 0.45 * opacity),
                    adjust_alpha(self.ui.palette_accent, opacity),
                ),
                ToastSeverity::Success => {
                    let green = [0.38, 0.73, 0.42, 1.0];
                    (adjust_alpha(green, 0.45 * opacity), adjust_alpha(green, opacity))
                }
                ToastSeverity::Warning => {
                    let amber = [0.92, 0.66, 0.20, 1.0];
                    (adjust_alpha(amber, 0.55 * opacity), adjust_alpha(amber, opacity))
                }
                ToastSeverity::Error => {
                    let red = [0.85, 0.36, 0.30, 1.0];
                    (adjust_alpha(red, 0.65 * opacity), adjust_alpha(red, opacity))
                }
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
                tx + 2.0, y + 3.0,
                w, row_h,
                [0.0, 0.0, 0.0, 0.25 * opacity],
                CARD_CORNER_RADIUS,
            );
            // Background
            self.rect_pipeline.push_rounded_rect(
                tx, y,
                w, row_h,
                bg, CARD_CORNER_RADIUS,
            );
            // Border (outer)
            self.rect_pipeline.push_rounded_rect(
                tx, y,
                w, row_h,
                border_color, CARD_CORNER_RADIUS,
            );
            self.rect_pipeline.push_rounded_rect(
                tx + 1.0, y + 1.0,
                w - 2.0, row_h - 2.0,
                bg, CARD_CORNER_RADIUS - 1.0,
            );
            // Left accent bar (severity indicator)
            self.rect_pipeline.push_rounded_rect(
                tx + 3.0, y + 4.0,
                3.0, row_h - 8.0,
                accent_color, 2.0,
            );

            // Message text
            let text_x = tx + padding + cell_width * 0.8;
            let text_y = y + (row_h - cell_height) / 2.0;
            let text_color = adjust_alpha(self.ui.palette_text, opacity);
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

            y -= gap;
        }
    }
}
