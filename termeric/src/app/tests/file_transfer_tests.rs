//! Dropped files, live-domain uploads, and Kitty file-transfer consent.

use super::*;
use crate::app::file_drop::{FileUploadOutcome, FileUploadSourceKind};

#[test]
fn local_file_drop_is_quoted_and_refuses_unsafe_terminal_routes() {
    let config = Config {
        shell: crate::test_support::sleeping_shell(),
        ..Config::default()
    };
    let mut app = App::new_headless_with_tabs(config);

    let path = std::path::Path::new("/tmp/project file's.txt");
    assert_eq!(
        app.dropped_path_payload(path).unwrap(),
        "'/tmp/project file'\\''s.txt' "
    );

    app.tabs
        .as_mut()
        .unwrap()
        .active_tab_mut()
        .unwrap()
        .tab_type = crate::tabs::TabType::Serial {
        name: "fixture".into(),
    };
    assert!(
        app.dropped_path_payload(path)
            .unwrap_err()
            .contains("serial terminal")
    );

    app.tabs
        .as_mut()
        .unwrap()
        .active_tab_mut()
        .unwrap()
        .tab_type = crate::tabs::TabType::Shell;
    let tab_id = app.tabs.as_ref().unwrap().active_tab().unwrap().id;
    app.broadcast = BroadcastState::Armed {
        tab_id,
        expires: Instant::now() + Duration::from_secs(1),
        targets: Vec::new(),
    };
    assert!(
        app.dropped_path_payload(path)
            .unwrap_err()
            .contains("broadcast")
    );
}

#[test]
fn a_wayland_drop_handles_each_file_and_says_why_one_was_refused() {
    let config = Config {
        shell: crate::test_support::sleeping_shell(),
        ..Config::default()
    };
    let mut app = App::new_headless_with_tabs(config);
    let messages = |app: &App| -> Vec<String> {
        app.toasts
            .iter()
            .map(|toast| toast.message.clone())
            .collect()
    };

    app.handle_window_result(WindowResult::FilesDropped(Err(
        "only files on this computer can be dropped".into(),
    )));
    assert_eq!(
        messages(&app),
        ["File was not dropped: only files on this computer can be dropped"]
    );

    app.handle_window_result(WindowResult::FilesDropped(Ok(vec![
        PathBuf::from("/tmp/one"),
        PathBuf::from("relative"),
    ])));
    let messages = messages(&app);
    assert_eq!(messages.len(), 3, "{messages:?}");
    assert_eq!(
        messages[1],
        "Inserted quoted local path; press Enter to use it"
    );
    assert!(messages[2].contains("non-absolute"), "{messages:?}");
}

#[test]
fn local_file_drop_writes_one_bracketed_payload_without_enter() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("project file's.txt");
    std::fs::write(&path, b"contents-must-not-be-read").unwrap();
    let path_text = path.to_str().unwrap();
    let quoted = format!("'{}' ", path_text.replace('\'', "'\\''"));
    let mut expected = b"\x1b[200~".to_vec();
    expected.extend_from_slice(quoted.as_bytes());
    expected.extend_from_slice(b"\x1b[201~");
    let script = format!(
        "stty raw -echo; printf '\rREADY\r\n'; \
         bytes=$(dd bs=1 count={} 2>/dev/null | od -An -tx1); \
         next=$(dd bs=1 count=1 2>/dev/null | od -An -tx1); \
         printf '\rdrop=%s next=%s\r\n' \"$bytes\" \"$next\"; sleep 2",
        expected.len()
    );
    let mut config = Config::default();
    config.shell.program = Some("/bin/sh".into());
    config.shell.args = vec!["-c".into(), script];
    let mut app = App::new_headless_with_tabs(config);
    app.active_terminal_mut()
        .unwrap()
        .set_mode(termeric_core::term::TerminalMode::BRACKETED_PASTE, true);

    app.wait_for_screen_text("READY");

    assert!(app.handle_dropped_path(&path));
    assert!(app.write_terminal_protocol_input(b"X"));
    let expected_hex = expected
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<Vec<_>>()
        .concat();
    // The hex dump wraps across rows, so match it with whitespace removed.
    let report = format!("drop={expected_hex}next=58");
    let mut output = String::new();
    let arrived = crate::test_support::poll_until(Duration::from_secs(3), || {
        app.process_pty_events();
        output = app.active_terminal().map_or_else(String::new, |terminal| {
            terminal.grid().plain_lines().join("\n")
        });
        output
            .chars()
            .filter(|character| !character.is_whitespace())
            .collect::<String>()
            .contains(&report)
    });
    assert!(
        arrived,
        "exact drop payload did not arrive without Enter: {output:?}"
    );
}

