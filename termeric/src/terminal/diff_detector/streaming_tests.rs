use super::*;

const PATCH: &str = "--- a/根.rs\n+++ b/根.rs\n@@ -1 +1 @@\n-old  \n+new 🌱\n";

fn capture(chunks: &[&[u8]]) -> Vec<String> {
    let mut detector = DiffDetector::new();
    let mut completed = Vec::new();
    for chunk in chunks {
        detector.feed_bytes(chunk, |diff| completed.push(diff.raw.clone()));
    }
    detector.finish_stream(|diff| completed.push(diff.raw.clone()));
    detector.finish_stream(|_| panic!("EOF duplicated a completion"));
    assert_eq!(detector.take_notices().count(), 0);
    completed
}

#[test]
fn every_byte_split_preserves_unicode_ansi_crlf_hunks_and_adjacent_files() {
    let decorated = PATCH
        .lines()
        .map(|line| format!("\x1b[31m{line}\x1b[0m\r\n"))
        .collect::<String>();
    let second = "--- /dev/null\n+++ b/new\n@@ -0,0 +1,2 @@\n+one\n+two\n@@ -2,0 +4 @@ title\n+three\n\\ No newline at end of file";
    let stream = format!("\x1b]8;;https://example.invalid/\x1b\\{decorated}\x1b]8;;\x07{second}");
    let expected = vec![PATCH.to_owned(), format!("{second}\n")];
    for split in 0..=stream.len() {
        assert_eq!(
            capture(&[&stream.as_bytes()[..split], &stream.as_bytes()[split..]]),
            expected,
            "split {split}"
        );
    }
    let chunks: Vec<_> = stream.as_bytes().chunks(1).collect();
    assert_eq!(capture(&chunks), expected);
    let onlcr = stream.replace("\r\n", "\r\r\n");
    assert_eq!(
        capture(&onlcr.as_bytes().chunks(1).collect::<Vec<_>>()),
        expected
    );
}

#[test]
fn poll_boundaries_do_not_publish_partial_files_and_noise_is_not_copied() {
    let mut detector = DiffDetector::new();
    detector.feed_bytes(PATCH.as_bytes(), |_| panic!("not EOF"));
    for _ in 0..100 {
        detector.feed_bytes(b"", |_| panic!("idle is not EOF"));
    }
    assert_eq!(detector.unreviewed_count(), 0);
    let next_hunk = "@@ -5 +5 @@\n-old\n+new\n";
    detector.feed_bytes(next_hunk.as_bytes(), |_| panic!("not a file boundary"));
    let mut calls = 0;
    detector.feed_bytes(b"finished\n", |diff| {
        calls += 1;
        assert_eq!(diff.raw, format!("{PATCH}{next_hunk}"));
        assert_eq!(diff.hunks.len(), 2);
    });
    assert_eq!(calls, 1);
}

#[test]
fn malformed_or_incomplete_captures_never_publish_a_truncated_patch() {
    for body in [
        "@@ -1,2 +1,2 @@\n-old\n+new\n",
        "@@ -1 +1 @@\n-old\n+new\n+extra\n",
        "@@ -1 +1 @@\n-old\n+new\n@@ broken\n",
        "@@ -1 +1 @@\n-old\n+new\n@@ -1 +1 @@\n-old\n+new\n",
        "@@ -0 +0 @@\n-old\n+new\n",
        "@@ -1,999999999999999999999 +1 @@\n-old\n+new\n",
        "@@ -1,1 +1,1 @@oops\n-old\n+new\n",
        "@@ -1 +1 @@\n\n",
        "@@ -1 +1 @@\n-old\n+ne\rrewritten\n",
        "@@ -1 +1 @@\n-old\n+new\x1b[2K\n",
        "@@ -1 +1 @@\n-old\n+new\x1b[",
        "@@ -1 +1 @@\n-old\n+new\n\\ No newline at end of file\n\\ No newline at end of file\n",
    ] {
        let mut detector = DiffDetector::new();
        let stream = format!("--- a/file\n+++ b/file\n{body}");
        for byte in stream.as_bytes().chunks(1) {
            detector.feed_bytes(byte, |_| panic!("accepted {body:?}"));
        }
        detector.finish_stream(|_| panic!("accepted at EOF: {body:?}"));
        assert_eq!(detector.unreviewed_count(), 0);
        assert_eq!(detector.take_notices().count(), 1, "{body:?}");
    }
}

