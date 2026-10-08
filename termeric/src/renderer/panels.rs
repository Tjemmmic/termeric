//! Slide-in side panels — Commands, Agents, and Workspaces — their shared
//! frame and header buttons, and the Commands panel hit test.

use super::Renderer;
use super::draw::FieldTextColors;
use crate::ui::color::{scale_alpha, with_alpha};
use crate::ui::constants::BUTTON_CORNER_RADIUS;
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
        let cell_height = self.cell_height();
        let layout = self.commands_panel_layout(info, window_width, window_height);
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
            panel.x + self.px(1.0),
            layout.separator_y.round(),
            panel.width - self.px(1.0),
            self.px(1.0),
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
                self.px(BUTTON_CORNER_RADIUS),
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
                    self.px(BUTTON_CORNER_RADIUS),
                );
                if active {
                    self.rect_pipeline.push_rounded_outline(
                        rect.x,
                        rect.y,
                        rect.width,
                        rect.height,
                        self.ui.palette_accent,
                        self.px(BUTTON_CORNER_RADIUS),
                        self.px(1.0),
                    );
                }
                let Some(view) = layout.input_viewport(info, field) else {
                    continue;
                };
                self.draw_field_text(
                    rect,
                    &view,
                    value,
                    placeholder,
                    FieldTextColors {
                        text,
                        placeholder: scale_alpha(dim, 0.6),
                    },
                    active.then_some(self.ui.palette_accent),
                );
            }
            self.build_panel_button(form.save_button, "Save", true, false, text, 1.0);
            self.build_panel_button(form.cancel_button, "Cancel", false, false, text, 1.0);
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
                    row.y + self.px(2.0),
                    row.width - padding,
                    row.height - self.px(4.0),
                    self.ui.menu_highlight,
                    self.px(BUTTON_CORNER_RADIUS),
                );
            }
            let name_x = row.x + padding;
            let right = if active {
                delete.x - self.px(8.0)
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
                let hovered = info.hovered == Some(CommandControl::Delete(index));
                let danger = if hovered {
                    self.ui.error
                } else {
                    with_alpha(self.ui.error, 0.85)
                };
                self.build_panel_button(delete, "Delete", false, hovered, danger, 1.0);
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
            self.draw_focus_ring(rect, self.ui.focus_ring, BUTTON_CORNER_RADIUS);
        }
    }

    pub(super) fn build_agents_panel(
        &mut self,
        info: &crate::ui::agents_panel::AgentsPanelInfo,
        window_width: f32,
        window_height: f32,
    ) {
        use crate::ui::agents_panel::{AgentsControl, AgentsPanelLayout};

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
            panel.x + self.px(1.0),
            layout.separator_y.round(),
            panel.width - self.px(1.0),
            self.px(1.0),
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
                    row.y + self.px(2.0),
                    row.width - padding,
                    row.height - self.px(4.0),
                    self.ui.menu_highlight,
                    self.px(BUTTON_CORNER_RADIUS),
                );
                self.ui.menu_highlight
            } else {
                self.ui.menu_bg
            };
            let top = row.y + (row.height - line * 2.0) / 2.0;
            // The tab badge's dot, beside the name's line.
            if let Some((fill, ring)) = self.agent_status_dot(row_info.state, text) {
                self.push_status_dot(
                    [title_x.round() + dot / 2.0, top + line / 2.0],
                    dot,
                    fill,
                    ring.then_some(surface),
                );
            }
            let name_x = title_x + dot + self.px(8.0);
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
            let place_x = name_x + name_width + self.px(8.0);
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
                panel.x + self.px(1.0),
                section.section.y.round(),
                panel.width - self.px(1.0),
                self.px(1.0),
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
            let hovered = info.hovered == Some(AgentsControl::Supervisor);
            let color = if hovered { text } else { with_alpha(text, 0.9) };
            self.build_panel_button(section.button, label, true, hovered, color, 1.0);
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
            self.draw_focus_ring(rect, self.ui.focus_ring, BUTTON_CORNER_RADIUS);
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
        let track = Bounds::new(
            panel.right() - self.px(6.0),
            list_top,
            self.px(3.0),
            visible as f32 * row_height,
        );
        self.draw_scroll_thumb(track, visible, total, offset, color);
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
        let hairline = self.px(1.0);
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
                self.px(BUTTON_CORNER_RADIUS),
            );
        }
        let color = if hovered {
            self.ui.menu_text
        } else {
            self.ui.menu_text_dim
        };
        self.push_icon(icon, x + width / 2.0, y + height / 2.0, color);
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

    /// Draw the workspace panel showing `info` in `layout`, the geometry the
    /// app hit-tests and describes it with.
    pub(super) fn build_workspace_panel(
        &mut self,
        info: &crate::ui::workspace_panel::WorkspacePanelInfo,
        layout: &crate::ui::workspace_panel::WorkspacePanelLayout,
    ) {
        use crate::ui::workspace_panel::WorkspacePanelControl;

        let anim = info.animation;
        if anim <= 0.0 {
            return;
        }
        let cell_height = self.cell_height();
        let layout = *layout;
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
                hovered_control == Some(WorkspacePanelControl::Back),
            );
            title_x = back.x + back.width + self.px(2.0);
        }
        self.build_header_icon_button(
            close,
            Icon::Close,
            hovered_control == Some(WorkspacePanelControl::Close),
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
                self.px(BUTTON_CORNER_RADIUS),
            );
            if let Some(view) = layout.rename_input_viewport(name, info.rename_cursor) {
                self.draw_field_text(
                    input,
                    &view,
                    name,
                    "",
                    FieldTextColors {
                        text,
                        placeholder: dim,
                    },
                    Some(self.ui.palette_accent),
                );
            }
            for (button, label, primary) in [
                (rename.save_button, "Save", true),
                (rename.cancel_button, "Cancel", false),
            ] {
                self.build_panel_button(button, label, primary, false, text, 1.0);
            }
        }

        self.rect_pipeline.push_rect(
            panel.x + self.px(1.0),
            layout.separator_y.round(),
            panel.width - self.px(1.0),
            self.px(1.0),
            self.ui.menu_border,
        );

        // ── Entries ──
        if info.entries.is_empty() {
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
        let end = (info.scroll_offset + layout.visible_count).min(info.entries.len());
        let start = info.scroll_offset.min(info.entries.len());
        for (visible_index, workspace) in info.entries[start..end].iter().enumerate() {
            let absolute_index = info.scroll_offset + visible_index;
            let entry = layout.entry(visible_index);
            let row = entry.row;
            let active =
                info.hovered_index == Some(absolute_index) || info.selected_index == absolute_index;
            if active {
                self.rect_pipeline.push_rounded_rect(
                    row.x + padding_x * 0.5,
                    row.y + self.px(2.0),
                    row.width - padding_x,
                    row.height - self.px(4.0),
                    self.ui.menu_highlight,
                    self.px(BUTTON_CORNER_RADIUS),
                );
            }
            let line = cell_height * 1.3;
            let title_top = entry.load_button.y + (entry.load_button.height - line) / 2.0;
            let actions_left = if active {
                entry.load_button.x - self.px(8.0)
            } else {
                row.x + row.width - padding_x
            };
            let name_x = row.x + padding_x;
            self.push_ui_label(
                &workspace.display_name,
                name_x,
                title_top,
                line,
                actions_left - name_x,
                size,
                text,
            );
            let meta = match (
                workspace.subtitle.is_empty(),
                workspace.last_modified.is_empty(),
            ) {
                (false, false) => format!("{} · {}", workspace.subtitle, workspace.last_modified),
                (false, true) => workspace.subtitle.clone(),
                (true, _) => workspace.last_modified.clone(),
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
                self.build_panel_button(entry.load_button, "Open", true, false, text, 1.0);
                if let Some(rename) = entry.rename_button {
                    self.build_panel_button(rename, "Rename", false, false, text, 1.0);
                }
                let danger = with_alpha(self.ui.error, 0.95);
                self.build_panel_button(entry.delete_button, "Delete", false, false, danger, 1.0);
            }
        }

        // ── Tab snapshots link, only when some exist ──
        if info.snapshot_count > 0
            && let Some(link) = layout.snapshots_link
        {
            let link_color = if hovered_control == Some(WorkspacePanelControl::SnapshotsLink) {
                self.rect_pipeline.push_rect(
                    link.x + self.px(1.0),
                    link.y,
                    link.width - self.px(1.0),
                    link.height,
                    self.ui.menu_highlight,
                );
                text
            } else {
                dim
            };
            self.rect_pipeline.push_rect(
                link.x + self.px(1.0),
                link.y,
                link.width - self.px(1.0),
                self.px(1.0),
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
    /// alone otherwise, and the highlight surface while hovered; its label
    /// centred in the UI face. The surface fades with `opacity`; `color`
    /// is the label's, already faded.
    pub(super) fn build_panel_button(
        &mut self,
        rect: Bounds,
        label: &str,
        primary: bool,
        hovered: bool,
        color: [f32; 4],
        opacity: f32,
    ) {
        let surface = if hovered {
            Some(self.ui.menu_highlight)
        } else {
            primary.then_some(self.ui.menu_border)
        };
        if let Some(surface) = surface {
            self.rect_pipeline.push_rounded_rect(
                rect.x,
                rect.y,
                rect.width,
                rect.height,
                scale_alpha(surface, opacity),
                self.px(BUTTON_CORNER_RADIUS),
            );
        }
        let size = self.ui_label_px() * 0.95;
        self.push_ui_label_centered(
            label,
            rect.center()[0],
            rect.y,
            rect.height,
            rect.width,
            size,
            color,
        );
    }
}
