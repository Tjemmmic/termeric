//! Bounded out-of-process execution for declarative plugin event handlers.
//!
//! Each plugin's handler is one long-lived process, shared by every window,
//! that reads one schema-v1 JSON request per line and answers each with one
//! JSON line echoing the request's `request_id`. [`StructuredPluginHost`]
//! runs each plugin on its own lane, so one plugin's slow answers never
//! delay another's. Each response is checked against a
//! [`HandlerAuthority`]: the plugin's granted capabilities plus where the
//! response sits in its bounded follow-up chain.

use super::permission::{Capability, PermissionScope, PluginPermissions};
use crate::agents::AgentConfig;
use crate::line_reader::read_bounded_line;
use crate::panes::PaneLayoutPreset;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};
use std::io::{BufRead, BufReader, Read, Write};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError, SyncSender, TrySendError, sync_channel};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use std::os::unix::process::CommandExt;

/// Requests queued on one plugin's lane.
const QUEUE_CAPACITY: usize = 32;
/// Plugin lanes, each with at most one handler process, in the whole process.
const MAX_PERSISTENT_HANDLERS: usize = 8;
/// One request's envelope before its settings and `request_id`: an event,
/// or the result of a query or action.
pub(crate) const MAX_PERSISTENT_SOURCE_EVENT_BYTES: usize =
    16 * 1024 - PERSISTENT_PROTOCOL_RESERVE_BYTES;
/// The plugin's settings document, bounded where settings are validated.
/// Every request of a conversation repeats the values, so they have their
/// own share of the request rather than competing with results.
pub(crate) const MAX_REQUEST_SETTINGS_BYTES: usize = 8 * 1024;
pub(crate) const MAX_STRUCTURED_EVENT_BYTES: usize = 16 * 1024 + MAX_REQUEST_SETTINGS_BYTES;
pub(crate) const MAX_TERMINAL_QUERY_BYTES: u32 = 8 * 1024;
const MAX_TERMINAL_INPUT_BYTES: usize = 8 * 1024;
const PERSISTENT_PROTOCOL_RESERVE_BYTES: usize = 64;
const HANDLER_TIMEOUT: Duration = Duration::from_secs(2);
const MAX_OUTPUT_BYTES: u64 = 64 * 1024;
const MAX_ERROR_BYTES: u64 = 4 * 1024;
const MAX_ACTIONS: usize = 8;
const MAX_MESSAGE_BYTES: usize = 512;
const MAX_TITLE_BYTES: usize = 128;
const MAX_NOTICE_LINES: usize = 16;
const MAX_LINE_BYTES: usize = 512;
const MAX_QUERY_ID_BYTES: usize = 64;
const MAX_ACTION_ID_BYTES: usize = 64;
/// The handler protocol version.
pub(crate) const SCHEMA_VERSION: u32 = 1;
const WORKER_POLL_INTERVAL: Duration = Duration::from_millis(10);
const PERSISTENT_REAP_INTERVAL: Duration = Duration::from_millis(100);

#[derive(Debug, Clone)]
pub struct StructuredHandlerInvocation {
    pub plugin_id: String,
    pub program: String,
    pub args: Vec<String>,
    pub event_json: String,
    /// The plugin's effective settings, when its manifest declares any. They
    /// come from the event that started the conversation and are sent with
    /// that event and with every result that follows it.
    pub settings: Option<serde_json::Value>,
    pub authority: HandlerAuthority,
}

/// Where a handler response sits in its follow-up chain. An event may start
/// one query or one host action; a workspace query result may perform one
/// more host operation; any other result ends the chain.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FollowUpStage {
    Event,
    AfterWorkspaceQuery,
    Ended,
}

/// What one handler response may do: the plugin's granted capabilities and
/// the response's position in its bounded follow-up chain.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HandlerAuthority {
    grants: PluginPermissions,
    stage: FollowUpStage,
    /// Whether the chain started from an explicitly invoked plugin command,
    /// the only origin allowed to close the plugin's own window at window
    /// scope.
    command_invoked: bool,
}

impl HandlerAuthority {
    /// Authority for a freshly dispatched event.
    pub fn for_event(grants: PluginPermissions, command_invoked: bool) -> Self {
        Self {
            grants,
            stage: FollowUpStage::Event,
            command_invoked,
        }
    }

    pub fn grants(&self) -> &PluginPermissions {
        &self.grants
    }

    /// Authority for the result of `operation`: a workspace query result may
    /// still perform one host operation; every other result ends the chain.
    pub fn after(&self, operation: &StructuredResponseAction) -> Self {
        let stage = match (self.stage, operation.operation_kind()) {
            (FollowUpStage::Event, OperationKind::WorkspaceQuery) => {
                FollowUpStage::AfterWorkspaceQuery
            }
            _ => FollowUpStage::Ended,
        };
        Self {
            stage,
            ..self.clone()
        }
    }

