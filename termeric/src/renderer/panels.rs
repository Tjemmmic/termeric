//! Slide-in panel rendering — commands panel, session management panel

use super::{Renderer, adjust_alpha, mix};
use super::types::*;
use crate::ui::constants::*;
use crate::ui::commands::panel::{CommandsPanelInfo, CommandsPanelAction, EditMode};

impl Renderer {
    pub(super) fn build_commands_panel(&mut self, info: &crate::ui::commands::panel::CommandsPanelInfo, window_width: f32, window_height: f32) {
        use crate::ui::commands::panel::EditMode;

        let anim = info.animation;
        if anim <= 0.0 {
            return;
        }

        let ui_cw = self.cell_width;
        let cell_height = self.cell_height;

        // Panel dimensions
        let panel_width = (ui_cw * 36.0).max(340.0).min(window_width * 0.45);
        let panel_x = window_width - panel_width * anim;
        let padding = ui_cw * 1.8;
        let small_pad = ui_cw * 0.8;

        // Colors
        let text_color = self.ui.settings_text;
        let dim_color = self.ui.settings_text_dim;
        let accent = self.ui.settings_accent;
        let separator = self.ui.settings_separator;
        let hover_color = [
            self.ui.settings_selected[0],
            self.ui.settings_selected[1],
            self.ui.settings_selected[2],
            self.ui.settings_selected[3] * 0.35,
        ];

        // ── Scrim ──
        let scrim = self.ui.scrim;
        self.rect_pipeline.push_rect(
            0.0, 0.0, panel_x, window_height,
            [scrim[0], scrim[1], scrim[2], scrim[3] * anim],
        );

        // ── Panel background ──
        self.rect_pipeline.push_rect(panel_x, 0.0, panel_width, window_height, self.ui.settings_bg);

        // ── HEADER: " ⚡ Custom Commands                × " ──
        let header_height = cell_height * 2.8;
        let header_text_y = (header_height - cell_height) / 2.0;

        // Title with drawn lightning bolt icon (matches tab bar)
        let icon_cx = panel_x + padding + cell_height * 0.5;
        let icon_cy = header_height / 2.0;
        let icon_h = cell_height * 0.45;
        let icon_w = icon_h * 0.4;
        let icon_lw = 1.5;
        // Top segment: upper-right to center-left
        self.rect_pipeline.push_line(icon_cx + icon_w * 0.3, icon_cy - icon_h, icon_cx - icon_w * 0.5, icon_cy, icon_lw, accent);
        // Middle bar
        self.rect_pipeline.push_line(icon_cx - icon_w * 0.3, icon_cy, icon_cx + icon_w * 0.7, icon_cy, icon_lw, accent);
        // Bottom segment: center-right to lower-left
        self.rect_pipeline.push_line(icon_cx + icon_w * 0.5, icon_cy, icon_cx - icon_w * 0.3, icon_cy + icon_h, icon_lw, accent);

        let title = "Custom Commands";
        let tx = panel_x + padding + cell_height * 1.2;
        for (i, c) in title.chars().enumerate() {
            self.text_pipeline.push_char(c, tx + i as f32 * ui_cw, header_text_y, text_color);
        }

        // Close (×) button — drawn as two diagonal lines
        let close_x = panel_x + panel_width - padding - ui_cw;
        let close_cx = close_x + ui_cw * 0.5;
        let close_cy = header_height / 2.0;
        let close_arm = cell_height * 0.25;
        self.rect_pipeline.push_line(close_cx - close_arm, close_cy - close_arm, close_cx + close_arm, close_cy + close_arm, 1.5, dim_color);
        self.rect_pipeline.push_line(close_cx + close_arm, close_cy - close_arm, close_cx - close_arm, close_cy + close_arm, 1.5, dim_color);

        // Header separator
        self.rect_pipeline.push_rect(
            panel_x + padding, header_height,
            panel_width - padding * 2.0, 1.0, separator,
        );

        // ── "+ New Command" button ──
        let btn_y = header_height + cell_height * 0.8;
        let btn_height = cell_height * 2.0;
        let btn_width = panel_width - padding * 2.0;
        let btn_x = panel_x + padding;

        // Button background — accent outline with subtle fill on hover
        let btn_bg = if info.hovered_add {
            [accent[0], accent[1], accent[2], 0.15]
        } else {
            [accent[0], accent[1], accent[2], 0.06]
        };
        self.rect_pipeline.push_rounded_rect(btn_x, btn_y, btn_width, btn_height, btn_bg, 6.0);
        // Accent border
        let border_color = [accent[0], accent[1], accent[2], if info.hovered_add { 0.8 } else { 0.4 }];
        // Top/bottom borders
        self.rect_pipeline.push_rect(btn_x, btn_y, btn_width, 1.0, border_color);
        self.rect_pipeline.push_rect(btn_x, btn_y + btn_height - 1.0, btn_width, 1.0, border_color);
        // Left/right borders
        self.rect_pipeline.push_rect(btn_x, btn_y, 1.0, btn_height, border_color);
        self.rect_pipeline.push_rect(btn_x + btn_width - 1.0, btn_y, 1.0, btn_height, border_color);

        // Button text: "+ New Command"
        let btn_text = "+ New Command";
        let btn_text_w = btn_text.len() as f32 * ui_cw;
        let btn_text_x = btn_x + (btn_width - btn_text_w) / 2.0;
        let btn_text_y = btn_y + (btn_height - cell_height) / 2.0;
        let btn_text_color = if info.hovered_add { accent } else { [accent[0], accent[1], accent[2], 0.8] };
        for (i, c) in btn_text.chars().enumerate() {
            self.text_pipeline.push_char(c, btn_text_x + i as f32 * ui_cw, btn_text_y, btn_text_color);
        }

        // ── Inline form (shown when editing) ──
        let mut list_start_y = btn_y + btn_height + cell_height * 0.8;

        if info.edit_mode != EditMode::Browse {
            let form_x = panel_x + padding;
            let form_w = panel_width - padding * 2.0;
            let field_height = cell_height * 1.8;
            let label_w = ui_cw * 10.0;

            // Form background
            let form_bg = [accent[0], accent[1], accent[2], 0.05];
            let form_total_h = field_height * 2.0 + cell_height * 3.2;
            self.rect_pipeline.push_rounded_rect(form_x, list_start_y, form_w, form_total_h, form_bg, 6.0);

            let fy = list_start_y + small_pad;

            // "Name:" label + input
            let name_label = "Name:";
            let name_label_y = fy + (field_height - cell_height) / 2.0;
            for (i, c) in name_label.chars().enumerate() {
                self.text_pipeline.push_char(c, form_x + small_pad + i as f32 * ui_cw, name_label_y, dim_color);
            }
            // Name input box
            let input_x = form_x + label_w;
            let input_w = form_w - label_w - small_pad;
            let name_active = info.edit_mode == EditMode::EditingName;
            let input_border = if name_active { accent } else { separator };
            self.rect_pipeline.push_rounded_rect(input_x, fy, input_w, field_height, [0.0, 0.0, 0.0, 0.2], 4.0);
            self.rect_pipeline.push_rect(input_x, fy + field_height - 2.0, input_w, 2.0, input_border);
            // Name text
            let name_text_y = fy + (field_height - cell_height) / 2.0;
            for (i, c) in info.form_name.chars().enumerate() {
                self.text_pipeline.push_char(c, input_x + small_pad + i as f32 * ui_cw, name_text_y, text_color);
            }
            // Cursor
            if name_active {
                let cursor_x = input_x + small_pad + info.form_name[..info.form_cursor].chars().count() as f32 * ui_cw;
                self.rect_pipeline.push_rect(cursor_x, fy + 3.0, 1.5, field_height - 6.0, accent);
            }
            // Placeholder
            if info.form_name.is_empty() && !name_active {
                let ph = "e.g. SSH Production";
                for (i, c) in ph.chars().enumerate() {
                    self.text_pipeline.push_char(c, input_x + small_pad + i as f32 * ui_cw, name_text_y, [dim_color[0], dim_color[1], dim_color[2], dim_color[3] * 0.5]);
                }
            }

            // "Command:" label + input
            let cmd_fy = fy + field_height + small_pad;
            let cmd_label = "Command:";
            let cmd_label_y = cmd_fy + (field_height - cell_height) / 2.0;
            for (i, c) in cmd_label.chars().enumerate() {
                self.text_pipeline.push_char(c, form_x + small_pad + i as f32 * ui_cw, cmd_label_y, dim_color);
            }
            let cmd_active = info.edit_mode == EditMode::EditingCommand;
            let cmd_border = if cmd_active { accent } else { separator };
            self.rect_pipeline.push_rounded_rect(input_x, cmd_fy, input_w, field_height, [0.0, 0.0, 0.0, 0.2], 4.0);
            self.rect_pipeline.push_rect(input_x, cmd_fy + field_height - 2.0, input_w, 2.0, cmd_border);
            let cmd_text_y = cmd_fy + (field_height - cell_height) / 2.0;
            for (i, c) in info.form_command.chars().enumerate() {
                self.text_pipeline.push_char(c, input_x + small_pad + i as f32 * ui_cw, cmd_text_y, text_color);
            }
            if cmd_active {
                let cursor_x = input_x + small_pad + info.form_command[..info.form_cursor].chars().count() as f32 * ui_cw;
                self.rect_pipeline.push_rect(cursor_x, cmd_fy + 3.0, 1.5, field_height - 6.0, accent);
            }
            if info.form_command.is_empty() && !cmd_active {
                let ph = "e.g. ssh admin@prod.app.com";
                for (i, c) in ph.chars().enumerate() {
                    self.text_pipeline.push_char(c, input_x + small_pad + i as f32 * ui_cw, cmd_text_y, [dim_color[0], dim_color[1], dim_color[2], dim_color[3] * 0.5]);
                }
            }

            // Save / Cancel buttons
            let buttons_y = cmd_fy + field_height + small_pad * 1.5;
            let save_text = "Save";
            let save_w = ui_cw * 7.0;
            let save_x = form_x + form_w - save_w;
            let save_h = cell_height * 1.6;
            self.rect_pipeline.push_rounded_rect(save_x, buttons_y, save_w, save_h, accent, 4.0);
            let save_text_y = buttons_y + (save_h - cell_height) / 2.0;
            let save_text_x = save_x + (save_w - save_text.len() as f32 * ui_cw) / 2.0;
            for (i, c) in save_text.chars().enumerate() {
                self.text_pipeline.push_char(c, save_text_x + i as f32 * ui_cw, save_text_y, self.ui.settings_bg);
            }

            let cancel_text = "Cancel";
            let cancel_w = ui_cw * 8.0;
            let cancel_x = save_x - cancel_w - small_pad;
            self.rect_pipeline.push_rounded_rect(cancel_x, buttons_y, cancel_w, save_h, separator, 4.0);
            let cancel_text_y = buttons_y + (save_h - cell_height) / 2.0;
            let cancel_text_x = cancel_x + (cancel_w - cancel_text.len() as f32 * ui_cw) / 2.0;
            for (i, c) in cancel_text.chars().enumerate() {
                self.text_pipeline.push_char(c, cancel_text_x + i as f32 * ui_cw, cancel_text_y, text_color);
            }

            // Hint
            let hint = "Tab to switch fields, Enter to save";
            let hint_y = buttons_y + save_h + small_pad;
            let hx = form_x + small_pad;
            for (i, c) in hint.chars().enumerate() {
                self.text_pipeline.push_char(c, hx + i as f32 * ui_cw, hint_y, [dim_color[0], dim_color[1], dim_color[2], dim_color[3] * 0.5]);
            }

            list_start_y = list_start_y + form_total_h + cell_height * 0.5;
        }

        // ── Section label ──
        if !info.commands.is_empty() {
            let section_y = list_start_y;
            let section_label = if info.commands.len() == 1 {
                "1 command".to_string()
            } else {
                format!("{} commands", info.commands.len())
            };
            let sx = panel_x + padding;
            for (i, c) in section_label.chars().enumerate() {
                self.text_pipeline.push_char(c, sx + i as f32 * ui_cw, section_y, dim_color);
            }
            list_start_y = section_y + cell_height * 1.5;
        }

        // ── Command rows ──
        let row_height = cell_height * 3.0;

        if info.commands.is_empty() && info.edit_mode == EditMode::Browse {
            // Empty state
            let empty_y = list_start_y + cell_height;
            let msg = "No commands yet";
            let msg_w = msg.len() as f32 * ui_cw;
            let msg_x = panel_x + (panel_width - msg_w) / 2.0;
            for (i, c) in msg.chars().enumerate() {
                self.text_pipeline.push_char(c, msg_x + i as f32 * ui_cw, empty_y, dim_color);
            }
            let hint = "Click + New Command above to add one";
            let hint_w = hint.len() as f32 * ui_cw;
            let hint_x = panel_x + (panel_width - hint_w) / 2.0;
            let hint_y = empty_y + cell_height * 1.5;
            for (i, c) in hint.chars().enumerate() {
                self.text_pipeline.push_char(c, hint_x + i as f32 * ui_cw, hint_y, [dim_color[0], dim_color[1], dim_color[2], dim_color[3] * 0.5]);
            }
        }

        for (i, cmd) in info.commands.iter().enumerate() {
            let item_y = list_start_y + i as f32 * row_height;

            // Hover highlight (full row)
            if info.hovered_index == Some(i) && info.hovered_delete != Some(i) {
                self.rect_pipeline.push_rounded_rect(
                    panel_x + padding * 0.5, item_y,
                    panel_width - padding, row_height,
                    hover_color, 6.0,
                );
            }

            // Run indicator (left side) — drawn play triangle
            let run_color = if info.hovered_index == Some(i) { accent } else { dim_color };
            let tri_x = panel_x + padding;
            let tri_cy = item_y + row_height / 2.0;
            let tri_h = cell_height * 0.3;
            let tri_w = tri_h * 0.7;
            // Three lines forming a triangle: top-left to right-center, right-center to bottom-left, bottom-left to top-left
            self.rect_pipeline.push_line(tri_x, tri_cy - tri_h, tri_x + tri_w, tri_cy, 1.3, run_color);
            self.rect_pipeline.push_line(tri_x + tri_w, tri_cy, tri_x, tri_cy + tri_h, 1.3, run_color);
            self.rect_pipeline.push_line(tri_x, tri_cy + tri_h, tri_x, tri_cy - tri_h, 1.3, run_color);

            // Command name
            let name_x = panel_x + padding + ui_cw * 2.2;
            let name_y = item_y + cell_height * 0.5;
            let name: String = if cmd.name.chars().count() > 24 {
                let truncated: String = cmd.name.chars().take(23).collect();
                format!("{}…", truncated)
            } else {
                cmd.name.clone()
            };
            for (i, c) in name.chars().enumerate() {
                self.text_pipeline.push_char(c, name_x + i as f32 * ui_cw, name_y, text_color);
            }

            // Command text (second line, dimmed)
            let cmd_y = name_y + cell_height * 1.2;
            let cmd_text: String = if cmd.command.chars().count() > 32 {
                let truncated: String = cmd.command.chars().take(31).collect();
                format!("{}…", truncated)
            } else {
                cmd.command.clone()
            };
            for (i, c) in cmd_text.chars().enumerate() {
                self.text_pipeline.push_char(c, name_x + i as f32 * ui_cw, cmd_y, dim_color);
            }

            // Delete (x) button (right side)
            let del_x = panel_x + panel_width - padding - ui_cw * 1.5;
            let _del_y = item_y + (row_height - cell_height) / 2.0;
            let del_color = if info.hovered_delete == Some(i) {
                [0.9, 0.3, 0.3, 1.0]
            } else {
                [dim_color[0], dim_color[1], dim_color[2], dim_color[3] * 0.5]
            };
            // Delete button — drawn as two diagonal lines (X)
            let del_cx = del_x + ui_cw * 0.5;
            let del_cy = item_y + row_height / 2.0;
            let del_arm = cell_height * 0.22;
            self.rect_pipeline.push_line(del_cx - del_arm, del_cy - del_arm, del_cx + del_arm, del_cy + del_arm, 1.3, del_color);
            self.rect_pipeline.push_line(del_cx + del_arm, del_cy - del_arm, del_cx - del_arm, del_cy + del_arm, 1.3, del_color);

            // Row separator
            let sep_y = item_y + row_height - 0.5;
            self.rect_pipeline.push_rect(
                panel_x + padding + ui_cw * 2.0, sep_y,
                panel_width - padding * 2.0 - ui_cw * 2.0, 0.5, separator,
            );
        }
    }

