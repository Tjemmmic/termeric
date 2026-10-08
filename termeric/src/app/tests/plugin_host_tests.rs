//! Structured plugin completions, workspace queries, and plugin window closes.

use super::*;

#[test]
fn structured_plugin_completions_require_a_live_generation_and_plugin() {
    let mut app = App::new_headless(Config::default()).unwrap();
    app.plugin_manager
        .register(crate::test_support::identity_plugin(
            "completion-fixture",
            "Completion Fixture",
        ))
        .unwrap();
    let completion =
        |generation, message: &str| crate::plugin::structured::StructuredHandlerCompletion {
            window_id: 1,
            plugin_generation: generation,
            plugin_id: "completion-fixture".into(),
            request_id: 7,
            continuation: crate::plugin::structured::StructuredHandlerInvocation::fixture(
                "completion-fixture",
                Default::default(),
            ),
            result: Ok(vec![
                crate::plugin::structured::StructuredResponseAction::ShowMessage {
                    message: message.into(),
                },
            ]),
        };

    assert!(
        app.accept_structured_plugin_completion(completion(
            app.plugin_runtime_generation.wrapping_sub(1),
            "stale",
        ))
        .is_none()
    );
    assert!(app.toasts.iter().all(|toast| toast.message != "stale"));

    assert!(
        app.accept_structured_plugin_completion(completion(
            app.plugin_runtime_generation,
            "current"
        ))
        .is_none()
    );
    assert!(app.toasts.iter().any(|toast| toast.message == "current"));

    app.plugin_manager.clear();
    assert!(
        app.accept_structured_plugin_completion(completion(
            app.plugin_runtime_generation,
            "unloaded"
        ))
        .is_none()
    );
    assert!(app.toasts.iter().all(|toast| toast.message != "unloaded"));
}

#[test]
fn structured_completion_hands_its_host_operation_to_the_application() {
    use crate::plugin::structured::{
        StructuredHandlerCompletion, StructuredHandlerInvocation, StructuredResponseAction,
    };

    let mut app = App::new_headless(Config::default()).unwrap();
    app.plugin_manager
        .register(crate::test_support::identity_plugin(
            "operation-fixture",
            "Operation Fixture",
        ))
        .unwrap();
    let select = StructuredResponseAction::SelectTab {
        action_id: "select".into(),
        window_id: None,
        tab_id: 1,
    };
    let generation = app.plugin_runtime_generation;
    let completion = StructuredHandlerCompletion {
        window_id: 1,
        plugin_generation: generation,
        plugin_id: "operation-fixture".into(),
        request_id: 3,
        continuation: StructuredHandlerInvocation::fixture(
            "operation-fixture",
            crate::plugin::permission::PluginPermissions::default(),
        ),
        result: Ok(vec![
            select.clone(),
            StructuredResponseAction::ShowMessage {
                message: "selecting".into(),
            },
        ]),
    };
    let host = app.accept_structured_plugin_completion(completion).unwrap();
    assert_eq!(host.plugin_id, "operation-fixture");
    assert_eq!(host.operation, select);
    assert!(app.toasts.iter().any(|toast| toast.message == "selecting"));
}

#[test]
fn plugin_window_close_validation_never_tears_down_the_window() {
    let shell = crate::test_support::sleeping_shell();
    let mut tabs = TabManager::for_test(shell);
    tabs.new_tab_with_command(
        "/bin/sh",
        &crate::test_support::sleeping_args(),
        None,
        Some("close fixture"),
        None,
        HashMap::new(),
    )
    .unwrap();
    let mut app = App::new_headless(Config::default()).unwrap();
    app.tabs = Some(tabs);

    assert_eq!(app.validate_plugin_window_close(), Ok(1));
    assert!(!app.should_exit);
    assert!(app.pending_plugin_window_close.is_none());

    app.pending_plugin_window_close = Some(PendingPluginWindowClose {
        plugin_id: "window-closer".into(),
        plugin_generation: app.plugin_runtime_generation,
        result_request_id: 9,
    });
    assert!(
        app.validate_plugin_window_close()
            .unwrap_err()
            .contains("pending plugin closure")
    );
    app.pending_plugin_window_close = None;
    app.should_exit = true;
    assert!(
        app.validate_plugin_window_close()
            .unwrap_err()
            .contains("already closing")
    );
}

