//! Command-line parsing: launch plans, and every subcommand's arguments.

use super::*;
use crate::control::ControlCommand;
use crate::control::protocol::{EnvironmentVariable, Request};

#[test]
fn exact_launch_environment_flags_are_explicit_and_order_independent() {
    for verb in ["launch-tab", "launch-window"] {
        for flags in [
            vec!["--clear-env", "--env", "EMPTY="],
            vec!["--unset-env", "PATH", "--env", "EMPTY="],
        ] {
            let mut args = vec!["control", verb, "42", "7"];
            args.extend(flags);
            args.extend(["--", "/bin/true"]);
            let outcome = parse(args.into_iter().map(OsString::from)).unwrap();
            let ParseOutcome::Control(crate::control::ControlCommand::Launch {
                request:
                    crate::control::protocol::Request::LaunchTab {
                        environment,
                        environment_policy: policy,
                        ..
                    }
                    | crate::control::protocol::Request::LaunchWindow {
                        environment,
                        environment_policy: policy,
                        ..
                    },
                ..
            }) = outcome
            else {
                panic!("wrong launch command");
            };
            assert_eq!(environment[0].value, "");
            assert!(policy.clear || policy.unset == ["PATH"]);
        }
        for flags in [
            vec!["--clear-env", "--clear-env"],
            vec!["--unset-env"],
            vec!["--unset-env", "A=B"],
            vec!["--unset-env", "TERM"],
            vec!["--unset-env", "SHELL"],
            vec!["--unset-env", "PATH", "--unset-env", "PATH"],
            vec!["--clear-env", "--unset-env", "PATH"],
            vec!["--unset-env", "PATH", "--clear-env"],
            vec!["--env", "PATH=x", "--unset-env", "PATH"],
            vec!["--unset-env", "PATH", "--env", "PATH=x"],
        ] {
            let mut args = vec!["control", verb, "42", "7"];
            args.extend(flags);
            args.extend(["--", "/bin/true"]);
            assert!(parse(args.into_iter().map(OsString::from)).is_err());
        }
    }
}
#[test]
fn background_command_tab_launch_flag_is_available() {
    for wait in [false, true] {
        let mut args = vec!["control", "launch-tab", "42", "7", "--background"];
        if wait {
            args.push("--wait");
        }
        args.extend(["--", "/bin/true", "--background"]);
        let ParseOutcome::Control(crate::control::ControlCommand::Launch {
            request:
                crate::control::protocol::Request::LaunchTab {
                    background: true,
                    args,
                    ..
                },
            wait: launch_wait,
            ..
        }) = parse(args.into_iter().map(OsString::from)).unwrap()
        else {
            panic!("background flag lost");
        };
        assert_eq!(args, ["--background"]);
        assert_eq!(launch_wait.is_some(), wait);
    }
    for args in [
        vec![
            "control",
            "launch-tab",
            "42",
            "7",
            "--background",
            "--background",
            "--",
            "/bin/true",
        ],
        vec![
            "control",
            "launch-window",
            "42",
            "7",
            "--background",
            "--",
            "/bin/true",
        ],
        vec!["control", "new-tab", "42", "7", "--background"],
    ] {
        assert!(parse(args.into_iter().map(OsString::from)).is_err());
    }
}

#[test]
fn stdin_environment_launch_flag_is_available() {
    for target in ["launch-tab", "launch-window"] {
        for wait in [false, true] {
            let mut args = vec!["control", target, "42", "7", "--env-stdin"];
            if wait {
                args.push("--wait");
            }
            args.extend(["--", "/bin/true", "--env-stdin"]);
            assert!(parse(args.into_iter().map(OsString::from)).is_ok());
        }
    }
}

#[test]
fn combined_launch_wait_options_and_child_arguments() {
    for target in ["launch-tab", "launch-window"] {
        let result = parse(
            [
                "control",
                target,
                "42",
                "7",
                "--wait",
                "--timeout",
                "1",
                "--json",
                "--",
                "/bin/true",
                "--wait",
                "--timeout",
                "0",
            ]
            .into_iter()
            .map(OsString::from),
        );
        let ParseOutcome::Control(crate::control::ControlCommand::Launch {
            process_id: 42,
            request,
            request_id: None,
            env_stdin: false,
            wait:
                Some(crate::control::Wait {
                    timeout_seconds: Some(1),
                }),
            json: true,
        }) = result.unwrap()
        else {
            panic!("expected combined wait")
        };
        let args = match request {
            crate::control::protocol::Request::LaunchTab { args, .. }
            | crate::control::protocol::Request::LaunchWindow { args, .. } => args,
            _ => panic!("expected launch request"),
        };
        assert_eq!(args, ["--wait", "--timeout", "0"]);
        for flags in [
            vec!["--wait", "--wait"],
            vec!["--timeout", "1"],
            vec!["--wait", "--timeout", "0"],
            vec!["--wait", "--timeout", "86401"],
            vec!["--wait", "--timeout", "-1"],
            vec!["--wait", "--timeout", "1.5"],
            vec!["--wait", "--timeout"],
            vec!["--wait", "--timeout", "1", "--timeout", "2"],
        ] {
            let mut arguments = vec!["control", target, "42", "7"];
            arguments.extend(flags);
            arguments.extend(["--", "/bin/true"]);
            assert!(parse(arguments.into_iter().map(OsString::from)).is_err());
        }
        for flags in [vec!["--wait"], vec!["--timeout", "86400", "--wait"]] {
            let mut arguments = vec!["control", target, "42", "7"];
            arguments.extend(flags);
            arguments.extend(["--", "/bin/true"]);
            assert!(parse(arguments.into_iter().map(OsString::from)).is_ok());
        }
    }
}

