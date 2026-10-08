//! Desktop notifications: occasions, delivery failures, replies, and activation.

use super::*;

#[test]
fn desktop_delivery_failures_become_deduplicated_visible_toasts() {
    let config = Config {
        shell: crate::test_support::sleeping_shell(),
        ..Config::default()
    };
    let mut app = App::new_headless_with_tabs(config);
    app.bell.set_config(crate::config::BellConfigFile {
        audio: true,
        ..Default::default()
    });
    app.bell
        .set_audio_command_for_test("/definitely/missing/termeric-audio-helper", &[]);
    app.notifications
        .fail_delivery_for_test("synthetic notification service failure");
    let origin = TerminalEventOrigin {
        tab_id: app.tabs.as_ref().unwrap().active_tab_id().unwrap(),
        pane_id: PaneId(0),
    };
    let notify = |app: &mut App, body: &str| {
        app.handle_terminal_event(RoutedTerminalEvent {
            origin,
            event: TerminalEvent::Notification(termeric_core::NotificationRequest {
                title: Some("Build".into()),
                body: body.into(),
                ..termeric_core::NotificationRequest::default()
            }),
        });
    };
    // Notification failures arrive from the delivery worker, so drain its
    // completions the way the event loop does.
    let settle = |app: &mut App, toasts: usize| {
        let deadline = Instant::now() + Duration::from_secs(3);
        while app.toasts.iter().count() < toasts && Instant::now() < deadline {
            app.process_pty_events();
            std::thread::sleep(Duration::from_millis(5));
        }
    };

    app.handle_terminal_event(RoutedTerminalEvent {
        origin,
        event: TerminalEvent::Bell,
    });
    notify(&mut app, "Finished");
    settle(&mut app, 2);
    let messages = app
        .toasts
        .iter()
        .map(|toast| toast.message.as_str())
        .collect::<Vec<_>>();
    assert_eq!(messages.len(), 2);
    assert!(messages[0].contains("Audio bell unavailable"));
    assert!(messages[1].contains("Desktop notification unavailable"));

    // The manager suppresses repeats of the same integration failure instead
    // of letting a hostile terminal fill the bounded toast stack.
    notify(&mut app, "Again");
    assert_eq!(app.notifications.settle_for_test(), []);
    app.process_pty_events();
    assert_eq!(app.toasts.iter().count(), 2);
}

#[test]
fn notification_occasions_use_exact_tab_pane_zoom_focus_and_visibility_state() {
    use termeric_core::NotificationOccasion::{Always, Invisible, Unfocused};

    let shell = crate::test_support::sleeping_shell();
    let mut tabs = TabManager::for_test(shell.clone());
    let split_tab_id = tabs.new_tab().unwrap();
    let split_tab = tabs.tab_mut_by_id(split_tab_id).unwrap();
    let panes = split_tab.panes_for_test(shell, 800.0, 480.0, 10.0, 20.0);
    panes
        .split(
            SplitDirection::Vertical,
            crate::config::PaneCwdMode::Focused,
        )
        .unwrap();
    assert_eq!(panes.focused_id(), PaneId(1));
    let active_tab_id = tabs.new_tab().unwrap();

    let split_visible = TerminalEventOrigin {
        tab_id: split_tab_id,
        pane_id: PaneId(0),
    };
    let split_focused = TerminalEventOrigin {
        tab_id: split_tab_id,
        pane_id: PaneId(1),
    };
    let active_tab = TerminalEventOrigin {
        tab_id: active_tab_id,
        pane_id: PaneId(0),
    };
    let stale = TerminalEventOrigin {
        tab_id: usize::MAX,
        pane_id: PaneId(0),
    };
    let mut app = App::new_headless(Config::default()).unwrap();
    app.tabs = Some(tabs);

    assert!(app.should_present_terminal_notification(active_tab, Always));
    assert!(!app.should_present_terminal_notification(active_tab, Unfocused));
    assert!(!app.should_present_terminal_notification(active_tab, Invisible));
    assert!(app.should_present_terminal_notification(split_visible, Unfocused));
    assert!(app.should_present_terminal_notification(split_visible, Invisible));
    assert!(!app.should_present_terminal_notification(stale, Always));

    app.tabs.as_mut().unwrap().switch_to(0);
    assert!(!app.should_present_terminal_notification(split_focused, Unfocused));
    assert!(!app.should_present_terminal_notification(split_focused, Invisible));
    assert!(app.should_present_terminal_notification(split_visible, Unfocused));
    assert!(!app.should_present_terminal_notification(split_visible, Invisible));

    assert!(app.active_panes_mut().toggle_zoom());
    assert!(app.should_present_terminal_notification(split_visible, Invisible));

    app.window_focused = false;
    assert!(app.should_present_terminal_notification(split_focused, Unfocused));
    assert!(!app.should_present_terminal_notification(split_focused, Invisible));
    app.window_occluded = true;
    assert!(app.should_present_terminal_notification(split_focused, Invisible));
}

