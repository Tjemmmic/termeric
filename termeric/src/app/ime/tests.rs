use super::*;

/// Where `field` draws its caret, when it is where text goes.
fn ime_viewport_of(
    app: &App,
    field: TextTarget,
    width: f32,
    height: f32,
    cell_width: f32,
    cell_height: f32,
) -> Option<crate::ui::text_input_viewport::TextInputViewport> {
    (app.text_target() == field)
        .then(|| app.field_ime_viewport(field, width, height, cell_width, cell_height))
        .flatten()
}

#[test]
fn an_open_workspaces_menu_swallows_ime_text_like_every_key() {
    let mut app = App::new_headless(Config::default()).unwrap();
    app.search.start();
    assert_ne!(app.text_target(), TextTarget::Blocked);
    app.workspace_menu = Some(crate::ui::popup_menu::PopupMenu::new(
        crate::ui::workspace_menu::workspace_menu_rows(&[], None, false),
        false,
    ));
    assert_eq!(app.text_target(), TextTarget::Blocked);
    assert_ne!(app.text_target(), TextTarget::Search);
    app.handle_ime_event(Ime::Preedit("日本".into(), None));
    assert!(app.ime.preedit().is_none());
    app.handle_ime_event(Ime::Commit("日本".into()));
    assert_eq!(app.search.query(), "");
}

#[test]
fn a_guardrail_over_an_info_overlay_field_swallows_ime_text() {
    let mut app = App::new_headless(Config::default()).unwrap();
    let request_id =
        app.info_overlay
            .show_input("Name", vec!["Name:".into()], 0, "Name", "", 64, Vec::new());
    assert_ne!(app.text_target(), TextTarget::Blocked);
    app.guardrail.show(
        crate::terminal::guardrails::GuardrailMatch {
            command: "rm -rf /".into(),
            reason: "fixture".into(),
            severity: crate::terminal::guardrails::Severity::Critical,
        },
        "GUI tab 1, pane 0".into(),
        vec![crate::terminal::guardrails::GuardedWrite::for_pane(
            1,
            crate::panes::PaneId(0),
            b"x".to_vec(),
        )],
    );
    assert_eq!(app.text_target(), TextTarget::Blocked);
    app.handle_ime_event(Ime::Commit("hidden".into()));
    assert_eq!(app.info_overlay.input_value(request_id), Some(""));
}

#[test]
fn settings_ime_matches_scrolled_filter_and_refuses_dropdown_or_higher_owners() {
    let mut app = App::new_headless(Config::default()).unwrap();
    app.search.start();
    app.commands_panel.toggle();
    app.commands_panel.start_add();
    app.settings_panel.set_reduced_motion(true);
    app.settings_panel.toggle();
    assert!(
        app.settings_panel
            .insert_filter_text(&"界e\u{301}👩\u{200d}💻".repeat(25))
    );
    for (width, cell) in [(800.0, 8.0), (1600.0, 16.0)] {
        let info = app
            .settings_panel
            .build_info(&app.config, &app.shortcut_registry);
        let layout = crate::ui::settings::SettingsModalLayout::calculate(
            width,
            800.0,
            cell,
            cell * 2.0,
            info.animation,
            info.entries.len(),
        );
        let view = ime_viewport_of(
            &app,
            TextTarget::SettingsFilter,
            width,
            800.0,
            cell,
            cell * 2.0,
        )
        .unwrap();
        assert_eq!(
            Some(view.clone()),
            layout.input_viewport(&info.filter, info.filter_cursor, cell * 2.0)
        );
        assert!(view.text_range.start > 0);
        assert_ne!(app.text_target(), TextTarget::CommandsForm);
        assert_ne!(app.text_target(), TextTarget::Search);
    }
    assert!(ime_viewport_of(&app, TextTarget::SettingsFilter, 40.0, 800.0, 8.0, 16.0).is_none());
    app.command_palette.open();
    assert_ne!(app.text_target(), TextTarget::SettingsFilter);
    app.command_palette.close();
    app.shortcuts_panel.open();
    assert_ne!(app.text_target(), TextTarget::SettingsFilter);
    app.shortcuts_panel.close_immediately();
    app.settings_panel.close();
    app.settings_panel
        .open_to_section(crate::ui::settings::SettingsSection::Theme);
    app.settings_panel
        .build_info(&app.config, &app.shortcut_registry);
    app.settings_panel.handle_key(
        crate::ui::settings::SettingsKey::Enter,
        &app.config,
        &app.shortcut_registry,
    );
    assert!(app.settings_panel.is_dropdown_open());
    assert_eq!(app.text_target(), TextTarget::Blocked);
    assert!(ime_viewport_of(&app, TextTarget::SettingsFilter, 800.0, 800.0, 8.0, 16.0).is_none());
    app.handle_ime_event(Ime::Preedit("draft".into(), None));
    assert!(app.ime.preedit().is_none());
    app.settings_panel.close();
    assert_ne!(app.text_target(), TextTarget::SettingsFilter);
}

