//! Overlay and panel input states, app-mediated clipboard replies, and OSC origins.

use super::*;

#[test]
fn headless_overlay_close_states_do_not_block_future_input() {
    let mut app = App::new_headless(Config::default()).unwrap();
    app.command_palette.open();
    assert!(app.command_palette.is_active());
    app.command_palette.close();
    assert!(!app.command_palette.is_active());

    app.domain_picker.open(&app.config.domains);
    assert!(app.domain_picker.is_active());
    app.domain_picker.close();
    assert!(!app.domain_picker.is_active());

    app.settings_panel.toggle();
    assert!(app.settings_panel.is_active());
    app.settings_panel.close();
    assert!(!app.settings_panel.is_active());
    app.settings_panel.toggle();
    assert!(app.settings_panel.is_active());

    app.commands_panel.toggle();
    assert!(app.commands_panel.is_open());
    app.commands_panel.close();
    assert!(!app.commands_panel.is_open());
}

#[test]
fn settings_toggle_switches_directly_from_peer_side_panels() {
    let mut app = App::new_headless(Config::default()).unwrap();

    app.commands_panel.toggle();
    assert!(app.commands_panel.is_open());
    app.toggle_settings_surface();
    assert!(!app.commands_panel.is_open());
    assert!(app.settings_panel.is_active());

    app.toggle_settings_surface();
    assert!(!app.settings_panel.is_active());
    app.toggle_settings_surface();
    assert!(app.settings_panel.is_active());
}

#[test]
fn app_mediated_clipboard_reply_returns_to_requesting_pane_not_focus() {
    let mut app = App::new_headless(Config::default()).unwrap();
    let mut tabs = TabManager::for_test(ShellConfig::default());
    let tab_id = tabs
        .new_tab_with_command(
            "/bin/sh",
            &crate::test_support::sleeping_args(),
            None,
            Some("pane-owner"),
            None,
            HashMap::new(),
        )
        .unwrap();
    let script = concat!(
        "stty raw -echo; ",
        "reply=$(dd bs=1 count=9 2>/dev/null); ",
        "printf '\\rreply='; ",
        "printf '%s' \"$reply\" | od -An -tx1; ",
        "printf '\\r\\n'"
    );
    let shell = ShellConfig {
        program: Some("/bin/sh".into()),
        args: vec!["-c".into(), script.into()],
        env: Default::default(),
    };
    let mut panes = PaneManager::for_test(
        80,
        24,
        shell,
        1_000,
        8 * 1024 * 1024,
        800.0,
        480.0,
        10.0,
        20.0,
    )
    .unwrap();
    let requesting_pane = panes.split(SplitDirection::Vertical).unwrap();
    panes.focus(PaneId(0));
    tabs.active_tab_mut().unwrap().panes = panes;
    app.tabs = Some(tabs);

    app.handle_terminal_event(RoutedTerminalEvent {
        origin: TerminalEventOrigin {
            tab_id,
            pane_id: requesting_pane,
        },
        event: TerminalEvent::ClipboardRequest {
            selection: ClipboardRequest::Clipboard,
            reply_uses_8bit_controls: false,
        },
    });

    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        app.tabs.as_mut().unwrap().poll_events();
        let text = app
            .active_panes()
            .get_pane(requesting_pane)
            .unwrap()
            .terminal
            .grid()
            .plain_lines()
            .join("\n");
        if text.contains("1b 5d 35 32 3b 63 3b 1b 5c") {
            break;
        }
        std::thread::sleep(Duration::from_millis(2));
    }

    let panes = app.active_panes();
    let requesting_text = panes
        .get_pane(requesting_pane)
        .unwrap()
        .terminal
        .grid()
        .plain_lines()
        .join("\n");
    let focused_text = panes
        .get_pane(PaneId(0))
        .unwrap()
        .terminal
        .grid()
        .plain_lines()
        .join("\n");
    assert!(requesting_text.contains("1b 5d 35 32 3b 63 3b 1b 5c"));
    assert!(!focused_text.contains("reply="));
    assert_eq!(panes.focused_id(), PaneId(0));

    // Give each pane deliberately different state, then prove all global
    // surfaces rebind when pane focus moves.
    let panes = app.active_panes_mut();
    for index in 0..30 {
        panes
            .get_pane_mut(PaneId(0))
            .unwrap()
            .terminal
            .process(format!("left-{index}\r\n").as_bytes());
        panes
            .get_pane_mut(requesting_pane)
            .unwrap()
            .terminal
            .process(format!("right-{index}\r\n").as_bytes());
    }
    panes
        .get_pane_mut(PaneId(0))
        .unwrap()
        .terminal
        .process(b"alpha https://l.io\r\n");
    panes
        .get_pane_mut(requesting_pane)
        .unwrap()
        .terminal
        .process(b"beta https://r.io\r\n\x1b[?2026h");
    panes
        .get_pane_mut(PaneId(0))
        .unwrap()
        .terminal
        .set_selection(Some(termeric_core::Selection::new(
            termeric_core::Point::new(0, 0),
            termeric_core::SelectionType::Simple,
        )));
    panes.focus(PaneId(0));

    app.search.start();
    for character in "alpha".chars() {
        app.with_search_and_active_grid(|search, grid| search.push_char(character, grid));
    }
    app.with_search_and_active_grid(|search, grid| search.finish(grid));
    assert!(app.search.match_count() > 0);
    assert!(app.active_terminal().unwrap().selection().is_some());
    assert!(!app.active_terminal().unwrap().is_synchronized_output());

    app.active_panes_mut().focus(requesting_pane);
    app.refresh_focused_terminal_surfaces();

    assert_eq!(app.search.match_count(), 0);
    assert!(app.active_terminal().unwrap().selection().is_none());
    assert!(app.active_terminal().unwrap().is_synchronized_output());
    assert!(
        (0..app.active_terminal().unwrap().grid().rows()).any(|row| {
            app.url_cache
                .url_at(row, 5)
                .is_some_and(|url| url.url == "https://r.io")
        })
    );

    app.active_terminal_mut()
        .unwrap()
        .grid_mut()
        .set_scroll_offset(0);
    app.handle_scroll(3.0);
    assert!(app.active_terminal().unwrap().grid().scroll_offset() > 0);
    assert_eq!(
        app.active_panes()
            .get_pane(PaneId(0))
            .unwrap()
            .terminal
            .grid()
            .scroll_offset(),
        0
    );
}

