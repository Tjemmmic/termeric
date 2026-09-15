//! Raw local PTYs and synthetic output events only; no provider launches.
use super::*;

#[test]
fn single_and_split_diff_capture_waits_for_real_eof_not_poll_boundaries() {
    use termeric_core::pty::PtyEvent;
    for split in [false, true] {
        for ending in 0..3 {
            let (mut app, _directory, entry, _) = harvest_rerun_capture_fixture_in(split);
            let owner = PaneId(entry.source.pane_id);
            mark_capture_plugin_owner(&mut app, owner, "codex");
            let tab = app.tabs.as_mut().unwrap().active_tab_mut().unwrap();
            let (sender, receiver) = std::sync::mpsc::channel();
            if let Some(panes) = &mut tab.pane_manager {
                panes.get_pane_mut(owner).unwrap().pty_rx = receiver;
            } else {
                tab.replace_single_test_output(receiver);
            }
            let poll = |tab: &mut crate::tabs::Tab| {
                if let Some(panes) = &mut tab.pane_manager {
                    panes.get_pane_mut(owner).unwrap().poll_events(|_, _| {});
                } else {
                    tab.poll_events();
                }
            };
            let first = "--- a/根\n+++ b/根\n@@ -1 +1 @@\n-old\n+🌱\n";
            for byte in first.as_bytes().chunks(1) {
                sender.send(PtyEvent::Output(byte.to_vec())).unwrap();
                poll(tab);
                assert_eq!(
                    tab.pane_session(owner)
                        .unwrap()
                        .diff_detector
                        .unreviewed_count(),
                    0
                );
            }
            for _ in 0..10 {
                poll(tab);
            }
            let second = "@@ -5 +5 @@\n-old\n+new";
            sender
                .send(PtyEvent::Output(second.as_bytes().to_vec()))
                .unwrap();
            poll(tab);
            assert_eq!(
                tab.pane_session(owner)
                    .unwrap()
                    .diff_detector
                    .unreviewed_count(),
                0
            );
            match ending {
                0 => sender.send(PtyEvent::Closed).unwrap(),
                1 => sender
                    .send(PtyEvent::Error("synthetic EOF error".into()))
                    .unwrap(),
                _ => {}
            }
            drop(sender);
            poll(tab);
            poll(tab);
            let detector = &tab.pane_session(owner).unwrap().diff_detector;
            assert_eq!(detector.unreviewed_count(), 1);
            let diff = detector.latest_unreviewed().unwrap();
            assert_eq!(diff.hunks.len(), 2);
            assert_eq!(diff.raw, format!("{first}{second}\n"));
            // Capture sees the same bytes as the terminal, but does not own or
            // rewrite the terminal display path.
            let terminal = if let Some(panes) = &tab.pane_manager {
                &panes.get_pane(owner).unwrap().terminal
            } else {
                tab.focused_terminal().unwrap()
            };
            assert!(
                terminal
                    .grid()
                    .export_scrollback_text()
                    .join("\n")
                    .contains("🌱")
            );
        }
    }
}

#[test]
fn evicted_diff_review_cannot_copy_or_dismiss_a_newer_patch() {
    use crate::ui::diff_review::DiffReviewAction;
    let (mut app, _directory, entry, _) = harvest_rerun_capture_fixture();
    let owner = PaneId(entry.source.pane_id);
    mark_capture_plugin_owner(&mut app, owner, "codex");
    let patch = b"--- a/file\n+++ b/file\n@@ -1 +1 @@\n-old\n+new\nfinished\n";
    app.tabs
        .as_mut()
        .unwrap()
        .active_tab_mut()
        .unwrap()
        .pane_session_mut(owner)
        .unwrap()
        .observe_output(patch, true);
    app.execute_plugin_command("codex.review-diff");
    let request_id = app.diff_review.request_id();
    let target = app.diff_review.target(request_id).unwrap();
    {
        let state = app
            .tabs
            .as_mut()
            .unwrap()
            .active_tab_mut()
            .unwrap()
            .pane_session_mut(owner)
            .unwrap();
        for _ in 0..80 {
            state.observe_output(patch, true);
        }
        assert!(state.diff_detector.detected_by_id(target.diff_id).is_none());
    }
    app.apply_diff_review_action(DiffReviewAction::Copy { request_id });
    assert_eq!(
        latest_toast(&app),
        Some("The reviewed diff is no longer available")
    );
    app.apply_diff_review_action(DiffReviewAction::Dismiss { request_id });
    let detector = &app
        .tabs
        .as_ref()
        .unwrap()
        .active_tab()
        .unwrap()
        .pane_session(owner)
        .unwrap()
        .diff_detector;
    assert_eq!(detector.unreviewed_count(), 64);
    assert_eq!(detector.latest_unreviewed().unwrap().id, 81);
}

