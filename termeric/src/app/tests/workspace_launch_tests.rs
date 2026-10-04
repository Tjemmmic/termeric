//! Workspace round trips, restore recovery, and CLI and control launches.

use super::*;

#[test]
fn complete_workspace_round_trip_restores_nested_panes_plugin_profile_and_focus() {
    let temp = tempfile::tempdir().unwrap();
    let fake_cli = temp.path().join("workspace-ai");
    std::fs::write(
        &fake_cli,
        "#!/bin/sh\nif [ \"${1-}\" = --version ]; then exit 0; fi\nexec sleep 30\n",
    )
    .unwrap();
    let mut permissions = std::fs::metadata(&fake_cli).unwrap().permissions();
    permissions.set_mode(0o755);
    std::fs::set_permissions(&fake_cli, permissions).unwrap();

    crate::workspace::set_test_workspaces_base_dir(Some(temp.path().join("workspaces")));
    AppState::set_test_state_path(Some(temp.path().join("state/state.json")));

    {
        let mut config = Config::default();
        config.shell.program = Some("/bin/sh".into());
        config.shell.args = vec!["-c".into(), "sleep 30".into()];
        config.agents.insert(
            "codex".into(),
            crate::agents::AgentConfig {
                program: Some(fake_cli.display().to_string()),
                default_profile: Some("archived".into()),
                profiles: std::collections::BTreeMap::from([(
                    "archived".into(),
                    Default::default(),
                )]),
                ..Default::default()
            },
        );
        let mut app = App::new_headless(config).unwrap();
        let mut tabs = TabManager::new(
            80,
            24,
            ShellConfig {
                program: Some("/bin/sh".into()),
                args: vec!["-c".into(), "sleep 30".into()],
                env: Default::default(),
            },
            1_000,
            8 * 1024 * 1024,
        );
        tabs.new_tab().unwrap();
        {
            let tab = tabs.active_tab_mut().unwrap();
            tab.title = "nested-shell".into();
            let panes = tab.panes_for_test(
                ShellConfig {
                    program: Some("/bin/sh".into()),
                    args: vec!["-c".into(), "sleep 30".into()],
                    env: Default::default(),
                },
                800.0,
                480.0,
                10.0,
                20.0,
            );
            let right = panes.split(SplitDirection::Vertical).unwrap();
            panes.focus(right);
            let lower_right = panes.split(SplitDirection::Horizontal).unwrap();
            panes.focus(lower_right);
        }
        app.tabs = Some(tabs);
        app.execute_plugin_command("codex.open");
        let report = app
            .save_workspace(crate::app::workspaces::WorkspaceSave::New)
            .unwrap();

        app.tabs.as_mut().unwrap().new_tab().unwrap();
        let outcome = app.restore_workspace_archive(&report.id).unwrap();
        assert_eq!(outcome.restored_tabs, 2);
        assert_eq!(outcome.skipped_tabs, 0);
        let tabs = app.tabs.as_ref().unwrap();
        assert_eq!(tabs.count(), 2);
        assert_eq!(tabs.active_index(), 1);
        let shell = tabs.iter().find(|(index, _)| *index == 0).unwrap().1;
        let panes = &shell.panes;
        assert_eq!(panes.pane_count(), 3);
        assert_eq!(panes.focused_id(), PaneId(2));
        let plugin = tabs.iter().find(|(index, _)| *index == 1).unwrap().1;
        assert!(matches!(
            &plugin.tab_type,
            crate::tabs::TabType::Plugin { plugin_id } if plugin_id == "codex"
        ));
        assert_eq!(
            plugin
                .pane_plugin_launch(plugin.focused_terminal_identity().pane_id)
                .and_then(|(_, profile)| profile),
            Some("archived")
        );
        assert_eq!(app.app_state.last_workspace_id(), Some(report.id.as_str()));
    }

    crate::workspace::set_test_workspaces_base_dir(None);
    AppState::set_test_state_path(None);
}

