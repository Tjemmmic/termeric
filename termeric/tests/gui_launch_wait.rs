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
const VERSION: u32 = 16;

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
        std::thread::scope(|scope| {
            let server = scope.spawn(|| serve(self));
            let mut child = Command::new(env!("CARGO_BIN_EXE_termeric"))
                .args(["control", verb, &std::process::id().to_string(), ID])
                .args(options)
                .env("XDG_RUNTIME_DIR", self.root.path())
                .env_remove("DISPLAY")
                .env_remove("WAYLAND_DISPLAY")
                .stdin(Stdio::null())
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
        assert_eq!(
            request,
            json!({"version": VERSION, "token": TOKEN,
            "request": {"operation": "launch_status", "launch_request_id": ID}})
        );
        stream
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
        let fixture = Fixture::new();
        let output = fixture.run("wait-launch", &["--json", "--timeout", "2"], |fixture| {
            send(fixture.request(), frame);
        });
        assert!(!output.status.success());
        assert!(output.stdout.is_empty());
        assert!(!String::from_utf8_lossy(&output.stderr).contains(TOKEN));
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