#[test]
fn wait_launch_accepts_only_bounded_unique_options() {
    let id = "a".repeat(32);
    for args in [
        vec!["control", "wait-launch", "42", &id],
        vec![
            "control",
            "wait-launch",
            "42",
            &id,
            "--timeout",
            "86400",
            "--json",
        ],
        vec![
            "control",
            "wait-launch",
            "42",
            &id,
            "--json",
            "--timeout",
            "1",
        ],
    ] {
        assert!(matches!(
            parse(args.into_iter().map(OsString::from)).unwrap(),
            ParseOutcome::Control(crate::control::ControlCommand::WaitLaunch {
                process_id: 42,
                ..
            })
        ));
    }
    for args in [
        vec!["control", "wait-launch", "42"],
        vec!["control", "wait-launch", "0", &id],
        vec!["control", "wait-launch", "42", "invalid"],
        vec!["control", "wait-launch", "42", &id, "--timeout"],
        vec!["control", "wait-launch", "42", &id, "--timeout", "0"],
        vec!["control", "wait-launch", "42", &id, "--timeout", "86401"],
        vec!["control", "wait-launch", "42", &id, "--timeout", "-1"],
        vec!["control", "wait-launch", "42", &id, "--timeout", "1.5"],
        vec![
            "control",
            "wait-launch",
            "42",
            &id,
            "--timeout",
            "999999999999999",
        ],
        vec![
            "control",
            "wait-launch",
            "42",
            &id,
            "--timeout",
            "1",
            "--timeout",
            "2",
        ],
        vec!["control", "wait-launch", "42", &id, "--json", "--json"],
        vec!["control", "wait-launch", "42", &id, "--enter"],
    ] {
        assert!(parse(args.into_iter().map(OsString::from)).is_err());
    }
}
#[test]
fn launch_status_requires_exact_launch_identity_and_only_optional_json() {
    let id = "a".repeat(32);
    assert!(
        matches!(parse(["control", "launch-status", "42", &id, "--json"].into_iter().map(OsString::from)).unwrap(),
        ParseOutcome::Control(crate::control::ControlCommand::LaunchStatus { process_id: 42, launch_request_id, json: true }) if launch_request_id == id)
    );
    for args in [
        vec!["control", "launch-status", "42"],
        vec!["control", "launch-status", "42", "invalid"],
        vec!["control", "launch-status", "0", &id],
        vec!["control", "launch-status", "42", &id, "--json", "--json"],
        vec!["control", "launch-status", "42", &id, "--wait"],
    ] {
        assert!(parse(args.into_iter().map(OsString::from)).is_err());
    }
}

fn run(arguments: &[&str]) -> LaunchPlan {
    let result = parse(arguments.iter().map(OsString::from)).unwrap();
    let ParseOutcome::Run(plan) = result else {
        panic!("expected launch plan");
    };
    plan
}

#[test]
fn arbitrary_argv_after_separator_is_preserved_without_option_parsing() {
    let plan = run(&["--", "printf", "--help", "two words", "λ"]);
    assert_eq!(
        plan.windows[0].tabs[0].target,
        LaunchTarget::Command {
            program: "printf".into(),
            args: vec!["--help".into(), "two words".into(), "λ".into()]
        }
    );
}

#[test]
fn reusable_terminal_profiles_are_distinct_from_plugin_profiles() {
    let plan = run(&[
        "--terminal-profile",
        "dev.rust",
        "--working-directory",
        "/tmp",
        "--title",
        "Override",
        "--split",
        "vertical",
        "--tab",
        "--profile",
        "codex:review",
    ]);
    assert_eq!(
        plan.windows[0].tabs[0].target,
        LaunchTarget::TerminalProfile {
            name: "dev.rust".into()
        }
    );
    assert_eq!(
        plan.windows[0].tabs[0].working_directory,
        Some(PathBuf::from("/tmp"))
    );
    assert_eq!(plan.windows[0].tabs[0].title.as_deref(), Some("Override"));
    assert_eq!(
        plan.windows[0].tabs[1].target,
        LaunchTarget::Plugin {
            plugin_id: "codex".into(),
            profile: "review".into()
        }
    );
}

#[test]
fn unicode_cwd_tabs_splits_windows_identity_and_geometry_are_grouped() {
    let plan = run(&[
        "--working-directory",
        "/tmp/根",
        "--split",
        "vertical",
        "--tab",
        "--profile",
        "codex:review",
        "--new-window",
        "--geometry",
        "1000x700",
        "--app-id",
        "termeric.review",
        "--workspace",
        "workspace-27",
    ]);
    assert_eq!(plan.windows.len(), 2);
    assert_eq!(plan.windows[0].tabs.len(), 2);
    assert_eq!(
        plan.windows[0].tabs[0].working_directory,
        Some(PathBuf::from("/tmp/根"))
    );
    assert_eq!(plan.windows[0].tabs[0].splits, [SplitDirection::Vertical]);
    assert_eq!(plan.windows[1].pixel_size, Some((1000, 700)));
    assert_eq!(plan.windows[1].app_id.as_deref(), Some("termeric.review"));
    assert_eq!(plan.windows[1].workspace.as_deref(), Some("workspace-27"));
}

#[test]
fn conflicting_and_invalid_arguments_fail_with_specific_errors() {
    for arguments in [
        vec!["--workspace", "workspace-1", "--tab"],
        vec!["--profile", "missing-colon"],
        vec!["--terminal-profile", "bad name"],
        vec!["--terminal-profile", ".dev"],
        vec!["--terminal-profile", "a..b"],
        vec!["--terminal-profile", &"a".repeat(65)],
        vec!["--terminal-profile", "one", "--command", "echo"],
        vec!["--command", "echo", "--profile", "codex:review"],
        vec!["--geometry", "80x24"],
        vec!["--"],
        vec!["positional"],
        vec!["--title", "two\nlines"],
        vec!["--title", "tab\tstop"],
        vec!["--title", "\u{202e}reversed"],
    ] {
        assert!(parse(arguments.into_iter().map(OsString::from)).is_err());
    }
    assert!(parse(["--title", "Ünïcode ✓"].into_iter().map(OsString::from)).is_ok());
}

#[test]
fn help_and_version_do_not_create_a_launch_plan() {
    assert!(matches!(
        parse([OsString::from("--help")]).unwrap(),
        ParseOutcome::Print(text) if text.contains("--new-window")
    ));
    assert!(matches!(
        parse([OsString::from("--version")]).unwrap(),
        ParseOutcome::Print(text) if text.starts_with("termeric ")
    ));
}

#[test]
fn quick_access_is_one_explicit_single_window_launch() {
    let ParseOutcome::QuickAccess(plan) = parse(
        ["--quick-access", "--terminal-profile", "dev.rust"]
            .into_iter()
            .map(OsString::from),
    )
    .unwrap() else {
        panic!("expected quick-access launch plan")
    };
    assert_eq!(plan.windows.len(), 1);
    assert!(plan.windows[0].quick_access);
    assert!(plan.windows[0].force_new);
    assert!(matches!(
        plan.windows[0].tabs[0].target,
        LaunchTarget::TerminalProfile { ref name } if name == "dev.rust"
    ));
    assert!(
        parse(
            ["--quick-access", "--new-window"]
                .into_iter()
                .map(OsString::from)
        )
        .unwrap_err()
        .to_string()
        .contains("exactly one window")
    );
    assert!(
        parse(
            ["--quick-access", "--quick-access"]
                .into_iter()
                .map(OsString::from)
        )
        .is_err()
    );
}

