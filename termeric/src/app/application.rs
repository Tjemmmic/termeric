//! Process-level ownership for independent native windows.

use super::{
    App, AppEvent, clear_pty_wake_pending, clear_transcript_wake_pending,
    redraw_scheduler::RedrawReason,
};
use crate::cli::{LaunchPlan, WindowLaunch};
use crate::config::Config;
use anyhow::Result;
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use std::collections::{HashMap, VecDeque};
use std::time::Instant;
use winit::application::ApplicationHandler;
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, ControlFlow};
use winit::window::WindowId;

/// One event-loop application containing any number of window-local terminal
/// controllers. A window owns its renderer, tabs, panes, UI state, clipboard,
/// plugins, and persistence lifecycle; process exit belongs here.
pub struct Application {
    windows: HashMap<WindowId, App>,
    /// Native creation order gives "next window" deterministic wraparound
    /// semantics independent of HashMap iteration order.
    window_order: Vec<WindowId>,
    /// Monotonic control-plane IDs remain stable when sibling windows close.
    control_window_ids: HashMap<WindowId, u64>,
    next_control_window_id: u64,
    control_replays: ControlReplayCache,
    pending_domain_controls: Vec<PendingDomainControl>,
    pending: Vec<App>,
    /// Most recent process-wide desktop preferences, applied to existing and
    /// newly created windows without persisting native state into config.
    system_accessibility: super::system_accessibility::SystemAccessibilityPreferences,
    system_accessibility_monitor_started: bool,
    /// Preserved when every requested startup window fails before becoming
    /// visible. Winit's handler API cannot return this error directly.
    fatal_startup_error: Option<String>,
}

const MAX_CONTROL_REPLAYS: usize = 256;
const MAX_PENDING_DOMAIN_CONTROLS: usize = 32;
const MAX_PENDING_DOMAIN_WAITERS: usize = 16;
type ControlResponder =
    std::sync::mpsc::SyncSender<std::result::Result<crate::control::protocol::Response, String>>;

struct PendingDomainControl {
    request: crate::control::protocol::Request,
    window_id: WindowId,
    control: super::LiveDomainControl,
    context: PendingDomainContext,
    responders: Vec<ControlResponder>,
}

#[derive(Clone, Copy)]
enum PendingDomainContext {
    None,
    ResizeDivider { previous_ratio_millis: u16 },
    MoveTab { changed: bool },
}

#[derive(Default)]
struct ControlReplayCache {
    entries: VecDeque<ControlReplay>,
}

struct ControlReplay {
    request: crate::control::protocol::Request,
    result: std::result::Result<crate::control::protocol::Response, String>,
    terminal_failure: bool,
}

impl ControlReplayCache {
    fn replay(
        &self,
        request: &crate::control::protocol::Request,
    ) -> Option<std::result::Result<crate::control::protocol::Response, String>> {
        let request_id = request.request_id()?;
        let replay = self
            .entries
            .iter()
            .find(|entry| entry.request.request_id() == Some(request_id))?;
        Some(if replay.request == *request {
            match &replay.result {
                Ok(response) => Ok(response.clone()),
                Err(error) if replay.terminal_failure => {
                    // A PTY write or remote authority can report an error after
                    // accepting a mutation. Never reevaluate the same logical
                    // request when its outcome may be ambiguous.
                    Err(error.clone())
                }
                // A failed mutation changed no state, so retrying it is safe
                // and allows transient native conditions to recover.
                Err(_) => return None,
            }
        } else {
            Err("GUI control request ID was already used for a different action".into())
        })
    }

    fn record(
        &mut self,
        request: crate::control::protocol::Request,
        result: std::result::Result<crate::control::protocol::Response, String>,
    ) {
        let terminal_failure =
            matches!(request, crate::control::protocol::Request::SendText { .. });
        self.record_with_policy(request, result, terminal_failure);
    }

    fn record_terminal(
        &mut self,
        request: crate::control::protocol::Request,
        result: std::result::Result<crate::control::protocol::Response, String>,
    ) {
        self.record_with_policy(request, result, true);
    }

    fn record_with_policy(
        &mut self,
        request: crate::control::protocol::Request,
        result: std::result::Result<crate::control::protocol::Response, String>,
        terminal_failure: bool,
    ) {
        debug_assert!(request.request_id().is_some());
        if let Some(existing) = self
            .entries
            .iter_mut()
            .find(|entry| entry.request.request_id() == request.request_id())
        {
            debug_assert_eq!(existing.request, request);
            existing.result = result;
            existing.terminal_failure |= terminal_failure;
            return;
        }
        if self.entries.len() == MAX_CONTROL_REPLAYS {
            self.entries.pop_front();
        }
        self.entries.push_back(ControlReplay {
            request,
            result,
            terminal_failure,
        });
    }
}

fn bounded_control_error(error: &str) -> String {
    const MAX_ERROR_BYTES: usize = 1_024;
    let mut output = String::with_capacity(error.len().min(MAX_ERROR_BYTES));
    for character in error.chars().filter(|character| !character.is_control()) {
        if output.len() + character.len_utf8() > MAX_ERROR_BYTES {
            break;
        }
        output.push(character);
    }
    if output.is_empty() {
        "GUI control action failed".into()
    } else {
        output
    }
}

fn live_layout_has_exact_move_split(
    layout: &crate::domain::model::LivePaneLayout,
    target_pane: u32,
    moved_pane: u32,
    direction: crate::domain::model::SplitDirection,
) -> bool {
    use crate::domain::model::LivePaneLayout;
    match layout {
        LivePaneLayout::Pane { .. } => false,
        LivePaneLayout::Split {
            direction: candidate,
            first,
            second,
            ..
        } => {
            (*candidate == direction
                && matches!(first.as_ref(), LivePaneLayout::Pane { pane } if *pane == target_pane)
                && matches!(second.as_ref(), LivePaneLayout::Pane { pane } if *pane == moved_pane))
                || live_layout_has_exact_move_split(first, target_pane, moved_pane, direction)
                || live_layout_has_exact_move_split(second, target_pane, moved_pane, direction)
        }
    }
}

impl Application {
    pub fn new(launch: LaunchPlan) -> Result<Self> {
        let (config, warning) = Config::load_for_startup();
        let state = super::state::AppState::load();
        let automatic_restore = launch == LaunchPlan::default()
            && config.session.enabled
            && config.session.auto_restore
            && !state.restore_workspace_ids().is_empty();
        let launch = expand_auto_restore_plan(&config, &state, launch);
        let mut application = Self::from_config(config, warning, launch)?;
        if automatic_restore {
            for window in &mut application.pending {
                window.allow_workspace_fallback = true;
            }
        }
        Ok(application)
    }

    fn from_config(
        config: Config,
        mut warning: Option<String>,
        launch: LaunchPlan,
    ) -> Result<Self> {
        validate_cli_launches(&config, &launch)?;
        let sole_default =
            launch.windows.len() == 1 && launch.windows.first() == Some(&WindowLaunch::default());
        let mut pending = Vec::with_capacity(launch.windows.len());
        for window_launch in launch.windows {
            let quick_access = window_launch.quick_access;
            let mut window_config = config.clone();
            if let Some(backend) = window_launch.renderer_backend {
                window_config.renderer.backend = backend;
            }
            let mut window = App::from_config(window_config, warning.take(), true)?;
            window.launch = window_launch;
            window.managed_window = true;
            window.allow_auto_restore = sole_default;
            if quick_access {
                window.allow_auto_restore = false;
                window.retain_workspace_on_exit = false;
                window.automatic_persistence = false;
            }
            pending.push(window);
        }
        Ok(Self {
            windows: HashMap::new(),
            window_order: Vec::new(),
            control_window_ids: HashMap::new(),
            next_control_window_id: 1,
            control_replays: ControlReplayCache::default(),
            pending_domain_controls: Vec::new(),
            pending,
            system_accessibility:
                super::system_accessibility::SystemAccessibilityPreferences::default(),
            system_accessibility_monitor_started: false,
            fatal_startup_error: None,
        })
    }

    fn create_pending_windows(
        &mut self,
        event_loop: &ActiveEventLoop,
    ) -> Vec<std::result::Result<u64, String>> {
        let mut startup_errors = Vec::new();
        let mut results = Vec::with_capacity(self.pending.len());
        for mut window in self.pending.drain(..) {
            ApplicationHandler::resumed(&mut window, event_loop);
            if window.startup_error.is_none()
                && !window.should_exit
                && let Some(id) = window.window.as_ref().map(|native| native.id())
            {
                let control_id = self.next_control_window_id;
                self.window_order.push(id);
                self.control_window_ids.insert(id, control_id);
                self.next_control_window_id = self.next_control_window_id.saturating_add(1);
                self.windows.insert(id, window);
                results.push(Ok(control_id));
            } else {
                let error = window
                    .startup_error
                    .take()
                    .unwrap_or_else(|| "Window creation stopped before becoming usable".into());
                startup_errors.push(error.clone());
                results.push(Err(error));
            }
        }
        self.record_fatal_startup_errors(startup_errors);
        results
    }

    fn record_fatal_startup_errors(&mut self, errors: Vec<String>) {
        if self.windows.is_empty() && self.pending.is_empty() && !errors.is_empty() {
            self.fatal_startup_error = Some(errors.join("\n"));
        }
    }

    pub(crate) fn take_fatal_startup_error(&mut self) -> Option<String> {
        self.fatal_startup_error.take()
    }

    fn queue_default_window(&mut self, config: Config, cwd: Option<String>) -> Result<()> {
        let mut launch = WindowLaunch::default();
        launch.tabs[0].working_directory = cwd.map(Into::into);
        self.queue_window(config, launch)
    }

    fn queue_window(&mut self, config: Config, launch: WindowLaunch) -> Result<()> {
        let mut window = App::from_config(config, None, true)?;
        window.set_system_accessibility_preferences(self.system_accessibility);
        window.launch = launch;
        window.managed_window = true;
        window.allow_auto_restore = false;
        self.pending.push(window);
        Ok(())
    }

    fn control_inventory(&self) -> crate::control::protocol::ProcessInventory {
        let mut truncated = self.window_order.len() > crate::control::protocol::MAX_WINDOWS;
        let mut budget = super::ControlInventoryBudget::default();
        let mut windows = Vec::with_capacity(
            self.window_order
                .len()
                .min(crate::control::protocol::MAX_WINDOWS),
        );
        for native_id in self
            .window_order
            .iter()
            .take(crate::control::protocol::MAX_WINDOWS)
        {
            let Some(window) = self.windows.get(native_id) else {
                truncated = true;
                continue;
            };
            let Some(control_id) = self.control_window_ids.get(native_id) else {
                truncated = true;
                continue;
            };
            let inventory = window.control_inventory(*control_id, &mut budget);
            truncated |= inventory.truncated;
            windows.push(inventory);
        }
        crate::control::protocol::ProcessInventory {
            process_id: std::process::id(),
            windows,
            truncated,
        }
    }

    fn plugin_process_inventory(&self) -> crate::control::protocol::ProcessInventory {
        let live_window_count = self
            .window_order
            .iter()
            .filter(|native_id| {
                self.windows
                    .get(native_id)
                    .is_some_and(|window| !window.should_exit)
            })
            .count();
        let windows: Vec<_> = self
            .window_order
            .iter()
            .filter_map(|native_id| {
                let window = self.windows.get(native_id)?;
                let control_id = *self.control_window_ids.get(native_id)?;
                Some((control_id, window))
            })
            .collect();
        let mut inventory = build_plugin_process_inventory(&windows);
        inventory.truncated |= windows
            .iter()
            .filter(|(_, window)| !window.should_exit)
            .count()
            != live_window_count;
        inventory
    }

    fn plugin_process_action_event(
        &mut self,
        plugin_id: &str,
        origin_window_id: u64,
        action: crate::plugin::structured::StructuredResponseAction,
    ) -> Result<String, String> {
        const MAX_ACTION_ERROR_BYTES: usize = 1024;

        let event = match action {
            crate::plugin::structured::StructuredResponseAction::SelectProcessTab {
                action_id,
                window_id,
                tab_id,
            } => {
                let result = self
                    .control_native_window_id(window_id)
                    .and_then(|native_id| {
                        self.windows
                            .get_mut(&native_id)
                            .filter(|window| !window.should_exit)
                            .ok_or_else(|| format!("GUI window {window_id} is closing"))?
                            .select_plugin_tab(tab_id)
                    });
                match result {
                    Ok(changed) => serde_json::json!({
                        "type": "process-action-result",
                        "action_id": action_id,
                        "action": "select-process-tab",
                        "ok": true,
                        "process_id": std::process::id(),
                        "origin_window_id": origin_window_id,
                        "window_id": window_id,
                        "tab_id": tab_id,
                        "changed": changed,
                        "error": null,
                    }),
                    Err(error) => {
                        let mut budget = MAX_ACTION_ERROR_BYTES;
                        let (error, error_truncated) = super::control_metadata(&error, &mut budget);
                        serde_json::json!({
                            "type": "process-action-result",
                            "action_id": action_id,
                            "action": "select-process-tab",
                            "ok": false,
                            "process_id": std::process::id(),
                            "origin_window_id": origin_window_id,
                            "window_id": window_id,
                            "tab_id": tab_id,
                            "changed": null,
                            "error": error,
                            "error_truncated": error_truncated,
                        })
                    }
                }
            }
            crate::plugin::structured::StructuredResponseAction::FocusProcessPane {
                action_id,
                window_id,
                tab_id,
                pane_id,
            } => {
                let result = self
                    .control_native_window_id(window_id)
                    .and_then(|native_id| {
                        self.windows
                            .get_mut(&native_id)
                            .filter(|window| !window.should_exit)
                            .ok_or_else(|| format!("GUI window {window_id} is closing"))?
                            .focus_plugin_pane(tab_id, pane_id)
                    });
                match result {
                    Ok((tab_changed, pane_changed)) => serde_json::json!({
                        "type": "process-action-result",
                        "action_id": action_id,
                        "action": "focus-process-pane",
                        "ok": true,
                        "process_id": std::process::id(),
                        "origin_window_id": origin_window_id,
                        "window_id": window_id,
                        "tab_id": tab_id,
                        "pane_id": pane_id,
                        "tab_changed": tab_changed,
                        "pane_changed": pane_changed,
                        "error": null,
                    }),
                    Err(error) => {
                        let mut budget = MAX_ACTION_ERROR_BYTES;
                        let (error, error_truncated) = super::control_metadata(&error, &mut budget);
                        serde_json::json!({
                            "type": "process-action-result",
                            "action_id": action_id,
                            "action": "focus-process-pane",
                            "ok": false,
                            "process_id": std::process::id(),
                            "origin_window_id": origin_window_id,
                            "window_id": window_id,
                            "tab_id": tab_id,
                            "pane_id": pane_id,
                            "tab_changed": null,
                            "pane_changed": null,
                            "error": error,
                            "error_truncated": error_truncated,
                        })
                    }
                }
            }
            crate::plugin::structured::StructuredResponseAction::CreateProcessTab {
                action_id,
                window_id,
            } => {
                let result = self
                    .control_native_window_id(window_id)
                    .and_then(|native_id| {
                        self.windows
                            .get_mut(&native_id)
                            .filter(|window| !window.should_exit)
                            .ok_or_else(|| format!("GUI window {window_id} is closing"))?
                            .create_plugin_tab()
                    });
                match result {
                    Ok(tab_id) => serde_json::json!({
                        "type": "process-action-result",
                        "action_id": action_id,
                        "action": "create-process-tab",
                        "ok": true,
                        "process_id": std::process::id(),
                        "origin_window_id": origin_window_id,
                        "window_id": window_id,
                        "tab_id": tab_id,
                        "selected": true,
                        "error": null,
                    }),
                    Err(error) => {
                        let mut budget = MAX_ACTION_ERROR_BYTES;
                        let (error, error_truncated) = super::control_metadata(&error, &mut budget);
                        serde_json::json!({
                            "type": "process-action-result",
                            "action_id": action_id,
                            "action": "create-process-tab",
                            "ok": false,
                            "process_id": std::process::id(),
                            "origin_window_id": origin_window_id,
                            "window_id": window_id,
                            "tab_id": null,
                            "selected": null,
                            "error": error,
                            "error_truncated": error_truncated,
                        })
                    }
                }
            }
            _ => return Err("structured response did not contain a process action".into()),
        };
        let envelope = serde_json::json!({
            "schema_version": 1,
            "plugin_id": plugin_id,
            "event": event,
        });
        let encoded = serde_json::to_string(&envelope)
            .map_err(|error| format!("could not encode process action result: {error}"))?;
        if encoded.len() > crate::plugin::structured::MAX_PERSISTENT_SOURCE_EVENT_BYTES {
            return Err(format!(
                "process action result exceeds {} bytes",
                crate::plugin::structured::MAX_PERSISTENT_SOURCE_EVENT_BYTES
            ));
        }
        Ok(encoded)
    }

