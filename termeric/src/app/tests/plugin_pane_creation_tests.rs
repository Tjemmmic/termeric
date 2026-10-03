//! Real local PTYs with temporary fake binaries only; never a provider CLI.
use super::*;
use crate::tabs::TerminalIdentity;
use crate::workspace::WorkspaceLaunch;

fn fixture() -> (App, tempfile::TempDir, TerminalIdentity) {
    let (mut app, dir, _) = super::workspace_plugin_tests::fixture();
    // Include the kernel's geometry as well as the launch environment.
    std::fs::write(dir.path().join("fake-workspace-plugin"), "#!/bin/sh\nstty raw -echo\nprintf '%s\\n' \"$PWD\" \"$TEST_TAG\" \"$*\" \"$COLUMNS\" \"$LINES\" \"$(stty size)\" > \"$CAPTURE_DIR/$TEST_TAG\"\nexec cat >/dev/null\n").unwrap();
    let mut tabs = super::workspace_plugin_tests::manager(&app);
    let tab_id = tabs
        .new_tab_with_command(
            "/bin/sh",
            &["-c".into(), "exec cat >/dev/null".into()],
            Some(dir.path().join("cwd-0").to_str().unwrap()),
            None,
            None,
            Default::default(),
        )
        .unwrap();
    let target = TerminalIdentity {
        tab_id,
        pane_id: tabs.active_tab().unwrap().single_pane_id(),
    };
    app.tabs = Some(tabs);
    (app, dir, target)
}

#[test]
fn explicit_plugin_panes_spawn_one_owner_with_exact_profile_cwd_and_geometry() {
    let (mut app, dir, target) = fixture();
    let original = app
        .tabs
        .as_ref()
        .unwrap()
        .active_tab()
        .unwrap()
        .child_observer()
        .unwrap();
    let mut original_pid = None;
    for (plugin, profile, direction) in [
        ("codex", "red", SplitDirection::Vertical),
        ("codex", "blue", SplitDirection::Horizontal),
        ("claude-code", "green", SplitDirection::Vertical),
        ("codex", "default", SplitDirection::Horizontal),
    ] {
        let expected_geometry = app
            .tabs
            .as_ref()
            .unwrap()
            .plugin_pane_geometry(target, direction)
            .unwrap();
        let id = app
            .launch_plugin_pane(target, direction, plugin, Some(profile))
            .unwrap();
        wait_for_file_text(&dir.path().join(profile), &format!("--fixture={profile}"));
        let capture = std::fs::read_to_string(dir.path().join(profile)).unwrap();
        let lines: Vec<_> = capture.lines().collect();
        assert_eq!(lines[0], dir.path().join("cwd-0").display().to_string());
        assert_eq!(lines[3], expected_geometry.cols.to_string());
        assert_eq!(lines[4], expected_geometry.rows.to_string());
        assert_eq!(
            lines[5],
            format!("{} {}", expected_geometry.rows, expected_geometry.cols)
        );
        let tab = app.tabs.as_ref().unwrap().active_tab().unwrap();
        let panes = tab.pane_manager.as_ref().unwrap();
        assert_eq!(panes.focused_id(), id);
        let pid = panes
            .get_pane(target.pane_id)
            .unwrap()
            .pty
            .child_id()
            .unwrap();
        assert_eq!(*original_pid.get_or_insert(pid), pid);
        assert!(!original.poll().unwrap().terminal_closed);
        let pane = panes.get_pane(id).unwrap();
        assert_eq!(
            pane.launch,
            WorkspaceLaunch::Plugin {
                plugin_id: plugin.into(),
                profile: Some(profile.into())
            }
        );
        assert_eq!(
            pane.launch_profile(),
            Some(profile),
            "the launch identity carries the profile the agent scan reports"
        );
        assert!(pane.integration.transcript.is_none());
        assert!(matches!(
            pane.shell_integration,
            crate::terminal::shell_integration::ShellIntegrationSupport::Unavailable(_)
        ));
        assert!(
            app.tabs
                .as_ref()
                .unwrap()
                .validate_pane_transfer_source_by_id(target.tab_id, id)
                .is_ok()
        );
    }
    let snapshot = app
        .tabs
        .as_ref()
        .unwrap()
        .capture_workspace("created plugins".into(), false, Default::default())
        .unwrap();
    assert_eq!(snapshot.tabs[0].panes.len(), 5);
    snapshot.validate().unwrap();
    let mut restored = super::workspace_plugin_tests::manager(&app);
    app.restore_workspace_tab(&mut restored, &snapshot.tabs[0])
        .unwrap();
    let captured = restored
        .active_tab()
        .unwrap()
        .capture_workspace_tab(0, false)
        .unwrap();
    for pane in &snapshot.tabs[0].panes {
        assert_eq!(
            captured
                .panes
                .iter()
                .find(|candidate| candidate.id == pane.id)
                .unwrap()
                .launch,
            pane.launch
        );
    }
    let tab = app.tabs.as_mut().unwrap().active_tab_mut().unwrap();
    let panes = tab.pane_manager.as_mut().unwrap();
    let ordinary = panes.split(SplitDirection::Vertical).unwrap();
    assert_eq!(
        panes.get_pane(ordinary).unwrap().launch,
        WorkspaceLaunch::Shell
    );
    assert!(
        panes
            .get_pane(ordinary)
            .unwrap()
            .integration
            .agent
            .is_none()
    );
}

