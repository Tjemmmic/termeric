//! What every agent protocol provides to a session: the lines that open the
//! connection, the lines for each command, and the events each output line
//! carries.
//!
//! Each agent speaks its own protocol over the process's pipes, one JSON
//! message per line. An implementation translates both directions, so the
//! session, transcript, and panel see only agent-neutral events and
//! commands.

use serde_json::{Map, Value, json};

use super::event::{
    AgentCommand, AgentEvent, AssistantMessage, BlockStart, ContentBlock, StreamDelta, Thread,
    ToolResult, TurnOutcome,
};

/// One agent's wire protocol, for one process.
pub trait Protocol: Send {
    /// Lines to send as soon as the process starts.
    fn start(&mut self) -> Vec<String>;

    /// Lines to send for `command`, and anything it settles at once, such
    /// as a command the agent cannot take. No lines when the command needs
    /// nothing sent now, such as a setting applied with the next message.
    fn encode(&mut self, command: &AgentCommand) -> Exchange;

    /// The events one output line carries, and any lines the agent must
    /// receive in reply.
    fn decode(&mut self, line: &str) -> Exchange;
}

/// Events for the panel and lines for the agent.
#[derive(Debug, Default, PartialEq)]
pub struct Exchange {
    pub events: Vec<AgentEvent>,
    /// Lines to send right away, in order.
    pub lines: Vec<String>,
}

impl Exchange {
    /// Only lines to send.
    pub fn send(lines: Vec<String>) -> Self {
        Self {
            events: Vec::new(),
            lines,
        }
    }
}

/// `value` as one line of newline-delimited JSON.
pub fn line(value: &Value) -> String {
    let mut text = value.to_string();
    text.push('\n');
    text
}

/// The start of a line the panel could not read, for the log.
pub fn preview(text: &str) -> String {
    crate::safe_text::truncate_with_ellipsis(text, 200)
}

/// One JSON-RPC message from an agent for its protocol to handle, for the
/// protocols built on JSON-RPC (Codex's app-server and the Agent Client
/// Protocol).
#[derive(Debug, PartialEq)]
pub enum Incoming {
    /// A request the agent makes, answered with the same `id`.
    Request {
        id: Value,
        method: String,
        params: Value,
    },
    Notification {
        method: String,
        params: Value,
    },
    /// A response to one of the host's requests, whose ids are numbers.
    Response {
        id: u64,
        message: Map<String, Value>,
    },
}

/// A line from a JSON-RPC agent that holds no message for its protocol.
#[derive(Debug, PartialEq)]
pub enum Unhandled {
    /// A response to no request the host could have made.
    Stray,
    /// An error response with an `id` of `null`: the agent could not read
    /// one of the host's requests well enough to name it. Holds a preview
    /// of the error.
    Rejected(String),
    /// Not a JSON object, or one with neither a method nor an id. Holds a
    /// preview of the line.
    Unreadable(String),
}

impl Unhandled {
    /// What the line means for the panel: nothing for a stray response,
    /// and only the log learns of the rest.
    pub fn exchange(self) -> Exchange {
        match self {
            Unhandled::Stray => Exchange::default(),
            Unhandled::Rejected(error) => {
                log::warn!("agent view: agent rejected a request it could not parse: {error}");
                Exchange::default()
            }
            Unhandled::Unreadable(preview) => Exchange {
                events: vec![AgentEvent::Unrecognized(preview)],
                lines: Vec::new(),
            },
        }
    }
}

