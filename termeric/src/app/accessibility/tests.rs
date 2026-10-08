use super::terminal::append_accessible_cell;
use super::*;
use termeric_core::CellContent;

#[test]
fn vertical_tab_list_semantics_match_the_visible_left_rail() {
    let (orientation, bounds) = tab_list_semantics(true, 1_000.0, 700.0, 32.0, 220.0);
    assert_eq!(orientation, accesskit::Orientation::Vertical);
    assert_eq!(bounds, rect(0.0, 32.0, 220.0, 668.0));

    let (orientation, bounds) = tab_list_semantics(false, 1_000.0, 700.0, 32.0, 0.0);
    assert_eq!(orientation, accesskit::Orientation::Horizontal);
    assert_eq!(bounds, rect(0.0, 0.0, 1_000.0, 32.0));
}

#[test]
fn hidden_terminal_cells_never_enter_the_semantic_value() {
    let mut cell = Cell {
        content: CellContent::Grapheme("secret👁".into()),
        attrs: CellAttrs::HIDDEN,
        ..Cell::default()
    };
    let mut value = String::new();
    let mut lengths = Vec::new();
    append_accessible_cell(&cell, &mut value, &mut lengths);
    assert_eq!(value, " ");
    assert_eq!(lengths, [1]);

    cell.attrs = CellAttrs::empty();
    value.clear();
    lengths.clear();
    append_accessible_cell(&cell, &mut value, &mut lengths);
    assert_eq!(value, "secret👁");
    assert_eq!(lengths.len(), 7);
}

#[test]
fn incremental_updates_only_send_changed_nodes() {
    let mut state = AccessibilityState::default();
    let mut root = Node::new(Role::Window);
    root.set_label("Termeric");
    let first = state.update(SemanticSnapshot {
        nodes: vec![(ROOT, root.clone())],
        focus: ROOT,
    });
    assert!(first.tree.is_some());
    assert_eq!(first.nodes.len(), 1);
    let steady = state.update(SemanticSnapshot {
        nodes: vec![(ROOT, root)],
        focus: ROOT,
    });
    assert!(steady.tree.is_none());
    assert!(steady.nodes.is_empty());
}

#[test]
fn removed_palette_commands_release_their_semantic_identity() {
    let mut state = AccessibilityState::default();
    let removed = state.palette.node("plugin.removed");
    let retained = state.palette.node("core.retained");
    state.palette.retain(|id| id == "core.retained");
    assert!(!state.palette.targets.contains_key(&removed));
    assert!(!state.palette.nodes.contains_key("plugin.removed"));
    assert_eq!(state.palette.nodes.get("core.retained"), Some(&retained));
}

#[test]
fn replaced_unicode_results_release_their_semantic_identity() {
    let mut state = AccessibilityState::default();
    let removed = state.unicode.node("unicode-picker.curated.2192");
    let retained = state.unicode.node("unicode-picker.codepoint.3bb");
    state
        .unicode
        .retain(|id| id == "unicode-picker.codepoint.3bb");
    assert!(!state.unicode.targets.contains_key(&removed));
    assert!(
        !state
            .unicode
            .nodes
            .contains_key("unicode-picker.curated.2192")
    );
    assert_eq!(
        state.unicode.nodes.get("unicode-picker.codepoint.3bb"),
        Some(&retained)
    );
}

#[test]
fn terminal_tree_exposes_visible_text_cursor_and_selection_without_hidden_text() {
    let mut terminal = termeric_core::Terminal::new(16, 2);
    terminal.process(b"visible\x1b[8msecret\x1b[28m");
    terminal.set_selection(Some(termeric_core::Selection {
        start: termeric_core::Point::new(0, 0),
        end: termeric_core::Point::new(0, 6),
        ty: termeric_core::SelectionType::Simple,
        active: false,
    }));
    let mut nodes = Vec::new();
    append_terminal_nodes(
        &mut nodes,
        pane_node(PaneId(0)),
        PaneId(0),
        &terminal,
        true,
        None,
        true,
        false,
        rect(0.0, 0.0, 160.0, 40.0),
        20.0,
        &[],
    );
    let semantic_text = nodes
        .iter()
        .filter_map(|(_, node)| node.value())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(semantic_text.contains("visible"));
    assert!(!semantic_text.contains("secret"));
    let terminal_node = nodes
        .iter()
        .find(|(id, _)| *id == pane_node(PaneId(0)))
        .unwrap();
    assert!(
        terminal_node
            .1
            .label()
            .is_some_and(|label| label.contains("read-only input lock active"))
    );
    let selection = terminal_node.1.text_selection().unwrap();
    assert_eq!(selection.anchor.character_index, 0);
    assert_eq!(selection.focus.character_index, 7);
}

#[test]
fn terminal_tree_uses_frozen_filtered_rows_and_projected_unicode_selection() {
    let mut terminal = termeric_core::Terminal::new(16, 4);
    terminal.process("hit界\r\nSECRET\r\nhit e\u{301}\x1b[?2026h\x1b[H\x1b[2KREPLACED".as_bytes());
    for reversed in [false, true] {
        let (start, end) = (
            termeric_core::Point::new(0, 0),
            termeric_core::Point::new(2, 4),
        );
        terminal.set_selection(Some(Selection {
            start: if reversed { end } else { start },
            end: if reversed { start } else { end },
            ty: termeric_core::SelectionType::Simple,
            active: false,
        }));
        let mut nodes = Vec::new();
        append_terminal_nodes(
            &mut nodes,
            pane_node(PaneId(0)),
            PaneId(0),
            &terminal,
            true,
            None,
            false,
            false,
            rect(0.0, 0.0, 160.0, 80.0),
            20.0,
            &[0, 2],
        );
        let text = nodes
            .iter()
            .filter_map(|(_, node)| node.value())
            .collect::<Vec<_>>();
        assert_eq!(text, ["hit界", "hit e\u{301}"]);
        let selection = nodes
            .iter()
            .find(|(id, _)| *id == pane_node(PaneId(0)))
            .unwrap()
            .1
            .text_selection()
            .unwrap();
        let (start, end) = if reversed {
            (selection.focus, selection.anchor)
        } else {
            (selection.anchor, selection.focus)
        };
        assert_eq!(start.node, line_node(PaneId(0), 0));
        assert_eq!(start.character_index, 0);
        assert_eq!(end.node, line_node(PaneId(0), 1));
        assert_eq!(end.character_index, 6);
    }
    let mut nodes = Vec::new();
    append_terminal_nodes(
        &mut nodes,
        pane_node(PaneId(0)),
        PaneId(0),
        &terminal,
        true,
        None,
        false,
        false,
        rect(0.0, 0.0, 160.0, 80.0),
        20.0,
        &[],
    );
    let text = nodes
        .iter()
        .filter_map(|(_, node)| node.value())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(text.contains("hit界"));
    assert!(!text.contains("REPLACED"));
}

#[test]
fn visible_scrollback_selection_is_mapped_into_semantic_viewport_rows() {
    let mut terminal = termeric_core::Terminal::new(8, 2);
    terminal.process(b"old\r\nnew\r\nlive");
    terminal.grid_mut().set_scroll_offset(1);
    terminal.set_selection(Some(termeric_core::Selection {
        start: termeric_core::Point::new(-1, 0),
        end: termeric_core::Point::new(-1, 2),
        ty: termeric_core::SelectionType::Simple,
        active: false,
    }));
    let mut nodes = Vec::new();
    append_terminal_nodes(
        &mut nodes,
        pane_node(PaneId(0)),
        PaneId(0),
        &terminal,
        true,
        None,
        false,
        false,
        rect(0.0, 0.0, 80.0, 40.0),
        20.0,
        &[],
    );

    let terminal_node = nodes
        .iter()
        .find(|(id, _)| *id == pane_node(PaneId(0)))
        .unwrap();
    let selection = terminal_node.1.text_selection().unwrap();
    assert_eq!(selection.anchor.node, line_node(PaneId(0), 0));
    assert_eq!(selection.anchor.character_index, 0);
    assert_eq!(selection.focus.node, line_node(PaneId(0), 0));
    assert_eq!(selection.focus.character_index, 3);
}

#[test]
fn keyboard_chrome_sequence_names_every_global_control() {
    let app = App::new_headless(Config::default()).unwrap();
    assert_eq!(
        app.chrome_focus_sequence(),
        [
            ChromeFocus::NewTab,
            ChromeFocus::Commands,
            ChromeFocus::Workspaces,
            ChromeFocus::Settings,
        ]
    );
}

#[test]
fn f6_walks_the_chrome_and_enter_activates_what_has_focus() {
    use winit::keyboard::{Key, NamedKey};
    let mut config = Config::default();
    config.shell.program = Some("/bin/sh".into());
    config.shell.args = vec!["-c".into(), "sleep 30".into()];
    let mut app = App::new_headless_with_tabs(config);
    app.handle_action(crate::input::Action::NewTab);
    let press = |app: &mut App, key: NamedKey| {
        app.handle_chrome_keyboard(&Key::Named(key), ElementState::Pressed)
    };

    // Keys belong to the terminal until F6 moves focus into the chrome.
    assert!(!press(&mut app, NamedKey::ArrowRight));
    assert!(press(&mut app, NamedKey::F6));
    assert_eq!(app.accessibility.keyboard_focus, ChromeFocus::Tab(0));
    assert!(press(&mut app, NamedKey::ArrowRight));
    assert_eq!(app.accessibility.keyboard_focus, ChromeFocus::CloseTab(0));
    assert!(press(&mut app, NamedKey::Tab));
    assert_eq!(app.accessibility.keyboard_focus, ChromeFocus::Tab(1));
    assert!(press(&mut app, NamedKey::End));
    assert_eq!(app.accessibility.keyboard_focus, ChromeFocus::Settings);
    // Releases stay in the chrome too, except F6's own.
    assert!(app.handle_chrome_keyboard(&Key::Named(NamedKey::Tab), ElementState::Released));
    assert!(!app.handle_chrome_keyboard(&Key::Named(NamedKey::F6), ElementState::Released));
    assert!(press(&mut app, NamedKey::Escape));
    assert_eq!(app.accessibility.keyboard_focus, ChromeFocus::Terminal);

    // Choosing a tab switches to it and leaves focus on it.
    press(&mut app, NamedKey::F6);
    press(&mut app, NamedKey::Home);
    assert!(press(&mut app, NamedKey::Enter));
    assert_eq!(app.tabs.as_ref().unwrap().active_index(), 0);
    assert_eq!(app.accessibility.keyboard_focus, ChromeFocus::Tab(0));

    // New Tab and Settings act, then hand the keyboard back.
    while app.accessibility.keyboard_focus != ChromeFocus::NewTab {
        press(&mut app, NamedKey::ArrowRight);
    }
    assert!(press(&mut app, NamedKey::Space));
    assert_eq!(app.tabs.as_ref().unwrap().count(), 3);
    assert_eq!(app.accessibility.keyboard_focus, ChromeFocus::Terminal);
    press(&mut app, NamedKey::F6);
    press(&mut app, NamedKey::End);
    press(&mut app, NamedKey::Enter);
    assert!(app.settings_panel.is_active());
    assert_eq!(app.accessibility.keyboard_focus, ChromeFocus::Terminal);

    // Shift+F6 enters from the end of the ring.
    app.settings_panel.close();
    app.modifiers = ModifiersState::SHIFT;
    press(&mut app, NamedKey::F6);
    assert_eq!(app.accessibility.keyboard_focus, ChromeFocus::Settings);
}

#[test]
fn a_screen_reader_click_on_close_tab_hands_the_keyboard_back_like_f6_enter() {
    use winit::keyboard::{Key, NamedKey};
    let config = Config {
        shell: crate::test_support::sleeping_shell(),
        ..Config::default()
    };
    let mut app = App::new_headless_with_tabs(config);
    app.handle_action(crate::input::Action::NewTab);
    assert_eq!(app.tabs.as_ref().unwrap().count(), 2);

    app.handle_accessibility_action(click(tab_close_node(0)));
    let closing = |app: &App| {
        app.tabs
            .as_ref()
            .unwrap()
            .iter()
            .filter(|(_, tab)| tab.is_closing())
            .count()
    };
    assert_eq!(closing(&app), 1);
    assert_eq!(app.accessibility.keyboard_focus, ChromeFocus::Terminal);
    assert_eq!(app.accessibility.focus_override, None);
    assert_eq!(app.chrome_focus_render_target(), None);
    // Keys reach the terminal again instead of the chrome swallowing them.
    assert!(!app.handle_chrome_keyboard(&Key::Character("a".into()), ElementState::Pressed));
    assert!(!app.handle_chrome_keyboard(&Key::Named(NamedKey::Enter), ElementState::Pressed));

    // Focus alone moves the focus ring without acting, as F6 and the arrows do.
    app.handle_accessibility_action(ActionRequest {
        action: Action::Focus,
        target_tree: TreeId::ROOT,
        target_node: tab_close_node(0),
        data: None,
    });
    assert_eq!(closing(&app), 1);
    assert_eq!(app.accessibility.keyboard_focus, ChromeFocus::CloseTab(0));
    assert_eq!(app.accessibility.focus_override, Some(tab_close_node(0)));
    // A control that is not drawn takes no focus.
    app.handle_accessibility_action(click(tab_close_node(5)));
    assert_eq!(app.accessibility.keyboard_focus, ChromeFocus::CloseTab(0));
}

#[test]
fn dispatched_primary_press_returns_chrome_focus_to_terminal() {
    let mut app = App::new_headless(Config::default()).unwrap();
    for focus in [
        ChromeFocus::Tab(19),
        ChromeFocus::CloseTab(19),
        ChromeFocus::PreviousTab,
        ChromeFocus::NextTab,
        ChromeFocus::NewTab,
        ChromeFocus::Commands,
        ChromeFocus::Workspaces,
        ChromeFocus::Settings,
    ] {
        app.set_chrome_focus(focus);
        app.finish_pointer_chrome_focus(MouseButton::Left, ElementState::Pressed);
        assert_eq!(app.accessibility.keyboard_focus, ChromeFocus::Terminal);
        assert_eq!(app.accessibility.focus_override, None);
        assert_eq!(app.chrome_focus_render_target(), None);
    }
}

#[test]
fn pointer_release_and_other_buttons_do_not_change_chrome_focus() {
    let mut app = App::new_headless(Config::default()).unwrap();
    app.set_chrome_focus(ChromeFocus::Tab(19));
    for (button, state) in [
        (MouseButton::Left, ElementState::Released),
        (MouseButton::Right, ElementState::Pressed),
        (MouseButton::Middle, ElementState::Pressed),
    ] {
        app.finish_pointer_chrome_focus(button, state);
        assert_eq!(app.accessibility.keyboard_focus, ChromeFocus::Tab(19));
        assert_eq!(app.accessibility.focus_override, Some(tab_node(19)));
    }
    // An already-terminal focus must not clear a text surface's AT focus
    // override merely because the normal pointer path finished.
    app.accessibility.keyboard_focus = ChromeFocus::Terminal;
    app.accessibility.focus_override = Some(SEARCH_INPUT);
    app.search.start();
    app.handle_ime_event(winit::event::Ime::Preedit("draft".into(), Some((0, 5))));
    let preedit = app.ime.preedit().cloned();
    assert!(preedit.is_some());
    app.finish_pointer_chrome_focus(MouseButton::Left, ElementState::Pressed);
    assert_eq!(app.accessibility.focus_override, Some(SEARCH_INPUT));
    assert_eq!(app.ime.preedit(), preedit.as_ref());
}

#[test]
fn command_palette_is_one_modal_search_and_result_tree_with_shared_geometry() {
    let mut app = App::new_headless(Config::default()).unwrap();
    // The layout slides with the palette's fade; reduced motion holds it
    // where it comes to rest.
    app.command_palette.set_reduced_motion(true);
    app.command_palette.open();
    let info = app.command_palette.build_info();
    let layout = app.command_palette_accessibility_layout(&info);
    let snapshot = app.accessibility_snapshot();
    let assert_rect_close = |actual: Option<Rect>, expected: Rect| {
        let actual = actual.unwrap();
        for (actual, expected) in [
            (actual.x0, expected.x0),
            (actual.y0, expected.y0),
            (actual.x1, expected.x1),
            (actual.y1, expected.y1),
        ] {
            assert!((actual - expected).abs() < 0.01, "{actual} != {expected}");
        }
    };

    let node = |id| {
        &snapshot
            .nodes
            .iter()
            .find(|(node_id, _)| *node_id == id)
            .unwrap()
            .1
    };
    assert_eq!(node(ROOT).children(), &[PALETTE_DIALOG]);
    assert_eq!(node(PALETTE_DIALOG).role(), Role::Dialog);
    assert!(node(PALETTE_DIALOG).is_modal());
    assert_eq!(node(PALETTE_INPUT).role(), Role::SearchInput);
    assert_eq!(node(PALETTE_INPUT).controls(), &[PALETTE_LIST]);
    assert!(node(PALETTE_INPUT).supports_action(Action::SetValue));
    assert_eq!(snapshot.focus, PALETTE_INPUT);
    assert_rect_close(node(PALETTE_INPUT).bounds(), bounds_rect(layout.input));

    let selected = node(PALETTE_INPUT).active_descendant().unwrap();
    assert_eq!(node(selected).role(), Role::ListBoxOption);
    assert_eq!(node(selected).is_selected(), Some(true));
    assert!(node(selected).supports_action(Action::Click));
    assert_rect_close(node(selected).bounds(), bounds_rect(layout.row(0)));
}

#[test]
fn search_semantic_focus_resumes_confirmed_input_and_preserves_results() {
    let mut app = App::new_headless(Config::default()).unwrap();
    let mut terminal = termeric_core::Terminal::new(40, 2);
    terminal.process(b"needle needle");
    app.search.start();
    assert!(app.search.set_query("needle", terminal.grid()));
    app.search.finish(terminal.grid());
    app.search_cursor_pos = 2;
    app.search.confirm();
    let selected = app.search.current().cloned();
    app.handle_ime_event(winit::event::Ime::Preedit("draft".into(), None));
    app.handle_accessibility_action(ActionRequest {
        action: Action::Focus,
        target_tree: TreeId::ROOT,
        target_node: SEARCH_INPUT,
        data: None,
    });
    assert!(app.search.is_active());
    assert_eq!(app.search_cursor_pos, 2);
    assert_eq!(app.search.current(), selected.as_ref());
    assert_eq!(app.search.match_count(), 2);
    assert!(!app.search.is_searching());
    assert!(app.ime.preedit().is_none());
    assert_eq!(app.accessibility_snapshot().focus, SEARCH_INPUT);
    assert!(app.search.set_query("e\u{301}", terminal.grid()));
    app.search_cursor_pos = 1;
    app.search.confirm();
    assert!(app.search.is_searching());
    app.handle_accessibility_action(ActionRequest {
        action: Action::Focus,
        target_tree: TreeId::ROOT,
        target_node: SEARCH_INPUT,
        data: None,
    });
    assert!(app.search.is_active());
    assert!(
        app.search.is_searching(),
        "resuming does not finish or discard a scan"
    );
    assert_eq!(app.search_cursor_pos, "e\u{301}".len());
    app.search.cancel();
    app.handle_accessibility_action(ActionRequest {
        action: Action::Focus,
        target_tree: TreeId::ROOT,
        target_node: SEARCH_INPUT,
        data: None,
    });
    assert!(
        !app.search.is_active(),
        "a stale hidden input cannot reopen search"
    );
}

