//! Query editing after higher-priority surfaces have intercepted keyboard input.

use super::*;
use crate::ui::text_input::TextInputEdit;
use winit::keyboard::{Key, NamedKey};

impl App {
    pub(super) fn handle_search_edit_key(&mut self, key: &Key) -> bool {
        let edit = match key {
            Key::Named(NamedKey::ArrowLeft) => TextInputEdit::Left,
            Key::Named(NamedKey::ArrowRight) => TextInputEdit::Right,
            Key::Named(NamedKey::Home) => TextInputEdit::Home,
            Key::Named(NamedKey::End) => TextInputEdit::End,
            Key::Named(NamedKey::Backspace) => TextInputEdit::Backspace,
            Key::Named(NamedKey::Delete) => TextInputEdit::Delete,
            _ => return false,
        };
        if self.alt_graph_active {
            return true;
        }
        let edit = if self.modifiers == ModifiersState::CONTROL {
            match edit {
                TextInputEdit::Left => TextInputEdit::WordLeft,
                TextInputEdit::Right => TextInputEdit::WordRight,
                TextInputEdit::Backspace => TextInputEdit::BackspaceWord,
                TextInputEdit::Delete => TextInputEdit::DeleteWord,
                _ => return true,
            }
        } else if !self.modifiers.is_empty() {
            // Selection and other modified edits remain unsupported. Do not
            // reinterpret them as plain edits or leak them into the PTY.
            return true;
        } else {
            edit
        };
        let cursor = self.search_cursor_pos;
        if let Some(Some(cursor)) =
            self.with_search_and_active_grid(|search, grid| search.edit_query(cursor, edit, grid))
        {
            self.search_cursor_pos = cursor;
        }
        self.ime.clear_preedit();
        self.redraw_scheduler
            .request(redraw_scheduler::RedrawReason::KeyboardInput);
        if let Some(window) = &self.window {
            window.request_redraw();
        }
        true
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    #[test]
    fn search_query_keys_edit_graphemes_and_keep_all_input_out_of_the_pty() {
        let mut tabs = TabManager::new(
            80,
            24,
            crate::config::ShellConfig::default(),
            100,
            1024 * 1024,
        );
        tabs.new_tab_with_command("/bin/sh", &[
            "-c".into(), "stty raw -echo; printf READY; dd bs=1 count=1 2>/dev/null | od -An -tx1; sleep 0.1".into(),
        ], None, Some("search-edit"), None, Default::default()).unwrap();
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
        app.search.start();
        let query = "a界e\u{301}👩\u{200d}💻🇺🇸z";
        assert!(app.handle_search_key(&Key::Character(query.into())));
        assert_eq!(app.search.query(), query);
        assert_eq!(app.search_cursor_pos, query.len());
        app.handle_ime_event(winit::event::Ime::Preedit("draft".into(), None));
        for key in [
            NamedKey::Home,
            NamedKey::ArrowRight,
            NamedKey::Delete,
            NamedKey::ArrowRight,
        ] {
            assert!(app.handle_search_key(&Key::Named(key)));
        }
        assert!(app.ime.preedit().is_none());
        assert_eq!(app.search_cursor_pos, "ae\u{301}".len());
        app.handle_ime_event(winit::event::Ime::Commit("Q".into()));
        assert_eq!(app.search.query(), "ae\u{301}Q👩\u{200d}💻🇺🇸z");
        for key in [
            NamedKey::End,
            NamedKey::ArrowLeft,
            NamedKey::Backspace,
            NamedKey::ArrowLeft,
            NamedKey::Delete,
        ] {
            assert!(app.handle_search_key(&Key::Named(key)));
        }
        assert_eq!(app.search.query(), "ae\u{301}Qz");
        assert_eq!(app.search_cursor_pos, "ae\u{301}Q".len());
        for modifier in [
            ModifiersState::ALT,
            ModifiersState::SHIFT,
            ModifiersState::SUPER,
            ModifiersState::CONTROL | ModifiersState::SHIFT,
            ModifiersState::CONTROL | ModifiersState::ALT,
        ] {
            app.modifiers = modifier;
            for key in [
                NamedKey::Home,
                NamedKey::End,
                NamedKey::ArrowLeft,
                NamedKey::ArrowRight,
                NamedKey::Delete,
                NamedKey::Backspace,
            ] {
                assert!(app.handle_search_key(&Key::Named(key)));
                assert_eq!(app.search.query(), "ae\u{301}Qz");
                assert_eq!(app.search_cursor_pos, "ae\u{301}Q".len());
            }
        }
        app.modifiers = ModifiersState::CONTROL;
        app.alt_graph_active = true;
        assert!(app.handle_search_key(&Key::Named(NamedKey::Backspace)));
        assert_eq!(app.search.query(), "ae\u{301}Qz");
        app.alt_graph_active = false;
        app.with_search_and_active_grid(|search, grid| {
            search.set_query("one  e\u{301} 👩\u{200d}💻 tail", grid)
        });
        app.search_cursor_pos = app.search.query().len();
        app.modifiers = ModifiersState::CONTROL;
        app.handle_ime_event(winit::event::Ime::Preedit("draft".into(), None));
        assert!(app.handle_search_key(&Key::Named(NamedKey::ArrowLeft)));
        assert_eq!(app.search_cursor_pos, "one  e\u{301} 👩\u{200d}💻 ".len());
        assert!(app.ime.preedit().is_none());
        assert!(app.handle_search_key(&Key::Named(NamedKey::Backspace)));
        assert_eq!(app.search.query(), "one  e\u{301} tail");
        assert!(app.handle_search_key(&Key::Named(NamedKey::Delete)));
        assert_eq!(app.search.query(), "one  e\u{301} ");
        assert!(app.handle_search_key(&Key::Named(NamedKey::ArrowLeft)));
        assert_eq!(app.search_cursor_pos, 5);
        assert!(app.handle_search_key(&Key::Named(NamedKey::Backspace)));
        assert_eq!(app.search.query(), "e\u{301} ");
        assert!(app.handle_search_key(&Key::Named(NamedKey::ArrowRight)));
        assert_eq!(app.search_cursor_pos, "e\u{301}".len());
        for key in [NamedKey::Home, NamedKey::End] {
            assert!(app.handle_search_key(&Key::Named(key)));
            assert_eq!(app.search_cursor_pos, "e\u{301}".len());
        }
        app.modifiers = ModifiersState::empty();
        assert!(app.redraw_scheduler.has_pending());
        assert!(app.handle_search_key(&Key::Named(NamedKey::Escape)));
        assert!(!app.handle_search_key(&Key::Named(NamedKey::Delete)));
        app.write_to_pty(b"Z");
        wait_for(&mut app, "5a");
    }
}
