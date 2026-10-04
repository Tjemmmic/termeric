//! Agents: context, input, hook reports, change review, transcripts, and the supervisor.

use super::*;

#[test]
fn context_picker_completion_preserves_plugin_owner_across_topology_changes() {
    use crate::ui::context_picker::{ContextPickerAction, ContextSourceId};
    for transition in [
        "split",
        "collapse",
        "nonzero-collapse",
        "removed",
        "replacement-plugin",
        "read-only",
        "changed-plugin",
        "closed",
        "closing",
        "cancel",
    ] {
        let (mut app, directory, _, _) =
            harvest_rerun_capture_fixture_in(transition == "nonzero-collapse");
        let source = app.active_harvest_source().unwrap();
        register_capture_agent(&mut app, "synthetic-capture");
        mark_capture_plugin_owner(&mut app, PaneId(source.pane_id), "synthetic-capture");
        app.open_context_picker_for(
            crate::tabs::TerminalIdentity {
                tab_id: source.tab_id,
                pane_id: PaneId(source.pane_id),
            },
            &synthetic_agent("synthetic-capture"),
        );
        let request_id = app.context_picker.request_id();
        for item in app
            .context_picker
            .snapshot()
            .sources
            .into_iter()
            .filter(|item| item.selected)
        {
            app.context_picker.apply(ContextPickerAction::Toggle {
                request_id,
                source: item.id,
            });
        }
        app.context_picker.apply(ContextPickerAction::Toggle {
            request_id,
            source: ContextSourceId::TerminalOutput,
        });
        assert!(
            app.context_picker
                .begin_gather(request_id, Some("local fixture".into()))
                .is_some()
        );
        if transition == "nonzero-collapse" {
            app.close_pane_for(Requester::Control, source.tab_id as u64, 0)
                .unwrap();
            app.close_pane_for(Requester::Control, source.tab_id as u64, 1)
                .unwrap();
        } else {
            let sibling = split_harvest_capture_owner(&mut app, directory.path());
            match transition {
                "collapse" => {
                    app.close_pane_for(Requester::Control, source.tab_id as u64, sibling.0)
                        .unwrap();
                }
                "removed" | "replacement-plugin" => {
                    app.close_pane_for(Requester::Control, source.tab_id as u64, source.pane_id)
                        .unwrap();
                }
                _ => {}
            }
            if transition == "replacement-plugin" {
                mark_capture_plugin_owner(&mut app, sibling, "synthetic-capture");
            }
        }
        let captures = current_harvest_capture_routes(&mut app, directory.path());
        assert!(
            fenced_harvest_capture(&mut app, &captures)
                .iter()
                .all(Vec::is_empty)
        );
        match transition {
            "read-only" => {
                app.focus_pane_for(Requester::Control, source.tab_id as u64, source.pane_id)
                    .unwrap();
                app.toggle_focused_pane_read_only();
            }
            "changed-plugin" => {
                mark_capture_plugin_owner(&mut app, PaneId(source.pane_id), "different-capture")
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
            "cancel" => {
                app.context_picker
                    .apply(ContextPickerAction::Cancel { request_id });
            }
            _ => {}
        }
        let focused = app.active_harvest_source();
        app.handle_context_gathered(request_id, Ok("local fixture".into()));
        app.handle_context_gathered(request_id, Ok("must not replay".into()));
        let allowed = matches!(transition, "split" | "collapse" | "nonzero-collapse");
        let source_origin = app
            .tabs
            .as_ref()
            .unwrap()
            .origin_for_pane(source.tab_id, PaneId(source.pane_id));
        let expected = captures
            .iter()
            .map(|(origin, _)| {
                let mut bytes = vec![0];
                if allowed && Some(*origin) == source_origin {
                    bytes.extend_from_slice(b"local fixture");
                }
                bytes
            })
            .collect::<Vec<_>>();
        assert_eq!(
            fenced_harvest_capture(&mut app, &captures),
            expected,
            "{transition}"
        );
        assert_eq!(app.active_harvest_source(), focused);
    }
}

#[test]
fn completed_context_never_falls_back_to_a_replacement_plugin_tab() {
    let mut tabs = TabManager::for_test(ShellConfig::default());
    for title in ["original-context-target", "replacement-context-target"] {
        tabs.new_tab_with_command(
            "/bin/sh",
            &crate::test_support::sleeping_args(),
            None,
            Some(title),
            Some(crate::tabs::TabType::Plugin {
                plugin_id: "fixture-plugin".into(),
            }),
            HashMap::new(),
        )
        .unwrap();
    }
    let original_id = tabs.iter().next().unwrap().1.id;
    let mut app = App::new_headless(Config::default()).unwrap();
    app.tabs = Some(tabs);
    let request = app.context_picker.open(
        None,
        crate::ui::context_picker::ContextTarget {
            agent_id: "fixture-plugin".into(),
            agent_name: "Fixture".into(),
            terminal: crate::tabs::TerminalIdentity {
                tab_id: original_id,
                pane_id: PaneId(0),
            },
        },
    );
    app.context_picker
        .apply(crate::ui::context_picker::ContextPickerAction::Toggle {
            request_id: request,
            source: crate::ui::context_picker::ContextSourceId::TerminalOutput,
        });
    assert!(
        app.context_picker
            .begin_gather(request, Some("local fixture context".into()))
            .is_some()
    );
    assert!(app.tabs.as_mut().unwrap().close_tab(0));

    app.handle_context_gathered(request, Ok("local fixture context".into()));

    assert_eq!(
        latest_toast(&app),
        Some("Fixture is no longer running in that terminal")
    );
    assert!(!app.context_picker.is_awaiting(request));
}

#[test]
fn typed_agent_input_is_submitted_separately_and_only_to_the_same_agent() {
    use std::io::Read;
    let temp = tempfile::tempdir().unwrap();
    let capture = temp.path().join("capture");
    // `cat` invoked as `codex`, so the terminal is recognized as running
    // Codex each time Termeric checks before writing.
    let cat = ["/bin/cat", "/usr/bin/cat"]
        .into_iter()
        .find(|path| std::path::Path::new(path).exists())
        .unwrap();
    let codex = temp.path().join("codex");
    std::os::unix::fs::symlink(cat, &codex).unwrap();
    let mut tabs = TabManager::for_test(ShellConfig::default());
    // Raw mode: bytes reach the capture file exactly as written.
    let tab_id = tabs
        .new_tab_with_command(
            "/bin/sh",
            &[
                "-c".into(),
                format!(
                    "stty raw -echo; printf RAW_READY; exec {} > {}",
                    codex.display(),
                    capture.display()
                ),
            ],
            None,
            None,
            None,
            HashMap::new(),
        )
        .unwrap();
    let tab = tabs.active_tab_mut().unwrap();
    let pane_id = tab.focused_terminal_identity().pane_id;
    tab.pane_integration_mut(pane_id).unwrap().agent = Some(crate::agents::AgentSession {
        agent_id: "codex".into(),
        launched: false,
        profile: None,
        process: None,
    });
    let mut app = App::new_headless(Config::default()).unwrap();
    app.tabs = Some(tabs);
    let target = super::agents::AgentTarget {
        identity: crate::tabs::TerminalIdentity { tab_id, pane_id },
        agent_id: "codex".into(),
        agent_name: "Codex".into(),
    };
    let read = |expected_len: usize| {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let mut bytes = Vec::new();
            if let Ok(mut file) = std::fs::File::open(&capture) {
                file.read_to_end(&mut bytes).unwrap();
            }
            if bytes.len() >= expected_len || Instant::now() > deadline {
                return bytes;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
    };

    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let tabs = app.tabs.as_mut().unwrap();
        tabs.poll_events();
        let ready = tabs
            .active_tab()
            .and_then(Tab::focused_terminal)
            .is_some_and(|terminal| terminal.grid().plain_lines().concat().contains("RAW_READY"));
        if ready {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "the capture never entered raw mode"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
    // `RAW_READY` is printed just before the `exec`.
    while app.recheck_agent_target(&target).is_err() {
        assert!(Instant::now() < deadline, "Codex was never recognized");
        std::thread::sleep(Duration::from_millis(5));
    }
    app.submit_to_agent(target.clone(), b"/new").unwrap();
    let due = app.next_agent_submit_deadline().unwrap();
    app.submit_due_agent_input(due - Duration::from_millis(1));
    assert_eq!(
        read(4),
        b"/new",
        "Enter must wait for the typed text to settle"
    );
    app.submit_due_agent_input(due);
    assert_eq!(read(5), b"/new\r");
    assert_eq!(app.next_agent_submit_deadline(), None);

    // Two messages in quick succession are submitted one after the other,
    // never merged into one prompt.
    app.submit_to_agent(target.clone(), b"a").unwrap();
    app.submit_to_agent(target.clone(), b"b").unwrap();
    assert_eq!(
        read(6),
        b"/new\ra",
        "the second waits for the first's Enter"
    );
    let due = app.next_agent_submit_deadline().unwrap();
    app.submit_due_agent_input(due);
    assert_eq!(read(8), b"/new\ra\rb");
    let due = app.next_agent_submit_deadline().unwrap();
    app.submit_due_agent_input(due);
    assert_eq!(read(9), b"/new\ra\rb\r");
    assert_eq!(app.next_agent_submit_deadline(), None);

    // An automated message waits while the person is typing to the agent.
    app.config.security.allow_gui_control_input = true;
    app.tabs
        .as_mut()
        .unwrap()
        .active_tab_mut()
        .unwrap()
        .pane_integration_mut(pane_id)
        .unwrap()
        .last_person_input = Some(Instant::now());
    assert!(
        app.submit_control_text_to_agent(tab_id as u64, pane_id.0, "codex", "go", None, None)
            .unwrap_err()
            .contains("typing")
    );
    app.tabs
        .as_mut()
        .unwrap()
        .active_tab_mut()
        .unwrap()
        .pane_integration_mut(pane_id)
        .unwrap()
        .last_person_input = None;
    app.config.security.allow_gui_control_input = false;

    // The control protocol lists the agent and can submit to it, but only
    // once the person allows external input, and only to the same agent.
    app.tabs
        .as_mut()
        .unwrap()
        .active_tab_mut()
        .unwrap()
        .pane_integration_mut(pane_id)
        .unwrap()
        .agent = Some(crate::agents::AgentSession {
        agent_id: "codex".into(),
        launched: false,
        profile: None,
        process: None,
    });
    let listed = app.control_agents(7);
    assert_eq!(listed.len(), 1);
    assert_eq!(
        (listed[0].window_id, listed[0].tab_id, listed[0].pane_id),
        (7, tab_id as u64, pane_id.0)
    );
    assert_eq!(listed[0].state, None, "no hook has reported");
    let submit = |app: &mut App, agent: &str| {
        app.submit_control_text_to_agent(tab_id as u64, pane_id.0, agent, "go\non", None, None)
    };
    assert!(
        submit(&mut app, "codex")
            .unwrap_err()
            .contains("allow_gui_control_input")
    );
    app.config.security.allow_gui_control_input = true;
    assert_eq!(
        submit(&mut app, "gemini").unwrap_err(),
        "Gemini CLI is no longer running in that terminal"
    );
    assert!(!submit(&mut app, "codex").unwrap(), "sent, not held");
    assert_eq!(read(14), b"/new\ra\rb\rgo\non", "pasted without Enter");
    app.submit_due_agent_input(Instant::now() + super::agents::AGENT_SUBMIT_DELAY);
    assert_eq!(read(15), b"/new\ra\rb\rgo\non\r");

    // Typing into the prompt after the text went in cancels its Enter,
    // which would submit the person's words with it.
    app.submit_to_agent(target.clone(), b"z").unwrap();
    let typed = |app: &mut App, at: Option<Instant>| {
        app.tabs
            .as_mut()
            .unwrap()
            .active_tab_mut()
            .unwrap()
            .pane_integration_mut(pane_id)
            .unwrap()
            .last_person_input = at;
    };
    typed(&mut app, Some(Instant::now()));
    app.submit_due_agent_input(Instant::now() + super::agents::AGENT_SUBMIT_DELAY);
    assert_eq!(
        latest_toast(&app),
        Some("Not submitted to Codex: you typed into its prompt")
    );
    assert_eq!(app.next_agent_submit_deadline(), None);
    typed(&mut app, None);
    assert_eq!(read(16), b"/new\ra\rb\rgo\non\rz");

    // An agent that exits before its Enter is due never receives it, nor
    // does the shell or whatever else takes its place.
    app.submit_to_agent(target.clone(), b"x").unwrap();
    assert_eq!(read(17), b"/new\ra\rb\rgo\non\rzx");
    let child = app
        .tabs
        .as_ref()
        .unwrap()
        .tab_by_id(tab_id)
        .unwrap()
        .pane_child_process(pane_id)
        .unwrap();
    // SAFETY: signalling a child this test started.
    assert_eq!(unsafe { libc::kill(child as i32, libc::SIGKILL) }, 0);
    // A process that has exited, even one not yet reaped, has no command line.
    let running =
        || std::fs::read(format!("/proc/{child}/cmdline")).is_ok_and(|argv| !argv.is_empty());
    let deadline = Instant::now() + Duration::from_secs(5);
    while running() {
        assert!(Instant::now() < deadline, "the agent did not exit");
        std::thread::sleep(Duration::from_millis(5));
    }
    app.submit_due_agent_input(Instant::now() + super::agents::AGENT_SUBMIT_DELAY);
    assert_eq!(
        latest_toast(&app),
        Some("Codex is no longer running in that terminal")
    );
    assert_eq!(app.next_agent_submit_deadline(), None);
}

#[test]
fn hook_reports_track_an_agent_started_from_a_shell_until_it_exits() {
    use crate::agents::{AgentActivity, AgentReport};
    use crate::control::protocol::AgentEventKind;
    let Some(bash) = ["/bin/bash", "/usr/bin/bash"]
        .into_iter()
        .find(|path| std::path::Path::new(path).exists())
    else {
        return;
    };
    // Outside any repository, so noticing the agent snapshots nothing real.
    let directory = tempfile::tempdir().unwrap();
    let mut tabs = TabManager::for_test(ShellConfig::default());
    let tab_id = tabs
        .new_tab_with_command(
            bash,
            &["--norc".into(), "--noprofile".into(), "-i".into()],
            directory.path().to_str(),
            None,
            None,
            HashMap::new(),
        )
        .unwrap();
    let mut app = App::new_headless(Config::default()).unwrap();
    let shown = app.notifications.capture_tracked_commands_for_test();
    app.tabs = Some(tabs);
    let terminal_id = app
        .tabs
        .as_ref()
        .unwrap()
        .active_tab()
        .unwrap()
        .lone_pty()
        .unwrap()
        .terminal_id()
        .unwrap()
        .to_owned();

    // An interactive shell gives the program its own foreground job, as it
    // does for an agent typed at the prompt. `sleep` is no known agent.
    app.tabs
        .as_mut()
        .unwrap()
        .active_tab_mut()
        .unwrap()
        .write(b"sleep 30\r")
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    let foreground = loop {
        let tab = app.tabs.as_ref().unwrap().active_tab().unwrap();
        let pty = tab.lone_pty().unwrap();
        let group = pty.foreground_process_group();
        if let Some(group) =
            group.filter(|group| Some(*group) != pty.child_id().map(|pid| pid as i32))
        {
            break group as u32;
        }
        assert!(Instant::now() < deadline, "sleep never took the foreground");
        std::thread::sleep(Duration::from_millis(10));
    };

    let report = |event| AgentReport {
        agent_id: "codex".into(),
        event,
        detail: None,
        last_message: None,
        transcript_path: None,
    };
    assert!(!app.record_agent_report("1.00", report(AgentEventKind::Working), None));
    // A program outside this terminal's session that borrowed its identity
    // is received and set aside.
    assert!(app.record_agent_report(
        &terminal_id,
        report(AgentEventKind::Working),
        Some(std::process::id())
    ));
    assert!(
        app.tabs
            .as_ref()
            .unwrap()
            .tab_by_id(tab_id)
            .unwrap()
            .focused_integration()
            .unwrap()
            .agent_status
            .is_none()
    );
    // The hook runs inside the terminal, as the foreground job does.
    assert!(app.record_agent_report(
        &terminal_id,
        report(AgentEventKind::Working),
        Some(foreground)
    ));
    let session = |app: &App| {
        app.tabs
            .as_ref()
            .unwrap()
            .tab_by_id(tab_id)
            .unwrap()
            .focused_integration()
            .map(|session| (session.agent.clone(), session.agent_status.clone()))
            .unwrap()
    };
    let (agent, status) = session(&app);
    assert_eq!(
        agent.unwrap().agent_id,
        "codex",
        "recognized from its report"
    );
    assert_eq!(status.unwrap().activity, AgentActivity::Working);
    // The scan keeps a reporting agent even though its process is unfamiliar.
    app.tabs.as_mut().unwrap().refresh_agents(&app.agents);
    assert!(session(&app).0.is_some());

    // Looking at the terminal answers the request; away from it, it waits.
    app.window_focused = false;
    assert!(app.record_agent_report(
        &terminal_id,
        AgentReport {
            detail: Some("Approve shell: cargo test".into()),
            ..report(AgentEventKind::NeedsInput)
        },
        None
    ));
    let status = session(&app).1.unwrap();
    assert!(status.needs_attention());
    {
        let identity = crate::tabs::TerminalIdentity {
            tab_id,
            pane_id: app
                .tabs
                .as_ref()
                .unwrap()
                .tab_by_id(tab_id)
                .unwrap()
                .focused_terminal_identity()
                .pane_id,
        };
        assert_eq!(
            shown(),
            vec![(
                "show",
                identity,
                format!("agent-{tab_id}-{}", identity.pane_id.0)
            )]
        );
        // A repeated request is not news.
        app.record_agent_report(&terminal_id, report(AgentEventKind::NeedsInput), None);
        assert!(shown().is_empty());
    }
    app.window_focused = true;
    assert!(app.acknowledge_focused_agent());
    assert!(!session(&app).1.unwrap().needs_attention());

    // When the program exits, the shell is back in front and the agent is
    // gone even though it never reported ending.
    app.tabs
        .as_mut()
        .unwrap()
        .active_tab_mut()
        .unwrap()
        .write(b"\x03")
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    while session(&app).0.is_some() {
        assert!(
            Instant::now() < deadline,
            "the exited agent stayed recognized"
        );
        app.tabs.as_mut().unwrap().refresh_agents(&app.agents);
        std::thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(session(&app).1, None);
}

#[test]
fn agent_list_puts_waiting_agents_first_and_answers_come_from_reports_or_logs() {
    use crate::agents::{AgentReport, AgentStatus};
    use crate::control::protocol::AgentEventKind;
    let temp = tempfile::tempdir().unwrap();
    let mut tabs = TabManager::for_test(ShellConfig::default());
    let mut open = |title: &str, agent: &str, event: Option<AgentEventKind>| {
        let tab_id = tabs
            .new_tab_with_command(
                "/bin/sh",
                &["-c".into(), "sleep 30".into()],
                None,
                Some(title),
                None,
                HashMap::new(),
            )
            .unwrap();
        let tab = tabs.active_tab_mut().unwrap();
        let pane_id = tab.focused_terminal_identity().pane_id;
        let session = tab.pane_integration_mut(pane_id).unwrap();
        session.agent = Some(crate::agents::AgentSession {
            agent_id: agent.into(),
            launched: false,
            profile: None,
            process: None,
        });
        session.agent_status = event.and_then(|event| {
            AgentStatus::after(
                None,
                AgentReport {
                    agent_id: agent.into(),
                    event,
                    detail: (event == AgentEventKind::NeedsInput)
                        .then(|| "Approve shell: rm build".into()),
                    last_message: None,
                    transcript_path: None,
                },
                Instant::now(),
            )
        });
        crate::tabs::TerminalIdentity { tab_id, pane_id }
    };
    let working = open("one", "claude-code", Some(AgentEventKind::Working));
    let unreported = open("two", "gemini", None);
    let waiting = open("three", "codex", Some(AgentEventKind::NeedsInput));
    let mut app = App::new_headless(Config::default()).unwrap();
    app.tabs = Some(tabs);

    let order: Vec<_> = app
        .agent_terminals()
        .iter()
        .map(|terminal| terminal.identity)
        .collect();
    assert_eq!(order, [waiting, working, unreported]);

    app.open_agent_list();
    assert_eq!(
        app.command_palette.active_pick(),
        Some(crate::ui::command_palette::palette::PickKind::AgentTerminal)
    );
    app.command_palette.close();
    app.focus_agent_terminal(&format!("{}:{}", working.tab_id, working.pane_id.0));
    assert_eq!(
        app.tabs.as_ref().unwrap().active_tab_id(),
        Some(working.tab_id)
    );
    app.focus_agent_terminal("99:0");
    assert_eq!(latest_toast(&app), Some("That agent's terminal has closed"));

    // Without an answer, copying is not offered.
    assert!(
        app.unavailable_agent_commands()
            .contains(&"agent.copy-last-response".to_string())
    );
    assert_eq!(
        app.focused_agent_answer().unwrap_err(),
        "Claude Code has not finished an answer yet"
    );

    // A log from an agent whose hook did not carry the answer.
    let log = temp.path().join("session.jsonl");
    std::fs::write(
        &log,
        r#"{"type":"assistant","message":{"content":[{"type":"text","text":"From the log"}]}}"#,
    )
    .unwrap();
    let set_status = |app: &mut App, message: Option<&str>| {
        let tab = app.tabs.as_mut().unwrap().active_tab_mut().unwrap();
        let status = tab
            .pane_integration_mut(working.pane_id)
            .unwrap()
            .agent_status
            .as_mut()
            .unwrap();
        status.transcript_path = Some(log.clone());
        status.last_message = message.map(Into::into);
    };
    set_status(&mut app, None);
    assert!(
        !app.unavailable_agent_commands()
            .contains(&"agent.copy-last-response".to_string())
    );
    assert_eq!(
        app.focused_agent_answer().unwrap(),
        ("Claude Code".to_string(), "From the log".to_string())
    );
    set_status(&mut app, Some("From the hook"));
    assert_eq!(app.focused_agent_answer().unwrap().1, "From the hook");

    // An agent without status hooks explains what is missing.
    app.focus_agent_terminal(&format!("{}:{}", unreported.tab_id, unreported.pane_id.0));
    assert_eq!(
        app.focused_agent_answer().unwrap_err(),
        "Set up agent status to copy an agent's answers"
    );
}

#[test]
fn agent_changes_are_listed_reviewed_staged_and_reverted_for_their_terminal() {
    use crate::ui::command_palette::palette::PickKind;
    use crate::ui::diff_review::DiffReviewAction;
    let repository = tempfile::tempdir().unwrap();
    let root = repository.path();
    let repo = git2::Repository::init(root).unwrap();
    std::fs::write(root.join("kept.txt"), "one\ntwo\n").unwrap();
    let mut index = repo.index().unwrap();
    index.add_path(std::path::Path::new("kept.txt")).unwrap();
    index.write().unwrap();
    let tree = repo.find_tree(index.write_tree().unwrap()).unwrap();
    let signature = git2::Signature::now("Test", "test@example.com").unwrap();
    repo.commit(Some("HEAD"), &signature, &signature, "start", &tree, &[])
        .unwrap();

    let mut tabs = TabManager::for_test(ShellConfig::default());
    tabs.new_tab_with_command(
        "/bin/sh",
        &["-c".into(), "sleep 30".into()],
        root.to_str(),
        None,
        None,
        HashMap::new(),
    )
    .unwrap();
    let mut app = App::new_headless(Config::default()).unwrap();
    app.tabs = Some(tabs);
    {
        // What noticing an agent in this terminal does.
        let tab = app.tabs.as_mut().unwrap().active_tab_mut().unwrap();
        let pane = tab.focused_terminal_identity().pane_id;
        let changes = &mut tab.pane_integration_mut(pane).unwrap().agent_changes;
        changes.start_capture(("codex", Some(1)), Some(root.to_path_buf()));
        let deadline = Instant::now() + Duration::from_secs(5);
        while !changes.poll() {
            assert!(Instant::now() < deadline, "the baseline was never captured");
            std::thread::sleep(Duration::from_millis(5));
        }
    }
    assert!(
        !app.unavailable_agent_commands()
            .contains(&"agent.review-changes".to_string())
    );
    let finish_job = |app: &mut App| {
        let deadline = Instant::now() + Duration::from_secs(5);
        while !app.poll_agent_change_job() {
            assert!(Instant::now() < deadline, "the Git job never finished");
            std::thread::sleep(Duration::from_millis(5));
        }
    };

    // The agent edits a file after it started.
    std::fs::write(root.join("kept.txt"), "one\nTWO\n").unwrap();
    app.review_agent_changes();
    finish_job(&mut app);
    assert_eq!(
        app.command_palette.active_pick(),
        Some(PickKind::AgentChange)
    );
    app.command_palette.close();

    app.pick_agent_change("0");
    finish_job(&mut app);
    let request = app.diff_review.request_id();
    assert!(app.diff_review.has_change_actions());
    assert!(
        app.diff_review
            .change_text(request)
            .is_some_and(|text| text.contains("-two\n") && text.contains("+TWO\n"))
    );

    app.apply_diff_review_action(DiffReviewAction::Stage {
        request_id: request,
    });
    finish_job(&mut app);
    assert_eq!(latest_toast(&app), Some("Staged kept.txt"));
    let mut index = repo.index().unwrap();
    index.read(true).unwrap();
    let staged = index.get_path(std::path::Path::new("kept.txt"), 0).unwrap();
    assert_eq!(repo.find_blob(staged.id).unwrap().content(), b"one\nTWO\n");
    // Staging reopens the list, which still shows the change.
    finish_job(&mut app);
    assert_eq!(
        app.command_palette.active_pick(),
        Some(PickKind::AgentChange)
    );
    app.command_palette.close();

    app.pick_agent_change("0");
    finish_job(&mut app);
    let request = app.diff_review.request_id();
    app.apply_diff_review_action(DiffReviewAction::Revert {
        request_id: request,
    });
    assert_eq!(
        std::fs::read_to_string(root.join("kept.txt")).unwrap(),
        "one\nTWO\n",
        "nothing is reverted before the person confirms"
    );
    let confirmation = app.info_overlay.request_id();
    app.apply_info_overlay_action(crate::ui::info_overlay::InfoOverlayAction::Choose {
        request_id: confirmation,
        choice_id: "agent-change-revert".into(),
    });
    finish_job(&mut app);
    assert_eq!(latest_toast(&app), Some("Reverted kept.txt"));
    assert_eq!(
        std::fs::read_to_string(root.join("kept.txt")).unwrap(),
        "one\ntwo\n"
    );
    finish_job(&mut app);
    assert_eq!(
        latest_toast(&app),
        Some("The agent has not changed any files since it started")
    );

    // Stage All stages everything that changed, including new files.
    std::fs::write(root.join("kept.txt"), "one\nthree\n").unwrap();
    std::fs::write(root.join("added.txt"), "new\n").unwrap();
    app.stage_all_agent_changes();
    finish_job(&mut app);
    assert_eq!(latest_toast(&app), Some("Staged 2 files"));
    index.read(true).unwrap();
    assert!(
        index
            .get_path(std::path::Path::new("added.txt"), 0)
            .is_some()
    );

    // A diff shown for one agent run cannot stage for the next.
    std::fs::write(root.join("kept.txt"), "once more\n").unwrap();
    app.review_agent_changes();
    finish_job(&mut app);
    app.command_palette.close();
    app.pick_agent_change("0");
    finish_job(&mut app);
    let request = app.diff_review.request_id();
    {
        let tab = app.tabs.as_mut().unwrap().active_tab_mut().unwrap();
        let pane = tab.focused_terminal_identity().pane_id;
        let changes = &mut tab.pane_integration_mut(pane).unwrap().agent_changes;
        changes.start_capture(("codex", Some(2)), Some(root.to_path_buf()));
        let deadline = Instant::now() + Duration::from_secs(5);
        while !changes.poll() {
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(5));
        }
    }
    app.apply_diff_review_action(DiffReviewAction::Stage {
        request_id: request,
    });
    assert_eq!(
        latest_toast(&app),
        Some("The agent has restarted since this diff was shown; review it again")
    );
    assert!(app.agent_change_job.is_none(), "nothing was started");
}

#[test]
fn the_agents_panel_lists_agents_by_urgency_and_shows_the_chosen_one() {
    use crate::ui::agents_panel::{AgentRowState, AgentsControl};
    use winit::keyboard::{Key, NamedKey};
    let (mut app, codex_tab, claude_tab) = two_agent_tabs();
    assert_eq!(app.tabs.as_ref().unwrap().active_tab_id(), Some(claude_tab));

    // Opening it closes another side panel, and it owns the pointer.
    app.commands_panel.toggle();
    app.toggle_agents_panel();
    assert!(!app.commands_panel.is_open());
    assert!(app.agents_panel.is_open());
    assert!(app.pointer_panel_owns_input());

    let info = app.agents_panel_info();
    assert_eq!(info.rows.len(), 2);
    assert_eq!(
        info.rows[0].name, "Claude Code",
        "the agent that needs you first"
    );
    assert_eq!(info.rows[0].state, AgentRowState::NeedsYou);
    assert_eq!(
        info.rows[0].summary,
        "Needs you · now · Approve shell: cargo test"
    );
    assert_eq!(info.rows[1].state, AgentRowState::Working);
    assert_eq!(info.focus, Some(AgentsControl::Agent(0)));

    // It is modal: text never reaches a terminal behind it.
    assert!(app.handle_agents_panel_key(&Key::Character("x".into())));
    assert!(app.agents_panel.is_open());

    // Down, then Enter, shows the second agent and closes the panel.
    app.handle_agents_panel_key(&Key::Named(NamedKey::ArrowDown));
    app.handle_agents_panel_key(&Key::Named(NamedKey::Enter));
    assert!(!app.agents_panel.is_open());
    assert_eq!(app.tabs.as_ref().unwrap().active_tab_id(), Some(codex_tab));

    // Escape closes it; with no agent and no supervisor, it is not offered.
    app.toggle_agents_panel();
    app.handle_agents_panel_key(&Key::Named(NamedKey::Escape));
    assert!(!app.agents_panel.is_open());
    assert!(
        !app.unavailable_agent_commands()
            .contains(&"agent.panel".to_string())
    );
    let empty = App::new_headless(Config::default()).unwrap();
    assert!(
        empty
            .unavailable_agent_commands()
            .contains(&"agent.panel".to_string())
    );
    assert!(
        !empty
            .unavailable_agent_commands()
            .contains(&"supervisor.activity".to_string())
            && !crate::ui::command_palette::palette::core_commands(false, &empty.shortcut_registry)
                .iter()
                .any(|command| command.id == "supervisor.activity"),
        "the panel replaces the activity dialog"
    );
}

#[test]
fn a_transcript_can_be_stopped_after_its_agent_exits() {
    let temp = tempfile::tempdir().unwrap();
    crate::session::set_test_sessions_base_dir(Some(temp.path().to_path_buf()));
    let mut tabs = TabManager::for_test(ShellConfig::default());
    tabs.new_tab_with_command(
        "/bin/sh",
        &["-c".into(), "sleep 30".into()],
        None,
        None,
        None,
        HashMap::new(),
    )
    .unwrap();
    let set_agent = |app: &mut App, agent: Option<crate::agents::AgentSession>| {
        let tab = app.tabs.as_mut().unwrap().active_tab_mut().unwrap();
        let pane_id = tab.focused_terminal_identity().pane_id;
        tab.pane_integration_mut(pane_id).unwrap().agent = agent;
    };
    let mut app = App::new_headless(Config::default()).unwrap();
    app.tabs = Some(tabs);
    set_agent(
        &mut app,
        Some(crate::agents::AgentSession {
            agent_id: "codex".into(),
            launched: false,
            profile: None,
            process: None,
        }),
    );
    app.toggle_transcript();
    assert_eq!(app.info_overlay.title(), "Transcript Started");

    // The agent exits while its transcript records.
    set_agent(&mut app, None);
    assert!(
        !app.unavailable_agent_commands()
            .contains(&"core.toggle-transcript".to_string()),
        "a recording can still be stopped"
    );
    app.toggle_transcript();
    assert_eq!(app.info_overlay.title(), "Transcript Stopped");
    assert!(
        app.unavailable_agent_commands()
            .contains(&"core.toggle-transcript".to_string()),
        "a new one needs an agent"
    );
    app.toggle_transcript();
    assert_eq!(
        latest_toast(&app),
        Some("Focus a writable local agent terminal to toggle its transcript")
    );
    crate::session::set_test_sessions_base_dir(None);
}

#[test]
fn a_read_only_agent_refuses_context_with_its_own_reason() {
    // `sleep` invoked as `codex`, so the terminal is recognized as Codex.
    let temp = tempfile::tempdir().unwrap();
    let codex = temp.path().join("codex");
    let sleep = ["/bin/sleep", "/usr/bin/sleep"]
        .into_iter()
        .find(|path| std::path::Path::new(path).exists())
        .unwrap();
    std::os::unix::fs::symlink(sleep, &codex).unwrap();
    let mut tabs = TabManager::for_test(ShellConfig::default());
    let tab_id = tabs
        .new_tab_with_command(
            codex.to_str().unwrap(),
            &["30".into()],
            None,
            None,
            None,
            HashMap::new(),
        )
        .unwrap();
    let tab = tabs.active_tab_mut().unwrap();
    let pane_id = tab.focused_terminal_identity().pane_id;
    tab.pane_integration_mut(pane_id).unwrap().agent = Some(crate::agents::AgentSession {
        agent_id: "codex".into(),
        launched: false,
        profile: None,
        process: None,
    });
    assert!(tab.toggle_focused_pane_read_only());
    let mut app = App::new_headless(Config::default()).unwrap();
    app.tabs = Some(tabs);
    let target = super::agents::AgentTarget {
        identity: crate::tabs::TerminalIdentity { tab_id, pane_id },
        agent_id: "codex".into(),
        agent_name: "Codex".into(),
    };
    let deadline = Instant::now() + Duration::from_secs(5);
    while app.recheck_agent_target(&target).is_err() {
        assert!(Instant::now() < deadline, "Codex was never recognized");
        std::thread::sleep(Duration::from_millis(5));
    }

    let sent = app.send_to_agent(
        super::agents::AgentTarget {
            identity: crate::tabs::TerminalIdentity { tab_id, pane_id },
            agent_id: "codex".into(),
            agent_name: "Codex".into(),
        },
        "context",
    );
    assert!(!sent);
    assert_eq!(latest_toast(&app), Some("Codex's terminal is read-only"));
}

#[test]
fn fullscreen_is_window_local_transient_state_for_shortcut_and_palette_routes() {
    let mut first = App::new_headless(Config::default()).unwrap();
    let second = App::new_headless(Config::default()).unwrap();

    assert!(!first.fullscreen_requested);
    assert!(!second.fullscreen_requested);
    assert!(first.handle_action(Action::ToggleFullscreen));
    assert!(first.fullscreen_requested);
    assert!(!second.fullscreen_requested);

    first.command_palette.open();
    let result = first
        .command_palette
        .execute_command("core.toggle-fullscreen")
        .unwrap();
    first.handle_palette_result(result);
    assert!(!first.fullscreen_requested);
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

#[test]
fn sending_to_an_agent_requires_one_to_be_running() {
    let app = App::new_headless(Config::default()).unwrap();
    assert_eq!(
        app.preferred_agent_target().unwrap_err(),
        "Start an agent before using Explain"
    );
}

#[test]
fn context_output_comes_from_a_shell_never_from_another_agent() {
    let mut app = App::new_headless(Config::default()).unwrap();
    let mut tabs = TabManager::for_test(ShellConfig::default());
    let mut open = |text: &str, agent: Option<&str>| {
        let tab_id = tabs
            .new_tab_with_command(
                "/bin/sh",
                &["-c".into(), "sleep 30".into()],
                None,
                None,
                None,
                HashMap::new(),
            )
            .unwrap();
        let tab = tabs.active_tab_mut().unwrap();
        tab.focused_terminal_mut().unwrap().process(text.as_bytes());
        let pane_id = tab.focused_terminal_identity().pane_id;
        tab.pane_integration_mut(pane_id).unwrap().agent =
            agent.map(|agent_id| crate::agents::AgentSession {
                agent_id: agent_id.into(),
                launched: false,
                profile: None,
                process: None,
            });
        crate::tabs::TerminalIdentity { tab_id, pane_id }
    };
    let asking = open("ASKING_AGENT", Some("codex"));
    open("OTHER_AGENT", Some("claude-code"));
    open("\x1b[1;31mUSER_WORK\x1b[0m", None);
    app.tabs = Some(tabs);

    let output = app.context_source_output(asking).unwrap();
    assert!(output.contains("USER_WORK"), "{output:?}");
    assert!(!output.contains("AGENT"), "{output:?}");
    // Agents get the text, not the styling the screen was drawn with.
    assert!(!output.contains('\x1b'), "{output:?}");
}

#[test]
fn plugin_launch_never_runs_a_synchronous_version_probe() {
    use std::os::unix::fs::PermissionsExt;

    let temp = tempfile::tempdir().unwrap();
    let executable = temp.path().join("probe-sensitive-plugin");
    let invocation = temp.path().join("invocation");
    std::fs::write(
        &executable,
        r#"#!/bin/sh
if [ "${1-}" = "--version" ]; then
  printf 'version-probe\n' >> "$TERMERIC_PLUGIN_PROBE_LOG"
  sleep 5
  exit 0
fi
printf 'launch=%s\n' "${1-}" >> "$TERMERIC_PLUGIN_PROBE_LOG"
"#,
    )
    .unwrap();
    let mut permissions = std::fs::metadata(&executable).unwrap().permissions();
    permissions.set_mode(0o755);
    std::fs::set_permissions(&executable, permissions).unwrap();

    let mut app = App::new_headless(Config::default()).unwrap();
    app.tabs = Some(TabManager::new(
        80,
        24,
        ShellConfig::default(),
        100,
        termeric_core::DEFAULT_SCROLLBACK_MEMORY_BYTES,
    ));
    let started = Instant::now();
    app.execute_plugin_action(PluginAction::OpenTab {
        program: executable.display().to_string(),
        args: vec!["run".into()],
        cwd: Some(temp.path().display().to_string()),
        title: Some("Probe-safe fixture".into()),
        plugin_id: Some("fixture-plugin".into()),
        profile: Some("default".into()),
        env: HashMap::from([(
            "TERMERIC_PLUGIN_PROBE_LOG".into(),
            invocation.display().to_string(),
        )]),
    });
    assert!(
        started.elapsed() < Duration::from_secs(1),
        "plugin dispatch blocked on executable behavior"
    );
    wait_for_file_text(&invocation, "launch=run");
    assert!(
        !std::fs::read_to_string(invocation)
            .unwrap()
            .contains("version-probe")
    );
}

#[test]
fn plugin_spawn_failure_is_visible_without_a_preflight_process() {
    let mut app = App::new_headless(Config::default()).unwrap();
    app.tabs = Some(TabManager::new(
        80,
        24,
        ShellConfig::default(),
        100,
        termeric_core::DEFAULT_SCROLLBACK_MEMORY_BYTES,
    ));
    app.execute_plugin_action(PluginAction::OpenTab {
        program: "/definitely/missing/termeric-plugin-fixture".into(),
        args: vec![],
        cwd: None,
        title: None,
        plugin_id: Some("fixture-plugin".into()),
        profile: None,
        env: HashMap::new(),
    });

    assert!(app.info_overlay.is_active());
    assert!(app.info_overlay.title().contains("Failed to launch"));
    assert!(!app.info_overlay.lines().is_empty());
    assert_eq!(app.tabs.as_ref().unwrap().count(), 0);
}

#[test]
fn fake_agent_clis_exercise_every_agent_command_end_to_end() {
    use crate::ui::command_palette::palette::AgentPlacement;
    let temp = tempfile::tempdir().unwrap();
    let fake_cli = temp.path().join("fake-ai");
    let invocation_log = temp.path().join("fake-ai.log");
    std::fs::write(
        &fake_cli,
        r#"#!/bin/sh
printf 'launch\tcwd=%s\targv=%s\tcolumns=%s\tlines=%s\n' "$PWD" "$*" "${COLUMNS-}" "${LINES-}" >> "$TERMERIC_FAKE_AI_LOG"
printf '%s\r\n' 'FAKE_AI_RESPONSE_26' '--- a/example.txt' '+++ b/example.txt' '@@ -1 +1 @@' '-old' '+new' 'END_DIFF_26'
printf '%s\r\n' 'FAKE_AI_READY_26'
while :; do
  if IFS= read -r line; then
    printf 'stdin=%s\tplugin=%s\n' "$line" "$TERMERIC_FAKE_AI_PLUGIN" >> "$TERMERIC_FAKE_AI_LOG"
  else
    sleep 0.01
  fi
done
"#,
    )
    .unwrap();
    let mut permissions = std::fs::metadata(&fake_cli).unwrap().permissions();
    permissions.set_mode(0o755);
    std::fs::set_permissions(&fake_cli, permissions).unwrap();

    let mut config = Config::default();
    for agent in ["claude-code", "codex"] {
        config.agents.insert(
            agent.into(),
            crate::agents::AgentConfig {
                program: Some(fake_cli.display().to_string()),
                default_profile: Some("acceptance".into()),
                profiles: std::collections::BTreeMap::from([(
                    "acceptance".into(),
                    crate::agents::config::AgentProfileConfig {
                        env: std::collections::BTreeMap::from([
                            (
                                "TERMERIC_FAKE_AI_LOG".into(),
                                invocation_log.display().to_string(),
                            ),
                            ("TERMERIC_FAKE_AI_PLUGIN".into(), agent.into()),
                        ]),
                        ..Default::default()
                    },
                )]),
                ..Default::default()
            },
        );
    }

    let mut app = App::new_headless(config).unwrap();
    let mut tabs = TabManager::for_test(ShellConfig::default());
    tabs.new_tab_with_command(
        "/bin/sh",
        &["-c".into(), "sleep 30".into()],
        Some(temp.path().to_str().unwrap()),
        Some("acceptance-shell"),
        None,
        HashMap::new(),
    )
    .unwrap();
    app.tabs = Some(tabs);
    crate::session::set_test_sessions_base_dir(Some(temp.path().join("sessions")));

    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        for (agent_id, new_conversation, resume) in [
            ("claude-code", "/clear", "--continue"),
            ("codex", "/new", "resume --last"),
        ] {
            app.start_picked_agent(AgentPlacement::NewTab, agent_id);
            wait_for_plugin_fixture(&mut app, agent_id, "FAKE_AI_READY_26");
            let focused = app.focused_agent().expect("the started agent is focused");
            assert!(focused.session.launched);
            assert_eq!(focused.session.profile.as_deref(), Some("acceptance"));
            let hidden = app.unavailable_agent_commands();
            for id in super::agents::FOCUSED_AGENT_COMMANDS {
                // Copying needs an answer the agent's hooks have reported.
                let expected_hidden = *id == "agent.copy-last-response";
                assert_eq!(
                    hidden.iter().any(|hidden| hidden == id),
                    expected_hidden,
                    "{id}"
                );
            }

            app.run_agent_command("agent.new-conversation");
            // The command is typed now and submitted once it has settled.
            app.submit_due_agent_input(Instant::now() + super::agents::AGENT_SUBMIT_DELAY);
            wait_for_file_text(
                &invocation_log,
                &format!("stdin={new_conversation}\tplugin={agent_id}"),
            );

            app.run_agent_command("agent.review-diff");
            assert!(app.diff_review.is_active());
            let request_id = app.diff_review.request_id();
            app.diff_review.dismiss(request_id);
            app.run_agent_command("agent.copy-diff");
            assert_eq!(
                latest_toast(&app),
                Some("Copy failed: clipboard unavailable: a headless app has no desktop clipboard")
            );

            app.run_agent_command("agent.insert-context");
            assert!(app.context_picker.is_active());
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
            app.context_picker
                .begin_gather(request_id, Some("FAKE_CONTEXT_SOURCE_26".into()))
                .expect("fake context source should be selected");
            app.handle_context_gathered(request_id, Ok("FAKE_CONTEXT_PAYLOAD_26\n".into()));
            wait_for_file_text(
                &invocation_log,
                &format!("stdin=FAKE_CONTEXT_PAYLOAD_26\tplugin={agent_id}"),
            );

            app.toggle_transcript();
            assert!(
                app.tabs
                    .as_ref()
                    .and_then(TabManager::active_tab)
                    .is_some_and(|tab| tab.focused_integration().unwrap().transcript.is_some()),
                "{agent_id} transcript did not start"
            );
            app.run_agent_command("agent.export");
            let export_deadline = Instant::now() + Duration::from_secs(5);
            while app.session_export.is_some() && Instant::now() < export_deadline {
                app.poll_session_export();
                std::thread::sleep(Duration::from_millis(5));
            }
            assert!(latest_toast(&app).is_some_and(|message| message.contains("Exported")));
            app.toggle_transcript();

            app.run_agent_command("agent.restart");
            wait_for_plugin_fixture(&mut app, agent_id, "FAKE_AI_READY_26");

            app.start_picked_agent(AgentPlacement::Resume, agent_id);
            // An earlier tab of this agent already shows READY; wait for the
            // resumed launch itself.
            wait_for_file_text(&invocation_log, &format!("argv={resume}\t"));
        }

        let log = std::fs::read_to_string(&invocation_log).unwrap();
        assert!(
            log.lines()
                .filter(|line| line.starts_with("launch\t"))
                .all(|line| line.contains("columns=80\tlines=24")),
            "terminal geometry was not passed to the fake agent: {log}"
        );
        let cwd = temp.path().display();
        for (prefix, count) in [
            // Open and restart for each agent.
            (format!("launch\tcwd={cwd}\targv=\t"), 4),
            (format!("launch\tcwd={cwd}\targv=--continue\t"), 1),
            (format!("launch\tcwd={cwd}\targv=resume --last\t"), 1),
        ] {
            assert_eq!(
                log.lines().filter(|line| line.starts_with(&prefix)).count(),
                count,
                "fake agent launch contract mismatch for {prefix:?}: {log}"
            );
        }
    }));
    crate::session::set_test_sessions_base_dir(None);
    result.unwrap();
}

#[test]
fn the_supervisor_hears_about_agents_and_sends_only_as_its_mode_allows() {
    use crate::agent_view::transcript::Item;
    use crate::agents::AgentReport;
    use crate::agents::supervisor::SupervisorMode;
    use crate::control::protocol::AgentEventKind;
    use crate::ui::command_palette::palette::AgentPlacement;
    use std::os::unix::fs::PermissionsExt;
    assert!(
        crate::agents::find_executable("python3").is_some(),
        "python3 is required by this test"
    );
    let temp = tempfile::tempdir().unwrap();
    let log = temp.path().join("agents.log");
    // The worker is an agent in a terminal that logs what it is typed.
    let worker_program = temp.path().join("fake-worker");
    std::fs::write(
        &worker_program,
        r#"#!/bin/sh
printf 'READY_worker\r\n'
while :; do
  if IFS= read -r line; then
    printf 'stdin\t%s\n' "$line" >> "$FAKE_LOG"
  else
    sleep 0.01
  fi
done
"#,
    )
    .unwrap();
    std::fs::set_permissions(&worker_program, std::fs::Permissions::from_mode(0o755)).unwrap();
    // The supervisor runs in the agent view, here as the fake stream-json
    // agent.
    let fake_claude = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/agent_view/fake_claude.py");
    let mut config = Config::default();
    config.agents.insert(
        "claude-code".into(),
        crate::agents::AgentConfig {
            program: Some(fake_claude.display().to_string()),
            env: std::collections::BTreeMap::from([("FAKE_CLAUDE_DELAY".into(), "0".into())]),
            ..Default::default()
        },
    );
    config.agents.insert(
        "codex".into(),
        crate::agents::AgentConfig {
            program: Some(worker_program.display().to_string()),
            env: std::collections::BTreeMap::from([("FAKE_LOG".into(), log.display().to_string())]),
            ..Default::default()
        },
    );
    config.supervisor.mode = SupervisorMode::Assist;
    let mut app = App::new_headless(config).unwrap();
    app.supervisor_rules_path = temp.path().join("supervisor.md");
    let mut tabs = TabManager::for_test(ShellConfig::default());
    tabs.new_tab_with_command(
        "/bin/sh",
        &["-c".into(), "sleep 30".into()],
        temp.path().to_str(),
        None,
        None,
        HashMap::new(),
    )
    .unwrap();
    app.tabs = Some(tabs);

    app.start_picked_agent(AgentPlacement::NewTab, "codex");
    wait_for_plugin_fixture(&mut app, "codex", "READY_worker");
    let worker = app
        .tabs
        .as_ref()
        .unwrap()
        .active_tab()
        .unwrap()
        .focused_terminal_identity();
    app.start_supervisor();
    // Whether status hooks need setting up depends on this machine's own
    // agent settings; the dialog offering it is not under test here.
    if app.info_overlay.is_awaiting_current() {
        assert_eq!(app.info_overlay.title(), "Set up agent status");
        let request = app.info_overlay.request_id();
        app.apply_info_overlay_action(crate::ui::info_overlay::InfoOverlayAction::Dismiss {
            request_id: request,
        });
    }
    assert!(app.supervisor_running());
    let supervisor = app.supervisor_session().unwrap();
    // Its own tab, filled by its conversation.
    assert!(app.agent_view_fills_tab());
    assert_eq!(app.agent_view.state.active, Some(supervisor));
    let token = app.supervisor_token().unwrap().to_owned();
    let args = app
        .agent_view
        .session(supervisor)
        .unwrap()
        .program_args()
        .join(" ");
    assert!(args.contains(&format!(
        "\"mcp\",\"--process\",\"{}\",\"--supervisor\",\"{token}\"",
        std::process::id()
    )));
    assert!(args.contains("assist mode"));
    assert!(args.contains("mcp__termeric_supervisor__answer_approval"));
    assert_eq!(
        std::fs::read_to_string(temp.path().join("supervisor.md")).unwrap(),
        crate::agents::supervisor::DEFAULT_RULES
    );
    assert_eq!(
        app.supervised_agents(1).len(),
        1,
        "the supervisor does not oversee itself"
    );

    // What the supervisor was told, in order.
    let told = |app: &App| -> Vec<String> {
        app.agent_view
            .session(supervisor)
            .unwrap()
            .transcript()
            .items()
            .iter()
            .filter_map(|item| match item {
                Item::User { text, .. } => Some(text.clone()),
                _ => None,
            })
            .collect()
    };
    let wait_until_told = |app: &mut App, needle: &str| {
        let deadline = Instant::now() + Duration::from_secs(10);
        while !told(app).iter().any(|text| text.contains(needle)) {
            assert!(
                Instant::now() < deadline,
                "never told {needle:?}: {:?}",
                told(app)
            );
            app.process_agent_view_events();
            app.deliver_supervisor_messages();
            std::thread::sleep(Duration::from_millis(5));
        }
    };
    wait_until_told(&mut app, "Call list_agents");
    let flush = |app: &mut App| {
        app.submit_due_agent_input(Instant::now() + super::agents::AGENT_SUBMIT_DELAY)
    };
    // A worker needing input is told to the supervisor as a message, once
    // it has finished its turn.
    let worker_id = app
        .tabs
        .as_ref()
        .unwrap()
        .tab_by_id(worker.tab_id)
        .unwrap()
        .pane_terminal_id(worker.pane_id)
        .unwrap()
        .to_owned();
    assert!(app.record_agent_report(
        &worker_id,
        AgentReport {
            agent_id: "codex".into(),
            event: AgentEventKind::NeedsInput,
            detail: Some("Approve shell: cargo test".into()),
            last_message: None,
            transcript_path: None,
        },
        None
    ));
    wait_until_told(
        &mut app,
        &format!(
            "Termeric: Codex (window 1, tab {}, pane {}) needs input: Approve shell: cargo test",
            worker.tab_id, worker.pane_id.0
        ),
    );

    // Requests must come from the supervisor's own processes; its agent
    // process stands in for its MCP server.
    let supervisor_process = app.agent_view.session(supervisor).unwrap().process_id();
    assert!(supervisor_process.is_some());
    let send = |app: &mut App, token: &str, text: &str| {
        app.supervised_submit(
            token,
            supervisor_process,
            worker.tab_id as u64,
            worker.pane_id.0,
            "codex",
            text,
        )
    };
    // The token alone, from a process outside the supervisor, is not enough.
    assert_eq!(
        app.supervised_submit(
            &token,
            Some(std::process::id()),
            worker.tab_id as u64,
            worker.pane_id.0,
            "codex",
            "hello"
        )
        .unwrap_err(),
        "Only the supervisor Termeric started may send as the supervisor"
    );
    assert!(
        send(&mut app, &token, "!rm -rf target")
            .unwrap_err()
            .contains("run as a command")
    );
    // Assist: held as a card in the supervisor's conversation, sent once
    // approved.
    assert_eq!(send(&mut app, &token, "yes, run it"), Ok(true));
    let request = app.supervisor_request().unwrap();
    assert_eq!(
        (request.recipient.as_str(), request.text.as_str()),
        ("Codex", "yes, run it")
    );
    app.sync_agent_view_tab_bar();
    let supervisor_tab = app.agent_session_tab(supervisor).unwrap();
    assert_eq!(
        app.tabs
            .as_ref()
            .unwrap()
            .tab_by_id(supervisor_tab)
            .unwrap()
            .agent_badge(),
        Some(crate::agents::AgentBadge::NeedsYou)
    );
    assert!(
        send(&mut app, &token, "another")
            .unwrap_err()
            .contains("waiting for the person's approval")
    );
    app.answer_supervisor_request(true);
    assert!(app.supervisor_request().is_none());
    flush(&mut app);
    wait_for_file_text(&log, "stdin\tyes, run it");
    wait_until_told(
        &mut app,
        "the person approved your message to Codex; it was sent.",
    );

    // Declined messages count toward the limits too.
    assert_eq!(send(&mut app, &token, "try the fix"), Ok(true));
    app.answer_supervisor_request(false);
    wait_until_told(&mut app, "the person declined your message to Codex");
    assert!(
        send(&mut app, &token, "Try the  FIX")
            .unwrap_err()
            .contains("just sent"),
        "a declined message counts toward the limits"
    );

    // Taking over leaves the supervisor watching; handing back restores it.
    app.take_over_from_supervisor();
    assert!(
        send(&mut app, &token, "keep going")
            .unwrap_err()
            .contains("watch mode")
    );
    app.hand_back_to_supervisor();
    app.set_supervisor_mode(SupervisorMode::Auto);
    assert_eq!(send(&mut app, &token, "keep going"), Ok(false));
    flush(&mut app);
    wait_for_file_text(&log, "stdin\tkeep going");
    assert!(
        send(&mut app, &token, "keep going")
            .unwrap_err()
            .contains("just sent")
    );
    assert_eq!(
        app.supervised_session_submit(&token, supervisor_process, supervisor, "hi")
            .unwrap_err(),
        "The supervisor cannot send to itself"
    );
    assert!(send(&mut app, &"f".repeat(32), "x").is_err());

    app.stop_supervisor();
    assert!(!app.supervisor_running());
    assert!(app.agent_view.session(supervisor).is_none());
    assert!(send(&mut app, &token, "x").is_err());
    assert!(
        app.tabs
            .as_ref()
            .unwrap()
            .tab_by_id(supervisor_tab)
            .is_none_or(|tab| tab.closed || tab.is_closing()),
        "the supervisor's own tab closes with it"
    );
}

#[test]
fn codex_supervises_as_an_agent_view_conversation() {
    use crate::agent_view::agent::AgentKind;
    use crate::agent_view::transcript::{Activity, Item};
    use crate::agents::supervisor::SupervisorMode;

    assert!(
        crate::agents::find_executable("python3").is_some(),
        "python3 is required by this test"
    );
    let temp = tempfile::tempdir().unwrap();
    let fake_codex = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/agent_view/fake_codex.py");
    let mut config = Config::default();
    config.agents.insert(
        "codex".into(),
        crate::agents::AgentConfig {
            program: Some(fake_codex.display().to_string()),
            env: std::collections::BTreeMap::from([("FAKE_CODEX_DELAY".into(), "0".into())]),
            ..Default::default()
        },
    );
    config.supervisor.agent = "codex".into();
    config.supervisor.mode = SupervisorMode::Watch;
    let mut app = App::new_headless(config).unwrap();
    app.supervisor_rules_path = temp.path().join("supervisor.md");
    let mut tabs = TabManager::for_test(ShellConfig::default());
    tabs.new_tab().unwrap();
    app.tabs = Some(tabs);

    app.start_supervisor();
    if app.info_overlay.is_awaiting_current() {
        let request = app.info_overlay.request_id();
        app.apply_info_overlay_action(crate::ui::info_overlay::InfoOverlayAction::Dismiss {
            request_id: request,
        });
    }
    assert!(app.supervisor_running());
    let supervisor = app.supervisor_session().unwrap();
    let session = app.agent_view.session(supervisor).unwrap();
    assert_eq!(session.agent(), AgentKind::Codex);
    // Codex takes its server and instructions as configuration overrides.
    let args = session.program_args().join(" ");
    assert!(args.contains("mcp_servers.termeric_supervisor.command="));
    assert!(args.contains("developer_instructions="));
    assert!(args.contains("watch mode"));

    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        app.process_agent_view_events();
        let session = app.agent_view.session(supervisor).unwrap();
        if session.activity() == (Activity::Finished { success: true }) {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "{:#?}",
            session.transcript().items()
        );
        std::thread::sleep(Duration::from_millis(5));
    }
    let transcript = app.agent_view.session(supervisor).unwrap().transcript();
    assert!(transcript.items().iter().any(|item| matches!(
        item,
        Item::User { text, .. } if text.starts_with("Call list_agents")
    )));
    app.stop_supervisor();
    assert!(!app.supervisor_running());
}

#[test]
fn earlier_claude_conversations_come_from_the_configured_directory() {
    let claude = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let folder = claude
        .path()
        .join("projects")
        .join(crate::agent_view::history::project_folder_name(cwd.path()));
    std::fs::create_dir_all(&folder).unwrap();
    std::fs::write(
        folder.join("earlier.jsonl"),
        "{\"type\":\"user\",\"message\":{\"content\":\"Plan the release\"}}\n",
    )
    .unwrap();
    let mut config = Config::default();
    config.agents.insert(
        "claude-code".into(),
        crate::agents::AgentConfig {
            env: [(
                "CLAUDE_CONFIG_DIR".to_owned(),
                claude.path().display().to_string(),
            )]
            .into(),
            ..Default::default()
        },
    );
    let mut app = App::new_headless(config).unwrap();
    let past = app.agent_view_past(
        crate::agent_view::agent::AgentKind::ClaudeCode,
        cwd.path(),
        true,
    );
    assert_eq!(past.len(), 1);
    assert_eq!(past[0].title, "Plan the release");
}
