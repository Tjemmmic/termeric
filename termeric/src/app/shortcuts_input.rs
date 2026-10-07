//! Bounded text and grapheme editing for the shortcut filter, not chord recording.

use super::*;
use crate::input::keybindings::KeyChord;
use crate::ui::shortcuts_panel::ShortcutsPanelLayout;
use crate::ui::text_input::TextInputEdit;
use winit::keyboard::{Key, NamedKey};

impl App {
    /// Keys for the Shortcuts panel: recording a chord, the filter, and the list.
    pub(super) fn handle_shortcuts_panel_key(&mut self, key: &Key) {
        use crate::ui::shortcuts_panel::ShortcutsKey;

        // In recording mode, capture the raw key
        if self.shortcuts_panel.is_recording() {
            let shortcuts_key = match key {
                Key::Named(NamedKey::Escape) => Some(ShortcutsKey::Escape),
                Key::Named(NamedKey::Backspace) | Key::Named(NamedKey::Delete) => {
                    Some(ShortcutsKey::Delete)
                }
                _ => match key {
                    Key::Named(
                        NamedKey::Control | NamedKey::Shift | NamedKey::Alt | NamedKey::Super,
                    ) => None,
                    _ => KeyChord::from_key_event(key, self.modifiers)
                        .map(ShortcutsKey::RecordedChord),
                },
            };
            if let Some(key) = shortcuts_key {
                let action = self
                    .shortcuts_panel
                    .handle_key(key, &self.shortcut_registry);
                self.apply_shortcuts_action(action);
                self.request_redraw(RedrawReason::KeyboardInput);
            }
            return;
        }

        if self.handle_shortcuts_filter_key(key) {
            return;
        }
        let shortcuts_key = match key {
            Key::Named(NamedKey::Escape) => Some(ShortcutsKey::Escape),
            Key::Named(NamedKey::ArrowUp) => Some(ShortcutsKey::Up),
            Key::Named(NamedKey::ArrowDown) => Some(ShortcutsKey::Down),
            Key::Named(NamedKey::Enter) => Some(ShortcutsKey::Enter),
            Key::Named(NamedKey::Backspace) => Some(ShortcutsKey::Backspace),
            Key::Named(NamedKey::Delete) => Some(ShortcutsKey::Delete),
            _ => None,
        };

        if let Some(key) = shortcuts_key {
            let action = self
                .shortcuts_panel
                .handle_key(key, &self.shortcut_registry);
            self.apply_shortcuts_action(action);
            self.request_redraw(RedrawReason::KeyboardInput);
        }
    }

    /// Called only for primary presses after higher-priority modal routing.
    /// Even a collapsed or recording input consumes its own rectangle.
    pub(super) fn handle_shortcuts_filter_click(
        &mut self,
        x: f32,
        y: f32,
        layout: &ShortcutsPanelLayout,
        cell_width: f32,
        cell_height: f32,
    ) -> bool {
        if !self.shortcuts_panel.is_active() || !layout.input.contains(x, y) {
            return false;
        }
        if self
            .shortcuts_panel
            .place_filter_caret_at(x, y, layout, cell_width, cell_height)
        {
            self.shortcut_filter_changed();
        }
        true
    }

