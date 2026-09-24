//! Clipboard ownership for bounded single-line queries and command forms.

use super::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PasteTarget {
    Command,
    Unicode,
    Domain,
    Search,
    CommandsForm,
    WorkspaceRename,
    TabRename,
    ShortcutsFilter,
    SettingsFilter,
    Blocked,
    Terminal,
}

impl App {
    fn clipboard_paste_target(&self) -> PasteTarget {
        if self.guardrail.is_active()
            || self.info_overlay.is_active()
            || self.diff_review.is_active()
            || self.context_picker.is_active()
            || matches!(self.broadcast, BroadcastState::Armed { .. })
            || self.harvest.as_ref().is_some_and(|harvest| {
                harvest.sprout_menu.is_some() || harvest.cellar.is_expanded()
            })
        {
            return PasteTarget::Blocked;
        }
        // Match native keyboard dispatch, including retained background fields.
        if self.domain_picker.is_active() {
            return PasteTarget::Domain;
        }
        if self.unicode_picker.is_active() {
            return PasteTarget::Unicode;
        }
        if self.command_palette.is_active() {
            return PasteTarget::Command;
        }
        if self.shortcuts_panel.is_active() {
            return if self.shortcuts_panel.is_recording() {
                PasteTarget::Blocked
            } else {
                PasteTarget::ShortcutsFilter
            };
        }
        if self.settings_panel.is_active() {
            return if self.settings_panel.is_dropdown_open() {
                PasteTarget::Blocked
            } else {
                PasteTarget::SettingsFilter
            };
        }
        if self.workspace_menu.is_some() {
            return PasteTarget::Blocked;
        }
        if self.session_panel.is_open() {
            return if self.session_panel.is_renaming() {
                PasteTarget::WorkspaceRename
            } else {
                PasteTarget::Blocked
            };
        }
        if self.commands_panel.is_open() {
            return if self.commands_panel.is_editing() {
                PasteTarget::CommandsForm
            } else {
                PasteTarget::Blocked
            };
        }
        if self.agents_panel.is_open() {
            return PasteTarget::Blocked;
        }
        if self.tab_rename.is_some() {
            return PasteTarget::TabRename;
        }
        if self.search.is_active() {
            return PasteTarget::Search;
        }
        if self.file_drop_input_blocked() {
            PasteTarget::Blocked
        } else {
            PasteTarget::Terminal
        }
    }

    pub(super) fn clipboard_paste_blocked(&self) -> bool {
        self.clipboard_paste_target() == PasteTarget::Blocked
    }

    pub(super) fn handle_query_paste_key(&mut self, key: &winit::keyboard::Key) -> bool {
        if self.shortcut_registry.check(key, self.modifiers)
            != Some(crate::input::keybindings::BindableAction::Paste)
            || matches!(
                self.clipboard_paste_target(),
                PasteTarget::Blocked | PasteTarget::Terminal
            )
        {
            return false;
        }
        self.paste_clipboard();
        true
    }

