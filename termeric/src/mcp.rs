//! `termeric mcp`: a Model Context Protocol server on standard input and
//! output, so an agent can see and direct the other agents in Termeric.
//!
//! It is a thin translation of the running GUI's control protocol into MCP
//! tools. Reading terminals needs nothing beyond what `termeric control`
//! already allows. Sending requires `security.allow_gui_control_input`, or,
//! for the server Termeric starts for its supervisor, that supervisor's mode
//! and limits; the GUI enforces both.

use std::io::{BufRead, Read, Write};

use anyhow::{Context, Result, bail};
use serde_json::{Value, json};

use crate::control::protocol::{AgentSummary, MAX_TEXT_HISTORY_LINES};

/// Protocol revisions this server speaks, newest first.
const SUPPORTED_VERSIONS: [&str; 3] = ["2025-06-18", "2025-03-26", "2024-11-05"];
const DEFAULT_READ_LINES: u16 = 80;
/// Lines longer than this are refused rather than buffered without bound.
const MAX_MESSAGE_BYTES: usize = 1024 * 1024;

const INSTRUCTIONS: &str = "Termeric is the terminal these agents run in. \
list_agents shows every coding agent in Termeric with its state (working, needs_input, \
finished, failed) and where it is: window_id, tab_id, and pane_id. read_terminal reads a \
pane's recent screen and scrollback. send_to_agent gives an agent a prompt, as if typed and \
submitted; Termeric refuses it unless the person enabled external input, while the person is \
typing to that agent, and for text starting with `!` or `/`, which the agent would run as a \
command.";

/// What the server needs from a running Termeric.
pub trait Backend {
    fn list_agents(&self) -> Result<(Vec<AgentSummary>, bool)>;
    fn read_terminal(
        &self,
        window_id: u64,
        tab_id: u64,
        pane_id: u32,
        lines: u16,
    ) -> Result<String>;
    fn submit(
        &self,
        window_id: u64,
        tab_id: u64,
        pane_id: u32,
        agent_id: &str,
        text: &str,
    ) -> Result<bool>;
}

/// The Termeric process to talk to: the one named, else the one owning the
/// terminal this server runs in, else the only one running.
pub struct ControlBackend {
    pub process_id: Option<u32>,
    /// Set when Termeric started this server for its supervisor.
    pub supervisor: Option<String>,
}

impl ControlBackend {
    fn process(&self) -> Result<u32> {
        if let Some(process_id) = self.process_id {
            return Ok(process_id);
        }
        if let Some(process_id) = std::env::var(termeric_core::pty::TERMINAL_ID_VARIABLE)
            .ok()
            .and_then(|id| id.split_once('.')?.0.parse().ok())
        {
            return Ok(process_id);
        }
        match crate::control::running_processes()?.as_slice() {
            [process_id] => Ok(*process_id),
            [] => bail!("No Termeric window is running"),
            several => bail!(
                "Several Termeric processes are running ({}); start the server with --process PID",
                several
                    .iter()
                    .map(u32::to_string)
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        }
    }
}

impl Backend for ControlBackend {
    fn list_agents(&self) -> Result<(Vec<AgentSummary>, bool)> {
        crate::control::list_agents(self.process()?, self.supervisor.as_deref())
    }

    fn read_terminal(
        &self,
        window_id: u64,
        tab_id: u64,
        pane_id: u32,
        lines: u16,
    ) -> Result<String> {
        crate::control::read_terminal_text(self.process()?, window_id, tab_id, pane_id, lines)
    }