#[test]
fn search_semantic_replacement_retires_preedit_and_never_writes_to_pty() {
    let mut app =
        App::new_headless_running(crate::test_support::FIRST_BYTE_PROBE, "search-semantic");
    let replace = |app: &mut App, data| {
        app.handle_accessibility_action(ActionRequest {
            action: Action::SetValue,
            target_tree: TreeId::ROOT,
            target_node: SEARCH_INPUT,
            data,
        })
    };
    app.wait_for_screen_text("READY");
    app.search.start();
    for value in ["READY", "e\u{301}👩\u{200d}💻", &"🫚".repeat(1024), ""] {
        app.handle_ime_event(winit::event::Ime::Preedit("draft".into(), None));
        replace(&mut app, Some(ActionData::Value(value.into())));
        assert_eq!(app.search.query(), value);
        assert_eq!(app.search_cursor_pos, value.len());
        assert!(app.ime.preedit().is_none());
        let snapshot = app.accessibility_snapshot();
        assert_eq!(snapshot.focus, SEARCH_INPUT);
        let input = &snapshot
            .nodes
            .iter()
            .find(|(id, _)| *id == SEARCH_INPUT)
            .unwrap()
            .1;
        assert_eq!(
            input.text_selection().unwrap().focus.character_index,
            value.chars().count()
        );
    }
    app.handle_ime_event(winit::event::Ime::Preedit("keep".into(), None));
    for data in [
        None,
        Some(ActionData::Value("bad\nquery".into())),
        Some(ActionData::Value("x".repeat(4097).into())),
    ] {
        replace(&mut app, data);
        assert_eq!(app.search.query(), "");
        assert_eq!(app.search_cursor_pos, 0);
        assert!(app.ime.preedit().is_some());
    }
    app.handle_ime_event(winit::event::Ime::Commit("bad\ncommit".into()));
    assert_eq!(app.search.query(), "");
    replace(&mut app, Some(ActionData::Value("READY".into())));
    app.with_search_and_active_grid(|search, grid| search.finish(grid));
    app.search.confirm();
    replace(&mut app, Some(ActionData::Value("stale".into())));
    assert_eq!(
        app.search.query(),
        "READY",
        "read-only query rejects stale SetValue"
    );
    app.handle_accessibility_action(ActionRequest {
        action: Action::Focus,
        target_tree: TreeId::ROOT,
        target_node: SEARCH_INPUT,
        data: None,
    });
    app.handle_ime_event(winit::event::Ime::Commit("界".into()));
    assert_eq!(app.search.query(), "READY界");
    app.command_palette.open();
    replace(&mut app, Some(ActionData::Value("hidden".into())));
    assert_eq!(app.search.query(), "READY界", "higher modal owns input");
    app.command_palette.close();
    app.search.cancel();
    app.assert_pty_untouched();
}

#[test]
fn terminal_search_exposes_editable_status_controls_and_shared_geometry() {
    let mut app = App::new_headless(Config::default()).unwrap();
    let terminal = termeric_core::Terminal::new(40, 2);
    app.search.start();
    assert!(app.search.set_query("needle", terminal.grid()));
    app.search_cursor_pos = "needle".len();
    let info = app.search.build_info(app.search_cursor_pos);
    let layout = app.search_accessibility_layout(&info);
    let snapshot = app.accessibility_snapshot();
    let node = |id| {
        &snapshot
            .nodes
            .iter()
            .find(|(node_id, _)| *node_id == id)
            .unwrap()
            .1
    };

    assert!(node(ROOT).children().contains(&SEARCH_TOOLBAR));
    assert_eq!(snapshot.focus, SEARCH_INPUT);
    assert_eq!(node(SEARCH_INPUT).role(), Role::SearchInput);
    assert_eq!(node(SEARCH_INPUT).value(), Some("needle"));
    assert!(node(SEARCH_INPUT).supports_action(Action::SetValue));
    assert_eq!(node(SEARCH_STATUS).role(), Role::Status);
    assert_eq!(node(SEARCH_STATUS).live(), Some(Live::Polite));
    assert_eq!(node(SEARCH_FILTER).toggled(), Some(Toggled::False));
    assert!(node(SEARCH_CLOSE).supports_action(Action::Click));
    assert_eq!(node(SEARCH_INPUT).bounds(), Some(bounds_rect(layout.input)));
    assert_eq!(node(SEARCH_REGEX).bounds(), Some(bounds_rect(layout.regex)));

    app.handle_accessibility_action(ActionRequest {
        action: Action::Focus,
        target_tree: TreeId::ROOT,
        target_node: SEARCH_REGEX,
        data: None,
    });
    assert_eq!(app.accessibility_snapshot().focus, SEARCH_REGEX);
    app.search.confirm();
    assert_ne!(app.accessibility_snapshot().focus, SEARCH_INPUT);

    app.handle_accessibility_action(ActionRequest {
        action: Action::Click,
        target_tree: TreeId::ROOT,
        target_node: SEARCH_CLOSE,
        data: None,
    });
    assert!(!app.search_is_visible());
    assert!(
        !app.accessibility_snapshot()
            .nodes
            .iter()
            .any(|(id, _)| *id == SEARCH_TOOLBAR)
    );
}

#[test]
fn toasts_are_stable_bounded_live_regions_even_above_modals() {
    let mut app = App::new_headless(Config::default()).unwrap();
    app.toasts.error("failure");
    app.command_palette.open();
    let base = app.accessibility_snapshot();
    let snapshot = app.with_toast_accessibility(base);
    let toast = toast_node(1);
    let root = &snapshot.nodes.iter().find(|(id, _)| *id == ROOT).unwrap().1;
    assert!(root.children().contains(&PALETTE_DIALOG));
    assert!(root.children().contains(&toast));
    let node = &snapshot
        .nodes
        .iter()
        .find(|(id, _)| *id == toast)
        .unwrap()
        .1;
    assert_eq!(node.role(), Role::Status);
    assert_eq!(node.live(), Some(Live::Assertive));
    assert_eq!(node.label(), Some("Error: failure"));

    let second_base = app.accessibility_snapshot();
    let second = app.with_toast_accessibility(second_base);
    assert!(second.nodes.iter().any(|(id, _)| *id == toast));
}

#[test]
fn toasts_are_dismissed_by_their_accessibility_actions_even_above_modals() {
    let mut app = App::new_headless(Config::default()).unwrap();
    app.toasts.error("first");
    app.toasts.warning("second");
    app.toasts.info("third");
    app.command_palette.open();
    let toast_ids = |app: &App| -> Vec<u64> {
        app.toasts
            .build_accessibility_info()
            .iter()
            .map(|toast| toast.id)
            .collect()
    };
    let [first, second, third] = toast_ids(&app)[..] else {
        panic!("three toasts expected");
    };
    let base = app.accessibility_snapshot();
    let snapshot = app.with_toast_accessibility(base);
    let node = &snapshot
        .nodes
        .iter()
        .find(|(id, _)| *id == toast_node(first))
        .unwrap()
        .1;
    assert!(node.supports_action(Action::Click));
    assert!(node.supports_action(Action::CustomAction));
    assert_eq!(
        node.custom_actions()
            .iter()
            .map(|action| (action.id, action.description.as_ref()))
            .collect::<Vec<_>>(),
        [(TOAST_DISMISS_ACTION, "Dismiss")]
    );

    let request = |toast, action, data| ActionRequest {
        action,
        target_tree: TreeId::ROOT,
        target_node: toast_node(toast),
        data,
    };
    // Neither focus nor another custom action dismisses a toast.
    app.handle_accessibility_action(request(third, Action::Focus, None));
    app.handle_accessibility_action(request(
        third,
        Action::CustomAction,
        Some(ActionData::CustomAction(SHORTCUT_CLEAR_ACTION)),
    ));
    app.handle_accessibility_action(request(first, Action::Click, None));
    app.handle_accessibility_action(request(
        second,
        Action::CustomAction,
        Some(ActionData::CustomAction(TOAST_DISMISS_ACTION)),
    ));
    assert_eq!(toast_ids(&app), [third]);
    // The modal beneath the toasts saw none of it.
    assert!(app.command_palette.is_active());
}

fn open_harvest_cellar_fixture(app: &mut App, count: usize) {
    let mut harvest = crate::harvest::HarvestMode::with_capacity(count.max(1));
    harvest.cellar.animation = crate::ui::animation::PanelAnimation::with_duration(0.0);
    for index in 0..count {
        let mut entry = crate::harvest::CommandEntry::new_for_source(
            crate::harvest::HarvestSource {
                tab_id: 7,
                pane_id: 2,
            },
            index as i32,
            Some(if index % 3 == 0 { 1 } else { 0 }),
        );
        entry.command = format!("cargo test fixture-{index}");
        harvest.cellar.add(entry);
    }
    harvest.cellar.toggle_expanded();
    app.harvest = Some(harvest);
}

#[test]
fn root_cellar_is_bounded_modal_command_history_with_shared_actions() {
    let mut app = App::new_headless(Config::default()).unwrap();
    open_harvest_cellar_fixture(&mut app, 30);
    let layout = app.cellar_layout().unwrap();
    let snapshot = app.accessibility_snapshot();
    let node = |id| {
        &snapshot
            .nodes
            .iter()
            .find(|(node_id, _)| *node_id == id)
            .unwrap()
            .1
    };
    assert_eq!(node(ROOT).children(), &[CELLAR_DIALOG]);
    assert!(node(CELLAR_DIALOG).is_modal());
    assert_eq!(node(CELLAR_LIST).role(), Role::ListBox);
    assert!(node(CELLAR_LIST).supports_action(Action::ScrollUp));
    assert_eq!(
        node(CELLAR_DIALOG).bounds(),
        Some(bounds_rect(layout.panel))
    );
    assert_eq!(node(CELLAR_CLOSE).bounds(), Some(bounds_rect(layout.close)));
    let focused = snapshot.focus;
    assert_eq!(node(focused).role(), Role::ListBoxOption);
    assert!(node(focused).label().unwrap().contains("fixture-29"));
    assert!(
        node(focused)
            .description()
            .unwrap()
            .contains("tab 7, pane 3")
    );

    app.handle_accessibility_action(ActionRequest {
        action: Action::ScrollUp,
        target_tree: TreeId::ROOT,
        target_node: CELLAR_LIST,
        data: None,
    });
    let scrolled = app.accessibility_snapshot();
    assert_ne!(scrolled.focus, focused);

    let selected_id = app.harvest.as_ref().unwrap().cellar.selected_id().unwrap();
    let selected_node = *app
        .accessibility
        .harvest
        .nodes
        .get(&HarvestSemanticTarget::CellarEntry(selected_id))
        .unwrap();
    app.handle_accessibility_action(ActionRequest {
        action: Action::Click,
        target_tree: TreeId::ROOT,
        target_node: selected_node,
        data: None,
    });
    assert!(!app.harvest.as_ref().unwrap().cellar.is_expanded());
    assert!(app.harvest.as_ref().unwrap().sprout_menu.is_some());
    assert_eq!(
        app.accessibility_snapshot()
            .nodes
            .iter()
            .find(|(id, _)| *id == ROOT)
            .unwrap()
            .1
            .children(),
        &[SPROUT_DIALOG]
    );
}

#[test]
fn root_cellar_announces_retention_eviction_and_clipped_output() {
    let mut app = App::new_headless(Config::default()).unwrap();
    open_harvest_cellar_fixture(&mut app, 2);
    let mut clipped = crate::harvest::CommandEntry::new_for_source(
        crate::harvest::HarvestSource {
            tab_id: 7,
            pane_id: 2,
        },
        3,
        Some(0),
    );
    clipped.command = "cargo test clipped".into();
    clipped.output_truncated = true;
    app.harvest.as_mut().unwrap().cellar.add(clipped);

    let snapshot = app.accessibility_snapshot();
    let status = &snapshot
        .nodes
        .iter()
        .find(|(id, _)| *id == CELLAR_STATUS)
        .unwrap()
        .1;
    assert!(status.label().unwrap().contains("1 older commands evicted"));
    let selected = snapshot.focus;
    let selected = &snapshot
        .nodes
        .iter()
        .find(|(id, _)| *id == selected)
        .unwrap()
        .1;
    assert!(
        selected
            .description()
            .unwrap()
            .contains("only the last 64 KiB of output is retained")
    );
}

#[test]
fn workspace_menu_is_a_menu_whose_items_focus_and_activate_like_the_keyboard() {
    let store = tempfile::tempdir().unwrap();
    crate::workspace::set_test_workspaces_base_dir(Some(store.path().to_path_buf()));
    let mut app = App::new_headless(Config::default()).unwrap();
    app.toggle_workspace_menu(true);
    let snapshot = app.accessibility_snapshot();
    let node = |id: NodeId| {
        &snapshot
            .nodes
            .iter()
            .find(|(node, _)| *node == id)
            .unwrap()
            .1
    };
    assert_eq!(node(WORKSPACE_MENU).role(), Role::Menu);
    let items = node(WORKSPACE_MENU).children().to_vec();
    let labels: Vec<_> = items
        .iter()
        .map(|id| node(*id).label().unwrap().to_owned())
        .collect();
    assert_eq!(
        labels,
        ["Save Workspace", "Save Workspace As…", "Manage Workspaces…"]
    );
    assert!(items.iter().all(|id| node(*id).role() == Role::MenuItem));
    assert_eq!(
        snapshot.focus, items[0],
        "focus follows the highlighted item"
    );

    app.handle_accessibility_action(ActionRequest {
        action: Action::Focus,
        target_node: items[2],
        target_tree: TreeId::ROOT,
        data: None,
    });
    assert_eq!(app.accessibility_snapshot().focus, items[2]);
    app.handle_accessibility_action(ActionRequest {
        action: Action::Click,
        target_node: items[2],
        target_tree: TreeId::ROOT,
        data: None,
    });
    assert!(app.workspace_menu.is_none());
    assert!(app.workspace_panel.is_open() && app.workspace_panel.is_workspace_mode());
    crate::workspace::set_test_workspaces_base_dir(None);
}

#[test]
fn explain_is_not_offered_while_agent_features_are_off() {
    use crate::harvest::sprouts::SproutAction;
    let mut app = App::new_headless(Config::default()).unwrap();
    open_harvest_cellar_fixture(&mut app, 1);
    let entry = app
        .harvest
        .as_ref()
        .unwrap()
        .cellar
        .latest()
        .unwrap()
        .clone();
    let availability = app.sprout_availability_for_entry(&entry);
    assert!(!availability.agent_features);
    app.harvest.as_mut().unwrap().show_sprout_menu(
        entry.row.max(0) as usize,
        0,
        Some(entry.clone()),
        availability,
    );
    let menu = app.harvest.as_ref().unwrap().sprout_menu.as_ref().unwrap();
    assert!(!menu.actions.contains(&SproutAction::Explain));
    app.accessibility_snapshot();
    assert!(
        app.accessibility
            .harvest
            .nodes
            .keys()
            .all(|target| !matches!(
                target,
                HarvestSemanticTarget::SproutAction {
                    action: SproutAction::Explain,
                    ..
                }
            ))
    );
    // Even asked for directly, nothing is handed to an agent.
    app.execute_sprout_action(SproutAction::Explain, Some(entry));
    assert_eq!(
        app.toasts.iter().last().map(|toast| toast.message.as_str()),
        Some(crate::agents::config::AGENT_FEATURES_OFF)
    );
}

#[test]
fn the_sprout_menu_nodes_sit_where_the_menu_is_drawn_inside_the_window() {
    let mut app = App::new_headless(Config::default().with_agent_features()).unwrap();
    open_harvest_cellar_fixture(&mut app, 1);
    let entry = app
        .harvest
        .as_ref()
        .unwrap()
        .cellar
        .latest()
        .unwrap()
        .clone();
    let availability = app.sprout_availability_for_entry(&entry);
    app.harvest
        .as_mut()
        .unwrap()
        .show_sprout_menu(0, 1_000, Some(entry), availability);
    let layout = app.sprout_menu_layout().unwrap();
    let snapshot = app.accessibility_snapshot();
    let metrics = app.ui_metrics();
    let bounds = |id: NodeId| {
        snapshot
            .nodes
            .iter()
            .find(|(node_id, _)| *node_id == id)
            .and_then(|(_, node)| node.bounds())
            .unwrap()
    };
    assert_eq!(
        bounds(ROOT),
        Rect::new(0.0, 0.0, metrics.width as f64, metrics.height as f64)
    );
    assert_eq!(bounds(SPROUT_DIALOG), bounds_rect(layout.panel));
    // A menu opened far right is pulled back inside the terminal area.
    assert!(layout.panel.right() <= metrics.width);
}

#[test]
fn sprout_semantics_are_request_scoped_and_never_invoke_explain_in_tests() {
    let mut app = App::new_headless(Config::default().with_agent_features()).unwrap();
    open_harvest_cellar_fixture(&mut app, 1);
    let entry = app
        .harvest
        .as_ref()
        .unwrap()
        .cellar
        .latest()
        .unwrap()
        .clone();
    let availability = app.sprout_availability_for_entry(&entry);
    app.harvest.as_mut().unwrap().show_sprout_menu(
        entry.row.max(0) as usize,
        0,
        Some(entry.clone()),
        availability,
    );
    let first = app.accessibility_snapshot();
    let first_request = app
        .harvest
        .as_ref()
        .unwrap()
        .sprout_menu
        .as_ref()
        .unwrap()
        .request_id;
    let old_copy = *app
        .accessibility
        .harvest
        .nodes
        .get(&HarvestSemanticTarget::SproutAction {
            request_id: first_request,
            action: crate::harvest::sprouts::SproutAction::CopyCommand,
        })
        .unwrap();
    let explain = *app
        .accessibility
        .harvest
        .nodes
        .get(&HarvestSemanticTarget::SproutAction {
            request_id: first_request,
            action: crate::harvest::sprouts::SproutAction::Explain,
        })
        .unwrap();
    let search = *app
        .accessibility
        .harvest
        .nodes
        .get(&HarvestSemanticTarget::SproutAction {
            request_id: first_request,
            action: crate::harvest::sprouts::SproutAction::SearchOutput,
        })
        .unwrap();
    let node = |id| {
        &first
            .nodes
            .iter()
            .find(|(node_id, _)| *node_id == id)
            .unwrap()
            .1
    };
    assert_eq!(node(ROOT).children(), &[SPROUT_DIALOG]);
    assert!(node(SPROUT_DIALOG).is_modal());
    assert_eq!(node(SPROUT_MENU).role(), Role::Menu);
    assert!(
        node(explain)
            .description()
            .unwrap()
            .contains("into the focused agent terminal, or the only one open")
    );
    assert!(node(old_copy).supports_action(Action::Click));
    assert!(node(search).is_disabled());
    assert!(!node(search).supports_action(Action::Click));
    assert!(node(search).label().unwrap().contains("not retained"));
    app.handle_accessibility_action(ActionRequest {
        action: Action::Click,
        target_tree: TreeId::ROOT,
        target_node: search,
        data: None,
    });
    assert_eq!(
        app.harvest
            .as_ref()
            .unwrap()
            .sprout_menu
            .as_ref()
            .unwrap()
            .request_id,
        first_request
    );

    let availability = app.sprout_availability_for_entry(&entry);
    app.harvest.as_mut().unwrap().show_sprout_menu(
        entry.row.max(0) as usize,
        0,
        Some(entry),
        availability,
    );
    app.accessibility_snapshot();
    let replacement_request = app
        .harvest
        .as_ref()
        .unwrap()
        .sprout_menu
        .as_ref()
        .unwrap()
        .request_id;
    assert_ne!(replacement_request, first_request);
    app.handle_accessibility_action(ActionRequest {
        action: Action::Click,
        target_tree: TreeId::ROOT,
        target_node: old_copy,
        data: None,
    });
    assert_eq!(
        app.harvest
            .as_ref()
            .unwrap()
            .sprout_menu
            .as_ref()
            .unwrap()
            .request_id,
        replacement_request
    );
    app.handle_accessibility_action(ActionRequest {
        action: Action::Click,
        target_tree: TreeId::ROOT,
        target_node: SETTINGS,
        data: None,
    });
    assert!(!app.settings_panel.is_active());

    let copy = *app
        .accessibility
        .harvest
        .nodes
        .get(&HarvestSemanticTarget::SproutAction {
            request_id: replacement_request,
            action: crate::harvest::sprouts::SproutAction::CopyCommand,
        })
        .unwrap();
    app.handle_accessibility_action(ActionRequest {
        action: Action::Click,
        target_tree: TreeId::ROOT,
        target_node: copy,
        data: None,
    });
    assert!(app.harvest.as_ref().unwrap().sprout_menu.is_none());
}

