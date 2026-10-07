//! Bounded wire types for inspecting and explicitly controlling GUI processes.

use crate::panes::SplitDirection;
use anyhow::Context;
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use std::collections::HashSet;
use std::io::{Read, Write};
use termeric_core::split::RATIO_MILLIS;

pub const PROTOCOL_VERSION: u32 = 1;
pub const JSON_SCHEMA_VERSION: u32 = 1;
pub const MAX_FRAME_BYTES: usize = 256 * 1024;
pub const MAX_WINDOWS: usize = 32;
pub const MAX_TABS_PER_WINDOW: usize = 256;
pub const MAX_PANES_PER_TAB: usize = crate::panes::MAX_PANES_PER_TAB;
pub const MAX_DIVIDERS_PER_TAB: usize = MAX_PANES_PER_TAB - 1;
pub const MAX_TOTAL_TABS: usize = 256;
pub const MAX_TOTAL_PANES: usize = 1_024;
pub const MAX_TOTAL_DIVIDERS: usize = MAX_TOTAL_PANES - 1;
pub const MAX_METADATA_BYTES: usize = 32 * 1024;
/// The largest process ID: Linux process IDs are positive `pid_t` values.
pub const MAX_PROCESS_ID: u32 = i32::MAX as u32;
pub const MIN_WINDOW_WIDTH: u32 = 160;
pub const MIN_WINDOW_HEIGHT: u32 = 120;
pub const MAX_WINDOW_DIMENSION: u32 = 16_384;
pub const MAX_TEXT_HISTORY_LINES: u16 = 256;
pub const MAX_TERMINAL_TEXT_BYTES: u32 = 64 * 1024;
pub const MAX_CONTROL_INPUT_BYTES: usize = 64 * 1024;
pub const MAX_CONTROL_LAUNCH_ARGS: usize = 128;
pub const MAX_CONTROL_LAUNCH_ARGV_BYTES: usize = 16 * 1024;
pub const MAX_CONTROL_LAUNCH_CWD_BYTES: usize = 4 * 1024;
pub const MAX_CONTROL_LAUNCH_TITLE_BYTES: usize = 256;
/// Tabs a joining launch may open in its new window.
pub const MAX_JOIN_TABS: usize = 32;
/// Longest display name or activation token a joining launch may carry.
pub const MAX_JOIN_IDENTITY_BYTES: usize = 1024;
pub const MAX_CONTROL_LAUNCH_ENVIRONMENT_ENTRIES: usize = 64;
pub const MAX_CONTROL_LAUNCH_ENVIRONMENT_NAME_BYTES: usize = 128;
pub const MAX_CONTROL_LAUNCH_ENVIRONMENT_VALUE_BYTES: usize = 8 * 1024;
pub const MAX_CONTROL_LAUNCH_ENVIRONMENT_BYTES: usize = 64 * 1024;
pub const MAX_AGENT_TERMINAL_ID_BYTES: usize = 64;
pub const MAX_AGENT_ID_BYTES: usize = 64;
pub const MAX_AGENT_EVENT_DETAIL_BYTES: usize = 512;
pub const MAX_AGENT_LAST_MESSAGE_BYTES: usize = 64 * 1024;
pub const MAX_AGENT_TRANSCRIPT_PATH_BYTES: usize = 4 * 1024;
pub const MAX_AGENT_SUMMARIES: usize = 256;
/// Longest reason given with an answer to an agent's approval request.
pub const MAX_APPROVAL_REASON_BYTES: usize = 2 * 1024;
/// Longest approval-request identifier an agent uses.
pub const MAX_APPROVAL_ID_BYTES: usize = 128;
pub const MAX_AGENT_SUMMARY_MESSAGE_BYTES: usize = 16 * 1024;
/// Longest error message a GUI sends back.
pub const MAX_ERROR_MESSAGE_BYTES: usize = 1024;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ClientFrame {
    pub version: u32,
    pub token: String,
    pub request: Request,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "operation", rename_all = "snake_case")]
#[serde(deny_unknown_fields)]
pub enum Request {
    Inventory,
    LaunchStatus {
        launch_request_id: String,
    },
    ReadText {
        window_id: u64,
        tab_id: u64,
        pane_id: u32,
        history_lines: u16,
        max_bytes: u32,
    },
    SendText {
        request_id: String,
        window_id: u64,
        tab_id: u64,
        pane_id: u32,
        text: String,
        paste: bool,
        enter: bool,
    },
    CreateWindow {
        request_id: String,
        source_window: u64,
    },
    /// Open the window a new launch asked for in this running process, so
    /// every window shares one process and tabs can move between them.
    JoinLaunch {
        request_id: String,
        /// The display the launch runs on (`WAYLAND_DISPLAY`, else
        /// `DISPLAY`); a process on another display refuses it.
        display: String,
        /// The desktop's activation token, so the window takes focus.
        activation_token: Option<String>,
        /// The launcher's build (its executable and when that file last
        /// changed); a process running any other build refuses it, so a
        /// rebuilt or different Termeric runs its own code.
        build: String,
        /// The launcher's environment identity, a fingerprint of the
        /// variables that shape the Termeric process itself
        /// (`control::JOIN_ENVIRONMENT_VARIABLES`); a process started with
        /// other values refuses it, so the launch runs with its own.
        environment: String,
        tabs: Vec<JoinTab>,
    },
    LaunchWindow {
        request_id: String,
        source_window: u64,
        program: String,
        args: Vec<String>,
        working_directory: Option<String>,
        title: Option<String>,
        environment: Vec<EnvironmentVariable>,
        environment_policy: termeric_core::pty::EnvironmentPolicy,
    },
    CreateTab {
        request_id: String,
        window_id: u64,
    },
    LaunchTab {
        background: bool,
        request_id: String,
        window_id: u64,
        program: String,
        args: Vec<String>,
        working_directory: Option<String>,
        title: Option<String>,
        environment: Vec<EnvironmentVariable>,
        environment_policy: termeric_core::pty::EnvironmentPolicy,
    },
    SplitPane {
        request_id: String,
        window_id: u64,
        tab_id: u64,
        pane_id: u32,
        direction: SplitDirection,
    },
    FocusWindow {
        request_id: String,
        window_id: u64,
    },
    SetQuickAccessVisibility {
        request_id: String,
        window_id: u64,
        visible: bool,
    },
    SelectTab {
        request_id: String,
        window_id: u64,
        tab_id: u64,
    },
    FocusPane {
        request_id: String,
        window_id: u64,
        tab_id: u64,
        pane_id: u32,
    },
    CloseWindow {
        request_id: String,
        window_id: u64,
    },
    CloseTab {
        request_id: String,
        window_id: u64,
        tab_id: u64,
    },
    ClosePane {
        request_id: String,
        window_id: u64,
        tab_id: u64,
        pane_id: u32,
    },
    MoveTab {
        request_id: String,
        source_window_id: u64,
        tab_id: u64,
        destination_window_id: u64,
        destination_index: u16,
    },
    MovePane {
        request_id: String,
        source_window_id: u64,
        source_tab_id: u64,
        pane_id: u32,
        destination_window_id: u64,
        destination_tab_id: u64,
        target_pane_id: u32,
        direction: SplitDirection,
    },
    ResizeWindow {
        request_id: String,
        window_id: u64,
        width: u32,
        height: u32,
    },
    ResizeDivider {
        request_id: String,
        window_id: u64,
        tab_id: u64,
        divider_id: u64,
        ratio_millis: u16,
    },
    /// Every terminal running a coding agent, with its reported state. With
    /// a supervisor token, only the agents that supervisor oversees.
    ListAgents {
        supervisor: Option<String>,
    },
    /// Paste `text` into the agent in one pane and submit it once it has
    /// settled, provided that pane still runs `agent_id`. With a supervisor
    /// token, the send is the supervisor's and follows its mode and limits.
    SubmitToAgent {
        request_id: String,
        window_id: u64,
        tab_id: u64,
        pane_id: u32,
        agent_id: String,
        text: String,
        supervisor: Option<String>,
    },
    /// The recent conversation of one agent view session, as text.
    ReadConversation {
        window_id: u64,
        session_id: u64,
    },
    /// Answer a pending approval in an agent view session. With a supervisor
    /// token the answer is the supervisor's and follows its mode: in Assist
    /// mode it is shown to the person as a suggestion instead.
    AnswerApproval {
        request_id: String,
        window_id: u64,
        session_id: u64,
        approval_id: String,
        allow: bool,
        reason: String,
        supervisor: Option<String>,
    },
    /// Send a message to an agent view session, as a new turn of its
    /// conversation. With a supervisor token, the send is the supervisor's.
    SubmitToSession {
        request_id: String,
        window_id: u64,
        session_id: u64,
        text: String,
        supervisor: Option<String>,
    },
    /// A coding agent's hook reporting what the agent is doing. The terminal
    /// is named by the identity its processes inherited, not by focus.
    AgentEvent {
        terminal_id: String,
        agent_id: String,
        event: AgentEventKind,
        detail: Option<String>,
        last_message: Option<String>,
        transcript_path: Option<String>,
    },
}

/// One coding agent: in a terminal, named by tab and pane, or in the agent
/// view, named by its session.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentSummary {
    pub window_id: u64,
    pub tab_id: u64,
    pub pane_id: u32,
    /// Set for an agent view session, whose tab and pane are zero.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<u64>,
    pub agent_id: String,
    pub name: String,
    pub tab_title: String,
    /// `None` when the agent's hooks are not reporting.
    pub state: Option<AgentState>,
    /// What the agent asks for, or why its turn failed.
    pub detail: Option<String>,
    /// The agent's latest answer, cut to a bounded length.
    pub last_message: Option<String>,
    pub seconds_in_state: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentState {
    Idle,
    Working,
    NeedsInput,
    Finished,
    Failed,
}

impl AgentSummary {
    fn validate(&self) -> anyhow::Result<()> {
        anyhow::ensure!(self.window_id != 0, "agent summary has no window");
        anyhow::ensure!(
            valid_agent_id(&self.agent_id)
                && self.name.len() <= MAX_METADATA_BYTES
                && self.tab_title.len() <= MAX_METADATA_BYTES
                && self
                    .detail
                    .as_ref()
                    .is_none_or(|detail| detail.len() <= MAX_AGENT_EVENT_DETAIL_BYTES)
                && self
                    .last_message
                    .as_ref()
                    .is_none_or(|message| message.len() <= MAX_AGENT_SUMMARY_MESSAGE_BYTES),
            "agent summary exceeds its bounds"
        );
        Ok(())
    }
}

/// Room kept in an agent list frame for everything but the summaries.
const AGENT_LIST_ENVELOPE_BYTES: usize = 1024;

/// Keeps the agent summaries that fit one frame, in order, and reports
/// whether any were left out.
pub fn fit_agent_summaries(agents: &mut Vec<AgentSummary>) -> bool {
    let budget = MAX_FRAME_BYTES - AGENT_LIST_ENVELOPE_BYTES;
    let mut used = 0usize;
    let fitting = agents
        .iter()
        .take(MAX_AGENT_SUMMARIES)
        .take_while(|agent| {
            // Serializing a summary of plain fields cannot fail; one that
            // somehow did is left out rather than guessed at.
            let size = serde_json::to_vec(agent).map_or(usize::MAX, |bytes| bytes.len() + 1);
            used = used.saturating_add(size);
            used <= budget
        })
        .count();
    let truncated = fitting < agents.len();
    agents.truncate(fitting);
    truncated
}

