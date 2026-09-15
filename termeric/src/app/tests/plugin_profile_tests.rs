//! Profile metadata tests use raw local PTYs, never provider executables.
use super::*;
use crate::plugin::PluginAction;
use crate::tabs::TabType;

#[test]
fn automatic_and_literal_default_profile_choices_are_distinct_and_revalidated() {
    use crate::ui::info_overlay::InfoOverlayAction;
    for plugin_id in ["codex", "claude-code"] {
        let (mut app, _directory, _, _) = harvest_rerun_capture_fixture_in(false);
        mark_capture_plugin_owner(&mut app, PaneId(0), plugin_id);
        for name in ["default", "automatic-profile", "profile:default"] {
            match plugin_id {
                "codex" => {
                    app.config
                        .plugins
                        .codex
                        .profiles
                        .insert(name.into(), Default::default());
                }
                _ => {
                    app.config
                        .plugins
                        .claude_code
                        .profiles
                        .insert(name.into(), Default::default());
                }
            }
        }
        for (id, expected) in [
            ("profile-choice:2", Some("default")),
            ("profile-choice:0", None),
            ("profile-choice:1", Some("automatic-profile")),
            ("profile-choice:3", Some("profile:default")),
        ] {
            app.show_plugin_profiles(plugin_id.into());
            assert!(
                app.info_overlay
                    .choices()
                    .iter()
                    .any(|choice| choice.label == "Automatic profile")
            );
            assert!(
                app.info_overlay
                    .choices()
                    .iter()
                    .any(|choice| choice.label == "Profile: default")
            );
            let request_id = app.info_overlay.request_id();
            app.apply_info_overlay_action(InfoOverlayAction::Choose {
                request_id,
                choice_id: id.into(),
            });
            let tab = app.tabs.as_ref().unwrap().active_tab().unwrap();
            assert_eq!(
                tab.pane_plugin_launch(tab.single_pane_id()),
                Some((plugin_id, expected))
            );
        }
        // Unlike the old sentinel, a removed literal default must be refused.
        app.show_plugin_profiles(plugin_id.into());
        let request_id = app.info_overlay.request_id();
        match plugin_id {
            "codex" => app.config.plugins.codex.profiles.clear(),
            _ => app.config.plugins.claude_code.profiles.clear(),
        }
        app.apply_info_overlay_action(InfoOverlayAction::Choose {
            request_id,
            choice_id: "profile-choice:2".into(),
        });
        assert!(latest_toast(&app).unwrap().contains("no longer configured"));
        let tab = app.tabs.as_ref().unwrap().active_tab().unwrap();
        assert_eq!(
            tab.pane_plugin_launch(tab.single_pane_id()),
            Some((plugin_id, Some("profile:default")))
        );
        // Clearing the reference must remain possible when no profiles exist.
        app.show_plugin_profiles(plugin_id.into());
        let request_id = app.info_overlay.request_id();
        assert_eq!(app.info_overlay.choices().len(), 1);
        app.apply_info_overlay_action(InfoOverlayAction::Choose {
            request_id,
            choice_id: "profile-choice:0".into(),
        });
        let tab = app.tabs.as_ref().unwrap().active_tab().unwrap();
        assert_eq!(
            tab.pane_plugin_launch(tab.single_pane_id()),
            Some((plugin_id, None))
        );
        app.execute_plugin_actions(vec![PluginAction::ApplyProfile {
            plugin_id: plugin_id.into(),
            profile_name: "default".into(),
        }]);
        assert!(latest_toast(&app).unwrap().contains("no longer configured"));
    }
}

#[test]
fn profile_beyond_number_shortcuts_can_be_selected_with_keyboard_focus() {
    let (mut app, _directory, _, _) = harvest_rerun_capture_fixture_in(false);
    mark_capture_plugin_owner(&mut app, PaneId(0), "codex");
    for i in 0..12 {
        app.config
            .plugins
            .codex
            .profiles
            .insert(format!("profile-{i:02}"), Default::default());
    }
    app.show_plugin_profiles("codex".into());
    let request = app.info_overlay.request_id();
    for _ in 0..13 {
        app.info_overlay.cycle_focus(request, false);
    }
    let action = app.info_overlay.activate_focused(request);
    app.apply_info_overlay_action(action);
    let tab = app.tabs.as_ref().unwrap().active_tab().unwrap();
    assert_eq!(
        tab.pane_plugin_launch(tab.single_pane_id()),
        Some(("codex", Some("profile-11")))
    );
}