    /// Check one complete handler response: every action needs its
    /// capability, host operations must fit the follow-up chain, and a
    /// response carries at most one host operation.
    pub fn check(&self, actions: &[StructuredResponseAction]) -> Result<(), String> {
        if actions.len() > MAX_ACTIONS {
            return Err(format!(
                "handler response contains more than {MAX_ACTIONS} actions"
            ));
        }
        let mut host_operations = 0;
        for action in actions {
            let name = action.name();
            let capability = action.required_capability();
            let Some(scope) = self.grants.scope(capability) else {
                return Err(format!(
                    "handler requested {name} without the {capability} permission"
                ));
            };
            match (action.operation_kind(), self.stage) {
                (OperationKind::Ui, _) => {}
                (_, FollowUpStage::Ended) => {
                    return Err(format!(
                        "handler requested {name}, but this response cannot start host operations; action results and terminal query results end the chain"
                    ));
                }
                (OperationKind::WorkspaceQuery, FollowUpStage::AfterWorkspaceQuery) => {
                    return Err(format!(
                        "handler requested {name} from a workspace query result, which may perform only one host action or terminal query"
                    ));
                }
                _ => host_operations += 1,
            }
            if matches!(action, StructuredResponseAction::CloseWindow { .. })
                && scope == PermissionScope::Window
                && !self.command_invoked
            {
                return Err(
                    "handler requested close-window outside an invoked plugin command; window-close for the plugin's own window is command-scoped"
                        .into(),
                );
            }
            action.validate_fields()?;
        }
        if host_operations > 1 {
            return Err("handler response contains more than one host operation".into());
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OperationKind {
    Ui,
    WorkspaceQuery,
    TerminalQuery,
    Action,
}

/// One action in a handler response. Actions that target a window, tab, or
/// pane accept an optional window ID that defaults to the plugin's
/// originating window; any other window requires the capability's
/// all-windows scope, which the host enforces where the action executes.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(tag = "type", rename_all = "kebab-case", deny_unknown_fields)]
pub enum StructuredResponseAction {
    ShowMessage {
        message: String,
    },
    ShowNotice {
        title: String,
        lines: Vec<String>,
    },
    QueryWorkspace {
        query_id: String,
    },
    QueryTerminal {
        query_id: String,
        #[serde(default)]
        window_id: Option<u64>,
        tab_id: u64,
        pane_id: u32,
        history_lines: u16,
        max_bytes: u32,
    },
    SelectTab {
        action_id: String,
        #[serde(default)]
        window_id: Option<u64>,
        tab_id: u64,
    },
    FocusPane {
        action_id: String,
        #[serde(default)]
        window_id: Option<u64>,
        tab_id: u64,
        pane_id: u32,
    },
    SendText {
        action_id: String,
        #[serde(default)]
        window_id: Option<u64>,
        tab_id: u64,
        pane_id: u32,
        text: String,
        paste: bool,
        enter: bool,
    },
    CreateTab {
        action_id: String,
        #[serde(default)]
        window_id: Option<u64>,
    },
    CloseTab {
        action_id: String,
        #[serde(default)]
        window_id: Option<u64>,
        tab_id: u64,
    },
    MoveTab {
        action_id: String,
        #[serde(default)]
        source_window_id: Option<u64>,
        tab_id: u64,
        #[serde(default)]
        destination_window_id: Option<u64>,
        destination_index: u16,
    },
    CreatePane {
        action_id: String,
        #[serde(default)]
        window_id: Option<u64>,
        tab_id: u64,
        pane_id: u32,
        direction: crate::panes::SplitDirection,
    },
    ClosePane {
        action_id: String,
        #[serde(default)]
        window_id: Option<u64>,
        tab_id: u64,
        pane_id: u32,
    },
    MovePane {
        action_id: String,
        #[serde(default)]
        source_window_id: Option<u64>,
        source_tab_id: u64,
        pane_id: u32,
        #[serde(default)]
        destination_window_id: Option<u64>,
        destination_tab_id: u64,
        target_pane_id: u32,
        direction: crate::panes::SplitDirection,
    },
    SwapPanes {
        action_id: String,
        #[serde(default)]
        window_id: Option<u64>,
        tab_id: u64,
        first_pane_id: u32,
        second_pane_id: u32,
    },
    BalancePanes {
        action_id: String,
        #[serde(default)]
        window_id: Option<u64>,
        tab_id: u64,
    },
    ApplyPaneLayout {
        action_id: String,
        #[serde(default)]
        window_id: Option<u64>,
        tab_id: u64,
        preset: PaneLayoutPreset,
    },
    ResizeDivider {
        action_id: String,
        #[serde(default)]
        window_id: Option<u64>,
        tab_id: u64,
        divider_id: u64,
        ratio_millis: u16,
    },
    CloseWindow {
        action_id: String,
        #[serde(default)]
        window_id: Option<u64>,
    },
    FocusWindow {
        action_id: String,
        window_id: u64,
    },
}

impl StructuredResponseAction {
    /// The action's wire `type`.
    pub fn name(&self) -> &'static str {
        match self {
            Self::ShowMessage { .. } => "show-message",
            Self::ShowNotice { .. } => "show-notice",
            Self::QueryWorkspace { .. } => "query-workspace",
            Self::QueryTerminal { .. } => "query-terminal",
            Self::SelectTab { .. } => "select-tab",
            Self::FocusPane { .. } => "focus-pane",
            Self::SendText { .. } => "send-text",
            Self::CreateTab { .. } => "create-tab",
            Self::CloseTab { .. } => "close-tab",
            Self::MoveTab { .. } => "move-tab",
            Self::CreatePane { .. } => "create-pane",
            Self::ClosePane { .. } => "close-pane",
            Self::MovePane { .. } => "move-pane",
            Self::SwapPanes { .. } => "swap-panes",
            Self::BalancePanes { .. } => "balance-panes",
            Self::ApplyPaneLayout { .. } => "apply-pane-layout",
            Self::ResizeDivider { .. } => "resize-divider",
            Self::CloseWindow { .. } => "close-window",
            Self::FocusWindow { .. } => "focus-window",
        }
    }

    /// The capability a plugin must hold for this action.
    pub fn required_capability(&self) -> Capability {
        match self {
            Self::ShowMessage { .. } | Self::ShowNotice { .. } => Capability::UiActions,
            Self::QueryWorkspace { .. } => Capability::WorkspaceQuery,
            Self::QueryTerminal { .. } => Capability::TerminalRead,
            Self::SelectTab { .. } | Self::FocusPane { .. } => Capability::Navigate,
            Self::SendText { .. } => Capability::TerminalInput,
            Self::CreateTab { .. } => Capability::TabCreate,
            Self::CloseTab { .. } => Capability::TabClose,
            Self::MoveTab { .. } => Capability::TabMove,
            Self::CreatePane { .. } => Capability::PaneCreate,
            Self::ClosePane { .. } => Capability::PaneClose,
            Self::MovePane { .. } => Capability::PaneMove,
            Self::SwapPanes { .. }
            | Self::BalancePanes { .. }
            | Self::ApplyPaneLayout { .. }
            | Self::ResizeDivider { .. } => Capability::PaneLayout,
            Self::CloseWindow { .. } => Capability::WindowClose,
            Self::FocusWindow { .. } => Capability::WindowFocus,
        }
    }

    /// Whether the action is a query or host action rather than UI output.
    pub fn is_host_operation(&self) -> bool {
        self.operation_kind() != OperationKind::Ui
    }

    fn operation_kind(&self) -> OperationKind {
        match self {
            Self::ShowMessage { .. } | Self::ShowNotice { .. } => OperationKind::Ui,
            Self::QueryWorkspace { .. } => OperationKind::WorkspaceQuery,
            Self::QueryTerminal { .. } => OperationKind::TerminalQuery,
            _ => OperationKind::Action,
        }
    }

    fn validate_fields(&self) -> Result<(), String> {
        let name = self.name();
        match self {
            Self::ShowMessage { message } => {
                validate_text("handler message", message, MAX_MESSAGE_BYTES)
            }
            Self::ShowNotice { title, lines } => {
                validate_text("handler notice title", title, MAX_TITLE_BYTES)?;
                if lines.is_empty() || lines.len() > MAX_NOTICE_LINES {
                    return Err(format!(
                        "handler notice must contain 1 to {MAX_NOTICE_LINES} lines"
                    ));
                }
                lines
                    .iter()
                    .try_for_each(|line| validate_text("handler notice line", line, MAX_LINE_BYTES))
            }
            Self::QueryWorkspace { query_id } => {
                validate_text("workspace query id", query_id, MAX_QUERY_ID_BYTES)
            }
            Self::QueryTerminal {
                query_id,
                history_lines,
                max_bytes,
                ..
            } => {
                validate_text("terminal query id", query_id, MAX_QUERY_ID_BYTES)?;
                validate_terminal_query_bounds(*history_lines, *max_bytes)
            }
            Self::SendText {
                action_id,
                text,
                enter,
                ..
            } => {
                validate_action_id(name, action_id)?;
                if text.len() > MAX_TERMINAL_INPUT_BYTES {
                    return Err(format!(
                        "plugin terminal input is limited to {MAX_TERMINAL_INPUT_BYTES} UTF-8 bytes"
                    ));
                }
                if text.is_empty() && !enter {
                    return Err("plugin terminal input must include text or enter".into());
                }
                if !crate::control::protocol::valid_control_input_text(text) {
                    return Err(
                        "plugin terminal input permits printable UTF-8, tab, and newline only"
                            .into(),
                    );
                }
                Ok(())
            }
            Self::MoveTab {
                action_id,
                destination_index,
                ..
            } => {
                validate_action_id(name, action_id)?;
                if usize::from(*destination_index) >= crate::control::protocol::MAX_TABS_PER_WINDOW
                {
                    return Err(format!(
                        "tab destination index must be below {}",
                        crate::control::protocol::MAX_TABS_PER_WINDOW
                    ));
                }
                Ok(())
            }
            Self::SwapPanes {
                action_id,
                first_pane_id,
                second_pane_id,
                ..
            } => {
                validate_action_id(name, action_id)?;
                if first_pane_id == second_pane_id {
                    return Err("pane swap requires two distinct pane IDs".into());
                }
                Ok(())
            }
            Self::ResizeDivider {
                action_id,
                ratio_millis,
                ..
            } => {
                validate_action_id(name, action_id)?;
                if !(100..=900).contains(ratio_millis) {
                    return Err("divider ratio must be between 100 and 900 thousandths".into());
                }
                Ok(())
            }
            Self::SelectTab { action_id, .. }
            | Self::FocusPane { action_id, .. }
            | Self::CreateTab { action_id, .. }
            | Self::CloseTab { action_id, .. }
            | Self::CreatePane { action_id, .. }
            | Self::ClosePane { action_id, .. }
            | Self::MovePane { action_id, .. }
            | Self::BalancePanes { action_id, .. }
            | Self::ApplyPaneLayout { action_id, .. }
            | Self::CloseWindow { action_id, .. }
            | Self::FocusWindow { action_id, .. } => validate_action_id(name, action_id),
        }
    }
}

/// Bounds shared by handler validation and the host's terminal read.
pub(crate) fn validate_terminal_query_bounds(
    history_lines: u16,
    max_bytes: u32,
) -> Result<(), String> {
    if history_lines > crate::control::protocol::MAX_TEXT_HISTORY_LINES {
        return Err(format!(
            "terminal query history is limited to {} lines",
            crate::control::protocol::MAX_TEXT_HISTORY_LINES
        ));
    }
    if !(1..=MAX_TERMINAL_QUERY_BYTES).contains(&max_bytes) {
        return Err(format!(
            "terminal query bytes must be between 1 and {MAX_TERMINAL_QUERY_BYTES}"
        ));
    }
    Ok(())
}

#[derive(Debug)]
pub struct StructuredHandlerCompletion {
    /// The control ID of the window that submitted the request.
    pub window_id: u64,
    pub plugin_generation: u64,
    pub plugin_id: String,
    pub request_id: u64,
    /// The handler's descriptor, kept to deliver one bounded follow-up
    /// result through the same process.
    pub continuation: StructuredHandlerInvocation,
    pub result: Result<Vec<StructuredResponseAction>, String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct StructuredResponse {
    schema_version: u32,
    request_id: u64,
    #[serde(default)]
    actions: Vec<StructuredResponseAction>,
}

/// Where a lane delivers each completed request: the event loop in the app,
/// a channel in tests.
pub type CompletionSink = Arc<dyn Fn(StructuredHandlerCompletion) + Send + Sync>;

/// Whether a window that submits handler requests is still open, shared
/// with every request it has queued. The window owns it, so dropping it as
/// the window ends marks the window closed: the requests it still has
/// queued are answered with an error rather than run, since nothing would
/// take their results.
#[derive(Debug)]
pub struct WindowRequests {
    open: Arc<AtomicBool>,
}

impl Default for WindowRequests {
    fn default() -> Self {
        Self {
            open: Arc::new(AtomicBool::new(true)),
        }
    }
}

impl Drop for WindowRequests {
    fn drop(&mut self) {
        self.open.store(false, Ordering::Release);
    }
}

struct WorkItem {
    /// The submitting window's control ID. Results are routed by it rather
    /// than by its winit ID, which another window can take once this one
    /// closes (on Wayland it comes from the surface's address).
    window_id: u64,
    window_open: Arc<AtomicBool>,
    plugin_generation: u64,
    request_id: u64,
    invocation: StructuredHandlerInvocation,
    deliver: CompletionSink,
}

/// What the host can see of a lane without blocking on it.
#[derive(Debug, Default)]
struct LaneState {
    /// Requests submitted and not yet finished.
    pending: AtomicUsize,
    /// Whether the lane holds a handler process.
    process: AtomicBool,
}

/// One plugin's handler: a thread, its bounded queue, and at most one
/// persistent process, so a slow plugin delays only its own requests.
struct PluginLane {
    sender: SyncSender<WorkItem>,
    state: Arc<LaneState>,
    thread: JoinHandle<()>,
}

impl PluginLane {
    fn spawn(
        plugin_id: &str,
        shutdown: Arc<AtomicBool>,
        generation: Arc<AtomicU64>,
    ) -> Result<Self, String> {
        let (sender, receiver) = sync_channel(QUEUE_CAPACITY);
        let state = Arc::new(LaneState::default());
        let lane_state = Arc::clone(&state);
        let lane_generation = generation.load(Ordering::Acquire);
        let thread = thread::Builder::new()
            .name(format!("termeric-plugin-{plugin_id}"))
            .spawn(move || {
                run_lane(
                    &receiver,
                    &lane_state,
                    &shutdown,
                    &generation,
                    lane_generation,
                );
            })
            .map_err(|error| format!("plugin handler lane could not start: {error}"))?;
        Ok(Self {
            sender,
            state,
            thread,
        })
    }

    /// Nothing queued or running and no process kept: the lane can give
    /// up its slot.
    fn is_idle(&self) -> bool {
        self.state.pending.load(Ordering::Acquire) == 0
            && !self.state.process.load(Ordering::Acquire)
    }
}

/// Serve one lane's queue until the host drops its sender, shuts down, or
/// moves to another generation. Every request is answered exactly once:
/// one that can no longer run (its generation ended, the host is shutting
/// down, or its window closed) is answered with an error instead, so a
/// window waiting on a result, such as one a plugin closes, never waits
/// forever.
fn run_lane(
    receiver: &Receiver<WorkItem>,
    state: &LaneState,
    shutdown: &AtomicBool,
    generation: &AtomicU64,
    lane_generation: u64,
) {
    let mut handler: Option<PersistentHandler> = None;
    let mut last_reap = Instant::now();
    loop {
        if shutdown.load(Ordering::Acquire) || generation.load(Ordering::Acquire) != lane_generation
        {
            break;
        }
        if last_reap.elapsed() >= PERSISTENT_REAP_INTERVAL {
            if handler
                .as_mut()
                .is_some_and(|handler| !handler.is_running())
            {
                handler = None;
            }
            state.process.store(handler.is_some(), Ordering::Release);
            last_reap = Instant::now();
        }
        let item = match receiver.recv_timeout(WORKER_POLL_INTERVAL) {
            Ok(item) => item,
            Err(RecvTimeoutError::Timeout) => continue,
            Err(RecvTimeoutError::Disconnected) => break,
        };
        let result = match refusal(&item, shutdown, lane_generation) {
            Some(refused) => Err(refused.into()),
            None => {
                let result = run_persistent_handler(
                    &mut handler,
                    &item.invocation,
                    item.request_id,
                    shutdown,
                    generation,
                    item.plugin_generation,
                );
                state.process.store(handler.is_some(), Ordering::Release);
                result
            }
        };
        answer(item, result, state);
    }
    drop(handler);
    // A lane stops only once the host has dropped, or is about to drop, its
    // sender, so this ends; what is still queued, or slips in as the
    // generation turns, is answered rather than lost.
    for item in receiver {
        let refused = refusal(&item, shutdown, lane_generation)
            .unwrap_or("plugins were reloaded before this request ran");
        answer(item, Err(refused.into()), state);
    }
}

/// Why `item` can't run on a lane of `lane_generation`, if it can't.
fn refusal(item: &WorkItem, shutdown: &AtomicBool, lane_generation: u64) -> Option<&'static str> {
    if shutdown.load(Ordering::Acquire) {
        Some("Termeric is shutting down")
    } else if item.plugin_generation != lane_generation {
        Some("plugins were reloaded before this request ran")
    } else if !item.window_open.load(Ordering::Acquire) {
        Some("its window closed before this request ran")
    } else {
        None
    }
}

fn answer(
    item: WorkItem,
    result: Result<Vec<StructuredResponseAction>, String>,
    state: &LaneState,
) {
    (item.deliver)(StructuredHandlerCompletion {
        window_id: item.window_id,
        plugin_generation: item.plugin_generation,
        plugin_id: item.invocation.plugin_id.clone(),
        request_id: item.request_id,
        continuation: item.invocation,
        result,
    });
    state.pending.fetch_sub(1, Ordering::AcqRel);
}

/// The process's one host for plugin event handlers, shared by every
/// window. Each plugin gets its own lane; request IDs and the plugin
/// generation are process-wide, so every window's requests and results
/// agree on them. Closing a window leaves handlers running; a new
/// generation (a plugin reload) or the host's end stops them.
pub struct StructuredPluginHost {
    lanes: Mutex<HashMap<String, PluginLane>>,
    /// Lanes of earlier generations, and idle lanes that gave way, still
    /// finishing; joined when the host ends.
    retired: Mutex<Vec<JoinHandle<()>>>,
    generation: Arc<AtomicU64>,
    /// The agents the current generation's plugins were built with, once a
    /// window's agents changed: their IDs can shadow plugins.
    agents: Mutex<Option<BTreeMap<String, AgentConfig>>>,
    next_request_id: AtomicU64,
    shutdown: Arc<AtomicBool>,
}

impl std::fmt::Debug for StructuredPluginHost {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("StructuredPluginHost")
            .field("generation", &self.generation())
            .finish_non_exhaustive()
    }
}

impl Default for StructuredPluginHost {
    fn default() -> Self {
        Self {
            lanes: Mutex::new(HashMap::new()),
            retired: Mutex::new(Vec::new()),
            generation: Arc::new(AtomicU64::new(1)),
            agents: Mutex::new(None),
            next_request_id: AtomicU64::new(1),
            shutdown: Arc::new(AtomicBool::new(false)),
        }
    }
}

impl StructuredPluginHost {
    /// The current plugin generation. A window builds its plugins at this
    /// generation, and only requests of this generation run.
    pub fn generation(&self) -> u64 {
        self.generation.load(Ordering::Acquire)
    }