#[test]
fn live_domain_drop_requires_request_scoped_confirmation_without_reading_contents() {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("upload λ.bin");
    std::fs::write(&source, b"never sent by this headless test").unwrap();
    let config = Config {
        shell: crate::test_support::sleeping_shell(),
        ..Config::default()
    };
    let mut app = App::new_headless_with_tabs(config);
    // Each dismissed confirmation must be gone before the next request,
    // without waiting out its fade.
    app.dialog.set_reduced_motion(true);
    app.live_domain_authority = Some(crate::domain::DomainAuthority::Local);
    app.tabs
        .as_mut()
        .unwrap()
        .active_tab_mut()
        .unwrap()
        .live_domain = Some(crate::tabs::LiveDomainTab {
        domain_id: "domain-upload".into(),
        tab_id: 7,
        pane_ids: HashMap::from([(PaneId(0), 42)]),
    });

    assert!(app.dropped_path_allowed(&source));
    assert!(app.handle_dropped_path(&source));
    let first_request = app.dialog.request_id();
    assert!(app.dialog.is_awaiting(first_request));
    assert!(app.pending_file_upload().is_some_and(|pending| {
        pending.request_id == first_request
            && pending.domain_id == "domain-upload"
            && pending.pane_id == 42
            && pending.name == "upload λ.bin"
            && pending.source_kind == FileUploadSourceKind::File { bytes: 32 }
    }));
    app.apply_dialog_action(crate::ui::dialog::DialogAction::Answer {
        request_id: first_request,
        id: crate::ui::dialog::CANCEL.into(),
    });
    assert!(app.pending_file_upload().is_none());
    assert!(app.active_file_upload.is_none());

    assert!(app.handle_dropped_path(&source));
    let second_request = app.dialog.request_id();
    app.apply_dialog_action(crate::ui::dialog::DialogAction::Answer {
        request_id: first_request,
        id: "file-upload-confirm".into(),
    });
    assert!(
        app.pending_file_upload().is_some(),
        "stale confirmation acted"
    );
    app.apply_dialog_action(crate::ui::dialog::DialogAction::Answer {
        request_id: second_request,
        id: "file-upload-confirm".into(),
    });
    assert!(app.pending_file_upload().is_none());
    assert!(app.active_file_upload.is_none());
    assert!(latest_toast(&app).is_some_and(|message| message.contains("native window")));

    let directory = temp.path().join("project λ");
    std::fs::create_dir(&directory).unwrap();
    assert!(app.handle_dropped_path(&directory));
    let directory_request = app.dialog.request_id();
    assert!(app.pending_file_upload().is_some_and(|pending| {
        pending.request_id == directory_request
            && pending.name == "project λ"
            && pending.source_kind == FileUploadSourceKind::Directory
    }));
    app.apply_dialog_action(crate::ui::dialog::DialogAction::Answer {
        request_id: directory_request,
        id: crate::ui::dialog::CANCEL.into(),
    });
    assert!(app.pending_file_upload().is_none());
}

#[test]
fn letter_keys_neither_answer_nor_copy_the_upload_confirmation() {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("upload.bin");
    std::fs::write(&source, b"never sent").unwrap();
    let config = Config {
        shell: crate::test_support::sleeping_shell(),
        ..Config::default()
    };
    let mut app = App::new_headless_with_tabs(config);
    let (clipboard, memory) = super::super::clipboard::Clipboard::in_memory();
    app.clipboard = clipboard;
    app.live_domain_authority = Some(crate::domain::DomainAuthority::Local);
    app.tabs
        .as_mut()
        .unwrap()
        .active_tab_mut()
        .unwrap()
        .live_domain = Some(crate::tabs::LiveDomainTab {
        domain_id: "domain-upload".into(),
        tab_id: 7,
        pane_ids: HashMap::from([(PaneId(0), 42)]),
    });

    assert!(app.handle_dropped_path(&source));
    let request_id = app.dialog.request_id();
    assert!(app.dialog.is_awaiting(request_id));
    for letter in ["c", "u", "y"] {
        app.handle_dialog_key(&winit::keyboard::Key::Character(letter.into()), false);
    }
    assert!(app.dialog.is_awaiting(request_id));
    assert!(app.pending_file_upload().is_some());
    assert_eq!(memory.lock().unwrap().clipboard, None);

    // Escape is the way out.
    app.handle_dialog_key(
        &winit::keyboard::Key::Named(winit::keyboard::NamedKey::Escape),
        false,
    );
    assert!(!app.dialog.is_awaiting(request_id));
    assert!(app.pending_file_upload().is_none());
}

