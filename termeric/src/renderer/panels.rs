//! Slide-in side panels — Commands, Agents, and session management — their
//! shared frame and header buttons, and the Commands and session panel hit tests.

use super::Renderer;
use crate::ui::color::{scale_alpha, with_alpha};
use crate::ui::geometry::Bounds;
use crate::ui::icon::Icon;

impl Renderer {
    /// The Commands panel's geometry, shared by its drawing and hit test.
    fn commands_panel_layout(
        &self,
        info: &crate::ui::commands::panel::CommandsPanelInfo,
        window_width: f32,
        window_height: f32,
    ) -> crate::ui::commands::panel::CommandsPanelLayout {
        use crate::ui::commands::panel::{CommandsPanelLayout, EditMode};
        CommandsPanelLayout::calculate(
            window_width,
            window_height,
            self.tab_bar_height(),
            self.cell_width(),
            self.cell_height(),
            info.animation,
            info.edit_mode != EditMode::Browse,
            info.commands.len(),
        )
        .scrolled(info.scroll_offset)
    }

    /// The session panel's geometry, shared by its drawing and hit test.
    fn session_panel_layout(
        &self,
        info: &crate::ui::session_panel::SessionPanelInfo,
        window_width: f32,
        window_height: f32,
    ) -> crate::ui::session_panel::SessionPanelLayout {
        crate::ui::session_panel::SessionPanelLayout::calculate(
            window_width,
            window_height,
            self.tab_bar_height(),
            self.cell_width(),
            self.cell_height(),
            info.animation,
            info.rename_text.is_some(),
            info.workspaces,
            info.sessions.len(),
        )
    }

