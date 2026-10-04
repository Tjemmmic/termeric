//! Movement preserves live local owners; no installed/provider CLI is run.
use super::*;
use crate::session::transcript::{EntryKind, TranscriptEntry, TranscriptRecorder};
use crate::tabs::TerminalIdentity;
use crate::workspace::WorkspaceLaunch;
use termeric_core::pty::PtyEvent;

fn fixture(primary: bool) -> (App, tempfile::TempDir, usize, usize, PaneId) {
    let (mut app, dir, mut snapshot) = super::workspace_plugin_tests::fixture();
    if primary {
        snapshot.primary_pane = 7;
        snapshot.launch = snapshot
            .panes
            .iter()
            .find(|pane| pane.id == 7)
            .unwrap()
            .launch
            .clone()
            .unwrap();
    }
    let mut tabs = super::workspace_plugin_tests::manager(&app);
    app.restore_workspace_tab(&mut tabs, &snapshot).unwrap();
    let source = tabs.active_tab().unwrap().id;
    let destination = tabs.new_tab().unwrap();
    tabs.switch_to(0);
    tabs.active_tab_mut()
        .unwrap()
        .pane_manager
        .as_mut()
        .unwrap()
        .focus(PaneId(7));
    app.tabs = Some(tabs);
    (app, dir, source, destination, PaneId(7))
}

#[test]
fn moved_agent_restart_and_archive_keep_its_resolved_profile() {
    let (mut app, dir, source, destination, owner) = fixture(true);
    let observer = app
        .tabs
        .as_ref()
        .unwrap()
        .tab_by_id(source)
        .unwrap()
        .pane_manager
        .as_ref()
        .unwrap()
        .get_pane(owner)
        .unwrap()
        .pty
        .child_observer();
    let moved = app
        .tabs
        .as_mut()
        .unwrap()
        .move_focused_pane_to_next_tab(app.config.panes.cwd)
        .unwrap();
    let launch = |app: &App| {
        app.tabs
            .as_ref()
            .unwrap()
            .tab_by_id(destination)
            .map(|tab| {
                tab.pane_plugin_launch(moved)
                    .map(|(id, profile)| (id.to_string(), profile.map(str::to_owned)))
            })
    };
    let red = Some(Some(("codex".to_string(), Some("red".to_string()))));
    assert_eq!(launch(&app), red);

    // A replacement that cannot start keeps the moved, live owner.
    let codex = app.config.agents.get_mut("codex").unwrap();
    let program = codex
        .program
        .replace(dir.path().join("not-installed").display().to_string());
    app.sync_plugins_from_config();
    app.restart_plugin("codex");
    assert!(!observer.poll().unwrap().terminal_closed);
    assert_eq!(app.tabs.as_ref().unwrap().count(), 2);
    assert_eq!(launch(&app), red);
    app.config.agents.get_mut("codex").unwrap().program = program;

    // A distinct capture directory proves this invocation, not the initial
    // fixture's marker.
    let captures = dir.path().join("restart-captures");
    std::fs::create_dir(&captures).unwrap();
    for profile in app
        .config
        .agents
        .get_mut("codex")
        .unwrap()
        .profiles
        .values_mut()
    {
        profile
            .env
            .insert("CAPTURE_DIR".into(), captures.display().to_string());
    }
    app.sync_plugins_from_config();
    app.restart_plugin("codex");
    let captured = captures.join("red");
    wait_for_file_text(&captured, "--fixture=red");
    assert_eq!(
        std::fs::read_to_string(captured)
            .unwrap()
            .lines()
            .next()
            .unwrap(),
        dir.path().join("cwd-7").display().to_string()
    );
    let tabs = app.tabs.as_ref().unwrap();
    let replacement = tabs.active_tab().unwrap();
    assert_ne!(replacement.id, destination);
    assert!(tabs.origin_for_pane(destination, moved).is_none());
    assert!(observer.poll().unwrap().terminal_closed);
    assert_eq!(
        replacement.pane_plugin_launch(replacement.single_pane_id()),
        Some(("codex", Some("red")))
    );
    let mut saved = tabs
        .capture_workspace("restart-roundtrip".into(), false, Default::default())
        .unwrap();
    let archive = dir.path().join("restart-archive");
    crate::workspace::save_workspace_to_dir(&mut saved, &archive).unwrap();
    let loaded = crate::workspace::load_workspace_from_dir(&archive)
        .unwrap()
        .workspace;
    assert_eq!(
        loaded.tabs.last().unwrap().launch,
        WorkspaceLaunch::Plugin {
            plugin_id: "codex".into(),
            profile: Some("red".into())
        }
    );
    // A later default change must not reinterpret the saved name.
    app.config.agents.get_mut("codex").unwrap().default_profile = Some("blue".into());
    app.sync_plugins_from_config();
    let mut restored = super::workspace_plugin_tests::manager(&app);
    for tab in &loaded.tabs {
        app.restore_workspace_tab(&mut restored, tab).unwrap();
    }
    let tab = restored.active_tab().unwrap();
    assert_eq!(
        tab.pane_plugin_launch(tab.single_pane_id()),
        Some(("codex", Some("red")))
    );
}