#[test]
fn plugin_window_close_waits_for_only_the_exact_result_callback() {
    let mut app = App::new_headless(Config::default()).unwrap();
    let generation = app.plugin_runtime_generation.wrapping_sub(1);
    app.pending_plugin_window_close = Some(PendingPluginWindowClose {
        plugin_id: "window-closer".into(),
        plugin_generation: generation,
        result_request_id: 41,
    });
    let completion = |request_id, result| crate::plugin::structured::StructuredHandlerCompletion {
        window_id: 1,
        plugin_generation: generation,
        plugin_id: "window-closer".into(),
        request_id,
        continuation: crate::plugin::structured::StructuredHandlerInvocation::fixture(
            "window-closer",
            Default::default(),
        ),
        result,
    };

    assert!(
        app.accept_structured_plugin_completion(completion(40, Ok(Vec::new())))
            .is_none()
    );
    assert!(!app.should_exit);
    assert!(app.pending_plugin_window_close.is_some());

    assert!(
        app.accept_structured_plugin_completion(completion(
            41,
            Ok(vec![
                crate::plugin::structured::StructuredResponseAction::ShowMessage {
                    message: "must not render during teardown".into(),
                },
            ]),
        ))
        .is_none()
    );
    assert!(app.should_exit);
    assert!(app.pending_plugin_window_close.is_none());
    assert!(
        app.toasts
            .iter()
            .all(|toast| toast.message != "must not render during teardown")
    );
}

#[test]
fn plugin_window_close_still_finishes_after_final_handler_failure() {
    let mut app = App::new_headless(Config::default()).unwrap();
    app.pending_plugin_window_close = Some(PendingPluginWindowClose {
        plugin_id: "window-closer".into(),
        plugin_generation: app.plugin_runtime_generation,
        result_request_id: 73,
    });
    app.accept_structured_plugin_completion(
        crate::plugin::structured::StructuredHandlerCompletion {
            window_id: 1,
            plugin_generation: app.plugin_runtime_generation,
            plugin_id: "window-closer".into(),
            request_id: 73,
            continuation: crate::plugin::structured::StructuredHandlerInvocation::fixture(
                "window-closer",
                Default::default(),
            ),
            result: Err("fixture callback failed".into()),
        },
    );
    assert!(app.should_exit);
    assert!(app.pending_plugin_window_close.is_none());
}

