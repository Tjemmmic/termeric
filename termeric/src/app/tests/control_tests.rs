//! Control-plane and plugin tab and pane operations by stable ID.

use super::*;

#[test]
fn control_and_plugin_text_reads_show_the_presented_screen_of_an_exact_pane() {
    let (mut app, first, second) = background_split_tab_app();
    let source_terminal = &mut split_pane_mut(&mut app, first, PaneId(0)).terminal;
    source_terminal.process(b"presented-secret");
    source_terminal.process(b"\x1b[?2026h\rhidden-update");
    let (screen, history, text, truncated) =
        app.read_control_text(first as u64, 0, 20, 4096).unwrap();
    assert_eq!(screen, crate::control::protocol::TextScreen::Primary);
    assert_eq!(history, 0);
    assert!(text.contains("presented-secret"));
    assert!(!text.contains("hidden-update"));
    assert!(!truncated);
    assert!(app.read_control_text(first as u64, 99, 0, 64).is_err());

    let event = plugin_terminal_query_event(&app, "visible-text", first as u64, 0, 20, 4096);
    assert_eq!(event["schema_version"], 1);
    assert_eq!(event["plugin_id"], "terminal-reader");
    assert_eq!(event["origin_window_id"], app.control_window_id);
    assert_eq!(event["event"]["type"], "terminal-query-result");
    assert_eq!(event["event"]["query_id"], "visible-text");
    assert_eq!(event["event"]["window_id"], app.control_window_id);
    assert_eq!(event["event"]["ok"], true);
    assert_eq!(event["event"]["tab_id"], first as u64);
    assert_eq!(event["event"]["pane_id"], 0);
    assert_eq!(event["event"]["screen"], "primary");
    let text = event["event"]["text"].as_str().unwrap();
    assert!(text.contains("presented-secret"));
    assert!(!text.contains("hidden-update"));

    let missing = plugin_terminal_query_event(&app, "missing-pane", first as u64, 99, 0, 64);
    assert_eq!(missing["event"]["ok"], false);
    assert_eq!(missing["event"]["pane_id"], 99);
    assert!(missing["event"]["text"].is_null());
    assert!(
        missing["event"]["error"]
            .as_str()
            .unwrap()
            .contains("does not exist")
    );

    let source_terminal = &mut split_pane_mut(&mut app, first, PaneId(0)).terminal;
    source_terminal.process(b"\x1b[?2026l\x1b[2J\x1b[H");
    source_terminal.process("\\".repeat(9_000).as_bytes());
    let escaped = plugin_terminal_query_event(
        &app,
        "escaped-text",
        first as u64,
        0,
        256,
        crate::plugin::structured::MAX_TERMINAL_QUERY_BYTES,
    );
    assert!(
        escaped.to_string().len() <= crate::plugin::structured::MAX_PERSISTENT_SOURCE_EVENT_BYTES
    );
    assert_eq!(escaped["event"]["ok"], true);
    assert_eq!(escaped["event"]["truncated"], true);
    assert!(
        escaped["event"]["text_bytes"].as_u64().unwrap()
            < u64::from(crate::plugin::structured::MAX_TERMINAL_QUERY_BYTES)
    );
    assert_eq!(app.tabs.as_ref().unwrap().active_tab_id(), Some(second));
}

#[test]
fn external_text_input_follows_policy_closed_panes_and_guardrails_without_changing_focus() {
    let (mut app, first, second) = background_split_tab_app();
    let send_text = |app: &mut App, action_id: &str, text: &str, enter: bool| {
        plugin_action_event(
            app,
            "terminal-writer",
            crate::plugin::structured::StructuredResponseAction::SendText {
                action_id: action_id.into(),
                window_id: None,
                tab_id: first as u64,
                pane_id: 0,
                text: text.into(),
                paste: false,
                enter,
            },
        )
    };

    let disabled_input = send_text(&mut app, "policy-disabled", "x", false);
    assert_eq!(disabled_input["event"]["action"], "send-text");
    assert_eq!(disabled_input["event"]["ok"], false);
    assert!(
        disabled_input["event"]["error"]
            .as_str()
            .is_some_and(|error| error.contains("disabled"))
    );

    app.config.security.allow_gui_control_input = true;
    let sent_input = send_text(&mut app, "safe-input", "x", false);
    assert_eq!(sent_input["event"]["ok"], true);
    assert_eq!(sent_input["event"]["disposition"], "sent");
    assert_eq!(sent_input["event"]["input_bytes"], 1);
    assert_eq!(sent_input["event"]["wire_bytes"], 1);
    assert_eq!(
        sent_input["event"]["confirmation_id"],
        serde_json::Value::Null
    );
    assert_eq!(app.tabs.as_ref().unwrap().active_tab_id(), Some(second));

    split_pane_mut(&mut app, first, PaneId(0)).closed = true;
    let closed_input = send_text(&mut app, "closed-input", "x", false);
    assert_eq!(closed_input["event"]["ok"], false);
    assert!(
        closed_input["event"]["error"]
            .as_str()
            .is_some_and(|error| error.contains("closed"))
    );
    split_pane_mut(&mut app, first, PaneId(0)).closed = false;

    let guarded_input = send_text(&mut app, "guarded-input", "rm -rf /", true);
    assert_eq!(guarded_input["event"]["ok"], true);
    assert_eq!(
        guarded_input["event"]["disposition"],
        "pending_confirmation"
    );
    let confirmation_id = guarded_input["event"]["confirmation_id"].as_u64().unwrap();
    assert!(app.guardrail.is_awaiting(confirmation_id));
    assert_eq!(app.tabs.as_ref().unwrap().active_tab_id(), Some(second));
    app.apply_guardrail_action(crate::terminal::guardrails::GuardrailAction::Cancel {
        request_id: confirmation_id,
    });
    assert!(!app.guardrail.is_awaiting_current());

    assert!(matches!(
        app.send_text_for(Requester::Control, first as u64, 0, "x", false, false),
        Ok((1, 1, crate::control::protocol::InputDisposition::Sent, None))
    ));
    assert_eq!(app.tabs.as_ref().unwrap().active_tab_id(), Some(second));
}

#[test]
fn control_selection_validates_complete_stable_paths_before_changing_focus() {
    let (mut app, first, second) = background_split_tab_app();
    assert!(
        app.focus_pane_for(Requester::Control, first as u64, 99)
            .is_err()
    );
    assert_eq!(app.tabs.as_ref().unwrap().active_tab_id(), Some(second));

    assert_eq!(
        app.focus_pane_for(Requester::Control, first as u64, 0),
        Ok((true, true))
    );
    let tabs = app.tabs.as_ref().unwrap();
    assert_eq!(tabs.active_tab_id(), Some(first));
    assert_eq!(tabs.active_tab().unwrap().panes.focused_id(), PaneId(0));
    assert_eq!(
        app.focus_pane_for(Requester::Control, first as u64, 0),
        Ok((false, false))
    );
    assert_eq!(
        app.select_tab_for(Requester::Control, second as u64),
        Ok(true)
    );
    assert_eq!(
        app.select_tab_for(Requester::Control, second as u64),
        Ok(false)
    );
    assert!(app.select_tab_for(Requester::Control, u64::MAX).is_err());
    assert_eq!(app.tabs.as_ref().unwrap().active_tab_id(), Some(second));
}