/// Tell `line` apart as a request, notification, or response. An `id` of
/// `null` counts as absent: JSON-RPC 2.0 allows it, but neither protocol's
/// requests use it, and on an error response it means the agent could not
/// read the request it answers.
pub fn incoming(line: &str) -> Result<Incoming, Unhandled> {
    let Ok(Value::Object(message)) = serde_json::from_str::<Value>(line) else {
        return Err(Unhandled::Unreadable(preview(line)));
    };
    let id = message.get("id").filter(|id| !id.is_null()).cloned();
    let method = message
        .get("method")
        .and_then(Value::as_str)
        .map(str::to_owned);
    let params = || message.get("params").cloned().unwrap_or(Value::Null);
    match (id, method) {
        (Some(id), Some(method)) => Ok(Incoming::Request {
            id,
            method,
            params: params(),
        }),
        (None, Some(method)) => Ok(Incoming::Notification {
            method,
            params: params(),
        }),
        (Some(id), None) => match id.as_u64() {
            Some(id) => Ok(Incoming::Response { id, message }),
            None => Err(Unhandled::Stray),
        },
        (None, None) => match message.get("error") {
            Some(error) if message.get("id").is_some_and(Value::is_null) => {
                Err(Unhandled::Rejected(preview(&error.to_string())))
            }
            _ => Err(Unhandled::Unreadable(preview(line))),
        },
    }
}

/// The panel's id for a request an agent made: `prefix` and the agent's
/// own `id` as text.
pub fn request_key(prefix: &str, id: &Value) -> String {
    match id {
        Value::String(id) => format!("{prefix}-{id}"),
        other => format!("{prefix}-{other}"),
    }
}

/// The JSON-RPC error answering a request this host does not handle, so the
/// agent never waits on it.
pub fn method_not_found(method: &str) -> Value {
    json!({ "code": -32601, "message": format!("Termeric does not handle {method}") })
}

/// How the host names itself when it connects.
pub fn client_info() -> Value {
    json!({
        "name": "termeric",
        "title": "Termeric",
        "version": env!("CARGO_PKG_VERSION"),
    })
}

/// The text at `key` in `value`, if it is text.
pub fn text(value: &Value, key: &str) -> Option<String> {
    value.get(key).and_then(Value::as_str).map(str::to_owned)
}

/// A main-thread message holding one tool call.
pub fn tool_use(id: &str, name: &str, input: Value) -> AgentEvent {
    AgentEvent::Message {
        thread: Thread::Main,
        message: AssistantMessage {
            id: id.to_owned(),
            blocks: vec![ContentBlock::ToolUse {
                id: id.to_owned(),
                name: name.to_owned(),
                input,
            }],
            error: None,
        },
    }
}

/// The start of main-thread message `id`, streamed as one block of text, or
/// of reasoning when `thinking`.
pub fn stream_started(id: &str, thinking: bool) -> [AgentEvent; 2] {
    let block = if thinking {
        BlockStart::Thinking
    } else {
        BlockStart::Text
    };
    [
        main_stream(StreamDelta::MessageStarted {
            message_id: id.to_owned(),
        }),
        main_stream(StreamDelta::BlockStarted { index: 0, block }),
    ]
}

/// The next piece of the block [`stream_started`] started.
pub fn stream_piece(thinking: bool, text: String) -> AgentEvent {
    main_stream(if thinking {
        StreamDelta::Thinking { index: 0, text }
    } else {
        StreamDelta::Text { index: 0, text }
    })
}

/// Main-thread message `id` complete as `block`, which is authoritative over
/// what streamed; when it was `streamed`, its stream ends too.
pub fn settled(id: &str, block: ContentBlock, streamed: bool) -> Vec<AgentEvent> {
    let mut events = vec![AgentEvent::Message {
        thread: Thread::Main,
        message: AssistantMessage {
            id: id.to_owned(),
            blocks: vec![block],
            error: None,
        },
    }];
    if streamed {
        events.push(main_stream(StreamDelta::BlockStopped { index: 0 }));
    }
    events
}

fn main_stream(delta: StreamDelta) -> AgentEvent {
    AgentEvent::Stream {
        thread: Thread::Main,
        delta,
    }
}

/// One to-do as the panel reads it, in the shape of Claude Code's to-do
/// list; `status` is `pending`, `in_progress`, or `completed`.
pub fn todo(content: &str, status: &str) -> Value {
    json!({ "content": content, "status": status, "activeForm": content })
}

