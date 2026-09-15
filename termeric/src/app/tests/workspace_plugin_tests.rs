//! Fake executables and raw local PTYs only; never installed provider binaries.
use super::*;
use crate::workspace::{
    WorkspaceLaunch, WorkspacePaneLayout, WorkspacePaneSnapshot, WorkspaceSplitDirection,
    WorkspaceTabSnapshot,
};

pub(super) fn fixture() -> (App, tempfile::TempDir, WorkspaceTabSnapshot) {
    let dir = tempfile::tempdir().unwrap();
    let program = dir.path().join("fake-workspace-plugin");
    std::fs::write(&program, "#!/bin/sh\nstty raw -echo\nprintf '%s\\n' \"$PWD\" \"$TEST_TAG\" \"$*\" > \"$CAPTURE_DIR/$TEST_TAG\"\nexec cat >/dev/null\n").unwrap();
    let mut permissions = std::fs::metadata(&program).unwrap().permissions();
    permissions.set_mode(0o755);
    std::fs::set_permissions(&program, permissions).unwrap();
    let mut config = Config {
        shell: ShellConfig {
            program: Some("/bin/sh".into()),
            args: vec!["-c".into(), "exec cat >/dev/null".into()],
            env: Default::default(),
        },
        ..Default::default()
    };
    config.profiles.insert(
        "dev".into(),
        crate::config::LaunchProfileConfig {
            program: "/bin/sh".into(),
            args: vec!["-c".into(), "exec cat >/dev/null".into()],
            ..Default::default()
        },
    );
    config.plugins.codex.enabled = true;
    config.plugins.codex.binary = program.display().to_string();
    config.plugins.claude_code.enabled = true;
    config.plugins.claude_code.binary = program.display().to_string();
    for name in ["red", "blue", "default"] {
        config.plugins.codex.profiles.insert(
            name.into(),
            crate::plugin::codex_profiles::CodexProfile {
                extra_args: vec![format!("--fixture={name}")],
                env: HashMap::from([
                    ("TEST_TAG".into(), name.into()),
                    ("CAPTURE_DIR".into(), dir.path().display().to_string()),
                    ("PWD".into(), dir.path().display().to_string()),
                ]),
                ..Default::default()
            },
        );
    }
    config.plugins.claude_code.profiles.insert(
        "green".into(),
        crate::plugin::claude_profiles::ClaudeProfile {
            extra_args: vec!["--fixture=green".into()],
            env: HashMap::from([
                ("TEST_TAG".into(), "green".into()),
                ("CAPTURE_DIR".into(), dir.path().display().to_string()),
            ]),
            ..Default::default()
        },
    );
    let identities = [
        (0, WorkspaceLaunch::Shell),
        (
            7,
            WorkspaceLaunch::Plugin {
                plugin_id: "codex".into(),
                profile: Some("red".into()),
            },
        ),
        (
            9,
            WorkspaceLaunch::Plugin {
                plugin_id: "codex".into(),
                profile: Some("blue".into()),
            },
        ),
        (
            11,
            WorkspaceLaunch::Plugin {
                plugin_id: "claude-code".into(),
                profile: Some("green".into()),
            },
        ),
        (13, WorkspaceLaunch::Profile { name: "dev".into() }),
    ];
    let mut panes = Vec::new();
    for (id, launch) in identities {
        let cwd = dir.path().join(format!("cwd-{id}"));
        std::fs::create_dir(&cwd).unwrap();
        panes.push(WorkspacePaneSnapshot {
            id,
            launch: Some(launch),
            title: Some(format!("pane-{id}")),
            working_directory: Some(cwd.display().to_string()),
            last_command: Some("saved command".into()),
            scrollback_file: None,
            scrollback_truncated: false,
            scrollback_lines: vec![format!("saved-{id}")],
        });
    }
    let layout =
        [7, 9, 11, 13]
            .into_iter()
            .fold(WorkspacePaneLayout::Pane { pane: 0 }, |first, pane| {
                WorkspacePaneLayout::Split {
                    direction: WorkspaceSplitDirection::Vertical,
                    ratio: 0.6,
                    first: Box::new(first),
                    second: Box::new(WorkspacePaneLayout::Pane { pane }),
                }
            });
    let snapshot = WorkspaceTabSnapshot {
        id: 0,
        title: "mixed".into(),
        user_title: Some("saved title".into()),
        pinned: false,
        launch: WorkspaceLaunch::Shell,
        primary_pane: 0,
        focused_pane: 9,
        layout,
        panes,
    };
    (App::new_headless(config).unwrap(), dir, snapshot)
}