    fn submit(
        &self,
        window_id: u64,
        tab_id: u64,
        pane_id: u32,
        agent_id: &str,
        text: &str,
    ) -> Result<bool> {
        crate::control::submit_to_agent(
            self.process()?,
            window_id,
            tab_id,
            pane_id,
            agent_id,
            text,
            self.supervisor.as_deref(),
        )
    }
}

/// Serve MCP over standard input and output until input ends.
pub fn serve(backend: impl Backend) -> Result<()> {
    serve_on(backend, std::io::stdin().lock(), std::io::stdout().lock())
}

/// Serve MCP messages, one per line, from `reader` to `writer`.
fn serve_on(backend: impl Backend, mut reader: impl BufRead, mut writer: impl Write) -> Result<()> {
    let mut server = Server::new(backend);
    let mut line = Vec::new();
    loop {
        line.clear();
        let read = (&mut reader)
            .take(MAX_MESSAGE_BYTES as u64 + 1)
            .read_until(b'\n', &mut line)
            .context("read from standard input")?;
        if read == 0 {
            return Ok(());
        }
        let complete = line.last() == Some(&b'\n');
        if complete {
            line.pop();
        }
        let reply = if !complete && line.len() > MAX_MESSAGE_BYTES {
            skip_line(&mut reader)?;
            Some(error_reply(Value::Null, -32600, "Message too large"))
        } else if line.trim_ascii().is_empty() {
            None
        } else {
            match serde_json::from_slice::<Value>(&line) {
                Ok(message) => server.handle_message(message),
                Err(error) => Some(error_reply(Value::Null, -32700, &error.to_string())),
            }
        };
        if let Some(reply) = reply {
            serde_json::to_writer(&mut writer, &reply)?;
            writer.write_all(b"\n")?;
            writer.flush()?;
        }
    }
}

/// Discard input through the next newline without holding it.
fn skip_line(reader: &mut impl BufRead) -> Result<()> {
    loop {
        let available = reader.fill_buf().context("read from standard input")?;
        if available.is_empty() {
            return Ok(());
        }
        match available.iter().position(|byte| *byte == b'\n') {
            Some(end) => {
                reader.consume(end + 1);
                return Ok(());
            }
            None => {
                let length = available.len();
                reader.consume(length);
            }
        }
    }
}

pub struct Server<B> {
    backend: B,
}

fn error_reply(id: Value, code: i64, message: &str) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message}})
}

fn tool_result(text: String, structured: Option<Value>, is_error: bool) -> Value {
    let mut result = json!({
        "content": [{"type": "text", "text": text}],
        "isError": is_error,
    });
    if let Some(structured) = structured {
        result["structuredContent"] = structured;
    }
    result
}

/// The schema of a tool addressed to one pane, with `extra` arguments given
/// as (name, schema, required).
fn pane_schema(extra: &[(&str, Value, bool)]) -> Value {
    let mut properties = json!({
        "window_id": {"type": "integer", "minimum": 1, "description": "From list_agents."},
        "tab_id": {"type": "integer", "minimum": 0, "description": "From list_agents."},
        "pane_id": {"type": "integer", "minimum": 0, "description": "From list_agents."},
    });
    let mut required = vec!["window_id", "tab_id", "pane_id"];
    for (name, schema, is_required) in extra {
        properties[*name] = schema.clone();
        if *is_required {
            required.push(name);
        }
    }
    json!({"type": "object", "properties": properties, "required": required})
}

fn tools() -> Value {
    json!([
        {
            "name": "list_agents",
            "title": "List agents",
            "description": "Every coding agent running in Termeric: its name, state (working, needs_input, finished, failed, idle, or null when unknown), what it is waiting for, its last answer, and its window_id, tab_id, and pane_id.",
            "inputSchema": {"type": "object", "properties": {}},
            "annotations": {"readOnlyHint": true},
        },
        {
            "name": "read_terminal",
            "title": "Read a terminal",
            "description": "The recent screen and scrollback of one pane, oldest line first.",
            "inputSchema": pane_schema(&[(
                "lines",
                json!({"type": "integer", "minimum": 0, "maximum": MAX_TEXT_HISTORY_LINES,
                       "description": "Scrollback lines above the screen to include (default 80)."}),
                false,
            )]),
            "annotations": {"readOnlyHint": true},
        },
        {
            "name": "send_to_agent",
            "title": "Send to an agent",
            "description": "Give the agent in one pane a prompt, pasted and submitted as if typed. Plain text of at most 4000 bytes and 80 lines, not starting with `!` or `/`. Refused if that pane no longer runs agent_id, while the person is typing to it, or if the person has not enabled external input.",
            "inputSchema": pane_schema(&[
                (
                    "agent_id",
                    json!({"type": "string", "description": "The agent's id from list_agents; the send fails if it has changed."}),
                    true,
                ),
                ("text", json!({"type": "string", "description": "The prompt."}), true),
            ]),
            "annotations": {"readOnlyHint": false, "destructiveHint": false},
        },
    ])
}

