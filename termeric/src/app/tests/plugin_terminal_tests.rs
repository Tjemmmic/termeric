//! Synthetic plugin launch metadata on local raw capture shells only.
use super::*;

#[test]
fn unavailable_plugin_restart_preserves_the_original_mixed_tab() {
    let (mut app, _directory, _, captures) = harvest_rerun_capture_fixture();
    mark_capture_plugin_owner(&mut app, PaneId(0), "synthetic-capture");
    app.tabs
        .as_mut()
        .unwrap()
        .active_tab_mut()
        .unwrap()
        .tab_type = crate::tabs::TabType::Plugin {
        plugin_id: "synthetic-capture".into(),
    };
    app.execute_plugin_actions(vec![crate::plugin::PluginAction::RestartPlugin {
        plugin_id: "synthetic-capture".into(),
    }]);
    let tab = app.tabs.as_ref().unwrap().active_tab().unwrap();
    assert!(
        !tab.is_closing(),
        "an unavailable replacement must not close the old tab"
    );
    assert_eq!(tab.pane_manager.as_ref().unwrap().pane_count(), 3);
    assert!(
        fenced_harvest_capture(&mut app, &captures)
            .iter()
            .all(Vec::is_empty)
    );
}

#[test]
fn plugin_terminal_selection_prefers_active_tab_without_a_writable_fallback() {
    let (mut app, directory, entry, mut captures) = harvest_rerun_capture_fixture_in(false);
    mark_capture_plugin_owner(&mut app, PaneId(0), "synthetic-capture");
    app.toggle_focused_pane_read_only();
    let second_tab = app.tabs.as_mut().unwrap().new_tab().unwrap();
    mark_capture_plugin_owner(&mut app, PaneId(0), "synthetic-capture");
    captures.extend(current_harvest_capture_routes(&mut app, directory.path()));
    let tabs = app.tabs.as_mut().unwrap();
    assert_eq!(
        tabs.plugin_terminal_identity("synthetic-capture")
            .unwrap()
            .tab_id,
        second_tab
    );
    app.select_control_tab(entry.source.tab_id as u64).unwrap();
    let tabs = app.tabs.as_mut().unwrap();
    assert!(!tabs.write_to_plugin_terminal("synthetic-capture", b"must-not-arrive"));
    assert!(
        fenced_harvest_capture(&mut app, &captures)
            .iter()
            .all(Vec::is_empty)
    );
    app.select_control_tab(entry.source.tab_id as u64).unwrap();
    app.toggle_focused_pane_read_only();
    app.select_control_tab(second_tab as u64).unwrap();
    app.toggle_focused_pane_read_only();
    assert!(
        !app.tabs
            .as_mut()
            .unwrap()
            .write_to_plugin_terminal("synthetic-capture", b"no-writable-fallback")
    );
    app.toggle_focused_pane_read_only();
    assert!(
        app.tabs
            .as_mut()
            .unwrap()
            .write_to_plugin_terminal("synthetic-capture", b"active-owner")
    );
    assert_eq!(
        fenced_harvest_capture(&mut app, &captures),
        vec![vec![0], b"\0active-owner".to_vec()]
    );
    assert_eq!(app.tabs.as_ref().unwrap().active_tab_id(), Some(second_tab));
    app.tabs.as_mut().unwrap().new_tab().unwrap();
    assert_eq!(
        app.tabs
            .as_ref()
            .unwrap()
            .plugin_terminal_identity("synthetic-capture")
            .unwrap()
            .tab_id,
        entry.source.tab_id
    );
}

