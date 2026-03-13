//! AI session chrome bar rendering

use super::{Renderer, adjust_alpha};
use crate::ui::constants::*;

impl Renderer {
    pub(super) fn build_chrome_bar(
        &mut self,
        info: &crate::ui::claude_chrome::ChromeBarInfo,
        window_width: f32,
        tab_bar_y_end: f32,
        bar_height: f32,
    ) {
        let cell_width = self.cell_width;
        let cell_height = self.cell_height;
        let char_scale = cell_width * 0.8;
        let bar_y = tab_bar_y_end;

        // Background — slightly darker than tab bar
        let bg = [
            self.ui.tab_bar_bg[0] * 0.85,
            self.ui.tab_bar_bg[1] * 0.85,
            self.ui.tab_bar_bg[2] * 0.85,
            1.0,
        ];
        self.rect_pipeline.push_rect(0.0, bar_y, window_width, bar_height, bg);

        // Thin separator line at bottom
        let sep = [
            self.ui.palette_border[0],
            self.ui.palette_border[1],
            self.ui.palette_border[2],
            0.3,
        ];
        self.rect_pipeline.push_rect(0.0, bar_y + bar_height - 1.0, window_width, 1.0, sep);

        let text_y = bar_y + (bar_height - cell_height) / 2.0;
        let text_color = self.ui.palette_text;
        let dim_color = self.ui.palette_text_dim;
        let accent = self.ui.palette_accent;

        let mut cx = cell_width; // left padding

        // ── Repo name ──
        if let Some(ref repo) = info.repo_name {
            for c in repo.chars() {
                if !c.is_whitespace() {
                    self.text_pipeline.push_char(c, cx, text_y, accent);
                }
                cx += char_scale;
            }
        }

        // ── Branch (with dirty indicator) ──
        if let Some(ref branch) = info.branch {
            // Separator dot
            cx += char_scale * 0.5;
            self.text_pipeline.push_char('·', cx, text_y, dim_color);
            cx += char_scale * 1.5;

            // Branch icon
            self.text_pipeline.push_char('⎇', cx, text_y, dim_color);
            cx += char_scale * 1.2;

            for c in branch.chars() {
                if !c.is_whitespace() {
                    self.text_pipeline.push_char(c, cx, text_y, text_color);
                }
                cx += char_scale;
            }

            // Dirty indicator
            if info.dirty {
                cx += char_scale * 0.3;
                self.text_pipeline.push_char('●', cx, text_y, [0.9, 0.6, 0.2, 1.0]);
                cx += char_scale;
            }
        }

        // ── Host badge (pill) ──
        cx += char_scale * 1.5;
        let host_start = cx;
        let badge_padding = char_scale * 0.5;
        let badge_text_width = info.host_label.chars().count() as f32 * char_scale;
        let badge_width = badge_text_width + badge_padding * 2.0;
        let badge_height = cell_height * 0.9;
        let badge_y = bar_y + (bar_height - badge_height) / 2.0;

        // Badge background
        let badge_bg = [accent[0] * 0.25, accent[1] * 0.25, accent[2] * 0.25, 0.6];
        self.rect_pipeline.push_rounded_rect(host_start, badge_y, badge_width, badge_height, badge_bg, badge_height / 2.0);

        // Badge text
        let badge_text_x = host_start + badge_padding;
        for (i, c) in info.host_label.chars().enumerate() {
            if !c.is_whitespace() {
                self.text_pipeline.push_char(c, badge_text_x + i as f32 * char_scale, text_y, accent);
            }
        }
        cx = host_start + badge_width + char_scale;

        // ── Profile name ──
        if info.profile != "default" {
            cx += char_scale * 0.5;
            self.text_pipeline.push_char('·', cx, text_y, dim_color);
            cx += char_scale * 1.5;

            for c in info.profile.chars() {
                if !c.is_whitespace() {
                    self.text_pipeline.push_char(c, cx, text_y, dim_color);
                }
                cx += char_scale;
            }
        }

        // ── Right side: recording indicator and Context button ──
        let right_margin = cell_width;

        // Recording dot (rightmost)
        if info.recording {
            let dot_x = window_width - right_margin - char_scale;
            let rec_color = [0.9, 0.15, 0.15, 1.0]; // red
            self.text_pipeline.push_char('●', dot_x, text_y, rec_color);

            // "REC" label
            let rec_label = "REC";
            let rec_x = dot_x - rec_label.len() as f32 * char_scale - char_scale * 0.5;
            for (i, c) in rec_label.chars().enumerate() {
                self.text_pipeline.push_char(c, rec_x + i as f32 * char_scale, text_y, rec_color);
            }
        }

        // Context button (before recording)
        let ctx_label = "[Context]";
        let ctx_width = ctx_label.chars().count() as f32 * char_scale;
        let ctx_x = if info.recording {
            window_width - right_margin - char_scale * 8.0 - ctx_width
        } else {
            window_width - right_margin - ctx_width
        };
        for (i, c) in ctx_label.chars().enumerate() {
            if !c.is_whitespace() {
                self.text_pipeline.push_char(c, ctx_x + i as f32 * char_scale, text_y, dim_color);
            }
        }
    }
}
