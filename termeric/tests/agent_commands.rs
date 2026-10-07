//! The commands agents run, run as agents run them: the `agent-event` hook,
//! which reports to the Termeric window process that owns its terminal and
//! otherwise stays silent and succeeds, and the `mcp` server.
//!
//! The test plays that window process: it holds the control socket a GUI
//! with this test's process ID would, and answers in the GUI control
//! protocol. Termeric is binary-only, so tests can't import its protocol
//! types; the requests and replies are spelled out as JSON here, which also
//! pins their wire format.

mod common;

use common::{GUI_TOKEN, StandInGui};
use serde_json::{Value, json};
use std::io::Write;
use std::process::{Command, Output, Stdio};
use std::time::{Duration, Instant};

/// The GUI control protocol version every request and reply carries.
const VERSION: u32 = 1;

type Gui = StandInGui;

impl Gui {
    /// An identity naming a terminal in this test's process, as Termeric
    /// gives each terminal's programs in `TERMERIC_TERMINAL_ID`.
    fn terminal_id(&self) -> String {
        format!("{}.00ff00ff00ff00ff00ff00ff00ff00ff", std::process::id())
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

    /// Accept one request, answer it with `reply` as the GUI would (or not
    /// at all), and return what was asked.
    fn serve(&self, reply: Option<Value>) -> Value {
        let mut stream = self.accept("the hook never reported");
        let request = common::read_frame(&mut stream);
        if let Some(reply) = reply {
            common::write_frame(&mut stream, &json!({"version": VERSION, "response": reply}));
        } else {
            // Hold the connection open without answering.
            std::thread::sleep(Duration::from_secs(3));
        }
        request
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
        json!({"version": VERSION, "token": GUI_TOKEN, "request": {
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
    let (output, _) = gui.hook(
        &["codex", "prompt"],
        Some("999999999.00ff00ff00ff00ff00ff00ff00ff00ff"),
        "",
    );
    assert_silent_success(&output);

    let id = gui.terminal_id();
    std::thread::scope(|scope| {
        let server = scope.spawn(|| {
            gui.serve(Some(json!({
                "result": "error",
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

#[test]
fn the_mcp_server_speaks_json_rpc_over_stdio_to_the_terminals_gui() {
    let gui = Gui::new();
    let id = gui.terminal_id();
    let requests = [
        json!({"jsonrpc": "2.0", "id": 1, "method": "initialize",
               "params": {"protocolVersion": "2025-06-18", "capabilities": {}}}),
        json!({"jsonrpc": "2.0", "method": "notifications/initialized"}),
        json!({"jsonrpc": "2.0", "id": 2, "method": "tools/call",
               "params": {"name": "list_agents", "arguments": {}}}),
    ];
    let input: String = requests
        .iter()
        .map(|request| format!("{request}\n"))
        .collect();
    let (output, request) = std::thread::scope(|scope| {
        let server = scope.spawn(|| {
            gui.serve(Some(json!({
                "result": "agents",
                "process_id": std::process::id(),
                "truncated": false,
                "agents": [{
                    "window_id": 1, "tab_id": 2, "pane_id": 0,
                    "agent_id": "codex", "name": "Codex", "tab_title": "api",
                    "state": "needs_input", "detail": "Approve shell: ls",
                    "last_message": null, "seconds_in_state": 3,
                }],
            })))
        });
        let mut child = Command::new(env!("CARGO_BIN_EXE_termeric"))
            .arg("mcp")
            .env("XDG_RUNTIME_DIR", gui.root.path())
            .env("TERMERIC_TERMINAL_ID", &id)
            .env("RUST_LOG", "off")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(input.as_bytes())
            .unwrap();
        (child.wait_with_output().unwrap(), server.join().unwrap())
    });
    assert!(output.status.success(), "{output:?}");
    assert_eq!(
        request,
        json!({"version": VERSION, "token": GUI_TOKEN,
               "request": {"operation": "list_agents", "supervisor": null}})
    );
    let replies: Vec<Value> = String::from_utf8(output.stdout)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(
        replies.len(),
        2,
        "one reply per request, none for notifications"
    );
    assert_eq!(replies[0]["result"]["serverInfo"]["name"], "termeric");
    let result = &replies[1]["result"];
    assert_eq!(result["isError"], false);
    assert_eq!(
        result["structuredContent"]["agents"][0]["state"],
        "needs_input"
    );
}

#[test]
fn the_hook_reads_every_payload_so_agents_never_see_a_broken_pipe() {
    let gui = Gui::new();
    let large = format!("{{\"tool_response\": \"{}\"}}", "x".repeat(3 * 1024 * 1024));
    // Writing the whole payload succeeds (the helper unwraps the write),
    // whether or not the hook runs inside Termeric or uses the payload.
    for (event, terminal) in [
        ("tool-done", None),
        ("stop", None),
        ("from-the-future", None),
    ] {
        let (output, _) = gui.hook(&["codex", event], terminal, &large);
        assert_silent_success(&output);
    }
    let id = gui.terminal_id();
    std::thread::scope(|scope| {
        let server = scope.spawn(|| {
            gui.serve(Some(json!({
                "result": "agent_event_recorded",
                "process_id": std::process::id(),
            })))
        });
        let (output, _) = gui.hook(&["codex", "tool-done"], Some(&id), &large);
        assert_silent_success(&output);
        server.join().unwrap();
    });
}
