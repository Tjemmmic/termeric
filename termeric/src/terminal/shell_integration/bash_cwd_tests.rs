use super::*;
use std::ffi::OsStr;
use std::process::Command;

fn report(path: impl AsRef<OsStr>) -> Option<Vec<u8>> {
    let program = super::tests::executable_on_path("bash")?;
    let home = tempfile::tempdir().unwrap();
    let script = format!(
        "{}\nPWD=$TEST_DIRECTORY\n__termeric_after_prompt\n[[ $LC_ALL == POSIX ]]\n",
        bash_rc()
    );
    let output = Command::new(program)
        .args(["--noprofile", "--norc", "-c", &script])
        .env_clear()
        .env("HOME", home.path())
        .env("TEST_DIRECTORY", path.as_ref())
        .env("LC_ALL", "POSIX")
        .output()
        .unwrap();
    assert!(output.status.success(), "{:?}", output.stderr);
    assert!(output.stderr.is_empty(), "{:?}", output.stderr);
    Some(output.stdout)
}

#[test]
fn bash_cwd_cannot_inject_terminal_commands() {
    let path = "/tmp/control\x07\x1b]2;INJECTED_TITLE\x07suffix";
    let Some(bytes) = report(path) else {
        eprintln!("skipping Bash cwd emission: bash unavailable");
        return;
    };
    let mut terminal = termeric_core::Terminal::new(80, 24);
    terminal.process(&bytes);
    assert!(
        !terminal
            .drain_events()
            .iter()
            .any(|event| matches!(event, termeric_core::TerminalEvent::TitleChanged(_))),
        "directory report escaped into a title command: {bytes:?}"
    );
    // The receiver intentionally refuses control-bearing cwd paths, even when
    // encoded. It must not accept only the prefix before an embedded BEL.
    assert!(terminal.reported_working_directory().is_none());
    assert!(bytes.windows(3).any(|window| window == b"%07"));
    assert!(bytes.windows(3).any(|window| window == b"%1B"));
}

#[test]
fn bash_cwd_encoding_round_trips_printable_utf8_and_uri_characters() {
    let path = "/tmp/space %2F #?;quote'\"/界/é/👩\u{200d}💻";
    let Some(bytes) = report(path) else {
        eprintln!("skipping Bash cwd emission: bash unavailable");
        return;
    };
    assert!(
        bytes.is_ascii(),
        "wire payload must be ASCII percent encoding"
    );
    let mut terminal = termeric_core::Terminal::new(80, 24);
    terminal.process(&bytes);
    assert_eq!(terminal.reported_working_directory().unwrap().path(), path);
    assert!(
        terminal
            .drain_events()
            .iter()
            .all(|event| !matches!(event, termeric_core::TerminalEvent::TitleChanged(_)))
    );
}

#[cfg(unix)]
#[test]
fn bash_cwd_encodes_every_nonzero_byte_without_leaking_protocol_controls() {
    use std::os::unix::ffi::OsStringExt;
    let path = std::ffi::OsString::from_vec(b"/bytes/".iter().copied().chain(1..=255).collect());
    let Some(bytes) = report(path) else {
        eprintln!("skipping Bash cwd emission: bash unavailable");
        return;
    };
    let mut expected = String::from("\x1b]7;file:///bytes/");
    for byte in 1u8..=255 {
        if byte.is_ascii_alphanumeric() || b"/._~-".contains(&byte) {
            expected.push(char::from(byte));
        } else {
            use std::fmt::Write;
            write!(expected, "%{byte:02X}").unwrap();
        }
    }
    expected.push_str("\x07\x1b]133;A\x07\x1b]133;B\x07");
    assert_eq!(bytes, expected.as_bytes());
    let mut terminal = termeric_core::Terminal::new(80, 24);
    terminal.process(b"\x1b]7;file:///safe\x07");
    terminal.process(&bytes);
    assert_eq!(
        terminal.reported_working_directory().unwrap().path(),
        "/safe"
    );
    assert!(terminal.drain_events().is_empty());
}

#[test]
fn bash_cwd_bounds_encoded_reports_without_truncating_or_losing_prompt_marks() {
    for (path, accepted) in [
        (format!("/{}", "a".repeat(4088)), true),
        (format!("/{}", "a".repeat(4089)), false),
        (format!("/aa{}", "%".repeat(1362)), true),
        (format!("/aaa{}", "%".repeat(1362)), false),
        (String::new(), false),
        (String::from("relative"), false),
    ] {
        let Some(bytes) = report(&path) else {
            eprintln!("skipping Bash cwd emission: bash unavailable");
            return;
        };
        assert!(bytes.ends_with(b"\x1b]133;A\x07\x1b]133;B\x07"));
        let mut terminal = termeric_core::Terminal::new(80, 24);
        terminal.process(&bytes);
        if accepted {
            assert_eq!(terminal.reported_working_directory().unwrap().path(), path);
        } else {
            assert_eq!(bytes, b"\x1b]133;A\x07\x1b]133;B\x07");
            assert!(terminal.reported_working_directory().is_none());
        }
    }
}