    /// Hit-test the custom commands panel
    pub fn commands_panel_hit_test(
        &self,
        x: f32,
        y: f32,
        commands: &[crate::config::CustomCommand],
        window_width: f32,
        _window_height: f32,
        animation: f32,
        edit_mode: &crate::ui::commands::panel::EditMode,
    ) -> crate::ui::commands::panel::CommandsPanelAction {
        use crate::ui::commands::panel::{CommandsPanelAction, EditMode};

        let ui_cw = self.cell_width;
        let cell_height = self.cell_height;

        let panel_width = (ui_cw * 36.0).max(340.0).min(window_width * 0.45);
        let panel_x = window_width - panel_width * animation;
        let padding = ui_cw * 1.8;

        // Click outside panel = close
        if x < panel_x {
            return CommandsPanelAction::Close;
        }

        let header_height = cell_height * 2.8;
        let header_text_y = (header_height - cell_height) / 2.0;

        // Close (x) button
        let close_x = panel_x + panel_width - padding - ui_cw;
        if x >= close_x && y >= header_text_y && y <= header_text_y + cell_height * 1.5 {
            return CommandsPanelAction::Close;
        }

        // "+ New Command" button
        let btn_y = header_height + cell_height * 0.8;
        let btn_height = cell_height * 2.0;
        let btn_x = panel_x + padding;
        let btn_width = panel_width - padding * 2.0;
        if x >= btn_x && x <= btn_x + btn_width && y >= btn_y && y <= btn_y + btn_height {
            return CommandsPanelAction::AddSubmit {
                name: String::new(),
                command: String::new(),
            };
        }

        // Calculate list start
        let mut list_start_y = btn_y + btn_height + cell_height * 0.8;

        // Hit-test form fields if editing
        if *edit_mode != EditMode::Browse {
            let small_pad = ui_cw * 0.8;
            let field_height = cell_height * 1.8;
            let label_w = ui_cw * 10.0;
            let form_x = panel_x + padding;
            let form_w = panel_width - padding * 2.0;
            let form_total_h = field_height * 2.0 + cell_height * 3.2;

            let fy = list_start_y + small_pad;
            let input_x = form_x + label_w;
            let input_w = form_w - label_w - small_pad;

            // Name field click
            if x >= input_x && x <= input_x + input_w && y >= fy && y <= fy + field_height {
                return CommandsPanelAction::ClickName;
            }

            // Command field click
            let cmd_fy = fy + field_height + small_pad;
            if x >= input_x && x <= input_x + input_w && y >= cmd_fy && y <= cmd_fy + field_height {
                return CommandsPanelAction::ClickCommand;
            }

            // Save button
            let buttons_y = cmd_fy + field_height + small_pad * 1.5;
            let save_w = ui_cw * 7.0;
            let save_x = form_x + form_w - save_w;
            let save_h = cell_height * 1.6;
            if x >= save_x && x <= save_x + save_w && y >= buttons_y && y <= buttons_y + save_h {
                return CommandsPanelAction::AddSubmit {
                    name: "SAVE".to_string(),
                    command: "SAVE".to_string(),
                };
            }

            // Cancel button
            let cancel_w = ui_cw * 8.0;
            let cancel_x = save_x - cancel_w - small_pad;
            if x >= cancel_x && x <= cancel_x + cancel_w && y >= buttons_y && y <= buttons_y + save_h {
                return CommandsPanelAction::Close;
            }

            list_start_y = list_start_y + form_total_h + cell_height * 0.5;
        }

        // Skip section label
        if !commands.is_empty() {
            list_start_y += cell_height * 1.5;
        }

        // Command rows
        let row_height = cell_height * 3.0;
        for i in 0..commands.len() {
            let item_y = list_start_y + i as f32 * row_height;
            if y >= item_y && y < item_y + row_height {
                // Delete (x) button
                let del_x = panel_x + panel_width - padding - ui_cw * 1.5;
                if x >= del_x - ui_cw * 0.5 && x <= del_x + ui_cw * 1.5 {
                    return CommandsPanelAction::Delete(i);
                }
                // Click row = Run
                return CommandsPanelAction::Run(i);
            }
        }

        CommandsPanelAction::None
    }

