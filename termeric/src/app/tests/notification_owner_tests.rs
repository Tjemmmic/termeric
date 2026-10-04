//! Provider-free owner-lifetime tests: raw PTYs, synthetic completions, and
//! captured notification commands. Never connect to a native notification bus.
use super::*;
use crate::notifications::NotificationCompletion;
use crate::tabs::TerminalIdentity;

#[test]
fn delayed_notification_effects_follow_only_the_same_surviving_owner() {
    for kind in ["lifecycle", "report", "focus", "focus-report"] {
        for eight_bit in [false, true] {
            for transition in [
                "split",
                "collapse",
                "nonzero-collapse",
                "removed",
                "resplit-replacement",
                "closed",
                "closing",
                "pane-closed",
                "read-only",
                "broadcast",
            ] {
                let (mut app, directory, _, _) =
                    harvest_rerun_capture_fixture_in(transition == "nonzero-collapse");
                let identity = app
                    .tabs
                    .as_ref()
                    .unwrap()
                    .active_tab()
                    .unwrap()
                    .focused_terminal_identity();
                if transition == "nonzero-collapse" {
                    app.close_pane_for(Requester::Control, identity.tab_id as u64, 0)
                        .unwrap();
                    app.close_pane_for(Requester::Control, identity.tab_id as u64, 1)
                        .unwrap();
                } else {
                    let sibling = split_harvest_capture_owner(&mut app, directory.path());
                    match transition {
                        "collapse" => {
                            app.close_pane_for(
                                Requester::Control,
                                identity.tab_id as u64,
                                sibling.0,
                            )
                            .unwrap();
                        }
                        "removed" | "resplit-replacement" => {
                            app.close_pane_for(
                                Requester::Control,
                                identity.tab_id as u64,
                                identity.pane_id.0,
                            )
                            .unwrap();
                            if transition == "resplit-replacement" {
                                split_harvest_capture_owner(&mut app, directory.path());
                            }
                        }
                        _ => {}
                    }
                }
                let captures = current_harvest_capture_routes(&mut app, directory.path());
                let tab = app.tabs.as_mut().unwrap().active_tab_mut().unwrap();
                match transition {
                    "closed" => tab.closed = true,
                    "closing" => tab.closing_at = Some(Instant::now()),
                    "pane-closed" => {
                        tab.pane_manager
                            .as_mut()
                            .unwrap()
                            .get_pane_mut(identity.pane_id)
                            .unwrap()
                            .closed = true
                    }
                    "read-only" => {
                        tab.pane_manager
                            .as_mut()
                            .unwrap()
                            .get_pane_mut(identity.pane_id)
                            .unwrap()
                            .read_only = true
                    }
                    "broadcast" => app.toggle_broadcast(),
                    _ => {}
                }
                let focused_before = app.active_harvest_source();
                app.input_line_buffer = "editable-prefix".into();
                let data = termeric_core::Terminal::frame_pty_reply_for_mode(
                    b"\x1b]99;i=fixture;1\x1b\\",
                    eight_bit,
                );
                let completion = if kind == "lifecycle" {
                    NotificationCompletion::ProtocolReply {
                        origin: identity,
                        data: data.clone(),
                    }
                } else {
                    NotificationCompletion::Activation {
                        origin: identity,
                        focus: kind.starts_with("focus"),
                        protocol_reply: (kind != "focus").then(|| data.clone()),
                    }
                };
                let survives = matches!(
                    transition,
                    "split" | "collapse" | "nonzero-collapse" | "read-only" | "broadcast"
                );
                let changed = app.handle_notification_completion(completion);
                assert_eq!(
                    changed,
                    survives && kind.starts_with("focus"),
                    "{kind} {transition}"
                );
                let source_route = app
                    .tabs
                    .as_ref()
                    .unwrap()
                    .origin_for_pane(identity.tab_id, identity.pane_id);
                let bytes = fenced_harvest_capture(&mut app, &captures);
                for ((route, _), captured) in captures.iter().zip(bytes) {
                    let expected = if survives && kind != "focus" && Some(*route) == source_route {
                        data.as_slice()
                    } else {
                        &[]
                    };
                    assert_eq!(captured, expected, "{kind} S8={eight_bit} {transition}");
                }
                if survives && kind.starts_with("focus") {
                    let focused = app.active_harvest_source().unwrap();
                    assert_eq!(
                        (focused.tab_id, focused.pane_id),
                        (identity.tab_id, identity.pane_id.0)
                    );
                } else {
                    assert_eq!(
                        app.active_harvest_source(),
                        focused_before,
                        "{kind} {transition}"
                    );
                }
                assert_eq!(app.input_line_buffer, "editable-prefix");
            }
        }
    }
}

