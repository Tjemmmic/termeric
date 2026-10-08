//! The find box's keys, after higher-priority surfaces have intercepted keyboard input.

use super::text_field_keys::{FieldKey, types_text};
use super::*;
use winit::event::ElementState;
use winit::keyboard::{Key, NamedKey};

impl App {
    /// Called only after higher-priority pointer or semantic owners intercept input.
    pub(super) fn focus_search_query_at(
        &mut self,
        cursor: usize,
        reason: redraw_scheduler::RedrawReason,
    ) -> bool {
        if self.reconcile_search_presentation() {
            return false;
        }
        if !self.search.resume_input_at(cursor) {
            return false;
        }
        self.search_cursor_pos = cursor;
        self.ime.clear_preedit();
        self.request_redraw(reason);

        true
    }

    /// Keyboard input while the find box is open. Returns true when the find
    /// box took the key, which then never reaches a terminal.
    pub(super) fn handle_search_mode_input(&mut self, key: &Key, state: ElementState) -> bool {
        if state != ElementState::Pressed {
            return self.search.is_active();
        }
        self.handle_search_key(key)
    }

    /// A pressed key while the find box is open: one of its own chords, else
    /// a built-in application shortcut, else nothing.
    pub(super) fn handle_search_key(&mut self, key: &Key) -> bool {
        if self.reconcile_search_presentation() {
            return true;
        }
        if self.reconcile_copy_mode_presentation() {
            return true;
        }
        if !self.search.is_active() {
            return false;
        }
        if !self.handle_search_field_key(key) {
            self.run_app_shortcut(key);
        }
        true
    }

    /// The find box's own chords. Returns false for any other key.
    fn handle_search_field_key(&mut self, key: &Key) -> bool {
        if self.handle_search_edit_key(key) {
            return true;
        }
        let plain = self.modifiers.is_empty() && !self.alt_graph_active;
        let alt = self.modifiers == ModifiersState::ALT && !self.alt_graph_active;
        let shift = self.modifiers == ModifiersState::SHIFT && !self.alt_graph_active;
        let typing = types_text(self.modifiers, self.alt_graph_active);
        match key {
            Key::Named(NamedKey::Escape) if plain => {
                self.search.cancel();
            }
            Key::Named(NamedKey::Enter) if plain => {
                // Close the query but keep the highlights.
                self.search.confirm();
                if self.copy_mode.is_active() {
                    self.jump_copy_cursor_to_search();
                }
            }
            Key::Named(NamedKey::ArrowUp) if plain => {
                self.search.prev();
                self.scroll_to_current_match();
            }
            // As in other programs' find bars, Shift+Enter steps backwards.
            Key::Named(NamedKey::Enter) if shift => {
                self.search.prev();
                self.scroll_to_current_match();
            }
            Key::Named(NamedKey::ArrowDown) if plain => {
                self.search.next();
                self.scroll_to_current_match();
            }
            Key::Character(text) if alt && text.eq_ignore_ascii_case("c") => {
                self.with_search_and_active_grid(|search, grid| {
                    search.toggle_case_sensitive(grid);
                });
            }
            Key::Character(text) if alt && text.eq_ignore_ascii_case("r") => {
                self.with_search_and_active_grid(|search, grid| {
                    search.toggle_regex(grid);
                });
            }
            Key::Character(text) if alt && text.eq_ignore_ascii_case("f") => {
                if self.copy_mode.is_active() {
                    self.toasts.warning(
                        "Filter mode is unavailable inside copy mode; matches remain highlighted",
                    );
                } else {
                    self.search.toggle_filter();
                }
            }
            Key::Named(NamedKey::Space) if typing => {
                self.insert_search_text(" ");
            }
            Key::Character(text) if typing => {
                self.insert_search_text(text);
            }
            _ => return false,
        }
        self.request_redraw(RedrawReason::Search);
        true
    }

    /// The shared editing keys. One held with modifiers the field doesn't
    /// edit with isn't the find box's, so it can still run a shortcut.
    fn handle_search_edit_key(&mut self, key: &Key) -> bool {
        let Some(FieldKey::Edit(edit)) = self.text_edit_for_key(key) else {
            return false;
        };
        let cursor = self.search_cursor_pos;
        if let Some(Some(cursor)) =
            self.with_search_and_active_grid(|search, grid| search.edit_query(cursor, edit, grid))
        {
            self.search_cursor_pos = cursor;
        }
        self.ime.clear_preedit();
        self.request_redraw(RedrawReason::KeyboardInput);

        true
    }