#[test]
fn agents_split_through_the_picker_instead_of_per_agent_commands() {
    use crate::ui::command_palette::palette::AgentPlacement;
    let (mut app, dir, _) = fixture();
    app.refresh_launch_profile_commands();
    assert!(
        app.plugin_pane_commands()
            .iter()
            .all(|command| !command.id.contains(".codex.") && !command.id.contains(".claude-code.")),
        "agents never get a split command per agent and profile"
    );
    let pane_count = |app: &App| {
        app.tabs
            .as_ref()
            .unwrap()
            .active_tab()
            .unwrap()
            .pane_manager
            .as_ref()
            .map_or(1, |panes| panes.pane_count())
    };
    app.start_picked_agent(AgentPlacement::SplitRight, "codex/red");
    wait_for_file_text(&dir.path().join("red"), "--fixture=red");
    assert_eq!(app.tabs.as_ref().unwrap().count(), 1);
    assert_eq!(pane_count(&app), 2);

    app.config.agents.get_mut("codex").unwrap().enabled = Some(false);
    app.sync_plugins_from_config();
    app.start_picked_agent(AgentPlacement::SplitRight, "codex/blue");
    assert!(!dir.path().join("blue").exists());
    assert_eq!(pane_count(&app), 2);
}

#[test]
fn created_plugin_owner_survives_close_collapse_and_resplit_with_its_exact_profile() {
    let (mut app, _dir, target) = fixture();
    let id = app
        .launch_plugin_pane(target, SplitDirection::Vertical, "codex", Some("default"))
        .unwrap();
    let before = app
        .tabs
        .as_ref()
        .unwrap()
        .active_tab()
        .unwrap()
        .pane_manager
        .as_ref()
        .unwrap()
        .get_pane(id)
        .unwrap()
        .pty
        .child_id();
    app.close_pane_for(Requester::Plugin, target.tab_id as u64, target.pane_id.0)
        .unwrap();
    let tab = app.tabs.as_ref().unwrap().active_tab().unwrap();
    assert!(tab.pane_manager.is_none());
    assert_eq!(tab.single_pane_id(), id);
    assert_eq!(tab.pane_plugin_launch(id), Some(("codex", Some("default"))));
    let source = TerminalIdentity {
        tab_id: target.tab_id,
        pane_id: id,
    };
    let sibling = app
        .launch_plugin_pane(source, SplitDirection::Horizontal, "codex", Some("blue"))
        .unwrap();
    assert!(sibling.0 > id.0);
    let panes = app
        .tabs
        .as_ref()
        .unwrap()
        .active_tab()
        .unwrap()
        .pane_manager
        .as_ref()
        .unwrap();
    assert_eq!(panes.get_pane(id).unwrap().pty.child_id(), before);
    assert_eq!(
        panes.get_pane(id).unwrap().launch_profile(),
        Some("default")
    );
    assert_eq!(
        panes.get_pane(sibling).unwrap().launch_profile(),
        Some("blue")
    );
}

#[test]
fn plugin_pane_spawn_failure_rolls_back_single_and_split_owners() {
    for split in [false, true] {
        let (mut app, _dir, target) = fixture();
        if split {
            app.launch_plugin_pane(target, SplitDirection::Vertical, "codex", Some("red"))
                .unwrap();
            app.tabs
                .as_mut()
                .unwrap()
                .active_tab_mut()
                .unwrap()
                .pane_manager
                .as_mut()
                .unwrap()
                .toggle_zoom();
        }
        let before = app
            .tabs
            .as_ref()
            .unwrap()
            .active_tab()
            .unwrap()
            .capture_workspace_tab(0, false)
            .unwrap();
        app.config.agents.get_mut("claude-code").unwrap().program =
            Some("/nonexistent-termeric-plugin-fixture".into());
        app.sync_plugins_from_config();
        assert!(
            app.launch_plugin_pane(
                target,
                SplitDirection::Horizontal,
                "claude-code",
                Some("green")
            )
            .is_err()
        );
        let tab = app.tabs.as_ref().unwrap().active_tab().unwrap();
        assert_eq!(tab.pane_manager.is_some(), split);
        if split {
            assert_eq!(
                tab.pane_manager.as_ref().unwrap().zoomed_id(),
                Some(PaneId(1))
            );
        }
        let after = tab.capture_workspace_tab(0, false).unwrap();
        assert_eq!(before.layout, after.layout);
        assert_eq!(before.focused_pane, after.focused_pane);
        assert_eq!(before.panes.len(), after.panes.len());
        let next = app
            .launch_plugin_pane(target, SplitDirection::Horizontal, "codex", Some("blue"))
            .unwrap();
        assert_eq!(next.0, if split { 2 } else { 1 });
    }
}

