//! Complete bounded Settings-filter events, separate from setting actions.

use super::*;
use winit::keyboard::{Key, NamedKey};

impl App {
    pub(super) fn handle_settings_filter_key(&mut self, key: &Key) -> bool {
        if !self.settings_panel.is_active() {
            return false;
        }
        let changed = match key {
            Key::Named(NamedKey::Backspace) => {
                if !self.modifiers.is_empty() || self.alt_graph_active {
                    return true;
                }
                self.settings_panel.backspace_filter()
            }
            Key::Character(text) => {
                // Preserve explicit Save/Reset actions, but never interpret
                // AltGraph-generated text as a Control shortcut.
                if !self.alt_graph_active
                    && self.modifiers == ModifiersState::CONTROL
                    && matches!(text.as_str(), "s" | "S" | "r" | "R")
                {
                    return false;
                }
                if !self.alt_graph_active
                    && self.modifiers.intersects(
                        ModifiersState::CONTROL | ModifiersState::ALT | ModifiersState::SUPER,
                    )
                {
                    return true;
                }
                self.settings_panel.insert_filter_text(text)
            }
            Key::Named(NamedKey::Space) => {
                if !self.alt_graph_active
                    && self.modifiers.intersects(
                        ModifiersState::CONTROL | ModifiersState::ALT | ModifiersState::SUPER,
                    )
                {
                    return true;
                }
                self.settings_panel.insert_filter_text(" ")
            }
            _ => return false,
        };
        if changed {
            self.ime.clear_preedit();
            self.redraw_scheduler
                .request(redraw_scheduler::RedrawReason::KeyboardInput);
            if let Some(window) = &self.window {
                window.request_redraw();
            }
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
            assert!(app.handle_settings_filter_key(&Key::Named(NamedKey::Backspace)));
        }
        app.modifiers = ModifiersState::CONTROL;
        for text in ["s", "S", "r", "R"] {
            assert!(!app.handle_settings_filter_key(&Key::Character(text.into())));
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

    #[cfg(unix)]
    #[test]
    fn settings_ime_and_keyboard_limits_never_leak_into_the_pty() {
        let mut tabs = TabManager::new(
            80,
            24,
            crate::config::ShellConfig::default(),
            100,
            1024 * 1024,
        );
        tabs.new_tab_with_command("/bin/sh", &[
            "-c".into(), "stty raw -echo; printf READY; dd bs=1 count=1 2>/dev/null | od -An -tx1; sleep 0.1".into(),
        ], None, Some("settings-filter"), None, Default::default()).unwrap();
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
        let layout = crate::ui::settings::SettingsModalLayout::calculate(
            800.0,
            700.0,
            8.0,
            16.0,
            info.animation,
            info.entries.len(),
        );
        let view = layout
            .input_viewport(&info.filter, info.filter_cursor, 16.0)
            .unwrap();
        let action = app.settings_panel.handle_click(
            view.text_x,
            view.text_y,
            &app.config,
            &app.shortcut_registry,
            800.0,
            700.0,
            8.0,
            16.0,
        );
        app.apply_settings_action(action);
        assert!(app.handle_settings_filter_key(&Key::Named(NamedKey::Backspace)));
        app.handle_ime_event(winit::event::Ime::Commit("@".into()));
        let info = app
            .settings_panel
            .build_info(&app.config, &app.shortcut_registry);
        assert!(info.filter.contains('@'));
        assert!(info.filter.ends_with('界'));
        app.settings_panel.close();
        app.write_to_pty(b"Z");
        wait_for(&mut app, "5a");
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
}
