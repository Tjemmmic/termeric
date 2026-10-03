//! Bounded out-of-process execution for declarative plugin event handlers.
//!
//! Each plugin's handler is one long-lived process that reads one schema-v1
//! JSON request per line and answers each with one JSON line echoing the
//! request's `request_id`. Each response is checked against a
//! [`HandlerAuthority`]: the plugin's granted capabilities plus where the
//! response sits in its bounded follow-up chain.

use super::permission::{Capability, PermissionScope, PluginPermissions};
use crate::panes::PaneLayoutPreset;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{RecvTimeoutError, SyncSender, TrySendError, sync_channel};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};
use winit::event_loop::EventLoopProxy;
use winit::window::WindowId;

use std::os::unix::process::CommandExt;

const QUEUE_CAPACITY: usize = 32;
const MAX_PERSISTENT_HANDLERS: usize = 8;
pub(crate) const MAX_STRUCTURED_EVENT_BYTES: usize = 16 * 1024;
pub(crate) const MAX_PERSISTENT_SOURCE_EVENT_BYTES: usize =
    MAX_STRUCTURED_EVENT_BYTES - PERSISTENT_PROTOCOL_RESERVE_BYTES;
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
    pub window_id: WindowId,
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

#[derive(Debug)]
struct WorkItem {
    window_id: WindowId,
    plugin_generation: u64,
    request_id: u64,
    invocation: StructuredHandlerInvocation,
    proxy: EventLoopProxy<crate::app::AppEvent>,
}

#[derive(Debug)]
enum WorkerMessage {
    Run(WorkItem),
    Shutdown,
}

#[derive(Debug)]
pub struct StructuredPluginWorker {
    sender: SyncSender<WorkerMessage>,
    shutdown: Arc<AtomicBool>,
    active_generation: Arc<AtomicU64>,
    thread: Option<JoinHandle<()>>,
    next_request_id: u64,
}

impl StructuredPluginWorker {
    pub fn new() -> Result<Self, String> {
        let (sender, receiver) = sync_channel(QUEUE_CAPACITY);
        let shutdown = Arc::new(AtomicBool::new(false));
        let worker_shutdown = Arc::clone(&shutdown);
        let active_generation = Arc::new(AtomicU64::new(1));
        let worker_generation = Arc::clone(&active_generation);
        let thread = thread::Builder::new()
            .name("termeric-plugin-runtime".into())
            .spawn(move || {
                let mut persistent_handlers: HashMap<String, PersistentHandler> = HashMap::new();
                let mut owned_generation = worker_generation.load(Ordering::Acquire);
                let mut last_persistent_reap = Instant::now();
                loop {
                    let current_generation = worker_generation.load(Ordering::Acquire);
                    if current_generation != owned_generation {
                        persistent_handlers.clear();
                        owned_generation = current_generation;
                    }
                    if last_persistent_reap.elapsed() >= PERSISTENT_REAP_INTERVAL {
                        persistent_handlers.retain(|_, handler| handler.is_running());
                        last_persistent_reap = Instant::now();
                    }
                    let message = match receiver.recv_timeout(WORKER_POLL_INTERVAL) {
                        Ok(message) => message,
                        Err(RecvTimeoutError::Timeout) => {
                            if worker_shutdown.load(Ordering::Acquire) {
                                break;
                            }
                            continue;
                        }
                        Err(RecvTimeoutError::Disconnected) => break,
                    };
                    // The generation may change while recv_timeout is blocked.
                    // Fence again before dispatch so the first new-generation
                    // request can never reuse an old-generation process.
                    let current_generation = worker_generation.load(Ordering::Acquire);
                    if current_generation != owned_generation {
                        persistent_handlers.clear();
                        owned_generation = current_generation;
                    }
                    match message {
                        WorkerMessage::Shutdown => break,
                        WorkerMessage::Run(item) => {
                            if worker_shutdown.load(Ordering::Acquire) {
                                break;
                            }
                            if worker_generation.load(Ordering::Acquire) != item.plugin_generation {
                                continue;
                            }
                            let result = run_persistent_handler(
                                &mut persistent_handlers,
                                &item.invocation,
                                item.request_id,
                                &worker_shutdown,
                                &worker_generation,
                                item.plugin_generation,
                            );
                            let plugin_id = item.invocation.plugin_id.clone();
                            let _ = item.proxy.send_event(
                                crate::app::AppEvent::StructuredPluginCompleted(
                                    StructuredHandlerCompletion {
                                        window_id: item.window_id,
                                        plugin_generation: item.plugin_generation,
                                        plugin_id,
                                        request_id: item.request_id,
                                        continuation: item.invocation,
                                        result,
                                    },
                                ),
                            );
                        }
                    }
                }
                persistent_handlers.clear();
            })
            .map_err(|error| format!("structured plugin worker could not start: {error}"))?;
        Ok(Self {
            sender,
            shutdown,
            active_generation,
            thread: Some(thread),
            next_request_id: 1,
        })
    }