#[test]
fn workspace_restore_partially_recovers_missing_cwd_profile_and_cli() {
    let temp = tempfile::tempdir().unwrap();
    let fake_cli = temp.path().join("available-ai");
    std::fs::write(
        &fake_cli,
        "#!/bin/sh\nif [ \"${1-}\" = --version ]; then exit 0; fi\nexec sleep 30\n",
    )
    .unwrap();
    let mut permissions = std::fs::metadata(&fake_cli).unwrap().permissions();
    permissions.set_mode(0o755);
    std::fs::set_permissions(&fake_cli, permissions).unwrap();
    crate::workspace::set_test_workspaces_base_dir(Some(temp.path().join("workspaces")));
    AppState::set_test_state_path(Some(temp.path().join("state/state.json")));

    let pane = |id, cwd: Option<&str>, launch| crate::workspace::WorkspacePaneSnapshot {
        id,
        launch,
        title: None,
        working_directory: cwd.map(String::from),
        last_command: None,
        scrollback_file: None,
        scrollback_truncated: false,
        scrollback_lines: Vec::new(),
    };
    let tab = |id, title: &str, launch: crate::workspace::WorkspaceLaunch, cwd| {
        crate::workspace::WorkspaceTabSnapshot {
            id,
            title: title.into(),
            user_title: None,
            pinned: false,
            primary_pane: 0,
            focused_pane: 0,
            layout: crate::workspace::WorkspacePaneLayout::Pane { pane: 0 },
            panes: vec![pane(0, cwd, launch)],
        }
    };
    let mut archive = crate::workspace::WorkspaceSnapshot::new(
        "workspace-degraded".into(),
        "Degraded restore".into(),
        vec![
            tab(
                0,
                "recoverable shell",
                crate::workspace::WorkspaceLaunch::Shell,
                Some("/definitely/missing/termeric-workspace-cwd"),
            ),
            tab(
                1,
                "missing profile",
                crate::workspace::WorkspaceLaunch::Plugin {
                    plugin_id: "claude-code".into(),
                    profile: Some("removed".into()),
                },
                None,
            ),
            tab(
                2,
                "missing cli",
                crate::workspace::WorkspaceLaunch::Plugin {
                    plugin_id: "codex".into(),
                    profile: None,
                },
                None,
            ),
        ],
    );
    archive.active_tab = 2;
    crate::workspace::save_workspace(&mut archive).unwrap();

    {
        let mut config = Config::default();
        config.shell.program = Some("/bin/sh".into());
        config.shell.args = vec!["-c".into(), "sleep 30".into()];
        config
            .agents
            .entry("claude-code".into())
            .or_default()
            .program = Some(fake_cli.display().to_string());
        config.agents.entry("codex".into()).or_default().program =
            Some(temp.path().join("missing-codex").display().to_string());
        let mut app = App::new_headless(config).unwrap();
        let mut current = TabManager::new(80, 24, ShellConfig::default(), 100, 1024 * 1024);
        current
            .new_tab_with_command(
                "/bin/sh",
                &["-c".into(), "sleep 30".into()],
                None,
                Some("original"),
                None,
                HashMap::new(),
            )
            .unwrap();
        app.tabs = Some(current);

        let outcome = app.restore_workspace_archive("workspace-degraded").unwrap();
        assert_eq!(outcome.restored_tabs, 1);
        assert_eq!(outcome.skipped_tabs, 2);
        assert!(outcome.warnings.iter().any(|line| line.contains("cwd")));
        assert!(
            outcome
                .warnings
                .iter()
                .any(|line| line.contains("no longer configured"))
        );
        assert!(
            outcome
                .warnings
                .iter()
                .any(|line| line.contains("missing or unusable"))
        );
        assert!(
            outcome
                .warnings
                .iter()
                .any(|line| line.starts_with("tab 2 (plugin \"codex\") was skipped:")),
            "{:?}",
            outcome.warnings
        );
        assert!(
            outcome
                .warnings
                .iter()
                .any(|line| line.contains("saved focused tab"))
        );
        assert_eq!(app.tabs.as_ref().unwrap().count(), 1);
        assert_eq!(
            app.tabs.as_ref().unwrap().active_tab().unwrap().title,
            "recoverable shell"
        );
    }

    crate::workspace::set_test_workspaces_base_dir(None);
    AppState::set_test_state_path(None);
}