#[test]
fn control_tab_moves_reorder_without_activating_and_reject_out_of_range_targets() {
    let (mut app, first, second) = background_split_tab_app();
    // Plugin tab moves share the control path, which reorders without
    // activating.
    assert_eq!(
        app.move_control_tab_within_window(first as u64, 1),
        Ok(true)
    );
    assert_eq!(app.tabs.as_ref().unwrap().active_tab_id(), Some(second));
    assert_eq!(tab_order(&app), vec![second, first]);
    assert!(
        app.move_control_tab_within_window(first as u64, 2)
            .is_err_and(|error| error.contains("outside"))
    );
    assert_eq!(tab_order(&app), vec![second, first]);
}

#[test]
fn divider_resizes_report_ratios_reach_the_inventory_and_refuse_live_domain_tabs() {
    let (mut app, first, second) = background_split_tab_app();
    let divider_id = app
        .tabs
        .as_ref()
        .unwrap()
        .all_tabs()
        .iter()
        .find(|tab| tab.id == first)
        .and_then(|tab| tab.panes.get_dividers().first().copied())
        .and_then(|divider| match divider.target {
            crate::panes::DividerTarget::Local(id) => Some(id),
            crate::panes::DividerTarget::Domain(_) => None,
        })
        .unwrap();
    let resized = plugin_action_event(
        &mut app,
        "workspace-organizer",
        crate::plugin::structured::StructuredResponseAction::ResizeDivider {
            action_id: "resize-first".into(),
            window_id: None,
            tab_id: first as u64,
            divider_id,
            ratio_millis: 650,
        },
    );
    assert_eq!(resized["event"]["action"], "resize-divider");
    assert_eq!(resized["event"]["ok"], true);
    assert_eq!(resized["event"]["previous_ratio_millis"], 500);
    assert_eq!(resized["event"]["ratio_millis"], 650);
    assert_eq!(resized["event"]["changed"], true);
    assert_eq!(
        app.resize_control_divider(first as u64, divider_id, 650),
        Ok((650, false))
    );
    let failed_resize = plugin_action_event(
        &mut app,
        "workspace-organizer",
        crate::plugin::structured::StructuredResponseAction::ResizeDivider {
            action_id: "missing-divider".into(),
            window_id: None,
            tab_id: first as u64,
            divider_id: u64::MAX,
            ratio_millis: 600,
        },
    );
    assert_eq!(failed_resize["event"]["ok"], false);
    assert_eq!(
        failed_resize["event"]["previous_ratio_millis"],
        serde_json::Value::Null
    );
    assert_eq!(failed_resize["event"]["changed"], serde_json::Value::Null);
    assert_eq!(app.tabs.as_ref().unwrap().active_tab_id(), Some(second));
    let inventory = app.control_inventory(7, &mut ControlInventoryBudget::default());
    let divider = inventory
        .tabs
        .iter()
        .find(|tab| tab.id == first as u64)
        .and_then(|tab| tab.dividers.first())
        .unwrap();
    assert_eq!(divider.id, divider_id);
    assert_eq!(divider.ratio_millis, 650);
    assert_eq!(divider.kind, crate::control::protocol::DividerKind::Local);
    app.tabs
        .as_mut()
        .unwrap()
        .tab_mut_by_id(first)
        .unwrap()
        .live_domain = Some(crate::tabs::LiveDomainTab {
        domain_id: "fake-domain".into(),
        tab_id: 8,
        pane_ids: HashMap::from([(PaneId(0), 10), (PaneId(1), 11)]),
    });
    assert!(
        app.resize_control_divider(first as u64, divider_id, 600)
            .unwrap_err()
            .contains("live-domain")
    );
}

#[test]
fn control_pane_and_tab_closure_leave_the_survivor_addressable_and_reject_repeats() {
    let (mut app, first, second) = background_split_tab_app();
    assert_eq!(
        app.close_pane_for(Requester::Control, first as u64, 0),
        Ok((1, 1))
    );
    let tabs = app.tabs.as_ref().unwrap();
    assert_eq!(tabs.active_tab_id(), Some(second));
    let first_tab = tabs.tab_by_id(first).unwrap();
    assert!(first_tab.panes.pane_count() == 1);
    assert_eq!(first_tab.panes.focused_id(), PaneId(1));
    assert!(app.read_control_text(first as u64, 1, 0, 64).is_ok());
    assert!(app.read_control_text(first as u64, 0, 0, 64).is_err());
    assert!(
        app.close_pane_for(Requester::Control, first as u64, 1)
            .is_err()
    );
    app.close_tab_for(Requester::Control, first as u64).unwrap();
    assert!(
        app.tabs
            .as_ref()
            .unwrap()
            .tab_by_id(first)
            .unwrap()
            .closing_at
            .is_some()
    );
    assert!(app.close_tab_for(Requester::Control, first as u64).is_err());
}

#[test]
fn control_requests_refuse_a_closing_tab_and_an_exited_pane_alike() {
    let (mut app, first, _) = background_split_tab_app();
    app.config.security.allow_gui_control_input = true;
    split_pane_mut(&mut app, first, PaneId(0)).closed = true;
    let exited = app
        .focus_pane_for(Requester::Control, first as u64, 0)
        .unwrap_err();
    assert_eq!(
        app.send_text_for(Requester::Control, first as u64, 0, "x", false, false)
            .unwrap_err(),
        exited
    );
    assert_eq!(
        app.close_pane_for(Requester::Control, first as u64, 0)
            .unwrap_err(),
        exited
    );
    // An exited pane keeps its text, so reading it still works.
    assert!(app.read_control_text(first as u64, 0, 0, 64).is_ok());
    split_pane_mut(&mut app, first, PaneId(0)).closed = false;

    app.close_tab_for(Requester::Control, first as u64).unwrap();
    let closing = app
        .select_tab_for(Requester::Control, first as u64)
        .unwrap_err();
    assert_eq!(
        app.close_tab_for(Requester::Control, first as u64)
            .unwrap_err(),
        closing
    );
    assert_eq!(
        app.control_live_domain_close_tab(first as u64).unwrap_err(),
        closing
    );
    assert_eq!(
        app.focus_pane_for(Requester::Control, first as u64, 1)
            .unwrap_err(),
        closing
    );
    assert_eq!(
        app.close_pane_for(Requester::Control, first as u64, 1)
            .unwrap_err(),
        closing
    );
    assert_eq!(
        app.read_control_text(first as u64, 1, 0, 64).unwrap_err(),
        closing
    );
    assert_eq!(
        app.send_text_for(Requester::Control, first as u64, 1, "x", false, false)
            .unwrap_err(),
        closing
    );
}

