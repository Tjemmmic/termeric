//! The conversation the panel shows, built from agent events.
//!
//! Streamed output appears as it arrives and is then replaced by the agent's
//! complete message, which is authoritative. Tool calls collect their input,
//! the person's decision, and their result in one place. Everything here is
//! plain data: the panel lays it out, and nothing in this module knows about
//! pixels or protocols.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use serde_json::Value;

use super::event::{
    AgentEvent, ApprovalRequest, BlockStart, ContentBlock, ContextUsage, ExitReport, Handshake,
    PermissionMode, Retry, SessionInfo, StreamDelta, Thread, ToolResult, TurnOutcome,
};

/// Longest tool result text kept for display.
const MAX_RESULT_BYTES: usize = 64 * 1024;
/// Longest streamed tool input kept while it is being written.
const MAX_PARTIAL_INPUT_BYTES: usize = 256 * 1024;

/// Context the person attached to a message.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Attachment {
    /// Shown on the message, e.g. "cargo test · 21 lines".
    pub label: String,
    /// What it contributes to the prompt, e.g. "Output of `cargo test`".
    pub heading: String,
    pub text: String,
}

impl Attachment {
    /// The attachment as the agent reads it.
    pub fn as_prompt(&self) -> String {
        let fence = fence_for(&self.text);
        format!(
            "{}:\n{fence}\n{}\n{fence}",
            self.heading,
            self.text.trim_end()
        )
    }
}

/// A code fence longer than any backtick run inside `text`.
fn fence_for(text: &str) -> String {
    let longest = text
        .split(|character| character != '`')
        .map(str::len)
        .max()
        .unwrap_or(0);
    "`".repeat(longest.max(2) + 1)
}

/// The full prompt for a message with attachments.
pub fn compose_prompt(text: &str, attachments: &[Attachment]) -> String {
    let mut prompt = text.trim().to_owned();
    for attachment in attachments {
        if !prompt.is_empty() {
            prompt.push_str("\n\n");
        }
        prompt.push_str(&attachment.as_prompt());
    }
    prompt
}

/// Who answered an approval.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DecidedBy {
    Person,
    Supervisor,
}

/// How an approval was answered.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Decision {
    pub allowed: bool,
    pub by: DecidedBy,
    /// Also allowed from now on, by the named rule.
    pub remembered: Option<String>,
    /// The supervisor's reason, or the person's note to the agent.
    pub note: Option<String>,
}

/// The supervisor's advice on a pending approval, shown to the person.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Suggestion {
    pub allow: bool,
    pub reason: String,
}

/// Progress of a sub-agent working inside a tool call.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SubAgentProgress {
    pub tool_calls: u32,
    /// The sub-agent's latest tool call, summarized.
    pub latest: Option<String>,
}

/// One tool call and everything known about it.
#[derive(Clone, Debug, PartialEq)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    /// Null until the complete input arrives.
    pub input: Value,
    /// Input streamed so far, while the agent is still writing it.
    pub partial_input: String,
    pub result: Option<ToolResult>,
    pub decision: Option<Decision>,
    /// The agent stopped waiting for a decision.
    pub withdrawn: bool,
    pub sub_agent: SubAgentProgress,
    /// Ended without a result because the turn ended.
    pub abandoned: bool,
}

/// Where a tool call stands.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ToolState {
    /// The agent is still writing the call.
    Preparing,
    /// Waiting for a decision.
    AwaitingApproval,
    Running,
    Succeeded,
    Failed,
    Denied,
    /// Ended without running or finishing.
    Stopped,
}

impl ToolCall {
    fn new(id: String, name: String) -> Self {
        Self {
            id,
            name,
            input: Value::Null,
            partial_input: String::new(),
            result: None,
            decision: None,
            withdrawn: false,
            sub_agent: SubAgentProgress::default(),
            abandoned: false,
        }
    }

    pub fn state(&self, awaiting_approval: bool) -> ToolState {
        if let Some(result) = &self.result {
            return if result.is_error {
                if self
                    .decision
                    .as_ref()
                    .is_some_and(|decision| !decision.allowed)
                {
                    ToolState::Denied
                } else {
                    ToolState::Failed
                }
            } else {
                ToolState::Succeeded
            };
        }
        if self
            .decision
            .as_ref()
            .is_some_and(|decision| !decision.allowed)
        {
            return ToolState::Denied;
        }
        if self.withdrawn || self.abandoned {
            return ToolState::Stopped;
        }
        if awaiting_approval {
            return ToolState::AwaitingApproval;
        }
        if self.input.is_null() {
            ToolState::Preparing
        } else {
            ToolState::Running
        }
    }
}

/// The end of one turn.
#[derive(Clone, Debug, PartialEq)]
pub struct TurnEnd {
    pub outcome: TurnOutcome,
    /// Files the turn edited or wrote, in first-touched order.
    pub files_changed: Vec<String>,
    /// The person stopped the turn.
    pub interrupted: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NoticeKind {
    Info,
    Warning,
    Error,
}

/// Something the panel reports inside the conversation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Notice {
    pub kind: NoticeKind,
    pub text: String,
}

