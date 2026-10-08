//! Panes: focus, ownership through splits and collapse, read-only, and closing.

use super::*;

#[test]
fn keyboard_mode_snapshot_follows_the_focused_pane() {
    let mut app = App::new_headless(Config::default()).unwrap();
    let shell = crate::test_support::sleeping_shell();
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
    let right = panes
        .split(
            SplitDirection::Vertical,
            crate::config::PaneCwdMode::Focused,
        )
        .unwrap();
    panes.get_pane_mut(PaneId(0)).unwrap().terminal.set_mode(
        termeric_core::term::TerminalMode::CURSOR_KEYS_APPLICATION,
        true,
    );
    panes.get_pane_mut(PaneId(0)).unwrap().terminal.process(
        b"\x1b[=1u\x1b[?1060h\x1b[>0;5m\x1b[>0;6f\x1b[>1;3m\x1b[>2;3m\x1b[>3;3m\x1b[>4;1m\x1b[>6;3m\x1b[>7;3m",
    );
    panes
        .get_pane_mut(right)
        .unwrap()
        .terminal
        .set_mode(termeric_core::term::TerminalMode::KEYPAD_APPLICATION, true);
    panes.get_pane_mut(right).unwrap().terminal.process(
        b"\x1b[=2u\x1b[?1061h\x1b[>0;9m\x1b[>0;10f\x1b[>1;4m\x1b[>1;1f\x1b[>2;4m\x1b[>2;1f\x1b[>3;4m\x1b[>3;1f\x1b[>4;2m\x1b[>4;1f\x1b[>6;4m\x1b[>6;1f\x1b[>7;4m\x1b[>7;1f",
    );

    let mut tabs = TabManager::for_test(ShellConfig::default());
    tabs.new_tab_with_command(
        "/bin/sh",
        &crate::test_support::sleeping_args(),
        None,
        Some("keyboard-mode"),
        None,
        HashMap::new(),
    )
    .unwrap();
    tabs.active_tab_mut().unwrap().panes = panes;
    app.tabs = Some(tabs);

    app.active_panes_mut().focus(PaneId(0));
    assert!(
        app.active_keyboard_mode()
            .contains(termeric_core::term::TerminalMode::CURSOR_KEYS_APPLICATION)
    );
    assert!(
        !app.active_keyboard_mode()
            .contains(termeric_core::term::TerminalMode::KEYPAD_APPLICATION)
    );
    assert_eq!(
        app.active_kitty_keyboard_flags(),
        termeric_core::term::KittyKeyboardFlags::DISAMBIGUATE_ESCAPE_CODES
    );
    assert_eq!(
        app.active_terminal()
            .unwrap()
            .xterm_modify_other_keys()
            .level(),
        1
    );
    assert_eq!(
        app.active_terminal().unwrap().xterm_keyboard_type(),
        termeric_core::term::XtermKeyboardType::Legacy
    );
    assert_eq!(
        app.active_terminal().unwrap().xterm_keyboard_resource(),
        termeric_core::term::XtermKeyboardResource::new(5, 6)
    );
    assert_eq!(
        app.active_terminal()
            .unwrap()
            .xterm_key_resource(termeric_core::term::XtermKeyResource::Cursor)
            .mode(),
        termeric_core::term::XtermKeyMode::Private
    );
    assert_eq!(
        app.active_terminal()
            .unwrap()
            .xterm_key_resource(termeric_core::term::XtermKeyResource::Function)
            .mode(),
        termeric_core::term::XtermKeyMode::Private
    );
    assert_eq!(
        app.active_terminal()
            .unwrap()
            .xterm_key_resource(termeric_core::term::XtermKeyResource::Keypad)
            .mode(),
        termeric_core::term::XtermKeyMode::Private
    );
    assert_eq!(
        app.active_terminal()
            .unwrap()
            .xterm_key_resource(termeric_core::term::XtermKeyResource::Modifier)
            .mode(),
        termeric_core::term::XtermKeyMode::Private
    );
    assert_eq!(
        app.active_terminal()
            .unwrap()
            .xterm_key_resource(termeric_core::term::XtermKeyResource::Special)
            .mode(),
        termeric_core::term::XtermKeyMode::Private
    );
    assert_eq!(
        app.active_terminal()
            .unwrap()
            .xterm_modify_other_keys()
            .format(),
        termeric_core::term::XtermKeyFormat::FunctionKey27
    );

    app.active_panes_mut().focus(right);
    assert!(
        app.active_keyboard_mode()
            .contains(termeric_core::term::TerminalMode::KEYPAD_APPLICATION)
    );
    assert!(
        !app.active_keyboard_mode()
            .contains(termeric_core::term::TerminalMode::CURSOR_KEYS_APPLICATION)
    );
    assert_eq!(
        app.active_kitty_keyboard_flags(),
        termeric_core::term::KittyKeyboardFlags::REPORT_EVENT_TYPES
    );
    assert_eq!(
        app.active_terminal()
            .unwrap()
            .xterm_modify_other_keys()
            .level(),
        2
    );
    assert_eq!(
        app.active_terminal().unwrap().xterm_keyboard_type(),
        termeric_core::term::XtermKeyboardType::Vt220
    );
    assert_eq!(
        app.active_terminal().unwrap().xterm_keyboard_resource(),
        termeric_core::term::XtermKeyboardResource::new(9, 10)
    );
    assert_eq!(
        app.active_terminal()
            .unwrap()
            .xterm_key_resource(termeric_core::term::XtermKeyResource::Cursor),
        termeric_core::term::XtermKeySetting::new(
            termeric_core::term::XtermKeyMode::Extended,
            termeric_core::term::XtermKeyFormat::CsiU
        )
    );
    assert_eq!(
        app.active_terminal()
            .unwrap()
            .xterm_key_resource(termeric_core::term::XtermKeyResource::Function),
        termeric_core::term::XtermKeySetting::new(
            termeric_core::term::XtermKeyMode::Extended,
            termeric_core::term::XtermKeyFormat::CsiU
        )
    );
    assert_eq!(
        app.active_terminal()
            .unwrap()
            .xterm_key_resource(termeric_core::term::XtermKeyResource::Keypad),
        termeric_core::term::XtermKeySetting::new(
            termeric_core::term::XtermKeyMode::Extended,
            termeric_core::term::XtermKeyFormat::CsiU
        )
    );
    assert_eq!(
        app.active_terminal()
            .unwrap()
            .xterm_key_resource(termeric_core::term::XtermKeyResource::Modifier),
        termeric_core::term::XtermKeySetting::new(
            termeric_core::term::XtermKeyMode::Extended,
            termeric_core::term::XtermKeyFormat::CsiU
        )
    );
    assert_eq!(
        app.active_terminal()
            .unwrap()
            .xterm_key_resource(termeric_core::term::XtermKeyResource::Special),
        termeric_core::term::XtermKeySetting::new(
            termeric_core::term::XtermKeyMode::Extended,
            termeric_core::term::XtermKeyFormat::CsiU
        )
    );
    assert_eq!(
        app.active_terminal()
            .unwrap()
            .xterm_modify_other_keys()
            .format(),
        termeric_core::term::XtermKeyFormat::CsiU
    );
}