    pub(super) fn build_commands_panel(
        &mut self,
        info: &crate::ui::commands::panel::CommandsPanelInfo,
        window_width: f32,
        window_height: f32,
    ) {
        use crate::ui::commands::navigation::CommandControl;
        use crate::ui::commands::panel::{COMMAND_PLACEHOLDER, EditMode, NAME_PLACEHOLDER};

        let anim = info.animation;
        if anim <= 0.0 {
            return;
        }
        let cell_width = self.cell_width();
        let cell_height = self.cell_height();
        let layout = self.commands_panel_layout(info, window_width, window_height);
        let scale = self.scale_factor;
        let px = |logical: f32| (logical * scale).round().max(1.0);
        let panel = layout.panel;
        let padding = layout.padding;
        let size = self.ui_label_px();
        let text = self.ui.menu_text;
        let dim = self.ui.menu_text_dim;

        self.build_side_panel_frame(panel, anim);

        // ── Header: title, "+" and × ──
        let close = layout.close_button;
        let new = layout.new_button;
        self.build_header_icon_button(new, Icon::Plus, info.hovered == Some(CommandControl::New));
        self.build_header_icon_button(
            close,
            Icon::Close,
            info.hovered == Some(CommandControl::Close),
        );
        let title_x = panel.x + padding;
        self.push_ui_label(
            "Custom Commands",
            title_x,
            layout.title_y,
            close.height,
            new.x - title_x,
            size * 1.15,
            text,
        );
        self.push_ui_label(
            "Click a command to run it in the focused terminal",
            title_x,
            layout.note_y,
            cell_height * 1.2,
            panel.width - padding * 2.0,
            size * 0.92,
            dim,
        );
        self.rect_pipeline.push_rect(
            panel.x + px(1.0),
            layout.separator_y.round(),
            panel.width - px(1.0),
            px(1.0),
            self.ui.menu_border,
        );

        // ── Inline add form ──
        if let Some(form) = layout.form {
            let container = form.container;
            self.rect_pipeline.push_rounded_rect(
                container.x,
                container.y,
                container.width,
                container.height,
                self.ui.menu_highlight,
                px(6.0),
            );
            for (field, rect, label, value, placeholder) in [
                (
                    EditMode::EditingName,
                    form.name_input,
                    "Name",
                    info.form_name.as_str(),
                    NAME_PLACEHOLDER,
                ),
                (
                    EditMode::EditingCommand,
                    form.command_input,
                    "Command",
                    info.form_command.as_str(),
                    COMMAND_PLACEHOLDER,
                ),
            ] {
                let active = info.edit_mode == field;
                let label_x = container.x + layout.small_padding;
                self.push_ui_label(
                    label,
                    label_x,
                    rect.y,
                    rect.height,
                    rect.x - label_x - layout.small_padding,
                    size,
                    dim,
                );
                self.rect_pipeline.push_rounded_rect(
                    rect.x,
                    rect.y,
                    rect.width,
                    rect.height,
                    self.ui.palette_input_bg,
                    px(5.0),
                );
                if active {
                    self.push_rect_outline(
                        rect.x,
                        rect.y,
                        rect.width,
                        rect.height,
                        px(1.0),
                        self.ui.palette_accent,
                    );
                }
                let Some(view) = layout.input_viewport(info, field) else {
                    continue;
                };
                let clip = super::text::TextClip {
                    left: view.text_x,
                    top: rect.y,
                    right: view.right,
                    bottom: rect.y + rect.height,
                };
                let (shown, color, range) = if value.is_empty() {
                    let hint = crate::ui::text_input_viewport::TextInputViewport::calculate(
                        placeholder,
                        0,
                        view.text_x,
                        view.text_y,
                        view.right,
                        cell_width,
                    );
                    (
                        placeholder,
                        scale_alpha(dim, 0.6),
                        hint.map_or(0..0, |hint| hint.text_range),
                    )
                } else {
                    (value, text, view.text_range.clone())
                };
                self.text_pipeline.push_ui_run(
                    &shown[range],
                    view.text_x,
                    view.text_y,
                    color,
                    cell_width,
                    clip,
                );
                if active {
                    self.rect_pipeline.push_rect(
                        view.cursor_x,
                        rect.y + px(3.0),
                        px(2.0).min(view.right - view.cursor_x),
                        rect.height - px(6.0),
                        self.ui.palette_accent,
                    );
                }
            }
            self.build_panel_button(form.save_button, "Save", true, text);
            self.build_panel_button(form.cancel_button, "Cancel", false, text);
        }

        // ── Commands ──
        if info.commands.is_empty() && layout.form.is_none() {
            self.push_ui_label(
                "No saved commands",
                panel.x + padding,
                layout.list_start_y,
                layout.row_height,
                panel.width - padding * 2.0,
                size,
                dim,
            );
        }
        let row_of = |control: Option<CommandControl>| control.and_then(CommandControl::row);
        let hovered_row = row_of(info.hovered);
        let focused_row = row_of(info.browse_focus);
        let line = cell_height * 1.3;
        for index in layout.visible_range() {
            let command = &info.commands[index];
            let row = layout.command_row(index);
            let delete = layout.command_delete(index);
            let active = hovered_row == Some(index) || focused_row == Some(index);
            if active {
                self.rect_pipeline.push_rounded_rect(
                    row.x + padding * 0.5,
                    row.y + px(2.0),
                    row.width - padding,
                    row.height - px(4.0),
                    self.ui.menu_highlight,
                    px(6.0),
                );
            }
            let name_x = row.x + padding;
            let right = if active {
                delete.x - px(8.0)
            } else {
                row.x + row.width - padding
            };
            let top = row.y + (row.height - line * 2.0) / 2.0;
            self.push_ui_label(&command.name, name_x, top, line, right - name_x, size, text);
            let first_line = command.command.lines().next().unwrap_or_default();
            self.push_ui_label(
                first_line,
                name_x,
                top + line,
                line,
                right - name_x,
                size * 0.92,
                dim,
            );
            if active {
                let danger = if info.hovered == Some(CommandControl::Delete(index)) {
                    self.ui.search_error
                } else {
                    with_alpha(self.ui.search_error, 0.85)
                };
                self.build_panel_button(delete, "Delete", false, danger);
            }
        }
        self.draw_panel_scroll_thumb(
            panel,
            layout.list_start_y,
            layout.row_height,
            layout.visible_count,
            info.commands.len(),
            layout.scroll_offset,
            with_alpha(dim, 0.45),
        );

        // Keyboard and assistive focus use the same geometry as clicks.
        if let Some(rect) = info
            .browse_focus
            .and_then(|control| layout.control_rect(control))
        {
            self.push_rect_outline(
                rect.x,
                rect.y,
                rect.width,
                rect.height,
                px(2.0),
                self.ui.palette_accent,
            );
        }
    }

