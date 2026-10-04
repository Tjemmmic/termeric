//! Dropped files, live-domain uploads, and Kitty file-transfer consent.

use super::*;

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

    let active_tab = app.tabs.as_mut().unwrap().active_tab_mut().unwrap();
    active_tab.tab_type = crate::tabs::TabType::Plugin {
        plugin_id: "hermetic-fixture".into(),
    };
    assert!(
        app.dropped_path_payload(path)
            .unwrap_err()
            .contains("data disclosure")
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

    let ready_deadline = Instant::now() + Duration::from_secs(3);
    let mut ready = false;
    while Instant::now() < ready_deadline {
        app.process_pty_events();
        if app
            .active_terminal()
            .unwrap()
            .grid()
            .plain_lines()
            .join("\n")
            .contains("READY")
        {
            ready = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    assert!(ready, "drop receiver did not become ready");

    assert!(app.handle_dropped_path(&path));
    assert!(app.write_terminal_protocol_input(b"X"));
    let expected_hex = expected
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<Vec<_>>()
        .concat();
    let deadline = Instant::now() + Duration::from_secs(3);
    let mut output = String::new();
    while Instant::now() < deadline {
        app.process_pty_events();
        let Some(terminal) = app.active_terminal() else {
            break;
        };
        output = terminal.grid().plain_lines().join("\n");
        let compact = output
            .chars()
            .filter(|character| !character.is_whitespace())
            .collect::<String>();
        if compact.contains(&format!("drop={expected_hex}next=58")) {
            return;
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    panic!("exact drop payload did not arrive without Enter: {output:?}");
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
    let first_request = app.info_overlay.request_id();
    assert!(app.info_overlay.is_awaiting(first_request));
    assert!(app.pending_file_upload().is_some_and(|pending| {
        pending.request_id == first_request
            && pending.domain_id == "domain-upload"
            && pending.pane_id == 42
            && pending.name == "upload λ.bin"
            && pending.source_kind == FileUploadSourceKind::File { bytes: 32 }
    }));
    app.apply_info_overlay_action(crate::ui::info_overlay::InfoOverlayAction::Choose {
        request_id: first_request,
        choice_id: "file-upload-cancel".into(),
    });
    assert!(app.pending_file_upload().is_none());
    assert!(app.active_file_upload.is_none());
    std::thread::sleep(Duration::from_millis(250));
    app.info_overlay.update_animation();

    assert!(app.handle_dropped_path(&source));
    let second_request = app.info_overlay.request_id();
    app.apply_info_overlay_action(crate::ui::info_overlay::InfoOverlayAction::Choose {
        request_id: first_request,
        choice_id: "file-upload-confirm".into(),
    });
    assert!(
        app.pending_file_upload().is_some(),
        "stale confirmation acted"
    );
    app.apply_info_overlay_action(crate::ui::info_overlay::InfoOverlayAction::Choose {
        request_id: second_request,
        choice_id: "file-upload-confirm".into(),
    });
    assert!(app.pending_file_upload().is_none());
    assert!(app.active_file_upload.is_none());
    assert!(latest_toast(&app).is_some_and(|message| message.contains("native window")));

    std::thread::sleep(Duration::from_millis(250));
    app.info_overlay.update_animation();
    let directory = temp.path().join("project λ");
    std::fs::create_dir(&directory).unwrap();
    assert!(app.handle_dropped_path(&directory));
    let directory_request = app.info_overlay.request_id();
    assert!(app.pending_file_upload().is_some_and(|pending| {
        pending.request_id == directory_request
            && pending.name == "project λ"
            && pending.source_kind == FileUploadSourceKind::Directory
    }));
    app.apply_info_overlay_action(crate::ui::info_overlay::InfoOverlayAction::Choose {
        request_id: directory_request,
        choice_id: "file-upload-cancel".into(),
    });
    assert!(app.pending_file_upload().is_none());
}

#[test]
fn kitty_download_consent_is_request_scoped_and_terminal_type_gated() {
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
    let request = termeric_core::KittyFileTransferCommand {
        action: termeric_core::KittyFileTransferAction::Send,
        compression: termeric_core::KittyFileTransferCompression::None,
        file_type: termeric_core::KittyFileTransferType::Regular,
        transmission_type: termeric_core::KittyFileTransmissionType::Simple,
        id: "download-1".into(),
        file_id: String::new(),
        quiet: 0,
        mtime_ns: None,
        permissions: None,
        size: None,
        name: None,
        status: None,
        parent: None,
        data: Box::default(),
        bypass_present: false,
        reply_uses_8bit_controls: true,
    };

    app.handle_terminal_event(RoutedTerminalEvent {
        origin,
        event: TerminalEvent::KittyFileTransfer(request.clone()),
    });
    let confirmation = app.info_overlay.request_id();
    assert!(app.info_overlay.is_awaiting(confirmation));
    // Pointer hit-testing, hover, and focus follow each choice's source line,
    // so every choice must sit on the line that shows its shortcut.
    for choice in app.info_overlay.choices() {
        let line = app.info_overlay.lines()[choice.source_line].trim_start();
        let key = choice.shortcut.unwrap().to_ascii_uppercase();
        assert!(line.starts_with(key), "{} sits on {line:?}", choice.id);
    }
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
    app.apply_info_overlay_action(crate::ui::info_overlay::InfoOverlayAction::Choose {
        request_id: confirmation.wrapping_add(1),
        choice_id: "kitty-download-allow".into(),
    });
    assert!(app.kitty_file_transfers.pending.is_some());
    app.apply_info_overlay_action(crate::ui::info_overlay::InfoOverlayAction::Choose {
        request_id: confirmation,
        choice_id: "kitty-download-deny".into(),
    });
    assert!(app.kitty_file_transfers.pending.is_none());
    assert!(app.kitty_file_transfers.active.is_empty());

    std::thread::sleep(Duration::from_millis(250));
    app.info_overlay.update_animation();
    let mut stale_request = request.clone();
    stale_request.id = "download-stale".into();
    app.handle_terminal_event(RoutedTerminalEvent {
        origin,
        event: TerminalEvent::KittyFileTransfer(stale_request),
    });
    assert!(app.kitty_file_transfers.pending.is_some());
    let replacement = app.info_overlay.show("Replacement", vec!["content".into()]);
    assert!(app.prune_stale_kitty_file_transfers());
    assert!(app.kitty_file_transfers.pending.is_none());
    app.info_overlay.dismiss(replacement);
    std::thread::sleep(Duration::from_millis(250));
    app.info_overlay.update_animation();

    app.tabs
        .as_mut()
        .unwrap()
        .active_tab_mut()
        .unwrap()
        .tab_type = crate::tabs::TabType::Plugin {
        plugin_id: "untrusted".into(),
    };
    let mut plugin_request = request;
    plugin_request.id = "plugin-download".into();
    app.handle_terminal_event(RoutedTerminalEvent {
        origin,
        event: TerminalEvent::KittyFileTransfer(plugin_request),
    });
    assert!(!app.info_overlay.is_active());
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
            action,
            compression: termeric_core::KittyFileTransferCompression::None,
            file_type: termeric_core::KittyFileTransferType::Regular,
            transmission_type: termeric_core::KittyFileTransmissionType::Simple,
            id: "upload-1".into(),
            file_id: file_id.into(),
            quiet: 0,
            mtime_ns: None,
            permissions: None,
            size,
            name: name.map(str::to_owned),
            status: None,
            parent: None,
            data: Box::default(),
            bypass_present: false,
            reply_uses_8bit_controls: true,
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
    assert!(!app.info_overlay.is_active());

    app.handle_terminal_event(RoutedTerminalEvent {
        origin,
        event: TerminalEvent::KittyFileTransfer(command(
            termeric_core::KittyFileTransferAction::File,
            "f1",
            Some("/tmp/approved-one.txt"),
            None,
        )),
    });
    assert!(!app.info_overlay.is_active());
    app.handle_terminal_event(RoutedTerminalEvent {
        origin,
        event: TerminalEvent::KittyFileTransfer(command(
            termeric_core::KittyFileTransferAction::File,
            "f2",
            Some("~/approved-two.txt"),
            None,
        )),
    });

    let request_id = app.info_overlay.request_id();
    assert!(app.info_overlay.is_awaiting(request_id));
    assert_eq!(
        app.info_overlay.title(),
        "Allow terminal to read host files?"
    );
    let notice = app.info_overlay.lines().join("\n");
    assert!(notice.contains("/tmp/approved-one.txt"));
    assert!(notice.contains("approved-two.txt"));
    app.apply_info_overlay_action(crate::ui::info_overlay::InfoOverlayAction::Choose {
        request_id,
        choice_id: "kitty-upload-deny".into(),
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
    let request_id = app.info_overlay.request_id();
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
        "a pending transfer must never reply to the replacement single owner"
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