    pub(super) fn build_session_management_panel(&mut self, info: &crate::session::panel::SessionPanelInfo, window_width: f32, window_height: f32) {
        let anim = info.animation;
        if anim <= 0.0 {
            return;
        }

        let ui_cw = self.cell_width;
        let cell_height = self.cell_height;

        // Panel dimensions
        let panel_width = (ui_cw * 35.0).max(320.0).min(window_width * 0.45);
        let panel_x = window_width - panel_width * anim;
        let panel_h = window_height;
        let row_height = (cell_height * 3.5).max(52.0);
        let padding_x = ui_cw * 1.5;
        let padding_y = cell_height * 0.5;

        // ── Scrim ──
        let scrim = self.ui.scrim;
        self.rect_pipeline.push_rect(
            0.0, 0.0,
            panel_x, window_height,
            [scrim[0], scrim[1], scrim[2], scrim[3] * anim],
        );

        // ── Panel background ──
        self.rect_pipeline.push_rect(
            panel_x, 0.0,
            panel_width, panel_h,
            self.ui.settings_bg,
        );

        // ── Left border ──
        self.rect_pipeline.push_rect(
            panel_x, 0.0,
            2.0, panel_h,
            self.ui.settings_accent,
        );

        // ── Title ──
        let title = "Sessions";
        let title_y = cell_height * 1.5;
        let title_x = panel_x + padding_x;
        for (i, c) in title.chars().enumerate() {
            self.text_pipeline.push_char(
                c,
                title_x + i as f32 * ui_cw,
                title_y,
                self.ui.settings_accent,
            );
        }

        // ── Close button (×) ──
        let close_x = panel_x + panel_width - padding_x - ui_cw;
        self.text_pipeline.push_char(
            '×',
            close_x,
            title_y,
            self.ui.settings_text,
        );

        // ── Separator under title ──
        let sep_y = title_y + cell_height * 2.0;
        self.rect_pipeline.push_rect(
            panel_x + padding_x, sep_y,
            panel_width - padding_x * 2.0, 1.0,
            self.ui.settings_separator,
        );

        // ── Session list ──
        let list_start_y = sep_y + padding_y + 4.0;
        let text_color = self.ui.settings_text;
        let dim_color = self.ui.settings_text_dim;
        let accent_color = self.ui.settings_accent;
        let btn_color = self.ui.settings_separator;

        if info.sessions.is_empty() {
            // Show "No saved sessions" placeholder
            let placeholder = "No saved sessions";
            let ph_y = list_start_y + row_height * 0.5;
            let mut px = panel_x + padding_x;
            for c in placeholder.chars() {
                if !c.is_whitespace() {
                    self.text_pipeline.push_char(c, px, ph_y, dim_color);
                }
                px += ui_cw * 0.85;
            }
        }

        for (i, session) in info.sessions.iter().enumerate() {
            let item_y = list_start_y + i as f32 * row_height;

            // Hover highlight for session rows
            if info.hovered_index == Some(i) {
                let hover_color = [
                    self.ui.settings_selected[0],
                    self.ui.settings_selected[1],
                    self.ui.settings_selected[2],
                    self.ui.settings_selected[3] * 0.4,
                ];
                self.rect_pipeline.push_rounded_rect(
                    panel_x + padding_x * 0.5, item_y,
                    panel_width - padding_x, row_height,
                    hover_color,
                    4.0,
                );
            }

            // Session icon
            let icon_y = item_y + padding_y;
            self.text_pipeline.push_char('◆', panel_x + padding_x, icon_y, accent_color);

            // Session display name
            let name_x = panel_x + padding_x + ui_cw * 2.0;
            let name: String = if session.display_name.chars().count() > 22 {
                let truncated: String = session.display_name.chars().take(21).collect();
                format!("{}…", truncated)
            } else {
                session.display_name.clone()
            };
            for (j, c) in name.chars().enumerate() {
                if !c.is_whitespace() {
                    self.text_pipeline.push_char(
                        c,
                        name_x + j as f32 * ui_cw * 0.85,
                        icon_y,
                        text_color,
                    );
                }
            }

            // Metadata line: "N tabs · Xm ago"
            let meta = format!("{} tab{} · {}", session.tab_count,
                if session.tab_count == 1 { "" } else { "s" },
                session.last_modified);
            let meta_y = icon_y + cell_height * 1.3;
            let mut mx = name_x;
            for c in meta.chars() {
                if !c.is_whitespace() {
                    self.text_pipeline.push_char(c, mx, meta_y, dim_color);
                }
                mx += ui_cw * 0.8;
            }

            // ── Action buttons: [Load] [Delete] ──
            let btn_height = cell_height * 1.5;
            let btn_y = icon_y + cell_height * 0.1;

            // Delete button (right-aligned)
            let delete_text = "Delete";
            let delete_w = ui_cw * (delete_text.len() as f32 + 2.0);
            let delete_x = panel_x + panel_width - padding_x - delete_w;
            self.rect_pipeline.push_rect(
                delete_x, btn_y,
                delete_w, btn_height,
                [0.6, 0.2, 0.2, 0.5],
            );
            for (j, c) in delete_text.chars().enumerate() {
                self.text_pipeline.push_char(
                    c,
                    delete_x + ui_cw * 1.0 + j as f32 * ui_cw * 0.85,
                    btn_y + (btn_height - cell_height) * 0.5,
                    text_color,
                );
            }

            // Load button (left of delete)
            let load_text = "Load";
            let load_w = ui_cw * (load_text.len() as f32 + 2.0);
            let load_x = delete_x - load_w - ui_cw * 0.5;
            self.rect_pipeline.push_rect(
                load_x, btn_y,
                load_w, btn_height,
                btn_color,
            );
            for (j, c) in load_text.chars().enumerate() {
                self.text_pipeline.push_char(
                    c,
                    load_x + ui_cw * 1.0 + j as f32 * ui_cw * 0.85,
                    btn_y + (btn_height - cell_height) * 0.5,
                    text_color,
                );
            }

            // Row separator
            let row_sep_y = item_y + row_height - 1.0;
            self.rect_pipeline.push_rect(
                panel_x + padding_x, row_sep_y,
                panel_width - padding_x * 2.0, 1.0,
                self.ui.settings_separator,
            );
        }
    }