fn integer<T: TryFrom<u64>>(arguments: &Value, name: &str) -> Result<T> {
    arguments
        .get(name)
        .and_then(Value::as_u64)
        .and_then(|value| T::try_from(value).ok())
        .with_context(|| format!("`{name}` must be a non-negative integer"))
}

impl<B: Backend> Server<B> {
    pub fn new(backend: B) -> Self {
        Self { backend }
    }

    /// The reply to one line of input: a message, or a batch of them as
    /// protocol revisions before 2025-06-18 allow.
    pub fn handle_message(&mut self, message: Value) -> Option<Value> {
        let Value::Array(batch) = message else {
            return self.handle(message);
        };
        if batch.is_empty() {
            return Some(error_reply(Value::Null, -32600, "Empty batch"));
        }
        let replies: Vec<Value> = batch
            .into_iter()
            .filter_map(|message| self.handle(message))
            .collect();
        (!replies.is_empty()).then_some(Value::Array(replies))
    }

    /// The reply to one message, if it needs one.
    pub fn handle(&mut self, message: Value) -> Option<Value> {
        if message.get("result").is_some() || message.get("error").is_some() {
            // A response, which is never answered; this server sends no
            // requests of its own.
            return None;
        }
        let id = message.get("id").cloned();
        let Some(method) = message.get("method").and_then(Value::as_str) else {
            return Some(error_reply(
                id.unwrap_or(Value::Null),
                -32600,
                "Not a request",
            ));
        };
        // Notifications (no id) never get a reply.
        let id = id?;
        let params = message.get("params").cloned().unwrap_or(Value::Null);
        let result = match method {
            "initialize" => {
                let requested = params
                    .get("protocolVersion")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                let version = SUPPORTED_VERSIONS
                    .into_iter()
                    .find(|version| *version == requested)
                    .unwrap_or(SUPPORTED_VERSIONS[0]);
                json!({
                    "protocolVersion": version,
                    "capabilities": {"tools": {"listChanged": false}},
                    "serverInfo": {"name": "termeric", "version": env!("CARGO_PKG_VERSION")},
                    "instructions": INSTRUCTIONS,
                })
            }
            "ping" => json!({}),
            "tools/list" => json!({"tools": tools()}),
            "tools/call" => self.call(&params),
            _ => return Some(error_reply(id, -32601, &format!("Unknown method {method}"))),
        };
        Some(json!({"jsonrpc": "2.0", "id": id, "result": result}))
    }