fn kitty_send_request(id: &str) -> termeric_core::KittyFileTransferCommand {
    termeric_core::KittyFileTransferCommand {
        reply_uses_8bit_controls: true,
        ..super::super::kitty_file_transfer::test_transfer_command(
            termeric_core::KittyFileTransferAction::Send,
            id,
        )
    }
}

#[test]
fn drops_reach_agent_panes_but_program_transfers_follow_each_panes_launch() {
    let config = Config {
        shell: crate::test_support::sleeping_shell(),
        ..Config::default()
    };
    let mut app = App::new_headless_with_tabs(config);
    let path = std::path::Path::new("/tmp/project.txt");
    let kitty_request_awaits_consent = |app: &mut App, tab_id, pane_id, id: &str| {
        app.handle_terminal_event(RoutedTerminalEvent {
            origin: TerminalEventOrigin { tab_id, pane_id },
            event: TerminalEvent::KittyFileTransfer(kitty_send_request(id)),
        });
        app.kitty_file_transfers.pending.is_some()
    };
    let agent_launch = crate::panes::LocalPaneLaunch {
        identity: crate::workspace::WorkspaceLaunch::Plugin {
            plugin_id: "agent-fixture".into(),
            profile: None,
        },
        shell: crate::test_support::sleeping_shell(),
        cwd: None,
    };

    // An agent split into a shell tab takes a drop like a shell, but its
    // program may not start a transfer, though its tab is a shell.
    let tabs = app.tabs.as_mut().unwrap();
    let shell_tab = tabs.active_tab_id().unwrap();
    let agent_pane = tabs
        .create_plugin_pane(
            tabs.active_tab().unwrap().focused_terminal_identity(),
            SplitDirection::Vertical,
            &agent_launch,
        )
        .unwrap();
    tabs.active_tab_mut().unwrap().panes.focus(agent_pane);
    assert_eq!(
        app.dropped_path_payload(path).unwrap(),
        "'/tmp/project.txt' "
    );
    assert!(!kitty_request_awaits_consent(
        &mut app,
        shell_tab,
        agent_pane,
        "agent-in-shell-tab"
    ));
    assert!(!app.dialog.is_active());

    // The agent of an agent tab takes a drop and is refused a transfer; a
    // shell split into that tab is accepted for both, though its tab is not.
    let tabs = app.tabs.as_mut().unwrap();
    tabs.new_tab_with_command(
        "/bin/sh",
        &crate::test_support::sleeping_args(),
        None,
        Some("agent-tab"),
        Some(crate::tabs::TabType::Plugin {
            plugin_id: "agent-fixture".into(),
        }),
        HashMap::new(),
    )
    .unwrap();
    let agent_tab = tabs.active_tab_id().unwrap();
    let agent_tab_pane = tabs
        .active_tab()
        .unwrap()
        .focused_terminal_identity()
        .pane_id;
    assert!(matches!(
        tabs.active_tab().unwrap().focused_pane().unwrap().launch,
        crate::workspace::WorkspaceLaunch::Plugin { .. }
    ));
    assert_eq!(
        app.dropped_path_payload(path).unwrap(),
        "'/tmp/project.txt' "
    );
    assert!(!kitty_request_awaits_consent(
        &mut app,
        agent_tab,
        agent_tab_pane,
        "agent-tab"
    ));
    assert!(!app.dialog.is_active());
    let panes = &mut app.tabs.as_mut().unwrap().active_tab_mut().unwrap().panes;
    panes.adopt_primary_shell(crate::test_support::sleeping_shell());
    let shell_pane = panes
        .split(
            SplitDirection::Vertical,
            crate::config::PaneCwdMode::Focused,
        )
        .unwrap();
    panes.focus(shell_pane);
    assert_eq!(
        app.dropped_path_payload(path).unwrap(),
        "'/tmp/project.txt' "
    );
    assert!(kitty_request_awaits_consent(
        &mut app,
        agent_tab,
        shell_pane,
        "shell-in-agent-tab"
    ));
}