/// What `termeric control ARGUMENTS` parses to.
fn control(arguments: &[&str]) -> ControlCommand {
    let mut all = vec!["control"];
    all.extend(arguments);
    match parse(all.into_iter().map(OsString::from)).unwrap() {
        ParseOutcome::Control(command) => command,
        _ => panic!("expected a control command"),
    }
}

/// `request` to process 42, as `control` parses it.
fn send(request: Request, request_id: Option<&str>, json: bool) -> ControlCommand {
    ControlCommand::Send {
        process_id: 42,
        request,
        request_id: request_id.map(str::to_owned),
        json,
    }
}

/// Checks `arguments`, a whole command line, are refused.
fn rejects(arguments: &[&str]) {
    assert!(
        parse(arguments.iter().map(OsString::from)).is_err(),
        "{arguments:?}"
    );
}

/// A replay identity for actions.
const REQUEST_ID: &str = "55555555555555555555555555555555";

#[test]
fn control_list_and_help_need_no_display() {
    assert!(matches!(
        parse(
            ["control", "list", "--json"]
                .into_iter()
                .map(OsString::from)
        )
        .unwrap(),
        ParseOutcome::Control(ControlCommand::List { json: true })
    ));
    assert!(matches!(
        parse(["control", "--help"].into_iter().map(OsString::from)).unwrap(),
        ParseOutcome::Print(text) if text.contains("Typing into panes with send-text stays off")
    ));
    // A command that does not exist.
    rejects(&["control", "send"]);
}

#[test]
fn control_get_text_reads_one_pane_within_bounds() {
    assert_eq!(
        control(&[
            "get-text",
            "42",
            "7",
            "3",
            "1",
            "--history-lines",
            "40",
            "--bytes",
            "4096",
            "--json",
        ]),
        send(
            Request::ReadText {
                window_id: 7,
                tab_id: 3,
                pane_id: 1,
                history_lines: 40,
                max_bytes: 4096,
            },
            None,
            true
        )
    );
    rejects(&["control", "get-text", "42", "7", "3", "1", "--bytes", "0"]);
}

#[test]
fn control_send_text_carries_exact_text_and_its_replay_identity() {
    let id = REQUEST_ID;
    assert_eq!(
        control(&[
            "send-text",
            "42",
            "7",
            "3",
            "1",
            "cargo test",
            "--paste",
            "--enter",
            "--request-id",
            id,
            "--json",
        ]),
        send(
            Request::SendText {
                request_id: String::new(),
                window_id: 7,
                tab_id: 3,
                pane_id: 1,
                text: "cargo test".into(),
                paste: true,
                enter: true,
            },
            Some(id),
            true
        )
    );
    // Typed text may itself look like an option.
    assert!(matches!(
        control(&["send-text", "42", "7", "3", "1", "-n"]),
        ControlCommand::Send {
            request: Request::SendText { text, .. },
            ..
        } if text == "-n"
    ));
    rejects(&["control", "send-text", "42", "7", "3", "1", "bad\rinput"]);
}

#[test]
fn control_new_windows_and_tabs_name_their_source_window() {
    let id = REQUEST_ID;
    assert_eq!(
        control(&["new-window", "42", "7", "--request-id", id, "--json"]),
        send(
            Request::CreateWindow {
                request_id: String::new(),
                source_window: 7,
            },
            Some(id),
            true
        )
    );
    assert_eq!(
        control(&["new-tab", "42", "7"]),
        send(
            Request::CreateTab {
                request_id: String::new(),
                window_id: 7,
            },
            None,
            false
        )
    );
    for arguments in [
        vec!["control", "new-window", "0", "7"],
        vec!["control", "new-window", "42", "0"],
        vec!["control", "new-window", "42", "7", "--request-id", "short"],
        vec!["control", "new-window", "42", "7", "--json", "--json"],
        vec!["control", "new-tab", "42", "0"],
        vec!["control", "new-tab", "42", "7", "--request-id", "short"],
        vec!["control", "new-tab", "42", "7", "--json", "--json"],
    ] {
        rejects(&arguments);
    }
}

#[test]
fn control_launches_keep_exact_argv_directory_title_and_environment() {
    let id = REQUEST_ID;
    let launch_place = (
        Some("/tmp/two words".to_string()),
        vec![
            EnvironmentVariable {
                name: "EMPTY_VALUE".into(),
                value: String::new(),
            },
            EnvironmentVariable {
                name: "EXACT_VALUE".into(),
                value: "= two words $HOME;🙂".into(),
            },
        ],
    );
    let argv = ["", "two words", "$HOME;🙂", "--json"]
        .map(String::from)
        .to_vec();
    for verb in ["launch-tab", "launch-window"] {
        let request = if verb == "launch-tab" {
            Request::LaunchTab {
                background: false,
                request_id: String::new(),
                window_id: 7,
                program: "/bin/printf".into(),
                args: argv.clone(),
                working_directory: launch_place.0.clone(),
                title: Some("Exact 🙂".into()),
                environment: launch_place.1.clone(),
                environment_policy: Default::default(),
            }
        } else {
            Request::LaunchWindow {
                request_id: String::new(),
                source_window: 7,
                program: "/bin/printf".into(),
                args: argv.clone(),
                working_directory: launch_place.0.clone(),
                title: Some("Exact 🙂".into()),
                environment: launch_place.1.clone(),
                environment_policy: Default::default(),
            }
        };
        assert_eq!(
            control(&[
                verb,
                "42",
                "7",
                "--cwd",
                "/tmp/two words",
                "--title",
                "Exact 🙂",
                "--env",
                "EXACT_VALUE== two words $HOME;🙂",
                "--env",
                "EMPTY_VALUE=",
                "--request-id",
                id,
                "--json",
                "--",
                "/bin/printf",
                "",
                "two words",
                "$HOME;🙂",
                "--json",
            ]),
            ControlCommand::Launch {
                process_id: 42,
                request,
                request_id: Some(id.into()),
                env_stdin: false,
                wait: None,
                json: true,
            }
        );
    }
    for arguments in [
        vec!["control", "launch-window", "42", "7"],
        vec!["control", "launch-window", "42", "0", "--", "ok"],
        vec!["control", "launch-window", "42", "7", "--", ""],
        vec![
            "control",
            "launch-window",
            "42",
            "7",
            "--env",
            "NO_EQUALS",
            "--",
            "ok",
        ],
        vec![
            "control",
            "launch-window",
            "42",
            "7",
            "--env",
            "TERM=unsafe",
            "--",
            "ok",
        ],
        vec![
            "control",
            "launch-window",
            "42",
            "7",
            "--cwd",
            "relative",
            "--",
            "ok",
        ],
        vec![
            "control",
            "launch-window",
            "42",
            "7",
            "--json",
            "--json",
            "--",
            "ok",
        ],
        vec!["control", "launch-tab", "42", "7"],
        vec!["control", "launch-tab", "42", "7", "--", ""],
        vec![
            "control",
            "launch-tab",
            "42",
            "7",
            "--env",
            "DUPLICATE=first",
            "--env",
            "DUPLICATE=second",
            "--",
            "ok",
        ],
        vec![
            "control",
            "launch-tab",
            "42",
            "7",
            "--cwd",
            "relative",
            "--",
            "ok",
        ],
        vec![
            "control",
            "launch-tab",
            "42",
            "7",
            "--title",
            "",
            "--",
            "ok",
        ],
        vec!["control", "launch-tab", "42", "7", "ok"],
        vec![
            "control",
            "launch-tab",
            "42",
            "7",
            "--json",
            "--json",
            "--",
            "ok",
        ],
    ] {
        rejects(&arguments);
    }
}

