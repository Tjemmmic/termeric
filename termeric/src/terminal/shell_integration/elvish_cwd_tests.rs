use super::cwd_report_checks::*;
use super::*;
use std::ffi::OsStr;
use std::process::Command;

fn report(path: impl AsRef<OsStr>, before_readline: bool) -> Option<Vec<u8>> {
    let program = super::tests::test_shell("elvish")?;
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
        let Some(bytes) = report(INJECTION_PATH, before_readline) else {
            return;
        };
        assert_cannot_inject(&bytes);
    }
}

#[test]
fn elvish_cwd_round_trips_utf8_percent_and_uri_punctuation() {
    for before_readline in [false, true] {
        let Some(bytes) = report(ROUND_TRIP_PATH, before_readline) else {
            return;
        };
        assert_round_trips(&bytes, ROUND_TRIP_PATH);
    }
}

#[test]
fn elvish_cwd_encodes_every_nonzero_byte() {
    use std::os::unix::ffi::OsStrExt;
    let path = every_byte_path();
    let Some(bytes) = report(OsStr::from_bytes(&path), true) else {
        return;
    };
    assert_every_byte_encoded(&bytes, &path, PROMPT_MARKS);
}

#[test]
fn elvish_cwd_bounds_reports_without_truncation_or_losing_prompt_marks() {
    for (path, accepted) in bound_cases() {
        for before_readline in [false, true] {
            let Some(bytes) = report(&path, before_readline) else {
                return;
            };
            // Only the prompt hook adds marks; the chdir hook reports alone.
            let marks = if before_readline { PROMPT_MARKS } else { b"" };
            assert_bounded(&bytes, &path, accepted, marks);
        }
    }
}
