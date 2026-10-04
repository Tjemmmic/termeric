//! Guarded input, prompts, the Unicode picker, and context picking follow their exact owner.

use super::*;

#[test]
fn external_confirmation_never_targets_a_replacement_single_owner() {
    use crate::terminal::guardrails::GuardrailAction;
    let (mut app, directory, entry, _) = harvest_rerun_capture_fixture_in(false);
    app.config.security.allow_gui_control_input = true;
    let result = app
        .send_text_for(
            Requester::Plugin,
            entry.source.tab_id as u64,
            0,
            "rm -rf /synthetic-never-executed",
            false,
            true,
        )
        .unwrap();
    let request_id = result.3.unwrap();
    split_harvest_capture_owner(&mut app, directory.path());
    app.close_pane_for(Requester::Control, entry.source.tab_id as u64, 0)
        .unwrap();
    let captures = current_harvest_capture_routes(&mut app, directory.path());
    app.apply_guardrail_action(GuardrailAction::Confirm { request_id });
    assert!(
        fenced_harvest_capture(&mut app, &captures)
            .iter()
            .all(Vec::is_empty),
        "confirmation for removed pane 0 must not write to surviving pane 1"
    );
}

#[test]
fn guarded_input_producers_follow_only_the_original_owner_through_topology_changes() {
    use crate::terminal::guardrails::GuardrailAction;
    const COMMAND: &str = "rm -rf /synthetic-never-executed";
    // Only automated producers are confirmed; typed and pasted input is sent as-is.
    for producer in ["control", "plugin"] {
        for transition in [
            "split",
            "collapse",
            "nonzero-collapse",
            "removed",
            "read-only",
            "cancel",
        ] {
            let (mut app, directory, _, initial_captures) =
                harvest_rerun_capture_fixture_in(transition == "nonzero-collapse");
            let source = app.active_harvest_source().unwrap();
            app.config.security.allow_gui_control_input = true;
            // Metadata only: the existing shell/cat capture process is unchanged.
            app.tabs
                .as_mut()
                .unwrap()
                .active_tab_mut()
                .unwrap()
                .tab_type = crate::tabs::TabType::Plugin {
                plugin_id: "synthetic-capture".into(),
            };
            app.active_terminal_mut().unwrap().process(b"\x1b[?2004h");
            let expected_wire = {
                let result = match producer {
                    "control" => app.send_text_for(
                        Requester::Control,
                        source.tab_id as u64,
                        source.pane_id,
                        COMMAND,
                        true,
                        true,
                    ),
                    "plugin" => app.send_text_for(
                        Requester::Plugin,
                        source.tab_id as u64,
                        source.pane_id,
                        COMMAND,
                        true,
                        true,
                    ),
                    _ => unreachable!(),
                }
                .unwrap();
                assert_eq!(
                    result.2,
                    crate::control::protocol::InputDisposition::PendingConfirmation
                );
                format!("\x1b[200~{COMMAND}\x1b[201~\r").into_bytes()
            };
            let request_id = app.guardrail.request_id();
            assert!(app.guardrail.is_awaiting(request_id));
            assert!(
                fenced_harvest_capture(&mut app, &initial_captures)
                    .iter()
                    .all(Vec::is_empty)
            );
            if transition == "nonzero-collapse" {
                app.close_pane_for(Requester::Control, source.tab_id as u64, 0)
                    .unwrap();
                app.close_pane_for(Requester::Control, source.tab_id as u64, 1)
                    .unwrap();
            } else {
                let sibling = split_harvest_capture_owner(&mut app, directory.path());
                if transition == "collapse" {
                    app.close_pane_for(Requester::Control, source.tab_id as u64, sibling.0)
                        .unwrap();
                } else if transition == "removed" {
                    app.close_pane_for(Requester::Control, source.tab_id as u64, source.pane_id)
                        .unwrap();
                }
            }
            let captures = current_harvest_capture_routes(&mut app, directory.path());
            if transition == "read-only" {
                app.focus_pane_for(Requester::Control, source.tab_id as u64, source.pane_id)
                    .unwrap();
                app.toggle_focused_pane_read_only();
            }
            // Mode changes after capture must not re-encode the approved bytes.
            if let Some(origin) = app
                .tabs
                .as_ref()
                .unwrap()
                .origin_for_pane(source.tab_id, PaneId(source.pane_id))
            {
                app.tabs
                    .as_mut()
                    .unwrap()
                    .terminal_for_origin_mut(origin)
                    .unwrap()
                    .process(b"\x1b[?2004l");
            }
            let before = fenced_harvest_capture(&mut app, &captures);
            assert!(before.iter().flatten().all(|byte| *byte == 0));
            let focused = app.active_harvest_source();
            app.input_line_buffer = "preserve sibling input".into();
            if transition == "cancel" {
                app.apply_guardrail_action(GuardrailAction::Cancel { request_id });
            }
            app.apply_guardrail_action(GuardrailAction::Confirm { request_id });
            app.apply_guardrail_action(GuardrailAction::Confirm { request_id });
            let deliver = matches!(transition, "split" | "collapse" | "nonzero-collapse");
            let source_origin = app
                .tabs
                .as_ref()
                .unwrap()
                .origin_for_pane(source.tab_id, PaneId(source.pane_id));
            let expected = captures
                .iter()
                .zip(before)
                .map(|((origin, _), mut bytes)| {
                    bytes.push(0);
                    if deliver && Some(*origin) == source_origin {
                        bytes.extend_from_slice(&expected_wire);
                    }
                    bytes
                })
                .collect::<Vec<_>>();
            assert_eq!(
                fenced_harvest_capture(&mut app, &captures),
                expected,
                "{producer}/{transition}"
            );
            assert_eq!(app.active_harvest_source(), focused);
            assert_eq!(
                app.input_line_buffer,
                if deliver && focused == Some(source) {
                    ""
                } else {
                    "preserve sibling input"
                }
            );
        }
    }
}

