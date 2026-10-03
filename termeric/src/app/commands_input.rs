//! Inline command-form keyboard ownership after higher-priority surfaces.

use super::text_field_keys::FieldKey;
use super::*;
use crate::ui::commands::panel::CommandsPanelAction;
use winit::keyboard::{Key, NamedKey};

impl App {
    /// Called after higher wheel owners; even a horizontal/invalid event stays modal.
    pub(super) fn handle_commands_panel_scroll(
        &mut self,
        delta: winit::event::MouseScrollDelta,
    ) -> bool {
        if !self.commands_panel.is_open() {
            return false;
        }
        let lines = match delta {
            winit::event::MouseScrollDelta::LineDelta(_, y) => f64::from(y),
            winit::event::MouseScrollDelta::PixelDelta(pos) => pos.y / 20.0,
        };
        if lines.is_finite() {
            let rows = if lines > 0.0 {
                -(lines.ceil() as isize)
            } else {
                (-lines).ceil() as isize
            };
            self.scroll_commands_panel(rows);
        }
        true
    }

    pub(super) fn commands_panel_layout(&self) -> crate::ui::commands::panel::CommandsPanelLayout {
        let (width, height, top, cell_width, cell_height) = self
            .renderer
            .as_ref()
            .map(|renderer| {
                let (width, height) = renderer.window_size();
                (
                    width as f32,
                    height as f32,
                    renderer.tab_bar_height(),
                    renderer.cell_width(),
                    renderer.cell_height(),
                )
            })
            .unwrap_or((800.0, 600.0, 0.0, 8.0, 16.0));
        // Read only panel state, without copying command/form strings on wheel events.
        self.commands_panel.layout(
            width,
            height,
            top,
            cell_width,
            cell_height,
            self.config.commands.len(),
        )
    }

    pub(super) fn scroll_commands_panel(&mut self, delta: isize) {
        let layout = self.commands_panel_layout();
        if self.commands_panel.scroll_rows(delta, layout) {
            self.redraw_scheduler
                .request(redraw_scheduler::RedrawReason::PointerInput);
            if let Some(window) = &self.window {
                window.request_redraw();
            }
        }
    }

    /// The command manager is modal in browse mode too. A retained rename
    /// or search, and the PTY behind them, must never receive its keystrokes.
    pub(super) fn handle_commands_panel_key(&mut self, key: &Key) -> bool {
        if !self.commands_panel.is_open() {
            return false;
        }
        if self.commands_panel.is_editing() {
            return self.handle_commands_form_key(key);
        }
        use crate::ui::commands::navigation::CommandNavigationKey;
        if self.alt_graph_active {
            return true;
        }
        let plain = self.modifiers.is_empty();
        let navigation = match key {
            Key::Named(NamedKey::Tab) if self.modifiers == ModifiersState::SHIFT => {
                Some(CommandNavigationKey::Previous)
            }
            Key::Named(NamedKey::Tab | NamedKey::ArrowDown | NamedKey::ArrowRight) if plain => {
                Some(CommandNavigationKey::Next)
            }
            Key::Named(NamedKey::ArrowUp | NamedKey::ArrowLeft) if plain => {
                Some(CommandNavigationKey::Previous)
            }
            Key::Named(NamedKey::Home) if plain => Some(CommandNavigationKey::First),
            Key::Named(NamedKey::End) if plain => Some(CommandNavigationKey::Last),
            _ => None,
        };
        let layout = self.commands_panel_layout();
        if let Some(navigation) = navigation {
            self.commands_panel
                .navigate_browse(navigation, &self.config.commands, layout);
        } else {
            let action = match key {
                Key::Named(NamedKey::Escape) if plain => CommandsPanelAction::Close,
                Key::Named(NamedKey::Enter | NamedKey::Space) if plain => self
                    .commands_panel
                    .activate_browse(&self.config.commands, layout),
                Key::Character(text) if plain && text == " " => self
                    .commands_panel
                    .activate_browse(&self.config.commands, layout),
                _ => return true,
            };
            self.apply_commands_panel_action(action);
        }
        self.clear_commands_accessibility_focus();
        self.ime.clear_preedit();
        self.redraw_scheduler
            .request(redraw_scheduler::RedrawReason::KeyboardInput);
        if let Some(window) = &self.window {
            window.request_redraw();
        }
        true
    }