    #[allow(clippy::too_many_arguments)]
    fn plugin_process_terminal_query_event(
        &self,
        plugin_id: &str,
        origin_window_id: u64,
        query_id: &str,
        window_id: u64,
        tab_id: u64,
        pane_id: u32,
        history_lines: u16,
        max_bytes: u32,
    ) -> Result<String, String> {
        if history_lines > crate::control::protocol::MAX_TEXT_HISTORY_LINES {
            return Err(format!(
                "process terminal history is limited to {} lines",
                crate::control::protocol::MAX_TEXT_HISTORY_LINES
            ));
        }
        if !(1..=crate::plugin::structured::MAX_TERMINAL_QUERY_BYTES).contains(&max_bytes) {
            return Err(format!(
                "process terminal query bytes must be between 1 and {}",
                crate::plugin::structured::MAX_TERMINAL_QUERY_BYTES
            ));
        }
        let mut attempt_bytes = max_bytes;
        loop {
            let result = self
                .control_native_window_id(window_id)
                .and_then(|native_id| {
                    self.windows
                        .get(&native_id)
                        .filter(|window| !window.should_exit)
                        .ok_or_else(|| format!("GUI window {window_id} is closing"))?
                        .read_control_text(tab_id, pane_id, history_lines, attempt_bytes)
                });
            let event = match result {
                Ok((screen, included_history, text, truncated)) => serde_json::json!({
                    "type": "process-terminal-query-result",
                    "query_id": query_id,
                    "text_schema_version": 1,
                    "ok": true,
                    "process_id": std::process::id(),
                    "origin_window_id": origin_window_id,
                    "window_id": window_id,
                    "tab_id": tab_id,
                    "pane_id": pane_id,
                    "screen": screen,
                    "requested_history_lines": history_lines,
                    "requested_max_bytes": max_bytes,
                    "history_lines": included_history,
                    "text_bytes": text.len(),
                    "text": text,
                    "truncated": truncated || attempt_bytes < max_bytes,
                    "error": null,
                }),
                Err(error) => {
                    let mut budget = 1024;
                    let (error, error_truncated) = super::control_metadata(&error, &mut budget);
                    serde_json::json!({
                        "type": "process-terminal-query-result",
                        "query_id": query_id,
                        "text_schema_version": 1,
                        "ok": false,
                        "process_id": std::process::id(),
                        "origin_window_id": origin_window_id,
                        "window_id": window_id,
                        "tab_id": tab_id,
                        "pane_id": pane_id,
                        "screen": null,
                        "requested_history_lines": history_lines,
                        "requested_max_bytes": max_bytes,
                        "history_lines": null,
                        "text_bytes": 0,
                        "text": null,
                        "truncated": null,
                        "error": error,
                        "error_truncated": error_truncated,
                    })
                }
            };
            let envelope = serde_json::json!({
                "schema_version": 1,
                "plugin_id": plugin_id,
                "event": event,
            });
            let encoded = serde_json::to_string(&envelope).map_err(|error| {
                format!("could not encode process terminal query result: {error}")
            })?;
            let limit = crate::plugin::structured::MAX_PERSISTENT_SOURCE_EVENT_BYTES;
            if encoded.len() <= limit {
                return Ok(encoded);
            }
            if attempt_bytes == 1 {
                return Err(format!(
                    "process terminal query result exceeds {limit} bytes"
                ));
            }
            let scaled = (usize::try_from(attempt_bytes).expect("u32 fits usize") * limit
                / encoded.len())
            .min(usize::try_from(attempt_bytes - 1).expect("u32 fits usize"))
            .max(1);
            attempt_bytes =
                u32::try_from(scaled).expect("scaled process terminal query budget fits u32");
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn plugin_process_terminal_input_event(
        &mut self,
        plugin_id: &str,
        origin_window_id: u64,
        action_id: &str,
        window_id: u64,
        tab_id: u64,
        pane_id: u32,
        input_text: &str,
        paste: bool,
        enter: bool,
    ) -> Result<String, String> {
        if action_id.trim().is_empty()
            || action_id.len() > crate::plugin::structured::MAX_ACTION_ID_BYTES
            || action_id.chars().any(char::is_control)
        {
            return Err(format!(
                "process terminal input action id must be non-empty, control-safe UTF-8 within {} bytes",
                crate::plugin::structured::MAX_ACTION_ID_BYTES
            ));
        }
        if input_text.len() > crate::plugin::structured::MAX_TERMINAL_INPUT_BYTES {
            return Err(format!(
                "plugin process terminal input is limited to {} UTF-8 bytes",
                crate::plugin::structured::MAX_TERMINAL_INPUT_BYTES
            ));
        }
        if input_text.is_empty() && !enter {
            return Err("plugin process terminal input must include text or enter".into());
        }
        if !crate::control::protocol::valid_control_input_text(input_text) {
            return Err(
                "plugin process terminal input permits printable UTF-8, tab, and newline only"
                    .into(),
            );
        }

        let result = self
            .control_native_window_id(window_id)
            .and_then(|native_id| {
                self.windows
                    .get_mut(&native_id)
                    .filter(|window| !window.should_exit)
                    .ok_or_else(|| format!("GUI window {window_id} is closing"))?
                    .send_process_plugin_text(window_id, tab_id, pane_id, input_text, paste, enter)
            });
        let event = match result {
            Ok((input_bytes, wire_bytes, disposition, confirmation_id)) => serde_json::json!({
                "type": "process-action-result",
                "action_id": action_id,
                "action": "send-process-text",
                "ok": true,
                "process_id": std::process::id(),
                "origin_window_id": origin_window_id,
                "window_id": window_id,
                "tab_id": tab_id,
                "pane_id": pane_id,
                "input_bytes": input_bytes,
                "wire_bytes": wire_bytes,
                "paste": paste,
                "enter": enter,
                "disposition": disposition,
                "confirmation_id": confirmation_id,
                "error": null,
            }),
            Err(error) => {
                let mut budget = 1024;
                let (error, error_truncated) = super::control_metadata(&error, &mut budget);
                serde_json::json!({
                    "type": "process-action-result",
                    "action_id": action_id,
                    "action": "send-process-text",
                    "ok": false,
                    "process_id": std::process::id(),
                    "origin_window_id": origin_window_id,
                    "window_id": window_id,
                    "tab_id": tab_id,
                    "pane_id": pane_id,
                    "input_bytes": null,
                    "wire_bytes": null,
                    "paste": paste,
                    "enter": enter,
                    "disposition": null,
                    "confirmation_id": null,
                    "error": error,
                    "error_truncated": error_truncated,
                })
            }
        };
        let envelope = serde_json::json!({
            "schema_version": 1,
            "plugin_id": plugin_id,
            "event": event,
        });
        let encoded = serde_json::to_string(&envelope)
            .map_err(|error| format!("could not encode process terminal input result: {error}"))?;
        if encoded.len() > crate::plugin::structured::MAX_PERSISTENT_SOURCE_EVENT_BYTES {
            return Err(format!(
                "process terminal input result exceeds {} bytes",
                crate::plugin::structured::MAX_PERSISTENT_SOURCE_EVENT_BYTES
            ));
        }
        Ok(encoded)
    }

    fn handle_structured_plugin_completed(
        &mut self,
        mut completion: crate::plugin::structured::StructuredHandlerCompletion,
    ) {
        let process_operation = completion.result.as_ref().ok().and_then(|actions| {
            actions.iter().find(|action| {
                matches!(
                    action,
                    crate::plugin::structured::StructuredResponseAction::QueryProcess { .. }
                        | crate::plugin::structured::StructuredResponseAction::SelectProcessTab {
                            ..
                        }
                        | crate::plugin::structured::StructuredResponseAction::FocusProcessPane {
                            ..
                        }
                        | crate::plugin::structured::StructuredResponseAction::QueryProcessTerminal {
                            ..
                        }
                        | crate::plugin::structured::StructuredResponseAction::SendProcessText {
                            ..
                        }
                        | crate::plugin::structured::StructuredResponseAction::CreateProcessTab {
                            ..
                        }
                )
            })
        });
        let Some(process_operation) = process_operation.cloned() else {
            if let Some(window) = self.windows.get_mut(&completion.window_id) {
                window.handle_structured_plugin_completed(completion);
            }
            return;
        };

        let Some(origin) = self.windows.get(&completion.window_id) else {
            return;
        };
        if origin.should_exit {
            log::debug!(
                "Discarding process operation from plugin {:?}: originating window is closing",
                completion.plugin_id
            );
            return;
        }
        if completion.plugin_generation != origin.plugin_runtime_generation
            || !origin.plugin_manager.contains(&completion.plugin_id)
        {
            if let Some(window) = self.windows.get_mut(&completion.window_id) {
                window.handle_structured_plugin_completed(completion);
            }
            return;
        }
        let has_continuation_authority =
            completion
                .continuation
                .as_ref()
                .is_some_and(|continuation| match &process_operation {
                    crate::plugin::structured::StructuredResponseAction::QueryProcess {
                        ..
                    } => continuation.allow_process_query,
                    crate::plugin::structured::StructuredResponseAction::SelectProcessTab {
                        ..
                    }
                    | crate::plugin::structured::StructuredResponseAction::FocusProcessPane {
                        ..
                    } => continuation.allow_process_actions,
                    crate::plugin::structured::StructuredResponseAction::QueryProcessTerminal {
                        ..
                    } => continuation.allow_process_terminal_text,
                    crate::plugin::structured::StructuredResponseAction::SendProcessText {
                        ..
                    } => continuation.allow_process_terminal_input,
                    crate::plugin::structured::StructuredResponseAction::CreateProcessTab {
                        ..
                    } => continuation.allow_process_tab_create,
                    _ => false,
                });
        if !has_continuation_authority {
            log::warn!(
                "Refusing process operation from plugin {:?}: persistent continuation authority is unavailable",
                completion.plugin_id
            );
            if let Some(window) = self.windows.get_mut(&completion.window_id) {
                window.handle_structured_plugin_completed(completion);
            }
            return;
        }
        let Some(origin_window_id) = self.control_window_ids.get(&completion.window_id).copied()
        else {
            log::warn!(
                "Discarding process operation from plugin {:?}: originating window has no stable process ID",
                completion.plugin_id
            );
            return;
        };

        let plugin_id = completion.plugin_id.clone();
        let (operation_name, event_json) = match &process_operation {
            crate::plugin::structured::StructuredResponseAction::QueryProcess { query_id } => {
                let inventory = self.plugin_process_inventory();
                (
                    "query",
                    App::structured_process_query_event(
                        &plugin_id,
                        query_id,
                        origin_window_id,
                        &inventory,
                    ),
                )
            }
            crate::plugin::structured::StructuredResponseAction::SelectProcessTab { .. } => (
                "select-tab",
                self.plugin_process_action_event(
                    &plugin_id,
                    origin_window_id,
                    process_operation.clone(),
                ),
            ),
            crate::plugin::structured::StructuredResponseAction::FocusProcessPane { .. } => (
                "focus-pane",
                self.plugin_process_action_event(
                    &plugin_id,
                    origin_window_id,
                    process_operation.clone(),
                ),
            ),
            crate::plugin::structured::StructuredResponseAction::QueryProcessTerminal {
                query_id,
                window_id,
                tab_id,
                pane_id,
                history_lines,
                max_bytes,
            } => (
                "terminal-query",
                self.plugin_process_terminal_query_event(
                    &plugin_id,
                    origin_window_id,
                    query_id,
                    *window_id,
                    *tab_id,
                    *pane_id,
                    *history_lines,
                    *max_bytes,
                ),
            ),
            crate::plugin::structured::StructuredResponseAction::SendProcessText {
                action_id,
                window_id,
                tab_id,
                pane_id,
                text,
                paste,
                enter,
            } => (
                "terminal-input",
                self.plugin_process_terminal_input_event(
                    &plugin_id,
                    origin_window_id,
                    action_id,
                    *window_id,
                    *tab_id,
                    *pane_id,
                    text,
                    *paste,
                    *enter,
                ),
            ),
            crate::plugin::structured::StructuredResponseAction::CreateProcessTab { .. } => (
                "tab-create",
                self.plugin_process_action_event(
                    &plugin_id,
                    origin_window_id,
                    process_operation.clone(),
                ),
            ),
            _ => unreachable!("only process operations are intercepted"),
        };
        let continuation = completion.continuation.take();
        if let Ok(actions) = &mut completion.result {
            actions.retain(|action| {
                !matches!(
                    action,
                    crate::plugin::structured::StructuredResponseAction::QueryProcess { .. }
                        | crate::plugin::structured::StructuredResponseAction::SelectProcessTab {
                            ..
                        }
                        | crate::plugin::structured::StructuredResponseAction::FocusProcessPane {
                            ..
                        }
                        | crate::plugin::structured::StructuredResponseAction::QueryProcessTerminal {
                            ..
                        }
                        | crate::plugin::structured::StructuredResponseAction::SendProcessText {
                            ..
                        }
                        | crate::plugin::structured::StructuredResponseAction::CreateProcessTab {
                            ..
                        }
                )
            });
        }
        if let Some(window) = self.windows.get_mut(&completion.window_id) {
            window.handle_structured_plugin_completed(completion);
            let result = event_json.and_then(|event_json| match process_operation {
                crate::plugin::structured::StructuredResponseAction::QueryProcess { .. } => {
                    window.continue_structured_process_query(event_json, continuation)
                }
                crate::plugin::structured::StructuredResponseAction::SelectProcessTab {
                    ..
                }
                | crate::plugin::structured::StructuredResponseAction::FocusProcessPane {
                    ..
                }
                | crate::plugin::structured::StructuredResponseAction::CreateProcessTab {
                    ..
                } => window.continue_structured_process_action(event_json, continuation),
                crate::plugin::structured::StructuredResponseAction::QueryProcessTerminal {
                    ..
                } => window.continue_structured_process_terminal_query(event_json, continuation),
                crate::plugin::structured::StructuredResponseAction::SendProcessText { .. } => {
                    window.continue_structured_process_terminal_input(event_json, continuation)
                }
                _ => unreachable!("only process operations are intercepted"),
            });
            if let Err(error) = result {
                log::error!(
                    "Structured plugin {:?} process {} failed: {}",
                    plugin_id,
                    operation_name,
                    error
                );
                window.toasts.error(format!(
                    "Plugin {plugin_id} process {operation_name} failed: {error}"
                ));
            }
        }
    }

    fn handle_control_request(
        &mut self,
        event_loop: &ActiveEventLoop,
        request: crate::control::protocol::Request,
    ) -> std::result::Result<crate::control::protocol::Response, String> {
        use crate::control::protocol::{Request, Response};

        request.validate()?;
        if let Some(replay) = self.control_replays.replay(&request) {
            return replay;
        }
        if request == Request::Inventory {
            return Ok(Response::Inventory(self.control_inventory()));
        }
        if let Request::ReadText {
            window_id,
            tab_id,
            pane_id,
            history_lines,
            max_bytes,
        } = request
        {
            return self.read_control_text(window_id, tab_id, pane_id, history_lines, max_bytes);
        }

        let result = match &request {
            Request::CreateWindow {
                request_id,
                source_window,
            } => self.create_control_window(event_loop, request_id, *source_window),
            Request::CreateTab {
                request_id,
                window_id,
            } => self.create_control_tab(request_id, *window_id),
            Request::SplitPane {
                request_id,
                window_id,
                tab_id,
                pane_id,
                direction,
            } => self.create_control_pane(request_id, *window_id, *tab_id, *pane_id, *direction),
            Request::FocusWindow {
                request_id,
                window_id,
            } => self.focus_control_window(request_id, *window_id),
            Request::SetQuickAccessVisibility {
                request_id,
                window_id,
                visible,
            } => self.set_control_quick_access_visibility(request_id, *window_id, *visible),
            Request::SelectTab {
                request_id,
                window_id,
                tab_id,
            } => self.select_control_tab(request_id, *window_id, *tab_id),
            Request::FocusPane {
                request_id,
                window_id,
                tab_id,
                pane_id,
            } => self.focus_control_pane(request_id, *window_id, *tab_id, *pane_id),
            Request::CloseWindow {
                request_id,
                window_id,
            } => self.close_control_window(request_id, *window_id),
            Request::CloseTab {
                request_id,
                window_id,
                tab_id,
            } => self.close_control_tab(request_id, *window_id, *tab_id),
            Request::ClosePane {
                request_id,
                window_id,
                tab_id,
                pane_id,
            } => self.close_control_pane(request_id, *window_id, *tab_id, *pane_id),
            Request::MoveTab {
                request_id,
                source_window_id,
                tab_id,
                destination_window_id,
                destination_index,
            } => self.move_control_tab(
                request_id,
                *source_window_id,
                *tab_id,
                *destination_window_id,
                *destination_index,
            ),
            Request::MovePane {
                request_id,
                source_window_id,
                source_tab_id,
                pane_id,
                destination_window_id,
                destination_tab_id,
                target_pane_id,
                direction,
            } => self.move_control_pane(
                request_id,
                *source_window_id,
                *source_tab_id,
                *pane_id,
                *destination_window_id,
                *destination_tab_id,
                *target_pane_id,
                *direction,
            ),
            Request::ResizeWindow {
                request_id,
                window_id,
                width,
                height,
            } => self.resize_control_window(request_id, *window_id, *width, *height),
            Request::ResizeDivider {
                request_id,
                window_id,
                tab_id,
                divider_id,
                ratio_millis,
            } => self.resize_control_divider(
                request_id,
                *window_id,
                *tab_id,
                *divider_id,
                *ratio_millis,
            ),
            Request::SendText {
                request_id,
                window_id,
                tab_id,
                pane_id,
                text,
                paste,
                enter,
            } => self.send_control_text(
                request_id, *window_id, *tab_id, *pane_id, text, *paste, *enter,
            ),
            Request::Inventory | Request::ReadText { .. } => {
                unreachable!("read-only requests returned before mutation dispatch")
            }
        }
        .map_err(|error| bounded_control_error(&error));
        self.control_replays.record(request, result.clone());
        result
    }

    fn defer_live_domain_control(
        &mut self,
        request: &crate::control::protocol::Request,
        responder: ControlResponder,
    ) -> Option<ControlResponder> {
        use crate::control::protocol::Request;

        let Some(request_id) = request.request_id() else {
            return Some(responder);
        };
        if let Some(pending) = self
            .pending_domain_controls
            .iter_mut()
            .find(|pending| pending.request.request_id() == Some(request_id))
        {
            if pending.request != *request {
                let _ = responder.send(Err(
                    "GUI control request ID was already used for a different action".into(),
                ));
            } else if pending.responders.len() >= MAX_PENDING_DOMAIN_WAITERS {
                let _ = responder.send(Err(
                    "Too many clients are waiting for this live-domain control request".into(),
                ));
            } else {
                pending.responders.push(responder);
            }
            return None;
        }
        if let Some(replay) = self.control_replays.replay(request) {
            let _ = responder.send(replay);
            return None;
        }

        let (window_id, control, context) = match request {
            Request::CreateTab { window_id, .. } => {
                let native_id = match self.control_native_window_id(*window_id) {
                    Ok(id) => id,
                    Err(_) => return Some(responder),
                };
                let Some(window) = self
                    .windows
                    .get(&native_id)
                    .filter(|window| !window.should_exit)
                else {
                    return Some(responder);
                };
                match window.control_live_domain_create_tab() {
                    Ok(Some(control)) => (native_id, control, PendingDomainContext::None),
                    Ok(None) | Err(_) => return Some(responder),
                }
            }
            Request::SplitPane {
                window_id,
                tab_id,
                pane_id,
                direction,
                ..
            } => {
                let native_id = match self.control_native_window_id(*window_id) {
                    Ok(id) => id,
                    Err(_) => return Some(responder),
                };
                let Some(window) = self
                    .windows
                    .get(&native_id)
                    .filter(|window| !window.should_exit)
                else {
                    return Some(responder);
                };
                match window.control_live_domain_split(*tab_id, *pane_id, *direction) {
                    Ok(Some(control)) => (native_id, control, PendingDomainContext::None),
                    Ok(None) | Err(_) => return Some(responder),
                }
            }
            Request::CloseTab {
                window_id, tab_id, ..
            } => {
                let native_id = match self.control_native_window_id(*window_id) {
                    Ok(id) => id,
                    Err(_) => return Some(responder),
                };
                let Some(window) = self
                    .windows
                    .get(&native_id)
                    .filter(|window| !window.should_exit)
                else {
                    return Some(responder);
                };
                match window.control_live_domain_close_tab(*tab_id) {
                    Ok(Some(control)) => (native_id, control, PendingDomainContext::None),
                    Ok(None) => return Some(responder),
                    Err(error) => {
                        let result = Err(bounded_control_error(&error));
                        self.control_replays.record(request.clone(), result.clone());
                        let _ = responder.send(result);
                        return None;
                    }
                }
            }
            Request::ClosePane {
                window_id,
                tab_id,
                pane_id,
                ..
            } => {
                let native_id = match self.control_native_window_id(*window_id) {
                    Ok(id) => id,
                    Err(_) => return Some(responder),
                };
                let Some(window) = self
                    .windows
                    .get(&native_id)
                    .filter(|window| !window.should_exit)
                else {
                    return Some(responder);
                };
                match window.control_live_domain_close_pane(*tab_id, *pane_id) {
                    Ok(Some(control)) => (native_id, control, PendingDomainContext::None),
                    Ok(None) => return Some(responder),
                    Err(error) => {
                        let result = Err(bounded_control_error(&error));
                        self.control_replays.record(request.clone(), result.clone());
                        let _ = responder.send(result);
                        return None;
                    }
                }
            }
            Request::ResizeDivider {
                window_id,
                tab_id,
                divider_id,
                ratio_millis,
                ..
            } => {
                let native_id = match self.control_native_window_id(*window_id) {
                    Ok(id) => id,
                    Err(_) => return Some(responder),
                };
                let Some(window) = self
                    .windows
                    .get(&native_id)
                    .filter(|window| !window.should_exit)
                else {
                    return Some(responder);
                };
                match window.control_live_domain_resize_divider(*tab_id, *divider_id, *ratio_millis)
                {
                    Ok(Some((control, previous_ratio_millis))) => (
                        native_id,
                        control,
                        PendingDomainContext::ResizeDivider {
                            previous_ratio_millis,
                        },
                    ),
                    Ok(None) => return Some(responder),
                    Err(error) => {
                        let result = Err(bounded_control_error(&error));
                        self.control_replays.record(request.clone(), result.clone());
                        let _ = responder.send(result);
                        return None;
                    }
                }
            }
            Request::MoveTab {
                source_window_id,
                tab_id,
                destination_window_id,
                destination_index,
                ..
            } if source_window_id == destination_window_id => {
                let native_id = match self.control_native_window_id(*source_window_id) {
                    Ok(id) => id,
                    Err(_) => return Some(responder),
                };
                let Some(window) = self
                    .windows
                    .get(&native_id)
                    .filter(|window| !window.should_exit)
                else {
                    return Some(responder);
                };
                match window.control_live_domain_move_tab(*tab_id, *destination_index) {
                    Ok(Some((control, changed))) => (
                        native_id,
                        control,
                        PendingDomainContext::MoveTab { changed },
                    ),
                    Ok(None) => return Some(responder),
                    Err(error) => {
                        let result = Err(bounded_control_error(&error));
                        self.control_replays.record(request.clone(), result.clone());
                        let _ = responder.send(result);
                        return None;
                    }
                }
            }
            Request::MovePane {
                source_window_id,
                source_tab_id,
                pane_id,
                destination_window_id,
                destination_tab_id,
                target_pane_id,
                direction,
                ..
            } if source_window_id == destination_window_id => {
                let native_id = match self.control_native_window_id(*source_window_id) {
                    Ok(id) => id,
                    Err(_) => return Some(responder),
                };
                let Some(window) = self
                    .windows
                    .get(&native_id)
                    .filter(|window| !window.should_exit)
                else {
                    return Some(responder);
                };
                match window.control_live_domain_move_pane(
                    *source_tab_id,
                    *pane_id,
                    *destination_tab_id,
                    *target_pane_id,
                    *direction,
                ) {
                    Ok(Some(control)) => (native_id, control, PendingDomainContext::None),
                    Ok(None) => return Some(responder),
                    Err(error) => {
                        let result = Err(bounded_control_error(&error));
                        self.control_replays.record(request.clone(), result.clone());
                        let _ = responder.send(result);
                        return None;
                    }
                }
            }
            _ => return Some(responder),
        };
        if self.pending_domain_controls.len() >= MAX_PENDING_DOMAIN_CONTROLS {
            let result = Err("Too many live-domain control requests are pending".into());
            self.control_replays.record(request.clone(), result.clone());
            let _ = responder.send(result);
            return None;
        }
        let result = self
            .windows
            .get_mut(&window_id)
            .expect("resolved live control window remains present")
            .start_live_domain_control(control.clone());
        if let Err(error) = result {
            let result = Err(bounded_control_error(&error));
            self.control_replays.record(request.clone(), result.clone());
            let _ = responder.send(result);
            return None;
        }
        self.pending_domain_controls.push(PendingDomainControl {
            request: request.clone(),
            window_id,
            control,
            context,
            responders: vec![responder],
        });
        None
    }

    fn complete_live_domain_control(
        &mut self,
        window_id: WindowId,
        control: &super::LiveDomainControl,
        result: &std::result::Result<crate::domain::model::DomainSnapshot, String>,
    ) {
        use crate::control::protocol::{Request, Response};

        let Some(index) = self
            .pending_domain_controls
            .iter()
            .position(|pending| pending.window_id == window_id && pending.control == *control)
        else {
            return;
        };
        let pending = self.pending_domain_controls.remove(index);
        let response = match result {
            Err(error) => Err(bounded_control_error(error)),
            Ok(snapshot) => match &pending.request {
                Request::CreateTab {
                    request_id,
                    window_id: control_window_id,
                } => self
                    .windows
                    .get(&window_id)
                    .and_then(|window| {
                        window
                            .control_local_live_tab_id(&snapshot.id, snapshot.workspace.active_tab)
                    })
                    .map(|tab_id| Response::TabCreated {
                        process_id: std::process::id(),
                        request_id: request_id.clone(),
                        window_id: *control_window_id,
                        tab_id,
                    })
                    .ok_or_else(|| {
                        "Live-domain tab was created but its GUI stable ID is unavailable".into()
                    }),
                Request::SplitPane {
                    request_id,
                    window_id: control_window_id,
                    tab_id,
                    ..
                } => snapshot
                    .workspace
                    .tabs
                    .iter()
                    .find(|tab| tab.id == snapshot.workspace.active_tab)
                    .and_then(|tab| {
                        self.windows.get(&window_id).and_then(|window| {
                            window.control_local_live_pane_id(
                                &snapshot.id,
                                tab.id,
                                tab.focused_pane,
                            )
                        })
                    })
                    .map(|pane_id| Response::PaneCreated {
                        process_id: std::process::id(),
                        request_id: request_id.clone(),
                        window_id: *control_window_id,
                        tab_id: *tab_id,
                        pane_id,
                    })
                    .ok_or_else(|| {
                        "Live-domain pane was created but its GUI stable ID is unavailable".into()
                    }),
                Request::CloseTab {
                    request_id,
                    window_id: control_window_id,
                    tab_id,
                } => match control {
                    super::LiveDomainControl::CloseTab { remote_tab_id, .. }
                        if !snapshot
                            .workspace
                            .tabs
                            .iter()
                            .any(|tab| tab.id == *remote_tab_id) =>
                    {
                        Ok(Response::TabCloseRequested {
                            process_id: std::process::id(),
                            request_id: request_id.clone(),
                            window_id: *control_window_id,
                            tab_id: *tab_id,
                        })
                    }
                    super::LiveDomainControl::CloseTab { .. } => Err(
                        "Live-domain tab closure returned a snapshot that still contains the tab"
                            .into(),
                    ),
                    _ => Err("Pending live-domain tab closure lost its exact target".into()),
                },
                Request::ClosePane {
                    request_id,
                    window_id: control_window_id,
                    tab_id,
                    pane_id,
                } => match control {
                    super::LiveDomainControl::ClosePane { remote_pane_id, .. }
                        if !snapshot
                            .workspace
                            .tabs
                            .iter()
                            .any(|tab| tab.panes.iter().any(|pane| pane.id == *remote_pane_id)) =>
                    {
                        self.windows
                            .get(&window_id)
                            .ok_or_else(|| {
                                "GUI window disappeared after live-domain pane closure".into()
                            })
                            .and_then(|window| window.control_live_pane_status(*tab_id))
                            .map(|(remaining_panes, focused_pane_id)| Response::PaneClosed {
                                process_id: std::process::id(),
                                request_id: request_id.clone(),
                                window_id: *control_window_id,
                                tab_id: *tab_id,
                                pane_id: *pane_id,
                                remaining_panes,
                                focused_pane_id,
                            })
                    }
                    super::LiveDomainControl::ClosePane { .. } => Err(
                        "Live-domain pane closure returned a snapshot that still contains the pane"
                            .into(),
                    ),
                    _ => Err("Pending live-domain pane closure lost its exact target".into()),
                },
                Request::ResizeDivider {
                    request_id,
                    window_id: control_window_id,
                    tab_id,
                    divider_id,
                    ratio_millis,
                } => match pending.context {
                    PendingDomainContext::ResizeDivider {
                        previous_ratio_millis,
                    } => self
                        .windows
                        .get(&window_id)
                        .ok_or_else(|| {
                            "GUI window disappeared after live-domain divider resize".into()
                        })
                        .and_then(|window| {
                            window.control_live_divider_ratio(*tab_id, *divider_id)
                        })
                        .and_then(|authoritative_ratio| {
                            if authoritative_ratio != *ratio_millis {
                                return Err(format!(
                                    "Live-domain divider resize returned ratio {authoritative_ratio}‰ instead of {ratio_millis}‰"
                                ));
                            }
                            Ok(Response::DividerResized {
                                process_id: std::process::id(),
                                request_id: request_id.clone(),
                                window_id: *control_window_id,
                                tab_id: *tab_id,
                                divider_id: *divider_id,
                                previous_ratio_millis,
                                ratio_millis: *ratio_millis,
                                changed: previous_ratio_millis != *ratio_millis,
                            })
                        }),
                    PendingDomainContext::None | PendingDomainContext::MoveTab { .. } => {
                        Err("Pending live-domain divider resize lost its previous ratio".into())
                    }
                },
                Request::MoveTab {
                    request_id,
                    source_window_id,
                    tab_id,
                    destination_window_id,
                    destination_index,
                } => match pending.context {
                    PendingDomainContext::MoveTab { changed } => self
                        .windows
                        .get(&window_id)
                        .ok_or_else(|| {
                            "GUI window disappeared after live-domain tab movement".into()
                        })
                        .and_then(|window| window.control_tab_visual_index(*tab_id))
                        .and_then(|authoritative_index| {
                            if authoritative_index != *destination_index {
                                return Err(format!(
                                    "Live-domain tab movement reconciled at position {authoritative_index} instead of {destination_index}"
                                ));
                            }
                            Ok(Response::TabMoved {
                                process_id: std::process::id(),
                                request_id: request_id.clone(),
                                source_window_id: *source_window_id,
                                source_tab_id: *tab_id,
                                destination_window_id: *destination_window_id,
                                destination_tab_id: *tab_id,
                                destination_index: *destination_index,
                                source_window_closing: false,
                                changed,
                            })
                        }),
                    PendingDomainContext::None
                    | PendingDomainContext::ResizeDivider { .. } => {
                        Err("Pending live-domain tab movement lost its source position".into())
                    }
                },
                Request::MovePane {
                    request_id,
                    source_window_id,
                    source_tab_id: source_local_tab_id,
                    pane_id,
                    destination_window_id,
                    destination_tab_id: destination_local_tab_id,
                    target_pane_id: target_local_pane_id,
                    ..
                } => match control {
                    super::LiveDomainControl::MovePane {
                        domain_id,
                        remote_pane_id,
                        source_tab_id: source_remote_tab_id,
                        destination_tab_id: destination_remote_tab_id,
                        target_pane_id: target_remote_pane_id,
                        direction,
                    } => {
                        let source = snapshot
                            .workspace
                            .tabs
                            .iter()
                            .find(|tab| tab.id == *source_remote_tab_id);
                        let destination = snapshot
                            .workspace
                            .tabs
                            .iter()
                            .find(|tab| tab.id == *destination_remote_tab_id);
                        if source.is_none_or(|tab| {
                            tab.panes.iter().any(|pane| pane.id == *remote_pane_id)
                        }) {
                            Err("Live-domain pane movement did not remove the source route".into())
                        } else if destination.is_none_or(|tab| {
                            !tab.panes.iter().any(|pane| pane.id == *remote_pane_id)
                                || !tab
                                    .panes
                                    .iter()
                                    .any(|pane| pane.id == *target_remote_pane_id)
                                || !live_layout_has_exact_move_split(
                                    &tab.layout,
                                    *target_remote_pane_id,
                                    *remote_pane_id,
                                    *direction,
                                )
                        }) {
                            Err(
                                "Live-domain pane movement did not preserve the exact destination anchor and direction"
                                    .into(),
                            )
                        } else {
                            self.windows
                                .get(&window_id)
                                .ok_or_else(|| {
                                    "GUI window disappeared after live-domain pane movement".into()
                                })
                                .and_then(|window| {
                                    let target = window.control_local_live_pane_id(
                                        domain_id,
                                        *destination_remote_tab_id,
                                        *target_remote_pane_id,
                                    );
                                    if target != Some(*target_local_pane_id) {
                                        return Err(
                                            "Live-domain pane movement changed or lost the exact destination anchor"
                                                .into(),
                                        );
                                    }
                                    window
                                        .control_local_live_pane_id(
                                            domain_id,
                                            *destination_remote_tab_id,
                                            *remote_pane_id,
                                        )
                                        .ok_or_else(|| {
                                            "Moved live-domain pane has no destination-local stable ID"
                                                .into()
                                        })
                                })
                                .map(|destination_pane_id| Response::PaneMoved {
                                    process_id: std::process::id(),
                                    request_id: request_id.clone(),
                                    source_window_id: *source_window_id,
                                    source_tab_id: *source_local_tab_id,
                                    source_pane_id: *pane_id,
                                    destination_window_id: *destination_window_id,
                                    destination_tab_id: *destination_local_tab_id,
                                    destination_pane_id,
                                    target_pane_id: *target_local_pane_id,
                                })
                        }
                    }
                    _ => Err("Pending live-domain pane movement lost its exact route".into()),
                },
                _ => Err("Pending live-domain control request has an invalid operation".into()),
            },
        };
        self.control_replays
            .record_terminal(pending.request, response.clone());
        for responder in pending.responders {
            let _ = responder.send(response.clone());
        }
    }

    fn create_control_window(
        &mut self,
        event_loop: &ActiveEventLoop,
        request_id: &str,
        source_window: u64,
    ) -> std::result::Result<crate::control::protocol::Response, String> {
        if self.windows.len().saturating_add(self.pending.len())
            >= crate::control::protocol::MAX_WINDOWS
        {
            return Err(format!(
                "GUI process window limit of {} reached",
                crate::control::protocol::MAX_WINDOWS
            ));
        }
        let source_native_id = self
            .control_window_ids
            .iter()
            .find_map(|(native_id, control_id)| {
                (*control_id == source_window).then_some(*native_id)
            })
            .ok_or_else(|| format!("GUI window {source_window} does not exist"))?;
        let source = self
            .windows
            .get(&source_native_id)
            .filter(|window| !window.should_exit)
            .ok_or_else(|| format!("GUI window {source_window} is closing"))?;
        let config = source.config.clone();
        let cwd = source
            .tabs
            .as_ref()
            .and_then(|tabs| tabs.active_tab())
            .and_then(|tab| tab.working_directory());
        let result_index = self.pending.len();
        self.queue_default_window(config, cwd)
            .map_err(|error| format!("Could not prepare a new window: {error:#}"))?;
        let creation = self
            .create_pending_windows(event_loop)
            .into_iter()
            .nth(result_index)
            .ok_or_else(|| "New window creation produced no result".to_string())?;
        let window_id = creation?;
        Ok(crate::control::protocol::Response::WindowCreated {
            process_id: std::process::id(),
            request_id: request_id.to_string(),
            window_id,
        })
    }

    fn create_control_tab(
        &mut self,
        request_id: &str,
        window_id: u64,
    ) -> std::result::Result<crate::control::protocol::Response, String> {
        let native_id = self
            .control_window_ids
            .iter()
            .find_map(|(native_id, control_id)| (*control_id == window_id).then_some(*native_id))
            .ok_or_else(|| format!("GUI window {window_id} does not exist"))?;
        let target = self
            .windows
            .get(&native_id)
            .filter(|window| !window.should_exit)
            .ok_or_else(|| format!("GUI window {window_id} is closing"))?;
        let target_tabs = target
            .tabs
            .as_ref()
            .map_or(0, crate::tabs::TabManager::count);
        if target_tabs >= crate::control::protocol::MAX_TABS_PER_WINDOW {
            return Err(format!(
                "GUI window tab limit of {} reached",
                crate::control::protocol::MAX_TABS_PER_WINDOW
            ));
        }
        let total_tabs = self
            .windows
            .values()
            .map(|window| {
                window
                    .tabs
                    .as_ref()
                    .map_or(0, crate::tabs::TabManager::count)
            })
            .sum::<usize>();
        if total_tabs >= crate::control::protocol::MAX_TOTAL_TABS {
            return Err(format!(
                "GUI process tab limit of {} reached",
                crate::control::protocol::MAX_TOTAL_TABS
            ));
        }
        let tab_id = self
            .windows
            .get_mut(&native_id)
            .expect("validated control window disappeared")
            .create_control_tab()?;
        Ok(crate::control::protocol::Response::TabCreated {
            process_id: std::process::id(),
            request_id: request_id.to_string(),
            window_id,
            tab_id,
        })
    }

    fn create_control_pane(
        &mut self,
        request_id: &str,
        window_id: u64,
        tab_id: u64,
        pane_id: u32,
        direction: crate::control::protocol::SplitDirection,
    ) -> std::result::Result<crate::control::protocol::Response, String> {
        let native_id = self
            .control_window_ids
            .iter()
            .find_map(|(native_id, control_id)| (*control_id == window_id).then_some(*native_id))
            .ok_or_else(|| format!("GUI window {window_id} does not exist"))?;
        self.windows
            .get(&native_id)
            .filter(|window| !window.should_exit)
            .ok_or_else(|| format!("GUI window {window_id} is closing"))?;
        let total_panes = self
            .windows
            .values()
            .filter_map(|window| window.tabs.as_ref())
            .flat_map(|tabs| tabs.all_tabs())
            .map(|tab| {
                tab.pane_manager
                    .as_ref()
                    .map_or(1, crate::panes::PaneManager::pane_count)
            })
            .sum::<usize>();
        if total_panes >= crate::control::protocol::MAX_TOTAL_PANES {
            return Err(format!(
                "GUI process pane limit of {} reached",
                crate::control::protocol::MAX_TOTAL_PANES
            ));
        }
        let direction = match direction {
            crate::control::protocol::SplitDirection::Horizontal => {
                crate::panes::SplitDirection::Horizontal
            }
            crate::control::protocol::SplitDirection::Vertical => {
                crate::panes::SplitDirection::Vertical
            }
        };
        let created_pane = self
            .windows
            .get_mut(&native_id)
            .expect("validated control window disappeared")
            .create_control_pane(tab_id, pane_id, direction)?;
        Ok(crate::control::protocol::Response::PaneCreated {
            process_id: std::process::id(),
            request_id: request_id.to_string(),
            window_id,
            tab_id,
            pane_id: created_pane,
        })
    }

    fn focus_control_window(
        &mut self,
        request_id: &str,
        window_id: u64,
    ) -> std::result::Result<crate::control::protocol::Response, String> {
        let native_id = self.control_native_window_id(window_id)?;
        let window = self
            .windows
            .get(&native_id)
            .filter(|window| !window.should_exit)
            .ok_or_else(|| format!("GUI window {window_id} is closing"))?;
        let native = window
            .window
            .as_ref()
            .ok_or_else(|| format!("GUI window {window_id} has no native window"))?;
        let already_focused = native.has_focus();
        if already_focused {
            return Ok(crate::control::protocol::Response::WindowFocusRequested {
                process_id: std::process::id(),
                request_id: request_id.to_string(),
                window_id,
                already_focused: true,
            });
        }
        let handle = native
            .window_handle()
            .map_err(|error| format!("Could not inspect GUI window {window_id}: {error}"))?;
        if matches!(handle.as_raw(), RawWindowHandle::Wayland(_)) {
            return Err(
                "The Wayland compositor controls native window focus; activate this window through the compositor"
                    .into(),
            );
        }
        if native.is_visible() == Some(false) {
            return Err(format!("GUI window {window_id} is not visible"));
        }
        if native.is_minimized() == Some(true) {
            return Err(format!("GUI window {window_id} is minimized"));
        }
        native.focus_window();
        Ok(crate::control::protocol::Response::WindowFocusRequested {
            process_id: std::process::id(),
            request_id: request_id.to_string(),
            window_id,
            already_focused: false,
        })
    }

    fn set_control_quick_access_visibility(
        &mut self,
        request_id: &str,
        window_id: u64,
        visible: bool,
    ) -> std::result::Result<crate::control::protocol::Response, String> {
        let native_id = self.control_native_window_id(window_id)?;
        let window = self
            .windows
            .get_mut(&native_id)
            .filter(|window| !window.should_exit)
            .ok_or_else(|| format!("GUI window {window_id} is closing"))?;
        if !window.launch.quick_access {
            return Err(format!(
                "GUI window {window_id} is not a quick-access terminal"
            ));
        }
        let native = window
            .window
            .as_ref()
            .ok_or_else(|| format!("GUI window {window_id} has no native window"))?;
        window.quick_access_visible = visible;
        native.set_visible(visible);
        window.window_occluded = !visible;
        if visible {
            native.set_minimized(false);
            native.focus_window();
            native.request_user_attention(Some(winit::window::UserAttentionType::Critical));
            native.request_redraw();
            window.redraw_scheduler.request(RedrawReason::WindowEvent);
        }
        Ok(crate::control::protocol::Response::QuickAccessChanged {
            process_id: std::process::id(),
            request_id: request_id.to_string(),
            window_id,
            visible,
        })
    }

    fn select_control_tab(
        &mut self,
        request_id: &str,
        window_id: u64,
        tab_id: u64,
    ) -> std::result::Result<crate::control::protocol::Response, String> {
        let native_id = self.control_native_window_id(window_id)?;
        let changed = self
            .windows
            .get_mut(&native_id)
            .filter(|window| !window.should_exit)
            .ok_or_else(|| format!("GUI window {window_id} is closing"))?
            .select_control_tab(tab_id)?;
        Ok(crate::control::protocol::Response::TabSelected {
            process_id: std::process::id(),
            request_id: request_id.to_string(),
            window_id,
            tab_id,
            changed,
        })
    }

    fn focus_control_pane(
        &mut self,
        request_id: &str,
        window_id: u64,
        tab_id: u64,
        pane_id: u32,
    ) -> std::result::Result<crate::control::protocol::Response, String> {
        let native_id = self.control_native_window_id(window_id)?;
        let (tab_changed, pane_changed) = self
            .windows
            .get_mut(&native_id)
            .filter(|window| !window.should_exit)
            .ok_or_else(|| format!("GUI window {window_id} is closing"))?
            .focus_control_pane(tab_id, pane_id)?;
        Ok(crate::control::protocol::Response::PaneFocused {
            process_id: std::process::id(),
            request_id: request_id.to_string(),
            window_id,
            tab_id,
            pane_id,
            tab_changed,
            pane_changed,
        })
    }

    fn close_control_window(
        &mut self,
        request_id: &str,
        window_id: u64,
    ) -> std::result::Result<crate::control::protocol::Response, String> {
        let native_id = self.control_native_window_id(window_id)?;
        let window = self
            .windows
            .get_mut(&native_id)
            .filter(|window| !window.should_exit)
            .ok_or_else(|| format!("GUI window {window_id} is already closing"))?;
        window.should_exit = true;
        Ok(crate::control::protocol::Response::WindowCloseRequested {
            process_id: std::process::id(),
            request_id: request_id.to_string(),
            window_id,
        })
    }

    fn close_control_tab(
        &mut self,
        request_id: &str,
        window_id: u64,
        tab_id: u64,
    ) -> std::result::Result<crate::control::protocol::Response, String> {
        let native_id = self.control_native_window_id(window_id)?;
        self.windows
            .get_mut(&native_id)
            .filter(|window| !window.should_exit)
            .ok_or_else(|| format!("GUI window {window_id} is closing"))?
            .close_control_tab(tab_id)?;
        Ok(crate::control::protocol::Response::TabCloseRequested {
            process_id: std::process::id(),
            request_id: request_id.to_string(),
            window_id,
            tab_id,
        })
    }

    fn close_control_pane(
        &mut self,
        request_id: &str,
        window_id: u64,
        tab_id: u64,
        pane_id: u32,
    ) -> std::result::Result<crate::control::protocol::Response, String> {
        let native_id = self.control_native_window_id(window_id)?;
        let (remaining_panes, focused_pane_id) = self
            .windows
            .get_mut(&native_id)
            .filter(|window| !window.should_exit)
            .ok_or_else(|| format!("GUI window {window_id} is closing"))?
            .close_control_pane(tab_id, pane_id)?;
        Ok(crate::control::protocol::Response::PaneClosed {
            process_id: std::process::id(),
            request_id: request_id.to_string(),
            window_id,
            tab_id,
            pane_id,
            remaining_panes,
            focused_pane_id,
        })
    }

    fn move_control_tab(
        &mut self,
        request_id: &str,
        source_window_id: u64,
        tab_id: u64,
        destination_window_id: u64,
        destination_index: u16,
    ) -> std::result::Result<crate::control::protocol::Response, String> {
        let source_native = self.control_native_window_id(source_window_id)?;
        let destination_native = self.control_native_window_id(destination_window_id)?;
        if source_native == destination_native {
            let changed = self
                .windows
                .get_mut(&source_native)
                .filter(|window| !window.should_exit)
                .ok_or_else(|| format!("GUI window {source_window_id} is closing"))?
                .move_control_tab_within_window(tab_id, destination_index)?;
            return Ok(crate::control::protocol::Response::TabMoved {
                process_id: std::process::id(),
                request_id: request_id.to_string(),
                source_window_id,
                source_tab_id: tab_id,
                destination_window_id,
                destination_tab_id: tab_id,
                destination_index,
                source_window_closing: false,
                changed,
            });
        }

        self.windows
            .get(&destination_native)
            .filter(|window| !window.should_exit)
            .ok_or_else(|| format!("GUI window {destination_window_id} is closing"))?;
        let mut destination = self
            .windows
            .remove(&destination_native)
            .expect("validated destination window remains present");
        let result = self
            .windows
            .get_mut(&source_native)
            .filter(|window| !window.should_exit)
            .ok_or_else(|| format!("GUI window {source_window_id} is closing"))
            .and_then(|source| {
                Self::transfer_control_tab(
                    source,
                    &mut destination,
                    tab_id,
                    usize::from(destination_index),
                )
            });
        self.windows.insert(destination_native, destination);
        let (destination_tab_id, source_window_closing) = result?;
        Ok(crate::control::protocol::Response::TabMoved {
            process_id: std::process::id(),
            request_id: request_id.to_string(),
            source_window_id,
            source_tab_id: tab_id,
            destination_window_id,
            destination_tab_id: u64::try_from(destination_tab_id)
                .map_err(|_| "Destination tab ID does not fit the control protocol".to_string())?,
            destination_index,
            source_window_closing,
            changed: true,
        })
    }

    fn transfer_control_tab(
        source: &mut App,
        destination: &mut App,
        tab_id: u64,
        destination_index: usize,
    ) -> std::result::Result<(usize, bool), String> {
        let tab_id = usize::try_from(tab_id)
            .map_err(|_| "Source tab ID does not fit this platform".to_string())?;
        let source_tabs = source
            .tabs
            .as_ref()
            .ok_or_else(|| "Source window has no initialized tab manager".to_string())?;
        let pinned = source_tabs
            .validate_window_tab_transfer_source_by_id(tab_id)
            .map_err(|error| format!("Could not move tab {tab_id}: {error:#}"))?;
        destination
            .tabs
            .as_ref()
            .ok_or_else(|| "Destination window has no initialized tab manager".to_string())?
            .validate_window_tab_transfer_destination(pinned, destination_index)
            .map_err(|error| format!("Could not move tab {tab_id}: {error:#}"))?;

        if source
            .tab_rename
            .as_ref()
            .is_some_and(|rename| rename.tab_id == tab_id)
            && let Some(rename) = source.tab_rename.take()
            && let Some(tab) = source
                .tabs
                .as_mut()
                .and_then(|tabs| tabs.tab_mut_by_id(tab_id))
        {
            tab.set_user_title(rename.original);
        }
        let tab = source
            .tabs
            .as_mut()
            .expect("validated source tab manager remains present")
            .detach_tab_for_window_transfer(tab_id)
            .expect("prevalidated source tab remains detachable");
        let destination_tab_id = destination
            .tabs
            .as_mut()
            .expect("validated destination tab manager remains present")
            .attach_window_transferred_tab_at(tab, destination_index)
            .expect("prevalidated destination remains attachable");

        source.broadcast = super::BroadcastState::Inactive;
        destination.broadcast = super::BroadcastState::Inactive;
        source.leave_copy_mode(false);
        destination.leave_copy_mode(false);
        source.terminal_mouse_button = None;
        destination.terminal_mouse_button = None;
        source.tab_dragging = None;
        destination.tab_dragging = None;
        source.refresh_focused_terminal_surfaces();
        destination.refresh_focused_terminal_surfaces();
        source.should_exit = source
            .tabs
            .as_ref()
            .is_none_or(crate::tabs::TabManager::is_empty);
        if let Some(window) = &source.window {
            window.request_redraw();
        }
        if let Some(window) = &destination.window {
            window.request_redraw();
        }
        Ok((destination_tab_id, source.should_exit))
    }

    #[allow(clippy::too_many_arguments)]
    fn move_control_pane(
        &mut self,
        request_id: &str,
        source_window_id: u64,
        source_tab_id: u64,
        pane_id: u32,
        destination_window_id: u64,
        destination_tab_id: u64,
        target_pane_id: u32,
        direction: crate::control::protocol::SplitDirection,
    ) -> std::result::Result<crate::control::protocol::Response, String> {
        let source_native = self.control_native_window_id(source_window_id)?;
        let destination_native = self.control_native_window_id(destination_window_id)?;
        let direction = match direction {
            crate::control::protocol::SplitDirection::Horizontal => {
                crate::panes::SplitDirection::Horizontal
            }
            crate::control::protocol::SplitDirection::Vertical => {
                crate::panes::SplitDirection::Vertical
            }
        };
        let destination_pane_id = if source_native == destination_native {
            self.windows
                .get_mut(&source_native)
                .filter(|window| !window.should_exit)
                .ok_or_else(|| format!("GUI window {source_window_id} is closing"))?
                .move_control_pane_within_window(
                    source_tab_id,
                    pane_id,
                    destination_tab_id,
                    target_pane_id,
                    direction,
                )?
        } else {
            self.windows
                .get(&destination_native)
                .filter(|window| !window.should_exit)
                .ok_or_else(|| format!("GUI window {destination_window_id} is closing"))?;
            let mut destination = self
                .windows
                .remove(&destination_native)
                .expect("validated destination window remains present");
            let result = self
                .windows
                .get_mut(&source_native)
                .filter(|window| !window.should_exit)
                .ok_or_else(|| format!("GUI window {source_window_id} is closing"))
                .and_then(|source| {
                    Self::transfer_control_pane(
                        source,
                        &mut destination,
                        source_tab_id,
                        pane_id,
                        destination_tab_id,
                        target_pane_id,
                        direction,
                    )
                });
            self.windows.insert(destination_native, destination);
            result?
        };
        Ok(crate::control::protocol::Response::PaneMoved {
            process_id: std::process::id(),
            request_id: request_id.to_string(),
            source_window_id,
            source_tab_id,
            source_pane_id: pane_id,
            destination_window_id,
            destination_tab_id,
            destination_pane_id,
            target_pane_id,
        })
    }

    #[allow(clippy::too_many_arguments)]
    fn transfer_control_pane(
        source: &mut App,
        destination: &mut App,
        source_tab_id: u64,
        pane_id: u32,
        destination_tab_id: u64,
        target_pane_id: u32,
        direction: crate::panes::SplitDirection,
    ) -> std::result::Result<u32, String> {
        let source_tab_id = usize::try_from(source_tab_id)
            .map_err(|_| "Source tab ID does not fit this platform".to_string())?;
        let destination_tab_id = usize::try_from(destination_tab_id)
            .map_err(|_| "Destination tab ID does not fit this platform".to_string())?;
        let pane_id = crate::panes::PaneId(pane_id);
        let target_pane_id = crate::panes::PaneId(target_pane_id);
        source
            .tabs
            .as_ref()
            .ok_or_else(|| "Source window has no initialized tab manager".to_string())?
            .validate_pane_transfer_source_by_id(source_tab_id, pane_id)
            .map_err(|error| format!("Could not move pane {}: {error:#}", pane_id.0))?;
        destination
            .tabs
            .as_ref()
            .ok_or_else(|| "Destination window has no initialized tab manager".to_string())?
            .validate_pane_transfer_destination_by_id(destination_tab_id, target_pane_id)
            .map_err(|error| format!("Could not move pane {}: {error:#}", pane_id.0))?;
        destination
            .tabs
            .as_mut()
            .expect("validated destination tab manager remains present")
            .prepare_pane_transfer_destination_by_id(
                destination_tab_id,
                target_pane_id,
                destination.config.panes.cwd,
            )
            .map_err(|error| format!("Could not prepare destination pane: {error:#}"))?;
        let pane = source
            .tabs
            .as_mut()
            .expect("validated source tab manager remains present")
            .detach_pane_for_transfer_by_id(source_tab_id, pane_id)
            .expect("prevalidated source pane remains detachable");
        let destination_pane_id = destination
            .tabs
            .as_mut()
            .expect("prepared destination tab manager remains present")
            .attach_transferred_pane_by_id(destination_tab_id, target_pane_id, pane, direction)
            .expect("prevalidated destination remains attachable");

        source.broadcast = super::BroadcastState::Inactive;
        destination.broadcast = super::BroadcastState::Inactive;
        source.leave_copy_mode(false);
        destination.leave_copy_mode(false);
        source.terminal_mouse_button = None;
        destination.terminal_mouse_button = None;
        source.refresh_focused_terminal_surfaces();
        destination.refresh_focused_terminal_surfaces();
        if let Some(window) = &source.window {
            window.request_redraw();
        }
        if let Some(window) = &destination.window {
            window.request_redraw();
        }
        Ok(destination_pane_id.0)
    }

    fn resize_control_window(
        &mut self,
        request_id: &str,
        window_id: u64,
        width: u32,
        height: u32,
    ) -> std::result::Result<crate::control::protocol::Response, String> {
        let native_id = self.control_native_window_id(window_id)?;
        let window = self
            .windows
            .get(&native_id)
            .filter(|window| !window.should_exit)
            .ok_or_else(|| format!("GUI window {window_id} is closing"))?;
        let native = window
            .window
            .as_ref()
            .ok_or_else(|| format!("GUI window {window_id} has no native window"))?;
        let immediate = native.request_inner_size(winit::dpi::PhysicalSize::new(width, height));
        Ok(crate::control::protocol::Response::WindowResizeRequested {
            process_id: std::process::id(),
            request_id: request_id.to_string(),
            window_id,
            requested_width: width,
            requested_height: height,
            immediate_width: immediate.map(|size| size.width),
            immediate_height: immediate.map(|size| size.height),
        })
    }

    fn resize_control_divider(
        &mut self,
        request_id: &str,
        window_id: u64,
        tab_id: u64,
        divider_id: u64,
        ratio_millis: u16,
    ) -> std::result::Result<crate::control::protocol::Response, String> {
        let native_id = self.control_native_window_id(window_id)?;
        let (previous_ratio_millis, changed) = self
            .windows
            .get_mut(&native_id)
            .filter(|window| !window.should_exit)
            .ok_or_else(|| format!("GUI window {window_id} is closing"))?
            .resize_control_divider(tab_id, divider_id, ratio_millis)?;
        Ok(crate::control::protocol::Response::DividerResized {
            process_id: std::process::id(),
            request_id: request_id.to_string(),
            window_id,
            tab_id,
            divider_id,
            previous_ratio_millis,
            ratio_millis,
            changed,
        })
    }

    fn read_control_text(
        &self,
        window_id: u64,
        tab_id: u64,
        pane_id: u32,
        history_lines: u16,
        max_bytes: u32,
    ) -> std::result::Result<crate::control::protocol::Response, String> {
        let native_id = self.control_native_window_id(window_id)?;
        let (screen, history_lines, text, truncated) = self
            .windows
            .get(&native_id)
            .filter(|window| !window.should_exit)
            .ok_or_else(|| format!("GUI window {window_id} is closing"))?
            .read_control_text(tab_id, pane_id, history_lines, max_bytes)?;
        Ok(crate::control::protocol::Response::Text {
            process_id: std::process::id(),
            window_id,
            tab_id,
            pane_id,
            screen,
            history_lines,
            text,
            truncated,
        })
    }

    #[allow(clippy::too_many_arguments)]
    fn send_control_text(
        &mut self,
        request_id: &str,
        window_id: u64,
        tab_id: u64,
        pane_id: u32,
        text: &str,
        paste: bool,
        enter: bool,
    ) -> std::result::Result<crate::control::protocol::Response, String> {
        let native_id = self.control_native_window_id(window_id)?;
        let (input_bytes, wire_bytes, disposition, confirmation_id) = self
            .windows
            .get_mut(&native_id)
            .filter(|window| !window.should_exit)
            .ok_or_else(|| format!("GUI window {window_id} is closing"))?
            .send_control_text(tab_id, pane_id, text, paste, enter)?;
        Ok(crate::control::protocol::Response::InputAccepted {
            process_id: std::process::id(),
            request_id: request_id.to_string(),
            window_id,
            tab_id,
            pane_id,
            input_bytes,
            wire_bytes,
            paste,
            enter,
            disposition,
            confirmation_id,
        })
    }

    fn control_native_window_id(&self, window_id: u64) -> std::result::Result<WindowId, String> {
        self.control_window_ids
            .iter()
            .find_map(|(native_id, control_id)| (*control_id == window_id).then_some(*native_id))
            .ok_or_else(|| format!("GUI window {window_id} does not exist"))
    }

    fn remove_closed_windows(&mut self, event_loop: &ActiveEventLoop) {
        let surviving = self
            .windows
            .values()
            .filter(|window| !window.should_exit)
            .count();
        if surviving > 0 {
            for window in self
                .windows
                .values_mut()
                .filter(|window| window.should_exit)
            {
                window.retain_workspace_on_exit = false;
                if let Some(id) = window.current_workspace_id.clone()
                    && let Err(error) = window.app_state.forget_workspace(&id)
                {
                    log::warn!(
                        "Could not remove closed window workspace from startup state: {error:#}"
                    );
                }
            }
        }
        self.windows.retain(|_, window| !window.should_exit);
        self.window_order.retain(|id| self.windows.contains_key(id));
        self.control_window_ids
            .retain(|id, _| self.windows.contains_key(id));
        if self.windows.is_empty() && self.pending.is_empty() {
            event_loop.exit();
        }
    }

    fn next_live_window(&self, source: WindowId) -> Option<WindowId> {
        let source_index = self.window_order.iter().position(|id| *id == source)?;
        (1..self.window_order.len())
            .map(|offset| self.window_order[(source_index + offset) % self.window_order.len()])
            .find(|candidate| {
                self.windows
                    .get(candidate)
                    .is_some_and(|window| !window.should_exit)
            })
    }

    fn transfer_focused_pane(
        source: &mut App,
        destination: &mut App,
    ) -> Result<crate::panes::PaneId> {
        source
            .tabs
            .as_ref()
            .expect("source tabs were inspected")
            .validate_window_pane_transfer_source()?;
        destination
            .tabs
            .as_mut()
            .expect("destination tabs were inspected")
            .prepare_window_pane_transfer_destination(destination.config.panes.cwd)?;

        let pane = source
            .tabs
            .as_mut()
            .expect("source tabs were validated")
            .detach_focused_pane_for_window_transfer()?;
        let id = destination
            .tabs
            .as_mut()
            .expect("destination tabs were prepared")
            .attach_window_transferred_pane(pane);

        source.broadcast = super::BroadcastState::Inactive;
        destination.broadcast = super::BroadcastState::Inactive;
        source.leave_copy_mode(false);
        destination.leave_copy_mode(false);
        source.terminal_mouse_button = None;
        destination.terminal_mouse_button = None;
        source.refresh_focused_terminal_surfaces();
        destination.refresh_focused_terminal_surfaces();
        source.toasts.success("Pane moved to the next window");
        destination.toasts.info("Pane received from another window");
        if let Some(window) = &source.window {
            window.request_redraw();
        }
        if let Some(window) = &destination.window {
            window.request_redraw();
        }
        Ok(id)
    }

    fn fulfill_pane_window_move(&mut self, source_id: WindowId) {
        let Some(destination_id) = self.next_live_window(source_id) else {
            if let Some(source) = self.windows.get_mut(&source_id) {
                source
                    .toasts
                    .warning("Pane was not moved: A second window is required");
            }
            return;
        };
        let Some(mut destination) = self.windows.remove(&destination_id) else {
            return;
        };
        let result = self
            .windows
            .get_mut(&source_id)
            .ok_or_else(|| anyhow::anyhow!("The source window closed before the move"))
            .and_then(|source| Self::transfer_focused_pane(source, &mut destination));
        if let Err(error) = result
            && let Some(source) = self.windows.get_mut(&source_id)
        {
            source
                .toasts
                .warning(format!("Pane was not moved: {error:#}"));
        }
        self.windows.insert(destination_id, destination);
    }

    fn transfer_active_tab(source: &mut App, destination: &mut App) -> Result<usize> {
        source
            .tabs
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("No source workspace is active"))?
            .validate_window_tab_transfer_source()?;
        if destination.tabs.is_none() {
            anyhow::bail!("No destination workspace is active");
        }
        if let Some(rename) = source.tab_rename.take()
            && let Some(tab) = source.tabs.as_mut().and_then(|tabs| {
                tabs.all_tabs_mut()
                    .iter_mut()
                    .find(|tab| tab.id == rename.tab_id)
            })
        {
            tab.set_user_title(rename.original);
        }
        let tab = source
            .tabs
            .as_mut()
            .expect("source tabs were validated")
            .detach_active_tab_for_window_transfer()?;
        let id = destination
            .tabs
            .as_mut()
            .expect("destination tabs were validated")
            .attach_window_transferred_tab(tab);

        source.broadcast = super::BroadcastState::Inactive;
        destination.broadcast = super::BroadcastState::Inactive;
        source.leave_copy_mode(false);
        destination.leave_copy_mode(false);
        source.terminal_mouse_button = None;
        destination.terminal_mouse_button = None;
        source.tab_dragging = None;
        destination.tab_dragging = None;
        source.refresh_focused_terminal_surfaces();
        destination.refresh_focused_terminal_surfaces();
        source.should_exit = source
            .tabs
            .as_ref()
            .is_none_or(crate::tabs::TabManager::is_empty);
        if !source.should_exit {
            source.toasts.success("Tab moved to the next window");
        }
        destination.toasts.info("Tab received from another window");
        if let Some(window) = &source.window {
            window.request_redraw();
        }
        if let Some(window) = &destination.window {
            window.request_redraw();
        }
        Ok(id)
    }

    fn fulfill_tab_window_move(&mut self, source_id: WindowId) {
        let Some(destination_id) = self.next_live_window(source_id) else {
            if let Some(source) = self.windows.get_mut(&source_id) {
                source
                    .toasts
                    .warning("Tab was not moved: A second window is required");
            }
            return;
        };
        let Some(mut destination) = self.windows.remove(&destination_id) else {
            return;
        };
        let result = self
            .windows
            .get_mut(&source_id)
            .ok_or_else(|| anyhow::anyhow!("The source window closed before the move"))
            .and_then(|source| Self::transfer_active_tab(source, &mut destination));
        if let Err(error) = result
            && let Some(source) = self.windows.get_mut(&source_id)
        {
            source
                .toasts
                .warning(format!("Tab was not moved: {error:#}"));
        }
        self.windows.insert(destination_id, destination);
    }
}

fn build_plugin_process_inventory(
    windows: &[(u64, &App)],
) -> crate::control::protocol::ProcessInventory {
    let live_window_count = windows
        .iter()
        .filter(|(_, window)| !window.should_exit)
        .count();
    let mut truncated = live_window_count > super::PLUGIN_PROCESS_MAX_WINDOWS;
    let mut budget = super::plugin_process_inventory_budget();
    let mut inventories =
        Vec::with_capacity(live_window_count.min(super::PLUGIN_PROCESS_MAX_WINDOWS));
    for (control_id, window) in windows
        .iter()
        .filter(|(_, window)| !window.should_exit)
        .take(super::PLUGIN_PROCESS_MAX_WINDOWS)
    {
        let inventory = window.control_inventory(*control_id, &mut budget);
        truncated |= inventory.truncated;
        inventories.push(inventory);
    }
    truncated |= inventories.len() < live_window_count.min(super::PLUGIN_PROCESS_MAX_WINDOWS);
    crate::control::protocol::ProcessInventory {
        process_id: std::process::id(),
        windows: inventories,
        truncated,
    }
}

fn validate_cli_launches(config: &Config, launch: &LaunchPlan) -> Result<()> {
    let quick_access_windows = launch
        .windows
        .iter()
        .filter(|window| window.quick_access)
        .count();
    if quick_access_windows > 0
        && (quick_access_windows != 1 || launch.windows.len() != 1 || !launch.windows[0].force_new)
    {
        anyhow::bail!("quick-access launch must be one explicit fresh window");
    }
    for window in &launch.windows {
        for tab in &window.tabs {
            if let Some(cwd) = &tab.working_directory {
                if !cwd.is_dir() {
                    anyhow::bail!("working directory {} does not exist", cwd.display());
                }
                cwd.to_str()
                    .ok_or_else(|| anyhow::anyhow!("working directory must be valid UTF-8"))?;
            }
            let crate::cli::LaunchTarget::TerminalProfile { name } = &tab.target else {
                continue;
            };
            let profile = config.launch_profile(name)?;
            if profile.serial_config().is_some() {
                #[cfg(not(target_os = "linux"))]
                anyhow::bail!("serial terminal profiles are currently supported only on Linux");
                if tab.working_directory.is_some() {
                    anyhow::bail!(
                        "serial terminal profile {name:?} cannot use --working-directory"
                    );
                }
                if !tab.splits.is_empty() {
                    anyhow::bail!("serial terminal profile {name:?} cannot be split into panes");
                }
                continue;
            }
            if tab.working_directory.is_none()
                && let Some(cwd) = &profile.cwd
            {
                if !cwd.is_dir() {
                    anyhow::bail!(
                        "terminal profile {name:?} working directory {} does not exist",
                        cwd.display()
                    );
                }
                cwd.to_str().ok_or_else(|| {
                    anyhow::anyhow!(
                        "terminal profile {name:?} working directory must be valid UTF-8"
                    )
                })?;
            }
        }
    }
    Ok(())
}

fn expand_auto_restore_plan(
    config: &Config,
    state: &super::state::AppState,
    launch: LaunchPlan,
) -> LaunchPlan {
    if launch != LaunchPlan::default() || !config.session.enabled || !config.session.auto_restore {
        return launch;
    }
    let ids = state.restore_workspace_ids();
    if ids.is_empty() {
        return launch;
    }
    LaunchPlan {
        windows: ids
            .into_iter()
            .map(|id| WindowLaunch {
                workspace: Some(id),
                ..WindowLaunch::default()
            })
            .collect(),
    }
}

impl ApplicationHandler<AppEvent> for Application {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if !self.system_accessibility_monitor_started {
            self.system_accessibility_monitor_started = true;
            if let Some(proxy) = super::EVENT_LOOP_PROXY.get().cloned() {
                super::system_accessibility::start_monitor(proxy);
            }
        }
        let _ = self.create_pending_windows(event_loop);
        self.remove_closed_windows(event_loop);
    }

    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        window_id: WindowId,
        event: WindowEvent,
    ) {
        let mut create_window = false;
        let mut requested_window_launch = None;
        let mut move_pane_to_next_window = false;
        let mut move_tab_to_next_window = false;
        if let Some(window) = self.windows.get_mut(&window_id) {
            ApplicationHandler::window_event(window, event_loop, window_id, event);
            create_window = std::mem::take(&mut window.request_new_window);
            requested_window_launch = window.requested_window_launch.take();
            move_pane_to_next_window = std::mem::take(&mut window.request_move_pane_to_next_window);
            move_tab_to_next_window = std::mem::take(&mut window.request_move_tab_to_next_window);
        }
        if move_pane_to_next_window {
            self.fulfill_pane_window_move(window_id);
        }
        if move_tab_to_next_window {
            self.fulfill_tab_window_move(window_id);
        }
        if let Some(launch) = requested_window_launch {
            let result = self
                .windows
                .get(&window_id)
                .ok_or_else(|| anyhow::anyhow!("source window closed during domain attachment"))
                .map(|source| source.config.clone())
                .and_then(|config| self.queue_window(config, launch));
            if let Err(error) = result {
                log::error!("Could not create requested window: {error:#}");
            }
            let _ = self.create_pending_windows(event_loop);
        }
        if create_window {
            let result = self
                .windows
                .get(&window_id)
                .ok_or_else(|| anyhow::anyhow!("source window closed during new-window request"))
                .map(|source| {
                    (
                        source.config.clone(),
                        source
                            .tabs
                            .as_ref()
                            .and_then(|tabs| tabs.active_tab())
                            .and_then(|tab| tab.working_directory()),
                    )
                })
                .and_then(|(config, cwd)| self.queue_default_window(config, cwd));
            if let Err(error) = result {
                log::error!("Could not create a new window: {error:#}");
            }
            let _ = self.create_pending_windows(event_loop);
        }
        self.remove_closed_windows(event_loop);
    }

    fn user_event(&mut self, event_loop: &ActiveEventLoop, event: AppEvent) {
        let process_pty = match event {
            AppEvent::SystemAccessibilityChanged(preferences) => {
                self.system_accessibility = preferences;
                for window in self.windows.values_mut() {
                    window.set_system_accessibility_preferences(preferences);
                }
                for window in &mut self.pending {
                    window.set_system_accessibility_preferences(preferences);
                }
                return;
            }
            AppEvent::SystemAccessibilityUnavailable(error) => {
                log::debug!("Desktop accessibility preferences are unavailable: {error}");
                return;
            }
            AppEvent::ControlRequest {
                request,
                respond_to,
            } => {
                let Some(respond_to) = self.defer_live_domain_control(&request, respond_to) else {
                    return;
                };
                let result = self.handle_control_request(event_loop, request);
                let _ = respond_to.send(result);
                return;
            }
            AppEvent::AccessKit(event) => {
                if let Some(window) = self.windows.get_mut(&event.window_id) {
                    window.handle_accesskit_event(event);
                }
                self.remove_closed_windows(event_loop);
                return;
            }
            AppEvent::RendererDeviceLost {
                window_id,
                generation,
                failure,
            } => {
                if let Some(window) = self.windows.get_mut(&window_id) {
                    window.handle_renderer_device_lost(generation, failure);
                }
                self.remove_closed_windows(event_loop);
                return;
            }
            AppEvent::BackgroundImageLoaded {
                window_id,
                generation,
                path,
                result,
            } => {
                if let Some(window) = self.windows.get_mut(&window_id) {
                    window.handle_background_image_loaded(generation, path, result);
                }
                self.remove_closed_windows(event_loop);
                return;
            }
            AppEvent::LiveDomainControlCompleted {
                window_id,
                control,
                result,
            } => {
                if let Some(window) = self.windows.get_mut(&window_id) {
                    window.handle_live_domain_control_completed(control.clone(), result.clone());
                }
                self.complete_live_domain_control(window_id, &control, &result);
                self.remove_closed_windows(event_loop);
                return;
            }
            AppEvent::LiveDomainTopology {
                window_id,
                domain_id,
                result,
            } => {
                if let Some(window) = self.windows.get_mut(&window_id) {
                    window.handle_live_domain_topology(domain_id, result);
                }
                self.remove_closed_windows(event_loop);
                return;
            }
            AppEvent::DomainPickerLoaded {
                window_id,
                generation,
                result,
            } => {
                if let Some(window) = self.windows.get_mut(&window_id) {
                    window.handle_domain_picker_loaded(generation, result);
                }
                self.remove_closed_windows(event_loop);
                return;
            }
            AppEvent::DomainPickerPrepared {
                window_id,
                generation,
                result,
            } => {
                let requested = if let Some(window) = self.windows.get_mut(&window_id) {
                    window.handle_domain_picker_prepared(generation, result);
                    window
                        .requested_window_launch
                        .take()
                        .map(|launch| (window.config.clone(), launch))
                } else {
                    None
                };
                if let Some((config, launch)) = requested {
                    if let Err(error) = self.queue_window(config, launch) {
                        log::error!("Could not create requested domain window: {error:#}");
                    }
                    let _ = self.create_pending_windows(event_loop);
                }
                self.remove_closed_windows(event_loop);
                return;
            }
            AppEvent::ContextGathered {
                window_id,
                request_id,
                result,
            } => {
                if let Some(window) = self.windows.get_mut(&window_id) {
                    window.handle_context_gathered(request_id, result);
                }
                self.remove_closed_windows(event_loop);
                return;
            }
            AppEvent::FileUploadCompleted {
                window_id,
                request_id,
                result,
            } => {
                if let Some(window) = self.windows.get_mut(&window_id) {
                    window.handle_file_upload_completed(request_id, result);
                }
                self.remove_closed_windows(event_loop);
                return;
            }
            AppEvent::FileUploadProgress {
                window_id,
                request_id,
                bytes_sent,
                total_bytes,
            } => {
                if let Some(window) = self.windows.get_mut(&window_id) {
                    window.handle_file_upload_progress(request_id, bytes_sent, total_bytes);
                }
                self.remove_closed_windows(event_loop);
                return;
            }
            AppEvent::StructuredPluginCompleted(completion) => {
                self.handle_structured_plugin_completed(completion);
                self.remove_closed_windows(event_loop);
                return;
            }
            AppEvent::Pty => {
                clear_pty_wake_pending();
                true
            }
            AppEvent::Transcript => {
                clear_transcript_wake_pending();
                true
            }
            AppEvent::Git => false,
        };
        for window in self.windows.values_mut() {
            if process_pty && window.process_pty_events() {
                window
                    .redraw_scheduler
                    .request(RedrawReason::TerminalOutput);
            }
        }
        self.remove_closed_windows(event_loop);
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        let now = Instant::now();
        let mut deadline = None;
        for window in self.windows.values_mut() {
            window.poll_idle_sources(now);
            if window.process_pty_events() {
                window
                    .redraw_scheduler
                    .request(RedrawReason::TerminalOutput);
            }
            if window.redraw_scheduler.has_pending()
                && !window.surface_lost
                && !window.window_occluded
                && let Some(native) = &window.window
            {
                native.request_redraw();
            }
            if let Some(candidate) = window.next_idle_deadline(now) {
                deadline =
                    Some(deadline.map_or(candidate, |current: Instant| current.min(candidate)));
            }
        }
        self.remove_closed_windows(event_loop);
        if self.windows.is_empty() {
            return;
        }
        if let Some(deadline) = deadline {
            event_loop.set_control_flow(ControlFlow::WaitUntil(deadline));
        } else {
            event_loop.set_control_flow(ControlFlow::Wait);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quick_access_launch_is_excluded_from_automatic_workspace_persistence() {
        let launch = LaunchPlan {
            windows: vec![WindowLaunch {
                force_new: true,
                quick_access: true,
                ..WindowLaunch::default()
            }],
        };
        let application = Application::from_config(Config::default(), None, launch).unwrap();
        assert_eq!(application.pending.len(), 1);
        assert!(!application.pending[0].allow_auto_restore);
        assert!(!application.pending[0].retain_workspace_on_exit);
        assert!(!application.pending[0].automatic_persistence);
    }

    #[test]
    fn serial_profiles_reject_process_only_cli_overrides_before_window_creation() {
        let mut config = Config::default();
        config.profiles.insert(
            "console".into(),
            crate::config::LaunchProfileConfig {
                serial: Some(crate::config::SerialProfileConfig {
                    device: std::path::PathBuf::from("/dev/ttyUSB0"),
                    ..crate::config::SerialProfileConfig::default()
                }),
                ..crate::config::LaunchProfileConfig::default()
            },
        );
        config.validate().unwrap();

        let mut launch = LaunchPlan::default();
        launch.windows[0].tabs[0].target = crate::cli::LaunchTarget::TerminalProfile {
            name: "console".into(),
        };
        launch.windows[0].tabs[0].working_directory = Some(std::env::temp_dir());
        assert!(validate_cli_launches(&config, &launch).is_err());

        launch.windows[0].tabs[0].working_directory = None;
        launch.windows[0].tabs[0]
            .splits
            .push(crate::cli::CliSplitDirection::Vertical);
        assert!(validate_cli_launches(&config, &launch).is_err());
    }

    #[test]
    fn control_replays_are_exact_collision_safe_and_bounded() {
        use crate::control::protocol::{Request, Response};

        let request = Request::CreateWindow {
            request_id: "0".repeat(32),
            source_window: 7,
        };
        let response = Ok(Response::WindowCreated {
            process_id: 42,
            request_id: "0".repeat(32),
            window_id: 9,
        });
        let mut cache = ControlReplayCache::default();
        cache.record(request.clone(), response.clone());
        assert_eq!(cache.replay(&request), Some(response));
        assert!(matches!(
            cache.replay(&Request::CreateWindow {
                request_id: "0".repeat(32),
                source_window: 8,
            }),
            Some(Err(error)) if error.contains("different action")
        ));
        assert!(matches!(
            cache.replay(&Request::CreateTab {
                request_id: "0".repeat(32),
                window_id: 7,
            }),
            Some(Err(error)) if error.contains("different action")
        ));

        let tab_request = Request::CreateTab {
            request_id: "e".repeat(32),
            window_id: 7,
        };
        let tab_response = Ok(Response::TabCreated {
            process_id: 42,
            request_id: "e".repeat(32),
            window_id: 7,
            tab_id: 3,
        });
        cache.record(tab_request.clone(), tab_response.clone());
        assert_eq!(cache.replay(&tab_request), Some(tab_response));

        let terminal_failure = Request::CreateTab {
            request_id: "1".repeat(32),
            window_id: 7,
        };
        cache.record_terminal(
            terminal_failure.clone(),
            Err("authoritative domain result was lost".into()),
        );
        assert_eq!(
            cache.replay(&terminal_failure),
            Some(Err("authoritative domain result was lost".into()))
        );

        let pane_request = Request::SplitPane {
            request_id: "d".repeat(32),
            window_id: 7,
            tab_id: 3,
            pane_id: 0,
            direction: crate::control::protocol::SplitDirection::Vertical,
        };
        let pane_response = Ok(Response::PaneCreated {
            process_id: 42,
            request_id: "d".repeat(32),
            window_id: 7,
            tab_id: 3,
            pane_id: 1,
        });
        cache.record(pane_request.clone(), pane_response.clone());
        assert_eq!(cache.replay(&pane_request), Some(pane_response));

        let close_request = Request::ClosePane {
            request_id: "c".repeat(32),
            window_id: 7,
            tab_id: 3,
            pane_id: 1,
        };
        let close_response = Ok(Response::PaneClosed {
            process_id: 42,
            request_id: "c".repeat(32),
            window_id: 7,
            tab_id: 3,
            pane_id: 1,
            remaining_panes: 1,
            focused_pane_id: 0,
        });
        cache.record(close_request.clone(), close_response.clone());
        assert_eq!(cache.replay(&close_request), Some(close_response));

        let move_request = Request::MoveTab {
            request_id: "b".repeat(32),
            source_window_id: 7,
            tab_id: 3,
            destination_window_id: 9,
            destination_index: 1,
        };
        let move_response = Ok(Response::TabMoved {
            process_id: 42,
            request_id: "b".repeat(32),
            source_window_id: 7,
            source_tab_id: 3,
            destination_window_id: 9,
            destination_tab_id: 4,
            destination_index: 1,
            source_window_closing: false,
            changed: true,
        });
        cache.record(move_request.clone(), move_response.clone());
        assert_eq!(cache.replay(&move_request), Some(move_response));

        let pane_move_request = Request::MovePane {
            request_id: "a".repeat(32),
            source_window_id: 7,
            source_tab_id: 3,
            pane_id: 1,
            destination_window_id: 9,
            destination_tab_id: 4,
            target_pane_id: 0,
            direction: crate::control::protocol::SplitDirection::Vertical,
        };
        let pane_move_response = Ok(Response::PaneMoved {
            process_id: 42,
            request_id: "a".repeat(32),
            source_window_id: 7,
            source_tab_id: 3,
            source_pane_id: 1,
            destination_window_id: 9,
            destination_tab_id: 4,
            destination_pane_id: 1,
            target_pane_id: 0,
        });
        cache.record(pane_move_request.clone(), pane_move_response.clone());
        assert_eq!(cache.replay(&pane_move_request), Some(pane_move_response));

        let resize_request = Request::ResizeDivider {
            request_id: "9".repeat(32),
            window_id: 7,
            tab_id: 3,
            divider_id: 11,
            ratio_millis: 650,
        };
        let resize_response = Ok(Response::DividerResized {
            process_id: 42,
            request_id: "9".repeat(32),
            window_id: 7,
            tab_id: 3,
            divider_id: 11,
            previous_ratio_millis: 500,
            ratio_millis: 650,
            changed: true,
        });
        cache.record(resize_request.clone(), resize_response.clone());
        assert_eq!(cache.replay(&resize_request), Some(resize_response));

        let failed = Request::CreateWindow {
            request_id: "f".repeat(32),
            source_window: 7,
        };
        cache.record(failed.clone(), Err("temporary failure".into()));
        assert!(cache.replay(&failed).is_none());

        let failed_input = Request::SendText {
            request_id: "8".repeat(32),
            window_id: 7,
            tab_id: 3,
            pane_id: 1,
            text: "bounded input".into(),
            paste: false,
            enter: true,
        };
        cache.record(failed_input.clone(), Err("PTY write may be partial".into()));
        assert!(matches!(
            cache.replay(&failed_input),
            Some(Err(error)) if error.contains("partial")
        ));

        for index in 1..=MAX_CONTROL_REPLAYS {
            let request_id = format!("{index:032x}");
            cache.record(
                Request::CreateWindow {
                    request_id: request_id.clone(),
                    source_window: 7,
                },
                Err(format!("failure {index}")),
            );
        }
        assert_eq!(cache.entries.len(), MAX_CONTROL_REPLAYS);
        assert!(cache.replay(&request).is_none());
    }

    #[test]
    fn control_errors_are_single_line_control_free_and_bounded() {
        let error = bounded_control_error(&format!("bad\n\u{1b}{}", "x".repeat(2_000)));
        assert!(error.len() <= 1_024);
        assert!(!error.chars().any(char::is_control));
    }

    #[test]
    fn control_window_resize_rejects_stale_or_not_yet_native_targets() {
        let mut application =
            Application::from_config(Config::default(), None, LaunchPlan::default()).unwrap();
        let native_id = WindowId::dummy();
        let window = application.pending.pop().unwrap();
        application.windows.insert(native_id, window);
        application.window_order.push(native_id);
        application.control_window_ids.insert(native_id, 7);

        let error = application
            .resize_control_window("88888888888888888888888888888888", 7, 900, 600)
            .unwrap_err();
        assert!(error.contains("no native window"), "{error}");
        let stale = application
            .resize_control_window("77777777777777777777777777777777", 8, 900, 600)
            .unwrap_err();
        assert!(stale.contains("does not exist"), "{stale}");
    }

    #[test]
    fn control_window_close_targets_one_stable_id_and_only_schedules_teardown() {
        let mut application =
            Application::from_config(Config::default(), None, LaunchPlan::default()).unwrap();
        let native_id = WindowId::dummy();
        let window = application.pending.pop().unwrap();
        application.windows.insert(native_id, window);
        application.window_order.push(native_id);
        application.control_window_ids.insert(native_id, 7);

        let response = application
            .close_control_window("9876543210abcdef9876543210abcdef", 7)
            .unwrap();
        assert!(matches!(
            response,
            crate::control::protocol::Response::WindowCloseRequested { window_id: 7, .. }
        ));
        assert!(application.windows[&native_id].should_exit);
        assert!(
            application
                .close_control_window("0".repeat(32).as_str(), 7)
                .is_err()
        );
        assert!(
            application
                .close_control_window("0".repeat(32).as_str(), 8)
                .is_err()
        );
    }

    #[cfg(unix)]
    #[test]
    fn live_domain_creation_completion_returns_authoritative_gui_ids_and_replays() {
        if !std::path::Path::new("/bin/true").is_file() {
            return;
        }
        use crate::domain::model::{
            DomainSnapshot, LivePane, LivePaneLayout, LiveTab, LiveWorkspace, SpawnSpec,
            SplitDirection,
        };
        use crate::tabs::TabManager;

        let pane = |id| LivePane {
            id,
            title: format!("pane-{id}"),
            working_directory: None,
            command: vec!["/bin/true".into()],
            cols: 80,
            rows: 24,
            closed: false,
            attachments: 0,
        };
        let tab = |id, pane_id| LiveTab {
            id,
            title: format!("tab-{id}"),
            user_title: None,
            pinned: false,
            focused_pane: pane_id,
            layout: LivePaneLayout::Pane { pane: pane_id },
            panes: vec![pane(pane_id)],
        };
        let snapshot = |active_tab, tabs| DomainSnapshot {
            id: "domain-control-ack".into(),
            name: "control ack".into(),
            created_at: "2026-01-01T00:00:00Z".into(),
            workspace: LiveWorkspace {
                id: "live-domain-control-ack".into(),
                name: "control ack".into(),
                active_tab,
                tabs,
            },
        };
        let initial = snapshot(10, vec![tab(10, 100)]);
        let relay = crate::domain::DomainRelayCommand::local("/bin/true");
        let mut tabs = TabManager::new(80, 24, Config::default().shell, 1_000, 1024 * 1024);
        tabs.attach_domain(&initial, &relay).unwrap();
        let mut window = App::new_headless(Config::default()).unwrap();
        window.tabs = Some(tabs);
        window.live_domain_relay_command = Some(relay);
        window.live_domain_topologies.insert(
            initial.id.clone(),
            crate::domain::model::DomainTopology::from(&initial),
        );

        let mut application =
            Application::from_config(Config::default(), None, LaunchPlan::default()).unwrap();
        application.pending.clear();
        let native_id = WindowId::dummy();
        application.windows.insert(native_id, window);
        application.window_order.push(native_id);
        application.control_window_ids.insert(native_id, 7);

        let create_request = crate::control::protocol::Request::CreateTab {
            request_id: "66666666666666666666666666666666".into(),
            window_id: 7,
        };
        let create_control = crate::app::LiveDomainControl::SpawnTab {
            domain_id: initial.id.clone(),
            spawn: SpawnSpec::default(),
        };
        let (create_tx, create_rx) = std::sync::mpsc::sync_channel(1);
        application
            .pending_domain_controls
            .push(PendingDomainControl {
                request: create_request.clone(),
                window_id: native_id,
                control: create_control.clone(),
                context: PendingDomainContext::None,
                responders: vec![create_tx],
            });
        let (create_retry_tx, create_retry_rx) = std::sync::mpsc::sync_channel(1);
        assert!(
            application
                .defer_live_domain_control(&create_request, create_retry_tx)
                .is_none()
        );
        let (collision_tx, collision_rx) = std::sync::mpsc::sync_channel(1);
        let collision = crate::control::protocol::Request::CreateTab {
            request_id: "66666666666666666666666666666666".into(),
            window_id: 8,
        };
        assert!(
            application
                .defer_live_domain_control(&collision, collision_tx)
                .is_none()
        );
        assert!(matches!(
            collision_rx.recv().unwrap(),
            Err(error) if error.contains("different action")
        ));
        let created = snapshot(20, vec![tab(10, 100), tab(20, 200)]);
        let created_result = Ok(created.clone());
        application
            .windows
            .get_mut(&native_id)
            .unwrap()
            .handle_live_domain_control_completed(create_control.clone(), created_result.clone());
        application.complete_live_domain_control(native_id, &create_control, &created_result);
        let create_response = create_rx.recv().unwrap().unwrap();
        let created_local_tab = match &create_response {
            crate::control::protocol::Response::TabCreated {
                window_id: 7,
                tab_id,
                ..
            } => *tab_id,
            response => panic!("unexpected create response {response:?}"),
        };
        assert_ne!(created_local_tab, 0);
        assert_eq!(create_retry_rx.recv().unwrap(), Ok(create_response.clone()));
        assert_eq!(
            application.control_replays.replay(&create_request),
            Some(Ok(create_response))
        );

        let split_request = crate::control::protocol::Request::SplitPane {
            request_id: "77777777777777777777777777777777".into(),
            window_id: 7,
            tab_id: created_local_tab,
            pane_id: 0,
            direction: crate::control::protocol::SplitDirection::Vertical,
        };
        let split_control = crate::app::LiveDomainControl::SplitPane {
            domain_id: created.id.clone(),
            remote_pane_id: 200,
            direction: SplitDirection::Vertical,
            spawn: SpawnSpec::default(),
        };
        let (first_tx, first_rx) = std::sync::mpsc::sync_channel(1);
        let (retry_tx, retry_rx) = std::sync::mpsc::sync_channel(1);
        application
            .pending_domain_controls
            .push(PendingDomainControl {
                request: split_request.clone(),
                window_id: native_id,
                control: split_control.clone(),
                context: PendingDomainContext::None,
                responders: vec![first_tx, retry_tx],
            });
        let mut split_tab = tab(20, 200);
        split_tab.focused_pane = 201;
        split_tab.panes.push(pane(201));
        split_tab.layout = LivePaneLayout::Split {
            split_id: 55,
            direction: SplitDirection::Vertical,
            ratio_millis: 500,
            first: Box::new(LivePaneLayout::Pane { pane: 200 }),
            second: Box::new(LivePaneLayout::Pane { pane: 201 }),
        };
        let split = snapshot(20, vec![tab(10, 100), split_tab]);
        let split_result = Ok(split.clone());
        application
            .windows
            .get_mut(&native_id)
            .unwrap()
            .handle_live_domain_control_completed(split_control.clone(), split_result.clone());
        application.complete_live_domain_control(native_id, &split_control, &split_result);
        let first = first_rx.recv().unwrap().unwrap();
        assert_eq!(retry_rx.recv().unwrap().unwrap(), first);
        assert!(matches!(
            first,
            crate::control::protocol::Response::PaneCreated {
                window_id: 7,
                tab_id,
                pane_id: 1,
                ..
            } if tab_id == created_local_tab
        ));

        let unproven_close_request = crate::control::protocol::Request::ClosePane {
            request_id: "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".into(),
            window_id: 7,
            tab_id: created_local_tab,
            pane_id: 1,
        };
        let unproven_close_control = crate::app::LiveDomainControl::ClosePane {
            domain_id: initial.id.clone(),
            remote_pane_id: 201,
            local_tab_id: usize::try_from(created_local_tab).unwrap(),
        };
        let (unproven_tx, unproven_rx) = std::sync::mpsc::sync_channel(1);
        application
            .pending_domain_controls
            .push(PendingDomainControl {
                request: unproven_close_request.clone(),
                window_id: native_id,
                control: unproven_close_control.clone(),
                context: PendingDomainContext::None,
                responders: vec![unproven_tx],
            });
        application.complete_live_domain_control(native_id, &unproven_close_control, &Ok(split));
        let unproven_error = unproven_rx.recv().unwrap().unwrap_err();
        assert!(unproven_error.contains("still contains the pane"));
        assert_eq!(
            application.control_replays.replay(&unproven_close_request),
            Some(Err(unproven_error))
        );

        let resize_request = crate::control::protocol::Request::ResizeDivider {
            request_id: "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb".into(),
            window_id: 7,
            tab_id: created_local_tab,
            divider_id: 55,
            ratio_millis: 650,
        };
        let resize_control = crate::app::LiveDomainControl::ResizeSplit {
            domain_id: initial.id.clone(),
            remote_split_id: 55,
            ratio_millis: 650,
        };
        let (resize_tx, resize_rx) = std::sync::mpsc::sync_channel(1);
        application
            .pending_domain_controls
            .push(PendingDomainControl {
                request: resize_request.clone(),
                window_id: native_id,
                control: resize_control.clone(),
                context: PendingDomainContext::ResizeDivider {
                    previous_ratio_millis: 500,
                },
                responders: vec![resize_tx],
            });
        let mut resized_tab = tab(20, 200);
        resized_tab.focused_pane = 201;
        resized_tab.panes.push(pane(201));
        resized_tab.layout = LivePaneLayout::Split {
            split_id: 55,
            direction: SplitDirection::Vertical,
            ratio_millis: 650,
            first: Box::new(LivePaneLayout::Pane { pane: 200 }),
            second: Box::new(LivePaneLayout::Pane { pane: 201 }),
        };
        let resized = snapshot(20, vec![tab(10, 100), resized_tab]);
        let resized_result = Ok(resized);
        application
            .windows
            .get_mut(&native_id)
            .unwrap()
            .handle_live_domain_control_completed(resize_control.clone(), resized_result.clone());
        application.complete_live_domain_control(native_id, &resize_control, &resized_result);
        assert!(matches!(
            resize_rx.recv().unwrap().unwrap(),
            crate::control::protocol::Response::DividerResized {
                window_id: 7,
                tab_id,
                divider_id: 55,
                previous_ratio_millis: 500,
                ratio_millis: 650,
                changed: true,
                ..
            } if tab_id == created_local_tab
        ));
        assert_eq!(
            application.windows[&native_id]
                .control_live_divider_ratio(created_local_tab, 55)
                .unwrap(),
            650
        );

        let destination_local_tab = application.windows[&native_id]
            .control_local_live_tab_id(&initial.id, 10)
            .unwrap();
        let move_pane_request = crate::control::protocol::Request::MovePane {
            request_id: "dddddddddddddddddddddddddddddddd".into(),
            source_window_id: 7,
            source_tab_id: created_local_tab,
            pane_id: 1,
            destination_window_id: 7,
            destination_tab_id: destination_local_tab,
            target_pane_id: 0,
            direction: crate::control::protocol::SplitDirection::Horizontal,
        };
        let move_pane_control = application.windows[&native_id]
            .control_live_domain_move_pane(
                created_local_tab,
                1,
                destination_local_tab,
                0,
                crate::control::protocol::SplitDirection::Horizontal,
            )
            .unwrap()
            .unwrap();
        assert!(matches!(
            &move_pane_control,
            crate::app::LiveDomainControl::MovePane {
                remote_pane_id: 201,
                source_tab_id: 20,
                destination_tab_id: 10,
                target_pane_id: 100,
                direction: SplitDirection::Horizontal,
                ..
            }
        ));
        let (move_pane_tx, move_pane_rx) = std::sync::mpsc::sync_channel(1);
        application
            .pending_domain_controls
            .push(PendingDomainControl {
                request: move_pane_request.clone(),
                window_id: native_id,
                control: move_pane_control.clone(),
                context: PendingDomainContext::None,
                responders: vec![move_pane_tx],
            });
        let mut destination_after_move = tab(10, 100);
        destination_after_move.focused_pane = 201;
        destination_after_move.panes.push(pane(201));
        destination_after_move.layout = LivePaneLayout::Split {
            split_id: 56,
            direction: SplitDirection::Horizontal,
            ratio_millis: 500,
            first: Box::new(LivePaneLayout::Pane { pane: 100 }),
            second: Box::new(LivePaneLayout::Pane { pane: 201 }),
        };
        let pane_moved = snapshot(10, vec![destination_after_move, tab(20, 200)]);
        let pane_moved_result = Ok(pane_moved);
        application
            .windows
            .get_mut(&native_id)
            .unwrap()
            .handle_live_domain_control_completed(
                move_pane_control.clone(),
                pane_moved_result.clone(),
            );
        application.complete_live_domain_control(native_id, &move_pane_control, &pane_moved_result);
        let moved_local_pane = match move_pane_rx.recv().unwrap().unwrap() {
            crate::control::protocol::Response::PaneMoved {
                source_window_id: 7,
                source_tab_id,
                source_pane_id: 1,
                destination_window_id: 7,
                destination_tab_id,
                destination_pane_id,
                target_pane_id: 0,
                ..
            } if source_tab_id == created_local_tab
                && destination_tab_id == destination_local_tab =>
            {
                destination_pane_id
            }
            response => panic!("unexpected live pane move response {response:?}"),
        };
        assert_ne!(moved_local_pane, 0);

        let move_tab_request = crate::control::protocol::Request::MoveTab {
            request_id: "cccccccccccccccccccccccccccccccc".into(),
            source_window_id: 7,
            tab_id: created_local_tab,
            destination_window_id: 7,
            destination_index: 0,
        };
        let (move_tab_control, move_changed) = application.windows[&native_id]
            .control_live_domain_move_tab(created_local_tab, 0)
            .unwrap()
            .unwrap();
        assert!(move_changed);
        assert!(matches!(
            &move_tab_control,
            crate::app::LiveDomainControl::MoveTab {
                remote_tab_id: 20,
                index: 0,
                ..
            }
        ));
        let (move_tab_tx, move_tab_rx) = std::sync::mpsc::sync_channel(1);
        application
            .pending_domain_controls
            .push(PendingDomainControl {
                request: move_tab_request.clone(),
                window_id: native_id,
                control: move_tab_control.clone(),
                context: PendingDomainContext::MoveTab { changed: true },
                responders: vec![move_tab_tx],
            });
        let mut moved_destination = tab(10, 100);
        moved_destination.focused_pane = 201;
        moved_destination.panes.push(pane(201));
        moved_destination.layout = LivePaneLayout::Split {
            split_id: 56,
            direction: SplitDirection::Horizontal,
            ratio_millis: 500,
            first: Box::new(LivePaneLayout::Pane { pane: 100 }),
            second: Box::new(LivePaneLayout::Pane { pane: 201 }),
        };
        let moved = snapshot(10, vec![tab(20, 200), moved_destination]);
        let moved_result = Ok(moved);
        application
            .windows
            .get_mut(&native_id)
            .unwrap()
            .handle_live_domain_control_completed(move_tab_control.clone(), moved_result.clone());
        application.complete_live_domain_control(native_id, &move_tab_control, &moved_result);
        assert!(matches!(
            move_tab_rx.recv().unwrap().unwrap(),
            crate::control::protocol::Response::TabMoved {
                source_window_id: 7,
                source_tab_id,
                destination_window_id: 7,
                destination_tab_id,
                destination_index: 0,
                source_window_closing: false,
                changed: true,
                ..
            } if source_tab_id == created_local_tab && destination_tab_id == created_local_tab
        ));

        let close_pane_request = crate::control::protocol::Request::ClosePane {
            request_id: "88888888888888888888888888888888".into(),
            window_id: 7,
            tab_id: destination_local_tab,
            pane_id: moved_local_pane,
        };
        let close_pane_control = crate::app::LiveDomainControl::ClosePane {
            domain_id: initial.id.clone(),
            remote_pane_id: 201,
            local_tab_id: usize::try_from(destination_local_tab).unwrap(),
        };
        let (close_pane_tx, close_pane_rx) = std::sync::mpsc::sync_channel(1);
        application
            .pending_domain_controls
            .push(PendingDomainControl {
                request: close_pane_request.clone(),
                window_id: native_id,
                control: close_pane_control.clone(),
                context: PendingDomainContext::None,
                responders: vec![close_pane_tx],
            });
        let pane_closed = snapshot(10, vec![tab(20, 200), tab(10, 100)]);
        let pane_closed_result = Ok(pane_closed);
        application
            .windows
            .get_mut(&native_id)
            .unwrap()
            .handle_live_domain_control_completed(
                close_pane_control.clone(),
                pane_closed_result.clone(),
            );
        application.complete_live_domain_control(
            native_id,
            &close_pane_control,
            &pane_closed_result,
        );
        assert!(matches!(
            close_pane_rx.recv().unwrap().unwrap(),
            crate::control::protocol::Response::PaneClosed {
                window_id: 7,
                tab_id,
                pane_id,
                remaining_panes: 1,
                focused_pane_id: 0,
                ..
            } if tab_id == destination_local_tab && pane_id == moved_local_pane
        ));
        assert!(
            application.windows[&native_id]
                .control_live_domain_close_pane(destination_local_tab, 0)
                .unwrap_err()
                .contains("sole pane")
        );

        let close_tab_request = crate::control::protocol::Request::CloseTab {
            request_id: "99999999999999999999999999999999".into(),
            window_id: 7,
            tab_id: created_local_tab,
        };
        let close_tab_control = crate::app::LiveDomainControl::CloseTab {
            domain_id: initial.id.clone(),
            remote_tab_id: 20,
            local_tab_id: usize::try_from(created_local_tab).unwrap(),
        };
        let (close_tab_tx, close_tab_rx) = std::sync::mpsc::sync_channel(1);
        application
            .pending_domain_controls
            .push(PendingDomainControl {
                request: close_tab_request.clone(),
                window_id: native_id,
                control: close_tab_control.clone(),
                context: PendingDomainContext::None,
                responders: vec![close_tab_tx],
            });
        let tab_closed = snapshot(10, vec![tab(10, 100)]);
        let tab_closed_result = Ok(tab_closed);
        application
            .windows
            .get_mut(&native_id)
            .unwrap()
            .handle_live_domain_control_completed(
                close_tab_control.clone(),
                tab_closed_result.clone(),
            );
        application.complete_live_domain_control(native_id, &close_tab_control, &tab_closed_result);
        assert!(matches!(
            close_tab_rx.recv().unwrap().unwrap(),
            crate::control::protocol::Response::TabCloseRequested {
                window_id: 7,
                tab_id,
                ..
            } if tab_id == created_local_tab
        ));
        let remaining_tab = application.windows[&native_id]
            .control_local_live_tab_id(&initial.id, 10)
            .unwrap();
        assert!(
            application.windows[&native_id]
                .control_live_domain_close_tab(remaining_tab)
                .unwrap_err()
                .contains("last tab")
        );
        assert!(application.pending_domain_controls.is_empty());
    }

    #[test]
    fn explicit_multi_window_plan_disables_implicit_restore_for_every_window() {
        let plan = LaunchPlan {
            windows: vec![WindowLaunch::default(), WindowLaunch::default()],
        };
        let application = Application::from_config(Config::default(), None, plan).unwrap();
        assert_eq!(application.pending.len(), 2);
        assert!(
            application
                .pending
                .iter()
                .all(|window| !window.allow_auto_restore && window.managed_window)
        );
    }

    #[test]
    fn plugin_process_inventory_is_ordered_bounded_and_excludes_closing_windows() {
        let plan = LaunchPlan {
            windows: vec![WindowLaunch::default(); super::super::PLUGIN_PROCESS_MAX_WINDOWS + 2],
        };
        let mut application = Application::from_config(Config::default(), None, plan).unwrap();
        application.pending[1].should_exit = true;
        let windows: Vec<_> = application
            .pending
            .iter()
            .enumerate()
            .map(|(index, window)| (100 + index as u64, window))
            .collect();

        let one_window = build_plugin_process_inventory(&windows[..1]);
        assert_eq!(one_window.windows.len(), 1);
        assert_eq!(one_window.windows[0].id, 100);
        assert!(!one_window.truncated);

        let two_live_windows = build_plugin_process_inventory(&windows[..3]);
        assert_eq!(two_live_windows.windows.len(), 2);
        assert_eq!(two_live_windows.windows[0].id, 100);
        assert_eq!(two_live_windows.windows[1].id, 102);
        assert!(!two_live_windows.truncated);

        drop(windows);
        application.pending[0].current_workspace_id = Some(format!("root\n{}", "β".repeat(2_000)));
        let windows: Vec<_> = application
            .pending
            .iter()
            .enumerate()
            .map(|(index, window)| (100 + index as u64, window))
            .collect();
        let inventory = build_plugin_process_inventory(&windows);
        assert_eq!(inventory.process_id, std::process::id());
        assert_eq!(
            inventory.windows.len(),
            super::super::PLUGIN_PROCESS_MAX_WINDOWS
        );
        assert_eq!(inventory.windows[0].id, 100);
        assert_eq!(inventory.windows[1].id, 102);
        assert!(inventory.truncated);
        assert!(inventory.windows[0].truncated);
        let workspace = inventory.windows[0].workspace_id.as_deref().unwrap();
        assert!(!workspace.contains('\n'));
        assert!(std::str::from_utf8(workspace.as_bytes()).is_ok());

        let encoded = App::structured_process_query_event(
            "process-inspector",
            "all-windows",
            102,
            &inventory,
        )
        .unwrap();
        assert!(encoded.len() <= crate::plugin::structured::MAX_PERSISTENT_SOURCE_EVENT_BYTES);
        let value: serde_json::Value = serde_json::from_str(&encoded).unwrap();
        assert_eq!(value["event"]["type"], "process-query-result");
        assert_eq!(value["event"]["origin_window_id"], 102);
        assert_eq!(value["event"]["windows"][0]["id"], 100);
        assert_eq!(value["event"]["windows"][1]["id"], 102);
        assert_eq!(value["event"]["truncated"], true);
    }

    #[cfg(unix)]
    #[test]
    fn plugin_process_navigation_is_exact_atomic_non_activating_and_hook_suppressed() {
        if !std::path::Path::new("/bin/sh").is_file() {
            return;
        }

        #[derive(Debug)]
        struct TabSwitchObserver(std::sync::Arc<std::sync::atomic::AtomicUsize>);

        impl crate::plugin::hooks::Plugin for TabSwitchObserver {
            fn id(&self) -> &str {
                "process-navigator"
            }

            fn name(&self) -> &str {
                "Process Navigation Observer"
            }

            fn on_hook(
                &mut self,
                hook: &crate::plugin::Hook,
                _context: &crate::plugin::PluginContext,
            ) -> crate::plugin::hooks::HookResponse {
                if matches!(hook, crate::plugin::Hook::TabSwitched { .. }) {
                    self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                }
                crate::plugin::hooks::HookResponse::Ignored
            }
        }

        let shell = crate::config::ShellConfig {
            program: Some("/bin/sh".into()),
            args: vec!["-c".into(), "exit 0".into()],
            env: Default::default(),
        };
        let mut tabs = crate::tabs::TabManager::new(80, 24, shell, 1_000, 8 * 1024 * 1024);
        let first = tabs
            .new_tab_with_command(
                "/bin/sh",
                &["-c".into(), "exit 0".into()],
                None,
                Some("first"),
                None,
                std::collections::HashMap::new(),
            )
            .unwrap();
        let second = tabs
            .new_tab_with_command(
                "/bin/sh",
                &["-c".into(), "exit 0".into()],
                None,
                Some("second"),
                None,
                std::collections::HashMap::new(),
            )
            .unwrap();
        let first_pane = tabs.tab_by_id(first).unwrap().single_pane_id().0;
        assert_eq!(tabs.active_tab_id(), Some(second));

        let observed_switches = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let mut target = App::new_headless(Config::default()).unwrap();
        target.tabs = Some(tabs);
        let context = target.build_plugin_context();
        target
            .plugin_manager
            .register(
                Box::new(TabSwitchObserver(std::sync::Arc::clone(&observed_switches))),
                &context,
            )
            .unwrap();

        let mut application =
            Application::from_config(Config::default(), None, LaunchPlan::default()).unwrap();
        application.pending.clear();
        let native_id = WindowId::dummy();
        application.windows.insert(native_id, target);
        application.window_order.push(native_id);
        application.control_window_ids.insert(native_id, 7);

        let selected = application
            .plugin_process_action_event(
                "process-navigator",
                9,
                crate::plugin::structured::StructuredResponseAction::SelectProcessTab {
                    action_id: "select-sibling".into(),
                    window_id: 7,
                    tab_id: first as u64,
                },
            )
            .unwrap();
        let selected: serde_json::Value = serde_json::from_str(&selected).unwrap();
        assert_eq!(selected["plugin_id"], "process-navigator");
        assert_eq!(selected["event"]["type"], "process-action-result");
        assert_eq!(selected["event"]["origin_window_id"], 9);
        assert_eq!(selected["event"]["window_id"], 7);
        assert_eq!(selected["event"]["tab_id"], first as u64);
        assert_eq!(selected["event"]["changed"], true);
        assert!(application.windows[&native_id].window.is_none());
        assert_eq!(
            application.windows[&native_id]
                .tabs
                .as_ref()
                .unwrap()
                .active_tab_id(),
            Some(first)
        );
        assert_eq!(
            observed_switches.load(std::sync::atomic::Ordering::SeqCst),
            0
        );

        let selected_same_window = application
            .plugin_process_action_event(
                "process-navigator",
                7,
                crate::plugin::structured::StructuredResponseAction::SelectProcessTab {
                    action_id: "select-current".into(),
                    window_id: 7,
                    tab_id: second as u64,
                },
            )
            .unwrap();
        let selected_same_window: serde_json::Value =
            serde_json::from_str(&selected_same_window).unwrap();
        assert_eq!(selected_same_window["event"]["ok"], true);
        assert_eq!(selected_same_window["event"]["origin_window_id"], 7);
        assert_eq!(selected_same_window["event"]["changed"], true);

        let generation = application.windows[&native_id].plugin_runtime_generation;
        application.handle_structured_plugin_completed(
            crate::plugin::structured::StructuredHandlerCompletion {
                window_id: native_id,
                plugin_generation: generation,
                plugin_id: "process-navigator".into(),
                request_id: 99,
                continuation: None,
                result: Ok(vec![
                    crate::plugin::structured::StructuredResponseAction::SelectProcessTab {
                        action_id: "missing-continuation".into(),
                        window_id: 7,
                        tab_id: first as u64,
                    },
                ]),
            },
        );
        assert_eq!(
            application.windows[&native_id]
                .tabs
                .as_ref()
                .unwrap()
                .active_tab_id(),
            Some(second)
        );

        let missing_pane = application
            .plugin_process_action_event(
                "process-navigator",
                7,
                crate::plugin::structured::StructuredResponseAction::FocusProcessPane {
                    action_id: "missing-pane".into(),
                    window_id: 7,
                    tab_id: first as u64,
                    pane_id: u32::MAX,
                },
            )
            .unwrap();
        let missing_pane: serde_json::Value = serde_json::from_str(&missing_pane).unwrap();
        assert_eq!(missing_pane["event"]["ok"], false);
        assert_eq!(
            missing_pane["event"]["tab_changed"],
            serde_json::Value::Null
        );
        assert_eq!(
            application.windows[&native_id]
                .tabs
                .as_ref()
                .unwrap()
                .active_tab_id(),
            Some(second)
        );
        let error = missing_pane["event"]["error"].as_str().unwrap();
        assert!(!error.is_empty());
        assert!(!error.chars().any(char::is_control));

        let focused = application
            .plugin_process_action_event(
                "process-navigator",
                9,
                crate::plugin::structured::StructuredResponseAction::FocusProcessPane {
                    action_id: "focus-sibling".into(),
                    window_id: 7,
                    tab_id: first as u64,
                    pane_id: first_pane,
                },
            )
            .unwrap();
        let focused: serde_json::Value = serde_json::from_str(&focused).unwrap();
        assert_eq!(focused["event"]["ok"], true);
        assert_eq!(focused["event"]["tab_changed"], true);
        assert_eq!(focused["event"]["pane_changed"], false);
        assert_eq!(
            observed_switches.load(std::sync::atomic::Ordering::SeqCst),
            0
        );

        let stale = application
            .plugin_process_action_event(
                "process-navigator",
                9,
                crate::plugin::structured::StructuredResponseAction::SelectProcessTab {
                    action_id: "stale-window".into(),
                    window_id: 8,
                    tab_id: first as u64,
                },
            )
            .unwrap();
        let stale: serde_json::Value = serde_json::from_str(&stale).unwrap();
        assert_eq!(stale["event"]["ok"], false);
        assert!(
            stale["event"]["error"]
                .as_str()
                .unwrap()
                .contains("does not exist")
        );

        application
            .windows
            .get_mut(&native_id)
            .unwrap()
            .tabs
            .as_mut()
            .unwrap()
            .tab_mut_by_id(second)
            .unwrap()
            .closed = true;
        let closed = application
            .plugin_process_action_event(
                "process-navigator",
                9,
                crate::plugin::structured::StructuredResponseAction::SelectProcessTab {
                    action_id: "closed-tab".into(),
                    window_id: 7,
                    tab_id: second as u64,
                },
            )
            .unwrap();
        let closed: serde_json::Value = serde_json::from_str(&closed).unwrap();
        assert_eq!(closed["event"]["ok"], false);
        assert!(
            closed["event"]["error"]
                .as_str()
                .unwrap()
                .contains("closing")
        );

        application.windows.get_mut(&native_id).unwrap().should_exit = true;
        let closing = application
            .plugin_process_action_event(
                "process-navigator",
                9,
                crate::plugin::structured::StructuredResponseAction::SelectProcessTab {
                    action_id: "closing-window".into(),
                    window_id: 7,
                    tab_id: second as u64,
                },
            )
            .unwrap();
        let closing: serde_json::Value = serde_json::from_str(&closing).unwrap();
        assert_eq!(closing["event"]["ok"], false);
        assert!(
            closing["event"]["error"]
                .as_str()
                .unwrap()
                .contains("closing")
        );

        let target = application.windows.get_mut(&native_id).unwrap();
        target.should_exit = false;
        target.tabs = None;
        let uninitialized = application
            .plugin_process_action_event(
                "process-navigator",
                9,
                crate::plugin::structured::StructuredResponseAction::SelectProcessTab {
                    action_id: "uninitialized-window".into(),
                    window_id: 7,
                    tab_id: first as u64,
                },
            )
            .unwrap();
        let uninitialized: serde_json::Value = serde_json::from_str(&uninitialized).unwrap();
        assert_eq!(uninitialized["event"]["ok"], false);
        assert!(
            uninitialized["event"]["error"]
                .as_str()
                .unwrap()
                .contains("no initialized tab manager")
        );
        assert!(
            selected.to_string().len()
                <= crate::plugin::structured::MAX_PERSISTENT_SOURCE_EVENT_BYTES
        );
    }

    #[cfg(unix)]
    #[test]
    fn plugin_process_tab_creation_is_exact_non_activating_and_hook_suppressed() {
        if !std::path::Path::new("/bin/sh").is_file() {
            return;
        }

        #[derive(Debug)]
        struct TabCreatedObserver(std::sync::Arc<std::sync::atomic::AtomicUsize>);

        impl crate::plugin::hooks::Plugin for TabCreatedObserver {
            fn id(&self) -> &str {
                "process-tab-builder"
            }

            fn name(&self) -> &str {
                "Process Tab Builder Observer"
            }

            fn on_hook(
                &mut self,
                hook: &crate::plugin::Hook,
                _context: &crate::plugin::PluginContext,
            ) -> crate::plugin::hooks::HookResponse {
                if matches!(hook, crate::plugin::Hook::TabCreated { .. }) {
                    self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                }
                crate::plugin::hooks::HookResponse::Ignored
            }
        }

        let shell = crate::config::ShellConfig {
            program: Some("/bin/sh".into()),
            args: vec!["-c".into(), "exit 0".into()],
            env: Default::default(),
        };
        let mut tabs = crate::tabs::TabManager::new(80, 24, shell, 1_000, 8 * 1024 * 1024);
        let initial = tabs.new_tab().unwrap();
        let observed_creations = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let mut target = App::new_headless(Config::default()).unwrap();
        target.tabs = Some(tabs);
        let context = target.build_plugin_context();
        target
            .plugin_manager
            .register(
                Box::new(TabCreatedObserver(std::sync::Arc::clone(
                    &observed_creations,
                ))),
                &context,
            )
            .unwrap();

        let mut application =
            Application::from_config(Config::default(), None, LaunchPlan::default()).unwrap();
        application.pending.clear();
        let native_id = WindowId::dummy();
        application.windows.insert(native_id, target);
        application.window_order.push(native_id);
        application.control_window_ids.insert(native_id, 7);

        let created = application
            .plugin_process_action_event(
                "process-tab-builder",
                9,
                crate::plugin::structured::StructuredResponseAction::CreateProcessTab {
                    action_id: "new-sibling-task".into(),
                    window_id: 7,
                },
            )
            .unwrap();
        let created: serde_json::Value = serde_json::from_str(&created).unwrap();
        assert_eq!(created["plugin_id"], "process-tab-builder");
        assert_eq!(created["event"]["type"], "process-action-result");
        assert_eq!(created["event"]["action"], "create-process-tab");
        assert_eq!(created["event"]["action_id"], "new-sibling-task");
        assert_eq!(created["event"]["ok"], true);
        assert_eq!(created["event"]["origin_window_id"], 9);
        assert_eq!(created["event"]["window_id"], 7);
        assert_eq!(created["event"]["selected"], true);
        let created_tab = created["event"]["tab_id"].as_u64().unwrap() as usize;
        assert_ne!(created_tab, initial);
        let target = &application.windows[&native_id];
        assert!(target.window.is_none());
        assert_eq!(target.tabs.as_ref().unwrap().count(), 2);
        assert_eq!(
            target.tabs.as_ref().unwrap().active_tab_id(),
            Some(created_tab)
        );
        assert_eq!(
            observed_creations.load(std::sync::atomic::Ordering::SeqCst),
            0
        );

        let created_same_window = application
            .plugin_process_action_event(
                "process-tab-builder",
                7,
                crate::plugin::structured::StructuredResponseAction::CreateProcessTab {
                    action_id: "new-current-task".into(),
                    window_id: 7,
                },
            )
            .unwrap();
        let created_same_window: serde_json::Value =
            serde_json::from_str(&created_same_window).unwrap();
        assert_eq!(created_same_window["event"]["ok"], true);
        assert_eq!(created_same_window["event"]["origin_window_id"], 7);
        assert_eq!(created_same_window["event"]["window_id"], 7);
        let same_window_tab = created_same_window["event"]["tab_id"].as_u64().unwrap() as usize;
        assert_eq!(
            application.windows[&native_id]
                .tabs
                .as_ref()
                .unwrap()
                .active_tab_id(),
            Some(same_window_tab)
        );
        assert_eq!(
            observed_creations.load(std::sync::atomic::Ordering::SeqCst),
            0
        );

        let stale = application
            .plugin_process_action_event(
                "process-tab-builder",
                9,
                crate::plugin::structured::StructuredResponseAction::CreateProcessTab {
                    action_id: "stale-window".into(),
                    window_id: 8,
                },
            )
            .unwrap();
        let stale: serde_json::Value = serde_json::from_str(&stale).unwrap();
        assert_eq!(stale["event"]["ok"], false);
        assert!(stale["event"]["tab_id"].is_null());
        assert!(
            stale["event"]["error"]
                .as_str()
                .unwrap()
                .contains("does not exist")
        );
        assert_eq!(
            application.windows[&native_id]
                .tabs
                .as_ref()
                .unwrap()
                .count(),
            3
        );

        application.windows.get_mut(&native_id).unwrap().should_exit = true;
        let closing = application
            .plugin_process_action_event(
                "process-tab-builder",
                9,
                crate::plugin::structured::StructuredResponseAction::CreateProcessTab {
                    action_id: "closing-window".into(),
                    window_id: 7,
                },
            )
            .unwrap();
        let closing: serde_json::Value = serde_json::from_str(&closing).unwrap();
        assert_eq!(closing["event"]["ok"], false);
        assert!(
            closing["event"]["error"]
                .as_str()
                .unwrap()
                .contains("closing")
        );
        assert_eq!(
            application.windows[&native_id]
                .tabs
                .as_ref()
                .unwrap()
                .count(),
            3
        );

        let target = application.windows.get_mut(&native_id).unwrap();
        target.should_exit = false;
        target
            .tabs
            .as_mut()
            .unwrap()
            .active_tab_mut()
            .unwrap()
            .live_domain = Some(crate::tabs::LiveDomainTab {
            domain_id: "fake-domain".into(),
            tab_id: 12,
            pane_ids: HashMap::new(),
        });
        let live_domain = application
            .plugin_process_action_event(
                "process-tab-builder",
                9,
                crate::plugin::structured::StructuredResponseAction::CreateProcessTab {
                    action_id: "live-domain-window".into(),
                    window_id: 7,
                },
            )
            .unwrap();
        let live_domain: serde_json::Value = serde_json::from_str(&live_domain).unwrap();
        assert_eq!(live_domain["event"]["ok"], false);
        assert!(
            live_domain["event"]["error"]
                .as_str()
                .unwrap()
                .contains("live domain")
        );
        assert_eq!(
            application.windows[&native_id]
                .tabs
                .as_ref()
                .unwrap()
                .count(),
            3
        );
        assert!(
            created.to_string().len()
                <= crate::plugin::structured::MAX_PERSISTENT_SOURCE_EVENT_BYTES
        );
    }

    #[cfg(unix)]
    #[test]
    fn plugin_process_terminal_queries_are_exact_bounded_and_non_activating() {
        if !std::path::Path::new("/bin/sh").is_file() {
            return;
        }

        let shell = crate::config::ShellConfig {
            program: Some("/bin/sh".into()),
            args: vec!["-c".into(), "exit 0".into()],
            env: Default::default(),
        };
        let mut tabs = crate::tabs::TabManager::new(80, 24, shell, 1_000, 8 * 1024 * 1024);
        let target_tab = tabs
            .new_tab_with_command(
                "/bin/sh",
                &["-c".into(), "exit 0".into()],
                None,
                Some("query target"),
                None,
                std::collections::HashMap::new(),
            )
            .unwrap();
        let untouched_tab = tabs
            .new_tab_with_command(
                "/bin/sh",
                &["-c".into(), "exit 0".into()],
                None,
                Some("active tab"),
                None,
                std::collections::HashMap::new(),
            )
            .unwrap();
        tabs.tab_mut_by_id(target_tab)
            .unwrap()
            .focused_terminal_mut()
            .unwrap()
            .process(b"sibling-visible-text\x1b[?2026h\rhidden-sibling-update");
        assert_eq!(tabs.active_tab_id(), Some(untouched_tab));

        let mut target = App::new_headless(Config::default()).unwrap();
        target.tabs = Some(tabs);
        let mut application =
            Application::from_config(Config::default(), None, LaunchPlan::default()).unwrap();
        application.pending.clear();
        let native_id = WindowId::dummy();
        application.windows.insert(native_id, target);
        application.window_order.push(native_id);
        application.control_window_ids.insert(native_id, 7);

        let encoded = application
            .plugin_process_terminal_query_event(
                "process-terminal-reader",
                9,
                "sibling-output",
                7,
                target_tab as u64,
                0,
                20,
                2048,
            )
            .unwrap();
        assert!(encoded.len() <= crate::plugin::structured::MAX_PERSISTENT_SOURCE_EVENT_BYTES);
        let value: serde_json::Value = serde_json::from_str(&encoded).unwrap();
        assert_eq!(value["schema_version"], 1);
        assert_eq!(value["plugin_id"], "process-terminal-reader");
        assert_eq!(value["event"]["type"], "process-terminal-query-result");
        assert_eq!(value["event"]["query_id"], "sibling-output");
        assert_eq!(value["event"]["text_schema_version"], 1);
        assert_eq!(value["event"]["ok"], true);
        assert_eq!(value["event"]["origin_window_id"], 9);
        assert_eq!(value["event"]["window_id"], 7);
        assert_eq!(value["event"]["tab_id"], target_tab as u64);
        assert_eq!(value["event"]["pane_id"], 0);
        assert!(
            value["event"]["text"]
                .as_str()
                .unwrap()
                .contains("sibling-visible-text")
        );
        assert!(
            !value["event"]["text"]
                .as_str()
                .unwrap()
                .contains("hidden-sibling-update")
        );
        assert!(application.windows[&native_id].window.is_none());
        assert_eq!(
            application.windows[&native_id]
                .tabs
                .as_ref()
                .unwrap()
                .active_tab_id(),
            Some(untouched_tab)
        );

        let missing = application
            .plugin_process_terminal_query_event(
                "process-terminal-reader",
                9,
                "stale-target",
                8,
                target_tab as u64,
                0,
                0,
                64,
            )
            .unwrap();
        let missing: serde_json::Value = serde_json::from_str(&missing).unwrap();
        assert_eq!(missing["event"]["ok"], false);
        assert_eq!(missing["event"]["query_id"], "stale-target");
        assert!(missing["event"]["text"].is_null());
        let error = missing["event"]["error"].as_str().unwrap();
        assert!(error.contains("does not exist"));
        assert!(!error.chars().any(char::is_control));

        application
            .windows
            .get_mut(&native_id)
            .unwrap()
            .tabs
            .as_mut()
            .unwrap()
            .tab_mut_by_id(target_tab)
            .unwrap()
            .focused_terminal_mut()
            .unwrap()
            .process(format!("\x1b[?2026l\x1b[2J\x1b[H{}", "\\".repeat(9_000)).as_bytes());
        let escaped = application
            .plugin_process_terminal_query_event(
                "process-terminal-reader",
                9,
                "escaped-text",
                7,
                target_tab as u64,
                0,
                256,
                crate::plugin::structured::MAX_TERMINAL_QUERY_BYTES,
            )
            .unwrap();
        assert!(escaped.len() <= crate::plugin::structured::MAX_PERSISTENT_SOURCE_EVENT_BYTES);
        let escaped: serde_json::Value = serde_json::from_str(&escaped).unwrap();
        assert_eq!(escaped["event"]["ok"], true);
        assert_eq!(escaped["event"]["truncated"], true);
        assert!(
            escaped["event"]["text_bytes"].as_u64().unwrap()
                < u64::from(crate::plugin::structured::MAX_TERMINAL_QUERY_BYTES)
        );

        assert!(
            application
                .plugin_process_terminal_query_event(
                    "process-terminal-reader",
                    9,
                    "invalid-history",
                    7,
                    target_tab as u64,
                    0,
                    crate::control::protocol::MAX_TEXT_HISTORY_LINES + 1,
                    64,
                )
                .unwrap_err()
                .contains("limited")
        );
        assert!(
            application
                .plugin_process_terminal_query_event(
                    "process-terminal-reader",
                    9,
                    "invalid-bytes",
                    7,
                    target_tab as u64,
                    0,
                    0,
                    0,
                )
                .unwrap_err()
                .contains("between")
        );

        application.windows.get_mut(&native_id).unwrap().should_exit = true;
        let closing = application
            .plugin_process_terminal_query_event(
                "process-terminal-reader",
                9,
                "closing-window",
                7,
                target_tab as u64,
                0,
                0,
                64,
            )
            .unwrap();
        let closing: serde_json::Value = serde_json::from_str(&closing).unwrap();
        assert_eq!(closing["event"]["ok"], false);
        assert!(
            closing["event"]["error"]
                .as_str()
                .unwrap()
                .contains("closing")
        );
    }

    #[cfg(unix)]
    #[test]
    fn plugin_process_terminal_input_is_exact_guarded_policy_gated_and_non_activating() {
        if !std::path::Path::new("/bin/sh").is_file() {
            return;
        }

        let shell = crate::config::ShellConfig {
            program: Some("/bin/sh".into()),
            args: vec!["-c".into(), "sleep 2".into()],
            env: Default::default(),
        };
        let mut tabs = crate::tabs::TabManager::new(80, 24, shell, 1_000, 8 * 1024 * 1024);
        let target_tab = tabs
            .new_tab_with_command(
                "/bin/sh",
                &["-c".into(), "sleep 2".into()],
                None,
                Some("input target"),
                None,
                std::collections::HashMap::new(),
            )
            .unwrap();
        let untouched_tab = tabs
            .new_tab_with_command(
                "/bin/sh",
                &["-c".into(), "sleep 2".into()],
                None,
                Some("active tab"),
                None,
                std::collections::HashMap::new(),
            )
            .unwrap();
        tabs.tab_mut_by_id(target_tab)
            .unwrap()
            .focused_terminal_mut()
            .unwrap()
            .set_mode(termeric_core::term::TerminalMode::BRACKETED_PASTE, true);
        assert_eq!(tabs.active_tab_id(), Some(untouched_tab));

        let mut target = App::new_headless(Config::default()).unwrap();
        target.tabs = Some(tabs);
        let mut application =
            Application::from_config(Config::default(), None, LaunchPlan::default()).unwrap();
        application.pending.clear();
        let native_id = WindowId::dummy();
        application.windows.insert(native_id, target);
        application.window_order.push(native_id);
        application.control_window_ids.insert(native_id, 7);

        let disabled = application
            .plugin_process_terminal_input_event(
                "process-terminal-writer",
                9,
                "policy-disabled",
                7,
                target_tab as u64,
                0,
                "safe",
                true,
                true,
            )
            .unwrap();
        let disabled: serde_json::Value = serde_json::from_str(&disabled).unwrap();
        assert_eq!(disabled["event"]["ok"], false);
        assert!(
            disabled["event"]["error"]
                .as_str()
                .unwrap()
                .contains("disabled")
        );

        application
            .windows
            .get_mut(&native_id)
            .unwrap()
            .config
            .security
            .allow_gui_control_input = true;
        let sent = application
            .plugin_process_terminal_input_event(
                "process-terminal-writer",
                9,
                "safe-input",
                7,
                target_tab as u64,
                0,
                "safe",
                true,
                true,
            )
            .unwrap();
        assert!(sent.len() <= crate::plugin::structured::MAX_PERSISTENT_SOURCE_EVENT_BYTES);
        let sent: serde_json::Value = serde_json::from_str(&sent).unwrap();
        assert_eq!(sent["plugin_id"], "process-terminal-writer");
        assert_eq!(sent["event"]["type"], "process-action-result");
        assert_eq!(sent["event"]["action"], "send-process-text");
        assert_eq!(sent["event"]["action_id"], "safe-input");
        assert_eq!(sent["event"]["ok"], true);
        assert_eq!(sent["event"]["origin_window_id"], 9);
        assert_eq!(sent["event"]["window_id"], 7);
        assert_eq!(sent["event"]["tab_id"], target_tab as u64);
        assert_eq!(sent["event"]["pane_id"], 0);
        assert_eq!(sent["event"]["input_bytes"], 5);
        assert_eq!(sent["event"]["wire_bytes"], 17);
        assert_eq!(sent["event"]["disposition"], "sent");
        assert!(sent["event"]["confirmation_id"].is_null());
        assert!(application.windows[&native_id].window.is_none());
        assert_eq!(
            application.windows[&native_id]
                .tabs
                .as_ref()
                .unwrap()
                .active_tab_id(),
            Some(untouched_tab)
        );

        let stale = application
            .plugin_process_terminal_input_event(
                "process-terminal-writer",
                9,
                "stale-window",
                8,
                target_tab as u64,
                0,
                "safe",
                false,
                false,
            )
            .unwrap();
        let stale: serde_json::Value = serde_json::from_str(&stale).unwrap();
        assert_eq!(stale["event"]["ok"], false);
        assert!(
            stale["event"]["error"]
                .as_str()
                .unwrap()
                .contains("does not exist")
        );

        application.windows.get_mut(&native_id).unwrap().should_exit = true;
        let closing = application
            .plugin_process_terminal_input_event(
                "process-terminal-writer",
                9,
                "closing-window",
                7,
                target_tab as u64,
                0,
                "safe",
                false,
                false,
            )
            .unwrap();
        let closing: serde_json::Value = serde_json::from_str(&closing).unwrap();
        assert_eq!(closing["event"]["ok"], false);
        assert!(
            closing["event"]["error"]
                .as_str()
                .unwrap()
                .contains("closing")
        );
        application.windows.get_mut(&native_id).unwrap().should_exit = false;

        assert!(
            application
                .windows
                .get_mut(&native_id)
                .unwrap()
                .tabs
                .as_mut()
                .unwrap()
                .tab_mut_by_id(target_tab)
                .unwrap()
                .toggle_focused_pane_read_only()
        );
        let read_only = application
            .plugin_process_terminal_input_event(
                "process-terminal-writer",
                7,
                "read-only",
                7,
                target_tab as u64,
                0,
                "safe",
                false,
                false,
            )
            .unwrap();
        let read_only: serde_json::Value = serde_json::from_str(&read_only).unwrap();
        assert_eq!(read_only["event"]["ok"], false);
        assert!(
            read_only["event"]["error"]
                .as_str()
                .unwrap()
                .contains("read-only")
        );
        assert!(
            !application
                .windows
                .get_mut(&native_id)
                .unwrap()
                .tabs
                .as_mut()
                .unwrap()
                .tab_mut_by_id(target_tab)
                .unwrap()
                .toggle_focused_pane_read_only()
        );

        let guarded = application
            .plugin_process_terminal_input_event(
                "process-terminal-writer",
                9,
                "guarded-input",
                7,
                target_tab as u64,
                0,
                "rm -rf /",
                false,
                true,
            )
            .unwrap();
        let guarded: serde_json::Value = serde_json::from_str(&guarded).unwrap();
        assert_eq!(guarded["event"]["ok"], true);
        assert_eq!(guarded["event"]["disposition"], "pending_confirmation");
        let confirmation_id = guarded["event"]["confirmation_id"].as_u64().unwrap();
        let target = application.windows.get_mut(&native_id).unwrap();
        assert!(target.guardrail.is_awaiting(confirmation_id));
        assert_eq!(
            target.guardrail.matched.as_ref().unwrap().target,
            format!("GUI window 7, tab {target_tab}, pane 0")
        );
        assert_eq!(
            target.tabs.as_ref().unwrap().active_tab_id(),
            Some(untouched_tab)
        );
        target.apply_guardrail_action(crate::terminal::guardrails::GuardrailAction::Cancel {
            request_id: confirmation_id,
        });
        assert!(!target.guardrail.is_awaiting_current());

        assert!(
            application
                .plugin_process_terminal_input_event(
                    "process-terminal-writer",
                    9,
                    "bad\naction",
                    7,
                    target_tab as u64,
                    0,
                    "safe",
                    false,
                    false,
                )
                .unwrap_err()
                .contains("control-safe")
        );
        assert!(
            application
                .plugin_process_terminal_input_event(
                    "process-terminal-writer",
                    9,
                    "empty",
                    7,
                    target_tab as u64,
                    0,
                    "",
                    false,
                    false,
                )
                .unwrap_err()
                .contains("text or enter")
        );
    }

    #[test]
    fn sole_default_window_retains_automatic_restore_eligibility() {
        let application =
            Application::from_config(Config::default(), None, LaunchPlan::default()).unwrap();
        assert!(application.pending[0].allow_auto_restore);
    }

    #[test]
    fn cli_renderer_override_becomes_the_window_recovery_policy() {
        let launch = WindowLaunch {
            renderer_backend: Some(crate::config::RendererBackend::Software),
            ..WindowLaunch::default()
        };
        let application = Application::from_config(
            Config::default(),
            None,
            LaunchPlan {
                windows: vec![launch],
            },
        )
        .unwrap();
        assert_eq!(
            application.pending[0].config.renderer.backend,
            crate::config::RendererBackend::Software
        );
    }

    #[test]
    fn all_failed_native_startups_remain_an_actionable_process_error() {
        let mut application =
            Application::from_config(Config::default(), None, LaunchPlan::default()).unwrap();
        application.pending.clear();
        application.record_fatal_startup_errors(vec![
            "Failed to initialize renderer: no adapter".into(),
            "Failed to initialize renderer: device rejected".into(),
        ]);

        let error = application.take_fatal_startup_error().unwrap();
        assert!(error.contains("no adapter"));
        assert!(error.contains("device rejected"));
        assert!(application.take_fatal_startup_error().is_none());
    }

    #[test]
    fn cli_cwds_and_terminal_profiles_are_validated_before_windows_are_queued() {
        let missing = std::path::PathBuf::from("/definitely/missing/cli-cwd");
        let mut shell_launch = LaunchPlan::default();
        shell_launch.windows[0].tabs[0].working_directory = Some(missing.clone());
        assert!(Application::from_config(Config::default(), None, shell_launch.clone()).is_err());
        shell_launch.windows[0].tabs[0].target = crate::cli::LaunchTarget::Command {
            program: "/bin/sh".into(),
            args: Vec::new(),
        };
        assert!(Application::from_config(Config::default(), None, shell_launch).is_err());

        let mut launch = LaunchPlan::default();
        launch.windows[0].tabs[0].target =
            crate::cli::LaunchTarget::TerminalProfile { name: "dev".into() };
        assert!(Application::from_config(Config::default(), None, launch.clone()).is_err());

        let mut config = Config::default();
        config.profiles.insert(
            "dev".into(),
            crate::config::LaunchProfileConfig {
                program: "/bin/sh".into(),
                cwd: Some(std::path::PathBuf::from("/definitely/missing/profile-cwd")),
                ..crate::config::LaunchProfileConfig::default()
            },
        );
        assert!(Application::from_config(config.clone(), None, launch.clone()).is_err());

        launch.windows[0].tabs[0].working_directory = Some(std::env::temp_dir());
        assert!(Application::from_config(config, None, launch).is_ok());
    }

    #[cfg(unix)]
    #[test]
    fn non_utf8_cli_cwd_fails_before_windows_are_queued() {
        use std::os::unix::ffi::OsStringExt;

        let temp = tempfile::tempdir().unwrap();
        let path = temp
            .path()
            .join(std::ffi::OsString::from_vec(vec![b'c', b'w', b'd', 0xff]));
        std::fs::create_dir(&path).unwrap();
        let mut launch = LaunchPlan::default();
        launch.windows[0].tabs[0].working_directory = Some(path);
        let error = Application::from_config(Config::default(), None, launch)
            .err()
            .expect("non-UTF-8 cwd must fail startup validation");
        assert!(format!("{error:#}").contains("valid UTF-8"));
    }

    #[test]
    fn automatic_restore_expands_each_remembered_window_workspace() {
        let mut config = Config::default();
        config.session.auto_restore = true;
        let state = super::super::state::AppState {
            last_workspace_ids: vec!["workspace-a".into(), "workspace-b".into()],
            ..super::super::state::AppState::default()
        };
        let plan = expand_auto_restore_plan(&config, &state, LaunchPlan::default());
        assert_eq!(plan.windows.len(), 2);
        assert_eq!(plan.windows[0].workspace.as_deref(), Some("workspace-a"));
        assert_eq!(plan.windows[1].workspace.as_deref(), Some("workspace-b"));
    }

    #[test]
    fn queued_domain_window_preserves_remote_authority_and_disables_auto_restore() {
        let mut application =
            Application::from_config(Config::default(), None, LaunchPlan::default()).unwrap();
        application.pending.clear();
        let target = crate::domain::DomainTarget {
            authority: crate::domain::DomainAuthority::Ssh {
                destination: "developer@build.example".into(),
            },
            domain_id: "domain-7".into(),
        };
        application
            .queue_window(
                Config::default(),
                WindowLaunch {
                    force_new: true,
                    domain: Some(target.clone()),
                    ..WindowLaunch::default()
                },
            )
            .unwrap();
        assert_eq!(application.pending.len(), 1);
        assert_eq!(application.pending[0].launch.domain, Some(target));
        assert!(!application.pending[0].allow_auto_restore);
        assert!(application.pending[0].managed_window);
    }

    #[cfg(unix)]
    fn pane_transfer_fixture() -> (App, App, Option<u32>) {
        use crate::config::ShellConfig;
        use crate::panes::{PaneManager, SplitDirection};
        use crate::tabs::TabManager;
        use std::collections::HashMap;

        let shell = ShellConfig {
            program: Some("/bin/sh".into()),
            args: vec!["-c".into(), "sleep 5".into()],
            env: Default::default(),
        };
        let mut source_tabs = TabManager::new(80, 24, shell.clone(), 1_000, 1024 * 1024);
        source_tabs
            .new_tab_with_command(
                "/bin/sh",
                &["-c".into(), "sleep 5".into()],
                None,
                Some("source"),
                None,
                HashMap::new(),
            )
            .unwrap();
        let owner = source_tabs
            .active_tab_mut()
            .unwrap()
            .take_terminal_owner()
            .unwrap();
        let mut source_panes = PaneManager::from_existing(
            owner,
            shell.clone(),
            1_000,
            1024 * 1024,
            800.0,
            480.0,
            10.0,
            20.0,
        );
        let moved = source_panes.split(SplitDirection::Vertical).unwrap();
        source_panes.get_pane_mut(moved).unwrap().title = Some("moved-owner".into());
        let moved_child = source_panes.get_pane(moved).unwrap().pty.child_id();
        source_tabs.active_tab_mut().unwrap().pane_manager = Some(source_panes);

        let mut destination_tabs = TabManager::new(100, 30, shell, 1_000, 1024 * 1024);
        destination_tabs
            .new_tab_with_command(
                "/bin/sh",
                &["-c".into(), "sleep 5".into()],
                None,
                Some("destination"),
                None,
                HashMap::new(),
            )
            .unwrap();

        let mut source = App::new_headless(Config::default()).unwrap();
        source.tabs = Some(source_tabs);
        let mut destination = App::new_headless(Config::default()).unwrap();
        destination.tabs = Some(destination_tabs);
        (source, destination, moved_child)
    }

    #[cfg(unix)]
    #[test]
    fn cross_window_transfer_preserves_live_owner_and_retargets_destination_geometry() {
        if !std::path::Path::new("/bin/sh").is_file() {
            return;
        }
        let (mut source, mut destination, moved_child) = pane_transfer_fixture();

        let attached = Application::transfer_focused_pane(&mut source, &mut destination).unwrap();
        let source_manager = source
            .tabs
            .as_ref()
            .unwrap()
            .active_tab()
            .unwrap()
            .pane_manager
            .as_ref()
            .unwrap();
        assert_eq!(source_manager.pane_count(), 1);

        let destination_manager = destination
            .tabs
            .as_ref()
            .unwrap()
            .active_tab()
            .unwrap()
            .pane_manager
            .as_ref()
            .unwrap();
        let pane = destination_manager.get_pane(attached).unwrap();
        assert_eq!(destination_manager.pane_count(), 2);
        assert_eq!(destination_manager.focused_id(), attached);
        assert_eq!(destination_manager.spawned_shell_count(), 0);
        assert_eq!(pane.pty.child_id(), moved_child);
        assert_eq!(pane.title.as_deref(), Some("moved-owner"));
        assert_eq!(pane.geometry.cols, 48);
        assert_eq!(pane.geometry.rows, 30);
        source
            .tabs
            .as_ref()
            .unwrap()
            .active_tab()
            .unwrap()
            .capture_workspace_tab(0, false)
            .unwrap()
            .validate()
            .unwrap();
        destination
            .tabs
            .as_ref()
            .unwrap()
            .active_tab()
            .unwrap()
            .capture_workspace_tab(0, false)
            .unwrap()
            .validate()
            .unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn exact_control_pane_transfer_validates_both_paths_before_detaching() {
        if !std::path::Path::new("/bin/sh").is_file() {
            return;
        }
        let (mut source, mut destination, moved_child) = pane_transfer_fixture();
        let error = Application::transfer_control_pane(
            &mut source,
            &mut destination,
            0,
            1,
            0,
            99,
            crate::panes::SplitDirection::Vertical,
        )
        .unwrap_err();
        assert!(error.contains("does not exist"), "{error}");
        assert_eq!(
            source
                .tabs
                .as_ref()
                .unwrap()
                .active_tab()
                .unwrap()
                .pane_manager
                .as_ref()
                .unwrap()
                .pane_count(),
            2
        );
        assert!(
            destination
                .tabs
                .as_ref()
                .unwrap()
                .active_tab()
                .unwrap()
                .pane_manager
                .is_none()
        );

        let destination_id = Application::transfer_control_pane(
            &mut source,
            &mut destination,
            0,
            1,
            0,
            0,
            crate::panes::SplitDirection::Horizontal,
        )
        .unwrap();
        assert_eq!(destination_id, 1);
        let destination_manager = destination
            .tabs
            .as_ref()
            .unwrap()
            .active_tab()
            .unwrap()
            .pane_manager
            .as_ref()
            .unwrap();
        assert_eq!(destination_manager.pane_count(), 2);
        assert_eq!(destination_manager.focused_id().0, destination_id);
        assert_eq!(
            destination_manager
                .get_pane(crate::panes::PaneId(destination_id))
                .unwrap()
                .pty
                .child_id(),
            moved_child
        );
    }

    #[cfg(unix)]
    #[test]
    fn cross_window_transfer_rejects_live_endpoints_without_detaching_source() {
        use crate::tabs::LiveDomainTab;
        use std::collections::HashMap;

        if !std::path::Path::new("/bin/sh").is_file() {
            return;
        }
        let (mut source, mut destination, moved_child) = pane_transfer_fixture();
        destination
            .tabs
            .as_mut()
            .unwrap()
            .active_tab_mut()
            .unwrap()
            .live_domain = Some(LiveDomainTab {
            domain_id: "remote".into(),
            tab_id: 7,
            pane_ids: HashMap::from([(crate::panes::PaneId(0), 9)]),
        });

        let error = Application::transfer_focused_pane(&mut source, &mut destination)
            .unwrap_err()
            .to_string();
        assert!(error.contains("live-domain tab"), "{error}");
        let source_manager = source
            .tabs
            .as_ref()
            .unwrap()
            .active_tab()
            .unwrap()
            .pane_manager
            .as_ref()
            .unwrap();
        assert_eq!(source_manager.pane_count(), 2);
        assert!(
            source_manager
                .iter_with_layout()
                .any(|(pane, _)| pane.pty.child_id() == moved_child)
        );
        assert!(
            destination
                .tabs
                .as_ref()
                .unwrap()
                .active_tab()
                .unwrap()
                .pane_manager
                .is_none()
        );

        let (mut live_source, mut ordinary_destination, moved_child) = pane_transfer_fixture();
        let source_tab = live_source.tabs.as_mut().unwrap().active_tab_mut().unwrap();
        let pane_ids = source_tab
            .pane_manager
            .as_ref()
            .unwrap()
            .pane_ids()
            .into_iter()
            .enumerate()
            .map(|(index, id)| (id, u32::try_from(index + 1).unwrap()))
            .collect();
        source_tab.live_domain = Some(LiveDomainTab {
            domain_id: "remote".into(),
            tab_id: 8,
            pane_ids,
        });
        let error = Application::transfer_focused_pane(&mut live_source, &mut ordinary_destination)
            .unwrap_err()
            .to_string();
        assert!(error.contains("daemon-owned"), "{error}");
        assert!(
            live_source
                .tabs
                .as_ref()
                .unwrap()
                .active_tab()
                .unwrap()
                .pane_manager
                .as_ref()
                .unwrap()
                .iter_with_layout()
                .any(|(pane, _)| pane.pty.child_id() == moved_child)
        );
        assert!(
            ordinary_destination
                .tabs
                .as_ref()
                .unwrap()
                .active_tab()
                .unwrap()
                .pane_manager
                .is_none()
        );

        let source_tab = live_source.tabs.as_mut().unwrap().active_tab_mut().unwrap();
        source_tab.live_domain = None;
        let focused = source_tab
            .pane_manager
            .as_mut()
            .unwrap()
            .focused_pane_mut()
            .unwrap();
        focused.launch = crate::workspace::WorkspaceLaunch::Profile { name: "dev".into() };
        let attached =
            Application::transfer_focused_pane(&mut live_source, &mut ordinary_destination)
                .unwrap();
        let snapshot = ordinary_destination
            .tabs
            .as_ref()
            .unwrap()
            .active_tab()
            .unwrap()
            .capture_workspace_tab(0, false)
            .unwrap();
        assert_eq!(
            snapshot
                .panes
                .iter()
                .find(|pane| pane.id == attached.0)
                .and_then(|pane| pane.launch.as_ref()),
            Some(&crate::workspace::WorkspaceLaunch::Profile { name: "dev".into() })
        );

        let (mut plugin_source, mut plugin_destination, _) = pane_transfer_fixture();
        let source_tab = plugin_source
            .tabs
            .as_mut()
            .unwrap()
            .active_tab_mut()
            .unwrap();
        source_tab.tab_type = crate::tabs::TabType::Plugin {
            plugin_id: "fake".into(),
        };
        source_tab
            .pane_manager
            .as_mut()
            .unwrap()
            .focused_pane_mut()
            .unwrap()
            .launch = crate::workspace::WorkspaceLaunch::Plugin {
            plugin_id: "fake".into(),
            profile: None,
        };
        let error = Application::transfer_focused_pane(&mut plugin_source, &mut plugin_destination)
            .unwrap_err()
            .to_string();
        assert!(error.contains("move the whole tab"), "{error}");
        assert_eq!(
            plugin_source
                .tabs
                .as_ref()
                .unwrap()
                .active_tab()
                .unwrap()
                .pane_manager
                .as_ref()
                .unwrap()
                .pane_count(),
            2
        );
        assert!(
            plugin_destination
                .tabs
                .as_ref()
                .unwrap()
                .active_tab()
                .unwrap()
                .pane_manager
                .is_none()
        );
    }

    #[cfg(unix)]
    #[test]
    fn whole_tab_transfer_preserves_plugin_panes_pinning_and_closes_empty_source() {
        if !std::path::Path::new("/bin/sh").is_file() {
            return;
        }
        let (mut source, mut destination, moved_child) = pane_transfer_fixture();
        let source_tab = source.tabs.as_mut().unwrap().active_tab_mut().unwrap();
        source_tab.tab_type = crate::tabs::TabType::Plugin {
            plugin_id: "fake-ai".into(),
        };
        source_tab.pinned = true;
        source_tab.set_user_title(Some("Pinned fake session".into()));

        let transferred = Application::transfer_active_tab(&mut source, &mut destination).unwrap();
        assert!(source.tabs.as_ref().unwrap().is_empty());
        assert!(source.should_exit);
        let tabs = destination.tabs.as_ref().unwrap();
        assert_eq!(tabs.count(), 2);
        assert_eq!(tabs.active_index(), 0);
        let tab = tabs.active_tab().unwrap();
        assert_eq!(tab.id, transferred);
        assert!(tab.pinned);
        assert_eq!(tab.user_title(), Some("Pinned fake session"));
        assert!(matches!(
            &tab.tab_type,
            crate::tabs::TabType::Plugin { plugin_id } if plugin_id == "fake-ai"
        ));
        let panes = tab.pane_manager.as_ref().unwrap();
        assert_eq!(panes.pane_count(), 2);
        assert!(
            panes
                .iter_with_layout()
                .any(|(pane, _)| pane.pty.child_id() == moved_child)
        );
        assert!(
            panes
                .iter_with_layout()
                .all(|(pane, _)| pane.geometry.rows == 30)
        );
        assert!(!tabs.iter().nth(1).unwrap().1.pinned);

        drop((source, destination));
        let (mut source, mut destination, _) = pane_transfer_fixture();
        source
            .tabs
            .as_mut()
            .unwrap()
            .new_tab_with_command(
                "/bin/sh",
                &["-c".into(), "sleep 5".into()],
                None,
                Some("survivor"),
                None,
                std::collections::HashMap::new(),
            )
            .unwrap();
        source.tabs.as_mut().unwrap().switch_to(0);
        Application::transfer_active_tab(&mut source, &mut destination).unwrap();
        assert!(!source.should_exit);
        assert_eq!(source.tabs.as_ref().unwrap().count(), 1);
        assert_eq!(
            source.tabs.as_ref().unwrap().active_tab().unwrap().title,
            "survivor"
        );
    }

    #[cfg(unix)]
    #[test]
    fn exact_control_tab_transfer_is_atomic_and_returns_destination_identity() {
        if !std::path::Path::new("/bin/sh").is_file() {
            return;
        }
        let (mut source, mut destination, moved_child) = pane_transfer_fixture();
        let error =
            Application::transfer_control_tab(&mut source, &mut destination, 0, 2).unwrap_err();
        assert!(error.contains("outside"), "{error}");
        assert_eq!(source.tabs.as_ref().unwrap().count(), 1);
        assert_eq!(destination.tabs.as_ref().unwrap().count(), 1);

        let (destination_tab_id, source_closing) =
            Application::transfer_control_tab(&mut source, &mut destination, 0, 1).unwrap();
        assert_eq!(destination_tab_id, 1);
        assert!(source_closing);
        assert!(source.should_exit);
        assert!(source.tabs.as_ref().unwrap().is_empty());
        let tabs = destination.tabs.as_ref().unwrap();
        assert_eq!(tabs.count(), 2);
        assert_eq!(tabs.active_index(), 1);
        let moved = tabs.active_tab().unwrap();
        assert_eq!(moved.id, destination_tab_id);
        assert!(
            moved
                .pane_manager
                .as_ref()
                .unwrap()
                .iter_with_layout()
                .any(|(pane, _)| pane.pty.child_id() == moved_child)
        );
    }

    #[cfg(unix)]
    #[test]
    fn whole_tab_transfer_rejects_live_source_atomically() {
        use crate::tabs::LiveDomainTab;

        if !std::path::Path::new("/bin/sh").is_file() {
            return;
        }
        let (mut source, mut destination, moved_child) = pane_transfer_fixture();
        let tab = source.tabs.as_mut().unwrap().active_tab_mut().unwrap();
        let pane_ids = tab
            .pane_manager
            .as_ref()
            .unwrap()
            .pane_ids()
            .into_iter()
            .enumerate()
            .map(|(index, id)| (id, u32::try_from(index + 1).unwrap()))
            .collect();
        tab.live_domain = Some(LiveDomainTab {
            domain_id: "domain".into(),
            tab_id: 4,
            pane_ids,
        });

        let error = Application::transfer_active_tab(&mut source, &mut destination)
            .unwrap_err()
            .to_string();
        assert!(error.contains("topology-owned"), "{error}");
        assert!(!source.should_exit);
        let source_panes = source
            .tabs
            .as_ref()
            .unwrap()
            .active_tab()
            .unwrap()
            .pane_manager
            .as_ref()
            .unwrap();
        assert_eq!(source_panes.pane_count(), 2);
        assert!(
            source_panes
                .iter_with_layout()
                .any(|(pane, _)| pane.pty.child_id() == moved_child)
        );
        assert_eq!(destination.tabs.as_ref().unwrap().count(), 1);
    }
}