#[test]
fn plugin_export_uses_its_own_transcript_or_scrollback_not_focused_sibling() {
    let _lock = crate::session::export_job::EXPORT_TEST_LOCK.lock().unwrap();
    for recording in [false, true] {
        let (mut app, directory, entry, _) = harvest_rerun_capture_fixture();
        let base = directory.path().join("exports");
        crate::session::set_test_sessions_base_dir(Some(base.clone()));
        let _reset = SessionDirectory;
        mark_capture_plugin_owner(&mut app, PaneId(0), "codex");
        mark_capture_plugin_owner(&mut app, PaneId(2), "claude-code");
        {
            let tabs = app.tabs.as_mut().unwrap();
            for (id, text) in [(PaneId(0), "OWNED_EXPORT"), (PaneId(2), "SIBLING_EXPORT")] {
                let origin = tabs.origin_for_pane(entry.source.tab_id, id).unwrap();
                tabs.terminal_for_origin_mut(origin)
                    .unwrap()
                    .process(text.as_bytes());
                if recording {
                    let mut recorder = crate::session::transcript::TranscriptRecorder::new(
                        &directory.path().join("transcripts"),
                        None,
                        None,
                        "Local",
                        "fixture",
                    )
                    .unwrap();
                    recorder.record_output(text).unwrap();
                    tabs.active_tab_mut()
                        .unwrap()
                        .pane_session_mut(id)
                        .unwrap()
                        .start_transcript(recorder);
                }
            }
        }
        let focused = app.active_harvest_source();
        app.execute_plugin_command("codex.export-session");
        assert!(app.session_export.is_some());
        let deadline = Instant::now() + Duration::from_secs(5);
        while app.session_export.is_some() {
            app.poll_session_export();
            assert!(Instant::now() < deadline, "local export timed out");
            std::thread::yield_now();
        }
        assert_eq!(app.active_harvest_source(), focused);
        assert!(latest_toast(&app).unwrap().contains("Exported"));
        let exports = std::fs::read_dir(base)
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .collect::<Vec<_>>();
        assert_eq!(exports.len(), 1);
        let files = std::fs::read_dir(&exports[0])
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .collect::<Vec<_>>();
        assert!(
            files
                .iter()
                .any(|path| path.extension().is_some_and(|ext| ext == "md"))
        );
        for path in files {
            if path
                .extension()
                .is_some_and(|ext| ext == "md" || ext == "jsonl")
            {
                let text = std::fs::read_to_string(path).unwrap();
                assert!(text.contains("OWNED_EXPORT"));
                assert!(!text.contains("SIBLING_EXPORT"));
            }
        }
    }
}

struct SessionDirectory;
impl Drop for SessionDirectory {
    fn drop(&mut self) {
        crate::session::set_test_sessions_base_dir(None);
    }
}

#[test]
fn split_transcript_records_only_its_original_terminal() {
    let (mut app, directory, entry, _) = harvest_rerun_capture_fixture_in(false);
    let recorder = crate::session::transcript::TranscriptRecorder::new(
        directory.path(),
        None,
        None,
        "Local",
        "fixture",
    )
    .unwrap();
    app.tabs
        .as_mut()
        .unwrap()
        .active_tab_mut()
        .unwrap()
        .start_transcript(recorder);
    let sibling = split_harvest_capture_owner(&mut app, directory.path());
    let mut senders = Vec::new();
    {
        let tabs = app.tabs.as_mut().unwrap();
        let tab = tabs.active_tab_mut().unwrap();
        let panes = tab.pane_manager.as_mut().unwrap();
        for (id, text) in [
            (PaneId(entry.source.pane_id), b"OWNED_OUTPUT\n".as_slice()),
            (sibling, b"SIBLING_OUTPUT\n".as_slice()),
        ] {
            let (sender, receiver) = std::sync::mpsc::channel();
            panes.get_pane_mut(id).unwrap().pty_rx = receiver;
            sender
                .send(termeric_core::pty::PtyEvent::Output(text.to_vec()))
                .unwrap();
            senders.push(sender);
        }
        tabs.poll_events();
        for (id, data) in [
            (PaneId(entry.source.pane_id), b"OWNED_INPUT".as_slice()),
            (sibling, b"SIBLING_INPUT".as_slice()),
        ] {
            let origin = tabs.origin_for_pane(entry.source.tab_id, id).unwrap();
            tabs.write_user_to_origin(origin, data).unwrap();
        }
    }
    app.focus_control_pane(entry.source.tab_id as u64, entry.source.pane_id)
        .unwrap();
    let mut recorder = app
        .tabs
        .as_mut()
        .unwrap()
        .active_tab_mut()
        .unwrap()
        .stop_transcript()
        .unwrap();
    recorder.finish(Duration::from_secs(2)).unwrap();
    let recorded = std::fs::read_to_string(recorder.path()).unwrap();
    assert!(recorded.contains("OWNED_OUTPUT"));
    assert!(recorded.contains("OWNED_INPUT"));
    assert!(
        !recorded.contains("SIBLING_OUTPUT"),
        "sibling output leaked into transcript"
    );
    assert!(
        !recorded.contains("SIBLING_INPUT"),
        "sibling input leaked into transcript"
    );
}