#[test]
fn suppressed_notification_has_no_delivery_or_tracked_worker_side_effects() {
    let config = Config {
        shell: crate::test_support::sleeping_shell(),
        ..Config::default()
    };
    let mut app = App::new_headless_with_tabs(config);
    let origin = TerminalEventOrigin {
        tab_id: app.tabs.as_ref().unwrap().active_tab_id().unwrap(),
        pane_id: PaneId(0),
    };
    app.handle_terminal_event(RoutedTerminalEvent {
        origin,
        event: TerminalEvent::Notification(termeric_core::NotificationRequest {
            title: Some("Suppressed".into()),
            occasion: termeric_core::NotificationOccasion::Unfocused,
            report_close: true,
            icon_data: Some(Box::from([1_u8, 2, 3])),
            ..termeric_core::NotificationRequest::default()
        }),
    });
    assert!(!app.notifications.has_delivery_state_for_test());
    assert_eq!(app.toasts.iter().count(), 0);
}

#[test]
fn delayed_notification_completion_does_not_alias_a_replacement_single_owner() {
    let (mut app, directory, entry, _) = harvest_rerun_capture_fixture_in(false);
    let origin = TerminalEventOrigin {
        tab_id: entry.source.tab_id,
        pane_id: PaneId(0),
    };
    let origin = app
        .tabs
        .as_ref()
        .unwrap()
        .identity_for_origin(origin)
        .unwrap();
    let completion = crate::notifications::NotificationCompletion::Activation {
        origin,
        focus: true,
        protocol_reply: Some(b"\x1b]99;i=fixture;1\x1b\\".to_vec()),
    };
    split_harvest_capture_owner(&mut app, directory.path());
    app.close_pane_for(Requester::Control, entry.source.tab_id as u64, 0)
        .unwrap();
    let captures = current_harvest_capture_routes(&mut app, directory.path());
    assert!(
        !app.handle_notification_completion(completion),
        "a notification must not activate the replacement owner"
    );
    assert!(
        fenced_harvest_capture(&mut app, &captures)
            .iter()
            .all(Vec::is_empty),
        "a notification reply must not reach the replacement owner"
    );
}

