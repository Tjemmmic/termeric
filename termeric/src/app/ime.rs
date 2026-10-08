//! Platform input-method state and commit routing.
//!
//! Preedit text is application-owned display state. It is never written to a
//! PTY; only `Ime::Commit` reaches the currently focused text surface.

use super::text_target::TextTarget;
use winit::event::Ime;

use super::*;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(super) struct ImeState {
    enabled: bool,
    preedit: Option<ImePreedit>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct ImePreedit {
    pub text: String,
    pub cursor: Option<(usize, usize)>,
}

impl ImeState {
    pub fn preedit(&self) -> Option<&ImePreedit> {
        self.preedit.as_ref()
    }

    pub fn clear_preedit(&mut self) {
        self.preedit = None;
    }

    fn set_preedit(&mut self, text: String, cursor: Option<(usize, usize)>) {
        if text.is_empty() {
            self.preedit = None;
            return;
        }
        let cursor = cursor.filter(|(start, end)| {
            start <= end
                && *end <= text.len()
                && text.is_char_boundary(*start)
                && text.is_char_boundary(*end)
        });
        self.preedit = Some(ImePreedit { text, cursor });
    }
}

impl App {
    pub(super) fn handle_ime_event(&mut self, event: Ime) {
        match event {
            Ime::Enabled => self.ime.enabled = true,
            Ime::Disabled => {
                self.ime.enabled = false;
                self.ime.clear_preedit();
            }
            Ime::Preedit(text, cursor) => {
                if self.text_target() != TextTarget::Blocked {
                    self.ime.set_preedit(text, cursor);
                } else {
                    self.ime.clear_preedit();
                }
            }
            Ime::Commit(text) => {
                self.ime.clear_preedit();
                self.commit_ime_text(&text);
            }
        }
        self.request_redraw(RedrawReason::KeyboardInput);
    }

    fn commit_ime_text(&mut self, text: &str) {
        if text.is_empty() {
            return;
        }
        match self.text_target() {
            TextTarget::Blocked => {}
            TextTarget::Palette(modal) => {
                self.palette_owner_mut(modal).insert_query_text(text);
            }
            TextTarget::ShortcutsFilter => {
                self.shortcuts_panel.insert_filter_text(text);
            }
            TextTarget::SettingsFilter => {
                self.settings_panel.insert_filter_text(text);
            }
            TextTarget::WorkspaceRename => {
                self.workspace_panel.insert_rename_text(text);
            }
            TextTarget::CommandsForm => {
                self.commands_panel.insert_form_text(text);
            }
            TextTarget::DialogField => {
                let request_id = self.dialog.request_id();
                self.dialog.insert_field_text(request_id, text);
            }
            TextTarget::TabRename => {
                self.insert_tab_rename_text(text);
            }
            TextTarget::AgentView => {
                self.commit_agent_view_text(text);
            }
            TextTarget::Search => {
                self.insert_search_text(text);
            }
            TextTarget::Terminal => {
                let encoded =
                    crate::input::encode_ime_text(self.active_kitty_keyboard_flags(), text);
                self.write_to_pty(&encoded);
            }
        }
    }

    /// The input viewport of `target` when it is a field of a palette,
    /// dialog, or panel, in the geometry it is drawn with; `None` for any
    /// other target.
    pub(super) fn field_ime_viewport(
        &self,
        target: TextTarget,
    ) -> Option<crate::ui::text_input_viewport::TextInputViewport> {
        let metrics = self.ui_metrics();
        match target {
            TextTarget::Palette(modal) => {
                let info = self.palette_owner(modal).build_info();
                self.palette_layout(&info)
                    .input_viewport(&info.input, info.cursor)
            }
            TextTarget::ShortcutsFilter => {
                let info = self.shortcuts_panel.build_info(&self.shortcut_registry);
                self.shortcuts_panel
                    .input_viewport(&self.shortcuts_panel_layout(&info))
            }
            TextTarget::SettingsFilter => {
                self.settings_panel.input_viewport(&self.settings_layout())
            }
            TextTarget::WorkspaceRename => self
                .workspace_panel
                .rename_input_viewport(&self.workspace_panel_layout()),
            TextTarget::CommandsForm => self.commands_ime_viewport(&metrics),
            TextTarget::DialogField
            | TextTarget::TabRename
            | TextTarget::AgentView
            | TextTarget::Search
            | TextTarget::Terminal
            | TextTarget::Blocked => None,
        }
    }

    fn commands_ime_viewport(
        &self,
        metrics: &UiMetrics,
    ) -> Option<crate::ui::text_input_viewport::TextInputViewport> {
        // Only form state is needed here; do not clone the command inventory
        // a second time just to position the input method.
        let info = self.commands_panel.build_info(&[]);
        crate::ui::commands::panel::CommandsPanelLayout::calculate(
            metrics.width,
            metrics.height,
            metrics.tab_bar_height,
            metrics.cell_width,
            metrics.cell_height,
            info.animation,
            true,
            info.commands.len(),
        )
        .input_viewport(&info, info.edit_mode.clone())
    }

    /// Keep the native candidate window and preedit decoration on the focused
    /// input's shared geometry, not the terminal cursor behind a palette.
    pub(super) fn ime_preedit_render_info(
        &mut self,
    ) -> Option<crate::renderer::ImePreeditRenderInfo> {
        let target = self.text_target();
        if target == TextTarget::Blocked {
            return None;
        }
        if target == TextTarget::TabRename {
            return self.tab_rename_ime_target();
        }
        self.renderer.as_ref()?;
        if target == TextTarget::DialogField {
            let view = self.dialog_field_viewport()?;
            let metrics = self.ui_metrics();
            return self.ime_target(
                [
                    view.cursor_x,
                    view.text_y,
                    metrics.cell_width,
                    metrics.cell_height,
                ],
                (view.right - view.cursor_x).max(metrics.cell_width),
            );
        }
        let renderer = self.renderer.as_ref()?;
        let width = renderer.window_size().0;
        let (cell_width, cell_height) = renderer.cell_size();
        let view = match target {
            TextTarget::AgentView => {
                let scene = self.agent_view.scene.as_ref()?;
                let caret = scene.caret?.bounds;
                return self.ime_target(
                    [caret.x, caret.y, caret.width, caret.height],
                    (scene.panel.right() - caret.x).max(cell_width),
                );
            }
            TextTarget::Search => {
                let info = self.search.build_info(self.search_cursor_pos);
                let layout = renderer.search_bar_layout(&info, width as f32);
                // A collapsed query has no visible caret; do not fall back to a
                // terminal position or overlap the neighboring search controls.
                layout.input_viewport(&info, cell_width)?
            }
            TextTarget::Terminal => return self.terminal_ime_target(renderer),
            field => self.field_ime_viewport(field)?,
        };
        self.ime_target(
            [view.cursor_x, view.text_y, cell_width, cell_height],
            view.right - view.cursor_x,
        )
    }

    /// The input method's caret at the caret the tab bar draws after the
    /// title being renamed. A tab scrolled out of the strip has none, and
    /// the input method is never sent to the terminal cursor instead.
    fn tab_rename_ime_target(&mut self) -> Option<crate::renderer::ImePreeditRenderInfo> {
        self.tab_rename.as_ref()?;
        let tab_info = self.tab_bar_info();
        let renderer = self.renderer.as_mut()?;
        let (width, height) = renderer.window_size();
        let caret = renderer.tab_rename_caret(&tab_info, width as f32, height as f32)?;
        // The preedit floats over the tab bar from the caret, with the rest
        // of the window's width to show the composed text in.
        let max_width = (width as f32 - caret.x).max(renderer.cell_width());
        self.ime_target([caret.x, caret.y, caret.width, caret.height], max_width)
    }

    /// The input method's caret at the focused pane's drawn cursor.
    fn terminal_ime_target(
        &self,
        renderer: &crate::renderer::Renderer,
    ) -> Option<crate::renderer::ImePreeditRenderInfo> {
        let panes = &self.tabs.as_ref()?.active_tab()?.panes;
        let terminal = &panes.focused_pane()?.terminal;
        // The cursor that is drawn: synchronized output freezes the
        // presentation grid while the live one moves on.
        let cursor = terminal.presentation_grid().cursor();
        let rect = *panes.calculate_layout().get(&panes.focused_id())?;
        let bounds = renderer
            .pane_placement()
            .pane_bounds(rect, panes.pane_count() == 1);
        let cursor_offset = cursor.col as f32 * renderer.cell_width();
        let max_width = (bounds.width - cursor_offset).max(renderer.cell_width());
        let x = bounds.x + cursor_offset;
        let y = bounds.y + cursor.row as f32 * renderer.cell_height();
        self.ime_target(
            [x, y, renderer.cell_width(), renderer.cell_height()],
            max_width,
        )
    }

    /// The preedit to draw for text composed at `caret` (x, y, width, and
    /// height in window pixels), with `max_width` to fit in, after telling
    /// the input method where the text is. While composing, that is the
    /// preedit box Termeric draws, so the input method's candidate window
    /// opens beside the text being composed rather than over it.
    fn ime_target(
        &self,
        caret: [f32; 4],
        max_width: f32,
    ) -> Option<crate::renderer::ImePreeditRenderInfo> {
        let info = self
            .ime
            .preedit()
            .map(|preedit| crate::renderer::ImePreeditRenderInfo {
                text: preedit.text.clone(),
                cursor: preedit.cursor,
                x: caret[0],
                y: caret[1],
                max_width,
            });
        let [x, y, width, height] = match (&info, &self.renderer) {
            (Some(info), Some(renderer)) => renderer.ime_preedit_bounds(info),
            _ => caret,
        };
        if let Some(window) = &self.window {
            window.set_ime_cursor_area(
                winit::dpi::PhysicalPosition::new(x.round() as i32, y.round() as i32),
                winit::dpi::PhysicalSize::new(
                    width.ceil().max(1.0) as u32,
                    height.ceil().max(1.0) as u32,
                ),
            );
        }
        info
    }
}

#[cfg(test)]
mod tests;
