use super::*;
use crate::plugin::hooks::{HookResponse, Plugin, PluginCommand};

#[derive(Debug)]
struct CountingTabClosedPlugin(std::sync::Arc<std::sync::atomic::AtomicUsize>);

impl Plugin for CountingTabClosedPlugin {
    fn id(&self) -> &str {
        "tab-closed-observer"
    }

    fn name(&self) -> &str {
        "Tab Closed Observer"
    }

    fn on_hook(
        &mut self,
        hook: &crate::plugin::Hook,
        _ctx: &crate::plugin::PluginContext,
    ) -> HookResponse {
        if matches!(hook, crate::plugin::Hook::TabClosed { .. }) {
            self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        }
        HookResponse::Ignored
    }
}

#[derive(Debug)]
struct CountingTabSwitchedPlugin(std::sync::Arc<std::sync::atomic::AtomicUsize>);

impl Plugin for CountingTabSwitchedPlugin {
    fn id(&self) -> &str {
        "tab-switched-observer"
    }

    fn name(&self) -> &str {
        "Tab Switched Observer"
    }

    fn on_hook(
        &mut self,
        hook: &crate::plugin::Hook,
        _ctx: &crate::plugin::PluginContext,
    ) -> HookResponse {
        if matches!(hook, crate::plugin::Hook::TabSwitched { .. }) {
            self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        }
        HookResponse::Ignored
    }
}

#[test]
fn native_window_title_tracks_the_active_tab_without_losing_product_identity() {
    assert_eq!(native_window_title(false, None), "Termeric");
    assert_eq!(native_window_title(false, Some("  ")), "Termeric");
    assert_eq!(
        native_window_title(false, Some("build; tests 🌱")),
        "build; tests 🌱 — Termeric"
    );
    assert_eq!(
        native_window_title(true, Some("server")),
        "server — Termeric Quick Access"
    );
}

#[derive(Debug)]
struct ThirdPartyReservedSuffixFixture;

impl Plugin for ThirdPartyReservedSuffixFixture {
    fn id(&self) -> &str {
        "third-party"
    }

    fn name(&self) -> &str {
        "Third-party reserved suffix fixture"
    }

    fn on_hook(
        &mut self,
        hook: &crate::plugin::Hook,
        _context: &crate::plugin::PluginContext,
    ) -> HookResponse {
        if matches!(
            hook,
            crate::plugin::Hook::CommandInvoked { command_id }
                if command_id == "third-party.review-diff"
        ) {
            HookResponse::Action(crate::plugin::PluginAction::ShowMessage {
                message: "third-party command routed".into(),
            })
        } else {
            HookResponse::Ignored
        }
    }

    fn commands(&self) -> Vec<PluginCommand> {
        vec![PluginCommand {
            id: "third-party.review-diff".into(),
            name: "Third-party review".into(),
            description: String::new(),
            category: None,
        }]
    }
}

#[test]
fn third_party_command_suffixes_do_not_invoke_builtin_ai_actions() {
    let mut app = App::new_headless(Config::default()).unwrap();
    let context = app.build_plugin_context();
    app.plugin_manager
        .register(Box::new(ThirdPartyReservedSuffixFixture), &context)
        .unwrap();

    app.execute_plugin_command("third-party.review-diff");

    assert_eq!(
        app.toasts.iter().last().map(|toast| toast.message.as_str()),
        Some("third-party command routed")
    );
    assert!(!app.diff_review.is_active());
}