pub(super) fn manager(app: &App) -> TabManager {
    TabManager::new_with_geometry(
        crate::terminal::geometry::TerminalGeometry::from_cells(120, 40, 10.0, 20.0),
        app.config.shell.clone(),
        1_000,
        8 * 1024 * 1024,
    )
}

#[test]
fn mixed_plugin_workspaces_restore_exact_profiles_cwds_and_terminal_owners() {
    for primary in [0, 7] {
        let (mut app, dir, mut snapshot) = fixture();
        snapshot.primary_pane = primary;
        snapshot.launch = snapshot
            .panes
            .iter()
            .find(|pane| pane.id == primary)
            .unwrap()
            .launch
            .clone()
            .unwrap();
        let mut tabs = manager(&app);
        let (_, warnings) = app.restore_workspace_tab(&mut tabs, &snapshot).unwrap();
        assert!(warnings.is_empty());
        for (id, tag) in [(7, "red"), (9, "blue"), (11, "green")] {
            let path = dir.path().join(tag);
            wait_for_file_text(&path, &format!("--fixture={tag}"));
            let recorded = std::fs::read_to_string(path).unwrap();
            assert!(
                recorded.starts_with(&dir.path().join(format!("cwd-{id}")).display().to_string())
            );
            let tab = tabs.active_tab().unwrap();
            let state = tab.pane_session(PaneId(id)).unwrap();
            assert_eq!(state.ai_session.as_ref().unwrap().profile, tag);
            assert!(state.transcript.is_none());
            assert_eq!(state.diff_detector.unreviewed_count(), 0);
            // Restored plugin owners are valid ordinary transfer sources.
            assert!(
                tabs.validate_pane_transfer_source_by_id(tab.id, PaneId(id))
                    .is_ok()
            );
        }
        let tab = tabs.active_tab().unwrap();
        assert!(tab.pane_session(PaneId(0)).unwrap().ai_session.is_none());
        assert!(tab.pane_session(PaneId(13)).unwrap().ai_session.is_none());
        assert_eq!(tab.pane_manager.as_ref().unwrap().focused_id(), PaneId(9));
        let mut captured = tabs
            .capture_workspace("roundtrip".into(), true, Default::default())
            .unwrap();
        assert_eq!(captured.tabs[0].primary_pane, primary);
        assert_eq!(captured.tabs[0].layout, snapshot.layout);
        for expected in &snapshot.panes {
            let pane = captured.tabs[0]
                .panes
                .iter()
                .find(|pane| pane.id == expected.id)
                .unwrap();
            assert_eq!(pane.launch, expected.launch);
            assert_eq!(pane.working_directory, expected.working_directory);
            assert!(
                pane.scrollback_lines
                    .iter()
                    .any(|line| line.contains(&format!("saved-{}", pane.id))),
                "pane {} history: {:?}",
                pane.id,
                pane.scrollback_lines
            );
        }
        crate::workspace::save_workspace_to_dir(&mut captured, &dir.path().join("archive"))
            .unwrap();
        let loaded = crate::workspace::load_workspace_from_dir(&dir.path().join("archive"))
            .unwrap()
            .workspace;
        assert_eq!(loaded.version, 3);
        let mut restored = manager(&app);
        app.restore_workspace_tab(&mut restored, &loaded.tabs[0])
            .unwrap();
        for expected in &snapshot.panes {
            let identity = restored
                .active_tab()
                .unwrap()
                .capture_workspace_tab(0, false)
                .unwrap()
                .panes
                .into_iter()
                .find(|pane| pane.id == expected.id)
                .unwrap()
                .launch;
            assert_eq!(identity, expected.launch);
        }
    }
}

#[test]
fn missing_nonprimary_plugin_cwd_uses_process_default_not_focused_sibling() {
    let (mut app, dir, mut snapshot) = fixture();
    snapshot
        .panes
        .iter_mut()
        .find(|pane| pane.id == 7)
        .unwrap()
        .working_directory = Some(dir.path().join("missing-cwd").display().to_string());
    let mut tabs = manager(&app);
    let (_, warnings) = app.restore_workspace_tab(&mut tabs, &snapshot).unwrap();
    assert!(warnings.iter().any(|warning| warning.contains("pane 7")));
    wait_for_file_text(&dir.path().join("red"), "--fixture=red");
    let recorded = std::fs::read_to_string(dir.path().join("red")).unwrap();
    assert_eq!(
        recorded.lines().next().unwrap(),
        std::env::var("HOME").unwrap()
    );
    wait_for_file_text(&dir.path().join("blue"), "--fixture=blue");
    let sibling = std::fs::read_to_string(dir.path().join("blue")).unwrap();
    assert_eq!(
        sibling.lines().next().unwrap(),
        dir.path().join("cwd-9").display().to_string()
    );
}