#[test]
fn closed_guarded_destination_rejects_the_whole_batch_before_any_user_write() {
    let (mut app, _directory, entry, captures) = harvest_rerun_capture_fixture();
    let request_id = app.guardrail.show(
        crate::terminal::guardrails::check_command("rm -rf /synthetic").unwrap(),
        vec![
            crate::terminal::guardrails::GuardedWrite::for_pane(
                entry.source.tab_id,
                PaneId(1),
                b"first".to_vec(),
            ),
            crate::terminal::guardrails::GuardedWrite::for_pane(
                entry.source.tab_id,
                PaneId(0),
                b"closed".to_vec(),
            ),
        ],
    );
    app.tabs
        .as_mut()
        .unwrap()
        .active_tab_mut()
        .unwrap()
        .pane_manager
        .as_mut()
        .unwrap()
        .get_pane_mut(PaneId(entry.source.pane_id))
        .unwrap()
        .closed = true;
    app.apply_guardrail_action(crate::terminal::guardrails::GuardrailAction::Confirm {
        request_id,
    });
    assert!(
        fenced_harvest_capture(&mut app, &captures)
            .iter()
            .all(Vec::is_empty)
    );
}

#[test]
fn prompt_actions_walk_the_focused_terminals_scrollback_in_both_directions() {
    let mut tabs = TabManager::new(14, 4, ShellConfig::default(), 1_000, 8 * 1024 * 1024);
    tabs.new_tab_with_command(
        "/bin/sh",
        &crate::test_support::sleeping_args(),
        None,
        Some("prompt-navigation"),
        None,
        HashMap::new(),
    )
    .unwrap();
    let terminal = tabs
        .active_tab_mut()
        .unwrap()
        .focused_terminal_mut()
        .unwrap();
    for index in 0..5 {
        terminal.process(b"\x1b]133;A\x07\x1b]133;B\x07");
        terminal.process(format!("echo {index}\r\n").as_bytes());
        terminal.process(b"\x1b]133;C\x07output\r\n\x1b]133;D;0\x07");
    }
    terminal.process(b"\x1b]133;A\x07");
    let cursor_row = i32::try_from(terminal.grid().cursor().row).unwrap();
    let expected = terminal
        .prompt_tracker()
        .all_prompts()
        .filter(|row| *row < cursor_row)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .take(2)
        .collect::<Vec<_>>();
    assert_eq!(expected.len(), 2);
    assert!(expected[1] < 0);

    let mut app = App::new_headless(Config::default()).unwrap();
    app.tabs = Some(tabs);
    assert!(app.handle_action(crate::input::Action::PreviousPrompt));
    assert_eq!(app.active_terminal().unwrap().grid().scroll_offset(), 0);
    assert!(app.handle_action(crate::input::Action::PreviousPrompt));
    assert_eq!(
        app.active_terminal().unwrap().grid().scroll_offset(),
        usize::try_from(expected[1].saturating_neg()).unwrap()
    );
    assert!(app.handle_action(crate::input::Action::NextPrompt));
    assert_eq!(app.active_terminal().unwrap().grid().scroll_offset(), 0);
}