#[test]
fn shortcut_ime_uses_scrolled_filter_geometry_and_recording_blocks_retained_settings() {
    let mut app = App::new_headless(Config::default()).unwrap();
    app.search.start();
    app.commands_panel.toggle();
    app.commands_panel.start_add();
    app.settings_panel.toggle();
    app.shortcuts_panel.set_reduced_motion(true);
    app.shortcuts_panel.open();
    assert!(
        app.shortcuts_panel
            .insert_filter_text(&"界e\u{301}👩\u{200d}💻".repeat(20))
    );
    for (width, cell) in [(800.0, 8.0), (1600.0, 16.0)] {
        let info = app.shortcuts_panel.build_info(&app.shortcut_registry);
        let layout = crate::ui::list_dialog::ListDialogLayout::calculate(
            crate::ui::shortcuts_panel::SHORTCUTS_DIALOG,
            width,
            800.0,
            cell,
            cell * 2.0,
            info.animation,
            info.entries.len(),
        );
        let view = ime_viewport_of(
            &app,
            TextTarget::ShortcutsFilter,
            width,
            800.0,
            cell,
            cell * 2.0,
        )
        .unwrap();
        assert_eq!(
            view,
            layout.input_viewport(&info.filter, info.cursor).unwrap()
        );
        assert!(view.text_range.start > 0);
        assert!(view.cursor_x + cell <= view.right + 0.001);
        assert_ne!(app.text_target(), TextTarget::Search);
        assert_ne!(app.text_target(), TextTarget::CommandsForm);
    }
    assert!(ime_viewport_of(&app, TextTarget::ShortcutsFilter, 40.0, 800.0, 8.0, 16.0).is_none());
    app.command_palette.open();
    assert!(ime_viewport_of(&app, TextTarget::ShortcutsFilter, 800.0, 800.0, 8.0, 16.0).is_none());
    app.command_palette.close();
    app.shortcuts_panel
        .start_recording(crate::input::keybindings::BindableAction::Copy);
    assert_eq!(app.text_target(), TextTarget::Blocked);
    assert_ne!(app.text_target(), TextTarget::ShortcutsFilter);
    app.handle_ime_event(Ime::Preedit("draft".into(), None));
    assert!(app.ime.preedit().is_none());
    app.shortcuts_panel.close();
    assert!(ime_viewport_of(&app, TextTarget::ShortcutsFilter, 800.0, 800.0, 8.0, 16.0).is_none());
}

#[test]
fn command_form_ime_follows_the_active_scrolled_field_and_respects_modal_priority() {
    use crate::ui::commands::panel::{CommandsPanelLayout, EditMode};
    let mut app = App::new_headless(Config::default()).unwrap();
    app.commands_panel.set_reduced_motion(true);
    app.commands_panel.toggle();
    app.commands_panel.start_add();
    // Fifteen repeats fill the 256-byte name field to one byte short.
    let text = "界e\u{301}👩\u{200d}💻".repeat(15);
    assert!(app.commands_panel.form_name.replace(&text));
    assert!(app.commands_panel.form_command.replace(&text));
    app.search.start(); // retained lower-priority query must not win
    for field in [EditMode::EditingName, EditMode::EditingCommand] {
        app.commands_panel.edit_mode = field.clone();
        for (width, cell) in [(800.0, 8.0), (1600.0, 16.0)] {
            let view = ime_viewport_of(
                &app,
                TextTarget::CommandsForm,
                width,
                800.0,
                cell,
                cell * 2.0,
            )
            .unwrap();
            let info = app.commands_panel.build_info(&[]);
            let layout = CommandsPanelLayout::calculate(
                width,
                800.0,
                0.0,
                cell,
                cell * 2.0,
                info.animation,
                true,
                0,
            );
            assert_eq!(view, layout.input_viewport(&info, field.clone()).unwrap());
            assert!(view.text_range.start > 0);
            assert!(view.cursor_x + cell <= view.right + 0.001);
            assert_ne!(app.text_target(), TextTarget::Search);
        }
    }
    assert!(ime_viewport_of(&app, TextTarget::CommandsForm, 160.0, 800.0, 8.0, 16.0).is_none());
    app.command_palette.open();
    assert_ne!(app.text_target(), TextTarget::CommandsForm);
    assert!(ime_viewport_of(&app, TextTarget::CommandsForm, 800.0, 800.0, 8.0, 16.0).is_none());
    app.command_palette.close();
    app.info_overlay.show("Notice", vec![]);
    assert_ne!(app.text_target(), TextTarget::CommandsForm);
    app.commands_panel.close();
    assert!(ime_viewport_of(&app, TextTarget::CommandsForm, 800.0, 800.0, 8.0, 16.0).is_none());
}

