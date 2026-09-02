//! Slide-in panel rendering — commands panel, session management panel

use super::Renderer;

impl Renderer {
    pub(super) fn build_commands_panel(
        &mut self,
        info: &crate::ui::commands::panel::CommandsPanelInfo,
        window_width: f32,
        window_height: f32,
    ) {
        use crate::ui::commands::panel::{CommandsPanelLayout, EditMode};

        let anim = info.animation;
        if anim <= 0.0 {
            return;
        }

        let ui_cw = self.cell_width;
        let cell_height = self.cell_height;

        let layout = CommandsPanelLayout::calculate(
            window_width,
            window_height,
            ui_cw,
            cell_height,
            anim,
            info.edit_mode != EditMode::Browse,
            info.commands.len(),
        );
        let panel_width = layout.panel.width;
        let panel_x = layout.panel.x;
        let padding = layout.padding;
        let small_pad = layout.small_padding;

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
            0.0,
            0.0,
            panel_x,
            window_height,
            [scrim[0], scrim[1], scrim[2], scrim[3] * anim],
        );

        // ── Panel background ──
        self.rect_pipeline.push_rect(
            panel_x,
            0.0,
            panel_width,
            window_height,
            self.ui.settings_bg,
        );

        // ── HEADER: " ⚡ Custom Commands                × " ──
        let header_height = layout.header_height;
        let header_text_y = (header_height - cell_height) / 2.0;

        // Title with drawn lightning bolt icon (matches tab bar)
        let icon_cx = panel_x + padding + cell_height * 0.5;
        let icon_cy = header_height / 2.0;
        let icon_h = cell_height * 0.45;
        let icon_w = icon_h * 0.4;
        let icon_lw = 1.5;
        // Top segment: upper-right to center-left
        self.rect_pipeline.push_line(
            icon_cx + icon_w * 0.3,
            icon_cy - icon_h,
            icon_cx - icon_w * 0.5,
            icon_cy,
            icon_lw,
            accent,
        );
        // Middle bar
        self.rect_pipeline.push_line(
            icon_cx - icon_w * 0.3,
            icon_cy,
            icon_cx + icon_w * 0.7,
            icon_cy,
            icon_lw,
            accent,
        );
        // Bottom segment: center-right to lower-left
        self.rect_pipeline.push_line(
            icon_cx + icon_w * 0.5,
            icon_cy,
            icon_cx - icon_w * 0.3,
            icon_cy + icon_h,
            icon_lw,
            accent,
        );

        let title = "Custom Commands";
        let tx = panel_x + padding + cell_height * 1.2;
        for (i, c) in title.chars().enumerate() {
            self.text_pipeline
                .push_char(c, tx + i as f32 * ui_cw, header_text_y, text_color);
        }

        // Close (×) button — drawn as two diagonal lines
        let close_x = layout.close_button.x;
        let close_cx = close_x + ui_cw * 0.5;
        let close_cy = header_height / 2.0;
        let close_arm = cell_height * 0.25;
        self.rect_pipeline.push_line(
            close_cx - close_arm,
            close_cy - close_arm,
            close_cx + close_arm,
            close_cy + close_arm,
            1.5,
            dim_color,
        );
        self.rect_pipeline.push_line(
            close_cx + close_arm,
            close_cy - close_arm,
            close_cx - close_arm,
            close_cy + close_arm,
            1.5,
            dim_color,
        );

        // Header separator
        self.rect_pipeline.push_rect(
            panel_x + padding,
            header_height,
            panel_width - padding * 2.0,
            1.0,
            separator,
        );

        // ── "+ New Command" button ──
        let btn_y = layout.new_button.y;
        let btn_height = layout.new_button.height;
        let btn_width = layout.new_button.width;
        let btn_x = layout.new_button.x;

