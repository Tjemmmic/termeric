//! What an agent reports and what the panel asks of it, independent of any
//! one agent's wire protocol.

use std::time::Duration;

use serde_json::Value;

/// How much the agent may do before it asks.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum PermissionMode {
    /// Ask before editing files or running commands.
    #[default]
    AskBeforeEdits,
    /// Edit files without asking; still ask before other actions.
    EditWithoutAsking,
    /// Read and plan only, then ask to start.
    PlanOnly,
    /// Never ask. Offered only when the person opted in.
    BypassPermissions,
    /// Refuse anything not already allowed, without asking.
    DontAsk,
    /// A safety classifier approves or blocks each action.
    Auto,
}

impl PermissionMode {
    /// The modes a person can choose, in menu order. The mode that skips
    /// every check is offered only when the person opted in to it.
    pub fn selectable(allow_bypass: bool) -> Vec<PermissionMode> {
        let mut modes = vec![
            PermissionMode::AskBeforeEdits,
            PermissionMode::EditWithoutAsking,
            PermissionMode::PlanOnly,
            PermissionMode::Auto,
            PermissionMode::DontAsk,
        ];
        if allow_bypass {
            modes.push(PermissionMode::BypassPermissions);
        }
        modes
    }

    pub fn label(self) -> &'static str {
        match self {
            PermissionMode::AskBeforeEdits => "Ask before edits",
            PermissionMode::EditWithoutAsking => "Edit without asking",
            PermissionMode::PlanOnly => "Plan only",
            PermissionMode::BypassPermissions => "Bypass permissions",
            PermissionMode::DontAsk => "Deny unless allowed",
            PermissionMode::Auto => "Auto",
        }
    }

    /// What choosing the mode means, for menus.
    pub fn description(self) -> &'static str {
        match self {
            PermissionMode::AskBeforeEdits => "Approve each edit and command",
            PermissionMode::EditWithoutAsking => "Edits apply at once; commands still ask",
            PermissionMode::PlanOnly => "Read and plan, then ask before changing anything",
            PermissionMode::Auto => "A safety classifier approves or blocks each action",
            PermissionMode::DontAsk => "Only actions already allowed run; the rest are refused",
            PermissionMode::BypassPermissions => {
                "Everything runs without asking. Use only in a sandbox"
            }
        }
    }

    /// Whether the mode lets actions run with no check at all.
    pub fn skips_every_check(self) -> bool {
        self == PermissionMode::BypassPermissions
    }
}

/// Whose conversation an event belongs to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Thread {
    /// The agent the person is talking to.
    Main,
    /// A sub-agent working inside one of the main agent's tool calls.
    SubAgent { parent_tool_use_id: String },
}

/// One model the agent offers.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ModelChoice {
    /// The value the agent accepts, e.g. `sonnet`.
    pub value: String,
    pub display_name: String,
    pub description: String,
}

/// Models offered before an agent has listed its own.
pub const FALLBACK_MODELS: [&str; 4] = ["default", "sonnet", "opus", "haiku"];

/// What to call a model value: the agent's own name for it when it listed
/// one, else a readable form of the value.
pub fn model_name(models: &[ModelChoice], value: &str) -> String {
    if let Some(model) = models.iter().find(|model| model.value == value) {
        return model
            .display_name
            .trim_end_matches(" (recommended)")
            .to_owned();
    }
    if value == "default" {
        return "Default model".to_owned();
    }
    let mut characters = value.chars();
    characters
        .next()
        .map(|first| first.to_uppercase().chain(characters).collect())
        .unwrap_or_default()
}

/// One slash command the agent offers.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SlashCommand {
    pub name: String,
    pub description: String,
    pub argument_hint: String,
}

/// What the agent reported when it accepted the connection.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Handshake {
    pub models: Vec<ModelChoice>,
    pub commands: Vec<SlashCommand>,
    pub permission_mode: Option<PermissionMode>,
}

/// The conversation the agent is running, reported when a turn starts.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SessionInfo {
    /// The agent's own conversation id, used to resume it later.
    pub session_id: String,
    pub model: String,
    pub cwd: String,
    pub permission_mode: Option<PermissionMode>,
    pub version: String,
}

/// Incremental output for the message being written.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StreamDelta {
    MessageStarted { message_id: String },
    BlockStarted { index: u32, block: BlockStart },
    Text { index: u32, text: String },
    Thinking { index: u32, text: String },
    ToolInput { index: u32, partial_json: String },
    BlockStopped { index: u32 },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BlockStart {
    Text,
    Thinking,
    ToolUse { id: String, name: String },
}