#[test]
fn transcript_controls_and_close_keep_independent_owner_state() {
    let (mut app, directory, entry, _) = harvest_rerun_capture_fixture();
    crate::session::set_test_sessions_base_dir(Some(directory.path().join("sessions")));
    let _reset = SessionDirectory;
    mark_capture_plugin_owner(&mut app, PaneId(0), "codex");
    mark_capture_plugin_owner(&mut app, PaneId(2), "claude-code");
    {
        let tab = app.tabs.as_mut().unwrap().active_tab_mut().unwrap();
        tab.set_plugin_profile(PaneId(0), "codex", "first".into());
        tab.set_plugin_profile(PaneId(2), "claude-code", "second".into());
        tab.pane_session_mut(PaneId(0))
            .unwrap()
            .ai_session
            .as_mut()
            .unwrap()
            .repo_name = Some("owner-zero".into());
        tab.pane_session_mut(PaneId(2))
            .unwrap()
            .ai_session
            .as_mut()
            .unwrap()
            .repo_name = Some("owner-two".into());
    }
    let focused = app.active_harvest_source();
    app.execute_plugin_command("codex.toggle-transcript");
    app.execute_plugin_command("claude-code.toggle-transcript");
    assert_eq!(app.active_harvest_source(), focused);
    let paths = {
        let tabs = app.tabs.as_mut().unwrap();
        for (id, bytes) in [
            (PaneId(0), b"ZERO_INPUT".as_slice()),
            (PaneId(2), b"TWO_INPUT".as_slice()),
        ] {
            let origin = tabs.origin_for_pane(entry.source.tab_id, id).unwrap();
            tabs.write_user_to_origin(origin, bytes).unwrap();
            tabs.write_to_origin(origin, b"PROTOCOL_ONLY").unwrap();
        }
        let tab = tabs.active_tab().unwrap();
        [PaneId(0), PaneId(2)].map(|id| {
            let state = tab.pane_session(id).unwrap();
            assert!(state.ai_session.as_ref().unwrap().recording);
            state.transcript.as_ref().unwrap().path().to_path_buf()
        })
    };
    assert_ne!(paths[0], paths[1]);
    // Refusing a read-only control must not toggle a different focused plugin.
    app.tabs
        .as_mut()
        .unwrap()
        .active_tab_mut()
        .unwrap()
        .pane_manager
        .as_mut()
        .unwrap()
        .get_pane_mut(PaneId(0))
        .unwrap()
        .read_only = true;
    app.execute_plugin_command("codex.toggle-transcript");
    assert!(latest_toast(&app).unwrap().contains("writable local"));
    for id in [PaneId(0), PaneId(2)] {
        assert!(
            app.tabs
                .as_ref()
                .unwrap()
                .active_tab()
                .unwrap()
                .pane_session(id)
                .unwrap()
                .ai_session
                .as_ref()
                .unwrap()
                .recording
        );
    }
    // Closing the source drops/flushed its recorder, without touching another
    // recorder; collapse then transfers the complete surviving session.
    app.close_control_pane(entry.source.tab_id as u64, 0)
        .unwrap();
    app.close_control_pane(entry.source.tab_id as u64, 1)
        .unwrap();
    let tab = app.tabs.as_ref().unwrap().active_tab().unwrap();
    assert!(tab.pane_manager.is_none());
    let state = tab.focused_session().unwrap();
    let ai = state.ai_session.as_ref().unwrap();
    assert!(ai.recording);
    assert_eq!(ai.profile, "second");
    assert_eq!(ai.repo_name.as_deref(), Some("owner-two"));
    assert_eq!(state.transcript.as_ref().unwrap().path(), paths[1]);
    app.execute_plugin_command("claude-code.toggle-transcript");
    let tab = app.tabs.as_ref().unwrap().active_tab().unwrap();
    assert!(
        !tab.focused_session()
            .unwrap()
            .ai_session
            .as_ref()
            .unwrap()
            .recording
    );
    assert!(tab.focused_session().unwrap().transcript.is_none());
    for (index, expected, absent) in [
        (0, "ZERO_INPUT", "TWO_INPUT"),
        (1, "TWO_INPUT", "ZERO_INPUT"),
    ] {
        let text = std::fs::read_to_string(&paths[index]).unwrap();
        assert!(text.contains(expected));
        assert!(!text.contains(absent));
        assert!(!text.contains("PROTOCOL_ONLY"));
    }
}