#[test]
fn search_ime_caret_uses_query_columns_and_shared_shifted_scaled_layout() {
    use crate::ui::search_bar::SearchBarLayout;
    let mut app = App::new_headless(Config::default()).unwrap();
    let terminal = termeric_core::Terminal::new(80, 24);
    app.search.start();
    assert_eq!(app.text_target(), TextTarget::Search);
    for query in [
        "界e\u{301}👩\u{200d}💻".to_string(),
        "界e\u{301}👩\u{200d}💻".repeat(80),
    ] {
        assert!(app.search.set_query(&query, terminal.grid()));
        for (left, width, top, cell) in [
            (0.0, 800.0, 30.0, 8.0),
            (160.0, 640.0, 54.0, 10.0),
            (240.0, 1100.0, 80.0, 16.0),
        ] {
            for cursor in [0, "界".len(), query.len()] {
                app.search_cursor_pos = cursor;
                let info = app.search.build_info(cursor);
                let layout = SearchBarLayout::calculate(&info, left, width, top, cell, cell * 2.0);
                let view = layout.input_viewport(&info, cell).unwrap();
                assert_eq!(view.text_y, layout.text_y);
                assert!(layout.bar.y > top, "the box floats inside the content");
                assert!(view.cursor_x + cell <= layout.input.x + layout.input.width + 0.001);
                assert!(view.text_range.end <= query.len());
                if cursor == query.len() {
                    assert_eq!(view.text_range.end, query.len());
                }
                if cursor == "界".len() {
                    assert_eq!(view.cursor_x, view.text_x + 2.0 * cell);
                }
                if query.chars().count() == 6 && cursor == query.len() {
                    assert_eq!(view.cursor_x, view.text_x + 5.0 * cell);
                }
            }
        }
    }
    let info = app.search.build_info(app.search_cursor_pos);
    let collapsed = SearchBarLayout::calculate(&info, 0.0, 100.0, 30.0, 10.0, 20.0);
    assert!(collapsed.input_viewport(&info, 10.0).is_none());
}

#[test]
fn retained_search_does_not_steal_ime_geometry_from_modal_or_text_owners() {
    let openers: [fn(&mut App); 9] = [
        |app| app.command_palette.open(),
        |app| app.unicode_picker.open(),
        |app| app.settings_panel.toggle(),
        |app| app.shortcuts_panel.open(),
        |app| app.commands_panel.toggle(),
        |app| {
            app.workspace_panel
                .open(crate::session::listing::WorkspaceListing::workspaces())
        },
        |app| {
            app.info_overlay.show("Notice", vec![]);
        },
        |app| app.domain_picker.open(&app.config.domains),
        |app| app.tab_rename = Some(TabRenameState::new(0, "", 80)),
    ];
    for open in openers {
        let mut app = App::new_headless(Config::default()).unwrap();
        app.search.start();
        assert_eq!(app.text_target(), TextTarget::Search);
        open(&mut app);
        assert_ne!(app.text_target(), TextTarget::Search);
    }
    let mut app = App::new_headless(Config::default()).unwrap();
    app.search.start();
    app.search.confirm();
    assert_ne!(app.text_target(), TextTarget::Search);
}

#[test]
fn search_ime_commit_inserts_at_query_caret_without_leaking_into_pty() {
    let mut app = App::new_headless_running(crate::test_support::FIRST_BYTE_PROBE, "search-ime");
    app.wait_for_screen_text("READY");
    app.search.start();
    assert!(
        app.with_search_and_active_grid(|search, grid| search.set_query("a界b", grid))
            .unwrap()
    );
    app.search_cursor_pos = 1;
    app.handle_ime_event(Ime::Preedit("日本".into(), None));
    assert_eq!(app.search.query(), "a界b");
    app.handle_ime_event(Ime::Commit("日本".into()));
    assert_eq!(app.search.query(), "a日本界b");
    assert_eq!(app.search_cursor_pos, "a日本".len());
    app.search.cancel();
    app.assert_pty_untouched();
}

#[test]
fn workspace_rename_ime_matches_paint_geometry_without_a_terminal() {
    let mut app = App::new_headless(Config::default()).unwrap();
    app.workspace_panel.set_reduced_motion(true);
    app.workspace_panel.set_test_entries(
        vec![crate::ui::workspace_panel::WorkspaceEntry {
            id: "workspace-test".into(),
            display_name: "Roots".into(),
            subtitle: String::new(),
            last_modified: String::new(),
        }],
        true,
    );
    app.workspace_panel.begin_rename("workspace-test");
    let name = "界e\u{301}👩\u{200d}💻".repeat(16);
    assert!(app.workspace_panel.accessibility_set_rename(&name));
    for (width, cell) in [(800.0, 8.0), (1400.0, 16.0)] {
        let info = app.workspace_panel.build_info();
        let layout = crate::ui::workspace_panel::WorkspacePanelLayout::calculate(
            width,
            800.0,
            0.0,
            cell,
            cell * 2.0,
            info.animation,
            true,
            true,
            info.entries.len(),
        );
        let expected = layout.rename_input_viewport(&name, name.len()).unwrap();
        assert_eq!(
            ime_viewport_of(
                &app,
                TextTarget::WorkspaceRename,
                width,
                800.0,
                cell,
                cell * 2.0
            ),
            Some(expected.clone())
        );
        assert!(expected.text_range.start > 0);
    }
    assert!(ime_viewport_of(&app, TextTarget::WorkspaceRename, 40.0, 600.0, 10.0, 20.0).is_none());
    app.search.start();
    assert_ne!(app.text_target(), TextTarget::Search);
    app.command_palette.open();
    assert!(ime_viewport_of(&app, TextTarget::WorkspaceRename, 800.0, 600.0, 10.0, 20.0).is_none());
    app.command_palette.close();
    app.domain_picker.open(&app.config.domains);
    assert!(ime_viewport_of(&app, TextTarget::WorkspaceRename, 800.0, 600.0, 10.0, 20.0).is_none());
    app.close_domain_picker();
    app.settings_panel.toggle();
    assert!(ime_viewport_of(&app, TextTarget::WorkspaceRename, 800.0, 600.0, 10.0, 20.0).is_none());
    app.settings_panel.toggle();
    app.workspace_panel.cancel_rename();
    assert!(ime_viewport_of(&app, TextTarget::WorkspaceRename, 800.0, 600.0, 10.0, 20.0).is_none());
    assert!(app.active_terminal().is_none());
    assert_eq!(app.workspace_panel.entries()[0].display_name, "Roots");
}