#[test]
fn control_layout_actions_target_exact_windows_tabs_and_panes() {
    for (arguments, request) in [
        (
            vec!["split-pane", "42", "7", "3", "0", "vertical"],
            Request::SplitPane {
                request_id: String::new(),
                window_id: 7,
                tab_id: 3,
                pane_id: 0,
                direction: SplitDirection::Vertical,
            },
        ),
        (
            vec!["split-pane", "42", "7", "3", "0", "h"],
            Request::SplitPane {
                request_id: String::new(),
                window_id: 7,
                tab_id: 3,
                pane_id: 0,
                direction: SplitDirection::Horizontal,
            },
        ),
        (
            vec!["focus-window", "42", "7"],
            Request::FocusWindow {
                request_id: String::new(),
                window_id: 7,
            },
        ),
        (
            vec!["select-tab", "42", "7", "3"],
            Request::SelectTab {
                request_id: String::new(),
                window_id: 7,
                tab_id: 3,
            },
        ),
        (
            vec!["focus-pane", "42", "7", "3", "0"],
            Request::FocusPane {
                request_id: String::new(),
                window_id: 7,
                tab_id: 3,
                pane_id: 0,
            },
        ),
        (
            vec!["close-window", "42", "7"],
            Request::CloseWindow {
                request_id: String::new(),
                window_id: 7,
            },
        ),
        (
            vec!["close-tab", "42", "7", "3"],
            Request::CloseTab {
                request_id: String::new(),
                window_id: 7,
                tab_id: 3,
            },
        ),
        (
            vec!["close-pane", "42", "7", "3", "1"],
            Request::ClosePane {
                request_id: String::new(),
                window_id: 7,
                tab_id: 3,
                pane_id: 1,
            },
        ),
        (
            vec!["move-tab", "42", "7", "3", "9", "2"],
            Request::MoveTab {
                request_id: String::new(),
                source_window_id: 7,
                tab_id: 3,
                destination_window_id: 9,
                destination_index: 2,
            },
        ),
        (
            vec![
                "move-pane",
                "42",
                "7",
                "3",
                "1",
                "9",
                "4",
                "2",
                "horizontal",
            ],
            Request::MovePane {
                request_id: String::new(),
                source_window_id: 7,
                source_tab_id: 3,
                pane_id: 1,
                destination_window_id: 9,
                destination_tab_id: 4,
                target_pane_id: 2,
                direction: SplitDirection::Horizontal,
            },
        ),
        (
            vec!["resize-window", "42", "7", "900", "600"],
            Request::ResizeWindow {
                request_id: String::new(),
                window_id: 7,
                width: 900,
                height: 600,
            },
        ),
        (
            vec!["resize-divider", "42", "7", "3", "0", "650"],
            Request::ResizeDivider {
                request_id: String::new(),
                window_id: 7,
                tab_id: 3,
                divider_id: 0,
                ratio_millis: 650,
            },
        ),
    ] {
        assert_eq!(control(&arguments), send(request, None, false));
    }
    for arguments in [
        vec!["control", "split-pane", "42", "7", "3", "0", "diagonal"],
        vec!["control", "split-pane", "42", "7", "3", "-1", "vertical"],
        vec!["control", "focus-window", "42", "0"],
        vec!["control", "select-tab", "42", "7"],
        vec!["control", "focus-pane", "42", "7", "3", "-1"],
        vec!["control", "close-window", "42", "0"],
        vec!["control", "close-tab", "42", "7"],
        vec!["control", "close-pane", "42", "7", "3", "-1"],
        vec!["control", "move-tab", "42", "7", "3", "0", "2"],
        vec!["control", "move-tab", "42", "7", "3", "9", "257"],
        vec!["control", "resize-window", "42", "7", "159", "600"],
        vec!["control", "resize-window", "42", "7", "900", "16385"],
        vec!["control", "resize-divider", "42", "7", "3", "1", "99"],
        vec!["control", "resize-divider", "42", "7", "3", "1", "901"],
        vec![
            "control",
            "move-pane",
            "42",
            "7",
            "3",
            "1",
            "9",
            "4",
            "2",
            "diagonal",
        ],
        vec![
            "control",
            "focus-pane",
            "42",
            "7",
            "3",
            "0",
            "--json",
            "--json",
        ],
        vec![
            "control",
            "split-pane",
            "42",
            "7",
            "3",
            "0",
            "vertical",
            "--json",
            "--json",
        ],
    ] {
        rejects(&arguments);
    }
}

#[test]
fn plain_launches_join_a_running_termeric_and_others_keep_their_own() {
    let cwd = std::path::Path::new("/home/me/project");
    let tabs = |arguments: &[&str]| run(arguments).join_tabs(Some(cwd));
    let plain = tabs(&[]).unwrap();
    assert_eq!(plain.len(), 1);
    assert_eq!(
        plain[0].working_directory.as_deref(),
        Some("/home/me/project")
    );
    assert_eq!(
        (plain[0].program.as_deref(), plain[0].args.len()),
        (None, 0)
    );
    let desktop = tabs(&["--new-window"]).unwrap();
    assert_eq!(desktop.len(), 1);
    let rich = tabs(&[
        "--working-directory",
        "/srv",
        "--title",
        "Build",
        "--tab",
        "--",
        "make",
        "-j8",
    ])
    .unwrap();
    assert_eq!(rich[0].working_directory.as_deref(), Some("/srv"));
    let relative = tabs(&["--working-directory", "src"]).unwrap();
    assert_eq!(
        relative[0].working_directory.as_deref(),
        Some("/home/me/project/src")
    );
    assert_eq!(rich[0].title.as_deref(), Some("Build"));
    assert_eq!(rich[1].program.as_deref(), Some("make"));
    assert_eq!(rich[1].args, ["-j8"]);
    for standalone in [
        &["--separate-process"][..],
        &["--new-window", "--new-window"],
        &["--workspace", "main"],
        &["--split", "vertical"],
        &["--app-id", "work"],
        &["--geometry", "800x600"],
        &["--renderer", "software"],
        &["--terminal-profile", "ssh"],
        &["--profile", "codex:review"],
    ] {
        assert!(
            tabs(standalone).is_none(),
            "{standalone:?} should keep its own process"
        );
    }
    assert!(run(&["--separate-process"]).separate_process);
}

