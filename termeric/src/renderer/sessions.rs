//! Session dropdown rendering and hit-testing

use super::Renderer;
use super::types::*;

impl Renderer {
    pub(super) fn build_session_dropdown(&mut self, info: &SessionDropdownInfo, window_width: f32) {
        let tab_bar_height = self.tab_bar_height();
        let cell_width = self.cell_width;
        let cell_height = self.cell_height;

        // Position dropdown below the ▾ button (left of gear)
        let gear_button_width = tab_bar_height;
        let session_button_width = tab_bar_height;
        let dropdown_anchor_x = window_width - gear_button_width - session_button_width;

        // Dropdown dimensions
        let row_height = (cell_height * 2.0).max(28.0);
        let padding_x = cell_width * 1.5;
        let dropdown_width = (cell_width * 22.0).max(200.0);
        let dropdown_x = (dropdown_anchor_x + session_button_width - dropdown_width).max(0.0);
        let dropdown_y = tab_bar_height;

        // Items: at least 1 row (placeholder if empty) + manager action.
        let session_count = info.sessions.len().min(SESSION_DROPDOWN_MAX_ITEMS);
        let display_rows = session_count.max(1); // at least 1 row for placeholder
        let extra_items = 1; // Manage Sessions
        let total_items = display_rows + extra_items;
        let dropdown_height = total_items as f32 * row_height + 4.0; // 4px padding

        // Background
        let bg_color = self.ui.settings_bg;
        self.rect_pipeline.push_rounded_rect(
            dropdown_x,
            dropdown_y,
            dropdown_width,
            dropdown_height,
            bg_color,
            8.0,
        );

        // Border
        let border_color = self.ui.settings_separator;
        self.rect_pipeline.push_rounded_rect(
            dropdown_x - 1.0,
            dropdown_y - 1.0,
            dropdown_width + 2.0,
            dropdown_height + 2.0,
            border_color,
            8.0,
        );
        // Re-draw bg on top of border to create border effect
        self.rect_pipeline.push_rounded_rect(
            dropdown_x,
            dropdown_y,
            dropdown_width,
            dropdown_height,
            bg_color,
            8.0,
        );

        let text_color = self.ui.settings_text;
        let dim_text_color = self.ui.settings_text_dim;
        let accent_color = self.ui.settings_accent;

        // Render session items (or placeholder if none)
        if info.sessions.is_empty() {
            let item_y = dropdown_y + 2.0;
            let text_y = item_y + (row_height - cell_height) / 2.0;
            let mut text_x = dropdown_x + padding_x;
            for c in "No legacy snapshots".chars() {
                if text_x + cell_width > dropdown_x + dropdown_width - padding_x {
                    break;
                }
                if !c.is_whitespace() {
                    self.text_pipeline
                        .push_char(c, text_x, text_y, dim_text_color);
                }
                text_x += cell_width * 0.85;
            }
        }

        for (i, (_, display_title)) in info
            .sessions
            .iter()
            .take(SESSION_DROPDOWN_MAX_ITEMS)
            .enumerate()
        {
            let item_y = dropdown_y + 2.0 + i as f32 * row_height;

            // Hover highlight
            if info.hovered_index == Some(i) {
                let hover_color = [
                    self.ui.settings_selected[0],
                    self.ui.settings_selected[1],
                    self.ui.settings_selected[2],
                    self.ui.settings_selected[3] * 0.4,
                ];
                self.rect_pipeline.push_rounded_rect(
                    dropdown_x + 4.0,
                    item_y,
                    dropdown_width - 8.0,
                    row_height,
                    hover_color,
                    4.0,
                );
            }

            let text_y = item_y + (row_height - cell_height) / 2.0;
            let mut text_x = dropdown_x + padding_x;

            // Session icon
            self.text_pipeline
                .push_char('◆', text_x - cell_width * 0.3, text_y, accent_color);
            text_x += cell_width;

            // Display session title, truncate if needed
            let display_name: String = if display_title.chars().count() > 18 {
                let truncated: String = display_title.chars().take(17).collect();
                format!("{}…", truncated)
            } else {
                display_title.clone()
            };
            for c in display_name.chars() {
                if text_x + cell_width > dropdown_x + dropdown_width - padding_x {
                    break;
                }
                if !c.is_whitespace() {
                    self.text_pipeline.push_char(c, text_x, text_y, text_color);
                }
                text_x += cell_width * 0.85;
            }
        }

        // Separator before action items
        let display_rows = session_count.max(1); // match build logic
        let sep_y = dropdown_y + 2.0 + display_rows as f32 * row_height;
        self.rect_pipeline.push_rect(
            dropdown_x + padding_x,
            sep_y,
            dropdown_width - padding_x * 2.0,
            1.0,
            self.ui.settings_separator,
        );

        // Snapshot-manager item
        let manage_y = sep_y + 1.0;

        // Hover highlight for the snapshot-manager item.
        let manage_row_idx = display_rows;
        if info.hovered_index == Some(manage_row_idx) {
            let hover_color = [
                self.ui.settings_selected[0],
                self.ui.settings_selected[1],
                self.ui.settings_selected[2],
                self.ui.settings_selected[3] * 0.4,
            ];
            self.rect_pipeline.push_rounded_rect(
                dropdown_x + 4.0,
                manage_y,
                dropdown_width - 8.0,
                row_height,
                hover_color,
                4.0,
            );
        }

        let manage_text_y = manage_y + (row_height - cell_height) / 2.0;
        let mut manage_x = dropdown_x + padding_x;
        for c in "Manage Snapshots…".chars() {
            if !c.is_whitespace() {
                self.text_pipeline
                    .push_char(c, manage_x, manage_text_y, dim_text_color);
            }
            manage_x += cell_width * 0.85;
        }
    }

    /// Hit-test the session dropdown to determine what was clicked
    pub fn session_dropdown_hit_test(
        &self,
        x: f32,
        y: f32,
        sessions: &[(String, String)],
        window_width: f32,
    ) -> SessionDropdownClick {
        let tab_bar_height = self.tab_bar_height();
        let cell_width = self.cell_width;
        let cell_height = self.cell_height;

        let gear_button_width = tab_bar_height;
        let session_button_width = tab_bar_height;
        let dropdown_anchor_x = window_width - gear_button_width - session_button_width;

        let row_height = (cell_height * 2.0).max(28.0);
        let dropdown_width = (cell_width * 22.0).max(200.0);
        let dropdown_x = (dropdown_anchor_x + session_button_width - dropdown_width).max(0.0);
        let dropdown_y = tab_bar_height;

        let session_count = sessions.len().min(SESSION_DROPDOWN_MAX_ITEMS);
        let display_rows = session_count.max(1); // match build logic
        let extra_items = 1;
        let total_items = display_rows + extra_items;
        let dropdown_height = total_items as f32 * row_height + 4.0;

        // Check if click is within dropdown bounds
        if x < dropdown_x
            || x > dropdown_x + dropdown_width
            || y < dropdown_y
            || y > dropdown_y + dropdown_height
        {
            return SessionDropdownClick::Dismiss;
        }

        // Determine which row was clicked
        let relative_y = y - dropdown_y - 2.0;
        let row = (relative_y / row_height) as usize;

        if row < display_rows {
            // Clicked on a session row (or placeholder if empty)
            if let Some((session_id, _)) = sessions.get(row) {
                return SessionDropdownClick::LoadSession(session_id.clone());
            }
            // Placeholder row — do nothing
            return SessionDropdownClick::None;
        } else if row == display_rows {
            return SessionDropdownClick::ManageSessions;
        }

        SessionDropdownClick::None
    }
}
