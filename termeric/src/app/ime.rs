//! Platform input-method state and commit routing.
//!
//! Preedit text is application-owned display state. It is never written to a
//! PTY; only `Ime::Commit` reaches the currently focused text surface.

use super::modal_layer::ModalLayer;
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
                if self.accepts_ime_text() {
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

    /// Return whether the current focus owner is a text destination. Modal
    /// surfaces without an editable field intentionally swallow IME input.
    fn accepts_ime_text(&self) -> bool {
        let top = self.top_modal_layer();
        if top == Some(ModalLayer::InfoOverlay) {
            return self.info_overlay.input_focused();
        }
        if matches!(self.broadcast, BroadcastState::Armed { .. }) {
            return false;
        }
        if let Some(layer) = top {
            return match layer {
                ModalLayer::DomainPicker
                | ModalLayer::UnicodePicker
                | ModalLayer::CommandPalette => self
                    .open_palette_modal()
                    .is_some_and(|modal| self.palette_owner(modal).query_editable()),
                ModalLayer::ShortcutsPanel => !self.shortcuts_panel.is_recording(),
                ModalLayer::Settings => !self.settings_panel.is_dropdown_open(),
                ModalLayer::SessionPanel => self.session_panel.is_renaming(),
                ModalLayer::CommandsPanel => self.commands_panel.is_editing(),
                // Like every key, IME text must not reach a field beneath a
                // layer that has none of its own.
                ModalLayer::Guardrail
                | ModalLayer::InfoOverlay
                | ModalLayer::DiffReview
                | ModalLayer::ContextPicker
                | ModalLayer::SproutMenu
                | ModalLayer::Cellar
                | ModalLayer::WorkspaceMenu
                | ModalLayer::AgentsPanel => false,
            };
        }
        if self.tab_rename.is_some() {
            return true;
        }
        if self.agent_view_has_keyboard() {
            return self.agent_view_accepts_text();
        }
        if self.search.is_active() {
            return true;
        }
        !self.copy_mode.is_active()
            && !self.focused_pane_read_only()
            && self.active_terminal().is_some()
    }

    fn commit_ime_text(&mut self, text: &str) {
        if text.is_empty() || !self.accepts_ime_text() {
            return;
        }

        if self.info_overlay.input_focused() {
            let request_id = self.info_overlay.request_id();
            self.info_overlay.insert_input_text(request_id, text);
        } else if let Some(modal) = self.open_palette_modal() {
            self.palette_owner_mut(modal).insert_query_text(text);
        } else if self.shortcuts_panel.is_active() {
            self.shortcuts_panel.insert_filter_text(text);
        } else if self.settings_panel.is_active() {
            self.settings_panel.insert_filter_text(text);
        } else if self.session_panel.is_open() {
            self.session_panel.insert_rename_text(text);
        } else if self.commands_panel.is_open() {
            self.commands_panel.insert_form_text(text);
        } else if self.tab_rename.is_some() {
            self.insert_tab_rename_text(text);
        } else if self.agent_view_has_keyboard() {
            self.commit_agent_view_text(text);
        } else if self.search.is_active() {
            let cursor = self.search_cursor_pos;
            if self
                .with_search_and_active_grid(|search, grid| search.insert_text(cursor, text, grid))
                .unwrap_or(false)
            {
                self.search_cursor_pos += text.len();
            }
        } else {
            let encoded = crate::input::encode_ime_text(self.active_kitty_keyboard_flags(), text);
            self.write_to_pty(&encoded);
        }
    }

    fn palette_ime_viewport(
        &self,
        width: f32,
        height: f32,
        cell_width: f32,
        cell_height: f32,
    ) -> Option<crate::ui::text_input_viewport::TextInputViewport> {
        let owner = self.palette_owner(self.open_palette_modal()?);
        if !owner.query_editable() {
            return None;
        }
        let info = owner.build_info();
        crate::ui::command_palette::PaletteLayout::calculate(
            width,
            height,
            cell_width,
            cell_height,
            info.animation,
            info.entries.len(),
        )
        .input_viewport(&info.input, info.cursor)
    }

    /// Match commit_ime_text's destination priority: the agent view's
    /// message box yields to every modal surface and rename field above it.
    fn agent_view_owns_ime_text(&self) -> bool {
        self.top_modal_layer().is_none()
            && self.tab_rename.is_none()
            && self.agent_view_has_keyboard()
            && self.agent_view_accepts_text()
            && self.accepts_ime_text()
    }

    /// Match commit_ime_text's destination priority: a retained search must
    /// not steal candidate geometry from a higher-priority text surface.
    fn search_owns_ime_text(&self) -> bool {
        self.top_modal_layer().is_none()
            && self.tab_rename.is_none()
            && self.search.is_active()
            && self.accepts_ime_text()
    }

    fn commands_owns_ime_text(&self) -> bool {
        self.modal_layer_on_top(ModalLayer::CommandsPanel)
            && self.commands_panel.is_editing()
            && self.accepts_ime_text()
    }

    fn shortcuts_owns_ime_text(&self) -> bool {
        self.modal_layer_on_top(ModalLayer::ShortcutsPanel)
            && !self.shortcuts_panel.is_recording()
            && self.accepts_ime_text()
    }

    fn settings_owns_ime_text(&self) -> bool {
        self.modal_layer_on_top(ModalLayer::Settings)
            && !self.settings_panel.is_dropdown_open()
            && self.accepts_ime_text()
    }

    fn settings_ime_viewport(
        &self,
        width: f32,
        height: f32,
        cell_width: f32,
        cell_height: f32,
    ) -> Option<crate::ui::text_input_viewport::TextInputViewport> {
        self.settings_owns_ime_text()
            .then(|| {
                self.settings_panel
                    .input_viewport(width, height, cell_width, cell_height)
            })
            .flatten()
    }

    fn session_owns_ime_text(&self) -> bool {
        self.modal_layer_on_top(ModalLayer::SessionPanel)
            && self.session_panel.is_renaming()
            && self.accepts_ime_text()
    }

    fn session_ime_viewport(
        &self,
        width: f32,
        height: f32,
        cell_width: f32,
        cell_height: f32,
    ) -> Option<crate::ui::text_input_viewport::TextInputViewport> {
        let top = self
            .renderer
            .as_ref()
            .map_or(0.0, crate::renderer::Renderer::tab_bar_height);
        self.session_owns_ime_text()
            .then(|| {
                self.session_panel.rename_input_viewport(
                    width,
                    height,
                    top,
                    cell_width,
                    cell_height,
                )
            })
            .flatten()
    }

    fn shortcuts_ime_viewport(
        &self,
        width: f32,
        height: f32,
        cell_width: f32,
        cell_height: f32,
    ) -> Option<crate::ui::text_input_viewport::TextInputViewport> {
        self.shortcuts_owns_ime_text()
            .then(|| {
                self.shortcuts_panel
                    .input_viewport(width, height, cell_width, cell_height)
            })
            .flatten()
    }

    fn commands_ime_viewport(
        &self,
        width: f32,
        height: f32,
        cell_width: f32,
        cell_height: f32,
    ) -> Option<crate::ui::text_input_viewport::TextInputViewport> {
        if !self.commands_owns_ime_text() {
            return None;
        }
        // Only form state is needed here; do not clone the command inventory
        // a second time just to position the input method.
        let info = self.commands_panel.build_info(&[]);
        let top = self
            .renderer
            .as_ref()
            .map_or(0.0, crate::renderer::Renderer::tab_bar_height);
        crate::ui::commands::panel::CommandsPanelLayout::calculate(
            width,
            height,
            top,
            cell_width,
            cell_height,
            info.animation,
            true,
            info.commands.len(),
        )
        .input_viewport(&info, info.edit_mode.clone())
    }

    fn search_ime_viewport(
        &self,
        layout: &crate::ui::search_bar::SearchBarLayout,
        cell_width: f32,
    ) -> Option<crate::ui::text_input_viewport::TextInputViewport> {
        self.search_owns_ime_text()
            .then(|| self.search.build_info(self.search_cursor_pos))
            .and_then(|info| layout.input_viewport(&info, cell_width))
    }

    /// Keep the native candidate window and preedit decoration on the focused
    /// input's shared geometry, not the terminal cursor behind a palette.
    pub(super) fn ime_preedit_render_info(
        &self,
        panes: &[crate::panes::PaneRenderInfo],
    ) -> Option<crate::renderer::ImePreeditRenderInfo> {
        if !self.accepts_ime_text() {
            return None;
        }
        let renderer = self.renderer.as_ref()?;
        if self.info_overlay.input_focused() {
            let request_id = self.info_overlay.request_id();
            let input = self.info_overlay.input_info(request_id)?;
            let (window_width, window_height) = renderer.window_size();
            let layout = crate::ui::info_overlay::InfoOverlayLayout::calculate(
                window_width as f32,
                window_height as f32,
                renderer.cell_width(),
                renderer.cell_height(),
                self.info_overlay.animation(),
                self.info_overlay.title(),
                self.info_overlay.lines().len(),
                self.info_overlay.max_line_chars(),
            );
            let row = layout.source_line_rect(
                input.source_line,
                self.info_overlay.scroll_offset(),
                self.info_overlay.lines().len(),
            )?;
            let cursor_column =
                input.label.chars().count() + input.value[..input.cursor_byte].chars().count();
            let x = row.x
                + cursor_column.saturating_sub(self.info_overlay.horizontal_offset()) as f32
                    * layout.char_scale;
            let y = row.y;
            return self.ime_target(
                [x, y, layout.char_scale, layout.line_height],
                (row.x + row.width - x).max(layout.char_scale),
            );
        }
        if self.open_palette_modal().is_some()
            || self.commands_owns_ime_text()
            || self.shortcuts_owns_ime_text()
            || self.settings_owns_ime_text()
            || self.session_owns_ime_text()
        {
            let (width, height) = renderer.window_size();
            let view = if self.commands_owns_ime_text() {
                self.commands_ime_viewport(
                    width as f32,
                    height as f32,
                    renderer.cell_width(),
                    renderer.cell_height(),
                )
            } else if self.shortcuts_owns_ime_text() {
                self.shortcuts_ime_viewport(
                    width as f32,
                    height as f32,
                    renderer.cell_width(),
                    renderer.cell_height(),
                )
            } else if self.settings_owns_ime_text() {
                self.settings_ime_viewport(
                    width as f32,
                    height as f32,
                    renderer.cell_width(),
                    renderer.cell_height(),
                )
            } else if self.session_owns_ime_text() {
                self.session_ime_viewport(
                    width as f32,
                    height as f32,
                    renderer.cell_width(),
                    renderer.cell_height(),
                )
            } else {
                self.palette_ime_viewport(
                    width as f32,
                    height as f32,
                    renderer.cell_width(),
                    renderer.cell_height(),
                )
            }?;
            return self.ime_target(
                [
                    view.cursor_x,
                    view.text_y,
                    renderer.cell_width(),
                    renderer.cell_height(),
                ],
                view.right - view.cursor_x,
            );
        }
        if self.agent_view_owns_ime_text() {
            let scene = self.agent_view.scene.as_ref()?;
            let caret = scene.caret?.bounds;
            return self.ime_target(
                [caret.x, caret.y, caret.width, caret.height],
                (scene.panel.right() - caret.x).max(renderer.cell_width()),
            );
        }
        if self.search_owns_ime_text() {
            let info = self.search.build_info(self.search_cursor_pos);
            let layout = renderer.search_bar_layout(&info, renderer.window_size().0 as f32);
            // A collapsed query has no visible caret; do not fall back to a
            // terminal position or overlap the neighboring search controls.
            let view = self.search_ime_viewport(&layout, renderer.cell_width())?;
            return self.ime_target(
                [
                    view.cursor_x,
                    view.text_y,
                    renderer.cell_width(),
                    renderer.cell_height(),
                ],
                view.right - view.cursor_x,
            );
        }
        let terminal = self.active_terminal()?;
        // The cursor that is drawn: synchronized output freezes the
        // presentation grid while the live one moves on.
        let cursor = terminal.presentation_grid().cursor();
        let content_top = renderer.content_top();
        let content_left = renderer.tab_bar_width();
        let focused_pane = panes.iter().find(|pane| pane.is_focused);
        let (origin_x, origin_y, max_width) = if let Some(pane) = focused_pane {
            let cursor_offset = cursor.col as f32 * renderer.cell_width();
            (
                content_left + renderer.padding() + pane.rect.x,
                content_top + pane.rect.y,
                (pane.rect.width - cursor_offset).max(renderer.cell_width()),
            )
        } else {
            let cursor_offset = cursor.col as f32 * renderer.cell_width();
            (
                content_left + renderer.padding(),
                content_top + renderer.padding(),
                (renderer.content_right()
                    - content_left
                    - renderer.padding() * 2.0
                    - cursor_offset)
                    .max(renderer.cell_width()),
            )
        };
        let x = origin_x + cursor.col as f32 * renderer.cell_width();
        let y = origin_y + cursor.row as f32 * renderer.cell_height();
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