    pub(super) fn handle_shortcuts_filter_key(&mut self, key: &Key) -> bool {
        if !self.shortcuts_panel.is_active() || self.shortcuts_panel.is_recording() {
            return false;
        }
        let edit = match key {
            Key::Named(NamedKey::ArrowLeft) => Some(TextInputEdit::Left),
            Key::Named(NamedKey::ArrowRight) => Some(TextInputEdit::Right),
            Key::Named(NamedKey::Home) => Some(TextInputEdit::Home),
            Key::Named(NamedKey::End) => Some(TextInputEdit::End),
            Key::Named(NamedKey::Backspace) => Some(TextInputEdit::Backspace),
            // Plain Delete belongs to clearing the selected binding. Modified
            // Delete must not accidentally become that destructive action.
            Key::Named(NamedKey::Delete) if !self.modifiers.is_empty() || self.alt_graph_active => {
                return true;
            }
            _ => None,
        };
        if let Some(edit) = edit {
            if self.alt_graph_active {
                return true;
            }
            let edit = if self.modifiers == ModifiersState::CONTROL {
                match edit {
                    TextInputEdit::Left => TextInputEdit::WordLeft,
                    TextInputEdit::Right => TextInputEdit::WordRight,
                    TextInputEdit::Backspace => TextInputEdit::BackspaceWord,
                    _ => return true,
                }
            } else if !self.modifiers.is_empty() {
                // Selection is not implemented; consume unsupported modifiers.
                return true;
            } else {
                edit
            };
            if self.shortcuts_panel.edit_filter(edit) {
                self.shortcut_filter_changed();
            }
            return true;
        }
        let text = match key {
            Key::Character(text) => text.as_str(),
            Key::Named(NamedKey::Space) => " ",
            _ => return false,
        };
        if !super::text_field_keys::types_text(self.modifiers, self.alt_graph_active) {
            return true;
        }
        if self.shortcuts_panel.insert_filter_text(text) {
            self.shortcut_filter_changed();
        }
        true
    }