#[test]
fn plugin_pane_creation_is_exact_correlated_and_rejects_unsupported_tabs() {
    let shell = crate::test_support::sleeping_shell();
    let mut tabs = TabManager::for_test(shell.clone());
    let tab_id = tabs.new_tab().unwrap();
    let tab = tabs.tab_mut_by_id(tab_id).unwrap();
    let panes = tab.panes_for_test(shell, 800.0, 480.0, 10.0, 20.0);
    panes.split(SplitDirection::Vertical).unwrap();
    let mut app = App::new_headless(Config::default()).unwrap();
    app.tabs = Some(tabs);

    let created = plugin_action_event(
        &mut app,
        "pane-builder",
        crate::plugin::structured::StructuredResponseAction::CreatePane {
            action_id: "split-exact-pane".into(),
            window_id: None,
            tab_id: tab_id as u64,
            pane_id: 0,
            direction: crate::panes::SplitDirection::Horizontal,
        },
    );
    assert_eq!(created["event"]["type"], "action-result");
    assert_eq!(created["event"]["action"], "create-pane");
    assert_eq!(created["event"]["action_id"], "split-exact-pane");
    assert_eq!(created["event"]["ok"], true);
    assert_eq!(created["event"]["tab_id"], tab_id as u64);
    assert_eq!(created["event"]["pane_id"], 0);
    assert_eq!(created["event"]["direction"], "horizontal");
    let created_pane_id = created["event"]["created_pane_id"].as_u64().unwrap() as u32;
    assert!(
        app.tabs
            .as_ref()
            .unwrap()
            .tab_by_id(tab_id)
            .unwrap()
            .panes
            .get_pane(PaneId(created_pane_id))
            .is_some()
    );

    let stale = plugin_action_event(
        &mut app,
        "pane-builder",
        crate::plugin::structured::StructuredResponseAction::CreatePane {
            action_id: "stale-pane".into(),
            window_id: None,
            tab_id: tab_id as u64,
            pane_id: u32::MAX,
            direction: crate::panes::SplitDirection::Vertical,
        },
    );
    assert_eq!(stale["event"]["ok"], false);
    assert!(stale["event"]["created_pane_id"].is_null());
    assert!(
        stale["event"]["error"]
            .as_str()
            .is_some_and(|error| error.contains("does not exist"))
    );

    let tab = app.tabs.as_mut().unwrap().tab_mut_by_id(tab_id).unwrap();
    tab.live_domain = Some(crate::tabs::LiveDomainTab {
        domain_id: "fake-domain".into(),
        tab_id: 8,
        pane_ids: HashMap::new(),
    });
    let live = plugin_action_event(
        &mut app,
        "pane-builder",
        crate::plugin::structured::StructuredResponseAction::CreatePane {
            action_id: "reject-live".into(),
            window_id: None,
            tab_id: tab_id as u64,
            pane_id: 0,
            direction: crate::panes::SplitDirection::Vertical,
        },
    );
    assert_eq!(live["event"]["ok"], false);
    assert!(
        live["event"]["error"]
            .as_str()
            .is_some_and(|error| error.contains("live-domain"))
    );

    let tab = app.tabs.as_mut().unwrap().tab_mut_by_id(tab_id).unwrap();
    tab.live_domain = None;
    tab.tab_type = crate::tabs::TabType::Serial {
        name: "fixture".into(),
    };
    let serial = plugin_action_event(
        &mut app,
        "pane-builder",
        crate::plugin::structured::StructuredResponseAction::CreatePane {
            action_id: "reject-serial".into(),
            window_id: None,
            tab_id: tab_id as u64,
            pane_id: 0,
            direction: crate::panes::SplitDirection::Vertical,
        },
    );
    assert_eq!(serial["event"]["ok"], false);
    assert!(
        serial["event"]["error"]
            .as_str()
            .is_some_and(|error| error.contains("Serial"))
    );

    let tab = app.tabs.as_mut().unwrap().tab_mut_by_id(tab_id).unwrap();
    tab.tab_type = crate::tabs::TabType::Shell;
    tab.closing_at = Some(Instant::now());
    let closing = plugin_action_event(
        &mut app,
        "pane-builder",
        crate::plugin::structured::StructuredResponseAction::CreatePane {
            action_id: "reject-closing".into(),
            window_id: None,
            tab_id: tab_id as u64,
            pane_id: 0,
            direction: crate::panes::SplitDirection::Vertical,
        },
    );
    assert_eq!(closing["event"]["ok"], false);
    assert!(
        closing["event"]["error"]
            .as_str()
            .is_some_and(|error| error.contains("closing"))
    );
}

#[test]
fn plugin_pane_movement_preserves_pty_and_returns_exact_destination_state() {
    let shell = crate::test_support::sleeping_shell();
    let mut tabs = TabManager::for_test(shell.clone());
    let source_tab_id = tabs.new_tab().unwrap();
    let source = tabs.tab_mut_by_id(source_tab_id).unwrap();
    let moved_child = source.lone_child_id();
    let source_panes = source.panes_for_test(shell, 800.0, 480.0, 10.0, 20.0);
    let surviving_source_pane = source_panes.split(SplitDirection::Vertical).unwrap();
    let destination_tab_id = tabs.new_tab().unwrap();
    let destination_anchor = tabs
        .tab_by_id(destination_tab_id)
        .unwrap()
        .panes
        .focused_id();
    tabs.switch_to(0);

    let mut app = App::new_headless(Config::default()).unwrap();
    app.tabs = Some(tabs);
    app.plugin_manager.clear();
    let observed = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    app.plugin_manager
        .register(crate::test_support::hook_counter(
            |hook| matches!(hook, crate::plugin::Hook::TabSwitched { .. }),
            &observed,
        ))
        .unwrap();
    app.copy_mode_target = Some(crate::tabs::TerminalIdentity {
        tab_id: source_tab_id,
        pane_id: PaneId(0),
    });
    app.broadcast = BroadcastState::Active {
        tab_id: source_tab_id,
        targets: vec![PaneId(0), surviving_source_pane],
    };

    // Plugins move panes through the shared control path; within one window
    // it validates the whole transfer before detaching the live PTY.
    let destination_pane_id = PaneId(
        app.move_control_pane_within_window(
            source_tab_id as u64,
            0,
            destination_tab_id as u64,
            destination_anchor.0,
            SplitDirection::Horizontal,
        )
        .unwrap(),
    );
    let tabs = app.tabs.as_ref().unwrap();
    assert_eq!(tabs.active_tab_id(), Some(destination_tab_id));
    let source = tabs.tab_by_id(source_tab_id).unwrap();
    assert_eq!(source.panes.pane_ids(), vec![surviving_source_pane]);
    let destination = tabs.tab_by_id(destination_tab_id).unwrap();
    let destination_panes = &destination.panes;
    assert_eq!(destination_panes.focused_id(), destination_pane_id);
    assert_eq!(
        destination_panes
            .get_pane(destination_pane_id)
            .unwrap()
            .pty
            .child_id(),
        moved_child
    );
    assert!(app.copy_mode_target.is_none());
    assert!(matches!(app.broadcast, BroadcastState::Inactive));
    assert_eq!(observed.load(std::sync::atomic::Ordering::SeqCst), 0);
}