#[test]
fn leading_new_window_flag_creates_one_window_and_later_delimiters_add_more() {
    assert_eq!(run(&["--new-window"]).windows.len(), 1);
    assert_eq!(run(&["--new-window", "--new-window"]).windows.len(), 2);
}

#[test]
fn only_launches_asking_for_no_windows_restore_the_last_session() {
    for restoring in [
        &[][..],
        &["--separate-process"],
        &["--renderer", "software"],
        &["--separate-process", "--renderer", "gpu"],
    ] {
        assert!(run(restoring).restores_last_session(), "{restoring:?}");
    }
    for explicit in [
        &["--new-window"][..],
        &["--working-directory", "/tmp"],
        &["--workspace", "main"],
        &["--title", "Logs"],
        &["--", "htop"],
    ] {
        assert!(!run(explicit).restores_last_session(), "{explicit:?}");
    }
}

#[test]
fn renderer_override_is_validated_and_belongs_to_the_process() {
    let plan = run(&["--new-window", "--renderer", "software"]);
    assert_eq!(plan.windows.len(), 1);
    assert_eq!(
        plan.renderer_backend,
        Some(crate::config::RendererBackend::Software)
    );

    let plan = run(&["--new-window", "--new-window", "--renderer", "gpu"]);
    assert_eq!(plan.windows.len(), 2);
    assert_eq!(
        plan.renderer_backend,
        Some(crate::config::RendererBackend::Gpu)
    );
    assert!(parse([OsString::from("--renderer"), OsString::from("cpu")]).is_err());
    assert!(
        parse(
            ["--renderer", "auto", "--renderer", "software"]
                .into_iter()
                .map(OsString::from)
        )
        .is_err()
    );
}

#[test]
fn options_take_their_value_after_an_equals_sign_and_flags_refuse_one() {
    use std::os::unix::ffi::OsStrExt;
    assert_eq!(
        run(&[
            "--title=a=b",
            "--cwd=/tmp",
            "--renderer=software",
            "--split=v",
            "--class=org.example.Roots",
        ]),
        run(&[
            "--title",
            "a=b",
            "--cwd",
            "/tmp",
            "--renderer",
            "software",
            "--split",
            "v",
            "--class",
            "org.example.Roots",
        ])
    );
    // The value is the rest of the argument, bytes and all.
    let path = OsStr::from_bytes(b"/tmp/\xff");
    let mut inline = OsString::from("--working-directory=");
    inline.push(path);
    let ParseOutcome::Run(plan) = parse([inline]).unwrap() else {
        panic!("expected launch plan");
    };
    assert_eq!(
        plan.windows[0].tabs[0].working_directory.as_deref(),
        Some(std::path::Path::new(path))
    );
    // An empty value is a value, which then has to be valid.
    assert!(parse([OsString::from("--title=")]).is_err());

    for flag in [
        "--tab=1",
        "--new-window=yes",
        "--separate-process=",
        "--quick-access=1",
        "--help=x",
        "--version=x",
    ] {
        let error = parse([OsString::from(flag)]).unwrap_err().to_string();
        let option = flag.split('=').next().unwrap();
        assert_eq!(error, format!("{option} does not take a value"), "{flag}");
    }
    let error = parse([OsString::from("--bogus=1")])
        .unwrap_err()
        .to_string();
    assert!(error.contains("unknown option \"--bogus\""), "{error}");
    let error = parse([OsString::from("--=x")]).unwrap_err().to_string();
    assert!(error.contains("unknown option \"--=x\""), "{error}");
    // After --, an equals sign is part of the command's own argument.
    assert_eq!(
        run(&["--", "env", "--title=x"]).windows[0].tabs[0].target,
        LaunchTarget::Command {
            program: "env".into(),
            args: vec!["--title=x".into()]
        }
    );
}

#[test]
fn workspace_list_parses_offline_and_rejects_strays() {
    let parsed = |arguments: &[&str]| parse(arguments.iter().map(OsString::from));
    use crate::workspace::WorkspaceCommand;
    assert_eq!(
        parsed(&["workspace", "list"]).unwrap(),
        ParseOutcome::Workspace(WorkspaceCommand::List { json: false })
    );
    assert_eq!(
        parsed(&["workspace", "list", "--json"]).unwrap(),
        ParseOutcome::Workspace(WorkspaceCommand::List { json: true })
    );
    assert!(matches!(
        parsed(&["workspace"]).unwrap(),
        ParseOutcome::Print(help) if help.contains("list")
    ));
    assert!(parsed(&["workspace", "list", "extra"]).is_err());
    assert!(parsed(&["workspace", "open", "id"]).is_err());
}

