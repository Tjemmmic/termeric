//! Claude Code's stream-json protocol.
//!
//! Claude Code runs headless with `-p --input-format stream-json
//! --output-format stream-json`: every line it writes is one JSON message and
//! every line it reads is a user message or a control message. With
//! `--permission-prompt-tool stdio` it asks its host, over the same pipes,
//! before any action its permission rules do not already allow. The official
//! Agent SDKs speak this protocol, but Claude Code does not document it as a
//! stable interface, so decoding here is forgiving: it reads only the fields it
//! needs, ignores message kinds it does not know, and never fails a whole line
//! because one optional field changed shape.

use std::collections::HashMap;
use std::time::Duration;

use serde_json::{Map, Value, json};

use super::event::{
    AgentCommand, AgentEvent, ApprovalDecision, ApprovalRequest, AssistantMessage, BlockStart,
    ContentBlock, ContextUsage, Handshake, ModeChoice, ModelChoice, RememberOption, Retry,
    SessionInfo, SlashCommand, StreamDelta, Thread, ToolResult, TurnOutcome,
};
use super::protocol::{Exchange, Protocol, line, preview};

/// The oldest Claude Code release whose protocol the agent view was built
/// against.
pub const MINIMUM_VERSION: (u32, u32, u32) = (2, 1, 0);

/// How the agent is started.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LaunchOptions {
    /// The id of the mode to start in, one of [`modes`].
    pub mode: String,
    /// A model value from the handshake; `None` keeps Claude's default.
    pub model: Option<String>,
    /// Continue an earlier conversation by its session id.
    pub resume: Option<String>,
    /// Let the session switch to the mode that skips every check, which
    /// Claude Code refuses unless launched with this allowance.
    pub allow_bypass: bool,
}

impl Default for LaunchOptions {
    fn default() -> Self {
        Self {
            mode: ASK_MODE.to_owned(),
            model: None,
            resume: None,
            allow_bypass: false,
        }
    }
}

/// Arguments for a headless Claude Code process speaking this protocol.
pub fn launch_args(options: &LaunchOptions) -> Vec<String> {
    let mut args: Vec<String> = [
        "-p",
        "--input-format",
        "stream-json",
        "--output-format",
        "stream-json",
        "--verbose",
        "--include-partial-messages",
        "--permission-prompt-tool",
        "stdio",
        "--permission-mode",
        options.mode.as_str(),
    ]
    .into_iter()
    .map(String::from)
    .collect();
    if let Some(model) = &options.model {
        args.extend(["--model".into(), model.clone()]);
    }
    if let Some(session) = &options.resume {
        args.extend(["--resume".into(), session.clone()]);
    }
    if options.allow_bypass {
        args.push("--allow-dangerously-skip-permissions".into());
    }
    args
}

/// Reads a `claude --version` line such as `2.1.282 (Claude Code)`.
pub fn parse_version(output: &str) -> Option<(u32, u32, u32)> {
    let token = output.split_whitespace().next()?;
    let mut parts = token.split('.').map(|part| {
        part.chars()
            .take_while(char::is_ascii_digit)
            .collect::<String>()
            .parse::<u32>()
            .ok()
    });
    Some((parts.next()??, parts.next()??, parts.next()??))
}

/// What a control request the panel sent was for, so its reply can be read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RequestKind {
    Initialize,
    Interrupt,
    SetMode,
    SetModel,
    ContextUsage,
}

/// Encodes commands and decodes output for one Claude Code process.
#[derive(Debug, Default)]
pub struct ClaudeCodec {
    next_request: u64,
    pending: HashMap<String, RequestKind>,
}

impl ClaudeCodec {
    pub fn new() -> Self {
        Self::default()
    }

    /// The first line to send: asks for models, commands, and the mode.
    pub fn initialize(&mut self) -> String {
        self.control_request(RequestKind::Initialize, json!({ "subtype": "initialize" }))
    }

    /// One line of protocol input for `command`.
    pub fn encode_command(&mut self, command: &AgentCommand) -> String {
        match command {
            AgentCommand::Prompt(text) => line(&json!({
                "type": "user",
                "session_id": "",
                "message": {
                    "role": "user",
                    "content": [{ "type": "text", "text": text }],
                },
                "parent_tool_use_id": null,
            })),
            AgentCommand::Answer {
                request_id,
                tool_use_id,
                decision,
            } => {
                let mut answer = match decision {
                    ApprovalDecision::Allow { input, remember } => {
                        let mut answer = json!({ "behavior": "allow", "updatedInput": input });
                        if let Some(updates) = remember {
                            answer["updatedPermissions"] = updates.clone();
                        }
                        answer
                    }
                    ApprovalDecision::Deny { message, interrupt } => json!({
                        "behavior": "deny",
                        "message": message,
                        "interrupt": interrupt,
                    }),
                };
                if let Some(id) = tool_use_id {
                    answer["toolUseID"] = Value::String(id.clone());
                }
                line(&json!({
                    "type": "control_response",
                    "response": {
                        "subtype": "success",
                        "request_id": request_id,
                        "response": answer,
                    },
                }))
            }
            AgentCommand::Interrupt => {
                self.control_request(RequestKind::Interrupt, json!({ "subtype": "interrupt" }))
            }
            AgentCommand::SetMode(mode) => self.control_request(
                RequestKind::SetMode,
                json!({ "subtype": "set_permission_mode", "mode": mode }),
            ),
            AgentCommand::SetModel(model) => self.control_request(
                RequestKind::SetModel,
                json!({ "subtype": "set_model", "model": model }),
            ),
            AgentCommand::QueryContextUsage => self.control_request(
                RequestKind::ContextUsage,
                json!({ "subtype": "get_context_usage" }),
            ),
        }
    }