/// A complete assistant message. Its blocks are authoritative over anything
/// streamed for the same message.
#[derive(Clone, Debug, PartialEq)]
pub struct AssistantMessage {
    pub id: String,
    pub blocks: Vec<ContentBlock>,
    /// Set when the agent could not produce the message, e.g. a rate limit.
    pub error: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum ContentBlock {
    Text(String),
    Thinking(String),
    ToolUse {
        id: String,
        name: String,
        input: Value,
    },
}

/// The outcome of one tool call.
#[derive(Clone, Debug, PartialEq)]
pub struct ToolResult {
    pub tool_use_id: String,
    /// The text the model saw.
    pub content: String,
    pub is_error: bool,
    /// Structured output, such as an edit's patch, when the agent reports it.
    pub detail: Option<Value>,
}

/// A way to stop being asked about similar actions.
#[derive(Clone, Debug, PartialEq)]
pub struct RememberOption {
    /// What choosing it means, e.g. "Always allow cargo test in this project".
    pub label: String,
    /// The agent's own rule updates, returned unchanged when chosen.
    pub updates: Value,
}

/// The agent wants a decision before it continues.
#[derive(Clone, Debug, PartialEq)]
pub struct ApprovalRequest {
    pub request_id: String,
    pub tool_use_id: Option<String>,
    pub tool_name: String,
    pub input: Value,
    /// The agent's own one-line summary, when it gives one.
    pub title: Option<String>,
    pub description: Option<String>,
    /// Why the agent is asking, e.g. the path outside the project.
    pub reason: Option<String>,
    pub remember: Option<RememberOption>,
    /// Set when a sub-agent, not the main agent, is asking.
    pub from_sub_agent: bool,
}

/// How a turn ended.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TurnOutcome {
    pub success: bool,
    /// Why the turn failed, when it did.
    pub error: Option<String>,
    pub duration: Duration,
    pub cost_usd: Option<f64>,
    pub session_id: Option<String>,
    /// Tool calls the agent was refused during the turn.
    pub denials: usize,
}

/// The agent is waiting to retry a failed request.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Retry {
    pub attempt: u32,
    pub max_attempts: u32,
    pub delay: Duration,
    pub error: String,
}

/// How much of the model's context the conversation fills.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ContextUsage {
    pub used_tokens: u64,
    pub max_tokens: u64,
}

impl ContextUsage {
    pub fn fraction(self) -> f32 {
        if self.max_tokens == 0 {
            0.0
        } else {
            (self.used_tokens as f64 / self.max_tokens as f64).clamp(0.0, 1.0) as f32
        }
    }
}

/// How the agent process ended.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExitReport {
    pub success: bool,
    /// The exit status in words, e.g. "exit status 1".
    pub status: String,
    /// The end of what the agent wrote to stderr.
    pub stderr_tail: String,
}

/// Everything an agent can report to the panel.
#[derive(Clone, Debug, PartialEq)]
pub enum AgentEvent {
    Ready(Handshake),
    SessionStarted(SessionInfo),
    Stream {
        thread: Thread,
        delta: StreamDelta,
    },
    Message {
        thread: Thread,
        message: AssistantMessage,
    },
    ToolResult {
        thread: Thread,
        result: ToolResult,
    },
    ApprovalRequested(ApprovalRequest),
    /// The agent no longer needs an answer, e.g. after an interrupt.
    ApprovalWithdrawn {
        request_id: String,
    },
    ModeChanged(PermissionMode),
    Compacting(bool),
    Retrying(Retry),
    ContextUsage(ContextUsage),
    TurnFinished(TurnOutcome),
    /// The agent refused the connection, so it cannot be driven.
    HandshakeFailed(String),
    /// A request the panel made was refused.
    RequestFailed {
        request_id: String,
        error: String,
    },
    /// Output the panel could not understand. Logged, never shown verbatim.
    Unrecognized(String),
    Exited(ExitReport),
}

/// A person's answer to an [`ApprovalRequest`].
#[derive(Clone, Debug, PartialEq)]
pub enum ApprovalDecision {
    Allow {
        /// The input the tool runs with: the request's own, or with answers
        /// filled in for a question.
        input: Value,
        /// Rule updates from the chosen [`RememberOption`], if any.
        remember: Option<Value>,
    },
    Deny {
        /// Shown to the agent as the reason.
        message: String,
        /// Also end the agent's turn.
        interrupt: bool,
    },
}

/// Everything the panel can ask of an agent.
#[derive(Clone, Debug, PartialEq)]
pub enum AgentCommand {
    Prompt(String),
    Answer {
        request_id: String,
        tool_use_id: Option<String>,
        decision: ApprovalDecision,
    },
    Interrupt,
    SetMode(PermissionMode),
    SetModel(String),
    QueryContextUsage,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn context_fraction_is_bounded() {
        let usage = |used, max| ContextUsage {
            used_tokens: used,
            max_tokens: max,
        };
        assert_eq!(usage(50, 200).fraction(), 0.25);
        assert_eq!(usage(500, 200).fraction(), 1.0);
        assert_eq!(usage(5, 0).fraction(), 0.0);
    }

    #[test]
    fn model_names_prefer_the_agents_own() {
        let offered = [ModelChoice {
            value: "haiku".into(),
            display_name: "Haiku 4.5 (recommended)".into(),
            description: String::new(),
        }];
        assert_eq!(model_name(&offered, "haiku"), "Haiku 4.5");
        assert_eq!(model_name(&offered, "opus"), "Opus");
        assert_eq!(model_name(&[], "default"), "Default model");
        assert_eq!(model_name(&[], "haiku"), "Haiku");
        assert_eq!(model_name(&[], ""), "");
    }
}