#[test]
fn semantic_command_completion_reaches_plugins_with_exact_bounded_source_text() {
    #[derive(Debug)]
    struct CommandCapture {
        captured: std::sync::Arc<std::sync::Mutex<Option<crate::plugin::Hook>>>,
    }

    impl crate::plugin::hooks::Plugin for CommandCapture {
        fn id(&self) -> &str {
            "command-capture"
        }

        fn name(&self) -> &str {
            "Command Capture"
        }

        fn on_hook(
            &mut self,
            hook: &crate::plugin::Hook,
            _context: &crate::plugin::PluginContext,
        ) -> Option<crate::plugin::PluginAction> {
            if matches!(hook, crate::plugin::Hook::CommandExecuted { .. }) {
                *self.captured.lock().unwrap() = Some(hook.clone());
                None
            } else {
                None
            }
        }
    }

    let mut tabs = TabManager::new(20, 4, ShellConfig::default(), 1_000, 8 * 1024 * 1024);
    tabs.new_tab_with_command(
        "/bin/sh",
        &crate::test_support::sleeping_args(),
        None,
        Some("command-event"),
        None,
        HashMap::new(),
    )
    .unwrap();
    let tab_id = tabs.active_tab().unwrap().id;
    tabs.active_tab_mut()
        .unwrap()
        .focused_terminal_mut()
        .unwrap()
        .process(
            b"\x1b]133;A\x07\x1b]133;B\x07printf safe\r\n\x1b]133;C\x07result\r\n\x1b]133;D;7\x07",
        );

    let captured = std::sync::Arc::new(std::sync::Mutex::new(None));
    let mut app = App::new_headless(Config::default()).unwrap();
    app.plugin_manager.clear();
    app.plugin_manager
        .register(Box::new(CommandCapture {
            captured: std::sync::Arc::clone(&captured),
        }))
        .unwrap();
    app.tabs = Some(tabs);
    assert!(app.process_pty_events());

    let hook = captured.lock().unwrap().take().unwrap();
    assert!(matches!(
        hook,
        crate::plugin::Hook::CommandExecuted {
            region_id: 1,
            tab_id: source_tab,
            pane_id: None,
            command,
            command_truncated: false,
            output,
            output_truncated: false,
            content_available: true,
            exit_code: Some(7),
        } if source_tab == tab_id && command == "printf safe" && output == "result"
    ));
}

#[test]
fn clear_scrollback_action_retires_shell_metadata_with_the_erased_history() {
    let mut tabs = TabManager::new(16, 4, ShellConfig::default(), 1_000, 8 * 1024 * 1024);
    tabs.new_tab_with_command(
        "/bin/sh",
        &crate::test_support::sleeping_args(),
        None,
        Some("clear-history"),
        None,
        HashMap::new(),
    )
    .unwrap();
    let terminal = tabs
        .active_tab_mut()
        .unwrap()
        .focused_terminal_mut()
        .unwrap();
    terminal
        .process(b"\x1b]133;A\x07\x1b]133;B\x07echo old\r\n\x1b]133;C\x07old\r\n\x1b]133;D;0\x07");
    terminal.process(b"one\r\ntwo\r\nthree\r\nfour\r\n");
    let region_id = terminal.prompt_tracker().completed_region(0).unwrap().id;
    assert!(terminal.grid().scrollback_len() > 0);

    let mut app = App::new_headless(Config::default()).unwrap();
    app.tabs = Some(tabs);
    assert!(app.handle_action(crate::input::Action::ClearScrollback));

    let terminal = app.active_terminal().unwrap();
    assert_eq!(terminal.grid().scrollback_len(), 0);
    assert!(
        !terminal
            .prompt_tracker()
            .completed_region_by_id(region_id)
            .unwrap()
            .content_available
    );
    assert!(terminal.prompt_tracker().all_prompts().all(|row| row >= 0));
}