    /// Run one tool. Failures are reported to the model as tool errors.
    fn call(&mut self, params: &Value) -> Value {
        let name = params
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let arguments = params
            .get("arguments")
            .cloned()
            .unwrap_or_else(|| json!({}));
        let outcome = match name {
            "list_agents" => self.backend.list_agents().map(|(agents, truncated)| {
                let text = if agents.is_empty() {
                    "No coding agent is running in Termeric.".to_string()
                } else {
                    serde_json::to_string_pretty(&agents).unwrap_or_default()
                };
                (
                    text,
                    Some(json!({"agents": agents, "truncated": truncated})),
                )
            }),
            "read_terminal" => (|| {
                let lines = match arguments.get("lines") {
                    None => DEFAULT_READ_LINES,
                    Some(_) => integer::<u16>(&arguments, "lines")?.min(MAX_TEXT_HISTORY_LINES),
                };
                let text = self.backend.read_terminal(
                    integer(&arguments, "window_id")?,
                    integer(&arguments, "tab_id")?,
                    integer(&arguments, "pane_id")?,
                    lines,
                )?;
                Ok((text, None))
            })(),
            "send_to_agent" => (|| {
                let text = arguments
                    .get("text")
                    .and_then(Value::as_str)
                    .context("`text` must be a string")?;
                let agent_id = arguments
                    .get("agent_id")
                    .and_then(Value::as_str)
                    .context("`agent_id` must be a string")?;
                let (window_id, tab_id, pane_id) = (
                    integer(&arguments, "window_id")?,
                    integer(&arguments, "tab_id")?,
                    integer(&arguments, "pane_id")?,
                );
                let pending = self
                    .backend
                    .submit(window_id, tab_id, pane_id, agent_id, text)?;
                let text = if pending {
                    format!(
                        "Waiting for the person to approve your message to {agent_id}. \
                         Termeric will tell you whether it was sent."
                    )
                } else {
                    format!(
                        "Sent to {agent_id} in window {window_id}, tab {tab_id}, pane {pane_id}."
                    )
                };
                Ok((text, None))
            })(),
            _ => Err(anyhow::anyhow!("Unknown tool {name}")),
        };
        match outcome {
            Ok((text, structured)) => tool_result(text, structured, false),
            Err(error) => tool_result(format!("{error:#}"), None, true),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::control::protocol::AgentState;
    use std::cell::RefCell;

    /// window, tab, pane, agent, text
    type Sent = (u64, u64, u32, String, String);

    #[derive(Default)]
    struct Fake {
        sent: RefCell<Vec<Sent>>,
    }

    impl Backend for Fake {
        fn list_agents(&self) -> Result<(Vec<AgentSummary>, bool)> {
            Ok((
                vec![AgentSummary {
                    window_id: 1,
                    tab_id: 4,
                    pane_id: 0,
                    agent_id: "codex".into(),
                    name: "Codex".into(),
                    tab_title: "api".into(),
                    state: Some(AgentState::NeedsInput),
                    detail: Some("Approve shell: cargo test".into()),
                    last_message: None,
                    seconds_in_state: Some(12),
                }],
                false,
            ))
        }

        fn read_terminal(&self, window: u64, tab: u64, pane: u32, lines: u16) -> Result<String> {
            Ok(format!("{window}/{tab}/{pane} last {lines}"))
        }

        fn submit(
            &self,
            window: u64,
            tab: u64,
            pane: u32,
            agent: &str,
            text: &str,
        ) -> Result<bool> {
            if agent != "codex" {
                bail!("{agent} is no longer running in that terminal");
            }
            self.sent
                .borrow_mut()
                .push((window, tab, pane, agent.into(), text.into()));
            Ok(text.contains("approve me"))
        }
    }

    fn request(server: &mut Server<Fake>, id: u64, method: &str, params: Value) -> Value {
        server
            .handle(json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}))
            .unwrap()
    }

    fn call(server: &mut Server<Fake>, name: &str, arguments: Value) -> Value {
        request(
            server,
            9,
            "tools/call",
            json!({"name": name, "arguments": arguments}),
        )["result"]
            .clone()
    }

    #[test]
    fn handshake_negotiates_a_version_and_lists_three_tools() {
        let mut server = Server::new(Fake::default());
        let init = request(
            &mut server,
            1,
            "initialize",
            json!({"protocolVersion": "2025-03-26", "capabilities": {}}),
        );
        assert_eq!(init["result"]["protocolVersion"], "2025-03-26");
        assert_eq!(init["result"]["serverInfo"]["name"], "termeric");
        let unknown = request(
            &mut server,
            2,
            "initialize",
            json!({"protocolVersion": "1999"}),
        );
        assert_eq!(unknown["result"]["protocolVersion"], SUPPORTED_VERSIONS[0]);
        assert_eq!(
            server.handle(json!({"jsonrpc": "2.0", "method": "notifications/initialized"})),
            None,
            "notifications get no reply"
        );
        let names: Vec<_> = request(&mut server, 3, "tools/list", json!({}))["result"]["tools"]
            .as_array()
            .unwrap()
            .iter()
            .map(|tool| tool["name"].as_str().unwrap().to_owned())
            .collect();
        assert_eq!(names, ["list_agents", "read_terminal", "send_to_agent"]);
        let send = &request(&mut server, 4, "tools/list", json!({}))["result"]["tools"][2];
        assert_eq!(
            send["inputSchema"]["required"],
            json!(["window_id", "tab_id", "pane_id", "agent_id", "text"])
        );
        assert_eq!(
            request(&mut server, 5, "resources/list", json!({}))["error"]["code"],
            -32601
        );
    }

