//! Platform input-method state and commit routing.
//!
//! Preedit text is application-owned display state. It is never written to a
//! PTY; only `Ime::Commit` reaches the currently focused text surface.

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
        self.redraw_scheduler
            .request(redraw_scheduler::RedrawReason::KeyboardInput);
        if let Some(window) = &self.window {
            window.request_redraw();
        }
    }

    /// Return whether the current focus owner is a text destination. Modal
    /// surfaces without an editable field intentionally swallow IME input.
    fn accepts_ime_text(&self) -> bool {
        if self.info_overlay.is_active() {
            return self.info_overlay.input_focused();
        }
        if matches!(self.broadcast, BroadcastState::Armed { .. })
            || self.guardrail.is_active()
            || self.diff_review.is_active()
            || self.context_picker.is_active()
            || self
                .harvest
                .as_ref()
                .is_some_and(|h| h.sprout_menu.is_some() || h.cellar.is_expanded())
        {
            return false;
        }
        if self.domain_picker.is_active() {
            return self.domain_picker.can_filter();
        }
        if self.command_palette.is_active() || self.unicode_picker.is_active() {
            return true;
        }
        if self.shortcuts_panel.is_active() {
            return !self.shortcuts_panel.is_recording();
        }
        if self.settings_panel.is_active() {
            return !self.settings_panel.is_dropdown_open();
        }
        if self.session_panel.is_open() {
            return self.session_panel.is_renaming();
        }
        if self.commands_panel.is_open() {
            return self.commands_panel.is_editing();
        }
        if self.agents_panel.is_open() {
            return false;
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
        } else if self.domain_picker.is_active() {
            self.domain_picker.insert_query_text(text);
        } else if self.command_palette.is_active() {
            self.command_palette.insert_query_text(text);
        } else if self.unicode_picker.is_active() {
            self.unicode_picker.insert_query_text(text);
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
    ) -> Option<crate::ui::command_palette::PaletteInputViewport> {
        let info = if self.domain_picker.is_active() {
            if !self.domain_picker.can_filter() {
                return None;
            }
            self.domain_picker.build_info()
        } else if self.command_palette.is_active() {
            self.command_palette.build_info()
        } else if self.unicode_picker.is_active() {
            self.unicode_picker.build_info()
        } else {
            return None;
        };
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
        self.agent_view_has_keyboard()
            && self.agent_view_accepts_text()
            && self.accepts_ime_text()
            && !self.info_overlay.is_active()
            && !self.domain_picker.is_active()
            && !self.command_palette.is_active()
            && !self.unicode_picker.is_active()
            && !self.shortcuts_panel.is_active()
            && !self.settings_panel.is_active()
            && !self.session_panel.is_open()
            && !self.commands_panel.is_open()
            && !self.agents_panel.is_open()
            && self.tab_rename.is_none()
    }

    /// Match commit_ime_text's destination priority: a retained search must
    /// not steal candidate geometry from a higher-priority text surface.
    fn search_owns_ime_text(&self) -> bool {
        self.search.is_active()
            && self.accepts_ime_text()
            && !self.info_overlay.is_active()
            && !self.domain_picker.is_active()
            && !self.command_palette.is_active()
            && !self.unicode_picker.is_active()
            && self.tab_rename.is_none()
            && !self.shortcuts_panel.is_active()
            && !self.settings_panel.is_active()
            && !self.session_panel.is_open()
            && !self.commands_panel.is_open()
            && !self.agents_panel.is_open()
    }

    fn commands_owns_ime_text(&self) -> bool {
        self.commands_panel.is_open()
            && self.commands_panel.is_editing()
            && self.accepts_ime_text()
            && !self.info_overlay.is_active()
            && !self.domain_picker.is_active()
            && !self.command_palette.is_active()
            && !self.unicode_picker.is_active()
            && !self.shortcuts_panel.is_active()
            && !self.settings_panel.is_active()
            && !self.session_panel.is_open()
    }

    fn shortcuts_owns_ime_text(&self) -> bool {
        self.shortcuts_panel.is_active()
            && !self.shortcuts_panel.is_recording()
            && self.accepts_ime_text()
            && !self.info_overlay.is_active()
            && !self.domain_picker.is_active()
            && !self.command_palette.is_active()
            && !self.unicode_picker.is_active()
    }

    fn settings_owns_ime_text(&self) -> bool {
        self.settings_panel.is_active()
            && !self.settings_panel.is_dropdown_open()
            && self.accepts_ime_text()
            && !self.info_overlay.is_active()
            && !self.domain_picker.is_active()
            && !self.command_palette.is_active()
            && !self.unicode_picker.is_active()
            && !self.shortcuts_panel.is_active()
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
        self.session_panel.is_open()
            && self.session_panel.is_renaming()
            && self.accepts_ime_text()
            && !self.info_overlay.is_active()
            && !self.domain_picker.is_active()
            && !self.command_palette.is_active()
            && !self.unicode_picker.is_active()
            && !self.shortcuts_panel.is_active()
            && !self.settings_panel.is_active()
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
        layout: &crate::terminal::search::SearchBarLayout,
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
        panes: &[crate::terminal::container::PaneRenderInfo],
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
            if let Some(window) = &self.window {
                window.set_ime_cursor_area(
                    winit::dpi::PhysicalPosition::new(x.round() as i32, y.round() as i32),
                    winit::dpi::PhysicalSize::new(
                        layout.char_scale.ceil().max(1.0) as u32,
                        layout.line_height.ceil().max(1.0) as u32,
                    ),
                );
            }
            let preedit = self.ime.preedit()?;
            return Some(crate::renderer::ImePreeditRenderInfo {
                text: preedit.text.clone(),
                cursor: preedit.cursor,
                x,
                y,
                max_width: (row.x + row.width - x).max(layout.char_scale),
            });
        }
        if self.domain_picker.is_active()
            || self.command_palette.is_active()
            || self.unicode_picker.is_active()
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
            if let Some(window) = &self.window {
                window.set_ime_cursor_area(
                    winit::dpi::PhysicalPosition::new(
                        view.cursor_x.round() as i32,
                        view.text_y.round() as i32,
                    ),
                    winit::dpi::PhysicalSize::new(
                        renderer.cell_width().ceil().max(1.0) as u32,
                        renderer.cell_height().ceil().max(1.0) as u32,
                    ),
                );
            }
            let preedit = self.ime.preedit()?;
            return Some(crate::renderer::ImePreeditRenderInfo {
                text: preedit.text.clone(),
                cursor: preedit.cursor,
                x: view.cursor_x,
                y: view.text_y,
                max_width: view.right - view.cursor_x,
            });
        }
        if self.agent_view_owns_ime_text() {
            let scene = self.agent_view.scene.as_ref()?;
            let caret = scene.caret?.bounds;
            if let Some(window) = &self.window {
                window.set_ime_cursor_area(
                    winit::dpi::PhysicalPosition::new(
                        caret.x.round() as i32,
                        caret.y.round() as i32,
                    ),
                    winit::dpi::PhysicalSize::new(
                        caret.width.ceil().max(1.0) as u32,
                        caret.height.ceil().max(1.0) as u32,
                    ),
                );
            }
            let preedit = self.ime.preedit()?;
            return Some(crate::renderer::ImePreeditRenderInfo {
                text: preedit.text.clone(),
                cursor: preedit.cursor,
                x: caret.x,
                y: caret.y,
                max_width: (scene.panel.right() - caret.x).max(renderer.cell_width()),
            });
        }
        if self.search_owns_ime_text() {
            let info = self.search.build_info(self.search_cursor_pos);
            let layout = renderer.search_bar_layout(&info, renderer.window_size().0 as f32);
            // A collapsed query has no visible caret; do not fall back to a
            // terminal position or overlap the neighboring search controls.
            let view = self.search_ime_viewport(&layout, renderer.cell_width())?;
            if let Some(window) = &self.window {
                window.set_ime_cursor_area(
                    winit::dpi::PhysicalPosition::new(
                        view.cursor_x.round() as i32,
                        view.text_y.round() as i32,
                    ),
                    winit::dpi::PhysicalSize::new(
                        renderer.cell_width().ceil().max(1.0) as u32,
                        renderer.cell_height().ceil().max(1.0) as u32,
                    ),
                );
            }
            let preedit = self.ime.preedit()?;
            return Some(crate::renderer::ImePreeditRenderInfo {
                text: preedit.text.clone(),
                cursor: preedit.cursor,
                x: view.cursor_x,
                y: view.text_y,
                max_width: view.right - view.cursor_x,
            });
        }
        let terminal = self.active_terminal()?;
        let cursor = terminal.grid().cursor();
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

        if let Some(window) = &self.window {
            window.set_ime_cursor_area(
                winit::dpi::PhysicalPosition::new(x.round() as i32, y.round() as i32),
                winit::dpi::PhysicalSize::new(
                    renderer.cell_width().ceil().max(1.0) as u32,
                    renderer.cell_height().ceil().max(1.0) as u32,
                ),
            );
        }

        let preedit = self.ime.preedit()?;
        Some(crate::renderer::ImePreeditRenderInfo {
            text: preedit.text.clone(),
            cursor: preedit.cursor,
            x,
            y,
            max_width,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_ime_matches_scrolled_filter_and_refuses_dropdown_or_higher_owners() {
        let mut app = App::new_headless(Config::default()).unwrap();
        app.search.start();
        app.commands_panel.toggle();
        app.commands_panel.start_add();
        app.settings_panel.set_reduced_motion(true);
        app.settings_panel.toggle();
        assert!(
            app.settings_panel
                .insert_filter_text(&"界e\u{301}👩\u{200d}💻".repeat(25))
        );
        for (width, cell) in [(800.0, 8.0), (1600.0, 16.0)] {
            let info = app
                .settings_panel
                .build_info(&app.config, &app.shortcut_registry);
            let layout = crate::ui::settings::SettingsModalLayout::calculate(
                width,
                800.0,
                cell,
                cell * 2.0,
                info.animation,
                info.entries.len(),
            );
            let view = app
                .settings_ime_viewport(width, 800.0, cell, cell * 2.0)
                .unwrap();
            assert_eq!(
                Some(view.clone()),
                layout.input_viewport(&info.filter, info.filter_cursor, cell * 2.0)
            );
            assert!(view.text_range.start > 0);
            assert!(!app.commands_owns_ime_text());
            assert!(!app.search_owns_ime_text());
        }
        assert!(app.settings_ime_viewport(40.0, 800.0, 8.0, 16.0).is_none());
        app.command_palette.open();
        assert!(!app.settings_owns_ime_text());
        app.command_palette.close();
        app.shortcuts_panel.open();
        assert!(!app.settings_owns_ime_text());
        app.shortcuts_panel.close_immediately();
        app.settings_panel.close();
        app.settings_panel
            .open_to_section(crate::ui::settings::SettingsSection::Theme);
        app.settings_panel
            .build_info(&app.config, &app.shortcut_registry);
        app.settings_panel.handle_key(
            crate::ui::settings::SettingsKey::Enter,
            &app.config,
            &app.shortcut_registry,
        );
        assert!(app.settings_panel.is_dropdown_open());
        assert!(!app.accepts_ime_text());
        assert!(app.settings_ime_viewport(800.0, 800.0, 8.0, 16.0).is_none());
        app.handle_ime_event(Ime::Preedit("draft".into(), None));
        assert!(app.ime.preedit().is_none());
        app.settings_panel.close();
        assert!(!app.settings_owns_ime_text());
    }

    #[test]
    fn shortcut_ime_uses_scrolled_filter_geometry_and_recording_blocks_retained_settings() {
        let mut app = App::new_headless(Config::default()).unwrap();
        app.search.start();
        app.commands_panel.toggle();
        app.commands_panel.start_add();
        app.settings_panel.toggle();
        app.shortcuts_panel.set_reduced_motion(true);
        app.shortcuts_panel.open();
        assert!(
            app.shortcuts_panel
                .insert_filter_text(&"界e\u{301}👩\u{200d}💻".repeat(20))
        );
        for (width, cell) in [(800.0, 8.0), (1600.0, 16.0)] {
            let info = app.shortcuts_panel.build_info(&app.shortcut_registry);
            let layout = crate::ui::shortcuts_panel::ShortcutsPanelLayout::calculate(
                width,
                800.0,
                cell,
                cell * 2.0,
                info.animation,
                info.entries.len(),
            );
            let view = app
                .shortcuts_ime_viewport(width, 800.0, cell, cell * 2.0)
                .unwrap();
            assert_eq!(
                view,
                layout
                    .input_viewport(&info.filter, info.cursor, cell, cell * 2.0)
                    .unwrap()
            );
            assert!(view.text_range.start > 0);
            assert!(view.cursor_x + cell <= view.right + 0.001);
            assert!(!app.search_owns_ime_text());
            assert!(!app.commands_owns_ime_text());
        }
        assert!(app.shortcuts_ime_viewport(40.0, 800.0, 8.0, 16.0).is_none());
        app.command_palette.open();
        assert!(
            app.shortcuts_ime_viewport(800.0, 800.0, 8.0, 16.0)
                .is_none()
        );
        app.command_palette.close();
        app.shortcuts_panel
            .start_recording(crate::input::keybindings::BindableAction::Copy);
        assert!(!app.accepts_ime_text());
        assert!(!app.shortcuts_owns_ime_text());
        app.handle_ime_event(Ime::Preedit("draft".into(), None));
        assert!(app.ime.preedit().is_none());
        app.shortcuts_panel.close();
        assert!(
            app.shortcuts_ime_viewport(800.0, 800.0, 8.0, 16.0)
                .is_none()
        );
    }

    #[test]
    fn command_form_ime_follows_the_active_scrolled_field_and_respects_modal_priority() {
        use crate::ui::commands::panel::{CommandsPanelLayout, EditMode};
        let mut app = App::new_headless(Config::default()).unwrap();
        app.commands_panel.set_reduced_motion(true);
        app.commands_panel.toggle();
        app.commands_panel.start_add();
        let text = "界e\u{301}👩\u{200d}💻".repeat(20);
        app.commands_panel.form_name.replace(&text);
        app.commands_panel.form_command.replace(&text);
        app.search.start(); // retained lower-priority query must not win
        for field in [EditMode::EditingName, EditMode::EditingCommand] {
            app.commands_panel.edit_mode = field.clone();
            for (width, cell) in [(800.0, 8.0), (1600.0, 16.0)] {
                let view = app
                    .commands_ime_viewport(width, 800.0, cell, cell * 2.0)
                    .unwrap();
                let info = app.commands_panel.build_info(&[]);
                let layout = CommandsPanelLayout::calculate(
                    width,
                    800.0,
                    0.0,
                    cell,
                    cell * 2.0,
                    info.animation,
                    true,
                    0,
                );
                assert_eq!(view, layout.input_viewport(&info, field.clone()).unwrap());
                assert!(view.text_range.start > 0);
                assert!(view.cursor_x + cell <= view.right + 0.001);
                assert!(!app.search_owns_ime_text());
            }
        }
        assert!(app.commands_ime_viewport(160.0, 800.0, 8.0, 16.0).is_none());
        app.command_palette.open();
        assert!(!app.commands_owns_ime_text());
        assert!(app.commands_ime_viewport(800.0, 800.0, 8.0, 16.0).is_none());
        app.command_palette.close();
        app.info_overlay.show("Notice", vec![]);
        assert!(!app.commands_owns_ime_text());
        app.commands_panel.close();
        assert!(app.commands_ime_viewport(800.0, 800.0, 8.0, 16.0).is_none());
    }

    #[test]
    fn search_ime_caret_uses_query_columns_and_shared_shifted_scaled_layout() {
        use crate::terminal::search::SearchBarLayout;
        let mut app = App::new_headless(Config::default()).unwrap();
        let terminal = termeric_core::Terminal::new(80, 24);
        app.search.start();
        for query in [
            "界e\u{301}👩\u{200d}💻".to_string(),
            "界e\u{301}👩\u{200d}💻".repeat(80),
        ] {
            assert!(app.search.set_query(&query, terminal.grid()));
            for (left, width, top, cell) in [
                (0.0, 800.0, 30.0, 8.0),
                (160.0, 640.0, 54.0, 10.0),
                (240.0, 1100.0, 80.0, 16.0),
            ] {
                for cursor in [0, "界".len(), query.len()] {
                    app.search_cursor_pos = cursor;
                    let info = app.search.build_info(cursor);
                    let layout =
                        SearchBarLayout::calculate(&info, left, width, top, cell, cell * 2.0);
                    let view = app.search_ime_viewport(&layout, cell).unwrap();
                    assert_eq!(view, layout.input_viewport(&info, cell).unwrap());
                    assert_eq!(view.text_y, layout.text_y);
                    assert!(layout.bar.y > top, "the box floats inside the content");
                    assert!(view.cursor_x + cell <= layout.input.x + layout.input.width + 0.001);
                    assert!(view.text_range.end <= query.len());
                    if cursor == query.len() {
                        assert_eq!(view.text_range.end, query.len());
                    }
                    if cursor == "界".len() {
                        assert_eq!(view.cursor_x, view.text_x + 2.0 * cell);
                    }
                    if query.chars().count() == 6 && cursor == query.len() {
                        assert_eq!(view.cursor_x, view.text_x + 5.0 * cell);
                    }
                }
            }
        }
        let info = app.search.build_info(app.search_cursor_pos);
        let collapsed = SearchBarLayout::calculate(&info, 0.0, 100.0, 30.0, 10.0, 20.0);
        assert!(app.search_ime_viewport(&collapsed, 10.0).is_none());
    }

    #[test]
    fn retained_search_does_not_steal_ime_geometry_from_modal_or_text_owners() {
        let openers: [fn(&mut App); 9] = [
            |app| app.command_palette.open(),
            |app| app.unicode_picker.open(),
            |app| app.settings_panel.toggle(),
            |app| app.shortcuts_panel.open(),
            |app| app.commands_panel.toggle(),
            |app| app.session_panel.open_workspaces(),
            |app| {
                app.info_overlay.show("Notice", vec![]);
            },
            |app| app.domain_picker.open(&app.config.domains),
            |app| {
                app.tab_rename = Some(TabRenameState {
                    tab_id: 0,
                    original: None,
                    input: String::new(),
                    max_chars: 80,
                })
            },
        ];
        for open in openers {
            let mut app = App::new_headless(Config::default()).unwrap();
            app.search.start();
            assert!(app.search_owns_ime_text());
            open(&mut app);
            assert!(!app.search_owns_ime_text());
        }
        let mut app = App::new_headless(Config::default()).unwrap();
        app.search.start();
        app.search.confirm();
        assert!(!app.search_owns_ime_text());
    }

    #[cfg(unix)]
    #[test]
    fn search_ime_commit_inserts_at_query_caret_without_leaking_into_pty() {
        let mut tabs = TabManager::new(
            80,
            24,
            crate::config::ShellConfig::default(),
            100,
            1024 * 1024,
        );
        tabs.new_tab_with_command("/bin/sh", &[
            "-c".into(), "stty raw -echo; printf READY; dd bs=1 count=1 2>/dev/null | od -An -tx1; sleep 0.1".into(),
        ], None, Some("search-ime"), None, Default::default()).unwrap();
        let mut app = App::new_headless(Config::default()).unwrap();
        app.tabs = Some(tabs);
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            app.process_pty_events();
            if app
                .active_terminal()
                .unwrap()
                .rows_text(0, 23)
                .contains("READY")
            {
                break;
            }
            assert!(Instant::now() < deadline);
            std::thread::yield_now();
        }
        app.search.start();
        assert!(
            app.with_search_and_active_grid(|search, grid| search.set_query("a界b", grid))
                .unwrap()
        );
        app.search_cursor_pos = 1;
        app.handle_ime_event(Ime::Preedit("日本".into(), None));
        assert_eq!(app.search.query(), "a界b");
        app.handle_ime_event(Ime::Commit("日本".into()));
        assert_eq!(app.search.query(), "a日本界b");
        assert_eq!(app.search_cursor_pos, "a日本".len());
        app.search.cancel();
        app.write_to_pty(b"Z");
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            app.process_pty_events();
            let text = app.active_terminal().unwrap().rows_text(0, 23);
            if text.contains("5a") {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "query input leaked before the Z fence: {text}"
            );
            std::thread::yield_now();
        }
    }

    #[test]
    fn workspace_rename_ime_matches_paint_geometry_without_a_terminal() {
        let mut app = App::new_headless(Config::default()).unwrap();
        app.session_panel.set_reduced_motion(true);
        app.session_panel.set_test_entries(
            vec![crate::session::panel::SessionEntry {
                id: "workspace-test".into(),
                display_name: "Roots".into(),
                subtitle: String::new(),
                last_modified: String::new(),
            }],
            true,
        );
        app.session_panel.begin_rename("workspace-test");
        let name = "界e\u{301}👩\u{200d}💻".repeat(16);
        assert!(app.session_panel.accessibility_set_rename(&name));
        for (width, cell) in [(800.0, 8.0), (1400.0, 16.0)] {
            let info = app.session_panel.build_info();
            let layout = crate::session::panel::SessionPanelLayout::calculate(
                width,
                800.0,
                0.0,
                cell,
                cell * 2.0,
                info.animation,
                true,
                true,
                info.sessions.len(),
            );
            let expected = layout.rename_input_viewport(&name, name.len()).unwrap();
            assert_eq!(
                app.session_ime_viewport(width, 800.0, cell, cell * 2.0),
                Some(expected.clone())
            );
            assert!(expected.text_range.start > 0);
        }
        assert!(app.session_ime_viewport(40.0, 600.0, 10.0, 20.0).is_none());
        app.search.start();
        assert!(!app.search_owns_ime_text());
        app.command_palette.open();
        assert!(app.session_ime_viewport(800.0, 600.0, 10.0, 20.0).is_none());
        app.command_palette.close();
        app.domain_picker.open(&app.config.domains);
        assert!(app.session_ime_viewport(800.0, 600.0, 10.0, 20.0).is_none());
        app.close_domain_picker();
        app.settings_panel.toggle();
        assert!(app.session_ime_viewport(800.0, 600.0, 10.0, 20.0).is_none());
        app.settings_panel.toggle();
        app.session_panel.cancel_rename();
        assert!(app.session_ime_viewport(800.0, 600.0, 10.0, 20.0).is_none());
        assert!(app.active_terminal().is_none());
        assert_eq!(app.session_panel.sessions()[0].display_name, "Roots");
    }

    #[test]
    fn domain_picker_ime_owns_editable_stages_and_blocks_busy_background_input() {
        use crate::ui::domain_picker::DomainPickerCommand;
        let mut app = App::new_headless(Config::default()).unwrap();
        app.command_palette.open();
        app.command_palette.accessibility_set_input("background");
        app.domain_picker.open(&app.config.domains);
        app.search.start();
        app.settings_panel.toggle();
        assert!(!app.search_owns_ime_text());
        assert!(!app.settings_owns_ime_text());
        app.handle_ime_event(Ime::Preedit("日本".into(), None));
        assert!(app.ime.preedit().is_some());
        app.handle_ime_event(Ime::Commit("a界b".into()));
        app.handle_palette_edit_key(&winit::keyboard::Key::Named(
            winit::keyboard::NamedKey::ArrowLeft,
        ));
        app.handle_ime_event(Ime::Commit("e\u{301}".into()));
        assert_eq!(app.domain_picker.build_info().input, "a界e\u{301}b");
        for text in ["bad\nquery".into(), "x".repeat(257)] {
            app.handle_ime_event(Ime::Commit(text));
            assert_eq!(app.domain_picker.build_info().input, "a界e\u{301}b");
        }
        app.domain_picker.accessibility_set_filter("");
        let Some(DomainPickerCommand::Discover { generation, .. }) = app.domain_picker.activate()
        else {
            panic!("expected discovery");
        };
        let before = app.domain_picker.build_info().input;
        app.handle_ime_event(Ime::Preedit("blocked".into(), None));
        app.handle_ime_event(Ime::Commit("blocked".into()));
        assert!(app.ime.preedit().is_none());
        assert_eq!(app.domain_picker.build_info().input, before);
        assert!(app.palette_ime_viewport(800.0, 600.0, 10.0, 20.0).is_none());
        app.handle_domain_picker_loaded(
            generation,
            Ok(vec![crate::domain::model::DomainSummary {
                id: "domain-test".into(),
                name: "日本".into(),
                workspace_id: "workspace-test".into(),
                tabs: 1,
                panes: 1,
                attachments: 0,
                created_at: "2026-01-01T00:00:00Z".into(),
            }]),
        );
        app.handle_ime_event(Ime::Preedit("日".into(), None));
        let preedit = app.ime.preedit().cloned();
        assert!(preedit.is_some());
        app.handle_domain_picker_loaded(generation.wrapping_sub(1), Err("stale".into()));
        assert_eq!(app.ime.preedit(), preedit.as_ref());
        app.handle_ime_event(Ime::Commit("日本".into()));
        assert_eq!(app.domain_picker.build_info().input, "日本");
        let Some(DomainPickerCommand::PrepareWindow { generation, .. }) =
            app.domain_picker.activate()
        else {
            panic!("expected preparation");
        };
        app.handle_ime_event(Ime::Preedit("blocked".into(), None));
        app.handle_ime_event(Ime::Commit("blocked".into()));
        assert!(app.ime.preedit().is_none());
        assert!(!app.accepts_ime_text());
        app.handle_domain_picker_prepared(generation, Err("test failure".into()));
        assert!(!app.accepts_ime_text(), "error stage is not a filter");
        app.domain_picker.back_or_close();
        assert!(app.accepts_ime_text());
        assert_eq!(app.command_palette.build_info().input, "background");
        assert!(app.requested_window_launch.is_none());
    }

    #[test]
    fn domain_picker_ime_geometry_uses_its_query_even_over_retained_panels() {
        let mut app = App::new_headless(Config::default()).unwrap();
        app.command_palette.open();
        app.command_palette.accessibility_set_input("background");
        app.domain_picker.set_reduced_motion(true);
        app.domain_picker.open(&app.config.domains);
        let query = "界e\u{301}👩\u{200d}💻".repeat(30);
        assert!(app.domain_picker.accessibility_set_filter(&query));
        for (width, cell_width) in [(450.0, 8.0), (800.0, 10.0), (1400.0, 20.0)] {
            let info = app.domain_picker.build_info();
            let expected = crate::ui::command_palette::PaletteLayout::calculate(
                width,
                900.0,
                cell_width,
                24.0,
                info.animation,
                info.entries.len(),
            )
            .input_viewport(&info.input, info.cursor)
            .unwrap();
            assert_eq!(
                app.palette_ime_viewport(width, 900.0, cell_width, 24.0),
                Some(expected.clone())
            );
            assert!(expected.text_range.start > 0);
            assert!(expected.cursor_x + cell_width <= expected.right + 0.001);
        }
    }

    #[test]
    fn palette_ime_geometry_tracks_the_visible_query_without_a_terminal() {
        for unicode in [false, true] {
            let mut app = App::new_headless(Config::default()).unwrap();
            app.command_palette.set_reduced_motion(true);
            app.unicode_picker.set_reduced_motion(true);
            let query = "界e\u{301}👩\u{200d}💻".repeat(30);
            if unicode {
                app.unicode_picker.open();
                app.unicode_picker.accessibility_set_filter(&query);
            } else {
                app.command_palette.open();
                app.command_palette.accessibility_set_input(&query);
            }
            for (width, cell_width) in [(450.0, 8.0), (800.0, 10.0), (1400.0, 20.0)] {
                let view = app
                    .palette_ime_viewport(width, 900.0, cell_width, 24.0)
                    .unwrap();
                let info = if unicode {
                    app.unicode_picker.build_info()
                } else {
                    app.command_palette.build_info()
                };
                let expected = crate::ui::command_palette::PaletteLayout::calculate(
                    width,
                    900.0,
                    cell_width,
                    24.0,
                    info.animation,
                    info.entries.len(),
                )
                .input_viewport(&info.input, info.cursor)
                .unwrap();
                assert_eq!(view, expected);
                assert!(view.text_range.start > 0);
                assert_eq!(view.text_range.end, query.len());
                assert!(view.cursor_x + cell_width <= view.right + 0.001);
            }
            app.command_palette.close();
            app.unicode_picker.close();
            assert!(app.palette_ime_viewport(800.0, 600.0, 10.0, 20.0).is_none());
            app.unicode_picker.finish_search_for_test();
        }
    }

    #[test]
    fn palette_ime_commits_are_atomic_at_query_limits_and_reject_multiline_text() {
        for unicode in [false, true] {
            let mut app = App::new_headless(Config::default()).unwrap();
            if unicode {
                app.unicode_picker.open();
            } else {
                app.command_palette.open();
            }
            app.handle_ime_event(Ime::Commit("界".repeat(300)));
            let info = if unicode {
                app.unicode_picker.build_info()
            } else {
                app.command_palette.build_info()
            };
            assert_eq!(info.input, "");
            assert_eq!(info.cursor, info.input.len());
            assert!(app.ime.preedit().is_none());
            let prefix = "界".repeat(255);
            app.handle_ime_event(Ime::Commit(prefix.clone()));
            app.handle_palette_edit_key(&winit::keyboard::Key::Named(
                winit::keyboard::NamedKey::Home,
            ));
            for text in ["xy", "a\nb", "\u{2028}", "\u{2029}", "\t", "\x1b"] {
                app.handle_ime_event(Ime::Commit(text.into()));
                let info = if unicode {
                    app.unicode_picker.build_info()
                } else {
                    app.command_palette.build_info()
                };
                assert_eq!(info.input, prefix, "{text:?}");
                assert_eq!(info.cursor, 0);
            }
            app.handle_ime_event(Ime::Commit("🫚".into()));
            let info = if unicode {
                app.unicode_picker.build_info()
            } else {
                app.command_palette.build_info()
            };
            assert_eq!(info.input, format!("🫚{prefix}"));
            assert_eq!(info.cursor, "🫚".len());
            app.unicode_picker.close();
            app.unicode_picker.finish_search_for_test();
        }
    }

    #[test]
    fn preedit_cursor_must_be_a_valid_utf8_range() {
        let mut state = ImeState::default();
        state.set_preedit("日本".into(), Some((0, 3)));
        assert_eq!(state.preedit().unwrap().cursor, Some((0, 3)));
        state.set_preedit("日本".into(), Some((1, 3)));
        assert_eq!(state.preedit().unwrap().cursor, None);
    }

    fn exercise_retained_rename_modal_ownership(app: &mut App) {
        use crate::session::panel::{SessionEntry, SessionPanelAction};
        use winit::keyboard::{Key, NamedKey};
        for owner in ["settings", "shortcuts", "session", "commands"] {
            app.tab_rename = Some(TabRenameState {
                tab_id: app
                    .tabs
                    .as_ref()
                    .and_then(TabManager::active_tab_id)
                    .unwrap_or(usize::MAX),
                original: None,
                input: "retained".into(),
                max_chars: 256,
            });
            app.search.start();
            match owner {
                "settings" => {
                    app.settings_panel.set_reduced_motion(true);
                    app.settings_panel.toggle();
                }
                "shortcuts" => {
                    app.shortcuts_panel.set_reduced_motion(true);
                    app.shortcuts_panel.open();
                }
                "session" => {
                    app.session_panel.set_reduced_motion(true);
                    app.session_panel.set_test_entries(
                        vec![SessionEntry {
                            id: "modal-owner-fixture".into(),
                            display_name: "Roots".into(),
                            subtitle: String::new(),
                            last_modified: String::new(),
                        }],
                        true,
                    );
                    app.apply_session_panel_action(SessionPanelAction::Rename(
                        "modal-owner-fixture".into(),
                    ));
                    assert!(app.session_panel.accessibility_set_rename(""));
                }
                _ => {
                    app.commands_panel.set_reduced_motion(true);
                    app.commands_panel.toggle();
                    app.commands_panel.start_add();
                }
            }
            let key = Key::Character("K".into());
            assert!(match owner {
                "settings" => app.handle_settings_filter_key(&key),
                "shortcuts" => app.handle_shortcuts_filter_key(&key),
                "session" => app.handle_session_rename_key(&key),
                _ => app.handle_commands_panel_key(&key),
            });
            app.handle_ime_event(Ime::Preedit("draft".into(), None));
            assert!(app.ime.preedit().is_some());
            let view = match owner {
                "settings" => app.settings_ime_viewport(1000.0, 800.0, 8.0, 16.0),
                "shortcuts" => app.shortcuts_ime_viewport(1000.0, 800.0, 8.0, 16.0),
                "session" => app.session_ime_viewport(1000.0, 800.0, 8.0, 16.0),
                _ => app.commands_ime_viewport(1000.0, 800.0, 8.0, 16.0),
            };
            assert!(view.is_some(), "missing {owner} candidate viewport");
            app.handle_ime_event(Ime::Commit("I".into()));
            assert!(app.paste_clipboard_payload("P"));
            let text = match owner {
                "settings" => {
                    app.settings_panel
                        .build_info(&app.config, &app.shortcut_registry)
                        .filter
                }
                "shortcuts" => {
                    app.shortcuts_panel
                        .build_info(&app.shortcut_registry)
                        .filter
                }
                "session" => app.session_panel.build_info().rename_text.unwrap(),
                _ => app.commands_panel.form_name.text().to_owned(),
            };
            assert_eq!(text, "KIP", "wrong {owner} input destination");
            assert_eq!(app.tab_rename.as_ref().unwrap().input, "retained");
            assert!(app.search.query().is_empty());
            // A frontmost read-only state must consume input, not expose
            // the retained rename behind it as an IME or paste destination.
            match owner {
                "settings" => {
                    app.settings_panel
                        .open_to_section(crate::ui::settings::SettingsSection::Theme);
                    app.settings_panel
                        .build_info(&app.config, &app.shortcut_registry);
                    app.settings_panel.handle_key(
                        crate::ui::settings::SettingsKey::Right,
                        &app.config,
                        &app.shortcut_registry,
                    );
                    assert!(app.settings_panel.is_dropdown_open());
                }
                "shortcuts" => app
                    .shortcuts_panel
                    .start_recording(crate::input::keybindings::BindableAction::Copy),
                "session" => app.apply_session_panel_action(SessionPanelAction::CancelRename),
                _ => {
                    app.commands_panel.cancel_edit();
                    assert!(app.handle_commands_panel_key(&Key::Character("blocked".into())));
                    assert!(app.handle_commands_panel_key(&Key::Named(NamedKey::Backspace)));
                }
            }
            assert!(!app.accepts_ime_text());
            app.handle_ime_event(Ime::Preedit("blocked".into(), None));
            assert!(app.ime.preedit().is_none());
            app.handle_ime_event(Ime::Commit("blocked".into()));
            assert!(!app.paste_clipboard_payload("blocked"));
            assert_eq!(app.tab_rename.as_ref().unwrap().input, "retained");
            assert!(app.search.query().is_empty());
            match owner {
                "settings" => app.settings_panel.close(),
                "shortcuts" => app.shortcuts_panel.close_immediately(),
                "session" => app.apply_session_panel_action(SessionPanelAction::Close),
                _ => {
                    assert!(app.handle_commands_panel_key(&Key::Named(NamedKey::Escape)));
                }
            }
            app.handle_ime_event(Ime::Commit(" resumed".into()));
            assert_eq!(app.tab_rename.as_ref().unwrap().input, "retained resumed");
            assert!(app.paste_clipboard_payload(" pasted"));
            assert_eq!(
                app.tab_rename.as_ref().unwrap().input,
                "retained resumed pasted"
            );
            app.tab_rename = None;
            app.search.cancel();
        }
        assert!(app.config.commands.is_empty());
    }

    #[test]
    fn frontmost_panels_own_keyboard_ime_geometry_and_paste_ahead_of_retained_tab_rename() {
        let mut app = App::new_headless(Config::default()).unwrap();
        exercise_retained_rename_modal_ownership(&mut app);
    }

    #[cfg(unix)]
    #[test]
    fn modal_atomic_commits_never_fall_through_to_a_live_pty() {
        let mut tabs = TabManager::new(
            80,
            24,
            crate::config::ShellConfig::default(),
            100,
            1024 * 1024,
        );
        tabs.new_tab_with_command("/bin/sh", &[
            "-c".into(), "stty raw -echo; printf READY; dd bs=1 count=1 2>/dev/null | od -An -tx1; sleep 0.1".into(),
        ], None, Some("palette-ime"), None, Default::default()).unwrap();
        let mut app = App::new_headless(Config::default()).unwrap();
        app.tabs = Some(tabs);
        let wait_for = |app: &mut App, expected: &str| {
            let deadline = Instant::now() + Duration::from_secs(3);
            loop {
                app.process_pty_events();
                let output = app.active_terminal().unwrap().rows_text(0, 23);
                if output.contains(expected) {
                    break;
                }
                assert!(Instant::now() < deadline, "missing {expected}: {output}");
                std::thread::yield_now();
            }
        };
        wait_for(&mut app, "READY");
        for kind in ["command", "unicode", "domain"] {
            match kind {
                "command" => app.command_palette.open(),
                "unicode" => app.unicode_picker.open(),
                _ => app.open_domain_picker(),
            }
            app.handle_ime_event(Ime::Preedit("draft".into(), None));
            app.handle_ime_event(Ime::Commit("x".repeat(257)));
            app.handle_ime_event(Ime::Commit("bad\ncommit".into()));
            app.handle_ime_event(Ime::Commit("e\u{301}👩\u{200d}💻".into()));
            let info = match kind {
                "command" => app.command_palette.build_info(),
                "unicode" => app.unicode_picker.build_info(),
                _ => app.domain_picker.build_info(),
            };
            assert_eq!(info.input, "e\u{301}👩\u{200d}💻");
            assert!(app.ime.preedit().is_none());
            app.close_domain_picker();
            app.command_palette.close();
            app.unicode_picker.close();
            app.unicode_picker.finish_search_for_test();
        }
        app.session_panel.set_test_entries(
            vec![crate::session::panel::SessionEntry {
                id: "workspace-fixture".into(),
                display_name: "Roots".into(),
                subtitle: String::new(),
                last_modified: String::new(),
            }],
            true,
        );
        app.apply_session_panel_action(crate::session::panel::SessionPanelAction::Rename(
            "workspace-fixture".into(),
        ));
        assert!(
            app.session_panel
                .accessibility_set_rename(&"界".repeat(127))
        );
        app.handle_ime_event(Ime::Commit("xy".into()));
        app.handle_ime_event(Ime::Commit("bad\ncommit".into()));
        assert_eq!(
            app.session_panel.build_info().rename_text,
            Some("界".repeat(127))
        );
        app.handle_ime_event(Ime::Commit("🫚".into()));
        assert_eq!(
            app.session_panel.build_info().rename_text,
            Some(format!("{}🫚", "界".repeat(127)))
        );
        use winit::keyboard::{Key, NamedKey};
        assert!(
            app.session_panel
                .accessibility_set_rename("a界e\u{301}👩\u{200d}💻z")
        );
        for key in [
            NamedKey::Home,
            NamedKey::ArrowRight,
            NamedKey::Delete,
            NamedKey::ArrowRight,
        ] {
            assert!(app.handle_session_rename_key(&Key::Named(key)));
        }
        assert!(app.handle_session_rename_key(&Key::Character("Q".into())));
        assert_eq!(
            app.session_panel.build_info().rename_text.as_deref(),
            Some("ae\u{301}Q👩\u{200d}💻z")
        );
        app.modifiers = ModifiersState::CONTROL;
        assert!(app.handle_session_rename_key(&Key::Named(NamedKey::Delete)));
        assert_eq!(
            app.session_panel.build_info().rename_text.as_deref(),
            Some("ae\u{301}Qz")
        );
        let info = app.session_panel.build_info();
        let layout = crate::session::panel::SessionPanelLayout::calculate(
            800.0, 600.0, 0.0, 8.0, 16.0, 1.0, true, true, 1,
        );
        let text = info.rename_text.as_deref().unwrap();
        let view = layout
            .rename_input_viewport(text, info.rename_cursor)
            .unwrap();
        app.handle_ime_event(Ime::Preedit("draft".into(), None));
        let action = layout
            .rename_input_click(text, info.rename_cursor, view.text_x + 8.0, view.text_y)
            .unwrap();
        app.apply_session_panel_action(action);
        assert!(app.ime.preedit().is_none());
        assert_eq!(app.session_panel.build_info().rename_cursor, 1);
        app.handle_ime_event(Ime::Commit("R".into()));
        assert_eq!(
            app.session_panel.build_info().rename_text.as_deref(),
            Some("aRe\u{301}Qz")
        );
        app.modifiers = ModifiersState::empty();
        assert!(app.handle_session_rename_key(&Key::Named(NamedKey::Escape)));
        assert_eq!(app.session_panel.sessions()[0].display_name, "Roots");
        app.apply_session_panel_action(crate::session::panel::SessionPanelAction::Close);
        exercise_retained_rename_modal_ownership(&mut app);
        app.tabs
            .as_mut()
            .unwrap()
            .active_tab_mut()
            .unwrap()
            .set_user_title(Some("Roots".into()));
        app.begin_tab_rename();
        assert!(app.handle_tab_rename_key(&Key::Character("e\u{301}👩\u{200d}💻".into())));
        assert_eq!(
            app.tabs
                .as_ref()
                .unwrap()
                .active_tab()
                .unwrap()
                .user_title(),
            Some("Rootse\u{301}👩\u{200d}💻")
        );
        assert!(app.handle_tab_rename_key(&Key::Named(NamedKey::Backspace)));
        assert_eq!(
            app.tabs
                .as_ref()
                .unwrap()
                .active_tab()
                .unwrap()
                .user_title(),
            Some("Rootse\u{301}")
        );
        app.handle_ime_event(Ime::Commit("bad\ncommit".into()));
        assert_eq!(
            app.tabs
                .as_ref()
                .unwrap()
                .active_tab()
                .unwrap()
                .user_title(),
            Some("Rootse\u{301}")
        );
        assert!(app.handle_tab_rename_key(&Key::Named(NamedKey::Escape)));
        assert_eq!(
            app.tabs
                .as_ref()
                .unwrap()
                .active_tab()
                .unwrap()
                .user_title(),
            Some("Roots")
        );
        app.begin_tab_rename();
        app.handle_ime_event(Ime::Commit(" 🫚".into()));
        assert!(app.handle_tab_rename_key(&Key::Named(NamedKey::Enter)));
        assert!(app.tab_rename.is_none());
        assert_eq!(
            app.tabs
                .as_ref()
                .unwrap()
                .active_tab()
                .unwrap()
                .user_title(),
            Some("Roots 🫚")
        );
        // A vanished captured owner cannot preview into the focused tab.
        app.tab_rename = Some(TabRenameState {
            tab_id: usize::MAX,
            original: None,
            input: String::new(),
            max_chars: 128,
        });
        app.handle_ime_event(Ime::Commit("wrong owner".into()));
        assert_eq!(
            app.tabs
                .as_ref()
                .unwrap()
                .active_tab()
                .unwrap()
                .user_title(),
            Some("Roots 🫚")
        );
        assert!(app.handle_tab_rename_key(&Key::Named(NamedKey::Escape)));
        app.write_to_pty(b"Z");
        wait_for(&mut app, "5a");
    }

    #[test]
    fn the_agent_view_composes_only_when_nothing_above_it_takes_text() {
        let mut app = App::new_headless(Config::default()).unwrap();
        app.open_agent_view(true);
        assert!(app.agent_view_owns_ime_text());
        app.tab_rename = Some(TabRenameState {
            tab_id: 0,
            original: None,
            input: String::new(),
            max_chars: 128,
        });
        assert!(!app.agent_view_owns_ime_text());
        app.tab_rename = None;
        app.command_palette.open();
        assert!(!app.agent_view_owns_ime_text());
    }

    #[test]
    fn empty_preedit_clears_composition() {
        let mut state = ImeState::default();
        state.set_preedit("かな".into(), None);
        state.set_preedit(String::new(), None);
        assert!(state.preedit().is_none());
    }

    #[test]
    fn preedit_is_not_inserted_and_commit_is_inserted_once() {
        let mut app = App::new_headless(Config::default()).unwrap();
        app.command_palette.open();

        app.handle_ime_event(Ime::Enabled);
        app.handle_ime_event(Ime::Preedit("にほん".into(), Some((0, 9))));
        assert_eq!(app.command_palette.build_info().input, "");
        assert_eq!(app.ime.preedit().unwrap().text, "にほん");

        app.handle_ime_event(Ime::Commit("日本".into()));
        assert_eq!(app.command_palette.build_info().input, "日本");
        assert!(app.ime.preedit().is_none());
    }

    #[test]
    fn ime_commit_reaches_the_active_information_overlay_input_only() {
        let mut app = App::new_headless(Config::default()).unwrap();
        let request = app.info_overlay.show_input(
            "Edit setting",
            vec![String::new()],
            0,
            "Value: ",
            "root",
            32,
            Vec::new(),
        );
        app.handle_ime_event(Ime::Preedit("しょうが".into(), None));
        assert_eq!(app.info_overlay.input_value(request), Some("root"));
        app.handle_ime_event(Ime::Commit("生姜".into()));
        assert_eq!(app.info_overlay.input_value(request), Some("root生姜"));
        assert!(app.ime.preedit().is_none());
        app.info_overlay.cycle_focus(request, false); // Copy button, not text.
        app.handle_ime_event(Ime::Preedit("blocked".into(), None));
        app.handle_ime_event(Ime::Commit("must not arrive".into()));
        assert_eq!(app.info_overlay.input_value(request), Some("root生姜"));
        assert!(app.ime.preedit().is_none());
    }

    #[cfg(unix)]
    #[test]
    fn ime_commit_reaches_only_the_focused_pane_pty() {
        use crate::config::ShellConfig;
        use crate::panes::{PaneManager, SplitDirection};
        use std::collections::HashMap;
        use std::time::{Duration, Instant};

        if !std::path::Path::new("/bin/sh").is_file() {
            return;
        }
        let shell = ShellConfig {
            program: Some("/bin/sh".into()),
            args: vec![
                "-c".into(),
                "stty raw -echo; dd bs=1 count=6 2>/dev/null | od -An -tx1; sleep 0.1".into(),
            ],
            env: Default::default(),
        };
        let mut panes =
            PaneManager::new(40, 8, shell, 100, 1024 * 1024, 400.0, 160.0, 10.0, 20.0).unwrap();
        let right = panes.split(SplitDirection::Vertical).unwrap();
        panes.focus(right);

        let mut tabs = TabManager::new(40, 8, ShellConfig::default(), 100, 1024 * 1024);
        tabs.new_tab_with_command(
            "/bin/sh",
            &["-c".into(), "sleep 2".into()],
            None,
            Some("ime-routing"),
            None,
            HashMap::new(),
        )
        .unwrap();
        tabs.active_tab_mut().unwrap().pane_manager = Some(panes);
        let mut app = App::new_headless(Config::default()).unwrap();
        app.tabs = Some(tabs);

        app.handle_ime_event(Ime::Preedit("にほん".into(), None));
        let quiet_deadline = Instant::now() + Duration::from_millis(80);
        while Instant::now() < quiet_deadline {
            app.process_pty_events();
            std::thread::yield_now();
        }
        let pane_text = |app: &App, id| {
            app.tabs
                .as_ref()
                .unwrap()
                .active_tab()
                .unwrap()
                .pane_manager
                .as_ref()
                .unwrap()
                .get_pane(id)
                .unwrap()
                .terminal
                .rows_text(0, 7)
        };
        assert!(!pane_text(&app, right).contains("e6 97 a5"));

        app.handle_ime_event(Ime::Commit("日本".into()));
        let deadline = Instant::now() + Duration::from_secs(2);
        let output = loop {
            app.process_pty_events();
            let output = pane_text(&app, right);
            if output.contains("e6 97 a5 e6 9c ac") || Instant::now() >= deadline {
                break output;
            }
            std::thread::yield_now();
        };
        assert!(output.contains("e6 97 a5 e6 9c ac"), "{output:?}");
        assert!(!pane_text(&app, PaneId(0)).contains("e6 97 a5"));
    }
}
