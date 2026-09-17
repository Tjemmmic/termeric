//! Bounded text and grapheme editing for the shortcut filter, not chord recording.

use super::*;
use crate::ui::text_input::TextInputEdit;
use winit::keyboard::{Key, NamedKey};

impl App {
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
        if !self.alt_graph_active
            && self
                .modifiers
                .intersects(ModifiersState::CONTROL | ModifiersState::ALT | ModifiersState::SUPER)
        {
            return true;
        }
        if self.shortcuts_panel.insert_filter_text(text) {
            self.shortcut_filter_changed();
        }
        true
    }

    fn shortcut_filter_changed(&mut self) {
        self.ime.clear_preedit();
        self.redraw_scheduler
            .request(redraw_scheduler::RedrawReason::KeyboardInput);
        if let Some(window) = &self.window {
            window.request_redraw();
        }
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

    #[cfg(unix)]
    #[test]
    fn shortcut_ime_limit_and_recording_refusal_never_leak_text_to_the_pty() {
        let mut tabs = TabManager::new(
            80,
            24,
            crate::config::ShellConfig::default(),
            100,
            1024 * 1024,
        );
        tabs.new_tab_with_command("/bin/sh", &[
            "-c".into(), "stty raw -echo; printf READY; dd bs=1 count=1 2>/dev/null | od -An -tx1; sleep 0.1".into(),
        ], None, Some("shortcut-filter"), None, Default::default()).unwrap();
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
        wait_for(&mut app, "5a");
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