#[test]
fn plugin_pane_layout_controls_background_tab_without_changing_terminal_owners() {
    let shell = crate::test_support::sleeping_shell();
    let mut tabs = TabManager::for_test(shell.clone());
    let target_tab_id = tabs.new_tab().unwrap();
    let target = tabs.tab_mut_by_id(target_tab_id).unwrap();
    let panes = target.panes_for_test(shell, 800.0, 480.0, 10.0, 20.0);
    panes.split(SplitDirection::Vertical).unwrap();
    panes.split(SplitDirection::Horizontal).unwrap();
    let original = panes.pane_ids();
    let owners: HashMap<_, _> = original
        .iter()
        .map(|id| (*id, panes.get_pane(*id).unwrap().pty.child_id()))
        .collect();
    panes.focus(original[1]);
    assert!(panes.toggle_zoom());
    let active_tab_id = tabs.new_tab().unwrap();
    assert_eq!(tabs.active_tab_id(), Some(active_tab_id));

    let mut app = App::new_headless(Config::default()).unwrap();
    app.tabs = Some(tabs);
    let swapped = plugin_action_event(
        &mut app,
        "pane-arranger",
        crate::plugin::structured::StructuredResponseAction::SwapPanes {
            action_id: "swap-edges".into(),
            window_id: None,
            tab_id: target_tab_id as u64,
            first_pane_id: original[0].0,
            second_pane_id: original[2].0,
        },
    );
    let mut expected = original.clone();
    expected.swap(0, 2);
    assert_eq!(swapped["event"]["ok"], true);
    assert_eq!(swapped["event"]["action"], "swap-panes");
    assert_eq!(
        swapped["event"]["pane_order"],
        serde_json::json!(expected.iter().map(|id| id.0).collect::<Vec<_>>())
    );
    assert_eq!(swapped["event"]["focused_pane_id"], original[1].0);
    assert_eq!(swapped["event"]["zoomed_pane_id"], original[1].0);
    assert_eq!(
        app.tabs.as_ref().unwrap().active_tab_id(),
        Some(active_tab_id)
    );

    let target = &mut app
        .tabs
        .as_mut()
        .unwrap()
        .tab_mut_by_id(target_tab_id)
        .unwrap()
        .panes;
    assert!(target.toggle_zoom());
    let crate::panes::DividerTarget::Local(divider_id) = target.get_dividers()[0].target else {
        panic!("ordinary layout unexpectedly used a domain divider")
    };
    target.resize_local_divider(divider_id, 700).unwrap();

    let balanced = plugin_action_event(
        &mut app,
        "pane-arranger",
        crate::plugin::structured::StructuredResponseAction::BalancePanes {
            action_id: "balance".into(),
            window_id: None,
            tab_id: target_tab_id as u64,
        },
    );
    assert_eq!(balanced["event"]["ok"], true);
    assert_eq!(balanced["event"]["changed"], true);
    assert_eq!(
        app.tabs.as_ref().unwrap().active_tab_id(),
        Some(active_tab_id)
    );

    let arranged = plugin_action_event(
        &mut app,
        "pane-arranger",
        crate::plugin::structured::StructuredResponseAction::ApplyPaneLayout {
            action_id: "tall".into(),
            window_id: None,
            tab_id: target_tab_id as u64,
            preset: crate::panes::PaneLayoutPreset::Tall,
        },
    );
    assert_eq!(arranged["event"]["ok"], true);
    assert_eq!(arranged["event"]["preset"], "tall");
    assert!(arranged["event"]["zoomed_pane_id"].is_null());
    assert_eq!(
        app.tabs.as_ref().unwrap().active_tab_id(),
        Some(active_tab_id)
    );
    let target = &app
        .tabs
        .as_ref()
        .unwrap()
        .tab_by_id(target_tab_id)
        .unwrap()
        .panes;
    assert_eq!(target.focused_id(), original[1]);
    let tall_order = [
        vec![original[1]],
        expected
            .into_iter()
            .filter(|id| *id != original[1])
            .collect(),
    ]
    .concat();
    assert_eq!(target.pane_ids(), tall_order);
    let layout = target.calculate_layout();
    assert!(
        tall_order[1..]
            .iter()
            .all(|id| layout[id].x > layout[&original[1]].x)
    );
    for (id, child_id) in owners {
        assert_eq!(target.get_pane(id).unwrap().pty.child_id(), child_id);
    }
}

#[test]
fn plugin_pane_layout_refuses_unsupported_or_incomplete_tabs_atomically() {
    let shell = crate::test_support::sleeping_shell();
    let mut tabs = TabManager::for_test(shell.clone());
    let tab_id = tabs.new_tab().unwrap();
    let tab = tabs.tab_mut_by_id(tab_id).unwrap();
    let panes = tab.panes_for_test(shell, 800.0, 480.0, 10.0, 20.0);
    let sibling = panes.split(SplitDirection::Vertical).unwrap();
    let original = panes.pane_ids();
    let mut app = App::new_headless(Config::default()).unwrap();
    app.tabs = Some(tabs);

    let error = app
        .swap_plugin_panes(tab_id as u64, original[0].0, u32::MAX)
        .unwrap_err();
    assert!(error.contains("does not exist"));
    let panes = &app.tabs.as_ref().unwrap().tab_by_id(tab_id).unwrap().panes;
    assert_eq!(panes.pane_ids(), original);

    app.tabs
        .as_mut()
        .unwrap()
        .tab_mut_by_id(tab_id)
        .unwrap()
        .panes
        .get_pane_mut(sibling)
        .unwrap()
        .closed = true;
    assert!(
        app.balance_plugin_panes(tab_id as u64)
            .unwrap_err()
            .contains("closed")
    );
    app.tabs
        .as_mut()
        .unwrap()
        .tab_mut_by_id(tab_id)
        .unwrap()
        .panes
        .get_pane_mut(sibling)
        .unwrap()
        .closed = false;

    let tab = app.tabs.as_mut().unwrap().tab_mut_by_id(tab_id).unwrap();
    tab.closing_at = Some(Instant::now());
    assert!(
        app.balance_plugin_panes(tab_id as u64)
            .unwrap_err()
            .contains("closing")
    );
    let tab = app.tabs.as_mut().unwrap().tab_mut_by_id(tab_id).unwrap();
    tab.closing_at = None;
    tab.live_domain = Some(crate::tabs::LiveDomainTab {
        domain_id: "fake-domain".into(),
        tab_id: 8,
        pane_ids: HashMap::new(),
    });
    assert!(
        app.balance_plugin_panes(tab_id as u64)
            .unwrap_err()
            .contains("live-domain")
    );
    let tab = app.tabs.as_mut().unwrap().tab_mut_by_id(tab_id).unwrap();
    tab.live_domain = None;
    tab.tab_type = crate::tabs::TabType::Serial {
        name: "fixture".into(),
    };
    assert!(
        app.balance_plugin_panes(tab_id as u64)
            .unwrap_err()
            .contains("serial terminal")
    );
    let unsplit_id = app.tabs.as_mut().unwrap().new_tab().unwrap();
    assert!(
        app.balance_plugin_panes(unsplit_id as u64)
            .unwrap_err()
            .contains("not split")
    );
    assert!(
        app.balance_plugin_panes(u64::MAX)
            .unwrap_err()
            .contains("does not exist")
    );
}

