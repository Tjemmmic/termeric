//! What every agent protocol provides to a session: the lines that open the
//! connection, the lines for each command, and the events each output line
//! carries.
//!
//! Each agent speaks its own protocol over the process's pipes, one JSON
//! message per line. An implementation translates both directions, so the
//! session, transcript, and panel see only agent-neutral events and
//! commands.

use serde_json::Value;

use super::event::{AgentCommand, AgentEvent};

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
    const LIMIT: usize = 200;
    match text.char_indices().nth(LIMIT) {
        Some((end, _)) => format!("{}…", &text[..end]),
        None => text.to_owned(),
    }
}