#[test]
fn domain_picker_ime_owns_editable_stages_and_blocks_busy_background_input() {
    use crate::ui::domain_picker::DomainPickerCommand;
    let mut app = App::new_headless(Config::default()).unwrap();
    app.command_palette.open();
    app.command_palette.accessibility_set_input("background");
    app.domain_picker.open(&app.config.domains);
    app.search.start();
    app.settings_panel.toggle();
    assert_ne!(app.text_target(), TextTarget::Search);
    assert_ne!(app.text_target(), TextTarget::SettingsFilter);
    app.handle_ime_event(Ime::Preedit("日本".into(), None));
    assert!(app.ime.preedit().is_some());
    app.handle_ime_event(Ime::Commit("a界b".into()));
    app.handle_palette_edit_key(&winit::keyboard::Key::Named(
        winit::keyboard::NamedKey::ArrowLeft,
    ));
    app.handle_ime_event(Ime::Commit("e\u{301}".into()));
    assert_eq!(app.domain_picker.build_info().input, "a界e\u{301}b");
    for text in ["bad\nquery".into(), "x".repeat(257)] {
        app.handle_ime_event(Ime::Commit(text));
        assert_eq!(app.domain_picker.build_info().input, "a界e\u{301}b");
    }
    app.domain_picker.set_query("");
    let Some(DomainPickerCommand::Discover { generation, .. }) = app.domain_picker.activate()
    else {
        panic!("expected discovery");
    };
    let before = app.domain_picker.build_info().input;
    app.handle_ime_event(Ime::Preedit("blocked".into(), None));
    app.handle_ime_event(Ime::Commit("blocked".into()));
    assert!(app.ime.preedit().is_none());
    assert_eq!(app.domain_picker.build_info().input, before);
    assert_eq!(app.text_target(), TextTarget::Blocked);
    app.handle_domain_picker_loaded(
        generation,
        Ok(vec![crate::domain::model::DomainSummary {
            id: "domain-test".into(),
            name: "日本".into(),
            tabs: 1,
            panes: 1,
            attachments: 0,
            created_at: "2026-01-01T00:00:00Z".into(),
        }]),
    );
    app.handle_ime_event(Ime::Preedit("日".into(), None));
    let preedit = app.ime.preedit().cloned();
    assert!(preedit.is_some());
    app.handle_domain_picker_loaded(generation.wrapping_sub(1), Err("stale".into()));
    assert_eq!(app.ime.preedit(), preedit.as_ref());
    app.handle_ime_event(Ime::Commit("日本".into()));
    assert_eq!(app.domain_picker.build_info().input, "日本");
    let Some(DomainPickerCommand::PrepareWindow { generation, .. }) = app.domain_picker.activate()
    else {
        panic!("expected preparation");
    };
    app.handle_ime_event(Ime::Preedit("blocked".into(), None));
    app.handle_ime_event(Ime::Commit("blocked".into()));
    assert!(app.ime.preedit().is_none());
    assert_eq!(app.text_target(), TextTarget::Blocked);
    app.handle_domain_picker_prepared(generation, Err("test failure".into()));
    assert_eq!(
        app.text_target(),
        TextTarget::Blocked,
        "error stage is not a filter"
    );
    app.domain_picker.back_or_close();
    assert_ne!(app.text_target(), TextTarget::Blocked);
    assert_eq!(app.command_palette.build_info().input, "background");
    assert!(app.requested_window_launch.is_none());
}

#[test]
fn domain_picker_ime_geometry_uses_its_query_even_over_retained_panels() {
    let mut app = App::new_headless(Config::default()).unwrap();
    app.command_palette.open();
    app.command_palette.accessibility_set_input("background");
    app.domain_picker.palette_mut().set_reduced_motion(true);
    app.domain_picker.open(&app.config.domains);
    let query = "界e\u{301}👩\u{200d}💻".repeat(30);
    assert!(app.domain_picker.set_query(&query));
    for (width, cell_width) in [(450.0, 8.0), (800.0, 10.0), (1400.0, 20.0)] {
        let info = app.domain_picker.build_info();
        let expected = crate::ui::list_dialog::ListDialogLayout::calculate(
            crate::ui::command_palette::PALETTE_DIALOG,
            width,
            900.0,
            cell_width,
            24.0,
            info.animation,
            info.entries.len(),
        )
        .input_viewport(&info.input, info.cursor)
        .unwrap();
        assert_eq!(
            ime_viewport_of(
                &app,
                TextTarget::Palette(PaletteModal::Domains),
                width,
                900.0,
                cell_width,
                24.0
            ),
            Some(expected.clone())
        );
        assert!(expected.text_range.start > 0);
        assert!(expected.cursor_x + cell_width <= expected.right + 0.001);
    }
}