/// Validate a bounded list of agent summaries from a GUI.
pub fn validate_agent_summaries(agents: &[AgentSummary]) -> anyhow::Result<()> {
    anyhow::ensure!(
        agents.len() <= MAX_AGENT_SUMMARIES,
        "more than {MAX_AGENT_SUMMARIES} agents"
    );
    agents.iter().try_for_each(AgentSummary::validate)
}

/// Whether `process_id` can name a running process.
pub fn valid_process_id(process_id: u32) -> bool {
    (1..=MAX_PROCESS_ID).contains(&process_id)
}

/// Whether `agent_id` is 1 to 64 ASCII letters, digits, `-`, or `_`.
pub fn valid_agent_id(agent_id: &str) -> bool {
    !agent_id.is_empty()
        && agent_id.len() <= MAX_AGENT_ID_BYTES
        && agent_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
}

/// What an agent reported through its hooks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentEventKind {
    /// A conversation started or resumed.
    SessionStarted,
    /// The agent received a prompt and is working on it.
    Working,
    /// The agent is waiting for the person, for example to approve a tool.
    NeedsInput,
    /// The agent finished its turn.
    Finished,
    /// The agent's turn ended in an error.
    Failed,
    /// The agent stopped without a result: the person interrupted it, or it
    /// sits waiting at its prompt.
    Stopped,
    /// The conversation ended.
    SessionEnded,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EnvironmentVariable {
    pub name: String,
    pub value: String,
}

impl std::fmt::Debug for EnvironmentVariable {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("EnvironmentVariable")
            .field("name", &"[redacted]")
            .field("value", &"[redacted]")
            .finish()
    }
}

/// One tab of a joining launch: a shell, or a command, in a folder.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct JoinTab {
    pub working_directory: Option<String>,
    pub title: Option<String>,
    /// A command to run instead of the shell.
    pub program: Option<String>,
    pub args: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case", deny_unknown_fields)]
pub enum LaunchState {
    Running {},
    Exited { exit_code: u32, success: bool },
}