#[test]
fn delayed_notification_reply_returns_to_requesting_pane_without_a_focus_fallback() {
    let data = b"\x1b]99;i=build;2\x1b\\".to_vec();
    let script = format!(
        "stty raw -echo; reply=$(dd bs=1 count={} 2>/dev/null); \
         printf '\\rnotification='; printf '%s' \"$reply\" | od -An -tx1; printf '\\r\\n'",
        data.len()
    );
    let shell = ShellConfig {
        program: Some("/bin/sh".into()),
        args: vec!["-c".into(), script],
        env: Default::default(),
    };
    let mut app = App::new_headless(Config::default()).unwrap();
    let mut tabs = TabManager::for_test(ShellConfig::default());
    let tab_id = tabs
        .new_tab_with_command(
            "/bin/sh",
            &crate::test_support::sleeping_args(),
            None,
            Some("notification-owner"),
            None,
            HashMap::new(),
        )
        .unwrap();
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
    let requesting_pane = panes
        .split(
            SplitDirection::Vertical,
            crate::config::PaneCwdMode::Focused,
        )
        .unwrap();
    panes.focus(PaneId(0));
    tabs.active_tab_mut().unwrap().panes = panes;
    app.tabs = Some(tabs);

    assert!(!app.handle_notification_completion(
        crate::notifications::NotificationCompletion::Activation {
            origin: crate::tabs::TerminalIdentity {
                tab_id,
                pane_id: requesting_pane,
            },
            focus: false,
            protocol_reply: Some(data),
        }
    ));

    let deadline = Instant::now() + Duration::from_secs(5);
    let expected_reply_hex = "notification=1b5d39393b693d6275696c643b321b5c";
    let mut requesting_text = String::new();
    while Instant::now() < deadline {
        app.tabs.as_mut().unwrap().poll_events();
        requesting_text = app
            .active_panes()
            .get_pane(requesting_pane)
            .unwrap()
            .terminal
            .grid()
            .plain_lines()
            .join("\n");
        if requesting_text
            .chars()
            .filter(|character| !character.is_whitespace())
            .collect::<String>()
            .contains(expected_reply_hex)
        {
            break;
        }
        std::thread::sleep(Duration::from_millis(2));
    }

    let panes = app.active_panes();
    let focused_text = panes
        .get_pane(PaneId(0))
        .unwrap()
        .terminal
        .grid()
        .plain_lines()
        .join("\n");
    let compact_requesting_text: String = requesting_text
        .chars()
        .filter(|character| !character.is_whitespace())
        .collect();
    assert!(
        compact_requesting_text.contains(expected_reply_hex),
        "requesting pane output did not contain the exact reply: {requesting_text:?}"
    );
    assert!(!focused_text.contains("notification="));
    assert_eq!(panes.focused_id(), PaneId(0));

    // Stale origins are discarded rather than falling back to active content.
    let latest_toast_before = latest_toast(&app).map(str::to_owned);
    assert!(!app.handle_notification_completion(
        crate::notifications::NotificationCompletion::ProtocolReply {
            origin: crate::tabs::TerminalIdentity {
                tab_id,
                pane_id: PaneId(u32::MAX),
            },
            data: b"must-not-leak".to_vec(),
        }
    ));
    assert_eq!(latest_toast(&app), latest_toast_before.as_deref());
}

#[test]
fn notification_activation_selects_the_exact_origin_and_stale_origins_fail_closed() {
    let shell = crate::test_support::sleeping_shell();
    let mut tabs = TabManager::for_test(shell.clone());
    let origin_tab = tabs
        .new_tab_with_command(
            "/bin/sh",
            &crate::test_support::sleeping_args(),
            None,
            Some("notification-origin"),
            None,
            HashMap::new(),
        )
        .unwrap();
    let active_tab = tabs
        .new_tab_with_command(
            "/bin/sh",
            &crate::test_support::sleeping_args(),
            None,
            Some("active"),
            None,
            HashMap::new(),
        )
        .unwrap();
    let origin = {
        let tab = tabs.tab_mut_by_id(origin_tab).unwrap();
        let panes = tab.panes_for_test(shell, 800.0, 480.0, 10.0, 20.0);
        let pane_id = panes
            .split(
                SplitDirection::Vertical,
                crate::config::PaneCwdMode::Focused,
            )
            .unwrap();
        panes.focus(PaneId(0));
        crate::tabs::TerminalIdentity {
            tab_id: origin_tab,
            pane_id,
        }
    };
    assert_eq!(tabs.active_tab_id(), Some(active_tab));

    let mut app = App::new_headless(Config::default()).unwrap();
    app.tabs = Some(tabs);
    assert!(app.handle_notification_completion(
        crate::notifications::NotificationCompletion::Activation {
            origin,
            focus: true,
            protocol_reply: None,
        }
    ));
    let tabs = app.tabs.as_ref().unwrap();
    assert_eq!(tabs.active_tab_id(), Some(origin_tab));
    assert_eq!(
        tabs.active_tab().unwrap().panes.focused_id(),
        origin.pane_id
    );

    assert_eq!(
        app.select_tab_for(Requester::Control, active_tab as u64),
        Ok(true)
    );
    let stale = crate::tabs::TerminalIdentity {
        tab_id: origin_tab,
        pane_id: PaneId(u32::MAX),
    };
    assert!(!app.handle_notification_completion(
        crate::notifications::NotificationCompletion::Activation {
            origin: stale,
            focus: true,
            protocol_reply: None,
        }
    ));
    assert_eq!(app.tabs.as_ref().unwrap().active_tab_id(), Some(active_tab));

    assert!(!app.handle_notification_completion(
        crate::notifications::NotificationCompletion::Activation {
            origin,
            focus: false,
            protocol_reply: None,
        }
    ));
    assert_eq!(app.tabs.as_ref().unwrap().active_tab_id(), Some(active_tab));
}
