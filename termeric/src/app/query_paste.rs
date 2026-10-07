//! Clipboard ownership for bounded single-line queries and command forms.

use super::text_target::TextTarget;
use super::*;

impl App {
    pub(super) fn clipboard_paste_blocked(&self) -> bool {
        self.text_target() == TextTarget::Blocked
    }

    pub(super) fn handle_query_paste_key(&mut self, key: &winit::keyboard::Key) -> bool {
        if self.shortcut_registry.check(key, self.modifiers)
            != Some(crate::input::keybindings::BindableAction::Paste)
            || matches!(
                self.text_target(),
                TextTarget::Blocked | TextTarget::Terminal
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
        let target = self.text_target();
        match target {
            TextTarget::Blocked => return false,
            TextTarget::Terminal => return self.write_paste_text(text),
            TextTarget::AgentView => return self.paste_into_agent_view(text),
            _ => {}
        }
        self.paste_into_field(target, text)
    }

    /// A middle click on the find box's query pastes the primary selection
    /// where it landed, under the same limits as a keyboard paste into it.
    pub(super) fn paste_primary_into_search(&mut self, cursor: usize) -> bool {
        self.focus_search_query_at(cursor, RedrawReason::PointerInput)
            && self.paste_primary_into_field(TextTarget::Search)
    }

    /// Pastes the primary selection into `target`, a field a middle click
    /// has just focused, when it is still where text goes.
    pub(super) fn paste_primary_into_field(&mut self, target: TextTarget) -> bool {
        if self.text_target() != target {
            return false;
        }
        let Some(text) = self.primary_selection_text() else {
            return false;
        };
        self.paste_into_field(target, &text)
    }

    /// A paste into one of the single-line fields: all of `text`, or none of
    /// it when it is empty, too long, or breaks the line.
    fn paste_into_field(&mut self, target: TextTarget, text: &str) -> bool {
        if text.is_empty() {
            return false;
        }
        let limit = if target == TextTarget::CommandsForm {
            crate::ui::commands::panel::CommandsPanel::MAX_FORM_COMMAND_BYTES
        } else {
            crate::terminal::search::MAX_SEARCH_QUERY_BYTES
        };
        if text.len() > limit || text.chars().any(crate::safe_text::breaks_single_line) {
            self.toasts.warning(
                "Field paste requires bounded single-line text without control characters",
            );
            return false;
        }
        let accepted = match target {
            TextTarget::Palette(modal) => self.palette_owner_mut(modal).insert_query_text(text),
            TextTarget::CommandsForm => self.commands_panel.insert_form_text(text),
            TextTarget::WorkspaceRename => self.session_panel.insert_rename_text(text),
            TextTarget::TabRename => self.insert_tab_rename_text(text),
            TextTarget::ShortcutsFilter => self.shortcuts_panel.insert_filter_text(text),
            TextTarget::SettingsFilter => self.settings_panel.insert_filter_text(text),
            TextTarget::InfoOverlayInput => {
                let request_id = self.info_overlay.request_id();
                self.info_overlay.insert_input_text(request_id, text)
            }
            TextTarget::Search => self.insert_search_text(text),
            TextTarget::Blocked | TextTarget::Terminal | TextTarget::AgentView => unreachable!(),
        };
        if accepted {
            self.ime.clear_preedit();
        } else {
            self.toasts.warning("Field paste was not inserted: the field is read-only or its size limit was exceeded");
        }
        self.request_redraw(RedrawReason::KeyboardInput);

        accepted
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::palette_input::PaletteModal;
    use crate::input::keybindings::{BindableAction, KeyChord};
    use crate::ui::text_input::TextInputEdit;
    use winit::keyboard::{Key, NamedKey};

    /// The focused paste owner takes the configured Paste chord and only it:
    /// the default chord, then a rebinding, and nothing once Paste is unbound.
    /// Headless mode has no OS clipboard, so an owned chord reads nothing but
    /// is still consumed instead of inserting its letter or reaching a PTY.
    /// Leaves Paste bound to F8 with no modifiers held, and returns that key.
    fn assert_paste_binding_follows_registry(app: &mut App) -> Key {
        let default = Key::Character("v".into());
        app.modifiers = ModifiersState::CONTROL | ModifiersState::SHIFT;
        assert!(app.handle_query_paste_key(&default));
        let rebound = Key::Named(NamedKey::F8);
        let chord = KeyChord::from_key_event(&rebound, ModifiersState::empty());
        app.shortcut_registry
            .set_binding(BindableAction::Paste, chord.clone());
        assert!(!app.handle_query_paste_key(&default));
        app.modifiers = ModifiersState::empty();
        assert!(app.handle_query_paste_key(&rebound));
        app.shortcut_registry
            .set_binding(BindableAction::Paste, None);
        assert!(!app.handle_query_paste_key(&rebound));
        app.shortcut_registry
            .set_binding(BindableAction::Paste, chord);
        rebound
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

    fn settings_filter(app: &mut App) -> String {
        app.settings_panel
            .build_info(&app.config, &app.shortcut_registry)
            .filter
    }

    fn shortcuts_filter(app: &mut App) -> String {
        app.shortcuts_panel
            .build_info(&app.shortcut_registry)
            .filter
    }

    /// A modal opened above the field under test.
    #[derive(Clone, Copy, Debug)]
    enum Higher {
        InfoOverlay,
        /// An info overlay with a focused text field, as the font editors show.
        InfoOverlayInput,
        Settings,
        Shortcuts,
        Palette,
        Unicode,
        Domains,
        /// Shortcut recording, over an open Shortcuts panel.
        Recording,
    }

    impl Higher {
        fn open(self, app: &mut App) {
            match self {
                Self::InfoOverlay => {
                    app.info_overlay.show("Notice", vec![]);
                }
                Self::InfoOverlayInput => {
                    app.info_overlay.show_input(
                        "Rule",
                        vec!["Rule: ".into()],
                        0,
                        "Rule: ",
                        "",
                        64,
                        Vec::new(),
                    );
                }
                Self::Settings => app.settings_panel.toggle(),
                Self::Shortcuts => app.shortcuts_panel.open(),
                Self::Palette => app.command_palette.open(),
                Self::Unicode => app.unicode_picker.open(),
                Self::Domains => app.domain_picker.open(&app.config.domains),
                Self::Recording => {
                    app.shortcuts_panel.start_recording(BindableAction::Copy);
                }
            }
        }

        /// The query field this modal pastes into, or `None` when it blocks
        /// pastes.
        fn query(self, app: &mut App) -> Option<String> {
            match self {
                Self::InfoOverlay | Self::Recording => None,
                Self::InfoOverlayInput => {
                    let request_id = app.info_overlay.request_id();
                    app.info_overlay.input_value(request_id).map(str::to_owned)
                }
                Self::Settings => Some(settings_filter(app)),
                Self::Shortcuts => Some(shortcuts_filter(app)),
                Self::Palette => Some(app.command_palette.build_info().input),
                Self::Unicode => Some(app.unicode_picker.build_info().input),
                Self::Domains => Some(app.domain_picker.build_info().input),
            }
        }
    }

    /// For each `higher` modal opened over a field (`open_lower` opens it and
    /// pastes into it, over a retained search and Commands form), a paste goes
    /// to the higher modal's query or is refused, and never reaches the lower
    /// field (`lower_intact` checks it) or anything beneath it.
    fn assert_higher_modals_own_paste(
        open_lower: fn(&mut App),
        lower_intact: fn(&mut App),
        highers: &[Higher],
    ) {
        for &higher in highers {
            let mut app = App::new_headless(Config::default()).unwrap();
            app.search.start();
            app.commands_panel.toggle();
            app.commands_panel.start_add();
            open_lower(&mut app);
            higher.open(&mut app);
            let takes_paste = higher.query(&mut app).is_some();
            assert_eq!(
                app.paste_clipboard_payload("higher"),
                takes_paste,
                "{higher:?}"
            );
            lower_intact(&mut app);
            assert!(app.search.query().is_empty(), "{higher:?}");
            assert!(app.commands_panel.form_name.text().is_empty(), "{higher:?}");
            match higher.query(&mut app) {
                Some(query) => assert_eq!(query, "higher", "{higher:?}"),
                None => assert!(app.clipboard_paste_blocked(), "{higher:?}"),
            }
            app.unicode_picker.close();
            app.unicode_picker.finish_search_for_test();
        }
    }

    fn exercise_tab_rename_paste(app: &mut App) {
        let search_before = app.search.query().to_owned();
        for max_chars in [128, 256] {
            let tab = app.tabs.as_ref().and_then(TabManager::active_tab);
            let tab_id = tab.map_or(usize::MAX, |tab| tab.id);
            let original_title = tab.and_then(|tab| tab.user_title().map(str::to_owned));
            app.tab_rename = Some(TabRenameState::new(
                tab_id,
                original_title.clone(),
                "Roots",
                max_chars,
            ));
            assert_eq!(app.text_target(), TextTarget::TabRename);
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
                assert_eq!(app.tab_rename.as_ref().unwrap().input.text(), "Roots");
                assert!(app.ime.preedit().is_some());
                if let Some(tab) = app.tabs.as_ref().and_then(TabManager::active_tab) {
                    assert_eq!(tab.user_title(), original_title.as_deref());
                }
            }
            app.redraw_scheduler.begin_frame();
            assert!(app.paste_clipboard_payload("界e\u{301}👩\u{200d}💻"));
            assert!(app.ime.preedit().is_none());
            assert_eq!(app.redraw_scheduler.begin_frame(), "keyboard-input");
            let draft = app.tab_rename.as_ref().unwrap().input.text().to_owned();
            assert_eq!(draft, "Roots界e\u{301}👩\u{200d}💻");
            assert!(
                app.paste_clipboard_payload(&"🫚".repeat(max_chars - 1 - draft.chars().count()))
            );
            let almost = app.tab_rename.as_ref().unwrap().input.text().to_owned();
            app.handle_ime_event(winit::event::Ime::Preedit("keep".into(), None));
            assert!(!app.paste_clipboard_payload("e\u{301}"));
            assert_eq!(app.tab_rename.as_ref().unwrap().input.text(), almost);
            assert!(app.ime.preedit().is_some());
            assert!(app.paste_clipboard_payload("🫚"));
            assert!(!app.paste_clipboard_payload("x"));
            assert_eq!(
                app.tab_rename
                    .as_ref()
                    .unwrap()
                    .input
                    .text()
                    .chars()
                    .count(),
                max_chars
            );
            assert_eq!(app.search.query(), search_before);
            assert!(app.pending_live_domain_controls.is_empty()); // no commit/request
            if let Some(tab) = app.tabs.as_ref().and_then(TabManager::active_tab) {
                assert_eq!(
                    tab.user_title(),
                    Some(app.tab_rename.as_ref().unwrap().input.text())
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
        app.tab_rename = Some(TabRenameState::new(usize::MAX, None, "Roots", 256));
        let rebound = assert_paste_binding_follows_registry(&mut app);
        app.commands_panel.toggle();
        assert!(app.clipboard_paste_blocked());
        assert!(!app.handle_query_paste_key(&rebound));
        assert!(!app.paste_clipboard_payload("hidden"));
        app.commands_panel.start_add();
        assert!(app.paste_clipboard_payload("front form"));
        assert_eq!(app.commands_panel.form_name.text(), "front form");
        assert_eq!(app.tab_rename.as_ref().unwrap().input.text(), "Roots");
        app.commands_panel.close();
        app.workspace_menu = Some(crate::ui::popup_menu::PopupMenu::new(
            crate::ui::workspace_menu::workspace_menu_rows(&[], None, false),
            false,
        ));
        assert!(app.clipboard_paste_blocked());
        assert!(!app.paste_clipboard_payload("hidden"));
        app.workspace_menu = None;
        assert_eq!(app.tab_rename.as_ref().unwrap().input.text(), "Roots");
    }

    fn open_workspace_rename(app: &mut App) {
        app.session_panel.set_test_entries(
            vec![crate::ui::session_panel::SessionEntry {
                id: "paste-workspace-fixture".into(),
                display_name: "Roots".into(),
                subtitle: String::new(),
                last_modified: String::new(),
            }],
            true,
        );
        app.apply_session_panel_action(crate::ui::session_panel::SessionPanelAction::Rename(
            "paste-workspace-fixture".into(),
        ));
    }

    fn exercise_workspace_rename_paste(app: &mut App) {
        open_workspace_rename(app);
        assert_eq!(app.text_target(), TextTarget::WorkspaceRename);
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
        let rebound = assert_paste_binding_follows_registry(&mut app);
        app.workspace_menu = Some(crate::ui::popup_menu::PopupMenu::new(
            crate::ui::workspace_menu::workspace_menu_rows(&[], None, false),
            false,
        ));
        assert!(app.clipboard_paste_blocked());
        assert!(!app.handle_query_paste_key(&rebound));
        assert!(!app.paste_clipboard_payload("hidden"));
        app.workspace_menu = None;
        assert_eq!(
            app.session_panel.build_info().rename_text.as_deref(),
            Some("Roots")
        );
        app.apply_session_panel_action(crate::ui::session_panel::SessionPanelAction::CancelRename);
        assert!(app.clipboard_paste_blocked());
        assert!(!app.paste_clipboard_payload("browse"));
        assert!(!app.session_panel.is_renaming());
        assert_eq!(app.session_panel.sessions()[0].display_name, "Roots");
    }

    #[test]
    fn workspace_paste_preserves_lower_fields_and_respects_higher_modal_owners() {
        assert_higher_modals_own_paste(
            |app| {
                open_workspace_rename(app);
                assert!(app.paste_clipboard_payload(" workspace"));
            },
            |app| {
                assert_eq!(
                    app.session_panel.build_info().rename_text.as_deref(),
                    Some("Roots workspace")
                );
                assert_eq!(app.session_panel.sessions()[0].display_name, "Roots");
            },
            &[
                Higher::InfoOverlay,
                Higher::InfoOverlayInput,
                Higher::Settings,
                Higher::Shortcuts,
                Higher::Palette,
                Higher::Unicode,
                Higher::Domains,
            ],
        );
    }

    #[test]
    fn settings_filter_paste_uses_atomic_scalar_limits_without_changing_configuration() {
        let mut app = App::new_headless(Config::default()).unwrap();
        let config_before = toml::to_string(&app.config).unwrap();
        app.settings_panel.toggle();
        assert_eq!(app.text_target(), TextTarget::SettingsFilter);
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
        let rebound = assert_paste_binding_follows_registry(&mut app);
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
    }

    #[test]
    fn settings_paste_retains_lower_fields_and_respects_higher_modal_owners() {
        assert_higher_modals_own_paste(
            |app| {
                app.session_panel
                    .open(crate::session::listing::SessionListing::workspaces());
                app.settings_panel.toggle();
                assert!(app.paste_clipboard_payload("settings only"));
            },
            |app| assert_eq!(settings_filter(app), "settings only"),
            &[
                Higher::InfoOverlay,
                Higher::InfoOverlayInput,
                Higher::Shortcuts,
                Higher::Palette,
                Higher::Unicode,
                Higher::Domains,
            ],
        );
    }

    #[test]
    fn shortcut_filter_paste_is_atomic_at_caret_and_obeys_its_own_scalar_limit() {
        let mut app = App::new_headless(Config::default()).unwrap();
        app.shortcuts_panel.open();
        assert_eq!(app.text_target(), TextTarget::ShortcutsFilter);
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
        let rebound = assert_paste_binding_follows_registry(&mut app);
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
        assert!(app.config.keybindings.is_empty());
    }

    #[test]
    fn shortcut_filter_paste_retains_lower_fields_and_respects_higher_owners() {
        assert_higher_modals_own_paste(
            |app| {
                app.settings_panel.toggle();
                app.shortcuts_panel.open();
                assert!(app.paste_clipboard_payload("filter"));
            },
            |app| assert_eq!(shortcuts_filter(app), "filter"),
            &[
                Higher::InfoOverlay,
                Higher::InfoOverlayInput,
                Higher::Palette,
                Higher::Unicode,
                Higher::Domains,
                Higher::Recording,
            ],
        );
    }

    #[test]
    fn command_form_paste_honors_shortcuts_and_preserves_state_on_rejection() {
        let mut app = App::new_headless(Config::default()).unwrap();
        app.commands_panel.toggle();
        assert!(app.clipboard_paste_blocked());
        app.commands_panel.start_add();
        assert_paste_binding_follows_registry(&mut app);
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
            |app| {
                app.session_panel
                    .open(crate::session::listing::SessionListing::workspaces())
            },
            |app| {
                app.info_overlay.show("Notice", vec![]);
            },
        ] {
            let mut app = App::new_headless(Config::default()).unwrap();
            app.search.start();
            app.commands_panel.toggle();
            app.commands_panel.start_add();
            assert_eq!(app.text_target(), TextTarget::CommandsForm);
            assert!(app.paste_clipboard_payload("form only"));
            assert!(app.search.query().is_empty());
            open(&mut app);
            assert_eq!(app.text_target(), TextTarget::Blocked);
            assert!(!app.paste_clipboard_payload("hidden"));
            assert_eq!(app.commands_panel.form_name.text(), "form only");
            assert!(app.search.query().is_empty());
        }
        let mut app = App::new_headless(Config::default()).unwrap();
        app.commands_panel.toggle();
        app.commands_panel.start_add();
        app.command_palette.open();
        assert_eq!(
            app.text_target(),
            TextTarget::Palette(PaletteModal::Commands)
        );
        assert!(app.paste_clipboard_payload("palette only"));
        assert!(app.commands_panel.form_name.text().is_empty());
    }

    #[test]
    fn query_paste_honors_rebound_and_disabled_shortcuts_without_reading_for_blocked_owners() {
        let mut app = App::new_headless(Config::default()).unwrap();
        app.command_palette.open();
        let rebound = assert_paste_binding_follows_registry(&mut app);
        app.info_overlay.show("Notice", vec![]);
        assert!(!app.handle_query_paste_key(&rebound));
        assert!(app.clipboard_paste_blocked());
        assert!(!app.paste_clipboard_payload("hidden"));
        assert_eq!(app.command_palette.build_info().input, "");
    }

    #[test]
    fn palette_query_paste_is_atomic_at_caret_and_retains_limits() {
        for modal in [
            PaletteModal::Commands,
            PaletteModal::Unicode,
            PaletteModal::Domains,
        ] {
            let mut app = App::new_headless(Config::default()).unwrap();
            match modal {
                PaletteModal::Commands => app.command_palette.open(),
                PaletteModal::Unicode => app.unicode_picker.open(),
                PaletteModal::Domains => app.domain_picker.open(&app.config.domains),
            }
            assert_eq!(app.text_target(), TextTarget::Palette(modal));
            assert!(app.paste_clipboard_payload("az"));
            assert!(app.handle_palette_edit_key(&Key::Named(NamedKey::ArrowLeft)));
            assert!(app.paste_clipboard_payload("界e\u{301}👩\u{200d}💻"));
            let info = |app: &App| app.palette_owner(modal).build_info();
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
            |app| {
                app.session_panel
                    .open(crate::session::listing::SessionListing::workspaces())
            },
        ] {
            let mut app = App::new_headless(Config::default()).unwrap();
            app.search.start();
            open(&mut app);
            assert_eq!(app.text_target(), TextTarget::Blocked);
            assert!(!app.paste_clipboard_payload("hidden"));
            assert!(app.search.query().is_empty());
        }
        app.command_palette.open();
        assert!(app.paste_clipboard_payload("background"));
        app.unicode_picker.open();
        assert_eq!(
            app.text_target(),
            TextTarget::Palette(PaletteModal::Unicode)
        );
        app.domain_picker.open(&app.config.domains);
        assert_eq!(
            app.text_target(),
            TextTarget::Palette(PaletteModal::Domains)
        );
        assert!(app.domain_picker.activate().is_some());
        assert!(app.domain_picker.is_busy());
        // A busy picker's query is read-only, so it takes no text at all.
        assert_eq!(app.text_target(), TextTarget::Blocked);
        assert!(!app.paste_clipboard_payload("local"));
        assert_eq!(app.command_palette.build_info().input, "background");
        assert!(app.unicode_picker.build_info().input.is_empty());
        assert!(app.search.query().is_empty());
    }

    #[test]
    fn search_clipboard_paste_is_bounded_and_query_pastes_never_reach_raw_pty() {
        let mut app = App::new_headless_running(
            "stty raw -echo; printf READY; dd bs=1 count=1 2>/dev/null | od -An -tx1; sleep 0.1",
            "query-paste",
        );
        app.wait_for_screen_text("READY");
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
        app.apply_session_panel_action(crate::ui::session_panel::SessionPanelAction::CancelRename);
        assert!(!app.paste_clipboard_payload("browse must not write"));
        app.apply_session_panel_action(crate::ui::session_panel::SessionPanelAction::Close);
        exercise_tab_rename_paste(&mut app);
        app.search.cancel();
        assert!(app.paste_clipboard_payload("Z"));
        app.wait_for_screen_text("5a");
    }
}