    pub(super) fn build_agents_panel(
        &mut self,
        info: &crate::ui::agents_panel::AgentsPanelInfo,
        window_width: f32,
        window_height: f32,
    ) {
        use crate::ui::agents_panel::{AgentRowState, AgentsControl, AgentsPanelLayout};

        let anim = info.animation;
        if anim <= 0.0 {
            return;
        }
        let cell_height = self.cell_height();
        let layout = AgentsPanelLayout::calculate(
            window_width,
            window_height,
            self.tab_bar_height(),
            self.cell_width(),
            cell_height,
            anim,
            info.rows.len(),
            info.supervisor
                .as_ref()
                .map(|supervisor| supervisor.entries.len()),
        )
        .scrolled(info.scroll_offset);
        let scale = self.scale_factor;
        let px = |logical: f32| (logical * scale).round().max(1.0);
        let panel = layout.panel;
        let padding = layout.padding;
        let size = self.ui_label_px();
        let text = self.ui.menu_text;
        let dim = self.ui.menu_text_dim;

        self.build_side_panel_frame(panel, anim);

        // ── Header ──
        let close = layout.close_button;
        self.build_header_icon_button(
            close,
            Icon::Close,
            info.hovered == Some(AgentsControl::Close),
        );
        let title_x = panel.x + padding;
        self.push_ui_label(
            "Agents",
            title_x,
            layout.title_y,
            close.height,
            close.x - title_x,
            size * 1.15,
            text,
        );
        self.rect_pipeline.push_rect(
            panel.x + px(1.0),
            layout.separator_y.round(),
            panel.width - px(1.0),
            px(1.0),
            self.ui.menu_border,
        );

        // ── Agents ──
        if info.rows.is_empty() {
            self.push_ui_label(
                "No agents in this window",
                title_x,
                layout.list_start_y,
                layout.row_height,
                panel.width - padding * 2.0,
                size,
                dim,
            );
        }
        let line = cell_height * 1.3;
        let dot = (size * 0.7).round();
        for index in layout.visible_range() {
            let row_info = &info.rows[index];
            let row = layout.row(index);
            // Focus on a row reads like a menu's selection: the same quiet
            // rounded surface as hover, not a hard outline.
            let active = info.hovered == Some(AgentsControl::Agent(index))
                || info.focus == Some(AgentsControl::Agent(index));
            let surface = if active {
                self.rect_pipeline.push_rounded_rect(
                    row.x + padding * 0.5,
                    row.y + px(2.0),
                    row.width - padding,
                    row.height - px(4.0),
                    self.ui.menu_highlight,
                    px(6.0),
                );
                self.ui.menu_highlight
            } else {
                self.ui.menu_bg
            };
            let top = row.y + (row.height - line * 2.0) / 2.0;
            // The tab badge's dot: a ring while working, the theme's colors
            // for a request, a result, or a failure.
            let fill = match row_info.state {
                AgentRowState::Unknown => None,
                AgentRowState::Idle => Some(with_alpha(dim, 0.5)),
                AgentRowState::Working => Some(text),
                AgentRowState::NeedsYou => Some(self.ui.warning),
                AgentRowState::Done => Some(self.ui.success),
                AgentRowState::Failed => Some(self.ui.error),
            };
            let (dot_x, dot_y) = ((title_x).round(), (top + (line - dot) / 2.0).round());
            if let Some(fill) = fill {
                self.rect_pipeline
                    .push_rounded_rect(dot_x, dot_y, dot, dot, fill, dot / 2.0);
                if matches!(row_info.state, AgentRowState::Working | AgentRowState::Idle) {
                    let inner = (dot - px(1.5) * 2.0).max(1.0);
                    self.rect_pipeline.push_rounded_rect(
                        dot_x + (dot - inner) / 2.0,
                        dot_y + (dot - inner) / 2.0,
                        inner,
                        inner,
                        surface,
                        inner / 2.0,
                    );
                }
            }
            let name_x = title_x + dot + px(8.0);
            let right = row.x + row.width - padding;
            let name_width = self.push_ui_label(
                &row_info.name,
                name_x,
                top,
                line,
                right - name_x,
                size,
                text,
            );
            let place_x = name_x + name_width + px(8.0);
            if place_x < right {
                self.push_ui_label(
                    &row_info.place,
                    place_x,
                    top,
                    line,
                    right - place_x,
                    size * 0.92,
                    dim,
                );
            }
            self.push_ui_label(
                &row_info.summary,
                name_x,
                top + line,
                line,
                right - name_x,
                size * 0.92,
                dim,
            );
        }
        self.draw_panel_scroll_thumb(
            panel,
            layout.list_start_y,
            layout.row_height,
            layout.visible_count,
            info.rows.len(),
            layout.scroll_offset,
            with_alpha(dim, 0.45),
        );

        // ── Supervisor ──
        if let (Some(section), Some(supervisor)) = (layout.supervisor, info.supervisor.as_ref()) {
            self.rect_pipeline.push_rect(
                panel.x + px(1.0),
                section.section.y.round(),
                panel.width - px(1.0),
                px(1.0),
                self.ui.menu_border,
            );
            let heading = format!("Supervisor · {}", supervisor.mode);
            self.push_ui_label(
                &heading,
                section.heading.x,
                section.heading.y,
                section.heading.height,
                section.heading.width,
                size,
                text,
            );
            let label = if supervisor.held {
                "Hand Back"
            } else {
                "Take Over"
            };
            let color = if info.hovered == Some(AgentsControl::Supervisor) {
                text
            } else {
                with_alpha(text, 0.9)
            };
            self.build_panel_button(section.button, label, true, color);
            for (index, entry) in supervisor
                .entries
                .iter()
                .take(section.entry_count)
                .enumerate()
            {
                let rect = section.entry(index);
                self.push_ui_label(
                    entry,
                    rect.x,
                    rect.y,
                    rect.height,
                    rect.width - padding,
                    size * 0.92,
                    dim,
                );
            }
        }

        // Keyboard and assistive focus on a button uses the same geometry
        // as clicks; a focused row is shown by its surface above.
        if let Some(rect) = info
            .focus
            .filter(|control| !matches!(control, AgentsControl::Agent(_)))
            .and_then(|control| layout.control_rect(control))
        {
            self.push_rect_outline(
                rect.x,
                rect.y,
                rect.width,
                rect.height,
                px(2.0),
                self.ui.palette_accent,
            );
        }
    }