#[test]
fn explicit_plugin_panes_respect_target_tab_and_process_cwd_modes() {
    use crate::config::PaneCwdMode;
    for mode in [PaneCwdMode::Focused, PaneCwdMode::Tab, PaneCwdMode::Process] {
        let (mut app, dir, target) = fixture();
        let pane = app
            .launch_plugin_pane(target, SplitDirection::Vertical, "codex", Some("red"))
            .unwrap();
        let reported = dir.path().join("cwd-7");
        let panes = app
            .tabs
            .as_mut()
            .unwrap()
            .active_tab_mut()
            .unwrap()
            .pane_manager
            .as_mut()
            .unwrap();
        panes
            .get_pane_mut(pane)
            .unwrap()
            .terminal
            .process(format!("\x1b]7;file://localhost{}\x1b\\", reported.display()).as_bytes());
        panes.focus(target.pane_id); // The explicit destination, not focus, owns cwd.
        app.config.panes.cwd = mode;
        app.launch_plugin_pane(
            TerminalIdentity {
                tab_id: target.tab_id,
                pane_id: pane,
            },
            SplitDirection::Horizontal,
            "codex",
            Some("blue"),
        )
        .unwrap();
        wait_for_file_text(&dir.path().join("blue"), "--fixture=blue");
        let expected = match mode {
            PaneCwdMode::Focused => reported,
            PaneCwdMode::Tab => dir.path().join("cwd-0"),
            PaneCwdMode::Process => std::env::current_dir().unwrap(),
        };
        let capture = std::fs::read_to_string(dir.path().join("blue")).unwrap();
        assert_eq!(
            capture.lines().next().unwrap(),
            expected.display().to_string()
        );
    }
}

#[test]
fn plugin_pane_launch_refuses_stale_read_only_missing_profile_and_broadcast_targets() {
    for refusal in [
        "stale",
        "closed",
        "closing",
        "serial",
        "domain",
        "read-only",
        "profile",
        "broadcast",
    ] {
        let (mut app, dir, mut target) = fixture();
        match refusal {
            "stale" => target.pane_id = PaneId(999),
            "closed" => app.tabs.as_mut().unwrap().active_tab_mut().unwrap().closed = true,
            "closing" => {
                app.tabs
                    .as_mut()
                    .unwrap()
                    .active_tab_mut()
                    .unwrap()
                    .closing_at = Some(Instant::now())
            }
            "serial" => {
                app.tabs
                    .as_mut()
                    .unwrap()
                    .active_tab_mut()
                    .unwrap()
                    .tab_type = crate::tabs::TabType::Serial {
                    name: "synthetic".into(),
                }
            }
            "domain" => {
                app.tabs
                    .as_mut()
                    .unwrap()
                    .active_tab_mut()
                    .unwrap()
                    .live_domain = Some(crate::tabs::LiveDomainTab {
                    domain_id: "synthetic".into(),
                    tab_id: 0,
                    pane_ids: Default::default(),
                })
            }
            "read-only" => {
                app.tabs
                    .as_mut()
                    .unwrap()
                    .active_tab_mut()
                    .unwrap()
                    .toggle_focused_pane_read_only();
            }
            "profile" => {
                app.config
                    .agents
                    .get_mut("codex")
                    .unwrap()
                    .profiles
                    .remove("red");
                app.sync_plugins_from_config();
            }
            "broadcast" => {
                app.broadcast = BroadcastState::Active {
                    tab_id: target.tab_id,
                    targets: vec![target.pane_id],
                };
            }
            _ => unreachable!(),
        }
        assert!(
            app.launch_plugin_pane(target, SplitDirection::Vertical, "codex", Some("red"))
                .is_err(),
            "{refusal}"
        );
        assert!(!dir.path().join("red").exists());
        assert!(
            app.tabs
                .as_ref()
                .unwrap()
                .active_tab()
                .unwrap()
                .pane_manager
                .is_none()
        );
    }
}
