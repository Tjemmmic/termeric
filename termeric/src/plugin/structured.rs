//! Bounded out-of-process execution for declarative plugin lifecycle handlers.

use serde::Deserialize;
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

#[cfg(unix)]
use std::os::unix::process::CommandExt;

const QUEUE_CAPACITY: usize = 32;
const MAX_PERSISTENT_HANDLERS: usize = 8;
pub(crate) const MAX_STRUCTURED_EVENT_BYTES: usize = 16 * 1024;
pub(crate) const MAX_PERSISTENT_SOURCE_EVENT_BYTES: usize =
    MAX_STRUCTURED_EVENT_BYTES - PERSISTENT_PROTOCOL_RESERVE_BYTES;
pub(crate) const MAX_TERMINAL_QUERY_BYTES: u32 = 8 * 1024;
pub(crate) const MAX_TERMINAL_INPUT_BYTES: usize = 8 * 1024;
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
pub(crate) const MAX_ACTION_ID_BYTES: usize = 64;
const RESPONSE_SCHEMA_VERSION: u32 = 1;
const PERSISTENT_SCHEMA_VERSION: u32 = 2;
const WORKER_POLL_INTERVAL: Duration = Duration::from_millis(10);
const PERSISTENT_REAP_INTERVAL: Duration = Duration::from_millis(100);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StructuredHandlerMode {
    ProcessPerEvent,
    Persistent,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, serde::Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum PluginPaneLayoutPreset {
    Columns,
    Rows,
    Grid,
}

#[derive(Debug, Clone)]
pub struct StructuredHandlerInvocation {
    pub plugin_id: String,
    pub program: String,
    pub args: Vec<String>,
    pub event_json: String,
    pub allow_ui_actions: bool,
    /// Permit one current-window workspace query from this response. Query
    /// result requests always clear this flag to prevent query chaining.
    pub allow_workspace_query: bool,
    /// Permit one current-window focus/selection mutation from this response.
    /// Workspace action result requests clear every host-operation authority
    /// so a handler cannot create an unbounded mutation loop.
    pub allow_workspace_actions: bool,
    /// Permit one non-destructive current-window topology adjustment from this
    /// response. Result requests clear every host-operation authority.
    pub allow_workspace_topology: bool,
    /// Permit one bounded presented-text query for an exact current-window
    /// terminal. A terminal result clears every host-operation authority.
    pub allow_terminal_query: bool,
    /// Permit one exact ordinary-pane split in the current window. Its result
    /// clears every host-operation authority to prevent recursive growth.
    pub allow_pane_create: bool,
    /// Permit one ordinary local tab in the current window. Its result clears
    /// every host-operation authority to prevent recursive growth.
    pub allow_tab_create: bool,
    /// Permit closing one exact ordinary pane in the current window. Its
    /// result clears every host-operation authority to prevent recursive
    /// mutation.
    pub allow_pane_close: bool,
    /// Permit scheduling closure of one exact ordinary tab in the current
    /// window. Its result clears every host-operation authority.
    pub allow_tab_close: bool,
    /// Permit moving one exact ordinary pane between current-window tabs.
    /// Its result clears every host-operation authority.
    pub allow_pane_move: bool,
    /// Permit sending bounded text to one exact current-window terminal. Its
    /// result clears every host-operation authority.
    pub allow_terminal_input: bool,
    /// Permit non-destructive same-tab pane layout changes in the current
    /// window. Its result clears every host-operation authority.
    pub allow_pane_layout: bool,
    /// Permit scheduling closure of this exact window. This authority exists
    /// only on an explicitly invoked plugin command and is cleared from the
    /// final result request.
    pub allow_window_close: bool,
    /// Permit one bounded process-wide workspace inventory query. A result
    /// clears every query authority while later APIs may retain only their
    /// separately granted process follow-up authorities.
    pub allow_process_query: bool,
    /// Permit one exact non-activating tab selection or pane focus in any live
    /// window owned by this Termeric process. Its result clears every host
    /// operation authority.
    pub allow_process_actions: bool,
    /// Permit one bounded presentation-safe text query for an exact terminal
    /// in any live window owned by this Termeric process. Its result clears
    /// every host operation authority.
    pub allow_process_terminal_text: bool,
    /// Permit one bounded guarded text write to an exact terminal in any live
    /// window owned by this Termeric process. Its result clears every host
    /// operation authority.
    pub allow_process_terminal_input: bool,
    /// Permit one ordinary local tab in an exact live window owned by this
    /// Termeric process. Its result clears every host operation authority.
    pub allow_process_tab_create: bool,
    pub mode: StructuredHandlerMode,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
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
    QueryProcess {
        query_id: String,
    },
    SelectProcessTab {
        action_id: String,
        window_id: u64,
        tab_id: u64,
    },
    FocusProcessPane {
        action_id: String,
        window_id: u64,
        tab_id: u64,
        pane_id: u32,
    },
    QueryProcessTerminal {
        query_id: String,
        window_id: u64,
        tab_id: u64,
        pane_id: u32,
        history_lines: u16,
        max_bytes: u32,
    },
    SendProcessText {
        action_id: String,
        window_id: u64,
        tab_id: u64,
        pane_id: u32,
        text: String,
        paste: bool,
        enter: bool,
    },
    CreateProcessTab {
        action_id: String,
        window_id: u64,
    },
    QueryTerminal {
        query_id: String,
        tab_id: u64,
        pane_id: u32,
        history_lines: u16,
        max_bytes: u32,
    },
    SelectTab {
        action_id: String,
        tab_id: u64,
    },
    FocusPane {
        action_id: String,
        tab_id: u64,
        pane_id: u32,
    },
    MoveTab {
        action_id: String,
        tab_id: u64,
        destination_index: u16,
    },
    ResizeDivider {
        action_id: String,
        tab_id: u64,
        divider_id: u64,
        ratio_millis: u16,
    },
    CreatePane {
        action_id: String,
        tab_id: u64,
        pane_id: u32,
        direction: crate::control::protocol::SplitDirection,
    },
    CreateTab {
        action_id: String,
    },
    ClosePane {
        action_id: String,
        tab_id: u64,
        pane_id: u32,
    },
    CloseTab {
        action_id: String,
        tab_id: u64,
    },
    MovePane {
        action_id: String,
        source_tab_id: u64,
        pane_id: u32,
        destination_tab_id: u64,
        target_pane_id: u32,
        direction: crate::control::protocol::SplitDirection,
    },
    SendText {
        action_id: String,
        tab_id: u64,
        pane_id: u32,
        text: String,
        paste: bool,
        enter: bool,
    },
    SwapPanes {
        action_id: String,
        tab_id: u64,
        first_pane_id: u32,
        second_pane_id: u32,
    },
    BalancePanes {
        action_id: String,
        tab_id: u64,
    },
    ApplyPaneLayout {
        action_id: String,
        tab_id: u64,
        preset: PluginPaneLayoutPreset,
    },
    CloseWindow {
        action_id: String,
    },
}

#[derive(Debug)]
pub struct StructuredHandlerCompletion {
    pub window_id: WindowId,
    pub plugin_generation: u64,
    pub plugin_id: String,
    pub request_id: u64,
    /// Persistent descriptor retained only long enough to deliver a bounded
    /// follow-up workspace result through the same isolated process.
    pub continuation: Option<StructuredHandlerInvocation>,
    pub result: Result<Vec<StructuredResponseAction>, String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct StructuredResponse {
    schema_version: u32,
    #[serde(default)]
    actions: Vec<StructuredResponseAction>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct PersistentStructuredResponse {
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
                            let result = match item.invocation.mode {
                                StructuredHandlerMode::ProcessPerEvent => run_handler(
                                    &item.invocation,
                                    &worker_shutdown,
                                    &worker_generation,
                                    item.plugin_generation,
                                ),
                                StructuredHandlerMode::Persistent => run_persistent_handler(
                                    &mut persistent_handlers,
                                    &item.invocation,
                                    item.request_id,
                                    &worker_shutdown,
                                    &worker_generation,
                                    item.plugin_generation,
                                ),
                            };
                            let plugin_id = item.invocation.plugin_id.clone();
                            let continuation = (item.invocation.mode
                                == StructuredHandlerMode::Persistent)
                                .then_some(item.invocation);
                            let _ = item.proxy.send_event(
                                crate::app::AppEvent::StructuredPluginCompleted(
                                    StructuredHandlerCompletion {
                                        window_id: item.window_id,
                                        plugin_generation: item.plugin_generation,
                                        plugin_id,
                                        request_id: item.request_id,
                                        continuation,
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

fn run_handler(
    invocation: &StructuredHandlerInvocation,
    shutdown: &AtomicBool,
    active_generation: &AtomicU64,
    expected_generation: u64,
) -> Result<Vec<StructuredResponseAction>, String> {
    let mut command = Command::new(&invocation.program);
    command
        .args(&invocation.args)
        .env_clear()
        .env("TERMERIC_PLUGIN_ID", &invocation.plugin_id)
        .env("TERMERIC_PLUGIN_EVENT_SCHEMA", "1")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(unix)]
    command.process_group(0);
    if let Some(path) = std::env::var_os("PATH") {
        command.env("PATH", path);
    }
    let mut child = command
        .spawn()
        .map_err(|error| bounded_error(&format!("handler launch failed: {error}")))?;
    let Some(stdin) = child.stdin.take() else {
        terminate_handler(&mut child);
        return Err("handler stdin was unavailable".into());
    };
    let Some(stdout) = child.stdout.take() else {
        terminate_handler(&mut child);
        return Err("handler stdout was unavailable".into());
    };
    let Some(stderr) = child.stderr.take() else {
        terminate_handler(&mut child);
        return Err("handler stderr was unavailable".into());
    };
    let event_json = invocation.event_json.clone();
    let stdin_writer = thread::spawn(move || {
        let mut stdin = stdin;
        stdin.write_all(event_json.as_bytes())
    });
    let stdout_reader = thread::spawn(move || read_bounded(stdout, MAX_OUTPUT_BYTES));
    let stderr_reader = thread::spawn(move || read_bounded(stderr, MAX_ERROR_BYTES));
    let started = Instant::now();
    let status = loop {
        if shutdown.load(Ordering::Acquire)
            || active_generation.load(Ordering::Acquire) != expected_generation
        {
            stop_handler(&mut child, stdin_writer, stdout_reader, stderr_reader);
            return Err("structured plugin handler was cancelled".into());
        }
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if started.elapsed() < HANDLER_TIMEOUT => {
                thread::sleep(Duration::from_millis(10));
            }
            Ok(None) => {
                stop_handler(&mut child, stdin_writer, stdout_reader, stderr_reader);
                return Err(format!(
                    "structured plugin handler exceeded {} ms",
                    HANDLER_TIMEOUT.as_millis()
                ));
            }
            Err(error) => {
                stop_handler(&mut child, stdin_writer, stdout_reader, stderr_reader);
                return Err(bounded_error(&format!("handler wait failed: {error}")));
            }
        }
    };
    // A handler is one bounded event process, not a service supervisor. Close
    // any descendants that outlived the parent before joining captured pipes.
    terminate_handler_group(child.id());
    let stdin_result = stdin_writer
        .join()
        .map_err(|_| "handler stdin writer failed".to_string())?
        .map_err(|error| bounded_error(&format!("handler event write failed: {error}")));
    let stdout_result = stdout_reader
        .join()
        .map_err(|_| "handler stdout reader failed".to_string())?
        .map_err(|error| bounded_error(&format!("handler stdout read failed: {error}")));
    let stderr_result = stderr_reader
        .join()
        .map_err(|_| "handler stderr reader failed".to_string())?
        .map_err(|error| bounded_error(&format!("handler stderr read failed: {error}")));
    if !status.success() {
        let mut detail = stderr_result
            .as_ref()
            .map(|(stderr, _)| String::from_utf8_lossy(stderr).into_owned())
            .unwrap_or_default();
        if stderr_result
            .as_ref()
            .is_ok_and(|(_, truncated)| *truncated)
        {
            detail.push('…');
        }
        return Err(bounded_error(&format!(
            "handler exited with {status}: {detail}"
        )));
    }
    stdin_result?;
    let (stdout, stdout_truncated) = stdout_result?;
    stderr_result?;
    if stdout_truncated {
        return Err(format!(
            "structured plugin response exceeds {MAX_OUTPUT_BYTES} bytes"
        ));
    }
    parse_response_with_host_mutation(
        &stdout,
        invocation.allow_ui_actions,
        invocation.allow_workspace_query,
        invocation.allow_workspace_actions,
        invocation.allow_workspace_topology,
        invocation.allow_terminal_query,
        invocation.allow_pane_create,
        invocation.allow_tab_create,
        invocation.allow_pane_close,
        invocation.allow_tab_close,
        invocation.allow_pane_move,
        invocation.allow_terminal_input,
        invocation.allow_pane_layout,
        invocation.allow_window_close,
        invocation.allow_process_query,
        invocation.allow_process_actions,
        invocation.allow_process_terminal_text,
        invocation.allow_process_terminal_input,
        invocation.allow_process_tab_create,
    )
}

#[derive(Debug)]
struct PersistentHandler {
    program: String,
    args: Vec<String>,
    allow_ui_actions: bool,
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
        let mut command = Command::new(&invocation.program);
        command
            .args(&invocation.args)
            .env_clear()
            .env("TERMERIC_PLUGIN_ID", &invocation.plugin_id)
            .env(
                "TERMERIC_PLUGIN_EVENT_SCHEMA",
                PERSISTENT_SCHEMA_VERSION.to_string(),
            )
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        #[cfg(unix)]
        command.process_group(0);
        if let Some(path) = std::env::var_os("PATH") {
            command.env("PATH", path);
        }
        let mut child = command
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
            allow_ui_actions: invocation.allow_ui_actions,
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
            && self.allow_ui_actions == invocation.allow_ui_actions
    }

    fn is_running(&mut self) -> bool {
        matches!(self.child.try_wait(), Ok(None))
    }

    fn request(
        &mut self,
        event: Vec<u8>,
        request_id: u64,
        allow_workspace_query: bool,
        allow_workspace_actions: bool,
        allow_workspace_topology: bool,
        allow_terminal_query: bool,
        allow_pane_create: bool,
        allow_tab_create: bool,
        allow_pane_close: bool,
        allow_tab_close: bool,
        allow_pane_move: bool,
        allow_terminal_input: bool,
        allow_pane_layout: bool,
        allow_window_close: bool,
        allow_process_query: bool,
        allow_process_actions: bool,
        allow_process_terminal_text: bool,
        allow_process_terminal_input: bool,
        allow_process_tab_create: bool,
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
        let actions = parse_persistent_response_with_window_close(
            &bytes,
            self.allow_ui_actions,
            allow_workspace_query,
            allow_workspace_actions,
            allow_workspace_topology,
            allow_terminal_query,
            allow_pane_create,
            allow_tab_create,
            allow_pane_close,
            allow_tab_close,
            allow_pane_move,
            allow_terminal_input,
            allow_pane_layout,
            allow_window_close,
            allow_process_query,
            allow_process_actions,
            allow_process_terminal_text,
            allow_process_terminal_input,
            allow_process_tab_create,
            request_id,
        )?;
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
                invocation.allow_workspace_query,
                invocation.allow_workspace_actions,
                invocation.allow_workspace_topology,
                invocation.allow_terminal_query,
                invocation.allow_pane_create,
                invocation.allow_tab_create,
                invocation.allow_pane_close,
                invocation.allow_tab_close,
                invocation.allow_pane_move,
                invocation.allow_terminal_input,
                invocation.allow_pane_layout,
                invocation.allow_window_close,
                invocation.allow_process_query,
                invocation.allow_process_actions,
                invocation.allow_process_terminal_text,
                invocation.allow_process_terminal_input,
                invocation.allow_process_tab_create,
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

fn persistent_request_json(event_json: &str, request_id: u64) -> Result<Vec<u8>, String> {
    let mut value: serde_json::Value = serde_json::from_str(event_json)
        .map_err(|error| bounded_error(&format!("invalid structured event JSON: {error}")))?;
    let object = value
        .as_object_mut()
        .ok_or_else(|| "persistent structured event must be a JSON object".to_string())?;
    if object
        .get("schema_version")
        .and_then(serde_json::Value::as_u64)
        != Some(u64::from(RESPONSE_SCHEMA_VERSION))
    {
        return Err("persistent handlers require a schema-v1 source event".into());
    }
    object.insert(
        "schema_version".into(),
        serde_json::json!(PERSISTENT_SCHEMA_VERSION),
    );
    object.insert("request_id".into(), serde_json::json!(request_id));
    let bytes = serde_json::to_vec(&value)
        .map_err(|error| bounded_error(&format!("could not encode persistent event: {error}")))?;
    if bytes.len() > MAX_STRUCTURED_EVENT_BYTES {
        return Err(format!(
            "persistent structured event exceeds {} bytes",
            MAX_STRUCTURED_EVENT_BYTES
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

fn stop_handler(
    child: &mut std::process::Child,
    stdin_writer: JoinHandle<std::io::Result<()>>,
    stdout_reader: JoinHandle<std::io::Result<(Vec<u8>, bool)>>,
    stderr_reader: JoinHandle<std::io::Result<(Vec<u8>, bool)>>,
) {
    terminate_handler(child);
    let _ = stdin_writer.join();
    let _ = stdout_reader.join();
    let _ = stderr_reader.join();
}

fn terminate_handler(child: &mut std::process::Child) {
    terminate_handler_group(child.id());
    let _ = child.kill();
    let _ = child.wait();
}

#[cfg(unix)]
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

#[cfg(not(unix))]
fn terminate_handler_group(_process_id: u32) {}

fn read_bounded(mut stream: impl Read, max: u64) -> std::io::Result<(Vec<u8>, bool)> {
    let mut bytes = Vec::new();
    stream.by_ref().take(max + 1).read_to_end(&mut bytes)?;
    let truncated = bytes.len() as u64 > max;
    bytes.truncate(max as usize);
    if truncated {
        // Keep draining without retaining bytes so a finite oversized writer
        // can exit and be classified by the output ceiling instead of filling
        // the pipe and being misreported as a timeout.
        std::io::copy(&mut stream, &mut std::io::sink())?;
    }
    Ok((bytes, truncated))
}

#[cfg(test)]
fn parse_response(
    bytes: &[u8],
    allow_ui_actions: bool,
    allow_workspace_query: bool,
    allow_workspace_actions: bool,
    allow_workspace_topology: bool,
    allow_terminal_query: bool,
) -> Result<Vec<StructuredResponseAction>, String> {
    parse_response_with_host_creation(
        bytes,
        allow_ui_actions,
        allow_workspace_query,
        allow_workspace_actions,
        allow_workspace_topology,
        allow_terminal_query,
        false,
        false,
        false,
        false,
    )
}

#[cfg(test)]
fn parse_response_with_host_creation(
    bytes: &[u8],
    allow_ui_actions: bool,
    allow_workspace_query: bool,
    allow_workspace_actions: bool,
    allow_workspace_topology: bool,
    allow_terminal_query: bool,
    allow_pane_create: bool,
    allow_tab_create: bool,
    allow_pane_close: bool,
    allow_tab_close: bool,
) -> Result<Vec<StructuredResponseAction>, String> {
    parse_response_with_host_mutation(
        bytes,
        allow_ui_actions,
        allow_workspace_query,
        allow_workspace_actions,
        allow_workspace_topology,
        allow_terminal_query,
        allow_pane_create,
        allow_tab_create,
        allow_pane_close,
        allow_tab_close,
        false,
        false,
        false,
        false,
        false,
        false,
        false,
        false,
        false,
    )
}

#[allow(clippy::too_many_arguments)]
fn parse_response_with_host_mutation(
    bytes: &[u8],
    allow_ui_actions: bool,
    allow_workspace_query: bool,
    allow_workspace_actions: bool,
    allow_workspace_topology: bool,
    allow_terminal_query: bool,
    allow_pane_create: bool,
    allow_tab_create: bool,
    allow_pane_close: bool,
    allow_tab_close: bool,
    allow_pane_move: bool,
    allow_terminal_input: bool,
    allow_pane_layout: bool,
    allow_window_close: bool,
    allow_process_query: bool,
    allow_process_actions: bool,
    allow_process_terminal_text: bool,
    allow_process_terminal_input: bool,
    allow_process_tab_create: bool,
) -> Result<Vec<StructuredResponseAction>, String> {
    let response: StructuredResponse = serde_json::from_slice(bytes)
        .map_err(|error| bounded_error(&format!("invalid handler response JSON: {error}")))?;
    if response.schema_version != RESPONSE_SCHEMA_VERSION {
        return Err(format!(
            "unsupported handler response schema {}; expected {RESPONSE_SCHEMA_VERSION}",
            response.schema_version
        ));
    }
    validate_response_actions(
        response.actions,
        allow_ui_actions,
        allow_workspace_query,
        allow_workspace_actions,
        allow_workspace_topology,
        allow_terminal_query,
        allow_pane_create,
        allow_tab_create,
        allow_pane_close,
        allow_tab_close,
        allow_pane_move,
        allow_terminal_input,
        allow_pane_layout,
        allow_window_close,
        allow_process_query,
        allow_process_actions,
        allow_process_terminal_text,
        allow_process_terminal_input,
        allow_process_tab_create,
    )
}

#[cfg(test)]
fn parse_persistent_response(
    bytes: &[u8],
    allow_ui_actions: bool,
    allow_workspace_query: bool,
    allow_workspace_actions: bool,
    allow_workspace_topology: bool,
    allow_terminal_query: bool,
    expected_request_id: u64,
) -> Result<Vec<StructuredResponseAction>, String> {
    parse_persistent_response_with_host_creation(
        bytes,
        allow_ui_actions,
        allow_workspace_query,
        allow_workspace_actions,
        allow_workspace_topology,
        allow_terminal_query,
        false,
        false,
        false,
        false,
        expected_request_id,
    )
}

#[cfg(test)]
fn parse_persistent_response_with_host_creation(
    bytes: &[u8],
    allow_ui_actions: bool,
    allow_workspace_query: bool,
    allow_workspace_actions: bool,
    allow_workspace_topology: bool,
    allow_terminal_query: bool,
    allow_pane_create: bool,
    allow_tab_create: bool,
    allow_pane_close: bool,
    allow_tab_close: bool,
    expected_request_id: u64,
) -> Result<Vec<StructuredResponseAction>, String> {
    parse_persistent_response_with_host_mutation(
        bytes,
        allow_ui_actions,
        allow_workspace_query,
        allow_workspace_actions,
        allow_workspace_topology,
        allow_terminal_query,
        allow_pane_create,
        allow_tab_create,
        allow_pane_close,
        allow_tab_close,
        false,
        false,
        false,
        expected_request_id,
    )
}

#[allow(clippy::too_many_arguments)]
#[cfg(test)]
fn parse_persistent_response_with_host_mutation(
    bytes: &[u8],
    allow_ui_actions: bool,
    allow_workspace_query: bool,
    allow_workspace_actions: bool,
    allow_workspace_topology: bool,
    allow_terminal_query: bool,
    allow_pane_create: bool,
    allow_tab_create: bool,
    allow_pane_close: bool,
    allow_tab_close: bool,
    allow_pane_move: bool,
    allow_terminal_input: bool,
    allow_pane_layout: bool,
    expected_request_id: u64,
) -> Result<Vec<StructuredResponseAction>, String> {
    parse_persistent_response_with_window_close(
        bytes,
        allow_ui_actions,
        allow_workspace_query,
        allow_workspace_actions,
        allow_workspace_topology,
        allow_terminal_query,
        allow_pane_create,
        allow_tab_create,
        allow_pane_close,
        allow_tab_close,
        allow_pane_move,
        allow_terminal_input,
        allow_pane_layout,
        false,
        false,
        false,
        false,
        false,
        false,
        expected_request_id,
    )
}

#[allow(clippy::too_many_arguments)]
fn parse_persistent_response_with_window_close(
    bytes: &[u8],
    allow_ui_actions: bool,
    allow_workspace_query: bool,
    allow_workspace_actions: bool,
    allow_workspace_topology: bool,
    allow_terminal_query: bool,
    allow_pane_create: bool,
    allow_tab_create: bool,
    allow_pane_close: bool,
    allow_tab_close: bool,
    allow_pane_move: bool,
    allow_terminal_input: bool,
    allow_pane_layout: bool,
    allow_window_close: bool,
    allow_process_query: bool,
    allow_process_actions: bool,
    allow_process_terminal_text: bool,
    allow_process_terminal_input: bool,
    allow_process_tab_create: bool,
    expected_request_id: u64,
) -> Result<Vec<StructuredResponseAction>, String> {
    let response: PersistentStructuredResponse = serde_json::from_slice(bytes)
        .map_err(|error| bounded_error(&format!("invalid handler response JSON: {error}")))?;
    if response.schema_version != PERSISTENT_SCHEMA_VERSION {
        return Err(format!(
            "unsupported persistent handler response schema {}; expected {PERSISTENT_SCHEMA_VERSION}",
            response.schema_version
        ));
    }
    if response.request_id != expected_request_id {
        return Err(format!(
            "persistent handler response request id {} did not match {expected_request_id}",
            response.request_id
        ));
    }
    validate_response_actions(
        response.actions,
        allow_ui_actions,
        allow_workspace_query,
        allow_workspace_actions,
        allow_workspace_topology,
        allow_terminal_query,
        allow_pane_create,
        allow_tab_create,
        allow_pane_close,
        allow_tab_close,
        allow_pane_move,
        allow_terminal_input,
        allow_pane_layout,
        allow_window_close,
        allow_process_query,
        allow_process_actions,
        allow_process_terminal_text,
        allow_process_terminal_input,
        allow_process_tab_create,
    )
}

fn validate_response_actions(
    actions: Vec<StructuredResponseAction>,
    allow_ui_actions: bool,
    allow_workspace_query: bool,
    allow_workspace_actions: bool,
    allow_workspace_topology: bool,
    allow_terminal_query: bool,
    allow_pane_create: bool,
    allow_tab_create: bool,
    allow_pane_close: bool,
    allow_tab_close: bool,
    allow_pane_move: bool,
    allow_terminal_input: bool,
    allow_pane_layout: bool,
    allow_window_close: bool,
    allow_process_query: bool,
    allow_process_actions: bool,
    allow_process_terminal_text: bool,
    allow_process_terminal_input: bool,
    allow_process_tab_create: bool,
) -> Result<Vec<StructuredResponseAction>, String> {
    if actions.len() > MAX_ACTIONS {
        return Err(format!(
            "handler response contains more than {MAX_ACTIONS} actions"
        ));
    }
    let mut host_operations = 0;
    for action in &actions {
        match action {
            StructuredResponseAction::ShowMessage { message } => {
                if !allow_ui_actions {
                    return Err("handler returned UI actions without the ui-actions grant".into());
                }
                validate_text("handler message", message, MAX_MESSAGE_BYTES)?;
            }
            StructuredResponseAction::ShowNotice { title, lines } => {
                if !allow_ui_actions {
                    return Err("handler returned UI actions without the ui-actions grant".into());
                }
                validate_text("handler notice title", title, MAX_TITLE_BYTES)?;
                if lines.is_empty() || lines.len() > MAX_NOTICE_LINES {
                    return Err(format!(
                        "handler notice must contain 1 to {MAX_NOTICE_LINES} lines"
                    ));
                }
                for line in lines {
                    validate_text("handler notice line", line, MAX_LINE_BYTES)?;
                }
            }
            StructuredResponseAction::QueryWorkspace { query_id } => {
                if !allow_workspace_query {
                    return Err(
                        "handler requested a workspace query without API v6 persistent workspace-query authority"
                            .into(),
                    );
                }
                host_operations += 1;
                validate_text("workspace query id", query_id, MAX_QUERY_ID_BYTES)?;
            }
            StructuredResponseAction::QueryProcess { query_id } => {
                if !allow_process_query {
                    return Err(
                        "handler requested a process query without API v20 persistent process-query authority"
                            .into(),
                    );
                }
                host_operations += 1;
                validate_text("process query id", query_id, MAX_QUERY_ID_BYTES)?;
            }
            StructuredResponseAction::SelectProcessTab { action_id, .. }
            | StructuredResponseAction::FocusProcessPane { action_id, .. } => {
                if !allow_process_actions {
                    return Err(
                        "handler requested a process action without API v21 persistent process-actions authority"
                            .into(),
                    );
                }
                host_operations += 1;
                validate_text("process action id", action_id, MAX_ACTION_ID_BYTES)?;
            }
            StructuredResponseAction::QueryProcessTerminal {
                query_id,
                history_lines,
                max_bytes,
                ..
            } => {
                if !allow_process_terminal_text {
                    return Err(
                        "handler requested process terminal text without API v22 persistent process-terminal-text authority"
                            .into(),
                    );
                }
                host_operations += 1;
                validate_text("process terminal query id", query_id, MAX_QUERY_ID_BYTES)?;
                if *history_lines > crate::control::protocol::MAX_TEXT_HISTORY_LINES {
                    return Err(format!(
                        "process terminal query history is limited to {} lines",
                        crate::control::protocol::MAX_TEXT_HISTORY_LINES
                    ));
                }
                if !(1..=MAX_TERMINAL_QUERY_BYTES).contains(max_bytes) {
                    return Err(format!(
                        "process terminal query bytes must be between 1 and {MAX_TERMINAL_QUERY_BYTES}"
                    ));
                }
            }
            StructuredResponseAction::SendProcessText {
                action_id,
                text,
                enter,
                ..
            } => {
                if !allow_process_terminal_input {
                    return Err(
                        "handler requested process terminal input without API v23 persistent process-terminal-input authority"
                            .into(),
                    );
                }
                host_operations += 1;
                validate_terminal_input_action("process terminal input", action_id, text, *enter)?;
            }
            StructuredResponseAction::CreateProcessTab { action_id, .. } => {
                if !allow_process_tab_create {
                    return Err(
                        "handler requested process tab creation without API v24 persistent process-tab-create authority"
                            .into(),
                    );
                }
                host_operations += 1;
                validate_text(
                    "process tab creation action id",
                    action_id,
                    MAX_ACTION_ID_BYTES,
                )?;
            }
            StructuredResponseAction::QueryTerminal {
                query_id,
                history_lines,
                max_bytes,
                ..
            } => {
                if !allow_terminal_query {
                    return Err(
                        "handler requested terminal text without API v11 persistent terminal-query authority"
                            .into(),
                    );
                }
                host_operations += 1;
                validate_text("terminal query id", query_id, MAX_QUERY_ID_BYTES)?;
                if *history_lines > crate::control::protocol::MAX_TEXT_HISTORY_LINES {
                    return Err(format!(
                        "terminal query history is limited to {} lines",
                        crate::control::protocol::MAX_TEXT_HISTORY_LINES
                    ));
                }
                if !(1..=MAX_TERMINAL_QUERY_BYTES).contains(max_bytes) {
                    return Err(format!(
                        "terminal query bytes must be between 1 and {MAX_TERMINAL_QUERY_BYTES}"
                    ));
                }
            }
            StructuredResponseAction::SelectTab { action_id, .. }
            | StructuredResponseAction::FocusPane { action_id, .. } => {
                if !allow_workspace_actions {
                    return Err(
                        "handler requested a workspace action without API v7 persistent workspace-actions authority"
                            .into(),
                    );
                }
                host_operations += 1;
                validate_text("workspace action id", action_id, MAX_ACTION_ID_BYTES)?;
            }
            StructuredResponseAction::MoveTab {
                action_id,
                destination_index,
                ..
            } => {
                if !allow_workspace_topology {
                    return Err(
                        "handler requested a workspace topology action without API v8 persistent workspace-topology authority"
                            .into(),
                    );
                }
                host_operations += 1;
                validate_text("workspace action id", action_id, MAX_ACTION_ID_BYTES)?;
                if usize::from(*destination_index) >= crate::control::protocol::MAX_TABS_PER_WINDOW
                {
                    return Err(format!(
                        "workspace tab destination index must be below {}",
                        crate::control::protocol::MAX_TABS_PER_WINDOW
                    ));
                }
            }
            StructuredResponseAction::ResizeDivider {
                action_id,
                ratio_millis,
                ..
            } => {
                if !allow_workspace_topology {
                    return Err(
                        "handler requested a workspace topology action without API v8 persistent workspace-topology authority"
                            .into(),
                    );
                }
                host_operations += 1;
                validate_text("workspace action id", action_id, MAX_ACTION_ID_BYTES)?;
                if !(100..=900).contains(ratio_millis) {
                    return Err(
                        "workspace divider ratio must be between 100 and 900 thousandths".into(),
                    );
                }
            }
            StructuredResponseAction::CreatePane { action_id, .. } => {
                if !allow_pane_create {
                    return Err(
                        "handler requested pane creation without API v12 persistent pane-create authority"
                            .into(),
                    );
                }
                host_operations += 1;
                validate_text("pane creation action id", action_id, MAX_ACTION_ID_BYTES)?;
            }
            StructuredResponseAction::CreateTab { action_id } => {
                if !allow_tab_create {
                    return Err(
                        "handler requested tab creation without API v13 persistent tab-create authority"
                            .into(),
                    );
                }
                host_operations += 1;
                validate_text("tab creation action id", action_id, MAX_ACTION_ID_BYTES)?;
            }
            StructuredResponseAction::ClosePane { action_id, .. } => {
                if !allow_pane_close {
                    return Err(
                        "handler requested pane closure without API v14 persistent pane-close authority"
                            .into(),
                    );
                }
                host_operations += 1;
                validate_text("pane closure action id", action_id, MAX_ACTION_ID_BYTES)?;
            }
            StructuredResponseAction::CloseTab { action_id, .. } => {
                if !allow_tab_close {
                    return Err(
                        "handler requested tab closure without API v15 persistent tab-close authority"
                            .into(),
                    );
                }
                host_operations += 1;
                validate_text("tab closure action id", action_id, MAX_ACTION_ID_BYTES)?;
            }
            StructuredResponseAction::MovePane { action_id, .. } => {
                if !allow_pane_move {
                    return Err(
                        "handler requested pane movement without API v16 persistent pane-move authority"
                            .into(),
                    );
                }
                host_operations += 1;
                validate_text("pane movement action id", action_id, MAX_ACTION_ID_BYTES)?;
            }
            StructuredResponseAction::SendText {
                action_id,
                text,
                enter,
                ..
            } => {
                if !allow_terminal_input {
                    return Err(
                        "handler requested terminal input without API v17 persistent terminal-input authority"
                            .into(),
                    );
                }
                host_operations += 1;
                validate_terminal_input_action("terminal input", action_id, text, *enter)?;
            }
            StructuredResponseAction::SwapPanes {
                action_id,
                first_pane_id,
                second_pane_id,
                ..
            } => {
                if !allow_pane_layout {
                    return Err(
                        "handler requested pane layout without API v18 persistent pane-layout authority"
                            .into(),
                    );
                }
                host_operations += 1;
                validate_text("pane layout action id", action_id, MAX_ACTION_ID_BYTES)?;
                if first_pane_id == second_pane_id {
                    return Err("pane swap requires two distinct pane IDs".into());
                }
            }
            StructuredResponseAction::BalancePanes { action_id, .. }
            | StructuredResponseAction::ApplyPaneLayout { action_id, .. } => {
                if !allow_pane_layout {
                    return Err(
                        "handler requested pane layout without API v18 persistent pane-layout authority"
                            .into(),
                    );
                }
                host_operations += 1;
                validate_text("pane layout action id", action_id, MAX_ACTION_ID_BYTES)?;
            }
            StructuredResponseAction::CloseWindow { action_id } => {
                if !allow_window_close {
                    return Err(
                        "handler requested window closure without API v19 command-scoped persistent window-close authority"
                            .into(),
                    );
                }
                host_operations += 1;
                validate_text("window closure action id", action_id, MAX_ACTION_ID_BYTES)?;
            }
        }
    }
    if host_operations > 1 {
        return Err("handler response contains more than one host operation".into());
    }
    Ok(actions)
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

fn validate_terminal_input_action(
    label: &str,
    action_id: &str,
    text: &str,
    enter: bool,
) -> Result<(), String> {
    validate_text(
        &format!("{label} action id"),
        action_id,
        MAX_ACTION_ID_BYTES,
    )?;
    if text.len() > MAX_TERMINAL_INPUT_BYTES {
        return Err(format!(
            "plugin {label} is limited to {MAX_TERMINAL_INPUT_BYTES} UTF-8 bytes"
        ));
    }
    if text.is_empty() && !enter {
        return Err(format!("plugin {label} must include text or enter"));
    }
    if !crate::control::protocol::valid_control_input_text(text) {
        return Err(format!(
            "plugin {label} permits printable UTF-8, tab, and newline only"
        ));
    }
    Ok(())
}

fn bounded_error(value: &str) -> String {
    let mut output = String::new();
    for character in value.chars().filter(|character| !character.is_control()) {
        if output.len() + character.len_utf8() > 1024 {
            break;
        }
        output.push(character);
    }
    if output.is_empty() {
        "structured plugin handler failed".into()
    } else {
        output
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn run_fixture(
        invocation: &StructuredHandlerInvocation,
    ) -> Result<Vec<StructuredResponseAction>, String> {
        run_handler(invocation, &AtomicBool::new(false), &AtomicU64::new(1), 1)
    }

    #[cfg(unix)]
    fn persistent_fixture(plugin_id: &str, script: &str) -> StructuredHandlerInvocation {
        StructuredHandlerInvocation {
            plugin_id: plugin_id.into(),
            program: "/bin/sh".into(),
            args: vec!["-c".into(), script.into()],
            event_json: r#"{"schema_version":1,"event":{"type":"app-ready"}}"#.into(),
            allow_ui_actions: true,
            allow_workspace_query: false,
            allow_workspace_actions: false,
            allow_workspace_topology: false,
            allow_terminal_query: false,
            allow_pane_create: false,
            allow_tab_create: false,
            allow_pane_close: false,
            allow_tab_close: false,
            allow_pane_move: false,
            allow_terminal_input: false,
            allow_pane_layout: false,
            allow_window_close: false,
            allow_process_query: false,
            allow_process_actions: false,
            allow_process_terminal_text: false,
            allow_process_terminal_input: false,
            allow_process_tab_create: false,
            mode: StructuredHandlerMode::Persistent,
        }
    }

    #[test]
    fn response_schema_permissions_and_bounds_fail_closed() {
        let valid = br#"{"schema_version":1,"actions":[{"type":"show-message","message":"done"},{"type":"show-notice","title":"Result","lines":["one","two"]}]}"#;
        assert_eq!(
            parse_response(valid, true, false, false, false, false)
                .unwrap()
                .len(),
            2
        );
        assert!(
            parse_response(valid, false, false, false, false, false)
                .unwrap_err()
                .contains("ui-actions")
        );
        assert!(
            parse_response(
                br#"{"schema_version":2,"actions":[]}"#,
                true,
                false,
                false,
                false,
                false,
            )
            .unwrap_err()
            .contains("unsupported")
        );
        assert!(
            parse_response(
                format!(
                    "{{\"schema_version\":1,\"actions\":[{{\"type\":\"show-message\",\"message\":{:?}}}]}}",
                    "x".repeat(MAX_MESSAGE_BYTES + 1)
                )
                .as_bytes(),
                true,
                false,
                false,
                false,
                false,
            )
            .is_err()
        );
        assert!(
            parse_response(
                br#"{"schema_version":1,"actions":[{"type":"show-message","message":"two\nlines"}]}"#,
                true,
                false,
                false,
                false,
                false,
            )
            .unwrap_err()
            .contains("control-safe")
        );
    }

    #[test]
    fn workspace_query_actions_are_independently_gated_and_non_chainable() {
        let query = br#"{"schema_version":2,"request_id":9,"actions":[{"type":"query-workspace","query_id":"refresh-1"}]}"#;
        assert_eq!(
            parse_persistent_response(query, false, true, false, false, false, 9).unwrap(),
            vec![StructuredResponseAction::QueryWorkspace {
                query_id: "refresh-1".into()
            }]
        );
        assert!(
            parse_persistent_response(query, true, false, false, false, false, 9)
                .unwrap_err()
                .contains("workspace-query authority")
        );
        let duplicate = br#"{"schema_version":2,"request_id":9,"actions":[{"type":"query-workspace","query_id":"one"},{"type":"query-workspace","query_id":"two"}]}"#;
        assert!(
            parse_persistent_response(duplicate, false, true, false, false, false, 9)
                .unwrap_err()
                .contains("more than one")
        );
        let unsafe_id = br#"{"schema_version":2,"request_id":9,"actions":[{"type":"query-workspace","query_id":"bad\nquery"}]}"#;
        assert!(
            parse_persistent_response(unsafe_id, false, true, false, false, false, 9)
                .unwrap_err()
                .contains("control-safe")
        );
    }

    #[test]
    fn process_query_actions_require_v20_authority_and_one_host_operation() {
        let query = br#"{"schema_version":2,"request_id":9,"actions":[{"type":"query-process","query_id":"all-windows"}]}"#;
        assert_eq!(
            parse_persistent_response_with_window_close(
                query, false, false, false, false, false, false, false, false, false, false, false,
                false, false, true, false, false, false, false, 9,
            )
            .unwrap(),
            vec![StructuredResponseAction::QueryProcess {
                query_id: "all-windows".into(),
            }]
        );
        assert!(
            parse_persistent_response(query, false, false, false, false, false, 9)
                .unwrap_err()
                .contains("API v20")
        );

        let duplicate = br#"{"schema_version":2,"request_id":9,"actions":[{"type":"query-process","query_id":"one"},{"type":"query-process","query_id":"two"}]}"#;
        assert!(
            parse_persistent_response_with_window_close(
                duplicate, false, false, false, false, false, false, false, false, false, false,
                false, false, false, true, false, false, false, false, 9,
            )
            .unwrap_err()
            .contains("more than one host operation")
        );

        let mixed = br#"{"schema_version":2,"request_id":9,"actions":[{"type":"query-process","query_id":"all"},{"type":"query-workspace","query_id":"current"}]}"#;
        assert!(
            parse_persistent_response_with_window_close(
                mixed, false, true, false, false, false, false, false, false, false, false, false,
                false, false, true, false, false, false, false, 9,
            )
            .unwrap_err()
            .contains("more than one host operation")
        );

        let unsafe_id = br#"{"schema_version":2,"request_id":9,"actions":[{"type":"query-process","query_id":"two\nlines"}]}"#;
        assert!(
            parse_persistent_response_with_window_close(
                unsafe_id, false, false, false, false, false, false, false, false, false, false,
                false, false, false, true, false, false, false, false, 9,
            )
            .unwrap_err()
            .contains("control-safe")
        );
    }

    fn parse_process_action_response(
        bytes: &[u8],
        allow_process_query: bool,
        allow_process_actions: bool,
    ) -> Result<Vec<StructuredResponseAction>, String> {
        parse_persistent_response_with_window_close(
            bytes,
            false, // ui actions
            false, // workspace query
            false, // workspace actions
            false, // workspace topology
            false, // terminal query
            false, // pane create
            false, // tab create
            false, // pane close
            false, // tab close
            false, // pane move
            false, // terminal input
            false, // pane layout
            false, // window close
            allow_process_query,
            allow_process_actions,
            false, // process terminal text
            false, // process terminal input
            false, // process tab create
            9,
        )
    }

    fn parse_process_terminal_response(
        bytes: &[u8],
        allow_process_query: bool,
        allow_process_actions: bool,
        allow_process_terminal_text: bool,
    ) -> Result<Vec<StructuredResponseAction>, String> {
        parse_persistent_response_with_window_close(
            bytes,
            false, // ui actions
            false, // workspace query
            false, // workspace actions
            false, // workspace topology
            false, // terminal query
            false, // pane create
            false, // tab create
            false, // pane close
            false, // tab close
            false, // pane move
            false, // terminal input
            false, // pane layout
            false, // window close
            allow_process_query,
            allow_process_actions,
            allow_process_terminal_text,
            false, // process terminal input
            false, // process tab create
            9,
        )
    }

    fn parse_process_input_response(
        bytes: &[u8],
        allow_process_query: bool,
        allow_process_actions: bool,
        allow_process_terminal_text: bool,
        allow_process_terminal_input: bool,
    ) -> Result<Vec<StructuredResponseAction>, String> {
        parse_persistent_response_with_window_close(
            bytes,
            false, // ui actions
            false, // workspace query
            false, // workspace actions
            false, // workspace topology
            false, // terminal query
            false, // pane create
            false, // tab create
            false, // pane close
            false, // tab close
            false, // pane move
            false, // terminal input
            false, // pane layout
            false, // window close
            allow_process_query,
            allow_process_actions,
            allow_process_terminal_text,
            allow_process_terminal_input,
            false, // process tab create
            9,
        )
    }

    fn parse_process_tab_create_response(
        bytes: &[u8],
        allow_process_query: bool,
        allow_process_tab_create: bool,
    ) -> Result<Vec<StructuredResponseAction>, String> {
        parse_persistent_response_with_window_close(
            bytes,
            false, // ui actions
            false, // workspace query
            false, // workspace actions
            false, // workspace topology
            false, // terminal query
            false, // pane create
            false, // tab create
            false, // pane close
            false, // tab close
            false, // pane move
            false, // terminal input
            false, // pane layout
            false, // window close
            allow_process_query,
            false, // process actions
            false, // process terminal text
            false, // process terminal input
            allow_process_tab_create,
            9,
        )
    }

    #[test]
    fn process_navigation_actions_require_v21_authority_and_one_host_operation() {
        let select = br#"{"schema_version":2,"request_id":9,"actions":[{"type":"select-process-tab","action_id":"select-results","window_id":7,"tab_id":42}]}"#;
        assert_eq!(
            parse_process_action_response(select, false, true).unwrap(),
            vec![StructuredResponseAction::SelectProcessTab {
                action_id: "select-results".into(),
                window_id: 7,
                tab_id: 42,
            }]
        );

        let focus = br#"{"schema_version":2,"request_id":9,"actions":[{"type":"focus-process-pane","action_id":"focus-shell","window_id":7,"tab_id":42,"pane_id":3}]}"#;
        assert_eq!(
            parse_process_action_response(focus, false, true).unwrap(),
            vec![StructuredResponseAction::FocusProcessPane {
                action_id: "focus-shell".into(),
                window_id: 7,
                tab_id: 42,
                pane_id: 3,
            }]
        );
        assert!(
            parse_process_action_response(select, false, false)
                .unwrap_err()
                .contains("API v21")
        );

        let duplicate = br#"{"schema_version":2,"request_id":9,"actions":[{"type":"select-process-tab","action_id":"one","window_id":7,"tab_id":42},{"type":"select-process-tab","action_id":"two","window_id":8,"tab_id":43}]}"#;
        assert!(
            parse_process_action_response(duplicate, false, true)
                .unwrap_err()
                .contains("more than one host operation")
        );

        let mixed = br#"{"schema_version":2,"request_id":9,"actions":[{"type":"query-process","query_id":"inventory"},{"type":"focus-process-pane","action_id":"focus","window_id":7,"tab_id":42,"pane_id":3}]}"#;
        assert!(
            parse_process_action_response(mixed, true, true)
                .unwrap_err()
                .contains("more than one host operation")
        );

        let unsafe_id = br#"{"schema_version":2,"request_id":9,"actions":[{"type":"select-process-tab","action_id":"two\nlines","window_id":7,"tab_id":42}]}"#;
        assert!(
            parse_process_action_response(unsafe_id, false, true)
                .unwrap_err()
                .contains("control-safe")
        );

        let activation = br#"{"schema_version":2,"request_id":9,"actions":[{"type":"select-process-tab","action_id":"select","window_id":7,"tab_id":42,"activate":true}]}"#;
        assert!(
            parse_process_action_response(activation, false, true)
                .unwrap_err()
                .contains("unknown field")
        );

        let malformed_id = br#"{"schema_version":2,"request_id":9,"actions":[{"type":"focus-process-pane","action_id":"focus","window_id":"7","tab_id":42,"pane_id":3}]}"#;
        assert!(parse_process_action_response(malformed_id, false, true).is_err());

        let unsupported = br#"{"schema_version":2,"request_id":9,"actions":[{"type":"activate-process-window","action_id":"activate","window_id":7}]}"#;
        assert!(parse_process_action_response(unsupported, false, true).is_err());
    }

    #[test]
    fn process_terminal_queries_require_v22_authority_and_strict_bounded_shape() {
        let query = br#"{"schema_version":2,"request_id":9,"actions":[{"type":"query-process-terminal","query_id":"sibling-output","window_id":7,"tab_id":42,"pane_id":3,"history_lines":20,"max_bytes":2048}]}"#;
        assert_eq!(
            parse_process_terminal_response(query, false, false, true).unwrap(),
            vec![StructuredResponseAction::QueryProcessTerminal {
                query_id: "sibling-output".into(),
                window_id: 7,
                tab_id: 42,
                pane_id: 3,
                history_lines: 20,
                max_bytes: 2048,
            }]
        );
        assert!(
            parse_process_terminal_response(query, true, true, false)
                .unwrap_err()
                .contains("API v22")
        );

        let excessive_history = br#"{"schema_version":2,"request_id":9,"actions":[{"type":"query-process-terminal","query_id":"text","window_id":7,"tab_id":42,"pane_id":3,"history_lines":257,"max_bytes":2048}]}"#;
        assert!(
            parse_process_terminal_response(excessive_history, false, false, true)
                .unwrap_err()
                .contains("limited to 256 lines")
        );
        for max_bytes in [0, MAX_TERMINAL_QUERY_BYTES + 1] {
            let invalid = format!(
                "{{\"schema_version\":2,\"request_id\":9,\"actions\":[{{\"type\":\"query-process-terminal\",\"query_id\":\"text\",\"window_id\":7,\"tab_id\":42,\"pane_id\":3,\"history_lines\":0,\"max_bytes\":{max_bytes}}}]}}"
            );
            assert!(
                parse_process_terminal_response(invalid.as_bytes(), false, false, true)
                    .unwrap_err()
                    .contains("bytes must be between")
            );
        }

        let unsafe_id = br#"{"schema_version":2,"request_id":9,"actions":[{"type":"query-process-terminal","query_id":"two\nlines","window_id":7,"tab_id":42,"pane_id":3,"history_lines":0,"max_bytes":64}]}"#;
        assert!(
            parse_process_terminal_response(unsafe_id, false, false, true)
                .unwrap_err()
                .contains("control-safe")
        );
        let unknown = br#"{"schema_version":2,"request_id":9,"actions":[{"type":"query-process-terminal","query_id":"text","window_id":7,"tab_id":42,"pane_id":3,"history_lines":0,"max_bytes":64,"activate":true}]}"#;
        assert!(
            parse_process_terminal_response(unknown, false, false, true)
                .unwrap_err()
                .contains("unknown field")
        );
        let malformed = br#"{"schema_version":2,"request_id":9,"actions":[{"type":"query-process-terminal","query_id":"text","window_id":"7","tab_id":42,"pane_id":3,"history_lines":0,"max_bytes":64}]}"#;
        assert!(parse_process_terminal_response(malformed, false, false, true).is_err());

        let mixed = br#"{"schema_version":2,"request_id":9,"actions":[{"type":"query-process","query_id":"inventory"},{"type":"query-process-terminal","query_id":"text","window_id":7,"tab_id":42,"pane_id":3,"history_lines":0,"max_bytes":64}]}"#;
        assert!(
            parse_process_terminal_response(mixed, true, false, true)
                .unwrap_err()
                .contains("more than one host operation")
        );
        let duplicate = br#"{"schema_version":2,"request_id":9,"actions":[{"type":"query-process-terminal","query_id":"one","window_id":7,"tab_id":42,"pane_id":3,"history_lines":0,"max_bytes":64},{"type":"query-process-terminal","query_id":"two","window_id":7,"tab_id":42,"pane_id":3,"history_lines":0,"max_bytes":64}]}"#;
        assert!(
            parse_process_terminal_response(duplicate, false, false, true)
                .unwrap_err()
                .contains("more than one host operation")
        );
    }

    #[test]
    fn process_terminal_input_requires_v23_authority_and_strict_bounded_shape() {
        let send = br#"{"schema_version":2,"request_id":9,"actions":[{"type":"send-process-text","action_id":"draft-check","window_id":7,"tab_id":42,"pane_id":3,"text":"cargo test\n--workspace","paste":true,"enter":false}]}"#;
        assert_eq!(
            parse_process_input_response(send, false, false, false, true).unwrap(),
            vec![StructuredResponseAction::SendProcessText {
                action_id: "draft-check".into(),
                window_id: 7,
                tab_id: 42,
                pane_id: 3,
                text: "cargo test\n--workspace".into(),
                paste: true,
                enter: false,
            }]
        );
        assert!(
            parse_process_input_response(send, true, true, true, false)
                .unwrap_err()
                .contains("API v23")
        );

        let unknown = br#"{"schema_version":2,"request_id":9,"actions":[{"type":"send-process-text","action_id":"draft","window_id":7,"tab_id":42,"pane_id":3,"text":"safe","paste":false,"enter":false,"activate":true}]}"#;
        assert!(
            parse_process_input_response(unknown, false, false, false, true)
                .unwrap_err()
                .contains("unknown field")
        );
        let malformed = br#"{"schema_version":2,"request_id":9,"actions":[{"type":"send-process-text","action_id":"draft","window_id":"7","tab_id":42,"pane_id":3,"text":"safe","paste":false,"enter":false}]}"#;
        assert!(parse_process_input_response(malformed, false, false, false, true).is_err());
        let unsafe_id = br#"{"schema_version":2,"request_id":9,"actions":[{"type":"send-process-text","action_id":"two\nlines","window_id":7,"tab_id":42,"pane_id":3,"text":"safe","paste":false,"enter":false}]}"#;
        assert!(
            parse_process_input_response(unsafe_id, false, false, false, true)
                .unwrap_err()
                .contains("control-safe")
        );
        let escape = br#"{"schema_version":2,"request_id":9,"actions":[{"type":"send-process-text","action_id":"escape","window_id":7,"tab_id":42,"pane_id":3,"text":"\u001b[31m","paste":false,"enter":false}]}"#;
        assert!(
            parse_process_input_response(escape, false, false, false, true)
                .unwrap_err()
                .contains("printable UTF-8")
        );
        let empty = br#"{"schema_version":2,"request_id":9,"actions":[{"type":"send-process-text","action_id":"empty","window_id":7,"tab_id":42,"pane_id":3,"text":"","paste":false,"enter":false}]}"#;
        assert!(
            parse_process_input_response(empty, false, false, false, true)
                .unwrap_err()
                .contains("text or enter")
        );
        let oversized = serde_json::to_vec(&serde_json::json!({
            "schema_version": 2,
            "request_id": 9,
            "actions": [{
                "type": "send-process-text",
                "action_id": "oversized",
                "window_id": 7,
                "tab_id": 42,
                "pane_id": 3,
                "text": "x".repeat(MAX_TERMINAL_INPUT_BYTES + 1),
                "paste": false,
                "enter": false,
            }],
        }))
        .unwrap();
        assert!(
            parse_process_input_response(&oversized, false, false, false, true)
                .unwrap_err()
                .contains("limited")
        );

        let mixed = br#"{"schema_version":2,"request_id":9,"actions":[{"type":"query-process","query_id":"inventory"},{"type":"send-process-text","action_id":"draft","window_id":7,"tab_id":42,"pane_id":3,"text":"safe","paste":false,"enter":false}]}"#;
        assert!(
            parse_process_input_response(mixed, true, false, false, true)
                .unwrap_err()
                .contains("more than one host operation")
        );
        let duplicate = br#"{"schema_version":2,"request_id":9,"actions":[{"type":"send-process-text","action_id":"one","window_id":7,"tab_id":42,"pane_id":3,"text":"one","paste":false,"enter":false},{"type":"send-process-text","action_id":"two","window_id":7,"tab_id":42,"pane_id":3,"text":"two","paste":false,"enter":false}]}"#;
        assert!(
            parse_process_input_response(duplicate, false, false, false, true)
                .unwrap_err()
                .contains("more than one host operation")
        );
    }

    #[test]
    fn process_tab_creation_requires_v24_authority_and_strict_bounded_shape() {
        let create = br#"{"schema_version":2,"request_id":9,"actions":[{"type":"create-process-tab","action_id":"new-sibling-task","window_id":7}]}"#;
        assert_eq!(
            parse_process_tab_create_response(create, false, true).unwrap(),
            vec![StructuredResponseAction::CreateProcessTab {
                action_id: "new-sibling-task".into(),
                window_id: 7,
            }]
        );
        assert!(
            parse_process_tab_create_response(create, true, false)
                .unwrap_err()
                .contains("API v24")
        );

        let unknown = br#"{"schema_version":2,"request_id":9,"actions":[{"type":"create-process-tab","action_id":"new","window_id":7,"activate":true}]}"#;
        assert!(
            parse_process_tab_create_response(unknown, false, true)
                .unwrap_err()
                .contains("unknown field")
        );
        let malformed = br#"{"schema_version":2,"request_id":9,"actions":[{"type":"create-process-tab","action_id":"new","window_id":"7"}]}"#;
        assert!(parse_process_tab_create_response(malformed, false, true).is_err());
        let unsafe_id = br#"{"schema_version":2,"request_id":9,"actions":[{"type":"create-process-tab","action_id":"two\nlines","window_id":7}]}"#;
        assert!(
            parse_process_tab_create_response(unsafe_id, false, true)
                .unwrap_err()
                .contains("control-safe")
        );

        let mixed = br#"{"schema_version":2,"request_id":9,"actions":[{"type":"query-process","query_id":"inventory"},{"type":"create-process-tab","action_id":"new","window_id":7}]}"#;
        assert!(
            parse_process_tab_create_response(mixed, true, true)
                .unwrap_err()
                .contains("more than one host operation")
        );
        let duplicate = br#"{"schema_version":2,"request_id":9,"actions":[{"type":"create-process-tab","action_id":"one","window_id":7},{"type":"create-process-tab","action_id":"two","window_id":7}]}"#;
        assert!(
            parse_process_tab_create_response(duplicate, false, true)
                .unwrap_err()
                .contains("more than one host operation")
        );
    }

    #[test]
    fn terminal_queries_are_independently_gated_bounded_and_single_operation() {
        let query = br#"{"schema_version":2,"request_id":9,"actions":[{"type":"query-terminal","query_id":"visible-text","tab_id":42,"pane_id":7,"history_lines":20,"max_bytes":2048}]}"#;
        assert_eq!(
            parse_persistent_response(query, false, false, false, false, true, 9).unwrap(),
            vec![StructuredResponseAction::QueryTerminal {
                query_id: "visible-text".into(),
                tab_id: 42,
                pane_id: 7,
                history_lines: 20,
                max_bytes: 2048,
            }]
        );
        assert!(
            parse_persistent_response(query, false, false, false, false, false, 9)
                .unwrap_err()
                .contains("terminal-query authority")
        );

        let excessive_history = br#"{"schema_version":2,"request_id":9,"actions":[{"type":"query-terminal","query_id":"visible-text","tab_id":42,"pane_id":7,"history_lines":257,"max_bytes":2048}]}"#;
        assert!(
            parse_persistent_response(excessive_history, false, false, false, false, true, 9)
                .unwrap_err()
                .contains("limited to 256 lines")
        );
        for max_bytes in [0, MAX_TERMINAL_QUERY_BYTES + 1] {
            let invalid_bytes = format!(
                "{{\"schema_version\":2,\"request_id\":9,\"actions\":[{{\"type\":\"query-terminal\",\"query_id\":\"visible-text\",\"tab_id\":42,\"pane_id\":7,\"history_lines\":0,\"max_bytes\":{max_bytes}}}]}}"
            );
            assert!(
                parse_persistent_response(
                    invalid_bytes.as_bytes(),
                    false,
                    false,
                    false,
                    false,
                    true,
                    9,
                )
                .unwrap_err()
                .contains("bytes must be between")
            );
        }

        let query_and_workspace = br#"{"schema_version":2,"request_id":9,"actions":[{"type":"query-workspace","query_id":"inventory"},{"type":"query-terminal","query_id":"visible-text","tab_id":42,"pane_id":7,"history_lines":0,"max_bytes":2048}]}"#;
        assert!(
            parse_persistent_response(query_and_workspace, false, true, false, false, true, 9)
                .unwrap_err()
                .contains("more than one host operation")
        );
        let unsafe_id = br#"{"schema_version":2,"request_id":9,"actions":[{"type":"query-terminal","query_id":"bad\nquery","tab_id":42,"pane_id":7,"history_lines":0,"max_bytes":2048}]}"#;
        assert!(
            parse_persistent_response(unsafe_id, false, false, false, false, true, 9)
                .unwrap_err()
                .contains("control-safe")
        );
    }

    #[test]
    fn workspace_actions_are_independently_gated_correlated_and_single_operation() {
        let select = br#"{"schema_version":2,"request_id":9,"actions":[{"type":"select-tab","action_id":"nav-1","tab_id":42}]}"#;
        assert_eq!(
            parse_persistent_response(select, false, false, true, false, false, 9).unwrap(),
            vec![StructuredResponseAction::SelectTab {
                action_id: "nav-1".into(),
                tab_id: 42,
            }]
        );
        assert!(
            parse_persistent_response(select, true, true, false, false, false, 9)
                .unwrap_err()
                .contains("workspace-actions authority")
        );

        let focus = br#"{"schema_version":2,"request_id":9,"actions":[{"type":"focus-pane","action_id":"focus-1","tab_id":42,"pane_id":7}]}"#;
        assert_eq!(
            parse_persistent_response(focus, false, false, true, false, false, 9).unwrap(),
            vec![StructuredResponseAction::FocusPane {
                action_id: "focus-1".into(),
                tab_id: 42,
                pane_id: 7,
            }]
        );

        let query_and_action = br#"{"schema_version":2,"request_id":9,"actions":[{"type":"query-workspace","query_id":"inventory"},{"type":"select-tab","action_id":"nav-2","tab_id":42}]}"#;
        assert!(
            parse_persistent_response(query_and_action, false, true, true, false, false, 9)
                .unwrap_err()
                .contains("more than one host operation")
        );
        let unsafe_id = br#"{"schema_version":2,"request_id":9,"actions":[{"type":"select-tab","action_id":"bad\naction","tab_id":42}]}"#;
        assert!(
            parse_persistent_response(unsafe_id, false, false, true, false, false, 9)
                .unwrap_err()
                .contains("control-safe")
        );
    }

    #[test]
    fn workspace_topology_actions_are_independently_gated_bounded_and_single_operation() {
        let move_tab = br#"{"schema_version":2,"request_id":9,"actions":[{"type":"move-tab","action_id":"order-1","tab_id":42,"destination_index":3}]}"#;
        assert_eq!(
            parse_persistent_response(move_tab, false, false, false, true, false, 9).unwrap(),
            vec![StructuredResponseAction::MoveTab {
                action_id: "order-1".into(),
                tab_id: 42,
                destination_index: 3,
            }]
        );
        assert!(
            parse_persistent_response(move_tab, true, true, true, false, false, 9)
                .unwrap_err()
                .contains("workspace-topology authority")
        );

        let resize = br#"{"schema_version":2,"request_id":9,"actions":[{"type":"resize-divider","action_id":"ratio-1","tab_id":42,"divider_id":7,"ratio_millis":650}]}"#;
        assert_eq!(
            parse_persistent_response(resize, false, false, false, true, false, 9).unwrap(),
            vec![StructuredResponseAction::ResizeDivider {
                action_id: "ratio-1".into(),
                tab_id: 42,
                divider_id: 7,
                ratio_millis: 650,
            }]
        );

        let query_and_resize = br#"{"schema_version":2,"request_id":9,"actions":[{"type":"query-workspace","query_id":"inventory"},{"type":"resize-divider","action_id":"ratio-2","tab_id":42,"divider_id":7,"ratio_millis":650}]}"#;
        assert!(
            parse_persistent_response(query_and_resize, false, true, false, true, false, 9)
                .unwrap_err()
                .contains("more than one host operation")
        );
        let navigation_and_move = br#"{"schema_version":2,"request_id":9,"actions":[{"type":"select-tab","action_id":"nav","tab_id":42},{"type":"move-tab","action_id":"order","tab_id":42,"destination_index":3}]}"#;
        assert!(
            parse_persistent_response(navigation_and_move, false, false, true, true, false, 9)
                .unwrap_err()
                .contains("more than one host operation")
        );

        let invalid_ratio = br#"{"schema_version":2,"request_id":9,"actions":[{"type":"resize-divider","action_id":"ratio","tab_id":42,"divider_id":7,"ratio_millis":99}]}"#;
        assert!(
            parse_persistent_response(invalid_ratio, false, false, false, true, false, 9)
                .unwrap_err()
                .contains("between 100 and 900")
        );
        let invalid_index = format!(
            "{{\"schema_version\":2,\"request_id\":9,\"actions\":[{{\"type\":\"move-tab\",\"action_id\":\"order\",\"tab_id\":42,\"destination_index\":{}}}]}}",
            crate::control::protocol::MAX_TABS_PER_WINDOW
        );
        assert!(
            parse_persistent_response(
                invalid_index.as_bytes(),
                false,
                false,
                false,
                true,
                false,
                9,
            )
            .unwrap_err()
            .contains("destination index")
        );
    }

    #[test]
    fn pane_creation_is_typed_independently_gated_and_single_operation() {
        let create = br#"{"schema_version":2,"request_id":9,"actions":[{"type":"create-pane","action_id":"split-1","tab_id":42,"pane_id":7,"direction":"horizontal"}]}"#;
        assert_eq!(
            parse_persistent_response_with_host_creation(
                create, false, false, false, false, false, true, false, false, false, 9,
            )
            .unwrap(),
            vec![StructuredResponseAction::CreatePane {
                action_id: "split-1".into(),
                tab_id: 42,
                pane_id: 7,
                direction: crate::control::protocol::SplitDirection::Horizontal,
            }]
        );
        assert!(
            parse_persistent_response(create, false, false, false, false, false, 9)
                .unwrap_err()
                .contains("pane-create authority")
        );

        let invalid_direction = br#"{"schema_version":2,"request_id":9,"actions":[{"type":"create-pane","action_id":"split-1","tab_id":42,"pane_id":7,"direction":"diagonal"}]}"#;
        assert!(
            parse_persistent_response_with_host_creation(
                invalid_direction,
                false,
                false,
                false,
                false,
                false,
                true,
                false,
                false,
                false,
                9,
            )
            .unwrap_err()
            .contains("invalid handler response JSON")
        );
        let unknown_field = br#"{"schema_version":2,"request_id":9,"actions":[{"type":"create-pane","action_id":"split-1","tab_id":42,"pane_id":7,"direction":"vertical","command":"forbidden"}]}"#;
        assert!(
            parse_persistent_response_with_host_creation(
                unknown_field,
                false,
                false,
                false,
                false,
                false,
                true,
                false,
                false,
                false,
                9,
            )
            .unwrap_err()
            .contains("unknown field")
        );
        let unsafe_id = br#"{"schema_version":2,"request_id":9,"actions":[{"type":"create-pane","action_id":"bad\naction","tab_id":42,"pane_id":7,"direction":"vertical"}]}"#;
        assert!(
            parse_persistent_response_with_host_creation(
                unsafe_id, false, false, false, false, false, true, false, false, false, 9,
            )
            .unwrap_err()
            .contains("control-safe")
        );
        let query_and_create = br#"{"schema_version":2,"request_id":9,"actions":[{"type":"query-workspace","query_id":"inventory"},{"type":"create-pane","action_id":"split-2","tab_id":42,"pane_id":7,"direction":"vertical"}]}"#;
        assert!(
            parse_persistent_response_with_host_creation(
                query_and_create,
                false,
                true,
                false,
                false,
                false,
                true,
                false,
                false,
                false,
                9,
            )
            .unwrap_err()
            .contains("more than one host operation")
        );
    }

    #[test]
    fn tab_creation_is_independently_gated_strict_and_single_operation() {
        let create = br#"{"schema_version":2,"request_id":9,"actions":[{"type":"create-tab","action_id":"new-task"}]}"#;
        assert_eq!(
            parse_persistent_response_with_host_creation(
                create, false, false, false, false, false, false, true, false, false, 9,
            )
            .unwrap(),
            vec![StructuredResponseAction::CreateTab {
                action_id: "new-task".into(),
            }]
        );
        assert!(
            parse_persistent_response(create, false, false, false, false, false, 9)
                .unwrap_err()
                .contains("tab-create authority")
        );

        let unknown_field = br#"{"schema_version":2,"request_id":9,"actions":[{"type":"create-tab","action_id":"new-task","command":"forbidden"}]}"#;
        assert!(
            parse_persistent_response_with_host_creation(
                unknown_field,
                false,
                false,
                false,
                false,
                false,
                false,
                true,
                false,
                false,
                9,
            )
            .unwrap_err()
            .contains("unknown field")
        );
        let unsafe_id = br#"{"schema_version":2,"request_id":9,"actions":[{"type":"create-tab","action_id":"bad\naction"}]}"#;
        assert!(
            parse_persistent_response_with_host_creation(
                unsafe_id, false, false, false, false, false, false, true, false, false, 9,
            )
            .unwrap_err()
            .contains("control-safe")
        );
        let oversized_id = serde_json::json!({
            "schema_version": 2,
            "request_id": 9,
            "actions": [{
                "type": "create-tab",
                "action_id": "x".repeat(MAX_ACTION_ID_BYTES + 1),
            }],
        });
        assert!(
            parse_persistent_response_with_host_creation(
                &serde_json::to_vec(&oversized_id).unwrap(),
                false,
                false,
                false,
                false,
                false,
                false,
                true,
                false,
                false,
                9,
            )
            .unwrap_err()
            .contains("within 64 bytes")
        );
        let query_and_create = br#"{"schema_version":2,"request_id":9,"actions":[{"type":"query-workspace","query_id":"inventory"},{"type":"create-tab","action_id":"new-task"}]}"#;
        assert!(
            parse_persistent_response_with_host_creation(
                query_and_create,
                false,
                true,
                false,
                false,
                false,
                false,
                true,
                false,
                false,
                9,
            )
            .unwrap_err()
            .contains("more than one host operation")
        );
    }

    #[test]
    fn pane_closure_is_independently_gated_strict_and_single_operation() {
        let close = br#"{"schema_version":2,"request_id":9,"actions":[{"type":"close-pane","action_id":"prune-1","tab_id":42,"pane_id":7}]}"#;
        assert_eq!(
            parse_persistent_response_with_host_creation(
                close, false, false, false, false, false, false, false, true, false, 9,
            )
            .unwrap(),
            vec![StructuredResponseAction::ClosePane {
                action_id: "prune-1".into(),
                tab_id: 42,
                pane_id: 7,
            }]
        );
        assert!(
            parse_persistent_response(close, false, false, false, false, false, 9)
                .unwrap_err()
                .contains("pane-close authority")
        );

        let process_per_event = br#"{"schema_version":1,"actions":[{"type":"close-pane","action_id":"prune-1","tab_id":42,"pane_id":7}]}"#;
        assert!(
            parse_response_with_host_creation(
                process_per_event,
                false,
                false,
                false,
                false,
                false,
                false,
                false,
                false,
                false,
            )
            .unwrap_err()
            .contains("pane-close authority")
        );
        let unknown_field = br#"{"schema_version":2,"request_id":9,"actions":[{"type":"close-pane","action_id":"prune-1","tab_id":42,"pane_id":7,"force":true}]}"#;
        assert!(
            parse_persistent_response_with_host_creation(
                unknown_field,
                false,
                false,
                false,
                false,
                false,
                false,
                false,
                true,
                false,
                9,
            )
            .unwrap_err()
            .contains("unknown field")
        );
        let unsafe_id = br#"{"schema_version":2,"request_id":9,"actions":[{"type":"close-pane","action_id":"bad\naction","tab_id":42,"pane_id":7}]}"#;
        assert!(
            parse_persistent_response_with_host_creation(
                unsafe_id, false, false, false, false, false, false, false, true, false, 9,
            )
            .unwrap_err()
            .contains("control-safe")
        );
        let query_and_close = br#"{"schema_version":2,"request_id":9,"actions":[{"type":"query-workspace","query_id":"inventory"},{"type":"close-pane","action_id":"prune-1","tab_id":42,"pane_id":7}]}"#;
        assert!(
            parse_persistent_response_with_host_creation(
                query_and_close,
                false,
                true,
                false,
                false,
                false,
                false,
                false,
                true,
                false,
                9,
            )
            .unwrap_err()
            .contains("more than one host operation")
        );
    }

    #[test]
    fn tab_closure_is_independently_gated_strict_and_single_operation() {
        let close = br#"{"schema_version":2,"request_id":9,"actions":[{"type":"close-tab","action_id":"retire-1","tab_id":42}]}"#;
        assert_eq!(
            parse_persistent_response_with_host_creation(
                close, false, false, false, false, false, false, false, false, true, 9,
            )
            .unwrap(),
            vec![StructuredResponseAction::CloseTab {
                action_id: "retire-1".into(),
                tab_id: 42,
            }]
        );
        assert!(
            parse_persistent_response(close, false, false, false, false, false, 9)
                .unwrap_err()
                .contains("tab-close authority")
        );

        let process_per_event = br#"{"schema_version":1,"actions":[{"type":"close-tab","action_id":"retire-1","tab_id":42}]}"#;
        assert!(
            parse_response_with_host_creation(
                process_per_event,
                false,
                false,
                false,
                false,
                false,
                false,
                false,
                false,
                false,
            )
            .unwrap_err()
            .contains("tab-close authority")
        );
        let unknown_field = br#"{"schema_version":2,"request_id":9,"actions":[{"type":"close-tab","action_id":"retire-1","tab_id":42,"force":true}]}"#;
        assert!(
            parse_persistent_response_with_host_creation(
                unknown_field,
                false,
                false,
                false,
                false,
                false,
                false,
                false,
                false,
                true,
                9,
            )
            .unwrap_err()
            .contains("unknown field")
        );
        let unsafe_id = br#"{"schema_version":2,"request_id":9,"actions":[{"type":"close-tab","action_id":"bad\naction","tab_id":42}]}"#;
        assert!(
            parse_persistent_response_with_host_creation(
                unsafe_id, false, false, false, false, false, false, false, false, true, 9,
            )
            .unwrap_err()
            .contains("control-safe")
        );
        let query_and_close = br#"{"schema_version":2,"request_id":9,"actions":[{"type":"query-workspace","query_id":"inventory"},{"type":"close-tab","action_id":"retire-1","tab_id":42}]}"#;
        assert!(
            parse_persistent_response_with_host_creation(
                query_and_close,
                false,
                true,
                false,
                false,
                false,
                false,
                false,
                false,
                true,
                9,
            )
            .unwrap_err()
            .contains("more than one host operation")
        );
    }

    #[test]
    fn pane_movement_is_typed_independently_gated_strict_and_single_operation() {
        let move_pane = br#"{"schema_version":2,"request_id":9,"actions":[{"type":"move-pane","action_id":"relocate-1","source_tab_id":42,"pane_id":7,"destination_tab_id":43,"target_pane_id":2,"direction":"horizontal"}]}"#;
        assert_eq!(
            parse_persistent_response_with_host_mutation(
                move_pane, false, false, false, false, false, false, false, false, false, true,
                false, false, 9,
            )
            .unwrap(),
            vec![StructuredResponseAction::MovePane {
                action_id: "relocate-1".into(),
                source_tab_id: 42,
                pane_id: 7,
                destination_tab_id: 43,
                target_pane_id: 2,
                direction: crate::control::protocol::SplitDirection::Horizontal,
            }]
        );
        assert!(
            parse_persistent_response(move_pane, false, false, false, false, false, 9)
                .unwrap_err()
                .contains("pane-move authority")
        );

        let invalid_direction = br#"{"schema_version":2,"request_id":9,"actions":[{"type":"move-pane","action_id":"relocate-1","source_tab_id":42,"pane_id":7,"destination_tab_id":43,"target_pane_id":2,"direction":"diagonal"}]}"#;
        assert!(
            parse_persistent_response_with_host_mutation(
                invalid_direction,
                false,
                false,
                false,
                false,
                false,
                false,
                false,
                false,
                false,
                true,
                false,
                false,
                9,
            )
            .unwrap_err()
            .contains("unknown variant")
        );
        let unknown_field = br#"{"schema_version":2,"request_id":9,"actions":[{"type":"move-pane","action_id":"relocate-1","source_tab_id":42,"pane_id":7,"destination_tab_id":43,"target_pane_id":2,"direction":"vertical","window_id":9}]}"#;
        assert!(
            parse_persistent_response_with_host_mutation(
                unknown_field,
                false,
                false,
                false,
                false,
                false,
                false,
                false,
                false,
                false,
                true,
                false,
                false,
                9,
            )
            .unwrap_err()
            .contains("unknown field")
        );
        let unsafe_id = br#"{"schema_version":2,"request_id":9,"actions":[{"type":"move-pane","action_id":"bad\naction","source_tab_id":42,"pane_id":7,"destination_tab_id":43,"target_pane_id":2,"direction":"vertical"}]}"#;
        assert!(
            parse_persistent_response_with_host_mutation(
                unsafe_id, false, false, false, false, false, false, false, false, false, true,
                false, false, 9,
            )
            .unwrap_err()
            .contains("control-safe")
        );
        let oversized_id = serde_json::json!({
            "schema_version": 2,
            "request_id": 9,
            "actions": [{
                "type": "move-pane",
                "action_id": "x".repeat(MAX_ACTION_ID_BYTES + 1),
                "source_tab_id": 42,
                "pane_id": 7,
                "destination_tab_id": 43,
                "target_pane_id": 2,
                "direction": "vertical",
            }],
        });
        assert!(
            parse_persistent_response_with_host_mutation(
                &serde_json::to_vec(&oversized_id).unwrap(),
                false,
                false,
                false,
                false,
                false,
                false,
                false,
                false,
                false,
                true,
                false,
                false,
                9,
            )
            .unwrap_err()
            .contains("within 64 bytes")
        );
        let query_and_move = br#"{"schema_version":2,"request_id":9,"actions":[{"type":"query-workspace","query_id":"inventory"},{"type":"move-pane","action_id":"relocate-1","source_tab_id":42,"pane_id":7,"destination_tab_id":43,"target_pane_id":2,"direction":"vertical"}]}"#;
        assert!(
            parse_persistent_response_with_host_mutation(
                query_and_move,
                false,
                true,
                false,
                false,
                false,
                false,
                false,
                false,
                false,
                true,
                false,
                false,
                9,
            )
            .unwrap_err()
            .contains("more than one host operation")
        );
    }

    #[test]
    fn terminal_input_is_bounded_independently_gated_strict_and_single_operation() {
        let send_text = br#"{"schema_version":2,"request_id":9,"actions":[{"type":"send-text","action_id":"draft-command","tab_id":42,"pane_id":7,"text":"cargo test\n--workspace","paste":true,"enter":false}]}"#;
        assert_eq!(
            parse_persistent_response_with_host_mutation(
                send_text, false, false, false, false, false, false, false, false, false, false,
                true, false, 9,
            )
            .unwrap(),
            vec![StructuredResponseAction::SendText {
                action_id: "draft-command".into(),
                tab_id: 42,
                pane_id: 7,
                text: "cargo test\n--workspace".into(),
                paste: true,
                enter: false,
            }]
        );
        assert!(
            parse_persistent_response(send_text, false, false, false, false, false, 9)
                .unwrap_err()
                .contains("terminal-input authority")
        );

        let unknown_field = br#"{"schema_version":2,"request_id":9,"actions":[{"type":"send-text","action_id":"draft-command","tab_id":42,"pane_id":7,"text":"cargo test","paste":false,"enter":false,"window_id":3}]}"#;
        assert!(
            parse_persistent_response_with_host_mutation(
                unknown_field,
                false,
                false,
                false,
                false,
                false,
                false,
                false,
                false,
                false,
                false,
                true,
                false,
                9,
            )
            .unwrap_err()
            .contains("unknown field")
        );

        let unsafe_id = br#"{"schema_version":2,"request_id":9,"actions":[{"type":"send-text","action_id":"bad\naction","tab_id":42,"pane_id":7,"text":"cargo test","paste":false,"enter":false}]}"#;
        assert!(
            parse_persistent_response_with_host_mutation(
                unsafe_id, false, false, false, false, false, false, false, false, false, false,
                true, false, 9,
            )
            .unwrap_err()
            .contains("control-safe")
        );

        let escape = br#"{"schema_version":2,"request_id":9,"actions":[{"type":"send-text","action_id":"bad-input","tab_id":42,"pane_id":7,"text":"\u001b[31m","paste":false,"enter":false}]}"#;
        assert!(
            parse_persistent_response_with_host_mutation(
                escape, false, false, false, false, false, false, false, false, false, false, true,
                false, 9,
            )
            .unwrap_err()
            .contains("printable UTF-8")
        );

        let empty = br#"{"schema_version":2,"request_id":9,"actions":[{"type":"send-text","action_id":"empty","tab_id":42,"pane_id":7,"text":"","paste":false,"enter":false}]}"#;
        assert!(
            parse_persistent_response_with_host_mutation(
                empty, false, false, false, false, false, false, false, false, false, false, true,
                false, 9,
            )
            .unwrap_err()
            .contains("include text or enter")
        );

        let oversized = serde_json::json!({
            "schema_version": 2,
            "request_id": 9,
            "actions": [{
                "type": "send-text",
                "action_id": "oversized",
                "tab_id": 42,
                "pane_id": 7,
                "text": "x".repeat(MAX_TERMINAL_INPUT_BYTES + 1),
                "paste": false,
                "enter": false,
            }],
        });
        assert!(
            parse_persistent_response_with_host_mutation(
                &serde_json::to_vec(&oversized).unwrap(),
                false,
                false,
                false,
                false,
                false,
                false,
                false,
                false,
                false,
                false,
                true,
                false,
                9,
            )
            .unwrap_err()
            .contains("limited to 8192")
        );

        let query_and_send = br#"{"schema_version":2,"request_id":9,"actions":[{"type":"query-workspace","query_id":"inventory"},{"type":"send-text","action_id":"draft-command","tab_id":42,"pane_id":7,"text":"cargo test","paste":false,"enter":false}]}"#;
        assert!(
            parse_persistent_response_with_host_mutation(
                query_and_send,
                false,
                true,
                false,
                false,
                false,
                false,
                false,
                false,
                false,
                false,
                true,
                false,
                9,
            )
            .unwrap_err()
            .contains("more than one host operation")
        );
    }

    #[test]
    fn pane_layout_is_typed_independently_gated_strict_and_single_operation() {
        let swap = br#"{"schema_version":2,"request_id":9,"actions":[{"type":"swap-panes","action_id":"swap-build-logs","tab_id":42,"first_pane_id":3,"second_pane_id":7}]}"#;
        assert_eq!(
            parse_persistent_response_with_host_mutation(
                swap, false, false, false, false, false, false, false, false, false, false, false,
                true, 9,
            )
            .unwrap(),
            vec![StructuredResponseAction::SwapPanes {
                action_id: "swap-build-logs".into(),
                tab_id: 42,
                first_pane_id: 3,
                second_pane_id: 7,
            }]
        );
        assert!(
            parse_persistent_response(swap, false, false, false, false, false, 9)
                .unwrap_err()
                .contains("pane-layout authority")
        );

        let balance = br#"{"schema_version":2,"request_id":9,"actions":[{"type":"balance-panes","action_id":"balance-build","tab_id":42}]}"#;
        assert_eq!(
            parse_persistent_response_with_host_mutation(
                balance, false, false, false, false, false, false, false, false, false, false,
                false, true, 9,
            )
            .unwrap(),
            vec![StructuredResponseAction::BalancePanes {
                action_id: "balance-build".into(),
                tab_id: 42,
            }]
        );

        let preset = br#"{"schema_version":2,"request_id":9,"actions":[{"type":"apply-pane-layout","action_id":"grid-build","tab_id":42,"preset":"grid"}]}"#;
        assert_eq!(
            parse_persistent_response_with_host_mutation(
                preset, false, false, false, false, false, false, false, false, false, false,
                false, true, 9,
            )
            .unwrap(),
            vec![StructuredResponseAction::ApplyPaneLayout {
                action_id: "grid-build".into(),
                tab_id: 42,
                preset: PluginPaneLayoutPreset::Grid,
            }]
        );

        let same = br#"{"schema_version":2,"request_id":9,"actions":[{"type":"swap-panes","action_id":"same","tab_id":42,"first_pane_id":3,"second_pane_id":3}]}"#;
        assert!(
            parse_persistent_response_with_host_mutation(
                same, false, false, false, false, false, false, false, false, false, false, false,
                true, 9,
            )
            .unwrap_err()
            .contains("distinct")
        );
        let invalid_preset = br#"{"schema_version":2,"request_id":9,"actions":[{"type":"apply-pane-layout","action_id":"grid-build","tab_id":42,"preset":"mosaic"}]}"#;
        assert!(
            parse_persistent_response_with_host_mutation(
                invalid_preset,
                false,
                false,
                false,
                false,
                false,
                false,
                false,
                false,
                false,
                false,
                false,
                true,
                9,
            )
            .unwrap_err()
            .contains("unknown variant")
        );
        let unknown_fields = [
            serde_json::json!({"type":"swap-panes","action_id":"swap","tab_id":42,"first_pane_id":3,"second_pane_id":7,"window_id":3}),
            serde_json::json!({"type":"balance-panes","action_id":"balance","tab_id":42,"window_id":3}),
            serde_json::json!({"type":"apply-pane-layout","action_id":"grid","tab_id":42,"preset":"grid","window_id":3}),
        ];
        for action in unknown_fields {
            let response = serde_json::json!({
                "schema_version": 2,
                "request_id": 9,
                "actions": [action],
            });
            assert!(
                parse_persistent_response_with_host_mutation(
                    &serde_json::to_vec(&response).unwrap(),
                    false,
                    false,
                    false,
                    false,
                    false,
                    false,
                    false,
                    false,
                    false,
                    false,
                    false,
                    true,
                    9,
                )
                .unwrap_err()
                .contains("unknown field")
            );
        }
        for action_id in ["bad\naction".into(), "x".repeat(MAX_ACTION_ID_BYTES + 1)] {
            let shapes = [
                serde_json::json!({"type":"swap-panes","action_id":action_id.clone(),"tab_id":42,"first_pane_id":3,"second_pane_id":7}),
                serde_json::json!({"type":"balance-panes","action_id":action_id.clone(),"tab_id":42}),
                serde_json::json!({"type":"apply-pane-layout","action_id":action_id,"tab_id":42,"preset":"grid"}),
            ];
            for action in shapes {
                let response = serde_json::json!({
                    "schema_version": 2,
                    "request_id": 9,
                    "actions": [action],
                });
                assert!(
                    parse_persistent_response_with_host_mutation(
                        &serde_json::to_vec(&response).unwrap(),
                        false,
                        false,
                        false,
                        false,
                        false,
                        false,
                        false,
                        false,
                        false,
                        false,
                        false,
                        true,
                        9,
                    )
                    .unwrap_err()
                    .contains("control-safe")
                );
            }
        }
        let two_layouts = br#"{"schema_version":2,"request_id":9,"actions":[{"type":"balance-panes","action_id":"balance-build","tab_id":42},{"type":"apply-pane-layout","action_id":"grid-build","tab_id":42,"preset":"grid"}]}"#;
        assert!(
            parse_persistent_response_with_host_mutation(
                two_layouts,
                false,
                false,
                false,
                false,
                false,
                false,
                false,
                false,
                false,
                false,
                false,
                true,
                9,
            )
            .unwrap_err()
            .contains("more than one host operation")
        );
    }

    #[test]
    fn window_close_is_typed_command_gated_strict_and_single_operation() {
        let close = br#"{"schema_version":2,"request_id":9,"actions":[{"type":"close-window","action_id":"close-current-window"}]}"#;
        assert_eq!(
            parse_persistent_response_with_window_close(
                close, false, false, false, false, false, false, false, false, false, false, false,
                false, true, false, false, false, false, false, 9,
            )
            .unwrap(),
            vec![StructuredResponseAction::CloseWindow {
                action_id: "close-current-window".into(),
            }]
        );
        assert!(
            parse_persistent_response(close, false, false, false, false, false, 9)
                .unwrap_err()
                .contains("command-scoped")
        );

        let unknown = br#"{"schema_version":2,"request_id":9,"actions":[{"type":"close-window","action_id":"close-current-window","window_id":3}]}"#;
        assert!(
            parse_persistent_response_with_window_close(
                unknown, false, false, false, false, false, false, false, false, false, false,
                false, false, true, false, false, false, false, false, 9,
            )
            .unwrap_err()
            .contains("unknown field")
        );

        let unsafe_id = br#"{"schema_version":2,"request_id":9,"actions":[{"type":"close-window","action_id":"bad\naction"}]}"#;
        assert!(
            parse_persistent_response_with_window_close(
                unsafe_id, false, false, false, false, false, false, false, false, false, false,
                false, false, true, false, false, false, false, false, 9,
            )
            .unwrap_err()
            .contains("control-safe")
        );

        let query_and_close = br#"{"schema_version":2,"request_id":9,"actions":[{"type":"query-workspace","query_id":"inventory"},{"type":"close-window","action_id":"close-current-window"}]}"#;
        assert!(
            parse_persistent_response_with_window_close(
                query_and_close,
                false,
                true,
                false,
                false,
                false,
                false,
                false,
                false,
                false,
                false,
                false,
                false,
                true,
                false,
                false,
                false,
                false,
                false,
                9,
            )
            .unwrap_err()
            .contains("more than one host operation")
        );
    }

    #[cfg(unix)]
    #[test]
    fn persistent_workspace_query_round_trip_reuses_the_handler() {
        let mut invocation = persistent_fixture(
            "query-fixture",
            r#"count=0
while IFS= read -r event; do
  count=$((count + 1))
  if [ "$count" -eq 1 ]; then
    printf '{"schema_version":2,"request_id":1,"actions":[{"type":"query-workspace","query_id":"inventory"}]}\n'
  else
    printf '{"schema_version":2,"request_id":2,"actions":[{"type":"show-message","message":"query-result-received"}]}\n'
  fi
done"#,
        );
        invocation.allow_workspace_query = true;
        let mut handlers = HashMap::new();
        let shutdown = AtomicBool::new(false);
        let generation = AtomicU64::new(1);
        assert_eq!(
            run_persistent_handler(&mut handlers, &invocation, 1, &shutdown, &generation, 1,)
                .unwrap(),
            vec![StructuredResponseAction::QueryWorkspace {
                query_id: "inventory".into()
            }]
        );
        invocation.event_json = r#"{"schema_version":1,"event":{"type":"workspace-query-result","query_id":"inventory"}}"#.into();
        invocation.allow_workspace_query = false;
        assert_eq!(
            run_persistent_handler(&mut handlers, &invocation, 2, &shutdown, &generation, 1,)
                .unwrap(),
            vec![StructuredResponseAction::ShowMessage {
                message: "query-result-received".into()
            }]
        );
        assert_eq!(handlers.len(), 1);
    }

    #[cfg(unix)]
    #[test]
    fn persistent_terminal_query_round_trip_reuses_the_handler_without_query_chaining() {
        let mut invocation = persistent_fixture(
            "terminal-query-fixture",
            r#"count=0
while IFS= read -r event; do
  count=$((count + 1))
  if [ "$count" -eq 1 ]; then
    printf '{"schema_version":2,"request_id":1,"actions":[{"type":"query-terminal","query_id":"visible-text","tab_id":7,"pane_id":3,"history_lines":20,"max_bytes":2048}]}\n'
  else
    printf '{"schema_version":2,"request_id":2,"actions":[{"type":"show-message","message":"terminal-result-received"}]}\n'
  fi
done"#,
        );
        invocation.allow_terminal_query = true;
        let mut handlers = HashMap::new();
        let shutdown = AtomicBool::new(false);
        let generation = AtomicU64::new(1);
        assert!(matches!(
            run_persistent_handler(&mut handlers, &invocation, 1, &shutdown, &generation, 1)
                .unwrap()[..],
            [StructuredResponseAction::QueryTerminal { .. }]
        ));

        invocation.event_json = r#"{"schema_version":1,"event":{"type":"terminal-query-result","query_id":"visible-text","ok":true}}"#.into();
        invocation.allow_terminal_query = false;
        assert_eq!(
            run_persistent_handler(&mut handlers, &invocation, 2, &shutdown, &generation, 1)
                .unwrap(),
            vec![StructuredResponseAction::ShowMessage {
                message: "terminal-result-received".into(),
            }]
        );
        assert_eq!(handlers.len(), 1);
    }

    #[cfg(unix)]
    #[test]
    fn persistent_pane_creation_result_cannot_create_again() {
        let mut invocation = persistent_fixture(
            "pane-create-fixture",
            r#"while IFS= read -r event; do
  printf '{"schema_version":2,"request_id":1,"actions":[{"type":"create-pane","action_id":"split","tab_id":7,"pane_id":3,"direction":"vertical"}]}\n'
done"#,
        );
        invocation.allow_pane_create = true;
        let mut handlers = HashMap::new();
        let shutdown = AtomicBool::new(false);
        let generation = AtomicU64::new(1);
        assert!(matches!(
            run_persistent_handler(&mut handlers, &invocation, 1, &shutdown, &generation, 1)
                .unwrap()[..],
            [StructuredResponseAction::CreatePane { .. }]
        ));

        invocation.event_json = r#"{"schema_version":1,"event":{"type":"workspace-action-result","action":"create-pane","ok":true}}"#.into();
        invocation.allow_pane_create = false;
        assert!(
            run_persistent_handler(&mut handlers, &invocation, 1, &shutdown, &generation, 1)
                .unwrap_err()
                .contains("pane-create authority")
        );
        assert!(handlers.is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn persistent_tab_creation_result_cannot_create_again() {
        let mut invocation = persistent_fixture(
            "tab-create-fixture",
            r#"while IFS= read -r event; do
  printf '{"schema_version":2,"request_id":1,"actions":[{"type":"create-tab","action_id":"new-task"}]}\n'
done"#,
        );
        invocation.allow_tab_create = true;
        let mut handlers = HashMap::new();
        let shutdown = AtomicBool::new(false);
        let generation = AtomicU64::new(1);
        assert!(matches!(
            run_persistent_handler(&mut handlers, &invocation, 1, &shutdown, &generation, 1)
                .unwrap()[..],
            [StructuredResponseAction::CreateTab { .. }]
        ));

        invocation.event_json = r#"{"schema_version":1,"event":{"type":"workspace-action-result","action":"create-tab","ok":true}}"#.into();
        invocation.allow_tab_create = false;
        assert!(
            run_persistent_handler(&mut handlers, &invocation, 1, &shutdown, &generation, 1)
                .unwrap_err()
                .contains("tab-create authority")
        );
        assert!(handlers.is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn persistent_query_then_pane_close_then_result_is_bounded() {
        let mut invocation = persistent_fixture(
            "pane-close-fixture",
            r#"count=0
while IFS= read -r event; do
  count=$((count + 1))
  if [ "$count" -eq 1 ]; then
    printf '{"schema_version":2,"request_id":1,"actions":[{"type":"query-workspace","query_id":"inventory"}]}\n'
  elif [ "$count" -eq 2 ]; then
    printf '{"schema_version":2,"request_id":2,"actions":[{"type":"close-pane","action_id":"prune","tab_id":7,"pane_id":3}]}\n'
  else
    printf '{"schema_version":2,"request_id":3,"actions":[{"type":"show-message","message":"close-result-received"}]}\n'
  fi
done"#,
        );
        invocation.allow_workspace_query = true;
        invocation.allow_pane_close = true;
        let mut handlers = HashMap::new();
        let shutdown = AtomicBool::new(false);
        let generation = AtomicU64::new(1);
        assert!(matches!(
            run_persistent_handler(&mut handlers, &invocation, 1, &shutdown, &generation, 1)
                .unwrap()[..],
            [StructuredResponseAction::QueryWorkspace { .. }]
        ));

        invocation.event_json = r#"{"schema_version":1,"event":{"type":"workspace-query-result","query_id":"inventory"}}"#.into();
        invocation.allow_workspace_query = false;
        assert!(matches!(
            run_persistent_handler(&mut handlers, &invocation, 2, &shutdown, &generation, 1)
                .unwrap()[..],
            [StructuredResponseAction::ClosePane { .. }]
        ));

        invocation.event_json = r#"{"schema_version":1,"event":{"type":"workspace-action-result","action":"close-pane","ok":true}}"#.into();
        invocation.allow_pane_close = false;
        assert_eq!(
            run_persistent_handler(&mut handlers, &invocation, 3, &shutdown, &generation, 1)
                .unwrap(),
            vec![StructuredResponseAction::ShowMessage {
                message: "close-result-received".into(),
            }]
        );
        assert_eq!(handlers.len(), 1);
    }

    #[cfg(unix)]
    #[test]
    fn persistent_query_then_tab_close_then_result_is_bounded() {
        let mut invocation = persistent_fixture(
            "tab-close-fixture",
            r#"count=0
while IFS= read -r event; do
  count=$((count + 1))
  if [ "$count" -eq 1 ]; then
    printf '{"schema_version":2,"request_id":1,"actions":[{"type":"query-workspace","query_id":"inventory"}]}\n'
  elif [ "$count" -eq 2 ]; then
    printf '{"schema_version":2,"request_id":2,"actions":[{"type":"close-tab","action_id":"retire","tab_id":7}]}\n'
  else
    printf '{"schema_version":2,"request_id":3,"actions":[{"type":"show-message","message":"close-result-received"}]}\n'
  fi
done"#,
        );
        invocation.allow_workspace_query = true;
        invocation.allow_tab_close = true;
        let mut handlers = HashMap::new();
        let shutdown = AtomicBool::new(false);
        let generation = AtomicU64::new(1);
        assert!(matches!(
            run_persistent_handler(&mut handlers, &invocation, 1, &shutdown, &generation, 1)
                .unwrap()[..],
            [StructuredResponseAction::QueryWorkspace { .. }]
        ));

        invocation.event_json = r#"{"schema_version":1,"event":{"type":"workspace-query-result","query_id":"inventory"}}"#.into();
        invocation.allow_workspace_query = false;
        assert!(matches!(
            run_persistent_handler(&mut handlers, &invocation, 2, &shutdown, &generation, 1)
                .unwrap()[..],
            [StructuredResponseAction::CloseTab { .. }]
        ));

        invocation.event_json = r#"{"schema_version":1,"event":{"type":"workspace-action-result","action":"close-tab","ok":true,"scheduled":true}}"#.into();
        invocation.allow_tab_close = false;
        assert_eq!(
            run_persistent_handler(&mut handlers, &invocation, 3, &shutdown, &generation, 1)
                .unwrap(),
            vec![StructuredResponseAction::ShowMessage {
                message: "close-result-received".into(),
            }]
        );
        assert_eq!(handlers.len(), 1);
    }

    #[cfg(unix)]
    #[test]
    fn persistent_query_then_pane_move_then_result_is_bounded() {
        let mut invocation = persistent_fixture(
            "pane-move-fixture",
            r#"count=0
while IFS= read -r event; do
  count=$((count + 1))
  if [ "$count" -eq 1 ]; then
    printf '{"schema_version":2,"request_id":1,"actions":[{"type":"query-workspace","query_id":"inventory"}]}\n'
  elif [ "$count" -eq 2 ]; then
    printf '{"schema_version":2,"request_id":2,"actions":[{"type":"move-pane","action_id":"relocate","source_tab_id":7,"pane_id":3,"destination_tab_id":8,"target_pane_id":1,"direction":"vertical"}]}\n'
  else
    printf '{"schema_version":2,"request_id":3,"actions":[{"type":"show-message","message":"move-result-received"}]}\n'
  fi
done"#,
        );
        invocation.allow_workspace_query = true;
        invocation.allow_pane_move = true;
        let mut handlers = HashMap::new();
        let shutdown = AtomicBool::new(false);
        let generation = AtomicU64::new(1);
        assert!(matches!(
            run_persistent_handler(&mut handlers, &invocation, 1, &shutdown, &generation, 1)
                .unwrap()[..],
            [StructuredResponseAction::QueryWorkspace { .. }]
        ));

        invocation.event_json = r#"{"schema_version":1,"event":{"type":"workspace-query-result","query_id":"inventory"}}"#.into();
        invocation.allow_workspace_query = false;
        assert!(matches!(
            run_persistent_handler(&mut handlers, &invocation, 2, &shutdown, &generation, 1)
                .unwrap()[..],
            [StructuredResponseAction::MovePane { .. }]
        ));

        invocation.event_json = r#"{"schema_version":1,"event":{"type":"workspace-action-result","action":"move-pane","ok":true}}"#.into();
        invocation.allow_pane_move = false;
        assert_eq!(
            run_persistent_handler(&mut handlers, &invocation, 3, &shutdown, &generation, 1)
                .unwrap(),
            vec![StructuredResponseAction::ShowMessage {
                message: "move-result-received".into(),
            }]
        );
        assert_eq!(handlers.len(), 1);
    }

    #[cfg(unix)]
    #[test]
    fn persistent_query_then_terminal_input_then_result_is_bounded() {
        let mut invocation = persistent_fixture(
            "terminal-input-fixture",
            r#"count=0
while IFS= read -r event; do
  count=$((count + 1))
  if [ "$count" -eq 1 ]; then
    printf '{"schema_version":2,"request_id":1,"actions":[{"type":"query-workspace","query_id":"inventory"}]}\n'
  elif [ "$count" -eq 2 ]; then
    printf '{"schema_version":2,"request_id":2,"actions":[{"type":"send-text","action_id":"draft","tab_id":7,"pane_id":3,"text":"cargo test","paste":false,"enter":false}]}\n'
  else
    printf '{"schema_version":2,"request_id":3,"actions":[{"type":"show-message","message":"input-result-received"}]}\n'
  fi
done"#,
        );
        invocation.allow_workspace_query = true;
        invocation.allow_terminal_input = true;
        let mut handlers = HashMap::new();
        let shutdown = AtomicBool::new(false);
        let generation = AtomicU64::new(1);
        assert!(matches!(
            run_persistent_handler(&mut handlers, &invocation, 1, &shutdown, &generation, 1)
                .unwrap()[..],
            [StructuredResponseAction::QueryWorkspace { .. }]
        ));

        invocation.event_json = r#"{"schema_version":1,"event":{"type":"workspace-query-result","query_id":"inventory"}}"#.into();
        invocation.allow_workspace_query = false;
        assert!(matches!(
            run_persistent_handler(&mut handlers, &invocation, 2, &shutdown, &generation, 1)
                .unwrap()[..],
            [StructuredResponseAction::SendText { .. }]
        ));

        invocation.event_json = r#"{"schema_version":1,"event":{"type":"workspace-action-result","action":"send-text","ok":true,"disposition":"sent"}}"#.into();
        invocation.allow_terminal_input = false;
        assert_eq!(
            run_persistent_handler(&mut handlers, &invocation, 3, &shutdown, &generation, 1)
                .unwrap(),
            vec![StructuredResponseAction::ShowMessage {
                message: "input-result-received".into(),
            }]
        );
        assert_eq!(handlers.len(), 1);
    }

    #[cfg(unix)]
    #[test]
    fn persistent_query_then_pane_layout_then_result_is_bounded() {
        let mut invocation = persistent_fixture(
            "pane-layout-fixture",
            r#"count=0
while IFS= read -r event; do
  count=$((count + 1))
  if [ "$count" -eq 1 ]; then
    printf '{"schema_version":2,"request_id":1,"actions":[{"type":"query-workspace","query_id":"inventory"}]}\n'
  elif [ "$count" -eq 2 ]; then
    printf '{"schema_version":2,"request_id":2,"actions":[{"type":"apply-pane-layout","action_id":"grid","tab_id":7,"preset":"grid"}]}\n'
  else
    printf '{"schema_version":2,"request_id":3,"actions":[{"type":"show-message","message":"layout-result-received"}]}\n'
  fi
done"#,
        );
        invocation.allow_workspace_query = true;
        invocation.allow_pane_layout = true;
        let mut handlers = HashMap::new();
        let shutdown = AtomicBool::new(false);
        let generation = AtomicU64::new(1);
        assert!(matches!(
            run_persistent_handler(&mut handlers, &invocation, 1, &shutdown, &generation, 1)
                .unwrap()[..],
            [StructuredResponseAction::QueryWorkspace { .. }]
        ));

        invocation.event_json = r#"{"schema_version":1,"event":{"type":"workspace-query-result","query_id":"inventory"}}"#.into();
        invocation.allow_workspace_query = false;
        assert!(matches!(
            run_persistent_handler(&mut handlers, &invocation, 2, &shutdown, &generation, 1)
                .unwrap()[..],
            [StructuredResponseAction::ApplyPaneLayout {
                preset: PluginPaneLayoutPreset::Grid,
                ..
            }]
        ));

        invocation.event_json = r#"{"schema_version":1,"event":{"type":"workspace-action-result","action":"apply-pane-layout","ok":true}}"#.into();
        invocation.allow_pane_layout = false;
        assert_eq!(
            run_persistent_handler(&mut handlers, &invocation, 3, &shutdown, &generation, 1)
                .unwrap(),
            vec![StructuredResponseAction::ShowMessage {
                message: "layout-result-received".into(),
            }]
        );
        assert_eq!(handlers.len(), 1);
    }

    #[cfg(unix)]
    #[test]
    fn persistent_query_then_window_close_then_final_result_is_bounded() {
        let mut invocation = persistent_fixture(
            "window-close-fixture",
            r#"count=0
while IFS= read -r event; do
  count=$((count + 1))
  if [ "$count" -eq 1 ]; then
    printf '{"schema_version":2,"request_id":1,"actions":[{"type":"query-workspace","query_id":"confirm-current-window"}]}\n'
  elif [ "$count" -eq 2 ]; then
    printf '{"schema_version":2,"request_id":2,"actions":[{"type":"close-window","action_id":"close-current-window"}]}\n'
  else
    printf '{"schema_version":2,"request_id":3,"actions":[]}\n'
  fi
done"#,
        );
        invocation.allow_workspace_query = true;
        invocation.allow_window_close = true;
        let mut handlers = HashMap::new();
        let shutdown = AtomicBool::new(false);
        let generation = AtomicU64::new(1);
        assert!(matches!(
            run_persistent_handler(&mut handlers, &invocation, 1, &shutdown, &generation, 1)
                .unwrap()[..],
            [StructuredResponseAction::QueryWorkspace { .. }]
        ));

        invocation.event_json = r#"{"schema_version":1,"event":{"type":"workspace-query-result","query_id":"confirm-current-window"}}"#.into();
        invocation.allow_workspace_query = false;
        assert_eq!(
            run_persistent_handler(&mut handlers, &invocation, 2, &shutdown, &generation, 1)
                .unwrap(),
            vec![StructuredResponseAction::CloseWindow {
                action_id: "close-current-window".into(),
            }]
        );

        invocation.event_json = r#"{"schema_version":1,"event":{"type":"workspace-action-result","action":"close-window","ok":true,"scheduled":true}}"#.into();
        invocation.allow_window_close = false;
        assert!(
            run_persistent_handler(&mut handlers, &invocation, 3, &shutdown, &generation, 1)
                .unwrap()
                .is_empty()
        );
        assert_eq!(handlers.len(), 1);
    }

    #[cfg(unix)]
    #[test]
    fn persistent_query_then_action_then_result_reuses_one_bounded_handler() {
        let mut invocation = persistent_fixture(
            "workspace-action-fixture",
            r#"count=0
while IFS= read -r event; do
  count=$((count + 1))
  if [ "$count" -eq 1 ]; then
    printf '{"schema_version":2,"request_id":1,"actions":[{"type":"query-workspace","query_id":"inventory"}]}\n'
  elif [ "$count" -eq 2 ]; then
    printf '{"schema_version":2,"request_id":2,"actions":[{"type":"select-tab","action_id":"select-active","tab_id":7}]}\n'
  else
    printf '{"schema_version":2,"request_id":3,"actions":[{"type":"show-message","message":"action-result-received"}]}\n'
  fi
done"#,
        );
        invocation.allow_workspace_query = true;
        invocation.allow_workspace_actions = true;
        let mut handlers = HashMap::new();
        let shutdown = AtomicBool::new(false);
        let generation = AtomicU64::new(1);
        assert!(matches!(
            run_persistent_handler(&mut handlers, &invocation, 1, &shutdown, &generation, 1)
                .unwrap()[..],
            [StructuredResponseAction::QueryWorkspace { .. }]
        ));

        invocation.event_json = r#"{"schema_version":1,"event":{"type":"workspace-query-result","query_id":"inventory"}}"#.into();
        invocation.allow_workspace_query = false;
        assert_eq!(
            run_persistent_handler(&mut handlers, &invocation, 2, &shutdown, &generation, 1)
                .unwrap(),
            vec![StructuredResponseAction::SelectTab {
                action_id: "select-active".into(),
                tab_id: 7,
            }]
        );

        invocation.event_json = r#"{"schema_version":1,"event":{"type":"workspace-action-result","action_id":"select-active","ok":true}}"#.into();
        invocation.allow_workspace_actions = false;
        assert_eq!(
            run_persistent_handler(&mut handlers, &invocation, 3, &shutdown, &generation, 1)
                .unwrap(),
            vec![StructuredResponseAction::ShowMessage {
                message: "action-result-received".into(),
            }]
        );
        assert_eq!(handlers.len(), 1);
    }

    #[cfg(unix)]
    #[test]
    fn persistent_process_query_then_navigation_then_result_reuses_one_bounded_handler() {
        let mut invocation = persistent_fixture(
            "process-navigation-fixture",
            r#"count=0
while IFS= read -r event; do
  count=$((count + 1))
  if [ "$count" -eq 1 ]; then
    printf '{"schema_version":2,"request_id":1,"actions":[{"type":"query-process","query_id":"windows"}]}\n'
  elif [ "$count" -eq 2 ]; then
    printf '{"schema_version":2,"request_id":2,"actions":[{"type":"focus-process-pane","action_id":"focus-target","window_id":7,"tab_id":12,"pane_id":3}]}\n'
  else
    printf '{"schema_version":2,"request_id":3,"actions":[{"type":"show-message","message":"process-action-result-received"}]}\n'
  fi
done"#,
        );
        invocation.allow_process_query = true;
        invocation.allow_process_actions = true;
        let mut handlers = HashMap::new();
        let shutdown = AtomicBool::new(false);
        let generation = AtomicU64::new(1);
        assert!(matches!(
            run_persistent_handler(&mut handlers, &invocation, 1, &shutdown, &generation, 1)
                .unwrap()[..],
            [StructuredResponseAction::QueryProcess { .. }]
        ));

        invocation.event_json =
            r#"{"schema_version":1,"event":{"type":"process-query-result","query_id":"windows"}}"#
                .into();
        invocation.allow_process_query = false;
        assert_eq!(
            run_persistent_handler(&mut handlers, &invocation, 2, &shutdown, &generation, 1)
                .unwrap(),
            vec![StructuredResponseAction::FocusProcessPane {
                action_id: "focus-target".into(),
                window_id: 7,
                tab_id: 12,
                pane_id: 3,
            }]
        );

        invocation.event_json = r#"{"schema_version":1,"event":{"type":"process-action-result","action_id":"focus-target","ok":true}}"#.into();
        invocation.allow_process_actions = false;
        assert_eq!(
            run_persistent_handler(&mut handlers, &invocation, 3, &shutdown, &generation, 1)
                .unwrap(),
            vec![StructuredResponseAction::ShowMessage {
                message: "process-action-result-received".into(),
            }]
        );
        assert_eq!(handlers.len(), 1);
    }

    #[cfg(unix)]
    #[test]
    fn persistent_process_query_then_terminal_query_then_result_is_bounded() {
        let mut invocation = persistent_fixture(
            "process-terminal-fixture",
            r#"count=0
while IFS= read -r event; do
  count=$((count + 1))
  if [ "$count" -eq 1 ]; then
    printf '{"schema_version":2,"request_id":1,"actions":[{"type":"query-process","query_id":"windows"}]}\n'
  elif [ "$count" -eq 2 ]; then
    printf '{"schema_version":2,"request_id":2,"actions":[{"type":"query-process-terminal","query_id":"sibling-output","window_id":7,"tab_id":12,"pane_id":3,"history_lines":20,"max_bytes":2048}]}\n'
  else
    printf '{"schema_version":2,"request_id":3,"actions":[{"type":"show-message","message":"process-terminal-result-received"}]}\n'
  fi
done"#,
        );
        invocation.allow_process_query = true;
        invocation.allow_process_terminal_text = true;
        let mut handlers = HashMap::new();
        let shutdown = AtomicBool::new(false);
        let generation = AtomicU64::new(1);
        assert!(matches!(
            run_persistent_handler(&mut handlers, &invocation, 1, &shutdown, &generation, 1)
                .unwrap()[..],
            [StructuredResponseAction::QueryProcess { .. }]
        ));

        invocation.event_json =
            r#"{"schema_version":1,"event":{"type":"process-query-result","query_id":"windows"}}"#
                .into();
        invocation.allow_process_query = false;
        assert_eq!(
            run_persistent_handler(&mut handlers, &invocation, 2, &shutdown, &generation, 1)
                .unwrap(),
            vec![StructuredResponseAction::QueryProcessTerminal {
                query_id: "sibling-output".into(),
                window_id: 7,
                tab_id: 12,
                pane_id: 3,
                history_lines: 20,
                max_bytes: 2048,
            }]
        );

        invocation.event_json = r#"{"schema_version":1,"event":{"type":"process-terminal-query-result","query_id":"sibling-output","ok":true,"text":"bounded"}}"#.into();
        invocation.allow_process_terminal_text = false;
        assert_eq!(
            run_persistent_handler(&mut handlers, &invocation, 3, &shutdown, &generation, 1)
                .unwrap(),
            vec![StructuredResponseAction::ShowMessage {
                message: "process-terminal-result-received".into(),
            }]
        );
        assert_eq!(handlers.len(), 1);
    }

    #[cfg(unix)]
    #[test]
    fn checked_in_process_terminal_reader_completes_a_hermetic_three_event_conversation() {
        if !std::path::Path::new("/usr/bin/python3").is_file()
            && !std::path::Path::new("/bin/python3").is_file()
        {
            return;
        }
        let directory = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../examples/plugins/process-terminal-reader");
        let (manifest, _, _) =
            crate::plugin::runtime::PluginManifest::from_directory(&directory).unwrap();
        let handler = manifest.event_handler.unwrap();
        let mut invocation = StructuredHandlerInvocation {
            plugin_id: manifest.id,
            program: handler.program,
            args: handler.args,
            event_json: r#"{"schema_version":1,"event":{"type":"command-invoked"}}"#.into(),
            allow_ui_actions: true,
            allow_workspace_query: false,
            allow_workspace_actions: false,
            allow_workspace_topology: false,
            allow_terminal_query: false,
            allow_pane_create: false,
            allow_tab_create: false,
            allow_pane_close: false,
            allow_tab_close: false,
            allow_pane_move: false,
            allow_terminal_input: false,
            allow_pane_layout: false,
            allow_window_close: false,
            allow_process_query: true,
            allow_process_actions: false,
            allow_process_terminal_text: true,
            allow_process_terminal_input: false,
            allow_process_tab_create: false,
            mode: StructuredHandlerMode::Persistent,
        };
        let mut handlers = HashMap::new();
        let shutdown = AtomicBool::new(false);
        let generation = AtomicU64::new(1);

        assert_eq!(
            run_persistent_handler(&mut handlers, &invocation, 1, &shutdown, &generation, 1)
                .unwrap(),
            vec![StructuredResponseAction::QueryProcess {
                query_id: "terminal-targets".into(),
            }]
        );

        invocation.event_json = r#"{"schema_version":1,"event":{"type":"process-query-result","origin_window_id":2,"windows":[{"id":2,"tabs":[]},{"id":7,"tabs":[{"id":12,"closed":false,"panes":[{"id":3,"closed":false}]}]}]}}"#.into();
        invocation.allow_process_query = false;
        assert_eq!(
            run_persistent_handler(&mut handlers, &invocation, 2, &shutdown, &generation, 1)
                .unwrap(),
            vec![StructuredResponseAction::QueryProcessTerminal {
                query_id: "sibling-output".into(),
                window_id: 7,
                tab_id: 12,
                pane_id: 3,
                history_lines: 20,
                max_bytes: 2048,
            }]
        );

        invocation.event_json = r#"{"schema_version":1,"event":{"type":"process-terminal-query-result","ok":true,"text_bytes":11,"history_lines":0,"truncated":false,"text":"hello\nworld"}}"#.into();
        invocation.allow_process_terminal_text = false;
        assert_eq!(
            run_persistent_handler(&mut handlers, &invocation, 3, &shutdown, &generation, 1)
                .unwrap(),
            vec![StructuredResponseAction::ShowNotice {
                title: "Sibling pane text".into(),
                lines: vec![
                    "Text bytes: 11".into(),
                    "History lines: 0".into(),
                    "Truncated: False".into(),
                    "Preview: hello world".into(),
                ],
            }]
        );
        assert_eq!(handlers.len(), 1);
    }

    #[cfg(unix)]
    #[test]
    fn checked_in_process_terminal_writer_completes_a_hermetic_three_event_conversation() {
        if !std::path::Path::new("/usr/bin/python3").is_file()
            && !std::path::Path::new("/bin/python3").is_file()
        {
            return;
        }
        let directory = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../examples/plugins/process-terminal-writer");
        let (manifest, _, _) =
            crate::plugin::runtime::PluginManifest::from_directory(&directory).unwrap();
        let handler = manifest.event_handler.unwrap();
        let mut invocation = StructuredHandlerInvocation {
            plugin_id: manifest.id,
            program: handler.program,
            args: handler.args,
            event_json: r#"{"schema_version":1,"event":{"type":"command-invoked"}}"#.into(),
            allow_ui_actions: true,
            allow_workspace_query: false,
            allow_workspace_actions: false,
            allow_workspace_topology: false,
            allow_terminal_query: false,
            allow_pane_create: false,
            allow_tab_create: false,
            allow_pane_close: false,
            allow_tab_close: false,
            allow_pane_move: false,
            allow_terminal_input: false,
            allow_pane_layout: false,
            allow_window_close: false,
            allow_process_query: true,
            allow_process_actions: false,
            allow_process_terminal_text: false,
            allow_process_terminal_input: true,
            allow_process_tab_create: false,
            mode: StructuredHandlerMode::Persistent,
        };
        let mut handlers = HashMap::new();
        let shutdown = AtomicBool::new(false);
        let generation = AtomicU64::new(1);

        assert_eq!(
            run_persistent_handler(&mut handlers, &invocation, 1, &shutdown, &generation, 1)
                .unwrap(),
            vec![StructuredResponseAction::QueryProcess {
                query_id: "input-targets".into(),
            }]
        );

        invocation.event_json = r#"{"schema_version":1,"event":{"type":"process-query-result","origin_window_id":2,"windows":[{"id":2,"tabs":[]},{"id":7,"tabs":[{"id":12,"closed":false,"panes":[{"id":3,"closed":false}]}]}]}}"#.into();
        invocation.allow_process_query = false;
        assert_eq!(
            run_persistent_handler(&mut handlers, &invocation, 2, &shutdown, &generation, 1)
                .unwrap(),
            vec![StructuredResponseAction::SendProcessText {
                action_id: "draft-safe-text".into(),
                window_id: 7,
                tab_id: 12,
                pane_id: 3,
                text: "hello from the process terminal writer".into(),
                paste: true,
                enter: false,
            }]
        );

        invocation.event_json = r#"{"schema_version":1,"event":{"type":"process-action-result","action":"send-process-text","ok":true,"disposition":"sent","window_id":7,"input_bytes":38}}"#.into();
        invocation.allow_process_terminal_input = false;
        assert_eq!(
            run_persistent_handler(&mut handlers, &invocation, 3, &shutdown, &generation, 1)
                .unwrap(),
            vec![StructuredResponseAction::ShowNotice {
                title: "Process terminal writer".into(),
                lines: vec!["Input sent.".into(), "Window: 7".into(), "Bytes: 38".into(),],
            }]
        );
        assert_eq!(handlers.len(), 1);
    }

    #[cfg(unix)]
    #[test]
    fn checked_in_process_tab_builder_completes_a_hermetic_three_event_conversation() {
        if !std::path::Path::new("/usr/bin/python3").is_file()
            && !std::path::Path::new("/bin/python3").is_file()
        {
            return;
        }
        let directory = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../examples/plugins/process-tab-builder");
        let (manifest, _, _) =
            crate::plugin::runtime::PluginManifest::from_directory(&directory).unwrap();
        let handler = manifest.event_handler.unwrap();
        let mut invocation = StructuredHandlerInvocation {
            plugin_id: manifest.id,
            program: handler.program,
            args: handler.args,
            event_json: r#"{"schema_version":1,"event":{"type":"command-invoked"}}"#.into(),
            allow_ui_actions: true,
            allow_workspace_query: false,
            allow_workspace_actions: false,
            allow_workspace_topology: false,
            allow_terminal_query: false,
            allow_pane_create: false,
            allow_tab_create: false,
            allow_pane_close: false,
            allow_tab_close: false,
            allow_pane_move: false,
            allow_terminal_input: false,
            allow_pane_layout: false,
            allow_window_close: false,
            allow_process_query: true,
            allow_process_actions: false,
            allow_process_terminal_text: false,
            allow_process_terminal_input: false,
            allow_process_tab_create: true,
            mode: StructuredHandlerMode::Persistent,
        };
        let mut handlers = HashMap::new();
        let shutdown = AtomicBool::new(false);
        let generation = AtomicU64::new(1);

        assert_eq!(
            run_persistent_handler(&mut handlers, &invocation, 1, &shutdown, &generation, 1)
                .unwrap(),
            vec![StructuredResponseAction::QueryProcess {
                query_id: "tab-creation-targets".into(),
            }]
        );

        invocation.event_json = r#"{"schema_version":1,"event":{"type":"process-query-result","origin_window_id":2,"windows":[{"id":2,"tabs":[]},{"id":7,"tabs":[]}]}}"#.into();
        invocation.allow_process_query = false;
        assert_eq!(
            run_persistent_handler(&mut handlers, &invocation, 2, &shutdown, &generation, 1)
                .unwrap(),
            vec![StructuredResponseAction::CreateProcessTab {
                action_id: "new-sibling-task".into(),
                window_id: 7,
            }]
        );

        invocation.event_json = r#"{"schema_version":1,"event":{"type":"process-action-result","action":"create-process-tab","ok":true,"window_id":7,"tab_id":12,"selected":true}}"#.into();
        invocation.allow_process_tab_create = false;
        assert_eq!(
            run_persistent_handler(&mut handlers, &invocation, 3, &shutdown, &generation, 1)
                .unwrap(),
            vec![StructuredResponseAction::ShowNotice {
                title: "Process tab builder".into(),
                lines: vec!["Created tab 12.".into(), "Window: 7".into()],
            }]
        );
        assert_eq!(handlers.len(), 1);
    }

    #[cfg(unix)]
    #[test]
    fn persistent_query_then_topology_then_result_reuses_one_bounded_handler() {
        let mut invocation = persistent_fixture(
            "workspace-topology-fixture",
            r#"count=0
while IFS= read -r event; do
  count=$((count + 1))
  if [ "$count" -eq 1 ]; then
    printf '{"schema_version":2,"request_id":1,"actions":[{"type":"query-workspace","query_id":"inventory"}]}\n'
  elif [ "$count" -eq 2 ]; then
    printf '{"schema_version":2,"request_id":2,"actions":[{"type":"move-tab","action_id":"keep-order","tab_id":7,"destination_index":0}]}\n'
  else
    printf '{"schema_version":2,"request_id":3,"actions":[{"type":"show-message","message":"topology-result-received"}]}\n'
  fi
done"#,
        );
        invocation.allow_workspace_query = true;
        invocation.allow_workspace_topology = true;
        let mut handlers = HashMap::new();
        let shutdown = AtomicBool::new(false);
        let generation = AtomicU64::new(1);
        assert!(matches!(
            run_persistent_handler(&mut handlers, &invocation, 1, &shutdown, &generation, 1)
                .unwrap()[..],
            [StructuredResponseAction::QueryWorkspace { .. }]
        ));

        invocation.event_json = r#"{"schema_version":1,"event":{"type":"workspace-query-result","query_id":"inventory"}}"#.into();
        invocation.allow_workspace_query = false;
        assert_eq!(
            run_persistent_handler(&mut handlers, &invocation, 2, &shutdown, &generation, 1)
                .unwrap(),
            vec![StructuredResponseAction::MoveTab {
                action_id: "keep-order".into(),
                tab_id: 7,
                destination_index: 0,
            }]
        );

        invocation.event_json = r#"{"schema_version":1,"event":{"type":"workspace-action-result","action_id":"keep-order","ok":true}}"#.into();
        invocation.allow_workspace_topology = false;
        assert_eq!(
            run_persistent_handler(&mut handlers, &invocation, 3, &shutdown, &generation, 1)
                .unwrap(),
            vec![StructuredResponseAction::ShowMessage {
                message: "topology-result-received".into(),
            }]
        );
        assert_eq!(handlers.len(), 1);
    }

    #[test]
    fn persistent_protocol_requires_object_source_and_exact_correlation() {
        let request =
            persistent_request_json(r#"{"schema_version":1,"event":{"type":"app-ready"}}"#, 42)
                .unwrap();
        let value: serde_json::Value = serde_json::from_slice(&request).unwrap();
        assert_eq!(value["schema_version"], PERSISTENT_SCHEMA_VERSION);
        assert_eq!(value["request_id"], 42);
        assert!(persistent_request_json("[]", 1).is_err());
        assert!(persistent_request_json(r#"{"schema_version":2}"#, 1).is_err());

        let valid = br#"{"schema_version":2,"request_id":42,"actions":[]}"#;
        assert!(parse_persistent_response(valid, true, false, false, false, false, 42).is_ok());
        assert!(
            parse_persistent_response(valid, true, false, false, false, false, 41)
                .unwrap_err()
                .contains("did not match")
        );
        assert!(
            parse_persistent_response(
                br#"{"schema_version":1,"request_id":42,"actions":[]}"#,
                true,
                false,
                false,
                false,
                false,
                42,
            )
            .unwrap_err()
            .contains("unsupported")
        );
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

    #[cfg(unix)]
    #[test]
    fn persistent_handler_reuses_one_stateful_process() {
        let invocation = persistent_fixture(
            "stateful-fixture",
            r#"count=0
while IFS= read -r event; do
  count=$((count + 1))
  printf '{"schema_version":2,"request_id":%s,"actions":[{"type":"show-message","message":"pid=%s count=%s"}]}\n' "$count" "$$" "$count"
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

    #[cfg(unix)]
    #[test]
    fn persistent_protocol_failure_discards_process_and_allows_restart() {
        let mismatched = persistent_fixture(
            "restart-fixture",
            r#"IFS= read -r event
printf '{"schema_version":2,"request_id":999,"actions":[]}\n'
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
  printf '{"schema_version":2,"request_id":2,"actions":[]}\n'
done"#,
        );
        assert!(
            run_persistent_handler(&mut handlers, &healthy, 2, &shutdown, &generation, 1,).is_ok()
        );
        assert_eq!(handlers.len(), 1);
    }

    #[cfg(unix)]
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

    #[cfg(unix)]
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
            run_persistent_handler(&mut handlers, &crash, 2, &shutdown, &generation, 1,).is_err()
        );
        assert!(handlers.is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn persistent_handler_count_is_bounded() {
        let mut handlers = HashMap::new();
        let shutdown = AtomicBool::new(false);
        let generation = AtomicU64::new(1);
        for request_id in 1..=MAX_PERSISTENT_HANDLERS as u64 {
            let invocation = persistent_fixture(
                &format!("fixture-{request_id}"),
                &format!(
                    "while IFS= read -r event; do printf '{{\"schema_version\":2,\"request_id\":{request_id},\"actions\":[]}}\\n'; done"
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
            run_persistent_handler(&mut handlers, &overflow, 99, &shutdown, &generation, 1,)
                .unwrap_err()
                .contains("limit reached")
        );
    }

    #[cfg(unix)]
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
            "while IFS= read -r event; do printf '{\"schema_version\":2,\"request_id\":1,\"actions\":[]}\\n'; done",
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

    #[cfg(unix)]
    #[test]
    fn local_stdio_handler_round_trip_is_argv_preserving_and_bounded() {
        let invocation = StructuredHandlerInvocation {
            plugin_id: "fixture".into(),
            program: "/bin/sh".into(),
            args: vec![
                "-c".into(),
                "cat >/dev/null; printf '%s' '{\"schema_version\":1,\"actions\":[{\"type\":\"show-message\",\"message\":\"handled\"}]}'".into(),
            ],
            event_json: "x".repeat(16 * 1024),
            allow_ui_actions: true,
            allow_workspace_query: false,
            allow_workspace_actions: false,
            allow_workspace_topology: false,
            allow_terminal_query: false,
            allow_pane_create: false,
            allow_tab_create: false,
            allow_pane_close: false,
            allow_tab_close: false,
            allow_pane_move: false,
            allow_terminal_input: false,
            allow_pane_layout: false,
            allow_window_close: false,
            allow_process_query: false,
            allow_process_actions: false,
            allow_process_terminal_text: false,
            allow_process_terminal_input: false,
            allow_process_tab_create: false,
            mode: StructuredHandlerMode::ProcessPerEvent,
        };
        assert_eq!(
            run_fixture(&invocation).unwrap(),
            vec![StructuredResponseAction::ShowMessage {
                message: "handled".into()
            }]
        );

        let failed = StructuredHandlerInvocation {
            args: vec![
                "-c".into(),
                "cat >/dev/null; printf 'bad' >&2; exit 7".into(),
            ],
            ..invocation
        };
        let error = run_fixture(&failed).unwrap_err();
        assert!(error.contains("exit status: 7"));
        assert!(error.contains("bad"));
    }

    #[cfg(unix)]
    #[test]
    fn timed_out_handler_process_group_is_terminated() {
        let invocation = StructuredHandlerInvocation {
            plugin_id: "timeout-fixture".into(),
            program: "/bin/sh".into(),
            args: vec!["-c".into(), "sleep 10".into()],
            event_json: "x".repeat(16 * 1024),
            allow_ui_actions: false,
            allow_workspace_query: false,
            allow_workspace_actions: false,
            allow_workspace_topology: false,
            allow_terminal_query: false,
            allow_pane_create: false,
            allow_tab_create: false,
            allow_pane_close: false,
            allow_tab_close: false,
            allow_pane_move: false,
            allow_terminal_input: false,
            allow_pane_layout: false,
            allow_window_close: false,
            allow_process_query: false,
            allow_process_actions: false,
            allow_process_terminal_text: false,
            allow_process_terminal_input: false,
            allow_process_tab_create: false,
            mode: StructuredHandlerMode::ProcessPerEvent,
        };
        let started = Instant::now();
        let error = run_fixture(&invocation).unwrap_err();
        assert!(error.contains("exceeded 2000 ms"));
        assert!(started.elapsed() < Duration::from_secs(4));
    }

    #[cfg(unix)]
    #[test]
    fn registry_generation_change_cancels_active_handler() {
        let invocation = StructuredHandlerInvocation {
            plugin_id: "cancel-fixture".into(),
            program: "/bin/sh".into(),
            args: vec!["-c".into(), "cat >/dev/null; sleep 10".into()],
            event_json: "{\"schema_version\":1}".into(),
            allow_ui_actions: false,
            allow_workspace_query: false,
            allow_workspace_actions: false,
            allow_workspace_topology: false,
            allow_terminal_query: false,
            allow_pane_create: false,
            allow_tab_create: false,
            allow_pane_close: false,
            allow_tab_close: false,
            allow_pane_move: false,
            allow_terminal_input: false,
            allow_pane_layout: false,
            allow_window_close: false,
            allow_process_query: false,
            allow_process_actions: false,
            allow_process_terminal_text: false,
            allow_process_terminal_input: false,
            allow_process_tab_create: false,
            mode: StructuredHandlerMode::ProcessPerEvent,
        };
        let generation = Arc::new(AtomicU64::new(1));
        let update = Arc::clone(&generation);
        let changer = thread::spawn(move || {
            thread::sleep(Duration::from_millis(50));
            update.store(2, Ordering::Release);
        });
        let started = Instant::now();
        let error = run_handler(&invocation, &AtomicBool::new(false), &generation, 1).unwrap_err();
        changer.join().unwrap();
        assert!(error.contains("cancelled"));
        assert!(started.elapsed() < Duration::from_secs(1));
    }

    #[test]
    fn errors_are_control_free_and_bounded() {
        let error = bounded_error(&format!("bad\n\u{1b}{}", "x".repeat(2_000)));
        assert!(error.len() <= 1024);
        assert!(!error.chars().any(char::is_control));
    }
}