/// One entry in the conversation.
#[derive(Clone, Debug, PartialEq)]
pub enum Item {
    User {
        text: String,
        attachments: Vec<Attachment>,
    },
    Text {
        message_id: String,
        text: String,
        streaming: bool,
        /// Replaced by the agent's complete message.
        confirmed: bool,
    },
    Thinking {
        message_id: String,
        text: String,
        streaming: bool,
        confirmed: bool,
    },
    Tool(ToolCall),
    TurnEnd(TurnEnd),
    Notice(Notice),
}

/// What the agent is doing, for the panel header and the Agents panel.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Activity {
    /// The process started; the agent has not answered yet.
    Starting,
    Idle,
    Working,
    NeedsYou,
    /// The last turn ended; `success` says how.
    Finished {
        success: bool,
    },
    /// The person stopped the last turn.
    Stopped,
    Exited,
}

impl Activity {
    pub fn label(self) -> &'static str {
        match self {
            Activity::Starting => "Starting",
            Activity::Idle => "Ready",
            Activity::Working => "Working",
            Activity::NeedsYou => "Needs you",
            Activity::Finished { success: true } => "Done",
            Activity::Finished { success: false } => "Stopped with an error",
            Activity::Stopped => "Stopped",
            Activity::Exited => "Closed",
        }
    }
}

/// The message the agent is writing on the main thread.
#[derive(Debug, Default)]
struct LiveMessage {
    id: String,
    /// Streamed block index → item index.
    blocks: HashMap<u32, usize>,
}

/// A conversation with one agent.
#[derive(Debug)]
pub struct Transcript {
    items: Vec<Item>,
    live: Option<LiveMessage>,
    approvals: Vec<ApprovalRequest>,
    /// The supervisor's advice on pending approvals, by request.
    suggestions: HashMap<String, Suggestion>,
    handshake: Option<Handshake>,
    session: Option<SessionInfo>,
    mode: PermissionMode,
    context: Option<ContextUsage>,
    retry: Option<(Retry, Instant)>,
    compacting: bool,
    turn: Option<Turn>,
    last_outcome: Option<Activity>,
    exit: Option<ExitReport>,
    /// Changes whenever anything shown changes, so layouts can be cached.
    generation: u64,
}

#[derive(Debug)]
struct Turn {
    started: Instant,
    first_item: usize,
    interrupted: bool,
}

impl Transcript {
    pub fn new(mode: PermissionMode) -> Self {
        Self {
            items: Vec::new(),
            live: None,
            approvals: Vec::new(),
            suggestions: HashMap::new(),
            handshake: None,
            session: None,
            mode,
            context: None,
            retry: None,
            compacting: false,
            turn: None,
            last_outcome: None,
            exit: None,
            generation: 0,
        }
    }

    pub fn items(&self) -> &[Item] {
        &self.items
    }

    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// Approvals waiting for an answer, oldest first.
    pub fn approvals(&self) -> &[ApprovalRequest] {
        &self.approvals
    }

    pub fn approval(&self, request_id: &str) -> Option<&ApprovalRequest> {
        self.approvals
            .iter()
            .find(|approval| approval.request_id == request_id)
    }

    /// The supervisor's advice on a pending approval.
    pub fn suggestion(&self, request_id: &str) -> Option<&Suggestion> {
        self.suggestions.get(request_id)
    }

    /// Records the supervisor's advice on a pending approval. Returns false
    /// when the request is no longer pending.
    pub fn suggest(&mut self, request_id: &str, suggestion: Suggestion) -> bool {
        if self.approval(request_id).is_none() {
            return false;
        }
        self.suggestions.insert(request_id.to_owned(), suggestion);
        self.changed();
        true
    }

    /// The pending approval for a tool call, if any.
    pub fn approval_for_tool(&self, tool_use_id: &str) -> Option<&ApprovalRequest> {
        self.approvals
            .iter()
            .find(|approval| approval.tool_use_id.as_deref() == Some(tool_use_id))
    }

    pub fn handshake(&self) -> Option<&Handshake> {
        self.handshake.as_ref()
    }

    pub fn session(&self) -> Option<&SessionInfo> {
        self.session.as_ref()
    }

    pub fn mode(&self) -> PermissionMode {
        self.mode
    }

    pub fn context(&self) -> Option<ContextUsage> {
        self.context
    }

    pub fn retry(&self) -> Option<&(Retry, Instant)> {
        self.retry.as_ref()
    }

    pub fn compacting(&self) -> bool {
        self.compacting
    }

    pub fn turn_active(&self) -> bool {
        self.turn.is_some()
    }

    pub fn interrupting(&self) -> bool {
        self.turn.as_ref().is_some_and(|turn| turn.interrupted)
    }

    pub fn activity(&self) -> Activity {
        if self.exit.is_some() {
            Activity::Exited
        } else if !self.approvals.is_empty() {
            Activity::NeedsYou
        } else if self.turn.is_some() {
            Activity::Working
        } else if let Some(outcome) = self.last_outcome {
            outcome
        } else if self.handshake.is_none() {
            Activity::Starting
        } else {
            Activity::Idle
        }
    }