#[test]
fn palette_accessibility_action_executes_the_announced_stable_command_only() {
    let mut app = App::new_headless(Config::default()).unwrap();
    app.command_palette.open();
    for character in "toggle fullscreen".chars() {
        app.command_palette
            .insert_query_text(character.encode_utf8(&mut [0; 4]));
    }
    let first = app.accessibility_snapshot();
    let command_node = *app
        .accessibility
        .palette
        .nodes
        .get("core.toggle-fullscreen")
        .unwrap();
    assert!(first.nodes.iter().any(|(id, _)| *id == command_node));

    app.handle_accessibility_action(ActionRequest {
        action: Action::Click,
        target_tree: TreeId::ROOT,
        target_node: command_node,
        data: None,
    });
    assert!(app.fullscreen_requested);
    assert!(!app.command_palette.is_active());

    app.command_palette.open();
    for character in "toggle fullscreen".chars() {
        app.command_palette
            .insert_query_text(character.encode_utf8(&mut [0; 4]));
    }
    let second = app.accessibility_snapshot();
    assert!(second.nodes.iter().any(|(id, _)| *id == command_node));
    assert_eq!(
        app.accessibility
            .palette
            .nodes
            .get("core.toggle-fullscreen"),
        Some(&command_node),
        "semantic command identity must survive close, filter, and reopen"
    );
}

fn click(node: NodeId) -> ActionRequest {
    ActionRequest {
        action: Action::Click,
        target_tree: TreeId::ROOT,
        target_node: node,
        data: None,
    }
}

#[test]
fn modals_consume_stale_background_accessibility_actions() {
    type Open = fn(&mut App);
    type IsOpen = fn(&App) -> bool;
    // (modal, open it, whether it is still open, a background control)
    let cases: [(&str, Open, IsOpen, NodeId); 6] = [
        (
            "palette",
            |app| app.command_palette.open(),
            |app| app.command_palette.is_active(),
            SETTINGS,
        ),
        (
            "settings",
            |app| app.settings_panel.toggle(),
            |app| app.settings_panel.is_active(),
            COMMANDS,
        ),
        (
            "custom commands",
            |app| app.commands_panel.toggle(),
            |app| app.commands_panel.is_open(),
            SETTINGS,
        ),
        (
            "workspace manager",
            |app| {
                app.workspace_panel
                    .set_test_entries(test_workspace_entries(1), true)
            },
            |app| app.workspace_panel.is_open(),
            SETTINGS,
        ),
        (
            "shortcuts editor",
            |app| app.shortcuts_panel.open(),
            |app| app.shortcuts_panel.is_active(),
            SETTINGS,
        ),
        (
            "guardrail",
            |app| {
                show_guardrail_fixture(app);
            },
            |app| app.guardrail.is_awaiting_current(),
            SETTINGS,
        ),
    ];
    for (modal, open, still_open, background) in cases {
        let mut app = App::new_headless(Config::default()).unwrap();
        open(&mut app);
        app.handle_accessibility_action(click(background));
        assert!(still_open(&app), "{modal} closed");
        let background_opened = match background {
            SETTINGS => app.settings_panel.is_active(),
            COMMANDS => app.commands_panel.is_open(),
            _ => unreachable!(),
        };
        assert!(!background_opened, "{modal} let {background:?} through");
    }
}

#[test]
fn palette_semantic_replacement_retires_composition_only_when_accepted() {
    for kind in [PALETTE_INPUT, UNICODE_INPUT, DOMAIN_INPUT] {
        let mut app = App::new_headless(Config::default()).unwrap();
        if kind == PALETTE_INPUT {
            app.command_palette.open();
        } else if kind == UNICODE_INPUT {
            app.unicode_picker.open();
        } else {
            app.domain_picker.open(&app.config.domains);
        }
        let set = |app: &mut App, value: &str| {
            app.handle_accessibility_action(ActionRequest {
                action: Action::SetValue,
                target_tree: TreeId::ROOT,
                target_node: kind,
                data: Some(ActionData::Value(value.into())),
            })
        };
        for value in ["e\u{301}👩\u{200d}💻", &"🫚".repeat(256), ""] {
            app.handle_ime_event(winit::event::Ime::Preedit("draft".into(), None));
            assert!(app.ime.preedit().is_some());
            set(&mut app, value);
            assert!(app.ime.preedit().is_none());
            let snapshot = app.accessibility_snapshot();
            assert_eq!(snapshot.focus, kind);
            let input = &snapshot.nodes.iter().find(|(id, _)| *id == kind).unwrap().1;
            assert_eq!(input.value(), Some(value));
            assert_eq!(
                input.text_selection().unwrap().focus.character_index,
                value.chars().count()
            );
        }
        set(&mut app, "keep");
        app.handle_ime_event(winit::event::Ime::Preedit("keep composition".into(), None));
        let preedit = app.ime.preedit().cloned();
        for value in ["a\nb", "\u{2028}", "\u{2029}", &"x".repeat(257)] {
            set(&mut app, value);
            assert_eq!(app.ime.preedit(), preedit.as_ref());
            let snapshot = app.accessibility_snapshot();
            let input = &snapshot.nodes.iter().find(|(id, _)| *id == kind).unwrap().1;
            assert_eq!(input.value(), Some("keep"));
        }
        assert!(!app.fullscreen_requested);
        app.unicode_picker.close();
        app.unicode_picker.finish_search_for_test();
    }
}

#[test]
fn palette_semantic_filter_is_bounded_utf8_and_never_executes_a_command() {
    let mut app = App::new_headless(Config::default()).unwrap();
    app.command_palette.open();
    app.handle_accessibility_action(ActionRequest {
        action: Action::SetValue,
        target_tree: TreeId::ROOT,
        target_node: PALETTE_INPUT,
        data: Some(ActionData::Value("toggle fullscreen 🫚".into())),
    });
    assert_eq!(
        app.command_palette.build_info().input,
        "toggle fullscreen 🫚"
    );
    assert!(app.command_palette.is_active());
    assert!(!app.fullscreen_requested);

    app.handle_accessibility_action(ActionRequest {
        action: Action::SetValue,
        target_tree: TreeId::ROOT,
        target_node: PALETTE_INPUT,
        data: Some(ActionData::Value("bad\nfilter".into())),
    });
    assert_eq!(
        app.command_palette.build_info().input,
        "toggle fullscreen 🫚"
    );
    assert!(!app.fullscreen_requested);
}

#[test]
fn settings_is_one_modal_tree_with_typed_controls_and_shared_geometry() {
    use crate::ui::settings::{SettingsPage, SettingsSection};

    let mut app = App::new_headless(Config::default()).unwrap();
    app.settings_panel.toggle();
    app.settings_panel.select_page(SettingsPage::Font);
    let info = app
        .settings_panel
        .build_info(&app.config, &app.shortcut_registry);
    let layout = app.settings_accessibility_layout(&info);
    let snapshot = app.accessibility_snapshot();
    let node = |id| {
        &snapshot
            .nodes
            .iter()
            .find(|(node_id, _)| *node_id == id)
            .unwrap()
            .1
    };

    assert_eq!(node(ROOT).children(), &[SETTINGS_DIALOG]);
    assert_eq!(node(SETTINGS_DIALOG).role(), Role::Dialog);
    assert!(node(SETTINGS_DIALOG).is_modal());
    assert_eq!(node(SETTINGS_INPUT).role(), Role::SearchInput);
    assert_eq!(node(SETTINGS_INPUT).controls(), &[SETTINGS_LIST]);
    assert_eq!(node(SETTINGS_LIST).role(), Role::List);
    assert_eq!(node(SETTINGS_LIST).label(), Some("Font"));
    assert_eq!(snapshot.focus, SETTINGS_INPUT);
    assert_eq!(node(SETTINGS_PAGES).role(), Role::TabList);
    let pages = node(SETTINGS_PAGES).children();
    assert_eq!(pages.len(), SettingsPage::all().len());
    let font_tab =
        app.accessibility.settings.nodes[&SettingsSemanticTarget::Page(SettingsPage::Font)];
    assert_eq!(node(font_tab).role(), Role::Tab);
    assert_eq!(node(font_tab).is_selected(), Some(true));
    assert_eq!(node(SETTINGS_CLOSE).label(), Some("Close settings"));
    assert_eq!(node(SETTINGS_SAVE).role(), Role::Button);

    let size = app
        .accessibility
        .settings
        .nodes
        .get(&SettingsSemanticTarget::Setting {
            section: SettingsSection::Font,
            label: "Size",
        })
        .copied()
        .unwrap();
    assert_eq!(node(size).role(), Role::SpinButton);
    assert!(node(size).supports_action(Action::Increment));
    assert_eq!(node(SETTINGS_INPUT).active_descendant(), Some(size));
    let normal_weight = app
        .accessibility
        .settings
        .nodes
        .get(&SettingsSemanticTarget::Setting {
            section: SettingsSection::Font,
            label: "Normal Weight",
        })
        .copied()
        .unwrap();
    assert_eq!(node(normal_weight).role(), Role::SpinButton);
    assert_eq!(node(normal_weight).numeric_value(), Some(400.0));
    assert!(node(normal_weight).supports_action(Action::Increment));
    let actual = node(size).bounds().unwrap();
    let dialog = node(SETTINGS_DIALOG).bounds().unwrap();
    // Size is the Font page's first row, in the content column.
    let row = layout.row_rect(0);
    let expected = rect(
        dialog.x0 as f32 + (row.x - layout.panel.x),
        dialog.y0 as f32 + (row.y - layout.panel.y),
        row.width,
        row.height,
    );
    for (actual, expected) in [
        (actual.x0, expected.x0),
        (actual.y0, expected.y0),
        (actual.x1, expected.x1),
        (actual.y1, expected.y1),
    ] {
        assert!((actual - expected).abs() < 0.05, "{actual} != {expected}");
    }
}

#[test]
fn settings_accessibility_actions_use_the_existing_validated_action_path() {
    use crate::ui::settings::SettingsSection;

    let mut app = App::new_headless(Config::default()).unwrap();
    app.settings_panel.toggle();
    assert!(app.settings_panel.accessibility_focus_setting(
        SettingsSection::Window,
        "Opacity",
        &app.config,
        &app.shortcut_registry,
    ));
    app.accessibility_snapshot();
    let opacity = *app
        .accessibility
        .settings
        .nodes
        .get(&SettingsSemanticTarget::Setting {
            section: SettingsSection::Window,
            label: "Opacity",
        })
        .unwrap();
    let before = app.config.window.opacity;
    app.handle_accessibility_action(ActionRequest {
        action: Action::Decrement,
        target_tree: TreeId::ROOT,
        target_node: opacity,
        data: None,
    });
    assert!((app.config.window.opacity - (before - 0.05)).abs() < f32::EPSILON);

    app.settings_panel.open_to_section(SettingsSection::Font);
    assert!(app.settings_panel.accessibility_focus_setting(
        SettingsSection::Font,
        "Normal Weight",
        &app.config,
        &app.shortcut_registry,
    ));
    app.accessibility_snapshot();
    let weight = *app
        .accessibility
        .settings
        .nodes
        .get(&SettingsSemanticTarget::Setting {
            section: SettingsSection::Font,
            label: "Normal Weight",
        })
        .unwrap();
    app.handle_accessibility_action(ActionRequest {
        action: Action::Increment,
        target_tree: TreeId::ROOT,
        target_node: weight,
        data: None,
    });
    assert_eq!(app.config.font.weight, 425);

    app.settings_panel
        .open_to_section(SettingsSection::Sessions);
    app.accessibility_snapshot();
    let auto_save = *app
        .accessibility
        .settings
        .nodes
        .get(&SettingsSemanticTarget::Setting {
            section: SettingsSection::Sessions,
            label: "Auto Save Workspace",
        })
        .unwrap();
    app.handle_accessibility_action(ActionRequest {
        action: Action::Expand,
        target_tree: TreeId::ROOT,
        target_node: auto_save,
        data: None,
    });
    app.accessibility_snapshot();
    let expected = !app.config.session.auto_save;
    let value = if expected { "On" } else { "Off" };
    let option = *app
        .accessibility
        .settings
        .nodes
        .get(&SettingsSemanticTarget::Option {
            section: SettingsSection::Sessions,
            label: "Auto Save Workspace",
            value: value.into(),
        })
        .unwrap();
    app.handle_accessibility_action(ActionRequest {
        action: Action::Click,
        target_tree: TreeId::ROOT,
        target_node: option,
        data: None,
    });
    assert_eq!(app.config.session.auto_save, expected);
    assert!(!app.settings_panel.is_dropdown_open());
}

#[test]
fn runtime_plugin_manager_setting_is_an_accessible_button() {
    use crate::ui::settings::SettingsSection;

    let mut app = App::new_headless(Config::default()).unwrap();
    app.settings_panel.toggle();
    app.settings_panel
        .select_page(SettingsSection::Plugins.page());
    assert!(app.settings_panel.accessibility_focus_setting(
        SettingsSection::Plugins,
        "Manage Third-party Plugins",
        &app.config,
        &app.shortcut_registry,
    ));
    let snapshot = app.accessibility_snapshot();
    let manager = *app
        .accessibility
        .settings
        .nodes
        .get(&SettingsSemanticTarget::Setting {
            section: SettingsSection::Plugins,
            label: "Manage Third-party Plugins",
        })
        .unwrap();
    let node = &snapshot
        .nodes
        .iter()
        .find(|(node_id, _)| *node_id == manager)
        .unwrap()
        .1;
    assert_eq!(node.role(), Role::Button);
    assert!(node.supports_action(Action::Click));

    app.handle_accessibility_action(ActionRequest {
        action: Action::Click,
        target_tree: TreeId::ROOT,
        target_node: manager,
        data: None,
    });
    assert!(!app.settings_panel.is_active());
    assert!(app.info_overlay.is_awaiting_current());
    assert_eq!(app.info_overlay.title(), "Third-party Plugin Manager");
}

#[test]
fn font_range_manager_setting_is_an_accessible_button() {
    use crate::ui::settings::SettingsSection;

    let mut app = App::new_headless(Config::default()).unwrap();
    app.settings_panel.toggle();
    app.settings_panel.select_page(SettingsSection::Font.page());
    assert!(app.settings_panel.accessibility_focus_setting(
        SettingsSection::Font,
        "Range Overrides",
        &app.config,
        &app.shortcut_registry,
    ));
    let snapshot = app.accessibility_snapshot();
    let manager = *app
        .accessibility
        .settings
        .nodes
        .get(&SettingsSemanticTarget::Setting {
            section: SettingsSection::Font,
            label: "Range Overrides",
        })
        .unwrap();
    let node = &snapshot
        .nodes
        .iter()
        .find(|(node_id, _)| *node_id == manager)
        .unwrap()
        .1;
    assert_eq!(node.role(), Role::Button);
    assert!(node.supports_action(Action::Click));

    app.handle_accessibility_action(ActionRequest {
        action: Action::Click,
        target_tree: TreeId::ROOT,
        target_node: manager,
        data: None,
    });
    assert!(!app.settings_panel.is_active());
    assert!(app.info_overlay.is_awaiting_current());
    assert_eq!(app.info_overlay.title(), "Font Range Overrides");

    let manager_request = app.info_overlay.request_id();
    app.accessibility_snapshot();
    let inspect = *app
        .accessibility
        .info
        .nodes
        .get(&InfoSemanticTarget::Choice {
            request_id: manager_request,
            id: "inspect".into(),
        })
        .unwrap();
    app.handle_accessibility_action(ActionRequest {
        action: Action::Click,
        target_tree: TreeId::ROOT,
        target_node: inspect,
        data: None,
    });
    assert_eq!(app.info_overlay.title(), "Inspect Glyph Coverage");

    let input_request = app.info_overlay.request_id();
    let snapshot = app.accessibility_snapshot();
    let input = *app
        .accessibility
        .info
        .nodes
        .get(&InfoSemanticTarget::Input(input_request))
        .unwrap();
    let submit = *app
        .accessibility
        .info
        .nodes
        .get(&InfoSemanticTarget::Dismiss(input_request))
        .unwrap();
    let input_node = &snapshot
        .nodes
        .iter()
        .find(|(node_id, _)| *node_id == input)
        .unwrap()
        .1;
    assert_eq!(input_node.role(), Role::TextInput);
    assert_eq!(input_node.label(), Some("Text"));
    app.handle_accessibility_action(ActionRequest {
        action: Action::SetValue,
        target_tree: TreeId::ROOT,
        target_node: input,
        data: Some(ActionData::Value("U+0634".into())),
    });
    app.handle_accessibility_action(ActionRequest {
        action: Action::Click,
        target_tree: TreeId::ROOT,
        target_node: submit,
        data: None,
    });
    assert_eq!(app.info_overlay.title(), "Glyph Coverage Report");
}