#[test]
fn diff_observation_and_delayed_review_are_terminal_local() {
    let (mut app, directory, entry, _) = harvest_rerun_capture_fixture();
    mark_capture_plugin_owner(&mut app, PaneId(0), "codex");
    mark_capture_plugin_owner(&mut app, PaneId(1), "claude-code");
    let mut senders = Vec::new();
    {
        let tabs = app.tabs.as_mut().unwrap();
        let panes = tabs
            .active_tab_mut()
            .unwrap()
            .pane_manager
            .as_mut()
            .unwrap();
        for id in panes.pane_ids() {
            let (sender, receiver) = std::sync::mpsc::channel();
            panes.get_pane_mut(id).unwrap().pty_rx = receiver;
            sender.send(termeric_core::pty::PtyEvent::Output(format!("--- a/owner-{}.rs\n+++ b/owner-{}.rs\n@@ -1,1 +1,1 @@\n-old\n+new\nfinished\n", id.0, id.0).into_bytes())).unwrap();
            senders.push(sender);
        }
        tabs.poll_events();
    }
    let tab = app.tabs.as_ref().unwrap().active_tab().unwrap();
    assert_eq!(tab.unreviewed_diff_count(), 2);
    assert_eq!(
        tab.pane_session(PaneId(2))
            .unwrap()
            .diff_detector
            .unreviewed_count(),
        0
    );
    app.execute_plugin_command("codex.review-diff");
    let request_id = app.diff_review.request_id();
    assert_eq!(
        app.diff_review.target(request_id).unwrap().pane_id,
        PaneId(0)
    );
    app.focus_control_pane(entry.source.tab_id as u64, 1)
        .unwrap();
    app.apply_diff_review_action(crate::ui::diff_review::DiffReviewAction::Dismiss { request_id });
    let tab = app.tabs.as_ref().unwrap().active_tab().unwrap();
    assert_eq!(
        tab.pane_session(PaneId(0))
            .unwrap()
            .diff_detector
            .unreviewed_count(),
        0
    );
    assert_eq!(
        tab.pane_session(PaneId(1))
            .unwrap()
            .diff_detector
            .unreviewed_count(),
        1
    );
    app.execute_plugin_command("claude-code.review-diff");
    let request_id = app.diff_review.request_id();
    app.close_control_pane(entry.source.tab_id as u64, 1)
        .unwrap();
    app.close_control_pane(entry.source.tab_id as u64, 2)
        .unwrap();
    let replacement = split_harvest_capture_owner(&mut app, directory.path());
    mark_capture_plugin_owner(&mut app, replacement, "claude-code");
    let tab = app.tabs.as_mut().unwrap().active_tab_mut().unwrap();
    tab.pane_session_mut(replacement).unwrap().observe_output(
        b"--- a/replacement\n+++ b/replacement\n@@ -1 +1 @@\n-old\n+new\nfinished\n",
        true,
    );
    tab.pane_session_mut(replacement).unwrap().finish_stream();
    app.apply_diff_review_action(crate::ui::diff_review::DiffReviewAction::Dismiss { request_id });
    assert_eq!(
        app.tabs
            .as_ref()
            .unwrap()
            .active_tab()
            .unwrap()
            .pane_session(replacement)
            .unwrap()
            .diff_detector
            .unreviewed_count(),
        1
    );
}
