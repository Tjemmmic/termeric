//! Complete bounded Settings-filter events, separate from setting actions.

use super::*;
use crate::ui::settings::SettingsKey;
use crate::ui::text_input::TextInputEdit;
use winit::keyboard::{Key, NamedKey};

impl App {
    /// The Settings dialog's geometry: what it is drawn with, and so what its
    /// pointer presses and hover, IME caret, and accessibility use.
    pub(super) fn settings_layout(&self) -> crate::ui::settings::SettingsModalLayout {
        let metrics = self.ui_metrics();
        crate::ui::settings::SettingsModalLayout::calculate(
            metrics.width,
            metrics.height,
            metrics.cell_width,
            metrics.cell_height,
            self.settings_panel.animation_progress(),
        )
    }

    /// Keys for Settings: its own toggle shortcut, the filter, and the controls.
    pub(super) fn handle_settings_panel_key(&mut self, key: &Key) {
        // The same shortcut opens and closes Settings. Check it
        // before treating character keys as filter input.
        if self.shortcut_registry.check(key, self.modifiers)
            == Some(crate::input::keybindings::BindableAction::ToggleSettings)
        {
            self.toggle_settings_surface();
            self.request_redraw(RedrawReason::KeyboardInput);
            return;
        }

        if self.handle_settings_filter_key(key) {
            return;
        }
        let settings_key = match key {
            Key::Named(NamedKey::Escape) => Some(SettingsKey::Escape),
            Key::Named(NamedKey::ArrowUp) => Some(SettingsKey::Up),
            Key::Named(NamedKey::ArrowDown) => Some(SettingsKey::Down),
            Key::Named(NamedKey::ArrowLeft) => Some(SettingsKey::Left),
            Key::Named(NamedKey::ArrowRight) => Some(SettingsKey::Right),
            Key::Named(NamedKey::Enter) => Some(SettingsKey::Enter),
            Key::Named(NamedKey::Tab) if self.modifiers.shift_key() => Some(SettingsKey::BackTab),
            Key::Named(NamedKey::Tab) => Some(SettingsKey::Tab),
            Key::Character(c) if self.modifiers.control_key() => match c.as_str() {
                "s" | "S" => Some(SettingsKey::Save),
                _ => None,
            },
            _ => None,
        };

        if let Some(key) = settings_key {
            let action = self
                .settings_panel
                .handle_key(key, &self.config, &self.shortcut_registry);
            self.apply_settings_action(action);
            self.request_redraw(RedrawReason::KeyboardInput);
        }
    }

    pub(super) fn handle_settings_filter_key(&mut self, key: &Key) -> bool {
        if !self.settings_panel.is_active() {
            return false;
        }
        let edit = match key {
            // Plain arrows still adjust the selected setting/dropdown. Modified
            // arrows must never fall through to those configuration actions.
            Key::Named(NamedKey::ArrowLeft | NamedKey::ArrowRight)
                if self.modifiers.is_empty() && !self.alt_graph_active =>
            {
                return false;
            }
            Key::Named(NamedKey::ArrowLeft) => Some(TextInputEdit::WordLeft),
            Key::Named(NamedKey::ArrowRight) => Some(TextInputEdit::WordRight),
            Key::Named(NamedKey::Home) => Some(TextInputEdit::Home),
            Key::Named(NamedKey::End) => Some(TextInputEdit::End),
            Key::Named(NamedKey::Backspace) => Some(TextInputEdit::Backspace),
            Key::Named(NamedKey::Delete) => Some(TextInputEdit::Delete),
            _ => None,
        };
        if let Some(edit) = edit {
            let edit = match (self.modifiers, self.alt_graph_active, edit) {
                (_, true, _) => return true,
                (ModifiersState::CONTROL, false, TextInputEdit::Backspace) => {
                    TextInputEdit::BackspaceWord
                }
                (ModifiersState::CONTROL, false, TextInputEdit::Delete) => {
                    TextInputEdit::DeleteWord
                }
                (
                    ModifiersState::CONTROL,
                    false,
                    TextInputEdit::WordLeft | TextInputEdit::WordRight,
                ) => edit,
                (modifiers, false, _) if modifiers.is_empty() => edit,
                // Selection is not implemented; swallow unsupported modifiers.
                _ => return true,
            };
            if self.settings_panel.edit_filter(edit) {
                self.settings_filter_changed();
            }
            return true;
        }
        let changed = match key {
            Key::Character(text) => {
                // Preserve the explicit Save action, but never interpret
                // AltGraph-generated text as a Control shortcut.
                if !self.alt_graph_active
                    && self.modifiers == ModifiersState::CONTROL
                    && matches!(text.as_str(), "s" | "S")
                {
                    return false;
                }
                if !super::text_field_keys::types_text(self.modifiers, self.alt_graph_active) {
                    return true;
                }
                self.settings_panel.insert_filter_text(text)
            }
            Key::Named(NamedKey::Space) => {
                if !super::text_field_keys::types_text(self.modifiers, self.alt_graph_active) {
                    return true;
                }
                self.settings_panel.insert_filter_text(" ")
            }
            _ => return false,
        };
        if changed {
            self.settings_filter_changed();
        }
        true
    }

