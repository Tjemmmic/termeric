//! Opt-in, platform-native PTY lifecycle acceptance.
//!
//! The same test binary is its own deterministic child fixture. This avoids
//! depending on a particular shell while still crossing the real Unix PTY or
//! Windows ConPTY boundary used by Termeric.

use std::io::{self, Write};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use termeric_core::pty::{Pty, PtyEvent, PtySize, spawn_reader_thread};

const REQUIRE_SMOKE: &str = "TERMERIC_REQUIRE_NATIVE_PTY_SMOKE";
const CHILD_FIXTURE: &str = "TERMERIC_NATIVE_PTY_SMOKE_CHILD";
const READY_MARKER: &str = "TERMERIC_PTY_READY";
const INPUT_MARKER: &str = "termeric-round-trip";
const ECHO_MARKER: &str = "TERMERIC_PTY_ECHO:termeric-round-trip";

fn run_child_fixture() {
    println!("{READY_MARKER}");
    io::stdout().flush().expect("flush PTY readiness marker");

    let mut input = String::new();
    io::stdin()
        .read_line(&mut input)
        .expect("read parent input through native PTY");
    let input = input.trim_end_matches(&['\r', '\n'][..]);
    println!("TERMERIC_PTY_ECHO:{input}");
    io::stdout().flush().expect("flush PTY echo marker");
}

fn receive_until(
    receiver: &mpsc::Receiver<PtyEvent>,
    output: &mut Vec<u8>,
    marker: &str,
    deadline: Instant,
) -> Result<(), String> {
    while Instant::now() < deadline {
        match receiver.recv_timeout(Duration::from_millis(50)) {
            Ok(PtyEvent::Output(bytes)) => {
                output.extend(bytes);
                if String::from_utf8_lossy(output).contains(marker) {
                    return Ok(());
                }
            }
            Ok(PtyEvent::Closed) | Err(mpsc::RecvTimeoutError::Disconnected) => break,
            Ok(PtyEvent::Error(error)) => {
                // Unix PTY masters commonly return EIO when the slave exits.
                // Process state below remains the authoritative exit check.
                if String::from_utf8_lossy(output).contains(marker) {
                    return Ok(());
                }
                return Err(format!("PTY reader failed before {marker:?}: {error}"));
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
        }
    }

    Err(format!(
        "timed out waiting for {marker:?}; output={:?}",
        String::from_utf8_lossy(output)
    ))
}

#[test]
fn native_pty_round_trip_when_requested() {
    if std::env::var(CHILD_FIXTURE).as_deref() == Ok("1") {
        run_child_fixture();
        return;
    }

    if std::env::var(REQUIRE_SMOKE).as_deref() != Ok("1") {
        eprintln!("skipping native PTY smoke: set {REQUIRE_SMOKE}=1");
        return;
    }

    let executable = std::env::current_exe().expect("resolve native PTY fixture executable");
    let executable = executable
        .to_str()
        .expect("native PTY fixture executable path must be valid UTF-8");
    let arguments = vec![
        "--exact".to_string(),
        "native_pty_round_trip_when_requested".to_string(),
        "--nocapture".to_string(),
    ];
    let environment = vec![(CHILD_FIXTURE.to_string(), "1".to_string())];
    let initial = PtySize {
        cols: 83,
        rows: 27,
        pixel_width: 913,
        pixel_height: 513,
    };
    let pty = Pty::new_with_env_and_size(initial, Some(executable), &arguments, &environment)
        .expect("spawn fixture through the platform-native PTY");
    assert_eq!(pty.size().expect("read initial native PTY size"), initial);

    let (mut handle, reader) = pty.take_reader();
    let receiver = spawn_reader_thread(reader);
    let mut output = Vec::new();
    receive_until(
        &receiver,
        &mut output,
        READY_MARKER,
        Instant::now() + Duration::from_secs(10),
    )
    .expect("fixture should become ready");

    let resized = PtySize {
        cols: 47,
        rows: 19,
        pixel_width: 611,
        pixel_height: 437,
    };
    handle.resize(resized).expect("resize native PTY");
    assert_eq!(
        handle.size().expect("read resized native PTY size"),
        resized
    );

    handle
        .write_all(format!("{INPUT_MARKER}\r").as_bytes())
        .expect("write input through native PTY");
    receive_until(
        &receiver,
        &mut output,
        ECHO_MARKER,
        Instant::now() + Duration::from_secs(10),
    )
    .expect("fixture should echo native PTY input");

    let deadline = Instant::now() + Duration::from_secs(10);
    while handle.is_running() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(
        !handle.is_running(),
        "native PTY child did not exit; output={:?}",
        String::from_utf8_lossy(&output)
    );
}