#[test]
fn profile_picker_bounds_choices_and_keeps_long_names_out_of_choice_ids() {
    let (mut app, _directory, _, _) = harvest_rerun_capture_fixture_in(false);
    mark_capture_plugin_owner(&mut app, PaneId(0), "codex");
    let long = "a".repeat(512);
    app.config
        .plugins
        .codex
        .profiles
        .insert(long.clone(), Default::default());
    for i in 0..40 {
        app.config
            .plugins
            .codex
            .profiles
            .insert(format!("profile-{i:02}"), Default::default());
    }
    app.show_plugin_profiles("codex".into());
    assert_eq!(
        app.info_overlay.choices().len(),
        crate::ui::info_overlay::MAX_INFO_CHOICES
    );
    assert_eq!(
        app.pending_profile_selection
            .as_ref()
            .unwrap()
            .profiles
            .len(),
        crate::ui::info_overlay::MAX_INFO_CHOICES
    );
    assert!(
        app.info_overlay
            .lines()
            .iter()
            .any(|line| line.contains("Only the first 31"))
    );
    let request_id = app.info_overlay.request_id();
    app.apply_info_overlay_action(crate::ui::info_overlay::InfoOverlayAction::Choose {
        request_id,
        choice_id: "profile-choice:1".into(),
    });
    let tab = app.tabs.as_ref().unwrap().active_tab().unwrap();
    assert_eq!(
        tab.pane_plugin_launch(tab.single_pane_id()),
        Some(("codex", Some(long.as_str())))
    );
}

#[test]
fn open_tab_and_direct_profile_actions_preserve_literal_default() {
    for plugin_id in ["codex", "claude-code"] {
        let (mut app, dir, _) = super::workspace_plugin_tests::fixture();
        app.config
            .plugins
            .claude_code
            .profiles
            .insert("default".into(), Default::default());
        app.tabs = Some(super::workspace_plugin_tests::manager(&app));
        for profile in [Some("default"), None] {
            app.execute_plugin_actions(vec![PluginAction::OpenTab {
                program: "/bin/sh".into(),
                args: vec!["-c".into(), "exec cat >/dev/null".into()],
                cwd: Some(dir.path().display().to_string()),
                title: None,
                plugin_id: Some(plugin_id.into()),
                profile: profile.map(str::to_owned),
                env: Default::default(),
            }]);
            let tab = app.tabs.as_ref().unwrap().active_tab().unwrap();
            assert_eq!(
                tab.pane_plugin_launch(tab.single_pane_id()),
                Some((plugin_id, profile))
            );
            app.execute_plugin_actions(vec![PluginAction::ApplyProfile {
                plugin_id: plugin_id.into(),
                profile_name: "default".into(),
            }]);
            let tab = app.tabs.as_ref().unwrap().active_tab().unwrap();
            assert_eq!(
                tab.pane_plugin_launch(tab.single_pane_id()),
                Some((plugin_id, Some("default")))
            );
        }
    }
}