#[test]
fn invalid_utf8_rejects_whole_capture_and_recovers_on_next_file() {
    let mut detector = DiffDetector::new();
    detector.feed_bytes(
        b"--- a/file\n+++ b/file\n@@ -1 +1 @@\n-old\n+\xff\n",
        |_| panic!("invalid UTF-8"),
    );
    detector.feed_bytes(PATCH.as_bytes(), |_| {});
    detector.finish_stream(|diff| assert_eq!(diff.raw, PATCH));
    assert_eq!(detector.unreviewed_count(), 1);
    assert_eq!(detector.take_notices().count(), 1);
}

#[test]
fn removed_header_like_text_and_dev_null_deletion_are_supported() {
    let patch = "--- a/deleted\n+++ /dev/null\n@@ -1,2 +0,0 @@\n--- a/not-a-file-header\n-removed  \n\\ No newline at end of file\n";
    let mut detector = DiffDetector::new();
    detector.feed_bytes(patch.as_bytes(), |_| {});
    detector.finish_stream(|diff| {
        assert_eq!(diff.filename, "deleted");
        assert_eq!(diff.raw, patch);
        assert_eq!(
            diff.hunks[0].lines[0],
            DiffLine::Removed("-- a/not-a-file-header".into())
        );
    });
    assert_eq!(detector.unreviewed_count(), 1);
}

#[test]
fn empty_ranges_use_the_position_after_the_named_line() {
    let patch = "--- a/file\n+++ b/file\n@@ -1 +1 @@\n-old\n+new\n@@ -1,0 +2 @@\n+inserted\n";
    assert_eq!(capture(&[patch.as_bytes()]), vec![patch]);
}

#[test]
fn exact_line_and_hunk_ceilings_still_accept_complete_patches() {
    let lines = MAX_PATCH_LINES - 3; // File headers and one hunk header.
    let patch = format!(
        "--- a/file\n+++ b/file\n@@ -0,0 +1,{lines} @@\n{}",
        "+x\n".repeat(lines)
    );
    assert_eq!(capture(&[patch.as_bytes()]), vec![patch.as_str()]);
    let patch = format!(
        "--- a/file\n+++ b/file\n{}",
        (1..=MAX_HUNKS)
            .map(|row| format!("@@ -{row} +{row} @@\n-x\n+y\n"))
            .collect::<String>()
    );
    assert_eq!(capture(&[patch.as_bytes()]), vec![patch.as_str()]);
    let patch = format!(
        "--- a/file\n+++ b/file\n@@ -0,0 +1 @@\n+{}\n",
        "x".repeat(MAX_LINE_BYTES - 1)
    );
    assert_eq!(capture(&[patch.as_bytes()]), vec![patch.as_str()]);
}

#[test]
fn osc_payload_newlines_cannot_invent_a_patch() {
    let hidden = format!("\x1b]0;{PATCH}\x1b\\ordinary output\n");
    assert!(capture(&hidden.as_bytes().chunks(1).collect::<Vec<_>>()).is_empty());
}

#[test]
fn pending_lines_and_control_strings_have_a_hard_storage_ceiling() {
    for prefix in ["--- a/", "\x1b]0;"] {
        let mut detector = DiffDetector::new();
        detector.feed_bytes(prefix.as_bytes(), |_| {});
        for _ in 0..100 {
            detector.feed_bytes(&[b'x'; MAX_LINE_BYTES], |_| panic!("oversized"));
            assert!(detector.framer.allocated_bytes() <= MAX_LINE_BYTES);
        }
        detector.finish_stream(|_| panic!("oversized EOF"));
        assert_eq!(detector.unreviewed_count(), 0);
        assert_eq!(detector.take_notices().count(), 1);
    }
}