#[test]
fn plugin_pane_movement_refuses_unsupported_targets_before_detaching() {
    fn fixture() -> (App, usize, usize, PaneId) {
        let shell = crate::test_support::sleeping_shell();
        let mut tabs = TabManager::for_test(shell.clone());
        let source_tab_id = tabs.new_tab().unwrap();
        let source = tabs.tab_mut_by_id(source_tab_id).unwrap();
        let panes = source.panes_for_test(shell, 800.0, 480.0, 10.0, 20.0);
        let sibling = panes.split(SplitDirection::Vertical).unwrap();
        let destination_tab_id = tabs.new_tab().unwrap();
        let mut app = App::new_headless(Config::default()).unwrap();
        app.tabs = Some(tabs);
        (app, source_tab_id, destination_tab_id, sibling)
    }

    fn move_result(
        app: &mut App,
        source_tab_id: u64,
        pane_id: u32,
        destination_tab_id: u64,
        target_pane_id: u32,
    ) -> std::result::Result<u32, String> {
        app.move_control_pane_within_window(
            source_tab_id,
            pane_id,
            destination_tab_id,
            target_pane_id,
            SplitDirection::Vertical,
        )
    }

    fn assert_refused(result: &std::result::Result<u32, String>, expected: &str) {
        assert!(
            result.as_ref().is_err_and(|error| error.contains(expected)),
            "unexpected result: {result:?}"
        );
    }

    fn assert_source_intact(app: &App, source_tab_id: usize) {
        let pane_count = app
            .tabs
            .as_ref()
            .unwrap()
            .tab_by_id(source_tab_id)
            .unwrap()
            .panes
            .pane_count();
        assert_eq!(pane_count, 2);
    }

    let (mut same, source, _, sibling) = fixture();
    assert_refused(
        &move_result(&mut same, source as u64, 0, source as u64, sibling.0),
        "same tab",
    );
    assert_source_intact(&same, source);

    let (mut missing, source, destination, _) = fixture();
    assert_refused(
        &move_result(&mut missing, u64::MAX, 0, destination as u64, 0),
        "does not exist",
    );
    assert_refused(
        &move_result(&mut missing, source as u64, 0, destination as u64, u32::MAX),
        "does not exist",
    );
    assert_source_intact(&missing, source);

    let (mut closing, source, destination, _) = fixture();
    closing
        .tabs
        .as_mut()
        .unwrap()
        .tab_mut_by_id(source)
        .unwrap()
        .closing_at = Some(Instant::now());
    assert_refused(
        &move_result(&mut closing, source as u64, 0, destination as u64, 0),
        "closing",
    );
    assert_source_intact(&closing, source);

    let (mut closed, source, destination, _) = fixture();
    closed
        .tabs
        .as_mut()
        .unwrap()
        .tab_mut_by_id(source)
        .unwrap()
        .panes
        .get_pane_mut(PaneId(0))
        .unwrap()
        .closed = true;
    assert_refused(
        &move_result(&mut closed, source as u64, 0, destination as u64, 0),
        "closed",
    );
    assert_source_intact(&closed, source);

    let (mut last_live, source, destination, sibling) = fixture();
    last_live
        .tabs
        .as_mut()
        .unwrap()
        .tab_mut_by_id(source)
        .unwrap()
        .panes
        .get_pane_mut(sibling)
        .unwrap()
        .closed = true;
    assert_refused(
        &move_result(&mut last_live, source as u64, 0, destination as u64, 0),
        "last pane",
    );
    assert_source_intact(&last_live, source);

    let (mut live, source, destination, _) = fixture();
    live.tabs
        .as_mut()
        .unwrap()
        .tab_mut_by_id(source)
        .unwrap()
        .live_domain = Some(crate::tabs::LiveDomainTab {
        domain_id: "fake-domain".into(),
        tab_id: 8,
        pane_ids: HashMap::new(),
    });
    assert_refused(
        &move_result(&mut live, source as u64, 0, destination as u64, 0),
        "Live-domain",
    );
    assert_source_intact(&live, source);

    let (mut serial, source, destination, _) = fixture();
    serial
        .tabs
        .as_mut()
        .unwrap()
        .tab_mut_by_id(destination)
        .unwrap()
        .tab_type = crate::tabs::TabType::Serial {
        name: "fixture".into(),
    };
    assert_refused(
        &move_result(&mut serial, source as u64, 0, destination as u64, 0),
        "serial terminal",
    );
    assert_source_intact(&serial, source);

    let (mut closed_anchor, source, destination, _) = fixture();
    let shell = crate::test_support::sleeping_shell();
    let destination_tab = closed_anchor
        .tabs
        .as_mut()
        .unwrap()
        .tab_mut_by_id(destination)
        .unwrap();
    let destination_panes = destination_tab.panes_for_test(shell, 800.0, 480.0, 10.0, 20.0);
    destination_panes.split(SplitDirection::Vertical).unwrap();
    destination_panes.get_pane_mut(PaneId(0)).unwrap().closed = true;
    assert_refused(
        &move_result(&mut closed_anchor, source as u64, 0, destination as u64, 0),
        "destination pane 0 is closed",
    );
    assert_source_intact(&closed_anchor, source);

    let (mut plugin, source, destination, _) = fixture();
    plugin
        .tabs
        .as_mut()
        .unwrap()
        .tab_mut_by_id(source)
        .unwrap()
        .panes
        .get_pane_mut(PaneId(0))
        .unwrap()
        .launch = crate::workspace::WorkspaceLaunch::Plugin {
        plugin_id: "fake".into(),
        profile: None,
    };
    move_result(&mut plugin, source as u64, 0, destination as u64, 0).unwrap();
    let source_panes = plugin
        .tabs
        .as_ref()
        .unwrap()
        .tab_by_id(source)
        .unwrap()
        .panes
        .open_pane_count();
    assert_eq!(source_panes, 1);

    let (mut last, source, destination, _) = fixture();
    assert_refused(
        &move_result(&mut last, destination as u64, 0, source as u64, 0),
        "Split the source tab",
    );
    assert_source_intact(&last, source);
}

