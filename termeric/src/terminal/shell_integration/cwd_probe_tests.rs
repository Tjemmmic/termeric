//! Audit every accepted cwd transition, not just the final state of a PTY read.

use termeric_core::Terminal;

pub(super) fn first_cwd_mismatch(bytes: &[u8], expected: &str) -> Option<(usize, String)> {
    // The native semantics fixture never changes directories. Presence is
    // checked separately at its first prompt and at completion.
    let mut terminal = Terminal::new(100, 30);
    for (offset, byte) in bytes.iter().enumerate() {
        terminal.process(std::slice::from_ref(byte));
        if let Some(cwd) = terminal.reported_working_directory()
            && cwd.path() != expected
        {
            return Some((offset, cwd.path().to_owned()));
        }
        // This is an observation-only replay. Never execute emitted replies
        // or app side effects, and do not accumulate their event queue.
        terminal.drain_events();
    }
    None
}

#[test]
fn cwd_probe_rejects_transient_misreports_even_when_final_state_is_correct() {
    let correct = b"\x1b]7;file:///tmp/home%252Fpercent\x1b\\";
    let incorrect = b"\x1b]7;file:///tmp/home%2Fpercent\x1b\\";
    for initial_correct in [false, true] {
        let mut bytes = Vec::new();
        if initial_correct {
            bytes.extend(correct);
        }
        bytes.extend(incorrect);
        // VTE dispatches OSC when it sees the ESC starting the ST terminator.
        let mismatch_offset = bytes.len() - 2;
        bytes.extend(correct);
        bytes.extend(b"\x1b]133;A\x07\x1b]133;B\x07prompt");
        let mut final_state_only = Terminal::new(100, 30);
        final_state_only.process(&bytes);
        assert_eq!(
            final_state_only
                .reported_working_directory()
                .unwrap()
                .path(),
            "/tmp/home%2Fpercent"
        );
        assert_eq!(
            first_cwd_mismatch(&bytes, "/tmp/home%2Fpercent"),
            Some((mismatch_offset, "/tmp/home/percent".into()))
        );
    }
}

#[test]
fn cwd_probe_accepts_exact_repeated_unicode_paths_in_all_supported_framing() {
    let expected = "/tmp/space %2F/界";
    let uri = b"file:///tmp/space%20%252F/%E7%95%8C";
    let mut bytes = "\x1b[31mUnicode output: 界\x1b[0m\r\n".as_bytes().to_vec();
    for (start, end) in [
        (&b"\x1b]7;"[..], &b"\x1b\\"[..]),
        (&b"\x1b]7;"[..], &b"\x07"[..]),
        (&b"\x9d7;"[..], &b"\x9c"[..]),
    ] {
        let report = [start, uri, end].concat();
        assert_eq!(first_cwd_mismatch(&report, expected), None);
        assert_eq!(
            first_cwd_mismatch(&report, "/different").map(|(_, actual)| actual),
            Some(expected.into())
        );
        bytes.extend(start);
        bytes.extend(uri);
        bytes.extend(end);
        bytes.extend(b"\x1b]133;A\x07\x1b]133;B\x07");
    }
    assert_eq!(first_cwd_mismatch(&bytes, expected), None);
    let mut terminal = Terminal::new(100, 30);
    terminal.process(&bytes);
    assert_eq!(
        terminal.reported_working_directory().unwrap().path(),
        expected
    );
}

#[test]
fn cwd_probe_detects_mismatch_after_a_completed_command() {
    let mut bytes = b"\x1b]7;file:///tmp/expected\x07\x1b]133;A\x07\x1b]133;B\x07echo ready\x1b]133;C\x07\r\nready\r\n\x1b]133;D;0\x07".to_vec();
    bytes.extend(b"\x1b]7;file:///tmp/wrong\x07");
    let mismatch_offset = bytes.len() - 1;
    bytes.extend(b"\x1b]7;file:///tmp/expected\x07\x1b]133;A\x07\x1b]133;B\x07");
    assert_eq!(
        first_cwd_mismatch(&bytes, "/tmp/expected"),
        Some((mismatch_offset, "/tmp/wrong".into()))
    );
}