    #[test]
    fn tools_read_and_direct_agents_and_report_failures_as_tool_errors() {
        let mut server = Server::new(Fake::default());
        let listed = call(&mut server, "list_agents", json!({}));
        assert_eq!(listed["isError"], false);
        assert_eq!(
            listed["structuredContent"]["agents"][0]["state"],
            "needs_input"
        );
        assert!(
            listed["content"][0]["text"]
                .as_str()
                .unwrap()
                .contains("Approve shell")
        );

        let read = call(
            &mut server,
            "read_terminal",
            json!({"window_id": 1, "tab_id": 4, "pane_id": 0, "lines": 9999}),
        );
        assert_eq!(
            read["content"][0]["text"],
            format!("1/4/0 last {MAX_TEXT_HISTORY_LINES}")
        );

        let sent = call(
            &mut server,
            "send_to_agent",
            json!({"window_id": 1, "tab_id": 4, "pane_id": 0, "agent_id": "codex", "text": "yes, run it"}),
        );
        assert_eq!(sent["isError"], false);
        assert_eq!(
            server.backend.sent.borrow().as_slice(),
            [(1, 4, 0, "codex".into(), "yes, run it".into())]
        );

        let held = call(
            &mut server,
            "send_to_agent",
            json!({"window_id": 1, "tab_id": 4, "pane_id": 0, "agent_id": "codex", "text": "approve me"}),
        );
        assert!(
            held["content"][0]["text"]
                .as_str()
                .unwrap()
                .starts_with("Waiting for the person to approve")
        );

        let gone = call(
            &mut server,
            "send_to_agent",
            json!({"window_id": 1, "tab_id": 4, "pane_id": 0, "agent_id": "gemini", "text": "hi"}),
        );
        assert_eq!(gone["isError"], true);
        assert_eq!(
            gone["content"][0]["text"],
            "gemini is no longer running in that terminal"
        );
        let invalid = call(&mut server, "read_terminal", json!({"window_id": -1}));
        assert_eq!(invalid["isError"], true);
        assert_eq!(call(&mut server, "nope", json!({}))["isError"], true);
    }

    /// The replies `serve_on` writes for `input`.
    fn serve_text(input: &[u8]) -> Vec<Value> {
        let mut output = Vec::new();
        serve_on(Fake::default(), input, &mut output).unwrap();
        String::from_utf8(output)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect()
    }

    #[test]
    fn the_server_survives_bad_input_and_answers_batches() {
        let ping = |id: u64| format!("{{\"jsonrpc\":\"2.0\",\"id\":{id},\"method\":\"ping\"}}");
        let mut input = Vec::new();
        // Invalid UTF-8 is a parse error, not the end of the server.
        input.extend_from_slice(b"{\"x\": \"\xff\"}\n");
        // A message of exactly the limit is accepted; one byte more is not.
        let padded = |length: usize| {
            let message = ping(2);
            let mut value = message[..message.len() - 1].to_string();
            value.push_str(",\"pad\":\"");
            let pad = length - value.len() - 2;
            value.push_str(&"a".repeat(pad));
            value.push_str("\"}");
            assert_eq!(value.len(), length);
            value
        };
        input.extend_from_slice(padded(MAX_MESSAGE_BYTES).as_bytes());
        input.push(b'\n');
        input.extend_from_slice(padded(MAX_MESSAGE_BYTES + 1).as_bytes());
        input.push(b'\n');
        // A batch gets one reply per request, and none for notifications or
        // responses.
        input.extend_from_slice(
            format!(
                "[{}, {{\"jsonrpc\":\"2.0\",\"method\":\"notifications/initialized\"}}, {}]\n",
                ping(3),
                ping(4)
            )
            .as_bytes(),
        );
        input.extend_from_slice(b"{\"jsonrpc\":\"2.0\",\"id\":5,\"result\":{}}\n");
        input.extend_from_slice(b"[]\n");
        input.extend_from_slice(ping(6).as_bytes());
        let replies = serve_text(&input);
        assert_eq!(replies.len(), 6, "{replies:?}");
        assert_eq!(replies[0]["error"]["code"], -32700);
        assert_eq!(replies[1]["id"], 2);
        assert!(replies[1].get("result").is_some());
        assert_eq!(replies[2]["error"]["message"], "Message too large");
        let batch = replies[3].as_array().unwrap();
        assert_eq!(
            batch
                .iter()
                .map(|reply| reply["id"].clone())
                .collect::<Vec<_>>(),
            [json!(3), json!(4)]
        );
        assert_eq!(replies[4]["error"]["message"], "Empty batch");
        assert_eq!(
            replies[5]["id"], 6,
            "a last line without a newline is still read"
        );
    }
}