    /// Clipboard-only dispatch. Protocol pastes and file drops keep their
    /// explicitly terminal-owned path; a rejected query never falls through.
    pub(super) fn paste_clipboard_payload(&mut self, text: &str) -> bool {
        let target = self.clipboard_paste_target();
        match target {
            PasteTarget::Blocked => return false,
            PasteTarget::Terminal => return self.write_paste_text(text),
            _ => {}
        }
        if text.is_empty() {
            return false;
        }
        let limit = if target == PasteTarget::CommandsForm {
            crate::ui::commands::panel::CommandsPanel::MAX_FORM_COMMAND_BYTES
        } else {
            crate::terminal::search::MAX_SEARCH_QUERY_BYTES
        };
        if text.len() > limit
            || text
                .chars()
                .any(|c| c.is_control() || matches!(c, '\u{2028}' | '\u{2029}'))
        {
            self.toasts.warning(
                "Field paste requires bounded single-line text without control characters",
            );
            return false;
        }
        let accepted = match target {
            PasteTarget::Command => self.command_palette.insert_query_text(text),
            PasteTarget::Unicode => self.unicode_picker.insert_query_text(text),
            PasteTarget::Domain => self.domain_picker.insert_query_text(text),
            PasteTarget::CommandsForm => self.commands_panel.insert_form_text(text),
            PasteTarget::WorkspaceRename => self.session_panel.insert_rename_text(text),
            PasteTarget::TabRename => self.insert_tab_rename_text(text),
            PasteTarget::ShortcutsFilter => self.shortcuts_panel.insert_filter_text(text),
            PasteTarget::SettingsFilter => self.settings_panel.insert_filter_text(text),
            PasteTarget::Search => {
                let cursor = self.search_cursor_pos;
                let accepted = self
                    .with_search_and_active_grid(|search, grid| {
                        search.insert_text(cursor, text, grid)
                    })
                    .unwrap_or(false);
                if accepted {
                    self.search_cursor_pos += text.len();
                }
                accepted
            }
            PasteTarget::Blocked | PasteTarget::Terminal => unreachable!(),
        };
        if accepted {
            self.ime.clear_preedit();
        } else {
            self.toasts.warning("Field paste was not inserted: the field is read-only or its size limit was exceeded");
        }
        self.redraw_scheduler
            .request(redraw_scheduler::RedrawReason::KeyboardInput);
        if let Some(window) = &self.window {
            window.request_redraw();
        }
        accepted
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::input::keybindings::{BindableAction, KeyChord};
    use crate::ui::text_input::TextInputEdit;
    use winit::keyboard::{Key, NamedKey};

    fn exercise_tab_rename_paste(app: &mut App) {
        let search_before = app.search.query().to_owned();
        for max_chars in [128, 256] {
            let tab = app.tabs.as_ref().and_then(TabManager::active_tab);
            let tab_id = tab.map_or(usize::MAX, |tab| tab.id);
            let original_title = tab.and_then(|tab| tab.user_title().map(str::to_owned));
            app.tab_rename = Some(TabRenameState {
                tab_id,
                original: original_title.clone(),
                input: "Roots".into(),
                max_chars,
            });
            assert_eq!(app.clipboard_paste_target(), PasteTarget::TabRename);
            app.handle_ime_event(winit::event::Ime::Preedit("draft".into(), None));
            for value in [
                "",
                "a\nb",
                "\r",
                "\t",
                "\0",
                "\x1b[200~",
                "\u{7f}",
                "\u{85}",
                "\u{2028}",
                "\u{2029}",
                &"x".repeat(max_chars + 1),
                &"x".repeat(4097),
            ] {
                assert!(!app.paste_clipboard_payload(value));
                assert_eq!(app.tab_rename.as_ref().unwrap().input, "Roots");
                assert!(app.ime.preedit().is_some());
                if let Some(tab) = app.tabs.as_ref().and_then(TabManager::active_tab) {
                    assert_eq!(tab.user_title(), original_title.as_deref());
                }
            }
            app.redraw_scheduler.begin_frame();
            assert!(app.paste_clipboard_payload("界e\u{301}👩\u{200d}💻"));
            assert!(app.ime.preedit().is_none());
            assert_eq!(app.redraw_scheduler.begin_frame(), "keyboard-input");
            let draft = app.tab_rename.as_ref().unwrap().input.clone();
            assert_eq!(draft, "Roots界e\u{301}👩\u{200d}💻");
            assert!(
                app.paste_clipboard_payload(&"🫚".repeat(max_chars - 1 - draft.chars().count()))
            );
            let almost = app.tab_rename.as_ref().unwrap().input.clone();
            app.handle_ime_event(winit::event::Ime::Preedit("keep".into(), None));
            assert!(!app.paste_clipboard_payload("e\u{301}"));
            assert_eq!(app.tab_rename.as_ref().unwrap().input, almost);
            assert!(app.ime.preedit().is_some());
            assert!(app.paste_clipboard_payload("🫚"));
            assert!(!app.paste_clipboard_payload("x"));
            assert_eq!(
                app.tab_rename.as_ref().unwrap().input.chars().count(),
                max_chars
            );
            assert_eq!(app.search.query(), search_before);
            assert!(app.pending_live_domain_controls.is_empty()); // no commit/request
            if let Some(tab) = app.tabs.as_ref().and_then(TabManager::active_tab) {
                assert_eq!(
                    tab.user_title(),
                    Some(app.tab_rename.as_ref().unwrap().input.as_str())
                );
            }
            assert!(app.handle_tab_rename_key(&Key::Named(NamedKey::Escape)));
            assert!(app.tab_rename.is_none());
            if let Some(tab) = app.tabs.as_ref().and_then(TabManager::active_tab) {
                assert_eq!(tab.user_title(), original_title.as_deref());
            }
        }
    }

    #[test]
    fn tab_rename_paste_is_atomic_at_both_title_limits_and_cancel_restores_preview() {
        let mut app = App::new_headless(Config::default()).unwrap();
        app.search.start();
        exercise_tab_rename_paste(&mut app);
    }

    #[test]
    fn tab_rename_paste_honors_rebinding_disabling_and_higher_modal_guards() {
        let mut app = App::new_headless(Config::default()).unwrap();
        app.tab_rename = Some(TabRenameState {
            tab_id: usize::MAX,
            original: None,
            input: "Roots".into(),
            max_chars: 256,
        });
        app.modifiers = ModifiersState::CONTROL | ModifiersState::SHIFT;
        let default = Key::Character("v".into());
        assert!(app.handle_query_paste_key(&default)); // no OS clipboard in headless mode
        let rebound = Key::Named(NamedKey::F8);
        app.shortcut_registry.set_binding(
            BindableAction::Paste,
            KeyChord::from_key_lookup(&rebound, ModifiersState::empty()),
        );
        assert!(!app.handle_query_paste_key(&default));
        app.modifiers = ModifiersState::empty();
        assert!(app.handle_query_paste_key(&rebound));
        app.commands_panel.toggle();
        assert!(app.clipboard_paste_blocked());
        assert!(!app.handle_query_paste_key(&rebound));
        assert!(!app.paste_clipboard_payload("hidden"));
        app.commands_panel.start_add();
        assert!(app.paste_clipboard_payload("front form"));
        assert_eq!(app.commands_panel.form_name.text(), "front form");
        assert_eq!(app.tab_rename.as_ref().unwrap().input, "Roots");
        app.commands_panel.close();
        app.workspace_menu = Some(crate::ui::popup_menu::PopupMenu::new(
            crate::ui::workspace_menu::workspace_menu_rows(&[], None),
            false,
        ));
        assert!(app.clipboard_paste_blocked());
        assert!(!app.paste_clipboard_payload("hidden"));
        app.workspace_menu = None;
        app.shortcut_registry.clear_binding(BindableAction::Paste);
        assert!(!app.handle_query_paste_key(&rebound));
        assert_eq!(app.tab_rename.as_ref().unwrap().input, "Roots");
    }

    fn open_workspace_rename(app: &mut App) {
        app.session_panel.set_test_entries(
            vec![crate::session::panel::SessionEntry {
                id: "paste-workspace-fixture".into(),
                display_name: "Roots".into(),
                subtitle: String::new(),
                last_modified: String::new(),
            }],
            true,
        );
        app.apply_session_panel_action(crate::session::panel::SessionPanelAction::Rename(
            "paste-workspace-fixture".into(),
        ));
    }

    fn exercise_workspace_rename_paste(app: &mut App) {
        open_workspace_rename(app);
        assert_eq!(app.clipboard_paste_target(), PasteTarget::WorkspaceRename);
        assert!(app.session_panel.accessibility_set_rename("az"));
        app.session_panel.rename_edit(TextInputEdit::Left);
        app.handle_ime_event(winit::event::Ime::Preedit("draft".into(), None));
        for rejected in [
            "",
            "a\nb",
            "\r",
            "\t",
            "\0",
            "\x1b[200~",
            "\u{7f}",
            "\u{85}",
            "\u{2028}",
            "\u{2029}",
            &"x".repeat(129),
            &"x".repeat(4097),
        ] {
            assert!(!app.paste_clipboard_payload(rejected));
            let info = app.session_panel.build_info();
            assert_eq!(info.rename_text.as_deref(), Some("az"));
            assert_eq!(info.rename_cursor, 1);
            assert!(app.ime.preedit().is_some());
        }
        app.redraw_scheduler.begin_frame();
        assert!(app.paste_clipboard_payload("界e\u{301}👩\u{200d}💻"));
        let before = app.session_panel.build_info();
        let text = before.rename_text.as_deref().unwrap();
        assert_eq!(text, "a界e\u{301}👩\u{200d}💻z");
        assert_eq!(before.rename_cursor, text.len() - 1);
        assert!(app.ime.preedit().is_none());
        assert_eq!(app.redraw_scheduler.begin_frame(), "keyboard-input");
        assert!(app.paste_clipboard_payload(&"🫚".repeat(127 - text.chars().count())));
        let almost = app.session_panel.build_info();
        app.handle_ime_event(winit::event::Ime::Preedit("keep".into(), None));
        assert!(!app.paste_clipboard_payload("e\u{301}"));
        assert_eq!(
            app.session_panel.build_info().rename_text,
            almost.rename_text
        );
        assert_eq!(
            app.session_panel.build_info().rename_cursor,
            almost.rename_cursor
        );
        assert!(app.ime.preedit().is_some());
        assert!(app.paste_clipboard_payload("🫚"));
        assert!(!app.paste_clipboard_payload("x"));
        let full = app.session_panel.build_info();
        let text = full.rename_text.unwrap();
        assert_eq!(text.chars().count(), 128);
        assert!(text.ends_with('z'));
        assert_eq!(full.rename_cursor, text.len() - 1);
        assert!(app.session_panel.is_renaming());
        assert_eq!(app.session_panel.sessions()[0].display_name, "Roots");
    }

    #[test]
    fn workspace_rename_paste_is_atomic_at_caret_and_never_submits() {
        let mut app = App::new_headless(Config::default()).unwrap();
        exercise_workspace_rename_paste(&mut app);
    }

    #[test]
    fn workspace_rename_paste_honors_binding_changes_and_blocks_browse_and_dropdown() {
        let mut app = App::new_headless(Config::default()).unwrap();
        open_workspace_rename(&mut app);
        app.modifiers = ModifiersState::CONTROL | ModifiersState::SHIFT;
        let default = Key::Character("v".into());
        // Headless: no OS clipboard is read, but the configured chord is owned.
        assert!(app.handle_query_paste_key(&default));
        let rebound = Key::Named(NamedKey::F8);
        app.shortcut_registry.set_binding(
            BindableAction::Paste,
            KeyChord::from_key_lookup(&rebound, ModifiersState::empty()),
        );
        assert!(!app.handle_query_paste_key(&default));
        app.modifiers = ModifiersState::empty();
        assert!(app.handle_query_paste_key(&rebound));
        app.workspace_menu = Some(crate::ui::popup_menu::PopupMenu::new(
            crate::ui::workspace_menu::workspace_menu_rows(&[], None),
            false,
        ));
        assert!(app.clipboard_paste_blocked());
        assert!(!app.handle_query_paste_key(&rebound));
        assert!(!app.paste_clipboard_payload("hidden"));
        app.workspace_menu = None;
        app.shortcut_registry.clear_binding(BindableAction::Paste);
        assert!(!app.handle_query_paste_key(&rebound));
        assert_eq!(
            app.session_panel.build_info().rename_text.as_deref(),
            Some("Roots")
        );
        app.apply_session_panel_action(crate::session::panel::SessionPanelAction::CancelRename);
        assert!(app.clipboard_paste_blocked());
        assert!(!app.paste_clipboard_payload("browse"));
        assert!(!app.session_panel.is_renaming());
        assert_eq!(app.session_panel.sessions()[0].display_name, "Roots");
    }

    #[test]
    fn workspace_paste_preserves_lower_fields_and_respects_higher_modal_owners() {
        for higher in 0..6 {
            let mut app = App::new_headless(Config::default()).unwrap();
            app.search.start();
            app.commands_panel.toggle();
            app.commands_panel.start_add();
            open_workspace_rename(&mut app);
            assert!(app.paste_clipboard_payload(" workspace"));
            match higher {
                0 => {
                    app.info_overlay.show("Notice", vec![]);
                }
                1 => app.settings_panel.toggle(),
                2 => app.shortcuts_panel.open(),
                3 => app.command_palette.open(),
                4 => app.unicode_picker.open(),
                5 => app.domain_picker.open(&app.config.domains),
                _ => unreachable!(),
            }
            assert_eq!(
                app.paste_clipboard_payload("higher"),
                (1..=5).contains(&higher)
            );
            assert_eq!(
                app.session_panel.build_info().rename_text.as_deref(),
                Some("Roots workspace")
            );
            assert!(app.search.query().is_empty());
            assert!(app.commands_panel.form_name.text().is_empty());
            assert_eq!(app.session_panel.sessions()[0].display_name, "Roots");
            match higher {
                1 => assert_eq!(
                    app.settings_panel
                        .build_info(&app.config, &app.shortcut_registry)
                        .filter,
                    "higher"
                ),
                2 => assert_eq!(
                    app.shortcuts_panel
                        .build_info(&app.shortcut_registry)
                        .filter,
                    "higher"
                ),
                3 => assert_eq!(app.command_palette.build_info().input, "higher"),
                4 => assert_eq!(app.unicode_picker.build_info().input, "higher"),
                5 => assert_eq!(app.domain_picker.build_info().input, "higher"),
                _ => assert!(app.clipboard_paste_blocked()),
            }
            app.unicode_picker.close();
            app.unicode_picker.finish_search_for_test();
        }
    }

    fn open_settings_dropdown(app: &mut App) {
        app.settings_panel
            .open_to_section(crate::ui::settings::SettingsSection::Theme);
        app.settings_panel
            .build_info(&app.config, &app.shortcut_registry);
        app.settings_panel.handle_key(
            crate::ui::settings::SettingsKey::Right,
            &app.config,
            &app.shortcut_registry,
        );
        assert!(app.settings_panel.is_dropdown_open());
    }

    #[test]
    fn settings_filter_paste_uses_atomic_scalar_limits_without_changing_configuration() {
        let mut app = App::new_headless(Config::default()).unwrap();
        let config_before = toml::to_string(&app.config).unwrap();
        app.settings_panel.toggle();
        assert_eq!(app.clipboard_paste_target(), PasteTarget::SettingsFilter);
        assert!(app.paste_clipboard_payload("e\u{301}👩\u{200d}💻"));
        assert!(app.paste_clipboard_payload("界"));
        let before = app
            .settings_panel
            .build_info(&app.config, &app.shortcut_registry);
        assert_eq!(before.filter, "e\u{301}👩\u{200d}💻界");
        assert_eq!(before.filter_cursor, before.filter.len());
        app.handle_ime_event(winit::event::Ime::Preedit("draft".into(), None));
        for rejected in [
            "",
            "\nexecute\n",
            "\t",
            "\u{1b}[200~",
            "\u{2028}",
            "\u{2029}",
            &"x".repeat(257),
            &"x".repeat(4097),
        ] {
            assert!(!app.paste_clipboard_payload(rejected));
            let info = app
                .settings_panel
                .build_info(&app.config, &app.shortcut_registry);
            assert_eq!(info.filter, before.filter);
            assert_eq!(info.filter_cursor, before.filter_cursor);
            assert!(app.ime.preedit().is_some());
        }
        assert!(app.paste_clipboard_payload(&"🫚".repeat(255 - before.filter.chars().count())));
        assert!(app.ime.preedit().is_none());
        let before = app
            .settings_panel
            .build_info(&app.config, &app.shortcut_registry);
        assert!(!app.paste_clipboard_payload("e\u{301}"));
        assert_eq!(
            app.settings_panel
                .build_info(&app.config, &app.shortcut_registry)
                .filter,
            before.filter
        );
        assert!(app.paste_clipboard_payload("🫚"));
        assert!(!app.paste_clipboard_payload("x"));
        assert_eq!(
            app.settings_panel
                .build_info(&app.config, &app.shortcut_registry)
                .filter
                .chars()
                .count(),
            256
        );
        assert_eq!(toml::to_string(&app.config).unwrap(), config_before);
        assert!(app.redraw_scheduler.has_pending());
    }

    #[test]
    fn settings_paste_honors_binding_changes_and_dropdown_refuses_clipboard_reads() {
        let mut app = App::new_headless(Config::default()).unwrap();
        app.settings_panel.toggle();
        app.modifiers = ModifiersState::CONTROL | ModifiersState::SHIFT;
        let default = Key::Character("v".into());
        assert!(app.handle_query_paste_key(&default));
        let rebound = Key::Named(NamedKey::F8);
        app.shortcut_registry.set_binding(
            BindableAction::Paste,
            KeyChord::from_key_lookup(&rebound, ModifiersState::empty()),
        );
        assert!(!app.handle_query_paste_key(&default));
        app.modifiers = ModifiersState::empty();
        assert!(app.handle_query_paste_key(&rebound));
        open_settings_dropdown(&mut app);
        assert!(app.clipboard_paste_blocked());
        assert!(!app.handle_query_paste_key(&rebound));
        assert!(!app.paste_clipboard_payload("must not select an option"));
        assert!(app.settings_panel.is_dropdown_open());
        assert!(
            app.settings_panel
                .build_info(&app.config, &app.shortcut_registry)
                .filter
                .is_empty()
        );
        app.settings_panel.close();
        app.settings_panel.toggle();
        app.shortcut_registry.clear_binding(BindableAction::Paste);
        assert!(!app.handle_query_paste_key(&rebound));
    }

    #[test]
    fn settings_paste_retains_lower_fields_and_respects_higher_modal_owners() {
        for higher in 0..5 {
            let mut app = App::new_headless(Config::default()).unwrap();
            app.search.start();
            app.commands_panel.toggle();
            app.commands_panel.start_add();
            app.session_panel.open_workspaces();
            app.settings_panel.toggle();
            assert!(app.paste_clipboard_payload("settings only"));
            assert!(app.search.query().is_empty());
            assert!(app.commands_panel.form_name.text().is_empty());
            match higher {
                0 => {
                    app.info_overlay.show("Notice", vec![]);
                }
                1 => app.shortcuts_panel.open(),
                2 => app.command_palette.open(),
                3 => app.unicode_picker.open(),
                4 => app.domain_picker.open(&app.config.domains),
                _ => unreachable!(),
            }
            assert_eq!(
                app.paste_clipboard_payload("higher"),
                (1..=4).contains(&higher)
            );
            assert_eq!(
                app.settings_panel
                    .build_info(&app.config, &app.shortcut_registry)
                    .filter,
                "settings only"
            );
            assert!(app.search.query().is_empty());
            assert!(app.commands_panel.form_name.text().is_empty());
            match higher {
                1 => assert_eq!(
                    app.shortcuts_panel
                        .build_info(&app.shortcut_registry)
                        .filter,
                    "higher"
                ),
                2 => assert_eq!(app.command_palette.build_info().input, "higher"),
                3 => assert_eq!(app.unicode_picker.build_info().input, "higher"),
                4 => assert_eq!(app.domain_picker.build_info().input, "higher"),
                _ => assert!(app.clipboard_paste_blocked()),
            }
        }
    }

    #[test]
    fn shortcut_filter_paste_is_atomic_at_caret_and_obeys_its_own_scalar_limit() {
        let mut app = App::new_headless(Config::default()).unwrap();
        app.shortcuts_panel.open();
        assert_eq!(app.clipboard_paste_target(), PasteTarget::ShortcutsFilter);
        assert!(app.paste_clipboard_payload("az"));
        app.shortcuts_panel.edit_filter(TextInputEdit::Left);
        app.handle_ime_event(winit::event::Ime::Preedit("draft".into(), None));
        for rejected in [
            "",
            "\n",
            "\t",
            "\u{1b}[200~",
            "\u{2028}",
            "\u{2029}",
            &"x".repeat(129),
            &"x".repeat(4097),
        ] {
            assert!(!app.paste_clipboard_payload(rejected));
            let info = app.shortcuts_panel.build_info(&app.shortcut_registry);
            assert_eq!(info.filter, "az");
            assert_eq!(info.cursor, 1);
            assert!(app.ime.preedit().is_some());
        }
        assert!(app.paste_clipboard_payload("界e\u{301}👩\u{200d}💻"));
        let before = app.shortcuts_panel.build_info(&app.shortcut_registry);
        assert_eq!(before.filter, "a界e\u{301}👩\u{200d}💻z");
        assert_eq!(before.cursor, before.filter.len() - 1);
        assert!(app.ime.preedit().is_none());
        let remaining = 128 - before.filter.chars().count();
        assert!(app.paste_clipboard_payload(&"🫚".repeat(remaining - 1)));
        let almost = app.shortcuts_panel.build_info(&app.shortcut_registry);
        assert!(!app.paste_clipboard_payload("e\u{301}"));
        let unchanged = app.shortcuts_panel.build_info(&app.shortcut_registry);
        assert_eq!(unchanged.filter, almost.filter);
        assert_eq!(unchanged.cursor, almost.cursor);
        assert!(app.paste_clipboard_payload("🫚"));
        assert!(!app.paste_clipboard_payload("x"));
        assert_eq!(
            app.shortcuts_panel
                .build_info(&app.shortcut_registry)
                .filter
                .chars()
                .count(),
            128
        );
        assert!(app.redraw_scheduler.has_pending());
    }

    #[test]
    fn shortcut_filter_paste_binding_honors_rebinding_disabling_and_recording() {
        let mut app = App::new_headless(Config::default()).unwrap();
        app.shortcuts_panel.open();
        app.modifiers = ModifiersState::CONTROL | ModifiersState::SHIFT;
        let default = Key::Character("v".into());
        assert!(app.handle_query_paste_key(&default));
        let rebound = Key::Named(NamedKey::F8);
        app.shortcut_registry.set_binding(
            BindableAction::Paste,
            KeyChord::from_key_lookup(&rebound, ModifiersState::empty()),
        );
        assert!(!app.handle_query_paste_key(&default));
        app.modifiers = ModifiersState::empty();
        assert!(app.handle_query_paste_key(&rebound));
        app.shortcuts_panel.start_recording(BindableAction::Copy);
        assert!(app.clipboard_paste_blocked());
        // Let the recording handler see the chord, without reading a clipboard.
        assert!(!app.handle_query_paste_key(&rebound));
        assert!(!app.paste_clipboard_payload("not a binding"));
        assert!(app.shortcuts_panel.is_recording());
        assert!(
            app.shortcuts_panel
                .build_info(&app.shortcut_registry)
                .filter
                .is_empty()
        );
        app.shortcuts_panel.open();
        app.shortcut_registry.clear_binding(BindableAction::Paste);
        assert!(!app.handle_query_paste_key(&rebound));
        assert!(app.config.keybindings.is_empty());
    }

    #[test]
    fn shortcut_filter_paste_retains_lower_fields_and_respects_higher_owners() {
        for higher in 0..5 {
            let mut app = App::new_headless(Config::default()).unwrap();
            app.search.start();
            app.commands_panel.toggle();
            app.commands_panel.start_add();
            app.settings_panel.toggle();
            app.shortcuts_panel.open();
            assert!(app.paste_clipboard_payload("filter"));
            assert!(app.search.query().is_empty());
            assert!(app.commands_panel.form_name.text().is_empty());
            match higher {
                0 => {
                    app.info_overlay.show("Notice", vec![]);
                }
                1 => app.command_palette.open(),
                2 => app.unicode_picker.open(),
                3 => app.domain_picker.open(&app.config.domains),
                4 => {
                    app.shortcuts_panel.start_recording(BindableAction::Copy);
                }
                _ => unreachable!(),
            }
            assert_eq!(
                app.paste_clipboard_payload("higher"),
                (1..=3).contains(&higher)
            );
            assert_eq!(
                app.shortcuts_panel
                    .build_info(&app.shortcut_registry)
                    .filter,
                "filter"
            );
            assert!(app.search.query().is_empty());
            assert!(app.commands_panel.form_name.text().is_empty());
            match higher {
                1 => assert_eq!(app.command_palette.build_info().input, "higher"),
                2 => assert_eq!(app.unicode_picker.build_info().input, "higher"),
                3 => assert_eq!(app.domain_picker.build_info().input, "higher"),
                _ => assert!(app.clipboard_paste_blocked()),
            }
        }
    }

    #[test]
    fn command_form_paste_honors_shortcuts_and_preserves_state_on_rejection() {
        let mut app = App::new_headless(Config::default()).unwrap();
        app.commands_panel.toggle();
        assert!(app.clipboard_paste_blocked());
        app.commands_panel.start_add();
        app.modifiers = ModifiersState::CONTROL | ModifiersState::SHIFT;
        assert!(app.handle_query_paste_key(&Key::Character("v".into())));
        let rebound = Key::Named(NamedKey::F8);
        app.shortcut_registry.set_binding(
            BindableAction::Paste,
            KeyChord::from_key_lookup(&rebound, ModifiersState::empty()),
        );
        assert!(!app.handle_query_paste_key(&Key::Character("v".into())));
        app.modifiers = ModifiersState::empty();
        assert!(app.handle_query_paste_key(&rebound));
        app.shortcut_registry.clear_binding(BindableAction::Paste);
        assert!(!app.handle_query_paste_key(&rebound));
        assert!(app.paste_clipboard_payload("az"));
        app.commands_panel.form_edit(TextInputEdit::Left);
        app.handle_ime_event(winit::event::Ime::Preedit("draft".into(), None));
        for rejected in ["\n", "\t", "\u{2028}", &"x".repeat(257)] {
            assert!(!app.paste_clipboard_payload(rejected));
            assert_eq!(app.commands_panel.form_name.text(), "az");
            assert_eq!(app.commands_panel.form_name.cursor(), 1);
            assert!(app.ime.preedit().is_some());
        }
        assert!(app.paste_clipboard_payload("界"));
        assert_eq!(app.commands_panel.form_name.text(), "a界z");
        assert!(app.ime.preedit().is_none());
        app.commands_panel.focus_command();
        // The form's 16 KiB command limit is not the search field's 4 KiB limit.
        assert!(app.paste_clipboard_payload(&"x".repeat(16 * 1024)));
        assert!(!app.paste_clipboard_payload("界"));
        assert_eq!(app.commands_panel.form_command.text().len(), 16 * 1024);
        assert!(app.commands_panel.is_editing());
        assert!(app.config.commands.is_empty()); // paste does not submit/save
        assert!(app.redraw_scheduler.has_pending());
    }

    #[test]
    fn command_form_paste_respects_higher_modal_owners_and_retained_search() {
        for open in [
            open_settings_dropdown as fn(&mut App),
            |app| {
                app.shortcuts_panel.open();
                app.shortcuts_panel.start_recording(BindableAction::Copy);
            },
            |app| app.session_panel.open_workspaces(),
            |app| {
                app.info_overlay.show("Notice", vec![]);
            },
        ] {
            let mut app = App::new_headless(Config::default()).unwrap();
            app.search.start();
            app.commands_panel.toggle();
            app.commands_panel.start_add();
            assert_eq!(app.clipboard_paste_target(), PasteTarget::CommandsForm);
            assert!(app.paste_clipboard_payload("form only"));
            assert!(app.search.query().is_empty());
            open(&mut app);
            assert_eq!(app.clipboard_paste_target(), PasteTarget::Blocked);
            assert!(!app.paste_clipboard_payload("hidden"));
            assert_eq!(app.commands_panel.form_name.text(), "form only");
            assert!(app.search.query().is_empty());
        }
        let mut app = App::new_headless(Config::default()).unwrap();
        app.commands_panel.toggle();
        app.commands_panel.start_add();
        app.command_palette.open();
        assert_eq!(app.clipboard_paste_target(), PasteTarget::Command);
        assert!(app.paste_clipboard_payload("palette only"));
        assert!(app.commands_panel.form_name.text().is_empty());
    }

    #[test]
    fn query_paste_honors_rebound_and_disabled_shortcuts_without_reading_for_blocked_owners() {
        let mut app = App::new_headless(Config::default()).unwrap();
        app.command_palette.open();
        app.modifiers = ModifiersState::CONTROL | ModifiersState::SHIFT;
        let default = Key::Character("v".into());
        // No platform clipboard exists in headless mode, but ownership still
        // consumes the chord instead of inserting its V or reaching a PTY.
        assert!(app.handle_query_paste_key(&default));
        let rebound = Key::Named(NamedKey::F8);
        app.shortcut_registry.set_binding(
            BindableAction::Paste,
            KeyChord::from_key_lookup(&rebound, ModifiersState::empty()),
        );
        assert!(!app.handle_query_paste_key(&default));
        app.modifiers = ModifiersState::empty();
        assert!(app.handle_query_paste_key(&rebound));
        app.info_overlay.show("Notice", vec![]);
        assert!(!app.handle_query_paste_key(&rebound));
        assert!(app.clipboard_paste_blocked());
        assert!(!app.paste_clipboard_payload("hidden"));
        assert_eq!(app.command_palette.build_info().input, "");
        app.shortcut_registry.clear_binding(BindableAction::Paste);
        assert!(!app.handle_query_paste_key(&rebound));
    }

    #[test]
    fn palette_query_paste_is_atomic_at_caret_and_retains_limits() {
        for kind in [
            PasteTarget::Command,
            PasteTarget::Unicode,
            PasteTarget::Domain,
        ] {
            let mut app = App::new_headless(Config::default()).unwrap();
            match kind {
                PasteTarget::Command => app.command_palette.open(),
                PasteTarget::Unicode => app.unicode_picker.open(),
                PasteTarget::Domain => app.domain_picker.open(&app.config.domains),
                _ => unreachable!(),
            }
            assert_eq!(app.clipboard_paste_target(), kind);
            assert!(app.paste_clipboard_payload("az"));
            assert!(app.handle_palette_edit_key(&Key::Named(NamedKey::ArrowLeft)));
            assert!(app.paste_clipboard_payload("界e\u{301}👩\u{200d}💻"));
            let info = |app: &App| match kind {
                PasteTarget::Command => app.command_palette.build_info(),
                PasteTarget::Unicode => app.unicode_picker.build_info(),
                PasteTarget::Domain => app.domain_picker.build_info(),
                _ => unreachable!(),
            };
            let before = info(&app);
            assert_eq!(before.input, "a界e\u{301}👩\u{200d}💻z");
            assert_eq!(before.cursor, before.input.len() - 1);
            for rejected in [
                "",
                "a\nb",
                "a\rb",
                "a\tb",
                "\u{1b}[31m",
                "\u{7f}",
                "\u{2028}",
                "\u{2029}",
                &"x".repeat(257),
                &"x".repeat(4097),
            ] {
                assert!(!app.paste_clipboard_payload(rejected));
                assert_eq!(info(&app).input, before.input);
                assert_eq!(info(&app).cursor, before.cursor);
            }
            let remaining = 256 - before.input.chars().count();
            assert!(app.paste_clipboard_payload(&"🫚".repeat(remaining)));
            assert_eq!(info(&app).input.chars().count(), 256);
            assert!(!app.paste_clipboard_payload("x"));
            assert_eq!(info(&app).input.chars().count(), 256);
            app.unicode_picker.close();
            app.unicode_picker.finish_search_for_test();
        }
    }

    #[test]
    fn retained_query_paste_respects_modal_priority_and_busy_domain_refusal() {
        let mut app = App::new_headless(Config::default()).unwrap();
        app.search.start();
        for open in [
            open_settings_dropdown as fn(&mut App),
            |app| {
                app.shortcuts_panel.open();
                app.shortcuts_panel.start_recording(BindableAction::Copy);
            },
            |app| app.commands_panel.toggle(),
            |app| app.session_panel.open_workspaces(),
        ] {
            let mut app = App::new_headless(Config::default()).unwrap();
            app.search.start();
            open(&mut app);
            assert_eq!(app.clipboard_paste_target(), PasteTarget::Blocked);
            assert!(!app.paste_clipboard_payload("hidden"));
            assert!(app.search.query().is_empty());
        }
        app.command_palette.open();
        assert!(app.paste_clipboard_payload("background"));
        app.unicode_picker.open();
        assert_eq!(app.clipboard_paste_target(), PasteTarget::Unicode);
        app.domain_picker.open(&app.config.domains);
        assert_eq!(app.clipboard_paste_target(), PasteTarget::Domain);
        assert!(app.domain_picker.activate().is_some());
        assert!(app.domain_picker.is_busy());
        assert!(!app.paste_clipboard_payload("local"));
        assert_eq!(app.command_palette.build_info().input, "background");
        assert!(app.unicode_picker.build_info().input.is_empty());
        assert!(app.search.query().is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn search_clipboard_paste_is_bounded_and_query_pastes_never_reach_raw_pty() {
        let mut tabs = TabManager::new(
            80,
            24,
            crate::config::ShellConfig::default(),
            100,
            1024 * 1024,
        );
        tabs.new_tab_with_command("/bin/sh", &[
            "-c".into(), "stty raw -echo; printf READY; dd bs=1 count=1 2>/dev/null | od -An -tx1; sleep 0.1".into(),
        ], None, Some("query-paste"), None, Default::default()).unwrap();
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
        assert!(app.paste_clipboard_payload("az"));
        app.search_cursor_pos = 1;
        app.handle_ime_event(winit::event::Ime::Preedit("draft".into(), None));
        assert!(app.paste_clipboard_payload("界e\u{301}"));
        assert!(app.ime.preedit().is_none());
        assert_eq!(app.search.query(), "a界e\u{301}z");
        assert_eq!(app.search_cursor_pos, "a界e\u{301}".len());
        assert!(!app.paste_clipboard_payload("\nexecute\n"));
        let remaining = crate::terminal::search::MAX_SEARCH_QUERY_BYTES - app.search.query().len();
        assert!(app.paste_clipboard_payload(&"x".repeat(remaining)));
        let full = app.search.query().to_string();
        assert!(!app.paste_clipboard_payload("界"));
        assert_eq!(app.search.query(), full);
        app.command_palette.open();
        assert!(app.paste_clipboard_payload("query only"));
        app.command_palette.edit_input(TextInputEdit::Home);
        assert!(!app.paste_clipboard_payload("\u{1b}[200~"));
        app.command_palette.close();
        app.commands_panel.toggle();
        app.commands_panel.start_add();
        assert!(app.paste_clipboard_payload("inert name"));
        assert!(!app.paste_clipboard_payload("\nexecute\n"));
        assert!(!app.paste_clipboard_payload(&"x".repeat(257)));
        app.commands_panel.focus_command();
        assert!(app.paste_clipboard_payload("not executed; just field text"));
        assert!(!app.paste_clipboard_payload("\u{1b}[201~"));
        assert!(!app.paste_clipboard_payload(&"x".repeat(16 * 1024 + 1)));
        app.commands_panel.cancel_edit();
        assert!(!app.paste_clipboard_payload("browse must not write"));
        app.commands_panel.close();
        app.shortcuts_panel.open();
        assert!(app.paste_clipboard_payload("az"));
        app.shortcuts_panel.edit_filter(TextInputEdit::Left);
        assert!(app.paste_clipboard_payload("界e\u{301}👩\u{200d}💻"));
        assert_eq!(
            app.shortcuts_panel
                .build_info(&app.shortcut_registry)
                .filter,
            "a界e\u{301}👩\u{200d}💻z"
        );
        assert!(!app.paste_clipboard_payload("\nexecute\n"));
        assert!(!app.paste_clipboard_payload(&"x".repeat(129)));
        app.shortcuts_panel.start_recording(BindableAction::Copy);
        assert!(!app.paste_clipboard_payload("recording must not write"));
        app.shortcuts_panel.close_immediately();
        app.settings_panel.toggle();
        assert!(app.paste_clipboard_payload("界e\u{301}👩\u{200d}💻"));
        assert!(!app.paste_clipboard_payload("\nexecute\n"));
        assert!(!app.paste_clipboard_payload(&"x".repeat(257)));
        open_settings_dropdown(&mut app);
        assert!(!app.paste_clipboard_payload("dropdown must not write"));
        app.settings_panel.close();
        exercise_workspace_rename_paste(&mut app);
        app.apply_session_panel_action(crate::session::panel::SessionPanelAction::CancelRename);
        assert!(!app.paste_clipboard_payload("browse must not write"));
        app.apply_session_panel_action(crate::session::panel::SessionPanelAction::Close);
        exercise_tab_rename_paste(&mut app);
        app.search.cancel();
        assert!(app.paste_clipboard_payload("Z"));
        wait_for(&mut app, "5a");
    }
}