#[test]
fn font_feature_manager_is_accessible_and_uses_the_shared_mutation_path() {
    use crate::ui::settings::SettingsSection;

    let mut app = App::new_headless(Config::default()).unwrap();
    app.settings_panel.toggle();
    app.settings_panel.select_page(SettingsSection::Font.page());
    assert!(app.settings_panel.accessibility_focus_setting(
        SettingsSection::Font,
        "OpenType Features",
        &app.config,
        &app.shortcut_registry,
    ));
    let snapshot = app.accessibility_snapshot();
    let manager = *app
        .accessibility
        .settings
        .nodes
        .get(&SettingsSemanticTarget::Setting {
            section: SettingsSection::Font,
            label: "OpenType Features",
        })
        .unwrap();
    let node = &snapshot
        .nodes
        .iter()
        .find(|(node_id, _)| *node_id == manager)
        .unwrap()
        .1;
    assert_eq!(node.role(), Role::Button);
    app.handle_accessibility_action(ActionRequest {
        action: Action::Click,
        target_tree: TreeId::ROOT,
        target_node: manager,
        data: None,
    });
    assert_eq!(app.info_overlay.title(), "OpenType Feature Overrides");

    let manager_request = app.info_overlay.request_id();
    app.accessibility_snapshot();
    let add = *app
        .accessibility
        .info
        .nodes
        .get(&InfoSemanticTarget::Choice {
            request_id: manager_request,
            id: "add".into(),
        })
        .unwrap();
    app.handle_accessibility_action(ActionRequest {
        action: Action::Click,
        target_tree: TreeId::ROOT,
        target_node: add,
        data: None,
    });

    let input_request = app.info_overlay.request_id();
    let snapshot = app.accessibility_snapshot();
    let input = *app
        .accessibility
        .info
        .nodes
        .get(&InfoSemanticTarget::Input(input_request))
        .unwrap();
    let submit = *app
        .accessibility
        .info
        .nodes
        .get(&InfoSemanticTarget::Dismiss(input_request))
        .unwrap();
    let input_node = &snapshot
        .nodes
        .iter()
        .find(|(node_id, _)| *node_id == input)
        .unwrap()
        .1;
    assert_eq!(input_node.role(), Role::TextInput);
    assert_eq!(input_node.label(), Some("Feature"));
    app.handle_accessibility_action(ActionRequest {
        action: Action::SetValue,
        target_tree: TreeId::ROOT,
        target_node: input,
        data: Some(ActionData::Value("ss01 = 1".into())),
    });
    app.handle_accessibility_action(ActionRequest {
        action: Action::Click,
        target_tree: TreeId::ROOT,
        target_node: submit,
        data: None,
    });
    assert_eq!(app.config.font.features.get("ss01"), Some(&1));
    assert_eq!(app.info_overlay.title(), "OpenType Feature Overrides");
}

#[test]
fn settings_filter_semantic_replacement_updates_text_caret_and_results_only() {
    let mut app = App::new_headless(Config::default()).unwrap();
    app.settings_panel.toggle();
    let original = toml::to_string(&app.config).unwrap();
    let snapshot = app.accessibility_snapshot();
    let input = &snapshot
        .nodes
        .iter()
        .find(|(id, _)| *id == SETTINGS_INPUT)
        .unwrap()
        .1;
    assert!(input.supports_action(Action::SetValue));
    assert!(!input.is_read_only());
    for text in ["size", "e\u{301}👩\u{200d}💻", "", &"🫚".repeat(256)] {
        app.handle_ime_event(winit::event::Ime::Preedit("draft".into(), None));
        app.handle_accessibility_action(ActionRequest {
            action: Action::SetValue,
            target_tree: TreeId::ROOT,
            target_node: SETTINGS_INPUT,
            data: Some(ActionData::Value(text.into())),
        });
        let snapshot = app.accessibility_snapshot();
        let input = &snapshot
            .nodes
            .iter()
            .find(|(id, _)| *id == SETTINGS_INPUT)
            .unwrap()
            .1;
        assert_eq!(input.value(), Some(text));
        assert_eq!(
            input.text_selection().unwrap().focus.character_index,
            text.chars().count()
        );
        assert_eq!(
            input
                .character_lengths()
                .iter()
                .map(|size| usize::from(*size))
                .sum::<usize>(),
            text.len()
        );
        assert_eq!(snapshot.focus, SETTINGS_INPUT);
        assert!(app.ime.preedit().is_none());
        assert!(app.redraw_scheduler.has_pending());
        assert_eq!(toml::to_string(&app.config).unwrap(), original);
        if text == "size" {
            assert!(
                snapshot.nodes.iter().any(
                    |(_, node)| node.label() == Some("Size") && node.role() == Role::SpinButton
                )
            );
        }
    }
}

#[test]
fn settings_filter_semantic_rejection_preserves_state_and_dropdown_focus() {
    use crate::ui::settings::SettingsKey;
    let mut app = App::new_headless(Config::default()).unwrap();
    app.settings_panel.toggle();
    assert!(app.settings_panel.accessibility_set_filter("family"));
    let original = toml::to_string(&app.config).unwrap();
    app.handle_ime_event(winit::event::Ime::Preedit("draft".into(), None));
    for data in [
        None,
        Some(ActionData::Value("bad\nquery".into())),
        Some(ActionData::Value("x".repeat(257).into())),
    ] {
        app.handle_accessibility_action(ActionRequest {
            action: Action::SetValue,
            target_tree: TreeId::ROOT,
            target_node: SETTINGS_INPUT,
            data,
        });
        assert!(app.ime.preedit().is_some());
        assert_eq!(
            app.settings_panel
                .build_info(&app.config, &app.shortcut_registry)
                .filter,
            "family"
        );
    }
    app.accessibility.focus_override = Some(SETTINGS_INPUT);
    app.settings_panel
        .handle_key(SettingsKey::Right, &app.config, &app.shortcut_registry);
    assert!(app.settings_panel.is_dropdown_open());
    let snapshot = app.accessibility_snapshot();
    let input = &snapshot
        .nodes
        .iter()
        .find(|(id, _)| *id == SETTINGS_INPUT)
        .unwrap()
        .1;
    assert!(input.is_read_only());
    assert!(!input.supports_action(Action::SetValue));
    assert!(!input.supports_action(Action::Focus));
    let focus = snapshot.focus;
    assert_ne!(focus, SETTINGS_INPUT);
    assert_eq!(
        snapshot
            .nodes
            .iter()
            .find(|(id, _)| *id == focus)
            .unwrap()
            .1
            .is_expanded(),
        Some(true)
    );
    for action in [Action::Focus, Action::SetValue] {
        app.handle_accessibility_action(ActionRequest {
            action,
            target_tree: TreeId::ROOT,
            target_node: SETTINGS_INPUT,
            data: Some(ActionData::Value("size".into())),
        });
        assert_eq!(app.accessibility_snapshot().focus, focus);
        assert_eq!(
            app.settings_panel
                .build_info(&app.config, &app.shortcut_registry)
                .filter,
            "family"
        );
    }
    app.settings_panel
        .handle_key(SettingsKey::Escape, &app.config, &app.shortcut_registry);
    let snapshot = app.accessibility_snapshot();
    assert!(
        snapshot
            .nodes
            .iter()
            .find(|(id, _)| *id == SETTINGS_INPUT)
            .unwrap()
            .1
            .supports_action(Action::SetValue)
    );
    assert_eq!(toml::to_string(&app.config).unwrap(), original);
}

#[test]
fn settings_filter_semantic_requests_cannot_edit_hidden_or_covered_fields() {
    let mut app = App::new_headless(Config::default()).unwrap();
    app.settings_panel.toggle();
    assert!(app.settings_panel.accessibility_set_filter("size"));
    app.command_palette.open();
    for covered in [true, false] {
        if !covered {
            app.command_palette.close();
            app.settings_panel.close();
        }
        app.handle_accessibility_action(ActionRequest {
            action: Action::SetValue,
            target_tree: TreeId::ROOT,
            target_node: SETTINGS_INPUT,
            data: Some(ActionData::Value("hidden".into())),
        });
        assert_eq!(
            app.settings_panel
                .build_info(&app.config, &app.shortcut_registry)
                .filter,
            "size"
        );
        assert!(app.command_palette.build_info().input.is_empty());
    }
}

#[test]
fn settings_filter_reports_utf8_cursor_and_empty_results() {
    let mut app = App::new_headless(Config::default()).unwrap();
    app.settings_panel.toggle();
    assert!(app.settings_panel.insert_filter_text("🫚absent"));
    let snapshot = app.accessibility_snapshot();
    let input = &snapshot
        .nodes
        .iter()
        .find(|(id, _)| *id == SETTINGS_INPUT)
        .unwrap()
        .1;
    assert_eq!(input.character_lengths()[0], 4);
    assert_eq!(
        input.text_selection().unwrap().focus.character_index,
        "🫚absent".chars().count()
    );
    assert!(snapshot.nodes.iter().any(
        |(id, node)| *id == SETTINGS_EMPTY && node.label() == Some("No matching settings")
    ));
}

#[test]
fn command_deletion_unifies_focus_and_retires_stale_duplicate_actions() {
    use crate::ui::commands::panel::CommandsPanelAction;
    use winit::keyboard::{Key, NamedKey};
    for (route, editing) in [
        ("pointer", false),
        ("semantic", false),
        ("keyboard", false),
        ("pointer", true),
        ("semantic", true),
        ("pointer-last", false),
    ] {
        let mut app = App::new_headless(Config::default()).unwrap();
        // Deleting a command saves the configuration, to a file of this
        // case's own.
        let directory = tempfile::tempdir().unwrap();
        app.config_path = Some(directory.path().join("config.toml"));
        let duplicate = crate::config::CustomCommand {
            name: "same".into(),
            command: "inert: never run".into(),
            alias: None,
        };
        app.config.commands = vec![
            duplicate.clone(),
            duplicate,
            crate::config::CustomCommand {
                name: "other".into(),
                command: "inert: never run either".into(),
                alias: None,
            },
        ];
        if route == "pointer-last" {
            app.config.commands.truncate(1);
        }
        let remaining = app.config.commands.len() - 1;
        app.commands_panel.set_reduced_motion(true);
        app.commands_panel.toggle();
        app.accessibility_snapshot();
        let key = custom_command_keys(&app.config.commands).remove(0);
        let stale_delete =
            app.accessibility.commands.nodes[&CommandsSemanticTarget::Delete(key.clone())];
        let run = app.accessibility.commands.nodes[&CommandsSemanticTarget::Run(key)];
        app.handle_accessibility_action(ActionRequest {
            action: Action::Focus,
            target_tree: TreeId::ROOT,
            target_node: if route == "keyboard" {
                stale_delete
            } else {
                run
            },
            data: None,
        });
        if editing {
            app.commands_panel.start_add();
            assert!(app.commands_panel.form_name.replace("retained name"));
            assert!(app.commands_panel.form_command.replace("retained command"));
            app.commands_panel.focus_command();
        }
        // A refused index must not revoke still-valid row actions.
        app.apply_commands_panel_action(CommandsPanelAction::Delete(usize::MAX));
        assert_eq!(app.config.commands.len(), remaining + 1);
        assert!(app.accessibility.commands.targets.contains_key(&run));
        match route {
            "pointer" | "pointer-last" => {
                app.apply_commands_panel_action(CommandsPanelAction::Delete(0))
            }
            "semantic" => app.handle_accessibility_action(ActionRequest {
                action: Action::Click,
                target_tree: TreeId::ROOT,
                target_node: stale_delete,
                data: None,
            }),
            _ => {
                assert!(app.handle_commands_panel_key(&Key::Named(NamedKey::Enter)));
            }
        }
        assert_eq!(app.config.commands.len(), remaining, "{route}");
        assert_eq!(
            app.commands_panel
                .build_info(&app.config.commands)
                .browse_focus,
            (!editing).then_some(CommandControl::New),
            "{route}: keyboard and rendered focus must agree"
        );
        // Queued events must not hit the identical command that shifted into
        // occurrence zero, either before or after the next semantic frame.
        for rebuild in [false, true] {
            if rebuild {
                assert_eq!(
                    app.accessibility_snapshot().focus,
                    if editing {
                        COMMANDS_COMMAND
                    } else {
                        COMMANDS_NEW
                    }
                );
            }
            app.handle_accessibility_action(ActionRequest {
                action: Action::Click,
                target_tree: TreeId::ROOT,
                target_node: stale_delete,
                data: None,
            });
            assert_eq!(
                app.config.commands.len(),
                remaining,
                "{route}: stale Delete retargeted a duplicate"
            );
            for action in [Action::Click, Action::Focus] {
                app.handle_accessibility_action(ActionRequest {
                    action,
                    target_tree: TreeId::ROOT,
                    target_node: run,
                    data: None,
                });
                assert!(app.commands_panel.is_open(), "{route}: stale Run activated");
            }
        }
        let persisted = Config::load_from(app.config_path.as_deref().unwrap()).unwrap();
        assert_eq!(persisted.commands.len(), remaining);
        if editing {
            assert_eq!(app.commands_panel.form_name.text(), "retained name");
            assert_eq!(app.commands_panel.form_command.text(), "retained command");
            assert!(app.handle_commands_panel_key(&Key::Character("!".into())));
            assert_eq!(app.commands_panel.form_command.text(), "retained command!");
        } else {
            assert!(app.handle_commands_panel_key(&Key::Named(NamedKey::Enter)));
        }
        assert!(
            app.commands_panel.is_editing(),
            "{route}: Enter should open New, not run or delete a command"
        );
        assert!(app.commands_panel.is_open());
        assert_eq!(app.config.commands.len(), remaining);
    }
}

#[test]
fn command_reload_retires_ambiguous_duplicates_before_queued_actions() {
    use winit::keyboard::{Key, NamedKey};
    for editing in [false, true] {
        let mut app = App::new_headless(Config::default()).unwrap();
        let duplicate = crate::config::CustomCommand {
            name: "same".into(),
            command: "inert: never run".into(),
            alias: None,
        };
        app.config.commands = vec![duplicate.clone(), duplicate];
        app.commands_panel.set_reduced_motion(true);
        app.commands_panel.toggle();
        app.accessibility_snapshot();
        let key = custom_command_keys(&app.config.commands).remove(0);
        let run = app.accessibility.commands.nodes[&CommandsSemanticTarget::Run(key.clone())];
        let delete = app.accessibility.commands.nodes[&CommandsSemanticTarget::Delete(key)];
        app.handle_accessibility_action(ActionRequest {
            action: Action::Focus,
            target_tree: TreeId::ROOT,
            target_node: run,
            data: None,
        });
        if editing {
            app.commands_panel.start_add();
            assert!(app.commands_panel.form_name.replace("retained"));
            assert!(app.commands_panel.form_command.replace("retained command"));
            app.commands_panel.focus_command();
        }
        let mut replacement = app.config.clone();
        replacement.commands.remove(0);
        app.apply_config(replacement).unwrap();
        // Assert revocation before dispatch: a failing regression must never
        // invoke Delete's real configuration persistence path.
        assert!(
            !app.accessibility.commands.targets.contains_key(&delete),
            "reload retained a stale duplicate Delete identity"
        );
        assert!(!app.accessibility.commands.targets.contains_key(&run));
        assert_eq!(
            app.commands_panel
                .build_info(&app.config.commands)
                .browse_focus,
            (!editing).then_some(CommandControl::New)
        );
        for rebuild in [false, true] {
            if rebuild {
                app.accessibility_snapshot();
            }
            for target_node in [run, delete] {
                assert!(
                    !app.accessibility
                        .commands
                        .targets
                        .contains_key(&target_node)
                );
                for action in [Action::Click, Action::Focus] {
                    app.handle_accessibility_action(ActionRequest {
                        action,
                        target_tree: TreeId::ROOT,
                        target_node,
                        data: None,
                    });
                }
            }
            assert!(app.commands_panel.is_open());
            assert_eq!(app.config.commands.len(), 1);
        }
        assert_eq!(
            app.accessibility_snapshot().focus,
            if editing {
                COMMANDS_COMMAND
            } else {
                COMMANDS_NEW
            }
        );
        if editing {
            assert_eq!(app.commands_panel.form_name.text(), "retained");
            assert_eq!(app.commands_panel.form_command.text(), "retained command");
        } else {
            assert!(app.handle_commands_panel_key(&Key::Named(NamedKey::Enter)));
            assert!(app.commands_panel.is_editing());
        }
    }
}

#[test]
fn command_reload_preserves_unambiguous_ids_focus_and_noop_inventory() {
    let mut app = App::new_headless(Config::default()).unwrap();
    let duplicate = crate::config::CustomCommand {
        name: "same".into(),
        command: "inert".into(),
        alias: None,
    };
    let unique = crate::config::CustomCommand {
        name: "unique".into(),
        command: "never run".into(),
        alias: Some("stable".into()),
    };
    app.config.commands = vec![duplicate.clone(), duplicate.clone(), unique.clone()];
    app.commands_panel.set_reduced_motion(true);
    app.commands_panel.toggle();
    app.accessibility_snapshot();
    let keys = custom_command_keys(&app.config.commands);
    let stable = app.accessibility.commands.nodes[&CommandsSemanticTarget::Run(keys[2].clone())];
    let ambiguous = app.accessibility.commands.nodes[&CommandsSemanticTarget::Run(keys[0].clone())];
    app.handle_accessibility_action(ActionRequest {
        action: Action::Focus,
        target_tree: TreeId::ROOT,
        target_node: stable,
        data: None,
    });
    let mut replacement = app.config.clone();
    replacement.commands = vec![unique, duplicate];
    app.apply_config(replacement).unwrap();
    assert_eq!(
        app.commands_panel.browse_focus(&app.config.commands),
        Some(CommandControl::Run(0))
    );
    assert!(app.accessibility.commands.targets.contains_key(&stable));
    assert!(!app.accessibility.commands.targets.contains_key(&ambiguous));
    assert_eq!(app.accessibility_snapshot().focus, stable);
    let nodes = app.accessibility.commands.nodes.clone();
    app.apply_config(app.config.clone()).unwrap();
    assert_eq!(app.accessibility_snapshot().focus, stable);
    assert_eq!(app.accessibility.commands.nodes, nodes);
}

#[test]
fn rejected_command_reload_does_not_revoke_current_focus_or_actions() {
    let mut app = App::new_headless(Config::default()).unwrap();
    let duplicate = crate::config::CustomCommand {
        name: "same".into(),
        command: "inert".into(),
        alias: None,
    };
    app.config.commands = vec![duplicate.clone(), duplicate];
    app.commands_panel.set_reduced_motion(true);
    app.commands_panel.toggle();
    app.accessibility_snapshot();
    let key = custom_command_keys(&app.config.commands).remove(0);
    let run = app.accessibility.commands.nodes[&CommandsSemanticTarget::Run(key)];
    app.handle_accessibility_action(ActionRequest {
        action: Action::Focus,
        target_tree: TreeId::ROOT,
        target_node: run,
        data: None,
    });
    let nodes = app.accessibility.commands.nodes.clone();
    let mut invalid = app.config.clone();
    invalid.commands.remove(0);
    invalid.font.size = 0.0;
    assert!(app.apply_config(invalid).is_err());
    assert_eq!(app.config.commands.len(), 2);
    assert_eq!(
        app.commands_panel.browse_focus(&app.config.commands),
        Some(CommandControl::Run(0))
    );
    assert_eq!(app.accessibility_snapshot().focus, run);
    assert_eq!(app.accessibility.commands.nodes, nodes);
}