#[test]
fn plugin_transfer_keeps_streaming_transcript_diff_and_read_only_state_with_one_pty() {
    for route in ["exact", "next-tab", "window"] {
        let (mut app, dir, source, destination, owner) = fixture(true);
        app.tabs.as_mut().unwrap().switch_to(1);
        let anchor = app
            .launch_plugin_pane(
                TerminalIdentity {
                    tab_id: destination,
                    pane_id: PaneId(0),
                },
                SplitDirection::Vertical,
                "codex",
                Some("blue"),
            )
            .unwrap();
        app.tabs.as_mut().unwrap().switch_to(0);
        let recorder = TranscriptRecorder::new(dir.path(), None, None, "Local", route).unwrap();
        let path = recorder.path().to_owned();
        let (sender, receiver) = std::sync::mpsc::channel();
        let (pid, observer) = {
            let pane = app
                .tabs
                .as_mut()
                .unwrap()
                .tab_mut_by_id(source)
                .unwrap()
                .pane_manager
                .as_mut()
                .unwrap()
                .get_pane_mut(owner)
                .unwrap();
            pane.pty_rx = receiver;
            pane.integration.start_transcript(recorder);
            (pane.pty.child_id(), pane.pty.child_observer())
        };
        let prefix =
            b"diff --git a/file b/file\n--- a/file\n+++ b/file\n@@ -1 +1 @@\n-old\n+new \xf0\x9f";
        sender.send(PtyEvent::Output(prefix.to_vec())).unwrap();
        let tabs = app.tabs.as_mut().unwrap();
        tabs.poll_events();
        let old_origin = tabs.origin_for_pane(source, owner).unwrap();
        tabs.write_user_to_origin(old_origin, b"input \xc2")
            .unwrap();
        let pane = tabs
            .tab_mut_by_id(source)
            .unwrap()
            .pane_manager
            .as_mut()
            .unwrap()
            .get_pane_mut(owner)
            .unwrap();
        pane.read_only = true;
        let mut other_window = None;
        let moved = match route {
            "exact" => PaneId(
                app.move_control_pane_within_window(
                    source as u64,
                    owner.0,
                    destination as u64,
                    anchor.0,
                    SplitDirection::Horizontal,
                )
                .unwrap(),
            ),
            "next-tab" => {
                app.enter_copy_mode();
                assert!(app.copy_mode.is_active());
                app.broadcast = BroadcastState::Active {
                    tab_id: source,
                    targets: vec![owner, PaneId(0)],
                };
                app.terminal_mouse_capture = Some(TerminalMouseCapture {
                    owner: crate::tabs::TerminalIdentity {
                        tab_id: source,
                        pane_id: owner,
                    },
                    button: MouseReportButton::Left,
                });
                app.move_pane_to_next_tab();
                assert!(matches!(app.broadcast, BroadcastState::Inactive));
                assert!(!app.copy_mode.is_active());
                assert!(app.terminal_mouse_capture.is_none());
                app.tabs
                    .as_ref()
                    .unwrap()
                    .active_tab()
                    .unwrap()
                    .pane_manager
                    .as_ref()
                    .unwrap()
                    .focused_id()
            }
            "window" => {
                // Use the focused cross-window handoff used by the application,
                // with a separate local ID namespace and no process recreation.
                let mut manager = super::workspace_plugin_tests::manager(&app);
                manager.new_tab().unwrap();
                manager
                    .prepare_window_pane_transfer_destination(app.config.panes.cwd)
                    .unwrap();
                let pane = app
                    .tabs
                    .as_mut()
                    .unwrap()
                    .detach_focused_pane_for_window_transfer()
                    .unwrap();
                let moved = manager.attach_window_transferred_pane(pane);
                other_window = Some(manager);
                moved
            }
            _ => unreachable!(),
        };
        assert!(
            app.tabs
                .as_ref()
                .unwrap()
                .origin_for_pane(source, owner)
                .is_none()
        );
        assert!(
            app.tabs
                .as_mut()
                .unwrap()
                .write_user_to_origin(old_origin, b"STALE")
                .is_err()
        );
        let tabs = other_window
            .as_mut()
            .unwrap_or_else(|| app.tabs.as_mut().unwrap());
        let destination_id = tabs.active_tab().unwrap().id;
        let origin = tabs.origin_for_pane(destination_id, moved).unwrap();
        let pane = tabs
            .active_tab()
            .unwrap()
            .pane_manager
            .as_ref()
            .unwrap()
            .get_pane(moved)
            .unwrap();
        assert_eq!(pane.pty.child_id(), pid);
        assert!(!observer.poll().unwrap().terminal_closed);
        assert!(pane.read_only);
        assert_eq!(pane.launch_profile(), Some("red"));
        assert!(pane.integration.transcript.is_some());
        assert_eq!(pane.integration.transcript.as_ref().unwrap().path(), path);
        assert!(tabs.write_user_to_origin(origin, b"\xa2REFUSED").is_err());
        tabs.write_to_origin(origin, b"PROTOCOL_ONLY").unwrap();
        tabs.active_tab_mut()
            .unwrap()
            .pane_manager
            .as_mut()
            .unwrap()
            .get_pane_mut(moved)
            .unwrap()
            .read_only = false;
        tabs.write_user_to_origin(origin, b"\xa2 moved").unwrap();
        sender
            .send(PtyEvent::Output(b"\x8c\xb1\nfinished\n".to_vec()))
            .unwrap();
        tabs.poll_events();
        let state = tabs
            .active_tab_mut()
            .unwrap()
            .pane_session_mut(moved)
            .unwrap();
        assert_eq!(state.diff_detector.unreviewed_count(), 1);
        let mut recorder = state.stop_transcript().unwrap();
        recorder.finish(Duration::from_secs(2)).unwrap();
        let entries: Vec<TranscriptEntry> = std::fs::read_to_string(path)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        let text = |kind| {
            entries
                .iter()
                .filter(|entry| entry.kind == kind)
                .map(|entry| entry.content.as_str())
                .collect::<String>()
        };
        assert_eq!(text(EntryKind::Input), "input ¢ moved");
        assert!(text(EntryKind::Output).contains("+new 🌱\nfinished\n"));
        assert_eq!(
            entries
                .iter()
                .filter(|entry| entry.kind == EntryKind::Diff)
                .count(),
            1
        );
        let captured = tabs
            .capture_workspace("transferred".into(), false, Default::default())
            .unwrap();
        captured.validate().unwrap();
        if route != "window" {
            assert_eq!(
                tabs.tab_by_id(destination)
                    .unwrap()
                    .pane_plugin_launch(anchor),
                Some(("codex", Some("blue")))
            );
            assert!(
                tabs.tab_by_id(destination)
                    .unwrap()
                    .pane_session(anchor)
                    .unwrap()
                    .transcript
                    .is_none()
            );
        }
    }
}