    /// A refusal for a control request this host does not handle, so the
    /// agent never waits on it.
    pub fn refuse(request_id: &str, reason: &str) -> String {
        line(&json!({
            "type": "control_response",
            "response": {
                "subtype": "error",
                "request_id": request_id,
                "error": reason,
            },
        }))
    }

    /// The events one output line carries, plus any reply the agent must
    /// receive immediately (a refusal of a request this host cannot serve).
    pub fn decode_line(&mut self, text: &str) -> Exchange {
        let mut decoded = Exchange::default();
        let Ok(Value::Object(message)) = serde_json::from_str::<Value>(text) else {
            decoded.events.push(AgentEvent::Unrecognized(preview(text)));
            return decoded;
        };
        match str_field(&message, "type") {
            Some("stream_event") => {
                if let Some(delta) = message.get("event").and_then(stream_delta) {
                    decoded.events.push(AgentEvent::Stream {
                        thread: thread_of(&message),
                        delta,
                    });
                }
            }
            Some("assistant") => {
                if let Some(assistant) = assistant_message(&message) {
                    decoded.events.push(AgentEvent::Message {
                        thread: thread_of(&message),
                        message: assistant,
                    });
                }
            }
            Some("user") => {
                let thread = thread_of(&message);
                decoded
                    .events
                    .extend(tool_results(&message).into_iter().map(|result| {
                        AgentEvent::ToolResult {
                            thread: thread.clone(),
                            result,
                        }
                    }));
            }
            Some("system") => decoded.events.extend(system_events(&message)),
            Some("result") => decoded
                .events
                .push(AgentEvent::TurnFinished(turn_outcome(&message))),
            Some("control_request") => self.decode_control_request(&message, &mut decoded),
            Some("control_cancel_request") => {
                if let Some(request_id) = str_field(&message, "request_id") {
                    decoded.events.push(AgentEvent::ApprovalWithdrawn {
                        request_id: request_id.to_owned(),
                    });
                }
            }
            Some("control_response") => {
                if let Some(event) = self.decode_control_response(&message) {
                    decoded.events.push(event);
                }
            }
            // Progress, rate-limit, hook, and summary messages carry nothing
            // the panel shows that the messages above do not.
            Some(_) => {}
            None => decoded.events.push(AgentEvent::Unrecognized(preview(text))),
        }
        decoded
    }

    fn control_request(&mut self, kind: RequestKind, request: Value) -> String {
        self.next_request += 1;
        let request_id = format!("termeric-{}", self.next_request);
        self.pending.insert(request_id.clone(), kind);
        line(&json!({
            "type": "control_request",
            "request_id": request_id,
            "request": request,
        }))
    }

    fn decode_control_request(&mut self, message: &Map<String, Value>, decoded: &mut Exchange) {
        let Some(request_id) = str_field(message, "request_id") else {
            return;
        };
        let request = message.get("request").and_then(Value::as_object);
        match request.and_then(|request| str_field(request, "subtype")) {
            Some("can_use_tool") => {
                if let Some(approval) = request.and_then(|request| approval(request_id, request)) {
                    decoded.events.push(AgentEvent::ApprovalRequested(approval));
                } else {
                    decoded
                        .lines
                        .push(Self::refuse(request_id, "malformed permission request"));
                }
            }
            other => {
                let subtype = other.unwrap_or("unknown");
                decoded.lines.push(Self::refuse(
                    request_id,
                    &format!("Termeric does not handle {subtype} requests"),
                ));
            }
        }
    }

    fn decode_control_response(&mut self, message: &Map<String, Value>) -> Option<AgentEvent> {
        let response = message.get("response")?.as_object()?;
        let request_id = str_field(response, "request_id")?;
        let kind = self.pending.remove(request_id)?;
        if str_field(response, "subtype") != Some("success") {
            let error = str_field(response, "error")
                .unwrap_or("the agent refused the request")
                .to_owned();
            return match kind {
                RequestKind::Initialize => Some(AgentEvent::HandshakeFailed(error)),
                // Asked in the background after every turn; a refusal says
                // nothing the person could act on.
                RequestKind::ContextUsage => {
                    log::debug!("agent view: context usage unavailable: {error}");
                    None
                }
                RequestKind::Interrupt | RequestKind::SetMode | RequestKind::SetModel => {
                    Some(AgentEvent::RequestFailed {
                        request_id: request_id.to_owned(),
                        error,
                    })
                }
            };
        }
        let body = response.get("response").and_then(Value::as_object);
        match kind {
            RequestKind::Initialize => Some(AgentEvent::Ready(handshake(body))),
            RequestKind::ContextUsage => {
                let body = body?;
                Some(AgentEvent::ContextUsage(ContextUsage {
                    used_tokens: body.get("totalTokens")?.as_u64()?,
                    max_tokens: body.get("maxTokens")?.as_u64()?,
                }))
            }
            RequestKind::Interrupt | RequestKind::SetMode | RequestKind::SetModel => None,
        }
    }
}

impl Protocol for ClaudeCodec {
    fn start(&mut self) -> Vec<String> {
        vec![self.initialize()]
    }

    fn encode(&mut self, command: &AgentCommand) -> Exchange {
        Exchange::send(vec![self.encode_command(command)])
    }

    fn decode(&mut self, line: &str) -> Exchange {
        let mut decoded = self.decode_line(line);
        let mut warnings = Vec::new();
        for event in &decoded.events {
            match event {
                AgentEvent::SessionStarted(info) => {
                    if let Some(version) = parse_version(&info.version)
                        && version < MINIMUM_VERSION
                    {
                        warnings.push(AgentEvent::Warning(format!(
                            "Claude Code {} is older than the agent view supports; \
                             update it if anything looks wrong",
                            info.version
                        )));
                    }
                }
                // Claude Code reports how full the context is only when asked.
                AgentEvent::TurnFinished(_) => decoded
                    .lines
                    .push(self.encode_command(&AgentCommand::QueryContextUsage)),
                _ => {}
            }
        }
        decoded.events.extend(warnings);
        decoded
    }
}