#[test]
fn palette_ime_geometry_tracks_the_visible_query_without_a_terminal() {
    for unicode in [false, true] {
        let target = TextTarget::Palette(if unicode {
            PaletteModal::Unicode
        } else {
            PaletteModal::Commands
        });
        let mut app = App::new_headless(Config::default()).unwrap();
        app.command_palette.set_reduced_motion(true);
        app.unicode_picker.palette_mut().set_reduced_motion(true);
        let query = "界e\u{301}👩\u{200d}💻".repeat(30);
        if unicode {
            app.unicode_picker.open();
            app.unicode_picker.set_query(&query);
        } else {
            app.command_palette.open();
            app.command_palette.accessibility_set_input(&query);
        }
        for (width, cell_width) in [(450.0, 8.0), (800.0, 10.0), (1400.0, 20.0)] {
            let view = ime_viewport_of(&app, target, width, 900.0, cell_width, 24.0).unwrap();
            let info = if unicode {
                app.unicode_picker.build_info()
            } else {
                app.command_palette.build_info()
            };
            let expected = crate::ui::list_dialog::ListDialogLayout::calculate(
                crate::ui::command_palette::PALETTE_DIALOG,
                width,
                900.0,
                cell_width,
                24.0,
                info.animation,
                info.entries.len(),
            )
            .input_viewport(&info.input, info.cursor)
            .unwrap();
            assert_eq!(view, expected);
            assert!(view.text_range.start > 0);
            assert_eq!(view.text_range.end, query.len());
            assert!(view.cursor_x + cell_width <= view.right + 0.001);
        }
        app.command_palette.close();
        app.unicode_picker.close();
        assert!(ime_viewport_of(&app, target, 800.0, 600.0, 10.0, 20.0).is_none());
        app.unicode_picker.finish_search_for_test();
    }
}

#[test]
fn palette_ime_commits_are_atomic_at_query_limits_and_reject_multiline_text() {
    for unicode in [false, true] {
        let mut app = App::new_headless(Config::default()).unwrap();
        if unicode {
            app.unicode_picker.open();
        } else {
            app.command_palette.open();
        }
        app.handle_ime_event(Ime::Commit("界".repeat(300)));
        let info = if unicode {
            app.unicode_picker.build_info()
        } else {
            app.command_palette.build_info()
        };
        assert_eq!(info.input, "");
        assert_eq!(info.cursor, info.input.len());
        assert!(app.ime.preedit().is_none());
        let prefix = "界".repeat(255);
        app.handle_ime_event(Ime::Commit(prefix.clone()));
        app.handle_palette_edit_key(&winit::keyboard::Key::Named(
            winit::keyboard::NamedKey::Home,
        ));
        for text in ["xy", "a\nb", "\u{2028}", "\u{2029}", "\t", "\x1b"] {
            app.handle_ime_event(Ime::Commit(text.into()));
            let info = if unicode {
                app.unicode_picker.build_info()
            } else {
                app.command_palette.build_info()
            };
            assert_eq!(info.input, prefix, "{text:?}");
            assert_eq!(info.cursor, 0);
        }
        app.handle_ime_event(Ime::Commit("🫚".into()));
        let info = if unicode {
            app.unicode_picker.build_info()
        } else {
            app.command_palette.build_info()
        };
        assert_eq!(info.input, format!("🫚{prefix}"));
        assert_eq!(info.cursor, "🫚".len());
        app.unicode_picker.close();
        app.unicode_picker.finish_search_for_test();
    }
}

#[test]
fn preedit_cursor_must_be_a_valid_utf8_range() {
    let mut state = ImeState::default();
    state.set_preedit("日本".into(), Some((0, 3)));
    assert_eq!(state.preedit().unwrap().cursor, Some((0, 3)));
    state.set_preedit("日本".into(), Some((1, 3)));
    assert_eq!(state.preedit().unwrap().cursor, None);
}