    /// The first thing the person said, for naming the conversation.
    pub fn first_prompt(&self) -> Option<&str> {
        self.items.iter().find_map(|item| match item {
            Item::User { text, .. } if !text.trim().is_empty() => Some(text.as_str()),
            _ => None,
        })
    }

    /// The agent's last complete answer, for summaries.
    pub fn last_answer(&self) -> Option<&str> {
        self.items.iter().rev().find_map(|item| match item {
            Item::Text { text, .. } if !text.trim().is_empty() => Some(text.as_str()),
            _ => None,
        })
    }

    fn changed(&mut self) {
        self.generation = self.generation.wrapping_add(1);
    }

    /// Records a message the person sent and starts a turn.
    pub fn push_user(&mut self, text: String, attachments: Vec<Attachment>) {
        self.items.push(Item::User { text, attachments });
        if self.turn.is_none() {
            self.turn = Some(Turn {
                started: Instant::now(),
                first_item: self.items.len() - 1,
                interrupted: false,
            });
        }
        self.last_outcome = None;
        self.changed();
    }

    pub fn push_notice(&mut self, kind: NoticeKind, text: impl Into<String>) {
        self.items.push(Item::Notice(Notice {
            kind,
            text: text.into(),
        }));
        self.changed();
    }

    /// The agent was relaunched after closing; the conversation continues.
    pub fn reopen(&mut self) {
        if self.exit.take().is_some() {
            self.handshake = None;
            self.changed();
        }
    }

    /// The person asked the agent to stop its turn.
    pub fn mark_interrupting(&mut self) {
        if let Some(turn) = &mut self.turn {
            turn.interrupted = true;
            self.changed();
        }
    }

    /// Optimistically shows a mode the person chose; the agent confirms it.
    pub fn set_mode(&mut self, mode: PermissionMode) {
        if self.mode != mode {
            self.mode = mode;
            self.changed();
        }
    }

    /// Records the answer to an approval and takes it off the waiting list.
    /// An allowed call runs with `input`, such as a question with its
    /// answers filled in, so the conversation shows that input.
    pub fn record_decision(
        &mut self,
        request_id: &str,
        decision: Decision,
        input: Option<Value>,
    ) -> bool {
        let Some(position) = self
            .approvals
            .iter()
            .position(|approval| approval.request_id == request_id)
        else {
            return false;
        };
        let approval = self.approvals.remove(position);
        self.suggestions.remove(request_id);
        if let Some(tool) = approval
            .tool_use_id
            .as_deref()
            .and_then(|id| self.tool_mut(id))
        {
            if let Some(input) = input.filter(|_| decision.allowed) {
                tool.input = input;
            }
            tool.decision = Some(decision);
        }
        self.changed();
        true
    }

    pub fn apply(&mut self, event: AgentEvent) {
        match event {
            AgentEvent::Ready(handshake) => {
                if let Some(mode) = handshake.permission_mode {
                    self.mode = mode;
                }
                self.handshake = Some(handshake);
            }
            AgentEvent::SessionStarted(info) => {
                if let Some(mode) = info.permission_mode {
                    self.mode = mode;
                }
                self.session = Some(info);
            }
            AgentEvent::Stream { thread, delta } => {
                self.retry = None;
                if thread == Thread::Main {
                    self.apply_delta(delta);
                } else {
                    return;
                }
            }
            AgentEvent::Message { thread, message } => {
                self.retry = None;
                match thread {
                    Thread::Main => {
                        for block in message.blocks {
                            self.apply_block(&message.id, block);
                        }
                        if let Some(error) = message.error {
                            self.items.push(Item::Notice(Notice {
                                kind: NoticeKind::Error,
                                text: error,
                            }));
                        }
                    }
                    Thread::SubAgent { parent_tool_use_id } => {
                        let Some(parent) = self.tool_mut(&parent_tool_use_id) else {
                            return;
                        };
                        for block in message.blocks {
                            if let ContentBlock::ToolUse { name, input, .. } = block {
                                parent.sub_agent.tool_calls += 1;
                                parent.sub_agent.latest = Some(sub_agent_step(&name, &input));
                            }
                        }
                    }
                }
            }
            AgentEvent::ToolResult { thread, result } => {
                if thread != Thread::Main {
                    return;
                }
                let Some(tool) = self.tool_mut(&result.tool_use_id) else {
                    return;
                };
                tool.result = Some(bounded_result(result));
            }
            AgentEvent::ApprovalRequested(request) => {
                if !request.from_sub_agent
                    && let Some(id) = request.tool_use_id.clone()
                    && self.tool_mut(&id).is_none()
                {
                    let mut tool = ToolCall::new(id, request.tool_name.clone());
                    tool.input = request.input.clone();
                    self.items.push(Item::Tool(tool));
                }
                self.approvals
                    .retain(|approval| approval.request_id != request.request_id);
                self.approvals.push(request);
            }
            AgentEvent::ApprovalWithdrawn { request_id } => {
                self.suggestions.remove(&request_id);
                let Some(position) = self
                    .approvals
                    .iter()
                    .position(|approval| approval.request_id == request_id)
                else {
                    return;
                };
                let approval = self.approvals.remove(position);
                if let Some(tool) = approval
                    .tool_use_id
                    .as_deref()
                    .and_then(|id| self.tool_mut(id))
                {
                    tool.withdrawn = true;
                }
            }
            AgentEvent::ModeChanged(mode) => self.mode = mode,
            AgentEvent::Compacting(compacting) => self.compacting = compacting,
            AgentEvent::Retrying(retry) => self.retry = Some((retry, Instant::now())),
            AgentEvent::ContextUsage(usage) => self.context = Some(usage),
            AgentEvent::TurnFinished(outcome) => self.finish_turn(outcome),
            AgentEvent::HandshakeFailed(error) => {
                self.items.push(Item::Notice(Notice {
                    kind: NoticeKind::Error,
                    text: format!(
                        "Claude Code refused to connect ({error}). This version may not \
                         support the agent view; open it in a terminal instead."
                    ),
                }));
            }
            AgentEvent::RequestFailed { error, .. } => {
                self.items.push(Item::Notice(Notice {
                    kind: NoticeKind::Warning,
                    text: error,
                }));
            }
            AgentEvent::Unrecognized(_) => return,
            AgentEvent::Exited(report) => self.exited(report),
        }
        self.changed();
    }