#[test]
fn notification_show_close_and_alive_commands_capture_stable_owner_keys() {
    let (mut app, directory, entry, _) = harvest_rerun_capture_fixture_in(false);
    let capture = app.notifications.capture_tracked_commands_for_test();
    let owner = TerminalIdentity {
        tab_id: entry.source.tab_id,
        pane_id: PaneId(0),
    };
    let issue = |app: &mut App, origin| {
        app.handle_terminal_event(RoutedTerminalEvent {
            origin,
            event: TerminalEvent::Notification(termeric_core::NotificationRequest {
                identifier: Some("fixture".into()),
                title: Some("local fixture".into()),
                occasion: termeric_core::NotificationOccasion::Always,
                ..Default::default()
            }),
        });
        app.handle_terminal_event(RoutedTerminalEvent {
            origin,
            event: TerminalEvent::NotificationAliveQuery {
                identifier: "fixture".into(),
                reply_uses_8bit_controls: true,
            },
        });
        app.handle_terminal_event(RoutedTerminalEvent {
            origin,
            event: TerminalEvent::NotificationClose {
                identifier: "fixture".into(),
            },
        });
    };
    let expected = |identity| {
        vec![
            ("show", identity, "fixture".into()),
            ("alive", identity, "fixture".into()),
            ("close", identity, "fixture".into()),
        ]
    };
    let route = TerminalEventOrigin {
        tab_id: owner.tab_id,
        pane_id: owner.pane_id,
    };
    issue(&mut app, route);
    assert_eq!(capture(), expected(owner));
    // The route names pane zero, so it stays valid while the tab is split.
    let sibling = split_harvest_capture_owner(&mut app, directory.path());
    current_harvest_capture_routes(&mut app, directory.path());
    issue(&mut app, route);
    assert_eq!(capture(), expected(owner));
    let sibling_identity = TerminalIdentity {
        tab_id: owner.tab_id,
        pane_id: sibling,
    };
    issue(
        &mut app,
        TerminalEventOrigin {
            tab_id: owner.tab_id,
            pane_id: sibling,
        },
    );
    assert_eq!(capture(), expected(sibling_identity));
    app.close_pane_for(Requester::Control, owner.tab_id as u64, sibling.0)
        .unwrap();
    issue(&mut app, route);
    assert_eq!(capture(), expected(owner));
    // Removing pane zero retires its route; the surviving pane has its own.
    let replacement = split_harvest_capture_owner(&mut app, directory.path());
    app.close_pane_for(Requester::Control, owner.tab_id as u64, owner.pane_id.0)
        .unwrap();
    current_harvest_capture_routes(&mut app, directory.path());
    issue(&mut app, route);
    assert!(capture().is_empty());
    let replacement_route = TerminalEventOrigin {
        tab_id: owner.tab_id,
        pane_id: replacement,
    };
    issue(&mut app, replacement_route);
    assert_eq!(
        capture(),
        expected(TerminalIdentity {
            tab_id: owner.tab_id,
            pane_id: replacement
        })
    );
    app.tabs
        .as_mut()
        .unwrap()
        .active_tab_mut()
        .unwrap()
        .closing_at = Some(Instant::now());
    issue(&mut app, replacement_route);
    assert!(capture().is_empty());
}