#[test]
fn plugin_pane_closure_is_exact_correlated_and_preserves_last_pane() {
    let shell = crate::test_support::sleeping_shell();
    let mut tabs = TabManager::for_test(shell.clone());
    let target_tab_id = tabs.new_tab().unwrap();
    let tab = tabs.tab_mut_by_id(target_tab_id).unwrap();
    let panes = tab.panes_for_test(shell, 800.0, 480.0, 10.0, 20.0);
    let first_sibling = panes.split(SplitDirection::Vertical).unwrap();
    let second_sibling = panes
        .split_target(first_sibling, SplitDirection::Horizontal)
        .unwrap();
    let active_tab_id = tabs.new_tab().unwrap();
    let mut app = App::new_headless(Config::default()).unwrap();
    app.tabs = Some(tabs);
    app.copy_mode_target = Some(crate::tabs::TerminalIdentity {
        tab_id: target_tab_id,
        pane_id: PaneId(0),
    });
    app.broadcast = BroadcastState::Active {
        tab_id: target_tab_id,
        targets: vec![PaneId(0), first_sibling],
    };

    let closed = plugin_action_event(
        &mut app,
        "pane-pruner",
        crate::plugin::structured::StructuredResponseAction::ClosePane {
            action_id: "close-original".into(),
            window_id: None,
            tab_id: target_tab_id as u64,
            pane_id: 0,
        },
    );
    assert_eq!(closed["event"]["type"], "action-result");
    assert_eq!(closed["event"]["action"], "close-pane");
    assert_eq!(closed["event"]["action_id"], "close-original");
    assert_eq!(closed["event"]["ok"], true);
    assert_eq!(closed["event"]["tab_id"], target_tab_id as u64);
    assert_eq!(closed["event"]["pane_id"], 0);
    assert_eq!(closed["event"]["remaining_panes"], 2);
    assert_eq!(
        closed["event"]["focused_pane_id"],
        u64::from(second_sibling.0)
    );
    assert_eq!(
        app.tabs.as_ref().unwrap().active_tab_id(),
        Some(active_tab_id)
    );
    assert!(app.copy_mode_target.is_none());
    assert!(matches!(app.broadcast, BroadcastState::Inactive));

    let stale = plugin_action_event(
        &mut app,
        "pane-pruner",
        crate::plugin::structured::StructuredResponseAction::ClosePane {
            action_id: "stale-pane".into(),
            window_id: None,
            tab_id: target_tab_id as u64,
            pane_id: u32::MAX,
        },
    );
    assert_eq!(stale["event"]["ok"], false);
    assert!(stale["event"]["remaining_panes"].is_null());
    assert!(stale["event"]["focused_pane_id"].is_null());
    assert!(
        stale["event"]["error"]
            .as_str()
            .is_some_and(|error| error.contains("does not exist"))
    );
    assert_eq!(
        app.tabs
            .as_ref()
            .unwrap()
            .tab_by_id(target_tab_id)
            .unwrap()
            .panes
            .pane_count(),
        2
    );

    app.tabs
        .as_mut()
        .unwrap()
        .tab_mut_by_id(target_tab_id)
        .unwrap()
        .live_domain = Some(crate::tabs::LiveDomainTab {
        domain_id: "fake-domain".into(),
        tab_id: 8,
        pane_ids: HashMap::new(),
    });
    let live = plugin_action_event(
        &mut app,
        "pane-pruner",
        crate::plugin::structured::StructuredResponseAction::ClosePane {
            action_id: "reject-live".into(),
            window_id: None,
            tab_id: target_tab_id as u64,
            pane_id: first_sibling.0,
        },
    );
    assert_eq!(live["event"]["ok"], false);
    assert!(
        live["event"]["error"]
            .as_str()
            .is_some_and(|error| error.contains("live-domain"))
    );

    let tab = app
        .tabs
        .as_mut()
        .unwrap()
        .tab_mut_by_id(target_tab_id)
        .unwrap();
    tab.live_domain = None;
    tab.tab_type = crate::tabs::TabType::Serial {
        name: "fixture".into(),
    };
    let serial = plugin_action_event(
        &mut app,
        "pane-pruner",
        crate::plugin::structured::StructuredResponseAction::ClosePane {
            action_id: "reject-serial".into(),
            window_id: None,
            tab_id: target_tab_id as u64,
            pane_id: first_sibling.0,
        },
    );
    assert_eq!(serial["event"]["ok"], false);
    assert!(
        serial["event"]["error"]
            .as_str()
            .is_some_and(|error| error.contains("Serial"))
    );

    app.tabs
        .as_mut()
        .unwrap()
        .tab_mut_by_id(target_tab_id)
        .unwrap()
        .tab_type = crate::tabs::TabType::Shell;
    let collapsed = plugin_action_event(
        &mut app,
        "pane-pruner",
        crate::plugin::structured::StructuredResponseAction::ClosePane {
            action_id: "collapse-split".into(),
            window_id: None,
            tab_id: target_tab_id as u64,
            pane_id: first_sibling.0,
        },
    );
    assert_eq!(collapsed["event"]["ok"], true);
    assert_eq!(collapsed["event"]["remaining_panes"], 1);
    assert_eq!(
        collapsed["event"]["focused_pane_id"],
        u64::from(second_sibling.0)
    );
    let tab = app.tabs.as_ref().unwrap().tab_by_id(target_tab_id).unwrap();
    assert!(tab.panes.pane_count() == 1);
    assert_eq!(tab.panes.focused_id(), second_sibling);

    let sole = plugin_action_event(
        &mut app,
        "pane-pruner",
        crate::plugin::structured::StructuredResponseAction::ClosePane {
            action_id: "reject-last".into(),
            window_id: None,
            tab_id: target_tab_id as u64,
            pane_id: second_sibling.0,
        },
    );
    assert_eq!(sole["event"]["ok"], false);
    assert!(
        sole["event"]["error"]
            .as_str()
            .is_some_and(|error| error.contains("sole pane"))
    );
    assert_eq!(
        app.tabs.as_ref().unwrap().active_tab_id(),
        Some(active_tab_id)
    );

    app.tabs
        .as_mut()
        .unwrap()
        .tab_mut_by_id(target_tab_id)
        .unwrap()
        .closing_at = Some(Instant::now());
    let closing = plugin_action_event(
        &mut app,
        "pane-pruner",
        crate::plugin::structured::StructuredResponseAction::ClosePane {
            action_id: "reject-closing".into(),
            window_id: None,
            tab_id: target_tab_id as u64,
            pane_id: second_sibling.0,
        },
    );
    assert_eq!(closing["event"]["ok"], false);
    assert!(
        closing["event"]["error"]
            .as_str()
            .is_some_and(|error| error.contains("closing"))
    );

    let missing_tab = plugin_action_event(
        &mut app,
        "pane-pruner",
        crate::plugin::structured::StructuredResponseAction::ClosePane {
            action_id: "missing-tab".into(),
            window_id: None,
            tab_id: u64::MAX,
            pane_id: second_sibling.0,
        },
    );
    assert_eq!(missing_tab["event"]["ok"], false);
    assert!(
        missing_tab["event"]["error"]
            .as_str()
            .is_some_and(|error| error.contains("does not exist") || error.contains("fit"))
    );
}

