//! Raw local PTYs and synthetic output events only; no provider launches.
use super::*;

#[test]
fn transcript_utf8_carry_follows_owner_through_split_collapse_and_sibling_close() {
    use crate::session::transcript::{EntryKind, TranscriptEntry, TranscriptRecorder};
    use termeric_core::pty::PtyEvent;
    let (mut app, directory, entry, _) = harvest_rerun_capture_fixture_in(false);
    let owner = PaneId(entry.source.pane_id);
    let (sender, receiver) = std::sync::mpsc::channel();
    {
        let tabs = app.tabs.as_mut().unwrap();
        let tab = tabs.active_tab_mut().unwrap();
        tab.replace_lone_test_output(receiver);
        tab.start_transcript(
            TranscriptRecorder::new(directory.path(), None, None, "Local", "owner").unwrap(),
        );
        sender
            .send(PtyEvent::Output(b"before \xf0\x9f".to_vec()))
            .unwrap();
        tabs.poll_events();
        let origin = tabs.origin_for_pane(entry.source.tab_id, owner).unwrap();
        tabs.write_user_to_origin(origin, b"input \xf0\x9f")
            .unwrap();
    }
    let sibling = split_harvest_capture_owner(&mut app, directory.path());
    let sibling_path;
    let _sibling_sender;
    {
        let tabs = app.tabs.as_mut().unwrap();
        let tab = tabs.active_tab_mut().unwrap();
        let pane = tab.panes.get_pane_mut(sibling).unwrap();
        let (sibling_sender, receiver) = std::sync::mpsc::channel();
        _sibling_sender = sibling_sender;
        pane.pty_rx = receiver;
        let recorder =
            TranscriptRecorder::new(directory.path(), None, None, "Local", "sibling").unwrap();
        sibling_path = recorder.path().to_owned();
        pane.integration.start_transcript(recorder);
        pane.integration.observe_output(b"sibling \xe2", false);
        pane.integration.record_input(b"sibling input \xc2");
        sender
            .send(PtyEvent::Output(b"\x8c\xb1 split \xe2".to_vec()))
            .unwrap();
        tabs.poll_events();
        let origin = tabs.origin_for_pane(entry.source.tab_id, owner).unwrap();
        // These protocol bytes must not complete or corrupt the input carry.
        tabs.write_to_origin(origin, b"\x8c\xb1PROTOCOL_ONLY")
            .unwrap();
        tabs.write_user_to_origin(origin, b"\x8c\xb1 split \xc2")
            .unwrap();
        let pane = tabs
            .active_tab_mut()
            .unwrap()
            .panes
            .get_pane_mut(owner)
            .unwrap();
        pane.read_only = true;
        assert!(tabs.write_user_to_origin(origin, b"\xa2DENIED").is_err());
        tabs.active_tab_mut()
            .unwrap()
            .panes
            .get_pane_mut(owner)
            .unwrap()
            .read_only = false;
    }
    app.close_pane_for(Requester::Control, entry.source.tab_id as u64, sibling.0)
        .unwrap();
    let read = |path: &std::path::Path, kind: EntryKind| -> String {
        std::fs::read_to_string(path)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str::<TranscriptEntry>(line).unwrap())
            .filter(|entry| entry.kind == kind)
            .map(|entry| entry.content)
            .collect()
    };
    assert_eq!(read(&sibling_path, EntryKind::Output), "sibling �");
    assert_eq!(read(&sibling_path, EntryKind::Input), "sibling input �");
    let tabs = app.tabs.as_mut().unwrap();
    assert!(tabs.active_tab().unwrap().panes.pane_count() == 1);
    sender
        .send(PtyEvent::Output(b"\x82\xac collapsed".to_vec()))
        .unwrap();
    tabs.poll_events();
    let origin = tabs.origin_for_pane(entry.source.tab_id, owner).unwrap();
    tabs.write_user_to_origin(origin, b"\xa2 collapsed")
        .unwrap();
    let mut recorder = tabs.active_tab_mut().unwrap().stop_transcript().unwrap();
    recorder.finish(Duration::from_secs(2)).unwrap();
    assert_eq!(
        read(recorder.path(), EntryKind::Output),
        "before 🌱 split € collapsed"
    );
    assert_eq!(
        read(recorder.path(), EntryKind::Input),
        "input 🌱 split ¢ collapsed"
    );
}

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
            tab.panes.get_pane_mut(owner).unwrap().pty_rx = receiver;
            let poll = |tab: &mut crate::tabs::Tab| {
                tab.panes
                    .get_pane_mut(owner)
                    .unwrap()
                    .poll_events(|_, _| {});
            };
            let first = "--- a/根\n+++ b/根\n@@ -1 +1 @@\n-old\n+🌱\n";
            for byte in first.as_bytes().chunks(1) {
                sender.send(PtyEvent::Output(byte.to_vec())).unwrap();
                poll(tab);
                assert_eq!(
                    tab.pane_integration(owner)
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
                tab.pane_integration(owner)
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
            let detector = &tab.pane_integration(owner).unwrap().diff_detector;
            assert_eq!(detector.unreviewed_count(), 1);
            let diff = detector.latest_unreviewed().unwrap();
            assert_eq!(diff.hunks.len(), 2);
            assert_eq!(diff.raw, format!("{first}{second}\n"));
            // Capture sees the same bytes as the terminal, but does not own or
            // rewrite the terminal display path.
            let terminal = &tab.panes.get_pane(owner).unwrap().terminal;
            assert!(terminal.grid().plain_lines().join("\n").contains("🌱"));
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
        .pane_integration_mut(owner)
        .unwrap()
        .observe_output(patch, true);
    app.open_agent_diff_review(crate::tabs::TerminalIdentity {
        tab_id: entry.source.tab_id,
        pane_id: owner,
    });
    let request_id = app.diff_review.request_id();
    let target = app.diff_review.target(request_id).unwrap();
    {
        let state = app
            .tabs
            .as_mut()
            .unwrap()
            .active_tab_mut()
            .unwrap()
            .pane_integration_mut(owner)
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
        .pane_integration(owner)
        .unwrap()
        .diff_detector;
    assert_eq!(detector.unreviewed_count(), 64);
    assert_eq!(detector.latest_unreviewed().unwrap().id, 81);
}

#[test]
fn plugin_export_uses_its_own_transcript_or_scrollback_not_focused_sibling() {
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
                        .pane_integration_mut(id)
                        .unwrap()
                        .start_transcript(recorder);
                }
            }
        }
        let focused = app.active_harvest_source();
        app.export_terminal_session(crate::tabs::TerminalIdentity {
            tab_id: entry.source.tab_id,
            pane_id: PaneId(0),
        });
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
        let panes = &mut tab.panes;
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
    app.focus_pane_for(
        Requester::Control,
        entry.source.tab_id as u64,
        entry.source.pane_id,
    )
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
        tab.set_plugin_profile_reference(PaneId(0), "codex", Some("first".into()));
        tab.set_plugin_profile_reference(PaneId(2), "claude-code", Some("second".into()));
    }
    let owner = |pane_id| crate::tabs::TerminalIdentity {
        tab_id: entry.source.tab_id,
        pane_id,
    };
    let focused = app.active_harvest_source();
    app.toggle_terminal_transcript(owner(PaneId(0)));
    app.toggle_terminal_transcript(owner(PaneId(2)));
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
            let state = tab.pane_integration(id).unwrap();
            state.transcript.as_ref().unwrap().path().to_path_buf()
        })
    };
    assert_ne!(paths[0], paths[1]);
    // Refusing a read-only control must not toggle a different focused plugin.
    app.active_panes_mut()
        .get_pane_mut(PaneId(0))
        .unwrap()
        .read_only = true;
    app.toggle_terminal_transcript(owner(PaneId(0)));
    assert!(latest_toast(&app).unwrap().contains("writable local"));
    for id in [PaneId(0), PaneId(2)] {
        assert!(
            app.tabs
                .as_ref()
                .unwrap()
                .active_tab()
                .unwrap()
                .pane_integration(id)
                .unwrap()
                .transcript
                .is_some()
        );
    }
    // Closing the source drops/flushed its recorder, without touching another
    // recorder; collapse then transfers the complete surviving session.
    app.close_pane_for(Requester::Control, entry.source.tab_id as u64, 0)
        .unwrap();
    app.close_pane_for(Requester::Control, entry.source.tab_id as u64, 1)
        .unwrap();
    let tab = app.tabs.as_ref().unwrap().active_tab().unwrap();
    assert!(tab.panes.pane_count() == 1);
    let state = tab.focused_integration().unwrap();
    assert_eq!(state.agent.as_ref().unwrap().agent_id, "claude-code");
    assert_eq!(state.transcript.as_ref().unwrap().path(), paths[1]);
    let survivor = tab.focused_terminal_identity();
    app.toggle_terminal_transcript(survivor);
    let tab = app.tabs.as_ref().unwrap().active_tab().unwrap();
    assert!(tab.focused_integration().unwrap().transcript.is_none());
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
        let panes = &mut tabs.active_tab_mut().unwrap().panes;
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
        tab.pane_integration(PaneId(2))
            .unwrap()
            .diff_detector
            .unreviewed_count(),
        0
    );
    app.open_agent_diff_review(crate::tabs::TerminalIdentity {
        tab_id: entry.source.tab_id,
        pane_id: PaneId(0),
    });
    let request_id = app.diff_review.request_id();
    assert_eq!(
        app.diff_review.target(request_id).unwrap().pane_id,
        PaneId(0)
    );
    app.focus_pane_for(Requester::Control, entry.source.tab_id as u64, 1)
        .unwrap();
    app.apply_diff_review_action(crate::ui::diff_review::DiffReviewAction::Dismiss { request_id });
    let tab = app.tabs.as_ref().unwrap().active_tab().unwrap();
    assert_eq!(
        tab.pane_integration(PaneId(0))
            .unwrap()
            .diff_detector
            .unreviewed_count(),
        0
    );
    assert_eq!(
        tab.pane_integration(PaneId(1))
            .unwrap()
            .diff_detector
            .unreviewed_count(),
        1
    );
    app.open_agent_diff_review(crate::tabs::TerminalIdentity {
        tab_id: entry.source.tab_id,
        pane_id: PaneId(1),
    });
    let request_id = app.diff_review.request_id();
    app.close_pane_for(Requester::Control, entry.source.tab_id as u64, 1)
        .unwrap();
    app.close_pane_for(Requester::Control, entry.source.tab_id as u64, 2)
        .unwrap();
    let replacement = split_harvest_capture_owner(&mut app, directory.path());
    mark_capture_plugin_owner(&mut app, replacement, "claude-code");
    let tab = app.tabs.as_mut().unwrap().active_tab_mut().unwrap();
    tab.pane_integration_mut(replacement)
        .unwrap()
        .observe_output(
            b"--- a/replacement\n+++ b/replacement\n@@ -1 +1 @@\n-old\n+new\nfinished\n",
            true,
        );
    tab.pane_integration_mut(replacement)
        .unwrap()
        .finish_stream();
    app.apply_diff_review_action(crate::ui::diff_review::DiffReviewAction::Dismiss { request_id });
    assert_eq!(
        app.tabs
            .as_ref()
            .unwrap()
            .active_tab()
            .unwrap()
            .pane_integration(replacement)
            .unwrap()
            .diff_detector
            .unreviewed_count(),
        1
    );
}