    /// Start a new generation: every handler process stops, queued requests
    /// are dropped, and the next request starts a fresh process.
    pub fn advance_generation(&self) -> u64 {
        let next = self.generation().wrapping_add(1).max(1);
        self.generation.store(next, Ordering::Release);
        let retiring = std::mem::take(&mut *self.lanes.lock().unwrap());
        self.retire(retiring.into_values().map(|lane| lane.thread));
        next
    }

    /// Start a new generation for a window whose agents changed to
    /// `agents`, unless the current generation was already started for
    /// them. Every window notices one change to the configuration file on
    /// its own, so only the first of them reloads the handlers; the others
    /// rebuild their plugins in the generation it started. Returns whether
    /// this started one.
    pub fn advance_generation_for_agents(&self, agents: &BTreeMap<String, AgentConfig>) -> bool {
        let mut current = self.agents.lock().unwrap();
        if current.as_ref() == Some(agents) {
            return false;
        }
        *current = Some(agents.clone());
        self.advance_generation();
        true
    }

    /// Keep `threads` until they finish, letting go of those that have, so
    /// the list stays as short as the lanes still winding down.
    fn retire(&self, threads: impl IntoIterator<Item = JoinHandle<()>>) {
        let mut retired = self.retired.lock().unwrap();
        retired.retain(|thread| !thread.is_finished());
        retired.extend(threads);
    }

    /// Queue one request from the window with control ID `window_id` on its
    /// plugin's lane and return its process-wide request ID. `deliver`
    /// receives the completion, which names the same window; `window` says
    /// whether it is still open when the request's turn comes.
    pub fn submit(
        &self,
        window_id: u64,
        window: &WindowRequests,
        plugin_generation: u64,
        invocation: StructuredHandlerInvocation,
        deliver: CompletionSink,
    ) -> Result<u64, String> {
        if self.generation() != plugin_generation {
            return Err("structured plugin generation is no longer active".into());
        }
        let mut lanes = self.lanes.lock().unwrap();
        lanes.retain(|_, lane| !lane.thread.is_finished());
        if !lanes.contains_key(&invocation.plugin_id) {
            if lanes.len() >= MAX_PERSISTENT_HANDLERS {
                // A lane with nothing queued and no process gives way.
                let idle = lanes
                    .iter()
                    .find_map(|(plugin_id, lane)| lane.is_idle().then(|| plugin_id.clone()));
                let Some(idle) = idle else {
                    return Err(format!(
                        "plugin handler limit reached ({MAX_PERSISTENT_HANDLERS})"
                    ));
                };
                let lane = lanes.remove(&idle).expect("idle lane was found");
                self.retire([lane.thread]);
            }
            let lane = PluginLane::spawn(
                &invocation.plugin_id,
                Arc::clone(&self.shutdown),
                Arc::clone(&self.generation),
            )?;
            lanes.insert(invocation.plugin_id.clone(), lane);
        }
        let lane = &lanes[&invocation.plugin_id];
        let request_id = self.next_request_id.fetch_add(1, Ordering::AcqRel);
        lane.state.pending.fetch_add(1, Ordering::AcqRel);
        let item = WorkItem {
            window_id,
            window_open: Arc::clone(&window.open),
            plugin_generation,
            request_id,
            invocation,
            deliver,
        };
        match lane.sender.try_send(item) {
            Ok(()) => Ok(request_id),
            Err(error) => {
                lane.state.pending.fetch_sub(1, Ordering::AcqRel);
                Err(match error {
                    TrySendError::Full(_) => {
                        format!("plugin handler queue is full ({QUEUE_CAPACITY} pending requests)")
                    }
                    TrySendError::Disconnected(_) => "plugin handler lane is unavailable".into(),
                })
            }
        }
    }
}

impl Drop for StructuredPluginHost {
    fn drop(&mut self) {
        self.shutdown.store(true, Ordering::Release);
        let lanes = std::mem::take(&mut *self.lanes.lock().unwrap());
        let mut threads = std::mem::take(&mut *self.retired.lock().unwrap());
        threads.extend(lanes.into_values().map(|lane| lane.thread));
        for thread in threads {
            let _ = thread.join();
        }
    }
}

fn handler_command(invocation: &StructuredHandlerInvocation) -> Command {
    let mut command = Command::new(&invocation.program);
    command
        .args(&invocation.args)
        .env_clear()
        .env("TERMERIC_PLUGIN_ID", &invocation.plugin_id)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0);
    if let Some(path) = std::env::var_os("PATH") {
        command.env("PATH", path);
    }
    command
}

#[derive(Debug)]
struct PersistentHandler {
    program: String,
    args: Vec<String>,
    grants: PluginPermissions,
    child: Child,
    requests: Option<SyncSender<PersistentRequest>>,
    io_thread: Option<JoinHandle<()>>,
    stderr: Arc<Mutex<BoundedStderr>>,
    stderr_thread: Option<JoinHandle<()>>,
}

#[derive(Debug)]
struct PersistentRequest {
    event: Vec<u8>,
    response: SyncSender<Result<(Vec<u8>, bool), String>>,
}

#[derive(Debug, Default)]
struct BoundedStderr {
    bytes: Vec<u8>,
    truncated: bool,
}

impl PersistentHandler {
    fn spawn(invocation: &StructuredHandlerInvocation) -> Result<Self, String> {
        let mut child = handler_command(invocation)
            .spawn()
            .map_err(|error| bounded_error(&format!("handler launch failed: {error}")))?;
        let Some(mut stdin) = child.stdin.take() else {
            terminate_handler(&mut child);
            return Err("handler stdin was unavailable".into());
        };
        let Some(stdout) = child.stdout.take() else {
            terminate_handler(&mut child);
            return Err("handler stdout was unavailable".into());
        };
        let Some(stderr_stream) = child.stderr.take() else {
            terminate_handler(&mut child);
            return Err("handler stderr was unavailable".into());
        };
        let (request_sender, request_receiver) = sync_channel::<PersistentRequest>(1);
        let io_thread = thread::spawn(move || {
            let mut stdout = BufReader::new(stdout);
            while let Ok(request) = request_receiver.recv() {
                let result = stdin
                    .write_all(&request.event)
                    .and_then(|()| stdin.write_all(b"\n"))
                    .and_then(|()| stdin.flush())
                    .map_err(|error| bounded_error(&format!("handler event write failed: {error}")))
                    .and_then(|()| {
                        read_response_line(&mut stdout)
                            .map_err(|error| {
                                bounded_error(&format!("handler response read failed: {error}"))
                            })?
                            .ok_or_else(|| "handler closed stdout".to_string())
                    });
                let failed = result.is_err();
                let _ = request.response.send(result);
                if failed {
                    break;
                }
            }
        });
        let stderr = Arc::new(Mutex::new(BoundedStderr::default()));
        let stderr_target = Arc::clone(&stderr);
        let stderr_thread = thread::spawn(move || {
            drain_persistent_stderr(stderr_stream, &stderr_target);
        });
        Ok(Self {
            program: invocation.program.clone(),
            args: invocation.args.clone(),
            grants: invocation.authority.grants().clone(),
            child,
            requests: Some(request_sender),
            io_thread: Some(io_thread),
            stderr,
            stderr_thread: Some(stderr_thread),
        })
    }

    fn matches(&self, invocation: &StructuredHandlerInvocation) -> bool {
        self.program == invocation.program
            && self.args == invocation.args
            && &self.grants == invocation.authority.grants()
    }

    fn is_running(&mut self) -> bool {
        matches!(self.child.try_wait(), Ok(None))
    }

    fn request(
        &mut self,
        event: Vec<u8>,
        request_id: u64,
        authority: &HandlerAuthority,
        shutdown: &AtomicBool,
        active_generation: &AtomicU64,
        expected_generation: u64,
    ) -> Result<Vec<StructuredResponseAction>, String> {
        let deadline = Instant::now() + HANDLER_TIMEOUT;
        let cancelled = || {
            shutdown.load(Ordering::Acquire)
                || active_generation.load(Ordering::Acquire) != expected_generation
        };
        if let Some(status) = self
            .child
            .try_wait()
            .map_err(|error| bounded_error(&format!("handler wait failed: {error}")))?
        {
            return Err(self.exit_error(status, deadline));
        }
        let (response_sender, response_receiver) = sync_channel(1);
        self.requests
            .as_ref()
            .ok_or_else(|| "handler input is closed".to_string())?
            .try_send(PersistentRequest {
                event,
                response: response_sender,
            })
            .map_err(|error| match error {
                TrySendError::Full(_) => String::from("handler already has a pending request"),
                TrySendError::Disconnected(_) => String::from("handler input is closed"),
            })?;
        let (bytes, truncated) = loop {
            if cancelled() {
                return Err("structured plugin handler was cancelled".into());
            }
            match response_receiver.recv_timeout(WORKER_POLL_INTERVAL) {
                Ok(Ok(response)) => break response,
                Ok(Err(error)) => return Err(self.failure_error(error, deadline, &cancelled)),
                Err(RecvTimeoutError::Timeout) if Instant::now() < deadline => {}
                Err(RecvTimeoutError::Timeout) => {
                    return Err(format!(
                        "structured plugin handler exceeded {} ms",
                        HANDLER_TIMEOUT.as_millis()
                    ));
                }
                Err(RecvTimeoutError::Disconnected) => {
                    return Err(self.failure_error(
                        "handler response channel closed".into(),
                        deadline,
                        &cancelled,
                    ));
                }
            }
        };
        if truncated {
            return Err(format!(
                "structured plugin response exceeds {MAX_OUTPUT_BYTES} bytes"
            ));
        }
        let actions = parse_response(&bytes, authority, request_id)?;
        if let Some(status) = self
            .child
            .try_wait()
            .map_err(|error| bounded_error(&format!("handler wait failed: {error}")))?
        {
            return Err(self.exit_error(status, deadline));
        }
        Ok(actions)
    }

    /// Explain a request whose I/O failed. A closed stdout or broken pipe
    /// usually means the handler is exiting, so wait for it, until `deadline`
    /// or cancellation, and report its exit status and standard error rather
    /// than only the broken pipe.
    fn failure_error(
        &mut self,
        error: String,
        deadline: Instant,
        cancelled: &dyn Fn() -> bool,
    ) -> String {
        loop {
            match self.child.try_wait() {
                Ok(Some(status)) => return self.exit_error(status, deadline),
                Ok(None) if Instant::now() < deadline && !cancelled() => {
                    thread::sleep(WORKER_POLL_INTERVAL);
                }
                Ok(None) | Err(_) => return error,
            }
        }
    }

    /// The handler's exit status and the start of its standard error. The
    /// stderr reader gets until `deadline` to reach end of file, so what the
    /// handler wrote just before exiting is included. The handler is being
    /// discarded, so its process group is killed first: a descendant still
    /// holding the pipe would otherwise keep the reader from finishing.
    fn exit_error(&mut self, status: std::process::ExitStatus, deadline: Instant) -> String {
        terminate_handler_group(self.child.id());
        while self
            .stderr_thread
            .as_ref()
            .is_some_and(|reader| !reader.is_finished())
            && Instant::now() < deadline
        {
            thread::sleep(WORKER_POLL_INTERVAL);
        }
        if let Some(reader) = self.stderr_thread.take_if(|reader| reader.is_finished()) {
            let _ = reader.join();
        }
        let stderr = self.stderr.lock().unwrap();
        let mut detail = String::from_utf8_lossy(&stderr.bytes).into_owned();
        if stderr.truncated {
            detail.push('…');
        }
        bounded_error(&format!("handler exited with {status}: {detail}"))
    }
}

impl Drop for PersistentHandler {
    fn drop(&mut self) {
        terminate_handler(&mut self.child);
        self.requests.take();
        if let Some(thread) = self.io_thread.take() {
            let _ = thread.join();
        }
        if let Some(thread) = self.stderr_thread.take() {
            let _ = thread.join();
        }
    }
}