    fn apply_delta(&mut self, delta: StreamDelta) {
        match delta {
            StreamDelta::MessageStarted { message_id } => {
                self.live = Some(LiveMessage {
                    id: message_id,
                    blocks: HashMap::new(),
                });
            }
            StreamDelta::BlockStarted { index, block } => {
                let Some(live) = &self.live else { return };
                let message_id = live.id.clone();
                let item = match block {
                    BlockStart::Text => Item::Text {
                        message_id,
                        text: String::new(),
                        streaming: true,
                        confirmed: false,
                    },
                    BlockStart::Thinking => Item::Thinking {
                        message_id,
                        text: String::new(),
                        streaming: true,
                        confirmed: false,
                    },
                    BlockStart::ToolUse { id, name } => {
                        if self.tool_mut(&id).is_some() {
                            return;
                        }
                        Item::Tool(ToolCall::new(id, name))
                    }
                };
                self.items.push(item);
                let position = self.items.len() - 1;
                if let Some(live) = &mut self.live {
                    live.blocks.insert(index, position);
                }
            }
            StreamDelta::Text { index, text } | StreamDelta::Thinking { index, text } => {
                if let Some(
                    Item::Text {
                        text: existing,
                        confirmed: false,
                        ..
                    }
                    | Item::Thinking {
                        text: existing,
                        confirmed: false,
                        ..
                    },
                ) = self.live_item(index)
                {
                    existing.push_str(&text);
                }
            }
            StreamDelta::ToolInput {
                index,
                partial_json,
            } => {
                if let Some(Item::Tool(tool)) = self.live_item(index)
                    && tool.input.is_null()
                    && tool.partial_input.len() + partial_json.len() <= MAX_PARTIAL_INPUT_BYTES
                {
                    tool.partial_input.push_str(&partial_json);
                }
            }
            StreamDelta::BlockStopped { index } => {
                if let Some(Item::Text { streaming, .. } | Item::Thinking { streaming, .. }) =
                    self.live_item(index)
                {
                    *streaming = false;
                }
            }
        }
    }

    fn live_item(&mut self, index: u32) -> Option<&mut Item> {
        let position = *self.live.as_ref()?.blocks.get(&index)?;
        self.items.get_mut(position)
    }

    fn apply_block(&mut self, message_id: &str, block: ContentBlock) {
        match block {
            ContentBlock::Text(text) => {
                self.confirm_text(message_id, text, false);
            }
            ContentBlock::Thinking(text) => {
                self.confirm_text(message_id, text, true);
            }
            ContentBlock::ToolUse { id, name, input } => {
                if let Some(tool) = self.tool_mut(&id) {
                    // A decided call keeps the input it ran with.
                    if tool.decision.is_none() {
                        tool.input = input;
                    }
                    tool.partial_input = String::new();
                } else {
                    let mut tool = ToolCall::new(id, name);
                    tool.input = input;
                    self.items.push(Item::Tool(tool));
                }
            }
        }
    }

    /// Replaces the first unconfirmed streamed block of the same kind from
    /// the same message, or adds the block when nothing was streamed.
    fn confirm_text(&mut self, message_id: &str, complete: String, thinking: bool) {
        let streamed = self.items.iter_mut().find(|item| match item {
            Item::Text {
                message_id: id,
                confirmed: false,
                ..
            } => !thinking && id == message_id,
            Item::Thinking {
                message_id: id,
                confirmed: false,
                ..
            } => thinking && id == message_id,
            _ => false,
        });
        match streamed {
            Some(
                Item::Text {
                    text,
                    streaming,
                    confirmed,
                    ..
                }
                | Item::Thinking {
                    text,
                    streaming,
                    confirmed,
                    ..
                },
            ) => {
                *text = complete;
                *streaming = false;
                *confirmed = true;
            }
            _ => {
                if complete.trim().is_empty() {
                    return;
                }
                let message_id = message_id.to_owned();
                self.items.push(if thinking {
                    Item::Thinking {
                        message_id,
                        text: complete,
                        streaming: false,
                        confirmed: true,
                    }
                } else {
                    Item::Text {
                        message_id,
                        text: complete,
                        streaming: false,
                        confirmed: true,
                    }
                });
            }
        }
    }