#[test]
fn plugin_admin_commands_are_display_free_and_permission_explicit() {
    let ParseOutcome::Plugin(crate::plugin::runtime::PluginAdminCommand::Install {
        directory,
        grants,
        enable,
        replace,
    }) = parse(
        [
            "plugin",
            "install",
            "/tmp/sample",
            "--allow",
            "process-spawn",
            "--allow",
            "workspace-query:all-windows",
            "--allow",
            "navigate",
            "--allow",
            "pane-layout:all-windows",
            "--allow",
            "command-output",
            "--enable",
            "--replace",
        ]
        .into_iter()
        .map(OsString::from),
    )
    .unwrap()
    else {
        panic!("expected plugin install command");
    };
    assert_eq!(directory, PathBuf::from("/tmp/sample"));
    assert_eq!(
        grants.to_string(),
        "process-spawn,command-output,workspace-query:all-windows,navigate,pane-layout:all-windows"
    );
    assert!(enable && replace);

    assert!(matches!(
        parse(["plugin", "list", "--json"].into_iter().map(OsString::from)).unwrap(),
        ParseOutcome::Plugin(crate::plugin::runtime::PluginAdminCommand::List { json: true })
    ));
    assert!(matches!(
        parse(
            ["plugin", "configure", "sample", "--set", "line-count", "42"]
                .into_iter()
                .map(OsString::from)
        )
        .unwrap(),
        ParseOutcome::Plugin(crate::plugin::runtime::PluginAdminCommand::Configure {
            id,
            change: crate::plugin::runtime::PluginConfigurationChange::Set { key, value },
        }) if id == "sample" && key == "line-count" && value == "42"
    ));
    assert!(matches!(
        parse(
            ["plugin", "configure", "sample", "--reset-settings"]
                .into_iter()
                .map(OsString::from)
        )
        .unwrap(),
        ParseOutcome::Plugin(crate::plugin::runtime::PluginAdminCommand::Configure {
            change: crate::plugin::runtime::PluginConfigurationChange::ResetSettings,
            ..
        })
    ));
    assert!(matches!(
        parse(
            ["plugin", "configure", "sample", "--clear-default-profile"]
                .into_iter()
                .map(OsString::from)
        )
        .unwrap(),
        ParseOutcome::Plugin(crate::plugin::runtime::PluginAdminCommand::Configure {
            id,
            change: crate::plugin::runtime::PluginConfigurationChange::ClearDefaultProfile,
        }) if id == "sample"
    ));
    assert!(
        parse(
            [
                "plugin",
                "configure",
                "sample",
                "--clear-default-profile",
                "extra",
            ]
            .into_iter()
            .map(OsString::from)
        )
        .is_err()
    );
    for invalid in [
        &["--allow", "camera"][..],
        &["--allow", "terminal-text"][..],
        &["--allow", "process-spawn:all-windows"][..],
        &["--allow", "navigate", "--allow", "navigate:all-windows"][..],
    ] {
        assert!(
            parse(
                ["plugin", "install", "/tmp/sample"]
                    .into_iter()
                    .chain(invalid.iter().copied())
                    .map(OsString::from)
            )
            .is_err(),
            "{invalid:?}"
        );
    }
    assert!(
        parse(
            ["plugin", "configure", "sample"]
                .into_iter()
                .map(OsString::from)
        )
        .is_err()
    );
}

#[test]
fn plugin_grant_commands_name_their_permissions() {
    use crate::plugin::runtime::PluginAdminCommand;
    let parse_plugin = |arguments: &[&str]| {
        parse(
            ["plugin"]
                .iter()
                .chain(arguments)
                .copied()
                .map(OsString::from),
        )
    };
    let ParseOutcome::Plugin(PluginAdminCommand::Enable { id, grants }) =
        parse_plugin(&["enable", "sample"]).unwrap()
    else {
        panic!("expected plugin enable");
    };
    assert_eq!(id, "sample");
    assert!(grants.is_empty());
    let ParseOutcome::Plugin(PluginAdminCommand::Enable { grants, .. }) = parse_plugin(&[
        "enable",
        "sample",
        "--allow",
        "process-spawn",
        "--allow",
        "navigate:all-windows",
    ])
    .unwrap() else {
        panic!("expected plugin enable");
    };
    assert_eq!(grants.to_string(), "process-spawn,navigate:all-windows");
    let ParseOutcome::Plugin(PluginAdminCommand::Grant { id, grants }) =
        parse_plugin(&["grant", "sample", "--allow", "workspace-query"]).unwrap()
    else {
        panic!("expected plugin grant");
    };
    assert_eq!(id, "sample");
    assert_eq!(grants.to_string(), "workspace-query");
    let ParseOutcome::Plugin(PluginAdminCommand::Revoke { id, permissions }) =
        parse_plugin(&["revoke", "sample", "--allow", "pane-create:all-windows"]).unwrap()
    else {
        panic!("expected plugin revoke");
    };
    assert_eq!(id, "sample");
    assert_eq!(permissions.to_string(), "pane-create:all-windows");
    // Grant and revoke need at least one permission, each a known one.
    for invalid in [
        &["grant", "sample"][..],
        &["revoke", "sample"][..],
        &["grant", "sample", "--allow", "camera"][..],
    ] {
        assert!(parse_plugin(invalid).is_err(), "{invalid:?}");
    }
}

/// What `termeric domain ARGUMENTS` parses to.
fn domain(arguments: &[&str]) -> crate::domain::DomainCommand {
    let mut all = vec!["domain"];
    all.extend(arguments);
    match parse(all.into_iter().map(OsString::from)).unwrap() {
        ParseOutcome::Domain(command) => command,
        _ => panic!("expected a domain command"),
    }
}

/// The daemon request `termeric domain ARGUMENTS` sends.
fn domain_request(arguments: &[&str]) -> crate::domain::protocol::Request {
    match domain(arguments) {
        crate::domain::DomainCommand::Request { request, .. } => request,
        command => panic!("expected a daemon request, got {command:?}"),
    }
}

/// The window `termeric domain ARGUMENTS` opens.
fn domain_window(arguments: &[&str]) -> WindowLaunch {
    let mut all = vec!["domain"];
    all.extend(arguments);
    match parse(all.into_iter().map(OsString::from)).unwrap() {
        ParseOutcome::Run(plan) => plan.windows.into_iter().next().unwrap(),
        _ => panic!("expected a domain window launch"),
    }
}

#[test]
fn domain_create_keeps_unicode_argv_size_and_json() {
    let crate::domain::DomainCommand::Request {
        request: crate::domain::protocol::Request::CreateDomain { name, spawn },
        json,
    } = domain(&[
        "create",
        "daily",
        "--cwd",
        "/tmp",
        "--size",
        "90x30",
        "--json",
        "--",
        "printf",
        "two words",
        "λ",
    ])
    else {
        panic!("expected domain create command");
    };
    assert_eq!(name, "daily");
    assert_eq!(spawn.program.as_deref(), Some("printf"));
    assert_eq!(spawn.args, ["two words", "λ"]);
    assert_eq!((spawn.cols, spawn.rows), (90, 30));
    assert!(json);
}

#[test]
fn domain_attach_opens_a_window_on_a_local_or_remote_domain() {
    let window = domain_window(&[
        "attach",
        "domain-123",
        "--geometry",
        "900x600",
        "--app-id",
        "termeric.domain",
    ]);
    assert_eq!(
        window.domain,
        Some(crate::domain::DomainTarget {
            authority: crate::domain::DomainAuthority::Local,
            domain_id: "domain-123".into(),
        })
    );
    assert_eq!(window.pixel_size, Some((900, 600)));
    assert_eq!(window.app_id.as_deref(), Some("termeric.domain"));

    let remote = domain_window(&["--host", "dev@example.test", "attach", "domain-123"]);
    assert_eq!(
        remote.domain,
        Some(crate::domain::DomainTarget {
            authority: crate::domain::DomainAuthority::Ssh {
                destination: "dev@example.test".into(),
            },
            domain_id: "domain-123".into(),
        })
    );
}

