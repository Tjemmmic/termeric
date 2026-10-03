use super::cwd_report_checks::*;
use super::*;
use std::ffi::OsStr;
use std::process::Command;

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
    let Some(bytes) = report(INJECTION_PATH, "") else {
        return;
    };
    assert_cannot_inject(&bytes);
}

#[test]
fn zsh_cwd_round_trips_utf8_under_different_indexing_and_multibyte_options() {
    for options in [
        "",
        "setopt ksharrays",
        "unsetopt multibyte",
        "setopt shwordsplit",
    ] {
        let Some(bytes) = report(ROUND_TRIP_PATH, options) else {
            return;
        };
        assert_round_trips(&bytes, ROUND_TRIP_PATH);
    }
}

#[test]
fn zsh_cwd_encodes_every_nonzero_byte() {
    use std::os::unix::ffi::OsStrExt;
    let path = every_byte_path();
    let Some(bytes) = report(OsStr::from_bytes(&path), "") else {
        return;
    };
    assert_every_byte_encoded(&bytes, &path, PROMPT_MARKS);
}

#[test]
fn zsh_cwd_bounds_reports_without_truncation_and_keeps_prompt_markers() {
    for (path, accepted) in bound_cases() {
        let Some(bytes) = report(&path, "") else {
            return;
        };
        assert_bounded(&bytes, &path, accepted, PROMPT_MARKS);
    }
}
