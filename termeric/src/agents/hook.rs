//! `termeric agent-event AGENT EVENT`: the command an agent's hooks run.
//!
//! The agent passes its hook payload as JSON on standard input. Outside a
//! Termeric terminal the command does nothing. Inside one, it tells the GUI
//! that owns the terminal what the agent is doing. It never prints, and it
//! always succeeds, so a hook can neither disturb the agent's conversation
//! nor fail its turn.

use std::io::{IsTerminal, Read};

use crate::control::protocol::{
    AgentEventKind, MAX_AGENT_EVENT_DETAIL_BYTES, MAX_AGENT_LAST_MESSAGE_BYTES,
    MAX_AGENT_TRANSCRIPT_PATH_BYTES, Request,
};

/// The largest hook payload read. Larger ones are ignored rather than
/// parsed partially; the event itself is still reported.
const MAX_PAYLOAD_BYTES: u64 = 1024 * 1024;

/// Which hook fired, named on the command line by the installed hook.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HookEvent {
    SessionStart,
    Prompt,
    ToolDone,
    NeedsInput,
    Stop,
    StopFailure,
    SessionEnd,
}

impl HookEvent {
    pub const ALL: [Self; 7] = [
        Self::SessionStart,
        Self::Prompt,
        Self::ToolDone,
        Self::NeedsInput,
        Self::Stop,
        Self::StopFailure,
        Self::SessionEnd,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Self::SessionStart => "session-start",
            Self::Prompt => "prompt",
            Self::ToolDone => "tool-done",
            Self::NeedsInput => "needs-input",
            Self::Stop => "stop",
            Self::StopFailure => "stop-failure",
            Self::SessionEnd => "session-end",
        }
    }

    pub fn parse(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|event| event.name() == name)
    }

    fn kind(self) -> AgentEventKind {
        match self {
            Self::SessionStart => AgentEventKind::SessionStarted,
            Self::Prompt | Self::ToolDone => AgentEventKind::Working,
            Self::NeedsInput => AgentEventKind::NeedsInput,
            Self::Stop => AgentEventKind::Finished,
            Self::StopFailure => AgentEventKind::Failed,
            Self::SessionEnd => AgentEventKind::SessionEnded,
        }
    }

    /// Frequent events carry nothing Termeric uses, so their (possibly
    /// large) payloads are not read at all.
    fn reads_payload(self) -> bool {
        matches!(
            self,
            Self::SessionStart | Self::NeedsInput | Self::Stop | Self::StopFailure
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HookInvocation {
    pub agent_id: String,
    pub event: HookEvent,
}

/// Report the hook to the terminal's GUI, if it runs in a Termeric terminal.
pub fn run(invocation: &HookInvocation) {
    let Ok(terminal_id) = std::env::var(termeric_core::pty::TERMINAL_ID_VARIABLE) else {
        return;
    };
    let Some(process_id) = terminal_id
        .split_once('.')
        .and_then(|(process, _)| process.parse::<u32>().ok())
    else {
        return;
    };
    let payload = if invocation.event.reads_payload() {
        read_payload()
    } else {
        None
    };
    let report = build_report(invocation, terminal_id, payload.as_ref());
    // Nothing useful can be done about a failure from inside a hook: the GUI
    // may have exited, or be an older build.
    if let Err(error) = crate::control::report_agent_event(process_id, report) {
        log::debug!("Agent event was not delivered: {error:#}");
    }
}

fn read_payload() -> Option<serde_json::Value> {
    let stdin = std::io::stdin();
    if stdin.is_terminal() {
        return None;
    }
    let mut bytes = Vec::new();
    stdin
        .lock()
        .take(MAX_PAYLOAD_BYTES + 1)
        .read_to_end(&mut bytes)
        .ok()?;
    if bytes.len() as u64 > MAX_PAYLOAD_BYTES {
        return None;
    }
    serde_json::from_slice(&bytes).ok()
}

/// The wire request for one hook, taking what it can from the payload.
fn build_report(
    invocation: &HookInvocation,
    terminal_id: String,
    payload: Option<&serde_json::Value>,
) -> Request {
    let text = |key: &str| {
        payload
            .and_then(|payload| payload.get(key))
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .filter(|text| !text.is_empty())
    };
    let detail = match invocation.event {
        // Claude Code's notification carries a sentence; Codex's permission
        // request names the tool.
        HookEvent::NeedsInput => text("message").map(str::to_owned).or_else(|| {
            let tool = text("tool_name")?;
            let command = payload
                .and_then(|payload| payload.pointer("/tool_input/command"))
                .and_then(serde_json::Value::as_str);
            Some(match command {
                Some(command) => format!("Approve {tool}: {command}"),
                None => format!("Approve {tool}"),
            })
        }),
        HookEvent::StopFailure => Some(
            text("error")
                .or_else(|| text("error_type"))
                .or_else(|| text("reason"))
                .unwrap_or("The turn ended with an error")
                .to_owned(),
        ),
        _ => None,
    };
    let last_message = if invocation.event == HookEvent::Stop {
        text("last_assistant_message")
            .map(|message| truncate(message, MAX_AGENT_LAST_MESSAGE_BYTES).to_owned())
    } else {
        None
    };
    let transcript_path = text("transcript_path")
        .filter(|path| path.starts_with('/') && path.len() <= MAX_AGENT_TRANSCRIPT_PATH_BYTES)
        .map(str::to_owned);
    Request::AgentEvent {
        terminal_id,
        agent_id: invocation.agent_id.clone(),
        event: invocation.event.kind(),
        detail: detail.map(|detail| truncate(&detail, MAX_AGENT_EVENT_DETAIL_BYTES).to_owned()),
        last_message,
        transcript_path,
    }
}

/// The longest prefix of `text` within `limit` bytes, on a character
/// boundary.
fn truncate(text: &str, limit: usize) -> &str {
    if text.len() <= limit {
        return text;
    }
    let mut end = limit;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn invocation(event: HookEvent) -> HookInvocation {
        HookInvocation {
            agent_id: "claude-code".into(),
            event,
        }
    }

    fn fields(
        request: Request,
    ) -> (
        AgentEventKind,
        Option<String>,
        Option<String>,
        Option<String>,
    ) {
        let Request::AgentEvent {
            event,
            detail,
            last_message,
            transcript_path,
            ..
        } = request
        else {
            panic!("not an agent event");
        };
        (event, detail, last_message, transcript_path)
    }

    #[test]
    fn event_names_round_trip() {
        for event in HookEvent::ALL {
            assert_eq!(HookEvent::parse(event.name()), Some(event));
        }
        assert_eq!(HookEvent::parse("PreToolUse"), None);
    }

    #[test]
    fn payloads_supply_details_answers_and_logs() {
        let claude = json!({
            "hook_event_name": "Notification",
            "message": "Claude needs your permission to use Bash",
            "transcript_path": "/home/me/.claude/projects/x/s.jsonl",
        });
        let (kind, detail, _, transcript) = fields(build_report(
            &invocation(HookEvent::NeedsInput),
            "1.ab".into(),
            Some(&claude),
        ));
        assert_eq!(kind, AgentEventKind::NeedsInput);
        assert_eq!(
            detail.as_deref(),
            Some("Claude needs your permission to use Bash")
        );
        assert_eq!(
            transcript.as_deref(),
            Some("/home/me/.claude/projects/x/s.jsonl")
        );

        let codex = json!({
            "hook_event_name": "PermissionRequest",
            "tool_name": "shell",
            "tool_input": {"command": "cargo test"},
            "transcript_path": null,
        });
        let (_, detail, _, transcript) = fields(build_report(
            &invocation(HookEvent::NeedsInput),
            "1.ab".into(),
            Some(&codex),
        ));
        assert_eq!(detail.as_deref(), Some("Approve shell: cargo test"));
        assert_eq!(transcript, None);

        let stop = json!({"last_assistant_message": "  Done.  ", "transcript_path": "relative"});
        let (kind, detail, message, transcript) = fields(build_report(
            &invocation(HookEvent::Stop),
            "1.ab".into(),
            Some(&stop),
        ));
        assert_eq!(kind, AgentEventKind::Finished);
        assert_eq!((detail, message.as_deref()), (None, Some("Done.")));
        assert_eq!(transcript, None, "only absolute paths are reported");

        let (kind, detail, ..) = fields(build_report(
            &invocation(HookEvent::StopFailure),
            "1.ab".into(),
            None,
        ));
        assert_eq!(kind, AgentEventKind::Failed);
        assert_eq!(detail.as_deref(), Some("The turn ended with an error"));
    }

    #[test]
    fn oversized_text_is_cut_on_a_character_boundary() {
        let message = "é".repeat(MAX_AGENT_LAST_MESSAGE_BYTES);
        let (.., last, _) = fields(build_report(
            &invocation(HookEvent::Stop),
            "1.ab".into(),
            Some(&json!({ "last_assistant_message": message })),
        ));
        let last = last.unwrap();
        assert!(last.len() <= MAX_AGENT_LAST_MESSAGE_BYTES);
        assert!(last.chars().all(|c| c == 'é'));
        let request = build_report(
            &invocation(HookEvent::NeedsInput),
            "1.ab".into(),
            Some(&json!({ "message": "x".repeat(4096) })),
        );
        request.validate().unwrap();
    }
}