#[test]
fn domain_tab_and_pane_edits_name_their_exact_targets() {
    use crate::domain::protocol::Request;
    use crate::panes::PaneLayoutPreset;
    assert!(matches!(
        domain_request(&["close-pane", "domain-123", "42"]),
        Request::ClosePane { domain, pane: 42 } if domain == "domain-123"
    ));
    assert!(matches!(
        domain_request(&["close-tab", "domain-123", "7"]),
        Request::CloseTab { domain, tab: 7 } if domain == "domain-123"
    ));
    assert!(matches!(
        domain_request(&["resize-split", "domain-123", "9", "375"]),
        Request::ResizeSplit { domain, split: 9, ratio_millis: 375 } if domain == "domain-123"
    ));
    assert!(matches!(
        domain_request(&["rename-tab", "domain-123", "7", "Build λ"]),
        Request::RenameTab { domain, tab: 7, title: Some(title) }
            if domain == "domain-123" && title == "Build λ"
    ));
    // An empty title clears it.
    assert!(matches!(
        domain_request(&["rename-tab", "domain-123", "7", ""]),
        Request::RenameTab {
            tab: 7,
            title: None,
            ..
        }
    ));
    assert!(matches!(
        domain_request(&["pin-tab", "domain-123", "7"]),
        Request::SetTabPinned {
            tab: 7,
            pinned: true,
            ..
        }
    ));
    assert!(matches!(
        domain_request(&["move-tab", "domain-123", "7", "2"]),
        Request::MoveTab {
            tab: 7,
            index: 2,
            ..
        }
    ));
    assert!(matches!(
        domain_request(&["swap-pane", "domain-123", "42"]),
        Request::SwapPaneNext { pane: 42, .. }
    ));
    assert!(matches!(
        domain_request(&["balance-tab", "domain-123", "7"]),
        Request::BalanceTab { tab: 7, .. }
    ));
    for (name, preset) in [
        ("grid", PaneLayoutPreset::Grid),
        ("tall", PaneLayoutPreset::Tall),
        ("fat", PaneLayoutPreset::Fat),
    ] {
        assert_eq!(
            domain_request(&["layout-tab", "domain-123", "7", name]),
            Request::ApplyPaneLayout {
                domain: "domain-123".into(),
                tab: 7,
                preset,
            }
        );
    }
    assert!(matches!(
        domain_request(&["move-pane", "domain-123", "42", "7"]),
        Request::MovePane {
            pane: 42,
            tab: 7,
            ..
        }
    ));
}

#[test]
fn domain_bridge_and_version_commands_run_without_a_daemon() {
    use crate::domain::DomainCommand;
    assert_eq!(
        domain(&["proxy-stdio"]),
        DomainCommand::ProxyStdio { start_server: true }
    );
    assert_eq!(
        domain(&["proxy-stdio", "--no-start"]),
        DomainCommand::ProxyStdio {
            start_server: false
        }
    );
    assert_eq!(
        domain(&["protocol-version", "--json"]),
        DomainCommand::ProtocolVersion { json: true }
    );
}

#[test]
fn domain_host_sends_daemon_commands_over_ssh() {
    use crate::domain::DomainCommand;
    assert!(matches!(
        domain(&["--host", "dev@example.test", "list", "--json"]),
        DomainCommand::Remote { destination, command }
            if destination == "dev@example.test"
                && *command == DomainCommand::Request {
                    request: crate::domain::protocol::Request::ListDomains,
                    json: true,
                }
    ));
    assert!(matches!(
        domain(&[
            "--host",
            "dev@example.test",
            "upload",
            "domain-123",
            "42",
            "/tmp/local artifact.bin",
            "--name",
            "remote λ.bin",
            "--json",
        ]),
        DomainCommand::Remote { destination, command }
            if destination == "dev@example.test"
                && matches!(
                    command.as_ref(),
                    DomainCommand::Upload {
                        domain,
                        pane: 42,
                        source,
                        name: Some(name),
                        json: true,
                    } if domain == "domain-123"
                        && source == std::path::Path::new("/tmp/local artifact.bin")
                        && name == "remote λ.bin"
                )
    ));
}

#[test]
fn domain_upload_names_cannot_leave_the_pane_directory() {
    rejects(&[
        "domain",
        "upload",
        "domain-123",
        "42",
        "/tmp/file",
        "--name",
        "../escape",
    ]);
}

#[test]
fn saved_ssh_profiles_resolve_for_controls_and_gui_without_re_resolving_relays() {
    let config: crate::config::Config = toml::from_str(
        r#"
            [domains.ssh_hosts.build]
            destination = "developer@build.example"
            description = "Build farm"
        "#,
    )
    .unwrap();
    config.validate().unwrap();
    let domains = || Ok(config.domains.clone());

    for arguments in [
        ["domain", "--host-profile", "build", "list", "--json"].as_slice(),
        ["domain", "--host-profile=build", "list", "--json"].as_slice(),
    ] {
        let arguments = arguments.iter().map(OsString::from).collect();
        let ParseOutcome::Domain(crate::domain::DomainCommand::Remote {
            destination,
            command,
        }) = parse_collected(arguments, &domains).unwrap()
        else {
            panic!("expected a remote domain command");
        };
        assert_eq!(destination, "developer@build.example");
        assert_eq!(
            *command,
            crate::domain::DomainCommand::Request {
                request: crate::domain::protocol::Request::ListDomains,
                json: true,
            }
        );
    }

    let arguments = ["domain", "--host-profile", "build", "attach", "domain-123"]
        .into_iter()
        .map(OsString::from)
        .collect();
    let ParseOutcome::Run(plan) = parse_collected(arguments, &domains).unwrap() else {
        panic!("expected saved-host GUI launch");
    };
    let target = plan.windows[0].domain.as_ref().unwrap();
    assert_eq!(
        target.authority,
        crate::domain::DomainAuthority::Ssh {
            destination: "developer@build.example".into(),
        }
    );
    assert_eq!(
        target
            .authority
            .relay_command("/opt/termeric".into())
            .pane_arguments("domain-123", 7),
        [
            "domain",
            "--host",
            "developer@build.example",
            "relay",
            "domain-123",
            "7",
        ]
    );

    assert!(matches!(
        parse(
            ["domain", "hosts", "--json"]
                .into_iter()
                .map(OsString::from)
        )
        .unwrap(),
        ParseOutcome::Domain(crate::domain::DomainCommand::Hosts { json: true })
    ));
    assert!(
        parse(
            ["domain", "--host-profile", "build", "list"]
                .into_iter()
                .map(OsString::from)
        )
        .is_err(),
        "ordinary deterministic parsing has no implicit profile source"
    );
    let nested = ["domain", "--host-profile", "build", "hosts"]
        .into_iter()
        .map(OsString::from)
        .collect();
    assert!(parse_collected(nested, &domains).is_err());
}