/// Run one request on a lane's handler process, starting it when the lane
/// has none or its process has ended. A failed request discards the process;
/// the next request starts a fresh one.
fn run_persistent_handler(
    handler: &mut Option<PersistentHandler>,
    invocation: &StructuredHandlerInvocation,
    request_id: u64,
    shutdown: &AtomicBool,
    active_generation: &AtomicU64,
    expected_generation: u64,
) -> Result<Vec<StructuredResponseAction>, String> {
    let event = persistent_request_json(
        &invocation.event_json,
        invocation.settings.as_ref(),
        request_id,
    )?;
    if handler
        .as_mut()
        .is_some_and(|handler| !handler.is_running())
    {
        *handler = None;
    }
    let running = match handler {
        Some(running) => running,
        None => handler.insert(PersistentHandler::spawn(invocation)?),
    };
    let result = if running.matches(invocation) {
        running.request(
            event,
            request_id,
            &invocation.authority,
            shutdown,
            active_generation,
            expected_generation,
        )
    } else {
        Err("handler descriptor changed within one plugin generation".into())
    };
    if result.is_err() {
        *handler = None;
    }
    result
}

/// Add the plugin's settings and the correlation ID to a schema-v1 event or
/// result for a persistent handler. The source envelope stays within
/// [`MAX_PERSISTENT_SOURCE_EVENT_BYTES`] and the settings document within
/// [`MAX_REQUEST_SETTINGS_BYTES`], so a valid request always fits.
fn persistent_request_json(
    event_json: &str,
    settings: Option<&serde_json::Value>,
    request_id: u64,
) -> Result<Vec<u8>, String> {
    let mut value: serde_json::Value = serde_json::from_str(event_json)
        .map_err(|error| bounded_error(&format!("invalid structured event JSON: {error}")))?;
    let object = value
        .as_object_mut()
        .ok_or_else(|| "structured event must be a JSON object".to_string())?;
    if object
        .get("schema_version")
        .and_then(serde_json::Value::as_u64)
        != Some(u64::from(SCHEMA_VERSION))
    {
        return Err(format!(
            "handlers require a schema-v{SCHEMA_VERSION} source event"
        ));
    }
    if let Some(settings) = settings {
        object.insert("settings".into(), settings.clone());
    }
    object.insert("request_id".into(), serde_json::json!(request_id));
    let bytes = serde_json::to_vec(&value)
        .map_err(|error| bounded_error(&format!("could not encode the handler event: {error}")))?;
    if bytes.len() > MAX_STRUCTURED_EVENT_BYTES {
        return Err(format!(
            "structured event exceeds {MAX_STRUCTURED_EVENT_BYTES} bytes"
        ));
    }
    Ok(bytes)
}

/// Read one response line, without its newline or a trailing `\r`, and
/// whether it ran past [`MAX_OUTPUT_BYTES`]. A response must end with a
/// newline.
fn read_response_line(reader: &mut impl BufRead) -> std::io::Result<Option<(Vec<u8>, bool)>> {
    let mut bytes = Vec::new();
    let Some(read) = read_bounded_line(reader, MAX_OUTPUT_BYTES as usize, &mut bytes)? else {
        return Ok(None);
    };
    if !read.terminated {
        return Err(std::io::Error::new(
            std::io::ErrorKind::UnexpectedEof,
            "handler response ended without a newline",
        ));
    }
    if bytes.last() == Some(&b'\r') {
        bytes.pop();
    }
    Ok(Some((bytes, read.oversized)))
}

fn drain_persistent_stderr(mut stream: impl Read, target: &Mutex<BoundedStderr>) {
    let mut buffer = [0_u8; 1024];
    loop {
        let read = match stream.read(&mut buffer) {
            Ok(0) | Err(_) => return,
            Ok(read) => read,
        };
        let mut target = target.lock().unwrap();
        let remaining = (MAX_ERROR_BYTES as usize).saturating_sub(target.bytes.len());
        let retained = read.min(remaining);
        target.bytes.extend_from_slice(&buffer[..retained]);
        target.truncated |= retained < read;
    }
}

fn terminate_handler(child: &mut std::process::Child) {
    terminate_handler_group(child.id());
    let _ = child.kill();
    let _ = child.wait();
}

fn terminate_handler_group(process_id: u32) {
    if let Ok(process_group) = libc::pid_t::try_from(process_id) {
        // The child is started as its own process-group leader. A negative PID
        // targets that complete group and prevents timed-out descendants from
        // surviving or retaining captured output pipes.
        unsafe {
            libc::kill(-process_group, libc::SIGKILL);
        }
    }
}

/// Parse one handler response, which must echo `expected_request_id`.
fn parse_response(
    bytes: &[u8],
    authority: &HandlerAuthority,
    expected_request_id: u64,
) -> Result<Vec<StructuredResponseAction>, String> {
    let response: StructuredResponse = serde_json::from_slice(bytes)
        .map_err(|error| bounded_error(&format!("invalid handler response JSON: {error}")))?;
    if response.schema_version != SCHEMA_VERSION {
        return Err(format!(
            "unsupported handler response schema {}; expected {SCHEMA_VERSION}",
            response.schema_version
        ));
    }
    if response.request_id != expected_request_id {
        return Err(format!(
            "handler response request id {} did not match {expected_request_id}",
            response.request_id
        ));
    }
    authority.check(&response.actions)?;
    Ok(response.actions)
}

fn validate_action_id(action: &str, action_id: &str) -> Result<(), String> {
    validate_text(
        &format!("{action} action id"),
        action_id,
        MAX_ACTION_ID_BYTES,
    )
}

fn validate_text(label: &str, value: &str, max: usize) -> Result<(), String> {
    if value.trim().is_empty()
        || value.len() > max
        || value.contains('\0')
        || value.chars().any(char::is_control)
    {
        return Err(format!(
            "{label} must be non-empty, control-safe UTF-8 within {max} bytes"
        ));
    }
    Ok(())
}

fn bounded_error(value: &str) -> String {
    let output = crate::safe_text::single_line_within(value, 1_024);
    if output.is_empty() {
        "structured plugin handler failed".into()
    } else {
        output
    }
}