        // Button background — accent outline with subtle fill on hover
        let btn_bg = if info.hovered_add {
            [accent[0], accent[1], accent[2], 0.15]
        } else {
            [accent[0], accent[1], accent[2], 0.06]
        };
        self.rect_pipeline
            .push_rounded_rect(btn_x, btn_y, btn_width, btn_height, btn_bg, 6.0);
        // Accent border
        let border_color = [
            accent[0],
            accent[1],
            accent[2],
            if info.hovered_add { 0.8 } else { 0.4 },
        ];
        // Top/bottom borders
        self.rect_pipeline
            .push_rect(btn_x, btn_y, btn_width, 1.0, border_color);
        self.rect_pipeline.push_rect(
            btn_x,
            btn_y + btn_height - 1.0,
            btn_width,
            1.0,
            border_color,
        );
        // Left/right borders
        self.rect_pipeline
            .push_rect(btn_x, btn_y, 1.0, btn_height, border_color);
        self.rect_pipeline.push_rect(
            btn_x + btn_width - 1.0,
            btn_y,
            1.0,
            btn_height,
            border_color,
        );

        // Button text: "+ New Command"
        let btn_text = "+ New Command";
        let btn_text_w = btn_text.len() as f32 * ui_cw;
        let btn_text_x = btn_x + (btn_width - btn_text_w) / 2.0;
        let btn_text_y = btn_y + (btn_height - cell_height) / 2.0;
        let btn_text_color = if info.hovered_add {
            accent
        } else {
            [accent[0], accent[1], accent[2], 0.8]
        };
        for (i, c) in btn_text.chars().enumerate() {
            self.text_pipeline.push_char(
                c,
                btn_text_x + i as f32 * ui_cw,
                btn_text_y,
                btn_text_color,
            );
        }

        // ── Inline form (shown when editing) ──
        let mut list_start_y = btn_y + btn_height + cell_height * 0.8;