    pub(super) fn handle_commands_form_key(&mut self, key: &Key) -> bool {
        if !self.commands_panel.is_open() || !self.commands_panel.is_editing() {
            return false;
        }
        if let Some(field_key) = self.text_edit_for_key(key) {
            let FieldKey::Edit(edit) = field_key else {
                return true;
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
        self.clear_commands_accessibility_focus();
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

    fn exercise_browse_navigation(app: &mut App) {
        use crate::ui::commands::navigation::CommandControl;
        let original = std::mem::take(&mut app.config.commands);
        app.config.commands = (0..20)
            .map(|index| crate::config::CustomCommand {
                name: format!("Command {index}"),
                command: "inert: never run".into(),
                alias: None,
            })
            .collect();
        app.commands_panel.close();
        app.commands_panel.toggle();
        app.modifiers = ModifiersState::empty();
        app.redraw_scheduler.begin_frame();
        assert!(app.handle_commands_panel_key(&Key::Named(NamedKey::Tab)));
        assert_eq!(
            app.commands_panel.browse_focus(&app.config.commands),
            Some(CommandControl::Run(0))
        );
        assert_eq!(app.redraw_scheduler.begin_frame(), "keyboard-input");
        assert!(app.handle_commands_panel_key(&Key::Named(NamedKey::ArrowRight)));
        assert_eq!(
            app.commands_panel.browse_focus(&app.config.commands),
            Some(CommandControl::Delete(0))
        );
        app.modifiers = ModifiersState::SHIFT;
        assert!(app.handle_commands_panel_key(&Key::Named(NamedKey::Tab)));
        assert_eq!(
            app.commands_panel.browse_focus(&app.config.commands),
            Some(CommandControl::Run(0))
        );
        for modifiers in [
            ModifiersState::CONTROL,
            ModifiersState::ALT,
            ModifiersState::SUPER,
            ModifiersState::SHIFT,
        ] {
            app.modifiers = modifiers;
            for key in [
                NamedKey::Enter,
                NamedKey::Space,
                NamedKey::Escape,
                NamedKey::ArrowDown,
                NamedKey::Home,
                NamedKey::End,
                NamedKey::Delete,
            ] {
                assert!(app.handle_commands_panel_key(&Key::Named(key)));
                assert_eq!(
                    app.commands_panel.browse_focus(&app.config.commands),
                    Some(CommandControl::Run(0))
                );
                assert!(app.commands_panel.is_open());
                assert!(!app.commands_panel.is_editing());
            }
        }
        app.modifiers = ModifiersState::empty();
        app.alt_graph_active = true;
        assert!(app.handle_commands_panel_key(&Key::Named(NamedKey::Enter)));
        app.alt_graph_active = false;
        // Plain Delete is not a destructive shortcut; only activation of the Delete button is.
        assert!(app.handle_commands_panel_key(&Key::Named(NamedKey::Delete)));
        assert_eq!(app.config.commands.len(), 20);
        app.config.commands[0].command = "replacement".into();
        assert!(app.handle_commands_panel_key(&Key::Named(NamedKey::Enter)));
        assert!(app.commands_panel.is_open());
        assert!(!app.commands_panel.is_editing());
        assert_eq!(
            app.commands_panel.browse_focus(&app.config.commands),
            Some(CommandControl::New)
        );
        assert!(app.handle_commands_panel_key(&Key::Named(NamedKey::Enter)));
        assert!(app.commands_panel.is_editing());
        assert!(app.handle_commands_panel_key(&Key::Named(NamedKey::Escape)));
        assert!(!app.commands_panel.is_editing());
        // Space activates the New button through the same application action path.
        assert!(app.handle_commands_panel_key(&Key::Character(" ".into())));
        assert!(app.commands_panel.is_editing());
        assert!(app.handle_commands_panel_key(&Key::Named(NamedKey::Escape)));
        assert!(app.handle_commands_panel_key(&Key::Named(NamedKey::End)));
        assert_eq!(
            app.commands_panel.browse_focus(&app.config.commands),
            Some(CommandControl::Close)
        );
        assert!(app.handle_commands_panel_key(&Key::Named(NamedKey::Enter)));
        assert!(!app.commands_panel.is_open());
        app.config.commands = original;
    }

    #[test]
    fn command_keyboard_navigation_modifiers_and_safe_actions_stay_modal() {
        let mut app = App::new_headless(Config::default()).unwrap();
        exercise_browse_navigation(&mut app);
    }

    fn exercise_command_scrolling(app: &mut App) {
        use winit::dpi::PhysicalPosition;
        use winit::event::{Ime, MouseScrollDelta};

        let original = std::mem::take(&mut app.config.commands);
        app.config.commands = (0..100)
            .map(|index| crate::config::CustomCommand {
                name: format!("Command {index}"),
                command: "inert: must never run".into(),
                alias: None,
            })
            .collect();
        app.commands_panel.close();
        assert!(!app.handle_commands_panel_scroll(MouseScrollDelta::LineDelta(0.0, -1.0)));
        app.commands_panel.toggle();
        app.commands_panel.start_add();
        app.commands_panel.form_name.replace("kept draft");
        app.handle_ime_event(Ime::Preedit("kept preedit".into(), None));
        let initial = app.commands_panel_layout();
        app.redraw_scheduler.begin_frame();
        assert!(app.handle_commands_panel_scroll(MouseScrollDelta::LineDelta(0.0, -3.0)));
        assert_eq!(app.commands_panel_layout().scroll_offset, 3);
        assert_eq!(app.redraw_scheduler.begin_frame(), "pointer-input");
        assert!(
            app.handle_commands_panel_scroll(MouseScrollDelta::PixelDelta(PhysicalPosition::new(
                0.0, 21.0
            )))
        );
        assert_eq!(app.commands_panel_layout().scroll_offset, 1);
        for delta in [
            MouseScrollDelta::LineDelta(5.0, 0.0),
            MouseScrollDelta::LineDelta(0.0, f32::NAN),
            MouseScrollDelta::PixelDelta(PhysicalPosition::new(0.0, f64::INFINITY)),
        ] {
            assert!(app.handle_commands_panel_scroll(delta));
            assert_eq!(app.commands_panel_layout().scroll_offset, 1);
        }
        assert!(app.handle_commands_panel_scroll(MouseScrollDelta::LineDelta(0.0, -f32::MAX)));
        let last = app.commands_panel_layout();
        assert_eq!(last.visible_range().end, 100);
        assert_eq!(last.form, initial.form);
        assert_eq!(app.commands_panel.form_name.text(), "kept draft");
        assert!(app.ime.preedit().is_some());
        assert_eq!(app.config.commands.len(), 100);
        app.config.commands = original;
        app.commands_panel.close();
    }

    #[test]
    fn command_wheel_scroll_is_bounded_and_preserves_form_and_composition() {
        let mut app = App::new_headless(Config::default()).unwrap();
        app.commands_panel.set_reduced_motion(true);
        exercise_command_scrolling(&mut app);
    }

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
    fn browse_panel_consumes_text_and_navigation_without_mutating_retained_fields() {
        let mut app = App::new_headless(Config::default()).unwrap();
        assert!(!app.handle_commands_panel_key(&Key::Named(NamedKey::Enter)));
        app.search.start();
        app.commands_panel.toggle();
        for key in [
            Key::Character("x".into()),
            Key::Named(NamedKey::Backspace),
            Key::Named(NamedKey::ArrowDown),
        ] {
            assert!(app.handle_commands_panel_key(&key));
            assert!(app.commands_panel.is_open());
            assert!(!app.commands_panel.is_editing());
            assert!(app.search.query().is_empty());
        }
        assert!(app.handle_commands_panel_key(&Key::Named(NamedKey::Escape)));
        assert!(!app.commands_panel.is_open());
        assert!(app.search.is_active());
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
    fn keyboard_run_activates_only_the_selected_command_in_an_inert_raw_pty() {
        let mut tabs = TabManager::new(
            80,
            24,
            crate::config::ShellConfig::default(),
            100,
            1024 * 1024,
        );
        tabs.new_tab_with_command("/bin/sh", &[
            "-c".into(), "stty raw -echo; printf READY; dd bs=1 count=3 2>/dev/null | od -An -tx1; sleep 0.1".into(),
        ], None, Some("command-navigation"), None, Default::default()).unwrap();
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
        app.config.commands = ["A", "B"]
            .into_iter()
            .map(|text| crate::config::CustomCommand {
                name: format!("Entry {text}"),
                command: text.into(),
                alias: None,
            })
            .collect();
        // The selected payload collides with another configured command name.
        app.config.commands.push(crate::config::CustomCommand {
            name: "B".into(),
            command: "wrong".into(),
            alias: None,
        });
        app.commands_panel.toggle();
        for _ in 0..3 {
            assert!(app.handle_commands_panel_key(&Key::Named(NamedKey::Tab)));
        }
        assert!(app.handle_commands_panel_key(&Key::Named(NamedKey::Enter)));
        assert!(!app.commands_panel.is_open());
        // These bytes go to `dd`, not a shell command interpreter: clear line, B, Enter.
        wait_for(&mut app, "15 42 0d");
        assert_eq!(app.config.commands.len(), 3);
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
            800.0, 800.0, 0.0, 8.0, 16.0, 1.0, true, 0,
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
        app.commands_panel.set_reduced_motion(true);
        exercise_command_scrolling(&mut app);
        exercise_browse_navigation(&mut app);
        app.commands_panel.close();
        app.write_to_pty(b"Z");
        wait_for(&mut app, "5a"); // first raw byte: no editing/IME bytes leaked
    }
}
