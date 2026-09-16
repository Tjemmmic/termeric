use super::*;
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::{Duration, Instant};

fn controlled_picker() -> (UnicodePicker, Receiver<String>, Sender<()>) {
    let (started, observed) = mpsc::channel();
    let (release, gate) = mpsc::channel();
    let mut picker = UnicodePicker::new();
    picker.open();
    picker.worker = Some(
        SearchWorker::with_search(move |query| {
            started.send(query.into()).unwrap();
            gate.recv_timeout(Duration::from_secs(5)).unwrap();
            search_unicode_characters(query)
        })
        .unwrap(),
    );
    (picker, observed, release)
}

fn poll_until(picker: &mut UnicodePicker, condition: impl Fn(&UnicodePicker) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while !condition(picker) {
        picker.poll_search();
        assert!(Instant::now() < deadline, "search lifecycle timed out");
        std::thread::yield_now();
    }
}

#[test]
fn cursor_and_noop_edits_preserve_pending_generation_but_delete_replaces_query() {
    use crate::ui::text_input::TextInputEdit;
    let (mut picker, observed, release) = controlled_picker();
    picker.accessibility_set_filter("snowman");
    observed.recv_timeout(Duration::from_secs(5)).unwrap();
    let generation = picker.generation;
    picker.edit_input(TextInputEdit::Home);
    picker.handle_backspace();
    picker.edit_input(TextInputEdit::End);
    picker.edit_input(TextInputEdit::Delete);
    picker.edit_input(TextInputEdit::Left);
    picker.edit_input(TextInputEdit::Right);
    picker.edit_input(TextInputEdit::WordLeft);
    picker.edit_input(TextInputEdit::BackspaceWord);
    picker.edit_input(TextInputEdit::WordRight);
    picker.edit_input(TextInputEdit::DeleteWord);
    let info = picker.build_info();
    let layout = PaletteLayout::calculate(800.0, 600.0, 10.0, 20.0, 1.0, info.entries.len());
    let view = layout.input_viewport(&info.input, info.cursor).unwrap();
    assert!(
        picker
            .handle_click(view.text_x + 20.0, view.text_y, &layout)
            .is_none()
    );
    assert_eq!(picker.build_info().cursor, 2);
    assert_eq!(picker.generation, generation);
    assert!(picker.pending.is_none());
    release.send(()).unwrap();
    picker.finish_search_for_test();
    let ids: Vec<_> = picker.actions.iter().map(|a| a.id.clone()).collect();
    picker.edit_input(TextInputEdit::Home);
    assert!(
        picker
            .handle_click(view.text_x + 30.0, view.text_y, &layout)
            .is_none()
    );
    assert_eq!(picker.build_info().cursor, 3);
    picker.edit_input(TextInputEdit::Home);
    assert_eq!(
        picker
            .actions
            .iter()
            .map(|a| a.id.clone())
            .collect::<Vec<_>>(),
        ids
    );
    assert!(!picker.search_pending());
    picker.edit_input(TextInputEdit::Delete);
    assert!(picker.actions.is_empty());
    assert_ne!(picker.generation, generation);
    assert_eq!(
        observed.recv_timeout(Duration::from_secs(5)).unwrap(),
        "nowman"
    );
    release.send(()).unwrap();
    picker.finish_search_for_test();
    let generation = picker.generation;
    picker.edit_input(TextInputEdit::WordRight);
    picker.edit_input(TextInputEdit::BackspaceWord);
    assert_eq!(picker.build_info().input, "");
    assert_ne!(picker.generation, generation);
    assert!(!picker.search_pending());
    assert!(picker.pending.is_none());
    assert!(observed.try_recv().is_err());
}

#[test]
fn blocked_search_coalesces_edits_and_retires_actions_without_waiting() {
    let (mut picker, observed, release) = controlled_picker();
    let stale = picker.actions[0].id.clone();
    assert!(picker.accessibility_set_filter("snowman"));
    assert_eq!(
        observed.recv_timeout(Duration::from_secs(5)).unwrap(),
        "snowman"
    );
    // The worker cannot finish until released below. These calls must return.
    assert!(picker.accessibility_set_filter("umbrella"));
    assert!(picker.accessibility_set_filter("woman judge"));
    assert!(picker.actions.is_empty());
    assert!(picker.build_info().entries.is_empty());
    assert_eq!(picker.build_info().status, Some("Searching characters…"));
    assert!(picker.execute_command(&stale).is_none());
    assert!(picker.activate().is_none());
    assert!(picker.search_session);
    release.send(()).unwrap();
    poll_until(&mut picker, |p| p.pending.is_none());
    assert_eq!(
        observed.recv_timeout(Duration::from_secs(5)).unwrap(),
        "woman judge"
    );
    assert!(
        picker.actions.is_empty(),
        "stale completion restored actions"
    );
    release.send(()).unwrap();
    picker.finish_search_for_test();
    assert!(!picker.actions.is_empty());
    assert_eq!(picker.build_info().status, None);
    assert!(observed.try_recv().is_err(), "superseded query was scanned");
}

