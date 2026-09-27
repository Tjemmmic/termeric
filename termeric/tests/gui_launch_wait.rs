//! Provider-free executable acceptance against a private fake GUI endpoint.
#![cfg(unix)]

use serde_json::{Value, json};
use std::fs;
use std::io::{Read, Write};
use std::os::unix::{
    fs::PermissionsExt,
    net::{UnixListener, UnixStream},
};
use std::path::PathBuf;
use std::process::{Command, Output, Stdio};
use std::time::{Duration, Instant};

const ID: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const TOKEN: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
const VERSION: u32 = 24;

struct Fixture {
    root: tempfile::TempDir,
    listener: UnixListener,
    token: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        fs::set_permissions(root.path(), fs::Permissions::from_mode(0o700)).unwrap();
        let directory = root.path().join("termeric");
        fs::create_dir(&directory).unwrap();
        fs::set_permissions(&directory, fs::Permissions::from_mode(0o700)).unwrap();
        let stem = format!("gui-v1-{}", std::process::id());
        let socket = directory.join(format!("{stem}.sock"));
        let token = directory.join(format!("{stem}.token"));
        let listener = UnixListener::bind(&socket).unwrap();
        listener.set_nonblocking(true).unwrap();
        fs::set_permissions(socket, fs::Permissions::from_mode(0o600)).unwrap();
        fs::write(&token, TOKEN).unwrap();
        fs::set_permissions(&token, fs::Permissions::from_mode(0o600)).unwrap();
        Self {
            root,
            listener,
            token,
        }
    }

    fn run(&self, verb: &str, options: &[&str], serve: impl FnOnce(&Self) + Send) -> Output {
        self.run_with_input(verb, options, Stdio::null(), serve)
    }

    fn run_with_input(
        &self,
        verb: &str,
        options: &[&str],
        input: Stdio,
        serve: impl FnOnce(&Self) + Send,
    ) -> Output {
        std::thread::scope(|scope| {
            let server = scope.spawn(|| serve(self));
            let mut child = Command::new(env!("CARGO_BIN_EXE_termeric"))
                .args([
                    "control",
                    verb,
                    &std::process::id().to_string(),
                    if matches!(verb, "launch-tab" | "launch-window") {
                        "7"
                    } else {
                        ID
                    },
                ])
                .args(options)
                .env("XDG_RUNTIME_DIR", self.root.path())
                .env_remove("DISPLAY")
                .env_remove("WAYLAND_DISPLAY")
                .stdin(input)
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .unwrap();
            let deadline = Instant::now() + Duration::from_secs(8);
            loop {
                if child.try_wait().unwrap().is_some() {
                    break;
                }
                if Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    panic!("wait-launch executable exceeded fixture deadline");
                }
                std::thread::sleep(Duration::from_millis(5));
            }
            let output = child.wait_with_output().unwrap();
            server.join().unwrap();
            output
        })
    }

    fn request(&self) -> UnixStream {
        let (stream, request) = self.read_request();
        assert_eq!(
            request,
            json!({"version": VERSION, "token": TOKEN,
            "request": {"operation": "launch_status", "launch_request_id": ID}})
        );
        stream
    }

    fn read_request(&self) -> (UnixStream, Value) {
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut stream = loop {
            match self.listener.accept() {
                Ok((stream, _)) => break stream,
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    assert!(Instant::now() < deadline, "CLI did not query endpoint");
                    std::thread::sleep(Duration::from_millis(5));
                }
                Err(error) => panic!("accept failed: {error}"),
            }
        };
        stream
            .set_read_timeout(Some(Duration::from_secs(3)))
            .unwrap();
        stream
            .set_write_timeout(Some(Duration::from_secs(3)))
            .unwrap();
        let mut size = [0; 4];
        stream.read_exact(&mut size).unwrap();
        let size = u32::from_be_bytes(size) as usize;
        assert!(size < 4096);
        let mut payload = vec![0; size];
        stream.read_exact(&mut payload).unwrap();
        let request: Value = serde_json::from_slice(&payload).unwrap();
        (stream, request)
    }

    fn no_more_requests(&self) {
        assert!(
            matches!(self.listener.accept(), Err(error) if error.kind() == std::io::ErrorKind::WouldBlock)
        );
    }
}