    fn tool_mut(&mut self, id: &str) -> Option<&mut ToolCall> {
        self.items.iter_mut().rev().find_map(|item| match item {
            Item::Tool(tool) if tool.id == id => Some(tool),
            _ => None,
        })
    }

    fn settle_streaming(&mut self, first_item: usize) {
        for item in self.items.iter_mut().skip(first_item) {
            match item {
                Item::Text { streaming, .. } | Item::Thinking { streaming, .. } => {
                    *streaming = false;
                }
                Item::Tool(tool) if tool.result.is_none() => tool.abandoned = true,
                _ => {}
            }
        }
        self.live = None;
    }

    fn finish_turn(&mut self, outcome: TurnOutcome) {
        let (first_item, interrupted) =
            self.turn.take().map_or((self.items.len(), false), |turn| {
                (turn.first_item, turn.interrupted)
            });
        self.settle_streaming(first_item);
        self.approvals.clear();
        self.suggestions.clear();
        self.retry = None;
        self.compacting = false;
        let files_changed = changed_files(&self.items[first_item.min(self.items.len())..]);
        self.last_outcome = Some(if interrupted {
            Activity::Stopped
        } else {
            Activity::Finished {
                success: outcome.success,
            }
        });
        if let Some(session_id) = &outcome.session_id
            && let Some(session) = &mut self.session
        {
            session.session_id.clone_from(session_id);
        }
        self.items.push(Item::TurnEnd(TurnEnd {
            outcome,
            files_changed,
            interrupted,
        }));
    }

    fn exited(&mut self, report: ExitReport) {
        let first_item = self
            .turn
            .take()
            .map_or(self.items.len(), |turn| turn.first_item);
        self.settle_streaming(first_item);
        self.approvals.clear();
        self.suggestions.clear();
        self.retry = None;
        self.compacting = false;
        if report.status != "closed" {
            let mut text = format!("Claude Code stopped ({})", report.status);
            if let Some(reason) = last_meaningful_line(&report.stderr_tail) {
                text.push_str(": ");
                text.push_str(reason);
            }
            self.items.push(Item::Notice(Notice {
                kind: NoticeKind::Error,
                text,
            }));
        }
        self.exit = Some(report);
    }

    /// How long the current turn has run.
    pub fn turn_elapsed(&self) -> Option<Duration> {
        self.turn.as_ref().map(|turn| turn.started.elapsed())
    }
}

fn last_meaningful_line(text: &str) -> Option<&str> {
    text.lines()
        .rev()
        .map(str::trim)
        .find(|line| !line.is_empty())
}

fn bounded_result(mut result: ToolResult) -> ToolResult {
    if result.content.len() > MAX_RESULT_BYTES {
        let mut end = MAX_RESULT_BYTES;
        while !result.content.is_char_boundary(end) {
            end -= 1;
        }
        result.content.truncate(end);
        result.content.push_str("\n…");
    }
    result
}

/// Tools that change files, and the input field naming the file.
pub fn edited_path(tool: &ToolCall) -> Option<&str> {
    match tool.name.as_str() {
        "Edit" | "MultiEdit" | "Write" => tool.input.get("file_path")?.as_str(),
        "NotebookEdit" => tool.input.get("notebook_path")?.as_str(),
        _ => None,
    }
}

fn changed_files(items: &[Item]) -> Vec<String> {
    let mut files: Vec<String> = Vec::new();
    for item in items {
        let Item::Tool(tool) = item else { continue };
        let succeeded = tool.result.as_ref().is_some_and(|result| !result.is_error);
        if let Some(path) = edited_path(tool).filter(|_| succeeded)
            && !files.iter().any(|file| file == path)
        {
            files.push(path.to_owned());
        }
    }
    files
}

