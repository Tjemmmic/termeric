use super::cwd_report_checks::*;
use super::*;
use std::ffi::OsStr;
use std::process::Command;

const AFTER_REPORT: &[u8] = b"<after-report>";

fn report(cwd: &Path, synthetic_path: Option<&OsStr>) -> Option<Vec<u8>> {
    let program = super::tests::test_shell("fish")?;
    let home = tempfile::tempdir().unwrap();
    let invocation = if synthetic_path.is_some() {
        "__termeric_report_cwd \"$TEST_DIRECTORY\"; printf '<after-report>'"
    } else {
        // Use the actual event subscription, not just a call to the function.
        "function __termeric_test_after --on-event fish_prompt; printf '<after-report>'; end; emit fish_prompt"
    };
    let script = format!(
        "{FISH_INIT}\nfunctions --query __termeric_prompt; printf '%s\\n' $status\n{invocation}\ntest \"$LC_ALL\" = C.UTF-8\n"
    );
    let output = Command::new(program)
        .args(["--no-config", "-c", &script])
        .env_clear()
        .env("HOME", home.path())
        .env("LC_ALL", "C.UTF-8")
        .env("TEST_DIRECTORY", synthetic_path.unwrap_or_default())
        .current_dir(cwd)
        .output()
        .unwrap();
    assert!(output.status.success(), "{:?}", output.stderr);
    assert!(output.stderr.is_empty(), "{:?}", output.stderr);
    let mut parts = output.stdout.splitn(2, |byte| *byte == b'\n');
    let legacy = parts.next().unwrap();
    let bytes = parts.next().unwrap();
    assert!(legacy == b"0" || legacy == b"1");
    let mut bytes = bytes.to_vec();
    if legacy == b"0" && synthetic_path.is_none() {
        // Fish 3 uses our OSC 133 hook; Fish 4's native producer is outside
        // this non-interactive event test. Require the legacy hook to survive
        // encoding, then remove its verified suffix for byte-exact cwd checks.
        let suffix = [PROMPT_MARKS, AFTER_REPORT].concat();
        assert!(bytes.ends_with(&suffix), "{:?}", bytes);
        bytes.truncate(bytes.len() - suffix.len());
        bytes.extend_from_slice(AFTER_REPORT);
    }
    Some(bytes)
}

#[test]
fn fish_cwd_prompt_event_cannot_inject_terminal_commands() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp
        .path()
        .join(INJECTION_PATH.strip_prefix("/tmp/").unwrap());
    std::fs::create_dir(&path).unwrap();
    let Some(bytes) = report(&path, None) else {
        return;
    };
    assert!(bytes.ends_with(AFTER_REPORT));
    assert_cannot_inject(&bytes);
}

#[test]
fn fish_cwd_prompt_event_round_trips_utf8_percent_and_uri_punctuation() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp
        .path()
        .join(ROUND_TRIP_PATH.strip_prefix("/tmp/").unwrap());
    std::fs::create_dir_all(&path).unwrap();
    let Some(bytes) = report(&path, None) else {
        return;
    };
    assert!(bytes.ends_with(AFTER_REPORT));
    assert_round_trips(&bytes, path.to_str().unwrap());
}

#[test]
fn fish_cwd_prompt_event_encodes_every_nonzero_byte() {
    use std::os::unix::ffi::{OsStrExt, OsStringExt};
    let temp = tempfile::tempdir().unwrap();
    // The slash byte splits this into components shorter than NAME_MAX.
    let path = temp
        .path()
        .join(std::ffi::OsString::from_vec((1..=255).collect()));
    std::fs::create_dir_all(&path).unwrap();
    let Some(bytes) = report(&path, None) else {
        return;
    };
    assert_every_byte_encoded(&bytes, path.as_os_str().as_bytes(), AFTER_REPORT);
}

#[test]
fn fish_cwd_oversized_prompt_report_does_not_stop_other_handlers() {
    let temp = tempfile::tempdir().unwrap();
    let mut path = temp.path().to_path_buf();
    for _ in 0..12 {
        path.push("界".repeat(50));
    }
    std::fs::create_dir_all(&path).unwrap();
    let Some(bytes) = report(&path, None) else {
        return;
    };
    // The real path fits the filesystem limit but exceeds the encoded URI
    // cap. Fish 3's prompt markers are checked by report() before removal.
    assert_eq!(bytes, AFTER_REPORT);
}

#[test]
fn fish_cwd_bounds_encoded_reports_without_truncation() {
    let temp = tempfile::tempdir().unwrap();
    for (path, accepted) in bound_cases() {
        let Some(bytes) = report(temp.path(), Some(OsStr::new(&path))) else {
            return;
        };
        assert_bounded(&bytes, &path, accepted, AFTER_REPORT);
    }
}