#[test]
fn structured_workspace_query_is_current_window_bounded_and_control_safe() {
    let shell = crate::test_support::sleeping_shell();
    let mut tabs = TabManager::for_test(shell);
    tabs.new_tab_with_command(
        "/bin/sh",
        &crate::test_support::sleeping_args(),
        None,
        Some("unsafe\u{1b} title"),
        None,
        HashMap::new(),
    )
    .unwrap();
    let first_tab_id = tabs.active_tab().unwrap().id;
    let mut last_tab_id = first_tab_id;
    for index in 1..17 {
        last_tab_id = tabs
            .new_tab_with_command(
                "/bin/sh",
                &["-c".into(), "exit 0".into()],
                None,
                Some(&format!("tab-{index}-{}", "x".repeat(1024))),
                None,
                HashMap::new(),
            )
            .unwrap();
    }

    let mut app = App::new_headless(Config::default()).unwrap();
    app.tabs = Some(tabs);
    app.current_workspace_id = Some("workspace\u{7}fixture".into());
    // At window scope the inventory holds only the plugin's own window.
    let inventory =
        super::application::inventory::plugin_inventory(&[(app.control_window_id, &app)]);
    let encoded = super::plugin_actions::plugin_workspace_query_event(
        "query-fixture",
        app.control_window_id,
        "inventory-1",
        &inventory,
    )
    .unwrap();
    assert!(encoded.len() <= crate::plugin::structured::MAX_PERSISTENT_SOURCE_EVENT_BYTES);
    assert!(!encoded.contains("presented-secret"));
    let value: serde_json::Value = serde_json::from_str(&encoded).unwrap();
    assert_eq!(value["schema_version"], 1);
    assert_eq!(value["plugin_id"], "query-fixture");
    assert_eq!(value["origin_window_id"], app.control_window_id);
    assert_eq!(value["event"]["type"], "workspace-query-result");
    assert_eq!(value["event"]["query_id"], "inventory-1");
    let windows = value["event"]["windows"].as_array().unwrap();
    assert_eq!(windows.len(), 1);
    let window = &windows[0];
    assert_eq!(window["id"], app.control_window_id);
    // The shared metadata budget may truncate the ID, never pass controls.
    assert!("workspacefixture".starts_with(window["workspace_id"].as_str().unwrap()));
    assert_eq!(window["tabs"].as_array().unwrap().len(), 16);
    assert!(window["truncated"].as_bool().unwrap());
    assert_eq!(window["tabs"][0]["id"], first_tab_id);
    assert_eq!(window["tabs"][0]["title"], "unsafe title");
    assert_eq!(window["tabs"][16 - 1]["active"], false);
    assert_eq!(window["tabs"][0]["panes"][0]["cols"], 80);
    assert_eq!(window["tabs"][0]["panes"][0]["rows"], 24);

    // The event loop has already reported the window's active tab.
    app.report_active_tab_switch();
    let observed_switches = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    app.plugin_manager
        .register(crate::test_support::hook_counter(
            |hook| matches!(hook, crate::plugin::Hook::TabSwitched { .. }),
            &observed_switches,
        ))
        .unwrap();

    let failed = plugin_action_event(
        &mut app,
        "workspace-navigator",
        crate::plugin::structured::StructuredResponseAction::FocusPane {
            action_id: "missing-pane".into(),
            window_id: None,
            tab_id: first_tab_id as u64,
            pane_id: u32::MAX,
        },
    );
    assert_eq!(failed["event"]["type"], "action-result");
    assert_eq!(failed["event"]["action_id"], "missing-pane");
    assert_eq!(failed["event"]["action"], "focus-pane");
    assert_eq!(failed["event"]["ok"], false);
    assert!(
        failed["event"]["error"]
            .as_str()
            .is_some_and(|error| { !error.is_empty() && !error.chars().any(char::is_control) })
    );
    assert_eq!(
        app.tabs.as_ref().unwrap().active_tab_id(),
        Some(last_tab_id)
    );

    let selected = plugin_action_event(
        &mut app,
        "workspace-navigator",
        crate::plugin::structured::StructuredResponseAction::SelectTab {
            action_id: "select-first".into(),
            window_id: None,
            tab_id: first_tab_id as u64,
        },
    );
    assert_eq!(selected["plugin_id"], "workspace-navigator");
    assert_eq!(selected["event"]["ok"], true);
    assert_eq!(selected["event"]["changed"], true);
    assert_eq!(
        app.tabs.as_ref().unwrap().active_tab_id(),
        Some(first_tab_id)
    );
    // The next pass of the event loop has nothing left to report.
    app.report_active_tab_switch();
    assert_eq!(
        observed_switches.load(std::sync::atomic::Ordering::SeqCst),
        0
    );

    // The ordinary authenticated control route still emits the lifecycle hook;
    // only plugin-origin navigation is suppressed to prevent action loops.
    assert_eq!(
        app.select_tab_for(Requester::Control, last_tab_id as u64),
        Ok(true)
    );
    assert_eq!(
        observed_switches.load(std::sync::atomic::Ordering::SeqCst),
        1
    );

    let pane_id = u32::try_from(
        value["event"]["windows"][0]["tabs"][0]["panes"][0]["id"]
            .as_u64()
            .unwrap(),
    )
    .unwrap();
    let focused = plugin_action_event(
        &mut app,
        "workspace-navigator",
        crate::plugin::structured::StructuredResponseAction::FocusPane {
            action_id: "focus-first".into(),
            window_id: None,
            tab_id: first_tab_id as u64,
            pane_id,
        },
    );
    assert_eq!(focused["event"]["ok"], true);
    assert_eq!(focused["event"]["tab_changed"], true);
    assert_eq!(focused["event"]["pane_changed"], false);
    app.report_active_tab_switch();
    assert_eq!(
        observed_switches.load(std::sync::atomic::Ordering::SeqCst),
        1
    );
}

/// One tab-switched hook's `from` and `to`.
type TabSwitch = (Option<usize>, usize);

/// Records every tab-switched hook a window raises.
#[derive(Debug)]
struct TabSwitchRecorder(std::sync::Arc<std::sync::Mutex<Vec<TabSwitch>>>);

impl crate::plugin::hooks::Plugin for TabSwitchRecorder {
    fn id(&self) -> &str {
        "tab-switch-recorder"
    }

    fn name(&self) -> &str {
        "Tab Switch Recorder"
    }

    fn on_hook(
        &mut self,
        hook: &crate::plugin::Hook,
        _ctx: &crate::plugin::PluginContext,
    ) -> Option<crate::plugin::PluginAction> {
        if let crate::plugin::Hook::TabSwitched { from, to } = hook {
            self.0.lock().unwrap().push((*from, *to));
        }
        None
    }
}