#[test]
fn plugin_pane_moves_to_next_tab_without_relaunching_primary_or_sibling_owner() {
    for primary in [false, true] {
        let (mut app, _dir, source, destination, owner) = fixture(primary);
        let pane = app
            .tabs
            .as_ref()
            .unwrap()
            .tab_by_id(source)
            .unwrap()
            .pane_manager
            .as_ref()
            .unwrap()
            .get_pane(owner)
            .unwrap();
        let pid = pane.pty.child_id();
        let observer = pane.pty.child_observer();
        let moved = app
            .tabs
            .as_mut()
            .unwrap()
            .move_focused_pane_to_next_tab(app.config.panes.cwd)
            .unwrap();
        let tabs = app.tabs.as_ref().unwrap();
        assert_eq!(tabs.active_tab().unwrap().id, destination);
        assert!(tabs.origin_for_pane(source, owner).is_none());
        let pane = tabs
            .tab_by_id(destination)
            .unwrap()
            .pane_manager
            .as_ref()
            .unwrap()
            .get_pane(moved)
            .unwrap();
        assert_eq!(pane.pty.child_id(), pid);
        assert!(!observer.poll().unwrap().terminal_closed);
        assert_eq!(
            pane.launch,
            WorkspaceLaunch::Plugin {
                plugin_id: "codex".into(),
                profile: Some("red".into())
            }
        );
        assert_eq!(pane.launch_profile(), Some("red"));
        let mut saved = tabs
            .capture_workspace("moved".into(), false, Default::default())
            .unwrap();
        let archive = _dir.path().join("moved-workspace");
        crate::workspace::save_workspace_to_dir(&mut saved, &archive).unwrap();
        let loaded = crate::workspace::load_workspace_from_dir(&archive)
            .unwrap()
            .workspace;
        let mut restored = super::workspace_plugin_tests::manager(&app);
        for tab in &loaded.tabs {
            app.restore_workspace_tab(&mut restored, tab).unwrap();
        }
        assert_eq!(restored.count(), 2);
        assert_eq!(
            restored.all_tabs()[1].pane_plugin_launch(moved),
            Some(("codex", Some("red")))
        );
    }
}