    /// The thin scroll thumb at a side panel's right edge, beside a list of
    /// `visible` of `total` rows scrolled by `offset`; nothing while it fits.
    fn draw_panel_scroll_thumb(
        &mut self,
        panel: Bounds,
        list_top: f32,
        row_height: f32,
        visible: usize,
        total: usize,
        offset: usize,
        color: [f32; 4],
    ) {
        if visible == 0 || visible >= total {
            return;
        }
        let px = |logical: f32| (logical * self.scale_factor).round().max(1.0);
        let track_height = visible as f32 * row_height;
        let thumb_height = (track_height * visible as f32 / total as f32).max(px(24.0));
        let travel = (track_height - thumb_height).max(0.0);
        let thumb_top = list_top + travel * offset as f32 / (total - visible) as f32;
        self.rect_pipeline.push_rounded_rect(
            panel.x + panel.width - px(6.0),
            thumb_top,
            px(3.0),
            thumb_height,
            color,
            px(1.5),
        );
    }

    /// Scrim over the terminal beside a side panel, the panel surface, and
    /// its hairline edge. Shared by every right-edge panel.
    pub(super) fn build_side_panel_frame(&mut self, panel: Bounds, animation: f32) {
        let Bounds {
            x,
            y,
            width,
            height,
        } = panel;
        let hairline = self.scale_factor.round().max(1.0);
        let scrim = self.ui.scrim;
        self.rect_pipeline
            .push_rect(0.0, y, x, height, scale_alpha(scrim, animation));
        self.rect_pipeline
            .push_rect(x, y, width, height, self.ui.menu_bg);
        self.rect_pipeline
            .push_rect(x, y, hairline, height, self.ui.menu_border);
    }

