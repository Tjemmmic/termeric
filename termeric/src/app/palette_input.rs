//! Shared query editing dispatch, after higher-priority modal interception.

use super::text_field_keys::FieldKey;
use super::*;
use winit::keyboard::Key;

impl App {
    /// True means the key belongs to a palette, even when its busy stage is
    /// read-only. Never let editing keys fall through to a terminal behind it.
    pub(super) fn handle_palette_edit_key(&mut self, key: &Key) -> bool {
        if !self.domain_picker.is_active()
            && !self.unicode_picker.is_active()
            && !self.command_palette.is_active()
        {
            return false;
        }
        let edit = match self.text_edit_for_key(key) {
            Some(FieldKey::Edit(edit)) => edit,
            Some(FieldKey::Consume) => return true,
            None => return false,
        };
        if self.domain_picker.is_active() {
            self.domain_picker.edit_input(edit);
        } else if self.unicode_picker.is_active() {
            self.unicode_picker.edit_input(edit);
        } else if self.command_palette.is_active() {
            self.command_palette.edit_input(edit);
        } else {
            return false;
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

#[cfg(test)]
mod tests {
    use super::*;
    use winit::keyboard::NamedKey;

    #[test]
    fn palette_query_keys_edit_only_the_active_model_and_insert_at_its_caret() {
        for kind in ["command", "unicode", "domain"] {
            let mut app = App::new_headless(Config::default()).unwrap();
            let query = "a界e\u{301}👩\u{200d}💻z";
            match kind {
                "command" => {
                    app.command_palette.open();
                    app.command_palette.accessibility_set_input(query);
                }
                "unicode" => {
                    app.unicode_picker.open();
                    app.unicode_picker.accessibility_set_filter(query);
                }
                _ => {
                    app.domain_picker.open(&app.config.domains);
                    app.domain_picker.accessibility_set_filter(query);
                }
            }
            for key in [
                NamedKey::Home,
                NamedKey::ArrowRight,
                NamedKey::Delete,
                NamedKey::ArrowRight,
            ] {
                assert!(app.handle_palette_edit_key(&Key::Named(key)));
            }
            match kind {
                "command" => {
                    app.command_palette.handle_char('X');
                }
                "unicode" => app.unicode_picker.handle_char('X'),
                _ => app.domain_picker.handle_char('X'),
            }
            let info = match kind {
                "command" => app.command_palette.build_info(),
                "unicode" => app.unicode_picker.build_info(),
                _ => app.domain_picker.build_info(),
            };
            assert_eq!(info.input, "ae\u{301}X👩\u{200d}💻z");
            assert_eq!(info.cursor, "ae\u{301}X".len());
            app.handle_ime_event(winit::event::Ime::Commit("Q".into()));
            let info = match kind {
                "command" => app.command_palette.build_info(),
                "unicode" => app.unicode_picker.build_info(),
                _ => app.domain_picker.build_info(),
            };
            assert_eq!(info.input, "ae\u{301}XQ👩\u{200d}💻z");
            assert_eq!(info.cursor, "ae\u{301}XQ".len());
            assert!(app.handle_palette_edit_key(&Key::Named(NamedKey::End)));
            assert!(app.handle_palette_edit_key(&Key::Named(NamedKey::ArrowLeft)));
            assert!(app.handle_palette_edit_key(&Key::Named(NamedKey::Delete)));
            let info = match kind {
                "command" => app.command_palette.build_info(),
                "unicode" => app.unicode_picker.build_info(),
                _ => app.domain_picker.build_info(),
            };
            assert!(info.input.ends_with("👩\u{200d}💻"));
            assert!(!app.handle_palette_edit_key(&Key::Named(NamedKey::Enter)));
            assert!(app.redraw_scheduler.has_pending());
            app.command_palette.close();
            app.domain_picker.close();
            app.unicode_picker.close();
            assert!(!app.handle_palette_edit_key(&Key::Named(NamedKey::Home)));
            app.unicode_picker.finish_search_for_test();
        }
    }

    #[test]
    fn busy_domain_picker_consumes_edits_without_mutating_it_or_background_palette() {
        let mut app = App::new_headless(Config::default()).unwrap();
        app.command_palette.open();
        app.command_palette.accessibility_set_input("background");
        app.domain_picker.open(&app.config.domains);
        app.domain_picker.accessibility_set_filter("local");
        // UI-only transition; do not dispatch its discovery/network command.
        assert!(app.domain_picker.activate().is_some());
        assert!(app.domain_picker.is_busy());
        let before = app.domain_picker.build_info();
        for key in [
            NamedKey::Home,
            NamedKey::Delete,
            NamedKey::End,
            NamedKey::ArrowLeft,
            NamedKey::ArrowRight,
            NamedKey::Backspace,
        ] {
            assert!(app.handle_palette_edit_key(&Key::Named(key)));
            app.modifiers = ModifiersState::CONTROL;
            assert!(app.handle_palette_edit_key(&Key::Named(key)));
            app.modifiers = ModifiersState::empty();
        }
        assert_eq!(app.domain_picker.build_info().input, before.input);
        assert_eq!(app.domain_picker.build_info().cursor, before.cursor);
        assert_eq!(app.command_palette.build_info().input, "background");
        assert_eq!(app.command_palette.build_info().cursor, "background".len());
    }

    #[test]
    fn unsupported_modified_edits_are_consumed_without_mutation() {
        let mut app = App::new_headless(Config::default()).unwrap();
        app.command_palette.open();
        app.command_palette.accessibility_set_input("two words");
        for modifier in [
            ModifiersState::ALT,
            ModifiersState::SHIFT,
            ModifiersState::SUPER,
            ModifiersState::CONTROL | ModifiersState::SHIFT,
            ModifiersState::CONTROL | ModifiersState::ALT,
        ] {
            app.modifiers = modifier;
            for key in [
                NamedKey::ArrowLeft,
                NamedKey::ArrowRight,
                NamedKey::Home,
                NamedKey::End,
                NamedKey::Backspace,
                NamedKey::Delete,
            ] {
                assert!(app.handle_palette_edit_key(&Key::Named(key)));
                assert_eq!(app.command_palette.build_info().input, "two words");
                assert_eq!(app.command_palette.build_info().cursor, "two words".len());
            }
        }
        app.modifiers = ModifiersState::CONTROL;
        app.alt_graph_active = true;
        assert!(app.handle_palette_edit_key(&Key::Named(NamedKey::Backspace)));
        assert_eq!(app.command_palette.build_info().input, "two words");
        app.command_palette.close();
        assert!(!app.handle_palette_edit_key(&Key::Named(NamedKey::Backspace)));
    }

    #[test]
    fn palette_word_edits_preserve_graphemes_and_clear_preedit() {
        for kind in ["command", "unicode", "domain"] {
            let mut app = App::new_headless(Config::default()).unwrap();
            let query = "one  e\u{301} 👩\u{200d}💻 tail";
            match kind {
                "command" => {
                    app.command_palette.open();
                    app.command_palette.accessibility_set_input(query);
                }
                "unicode" => {
                    app.unicode_picker.open();
                    app.unicode_picker.accessibility_set_filter(query);
                }
                _ => {
                    app.domain_picker.open(&app.config.domains);
                    app.domain_picker.accessibility_set_filter(query);
                }
            }
            app.modifiers = ModifiersState::CONTROL;
            app.handle_ime_event(winit::event::Ime::Preedit("draft".into(), None));
            for (key, expected, cursor) in [
                (
                    NamedKey::ArrowLeft,
                    query,
                    "one  e\u{301} 👩\u{200d}💻 ".len(),
                ),
                (
                    NamedKey::Backspace,
                    "one  e\u{301} tail",
                    "one  e\u{301} ".len(),
                ),
                (NamedKey::Delete, "one  e\u{301} ", "one  e\u{301} ".len()),
                (NamedKey::ArrowLeft, "one  e\u{301} ", 5),
                (NamedKey::Backspace, "e\u{301} ", 0),
                (NamedKey::ArrowRight, "e\u{301} ", "e\u{301}".len()),
                (NamedKey::Home, "e\u{301} ", "e\u{301}".len()),
                (NamedKey::End, "e\u{301} ", "e\u{301}".len()),
            ] {
                assert!(app.handle_palette_edit_key(&Key::Named(key)));
                let info = match kind {
                    "command" => app.command_palette.build_info(),
                    "unicode" => app.unicode_picker.build_info(),
                    _ => app.domain_picker.build_info(),
                };
                assert_eq!(info.input, expected, "{kind}: {key:?}");
                assert_eq!(info.cursor, cursor, "{kind}: {key:?}");
                assert!(app.ime.preedit().is_none());
            }
            app.unicode_picker.finish_search_for_test();
        }
    }
}