fn str_field<'a>(object: &'a Map<String, Value>, key: &str) -> Option<&'a str> {
    object.get(key).and_then(Value::as_str)
}

fn string_field(object: &Map<String, Value>, key: &str) -> Option<String> {
    str_field(object, key)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
}

fn thread_of(message: &Map<String, Value>) -> Thread {
    match str_field(message, "parent_tool_use_id") {
        Some(parent) => Thread::SubAgent {
            parent_tool_use_id: parent.to_owned(),
        },
        None => Thread::Main,
    }
}

/// The mode Claude Code asks in, the one it starts in by default.
pub const ASK_MODE: &str = "default";

/// Claude Code's permission modes, which it does not list itself.
pub fn modes() -> Vec<ModeChoice> {
    vec![
        ModeChoice::new(
            ASK_MODE,
            "Ask before edits",
            "Approve each edit and command",
        ),
        ModeChoice::new(
            "acceptEdits",
            "Edit without asking",
            "Edits apply at once; commands still ask",
        ),
        ModeChoice::new(
            "plan",
            "Plan only",
            "Read and plan, then ask before changing anything",
        )
        .plans(),
        ModeChoice::new(
            "auto",
            "Auto",
            "A safety classifier approves or blocks each action",
        ),
        ModeChoice::new(
            "dontAsk",
            "Deny unless allowed",
            "Only actions already allowed run; the rest are refused",
        ),
        ModeChoice::new(
            "bypassPermissions",
            "Bypass permissions",
            "Everything runs without asking. Use only in a sandbox",
        )
        .unrestricted(),
    ]
}

/// A mode as Claude Code reports it, by the id [`modes`] uses.
fn parse_mode(name: &str) -> Option<String> {
    match name {
        // Claude Code 2.1.283 renamed `default` to `manual`.
        "manual" => Some(ASK_MODE.to_owned()),
        name if modes().iter().any(|mode| mode.id == name) => Some(name.to_owned()),
        _ => None,
    }
}

fn handshake(body: Option<&Map<String, Value>>) -> Handshake {
    let Some(body) = body else {
        return Handshake {
            modes: modes(),
            ..Handshake::default()
        };
    };
    let objects = |key: &str| {
        body.get(key)
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_object)
    };
    let models = objects("models")
        .filter_map(|model| {
            let value = string_field(model, "value")?;
            Some(ModelChoice {
                display_name: string_field(model, "displayName").unwrap_or_else(|| value.clone()),
                description: string_field(model, "description").unwrap_or_default(),
                value,
            })
        })
        .collect();
    let commands = objects("commands")
        .filter_map(|command| {
            Some(SlashCommand {
                name: string_field(command, "name")?,
                description: string_field(command, "description").unwrap_or_default(),
                argument_hint: string_field(command, "argumentHint").unwrap_or_default(),
            })
        })
        .collect();
    Handshake {
        models,
        commands,
        modes: modes(),
        permission_mode: str_field(body, "current_permission_mode").and_then(parse_mode),
    }
}

fn stream_delta(event: &Value) -> Option<StreamDelta> {
    let event = event.as_object()?;
    let index = || {
        event
            .get("index")
            .and_then(Value::as_u64)
            .and_then(|index| u32::try_from(index).ok())
    };
    match str_field(event, "type")? {
        "message_start" => Some(StreamDelta::MessageStarted {
            message_id: event.get("message")?.get("id")?.as_str()?.to_owned(),
        }),
        "content_block_start" => {
            let block = event.get("content_block")?.as_object()?;
            let block = match str_field(block, "type")? {
                "text" => BlockStart::Text,
                "thinking" => BlockStart::Thinking,
                "tool_use" | "server_tool_use" => BlockStart::ToolUse {
                    id: str_field(block, "id")?.to_owned(),
                    name: str_field(block, "name")?.to_owned(),
                },
                _ => return None,
            };
            Some(StreamDelta::BlockStarted {
                index: index()?,
                block,
            })
        }
        "content_block_delta" => {
            let delta = event.get("delta")?.as_object()?;
            let index = index()?;
            match str_field(delta, "type")? {
                "text_delta" => Some(StreamDelta::Text {
                    index,
                    text: str_field(delta, "text")?.to_owned(),
                }),
                "thinking_delta" => Some(StreamDelta::Thinking {
                    index,
                    text: str_field(delta, "thinking")?.to_owned(),
                }),
                "input_json_delta" => Some(StreamDelta::ToolInput {
                    index,
                    partial_json: str_field(delta, "partial_json")?.to_owned(),
                }),
                _ => None,
            }
        }
        "content_block_stop" => Some(StreamDelta::BlockStopped { index: index()? }),
        _ => None,
    }
}

fn assistant_message(message: &Map<String, Value>) -> Option<AssistantMessage> {
    let inner = message.get("message")?.as_object()?;
    let blocks = inner
        .get("content")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_object)
        .filter_map(|block| match str_field(block, "type")? {
            "text" => Some(ContentBlock::Text(str_field(block, "text")?.to_owned())),
            "thinking" => Some(ContentBlock::Thinking(
                str_field(block, "thinking")?.to_owned(),
            )),
            "tool_use" | "server_tool_use" => Some(ContentBlock::ToolUse {
                id: str_field(block, "id")?.to_owned(),
                name: str_field(block, "name")?.to_owned(),
                input: block.get("input").cloned().unwrap_or(Value::Null),
            }),
            _ => None,
        })
        .collect();
    Some(AssistantMessage {
        id: str_field(inner, "id").unwrap_or_default().to_owned(),
        blocks,
        error: str_field(message, "error").map(assistant_error),
    })
}