#[test]
fn serial_tabs_refuse_drops_and_program_transfers() {
    let config = Config {
        shell: crate::test_support::sleeping_shell(),
        ..Config::default()
    };
    let mut app = App::new_headless_with_tabs(config);
    let tab = app.tabs.as_mut().unwrap().active_tab_mut().unwrap();
    tab.set_serial_profile_launch("fixture", "Serial fixture".into());
    let identity = tab.focused_terminal_identity();

    let error = app
        .dropped_path_payload(std::path::Path::new("/tmp/project.txt"))
        .unwrap_err();
    assert!(error.contains("serial terminals"), "{error}");
    app.handle_terminal_event(RoutedTerminalEvent {
        origin: TerminalEventOrigin {
            tab_id: identity.tab_id,
            pane_id: identity.pane_id,
        },
        event: TerminalEvent::KittyFileTransfer(kitty_send_request("serial")),
    });
    assert!(app.kitty_file_transfers.pending.is_none());
    assert!(!app.dialog.is_active());
}

#[test]
fn kitty_download_consent_is_request_scoped_and_terminal_type_gated() {
    let config = Config {
        shell: crate::test_support::sleeping_shell(),
        ..Config::default()
    };
    let mut app = App::new_headless_with_tabs(config);
    // Each dismissed confirmation must be gone before the next request,
    // without waiting out its fade.
    app.dialog.set_reduced_motion(true);
    let tab_id = app.tabs.as_ref().unwrap().active_tab_id().unwrap();
    let origin = TerminalEventOrigin {
        tab_id,
        pane_id: PaneId(0),
    };
    let request = kitty_send_request("download-1");

    app.handle_terminal_event(RoutedTerminalEvent {
        origin,
        event: TerminalEvent::KittyFileTransfer(request.clone()),
    });
    let confirmation = app.dialog.request_id();
    assert!(app.dialog.is_awaiting(confirmation));
    // Deny is where focus starts, so a stray Enter refuses.
    assert_eq!(
        app.dialog.focus(),
        Some(crate::ui::dialog::DialogFocus::Button(0))
    );
    assert_eq!(app.dialog.buttons()[0].label, "Deny");
    assert!(
        app.kitty_file_transfers
            .pending
            .as_ref()
            .is_some_and(|pending| {
                pending.request_id == confirmation
                    && Some(pending.key.origin)
                        == app.tabs.as_ref().unwrap().identity_for_origin(origin)
                    && pending.key.id == "download-1"
                    && pending.reply_uses_8bit_controls
            })
    );
    app.apply_dialog_action(crate::ui::dialog::DialogAction::Answer {
        request_id: confirmation.wrapping_add(1),
        id: "kitty-download-allow".into(),
    });
    assert!(app.kitty_file_transfers.pending.is_some());
    app.apply_dialog_action(crate::ui::dialog::DialogAction::Answer {
        request_id: confirmation,
        id: crate::ui::dialog::CANCEL.into(),
    });
    assert!(app.kitty_file_transfers.pending.is_none());
    assert!(app.kitty_file_transfers.active.is_empty());

    let mut stale_request = request.clone();
    stale_request.id = "download-stale".into();
    app.handle_terminal_event(RoutedTerminalEvent {
        origin,
        event: TerminalEvent::KittyFileTransfer(stale_request),
    });
    assert!(app.kitty_file_transfers.pending.is_some());
    let replacement = app.dialog.show(crate::ui::dialog::DialogSpec::message(
        "Replacement",
        vec![],
    ));
    assert!(app.prune_stale_kitty_file_transfers());
    assert!(app.kitty_file_transfers.pending.is_none());
    app.dialog.dismiss(replacement);

    app.tabs
        .as_mut()
        .unwrap()
        .active_tab_mut()
        .unwrap()
        .tab_type = crate::tabs::TabType::Serial {
        name: "fixture".into(),
    };
    let mut serial_request = request;
    serial_request.id = "serial-download".into();
    app.handle_terminal_event(RoutedTerminalEvent {
        origin,
        event: TerminalEvent::KittyFileTransfer(serial_request),
    });
    assert!(!app.dialog.is_active());
    assert!(app.kitty_file_transfers.pending.is_none());
    assert!(app.kitty_file_transfers.active.is_empty());
}

