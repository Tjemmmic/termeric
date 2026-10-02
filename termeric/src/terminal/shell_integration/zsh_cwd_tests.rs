use super::*;
use std::ffi::OsStr;
use std::process::Command;
use termeric_core::{Terminal, TerminalEvent};

fn report(path: impl AsRef<OsStr>, options: &str) -> Option<Vec<u8>> {
    let program = super::tests::test_shell("zsh")?;
    let home = tempfile::tempdir().unwrap();
    let script = format!(
        "{ZSH_RC}\nPWD=$TEST_DIRECTORY\n{options}\n__termeric_test_options=$(setopt)\n__termeric_precmd\n[[ $LC_ALL == C.UTF-8 && $(setopt) == $__termeric_test_options ]]\n"
    );
    let output = Command::new(program)
        .args(["-f", "-c", &script])
        .env_clear()
        .env("HOME", home.path())
        .env("TERMERIC_ORIGINAL_ZDOTDIR", "")
        .env("LC_ALL", "C.UTF-8")
        .env("TEST_DIRECTORY", path.as_ref())
        .output()
        .unwrap();
    assert!(output.status.success(), "{:?}", output.stderr);
    assert!(output.stderr.is_empty(), "{:?}", output.stderr);
    Some(output.stdout)
}

#[test]
fn zsh_cwd_cannot_inject_terminal_commands() {
    let Some(bytes) = report("/tmp/control\x07\x1b]2;INJECTED_TITLE\x07suffix", "") else {
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

#[test]
fn zsh_cwd_round_trips_utf8_under_different_indexing_and_multibyte_options() {
    let path = "/tmp/space %2F #?;quote'\"/界/é/👩\u{200d}💻";
    for options in [
        "",
        "setopt ksharrays",
        "unsetopt multibyte",
        "setopt shwordsplit",
    ] {
        let Some(bytes) = report(path, options) else {
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
fn zsh_cwd_encodes_every_nonzero_byte() {
    use std::fmt::Write;
    use std::os::unix::ffi::OsStringExt;
    let path = std::ffi::OsString::from_vec(b"/bytes/".iter().copied().chain(1..=255).collect());
    let Some(bytes) = report(path, "") else {
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
fn zsh_cwd_bounds_reports_without_truncation_and_keeps_prompt_markers() {
    for (path, accepted) in [
        (format!("/{}", "a".repeat(4088)), true),
        (format!("/{}", "a".repeat(4089)), false),
        (format!("/aa{}", "%".repeat(1362)), true),
        (format!("/aaa{}", "%".repeat(1362)), false),
        (String::new(), false),
        (String::from("relative"), false),
    ] {
        let Some(bytes) = report(&path, "") else {
            return;
        };
        assert!(bytes.ends_with(b"\x1b]133;A\x07\x1b]133;B\x07"));
        let mut terminal = Terminal::new(80, 24);
        terminal.process(&bytes);
        if accepted {
            assert_eq!(terminal.reported_working_directory().unwrap().path(), path);
        } else {
            assert_eq!(bytes, b"\x1b]133;A\x07\x1b]133;B\x07");
            assert!(terminal.reported_working_directory().is_none());
        }
    }
}