    /// Hit-test the session management panel for click handling
    pub fn session_panel_hit_test(
        &self,
        x: f32,
        y: f32,
        sessions: &[crate::session::panel::SessionEntry],
        window_width: f32,
        _window_height: f32,
        animation: f32,
    ) -> crate::session::panel::SessionPanelAction {
        use crate::session::panel::SessionPanelAction;

        let ui_cw = self.cell_width;
        let cell_height = self.cell_height;

        let panel_width = (ui_cw * 35.0).max(320.0).min(window_width * 0.45);
        let panel_x = window_width - panel_width * animation;
        let padding_x = ui_cw * 1.5;
        let padding_y = cell_height * 0.5;
        let row_height = (cell_height * 3.5).max(52.0);

        // Click outside panel → close
        if x < panel_x {
            return SessionPanelAction::Close;
        }

        // Close button (× in top-right)
        let title_y = cell_height * 1.5;
        let close_x = panel_x + panel_width - padding_x - ui_cw;
        if x >= close_x && x <= close_x + ui_cw * 1.5
            && y >= title_y && y <= title_y + cell_height * 1.5
        {
            return SessionPanelAction::Close;
        }

        // Session list area
        let sep_y = title_y + cell_height * 2.0;
        let list_start_y = sep_y + padding_y + 4.0;

        for (i, session) in sessions.iter().enumerate() {
            let item_y = list_start_y + i as f32 * row_height;
            if y < item_y || y > item_y + row_height {
                continue;
            }

            let btn_height = cell_height * 1.5;
            let btn_y = item_y + padding_y + cell_height * 0.1;

            // Check if click is in the button row area
            if y >= btn_y && y <= btn_y + btn_height {
                // Delete button
                let delete_text = "Delete";
                let delete_w = ui_cw * (delete_text.len() as f32 + 2.0);
                let delete_x = panel_x + panel_width - padding_x - delete_w;
                if x >= delete_x && x <= delete_x + delete_w {
                    return SessionPanelAction::Delete(session.id.clone());
                }

                // Load button
                let load_text = "Load";
                let load_w = ui_cw * (load_text.len() as f32 + 2.0);
                let load_x = delete_x - load_w - ui_cw * 0.5;
                if x >= load_x && x <= load_x + load_w {
                    return SessionPanelAction::Load(session.id.clone());
                }
            }

            // Clicking anywhere else on the session row → load it
            return SessionPanelAction::Load(session.id.clone());
        }

        SessionPanelAction::None
    }
}