#[cfg(test)]
impl StructuredHandlerInvocation {
    /// A descriptor for tests that need a completion but never run it.
    pub(crate) fn fixture(plugin_id: &str, grants: PluginPermissions) -> Self {
        Self {
            plugin_id: plugin_id.into(),
            program: "fixture".into(),
            args: Vec::new(),
            event_json: "{}".into(),
            settings: None,
            authority: HandlerAuthority::for_event(grants, false),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;
    use std::io::Cursor;

    fn grants(permissions: &[&str]) -> PluginPermissions {
        PluginPermissions::from_permissions(
            permissions
                .iter()
                .map(|permission| permission.parse().unwrap()),
        )
        .unwrap()
    }

    fn persistent_authority(permissions: &[&str]) -> HandlerAuthority {
        HandlerAuthority::for_event(grants(permissions), false)
    }

    /// Check one persistent response carrying `actions` (a JSON array body).
    fn check(
        authority: &HandlerAuthority,
        actions: &str,
    ) -> Result<Vec<StructuredResponseAction>, String> {
        parse_response(
            format!(r#"{{"schema_version":1,"request_id":9,"actions":[{actions}]}}"#).as_bytes(),
            authority,
            9,
        )
    }

    fn persistent_fixture(plugin_id: &str, script: &str) -> StructuredHandlerInvocation {
        StructuredHandlerInvocation {
            plugin_id: plugin_id.into(),
            program: "/bin/sh".into(),
            args: vec!["-c".into(), script.into()],
            event_json: r#"{"schema_version":1,"event":{"type":"app-ready"}}"#.into(),
            settings: None,
            authority: persistent_authority(&["ui-actions"]),
        }
    }

    /// One valid sample of every action, with the capability it needs.
    const ACTION_SAMPLES: &[(&str, &str)] = &[
        ("ui-actions", r#"{"type":"show-message","message":"done"}"#),
        (
            "ui-actions",
            r#"{"type":"show-notice","title":"Result","lines":["one","two"]}"#,
        ),
        (
            "workspace-query",
            r#"{"type":"query-workspace","query_id":"inventory"}"#,
        ),
        (
            "terminal-read",
            r#"{"type":"query-terminal","query_id":"text","tab_id":7,"pane_id":3,"history_lines":20,"max_bytes":2048}"#,
        ),
        (
            "navigate",
            r#"{"type":"select-tab","action_id":"a","tab_id":7}"#,
        ),
        (
            "navigate",
            r#"{"type":"focus-pane","action_id":"a","window_id":2,"tab_id":7,"pane_id":3}"#,
        ),
        (
            "terminal-input",
            r#"{"type":"send-text","action_id":"a","tab_id":7,"pane_id":3,"text":"ls","paste":false,"enter":true}"#,
        ),
        ("tab-create", r#"{"type":"create-tab","action_id":"a"}"#),
        (
            "tab-close",
            r#"{"type":"close-tab","action_id":"a","tab_id":7}"#,
        ),
        (
            "tab-move",
            r#"{"type":"move-tab","action_id":"a","tab_id":7,"destination_window_id":2,"destination_index":0}"#,
        ),
        (
            "pane-create",
            r#"{"type":"create-pane","action_id":"a","tab_id":7,"pane_id":3,"direction":"vertical"}"#,
        ),
        (
            "pane-close",
            r#"{"type":"close-pane","action_id":"a","tab_id":7,"pane_id":3}"#,
        ),
        (
            "pane-move",
            r#"{"type":"move-pane","action_id":"a","source_tab_id":7,"pane_id":3,"destination_tab_id":8,"target_pane_id":0,"direction":"horizontal"}"#,
        ),
        (
            "pane-layout",
            r#"{"type":"swap-panes","action_id":"a","tab_id":7,"first_pane_id":0,"second_pane_id":1}"#,
        ),
        (
            "pane-layout",
            r#"{"type":"balance-panes","action_id":"a","tab_id":7}"#,
        ),
        (
            "pane-layout",
            r#"{"type":"apply-pane-layout","action_id":"a","tab_id":7,"preset":"grid"}"#,
        ),
        (
            "pane-layout",
            r#"{"type":"resize-divider","action_id":"a","tab_id":7,"divider_id":1,"ratio_millis":400}"#,
        ),
        (
            "window-close:all-windows",
            r#"{"type":"close-window","action_id":"a","window_id":2}"#,
        ),
        (
            "window-focus",
            r#"{"type":"focus-window","action_id":"a","window_id":2}"#,
        ),
    ];

    #[test]
    fn every_action_requires_exactly_its_capability() {
        for (permission, action) in ACTION_SAMPLES {
            let granted = persistent_authority(&[permission]);
            let parsed = check(&granted, action)
                .unwrap_or_else(|error| panic!("{action} should pass with {permission}: {error}"));
            assert_eq!(parsed.len(), 1);
            let capability = permission.split(':').next().unwrap();
            assert_eq!(parsed[0].required_capability().name(), capability);

            let everything_else = ACTION_SAMPLES
                .iter()
                .map(|(other, _)| *other)
                .filter(|other| other.split(':').next() != Some(capability))
                .collect::<BTreeSet<_>>()
                .into_iter()
                .collect::<Vec<_>>();
            let error = check(&persistent_authority(&everything_else), action).unwrap_err();
            assert!(
                error.contains(&format!("without the {capability} permission")),
                "{action}: {error}"
            );
        }
    }

    #[test]
    fn response_schema_and_correlation_fail_closed() {
        let authority = persistent_authority(&["ui-actions"]);
        let valid = br#"{"schema_version":1,"request_id":42,"actions":[{"type":"show-message","message":"done"},{"type":"show-notice","title":"Result","lines":["one","two"]}]}"#;
        assert_eq!(parse_response(valid, &authority, 42).unwrap().len(), 2);
        assert!(
            parse_response(
                br#"{"schema_version":2,"request_id":42,"actions":[]}"#,
                &authority,
                42
            )
            .unwrap_err()
            .contains("unsupported")
        );
        assert!(
            parse_response(valid, &authority, 41)
                .unwrap_err()
                .contains("did not match")
        );
        assert!(
            parse_response(br#"{"schema_version":1,"actions":[]}"#, &authority, 42)
                .unwrap_err()
                .contains("missing field `request_id`")
        );
        for removed in [
            r#"{"type":"query-process","query_id":"all"}"#,
            r#"{"type":"select-process-tab","action_id":"a","window_id":2,"tab_id":7}"#,
            r#"{"type":"select-tab","action_id":"a","tab_id":7,"extra":true}"#,
        ] {
            assert!(
                check(&persistent_authority(&["navigate"]), removed)
                    .unwrap_err()
                    .contains("invalid handler response JSON"),
                "{removed}"
            );
        }
    }

    #[test]
    fn optional_window_ids_parse_as_absent_or_exact() {
        let authority = persistent_authority(&["navigate:all-windows", "tab-move"]);
        assert_eq!(
            check(
                &authority,
                r#"{"type":"select-tab","action_id":"a","tab_id":7}"#
            )
            .unwrap(),
            vec![StructuredResponseAction::SelectTab {
                action_id: "a".into(),
                window_id: None,
                tab_id: 7,
            }]
        );
        assert_eq!(
            check(
                &authority,
                r#"{"type":"select-tab","action_id":"a","window_id":4,"tab_id":7}"#
            )
            .unwrap(),
            vec![StructuredResponseAction::SelectTab {
                action_id: "a".into(),
                window_id: Some(4),
                tab_id: 7,
            }]
        );
        assert!(matches!(
            check(
                &authority,
                r#"{"type":"move-tab","action_id":"a","source_window_id":3,"tab_id":7,"destination_index":1}"#
            )
            .unwrap()[..],
            [StructuredResponseAction::MoveTab {
                source_window_id: Some(3),
                destination_window_id: None,
                ..
            }]
        ));
    }

    #[test]
    fn follow_up_chain_allows_one_query_then_one_host_operation() {
        let event =
            persistent_authority(&["ui-actions", "workspace-query", "terminal-read", "navigate"]);
        let query = StructuredResponseAction::QueryWorkspace {
            query_id: "inventory".into(),
        };
        let select = r#"{"type":"select-tab","action_id":"a","tab_id":7}"#;
        let read = r#"{"type":"query-terminal","query_id":"t","tab_id":7,"pane_id":0,"history_lines":0,"max_bytes":64}"#;
        let workspace_query = r#"{"type":"query-workspace","query_id":"again"}"#;
        let message = r#"{"type":"show-message","message":"still allowed"}"#;

        // An event may start one query or one host action.
        assert!(check(&event, workspace_query).is_ok());
        assert!(check(&event, read).is_ok());
        assert!(check(&event, select).is_ok());

        // A workspace query result may perform one host action or terminal
        // query, but never another workspace query.
        let after_query = event.after(&query);
        assert!(check(&after_query, select).is_ok());
        assert!(check(&after_query, read).is_ok());
        assert!(
            check(&after_query, workspace_query)
                .unwrap_err()
                .contains("from a workspace query result")
        );

        // Action results and terminal query results end the chain.
        let select_action = check(&after_query, select).unwrap().remove(0);
        let read_action = check(&event, read).unwrap().remove(0);
        for ended in [
            after_query.after(&select_action),
            event.after(&select_action),
            event.after(&read_action),
            after_query.after(&read_action),
        ] {
            for action in [select, read, workspace_query] {
                assert!(
                    check(&ended, action)
                        .unwrap_err()
                        .contains("cannot start host operations")
                );
            }
            assert!(check(&ended, message).is_ok());
        }
    }

    #[test]
    fn responses_carry_at_most_one_host_operation_and_eight_actions() {
        let authority = persistent_authority(&["ui-actions", "workspace-query", "navigate"]);
        for pair in [
            r#"{"type":"query-workspace","query_id":"one"},{"type":"query-workspace","query_id":"two"}"#,
            r#"{"type":"query-workspace","query_id":"one"},{"type":"select-tab","action_id":"a","tab_id":7}"#,
        ] {
            assert!(
                check(&authority, pair)
                    .unwrap_err()
                    .contains("more than one host operation")
            );
        }
        let mixed = r#"{"type":"select-tab","action_id":"a","tab_id":7},{"type":"show-message","message":"selected"}"#;
        assert_eq!(check(&authority, mixed).unwrap().len(), 2);
        let too_many = [r#"{"type":"show-message","message":"m"}"#; MAX_ACTIONS + 1].join(",");
        assert!(
            check(&authority, &too_many)
                .unwrap_err()
                .contains("more than 8 actions")
        );
    }

    #[test]
    fn window_scoped_window_close_is_limited_to_invoked_commands() {
        let close = r#"{"type":"close-window","action_id":"close"}"#;
        let window_scoped = grants(&["window-close"]);
        let lifecycle = HandlerAuthority::for_event(window_scoped.clone(), false);
        assert!(
            check(&lifecycle, close)
                .unwrap_err()
                .contains("outside an invoked plugin command")
        );
        let command = HandlerAuthority::for_event(window_scoped, true);
        assert!(check(&command, close).is_ok());
        // A workspace query keeps the command origin for its follow-up.
        let query = StructuredResponseAction::QueryWorkspace {
            query_id: "q".into(),
        };
        assert!(check(&command.after(&query), close).is_ok());

        let all_windows = persistent_authority(&["window-close:all-windows"]);
        assert!(check(&all_windows, close).is_ok());
    }

    #[test]
    fn action_fields_are_bounded_and_control_safe() {
        let authority = persistent_authority(&[
            "ui-actions",
            "workspace-query",
            "terminal-read",
            "terminal-input",
            "tab-move",
            "pane-layout",
            "navigate",
        ]);
        let rejected = [
            (
                format!(
                    r#"{{"type":"show-message","message":{:?}}}"#,
                    "x".repeat(MAX_MESSAGE_BYTES + 1)
                ),
                "control-safe",
            ),
            (
                r#"{"type":"show-message","message":"two\nlines"}"#.into(),
                "control-safe",
            ),
            (
                r#"{"type":"show-notice","title":"t","lines":[]}"#.into(),
                "1 to 16 lines",
            ),
            (
                r#"{"type":"query-workspace","query_id":"bad\nquery"}"#.into(),
                "control-safe",
            ),
            (
                r#"{"type":"select-tab","action_id":"","tab_id":7}"#.into(),
                "select-tab action id",
            ),
            (
                format!(
                    r#"{{"type":"query-terminal","query_id":"t","tab_id":7,"pane_id":0,"history_lines":{},"max_bytes":64}}"#,
                    crate::control::protocol::MAX_TEXT_HISTORY_LINES + 1
                ),
                "history is limited",
            ),
            (
                format!(
                    r#"{{"type":"query-terminal","query_id":"t","tab_id":7,"pane_id":0,"history_lines":0,"max_bytes":{}}}"#,
                    MAX_TERMINAL_QUERY_BYTES + 1
                ),
                "bytes must be between",
            ),
            (
                r#"{"type":"query-terminal","query_id":"t","tab_id":7,"pane_id":0,"history_lines":0,"max_bytes":0}"#.into(),
                "bytes must be between",
            ),
            (
                format!(
                    r#"{{"type":"send-text","action_id":"a","tab_id":7,"pane_id":0,"text":{:?},"paste":false,"enter":false}}"#,
                    "x".repeat(MAX_TERMINAL_INPUT_BYTES + 1)
                ),
                "limited to",
            ),
            (
                r#"{"type":"send-text","action_id":"a","tab_id":7,"pane_id":0,"text":"","paste":false,"enter":false}"#.into(),
                "text or enter",
            ),
            (
                r#"{"type":"send-text","action_id":"a","tab_id":7,"pane_id":0,"text":"\u001b[A","paste":false,"enter":false}"#.into(),
                "printable UTF-8",
            ),
            (
                format!(
                    r#"{{"type":"move-tab","action_id":"a","tab_id":7,"destination_index":{}}}"#,
                    crate::control::protocol::MAX_TABS_PER_WINDOW
                ),
                "destination index",
            ),
            (
                r#"{"type":"swap-panes","action_id":"a","tab_id":7,"first_pane_id":1,"second_pane_id":1}"#.into(),
                "two distinct pane IDs",
            ),
            (
                r#"{"type":"resize-divider","action_id":"a","tab_id":7,"divider_id":1,"ratio_millis":950}"#.into(),
                "between 100 and 900",
            ),
        ];
        for (action, expected) in rejected {
            let error = check(&authority, &action).unwrap_err();
            assert!(error.contains(expected), "{action}: {error}");
        }
        let tab_and_newline = r#"{"type":"send-text","action_id":"a","tab_id":7,"pane_id":0,"text":"a\tb\n","paste":true,"enter":false}"#;
        assert!(check(&authority, tab_and_newline).is_ok());
        let enter_only = r#"{"type":"send-text","action_id":"a","tab_id":7,"pane_id":0,"text":"","paste":false,"enter":true}"#;
        assert!(check(&authority, enter_only).is_ok());
    }

    #[test]
    fn persistent_query_then_action_then_result_reuses_one_bounded_handler() {
        let mut invocation = persistent_fixture(
            "workspace-action-fixture",
            r#"count=0
while IFS= read -r event; do
  count=$((count + 1))
  if [ "$count" -eq 1 ]; then
    printf '{"schema_version":1,"request_id":1,"actions":[{"type":"query-workspace","query_id":"inventory"}]}\n'
  elif [ "$count" -eq 2 ]; then
    printf '{"schema_version":1,"request_id":2,"actions":[{"type":"select-tab","action_id":"select-active","window_id":4,"tab_id":7}]}\n'
  else
    printf '{"schema_version":1,"request_id":3,"actions":[{"type":"show-message","message":"action-result-received"}]}\n'
  fi
done"#,
        );
        invocation.authority =
            persistent_authority(&["ui-actions", "workspace-query", "navigate:all-windows"]);
        let mut handler = None;
        let shutdown = AtomicBool::new(false);
        let generation = AtomicU64::new(1);
        let query = run_persistent_handler(&mut handler, &invocation, 1, &shutdown, &generation, 1)
            .unwrap();
        assert!(matches!(
            query[..],
            [StructuredResponseAction::QueryWorkspace { .. }]
        ));

        invocation.event_json = r#"{"schema_version":1,"origin_window_id":4,"event":{"type":"workspace-query-result","query_id":"inventory","windows":[]}}"#.into();
        invocation.authority = invocation.authority.after(&query[0]);
        let action =
            run_persistent_handler(&mut handler, &invocation, 2, &shutdown, &generation, 1)
                .unwrap();
        assert_eq!(
            action,
            vec![StructuredResponseAction::SelectTab {
                action_id: "select-active".into(),
                window_id: Some(4),
                tab_id: 7,
            }]
        );

        invocation.event_json = r#"{"schema_version":1,"origin_window_id":4,"event":{"type":"action-result","action":"select-tab","action_id":"select-active","ok":true}}"#.into();
        invocation.authority = invocation.authority.after(&action[0]);
        assert_eq!(
            run_persistent_handler(&mut handler, &invocation, 3, &shutdown, &generation, 1)
                .unwrap(),
            vec![StructuredResponseAction::ShowMessage {
                message: "action-result-received".into(),
            }]
        );
        assert!(handler.is_some());
    }

    #[test]
    fn persistent_result_that_repeats_its_operation_fails_closed() {
        let mut invocation = persistent_fixture(
            "pane-create-fixture",
            r#"count=0
while IFS= read -r event; do
  count=$((count + 1))
  printf '{"schema_version":1,"request_id":%s,"actions":[{"type":"create-pane","action_id":"split","tab_id":7,"pane_id":3,"direction":"vertical"}]}\n' "$count"
done"#,
        );
        invocation.authority = persistent_authority(&["pane-create"]);
        let mut handler = None;
        let shutdown = AtomicBool::new(false);
        let generation = AtomicU64::new(1);
        let created =
            run_persistent_handler(&mut handler, &invocation, 1, &shutdown, &generation, 1)
                .unwrap();
        invocation.event_json = r#"{"schema_version":1,"event":{"type":"action-result","action":"create-pane","ok":true}}"#.into();
        invocation.authority = invocation.authority.after(&created[0]);
        let error = run_persistent_handler(&mut handler, &invocation, 2, &shutdown, &generation, 1)
            .unwrap_err();
        assert!(error.contains("cannot start host operations"), "{error}");
        // A protocol violation discards the process; the next event restarts it.
        assert!(handler.is_none());
    }

    #[test]
    fn persistent_protocol_requires_object_source_and_keeps_one_schema() {
        let source = r#"{"schema_version":1,"event":{"type":"app-ready"}}"#;
        let request = persistent_request_json(source, None, 42).unwrap();
        let value: serde_json::Value = serde_json::from_slice(&request).unwrap();
        assert_eq!(value["schema_version"], SCHEMA_VERSION);
        assert_eq!(value["request_id"], 42);
        assert!(value.get("settings").is_none());
        assert!(persistent_request_json("[]", None, 1).is_err());
        assert!(persistent_request_json(r#"{"schema_version":2}"#, None, 1).is_err());
    }

    #[test]
    fn every_request_of_a_conversation_carries_its_settings() {
        let settings = serde_json::json!({"direction": "horizontal"});
        let event = r#"{"schema_version":1,"plugin_id":"split-here","origin_window_id":3,"event":{"type":"command-invoked","command_id":"split-here.split"}}"#;
        let result = r#"{"schema_version":1,"plugin_id":"split-here","origin_window_id":3,"event":{"type":"action-result","action":"create-pane","ok":true}}"#;
        for source in [event, result] {
            let request = persistent_request_json(source, Some(&settings), 7).unwrap();
            let value: serde_json::Value = serde_json::from_slice(&request).unwrap();
            assert_eq!(value["settings"], settings);
            assert_eq!(value["request_id"], 7);
            assert!(value.get("context").is_none());
        }
    }

    #[test]
    fn persistent_line_reader_bounds_and_drains_each_frame() {
        let oversized = format!("{}\nnext\n", "x".repeat(MAX_OUTPUT_BYTES as usize + 17));
        let mut reader = Cursor::new(oversized.into_bytes());
        let (first, truncated) = read_response_line(&mut reader).unwrap().unwrap();
        assert_eq!(first.len(), MAX_OUTPUT_BYTES as usize);
        assert!(truncated);
        assert_eq!(
            read_response_line(&mut reader).unwrap().unwrap(),
            (b"next".to_vec(), false)
        );
        assert!(read_response_line(&mut reader).unwrap().is_none());

        let mut crlf = Cursor::new(b"{}\r\npartial".to_vec());
        assert_eq!(
            read_response_line(&mut crlf).unwrap().unwrap(),
            (b"{}".to_vec(), false)
        );
        assert!(read_response_line(&mut crlf).is_err());
    }

    #[test]
    fn persistent_handler_reuses_one_stateful_process() {
        let invocation = persistent_fixture(
            "stateful-fixture",
            r#"count=0
while IFS= read -r event; do
  count=$((count + 1))
  printf '{"schema_version":1,"request_id":%s,"actions":[{"type":"show-message","message":"pid=%s count=%s"}]}\n' "$count" "$$" "$count"
done"#,
        );
        let mut handler = None;
        let shutdown = AtomicBool::new(false);
        let generation = AtomicU64::new(1);
        let first = run_persistent_handler(&mut handler, &invocation, 1, &shutdown, &generation, 1)
            .unwrap();
        let second =
            run_persistent_handler(&mut handler, &invocation, 2, &shutdown, &generation, 1)
                .unwrap();
        let StructuredResponseAction::ShowMessage { message: first } = &first[0] else {
            panic!("fixture returned the wrong action")
        };
        let StructuredResponseAction::ShowMessage { message: second } = &second[0] else {
            panic!("fixture returned the wrong action")
        };
        let first_pid = first.split_once(" count=").unwrap().0;
        let second_pid = second.split_once(" count=").unwrap().0;
        assert_eq!(first_pid, second_pid);
        assert!(first.ends_with("count=1"));
        assert!(second.ends_with("count=2"));
        assert!(handler.is_some());
    }

    #[test]
    fn persistent_handler_is_replaced_when_its_grants_change() {
        let mut invocation = persistent_fixture(
            "descriptor-fixture",
            r#"while IFS= read -r event; do
  printf '{"schema_version":1,"request_id":1,"actions":[]}\n'
done"#,
        );
        let mut handler = None;
        let shutdown = AtomicBool::new(false);
        let generation = AtomicU64::new(1);
        run_persistent_handler(&mut handler, &invocation, 1, &shutdown, &generation, 1).unwrap();
        invocation.authority = persistent_authority(&["ui-actions", "navigate"]);
        assert!(
            run_persistent_handler(&mut handler, &invocation, 1, &shutdown, &generation, 1)
                .unwrap_err()
                .contains("descriptor changed")
        );
        assert!(handler.is_none());
    }

    #[test]
    fn persistent_protocol_failure_discards_process_and_allows_restart() {
        let mismatched = persistent_fixture(
            "restart-fixture",
            r#"IFS= read -r event
printf '{"schema_version":1,"request_id":999,"actions":[]}\n'
sleep 10"#,
        );
        let mut handler = None;
        let shutdown = AtomicBool::new(false);
        let generation = AtomicU64::new(1);
        let error = run_persistent_handler(&mut handler, &mismatched, 1, &shutdown, &generation, 1)
            .unwrap_err();
        assert!(error.contains("did not match"));
        assert!(handler.is_none());

        let healthy = persistent_fixture(
            "restart-fixture",
            r#"while IFS= read -r event; do
  printf '{"schema_version":1,"request_id":2,"actions":[]}\n'
done"#,
        );
        assert!(
            run_persistent_handler(&mut handler, &healthy, 2, &shutdown, &generation, 1).is_ok()
        );
        assert!(handler.is_some());
    }

    #[test]
    fn persistent_timeout_and_generation_change_terminate_handlers() {
        let invocation = persistent_fixture("cancel-fixture", "IFS= read -r event\nsleep 10");
        let mut handler = None;
        let shutdown = AtomicBool::new(false);
        let generation = Arc::new(AtomicU64::new(1));
        let update = Arc::clone(&generation);
        let changer = thread::spawn(move || {
            thread::sleep(Duration::from_millis(50));
            update.store(2, Ordering::Release);
        });
        let started = Instant::now();
        let error = run_persistent_handler(&mut handler, &invocation, 1, &shutdown, &generation, 1)
            .unwrap_err();
        changer.join().unwrap();
        assert!(error.contains("cancelled"));
        assert!(started.elapsed() < Duration::from_secs(1));
        assert!(handler.is_none());
    }

    #[test]
    fn persistent_timeout_and_crash_discard_the_failed_process() {
        let shutdown = AtomicBool::new(false);
        let generation = AtomicU64::new(1);

        let mut handler = None;
        let timeout = persistent_fixture("timeout-fixture", "IFS= read -r event\nsleep 10");
        let started = Instant::now();
        let error = run_persistent_handler(&mut handler, &timeout, 1, &shutdown, &generation, 1)
            .unwrap_err();
        assert!(error.contains("exceeded 2000 ms"));
        assert!(started.elapsed() < Duration::from_secs(4));
        assert!(handler.is_none());

        let crash = persistent_fixture("crash-fixture", "printf 'fixture failure\\n' >&2\nexit 7");
        let error =
            run_persistent_handler(&mut handler, &crash, 2, &shutdown, &generation, 1).unwrap_err();
        assert!(error.contains("exit status: 7"), "{error}");
        assert!(error.contains("fixture failure"), "{error}");
        assert!(handler.is_none());

        // A handler that reads its event and then dies mid-request reports
        // the same, rather than only that its stdout closed.
        let mid_request = persistent_fixture(
            "mid-request-crash-fixture",
            "IFS= read -r event\nprintf 'died reading %s\\n' \"${#event}\" >&2\nexit 7",
        );
        let error =
            run_persistent_handler(&mut handler, &mid_request, 3, &shutdown, &generation, 1)
                .unwrap_err();
        assert!(error.contains("exit status: 7"), "{error}");
        assert!(error.contains("died reading"), "{error}");
        assert!(handler.is_none());
    }

    #[test]
    fn handler_stderr_keeps_only_its_first_four_kib() {
        let stderr = Mutex::new(BoundedStderr::default());
        let written = vec![b'e'; MAX_ERROR_BYTES as usize + 1_500];
        drain_persistent_stderr(Cursor::new(written), &stderr);
        let stderr = stderr.into_inner().unwrap();
        assert_eq!(stderr.bytes.len(), MAX_ERROR_BYTES as usize);
        assert!(stderr.truncated);

        let stderr = Mutex::new(BoundedStderr::default());
        drain_persistent_stderr(Cursor::new(vec![b'e'; MAX_ERROR_BYTES as usize]), &stderr);
        let stderr = stderr.into_inner().unwrap();
        assert_eq!(stderr.bytes.len(), MAX_ERROR_BYTES as usize);
        assert!(!stderr.truncated);
    }

    #[test]
    fn handler_requests_are_refused_past_their_limit() {
        // The request ID the worker adds counts toward the bound.
        let event_of = |padding: usize| {
            format!(
                r#"{{"schema_version":1,"event":{{"type":"app-ready","pad":"{}"}}}}"#,
                "x".repeat(padding)
            )
        };
        let request_id = u64::MAX;
        let added = r#","request_id":18446744073709551615"#.len();

        // The largest source envelope with the largest settings document
        // the runtime accepts always fits.
        let document_of = |values: &serde_json::Value| {
            serde_json::json!({"schema_version": 1, "plugin_id": "p", "values": values}).to_string()
        };
        let empty = serde_json::json!({"s": ""});
        let largest_settings = serde_json::json!({
            "s": "x".repeat(MAX_REQUEST_SETTINGS_BYTES - document_of(&empty).len()),
        });
        assert_eq!(
            document_of(&largest_settings).len(),
            MAX_REQUEST_SETTINGS_BYTES
        );
        let largest_source = event_of(MAX_PERSISTENT_SOURCE_EVENT_BYTES - event_of(0).len());
        assert!(
            persistent_request_json(&largest_source, Some(&largest_settings), request_id).is_ok()
        );

        let fits = MAX_STRUCTURED_EVENT_BYTES - added - event_of(0).len();
        let encoded = persistent_request_json(&event_of(fits), None, request_id).unwrap();
        assert_eq!(encoded.len(), MAX_STRUCTURED_EVENT_BYTES);
        let error = persistent_request_json(&event_of(fits + 1), None, request_id).unwrap_err();
        assert!(
            error.contains(&format!("exceeds {MAX_STRUCTURED_EVENT_BYTES} bytes")),
            "{error}"
        );
    }

    /// A completion sink whose completions arrive on the returned receiver.
    fn completion_channel() -> (
        CompletionSink,
        std::sync::mpsc::Receiver<StructuredHandlerCompletion>,
    ) {
        let (sender, receiver) = std::sync::mpsc::channel();
        let deliver: CompletionSink = Arc::new(move |completion| {
            let _ = sender.send(completion);
        });
        (deliver, receiver)
    }

    /// A handler that answers its `n`th request with request ID
    /// `first_request_id + n - 1` and a message naming its process.
    fn counting_handler(plugin_id: &str, first_request_id: u64) -> StructuredHandlerInvocation {
        persistent_fixture(
            plugin_id,
            &format!(
                r#"n={first_request_id}
while IFS= read -r event; do
  printf '{{"schema_version":1,"request_id":%d,"actions":[{{"type":"show-message","message":"pid %d"}}]}}\n' "$n" "$$"
  n=$((n + 1))
done"#
            ),
        )
    }

    fn receive(
        receiver: &std::sync::mpsc::Receiver<StructuredHandlerCompletion>,
    ) -> StructuredHandlerCompletion {
        receiver
            .recv_timeout(Duration::from_secs(5))
            .expect("a completion arrives")
    }

    fn message_of(completion: &StructuredHandlerCompletion) -> String {
        match completion.result.as_deref() {
            Ok([StructuredResponseAction::ShowMessage { message }]) => message.clone(),
            other => panic!("unexpected handler result {other:?}"),
        }
    }

    #[test]
    fn windows_share_one_handler_process_that_outlives_a_closed_window() {
        let host = Arc::new(StructuredPluginHost::default());
        let (deliver, completions) = completion_channel();
        let first_window = WindowRequests::default();
        let second_window = WindowRequests::default();
        let generation = host.generation();

        // Each window holds the host; the first window's handle goes away
        // when it closes.
        let first_handle = Arc::clone(&host);
        let first_request = first_handle
            .submit(
                1,
                &first_window,
                generation,
                counting_handler("shared", 1),
                Arc::clone(&deliver),
            )
            .unwrap();
        let first = receive(&completions);
        assert_eq!(first.window_id, 1);
        assert_eq!(first.request_id, first_request);
        drop(first_handle);
        drop(first_window);

        let second_request = host
            .submit(
                2,
                &second_window,
                generation,
                counting_handler("shared", 1),
                deliver,
            )
            .unwrap();
        assert_eq!(second_request, first_request + 1);
        let second = receive(&completions);
        assert_eq!(second.window_id, 2);
        assert_eq!(message_of(&second), message_of(&first));
    }

    #[test]
    fn a_slow_plugin_does_not_delay_another() {
        let host = StructuredPluginHost::default();
        let window = WindowRequests::default();
        let (deliver, completions) = completion_channel();
        let generation = host.generation();
        let slow = persistent_fixture(
            "slow",
            r#"while IFS= read -r event; do
  sleep 1
  printf '{"schema_version":1,"request_id":1,"actions":[]}\n'
done"#,
        );
        host.submit(1, &window, generation, slow, Arc::clone(&deliver))
            .unwrap();
        host.submit(1, &window, generation, counting_handler("fast", 2), deliver)
            .unwrap();
        let first = receive(&completions);
        assert_eq!(first.plugin_id, "fast");
        assert!(first.result.is_ok());
        let second = receive(&completions);
        assert_eq!(second.plugin_id, "slow");
        assert!(second.result.is_ok(), "{:?}", second.result);
    }

    #[test]
    fn a_new_generation_stops_handlers_and_refuses_older_requests() {
        let host = StructuredPluginHost::default();
        let window = WindowRequests::default();
        let (deliver, completions) = completion_channel();
        let old_generation = host.generation();
        host.submit(
            1,
            &window,
            old_generation,
            counting_handler("reloaded", 1),
            Arc::clone(&deliver),
        )
        .unwrap();
        let before = message_of(&receive(&completions));
        let old_pid = before.strip_prefix("pid ").unwrap().to_string();

        let new_generation = host.advance_generation();
        assert_ne!(new_generation, old_generation);
        assert!(
            host.submit(
                2,
                &window,
                old_generation,
                counting_handler("reloaded", 2),
                Arc::clone(&deliver),
            )
            .unwrap_err()
            .contains("no longer active")
        );
        assert!(crate::test_support::poll_until(
            Duration::from_secs(3),
            || !std::path::Path::new(&format!("/proc/{old_pid}")).exists()
        ));
        host.submit(
            2,
            &window,
            new_generation,
            counting_handler("reloaded", 2),
            deliver,
        )
        .unwrap();
        assert_ne!(message_of(&receive(&completions)), before);
    }

    #[test]
    fn plugin_lanes_are_bounded_and_idle_ones_give_way() {
        let host = StructuredPluginHost::default();
        let window = WindowRequests::default();
        let (deliver, completions) = completion_channel();
        let generation = host.generation();
        for index in 1..=MAX_PERSISTENT_HANDLERS as u64 {
            host.submit(
                1,
                &window,
                generation,
                counting_handler(&format!("running-{index}"), index),
                Arc::clone(&deliver),
            )
            .unwrap();
            assert!(receive(&completions).result.is_ok());
        }
        // Every lane keeps a running process, so none gives way.
        assert!(
            host.submit(
                1,
                &window,
                generation,
                counting_handler("overflow", 99),
                Arc::clone(&deliver),
            )
            .unwrap_err()
            .contains("limit reached")
        );

        // Lanes whose processes ended and that have nothing queued do.
        let host = StructuredPluginHost::default();
        let generation = host.generation();
        for index in 1..=MAX_PERSISTENT_HANDLERS {
            host.submit(
                1,
                &window,
                generation,
                persistent_fixture(&format!("exiting-{index}"), "exit 0"),
                Arc::clone(&deliver),
            )
            .unwrap();
            assert!(receive(&completions).result.is_err());
        }
        let mut replacement = None;
        assert!(crate::test_support::poll_until(
            Duration::from_secs(3),
            || {
                replacement = host
                    .submit(
                        1,
                        &window,
                        generation,
                        counting_handler("replacement", 9),
                        Arc::clone(&deliver),
                    )
                    .ok();
                replacement.is_some()
            }
        ));
        assert_eq!(receive(&completions).plugin_id, "replacement");
    }

    #[test]
    fn a_closed_windows_queued_requests_are_answered_without_running() {
        let temp = tempfile::tempdir().unwrap();
        let log = temp.path().join("requests");
        let release = temp.path().join("release");
        // Each request is logged; the first waits to be released, so the
        // second is still queued when its window closes.
        let handler = persistent_fixture(
            "logged",
            &format!(
                r#"n=1
while IFS= read -r event; do
  printf '%s\n' "$n" >> '{log}'
  while [ "$n" = 1 ] && [ ! -e '{release}' ]; do sleep 0.01; done
  printf '{{"schema_version":1,"request_id":%d,"actions":[]}}\n' "$n"
  n=$((n + 1))
done"#,
                log = log.display(),
                release = release.display(),
            ),
        );
        let host = StructuredPluginHost::default();
        let (deliver, completions) = completion_channel();
        let generation = host.generation();
        let staying = WindowRequests::default();
        let closing = WindowRequests::default();
        let first = host
            .submit(
                1,
                &staying,
                generation,
                handler.clone(),
                Arc::clone(&deliver),
            )
            .unwrap();
        let queued = host
            .submit(2, &closing, generation, handler, deliver)
            .unwrap();
        drop(closing);
        std::fs::write(&release, "").unwrap();

        let answered = receive(&completions);
        assert_eq!(answered.request_id, first);
        assert!(answered.result.is_ok(), "{:?}", answered.result);
        let refused = receive(&completions);
        assert_eq!(refused.request_id, queued);
        assert_eq!(refused.window_id, 2);
        assert!(
            refused
                .result
                .as_ref()
                .unwrap_err()
                .contains("window closed"),
            "{:?}",
            refused.result
        );
        assert_eq!(std::fs::read_to_string(&log).unwrap(), "1\n");
    }

    #[test]
    fn a_new_generation_answers_the_requests_it_cancels() {
        let host = StructuredPluginHost::default();
        let (deliver, completions) = completion_channel();
        let generation = host.generation();
        let window = WindowRequests::default();
        // A handler that never answers holds the lane while more queue up.
        let silent = persistent_fixture("silent", "while IFS= read -r event; do :; done");
        let running = host
            .submit(1, &window, generation, silent.clone(), Arc::clone(&deliver))
            .unwrap();
        let queued: Vec<_> = (0..3)
            .map(|_| {
                host.submit(1, &window, generation, silent.clone(), Arc::clone(&deliver))
                    .unwrap()
            })
            .collect();
        host.advance_generation();

        let mut answered: Vec<_> = (0..4).map(|_| receive(&completions)).collect();
        answered.sort_by_key(|completion| completion.request_id);
        assert_eq!(answered[0].request_id, running);
        assert!(answered[0].result.is_err());
        for (completion, request_id) in answered[1..].iter().zip(queued) {
            assert_eq!(completion.request_id, request_id);
            assert_eq!(completion.plugin_generation, generation);
            assert!(
                completion.result.as_ref().unwrap_err().contains("reloaded"),
                "{:?}",
                completion.result
            );
        }
        assert!(
            completions
                .recv_timeout(Duration::from_millis(100))
                .is_err(),
            "each request is answered once"
        );
    }

    #[test]
    fn agent_changes_start_one_generation_however_many_windows_notice() {
        let host = StructuredPluginHost::default();
        let first = host.generation();
        let mut agents = BTreeMap::new();
        agents.insert(
            "codex".to_string(),
            AgentConfig {
                enabled: Some(false),
                ..AgentConfig::default()
            },
        );
        assert!(host.advance_generation_for_agents(&agents));
        let second = host.generation();
        assert_ne!(second, first);
        assert!(!host.advance_generation_for_agents(&agents));
        assert_eq!(host.generation(), second);

        // A registry reload in between leaves the agents as they were.
        host.advance_generation();
        let third = host.generation();
        assert!(!host.advance_generation_for_agents(&agents));
        assert_eq!(host.generation(), third);

        assert!(host.advance_generation_for_agents(&BTreeMap::new()));
        assert_ne!(host.generation(), third);
    }

    #[test]
    fn retired_lanes_are_let_go_once_finished() {
        let host = StructuredPluginHost::default();
        let finished = thread::spawn(|| {});
        assert!(crate::test_support::poll_until(
            Duration::from_secs(3),
            || finished.is_finished()
        ));
        let (stop, stopped) = std::sync::mpsc::channel::<()>();
        let waiting = thread::spawn(move || {
            let _ = stopped.recv();
        });
        host.retire([finished]);
        host.retire([waiting]);
        assert_eq!(host.retired.lock().unwrap().len(), 1);

        drop(stop);
        assert!(crate::test_support::poll_until(
            Duration::from_secs(3),
            || host.retired.lock().unwrap()[0].is_finished()
        ));
        host.retire([]);
        assert!(host.retired.lock().unwrap().is_empty());
    }

    #[test]
    fn errors_are_control_free_and_bounded() {
        let error = bounded_error(&format!("bad\n\u{1b}{}", "x".repeat(2_000)));
        assert!(error.len() <= 1024);
        assert!(!error.chars().any(char::is_control));
    }

    /// Start a conversation with a checked-in example's handler, granted
    /// everything its manifest requests.
    fn example_invocation(example: &str, command_invoked: bool) -> StructuredHandlerInvocation {
        let directory = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../examples/plugins")
            .join(example);
        let (manifest, _, _) =
            crate::plugin::runtime::PluginManifest::from_directory(&directory).unwrap();
        let handler = manifest.event_handler.unwrap();
        StructuredHandlerInvocation {
            plugin_id: manifest.id,
            program: handler.program,
            args: handler.args,
            event_json: String::new(),
            settings: None,
            authority: HandlerAuthority::for_event(manifest.permissions, command_invoked),
        }
    }

    #[test]
    fn rerun_in_sibling_example_sends_the_last_command_to_the_next_pane() {
        let mut handler = None;
        let shutdown = AtomicBool::new(false);
        let generation = AtomicU64::new(1);

        // A completed command in pane 0 of tab 5 is remembered, not acted on.
        let mut completed = example_invocation("rerun-in-sibling", false);
        completed.event_json = r#"{"schema_version":1,"plugin_id":"rerun-in-sibling","origin_window_id":3,"event":{"type":"command-completed","region_id":1,"tab_id":5,"pane_id":0,"exit_code":0,"content_available":true,"command":{"text":"cargo test","truncated":false,"sanitized":false},"output":{"text":"ok","truncated":false,"sanitized":false}}}"#.into();
        assert!(
            run_persistent_handler(&mut handler, &completed, 1, &shutdown, &generation, 1)
                .unwrap()
                .is_empty()
        );

        // The same handler hears other windows, whose tab IDs repeat; their
        // commands stay theirs.
        let mut elsewhere = example_invocation("rerun-in-sibling", false);
        elsewhere.event_json = r#"{"schema_version":1,"plugin_id":"rerun-in-sibling","origin_window_id":4,"event":{"type":"command-completed","region_id":1,"tab_id":5,"pane_id":0,"exit_code":0,"content_available":true,"command":{"text":"make clean","truncated":false,"sanitized":false},"output":{"text":"","truncated":false,"sanitized":false}}}"#.into();
        assert!(
            run_persistent_handler(&mut handler, &elsewhere, 10, &shutdown, &generation, 1)
                .unwrap()
                .is_empty()
        );

        let mut invocation = example_invocation("rerun-in-sibling", true);
        invocation.event_json = r#"{"schema_version":1,"plugin_id":"rerun-in-sibling","origin_window_id":3,"event":{"type":"command-invoked","command_id":"rerun-in-sibling.rerun"}}"#.into();
        let query = run_persistent_handler(&mut handler, &invocation, 2, &shutdown, &generation, 1)
            .unwrap();
        assert_eq!(
            query,
            vec![StructuredResponseAction::QueryWorkspace {
                query_id: "rerun".into(),
            }]
        );

        invocation.authority = invocation.authority.after(&query[0]);
        invocation.event_json = r#"{"schema_version":1,"plugin_id":"rerun-in-sibling","origin_window_id":3,"event":{"type":"workspace-query-result","query_id":"rerun","truncated":false,"windows":[{"id":3,"tabs":[{"id":4,"title":"other","active":false,"closed":false,"panes":[]},{"id":5,"title":"work","active":true,"closed":false,"panes":[{"id":0,"focused":true,"closed":false},{"id":1,"focused":false,"closed":false}]}]}]}}"#.into();
        let send = run_persistent_handler(&mut handler, &invocation, 3, &shutdown, &generation, 1)
            .unwrap();
        assert_eq!(
            send,
            vec![StructuredResponseAction::SendText {
                action_id: "rerun".into(),
                window_id: None,
                tab_id: 5,
                pane_id: 1,
                text: "cargo test".into(),
                paste: false,
                enter: true,
            }]
        );

        invocation.authority = invocation.authority.after(&send[0]);
        invocation.event_json = r#"{"schema_version":1,"plugin_id":"rerun-in-sibling","origin_window_id":3,"event":{"type":"action-result","action":"send-text","action_id":"rerun","ok":false,"error":"External terminal input is disabled"}}"#.into();
        assert_eq!(
            run_persistent_handler(&mut handler, &invocation, 4, &shutdown, &generation, 1)
                .unwrap(),
            vec![StructuredResponseAction::ShowMessage {
                message: "Rerun failed: External terminal input is disabled".into(),
            }]
        );
        assert!(handler.is_some());
    }

    #[test]
    fn command_notifier_example_reports_only_unexpected_failures() {
        let mut handler = None;
        let shutdown = AtomicBool::new(false);
        let generation = AtomicU64::new(1);
        let mut invocation = example_invocation("command-notifier", false);
        let completed = |exit_code: i32| {
            format!(
                r#"{{"schema_version":1,"plugin_id":"command-notifier","origin_window_id":1,"settings":{{"output-lines":2,"ignored-exit-codes":"130"}},"event":{{"type":"command-completed","region_id":1,"tab_id":5,"pane_id":null,"exit_code":{exit_code},"content_available":true,"command":{{"text":"make\u001b build","truncated":false,"sanitized":true}},"output":{{"text":"one\ntwo\n\nthree\n","truncated":false,"sanitized":false}}}}}}"#
            )
        };
        for (request_id, exit_code) in [(1, 0), (2, 130)] {
            invocation.event_json = completed(exit_code);
            assert!(
                run_persistent_handler(
                    &mut handler,
                    &invocation,
                    request_id,
                    &shutdown,
                    &generation,
                    1
                )
                .unwrap()
                .is_empty()
            );
        }
        invocation.event_json = completed(2);
        assert_eq!(
            run_persistent_handler(&mut handler, &invocation, 3, &shutdown, &generation, 1)
                .unwrap(),
            vec![StructuredResponseAction::ShowNotice {
                title: "Command failed".into(),
                lines: vec![
                    "Command: make  build".into(),
                    "Exit code: 2".into(),
                    "Output:".into(),
                    "  two".into(),
                    "  three".into(),
                ],
            }]
        );
    }

    /// A persistent-handler conversation with a checked-in example, granted
    /// everything its manifest requests. Like the host, it sends a command's
    /// settings with the command and with every result that follows it.
    struct ExampleConversation {
        handler: Option<PersistentHandler>,
        shutdown: AtomicBool,
        generation: AtomicU64,
        invocation: StructuredHandlerInvocation,
        origin_window_id: u64,
        request_id: u64,
    }

    impl ExampleConversation {
        fn new(example: &str, origin_window_id: u64) -> Self {
            Self {
                handler: None,
                shutdown: AtomicBool::new(false),
                generation: AtomicU64::new(1),
                invocation: example_invocation(example, true),
                origin_window_id,
                request_id: 0,
            }
        }

        /// Invoke one of the example's palette commands.
        fn command(
            &mut self,
            command: &str,
            settings: Option<serde_json::Value>,
        ) -> Vec<StructuredResponseAction> {
            self.invocation.authority =
                HandlerAuthority::for_event(self.invocation.authority.grants().clone(), true);
            self.invocation.settings = settings;
            let plugin_id = self.invocation.plugin_id.clone();
            let envelope = self.envelope(serde_json::json!({
                "type": "command-invoked",
                "command_id": format!("{plugin_id}.{command}"),
            }));
            self.send(envelope)
        }

        /// Deliver the result of `operation`, the handler's previous request.
        fn result(
            &mut self,
            operation: &StructuredResponseAction,
            event: serde_json::Value,
        ) -> Vec<StructuredResponseAction> {
            self.invocation.authority = self.invocation.authority.after(operation);
            let envelope = self.envelope(event);
            self.send(envelope)
        }

        /// Invoke a command whose handler first queries the workspace under
        /// the command's name, answer with `windows`, and return the
        /// handler's follow-up.
        fn command_with_workspace(
            &mut self,
            command: &str,
            settings: Option<serde_json::Value>,
            windows: serde_json::Value,
        ) -> Vec<StructuredResponseAction> {
            assert_eq!(self.command(command, settings), vec![query(command)]);
            self.result(
                &query(command),
                serde_json::json!({
                    "type": "workspace-query-result",
                    "query_id": command,
                    "truncated": false,
                    "windows": windows,
                }),
            )
        }

        fn envelope(&self, event: serde_json::Value) -> serde_json::Value {
            serde_json::json!({
                "schema_version": SCHEMA_VERSION,
                "plugin_id": self.invocation.plugin_id,
                "origin_window_id": self.origin_window_id,
                "event": event,
            })
        }

        fn send(&mut self, envelope: serde_json::Value) -> Vec<StructuredResponseAction> {
            self.invocation.event_json = envelope.to_string();
            self.request_id += 1;
            let actions = run_persistent_handler(
                &mut self.handler,
                &self.invocation,
                self.request_id,
                &self.shutdown,
                &self.generation,
                1,
            )
            .unwrap();
            assert!(self.handler.is_some(), "the handler must stay persistent");
            actions
        }
    }

    fn query(query_id: &str) -> StructuredResponseAction {
        StructuredResponseAction::QueryWorkspace {
            query_id: query_id.into(),
        }
    }

    fn message(message: &str) -> StructuredResponseAction {
        StructuredResponseAction::ShowMessage {
            message: message.into(),
        }
    }

    #[test]
    fn window_switcher_example_jumps_to_a_tab_by_title_in_any_window() {
        let tab = |id: u64, title: &str, active: bool| {
            serde_json::json!({
                "id": id,
                "title": title,
                "active": active,
                "closed": false,
                "panes": [{"id": 0, "focused": true, "closed": false}],
            })
        };
        let windows = |logs_active: bool| {
            serde_json::json!([
                {"id": 3, "tabs": [tab(1, "shell", true)]},
                {"id": 4, "tabs": [tab(2, "editor", !logs_active), tab(3, "App Logs", logs_active)]},
                {"id": 5, "tabs": [tab(6, "build", true)]},
            ])
        };
        let settings = || Some(serde_json::json!({"tab-title": "logs"}));
        let mut conversation = ExampleConversation::new("window-switcher", 3);

        // Jump to Tab reads the title setting from the workspace query
        // result, which carries the command's settings, and selects the
        // matching tab in its own window.
        let select = StructuredResponseAction::SelectTab {
            action_id: "jump".into(),
            window_id: Some(4),
            tab_id: 3,
        };
        assert_eq!(
            conversation.command_with_workspace("jump-to-tab", settings(), windows(false)),
            vec![
                select.clone(),
                message(
                    "Tab selected; run Jump to Tab again to bring its window forward (X11 only)"
                ),
            ]
        );
        assert!(
            conversation
                .result(
                    &select,
                    serde_json::json!({
                        "type": "action-result",
                        "action": "select-tab",
                        "action_id": "jump",
                        "window_id": 4,
                        "tab_id": 3,
                        "ok": true,
                        "changed": true,
                    }),
                )
                .is_empty()
        );

        // Once the tab is selected, running it again focuses its window, and
        // a refused focus request, as on Wayland, is reported.
        let focus = StructuredResponseAction::FocusWindow {
            action_id: "jump".into(),
            window_id: 4,
        };
        assert_eq!(
            conversation.command_with_workspace("jump-to-tab", settings(), windows(true)),
            vec![focus.clone()]
        );
        let error = "Termeric can focus another window on Wayland only when it is already focused";
        assert_eq!(
            conversation.result(
                &focus,
                serde_json::json!({
                    "type": "action-result",
                    "action": "focus-window",
                    "action_id": "jump",
                    "window_id": 4,
                    "ok": false,
                    "error": error,
                    "error_truncated": false,
                }),
            ),
            vec![message(&format!("Window switch failed: {error}"))]
        );

        // A title nothing matches is reported.
        let missing = Some(serde_json::json!({"tab-title": "nowhere"}));
        assert_eq!(
            conversation.command_with_workspace("jump-to-tab", missing, windows(false)),
            vec![message("No tab title contains 'nowhere'")]
        );
    }

    #[test]
    fn pane_layouts_example_arranges_the_origin_windows_active_tab() {
        use crate::panes::SplitDirection;
        // Window 3 is the origin; its active tab 5 holds `pane_count` panes,
        // the last one focused. Window 2's active tab must be ignored.
        let windows = |pane_count: u32| {
            let panes: Vec<_> = (0..pane_count)
                .map(|id| serde_json::json!({"id": id, "focused": id + 1 == pane_count, "closed": false}))
                .collect();
            let single = serde_json::json!([{"id": 0, "focused": true, "closed": false}]);
            serde_json::json!([
                {"id": 2, "tabs": [{"id": 9, "title": "other", "active": true, "closed": false, "panes": single}]},
                {"id": 3, "tabs": [
                    {"id": 4, "title": "idle", "active": false, "closed": false, "panes": single},
                    {"id": 5, "title": "work", "active": true, "closed": false, "panes": panes},
                ]},
            ])
        };
        let mut conversation = ExampleConversation::new("pane-layouts", 3);

        // Dev Layout adds a pane to the right of one pane and below the
        // focused one of two, and arranges three or more as a tall main pane
        // beside a stack. A failed change is reported.
        let create = |pane_id, direction| StructuredResponseAction::CreatePane {
            action_id: "dev-layout".into(),
            window_id: None,
            tab_id: 5,
            pane_id,
            direction,
        };
        assert_eq!(
            conversation.command_with_workspace("dev-layout", None, windows(1)),
            vec![create(0, SplitDirection::Vertical)]
        );
        let split = create(1, SplitDirection::Horizontal);
        assert_eq!(
            conversation.command_with_workspace("dev-layout", None, windows(2)),
            vec![split.clone()]
        );
        assert_eq!(
            conversation.result(
                &split,
                serde_json::json!({
                    "type": "action-result",
                    "action": "create-pane",
                    "action_id": "dev-layout",
                    "window_id": 3,
                    "tab_id": 5,
                    "pane_id": 1,
                    "direction": "horizontal",
                    "ok": false,
                    "error": "Tab 5 has reached its pane limit",
                    "error_truncated": false,
                }),
            ),
            vec![message(
                "Layout unchanged: Tab 5 has reached its pane limit"
            )]
        );
        // Three or more panes become a tall main pane beside a stack; a
        // successful result is quiet.
        let tall = StructuredResponseAction::ApplyPaneLayout {
            action_id: "dev-layout".into(),
            window_id: None,
            tab_id: 5,
            preset: PaneLayoutPreset::Tall,
        };
        assert_eq!(
            conversation.command_with_workspace("dev-layout", None, windows(3)),
            vec![tall.clone()]
        );
        assert!(
            conversation
                .result(
                    &tall,
                    serde_json::json!({
                        "type": "action-result",
                        "action": "apply-pane-layout",
                        "action_id": "dev-layout",
                        "window_id": 3,
                        "tab_id": 5,
                        "preset": "tall",
                        "ok": true,
                        "changed": true,
                    }),
                )
                .is_empty()
        );

        // A workspace without the origin window has nothing to arrange.
        assert_eq!(
            conversation.command_with_workspace("dev-layout", None, serde_json::json!([])),
            vec![message("No active tab to arrange")]
        );
    }
}