#[test]
fn copy_mode_is_owned_by_one_focused_pane_and_disappears_after_focus_changes() {
    let mut app = App::new_headless(Config::default()).unwrap();
    let shell = crate::test_support::sleeping_shell();
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
    let right = panes
        .split(
            SplitDirection::Vertical,
            crate::config::PaneCwdMode::Focused,
        )
        .unwrap();
    let mut tabs = TabManager::for_test(ShellConfig::default());
    tabs.new_tab_with_command(
        "/bin/sh",
        &crate::test_support::sleeping_args(),
        None,
        Some("copy-mode-owner"),
        None,
        HashMap::new(),
    )
    .unwrap();
    tabs.active_tab_mut().unwrap().panes = panes;
    app.tabs = Some(tabs);

    app.active_panes_mut().focus(PaneId(0));
    app.enter_copy_mode();
    assert!(app.copy_mode_render_info().is_some());

    app.active_panes_mut().focus(right);
    assert!(app.copy_mode_render_info().is_none());
    assert!(
        app.tabs
            .as_ref()
            .unwrap()
            .active_tab()
            .unwrap()
            .focused_terminal()
            .unwrap()
            .selection()
            .is_none()
    );
}

#[test]
fn mouse_focus_and_bracketed_paste_reach_the_focused_pane_in_order() {
    let script = concat!(
        "stty raw -echo; ",
        "printf '\rREADY\r\n'; ",
        "bytes=$(dd bs=1 count=38 2>/dev/null | od -An -tx1); ",
        "printf '\rprotocol=%s\r\n' \"$bytes\"; ",
        "sleep 0.1"
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
    let receiving_pane = panes
        .split(
            SplitDirection::Vertical,
            crate::config::PaneCwdMode::Focused,
        )
        .unwrap();
    panes.focus(receiving_pane);
    panes
        .get_pane_mut(receiving_pane)
        .unwrap()
        .terminal
        .set_mode(
            termeric_core::term::TerminalMode::FOCUS_EVENTS
                | termeric_core::term::TerminalMode::BRACKETED_PASTE
                | termeric_core::term::TerminalMode::MOUSE_TRACKING
                | termeric_core::term::TerminalMode::MOUSE_SGR,
            true,
        );

    let mut app = App::new_headless(Config::default()).unwrap();
    let mut tabs = TabManager::for_test(ShellConfig::default());
    tabs.new_tab_with_command(
        "/bin/sh",
        &["-c".into(), "sleep 3".into()],
        None,
        Some("protocol-routing"),
        None,
        HashMap::new(),
    )
    .unwrap();
    tabs.active_tab_mut().unwrap().panes = panes;
    app.tabs = Some(tabs);

    let ready_deadline = Instant::now() + Duration::from_secs(3);
    let mut receiver_ready = false;
    while Instant::now() < ready_deadline {
        app.tabs.as_mut().unwrap().poll_events();
        let ready = app
            .active_panes()
            .get_pane(receiving_pane)
            .unwrap()
            .terminal
            .grid()
            .plain_lines()
            .join("\n")
            .contains("READY");
        if ready {
            receiver_ready = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    assert!(
        receiver_ready,
        "focused pane PTY byte receiver did not become ready"
    );

    // Duplicate native notifications must not produce duplicate CSI I/O reports.
    app.handle_window_focus_change(false);
    app.handle_window_focus_change(false);
    app.handle_window_focus_change(true);
    app.handle_window_focus_change(true);
    app.write_paste_text("P\n");

    let mode = app.active_keyboard_mode();
    for kind in [
        crate::input::MouseReportKind::Press(crate::input::MouseReportButton::Left),
        crate::input::MouseReportKind::Release(crate::input::MouseReportButton::Left),
    ] {
        let report = crate::input::encode_mouse_report(
            mode,
            kind,
            2,
            3,
            winit::keyboard::ModifiersState::empty(),
        )
        .unwrap();
        assert!(app.write_terminal_protocol_input(&report));
    }

    let expected = concat!(
        "1b 5b 4f 1b 5b 49 ",
        "1b 5b 32 30 30 7e 50 0a 1b 5b 32 30 31 7e ",
        "1b 5b 3c 30 3b 33 3b 34 4d ",
        "1b 5b 3c 30 3b 33 3b 34 6d"
    );
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut output = String::new();
    while Instant::now() < deadline {
        app.tabs.as_mut().unwrap().poll_events();
        output = app
            .active_panes()
            .get_pane(receiving_pane)
            .unwrap()
            .terminal
            .grid()
            .plain_lines()
            .join("\n");
        if output
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
            .contains(expected)
        {
            break;
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    assert!(
        output
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
            .contains(expected),
        "focused pane did not receive the ordered protocol stream: {output}"
    );
}

#[test]
fn splitting_and_closing_back_to_one_pane_keep_the_first_terminal_without_orphans() {
    let mut tab = Tab::new_with_command(
        0,
        80,
        24,
        &crate::tabs::CommandLaunch {
            program: "/bin/sh".into(),
            args: vec![
                "-c".into(),
                "printf 'before\\n'; sleep 0.05; printf 'after\\n'; sleep 5".into(),
            ],
            title: Some("owner".into()),
            ..crate::tabs::CommandLaunch::default()
        },
        1_000,
        8 * 1024 * 1024,
    )
    .unwrap();
    let original_pid = tab
        .panes
        .lone_pane()
        .and_then(|pane| pane.pty.child_id())
        .unwrap();
    let original_cwd = std::fs::read_link(format!("/proc/{original_pid}/cwd")).unwrap();

    let before_deadline = Instant::now() + Duration::from_secs(2);
    while Instant::now() < before_deadline
        && !tab
            .focused_terminal()
            .unwrap()
            .rows_text(0, 23)
            .contains("before")
    {
        tab.poll_events();
        std::thread::sleep(Duration::from_millis(2));
    }

    let shell = ShellConfig {
        program: Some("/bin/sh".into()),
        args: vec!["-c".into(), "sleep 5".into()],
        env: Default::default(),
    };
    tab.panes_for_test(shell, 800.0, 480.0, 10.0, 20.0);
    for cycle in 0..3 {
        let panes = &mut tab.panes;
        assert_eq!(panes.pane_count(), 1);
        assert_eq!(
            panes.get_pane(PaneId(0)).unwrap().pty.child_id(),
            Some(original_pid)
        );

        let added = panes
            .split(
                SplitDirection::Vertical,
                crate::config::PaneCwdMode::Focused,
            )
            .unwrap();
        assert_eq!(
            added,
            PaneId(cycle + 1),
            "retired pane IDs must not be reused"
        );
        assert_eq!(
            panes.get_dividers()[0].target,
            crate::panes::DividerTarget::Local(u64::from(cycle + 1)),
            "retired divider IDs must not be reused"
        );
        let added_pid = panes.get_pane(added).unwrap().pty.child_id().unwrap();
        assert_ne!(added_pid, original_pid);
        assert_eq!(
            std::fs::read_link(format!("/proc/{added_pid}/cwd")).unwrap(),
            original_cwd
        );

        if cycle == 0 {
            let after_deadline = Instant::now() + Duration::from_secs(2);
            while Instant::now() < after_deadline
                && !panes
                    .get_pane(PaneId(0))
                    .unwrap()
                    .terminal
                    .rows_text(0, 23)
                    .contains("after")
            {
                panes.poll_events();
                std::thread::sleep(Duration::from_millis(2));
            }
        }

        assert!(panes.close_focused());
        assert_eq!(panes.pane_count(), 1);
        assert_eq!(
            tab.panes.lone_pane().and_then(|pane| pane.pty.child_id()),
            Some(original_pid)
        );
        assert!(
            tab.focused_terminal()
                .unwrap()
                .rows_text(0, 23)
                .contains("before")
        );

        let exit_deadline = Instant::now() + Duration::from_secs(1);
        while std::path::Path::new(&format!("/proc/{added_pid}")).exists()
            && Instant::now() < exit_deadline
        {
            std::thread::sleep(Duration::from_millis(2));
        }
        assert!(!std::path::Path::new(&format!("/proc/{added_pid}")).exists());
    }

    assert!(
        tab.focused_terminal()
            .unwrap()
            .rows_text(0, 23)
            .contains("after")
    );
    drop(tab);
    let exit_deadline = Instant::now() + Duration::from_secs(1);
    while std::path::Path::new(&format!("/proc/{original_pid}")).exists()
        && Instant::now() < exit_deadline
    {
        std::thread::sleep(Duration::from_millis(2));
    }
    assert!(!std::path::Path::new(&format!("/proc/{original_pid}")).exists());
}

#[test]
fn pane_identity_survives_closing_back_to_one_pane_and_the_next_split() {
    let shell = crate::test_support::sleeping_shell();
    let mut tab = Tab::new_with_command(
        0,
        80,
        24,
        &crate::tabs::CommandLaunch {
            program: "/bin/sh".into(),
            args: crate::test_support::sleeping_args(),
            title: Some("stable-pane".into()),
            ..crate::tabs::CommandLaunch::default()
        },
        1_000,
        8 * 1024 * 1024,
    )
    .unwrap();
    let panes = tab.panes_for_test(shell.clone(), 800.0, 480.0, 10.0, 20.0);
    assert_eq!(
        panes
            .split(
                SplitDirection::Vertical,
                crate::config::PaneCwdMode::Focused
            )
            .unwrap(),
        PaneId(1)
    );
    assert!(panes.close(PaneId(0)));
    assert_eq!(tab.panes.focused_id(), PaneId(1));
    let snapshot = tab.capture_workspace_tab(0, false).unwrap();
    assert_eq!(snapshot.primary_pane, 1);
    assert_eq!(snapshot.focused_pane, 1);
    assert_eq!(snapshot.panes[0].id, 1);
    let mut restored = TabManager::for_test(shell.clone());
    restored
        .new_shell_tab_from_workspace(&snapshot, &HashMap::new())
        .unwrap();
    assert_eq!(restored.active_tab().unwrap().panes.focused_id(), PaneId(1));

    assert!(tab.panes.get_pane(PaneId(1)).is_some());
    assert_eq!(
        tab.panes
            .split(
                SplitDirection::Horizontal,
                crate::config::PaneCwdMode::Focused
            )
            .unwrap(),
        PaneId(2)
    );
}

#[test]
fn collapsed_tab_rejects_retired_pane_and_divider_requests_after_resplit() {
    let (mut app, directory, mut entry, original_captures) = harvest_rerun_capture_fixture();
    app.config.security.allow_gui_control_input = true;
    let tab_id = entry.source.tab_id;
    // Keep a history action addressed to a pane that is about to disappear.
    entry.source.pane_id = 1;
    let original_pid = app
        .active_panes()
        .get_pane(PaneId(0))
        .unwrap()
        .pty
        .child_id();
    assert_eq!(
        app.close_pane_for(Requester::Control, tab_id as u64, 2)
            .unwrap(),
        (2, 1)
    );
    assert_eq!(
        app.close_pane_for(Requester::Control, tab_id as u64, 1)
            .unwrap(),
        (1, 0)
    );
    let tab = app.tabs.as_mut().unwrap().active_tab_mut().unwrap();
    assert!(tab.panes.pane_count() == 1);
    assert_eq!(
        tab.panes.lone_pane().and_then(|pane| pane.pty.child_id()),
        original_pid
    );
    let shell = ShellConfig {
        program: Some("/bin/sh".into()),
        args: vec![
            "-c".into(),
            "stty raw -echo; printf READY; cat > \"$CAPTURE_DIR/$$\"".into(),
        ],
        env: std::collections::BTreeMap::from([(
            "CAPTURE_DIR".into(),
            directory.path().display().to_string(),
        )]),
    };
    // Only the renderer-dependent first adoption is performed directly; all
    // closure and stale control requests below use the real app handlers.
    let panes = tab.panes_for_test(shell, 800.0, 480.0, 10.0, 20.0);
    let replacement = panes
        .split(
            SplitDirection::Vertical,
            crate::config::PaneCwdMode::Focused,
        )
        .unwrap();
    assert_eq!(replacement, PaneId(3));
    assert_eq!(
        panes.get_dividers()[0].target,
        crate::panes::DividerTarget::Local(3)
    );
    let replacement_pid = panes.get_pane(replacement).unwrap().pty.child_id().unwrap();
    let survivor_capture = original_captures
        .iter()
        .find(|(origin, _)| {
            *origin
                == crate::tabs::TerminalEventOrigin {
                    tab_id,
                    pane_id: PaneId(0),
                }
        })
        .unwrap()
        .clone();
    let captures = vec![
        survivor_capture,
        (
            crate::tabs::TerminalEventOrigin {
                tab_id,
                pane_id: replacement,
            },
            directory.path().join(replacement_pid.to_string()),
        ),
    ];
    let deadline = Instant::now() + std::time::Duration::from_secs(3);
    loop {
        app.process_pty_events();
        if app
            .tabs
            .as_ref()
            .unwrap()
            .terminal_for_origin(captures[1].0)
            .unwrap()
            .rows_text(0, 23)
            .contains("READY")
        {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "replacement capture did not become ready"
        );
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    for retired in [1, 2] {
        assert!(
            app.send_text_for(
                Requester::Control,
                tab_id as u64,
                retired,
                "must not arrive",
                false,
                true
            )
            .unwrap_err()
            .contains("does not exist")
        );
        assert!(
            app.focus_pane_for(Requester::Control, tab_id as u64, retired)
                .unwrap_err()
                .contains("does not exist")
        );
        assert!(
            app.close_pane_for(Requester::Control, tab_id as u64, retired)
                .unwrap_err()
                .contains("does not exist")
        );
        assert!(
            app.resize_control_divider(tab_id as u64, u64::from(retired), 700)
                .unwrap_err()
                .contains("does not exist")
        );
    }
    app.execute_sprout_action(crate::harvest::sprouts::SproutAction::Rerun, Some(entry));
    let panes = app.active_panes();
    assert_eq!(panes.focused_id(), replacement);
    assert_eq!(panes.pane_count(), 2);
    assert_eq!(panes.get_dividers()[0].ratio_millis, 500);
    assert_eq!(
        panes.get_pane(PaneId(0)).unwrap().pty.child_id(),
        original_pid
    );
    assert!(
        fenced_harvest_capture(&mut app, &captures)
            .iter()
            .all(Vec::is_empty)
    );
    // Fresh IDs still route successfully; the fixture captures, never executes.
    app.send_text_for(
        Requester::Control,
        tab_id as u64,
        replacement.0,
        "fresh",
        false,
        false,
    )
    .unwrap();
    assert_eq!(
        fenced_harvest_capture(&mut app, &captures),
        vec![b"\0".to_vec(), b"\0fresh".to_vec()]
    );
}

#[test]
fn pane_allocation_history_survives_failed_split_and_exhaustion() {
    for next_pane in [u32::MAX, u32::MAX - 1] {
        let (mut app, _directory, _, _) = harvest_rerun_capture_fixture_in(false);
        let tab = app.tabs.as_mut().unwrap().active_tab_mut().unwrap();
        let original_pid = tab.panes.lone_pane().and_then(|pane| pane.pty.child_id());
        let mut shell = ShellConfig {
            program: Some("/definitely/missing/termeric-shell".into()),
            args: vec!["-c".into(), "sleep 5".into()],
            env: Default::default(),
        };
        let panes = tab.panes_for_test(shell.clone(), 800.0, 480.0, 10.0, 20.0);
        panes.set_next_pane_id_for_test(next_pane);
        assert!(
            panes
                .split(
                    SplitDirection::Vertical,
                    crate::config::PaneCwdMode::Focused
                )
                .is_err()
        );
        assert_eq!(panes.pane_count(), 1);
        shell.program = Some("/bin/sh".into());
        panes.adopt_primary_shell(shell);
        let spawned = next_pane == u32::MAX - 1;
        if spawned {
            let last = panes
                .split(
                    SplitDirection::Vertical,
                    crate::config::PaneCwdMode::Focused,
                )
                .unwrap();
            assert_eq!(last, PaneId(u32::MAX - 1));
            assert!(panes.close(last));
        }
        assert_eq!(
            tab.panes.lone_pane().and_then(|pane| pane.pty.child_id()),
            original_pid
        );
        let panes = &mut tab.panes;
        assert!(
            panes
                .split(
                    SplitDirection::Vertical,
                    crate::config::PaneCwdMode::Focused
                )
                .unwrap_err()
                .to_string()
                .contains("pane ID space exhausted")
        );
        assert_eq!(panes.pane_count(), 1);
    }
}

#[test]
fn pane_read_only_blocks_user_routes_but_not_protocol_replies_and_follows_ownership() {
    let shell = crate::test_support::sleeping_shell();
    let mut tab = Tab::new_with_command(
        0,
        80,
        24,
        &crate::tabs::CommandLaunch {
            program: "/bin/sh".into(),
            args: shell.args.clone(),
            title: Some("read-only-pane".into()),
            ..crate::tabs::CommandLaunch::default()
        },
        1_000,
        8 * 1024 * 1024,
    )
    .unwrap();
    assert!(tab.toggle_focused_pane_read_only());
    assert_eq!(
        tab.write(b"blocked").unwrap_err().kind(),
        std::io::ErrorKind::PermissionDenied
    );
    assert!(
        tab.focused_pane_mut()
            .unwrap()
            .write_protocol_reply(b"")
            .is_ok()
    );

    let primary = tab.panes.focused_id();
    let panes = tab.panes_for_test(shell, 800.0, 480.0, 10.0, 20.0);
    assert!(panes.get_pane(primary).unwrap().read_only);
    assert_eq!(
        panes.write_to(primary, b"blocked").unwrap_err().kind(),
        std::io::ErrorKind::PermissionDenied
    );
    let writable = panes
        .split(
            SplitDirection::Vertical,
            crate::config::PaneCwdMode::Focused,
        )
        .unwrap();
    assert!(!panes.get_pane(writable).unwrap().read_only);
    assert!(panes.write_to(writable, b"").is_ok());
    assert!(panes.close(writable));

    assert!(tab.focused_pane_read_only());
    assert!(!tab.toggle_focused_pane_read_only());
    assert!(tab.write(b"").is_ok());
}

#[test]
fn read_only_focused_pane_rejects_keyboard_and_running_gui_control_input() {
    let mut config = Config {
        shell: crate::test_support::sleeping_shell(),
        ..Config::default()
    };
    config.security.allow_gui_control_input = true;
    let mut app = App::new_headless_with_tabs(config);
    app.toggle_focused_pane_read_only();

    let tab = app.tabs.as_ref().unwrap().active_tab().unwrap();
    let tab_id = tab.id as u64;
    let pane_id = tab.panes.focused_id().0;
    assert!(tab.focused_pane_read_only());
    assert!(app.tabs.as_ref().unwrap().tab_info()[0].read_only);
    assert!(!app.write_terminal_protocol_input(b"blocked"));

    app.input_line_buffer = "stale-shell-state".into();
    app.git_completion =
        crate::app::git_completion::GitCompletionPopup::new(vec!["stale-branch".into()]);
    app.write_to_pty(b"more");
    assert!(app.input_line_buffer.is_empty());
    assert!(app.git_completion.is_none());

    app.handle_ime_event(winit::event::Ime::Preedit("blocked".into(), None));
    assert!(app.ime.preedit().is_none());
    app.handle_ime_event(winit::event::Ime::Commit("blocked".into()));
    assert!(app.input_line_buffer.is_empty());

    let dropped_file = tempfile::NamedTempFile::new().unwrap();
    let drop_error = app.dropped_path_payload(dropped_file.path()).unwrap_err();
    assert!(drop_error.contains("read-only"), "{drop_error}");

    app.tabs
        .as_mut()
        .unwrap()
        .active_tab_mut()
        .unwrap()
        .tab_type = crate::tabs::TabType::Plugin {
        plugin_id: "hermetic-read-only-fixture".into(),
    };
    assert!(!app.write_paste_text("rm -rf /\n"));
    assert!(!app.guardrail.is_awaiting_current());
    let error = app
        .send_text_for(Requester::Control, tab_id, pane_id, "rm -rf /", false, true)
        .unwrap_err();
    assert!(error.contains("read-only"), "{error}");
    let error = app
        .send_text_for(
            Requester::Plugin,
            tab_id,
            pane_id,
            "cargo test",
            false,
            false,
        )
        .unwrap_err();
    assert!(error.contains("read-only"), "{error}");
    assert!(!app.guardrail.is_awaiting_current());
}

#[test]
fn a_closed_split_tab_keeps_every_pane_and_is_named_for_the_focused_one() {
    let workspace = tempfile::tempdir().unwrap();
    let left_dir = workspace.path().join("left");
    let right_dir = workspace.path().join("right");
    std::fs::create_dir_all(&left_dir).unwrap();
    std::fs::create_dir_all(&right_dir).unwrap();

    let mut tab = Tab::new_with_command(
        0,
        80,
        8,
        &crate::tabs::CommandLaunch {
            program: "/bin/sh".into(),
            args: vec![],
            title: Some("session-source".into()),
            ..crate::tabs::CommandLaunch::default()
        },
        1_000,
        8 * 1024 * 1024,
    )
    .unwrap();
    let shell = ShellConfig {
        program: Some("/bin/sh".into()),
        args: Vec::new(),
        env: Default::default(),
    };
    let panes = tab.panes_for_test(shell, 800.0, 160.0, 10.0, 20.0);
    let right_id = panes
        .split(
            SplitDirection::Vertical,
            crate::config::PaneCwdMode::Focused,
        )
        .unwrap();

    for (pane_id, directory) in [(PaneId(0), &left_dir), (right_id, &right_dir)] {
        panes
            .get_pane_mut(pane_id)
            .unwrap()
            .write(format!("cd '{}'\n", directory.display()).as_bytes())
            .unwrap();
    }
    let cwd_deadline = Instant::now() + Duration::from_secs(2);
    loop {
        panes.poll_events();
        let left_cwd = panes
            .get_pane(PaneId(0))
            .unwrap()
            .pty
            .child_id()
            .and_then(crate::terminal::process::get_process_cwd);
        let right_cwd = panes
            .get_pane(right_id)
            .unwrap()
            .pty
            .child_id()
            .and_then(crate::terminal::process::get_process_cwd);
        if left_cwd.as_deref() == left_dir.to_str() && right_cwd.as_deref() == right_dir.to_str() {
            break;
        }
        assert!(Instant::now() < cwd_deadline, "pane cwd changes timed out");
        std::thread::sleep(Duration::from_millis(2));
    }

    for (pane_id, marker, command) in [
        (PaneId(0), "left-history", "left-command"),
        (right_id, "right-history", "right-command"),
    ] {
        let pane = panes.get_pane_mut(pane_id).unwrap();
        for index in 0..20 {
            pane.terminal
                .process(format!("{marker}-{index}\r\n").as_bytes());
        }
        pane.last_command = Some(command.into());
    }

    for (focused, directory, command) in [
        (PaneId(0), &left_dir, "left-command"),
        (right_id, &right_dir, "right-command"),
    ] {
        tab.panes.focus(focused);
        let snapshot = tab.closed_tab_snapshot(true).unwrap();
        assert!(snapshot.closed_tab);
        let name = directory.file_name().unwrap().to_str().unwrap();
        assert!(
            snapshot.name.contains(name) && snapshot.name.ends_with(command),
            "{}",
            snapshot.name
        );
        let [saved] = snapshot.tabs.as_slice() else {
            panic!("a closed tab is one tab");
        };
        assert_eq!(saved.focused_pane, focused.0);
        for (pane, pane_dir, marker, pane_command) in [
            (PaneId(0), &left_dir, "left-history", "left-command"),
            (right_id, &right_dir, "right-history", "right-command"),
        ] {
            let pane = saved.panes.iter().find(|saved| saved.id == pane.0).unwrap();
            assert_eq!(pane.working_directory.as_deref(), pane_dir.to_str());
            assert_eq!(pane.last_command.as_deref(), Some(pane_command));
            assert!(pane.scrollback_lines.join("\n").contains(marker));
        }
    }
}

#[test]
fn page_scrolling_moves_by_the_focused_terminals_screen() {
    let config = Config {
        shell: crate::test_support::sleeping_shell(),
        ..Config::default()
    };
    let mut app = App::new_headless_with_tabs(config);
    let terminal = app.active_terminal_mut().unwrap();
    for line in 0..200 {
        terminal.process(format!("line {line}\r\n").as_bytes());
    }
    let rows = terminal.grid().rows();
    assert!(terminal.grid().scrollback_len() > 2 * rows);

    assert!(app.handle_action(Action::ScrollPageUp));
    assert!(app.handle_action(Action::ScrollPageUp));
    assert_eq!(
        app.active_terminal().unwrap().grid().scroll_offset(),
        2 * rows
    );
    assert!(app.handle_action(Action::ScrollPageDown));
    assert_eq!(app.active_terminal().unwrap().grid().scroll_offset(), rows);
    assert!(app.handle_action(Action::ScrollPageDown));
    assert!(app.handle_action(Action::ScrollPageDown));
    assert_eq!(app.active_terminal().unwrap().grid().scroll_offset(), 0);
}