    fn settings_filter_changed(&mut self) {
        self.ime.clear_preedit();
        self.request_redraw(RedrawReason::KeyboardInput);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::settings::SettingsKey;

    #[test]
    fn settings_text_keeps_complete_events_and_altgraph_without_triggering_actions() {
        let mut app = App::new_headless(Config::default()).unwrap();
        let original_config = toml::to_string(&app.config).unwrap();
        let text = Key::Character("e\u{301}👩\u{200d}💻".into());
        assert!(!app.handle_settings_filter_key(&text));
        app.settings_panel.toggle();
        app.handle_ime_event(winit::event::Ime::Preedit("draft".into(), None));
        assert!(app.handle_settings_filter_key(&text));
        assert!(app.ime.preedit().is_none());
        assert_eq!(
            app.settings_panel
                .build_info(&app.config, &app.shortcut_registry)
                .filter,
            "e\u{301}👩\u{200d}💻"
        );
        for modifier in [
            ModifiersState::CONTROL,
            ModifiersState::ALT,
            ModifiersState::SUPER,
        ] {
            app.modifiers = modifier;
            assert!(app.handle_settings_filter_key(&Key::Character("x".into())));
            if modifier != ModifiersState::CONTROL {
                assert!(app.handle_settings_filter_key(&Key::Named(NamedKey::Backspace)));
            }
        }
        app.modifiers = ModifiersState::CONTROL;
        for text in ["s", "S"] {
            assert!(!app.handle_settings_filter_key(&Key::Character(text.into())));
        }
        // Settings has no reset chord: Ctrl+R is swallowed like any other.
        for text in ["r", "R"] {
            assert!(app.handle_settings_filter_key(&Key::Character(text.into())));
        }
        app.alt_graph_active = true;
        app.modifiers = ModifiersState::CONTROL | ModifiersState::ALT;
        assert!(app.handle_settings_filter_key(&Key::Character("s".into())));
        app.alt_graph_active = false;
        app.modifiers = ModifiersState::SHIFT;
        assert!(app.handle_settings_filter_key(&Key::Named(NamedKey::Space)));
        assert_eq!(
            app.settings_panel
                .build_info(&app.config, &app.shortcut_registry)
                .filter,
            "e\u{301}👩\u{200d}💻s "
        );
        assert_eq!(toml::to_string(&app.config).unwrap(), original_config);
        assert!(app.redraw_scheduler.has_pending());
    }

    #[test]
    fn settings_ime_and_keyboard_limits_never_leak_into_the_pty() {
        let mut app =
            App::new_headless_running(crate::test_support::FIRST_BYTE_PROBE, "settings-filter");
        app.wait_for_screen_text("READY");
        // The click below targets a layout computed before it; an opening
        // animation would move the field between the two.
        app.settings_panel.set_reduced_motion(true);
        app.settings_panel.toggle();
        assert!(app.handle_settings_filter_key(&Key::Character("e\u{301}".into())));
        app.handle_ime_event(winit::event::Ime::Commit("👩\u{200d}💻".into()));
        assert_eq!(
            app.settings_panel
                .build_info(&app.config, &app.shortcut_registry)
                .filter,
            "e\u{301}👩\u{200d}💻"
        );
        assert!(app.handle_settings_filter_key(&Key::Named(NamedKey::Backspace)));
        assert!(app.handle_settings_filter_key(&Key::Named(NamedKey::Backspace)));
        assert!(app.settings_panel.insert_filter_text(&"x".repeat(255)));
        for rejected in ["e\u{301}", "\nexecute", "\u{2028}", &"x".repeat(1025)] {
            app.handle_ime_event(winit::event::Ime::Commit(rejected.into()));
            assert!(app.handle_settings_filter_key(&Key::Character(rejected.into())));
            assert_eq!(
                app.settings_panel
                    .build_info(&app.config, &app.shortcut_registry)
                    .filter,
                "x".repeat(255)
            );
        }
        app.handle_ime_event(winit::event::Ime::Commit("界".into()));
        assert_eq!(
            app.settings_panel
                .build_info(&app.config, &app.shortcut_registry)
                .filter,
            format!("{}界", "x".repeat(255))
        );
        let info = app
            .settings_panel
            .build_info(&app.config, &app.shortcut_registry);
        let layout = app.settings_layout();
        let view = layout
            .input_viewport(&info.filter, info.filter_cursor)
            .unwrap();
        let action = app.settings_panel.handle_click(
            view.text_x,
            view.text_y,
            &app.config,
            &app.shortcut_registry,
            &layout,
        );
        app.apply_settings_action(action);
        assert!(app.handle_settings_filter_key(&Key::Named(NamedKey::Backspace)));
        app.handle_ime_event(winit::event::Ime::Commit("@".into()));
        let info = app
            .settings_panel
            .build_info(&app.config, &app.shortcut_registry);
        assert!(info.filter.contains('@'));
        assert!(info.filter.ends_with('界'));
        for (modifiers, key) in [
            (ModifiersState::empty(), NamedKey::Home),
            (ModifiersState::empty(), NamedKey::Delete),
            (ModifiersState::CONTROL, NamedKey::ArrowRight),
            (ModifiersState::CONTROL, NamedKey::ArrowLeft),
            (ModifiersState::CONTROL, NamedKey::Delete),
            (ModifiersState::empty(), NamedKey::End),
            (ModifiersState::CONTROL, NamedKey::Backspace),
        ] {
            app.modifiers = modifiers;
            assert!(app.handle_settings_filter_key(&Key::Named(key)));
        }
        app.settings_panel.close();
        app.assert_pty_untouched();
    }

    #[test]
    fn settings_filter_keyboard_navigation_and_deletion_preserve_config() {
        let mut app = App::new_headless(Config::default()).unwrap();
        app.settings_panel.toggle();
        let original = toml::to_string(&app.config).unwrap();
        assert!(app.settings_panel.insert_filter_text("e\u{301} draft size"));
        for (modifiers, key, text, cursor) in [
            (
                ModifiersState::empty(),
                NamedKey::Home,
                "e\u{301} draft size",
                0,
            ),
            (ModifiersState::empty(), NamedKey::Delete, " draft size", 0),
            (ModifiersState::empty(), NamedKey::Delete, "draft size", 0),
            (
                ModifiersState::CONTROL,
                NamedKey::ArrowRight,
                "draft size",
                5,
            ),
            (
                ModifiersState::CONTROL,
                NamedKey::ArrowLeft,
                "draft size",
                0,
            ),
            (ModifiersState::CONTROL, NamedKey::Delete, " size", 0),
            (ModifiersState::empty(), NamedKey::Delete, "size", 0),
            (ModifiersState::empty(), NamedKey::End, "size", 4),
            (ModifiersState::CONTROL, NamedKey::Backspace, "", 0),
        ] {
            app.handle_ime_event(winit::event::Ime::Preedit("draft".into(), None));
            app.modifiers = modifiers;
            assert!(app.handle_settings_filter_key(&Key::Named(key)));
            let info = app
                .settings_panel
                .build_info(&app.config, &app.shortcut_registry);
            assert_eq!((info.filter.as_str(), info.filter_cursor), (text, cursor));
            assert!(app.ime.preedit().is_none());
            assert_eq!(toml::to_string(&app.config).unwrap(), original);
        }
        assert!(app.redraw_scheduler.has_pending());
        app.modifiers = ModifiersState::empty();
        for key in [NamedKey::ArrowLeft, NamedKey::ArrowRight] {
            assert!(!app.handle_settings_filter_key(&Key::Named(key)));
        }
    }

    #[test]
    fn settings_filter_rejects_modified_edits_and_dropdown_mutation() {
        let mut app = App::new_headless(Config::default()).unwrap();
        app.settings_panel.toggle();
        assert!(app.settings_panel.insert_filter_text("size"));
        app.handle_ime_event(winit::event::Ime::Preedit("draft".into(), None));
        for modifiers in [
            ModifiersState::SHIFT,
            ModifiersState::ALT,
            ModifiersState::SUPER,
            ModifiersState::CONTROL | ModifiersState::SHIFT,
        ] {
            app.modifiers = modifiers;
            for key in [
                NamedKey::ArrowLeft,
                NamedKey::ArrowRight,
                NamedKey::Home,
                NamedKey::End,
                NamedKey::Backspace,
                NamedKey::Delete,
            ] {
                assert!(app.handle_settings_filter_key(&Key::Named(key)));
            }
        }
        app.alt_graph_active = true;
        app.modifiers = ModifiersState::CONTROL | ModifiersState::ALT;
        for key in [
            NamedKey::ArrowLeft,
            NamedKey::ArrowRight,
            NamedKey::Backspace,
            NamedKey::Delete,
        ] {
            assert!(app.handle_settings_filter_key(&Key::Named(key)));
        }
        let info = app
            .settings_panel
            .build_info(&app.config, &app.shortcut_registry);
        assert_eq!((info.filter.as_str(), info.filter_cursor), ("size", 4));
        assert!(app.ime.preedit().is_some());

        app.alt_graph_active = false;
        app.settings_panel
            .handle_key(SettingsKey::Escape, &app.config, &app.shortcut_registry);
        // Appearance's first row is the Theme dropdown.
        app.settings_panel
            .handle_key(SettingsKey::Right, &app.config, &app.shortcut_registry);
        assert!(app.settings_panel.is_dropdown_open());
        for modifiers in [ModifiersState::empty(), ModifiersState::CONTROL] {
            app.modifiers = modifiers;
            for key in [
                NamedKey::Home,
                NamedKey::End,
                NamedKey::Backspace,
                NamedKey::Delete,
            ] {
                assert!(app.handle_settings_filter_key(&Key::Named(key)));
            }
        }
        assert!(app.settings_panel.is_dropdown_open());
        assert!(
            app.settings_panel
                .build_info(&app.config, &app.shortcut_registry)
                .filter
                .is_empty()
        );
    }

    #[test]
    fn settings_pointer_caret_action_clears_preedit_without_applying_configuration() {
        let mut app = App::new_headless(Config::default()).unwrap();
        let original_config = toml::to_string(&app.config).unwrap();
        app.settings_panel.set_reduced_motion(true);
        app.settings_panel.toggle();
        assert!(app.settings_panel.insert_filter_text("e\u{301}界"));
        app.handle_ime_event(winit::event::Ime::Preedit("draft".into(), None));
        app.apply_settings_action(SettingsAction::PlaceFilterCaret(1));
        assert!(app.ime.preedit().is_some());
        app.apply_settings_action(SettingsAction::PlaceFilterCaret(3));
        assert!(app.ime.preedit().is_none());
        assert!(app.paste_clipboard_payload("@"));
        assert_eq!(
            app.settings_panel
                .build_info(&app.config, &app.shortcut_registry)
                .filter,
            "e\u{301}@界"
        );
        assert_eq!(toml::to_string(&app.config).unwrap(), original_config);
        assert!(app.redraw_scheduler.has_pending());
        app.settings_panel.close();
        app.apply_settings_action(SettingsAction::PlaceFilterCaret(0));
        assert_eq!(
            app.settings_panel
                .build_info(&app.config, &app.shortcut_registry)
                .filter_cursor,
            4
        );
    }

    #[test]
    fn settings_report_unsaved_changes_until_the_file_matches_again() {
        use crate::ui::settings::SettingsAction;
        let mut app = App::new_headless(Config::default()).unwrap();
        assert!(!app.config_has_unsaved_changes());
        let visual = app.config.bell.visual;
        app.apply_settings_action(SettingsAction::SetVisualBell(!visual));
        assert!(app.config_has_unsaved_changes());
        app.apply_settings_action(SettingsAction::SetVisualBell(visual));
        assert!(
            !app.config_has_unsaved_changes(),
            "undoing a change is clean"
        );
        app.apply_settings_action(SettingsAction::SetVisualBell(!visual));
        // Saving or reloading records the file's contents as the baseline.
        app.mark_config_saved();
        assert!(!app.config_has_unsaved_changes());
    }

    #[test]
    fn keyboard_page_hands_off_to_the_shortcuts_dialog() {
        use crate::ui::settings::SettingsAction;
        let mut app = App::new_headless(Config::default()).unwrap();
        app.settings_panel.toggle();
        app.apply_settings_action(SettingsAction::OpenKeyboardShortcuts);
        assert!(!app.settings_panel.is_active());
        assert!(app.shortcuts_panel.is_active());
    }
}