#[cfg(unix)]
#[test]
fn local_file_drop_is_quoted_and_refuses_unsafe_terminal_routes() {
    if !std::path::Path::new("/bin/sh").exists() {
        return;
    }
    let mut config = Config::default();
    config.shell.program = Some("/bin/sh".into());
    config.shell.args = vec!["-c".into(), "sleep 2".into()];
    let mut app = App::new_headless(config).unwrap();
    app.allow_auto_restore = false;
    app.init_tabs().unwrap();

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

#[cfg(unix)]
#[test]
fn local_file_drop_writes_one_bracketed_payload_without_enter() {
    if !std::path::Path::new("/bin/sh").exists() {
        return;
    }
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
    let mut app = App::new_headless(config).unwrap();
    app.allow_auto_restore = false;
    app.init_tabs().unwrap();
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
            .export_scrollback_text()
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
        output = terminal.grid().export_scrollback_text().join("\n");
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

#[cfg(unix)]
#[test]
fn live_domain_drop_requires_request_scoped_confirmation_without_reading_contents() {
    if !std::path::Path::new("/bin/sh").exists() {
        return;
    }
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("upload λ.bin");
    std::fs::write(&source, b"never sent by this headless test").unwrap();
    let mut config = Config::default();
    config.shell.program = Some("/bin/sh".into());
    config.shell.args = vec!["-c".into(), "sleep 2".into()];
    let mut app = App::new_headless(config).unwrap();
    app.allow_auto_restore = false;
    app.init_tabs().unwrap();
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
    assert!(app.pending_file_upload.as_ref().is_some_and(|pending| {
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
    assert!(app.pending_file_upload.is_none());
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
        app.pending_file_upload.is_some(),
        "stale confirmation acted"
    );
    app.apply_info_overlay_action(crate::ui::info_overlay::InfoOverlayAction::Choose {
        request_id: second_request,
        choice_id: "file-upload-confirm".into(),
    });
    assert!(app.pending_file_upload.is_none());
    assert!(app.active_file_upload.is_none());
    assert!(latest_toast(&app).is_some_and(|message| message.contains("native window")));

    std::thread::sleep(Duration::from_millis(250));
    app.info_overlay.update_animation();
    let directory = temp.path().join("project λ");
    std::fs::create_dir(&directory).unwrap();
    assert!(app.handle_dropped_path(&directory));
    let directory_request = app.info_overlay.request_id();
    assert!(app.pending_file_upload.as_ref().is_some_and(|pending| {
        pending.request_id == directory_request
            && pending.name == "project λ"
            && pending.source_kind == FileUploadSourceKind::Directory
    }));
    app.apply_info_overlay_action(crate::ui::info_overlay::InfoOverlayAction::Choose {
        request_id: directory_request,
        choice_id: "file-upload-cancel".into(),
    });
    assert!(app.pending_file_upload.is_none());
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
fn native_presentation_latency_waits_for_terminal_output_and_records_once() {
    let temporary = tempfile::tempdir().unwrap();
    let output = temporary.path().join("latency.samples");
    let mut probe = NativePresentationLatencyProbe {
        output: Some(output.clone()),
        pending: None,
    };

    probe.begin();
    probe.finish_after_terminal_present("keyboard-input");
    assert!(!output.exists());
    probe.finish_after_terminal_present("keyboard-input,terminal-output");
    let samples = std::fs::read_to_string(&output).unwrap();
    assert_eq!(samples.lines().count(), 1);
    assert!(samples.trim().parse::<f64>().unwrap() >= 0.0);
    probe.finish_after_terminal_present("terminal-output");
    assert_eq!(std::fs::read_to_string(output).unwrap(), samples);
}

#[test]
fn native_frame_stats_publish_the_latest_complete_counter_atomically() {
    let temporary = tempfile::tempdir().unwrap();
    let output = temporary.path().join("frame-count");
    let mut probe = NativeFrameStatsProbe {
        output: Some(output.clone()),
    };

    probe.record(1);
    assert_eq!(std::fs::read_to_string(&output).unwrap(), "1\n");
    probe.record(17);
    assert_eq!(std::fs::read_to_string(&output).unwrap(), "17\n");
    assert!(!output.with_extension("tmp").exists());
}

#[cfg(target_os = "linux")]
#[test]
fn desktop_helper_spawn_failures_become_deduplicated_visible_toasts() {
    let mut app = App::new_headless(Config::default()).unwrap();
    app.bell.set_audio(true);
    app.bell
        .set_audio_command_for_test("/definitely/missing/termeric-audio-helper");
    app.notifications
        .set_command_for_test("/definitely/missing/termeric-notify-helper");
    let origin = TerminalEventOrigin::Tab { tab_id: 999 };

    app.handle_terminal_event(RoutedTerminalEvent {
        origin,
        event: TerminalEvent::Bell,
    });
    app.handle_terminal_event(RoutedTerminalEvent {
        origin,
        event: TerminalEvent::Notification(termeric_core::NotificationRequest {
            title: Some("Build".into()),
            body: "Finished".into(),
        }),
    });
    let messages = app
        .toasts
        .iter()
        .map(|toast| toast.message.as_str())
        .collect::<Vec<_>>();
    assert_eq!(messages.len(), 2);
    assert!(messages[0].contains("Audio bell unavailable"));
    assert!(messages[1].contains("Desktop notification unavailable"));

    // The manager suppresses repeats of the same integration failure instead
    // of letting a hostile terminal fill the bounded toast stack.
    app.handle_terminal_event(RoutedTerminalEvent {
        origin,
        event: TerminalEvent::Notification(termeric_core::NotificationRequest {
            title: Some("Build".into()),
            body: "Again".into(),
        }),
    });
    assert_eq!(app.toasts.iter().count(), 2);
}
use crate::config::ShellConfig;
use crate::panes::{PaneManager, SplitDirection};
use crate::plugin::PluginAction;
use crate::tabs::{RoutedTerminalEvent, Tab, TerminalEventOrigin};
use std::collections::HashMap;
use std::time::{Duration, Instant};
use termeric_core::event::ClipboardRequest;

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

#[cfg(unix)]
#[test]
fn control_selection_validates_complete_stable_paths_before_changing_focus() {
    if !std::path::Path::new("/bin/sh").exists() {
        return;
    }
    let shell = ShellConfig {
        program: Some("/bin/sh".into()),
        args: vec!["-c".into(), "sleep 2".into()],
        env: Default::default(),
    };
    let mut tabs = TabManager::new(80, 24, shell.clone(), 1_000, 8 * 1024 * 1024);
    let first = tabs
        .new_tab_with_command(
            "/bin/sh",
            &["-c".into(), "sleep 2".into()],
            None,
            Some("first"),
            None,
            HashMap::new(),
        )
        .unwrap();
    let second = tabs
        .new_tab_with_command(
            "/bin/sh",
            &["-c".into(), "sleep 2".into()],
            None,
            Some("second"),
            None,
            HashMap::new(),
        )
        .unwrap();
    let first_tab = tabs.tab_mut_by_id(first).unwrap();
    let owner = first_tab.take_terminal_owner().unwrap();
    let mut panes = PaneManager::from_existing(
        owner,
        shell,
        1_000,
        8 * 1024 * 1024,
        800.0,
        480.0,
        10.0,
        20.0,
    );
    panes.split(SplitDirection::Vertical).unwrap();
    assert_eq!(panes.focused_id(), PaneId(1));
    first_tab.pane_manager = Some(panes);
    assert_eq!(tabs.active_tab_id(), Some(second));

    let mut app = App::new_headless(Config::default()).unwrap();
    app.tabs = Some(tabs);
    let source_terminal = &mut app
        .tabs
        .as_mut()
        .unwrap()
        .tab_mut_by_id(first)
        .unwrap()
        .pane_manager
        .as_mut()
        .unwrap()
        .get_pane_mut(PaneId(0))
        .unwrap()
        .terminal;
    source_terminal.process(b"presented-secret");
    source_terminal.process(b"\x1b[?2026h\rhidden-update");
    let (screen, history, text, truncated) =
        app.read_control_text(first as u64, 0, 20, 4096).unwrap();
    assert_eq!(screen, crate::control::protocol::TextScreen::Primary);
    assert_eq!(history, 0);
    assert!(text.contains("presented-secret"));
    assert!(!text.contains("hidden-update"));
    assert!(!truncated);
    assert!(app.read_control_text(first as u64, 99, 0, 64).is_err());

    let event = app
        .structured_terminal_query_event(
            "terminal-reader",
            "visible-text",
            first as u64,
            0,
            20,
            4096,
        )
        .unwrap();
    let event: serde_json::Value = serde_json::from_str(&event).unwrap();
    assert_eq!(event["schema_version"], 1);
    assert_eq!(event["plugin_id"], "terminal-reader");
    assert_eq!(event["event"]["type"], "terminal-query-result");
    assert_eq!(event["event"]["query_id"], "visible-text");
    assert_eq!(event["event"]["text_schema_version"], 1);
    assert_eq!(event["event"]["ok"], true);
    assert_eq!(event["event"]["tab_id"], first as u64);
    assert_eq!(event["event"]["pane_id"], 0);
    assert_eq!(event["event"]["screen"], "primary");
    assert!(
        event["event"]["text"]
            .as_str()
            .unwrap()
            .contains("presented-secret")
    );
    assert!(
        !event["event"]["text"]
            .as_str()
            .unwrap()
            .contains("hidden-update")
    );

    let missing = app
        .structured_terminal_query_event("terminal-reader", "missing-pane", first as u64, 99, 0, 64)
        .unwrap();
    let missing: serde_json::Value = serde_json::from_str(&missing).unwrap();
    assert_eq!(missing["event"]["ok"], false);
    assert_eq!(missing["event"]["pane_id"], 99);
    assert!(missing["event"]["text"].is_null());
    assert!(
        missing["event"]["error"]
            .as_str()
            .unwrap()
            .contains("does not exist")
    );

    let source_terminal = &mut app
        .tabs
        .as_mut()
        .unwrap()
        .tab_mut_by_id(first)
        .unwrap()
        .pane_manager
        .as_mut()
        .unwrap()
        .get_pane_mut(PaneId(0))
        .unwrap()
        .terminal;
    source_terminal.process(b"\x1b[?2026l\x1b[2J\x1b[H");
    source_terminal.process("\\".repeat(9_000).as_bytes());
    let escaped = app
        .structured_terminal_query_event(
            "terminal-reader",
            "escaped-text",
            first as u64,
            0,
            256,
            crate::plugin::structured::MAX_TERMINAL_QUERY_BYTES,
        )
        .unwrap();
    assert!(escaped.len() <= crate::plugin::structured::MAX_PERSISTENT_SOURCE_EVENT_BYTES);
    let escaped: serde_json::Value = serde_json::from_str(&escaped).unwrap();
    assert_eq!(escaped["event"]["ok"], true);
    assert_eq!(escaped["event"]["truncated"], true);
    assert!(
        escaped["event"]["text_bytes"].as_u64().unwrap()
            < u64::from(crate::plugin::structured::MAX_TERMINAL_QUERY_BYTES)
    );

    let disabled_input = app
        .structured_workspace_action_event(
            "terminal-writer",
            crate::plugin::structured::StructuredResponseAction::SendText {
                action_id: "policy-disabled".into(),
                tab_id: first as u64,
                pane_id: 0,
                text: "x".into(),
                paste: false,
                enter: false,
            },
        )
        .unwrap();
    let disabled_input: serde_json::Value = serde_json::from_str(&disabled_input).unwrap();
    assert_eq!(disabled_input["event"]["action"], "send-text");
    assert_eq!(disabled_input["event"]["ok"], false);
    assert!(
        disabled_input["event"]["error"]
            .as_str()
            .is_some_and(|error| error.contains("disabled"))
    );

    app.config.security.allow_gui_control_input = true;
    let sent_input = app
        .structured_workspace_action_event(
            "terminal-writer",
            crate::plugin::structured::StructuredResponseAction::SendText {
                action_id: "safe-input".into(),
                tab_id: first as u64,
                pane_id: 0,
                text: "x".into(),
                paste: false,
                enter: false,
            },
        )
        .unwrap();
    let sent_input: serde_json::Value = serde_json::from_str(&sent_input).unwrap();
    assert_eq!(sent_input["event"]["ok"], true);
    assert_eq!(sent_input["event"]["disposition"], "sent");
    assert_eq!(sent_input["event"]["input_bytes"], 1);
    assert_eq!(sent_input["event"]["wire_bytes"], 1);
    assert_eq!(
        sent_input["event"]["confirmation_id"],
        serde_json::Value::Null
    );
    assert_eq!(app.tabs.as_ref().unwrap().active_tab_id(), Some(second));

    app.tabs
        .as_mut()
        .unwrap()
        .tab_mut_by_id(first)
        .unwrap()
        .pane_manager
        .as_mut()
        .unwrap()
        .get_pane_mut(PaneId(0))
        .unwrap()
        .closed = true;
    let closed_input = app
        .structured_workspace_action_event(
            "terminal-writer",
            crate::plugin::structured::StructuredResponseAction::SendText {
                action_id: "closed-input".into(),
                tab_id: first as u64,
                pane_id: 0,
                text: "x".into(),
                paste: false,
                enter: false,
            },
        )
        .unwrap();
    let closed_input: serde_json::Value = serde_json::from_str(&closed_input).unwrap();
    assert_eq!(closed_input["event"]["ok"], false);
    assert!(
        closed_input["event"]["error"]
            .as_str()
            .is_some_and(|error| error.contains("closed"))
    );
    app.tabs
        .as_mut()
        .unwrap()
        .tab_mut_by_id(first)
        .unwrap()
        .pane_manager
        .as_mut()
        .unwrap()
        .get_pane_mut(PaneId(0))
        .unwrap()
        .closed = false;

    let guarded_input = app
        .structured_workspace_action_event(
            "terminal-writer",
            crate::plugin::structured::StructuredResponseAction::SendText {
                action_id: "guarded-input".into(),
                tab_id: first as u64,
                pane_id: 0,
                text: "rm -rf /".into(),
                paste: false,
                enter: true,
            },
        )
        .unwrap();
    let guarded_input: serde_json::Value = serde_json::from_str(&guarded_input).unwrap();
    assert_eq!(guarded_input["event"]["ok"], true);
    assert_eq!(
        guarded_input["event"]["disposition"],
        "pending_confirmation"
    );
    let confirmation_id = guarded_input["event"]["confirmation_id"].as_u64().unwrap();
    assert!(app.guardrail.is_awaiting(confirmation_id));
    assert_eq!(app.tabs.as_ref().unwrap().active_tab_id(), Some(second));
    app.apply_guardrail_action(crate::terminal::guardrails::GuardrailAction::Cancel {
        request_id: confirmation_id,
    });
    assert!(!app.guardrail.is_awaiting_current());

    assert!(matches!(
        app.send_control_text(first as u64, 0, "x", false, false),
        Ok((1, 1, crate::control::protocol::InputDisposition::Sent, None))
    ));
    assert_eq!(app.tabs.as_ref().unwrap().active_tab_id(), Some(second));
    assert!(app.focus_control_pane(first as u64, 99).is_err());
    assert_eq!(app.tabs.as_ref().unwrap().active_tab_id(), Some(second));

    assert_eq!(app.focus_control_pane(first as u64, 0), Ok((true, true)));
    let tabs = app.tabs.as_ref().unwrap();
    assert_eq!(tabs.active_tab_id(), Some(first));
    assert_eq!(
        tabs.active_tab()
            .unwrap()
            .pane_manager
            .as_ref()
            .unwrap()
            .focused_id(),
        PaneId(0)
    );
    assert_eq!(app.focus_control_pane(first as u64, 0), Ok((false, false)));
    assert_eq!(app.select_control_tab(second as u64), Ok(true));
    assert_eq!(app.select_control_tab(second as u64), Ok(false));
    assert!(app.select_control_tab(u64::MAX).is_err());
    assert_eq!(app.tabs.as_ref().unwrap().active_tab_id(), Some(second));

    let moved = app
        .structured_workspace_action_event(
            "workspace-organizer",
            crate::plugin::structured::StructuredResponseAction::MoveTab {
                action_id: "move-first".into(),
                tab_id: first as u64,
                destination_index: 1,
            },
        )
        .unwrap();
    let moved: serde_json::Value = serde_json::from_str(&moved).unwrap();
    assert_eq!(moved["event"]["type"], "workspace-action-result");
    assert_eq!(moved["event"]["action"], "move-tab");
    assert_eq!(moved["event"]["ok"], true);
    assert_eq!(moved["event"]["changed"], true);
    assert_eq!(moved["event"]["destination_index"], 1);
    assert_eq!(app.tabs.as_ref().unwrap().active_tab_id(), Some(second));
    assert_eq!(
        app.tabs
            .as_ref()
            .unwrap()
            .all_tabs()
            .iter()
            .map(|tab| tab.id)
            .collect::<Vec<_>>(),
        vec![second, first]
    );

    let failed_move = app
        .structured_workspace_action_event(
            "workspace-organizer",
            crate::plugin::structured::StructuredResponseAction::MoveTab {
                action_id: "invalid-index".into(),
                tab_id: first as u64,
                destination_index: 2,
            },
        )
        .unwrap();
    let failed_move: serde_json::Value = serde_json::from_str(&failed_move).unwrap();
    assert_eq!(failed_move["event"]["ok"], false);
    assert_eq!(failed_move["event"]["changed"], serde_json::Value::Null);
    assert!(
        failed_move["event"]["error"]
            .as_str()
            .is_some_and(|error| error.contains("outside"))
    );
    assert_eq!(
        app.tabs
            .as_ref()
            .unwrap()
            .all_tabs()
            .iter()
            .map(|tab| tab.id)
            .collect::<Vec<_>>(),
        vec![second, first]
    );

    let divider_id = app
        .tabs
        .as_ref()
        .unwrap()
        .all_tabs()
        .iter()
        .find(|tab| tab.id == first)
        .and_then(|tab| tab.pane_manager.as_ref())
        .and_then(|manager| manager.get_dividers().first().copied())
        .and_then(|divider| match divider.target {
            crate::panes::DividerTarget::Local(id) => Some(id),
            crate::panes::DividerTarget::Domain(_) => None,
        })
        .unwrap();
    let resized = app
        .structured_workspace_action_event(
            "workspace-organizer",
            crate::plugin::structured::StructuredResponseAction::ResizeDivider {
                action_id: "resize-first".into(),
                tab_id: first as u64,
                divider_id,
                ratio_millis: 650,
            },
        )
        .unwrap();
    let resized: serde_json::Value = serde_json::from_str(&resized).unwrap();
    assert_eq!(resized["event"]["action"], "resize-divider");
    assert_eq!(resized["event"]["ok"], true);
    assert_eq!(resized["event"]["previous_ratio_millis"], 500);
    assert_eq!(resized["event"]["ratio_millis"], 650);
    assert_eq!(resized["event"]["changed"], true);
    assert_eq!(
        app.resize_control_divider(first as u64, divider_id, 650),
        Ok((650, false))
    );
    let failed_resize = app
        .structured_workspace_action_event(
            "workspace-organizer",
            crate::plugin::structured::StructuredResponseAction::ResizeDivider {
                action_id: "missing-divider".into(),
                tab_id: first as u64,
                divider_id: u64::MAX,
                ratio_millis: 600,
            },
        )
        .unwrap();
    let failed_resize: serde_json::Value = serde_json::from_str(&failed_resize).unwrap();
    assert_eq!(failed_resize["event"]["ok"], false);
    assert_eq!(
        failed_resize["event"]["previous_ratio_millis"],
        serde_json::Value::Null
    );
    assert_eq!(failed_resize["event"]["changed"], serde_json::Value::Null);
    assert_eq!(app.tabs.as_ref().unwrap().active_tab_id(), Some(second));
    let inventory = app.control_inventory(7, &mut ControlInventoryBudget::default());
    let divider = inventory
        .tabs
        .iter()
        .find(|tab| tab.id == first as u64)
        .and_then(|tab| tab.dividers.first())
        .unwrap();
    assert_eq!(divider.id, divider_id);
    assert_eq!(divider.ratio_millis, 650);
    assert_eq!(divider.kind, crate::control::protocol::DividerKind::Local);
    app.tabs
        .as_mut()
        .unwrap()
        .tab_mut_by_id(first)
        .unwrap()
        .live_domain = Some(crate::tabs::LiveDomainTab {
        domain_id: "fake-domain".into(),
        tab_id: 8,
        pane_ids: HashMap::from([(PaneId(0), 10), (PaneId(1), 11)]),
    });
    assert!(
        app.resize_control_divider(first as u64, divider_id, 600)
            .unwrap_err()
            .contains("live-domain")
    );
    app.tabs
        .as_mut()
        .unwrap()
        .tab_mut_by_id(first)
        .unwrap()
        .live_domain = None;

    assert_eq!(app.close_control_pane(first as u64, 0), Ok((1, 1)));
    let tabs = app.tabs.as_ref().unwrap();
    assert_eq!(tabs.active_tab_id(), Some(second));
    assert!(
        tabs.iter()
            .find(|(_, tab)| tab.id == first)
            .unwrap()
            .1
            .pane_manager
            .is_none()
    );
    assert_eq!(
        tabs.iter()
            .find(|(_, tab)| tab.id == first)
            .unwrap()
            .1
            .single_pane_id(),
        PaneId(1)
    );
    assert!(app.read_control_text(first as u64, 1, 0, 64).is_ok());
    assert!(app.read_control_text(first as u64, 0, 0, 64).is_err());
    assert!(app.close_control_pane(first as u64, 1).is_err());
    app.close_control_tab(first as u64).unwrap();
    assert!(
        app.tabs
            .as_ref()
            .unwrap()
            .iter()
            .find(|(_, tab)| tab.id == first)
            .unwrap()
            .1
            .closing_at
            .is_some()
    );
    assert!(app.close_control_tab(first as u64).is_err());
}

#[cfg(unix)]
#[test]
fn plugin_pane_creation_is_exact_correlated_and_rejects_unsupported_tabs() {
    if !std::path::Path::new("/bin/sh").exists() {
        return;
    }
    let shell = ShellConfig {
        program: Some("/bin/sh".into()),
        args: vec!["-c".into(), "sleep 2".into()],
        env: Default::default(),
    };
    let mut tabs = TabManager::new(80, 24, shell.clone(), 1_000, 8 * 1024 * 1024);
    let tab_id = tabs.new_tab().unwrap();
    let tab = tabs.tab_mut_by_id(tab_id).unwrap();
    let owner = tab.take_terminal_owner().unwrap();
    let mut panes = PaneManager::from_existing(
        owner,
        shell,
        1_000,
        8 * 1024 * 1024,
        800.0,
        480.0,
        10.0,
        20.0,
    );
    panes.split(SplitDirection::Vertical).unwrap();
    tab.pane_manager = Some(panes);
    let mut app = App::new_headless(Config::default()).unwrap();
    app.tabs = Some(tabs);

    let created = app
        .structured_workspace_action_event(
            "pane-builder",
            crate::plugin::structured::StructuredResponseAction::CreatePane {
                action_id: "split-exact-pane".into(),
                tab_id: tab_id as u64,
                pane_id: 0,
                direction: crate::control::protocol::SplitDirection::Horizontal,
            },
        )
        .unwrap();
    let created: serde_json::Value = serde_json::from_str(&created).unwrap();
    assert_eq!(created["event"]["type"], "workspace-action-result");
    assert_eq!(created["event"]["action"], "create-pane");
    assert_eq!(created["event"]["action_id"], "split-exact-pane");
    assert_eq!(created["event"]["ok"], true);
    assert_eq!(created["event"]["tab_id"], tab_id as u64);
    assert_eq!(created["event"]["pane_id"], 0);
    assert_eq!(created["event"]["direction"], "horizontal");
    let created_pane_id = created["event"]["created_pane_id"].as_u64().unwrap() as u32;
    assert!(
        app.tabs
            .as_ref()
            .unwrap()
            .tab_by_id(tab_id)
            .unwrap()
            .pane_manager
            .as_ref()
            .unwrap()
            .get_pane(PaneId(created_pane_id))
            .is_some()
    );

    let stale = app
        .structured_workspace_action_event(
            "pane-builder",
            crate::plugin::structured::StructuredResponseAction::CreatePane {
                action_id: "stale-pane".into(),
                tab_id: tab_id as u64,
                pane_id: u32::MAX,
                direction: crate::control::protocol::SplitDirection::Vertical,
            },
        )
        .unwrap();
    let stale: serde_json::Value = serde_json::from_str(&stale).unwrap();
    assert_eq!(stale["event"]["ok"], false);
    assert!(stale["event"]["created_pane_id"].is_null());
    assert!(
        stale["event"]["error"]
            .as_str()
            .is_some_and(|error| error.contains("does not exist"))
    );

    let tab = app.tabs.as_mut().unwrap().tab_mut_by_id(tab_id).unwrap();
    tab.live_domain = Some(crate::tabs::LiveDomainTab {
        domain_id: "fake-domain".into(),
        tab_id: 8,
        pane_ids: HashMap::new(),
    });
    let live = app
        .structured_workspace_action_event(
            "pane-builder",
            crate::plugin::structured::StructuredResponseAction::CreatePane {
                action_id: "reject-live".into(),
                tab_id: tab_id as u64,
                pane_id: 0,
                direction: crate::control::protocol::SplitDirection::Vertical,
            },
        )
        .unwrap();
    let live: serde_json::Value = serde_json::from_str(&live).unwrap();
    assert_eq!(live["event"]["ok"], false);
    assert!(
        live["event"]["error"]
            .as_str()
            .is_some_and(|error| error.contains("live-domain"))
    );

    let tab = app.tabs.as_mut().unwrap().tab_mut_by_id(tab_id).unwrap();
    tab.live_domain = None;
    tab.tab_type = crate::tabs::TabType::Serial {
        name: "fixture".into(),
    };
    let serial = app
        .structured_workspace_action_event(
            "pane-builder",
            crate::plugin::structured::StructuredResponseAction::CreatePane {
                action_id: "reject-serial".into(),
                tab_id: tab_id as u64,
                pane_id: 0,
                direction: crate::control::protocol::SplitDirection::Vertical,
            },
        )
        .unwrap();
    let serial: serde_json::Value = serde_json::from_str(&serial).unwrap();
    assert_eq!(serial["event"]["ok"], false);
    assert!(
        serial["event"]["error"]
            .as_str()
            .is_some_and(|error| error.contains("Serial"))
    );

    let tab = app.tabs.as_mut().unwrap().tab_mut_by_id(tab_id).unwrap();
    tab.tab_type = crate::tabs::TabType::Shell;
    tab.closing_at = Some(Instant::now());
    let closing = app
        .structured_workspace_action_event(
            "pane-builder",
            crate::plugin::structured::StructuredResponseAction::CreatePane {
                action_id: "reject-closing".into(),
                tab_id: tab_id as u64,
                pane_id: 0,
                direction: crate::control::protocol::SplitDirection::Vertical,
            },
        )
        .unwrap();
    let closing: serde_json::Value = serde_json::from_str(&closing).unwrap();
    assert_eq!(closing["event"]["ok"], false);
    assert!(
        closing["event"]["error"]
            .as_str()
            .is_some_and(|error| error.contains("closing"))
    );
}

#[cfg(unix)]
#[test]
fn plugin_pane_movement_preserves_pty_and_returns_exact_destination_state() {
    if !std::path::Path::new("/bin/sh").exists() {
        return;
    }
    let shell = ShellConfig {
        program: Some("/bin/sh".into()),
        args: vec!["-c".into(), "sleep 2".into()],
        env: Default::default(),
    };
    let mut tabs = TabManager::new(80, 24, shell.clone(), 1_000, 8 * 1024 * 1024);
    let source_tab_id = tabs.new_tab().unwrap();
    let source = tabs.tab_mut_by_id(source_tab_id).unwrap();
    let owner = source.take_terminal_owner().unwrap();
    let moved_child = owner.pty.child_id();
    let mut source_panes = PaneManager::from_existing(
        owner,
        shell,
        1_000,
        8 * 1024 * 1024,
        800.0,
        480.0,
        10.0,
        20.0,
    );
    let surviving_source_pane = source_panes.split(SplitDirection::Vertical).unwrap();
    source.pane_manager = Some(source_panes);
    let destination_tab_id = tabs.new_tab().unwrap();
    let destination_anchor = tabs.tab_by_id(destination_tab_id).unwrap().single_pane_id();
    tabs.switch_to(0);

    let mut app = App::new_headless(Config::default()).unwrap();
    app.tabs = Some(tabs);
    app.plugin_manager.unload_all();
    let observed = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let context = app.build_plugin_context();
    app.plugin_manager
        .register(
            Box::new(CountingTabSwitchedPlugin(std::sync::Arc::clone(&observed))),
            &context,
        )
        .unwrap();
    app.copy_mode_target = Some(CopyModeTarget {
        tab: source_tab_id,
        pane: Some(PaneId(0)),
    });
    app.broadcast = BroadcastState::Active {
        tab_id: source_tab_id,
        targets: vec![PaneId(0), surviving_source_pane],
    };

    let moved = app
        .structured_workspace_action_event(
            "pane-mover",
            crate::plugin::structured::StructuredResponseAction::MovePane {
                action_id: "move-build-output".into(),
                source_tab_id: source_tab_id as u64,
                pane_id: 0,
                destination_tab_id: destination_tab_id as u64,
                target_pane_id: destination_anchor.0,
                direction: crate::control::protocol::SplitDirection::Horizontal,
            },
        )
        .unwrap();
    let moved: serde_json::Value = serde_json::from_str(&moved).unwrap();
    assert_eq!(moved["event"]["type"], "workspace-action-result");
    assert_eq!(moved["event"]["action"], "move-pane");
    assert_eq!(moved["event"]["action_id"], "move-build-output");
    assert_eq!(moved["event"]["ok"], true);
    assert_eq!(moved["event"]["source_tab_id"], source_tab_id as u64);
    assert_eq!(moved["event"]["pane_id"], 0);
    assert_eq!(
        moved["event"]["destination_tab_id"],
        destination_tab_id as u64
    );
    assert_eq!(
        moved["event"]["target_pane_id"],
        u64::from(destination_anchor.0)
    );
    assert_eq!(moved["event"]["direction"], "horizontal");
    assert_eq!(moved["event"]["source_remaining_panes"], 1);
    assert_eq!(moved["event"]["active_tab_id"], destination_tab_id as u64);
    assert_eq!(moved["event"]["active_changed"], true);
    assert!(moved["event"]["error"].is_null());

    let destination_pane_id = PaneId(
        moved["event"]["destination_pane_id"]
            .as_u64()
            .unwrap()
            .try_into()
            .unwrap(),
    );
    assert_eq!(
        moved["event"]["focused_pane_id"],
        u64::from(destination_pane_id.0)
    );
    let tabs = app.tabs.as_ref().unwrap();
    assert_eq!(tabs.active_tab_id(), Some(destination_tab_id));
    let source = tabs.tab_by_id(source_tab_id).unwrap();
    assert_eq!(
        source.pane_manager.as_ref().unwrap().pane_ids(),
        vec![surviving_source_pane]
    );
    let destination = tabs.tab_by_id(destination_tab_id).unwrap();
    let destination_panes = destination.pane_manager.as_ref().unwrap();
    assert_eq!(destination_panes.focused_id(), destination_pane_id);
    assert_eq!(
        destination_panes
            .get_pane(destination_pane_id)
            .unwrap()
            .pty
            .child_id(),
        moved_child
    );
    assert!(app.copy_mode_target.is_none());
    assert!(matches!(app.broadcast, BroadcastState::Inactive));
    assert_eq!(observed.load(std::sync::atomic::Ordering::SeqCst), 0);
}

#[cfg(unix)]
#[test]
fn plugin_pane_layout_controls_background_tab_without_changing_terminal_owners() {
    if !std::path::Path::new("/bin/sh").exists() {
        return;
    }
    let shell = ShellConfig {
        program: Some("/bin/sh".into()),
        args: vec!["-c".into(), "sleep 2".into()],
        env: Default::default(),
    };
    let mut tabs = TabManager::new(80, 24, shell.clone(), 1_000, 8 * 1024 * 1024);
    let target_tab_id = tabs.new_tab().unwrap();
    let target = tabs.tab_mut_by_id(target_tab_id).unwrap();
    let owner = target.take_terminal_owner().unwrap();
    let mut panes = PaneManager::from_existing(
        owner,
        shell,
        1_000,
        8 * 1024 * 1024,
        800.0,
        480.0,
        10.0,
        20.0,
    );
    panes.split(SplitDirection::Vertical).unwrap();
    panes.split(SplitDirection::Horizontal).unwrap();
    let original = panes.pane_ids();
    let owners: HashMap<_, _> = original
        .iter()
        .map(|id| (*id, panes.get_pane(*id).unwrap().pty.child_id()))
        .collect();
    panes.focus(original[1]);
    assert!(panes.toggle_zoom());
    target.pane_manager = Some(panes);
    let active_tab_id = tabs.new_tab().unwrap();
    assert_eq!(tabs.active_tab_id(), Some(active_tab_id));

    let mut app = App::new_headless(Config::default()).unwrap();
    app.tabs = Some(tabs);
    let swapped = app
        .structured_workspace_action_event(
            "pane-arranger",
            crate::plugin::structured::StructuredResponseAction::SwapPanes {
                action_id: "swap-edges".into(),
                tab_id: target_tab_id as u64,
                first_pane_id: original[0].0,
                second_pane_id: original[2].0,
            },
        )
        .unwrap();
    let swapped: serde_json::Value = serde_json::from_str(&swapped).unwrap();
    let mut expected = original.clone();
    expected.swap(0, 2);
    assert_eq!(swapped["event"]["ok"], true);
    assert_eq!(swapped["event"]["action"], "swap-panes");
    assert_eq!(
        swapped["event"]["pane_order"],
        serde_json::json!(expected.iter().map(|id| id.0).collect::<Vec<_>>())
    );
    assert_eq!(swapped["event"]["focused_pane_id"], original[1].0);
    assert_eq!(swapped["event"]["zoomed_pane_id"], original[1].0);
    assert_eq!(
        app.tabs.as_ref().unwrap().active_tab_id(),
        Some(active_tab_id)
    );

    let target = app
        .tabs
        .as_mut()
        .unwrap()
        .tab_mut_by_id(target_tab_id)
        .unwrap()
        .pane_manager
        .as_mut()
        .unwrap();
    assert!(target.toggle_zoom());
    let crate::panes::DividerTarget::Local(divider_id) = target.get_dividers()[0].target else {
        panic!("ordinary layout unexpectedly used a domain divider")
    };
    target.resize_local_divider(divider_id, 700).unwrap();

    let balanced = app
        .structured_workspace_action_event(
            "pane-arranger",
            crate::plugin::structured::StructuredResponseAction::BalancePanes {
                action_id: "balance".into(),
                tab_id: target_tab_id as u64,
            },
        )
        .unwrap();
    let balanced: serde_json::Value = serde_json::from_str(&balanced).unwrap();
    assert_eq!(balanced["event"]["ok"], true);
    assert_eq!(balanced["event"]["changed"], true);
    assert_eq!(
        app.tabs.as_ref().unwrap().active_tab_id(),
        Some(active_tab_id)
    );

    let arranged = app
        .structured_workspace_action_event(
            "pane-arranger",
            crate::plugin::structured::StructuredResponseAction::ApplyPaneLayout {
                action_id: "grid".into(),
                tab_id: target_tab_id as u64,
                preset: crate::plugin::structured::PluginPaneLayoutPreset::Grid,
            },
        )
        .unwrap();
    let arranged: serde_json::Value = serde_json::from_str(&arranged).unwrap();
    assert_eq!(arranged["event"]["ok"], true);
    assert_eq!(arranged["event"]["preset"], "grid");
    assert!(arranged["event"]["zoomed_pane_id"].is_null());
    assert_eq!(
        app.tabs.as_ref().unwrap().active_tab_id(),
        Some(active_tab_id)
    );
    let target = app
        .tabs
        .as_ref()
        .unwrap()
        .tab_by_id(target_tab_id)
        .unwrap()
        .pane_manager
        .as_ref()
        .unwrap();
    assert_eq!(target.focused_id(), original[1]);
    assert_eq!(target.pane_ids(), expected);
    for (id, child_id) in owners {
        assert_eq!(target.get_pane(id).unwrap().pty.child_id(), child_id);
    }
}

#[cfg(unix)]
#[test]
fn plugin_pane_layout_refuses_unsupported_or_incomplete_tabs_atomically() {
    if !std::path::Path::new("/bin/sh").exists() {
        return;
    }
    let shell = ShellConfig {
        program: Some("/bin/sh".into()),
        args: vec!["-c".into(), "sleep 2".into()],
        env: Default::default(),
    };
    let mut tabs = TabManager::new(80, 24, shell.clone(), 1_000, 8 * 1024 * 1024);
    let tab_id = tabs.new_tab().unwrap();
    let tab = tabs.tab_mut_by_id(tab_id).unwrap();
    let owner = tab.take_terminal_owner().unwrap();
    let mut panes = PaneManager::from_existing(
        owner,
        shell,
        1_000,
        8 * 1024 * 1024,
        800.0,
        480.0,
        10.0,
        20.0,
    );
    let sibling = panes.split(SplitDirection::Vertical).unwrap();
    let original = panes.pane_ids();
    tab.pane_manager = Some(panes);
    let mut app = App::new_headless(Config::default()).unwrap();
    app.tabs = Some(tabs);

    let error = app
        .swap_plugin_panes(tab_id as u64, original[0].0, u32::MAX)
        .unwrap_err();
    assert!(error.contains("does not exist"));
    let panes = app
        .tabs
        .as_ref()
        .unwrap()
        .tab_by_id(tab_id)
        .unwrap()
        .pane_manager
        .as_ref()
        .unwrap();
    assert_eq!(panes.pane_ids(), original);

    app.tabs
        .as_mut()
        .unwrap()
        .tab_mut_by_id(tab_id)
        .unwrap()
        .pane_manager
        .as_mut()
        .unwrap()
        .get_pane_mut(sibling)
        .unwrap()
        .closed = true;
    assert!(
        app.balance_plugin_panes(tab_id as u64)
            .unwrap_err()
            .contains("closed")
    );
    app.tabs
        .as_mut()
        .unwrap()
        .tab_mut_by_id(tab_id)
        .unwrap()
        .pane_manager
        .as_mut()
        .unwrap()
        .get_pane_mut(sibling)
        .unwrap()
        .closed = false;

    let tab = app.tabs.as_mut().unwrap().tab_mut_by_id(tab_id).unwrap();
    tab.closing_at = Some(Instant::now());
    assert!(
        app.balance_plugin_panes(tab_id as u64)
            .unwrap_err()
            .contains("closing")
    );
    let tab = app.tabs.as_mut().unwrap().tab_mut_by_id(tab_id).unwrap();
    tab.closing_at = None;
    tab.live_domain = Some(crate::tabs::LiveDomainTab {
        domain_id: "fake-domain".into(),
        tab_id: 8,
        pane_ids: HashMap::new(),
    });
    assert!(
        app.balance_plugin_panes(tab_id as u64)
            .unwrap_err()
            .contains("live-domain")
    );
    let tab = app.tabs.as_mut().unwrap().tab_mut_by_id(tab_id).unwrap();
    tab.live_domain = None;
    tab.tab_type = crate::tabs::TabType::Serial {
        name: "fixture".into(),
    };
    assert!(
        app.balance_plugin_panes(tab_id as u64)
            .unwrap_err()
            .contains("serial terminal")
    );
    let unsplit_id = app.tabs.as_mut().unwrap().new_tab().unwrap();
    assert!(
        app.balance_plugin_panes(unsplit_id as u64)
            .unwrap_err()
            .contains("not split")
    );
    assert!(
        app.balance_plugin_panes(u64::MAX)
            .unwrap_err()
            .contains("does not exist")
    );
}

#[cfg(unix)]
#[test]
fn plugin_pane_movement_refuses_unsupported_targets_before_detaching() {
    if !std::path::Path::new("/bin/sh").exists() {
        return;
    }

    fn fixture() -> (App, usize, usize, PaneId) {
        let shell = ShellConfig {
            program: Some("/bin/sh".into()),
            args: vec!["-c".into(), "sleep 2".into()],
            env: Default::default(),
        };
        let mut tabs = TabManager::new(80, 24, shell.clone(), 1_000, 8 * 1024 * 1024);
        let source_tab_id = tabs.new_tab().unwrap();
        let source = tabs.tab_mut_by_id(source_tab_id).unwrap();
        let owner = source.take_terminal_owner().unwrap();
        let mut panes = PaneManager::from_existing(
            owner,
            shell,
            1_000,
            8 * 1024 * 1024,
            800.0,
            480.0,
            10.0,
            20.0,
        );
        let sibling = panes.split(SplitDirection::Vertical).unwrap();
        source.pane_manager = Some(panes);
        let destination_tab_id = tabs.new_tab().unwrap();
        let mut app = App::new_headless(Config::default()).unwrap();
        app.tabs = Some(tabs);
        (app, source_tab_id, destination_tab_id, sibling)
    }

    fn move_result(
        app: &mut App,
        source_tab_id: u64,
        pane_id: u32,
        destination_tab_id: u64,
        target_pane_id: u32,
    ) -> serde_json::Value {
        let encoded = app
            .structured_workspace_action_event(
                "pane-mover",
                crate::plugin::structured::StructuredResponseAction::MovePane {
                    action_id: "refused-move".into(),
                    source_tab_id,
                    pane_id,
                    destination_tab_id,
                    target_pane_id,
                    direction: crate::control::protocol::SplitDirection::Vertical,
                },
            )
            .unwrap();
        serde_json::from_str(&encoded).unwrap()
    }

    fn assert_refused(result: &serde_json::Value, expected: &str) {
        assert_eq!(result["event"]["ok"], false);
        assert!(result["event"]["destination_pane_id"].is_null());
        assert!(result["event"]["source_remaining_panes"].is_null());
        assert!(result["event"]["active_tab_id"].is_null());
        assert!(result["event"]["focused_pane_id"].is_null());
        assert!(result["event"]["active_changed"].is_null());
        assert!(
            result["event"]["error"]
                .as_str()
                .is_some_and(|error| error.contains(expected)),
            "unexpected error: {}",
            result["event"]["error"]
        );
    }

    fn assert_source_intact(app: &App, source_tab_id: usize) {
        let pane_count = app
            .tabs
            .as_ref()
            .unwrap()
            .tab_by_id(source_tab_id)
            .unwrap()
            .pane_manager
            .as_ref()
            .unwrap()
            .pane_count();
        assert_eq!(pane_count, 2);
    }

    let (mut same, source, _, sibling) = fixture();
    assert_refused(
        &move_result(&mut same, source as u64, 0, source as u64, sibling.0),
        "same tab",
    );
    assert_source_intact(&same, source);

    let (mut missing, source, destination, _) = fixture();
    assert_refused(
        &move_result(&mut missing, u64::MAX, 0, destination as u64, 0),
        "does not exist",
    );
    assert_refused(
        &move_result(&mut missing, source as u64, 0, destination as u64, u32::MAX),
        "does not exist",
    );
    assert_source_intact(&missing, source);

    let (mut closing, source, destination, _) = fixture();
    closing
        .tabs
        .as_mut()
        .unwrap()
        .tab_mut_by_id(source)
        .unwrap()
        .closing_at = Some(Instant::now());
    assert_refused(
        &move_result(&mut closing, source as u64, 0, destination as u64, 0),
        "closing",
    );
    assert_source_intact(&closing, source);

    let (mut closed, source, destination, _) = fixture();
    closed
        .tabs
        .as_mut()
        .unwrap()
        .tab_mut_by_id(source)
        .unwrap()
        .pane_manager
        .as_mut()
        .unwrap()
        .get_pane_mut(PaneId(0))
        .unwrap()
        .closed = true;
    assert_refused(
        &move_result(&mut closed, source as u64, 0, destination as u64, 0),
        "closed",
    );
    assert_source_intact(&closed, source);

    let (mut last_live, source, destination, sibling) = fixture();
    last_live
        .tabs
        .as_mut()
        .unwrap()
        .tab_mut_by_id(source)
        .unwrap()
        .pane_manager
        .as_mut()
        .unwrap()
        .get_pane_mut(sibling)
        .unwrap()
        .closed = true;
    assert_refused(
        &move_result(&mut last_live, source as u64, 0, destination as u64, 0),
        "last pane",
    );
    assert_source_intact(&last_live, source);

    let (mut live, source, destination, _) = fixture();
    live.tabs
        .as_mut()
        .unwrap()
        .tab_mut_by_id(source)
        .unwrap()
        .live_domain = Some(crate::tabs::LiveDomainTab {
        domain_id: "fake-domain".into(),
        tab_id: 8,
        pane_ids: HashMap::new(),
    });
    assert_refused(
        &move_result(&mut live, source as u64, 0, destination as u64, 0),
        "Live-domain",
    );
    assert_source_intact(&live, source);

    let (mut serial, source, destination, _) = fixture();
    serial
        .tabs
        .as_mut()
        .unwrap()
        .tab_mut_by_id(destination)
        .unwrap()
        .tab_type = crate::tabs::TabType::Serial {
        name: "fixture".into(),
    };
    assert_refused(
        &move_result(&mut serial, source as u64, 0, destination as u64, 0),
        "serial terminal",
    );
    assert_source_intact(&serial, source);

    let (mut closed_anchor, source, destination, _) = fixture();
    let shell = ShellConfig {
        program: Some("/bin/sh".into()),
        args: vec!["-c".into(), "sleep 2".into()],
        env: Default::default(),
    };
    let destination_tab = closed_anchor
        .tabs
        .as_mut()
        .unwrap()
        .tab_mut_by_id(destination)
        .unwrap();
    let owner = destination_tab.take_terminal_owner().unwrap();
    let mut destination_panes = PaneManager::from_existing(
        owner,
        shell,
        1_000,
        8 * 1024 * 1024,
        800.0,
        480.0,
        10.0,
        20.0,
    );
    destination_panes.split(SplitDirection::Vertical).unwrap();
    destination_panes.get_pane_mut(PaneId(0)).unwrap().closed = true;
    destination_tab.pane_manager = Some(destination_panes);
    assert_refused(
        &move_result(&mut closed_anchor, source as u64, 0, destination as u64, 0),
        "destination pane 0 is closed",
    );
    assert_source_intact(&closed_anchor, source);

    let (mut plugin, source, destination, _) = fixture();
    plugin
        .tabs
        .as_mut()
        .unwrap()
        .tab_mut_by_id(source)
        .unwrap()
        .pane_manager
        .as_mut()
        .unwrap()
        .get_pane_mut(PaneId(0))
        .unwrap()
        .launch = crate::workspace::WorkspaceLaunch::Plugin {
        plugin_id: "fake".into(),
        profile: None,
    };
    assert_refused(
        &move_result(&mut plugin, source as u64, 0, destination as u64, 0),
        "move the whole tab",
    );
    assert_source_intact(&plugin, source);

    let (mut last, source, destination, _) = fixture();
    assert_refused(
        &move_result(&mut last, destination as u64, 0, source as u64, 0),
        "Split the source tab",
    );
    assert_source_intact(&last, source);
}

#[cfg(unix)]
#[test]
fn plugin_pane_closure_is_exact_correlated_and_preserves_last_pane() {
    if !std::path::Path::new("/bin/sh").exists() {
        return;
    }
    let shell = ShellConfig {
        program: Some("/bin/sh".into()),
        args: vec!["-c".into(), "sleep 2".into()],
        env: Default::default(),
    };
    let mut tabs = TabManager::new(80, 24, shell.clone(), 1_000, 8 * 1024 * 1024);
    let target_tab_id = tabs.new_tab().unwrap();
    let tab = tabs.tab_mut_by_id(target_tab_id).unwrap();
    let owner = tab.take_terminal_owner().unwrap();
    let mut panes = PaneManager::from_existing(
        owner,
        shell,
        1_000,
        8 * 1024 * 1024,
        800.0,
        480.0,
        10.0,
        20.0,
    );
    let first_sibling = panes.split(SplitDirection::Vertical).unwrap();
    let second_sibling = panes
        .split_target(first_sibling, SplitDirection::Horizontal)
        .unwrap();
    tab.pane_manager = Some(panes);
    let active_tab_id = tabs.new_tab().unwrap();
    let mut app = App::new_headless(Config::default()).unwrap();
    app.tabs = Some(tabs);
    app.copy_mode_target = Some(CopyModeTarget {
        tab: target_tab_id,
        pane: Some(PaneId(0)),
    });
    app.broadcast = BroadcastState::Active {
        tab_id: target_tab_id,
        targets: vec![PaneId(0), first_sibling],
    };

    let closed = app
        .structured_workspace_action_event(
            "pane-pruner",
            crate::plugin::structured::StructuredResponseAction::ClosePane {
                action_id: "close-original".into(),
                tab_id: target_tab_id as u64,
                pane_id: 0,
            },
        )
        .unwrap();
    let closed: serde_json::Value = serde_json::from_str(&closed).unwrap();
    assert_eq!(closed["event"]["type"], "workspace-action-result");
    assert_eq!(closed["event"]["action"], "close-pane");
    assert_eq!(closed["event"]["action_id"], "close-original");
    assert_eq!(closed["event"]["ok"], true);
    assert_eq!(closed["event"]["tab_id"], target_tab_id as u64);
    assert_eq!(closed["event"]["pane_id"], 0);
    assert_eq!(closed["event"]["remaining_panes"], 2);
    assert_eq!(
        closed["event"]["focused_pane_id"],
        u64::from(second_sibling.0)
    );
    assert_eq!(
        app.tabs.as_ref().unwrap().active_tab_id(),
        Some(active_tab_id)
    );
    assert!(app.copy_mode_target.is_none());
    assert!(matches!(app.broadcast, BroadcastState::Inactive));

    let stale = app
        .structured_workspace_action_event(
            "pane-pruner",
            crate::plugin::structured::StructuredResponseAction::ClosePane {
                action_id: "stale-pane".into(),
                tab_id: target_tab_id as u64,
                pane_id: u32::MAX,
            },
        )
        .unwrap();
    let stale: serde_json::Value = serde_json::from_str(&stale).unwrap();
    assert_eq!(stale["event"]["ok"], false);
    assert!(stale["event"]["remaining_panes"].is_null());
    assert!(stale["event"]["focused_pane_id"].is_null());
    assert!(
        stale["event"]["error"]
            .as_str()
            .is_some_and(|error| error.contains("does not exist"))
    );
    assert_eq!(
        app.tabs
            .as_ref()
            .unwrap()
            .tab_by_id(target_tab_id)
            .unwrap()
            .pane_manager
            .as_ref()
            .unwrap()
            .pane_count(),
        2
    );

    app.tabs
        .as_mut()
        .unwrap()
        .tab_mut_by_id(target_tab_id)
        .unwrap()
        .live_domain = Some(crate::tabs::LiveDomainTab {
        domain_id: "fake-domain".into(),
        tab_id: 8,
        pane_ids: HashMap::new(),
    });
    let live = app
        .structured_workspace_action_event(
            "pane-pruner",
            crate::plugin::structured::StructuredResponseAction::ClosePane {
                action_id: "reject-live".into(),
                tab_id: target_tab_id as u64,
                pane_id: first_sibling.0,
            },
        )
        .unwrap();
    let live: serde_json::Value = serde_json::from_str(&live).unwrap();
    assert_eq!(live["event"]["ok"], false);
    assert!(
        live["event"]["error"]
            .as_str()
            .is_some_and(|error| error.contains("live-domain"))
    );

    let tab = app
        .tabs
        .as_mut()
        .unwrap()
        .tab_mut_by_id(target_tab_id)
        .unwrap();
    tab.live_domain = None;
    tab.tab_type = crate::tabs::TabType::Serial {
        name: "fixture".into(),
    };
    let serial = app
        .structured_workspace_action_event(
            "pane-pruner",
            crate::plugin::structured::StructuredResponseAction::ClosePane {
                action_id: "reject-serial".into(),
                tab_id: target_tab_id as u64,
                pane_id: first_sibling.0,
            },
        )
        .unwrap();
    let serial: serde_json::Value = serde_json::from_str(&serial).unwrap();
    assert_eq!(serial["event"]["ok"], false);
    assert!(
        serial["event"]["error"]
            .as_str()
            .is_some_and(|error| error.contains("Serial"))
    );

    app.tabs
        .as_mut()
        .unwrap()
        .tab_mut_by_id(target_tab_id)
        .unwrap()
        .tab_type = crate::tabs::TabType::Shell;
    let collapsed = app
        .structured_workspace_action_event(
            "pane-pruner",
            crate::plugin::structured::StructuredResponseAction::ClosePane {
                action_id: "collapse-split".into(),
                tab_id: target_tab_id as u64,
                pane_id: first_sibling.0,
            },
        )
        .unwrap();
    let collapsed: serde_json::Value = serde_json::from_str(&collapsed).unwrap();
    assert_eq!(collapsed["event"]["ok"], true);
    assert_eq!(collapsed["event"]["remaining_panes"], 1);
    assert_eq!(
        collapsed["event"]["focused_pane_id"],
        u64::from(second_sibling.0)
    );
    let tab = app.tabs.as_ref().unwrap().tab_by_id(target_tab_id).unwrap();
    assert!(tab.pane_manager.is_none());
    assert_eq!(tab.single_pane_id(), second_sibling);

    let sole = app
        .structured_workspace_action_event(
            "pane-pruner",
            crate::plugin::structured::StructuredResponseAction::ClosePane {
                action_id: "reject-last".into(),
                tab_id: target_tab_id as u64,
                pane_id: second_sibling.0,
            },
        )
        .unwrap();
    let sole: serde_json::Value = serde_json::from_str(&sole).unwrap();
    assert_eq!(sole["event"]["ok"], false);
    assert!(
        sole["event"]["error"]
            .as_str()
            .is_some_and(|error| error.contains("sole pane"))
    );
    assert_eq!(
        app.tabs.as_ref().unwrap().active_tab_id(),
        Some(active_tab_id)
    );

    app.tabs
        .as_mut()
        .unwrap()
        .tab_mut_by_id(target_tab_id)
        .unwrap()
        .closing_at = Some(Instant::now());
    let closing = app
        .structured_workspace_action_event(
            "pane-pruner",
            crate::plugin::structured::StructuredResponseAction::ClosePane {
                action_id: "reject-closing".into(),
                tab_id: target_tab_id as u64,
                pane_id: second_sibling.0,
            },
        )
        .unwrap();
    let closing: serde_json::Value = serde_json::from_str(&closing).unwrap();
    assert_eq!(closing["event"]["ok"], false);
    assert!(
        closing["event"]["error"]
            .as_str()
            .is_some_and(|error| error.contains("closing"))
    );

    let missing_tab = app
        .structured_workspace_action_event(
            "pane-pruner",
            crate::plugin::structured::StructuredResponseAction::ClosePane {
                action_id: "missing-tab".into(),
                tab_id: u64::MAX,
                pane_id: second_sibling.0,
            },
        )
        .unwrap();
    let missing_tab: serde_json::Value = serde_json::from_str(&missing_tab).unwrap();
    assert_eq!(missing_tab["event"]["ok"], false);
    assert!(
        missing_tab["event"]["error"]
            .as_str()
            .is_some_and(|error| error.contains("does not exist") || error.contains("fit"))
    );
}

#[cfg(unix)]
#[test]
fn plugin_tab_closure_is_scheduled_exact_bounded_and_hook_suppressed() {
    if !std::path::Path::new("/bin/sh").exists() {
        return;
    }
    let shell = ShellConfig {
        program: Some("/bin/sh".into()),
        args: vec!["-c".into(), "sleep 2".into()],
        env: Default::default(),
    };
    let mut tabs = TabManager::new(80, 24, shell, 1_000, 8 * 1024 * 1024);
    let survivor = tabs.new_tab().unwrap();
    let serial = tabs.new_tab().unwrap();
    let live_candidate = tabs.new_tab().unwrap();
    let initially_active = tabs.new_tab().unwrap();
    tabs.tab_mut_by_id(survivor).unwrap().pinned = true;
    tabs.tab_mut_by_id(serial).unwrap().pinned = true;
    tabs.tab_mut_by_id(serial).unwrap().tab_type = crate::tabs::TabType::Serial {
        name: "fixture".into(),
    };
    let mut app = App::new_headless(Config::default()).unwrap();
    app.tabs = Some(tabs);
    app.plugin_manager.unload_all();
    let observed = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let context = app.build_plugin_context();
    app.plugin_manager
        .register(
            Box::new(CountingTabClosedPlugin(std::sync::Arc::clone(&observed))),
            &context,
        )
        .unwrap();

    let background = app
        .structured_workspace_action_event(
            "tab-pruner",
            crate::plugin::structured::StructuredResponseAction::CloseTab {
                action_id: "close-serial-background".into(),
                tab_id: serial as u64,
            },
        )
        .unwrap();
    let background: serde_json::Value = serde_json::from_str(&background).unwrap();
    assert_eq!(background["event"]["type"], "workspace-action-result");
    assert_eq!(background["event"]["action"], "close-tab");
    assert_eq!(background["event"]["action_id"], "close-serial-background");
    assert_eq!(background["event"]["ok"], true);
    assert_eq!(background["event"]["tab_id"], serial as u64);
    assert_eq!(background["event"]["scheduled"], true);
    assert_eq!(background["event"]["remaining_tabs"], 3);
    assert_eq!(
        background["event"]["active_tab_id"],
        initially_active as u64
    );
    assert_eq!(background["event"]["active_changed"], false);
    assert!(
        app.tabs
            .as_ref()
            .unwrap()
            .tab_by_id(serial)
            .unwrap()
            .closing_at
            .is_some()
    );
    assert_eq!(
        app.tabs.as_ref().unwrap().active_tab_id(),
        Some(initially_active)
    );
    assert_eq!(observed.load(std::sync::atomic::Ordering::SeqCst), 0);

    app.tabs
        .as_mut()
        .unwrap()
        .tab_mut_by_id(live_candidate)
        .unwrap()
        .live_domain = Some(crate::tabs::LiveDomainTab {
        domain_id: "fake-domain".into(),
        tab_id: 8,
        pane_ids: HashMap::new(),
    });
    let live = app
        .structured_workspace_action_event(
            "tab-pruner",
            crate::plugin::structured::StructuredResponseAction::CloseTab {
                action_id: "reject-live".into(),
                tab_id: live_candidate as u64,
            },
        )
        .unwrap();
    let live: serde_json::Value = serde_json::from_str(&live).unwrap();
    assert_eq!(live["event"]["ok"], false);
    assert_eq!(live["event"]["scheduled"], false);
    assert!(live["event"]["remaining_tabs"].is_null());
    assert!(live["event"]["active_tab_id"].is_null());
    assert!(live["event"]["active_changed"].is_null());
    assert!(
        live["event"]["error"]
            .as_str()
            .is_some_and(|error| error.contains("live-domain"))
    );
    assert!(
        app.tabs
            .as_ref()
            .unwrap()
            .tab_by_id(live_candidate)
            .unwrap()
            .closing_at
            .is_none()
    );

    let tab = app
        .tabs
        .as_mut()
        .unwrap()
        .tab_mut_by_id(live_candidate)
        .unwrap();
    tab.live_domain = None;
    tab.closed = true;
    let closed = app
        .structured_workspace_action_event(
            "tab-pruner",
            crate::plugin::structured::StructuredResponseAction::CloseTab {
                action_id: "reject-closed".into(),
                tab_id: live_candidate as u64,
            },
        )
        .unwrap();
    let closed: serde_json::Value = serde_json::from_str(&closed).unwrap();
    assert_eq!(closed["event"]["ok"], false);
    assert_eq!(closed["event"]["scheduled"], false);
    assert!(
        closed["event"]["error"]
            .as_str()
            .is_some_and(|error| error.contains("already closing"))
    );
    app.tabs
        .as_mut()
        .unwrap()
        .tab_mut_by_id(live_candidate)
        .unwrap()
        .closed = false;

    let already_closing = app
        .structured_workspace_action_event(
            "tab-pruner",
            crate::plugin::structured::StructuredResponseAction::CloseTab {
                action_id: "already-closing".into(),
                tab_id: serial as u64,
            },
        )
        .unwrap();
    let already_closing: serde_json::Value = serde_json::from_str(&already_closing).unwrap();
    assert_eq!(already_closing["event"]["ok"], false);
    assert!(
        already_closing["event"]["error"]
            .as_str()
            .is_some_and(|error| error.contains("already closing"))
    );

    let missing = app
        .structured_workspace_action_event(
            "tab-pruner",
            crate::plugin::structured::StructuredResponseAction::CloseTab {
                action_id: "missing-tab".into(),
                tab_id: u64::MAX,
            },
        )
        .unwrap();
    let missing: serde_json::Value = serde_json::from_str(&missing).unwrap();
    assert_eq!(missing["event"]["ok"], false);
    assert!(
        missing["event"]["error"]
            .as_str()
            .is_some_and(|error| error.contains("does not exist") || error.contains("fit"))
    );

    app.tabs
        .as_mut()
        .unwrap()
        .tab_mut_by_id(live_candidate)
        .unwrap()
        .live_domain = None;
    app.copy_mode_target = Some(CopyModeTarget {
        tab: initially_active,
        pane: None,
    });
    let active = app
        .structured_workspace_action_event(
            "tab-pruner",
            crate::plugin::structured::StructuredResponseAction::CloseTab {
                action_id: "close-active".into(),
                tab_id: initially_active as u64,
            },
        )
        .unwrap();
    let active: serde_json::Value = serde_json::from_str(&active).unwrap();
    assert_eq!(active["event"]["ok"], true);
    assert_eq!(active["event"]["scheduled"], true);
    assert_eq!(active["event"]["remaining_tabs"], 2);
    assert_eq!(active["event"]["active_tab_id"], live_candidate as u64);
    assert_eq!(active["event"]["active_changed"], true);
    assert_eq!(
        app.tabs.as_ref().unwrap().active_tab_id(),
        Some(live_candidate)
    );
    assert!(app.copy_mode_target.is_none());

    let next_active = app
        .structured_workspace_action_event(
            "tab-pruner",
            crate::plugin::structured::StructuredResponseAction::CloseTab {
                action_id: "leave-one".into(),
                tab_id: live_candidate as u64,
            },
        )
        .unwrap();
    let next_active: serde_json::Value = serde_json::from_str(&next_active).unwrap();
    assert_eq!(next_active["event"]["ok"], true);
    assert_eq!(next_active["event"]["remaining_tabs"], 1);
    assert_eq!(next_active["event"]["active_tab_id"], survivor as u64);
    assert_eq!(next_active["event"]["active_changed"], true);

    let final_tab = app
        .structured_workspace_action_event(
            "tab-pruner",
            crate::plugin::structured::StructuredResponseAction::CloseTab {
                action_id: "reject-final".into(),
                tab_id: survivor as u64,
            },
        )
        .unwrap();
    let final_tab: serde_json::Value = serde_json::from_str(&final_tab).unwrap();
    assert_eq!(final_tab["event"]["ok"], false);
    assert_eq!(final_tab["event"]["scheduled"], false);
    assert!(
        final_tab["event"]["error"]
            .as_str()
            .is_some_and(|error| error.contains("final non-closing tab"))
    );
    assert!(
        app.tabs
            .as_ref()
            .unwrap()
            .tab_by_id(survivor)
            .unwrap()
            .closing_at
            .is_none()
    );
    assert_eq!(app.tabs.as_ref().unwrap().active_tab_id(), Some(survivor));
    assert_eq!(observed.load(std::sync::atomic::Ordering::SeqCst), 0);
}

#[cfg(unix)]
#[test]
fn control_tab_closure_retains_final_tab_authority() {
    if !std::path::Path::new("/bin/sh").exists() {
        return;
    }
    let shell = ShellConfig {
        program: Some("/bin/sh".into()),
        args: vec!["-c".into(), "sleep 2".into()],
        env: Default::default(),
    };
    let mut tabs = TabManager::new(80, 24, shell, 1_000, 8 * 1024 * 1024);
    let tab_id = tabs.new_tab().unwrap();
    let mut app = App::new_headless(Config::default()).unwrap();
    app.tabs = Some(tabs);
    app.plugin_manager.unload_all();
    let observed = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let context = app.build_plugin_context();
    app.plugin_manager
        .register(
            Box::new(CountingTabClosedPlugin(std::sync::Arc::clone(&observed))),
            &context,
        )
        .unwrap();

    app.close_control_tab(tab_id as u64).unwrap();
    assert!(
        app.tabs
            .as_ref()
            .unwrap()
            .tab_by_id(tab_id)
            .unwrap()
            .closing_at
            .is_some()
    );
    assert_eq!(observed.load(std::sync::atomic::Ordering::SeqCst), 1);
}

#[cfg(unix)]
#[test]
fn plugin_tab_creation_returns_stable_identity_without_lifecycle_redispatch() {
    if !std::path::Path::new("/bin/sh").exists() {
        return;
    }
    #[derive(Debug)]
    struct TabCreatedObserver(std::sync::Arc<std::sync::atomic::AtomicUsize>);

    impl crate::plugin::hooks::Plugin for TabCreatedObserver {
        fn id(&self) -> &str {
            "tab-created-observer"
        }

        fn name(&self) -> &str {
            "Tab Created Observer"
        }

        fn on_hook(
            &mut self,
            hook: &crate::plugin::Hook,
            _ctx: &crate::plugin::PluginContext,
        ) -> crate::plugin::hooks::HookResponse {
            if matches!(hook, crate::plugin::Hook::TabCreated { .. }) {
                self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            }
            crate::plugin::hooks::HookResponse::Ignored
        }
    }

    let shell = ShellConfig {
        program: Some("/bin/sh".into()),
        args: vec!["-c".into(), "sleep 2".into()],
        env: Default::default(),
    };
    let mut tabs = TabManager::new(80, 24, shell, 1_000, 8 * 1024 * 1024);
    let initial_tab_id = tabs.new_tab().unwrap();
    let mut app = App::new_headless(Config::default()).unwrap();
    app.tabs = Some(tabs);
    app.plugin_manager.unload_all();
    let observed = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let context = app.build_plugin_context();
    app.plugin_manager
        .register(
            Box::new(TabCreatedObserver(std::sync::Arc::clone(&observed))),
            &context,
        )
        .unwrap();

    let created = app
        .structured_workspace_action_event(
            "tab-builder",
            crate::plugin::structured::StructuredResponseAction::CreateTab {
                action_id: "new-task".into(),
            },
        )
        .unwrap();
    let created: serde_json::Value = serde_json::from_str(&created).unwrap();
    assert_eq!(created["event"]["type"], "workspace-action-result");
    assert_eq!(created["event"]["action"], "create-tab");
    assert_eq!(created["event"]["action_id"], "new-task");
    assert_eq!(created["event"]["ok"], true);
    let created_tab_id = created["event"]["created_tab_id"].as_u64().unwrap() as usize;
    assert_ne!(created_tab_id, initial_tab_id);
    assert_eq!(app.tabs.as_ref().unwrap().count(), 2);
    assert_eq!(
        app.tabs.as_ref().unwrap().active_tab().unwrap().id,
        created_tab_id
    );
    assert_eq!(observed.load(std::sync::atomic::Ordering::SeqCst), 0);

    let control_tab_id = app.create_control_tab().unwrap() as usize;
    assert_eq!(app.tabs.as_ref().unwrap().count(), 3);
    assert_eq!(
        app.tabs.as_ref().unwrap().active_tab().unwrap().id,
        control_tab_id
    );
    assert_eq!(observed.load(std::sync::atomic::Ordering::SeqCst), 1);

    app.tabs
        .as_mut()
        .unwrap()
        .active_tab_mut()
        .unwrap()
        .live_domain = Some(crate::tabs::LiveDomainTab {
        domain_id: "fake-domain".into(),
        tab_id: 8,
        pane_ids: HashMap::new(),
    });
    let live = app
        .structured_workspace_action_event(
            "tab-builder",
            crate::plugin::structured::StructuredResponseAction::CreateTab {
                action_id: "reject-live".into(),
            },
        )
        .unwrap();
    let live: serde_json::Value = serde_json::from_str(&live).unwrap();
    assert_eq!(live["event"]["ok"], false);
    assert!(live["event"]["created_tab_id"].is_null());
    assert!(
        live["event"]["error"]
            .as_str()
            .is_some_and(|error| error.contains("live domain"))
    );
    assert_eq!(app.tabs.as_ref().unwrap().count(), 3);

    let tabs = app.tabs.as_mut().unwrap();
    tabs.active_tab_mut().unwrap().live_domain = None;
    tabs.set_shell_config(ShellConfig {
        program: Some("/definitely/missing/termeric-shell".into()),
        args: Vec::new(),
        env: Default::default(),
    });
    let launch_failure = app
        .structured_workspace_action_event(
            "tab-builder",
            crate::plugin::structured::StructuredResponseAction::CreateTab {
                action_id: "failed-launch".into(),
            },
        )
        .unwrap();
    let launch_failure: serde_json::Value = serde_json::from_str(&launch_failure).unwrap();
    assert_eq!(launch_failure["event"]["ok"], false);
    assert!(launch_failure["event"]["created_tab_id"].is_null());
    assert_eq!(app.tabs.as_ref().unwrap().count(), 3);
    assert_eq!(observed.load(std::sync::atomic::Ordering::SeqCst), 1);

    assert!(
        super::workspace_actions::validate_ordinary_tab_creation_capacity(
            crate::control::protocol::MAX_TABS_PER_WINDOW,
        )
        .unwrap_err()
        .contains("tab limit")
    );

    let mut uninitialized = App::new_headless(Config::default()).unwrap();
    let unavailable = uninitialized
        .structured_workspace_action_event(
            "tab-builder",
            crate::plugin::structured::StructuredResponseAction::CreateTab {
                action_id: "no-tabs".into(),
            },
        )
        .unwrap();
    let unavailable: serde_json::Value = serde_json::from_str(&unavailable).unwrap();
    assert_eq!(unavailable["event"]["ok"], false);
    assert!(
        unavailable["event"]["error"]
            .as_str()
            .is_some_and(|error| error.contains("tab manager"))
    );
}

#[cfg(unix)]
#[test]
fn control_input_is_opt_in_exact_paste_aware_and_guarded_without_provider_execution() {
    if !std::path::Path::new("/bin/sh").exists() {
        return;
    }
    let script = concat!(
        "stty raw -echo; ",
        "printf '\rREADY\r\n'; ",
        "bytes=$(dd bs=1 count=17 2>/dev/null | od -An -tx1); ",
        "printf '\rinput=%s\r\n' \"$bytes\"; ",
        "sleep 0.1"
    );
    let mut tabs = TabManager::new(80, 24, ShellConfig::default(), 1_000, 8 * 1024 * 1024);
    let tab_id = tabs
        .new_tab_with_command(
            "/bin/sh",
            &["-c".into(), script.into()],
            None,
            Some("control-input-fixture"),
            None,
            HashMap::new(),
        )
        .unwrap();
    tabs.active_tab_mut()
        .unwrap()
        .focused_terminal_mut()
        .unwrap()
        .set_mode(termeric_core::term::TerminalMode::BRACKETED_PASTE, true);

    let mut app = App::new_headless(Config::default()).unwrap();
    app.tabs = Some(tabs);
    assert!(
        app.send_control_text(tab_id as u64, 0, "safe", true, true)
            .unwrap_err()
            .contains("disabled")
    );
    app.config.security.allow_gui_control_input = true;
    assert!(
        app.send_control_text(tab_id as u64, 99, "wrong", false, false)
            .is_err()
    );

    let ready_deadline = Instant::now() + Duration::from_secs(3);
    while Instant::now() < ready_deadline {
        app.tabs.as_mut().unwrap().poll_events();
        if app
            .active_terminal()
            .unwrap()
            .grid()
            .export_scrollback_text()
            .join("\n")
            .contains("READY")
        {
            break;
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    assert_eq!(
        app.send_plugin_text(tab_id as u64, 0, "safe", true, true),
        Ok((
            5,
            17,
            crate::control::protocol::InputDisposition::Sent,
            None
        ))
    );

    let expected = "1b 5b 32 30 30 7e 73 61 66 65 1b 5b 32 30 31 7e 0d";
    let deadline = Instant::now() + Duration::from_secs(3);
    let mut output = String::new();
    while Instant::now() < deadline {
        app.tabs.as_mut().unwrap().poll_events();
        output = app
            .active_terminal()
            .unwrap()
            .grid()
            .export_scrollback_text()
            .join("\n");
        if output
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
            .contains(expected)
        {
            break;
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    assert!(
        output
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
            .contains(expected),
        "exact control input was {output:?}"
    );

    app.tabs
        .as_mut()
        .unwrap()
        .active_tab_mut()
        .unwrap()
        .tab_type = crate::tabs::TabType::Plugin {
        plugin_id: "hermetic-fixture".into(),
    };
    let (_, _, disposition, confirmation_id) = app
        .send_control_text(tab_id as u64, 0, "rm -rf /", false, true)
        .unwrap();
    assert_eq!(
        disposition,
        crate::control::protocol::InputDisposition::PendingConfirmation
    );
    let confirmation_id = confirmation_id.unwrap();
    assert!(app.guardrail.is_awaiting(confirmation_id));
    app.apply_guardrail_action(crate::terminal::guardrails::GuardrailAction::Cancel {
        request_id: confirmation_id,
    });
    assert!(!app.guardrail.is_awaiting(confirmation_id));
}

#[cfg(unix)]
#[test]
fn diff_review_done_marks_only_its_original_tab_and_exact_diff() {
    if !std::path::Path::new("/bin/sh").exists() {
        return;
    }
    let mut tabs = TabManager::new(80, 24, ShellConfig::default(), 1_000, 8 * 1024 * 1024);
    for title in ["first-diff-tab", "active-other-tab"] {
        tabs.new_tab_with_command(
            "/bin/sh",
            &["-c".into(), "sleep 2".into()],
            None,
            Some(title),
            None,
            HashMap::new(),
        )
        .unwrap();
    }
    for (index, tab) in tabs.all_tabs_mut().iter_mut().enumerate() {
        for line in [
            format!("--- a/fixture-{index}.rs"),
            format!("+++ b/fixture-{index}.rs"),
            "@@ -1,1 +1,1 @@".into(),
            "-old".into(),
            "+new".into(),
            "finished".into(),
        ] {
            tab.diff_detector.feed_line(&line);
        }
    }
    let first_tab_id = tabs.iter().next().unwrap().1.id;
    let first_diff = tabs
        .iter()
        .next()
        .unwrap()
        .1
        .diff_detector
        .latest_unreviewed()
        .unwrap()
        .clone();
    tabs.switch_to(1);

    let mut app = App::new_headless(Config::default()).unwrap();
    app.tabs = Some(tabs);
    let request_id = app.diff_review.show(&first_diff, first_tab_id);
    app.apply_diff_review_action(crate::ui::diff_review::DiffReviewAction::Dismiss { request_id });

    let tabs = app.tabs.as_ref().unwrap();
    assert_eq!(tabs.active_index(), 1);
    let mut tabs = tabs.iter();
    assert_eq!(tabs.next().unwrap().1.diff_detector.unreviewed_count(), 0);
    assert_eq!(tabs.next().unwrap().1.diff_detector.unreviewed_count(), 1);
}

#[cfg(unix)]
#[test]
fn harvest_search_switches_to_the_exact_producing_terminal() {
    if !std::path::Path::new("/bin/sh").exists() {
        return;
    }
    let mut tabs = TabManager::new(80, 24, ShellConfig::default(), 1_000, 8 * 1024 * 1024);
    let source_tab = tabs
        .new_tab_with_command(
            "/bin/sh",
            &["-c".into(), "sleep 2".into()],
            None,
            Some("harvest-source"),
            None,
            HashMap::new(),
        )
        .unwrap();
    tabs.active_tab_mut()
        .unwrap()
        .focused_terminal_mut()
        .unwrap()
        .process(
            b"\x1b]133;A\x07\x1b]133;B\x07echo exact\r\n\x1b]133;C\x07exact\r\n\x1b]133;D;0\x07",
        );
    let region = tabs
        .active_tab()
        .unwrap()
        .focused_terminal()
        .unwrap()
        .shell_integration()
        .prompt_tracker
        .completed_region(0)
        .unwrap()
        .clone();
    for index in 0..30 {
        tabs.active_tab_mut()
            .unwrap()
            .focused_terminal_mut()
            .unwrap()
            .process(format!("history-{index}\r\n").as_bytes());
    }
    let moved_region = tabs
        .active_tab()
        .unwrap()
        .focused_terminal()
        .unwrap()
        .shell_integration()
        .prompt_tracker
        .completed_region_by_id(region.id)
        .unwrap();
    assert!(moved_region.prompt_row < 0);
    let expected_scroll_offset = moved_region.prompt_row.saturating_neg() as usize;
    let other_tab = tabs
        .new_tab_with_command(
            "/bin/sh",
            &["-c".into(), "sleep 2".into()],
            None,
            Some("other-active-tab"),
            None,
            HashMap::new(),
        )
        .unwrap();
    assert_eq!(tabs.active_tab_id(), Some(other_tab));

    let mut app = App::new_headless(Config::default()).unwrap();
    app.tabs = Some(tabs);
    let mut entry = crate::harvest::CommandEntry::new_for_source(
        crate::harvest::HarvestSource {
            tab_id: source_tab,
            pane_id: None,
        },
        region.prompt_row,
        Some(0),
    );
    entry.region_id = region.id;
    entry.command = "echo exact".into();
    entry.output = "exact".into();
    let availability = app.sprout_availability_for_entry(&entry);
    assert!(availability.live_content_available);
    assert!(availability.action_enabled(crate::harvest::sprouts::SproutAction::SearchOutput));
    app.handle_action(crate::input::Action::SwitchToTab(0));
    app.active_terminal_mut().unwrap().process(b"\x1b[?1049h");
    let alternate = app.sprout_availability_for_entry(&entry);
    assert!(alternate.source_alternate_screen);
    assert!(!alternate.live_content_available);
    assert_eq!(
        alternate.action_label(crate::harvest::sprouts::SproutAction::SearchOutput),
        "Search (alternate screen)"
    );
    app.active_terminal_mut().unwrap().process(b"\x1b[?1049l");
    app.handle_action(crate::input::Action::SwitchToTab(1));
    app.execute_sprout_action(
        crate::harvest::sprouts::SproutAction::SearchOutput,
        Some(entry.clone()),
    );

    assert_eq!(app.tabs.as_ref().unwrap().active_tab_id(), Some(source_tab));
    assert!(app.search.is_active());
    app.execute_sprout_action(
        crate::harvest::sprouts::SproutAction::ScrollTo,
        Some(entry.clone()),
    );
    assert_eq!(
        app.active_terminal().unwrap().grid().scroll_offset(),
        expected_scroll_offset
    );

    app.active_terminal_mut()
        .unwrap()
        .set_scrollback_limits(1, 1024 * 1024);
    assert!(
        !app.active_terminal()
            .unwrap()
            .shell_integration()
            .prompt_tracker
            .completed_region_by_id(region.id)
            .unwrap()
            .content_available
    );
    let availability = app.sprout_availability_for_entry(&entry);
    assert!(!availability.live_content_available);
    assert!(!availability.action_enabled(crate::harvest::sprouts::SproutAction::SearchOutput));
    assert!(availability.action_enabled(crate::harvest::sprouts::SproutAction::CopyOutput));
    assert!(availability.action_enabled(crate::harvest::sprouts::SproutAction::Rerun));
    app.handle_action(crate::input::Action::SwitchToTab(1));
    assert_eq!(app.tabs.as_ref().unwrap().active_tab_id(), Some(other_tab));
    app.execute_sprout_action(crate::harvest::sprouts::SproutAction::Rerun, Some(entry));
    assert_eq!(app.tabs.as_ref().unwrap().active_tab_id(), Some(source_tab));
}

#[cfg(unix)]
#[test]
fn prompt_actions_walk_the_focused_terminals_scrollback_in_both_directions() {
    if !std::path::Path::new("/bin/sh").exists() {
        return;
    }
    let mut tabs = TabManager::new(14, 4, ShellConfig::default(), 1_000, 8 * 1024 * 1024);
    tabs.new_tab_with_command(
        "/bin/sh",
        &["-c".into(), "sleep 2".into()],
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
        .shell_integration()
        .prompt_tracker
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

#[cfg(unix)]
#[test]
fn semantic_command_completion_reaches_plugins_with_exact_bounded_source_text() {
    if !std::path::Path::new("/bin/sh").exists() {
        return;
    }
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
        ) -> crate::plugin::hooks::HookResponse {
            if matches!(hook, crate::plugin::Hook::CommandExecuted { .. }) {
                *self.captured.lock().unwrap() = Some(hook.clone());
                crate::plugin::hooks::HookResponse::Handled
            } else {
                crate::plugin::hooks::HookResponse::Ignored
            }
        }
    }

    let mut tabs = TabManager::new(20, 4, ShellConfig::default(), 1_000, 8 * 1024 * 1024);
    tabs.new_tab_with_command(
        "/bin/sh",
        &["-c".into(), "sleep 2".into()],
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
    app.plugin_manager.unload_all();
    let context = app.build_plugin_context();
    app.plugin_manager
        .register(
            Box::new(CommandCapture {
                captured: std::sync::Arc::clone(&captured),
            }),
            &context,
        )
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

#[cfg(unix)]
#[test]
fn clear_scrollback_action_retires_shell_metadata_with_the_erased_history() {
    if !std::path::Path::new("/bin/sh").exists() {
        return;
    }
    let mut tabs = TabManager::new(16, 4, ShellConfig::default(), 1_000, 8 * 1024 * 1024);
    tabs.new_tab_with_command(
        "/bin/sh",
        &["-c".into(), "sleep 2".into()],
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
    let region_id = terminal
        .shell_integration()
        .prompt_tracker
        .completed_region(0)
        .unwrap()
        .id;
    assert!(terminal.grid().scrollback_len() > 0);

    let mut app = App::new_headless(Config::default()).unwrap();
    app.tabs = Some(tabs);
    assert!(app.handle_action(crate::input::Action::ClearScrollback));

    let terminal = app.active_terminal().unwrap();
    assert_eq!(terminal.grid().scrollback_len(), 0);
    assert!(
        !terminal
            .shell_integration()
            .prompt_tracker
            .completed_region_by_id(region_id)
            .unwrap()
            .content_available
    );
    assert!(
        terminal
            .shell_integration()
            .prompt_tracker
            .all_prompts()
            .all(|row| row >= 0)
    );
}

#[cfg(unix)]
#[test]
fn completed_context_never_falls_back_to_a_replacement_plugin_tab() {
    if !std::path::Path::new("/bin/sh").exists() {
        return;
    }
    let mut tabs = TabManager::new(80, 24, ShellConfig::default(), 1_000, 8 * 1024 * 1024);
    for title in ["original-context-target", "replacement-context-target"] {
        tabs.new_tab_with_command(
            "/bin/sh",
            &["-c".into(), "sleep 2".into()],
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
            plugin_id: "fixture-plugin".into(),
            origin: TerminalEventOrigin::Tab {
                tab_id: original_id,
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
        Some("The original fixture-plugin context destination is no longer writable")
    );
    assert!(!app.context_picker.is_awaiting(request));
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

    first.handle_palette_result(PaletteResult::RunAction("core.toggle-fullscreen".into()));
    assert!(!first.fullscreen_requested);
}

#[test]
fn app_input_modules_use_the_focused_terminal_route() {
    for (name, source) in [
        ("mod.rs", include_str!("mod.rs")),
        ("mouse.rs", include_str!("mouse.rs")),
        ("desktop_io.rs", include_str!("desktop_io.rs")),
        ("plugin_actions.rs", include_str!("plugin_actions.rs")),
        ("sessions.rs", include_str!("sessions.rs")),
    ] {
        assert!(
            !source.contains("tab.terminal") && !source.contains("t.terminal"),
            "{name} bypasses Tab::focused_terminal and can target detached pane state"
        );
    }
}

#[cfg(unix)]
#[test]
fn plugin_context_cwd_follows_the_focused_pane_and_single_terminal() {
    if !std::path::Path::new("/bin/sh").exists() {
        return;
    }
    let temporary = tempfile::tempdir().unwrap();
    let left_cwd = temporary.path().join("left-context");
    let right_cwd = temporary.path().join("right-context");
    let single_cwd = temporary.path().join("single-context");
    for directory in [&left_cwd, &right_cwd, &single_cwd] {
        std::fs::create_dir(directory).unwrap();
    }

    let shell = ShellConfig {
        program: Some("/bin/sh".into()),
        args: vec!["-c".into(), "sleep 2".into()],
        env: Default::default(),
    };
    let mut panes = PaneManager::new(
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

    let mut tabs = TabManager::new(80, 24, ShellConfig::default(), 1_000, 8 * 1024 * 1024);
    tabs.new_tab_with_command(
        "/bin/sh",
        &["-c".into(), "sleep 2".into()],
        None,
        Some("plugin-context-routing"),
        None,
        HashMap::new(),
    )
    .unwrap();
    tabs.active_tab_mut().unwrap().pane_manager = Some(panes);
    let mut app = App::new_headless(Config::default()).unwrap();
    app.tabs = Some(tabs);

    assert_eq!(app.build_plugin_context().cwd.as_deref(), left_cwd.to_str());
    app.tabs
        .as_mut()
        .unwrap()
        .active_tab_mut()
        .unwrap()
        .pane_manager
        .as_mut()
        .unwrap()
        .focus(right);
    app.refresh_focused_terminal_surfaces();
    assert_eq!(
        app.build_plugin_context().cwd.as_deref(),
        right_cwd.to_str()
    );

    let mut single_tabs = TabManager::new(80, 24, ShellConfig::default(), 1_000, 8 * 1024 * 1024);
    single_tabs
        .new_tab_with_command(
            "/bin/sh",
            &["-c".into(), "sleep 2".into()],
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

#[cfg(unix)]
#[test]
fn idle_deadline_releases_stalled_synchronized_output_and_requests_the_final_frame() {
    if !std::path::Path::new("/bin/sh").exists() {
        return;
    }
    let mut app = App::new_headless(Config::default()).unwrap();
    let mut tabs = TabManager::new(80, 24, ShellConfig::default(), 1_000, 8 * 1024 * 1024);
    tabs.new_tab_with_command(
        "/bin/sh",
        &["-c".into(), "sleep 2".into()],
        None,
        Some("synchronized-output-timeout"),
        None,
        HashMap::new(),
    )
    .unwrap();
    app.tabs = Some(tabs);
    app.active_terminal_mut()
        .unwrap()
        .process(b"before\x1b[?2026h\rafter");
    let deadline = app
        .active_terminal()
        .unwrap()
        .synchronized_output_deadline()
        .unwrap();
    app.window_occluded = true;
    app.surface_lost = true;

    assert_eq!(app.next_synchronized_output_deadline(), Some(deadline));
    assert!(!app.redraw_scheduler.has_pending());

    app.poll_idle_sources(deadline);

    assert!(!app.active_terminal().unwrap().is_synchronized_output());
    assert!(app.redraw_scheduler.has_pending());
    assert!(
        app.active_terminal()
            .unwrap()
            .presentation_grid()
            .row(0)
            .unwrap()
            .to_string()
            .starts_with("after")
    );
}

#[test]
fn occluded_and_surface_lost_windows_suspend_presentation_only_deadlines() {
    let mut app = App::new_headless(Config::default()).unwrap();
    // Consume the intentionally overdue first Git poll so maintenance timing
    // cannot mask the presentation-deadline assertions below.
    app.poll_idle_sources(Instant::now());
    app.redraw_scheduler.begin_frame();

    let now = Instant::now();
    app.last_config_check = now;
    app.last_frame_time = now;
    app.focus_change_time = now;
    assert_eq!(
        app.next_idle_deadline(now),
        Some(now + Duration::from_millis(16))
    );

    app.window_occluded = true;
    assert!(
        app.next_idle_deadline(now).unwrap() >= now + Duration::from_millis(500),
        "occlusion retained a presentation-cadence wake"
    );
    app.poll_idle_sources(now + Duration::from_millis(32));
    assert!(!app.redraw_scheduler.has_pending());

    app.window_occluded = false;
    app.surface_lost = true;
    assert!(
        app.next_idle_deadline(now).unwrap() >= now + Duration::from_millis(500),
        "surface loss retained a presentation-cadence wake"
    );
    app.poll_idle_sources(now + Duration::from_millis(48));
    assert!(!app.redraw_scheduler.has_pending());
}

#[test]
fn surface_loss_has_one_deferred_recovery_deadline_instead_of_indefinite_pause() {
    let mut app = App::new_headless(Config::default()).unwrap();
    app.poll_idle_sources(Instant::now());
    let now = Instant::now();
    app.last_config_check = now;
    app.surface_lost = true;
    app.surface_lost_at = Some(now);

    let recovery_at = now + super::renderer_recovery::SURFACE_RECOVERY_DELAY;
    assert_eq!(app.next_idle_deadline(now), Some(recovery_at));
    app.poll_idle_sources(recovery_at - Duration::from_millis(1));
    assert!(app.surface_lost);
    assert!(!app.should_exit);

    // A headless controller has no native window to recreate. Reaching that
    // branch proves the deadline fired once; production retains its window and
    // reinstalls a renderer while leaving the terminal/PTY state untouched.
    app.poll_idle_sources(recovery_at);
    assert!(!app.surface_lost);
    assert!(app.surface_lost_at.is_none());
    assert!(app.should_exit);
}

#[test]
fn broadcast_arming_expiry_is_an_exact_idle_deadline_and_requests_cleanup_frame() {
    let mut app = App::new_headless(Config::default()).unwrap();
    app.poll_idle_sources(Instant::now());
    app.redraw_scheduler.begin_frame();
    let now = Instant::now();
    let expires = now + Duration::from_millis(100);
    app.last_config_check = now;
    app.window_occluded = true;
    app.broadcast = BroadcastState::Armed {
        tab_id: 1,
        expires,
        targets: vec![PaneId(1), PaneId(2)],
    };

    assert_eq!(app.next_idle_deadline(now), Some(expires));
    app.poll_idle_sources(expires - Duration::from_millis(1));
    assert!(matches!(app.broadcast, BroadcastState::Armed { .. }));
    app.poll_idle_sources(expires);
    assert!(matches!(app.broadcast, BroadcastState::Inactive));
    assert!(app.redraw_scheduler.has_pending());
}

#[cfg(unix)]
#[test]
fn kitty_animation_uses_its_exact_deadline_and_requests_one_frame() {
    if !std::path::Path::new("/bin/sh").exists() {
        return;
    }
    let mut app = App::new_headless(Config::default()).unwrap();
    let mut tabs = TabManager::new(8, 4, ShellConfig::default(), 1_000, 8 * 1024 * 1024);
    tabs.new_tab_with_command(
        "/bin/sh",
        &["-c".into(), "sleep 2".into()],
        None,
        Some("kitty-animation-deadline"),
        None,
        HashMap::new(),
    )
    .unwrap();
    app.tabs = Some(tabs);
    app.active_terminal_mut().unwrap().process(
        b"\x1b_Ga=T,f=32,s=1,v=1,i=73,c=1,r=1,C=1;AQIDBA==\x1b\\\x1b_Ga=f,f=32,s=1,v=1,i=73,z=10;BQYHCA==\x1b\\\x1b_Ga=a,i=73,r=1,z=10,s=3\x1b\\",
    );
    let deadline = app.next_graphics_animation_deadline().unwrap();
    assert_eq!(
        app.active_terminal().unwrap().presentation_images()[0]
            .image
            .data
            .as_ref(),
        &[1, 2, 3, 4]
    );
    app.redraw_scheduler.begin_frame();
    assert!(!app.redraw_scheduler.has_pending());

    app.poll_idle_sources(deadline);

    assert!(app.redraw_scheduler.has_pending());
    assert_eq!(
        app.active_terminal().unwrap().presentation_images()[0]
            .image
            .data
            .as_ref(),
        &[5, 6, 7, 8]
    );
}

#[cfg(unix)]
#[test]
fn synchronized_output_timeout_is_terminal_local_even_when_its_pane_is_unfocused() {
    if !std::path::Path::new("/bin/sh").exists() {
        return;
    }
    let mut app = App::new_headless(Config::default()).unwrap();
    let shell = ShellConfig {
        program: Some("/bin/sh".into()),
        args: vec!["-c".into(), "sleep 2".into()],
        env: Default::default(),
    };
    let mut panes = PaneManager::new(
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
        .get_pane_mut(right)
        .unwrap()
        .terminal
        .process(b"\x1b[?2026hhidden-update");
    let deadline = panes
        .get_pane(right)
        .unwrap()
        .terminal
        .synchronized_output_deadline()
        .unwrap();
    panes.focus(PaneId(0));

    let mut tabs = TabManager::new(80, 24, ShellConfig::default(), 1_000, 8 * 1024 * 1024);
    tabs.new_tab_with_command(
        "/bin/sh",
        &["-c".into(), "sleep 2".into()],
        None,
        Some("pane-synchronized-output-timeout"),
        None,
        HashMap::new(),
    )
    .unwrap();
    tabs.active_tab_mut().unwrap().pane_manager = Some(panes);
    app.tabs = Some(tabs);

    assert!(!app.active_terminal().unwrap().is_synchronized_output());
    assert_eq!(app.next_synchronized_output_deadline(), Some(deadline));

    assert_eq!(app.expire_synchronized_outputs(deadline), 1);

    let panes = app
        .tabs
        .as_ref()
        .unwrap()
        .active_tab()
        .unwrap()
        .pane_manager
        .as_ref()
        .unwrap();
    assert!(
        !panes
            .get_pane(right)
            .unwrap()
            .terminal
            .is_synchronized_output()
    );
}

#[test]
fn terminal_encoder_runs_after_application_and_plugin_shortcuts() {
    let source = include_str!("event_loop.rs");
    let application_shortcuts = source.find("shortcut_registry").unwrap();
    let plugin_shortcuts = source.find("keybinding_manager").unwrap();
    let terminal_encoder = source.find("key_to_bytes_for_terminal").unwrap();
    assert!(application_shortcuts < terminal_encoder);
    assert!(plugin_shortcuts < terminal_encoder);
}

#[cfg(unix)]
#[test]
fn keyboard_mode_snapshot_follows_the_focused_pane() {
    if !std::path::Path::new("/bin/sh").exists() {
        return;
    }
    let mut app = App::new_headless(Config::default()).unwrap();
    let shell = ShellConfig {
        program: Some("/bin/sh".into()),
        args: vec!["-c".into(), "sleep 2".into()],
        env: Default::default(),
    };
    let mut panes = PaneManager::new(
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
    panes.get_pane_mut(PaneId(0)).unwrap().terminal.set_mode(
        termeric_core::term::TerminalMode::CURSOR_KEYS_APPLICATION,
        true,
    );
    panes.get_pane_mut(PaneId(0)).unwrap().terminal.process(
        b"\x1b[=1u\x1b[?1060h\x1b[>0;5m\x1b[>0;6f\x1b[>1;3m\x1b[>2;3m\x1b[>3;3m\x1b[>4;1m\x1b[>6;3m\x1b[>7;3m",
    );
    panes
        .get_pane_mut(right)
        .unwrap()
        .terminal
        .set_mode(termeric_core::term::TerminalMode::KEYPAD_APPLICATION, true);
    panes.get_pane_mut(right).unwrap().terminal.process(
        b"\x1b[=2u\x1b[?1061h\x1b[>0;9m\x1b[>0;10f\x1b[>1;4m\x1b[>1;1f\x1b[>2;4m\x1b[>2;1f\x1b[>3;4m\x1b[>3;1f\x1b[>4;2m\x1b[>4;1f\x1b[>6;4m\x1b[>6;1f\x1b[>7;4m\x1b[>7;1f",
    );

    let mut tabs = TabManager::new(80, 24, ShellConfig::default(), 1_000, 8 * 1024 * 1024);
    tabs.new_tab_with_command(
        "/bin/sh",
        &["-c".into(), "sleep 2".into()],
        None,
        Some("keyboard-mode"),
        None,
        HashMap::new(),
    )
    .unwrap();
    tabs.active_tab_mut().unwrap().pane_manager = Some(panes);
    app.tabs = Some(tabs);

    app.tabs
        .as_mut()
        .unwrap()
        .active_tab_mut()
        .unwrap()
        .pane_manager
        .as_mut()
        .unwrap()
        .focus(PaneId(0));
    assert!(
        app.active_keyboard_mode()
            .contains(termeric_core::term::TerminalMode::CURSOR_KEYS_APPLICATION)
    );
    assert!(
        !app.active_keyboard_mode()
            .contains(termeric_core::term::TerminalMode::KEYPAD_APPLICATION)
    );
    assert_eq!(
        app.active_kitty_keyboard_flags(),
        termeric_core::term::KittyKeyboardFlags::DISAMBIGUATE_ESCAPE_CODES
    );
    assert_eq!(
        app.active_terminal()
            .unwrap()
            .xterm_modify_other_keys()
            .level(),
        1
    );
    assert_eq!(
        app.active_terminal().unwrap().xterm_keyboard_type(),
        termeric_core::term::XtermKeyboardType::Legacy
    );
    assert_eq!(
        app.active_terminal().unwrap().xterm_keyboard_resource(),
        termeric_core::term::XtermKeyboardResource::new(5, 6)
    );
    assert_eq!(
        app.active_terminal().unwrap().xterm_cursor_keys().mode(),
        termeric_core::term::XtermKeyMode::Private
    );
    assert_eq!(
        app.active_terminal().unwrap().xterm_function_keys().mode(),
        termeric_core::term::XtermKeyMode::Private
    );
    assert_eq!(
        app.active_terminal().unwrap().xterm_keypad_keys().mode(),
        termeric_core::term::XtermKeyMode::Private
    );
    assert_eq!(
        app.active_terminal().unwrap().xterm_modifier_keys().mode(),
        termeric_core::term::XtermKeyMode::Private
    );
    assert_eq!(
        app.active_terminal().unwrap().xterm_special_keys().mode(),
        termeric_core::term::XtermKeyMode::Private
    );
    assert_eq!(
        app.active_terminal()
            .unwrap()
            .xterm_modify_other_keys()
            .format(),
        termeric_core::term::XtermKeyFormat::FunctionKey27
    );

    app.tabs
        .as_mut()
        .unwrap()
        .active_tab_mut()
        .unwrap()
        .pane_manager
        .as_mut()
        .unwrap()
        .focus(right);
    assert!(
        app.active_keyboard_mode()
            .contains(termeric_core::term::TerminalMode::KEYPAD_APPLICATION)
    );
    assert!(
        !app.active_keyboard_mode()
            .contains(termeric_core::term::TerminalMode::CURSOR_KEYS_APPLICATION)
    );
    assert_eq!(
        app.active_kitty_keyboard_flags(),
        termeric_core::term::KittyKeyboardFlags::REPORT_EVENT_TYPES
    );
    assert_eq!(
        app.active_terminal()
            .unwrap()
            .xterm_modify_other_keys()
            .level(),
        2
    );
    assert_eq!(
        app.active_terminal().unwrap().xterm_keyboard_type(),
        termeric_core::term::XtermKeyboardType::Vt220
    );
    assert_eq!(
        app.active_terminal().unwrap().xterm_keyboard_resource(),
        termeric_core::term::XtermKeyboardResource::new(9, 10)
    );
    assert_eq!(
        app.active_terminal().unwrap().xterm_cursor_keys().mode(),
        termeric_core::term::XtermKeyMode::Extended
    );
    assert_eq!(
        app.active_terminal().unwrap().xterm_cursor_keys().format(),
        termeric_core::term::XtermKeyFormat::CsiU
    );
    assert_eq!(
        app.active_terminal().unwrap().xterm_function_keys().mode(),
        termeric_core::term::XtermKeyMode::Extended
    );
    assert_eq!(
        app.active_terminal()
            .unwrap()
            .xterm_function_keys()
            .format(),
        termeric_core::term::XtermKeyFormat::CsiU
    );
    assert_eq!(
        app.active_terminal().unwrap().xterm_keypad_keys().mode(),
        termeric_core::term::XtermKeyMode::Extended
    );
    assert_eq!(
        app.active_terminal().unwrap().xterm_keypad_keys().format(),
        termeric_core::term::XtermKeyFormat::CsiU
    );
    assert_eq!(
        app.active_terminal().unwrap().xterm_modifier_keys().mode(),
        termeric_core::term::XtermKeyMode::Extended
    );
    assert_eq!(
        app.active_terminal()
            .unwrap()
            .xterm_modifier_keys()
            .format(),
        termeric_core::term::XtermKeyFormat::CsiU
    );
    assert_eq!(
        app.active_terminal().unwrap().xterm_special_keys().mode(),
        termeric_core::term::XtermKeyMode::Extended
    );
    assert_eq!(
        app.active_terminal().unwrap().xterm_special_keys().format(),
        termeric_core::term::XtermKeyFormat::CsiU
    );
    assert_eq!(
        app.active_terminal()
            .unwrap()
            .xterm_modify_other_keys()
            .format(),
        termeric_core::term::XtermKeyFormat::CsiU
    );
}

#[cfg(unix)]
#[test]
fn copy_mode_is_owned_by_one_focused_pane_and_disappears_after_focus_changes() {
    if !std::path::Path::new("/bin/sh").exists() {
        return;
    }
    let mut app = App::new_headless(Config::default()).unwrap();
    let shell = ShellConfig {
        program: Some("/bin/sh".into()),
        args: vec!["-c".into(), "sleep 2".into()],
        env: Default::default(),
    };
    let mut panes = PaneManager::new(
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
    let mut tabs = TabManager::new(80, 24, ShellConfig::default(), 1_000, 8 * 1024 * 1024);
    tabs.new_tab_with_command(
        "/bin/sh",
        &["-c".into(), "sleep 2".into()],
        None,
        Some("copy-mode-owner"),
        None,
        HashMap::new(),
    )
    .unwrap();
    tabs.active_tab_mut().unwrap().pane_manager = Some(panes);
    app.tabs = Some(tabs);

    app.tabs
        .as_mut()
        .unwrap()
        .active_tab_mut()
        .unwrap()
        .pane_manager
        .as_mut()
        .unwrap()
        .focus(PaneId(0));
    app.enter_copy_mode();
    assert!(app.copy_mode_render_info().is_some());

    app.tabs
        .as_mut()
        .unwrap()
        .active_tab_mut()
        .unwrap()
        .pane_manager
        .as_mut()
        .unwrap()
        .focus(right);
    assert!(app.copy_mode_render_info().is_none());
    assert!(
        app.tabs
            .as_ref()
            .unwrap()
            .active_tab()
            .unwrap()
            .focused_terminal()
            .unwrap()
            .selection()
            .is_none()
    );
}

#[cfg(unix)]
#[test]
fn mouse_focus_and_bracketed_paste_reach_the_focused_pane_in_order() {
    if !std::path::Path::new("/bin/sh").exists() {
        return;
    }
    let script = concat!(
        "stty raw -echo; ",
        "printf '\rREADY\r\n'; ",
        "bytes=$(dd bs=1 count=38 2>/dev/null | od -An -tx1); ",
        "printf '\rprotocol=%s\r\n' \"$bytes\"; ",
        "sleep 0.1"
    );
    let shell = ShellConfig {
        program: Some("/bin/sh".into()),
        args: vec!["-c".into(), script.into()],
        env: Default::default(),
    };
    let mut panes = PaneManager::new(
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
    let receiving_pane = panes.split(SplitDirection::Vertical).unwrap();
    panes.focus(receiving_pane);
    panes
        .get_pane_mut(receiving_pane)
        .unwrap()
        .terminal
        .set_mode(
            termeric_core::term::TerminalMode::FOCUS_EVENTS
                | termeric_core::term::TerminalMode::BRACKETED_PASTE
                | termeric_core::term::TerminalMode::MOUSE_TRACKING
                | termeric_core::term::TerminalMode::MOUSE_SGR,
            true,
        );

    let mut app = App::new_headless(Config::default()).unwrap();
    let mut tabs = TabManager::new(80, 24, ShellConfig::default(), 1_000, 8 * 1024 * 1024);
    tabs.new_tab_with_command(
        "/bin/sh",
        &["-c".into(), "sleep 3".into()],
        None,
        Some("protocol-routing"),
        None,
        HashMap::new(),
    )
    .unwrap();
    tabs.active_tab_mut().unwrap().pane_manager = Some(panes);
    app.tabs = Some(tabs);

    let ready_deadline = Instant::now() + Duration::from_secs(3);
    let mut receiver_ready = false;
    while Instant::now() < ready_deadline {
        app.tabs.as_mut().unwrap().poll_events();
        let ready = app
            .tabs
            .as_ref()
            .unwrap()
            .active_tab()
            .unwrap()
            .pane_manager
            .as_ref()
            .unwrap()
            .get_pane(receiving_pane)
            .unwrap()
            .terminal
            .grid()
            .export_scrollback_text()
            .join("\n")
            .contains("READY");
        if ready {
            receiver_ready = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    assert!(
        receiver_ready,
        "focused pane PTY byte receiver did not become ready"
    );

    // Duplicate native notifications must not produce duplicate CSI I/O reports.
    app.handle_window_focus_change(false);
    app.handle_window_focus_change(false);
    app.handle_window_focus_change(true);
    app.handle_window_focus_change(true);
    app.write_paste_text("P\n");

    let mode = app.active_keyboard_mode();
    for kind in [
        crate::input::MouseReportKind::Press(crate::input::MouseReportButton::Left),
        crate::input::MouseReportKind::Release(crate::input::MouseReportButton::Left),
    ] {
        let report = crate::input::encode_mouse_report(
            mode,
            kind,
            2,
            3,
            winit::keyboard::ModifiersState::empty(),
        )
        .unwrap();
        assert!(app.write_terminal_protocol_input(&report));
    }

    let expected = concat!(
        "1b 5b 4f 1b 5b 49 ",
        "1b 5b 32 30 30 7e 50 0a 1b 5b 32 30 31 7e ",
        "1b 5b 3c 30 3b 33 3b 34 4d ",
        "1b 5b 3c 30 3b 33 3b 34 6d"
    );
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut output = String::new();
    while Instant::now() < deadline {
        app.tabs.as_mut().unwrap().poll_events();
        output = app
            .tabs
            .as_ref()
            .unwrap()
            .active_tab()
            .unwrap()
            .pane_manager
            .as_ref()
            .unwrap()
            .get_pane(receiving_pane)
            .unwrap()
            .terminal
            .grid()
            .export_scrollback_text()
            .join("\n");
        if output
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
            .contains(expected)
        {
            break;
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    assert!(
        output
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
            .contains(expected),
        "focused pane did not receive the ordered protocol stream: {output}"
    );
}

#[cfg(target_os = "linux")]
#[test]
fn real_bash_readline_accepts_normal_and_application_home_from_the_encoder() {
    if !std::path::Path::new("/bin/bash").exists() {
        return;
    }
    use winit::keyboard::{Key, KeyCode, ModifiersState, NamedKey, PhysicalKey};

    let mut environment = HashMap::new();
    environment.insert("TERM".into(), "xterm-256color".into());
    environment.insert("PS1".into(), "termeric-readline> ".into());
    let mut tab = Tab::new_with_command(
        0,
        80,
        24,
        "/bin/bash",
        &["--noprofile".into(), "--norc".into(), "-i".into()],
        None,
        Some("readline"),
        1_000,
        8 * 1024 * 1024,
        None,
        environment,
    )
    .unwrap();
    let home = |mode| {
        crate::input::encode_legacy_xterm_test(
            &Key::Named(NamedKey::Home),
            PhysicalKey::Code(KeyCode::Home),
            None,
            ModifiersState::empty(),
            false,
            mode,
        )
        .unwrap()
    };

    let prompt_deadline = Instant::now() + Duration::from_secs(2);
    while Instant::now() < prompt_deadline
        && !tab
            .focused_terminal()
            .unwrap()
            .rows_text(0, 23)
            .contains("termeric-readline>")
    {
        tab.poll_events();
        std::thread::sleep(Duration::from_millis(2));
    }

    tab.write(b"printf 'NORMAL_TAIL\\n'").unwrap();
    tab.write(&home(termeric_core::term::TerminalMode::default()))
        .unwrap();
    tab.write(b"printf 'NORMAL_HEAD\\n'; ").unwrap();
    tab.write(b"\r").unwrap();

    tab.write(b"printf 'APP_TAIL\\n'").unwrap();
    tab.write(&home(
        termeric_core::term::TerminalMode::CURSOR_KEYS_APPLICATION,
    ))
    .unwrap();
    tab.write(b"printf 'APP_HEAD\\n'; ").unwrap();
    tab.write(b"\r").unwrap();

    let output_deadline = Instant::now() + Duration::from_secs(3);
    let mut output = String::new();
    while Instant::now() < output_deadline {
        tab.poll_events();
        output = tab.focused_terminal().unwrap().rows_text(0, 23);
        if ["NORMAL_HEAD", "NORMAL_TAIL", "APP_HEAD", "APP_TAIL"]
            .iter()
            .all(|marker| output.matches(marker).count() >= 2)
        {
            break;
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    for marker in ["NORMAL_HEAD", "NORMAL_TAIL", "APP_HEAD", "APP_TAIL"] {
        assert!(
            output.matches(marker).count() >= 2,
            "readline omitted executed output for {marker}: {output}"
        );
    }
}

#[cfg(target_os = "linux")]
#[test]
fn split_adopts_one_owner_and_dissolve_preserves_output_without_orphans() {
    if !std::path::Path::new("/bin/sh").exists() {
        return;
    }
    let mut tab = Tab::new_with_command(
        0,
        80,
        24,
        "/bin/sh",
        &[
            "-c".into(),
            "printf 'before\\n'; sleep 0.05; printf 'after\\n'; sleep 5".into(),
        ],
        None,
        Some("owner"),
        1_000,
        8 * 1024 * 1024,
        None,
        HashMap::new(),
    )
    .unwrap();
    let original_pid = tab.single_child_id().unwrap();
    let original_cwd = std::fs::read_link(format!("/proc/{original_pid}/cwd")).unwrap();

    let before_deadline = Instant::now() + Duration::from_secs(2);
    while Instant::now() < before_deadline
        && !tab
            .focused_terminal()
            .unwrap()
            .rows_text(0, 23)
            .contains("before")
    {
        tab.poll_events();
        std::thread::sleep(Duration::from_millis(2));
    }

    let shell = ShellConfig {
        program: Some("/bin/sh".into()),
        args: vec!["-c".into(), "sleep 5".into()],
        env: Default::default(),
    };
    for cycle in 0..3 {
        let owner = tab.take_terminal_owner().unwrap();
        assert!(!tab.has_single_terminal_owner());
        let mut panes = PaneManager::from_existing(
            owner,
            shell.clone(),
            1_000,
            8 * 1024 * 1024,
            800.0,
            480.0,
            10.0,
            20.0,
        );
        assert_eq!(panes.spawned_shell_count(), 0);
        assert_eq!(
            panes.get_pane(PaneId(0)).unwrap().pty.child_id(),
            Some(original_pid)
        );

        let added = panes.split(SplitDirection::Vertical).unwrap();
        assert_eq!(panes.spawned_shell_count(), 1);
        let added_pid = panes.get_pane(added).unwrap().pty.child_id().unwrap();
        assert_eq!(
            std::fs::read_link(format!("/proc/{added_pid}/cwd")).unwrap(),
            original_cwd
        );

        if cycle == 0 {
            let after_deadline = Instant::now() + Duration::from_secs(2);
            while Instant::now() < after_deadline
                && !panes
                    .get_pane(PaneId(0))
                    .unwrap()
                    .terminal
                    .rows_text(0, 23)
                    .contains("after")
            {
                panes.poll_events(|_, _| {});
                std::thread::sleep(Duration::from_millis(2));
            }
        }

        assert!(panes.close_focused());
        assert_eq!(panes.pane_count(), 1);
        let remaining = panes.take_remaining_pane().unwrap();
        assert_eq!(remaining.pty.child_id(), Some(original_pid));
        tab.restore_terminal_owner(remaining);
        assert!(tab.has_single_terminal_owner());
        assert_eq!(tab.single_child_id(), Some(original_pid));
        assert!(
            tab.focused_terminal()
                .unwrap()
                .rows_text(0, 23)
                .contains("before")
        );

        let exit_deadline = Instant::now() + Duration::from_secs(1);
        while std::path::Path::new(&format!("/proc/{added_pid}")).exists()
            && Instant::now() < exit_deadline
        {
            std::thread::sleep(Duration::from_millis(2));
        }
        assert!(!std::path::Path::new(&format!("/proc/{added_pid}")).exists());
    }

    assert!(
        tab.focused_terminal()
            .unwrap()
            .rows_text(0, 23)
            .contains("after")
    );
    drop(tab);
    let exit_deadline = Instant::now() + Duration::from_secs(1);
    while std::path::Path::new(&format!("/proc/{original_pid}")).exists()
        && Instant::now() < exit_deadline
    {
        std::thread::sleep(Duration::from_millis(2));
    }
    assert!(!std::path::Path::new(&format!("/proc/{original_pid}")).exists());
}

#[cfg(target_os = "linux")]
#[test]
fn pane_identity_survives_collapse_and_the_next_split() {
    if !std::path::Path::new("/bin/sh").exists() {
        return;
    }
    let shell = ShellConfig {
        program: Some("/bin/sh".into()),
        args: vec!["-c".into(), "sleep 2".into()],
        env: Default::default(),
    };
    let mut tab = Tab::new_with_command(
        0,
        80,
        24,
        "/bin/sh",
        &["-c".into(), "sleep 2".into()],
        None,
        Some("stable-pane"),
        1_000,
        8 * 1024 * 1024,
        None,
        HashMap::new(),
    )
    .unwrap();
    let owner = tab.take_terminal_owner().unwrap();
    let mut panes = PaneManager::from_existing(
        owner,
        shell.clone(),
        1_000,
        8 * 1024 * 1024,
        800.0,
        480.0,
        10.0,
        20.0,
    );
    assert_eq!(panes.split(SplitDirection::Vertical).unwrap(), PaneId(1));
    assert!(panes.close(PaneId(0)));
    let remaining = panes.take_remaining_pane().unwrap();
    tab.restore_terminal_owner(remaining);
    assert_eq!(tab.single_pane_id(), PaneId(1));
    let snapshot = tab.capture_workspace_tab(0, false).unwrap();
    assert_eq!(snapshot.primary_pane, 1);
    assert_eq!(snapshot.focused_pane, 1);
    assert_eq!(snapshot.panes[0].id, 1);
    let mut restored = TabManager::new(80, 24, shell.clone(), 1_000, 8 * 1024 * 1024);
    restored
        .new_shell_tab_from_workspace(&snapshot, &HashMap::new())
        .unwrap();
    assert_eq!(restored.active_tab().unwrap().single_pane_id(), PaneId(1));

    let owner = tab.take_terminal_owner().unwrap();
    let mut panes = PaneManager::from_existing(
        owner,
        shell,
        1_000,
        8 * 1024 * 1024,
        800.0,
        480.0,
        10.0,
        20.0,
    );
    assert!(panes.get_pane(PaneId(1)).is_some());
    assert_eq!(panes.split(SplitDirection::Horizontal).unwrap(), PaneId(2));
}

#[cfg(unix)]
#[test]
fn pane_read_only_blocks_user_routes_but_not_protocol_replies_and_follows_ownership() {
    if !std::path::Path::new("/bin/sh").exists() {
        return;
    }
    let shell = ShellConfig {
        program: Some("/bin/sh".into()),
        args: vec!["-c".into(), "sleep 2".into()],
        env: Default::default(),
    };
    let mut tab = Tab::new_with_command(
        0,
        80,
        24,
        "/bin/sh",
        &shell.args,
        None,
        Some("read-only-pane"),
        1_000,
        8 * 1024 * 1024,
        None,
        HashMap::new(),
    )
    .unwrap();
    assert!(tab.toggle_focused_pane_read_only());
    assert_eq!(
        tab.write(b"blocked").unwrap_err().kind(),
        std::io::ErrorKind::PermissionDenied
    );
    assert!(tab.write_protocol_reply(b"").is_ok());

    let owner = tab.take_terminal_owner().unwrap();
    assert!(owner.read_only);
    let primary = owner.pane_id;
    let mut panes = PaneManager::from_existing(
        owner,
        shell,
        1_000,
        8 * 1024 * 1024,
        800.0,
        480.0,
        10.0,
        20.0,
    );
    assert!(panes.get_pane(primary).unwrap().read_only);
    assert_eq!(
        panes.write_to(primary, b"blocked").unwrap_err().kind(),
        std::io::ErrorKind::PermissionDenied
    );
    let writable = panes.split(SplitDirection::Vertical).unwrap();
    assert!(!panes.get_pane(writable).unwrap().read_only);
    assert!(panes.write_to(writable, b"").is_ok());
    assert!(panes.close(writable));

    tab.restore_terminal_owner(panes.take_remaining_pane().unwrap());
    assert!(tab.focused_pane_read_only());
    assert!(!tab.toggle_focused_pane_read_only());
    assert!(tab.write(b"").is_ok());
}

#[cfg(unix)]
#[test]
fn read_only_focused_pane_rejects_keyboard_and_running_gui_control_input() {
    if !std::path::Path::new("/bin/sh").exists() {
        return;
    }
    let mut config = Config::default();
    config.shell.program = Some("/bin/sh".into());
    config.shell.args = vec!["-c".into(), "sleep 2".into()];
    config.security.allow_gui_control_input = true;
    let mut app = App::new_headless(config).unwrap();
    app.allow_auto_restore = false;
    app.init_tabs().unwrap();
    app.toggle_focused_pane_read_only();

    let tab = app.tabs.as_ref().unwrap().active_tab().unwrap();
    let tab_id = tab.id as u64;
    let pane_id = tab.single_pane_id().0;
    assert!(tab.focused_pane_read_only());
    assert!(app.tabs.as_ref().unwrap().tab_info()[0].read_only);
    assert!(!app.write_terminal_protocol_input(b"blocked"));

    app.input_line_buffer = "stale-shell-state".into();
    app.git_completions = vec!["stale-branch".into()];
    app.git_completion_index = Some(0);
    app.git_completions_visible = true;
    app.write_to_pty(b"more");
    assert!(app.input_line_buffer.is_empty());
    assert!(app.git_completions.is_empty());
    assert_eq!(app.git_completion_index, None);
    assert!(!app.git_completions_visible);

    app.handle_ime_event(winit::event::Ime::Preedit("blocked".into(), None));
    assert!(app.ime.preedit().is_none());
    app.handle_ime_event(winit::event::Ime::Commit("blocked".into()));
    assert!(app.input_line_buffer.is_empty());

    let dropped_file = tempfile::NamedTempFile::new().unwrap();
    let drop_error = app.dropped_path_payload(dropped_file.path()).unwrap_err();
    assert!(drop_error.contains("read-only"), "{drop_error}");

    app.tabs
        .as_mut()
        .unwrap()
        .active_tab_mut()
        .unwrap()
        .tab_type = crate::tabs::TabType::Plugin {
        plugin_id: "hermetic-read-only-fixture".into(),
    };
    assert!(!app.write_paste_text("rm -rf /\n"));
    assert!(!app.guardrail.is_awaiting_current());
    let error = app
        .send_control_text(tab_id, pane_id, "rm -rf /", false, true)
        .unwrap_err();
    assert!(error.contains("read-only"), "{error}");
    let error = app
        .send_plugin_text(tab_id, pane_id, "cargo test", false, false)
        .unwrap_err();
    assert!(error.contains("read-only"), "{error}");
    assert!(!app.guardrail.is_awaiting_current());
}

#[cfg(target_os = "linux")]
#[test]
fn split_session_capture_follows_focus_for_all_persisted_terminal_fields() {
    if !std::path::Path::new("/bin/sh").exists() {
        return;
    }
    let workspace = tempfile::tempdir().unwrap();
    let left_dir = workspace.path().join("left");
    let right_dir = workspace.path().join("right");
    std::fs::create_dir_all(&left_dir).unwrap();
    std::fs::create_dir_all(&right_dir).unwrap();

    let mut tab = Tab::new_with_command(
        0,
        80,
        8,
        "/bin/sh",
        &[],
        None,
        Some("session-source"),
        1_000,
        8 * 1024 * 1024,
        None,
        HashMap::new(),
    )
    .unwrap();
    let owner = tab.take_terminal_owner().unwrap();
    let shell = ShellConfig {
        program: Some("/bin/sh".into()),
        args: Vec::new(),
        env: Default::default(),
    };
    let mut panes = PaneManager::from_existing(
        owner,
        shell,
        1_000,
        8 * 1024 * 1024,
        800.0,
        160.0,
        10.0,
        20.0,
    );
    let right_id = panes.split(SplitDirection::Vertical).unwrap();

    for (pane_id, directory) in [(PaneId(0), &left_dir), (right_id, &right_dir)] {
        panes
            .get_pane_mut(pane_id)
            .unwrap()
            .write(format!("cd '{}'\n", directory.display()).as_bytes())
            .unwrap();
    }
    let cwd_deadline = Instant::now() + Duration::from_secs(2);
    loop {
        panes.poll_events(|_, _| {});
        let left_cwd = panes
            .get_pane(PaneId(0))
            .unwrap()
            .pty
            .child_id()
            .and_then(crate::tabs::get_process_cwd);
        let right_cwd = panes
            .get_pane(right_id)
            .unwrap()
            .pty
            .child_id()
            .and_then(crate::tabs::get_process_cwd);
        if left_cwd.as_deref() == left_dir.to_str() && right_cwd.as_deref() == right_dir.to_str() {
            break;
        }
        assert!(Instant::now() < cwd_deadline, "pane cwd changes timed out");
        std::thread::sleep(Duration::from_millis(2));
    }

    for (pane_id, marker, command) in [
        (PaneId(0), "left-history", "left-command"),
        (right_id, "right-history", "right-command"),
    ] {
        let pane = panes.get_pane_mut(pane_id).unwrap();
        for index in 0..20 {
            pane.terminal
                .process(format!("{marker}-{index}\r\n").as_bytes());
        }
        pane.last_command = Some(command.into());
    }
    tab.pane_manager = Some(panes);

    let assert_capture = |capture: crate::tabs::SessionCapture,
                          directory: &std::path::Path,
                          marker: &str,
                          other_marker: &str,
                          command: &str| {
        assert_eq!(
            capture.scope,
            crate::session::SessionCaptureScope::FocusedPane
        );
        assert_eq!(capture.working_directory.as_deref(), directory.to_str());
        assert_eq!(capture.last_command.as_deref(), Some(command));
        let text = capture.scrollback.join("\n");
        assert!(text.contains(marker));
        assert!(!text.contains(other_marker));
    };

    tab.pane_manager.as_mut().unwrap().focus(PaneId(0));
    let left_capture = tab.capture_focused_session(true).unwrap();
    assert_capture(
        left_capture,
        &left_dir,
        "left-history",
        "right-history",
        "left-command",
    );

    tab.pane_manager.as_mut().unwrap().focus(right_id);
    let right_capture = tab.capture_focused_session(true).unwrap();
    assert_capture(
        right_capture,
        &right_dir,
        "right-history",
        "left-history",
        "right-command",
    );

    let right_capture = tab.capture_focused_session(true).unwrap();
    let mut persisted =
        crate::session::Session::new("right snapshot".into(), right_capture.working_directory);
    persisted.last_command = right_capture.last_command;
    persisted.capture_scope = right_capture.scope;
    let snapshot_dir = workspace.path().join("snapshot");
    crate::session::save::save_session_to_dir(&persisted, &snapshot_dir).unwrap();
    let restored =
        crate::session::restore::restore_session_from_path(&snapshot_dir.join("session.toml"))
            .unwrap();
    assert_eq!(restored.working_directory.as_deref(), right_dir.to_str());
    assert_eq!(restored.last_command.as_deref(), Some("right-command"));
    assert_eq!(
        restored.capture_scope,
        crate::session::SessionCaptureScope::FocusedPane
    );
}

#[cfg(unix)]
#[test]
fn real_pty_unicode_clusters_preserve_width_and_content() {
    if !std::path::Path::new("/bin/sh").exists() {
        return;
    }
    let expected = "界e\u{301}👩\u{200d}💻";
    let mut tab = Tab::new_with_command(
        0,
        20,
        3,
        "/bin/sh",
        &["-c".into(), format!("printf '%s' '{expected}'")],
        None,
        Some("unicode-pty"),
        100,
        1024 * 1024,
        None,
        HashMap::new(),
    )
    .unwrap();
    let deadline = Instant::now() + Duration::from_secs(2);
    while Instant::now() < deadline {
        tab.poll_events();
        if tab
            .focused_terminal()
            .unwrap()
            .rows_text(0, 0)
            .contains(expected)
        {
            break;
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    let terminal = tab.focused_terminal().unwrap();
    assert_eq!(terminal.rows_text(0, 0), expected);
    assert_eq!(terminal.grid().cursor().col, 5);
    assert!(terminal.grid().cell(0, 0).unwrap().is_wide_char());
    assert!(terminal.grid().cell(0, 1).unwrap().is_wide_char_spacer());
    assert_eq!(
        terminal.grid().cell(0, 2).unwrap().content.to_text(),
        "e\u{301}"
    );
    assert!(terminal.grid().cell(0, 3).unwrap().is_wide_char());
    assert!(terminal.grid().cell(0, 4).unwrap().is_wide_char_spacer());
}

#[cfg(unix)]
#[test]
fn real_pty_sgr_attributes_reach_the_terminal_grid_without_leaking() {
    if !std::path::Path::new("/bin/sh").exists() {
        return;
    }
    let command = "printf '\\033[1mA\\033[22mB\\033[6mC\\033[25mD\\033[8mE\\033[28;9mF\\033[29;4:3;58:2::12:34:56mG\\033[0mH'";
    let mut tab = Tab::new_with_command(
        0,
        20,
        3,
        "/bin/sh",
        &["-c".into(), command.into()],
        None,
        Some("sgr-pty"),
        100,
        1024 * 1024,
        None,
        HashMap::new(),
    )
    .unwrap();
    let deadline = Instant::now() + Duration::from_secs(2);
    while Instant::now() < deadline {
        tab.poll_events();
        if tab
            .focused_terminal()
            .unwrap()
            .rows_text(0, 0)
            .contains("ABCDEFGH")
        {
            break;
        }
        std::thread::sleep(Duration::from_millis(2));
    }

    let terminal = tab.focused_terminal().unwrap();
    assert_eq!(terminal.rows_text(0, 0), "ABCDEFGH");
    let grid = terminal.grid();
    assert!(
        grid.cell(0, 0)
            .unwrap()
            .attrs
            .contains(termeric_core::CellAttrs::BOLD)
    );
    assert!(grid.cell(0, 1).unwrap().attrs.is_empty());
    assert!(
        grid.cell(0, 2)
            .unwrap()
            .attrs
            .contains(termeric_core::CellAttrs::RAPID_BLINK)
    );
    assert!(grid.cell(0, 3).unwrap().attrs.is_empty());
    assert!(
        grid.cell(0, 4)
            .unwrap()
            .attrs
            .contains(termeric_core::CellAttrs::HIDDEN)
    );
    assert!(
        grid.cell(0, 5)
            .unwrap()
            .attrs
            .contains(termeric_core::CellAttrs::STRIKE)
    );
    let underlined = grid.cell(0, 6).unwrap();
    assert!(
        underlined
            .attrs
            .contains(termeric_core::CellAttrs::CURLY_UNDERLINE)
    );
    assert_eq!(
        underlined.underline_color,
        Some(termeric_core::Color::rgb(12, 34, 56))
    );
    assert_eq!(
        underlined.underline_color_spec,
        Some(termeric_core::ColorSpec::TrueColor(
            termeric_core::Color::rgb(12, 34, 56)
        ))
    );
    assert!(grid.cell(0, 7).unwrap().attrs.is_empty());
}

#[cfg(unix)]
#[test]
fn representative_git_color_output_maps_to_rendered_sgr_state() {
    if !std::path::Path::new("/etc/hostname").exists() {
        return;
    }
    let Ok(output) = std::process::Command::new("git")
        .args([
            "-c",
            "color.ui=always",
            "diff",
            "--no-index",
            "--",
            "/dev/null",
            "/etc/hostname",
        ])
        .output()
    else {
        return;
    };
    assert_eq!(output.status.code(), Some(1));

    let mut terminal = termeric_core::Terminal::new(120, 20);
    terminal.process(&output.stdout);
    let has_bold = (0..terminal.rows()).any(|row| {
        (0..terminal.cols()).any(|col| {
            terminal
                .grid()
                .cell(row, col)
                .is_some_and(|cell| cell.attrs.contains(termeric_core::CellAttrs::BOLD))
        })
    });
    assert!(has_bold);
    assert!(terminal.rows_text(0, 19).contains("diff --git"));
}

#[test]
fn headless_config_application_updates_plugins_harvest_and_palette() {
    let mut config = Config::default();
    config.harvest.enabled = true;
    config.harvest.max_history = 7;
    config.plugins.claude_code.enabled = false;
    config.plugins.codex.enabled = true;

    let mut app = App::new_headless(config).unwrap();
    assert_eq!(app.plugin_manager.count(), 1);
    assert!(app.harvest.is_some());
    app.command_palette.open();
    let enabled_count = app.command_palette.build_info().total_count;

    let mut disabled = app.config.clone();
    disabled.harvest.enabled = false;
    disabled.plugins.codex.enabled = false;
    app.apply_config(disabled).unwrap();
    assert_eq!(app.plugin_manager.count(), 0);
    assert!(app.harvest.is_none());
    app.command_palette.open();
    let disabled_count = app.command_palette.build_info().total_count;
    assert_eq!(enabled_count - disabled_count, 16);
}

#[test]
fn structured_plugin_completions_require_a_live_generation_and_plugin() {
    #[derive(Debug)]
    struct CompletionPlugin;

    impl crate::plugin::hooks::Plugin for CompletionPlugin {
        fn id(&self) -> &str {
            "completion-fixture"
        }

        fn name(&self) -> &str {
            "Completion Fixture"
        }
    }

    let mut app = App::new_headless(Config::default()).unwrap();
    let context = app.build_plugin_context();
    app.plugin_manager
        .register(Box::new(CompletionPlugin), &context)
        .unwrap();
    let completion =
        |generation, message: &str| crate::plugin::structured::StructuredHandlerCompletion {
            window_id: winit::window::WindowId::dummy(),
            plugin_generation: generation,
            plugin_id: "completion-fixture".into(),
            request_id: 7,
            continuation: None,
            result: Ok(vec![
                crate::plugin::structured::StructuredResponseAction::ShowMessage {
                    message: message.into(),
                },
            ]),
        };

    app.handle_structured_plugin_completed(completion(
        app.plugin_runtime_generation.wrapping_sub(1),
        "stale",
    ));
    assert!(app.toasts.iter().all(|toast| toast.message != "stale"));

    app.handle_structured_plugin_completed(completion(app.plugin_runtime_generation, "current"));
    assert!(app.toasts.iter().any(|toast| toast.message == "current"));

    app.plugin_manager.deregister("completion-fixture");
    app.handle_structured_plugin_completed(completion(app.plugin_runtime_generation, "unloaded"));
    assert!(app.toasts.iter().all(|toast| toast.message != "unloaded"));
}

#[test]
fn process_query_retains_only_independent_process_followups_and_results_revoke_authority() {
    let mut invocation = crate::plugin::structured::StructuredHandlerInvocation {
        plugin_id: "process-inspector".into(),
        program: "fixture".into(),
        args: Vec::new(),
        event_json: "{}".into(),
        allow_ui_actions: true,
        allow_workspace_query: true,
        allow_workspace_actions: true,
        allow_workspace_topology: true,
        allow_terminal_query: true,
        allow_pane_create: true,
        allow_tab_create: true,
        allow_pane_close: true,
        allow_tab_close: true,
        allow_pane_move: true,
        allow_terminal_input: true,
        allow_pane_layout: true,
        allow_window_close: true,
        allow_process_query: true,
        allow_process_actions: true,
        allow_process_terminal_text: true,
        allow_process_terminal_input: true,
        mode: crate::plugin::structured::StructuredHandlerMode::Persistent,
    };

    let mut process_query_result = invocation.clone();
    super::plugin_actions::reduce_structured_process_query_authority(&mut process_query_result);
    assert!(process_query_result.allow_ui_actions);
    assert!(process_query_result.allow_process_actions);
    assert!(process_query_result.allow_process_terminal_text);
    assert!(process_query_result.allow_process_terminal_input);
    assert!(!process_query_result.allow_workspace_query);
    assert!(!process_query_result.allow_workspace_actions);
    assert!(!process_query_result.allow_workspace_topology);
    assert!(!process_query_result.allow_terminal_query);
    assert!(!process_query_result.allow_pane_create);
    assert!(!process_query_result.allow_tab_create);
    assert!(!process_query_result.allow_pane_close);
    assert!(!process_query_result.allow_tab_close);
    assert!(!process_query_result.allow_pane_move);
    assert!(!process_query_result.allow_terminal_input);
    assert!(!process_query_result.allow_pane_layout);
    assert!(!process_query_result.allow_window_close);
    assert!(!process_query_result.allow_process_query);

    super::plugin_actions::clear_structured_host_authority(&mut invocation);

    assert!(invocation.allow_ui_actions);
    assert!(!invocation.allow_workspace_query);
    assert!(!invocation.allow_workspace_actions);
    assert!(!invocation.allow_workspace_topology);
    assert!(!invocation.allow_terminal_query);
    assert!(!invocation.allow_pane_create);
    assert!(!invocation.allow_tab_create);
    assert!(!invocation.allow_pane_close);
    assert!(!invocation.allow_tab_close);
    assert!(!invocation.allow_pane_move);
    assert!(!invocation.allow_terminal_input);
    assert!(!invocation.allow_pane_layout);
    assert!(!invocation.allow_window_close);
    assert!(!invocation.allow_process_query);
    assert!(!invocation.allow_process_actions);
    assert!(!invocation.allow_process_terminal_text);
    assert!(!invocation.allow_process_terminal_input);
}

#[cfg(unix)]
#[test]
fn plugin_window_close_result_is_scheduled_without_premature_teardown() {
    if !std::path::Path::new("/bin/sh").exists() {
        return;
    }
    let shell = ShellConfig {
        program: Some("/bin/sh".into()),
        args: vec!["-c".into(), "sleep 2".into()],
        env: Default::default(),
    };
    let mut tabs = TabManager::new(80, 24, shell, 1_000, 8 * 1024 * 1024);
    tabs.new_tab_with_command(
        "/bin/sh",
        &["-c".into(), "sleep 2".into()],
        None,
        Some("close fixture"),
        None,
        HashMap::new(),
    )
    .unwrap();
    let mut app = App::new_headless(Config::default()).unwrap();
    app.tabs = Some(tabs);

    let encoded = app
        .structured_workspace_action_event(
            "window-closer",
            crate::plugin::structured::StructuredResponseAction::CloseWindow {
                action_id: "close-current-window".into(),
            },
        )
        .unwrap();
    let envelope: serde_json::Value = serde_json::from_str(&encoded).unwrap();
    assert_eq!(envelope["plugin_id"], "window-closer");
    assert_eq!(envelope["event"]["type"], "workspace-action-result");
    assert_eq!(envelope["event"]["action"], "close-window");
    assert_eq!(envelope["event"]["ok"], true);
    assert_eq!(envelope["event"]["scheduled"], true);
    assert_eq!(envelope["event"]["tab_count"], 1);
    assert!(!app.should_exit);
    assert!(app.pending_plugin_window_close.is_none());

    let error = app
        .continue_structured_workspace_operation(
            "window-closer",
            crate::plugin::structured::StructuredResponseAction::CloseWindow {
                action_id: "submit-without-native-window".into(),
            },
            Some(crate::plugin::structured::StructuredHandlerInvocation {
                plugin_id: "window-closer".into(),
                program: "/bin/sh".into(),
                args: vec!["-c".into(), "exit 0".into()],
                event_json: r#"{"schema_version":1}"#.into(),
                allow_ui_actions: false,
                allow_workspace_query: false,
                allow_workspace_actions: false,
                allow_workspace_topology: false,
                allow_terminal_query: false,
                allow_pane_create: false,
                allow_tab_create: false,
                allow_pane_close: false,
                allow_tab_close: false,
                allow_pane_move: false,
                allow_terminal_input: false,
                allow_pane_layout: false,
                allow_window_close: true,
                allow_process_query: false,
                allow_process_actions: false,
                allow_process_terminal_text: false,
                allow_process_terminal_input: false,
                mode: crate::plugin::structured::StructuredHandlerMode::Persistent,
            }),
        )
        .unwrap_err();
    assert!(error.contains("native window"));
    assert!(!app.should_exit);
    assert!(app.pending_plugin_window_close.is_none());

    app.pending_plugin_window_close = Some(PendingPluginWindowClose {
        plugin_id: "window-closer".into(),
        plugin_generation: app.plugin_runtime_generation,
        result_request_id: 9,
    });
    let duplicate: serde_json::Value = serde_json::from_str(
        &app.structured_workspace_action_event(
            "window-closer",
            crate::plugin::structured::StructuredResponseAction::CloseWindow {
                action_id: "duplicate".into(),
            },
        )
        .unwrap(),
    )
    .unwrap();
    assert_eq!(duplicate["event"]["ok"], false);
    assert_eq!(duplicate["event"]["scheduled"], false);
    assert!(
        duplicate["event"]["error"]
            .as_str()
            .unwrap()
            .contains("pending plugin closure")
    );
    assert!(!app.should_exit);
}

#[test]
fn plugin_window_close_waits_for_only_the_exact_result_callback() {
    let mut app = App::new_headless(Config::default()).unwrap();
    let generation = app.plugin_runtime_generation.wrapping_sub(1);
    app.pending_plugin_window_close = Some(PendingPluginWindowClose {
        plugin_id: "window-closer".into(),
        plugin_generation: generation,
        result_request_id: 41,
    });
    let completion = |request_id, result| crate::plugin::structured::StructuredHandlerCompletion {
        window_id: winit::window::WindowId::dummy(),
        plugin_generation: generation,
        plugin_id: "window-closer".into(),
        request_id,
        continuation: None,
        result,
    };

    app.handle_structured_plugin_completed(completion(40, Ok(Vec::new())));
    assert!(!app.should_exit);
    assert!(app.pending_plugin_window_close.is_some());

    app.handle_structured_plugin_completed(completion(
        41,
        Ok(vec![
            crate::plugin::structured::StructuredResponseAction::ShowMessage {
                message: "must not render during teardown".into(),
            },
        ]),
    ));
    assert!(app.should_exit);
    assert!(app.pending_plugin_window_close.is_none());
    assert!(
        app.toasts
            .iter()
            .all(|toast| toast.message != "must not render during teardown")
    );
}

#[test]
fn plugin_window_close_still_finishes_after_final_handler_failure() {
    let mut app = App::new_headless(Config::default()).unwrap();
    app.pending_plugin_window_close = Some(PendingPluginWindowClose {
        plugin_id: "window-closer".into(),
        plugin_generation: app.plugin_runtime_generation,
        result_request_id: 73,
    });
    app.handle_structured_plugin_completed(
        crate::plugin::structured::StructuredHandlerCompletion {
            window_id: winit::window::WindowId::dummy(),
            plugin_generation: app.plugin_runtime_generation,
            plugin_id: "window-closer".into(),
            request_id: 73,
            continuation: None,
            result: Err("fixture callback failed".into()),
        },
    );
    assert!(app.should_exit);
    assert!(app.pending_plugin_window_close.is_none());
}

#[cfg(unix)]
#[test]
fn structured_workspace_query_is_current_window_bounded_and_control_safe() {
    if !std::path::Path::new("/bin/sh").exists() {
        return;
    }
    let shell = ShellConfig {
        program: Some("/bin/sh".into()),
        args: vec!["-c".into(), "sleep 2".into()],
        env: Default::default(),
    };
    let mut tabs = TabManager::new(80, 24, shell, 1_000, 8 * 1024 * 1024);
    tabs.new_tab_with_command(
        "/bin/sh",
        &["-c".into(), "sleep 2".into()],
        None,
        Some("unsafe\u{1b} title"),
        None,
        HashMap::new(),
    )
    .unwrap();
    let first_tab_id = tabs.active_tab().unwrap().id;
    let mut last_tab_id = first_tab_id;
    for index in 1..17 {
        last_tab_id = tabs
            .new_tab_with_command(
                "/bin/sh",
                &["-c".into(), "exit 0".into()],
                None,
                Some(&format!("tab-{index}-{}", "x".repeat(1024))),
                None,
                HashMap::new(),
            )
            .unwrap();
    }

    let mut app = App::new_headless(Config::default()).unwrap();
    app.tabs = Some(tabs);
    app.current_workspace_id = Some("workspace\u{7}fixture".into());
    let encoded = app
        .structured_workspace_query_event("query-fixture", "inventory-1")
        .unwrap();
    assert!(encoded.len() <= crate::plugin::structured::MAX_PERSISTENT_SOURCE_EVENT_BYTES);
    assert!(!encoded.contains("presented-secret"));
    let value: serde_json::Value = serde_json::from_str(&encoded).unwrap();
    assert_eq!(value["schema_version"], 1);
    assert_eq!(value["plugin_id"], "query-fixture");
    assert_eq!(value["event"]["type"], "workspace-query-result");
    assert_eq!(value["event"]["query_id"], "inventory-1");
    assert_eq!(value["event"]["snapshot_schema_version"], 1);
    assert_eq!(value["event"]["tab_count"], 17);
    assert_eq!(value["event"]["active_tab_index"], 16);
    assert!(value["event"]["window"].get("id").is_none());
    assert_eq!(value["event"]["window"]["workspace_id"], "workspacefixture");
    assert_eq!(
        value["event"]["window"]["tabs"].as_array().unwrap().len(),
        16
    );
    assert!(value["event"]["window"]["truncated"].as_bool().unwrap());
    assert_eq!(value["event"]["window"]["tabs"][0]["id"], first_tab_id);
    assert_eq!(value["event"]["window"]["tabs"][0]["title"], "unsafe title");
    assert!(
        value["event"]["window"]["tabs"][0]["truncated"]
            .as_bool()
            .unwrap()
    );
    assert_eq!(value["event"]["window"]["tabs"][0]["panes"][0]["cols"], 80);
    assert_eq!(value["event"]["window"]["tabs"][0]["panes"][0]["rows"], 24);

    #[derive(Debug)]
    struct TabSwitchObserver(std::sync::Arc<std::sync::atomic::AtomicUsize>);

    impl crate::plugin::hooks::Plugin for TabSwitchObserver {
        fn id(&self) -> &str {
            "workspace-action-observer"
        }

        fn name(&self) -> &str {
            "Workspace Action Observer"
        }

        fn on_hook(
            &mut self,
            hook: &crate::plugin::Hook,
            _ctx: &crate::plugin::PluginContext,
        ) -> crate::plugin::hooks::HookResponse {
            if matches!(hook, crate::plugin::Hook::TabSwitched { .. }) {
                self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            }
            crate::plugin::hooks::HookResponse::Ignored
        }
    }

    let observed_switches = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let context = app.build_plugin_context();
    app.plugin_manager
        .register(
            Box::new(TabSwitchObserver(std::sync::Arc::clone(&observed_switches))),
            &context,
        )
        .unwrap();

    let failed = app
        .structured_workspace_action_event(
            "workspace-navigator",
            crate::plugin::structured::StructuredResponseAction::FocusPane {
                action_id: "missing-pane".into(),
                tab_id: first_tab_id as u64,
                pane_id: u32::MAX,
            },
        )
        .unwrap();
    let failed: serde_json::Value = serde_json::from_str(&failed).unwrap();
    assert_eq!(failed["event"]["type"], "workspace-action-result");
    assert_eq!(failed["event"]["action_id"], "missing-pane");
    assert_eq!(failed["event"]["action"], "focus-pane");
    assert_eq!(failed["event"]["ok"], false);
    assert!(
        failed["event"]["error"]
            .as_str()
            .is_some_and(|error| { !error.is_empty() && !error.chars().any(char::is_control) })
    );
    assert_eq!(
        app.tabs.as_ref().unwrap().active_tab_id(),
        Some(last_tab_id)
    );

    let selected = app
        .structured_workspace_action_event(
            "workspace-navigator",
            crate::plugin::structured::StructuredResponseAction::SelectTab {
                action_id: "select-first".into(),
                tab_id: first_tab_id as u64,
            },
        )
        .unwrap();
    let selected: serde_json::Value = serde_json::from_str(&selected).unwrap();
    assert_eq!(selected["plugin_id"], "workspace-navigator");
    assert_eq!(selected["event"]["ok"], true);
    assert_eq!(selected["event"]["changed"], true);
    assert_eq!(
        app.tabs.as_ref().unwrap().active_tab_id(),
        Some(first_tab_id)
    );
    assert_eq!(
        observed_switches.load(std::sync::atomic::Ordering::SeqCst),
        0
    );

    // The ordinary authenticated control route still emits the lifecycle hook;
    // only plugin-origin navigation is suppressed to prevent action loops.
    assert_eq!(app.select_control_tab(last_tab_id as u64), Ok(true));
    assert_eq!(
        observed_switches.load(std::sync::atomic::Ordering::SeqCst),
        1
    );

    let pane_id = u32::try_from(
        value["event"]["window"]["tabs"][0]["panes"][0]["id"]
            .as_u64()
            .unwrap(),
    )
    .unwrap();
    let focused = app
        .structured_workspace_action_event(
            "workspace-navigator",
            crate::plugin::structured::StructuredResponseAction::FocusPane {
                action_id: "focus-first".into(),
                tab_id: first_tab_id as u64,
                pane_id,
            },
        )
        .unwrap();
    let focused: serde_json::Value = serde_json::from_str(&focused).unwrap();
    assert_eq!(focused["event"]["ok"], true);
    assert_eq!(focused["event"]["tab_changed"], true);
    assert_eq!(focused["event"]["pane_changed"], false);
    assert_eq!(
        observed_switches.load(std::sync::atomic::Ordering::SeqCst),
        1
    );
}

#[test]
fn headless_settings_apply_scrollback_memory_budget() {
    let mut app = App::new_headless(Config::default()).unwrap();
    app.apply_settings_action(SettingsAction::SetScrollbackMemory(64));
    assert_eq!(app.config.scrollback.max_memory_mib, 64);
    assert_eq!(app.config.scrollback.max_memory_bytes(), 64 * 1024 * 1024);
}

#[test]
fn headless_font_settings_preserve_order_and_apply_without_a_window() {
    let mut app = App::new_headless(Config::default()).unwrap();
    app.apply_settings_action(SettingsAction::SetFontFamily("Fira Mono".into()));
    app.apply_settings_action(SettingsAction::SetFontFallback {
        index: 0,
        family: Some("Noto Sans Mono".into()),
    });
    app.apply_settings_action(SettingsAction::SetFontFallback {
        index: 1,
        family: Some("Noto Color Emoji".into()),
    });
    app.apply_settings_action(SettingsAction::SetFontLigatures(false));
    app.apply_settings_action(SettingsAction::SetBoldIsBright(true));
    app.apply_settings_action(SettingsAction::SetLineHeight(1.35));

    assert_eq!(app.config.font.family, "Fira Mono");
    assert_eq!(
        app.config.font.fallback,
        ["Noto Sans Mono", "Noto Color Emoji"]
    );
    assert!(!app.config.font.ligatures);
    assert!(app.config.font.bold_is_bright);
    assert_eq!(app.config.font.line_height, 1.35);

    app.apply_settings_action(SettingsAction::SetFontFallback {
        index: 0,
        family: None,
    });
    assert_eq!(app.config.font.fallback, ["Noto Color Emoji"]);

    // Promoting an existing fallback to primary removes the duplicate.
    app.apply_settings_action(SettingsAction::SetFontFamily("Noto Color Emoji".into()));
    assert!(app.config.font.fallback.is_empty());
}

#[test]
fn headless_window_level_setting_updates_persistent_policy() {
    let mut app = App::new_headless(Config::default()).unwrap();
    app.apply_settings_action(SettingsAction::SetWindowLevel(
        crate::config::WindowLevelConfig::AlwaysOnBottom,
    ));
    assert_eq!(
        app.config.window.level,
        crate::config::WindowLevelConfig::AlwaysOnBottom
    );
}

#[test]
fn headless_tab_bar_position_setting_updates_persistent_policy() {
    let mut app = App::new_headless(Config::default()).unwrap();
    app.apply_settings_action(SettingsAction::SetTabBarPosition(
        crate::config::TabBarPosition::Left,
    ));
    assert_eq!(
        app.config.window.tab_bar_position,
        crate::config::TabBarPosition::Left
    );
}

#[test]
fn headless_background_image_settings_update_persistent_layout_policy() {
    let mut app = App::new_headless(Config::default()).unwrap();
    app.apply_settings_action(SettingsAction::SetBackgroundImageFit(
        crate::config::BackgroundImageFit::Contain,
    ));
    app.apply_settings_action(SettingsAction::SetBackgroundImageOpacity(0.45));
    assert_eq!(
        app.config.window.background_image.fit,
        crate::config::BackgroundImageFit::Contain
    );
    assert!((app.config.window.background_image.opacity - 0.45).abs() < f32::EPSILON);
}

#[test]
fn headless_accessibility_settings_apply_without_transition_frames() {
    let mut app = App::new_headless(Config::default()).unwrap();
    app.apply_settings_action(SettingsAction::SetReduceMotion(
        crate::config::AccessibilityPreference::On,
    ));
    app.apply_settings_action(SettingsAction::SetHighContrast(
        crate::config::AccessibilityPreference::On,
    ));
    assert_eq!(
        app.config.accessibility.reduce_motion,
        crate::config::AccessibilityPreference::On
    );
    assert_eq!(
        app.config.accessibility.high_contrast,
        crate::config::AccessibilityPreference::On
    );

    app.settings_panel.toggle();
    assert_eq!(app.settings_panel.animation_progress(), 1.0);
    app.settings_panel.close();
    assert!(!app.settings_panel.update_animation());
    assert!(!app.settings_panel.is_visible());
}

#[test]
fn headless_system_accessibility_preferences_respect_explicit_overrides() {
    let mut config = Config::default();
    config.accessibility.high_contrast = crate::config::AccessibilityPreference::Off;
    let mut app = App::new_headless(config).unwrap();

    app.set_system_accessibility_preferences(
        super::system_accessibility::SystemAccessibilityPreferences {
            reduce_motion: Some(true),
            high_contrast: Some(true),
        },
    );
    assert!(app.reduced_motion_enabled());
    assert!(!app.high_contrast_enabled());
    app.settings_panel.toggle();
    assert_eq!(app.settings_panel.animation_progress(), 1.0);

    app.apply_settings_action(SettingsAction::SetReduceMotion(
        crate::config::AccessibilityPreference::Off,
    ));
    assert!(!app.reduced_motion_enabled());
}

#[test]
fn background_image_completions_are_generation_checked_and_fail_visibly() {
    let mut app = App::new_headless(Config::default()).unwrap();
    app.background_image_generation = 7;
    app.background_image_loading = true;
    app.handle_background_image_loaded(6, PathBuf::from("stale.png"), Err("stale decode".into()));
    assert!(!app.background_image_loading);
    assert!(
        app.toasts
            .iter()
            .all(|toast| !toast.message.contains("stale"))
    );

    app.background_image_loading = true;
    app.handle_background_image_loaded(
        7,
        PathBuf::from("current.png"),
        Err("current decode".into()),
    );
    assert!(!app.background_image_loading);
    assert!(
        latest_toast(&app).is_some_and(|message| message.contains("current decode")),
        "the current failure must be visible while preserving the prior texture"
    );
}

#[cfg(unix)]
#[test]
fn config_reload_only_overrides_application_keyboard_type_when_profile_changes() {
    if !std::path::Path::new("/bin/sh").exists() {
        return;
    }
    let mut config = Config::default();
    config.keyboard.xterm_type = crate::config::XtermKeyboardTypeConfig::Legacy;
    let mut app = App::new_headless(config).unwrap();
    let mut tabs = TabManager::new(80, 24, ShellConfig::default(), 1_000, 8 * 1024 * 1024);
    tabs.set_xterm_keyboard_type(termeric_core::term::XtermKeyboardType::Legacy, true);
    tabs.new_tab_with_command(
        "/bin/sh",
        &["-c".into(), "sleep 1".into()],
        None,
        Some("config-keyboard-profile"),
        None,
        HashMap::new(),
    )
    .unwrap();
    app.tabs = Some(tabs);
    app.active_terminal_mut().unwrap().process(b"\x1b[?1061h");

    let mut unrelated = app.config.clone();
    unrelated.cursor.blink_rate += 1;
    app.apply_config(unrelated).unwrap();
    assert_eq!(
        app.active_terminal().unwrap().xterm_keyboard_type(),
        termeric_core::term::XtermKeyboardType::Vt220
    );

    let mut changed = app.config.clone();
    changed.keyboard.xterm_type = crate::config::XtermKeyboardTypeConfig::Normal;
    app.apply_config(changed).unwrap();
    let terminal = app.active_terminal().unwrap();
    assert_eq!(
        terminal.xterm_keyboard_type(),
        termeric_core::term::XtermKeyboardType::Normal
    );
    assert_eq!(
        terminal.initial_xterm_keyboard_type(),
        termeric_core::term::XtermKeyboardType::Normal
    );
}

#[test]
fn headless_plugin_dispatch_surfaces_missing_target() {
    let mut app = App::new_headless(Config::default()).unwrap();
    app.execute_plugin_actions(vec![PluginAction::InjectContext {
        plugin_id: "codex".into(),
        text: "context".into(),
    }]);
    let messages: Vec<_> = app
        .toasts
        .iter()
        .map(|toast| toast.message.as_str())
        .collect();
    assert_eq!(messages, vec!["No writable codex tab found"]);
}

#[cfg(unix)]
#[test]
fn plugin_launch_never_runs_a_synchronous_version_probe() {
    use std::os::unix::fs::PermissionsExt;

    if !std::path::Path::new("/bin/sh").exists() {
        return;
    }
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
    app.execute_plugin_actions(vec![PluginAction::OpenTab {
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
    }]);
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

#[cfg(unix)]
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
    app.execute_plugin_actions(vec![PluginAction::OpenTab {
        program: "/definitely/missing/termeric-plugin-fixture".into(),
        args: vec![],
        cwd: None,
        title: None,
        plugin_id: Some("fixture-plugin".into()),
        profile: None,
        env: HashMap::new(),
    }]);

    assert!(app.info_overlay.is_active());
    assert!(app.info_overlay.title().contains("Failed to launch"));
    assert!(!app.info_overlay.lines().is_empty());
    assert_eq!(app.tabs.as_ref().unwrap().count(), 0);
}

#[cfg(unix)]
#[test]
fn fake_ai_clis_exercise_every_builtin_command_end_to_end() {
    if !std::path::Path::new("/bin/sh").exists() {
        return;
    }

    let temp = tempfile::tempdir().unwrap();
    let fake_cli = temp.path().join("fake-ai");
    let invocation_log = temp.path().join("fake-ai.log");
    std::fs::write(
        &fake_cli,
        r#"#!/bin/sh
if [ "${1-}" = "--version" ]; then
  printf 'fake-ai 1.0\n'
  exit 0
fi
printf 'launch\tcwd=%s\targv=%s\tcolumns=%s\tlines=%s\n' "$PWD" "$*" "${COLUMNS-}" "${LINES-}" >> "$TERMERIC_FAKE_AI_LOG"
printf '%s\r\n' 'FAKE_AI_RESPONSE_26' '--- a/example.txt' '+++ b/example.txt' '@@ -1 +1 @@' '-old' '+new' 'END_DIFF_26' 'ERROR acceptance failure marker'
i=0
while [ "$i" -lt 40 ]; do
  printf 'padding line %s\r\n' "$i"
  i=$((i + 1))
done
printf '%s\r\n' 'ERROR acceptance failure marker after scrollback'
trap 'printf "signal=INT\\n" >> "$TERMERIC_FAKE_AI_LOG"' INT
while :; do
  if IFS= read -r line; then
    printf 'stdin=%s\n' "$line" >> "$TERMERIC_FAKE_AI_LOG"
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
    config.plugins.claude_code.enabled = true;
    config.plugins.claude_code.binary = fake_cli.display().to_string();
    config.plugins.claude_code.default_profile = Some("acceptance".into());
    config.plugins.claude_code.profiles.insert(
        "acceptance".into(),
        crate::plugin::claude_profiles::ClaudeProfile {
            env: HashMap::from([(
                "TERMERIC_FAKE_AI_LOG".into(),
                invocation_log.display().to_string(),
            )]),
            ..Default::default()
        },
    );
    config.plugins.codex.enabled = true;
    config.plugins.codex.binary = fake_cli.display().to_string();
    config.plugins.codex.default_profile = Some("acceptance".into());
    config.plugins.codex.profiles.insert(
        "acceptance".into(),
        crate::plugin::codex_profiles::CodexProfile {
            env: HashMap::from([(
                "TERMERIC_FAKE_AI_LOG".into(),
                invocation_log.display().to_string(),
            )]),
            ..Default::default()
        },
    );

    let mut app = App::new_headless(config).unwrap();
    let mut tabs = TabManager::new(80, 24, ShellConfig::default(), 1_000, 8 * 1024 * 1024);
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
        for (plugin_id, resume_argument) in
            [("claude-code", "--continue"), ("codex", "resume --last")]
        {
            app.execute_plugin_command(&format!("{plugin_id}.open"));
            wait_for_plugin_fixture(&mut app, plugin_id, "FAKE_AI_RESPONSE_26");

            app.execute_plugin_command(&format!("{plugin_id}.copy-last"));
            assert_eq!(latest_toast(&app), Some("Clipboard not available"));

            app.execute_plugin_command(&format!("{plugin_id}.clear"));
            wait_for_file_text(&invocation_log, "stdin=clear");

            app.execute_plugin_command(&format!("{plugin_id}.review-diff"));
            assert!(app.diff_review.is_active());
            let request_id = app.diff_review.request_id();
            app.diff_review.dismiss(request_id);

            app.execute_plugin_command(&format!("{plugin_id}.copy-diff"));
            assert_eq!(latest_toast(&app), Some("Clipboard not available"));

            app.execute_plugin_command(&format!("{plugin_id}.insert-context"));
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
            let gather = app
                .context_picker
                .begin_gather(request_id, Some("FAKE_CONTEXT_SOURCE_26".into()))
                .expect("fake context source should be selected");
            assert_eq!(
                gather.selected,
                [crate::ui::context_picker::ContextSourceId::TerminalOutput]
            );
            app.handle_context_gathered(request_id, Ok("FAKE_CONTEXT_PAYLOAD_26".into()));
            wait_for_file_text(&invocation_log, "stdin=FAKE_CONTEXT_PAYLOAD_26");

            app.execute_plugin_command(&format!("{plugin_id}.toggle-transcript"));
            let recording = app
                .tabs
                .as_ref()
                .unwrap()
                .iter()
                .find(|(_, tab)| {
                    matches!(
                        &tab.tab_type,
                        crate::tabs::TabType::Plugin { plugin_id: id } if id == plugin_id
                    ) && !tab.is_closing()
                })
                .is_some_and(|(_, tab)| {
                    tab.ai_session
                        .as_ref()
                        .is_some_and(|session| session.recording)
                        && tab.transcript.is_some()
                });
            assert!(recording, "{plugin_id} fake session was not live");

            app.execute_plugin_command(&format!("{plugin_id}.export-session"));
            assert!(latest_toast(&app).is_some_and(|message| message.contains("Exported")));

            // The initial readiness marker is emitted before the fake CLI's
            // padding lines. Wait for its final error marker so this assertion
            // proves an actual scrollback jump instead of racing PTY delivery.
            wait_for_plugin_fixture(
                &mut app,
                plugin_id,
                "ERROR acceptance failure marker after scrollback",
            );
            app.execute_plugin_command(&format!("{plugin_id}.jump-to-error"));
            let scroll_offset = app
                .tabs
                .as_ref()
                .unwrap()
                .active_tab()
                .unwrap()
                .focused_terminal()
                .unwrap()
                .grid()
                .scroll_offset();
            assert!(
                scroll_offset > 0,
                "{plugin_id} did not jump to its error output"
            );

            app.execute_plugin_command(&format!("{plugin_id}.select-profile"));
            assert!(app.info_overlay.is_active());
            let request_id = app.info_overlay.request_id();
            assert!(
                app.pending_profile_selection
                    .as_ref()
                    .is_some_and(|pending| pending.plugin_id == plugin_id
                        && pending.profiles == ["acceptance".to_string()])
            );
            app.apply_info_overlay_action(crate::ui::info_overlay::InfoOverlayAction::Choose {
                request_id,
                choice_id: "acceptance".into(),
            });
            let selected_profile = app
                .tabs
                .as_ref()
                .and_then(|tabs| {
                    tabs.iter()
                        .find(|(_, tab)| {
                            matches!(
                                &tab.tab_type,
                                crate::tabs::TabType::Plugin { plugin_id: id }
                                    if id == plugin_id && !tab.is_closing()
                            )
                        })
                        .map(|(_, tab)| tab)
                })
                .and_then(|tab| tab.ai_session.as_ref())
                .map(|session| session.profile.as_str());
            assert_eq!(selected_profile, Some("acceptance"));
            assert!(app.pending_profile_selection.is_none());
            app.info_overlay.dismiss(app.info_overlay.request_id());

            app.execute_plugin_command(&format!("{plugin_id}.restart"));
            wait_for_plugin_fixture(&mut app, plugin_id, "FAKE_AI_RESPONSE_26");
            let log = std::fs::read_to_string(&invocation_log).unwrap();
            assert!(
                log.lines()
                    .filter(|line| line.starts_with("launch\t"))
                    .all(|line| line.contains("columns=80\tlines=24")),
                "terminal geometry was not passed to fake AI CLI: {log}"
            );

            app.execute_plugin_command(&format!("{plugin_id}.resume"));
            wait_for_file_text(&invocation_log, resume_argument);

            // Stop the active transcript if restart preserved an active
            // recorder; this also exercises the stop/flush half of the action.
            if app
                .tabs
                .as_ref()
                .and_then(|tabs| tabs.active_tab())
                .is_some_and(|tab| tab.transcript.is_some())
            {
                app.execute_plugin_command(&format!("{plugin_id}.toggle-transcript"));
            }
        }

        let log = std::fs::read_to_string(&invocation_log).unwrap();
        let cwd = temp.path().display();
        let expected = [
            (format!("launch\tcwd={cwd}\targv=\t"), 2),
            (format!("launch\tcwd={cwd}\targv=--continue\t"), 1),
            (
                format!(
                    "launch\tcwd={cwd}\targv=--ask-for-approval untrusted --sandbox workspace-write\t"
                ),
                2,
            ),
            (
                format!(
                    "launch\tcwd={cwd}\targv=--ask-for-approval untrusted --sandbox workspace-write resume --last\t"
                ),
                1,
            ),
        ];
        for (prefix, count) in expected {
            assert_eq!(
                log.lines().filter(|line| line.starts_with(&prefix)).count(),
                count,
                "fake AI launch contract mismatch for {prefix:?}: {log}"
            );
        }
    }));
    crate::session::set_test_sessions_base_dir(None);
    result.unwrap();
}

#[cfg(unix)]
fn wait_for_plugin_fixture(app: &mut App, plugin_id: &str, needle: &str) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        if let Some(tabs) = &mut app.tabs {
            let (_, events) = tabs.poll_events();
            for event in events {
                app.handle_terminal_event(event);
            }
        }
        let found = app
            .tabs
            .as_ref()
            .and_then(|tabs| tabs.find_plugin_tab_index(plugin_id))
            .and_then(|index| tabs_at(app, index))
            .and_then(|tab| tab.focused_terminal())
            .is_some_and(|terminal| {
                terminal
                    .grid()
                    .export_scrollback_text()
                    .join("\n")
                    .contains(needle)
            });
        if found {
            return;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    panic!("timed out waiting for {plugin_id} fake CLI output containing {needle:?}");
}

#[cfg(unix)]
fn tabs_at(app: &App, index: usize) -> Option<&Tab> {
    app.tabs
        .as_ref()?
        .iter()
        .find_map(|(candidate, tab)| (candidate == index).then_some(tab))
}

#[cfg(unix)]
fn wait_for_file_text(path: &std::path::Path, needle: &str) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        if std::fs::read_to_string(path)
            .ok()
            .is_some_and(|text| text.contains(needle))
        {
            return;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    panic!("timed out waiting for {needle:?} in {}", path.display());
}

#[cfg(unix)]
fn latest_toast(app: &App) -> Option<&str> {
    app.toasts.iter().last().map(|toast| toast.message.as_str())
}

#[test]
fn headless_overlay_close_states_do_not_block_future_input() {
    let mut app = App::new_headless(Config::default()).unwrap();
    app.command_palette.open();
    assert!(app.command_palette.is_active());
    app.command_palette.close();
    assert!(!app.command_palette.is_active());

    app.domain_picker.open(&app.config.domains);
    assert!(app.domain_picker.is_active());
    app.domain_picker.close();
    assert!(!app.domain_picker.is_active());

    app.settings_panel.toggle();
    assert!(app.settings_panel.is_active());
    app.settings_panel.close();
    assert!(!app.settings_panel.is_active());
    app.settings_panel.toggle();
    assert!(app.settings_panel.is_active());

    app.commands_panel.toggle();
    assert!(app.commands_panel.is_open());
    app.commands_panel.close();
    assert!(!app.commands_panel.is_open());
}

#[test]
fn settings_toggle_switches_directly_from_peer_side_panels() {
    let mut app = App::new_headless(Config::default()).unwrap();

    app.commands_panel.toggle();
    assert!(app.commands_panel.is_open());
    app.toggle_settings_surface();
    assert!(!app.commands_panel.is_open());
    assert!(app.settings_panel.is_active());

    app.toggle_settings_surface();
    assert!(!app.settings_panel.is_active());
    app.toggle_settings_surface();
    assert!(app.settings_panel.is_active());
}

#[cfg(unix)]
#[test]
fn app_mediated_clipboard_reply_returns_to_requesting_pane_not_focus() {
    if !std::path::Path::new("/bin/sh").exists() {
        return;
    }
    let mut app = App::new_headless(Config::default()).unwrap();
    let mut tabs = TabManager::new(80, 24, ShellConfig::default(), 1_000, 8 * 1024 * 1024);
    let tab_id = tabs
        .new_tab_with_command(
            "/bin/sh",
            &["-c".into(), "sleep 2".into()],
            None,
            Some("pane-owner"),
            None,
            HashMap::new(),
        )
        .unwrap();
    let script = concat!(
        "stty raw -echo; ",
        "reply=$(dd bs=1 count=9 2>/dev/null); ",
        "printf '\\rreply='; ",
        "printf '%s' \"$reply\" | od -An -tx1; ",
        "printf '\\r\\n'"
    );
    let shell = ShellConfig {
        program: Some("/bin/sh".into()),
        args: vec!["-c".into(), script.into()],
        env: Default::default(),
    };
    let mut panes = PaneManager::new(
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
    let requesting_pane = panes.split(SplitDirection::Vertical).unwrap();
    panes.focus(PaneId(0));
    tabs.active_tab_mut().unwrap().pane_manager = Some(panes);
    app.tabs = Some(tabs);

    app.handle_terminal_event(RoutedTerminalEvent {
        origin: TerminalEventOrigin::Pane {
            tab_id,
            pane_id: requesting_pane,
        },
        event: TerminalEvent::ClipboardRequest {
            selection: ClipboardRequest::Clipboard,
            reply_uses_8bit_controls: false,
        },
    });

    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        app.tabs.as_mut().unwrap().poll_events();
        let text = app
            .tabs
            .as_ref()
            .unwrap()
            .active_tab()
            .unwrap()
            .pane_manager
            .as_ref()
            .unwrap()
            .get_pane(requesting_pane)
            .unwrap()
            .terminal
            .grid()
            .export_scrollback_text()
            .join("\n");
        if text.contains("1b 5d 35 32 3b 63 3b 1b 5c") {
            break;
        }
        std::thread::sleep(Duration::from_millis(2));
    }

    let panes = app
        .tabs
        .as_ref()
        .unwrap()
        .active_tab()
        .unwrap()
        .pane_manager
        .as_ref()
        .unwrap();
    let requesting_text = panes
        .get_pane(requesting_pane)
        .unwrap()
        .terminal
        .grid()
        .export_scrollback_text()
        .join("\n");
    let focused_text = panes
        .get_pane(PaneId(0))
        .unwrap()
        .terminal
        .grid()
        .export_scrollback_text()
        .join("\n");
    assert!(requesting_text.contains("1b 5d 35 32 3b 63 3b 1b 5c"));
    assert!(!focused_text.contains("reply="));
    assert_eq!(panes.focused_id(), PaneId(0));

    // Give each pane deliberately different state, then prove all global
    // surfaces rebind when pane focus moves.
    let panes = app
        .tabs
        .as_mut()
        .unwrap()
        .active_tab_mut()
        .unwrap()
        .pane_manager
        .as_mut()
        .unwrap();
    for index in 0..30 {
        panes
            .get_pane_mut(PaneId(0))
            .unwrap()
            .terminal
            .process(format!("left-{index}\r\n").as_bytes());
        panes
            .get_pane_mut(requesting_pane)
            .unwrap()
            .terminal
            .process(format!("right-{index}\r\n").as_bytes());
    }
    panes
        .get_pane_mut(PaneId(0))
        .unwrap()
        .terminal
        .process(b"alpha https://l.io\r\n");
    panes
        .get_pane_mut(requesting_pane)
        .unwrap()
        .terminal
        .process(b"beta https://r.io\r\n\x1b[?2026h");
    panes
        .get_pane_mut(PaneId(0))
        .unwrap()
        .terminal
        .set_selection(Some(termeric_core::Selection::new(
            termeric_core::Point::new(0, 0),
            termeric_core::SelectionType::Simple,
        )));
    panes.focus(PaneId(0));

    app.search.start();
    for character in "alpha".chars() {
        app.with_search_and_active_grid(|search, grid| search.push_char(character, grid));
    }
    app.with_search_and_active_grid(|search, grid| search.finish(grid));
    assert!(app.search.match_count() > 0);
    assert!(app.active_terminal().unwrap().selection().is_some());
    assert!(!app.active_terminal().unwrap().is_synchronized_output());

    app.tabs
        .as_mut()
        .unwrap()
        .active_tab_mut()
        .unwrap()
        .pane_manager
        .as_mut()
        .unwrap()
        .focus(requesting_pane);
    app.refresh_focused_terminal_surfaces();

    assert_eq!(app.search.match_count(), 0);
    assert!(app.active_terminal().unwrap().selection().is_none());
    assert!(app.active_terminal().unwrap().is_synchronized_output());
    assert!(
        (0..app.active_terminal().unwrap().grid().rows()).any(|row| {
            app.url_cache
                .url_at(row, 5)
                .is_some_and(|url| url.url == "https://r.io")
        })
    );

    app.active_terminal_mut()
        .unwrap()
        .grid_mut()
        .set_scroll_offset(0);
    app.handle_scroll(3.0);
    assert!(app.active_terminal().unwrap().grid().scroll_offset() > 0);
    assert_eq!(
        app.tabs
            .as_ref()
            .unwrap()
            .active_tab()
            .unwrap()
            .pane_manager
            .as_ref()
            .unwrap()
            .get_pane(PaneId(0))
            .unwrap()
            .terminal
            .grid()
            .scroll_offset(),
        0
    );
}

#[cfg(unix)]
#[test]
fn osc_titles_and_dynamic_colors_apply_to_exact_inactive_tab_and_pane_origins() {
    if !std::path::Path::new("/bin/sh").exists() {
        return;
    }
    let mut app = App::new_headless(Config::default()).unwrap();
    let mut tabs = TabManager::new(40, 8, ShellConfig::default(), 1_000, 8 * 1024 * 1024);
    let dormant_tab_id = tabs
        .new_tab_with_command(
            "/bin/sh",
            &["-c".into(), "sleep 2".into()],
            None,
            Some("dormant"),
            None,
            HashMap::new(),
        )
        .unwrap();
    let mut panes = PaneManager::new(
        40,
        8,
        ShellConfig {
            program: Some("/bin/sh".into()),
            args: vec!["-c".into(), "sleep 2".into()],
            env: Default::default(),
        },
        1_000,
        8 * 1024 * 1024,
        800.0,
        320.0,
        10.0,
        20.0,
    )
    .unwrap();
    let second_pane = panes.split(SplitDirection::Vertical).unwrap();
    panes.focus(PaneId(0));
    tabs.active_tab_mut().unwrap().pane_manager = Some(panes);
    let active_tab_id = tabs
        .new_tab_with_command(
            "/bin/sh",
            &["-c".into(), "sleep 2".into()],
            None,
            Some("active"),
            None,
            HashMap::new(),
        )
        .unwrap();
    app.tabs = Some(tabs);

    let first_origin = TerminalEventOrigin::Pane {
        tab_id: dormant_tab_id,
        pane_id: PaneId(0),
    };
    let second_origin = TerminalEventOrigin::Pane {
        tab_id: dormant_tab_id,
        pane_id: second_pane,
    };
    app.handle_terminal_event(RoutedTerminalEvent {
        origin: first_origin,
        event: TerminalEvent::DynamicColor(termeric_core::DynamicColorOperation::Set {
            target: termeric_core::DynamicColorTarget::Palette(3),
            color: termeric_core::Color::rgb(255, 0, 0),
        }),
    });
    app.handle_terminal_event(RoutedTerminalEvent {
        origin: second_origin,
        event: TerminalEvent::DynamicColor(termeric_core::DynamicColorOperation::Set {
            target: termeric_core::DynamicColorTarget::Palette(3),
            color: termeric_core::Color::rgb(0, 0, 255),
        }),
    });
    app.handle_terminal_event(RoutedTerminalEvent {
        origin: first_origin,
        event: TerminalEvent::TitleChanged("pane-build".into()),
    });
    app.handle_terminal_event(RoutedTerminalEvent {
        origin: TerminalEventOrigin::Tab {
            tab_id: active_tab_id,
        },
        event: TerminalEvent::TitleChanged("active-editor".into()),
    });

    let tabs = app.tabs.as_ref().unwrap();
    assert_eq!(
        tabs.terminal_for_origin(first_origin)
            .unwrap()
            .dynamic_colors()
            .palette(3),
        Some(termeric_core::Color::rgb(255, 0, 0))
    );
    assert_eq!(
        tabs.terminal_for_origin(second_origin)
            .unwrap()
            .dynamic_colors()
            .palette(3),
        Some(termeric_core::Color::rgb(0, 0, 255))
    );
    let titles = tabs.tab_info();
    assert_eq!(titles[0].title, "pane-build");
    assert_eq!(titles[1].title, "active-editor");
    assert_eq!(
        tabs.iter()
            .find(|(_, tab)| tab.id == dormant_tab_id)
            .and_then(|(_, tab)| tab.pane_manager.as_ref())
            .and_then(|panes| panes.get_pane(PaneId(0)))
            .and_then(|pane| pane.title.as_deref()),
        Some("pane-build")
    );
    assert_eq!(tabs.active_index(), 1);

    let tabs = app.tabs.as_mut().unwrap();
    assert!(!tabs.set_title_for_origin(
        TerminalEventOrigin::Pane {
            tab_id: dormant_tab_id,
            pane_id: PaneId(u32::MAX),
        },
        "stale-pane-title".into(),
    ));
    assert_eq!(tabs.tab_info()[0].title, "pane-build");

    assert!(tabs.set_title_for_origin(first_origin, String::new()));
    assert_eq!(
        tabs.iter()
            .find(|(_, tab)| tab.id == dormant_tab_id)
            .and_then(|(_, tab)| tab.pane_manager.as_ref())
            .and_then(|panes| panes.get_pane(PaneId(0)))
            .and_then(|pane| pane.title.as_deref()),
        None,
        "an empty OSC title must clear persisted pane title metadata"
    );
}

#[cfg(unix)]
#[test]
fn complete_workspace_round_trip_restores_nested_panes_plugin_profile_and_focus() {
    if !std::path::Path::new("/bin/sh").exists() {
        return;
    }
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
        config.plugins.codex.enabled = true;
        config.plugins.codex.binary = fake_cli.display().to_string();
        config.plugins.codex.default_profile = Some("archived".into());
        config.plugins.codex.profiles.insert(
            "archived".into(),
            crate::plugin::codex_profiles::CodexProfile::default(),
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
            let owner = tab.take_terminal_owner().unwrap();
            let mut panes = PaneManager::from_existing(
                owner,
                ShellConfig {
                    program: Some("/bin/sh".into()),
                    args: vec!["-c".into(), "sleep 30".into()],
                    env: Default::default(),
                },
                1_000,
                8 * 1024 * 1024,
                800.0,
                480.0,
                10.0,
                20.0,
            );
            let right = panes.split(SplitDirection::Vertical).unwrap();
            panes.focus(right);
            let lower_right = panes.split(SplitDirection::Horizontal).unwrap();
            panes.focus(lower_right);
            tab.pane_manager = Some(panes);
        }
        app.tabs = Some(tabs);
        app.execute_plugin_command("codex.open");
        let report = app.save_workspace_as("Nested and AI".into()).unwrap();

        app.tabs.as_mut().unwrap().new_tab().unwrap();
        let outcome = app.restore_workspace_archive(&report.id).unwrap();
        assert_eq!(outcome.restored_tabs, 2);
        assert_eq!(outcome.skipped_tabs, 0);
        let tabs = app.tabs.as_ref().unwrap();
        assert_eq!(tabs.count(), 2);
        assert_eq!(tabs.active_index(), 1);
        let shell = tabs.iter().find(|(index, _)| *index == 0).unwrap().1;
        let panes = shell.pane_manager.as_ref().unwrap();
        assert_eq!(panes.pane_count(), 3);
        assert_eq!(panes.focused_id(), PaneId(2));
        let plugin = tabs.iter().find(|(index, _)| *index == 1).unwrap().1;
        assert!(matches!(
            &plugin.tab_type,
            crate::tabs::TabType::Plugin { plugin_id } if plugin_id == "codex"
        ));
        assert_eq!(
            plugin
                .ai_session
                .as_ref()
                .map(|session| session.profile.as_str()),
            Some("archived")
        );
        assert_eq!(
            app.app_state.last_workspace_id.as_deref(),
            Some(report.id.as_str())
        );
    }

    crate::workspace::set_test_workspaces_base_dir(None);
    AppState::set_test_state_path(None);
}

#[cfg(unix)]
#[test]
fn workspace_restore_partially_recovers_missing_cwd_profile_and_cli() {
    if !std::path::Path::new("/bin/sh").exists() {
        return;
    }
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
        launch: Some(launch),
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
            launch: launch.clone(),
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
        config.plugins.claude_code.enabled = true;
        config.plugins.claude_code.binary = fake_cli.display().to_string();
        config.plugins.codex.enabled = true;
        config.plugins.codex.binary = temp.path().join("missing-codex").display().to_string();
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

#[cfg(unix)]
#[test]
fn cli_launch_plan_creates_ordered_shell_and_arbitrary_command_tabs() {
    if !std::path::Path::new("/bin/sh").exists() {
        return;
    }
    let temp = tempfile::tempdir().unwrap();
    let mut config = Config::default();
    config.shell.program = Some("/bin/sh".into());
    config.shell.args = vec!["-c".into(), "sleep 2".into()];
    let mut app = App::new_headless(config).unwrap();
    app.allow_auto_restore = false;
    app.launch = crate::cli::WindowLaunch {
        domain: None,
        domain_snapshot: None,
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
                splits: Vec::new(),
            },
        ],
        app_id: None,
        pixel_size: None,
        renderer_backend: None,
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
                    .export_scrollback_text()
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

#[cfg(unix)]
#[test]
fn terminal_profile_cli_launch_applies_env_cwd_title_and_workspace_identity() {
    if !std::path::Path::new("/bin/sh").exists() {
        return;
    }
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
        tab.split_shell_config(),
        config.profiles["dev"].shell_config()
    );

    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        app.process_pty_events();
        let text = app
            .active_terminal()
            .unwrap()
            .grid()
            .export_scrollback_text()
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
        workspace.tabs[0].launch,
        crate::workspace::WorkspaceLaunch::Profile { name: "dev".into() }
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

#[cfg(unix)]
#[test]
fn mixed_shell_and_profile_panes_restore_each_saved_launch() {
    if !std::path::Path::new("/bin/sh").exists() {
        return;
    }
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
        launch: crate::workspace::WorkspaceLaunch::Shell,
        primary_pane: 0,
        focused_pane: 7,
        layout: crate::workspace::WorkspacePaneLayout::Split {
            direction: crate::workspace::WorkspaceSplitDirection::Vertical,
            ratio: 0.5,
            first: Box::new(crate::workspace::WorkspacePaneLayout::Pane { pane: 0 }),
            second: Box::new(crate::workspace::WorkspacePaneLayout::Pane { pane: 7 }),
        },
        panes: vec![
            crate::workspace::WorkspacePaneSnapshot {
                id: 0,
                launch: Some(crate::workspace::WorkspaceLaunch::Shell),
                title: None,
                working_directory: None,
                last_command: None,
                scrollback_file: None,
                scrollback_truncated: false,
                scrollback_lines: Vec::new(),
            },
            crate::workspace::WorkspacePaneSnapshot {
                id: 7,
                launch: Some(crate::workspace::WorkspaceLaunch::Profile { name: "dev".into() }),
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
        let panes = restored
            .active_tab()
            .unwrap()
            .pane_manager
            .as_ref()
            .unwrap();
        let default_text = panes
            .get_pane(crate::panes::PaneId(0))
            .unwrap()
            .terminal
            .grid()
            .export_scrollback_text()
            .join("\n");
        let profile_text = panes
            .get_pane(crate::panes::PaneId(7))
            .unwrap()
            .terminal
            .grid()
            .export_scrollback_text()
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

#[cfg(unix)]
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
        app.command_palette.handle_char(character);
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
    app.handle_palette_result(crate::ui::command_palette::PaletteResult::RunAction(
        "profile.open.dev".into(),
    ));
    assert!(matches!(
        &app.tabs.as_ref().unwrap().active_tab().unwrap().tab_type,
        crate::tabs::TabType::Profile { name } if name == "dev"
    ));

    app.apply_config(Config::default()).unwrap();
    app.command_palette.open();
    for character in "dev".chars() {
        app.command_palette.handle_char(character);
    }
    assert!(
        app.command_palette
            .build_info()
            .entries
            .iter()
            .all(|entry| !entry.title.contains("New Tab: dev"))
    );
}

#[cfg(unix)]
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
    config.plugins.codex.enabled = true;
    config.plugins.codex.binary = fake_cli.display().to_string();
    config.plugins.codex.profiles.insert(
        "review".into(),
        crate::plugin::codex_profiles::CodexProfile::default(),
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
        tab.ai_session
            .as_ref()
            .map(|session| session.profile.as_str()),
        Some("review")
    );
}

#[cfg(unix)]
#[test]
fn missing_automatic_multi_window_workspace_falls_back_but_explicit_cli_is_strict() {
    if !std::path::Path::new("/bin/sh").exists() {
        return;
    }
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