impl LaunchState {
    pub fn validate(&self) -> Result<(), String> {
        if let Self::Exited { exit_code, success } = self
            && *success != (*exit_code == 0)
        {
            return Err("Inconsistent GUI child exit result".into());
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TextScreen {
    Primary,
    Alternate,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InputDisposition {
    Sent,
    PendingConfirmation,
}

/// A GUI's reply to one request. It travels inside [`ServerFrame`], and
/// `--json` prints it as is, beside a `schema_version`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "result", rename_all = "snake_case")]
#[serde(deny_unknown_fields)]
pub enum Response {
    Inventory(ProcessInventory),
    LaunchStatus {
        process_id: u32,
        launch_request_id: String,
        state: LaunchState,
        terminal_closed: bool,
    },
    Text {
        process_id: u32,
        window_id: u64,
        tab_id: u64,
        pane_id: u32,
        screen: TextScreen,
        history_lines: u16,
        text: String,
        truncated: bool,
    },
    InputAccepted {
        process_id: u32,
        request_id: String,
        window_id: u64,
        tab_id: u64,
        pane_id: u32,
        input_bytes: u32,
        wire_bytes: u32,
        paste: bool,
        enter: bool,
        disposition: InputDisposition,
        confirmation_id: Option<u64>,
    },
    WindowCreated {
        process_id: u32,
        request_id: String,
        window_id: u64,
    },
    WindowLaunched {
        process_id: u32,
        request_id: String,
        window_id: u64,
        tab_id: u64,
    },
    TabCreated {
        process_id: u32,
        request_id: String,
        window_id: u64,
        tab_id: u64,
    },
    PaneCreated {
        process_id: u32,
        request_id: String,
        window_id: u64,
        tab_id: u64,
        pane_id: u32,
    },
    WindowFocusRequested {
        process_id: u32,
        request_id: String,
        window_id: u64,
        already_focused: bool,
    },
    QuickAccessChanged {
        process_id: u32,
        request_id: String,
        window_id: u64,
        visible: bool,
    },
    TabSelected {
        process_id: u32,
        request_id: String,
        window_id: u64,
        tab_id: u64,
        changed: bool,
    },
    PaneFocused {
        process_id: u32,
        request_id: String,
        window_id: u64,
        tab_id: u64,
        pane_id: u32,
        tab_changed: bool,
        pane_changed: bool,
    },
    WindowCloseRequested {
        process_id: u32,
        request_id: String,
        window_id: u64,
    },
    TabCloseRequested {
        process_id: u32,
        request_id: String,
        window_id: u64,
        tab_id: u64,
    },
    PaneClosed {
        process_id: u32,
        request_id: String,
        window_id: u64,
        tab_id: u64,
        pane_id: u32,
        remaining_panes: u16,
        focused_pane_id: u32,
    },
    TabMoved {
        process_id: u32,
        request_id: String,
        source_window_id: u64,
        source_tab_id: u64,
        destination_window_id: u64,
        destination_tab_id: u64,
        destination_index: u16,
        source_window_closing: bool,
        changed: bool,
    },
    PaneMoved {
        process_id: u32,
        request_id: String,
        source_window_id: u64,
        source_tab_id: u64,
        source_pane_id: u32,
        destination_window_id: u64,
        destination_tab_id: u64,
        destination_pane_id: u32,
        target_pane_id: u32,
    },
    WindowResizeRequested {
        process_id: u32,
        request_id: String,
        window_id: u64,
        requested_width: u32,
        requested_height: u32,
        immediate_width: Option<u32>,
        immediate_height: Option<u32>,
    },
    DividerResized {
        process_id: u32,
        request_id: String,
        window_id: u64,
        tab_id: u64,
        divider_id: u64,
        previous_ratio_millis: u16,
        ratio_millis: u16,
        changed: bool,
    },
    AgentEventRecorded {
        process_id: u32,
    },
    Agents {
        process_id: u32,
        agents: Vec<AgentSummary>,
        truncated: bool,
    },
    Conversation {
        process_id: u32,
        window_id: u64,
        session_id: u64,
        text: String,
        truncated: bool,
    },
    ApprovalAnswered {
        process_id: u32,
        request_id: String,
        window_id: u64,
        session_id: u64,
        /// False when the answer was recorded as a suggestion for the person.
        answered: bool,
    },
    SessionInputAccepted {
        process_id: u32,
        request_id: String,
        window_id: u64,
        session_id: u64,
        pending_approval: bool,
    },
    AgentInputAccepted {
        process_id: u32,
        request_id: String,
        window_id: u64,
        tab_id: u64,
        pane_id: u32,
        /// Held until the person approves it.
        pending_approval: bool,
    },
    /// The request was refused or failed; nothing else in this frame.
    Error {
        message: String,
    },
}

/// What a GUI writes back on its control socket: exactly one reply.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ServerFrame {
    pub version: u32,
    pub response: Response,
}

impl ServerFrame {
    pub fn new(response: Response) -> Self {
        Self {
            version: PROTOCOL_VERSION,
            response,
        }
    }

    pub fn error(message: impl Into<String>) -> Self {
        Self::new(Response::Error {
            message: message.into(),
        })
    }
}

impl Response {
    /// The process that answered; every reply but an error names it.
    pub fn process_id(&self) -> Option<u32> {
        match self {
            Self::Inventory(inventory) => Some(inventory.process_id),
            Self::LaunchStatus { process_id, .. }
            | Self::Text { process_id, .. }
            | Self::InputAccepted { process_id, .. }
            | Self::WindowCreated { process_id, .. }
            | Self::WindowLaunched { process_id, .. }
            | Self::TabCreated { process_id, .. }
            | Self::PaneCreated { process_id, .. }
            | Self::WindowFocusRequested { process_id, .. }
            | Self::QuickAccessChanged { process_id, .. }
            | Self::TabSelected { process_id, .. }
            | Self::PaneFocused { process_id, .. }
            | Self::WindowCloseRequested { process_id, .. }
            | Self::TabCloseRequested { process_id, .. }
            | Self::PaneClosed { process_id, .. }
            | Self::TabMoved { process_id, .. }
            | Self::PaneMoved { process_id, .. }
            | Self::WindowResizeRequested { process_id, .. }
            | Self::DividerResized { process_id, .. }
            | Self::AgentEventRecorded { process_id }
            | Self::Agents { process_id, .. }
            | Self::Conversation { process_id, .. }
            | Self::ApprovalAnswered { process_id, .. }
            | Self::SessionInputAccepted { process_id, .. }
            | Self::AgentInputAccepted { process_id, .. } => Some(*process_id),
            Self::Error { .. } => None,
        }
    }

    /// The replay identity an action's reply echoes.
    pub fn request_id(&self) -> Option<&str> {
        match self {
            Self::InputAccepted { request_id, .. }
            | Self::WindowCreated { request_id, .. }
            | Self::WindowLaunched { request_id, .. }
            | Self::TabCreated { request_id, .. }
            | Self::PaneCreated { request_id, .. }
            | Self::WindowFocusRequested { request_id, .. }
            | Self::QuickAccessChanged { request_id, .. }
            | Self::TabSelected { request_id, .. }
            | Self::PaneFocused { request_id, .. }
            | Self::WindowCloseRequested { request_id, .. }
            | Self::TabCloseRequested { request_id, .. }
            | Self::PaneClosed { request_id, .. }
            | Self::TabMoved { request_id, .. }
            | Self::PaneMoved { request_id, .. }
            | Self::WindowResizeRequested { request_id, .. }
            | Self::DividerResized { request_id, .. }
            | Self::ApprovalAnswered { request_id, .. }
            | Self::SessionInputAccepted { request_id, .. }
            | Self::AgentInputAccepted { request_id, .. } => Some(request_id),
            Self::Inventory(_)
            | Self::LaunchStatus { .. }
            | Self::Text { .. }
            | Self::AgentEventRecorded { .. }
            | Self::Agents { .. }
            | Self::Conversation { .. }
            | Self::Error { .. } => None,
        }
    }

    /// Checks that a reply is consistent in itself; whether it answers a
    /// particular request is for the client that sent it to check. The GUI
    /// checks before sending and the client again on receipt.
    pub fn validate(&self) -> anyhow::Result<()> {
        if let Self::Error { message } = self {
            anyhow::ensure!(
                message.len() <= MAX_ERROR_MESSAGE_BYTES,
                "GUI control error message exceeds its limit"
            );
            return Ok(());
        }
        anyhow::ensure!(
            valid_process_id(self.process_id().unwrap_or_default()),
            "GUI control reply names an invalid process"
        );
        if let Some(request_id) = self.request_id() {
            anyhow::ensure!(
                valid_request_id(request_id),
                "GUI control reply carries an invalid request ID"
            );
        }
        let consistent = match self {
            Self::Inventory(inventory) => {
                inventory.validate()?;
                true
            }
            Self::Agents { agents, .. } => {
                validate_agent_summaries(agents)?;
                true
            }
            Self::LaunchStatus {
                launch_request_id,
                state,
                ..
            } => {
                state.validate().map_err(anyhow::Error::msg)?;
                valid_request_id(launch_request_id)
            }
            Self::AgentEventRecorded { .. } => true,
            Self::Text {
                window_id,
                history_lines,
                text,
                ..
            } => {
                *window_id != 0
                    && *history_lines <= MAX_TEXT_HISTORY_LINES
                    && text.len() <= MAX_TERMINAL_TEXT_BYTES as usize
            }
            Self::Conversation {
                window_id,
                session_id,
                text,
                ..
            } => {
                *window_id != 0
                    && *session_id != 0
                    && text.len() <= MAX_TERMINAL_TEXT_BYTES as usize
            }
            Self::InputAccepted {
                window_id,
                input_bytes,
                wire_bytes,
                disposition,
                confirmation_id,
                ..
            } => {
                let valid_disposition = match (disposition, confirmation_id) {
                    (InputDisposition::Sent, None) => true,
                    (InputDisposition::PendingConfirmation, Some(id)) => *id != 0,
                    _ => false,
                };
                // Enter adds one carriage return to the text, and a
                // bracketed paste its delimiters on the wire.
                let max_input = MAX_CONTROL_INPUT_BYTES + 1;
                *window_id != 0
                    && *input_bytes != 0
                    && *input_bytes as usize <= max_input
                    && *wire_bytes >= *input_bytes
                    && *wire_bytes as usize <= max_input + crate::input::BRACKETED_PASTE_OVERHEAD
                    && valid_disposition
            }
            Self::PaneCreated {
                window_id, pane_id, ..
            } => *window_id != 0 && *pane_id != 0,
            Self::PaneClosed {
                window_id,
                remaining_panes,
                ..
            } => {
                *window_id != 0
                    && *remaining_panes != 0
                    && usize::from(*remaining_panes) <= MAX_PANES_PER_TAB
            }
            Self::TabMoved {
                source_window_id,
                destination_window_id,
                destination_index,
                ..
            } => {
                *source_window_id != 0
                    && *destination_window_id != 0
                    && usize::from(*destination_index) < MAX_TABS_PER_WINDOW
            }
            Self::PaneMoved {
                source_window_id,
                destination_window_id,
                destination_pane_id,
                target_pane_id,
                ..
            } => {
                *source_window_id != 0
                    && *destination_window_id != 0
                    && destination_pane_id != target_pane_id
            }
            Self::WindowResizeRequested {
                window_id,
                requested_width,
                requested_height,
                immediate_width,
                immediate_height,
                ..
            } => {
                *window_id != 0
                    && (MIN_WINDOW_WIDTH..=MAX_WINDOW_DIMENSION).contains(requested_width)
                    && (MIN_WINDOW_HEIGHT..=MAX_WINDOW_DIMENSION).contains(requested_height)
                    && immediate_width.is_some() == immediate_height.is_some()
                    && *immediate_width != Some(0)
                    && *immediate_height != Some(0)
            }
            Self::DividerResized {
                window_id,
                previous_ratio_millis,
                ratio_millis,
                changed,
                ..
            } => {
                *window_id != 0
                    && RATIO_MILLIS.contains(previous_ratio_millis)
                    && RATIO_MILLIS.contains(ratio_millis)
                    && *changed == (previous_ratio_millis != ratio_millis)
            }
            Self::WindowCreated { window_id, .. }
            | Self::WindowLaunched { window_id, .. }
            | Self::TabCreated { window_id, .. }
            | Self::WindowFocusRequested { window_id, .. }
            | Self::QuickAccessChanged { window_id, .. }
            | Self::TabSelected { window_id, .. }
            | Self::PaneFocused { window_id, .. }
            | Self::WindowCloseRequested { window_id, .. }
            | Self::TabCloseRequested { window_id, .. }
            | Self::ApprovalAnswered { window_id, .. }
            | Self::SessionInputAccepted { window_id, .. }
            | Self::AgentInputAccepted { window_id, .. } => *window_id != 0,
            Self::Error { .. } => unreachable!("errors return above"),
        };
        anyhow::ensure!(consistent, "the GUI control reply is inconsistent");
        Ok(())
    }
}

impl Request {
    pub fn canonicalize(&mut self) {
        let (environment, environment_policy) = match self {
            Self::LaunchWindow {
                environment,
                environment_policy,
                ..
            }
            | Self::LaunchTab {
                environment,
                environment_policy,
                ..
            } => (environment, environment_policy),
            _ => return,
        };
        environment.sort_by(|left, right| left.name.cmp(&right.name));
        environment_policy.unset.sort();
    }

    pub fn validate(&self) -> Result<(), String> {
        if self
            .request_id()
            .is_some_and(|request_id| !valid_request_id(request_id))
        {
            return Err("GUI control request ID must be exactly 32 lowercase hex digits".into());
        }
        match self {
            Self::Inventory => Ok(()),
            Self::ListAgents { supervisor } => validate_supervisor(supervisor.as_deref()),
            Self::SubmitToAgent {
                window_id,
                agent_id,
                text,
                supervisor,
                ..
            } => {
                validate_supervisor(supervisor.as_deref())?;
                require_target_window(*window_id)?;
                if !valid_agent_id(agent_id) {
                    return Err("Agent ID must be 1-64 letters, digits, '-' or '_'".into());
                }
                if text.trim().is_empty() || text.len() > MAX_CONTROL_INPUT_BYTES {
                    return Err(format!(
                        "Agent input must be 1 to {MAX_CONTROL_INPUT_BYTES} UTF-8 bytes of text"
                    ));
                }
                if !valid_control_input_text(text) {
                    return Err("Agent input permits printable UTF-8, tab, and newline only".into());
                }
                Ok(())
            }
            Self::ReadConversation {
                window_id,
                session_id,
            } => {
                if *window_id == 0 || *session_id == 0 {
                    return Err("Conversation reads need a nonzero window and session".into());
                }
                Ok(())
            }
            Self::AnswerApproval {
                window_id,
                session_id,
                approval_id,
                reason,
                supervisor,
                ..
            } => {
                validate_supervisor(supervisor.as_deref())?;
                if *window_id == 0 || *session_id == 0 {
                    return Err("Approval answers need a nonzero window and session".into());
                }
                if !valid_approval_id(approval_id) {
                    return Err(format!(
                        "Approval IDs are 1 to {MAX_APPROVAL_ID_BYTES} visible ASCII characters"
                    ));
                }
                if reason.len() > MAX_APPROVAL_REASON_BYTES || !valid_control_input_text(reason) {
                    return Err(format!(
                        "An approval reason is at most {MAX_APPROVAL_REASON_BYTES} bytes of \
                         printable text"
                    ));
                }
                Ok(())
            }
            Self::SubmitToSession {
                window_id,
                session_id,
                text,
                supervisor,
                ..
            } => {
                validate_supervisor(supervisor.as_deref())?;
                if *window_id == 0 || *session_id == 0 {
                    return Err("Session input needs a nonzero window and session".into());
                }
                if text.trim().is_empty() || text.len() > MAX_CONTROL_INPUT_BYTES {
                    return Err(format!(
                        "Agent input must be 1 to {MAX_CONTROL_INPUT_BYTES} UTF-8 bytes of text"
                    ));
                }
                if !valid_control_input_text(text) {
                    return Err("Agent input permits printable UTF-8, tab, and newline only".into());
                }
                Ok(())
            }
            Self::AgentEvent {
                terminal_id,
                agent_id,
                detail,
                last_message,
                transcript_path,
                ..
            } => validate_agent_event(
                terminal_id,
                agent_id,
                detail.as_deref(),
                last_message.as_deref(),
                transcript_path.as_deref(),
            ),
            Self::LaunchStatus { launch_request_id } => {
                if !valid_request_id(launch_request_id) {
                    return Err("Launch request ID must be exactly 32 lowercase hex digits".into());
                }
                Ok(())
            }
            Self::ReadText {
                window_id,
                history_lines,
                max_bytes,
                ..
            } => {
                require_target_window(*window_id)?;
                if *history_lines > MAX_TEXT_HISTORY_LINES {
                    return Err(format!(
                        "GUI terminal-text history is limited to {MAX_TEXT_HISTORY_LINES} lines"
                    ));
                }
                if !(1..=MAX_TERMINAL_TEXT_BYTES).contains(max_bytes) {
                    return Err(format!(
                        "GUI terminal-text output must be between 1 and {MAX_TERMINAL_TEXT_BYTES} bytes"
                    ));
                }
                Ok(())
            }
            Self::SendText {
                window_id,
                text,
                enter,
                ..
            } => {
                require_target_window(*window_id)?;
                if text.len() > MAX_CONTROL_INPUT_BYTES {
                    return Err(format!(
                        "GUI control input is limited to {MAX_CONTROL_INPUT_BYTES} UTF-8 bytes"
                    ));
                }
                if text.is_empty() && !enter {
                    return Err("GUI control input must include text or --enter".into());
                }
                if !valid_control_input_text(text) {
                    return Err(
                        "GUI control input permits printable UTF-8, tab, and newline only".into(),
                    );
                }
                Ok(())
            }
            Self::CreateWindow { source_window, .. } => {
                if *source_window == 0 {
                    return Err("GUI control source window ID must be nonzero".into());
                }
                Ok(())
            }
            Self::JoinLaunch {
                display,
                activation_token,
                build,
                environment,
                tabs,
                ..
            } => {
                let identity = |value: &str| {
                    !value.is_empty()
                        && value.len() <= MAX_JOIN_IDENTITY_BYTES
                        && !value.chars().any(crate::safe_text::is_unsafe_display_char)
                };
                if !identity(display)
                    || !identity(build)
                    || activation_token
                        .as_deref()
                        .is_some_and(|token| !identity(token))
                {
                    return Err(format!(
                        "A joining launch's display, build, and activation token must be printable UTF-8 of at most {MAX_JOIN_IDENTITY_BYTES} bytes"
                    ));
                }
                if !termeric_core::hex::is_lower(
                    environment,
                    crate::control::ENVIRONMENT_IDENTITY_DIGITS,
                ) {
                    return Err(format!(
                        "A joining launch's environment identity must be exactly {} lowercase hex digits",
                        crate::control::ENVIRONMENT_IDENTITY_DIGITS
                    ));
                }
                if tabs.is_empty() || tabs.len() > MAX_JOIN_TABS {
                    return Err(format!(
                        "A joining launch opens between 1 and {MAX_JOIN_TABS} tabs"
                    ));
                }
                for tab in tabs {
                    match &tab.program {
                        Some(program) => validate_control_launch(
                            program,
                            &tab.args,
                            tab.working_directory.as_deref(),
                            tab.title.as_deref(),
                            &[],
                            &termeric_core::pty::EnvironmentPolicy::default(),
                        )?,
                        None if !tab.args.is_empty() => {
                            return Err("A joining launch's shell tab takes no arguments".into());
                        }
                        None => validate_launch_place(
                            tab.working_directory.as_deref(),
                            tab.title.as_deref(),
                        )?,
                    }
                }
                Ok(())
            }
            Self::CreateTab { window_id, .. } => {
                require_target_window(*window_id)?;
                Ok(())
            }
            Self::LaunchTab {
                background: _,
                window_id,
                program,
                args,
                working_directory,
                title,
                environment,
                environment_policy,
                ..
            } => {
                require_target_window(*window_id)?;
                validate_control_launch(
                    program,
                    args,
                    working_directory.as_deref(),
                    title.as_deref(),
                    environment,
                    environment_policy,
                )
            }
            Self::LaunchWindow {
                source_window,
                program,
                args,
                working_directory,
                title,
                environment,
                environment_policy,
                ..
            } => {
                if *source_window == 0 {
                    return Err("GUI control source window ID must be nonzero".into());
                }
                validate_control_launch(
                    program,
                    args,
                    working_directory.as_deref(),
                    title.as_deref(),
                    environment,
                    environment_policy,
                )
            }
            Self::SplitPane { window_id, .. } => {
                require_target_window(*window_id)?;
                Ok(())
            }
            Self::FocusWindow { window_id, .. }
            | Self::SetQuickAccessVisibility { window_id, .. }
            | Self::SelectTab { window_id, .. }
            | Self::FocusPane { window_id, .. }
            | Self::CloseWindow { window_id, .. }
            | Self::CloseTab { window_id, .. }
            | Self::ClosePane { window_id, .. } => {
                require_target_window(*window_id)?;
                Ok(())
            }
            Self::MoveTab {
                source_window_id,
                destination_window_id,
                destination_index,
                ..
            } => {
                if *source_window_id == 0 || *destination_window_id == 0 {
                    return Err(
                        "GUI control source and destination window IDs must be nonzero".into(),
                    );
                }
                // A window never holds more than MAX_TABS_PER_WINDOW tabs, so
                // the last position a tab can land on is one below it.
                if usize::from(*destination_index) >= MAX_TABS_PER_WINDOW {
                    return Err("GUI control destination tab index exceeds the window limit".into());
                }
                Ok(())
            }
            Self::MovePane {
                source_window_id,
                destination_window_id,
                ..
            } => {
                if *source_window_id == 0 || *destination_window_id == 0 {
                    return Err(
                        "GUI control source and destination window IDs must be nonzero".into(),
                    );
                }
                Ok(())
            }
            Self::ResizeWindow {
                window_id,
                width,
                height,
                ..
            } => {
                require_target_window(*window_id)?;
                if !(MIN_WINDOW_WIDTH..=MAX_WINDOW_DIMENSION).contains(width)
                    || !(MIN_WINDOW_HEIGHT..=MAX_WINDOW_DIMENSION).contains(height)
                {
                    return Err(format!(
                        "GUI window dimensions must be between {MIN_WINDOW_WIDTH}x{MIN_WINDOW_HEIGHT} and {MAX_WINDOW_DIMENSION}x{MAX_WINDOW_DIMENSION} pixels"
                    ));
                }
                Ok(())
            }
            Self::ResizeDivider {
                window_id,
                ratio_millis,
                ..
            } => {
                require_target_window(*window_id)?;
                if !RATIO_MILLIS.contains(ratio_millis) {
                    return Err("GUI divider ratio must be between 100 and 900 thousandths".into());
                }
                Ok(())
            }
        }
    }

    /// The replay identity of an action; reads and reports have none.
    pub fn request_id(&self) -> Option<&str> {
        match self {
            Self::Inventory
            | Self::ReadText { .. }
            | Self::LaunchStatus { .. }
            | Self::ListAgents { .. }
            | Self::ReadConversation { .. }
            | Self::AgentEvent { .. } => None,
            Self::CreateWindow { request_id, .. }
            | Self::JoinLaunch { request_id, .. }
            | Self::LaunchWindow { request_id, .. }
            | Self::CreateTab { request_id, .. }
            | Self::LaunchTab { request_id, .. }
            | Self::SplitPane { request_id, .. }
            | Self::FocusWindow { request_id, .. }
            | Self::SetQuickAccessVisibility { request_id, .. }
            | Self::SelectTab { request_id, .. }
            | Self::FocusPane { request_id, .. }
            | Self::CloseWindow { request_id, .. }
            | Self::CloseTab { request_id, .. }
            | Self::ClosePane { request_id, .. }
            | Self::MoveTab { request_id, .. }
            | Self::MovePane { request_id, .. }
            | Self::ResizeWindow { request_id, .. }
            | Self::ResizeDivider { request_id, .. }
            | Self::SendText { request_id, .. }
            | Self::SubmitToAgent { request_id, .. }
            | Self::AnswerApproval { request_id, .. }
            | Self::SubmitToSession { request_id, .. } => Some(request_id),
        }
    }

    /// [`Self::request_id`], for the client to fill in before sending.
    pub fn request_id_mut(&mut self) -> Option<&mut String> {
        match self {
            // Agent events restate the agent's current state; repeating one
            // is harmless, so they carry no replay identity.
            Self::Inventory
            | Self::ReadText { .. }
            | Self::LaunchStatus { .. }
            | Self::ListAgents { .. }
            | Self::ReadConversation { .. }
            | Self::AgentEvent { .. } => None,
            Self::CreateWindow { request_id, .. }
            | Self::JoinLaunch { request_id, .. }
            | Self::LaunchWindow { request_id, .. }
            | Self::CreateTab { request_id, .. }
            | Self::LaunchTab { request_id, .. }
            | Self::SplitPane { request_id, .. }
            | Self::FocusWindow { request_id, .. }
            | Self::SetQuickAccessVisibility { request_id, .. }
            | Self::SelectTab { request_id, .. }
            | Self::FocusPane { request_id, .. }
            | Self::CloseWindow { request_id, .. }
            | Self::CloseTab { request_id, .. }
            | Self::ClosePane { request_id, .. }
            | Self::MoveTab { request_id, .. }
            | Self::MovePane { request_id, .. }
            | Self::ResizeWindow { request_id, .. }
            | Self::ResizeDivider { request_id, .. }
            | Self::SendText { request_id, .. }
            | Self::SubmitToAgent { request_id, .. }
            | Self::AnswerApproval { request_id, .. }
            | Self::SubmitToSession { request_id, .. } => Some(request_id),
        }
    }
}

/// Supervisor tokens are 32 lowercase hex digits, like request IDs.
fn require_target_window(window_id: u64) -> Result<(), String> {
    if window_id == 0 {
        return Err("GUI control target window ID must be nonzero".into());
    }
    Ok(())
}

fn validate_supervisor(token: Option<&str>) -> Result<(), String> {
    if token.is_some_and(|token| !valid_request_id(token)) {
        return Err("Supervisor token must be exactly 32 lowercase hex digits".into());
    }
    Ok(())
}

fn validate_agent_event(
    terminal_id: &str,
    agent_id: &str,
    detail: Option<&str>,
    last_message: Option<&str>,
    transcript_path: Option<&str>,
) -> Result<(), String> {
    if terminal_id.len() > MAX_AGENT_TERMINAL_ID_BYTES
        || termeric_core::pty::terminal_id_process(terminal_id).is_none()
    {
        return Err("Agent terminal identity must be PID.HEX".into());
    }
    if !valid_agent_id(agent_id) {
        return Err("Agent ID must be 1-64 letters, digits, '-' or '_'".into());
    }
    if detail.is_some_and(|detail| detail.len() > MAX_AGENT_EVENT_DETAIL_BYTES) {
        return Err(format!(
            "Agent event detail is limited to {MAX_AGENT_EVENT_DETAIL_BYTES} bytes"
        ));
    }
    if last_message.is_some_and(|message| message.len() > MAX_AGENT_LAST_MESSAGE_BYTES) {
        return Err(format!(
            "Agent last message is limited to {MAX_AGENT_LAST_MESSAGE_BYTES} bytes"
        ));
    }
    if transcript_path.is_some_and(|path| {
        path.len() > MAX_AGENT_TRANSCRIPT_PATH_BYTES
            || !path.starts_with('/')
            || path.contains('\0')
    }) {
        return Err("Agent transcript path must be absolute and bounded".into());
    }
    Ok(())
}

fn validate_control_launch(
    program: &str,
    args: &[String],
    working_directory: Option<&str>,
    title: Option<&str>,
    environment: &[EnvironmentVariable],
    environment_policy: &termeric_core::pty::EnvironmentPolicy,
) -> Result<(), String> {
    if program.is_empty()
        || program
            .chars()
            .any(crate::safe_text::is_unsafe_display_char)
    {
        return Err(
            "GUI control launch program must be nonempty UTF-8 without control or bidirectional formatting text"
                .into(),
        );
    }
    if args.len() > MAX_CONTROL_LAUNCH_ARGS {
        return Err(format!(
            "GUI control launch is limited to {MAX_CONTROL_LAUNCH_ARGS} arguments"
        ));
    }
    if args.iter().any(|argument| argument.contains('\0')) {
        return Err("GUI control launch arguments cannot contain NUL bytes".into());
    }
    let argv_bytes = args.iter().fold(program.len(), |total, argument| {
        total.saturating_add(argument.len())
    });
    if argv_bytes > MAX_CONTROL_LAUNCH_ARGV_BYTES {
        return Err(format!(
            "GUI control launch program and arguments are limited to {MAX_CONTROL_LAUNCH_ARGV_BYTES} UTF-8 bytes"
        ));
    }
    validate_launch_place(working_directory, title)?;
    validate_control_launch_environment(environment, environment_policy)?;
    Ok(())
}

/// A launch's folder and title: an absolute path and a single printable
/// line, each within its limit.
fn validate_launch_place(
    working_directory: Option<&str>,
    title: Option<&str>,
) -> Result<(), String> {
    if let Some(directory) = working_directory
        && (directory.is_empty()
            || directory.len() > MAX_CONTROL_LAUNCH_CWD_BYTES
            || !std::path::Path::new(directory).is_absolute()
            || directory
                .chars()
                .any(crate::safe_text::is_unsafe_display_char))
    {
        return Err(format!(
            "GUI control launch working directory must be an absolute UTF-8 path of at most {MAX_CONTROL_LAUNCH_CWD_BYTES} bytes without control or bidirectional formatting text"
        ));
    }
    if let Some(title) = title
        && (title.trim().is_empty()
            || title.len() > MAX_CONTROL_LAUNCH_TITLE_BYTES
            || title.chars().any(crate::safe_text::is_unsafe_display_char))
    {
        return Err(format!(
            "GUI control launch title must be nonempty UTF-8 of at most {MAX_CONTROL_LAUNCH_TITLE_BYTES} bytes without control or bidirectional formatting text"
        ));
    }
    Ok(())
}

pub(super) fn validate_control_launch_environment(
    environment: &[EnvironmentVariable],
    policy: &termeric_core::pty::EnvironmentPolicy,
) -> Result<(), String> {
    if environment.len().saturating_add(policy.unset.len()) > MAX_CONTROL_LAUNCH_ENVIRONMENT_ENTRIES
    {
        return Err(format!(
            "GUI control launch environment is limited to {MAX_CONTROL_LAUNCH_ENVIRONMENT_ENTRIES} entries"
        ));
    }
    let mut names = HashSet::with_capacity(environment.len());
    policy.validate().map_err(str::to_string)?;
    let mut total_bytes = 0usize;
    for name in &policy.unset {
        if name.len() > MAX_CONTROL_LAUNCH_ENVIRONMENT_NAME_BYTES {
            return Err("GUI control launch environment removal name is too long".into());
        }
        names.insert(name.as_str());
        total_bytes = total_bytes.saturating_add(name.len());
    }
    for entry in environment {
        let mut bytes = entry.name.bytes();
        let valid_name = bytes
            .next()
            .is_some_and(|byte| byte == b'_' || byte.is_ascii_alphabetic())
            && bytes.all(|byte| byte == b'_' || byte.is_ascii_alphanumeric());
        let reserved =
            entry.name == "PWD" || termeric_core::pty::terminal_identity_variable(&entry.name);
        if !valid_name || entry.name.len() > MAX_CONTROL_LAUNCH_ENVIRONMENT_NAME_BYTES || reserved {
            return Err(
                "GUI control launch environment contains an invalid or reserved variable name"
                    .into(),
            );
        }
        if !names.insert(entry.name.as_str()) {
            return Err("GUI control launch environment contains a duplicate variable name".into());
        }
        if entry.value.len() > MAX_CONTROL_LAUNCH_ENVIRONMENT_VALUE_BYTES
            || entry.value.contains('\0')
        {
            return Err(format!(
                "GUI control launch environment values must be NUL-free UTF-8 of at most {MAX_CONTROL_LAUNCH_ENVIRONMENT_VALUE_BYTES} bytes"
            ));
        }
        total_bytes = total_bytes
            .saturating_add(entry.name.len())
            .saturating_add(entry.value.len());
    }
    if total_bytes > MAX_CONTROL_LAUNCH_ENVIRONMENT_BYTES {
        return Err(format!(
            "GUI control launch environment is limited to {MAX_CONTROL_LAUNCH_ENVIRONMENT_BYTES} UTF-8 bytes"
        ));
    }
    Ok(())
}

pub fn valid_control_input_text(value: &str) -> bool {
    value
        .chars()
        .all(|character| !character.is_control() || matches!(character, '\n' | '\t'))
}

/// An agent's own identifier for one approval request.
pub fn valid_approval_id(value: &str) -> bool {
    (1..=MAX_APPROVAL_ID_BYTES).contains(&value.len())
        && value.bytes().all(|byte| byte.is_ascii_graphic())
}

pub fn valid_request_id(value: &str) -> bool {
    termeric_core::hex::is_lower(value, 32)
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProcessInventory {
    pub process_id: u32,
    pub windows: Vec<WindowInventory>,
    pub truncated: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WindowInventory {
    /// Stable within the lifetime of this GUI process.
    pub id: u64,
    pub focused: bool,
    pub visible: bool,
    pub quick_access: bool,
    pub workspace_id: Option<String>,
    pub pixel_width: u32,
    pub pixel_height: u32,
    pub tabs: Vec<TabInventory>,
    pub truncated: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TabInventory {
    /// Stable within the lifetime of this window.
    pub id: u64,
    pub title: String,
    pub kind: TabKind,
    pub active: bool,
    pub pinned: bool,
    pub activity: bool,
    pub closed: bool,
    pub domain_id: Option<String>,
    pub panes: Vec<PaneInventory>,
    pub dividers: Vec<DividerInventory>,
    pub truncated: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TabKind {
    Shell,
    Profile,
    Plugin,
    LiveDomain,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PaneInventory {
    /// Stable within the lifetime of this tab.
    pub id: u32,
    pub title: Option<String>,
    pub focused: bool,
    pub closed: bool,
    pub cols: u16,
    pub rows: u16,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DividerInventory {
    /// Stable within the lifetime of this tab topology.
    pub id: u64,
    pub kind: DividerKind,
    pub direction: SplitDirection,
    /// First-child ratio in thousandths, bounded to 100 through 900.
    pub ratio_millis: u16,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DividerKind {
    Local,
    LiveDomain,
}

impl ProcessInventory {
    pub fn validate(&self) -> anyhow::Result<()> {
        if !valid_process_id(self.process_id) {
            anyhow::bail!("GUI inventory has an invalid process ID");
        }
        if self.windows.len() > MAX_WINDOWS {
            anyhow::bail!("GUI inventory exceeds the window limit");
        }
        let mut window_ids = HashSet::with_capacity(self.windows.len());
        let mut tab_count = 0usize;
        let mut pane_count = 0usize;
        let mut divider_count = 0usize;
        let mut metadata_bytes = 0usize;
        let mut quick_access_windows = 0usize;
        for window in &self.windows {
            if window.id == 0 || !window_ids.insert(window.id) {
                anyhow::bail!("GUI inventory contains an invalid or duplicate window ID");
            }
            if window.pixel_width == 0 || window.pixel_height == 0 {
                anyhow::bail!("GUI inventory contains zero-sized window geometry");
            }
            quick_access_windows += usize::from(window.quick_access);
            if quick_access_windows > 1 {
                anyhow::bail!("GUI inventory contains multiple quick-access windows");
            }
            validate_metadata(window.workspace_id.as_deref(), &mut metadata_bytes)?;
            if window.tabs.len() > MAX_TABS_PER_WINDOW {
                anyhow::bail!("GUI inventory exceeds the per-window tab limit");
            }
            tab_count = tab_count
                .checked_add(window.tabs.len())
                .context("GUI inventory tab count overflow")?;
            let mut tab_ids = HashSet::with_capacity(window.tabs.len());
            for tab in &window.tabs {
                if !tab_ids.insert(tab.id) {
                    anyhow::bail!("GUI inventory contains a duplicate tab ID");
                }
                validate_metadata(Some(&tab.title), &mut metadata_bytes)?;
                validate_metadata(tab.domain_id.as_deref(), &mut metadata_bytes)?;
                if tab.panes.len() > MAX_PANES_PER_TAB {
                    anyhow::bail!("GUI inventory exceeds the per-tab pane limit");
                }
                if tab.dividers.len() > MAX_DIVIDERS_PER_TAB {
                    anyhow::bail!("GUI inventory exceeds the per-tab divider limit");
                }
                if tab.dividers.len() > tab.panes.len().saturating_sub(1) {
                    anyhow::bail!("GUI inventory has more dividers than its pane tree permits");
                }
                pane_count = pane_count
                    .checked_add(tab.panes.len())
                    .context("GUI inventory pane count overflow")?;
                let mut pane_ids = HashSet::with_capacity(tab.panes.len());
                for pane in &tab.panes {
                    if !pane_ids.insert(pane.id) || pane.cols == 0 || pane.rows == 0 {
                        anyhow::bail!(
                            "GUI inventory contains a duplicate pane ID or zero cell geometry"
                        );
                    }
                    validate_metadata(pane.title.as_deref(), &mut metadata_bytes)?;
                }
                divider_count = divider_count
                    .checked_add(tab.dividers.len())
                    .context("GUI inventory divider count overflow")?;
                let mut divider_ids = HashSet::with_capacity(tab.dividers.len());
                for divider in &tab.dividers {
                    if !divider_ids.insert(divider.id)
                        || !RATIO_MILLIS.contains(&divider.ratio_millis)
                    {
                        anyhow::bail!("GUI inventory contains an invalid or duplicate divider");
                    }
                }
            }
        }
        if tab_count > MAX_TOTAL_TABS
            || pane_count > MAX_TOTAL_PANES
            || divider_count > MAX_TOTAL_DIVIDERS
            || metadata_bytes > MAX_METADATA_BYTES
        {
            anyhow::bail!("GUI inventory exceeds its process-wide resource limits");
        }
        Ok(())
    }
}

fn validate_metadata(value: Option<&str>, total: &mut usize) -> anyhow::Result<()> {
    const MAX_FIELD_BYTES: usize = 1_024;
    let Some(value) = value else {
        return Ok(());
    };
    if value.len() > MAX_FIELD_BYTES || value.chars().any(crate::safe_text::is_unsafe_display_char)
    {
        anyhow::bail!("GUI inventory contains unsafe or oversized metadata");
    }
    *total = total
        .checked_add(value.len())
        .context("GUI inventory metadata size overflow")?;
    Ok(())
}

pub fn write_frame<T: Serialize>(writer: &mut impl Write, frame: &T) -> anyhow::Result<()> {
    crate::domain::protocol::write_json_frame(writer, frame, MAX_FRAME_BYTES, "GUI control")
}

pub fn read_frame<T: DeserializeOwned>(reader: &mut impl Read) -> anyhow::Result<T> {
    crate::domain::protocol::read_json_frame(reader, MAX_FRAME_BYTES, "GUI control")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `value` crosses the wire unchanged.
    fn round_trip<T: Serialize + DeserializeOwned + PartialEq + std::fmt::Debug>(value: &T) {
        let mut wire = Vec::new();
        write_frame(&mut wire, value).unwrap();
        assert_eq!(&read_frame::<T>(&mut wire.as_slice()).unwrap(), value);
    }

    /// `request` is valid and crosses the wire unchanged.
    fn sends_unchanged(request: Request) {
        request.validate().unwrap();
        round_trip(&ClientFrame {
            version: PROTOCOL_VERSION,
            token: "secret".into(),
            request,
        });
    }

    #[test]
    fn process_and_agent_ids_have_one_rule_everywhere() {
        assert!(!valid_process_id(0));
        assert!(valid_process_id(1));
        assert!(valid_process_id(MAX_PROCESS_ID));
        assert!(!valid_process_id(MAX_PROCESS_ID + 1));
        assert!(valid_agent_id("claude-code_2"));
        assert!(valid_agent_id(&"a".repeat(MAX_AGENT_ID_BYTES)));
        for invalid in [
            "",
            "co dex",
            "codex.v2",
            &"a".repeat(MAX_AGENT_ID_BYTES + 1),
        ] {
            assert!(!valid_agent_id(invalid), "{invalid:?}");
        }
    }

    #[test]
    fn agent_lists_are_cut_to_fit_one_frame() {
        let summary = |index: u64| AgentSummary {
            window_id: 1,
            tab_id: index,
            pane_id: 0,
            session_id: None,
            agent_id: "claude-code".into(),
            name: "Claude Code".into(),
            tab_title: "tab".into(),
            state: Some(AgentState::Idle),
            detail: None,
            last_message: Some("m".repeat(MAX_AGENT_SUMMARY_MESSAGE_BYTES)),
            seconds_in_state: Some(1),
        };
        let mut few: Vec<_> = (1..=3).map(summary).collect();
        assert!(!fit_agent_summaries(&mut few));
        assert_eq!(few.len(), 3);

        let mut many: Vec<_> = (1..=40).map(summary).collect();
        assert!(fit_agent_summaries(&mut many));
        assert!(many.len() < 40 && !many.is_empty());
        assert_eq!(many.last().unwrap().tab_id, many.len() as u64);
        let frame = ServerFrame::new(Response::Agents {
            process_id: 1,
            agents: many,
            truncated: true,
        });
        let mut wire = Vec::new();
        write_frame(&mut wire, &frame).unwrap();
    }

    #[test]
    fn launch_environment_policy_is_bounded_unambiguous_and_strict() {
        use termeric_core::pty::EnvironmentPolicy;
        let clear = EnvironmentPolicy {
            clear: true,
            unset: vec![],
        };
        let assignment = vec![EnvironmentVariable {
            name: "BUILD_VALUE".into(),
            value: String::new(),
        }];
        validate_control_launch_environment(&assignment, &clear).unwrap();
        for unset in [
            vec!["PATH".into(), "LANG".into()],
            (0..MAX_CONTROL_LAUNCH_ENVIRONMENT_ENTRIES)
                .map(|index| format!("VALUE_{index}"))
                .collect(),
        ] {
            validate_control_launch_environment(
                &[],
                &EnvironmentPolicy {
                    clear: false,
                    unset,
                },
            )
            .unwrap();
        }
        for policy in [
            EnvironmentPolicy {
                clear: true,
                unset: vec!["PATH".into()],
            },
            EnvironmentPolicy {
                clear: false,
                unset: vec!["PATH".into(), "PATH".into()],
            },
            EnvironmentPolicy {
                clear: false,
                unset: vec!["BUILD_VALUE".into()],
            },
            EnvironmentPolicy {
                clear: false,
                unset: vec!["X".repeat(MAX_CONTROL_LAUNCH_ENVIRONMENT_NAME_BYTES + 1)],
            },
            EnvironmentPolicy {
                clear: false,
                unset: (0..MAX_CONTROL_LAUNCH_ENVIRONMENT_ENTRIES)
                    .map(|index| format!("V_{index}"))
                    .collect(),
            },
        ] {
            assert!(validate_control_launch_environment(&assignment, &policy).is_err());
        }
        for name in [
            "",
            "1BAD",
            "A=B",
            "A\0B",
            "λ",
            "TERM",
            "COLORTERM",
            "TERMINFO",
            "TERMINFO_DIRS",
            "TERM_PROGRAM",
            "TERM_PROGRAM_VERSION",
            "TERMERIC_TERMINAL_ID",
            "PWD",
            "SHELL",
        ] {
            assert!(
                validate_control_launch_environment(
                    &[],
                    &EnvironmentPolicy {
                        clear: false,
                        unset: vec![name.into()]
                    }
                )
                .is_err()
            );
        }
        let mut budget = MAX_CONTROL_LAUNCH_ENVIRONMENT_BYTES;
        let mut assignments = Vec::new();
        while budget > 0 {
            let name = format!("K{:03}", assignments.len());
            let bytes = (budget - name.len()).min(MAX_CONTROL_LAUNCH_ENVIRONMENT_VALUE_BYTES);
            budget -= name.len() + bytes;
            assignments.push(EnvironmentVariable {
                name,
                value: "x".repeat(bytes),
            });
        }
        validate_control_launch_environment(&assignments, &Default::default()).unwrap();
        assert!(
            validate_control_launch_environment(
                &assignments,
                &EnvironmentPolicy {
                    clear: false,
                    unset: vec!["MISSING_FROM_SET".into()]
                }
            )
            .unwrap_err()
            .contains("bytes")
        );
        for wire in [
            r#"{"clear":true,"unset":[],"extra":true}"#,
            r#"{"clear":"true","unset":[]}"#,
            r#"{"clear":true}"#,
        ] {
            assert!(serde_json::from_str::<EnvironmentPolicy>(wire).is_err());
        }
    }

    #[test]
    fn launch_status_is_read_only_strict_and_preserves_portable_results() {
        let request = Request::LaunchStatus {
            launch_request_id: "1".repeat(32),
        };
        request.validate().unwrap();
        assert!(request.request_id().is_none());
        sends_unchanged(request);
        assert!(
            Request::LaunchStatus {
                launch_request_id: "wrong".into()
            }
            .validate()
            .is_err()
        );
        assert!(
            LaunchState::Exited {
                exit_code: 0,
                success: false
            }
            .validate()
            .is_err()
        );
        assert!(
            LaunchState::Exited {
                exit_code: 7,
                success: true
            }
            .validate()
            .is_err()
        );
        assert!(
            serde_json::from_str::<LaunchState>(r#"{"status":"running","exit_code":0}"#).is_err()
        );
        for state in [
            LaunchState::Running {},
            LaunchState::Exited {
                exit_code: 7,
                success: false,
            },
        ] {
            let response = ServerFrame::new(Response::LaunchStatus {
                process_id: 42,
                launch_request_id: "1".repeat(32),
                state,
                terminal_closed: true,
            });
            round_trip(&response);
        }
    }

    #[test]
    fn inventory_round_trips_with_stable_versioned_shape() {
        let frame = ServerFrame::new(Response::Inventory(ProcessInventory {
            process_id: 42,
            windows: vec![WindowInventory {
                id: 7,
                focused: true,
                visible: true,
                quick_access: false,
                workspace_id: Some("daily".into()),
                pixel_width: 1_200,
                pixel_height: 800,
                tabs: vec![TabInventory {
                    id: 3,
                    title: "build".into(),
                    kind: TabKind::Shell,
                    active: true,
                    pinned: false,
                    activity: false,
                    closed: false,
                    domain_id: None,
                    panes: vec![
                        PaneInventory {
                            id: 0,
                            title: None,
                            focused: true,
                            closed: false,
                            cols: 120,
                            rows: 40,
                        },
                        PaneInventory {
                            id: 1,
                            title: Some("logs".into()),
                            focused: false,
                            closed: false,
                            cols: 120,
                            rows: 40,
                        },
                    ],
                    dividers: vec![DividerInventory {
                        id: 1,
                        kind: DividerKind::Local,
                        direction: SplitDirection::Vertical,
                        ratio_millis: 500,
                    }],
                    truncated: false,
                }],
                truncated: false,
            }],
            truncated: false,
        }));
        round_trip(&frame);
    }

    #[test]
    fn create_window_requests_and_responses_are_explicitly_versioned() {
        let request_id = "0123456789abcdef0123456789abcdef".to_string();
        let request = Request::CreateWindow {
            request_id: request_id.clone(),
            source_window: 7,
        };
        sends_unchanged(request);

        let response = ServerFrame::new(Response::WindowCreated {
            process_id: 42,
            request_id,
            window_id: 9,
        });
        round_trip(&response);

        assert!(
            Request::CreateWindow {
                request_id: "ABC".into(),
                source_window: 7,
            }
            .validate()
            .is_err()
        );
        assert!(
            Request::CreateWindow {
                request_id: "0".repeat(32),
                source_window: 0,
            }
            .validate()
            .is_err()
        );
    }

    #[test]
    fn create_tab_requests_and_responses_are_explicitly_versioned() {
        let request_id = "fedcba9876543210fedcba9876543210".to_string();
        let request = Request::CreateTab {
            request_id: request_id.clone(),
            window_id: 7,
        };
        sends_unchanged(request);

        let response = ServerFrame::new(Response::TabCreated {
            process_id: 42,
            request_id,
            window_id: 7,
            tab_id: 3,
        });
        round_trip(&response);

        assert!(
            Request::CreateTab {
                request_id: "0".repeat(32),
                window_id: 0,
            }
            .validate()
            .is_err()
        );
    }

    #[test]
    fn background_launch_mode_is_a_required_strict_wire_boolean() {
        let request = Request::LaunchTab {
            background: true,
            request_id: "a".repeat(32),
            window_id: 7,
            program: "/bin/true".into(),
            args: Vec::new(),
            working_directory: None,
            title: None,
            environment: Vec::new(),
            environment_policy: Default::default(),
        };
        let value = serde_json::to_value(&request).unwrap();
        for background in [true, false] {
            let mut valid = value.clone();
            valid["background"] = serde_json::json!(background);
            let decoded: Request = serde_json::from_value(valid).unwrap();
            assert!(
                matches!(decoded, Request::LaunchTab { background: actual, .. } if actual == background)
            );
            decoded.validate().unwrap();
        }
        for invalid in [
            serde_json::json!("true"),
            serde_json::json!(1),
            serde_json::Value::Null,
        ] {
            let mut invalid_request = value.clone();
            invalid_request["background"] = invalid;
            assert!(serde_json::from_value::<Request>(invalid_request).is_err());
        }
        let mut absent = value;
        absent.as_object_mut().unwrap().remove("background");
        assert!(serde_json::from_value::<Request>(absent).is_err());
    }

    #[test]
    fn launch_tab_requests_preserve_exact_argv_and_reject_unsafe_or_oversized_input() {
        let request = Request::LaunchTab {
            background: false,
            request_id: "0123456789abcdef0123456789abcdef".into(),
            window_id: 7,
            program: "/bin/printf".into(),
            args: vec![
                "".into(),
                "two words".into(),
                "$HOME;🙂".into(),
                "--json".into(),
            ],
            working_directory: Some("/tmp/launch space".into()),
            title: Some("Exact launch 🙂".into()),
            environment_policy: Default::default(),
            environment: vec![EnvironmentVariable {
                name: "EXACT_VALUE".into(),
                value: "= two words $HOME;🙂".into(),
            }],
        };
        sends_unchanged(request.clone());
        assert_eq!(
            request.request_id(),
            Some("0123456789abcdef0123456789abcdef")
        );

        let invalid = |program: String,
                       args: Vec<String>,
                       working_directory: Option<String>,
                       title: Option<String>| Request::LaunchTab {
            background: false,
            request_id: "0123456789abcdef0123456789abcdef".into(),
            window_id: 7,
            program,
            args,
            working_directory,
            title,
            environment_policy: Default::default(),
            environment: Vec::new(),
        };
        assert!(invalid("".into(), vec![], None, None).validate().is_err());
        assert!(
            invalid("bad\u{1b}".into(), vec![], None, None)
                .validate()
                .is_err()
        );
        assert!(
            invalid("ok".into(), vec!["bad\0arg".into()], None, None)
                .validate()
                .is_err()
        );
        assert!(
            invalid(
                "ok".into(),
                vec!["x".into(); MAX_CONTROL_LAUNCH_ARGS + 1],
                None,
                None,
            )
            .validate()
            .is_err()
        );
        assert!(
            invalid(
                "x".repeat(MAX_CONTROL_LAUNCH_ARGV_BYTES + 1),
                vec![],
                None,
                None,
            )
            .validate()
            .is_err()
        );
        assert!(
            invalid("ok".into(), vec![], Some("relative".into()), None)
                .validate()
                .is_err()
        );
        assert!(
            invalid(
                "ok".into(),
                vec![],
                Some("/tmp/\u{202e}hidden".into()),
                None,
            )
            .validate()
            .is_err()
        );
        assert!(
            invalid("ok".into(), vec![], None, Some("line\nbreak".into()))
                .validate()
                .is_err()
        );
        assert!(
            invalid(
                "ok".into(),
                vec![],
                None,
                Some("x".repeat(MAX_CONTROL_LAUNCH_TITLE_BYTES + 1)),
            )
            .validate()
            .is_err()
        );
    }

    #[test]
    fn launch_window_requests_and_responses_preserve_exact_stable_ids() {
        let request_id = "abcdef0123456789abcdef0123456789".to_string();
        let request = Request::LaunchWindow {
            request_id: request_id.clone(),
            source_window: 7,
            program: "/bin/printf".into(),
            args: vec!["".into(), "two words".into(), "$HOME;🙂".into()],
            working_directory: Some("/tmp/launch space".into()),
            title: Some("Exact window 🙂".into()),
            environment_policy: Default::default(),
            environment: vec![EnvironmentVariable {
                name: "EMPTY_VALUE".into(),
                value: String::new(),
            }],
        };
        sends_unchanged(request.clone());
        assert_eq!(request.request_id(), Some(request_id.as_str()));

        let response = ServerFrame::new(Response::WindowLaunched {
            process_id: 42,
            request_id,
            window_id: 9,
            tab_id: 0,
        });
        round_trip(&response);

        assert!(
            Request::LaunchWindow {
                request_id: "0".repeat(32),
                source_window: 0,
                program: "/bin/true".into(),
                args: Vec::new(),
                working_directory: None,
                title: None,
                environment_policy: Default::default(),
                environment: Vec::new(),
            }
            .validate()
            .is_err()
        );
        assert!(
            Request::LaunchWindow {
                request_id: "0".repeat(32),
                source_window: 7,
                program: "/bin/true".into(),
                args: vec!["x".into(); MAX_CONTROL_LAUNCH_ARGS + 1],
                working_directory: None,
                title: None,
                environment_policy: Default::default(),
                environment: Vec::new(),
            }
            .validate()
            .is_err()
        );
    }

    #[test]
    fn launch_environment_is_bounded_unique_portable_and_identity_safe() {
        let request = |environment: Vec<EnvironmentVariable>| Request::LaunchTab {
            background: false,
            request_id: "0123456789abcdef0123456789abcdef".into(),
            window_id: 7,
            program: "/bin/true".into(),
            args: Vec::new(),
            working_directory: None,
            title: None,
            environment,
            environment_policy: Default::default(),
        };
        request(vec![
            EnvironmentVariable {
                name: "EMPTY_VALUE".into(),
                value: String::new(),
            },
            EnvironmentVariable {
                name: "EXACT_VALUE".into(),
                value: "= two words\n$HOME;🙂".into(),
            },
        ])
        .validate()
        .unwrap();

        for name in [
            "PWD",
            "TERM",
            "COLORTERM",
            "TERMINFO",
            "TERMINFO_DIRS",
            "TERM_PROGRAM",
            "TERM_PROGRAM_VERSION",
            "TERMERIC_TERMINAL_ID",
            "",
            "9INVALID",
            "INVALID-NAME",
            "UNICODE_🙂",
        ] {
            let error = request(vec![EnvironmentVariable {
                name: name.into(),
                value: "private-value".into(),
            }])
            .validate()
            .unwrap_err();
            assert!(error.contains("invalid or reserved"), "{error}");
            if !name.is_empty() {
                assert!(!error.contains(name));
            }
            assert!(!error.contains("private-value"));
        }
        assert!(
            request(vec![EnvironmentVariable {
                name: "X".repeat(MAX_CONTROL_LAUNCH_ENVIRONMENT_NAME_BYTES + 1),
                value: String::new(),
            }])
            .validate()
            .is_err()
        );
        assert!(
            request(vec![
                EnvironmentVariable {
                    name: "DUPLICATE".into(),
                    value: "first".into(),
                },
                EnvironmentVariable {
                    name: "DUPLICATE".into(),
                    value: "second".into(),
                },
            ])
            .validate()
            .unwrap_err()
            .contains("duplicate")
        );
        assert!(
            request(vec![EnvironmentVariable {
                name: "NUL_VALUE".into(),
                value: "private\0value".into(),
            }])
            .validate()
            .unwrap_err()
            .contains("NUL-free")
        );
        assert!(
            request(vec![EnvironmentVariable {
                name: "LARGE_VALUE".into(),
                value: "x".repeat(MAX_CONTROL_LAUNCH_ENVIRONMENT_VALUE_BYTES + 1),
            }])
            .validate()
            .is_err()
        );
        assert!(
            request(
                (0..=MAX_CONTROL_LAUNCH_ENVIRONMENT_ENTRIES)
                    .map(|index| EnvironmentVariable {
                        name: format!("ENTRY_{index}"),
                        value: String::new(),
                    })
                    .collect()
            )
            .validate()
            .is_err()
        );
        assert!(
            request(
                (0..8)
                    .map(|index| EnvironmentVariable {
                        name: format!("ENTRY_{index}"),
                        value: "x".repeat(MAX_CONTROL_LAUNCH_ENVIRONMENT_VALUE_BYTES),
                    })
                    .collect()
            )
            .validate()
            .unwrap_err()
            .contains("65536")
        );
    }

    #[test]
    fn launch_environment_canonicalization_is_name_ordered_and_value_preserving() {
        let mut request = Request::LaunchWindow {
            request_id: "0123456789abcdef0123456789abcdef".into(),
            source_window: 7,
            program: "/bin/true".into(),
            args: Vec::new(),
            working_directory: None,
            title: None,
            environment_policy: Default::default(),
            environment: vec![
                EnvironmentVariable {
                    name: "Z_VALUE".into(),
                    value: "last".into(),
                },
                EnvironmentVariable {
                    name: "A_VALUE".into(),
                    value: "= first $HOME;🙂".into(),
                },
            ],
        };
        request.validate().unwrap();
        request.canonicalize();

        let Request::LaunchWindow { environment, .. } = request else {
            unreachable!();
        };
        assert_eq!(
            environment,
            vec![
                EnvironmentVariable {
                    name: "A_VALUE".into(),
                    value: "= first $HOME;🙂".into(),
                },
                EnvironmentVariable {
                    name: "Z_VALUE".into(),
                    value: "last".into(),
                },
            ]
        );
    }

    #[test]
    fn split_pane_requests_and_responses_are_explicitly_versioned() {
        let request_id = "1234567890abcdef1234567890abcdef".to_string();
        let request = Request::SplitPane {
            request_id: request_id.clone(),
            window_id: 7,
            tab_id: 3,
            pane_id: 0,
            direction: SplitDirection::Vertical,
        };
        sends_unchanged(request);

        let response = ServerFrame::new(Response::PaneCreated {
            process_id: 42,
            request_id,
            window_id: 7,
            tab_id: 3,
            pane_id: 1,
        });
        round_trip(&response);

        assert!(
            Request::SplitPane {
                request_id: "0".repeat(32),
                window_id: 0,
                tab_id: 0,
                pane_id: 0,
                direction: SplitDirection::Horizontal,
            }
            .validate()
            .is_err()
        );
    }

    #[test]
    fn focus_requests_and_responses_are_explicit_and_id_targeted() {
        let request_id = "abcdef0123456789abcdef0123456789".to_string();
        for request in [
            Request::FocusWindow {
                request_id: request_id.clone(),
                window_id: 7,
            },
            Request::SetQuickAccessVisibility {
                request_id: request_id.clone(),
                window_id: 7,
                visible: false,
            },
            Request::SelectTab {
                request_id: request_id.clone(),
                window_id: 7,
                tab_id: 3,
            },
            Request::FocusPane {
                request_id: request_id.clone(),
                window_id: 7,
                tab_id: 3,
                pane_id: 1,
            },
        ] {
            sends_unchanged(request);
        }

        for response in [
            ServerFrame::new(Response::WindowFocusRequested {
                process_id: 42,
                request_id: request_id.clone(),
                window_id: 7,
                already_focused: false,
            }),
            ServerFrame::new(Response::QuickAccessChanged {
                process_id: 42,
                request_id: request_id.clone(),
                window_id: 7,
                visible: false,
            }),
            ServerFrame::new(Response::TabSelected {
                process_id: 42,
                request_id: request_id.clone(),
                window_id: 7,
                tab_id: 3,
                changed: true,
            }),
            ServerFrame::new(Response::PaneFocused {
                process_id: 42,
                request_id: request_id.clone(),
                window_id: 7,
                tab_id: 3,
                pane_id: 1,
                tab_changed: true,
                pane_changed: true,
            }),
        ] {
            round_trip(&response);
        }
    }

    #[test]
    fn close_requests_distinguish_scheduled_and_completed_teardown() {
        let request_id = "9876543210abcdef9876543210abcdef".to_string();
        for request in [
            Request::CloseWindow {
                request_id: request_id.clone(),
                window_id: 7,
            },
            Request::CloseTab {
                request_id: request_id.clone(),
                window_id: 7,
                tab_id: 3,
            },
            Request::ClosePane {
                request_id: request_id.clone(),
                window_id: 7,
                tab_id: 3,
                pane_id: 1,
            },
        ] {
            sends_unchanged(request);
        }

        for response in [
            ServerFrame::new(Response::WindowCloseRequested {
                process_id: 42,
                request_id: request_id.clone(),
                window_id: 7,
            }),
            ServerFrame::new(Response::TabCloseRequested {
                process_id: 42,
                request_id: request_id.clone(),
                window_id: 7,
                tab_id: 3,
            }),
            ServerFrame::new(Response::PaneClosed {
                process_id: 42,
                request_id: request_id.clone(),
                window_id: 7,
                tab_id: 3,
                pane_id: 1,
                remaining_panes: 1,
                focused_pane_id: 0,
            }),
        ] {
            round_trip(&response);
        }
    }

    #[test]
    fn tab_move_wire_shape_returns_destination_local_identity() {
        let request_id = "0123456789abcdef0123456789abcdef".to_string();
        let request = Request::MoveTab {
            request_id: request_id.clone(),
            source_window_id: 7,
            tab_id: 3,
            destination_window_id: 9,
            destination_index: 2,
        };
        sends_unchanged(request);

        let response = ServerFrame::new(Response::TabMoved {
            process_id: 42,
            request_id,
            source_window_id: 7,
            source_tab_id: 3,
            destination_window_id: 9,
            destination_tab_id: 11,
            destination_index: 2,
            source_window_closing: false,
            changed: true,
        });
        round_trip(&response);

        assert!(
            Request::MoveTab {
                request_id: "0".repeat(32),
                source_window_id: 0,
                tab_id: 3,
                destination_window_id: 9,
                destination_index: 0,
            }
            .validate()
            .is_err()
        );
    }

    #[test]
    fn tab_move_request_accepts_exactly_the_indexes_its_reply_can_report() {
        let move_to = |destination_index: usize| Request::MoveTab {
            request_id: "0".repeat(32),
            source_window_id: 7,
            tab_id: 3,
            destination_window_id: 9,
            destination_index: u16::try_from(destination_index).unwrap(),
        };
        let moved_to = |destination_index: usize| Response::TabMoved {
            process_id: 42,
            request_id: "0".repeat(32),
            source_window_id: 7,
            source_tab_id: 3,
            destination_window_id: 9,
            destination_tab_id: 11,
            destination_index: u16::try_from(destination_index).unwrap(),
            source_window_closing: false,
            changed: true,
        };

        let last = MAX_TABS_PER_WINDOW - 1;
        assert!(move_to(last).validate().is_ok());
        assert!(moved_to(last).validate().is_ok());
        assert!(move_to(MAX_TABS_PER_WINDOW).validate().is_err());
        assert!(moved_to(MAX_TABS_PER_WINDOW).validate().is_err());
    }

    #[test]
    fn pane_move_wire_shape_names_both_complete_paths() {
        let request_id = "abcdefabcdefabcdefabcdefabcdefab".to_string();
        let request = Request::MovePane {
            request_id: request_id.clone(),
            source_window_id: 7,
            source_tab_id: 3,
            pane_id: 1,
            destination_window_id: 9,
            destination_tab_id: 4,
            target_pane_id: 2,
            direction: SplitDirection::Horizontal,
        };
        sends_unchanged(request);

        let response = ServerFrame::new(Response::PaneMoved {
            process_id: 42,
            request_id,
            source_window_id: 7,
            source_tab_id: 3,
            source_pane_id: 1,
            destination_window_id: 9,
            destination_tab_id: 4,
            destination_pane_id: 5,
            target_pane_id: 2,
        });
        round_trip(&response);
    }

    #[test]
    fn terminal_text_wire_shape_is_bounded_and_read_only() {
        let request = Request::ReadText {
            window_id: 7,
            tab_id: 3,
            pane_id: 1,
            history_lines: 40,
            max_bytes: 4096,
        };
        request.validate().unwrap();
        assert_eq!(request.request_id(), None);
        sends_unchanged(request);

        let response = ServerFrame::new(Response::Text {
            process_id: 42,
            window_id: 7,
            tab_id: 3,
            pane_id: 1,
            screen: TextScreen::Primary,
            history_lines: 12,
            text: "recent output".into(),
            truncated: false,
        });
        round_trip(&response);

        for invalid in [
            Request::ReadText {
                window_id: 0,
                tab_id: 3,
                pane_id: 1,
                history_lines: 0,
                max_bytes: 1,
            },
            Request::ReadText {
                window_id: 7,
                tab_id: 3,
                pane_id: 1,
                history_lines: MAX_TEXT_HISTORY_LINES + 1,
                max_bytes: 1,
            },
            Request::ReadText {
                window_id: 7,
                tab_id: 3,
                pane_id: 1,
                history_lines: 0,
                max_bytes: 0,
            },
        ] {
            assert!(invalid.validate().is_err());
        }
    }

    #[test]
    fn terminal_input_wire_shape_is_utf8_typed_and_replay_identified() {
        let request_id = "33333333333333333333333333333333".to_string();
        let request = Request::SendText {
            request_id: request_id.clone(),
            window_id: 7,
            tab_id: 3,
            pane_id: 1,
            text: "cargo test\nnext".into(),
            paste: true,
            enter: true,
        };
        request.validate().unwrap();
        assert_eq!(request.request_id(), Some(request_id.as_str()));
        sends_unchanged(request);

        let response = ServerFrame::new(Response::InputAccepted {
            process_id: 42,
            request_id,
            window_id: 7,
            tab_id: 3,
            pane_id: 1,
            input_bytes: 16,
            wire_bytes: 28,
            paste: true,
            enter: true,
            disposition: InputDisposition::PendingConfirmation,
            confirmation_id: Some(9),
        });
        round_trip(&response);

        for text in ["\u{1b}[31m", "bad\rreturn", "\0"] {
            assert!(
                Request::SendText {
                    request_id: "4".repeat(32),
                    window_id: 7,
                    tab_id: 3,
                    pane_id: 1,
                    text: text.into(),
                    paste: false,
                    enter: false,
                }
                .validate()
                .is_err()
            );
        }
        assert!(
            Request::SendText {
                request_id: "4".repeat(32),
                window_id: 7,
                tab_id: 3,
                pane_id: 1,
                text: "x".repeat(MAX_CONTROL_INPUT_BYTES + 1),
                paste: false,
                enter: false,
            }
            .validate()
            .is_err()
        );
    }

    #[test]
    fn resize_wire_shapes_distinguish_native_request_from_completed_divider_change() {
        let request_id = "22222222222222222222222222222222".to_string();
        for request in [
            Request::ResizeWindow {
                request_id: request_id.clone(),
                window_id: 7,
                width: 900,
                height: 600,
            },
            Request::ResizeDivider {
                request_id: request_id.clone(),
                window_id: 7,
                tab_id: 3,
                divider_id: 11,
                ratio_millis: 650,
            },
        ] {
            sends_unchanged(request);
        }

        for response in [
            ServerFrame::new(Response::WindowResizeRequested {
                process_id: 42,
                request_id: request_id.clone(),
                window_id: 7,
                requested_width: 900,
                requested_height: 600,
                immediate_width: None,
                immediate_height: None,
            }),
            ServerFrame::new(Response::DividerResized {
                process_id: 42,
                request_id: request_id.clone(),
                window_id: 7,
                tab_id: 3,
                divider_id: 11,
                previous_ratio_millis: 500,
                ratio_millis: 650,
                changed: true,
            }),
        ] {
            round_trip(&response);
        }

        assert!(
            Request::ResizeWindow {
                request_id: request_id.clone(),
                window_id: 7,
                width: 159,
                height: 600,
            }
            .validate()
            .is_err()
        );
        assert!(
            Request::ResizeDivider {
                request_id,
                window_id: 7,
                tab_id: 3,
                divider_id: 11,
                ratio_millis: 901,
            }
            .validate()
            .is_err()
        );
    }

    #[test]
    fn decoding_rejects_unknown_and_unbounded_frames() {
        let payload = br#"{"version":1,"token":"x","request":"inventory","extra":true}"#;
        let mut wire = (payload.len() as u32).to_be_bytes().to_vec();
        wire.extend_from_slice(payload);
        assert!(read_frame::<ClientFrame>(&mut wire.as_slice()).is_err());

        let oversized_length = ((MAX_FRAME_BYTES + 1) as u32).to_be_bytes();
        let mut oversized = oversized_length.as_slice();
        assert!(read_frame::<ClientFrame>(&mut oversized).is_err());
        let empty_length = 0u32.to_be_bytes();
        let mut empty = empty_length.as_slice();
        assert!(read_frame::<ClientFrame>(&mut empty).is_err());
    }

    #[test]
    fn process_wide_snapshot_caps_fit_the_wire_budget() {
        let mut windows = Vec::new();
        let mut tab_id = 0usize;
        let mut metadata_left = MAX_METADATA_BYTES;
        for window_id in 0..MAX_WINDOWS {
            let mut tabs = Vec::new();
            for _ in 0..(MAX_TOTAL_TABS / MAX_WINDOWS) {
                let title_bytes = metadata_left.min(128);
                metadata_left -= title_bytes;
                let panes_per_tab = MAX_TOTAL_PANES / MAX_TOTAL_TABS;
                let panes = (0..panes_per_tab)
                    .map(|pane_id| PaneInventory {
                        id: u32::MAX - pane_id as u32,
                        title: None,
                        focused: pane_id == 0,
                        closed: false,
                        cols: u16::MAX,
                        rows: u16::MAX,
                    })
                    .collect();
                let dividers = (0..panes_per_tab.saturating_sub(1))
                    .map(|divider_id| DividerInventory {
                        id: divider_id as u64,
                        kind: DividerKind::LiveDomain,
                        direction: SplitDirection::Vertical,
                        ratio_millis: 500,
                    })
                    .collect();
                tabs.push(TabInventory {
                    id: u64::MAX - tab_id as u64,
                    title: "x".repeat(title_bytes),
                    kind: TabKind::LiveDomain,
                    active: tab_id == 0,
                    pinned: false,
                    activity: true,
                    closed: false,
                    domain_id: None,
                    panes,
                    dividers,
                    truncated: false,
                });
                tab_id += 1;
            }
            windows.push(WindowInventory {
                id: u64::MAX - window_id as u64,
                focused: window_id == 0,
                visible: true,
                quick_access: false,
                workspace_id: None,
                pixel_width: MAX_WINDOW_DIMENSION,
                pixel_height: MAX_WINDOW_DIMENSION,
                tabs,
                truncated: false,
            });
        }
        assert_eq!(tab_id, MAX_TOTAL_TABS);
        let frame = ServerFrame::new(Response::Inventory(ProcessInventory {
            process_id: i32::MAX as u32,
            windows,
            truncated: false,
        }));
        let mut wire = Vec::new();
        write_frame(&mut wire, &frame).unwrap();
        assert!(wire.len() <= MAX_FRAME_BYTES + 4);
        frame.response.validate().unwrap();
    }

    #[test]
    fn semantic_validation_rejects_duplicate_ids_controls_and_aggregate_overflow() {
        let mut inventory = ProcessInventory {
            process_id: 42,
            windows: vec![WindowInventory {
                id: 1,
                focused: true,
                visible: true,
                quick_access: false,
                workspace_id: None,
                pixel_width: 800,
                pixel_height: 600,
                tabs: vec![TabInventory {
                    id: 1,
                    title: "safe".into(),
                    kind: TabKind::Shell,
                    active: true,
                    pinned: false,
                    activity: false,
                    closed: false,
                    domain_id: None,
                    panes: vec![PaneInventory {
                        id: 0,
                        title: None,
                        focused: true,
                        closed: false,
                        cols: 80,
                        rows: 24,
                    }],
                    dividers: Vec::new(),
                    truncated: false,
                }],
                truncated: false,
            }],
            truncated: false,
        };
        inventory.validate().unwrap();
        inventory.windows[0].tabs[0]
            .dividers
            .push(DividerInventory {
                id: 1,
                kind: DividerKind::Local,
                direction: SplitDirection::Vertical,
                ratio_millis: 99,
            });
        assert!(inventory.validate().is_err());
        inventory.windows[0].tabs[0].dividers.clear();
        inventory.windows[0].tabs[0].title = "unsafe\u{1b}".into();
        assert!(inventory.validate().is_err());
        inventory.windows[0].tabs[0].title = "safe".into();
        inventory.windows.push(inventory.windows[0].clone());
        assert!(inventory.validate().is_err());

        let template = inventory.windows[0].tabs[0].clone();
        inventory.windows.truncate(1);
        inventory.windows.push(WindowInventory {
            id: 2,
            focused: false,
            visible: true,
            quick_access: false,
            workspace_id: None,
            pixel_width: 800,
            pixel_height: 600,
            tabs: (0..MAX_TOTAL_TABS)
                .map(|index| TabInventory {
                    id: index as u64 + 2,
                    panes: Vec::new(),
                    ..template.clone()
                })
                .collect(),
            truncated: false,
        });
        assert!(inventory.validate().is_err());
    }

    #[test]
    fn joining_launches_are_validated_before_any_window_opens() {
        let tab = |working_directory: Option<&str>, program: Option<&str>, args: &[&str]| JoinTab {
            working_directory: working_directory.map(str::to_owned),
            title: None,
            program: program.map(str::to_owned),
            args: args.iter().map(|arg| (*arg).to_owned()).collect(),
        };
        let join = |display: &str, token: Option<&str>, tabs: Vec<JoinTab>| Request::JoinLaunch {
            request_id: "0123456789abcdef0123456789abcdef".into(),
            display: display.into(),
            activation_token: token.map(str::to_owned),
            build: "/usr/bin/termeric@1790000000.5".into(),
            environment: "0123456789abcdef".into(),
            tabs,
        };
        assert!(
            join(
                "wayland:wayland-0",
                Some("abc"),
                vec![tab(Some("/tmp"), None, &[])]
            )
            .validate()
            .is_ok()
        );
        assert!(
            join("x11::1", None, vec![tab(None, Some("make"), &["-j8"])])
                .validate()
                .is_ok()
        );
        let mut no_build = join("wayland:w", None, vec![tab(None, None, &[])]);
        if let Request::JoinLaunch { build, .. } = &mut no_build {
            build.clear();
        }
        assert!(no_build.validate().is_err());
        for environment in [
            "",
            "0123456789ABCDEF",
            "0123456789abcde",
            "0123456789abcdef0",
        ] {
            let mut refused = join("wayland:w", None, vec![tab(None, None, &[])]);
            if let Request::JoinLaunch {
                environment: identity,
                ..
            } = &mut refused
            {
                *identity = environment.into();
            }
            assert!(refused.validate().is_err(), "{environment:?}");
        }
        for refused in [
            join("", None, vec![tab(None, None, &[])]),
            join("wayland:\u{202e}", None, vec![tab(None, None, &[])]),
            join("wayland:w", Some(""), vec![tab(None, None, &[])]),
            join("wayland:w", None, Vec::new()),
            join(
                "wayland:w",
                None,
                vec![tab(None, None, &[]); MAX_JOIN_TABS + 1],
            ),
            join("wayland:w", None, vec![tab(Some("relative"), None, &[])]),
            join("wayland:w", None, vec![tab(None, None, &["stray"])]),
            join("wayland:w", None, vec![tab(None, Some(""), &[])]),
        ] {
            assert!(refused.validate().is_err(), "{refused:?}");
        }
    }
}