#[test]
fn unicode_picker_never_inserts_into_a_replacement_single_owner() {
    let (mut app, directory, entry, _) = harvest_rerun_capture_fixture_in(false);
    app.open_unicode_picker();
    split_harvest_capture_owner(&mut app, directory.path());
    app.close_pane_for(Requester::Control, entry.source.tab_id as u64, 0)
        .unwrap();
    let captures = current_harvest_capture_routes(&mut app, directory.path());
    assert!(
        !app.apply_unicode_picker_selection(crate::ui::unicode_picker::UnicodePickerSelection {
            id: "fixture.lambda".into(),
            text: "λ".into(),
        }),
        "removed Unicode source must not alias the remaining pane"
    );
    assert!(
        fenced_harvest_capture(&mut app, &captures)
            .iter()
            .all(Vec::is_empty)
    );
}

#[test]
fn plugin_context_command_never_targets_a_focused_shell_sibling() {
    let (mut app, _directory, _, captures) = harvest_rerun_capture_fixture();
    register_capture_agent(&mut app, "synthetic-capture");
    mark_capture_plugin_owner(&mut app, PaneId(0), "synthetic-capture");
    app.tabs
        .as_mut()
        .unwrap()
        .active_tab_mut()
        .unwrap()
        .tab_type = crate::tabs::TabType::Plugin {
        plugin_id: "synthetic-capture".into(),
    };
    let tab_id = app.tabs.as_ref().unwrap().active_tab().unwrap().id;
    assert!(app.send_to_agent(
        super::agents::AgentTarget {
            identity: crate::tabs::TerminalIdentity {
                tab_id,
                pane_id: PaneId(0),
            },
            agent_id: "synthetic-capture".into(),
            agent_name: "Synthetic".into(),
        },
        "fixture context"
    ));
    assert_eq!(
        fenced_harvest_capture(&mut app, &captures),
        vec![b"fixture context".to_vec(), vec![], vec![]]
    );
}

#[test]
fn context_picker_targets_the_plugin_owner_not_its_focused_shell_sibling() {
    let (mut app, _directory, _, captures) = harvest_rerun_capture_fixture();
    let plugin_directory = tempfile::tempdir().unwrap();
    let sibling_directory = tempfile::tempdir().unwrap();
    let tab = app.tabs.as_mut().unwrap().active_tab_mut().unwrap();
    // Launch identity only; existing raw shell/cat processes remain unchanged.
    tab.tab_type = crate::tabs::TabType::Plugin {
        plugin_id: "synthetic-capture".into(),
    };
    tab.pane_manager
        .as_mut()
        .unwrap()
        .get_pane_mut(PaneId(0))
        .unwrap()
        .launch = crate::workspace::WorkspaceLaunch::Plugin {
        plugin_id: "synthetic-capture".into(),
        profile: None,
    };
    let owner = crate::tabs::TerminalIdentity {
        tab_id: tab.id,
        pane_id: PaneId(0),
    };
    register_capture_agent(&mut app, "synthetic-capture");
    mark_capture_plugin_owner(&mut app, PaneId(0), "synthetic-capture");
    let tab = app.tabs.as_mut().unwrap().active_tab_mut().unwrap();
    for (pane_id, directory) in [
        (PaneId(0), plugin_directory.path()),
        (PaneId(2), sibling_directory.path()),
    ] {
        tab.pane_manager
            .as_mut()
            .unwrap()
            .get_pane_mut(pane_id)
            .unwrap()
            .terminal
            .process(format!("\x1b]7;file://localhost{}\x07", directory.display()).as_bytes());
    }
    app.open_context_picker_for(owner, &synthetic_agent("synthetic-capture"));
    let request_id = app.context_picker.request_id();
    for source in app
        .context_picker
        .snapshot()
        .sources
        .into_iter()
        .filter(|source| source.selected)
    {
        app.context_picker
            .apply(crate::ui::context_picker::ContextPickerAction::Toggle {
                request_id,
                source: source.id,
            });
    }
    app.context_picker
        .apply(crate::ui::context_picker::ContextPickerAction::Toggle {
            request_id,
            source: crate::ui::context_picker::ContextSourceId::TerminalOutput,
        });
    let request = app
        .context_picker
        .begin_gather(request_id, Some("fixture context".into()))
        .unwrap();
    assert_eq!(request.cwd.as_deref(), Some(plugin_directory.path()));
    app.handle_context_gathered(request_id, Ok("fixture context".into()));
    assert_eq!(
        fenced_harvest_capture(&mut app, &captures),
        vec![b"fixture context".to_vec(), vec![], vec![]]
    );
}

