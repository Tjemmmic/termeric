#[test]
fn control_environment_policy_child_probe() {
    let Some(marker) = std::env::var_os("TERMERIC_TEST_CONTROL_ENV_POLICY_MARKER") else {
        return;
    };
    assert!(std::env::var_os("PATH").is_none());
    assert_eq!(std::env::var("EMPTY_VALUE").unwrap(), "");
    assert_eq!(std::env::var("EXACT_VALUE").unwrap(), "= literal $HOME;λ");
    assert_eq!(std::env::var("TERM_PROGRAM").unwrap(), "Termeric");
    assert_eq!(
        std::env::var_os("PWD").unwrap(),
        std::env::current_dir().unwrap().as_os_str()
    );
    std::fs::write(marker, b"environment policy applied").unwrap();
}

#[test]
fn control_environment_policy_reaches_tab_and_prepared_window_children() {
    use crate::control::protocol::{EnvironmentVariable, LaunchState};
    use termeric_core::pty::EnvironmentPolicy;
    let executable = std::env::current_exe().unwrap();
    let args = [
        "--exact".into(),
        "app::application::tests::control_environment_policy_child_probe".into(),
        "--nocapture".into(),
    ];
    for window_launch in [false, true] {
        for clear in [false, true] {
            let directory = tempfile::tempdir().unwrap();
            let marker = directory.path().join("observed");
            let policy = EnvironmentPolicy {
                clear,
                unset: if clear { vec![] } else { vec!["PATH".into()] },
            };
            let environment = [
                EnvironmentVariable {
                    name: "TERMERIC_TEST_CONTROL_ENV_POLICY_MARKER".into(),
                    value: marker.to_str().unwrap().into(),
                },
                EnvironmentVariable {
                    name: "EMPTY_VALUE".into(),
                    value: String::new(),
                },
                EnvironmentVariable {
                    name: "EXACT_VALUE".into(),
                    value: "= literal $HOME;λ".into(),
                },
            ];
            let mut application =
                Application::from_config(Config::default(), None, LaunchPlan::default()).unwrap();
            application.pending.clear();
            let native_id = WindowId::dummy();
            let mut window = App::new_headless(Config::default()).unwrap();
            window.tabs = Some(crate::tabs::TabManager::new(
                80,
                24,
                Config::default().shell,
                100,
                1024 * 1024,
            ));
            application.windows.insert(native_id, window);
            application.control_window_ids.insert(native_id, 7);
            application.window_order.push(native_id);
            let request_id = "a".repeat(32);
            if window_launch {
                let index = application
                    .prepare_control_launch_window(
                        7,
                        executable.to_str().unwrap(),
                        &args,
                        directory.path().to_str(),
                        None,
                        &environment,
                        &policy,
                    )
                    .unwrap();
                let mut prepared = application.pending.remove(index);
                assert_eq!(prepared.launch.tabs[0].environment_policy, policy);
                prepared.init_tabs().unwrap();
                // Native creation itself remains deferred; exercise its real
                // prepared startup plan, then the same completion registrar.
                application.windows.insert(native_id, prepared);
                application.control_launches.prepare(&request_id).unwrap();
                application.record_control_launch(&request_id, 7, 0);
            } else {
                application
                    .launch_control_tab(
                        false,
                        &request_id,
                        7,
                        executable.to_str().unwrap(),
                        &args,
                        directory.path().to_str(),
                        None,
                        &environment,
                        &policy,
                    )
                    .unwrap();
            }
            let deadline = Instant::now() + std::time::Duration::from_secs(5);
            loop {
                application
                    .windows
                    .get_mut(&native_id)
                    .unwrap()
                    .process_pty_events();
                if let (LaunchState::Exited { exit_code, success }, _) =
                    application.control_launches.query(&request_id).unwrap()
                {
                    assert_eq!(exit_code, 0, "controlled environment probe failed");
                    assert!(success);
                    break;
                }
                assert!(Instant::now() < deadline);
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
            assert_eq!(
                std::fs::read(marker).unwrap(),
                b"environment policy applied"
            );
        }
    }
}
use super::*;
use crate::plugin::permission::PluginPermission;

/// Every capability, scoped ones at `scope`.
fn every_capability(scope: PermissionScope) -> PluginPermissions {
    PluginPermissions::from_permissions(Capability::ALL.into_iter().map(|capability| {
        PluginPermission {
            capability,
            scope: if capability.is_scoped() {
                scope
            } else {
                PermissionScope::Window
            },
        }
    }))
    .unwrap()
}

/// Every capability, scoped ones at all-windows scope.
fn all_windows_grants() -> PluginPermissions {
    every_capability(PermissionScope::AllWindows)
}

/// Execute one plugin host operation for origin window `origin_window_id`,
/// which need not be registered, and decode its result event.
fn plugin_operation(
    application: &mut Application,
    plugin_id: &str,
    origin_window_id: u64,
    grants: &PluginPermissions,
    operation: StructuredResponseAction,
) -> (serde_json::Value, Option<WindowId>) {
    let native_id = application
        .control_native_window_id(origin_window_id)
        .unwrap_or_else(|_| WindowId::from(u64::MAX - origin_window_id));
    let origin = PluginOrigin {
        native_id,
        window_id: origin_window_id,
    };
    // The continuation path is the one the event loop takes, including how
    // it keeps the operation's own tab changes from raising hooks.
    let (continuation, close_target) = application
        .plugin_host_operation_continuation(
            plugin_id,
            origin,
            &operation,
            StructuredHandlerInvocation::fixture(plugin_id, grants.clone()),
        )
        .unwrap();
    (
        serde_json::from_str(&continuation.event_json).unwrap(),
        close_target,
    )
}

/// [`plugin_operation`] with every capability granted for all windows.
fn plugin_event(
    application: &mut Application,
    plugin_id: &str,
    origin_window_id: u64,
    operation: StructuredResponseAction,
) -> serde_json::Value {
    plugin_operation(
        application,
        plugin_id,
        origin_window_id,
        &all_windows_grants(),
        operation,
    )
    .0
}

#[test]
fn a_tab_dropped_on_another_window_moves_there_after_its_pinned_tabs() {
    let window = || {
        let mut app = App::new_headless(Config::default()).unwrap();
        let mut tabs =
            crate::tabs::TabManager::new(80, 24, Config::default().shell, 1_000, 1024 * 1024);
        tabs.new_tab().unwrap();
        tabs.new_tab().unwrap();
        app.tabs = Some(tabs);
        app
    };
    let mut application =
        Application::from_config(Config::default(), None, LaunchPlan::default()).unwrap();
    application.pending.clear();
    let (source_id, destination_id) = (WindowId::dummy(), WindowId::from(1u64));
    let source = window();
    let dragged = source.tabs.as_ref().unwrap().iter().nth(1).unwrap().1.id;
    let mut destination = window();
    destination.tabs.as_mut().unwrap().all_tabs_mut()[0].pinned = true;
    application.windows.insert(source_id, source);
    application.windows.insert(destination_id, destination);
    let drop = PendingTabDrop {
        source: source_id,
        tab_id: dragged,
        deadline: Instant::now() + TAB_DROP_CLAIM,
    };
    // Without a tab bar under the pointer, it goes to the end.
    application.drop_tab_into(drop, destination_id, (10.0, 400.0));
    let source_tabs = application.windows[&source_id].tabs.as_ref().unwrap();
    assert_eq!(source_tabs.count(), 1);
    let destination_tabs = application.windows[&destination_id].tabs.as_ref().unwrap();
    assert_eq!(destination_tabs.count(), 3);
    assert_eq!(
        destination_tabs.active_index(),
        2,
        "the dropped tab is shown"
    );
    assert!(!destination_tabs.active_tab().unwrap().pinned);

    // A tab that is gone by the time a window claims it moves nowhere.
    application.drop_tab_into(drop, destination_id, (10.0, 400.0));
    assert_eq!(
        application.windows[&destination_id]
            .tabs
            .as_ref()
            .unwrap()
            .count(),
        3
    );
}

#[test]
fn a_joining_launch_opens_its_tabs_in_a_new_window_on_this_display() {
    use crate::control::protocol::JoinTab;
    let mut application =
        Application::from_config(Config::default(), None, LaunchPlan::default()).unwrap();
    application.pending.clear();
    let tabs = [
        JoinTab {
            working_directory: Some("/tmp".into()),
            title: Some("Logs".into()),
            program: None,
            args: Vec::new(),
        },
        JoinTab {
            working_directory: None,
            title: None,
            program: Some("htop".into()),
            args: vec!["-d".into(), "10".into()],
        },
    ];
    const OWN: &str = "00000000000000aa";
    // Nothing to join yet.
    assert!(
        application
            .prepare_join_launch(Some("wayland:w-1"), OWN, "wayland:w-1", OWN, None, &tabs)
            .unwrap_err()
            .contains("no open window")
    );
    let native_id = WindowId::dummy();
    let mut open = App::new_headless(Config::default()).unwrap();
    open.config.window.opacity = 0.9;
    application.windows.insert(native_id, open);
    application.window_order.push(native_id);

    assert_eq!(
        application.prepare_join_launch(Some("wayland:w-1"), OWN, "wayland:w-0", OWN, None, &tabs),
        Err("This Termeric runs on another display".into())
    );
    // A launch with, say, another RUST_LOG or XDG_CONFIG_HOME runs its own
    // process rather than silently taking this one's.
    assert_eq!(
        application.prepare_join_launch(
            Some("wayland:w-1"),
            OWN,
            "wayland:w-1",
            "00000000000000bb",
            None,
            &tabs
        ),
        Err("This Termeric runs with another environment".into())
    );
    assert!(
        application.pending.is_empty(),
        "a refused launch queues nothing"
    );
    let index = application
        .prepare_join_launch(
            Some("wayland:w-1"),
            OWN,
            "wayland:w-1",
            OWN,
            Some("token".into()),
            &tabs,
        )
        .unwrap();
    let window = &application.pending[index];
    assert_eq!(window.activation_token.as_deref(), Some("token"));
    assert!(
        !window.allow_auto_restore,
        "a joined window restores nothing"
    );
    assert_eq!(
        window.config.window.opacity, 0.9,
        "the process's settings apply"
    );
    let launched: Vec<_> = window
        .launch
        .tabs
        .iter()
        .map(|tab| {
            (
                tab.working_directory.clone(),
                tab.title.clone(),
                tab.target.clone(),
            )
        })
        .collect();
    assert_eq!(
        launched,
        [
            (
                Some("/tmp".into()),
                Some("Logs".into()),
                crate::cli::LaunchTarget::Shell
            ),
            (
                None,
                None,
                crate::cli::LaunchTarget::Command {
                    program: "htop".into(),
                    args: vec!["-d".into(), "10".into()],
                }
            ),
        ]
    );
}

#[test]
fn native_window_focus_policy_is_truthful_and_non_restoring() {
    assert_eq!(
        native_window_focus_disposition(7, true, true, Some(false), Some(true)).unwrap(),
        NativeWindowFocusDisposition::AlreadyFocused
    );
    assert_eq!(
        native_window_focus_disposition(7, false, false, Some(true), Some(false)).unwrap(),
        NativeWindowFocusDisposition::Request
    );
    assert!(
        native_window_focus_disposition(7, false, true, Some(true), Some(false))
            .unwrap_err()
            .contains("Wayland compositor")
    );
    assert_eq!(
        native_window_focus_disposition(7, false, false, Some(false), Some(false)).unwrap_err(),
        "GUI window 7 is not visible"
    );
    assert_eq!(
        native_window_focus_disposition(7, false, false, Some(true), Some(true)).unwrap_err(),
        "GUI window 7 is minimized"
    );
}

#[test]
fn quick_access_launch_is_excluded_from_automatic_workspace_persistence() {
    let launch = LaunchPlan {
        windows: vec![WindowLaunch {
            force_new: true,
            quick_access: true,
            ..WindowLaunch::default()
        }],
        ..LaunchPlan::default()
    };
    let application = Application::from_config(Config::default(), None, launch).unwrap();
    assert_eq!(application.pending.len(), 1);
    assert!(!application.pending[0].allow_auto_restore);
    assert!(!application.pending[0].retain_workspace_on_exit);
    assert!(!application.pending[0].automatic_persistence);
}

#[test]
fn serial_profiles_reject_process_only_cli_overrides_before_window_creation() {
    let mut config = Config::default();
    config.profiles.insert(
        "console".into(),
        crate::config::LaunchProfileConfig {
            serial: Some(crate::config::SerialProfileConfig {
                device: std::path::PathBuf::from("/dev/ttyUSB0"),
                ..crate::config::SerialProfileConfig::default()
            }),
            ..crate::config::LaunchProfileConfig::default()
        },
    );
    config.validate().unwrap();

    let mut launch = LaunchPlan::default();
    launch.windows[0].tabs[0].target = crate::cli::LaunchTarget::TerminalProfile {
        name: "console".into(),
    };
    launch.windows[0].tabs[0].working_directory = Some(std::env::temp_dir());
    assert!(validate_cli_launches(&config, &launch).is_err());

    launch.windows[0].tabs[0].working_directory = None;
    launch.windows[0].tabs[0]
        .splits
        .push(crate::panes::SplitDirection::Vertical);
    assert!(validate_cli_launches(&config, &launch).is_err());
}

#[test]
fn control_replays_are_exact_collision_safe_and_bounded() {
    use crate::control::protocol::{EnvironmentVariable, Request, Response};

    let different = |result: Option<std::result::Result<Response, String>>| matches!(result, Some(Err(error)) if error.contains("different action"));
    let request = Request::CreateWindow {
        request_id: "0".repeat(32),
        source_window: 7,
    };
    let response = Ok(Response::WindowCreated {
        process_id: 42,
        request_id: "0".repeat(32),
        window_id: 9,
    });
    let mut cache = ControlReplayCache::default();
    cache.record(request.clone(), response.clone());
    assert_eq!(cache.replay(&request), Some(response));
    // The same ID for another target or another kind of request collides.
    assert!(different(cache.replay(&Request::CreateWindow {
        request_id: "0".repeat(32),
        source_window: 8,
    })));
    assert!(different(cache.replay(&Request::CreateTab {
        request_id: "0".repeat(32),
        window_id: 7,
    })));

    // A launch matches only with every field equal, including whether it
    // runs in the background.
    let launch_request = Request::LaunchTab {
        background: false,
        request_id: "2".repeat(32),
        window_id: 7,
        program: "/usr/bin/printf".into(),
        args: vec!["%s".into(), "exact".into()],
        working_directory: Some("/tmp".into()),
        title: Some("build".into()),
        environment_policy: Default::default(),
        environment: vec![EnvironmentVariable {
            name: "BUILD_MODE".into(),
            value: "exact".into(),
        }],
    };
    let launch_response = Ok(Response::TabCreated {
        process_id: 42,
        request_id: "2".repeat(32),
        window_id: 7,
        tab_id: 4,
    });
    cache.record(launch_request.clone(), launch_response.clone());
    assert_eq!(cache.replay(&launch_request), Some(launch_response));
    let mut background_request = launch_request.clone();
    let mut changed_args = launch_request;
    if let Request::LaunchTab { background, .. } = &mut background_request {
        *background = true;
    }
    if let Request::LaunchTab { args, .. } = &mut changed_args {
        args[1] = "different".into();
    }
    assert!(different(cache.replay(&background_request)));
    assert!(different(cache.replay(&changed_args)));

    // Canonical order makes a reordered environment the same request; a
    // changed value or policy is not.
    let window_launch = |unset: [&str; 2], environment: [(&str, &str); 2]| {
        let mut request = Request::LaunchWindow {
            request_id: "3".repeat(32),
            source_window: 7,
            program: "/usr/bin/printf".into(),
            args: vec!["%s".into(), "exact".into()],
            working_directory: Some("/tmp".into()),
            title: Some("window build".into()),
            environment_policy: termeric_core::pty::EnvironmentPolicy {
                clear: false,
                unset: unset.map(String::from).to_vec(),
            },
            environment: environment
                .map(|(name, value)| EnvironmentVariable {
                    name: name.into(),
                    value: value.into(),
                })
                .to_vec(),
        };
        request.canonicalize();
        request
    };
    let window_launch_request = window_launch(
        ["REMOVE_Z", "REMOVE_A"],
        [("WINDOW_Z", "last"), ("WINDOW_A", "exact")],
    );
    let window_launch_response = Ok(Response::WindowLaunched {
        process_id: 42,
        request_id: "3".repeat(32),
        window_id: 9,
        tab_id: 0,
    });
    cache.record(
        window_launch_request.clone(),
        window_launch_response.clone(),
    );
    assert_eq!(
        cache.replay(&window_launch(
            ["REMOVE_A", "REMOVE_Z"],
            [("WINDOW_A", "exact"), ("WINDOW_Z", "last")],
        )),
        Some(window_launch_response)
    );
    assert!(different(cache.replay(&window_launch(
        ["REMOVE_A", "REMOVE_Z"],
        [("WINDOW_Z", "last"), ("WINDOW_A", "different")],
    ))));
    let mut changed_policy = window_launch_request;
    if let Request::LaunchWindow {
        environment_policy, ..
    } = &mut changed_policy
    {
        *environment_policy = termeric_core::pty::EnvironmentPolicy {
            clear: true,
            unset: vec![],
        };
    }
    assert!(different(cache.replay(&changed_policy)));

    // A failure whose outcome may be ambiguous replays; any other
    // failure changed nothing, so it may be retried.
    let terminal_failure = Request::CreateTab {
        request_id: "1".repeat(32),
        window_id: 7,
    };
    cache.record_terminal(
        terminal_failure.clone(),
        Err("authoritative domain result was lost".into()),
    );
    assert_eq!(
        cache.replay(&terminal_failure),
        Some(Err("authoritative domain result was lost".into()))
    );
    let failed = Request::CreateWindow {
        request_id: "f".repeat(32),
        source_window: 7,
    };
    cache.record(failed.clone(), Err("temporary failure".into()));
    assert!(cache.replay(&failed).is_none());
    let failed_input = Request::SendText {
        request_id: "8".repeat(32),
        window_id: 7,
        tab_id: 3,
        pane_id: 1,
        text: "bounded input".into(),
        paste: false,
        enter: true,
    };
    cache.record(failed_input.clone(), Err("PTY write may be partial".into()));
    assert!(matches!(
        cache.replay(&failed_input),
        Some(Err(error)) if error.contains("partial")
    ));

    for index in 1..=MAX_CONTROL_REPLAYS {
        let request_id = format!("{index:032x}");
        cache.record(
            Request::CreateWindow {
                request_id: request_id.clone(),
                source_window: 7,
            },
            Err(format!("failure {index}")),
        );
    }
    assert_eq!(cache.entries.len(), MAX_CONTROL_REPLAYS);
    assert!(cache.replay(&request).is_none());
}

#[test]
fn control_launch_tab_targets_one_stable_window_and_returns_its_tab_id() {
    let mut application =
        Application::from_config(Config::default(), None, LaunchPlan::default()).unwrap();
    application.pending.clear();
    let native_id = WindowId::dummy();
    let mut window = App::new_headless(Config::default()).unwrap();
    window.tabs = Some(crate::tabs::TabManager::new(
        80,
        24,
        Config::default().shell,
        1_000,
        1024 * 1024,
    ));
    application.windows.insert(native_id, window);
    application.window_order.push(native_id);
    application.control_window_ids.insert(native_id, 7);

    let missing = application
        .launch_control_tab(
            false,
            "33333333333333333333333333333333",
            8,
            "/bin/sh",
            &["-c".into(), "sleep 1".into()],
            None,
            Some("wrong target"),
            &[],
            &Default::default(),
        )
        .unwrap_err();
    assert!(missing.contains("does not exist"), "{missing}");
    assert_eq!(
        application.windows[&native_id]
            .tabs
            .as_ref()
            .unwrap()
            .count(),
        0
    );

    let response = application
        .launch_control_tab(
            true,
            "44444444444444444444444444444444",
            7,
            "/bin/sh",
            &["-c".into(), "sleep 1".into()],
            None,
            Some("targeted"),
            &[],
            &Default::default(),
        )
        .unwrap();
    assert!(matches!(
        response,
        crate::control::protocol::Response::TabCreated {
            window_id: 7,
            tab_id: 0,
            ..
        }
    ));
    let tabs = application.windows[&native_id].tabs.as_ref().unwrap();
    assert_eq!(tabs.count(), 1);
    assert_eq!(tabs.active_tab().unwrap().title, "targeted");
    assert!(
        application
            .control_launches
            .contains("44444444444444444444444444444444")
    );
    assert!(
        !application
            .control_launches
            .contains("33333333333333333333333333333333")
    );
}

#[test]
fn launch_status_survives_fast_exit_window_cleanup_and_replay_expiry() {
    use crate::control::protocol::{LaunchState, Request};
    for background in [false, true] {
        let mut application =
            Application::from_config(Config::default(), None, LaunchPlan::default()).unwrap();
        application.pending.clear();
        let native_id = WindowId::dummy();
        let mut window = App::new_headless(Config::default()).unwrap();
        window.tabs = Some(crate::tabs::TabManager::new(
            80,
            24,
            Config::default().shell,
            100,
            1024 * 1024,
        ));
        application.windows.insert(native_id, window);
        application.window_order.push(native_id);
        application.control_window_ids.insert(native_id, 7);
        let id = "a".repeat(32);
        let original = application
            .windows
            .get_mut(&native_id)
            .unwrap()
            .tabs
            .as_mut()
            .unwrap()
            .new_tab_with_command(
                "/bin/sh",
                &["-c".into(), "read unused".into()],
                None,
                Some("original"),
                None,
                Default::default(),
            )
            .unwrap();
        application
            .launch_control_tab(
                background,
                &id,
                7,
                "/bin/sh",
                &["-c".into(), "exit 23".into()],
                None,
                None,
                &[],
                &Default::default(),
            )
            .unwrap();
        let selected = application.windows[&native_id]
            .tabs
            .as_ref()
            .unwrap()
            .active_tab_id();
        if background {
            assert_eq!(selected, Some(original));
        } else {
            assert_ne!(selected, Some(original));
        }
        let deadline = Instant::now() + std::time::Duration::from_secs(2);
        loop {
            application
                .windows
                .get_mut(&native_id)
                .unwrap()
                .process_pty_events();
            let (state, closed) = application.control_launches.query(&id).unwrap();
            if background {
                assert_eq!(
                    application.windows[&native_id]
                        .tabs
                        .as_ref()
                        .unwrap()
                        .active_tab_id(),
                    Some(original)
                );
            }
            if closed && matches!(state, LaunchState::Exited { .. }) {
                assert_eq!(
                    state,
                    LaunchState::Exited {
                        exit_code: 23,
                        success: false
                    }
                );
                break;
            }
            assert!(Instant::now() < deadline);
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        application.windows.remove(&native_id);
        assert_eq!(
            application.control_launches.query(&id).unwrap(),
            (
                LaunchState::Exited {
                    exit_code: 23,
                    success: false
                },
                true
            )
        );
        let (tx, rx) = std::sync::mpsc::sync_channel(1);
        let reused = Request::CreateTab {
            request_id: id,
            window_id: 7,
        };
        assert!(application.defer_live_domain_control(&reused, tx).is_none());
        assert!(rx.recv().unwrap().unwrap_err().contains("replay expired"));
    }
}

#[test]
fn control_launch_window_prepares_one_exact_local_command_and_real_pty() {
    use std::os::unix::fs::PermissionsExt;

    let temp = tempfile::tempdir().unwrap();
    let helper = temp.path().join("capture-window-argv.sh");
    std::fs::write(
            &helper,
            "#!/bin/sh\nprintf '[%s]|[%s]|[%s]|[%s]\\n' \"$1\" \"$2\" \"$3\" \"$4\"\nprintf 'WINDOW_ENV=[%s]|[%s]\\n' \"${EMPTY_VALUE-unset}\" \"${WINDOW_VALUE-unset}\"\npwd\nsleep 1\n",
        )
        .unwrap();
    let mut permissions = std::fs::metadata(&helper).unwrap().permissions();
    permissions.set_mode(0o700);
    std::fs::set_permissions(&helper, permissions).unwrap();

    let mut source_config = Config::default();
    source_config.font.size = 19.0;
    let mut source = App::new_headless(source_config.clone()).unwrap();
    let mut source_tabs =
        crate::tabs::TabManager::new(80, 24, source_config.shell.clone(), 1_000, 1024 * 1024);
    source_tabs
        .new_tab_with_command(
            "/bin/sh",
            &crate::test_support::sleeping_args(),
            temp.path().to_str(),
            Some("source"),
            None,
            HashMap::new(),
        )
        .unwrap();
    source.tabs = Some(source_tabs);

    let mut application =
        Application::from_config(Config::default(), None, LaunchPlan::default()).unwrap();
    application.pending.clear();
    let native_id = WindowId::dummy();
    application.windows.insert(native_id, source);
    application.window_order.push(native_id);
    application.control_window_ids.insert(native_id, 7);

    let missing_cwd = application
        .prepare_control_launch_window(
            7,
            "/bin/true",
            &[],
            Some("/definitely/missing/termeric-launch-window-cwd"),
            None,
            &[],
            &Default::default(),
        )
        .unwrap_err();
    assert!(missing_cwd.contains("pass an existing absolute directory"));
    assert!(application.pending.is_empty());

    let helper = helper.to_str().unwrap();
    let result_index = application
        .prepare_control_launch_window(
            7,
            helper,
            &[
                "".into(),
                "two words".into(),
                "$HOME;🙂".into(),
                "--json".into(),
            ],
            None,
            Some("Exact window 🙂"),
            &[
                crate::control::protocol::EnvironmentVariable {
                    name: "EMPTY_VALUE".into(),
                    value: String::new(),
                },
                crate::control::protocol::EnvironmentVariable {
                    name: "WINDOW_VALUE".into(),
                    value: "= exact $HOME;🙂".into(),
                },
            ],
            &Default::default(),
        )
        .unwrap();
    assert_eq!(result_index, 0);
    assert_eq!(application.pending.len(), 1);
    let prepared = &application.pending[0];
    assert_eq!(prepared.config.font.size, 19.0);
    assert!(!prepared.allow_auto_restore);
    assert!(prepared.redact_startup_error_log);
    assert!(prepared.launch.force_new);
    assert_eq!(prepared.launch.tabs.len(), 1);
    let tab = &prepared.launch.tabs[0];
    assert_eq!(tab.working_directory.as_deref(), Some(temp.path()));
    assert_eq!(tab.title.as_deref(), Some("Exact window 🙂"));
    assert!(tab.splits.is_empty());
    assert_eq!(tab.environment.get("EMPTY_VALUE"), Some(&String::new()));
    assert_eq!(
        tab.environment.get("WINDOW_VALUE").map(String::as_str),
        Some("= exact $HOME;🙂")
    );
    assert!(matches!(
        &tab.target,
        crate::cli::LaunchTarget::Command { program, args }
            if program == helper
                && args == &["", "two words", "$HOME;🙂", "--json"]
    ));
    assert!(prepared.launch.workspace.is_none());
    assert!(prepared.launch.domain.is_none());
    assert!(prepared.launch.domain_snapshot.is_none());

    let mut prepared = application.pending.pop().unwrap();
    prepared.init_tabs().unwrap();
    let tabs = prepared.tabs.as_ref().unwrap();
    assert_eq!(tabs.count(), 1);
    assert_eq!(tabs.active_tab_id(), Some(0));
    assert_eq!(tabs.active_tab().unwrap().title, "Exact window 🙂");

    let deadline = Instant::now() + std::time::Duration::from_secs(2);
    while Instant::now() < deadline {
        prepared.process_pty_events();
        let output = prepared
            .tabs
            .as_ref()
            .and_then(crate::tabs::TabManager::active_tab)
            .and_then(crate::tabs::Tab::focused_terminal)
            .map(|terminal| terminal.grid().plain_lines().join("\n"))
            .unwrap_or_default();
        if output.contains("[]|[two words]|[$HOME;🙂]|[--json]")
            && output.contains("WINDOW_ENV=[]|[= exact $HOME;🙂]")
            && output.contains(temp.path().to_str().unwrap())
        {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    panic!("exact argv or inherited cwd output did not reach the command window");
}

#[test]
fn control_launch_window_preparation_is_atomic_for_stale_and_spawn_failures() {
    let temp = tempfile::tempdir().unwrap();
    let mut application =
        Application::from_config(Config::default(), None, LaunchPlan::default()).unwrap();
    application.pending.clear();
    let native_id = WindowId::dummy();
    let source = App::new_headless(Config::default()).unwrap();
    application.windows.insert(native_id, source);
    application.window_order.push(native_id);
    application.control_window_ids.insert(native_id, 7);

    let private_environment = [crate::control::protocol::EnvironmentVariable {
        name: "PRIVATE_BUILD_VALUE".into(),
        value: "must-not-appear-in-errors".into(),
    }];
    let result_index = application
        .prepare_control_launch_window(
            7,
            "/definitely/missing/termeric-launch-window-program",
            &[],
            temp.path().to_str(),
            None,
            &private_environment,
            &Default::default(),
        )
        .unwrap();
    let mut prepared = application.pending.remove(result_index);
    let error = prepared.init_tabs().unwrap_err();
    let error = format!("{error:#}");
    assert!(error.contains("spawn"));
    assert!(!error.contains("PRIVATE_BUILD_VALUE"));
    assert!(!error.contains("must-not-appear-in-errors"));
    assert_eq!(prepared.tabs.as_ref().unwrap().count(), 0);
    assert!(application.pending.is_empty());

    application.windows.get_mut(&native_id).unwrap().should_exit = true;
    let error = application
        .prepare_control_launch_window(
            7,
            "/bin/true",
            &[],
            temp.path().to_str(),
            None,
            &[],
            &Default::default(),
        )
        .unwrap_err();
    assert!(error.contains("closing"));
    assert!(application.pending.is_empty());

    let error = application
        .prepare_control_launch_window(
            8,
            "/bin/true",
            &[],
            temp.path().to_str(),
            None,
            &[],
            &Default::default(),
        )
        .unwrap_err();
    assert!(error.contains("does not exist"));
    assert!(application.pending.is_empty());
}

#[test]
fn control_errors_are_single_line_control_free_and_bounded() {
    let error = bounded_control_error(&format!("bad\n\u{1b}{}", "x".repeat(2_000)));
    assert!(error.len() <= 1_024);
    assert!(!error.chars().any(char::is_control));
}

#[test]
fn control_window_capacity_includes_its_mandatory_initial_tab() {
    assert!(
        validate_control_window_creation_capacity(
            1,
            0,
            crate::control::protocol::MAX_TOTAL_TABS - 1,
        )
        .is_ok()
    );
    assert!(
        validate_control_window_creation_capacity(1, 0, crate::control::protocol::MAX_TOTAL_TABS,)
            .unwrap_err()
            .contains("process tab limit")
    );
    assert!(
        validate_control_window_creation_capacity(1, 0, usize::MAX)
            .unwrap_err()
            .contains("process tab limit")
    );
    assert!(
        validate_control_window_creation_capacity(crate::control::protocol::MAX_WINDOWS - 1, 1, 0,)
            .unwrap_err()
            .contains("process window limit")
    );
}

#[test]
fn control_window_resize_rejects_stale_or_not_yet_native_targets() {
    let mut application =
        Application::from_config(Config::default(), None, LaunchPlan::default()).unwrap();
    let native_id = WindowId::dummy();
    let window = application.pending.pop().unwrap();
    application.windows.insert(native_id, window);
    application.window_order.push(native_id);
    application.control_window_ids.insert(native_id, 7);

    let error = application
        .resize_control_window("88888888888888888888888888888888", 7, 900, 600)
        .unwrap_err();
    assert!(error.contains("no native window"), "{error}");
    let stale = application
        .resize_control_window("77777777777777777777777777777777", 8, 900, 600)
        .unwrap_err();
    assert!(stale.contains("does not exist"), "{stale}");
}

#[test]
fn control_window_close_targets_one_stable_id_and_only_schedules_teardown() {
    let mut application =
        Application::from_config(Config::default(), None, LaunchPlan::default()).unwrap();
    let native_id = WindowId::dummy();
    let window = application.pending.pop().unwrap();
    application.windows.insert(native_id, window);
    application.window_order.push(native_id);
    application.control_window_ids.insert(native_id, 7);

    let response = application
        .close_control_window("9876543210abcdef9876543210abcdef", 7)
        .unwrap();
    assert!(matches!(
        response,
        crate::control::protocol::Response::WindowCloseRequested { window_id: 7, .. }
    ));
    assert!(application.windows[&native_id].should_exit);
    assert!(
        application
            .close_control_window("0".repeat(32).as_str(), 7)
            .is_err()
    );
    assert!(
        application
            .close_control_window("0".repeat(32).as_str(), 8)
            .is_err()
    );
}

#[test]
fn live_domain_creation_completion_returns_authoritative_gui_ids_and_replays() {
    use crate::app::LiveDomainControl;
    use crate::control::protocol::{Request, Response};
    use crate::domain::model::{DomainSnapshot, LivePaneLayout, LiveTab, SpawnSpec};
    use crate::panes::SplitDirection;
    use crate::tabs::TabManager;
    use std::sync::mpsc::{Receiver, sync_channel};

    type Reply = Receiver<std::result::Result<Response, String>>;
    fn pend(
        application: &mut Application,
        window: WindowId,
        request: &Request,
        control: &LiveDomainControl,
        context: PendingDomainContext,
    ) -> Reply {
        let (responder, reply) = sync_channel(1);
        application
            .pending_domain_controls
            .push(PendingDomainControl {
                request: request.clone(),
                window_id: window,
                control: control.clone(),
                context,
                responders: vec![responder],
            });
        reply
    }
    /// The daemon's answer reaches the window, then the control client.
    fn finish(
        application: &mut Application,
        window: WindowId,
        control: &LiveDomainControl,
        snapshot: &DomainSnapshot,
    ) {
        let result = Ok(snapshot.clone());
        application
            .windows
            .get_mut(&window)
            .unwrap()
            .handle_live_domain_control_completed(control.clone(), result.clone());
        application.complete_live_domain_control(window, control, &result);
    }
    let tab = |id, pane| LiveTab::fixture(id, LivePaneLayout::pane(pane));
    let split_tab = |id, split_id, direction, ratio_millis, first, second| -> LiveTab {
        LiveTab::fixture(
            id,
            LivePaneLayout::split(
                split_id,
                direction,
                ratio_millis,
                LivePaneLayout::pane(first),
                LivePaneLayout::pane(second),
            ),
        )
    };
    let snapshot =
        |active_tab, tabs| DomainSnapshot::fixture("domain-control-ack", active_tab, tabs);

    let initial = snapshot(10, vec![tab(10, 100)]);
    let relay = crate::domain::DomainRelayCommand::local("/bin/true");
    let mut tabs = TabManager::new(80, 24, Config::default().shell, 1_000, 1024 * 1024);
    tabs.attach_domain(&initial, &relay).unwrap();
    let mut window = App::new_headless(Config::default()).unwrap();
    window.tabs = Some(tabs);
    window.live_domain_relay_command = Some(relay);
    window.live_domain_topologies.insert(
        initial.id.clone(),
        crate::domain::model::DomainTopology::from(&initial),
    );
    let domain_tab_count = window.tabs.as_ref().unwrap().count();
    let local_launch = window
        .launch_control_tab(
            false,
            "/bin/true",
            &[],
            None,
            Some("must not launch"),
            &[],
            &Default::default(),
        )
        .unwrap_err();
    assert!(local_launch.contains("domain spawn"), "{local_launch}");
    assert_eq!(window.tabs.as_ref().unwrap().count(), domain_tab_count);

    let mut application =
        Application::from_config(Config::default(), None, LaunchPlan::default()).unwrap();
    application.pending.clear();
    let native_id = WindowId::dummy();
    application.windows.insert(native_id, window);
    application.window_order.push(native_id);
    application.control_window_ids.insert(native_id, 7);
    let domain_id = initial.id.clone();

    // A retry with the same ID waits on the same completion; the same ID
    // for another window collides.
    let create_request = Request::CreateTab {
        request_id: "6".repeat(32),
        window_id: 7,
    };
    let create_control = LiveDomainControl::SpawnTab {
        domain_id: domain_id.clone(),
        spawn: SpawnSpec::default(),
    };
    let create_reply = pend(
        &mut application,
        native_id,
        &create_request,
        &create_control,
        PendingDomainContext::None,
    );
    let (retry_tx, retry_rx) = sync_channel(1);
    assert!(
        application
            .defer_live_domain_control(&create_request, retry_tx)
            .is_none()
    );
    let (collision_tx, collision_rx) = sync_channel(1);
    let collision = Request::CreateTab {
        request_id: "6".repeat(32),
        window_id: 8,
    };
    assert!(
        application
            .defer_live_domain_control(&collision, collision_tx)
            .is_none()
    );
    assert!(matches!(
        collision_rx.recv().unwrap(),
        Err(error) if error.contains("different action")
    ));
    finish(
        &mut application,
        native_id,
        &create_control,
        &snapshot(20, vec![tab(10, 100), tab(20, 200)]),
    );
    let create_response = create_reply.recv().unwrap().unwrap();
    let created_local_tab = match &create_response {
        Response::TabCreated {
            window_id: 7,
            tab_id,
            ..
        } => *tab_id,
        response => panic!("unexpected create response {response:?}"),
    };
    assert_ne!(created_local_tab, 0);
    assert_eq!(retry_rx.recv().unwrap(), Ok(create_response.clone()));
    assert_eq!(
        application.control_replays.replay(&create_request),
        Some(Ok(create_response))
    );

    let split_control = LiveDomainControl::SplitPane {
        domain_id: domain_id.clone(),
        remote_pane_id: 200,
        direction: SplitDirection::Vertical,
        spawn: SpawnSpec::default(),
    };
    let split_reply = pend(
        &mut application,
        native_id,
        &Request::SplitPane {
            request_id: "7".repeat(32),
            window_id: 7,
            tab_id: created_local_tab,
            pane_id: 0,
            direction: SplitDirection::Vertical,
        },
        &split_control,
        PendingDomainContext::None,
    );
    let split = snapshot(
        20,
        vec![
            tab(10, 100),
            split_tab(20, 55, SplitDirection::Vertical, 500, 200, 201),
        ],
    );
    finish(&mut application, native_id, &split_control, &split);
    assert!(matches!(
        split_reply.recv().unwrap().unwrap(),
        Response::PaneCreated {
            window_id: 7,
            tab_id,
            pane_id: 1,
            ..
        } if tab_id == created_local_tab
    ));

    // A close the daemon's snapshot does not show is a failure that a
    // retry gets back rather than repeats.
    let unproven_close_request = Request::ClosePane {
        request_id: "a".repeat(32),
        window_id: 7,
        tab_id: created_local_tab,
        pane_id: 1,
    };
    let unproven_close_control = LiveDomainControl::ClosePane {
        domain_id: domain_id.clone(),
        remote_pane_id: 201,
        local_tab_id: usize::try_from(created_local_tab).unwrap(),
    };
    let unproven_reply = pend(
        &mut application,
        native_id,
        &unproven_close_request,
        &unproven_close_control,
        PendingDomainContext::None,
    );
    application.complete_live_domain_control(native_id, &unproven_close_control, &Ok(split));
    let unproven_error = unproven_reply.recv().unwrap().unwrap_err();
    assert!(unproven_error.contains("still contains the pane"));
    assert_eq!(
        application.control_replays.replay(&unproven_close_request),
        Some(Err(unproven_error))
    );

    let resize_control = LiveDomainControl::ResizeSplit {
        domain_id: domain_id.clone(),
        remote_split_id: 55,
        ratio_millis: 650,
    };
    let resize_reply = pend(
        &mut application,
        native_id,
        &Request::ResizeDivider {
            request_id: "b".repeat(32),
            window_id: 7,
            tab_id: created_local_tab,
            divider_id: 55,
            ratio_millis: 650,
        },
        &resize_control,
        PendingDomainContext::ResizeDivider {
            previous_ratio_millis: 500,
        },
    );
    finish(
        &mut application,
        native_id,
        &resize_control,
        &snapshot(
            20,
            vec![
                tab(10, 100),
                split_tab(20, 55, SplitDirection::Vertical, 650, 200, 201),
            ],
        ),
    );
    assert!(matches!(
        resize_reply.recv().unwrap().unwrap(),
        Response::DividerResized {
            window_id: 7,
            tab_id,
            divider_id: 55,
            previous_ratio_millis: 500,
            ratio_millis: 650,
            changed: true,
            ..
        } if tab_id == created_local_tab
    ));
    assert_eq!(
        application.windows[&native_id]
            .control_live_divider_ratio(created_local_tab, 55)
            .unwrap(),
        650
    );

    let destination_local_tab = application.windows[&native_id]
        .control_local_live_tab_id(&domain_id, 10)
        .unwrap();
    let move_pane_control = application.windows[&native_id]
        .control_live_domain_move_pane(
            created_local_tab,
            1,
            destination_local_tab,
            0,
            SplitDirection::Horizontal,
        )
        .unwrap()
        .unwrap();
    assert!(matches!(
        &move_pane_control,
        LiveDomainControl::MovePane {
            remote_pane_id: 201,
            source_tab_id: 20,
            destination_tab_id: 10,
            target_pane_id: 100,
            direction: SplitDirection::Horizontal,
            ..
        }
    ));
    let move_pane_reply = pend(
        &mut application,
        native_id,
        &Request::MovePane {
            request_id: "d".repeat(32),
            source_window_id: 7,
            source_tab_id: created_local_tab,
            pane_id: 1,
            destination_window_id: 7,
            destination_tab_id: destination_local_tab,
            target_pane_id: 0,
            direction: SplitDirection::Horizontal,
        },
        &move_pane_control,
        PendingDomainContext::None,
    );
    let destination = || split_tab(10, 56, SplitDirection::Horizontal, 500, 100, 201);
    finish(
        &mut application,
        native_id,
        &move_pane_control,
        &snapshot(10, vec![destination(), tab(20, 200)]),
    );
    let moved_local_pane = match move_pane_reply.recv().unwrap().unwrap() {
        Response::PaneMoved {
            source_window_id: 7,
            source_tab_id,
            source_pane_id: 1,
            destination_window_id: 7,
            destination_tab_id,
            destination_pane_id,
            target_pane_id: 0,
            ..
        } if source_tab_id == created_local_tab && destination_tab_id == destination_local_tab => {
            destination_pane_id
        }
        response => panic!("unexpected live pane move response {response:?}"),
    };
    assert_ne!(moved_local_pane, 0);

    let (move_tab_control, move_changed) = application.windows[&native_id]
        .control_live_domain_move_tab(created_local_tab, 0)
        .unwrap()
        .unwrap();
    assert!(move_changed);
    assert!(matches!(
        &move_tab_control,
        LiveDomainControl::MoveTab {
            remote_tab_id: 20,
            index: 0,
            ..
        }
    ));
    let move_tab_reply = pend(
        &mut application,
        native_id,
        &Request::MoveTab {
            request_id: "c".repeat(32),
            source_window_id: 7,
            tab_id: created_local_tab,
            destination_window_id: 7,
            destination_index: 0,
        },
        &move_tab_control,
        PendingDomainContext::MoveTab { changed: true },
    );
    finish(
        &mut application,
        native_id,
        &move_tab_control,
        &snapshot(10, vec![tab(20, 200), destination()]),
    );
    assert!(matches!(
        move_tab_reply.recv().unwrap().unwrap(),
        Response::TabMoved {
            source_window_id: 7,
            source_tab_id,
            destination_window_id: 7,
            destination_tab_id,
            destination_index: 0,
            source_window_closing: false,
            changed: true,
            ..
        } if source_tab_id == created_local_tab && destination_tab_id == created_local_tab
    ));

    let close_pane_control = LiveDomainControl::ClosePane {
        domain_id: domain_id.clone(),
        remote_pane_id: 201,
        local_tab_id: usize::try_from(destination_local_tab).unwrap(),
    };
    let close_pane_reply = pend(
        &mut application,
        native_id,
        &Request::ClosePane {
            request_id: "8".repeat(32),
            window_id: 7,
            tab_id: destination_local_tab,
            pane_id: moved_local_pane,
        },
        &close_pane_control,
        PendingDomainContext::None,
    );
    finish(
        &mut application,
        native_id,
        &close_pane_control,
        &snapshot(10, vec![tab(20, 200), tab(10, 100)]),
    );
    assert!(matches!(
        close_pane_reply.recv().unwrap().unwrap(),
        Response::PaneClosed {
            window_id: 7,
            tab_id,
            pane_id,
            remaining_panes: 1,
            focused_pane_id: 0,
            ..
        } if tab_id == destination_local_tab && pane_id == moved_local_pane
    ));
    assert!(
        application.windows[&native_id]
            .control_live_domain_close_pane(destination_local_tab, 0)
            .unwrap_err()
            .contains("sole pane")
    );

    let close_tab_control = LiveDomainControl::CloseTab {
        domain_id: domain_id.clone(),
        remote_tab_id: 20,
        local_tab_id: usize::try_from(created_local_tab).unwrap(),
    };
    let close_tab_reply = pend(
        &mut application,
        native_id,
        &Request::CloseTab {
            request_id: "9".repeat(32),
            window_id: 7,
            tab_id: created_local_tab,
        },
        &close_tab_control,
        PendingDomainContext::None,
    );
    finish(
        &mut application,
        native_id,
        &close_tab_control,
        &snapshot(10, vec![tab(10, 100)]),
    );
    assert!(matches!(
        close_tab_reply.recv().unwrap().unwrap(),
        Response::TabCloseRequested {
            window_id: 7,
            tab_id,
            ..
        } if tab_id == created_local_tab
    ));
    let remaining_tab = application.windows[&native_id]
        .control_local_live_tab_id(&domain_id, 10)
        .unwrap();
    assert!(
        application.windows[&native_id]
            .control_live_domain_close_tab(remaining_tab)
            .unwrap_err()
            .contains("last tab")
    );
    assert!(application.pending_domain_controls.is_empty());
}

#[test]
fn explicit_multi_window_plan_disables_implicit_restore_for_every_window() {
    let plan = LaunchPlan {
        windows: vec![WindowLaunch::default(), WindowLaunch::default()],
        ..LaunchPlan::default()
    };
    let application = Application::from_config(Config::default(), None, plan).unwrap();
    assert_eq!(application.pending.len(), 2);
    assert!(
        application
            .pending
            .iter()
            .all(|window| !window.allow_auto_restore)
    );
}

#[test]
fn plugin_workspace_query_inventory_is_ordered_bounded_and_excludes_closing_windows() {
    let plan = LaunchPlan {
        windows: vec![WindowLaunch::default(); super::super::PLUGIN_PROCESS_MAX_WINDOWS + 2],
        ..LaunchPlan::default()
    };
    let mut application = Application::from_config(Config::default(), None, plan).unwrap();
    application.pending[1].should_exit = true;
    let windows: Vec<_> = application
        .pending
        .iter()
        .enumerate()
        .map(|(index, window)| (100 + index as u64, window))
        .collect();

    let one_window = build_plugin_process_inventory(&windows[..1]);
    assert_eq!(one_window.windows.len(), 1);
    assert_eq!(one_window.windows[0].id, 100);
    assert!(!one_window.truncated);

    let two_live_windows = build_plugin_process_inventory(&windows[..3]);
    assert_eq!(two_live_windows.windows.len(), 2);
    assert_eq!(two_live_windows.windows[0].id, 100);
    assert_eq!(two_live_windows.windows[1].id, 102);
    assert!(!two_live_windows.truncated);

    drop(windows);
    application.pending[0].current_workspace_id = Some(format!("root\n{}", "β".repeat(2_000)));
    let windows: Vec<_> = application
        .pending
        .iter()
        .enumerate()
        .map(|(index, window)| (100 + index as u64, window))
        .collect();
    let inventory = build_plugin_process_inventory(&windows);
    assert_eq!(inventory.process_id, std::process::id());
    assert_eq!(
        inventory.windows.len(),
        super::super::PLUGIN_PROCESS_MAX_WINDOWS
    );
    assert_eq!(inventory.windows[0].id, 100);
    assert_eq!(inventory.windows[1].id, 102);
    assert!(inventory.truncated);
    assert!(inventory.windows[0].truncated);
    let workspace = inventory.windows[0].workspace_id.as_deref().unwrap();
    assert!(!workspace.contains('\n'));
    assert!(std::str::from_utf8(workspace.as_bytes()).is_ok());

    let encoded = super::super::plugin_actions::plugin_workspace_query_event(
        "window-inspector",
        102,
        "all-windows",
        &inventory,
    )
    .unwrap();
    assert!(encoded.len() <= crate::plugin::structured::MAX_PERSISTENT_SOURCE_EVENT_BYTES);
    let value: serde_json::Value = serde_json::from_str(&encoded).unwrap();
    assert_eq!(value["event"]["type"], "workspace-query-result");
    assert_eq!(value["event"]["query_id"], "all-windows");
    assert_eq!(value["origin_window_id"], 102);
    assert_eq!(value["event"]["windows"][0]["id"], 100);
    assert_eq!(value["event"]["windows"][1]["id"], 102);
    assert_eq!(value["event"]["truncated"], true);
}

#[test]
fn plugin_navigation_is_exact_atomic_non_activating_and_hook_suppressed() {
    let shell = crate::config::ShellConfig {
        program: Some("/bin/sh".into()),
        args: vec!["-c".into(), "exit 0".into()],
        env: Default::default(),
    };
    let mut tabs = crate::tabs::TabManager::for_test(shell);
    let first = tabs
        .new_tab_with_command(
            "/bin/sh",
            &["-c".into(), "exit 0".into()],
            None,
            Some("first"),
            None,
            std::collections::HashMap::new(),
        )
        .unwrap();
    let second = tabs
        .new_tab_with_command(
            "/bin/sh",
            &["-c".into(), "exit 0".into()],
            None,
            Some("second"),
            None,
            std::collections::HashMap::new(),
        )
        .unwrap();
    let first_pane = tabs.tab_by_id(first).unwrap().panes.focused_id().0;
    assert_eq!(tabs.active_tab_id(), Some(second));

    let observed_switches = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let mut target = App::new_headless(Config::default()).unwrap();
    target.tabs = Some(tabs);
    // The event loop has already reported the window's active tab.
    target.report_active_tab_switch();
    target
        .plugin_manager
        .register(crate::test_support::hook_counter(
            |hook| matches!(hook, crate::plugin::Hook::TabSwitched { .. }),
            &observed_switches,
        ))
        .unwrap();

    let mut application =
        Application::from_config(Config::default(), None, LaunchPlan::default()).unwrap();
    application.pending.clear();
    let native_id = WindowId::dummy();
    application.windows.insert(native_id, target);
    application.window_order.push(native_id);
    application.control_window_ids.insert(native_id, 7);

    let selected = plugin_event(
        &mut application,
        "navigator",
        9,
        crate::plugin::structured::StructuredResponseAction::SelectTab {
            action_id: "select-sibling".into(),
            window_id: Some(7),
            tab_id: first as u64,
        },
    );
    assert_eq!(selected["plugin_id"], "navigator");
    assert_eq!(selected["event"]["type"], "action-result");
    assert_eq!(selected["origin_window_id"], 9);
    assert_eq!(selected["event"]["window_id"], 7);
    assert_eq!(selected["event"]["tab_id"], first as u64);
    assert_eq!(selected["event"]["changed"], true);
    assert!(application.windows[&native_id].window.is_none());
    assert_eq!(
        application.windows[&native_id]
            .tabs
            .as_ref()
            .unwrap()
            .active_tab_id(),
        Some(first)
    );
    application
        .windows
        .get_mut(&native_id)
        .unwrap()
        .report_active_tab_switch();
    assert_eq!(
        observed_switches.load(std::sync::atomic::Ordering::SeqCst),
        0
    );

    let selected_same_window = plugin_event(
        &mut application,
        "navigator",
        7,
        crate::plugin::structured::StructuredResponseAction::SelectTab {
            action_id: "select-current".into(),
            window_id: Some(7),
            tab_id: second as u64,
        },
    );
    assert_eq!(selected_same_window["event"]["ok"], true);
    assert_eq!(selected_same_window["origin_window_id"], 7);
    assert_eq!(selected_same_window["event"]["changed"], true);

    let missing_pane = plugin_event(
        &mut application,
        "navigator",
        7,
        crate::plugin::structured::StructuredResponseAction::FocusPane {
            action_id: "missing-pane".into(),
            window_id: Some(7),
            tab_id: first as u64,
            pane_id: u32::MAX,
        },
    );
    assert_eq!(missing_pane["event"]["ok"], false);
    assert_eq!(
        missing_pane["event"]["tab_changed"],
        serde_json::Value::Null
    );
    assert_eq!(
        application.windows[&native_id]
            .tabs
            .as_ref()
            .unwrap()
            .active_tab_id(),
        Some(second)
    );
    let error = missing_pane["event"]["error"].as_str().unwrap();
    assert!(!error.is_empty());
    assert!(!error.chars().any(char::is_control));

    let focused = plugin_event(
        &mut application,
        "navigator",
        9,
        crate::plugin::structured::StructuredResponseAction::FocusPane {
            action_id: "focus-sibling".into(),
            window_id: Some(7),
            tab_id: first as u64,
            pane_id: first_pane,
        },
    );
    assert_eq!(focused["event"]["ok"], true);
    assert_eq!(focused["event"]["tab_changed"], true);
    assert_eq!(focused["event"]["pane_changed"], false);
    application
        .windows
        .get_mut(&native_id)
        .unwrap()
        .report_active_tab_switch();
    assert_eq!(
        observed_switches.load(std::sync::atomic::Ordering::SeqCst),
        0
    );

    let stale = plugin_event(
        &mut application,
        "navigator",
        9,
        crate::plugin::structured::StructuredResponseAction::SelectTab {
            action_id: "stale-window".into(),
            window_id: Some(8),
            tab_id: first as u64,
        },
    );
    assert_eq!(stale["event"]["ok"], false);
    assert!(
        stale["event"]["error"]
            .as_str()
            .unwrap()
            .contains("does not exist")
    );

    application
        .windows
        .get_mut(&native_id)
        .unwrap()
        .tabs
        .as_mut()
        .unwrap()
        .tab_mut_by_id(second)
        .unwrap()
        .closed = true;
    let closed = plugin_event(
        &mut application,
        "navigator",
        9,
        crate::plugin::structured::StructuredResponseAction::SelectTab {
            action_id: "closed-tab".into(),
            window_id: Some(7),
            tab_id: second as u64,
        },
    );
    assert_eq!(closed["event"]["ok"], false);
    assert!(
        closed["event"]["error"]
            .as_str()
            .unwrap()
            .contains("closing")
    );

    application.windows.get_mut(&native_id).unwrap().should_exit = true;
    let closing = plugin_event(
        &mut application,
        "navigator",
        9,
        crate::plugin::structured::StructuredResponseAction::SelectTab {
            action_id: "closing-window".into(),
            window_id: Some(7),
            tab_id: second as u64,
        },
    );
    assert_eq!(closing["event"]["ok"], false);
    assert!(
        closing["event"]["error"]
            .as_str()
            .unwrap()
            .contains("closing")
    );

    let target = application.windows.get_mut(&native_id).unwrap();
    target.should_exit = false;
    target.tabs = None;
    let uninitialized = plugin_event(
        &mut application,
        "navigator",
        9,
        crate::plugin::structured::StructuredResponseAction::SelectTab {
            action_id: "uninitialized-window".into(),
            window_id: Some(7),
            tab_id: first as u64,
        },
    );
    assert_eq!(uninitialized["event"]["ok"], false);
    assert!(
        uninitialized["event"]["error"]
            .as_str()
            .unwrap()
            .contains("no initialized tab manager")
    );
    assert!(
        selected.to_string().len() <= crate::plugin::structured::MAX_PERSISTENT_SOURCE_EVENT_BYTES
    );
}

/// The same-window path and live-domain refusal are covered in
/// `app/tests.rs`.
#[test]
fn plugin_tab_creation_in_another_window_is_exact_non_activating_and_hook_suppressed() {
    let shell = crate::config::ShellConfig {
        program: Some("/bin/sh".into()),
        args: vec!["-c".into(), "exit 0".into()],
        env: Default::default(),
    };
    let mut tabs = crate::tabs::TabManager::for_test(shell);
    let initial = tabs.new_tab().unwrap();
    let observed_creations = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let mut target = App::new_headless(Config::default()).unwrap();
    target.tabs = Some(tabs);
    target
        .plugin_manager
        .register(crate::test_support::hook_counter(
            |hook| matches!(hook, crate::plugin::Hook::TabCreated { .. }),
            &observed_creations,
        ))
        .unwrap();

    let mut application =
        Application::from_config(Config::default(), None, LaunchPlan::default()).unwrap();
    application.pending.clear();
    let native_id = WindowId::dummy();
    application.windows.insert(native_id, target);
    application.window_order.push(native_id);
    application.control_window_ids.insert(native_id, 7);

    let created = plugin_event(
        &mut application,
        "tab-builder",
        9,
        crate::plugin::structured::StructuredResponseAction::CreateTab {
            action_id: "new-sibling-task".into(),
            window_id: Some(7),
        },
    );
    assert_eq!(created["plugin_id"], "tab-builder");
    assert_eq!(created["event"]["type"], "action-result");
    assert_eq!(created["event"]["action"], "create-tab");
    assert_eq!(created["event"]["action_id"], "new-sibling-task");
    assert_eq!(created["event"]["ok"], true);
    assert_eq!(created["origin_window_id"], 9);
    assert_eq!(created["event"]["window_id"], 7);
    let created_tab = created["event"]["created_tab_id"].as_u64().unwrap() as usize;
    assert_ne!(created_tab, initial);
    let target = &application.windows[&native_id];
    assert!(target.window.is_none());
    assert_eq!(target.tabs.as_ref().unwrap().count(), 2);
    assert_eq!(
        target.tabs.as_ref().unwrap().active_tab_id(),
        Some(created_tab)
    );
    assert_eq!(
        observed_creations.load(std::sync::atomic::Ordering::SeqCst),
        0
    );

    let stale = plugin_event(
        &mut application,
        "tab-builder",
        9,
        crate::plugin::structured::StructuredResponseAction::CreateTab {
            action_id: "stale-window".into(),
            window_id: Some(8),
        },
    );
    assert_eq!(stale["event"]["ok"], false);
    assert!(stale["event"]["created_tab_id"].is_null());
    assert!(
        stale["event"]["error"]
            .as_str()
            .unwrap()
            .contains("does not exist")
    );
    assert_eq!(
        application.windows[&native_id]
            .tabs
            .as_ref()
            .unwrap()
            .count(),
        2
    );

    application.windows.get_mut(&native_id).unwrap().should_exit = true;
    let closing = plugin_event(
        &mut application,
        "tab-builder",
        9,
        crate::plugin::structured::StructuredResponseAction::CreateTab {
            action_id: "closing-window".into(),
            window_id: Some(7),
        },
    );
    assert_eq!(closing["event"]["ok"], false);
    assert!(
        closing["event"]["error"]
            .as_str()
            .unwrap()
            .contains("closing")
    );
    assert_eq!(
        application.windows[&native_id]
            .tabs
            .as_ref()
            .unwrap()
            .count(),
        2
    );

    assert!(
        created.to_string().len() <= crate::plugin::structured::MAX_PERSISTENT_SOURCE_EVENT_BYTES
    );
    assert!(
        validate_process_tab_creation_capacity(crate::control::protocol::MAX_TOTAL_TABS)
            .unwrap_err()
            .contains("process tab limit")
    );
}

#[test]
fn plugin_pane_creation_is_exact_atomic_and_non_activating() {
    let shell = crate::config::ShellConfig {
        program: Some("/bin/sh".into()),
        args: vec!["-c".into(), "exit 0".into()],
        env: Default::default(),
    };
    let mut tabs = crate::tabs::TabManager::for_test(shell.clone());
    let tab_id = tabs.new_tab().unwrap();
    let tab = tabs.tab_mut_by_id(tab_id).unwrap();
    let panes = tab.panes_for_test(shell, 800.0, 480.0, 10.0, 20.0);
    panes
        .split(crate::panes::SplitDirection::Horizontal)
        .unwrap();

    let mut target = App::new_headless(Config::default()).unwrap();
    target.tabs = Some(tabs);
    target.control_window_id = 7;
    let mut application =
        Application::from_config(Config::default(), None, LaunchPlan::default()).unwrap();
    application.pending.clear();
    let native_id = WindowId::dummy();
    application.windows.insert(native_id, target);
    application.window_order.push(native_id);
    application.control_window_ids.insert(native_id, 7);

    let created = plugin_event(
        &mut application,
        "pane-builder",
        9,
        crate::plugin::structured::StructuredResponseAction::CreatePane {
            action_id: "split-sibling-task".into(),
            window_id: Some(7),
            tab_id: tab_id as u64,
            pane_id: 0,
            direction: crate::panes::SplitDirection::Vertical,
        },
    );
    assert_eq!(created["plugin_id"], "pane-builder");
    assert_eq!(created["event"]["action"], "create-pane");
    assert_eq!(created["event"]["action_id"], "split-sibling-task");
    assert_eq!(created["event"]["ok"], true);
    assert_eq!(created["origin_window_id"], 9);
    assert_eq!(created["event"]["window_id"], 7);
    assert_eq!(created["event"]["tab_id"], tab_id as u64);
    assert_eq!(created["event"]["pane_id"], 0);
    assert_eq!(created["event"]["direction"], "vertical");
    let created_pane = created["event"]["created_pane_id"].as_u64().unwrap() as u32;
    let target = &application.windows[&native_id];
    assert!(target.window.is_none());
    assert_eq!(target.tabs.as_ref().unwrap().active_tab_id(), Some(tab_id));
    let manager = &target
        .tabs
        .as_ref()
        .unwrap()
        .tab_by_id(tab_id)
        .unwrap()
        .panes;
    assert_eq!(manager.pane_count(), 3);
    assert!(
        manager
            .get_pane(crate::panes::PaneId(created_pane))
            .is_some()
    );
    assert_eq!(manager.focused_id(), crate::panes::PaneId(created_pane));

    let created_same_window = plugin_event(
        &mut application,
        "pane-builder",
        7,
        crate::plugin::structured::StructuredResponseAction::CreatePane {
            action_id: "split-current-task".into(),
            window_id: Some(7),
            tab_id: tab_id as u64,
            pane_id: 0,
            direction: crate::panes::SplitDirection::Horizontal,
        },
    );
    assert_eq!(created_same_window["event"]["ok"], true);
    assert_eq!(created_same_window["origin_window_id"], 7);
    assert_eq!(created_same_window["event"]["window_id"], 7);
    assert_eq!(
        application.windows[&native_id]
            .tabs
            .as_ref()
            .unwrap()
            .tab_by_id(tab_id)
            .unwrap()
            .panes
            .pane_count(),
        4
    );

    let stale_window = plugin_event(
        &mut application,
        "pane-builder",
        9,
        crate::plugin::structured::StructuredResponseAction::CreatePane {
            action_id: "stale-window".into(),
            window_id: Some(8),
            tab_id: tab_id as u64,
            pane_id: 0,
            direction: crate::panes::SplitDirection::Vertical,
        },
    );
    assert_eq!(stale_window["event"]["ok"], false);
    assert!(
        stale_window["event"]["error"]
            .as_str()
            .unwrap()
            .contains("does not exist")
    );

    let stale_pane = plugin_event(
        &mut application,
        "pane-builder",
        9,
        crate::plugin::structured::StructuredResponseAction::CreatePane {
            action_id: "stale-pane".into(),
            window_id: Some(7),
            tab_id: tab_id as u64,
            pane_id: u32::MAX,
            direction: crate::panes::SplitDirection::Vertical,
        },
    );
    assert_eq!(stale_pane["event"]["ok"], false);
    assert!(stale_pane["event"]["created_pane_id"].is_null());
    assert_eq!(
        application.windows[&native_id]
            .tabs
            .as_ref()
            .unwrap()
            .tab_by_id(tab_id)
            .unwrap()
            .panes
            .pane_count(),
        4
    );

    let tab = application
        .windows
        .get_mut(&native_id)
        .unwrap()
        .tabs
        .as_mut()
        .unwrap()
        .tab_mut_by_id(tab_id)
        .unwrap();
    tab.live_domain = Some(crate::tabs::LiveDomainTab {
        domain_id: "fake-domain".into(),
        tab_id: 12,
        pane_ids: HashMap::new(),
    });
    let live_domain = plugin_event(
        &mut application,
        "pane-builder",
        9,
        crate::plugin::structured::StructuredResponseAction::CreatePane {
            action_id: "live-domain".into(),
            window_id: Some(7),
            tab_id: tab_id as u64,
            pane_id: 0,
            direction: crate::panes::SplitDirection::Vertical,
        },
    );
    assert_eq!(live_domain["event"]["ok"], false);
    assert!(
        live_domain["event"]["error"]
            .as_str()
            .unwrap()
            .contains("live-domain")
    );

    let tab = application
        .windows
        .get_mut(&native_id)
        .unwrap()
        .tabs
        .as_mut()
        .unwrap()
        .tab_mut_by_id(tab_id)
        .unwrap();
    tab.live_domain = None;
    tab.tab_type = crate::tabs::TabType::Serial {
        name: "fixture".into(),
    };
    let serial = plugin_event(
        &mut application,
        "pane-builder",
        9,
        crate::plugin::structured::StructuredResponseAction::CreatePane {
            action_id: "serial".into(),
            window_id: Some(7),
            tab_id: tab_id as u64,
            pane_id: 0,
            direction: crate::panes::SplitDirection::Vertical,
        },
    );
    assert_eq!(serial["event"]["ok"], false);
    assert!(
        serial["event"]["error"]
            .as_str()
            .unwrap()
            .contains("Serial")
    );

    application.windows.get_mut(&native_id).unwrap().should_exit = true;
    let closing = plugin_event(
        &mut application,
        "pane-builder",
        9,
        crate::plugin::structured::StructuredResponseAction::CreatePane {
            action_id: "closing-window".into(),
            window_id: Some(7),
            tab_id: tab_id as u64,
            pane_id: 0,
            direction: crate::panes::SplitDirection::Vertical,
        },
    );
    assert_eq!(closing["event"]["ok"], false);
    assert!(
        closing["event"]["error"]
            .as_str()
            .unwrap()
            .contains("closing")
    );
    assert!(
        created.to_string().len() <= crate::plugin::structured::MAX_PERSISTENT_SOURCE_EVENT_BYTES
    );
    assert!(
        validate_process_pane_creation_capacity(crate::control::protocol::MAX_TOTAL_PANES)
            .unwrap_err()
            .contains("process pane limit")
    );
}

#[test]
fn plugin_terminal_queries_are_exact_bounded_and_non_activating() {
    let shell = crate::config::ShellConfig {
        program: Some("/bin/sh".into()),
        args: vec!["-c".into(), "exit 0".into()],
        env: Default::default(),
    };
    let mut tabs = crate::tabs::TabManager::for_test(shell);
    let target_tab = tabs
        .new_tab_with_command(
            "/bin/sh",
            &["-c".into(), "exit 0".into()],
            None,
            Some("query target"),
            None,
            std::collections::HashMap::new(),
        )
        .unwrap();
    let untouched_tab = tabs
        .new_tab_with_command(
            "/bin/sh",
            &["-c".into(), "exit 0".into()],
            None,
            Some("active tab"),
            None,
            std::collections::HashMap::new(),
        )
        .unwrap();
    tabs.tab_mut_by_id(target_tab)
        .unwrap()
        .focused_terminal_mut()
        .unwrap()
        .process(b"sibling-visible-text\x1b[?2026h\rhidden-sibling-update");
    assert_eq!(tabs.active_tab_id(), Some(untouched_tab));

    let mut target = App::new_headless(Config::default()).unwrap();
    target.tabs = Some(tabs);
    target.control_window_id = 7;
    let mut application =
        Application::from_config(Config::default(), None, LaunchPlan::default()).unwrap();
    application.pending.clear();
    let native_id = WindowId::dummy();
    application.windows.insert(native_id, target);
    application.window_order.push(native_id);
    application.control_window_ids.insert(native_id, 7);

    let encoded = plugin_event(
        &mut application,
        "terminal-reader",
        9,
        StructuredResponseAction::QueryTerminal {
            query_id: "sibling-output".into(),
            window_id: Some(7),
            tab_id: target_tab as u64,
            pane_id: 0,
            history_lines: 20,
            max_bytes: 2048,
        },
    );
    assert!(
        encoded.to_string().len() <= crate::plugin::structured::MAX_PERSISTENT_SOURCE_EVENT_BYTES
    );
    let value = encoded;
    assert_eq!(value["schema_version"], 1);
    assert_eq!(value["plugin_id"], "terminal-reader");
    assert_eq!(value["event"]["type"], "terminal-query-result");
    assert_eq!(value["event"]["query_id"], "sibling-output");
    assert_eq!(value["event"]["ok"], true);
    assert_eq!(value["origin_window_id"], 9);
    assert_eq!(value["event"]["window_id"], 7);
    assert_eq!(value["event"]["tab_id"], target_tab as u64);
    assert_eq!(value["event"]["pane_id"], 0);
    assert!(
        value["event"]["text"]
            .as_str()
            .unwrap()
            .contains("sibling-visible-text")
    );
    assert!(
        !value["event"]["text"]
            .as_str()
            .unwrap()
            .contains("hidden-sibling-update")
    );
    assert!(application.windows[&native_id].window.is_none());
    assert_eq!(
        application.windows[&native_id]
            .tabs
            .as_ref()
            .unwrap()
            .active_tab_id(),
        Some(untouched_tab)
    );

    let missing = plugin_event(
        &mut application,
        "terminal-reader",
        9,
        StructuredResponseAction::QueryTerminal {
            query_id: "stale-target".into(),
            window_id: Some(8),
            tab_id: target_tab as u64,
            pane_id: 0,
            history_lines: 0,
            max_bytes: 64,
        },
    );
    assert_eq!(missing["event"]["ok"], false);
    assert_eq!(missing["event"]["query_id"], "stale-target");
    assert!(missing["event"]["text"].is_null());
    let error = missing["event"]["error"].as_str().unwrap();
    assert!(error.contains("does not exist"));
    assert!(!error.chars().any(char::is_control));

    application
        .windows
        .get_mut(&native_id)
        .unwrap()
        .tabs
        .as_mut()
        .unwrap()
        .tab_mut_by_id(target_tab)
        .unwrap()
        .focused_terminal_mut()
        .unwrap()
        .process(format!("\x1b[?2026l\x1b[2J\x1b[H{}", "\\".repeat(9_000)).as_bytes());
    let escaped = plugin_event(
        &mut application,
        "terminal-reader",
        9,
        StructuredResponseAction::QueryTerminal {
            query_id: "escaped-text".into(),
            window_id: Some(7),
            tab_id: target_tab as u64,
            pane_id: 0,
            history_lines: 256,
            max_bytes: crate::plugin::structured::MAX_TERMINAL_QUERY_BYTES,
        },
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

    assert!(
        application
            .plugin_host_operation_event(
                "terminal-reader",
                PluginOrigin {
                    native_id,
                    window_id: 9,
                },
                &all_windows_grants(),
                &StructuredResponseAction::QueryTerminal {
                    query_id: "invalid-history".into(),
                    window_id: Some(7),
                    tab_id: target_tab as u64,
                    pane_id: 0,
                    history_lines: crate::control::protocol::MAX_TEXT_HISTORY_LINES + 1,
                    max_bytes: 64,
                },
            )
            .unwrap_err()
            .contains("limited")
    );
    assert!(
        application
            .plugin_host_operation_event(
                "terminal-reader",
                PluginOrigin {
                    native_id,
                    window_id: 9,
                },
                &all_windows_grants(),
                &StructuredResponseAction::QueryTerminal {
                    query_id: "invalid-bytes".into(),
                    window_id: Some(7),
                    tab_id: target_tab as u64,
                    pane_id: 0,
                    history_lines: 0,
                    max_bytes: 0,
                },
            )
            .unwrap_err()
            .contains("between")
    );

    application.windows.get_mut(&native_id).unwrap().should_exit = true;
    let closing = plugin_event(
        &mut application,
        "terminal-reader",
        9,
        StructuredResponseAction::QueryTerminal {
            query_id: "closing-window".into(),
            window_id: Some(7),
            tab_id: target_tab as u64,
            pane_id: 0,
            history_lines: 0,
            max_bytes: 64,
        },
    );
    assert_eq!(closing["event"]["ok"], false);
    assert!(
        closing["event"]["error"]
            .as_str()
            .unwrap()
            .contains("closing")
    );
}

#[test]
fn plugin_terminal_input_is_exact_guarded_policy_gated_and_non_activating() {
    let shell = crate::test_support::sleeping_shell();
    let mut tabs = crate::tabs::TabManager::for_test(shell);
    let target_tab = tabs
        .new_tab_with_command(
            "/bin/sh",
            &crate::test_support::sleeping_args(),
            None,
            Some("input target"),
            None,
            std::collections::HashMap::new(),
        )
        .unwrap();
    let untouched_tab = tabs
        .new_tab_with_command(
            "/bin/sh",
            &crate::test_support::sleeping_args(),
            None,
            Some("active tab"),
            None,
            std::collections::HashMap::new(),
        )
        .unwrap();
    tabs.tab_mut_by_id(target_tab)
        .unwrap()
        .focused_terminal_mut()
        .unwrap()
        .set_mode(termeric_core::term::TerminalMode::BRACKETED_PASTE, true);
    assert_eq!(tabs.active_tab_id(), Some(untouched_tab));

    let mut target = App::new_headless(Config::default()).unwrap();
    target.tabs = Some(tabs);
    target.control_window_id = 7;
    let mut application =
        Application::from_config(Config::default(), None, LaunchPlan::default()).unwrap();
    application.pending.clear();
    let native_id = WindowId::dummy();
    application.windows.insert(native_id, target);
    application.window_order.push(native_id);
    application.control_window_ids.insert(native_id, 7);

    let disabled = plugin_event(
        &mut application,
        "terminal-writer",
        9,
        StructuredResponseAction::SendText {
            action_id: "policy-disabled".into(),
            window_id: Some(7),
            tab_id: target_tab as u64,
            pane_id: 0,
            text: "safe".into(),
            paste: true,
            enter: true,
        },
    );
    assert_eq!(disabled["event"]["ok"], false);
    assert!(
        disabled["event"]["error"]
            .as_str()
            .unwrap()
            .contains("disabled")
    );

    application
        .windows
        .get_mut(&native_id)
        .unwrap()
        .config
        .security
        .allow_gui_control_input = true;
    let sent = plugin_event(
        &mut application,
        "terminal-writer",
        9,
        StructuredResponseAction::SendText {
            action_id: "safe-input".into(),
            window_id: Some(7),
            tab_id: target_tab as u64,
            pane_id: 0,
            text: "safe".into(),
            paste: true,
            enter: true,
        },
    );
    assert!(sent.to_string().len() <= crate::plugin::structured::MAX_PERSISTENT_SOURCE_EVENT_BYTES);
    assert_eq!(sent["plugin_id"], "terminal-writer");
    assert_eq!(sent["event"]["type"], "action-result");
    assert_eq!(sent["event"]["action"], "send-text");
    assert_eq!(sent["event"]["action_id"], "safe-input");
    assert_eq!(sent["event"]["ok"], true);
    assert_eq!(sent["origin_window_id"], 9);
    assert_eq!(sent["event"]["window_id"], 7);
    assert_eq!(sent["event"]["tab_id"], target_tab as u64);
    assert_eq!(sent["event"]["pane_id"], 0);
    assert_eq!(sent["event"]["input_bytes"], 5);
    assert_eq!(sent["event"]["wire_bytes"], 17);
    assert_eq!(sent["event"]["disposition"], "sent");
    assert!(sent["event"]["confirmation_id"].is_null());
    assert!(application.windows[&native_id].window.is_none());
    assert_eq!(
        application.windows[&native_id]
            .tabs
            .as_ref()
            .unwrap()
            .active_tab_id(),
        Some(untouched_tab)
    );

    let stale = plugin_event(
        &mut application,
        "terminal-writer",
        9,
        StructuredResponseAction::SendText {
            action_id: "stale-window".into(),
            window_id: Some(8),
            tab_id: target_tab as u64,
            pane_id: 0,
            text: "safe".into(),
            paste: false,
            enter: false,
        },
    );
    assert_eq!(stale["event"]["ok"], false);
    assert!(
        stale["event"]["error"]
            .as_str()
            .unwrap()
            .contains("does not exist")
    );

    application.windows.get_mut(&native_id).unwrap().should_exit = true;
    let closing = plugin_event(
        &mut application,
        "terminal-writer",
        9,
        StructuredResponseAction::SendText {
            action_id: "closing-window".into(),
            window_id: Some(7),
            tab_id: target_tab as u64,
            pane_id: 0,
            text: "safe".into(),
            paste: false,
            enter: false,
        },
    );
    assert_eq!(closing["event"]["ok"], false);
    assert!(
        closing["event"]["error"]
            .as_str()
            .unwrap()
            .contains("closing")
    );
    application.windows.get_mut(&native_id).unwrap().should_exit = false;

    assert!(
        application
            .windows
            .get_mut(&native_id)
            .unwrap()
            .tabs
            .as_mut()
            .unwrap()
            .tab_mut_by_id(target_tab)
            .unwrap()
            .toggle_focused_pane_read_only()
    );
    let read_only = plugin_event(
        &mut application,
        "terminal-writer",
        7,
        StructuredResponseAction::SendText {
            action_id: "read-only".into(),
            window_id: Some(7),
            tab_id: target_tab as u64,
            pane_id: 0,
            text: "safe".into(),
            paste: false,
            enter: false,
        },
    );
    assert_eq!(read_only["event"]["ok"], false);
    assert!(
        read_only["event"]["error"]
            .as_str()
            .unwrap()
            .contains("read-only")
    );
    assert!(
        !application
            .windows
            .get_mut(&native_id)
            .unwrap()
            .tabs
            .as_mut()
            .unwrap()
            .tab_mut_by_id(target_tab)
            .unwrap()
            .toggle_focused_pane_read_only()
    );

    let guarded = plugin_event(
        &mut application,
        "terminal-writer",
        9,
        StructuredResponseAction::SendText {
            action_id: "guarded-input".into(),
            window_id: Some(7),
            tab_id: target_tab as u64,
            pane_id: 0,
            text: "rm -rf /".into(),
            paste: false,
            enter: true,
        },
    );
    assert_eq!(guarded["event"]["ok"], true);
    assert_eq!(guarded["event"]["disposition"], "pending_confirmation");
    let confirmation_id = guarded["event"]["confirmation_id"].as_u64().unwrap();
    let target = application.windows.get_mut(&native_id).unwrap();
    assert!(target.guardrail.is_awaiting(confirmation_id));
    assert_eq!(
        target.guardrail.matched.as_ref().unwrap().target,
        format!("GUI window 7, tab {target_tab}, pane 0")
    );
    assert_eq!(
        target.tabs.as_ref().unwrap().active_tab_id(),
        Some(untouched_tab)
    );
    target.apply_guardrail_action(crate::terminal::guardrails::GuardrailAction::Cancel {
        request_id: confirmation_id,
    });
    assert!(!target.guardrail.is_awaiting_current());
}

fn window_scoped_grants(values: &[&str]) -> PluginPermissions {
    PluginPermissions::from_permissions(values.iter().map(|value| value.parse().unwrap())).unwrap()
}

#[test]
fn plugin_window_scope_is_enforced_where_operations_execute() {
    let (origin, other, _) = pane_transfer_fixture();
    let mut application =
        Application::from_config(Config::default(), None, LaunchPlan::default()).unwrap();
    application.pending.clear();
    let origin_native = WindowId::from(2_101_u64);
    let other_native = WindowId::from(2_202_u64);
    application.windows.insert(origin_native, origin);
    application.windows.insert(other_native, other);
    application.window_order = vec![origin_native, other_native];
    application.control_window_ids.insert(origin_native, 7);
    application.control_window_ids.insert(other_native, 8);
    let window_scoped = window_scoped_grants(&["workspace-query", "navigate", "window-close"]);
    let select = |window_id| StructuredResponseAction::SelectTab {
        action_id: "select".into(),
        window_id,
        tab_id: 0,
    };

    // Omitted and explicit origin window IDs are the plugin's own window.
    for window_id in [None, Some(7)] {
        let (event, _) = plugin_operation(
            &mut application,
            "scoped",
            7,
            &window_scoped,
            select(window_id),
        );
        assert_eq!(event["event"]["ok"], true, "{event}");
        assert_eq!(event["event"]["window_id"], 7);
    }

    // Any other window needs the all-windows scope, checked before the
    // window is even looked up.
    for window_id in [8, 99] {
        let (event, _) = plugin_operation(
            &mut application,
            "scoped",
            7,
            &window_scoped,
            select(Some(window_id)),
        );
        assert_eq!(event["event"]["ok"], false);
        let error = event["event"]["error"].as_str().unwrap();
        assert!(
                error.contains(&format!(
                    "window {window_id} is not the plugin's own window; targeting it requires navigate:all-windows"
                )),
                "{error}"
            );
    }

    // Workspace queries see only the origin window at window scope.
    let query = StructuredResponseAction::QueryWorkspace {
        query_id: "windows".into(),
    };
    let (scoped_query, _) =
        plugin_operation(&mut application, "scoped", 7, &window_scoped, query.clone());
    let ids = |event: &serde_json::Value| {
        event["event"]["windows"]
            .as_array()
            .unwrap()
            .iter()
            .map(|window| window["id"].as_u64().unwrap())
            .collect::<Vec<_>>()
    };
    assert_eq!(scoped_query["origin_window_id"], 7);
    assert_eq!(ids(&scoped_query), vec![7]);
    let (all_query, _) = plugin_operation(&mut application, "all", 7, &all_windows_grants(), query);
    assert_eq!(ids(&all_query), vec![7, 8]);

    // The same actions succeed with all-windows scope.
    let (selected, _) = plugin_operation(
        &mut application,
        "all",
        7,
        &all_windows_grants(),
        select(Some(8)),
    );
    assert_eq!(selected["event"]["ok"], true);
    assert_eq!(selected["event"]["window_id"], 8);

    // Window-scoped closure closes only the origin window, and may close
    // the last one; all-windows closure never closes the last window.
    let close = |window_id| StructuredResponseAction::CloseWindow {
        action_id: "close".into(),
        window_id,
    };
    let (refused, target) = plugin_operation(
        &mut application,
        "scoped",
        7,
        &window_scoped,
        close(Some(8)),
    );
    assert_eq!(refused["event"]["ok"], false);
    assert!(target.is_none());
    assert!(
        refused["event"]["error"]
            .as_str()
            .unwrap()
            .contains("window-close:all-windows")
    );
    let (origin_close, target) =
        plugin_operation(&mut application, "scoped", 7, &window_scoped, close(None));
    assert_eq!(origin_close["event"]["ok"], true);
    assert_eq!(origin_close["event"]["scheduled"], true);
    assert_eq!(origin_close["event"]["remaining_windows"], 1);
    assert_eq!(target, Some(origin_native));

    application
        .windows
        .get_mut(&other_native)
        .unwrap()
        .should_exit = true;
    let (last_close, target) =
        plugin_operation(&mut application, "scoped", 7, &window_scoped, close(None));
    assert_eq!(last_close["event"]["ok"], true);
    assert_eq!(last_close["event"]["remaining_windows"], 0);
    assert_eq!(target, Some(origin_native));
    let (final_refused, target) = plugin_operation(
        &mut application,
        "all",
        7,
        &all_windows_grants(),
        close(None),
    );
    assert_eq!(final_refused["event"]["ok"], false);
    assert!(target.is_none());
    assert!(
        final_refused["event"]["error"]
            .as_str()
            .unwrap()
            .contains("final live window")
    );
    assert!(!application.windows[&origin_native].should_exit);
}

/// Every action that targets a window, tab, or pane, aimed at window 8,
/// with the capability whose scope governs it and its outcome once that
/// capability is granted for all windows. Windows 7 (the origin) and 8
/// each hold tab 0, split into panes 0 and 1 by divider `divider_id`, and
/// a single-pane tab 1.
fn actions_targeting_window_8(
    divider_id: u64,
) -> Vec<(
    StructuredResponseAction,
    Capability,
    std::result::Result<(), &'static str>,
)> {
    use crate::panes::SplitDirection;
    let action_id = || "scoped".to_string();
    vec![
        (
            StructuredResponseAction::SelectTab {
                action_id: action_id(),
                window_id: Some(8),
                tab_id: 1,
            },
            Capability::Navigate,
            Ok(()),
        ),
        (
            StructuredResponseAction::FocusPane {
                action_id: action_id(),
                window_id: Some(8),
                tab_id: 0,
                pane_id: 0,
            },
            Capability::Navigate,
            Ok(()),
        ),
        (
            StructuredResponseAction::QueryTerminal {
                query_id: "scoped".into(),
                window_id: Some(8),
                tab_id: 0,
                pane_id: 0,
                history_lines: 0,
                max_bytes: 64,
            },
            Capability::TerminalRead,
            Ok(()),
        ),
        (
            StructuredResponseAction::SendText {
                action_id: action_id(),
                window_id: Some(8),
                tab_id: 0,
                pane_id: 0,
                text: "scoped".into(),
                paste: false,
                enter: false,
            },
            Capability::TerminalInput,
            Ok(()),
        ),
        (
            StructuredResponseAction::CreateTab {
                action_id: action_id(),
                window_id: Some(8),
            },
            Capability::TabCreate,
            Ok(()),
        ),
        (
            StructuredResponseAction::CloseTab {
                action_id: action_id(),
                window_id: Some(8),
                tab_id: 1,
            },
            Capability::TabClose,
            Ok(()),
        ),
        (
            StructuredResponseAction::MoveTab {
                action_id: action_id(),
                source_window_id: Some(8),
                tab_id: 1,
                destination_window_id: None,
                destination_index: 0,
            },
            Capability::TabMove,
            Ok(()),
        ),
        (
            StructuredResponseAction::MoveTab {
                action_id: action_id(),
                source_window_id: None,
                tab_id: 1,
                destination_window_id: Some(8),
                destination_index: 0,
            },
            Capability::TabMove,
            Ok(()),
        ),
        (
            StructuredResponseAction::CreatePane {
                action_id: action_id(),
                window_id: Some(8),
                tab_id: 1,
                pane_id: 0,
                direction: SplitDirection::Vertical,
            },
            Capability::PaneCreate,
            Ok(()),
        ),
        (
            StructuredResponseAction::ClosePane {
                action_id: action_id(),
                window_id: Some(8),
                tab_id: 0,
                pane_id: 1,
            },
            Capability::PaneClose,
            Ok(()),
        ),
        (
            StructuredResponseAction::MovePane {
                action_id: action_id(),
                source_window_id: Some(8),
                source_tab_id: 0,
                pane_id: 1,
                destination_window_id: None,
                destination_tab_id: 0,
                target_pane_id: 0,
                direction: SplitDirection::Vertical,
            },
            Capability::PaneMove,
            Ok(()),
        ),
        (
            StructuredResponseAction::MovePane {
                action_id: action_id(),
                source_window_id: None,
                source_tab_id: 0,
                pane_id: 1,
                destination_window_id: Some(8),
                destination_tab_id: 0,
                target_pane_id: 0,
                direction: SplitDirection::Vertical,
            },
            Capability::PaneMove,
            Ok(()),
        ),
        (
            StructuredResponseAction::SwapPanes {
                action_id: action_id(),
                window_id: Some(8),
                tab_id: 0,
                first_pane_id: 0,
                second_pane_id: 1,
            },
            Capability::PaneLayout,
            Ok(()),
        ),
        (
            StructuredResponseAction::BalancePanes {
                action_id: action_id(),
                window_id: Some(8),
                tab_id: 0,
            },
            Capability::PaneLayout,
            Ok(()),
        ),
        (
            StructuredResponseAction::ApplyPaneLayout {
                action_id: action_id(),
                window_id: Some(8),
                tab_id: 0,
                preset: crate::panes::PaneLayoutPreset::Rows,
            },
            Capability::PaneLayout,
            Ok(()),
        ),
        (
            StructuredResponseAction::ResizeDivider {
                action_id: action_id(),
                window_id: Some(8),
                tab_id: 0,
                divider_id,
                ratio_millis: 400,
            },
            Capability::PaneLayout,
            Ok(()),
        ),
        (
            StructuredResponseAction::CloseWindow {
                action_id: action_id(),
                window_id: Some(8),
            },
            Capability::WindowClose,
            Ok(()),
        ),
    ]
}

/// Windows 7 and 8 as described by [`actions_targeting_window_8`], both
/// accepting external terminal input so only scope can refuse it.
fn two_window_scope_fixture() -> Application {
    let mut application =
        Application::from_config(Config::default(), None, LaunchPlan::default()).unwrap();
    application.pending.clear();
    for (native_id, control_id) in [(2_101_u64, 7), (2_202, 8)] {
        let (mut window, _, _) = pane_transfer_fixture();
        window.config.security.allow_gui_control_input = true;
        window
            .tabs
            .as_mut()
            .unwrap()
            .new_tab_with_command(
                "/bin/sh",
                &["-c".into(), "sleep 5".into()],
                None,
                Some("second"),
                None,
                std::collections::HashMap::new(),
            )
            .unwrap();
        let native_id = WindowId::from(native_id);
        application.windows.insert(native_id, window);
        application.window_order.push(native_id);
        application.control_window_ids.insert(native_id, control_id);
    }
    application
}

#[test]
fn window_scoped_grants_refuse_every_action_targeting_another_window() {
    let inventory = |application: &Application| {
        serde_json::to_value(application.plugin_process_inventory()).unwrap()
    };
    let divider_id = inventory(&two_window_scope_fixture())["windows"][1]["tabs"][0]["dividers"][0]
        ["id"]
        .as_u64()
        .unwrap();
    let window_scoped = every_capability(PermissionScope::Window);
    let all_windows = all_windows_grants();
    for (action, capability, allowed) in actions_targeting_window_8(divider_id) {
        let name = action.name();
        let mut application = two_window_scope_fixture();
        let before = inventory(&application);
        let (refused, close_target) = plugin_operation(
            &mut application,
            "scoped",
            7,
            &window_scoped,
            action.clone(),
        );
        assert_eq!(refused["event"]["ok"], false, "{name}: {refused}");
        assert_eq!(
            refused["event"]["error"],
            format!(
                "window 8 is not the plugin's own window; targeting it requires {capability}:all-windows"
            ),
            "{name}"
        );
        assert!(close_target.is_none(), "{name}");
        assert_eq!(inventory(&application), before, "{name} changed a window");

        let (result, _) = plugin_operation(&mut application, "all", 7, &all_windows, action);
        match allowed {
            Ok(()) => assert_eq!(result["event"]["ok"], true, "{name}: {result}"),
            Err(error) => {
                assert_eq!(result["event"]["ok"], false, "{name}: {result}");
                assert_eq!(result["event"]["error"], error, "{name}");
            }
        }
    }
}

#[test]
fn plugin_host_results_continue_under_the_follow_up_rule() {
    use crate::plugin::structured::HandlerAuthority;
    let mut application = two_window_scope_fixture();
    let origin = PluginOrigin {
        native_id: application.control_native_window_id(7).unwrap(),
        window_id: 7,
    };
    let grants = all_windows_grants();
    let event_continuation = StructuredHandlerInvocation {
        plugin_id: "follow-up".into(),
        program: "handler".into(),
        args: Vec::new(),
        event_json: "{}".into(),
        settings: Some(serde_json::json!({"direction": "horizontal"})),
        authority: HandlerAuthority::for_event(grants.clone(), false),
    };
    let query = StructuredResponseAction::QueryWorkspace {
        query_id: "windows".into(),
    };
    let select = StructuredResponseAction::SelectTab {
        action_id: "select".into(),
        window_id: None,
        tab_id: 1,
    };
    let notice = StructuredResponseAction::ShowMessage {
        message: "done".into(),
    };
    let continue_after =
        |application: &mut Application, operation: &StructuredResponseAction, from| {
            let (continuation, close_target) = application
                .plugin_host_operation_continuation("follow-up", origin, operation, from)
                .unwrap();
            assert!(close_target.is_none());
            continuation
        };

    // A workspace query result may still perform one host action, but not
    // another query.
    let after_query = continue_after(&mut application, &query, event_continuation.clone());
    let result: serde_json::Value = serde_json::from_str(&after_query.event_json).unwrap();
    assert_eq!(result["event"]["type"], "workspace-query-result");
    assert_eq!(after_query.authority.grants(), &grants);
    // Results carry the settings of the event that began the conversation.
    assert_eq!(after_query.settings, event_continuation.settings);
    assert!(
        after_query
            .authority
            .check(std::slice::from_ref(&select))
            .is_ok()
    );
    assert!(
        after_query
            .authority
            .check(std::slice::from_ref(&query))
            .unwrap_err()
            .contains("may perform only one host action")
    );

    // An action result ends the chain, whether the action came straight
    // from an event or from a workspace query result: it may only show UI.
    for from in [event_continuation, after_query] {
        let after_action = continue_after(&mut application, &select, from);
        let result: serde_json::Value = serde_json::from_str(&after_action.event_json).unwrap();
        assert_eq!(result["event"]["type"], "action-result");
        assert_eq!(result["event"]["ok"], true, "{result}");
        assert_eq!(
            after_action.settings,
            Some(serde_json::json!({"direction": "horizontal"}))
        );
        assert!(
            after_action
                .authority
                .check(std::slice::from_ref(&notice))
                .is_ok()
        );
        for next in [&select, &query] {
            assert!(
                after_action
                    .authority
                    .check(std::slice::from_ref(next))
                    .unwrap_err()
                    .contains("cannot start host operations"),
                "{} after an action result",
                next.name()
            );
        }
    }
}

#[test]
fn sole_default_window_retains_automatic_restore_eligibility() {
    let application =
        Application::from_config(Config::default(), None, LaunchPlan::default()).unwrap();
    assert!(application.pending[0].allow_auto_restore);
}

#[test]
fn cli_renderer_override_becomes_every_window_recovery_policy() {
    let mut application = Application::from_config(
        Config::default(),
        None,
        LaunchPlan {
            windows: vec![WindowLaunch::default(); 2],
            renderer_backend: Some(crate::config::RendererBackend::Software),
            ..LaunchPlan::default()
        },
    )
    .unwrap();
    // A window opened later in the same process asks for the same renderer.
    application
        .queue_default_window(Config::default(), None)
        .unwrap();
    assert_eq!(application.pending.len(), 3);
    assert!(application.pending.iter().all(|window| {
        window.renderer_backend() == crate::config::RendererBackend::Software
            && window.config.renderer.backend == crate::config::RendererBackend::Auto
    }));
}

#[test]
fn all_failed_native_startups_remain_an_actionable_process_error() {
    let mut application =
        Application::from_config(Config::default(), None, LaunchPlan::default()).unwrap();
    application.pending.clear();
    application.record_fatal_startup_errors(vec![
        "Failed to initialize renderer: no adapter".into(),
        "Failed to initialize renderer: device rejected".into(),
    ]);

    let error = application.take_fatal_startup_error().unwrap();
    assert!(error.contains("no adapter"));
    assert!(error.contains("device rejected"));
    assert!(application.take_fatal_startup_error().is_none());
}

#[test]
fn each_window_reports_app_ready_under_its_own_stable_id() {
    #[derive(Debug)]
    struct AppReadyObserver(std::sync::Arc<std::sync::Mutex<Vec<u64>>>);

    impl crate::plugin::hooks::Plugin for AppReadyObserver {
        fn id(&self) -> &str {
            "app-ready-observer"
        }

        fn name(&self) -> &str {
            "App Ready Observer"
        }

        fn on_hook(
            &mut self,
            hook: &crate::plugin::Hook,
            context: &crate::plugin::PluginContext,
        ) -> Option<crate::plugin::PluginAction> {
            if matches!(hook, crate::plugin::Hook::AppReady) {
                self.0.lock().unwrap().push(context.window_id);
            }
            None
        }
    }

    let mut application =
        Application::from_config(Config::default(), None, LaunchPlan::default()).unwrap();
    application.pending.clear();
    let observed = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    for _ in 0..2 {
        let mut window = App::new_headless(Config::default()).unwrap();
        window.launch.tabs[0].target = crate::cli::LaunchTarget::Command {
            program: "/bin/sh".into(),
            args: vec!["-c".into(), "exit 0".into()],
        };
        window
            .plugin_manager
            .register(Box::new(AppReadyObserver(observed.clone())))
            .unwrap();
        application.pending.push(window);
    }

    // Headless windows never gain a native window, so both starts fail
    // after app-ready and neither lends its ID to a later window.
    let results = application.start_pending_windows(|window| window.init_tabs().unwrap());
    assert!(results.iter().all(std::result::Result::is_err));
    assert_eq!(*observed.lock().unwrap(), vec![1, 2]);
    assert_eq!(application.next_control_window_id, 3);
}

#[test]
fn cli_cwds_and_terminal_profiles_are_validated_before_windows_are_queued() {
    let missing = std::path::PathBuf::from("/definitely/missing/cli-cwd");
    let mut shell_launch = LaunchPlan::default();
    shell_launch.windows[0].tabs[0].working_directory = Some(missing.clone());
    assert!(Application::from_config(Config::default(), None, shell_launch.clone()).is_err());
    shell_launch.windows[0].tabs[0].target = crate::cli::LaunchTarget::Command {
        program: "/bin/sh".into(),
        args: Vec::new(),
    };
    assert!(Application::from_config(Config::default(), None, shell_launch).is_err());

    let mut launch = LaunchPlan::default();
    launch.windows[0].tabs[0].target =
        crate::cli::LaunchTarget::TerminalProfile { name: "dev".into() };
    assert!(Application::from_config(Config::default(), None, launch.clone()).is_err());

    let mut config = Config::default();
    config.profiles.insert(
        "dev".into(),
        crate::config::LaunchProfileConfig {
            program: "/bin/sh".into(),
            cwd: Some(std::path::PathBuf::from("/definitely/missing/profile-cwd")),
            ..crate::config::LaunchProfileConfig::default()
        },
    );
    assert!(Application::from_config(config.clone(), None, launch.clone()).is_err());

    launch.windows[0].tabs[0].working_directory = Some(std::env::temp_dir());
    assert!(Application::from_config(config, None, launch).is_ok());
}

#[test]
fn non_utf8_cli_cwd_fails_before_windows_are_queued() {
    use std::os::unix::ffi::OsStringExt;

    let temp = tempfile::tempdir().unwrap();
    let path = temp
        .path()
        .join(std::ffi::OsString::from_vec(vec![b'c', b'w', b'd', 0xff]));
    std::fs::create_dir(&path).unwrap();
    let mut launch = LaunchPlan::default();
    launch.windows[0].tabs[0].working_directory = Some(path);
    let error = Application::from_config(Config::default(), None, launch)
        .err()
        .expect("non-UTF-8 cwd must fail startup validation");
    assert!(format!("{error:#}").contains("valid UTF-8"));
}

fn cli_plan(arguments: &[&str]) -> LaunchPlan {
    match crate::cli::parse(arguments.iter().map(std::ffi::OsString::from)).unwrap() {
        crate::cli::ParseOutcome::Run(plan) => plan,
        other => panic!("expected a launch plan, got {other:?}"),
    }
}

#[test]
fn automatic_restore_expands_each_remembered_window_workspace() {
    let mut config = Config::default();
    config.session.auto_restore = true;
    let state = super::super::state::AppState {
        last_workspace_ids: vec!["workspace-a".into(), "workspace-b".into()],
        ..super::super::state::AppState::default()
    };
    // Process-wide options change how the process runs, not what it opens.
    for arguments in [
        &[][..],
        &["--separate-process"],
        &["--renderer", "software"],
    ] {
        let launch = cli_plan(arguments);
        let application =
            Application::for_startup(config.clone(), None, &state, launch.clone()).unwrap();
        let workspaces: Vec<_> = application
            .pending
            .iter()
            .map(|window| window.launch.workspace.as_deref())
            .collect();
        assert_eq!(
            workspaces,
            [Some("workspace-a"), Some("workspace-b")],
            "{arguments:?}"
        );
        assert!(application.pending.iter().all(|window| {
            window.allow_workspace_fallback
                && window.renderer_backend()
                    == launch.renderer_backend.unwrap_or(config.renderer.backend)
        }));
        let plan = auto_restore_plan(&config, &state, &launch).unwrap();
        assert_eq!(plan.separate_process, launch.separate_process);
    }
    // A launch that asks for a window of its own opens just that.
    let application =
        Application::for_startup(config.clone(), None, &state, cli_plan(&["--new-window"]))
            .unwrap();
    assert_eq!(application.pending.len(), 1);
    assert!(application.pending[0].launch.workspace.is_none());
    assert!(!application.pending[0].allow_workspace_fallback);
}

#[test]
fn a_broken_automatic_workspace_opens_a_shell_in_a_separate_process() {
    let temp = tempfile::tempdir().unwrap();
    crate::workspace::set_test_workspaces_base_dir(Some(temp.path().join("workspaces")));
    super::super::state::AppState::set_test_state_path(Some(temp.path().join("state.json")));
    let outcome = std::panic::catch_unwind(|| {
        let workspace_id = "0123456789abcdef0123456789abcdef";
        let archive = crate::workspace::workspace_dir(workspace_id).unwrap();
        std::fs::create_dir_all(&archive).unwrap();
        std::fs::write(archive.join("workspace.toml"), "not an archive").unwrap();
        let mut state = super::super::state::AppState::default();
        state.remember_workspace(workspace_id).unwrap();
        let mut config = Config::default();
        config.session.auto_restore = true;
        config.shell.program = Some("/bin/sh".into());
        config.shell.args = vec!["-c".into(), "sleep 5".into()];

        let mut application =
            Application::for_startup(config, None, &state, cli_plan(&["--separate-process"]))
                .unwrap();
        let window = &mut application.pending[0];
        window.automatic_persistence = false;
        window.init_tabs().unwrap();
        assert_eq!(window.tabs.as_ref().unwrap().count(), 1, "a shell opened");
        assert!(
            super::super::state::AppState::load()
                .last_workspace_ids
                .is_empty(),
            "the broken archive is no longer restored"
        );
    });
    crate::workspace::set_test_workspaces_base_dir(None);
    super::super::state::AppState::set_test_state_path(None);
    if let Err(panic) = outcome {
        std::panic::resume_unwind(panic);
    }
}

#[test]
fn queued_domain_window_preserves_remote_authority_and_disables_auto_restore() {
    let mut application =
        Application::from_config(Config::default(), None, LaunchPlan::default()).unwrap();
    application.pending.clear();
    let target = crate::domain::DomainTarget {
        authority: crate::domain::DomainAuthority::Ssh {
            destination: "developer@build.example".into(),
        },
        domain_id: "domain-7".into(),
    };
    application
        .queue_window(
            Config::default(),
            WindowLaunch {
                force_new: true,
                domain: Some(target.clone()),
                ..WindowLaunch::default()
            },
        )
        .unwrap();
    assert_eq!(application.pending.len(), 1);
    assert_eq!(application.pending[0].launch.domain, Some(target));
    assert!(!application.pending[0].allow_auto_restore);
}

fn pane_transfer_fixture() -> (App, App, Option<u32>) {
    use crate::config::ShellConfig;
    use crate::panes::SplitDirection;
    use crate::tabs::TabManager;
    use std::collections::HashMap;

    let shell = ShellConfig {
        program: Some("/bin/sh".into()),
        args: vec!["-c".into(), "sleep 5".into()],
        env: Default::default(),
    };
    let mut source_tabs = TabManager::new(80, 24, shell.clone(), 1_000, 1024 * 1024);
    source_tabs
        .new_tab_with_command(
            "/bin/sh",
            &["-c".into(), "sleep 5".into()],
            None,
            Some("source"),
            None,
            HashMap::new(),
        )
        .unwrap();
    let source_panes = source_tabs.active_tab_mut().unwrap().panes_for_test(
        shell.clone(),
        800.0,
        480.0,
        10.0,
        20.0,
    );
    let moved = source_panes.split(SplitDirection::Vertical).unwrap();
    source_panes.get_pane_mut(moved).unwrap().title = Some("moved-owner".into());
    let moved_child = source_panes.get_pane(moved).unwrap().pty.child_id();

    let mut destination_tabs = TabManager::new(100, 30, shell, 1_000, 1024 * 1024);
    destination_tabs
        .new_tab_with_command(
            "/bin/sh",
            &["-c".into(), "sleep 5".into()],
            None,
            Some("destination"),
            None,
            HashMap::new(),
        )
        .unwrap();

    let mut source = App::new_headless(Config::default()).unwrap();
    source.tabs = Some(source_tabs);
    let mut destination = App::new_headless(Config::default()).unwrap();
    destination.tabs = Some(destination_tabs);
    (source, destination, moved_child)
}

#[test]
fn cross_window_transfer_preserves_live_owner_and_retargets_destination_geometry() {
    let (mut source, mut destination, moved_child) = pane_transfer_fixture();

    let attached = Application::transfer_focused_pane(&mut source, &mut destination).unwrap();
    let source_manager = source.active_panes();
    assert_eq!(source_manager.pane_count(), 1);

    let destination_manager = destination.active_panes();
    let pane = destination_manager.get_pane(attached).unwrap();
    assert_eq!(destination_manager.pane_count(), 2);
    assert_eq!(destination_manager.focused_id(), attached);
    assert_eq!(destination_manager.spawned_shell_count(), 0);
    assert_eq!(pane.pty.child_id(), moved_child);
    assert_eq!(pane.title.as_deref(), Some("moved-owner"));
    assert_eq!(pane.geometry.cols, 48);
    assert_eq!(pane.geometry.rows, 30);
    source
        .tabs
        .as_ref()
        .unwrap()
        .active_tab()
        .unwrap()
        .capture_workspace_tab(0, false)
        .unwrap()
        .validate()
        .unwrap();
    destination
        .tabs
        .as_ref()
        .unwrap()
        .active_tab()
        .unwrap()
        .capture_workspace_tab(0, false)
        .unwrap()
        .validate()
        .unwrap();
}

#[test]
fn exact_control_pane_transfer_validates_both_paths_before_detaching() {
    let (mut source, mut destination, moved_child) = pane_transfer_fixture();
    let error = Application::transfer_control_pane(
        &mut source,
        &mut destination,
        0,
        1,
        0,
        99,
        crate::panes::SplitDirection::Vertical,
    )
    .unwrap_err();
    assert!(error.contains("does not exist"), "{error}");
    assert_eq!(source.active_panes().pane_count(), 2);
    assert!(
        destination
            .tabs
            .as_ref()
            .unwrap()
            .active_tab()
            .unwrap()
            .panes
            .pane_count()
            == 1
    );

    let destination_id = Application::transfer_control_pane(
        &mut source,
        &mut destination,
        0,
        1,
        0,
        0,
        crate::panes::SplitDirection::Horizontal,
    )
    .unwrap();
    assert_eq!(destination_id, 1);
    let destination_manager = destination.active_panes();
    assert_eq!(destination_manager.pane_count(), 2);
    assert_eq!(destination_manager.focused_id().0, destination_id);
    assert_eq!(
        destination_manager
            .get_pane(crate::panes::PaneId(destination_id))
            .unwrap()
            .pty
            .child_id(),
        moved_child
    );
}

#[test]
fn plugin_pane_movement_routes_exactly_across_windows_and_is_atomic() {
    let (source, destination, moved_child) = pane_transfer_fixture();
    let mut application =
        Application::from_config(Config::default(), None, LaunchPlan::default()).unwrap();
    application.pending.clear();
    let source_native = WindowId::from(101_u64);
    let destination_native = WindowId::from(202_u64);
    application.windows.insert(source_native, source);
    application.windows.insert(destination_native, destination);
    application.window_order = vec![source_native, destination_native];
    application.control_window_ids.insert(source_native, 7);
    application.control_window_ids.insert(destination_native, 8);

    let refused = plugin_event(
        &mut application,
        "pane-mover",
        7,
        crate::plugin::structured::StructuredResponseAction::MovePane {
            action_id: "stale-target".into(),
            source_window_id: Some(7),
            source_tab_id: 0,
            pane_id: 1,
            destination_window_id: Some(8),
            destination_tab_id: 0,
            target_pane_id: 99,
            direction: crate::panes::SplitDirection::Vertical,
        },
    );
    assert_eq!(refused["event"]["ok"], false);
    assert!(refused["event"]["destination_pane_id"].is_null());
    assert!(
        refused["event"]["error"]
            .as_str()
            .unwrap()
            .contains("does not exist")
    );
    assert_eq!(
        application.windows[&source_native]
            .tabs
            .as_ref()
            .unwrap()
            .active_tab()
            .unwrap()
            .panes
            .pane_count(),
        2
    );

    let moved = plugin_event(
        &mut application,
        "pane-mover",
        7,
        crate::plugin::structured::StructuredResponseAction::MovePane {
            action_id: "move-sibling-task".into(),
            source_window_id: Some(7),
            source_tab_id: 0,
            pane_id: 1,
            destination_window_id: Some(8),
            destination_tab_id: 0,
            target_pane_id: 0,
            direction: crate::panes::SplitDirection::Horizontal,
        },
    );
    assert_eq!(moved["plugin_id"], "pane-mover");
    assert_eq!(moved["event"]["type"], "action-result");
    assert_eq!(moved["event"]["action"], "move-pane");
    assert_eq!(moved["event"]["action_id"], "move-sibling-task");
    assert_eq!(moved["event"]["ok"], true);
    assert_eq!(moved["origin_window_id"], 7);
    assert_eq!(moved["event"]["source_window_id"], 7);
    assert_eq!(moved["event"]["source_tab_id"], 0);
    assert_eq!(moved["event"]["pane_id"], 1);
    assert_eq!(moved["event"]["destination_window_id"], 8);
    assert_eq!(moved["event"]["destination_tab_id"], 0);
    assert_eq!(moved["event"]["target_pane_id"], 0);
    assert_eq!(moved["event"]["direction"], "horizontal");
    let destination_pane_id = moved["event"]["destination_pane_id"].as_u64().unwrap() as u32;
    assert_eq!(
        application.windows[&source_native]
            .tabs
            .as_ref()
            .unwrap()
            .active_tab()
            .unwrap()
            .panes
            .pane_count(),
        1
    );
    let destination_manager = &application.windows[&destination_native]
        .tabs
        .as_ref()
        .unwrap()
        .active_tab()
        .unwrap()
        .panes;
    assert_eq!(destination_manager.pane_count(), 2);
    assert_eq!(destination_manager.focused_id().0, destination_pane_id);
    assert_eq!(
        destination_manager
            .get_pane(crate::panes::PaneId(destination_pane_id))
            .unwrap()
            .pty
            .child_id(),
        moved_child
    );
    assert!(application.windows[&source_native].window.is_none());
    assert!(application.windows[&destination_native].window.is_none());
    assert_eq!(
        application.window_order,
        vec![source_native, destination_native]
    );
    assert!(
        moved.to_string().len() <= crate::plugin::structured::MAX_PERSISTENT_SOURCE_EVENT_BYTES
    );
}

#[test]
fn cross_window_transfer_rejects_live_endpoints_without_detaching_source() {
    use crate::tabs::LiveDomainTab;
    use std::collections::HashMap;

    let (mut source, mut destination, moved_child) = pane_transfer_fixture();
    destination
        .tabs
        .as_mut()
        .unwrap()
        .active_tab_mut()
        .unwrap()
        .live_domain = Some(LiveDomainTab {
        domain_id: "remote".into(),
        tab_id: 7,
        pane_ids: HashMap::from([(crate::panes::PaneId(0), 9)]),
    });

    let error = Application::transfer_focused_pane(&mut source, &mut destination)
        .unwrap_err()
        .to_string();
    assert!(error.contains("live-domain tab"), "{error}");
    let source_manager = source.active_panes();
    assert_eq!(source_manager.pane_count(), 2);
    assert!(
        source_manager
            .iter_with_layout()
            .any(|(pane, _)| pane.pty.child_id() == moved_child)
    );
    assert!(
        destination
            .tabs
            .as_ref()
            .unwrap()
            .active_tab()
            .unwrap()
            .panes
            .pane_count()
            == 1
    );

    let (mut live_source, mut ordinary_destination, moved_child) = pane_transfer_fixture();
    let source_tab = live_source.tabs.as_mut().unwrap().active_tab_mut().unwrap();
    let pane_ids = source_tab
        .panes
        .pane_ids()
        .into_iter()
        .enumerate()
        .map(|(index, id)| (id, u32::try_from(index + 1).unwrap()))
        .collect();
    source_tab.live_domain = Some(LiveDomainTab {
        domain_id: "remote".into(),
        tab_id: 8,
        pane_ids,
    });
    let error = Application::transfer_focused_pane(&mut live_source, &mut ordinary_destination)
        .unwrap_err()
        .to_string();
    assert!(error.contains("daemon-owned"), "{error}");
    assert!(
        live_source
            .active_panes()
            .iter_with_layout()
            .any(|(pane, _)| pane.pty.child_id() == moved_child)
    );
    assert!(
        ordinary_destination
            .tabs
            .as_ref()
            .unwrap()
            .active_tab()
            .unwrap()
            .panes
            .pane_count()
            == 1
    );

    let source_tab = live_source.tabs.as_mut().unwrap().active_tab_mut().unwrap();
    source_tab.live_domain = None;
    let focused = source_tab.panes.focused_pane_mut().unwrap();
    focused.launch = crate::workspace::WorkspaceLaunch::Profile { name: "dev".into() };
    let attached =
        Application::transfer_focused_pane(&mut live_source, &mut ordinary_destination).unwrap();
    let snapshot = ordinary_destination
        .tabs
        .as_ref()
        .unwrap()
        .active_tab()
        .unwrap()
        .capture_workspace_tab(0, false)
        .unwrap();
    assert_eq!(
        snapshot
            .panes
            .iter()
            .find(|pane| pane.id == attached.0)
            .map(|pane| &pane.launch),
        Some(&crate::workspace::WorkspaceLaunch::Profile { name: "dev".into() })
    );

    let (mut plugin_source, mut plugin_destination, _) = pane_transfer_fixture();
    let source_tab = plugin_source
        .tabs
        .as_mut()
        .unwrap()
        .active_tab_mut()
        .unwrap();
    source_tab.tab_type = crate::tabs::TabType::Plugin {
        plugin_id: "fake".into(),
    };
    source_tab.panes.focused_pane_mut().unwrap().launch =
        crate::workspace::WorkspaceLaunch::Plugin {
            plugin_id: "fake".into(),
            profile: None,
        };
    let moved =
        Application::transfer_focused_pane(&mut plugin_source, &mut plugin_destination).unwrap();
    assert_eq!(
        plugin_destination
            .tabs
            .as_ref()
            .unwrap()
            .active_tab()
            .unwrap()
            .pane_plugin_id(moved),
        Some("fake")
    );
    assert_eq!(plugin_source.active_panes().pane_count(), 1);
    assert!(
        plugin_destination
            .tabs
            .as_ref()
            .unwrap()
            .active_tab()
            .unwrap()
            .panes
            .pane_count()
            > 1
    );
}

#[test]
fn whole_tab_transfer_preserves_plugin_panes_pinning_and_closes_empty_source() {
    let (mut source, mut destination, moved_child) = pane_transfer_fixture();
    let source_tab = source.tabs.as_mut().unwrap().active_tab_mut().unwrap();
    source_tab.tab_type = crate::tabs::TabType::Plugin {
        plugin_id: "fake-ai".into(),
    };
    source_tab.pinned = true;
    source_tab.set_user_title(Some("Pinned fake session".into()));

    let transferred = Application::transfer_active_tab(&mut source, &mut destination).unwrap();
    assert!(source.tabs.as_ref().unwrap().is_empty());
    assert!(source.should_exit);
    let tabs = destination.tabs.as_ref().unwrap();
    assert_eq!(tabs.count(), 2);
    assert_eq!(tabs.active_index(), 0);
    let tab = tabs.active_tab().unwrap();
    assert_eq!(tab.id, transferred);
    assert!(tab.pinned);
    assert_eq!(tab.user_title(), Some("Pinned fake session"));
    assert!(matches!(
        &tab.tab_type,
        crate::tabs::TabType::Plugin { plugin_id } if plugin_id == "fake-ai"
    ));
    let panes = &tab.panes;
    assert_eq!(panes.pane_count(), 2);
    assert!(
        panes
            .iter_with_layout()
            .any(|(pane, _)| pane.pty.child_id() == moved_child)
    );
    assert!(
        panes
            .iter_with_layout()
            .all(|(pane, _)| pane.geometry.rows == 30)
    );
    assert!(!tabs.iter().nth(1).unwrap().1.pinned);

    drop((source, destination));
    let (mut source, mut destination, _) = pane_transfer_fixture();
    source
        .tabs
        .as_mut()
        .unwrap()
        .new_tab_with_command(
            "/bin/sh",
            &["-c".into(), "sleep 5".into()],
            None,
            Some("survivor"),
            None,
            std::collections::HashMap::new(),
        )
        .unwrap();
    source.tabs.as_mut().unwrap().switch_to(0);
    Application::transfer_active_tab(&mut source, &mut destination).unwrap();
    assert!(!source.should_exit);
    assert_eq!(source.tabs.as_ref().unwrap().count(), 1);
    assert_eq!(
        source.tabs.as_ref().unwrap().active_tab().unwrap().title,
        "survivor"
    );
}

#[test]
fn a_moved_tab_takes_its_agent_view_conversation_to_the_next_window() {
    use crate::agent_view::event::{AgentEvent, Handshake};
    use crate::agent_view::session::AgentSession;
    use crate::agent_view::transcript::Transcript;
    let (mut source, mut destination, _) = pane_transfer_fixture();
    let mut transcript = Transcript::new("Claude Code", "default");
    transcript.apply(AgentEvent::Ready(Handshake::default()));
    transcript.push_user("fix the build".into(), Vec::new());
    let session = AgentSession::detached(std::env::temp_dir(), transcript);
    let id = session.id();
    source.agent_view.sessions.push(session);
    source.open_agent_view(true);
    source.agent_view.state.show(id);

    let moved = Application::transfer_active_tab(&mut source, &mut destination).unwrap();
    source.reconcile_agent_view_tabs();
    assert!(source.agent_view.session(id).is_none());
    assert!(destination.agent_view.session(id).is_some());
    assert_eq!(destination.agent_session_tab(id), Some(moved));
}

#[test]
fn exact_control_tab_transfer_is_atomic_and_returns_destination_identity() {
    let (mut source, mut destination, moved_child) = pane_transfer_fixture();
    let error = Application::transfer_control_tab(&mut source, &mut destination, 0, 2).unwrap_err();
    assert!(error.contains("outside"), "{error}");
    assert_eq!(source.tabs.as_ref().unwrap().count(), 1);
    assert_eq!(destination.tabs.as_ref().unwrap().count(), 1);

    let (destination_tab_id, source_closing) =
        Application::transfer_control_tab(&mut source, &mut destination, 0, 1).unwrap();
    assert_eq!(destination_tab_id, 1);
    assert!(source_closing);
    assert!(source.should_exit);
    assert!(source.tabs.as_ref().unwrap().is_empty());
    let tabs = destination.tabs.as_ref().unwrap();
    assert_eq!(tabs.count(), 2);
    assert_eq!(tabs.active_index(), 1);
    let moved = tabs.active_tab().unwrap();
    assert_eq!(moved.id, destination_tab_id);
    assert!(
        moved
            .panes
            .iter_with_layout()
            .any(|(pane, _)| pane.pty.child_id() == moved_child)
    );
}

#[test]
fn plugin_tab_movement_preserves_state_and_refuses_final_source_tab() {
    let (source, destination, moved_child) = pane_transfer_fixture();
    let mut application =
        Application::from_config(Config::default(), None, LaunchPlan::default()).unwrap();
    application.pending.clear();
    let source_native = WindowId::from(303_u64);
    let destination_native = WindowId::from(404_u64);
    application.windows.insert(source_native, source);
    application.windows.insert(destination_native, destination);
    application.window_order = vec![source_native, destination_native];
    application.control_window_ids.insert(source_native, 7);
    application.control_window_ids.insert(destination_native, 8);
    let observed_switches = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    for window in application.windows.values_mut() {
        window.report_active_tab_switch();
        window
            .plugin_manager
            .register(crate::test_support::hook_counter(
                |hook| matches!(hook, crate::plugin::Hook::TabSwitched { .. }),
                &observed_switches,
            ))
            .unwrap();
    }

    let action = || crate::plugin::structured::StructuredResponseAction::MoveTab {
        action_id: "move-sibling-tab".into(),
        source_window_id: Some(7),
        tab_id: 0,
        destination_window_id: Some(8),
        destination_index: 1,
    };
    let refused = plugin_event(&mut application, "tab-mover", 7, action());
    assert_eq!(refused["event"]["ok"], false);
    assert!(refused["event"]["destination_tab_id"].is_null());
    assert!(
        refused["event"]["error"]
            .as_str()
            .unwrap()
            .contains("final non-closing tab")
    );
    assert_eq!(
        application.windows[&source_native]
            .tabs
            .as_ref()
            .unwrap()
            .count(),
        1
    );
    assert_eq!(
        application.windows[&destination_native]
            .tabs
            .as_ref()
            .unwrap()
            .count(),
        1
    );

    application
        .windows
        .get_mut(&source_native)
        .unwrap()
        .tabs
        .as_mut()
        .unwrap()
        .new_tab_with_command(
            "/bin/sh",
            &["-c".into(), "sleep 5".into()],
            None,
            Some("survivor"),
            None,
            std::collections::HashMap::new(),
        )
        .unwrap();
    let reordered = plugin_event(
        &mut application,
        "tab-mover",
        7,
        crate::plugin::structured::StructuredResponseAction::MoveTab {
            action_id: "reorder-source-tab".into(),
            source_window_id: Some(7),
            tab_id: 0,
            destination_window_id: Some(7),
            destination_index: 1,
        },
    );
    assert_eq!(reordered["event"]["ok"], true);
    assert_eq!(reordered["event"]["destination_tab_id"], 0);
    assert_eq!(reordered["event"]["changed"], true);
    assert_eq!(reordered["event"]["source_window_closing"], false);
    let moved = plugin_event(&mut application, "tab-mover", 7, action());
    assert_eq!(moved["plugin_id"], "tab-mover");
    assert_eq!(moved["event"]["action"], "move-tab");
    assert_eq!(moved["event"]["ok"], true);
    assert_eq!(moved["event"]["tab_id"], 0);
    assert_eq!(moved["event"]["destination_window_id"], 8);
    assert_eq!(moved["event"]["destination_tab_id"], 1);
    assert_eq!(moved["event"]["destination_index"], 1);
    assert_eq!(moved["event"]["source_window_closing"], false);
    assert_eq!(moved["event"]["changed"], true);
    assert_eq!(
        application.windows[&source_native]
            .tabs
            .as_ref()
            .unwrap()
            .count(),
        1
    );
    assert!(!application.windows[&source_native].should_exit);
    // The move activated the tab in its new window, and the plugin hears of
    // that only through its result. The source's new tab was activated
    // outside the plugin, before its first move, and was reported then.
    for window in application.windows.values_mut() {
        window.report_active_tab_switch();
    }
    assert_eq!(
        observed_switches.load(std::sync::atomic::Ordering::SeqCst),
        1
    );
    let destination_tabs = application.windows[&destination_native]
        .tabs
        .as_ref()
        .unwrap();
    assert_eq!(destination_tabs.count(), 2);
    assert_eq!(destination_tabs.active_tab_id(), Some(1));
    let transferred = destination_tabs.tab_by_id(1).unwrap();
    assert!(
        transferred
            .panes
            .iter_with_layout()
            .any(|(pane, _)| pane.pty.child_id() == moved_child)
    );
    assert_eq!(
        application.window_order,
        vec![source_native, destination_native]
    );
    assert!(
        moved.to_string().len() <= crate::plugin::structured::MAX_PERSISTENT_SOURCE_EVENT_BYTES
    );
}

#[test]
fn plugin_pane_closure_targets_exact_same_or_sibling_window_and_preserves_tab() {
    let (target, origin, _) = pane_transfer_fixture();
    let mut application =
        Application::from_config(Config::default(), None, LaunchPlan::default()).unwrap();
    application.pending.clear();
    let target_native = WindowId::from(505_u64);
    let origin_native = WindowId::from(606_u64);
    application.windows.insert(target_native, target);
    application.windows.insert(origin_native, origin);
    application.window_order = vec![target_native, origin_native];
    application.control_window_ids.insert(target_native, 8);
    application.control_window_ids.insert(origin_native, 7);

    let target_tab_id = application.windows[&target_native]
        .tabs
        .as_ref()
        .unwrap()
        .active_tab()
        .unwrap()
        .id as u64;
    let target_pane_id = application.windows[&target_native]
        .tabs
        .as_ref()
        .unwrap()
        .active_tab()
        .unwrap()
        .panes
        .focused_id()
        .0;
    let stale = plugin_event(
        &mut application,
        "pane-pruner",
        7,
        crate::plugin::structured::StructuredResponseAction::ClosePane {
            action_id: "stale-pane".into(),
            window_id: Some(8),
            tab_id: target_tab_id,
            pane_id: u32::MAX,
        },
    );
    assert_eq!(stale["event"]["ok"], false);
    assert!(
        stale["event"]["error"]
            .as_str()
            .unwrap()
            .contains("does not exist")
    );

    let closed = plugin_event(
        &mut application,
        "pane-pruner",
        7,
        crate::plugin::structured::StructuredResponseAction::ClosePane {
            action_id: "close-sibling-pane".into(),
            window_id: Some(8),
            tab_id: target_tab_id,
            pane_id: target_pane_id,
        },
    );
    assert_eq!(closed["plugin_id"], "pane-pruner");
    assert_eq!(closed["event"]["action"], "close-pane");
    assert_eq!(closed["event"]["ok"], true);
    assert_eq!(closed["origin_window_id"], 7);
    assert_eq!(closed["event"]["window_id"], 8);
    assert_eq!(closed["event"]["tab_id"], target_tab_id);
    assert_eq!(closed["event"]["pane_id"], target_pane_id);
    assert_eq!(closed["event"]["remaining_panes"], 1);
    assert!(!closed["event"]["focused_pane_id"].is_null());
    let target_tabs = application.windows[&target_native].tabs.as_ref().unwrap();
    assert_eq!(target_tabs.count(), 1);
    assert_eq!(target_tabs.active_tab().unwrap().id as u64, target_tab_id);
    assert!(!application.windows[&target_native].should_exit);
    assert!(!application.windows[&origin_native].should_exit);

    let sole_pane_id = closed["event"]["focused_pane_id"].as_u64().unwrap() as u32;
    let refused = plugin_event(
        &mut application,
        "pane-pruner",
        8,
        crate::plugin::structured::StructuredResponseAction::ClosePane {
            action_id: "refuse-final-pane".into(),
            window_id: Some(8),
            tab_id: target_tab_id,
            pane_id: sole_pane_id,
        },
    );
    assert_eq!(refused["event"]["ok"], false);
    assert_eq!(refused["origin_window_id"], 8);
    assert!(refused["event"]["remaining_panes"].is_null());
    assert!(
        refused["event"]["error"]
            .as_str()
            .unwrap()
            .contains("sole pane")
    );
    let target_tab = application.windows[&target_native]
        .tabs
        .as_ref()
        .unwrap()
        .active_tab()
        .unwrap();
    assert_eq!(target_tab.panes.pane_count(), 1);

    assert!(
        refused.to_string().len() <= crate::plugin::structured::MAX_PERSISTENT_SOURCE_EVENT_BYTES
    );
}

#[test]
fn plugin_tab_closure_targets_exact_window_and_preserves_final_tab() {
    let (mut target, origin, _) = pane_transfer_fixture();
    let target_tab_id = target.tabs.as_ref().unwrap().active_tab().unwrap().id;
    let survivor_id = target
        .tabs
        .as_mut()
        .unwrap()
        .new_tab_with_command(
            "/bin/sh",
            &["-c".into(), "sleep 5".into()],
            None,
            Some("survivor"),
            None,
            std::collections::HashMap::new(),
        )
        .unwrap();
    let mut application =
        Application::from_config(Config::default(), None, LaunchPlan::default()).unwrap();
    application.pending.clear();
    let target_native = WindowId::from(707_u64);
    let origin_native = WindowId::from(808_u64);
    application.windows.insert(target_native, target);
    application.windows.insert(origin_native, origin);
    application.window_order = vec![target_native, origin_native];
    application.control_window_ids.insert(target_native, 8);
    application.control_window_ids.insert(origin_native, 7);

    let stale = plugin_event(
        &mut application,
        "tab-pruner",
        7,
        crate::plugin::structured::StructuredResponseAction::CloseTab {
            action_id: "stale-tab".into(),
            window_id: Some(8),
            tab_id: u64::MAX,
        },
    );
    assert_eq!(stale["event"]["ok"], false);
    assert!(
        stale["event"]["error"]
            .as_str()
            .unwrap()
            .contains("does not exist")
    );

    let closed = plugin_event(
        &mut application,
        "tab-pruner",
        7,
        crate::plugin::structured::StructuredResponseAction::CloseTab {
            action_id: "close-sibling-tab".into(),
            window_id: Some(8),
            tab_id: target_tab_id as u64,
        },
    );
    assert_eq!(closed["plugin_id"], "tab-pruner");
    assert_eq!(closed["event"]["action"], "close-tab");
    assert_eq!(closed["event"]["ok"], true);
    assert_eq!(closed["origin_window_id"], 7);
    assert_eq!(closed["event"]["window_id"], 8);
    assert_eq!(closed["event"]["tab_id"], target_tab_id);
    assert_eq!(closed["event"]["remaining_tabs"], 1);
    assert_eq!(closed["event"]["active_tab_id"], survivor_id);
    assert_eq!(closed["event"]["active_changed"], false);
    let tabs = application.windows[&target_native].tabs.as_ref().unwrap();
    assert!(tabs.tab_by_id(target_tab_id).unwrap().closing_at.is_some());
    assert_eq!(tabs.active_tab_id(), Some(survivor_id));
    assert!(!application.windows[&target_native].should_exit);

    let refused = plugin_event(
        &mut application,
        "tab-pruner",
        8,
        crate::plugin::structured::StructuredResponseAction::CloseTab {
            action_id: "refuse-final-tab".into(),
            window_id: Some(8),
            tab_id: survivor_id as u64,
        },
    );
    assert_eq!(refused["event"]["ok"], false);
    assert_eq!(refused["origin_window_id"], 8);
    assert!(refused["event"]["remaining_tabs"].is_null());
    assert!(
        refused["event"]["error"]
            .as_str()
            .unwrap()
            .contains("final non-closing tab")
    );
    assert!(!application.windows[&target_native].should_exit);
    assert!(
        refused.to_string().len() <= crate::plugin::structured::MAX_PERSISTENT_SOURCE_EVENT_BYTES
    );
}

#[test]
fn plugin_pane_layout_targets_exact_window_and_preserves_identity_and_focus() {
    let (target, origin, _) = pane_transfer_fixture();
    let mut application =
        Application::from_config(Config::default(), None, LaunchPlan::default()).unwrap();
    application.pending.clear();
    let target_native = WindowId::from(909_u64);
    let origin_native = WindowId::from(1_010_u64);
    application.windows.insert(target_native, target);
    application.windows.insert(origin_native, origin);
    application.window_order = vec![target_native, origin_native];
    application.control_window_ids.insert(target_native, 8);
    application.control_window_ids.insert(origin_native, 7);

    let target_tab_id = application.windows[&target_native]
        .tabs
        .as_ref()
        .unwrap()
        .active_tab_id()
        .unwrap();
    let (initial_order, initial_focus) = {
        let manager = &application.windows[&target_native]
            .tabs
            .as_ref()
            .unwrap()
            .tab_by_id(target_tab_id)
            .unwrap()
            .panes;
        (
            manager
                .pane_ids()
                .into_iter()
                .map(|pane_id| pane_id.0)
                .collect::<Vec<_>>(),
            manager.focused_id().0,
        )
    };
    assert_eq!(initial_order.len(), 2);

    let stale = plugin_event(
        &mut application,
        "pane-arranger",
        7,
        crate::plugin::structured::StructuredResponseAction::BalancePanes {
            action_id: "stale-window".into(),
            window_id: Some(99),
            tab_id: target_tab_id as u64,
        },
    );
    assert_eq!(stale["event"]["ok"], false);
    assert!(stale["event"]["error"].as_str().unwrap().contains("99"));

    let swapped = plugin_event(
        &mut application,
        "pane-arranger",
        7,
        crate::plugin::structured::StructuredResponseAction::SwapPanes {
            action_id: "swap-sibling-panes".into(),
            window_id: Some(8),
            tab_id: target_tab_id as u64,
            first_pane_id: initial_order[0],
            second_pane_id: initial_order[1],
        },
    );
    assert_eq!(swapped["plugin_id"], "pane-arranger");
    assert_eq!(swapped["event"]["action"], "swap-panes");
    assert_eq!(swapped["event"]["ok"], true);
    assert_eq!(swapped["origin_window_id"], 7);
    assert_eq!(swapped["event"]["window_id"], 8);
    assert_eq!(swapped["event"]["tab_id"], target_tab_id);
    assert_eq!(swapped["event"]["focused_pane_id"], initial_focus);
    assert_eq!(
        swapped["event"]["pane_order"],
        serde_json::json!([initial_order[1], initial_order[0]])
    );
    assert_eq!(
        application.windows[&target_native]
            .tabs
            .as_ref()
            .unwrap()
            .active_tab_id(),
        Some(target_tab_id)
    );

    let balanced = plugin_event(
        &mut application,
        "pane-arranger",
        8,
        crate::plugin::structured::StructuredResponseAction::BalancePanes {
            action_id: "balance-current-panes".into(),
            window_id: Some(8),
            tab_id: target_tab_id as u64,
        },
    );
    assert_eq!(balanced["event"]["action"], "balance-panes");
    assert_eq!(balanced["event"]["ok"], true);
    assert_eq!(balanced["origin_window_id"], 8);
    assert_eq!(balanced["event"]["focused_pane_id"], initial_focus);

    let applied = plugin_event(
        &mut application,
        "pane-arranger",
        7,
        crate::plugin::structured::StructuredResponseAction::ApplyPaneLayout {
            action_id: "fat-sibling-panes".into(),
            window_id: Some(8),
            tab_id: target_tab_id as u64,
            preset: crate::panes::PaneLayoutPreset::Fat,
        },
    );
    assert_eq!(applied["event"]["action"], "apply-pane-layout");
    assert_eq!(applied["event"]["preset"], "fat");
    assert_eq!(applied["event"]["ok"], true);
    assert_eq!(applied["event"]["focused_pane_id"], initial_focus);
    let target_manager = &application.windows[&target_native]
        .tabs
        .as_ref()
        .unwrap()
        .tab_by_id(target_tab_id)
        .unwrap()
        .panes;
    let target_layout = target_manager.calculate_layout();
    let detail = target_manager
        .pane_ids()
        .into_iter()
        .find(|pane| pane.0 != initial_focus)
        .unwrap();
    assert!(target_layout[&detail].y > target_layout[&crate::panes::PaneId(initial_focus)].y);
    assert_eq!(
        application.windows[&target_native]
            .tabs
            .as_ref()
            .unwrap()
            .active_tab_id(),
        Some(target_tab_id)
    );

    let origin_tab_id = application.windows[&origin_native]
        .tabs
        .as_ref()
        .unwrap()
        .active_tab_id()
        .unwrap();
    let unsplit = plugin_event(
        &mut application,
        "pane-arranger",
        7,
        crate::plugin::structured::StructuredResponseAction::BalancePanes {
            action_id: "refuse-unsplit".into(),
            window_id: Some(7),
            tab_id: origin_tab_id as u64,
        },
    );
    assert_eq!(unsplit["event"]["ok"], false);
    assert!(
        unsplit["event"]["error"]
            .as_str()
            .unwrap()
            .contains("not split")
    );
    assert!(!application.windows[&target_native].should_exit);
    assert!(!application.windows[&origin_native].should_exit);
    assert!(
        unsplit.to_string().len() <= crate::plugin::structured::MAX_PERSISTENT_SOURCE_EVENT_BYTES
    );
}

#[test]
fn plugin_window_closure_is_exact_non_final_and_result_gated() {
    let (target, origin, _) = pane_transfer_fixture();
    let mut application =
        Application::from_config(Config::default(), None, LaunchPlan::default()).unwrap();
    application.pending.clear();
    let target_native = WindowId::from(1_111_u64);
    let origin_native = WindowId::from(1_212_u64);
    application.windows.insert(target_native, target);
    application.windows.insert(origin_native, origin);
    application.window_order = vec![target_native, origin_native];
    application.control_window_ids.insert(target_native, 8);
    application.control_window_ids.insert(origin_native, 7);

    let stale = plugin_event(
        &mut application,
        "window-closer",
        7,
        crate::plugin::structured::StructuredResponseAction::CloseWindow {
            action_id: "stale-window".into(),
            window_id: Some(99),
        },
    );
    assert_eq!(stale["event"]["ok"], false);
    assert!(stale["event"]["scheduled"].is_null());
    assert!(stale["event"]["error"].as_str().unwrap().contains("99"));

    application
        .windows
        .get_mut(&target_native)
        .unwrap()
        .pending_plugin_window_close = Some(PendingPluginWindowClose {
        plugin_id: "other-closer".into(),
        plugin_generation: 3,
        result_request_id: 9,
    });
    let already_pending = plugin_event(
        &mut application,
        "window-closer",
        7,
        crate::plugin::structured::StructuredResponseAction::CloseWindow {
            action_id: "duplicate-close".into(),
            window_id: Some(8),
        },
    );
    assert_eq!(already_pending["event"]["ok"], false);
    assert!(
        already_pending["event"]["error"]
            .as_str()
            .unwrap()
            .contains("pending plugin closure")
    );
    application
        .windows
        .get_mut(&target_native)
        .unwrap()
        .pending_plugin_window_close = None;

    let sibling = plugin_event(
        &mut application,
        "window-closer",
        7,
        crate::plugin::structured::StructuredResponseAction::CloseWindow {
            action_id: "close-sibling-window".into(),
            window_id: Some(8),
        },
    );
    assert_eq!(sibling["plugin_id"], "window-closer");
    assert_eq!(sibling["event"]["action"], "close-window");
    assert_eq!(sibling["event"]["ok"], true);
    assert_eq!(sibling["origin_window_id"], 7);
    assert_eq!(sibling["event"]["window_id"], 8);
    assert_eq!(sibling["event"]["scheduled"], true);
    assert_eq!(sibling["event"]["tab_count"], 1);
    assert_eq!(sibling["event"]["remaining_windows"], 1);
    assert!(!application.windows[&target_native].should_exit);
    application.arm_plugin_window_close(target_native, origin_native, "window-closer", 4, 41);
    assert!(application.windows[&target_native].should_exit);
    assert!(!application.windows[&origin_native].should_exit);
    assert!(
        application.windows[&origin_native]
            .pending_plugin_window_close
            .is_none()
    );

    application
        .windows
        .get_mut(&target_native)
        .unwrap()
        .should_exit = false;
    let origin_event = plugin_event(
        &mut application,
        "window-closer",
        7,
        crate::plugin::structured::StructuredResponseAction::CloseWindow {
            action_id: "close-origin-window".into(),
            window_id: Some(7),
        },
    );
    assert_eq!(origin_event["event"]["ok"], true);
    let generation = application.windows[&origin_native].plugin_runtime_generation;
    application.arm_plugin_window_close(
        origin_native,
        origin_native,
        "window-closer",
        generation,
        42,
    );
    let completion = |request_id| crate::plugin::structured::StructuredHandlerCompletion {
        window_id: 7,
        plugin_generation: generation,
        plugin_id: "window-closer".into(),
        request_id,
        continuation: StructuredHandlerInvocation::fixture("window-closer", Default::default()),
        result: Ok(Vec::new()),
    };
    application
        .windows
        .get_mut(&origin_native)
        .unwrap()
        .accept_structured_plugin_completion(completion(41));
    assert!(!application.windows[&origin_native].should_exit);
    assert!(
        application.windows[&origin_native]
            .pending_plugin_window_close
            .is_some()
    );
    application
        .windows
        .get_mut(&origin_native)
        .unwrap()
        .accept_structured_plugin_completion(completion(42));
    assert!(application.windows[&origin_native].should_exit);
    assert!(
        application.windows[&origin_native]
            .pending_plugin_window_close
            .is_none()
    );
    assert!(
        origin_event.to_string().len()
            <= crate::plugin::structured::MAX_PERSISTENT_SOURCE_EVENT_BYTES
    );

    application
        .windows
        .get_mut(&origin_native)
        .unwrap()
        .should_exit = false;
    application
        .windows
        .get_mut(&target_native)
        .unwrap()
        .should_exit = true;
    let final_window = plugin_event(
        &mut application,
        "window-closer",
        7,
        crate::plugin::structured::StructuredResponseAction::CloseWindow {
            action_id: "refuse-final-window".into(),
            window_id: Some(7),
        },
    );
    assert_eq!(final_window["event"]["ok"], false);
    assert_eq!(final_window["event"]["scheduled"], serde_json::Value::Null);
    assert!(
        final_window["event"]["error"]
            .as_str()
            .unwrap()
            .contains("final live")
    );
    assert!(!application.windows[&origin_native].should_exit);
}

#[test]
fn plugin_window_focus_is_exact_bounded_and_reuses_native_policy() {
    let (target, origin, _) = pane_transfer_fixture();
    let mut application =
        Application::from_config(Config::default(), None, LaunchPlan::default()).unwrap();
    application.pending.clear();
    let target_native = WindowId::from(1_313_u64);
    let origin_native = WindowId::from(1_414_u64);
    application.windows.insert(target_native, target);
    application.windows.insert(origin_native, origin);
    application.window_order = vec![target_native, origin_native];
    application.control_window_ids.insert(target_native, 8);
    application.control_window_ids.insert(origin_native, 7);

    let stale = plugin_event(
        &mut application,
        "window-focuser",
        7,
        crate::plugin::structured::StructuredResponseAction::FocusWindow {
            action_id: "stale-window".into(),
            window_id: 99,
        },
    );
    assert_eq!(stale["event"]["action"], "focus-window");
    assert_eq!(stale["event"]["ok"], false);
    assert_eq!(stale["origin_window_id"], 7);
    assert_eq!(stale["event"]["window_id"], 99);
    assert_eq!(stale["event"]["requested"], serde_json::Value::Null);
    assert!(stale["event"]["already_focused"].is_null());
    assert!(stale["event"]["error"].as_str().unwrap().contains("99"));

    let native_less = plugin_event(
        &mut application,
        "window-focuser",
        7,
        crate::plugin::structured::StructuredResponseAction::FocusWindow {
            action_id: "exact-sibling".into(),
            window_id: 8,
        },
    );
    assert_eq!(native_less["event"]["ok"], false);
    assert_eq!(native_less["event"]["window_id"], 8);
    assert_eq!(native_less["event"]["requested"], serde_json::Value::Null);
    assert!(
        native_less["event"]["error"]
            .as_str()
            .unwrap()
            .contains("has no native window")
    );

    application
        .windows
        .get_mut(&target_native)
        .unwrap()
        .should_exit = true;
    let closing = plugin_event(
        &mut application,
        "window-focuser",
        7,
        crate::plugin::structured::StructuredResponseAction::FocusWindow {
            action_id: "closing-sibling".into(),
            window_id: 8,
        },
    );
    assert_eq!(closing["event"]["ok"], false);
    assert!(
        closing["event"]["error"]
            .as_str()
            .unwrap()
            .contains("closing")
    );
    assert!(
        closing.to_string().len() <= crate::plugin::structured::MAX_PERSISTENT_SOURCE_EVENT_BYTES
    );
    assert!(!application.windows[&origin_native].should_exit);
}

#[test]
fn whole_tab_transfer_rejects_live_source_atomically() {
    use crate::tabs::LiveDomainTab;

    let (mut source, mut destination, moved_child) = pane_transfer_fixture();
    let tab = source.tabs.as_mut().unwrap().active_tab_mut().unwrap();
    let pane_ids = tab
        .panes
        .pane_ids()
        .into_iter()
        .enumerate()
        .map(|(index, id)| (id, u32::try_from(index + 1).unwrap()))
        .collect();
    tab.live_domain = Some(LiveDomainTab {
        domain_id: "domain".into(),
        tab_id: 4,
        pane_ids,
    });

    let error = Application::transfer_active_tab(&mut source, &mut destination)
        .unwrap_err()
        .to_string();
    assert!(error.contains("topology-owned"), "{error}");
    assert!(!source.should_exit);
    let source_panes = source.active_panes();
    assert_eq!(source_panes.pane_count(), 2);
    assert!(
        source_panes
            .iter_with_layout()
            .any(|(pane, _)| pane.pty.child_id() == moved_child)
    );
    assert_eq!(destination.tabs.as_ref().unwrap().count(), 1);
}

#[test]
fn one_plugin_reload_moves_every_window_to_one_generation() {
    let temp = tempfile::tempdir().unwrap();
    let registry = temp.path().join("registry.toml");
    let root = temp.path().join("installed");
    std::fs::create_dir_all(&root).unwrap();
    let mut application =
        Application::from_config(Config::default(), None, LaunchPlan::default()).unwrap();
    application.pending.clear();
    application.plugin_paths = crate::plugin::runtime::PluginPaths {
        registry: registry.clone(),
        root: root.clone(),
    };
    application.last_plugin_registry_mtime = application.plugin_paths.registry_mtime();
    let natives = [WindowId::from(11_u64), WindowId::from(12_u64)];
    for native in natives {
        let mut window = App::new_headless(Config::default()).unwrap();
        window.plugin_host = Arc::clone(&application.plugin_host);
        window.set_runtime_plugin_test_paths(registry.clone(), root.clone());
        application.windows.insert(native, window);
    }
    let generation = application.plugin_host.generation();
    let poll = |application: &mut Application| {
        application.last_plugin_registry_check = Instant::now()
            .checked_sub(App::CONFIG_POLL_INTERVAL * 2)
            .unwrap();
        application.sync_plugin_generations(Instant::now());
    };
    let reload_toasts = |application: &Application| {
        application
            .windows
            .values()
            .map(|window| {
                window
                    .toasts
                    .iter()
                    .filter(|toast| toast.message == "Third-party plugins reloaded")
                    .count()
            })
            .collect::<Vec<_>>()
    };
    let stale_is_discarded = |application: &mut Application, plugin_generation| {
        natives.iter().all(|native| {
            application
                .windows
                .get_mut(native)
                .unwrap()
                .accept_structured_plugin_completion(
                    crate::plugin::structured::StructuredHandlerCompletion {
                        window_id: 1,
                        plugin_generation,
                        plugin_id: "absent".into(),
                        request_id: 1,
                        continuation: StructuredHandlerInvocation::fixture(
                            "absent",
                            Default::default(),
                        ),
                        result: Ok(Vec::new()),
                    },
                )
                .is_none()
        })
    };

    poll(&mut application);
    assert_eq!(application.plugin_host.generation(), generation);
    assert_eq!(reload_toasts(&application), vec![0, 0]);

    // A registry change made outside Termeric is noticed once for the whole
    // process, and every window follows the one new generation.
    std::fs::write(
        &registry,
        format!(
            "schema_version = {}\n",
            crate::plugin::runtime::REGISTRY_SCHEMA_VERSION
        ),
    )
    .unwrap();
    poll(&mut application);
    let reloaded = application.plugin_host.generation();
    assert_ne!(reloaded, generation);
    assert_eq!(reload_toasts(&application), vec![1, 1]);
    for window in application.windows.values() {
        assert_eq!(window.plugin_runtime_generation, reloaded);
    }
    assert!(stale_is_discarded(&mut application, generation));
    poll(&mut application);
    assert_eq!(application.plugin_host.generation(), reloaded);

    // One window's own reload, as when its plugin manager writes the
    // registry, brings the other along without a second reload when the
    // registry poll comes due.
    std::fs::write(
        &registry,
        format!(
            "schema_version = {}\n\n",
            crate::plugin::runtime::REGISTRY_SCHEMA_VERSION
        ),
    )
    .unwrap();
    application
        .windows
        .get_mut(&natives[0])
        .unwrap()
        .reload_plugins();
    let after_window_reload = application.plugin_host.generation();
    application.sync_plugin_generations(Instant::now());
    for window in application.windows.values() {
        assert_eq!(window.plugin_runtime_generation, after_window_reload);
    }
    assert!(stale_is_discarded(&mut application, reloaded));
    poll(&mut application);
    assert_eq!(application.plugin_host.generation(), after_window_reload);
    assert_eq!(reload_toasts(&application), vec![1, 1]);
}

#[test]
fn a_plugin_result_never_reaches_a_window_that_took_a_closed_ones_native_id() {
    let mut application =
        Application::from_config(Config::default(), None, LaunchPlan::default()).unwrap();
    application.pending.clear();
    // Window 7 asked and closed; window 8 then took its native ID, as a
    // Wayland surface at a reused address does.
    let native = WindowId::from(21_u64);
    let mut window = App::new_headless(Config::default()).unwrap();
    window.control_window_id = 8;
    window
        .plugin_manager
        .register(crate::test_support::identity_plugin(
            "late-fixture",
            "Late Fixture",
        ))
        .unwrap();
    let generation = window.plugin_runtime_generation;
    application.windows.insert(native, window);
    application.window_order = vec![native];
    application.control_window_ids.insert(native, 8);
    let completion = |window_id| crate::plugin::structured::StructuredHandlerCompletion {
        window_id,
        plugin_generation: generation,
        plugin_id: "late-fixture".into(),
        request_id: 5,
        continuation: StructuredHandlerInvocation::fixture("late-fixture", Default::default()),
        result: Ok(vec![StructuredResponseAction::ShowMessage {
            message: "for the window that asked".into(),
        }]),
    };
    let shown = |application: &Application| {
        application.windows[&native]
            .toasts
            .iter()
            .any(|toast| toast.message == "for the window that asked")
    };

    application.handle_structured_plugin_completed(completion(7));
    assert!(!shown(&application));
    application.handle_structured_plugin_completed(completion(8));
    assert!(shown(&application), "window 8's own result reaches it");
}

#[test]
fn local_tabs_never_move_into_a_live_domain_window() {
    let (mut source, mut destination, moved_child) = pane_transfer_fixture();
    destination.launch.domain = Some(crate::domain::DomainTarget {
        authority: crate::domain::DomainAuthority::Local,
        domain_id: "domain".into(),
    });
    let source_tab = source.tabs.as_ref().unwrap().active_tab().unwrap().id;

    let moving = Application::transfer_active_tab(&mut source, &mut destination)
        .unwrap_err()
        .to_string();
    assert!(moving.contains("live-domain window"), "{moving}");
    // Dropping the tab on the window takes the same refusal.
    let dropping =
        Application::transfer_control_tab(&mut source, &mut destination, source_tab as u64, 1)
            .unwrap_err();
    assert!(dropping.contains("live-domain window"), "{dropping}");

    assert!(!source.should_exit);
    assert_eq!(source.tabs.as_ref().unwrap().count(), 1);
    assert!(
        source
            .active_panes()
            .iter_with_layout()
            .any(|(pane, _)| pane.pty.child_id() == moved_child)
    );
    assert_eq!(destination.tabs.as_ref().unwrap().count(), 1);
}
