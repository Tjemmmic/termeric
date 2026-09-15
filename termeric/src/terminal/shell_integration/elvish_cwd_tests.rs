use super::*;
use std::ffi::OsStr;
use std::process::Command;
use termeric_core::{Terminal, TerminalEvent};

fn report(path: impl AsRef<OsStr>, before_readline: bool) -> Option<Vec<u8>> {
    let Some(program) = super::tests::executable_on_path("elvish") else {
        assert_ne!(
            std::env::var("TERMERIC_REQUIRE_ELVISH_CWD_TESTS").as_deref(),
            Ok("1"),
            "Elvish cwd tests required, but elvish is unavailable"
        );
        eprintln!("skipping Elvish cwd emission: elvish unavailable");
        return None;
    };
    let home = tempfile::tempdir().unwrap();
    // Elvish only supplies edit: in interactive mode. Stub its hook registry,
    // not the encoder, and lexically shadow the filesystem-backed builtin pwd
    // to test arbitrary bytes and exact limits with the unchanged whole module.
    // The separate real-PTY acceptance test verifies native editor wiring.
    let registry = r#"
var pwd = $E:TEST_DIRECTORY
var edit: = (ns [&before-readline=[{ printf '<user>' }] &after-readline=[] &after-command=[]])
set after-chdir = [{|_| printf '<user>' }]
"#;
    let invoke = if before_readline {
        "for hook $edit:before-readline { $hook }"
    } else {
        "for hook $after-chdir { $hook ignored }"
    };
    let script = format!("{registry}\n{ELVISH_MODULE}\n{invoke}\n");
    let output = Command::new(program)
        .args(["-norc", "-c", &script])
        .env_clear()
        .env("HOME", home.path())
        .env("LC_ALL", "C.UTF-8")
        .env("TERMERIC_ELVISH_HAD_XDG_DATA_DIRS", "0")
        .env("TEST_DIRECTORY", path.as_ref())
        .output()
        .unwrap();
    let diagnostics = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "{}", diagnostics.escape_debug());
    assert!(output.stderr.is_empty(), "{}", diagnostics.escape_debug());
    assert!(output.stdout.starts_with(b"<user>"));
    Some(output.stdout[b"<user>".len()..].to_vec())
}

#[test]
fn elvish_cwd_cannot_inject_terminal_commands_from_either_hook() {
    for before_readline in [false, true] {
        let Some(bytes) = report(
            "/tmp/control\x07\x1b]2;INJECTED_TITLE\x07suffix",
            before_readline,
        ) else {
            return;
        };
        let mut terminal = Terminal::new(80, 24);
        terminal.process(&bytes);
        assert!(
            !terminal
                .drain_events()
                .iter()
                .any(|event| matches!(event, TerminalEvent::TitleChanged(_))),
            "directory report escaped into a title command: {bytes:?}"
        );
        assert!(terminal.reported_working_directory().is_none());
        assert!(bytes.windows(3).any(|window| window == b"%07"));
        assert!(bytes.windows(3).any(|window| window == b"%1B"));
    }
}

#[test]
fn elvish_cwd_round_trips_utf8_percent_and_uri_punctuation() {
    let path = "/tmp/space %2F #?;quote'\"/界/é/👩\u{200d}💻";
    for before_readline in [false, true] {
        let Some(bytes) = report(path, before_readline) else {
            return;
        };
        assert!(bytes.is_ascii());
        let mut terminal = Terminal::new(80, 24);
        terminal.process(&bytes);
        assert_eq!(terminal.reported_working_directory().unwrap().path(), path);
        assert!(terminal.drain_events().is_empty());
    }
}

#[cfg(unix)]
#[test]
fn elvish_cwd_encodes_every_nonzero_byte() {
    use std::fmt::Write;
    use std::os::unix::ffi::OsStringExt;
    let path = std::ffi::OsString::from_vec(b"/bytes/".iter().copied().chain(1..=255).collect());
    let Some(bytes) = report(path, true) else {
        return;
    };
    let mut expected = String::from("\x1b]7;file:///bytes/");
    for byte in 1u8..=255 {
        if byte.is_ascii_alphanumeric() || b"/._~-".contains(&byte) {
            expected.push(char::from(byte));
        } else {
            write!(expected, "%{byte:02X}").unwrap();
        }
    }
    expected.push_str("\x07\x1b]133;A\x07\x1b]133;B\x07");
    assert_eq!(bytes, expected.as_bytes());
    let mut terminal = Terminal::new(80, 24);
    terminal.process(b"\x1b]7;file:///safe\x07");
    terminal.process(&bytes);
    assert_eq!(
        terminal.reported_working_directory().unwrap().path(),
        "/safe"
    );
    assert!(terminal.drain_events().is_empty());
}

#[test]
fn elvish_cwd_bounds_reports_without_truncation_or_losing_prompt_marks() {
    for (path, accepted) in [
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
    ] {
        for before_readline in [false, true] {
            let Some(bytes) = report(&path, before_readline) else {
                return;
            };
            let marks = if before_readline {
                b"\x1b]133;A\x07\x1b]133;B\x07".as_slice()
            } else {
                b""
            };
            assert!(bytes.ends_with(marks));
            let mut terminal = Terminal::new(80, 24);
            terminal.process(&bytes);
            if accepted {
                assert_eq!(terminal.reported_working_directory().unwrap().path(), path);
            } else {
                assert_eq!(bytes, marks);
                assert!(terminal.reported_working_directory().is_none());
            }
        }
    }
}