#[test]
fn literal_default_profile_is_not_lost() {
    let (mut app, dir, mut snapshot) = fixture();
    snapshot.primary_pane = 7;
    let named = WorkspaceLaunch::Plugin {
        plugin_id: "codex".into(),
        profile: Some("default".into()),
    };
    snapshot.launch = named.clone();
    snapshot
        .panes
        .iter_mut()
        .find(|pane| pane.id == 7)
        .unwrap()
        .launch = Some(named.clone());
    let mut tabs = manager(&app);
    app.restore_workspace_tab(&mut tabs, &snapshot).unwrap();
    wait_for_file_text(&dir.path().join("default"), "--fixture=default");
    let captured = tabs
        .active_tab()
        .unwrap()
        .capture_workspace_tab(0, false)
        .unwrap();
    assert_eq!(captured.launch, named);
    assert_eq!(
        captured
            .panes
            .iter()
            .find(|pane| pane.id == 7)
            .unwrap()
            .launch,
        Some(named)
    );
}

#[test]
fn missing_profile_and_spawn_failure_keep_existing_tabs_and_focus() {
    for failure in ["profile", "binary", "disabled", "process-profile"] {
        let (mut app, dir, mut snapshot) = fixture();
        match failure {
            "profile" => {
                snapshot
                    .panes
                    .iter_mut()
                    .find(|pane| pane.id == 11)
                    .unwrap()
                    .launch = Some(WorkspaceLaunch::Plugin {
                    plugin_id: "claude-code".into(),
                    profile: Some("removed".into()),
                });
            }
            "binary" => {
                app.config.plugins.claude_code.binary =
                    dir.path().join("missing-program").display().to_string();
                app = App::new_headless(app.config.clone()).unwrap();
            }
            "disabled" => app.config.plugins.claude_code.enabled = false,
            "process-profile" => {
                // A missing process-profile reference is never replaced with a shell.
                snapshot
                    .panes
                    .iter_mut()
                    .find(|pane| pane.id == 13)
                    .unwrap()
                    .launch = Some(WorkspaceLaunch::Profile {
                    name: "removed-process".into(),
                });
            }
            _ => unreachable!(),
        }
        let mut tabs = manager(&app);
        tabs.new_tab().unwrap();
        let original = tabs.active_tab().unwrap().child_observer().unwrap();
        let original_id = tabs.active_tab().unwrap().id;
        let error = app.restore_workspace_tab(&mut tabs, &snapshot).unwrap_err();
        assert!(!error.to_string().is_empty());
        assert_eq!(
            tabs.count(),
            1,
            "{failure}: failed restore retained a partial tab"
        );
        assert_eq!(tabs.active_tab().unwrap().id, original_id);
        assert!(!original.poll().unwrap().terminal_closed);
        let mut valid = snapshot.clone();
        valid.primary_pane = 0;
        valid.focused_pane = 0;
        valid.launch = WorkspaceLaunch::Shell;
        valid.panes.retain(|pane| pane.id == 0);
        valid.layout = WorkspacePaneLayout::Pane { pane: 0 };
        let (index, _) = app.restore_workspace_tab(&mut tabs, &valid).unwrap();
        assert_eq!(
            index, 1,
            "failed restore must not leave a hole in later source-tab mapping"
        );
        assert_eq!(tabs.count(), 2);
    }
}

#[test]
fn rollback_drops_only_new_workspace_owners_without_animation() {
    let (app, _dir, _) = fixture();
    let mut tabs = manager(&app);
    tabs.new_tab().unwrap();
    let original = tabs.active_tab().unwrap().child_observer().unwrap();
    tabs.new_tab().unwrap();
    let partial = tabs.active_tab().unwrap().child_observer().unwrap();
    tabs.rollback_workspace_tab(1, 0);
    assert_eq!(tabs.count(), 1);
    assert!(partial.poll().unwrap().terminal_closed);
    assert!(!original.poll().unwrap().terminal_closed);
}

#[test]
fn resolved_default_profile_is_saved_as_the_actual_owner_reference() {
    let (mut app, _dir, mut snapshot) = fixture();
    app.config.plugins.codex.default_profile = Some("red".into());
    app = App::new_headless(app.config.clone()).unwrap();
    snapshot.primary_pane = 7;
    snapshot.launch = WorkspaceLaunch::Plugin {
        plugin_id: "codex".into(),
        profile: None,
    };
    snapshot
        .panes
        .iter_mut()
        .find(|pane| pane.id == 7)
        .unwrap()
        .launch = Some(snapshot.launch.clone());
    let mut tabs = manager(&app);
    app.restore_workspace_tab(&mut tabs, &snapshot).unwrap();
    let captured = tabs
        .active_tab()
        .unwrap()
        .capture_workspace_tab(0, false)
        .unwrap();
    assert_eq!(
        captured.launch,
        WorkspaceLaunch::Plugin {
            plugin_id: "codex".into(),
            profile: Some("red".into())
        }
    );
    assert_eq!(
        tabs.active_tab()
            .unwrap()
            .pane_session(PaneId(7))
            .unwrap()
            .ai_session
            .as_ref()
            .unwrap()
            .profile,
        "red"
    );
}

