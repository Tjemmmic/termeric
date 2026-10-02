use super::*;
use std::process::Command;
use termeric_core::{Terminal, TerminalEvent};

fn report(path: &str) -> Option<Vec<u8>> {
    // Enable native reporting first so the prelude itself emits nothing in
    // this non-interactive test; exercise the same bounded helper directly.
    let script = format!(
        "$env.config.shell_integration.osc7 = true\n{NUSHELL_INIT}\n__termeric_report_initial_cwd $env.TEST_DIRECTORY\n"
    );
    run(&script, path)
}

fn run(script: &str, path: &str) -> Option<Vec<u8>> {
    let program = super::tests::test_shell("nu")?;
    let home = tempfile::tempdir().unwrap();
    let output = Command::new(program)
        .args(["--no-config-file", "-c", script])
        .env_clear()
        .env("HOME", home.path())
        .env("TEST_DIRECTORY", path)
        .output()
        .unwrap();
    let diagnostics = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "{}", diagnostics.escape_debug());
    assert!(output.stderr.is_empty(), "{}", diagnostics.escape_debug());
    Some(output.stdout)
}

#[test]
fn nushell_initial_cwd_is_conditional_and_not_duplicated_on_reentry() {
    for enabled in [false, true] {
        let script = format!(
            // Separate evaluations allow the helper to be defined again,
            // while retaining the environment changes from the first run.
            "$env.config.shell_integration.osc7 = {enabled}\ndo --env {{\n{NUSHELL_INIT}\n}}\ndo --env {{\n{NUSHELL_INIT}\n}}\n"
        );
        let Some(bytes) = run(&script, "") else {
            return;
        };
        if enabled {
            assert!(
                bytes.is_empty(),
                "native startup reporting must not be duplicated"
            );
        } else {
            assert_eq!(
                bytes
                    .windows(4)
                    .filter(|window| *window == b"\x1b]7;")
                    .count(),
                1
            );
            let mut terminal = Terminal::new(80, 24);
            terminal.process(&bytes);
            assert_eq!(
                terminal.reported_working_directory().unwrap().path(),
                std::env::current_dir().unwrap().to_str().unwrap()
            );
        }
    }
}

#[test]
fn nushell_initial_cwd_encodes_controls_without_injecting_commands() {
    let Some(bytes) = report("/tmp/control\x07\x1b]2;INJECTED_TITLE\x07suffix") else {
        return;
    };
    let mut terminal = Terminal::new(80, 24);
    terminal.process(&bytes);
    assert!(
        !terminal
            .drain_events()
            .iter()
            .any(|event| matches!(event, TerminalEvent::TitleChanged(_)))
    );
    assert!(terminal.reported_working_directory().is_none());
    assert!(bytes.windows(3).any(|window| window == b"%07"));
    assert!(bytes.windows(3).any(|window| window == b"%1B"));
}

#[test]
fn nushell_initial_cwd_round_trips_percent_utf8_and_uri_punctuation() {
    let path = "/tmp/space %2F #?;quote'\"/界/é/👩\u{200d}💻";
    let Some(bytes) = report(path) else {
        return;
    };
    assert!(bytes.is_ascii());
    let mut terminal = Terminal::new(80, 24);
    terminal.process(&bytes);
    assert_eq!(terminal.reported_working_directory().unwrap().path(), path);
    assert!(terminal.drain_events().is_empty());
}

#[test]
fn nushell_initial_cwd_bounds_complete_reports() {
    for (path, accepted) in [
        (String::from("/"), true),
        (String::from("/tmp/plain"), true),
        (format!("/{}", "a".repeat(4088)), true),
        (format!("/{}", "a".repeat(4089)), false),
        (format!("/aa{}", "%".repeat(1362)), true),
        (format!("/aaa{}", "%".repeat(1362)), false),
        (format!("/aa{}", "界".repeat(454)), true),
        (format!("/aaa{}", "界".repeat(454)), false),
        (String::new(), false),
        (String::from("relative"), false),
    ] {
        let Some(bytes) = report(&path) else {
            return;
        };
        let mut terminal = Terminal::new(80, 24);
        terminal.process(&bytes);
        if accepted {
            assert!(bytes.starts_with(b"\x1b]7;file://"));
            assert!(bytes.ends_with(b"\x1b\\"));
            assert_eq!(terminal.reported_working_directory().unwrap().path(), path);
        } else {
            assert!(
                bytes.is_empty(),
                "oversized/invalid reports must be omitted whole"
            );
            assert!(terminal.reported_working_directory().is_none());
        }
    }
}
