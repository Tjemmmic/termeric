//! The `termeric agent-event` hook command, run as agents run it: it reports
//! to the GUI that owns its terminal and otherwise stays silent and succeeds.
#![cfg(unix)]

use serde_json::{Value, json};
use std::fs;
use std::io::{Read, Write};
use std::os::unix::{fs::PermissionsExt, net::UnixListener};
use std::process::{Command, Output, Stdio};
use std::time::{Duration, Instant};

const TOKEN: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
const VERSION: u32 = 19;

/// A private runtime directory holding a fake GUI endpoint for this process.
struct Gui {
    root: tempfile::TempDir,
    listener: UnixListener,
}

impl Gui {
    fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        fs::set_permissions(root.path(), fs::Permissions::from_mode(0o700)).unwrap();
        let directory = root.path().join("termeric");
        fs::create_dir(&directory).unwrap();
        fs::set_permissions(&directory, fs::Permissions::from_mode(0o700)).unwrap();
        let stem = format!("gui-v1-{}", std::process::id());
        let socket = directory.join(format!("{stem}.sock"));
        let listener = UnixListener::bind(&socket).unwrap();
        listener.set_nonblocking(true).unwrap();
        fs::set_permissions(socket, fs::Permissions::from_mode(0o600)).unwrap();
        let token = directory.join(format!("{stem}.token"));
        fs::write(&token, TOKEN).unwrap();
        fs::set_permissions(&token, fs::Permissions::from_mode(0o600)).unwrap();
        Self { root, listener }
    }

    fn terminal_id(&self) -> String {
        format!("{}.00ff", std::process::id())
    }

    /// Run the hook as an agent would, feeding `payload` on stdin.
    fn hook(&self, args: &[&str], terminal_id: Option<&str>, payload: &str) -> (Output, Duration) {
        let mut command = Command::new(env!("CARGO_BIN_EXE_termeric"));
        command
            .arg("agent-event")
            .args(args)
            .env("XDG_RUNTIME_DIR", self.root.path())
            .env_remove("TERMERIC_TERMINAL_ID")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if let Some(id) = terminal_id {
            command.env("TERMERIC_TERMINAL_ID", id);
        }
        let started = Instant::now();
        let mut child = command.spawn().unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(payload.as_bytes())
            .unwrap();
        let output = child.wait_with_output().unwrap();
        (output, started.elapsed())
    }

    /// Accept one request, answer it with `reply` (or not at all), and
    /// return what was asked.
    fn serve(&self, reply: Option<Value>) -> Value {
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut stream = loop {
            match self.listener.accept() {
                Ok((stream, _)) => break stream,
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    assert!(Instant::now() < deadline, "the hook never reported");
                    std::thread::sleep(Duration::from_millis(5));
                }
                Err(error) => panic!("accept failed: {error}"),
            }
        };
        stream.set_nonblocking(false).unwrap();
        let mut size = [0; 4];
        stream.read_exact(&mut size).unwrap();
        let mut payload = vec![0; u32::from_be_bytes(size) as usize];
        stream.read_exact(&mut payload).unwrap();
        if let Some(reply) = reply {
            let bytes = serde_json::to_vec(&reply).unwrap();
            stream
                .write_all(&(bytes.len() as u32).to_be_bytes())
                .unwrap();
            stream.write_all(&bytes).unwrap();
        } else {
            // Hold the connection open without answering.
            std::thread::sleep(Duration::from_secs(3));
        }
        serde_json::from_slice(&payload).unwrap()
    }

    fn nothing_requested(&self) {
        assert!(matches!(
            self.listener.accept(),
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock
        ));
    }
}

fn assert_silent_success(output: &Output) {
    assert!(output.status.success(), "{output:?}");
    assert!(output.stdout.is_empty(), "a hook must print nothing");
    assert!(output.stderr.is_empty(), "a hook must print nothing");
}

#[test]
fn a_finished_turn_reaches_the_gui_that_owns_the_terminal() {
    let gui = Gui::new();
    let id = gui.terminal_id();
    let (output, request) = std::thread::scope(|scope| {
        let server = scope.spawn(|| {
            gui.serve(Some(json!({
                "result": "agent_event_recorded",
                "version": VERSION,
                "process_id": std::process::id(),
            })))
        });
        let payload = r#"{"hook_event_name":"Stop","last_assistant_message":"All done.","transcript_path":"/tmp/s.jsonl","stop_hook_active":false}"#;
        let (output, _) = gui.hook(&["claude-code", "stop"], Some(&id), payload);
        (output, server.join().unwrap())
    });
    assert_silent_success(&output);
    assert_eq!(
        request,
        json!({"version": VERSION, "token": TOKEN, "request": {
            "operation": "agent_event",
            "terminal_id": id,
            "agent_id": "claude-code",
            "event": "finished",
            "detail": null,
            "last_message": "All done.",
            "transcript_path": "/tmp/s.jsonl",
        }})
    );
}

#[test]
fn outside_termeric_or_with_unknown_arguments_nothing_is_sent() {
    let gui = Gui::new();
    let id = gui.terminal_id();
    for (args, terminal) in [
        (&["codex", "stop"][..], None),
        (&["codex", "not-an-event"][..], Some(id.as_str())),
        (&["co dex", "stop"][..], Some(id.as_str())),
        (&["codex"][..], Some(id.as_str())),
        (&[][..], Some(id.as_str())),
    ] {
        let (output, _) = gui.hook(args, terminal, "{}");
        assert_silent_success(&output);
    }
    gui.nothing_requested();
}

#[test]
fn a_missing_refusing_or_stuck_gui_never_fails_the_hook() {
    let gui = Gui::new();
    // No GUI process owns this identity.
    let (output, _) = gui.hook(&["codex", "prompt"], Some("999999999.00ff"), "");
    assert_silent_success(&output);

    let id = gui.terminal_id();
    std::thread::scope(|scope| {
        let server = scope.spawn(|| {
            gui.serve(Some(json!({
                "result": "error",
                "version": VERSION,
                "message": "No terminal in this process has that identity",
            })))
        });
        let (output, _) = gui.hook(&["codex", "prompt"], Some(&id), "");
        assert_silent_success(&output);
        server.join().unwrap();
    });

    std::thread::scope(|scope| {
        let server = scope.spawn(|| gui.serve(None));
        let (output, elapsed) = gui.hook(&["codex", "needs-input"], Some(&id), "{}");
        assert_silent_success(&output);
        assert!(
            elapsed < Duration::from_millis(2_900),
            "a stuck GUI held the hook for {elapsed:?}"
        );
        server.join().unwrap();
    });
}