#[test]
fn profile_application_changes_only_the_exact_plugin_owner() {
    for plugin_presentation in [true, false] {
        let (mut app, _directory, entry, _) = harvest_rerun_capture_fixture();
        app.config
            .plugins
            .codex
            .profiles
            .insert("chosen".into(), Default::default());
        mark_capture_plugin_owner(&mut app, PaneId(0), "codex");
        mark_capture_plugin_owner(&mut app, PaneId(1), "codex");
        mark_capture_plugin_owner(&mut app, PaneId(2), "claude-code");
        if plugin_presentation {
            let tab = app.tabs.as_mut().unwrap().active_tab_mut().unwrap();
            tab.tab_type = TabType::Plugin {
                plugin_id: "codex".into(),
            };
            tab.pane_session_mut(PaneId(0)).unwrap().ai_session = Some(Default::default());
        }
        app.execute_plugin_actions(vec![PluginAction::ApplyProfile {
            plugin_id: "codex".into(),
            profile_name: "chosen".into(),
        }]);
        let tab = app
            .tabs
            .as_ref()
            .unwrap()
            .tab_by_id(entry.source.tab_id)
            .unwrap();
        assert_eq!(
            tab.pane_plugin_launch(PaneId(0)),
            Some(("codex", Some("chosen")))
        );
        assert_eq!(tab.pane_plugin_launch(PaneId(1)), Some(("codex", None)));
        assert_eq!(
            tab.pane_plugin_launch(PaneId(2)),
            Some(("claude-code", None))
        );
        assert!(app.info_overlay.is_active());
        // A focused matching sibling takes precedence for a new command;
        // the original owner's profile must remain unchanged.
        app.focus_control_pane(entry.source.tab_id as u64, 1)
            .unwrap();
        app.execute_plugin_actions(vec![PluginAction::ApplyProfile {
            plugin_id: "codex".into(),
            profile_name: "chosen".into(),
        }]);
        let tab = app.tabs.as_mut().unwrap().active_tab_mut().unwrap();
        assert!(tab.set_plugin_profile_reference(PaneId(0), "codex", None));
        assert_eq!(tab.pane_plugin_launch(PaneId(0)), Some(("codex", None)));
        assert_eq!(
            tab.pane_plugin_launch(PaneId(1)),
            Some(("codex", Some("chosen")))
        );
        assert_eq!(
            tab.pane_plugin_launch(PaneId(2)),
            Some(("claude-code", None))
        );
        // Changing the presentation's primary owner must read that owner's
        // profile rather than carrying the previous primary's value forward.
        if plugin_presentation {
            app.close_control_pane(entry.source.tab_id as u64, 0)
                .unwrap();
            assert_eq!(
                app.tabs
                    .as_ref()
                    .unwrap()
                    .active_tab()
                    .unwrap()
                    .focused_session()
                    .unwrap()
                    .ai_session
                    .as_ref()
                    .unwrap()
                    .profile,
                "chosen"
            );
            app.close_control_pane(entry.source.tab_id as u64, 2)
                .unwrap();
            assert_eq!(
                app.tabs
                    .as_ref()
                    .unwrap()
                    .active_tab()
                    .unwrap()
                    .focused_session()
                    .unwrap()
                    .ai_session
                    .as_ref()
                    .unwrap()
                    .profile,
                "chosen"
            );
        }
    }
}

#[test]
fn profile_picker_never_falls_back_to_a_replacement_tab() {
    use crate::ui::info_overlay::InfoOverlayAction;
    let (mut app, _directory, source, _) = harvest_rerun_capture_fixture_in(false);
    app.config
        .plugins
        .codex
        .profiles
        .insert("chosen".into(), Default::default());
    mark_capture_plugin_owner(&mut app, PaneId(0), "codex");
    app.execute_plugin_actions(vec![PluginAction::ListProfiles {
        plugin_id: "codex".into(),
    }]);
    let request_id = app.info_overlay.request_id();
    let tabs = app.tabs.as_mut().unwrap();
    let replacement = tabs
        .new_tab_with_command(
            "/bin/sh",
            &["-c".into(), "read fixture".into()],
            None,
            None,
            Some(TabType::Plugin {
                plugin_id: "codex".into(),
            }),
            HashMap::new(),
        )
        .unwrap();
    assert!(tabs.close_tab_by_id(source.source.tab_id));
    app.apply_info_overlay_action(InfoOverlayAction::Choose {
        request_id,
        choice_id: "profile-choice:1".into(),
    });
    let tab = app.tabs.as_ref().unwrap().tab_by_id(replacement).unwrap();
    assert_eq!(
        tab.pane_plugin_launch(tab.single_pane_id()),
        Some(("codex", None))
    );
    assert!(
        latest_toast(&app)
            .unwrap()
            .contains("Original plugin terminal")
    );
    // Direct application of a missing profile must also refuse mutation.
    app.execute_plugin_actions(vec![PluginAction::ApplyProfile {
        plugin_id: "codex".into(),
        profile_name: "not-configured".into(),
    }]);
    assert!(latest_toast(&app).unwrap().contains("no longer configured"));
    assert_eq!(
        app.tabs
            .as_ref()
            .unwrap()
            .tab_by_id(replacement)
            .unwrap()
            .focused_session()
            .unwrap()
            .ai_session
            .as_ref()
            .unwrap()
            .profile,
        "default"
    );
}