    fn shortcut_filter_changed(&mut self) {
        self.ime.clear_preedit();
        self.request_redraw(RedrawReason::KeyboardInput);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filter_keeps_complete_text_events_and_consumes_unsupported_modifiers() {
        let mut app = App::new_headless(Config::default()).unwrap();
        let key = Key::Character("e\u{301}👩\u{200d}💻".into());
        assert!(!app.handle_shortcuts_filter_key(&key));
        app.shortcuts_panel.open();
        app.handle_ime_event(winit::event::Ime::Preedit("draft".into(), None));
        assert!(app.handle_shortcuts_filter_key(&key));
        assert_eq!(
            app.shortcuts_panel
                .build_info(&app.shortcut_registry)
                .filter,
            "e\u{301}👩\u{200d}💻"
        );
        assert!(app.ime.preedit().is_none());
        for modifier in [
            ModifiersState::CONTROL,
            ModifiersState::ALT,
            ModifiersState::SUPER,
        ] {
            app.modifiers = modifier;
            assert!(app.handle_shortcuts_filter_key(&Key::Character("x".into())));
            assert_eq!(
                app.shortcuts_panel
                    .build_info(&app.shortcut_registry)
                    .filter,
                "e\u{301}👩\u{200d}💻"
            );
        }
        app.alt_graph_active = true;
        app.modifiers = ModifiersState::CONTROL | ModifiersState::ALT;
        assert!(app.handle_shortcuts_filter_key(&Key::Character("@".into())));
        app.alt_graph_active = false;
        app.modifiers = ModifiersState::SHIFT;
        assert!(app.handle_shortcuts_filter_key(&Key::Named(NamedKey::Space)));
        assert!(
            app.shortcuts_panel
                .build_info(&app.shortcut_registry)
                .filter
                .ends_with("@ ")
        );
        assert!(app.redraw_scheduler.has_pending());
    }

    #[test]
    fn shortcut_ime_limit_and_recording_refusal_never_leak_text_to_the_pty() {
        let mut app = App::new_headless_running(
            "stty raw -echo; printf READY; dd bs=1 count=1 2>/dev/null | od -An -tx1; sleep 0.1",
            "shortcut-filter",
        );
        app.wait_for_screen_text("READY");
        app.shortcuts_panel.open();
        assert!(app.handle_shortcuts_filter_key(&Key::Character("e\u{301}".into())));
        app.handle_ime_event(winit::event::Ime::Commit("👩\u{200d}💻".into()));
        assert_eq!(
            app.shortcuts_panel
                .build_info(&app.shortcut_registry)
                .filter,
            "e\u{301}👩\u{200d}💻"
        );
        assert!(app.handle_shortcuts_filter_key(&Key::Named(NamedKey::ArrowLeft)));
        app.handle_ime_event(winit::event::Ime::Commit("界".into()));
        assert_eq!(
            app.shortcuts_panel
                .build_info(&app.shortcut_registry)
                .filter,
            "e\u{301}界👩\u{200d}💻"
        );
        assert!(app.handle_shortcuts_filter_key(&Key::Named(NamedKey::Backspace)));
        let layout = ShortcutsPanelLayout::calculate(1000.0, 700.0, 8.0, 16.0, 1.0, 0);
        assert!(app.handle_shortcuts_filter_click(
            layout.input.x,
            layout.input.y,
            &layout,
            8.0,
            16.0
        ));
        app.handle_ime_event(winit::event::Ime::Commit("@".into()));
        assert_eq!(
            app.shortcuts_panel
                .build_info(&app.shortcut_registry)
                .filter,
            "@e\u{301}👩\u{200d}💻"
        );
        app.modifiers = ModifiersState::CONTROL;
        assert!(app.handle_shortcuts_filter_key(&Key::Named(NamedKey::ArrowLeft)));
        app.modifiers = ModifiersState::empty();
        let before = "x".repeat(127);
        assert!(app.shortcuts_panel.accessibility_set_filter(&before));
        for rejected in ["e\u{301}", "\nexecute", "\u{2028}", &"x".repeat(513)] {
            app.handle_ime_event(winit::event::Ime::Commit(rejected.into()));
            assert_eq!(
                app.shortcuts_panel
                    .build_info(&app.shortcut_registry)
                    .filter,
                before
            );
            assert!(app.handle_shortcuts_filter_key(&Key::Character(rejected.into())));
            assert_eq!(
                app.shortcuts_panel
                    .build_info(&app.shortcut_registry)
                    .filter,
                before
            );
        }
        app.handle_ime_event(winit::event::Ime::Commit("界".into()));
        assert_eq!(
            app.shortcuts_panel
                .build_info(&app.shortcut_registry)
                .filter,
            format!("{before}界")
        );
        app.shortcuts_panel
            .start_recording(crate::input::keybindings::BindableAction::Copy);
        assert!(!app.handle_shortcuts_filter_key(&Key::Character("recording".into())));
        app.handle_ime_event(winit::event::Ime::Commit("recording".into()));
        assert!(app.shortcuts_panel.is_recording());
        assert_eq!(
            app.shortcuts_panel
                .build_info(&app.shortcut_registry)
                .filter,
            format!("{before}界")
        );
        app.shortcuts_panel.close();
        app.write_to_pty(b"Z");
        app.wait_for_screen_text("5a");
    }

    #[test]
    fn shortcut_filter_pointer_click_clears_preedit_without_changing_recording() {
        let mut app = App::new_headless(Config::default()).unwrap();
        let layout = ShortcutsPanelLayout::calculate(1000.0, 700.0, 8.0, 16.0, 1.0, 0);
        let (x, y) = (layout.input.x, layout.input.y);
        assert!(!app.handle_shortcuts_filter_click(x, y, &layout, 8.0, 16.0));
        app.shortcuts_panel.open();
        assert!(app.shortcuts_panel.insert_filter_text("e\u{301}界"));
        app.handle_ime_event(winit::event::Ime::Preedit("draft".into(), None));
        assert!(app.handle_shortcuts_filter_click(x, y, &layout, 8.0, 16.0));
        assert!(app.ime.preedit().is_none());
        assert!(app.redraw_scheduler.has_pending());
        assert_eq!(
            app.shortcuts_panel
                .build_info(&app.shortcut_registry)
                .cursor,
            0
        );
        assert!(app.handle_shortcuts_filter_key(&Key::Character("@".into())));
        assert_eq!(
            app.shortcuts_panel
                .build_info(&app.shortcut_registry)
                .filter,
            "@e\u{301}界"
        );
        app.shortcuts_panel
            .start_recording(crate::input::keybindings::BindableAction::Copy);
        assert!(app.handle_shortcuts_filter_click(x, y, &layout, 8.0, 16.0));
        assert!(app.shortcuts_panel.is_recording());
        assert_eq!(
            app.shortcuts_panel
                .build_info(&app.shortcut_registry)
                .cursor,
            1
        );
        assert!(!app.handle_shortcuts_filter_click(x, y - 1.0, &layout, 8.0, 16.0));
        let collapsed = ShortcutsPanelLayout::calculate(60.0, 200.0, 8.0, 16.0, 1.0, 0);
        app.shortcuts_panel.open();
        assert!(app.handle_shortcuts_filter_click(
            collapsed.input.x,
            collapsed.input.y,
            &collapsed,
            8.0,
            16.0
        ));
        assert_eq!(
            app.shortcuts_panel
                .build_info(&app.shortcut_registry)
                .cursor,
            0
        );
    }

    #[test]
    fn shortcut_filter_key_edits_clear_preedit_and_consume_unsupported_modifiers() {
        let mut app = App::new_headless(Config::default()).unwrap();
        let key = |named| Key::Named(named);
        assert!(!app.handle_shortcuts_filter_key(&key(NamedKey::ArrowLeft)));
        app.shortcuts_panel.open();
        assert!(app.shortcuts_panel.insert_filter_text("copy paste"));
        app.handle_ime_event(winit::event::Ime::Preedit("draft".into(), None));
        app.modifiers = ModifiersState::CONTROL;
        assert!(app.handle_shortcuts_filter_key(&key(NamedKey::ArrowLeft)));
        assert_eq!(
            app.shortcuts_panel
                .build_info(&app.shortcut_registry)
                .cursor,
            5
        );
        assert!(app.ime.preedit().is_none());
        assert!(app.redraw_scheduler.has_pending());
        assert!(app.handle_shortcuts_filter_key(&key(NamedKey::ArrowRight)));
        assert_eq!(
            app.shortcuts_panel
                .build_info(&app.shortcut_registry)
                .cursor,
            10
        );
        assert!(app.handle_shortcuts_filter_key(&key(NamedKey::Backspace)));
        assert_eq!(
            app.shortcuts_panel
                .build_info(&app.shortcut_registry)
                .filter,
            "copy "
        );
        for modifier in [
            ModifiersState::SHIFT,
            ModifiersState::ALT,
            ModifiersState::SUPER,
            ModifiersState::CONTROL | ModifiersState::SHIFT,
        ] {
            app.modifiers = modifier;
            for named in [
                NamedKey::ArrowLeft,
                NamedKey::ArrowRight,
                NamedKey::Home,
                NamedKey::End,
                NamedKey::Backspace,
                NamedKey::Delete,
            ] {
                assert!(app.handle_shortcuts_filter_key(&key(named)));
                let info = app.shortcuts_panel.build_info(&app.shortcut_registry);
                assert_eq!(info.filter, "copy ");
                assert_eq!(info.cursor, 5);
            }
        }
        app.modifiers = ModifiersState::CONTROL;
        assert!(app.handle_shortcuts_filter_key(&key(NamedKey::Delete)));
        app.alt_graph_active = true;
        assert!(app.handle_shortcuts_filter_key(&key(NamedKey::Backspace)));
        assert_eq!(
            app.shortcuts_panel
                .build_info(&app.shortcut_registry)
                .filter,
            "copy "
        );
        app.alt_graph_active = false;
        app.modifiers = ModifiersState::empty();
        assert!(app.handle_shortcuts_filter_key(&key(NamedKey::Home)));
        assert_eq!(
            app.shortcuts_panel
                .build_info(&app.shortcut_registry)
                .cursor,
            0
        );
        assert!(app.handle_shortcuts_filter_key(&key(NamedKey::End)));
        assert_eq!(
            app.shortcuts_panel
                .build_info(&app.shortcut_registry)
                .cursor,
            5
        );
        assert!(!app.handle_shortcuts_filter_key(&key(NamedKey::Delete)));
        assert!(!app.handle_shortcuts_filter_key(&key(NamedKey::Enter)));
        app.shortcuts_panel
            .start_recording(crate::input::keybindings::BindableAction::Copy);
        assert!(!app.handle_shortcuts_filter_key(&key(NamedKey::Backspace)));
        assert!(!app.handle_shortcuts_filter_key(&key(NamedKey::ArrowLeft)));
        assert_eq!(
            app.shortcuts_panel
                .build_info(&app.shortcut_registry)
                .filter,
            "copy "
        );
    }
}