#[test]
fn only_a_host_profile_reads_the_configuration() {
    let broken = || anyhow::bail!("configuration unreadable");
    let parse_with_broken_config = |arguments: &[&str]| {
        let arguments = arguments.iter().map(OsString::from).collect();
        parse_collected(arguments, &broken)
    };
    assert!(parse_with_broken_config(&["domain", "list"]).is_ok());
    assert!(parse_with_broken_config(&["domain", "--host", "build.example", "list"]).is_ok());
    for arguments in [
        ["domain", "--host-profile", "build", "list"].as_slice(),
        ["domain", "--host-profile=build", "list"].as_slice(),
    ] {
        let error = parse_with_broken_config(arguments).unwrap_err();
        assert_eq!(error.to_string(), "configuration unreadable");
    }
}

#[test]
fn free_text_values_may_start_with_a_hyphen() {
    use crate::domain::DomainCommand;
    use crate::domain::protocol::Request;
    use crate::plugin::runtime::{PluginAdminCommand, PluginConfigurationChange};
    let parse_args = |arguments: &[&str]| parse(arguments.iter().map(OsString::from)).unwrap();

    let ParseOutcome::Plugin(PluginAdminCommand::Configure { change, .. }) =
        parse_args(&["plugin", "configure", "sample", "--set", "offset", "-5"])
    else {
        panic!("expected plugin configure");
    };
    assert_eq!(
        change,
        PluginConfigurationChange::Set {
            key: "offset".into(),
            value: "-5".into(),
        }
    );
    let ParseOutcome::Plugin(PluginAdminCommand::Configure { change, .. }) =
        parse_args(&["plugin", "configure", "sample", "--default-profile", "-dev"])
    else {
        panic!("expected plugin configure");
    };
    assert_eq!(
        change,
        PluginConfigurationChange::DefaultProfile("-dev".into())
    );

    let ParseOutcome::Domain(DomainCommand::Request {
        request: Request::CreateDomain { name, spawn },
        json: true,
    }) = parse_args(&[
        "domain", "create", "-named", "--title", "-foo", "--cwd", "-dir", "--json",
    ])
    else {
        panic!("expected domain create");
    };
    assert_eq!(name, "-named");
    assert_eq!(spawn.title.as_deref(), Some("-foo"));
    assert_eq!(spawn.working_directory.as_deref(), Some("-dir"));
    let ParseOutcome::Domain(DomainCommand::Upload {
        name, json: false, ..
    }) = parse_args(&[
        "domain", "upload", "domain-1", "0", "source", "--name", "-file",
    ])
    else {
        panic!("expected domain upload");
    };
    assert_eq!(name.as_deref(), Some("-file"));

    let ParseOutcome::Control(crate::control::ControlCommand::Launch {
        request: crate::control::protocol::Request::LaunchTab { title, .. },
        ..
    }) = parse_args(&[
        "control",
        "launch-tab",
        "42",
        "7",
        "--title",
        "-foo",
        "--",
        "/bin/true",
    ])
    else {
        panic!("expected control launch-tab");
    };
    assert_eq!(title.as_deref(), Some("-foo"));
}

#[test]
fn a_remote_working_directory_is_left_for_the_remote_daemon_to_check() {
    let outcome = parse(
        [
            "domain",
            "--host",
            "developer@build.example",
            "spawn",
            "domain-1",
            "--cwd",
            "/only/on/the/remote/computer",
        ]
        .into_iter()
        .map(OsString::from),
    )
    .unwrap();
    let ParseOutcome::Domain(crate::domain::DomainCommand::Remote { command, .. }) = outcome else {
        panic!("expected a remote domain command");
    };
    let crate::domain::DomainCommand::Request {
        request: crate::domain::protocol::Request::SpawnTab { spawn, .. },
        ..
    } = *command
    else {
        panic!("expected domain spawn");
    };
    assert_eq!(
        spawn.working_directory.as_deref(),
        Some("/only/on/the/remote/computer")
    );
}

#[test]
fn invalid_domain_commands_fail_before_connecting() {
    for arguments in [
        vec!["domain", "attach", "bad/id"],
        vec!["domain", "split", "domain-1", "x", "vertical"],
        vec!["domain", "split", "domain-1", "0", "diagonal"],
        vec!["domain", "get-text", "domain-1", "0", "--bytes", "0"],
        vec!["domain", "close-pane", "domain-1", "not-a-pane"],
        vec!["domain", "close-tab", "domain-1", "not-a-tab"],
        vec!["domain", "resize-split", "domain-1", "x", "500"],
        vec!["domain", "resize-split", "domain-1", "0", "99"],
        vec!["domain", "resize-split", "domain-1", "0", "901"],
        vec!["domain", "rename-tab", "domain-1", "0", "two\nlines"],
        vec!["domain", "pin-tab", "domain-1", "not-a-tab"],
        vec!["domain", "move-tab", "domain-1", "0", "not-an-index"],
        vec!["domain", "swap-pane", "domain-1", "not-a-pane"],
        vec!["domain", "balance-tab", "domain-1", "not-a-tab"],
        vec!["domain", "layout-tab", "domain-1", "0", "diagonal"],
        vec!["domain", "move-pane", "domain-1", "x", "0"],
        vec!["domain", "move-pane", "domain-1", "0", "x"],
        vec!["domain", "--host", "-unsafe", "list"],
        vec!["domain", "--host", "two words", "list"],
        vec!["domain", "--host", "host", "server", "--foreground"],
        vec!["domain", "--host", "host", "protocol-version"],
        vec!["domain", "--host", "host", "--host", "other", "list"],
        vec!["domain", "create", "name", "--size", "0x24"],
        vec!["domain", "server"],
        vec!["domain", "upload", "domain-1", "0", "/tmp/file", "--retain"],
        vec!["domain", "uploads"],
    ] {
        assert!(parse(arguments.into_iter().map(OsString::from)).is_err());
    }
}
