//! Executable bridge acceptance with a private real daemon, no SSH or providers.
#![cfg(unix)]

use serde_json::{Value, json};
use std::fs;
use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::time::{Duration, Instant};

const VERSION: u32 = 1;
const LIMIT: Duration = Duration::from_secs(5);

struct OwnedChild(Option<Child>);

impl OwnedChild {
    fn output(self) -> Output {
        self.output_with_timeout(LIMIT)
    }

    fn output_with_timeout(mut self, timeout: Duration) -> Output {
        let deadline = Instant::now() + timeout;
        loop {
            let child = self.0.as_mut().unwrap();
            if child.try_wait().unwrap().is_some() {
                return self.0.take().unwrap().wait_with_output().unwrap();
            }
            assert!(
                Instant::now() < deadline,
                "bridge fixture exceeded its deadline"
            );
            std::thread::sleep(Duration::from_millis(5));
        }
    }
}

impl Drop for OwnedChild {
    fn drop(&mut self) {
        if let Some(mut child) = self.0.take() {
            if !matches!(child.try_wait(), Ok(Some(_))) {
                let _ = child.kill();
            }
            let _ = child.wait();
        }
    }
}

struct Fixture {
    // Children are owned separately and dropped before this private directory.
    _root: tempfile::TempDir,
    runtime: PathBuf,
    config: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let runtime = root.path().join("runtime");
        let config = root.path().join("config");
        fs::create_dir(&runtime).unwrap();
        fs::create_dir(&config).unwrap();
        fs::set_permissions(&runtime, fs::Permissions::from_mode(0o700)).unwrap();
        Self {
            _root: root,
            runtime,
            config,
        }
    }

    fn command(&self, args: &[&str]) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_termeric"));
        command
            .args(args)
            .env("XDG_RUNTIME_DIR", &self.runtime)
            .env("XDG_CONFIG_HOME", &self.config)
            .env_remove("DISPLAY")
            .env_remove("WAYLAND_DISPLAY")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        command
    }

    fn server(&self) -> OwnedChild {
        let mut child = OwnedChild(Some(
            self.command(&["domain", "server", "--foreground"])
                .stdin(Stdio::null())
                .spawn()
                .unwrap(),
        ));
        let deadline = Instant::now() + LIMIT;
        let socket = self.runtime.join("termeric/domain-v1.sock");
        while !socket.exists() {
            assert!(
                child.0.as_mut().unwrap().try_wait().unwrap().is_none(),
                "private daemon exited before listening"
            );
            assert!(Instant::now() < deadline, "private daemon did not listen");
            std::thread::sleep(Duration::from_millis(5));
        }
        child
    }

    fn bridge(&self, wire: &[u8], close_input: bool) -> Output {
        let mut child = OwnedChild(Some(
            self.command(&["domain", "proxy-stdio"])
                .stdin(Stdio::piped())
                .spawn()
                .unwrap(),
        ));
        let mut input = child.0.as_mut().unwrap().stdin.take().unwrap();
        // These fixtures fit comfortably in the pipe. One write queues the
        // handshake and request together before the reader can prefetch them.
        input.write_all(wire).unwrap();
        let held_input = if close_input {
            drop(input);
            None
        } else {
            Some(input)
        };
        let output = child.output();
        drop(held_input);
        output
    }
}

fn frame(value: Value) -> Vec<u8> {
    let payload = serde_json::to_vec(&value).unwrap();
    [
        (payload.len() as u32).to_be_bytes().as_slice(),
        payload.as_slice(),
    ]
    .concat()
}

fn decode(mut wire: &[u8]) -> Vec<Value> {
    let mut frames = Vec::new();
    while !wire.is_empty() {
        assert!(wire.len() >= 4, "truncated stdout frame header");
        let length = u32::from_be_bytes(wire[..4].try_into().unwrap()) as usize;
        assert!((1..=4 * 1024 * 1024).contains(&length));
        wire = &wire[4..];
        assert!(wire.len() >= length, "truncated stdout payload");
        frames.push(serde_json::from_slice(&wire[..length]).unwrap());
        wire = &wire[length..];
    }
    frames
}

fn assert_no_token(output: &Output, token_path: &Path) {
    let token = fs::read(token_path).unwrap();
    assert!(!token.is_empty());
    for bytes in [&output.stdout, &output.stderr] {
        assert!(
            !bytes.windows(token.len()).any(|window| window == token),
            "daemon credential escaped through bridge stdio"
        );
    }
}

#[test]
fn real_bridge_handles_coalesced_request_with_closed_or_still_open_stdin() {
    let fixture = Fixture::new();
    let _server = fixture.server();
    let wire = [
        frame(json!({"type":"hello", "version":VERSION, "token":""})),
        frame(json!({"type":"request", "request":{"command":"list_domains"}})),
    ]
    .concat();
    for close_input in [true, false] {
        let output = fixture.bridge(&wire, close_input);
        assert!(
            output.status.success(),
            "bridge failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(
            decode(&output.stdout),
            vec![
                json!({"type":"hello", "version":VERSION}),
                json!({"type":"response", "response":{"result":"domains", "domains":[]}}),
            ]
        );
        assert_no_token(&output, &fixture.runtime.join("termeric/domain-v1.token"));
    }
}

#[test]
fn real_bridge_rejects_invalid_negotiation_without_starting_a_daemon() {
    for (hello, expected) in [
        (
            json!({"type":"hello", "version":0, "token":""}),
            "incompatible",
        ),
        (
            json!({"type":"hello", "version":VERSION, "token":"fixture-secret"}),
            "does not accept daemon tokens",
        ),
        (
            json!({"type":"request", "request":{"command":"list_domains"}}),
            "requires negotiation",
        ),
    ] {
        let fixture = Fixture::new();
        let output = fixture.bridge(&frame(hello), true);
        assert!(output.status.success());
        let frames = decode(&output.stdout);
        assert_eq!(frames.len(), 1);
        assert_eq!(frames[0]["type"], "error");
        assert!(frames[0]["message"].as_str().unwrap().contains(expected));
        assert!(!String::from_utf8_lossy(&output.stdout).contains("fixture-secret"));
        assert!(!String::from_utf8_lossy(&output.stderr).contains("fixture-secret"));
        assert!(
            fs::read_dir(&fixture.runtime).unwrap().next().is_none(),
            "rejected negotiation created daemon state"
        );
    }
}

#[test]
fn real_bridge_times_out_partial_negotiation_before_starting_a_daemon() {
    let fixture = Fixture::new();
    let mut child = OwnedChild(Some(
        fixture
            .command(&["domain", "proxy-stdio"])
            .stdin(Stdio::piped())
            .spawn()
            .unwrap(),
    ));
    let mut input = child.0.as_mut().unwrap().stdin.take().unwrap();
    input.write_all(&1024u32.to_be_bytes()).unwrap();
    // Keep the input pipe open with an incomplete payload. The production
    // ten-second budget must expire before this fifteen-second safety net.
    let output = child.output_with_timeout(Duration::from_secs(15));
    drop(input);
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("bridge negotiation") && stderr.contains("deadline elapsed"),
        "{stderr}"
    );
    assert!(
        fs::read_dir(&fixture.runtime).unwrap().next().is_none(),
        "incomplete negotiation created daemon state"
    );
}