    pub fn submit(
        &mut self,
        window_id: WindowId,
        plugin_generation: u64,
        invocation: StructuredHandlerInvocation,
        proxy: EventLoopProxy<crate::app::AppEvent>,
    ) -> Result<u64, String> {
        if self.active_generation.load(Ordering::Acquire) != plugin_generation {
            return Err("structured plugin generation is no longer active".into());
        }
        let request_id = self.next_request_id;
        self.next_request_id = self.next_request_id.wrapping_add(1).max(1);
        let item = WorkItem {
            window_id,
            plugin_generation,
            request_id,
            invocation,
            proxy,
        };
        match self.sender.try_send(WorkerMessage::Run(item)) {
            Ok(()) => Ok(request_id),
            Err(TrySendError::Full(_)) => Err(format!(
                "structured plugin queue is full ({QUEUE_CAPACITY} pending requests)"
            )),
            Err(TrySendError::Disconnected(_)) => {
                Err("structured plugin worker is unavailable".into())
            }
        }
    }

    /// Cancel active and queued work from every earlier plugin registry
    /// generation. The worker observes this during its 10 ms process poll.
    pub fn set_generation(&self, generation: u64) {
        self.active_generation.store(generation, Ordering::Release);
    }
}

impl Drop for StructuredPluginWorker {
    fn drop(&mut self) {
        self.shutdown.store(true, Ordering::Release);
        let _ = self.sender.try_send(WorkerMessage::Shutdown);
        if let Some(thread) = self.thread.take() {
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
            return Err("persistent handler stdin was unavailable".into());
        };
        let Some(stdout) = child.stdout.take() else {
            terminate_handler(&mut child);
            return Err("persistent handler stdout was unavailable".into());
        };
        let Some(stderr_stream) = child.stderr.take() else {
            terminate_handler(&mut child);
            return Err("persistent handler stderr was unavailable".into());
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
                        read_bounded_line(&mut stdout, MAX_OUTPUT_BYTES as usize)
                            .map_err(|error| {
                                bounded_error(&format!("handler response read failed: {error}"))
                            })?
                            .ok_or_else(|| "persistent handler closed stdout".to_string())
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
        if let Some(status) = self
            .child
            .try_wait()
            .map_err(|error| bounded_error(&format!("handler wait failed: {error}")))?
        {
            return Err(self.exit_error(status));
        }
        let (response_sender, response_receiver) = sync_channel(1);
        self.requests
            .as_ref()
            .ok_or_else(|| "persistent handler input is closed".to_string())?
            .try_send(PersistentRequest {
                event,
                response: response_sender,
            })
            .map_err(|error| match error {
                TrySendError::Full(_) => {
                    String::from("persistent handler already has a pending request")
                }
                TrySendError::Disconnected(_) => String::from("persistent handler input is closed"),
            })?;
        let started = Instant::now();
        let (bytes, truncated) = loop {
            if shutdown.load(Ordering::Acquire)
                || active_generation.load(Ordering::Acquire) != expected_generation
            {
                return Err("structured plugin handler was cancelled".into());
            }
            match response_receiver.recv_timeout(WORKER_POLL_INTERVAL) {
                Ok(result) => break result?,
                Err(RecvTimeoutError::Timeout) if started.elapsed() < HANDLER_TIMEOUT => {}
                Err(RecvTimeoutError::Timeout) => {
                    return Err(format!(
                        "structured plugin handler exceeded {} ms",
                        HANDLER_TIMEOUT.as_millis()
                    ));
                }
                Err(RecvTimeoutError::Disconnected) => {
                    let status = self.child.try_wait().ok().flatten();
                    return Err(status.map_or_else(
                        || "persistent handler response channel closed".into(),
                        |status| self.exit_error(status),
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
            return Err(self.exit_error(status));
        }
        Ok(actions)
    }

    fn exit_error(&self, status: std::process::ExitStatus) -> String {
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

fn run_persistent_handler(
    handlers: &mut HashMap<String, PersistentHandler>,
    invocation: &StructuredHandlerInvocation,
    request_id: u64,
    shutdown: &AtomicBool,
    active_generation: &AtomicU64,
    expected_generation: u64,
) -> Result<Vec<StructuredResponseAction>, String> {
    let event = persistent_request_json(&invocation.event_json, request_id)?;
    handlers.retain(|_, handler| handler.is_running());
    if !handlers.contains_key(&invocation.plugin_id) {
        if handlers.len() >= MAX_PERSISTENT_HANDLERS {
            return Err(format!(
                "persistent plugin handler limit reached ({MAX_PERSISTENT_HANDLERS})"
            ));
        }
        handlers.insert(
            invocation.plugin_id.clone(),
            PersistentHandler::spawn(invocation)?,
        );
    }
    let result = {
        let handler = handlers
            .get_mut(&invocation.plugin_id)
            .expect("persistent handler was inserted");
        if !handler.matches(invocation) {
            Err("persistent handler descriptor changed within one plugin generation".into())
        } else {
            handler.request(
                event,
                request_id,
                &invocation.authority,
                shutdown,
                active_generation,
                expected_generation,
            )
        }
    };
    if result.is_err() {
        handlers.remove(&invocation.plugin_id);
    }
    result
}

/// Add the correlation ID to a schema-v1 source event for a persistent
/// handler. The schema version is unchanged: `request_id` is part of v1.
fn persistent_request_json(event_json: &str, request_id: u64) -> Result<Vec<u8>, String> {
    let mut value: serde_json::Value = serde_json::from_str(event_json)
        .map_err(|error| bounded_error(&format!("invalid structured event JSON: {error}")))?;
    let object = value
        .as_object_mut()
        .ok_or_else(|| "persistent structured event must be a JSON object".to_string())?;
    if object
        .get("schema_version")
        .and_then(serde_json::Value::as_u64)
        != Some(u64::from(SCHEMA_VERSION))
    {
        return Err(format!(
            "persistent handlers require a schema-v{SCHEMA_VERSION} source event"
        ));
    }
    object.insert("request_id".into(), serde_json::json!(request_id));
    let bytes = serde_json::to_vec(&value)
        .map_err(|error| bounded_error(&format!("could not encode persistent event: {error}")))?;
    if bytes.len() > MAX_STRUCTURED_EVENT_BYTES {
        return Err(format!(
            "persistent structured event exceeds {MAX_STRUCTURED_EVENT_BYTES} bytes"
        ));
    }
    Ok(bytes)
}

fn read_bounded_line(
    reader: &mut impl BufRead,
    max_bytes: usize,
) -> std::io::Result<Option<(Vec<u8>, bool)>> {
    let mut bytes = Vec::with_capacity(max_bytes.min(4096));
    let mut truncated = false;
    loop {
        let available = reader.fill_buf()?;
        if available.is_empty() {
            if bytes.is_empty() {
                return Ok(None);
            }
            return Err(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "persistent handler response ended without a newline",
            ));
        }
        let newline = available.iter().position(|byte| *byte == b'\n');
        let consumed = newline.map_or(available.len(), |index| index + 1);
        let content = newline.map_or(available, |index| &available[..index]);
        let remaining = max_bytes.saturating_sub(bytes.len());
        let retained = content.len().min(remaining);
        bytes.extend_from_slice(&content[..retained]);
        truncated |= retained < content.len();
        reader.consume(consumed);
        if newline.is_some() {
            if bytes.last() == Some(&b'\r') {
                bytes.pop();
            }
            return Ok(Some((bytes, truncated)));
        }
    }
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
        let mut handlers = HashMap::new();
        let shutdown = AtomicBool::new(false);
        let generation = AtomicU64::new(1);
        let query =
            run_persistent_handler(&mut handlers, &invocation, 1, &shutdown, &generation, 1)
                .unwrap();
        assert!(matches!(
            query[..],
            [StructuredResponseAction::QueryWorkspace { .. }]
        ));

        invocation.event_json = r#"{"schema_version":1,"origin_window_id":4,"event":{"type":"workspace-query-result","query_id":"inventory","windows":[]}}"#.into();
        invocation.authority = invocation.authority.after(&query[0]);
        let action =
            run_persistent_handler(&mut handlers, &invocation, 2, &shutdown, &generation, 1)
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
            run_persistent_handler(&mut handlers, &invocation, 3, &shutdown, &generation, 1)
                .unwrap(),
            vec![StructuredResponseAction::ShowMessage {
                message: "action-result-received".into(),
            }]
        );
        assert_eq!(handlers.len(), 1);
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
        let mut handlers = HashMap::new();
        let shutdown = AtomicBool::new(false);
        let generation = AtomicU64::new(1);
        let created =
            run_persistent_handler(&mut handlers, &invocation, 1, &shutdown, &generation, 1)
                .unwrap();
        invocation.event_json = r#"{"schema_version":1,"event":{"type":"action-result","action":"create-pane","ok":true}}"#.into();
        invocation.authority = invocation.authority.after(&created[0]);
        let error =
            run_persistent_handler(&mut handlers, &invocation, 2, &shutdown, &generation, 1)
                .unwrap_err();
        assert!(error.contains("cannot start host operations"), "{error}");
        // A protocol violation discards the process; the next event restarts it.
        assert!(handlers.is_empty());
    }

    #[test]
    fn persistent_protocol_requires_object_source_and_keeps_one_schema() {
        let request =
            persistent_request_json(r#"{"schema_version":1,"event":{"type":"app-ready"}}"#, 42)
                .unwrap();
        let value: serde_json::Value = serde_json::from_slice(&request).unwrap();
        assert_eq!(value["schema_version"], SCHEMA_VERSION);
        assert_eq!(value["request_id"], 42);
        assert!(persistent_request_json("[]", 1).is_err());
        assert!(persistent_request_json(r#"{"schema_version":2}"#, 1).is_err());
    }

    #[test]
    fn persistent_line_reader_bounds_and_drains_each_frame() {
        let oversized = format!("{}\nnext\n", "x".repeat(MAX_OUTPUT_BYTES as usize + 17));
        let mut reader = Cursor::new(oversized.into_bytes());
        let (first, truncated) = read_bounded_line(&mut reader, MAX_OUTPUT_BYTES as usize)
            .unwrap()
            .unwrap();
        assert_eq!(first.len(), MAX_OUTPUT_BYTES as usize);
        assert!(truncated);
        assert_eq!(
            read_bounded_line(&mut reader, MAX_OUTPUT_BYTES as usize)
                .unwrap()
                .unwrap(),
            (b"next".to_vec(), false)
        );
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
        let mut handlers = HashMap::new();
        let shutdown = AtomicBool::new(false);
        let generation = AtomicU64::new(1);
        let first =
            run_persistent_handler(&mut handlers, &invocation, 1, &shutdown, &generation, 1)
                .unwrap();
        let second =
            run_persistent_handler(&mut handlers, &invocation, 2, &shutdown, &generation, 1)
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
        assert_eq!(handlers.len(), 1);
    }

    #[test]
    fn persistent_handler_is_replaced_when_its_grants_change() {
        let mut invocation = persistent_fixture(
            "descriptor-fixture",
            r#"while IFS= read -r event; do
  printf '{"schema_version":1,"request_id":1,"actions":[]}\n'
done"#,
        );
        let mut handlers = HashMap::new();
        let shutdown = AtomicBool::new(false);
        let generation = AtomicU64::new(1);
        run_persistent_handler(&mut handlers, &invocation, 1, &shutdown, &generation, 1).unwrap();
        invocation.authority = persistent_authority(&["ui-actions", "navigate"]);
        assert!(
            run_persistent_handler(&mut handlers, &invocation, 1, &shutdown, &generation, 1)
                .unwrap_err()
                .contains("descriptor changed")
        );
        assert!(handlers.is_empty());
    }

    #[test]
    fn persistent_protocol_failure_discards_process_and_allows_restart() {
        let mismatched = persistent_fixture(
            "restart-fixture",
            r#"IFS= read -r event
printf '{"schema_version":1,"request_id":999,"actions":[]}\n'
sleep 10"#,
        );
        let mut handlers = HashMap::new();
        let shutdown = AtomicBool::new(false);
        let generation = AtomicU64::new(1);
        let error =
            run_persistent_handler(&mut handlers, &mismatched, 1, &shutdown, &generation, 1)
                .unwrap_err();
        assert!(error.contains("did not match"));
        assert!(handlers.is_empty());

        let healthy = persistent_fixture(
            "restart-fixture",
            r#"while IFS= read -r event; do
  printf '{"schema_version":1,"request_id":2,"actions":[]}\n'
done"#,
        );
        assert!(
            run_persistent_handler(&mut handlers, &healthy, 2, &shutdown, &generation, 1).is_ok()
        );
        assert_eq!(handlers.len(), 1);
    }

    #[test]
    fn persistent_timeout_and_generation_change_terminate_handlers() {
        let invocation = persistent_fixture("cancel-fixture", "IFS= read -r event\nsleep 10");
        let mut handlers = HashMap::new();
        let shutdown = AtomicBool::new(false);
        let generation = Arc::new(AtomicU64::new(1));
        let update = Arc::clone(&generation);
        let changer = thread::spawn(move || {
            thread::sleep(Duration::from_millis(50));
            update.store(2, Ordering::Release);
        });
        let started = Instant::now();
        let error =
            run_persistent_handler(&mut handlers, &invocation, 1, &shutdown, &generation, 1)
                .unwrap_err();
        changer.join().unwrap();
        assert!(error.contains("cancelled"));
        assert!(started.elapsed() < Duration::from_secs(1));
        assert!(handlers.is_empty());
    }

    #[test]
    fn persistent_timeout_and_crash_discard_the_failed_process() {
        let shutdown = AtomicBool::new(false);
        let generation = AtomicU64::new(1);

        let mut handlers = HashMap::new();
        let timeout = persistent_fixture("timeout-fixture", "IFS= read -r event\nsleep 10");
        let started = Instant::now();
        let error = run_persistent_handler(&mut handlers, &timeout, 1, &shutdown, &generation, 1)
            .unwrap_err();
        assert!(error.contains("exceeded 2000 ms"));
        assert!(started.elapsed() < Duration::from_secs(4));
        assert!(handlers.is_empty());

        let crash = persistent_fixture("crash-fixture", "printf 'fixture failure\\n' >&2\nexit 7");
        assert!(
            run_persistent_handler(&mut handlers, &crash, 2, &shutdown, &generation, 1).is_err()
        );
        assert!(handlers.is_empty());
    }

    #[test]
    fn persistent_handler_count_is_bounded() {
        let mut handlers = HashMap::new();
        let shutdown = AtomicBool::new(false);
        let generation = AtomicU64::new(1);
        for request_id in 1..=MAX_PERSISTENT_HANDLERS as u64 {
            let invocation = persistent_fixture(
                &format!("fixture-{request_id}"),
                &format!(
                    "while IFS= read -r event; do printf '{{\"schema_version\":1,\"request_id\":{request_id},\"actions\":[]}}\\n'; done"
                ),
            );
            run_persistent_handler(
                &mut handlers,
                &invocation,
                request_id,
                &shutdown,
                &generation,
                1,
            )
            .unwrap();
        }
        assert_eq!(handlers.len(), MAX_PERSISTENT_HANDLERS);
        let overflow = persistent_fixture("fixture-overflow", "sleep 10");
        assert!(
            run_persistent_handler(&mut handlers, &overflow, 99, &shutdown, &generation, 1)
                .unwrap_err()
                .contains("limit reached")
        );
    }

    #[test]
    fn exited_idle_handlers_do_not_consume_process_capacity() {
        let mut handlers = HashMap::new();
        for index in 0..MAX_PERSISTENT_HANDLERS {
            let invocation = persistent_fixture(&format!("exited-{index}"), "exit 0");
            handlers.insert(
                invocation.plugin_id.clone(),
                PersistentHandler::spawn(&invocation).unwrap(),
            );
        }
        thread::sleep(Duration::from_millis(50));

        let replacement = persistent_fixture(
            "replacement",
            "while IFS= read -r event; do printf '{\"schema_version\":1,\"request_id\":1,\"actions\":[]}\\n'; done",
        );
        assert!(
            run_persistent_handler(
                &mut handlers,
                &replacement,
                1,
                &AtomicBool::new(false),
                &AtomicU64::new(1),
                1,
            )
            .is_ok()
        );
        assert_eq!(handlers.len(), 1);
        assert!(handlers.contains_key("replacement"));
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
            authority: HandlerAuthority::for_event(manifest.permissions, command_invoked),
        }
    }

    #[test]
    fn rerun_in_sibling_example_sends_the_last_command_to_the_next_pane() {
        let mut handlers = HashMap::new();
        let shutdown = AtomicBool::new(false);
        let generation = AtomicU64::new(1);

        // A completed command in pane 0 of tab 5 is remembered, not acted on.
        let mut completed = example_invocation("rerun-in-sibling", false);
        completed.event_json = r#"{"schema_version":1,"plugin_id":"rerun-in-sibling","origin_window_id":3,"event":{"type":"command-completed","region_id":1,"tab_id":5,"pane_id":0,"exit_code":0,"content_available":true,"command":{"text":"cargo test","truncated":false,"sanitized":false},"output":{"text":"ok","truncated":false,"sanitized":false}}}"#.into();
        assert!(
            run_persistent_handler(&mut handlers, &completed, 1, &shutdown, &generation, 1)
                .unwrap()
                .is_empty()
        );

        let mut invocation = example_invocation("rerun-in-sibling", true);
        invocation.event_json = r#"{"schema_version":1,"plugin_id":"rerun-in-sibling","origin_window_id":3,"event":{"type":"command-invoked","command_id":"rerun-in-sibling.rerun"}}"#.into();
        let query =
            run_persistent_handler(&mut handlers, &invocation, 2, &shutdown, &generation, 1)
                .unwrap();
        assert_eq!(
            query,
            vec![StructuredResponseAction::QueryWorkspace {
                query_id: "rerun".into(),
            }]
        );

        invocation.authority = invocation.authority.after(&query[0]);
        invocation.event_json = r#"{"schema_version":1,"plugin_id":"rerun-in-sibling","origin_window_id":3,"event":{"type":"workspace-query-result","query_id":"rerun","truncated":false,"windows":[{"id":3,"tabs":[{"id":4,"title":"other","active":false,"closed":false,"panes":[]},{"id":5,"title":"work","active":true,"closed":false,"panes":[{"id":0,"focused":true,"closed":false},{"id":1,"focused":false,"closed":false}]}]}]}}"#.into();
        let send = run_persistent_handler(&mut handlers, &invocation, 3, &shutdown, &generation, 1)
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
            run_persistent_handler(&mut handlers, &invocation, 4, &shutdown, &generation, 1)
                .unwrap(),
            vec![StructuredResponseAction::ShowMessage {
                message: "Rerun failed: External terminal input is disabled".into(),
            }]
        );
        assert_eq!(handlers.len(), 1);
    }

    #[test]
    fn command_notifier_example_reports_only_unexpected_failures() {
        let mut handlers = HashMap::new();
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
                    &mut handlers,
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
            run_persistent_handler(&mut handlers, &invocation, 3, &shutdown, &generation, 1)
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
    /// everything its manifest requests. Like the host, it sends settings
    /// only with events, never with results.
    struct ExampleConversation {
        handlers: HashMap<String, PersistentHandler>,
        shutdown: AtomicBool,
        generation: AtomicU64,
        invocation: StructuredHandlerInvocation,
        origin_window_id: u64,
        request_id: u64,
    }

    impl ExampleConversation {
        fn new(example: &str, origin_window_id: u64) -> Self {
            Self {
                handlers: HashMap::new(),
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
            let plugin_id = self.invocation.plugin_id.clone();
            let mut envelope = self.envelope(serde_json::json!({
                "type": "command-invoked",
                "command_id": format!("{plugin_id}.{command}"),
            }));
            if let Some(settings) = settings {
                envelope["settings"] = settings;
            }
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
                &mut self.handlers,
                &self.invocation,
                self.request_id,
                &self.shutdown,
                &self.generation,
                1,
            )
            .unwrap();
            assert_eq!(self.handlers.len(), 1, "the handler must stay persistent");
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
    fn window_switcher_example_cycles_windows_and_jumps_to_a_tab_by_title() {
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
        let mut conversation = ExampleConversation::new("window-switcher", 4);

        // Next Window focuses the window after the origin, wrapping around,
        // and reports a refused focus request.
        for (origin, target) in [(4, 5), (5, 3)] {
            conversation.origin_window_id = origin;
            let focus = StructuredResponseAction::FocusWindow {
                action_id: "next-window".into(),
                window_id: target,
            };
            assert_eq!(
                conversation.command_with_workspace("next-window", settings(), windows(false)),
                vec![focus.clone()]
            );
            let error =
                "Termeric can focus another window on Wayland only when it is already focused";
            assert_eq!(
                conversation.result(
                    &focus,
                    serde_json::json!({
                        "type": "action-result",
                        "action": "focus-window",
                        "action_id": "next-window",
                        "window_id": target,
                        "ok": false,
                        "error": error,
                        "error_truncated": false,
                    }),
                ),
                vec![message(&format!("Window switch failed: {error}"))]
            );
        }

        // Jump to Tab matches the title setting from the command event even
        // though the query result carries no settings; without it, the empty
        // title would match window 3's "shell" tab first.
        conversation.origin_window_id = 3;
        let select = StructuredResponseAction::SelectTab {
            action_id: "jump".into(),
            window_id: Some(4),
            tab_id: 3,
        };
        assert_eq!(
            conversation.command_with_workspace("jump-to-tab", settings(), windows(false)),
            vec![
                select.clone(),
                message("Tab selected; run Jump to Tab again to focus its window"),
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

        // Once the tab is selected, running it again focuses its window.
        assert_eq!(
            conversation.command_with_workspace("jump-to-tab", settings(), windows(true)),
            vec![StructuredResponseAction::FocusWindow {
                action_id: "jump".into(),
                window_id: 4,
            }]
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
        let layout = |action_id: &str, preset| StructuredResponseAction::ApplyPaneLayout {
            action_id: action_id.into(),
            window_id: None,
            tab_id: 5,
            preset,
        };
        assert_eq!(
            conversation.command_with_workspace("dev-layout", None, windows(3)),
            vec![layout("dev-layout", PaneLayoutPreset::Tall)]
        );

        // Tile and Balance act on the same tab; a successful result is quiet.
        let tile = layout("tile", PaneLayoutPreset::Grid);
        assert_eq!(
            conversation.command_with_workspace("tile", None, windows(4)),
            vec![tile.clone()]
        );
        assert!(
            conversation
                .result(
                    &tile,
                    serde_json::json!({
                        "type": "action-result",
                        "action": "apply-pane-layout",
                        "action_id": "tile",
                        "window_id": 3,
                        "tab_id": 5,
                        "preset": "grid",
                        "ok": true,
                        "changed": true,
                    }),
                )
                .is_empty()
        );
        assert_eq!(
            conversation.command_with_workspace("balance", None, windows(2)),
            vec![StructuredResponseAction::BalancePanes {
                action_id: "balance".into(),
                window_id: None,
                tab_id: 5,
            }]
        );

        // A workspace without the origin window has nothing to arrange.
        assert_eq!(
            conversation.command_with_workspace("tile", None, serde_json::json!([])),
            vec![message("No active tab to arrange")]
        );
    }
}