#[test]
fn osc_titles_and_dynamic_colors_apply_to_exact_inactive_tab_and_pane_origins() {
    let mut app = App::new_headless(Config::default()).unwrap();
    let mut tabs = TabManager::new(40, 8, ShellConfig::default(), 1_000, 8 * 1024 * 1024);
    let dormant_tab_id = tabs
        .new_tab_with_command(
            "/bin/sh",
            &crate::test_support::sleeping_args(),
            None,
            Some("dormant"),
            None,
            HashMap::new(),
        )
        .unwrap();
    let mut panes = PaneManager::for_test(
        40,
        8,
        crate::test_support::sleeping_shell(),
        1_000,
        8 * 1024 * 1024,
        800.0,
        320.0,
        10.0,
        20.0,
    )
    .unwrap();
    let second_pane = panes.split(SplitDirection::Vertical).unwrap();
    panes.focus(PaneId(0));
    tabs.active_tab_mut().unwrap().panes = panes;
    let active_tab_id = tabs
        .new_tab_with_command(
            "/bin/sh",
            &crate::test_support::sleeping_args(),
            None,
            Some("active"),
            None,
            HashMap::new(),
        )
        .unwrap();
    app.tabs = Some(tabs);

    let first_origin = TerminalEventOrigin {
        tab_id: dormant_tab_id,
        pane_id: PaneId(0),
    };
    let second_origin = TerminalEventOrigin {
        tab_id: dormant_tab_id,
        pane_id: second_pane,
    };
    app.handle_terminal_event(RoutedTerminalEvent {
        origin: first_origin,
        event: TerminalEvent::DynamicColor(termeric_core::DynamicColorOperation::Set {
            target: termeric_core::DynamicColorTarget::Palette(3),
            color: termeric_core::Color::rgb(255, 0, 0),
        }),
    });
    app.handle_terminal_event(RoutedTerminalEvent {
        origin: second_origin,
        event: TerminalEvent::DynamicColor(termeric_core::DynamicColorOperation::Set {
            target: termeric_core::DynamicColorTarget::Palette(3),
            color: termeric_core::Color::rgb(0, 0, 255),
        }),
    });
    app.handle_terminal_event(RoutedTerminalEvent {
        origin: first_origin,
        // Chrome drops what could reorder or break the title line.
        event: TerminalEvent::TitleChanged("pane\u{202e}-build\u{2028}".into()),
    });
    app.handle_terminal_event(RoutedTerminalEvent {
        origin: TerminalEventOrigin {
            tab_id: active_tab_id,
            pane_id: PaneId(0),
        },
        event: TerminalEvent::TitleChanged("active-editor".into()),
    });

    let tabs = app.tabs.as_ref().unwrap();
    assert_eq!(
        tabs.terminal_for_origin(first_origin)
            .unwrap()
            .dynamic_colors()
            .palette(3),
        Some(termeric_core::Color::rgb(255, 0, 0))
    );
    assert_eq!(
        tabs.terminal_for_origin(second_origin)
            .unwrap()
            .dynamic_colors()
            .palette(3),
        Some(termeric_core::Color::rgb(0, 0, 255))
    );
    let titles = tabs.tab_info();
    assert_eq!(titles[0].title, "pane-build");
    assert_eq!(titles[1].title, "active-editor");
    assert_eq!(
        tabs.iter()
            .find(|(_, tab)| tab.id == dormant_tab_id)
            .and_then(|(_, tab)| tab.panes.get_pane(PaneId(0)))
            .and_then(|pane| pane.title.as_deref()),
        Some("pane-build")
    );
    assert_eq!(tabs.active_index(), 1);

    let tabs = app.tabs.as_mut().unwrap();
    assert!(!tabs.set_title_for_origin(
        TerminalEventOrigin {
            tab_id: dormant_tab_id,
            pane_id: PaneId(u32::MAX),
        },
        "stale-pane-title".into(),
    ));
    assert_eq!(tabs.tab_info()[0].title, "pane-build");

    assert!(tabs.set_title_for_origin(first_origin, String::new()));
    assert_eq!(
        tabs.iter()
            .find(|(_, tab)| tab.id == dormant_tab_id)
            .and_then(|(_, tab)| tab.panes.get_pane(PaneId(0)))
            .and_then(|pane| pane.title.as_deref()),
        None,
        "an empty OSC title must clear persisted pane title metadata"
    );
}