#[test]
fn kitty_upload_collects_exact_paths_before_request_scoped_consent() {
    let config = Config {
        shell: crate::test_support::sleeping_shell(),
        ..Config::default()
    };
    let mut app = App::new_headless_with_tabs(config);
    let tab_id = app.tabs.as_ref().unwrap().active_tab_id().unwrap();
    let origin = TerminalEventOrigin {
        tab_id,
        pane_id: PaneId(0),
    };
    let command =
        |action, file_id: &str, name: Option<&str>, size| termeric_core::KittyFileTransferCommand {
            file_id: file_id.into(),
            size,
            name: name.map(str::to_owned),
            reply_uses_8bit_controls: true,
            ..super::super::kitty_file_transfer::test_transfer_command(action, "upload-1")
        };

    app.handle_terminal_event(RoutedTerminalEvent {
        origin,
        event: TerminalEvent::KittyFileTransfer(command(
            termeric_core::KittyFileTransferAction::Receive,
            "",
            None,
            Some(2),
        )),
    });
    assert!(app.kitty_file_transfers.pending_upload.is_some());
    assert!(!app.dialog.is_active());

    app.handle_terminal_event(RoutedTerminalEvent {
        origin,
        event: TerminalEvent::KittyFileTransfer(command(
            termeric_core::KittyFileTransferAction::File,
            "f1",
            Some("/tmp/approved-one.txt"),
            None,
        )),
    });
    assert!(!app.dialog.is_active());
    app.handle_terminal_event(RoutedTerminalEvent {
        origin,
        event: TerminalEvent::KittyFileTransfer(command(
            termeric_core::KittyFileTransferAction::File,
            "f2",
            Some("~/approved-two.txt"),
            None,
        )),
    });

    let request_id = app.dialog.request_id();
    assert!(app.dialog.is_awaiting(request_id));
    assert!(app.dialog.kind().is_alert());
    let notice = app.dialog_text();
    assert!(notice.contains("/tmp/approved-one.txt"));
    assert!(notice.contains("approved-two.txt"));
    app.apply_dialog_action(crate::ui::dialog::DialogAction::Answer {
        request_id,
        id: crate::ui::dialog::CANCEL.into(),
    });
    assert!(app.kitty_file_transfers.pending_upload.is_none());
    assert!(app.kitty_file_transfers.active_uploads.is_empty());
}

#[test]
fn kitty_transfer_confirmation_never_replies_to_a_replacement_owner() {
    let (mut app, directory, entry, _) = harvest_rerun_capture_fixture_in(false);
    app.handle_kitty_file_transfer(
        TerminalEventOrigin {
            tab_id: entry.source.tab_id,
            pane_id: PaneId(0),
        },
        kitty_capture_command(termeric_core::KittyFileTransferAction::Send),
    );
    let request_id = app.dialog.request_id();
    assert!(app.kitty_file_transfers.pending.is_some());
    split_harvest_capture_owner(&mut app, directory.path());
    app.close_pane_for(Requester::Control, entry.source.tab_id as u64, 0)
        .unwrap();
    let captures = current_harvest_capture_routes(&mut app, directory.path());
    app.handle_kitty_download_choice(request_id, "kitty-download-allow");
    assert!(app.kitty_file_transfers.pending.is_none());
    assert!(app.kitty_file_transfers.active.is_empty());
    assert!(
        fenced_harvest_capture(&mut app, &captures)
            .iter()
            .all(Vec::is_empty),
        "a pending transfer must never reply to the surviving pane"
    );
}

#[test]
fn file_upload_completion_is_request_scoped_and_distinguishes_partial_success() {
    let mut app = App::new_headless(Config::default()).unwrap();
    let progress_toast_id = app.toasts.persistent_info("uploading");
    app.active_file_upload = Some(ActiveFileUpload {
        request_id: 9,
        name: "artifact.bin".into(),
        bytes_sent: 0,
        total_bytes: Some(42),
        cancellation: Arc::new(AtomicBool::new(false)),
        cancel_requested: false,
        progress_toast_id,
    });
    app.handle_file_upload_completed(
        8,
        Ok(FileUploadCompletion::Uploaded(FileUploadOutcome {
            path: "/remote/stale.bin".into(),
            bytes: 1,
            entries: None,
            insertion_error: None,
        })),
    );
    assert_eq!(
        app.active_file_upload
            .as_ref()
            .map(|active| active.request_id),
        Some(9)
    );
    assert_eq!(latest_toast(&app), Some("uploading"));

    app.handle_file_upload_completed(
        9,
        Ok(FileUploadCompletion::Uploaded(FileUploadOutcome {
            path: "/remote/artifact.bin".into(),
            bytes: 42,
            entries: None,
            insertion_error: Some("pane closed".into()),
        })),
    );
    assert!(app.active_file_upload.is_none());
    assert!(latest_toast(&app).is_some_and(|message| {
        message.contains("Uploaded 42 bytes")
            && message.contains("artifact.bin")
            && message.contains("pane closed")
    }));
}