fn assistant_error(code: &str) -> String {
    match code {
        "authentication_failed" => "Claude Code is not signed in".into(),
        "oauth_org_not_allowed" => "This account's organization cannot use Claude Code".into(),
        "billing_error" => "The account has a billing problem".into(),
        "rate_limit" => "The usage limit was reached".into(),
        "invalid_request" => "Claude rejected the request".into(),
        "server_error" => "Claude's service had an error".into(),
        "max_output_tokens" => "The reply reached its length limit".into(),
        other => format!("The request failed ({other})"),
    }
}

fn tool_results(message: &Map<String, Value>) -> Vec<ToolResult> {
    let Some(content) = message
        .get("message")
        .and_then(|inner| inner.get("content"))
        .and_then(Value::as_array)
    else {
        return Vec::new();
    };
    let detail = message
        .get("tool_use_result")
        .filter(|detail| detail.is_object());
    let results: Vec<_> = content
        .iter()
        .filter_map(Value::as_object)
        .filter(|block| str_field(block, "type") == Some("tool_result"))
        .filter_map(|block| {
            Some(ToolResult {
                tool_use_id: str_field(block, "tool_use_id")?.to_owned(),
                content: result_text(block.get("content")),
                is_error: block
                    .get("is_error")
                    .and_then(Value::as_bool)
                    .unwrap_or(false),
                detail: None,
            })
        })
        .collect();
    // The structured result describes the message's only tool result; with
    // several there is no telling which one it belongs to.
    match (results.len(), detail) {
        (1, Some(detail)) => results
            .into_iter()
            .map(|result| ToolResult {
                detail: Some(detail.clone()),
                ..result
            })
            .collect(),
        _ => results,
    }
}

fn result_text(content: Option<&Value>) -> String {
    match content {
        Some(Value::String(text)) => text.clone(),
        Some(Value::Array(parts)) => parts
            .iter()
            .filter_map(|part| {
                let part = part.as_object()?;
                match str_field(part, "type")? {
                    "text" => str_field(part, "text").map(str::to_owned),
                    "image" => Some("[image]".to_owned()),
                    _ => None,
                }
            })
            .collect::<Vec<_>>()
            .join("\n"),
        _ => String::new(),
    }
}

fn system_events(message: &Map<String, Value>) -> Vec<AgentEvent> {
    match str_field(message, "subtype") {
        Some("init") => str_field(message, "session_id")
            .map(|session_id| {
                AgentEvent::SessionStarted(SessionInfo {
                    session_id: session_id.to_owned(),
                    model: str_field(message, "model").unwrap_or_default().to_owned(),
                    cwd: str_field(message, "cwd").unwrap_or_default().to_owned(),
                    permission_mode: str_field(message, "permissionMode").and_then(parse_mode),
                    version: str_field(message, "claude_code_version")
                        .unwrap_or_default()
                        .to_owned(),
                })
            })
            .into_iter()
            .collect(),
        // One status message can report both a mode change and compaction.
        Some("status") => {
            let mut events = Vec::new();
            if let Some(mode) = str_field(message, "permissionMode").and_then(parse_mode) {
                events.push(AgentEvent::ModeChanged(mode));
            }
            if message.contains_key("status") {
                events.push(AgentEvent::Compacting(
                    str_field(message, "status") == Some("compacting"),
                ));
            }
            events
        }
        Some("api_retry") => vec![AgentEvent::Retrying(Retry {
            attempt: number(message, "attempt") as u32,
            max_attempts: number(message, "max_retries") as u32,
            delay: Duration::from_millis(number(message, "retry_delay_ms")),
            error: str_field(message, "error")
                .map(assistant_error)
                .unwrap_or_else(|| "The request failed".into()),
        })],
        _ => Vec::new(),
    }
}

fn number(object: &Map<String, Value>, key: &str) -> u64 {
    object.get(key).and_then(Value::as_u64).unwrap_or(0)
}

fn turn_outcome(message: &Map<String, Value>) -> TurnOutcome {
    let is_error = message
        .get("is_error")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let success = str_field(message, "subtype") == Some("success") && !is_error;
    let error = (!success).then(|| {
        let listed: Vec<&str> = message
            .get("errors")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .collect();
        if !listed.is_empty() {
            return listed.join("; ");
        }
        match str_field(message, "subtype") {
            Some("error_max_turns") => "The turn reached its step limit".to_owned(),
            Some("error_max_budget_usd") => "The turn reached its budget".to_owned(),
            _ => string_field(message, "result")
                .unwrap_or_else(|| "The turn ended with an error".to_owned()),
        }
    });
    TurnOutcome {
        success,
        error,
        duration: Duration::from_millis(number(message, "duration_ms")),
        cost_usd: message.get("total_cost_usd").and_then(Value::as_f64),
        session_id: str_field(message, "session_id").map(str::to_owned),
        denials: message
            .get("permission_denials")
            .and_then(Value::as_array)
            .map_or(0, Vec::len),
        // Claude Code reports a stopped turn only as its error, if at all.
        interrupted: false,
    }
}

fn approval(request_id: &str, request: &Map<String, Value>) -> Option<ApprovalRequest> {
    Some(ApprovalRequest {
        request_id: request_id.to_owned(),
        tool_use_id: str_field(request, "tool_use_id").map(str::to_owned),
        tool_name: str_field(request, "tool_name")?.to_owned(),
        input: request.get("input").cloned().unwrap_or(Value::Null),
        title: string_field(request, "title"),
        description: string_field(request, "description"),
        reason: string_field(request, "blocked_path")
            .map(|path| format!("It needs {path}"))
            .or_else(|| string_field(request, "decision_reason")),
        remember: request
            .get("permission_suggestions")
            .and_then(Value::as_array)
            .filter(|suggestions| !suggestions.is_empty())
            .and_then(|suggestions| remember_option(suggestions)),
        from_sub_agent: request.get("agent_id").is_some_and(|id| !id.is_null()),
    })
}