fn acknowledgement(verb: &str, id: &str) -> Value {
    json!({"result": if verb == "launch-tab" { "tab_created" } else { "window_launched" },
        "version": VERSION, "process_id": std::process::id(), "request_id": id,
        "window_id": if verb == "launch-tab" { 7 } else { 8 }, "tab_id": 3})
}

#[cfg(target_os = "linux")]
fn input_file(bytes: &[u8]) -> Stdio {
    use std::io::{Seek, SeekFrom};
    let mut file = tempfile::tempfile().unwrap();
    file.write_all(bytes).unwrap();
    file.seek(SeekFrom::Start(0)).unwrap();
    file.into()
}

#[cfg(target_os = "linux")]
fn input_pipe(bytes: &[u8]) -> (std::fs::File, std::fs::File) {
    use std::os::fd::FromRawFd;
    let mut fds = [-1; 2];
    // SAFETY: two output descriptors are initialized by pipe2.
    assert_eq!(unsafe { libc::pipe2(fds.as_mut_ptr(), libc::O_CLOEXEC) }, 0);
    // SAFETY: each newly created descriptor is adopted exactly once.
    let (reader, mut writer) = unsafe {
        (
            std::fs::File::from_raw_fd(fds[0]),
            std::fs::File::from_raw_fd(fds[1]),
        )
    };
    assert!(bytes.len() < 4096);
    writer.write_all(bytes).unwrap();
    (reader, writer)
}

#[cfg(target_os = "linux")]
#[test]
fn executable_stdin_environment_reaches_both_launch_targets_without_argv_or_output_exposure() {
    use std::os::fd::AsRawFd;
    for verb in ["launch-tab", "launch-window"] {
        for wait in [false, true] {
            for pipe in [false, true] {
                let fixture = Fixture::new();
                let secret = "synthetic-private-not-a-real-credential";
                let bytes = serde_json::to_vec(
                    &json!({"PRIVATE_VALUE": secret, "EMPTY": "", "EXACT": "= $literal\n\t☺"}),
                )
                .unwrap();
                let input = if pipe {
                    let (reader, writer) = input_pipe(&bytes);
                    drop(writer);
                    reader.into()
                } else {
                    input_file(&bytes)
                };
                let mut options = vec![
                    "--env-stdin",
                    "--env",
                    "LITERAL=ok",
                    "--clear-env",
                    "--json",
                    "--request-id",
                    ID,
                ];
                if verb == "launch-tab" {
                    options.push("--background");
                }
                if wait {
                    options.extend(["--wait", "--timeout", "2"]);
                }
                options.extend(["--", "/bin/true", "--env-stdin"]);
                let output = fixture.run_with_input(verb, &options, input, |fixture| {
                    let (stream, request) = fixture.read_request();
                    assert_launch(&request, verb);
                    assert_eq!(request["request"]["args"], json!(["--env-stdin"]));
                    if verb == "launch-tab" {
                        assert_eq!(request["request"]["background"], true);
                    }
                    let entries = request["request"]["environment"].as_array().unwrap();
                    assert_eq!(entries.len(), 4);
                    assert!(entries.contains(&json!({"name": "PRIVATE_VALUE", "value": secret})));
                    assert!(
                        entries.contains(&json!({"name": "EXACT", "value": "= $literal\n\t☺"}))
                    );
                    assert!(entries.contains(&json!({"name": "EMPTY", "value": ""})));
                    assert!(entries.contains(&json!({"name": "LITERAL", "value": "ok"})));
                    assert_eq!(request["request"]["environment_policy"]["clear"], true);
                    // Inspect the live CLI peer while it awaits our acknowledgement.
                    // SAFETY: getsockopt writes a bounded ucred record for this live socket.
                    let mut credentials: libc::ucred = unsafe { std::mem::zeroed() };
                    let mut length = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
                    assert_eq!(
                        unsafe {
                            libc::getsockopt(
                                stream.as_raw_fd(),
                                libc::SOL_SOCKET,
                                libc::SO_PEERCRED,
                                (&mut credentials as *mut libc::ucred).cast(),
                                &mut length,
                            )
                        },
                        0
                    );
                    let argv = fs::read(format!("/proc/{}/cmdline", credentials.pid)).unwrap();
                    assert!(
                        !argv
                            .windows(secret.len())
                            .any(|bytes| bytes == secret.as_bytes())
                    );
                    send(stream, acknowledgement(verb, ID));
                    if wait {
                        send(
                            fixture.request(),
                            response(json!({"status": "exited", "exit_code": 0, "success": true})),
                        );
                    }
                });
                assert!(
                    output.status.success(),
                    "{}",
                    String::from_utf8_lossy(&output.stderr)
                );
                assert!(!String::from_utf8_lossy(&output.stdout).contains(secret));
                assert!(!String::from_utf8_lossy(&output.stderr).contains(secret));
                fixture.no_more_requests();
            }
        }
    }
}