#[test]
fn plugin_tab_closure_is_scheduled_exact_bounded_and_hook_suppressed() {
    let shell = crate::test_support::sleeping_shell();
    let mut tabs = TabManager::for_test(shell);
    let survivor = tabs.new_tab().unwrap();
    let serial = tabs.new_tab().unwrap();
    let live_candidate = tabs.new_tab().unwrap();
    let initially_active = tabs.new_tab().unwrap();
    tabs.tab_mut_by_id(survivor).unwrap().pinned = true;
    tabs.tab_mut_by_id(serial).unwrap().pinned = true;
    tabs.tab_mut_by_id(serial).unwrap().tab_type = crate::tabs::TabType::Serial {
        name: "fixture".into(),
    };
    let mut app = App::new_headless(Config::default()).unwrap();
    app.tabs = Some(tabs);
    app.plugin_manager.clear();
    let observed = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    app.plugin_manager
        .register(crate::test_support::hook_counter(
            |hook| matches!(hook, crate::plugin::Hook::TabClosed { .. }),
            &observed,
        ))
        .unwrap();

    let background = plugin_action_event(
        &mut app,
        "tab-pruner",
        crate::plugin::structured::StructuredResponseAction::CloseTab {
            action_id: "close-serial-background".into(),
            window_id: None,
            tab_id: serial as u64,
        },
    );
    assert_eq!(background["event"]["type"], "action-result");
    assert_eq!(background["event"]["action"], "close-tab");
    assert_eq!(background["event"]["action_id"], "close-serial-background");
    assert_eq!(background["event"]["ok"], true);
    assert_eq!(background["event"]["tab_id"], serial as u64);
    assert_eq!(background["event"]["scheduled"], true);
    assert_eq!(background["event"]["remaining_tabs"], 3);
    assert_eq!(
        background["event"]["active_tab_id"],
        initially_active as u64
    );
    assert_eq!(background["event"]["active_changed"], false);
    assert!(
        app.tabs
            .as_ref()
            .unwrap()
            .tab_by_id(serial)
            .unwrap()
            .closing_at
            .is_some()
    );
    assert_eq!(
        app.tabs.as_ref().unwrap().active_tab_id(),
        Some(initially_active)
    );
    assert_eq!(observed.load(std::sync::atomic::Ordering::SeqCst), 0);

    app.tabs
        .as_mut()
        .unwrap()
        .tab_mut_by_id(live_candidate)
        .unwrap()
        .live_domain = Some(crate::tabs::LiveDomainTab {
        domain_id: "fake-domain".into(),
        tab_id: 8,
        pane_ids: HashMap::new(),
    });
    let live = plugin_action_event(
        &mut app,
        "tab-pruner",
        crate::plugin::structured::StructuredResponseAction::CloseTab {
            action_id: "reject-live".into(),
            window_id: None,
            tab_id: live_candidate as u64,
        },
    );
    assert_eq!(live["event"]["ok"], false);
    assert_eq!(live["event"]["scheduled"], serde_json::Value::Null);
    assert!(live["event"]["remaining_tabs"].is_null());
    assert!(live["event"]["active_tab_id"].is_null());
    assert!(live["event"]["active_changed"].is_null());
    assert!(
        live["event"]["error"]
            .as_str()
            .is_some_and(|error| error.contains("live-domain"))
    );
    assert!(
        app.tabs
            .as_ref()
            .unwrap()
            .tab_by_id(live_candidate)
            .unwrap()
            .closing_at
            .is_none()
    );

    let tab = app
        .tabs
        .as_mut()
        .unwrap()
        .tab_mut_by_id(live_candidate)
        .unwrap();
    tab.live_domain = None;
    tab.closed = true;
    let closed = plugin_action_event(
        &mut app,
        "tab-pruner",
        crate::plugin::structured::StructuredResponseAction::CloseTab {
            action_id: "reject-closed".into(),
            window_id: None,
            tab_id: live_candidate as u64,
        },
    );
    assert_eq!(closed["event"]["ok"], false);
    assert_eq!(closed["event"]["scheduled"], serde_json::Value::Null);
    assert!(
        closed["event"]["error"]
            .as_str()
            .is_some_and(|error| error.contains("already closing"))
    );
    app.tabs
        .as_mut()
        .unwrap()
        .tab_mut_by_id(live_candidate)
        .unwrap()
        .closed = false;

    let already_closing = plugin_action_event(
        &mut app,
        "tab-pruner",
        crate::plugin::structured::StructuredResponseAction::CloseTab {
            action_id: "already-closing".into(),
            window_id: None,
            tab_id: serial as u64,
        },
    );
    assert_eq!(already_closing["event"]["ok"], false);
    assert!(
        already_closing["event"]["error"]
            .as_str()
            .is_some_and(|error| error.contains("already closing"))
    );

    let missing = plugin_action_event(
        &mut app,
        "tab-pruner",
        crate::plugin::structured::StructuredResponseAction::CloseTab {
            action_id: "missing-tab".into(),
            window_id: None,
            tab_id: u64::MAX,
        },
    );
    assert_eq!(missing["event"]["ok"], false);
    assert!(
        missing["event"]["error"]
            .as_str()
            .is_some_and(|error| error.contains("does not exist") || error.contains("fit"))
    );

    app.tabs
        .as_mut()
        .unwrap()
        .tab_mut_by_id(live_candidate)
        .unwrap()
        .live_domain = None;
    app.copy_mode_target = Some(crate::tabs::TerminalIdentity {
        tab_id: initially_active,
        pane_id: PaneId(0),
    });
    let active = plugin_action_event(
        &mut app,
        "tab-pruner",
        crate::plugin::structured::StructuredResponseAction::CloseTab {
            action_id: "close-active".into(),
            window_id: None,
            tab_id: initially_active as u64,
        },
    );
    assert_eq!(active["event"]["ok"], true);
    assert_eq!(active["event"]["scheduled"], true);
    assert_eq!(active["event"]["remaining_tabs"], 2);
    assert_eq!(active["event"]["active_tab_id"], live_candidate as u64);
    assert_eq!(active["event"]["active_changed"], true);
    assert_eq!(
        app.tabs.as_ref().unwrap().active_tab_id(),
        Some(live_candidate)
    );
    assert!(app.copy_mode_target.is_none());

    let next_active = plugin_action_event(
        &mut app,
        "tab-pruner",
        crate::plugin::structured::StructuredResponseAction::CloseTab {
            action_id: "leave-one".into(),
            window_id: None,
            tab_id: live_candidate as u64,
        },
    );
    assert_eq!(next_active["event"]["ok"], true);
    assert_eq!(next_active["event"]["remaining_tabs"], 1);
    assert_eq!(next_active["event"]["active_tab_id"], survivor as u64);
    assert_eq!(next_active["event"]["active_changed"], true);

    let final_tab = plugin_action_event(
        &mut app,
        "tab-pruner",
        crate::plugin::structured::StructuredResponseAction::CloseTab {
            action_id: "reject-final".into(),
            window_id: None,
            tab_id: survivor as u64,
        },
    );
    assert_eq!(final_tab["event"]["ok"], false);
    assert_eq!(final_tab["event"]["scheduled"], serde_json::Value::Null);
    assert!(
        final_tab["event"]["error"]
            .as_str()
            .is_some_and(|error| error.contains("final non-closing tab"))
    );
    assert!(
        app.tabs
            .as_ref()
            .unwrap()
            .tab_by_id(survivor)
            .unwrap()
            .closing_at
            .is_none()
    );
    assert_eq!(app.tabs.as_ref().unwrap().active_tab_id(), Some(survivor));
    assert_eq!(observed.load(std::sync::atomic::Ordering::SeqCst), 0);
}

#[test]
fn control_tab_closure_retains_final_tab_authority() {
    let shell = crate::test_support::sleeping_shell();
    let mut tabs = TabManager::for_test(shell);
    let tab_id = tabs.new_tab().unwrap();
    let mut app = App::new_headless(Config::default()).unwrap();
    app.tabs = Some(tabs);
    app.plugin_manager.clear();
    let observed = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    app.plugin_manager
        .register(crate::test_support::hook_counter(
            |hook| matches!(hook, crate::plugin::Hook::TabClosed { .. }),
            &observed,
        ))
        .unwrap();

    app.close_tab_for(Requester::Control, tab_id as u64)
        .unwrap();
    assert!(
        app.tabs
            .as_ref()
            .unwrap()
            .tab_by_id(tab_id)
            .unwrap()
            .closing_at
            .is_some()
    );
    assert_eq!(observed.load(std::sync::atomic::Ordering::SeqCst), 1);
}