    /// Inserts `text` at the query's caret and moves the caret past it.
    /// Returns false when the query refused it.
    pub(super) fn insert_search_text(&mut self, text: &str) -> bool {
        let cursor = self.search_cursor_pos;
        let inserted = self
            .with_search_and_active_grid(|search, grid| search.insert_text(cursor, text, grid))
            .unwrap_or(false);
        if inserted {
            self.search_cursor_pos += text.len();
        }
        inserted
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn search_query_keys_edit_graphemes_and_keep_all_input_out_of_the_pty() {
        let mut app =
            App::new_headless_running(crate::test_support::FIRST_BYTE_PROBE, "search-edit");
        app.wait_for_screen_text("READY");
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
        app.with_search_and_active_grid(|search, grid| {
            search.set_query("READY", grid);
            search.finish(grid);
        });
        assert!(app.handle_search_key(&Key::Named(NamedKey::Enter)));
        assert!(!app.search.is_active());
        let selected = app.search.current_match_index();
        let count = app.search.match_count();
        assert!(count > 0);
        assert!(app.focus_search_query_at(2, redraw_scheduler::RedrawReason::PointerInput));
        assert_eq!(app.search.current_match_index(), selected);
        assert_eq!(app.search.match_count(), count);
        app.handle_ime_event(winit::event::Ime::Preedit("draft".into(), None));
        assert!(app.focus_search_query_at(3, redraw_scheduler::RedrawReason::PointerInput));
        assert!(app.ime.preedit().is_none());
        app.handle_ime_event(winit::event::Ime::Commit("界".into()));
        assert_eq!(app.search.query(), "REA界DY");
        assert_eq!(app.search_cursor_pos, "REA界".len());
        assert!(app.handle_search_key(&Key::Named(NamedKey::Escape)));
        assert!(!app.focus_search_query_at(0, redraw_scheduler::RedrawReason::PointerInput));
        assert!(!app.handle_search_key(&Key::Named(NamedKey::Delete)));
        app.assert_pty_untouched();
    }

    #[test]
    fn shift_enter_goes_to_the_previous_match_and_keeps_the_query_open() {
        let mut app = App::new_headless_running("printf 'x x x READY'; sleep 5", "search-back");
        app.wait_for_screen_text("READY");
        app.search.start();
        app.with_search_and_active_grid(|search, grid| {
            search.set_query("x", grid);
            search.finish(grid);
        });
        assert_eq!(app.search.match_count(), 3);
        let first = app.search.current_match_index();
        assert!(app.handle_search_key(&Key::Named(NamedKey::ArrowDown)));
        assert_ne!(app.search.current_match_index(), first);
        app.modifiers = ModifiersState::SHIFT;
        assert!(app.handle_search_key(&Key::Named(NamedKey::Enter)));
        assert_eq!(app.search.current_match_index(), first);
        assert!(app.search.is_active());
        assert_eq!(app.search.query(), "x");
        app.modifiers = ModifiersState::empty();
        assert!(app.handle_search_key(&Key::Named(NamedKey::Enter)));
        assert!(!app.search.is_active());
        assert_eq!(app.search.current_match_index(), first);
    }

    #[test]
    fn the_find_box_runs_app_shortcuts_for_keys_it_does_not_use_and_swallows_the_rest() {
        let mut app =
            App::new_headless_running(crate::test_support::FIRST_BYTE_PROBE, "search-shortcuts");
        app.wait_for_screen_text("READY");
        let tabs = app.tabs.as_mut().unwrap();
        tabs.new_tab_with_command(
            "/bin/sh",
            &["-c".into(), "sleep 5".into()],
            None,
            Some("second"),
            None,
            Default::default(),
        )
        .unwrap();
        tabs.switch_to(0);
        let active_tab = |app: &App| app.tabs.as_ref().unwrap().active_index();
        app.search.start();

        // Characters type alone, with Shift, or through AltGr (which some
        // systems report as Ctrl+Alt). A Ctrl chord types nothing.
        app.modifiers = ModifiersState::SHIFT;
        assert!(app.handle_search_key(&Key::Character("R".into())));
        app.modifiers = ModifiersState::CONTROL;
        assert!(app.handle_search_key(&Key::Character("c".into())));
        app.modifiers = ModifiersState::empty();
        assert!(app.handle_search_key(&Key::Named(NamedKey::Space)));
        app.modifiers = ModifiersState::CONTROL | ModifiersState::ALT;
        app.alt_graph_active = true;
        assert!(app.handle_search_key(&Key::Character("@".into())));
        app.alt_graph_active = false;
        assert_eq!(app.search.query(), "R @");

        // Keys that are neither the find box's nor a shortcut are swallowed.
        for (key, modifiers) in [
            (NamedKey::ArrowUp, ModifiersState::SHIFT),
            (NamedKey::Enter, ModifiersState::CONTROL),
            (NamedKey::Escape, ModifiersState::SHIFT),
            (NamedKey::F12, ModifiersState::empty()),
        ] {
            app.modifiers = modifiers;
            assert!(app.handle_search_key(&Key::Named(key)));
        }
        assert!(app.search.is_active());
        assert_eq!(app.search.query(), "R @");

        // Tab shortcuts work with the find box open.
        app.modifiers = ModifiersState::CONTROL;
        assert!(app.handle_search_key(&Key::Named(NamedKey::Tab)));
        assert_eq!(active_tab(&app), 1);
        app.modifiers = ModifiersState::CONTROL | ModifiersState::SHIFT;
        assert!(app.handle_search_key(&Key::Named(NamedKey::Tab)));
        assert_eq!(active_tab(&app), 0);
        assert!(app.search.is_active());
        assert_eq!(app.search.query(), "R @");

        // The find shortcut closes the box rather than typing "F".
        assert!(app.handle_search_key(&Key::Character("F".into())));
        assert!(!app.search.is_active());
        assert!(!app.search.query().contains('F'));

        app.modifiers = ModifiersState::empty();
        app.assert_pty_untouched();
    }
}