#[test]
fn patch_byte_line_and_hunk_limits_reject_without_partial_history() {
    let cases = [
        format!(
            "@@ -0,0 +1,100 @@\n{}",
            format!("+{}\n", "x".repeat(4096)).repeat(100)
        ),
        format!(
            "@@ -0,0 +1,{MAX_PATCH_LINES} @@\n{}",
            "+x\n".repeat(MAX_PATCH_LINES)
        ),
        (1..=MAX_HUNKS + 1)
            .map(|row| format!("@@ -{row} +{row} @@\n-x\n+y\n"))
            .collect::<String>(),
    ];
    for body in cases {
        let mut detector = DiffDetector::new();
        detector.feed_bytes(b"--- a/file\n+++ b/file\n", |_| {});
        for chunk in body.as_bytes().chunks(503) {
            detector.feed_bytes(chunk, |_| panic!("oversized patch"));
            if let ScanState::InDiff(capture) = &detector.state {
                assert!(capture.raw.capacity() <= MAX_PATCH_BYTES);
                assert!(capture.lines <= MAX_PATCH_LINES);
                assert!(capture.hunks <= MAX_HUNKS);
            }
        }
        detector.finish_stream(|_| panic!("oversized patch EOF"));
        assert_eq!(detector.unreviewed_count(), 0);
        assert_eq!(detector.take_notices().count(), 1);
    }
}

#[test]
fn history_evicts_by_count_and_owned_capacity_without_reusing_ids() {
    for large in [false, true] {
        let patch = if large {
            format!(
                "--- a/file\n+++ b/file\n@@ -0,0 +1,60 @@\n{}finished\n",
                format!("+{}\n", "x".repeat(4000)).repeat(60)
            )
        } else {
            format!("{PATCH}finished\n")
        };
        let mut detector = DiffDetector::new();
        let mut completions = 0;
        for _ in 0..200 {
            detector.feed_bytes(patch.as_bytes(), |diff| {
                completions += 1;
                assert_eq!(diff.id, completions);
            });
            assert!(detector.history_bytes <= MAX_HISTORY_BYTES);
            assert!(detector.detected.len() <= MAX_HISTORY_COUNT);
            assert_eq!(
                detector.history_bytes,
                detector
                    .detected
                    .iter()
                    .map(DetectedDiff::owned_bytes)
                    .sum::<usize>()
                    + detector.detected.capacity() * std::mem::size_of::<DetectedDiff>()
            );
            // Reviewed data is included in both budgets, not an escape hatch.
            assert!(detector.mark_reviewed(completions));
        }
        assert_eq!(completions, 200);
        assert!(detector.detected_by_id(1).is_none());
        assert!(!detector.mark_reviewed(1));
        assert_eq!(detector.unreviewed_count(), 0);
        assert_eq!(detector.take_notices().count(), 1);
        if large {
            assert!(detector.detected.len() < MAX_HISTORY_COUNT);
        } else {
            assert_eq!(detector.detected.len(), MAX_HISTORY_COUNT);
        }
    }
}

#[test]
fn clearing_forgets_history_and_the_capture_in_progress_without_reusing_ids() {
    let mut detector = DiffDetector::new();
    detector.feed_bytes(format!("{PATCH}finished\n").as_bytes(), |_| {});
    // Half a patch, ending in half a line, is in progress.
    detector.feed_bytes(b"--- a/next\n+++ b/next\n@@ -1 +1 @@\n-old\n+ne", |_| {});
    assert_eq!(detector.unreviewed_count(), 1);

    assert!(detector.clear());
    assert_eq!(detector.unreviewed_count(), 0);
    assert!(detector.latest_unreviewed().is_none());
    assert_eq!(
        detector.history_bytes,
        detector.detected.capacity() * std::mem::size_of::<DetectedDiff>()
    );
    assert!(!detector.clear(), "nothing is left to forget");

    // What was in progress is not finished by later output, and nothing is
    // reported about it.
    let mut completed = Vec::new();
    detector.feed_bytes(b"w\nfinished\n", |diff| completed.push(diff.id));
    detector.finish_stream(|diff| completed.push(diff.id));
    assert!(completed.is_empty());
    assert_eq!(detector.take_notices().count(), 0);

    detector.feed_bytes(format!("{PATCH}finished\n").as_bytes(), |diff| {
        completed.push(diff.id);
    });
    assert_eq!(completed, [2]);
}