#[cfg(target_os = "linux")]
#[test]
fn executable_invalid_stdin_environment_is_redacted_and_never_sent() {
    let marker = "synthetic-private";
    let mut documents = vec![
        br#"{"A":"synthetic-private","A":"second"}"#.to_vec(),
        br#"{"A":123,"synthetic-private":"x"}"#.to_vec(),
        br#"{"TERM":"synthetic-private"}"#.to_vec(),
        br#"{"BAD-NAME":"synthetic-private"}"#.to_vec(),
        br#"{"A":"synthetic-private\u0000"}"#.to_vec(),
        b"{}synthetic-private".to_vec(),
        b"{\"A\":\"\xffsynthetic-private\"}".to_vec(),
    ];
    documents.push(format!("{{\"A\":\"{marker}{}\"}}", "x".repeat(8192)).into_bytes());
    documents.push(vec![b'x'; 256 * 1024 + 1]);
    for verb in ["launch-tab", "launch-window"] {
        for document in &documents {
            let fixture = Fixture::new();
            let output = fixture.run_with_input(
                verb,
                &["--env-stdin", "--wait", "--json", "--", "/bin/true"],
                input_file(document),
                |_| {},
            );
            assert!(!output.status.success());
            assert!(output.stdout.is_empty());
            assert!(!String::from_utf8_lossy(&output.stderr).contains(marker));
            fixture.no_more_requests();
        }
        for flags in [
            vec!["--env-stdin", "--env-stdin"],
            vec!["--env-stdin", "--env", "A=literal"],
            vec!["--env-stdin", "--unset-env", "A"],
        ] {
            let fixture = Fixture::new();
            let mut options = flags;
            options.extend(["--", "/bin/true"]);
            let output = fixture.run_with_input(
                verb,
                &options,
                input_file(br#"{"A":"synthetic-private"}"#),
                |_| {},
            );
            assert!(!output.status.success());
            assert!(output.stdout.is_empty());
            assert!(!String::from_utf8_lossy(&output.stderr).contains(marker));
            fixture.no_more_requests();
        }
    }
}

#[cfg(target_os = "linux")]
#[test]
fn executable_stdin_environment_requires_eof_and_times_out_before_contact() {
    let fixture = Fixture::new();
    let (reader, _writer) = input_pipe(b"{}");
    let start = Instant::now();
    let output = fixture.run_with_input(
        "launch-tab",
        &["--env-stdin", "--wait", "--timeout", "1", "--", "/bin/true"],
        reader.into(),
        |_| {},
    );
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    assert!(String::from_utf8_lossy(&output.stderr).contains("input timed out before launch"));
    assert!(start.elapsed() >= Duration::from_secs(5));
    assert!(start.elapsed() < Duration::from_secs(7));
    fixture.no_more_requests();
}

fn assert_launch(request: &Value, verb: &str) {
    assert_eq!(request["version"], VERSION);
    assert_eq!(request["token"], TOKEN);
    assert_eq!(request["request"]["operation"], verb.replace('-', "_"));
    let target = if verb == "launch-tab" {
        "window_id"
    } else {
        "source_window"
    };
    assert_eq!(request["request"][target], 7);
    assert_eq!(request["request"]["program"], "/bin/true");
    if verb == "launch-tab" {
        assert!(request["request"]["background"].is_boolean());
    }
}

#[test]
fn executable_background_tab_launch_preserves_wire_mode_and_wait_behavior() {
    for wait in [false, true] {
        let fixture = Fixture::new();
        let mut options = vec!["--background", "--json", "--request-id", ID];
        if wait {
            options.extend(["--wait", "--timeout", "2"]);
        }
        options.extend(["--", "/bin/true", "--background"]);
        let output = fixture.run("launch-tab", &options, |fixture| {
            let (stream, request) = fixture.read_request();
            assert_launch(&request, "launch-tab");
            assert_eq!(request["request"]["background"], true);
            assert_eq!(request["request"]["args"], json!(["--background"]));
            send(stream, acknowledgement("launch-tab", ID));
            if wait {
                send(
                    fixture.request(),
                    response(json!({"status": "exited", "exit_code": 7, "success": false})),
                );
            }
        });
        assert_eq!(output.status.code(), Some(if wait { 7 } else { 0 }));
        let result: Value = serde_json::from_slice(&output.stdout).unwrap();
        if wait {
            assert_eq!(result["state"]["exit_code"], 7);
        } else {
            assert_eq!(result["tab_id"], 3);
        }
        fixture.no_more_requests();
    }
    for (verb, flags) in [
        ("launch-tab", vec!["--background", "--background"]),
        ("launch-window", vec!["--background"]),
    ] {
        let fixture = Fixture::new();
        let mut options = flags;
        options.extend(["--", "/bin/true"]);
        let output = fixture.run(verb, &options, |_| {});
        assert!(!output.status.success());
        fixture.no_more_requests();
    }
}

#[test]
fn executable_combined_launch_wait_preserves_exit_and_final_only_output() {
    for verb in ["launch-tab", "launch-window"] {
        for code in [0, 7, 124, 255, 256] {
            let fixture = Fixture::new();
            let output = fixture.run(
                verb,
                &[
                    "--wait",
                    "--timeout",
                    "3",
                    "--json",
                    "--request-id",
                    ID,
                    "--env",
                    "MODE=two words",
                    "--",
                    "/bin/true",
                    "--wait",
                ],
                |fixture| {
                    let (stream, request) = fixture.read_request();
                    assert_launch(&request, verb);
                    assert_eq!(request["request"]["request_id"], ID);
                    assert_eq!(request["request"]["args"], json!(["--wait"]));
                    assert_eq!(
                        request["request"]["environment"],
                        json!([{"name": "MODE", "value": "two words"}])
                    );
                    send(stream, acknowledgement(verb, ID));
                    if code != 0 {
                        send(fixture.request(), response(json!({"status": "running"})));
                    }
                    send(
                        fixture.request(),
                        response(
                            json!({"status": "exited", "exit_code": code, "success": code == 0}),
                        ),
                    );
                },
            );
            assert_eq!(
                output.status.code(),
                Some(if code > 255 { 1 } else { code }),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
            assert!(output.stderr.is_empty());
            let value: Value = serde_json::from_slice(&output.stdout).unwrap();
            assert_eq!(value["launch_request_id"], ID);
            assert_eq!(value["state"]["exit_code"], code);
            fixture.no_more_requests();
        }
    }
}

#[test]
fn executable_combined_wait_lost_ack_keeps_generated_recovery_id_without_retry() {
    for verb in ["launch-tab", "launch-window"] {
        let fixture = Fixture::new();
        let captured = std::sync::Mutex::new(String::new());
        let output = fixture.run(verb, &["--wait", "--json", "--", "/bin/true"], |fixture| {
            let (stream, request) = fixture.read_request();
            assert_launch(&request, verb);
            *captured.lock().unwrap() = request["request"]["request_id"]
                .as_str()
                .unwrap()
                .to_owned();
            drop(stream);
        });
        let id = captured.into_inner().unwrap();
        assert_eq!(id.len(), 32);
        assert!(
            id.bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        );
        assert_ne!(id, "0".repeat(32));
        assert!(!output.status.success());
        assert!(output.stdout.is_empty());
        let error = String::from_utf8_lossy(&output.stderr);
        assert!(error.contains(&id));
        assert!(error.contains("control launch-status"));
        assert!(error.contains("control wait-launch"));
        assert!(!error.contains(TOKEN));
        fixture.no_more_requests();
    }
}

#[test]
fn executable_combined_wait_preserves_generated_id_through_success() {
    let fixture = Fixture::new();
    let output = fixture.run(
        "launch-tab",
        &["--wait", "--json", "--", "/bin/true"],
        |fixture| {
            let (stream, request) = fixture.read_request();
            assert_launch(&request, "launch-tab");
            let id = request["request"]["request_id"].as_str().unwrap();
            assert_eq!(id.len(), 32);
            send(stream, acknowledgement("launch-tab", id));
            let (stream, status) = fixture.read_request();
            assert_eq!(
                status["request"],
                json!({"operation": "launch_status", "launch_request_id": id})
            );
            let mut frame = response(json!({"status": "exited", "exit_code": 0, "success": true}));
            frame["launch_request_id"] = json!(id);
            send(stream, frame);
        },
    );
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.stderr.is_empty());
    let result: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(result["state"]["exit_code"], 0);
    assert_ne!(result["launch_request_id"], "0".repeat(32));
    fixture.no_more_requests();
}

#[test]
fn executable_combined_wait_invalid_options_never_contact_endpoint() {
    for verb in ["launch-tab", "launch-window"] {
        for flags in [
            vec!["--wait", "--wait"],
            vec!["--timeout", "1"],
            vec!["--wait", "--timeout", "0"],
            vec!["--wait", "--request-id", "invalid"],
        ] {
            let fixture = Fixture::new();
            let mut options = flags;
            options.extend(["--", "/bin/true"]);
            let output = fixture.run(verb, &options, |_| {});
            assert!(!output.status.success());
            assert!(output.stdout.is_empty());
            fixture.no_more_requests();
        }
    }
}

#[test]
fn executable_combined_wait_refuses_wrong_acknowledgement_before_polling() {
    for verb in ["launch-tab", "launch-window"] {
        let ack = acknowledgement(verb, ID);
        let mut cases = Vec::new();
        for (key, value) in [
            ("version", json!(VERSION - 1)),
            ("process_id", json!(0)),
            ("request_id", json!("c".repeat(32))),
            ("window_id", json!(0)),
            ("result", json!("inventory")),
        ] {
            let mut wrong = ack.clone();
            wrong[key] = value;
            cases.push(wrong);
        }
        cases.push(json!({"result": "error", "version": VERSION, "message": "launch refused"}));
        for frame in cases {
            let fixture = Fixture::new();
            let output = fixture.run(
                verb,
                &["--wait", "--json", "--request-id", ID, "--", "/bin/true"],
                |fixture| {
                    let (stream, request) = fixture.read_request();
                    assert_launch(&request, verb);
                    send(stream, frame);
                },
            );
            assert!(!output.status.success());
            assert!(output.stdout.is_empty());
            assert!(String::from_utf8_lossy(&output.stderr).contains(ID));
            fixture.no_more_requests();
        }
    }
}

#[test]
fn executable_combined_deadline_covers_launch_and_observation_without_cancellation() {
    for during_launch in [true, false] {
        let fixture = Fixture::new();
        let started = Instant::now();
        let output = fixture.run(
            "launch-tab",
            &[
                "--wait",
                "--timeout",
                "1",
                "--json",
                "--request-id",
                ID,
                "--",
                "/bin/true",
            ],
            |fixture| {
                let (mut stream, request) = fixture.read_request();
                assert_launch(&request, "launch-tab");
                if !during_launch {
                    // Time consumed by launch must reduce the observation budget.
                    std::thread::sleep(Duration::from_millis(600));
                    send(stream, acknowledgement("launch-tab", ID));
                    stream = fixture.request();
                }
                stream.write_all(&[0, 0]).unwrap();
                assert_eq!(stream.read(&mut [0]).unwrap(), 0);
            },
        );
        assert!(!output.status.success());
        assert!(output.stdout.is_empty());
        let error = String::from_utf8_lossy(&output.stderr);
        assert!(error.contains("timed out"));
        assert!(error.contains(ID));
        assert!(started.elapsed() >= Duration::from_secs(1));
        assert!(started.elapsed() < Duration::from_millis(1500));
        fixture.no_more_requests();
    }
}

#[test]
fn executable_combined_wait_pins_endpoint_across_launch_acknowledgement() {
    for replace_socket in [false, true] {
        let fixture = Fixture::new();
        let output = fixture.run("launch-window", &["--wait", "--json", "--request-id", ID, "--", "/bin/true"], |fixture| {
            let (stream, request) = fixture.read_request();
            assert_launch(&request, "launch-window");
            let replacement = if replace_socket {
                let socket = fixture.token.with_extension("sock");
                fs::rename(&socket, fixture.root.path().join("original.sock")).unwrap();
                let replacement = UnixListener::bind(&socket).unwrap();
                replacement.set_nonblocking(true).unwrap();
                fs::set_permissions(socket, fs::Permissions::from_mode(0o600)).unwrap();
                Some(replacement)
            } else {
                fs::write(&fixture.token, "c".repeat(64)).unwrap();
                None
            };
            send(stream, acknowledgement("launch-window", ID));
            std::thread::sleep(Duration::from_millis(300));
            if let Some(listener) = replacement {
                assert!(matches!(listener.accept(), Err(error) if error.kind() == std::io::ErrorKind::WouldBlock));
            }
        });
        assert!(!output.status.success());
        assert!(output.stdout.is_empty());
        assert!(String::from_utf8_lossy(&output.stderr).contains("endpoint changed"));
        fixture.no_more_requests();
    }
}

#[test]
fn executable_non_wait_launches_keep_their_original_output() {
    for verb in ["launch-tab", "launch-window"] {
        let fixture = Fixture::new();
        let output = fixture.run(
            verb,
            &["--json", "--request-id", ID, "--", "/bin/true"],
            |fixture| {
                let (stream, request) = fixture.read_request();
                assert_launch(&request, verb);
                send(stream, acknowledgement(verb, ID));
            },
        );
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let value: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(value["request_id"], ID);
        assert_eq!(value["tab_id"], 3);
        assert!(value.get("state").is_none());
        fixture.no_more_requests();
    }
}

fn response(state: Value) -> Value {
    json!({"result": "launch_status", "version": VERSION, "process_id": std::process::id(),
        "launch_request_id": ID, "state": state, "terminal_closed": false})
}

fn send(mut stream: UnixStream, frame: Value) {
    let bytes = serde_json::to_vec(&frame).unwrap();
    stream
        .write_all(&(bytes.len() as u32).to_be_bytes())
        .unwrap();
    stream.write_all(&bytes).unwrap();
}

#[test]
fn executable_wait_progresses_and_propagates_portable_child_exit() {
    for code in [0, 7, 124, 255, 256] {
        let fixture = Fixture::new();
        let output = fixture.run("wait-launch", &["--json", "--timeout", "3"], |fixture| {
            send(fixture.request(), response(json!({"status": "running"})));
            let started = Instant::now();
            let next = fixture.request();
            assert!(
                started.elapsed() >= Duration::from_millis(200),
                "busy-polling GUI endpoint"
            );
            send(
                next,
                response(json!({"status": "exited", "exit_code": code, "success": code == 0})),
            );
        });
        assert_eq!(
            output.status.code(),
            Some(if code > 255 { 1 } else { code }),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(output.stderr.is_empty());
        let value: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(value["state"]["exit_code"], code);
        assert_eq!(value["schema_version"], 3);
        assert_eq!(value["terminal_closed"], false);
    }
    let fixture = Fixture::new();
    let output = fixture.run("launch-status", &["--json"], |fixture| {
        send(
            fixture.request(),
            response(json!({"status": "exited", "exit_code": 7, "success": false})),
        );
    });
    assert!(
        output.status.success(),
        "one-shot query changed its exit semantics"
    );
}

#[test]
fn executable_wait_refuses_invalid_or_unavailable_results() {
    let exited = response(json!({"status": "exited", "exit_code": 0, "success": true}));
    let mut wrong_id = exited.clone();
    wrong_id["launch_request_id"] = json!("c".repeat(32));
    let mut wrong_process = exited.clone();
    wrong_process["process_id"] = json!(0);
    let mut wrong_version = exited;
    wrong_version["version"] = json!(VERSION - 1);
    for frame in [
        wrong_id,
        wrong_process,
        wrong_version,
        response(json!({"status": "exited", "exit_code": 0, "success": false})),
        response(json!({"status": "running", "exit_code": 0})),
        json!({"result": "error", "version": VERSION, "message": "GUI launch result is unknown or expired"}),
    ] {
        for verb in ["wait-launch", "launch-tab", "launch-window"] {
            let fixture = Fixture::new();
            let options = if verb == "wait-launch" {
                vec!["--json", "--timeout", "2"]
            } else {
                vec![
                    "--wait",
                    "--json",
                    "--timeout",
                    "2",
                    "--request-id",
                    ID,
                    "--",
                    "/bin/true",
                ]
            };
            let output = fixture.run(verb, &options, |fixture| {
                if verb != "wait-launch" {
                    let (stream, request) = fixture.read_request();
                    assert_launch(&request, verb);
                    send(stream, acknowledgement(verb, ID));
                }
                send(fixture.request(), frame.clone());
            });
            assert!(!output.status.success());
            assert!(output.stdout.is_empty());
            assert!(!String::from_utf8_lossy(&output.stderr).contains(TOKEN));
            fixture.no_more_requests();
        }
    }
    let fixture = Fixture::new();
    let output = fixture.run("wait-launch", &["--json"], |fixture| {
        drop(fixture.request());
    });
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
}

#[test]
fn executable_deadline_interrupts_partial_response_without_mutation() {
    let fixture = Fixture::new();
    let started = Instant::now();
    let output = fixture.run("wait-launch", &["--timeout", "1", "--json"], |fixture| {
        let mut stream = fixture.request();
        stream.write_all(&[0, 0]).unwrap();
        // The only further bytes must be EOF when the CLI times out, never a
        // cancellation or kill request. No GUI or real launched child exists.
        assert_eq!(stream.read(&mut [0]).unwrap(), 0);
    });
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    assert!(String::from_utf8_lossy(&output.stderr).contains("timed out"));
    assert!(started.elapsed() >= Duration::from_secs(1));
    assert!(started.elapsed() < Duration::from_secs(3));
}

#[test]
fn executable_wait_pins_credentials_between_polls() {
    let fixture = Fixture::new();
    let output = fixture.run("wait-launch", &["--json"], |fixture| {
        let stream = fixture.request();
        fs::write(&fixture.token, "c".repeat(64)).unwrap();
        send(stream, response(json!({"status": "running"})));
    });
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    assert!(String::from_utf8_lossy(&output.stderr).contains("endpoint changed"));
    assert!(
        matches!(fixture.listener.accept(), Err(error) if error.kind() == std::io::ErrorKind::WouldBlock)
    );
}

#[test]
fn executable_wait_refuses_replaced_socket_with_unchanged_credentials() {
    let fixture = Fixture::new();
    let output = fixture.run("wait-launch", &["--json"], |fixture| {
        let stream = fixture.request();
        let socket = fixture.token.with_extension("sock");
        // Keep the old inode alive so the replacement cannot reuse its identity.
        fs::rename(&socket, fixture.root.path().join("original.sock")).unwrap();
        let replacement = UnixListener::bind(&socket).unwrap();
        replacement.set_nonblocking(true).unwrap();
        fs::set_permissions(&socket, fs::Permissions::from_mode(0o600)).unwrap();
        send(stream, response(json!({"status": "running"})));
        // Keep the replacement reachable beyond the next poll, but no token or
        // request may be sent to it even though it is owned by the same user.
        std::thread::sleep(Duration::from_millis(600));
        assert!(matches!(replacement.accept(), Err(error) if error.kind() == std::io::ErrorKind::WouldBlock));
    });
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    assert!(String::from_utf8_lossy(&output.stderr).contains("endpoint changed"));
}
