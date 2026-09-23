//! Slide-in panel rendering — commands panel, session management panel

use super::{Renderer, adjust_alpha};

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
        )
        .scrolled(info.scroll_offset);
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

        // Keyboard and assistive focus use the same visible control geometry as clicks.
        if let Some(rect) = info
            .browse_focus
            .and_then(|control| layout.control_rect(control))
        {
            let inset = 2.0;
            let left = rect.x + inset;
            let top = rect.y + inset;
            let right = rect.x + rect.width - inset;
            let bottom = rect.y + rect.height - inset;
            if right > left && bottom > top {
                for (x1, y1, x2, y2) in [
                    (left, top, right, top),
                    (right, top, right, bottom),
                    (right, bottom, left, bottom),
                    (left, bottom, left, top),
                ] {
                    self.rect_pipeline.push_line(x1, y1, x2, y2, 2.0, accent);
                }
            }
        }

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

            for (field, rect, label, text, placeholder) in [
                (
                    EditMode::EditingName,
                    form.name_input,
                    "Name:",
                    info.form_name.as_str(),
                    "e.g. SSH Production",
                ),
                (
                    EditMode::EditingCommand,
                    form.command_input,
                    "Command:",
                    info.form_command.as_str(),
                    "e.g. ssh admin@prod.app.com",
                ),
            ] {
                let active = info.edit_mode == field;
                let text_y = rect.y + (rect.height - cell_height) / 2.0;
                for (i, c) in label.chars().enumerate() {
                    self.text_pipeline.push_char(
                        c,
                        form_x + small_pad + i as f32 * ui_cw,
                        text_y,
                        dim_color,
                    );
                }
                self.rect_pipeline.push_rounded_rect(
                    rect.x,
                    rect.y,
                    rect.width,
                    rect.height,
                    [0.0, 0.0, 0.0, 0.2],
                    4.0,
                );
                self.rect_pipeline.push_rect(
                    rect.x,
                    rect.y + rect.height - 2.0,
                    rect.width,
                    2.0,
                    if active { accent } else { separator },
                );
                if let Some(view) = layout.input_viewport(info, field) {
                    let clip = super::text::TextClip {
                        left: view.text_x,
                        top: rect.y,
                        right: view.right,
                        bottom: rect.y + rect.height,
                    };
                    self.text_pipeline.push_ui_run(
                        &text[view.text_range.clone()],
                        view.text_x,
                        view.text_y,
                        text_color,
                        ui_cw,
                        clip,
                    );
                    if active {
                        self.rect_pipeline.push_rect(
                            view.cursor_x,
                            rect.y + 3.0,
                            1.5_f32.min(view.right - view.cursor_x),
                            rect.height - 6.0,
                            accent,
                        );
                    } else if text.is_empty()
                        && let Some(hint) =
                            crate::ui::text_input_viewport::TextInputViewport::calculate(
                                placeholder,
                                0,
                                view.text_x,
                                view.text_y,
                                view.right,
                                ui_cw,
                            )
                    {
                        self.text_pipeline.push_ui_run(
                            &placeholder[hint.text_range],
                            view.text_x,
                            view.text_y,
                            [dim_color[0], dim_color[1], dim_color[2], dim_color[3] * 0.5],
                            ui_cw,
                            clip,
                        );
                    }
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
            let section_label = if layout.visible_count < info.commands.len() {
                let range = layout.visible_range();
                format!(
                    "{}–{} / {} · scroll",
                    if range.is_empty() { 0 } else { range.start + 1 },
                    range.end,
                    info.commands.len()
                )
            } else if info.commands.len() == 1 {
                "1 command".to_string()
            } else {
                format!("{} commands", info.commands.len())
            };
            let sx = panel_x + padding;
            let label_cells = ((panel_width - padding * 2.0).max(0.0) / ui_cw) as usize;
            for (i, c) in section_label.chars().take(label_cells).enumerate() {
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

        for i in layout.visible_range() {
            let cmd = &info.commands[i];
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
        info: &crate::ui::commands::panel::CommandsPanelInfo,
        window_width: f32,
        window_height: f32,
    ) -> crate::ui::commands::panel::CommandsPanelAction {
        use crate::ui::commands::panel::{CommandsPanelAction, CommandsPanelLayout, EditMode};

        let ui_cw = self.cell_width;
        let cell_height = self.cell_height;

        let layout = CommandsPanelLayout::calculate(
            window_width,
            window_height,
            ui_cw,
            cell_height,
            info.animation,
            info.edit_mode != EditMode::Browse,
            info.commands.len(),
        )
        .scrolled(info.scroll_offset);

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
            if let Some(action) = layout.input_click(info, x, y) {
                return action;
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
        layout.row_action(x, y)
    }

    pub(super) fn build_session_management_panel(
        &mut self,
        info: &crate::session::panel::SessionPanelInfo,
        window_width: f32,
        window_height: f32,
    ) {
        use crate::session::panel::{SessionPanelControl, SessionPanelLayout, SessionPanelRect};

        let anim = info.animation;
        if anim <= 0.0 {
            return;
        }
        let cell_height = self.cell_height;
        let layout = SessionPanelLayout::calculate(
            window_width,
            window_height,
            self.tab_bar_height(),
            self.cell_width,
            cell_height,
            anim,
            info.rename_text.is_some(),
            info.workspaces,
            info.sessions.len(),
        );
        let scale = self.scale_factor;
        let px = |logical: f32| (logical * scale).round().max(1.0);
        let panel = layout.panel;
        let padding_x = layout.padding_x;
        let size = self.ui_label_px();
        let text = self.ui.menu_text;
        let dim = self.ui.menu_text_dim;
        let stroke = (1.5 * scale).max(1.0);

        // ── Scrim over the terminal, surface, and a hairline edge ──
        let scrim = self.ui.scrim;
        self.rect_pipeline.push_rect(
            0.0,
            panel.y,
            panel.x,
            panel.height,
            adjust_alpha(scrim, scrim[3] * anim),
        );
        self.rect_pipeline
            .push_rect(panel.x, panel.y, panel.width, panel.height, self.ui.menu_bg);
        self.rect_pipeline
            .push_rect(panel.x, panel.y, px(1.0), panel.height, self.ui.menu_border);

        // ── Header: optional back chevron, title, close × ──
        let header_height = layout.close_button.height;
        let hovered_control = info.hovered_control;
        let radius = (5.0 * scale).round();
        for (control, rect) in [
            (SessionPanelControl::Close, Some(layout.close_button)),
            (SessionPanelControl::Back, layout.back_button),
        ] {
            if let Some(rect) = rect
                && hovered_control == Some(control)
            {
                self.rect_pipeline.push_rounded_rect(
                    rect.x,
                    rect.y,
                    rect.width,
                    rect.height,
                    self.ui.menu_highlight,
                    radius,
                );
            }
        }
        let mut title_x = panel.x + padding_x;
        if let Some(back) = layout.back_button {
            let (cx, cy) = (back.x + back.width / 2.0, back.y + back.height / 2.0);
            let arm = 4.5 * scale;
            self.rect_pipeline.push_line(
                cx + arm * 0.5,
                cy - arm,
                cx - arm * 0.5,
                cy,
                stroke,
                text,
            );
            self.rect_pipeline.push_line(
                cx - arm * 0.5,
                cy,
                cx + arm * 0.5,
                cy + arm,
                stroke,
                text,
            );
            title_x = back.x + back.width + px(2.0);
        }
        let title = if info.workspaces {
            "Workspaces"
        } else {
            "Tab Snapshots"
        };
        self.push_ui_label(
            title,
            title_x,
            layout.title_y,
            header_height,
            layout.close_button.x - title_x,
            size * 1.15,
            text,
        );
        let close = layout.close_button;
        let (cx, cy) = (close.x + close.width / 2.0, close.y + close.height / 2.0);
        let arm = 4.0 * scale;
        let close_color = if hovered_control == Some(SessionPanelControl::Close) {
            text
        } else {
            dim
        };
        self.rect_pipeline
            .push_line(cx - arm, cy - arm, cx + arm, cy + arm, stroke, close_color);
        self.rect_pipeline
            .push_line(cx + arm, cy - arm, cx - arm, cy + arm, stroke, close_color);

        let note = if info.workspaces {
            "Opening relaunches the saved tabs and panes"
        } else {
            "Saved when tabs close. Opening starts a new shell"
        };
        self.push_ui_label(
            note,
            panel.x + padding_x,
            layout.note_y,
            cell_height * 1.2,
            panel.width - padding_x * 2.0,
            size * 0.92,
            dim,
        );

        // ── Inline rename form ──
        if let (Some(name), Some(rename)) = (&info.rename_text, layout.rename) {
            let input = rename.input;
            self.rect_pipeline.push_rounded_rect(
                input.x,
                input.y,
                input.width,
                input.height,
                self.ui.palette_input_bg,
                px(5.0),
            );
            if let Some(view) = layout.rename_input_viewport(name, info.rename_cursor) {
                self.text_pipeline.push_ui_run(
                    &name[view.text_range.clone()],
                    view.text_x,
                    view.text_y,
                    text,
                    self.cell_width,
                    super::text::TextClip {
                        left: view.text_x,
                        top: input.y,
                        right: view.right,
                        bottom: input.y + input.height,
                    },
                );
                self.rect_pipeline.push_rect(
                    view.cursor_x,
                    input.y + px(3.0),
                    px(2.0).min(view.right - view.cursor_x),
                    input.height - px(6.0),
                    self.ui.palette_accent,
                );
            }
            for (button, label, primary) in [
                (rename.save_button, "Save", true),
                (rename.cancel_button, "Cancel", false),
            ] {
                self.build_panel_button(button, label, primary, text);
            }
        }

        self.rect_pipeline.push_rect(
            panel.x + px(1.0),
            layout.separator_y.round(),
            panel.width - px(1.0),
            px(1.0),
            self.ui.menu_border,
        );

        // ── Entries ──
        if info.sessions.is_empty() {
            let empty = if info.workspaces {
                "No saved workspaces"
            } else {
                "No tab snapshots"
            };
            self.push_ui_label(
                empty,
                panel.x + padding_x,
                layout.list_start_y,
                layout.row_height,
                panel.width - padding_x * 2.0,
                size,
                dim,
            );
        }
        let end = (info.scroll_offset + layout.visible_count).min(info.sessions.len());
        let start = info.scroll_offset.min(info.sessions.len());
        for (visible_index, session) in info.sessions[start..end].iter().enumerate() {
            let absolute_index = info.scroll_offset + visible_index;
            let entry = layout.entry(visible_index);
            let row = entry.row;
            let active =
                info.hovered_index == Some(absolute_index) || info.selected_index == absolute_index;
            if active {
                self.rect_pipeline.push_rounded_rect(
                    row.x + padding_x * 0.5,
                    row.y + px(2.0),
                    row.width - padding_x,
                    row.height - px(4.0),
                    self.ui.menu_highlight,
                    px(6.0),
                );
            }
            let line = cell_height * 1.3;
            let title_top = entry.load_button.y + (entry.load_button.height - line) / 2.0;
            let actions_left = if active {
                entry.load_button.x - px(8.0)
            } else {
                row.x + row.width - padding_x
            };
            let name_x = row.x + padding_x;
            self.push_ui_label(
                &session.display_name,
                name_x,
                title_top,
                line,
                actions_left - name_x,
                size,
                text,
            );
            let meta = match (
                session.subtitle.is_empty(),
                session.last_modified.is_empty(),
            ) {
                (false, false) => format!("{} · {}", session.subtitle, session.last_modified),
                (false, true) => session.subtitle.clone(),
                (true, _) => session.last_modified.clone(),
            };
            self.push_ui_label(
                &meta,
                name_x,
                title_top + line,
                line,
                row.x + row.width - padding_x - name_x,
                size * 0.92,
                dim,
            );
            if active {
                self.build_panel_button(entry.load_button, "Open", true, text);
                if let Some(rename) = entry.rename_button {
                    self.build_panel_button(rename, "Rename", false, text);
                }
                let danger = adjust_alpha(self.ui.search_error, 0.95);
                self.build_panel_button(entry.delete_button, "Delete", false, danger);
            }
        }

        // ── Tab snapshots link, only when some exist ──
        if info.snapshot_count > 0
            && let Some(link) = layout.snapshots_link
        {
            let link_color = if hovered_control == Some(SessionPanelControl::SnapshotsLink) {
                self.rect_pipeline.push_rect(
                    link.x + px(1.0),
                    link.y,
                    link.width - px(1.0),
                    link.height,
                    self.ui.menu_highlight,
                );
                text
            } else {
                dim
            };
            self.rect_pipeline.push_rect(
                link.x + px(1.0),
                link.y,
                link.width - px(1.0),
                px(1.0),
                self.ui.menu_border,
            );
            let label = format!("Tab snapshots ({})", info.snapshot_count);
            self.push_ui_label(
                &label,
                link.x + padding_x,
                link.y,
                link.height,
                link.width - padding_x * 3.0,
                size,
                link_color,
            );
            let chevron = SessionPanelRect {
                x: link.x + link.width - padding_x * 1.5,
                ..link
            };
            let (cx, cy) = (chevron.x, chevron.y + chevron.height / 2.0);
            let arm = 4.0 * scale;
            self.rect_pipeline.push_line(
                cx - arm * 0.5,
                cy - arm,
                cx + arm * 0.5,
                cy,
                stroke,
                link_color,
            );
            self.rect_pipeline.push_line(
                cx + arm * 0.5,
                cy,
                cx - arm * 0.5,
                cy + arm,
                stroke,
                link_color,
            );
        }
    }

    /// A quiet text button: a subtle surface for the primary action, text
    /// alone otherwise, label centred in the UI face.
    fn build_panel_button(
        &mut self,
        rect: crate::session::panel::SessionPanelRect,
        label: &str,
        primary: bool,
        color: [f32; 4],
    ) {
        let radius = (5.0 * self.scale_factor).round();
        if primary {
            self.rect_pipeline.push_rounded_rect(
                rect.x,
                rect.y,
                rect.width,
                rect.height,
                self.ui.menu_border,
                radius,
            );
        }
        let size = self.ui_label_px() * 0.95;
        let width = self.ui_text_width(label, size).min(rect.width);
        self.push_ui_label(
            label,
            rect.x + (rect.width - width) / 2.0,
            rect.y,
            rect.height,
            rect.width,
            size,
            color,
        );
    }

    /// Hit-test the session management panel for click handling
    pub fn session_panel_hit_test(
        &self,
        x: f32,
        y: f32,
        info: &crate::session::panel::SessionPanelInfo,
        window_width: f32,
        window_height: f32,
    ) -> crate::session::panel::SessionPanelAction {
        use crate::session::panel::{SessionPanelAction, SessionPanelControl, SessionPanelLayout};

        let ui_cw = self.cell_width;
        let cell_height = self.cell_height;

        let layout = SessionPanelLayout::calculate(
            window_width,
            window_height,
            self.tab_bar_height(),
            ui_cw,
            cell_height,
            info.animation,
            info.rename_text.is_some(),
            info.workspaces,
            info.sessions.len(),
        );

        // A click beside or above the panel (the toolbar) dismisses it.
        if x < layout.panel.x || y < layout.panel.y {
            return SessionPanelAction::Close;
        }

        let control = layout.control_at(x, y, info.snapshot_count);
        if control == Some(SessionPanelControl::Close) {
            return SessionPanelAction::Close;
        }

        if let Some(rename) = layout.rename {
            if rename.save_button.contains(x, y) {
                return SessionPanelAction::SubmitRename;
            }
            if rename.cancel_button.contains(x, y) {
                return SessionPanelAction::CancelRename;
            }
            if let Some(text) = &info.rename_text
                && let Some(action) = layout.rename_input_click(text, info.rename_cursor, x, y)
            {
                return action;
            }
            // The rename form owns input until it is saved or cancelled.
            return SessionPanelAction::None;
        }

        match control {
            Some(SessionPanelControl::Back) => return SessionPanelAction::ShowWorkspaces,
            Some(SessionPanelControl::SnapshotsLink) => return SessionPanelAction::ShowSnapshots,
            Some(SessionPanelControl::Close) | None => {}
        }

        let end = (info.scroll_offset + layout.visible_count).min(info.sessions.len());
        for (visible_index, session) in info.sessions
            [info.scroll_offset.min(info.sessions.len())..end]
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

            // Elsewhere on the row selects it; restoring replaces the open
            // tabs, so it needs Open, Enter, or a double click.
            return SessionPanelAction::Select(session.id.clone());
        }

        SessionPanelAction::None
    }
}