#[test]
fn command_keyboard_and_assistive_focus_share_controls_and_form_transitions() {
    use winit::keyboard::{Key, NamedKey};
    let mut app = App::new_headless(Config::default()).unwrap();
    app.config.commands = vec![crate::config::CustomCommand {
        name: "inert".into(),
        command: "never run".into(),
        alias: None,
    }];
    app.commands_panel.set_reduced_motion(true);
    app.commands_panel.toggle();
    app.accessibility_snapshot();
    let key = custom_command_keys(&app.config.commands).remove(0);
    let run = app.accessibility.commands.nodes[&CommandsSemanticTarget::Run(key.clone())];
    let delete = app.accessibility.commands.nodes[&CommandsSemanticTarget::Delete(key)];
    app.handle_accessibility_action(ActionRequest {
        action: Action::Focus,
        target_tree: TreeId::ROOT,
        target_node: run,
        data: None,
    });
    assert_eq!(
        app.commands_panel
            .build_info(&app.config.commands)
            .browse_focus,
        Some(CommandControl::Run(0))
    );
    assert!(app.handle_commands_panel_key(&Key::Named(NamedKey::Tab)));
    assert_eq!(app.accessibility_snapshot().focus, delete);
    assert_eq!(
        app.commands_panel
            .build_info(&app.config.commands)
            .browse_focus,
        Some(CommandControl::Delete(0))
    );
    app.handle_accessibility_action(ActionRequest {
        action: Action::Focus,
        target_tree: TreeId::ROOT,
        target_node: COMMANDS_CLOSE,
        data: None,
    });
    assert_eq!(
        app.commands_panel.browse_focus(&app.config.commands),
        Some(CommandControl::Close)
    );
    assert!(app.handle_commands_panel_key(&Key::Named(NamedKey::Home)));
    assert_eq!(app.accessibility_snapshot().focus, COMMANDS_NEW);
    assert!(app.handle_commands_panel_key(&Key::Named(NamedKey::Enter)));
    assert_eq!(app.accessibility_snapshot().focus, COMMANDS_NAME);
    app.handle_accessibility_action(ActionRequest {
        action: Action::Focus,
        target_tree: TreeId::ROOT,
        target_node: COMMANDS_NAME,
        data: None,
    });
    assert!(app.handle_commands_panel_key(&Key::Named(NamedKey::Tab)));
    assert_eq!(app.accessibility_snapshot().focus, COMMANDS_COMMAND);
    assert!(app.handle_commands_panel_key(&Key::Named(NamedKey::Escape)));
    assert_eq!(app.accessibility_snapshot().focus, COMMANDS_NEW);
    assert!(app.handle_commands_panel_key(&Key::Named(NamedKey::End)));
    assert!(app.handle_commands_panel_key(&Key::Named(NamedKey::Space)));
    assert!(!app.commands_panel.is_open());
    assert_eq!(app.config.commands.len(), 1);
}

#[test]
fn command_semantic_focus_reveals_exact_offscreen_rows_without_execution() {
    let mut app = App::new_headless(Config::default()).unwrap();
    app.config.commands = (0..100)
        .map(|index| crate::config::CustomCommand {
            name: format!("Command {index}"),
            command: "inert: never run".into(),
            alias: None,
        })
        .collect();
    app.commands_panel.set_reduced_motion(true);
    app.commands_panel.toggle();
    let keys = custom_command_keys(&app.config.commands);
    let before = app.accessibility_snapshot();
    let last = app.accessibility.commands.nodes[&CommandsSemanticTarget::Run(keys[99].clone())];
    let bounds = |snapshot: &SemanticSnapshot, id| {
        snapshot
            .nodes
            .iter()
            .find(|(node, _)| *node == id)
            .unwrap()
            .1
            .bounds()
    };
    assert_eq!(bounds(&before, last), None);
    assert!(
        before
            .nodes
            .iter()
            .any(|(id, node)| *id == COMMANDS_LIST && node.supports_action(Action::ScrollDown))
    );
    app.handle_accessibility_action(ActionRequest {
        action: Action::Focus,
        target_tree: TreeId::ROOT,
        target_node: last,
        data: None,
    });
    let after = app.accessibility_snapshot();
    assert_eq!(after.focus, last);
    let layout = app.commands_panel_layout();
    assert_eq!(layout.visible_range().end, 100);
    assert_eq!(
        bounds(&after, last),
        Some(bounds_rect(layout.command_run(99)))
    );
    assert!(app.commands_panel.is_open());
    assert_eq!(app.config.commands.len(), 100);
    app.handle_accessibility_action(ActionRequest {
        action: Action::ScrollUp,
        target_tree: TreeId::ROOT,
        target_node: COMMANDS_LIST,
        data: None,
    });
    assert_eq!(
        app.commands_panel_layout().scroll_offset,
        layout.scroll_offset - 1
    );
    app.handle_accessibility_action(ActionRequest {
        action: Action::ScrollDown,
        target_tree: TreeId::ROOT,
        target_node: COMMANDS_LIST,
        data: None,
    });
    assert_eq!(
        app.commands_panel_layout().scroll_offset,
        layout.scroll_offset
    );
    // Config replacement must not turn the old row's focus into another command.
    app.config.commands[99].name = "replacement".into();
    app.scroll_commands_panel(isize::MIN);
    app.handle_accessibility_action(ActionRequest {
        action: Action::Focus,
        target_tree: TreeId::ROOT,
        target_node: last,
        data: None,
    });
    assert_eq!(app.commands_panel_layout().scroll_offset, 0);
    let fresh = app.accessibility_snapshot();
    assert!(!fresh.nodes.iter().any(|(id, _)| *id == last));
    assert!(app.commands_panel.is_open());
    assert_eq!(app.config.commands.len(), 100);
}

#[test]
fn agents_panel_is_one_modal_tree_whose_rows_show_their_agent() {
    let (mut app, codex_tab, _) = crate::app::tests::two_agent_tabs();
    app.agents_panel.set_reduced_motion(true);
    app.toggle_agents_panel();
    let snapshot = app.accessibility_snapshot();
    let node = |id| {
        snapshot
            .nodes
            .iter()
            .find(|(node_id, _)| *node_id == id)
            .map(|(_, node)| node.clone())
            .unwrap()
    };
    assert_eq!(node(ROOT).children(), &[AGENTS_DIALOG]);
    assert!(node(AGENTS_DIALOG).is_modal());
    assert_eq!(node(AGENTS_LIST).role(), Role::List);
    let rows = node(AGENTS_LIST).children().to_vec();
    assert_eq!(rows.len(), 2);
    assert_eq!(snapshot.focus, rows[0], "the most urgent agent has focus");
    let first = node(rows[0]);
    assert_eq!(first.role(), Role::Button);
    assert!(first.label().unwrap().starts_with("Claude Code, "));
    assert!(first.description().unwrap().contains("Approve shell"));
    let layout = app.agents_panel_layout();
    let bounds = node(rows[1]).bounds().unwrap();
    assert!((bounds.y0 - f64::from(layout.row(1).y)).abs() < 0.05);

    // A request for anything outside the modal panel is ignored.
    app.handle_accessibility_action(ActionRequest {
        action: Action::Click,
        target_tree: TreeId::ROOT,
        target_node: COMMANDS_NEW,
        data: None,
    });
    assert!(app.agents_panel.is_open());
    assert!(!app.commands_panel.is_open());

    // Clicking a row shows its agent and closes the panel.
    app.handle_accessibility_action(ActionRequest {
        action: Action::Click,
        target_tree: TreeId::ROOT,
        target_node: rows[1],
        data: None,
    });
    assert!(!app.agents_panel.is_open());
    assert_eq!(app.tabs.as_ref().unwrap().active_tab_id(), Some(codex_tab));
}

#[test]
fn custom_commands_is_one_modal_tree_with_shared_action_geometry() {
    use crate::config::CustomCommand;

    let mut app = App::new_headless(Config::default()).unwrap();
    app.config.commands = vec![CustomCommand {
        name: "Inspect logs".into(),
        command: "printf local-fixture".into(),
        alias: Some("logs".into()),
    }];
    // Geometry equality is the subject of this test. Freeze the optional
    // slide transition so the independently built expected layout and the
    // accessibility snapshot sample the same panel position.
    app.commands_panel.set_reduced_motion(true);
    app.commands_panel.toggle();
    let layout = app.commands_panel_layout();
    let snapshot = app.accessibility_snapshot();
    let node = |id| {
        &snapshot
            .nodes
            .iter()
            .find(|(node_id, _)| *node_id == id)
            .unwrap()
            .1
    };
    assert_eq!(node(ROOT).children(), &[COMMANDS_DIALOG]);
    assert!(node(COMMANDS_DIALOG).is_modal());
    assert_eq!(node(COMMANDS_NEW).role(), Role::Button);
    assert_eq!(node(COMMANDS_CLOSE).role(), Role::Button);
    assert_eq!(node(COMMANDS_LIST).role(), Role::List);
    assert_eq!(snapshot.focus, COMMANDS_NEW);

    let key = custom_command_keys(&app.config.commands).remove(0);
    let group = *app
        .accessibility
        .commands
        .nodes
        .get(&CommandsSemanticTarget::Group(key.clone()))
        .unwrap();
    let run = *app
        .accessibility
        .commands
        .nodes
        .get(&CommandsSemanticTarget::Run(key.clone()))
        .unwrap();
    let delete = *app
        .accessibility
        .commands
        .nodes
        .get(&CommandsSemanticTarget::Delete(key))
        .unwrap();
    assert_eq!(node(group).role(), Role::Group);
    assert_eq!(node(group).children(), &[run, delete]);
    assert_eq!(node(run).label(), Some("Run Inspect logs"));
    assert_eq!(node(delete).label(), Some("Delete Inspect logs"));
    assert!(node(run).supports_action(Action::Click));

    let assert_rect_close = |actual: Option<Rect>, expected: Rect| {
        let actual = actual.unwrap();
        for (actual, expected) in [
            (actual.x0, expected.x0),
            (actual.y0, expected.y0),
            (actual.x1, expected.x1),
            (actual.y1, expected.y1),
        ] {
            assert!((actual - expected).abs() < 0.05, "{actual} != {expected}");
        }
    };
    assert_rect_close(node(run).bounds(), bounds_rect(layout.command_run(0)));
    assert_rect_close(node(delete).bounds(), bounds_rect(layout.command_delete(0)));
}

#[test]
fn custom_command_form_accepts_bounded_utf8_values_without_executing_them() {
    let mut app = App::new_headless(Config::default()).unwrap();
    app.commands_panel.toggle();
    app.accessibility_snapshot();
    app.handle_accessibility_action(ActionRequest {
        action: Action::Click,
        target_tree: TreeId::ROOT,
        target_node: COMMANDS_NEW,
        data: None,
    });
    let form = app.accessibility_snapshot();
    assert_eq!(form.focus, COMMANDS_NAME);
    assert!(form.nodes.iter().any(|(id, node)| {
        *id == COMMANDS_COMMAND
            && node.role() == Role::TextInput
            && node.supports_action(Action::SetValue)
    }));

    app.handle_accessibility_action(ActionRequest {
        action: Action::SetValue,
        target_tree: TreeId::ROOT,
        target_node: COMMANDS_NAME,
        data: Some(ActionData::Value("Deploy 🫚".into())),
    });
    app.handle_accessibility_action(ActionRequest {
        action: Action::SetValue,
        target_tree: TreeId::ROOT,
        target_node: COMMANDS_COMMAND,
        data: Some(ActionData::Value("printf inert-fixture".into())),
    });
    let info = app.commands_panel.build_info(&app.config.commands);
    assert_eq!(info.form_name, "Deploy 🫚");
    assert_eq!(info.form_command, "printf inert-fixture");

    app.handle_accessibility_action(ActionRequest {
        action: Action::SetValue,
        target_tree: TreeId::ROOT,
        target_node: COMMANDS_COMMAND,
        data: Some(ActionData::Value("rejected\nsecond line".into())),
    });
    assert_eq!(
        app.commands_panel
            .build_info(&app.config.commands)
            .form_command,
        "printf inert-fixture"
    );
    app.handle_accessibility_action(ActionRequest {
        action: Action::Click,
        target_tree: TreeId::ROOT,
        target_node: COMMANDS_CANCEL,
        data: None,
    });
    assert!(!app.commands_panel.is_editing());
    assert!(app.config.commands.is_empty());
}

#[test]
fn custom_command_semantic_replacement_rejects_controls_without_changing_focus() {
    let mut app = App::new_headless(Config::default()).unwrap();
    app.commands_panel.toggle();
    app.commands_panel.start_add();
    for (target, other, limit) in [
        (COMMANDS_NAME, COMMANDS_COMMAND, 256),
        (COMMANDS_COMMAND, COMMANDS_NAME, 16 * 1024),
    ] {
        for value in ["", "e\u{301}👩\u{200d}💻", &"🫚".repeat(limit / 4)] {
            app.handle_ime_event(winit::event::Ime::Preedit("draft".into(), None));
            app.redraw_scheduler.begin_frame();
            app.handle_accessibility_action(ActionRequest {
                action: Action::SetValue,
                target_tree: TreeId::ROOT,
                target_node: target,
                data: Some(ActionData::Value(value.into())),
            });
            assert!(app.ime.preedit().is_none());
            assert!(
                app.redraw_scheduler
                    .begin_frame()
                    .contains(RedrawReason::Accessibility)
            );
            let snapshot = app.accessibility_snapshot();
            assert_eq!(snapshot.focus, target);
            let node = &snapshot
                .nodes
                .iter()
                .find(|(id, _)| *id == target)
                .unwrap()
                .1;
            assert_eq!(node.value(), Some(value));
            assert_eq!(
                node.text_selection().unwrap().focus.character_index,
                value.chars().count()
            );
        }
        app.handle_accessibility_action(ActionRequest {
            action: Action::Focus,
            target_tree: TreeId::ROOT,
            target_node: other,
            data: None,
        });
        let before = app.commands_panel.build_info(&[]);
        let cursors = (
            app.commands_panel.form_name.cursor(),
            app.commands_panel.form_command.cursor(),
        );
        app.handle_ime_event(winit::event::Ime::Preedit("keep".into(), None));
        for value in [
            "a\nb",
            "a\rb",
            "\t",
            "\x1b",
            "\0",
            "\u{7f}",
            "\u{85}",
            "\u{2028}",
            "\u{2029}",
            &"x".repeat(limit + 1),
        ] {
            app.handle_accessibility_action(ActionRequest {
                action: Action::SetValue,
                target_tree: TreeId::ROOT,
                target_node: target,
                data: Some(ActionData::Value(value.into())),
            });
            assert!(app.ime.preedit().is_some());
            assert_eq!(app.accessibility_snapshot().focus, other);
            let after = app.commands_panel.build_info(&[]);
            assert_eq!(after.form_name, before.form_name);
            assert_eq!(after.form_command, before.form_command);
            assert_eq!(app.commands_panel.form_name.cursor(), cursors.0);
            assert_eq!(app.commands_panel.form_command.cursor(), cursors.1);
            assert_eq!(after.edit_mode, before.edit_mode);
        }
    }
    assert!(app.config.commands.is_empty());
}

#[test]
fn stale_custom_command_actions_cannot_retarget_another_command() {
    use crate::config::CustomCommand;

    let mut app = App::new_headless(Config::default()).unwrap();
    app.config.commands = vec![
        CustomCommand {
            name: "First".into(),
            command: "printf first".into(),
            alias: None,
        },
        CustomCommand {
            name: "Second".into(),
            command: "printf second".into(),
            alias: None,
        },
    ];
    app.commands_panel.toggle();
    app.accessibility_snapshot();
    let first_key = custom_command_keys(&app.config.commands).remove(0);
    let stale_delete = *app
        .accessibility
        .commands
        .nodes
        .get(&CommandsSemanticTarget::Delete(first_key))
        .unwrap();
    app.config.commands.remove(0);
    app.handle_accessibility_action(ActionRequest {
        action: Action::Click,
        target_tree: TreeId::ROOT,
        target_node: stale_delete,
        data: None,
    });
    assert_eq!(app.config.commands.len(), 1);
    assert_eq!(app.config.commands[0].name, "Second");

    app.accessibility_snapshot();
    assert!(
        !app.accessibility
            .commands
            .targets
            .contains_key(&stale_delete)
    );
}

fn test_workspace_entries(count: usize) -> Vec<crate::ui::workspace_panel::WorkspaceEntry> {
    (0..count)
        .map(|index| crate::ui::workspace_panel::WorkspaceEntry {
            id: format!("workspace-{index}"),
            display_name: format!("Workspace {index}"),
            subtitle: format!("{} tabs", index + 1),
            last_modified: "local fixture".into(),
        })
        .collect()
}

#[test]
fn workspace_manager_is_modal_bounded_and_uses_shared_action_geometry() {
    let mut app = App::new_headless(Config::default()).unwrap();
    app.workspace_panel
        .set_test_entries(test_workspace_entries(20), true);
    let preview = app.workspace_panel.build_info();
    let layout = app.workspace_accessibility_layout(&preview);
    let snapshot = app.accessibility_snapshot();
    let node = |id| {
        &snapshot
            .nodes
            .iter()
            .find(|(node_id, _)| *node_id == id)
            .unwrap()
            .1
    };
    assert_eq!(node(ROOT).children(), &[WORKSPACE_DIALOG]);
    assert_eq!(node(WORKSPACE_DIALOG).role(), Role::Dialog);
    assert!(node(WORKSPACE_DIALOG).is_modal());
    assert_eq!(node(WORKSPACE_LIST).role(), Role::List);

    let key = WorkspaceSemanticKey {
        id: "workspace-0".into(),
        workspaces: true,
    };
    let group = *app
        .accessibility
        .workspace
        .nodes
        .get(&WorkspaceSemanticTarget::Group(key.clone()))
        .unwrap();
    let load = *app
        .accessibility
        .workspace
        .nodes
        .get(&WorkspaceSemanticTarget::Load(key.clone()))
        .unwrap();
    let rename = *app
        .accessibility
        .workspace
        .nodes
        .get(&WorkspaceSemanticTarget::Rename(key.clone()))
        .unwrap();
    let delete = *app
        .accessibility
        .workspace
        .nodes
        .get(&WorkspaceSemanticTarget::Delete(key))
        .unwrap();
    assert_eq!(node(group).children(), &[load, rename, delete]);
    assert_eq!(node(WORKSPACE_LIST).active_descendant(), Some(group));
    assert_eq!(snapshot.focus, load);
    assert!(node(load).supports_action(Action::Click));

    let actual = node(load).bounds().unwrap();
    let button = layout.entry(0).load_button;
    let dialog = node(WORKSPACE_DIALOG).bounds().unwrap();
    let expected = rect(
        dialog.x0 as f32 + (button.x - layout.panel.x),
        dialog.y0 as f32 + (button.y - layout.panel.y),
        button.width,
        button.height,
    );
    for (actual, expected) in [
        (actual.x0, expected.x0),
        (actual.y0, expected.y0),
        (actual.x1, expected.x1),
        (actual.y1, expected.y1),
    ] {
        assert!((actual - expected).abs() < 0.1, "{actual} != {expected}");
    }

    let exposed_groups = node(WORKSPACE_LIST).children().len();
    assert_eq!(exposed_groups, layout.visible_count);
    assert!(exposed_groups < 20);
}