    /// A square header control drawn as a vector icon, dim until hovered.
    pub(super) fn build_header_icon_button(&mut self, bounds: Bounds, icon: Icon, hovered: bool) {
        let Bounds {
            x,
            y,
            width,
            height,
        } = bounds;
        if hovered {
            self.rect_pipeline.push_rounded_rect(
                x,
                y,
                width,
                height,
                self.ui.menu_highlight,
                (5.0 * self.scale_factor).round(),
            );
        }
        let color = if hovered {
            self.ui.menu_text
        } else {
            self.ui.menu_text_dim
        };
        self.push_icon(icon, x + width / 2.0, y + height / 2.0, color);
    }

    /// A rectangular outline of `stroke` pixels inside the given bounds.
    fn push_rect_outline(
        &mut self,
        x: f32,
        y: f32,
        width: f32,
        height: f32,
        stroke: f32,
        color: [f32; 4],
    ) {
        if width <= stroke * 2.0 || height <= stroke * 2.0 {
            return;
        }
        self.rect_pipeline
            .push_rounded_outline(x, y, width, height, color, 0.0, stroke);
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
        use crate::ui::commands::panel::CommandsPanelAction;

        let layout = self.commands_panel_layout(info, window_width, window_height);

        // A click beside or above the panel (the toolbar) dismisses it.
        if x < layout.panel.x || y < layout.panel.y {
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
        info: &crate::ui::session_panel::SessionPanelInfo,
        window_width: f32,
        window_height: f32,
    ) {
        use crate::ui::session_panel::SessionPanelControl;

        let anim = info.animation;
        if anim <= 0.0 {
            return;
        }
        let cell_height = self.cell_height();
        let layout = self.session_panel_layout(info, window_width, window_height);
        let scale = self.scale_factor;
        let px = |logical: f32| (logical * scale).round().max(1.0);
        let panel = layout.panel;
        let padding_x = layout.padding_x;
        let size = self.ui_label_px();
        let text = self.ui.menu_text;
        let dim = self.ui.menu_text_dim;

        self.build_side_panel_frame(panel, anim);

        // ── Header: optional back chevron, title, close × ──
        let hovered_control = info.hovered_control;
        let close = layout.close_button;
        let mut title_x = panel.x + padding_x;
        if let Some(back) = layout.back_button {
            self.build_header_icon_button(
                back,
                Icon::Back,
                hovered_control == Some(SessionPanelControl::Back),
            );
            title_x = back.x + back.width + px(2.0);
        }
        self.build_header_icon_button(
            close,
            Icon::Close,
            hovered_control == Some(SessionPanelControl::Close),
        );
        let title = if info.workspaces {
            "Workspaces"
        } else {
            "Tab Snapshots"
        };
        self.push_ui_label(
            title,
            title_x,
            layout.title_y,
            close.height,
            close.x - title_x,
            size * 1.15,
            text,
        );

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
                    self.cell_width(),
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
                let danger = with_alpha(self.ui.search_error, 0.95);
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
            self.push_icon(
                Icon::ChevronRight,
                link.x + link.width - padding_x * 1.5,
                link.y + link.height / 2.0,
                link_color,
            );
        }
    }

    /// A quiet text button: a subtle surface for the primary action, text
    /// alone otherwise, label centred in the UI face.
    fn build_panel_button(&mut self, rect: Bounds, label: &str, primary: bool, color: [f32; 4]) {
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
        info: &crate::ui::session_panel::SessionPanelInfo,
        window_width: f32,
        window_height: f32,
    ) -> crate::ui::session_panel::SessionPanelAction {
        use crate::ui::session_panel::{SessionPanelAction, SessionPanelControl};

        let layout = self.session_panel_layout(info, window_width, window_height);

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