#[derive(Debug)]
struct RestoreResponseFixture {
    id: &'static str,
    mode: &'static str,
    calls: std::sync::Arc<std::sync::atomic::AtomicUsize>,
}

impl Plugin for RestoreResponseFixture {
    fn id(&self) -> &str {
        self.id
    }
    fn name(&self) -> &str {
        "Synthetic workspace restore"
    }
    fn on_hook(&mut self, hook: &crate::plugin::Hook, ctx: &PluginContext) -> HookResponse {
        use crate::plugin::{Hook, PluginAction};
        let (Hook::WorkspaceRestore { profile, cwd, .. } | Hook::PaneLaunch { profile, cwd, .. }) =
            hook
        else {
            return HookResponse::Ignored;
        };
        self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        assert_eq!(&ctx.cwd, cwd);
        let launch = PluginAction::OpenTab {
            program: "/bin/sh".into(),
            args: vec!["-c".into(), "exec cat >/dev/null".into()],
            cwd: if self.mode == "cwd" {
                None
            } else {
                cwd.clone()
            },
            title: None,
            plugin_id: Some(
                if self.mode == "owner" {
                    "wrong-owner"
                } else {
                    self.id
                }
                .into(),
            ),
            profile: if self.mode == "profile" {
                Some("different".into())
            } else {
                profile.clone()
            },
            env: Default::default(),
        };
        match self.mode {
            "duplicate" => HookResponse::Actions(vec![launch.clone(), launch]),
            "side-effect" => HookResponse::Actions(vec![
                launch,
                PluginAction::WriteToPty {
                    data: b"must not be sent".to_vec(),
                },
            ]),
            "notice" => HookResponse::Actions(vec![
                PluginAction::ShowNotice {
                    title: "Fallback".into(),
                    lines: vec!["Do not accept silently".into()],
                },
                launch,
            ]),
            _ => HookResponse::Action(launch),
        }
    }
}

#[test]
fn workspace_plugin_hooks_are_targeted_and_mismatched_or_ambiguous_actions_are_rejected() {
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };
    for mode in [
        "owner",
        "profile",
        "cwd",
        "duplicate",
        "side-effect",
        "notice",
    ] {
        let (mut app, _dir, mut snapshot) = fixture();
        let called = Arc::new(AtomicUsize::new(0));
        let unrelated = Arc::new(AtomicUsize::new(0));
        for (id, calls) in [
            ("synthetic-workspace", called.clone()),
            ("unrelated", unrelated.clone()),
        ] {
            app.plugin_manager
                .register(
                    Box::new(RestoreResponseFixture { id, mode, calls }),
                    &app.build_plugin_context(),
                )
                .unwrap();
        }
        snapshot.launch = WorkspaceLaunch::Plugin {
            plugin_id: "synthetic-workspace".into(),
            profile: Some("saved".into()),
        };
        snapshot.primary_pane = 7;
        snapshot.focused_pane = 7;
        snapshot.panes.retain(|pane| pane.id == 7);
        snapshot.panes[0].launch = Some(snapshot.launch.clone());
        snapshot.layout = WorkspacePaneLayout::Pane { pane: 7 };
        let mut tabs = manager(&app);
        tabs.new_tab().unwrap();
        assert!(
            app.restore_workspace_tab(&mut tabs, &snapshot).is_err(),
            "{mode}"
        );
        assert_eq!(tabs.count(), 1);
        assert_eq!(called.load(Ordering::SeqCst), 1);
        assert_eq!(unrelated.load(Ordering::SeqCst), 0);
        // The same rejection contract applies to fresh explicit pane launches.
        let ctx = PluginContext {
            cwd: snapshot.panes[0].working_directory.clone(),
            tab_count: 1,
            active_tab: 0,
            terminal_size: (40, 20),
        };
        assert!(
            app.resolve_plugin_terminal_launch(
                &crate::plugin::Hook::PaneLaunch {
                    plugin_id: "synthetic-workspace".into(),
                    profile: Some("saved".into()),
                    cwd: ctx.cwd.clone(),
                },
                &ctx
            )
            .is_err(),
            "{mode}"
        );
        assert_eq!(called.load(Ordering::SeqCst), 2);
        assert_eq!(unrelated.load(Ordering::SeqCst), 0);
    }
}
