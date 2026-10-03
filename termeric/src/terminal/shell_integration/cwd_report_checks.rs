//! The OSC 7 checks every shell's cwd encoder must pass. Each shell's own test
//! file captures the bytes its integration emits and runs them through these.

use std::fmt::Write;
use termeric_core::{Terminal, TerminalEvent};

/// A path whose controls would end the report and start a title command if
/// they reached the wire unencoded.
pub(super) const INJECTION_PATH: &str = "/tmp/control\x07\x1b]2;INJECTED_TITLE\x07suffix";

/// Printable UTF-8, a literal `%2F`, and URI punctuation that must survive.
pub(super) const ROUND_TRIP_PATH: &str = "/tmp/space %2F #?;quote'\"/界/é/👩\u{200d}💻";

/// The prompt marks shells emit after the cwd report.
pub(super) const PROMPT_MARKS: &[u8] = b"\x1b]133;A\x07\x1b]133;B\x07";

/// Paths around the 4,096-byte URI ceiling ("file://" leaves 4,089 bytes for
/// the encoded path), each with whether a complete report fits.
pub(super) fn bound_cases() -> Vec<(String, bool)> {
    vec![
        (String::from("/"), true),
        (String::from("/tmp/plain"), true),
        (format!("/{}", "a".repeat(4088)), true),
        (format!("/{}", "a".repeat(4089)), false),
        (format!("/aa{}", "%".repeat(1362)), true),
        (format!("/aaa{}", "%".repeat(1362)), false),
        (format!("/aa{}", "界".repeat(454)), true),
        (format!("/aaa{}", "界".repeat(454)), false),
        (format!("/{}", "界".repeat(4088)), false),
        (String::new(), false),
        (String::from("relative"), false),
    ]
}

/// The exact BEL-terminated report for `path`, with every byte outside
/// `[A-Za-z0-9/._~-]` percent-encoded.
pub(super) fn encoded_report(path: &[u8]) -> Vec<u8> {
    let mut report = String::from("\x1b]7;file://");
    for &byte in path {
        if byte.is_ascii_alphanumeric() || b"/._~-".contains(&byte) {
            report.push(char::from(byte));
        } else {
            write!(report, "%{byte:02X}").unwrap();
        }
    }
    report.push('\x07');
    report.into_bytes()
}

/// Every nonzero byte after `/bytes/`, so one report covers each encoding.
pub(super) fn every_byte_path() -> Vec<u8> {
    b"/bytes/".iter().copied().chain(1..=255).collect()
}

/// Checks the bytes a shell emitted for [`INJECTION_PATH`].
pub(super) fn assert_cannot_inject(bytes: &[u8]) {
    let mut terminal = Terminal::new(80, 24);
    terminal.process(bytes);
    assert!(
        !terminal
            .drain_events()
            .iter()
            .any(|event| matches!(event, TerminalEvent::TitleChanged(_))),
        "directory report escaped into a title command: {bytes:?}"
    );
    // The receiver refuses control-bearing cwd paths, even when encoded. It
    // must not accept only the prefix before an embedded BEL.
    assert!(terminal.reported_working_directory().is_none());
    assert!(bytes.windows(3).any(|window| window == b"%07"));
    assert!(bytes.windows(3).any(|window| window == b"%1B"));
}

/// Checks that `bytes` is an ASCII report the receiver decodes to `path`.
pub(super) fn assert_round_trips(bytes: &[u8], path: &str) {
    assert!(
        bytes.is_ascii(),
        "wire payload must be ASCII percent encoding"
    );
    let mut terminal = Terminal::new(80, 24);
    terminal.process(bytes);
    assert_eq!(terminal.reported_working_directory().unwrap().path(), path);
    assert!(terminal.drain_events().is_empty());
}

/// Checks the report for [`every_byte_path`]: exact encoding, followed by
/// `suffix`, and refused by the receiver (it holds controls) without events.
pub(super) fn assert_every_byte_encoded(bytes: &[u8], path: &[u8], suffix: &[u8]) {
    assert_eq!(bytes, [encoded_report(path).as_slice(), suffix].concat());
    let mut terminal = Terminal::new(80, 24);
    terminal.process(b"\x1b]7;file:///safe\x07");
    terminal.process(bytes);
    assert_eq!(
        terminal.reported_working_directory().unwrap().path(),
        "/safe"
    );
    assert!(terminal.drain_events().is_empty());
}

/// Checks one [`bound_cases`] entry: a fitting path is reported whole, and
/// any other emits nothing but `suffix`, never a truncated report.
pub(super) fn assert_bounded(bytes: &[u8], path: &str, accepted: bool, suffix: &[u8]) {
    assert!(bytes.ends_with(suffix), "{path:?}: {bytes:?}");
    let mut terminal = Terminal::new(80, 24);
    terminal.process(bytes);
    if accepted {
        assert_eq!(terminal.reported_working_directory().unwrap().path(), path);
    } else {
        assert_eq!(bytes, suffix, "{path:?}");
        assert!(terminal.reported_working_directory().is_none());
    }
}