#[test]
fn file_upload_progress_and_cancellation_are_exact_and_monotonic() {
    let mut app = App::new_headless(Config::default()).unwrap();
    let progress_toast_id = app.toasts.persistent_info("uploading");
    let cancellation = Arc::new(AtomicBool::new(false));
    app.active_file_upload = Some(ActiveFileUpload {
        request_id: 17,
        name: "archive.tar".into(),
        bytes_sent: 10,
        total_bytes: Some(100),
        cancellation: Arc::clone(&cancellation),
        cancel_requested: false,
        progress_toast_id,
    });

    app.handle_file_upload_progress(16, 80, 100);
    app.handle_file_upload_progress(17, 9, 100);
    app.handle_file_upload_progress(17, 50, 101);
    assert_eq!(app.active_file_upload.as_ref().unwrap().bytes_sent, 10);

    app.handle_file_upload_progress(17, 50, 100);
    assert_eq!(app.active_file_upload.as_ref().unwrap().bytes_sent, 50);
    assert!(latest_toast(&app).is_some_and(|message| message.contains("50%")));

    app.cancel_file_upload();
    assert!(cancellation.load(Ordering::Acquire));
    assert!(app.active_file_upload.as_ref().unwrap().cancel_requested);
    assert!(latest_toast(&app).is_some_and(|message| message.contains("Cancelling")));

    app.handle_file_upload_progress(17, 75, 100);
    assert_eq!(app.active_file_upload.as_ref().unwrap().bytes_sent, 75);
    assert!(latest_toast(&app).is_some_and(|message| message.contains("Cancelling")));

    app.handle_file_upload_completed(
        17,
        Ok(FileUploadCompletion::Cancelled { bytes_received: 50 }),
    );
    assert!(app.active_file_upload.is_none());
    assert!(latest_toast(&app).is_some_and(|message| {
        message.contains("Cancelled upload of archive.tar")
            && message.contains("50/100 bytes")
            && message.contains("no destination was published")
    }));
}

#[test]
fn directory_upload_progress_becomes_determinate_after_local_preparation() {
    let mut app = App::new_headless(Config::default()).unwrap();
    let progress_toast_id = app.toasts.persistent_info("preparing directory");
    app.active_file_upload = Some(ActiveFileUpload {
        request_id: 18,
        name: "project".into(),
        bytes_sent: 0,
        total_bytes: None,
        cancellation: Arc::new(AtomicBool::new(false)),
        cancel_requested: false,
        progress_toast_id,
    });

    app.handle_file_upload_progress(18, 0, 200);
    assert_eq!(
        app.active_file_upload
            .as_ref()
            .and_then(|active| active.total_bytes),
        Some(200)
    );
    assert!(latest_toast(&app).is_some_and(|message| message.contains("0%")));
    app.handle_file_upload_progress(18, 100, 201);
    assert_eq!(app.active_file_upload.as_ref().unwrap().bytes_sent, 0);
    app.handle_file_upload_progress(18, 100, 200);
    assert!(latest_toast(&app).is_some_and(|message| message.contains("50%")));
}

#[test]
fn empty_upload_progress_is_complete_without_dividing_by_zero() {
    let mut app = App::new_headless(Config::default()).unwrap();
    let progress_toast_id = app.toasts.persistent_info("uploading");
    app.active_file_upload = Some(ActiveFileUpload {
        request_id: 19,
        name: "empty.txt".into(),
        bytes_sent: 0,
        total_bytes: Some(0),
        cancellation: Arc::new(AtomicBool::new(false)),
        cancel_requested: false,
        progress_toast_id,
    });
    app.handle_file_upload_progress(19, 0, 0);
    assert!(latest_toast(&app).is_some_and(|message| message.contains("100% (0/0 bytes)")));
    assert_eq!(app.active_file_upload.as_ref().unwrap().bytes_sent, 0);
}