#[test]
fn profile_picker_keeps_its_owner_across_focus_and_topology_changes() {
    use crate::ui::info_overlay::InfoOverlayAction;
    for transition in [
        "split",
        "collapse",
        "nonzero-collapse",
        "focus",
        "removed",
        "changed-plugin",
        "read-only",
        "pane-closed",
        "closed",
        "closing",
        "live-domain",
        "removed-profile",
        "replaced-overlay",
        "cancel",
    ] {
        let (mut app, directory, _, _) =
            harvest_rerun_capture_fixture_in(transition == "nonzero-collapse");
        app.config
            .plugins
            .codex
            .profiles
            .insert("chosen".into(), Default::default());
        let source = app.active_harvest_source().unwrap();
        mark_capture_plugin_owner(&mut app, PaneId(source.pane_id), "codex");
        app.execute_plugin_actions(vec![PluginAction::ListProfiles {
            plugin_id: "codex".into(),
        }]);
        let request_id = app.info_overlay.request_id();
        assert!(
            app.info_overlay
                .lines()
                .iter()
                .any(|line| line == "Next restart: Automatic profile")
        );
        assert_eq!(
            app.pending_profile_selection
                .as_ref()
                .unwrap()
                .terminal
                .pane_id,
            PaneId(source.pane_id)
        );
        if transition == "nonzero-collapse" {
            app.close_control_pane(source.tab_id as u64, 0).unwrap();
            app.close_control_pane(source.tab_id as u64, 1).unwrap();
        } else {
            let sibling = split_harvest_capture_owner(&mut app, directory.path());
            mark_capture_plugin_owner(&mut app, sibling, "codex");
            match transition {
                "collapse" => {
                    app.close_control_pane(source.tab_id as u64, sibling.0)
                        .unwrap();
                }
                "removed" => {
                    app.close_control_pane(source.tab_id as u64, source.pane_id)
                        .unwrap();
                }
                "focus" => {
                    app.focus_control_pane(source.tab_id as u64, sibling.0)
                        .unwrap();
                }
                "changed-plugin" => {
                    mark_capture_plugin_owner(&mut app, PaneId(source.pane_id), "claude-code")
                }
                "read-only" => {
                    app.focus_control_pane(source.tab_id as u64, source.pane_id)
                        .unwrap();
                    app.toggle_focused_pane_read_only();
                }
                "pane-closed" => {
                    app.tabs
                        .as_mut()
                        .unwrap()
                        .active_tab_mut()
                        .unwrap()
                        .pane_manager
                        .as_mut()
                        .unwrap()
                        .get_pane_mut(PaneId(source.pane_id))
                        .unwrap()
                        .closed = true
                }
                "closed" => app.tabs.as_mut().unwrap().active_tab_mut().unwrap().closed = true,
                "closing" => {
                    app.tabs
                        .as_mut()
                        .unwrap()
                        .active_tab_mut()
                        .unwrap()
                        .closing_at = Some(Instant::now())
                }
                "live-domain" => {
                    app.tabs
                        .as_mut()
                        .unwrap()
                        .active_tab_mut()
                        .unwrap()
                        .live_domain = Some(crate::tabs::LiveDomainTab {
                        domain_id: "not-connected".into(),
                        tab_id: 1,
                        pane_ids: HashMap::new(),
                    })
                }
                "removed-profile" => {
                    app.config.plugins.codex.profiles.clear();
                }
                "replaced-overlay" => {
                    app.info_overlay.show("Replacement", vec![]);
                }
                "cancel" => {
                    app.apply_info_overlay_action(InfoOverlayAction::Dismiss { request_id });
                }
                _ => {}
            }
        }
        let focused = app.active_harvest_source();
        let choose = || InfoOverlayAction::Choose {
            request_id,
            choice_id: "profile-choice:1".into(),
        };
        app.apply_info_overlay_action(choose());
        app.apply_info_overlay_action(choose()); // An old choice must not replay.
        let allowed = matches!(
            transition,
            "split" | "collapse" | "nonzero-collapse" | "focus"
        );
        let tab = app.tabs.as_ref().unwrap().tab_by_id(source.tab_id).unwrap();
        if let Some(panes) = &tab.pane_manager {
            for id in panes.pane_ids() {
                let launch = &panes.get_pane(id).unwrap().launch;
                if let crate::workspace::WorkspaceLaunch::Plugin { profile, .. } = launch {
                    assert_eq!(
                        profile.as_deref(),
                        (allowed && id.0 == source.pane_id).then_some("chosen"),
                        "{transition}, {id:?}"
                    );
                }
            }
        } else if allowed {
            assert_eq!(
                tab.pane_plugin_launch(PaneId(source.pane_id)),
                Some(("codex", Some("chosen"))),
                "{transition}"
            );
        } else {
            assert_eq!(
                tab.pane_plugin_launch(tab.single_pane_id()),
                Some(("codex", None)),
                "{transition}"
            );
        }
        assert_eq!(app.active_harvest_source(), focused, "{transition}");
        if !allowed && !matches!(transition, "cancel" | "replaced-overlay") {
            assert!(
                latest_toast(&app).is_some_and(
                    |message| message.contains("no terminal") || message.contains("no profile")
                ),
                "{transition}"
            );
        }
    }
}