/// A few words for a sub-agent's step, e.g. "Read main.rs".
fn sub_agent_step(name: &str, input: &Value) -> String {
    let field = |key: &str| input.get(key).and_then(Value::as_str);
    let file_name = |path: &str| {
        std::path::Path::new(path).file_name().map_or_else(
            || path.to_owned(),
            |name| name.to_string_lossy().into_owned(),
        )
    };
    match name {
        "Read" | "Edit" | "Write" | "MultiEdit" => field("file_path").map_or_else(
            || name.to_owned(),
            |path| format!("{name} {}", file_name(path)),
        ),
        "Grep" | "Glob" => field("pattern").map_or_else(
            || "Searched".to_owned(),
            |pattern| format!("Searched {pattern}"),
        ),
        "Bash" => field("description")
            .or_else(|| field("command"))
            .map_or_else(|| "Ran a command".to_owned(), str::to_owned),
        other => other.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent_view::event::{AssistantMessage, ModelChoice};
    use serde_json::json;

    fn ready(transcript: &mut Transcript) {
        transcript.apply(AgentEvent::Ready(Handshake {
            models: vec![ModelChoice {
                value: "sonnet".into(),
                display_name: "Sonnet".into(),
                description: String::new(),
            }],
            commands: Vec::new(),
            permission_mode: Some(PermissionMode::AskBeforeEdits),
        }));
    }

    fn stream(transcript: &mut Transcript, delta: StreamDelta) {
        transcript.apply(AgentEvent::Stream {
            thread: Thread::Main,
            delta,
        });
    }

    fn message(transcript: &mut Transcript, id: &str, blocks: Vec<ContentBlock>) {
        transcript.apply(AgentEvent::Message {
            thread: Thread::Main,
            message: AssistantMessage {
                id: id.into(),
                blocks,
                error: None,
            },
        });
    }

    fn result(transcript: &mut Transcript, id: &str, is_error: bool) {
        transcript.apply(AgentEvent::ToolResult {
            thread: Thread::Main,
            result: ToolResult {
                tool_use_id: id.into(),
                content: "output".into(),
                is_error,
                detail: None,
            },
        });
    }

    fn finish(transcript: &mut Transcript, success: bool) {
        transcript.apply(AgentEvent::TurnFinished(TurnOutcome {
            success,
            ..TurnOutcome::default()
        }));
    }

    fn approval(request_id: &str, tool_use_id: &str) -> ApprovalRequest {
        ApprovalRequest {
            request_id: request_id.into(),
            tool_use_id: Some(tool_use_id.into()),
            tool_name: "Bash".into(),
            input: json!({"command": "cargo test"}),
            title: None,
            description: None,
            reason: None,
            remember: None,
            from_sub_agent: false,
        }
    }

    #[test]
    fn activity_follows_the_handshake_turns_and_approvals() {
        let mut transcript = Transcript::new(PermissionMode::AskBeforeEdits);
        assert_eq!(transcript.activity(), Activity::Starting);
        ready(&mut transcript);
        assert_eq!(transcript.activity(), Activity::Idle);
        transcript.push_user("hi".into(), Vec::new());
        assert_eq!(transcript.activity(), Activity::Working);
        transcript.apply(AgentEvent::ApprovalRequested(approval("r1", "t1")));
        assert_eq!(transcript.activity(), Activity::NeedsYou);
        assert!(transcript.record_decision(
            "r1",
            Decision {
                allowed: true,
                by: DecidedBy::Person,
                remembered: None,
                note: None
            },
            Some(json!({"command": "cargo test --quiet"}))
        ));
        assert!(!transcript.record_decision(
            "r1",
            Decision {
                allowed: true,
                by: DecidedBy::Person,
                remembered: None,
                note: None
            },
            None
        ));
        let Item::Tool(tool) = &transcript.items()[1] else {
            panic!()
        };
        assert_eq!(tool.input["command"], "cargo test --quiet");
        assert_eq!(transcript.activity(), Activity::Working);
        finish(&mut transcript, true);
        assert_eq!(transcript.activity(), Activity::Finished { success: true });
        transcript.push_user("again".into(), Vec::new());
        assert_eq!(transcript.activity(), Activity::Working);
    }

    #[test]
    fn streamed_text_is_replaced_by_the_complete_message() {
        let mut transcript = Transcript::new(PermissionMode::AskBeforeEdits);
        transcript.push_user("hi".into(), Vec::new());
        stream(
            &mut transcript,
            StreamDelta::MessageStarted {
                message_id: "m1".into(),
            },
        );
        stream(
            &mut transcript,
            StreamDelta::BlockStarted {
                index: 0,
                block: BlockStart::Text,
            },
        );
        stream(
            &mut transcript,
            StreamDelta::Text {
                index: 0,
                text: "Hel".into(),
            },
        );
        stream(
            &mut transcript,
            StreamDelta::Text {
                index: 0,
                text: "lo".into(),
            },
        );
        assert!(matches!(
            &transcript.items()[1],
            Item::Text { text, streaming: true, confirmed: false, .. } if text == "Hello"
        ));
        let before = transcript.generation();
        message(
            &mut transcript,
            "m1",
            vec![ContentBlock::Text("Hello there.".into())],
        );
        assert!(transcript.generation() != before);
        assert_eq!(transcript.items().len(), 2);
        assert!(matches!(
            &transcript.items()[1],
            Item::Text { text, streaming: false, confirmed: true, .. } if text == "Hello there."
        ));
        // A message with no streamed counterpart is added whole.
        message(
            &mut transcript,
            "m2",
            vec![ContentBlock::Text("Next.".into())],
        );
        assert_eq!(transcript.items().len(), 3);
        assert_eq!(transcript.last_answer(), Some("Next."));
    }

    #[test]
    fn claude_splits_one_message_into_several_complete_messages() {
        let mut transcript = Transcript::new(PermissionMode::AskBeforeEdits);
        transcript.push_user("hi".into(), Vec::new());
        stream(
            &mut transcript,
            StreamDelta::MessageStarted {
                message_id: "m1".into(),
            },
        );
        for (index, block) in [
            BlockStart::Text,
            BlockStart::ToolUse {
                id: "t1".into(),
                name: "Read".into(),
            },
        ]
        .into_iter()
        .enumerate()
        {
            stream(
                &mut transcript,
                StreamDelta::BlockStarted {
                    index: index as u32,
                    block,
                },
            );
        }
        stream(
            &mut transcript,
            StreamDelta::ToolInput {
                index: 1,
                partial_json: "{\"file_path\":".into(),
            },
        );
        let Item::Tool(tool) = &transcript.items()[2] else {
            panic!()
        };
        assert_eq!(tool.state(false), ToolState::Preparing);
        assert_eq!(tool.partial_input, "{\"file_path\":");
        message(
            &mut transcript,
            "m1",
            vec![ContentBlock::Text("Looking.".into())],
        );
        message(
            &mut transcript,
            "m1",
            vec![ContentBlock::ToolUse {
                id: "t1".into(),
                name: "Read".into(),
                input: json!({"file_path": "/a.rs"}),
            }],
        );
        assert_eq!(transcript.items().len(), 3);
        let Item::Tool(tool) = &transcript.items()[2] else {
            panic!()
        };
        assert_eq!(tool.state(false), ToolState::Running);
        assert!(tool.partial_input.is_empty());
        result(&mut transcript, "t1", false);
        let Item::Tool(tool) = &transcript.items()[2] else {
            panic!()
        };
        assert_eq!(tool.state(false), ToolState::Succeeded);
    }

    #[test]
    fn approvals_attach_to_their_tool_call_and_record_the_answer() {
        let mut transcript = Transcript::new(PermissionMode::AskBeforeEdits);
        transcript.push_user("test it".into(), Vec::new());
        // The request can arrive before the tool call is known.
        transcript.apply(AgentEvent::ApprovalRequested(approval("r1", "t1")));
        assert_eq!(transcript.items().len(), 2);
        assert!(transcript.approval_for_tool("t1").is_some());
        let Item::Tool(tool) = &transcript.items()[1] else {
            panic!()
        };
        assert_eq!(tool.state(true), ToolState::AwaitingApproval);
        // The complete message then fills in the same call.
        message(
            &mut transcript,
            "m1",
            vec![ContentBlock::ToolUse {
                id: "t1".into(),
                name: "Bash".into(),
                input: json!({"command": "cargo test"}),
            }],
        );
        assert_eq!(transcript.items().len(), 2);
        transcript.record_decision(
            "r1",
            Decision {
                allowed: false,
                by: DecidedBy::Supervisor,
                remembered: None,
                note: Some("outside the rules".into()),
            },
            Some(json!({"ignored": true})),
        );
        let Item::Tool(tool) = &transcript.items()[1] else {
            panic!()
        };
        assert_eq!(tool.state(false), ToolState::Denied);
        assert_eq!(tool.decision.as_ref().unwrap().by, DecidedBy::Supervisor);
        result(&mut transcript, "t1", true);
        let Item::Tool(tool) = &transcript.items()[1] else {
            panic!()
        };
        assert_eq!(tool.state(false), ToolState::Denied);
    }

    #[test]
    fn suggestions_last_only_while_their_request_is_pending() {
        let mut transcript = Transcript::new(PermissionMode::AskBeforeEdits);
        transcript.push_user("go".into(), Vec::new());
        let suggestion = Suggestion {
            allow: true,
            reason: "tests are allowed".into(),
        };
        assert!(!transcript.suggest("r1", suggestion.clone()));
        transcript.apply(AgentEvent::ApprovalRequested(approval("r1", "t1")));
        assert!(transcript.suggest("r1", suggestion.clone()));
        assert_eq!(transcript.suggestion("r1"), Some(&suggestion));
        transcript.record_decision(
            "r1",
            Decision {
                allowed: true,
                by: DecidedBy::Person,
                remembered: None,
                note: None,
            },
            None,
        );
        assert_eq!(transcript.suggestion("r1"), None);
    }

    #[test]
    fn withdrawn_approvals_stop_their_tool_call() {
        let mut transcript = Transcript::new(PermissionMode::AskBeforeEdits);
        transcript.push_user("go".into(), Vec::new());
        transcript.apply(AgentEvent::ApprovalRequested(approval("r1", "t1")));
        transcript.apply(AgentEvent::ApprovalWithdrawn {
            request_id: "r1".into(),
        });
        assert!(transcript.approvals().is_empty());
        let Item::Tool(tool) = &transcript.items()[1] else {
            panic!()
        };
        assert_eq!(tool.state(false), ToolState::Stopped);
    }

    #[test]
    fn a_turn_end_lists_changed_files_and_settles_everything_open() {
        let mut transcript = Transcript::new(PermissionMode::AskBeforeEdits);
        transcript.push_user("fix".into(), Vec::new());
        transcript.mark_interrupting();
        assert!(transcript.interrupting());
        for (id, name, path) in [
            ("t1", "Edit", "/w/a.rs"),
            ("t2", "Write", "/w/b.rs"),
            ("t3", "Edit", "/w/a.rs"),
            ("t4", "Edit", "/w/failed.rs"),
            ("t5", "Read", "/w/c.rs"),
        ] {
            message(
                &mut transcript,
                id,
                vec![ContentBlock::ToolUse {
                    id: id.into(),
                    name: name.into(),
                    input: json!({"file_path": path}),
                }],
            );
        }
        result(&mut transcript, "t1", false);
        result(&mut transcript, "t2", false);
        result(&mut transcript, "t3", false);
        result(&mut transcript, "t4", true);
        transcript.apply(AgentEvent::ApprovalRequested(approval("r9", "t6")));
        finish(&mut transcript, false);
        assert!(transcript.approvals().is_empty());
        let Some(Item::TurnEnd(end)) = transcript.items().last() else {
            panic!()
        };
        assert_eq!(end.files_changed, ["/w/a.rs", "/w/b.rs"]);
        assert!(end.interrupted);
        assert!(!transcript.interrupting());
        // The unanswered read never ran.
        let read = transcript
            .items()
            .iter()
            .find_map(|item| match item {
                Item::Tool(tool) if tool.id == "t5" => Some(tool),
                _ => None,
            })
            .unwrap();
        assert_eq!(read.state(false), ToolState::Stopped);
        // The person stopped it, which is not a failure.
        assert_eq!(transcript.activity(), Activity::Stopped);
    }

    #[test]
    fn sub_agent_work_is_summarized_on_its_parent_call() {
        let mut transcript = Transcript::new(PermissionMode::AskBeforeEdits);
        transcript.push_user("research".into(), Vec::new());
        message(
            &mut transcript,
            "m1",
            vec![ContentBlock::ToolUse {
                id: "task".into(),
                name: "Agent".into(),
                input: json!({"description": "Find callers"}),
            }],
        );
        for (name, input) in [
            ("Grep", json!({"pattern": "refresh_focus"})),
            ("Read", json!({"file_path": "/w/src/app/focus.rs"})),
        ] {
            transcript.apply(AgentEvent::Message {
                thread: Thread::SubAgent {
                    parent_tool_use_id: "task".into(),
                },
                message: AssistantMessage {
                    id: "s".into(),
                    blocks: vec![ContentBlock::ToolUse {
                        id: "x".into(),
                        name: name.into(),
                        input,
                    }],
                    error: None,
                },
            });
        }
        // Sub-agent streaming never adds items to the main conversation.
        transcript.apply(AgentEvent::Stream {
            thread: Thread::SubAgent {
                parent_tool_use_id: "task".into(),
            },
            delta: StreamDelta::MessageStarted {
                message_id: "s".into(),
            },
        });
        assert_eq!(transcript.items().len(), 2);
        let Item::Tool(task) = &transcript.items()[1] else {
            panic!()
        };
        assert_eq!(task.sub_agent.tool_calls, 2);
        assert_eq!(task.sub_agent.latest.as_deref(), Some("Read focus.rs"));
    }

    #[test]
    fn an_unexpected_exit_is_explained_and_a_requested_close_is_quiet() {
        let mut transcript = Transcript::new(PermissionMode::AskBeforeEdits);
        transcript.push_user("hi".into(), Vec::new());
        transcript.apply(AgentEvent::Exited(ExitReport {
            success: false,
            status: "exit status: 1".into(),
            stderr_tail: "warming up\nError: not logged in\n".into(),
        }));
        assert_eq!(transcript.activity(), Activity::Exited);
        assert!(!transcript.turn_active());
        let Some(Item::Notice(notice)) = transcript.items().last() else {
            panic!()
        };
        assert_eq!(notice.kind, NoticeKind::Error);
        assert_eq!(
            notice.text,
            "Claude Code stopped (exit status: 1): Error: not logged in"
        );
        let mut closed = Transcript::new(PermissionMode::AskBeforeEdits);
        closed.apply(AgentEvent::Exited(ExitReport {
            success: false,
            status: "closed".into(),
            stderr_tail: String::new(),
        }));
        assert!(closed.items().is_empty());
    }

    #[test]
    fn large_results_are_bounded_on_a_character_boundary() {
        let bounded = bounded_result(ToolResult {
            tool_use_id: "t".into(),
            content: "é".repeat(MAX_RESULT_BYTES),
            is_error: false,
            detail: None,
        });
        assert!(bounded.content.len() <= MAX_RESULT_BYTES + "\n…".len());
        assert!(bounded.content.ends_with("\n…"));
    }

    #[test]
    fn prompts_carry_attachments_in_fences_longer_than_their_content() {
        let attachment = Attachment {
            label: "cargo test".into(),
            heading: "Output of `cargo test`".into(),
            text: "a ``` b\n".into(),
        };
        assert_eq!(
            compose_prompt("  Why? ", &[attachment]),
            "Why?\n\nOutput of `cargo test`:\n````\na ``` b\n````"
        );
        assert_eq!(compose_prompt("plain", &[]), "plain");
    }

    #[test]
    fn the_session_id_follows_the_latest_turn() {
        let mut transcript = Transcript::new(PermissionMode::AskBeforeEdits);
        transcript.apply(AgentEvent::SessionStarted(SessionInfo {
            session_id: "a".into(),
            permission_mode: Some(PermissionMode::PlanOnly),
            ..SessionInfo::default()
        }));
        assert_eq!(transcript.mode(), PermissionMode::PlanOnly);
        transcript.push_user("hi".into(), Vec::new());
        transcript.apply(AgentEvent::TurnFinished(TurnOutcome {
            success: true,
            session_id: Some("b".into()),
            ..TurnOutcome::default()
        }));
        assert_eq!(transcript.session().unwrap().session_id, "b");
    }
}