#[test]
fn exact_and_empty_queries_remain_immediate_during_a_blocked_scan() {
    for replacement in ["U+2603", ""] {
        let (mut picker, observed, release) = controlled_picker();
        picker.accessibility_set_filter("woman judge");
        observed.recv_timeout(Duration::from_secs(5)).unwrap();
        picker.accessibility_set_filter(replacement);
        let expected: Vec<_> = picker.actions.iter().map(|a| a.id.clone()).collect();
        assert!(!expected.is_empty());
        assert_eq!(picker.build_info().status, None);
        release.send(()).unwrap();
        picker.finish_search_for_test();
        assert_eq!(
            picker
                .actions
                .iter()
                .map(|a| a.id.clone())
                .collect::<Vec<_>>(),
            expected
        );
        assert!(picker.activate().is_some());
    }
}

#[test]
fn close_reopen_reuses_inflight_worker_and_rejects_old_generation() {
    let (mut picker, observed, release) = controlled_picker();
    picker.accessibility_set_filter("snowman");
    observed.recv_timeout(Duration::from_secs(5)).unwrap();
    picker.close();
    picker.handle_char('x');
    picker.handle_backspace();
    assert!(!picker.accessibility_set_filter("umbrella"));
    assert!(picker.pending.is_none());
    picker.open();
    picker.accessibility_set_filter("umbrella");
    assert!(picker.pending.is_some());
    assert!(observed.try_recv().is_err());
    release.send(()).unwrap();
    poll_until(&mut picker, |p| p.pending.is_none());
    assert_eq!(
        observed.recv_timeout(Duration::from_secs(5)).unwrap(),
        "umbrella"
    );
    assert!(picker.actions.is_empty());
    picker.close();
    release.send(()).unwrap();
    picker.finish_search_for_test();
    assert!(picker.actions.is_empty());
    assert!(picker.worker.is_none());
    assert!(observed.recv_timeout(Duration::from_secs(5)).is_err());
}

#[test]
fn disconnected_worker_clears_polling_and_next_edit_retries() {
    let mut picker = UnicodePicker::new();
    picker.open();
    let (requests, input) = mpsc::sync_channel(1);
    let (output, completions) = mpsc::sync_channel(1);
    picker.worker = Some(SearchWorker {
        requests,
        completions,
    });
    picker.accessibility_set_filter("snowman");
    assert!(picker.search_pending());
    drop(output);
    drop(input);
    assert!(picker.poll_search());
    assert!(!picker.search_pending());
    assert_eq!(
        picker.build_info().status,
        Some("Character search unavailable; edit to retry")
    );
    assert!(picker.actions.is_empty());
    picker.accessibility_set_filter("umbrella");
    picker.finish_search_for_test();
    assert!(!picker.actions.is_empty());
    assert_eq!(picker.build_info().status, None);
    picker.close();
    assert!(picker.worker.is_none());
}

#[test]
fn unicode_query_limit_matches_keyboard_and_accessibility() {
    let (mut picker, observed, release) = controlled_picker();
    picker.accessibility_set_filter("snowman");
    observed.recv_timeout(Duration::from_secs(5)).unwrap();
    let maximum = "🦀".repeat(256);
    assert!(picker.accessibility_set_filter(&maximum));
    picker.handle_char('a');
    picker.handle_char('\n');
    assert!(!picker.accessibility_set_filter(&(maximum.clone() + "a")));
    assert!(!picker.accessibility_set_filter("bad\nquery"));
    assert_eq!(picker.build_info().input, maximum);
    picker.handle_backspace();
    picker.handle_char('b');
    assert_eq!(picker.build_info().input.chars().count(), 256);
    assert!(picker.build_info().input.ends_with('b'));
    picker.close();
    release.send(()).unwrap();
    picker.finish_search_for_test();
}