#[test]
fn plugin_tab_creation_returns_stable_identity_without_lifecycle_redispatch() {
    let shell = crate::test_support::sleeping_shell();
    let mut tabs = TabManager::for_test(shell);
    let initial_tab_id = tabs.new_tab().unwrap();
    let mut app = App::new_headless(Config::default()).unwrap();
    app.tabs = Some(tabs);
    app.plugin_manager.clear();
    let observed = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    app.plugin_manager
        .register(crate::test_support::hook_counter(
            |hook| matches!(hook, crate::plugin::Hook::TabCreated { .. }),
            &observed,
        ))
        .unwrap();

    let created = plugin_action_event(
        &mut app,
        "tab-builder",
        crate::plugin::structured::StructuredResponseAction::CreateTab {
            action_id: "new-task".into(),
            window_id: None,
        },
    );
    assert_eq!(created["event"]["type"], "action-result");
    assert_eq!(created["event"]["action"], "create-tab");
    assert_eq!(created["event"]["action_id"], "new-task");
    assert_eq!(created["event"]["ok"], true);
    let created_tab_id = created["event"]["created_tab_id"].as_u64().unwrap() as usize;
    assert_ne!(created_tab_id, initial_tab_id);
    assert_eq!(app.tabs.as_ref().unwrap().count(), 2);
    assert_eq!(
        app.tabs.as_ref().unwrap().active_tab().unwrap().id,
        created_tab_id
    );
    assert_eq!(observed.load(std::sync::atomic::Ordering::SeqCst), 0);

    let control_tab_id = app.create_tab_for(Requester::Control).unwrap() as usize;
    assert_eq!(app.tabs.as_ref().unwrap().count(), 3);
    assert_eq!(
        app.tabs.as_ref().unwrap().active_tab().unwrap().id,
        control_tab_id
    );
    assert_eq!(observed.load(std::sync::atomic::Ordering::SeqCst), 1);

    app.tabs
        .as_mut()
        .unwrap()
        .active_tab_mut()
        .unwrap()
        .live_domain = Some(crate::tabs::LiveDomainTab {
        domain_id: "fake-domain".into(),
        tab_id: 8,
        pane_ids: HashMap::new(),
    });
    let live = plugin_action_event(
        &mut app,
        "tab-builder",
        crate::plugin::structured::StructuredResponseAction::CreateTab {
            action_id: "reject-live".into(),
            window_id: None,
        },
    );
    assert_eq!(live["event"]["ok"], false);
    assert!(live["event"]["created_tab_id"].is_null());
    assert!(
        live["event"]["error"]
            .as_str()
            .is_some_and(|error| error.contains("live domain"))
    );
    assert_eq!(app.tabs.as_ref().unwrap().count(), 3);

    let tabs = app.tabs.as_mut().unwrap();
    tabs.active_tab_mut().unwrap().live_domain = None;
    tabs.set_shell_config(ShellConfig {
        program: Some("/definitely/missing/termeric-shell".into()),
        args: Vec::new(),
        env: Default::default(),
    });
    let launch_failure = plugin_action_event(
        &mut app,
        "tab-builder",
        crate::plugin::structured::StructuredResponseAction::CreateTab {
            action_id: "failed-launch".into(),
            window_id: None,
        },
    );
    assert_eq!(launch_failure["event"]["ok"], false);
    assert!(launch_failure["event"]["created_tab_id"].is_null());
    assert_eq!(app.tabs.as_ref().unwrap().count(), 3);
    assert_eq!(observed.load(std::sync::atomic::Ordering::SeqCst), 1);

    assert!(
        crate::app::control_actions::validate_ordinary_tab_creation_capacity(
            crate::control::protocol::MAX_TABS_PER_WINDOW,
        )
        .unwrap_err()
        .contains("tab limit")
    );

    let mut uninitialized = App::new_headless(Config::default()).unwrap();
    let unavailable = plugin_action_event(
        &mut uninitialized,
        "tab-builder",
        crate::plugin::structured::StructuredResponseAction::CreateTab {
            action_id: "no-tabs".into(),
            window_id: None,
        },
    );
    assert_eq!(unavailable["event"]["ok"], false);
    assert!(
        unavailable["event"]["error"]
            .as_str()
            .is_some_and(|error| error.contains("tab manager"))
    );
}

#[test]
fn control_input_is_opt_in_exact_paste_aware_and_guarded_without_provider_execution() {
    let script = concat!(
        "stty raw -echo; ",
        "printf '\rREADY\r\n'; ",
        "bytes=$(dd bs=1 count=17 2>/dev/null | od -An -tx1); ",
        "printf '\rinput=%s\r\n' \"$bytes\"; ",
        "sleep 0.1"
    );
    let mut tabs = TabManager::for_test(ShellConfig::default());
    let tab_id = tabs
        .new_tab_with_command(
            "/bin/sh",
            &["-c".into(), script.into()],
            None,
            Some("control-input-fixture"),
            None,
            HashMap::new(),
        )
        .unwrap();
    tabs.active_tab_mut()
        .unwrap()
        .focused_terminal_mut()
        .unwrap()
        .set_mode(termeric_core::term::TerminalMode::BRACKETED_PASTE, true);

    let mut app = App::new_headless(Config::default()).unwrap();
    app.tabs = Some(tabs);
    assert_eq!(
        app.send_text_for(Requester::Control, tab_id as u64, 0, "safe", true, true)
            .unwrap_err(),
        String::from(crate::app::control_actions::ControlInputDisabled)
    );
    app.config.security.allow_gui_control_input = true;
    assert!(
        app.send_text_for(Requester::Control, tab_id as u64, 99, "wrong", false, false)
            .is_err()
    );

    let ready_deadline = Instant::now() + Duration::from_secs(3);
    while Instant::now() < ready_deadline {
        app.tabs.as_mut().unwrap().poll_events();
        if app
            .active_terminal()
            .unwrap()
            .grid()
            .plain_lines()
            .join("\n")
            .contains("READY")
        {
            break;
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    assert_eq!(
        app.send_text_for(Requester::Plugin, tab_id as u64, 0, "safe", true, true),
        Ok((
            5,
            17,
            crate::control::protocol::InputDisposition::Sent,
            None
        ))
    );

    let expected = "1b 5b 32 30 30 7e 73 61 66 65 1b 5b 32 30 31 7e 0d";
    let deadline = Instant::now() + Duration::from_secs(3);
    let mut output = String::new();
    while Instant::now() < deadline {
        app.tabs.as_mut().unwrap().poll_events();
        output = app
            .active_terminal()
            .unwrap()
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
        "exact control input was {output:?}"
    );

    app.tabs
        .as_mut()
        .unwrap()
        .active_tab_mut()
        .unwrap()
        .tab_type = crate::tabs::TabType::Plugin {
        plugin_id: "hermetic-fixture".into(),
    };
    let (_, _, disposition, confirmation_id) = app
        .send_text_for(
            Requester::Control,
            tab_id as u64,
            0,
            "rm -rf /",
            false,
            true,
        )
        .unwrap();
    assert_eq!(
        disposition,
        crate::control::protocol::InputDisposition::PendingConfirmation
    );
    let confirmation_id = confirmation_id.unwrap();
    assert!(app.guardrail.is_awaiting(confirmation_id));
    app.apply_guardrail_action(crate::terminal::guardrails::GuardrailAction::Cancel {
        request_id: confirmation_id,
    });
    assert!(!app.guardrail.is_awaiting(confirmation_id));
}
