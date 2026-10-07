//! What every agent protocol provides to a session: the lines that open the
//! connection, the lines for each command, and the events each output line
//! carries.
//!
//! Each agent speaks its own protocol over the process's pipes, one JSON
//! message per line. An implementation translates both directions, so the
//! session, transcript, and panel see only agent-neutral events and
//! commands.

use serde_json::{Map, Value};

use super::event::{
    AgentCommand, AgentEvent, AssistantMessage, ContentBlock, Thread, ToolResult, TurnOutcome,
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

/// One JSON-RPC message from an agent, for the protocols built on JSON-RPC
/// (Codex's app-server and the Agent Client Protocol).
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
    /// A response to no request the host could have made.
    Stray,
    /// An error response with an `id` of `null`: the agent could not read
    /// one of the host's requests well enough to name it. Holds a preview
    /// of the error.
    Rejected(String),
    /// Not a JSON object, or one with neither a method nor an id.
    Unreadable,
}

/// Tell `line` apart as a request, notification, or response. An `id` of
/// `null` counts as absent: JSON-RPC 2.0 allows it, but neither protocol's
/// requests use it, and on an error response it means the agent could not
/// read the request it answers.
pub fn incoming(line: &str) -> Incoming {
    let Ok(Value::Object(message)) = serde_json::from_str::<Value>(line) else {
        return Incoming::Unreadable;
    };
    let id = message.get("id").filter(|id| !id.is_null()).cloned();
    let method = message
        .get("method")
        .and_then(Value::as_str)
        .map(str::to_owned);
    let params = || message.get("params").cloned().unwrap_or(Value::Null);
    match (id, method) {
        (Some(id), Some(method)) => Incoming::Request {
            id,
            method,
            params: params(),
        },
        (None, Some(method)) => Incoming::Notification {
            method,
            params: params(),
        },
        (Some(id), None) => match id.as_u64() {
            Some(id) => Incoming::Response { id, message },
            None => Incoming::Stray,
        },
        (None, None) => match message.get("error") {
            Some(error) if message.get("id").is_some_and(Value::is_null) => {
                Incoming::Rejected(preview(&error.to_string()))
            }
            _ => Incoming::Unreadable,
        },
    }
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
            Incoming::Request {
                id: Value::from("q"),
                method: "ask".into(),
                params: serde_json::json!({ "a": 1 }),
            }
        );
        for notification in [r#"{"method":"note"}"#, r#"{"id":null,"method":"note"}"#] {
            assert_eq!(
                incoming(notification),
                Incoming::Notification {
                    method: "note".into(),
                    params: Value::Null,
                },
                "{notification}"
            );
        }
        assert!(matches!(
            incoming(r#"{"id":7,"result":{}}"#),
            Incoming::Response { id: 7, .. }
        ));
        assert_eq!(incoming(r#"{"id":"x","result":{}}"#), Incoming::Stray);
        assert_eq!(
            incoming(
                r#"{"jsonrpc":"2.0","id":null,"error":{"code":-32700,"message":"Parse error"}}"#
            ),
            Incoming::Rejected(r#"{"code":-32700,"message":"Parse error"}"#.into())
        );
        for unreadable in [
            "not json",
            "[]",
            r#"{"id":null}"#,
            "{}",
            r#"{"error":{"code":-32700}}"#,
        ] {
            assert_eq!(incoming(unreadable), Incoming::Unreadable, "{unreadable}");
        }
    }
}