/// Words for Claude's suggested rule updates, such as "Always allow
/// cargo test in this project". Suggestions this cannot describe are not
/// offered: a person must know what they are agreeing to.
fn remember_option(suggestions: &[Value]) -> Option<RememberOption> {
    let mut subjects = Vec::new();
    let mut place = None;
    for suggestion in suggestions {
        let suggestion = suggestion.as_object()?;
        let destination = match str_field(suggestion, "destination")? {
            "session" | "cliArg" => "for this session",
            "localSettings" | "projectSettings" => "in this project",
            "userSettings" => "everywhere",
            _ => return None,
        };
        if place.is_some_and(|place| place != destination) {
            return None;
        }
        place = Some(destination);
        match str_field(suggestion, "type")? {
            "addRules" if str_field(suggestion, "behavior") == Some("allow") => {
                for rule in suggestion.get("rules")?.as_array()? {
                    subjects.push(rule_subject(rule.as_object()?)?);
                }
            }
            "setMode" if str_field(suggestion, "mode") == Some("acceptEdits") => {
                subjects.push("all edits".to_owned());
            }
            "addDirectories" => {
                for directory in suggestion.get("directories")?.as_array()? {
                    subjects.push(format!("access to {}", directory.as_str()?));
                }
            }
            _ => return None,
        }
    }
    if subjects.is_empty() {
        return None;
    }
    Some(RememberOption {
        label: format!("Always allow {} {}", subjects.join(", "), place?),
        updates: Value::Array(suggestions.to_vec()),
    })
}