#[test]
fn workspace_restore_preserves_primary_plugin_profile_and_sibling_launches() {
    for named_profile in [false, true] {
        let (mut app, _directory, _, _) = harvest_rerun_capture_fixture();
        mark_capture_plugin_owner(&mut app, PaneId(0), "codex");
        let tabs = app.tabs.as_mut().unwrap();
        let tab = tabs.active_tab_mut().unwrap();
        assert!(tab.set_plugin_profile_reference(
            PaneId(0),
            "codex",
            Some("primary-profile".into())
        ));
        if named_profile {
            tab.pane_manager
                .as_mut()
                .unwrap()
                .get_pane_mut(PaneId(1))
                .unwrap()
                .launch = crate::workspace::WorkspaceLaunch::Profile {
                name: "local-fixture".into(),
            };
        }
        let snapshot = tabs
            .capture_workspace("fixture".into(), false, Default::default())
            .unwrap()
            .tabs
            .remove(0);
        let shell = ShellConfig {
            program: Some("/bin/sh".into()),
            args: vec!["-c".into(), "read fixture".into()],
            ..Default::default()
        };
        let launches = snapshot
            .panes
            .iter()
            .filter(|pane| pane.id != snapshot.primary_pane)
            .map(|pane| (pane.id, (pane.launch.clone().unwrap(), shell.clone())))
            .collect();
        let mut restored = tabs.empty_like();
        restored
            .new_tab_with_command(
                "/bin/sh",
                &shell.args,
                None,
                None,
                Some(TabType::Plugin {
                    plugin_id: "codex".into(),
                }),
                HashMap::new(),
            )
            .unwrap();
        restored
            .apply_workspace_layout(0, &snapshot, &launches)
            .unwrap();
        let tab = restored.active_tab().unwrap();
        assert_eq!(
            tab.pane_plugin_launch(PaneId(0)),
            Some(("codex", Some("primary-profile")))
        );
        assert_eq!(tab.pane_plugin_launch(PaneId(1)), None);
        assert_eq!(
            tab.pane_session(PaneId(0))
                .unwrap()
                .ai_session
                .as_ref()
                .unwrap()
                .profile,
            "primary-profile"
        );
        let captured = restored
            .capture_workspace("restored".into(), false, Default::default())
            .unwrap();
        for pane in &snapshot.panes {
            assert_eq!(
                captured.tabs[0]
                    .panes
                    .iter()
                    .find(|restored| restored.id == pane.id)
                    .unwrap()
                    .launch,
                pane.launch
            );
        }
    }
}