#[test]
fn every_active_tab_change_raises_one_tab_switched_by_tab_id() {
    let shell = crate::test_support::sleeping_shell();
    let mut tabs = TabManager::for_test(shell);
    let first = tabs.new_tab().unwrap();
    let second = tabs.new_tab().unwrap();
    let mut app = App::new_headless(Config::default()).unwrap();
    app.tabs = Some(tabs);
    app.plugin_manager.clear();
    let switches = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    app.plugin_manager
        .register(Box::new(TabSwitchRecorder(switches.clone())))
        .unwrap();
    let take = || std::mem::take(&mut *switches.lock().unwrap());
    let index_of = |app: &App, id: usize| {
        app.tabs
            .as_ref()
            .unwrap()
            .iter()
            .find_map(|(index, tab)| (tab.id == id).then_some(index))
            .unwrap()
    };

    // A window's first report names no earlier tab; nothing changes after.
    app.report_active_tab_switch();
    assert_eq!(take(), vec![(None, second)]);
    app.report_active_tab_switch();
    assert!(take().is_empty());

    // Keyboard navigation: one event per change, in tab IDs.
    app.handle_action(crate::input::Action::PrevTab);
    app.report_active_tab_switch();
    assert_eq!(take(), vec![(Some(second), first)]);

    // A new tab becomes active without any explicit switch.
    let third = app.create_local_tab_in(true, None).unwrap();
    app.report_active_tab_switch();
    assert_eq!(take(), vec![(Some(first), third)]);

    // Closing the active tab activates another; `from` is the closed tab.
    let third_index = index_of(&app, third);
    assert!(app.tabs.as_mut().unwrap().close_tab(third_index));
    let neighbour = app.tabs.as_ref().unwrap().active_tab_id().unwrap();
    assert_ne!(neighbour, third);
    app.report_active_tab_switch();
    assert_eq!(take(), vec![(Some(third), neighbour)]);

    // A control client's selection is reported at once.
    let other = if neighbour == first { second } else { first };
    assert_eq!(
        app.select_tab_for(Requester::Control, other as u64),
        Ok(true)
    );
    assert_eq!(take(), vec![(Some(neighbour), other)]);

    // A plugin's own selection raises nothing, now or later, while a change
    // made before it keeps its event.
    let neighbour_index = index_of(&app, neighbour);
    app.handle_action(crate::input::Action::SwitchToTab(neighbour_index));
    assert_eq!(
        app.select_tab_for(Requester::Plugin, other as u64),
        Ok(true)
    );
    assert_eq!(take(), vec![(Some(other), neighbour)]);
    app.report_active_tab_switch();
    assert!(take().is_empty());

    // The next change is reported from the tab the plugin selected.
    app.handle_action(crate::input::Action::SwitchToTab(neighbour_index));
    app.report_active_tab_switch();
    assert_eq!(take(), vec![(Some(other), neighbour)]);
}

#[test]
fn plugin_context_cwd_follows_the_focused_pane_and_single_terminal() {
    let temporary = tempfile::tempdir().unwrap();
    let left_cwd = temporary.path().join("left-context");
    let right_cwd = temporary.path().join("right-context");
    let single_cwd = temporary.path().join("single-context");
    for directory in [&left_cwd, &right_cwd, &single_cwd] {
        std::fs::create_dir(directory).unwrap();
    }

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
    let right = panes.split(SplitDirection::Vertical).unwrap();
    panes
        .get_pane_mut(PaneId(0))
        .unwrap()
        .terminal
        .process(format!("\x1b]7;file://{}\x07", left_cwd.display()).as_bytes());
    panes
        .get_pane_mut(right)
        .unwrap()
        .terminal
        .process(format!("\x1b]7;file://{}\x07", right_cwd.display()).as_bytes());
    panes.focus(PaneId(0));

    let mut tabs = TabManager::for_test(ShellConfig::default());
    tabs.new_tab_with_command(
        "/bin/sh",
        &crate::test_support::sleeping_args(),
        None,
        Some("plugin-context-routing"),
        None,
        HashMap::new(),
    )
    .unwrap();
    tabs.active_tab_mut().unwrap().panes = panes;
    let mut app = App::new_headless(Config::default()).unwrap();
    app.tabs = Some(tabs);

    assert_eq!(app.build_plugin_context().cwd.as_deref(), left_cwd.to_str());
    app.active_panes_mut().focus(right);
    app.refresh_focused_terminal_surfaces();
    assert_eq!(
        app.build_plugin_context().cwd.as_deref(),
        right_cwd.to_str()
    );

    let mut single_tabs = TabManager::for_test(ShellConfig::default());
    single_tabs
        .new_tab_with_command(
            "/bin/sh",
            &crate::test_support::sleeping_args(),
            None,
            Some("single-plugin-context"),
            None,
            HashMap::new(),
        )
        .unwrap();
    single_tabs
        .active_tab_mut()
        .unwrap()
        .focused_terminal_mut()
        .unwrap()
        .process(format!("\x1b]7;file://{}\x07", single_cwd.display()).as_bytes());
    app.tabs = Some(single_tabs);
    assert_eq!(
        app.build_plugin_context().cwd.as_deref(),
        single_cwd.to_str()
    );
}