fn exercise_retained_rename_modal_ownership(app: &mut App) {
    use crate::ui::workspace_panel::{WorkspaceEntry, WorkspacePanelAction};
    use winit::keyboard::{Key, NamedKey};
    for owner in ["settings", "shortcuts", "session", "commands"] {
        app.tab_rename = Some(TabRenameState::new(
            app.tabs
                .as_ref()
                .and_then(TabManager::active_tab_id)
                .unwrap_or(usize::MAX),
            "retained",
            256,
        ));
        app.search.start();
        match owner {
            "settings" => {
                app.settings_panel.set_reduced_motion(true);
                app.settings_panel.toggle();
            }
            "shortcuts" => {
                app.shortcuts_panel.set_reduced_motion(true);
                app.shortcuts_panel.open();
            }
            "session" => {
                app.workspace_panel.set_reduced_motion(true);
                app.workspace_panel.set_test_entries(
                    vec![WorkspaceEntry {
                        id: "modal-owner-fixture".into(),
                        display_name: "Roots".into(),
                        subtitle: String::new(),
                        last_modified: String::new(),
                    }],
                    true,
                );
                app.apply_workspace_panel_action(WorkspacePanelAction::Rename(
                    "modal-owner-fixture".into(),
                ));
                assert!(app.workspace_panel.accessibility_set_rename(""));
            }
            _ => {
                app.commands_panel.set_reduced_motion(true);
                app.commands_panel.toggle();
                app.commands_panel.start_add();
            }
        }
        let key = Key::Character("K".into());
        assert!(match owner {
            "settings" => app.handle_settings_filter_key(&key),
            "shortcuts" => app.handle_shortcuts_filter_key(&key),
            "session" => app.handle_workspace_rename_key(&key),
            _ => app.handle_commands_panel_key(&key),
        });
        app.handle_ime_event(Ime::Preedit("draft".into(), None));
        assert!(app.ime.preedit().is_some());
        let field = match owner {
            "settings" => TextTarget::SettingsFilter,
            "shortcuts" => TextTarget::ShortcutsFilter,
            "session" => TextTarget::WorkspaceRename,
            _ => TextTarget::CommandsForm,
        };
        let view = ime_viewport_of(app, field, 1000.0, 800.0, 8.0, 16.0);
        assert!(view.is_some(), "missing {owner} candidate viewport");
        app.handle_ime_event(Ime::Commit("I".into()));
        assert!(app.paste_clipboard_payload("P"));
        let text = match owner {
            "settings" => {
                app.settings_panel
                    .build_info(&app.config, &app.shortcut_registry)
                    .filter
            }
            "shortcuts" => {
                app.shortcuts_panel
                    .build_info(&app.shortcut_registry)
                    .filter
            }
            "session" => app.workspace_panel.build_info().rename_text.unwrap(),
            _ => app.commands_panel.form_name.text().to_owned(),
        };
        assert_eq!(text, "KIP", "wrong {owner} input destination");
        assert_eq!(app.tab_rename.as_ref().unwrap().input.text(), "retained");
        assert!(app.search.query().is_empty());
        // A frontmost read-only state must consume input, not expose
        // the retained rename behind it as an IME or paste destination.
        match owner {
            "settings" => {
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
            "shortcuts" => app
                .shortcuts_panel
                .start_recording(crate::input::keybindings::BindableAction::Copy),
            "session" => app.apply_workspace_panel_action(WorkspacePanelAction::CancelRename),
            _ => {
                app.commands_panel.cancel_edit();
                assert!(app.handle_commands_panel_key(&Key::Character("blocked".into())));
                assert!(app.handle_commands_panel_key(&Key::Named(NamedKey::Backspace)));
            }
        }
        assert_eq!(app.text_target(), TextTarget::Blocked);
        app.handle_ime_event(Ime::Preedit("blocked".into(), None));
        assert!(app.ime.preedit().is_none());
        app.handle_ime_event(Ime::Commit("blocked".into()));
        assert!(!app.paste_clipboard_payload("blocked"));
        assert_eq!(app.tab_rename.as_ref().unwrap().input.text(), "retained");
        assert!(app.search.query().is_empty());
        match owner {
            "settings" => app.settings_panel.close(),
            "shortcuts" => app.shortcuts_panel.close_immediately(),
            "session" => app.apply_workspace_panel_action(WorkspacePanelAction::Close),
            _ => {
                assert!(app.handle_commands_panel_key(&Key::Named(NamedKey::Escape)));
            }
        }
        app.handle_ime_event(Ime::Commit(" resumed".into()));
        assert_eq!(
            app.tab_rename.as_ref().unwrap().input.text(),
            "retained resumed"
        );
        assert!(app.paste_clipboard_payload(" pasted"));
        assert_eq!(
            app.tab_rename.as_ref().unwrap().input.text(),
            "retained resumed pasted"
        );
        app.tab_rename = None;
        app.search.cancel();
    }
    assert!(app.config.commands.is_empty());
}

#[test]
fn frontmost_panels_own_keyboard_ime_geometry_and_paste_ahead_of_retained_tab_rename() {
    let mut app = App::new_headless(Config::default()).unwrap();
    exercise_retained_rename_modal_ownership(&mut app);
}

#[test]
fn modal_atomic_commits_never_fall_through_to_a_live_pty() {
    let mut app = App::new_headless_running(crate::test_support::FIRST_BYTE_PROBE, "palette-ime");
    app.wait_for_screen_text("READY");
    for kind in ["command", "unicode", "domain"] {
        match kind {
            "command" => app.command_palette.open(),
            "unicode" => app.unicode_picker.open(),
            _ => app.open_domain_picker(),
        }
        app.handle_ime_event(Ime::Preedit("draft".into(), None));
        app.handle_ime_event(Ime::Commit("x".repeat(257)));
        app.handle_ime_event(Ime::Commit("bad\ncommit".into()));
        app.handle_ime_event(Ime::Commit("e\u{301}👩\u{200d}💻".into()));
        let info = match kind {
            "command" => app.command_palette.build_info(),
            "unicode" => app.unicode_picker.build_info(),
            _ => app.domain_picker.build_info(),
        };
        assert_eq!(info.input, "e\u{301}👩\u{200d}💻");
        assert!(app.ime.preedit().is_none());
        app.close_domain_picker();
        app.command_palette.close();
        app.unicode_picker.close();
        app.unicode_picker.finish_search_for_test();
    }
    app.workspace_panel.set_test_entries(
        vec![crate::ui::workspace_panel::WorkspaceEntry {
            id: "workspace-fixture".into(),
            display_name: "Roots".into(),
            subtitle: String::new(),
            last_modified: String::new(),
        }],
        true,
    );
    app.apply_workspace_panel_action(crate::ui::workspace_panel::WorkspacePanelAction::Rename(
        "workspace-fixture".into(),
    ));
    assert!(
        app.workspace_panel
            .accessibility_set_rename(&"界".repeat(127))
    );
    app.handle_ime_event(Ime::Commit("xy".into()));
    app.handle_ime_event(Ime::Commit("bad\ncommit".into()));
    assert_eq!(
        app.workspace_panel.build_info().rename_text,
        Some("界".repeat(127))
    );
    app.handle_ime_event(Ime::Commit("🫚".into()));
    assert_eq!(
        app.workspace_panel.build_info().rename_text,
        Some(format!("{}🫚", "界".repeat(127)))
    );
    use winit::keyboard::{Key, NamedKey};
    assert!(
        app.workspace_panel
            .accessibility_set_rename("a界e\u{301}👩\u{200d}💻z")
    );
    for key in [
        NamedKey::Home,
        NamedKey::ArrowRight,
        NamedKey::Delete,
        NamedKey::ArrowRight,
    ] {
        assert!(app.handle_workspace_rename_key(&Key::Named(key)));
    }
    assert!(app.handle_workspace_rename_key(&Key::Character("Q".into())));
    assert_eq!(
        app.workspace_panel.build_info().rename_text.as_deref(),
        Some("ae\u{301}Q👩\u{200d}💻z")
    );
    app.modifiers = ModifiersState::CONTROL;
    assert!(app.handle_workspace_rename_key(&Key::Named(NamedKey::Delete)));
    assert_eq!(
        app.workspace_panel.build_info().rename_text.as_deref(),
        Some("ae\u{301}Qz")
    );
    let info = app.workspace_panel.build_info();
    let layout = crate::ui::workspace_panel::WorkspacePanelLayout::calculate(
        800.0, 600.0, 0.0, 8.0, 16.0, 1.0, true, true, 1,
    );
    let text = info.rename_text.as_deref().unwrap();
    let view = layout
        .rename_input_viewport(text, info.rename_cursor)
        .unwrap();
    app.handle_ime_event(Ime::Preedit("draft".into(), None));
    let action = layout
        .rename_input_click(text, info.rename_cursor, view.text_x + 8.0, view.text_y)
        .unwrap();
    app.apply_workspace_panel_action(action);
    assert!(app.ime.preedit().is_none());
    assert_eq!(app.workspace_panel.build_info().rename_cursor, 1);
    app.handle_ime_event(Ime::Commit("R".into()));
    assert_eq!(
        app.workspace_panel.build_info().rename_text.as_deref(),
        Some("aRe\u{301}Qz")
    );
    app.modifiers = ModifiersState::empty();
    assert!(app.handle_workspace_rename_key(&Key::Named(NamedKey::Escape)));
    assert_eq!(app.workspace_panel.entries()[0].display_name, "Roots");
    app.apply_workspace_panel_action(crate::ui::workspace_panel::WorkspacePanelAction::Close);
    exercise_retained_rename_modal_ownership(&mut app);
    app.tabs
        .as_mut()
        .unwrap()
        .active_tab_mut()
        .unwrap()
        .set_user_title(Some("Roots".into()));
    app.begin_tab_rename();
    // The draft takes every edit; the tab keeps its title until Enter.
    let draft = |app: &App| app.tab_rename.as_ref().unwrap().input.text().to_owned();
    let saved = |app: &App| {
        app.tabs
            .as_ref()
            .unwrap()
            .active_tab()
            .unwrap()
            .user_title()
            .map(str::to_owned)
    };
    assert!(app.handle_tab_rename_key(&Key::Character("e\u{301}👩\u{200d}💻".into())));
    assert_eq!(draft(&app), "Rootse\u{301}👩\u{200d}💻");
    assert!(app.handle_tab_rename_key(&Key::Named(NamedKey::Backspace)));
    assert_eq!(draft(&app), "Rootse\u{301}");
    app.handle_ime_event(Ime::Commit("bad\ncommit".into()));
    assert_eq!(draft(&app), "Rootse\u{301}");
    assert_eq!(saved(&app).as_deref(), Some("Roots"));
    assert!(app.handle_tab_rename_key(&Key::Named(NamedKey::Escape)));
    assert_eq!(saved(&app).as_deref(), Some("Roots"));
    app.begin_tab_rename();
    app.handle_ime_event(Ime::Commit(" 🫚".into()));
    assert!(app.handle_tab_rename_key(&Key::Named(NamedKey::Enter)));
    assert!(app.tab_rename.is_none());
    assert_eq!(saved(&app).as_deref(), Some("Roots 🫚"));
    // A rename whose tab has gone never reaches the focused tab.
    app.tab_rename = Some(TabRenameState::new(usize::MAX, "", 128));
    app.handle_ime_event(Ime::Commit("wrong owner".into()));
    assert_eq!(saved(&app).as_deref(), Some("Roots 🫚"));
    assert!(app.handle_tab_rename_key(&Key::Named(NamedKey::Escape)));
    app.assert_pty_untouched();
}

#[test]
fn the_agent_view_composes_only_when_nothing_above_it_takes_text() {
    let mut app = App::new_headless(Config::default()).unwrap();
    app.open_agent_view(true);
    assert_eq!(app.text_target(), TextTarget::AgentView);
    app.tab_rename = Some(TabRenameState::new(0, "", 128));
    assert_ne!(app.text_target(), TextTarget::AgentView);
    app.tab_rename = None;
    app.command_palette.open();
    assert_ne!(app.text_target(), TextTarget::AgentView);
}

#[test]
fn empty_preedit_clears_composition() {
    let mut state = ImeState::default();
    state.set_preedit("かな".into(), None);
    state.set_preedit(String::new(), None);
    assert!(state.preedit().is_none());
}

#[test]
fn preedit_is_not_inserted_and_commit_is_inserted_once() {
    let mut app = App::new_headless(Config::default()).unwrap();
    app.command_palette.open();

    app.handle_ime_event(Ime::Enabled);
    app.handle_ime_event(Ime::Preedit("にほん".into(), Some((0, 9))));
    assert_eq!(app.command_palette.build_info().input, "");
    assert_eq!(app.ime.preedit().unwrap().text, "にほん");

    app.handle_ime_event(Ime::Commit("日本".into()));
    assert_eq!(app.command_palette.build_info().input, "日本");
    assert!(app.ime.preedit().is_none());
}

#[test]
fn ime_commit_reaches_the_active_information_overlay_input_only() {
    let mut app = App::new_headless(Config::default()).unwrap();
    let request = app.info_overlay.show_input(
        "Edit setting",
        vec![String::new()],
        0,
        "Value: ",
        "root",
        32,
        Vec::new(),
    );
    app.handle_ime_event(Ime::Preedit("しょうが".into(), None));
    assert_eq!(app.info_overlay.input_value(request), Some("root"));
    app.handle_ime_event(Ime::Commit("生姜".into()));
    assert_eq!(app.info_overlay.input_value(request), Some("root生姜"));
    assert!(app.ime.preedit().is_none());
    app.info_overlay.cycle_focus(request, false); // Copy button, not text.
    app.handle_ime_event(Ime::Preedit("blocked".into(), None));
    app.handle_ime_event(Ime::Commit("must not arrive".into()));
    assert_eq!(app.info_overlay.input_value(request), Some("root生姜"));
    assert!(app.ime.preedit().is_none());
}

#[test]
fn ime_commit_reaches_only_the_focused_pane_pty() {
    use crate::config::ShellConfig;
    use crate::panes::{PaneManager, SplitDirection};
    use crate::tabs::TerminalIdentity;
    use std::collections::HashMap;

    let shell = ShellConfig {
        program: Some("/bin/sh".into()),
        args: vec!["-c".into(), crate::test_support::byte_probe(6)],
        env: Default::default(),
    };
    // Wide enough that each half shows READY and six bytes on one row.
    let mut panes =
        PaneManager::for_test(80, 8, shell, 100, 1024 * 1024, 800.0, 160.0, 10.0, 20.0).unwrap();
    let right = panes
        .split(
            SplitDirection::Vertical,
            crate::config::PaneCwdMode::Focused,
        )
        .unwrap();
    panes.focus(right);

    let mut tabs = TabManager::new(40, 8, ShellConfig::default(), 100, 1024 * 1024);
    tabs.new_tab_with_command(
        "/bin/sh",
        &crate::test_support::sleeping_args(),
        None,
        Some("ime-routing"),
        None,
        HashMap::new(),
    )
    .unwrap();
    tabs.active_tab_mut().unwrap().panes = panes;
    let tab_id = tabs.active_tab_id().unwrap();
    let mut app = App::new_headless(Config::default()).unwrap();
    app.tabs = Some(tabs);
    let left = TerminalIdentity {
        tab_id,
        pane_id: PaneId(0),
    };
    let right = TerminalIdentity {
        tab_id,
        pane_id: right,
    };
    for pane in [left, right] {
        app.wait_for_terminal_text(pane, "READY");
    }

    // Each probe shows the first six bytes it reads, so the commit's six
    // bytes showing also proves the preedit before it wrote nothing.
    app.handle_ime_event(Ime::Preedit("にほん".into(), None));
    app.handle_ime_event(Ime::Commit("日本".into()));
    app.wait_for_terminal_text(right, "e6 97 a5 e6 9c ac");

    // Six bytes written to the other pane are the first it reads.
    app.active_panes_mut().focus(left.pane_id);
    app.write_to_pty(b"ZZZZZZ");
    app.wait_for_terminal_text(left, "5a 5a 5a 5a 5a 5a");
}
