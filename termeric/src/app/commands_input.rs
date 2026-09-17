//! Inline command-form keyboard ownership after higher-priority surfaces.

use super::*;
use crate::ui::commands::panel::CommandsPanelAction;
use crate::ui::text_input::TextInputEdit;
use winit::keyboard::{Key, NamedKey};

impl App {
    pub(super) fn handle_commands_form_key(&mut self, key: &Key) -> bool {
        if !self.commands_panel.is_open() || !self.commands_panel.is_editing() {
            return false;
        }
        let edit = match key {
            Key::Named(NamedKey::ArrowLeft) => Some(TextInputEdit::Left),
            Key::Named(NamedKey::ArrowRight) => Some(TextInputEdit::Right),
            Key::Named(NamedKey::Home) => Some(TextInputEdit::Home),
            Key::Named(NamedKey::End) => Some(TextInputEdit::End),
            Key::Named(NamedKey::Backspace) => Some(TextInputEdit::Backspace),
            Key::Named(NamedKey::Delete) => Some(TextInputEdit::Delete),
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
                    TextInputEdit::Delete => TextInputEdit::DeleteWord,
                    _ => return true,
                }
            } else if !self.modifiers.is_empty() {
                // Selection is not implemented; never turn modified keys
                // into plain deletion or forward them to the terminal.
                return true;
            } else {
                edit
            };
            self.commands_panel.form_edit(edit);
        } else {
            let plain = self.modifiers.is_empty() && !self.alt_graph_active;
            let text = self.alt_graph_active
                || !self.modifiers.intersects(
                    ModifiersState::CONTROL | ModifiersState::ALT | ModifiersState::SUPER,
                );
            match key {
                Key::Named(NamedKey::Escape) if plain => self.commands_panel.cancel_edit(),
                Key::Named(NamedKey::Tab)
                    if !self.alt_graph_active
                        && (self.modifiers.is_empty()
                            || self.modifiers == ModifiersState::SHIFT) =>
                {
                    self.commands_panel.next_field();
                }
                Key::Named(NamedKey::Enter) if plain => {
                    self.apply_commands_panel_action(CommandsPanelAction::Submit);
                }
                Key::Named(NamedKey::Space) if text => self.commands_panel.form_char(' '),
                Key::Character(value) if text => {
                    for character in value.chars().filter(|c| !c.is_control()) {
                        self.commands_panel.form_char(character);
                    }
                }
                _ => return true,
            }
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
    fn pointer_and_semantic_field_changes_retire_old_preedit() {
        let mut app = App::new_headless(Config::default()).unwrap();
        app.commands_panel.toggle();
        app.commands_panel.start_add();
        for action in [
            CommandsPanelAction::ClickCommand,
            CommandsPanelAction::ClickName,
            CommandsPanelAction::StartAdd,
            CommandsPanelAction::CancelEdit,
        ] {
            app.handle_ime_event(winit::event::Ime::Preedit("draft".into(), None));
            assert!(app.ime.preedit().is_some());
            app.apply_commands_panel_action(action);
            assert!(app.ime.preedit().is_none());
        }
        assert!(app.commands_panel.is_open());
        assert!(!app.commands_panel.is_editing());
    }

    #[test]
    fn form_modifiers_are_not_silently_typed_or_reinterpreted_as_deletion() {
        let mut app = App::new_headless(Config::default()).unwrap();
        assert!(!app.handle_commands_form_key(&Key::Named(NamedKey::Home)));
        app.commands_panel.toggle();
        app.commands_panel.start_add();
        app.commands_panel.accessibility_set_name("two words");
        for modifiers in [
            ModifiersState::ALT,
            ModifiersState::SUPER,
            ModifiersState::SHIFT,
            ModifiersState::CONTROL | ModifiersState::SHIFT,
        ] {
            app.modifiers = modifiers;
            for key in [
                NamedKey::Home,
                NamedKey::End,
                NamedKey::ArrowLeft,
                NamedKey::ArrowRight,
                NamedKey::Backspace,
                NamedKey::Delete,
            ] {
                assert!(app.handle_commands_form_key(&Key::Named(key)));
                assert_eq!(app.commands_panel.form_name.text(), "two words");
                assert_eq!(app.commands_panel.form_name.cursor(), 9);
            }
        }
        for modifiers in [
            ModifiersState::CONTROL,
            ModifiersState::ALT,
            ModifiersState::SUPER,
        ] {
            app.modifiers = modifiers;
            assert!(app.handle_commands_form_key(&Key::Character("x".into())));
            assert_eq!(app.commands_panel.form_name.text(), "two words");
        }
        app.modifiers = ModifiersState::CONTROL | ModifiersState::ALT;
        app.alt_graph_active = true;
        assert!(app.handle_commands_form_key(&Key::Named(NamedKey::Backspace)));
        assert!(app.handle_commands_form_key(&Key::Character("@".into())));
        assert_eq!(app.commands_panel.form_name.text(), "two words@");
        app.alt_graph_active = false;
        app.modifiers = ModifiersState::SHIFT;
        assert!(app.handle_commands_form_key(&Key::Named(NamedKey::Tab)));
        assert_eq!(
            app.commands_panel.build_info(&[]).edit_mode,
            crate::ui::commands::panel::EditMode::EditingCommand
        );
    }

    #[cfg(unix)]
    #[test]
    fn editing_and_ime_commit_stay_in_the_form_and_leave_the_raw_pty_untouched() {
        let mut tabs = TabManager::new(
            80,
            24,
            crate::config::ShellConfig::default(),
            100,
            1024 * 1024,
        );
        tabs.new_tab_with_command("/bin/sh", &[
            "-c".into(), "stty raw -echo; printf READY; dd bs=1 count=1 2>/dev/null | od -An -tx1; sleep 0.1".into(),
        ], None, Some("command-editor"), None, Default::default()).unwrap();
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
        app.commands_panel.toggle();
        app.commands_panel.start_add();
        assert!(app.handle_commands_form_key(&Key::Character("a界e\u{301}👩\u{200d}💻z".into())));
        for key in [
            NamedKey::Home,
            NamedKey::ArrowRight,
            NamedKey::Delete,
            NamedKey::ArrowRight,
        ] {
            app.handle_ime_event(winit::event::Ime::Preedit("draft".into(), None));
            assert!(app.handle_commands_form_key(&Key::Named(key)));
            assert!(app.ime.preedit().is_none());
        }
        app.handle_ime_event(winit::event::Ime::Commit("Q".into()));
        assert_eq!(
            app.commands_panel.form_name.text(),
            "ae\u{301}Q👩\u{200d}💻z"
        );
        assert_eq!(app.commands_panel.form_name.cursor(), "ae\u{301}Q".len());
        assert!(app.handle_commands_form_key(&Key::Named(NamedKey::Tab)));
        assert!(
            app.handle_commands_form_key(&Key::Character("one e\u{301} 👩\u{200d}💻 tail".into()))
        );
        app.modifiers = ModifiersState::CONTROL;
        for key in [NamedKey::ArrowLeft, NamedKey::Backspace, NamedKey::Delete] {
            assert!(app.handle_commands_form_key(&Key::Named(key)));
        }
        assert_eq!(app.commands_panel.form_command.text(), "one e\u{301} ");
        app.modifiers = ModifiersState::empty();
        assert!(app.handle_commands_form_key(&Key::Named(NamedKey::Escape)));
        assert!(!app.commands_panel.is_editing());
        assert!(app.commands_panel.is_open());
        app.commands_panel.close();
        app.write_to_pty(b"Z");
        wait_for(&mut app, "5a"); // first raw byte: no editing/IME bytes leaked
    }
}
