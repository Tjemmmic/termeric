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
                Key::Named(NamedKey::Space) if text => {
                    if !self.commands_panel.insert_form_text(" ") {
                        return true;
                    }
                }
                Key::Character(value) if text => {
                    if !self.commands_panel.insert_form_text(value) {
                        return true;
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

    fn exercise_atomic_form_input(app: &mut App) {
        use crate::ui::commands::panel::{CommandsPanel, EditMode};
        use winit::event::Ime;

        for name in [true, false] {
            app.commands_panel.start_add();
            let limit = if name {
                256
            } else {
                CommandsPanel::MAX_FORM_COMMAND_BYTES
            };
            let original = "x".repeat(limit - 3);
            assert!(if name {
                app.commands_panel.accessibility_set_name(&original)
            } else {
                app.commands_panel.accessibility_set_command(&original)
            });
            let field = if name {
                EditMode::EditingName
            } else {
                EditMode::EditingCommand
            };
            assert!(app.commands_panel.place_form_caret(field, 1));
            let assert_unchanged = |app: &App| {
                let (active, other) = if name {
                    (
                        &app.commands_panel.form_name,
                        &app.commands_panel.form_command,
                    )
                } else {
                    (
                        &app.commands_panel.form_command,
                        &app.commands_panel.form_name,
                    )
                };
                assert_eq!(active.text(), original);
                assert_eq!(active.cursor(), 1);
                assert!(other.text().is_empty());
            };
            for rejected in [
                "",
                "xy界",
                "a\nb",
                "a\rb",
                "\t",
                "\x1b",
                "\0",
                "\u{85}",
                "\u{2028}",
                "\u{2029}",
                &"x".repeat(limit + 1),
            ] {
                app.handle_ime_event(Ime::Preedit("draft".into(), None));
                assert!(app.handle_commands_form_key(&Key::Character(rejected.into())));
                assert_unchanged(app);
                assert!(app.ime.preedit().is_some());
                app.handle_ime_event(Ime::Commit(rejected.into()));
                assert_unchanged(app);
                // A native Commit ends the composition even when its text
                // is rejected; keyboard rejection does not end composition.
                assert!(app.ime.preedit().is_none());
            }
            app.handle_ime_event(Ime::Commit("界".into()));
            let expected = format!("x界{}", &original[1..]);
            let active = if name {
                &app.commands_panel.form_name
            } else {
                &app.commands_panel.form_command
            };
            assert_eq!(active.text(), expected);
            assert_eq!(active.cursor(), 4);
            app.handle_ime_event(Ime::Preedit("keep".into(), None));
            assert!(app.handle_commands_form_key(&Key::Named(NamedKey::Space)));
            assert!(app.ime.preedit().is_some());
            assert!(app.handle_commands_form_key(&Key::Named(NamedKey::Backspace)));
            assert!(app.handle_commands_form_key(&Key::Character("e\u{301}".into())));
            let active = if name {
                &app.commands_panel.form_name
            } else {
                &app.commands_panel.form_command
            };
            assert_eq!(active.text(), format!("xe\u{301}{}", &original[1..]));
            assert_eq!(active.cursor(), 4);
            assert!(app.ime.preedit().is_none());
            assert!(app.config.commands.is_empty());
        }
        app.commands_panel.start_add();
    }

    #[test]
    fn keyboard_and_ime_form_text_are_atomic_at_each_field_limit() {
        let mut app = App::new_headless(Config::default()).unwrap();
        app.commands_panel.toggle();
        exercise_atomic_form_input(&mut app);
    }

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
        exercise_atomic_form_input(&mut app);
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
        // Route a field click through the same layout/action path as native
        // mouse input, then insert IME text at that exact visible boundary.
        let info = app.commands_panel.build_info(&[]);
        let layout = crate::ui::commands::panel::CommandsPanelLayout::calculate(
            800.0, 800.0, 8.0, 16.0, 1.0, true, 0,
        );
        let view = layout
            .input_viewport(&info, crate::ui::commands::panel::EditMode::EditingName)
            .unwrap();
        app.handle_ime_event(winit::event::Ime::Preedit("draft".into(), None));
        let action = layout
            .input_click(&info, view.text_x + 8.0, view.text_y)
            .unwrap();
        app.apply_commands_panel_action(action);
        assert!(app.ime.preedit().is_none());
        app.handle_ime_event(winit::event::Ime::Commit("R".into()));
        assert_eq!(
            app.commands_panel.form_name.text(),
            "aRe\u{301}Q👩\u{200d}💻z"
        );
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
