use super::*;

fn entries(recorder: &mut TranscriptRecorder) -> Vec<TranscriptEntry> {
    recorder.finish(Duration::from_secs(2)).unwrap();
    fs::read_to_string(recorder.path())
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

fn text(entries: &[TranscriptEntry], kind: EntryKind) -> String {
    entries
        .iter()
        .filter(|entry| entry.kind == kind)
        .map(|entry| entry.content.as_str())
        .collect()
}

#[test]
fn recorded_unicode_is_invariant_under_every_split_and_one_byte_calls() {
    let directory = tempfile::tempdir().unwrap();
    let original = "before é根🌱e\u{301}\x1b[31m\r\n\0after";
    for split in 0..=original.len() + 1 {
        let mut recorder =
            TranscriptRecorder::new(directory.path(), None, None, "Local", "fixture").unwrap();
        let chunks = if split > original.len() {
            original.as_bytes().chunks(1).collect::<Vec<_>>()
        } else {
            vec![&original.as_bytes()[..split], &original.as_bytes()[split..]]
        };
        for chunk in chunks {
            assert_eq!(
                recorder.record_input(chunk).unwrap(),
                TranscriptWrite::Written
            );
            assert_eq!(
                recorder.record_output(chunk).unwrap(),
                TranscriptWrite::Written
            );
        }
        let recorded = entries(&mut recorder);
        assert_eq!(text(&recorded, EntryKind::Input), original);
        assert_eq!(text(&recorded, EntryKind::Output), original);
        assert!(recorder.poll_notices().is_empty());
    }
}

#[test]
fn malformed_and_eof_replacements_are_persisted_and_notices_are_once_per_direction() {
    let directory = tempfile::tempdir().unwrap();
    let mut recorder =
        TranscriptRecorder::new(directory.path(), None, None, "Local", "fixture").unwrap();
    let raw = b"before\xffafter\xf0\x9fX\xe2\x82";
    for byte in raw.chunks(1) {
        recorder.record_input(byte).unwrap();
        recorder.record_output(byte).unwrap();
    }
    assert_eq!(
        recorder.poll_notices(),
        vec![
            TranscriptNotice::TextReplaced { input: true },
            TranscriptNotice::TextReplaced { input: false },
        ]
    );
    recorder.finish_text_streams().unwrap();
    recorder.finish_text_streams().unwrap();
    let recorded = entries(&mut recorder);
    for kind in [EntryKind::Input, EntryKind::Output] {
        assert_eq!(text(&recorded, kind), String::from_utf8_lossy(raw));
    }
    assert!(recorder.poll_notices().is_empty());
    assert!(
        recorded
            .iter()
            .filter(|entry| entry.content.contains('�'))
            .all(|entry| entry.metadata.as_ref().unwrap()["utf8_replaced"] == true)
    );
    let markdown = crate::session::export::markdown_of_file(recorder.path()).unwrap();
    assert!(markdown.contains('�'));
}

#[test]
fn checkpoint_exports_only_decoded_prefix_without_finalizing_a_pending_character() {
    let directory = tempfile::tempdir().unwrap();
    let mut recorder =
        TranscriptRecorder::new(directory.path(), None, None, "Local", "fixture").unwrap();
    recorder.record_output(b"before \xf0\x9f").unwrap();
    let pending = recorder.checkpoint().unwrap();
    let checkpoint = pending
        .reply
        .recv_timeout(Duration::from_secs(2))
        .unwrap()
        .unwrap();
    let prefix = fs::read(recorder.path()).unwrap();
    let prefix = std::str::from_utf8(&prefix[..checkpoint.bytes as usize]).unwrap();
    assert!(prefix.contains("before "));
    assert!(!prefix.contains('�'));
    recorder.record_output(b"\x8c\xb1 after").unwrap();
    assert_eq!(
        text(&entries(&mut recorder), EntryKind::Output),
        "before 🌱 after"
    );
    assert!(recorder.poll_notices().is_empty());
}

#[test]
fn repair_does_not_split_one_calls_redaction_boundary() {
    let directory = tempfile::tempdir().unwrap();
    let mut recorder =
        TranscriptRecorder::new(directory.path(), None, None, "Local", "fixture").unwrap();
    let raw = [
        b"\xff sk-".as_slice(),
        "x".repeat(20_000).as_bytes(),
        b"SYNTHETIC_SUFFIX safe",
    ]
    .concat();
    recorder.record_output(&raw).unwrap();
    let recorded = entries(&mut recorder);
    assert_eq!(text(&recorded, EntryKind::Output), "� sk-[REDACTED] safe");
    assert_eq!(
        recorded
            .iter()
            .filter(|entry| entry.kind == EntryKind::Output)
            .count(),
        1
    );
    let markdown = crate::session::export::markdown_of_file(recorder.path()).unwrap();
    assert!(!markdown.contains("SYNTHETIC_SUFFIX"));
}

#[test]
fn stop_and_drop_finalize_unfinished_input_and_output_once() {
    let directory = tempfile::tempdir().unwrap();
    for explicit in [false, true] {
        let mut recorder =
            TranscriptRecorder::new(directory.path(), None, None, "Local", "fixture").unwrap();
        let path = recorder.path().to_owned();
        recorder.record_input(b"input \xf0\x9f").unwrap();
        recorder.record_output(b"output \xe2").unwrap();
        if explicit {
            recorder.finish(Duration::from_secs(2)).unwrap();
            recorder.finish(Duration::from_secs(2)).unwrap();
        }
        drop(recorder);
        let recorded: Vec<TranscriptEntry> = fs::read_to_string(path)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        assert_eq!(text(&recorded, EntryKind::Input), "input �");
        assert_eq!(text(&recorded, EntryKind::Output), "output �");
    }
}

#[test]
fn oversized_repairs_and_newline_free_calls_are_bounded_and_visible() {
    let directory = tempfile::tempdir().unwrap();
    let mut recorder =
        TranscriptRecorder::new(directory.path(), None, None, "Local", "fixture").unwrap();
    let limit = recorder.limits.max_queue_bytes;
    recorder.record_output(b"\xf0\x9f").unwrap();
    assert_eq!(
        recorder.record_output(vec![b'x'; limit + 1]).unwrap(),
        TranscriptWrite::Backpressure
    );
    assert_eq!(
        recorder.record_output(vec![0xff; limit]).unwrap(),
        TranscriptWrite::DroppedAtBackpressure
    );
    assert_eq!(recorder.queued_bytes_for_diagnostics(), 0);
    recorder.record_output("survives").unwrap();
    assert_eq!(text(&entries(&mut recorder), EntryKind::Output), "survives");
    assert!(recorder.poll_notices().is_empty()); // Overflow is the synchronous Backpressure result.
}

#[test]
fn blocked_writer_does_not_block_decoder_or_carry_text_across_dropped_calls() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("blocked.jsonl");
    let sink_path = path.clone();
    let (release, blocked) = mpsc::channel();
    let mut recorder = TranscriptRecorder::with_factory(
        path,
        metadata_entry(None, None, "Local", "fixture"),
        RecorderLimits {
            queue_entries: 1,
            ..RecorderLimits::default()
        },
        Box::new(move || {
            blocked.recv_timeout(Duration::from_secs(5)).unwrap();
            Ok(Box::new(FileSink(File::create(sink_path)?)))
        }),
        None,
    )
    .unwrap();
    recorder.record_output("fills queue").unwrap();
    recorder.record_input(b"\xe2").unwrap();
    assert_eq!(
        recorder.finish_text_streams().unwrap(),
        TranscriptWrite::Backpressure
    );
    assert_eq!(
        recorder.finish_text_streams().unwrap(),
        TranscriptWrite::Written
    );
    let began = Instant::now();
    // This call has a valid prefix plus a pending codepoint; queue refusal must
    // forget that carry rather than join it to the following accepted call.
    assert_eq!(
        recorder.record_output(b"dropped\xf0\x9f").unwrap(),
        TranscriptWrite::DroppedAtBackpressure
    );
    assert!(began.elapsed() < Duration::from_millis(100));
    assert!(recorder.queued_bytes_for_diagnostics() <= recorder.limits.max_queue_bytes);
    release.send(()).unwrap();
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        if let Ok(checkpoint) = recorder.checkpoint() {
            checkpoint
                .reply
                .recv_timeout(Duration::from_secs(2))
                .unwrap()
                .unwrap();
            break;
        }
        assert!(Instant::now() < deadline);
        std::thread::yield_now();
    }
    recorder.record_output(b"\x8c\xb1kept").unwrap();
    let recorded = entries(&mut recorder);
    assert_eq!(text(&recorded, EntryKind::Output), "fills queue��kept");
}
