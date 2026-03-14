//! Git branch completion dropdown rendering

use super::Renderer;
use super::types::*;

impl Renderer {
    pub(super) fn build_git_completions(&mut self, info: &GitCompletionInfo, cursor_pos: (f32, f32), window_width: f32, window_height: f32) {
        if info.completions.is_empty() {
            return;
        }

        let ui_cw = self.cell_width;
        let cell_h = self.cell_height;
        let padding = self.padding;
        let tab_bar_h = self.tab_bar_height();

        let accent = self.ui.settings_accent;
        let text_color = self.ui.settings_text;
        let bg = self.ui.settings_bg;
        let separator = self.ui.settings_separator;

        // Position: below the cursor
        let (cursor_col, cursor_row) = cursor_pos;
        let dropdown_x = padding + cursor_col * ui_cw;
        let dropdown_top = tab_bar_h + padding + (cursor_row + 1.0) * cell_h;

        // Sizing
        let max_visible = 8usize;
        let visible_count = info.completions.len().min(max_visible);
        let row_height = cell_h * 1.6;
        let dropdown_height = visible_count as f32 * row_height + 4.0; // +4 for top/bottom padding

        // Width: based on longest branch name (min 24 chars, max 50% of window)
        let max_len = info.completions.iter().map(|s| s.chars().count()).max().unwrap_or(10);
        let dropdown_width = ((max_len + 4) as f32 * ui_cw).max(ui_cw * 24.0).min(window_width * 0.5);

        // Ensure dropdown fits on screen
        let dropdown_x = dropdown_x.min(window_width - dropdown_width - padding);
        let dropdown_top = if dropdown_top + dropdown_height > window_height {
            // Flip above cursor if it goes off-screen
            tab_bar_h + padding + cursor_row * cell_h - dropdown_height
        } else {
            dropdown_top
        };

        // Background with slight transparency
        self.rect_pipeline.push_rounded_rect(
            dropdown_x, dropdown_top,
            dropdown_width, dropdown_height,
            [bg[0], bg[1], bg[2], 0.95],
            6.0,
        );

        // Border
        let border_color = [accent[0], accent[1], accent[2], 0.3];
        self.rect_pipeline.push_rect(dropdown_x, dropdown_top, dropdown_width, 1.0, border_color);
        self.rect_pipeline.push_rect(dropdown_x, dropdown_top + dropdown_height - 1.0, dropdown_width, 1.0, border_color);
        self.rect_pipeline.push_rect(dropdown_x, dropdown_top, 1.0, dropdown_height, border_color);
        self.rect_pipeline.push_rect(dropdown_x + dropdown_width - 1.0, dropdown_top, 1.0, dropdown_height, border_color);

        // Git branch icon header
        let header_text = " branches";
        let git_icon_x = dropdown_x + ui_cw * 0.8;
        let header_y = dropdown_top + 2.0 + (row_height - cell_h) / 2.0;

        // Draw small branch icon (fork shape)
        let icon_cx = git_icon_x + ui_cw * 0.5;
        let icon_cy = header_y + cell_h * 0.5;
        let icon_arm = cell_h * 0.25;
        // Main line (vertical)
        self.rect_pipeline.push_rect(icon_cx, icon_cy - icon_arm, 1.3, icon_arm * 2.0, accent);
        // Branch line (diagonal from top-right to center)
        self.rect_pipeline.push_line(
            icon_cx + icon_arm * 0.7, icon_cy - icon_arm * 0.8,
            icon_cx, icon_cy - icon_arm * 0.1,
            1.2, accent,
        );

        for (i, c) in header_text.chars().enumerate() {
            self.text_pipeline.push_char(
                c,
                git_icon_x + ui_cw * 1.5 + i as f32 * ui_cw,
                header_y,
                [accent[0], accent[1], accent[2], 0.7],
            );
        }

        // Separator after header
        let sep_y = dropdown_top + 2.0 + row_height;
        self.rect_pipeline.push_rect(
            dropdown_x + ui_cw * 0.5, sep_y,
            dropdown_width - ui_cw, 0.5, separator,
        );

        // Scroll offset (keep selected item in view)
        let selected = info.selected_index.unwrap_or(0);
        let scroll_offset = if selected >= max_visible - 1 {
            (selected + 1).saturating_sub(max_visible - 1)
        } else {
            0
        };

        // Draw branch items
        let items_start_y = sep_y + 2.0;
        let items_to_show = if info.completions.len() > max_visible {
            max_visible - 1 // Leave room for "..." indicator
        } else {
            visible_count
        };

        for (vi, idx) in (scroll_offset..info.completions.len())
            .take(items_to_show)
            .enumerate()
        {
            let branch = &info.completions[idx];
            let item_y = items_start_y + vi as f32 * row_height;
            let text_y = item_y + (row_height - cell_h) / 2.0;
            let is_selected = info.selected_index == Some(idx);

            // Selected highlight
            if is_selected {
                self.rect_pipeline.push_rounded_rect(
                    dropdown_x + 2.0, item_y,
                    dropdown_width - 4.0, row_height,
                    [accent[0], accent[1], accent[2], 0.15],
                    4.0,
                );
            }

            // Branch name
            let color = if is_selected { accent } else { text_color };
            for (j, c) in branch.chars().enumerate() {
                if j as f32 * ui_cw > dropdown_width - ui_cw * 3.0 {
                    // Truncate with ellipsis
                    self.text_pipeline.push_char(
                        '…', dropdown_x + ui_cw + j as f32 * ui_cw, text_y, color,
                    );
                    break;
                }
                self.text_pipeline.push_char(
                    c, dropdown_x + ui_cw + j as f32 * ui_cw, text_y, color,
                );
            }
        }

        // Overflow indicator
        if info.completions.len() > max_visible {
            let more_count = info.completions.len() - items_to_show - scroll_offset;
            if more_count > 0 {
                let more_text = format!("… +{} more", more_count);
                let more_y = items_start_y + items_to_show as f32 * row_height + (row_height - cell_h) / 2.0;
                for (j, c) in more_text.chars().enumerate() {
                    self.text_pipeline.push_char(
                        c,
                        dropdown_x + ui_cw + j as f32 * ui_cw,
                        more_y,
                        [text_color[0], text_color[1], text_color[2], 0.5],
                    );
                }
            }
        }
    }
}