fn rule_subject(rule: &Map<String, Value>) -> Option<String> {
    let tool = str_field(rule, "toolName")?;
    let content = str_field(rule, "ruleContent")
        .map(str::trim)
        .filter(|content| !content.is_empty());
    Some(match (tool, content) {
        ("Bash", Some(content)) => content
            .strip_suffix(":*")
            .or_else(|| content.strip_suffix(" *"))
            .unwrap_or(content)
            .to_owned(),
        ("Read", Some(content)) => format!("reading {content}"),
        ("Edit" | "Write" | "MultiEdit" | "NotebookEdit", Some(content)) => {
            format!("edits to {content}")
        }
        ("Edit" | "Write" | "MultiEdit" | "NotebookEdit", None) => "edits".to_owned(),
        ("WebFetch", Some(content)) => content
            .strip_prefix("domain:")
            .map_or_else(|| content.to_owned(), |domain| format!("fetching {domain}")),
        (tool, Some(content)) => format!("{tool} {content}"),
        (tool, None) => tool.to_owned(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn decode(codec: &mut ClaudeCodec, value: Value) -> Exchange {
        codec.decode_line(&value.to_string())
    }

    fn only_event(codec: &mut ClaudeCodec, value: Value) -> AgentEvent {
        let mut decoded = decode(codec, value);
        assert!(decoded.lines.is_empty(), "{:?}", decoded.lines);
        assert_eq!(decoded.events.len(), 1, "{:?}", decoded.events);
        decoded.events.remove(0)
    }

    fn parse(line: &str) -> Value {
        assert!(line.ends_with('\n'));
        serde_json::from_str(line).unwrap()
    }

    #[test]
    fn launch_arguments_select_stdio_permissions_mode_model_and_resume() {
        let args = launch_args(&LaunchOptions {
            mode: "plan".into(),
            model: Some("sonnet".into()),
            resume: Some("abc".into()),
            allow_bypass: false,
        });
        let joined = args.join(" ");
        assert!(joined.starts_with("-p --input-format stream-json --output-format stream-json"));
        assert!(joined.contains("--permission-prompt-tool stdio"));
        assert!(joined.contains("--permission-mode plan"));
        assert!(joined.ends_with("--model sonnet --resume abc"));
        let plain = launch_args(&LaunchOptions::default()).join(" ");
        assert!(plain.contains("--permission-mode default"));
        assert!(!plain.contains("--model") && !plain.contains("--resume"));
        assert!(!plain.contains("dangerously"));
        let allowed = launch_args(&LaunchOptions {
            allow_bypass: true,
            ..LaunchOptions::default()
        });
        assert!(allowed.contains(&"--allow-dangerously-skip-permissions".to_owned()));
    }

    #[test]
    fn versions_are_read_from_the_version_line() {
        assert_eq!(parse_version("2.1.282 (Claude Code)"), Some((2, 1, 282)));
        assert_eq!(parse_version("2.0.0-beta.1"), Some((2, 0, 0)));
        assert_eq!(parse_version("Claude Code"), None);
        assert_eq!(parse_version(""), None);
    }

    #[test]
    fn the_handshake_reports_models_commands_and_mode() {
        let mut codec = ClaudeCodec::new();
        let request = parse(&codec.initialize());
        assert_eq!(request["type"], "control_request");
        assert_eq!(request["request"]["subtype"], "initialize");
        let id = request["request_id"].as_str().unwrap().to_owned();
        let event = only_event(
            &mut codec,
            json!({"type": "control_response", "response": {
                "subtype": "success", "request_id": id, "response": {
                    "models": [
                        {"value": "sonnet", "displayName": "Sonnet 5", "description": "Everyday"},
                        {"value": "haiku"},
                        {"displayName": "no value"}
                    ],
                    "commands": [{"name": "compact", "description": "Summarize", "argumentHint": ""}],
                    "current_permission_mode": "acceptEdits"
                }
            }}),
        );
        let AgentEvent::Ready(handshake) = event else {
            panic!("{event:?}")
        };
        assert_eq!(handshake.models.len(), 2);
        assert_eq!(handshake.models[0].display_name, "Sonnet 5");
        assert_eq!(handshake.models[1].display_name, "haiku");
        assert_eq!(handshake.commands[0].name, "compact");
        assert_eq!(handshake.permission_mode, Some("acceptEdits".into()));
        // A reply is consumed once; a repeat or an unknown id is ignored.
        let repeat = decode(
            &mut codec,
            json!({"type": "control_response", "response": {"subtype": "success", "request_id": id}}),
        );
        assert!(repeat.events.is_empty());
    }

    #[test]
    fn prompts_and_answers_encode_as_the_sdk_does() {
        let mut codec = ClaudeCodec::new();
        let prompt = parse(&codec.encode_command(&AgentCommand::Prompt("hi".into())));
        assert_eq!(prompt["type"], "user");
        assert_eq!(prompt["message"]["content"][0]["text"], "hi");
        let allow = parse(&codec.encode_command(&AgentCommand::Answer {
            request_id: "r1".into(),
            tool_use_id: Some("t1".into()),
            decision: ApprovalDecision::Allow {
                input: json!({"command": "ls"}),
                remember: Some(json!([{"type": "addRules"}])),
            },
        }));
        assert_eq!(allow["response"]["request_id"], "r1");
        assert_eq!(allow["response"]["response"]["behavior"], "allow");
        assert_eq!(
            allow["response"]["response"]["updatedInput"]["command"],
            "ls"
        );
        assert_eq!(allow["response"]["response"]["toolUseID"], "t1");
        assert!(allow["response"]["response"]["updatedPermissions"].is_array());
        let deny = parse(&codec.encode_command(&AgentCommand::Answer {
            request_id: "r2".into(),
            tool_use_id: None,
            decision: ApprovalDecision::Deny {
                message: "no".into(),
                interrupt: true,
            },
        }));
        assert_eq!(deny["response"]["response"]["behavior"], "deny");
        assert_eq!(deny["response"]["response"]["interrupt"], true);
        let mode = parse(&codec.encode_command(&AgentCommand::SetMode("plan".into())));
        assert_eq!(mode["request"]["mode"], "plan");
    }

    #[test]
    fn failed_control_requests_are_reported() {
        let mut codec = ClaudeCodec::new();
        let request = parse(&codec.encode_command(&AgentCommand::SetModel("nope".into())));
        let id = request["request_id"].as_str().unwrap().to_owned();
        let event = only_event(
            &mut codec,
            json!({"type": "control_response", "response": {
                "subtype": "error", "request_id": id, "error": "unknown model"
            }}),
        );
        assert_eq!(
            event,
            AgentEvent::RequestFailed {
                request_id: id,
                error: "unknown model".into()
            }
        );

        let failure = |id: &str| {
            json!({"type": "control_response", "response": {
                "subtype": "error", "request_id": id, "error": "not supported"
            }})
        };
        // A refused connection is its own event: nothing else will work.
        let request = parse(&codec.initialize());
        let id = request["request_id"].as_str().unwrap().to_owned();
        assert_eq!(
            only_event(&mut codec, failure(&id)),
            AgentEvent::HandshakeFailed("not supported".into())
        );
        // A refused background query is not the person's concern.
        let request = parse(&codec.encode_command(&AgentCommand::QueryContextUsage));
        let id = request["request_id"].as_str().unwrap().to_owned();
        assert!(decode(&mut codec, failure(&id)).events.is_empty());
    }

    #[test]
    fn context_usage_replies_become_usage_events() {
        let mut codec = ClaudeCodec::new();
        let request = parse(&codec.encode_command(&AgentCommand::QueryContextUsage));
        let id = request["request_id"].as_str().unwrap().to_owned();
        let event = only_event(
            &mut codec,
            json!({"type": "control_response", "response": {
                "subtype": "success", "request_id": id,
                "response": {"totalTokens": 50000, "maxTokens": 200000, "categories": []}
            }}),
        );
        assert_eq!(
            event,
            AgentEvent::ContextUsage(ContextUsage {
                used_tokens: 50000,
                max_tokens: 200000
            })
        );
    }

    #[test]
    fn streamed_output_becomes_deltas_on_the_right_thread() {
        let mut codec = ClaudeCodec::new();
        let stream = |event: Value, parent: Value| json!({"type": "stream_event", "event": event, "parent_tool_use_id": parent});
        assert_eq!(
            only_event(
                &mut codec,
                stream(
                    json!({"type": "message_start", "message": {"id": "m1"}}),
                    Value::Null
                )
            ),
            AgentEvent::Stream {
                thread: Thread::Main,
                delta: StreamDelta::MessageStarted {
                    message_id: "m1".into()
                }
            }
        );
        assert_eq!(
            only_event(
                &mut codec,
                stream(
                    json!({"type": "content_block_start", "index": 1,
                        "content_block": {"type": "tool_use", "id": "t1", "name": "Bash", "input": {}}}),
                    json!("parent")
                )
            ),
            AgentEvent::Stream {
                thread: Thread::SubAgent {
                    parent_tool_use_id: "parent".into()
                },
                delta: StreamDelta::BlockStarted {
                    index: 1,
                    block: BlockStart::ToolUse {
                        id: "t1".into(),
                        name: "Bash".into()
                    }
                }
            }
        );
        assert_eq!(
            only_event(
                &mut codec,
                stream(
                    json!({"type": "content_block_delta", "index": 0,
                        "delta": {"type": "text_delta", "text": "Hel"}}),
                    Value::Null
                )
            ),
            AgentEvent::Stream {
                thread: Thread::Main,
                delta: StreamDelta::Text {
                    index: 0,
                    text: "Hel".into()
                }
            }
        );
        // Signature deltas and message-level events carry nothing shown.
        assert!(
            decode(
                &mut codec,
                stream(
                    json!({"type": "content_block_delta", "index": 0,
                        "delta": {"type": "signature_delta", "signature": "x"}}),
                    Value::Null
                )
            )
            .events
            .is_empty()
        );
        assert!(
            decode(
                &mut codec,
                stream(json!({"type": "message_stop"}), Value::Null)
            )
            .events
            .is_empty()
        );
    }

    #[test]
    fn assistant_messages_keep_text_thinking_and_tool_calls() {
        let mut codec = ClaudeCodec::new();
        let event = only_event(
            &mut codec,
            json!({"type": "assistant", "parent_tool_use_id": null, "message": {
                "id": "m1", "content": [
                    {"type": "thinking", "thinking": "hmm", "signature": "s"},
                    {"type": "text", "text": "Done."},
                    {"type": "tool_use", "id": "t1", "name": "Read", "input": {"file_path": "/a"}},
                    {"type": "image", "source": {}}
                ]
            }}),
        );
        let AgentEvent::Message { thread, message } = event else {
            panic!("{event:?}")
        };
        assert_eq!(thread, Thread::Main);
        assert_eq!(message.id, "m1");
        assert_eq!(
            message.blocks,
            vec![
                ContentBlock::Thinking("hmm".into()),
                ContentBlock::Text("Done.".into()),
                ContentBlock::ToolUse {
                    id: "t1".into(),
                    name: "Read".into(),
                    input: json!({"file_path": "/a"})
                }
            ]
        );
        assert_eq!(message.error, None);
        let failed = only_event(
            &mut codec,
            json!({"type": "assistant", "error": "rate_limit", "message": {"id": "m2", "content": []}}),
        );
        let AgentEvent::Message { message, .. } = failed else {
            panic!()
        };
        assert_eq!(
            message.error.as_deref(),
            Some("The usage limit was reached")
        );
    }

    #[test]
    fn tool_results_carry_text_errors_and_a_single_structured_detail() {
        let mut codec = ClaudeCodec::new();
        let event = only_event(
            &mut codec,
            json!({"type": "user", "parent_tool_use_id": null,
            "tool_use_result": {"structuredPatch": []},
            "message": {"role": "user", "content": [
                {"type": "tool_result", "tool_use_id": "t1", "is_error": true,
                 "content": [{"type": "text", "text": "a"}, {"type": "image"}, {"type": "text", "text": "b"}]}
            ]}}),
        );
        assert_eq!(
            event,
            AgentEvent::ToolResult {
                thread: Thread::Main,
                result: ToolResult {
                    tool_use_id: "t1".into(),
                    content: "a\n[image]\nb".into(),
                    is_error: true,
                    detail: Some(json!({"structuredPatch": []})),
                }
            }
        );
        let two = decode(
            &mut codec,
            json!({"type": "user", "tool_use_result": {"x": 1}, "message": {"content": [
                {"type": "tool_result", "tool_use_id": "a", "content": "one"},
                {"type": "tool_result", "tool_use_id": "b", "content": "two"}
            ]}}),
        );
        assert_eq!(two.events.len(), 2);
        assert!(two.events.iter().all(|event| matches!(
            event,
            AgentEvent::ToolResult { result, .. } if result.detail.is_none()
        )));
        // A plain echoed prompt carries no tool result.
        assert!(
            decode(
                &mut codec,
                json!({"type": "user", "message": {"content": "hello"}})
            )
            .events
            .is_empty()
        );
    }

    #[test]
    fn system_messages_report_the_session_mode_compaction_and_retries() {
        let mut codec = ClaudeCodec::new();
        assert_eq!(
            only_event(
                &mut codec,
                json!({"type": "system", "subtype": "init", "session_id": "s1", "model": "claude-sonnet-5",
                    "cwd": "/w", "permissionMode": "plan", "claude_code_version": "2.1.282", "tools": []})
            ),
            AgentEvent::SessionStarted(SessionInfo {
                session_id: "s1".into(),
                model: "claude-sonnet-5".into(),
                cwd: "/w".into(),
                permission_mode: Some("plan".into()),
                version: "2.1.282".into(),
            })
        );
        assert_eq!(
            decode(
                &mut codec,
                json!({"type": "system", "subtype": "status", "status": null, "permissionMode": "acceptEdits"})
            )
            .events,
            vec![
                AgentEvent::ModeChanged("acceptEdits".into()),
                AgentEvent::Compacting(false)
            ]
        );
        assert_eq!(
            only_event(
                &mut codec,
                json!({"type": "system", "subtype": "status", "status": "compacting"})
            ),
            AgentEvent::Compacting(true)
        );
        assert_eq!(
            only_event(
                &mut codec,
                json!({"type": "system", "subtype": "api_retry", "attempt": 2, "max_retries": 10,
                    "retry_delay_ms": 1500, "error": "server_error"})
            ),
            AgentEvent::Retrying(Retry {
                attempt: 2,
                max_attempts: 10,
                delay: Duration::from_millis(1500),
                error: "Claude's service had an error".into()
            })
        );
        assert!(
            decode(
                &mut codec,
                json!({"type": "system", "subtype": "hook_started"})
            )
            .events
            .is_empty()
        );
    }

    #[test]
    fn results_report_success_and_the_reason_for_failure() {
        let mut codec = ClaudeCodec::new();
        let AgentEvent::TurnFinished(done) = only_event(
            &mut codec,
            json!({"type": "result", "subtype": "success", "is_error": false, "duration_ms": 48000,
                "total_cost_usd": 0.08, "session_id": "s1", "permission_denials": [{}], "result": "ok"}),
        ) else {
            panic!()
        };
        assert!(done.success);
        assert_eq!(done.error, None);
        assert_eq!(done.duration, Duration::from_secs(48));
        assert_eq!(done.cost_usd, Some(0.08));
        assert_eq!(done.denials, 1);
        let AgentEvent::TurnFinished(api_error) = only_event(
            &mut codec,
            json!({"type": "result", "subtype": "success", "is_error": true, "result": "API Error: 529"}),
        ) else {
            panic!()
        };
        assert!(!api_error.success);
        assert_eq!(api_error.error.as_deref(), Some("API Error: 529"));
        let AgentEvent::TurnFinished(limited) = only_event(
            &mut codec,
            json!({"type": "result", "subtype": "error_max_turns", "is_error": true, "errors": []}),
        ) else {
            panic!()
        };
        assert_eq!(
            limited.error.as_deref(),
            Some("The turn reached its step limit")
        );
        let AgentEvent::TurnFinished(listed) = only_event(
            &mut codec,
            json!({"type": "result", "subtype": "error_during_execution", "errors": ["a", "b"]}),
        ) else {
            panic!()
        };
        assert_eq!(listed.error.as_deref(), Some("a; b"));
    }

    #[test]
    fn permission_requests_become_approvals_with_describable_rules() {
        let mut codec = ClaudeCodec::new();
        let event = only_event(
            &mut codec,
            json!({"type": "control_request", "request_id": "r1", "request": {
                "subtype": "can_use_tool", "tool_name": "Bash", "tool_use_id": "t1",
                "input": {"command": "cargo test"}, "description": "Run tests",
                "decision_reason": "Not in the allow list",
                "permission_suggestions": [{"type": "addRules", "behavior": "allow",
                    "destination": "localSettings",
                    "rules": [{"toolName": "Bash", "ruleContent": "cargo test:*"}]}]
            }}),
        );
        let AgentEvent::ApprovalRequested(approval) = event else {
            panic!("{event:?}")
        };
        assert_eq!(approval.request_id, "r1");
        assert_eq!(approval.tool_use_id.as_deref(), Some("t1"));
        assert_eq!(approval.description.as_deref(), Some("Run tests"));
        assert_eq!(approval.reason.as_deref(), Some("Not in the allow list"));
        assert!(!approval.from_sub_agent);
        let remember = approval.remember.unwrap();
        assert_eq!(remember.label, "Always allow cargo test in this project");
        assert!(remember.updates.is_array());
    }

    #[test]
    fn rules_are_offered_only_when_they_can_be_described() {
        let label = |suggestions: Value| {
            remember_option(suggestions.as_array().unwrap()).map(|option| option.label)
        };
        assert_eq!(
            label(json!([{"type": "setMode", "mode": "acceptEdits", "destination": "session"}])),
            Some("Always allow all edits for this session".into())
        );
        assert_eq!(
            label(
                json!([{"type": "addDirectories", "directories": ["/tmp/x"], "destination": "session"}])
            ),
            Some("Always allow access to /tmp/x for this session".into())
        );
        assert_eq!(
            label(
                json!([{"type": "addRules", "behavior": "allow", "destination": "userSettings",
                "rules": [{"toolName": "WebFetch", "ruleContent": "domain:docs.rs"}]}])
            ),
            Some("Always allow fetching docs.rs everywhere".into())
        );
        // Mixed destinations, removals, and unknown kinds are not offered.
        assert_eq!(
            label(json!([
                {"type": "setMode", "mode": "acceptEdits", "destination": "session"},
                {"type": "addDirectories", "directories": ["/x"], "destination": "userSettings"}
            ])),
            None
        );
        assert_eq!(
            label(
                json!([{"type": "removeRules", "behavior": "allow", "destination": "session", "rules": []}])
            ),
            None
        );
        assert_eq!(
            label(
                json!([{"type": "addRules", "behavior": "deny", "destination": "session",
                "rules": [{"toolName": "Bash"}]}])
            ),
            None
        );
    }

    #[test]
    fn unsupported_requests_are_refused_so_the_agent_never_waits() {
        let mut codec = ClaudeCodec::new();
        let decoded = decode(
            &mut codec,
            json!({"type": "control_request", "request_id": "r9", "request": {"subtype": "elicitation"}}),
        );
        assert!(decoded.events.is_empty());
        let reply = parse(&decoded.lines[0]);
        assert_eq!(reply["response"]["subtype"], "error");
        assert_eq!(reply["response"]["request_id"], "r9");
        let malformed = decode(
            &mut codec,
            json!({"type": "control_request", "request_id": "r10", "request": {"subtype": "can_use_tool"}}),
        );
        assert_eq!(malformed.lines.len(), 1);
    }

    #[test]
    fn cancelled_requests_withdraw_their_approval() {
        let mut codec = ClaudeCodec::new();
        assert_eq!(
            only_event(
                &mut codec,
                json!({"type": "control_cancel_request", "request_id": "r1"})
            ),
            AgentEvent::ApprovalWithdrawn {
                request_id: "r1".into()
            }
        );
    }

    #[test]
    fn unreadable_lines_are_reported_briefly_and_unknown_kinds_ignored() {
        let mut codec = ClaudeCodec::new();
        let long = "x".repeat(500);
        let AgentEvent::Unrecognized(text) = only_event_raw(&mut codec, &long) else {
            panic!()
        };
        assert_eq!(text.chars().count(), 201);
        assert!(
            decode(&mut codec, json!({"type": "rate_limit_event"}))
                .events
                .is_empty()
        );
        assert!(matches!(
            only_event(&mut codec, json!({"no": "type"})),
            AgentEvent::Unrecognized(_)
        ));
    }

    fn only_event_raw(codec: &mut ClaudeCodec, text: &str) -> AgentEvent {
        let mut decoded = codec.decode_line(text);
        assert_eq!(decoded.events.len(), 1);
        decoded.events.remove(0)
    }
}