#[test]
fn focused_moves_reject_exhausted_collapsed_destinations_before_detaching() {
    for divider in [false, true] {
        let (mut app, _dir, source, destination, owner) = fixture(true);
        let tabs = app.tabs.as_mut().unwrap();
        let tab = tabs.tab_mut_by_id(destination).unwrap();
        let mut terminal = tab.take_terminal_owner().unwrap();
        if divider {
            terminal.allocation.next_divider = u64::MAX;
        } else {
            terminal.allocation.next_pane = u32::MAX;
        }
        let observer = terminal.pane.pty.child_observer();
        let mut panes = PaneManager::from_existing(
            terminal,
            app.config.shell.clone(),
            1000,
            8 * 1024 * 1024,
            1200.0,
            800.0,
            10.0,
            20.0,
        );
        tab.restore_terminal_owner(panes.take_remaining_pane().unwrap());
        let error = tabs
            .move_focused_pane_to_next_tab(app.config.panes.cwd)
            .unwrap_err()
            .to_string();
        assert!(
            error.contains(if divider { "divider ID" } else { "pane ID" }),
            "{error}"
        );
        assert_eq!(tabs.active_tab().unwrap().id, source);
        assert!(tabs.origin_for_pane(source, owner).is_some());
        assert_eq!(
            tabs.tab_by_id(source)
                .unwrap()
                .pane_manager
                .as_ref()
                .unwrap()
                .pane_count(),
            5
        );
        assert!(tabs.tab_by_id(destination).unwrap().pane_manager.is_none());
        tabs.switch_to(1);
        assert!(
            tabs.prepare_window_pane_transfer_destination(app.config.panes.cwd)
                .is_err()
        );
        assert!(tabs.tab_by_id(destination).unwrap().pane_manager.is_none());
        assert!(!observer.poll().unwrap().terminal_closed);
    }
}