#[test]
fn cli_launch_plan_creates_ordered_shell_and_arbitrary_command_tabs() {
    let temp = tempfile::tempdir().unwrap();
    let config = Config {
        shell: crate::test_support::sleeping_shell(),
        ..Config::default()
    };
    let mut app = App::new_headless(config).unwrap();
    app.allow_auto_restore = false;
    app.launch = crate::cli::WindowLaunch {
        domain: None,
        domain_snapshot: None,
        adopts_tab: false,
        force_new: true,
        workspace: None,
        tabs: vec![
            crate::cli::TabLaunch {
                working_directory: Some(temp.path().to_path_buf()),
                title: Some("CLI shell".into()),
                ..crate::cli::TabLaunch::default()
            },
            crate::cli::TabLaunch {
                working_directory: Some(temp.path().to_path_buf()),
                title: Some("CLI command".into()),
                target: crate::cli::LaunchTarget::Command {
                    program: "/bin/sh".into(),
                    args: vec!["-c".into(), "printf CLI_ARGV_28; sleep 1".into()],
                },
                environment_policy: Default::default(),
                environment: HashMap::new(),
                splits: Vec::new(),
            },
        ],
        app_id: None,
        pixel_size: None,
        quick_access: false,
    };
    app.init_tabs().unwrap();
    assert_eq!(app.tabs.as_ref().unwrap().count(), 2);
    assert_eq!(app.tabs.as_ref().unwrap().active_index(), 1);
    assert_eq!(tabs_at(&app, 0).unwrap().title, "CLI shell");
    assert_eq!(tabs_at(&app, 1).unwrap().title, "CLI command");

    let deadline = Instant::now() + Duration::from_secs(2);
    while Instant::now() < deadline {
        app.process_pty_events();
        let found = tabs_at(&app, 1)
            .and_then(|tab| tab.focused_terminal())
            .is_some_and(|terminal| {
                terminal
                    .grid()
                    .plain_lines()
                    .join("\n")
                    .contains("CLI_ARGV_28")
            });
        if found {
            return;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    panic!("CLI arbitrary-command output did not reach its tab");
}

#[test]
fn background_command_launch_preserves_focused_pane_and_interaction_surfaces() {
    for copy_mode in [false, true] {
        let shell = ShellConfig {
            program: Some("/bin/sh".into()),
            args: vec!["-c".into(), "read unused".into()],
            env: Default::default(),
        };
        let mut app = App::new_headless(Config::default()).unwrap();
        let mut tabs = TabManager::new(80, 24, shell.clone(), 100, 1024 * 1024);
        let original = tabs
            .new_tab_with_command(
                "/bin/sh",
                &shell.args,
                None,
                Some("original"),
                None,
                HashMap::new(),
            )
            .unwrap();
        let panes = tabs
            .active_tab_mut()
            .unwrap()
            .panes_for_test(shell, 800.0, 480.0, 10.0, 20.0);
        let focused = panes.split(SplitDirection::Vertical).unwrap();
        panes.focus(focused);
        panes
            .focused_pane_mut()
            .unwrap()
            .terminal
            .process(b"kept content");
        app.tabs = Some(tabs);
        app.search.start();
        app.with_search_and_active_grid(|search, grid| {
            search.set_query("kept", grid);
        });
        let selection = termeric_core::Selection::new(
            termeric_core::Point::new(0, 0),
            termeric_core::SelectionType::Simple,
        );
        app.mouse_state.selection = Some(selection.clone());
        app.active_terminal_mut()
            .unwrap()
            .set_selection(Some(selection));
        if copy_mode {
            app.enter_copy_mode();
            assert!(app.copy_mode_target.is_some());
        } else {
            app.handle_ime_event(winit::event::Ime::Preedit(
                "composition".into(),
                Some((0, 0)),
            ));
            assert!(app.ime.preedit().is_some());
        }
        let ime = app.ime.clone();
        let query = app.search.query().to_string();
        if !copy_mode {
            assert_eq!(query, "kept");
        }
        let matches = app.search.match_count();
        let copy_target = app.copy_mode_target;
        let selection = app.active_terminal().unwrap().selection().cloned();
        let mouse_selection = app.mouse_state.selection.clone();
        app.redraw_scheduler.begin_frame();
        assert!(!app.redraw_scheduler.has_pending());
        let created = app
            .launch_control_tab(
                true,
                "/bin/sh",
                &["-c".into(), "printf background-ready; read unused".into()],
                None,
                Some("background"),
                &[],
                &Default::default(),
            )
            .unwrap();
        assert_ne!(created, original as u64);
        assert!(
            app.redraw_scheduler.has_pending(),
            "quiet creation must redraw tab chrome"
        );
        assert_eq!(app.tabs.as_ref().unwrap().active_tab_id(), Some(original));
        assert_eq!(app.active_panes().focused_pane().unwrap().id, focused);
        assert_eq!(app.ime, ime);
        assert_eq!(app.copy_mode_target, copy_target);
        let selection_state = |selection: Option<&termeric_core::Selection>| {
            selection.map(|selection| {
                (
                    selection.start,
                    selection.end,
                    selection.ty,
                    selection.active,
                )
            })
        };
        assert_eq!(
            selection_state(app.active_terminal().unwrap().selection()),
            selection_state(selection.as_ref())
        );
        assert_eq!(
            selection_state(app.mouse_state.selection.as_ref()),
            selection_state(mouse_selection.as_ref())
        );
        assert_eq!(app.search.query(), query);
        assert_eq!(app.search.match_count(), matches);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        loop {
            app.process_pty_events();
            let text = tabs_at(&app, created as usize)
                .unwrap()
                .focused_terminal()
                .unwrap()
                .grid()
                .plain_lines()
                .join("\n");
            if text.contains("background-ready") {
                break;
            }
            assert!(std::time::Instant::now() < deadline);
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert_eq!(app.tabs.as_ref().unwrap().active_tab_id(), Some(original));
        assert_eq!(app.ime, ime);
        assert_eq!(app.copy_mode_target, copy_target);
        assert_eq!(app.search.query(), query);
        assert_eq!(app.search.match_count(), matches);
        assert_eq!(
            selection_state(app.active_terminal().unwrap().selection()),
            selection_state(selection.as_ref())
        );
        let count = app.tabs.as_ref().unwrap().count();
        assert!(
            app.launch_control_tab(
                true,
                "/termeric-test/no-such-program",
                &[],
                None,
                None,
                &[],
                &Default::default()
            )
            .is_err()
        );
        assert_eq!(app.tabs.as_ref().unwrap().count(), count);
        assert_eq!(app.tabs.as_ref().unwrap().active_tab_id(), Some(original));
        assert_eq!(app.copy_mode_target, copy_target);
    }
}

#[test]
fn running_gui_launch_tab_preserves_argv_cwd_title_and_failure_atomicity() {
    let temp = tempfile::tempdir().unwrap();
    let cwd = temp.path().to_str().unwrap();
    let helper = temp.path().join("capture-argv.sh");
    std::fs::write(
        &helper,
        "#!/bin/sh\nprintf '[%s]|[%s]|[%s]|[%s]\\n' \"$1\" \"$2\" \"$3\" \"$4\"\nprintf 'ENV=[%s]|[%s]|[%s]\\n' \"${EMPTY_VALUE-unset}\" \"${EXACT_VALUE-unset}\" \"${UNICODE_VALUE-unset}\"\npwd\nsleep 1\n",
    )
    .unwrap();
    {
        use std::os::unix::fs::PermissionsExt;
        let mut permissions = std::fs::metadata(&helper).unwrap().permissions();
        permissions.set_mode(0o700);
        std::fs::set_permissions(&helper, permissions).unwrap();
    }
    let helper = helper.to_str().unwrap();
    let mut app = App::new_headless(Config::default()).unwrap();
    app.tabs = Some(TabManager::new(
        80,
        24,
        ShellConfig::default(),
        100,
        1024 * 1024,
    ));

    let exact_id = app
        .launch_control_tab(
            false,
            helper,
            &[
                "".into(),
                "two words".into(),
                "$HOME;🙂".into(),
                "--json".into(),
            ],
            Some(cwd),
            Some("Exact launch 🙂"),
            &[
                crate::control::protocol::EnvironmentVariable {
                    name: "EMPTY_VALUE".into(),
                    value: String::new(),
                },
                crate::control::protocol::EnvironmentVariable {
                    name: "EXACT_VALUE".into(),
                    value: "= two words $HOME;value".into(),
                },
                crate::control::protocol::EnvironmentVariable {
                    name: "UNICODE_VALUE".into(),
                    value: "line one\nENV_UNICODE_λ".into(),
                },
            ],
            &Default::default(),
        )
        .unwrap();
    assert_eq!(exact_id, 0);
    assert_eq!(app.tabs.as_ref().unwrap().count(), 1);
    assert_eq!(app.tabs.as_ref().unwrap().active_index(), 0);
    assert_eq!(tabs_at(&app, 0).unwrap().title, "Exact launch 🙂");

    let deadline = Instant::now() + Duration::from_secs(2);
    let mut exact_output = false;
    let mut environment_output = false;
    let mut cwd_output = false;
    while Instant::now() < deadline {
        app.process_pty_events();
        exact_output = tabs_at(&app, 0)
            .and_then(|tab| tab.focused_terminal())
            .is_some_and(|terminal| {
                terminal
                    .grid()
                    .plain_lines()
                    .join("\n")
                    .contains("[]|[two words]|[$HOME;🙂]|[--json]")
            });
        cwd_output = tabs_at(&app, 0)
            .and_then(|tab| tab.focused_terminal())
            .is_some_and(|terminal| terminal.grid().plain_lines().join("\n").contains(cwd));
        environment_output = tabs_at(&app, 0)
            .and_then(|tab| tab.focused_terminal())
            .is_some_and(|terminal| {
                terminal
                    .grid()
                    .plain_lines()
                    .join("\n")
                    .contains("ENV=[]|[= two words $HOME;value]|[line one")
                    && terminal
                        .grid()
                        .plain_lines()
                        .join("\n")
                        .contains("ENV_UNICODE_λ")
            });
        if exact_output && environment_output && cwd_output {
            break;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(
        exact_output,
        "exact argv output did not reach the command tab"
    );
    assert!(
        environment_output,
        "exact environment output did not reach the command tab"
    );
    assert!(cwd_output, "the command did not run in the requested cwd");

    let count = app.tabs.as_ref().unwrap().count();
    let error = app
        .launch_control_tab(
            false,
            "/definitely/missing/termeric-control-program",
            &[],
            Some(cwd),
            None,
            &[crate::control::protocol::EnvironmentVariable {
                name: "PRIVATE_BUILD_VALUE".into(),
                value: "must-not-appear-in-errors".into(),
            }],
            &Default::default(),
        )
        .unwrap_err();
    assert!(!error.contains("PRIVATE_BUILD_VALUE"));
    assert!(!error.contains("must-not-appear-in-errors"));
    assert_eq!(app.tabs.as_ref().unwrap().count(), count);
    assert!(
        app.launch_control_tab(
            false,
            helper,
            &[],
            Some("/definitely/missing/termeric-cwd"),
            None,
            &[],
            &Default::default(),
        )
        .is_err()
    );
    assert_eq!(app.tabs.as_ref().unwrap().count(), count);
}

#[test]
fn terminal_profile_cli_launch_applies_env_cwd_title_and_workspace_identity() {
    let temp = tempfile::tempdir().unwrap();
    let mut config = Config::default();
    config.profiles.insert(
        "dev".into(),
        crate::config::LaunchProfileConfig {
            description: Some("Hermetic development fixture".into()),
            serial: None,
            program: "/bin/sh".into(),
            args: vec![
                "-c".into(),
                "printf 'PROFILE_MARK=%s PWD=%s\\n' \"$PROFILE_MARK\" \"$PWD\"; sleep 2".into(),
            ],
            cwd: Some(temp.path().to_path_buf()),
            title: Some("Development".into()),
            env: std::collections::BTreeMap::from([("PROFILE_MARK".into(), "exact".into())]),
        },
    );
    config.validate().unwrap();
    let mut app = App::new_headless(config.clone()).unwrap();
    app.allow_auto_restore = false;
    app.launch.tabs[0].target = crate::cli::LaunchTarget::TerminalProfile { name: "dev".into() };
    app.init_tabs().unwrap();

    let tab = app.tabs.as_ref().unwrap().active_tab().unwrap();
    assert_eq!(tab.title, "Development");
    assert!(matches!(
        &tab.tab_type,
        crate::tabs::TabType::Profile { name } if name == "dev"
    ));
    assert_eq!(
        tab.panes.split_shell(),
        &config.profiles["dev"].shell_config()
    );

    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        app.process_pty_events();
        let text = app
            .active_terminal()
            .unwrap()
            .grid()
            .plain_lines()
            .join("\n");
        if text.contains("PROFILE_MARK=exact")
            && text.contains(&format!("PWD={}", temp.path().display()))
        {
            break;
        }
        assert!(Instant::now() < deadline, "profile output was {text:?}");
        std::thread::sleep(Duration::from_millis(5));
    }

    let workspace = app
        .tabs
        .as_ref()
        .unwrap()
        .capture_workspace(
            "profile workspace".into(),
            false,
            crate::workspace::WorkspaceConfigReference::default(),
        )
        .unwrap();
    assert_eq!(
        workspace.tabs[0].launch().unwrap(),
        &crate::workspace::WorkspaceLaunch::Profile { name: "dev".into() }
    );
    let snapshot = workspace.tabs[0].clone();
    let mut restored = TabManager::new(
        80,
        24,
        config.shell.clone(),
        config.scrollback.lines,
        config.scrollback.max_memory_bytes(),
    );
    app.restore_workspace_tab(&mut restored, &snapshot).unwrap();
    assert!(matches!(
        &restored.active_tab().unwrap().tab_type,
        crate::tabs::TabType::Profile { name } if name == "dev"
    ));

    app.config.profiles.clear();
    let mut unavailable = restored.empty_like();
    assert!(
        app.restore_workspace_tab(&mut unavailable, &snapshot)
            .is_err()
    );
}

#[test]
fn mixed_shell_and_profile_panes_restore_each_saved_launch() {
    let mut config = Config::default();
    config.shell.program = Some("/bin/sh".into());
    config.shell.args = vec!["-c".into(), "printf 'DEFAULT_PANE\\n'; sleep 2".into()];
    config.profiles.insert(
        "dev".into(),
        crate::config::LaunchProfileConfig {
            description: None,
            serial: None,
            program: "/bin/sh".into(),
            args: vec!["-c".into(), "printf 'PROFILE_PANE\\n'; sleep 2".into()],
            cwd: None,
            title: None,
            env: Default::default(),
        },
    );
    config.validate().unwrap();
    let mut app = App::new_headless(config.clone()).unwrap();
    let snapshot = crate::workspace::WorkspaceTabSnapshot {
        id: 0,
        title: "mixed".into(),
        user_title: None,
        pinned: false,
        primary_pane: 0,
        focused_pane: 7,
        layout: crate::workspace::WorkspacePaneLayout::Split {
            direction: crate::panes::SplitDirection::Vertical,
            ratio: 0.5,
            first: Box::new(crate::workspace::WorkspacePaneLayout::Pane { pane: 0 }),
            second: Box::new(crate::workspace::WorkspacePaneLayout::Pane { pane: 7 }),
        },
        panes: vec![
            crate::workspace::WorkspacePaneSnapshot {
                id: 0,
                launch: crate::workspace::WorkspaceLaunch::Shell,
                title: None,
                working_directory: None,
                last_command: None,
                scrollback_file: None,
                scrollback_truncated: false,
                scrollback_lines: Vec::new(),
            },
            crate::workspace::WorkspacePaneSnapshot {
                id: 7,
                launch: crate::workspace::WorkspaceLaunch::Profile { name: "dev".into() },
                title: None,
                working_directory: None,
                last_command: None,
                scrollback_file: None,
                scrollback_truncated: false,
                scrollback_lines: Vec::new(),
            },
        ],
    };
    snapshot.validate().unwrap();
    let mut restored = TabManager::new(
        80,
        24,
        config.shell.clone(),
        config.scrollback.lines,
        config.scrollback.max_memory_bytes(),
    );
    app.restore_workspace_tab(&mut restored, &snapshot).unwrap();

    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        restored.poll_events();
        let panes = &restored.active_tab().unwrap().panes;
        let default_text = panes
            .get_pane(crate::panes::PaneId(0))
            .unwrap()
            .terminal
            .grid()
            .plain_lines()
            .join("\n");
        let profile_text = panes
            .get_pane(crate::panes::PaneId(7))
            .unwrap()
            .terminal
            .grid()
            .plain_lines()
            .join("\n");
        if default_text.contains("DEFAULT_PANE") && profile_text.contains("PROFILE_PANE") {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "mixed pane output was default={default_text:?} profile={profile_text:?}"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn terminal_profile_palette_entries_follow_live_config_replacement() {
    let mut config = Config::default();
    config.profiles.insert(
        "dev".into(),
        crate::config::LaunchProfileConfig {
            description: Some("Development shell".into()),
            program: "/bin/sh".into(),
            ..crate::config::LaunchProfileConfig::default()
        },
    );
    let mut app = App::new_headless(config).unwrap();
    app.command_palette.open();
    for character in "dev".chars() {
        app.command_palette
            .insert_query_text(character.encode_utf8(&mut [0; 4]));
    }
    let entries = app.command_palette.build_info().entries;
    assert!(
        entries
            .iter()
            .any(|entry| entry.title.contains("New Tab: dev"))
    );

    app.tabs = Some(TabManager::new(
        80,
        24,
        app.config.shell.clone(),
        100,
        1024 * 1024,
    ));
    app.handle_palette_result(crate::ui::command_palette::PaletteResult::RunCommand(
        "profile.open.dev".into(),
    ));
    assert!(matches!(
        &app.tabs.as_ref().unwrap().active_tab().unwrap().tab_type,
        crate::tabs::TabType::Profile { name } if name == "dev"
    ));

    app.apply_config(Config::default()).unwrap();
    app.command_palette.open();
    for character in "dev".chars() {
        app.command_palette
            .insert_query_text(character.encode_utf8(&mut [0; 4]));
    }
    assert!(
        app.command_palette
            .build_info()
            .entries
            .iter()
            .all(|entry| !entry.title.contains("New Tab: dev"))
    );
}

#[test]
fn cli_named_profile_launches_the_exact_builtin_plugin_profile() {
    let temp = tempfile::tempdir().unwrap();
    let fake_cli = temp.path().join("profile-cli");
    std::fs::write(
        &fake_cli,
        "#!/bin/sh\nif [ \"${1-}\" = --version ]; then exit 0; fi\nexec sleep 2\n",
    )
    .unwrap();
    let mut permissions = std::fs::metadata(&fake_cli).unwrap().permissions();
    permissions.set_mode(0o755);
    std::fs::set_permissions(&fake_cli, permissions).unwrap();
    let mut config = Config::default();
    config.agents.insert(
        "codex".into(),
        crate::agents::AgentConfig {
            program: Some(fake_cli.display().to_string()),
            profiles: std::collections::BTreeMap::from([("review".into(), Default::default())]),
            ..Default::default()
        },
    );
    let mut app = App::new_headless(config).unwrap();
    app.allow_auto_restore = false;
    app.launch.tabs[0].target = crate::cli::LaunchTarget::Plugin {
        plugin_id: "codex".into(),
        profile: "review".into(),
    };
    app.init_tabs().unwrap();
    let tab = app.tabs.as_ref().unwrap().active_tab().unwrap();
    assert!(matches!(
        &tab.tab_type,
        crate::tabs::TabType::Plugin { plugin_id } if plugin_id == "codex"
    ));
    assert_eq!(
        tab.pane_plugin_launch(tab.focused_terminal_identity().pane_id)
            .and_then(|(_, profile)| profile),
        Some("review")
    );
}

#[test]
fn missing_automatic_multi_window_workspace_falls_back_but_explicit_cli_is_strict() {
    let temp = tempfile::tempdir().unwrap();
    crate::workspace::set_test_workspaces_base_dir(Some(temp.path().join("workspaces")));
    AppState::set_test_state_path(Some(temp.path().join("state/state.json")));
    let make_app = || {
        let mut config = Config::default();
        config.shell.program = Some("/bin/sh".into());
        config.shell.args = vec!["-c".into(), "sleep 1".into()];
        let mut app = App::new_headless(config).unwrap();
        app.launch.workspace = Some("workspace-missing".into());
        app
    };
    {
        let mut automatic = make_app();
        automatic.allow_workspace_fallback = true;
        automatic.init_tabs().unwrap();
        assert_eq!(automatic.tabs.as_ref().unwrap().count(), 1);

        let mut explicit = make_app();
        assert!(explicit.init_tabs().is_err());
        assert_eq!(explicit.tabs.as_ref().unwrap().count(), 0);
    }
    crate::workspace::set_test_workspaces_base_dir(None);
    AppState::set_test_state_path(None);
}

#[test]
fn detected_url_cache_joins_soft_wrapped_visible_rows() {
    let mut terminal = termeric_core::Terminal::new(8, 4);
    terminal.process(b"xx https://example.test/x yy");
    let mut cache = UrlCache::new(0);
    let detector = UrlDetector::new();
    update_visible_url_cache(&mut cache, terminal.grid(), &detector, &[]);
    for (row, col) in [(0, 3), (0, 7), (1, 0), (1, 7), (2, 0), (2, 7), (3, 0)] {
        assert_eq!(
            cache.url_at(row, col).map(|url| url.url.as_str()),
            Some("https://example.test/x"),
            "row={row} col={col}"
        );
    }
    assert!(cache.url_at(0, 2).is_none());
    assert!(cache.url_at(3, 1).is_none());
}