#[test]
fn unicode_picker_tracks_only_the_same_focused_owner_through_topology_changes() {
    for transition in [
        "split",
        "unfocused",
        "collapse",
        "nonzero-collapse",
        "removed",
        "read-only",
        "closed",
        "closing",
        "cancel",
        "broadcast",
    ] {
        let (mut app, directory, _, _) =
            harvest_rerun_capture_fixture_in(transition == "nonzero-collapse");
        let source = app.active_harvest_source().unwrap();
        app.open_unicode_picker();
        if transition == "nonzero-collapse" {
            app.close_pane_for(Requester::Control, source.tab_id as u64, 0)
                .unwrap();
            app.close_pane_for(Requester::Control, source.tab_id as u64, 1)
                .unwrap();
        } else {
            let sibling = split_harvest_capture_owner(&mut app, directory.path());
            match transition {
                "removed" => {
                    app.close_pane_for(Requester::Control, source.tab_id as u64, source.pane_id)
                        .unwrap();
                }
                "collapse" => {
                    app.close_pane_for(Requester::Control, source.tab_id as u64, sibling.0)
                        .unwrap();
                }
                "unfocused" => {}
                _ => {
                    app.focus_pane_for(Requester::Control, source.tab_id as u64, source.pane_id)
                        .unwrap();
                }
            }
        }
        let captures = current_harvest_capture_routes(&mut app, directory.path());
        match transition {
            "read-only" => app.toggle_focused_pane_read_only(),
            "closed" => app.tabs.as_mut().unwrap().active_tab_mut().unwrap().closed = true,
            "closing" => {
                app.tabs
                    .as_mut()
                    .unwrap()
                    .active_tab_mut()
                    .unwrap()
                    .closing_at = Some(Instant::now())
            }
            "cancel" => app.close_unicode_picker(),
            "broadcast" => app.toggle_broadcast(),
            _ => {}
        }
        let focused = app.active_harvest_source();
        app.input_line_buffer = "prefix".into();
        let selection = || crate::ui::unicode_picker::UnicodePickerSelection {
            id: "fixture.lambda".into(),
            text: "λ".into(),
        };
        let accepted = app.apply_unicode_picker_selection(selection());
        let allowed = matches!(transition, "split" | "collapse" | "nonzero-collapse");
        assert_eq!(accepted, allowed, "{transition}");
        assert!(!app.apply_unicode_picker_selection(selection()));
        let source_origin = app
            .tabs
            .as_ref()
            .unwrap()
            .origin_for_pane(source.tab_id, PaneId(source.pane_id));
        let expected = captures
            .iter()
            .map(|(origin, _)| {
                if allowed && Some(*origin) == source_origin {
                    "λ".as_bytes().to_vec()
                } else {
                    vec![]
                }
            })
            .collect::<Vec<_>>();
        assert_eq!(
            fenced_harvest_capture(&mut app, &captures),
            expected,
            "{transition}"
        );
        assert_eq!(app.active_harvest_source(), focused);
        if allowed {
            assert_eq!(app.input_line_buffer, "prefixλ");
        } else if matches!(transition, "unfocused" | "removed") {
            assert_eq!(app.input_line_buffer, "prefix");
        }
    }
}
