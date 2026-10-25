use super::cwd_report_checks::*;
use super::*;
use std::ffi::OsStr;
use std::process::Command;

fn report(path: impl AsRef<OsStr>) -> Option<Vec<u8>> {
    let program = super::tests::test_shell("bash")?;
    let home = tempfile::tempdir().unwrap();
    let script = format!(
        "{}\nPWD=$TEST_DIRECTORY\n__termeric_after_prompt\n[[ $LC_ALL == POSIX ]]\n",
        BASH_RC
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
    let Some(bytes) = report(INJECTION_PATH) else {
        return;
    };
    assert_cannot_inject(&bytes);
}

#[test]
fn bash_cwd_encoding_round_trips_printable_utf8_and_uri_characters() {
    let Some(bytes) = report(ROUND_TRIP_PATH) else {
        return;
    };
    assert_round_trips(&bytes, ROUND_TRIP_PATH);
}

#[test]
fn bash_cwd_encodes_every_nonzero_byte_without_leaking_protocol_controls() {
    use std::os::unix::ffi::OsStrExt;
    let path = every_byte_path();
    let Some(bytes) = report(OsStr::from_bytes(&path)) else {
        return;
    };
    assert_every_byte_encoded(&bytes, &path, PROMPT_START);
}

#[test]
fn bash_cwd_bounds_encoded_reports_without_truncating_or_losing_prompt_marks() {
    for (path, accepted) in bound_cases() {
        let Some(bytes) = report(&path) else {
            return;
        };
        assert_bounded(&bytes, &path, accepted, PROMPT_START);
    }
}
