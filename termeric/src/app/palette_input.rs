//! Shared query editing dispatch, after higher-priority modal interception.

use super::*;
use crate::ui::text_input::TextInputEdit;
use winit::keyboard::{Key, NamedKey};

impl App {
    /// True means the key belongs to a palette, even when its busy stage is
    /// read-only. Never let editing keys fall through to a terminal behind it.
    pub(super) fn handle_palette_edit_key(&mut self, key: &Key) -> bool {
        if self.modifiers.control_key()
            || self.modifiers.alt_key()
            || self.modifiers.super_key()
            || self.modifiers.shift_key()
        {
            return false;
        }
        let edit = match key {
            Key::Named(NamedKey::ArrowLeft) => TextInputEdit::Left,
            Key::Named(NamedKey::ArrowRight) => TextInputEdit::Right,
            Key::Named(NamedKey::Home) => TextInputEdit::Home,
            Key::Named(NamedKey::End) => TextInputEdit::End,
            Key::Named(NamedKey::Delete) => TextInputEdit::Delete,
            _ => return false,
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
            if kind != "domain" {
                app.handle_ime_event(winit::event::Ime::Commit("Q".into()));
                let info = if kind == "command" {
                    app.command_palette.build_info()
                } else {
                    app.unicode_picker.build_info()
                };
                assert_eq!(info.input, "ae\u{301}XQ👩\u{200d}💻z");
                assert_eq!(info.cursor, "ae\u{301}XQ".len());
            }
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
        ] {
            assert!(app.handle_palette_edit_key(&Key::Named(key)));
        }
        assert_eq!(app.domain_picker.build_info().input, before.input);
        assert_eq!(app.domain_picker.build_info().cursor, before.cursor);
        assert_eq!(app.command_palette.build_info().input, "background");
        assert_eq!(app.command_palette.build_info().cursor, "background".len());
    }

    #[test]
    fn modified_navigation_does_not_pretend_to_support_selection_or_word_motion() {
        let mut app = App::new_headless(Config::default()).unwrap();
        app.command_palette.open();
        app.command_palette.accessibility_set_input("two words");
        for modifier in [
            ModifiersState::CONTROL,
            ModifiersState::ALT,
            ModifiersState::SHIFT,
            ModifiersState::SUPER,
        ] {
            app.modifiers = modifier;
            assert!(!app.handle_palette_edit_key(&Key::Named(NamedKey::ArrowLeft)));
            assert_eq!(app.command_palette.build_info().cursor, "two words".len());
        }
    }
}