#[test]
fn plugin_terminal_commands_use_live_launch_owners_in_mixed_tabs() {
    for clear in [false, true] {
        for transition in [
            "mixed-shell-tab",
            "plugin-tab",
            "collapse",
            "nonzero-collapse",
            "missing",
            "read-only",
            "closed",
            "closing",
            "pane-closed",
            "broadcast",
        ] {
            let (mut app, directory, entry, _) = harvest_rerun_capture_fixture();
            let owner = if transition == "nonzero-collapse" {
                PaneId(2)
            } else {
                PaneId(0)
            };
            mark_capture_plugin_owner(&mut app, owner, "synthetic-capture");
            let tab = app.tabs.as_mut().unwrap().active_tab_mut().unwrap();
            if transition == "plugin-tab" {
                tab.tab_type = crate::tabs::TabType::Plugin {
                    plugin_id: "synthetic-capture".into(),
                };
            }
            for pane_id in [PaneId(0), PaneId(1), PaneId(2)] {
                let text = if pane_id == owner {
                    "plugin-owner-output"
                } else {
                    "shell-sibling-output"
                };
                tab.pane_manager
                    .as_mut()
                    .unwrap()
                    .get_pane_mut(pane_id)
                    .unwrap()
                    .terminal
                    .process(format!("\r\n{text}\r\n").as_bytes());
            }
            if transition == "collapse" {
                app.close_control_pane(entry.source.tab_id as u64, 2)
                    .unwrap();
                app.close_control_pane(entry.source.tab_id as u64, 1)
                    .unwrap();
            } else if transition == "nonzero-collapse" {
                app.close_control_pane(entry.source.tab_id as u64, 0)
                    .unwrap();
                app.close_control_pane(entry.source.tab_id as u64, 1)
                    .unwrap();
            } else if transition == "missing" {
                app.close_control_pane(entry.source.tab_id as u64, owner.0)
                    .unwrap();
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
                        .get_pane_mut(owner)
                        .unwrap()
                        .closed = true
                }
                "read-only" => {
                    tab.pane_manager
                        .as_mut()
                        .unwrap()
                        .get_pane_mut(owner)
                        .unwrap()
                        .read_only = true
                }
                "broadcast" => app.toggle_broadcast(),
                _ => {}
            }
            let focused = app.active_harvest_source();
            let available = !matches!(transition, "missing" | "closed" | "closing" | "pane-closed");
            let writable = available && transition != "read-only";
            let copied = app
                .tabs
                .as_ref()
                .unwrap()
                .copy_from_plugin_terminal("synthetic-capture", usize::MAX);
            assert_eq!(copied.is_some(), available, "{transition}");
            if let Some(text) = copied {
                assert!(text.contains("plugin-owner-output"));
                assert!(!text.contains("shell-sibling-output"));
            }
            if clear {
                app.execute_plugin_actions(vec![crate::plugin::PluginAction::ClearPluginTab {
                    plugin_id: "synthetic-capture".into(),
                }]);
            } else {
                assert_eq!(
                    app.inject_context_to_plugin("synthetic-capture", "fixture"),
                    writable,
                    "{transition}"
                );
            }
            let bytes = fenced_harvest_capture(&mut app, &captures);
            let route = app
                .tabs
                .as_ref()
                .unwrap()
                .origin_for_pane(entry.source.tab_id, owner);
            for ((origin, _), data) in captures.iter().zip(bytes) {
                let expected: &[u8] = if writable && Some(*origin) == route {
                    if clear {
                        b"\x03clear\r"
                    } else {
                        b"\n--- Context ---\nfixture\n--- End Context ---\n"
                    }
                } else {
                    &[]
                };
                assert_eq!(data, expected, "clear={clear} {transition}");
            }
            assert_eq!(app.active_harvest_source(), focused, "{transition}");
        }
    }
}

#[test]
fn plugin_owner_selection_prefers_matching_focus_and_never_bypasses_read_only() {
    let (mut app, _directory, entry, captures) = harvest_rerun_capture_fixture();
    mark_capture_plugin_owner(&mut app, PaneId(0), "synthetic-capture");
    mark_capture_plugin_owner(&mut app, PaneId(1), "synthetic-capture");
    let tabs = app.tabs.as_mut().unwrap();
    // A focused shell sibling is not a match; select the first visual plugin.
    assert_eq!(
        tabs.plugin_terminal_identity("synthetic-capture")
            .unwrap()
            .pane_id,
        PaneId(0)
    );
    tabs.active_tab_mut()
        .unwrap()
        .pane_manager
        .as_mut()
        .unwrap()
        .get_pane_mut(PaneId(0))
        .unwrap()
        .read_only = true;
    assert!(!tabs.write_to_plugin_terminal("synthetic-capture", b"must-not-arrive"));
    assert!(
        fenced_harvest_capture(&mut app, &captures)
            .iter()
            .all(Vec::is_empty)
    );
    app.focus_plugin_pane(entry.source.tab_id as u64, 1)
        .unwrap();
    let tabs = app.tabs.as_mut().unwrap();
    assert_eq!(
        tabs.plugin_terminal_identity("synthetic-capture")
            .unwrap()
            .pane_id,
        PaneId(1)
    );
    assert!(tabs.write_to_plugin_terminal("synthetic-capture", b"exact"));
    assert_eq!(
        fenced_harvest_capture(&mut app, &captures),
        vec![vec![0], b"\0exact".to_vec(), vec![0]]
    );
}

#[test]
fn plugin_jump_to_error_focuses_owner_and_preserves_sibling_scroll_state() {
    let (mut app, _directory, entry, captures) = harvest_rerun_capture_fixture();
    mark_capture_plugin_owner(&mut app, PaneId(0), "synthetic-capture");
    let panes = app
        .tabs
        .as_mut()
        .unwrap()
        .active_tab_mut()
        .unwrap()
        .pane_manager
        .as_mut()
        .unwrap();
    for id in [PaneId(0), PaneId(2)] {
        let terminal = &mut panes.get_pane_mut(id).unwrap().terminal;
        terminal.process(b"\r\nerror: fixture failure\r\n");
        for _ in 0..60 {
            terminal.process(b"fixture line\r\n");
        }
        terminal
            .grid_mut()
            .set_scroll_offset(if id == PaneId(2) { 3 } else { 0 });
    }
    assert_eq!(panes.focused_id(), PaneId(2));
    app.jump_to_last_error("synthetic-capture");
    let source = app.active_harvest_source().unwrap();
    assert_eq!((source.tab_id, source.pane_id), (entry.source.tab_id, 0));
    let panes = app
        .tabs
        .as_ref()
        .unwrap()
        .active_tab()
        .unwrap()
        .pane_manager
        .as_ref()
        .unwrap();
    assert!(
        panes
            .get_pane(PaneId(0))
            .unwrap()
            .terminal
            .grid()
            .scroll_offset()
            > 0
    );
    assert_eq!(
        panes
            .get_pane(PaneId(2))
            .unwrap()
            .terminal
            .grid()
            .scroll_offset(),
        3
    );
    app.jump_to_last_error("missing-plugin");
    assert_eq!(app.active_harvest_source(), Some(source));
    assert!(
        fenced_harvest_capture(&mut app, &captures)
            .iter()
            .all(Vec::is_empty)
    );
}