#[test]
fn workspace_rename_semantics_accept_utf8_and_cancel_without_persistence() {
    let mut app = App::new_headless(Config::default()).unwrap();
    app.workspace_panel
        .set_test_entries(test_workspace_entries(1), true);
    app.accessibility_snapshot();
    let key = WorkspaceSemanticKey {
        id: "workspace-0".into(),
        workspaces: true,
    };
    let rename = *app
        .accessibility
        .workspace
        .nodes
        .get(&WorkspaceSemanticTarget::Rename(key))
        .unwrap();
    app.handle_accessibility_action(ActionRequest {
        action: Action::Click,
        target_tree: TreeId::ROOT,
        target_node: rename,
        data: None,
    });
    let form = app.accessibility_snapshot();
    assert_eq!(form.focus, WORKSPACE_RENAME_INPUT);
    assert!(form.nodes.iter().any(|(id, node)| {
        *id == WORKSPACE_RENAME_SAVE && node.bounds().is_some() && node.role() == Role::Button
    }));
    let load = *app
        .accessibility
        .workspace
        .nodes
        .get(&WorkspaceSemanticTarget::Load(WorkspaceSemanticKey {
            id: "workspace-0".into(),
            workspaces: true,
        }))
        .unwrap();
    assert!(
        form.nodes
            .iter()
            .find(|(id, _)| *id == load)
            .unwrap()
            .1
            .is_disabled()
    );

    app.handle_accessibility_action(ActionRequest {
        action: Action::SetValue,
        target_tree: TreeId::ROOT,
        target_node: WORKSPACE_RENAME_INPUT,
        data: Some(ActionData::Value("New roots 🫚".into())),
    });
    assert_eq!(
        app.workspace_panel.build_info().rename_text.as_deref(),
        Some("New roots 🫚")
    );
    // Keyboard caret moves are reflected as scalar positions in the
    // semantic tree, even though the editor tracks UTF-8 byte offsets.
    assert!(
        app.workspace_panel
            .accessibility_set_rename("e\u{301}👩\u{200d}💻z")
    );
    for (key, expected_index) in [
        (winit::keyboard::NamedKey::Home, 0),
        (winit::keyboard::NamedKey::ArrowRight, 2),
        (winit::keyboard::NamedKey::ArrowRight, 5),
        (winit::keyboard::NamedKey::ArrowLeft, 2),
    ] {
        assert!(app.handle_workspace_rename_key(&winit::keyboard::Key::Named(key)));
        let snapshot = app.accessibility_snapshot();
        assert_eq!(snapshot.focus, WORKSPACE_RENAME_INPUT);
        let node = &snapshot
            .nodes
            .iter()
            .find(|(id, _)| *id == WORKSPACE_RENAME_INPUT)
            .unwrap()
            .1;
        let selection = node.text_selection().unwrap();
        assert_eq!(selection.anchor.character_index, expected_index);
        assert_eq!(selection.focus.character_index, expected_index);
        assert_eq!(node.value(), Some("e\u{301}👩\u{200d}💻z"));
    }
    for value in ["", "e\u{301}👩\u{200d}💻", &"🫚".repeat(128)] {
        app.handle_ime_event(winit::event::Ime::Preedit("draft".into(), None));
        app.handle_accessibility_action(ActionRequest {
            action: Action::SetValue,
            target_tree: TreeId::ROOT,
            target_node: WORKSPACE_RENAME_INPUT,
            data: Some(ActionData::Value(value.into())),
        });
        assert!(app.ime.preedit().is_none());
        let snapshot = app.accessibility_snapshot();
        assert_eq!(snapshot.focus, WORKSPACE_RENAME_INPUT);
        let node = &snapshot
            .nodes
            .iter()
            .find(|(id, _)| *id == WORKSPACE_RENAME_INPUT)
            .unwrap()
            .1;
        assert_eq!(node.value(), Some(value));
        assert_eq!(
            node.text_selection().unwrap().focus.character_index,
            value.chars().count()
        );
    }
    assert!(app.workspace_panel.accessibility_set_rename("New roots 🫚"));
    app.handle_ime_event(winit::event::Ime::Preedit("keep".into(), None));
    app.handle_accessibility_action(ActionRequest {
        action: Action::SetValue,
        target_tree: TreeId::ROOT,
        target_node: WORKSPACE_RENAME_INPUT,
        data: Some(ActionData::Value("bad\nname".into())),
    });
    assert!(app.ime.preedit().is_some());
    assert_eq!(
        app.workspace_panel.build_info().rename_text.as_deref(),
        Some("New roots 🫚")
    );
    app.handle_accessibility_action(ActionRequest {
        action: Action::Click,
        target_tree: TreeId::ROOT,
        target_node: WORKSPACE_RENAME_CANCEL,
        data: None,
    });
    assert!(!app.workspace_panel.is_renaming());
    assert!(app.ime.preedit().is_none());
    assert_eq!(app.workspace_panel.entries()[0].display_name, "Workspace 0");
}

#[test]
fn stale_workspace_actions_cannot_retarget_another_entry() {
    let mut app = App::new_headless(Config::default()).unwrap();
    app.workspace_panel
        .set_test_entries(test_workspace_entries(2), true);
    app.accessibility_snapshot();
    let stale_delete = *app
        .accessibility
        .workspace
        .nodes
        .get(&WorkspaceSemanticTarget::Delete(WorkspaceSemanticKey {
            id: "workspace-0".into(),
            workspaces: true,
        }))
        .unwrap();
    app.workspace_panel.set_test_entries(
        test_workspace_entries(2).into_iter().skip(1).collect(),
        true,
    );
    app.handle_accessibility_action(ActionRequest {
        action: Action::Click,
        target_tree: TreeId::ROOT,
        target_node: stale_delete,
        data: None,
    });
    assert_eq!(app.workspace_panel.entries().len(), 1);
    assert_eq!(app.workspace_panel.entries()[0].id, "workspace-1");
    app.accessibility_snapshot();
    assert!(
        !app.accessibility
            .workspace
            .targets
            .contains_key(&stale_delete)
    );
}

#[test]
fn legacy_snapshot_semantics_never_offer_rename() {
    let mut app = App::new_headless(Config::default()).unwrap();
    app.workspace_panel
        .set_test_entries(test_workspace_entries(1), false);
    let snapshot = app.accessibility_snapshot();
    assert_eq!(
        snapshot
            .nodes
            .iter()
            .find(|(id, _)| *id == WORKSPACE_DIALOG)
            .unwrap()
            .1
            .label(),
        Some("Tab snapshots")
    );
    assert!(
        !app.accessibility
            .workspace
            .targets
            .keys()
            .any(|target| matches!(
                app.accessibility.workspace.targets.get(target),
                Some(WorkspaceSemanticTarget::Rename(_))
            ))
    );
}

#[test]
fn shortcuts_editor_is_one_bounded_modal_tree_with_shared_geometry() {
    use crate::input::keybindings::BindableAction;

    let mut app = App::new_headless(Config::default()).unwrap();
    app.shortcuts_panel.open();
    let preview = app.shortcuts_panel.build_info(&app.shortcut_registry);
    let layout = app.shortcuts_accessibility_layout(&preview);
    let snapshot = app.accessibility_snapshot();
    let node = |id| {
        &snapshot
            .nodes
            .iter()
            .find(|(node_id, _)| *node_id == id)
            .unwrap()
            .1
    };
    assert_eq!(node(ROOT).children(), &[SHORTCUTS_DIALOG]);
    assert_eq!(node(SHORTCUTS_DIALOG).role(), Role::Dialog);
    assert!(node(SHORTCUTS_DIALOG).is_modal());
    assert_eq!(node(SHORTCUTS_INPUT).role(), Role::SearchInput);
    assert_eq!(node(SHORTCUTS_INPUT).controls(), &[SHORTCUTS_LIST]);
    assert!(node(SHORTCUTS_INPUT).supports_action(Action::SetValue));
    assert_eq!(node(SHORTCUTS_LIST).role(), Role::List);
    assert!(node(SHORTCUTS_LIST).children().len() <= layout.rows_that_fit);

    let copy = *app
        .accessibility
        .shortcuts
        .nodes
        .get(&ShortcutsSemanticTarget::Action(BindableAction::Copy))
        .unwrap();
    assert_eq!(node(copy).role(), Role::Button);
    assert!(node(copy).supports_action(Action::Click));
    assert!(node(copy).supports_action(Action::CustomAction));
    let row = node(copy).bounds().unwrap();
    let dialog = node(SHORTCUTS_DIALOG).bounds().unwrap();
    let first_action = layout.row(1);
    let expected_row = rect(
        dialog.x0 as f32 + (first_action.x - layout.panel.x),
        dialog.y0 as f32 + (first_action.y - layout.panel.y),
        first_action.width,
        first_action.height,
    );
    for (actual, expected) in [
        (row.x0, expected_row.x0),
        (row.y0, expected_row.y0),
        (row.x1, expected_row.x1),
        (row.y1, expected_row.y1),
    ] {
        assert!((actual - expected).abs() < 0.1, "{actual} != {expected}");
    }
}

#[test]
fn shortcut_filter_semantic_edits_retire_preedit_without_changing_bindings() {
    use crate::input::keybindings::BindableAction;
    let mut app = App::new_headless(Config::default()).unwrap();
    app.shortcuts_panel.open();
    let bindings = |app: &App| {
        BindableAction::all()
            .iter()
            .map(|action| app.shortcut_registry.binding_for(*action).cloned())
            .collect::<Vec<_>>()
    };
    let original_bindings = bindings(&app);
    let original_config = toml::to_string(&app.config).unwrap();
    for value in ["copy", "e\u{301}👩\u{200d}💻", &"🫚".repeat(128), ""] {
        app.handle_ime_event(winit::event::Ime::Preedit("draft".into(), None));
        app.handle_accessibility_action(ActionRequest {
            action: Action::SetValue,
            target_tree: TreeId::ROOT,
            target_node: SHORTCUTS_INPUT,
            data: Some(ActionData::Value(value.into())),
        });
        let snapshot = app.accessibility_snapshot();
        let input = &snapshot
            .nodes
            .iter()
            .find(|(id, _)| *id == SHORTCUTS_INPUT)
            .unwrap()
            .1;
        assert_eq!(input.value(), Some(value));
        assert_eq!(
            input.text_selection().unwrap().focus.character_index,
            value.chars().count()
        );
        assert!(input.supports_action(Action::SetValue));
        assert!(!input.is_read_only());
        assert_eq!(snapshot.focus, SHORTCUTS_INPUT);
        assert!(app.ime.preedit().is_none());
        assert!(app.redraw_scheduler.has_pending());
    }
    app.handle_ime_event(winit::event::Ime::Preedit("keep".into(), None));
    for data in [
        None,
        Some(ActionData::Value("bad\nquery".into())),
        Some(ActionData::Value("x".repeat(129).into())),
    ] {
        app.handle_accessibility_action(ActionRequest {
            action: Action::SetValue,
            target_tree: TreeId::ROOT,
            target_node: SHORTCUTS_INPUT,
            data,
        });
        assert!(
            app.shortcuts_panel
                .build_info(&app.shortcut_registry)
                .filter
                .is_empty()
        );
        assert!(app.ime.preedit().is_some());
    }
    assert_eq!(bindings(&app), original_bindings);
    assert_eq!(toml::to_string(&app.config).unwrap(), original_config);
}

#[test]
fn shortcut_recording_rejects_filter_focus_and_restores_editability_on_every_exit() {
    use crate::input::keybindings::{BindableAction, KeyChord};
    use crate::ui::shortcuts_panel::{ShortcutsAction, ShortcutsKey};
    for exit in ["cancel", "timeout", "capture"] {
        let mut app = App::new_headless(Config::default()).unwrap();
        app.shortcuts_panel.open();
        app.accessibility.focus_override = Some(SHORTCUTS_INPUT);
        app.apply_shortcuts_action(ShortcutsAction::StartRecording(BindableAction::Copy));
        let deadline = app.shortcuts_panel.recording_deadline();
        let snapshot = app.accessibility_snapshot();
        let copy = *app
            .accessibility
            .shortcuts
            .nodes
            .get(&ShortcutsSemanticTarget::Action(BindableAction::Copy))
            .unwrap();
        assert_eq!(snapshot.focus, copy);
        let input = &snapshot
            .nodes
            .iter()
            .find(|(id, _)| *id == SHORTCUTS_INPUT)
            .unwrap()
            .1;
        assert!(input.is_read_only());
        assert!(!input.supports_action(Action::SetValue));
        assert!(!input.supports_action(Action::Focus));
        for action in [Action::Focus, Action::SetValue] {
            app.handle_accessibility_action(ActionRequest {
                action,
                target_tree: TreeId::ROOT,
                target_node: SHORTCUTS_INPUT,
                data: Some(ActionData::Value("paste".into())),
            });
            assert_eq!(app.accessibility_snapshot().focus, copy);
            assert!(
                app.shortcuts_panel
                    .build_info(&app.shortcut_registry)
                    .filter
                    .is_empty()
            );
            assert_eq!(app.shortcuts_panel.recording_deadline(), deadline);
            assert!(app.shortcuts_panel.is_recording());
        }
        match exit {
            "cancel" => {
                app.shortcuts_panel
                    .handle_key(ShortcutsKey::Escape, &app.shortcut_registry);
            }
            "timeout" => {
                assert!(app.shortcuts_panel.tick_recording_timeout(
                    std::time::Instant::now()
                        + crate::ui::shortcuts_panel::ShortcutsPanel::RECORDING_TIMEOUT
                ));
            }
            "capture" => {
                let chord = KeyChord::new("F24", true, true, true, false);
                assert_eq!(
                    app.shortcuts_panel.handle_key(
                        ShortcutsKey::RecordedChord(chord.clone()),
                        &app.shortcut_registry
                    ),
                    ShortcutsAction::SetShortcut {
                        action: BindableAction::Copy,
                        chord: Some(chord)
                    }
                );
            }
            _ => unreachable!(),
        }
        assert!(!app.shortcuts_panel.is_recording());
        let snapshot = app.accessibility_snapshot();
        let input = &snapshot
            .nodes
            .iter()
            .find(|(id, _)| *id == SHORTCUTS_INPUT)
            .unwrap()
            .1;
        assert!(!input.is_read_only());
        assert!(input.supports_action(Action::SetValue));
        assert!(input.supports_action(Action::Focus));
        app.handle_accessibility_action(ActionRequest {
            action: Action::SetValue,
            target_tree: TreeId::ROOT,
            target_node: SHORTCUTS_INPUT,
            data: Some(ActionData::Value("paste".into())),
        });
        assert_eq!(app.accessibility_snapshot().focus, SHORTCUTS_INPUT);
        assert_eq!(
            app.shortcuts_panel
                .build_info(&app.shortcut_registry)
                .filter,
            "paste"
        );
    }
}

#[test]
fn shortcut_recording_never_falls_back_to_read_only_filter_when_target_scrolls_away() {
    use crate::input::keybindings::BindableAction;
    use crate::ui::shortcuts_panel::ShortcutsAction;
    let mut app = App::new_headless(Config::default()).unwrap();
    app.shortcuts_panel.open();
    app.apply_shortcuts_action(ShortcutsAction::StartRecording(BindableAction::Copy));
    app.accessibility.focus_override = Some(SHORTCUTS_INPUT);
    app.shortcuts_panel
        .scroll_down(usize::MAX, &app.shortcut_registry);
    let snapshot = app.accessibility_snapshot();
    assert_eq!(snapshot.focus, SHORTCUTS_DIALOG);
    assert!(app.shortcuts_panel.is_recording());
    assert!(
        snapshot
            .nodes
            .iter()
            .any(|(id, node)| *id == SHORTCUTS_STATUS && node.live() == Some(Live::Polite))
    );
}

#[test]
fn shortcut_semantic_actions_record_clear_and_reject_stale_filtered_targets() {
    use crate::input::keybindings::BindableAction;

    let mut app = App::new_headless(Config::default()).unwrap();
    app.shortcuts_panel.open();
    app.accessibility_snapshot();
    let copy = *app
        .accessibility
        .shortcuts
        .nodes
        .get(&ShortcutsSemanticTarget::Action(BindableAction::Copy))
        .unwrap();

    app.handle_accessibility_action(ActionRequest {
        action: Action::Click,
        target_tree: TreeId::ROOT,
        target_node: copy,
        data: None,
    });
    assert!(app.shortcuts_panel.is_recording());
    let recording = app.accessibility_snapshot();
    assert!(
        recording
            .nodes
            .iter()
            .any(|(id, node)| { *id == SHORTCUTS_STATUS && node.live() == Some(Live::Polite) })
    );
    app.shortcuts_panel.handle_key(
        crate::ui::shortcuts_panel::ShortcutsKey::Escape,
        &app.shortcut_registry,
    );

    app.handle_accessibility_action(ActionRequest {
        action: Action::CustomAction,
        target_tree: TreeId::ROOT,
        target_node: copy,
        data: Some(ActionData::CustomAction(SHORTCUT_CLEAR_ACTION)),
    });
    assert!(
        app.shortcut_registry
            .binding_for(BindableAction::Copy)
            .is_none()
    );

    app.handle_accessibility_action(ActionRequest {
        action: Action::SetValue,
        target_tree: TreeId::ROOT,
        target_node: SHORTCUTS_INPUT,
        data: Some(ActionData::Value("paste".into())),
    });
    app.handle_accessibility_action(ActionRequest {
        action: Action::Click,
        target_tree: TreeId::ROOT,
        target_node: copy,
        data: None,
    });
    assert!(!app.shortcuts_panel.is_recording());
}

fn domain_picker_config(hosts: usize) -> Config {
    let mut config = Config::default();
    config.domains.ssh_hosts = (0..hosts)
        .map(|index| {
            (
                format!("host-{index:02}"),
                crate::config::SshHostProfile {
                    destination: format!("developer@host-{index:02}.example"),
                    description: Some(format!("Build host {index}")),
                },
            )
        })
        .collect();
    config
}

#[test]
fn domain_picker_is_one_bounded_modal_tree_with_stable_host_actions() {
    let mut app = App::new_headless(domain_picker_config(20)).unwrap();
    app.domain_picker.open(&app.config.domains);
    let preview = app.domain_picker.build_info();
    let layout = app.command_palette_accessibility_layout(&preview);
    let snapshot = app.accessibility_snapshot();
    let node = |id| {
        &snapshot
            .nodes
            .iter()
            .find(|(node_id, _)| *node_id == id)
            .unwrap()
            .1
    };
    assert_eq!(node(ROOT).children(), &[DOMAIN_DIALOG]);
    assert_eq!(node(DOMAIN_DIALOG).role(), Role::Dialog);
    assert!(node(DOMAIN_DIALOG).is_modal());
    assert_eq!(node(DOMAIN_INPUT).role(), Role::SearchInput);
    assert_eq!(node(DOMAIN_INPUT).controls(), &[DOMAIN_LIST]);
    assert!(node(DOMAIN_INPUT).supports_action(Action::SetValue));
    assert_eq!(node(DOMAIN_LIST).role(), Role::ListBox);
    assert!(node(DOMAIN_LIST).children().len() <= layout.rows_that_fit);
    assert!(node(DOMAIN_LIST).children().len() < 21);

    let local = *app
        .accessibility
        .domain
        .nodes
        .get("domain-picker.authority.local")
        .unwrap();
    assert_eq!(node(local).role(), Role::ListBoxOption);
    assert!(node(local).supports_action(Action::Click));
    let first = node(node(DOMAIN_LIST).children()[0]).bounds().unwrap();
    let dialog = node(DOMAIN_DIALOG).bounds().unwrap();
    assert!((first.x0 - (dialog.x0 + f64::from(layout.padding * 0.5))).abs() < 0.1);
    assert!((first.y0 - (dialog.y0 + f64::from(layout.list.y - layout.panel.y))).abs() < 0.1);
}