        if info.edit_mode != EditMode::Browse {
            let form = layout.form.expect("editing layout includes a form");
            let form_x = form.container.x;
            let form_w = form.container.width;
            let field_height = form.name_input.height;

            // Form background
            let form_bg = [accent[0], accent[1], accent[2], 0.05];
            let form_total_h = form.container.height;
            self.rect_pipeline.push_rounded_rect(
                form_x,
                list_start_y,
                form_w,
                form_total_h,
                form_bg,
                6.0,
            );

            let fy = form.name_input.y;

            // "Name:" label + input
            let name_label = "Name:";
            let name_label_y = fy + (field_height - cell_height) / 2.0;
            for (i, c) in name_label.chars().enumerate() {
                self.text_pipeline.push_char(
                    c,
                    form_x + small_pad + i as f32 * ui_cw,
                    name_label_y,
                    dim_color,
                );
            }
            // Name input box
            let input_x = form.name_input.x;
            let input_w = form.name_input.width;
            let name_active = info.edit_mode == EditMode::EditingName;
            let input_border = if name_active { accent } else { separator };
            self.rect_pipeline.push_rounded_rect(
                input_x,
                fy,
                input_w,
                field_height,
                [0.0, 0.0, 0.0, 0.2],
                4.0,
            );
            self.rect_pipeline.push_rect(
                input_x,
                fy + field_height - 2.0,
                input_w,
                2.0,
                input_border,
            );
            // Name text
            let name_text_y = fy + (field_height - cell_height) / 2.0;
            for (i, c) in info.form_name.chars().enumerate() {
                self.text_pipeline.push_char(
                    c,
                    input_x + small_pad + i as f32 * ui_cw,
                    name_text_y,
                    text_color,
                );
            }
            // Cursor
            if name_active {
                let cursor_x = input_x
                    + small_pad
                    + info.form_name[..info.form_cursor].chars().count() as f32 * ui_cw;
                self.rect_pipeline
                    .push_rect(cursor_x, fy + 3.0, 1.5, field_height - 6.0, accent);
            }
            // Placeholder
            if info.form_name.is_empty() && !name_active {
                let ph = "e.g. SSH Production";
                for (i, c) in ph.chars().enumerate() {
                    self.text_pipeline.push_char(
                        c,
                        input_x + small_pad + i as f32 * ui_cw,
                        name_text_y,
                        [dim_color[0], dim_color[1], dim_color[2], dim_color[3] * 0.5],
                    );
                }
            }

            // "Command:" label + input
            let cmd_fy = form.command_input.y;
            let cmd_label = "Command:";
            let cmd_label_y = cmd_fy + (field_height - cell_height) / 2.0;
            for (i, c) in cmd_label.chars().enumerate() {
                self.text_pipeline.push_char(
                    c,
                    form_x + small_pad + i as f32 * ui_cw,
                    cmd_label_y,
                    dim_color,
                );
            }
            let cmd_active = info.edit_mode == EditMode::EditingCommand;
            let cmd_border = if cmd_active { accent } else { separator };
            self.rect_pipeline.push_rounded_rect(
                input_x,
                cmd_fy,
                input_w,
                field_height,
                [0.0, 0.0, 0.0, 0.2],
                4.0,
            );
            self.rect_pipeline.push_rect(
                input_x,
                cmd_fy + field_height - 2.0,
                input_w,
                2.0,
                cmd_border,
            );
            let cmd_text_y = cmd_fy + (field_height - cell_height) / 2.0;
            for (i, c) in info.form_command.chars().enumerate() {
                self.text_pipeline.push_char(
                    c,
                    input_x + small_pad + i as f32 * ui_cw,
                    cmd_text_y,
                    text_color,
                );
            }
            if cmd_active {
                let cursor_x = input_x
                    + small_pad
                    + info.form_command[..info.form_cursor].chars().count() as f32 * ui_cw;
                self.rect_pipeline.push_rect(
                    cursor_x,
                    cmd_fy + 3.0,
                    1.5,
                    field_height - 6.0,
                    accent,
                );
            }
            if info.form_command.is_empty() && !cmd_active {
                let ph = "e.g. ssh admin@prod.app.com";
                for (i, c) in ph.chars().enumerate() {
                    self.text_pipeline.push_char(
                        c,
                        input_x + small_pad + i as f32 * ui_cw,
                        cmd_text_y,
                        [dim_color[0], dim_color[1], dim_color[2], dim_color[3] * 0.5],
                    );
                }
            }

            // Save / Cancel buttons
            let buttons_y = form.save_button.y;
            let save_text = "Save";
            let save_w = form.save_button.width;
            let save_x = form.save_button.x;
            let save_h = form.save_button.height;
            self.rect_pipeline
                .push_rounded_rect(save_x, buttons_y, save_w, save_h, accent, 4.0);
            let save_text_y = buttons_y + (save_h - cell_height) / 2.0;
            let save_text_x = save_x + (save_w - save_text.len() as f32 * ui_cw) / 2.0;
            for (i, c) in save_text.chars().enumerate() {
                self.text_pipeline.push_char(
                    c,
                    save_text_x + i as f32 * ui_cw,
                    save_text_y,
                    self.ui.settings_bg,
                );
            }

            let cancel_text = "Cancel";
            let cancel_w = form.cancel_button.width;
            let cancel_x = form.cancel_button.x;
            self.rect_pipeline
                .push_rounded_rect(cancel_x, buttons_y, cancel_w, save_h, separator, 4.0);
            let cancel_text_y = buttons_y + (save_h - cell_height) / 2.0;
            let cancel_text_x = cancel_x + (cancel_w - cancel_text.len() as f32 * ui_cw) / 2.0;
            for (i, c) in cancel_text.chars().enumerate() {
                self.text_pipeline.push_char(
                    c,
                    cancel_text_x + i as f32 * ui_cw,
                    cancel_text_y,
                    text_color,
                );
            }

            // Hint
            let hint = "Tab to switch fields, Enter to save";
            let hint_y = form.hint_y;
            let hx = form_x + small_pad;
            for (i, c) in hint.chars().enumerate() {
                self.text_pipeline.push_char(
                    c,
                    hx + i as f32 * ui_cw,
                    hint_y,
                    [dim_color[0], dim_color[1], dim_color[2], dim_color[3] * 0.5],
                );
            }

            list_start_y = form.container.y + form_total_h + cell_height * 0.5;
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
                self.text_pipeline
                    .push_char(c, sx + i as f32 * ui_cw, section_y, dim_color);
            }
            list_start_y = section_y + cell_height * 1.5;
        }

        // ── Command rows ──
        let row_height = layout.row_height;

        if info.commands.is_empty() && info.edit_mode == EditMode::Browse {
            // Empty state
            let empty_y = list_start_y + cell_height;
            let msg = "No commands yet";
            let msg_w = msg.len() as f32 * ui_cw;
            let msg_x = panel_x + (panel_width - msg_w) / 2.0;
            for (i, c) in msg.chars().enumerate() {
                self.text_pipeline
                    .push_char(c, msg_x + i as f32 * ui_cw, empty_y, dim_color);
            }
            let hint = "Click + New Command above to add one";
            let hint_w = hint.len() as f32 * ui_cw;
            let hint_x = panel_x + (panel_width - hint_w) / 2.0;
            let hint_y = empty_y + cell_height * 1.5;
            for (i, c) in hint.chars().enumerate() {
                self.text_pipeline.push_char(
                    c,
                    hint_x + i as f32 * ui_cw,
                    hint_y,
                    [dim_color[0], dim_color[1], dim_color[2], dim_color[3] * 0.5],
                );
            }
        }

        for (i, cmd) in info.commands.iter().enumerate() {
            let item_y = layout.command_row(i).y;

            // Hover highlight (full row)
            if info.hovered_index == Some(i) && info.hovered_delete != Some(i) {
                self.rect_pipeline.push_rounded_rect(
                    panel_x + padding * 0.5,
                    item_y,
                    panel_width - padding,
                    row_height,
                    hover_color,
                    6.0,
                );
            }

            // Run indicator (left side) — drawn play triangle
            let run_color = if info.hovered_index == Some(i) {
                accent
            } else {
                dim_color
            };
            let tri_x = panel_x + padding;
            let tri_cy = item_y + row_height / 2.0;
            let tri_h = cell_height * 0.3;
            let tri_w = tri_h * 0.7;
            // Three lines forming a triangle: top-left to right-center, right-center to bottom-left, bottom-left to top-left
            self.rect_pipeline.push_line(
                tri_x,
                tri_cy - tri_h,
                tri_x + tri_w,
                tri_cy,
                1.3,
                run_color,
            );
            self.rect_pipeline.push_line(
                tri_x + tri_w,
                tri_cy,
                tri_x,
                tri_cy + tri_h,
                1.3,
                run_color,
            );
            self.rect_pipeline.push_line(
                tri_x,
                tri_cy + tri_h,
                tri_x,
                tri_cy - tri_h,
                1.3,
                run_color,
            );

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
                self.text_pipeline
                    .push_char(c, name_x + i as f32 * ui_cw, name_y, text_color);
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
                self.text_pipeline
                    .push_char(c, name_x + i as f32 * ui_cw, cmd_y, dim_color);
            }

            // Delete (x) button (right side)
            let del_x = layout.command_delete(i).x + ui_cw * 0.5;
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
            self.rect_pipeline.push_line(
                del_cx - del_arm,
                del_cy - del_arm,
                del_cx + del_arm,
                del_cy + del_arm,
                1.3,
                del_color,
            );
            self.rect_pipeline.push_line(
                del_cx + del_arm,
                del_cy - del_arm,
                del_cx - del_arm,
                del_cy + del_arm,
                1.3,
                del_color,
            );

            // Row separator
            let sep_y = item_y + row_height - 0.5;
            self.rect_pipeline.push_rect(
                panel_x + padding + ui_cw * 2.0,
                sep_y,
                panel_width - padding * 2.0 - ui_cw * 2.0,
                0.5,
                separator,
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
        window_height: f32,
        animation: f32,
        edit_mode: &crate::ui::commands::panel::EditMode,
    ) -> crate::ui::commands::panel::CommandsPanelAction {
        use crate::ui::commands::panel::{CommandsPanelAction, CommandsPanelLayout, EditMode};

        let ui_cw = self.cell_width;
        let cell_height = self.cell_height;

        let layout = CommandsPanelLayout::calculate(
            window_width,
            window_height,
            ui_cw,
            cell_height,
            animation,
            *edit_mode != EditMode::Browse,
            commands.len(),
        );

        // Click outside panel = close
        if x < layout.panel.x {
            return CommandsPanelAction::Close;
        }

        // Close (x) button
        if layout.close_button.contains(x, y) {
            return CommandsPanelAction::Close;
        }

        // "+ New Command" button
        if layout.new_button.contains(x, y) {
            return CommandsPanelAction::StartAdd;
        }

        // Hit-test form fields if editing
        if let Some(form) = layout.form {
            // Name field click
            if form.name_input.contains(x, y) {
                return CommandsPanelAction::ClickName;
            }

            // Command field click
            if form.command_input.contains(x, y) {
                return CommandsPanelAction::ClickCommand;
            }

            // Save button
            if form.save_button.contains(x, y) {
                return CommandsPanelAction::Submit;
            }

            // Cancel button
            if form.cancel_button.contains(x, y) {
                return CommandsPanelAction::CancelEdit;
            }
        }

        // Command rows
        for i in 0..commands.len() {
            if layout.command_delete(i).contains(x, y) {
                return CommandsPanelAction::Delete(i);
            }
            if layout.command_run(i).contains(x, y) {
                return CommandsPanelAction::Run(i);
            }
        }

        CommandsPanelAction::None
    }

    pub(super) fn build_session_management_panel(
        &mut self,
        info: &crate::session::panel::SessionPanelInfo,
        window_width: f32,
        window_height: f32,
    ) {
        use crate::session::panel::SessionPanelLayout;

        let anim = info.animation;
        if anim <= 0.0 {
            return;
        }

        let ui_cw = self.cell_width;
        let cell_height = self.cell_height;

        let layout = SessionPanelLayout::calculate(
            window_width,
            window_height,
            ui_cw,
            cell_height,
            anim,
            info.rename_text.is_some(),
            info.workspaces,
            info.sessions.len(),
        );
        let panel_width = layout.panel.width;
        let panel_x = layout.panel.x;
        let panel_h = layout.panel.height;
        let row_height = layout.row_height;
        let padding_x = layout.padding_x;
        let padding_y = layout.padding_y;

        // ── Scrim ──
        let scrim = self.ui.scrim;
        self.rect_pipeline.push_rect(
            0.0,
            0.0,
            panel_x,
            window_height,
            [scrim[0], scrim[1], scrim[2], scrim[3] * anim],
        );

        // ── Panel background ──
        self.rect_pipeline
            .push_rect(panel_x, 0.0, panel_width, panel_h, self.ui.settings_bg);

        // ── Left border ──
        self.rect_pipeline
            .push_rect(panel_x, 0.0, 2.0, panel_h, self.ui.settings_accent);

        // ── Title ──
        let title = if info.workspaces {
            "Workspaces"
        } else {
            "Legacy Snapshots"
        };
        let title_y = layout.title_y;
        let title_x = panel_x + padding_x;
        for (i, c) in title.chars().enumerate() {
            self.text_pipeline.push_char(
                c,
                title_x + i as f32 * ui_cw,
                title_y,
                self.ui.settings_accent,
            );
        }

        if let (Some(name), Some(rename)) = (&info.rename_text, layout.rename) {
            self.rect_pipeline.push_rect(
                rename.input.x,
                rename.input.y,
                rename.input.width,
                rename.input.height,
                [0.0, 0.0, 0.0, 0.2],
            );
            let text_y = rename.input.y + (rename.input.height - cell_height) / 2.0;
            let text_x = rename.input.x + ui_cw * 0.5;
            for (index, character) in name.chars().enumerate() {
                let x = text_x + index as f32 * ui_cw;
                if x + ui_cw > rename.input.x + rename.input.width {
                    break;
                }
                self.text_pipeline
                    .push_char(character, x, text_y, self.ui.settings_text);
            }
            let cursor_chars = name
                .get(..info.rename_cursor)
                .map(str::chars)
                .map(Iterator::count)
                .unwrap_or_else(|| name.chars().count());
            let cursor_x = text_x + cursor_chars as f32 * ui_cw;
            if cursor_x <= rename.input.x + rename.input.width {
                self.rect_pipeline.push_rect(
                    cursor_x,
                    rename.input.y + 2.0,
                    1.5,
                    rename.input.height - 4.0,
                    self.ui.settings_accent,
                );
            }
            for (button, label, color) in [
                (rename.save_button, "Save", self.ui.settings_accent),
                (rename.cancel_button, "Cancel", self.ui.settings_separator),
            ] {
                self.rect_pipeline.push_rect(
                    button.x,
                    button.y,
                    button.width,
                    button.height,
                    color,
                );
                let label_x =
                    button.x + (button.width - label.chars().count() as f32 * ui_cw * 0.85) / 2.0;
                let label_y = button.y + (button.height - cell_height) / 2.0;
                for (index, character) in label.chars().enumerate() {
                    self.text_pipeline.push_char(
                        character,
                        label_x + index as f32 * ui_cw * 0.85,
                        label_y,
                        self.ui.settings_text,
                    );
                }
            }
        } else {
            let restore_note = if info.workspaces {
                "Restore relaunches tabs and panes"
            } else {
                "Restore opens a new shell"
            };
            let note_y = title_y + cell_height * 1.2;
            for (i, c) in restore_note.chars().take(32).enumerate() {
                if !c.is_whitespace() {
                    self.text_pipeline.push_char(
                        c,
                        panel_x + padding_x + i as f32 * ui_cw * 0.8,
                        note_y,
                        self.ui.settings_text_dim,
                    );
                }
            }
        }

        // ── Close button (×) ──
        let close_x = layout.close_button.x;
        self.text_pipeline
            .push_char('×', close_x, title_y, self.ui.settings_text);

        // ── Separator under title ──
        let sep_y = layout.separator_y;
        self.rect_pipeline.push_rect(
            panel_x + padding_x,
            sep_y,
            panel_width - padding_x * 2.0,
            1.0,
            self.ui.settings_separator,
        );

        // ── Session list ──
        let list_start_y = layout.list_start_y;
        let text_color = self.ui.settings_text;
        let dim_color = self.ui.settings_text_dim;
        let accent_color = self.ui.settings_accent;
        let btn_color = self.ui.settings_separator;

        if info.sessions.is_empty() {
            // Show an explicit empty-workspace placeholder.
            let placeholder = if info.workspaces {
                "No saved workspaces"
            } else {
                "No legacy snapshots"
            };
            let ph_y = list_start_y + row_height * 0.5;
            let mut px = panel_x + padding_x;
            for c in placeholder.chars() {
                if !c.is_whitespace() {
                    self.text_pipeline.push_char(c, px, ph_y, dim_color);
                }
                px += ui_cw * 0.85;
            }
        }

        let end = (info.scroll_offset + layout.visible_count).min(info.sessions.len());
        for (visible_index, session) in info.sessions
            [info.scroll_offset.min(info.sessions.len())..end]
            .iter()
            .enumerate()
        {
            let absolute_index = info.scroll_offset + visible_index;
            let entry_layout = layout.entry(visible_index);
            let item_y = entry_layout.row.y;

            // Hover highlight for session rows
            if info.hovered_index == Some(absolute_index) || info.selected_index == absolute_index {
                let hover_color = [
                    self.ui.settings_selected[0],
                    self.ui.settings_selected[1],
                    self.ui.settings_selected[2],
                    self.ui.settings_selected[3] * 0.4,
                ];
                self.rect_pipeline.push_rounded_rect(
                    panel_x + padding_x * 0.5,
                    item_y,
                    panel_width - padding_x,
                    row_height,
                    hover_color,
                    4.0,
                );
            }

            // Session icon
            let icon_y = item_y + padding_y;
            self.text_pipeline
                .push_char('◆', panel_x + padding_x, icon_y, accent_color);

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

            // Metadata line: "<subtitle> · <last_modified>". Focused-pane
            // snapshots disclose their reduced capture scope here.
            let meta = if session.subtitle.is_empty() {
                session.last_modified.clone()
            } else {
                format!("{} · {}", session.subtitle, session.last_modified)
            };
            let meta_y = icon_y + cell_height * 1.3;
            let mut mx = name_x;
            for c in meta.chars().take(38) {
                if !c.is_whitespace() {
                    self.text_pipeline.push_char(c, mx, meta_y, dim_color);
                }
                mx += ui_cw * 0.8;
            }

            // ── Action buttons: [Load] [Rename] [Delete] ──
            let btn_height = entry_layout.delete_button.height;
            let btn_y = entry_layout.delete_button.y;

            // Delete button (right-aligned)
            let delete_text = "Delete";
            let delete_w = entry_layout.delete_button.width;
            let delete_x = entry_layout.delete_button.x;
            self.rect_pipeline.push_rect(
                delete_x,
                btn_y,
                delete_w,
                btn_height,
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

            // Rename button (left of delete, complete workspaces only)
            if let Some(rename_button) = entry_layout.rename_button {
                let rename_text = "Rename";
                let rename_w = rename_button.width;
                let rename_x = rename_button.x;
                self.rect_pipeline
                    .push_rect(rename_x, btn_y, rename_w, btn_height, btn_color);
                for (j, c) in rename_text.chars().enumerate() {
                    self.text_pipeline.push_char(
                        c,
                        rename_x + ui_cw * 1.0 + j as f32 * ui_cw * 0.85,
                        btn_y + (btn_height - cell_height) * 0.5,
                        text_color,
                    );
                }
            }

            // Load button (left of rename)
            let load_text = "Load";
            let load_w = entry_layout.load_button.width;
            let load_x = entry_layout.load_button.x;
            self.rect_pipeline
                .push_rect(load_x, btn_y, load_w, btn_height, btn_color);
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
                panel_x + padding_x,
                row_sep_y,
                panel_width - padding_x * 2.0,
                1.0,
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
        workspaces: bool,
        renaming: bool,
        scroll_offset: usize,
        window_width: f32,
        window_height: f32,
        animation: f32,
    ) -> crate::session::panel::SessionPanelAction {
        use crate::session::panel::{SessionPanelAction, SessionPanelLayout};

        let ui_cw = self.cell_width;
        let cell_height = self.cell_height;

        let layout = SessionPanelLayout::calculate(
            window_width,
            window_height,
            ui_cw,
            cell_height,
            animation,
            renaming,
            workspaces,
            sessions.len(),
        );

        // Click outside panel → close
        if x < layout.panel.x {
            return SessionPanelAction::Close;
        }

        // Close button (× in top-right)
        if layout.close_button.contains(x, y) {
            return SessionPanelAction::Close;
        }

        if let Some(rename) = layout.rename {
            if rename.save_button.contains(x, y) {
                return SessionPanelAction::SubmitRename;
            }
            if rename.cancel_button.contains(x, y) {
                return SessionPanelAction::CancelRename;
            }
            // The rename form owns input until it is saved or cancelled.
            return SessionPanelAction::None;
        }

        let end = (scroll_offset + layout.visible_count).min(sessions.len());
        for (visible_index, session) in sessions[scroll_offset.min(sessions.len())..end]
            .iter()
            .enumerate()
        {
            let entry = layout.entry(visible_index);
            if !entry.row.contains(x, y) {
                continue;
            }

            // Check if click is in the button row area
            if entry.delete_button.contains(x, y) {
                return SessionPanelAction::Delete(session.id.clone());
            }
            if entry
                .rename_button
                .is_some_and(|button| button.contains(x, y))
            {
                return SessionPanelAction::Rename(session.id.clone());
            }
            if entry.load_button.contains(x, y) {
                return SessionPanelAction::Load(session.id.clone());
            }

            // Clicking anywhere else on the session row → load it
            return SessionPanelAction::Load(session.id.clone());
        }

        SessionPanelAction::None
    }
}