/// The agent's plan for the turn as to-do list `id`, already settled: a plan
/// is a report, not a tool call that runs.
pub fn plan(id: &str, todos: Vec<Value>) -> Vec<AgentEvent> {
    vec![
        tool_use(id, "TodoWrite", json!({ "todos": todos })),
        tool_result(id, String::new(), false),
    ]
}

/// The result of the main-thread tool call `id`.
pub fn tool_result(id: &str, content: String, is_error: bool) -> AgentEvent {
    AgentEvent::ToolResult {
        thread: Thread::Main,
        result: ToolResult {
            tool_use_id: id.to_owned(),
            content,
            is_error,
            detail: None,
        },
    }
}

/// A turn that ended cleanly without the agent hearing of it, as a message
/// stopped before it was sent.
pub fn finished_turn() -> AgentEvent {
    AgentEvent::TurnFinished(TurnOutcome {
        success: true,
        ..TurnOutcome::default()
    })
}

/// A turn that ended with `error`.
pub fn failed_turn(error: &str) -> AgentEvent {
    AgentEvent::TurnFinished(TurnOutcome {
        success: false,
        error: Some(error.to_owned()),
        ..TurnOutcome::default()
    })
}

/// What the protocols' tests share.
#[cfg(test)]
pub(super) mod testing {
    use serde_json::Value;

    use super::{Exchange, Protocol};

    /// One line sent to the agent, which must end its message.
    pub fn parse(line: &str) -> Value {
        assert!(line.ends_with('\n'), "{line:?}");
        serde_json::from_str(line).unwrap()
    }

    /// What `protocol` makes of `value` written as one output line.
    pub fn decode(protocol: &mut impl Protocol, value: Value) -> Exchange {
        protocol.decode(&value.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn incoming_messages_are_told_apart_by_id_and_method() {
        let request = incoming(r#"{"id":"q","method":"ask","params":{"a":1}}"#);
        assert_eq!(
            request,
            Ok(Incoming::Request {
                id: Value::from("q"),
                method: "ask".into(),
                params: json!({ "a": 1 }),
            })
        );
        for notification in [r#"{"method":"note"}"#, r#"{"id":null,"method":"note"}"#] {
            assert_eq!(
                incoming(notification),
                Ok(Incoming::Notification {
                    method: "note".into(),
                    params: Value::Null,
                }),
                "{notification}"
            );
        }
        assert!(matches!(
            incoming(r#"{"id":7,"result":{}}"#),
            Ok(Incoming::Response { id: 7, .. })
        ));
        assert_eq!(incoming(r#"{"id":"x","result":{}}"#), Err(Unhandled::Stray));
        assert_eq!(
            incoming(
                r#"{"jsonrpc":"2.0","id":null,"error":{"code":-32700,"message":"Parse error"}}"#
            ),
            Err(Unhandled::Rejected(
                r#"{"code":-32700,"message":"Parse error"}"#.into()
            ))
        );
        for unreadable in [
            "not json",
            "[]",
            r#"{"id":null}"#,
            "{}",
            r#"{"error":{"code":-32700}}"#,
        ] {
            assert_eq!(
                incoming(unreadable),
                Err(Unhandled::Unreadable(unreadable.into())),
                "{unreadable}"
            );
        }
    }

    #[test]
    fn only_unreadable_lines_reach_the_panel_and_none_is_answered() {
        assert_eq!(Unhandled::Stray.exchange(), Exchange::default());
        assert_eq!(
            Unhandled::Rejected("parse error".into()).exchange(),
            Exchange::default()
        );
        assert_eq!(
            Unhandled::Unreadable("garbage".into()).exchange(),
            Exchange {
                events: vec![AgentEvent::Unrecognized("garbage".into())],
                lines: Vec::new(),
            }
        );
    }

    #[test]
    fn request_keys_read_string_ids_without_quotes() {
        assert_eq!(request_key("codex", &json!("a1")), "codex-a1");
        assert_eq!(request_key("acp", &json!(7)), "acp-7");
        assert_eq!(request_key("acp", &json!(null)), "acp-null");
    }
}