#[test]
fn domain_semantic_activation_is_exact_and_headless_discovery_stays_local() {
    let mut app = App::new_headless(domain_picker_config(1)).unwrap();
    app.domain_picker.open(&app.config.domains);
    app.accessibility_snapshot();
    let remote_id = "domain-picker.authority.ssh.host-00";
    let remote = *app.accessibility.domain.nodes.get(remote_id).unwrap();

    app.handle_accessibility_action(ActionRequest {
        action: Action::SetValue,
        target_tree: TreeId::ROOT,
        target_node: DOMAIN_INPUT,
        data: Some(ActionData::Value("Build host 0".into())),
    });
    app.handle_accessibility_action(ActionRequest {
        action: Action::Click,
        target_tree: TreeId::ROOT,
        target_node: remote,
        data: None,
    });

    // Headless orchestration rejects before constructing a client, so this
    // proves routing without touching a socket or invoking SSH.
    assert!(app.domain_picker.is_active());
    assert_eq!(app.domain_picker.build_info().item_noun, "result");
    let failed = app.accessibility_snapshot();
    let status = failed
        .nodes
        .iter()
        .find(|(_, node)| node.role() == Role::Status)
        .unwrap();
    assert_eq!(status.1.live(), Some(Live::Polite));
    assert!(
        status
            .1
            .label()
            .is_some_and(|label| label.contains("failed"))
    );
}

#[test]
fn domain_picker_rejects_hidden_stale_and_background_actions() {
    let mut app = App::new_headless(domain_picker_config(1)).unwrap();
    app.domain_picker.open(&app.config.domains);
    app.accessibility_snapshot();
    let local = *app
        .accessibility
        .domain
        .nodes
        .get("domain-picker.authority.local")
        .unwrap();
    app.handle_accessibility_action(ActionRequest {
        action: Action::SetValue,
        target_tree: TreeId::ROOT,
        target_node: DOMAIN_INPUT,
        data: Some(ActionData::Value("no such host".into())),
    });
    app.handle_accessibility_action(ActionRequest {
        action: Action::Click,
        target_tree: TreeId::ROOT,
        target_node: local,
        data: None,
    });
    assert!(app.domain_picker.can_filter());
    assert_eq!(app.domain_picker.build_info().item_noun, "host");

    app.handle_accessibility_action(ActionRequest {
        action: Action::Click,
        target_tree: TreeId::ROOT,
        target_node: SETTINGS,
        data: None,
    });
    assert!(app.domain_picker.is_active());
    assert!(!app.settings_panel.is_active());
}

#[test]
fn unicode_picker_is_one_bounded_modal_tree_with_stable_actions() {
    let mut app = App::new_headless(Config::default()).unwrap();
    app.unicode_picker.open();
    let preview = app.unicode_picker.build_info();
    let layout = app.command_palette_accessibility_layout(&preview);
    let snapshot = app.accessibility_snapshot();
    let node = |id| {
        &snapshot
            .nodes
            .iter()
            .find(|(node_id, _)| *node_id == id)
            .unwrap()
            .1
    };
    assert_eq!(node(ROOT).children(), &[UNICODE_DIALOG]);
    assert_eq!(node(UNICODE_DIALOG).role(), Role::Dialog);
    assert!(node(UNICODE_DIALOG).is_modal());
    assert_eq!(node(UNICODE_INPUT).role(), Role::SearchInput);
    assert_eq!(node(UNICODE_INPUT).controls(), &[UNICODE_LIST]);
    assert!(node(UNICODE_INPUT).supports_action(Action::SetValue));
    assert_eq!(node(UNICODE_LIST).role(), Role::ListBox);
    assert!(node(UNICODE_LIST).children().len() <= layout.rows_that_fit);

    let arrow = *app
        .accessibility
        .unicode
        .nodes
        .get("unicode-picker.curated.2192")
        .unwrap();
    assert_eq!(node(arrow).role(), Role::ListBoxOption);
    assert!(node(arrow).supports_action(Action::Click));
    assert!(node(arrow).supports_action(Action::Focus));
    let first = node(node(UNICODE_LIST).children()[0]).bounds().unwrap();
    let dialog = node(UNICODE_DIALOG).bounds().unwrap();
    assert!((first.x0 - (dialog.x0 + f64::from(layout.padding * 0.5))).abs() < 0.1);
    assert!((first.y0 - (dialog.y0 + f64::from(layout.list.y - layout.panel.y))).abs() < 0.1);
}

#[test]
fn unicode_accessibility_filter_retires_stale_results_and_routes_exact_focus() {
    let mut app = App::new_headless(Config::default()).unwrap();
    app.unicode_picker.open();
    app.accessibility_snapshot();
    let stale = *app
        .accessibility
        .unicode
        .nodes
        .get("unicode-picker.curated.2192")
        .unwrap();

    app.handle_accessibility_action(ActionRequest {
        action: Action::SetValue,
        target_tree: TreeId::ROOT,
        target_node: UNICODE_INPUT,
        data: Some(ActionData::Value("U+03BB".into())),
    });
    let filtered = app.accessibility_snapshot();
    let exact_id = "unicode-picker.codepoint.3bb";
    let exact = *app.accessibility.unicode.nodes.get(exact_id).unwrap();
    assert!(!app.accessibility.unicode.targets.contains_key(&stale));
    let input = &filtered
        .nodes
        .iter()
        .find(|(id, _)| *id == UNICODE_INPUT)
        .unwrap()
        .1;
    assert_eq!(input.active_descendant(), Some(exact));

    app.handle_accessibility_action(ActionRequest {
        action: Action::Click,
        target_tree: TreeId::ROOT,
        target_node: stale,
        data: None,
    });
    assert!(app.unicode_picker.is_active());
    app.handle_accessibility_action(ActionRequest {
        action: Action::Focus,
        target_tree: TreeId::ROOT,
        target_node: exact,
        data: None,
    });
    assert_eq!(app.accessibility.focus_override, Some(UNICODE_INPUT));
    assert_eq!(app.unicode_picker.build_info().entries[0].id, exact_id);

    app.handle_accessibility_action(ActionRequest {
        action: Action::Click,
        target_tree: TreeId::ROOT,
        target_node: SETTINGS,
        data: None,
    });
    assert!(app.unicode_picker.is_active());
    assert!(!app.settings_panel.is_active());
}

#[test]
fn palette_editing_publishes_the_moved_utf8_caret_to_accessibility() {
    use crate::ui::text_input::TextInputEdit;
    for unicode in [false, true] {
        let mut app = App::new_headless(Config::default()).unwrap();
        if unicode {
            app.unicode_picker.open();
            app.unicode_picker.set_query("a界e\u{301}z");
            app.unicode_picker.edit_query(TextInputEdit::Home);
            app.unicode_picker.edit_query(TextInputEdit::Right);
            app.unicode_picker.edit_query(TextInputEdit::Right);
            app.unicode_picker.edit_query(TextInputEdit::Right);
        } else {
            app.command_palette.open();
            app.command_palette.accessibility_set_input("a界e\u{301}z");
            app.command_palette.edit_input(TextInputEdit::Home);
            app.command_palette.edit_input(TextInputEdit::Right);
            app.command_palette.edit_input(TextInputEdit::Right);
            app.command_palette.edit_input(TextInputEdit::Right);
        }
        let snapshot = app.accessibility_snapshot();
        let id = if unicode {
            UNICODE_INPUT
        } else {
            PALETTE_INPUT
        };
        let (_, input) = snapshot
            .nodes
            .iter()
            .find(|(node_id, _)| *node_id == id)
            .unwrap();
        let selection = input.text_selection().unwrap();
        assert_eq!(selection.anchor.character_index, 4);
        assert_eq!(selection.focus.character_index, 4);
        assert_eq!(input.value(), Some("a界e\u{301}z"));
        app.unicode_picker.close();
        app.unicode_picker.finish_search_for_test();
    }
}

#[test]
fn unicode_name_search_keeps_the_semantic_registry_bounded() {
    let mut app = App::new_headless(Config::default()).unwrap();
    app.unicode_picker.open();
    app.accessibility_snapshot();
    let stale = *app
        .accessibility
        .unicode
        .nodes
        .get("unicode-picker.curated.2192")
        .unwrap();

    app.handle_accessibility_action(ActionRequest {
        action: Action::SetValue,
        target_tree: TreeId::ROOT,
        target_node: UNICODE_INPUT,
        data: Some(ActionData::Value("snowman".into())),
    });
    let pending = app.accessibility_snapshot();
    assert!(pending.nodes.iter().any(|(id, node)| {
        *id == UNICODE_EMPTY && node.label() == Some("Searching characters…")
    }));
    assert!(!app.accessibility.unicode.targets.contains_key(&stale));
    app.unicode_picker.finish_search_for_test();
    let snapshot = app.accessibility_snapshot();
    let info = app.unicode_picker.build_info();
    assert!(info.total_count <= 256);
    assert!(
        info.entries
            .iter()
            .any(|entry| entry.id == "unicode-picker.named.2603")
    );
    assert!(!app.accessibility.unicode.targets.contains_key(&stale));
    assert!(
        snapshot
            .nodes
            .iter()
            .find(|(node_id, _)| *node_id == UNICODE_LIST)
            .is_some_and(|(_, list)| list.children().len() <= info.entries.len())
    );
}

#[test]
fn cldr_emoji_results_expose_exact_sequence_names_and_retire_stale_nodes() {
    let mut app = App::new_headless(Config::default()).unwrap();
    app.unicode_picker.open();
    assert!(app.unicode_picker.set_query("flag united states"));
    app.unicode_picker.finish_search_for_test();
    let snapshot = app.accessibility_snapshot();
    let command = "unicode-picker.cldr.1f1fa-1f1f8";
    let node_id = *app.accessibility.unicode.nodes.get(command).unwrap();
    let node = snapshot
        .nodes
        .iter()
        .find(|(candidate, _)| *candidate == node_id)
        .map(|(_, node)| node)
        .unwrap();
    assert!(node.label().is_some_and(|label| {
        label.contains("🇺🇸") && label.contains("flag: United States")
    }));
    assert!(node.description().is_some_and(|description| {
        description.contains("CLDR 48.0.0 en") && description.contains("U+1F1FA U+1F1F8")
    }));

    assert!(app.unicode_picker.set_query("party pinata"));
    app.unicode_picker.finish_search_for_test();
    app.accessibility_snapshot();
    assert!(!app.accessibility.unicode.nodes.contains_key(command));
}

#[test]
fn pending_domain_picker_exposes_read_only_live_status() {
    let mut app = App::new_headless(Config::default()).unwrap();
    app.domain_picker.open(&app.config.domains);
    assert!(app.domain_picker.activate().is_some());
    let before = app.domain_picker.build_info().input;
    let snapshot = app.accessibility_snapshot();
    let input = &snapshot
        .nodes
        .iter()
        .find(|(id, _)| *id == DOMAIN_INPUT)
        .unwrap()
        .1;
    assert!(input.is_read_only());
    assert!(!input.supports_action(Action::SetValue));
    assert!(
        snapshot
            .nodes
            .iter()
            .any(|(_, node)| { node.role() == Role::Status && node.live() == Some(Live::Polite) })
    );
    app.handle_accessibility_action(ActionRequest {
        action: Action::SetValue,
        target_tree: TreeId::ROOT,
        target_node: DOMAIN_INPUT,
        data: Some(ActionData::Value("must not replace".into())),
    });
    assert_eq!(app.domain_picker.build_info().input, before);
}

fn show_info_fixture(app: &mut App) -> u64 {
    let mut lines: Vec<_> = (0..40)
        .map(|index| format!("fixture line {index} {}", "x".repeat(300)))
        .collect();
    lines[1] = "  1  Safe fixture choice".into();
    app.info_overlay.show_choices(
        "Hermetic notice",
        lines,
        vec![crate::ui::info_overlay::InfoOverlayChoice::new(
            "safe-choice",
            "Safe fixture choice",
            1,
            Some('1'),
        )],
    )
}

#[test]
fn info_overlay_keyboard_and_assistive_focus_share_the_same_owner_and_bounds() {
    use crate::ui::info_overlay::{InfoOverlayAction, InfoOverlayFocus};
    let mut app = App::new_headless(Config::default()).unwrap();
    // Compare one settled phase, not two advancing Instant samples.
    app.info_overlay.set_reduced_motion(true);
    let request = show_info_fixture(&mut app);
    app.accessibility_snapshot();
    app.info_overlay.cycle_focus(request, false);
    let snapshot = app.accessibility_snapshot();
    let choice = app.accessibility.info.nodes[&InfoSemanticTarget::Choice {
        request_id: request,
        id: "safe-choice".into(),
    }];
    assert_eq!(snapshot.focus, choice);
    let layout = app.info_overlay_layout();
    let bounds = snapshot
        .nodes
        .iter()
        .find(|(id, _)| *id == choice)
        .unwrap()
        .1
        .bounds()
        .unwrap();
    let expected = bounds_rect(
        layout
            .source_line_rect(1, app.info_overlay.viewport().row(), 40)
            .unwrap(),
    );
    assert_eq!(bounds, expected);
    assert_eq!(
        app.info_overlay.activate_focused(request),
        InfoOverlayAction::Choose {
            request_id: request,
            choice_id: "safe-choice".into()
        }
    );
    let copy = app.accessibility.info.nodes[&InfoSemanticTarget::Copy(request)];
    app.handle_accessibility_action(ActionRequest {
        action: Action::Focus,
        target_tree: TreeId::ROOT,
        target_node: copy,
        data: None,
    });
    assert_eq!(app.info_overlay.focus(), Some(&InfoOverlayFocus::Copy));
    assert_eq!(
        app.info_overlay.activate_focused(request),
        InfoOverlayAction::Copy {
            request_id: request
        }
    );
    assert_eq!(app.accessibility_snapshot().focus, copy);
    let replacement = app
        .info_overlay
        .show("Replacement", vec!["No choice".into()]);
    app.handle_accessibility_action(ActionRequest {
        action: Action::Focus,
        target_tree: TreeId::ROOT,
        target_node: choice,
        data: None,
    });
    assert_eq!(app.info_overlay.focus(), Some(&InfoOverlayFocus::Continue));
    assert_eq!(
        app.info_overlay.activate_focused(replacement),
        InfoOverlayAction::Dismiss {
            request_id: replacement
        }
    );
}

#[test]
fn info_overlay_is_one_scrollable_modal_tree_with_shared_geometry() {
    let mut app = App::new_headless(Config::default()).unwrap();
    app.info_overlay.set_reduced_motion(true);
    let request = show_info_fixture(&mut app);
    let layout = app.info_overlay_layout();
    let snapshot = app.accessibility_snapshot();
    let node = |id| {
        &snapshot
            .nodes
            .iter()
            .find(|(node_id, _)| *node_id == id)
            .unwrap()
            .1
    };
    let content = *app
        .accessibility
        .info
        .nodes
        .get(&InfoSemanticTarget::Content(request))
        .unwrap();
    let copy = *app
        .accessibility
        .info
        .nodes
        .get(&InfoSemanticTarget::Copy(request))
        .unwrap();
    let dismiss = *app
        .accessibility
        .info
        .nodes
        .get(&InfoSemanticTarget::Dismiss(request))
        .unwrap();
    let choice = *app
        .accessibility
        .info
        .nodes
        .get(&InfoSemanticTarget::Choice {
            request_id: request,
            id: "safe-choice".into(),
        })
        .unwrap();

    assert_eq!(node(ROOT).children(), &[INFO_DIALOG]);
    assert_eq!(node(INFO_DIALOG).role(), Role::Dialog);
    assert!(node(INFO_DIALOG).is_modal());
    assert_eq!(node(content).role(), Role::Document);
    assert!(node(content).supports_action(Action::ScrollDown));
    assert!(node(content).supports_action(Action::ScrollRight));
    assert_eq!(node(choice).role(), Role::Button);
    assert!(node(choice).supports_action(Action::Click));
    assert_eq!(snapshot.focus, dismiss);
    assert_eq!(node(copy).bounds(), Some(bounds_rect(layout.copy_button)));
    assert_eq!(
        node(dismiss).bounds(),
        Some(bounds_rect(layout.continue_button))
    );
    assert_eq!(node(content).bounds(), Some(bounds_rect(layout.content)));

    app.handle_accessibility_action(ActionRequest {
        action: Action::ScrollDown,
        target_tree: TreeId::ROOT,
        target_node: content,
        data: None,
    });
    assert!(app.info_overlay.viewport().row() > 0);
    let scrolled = app.accessibility_snapshot();
    let scrolled_content = &scrolled
        .nodes
        .iter()
        .find(|(id, _)| *id == content)
        .unwrap()
        .1;
    assert!(
        scrolled_content
            .value()
            .is_some_and(|value| !value.contains("fixture line 0"))
    );
}

#[test]
fn info_overlay_rejects_retired_choices_and_background_actions() {
    let mut app = App::new_headless(Config::default()).unwrap();
    let stale_request = show_info_fixture(&mut app);
    app.accessibility_snapshot();
    let stale_choice = *app
        .accessibility
        .info
        .nodes
        .get(&InfoSemanticTarget::Choice {
            request_id: stale_request,
            id: "safe-choice".into(),
        })
        .unwrap();

    let current = app
        .info_overlay
        .show("Replacement notice", vec!["No choices are active".into()]);
    app.accessibility_snapshot();
    app.handle_accessibility_action(ActionRequest {
        action: Action::Click,
        target_tree: TreeId::ROOT,
        target_node: stale_choice,
        data: None,
    });
    app.handle_accessibility_action(ActionRequest {
        action: Action::Click,
        target_tree: TreeId::ROOT,
        target_node: SETTINGS,
        data: None,
    });

    assert!(app.info_overlay.is_awaiting(current));
    assert!(!app.settings_panel.is_active());
}

#[test]
fn info_overlay_dismiss_action_revokes_the_request_immediately() {
    let mut app = App::new_headless(Config::default()).unwrap();
    let request = show_info_fixture(&mut app);
    app.accessibility_snapshot();
    let dismiss = *app
        .accessibility
        .info
        .nodes
        .get(&InfoSemanticTarget::Dismiss(request))
        .unwrap();
    app.handle_accessibility_action(ActionRequest {
        action: Action::Click,
        target_tree: TreeId::ROOT,
        target_node: dismiss,
        data: None,
    });
    assert!(app.info_overlay.is_active());
    assert!(!app.info_overlay.is_awaiting(request));
    app.handle_accessibility_action(ActionRequest {
        action: Action::Click,
        target_tree: TreeId::ROOT,
        target_node: dismiss,
        data: None,
    });
    assert!(!app.info_overlay.is_awaiting(request));
}

#[test]
fn info_overlay_input_is_editable_focused_and_uses_shared_row_geometry() {
    let mut app = App::new_headless(Config::default()).unwrap();
    app.info_overlay.set_reduced_motion(true);
    let request = app.info_overlay.show_input(
        "Edit setting",
        vec!["Typed value".into(), String::new(), "Enter saves".into()],
        1,
        "Value: ",
        "root",
        32,
        Vec::new(),
    );
    let layout = app.info_overlay_layout();
    let snapshot = app.accessibility_snapshot();
    let input_id = *app
        .accessibility
        .info
        .nodes
        .get(&InfoSemanticTarget::Input(request))
        .unwrap();
    let input = &snapshot
        .nodes
        .iter()
        .find(|(id, _)| *id == input_id)
        .unwrap()
        .1;
    assert_eq!(input.role(), Role::TextInput);
    assert_eq!(input.value(), Some("root"));
    assert!(input.supports_action(Action::SetValue));
    assert_eq!(snapshot.focus, input_id);
    let actual = input.bounds().unwrap();
    let expected = layout
        .source_line_rect(1, app.info_overlay.viewport().row(), 3)
        .map(bounds_rect)
        .unwrap();
    assert_eq!(actual, expected);

    app.handle_accessibility_action(ActionRequest {
        action: Action::SetValue,
        target_tree: TreeId::ROOT,
        target_node: input_id,
        data: Some(ActionData::Value("warm 🫚".into())),
    });
    assert_eq!(app.info_overlay.input_value(request), Some("warm 🫚"));
    app.handle_accessibility_action(ActionRequest {
        action: Action::SetValue,
        target_tree: TreeId::ROOT,
        target_node: input_id,
        data: Some(ActionData::Value("bad\nvalue".into())),
    });
    assert_eq!(app.info_overlay.input_value(request), Some("warm 🫚"));
}

fn show_diff_fixture(app: &mut App, id: u64) -> u64 {
    use crate::terminal::diff_detector::{DetectedDiff, DiffHunk, DiffLine};

    app.diff_review.show(
        &DetectedDiff {
            id,
            filename: "src/hermetic_fixture.rs".into(),
            hunks: vec![DiffHunk {
                header: "@@ -1,40 +1,40 @@".into(),
                lines: (0..40)
                    .map(|index| {
                        DiffLine::Added(format!("fixture row {index} {}", "x".repeat(300)))
                    })
                    .collect(),
            }],
            raw: "--- a/src/hermetic_fixture.rs\n+++ b/src/hermetic_fixture.rs".into(),
            reviewed: false,
        },
        usize::MAX,
        PaneId(0),
    )
}

#[test]
fn diff_review_is_one_scrollable_modal_tree_with_shared_geometry() {
    let mut app = App::new_headless(Config::default()).unwrap();
    app.diff_review.set_reduced_motion(true);
    let request = show_diff_fixture(&mut app, 41);
    let layout = app.diff_review_layout();
    let snapshot = app.accessibility_snapshot();
    let node = |id| {
        &snapshot
            .nodes
            .iter()
            .find(|(node_id, _)| *node_id == id)
            .unwrap()
            .1
    };
    let content = *app
        .accessibility
        .diff
        .nodes
        .get(&DiffSemanticTarget::Content(request))
        .unwrap();
    let copy = *app
        .accessibility
        .diff
        .nodes
        .get(&DiffSemanticTarget::Copy(request))
        .unwrap();
    let dismiss = *app
        .accessibility
        .diff
        .nodes
        .get(&DiffSemanticTarget::Dismiss(request))
        .unwrap();

    assert_eq!(node(ROOT).children(), &[DIFF_DIALOG]);
    assert_eq!(node(DIFF_DIALOG).role(), Role::Dialog);
    assert!(node(DIFF_DIALOG).is_modal());
    assert_eq!(node(content).role(), Role::Document);
    assert!(node(content).supports_action(Action::ScrollDown));
    assert!(node(content).supports_action(Action::ScrollRight));
    assert_eq!(snapshot.focus, dismiss);
    assert_eq!(node(copy).bounds(), Some(bounds_rect(layout.copy_button)));
    assert_eq!(
        node(dismiss).bounds(),
        Some(bounds_rect(layout.done_button))
    );
    assert_eq!(node(content).bounds(), Some(bounds_rect(layout.content)));

    app.handle_accessibility_action(ActionRequest {
        action: Action::ScrollDown,
        target_tree: TreeId::ROOT,
        target_node: content,
        data: None,
    });
    app.handle_accessibility_action(ActionRequest {
        action: Action::ScrollRight,
        target_tree: TreeId::ROOT,
        target_node: content,
        data: None,
    });
    assert!(app.diff_review.viewport().row() > 0);
    assert!(app.diff_review.viewport().column() > 0);
}

#[test]
fn diff_review_rejects_retired_and_background_semantic_actions() {
    let mut app = App::new_headless(Config::default()).unwrap();
    let stale_request = show_diff_fixture(&mut app, 51);
    app.accessibility_snapshot();
    let stale_dismiss = *app
        .accessibility
        .diff
        .nodes
        .get(&DiffSemanticTarget::Dismiss(stale_request))
        .unwrap();

    let current = show_diff_fixture(&mut app, 52);
    app.accessibility_snapshot();
    app.handle_accessibility_action(ActionRequest {
        action: Action::Click,
        target_tree: TreeId::ROOT,
        target_node: stale_dismiss,
        data: None,
    });
    app.handle_accessibility_action(ActionRequest {
        action: Action::Click,
        target_tree: TreeId::ROOT,
        target_node: SETTINGS,
        data: None,
    });

    assert!(app.diff_review.is_awaiting(current));
    assert!(!app.settings_panel.is_active());
}

#[test]
fn diff_review_dismiss_revokes_actions_during_its_exit_animation() {
    let mut app = App::new_headless(Config::default()).unwrap();
    let request = show_diff_fixture(&mut app, 61);
    app.accessibility_snapshot();
    let dismiss = *app
        .accessibility
        .diff
        .nodes
        .get(&DiffSemanticTarget::Dismiss(request))
        .unwrap();
    app.handle_accessibility_action(ActionRequest {
        action: Action::Click,
        target_tree: TreeId::ROOT,
        target_node: dismiss,
        data: None,
    });
    assert!(app.diff_review.is_active());
    assert!(!app.diff_review.is_awaiting(request));
    assert_eq!(app.diff_review.target(request), None);
}

fn show_context_fixture(app: &mut App, tab_id: usize, cwd: Option<String>) -> u64 {
    app.context_picker.open(
        cwd,
        crate::agents::AgentTarget {
            identity: crate::tabs::TerminalIdentity {
                tab_id,
                pane_id: crate::panes::PaneId(0),
            },
            agent_id: "fixture-agent".into(),
            agent_name: "Fixture Agent".into(),
        },
    )
}

#[test]
fn context_picker_is_one_modal_checkbox_tree_with_shared_geometry() {
    let mut app = App::new_headless(Config::default()).unwrap();
    app.context_picker.set_reduced_motion(true);
    let request = show_context_fixture(&mut app, 70, None);
    let layout = app.context_picker_layout();
    let snapshot = app.accessibility_snapshot();
    let node = |id| {
        &snapshot
            .nodes
            .iter()
            .find(|(node_id, _)| *node_id == id)
            .unwrap()
            .1
    };
    let list = *app
        .accessibility
        .context
        .nodes
        .get(&ContextSemanticTarget::List(request))
        .unwrap();
    let terminal = *app
        .accessibility
        .context
        .nodes
        .get(&ContextSemanticTarget::Source {
            request_id: request,
            source: crate::ui::context_picker::ContextSourceId::TerminalOutput,
        })
        .unwrap();
    let cancel = *app
        .accessibility
        .context
        .nodes
        .get(&ContextSemanticTarget::Cancel(request))
        .unwrap();
    let primary = *app
        .accessibility
        .context
        .nodes
        .get(&ContextSemanticTarget::Primary(request))
        .unwrap();

    assert_eq!(node(ROOT).children(), &[CONTEXT_DIALOG]);
    assert_eq!(node(CONTEXT_DIALOG).role(), Role::Dialog);
    assert!(node(CONTEXT_DIALOG).is_modal());
    assert_eq!(node(list).role(), Role::List);
    assert_eq!(node(terminal).role(), Role::CheckBox);
    assert!(node(terminal).supports_action(Action::Click));
    assert!(node(cancel).supports_action(Action::Click));
    assert!(node(primary).supports_action(Action::Click));
    let actual = node(list).bounds().unwrap();
    let expected = bounds_rect(layout.content);
    assert!((actual.x0 - expected.x0).abs() < 0.05);
    assert!((actual.y0 - expected.y0).abs() < 0.05);

    app.handle_accessibility_action(ActionRequest {
        action: Action::Click,
        target_tree: TreeId::ROOT,
        target_node: terminal,
        data: None,
    });
    assert!(
        app.context_picker
            .snapshot()
            .sources
            .iter()
            .find(|source| {
                source.id == crate::ui::context_picker::ContextSourceId::TerminalOutput
            })
            .unwrap()
            .selected
    );
}

#[test]
fn context_file_editor_is_settable_and_workspace_bounded_semantically() {
    let directory = tempfile::tempdir().unwrap();
    std::fs::write(directory.path().join("fixture.rs"), "fn fixture() {}\n").unwrap();
    let mut app = App::new_headless(Config::default()).unwrap();
    let request = show_context_fixture(&mut app, 71, Some(directory.path().display().to_string()));
    app.accessibility_snapshot();
    let files = *app
        .accessibility
        .context
        .nodes
        .get(&ContextSemanticTarget::Source {
            request_id: request,
            source: crate::ui::context_picker::ContextSourceId::SpecificFiles,
        })
        .unwrap();
    app.handle_accessibility_action(ActionRequest {
        action: Action::Click,
        target_tree: TreeId::ROOT,
        target_node: files,
        data: None,
    });

    let form = app.accessibility_snapshot();
    let input = *app
        .accessibility
        .context
        .nodes
        .get(&ContextSemanticTarget::FileInput(request))
        .unwrap();
    let primary = *app
        .accessibility
        .context
        .nodes
        .get(&ContextSemanticTarget::Primary(request))
        .unwrap();
    let input_node = &form
        .nodes
        .iter()
        .find(|(node_id, _)| *node_id == input)
        .unwrap()
        .1;
    assert_eq!(input_node.role(), Role::TextInput);
    assert!(input_node.supports_action(Action::SetValue));
    app.handle_accessibility_action(ActionRequest {
        action: Action::SetValue,
        target_tree: TreeId::ROOT,
        target_node: input,
        data: Some(ActionData::Value("fixture.rs".into())),
    });
    app.handle_accessibility_action(ActionRequest {
        action: Action::Click,
        target_tree: TreeId::ROOT,
        target_node: primary,
        data: None,
    });
    let source = app
        .context_picker
        .snapshot()
        .sources
        .into_iter()
        .find(|source| source.id == crate::ui::context_picker::ContextSourceId::SpecificFiles)
        .unwrap();
    assert!(source.selected);
    assert!(source.label.contains("(1)"));
}

#[test]
fn context_picker_rejects_stale_background_and_cancelled_gather_actions() {
    let mut app = App::new_headless(Config::default()).unwrap();
    let stale = show_context_fixture(&mut app, 80, None);
    app.accessibility_snapshot();
    let stale_cancel = *app
        .accessibility
        .context
        .nodes
        .get(&ContextSemanticTarget::Cancel(stale))
        .unwrap();
    let current = show_context_fixture(&mut app, 81, None);
    app.accessibility_snapshot();
    app.handle_accessibility_action(ActionRequest {
        action: Action::Click,
        target_tree: TreeId::ROOT,
        target_node: stale_cancel,
        data: None,
    });
    app.handle_accessibility_action(ActionRequest {
        action: Action::Click,
        target_tree: TreeId::ROOT,
        target_node: SETTINGS,
        data: None,
    });
    assert!(app.context_picker.is_awaiting(current));
    assert!(!app.settings_panel.is_active());

    app.context_picker
        .apply(crate::ui::context_picker::ContextPickerAction::Toggle {
            request_id: current,
            source: crate::ui::context_picker::ContextSourceId::TerminalOutput,
        });
    assert!(
        app.context_picker
            .begin_gather(current, Some("fixture output".into()))
            .is_some()
    );
    let gathering = app.accessibility_snapshot();
    let cancel = *app
        .accessibility
        .context
        .nodes
        .get(&ContextSemanticTarget::Cancel(current))
        .unwrap();
    let primary = *app
        .accessibility
        .context
        .nodes
        .get(&ContextSemanticTarget::Primary(current))
        .unwrap();
    assert!(
        gathering
            .nodes
            .iter()
            .find(|(id, _)| *id == primary)
            .unwrap()
            .1
            .is_disabled()
    );
    app.handle_accessibility_action(ActionRequest {
        action: Action::Click,
        target_tree: TreeId::ROOT,
        target_node: cancel,
        data: None,
    });
    assert!(!app.context_picker.is_awaiting(current));
    assert_eq!(
        app.context_picker.complete(current, Ok("stale".into())),
        crate::ui::context_picker::ContextPickerCompletion::Ignored
    );
}

fn show_guardrail_fixture(app: &mut App) -> u64 {
    app.guardrail.show(
        crate::terminal::guardrails::GuardrailMatch {
            command: "fixture command; never execute".into(),
            reason: "Hermetic destructive-command warning".into(),
            severity: crate::terminal::guardrails::Severity::Critical,
        },
        "Missing fixture terminal".into(),
        vec![crate::terminal::guardrails::GuardedWrite::for_pane(
            usize::MAX,
            crate::panes::PaneId(0),
            b"fixture-enter".to_vec(),
        )],
    )
}

#[test]
fn guardrail_is_assertive_modal_with_safe_default_and_shared_geometry() {
    let mut app = App::new_headless(Config::default()).unwrap();
    let request = show_guardrail_fixture(&mut app);
    let layout = app.guardrail_layout();
    let snapshot = app.accessibility_snapshot();
    let node = |id| {
        &snapshot
            .nodes
            .iter()
            .find(|(node_id, _)| *node_id == id)
            .unwrap()
            .1
    };
    let confirm = *app
        .accessibility
        .guardrail
        .nodes
        .get(&GuardrailSemanticTarget::Confirm(request))
        .unwrap();
    let cancel = *app
        .accessibility
        .guardrail
        .nodes
        .get(&GuardrailSemanticTarget::Cancel(request))
        .unwrap();

    assert_eq!(node(ROOT).children(), &[GUARDRAIL_DIALOG]);
    assert_eq!(node(GUARDRAIL_DIALOG).role(), Role::AlertDialog);
    assert!(node(GUARDRAIL_DIALOG).is_modal());
    assert_eq!(node(GUARDRAIL_REASON).role(), Role::Alert);
    assert_eq!(node(GUARDRAIL_REASON).live(), Some(Live::Assertive));
    assert_eq!(snapshot.focus, cancel, "Cancel is the safe default");
    assert!(node(cancel).supports_action(Action::Click));
    assert!(node(confirm).supports_action(Action::Click));
    assert!(
        node(confirm)
            .label()
            .is_some_and(|label| label.contains("dangerous"))
    );

    let dialog = node(GUARDRAIL_DIALOG).bounds().unwrap();
    let actual_cancel = node(cancel).bounds().unwrap();
    let expected_cancel = layout.cancel_button;
    assert!(
        (actual_cancel.x0 - (dialog.x0 + f64::from(expected_cancel.x - layout.card.x))).abs() < 0.1
    );
    assert!(
        (actual_cancel.y0 - (dialog.y0 + f64::from(expected_cancel.y - layout.card.y))).abs() < 0.1
    );
}

#[test]
fn guardrail_cancel_revokes_immediately_and_stale_confirm_cannot_retarget() {
    let mut app = App::new_headless(Config::default()).unwrap();
    let first = show_guardrail_fixture(&mut app);
    app.accessibility_snapshot();
    let stale_confirm = *app
        .accessibility
        .guardrail
        .nodes
        .get(&GuardrailSemanticTarget::Confirm(first))
        .unwrap();
    let cancel = *app
        .accessibility
        .guardrail
        .nodes
        .get(&GuardrailSemanticTarget::Cancel(first))
        .unwrap();
    app.handle_accessibility_action(ActionRequest {
        action: Action::Click,
        target_tree: TreeId::ROOT,
        target_node: cancel,
        data: None,
    });
    assert!(app.guardrail.is_active());
    assert!(!app.guardrail.is_awaiting(first));
    app.handle_accessibility_action(ActionRequest {
        action: Action::Click,
        target_tree: TreeId::ROOT,
        target_node: stale_confirm,
        data: None,
    });
    assert!(!app.guardrail.is_awaiting(first));

    let second = show_guardrail_fixture(&mut app);
    assert_ne!(first, second);
    app.handle_accessibility_action(ActionRequest {
        action: Action::Click,
        target_tree: TreeId::ROOT,
        target_node: stale_confirm,
        data: None,
    });
    assert!(app.guardrail.is_awaiting(second));
}

#[test]
fn current_guardrail_confirm_with_missing_origin_never_falls_back() {
    let mut app = App::new_headless(Config::default()).unwrap();
    let request = show_guardrail_fixture(&mut app);
    app.accessibility_snapshot();
    let confirm = *app
        .accessibility
        .guardrail
        .nodes
        .get(&GuardrailSemanticTarget::Confirm(request))
        .unwrap();
    app.handle_accessibility_action(ActionRequest {
        action: Action::Click,
        target_tree: TreeId::ROOT,
        target_node: confirm,
        data: None,
    });
    assert!(!app.guardrail.is_awaiting(request));
    assert!(app.tabs.is_none(), "no fallback terminal was created");
}

#[test]
fn a_headless_tree_spans_the_window_its_dialogs_are_laid_out_in() {
    let mut app = App::new_headless(Config::default()).unwrap();
    app.info_overlay.set_reduced_motion(true);
    let metrics = app.ui_metrics();
    let window = Rect::new(0.0, 0.0, metrics.width as f64, metrics.height as f64);
    let bounds = |snapshot: &SemanticSnapshot, id: NodeId| {
        snapshot
            .nodes
            .iter()
            .find(|(node_id, _)| *node_id == id)
            .and_then(|(_, node)| node.bounds())
            .unwrap()
    };
    assert_eq!(bounds(&app.accessibility_snapshot(), ROOT), window);

    show_info_fixture(&mut app);
    let snapshot = app.accessibility_snapshot();
    assert_eq!(bounds(&snapshot, ROOT), window);
    let dialog = bounds(&snapshot, INFO_DIALOG);
    assert_eq!(dialog, bounds_rect(app.info_overlay_layout().card));
    assert!(dialog.x0 >= window.x0 && dialog.x1 <= window.x1);
    assert!(dialog.y0 >= window.y0 && dialog.y1 <= window.y1);
}

#[test]
fn a_text_caret_counts_characters_and_an_off_boundary_one_goes_to_the_end() {
    let caret = |cursor_byte| {
        let mut node = Node::new(Role::TextInput);
        set_text_value(&mut node, SEARCH_INPUT, "aé😀b", cursor_byte);
        node.text_selection()
            .map(|selection| selection.focus.character_index)
    };
    assert_eq!(caret(None), None);
    assert_eq!(caret(Some(0)), Some(0));
    assert_eq!(caret(Some(3)), Some(2), "after the two-byte é");
    assert_eq!(caret(Some(7)), Some(3), "after the four-byte emoji");
    assert_eq!(caret(Some(2)), Some(4), "inside é");
    assert_eq!(caret(Some(99)), Some(4), "past the end");
}
