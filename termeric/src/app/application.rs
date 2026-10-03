//! Process-level ownership for independent native windows.

use super::{
    App, AppEvent, PendingPluginWindowClose, clear_agent_view_wake_pending, clear_pty_wake_pending,
    clear_transcript_wake_pending, redraw_scheduler::RedrawReason,
};
use crate::cli::{LaunchPlan, WindowLaunch};
use crate::config::Config;
use crate::plugin::permission::{Capability, PermissionScope, PluginPermissions};
use crate::plugin::structured::{StructuredHandlerInvocation, StructuredResponseAction};
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
    control_launches: crate::control::launches::LaunchLedger,
    pending_domain_controls: Vec<PendingDomainControl>,
    pending: Vec<App>,
    /// A tab dragged out of its window and released, waiting for the window
    /// the pointer enters next to claim it.
    tab_drop: Option<PendingTabDrop>,
    /// Most recent process-wide desktop preferences, applied to existing and
    /// newly created windows without persisting native state into config.
    system_accessibility: super::system_accessibility::SystemAccessibilityPreferences,
    system_accessibility_monitor_started: bool,
    /// Preserved when every requested startup window fails before becoming
    /// visible. Winit's handler API cannot return this error directly.
    fatal_startup_error: Option<String>,
}

/// A tab released outside its window. On Wayland no window knows where the
/// others are, and the source window holds the pointer until the release;
/// the window the pointer enters right after the release is where it was
/// dropped. When none claims it in time, it was dropped outside Termeric
/// and becomes a window of its own.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct PendingTabDrop {
    source: WindowId,
    tab_id: usize,
    deadline: Instant,
}

/// How long after a release another window may claim the dropped tab.
const TAB_DROP_CLAIM: std::time::Duration = std::time::Duration::from_millis(400);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct CreatedWindow {
    window_id: u64,
    initial_tab_id: u64,
}

const MAX_CONTROL_REPLAYS: usize = 256;
const MAX_PENDING_DOMAIN_CONTROLS: usize = 32;
const MAX_PENDING_DOMAIN_WAITERS: usize = 16;

fn process_tab_count(windows: &HashMap<WindowId, App>) -> usize {
    windows.values().fold(0, |total, window| {
        total.saturating_add(
            window
                .tabs
                .as_ref()
                .map_or(0, crate::tabs::TabManager::count),
        )
    })
}

fn process_pane_count(windows: &HashMap<WindowId, App>) -> usize {
    windows
        .values()
        .filter_map(|window| window.tabs.as_ref())
        .flat_map(|tabs| tabs.all_tabs())
        .map(|tab| {
            tab.pane_manager
                .as_ref()
                .map_or(1, crate::panes::PaneManager::pane_count)
        })
        .sum()
}

/// The window whose event started a plugin handler conversation.
#[derive(Clone, Copy, Debug)]
struct PluginOrigin {
    native_id: WindowId,
    window_id: u64,
}

fn validate_process_tab_creation_capacity(total_tabs: usize) -> std::result::Result<(), String> {
    if total_tabs >= crate::control::protocol::MAX_TOTAL_TABS {
        return Err(format!(
            "GUI process tab limit of {} reached",
            crate::control::protocol::MAX_TOTAL_TABS
        ));
    }
    Ok(())
}

fn validate_control_window_creation_capacity(
    live_windows: usize,
    pending_windows: usize,
    total_tabs: usize,
) -> std::result::Result<(), String> {
    if live_windows.saturating_add(pending_windows) >= crate::control::protocol::MAX_WINDOWS {
        return Err(format!(
            "GUI process window limit of {} reached",
            crate::control::protocol::MAX_WINDOWS
        ));
    }
    validate_process_tab_creation_capacity(total_tabs)
}

fn validate_process_pane_creation_capacity(total_panes: usize) -> std::result::Result<(), String> {
    if total_panes >= crate::control::protocol::MAX_TOTAL_PANES {
        return Err(format!(
            "GUI process pane limit of {} reached",
            crate::control::protocol::MAX_TOTAL_PANES
        ));
    }
    Ok(())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum NativeWindowFocusDisposition {
    AlreadyFocused,
    Request,
}

fn native_window_focus_disposition(
    window_id: u64,
    already_focused: bool,
    wayland: bool,
    visible: Option<bool>,
    minimized: Option<bool>,
) -> std::result::Result<NativeWindowFocusDisposition, String> {
    if already_focused {
        return Ok(NativeWindowFocusDisposition::AlreadyFocused);
    }
    if wayland {
        return Err(
            "The Wayland compositor controls native window focus; activate this window through the compositor"
                .into(),
        );
    }
    if visible == Some(false) {
        return Err(format!("GUI window {window_id} is not visible"));
    }
    if minimized == Some(true) {
        return Err(format!("GUI window {window_id} is minimized"));
    }
    Ok(NativeWindowFocusDisposition::Request)
}

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
            control_launches: crate::control::launches::LaunchLedger::default(),
            pending_domain_controls: Vec::new(),
            pending,
            tab_drop: None,
            system_accessibility:
                super::system_accessibility::SystemAccessibilityPreferences::default(),
            system_accessibility_monitor_started: false,
            fatal_startup_error: None,
        })
    }

    fn create_pending_windows(
        &mut self,
        event_loop: &ActiveEventLoop,
    ) -> Vec<std::result::Result<CreatedWindow, String>> {
        self.start_pending_windows(|window| window.resumed(event_loop))
    }

    /// Start every pending window with `start` (native window creation) and
    /// register each one that became usable.
    fn start_pending_windows(
        &mut self,
        mut start: impl FnMut(&mut App),
    ) -> Vec<std::result::Result<CreatedWindow, String>> {
        let mut startup_errors = Vec::new();
        let mut results = Vec::with_capacity(self.pending.len());
        for mut window in self.pending.drain(..) {
            // Starting a window already hands its stable ID to plugins (in
            // app-ready and in restored plugin panes' launch context), so the
            // ID is assigned first. A window that fails to start keeps its ID
            // out of circulation rather than lending it to the next one.
            window.control_window_id = self.next_control_window_id;
            self.next_control_window_id = self.next_control_window_id.saturating_add(1);
            start(&mut window);
            if window.startup_error.is_none()
                && !window.should_exit
                && let Some(id) = window.window.as_ref().map(|native| native.id())
                && let Some(initial_tab_id) = window
                    .tabs
                    .as_ref()
                    .and_then(crate::tabs::TabManager::active_tab_id)
                    .and_then(|tab_id| u64::try_from(tab_id).ok())
                    // A window opened for a torn-off tab is empty until the
                    // tab moves in, right after this.
                    .or(window.launch.adopts_tab.then_some(0))
            {
                let control_id = window.control_window_id;
                self.window_order.push(id);
                self.control_window_ids.insert(id, control_id);
                self.windows.insert(id, window);
                results.push(Ok(CreatedWindow {
                    window_id: control_id,
                    initial_tab_id,
                }));
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

    /// Resolve the window a plugin host operation targets. An omitted window
    /// ID means the origin window; any other window needs `capability`
    /// granted with all-windows scope. Scope is checked before the window is
    /// looked up, so a window-scoped plugin learns nothing about other windows.
    fn plugin_target_window(
        &self,
        origin: PluginOrigin,
        grants: &PluginPermissions,
        capability: Capability,
        requested: Option<u64>,
    ) -> std::result::Result<WindowId, String> {
        let window_id = requested.unwrap_or(origin.window_id);
        let native_id = if window_id == origin.window_id {
            origin.native_id
        } else {
            if grants.scope(capability) != Some(PermissionScope::AllWindows) {
                return Err(format!(
                    "window {window_id} is not the plugin's own window; targeting it requires {capability}:all-windows"
                ));
            }
            self.control_native_window_id(window_id)?
        };
        self.windows
            .get(&native_id)
            .filter(|window| !window.should_exit)
            .ok_or_else(|| format!("GUI window {window_id} is closing"))?;
        Ok(native_id)
    }

    /// Execute one handler query or host action for `origin` and encode the
    /// result event its handler receives next. A plugin window closure also
    /// returns the window to close once that result has been submitted.
    fn plugin_host_operation_event(
        &mut self,
        plugin_id: &str,
        origin: PluginOrigin,
        grants: &PluginPermissions,
        operation: &StructuredResponseAction,
    ) -> std::result::Result<(String, Option<WindowId>), String> {
        match operation {
            StructuredResponseAction::QueryWorkspace { query_id } => {
                let inventory = match grants.scope(Capability::WorkspaceQuery) {
                    Some(PermissionScope::AllWindows) => self.plugin_process_inventory(),
                    _ => {
                        let origin_window = self
                            .windows
                            .get(&origin.native_id)
                            .ok_or_else(|| "originating window is gone".to_string())?;
                        build_plugin_process_inventory(&[(origin.window_id, origin_window)])
                    }
                };
                let event = super::plugin_actions::plugin_workspace_query_event(
                    plugin_id,
                    origin.window_id,
                    query_id,
                    &inventory,
                )?;
                Ok((event, None))
            }
            StructuredResponseAction::QueryTerminal {
                window_id,
                tab_id,
                pane_id,
                history_lines,
                ..
            } => {
                let target =
                    self.plugin_target_window(origin, grants, Capability::TerminalRead, *window_id);
                let event = super::plugin_actions::plugin_terminal_query_event(
                    plugin_id,
                    origin.window_id,
                    operation,
                    |max_bytes| {
                        let native_id = target.clone()?;
                        self.windows
                            .get(&native_id)
                            .ok_or_else(|| "target window is gone".to_string())?
                            .read_control_text(*tab_id, *pane_id, *history_lines, max_bytes)
                    },
                )?;
                Ok((event, None))
            }
            StructuredResponseAction::CloseWindow { window_id, .. } => {
                let outcome = self.validate_plugin_window_close(origin, grants, *window_id);
                let close_target = outcome.as_ref().ok().map(|(native_id, _)| *native_id);
                let event = super::plugin_actions::plugin_action_result_event(
                    plugin_id,
                    origin.window_id,
                    operation,
                    outcome.map(|(_, outcome)| outcome),
                )?;
                Ok((event, close_target))
            }
            action => {
                let outcome = self.run_plugin_host_action(origin, grants, action);
                let event = super::plugin_actions::plugin_action_result_event(
                    plugin_id,
                    origin.window_id,
                    action,
                    outcome,
                )?;
                Ok((event, None))
            }
        }
    }

    /// Run one host action and return its outcome fields.
    fn run_plugin_host_action(
        &mut self,
        origin: PluginOrigin,
        grants: &PluginPermissions,
        action: &StructuredResponseAction,
    ) -> std::result::Result<serde_json::Value, String> {
        let capability = action.required_capability();
        match action {
            StructuredResponseAction::MoveTab {
                action_id,
                source_window_id,
                tab_id,
                destination_window_id,
                destination_index,
            } => {
                let source =
                    self.plugin_target_window(origin, grants, capability, *source_window_id)?;
                let destination =
                    self.plugin_target_window(origin, grants, capability, *destination_window_id)?;
                let source_id = source_window_id.unwrap_or(origin.window_id);
                let destination_id = destination_window_id.unwrap_or(origin.window_id);
                if source != destination {
                    self.validate_plugin_tab_move_source(source, *tab_id)?;
                }
                match self.move_control_tab(
                    action_id,
                    source_id,
                    *tab_id,
                    destination_id,
                    *destination_index,
                )? {
                    crate::control::protocol::Response::TabMoved {
                        destination_tab_id,
                        source_window_closing,
                        changed,
                        ..
                    } => Ok(serde_json::json!({
                        "destination_tab_id": destination_tab_id,
                        "source_window_closing": source_window_closing,
                        "changed": changed,
                    })),
                    _ => Err("tab movement returned an unexpected result".into()),
                }
            }
            StructuredResponseAction::MovePane {
                action_id,
                source_window_id,
                source_tab_id,
                pane_id,
                destination_window_id,
                destination_tab_id,
                target_pane_id,
                direction,
            } => {
                self.plugin_target_window(origin, grants, capability, *source_window_id)?;
                self.plugin_target_window(origin, grants, capability, *destination_window_id)?;
                match self.move_control_pane(
                    action_id,
                    source_window_id.unwrap_or(origin.window_id),
                    *source_tab_id,
                    *pane_id,
                    destination_window_id.unwrap_or(origin.window_id),
                    *destination_tab_id,
                    *target_pane_id,
                    *direction,
                )? {
                    crate::control::protocol::Response::PaneMoved {
                        destination_pane_id,
                        ..
                    } => Ok(serde_json::json!({ "destination_pane_id": destination_pane_id })),
                    _ => Err("pane movement returned an unexpected result".into()),
                }
            }
            StructuredResponseAction::FocusWindow {
                action_id,
                window_id,
            } => match self.focus_control_window(action_id, *window_id)? {
                crate::control::protocol::Response::WindowFocusRequested {
                    already_focused,
                    ..
                } => Ok(serde_json::json!({
                    "already_focused": already_focused,
                    "requested": !already_focused,
                })),
                _ => Err("native window focus returned an unexpected result".into()),
            },
            StructuredResponseAction::CreateTab { window_id, .. } => {
                let target = self.plugin_target_window(origin, grants, capability, *window_id)?;
                validate_process_tab_creation_capacity(process_tab_count(&self.windows))?;
                self.plugin_window_mut(target)
                    .run_plugin_window_action(action)
            }
            StructuredResponseAction::CreatePane { window_id, .. } => {
                let target = self.plugin_target_window(origin, grants, capability, *window_id)?;
                validate_process_pane_creation_capacity(process_pane_count(&self.windows))?;
                self.plugin_window_mut(target)
                    .run_plugin_window_action(action)
            }
            StructuredResponseAction::SelectTab { window_id, .. }
            | StructuredResponseAction::FocusPane { window_id, .. }
            | StructuredResponseAction::SendText { window_id, .. }
            | StructuredResponseAction::CloseTab { window_id, .. }
            | StructuredResponseAction::ClosePane { window_id, .. }
            | StructuredResponseAction::SwapPanes { window_id, .. }
            | StructuredResponseAction::BalancePanes { window_id, .. }
            | StructuredResponseAction::ApplyPaneLayout { window_id, .. }
            | StructuredResponseAction::ResizeDivider { window_id, .. } => {
                let target = self.plugin_target_window(origin, grants, capability, *window_id)?;
                self.plugin_window_mut(target)
                    .run_plugin_window_action(action)
            }
            StructuredResponseAction::ShowMessage { .. }
            | StructuredResponseAction::ShowNotice { .. }
            | StructuredResponseAction::QueryWorkspace { .. }
            | StructuredResponseAction::QueryTerminal { .. }
            | StructuredResponseAction::CloseWindow { .. } => {
                Err(format!("{} is not a host action", action.name()))
            }
        }
    }

    fn plugin_window_mut(&mut self, native_id: WindowId) -> &mut App {
        self.windows
            .get_mut(&native_id)
            .expect("resolved plugin target window remains registered")
    }

    /// A cross-window tab move may not empty its source window; closing a
    /// window needs the separate window-close capability.
    fn validate_plugin_tab_move_source(
        &self,
        source: WindowId,
        tab_id: u64,
    ) -> std::result::Result<(), String> {
        let tabs = self
            .windows
            .get(&source)
            .and_then(|window| window.tabs.as_ref())
            .ok_or_else(|| "Source window has no initialized tab manager".to_string())?;
        let tab_id = usize::try_from(tab_id)
            .map_err(|_| "Source tab ID does not fit this platform".to_string())?;
        let tab = tabs
            .tab_by_id(tab_id)
            .ok_or_else(|| format!("GUI tab {tab_id} does not exist in the source window"))?;
        if tab.closed || tab.is_closing() {
            return Err(format!("GUI tab {tab_id} is closing"));
        }
        let non_closing_tabs = tabs
            .all_tabs()
            .iter()
            .filter(|tab| !tab.closed && !tab.is_closing())
            .count();
        if non_closing_tabs <= 1 {
            return Err(format!(
                "Cannot move the final non-closing tab {tab_id}; closing its window requires window-close"
            ));
        }
        Ok(())
    }

    /// Validate a plugin window closure and return the window to close with
    /// the result's outcome fields. Window scope closes only the origin window,
    /// from an invoked command, and may close the last window; all-windows
    /// scope closes any window except the last live one.
    fn validate_plugin_window_close(
        &self,
        origin: PluginOrigin,
        grants: &PluginPermissions,
        window_id: Option<u64>,
    ) -> std::result::Result<(WindowId, serde_json::Value), String> {
        let target =
            self.plugin_target_window(origin, grants, Capability::WindowClose, window_id)?;
        let tab_count = self
            .windows
            .get(&target)
            .expect("resolved plugin target window remains registered")
            .validate_plugin_window_close()?;
        let live_windows = self
            .windows
            .values()
            .filter(|window| !window.should_exit && window.pending_plugin_window_close.is_none())
            .count();
        if grants.scope(Capability::WindowClose) == Some(PermissionScope::AllWindows)
            && live_windows <= 1
        {
            return Err(
                "Cannot close the final live window with window-close:all-windows; only a command-scoped window-close of the plugin's own window may"
                    .into(),
            );
        }
        let remaining_windows = u16::try_from(live_windows.saturating_sub(1))
            .map_err(|_| "live window count is unsupported".to_string())?;
        Ok((
            target,
            serde_json::json!({
                "scheduled": true,
                "tab_count": tab_count,
                "remaining_windows": remaining_windows,
            }),
        ))
    }

    /// Close a validated plugin target once its result has been submitted:
    /// the origin window waits for that exact result callback, while any other
    /// window closes immediately.
    fn arm_plugin_window_close(
        &mut self,
        target_native_id: WindowId,
        origin_native_id: WindowId,
        plugin_id: &str,
        plugin_generation: u64,
        result_request_id: u64,
    ) {
        let target = self.plugin_window_mut(target_native_id);
        debug_assert!(!target.should_exit);
        debug_assert!(target.pending_plugin_window_close.is_none());
        if target_native_id == origin_native_id {
            target.pending_plugin_window_close = Some(PendingPluginWindowClose {
                plugin_id: plugin_id.to_string(),
                plugin_generation,
                result_request_id,
            });
        } else {
            target.should_exit = true;
        }
    }

    fn handle_structured_plugin_completed(
        &mut self,
        completion: crate::plugin::structured::StructuredHandlerCompletion,
    ) {
        let origin_native_id = completion.window_id;
        let plugin_generation = completion.plugin_generation;
        let Some(origin_window) = self.windows.get_mut(&origin_native_id) else {
            return;
        };
        let Some(host) = origin_window.accept_structured_plugin_completion(completion) else {
            return;
        };
        if origin_window.should_exit {
            log::debug!(
                "Discarding {} from plugin {:?}: originating window is closing",
                host.operation.name(),
                host.plugin_id
            );
            return;
        }
        let Some(origin_window_id) = self.control_window_ids.get(&origin_native_id).copied() else {
            log::warn!(
                "Discarding {} from plugin {:?}: originating window has no stable ID",
                host.operation.name(),
                host.plugin_id
            );
            return;
        };
        let origin = PluginOrigin {
            native_id: origin_native_id,
            window_id: origin_window_id,
        };
        let super::plugin_actions::StructuredHostOperation {
            plugin_id,
            operation,
            continuation,
        } = host;
        let result =
            self.plugin_host_operation_continuation(&plugin_id, origin, &operation, continuation);
        let Some(origin_window) = self.windows.get_mut(&origin_native_id) else {
            return;
        };
        let submitted = result.and_then(|(continuation, close_target)| {
            origin_window
                .submit_structured_handler(continuation)
                .map(|request_id| (request_id, close_target))
        });
        match submitted {
            Ok((request_id, Some(close_target))) => self.arm_plugin_window_close(
                close_target,
                origin_native_id,
                &plugin_id,
                plugin_generation,
                request_id,
            ),
            Ok((_, None)) => {}
            Err(error) => {
                let name = operation.name();
                log::error!("Structured plugin {plugin_id:?} {name} failed: {error}");
                origin_window
                    .toasts
                    .error(format!("Plugin {plugin_id} {name} failed: {error}"));
            }
        }
    }

    /// Execute a handler's host operation for `origin` and turn the handler's
    /// `continuation` into the invocation that delivers the result, under the
    /// authority the follow-up rule leaves it. A plugin window closure also
    /// returns the window to close once that result has been submitted.
    fn plugin_host_operation_continuation(
        &mut self,
        plugin_id: &str,
        origin: PluginOrigin,
        operation: &StructuredResponseAction,
        mut continuation: StructuredHandlerInvocation,
    ) -> std::result::Result<(StructuredHandlerInvocation, Option<WindowId>), String> {
        let (event_json, close_target) = self.plugin_host_operation_event(
            plugin_id,
            origin,
            continuation.authority.grants(),
            operation,
        )?;
        continuation.authority = continuation.authority.after(operation);
        continuation.event_json = event_json;
        Ok((continuation, close_target))
    }

    fn handle_control_request(
        &mut self,
        event_loop: &ActiveEventLoop,
        mut request: crate::control::protocol::Request,
        peer_process: Option<u32>,
    ) -> std::result::Result<crate::control::protocol::Response, String> {
        use crate::control::protocol::{Request, Response};

        request.validate()?;
        request.canonicalize();
        if let Some(replay) = self.control_replays.replay(&request) {
            return replay;
        }
        if let Request::LaunchStatus { launch_request_id } = &request {
            let (state, terminal_closed) = self.control_launches.query(launch_request_id)?;
            return Ok(Response::LaunchStatus {
                process_id: std::process::id(),
                launch_request_id: launch_request_id.clone(),
                state,
                terminal_closed,
            });
        }
        if request
            .request_id()
            .is_some_and(|id| self.control_launches.contains(id))
        {
            return Err("Launch request ID remains retained but its mutation replay expired; query launch-status or use a new request ID".into());
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
        if let Request::ListAgents { supervisor } = &request {
            let mut claimed = supervisor.is_none();
            let mut agents = Vec::new();
            for (native_id, control_id) in &self.control_window_ids {
                let Some(window) = self
                    .windows
                    .get(native_id)
                    .filter(|window| !window.should_exit)
                else {
                    continue;
                };
                match supervisor {
                    None => agents.extend(window.control_agents(*control_id)),
                    Some(token) if window.owns_supervisor_token(token) => {
                        claimed = true;
                        agents.extend(window.supervised_agents(*control_id));
                    }
                    Some(_) => {}
                }
            }
            if !claimed {
                return Err("That supervisor is no longer running".into());
            }
            agents.sort_by_key(|agent| (agent.window_id, agent.tab_id, agent.pane_id));
            let truncated = crate::control::protocol::fit_agent_summaries(&mut agents);
            return Ok(Response::Agents {
                process_id: std::process::id(),
                agents,
                truncated,
            });
        }
        if let Request::ReadConversation {
            window_id,
            session_id,
        } = request
        {
            let native_id = self.control_native_window_id(window_id)?;
            let (text, truncated) = self
                .windows
                .get(&native_id)
                .filter(|window| !window.should_exit)
                .ok_or_else(|| format!("GUI window {window_id} is closing"))?
                .read_agent_view_conversation(session_id)?;
            return Ok(Response::Conversation {
                process_id: std::process::id(),
                window_id,
                session_id,
                text,
                truncated,
            });
        }
        if let Request::AgentEvent {
            terminal_id,
            agent_id,
            event,
            detail,
            last_message,
            transcript_path,
        } = request
        {
            let report = crate::agents::AgentReport::from_wire(
                agent_id,
                event,
                detail,
                last_message,
                transcript_path,
            );
            let owned = self.windows.values_mut().any(|window| {
                window.record_agent_report(&terminal_id, report.clone(), peer_process)
            });
            self.remove_closed_windows(event_loop);
            return if owned {
                Ok(Response::AgentEventRecorded {
                    process_id: std::process::id(),
                })
            } else {
                Err("No terminal in this process has that identity".into())
            };
        }

        let result = match &request {
            Request::CreateWindow {
                request_id,
                source_window,
            } => self.create_control_window(event_loop, request_id, *source_window),
            Request::JoinLaunch {
                request_id,
                display,
                activation_token,
                build,
                tabs,
            } => self.join_launch(
                event_loop,
                request_id,
                display,
                build,
                activation_token.clone(),
                tabs,
            ),
            Request::LaunchWindow {
                request_id,
                source_window,
                program,
                args,
                working_directory,
                title,
                environment,
                environment_policy,
            } => self.launch_control_window(
                event_loop,
                request_id,
                *source_window,
                program,
                args,
                working_directory.as_deref(),
                title.as_deref(),
                environment,
                environment_policy,
            ),
            Request::CreateTab {
                request_id,
                window_id,
            } => self.create_control_tab(request_id, *window_id),
            Request::LaunchTab {
                background,
                request_id,
                window_id,
                program,
                args,
                working_directory,
                title,
                environment,
                environment_policy,
            } => self.launch_control_tab(
                *background,
                request_id,
                *window_id,
                program,
                args,
                working_directory.as_deref(),
                title.as_deref(),
                environment,
                environment_policy,
            ),
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
            Request::SubmitToAgent {
                request_id,
                window_id,
                tab_id,
                pane_id,
                agent_id,
                text,
                supervisor,
            } => self.submit_control_text_to_agent(
                request_id,
                *window_id,
                *tab_id,
                *pane_id,
                agent_id,
                text,
                supervisor.as_deref(),
                peer_process,
            ),
            Request::AnswerApproval {
                request_id,
                window_id,
                session_id,
                approval_id,
                allow,
                reason,
                supervisor,
            } => self.answer_control_approval(
                request_id,
                *window_id,
                *session_id,
                approval_id,
                *allow,
                reason,
                supervisor.as_deref(),
                peer_process,
            ),
            Request::SubmitToSession {
                request_id,
                window_id,
                session_id,
                text,
                supervisor,
            } => self.submit_control_text_to_session(
                request_id,
                *window_id,
                *session_id,
                text,
                supervisor.as_deref(),
                peer_process,
            ),
            Request::Inventory
            | Request::ReadText { .. }
            | Request::LaunchStatus { .. }
            | Request::ListAgents { .. }
            | Request::ReadConversation { .. }
            | Request::AgentEvent { .. } => {
                unreachable!("read-only and agent requests returned before mutation dispatch")
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
        if self.control_launches.contains(request_id) {
            let _ = responder.send(Err("Launch request ID remains retained but its mutation replay expired; query launch-status or use a new request ID".into()));
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
        let total_tabs = process_tab_count(&self.windows);
        validate_control_window_creation_capacity(
            self.windows.len(),
            self.pending.len(),
            total_tabs,
        )?;
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
        let window_id = creation?.window_id;
        Ok(crate::control::protocol::Response::WindowCreated {
            process_id: std::process::id(),
            request_id: request_id.to_string(),
            window_id,
        })
    }

    /// Opens the window a new launch asked for, so it shares this process
    /// with the windows already open.
    fn join_launch(
        &mut self,
        event_loop: &ActiveEventLoop,
        request_id: &str,
        display: &str,
        build: &str,
        activation_token: Option<String>,
        tabs: &[crate::control::protocol::JoinTab],
    ) -> std::result::Result<crate::control::protocol::Response, String> {
        // Only the same build joins: a rebuilt or different Termeric runs
        // its own code in a process of its own.
        if crate::control::build_identity().as_deref() != Some(build) {
            return Err("This Termeric is a different build".into());
        }
        let result_index = self.prepare_join_launch(
            crate::control::display_identity().as_deref(),
            display,
            activation_token,
            tabs,
        )?;
        let creation = self
            .create_pending_windows(event_loop)
            .into_iter()
            .nth(result_index)
            .ok_or_else(|| "New window creation produced no result".to_string())?;
        let window_id = creation?.window_id;
        Ok(crate::control::protocol::Response::WindowCreated {
            process_id: std::process::id(),
            request_id: request_id.to_string(),
            window_id,
        })
    }

    /// Queues the window a joining launch asked for, when it runs on this
    /// process's display (`own_display`). Returns its place among the
    /// windows waiting to be created.
    fn prepare_join_launch(
        &mut self,
        own_display: Option<&str>,
        display: &str,
        activation_token: Option<String>,
        tabs: &[crate::control::protocol::JoinTab],
    ) -> std::result::Result<usize, String> {
        // Windows of one process share its display connection.
        if own_display != Some(display) {
            return Err("This Termeric runs on another display".into());
        }
        let total_tabs = process_tab_count(&self.windows);
        validate_control_window_creation_capacity(
            self.windows.len(),
            self.pending.len(),
            total_tabs,
        )?;
        // Settings are the process's, as for any window it opens.
        let config = self
            .window_order
            .iter()
            .find_map(|native_id| self.windows.get(native_id))
            .map(|window| window.config.clone())
            .ok_or("This Termeric has no open window to join")?;
        let mut launch = WindowLaunch {
            force_new: true,
            tabs: tabs
                .iter()
                .map(|tab| crate::cli::TabLaunch {
                    working_directory: tab.working_directory.as_ref().map(Into::into),
                    title: tab.title.clone(),
                    target: match &tab.program {
                        Some(program) => crate::cli::LaunchTarget::Command {
                            program: program.clone(),
                            args: tab.args.clone(),
                        },
                        None => crate::cli::LaunchTarget::Shell,
                    },
                    ..crate::cli::TabLaunch::default()
                })
                .collect(),
            ..WindowLaunch::default()
        };
        if launch.tabs.is_empty() {
            launch.tabs.push(crate::cli::TabLaunch::default());
        }
        let result_index = self.pending.len();
        self.queue_window(config, launch)
            .map_err(|error| format!("Could not prepare a new window: {error:#}"))?;
        if let Some(window) = self.pending.last_mut() {
            window.activation_token = activation_token;
        }
        Ok(result_index)
    }

    #[allow(clippy::too_many_arguments)]
    fn launch_control_window(
        &mut self,
        event_loop: &ActiveEventLoop,
        request_id: &str,
        source_window: u64,
        program: &str,
        args: &[String],
        working_directory: Option<&str>,
        title: Option<&str>,
        environment: &[crate::control::protocol::EnvironmentVariable],
        environment_policy: &termeric_core::pty::EnvironmentPolicy,
    ) -> std::result::Result<crate::control::protocol::Response, String> {
        self.control_launches.prepare(request_id)?;
        let result_index = self.prepare_control_launch_window(
            source_window,
            program,
            args,
            working_directory,
            title,
            environment,
            environment_policy,
        )?;
        let creation = self
            .create_pending_windows(event_loop)
            .into_iter()
            .nth(result_index)
            .ok_or_else(|| "New command window creation produced no result".to_string())??;
        self.record_control_launch(request_id, creation.window_id, creation.initial_tab_id);
        Ok(crate::control::protocol::Response::WindowLaunched {
            process_id: std::process::id(),
            request_id: request_id.to_string(),
            window_id: creation.window_id,
            tab_id: creation.initial_tab_id,
        })
    }

    fn record_control_launch(&mut self, request_id: &str, window_id: u64, tab_id: u64) {
        let native_id = self
            .control_window_ids
            .iter()
            .find_map(|(native, id)| (*id == window_id).then_some(native))
            .expect("successfully launched window has a control ID");
        let child = self
            .windows
            .get(native_id)
            .and_then(|window| window.tabs.as_ref())
            .and_then(|tabs| tabs.all_tabs().iter().find(|tab| tab.id as u64 == tab_id))
            .and_then(|tab| tab.child_observer())
            .expect("successfully launched command has an initial child");
        self.control_launches.record(request_id, child);
    }

    fn prepare_control_launch_window(
        &mut self,
        source_window: u64,
        program: &str,
        args: &[String],
        working_directory: Option<&str>,
        title: Option<&str>,
        environment: &[crate::control::protocol::EnvironmentVariable],
        environment_policy: &termeric_core::pty::EnvironmentPolicy,
    ) -> std::result::Result<usize, String> {
        let total_tabs = process_tab_count(&self.windows);
        validate_control_window_creation_capacity(
            self.windows.len(),
            self.pending.len(),
            total_tabs,
        )?;
        let source_native_id = self
            .control_window_ids
            .iter()
            .find_map(|(native_id, control_id)| {
                (*control_id == source_window).then_some(*native_id)
            })
            .ok_or_else(|| format!("GUI window {source_window} does not exist"))?;
        let (config, inherited_cwd) = {
            let source = self
                .windows
                .get(&source_native_id)
                .filter(|window| !window.should_exit)
                .ok_or_else(|| format!("GUI window {source_window} is closing"))?;
            let inherited_cwd = source
                .tabs
                .as_ref()
                .and_then(|tabs| tabs.active_tab())
                .and_then(|tab| tab.working_directory());
            (source.config.clone(), inherited_cwd)
        };
        let selected_cwd = match working_directory {
            Some(directory) => std::path::PathBuf::from(directory),
            None => inherited_cwd
                .map_or_else(std::env::current_dir, |directory| {
                    Ok(std::path::PathBuf::from(directory))
                })
                .map_err(|error| {
                    format!("Could not determine the GUI process working directory: {error}")
                })?,
        };
        if !selected_cwd.is_absolute() || !selected_cwd.is_dir() {
            return Err(
                "Selected command-window working directory is not an available local directory; pass an existing absolute directory with --cwd"
                    .into(),
            );
        }
        if selected_cwd.to_str().is_none() {
            return Err(
                "Selected command-window working directory is not valid UTF-8; pass an existing UTF-8 absolute directory with --cwd"
                    .into(),
            );
        }
        let launch = WindowLaunch {
            force_new: true,
            tabs: vec![crate::cli::TabLaunch {
                working_directory: Some(selected_cwd),
                title: title.map(str::to_string),
                target: crate::cli::LaunchTarget::Command {
                    program: program.to_string(),
                    args: args.to_vec(),
                },
                environment: environment
                    .iter()
                    .map(|entry| (entry.name.clone(), entry.value.clone()))
                    .collect(),
                environment_policy: environment_policy.clone(),
                splits: Vec::new(),
            }],
            ..WindowLaunch::default()
        };
        let result_index = self.pending.len();
        self.queue_window(config, launch)
            .map_err(|error| format!("Could not prepare a new command window: {error:#}"))?;
        self.pending[result_index].redact_startup_error_log = true;
        Ok(result_index)
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
        let total_tabs = process_tab_count(&self.windows);
        validate_process_tab_creation_capacity(total_tabs)?;
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

    #[allow(clippy::too_many_arguments)]
    fn launch_control_tab(
        &mut self,
        background: bool,
        request_id: &str,
        window_id: u64,
        program: &str,
        args: &[String],
        working_directory: Option<&str>,
        title: Option<&str>,
        environment: &[crate::control::protocol::EnvironmentVariable],
        environment_policy: &termeric_core::pty::EnvironmentPolicy,
    ) -> std::result::Result<crate::control::protocol::Response, String> {
        self.control_launches.prepare(request_id)?;
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
        let total_tabs = process_tab_count(&self.windows);
        validate_process_tab_creation_capacity(total_tabs)?;
        let tab_id = self
            .windows
            .get_mut(&native_id)
            .expect("validated control window disappeared")
            .launch_control_tab(
                background,
                program,
                args,
                working_directory,
                title,
                environment,
                environment_policy,
            )?;
        self.record_control_launch(request_id, window_id, tab_id);
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
        validate_process_pane_creation_capacity(process_pane_count(&self.windows))?;
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
        if native_window_focus_disposition(window_id, already_focused, false, None, None)?
            == NativeWindowFocusDisposition::AlreadyFocused
        {
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
        let disposition = native_window_focus_disposition(
            window_id,
            false,
            matches!(handle.as_raw(), RawWindowHandle::Wayland(_)),
            native.is_visible(),
            native.is_minimized(),
        )?;
        debug_assert_eq!(disposition, NativeWindowFocusDisposition::Request);
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
            .clone()
            .ok_or_else(|| format!("GUI window {window_id} has no native window"))?;
        window.quick_access_visible = visible;
        native.set_visible(visible);
        window.handle_window_occlusion_change(!visible);
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

        if let Some(refusal) = source.agent_view_tab_move_refusal(destination, tab_id) {
            return Err(format!("Could not move tab {tab_id}: {refusal}"));
        }
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
        // The tab's agent view conversation moves with it.
        App::move_agent_view_tab(source, destination, tab_id, destination_tab_id);

        source.broadcast = super::BroadcastState::Inactive;
        destination.broadcast = super::BroadcastState::Inactive;
        source.leave_copy_mode(false);
        destination.leave_copy_mode(false);
        source.terminal_mouse_capture = None;
        destination.terminal_mouse_capture = None;
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
        source.terminal_mouse_capture = None;
        destination.terminal_mouse_capture = None;
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

    #[allow(clippy::too_many_arguments)]
    fn submit_control_text_to_agent(
        &mut self,
        request_id: &str,
        window_id: u64,
        tab_id: u64,
        pane_id: u32,
        agent_id: &str,
        text: &str,
        supervisor: Option<&str>,
        peer_process: Option<u32>,
    ) -> std::result::Result<crate::control::protocol::Response, String> {
        let native_id = self.control_native_window_id(window_id)?;
        let pending_approval = self
            .windows
            .get_mut(&native_id)
            .filter(|window| !window.should_exit)
            .ok_or_else(|| format!("GUI window {window_id} is closing"))?
            .submit_control_text_to_agent(
                tab_id,
                pane_id,
                agent_id,
                text,
                supervisor,
                peer_process,
            )?;
        Ok(crate::control::protocol::Response::AgentInputAccepted {
            process_id: std::process::id(),
            request_id: request_id.to_string(),
            window_id,
            tab_id,
            pane_id,
            pending_approval,
        })
    }

    #[allow(clippy::too_many_arguments)]
    fn answer_control_approval(
        &mut self,
        request_id: &str,
        window_id: u64,
        session_id: u64,
        approval_id: &str,
        allow: bool,
        reason: &str,
        supervisor: Option<&str>,
        peer_process: Option<u32>,
    ) -> std::result::Result<crate::control::protocol::Response, String> {
        let native_id = self.control_native_window_id(window_id)?;
        let answered = self
            .windows
            .get_mut(&native_id)
            .filter(|window| !window.should_exit)
            .ok_or_else(|| format!("GUI window {window_id} is closing"))?
            .answer_agent_view_approval(
                session_id,
                approval_id,
                allow,
                reason,
                supervisor,
                peer_process,
            )?;
        Ok(crate::control::protocol::Response::ApprovalAnswered {
            process_id: std::process::id(),
            request_id: request_id.to_string(),
            window_id,
            session_id,
            answered,
        })
    }

    fn submit_control_text_to_session(
        &mut self,
        request_id: &str,
        window_id: u64,
        session_id: u64,
        text: &str,
        supervisor: Option<&str>,
        peer_process: Option<u32>,
    ) -> std::result::Result<crate::control::protocol::Response, String> {
        let native_id = self.control_native_window_id(window_id)?;
        let pending_approval = self
            .windows
            .get_mut(&native_id)
            .filter(|window| !window.should_exit)
            .ok_or_else(|| format!("GUI window {window_id} is closing"))?
            .submit_to_agent_view(session_id, text, supervisor, peer_process)?;
        Ok(crate::control::protocol::Response::SessionInputAccepted {
            process_id: std::process::id(),
            request_id: request_id.to_string(),
            window_id,
            session_id,
            pending_approval,
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
        source.terminal_mouse_capture = None;
        destination.terminal_mouse_capture = None;
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
        let moving = source
            .tabs
            .as_ref()
            .and_then(crate::tabs::TabManager::active_tab_id)
            .expect("the source tab was validated");
        if let Some(refusal) = source.agent_view_tab_move_refusal(destination, moving) {
            anyhow::bail!(refusal);
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
        // The tab's agent view conversation moves with it.
        App::move_agent_view_tab(source, destination, moving, id);

        source.broadcast = super::BroadcastState::Inactive;
        destination.broadcast = super::BroadcastState::Inactive;
        source.leave_copy_mode(false);
        destination.leave_copy_mode(false);
        source.terminal_mouse_capture = None;
        destination.terminal_mouse_capture = None;
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

    /// Moves a dropped tab into window `destination`, where the pointer is
    /// at `position`.
    fn drop_tab_into(
        &mut self,
        drop: PendingTabDrop,
        destination_id: WindowId,
        position: (f64, f64),
    ) {
        let Some(pinned) = self
            .windows
            .get(&drop.source)
            .and_then(|source| source.tabs.as_ref()?.tab_by_id(drop.tab_id))
            .map(|tab| tab.pinned)
        else {
            return;
        };
        let Some(mut destination) = self
            .windows
            .remove(&destination_id)
            .filter(|window| !window.should_exit)
        else {
            return;
        };
        let index = destination.tab_drop_index(position, pinned);
        let result = self
            .windows
            .get_mut(&drop.source)
            .ok_or_else(|| "The tab's window closed".to_string())
            .and_then(|source| {
                Self::transfer_control_tab(source, &mut destination, drop.tab_id as u64, index)
            });
        match result {
            Ok(_) => {
                if let Some(window) = &destination.window {
                    window.focus_window();
                }
            }
            Err(error) => destination
                .toasts
                .warning(format!("Tab was not moved: {error}")),
        }
        self.windows.insert(destination_id, destination);
    }

    /// Opens a window of its own for a tab dropped outside every Termeric
    /// window. A window's only tab stays: it already has a window.
    fn tear_off_tab(&mut self, drop: PendingTabDrop, event_loop: &ActiveEventLoop) {
        let Some(source) = self
            .windows
            .get(&drop.source)
            .filter(|window| !window.should_exit)
        else {
            return;
        };
        let alone = source
            .tabs
            .as_ref()
            .is_none_or(|tabs| tabs.iter().count() <= 1);
        if alone {
            return;
        }
        let config = source.config.clone();
        let launch = WindowLaunch {
            force_new: true,
            adopts_tab: true,
            ..WindowLaunch::default()
        };
        let result_index = self.pending.len();
        if let Err(error) = self.queue_window(config, launch) {
            log::error!("Could not prepare a window for the torn-off tab: {error:#}");
            return;
        }
        // The window exists before the tab leaves its old one, so a window
        // that cannot open loses nothing.
        let created = self
            .create_pending_windows(event_loop)
            .into_iter()
            .nth(result_index);
        let Some(Ok(created)) = created else {
            if let Some(source) = self.windows.get_mut(&drop.source) {
                source
                    .toasts
                    .warning("Tab was not moved: a new window could not open");
            }
            return;
        };
        let Some(destination_id) = self
            .control_window_ids
            .iter()
            .find_map(|(native, id)| (*id == created.window_id).then_some(*native))
        else {
            return;
        };
        let Some(mut destination) = self.windows.remove(&destination_id) else {
            return;
        };
        let result = self
            .windows
            .get_mut(&drop.source)
            .ok_or_else(|| "The tab's window closed".to_string())
            .and_then(|source| {
                Self::transfer_control_tab(source, &mut destination, drop.tab_id as u64, 0)
            });
        if let Err(error) = result {
            // Nothing moved in: the empty window has no reason to stay.
            destination.should_exit = true;
            if let Some(source) = self.windows.get_mut(&drop.source) {
                source.toasts.warning(format!("Tab was not moved: {error}"));
            }
        }
        self.windows.insert(destination_id, destination);
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

pub(super) fn build_plugin_process_inventory(
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
    // Only the windows asked for matter; starting a process of its own
    // still restores.
    if launch.windows != LaunchPlan::default().windows
        || !config.session.enabled
        || !config.session.auto_restore
    {
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
        separate_process: launch.separate_process,
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
        let mut dropped_outside = None;
        // The first pointer position another window hears after a release
        // outside the source is where the tab was dropped.
        if let WindowEvent::CursorMoved { position, .. } = &event
            && let Some(drop) = self.tab_drop
            && drop.source != window_id
        {
            self.tab_drop = None;
            self.drop_tab_into(drop, window_id, (position.x, position.y));
        }
        if let Some(window) = self.windows.get_mut(&window_id) {
            window.window_event(event);
            window.sync_tab_drag_pointer(event_loop);
            create_window = std::mem::take(&mut window.request_new_window);
            requested_window_launch = window.requested_window_launch.take();
            move_pane_to_next_window = std::mem::take(&mut window.request_move_pane_to_next_window);
            move_tab_to_next_window = std::mem::take(&mut window.request_move_tab_to_next_window);
            dropped_outside = window.tab_dropped_outside.take();
        }
        if let Some(tab_id) = dropped_outside {
            self.tab_drop = Some(PendingTabDrop {
                source: window_id,
                tab_id,
                deadline: Instant::now() + TAB_DROP_CLAIM,
            });
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
                mut request,
                peer_process,
                respond_to,
            } => {
                request.canonicalize();
                let Some(respond_to) = self.defer_live_domain_control(&request, respond_to) else {
                    return;
                };
                let result = self.handle_control_request(event_loop, request, peer_process);
                let _ = respond_to.send(result);
                return;
            }
            AppEvent::AccessKit(event) => {
                if let Some(window) = self.windows.get_mut(&event.window_id) {
                    window.redraw_scheduler.request(RedrawReason::Accessibility);
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
                    window
                        .redraw_scheduler
                        .request(RedrawReason::SurfaceRecovery);
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
                    window
                        .redraw_scheduler
                        .request(RedrawReason::BackgroundTask);
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
                    window
                        .redraw_scheduler
                        .request(RedrawReason::BackgroundTask);
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
                    window
                        .redraw_scheduler
                        .request(RedrawReason::BackgroundTask);
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
                    window
                        .redraw_scheduler
                        .request(RedrawReason::BackgroundTask);
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
                    window
                        .redraw_scheduler
                        .request(RedrawReason::BackgroundTask);
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
                    window
                        .redraw_scheduler
                        .request(RedrawReason::BackgroundTask);
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
                    window
                        .redraw_scheduler
                        .request(RedrawReason::BackgroundTask);
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
                    window
                        .redraw_scheduler
                        .request(RedrawReason::BackgroundTask);
                    window.handle_file_upload_progress(request_id, bytes_sent, total_bytes);
                }
                self.remove_closed_windows(event_loop);
                return;
            }
            AppEvent::KittyFileTransferCompleted {
                window_id,
                token,
                result,
            } => {
                if let Some(window) = self.windows.get_mut(&window_id) {
                    window
                        .redraw_scheduler
                        .request(RedrawReason::BackgroundTask);
                    window.handle_kitty_file_transfer_completed(token, result);
                }
                self.remove_closed_windows(event_loop);
                return;
            }
            AppEvent::KittyFileUploadCompleted {
                window_id,
                token,
                result,
            } => {
                if let Some(window) = self.windows.get_mut(&window_id) {
                    window
                        .redraw_scheduler
                        .request(RedrawReason::BackgroundTask);
                    window.handle_kitty_file_upload_completed(token, result);
                }
                self.remove_closed_windows(event_loop);
                return;
            }
            AppEvent::StructuredPluginCompleted(completion) => {
                self.handle_structured_plugin_completed(completion);
                self.remove_closed_windows(event_loop);
                return;
            }
            AppEvent::AgentView => {
                clear_agent_view_wake_pending();
                for window in self.windows.values_mut() {
                    window.process_agent_view_events();
                }
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
        if let Some(drop) = self.tab_drop {
            if now >= drop.deadline {
                self.tab_drop = None;
                self.tear_off_tab(drop, event_loop);
            } else {
                deadline = Some(drop.deadline);
            }
        }
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
    #[cfg(unix)]
    #[test]
    fn control_environment_policy_child_probe() {
        let Some(marker) = std::env::var_os("TERMERIC_TEST_CONTROL_ENV_POLICY_MARKER") else {
            return;
        };
        assert!(std::env::var_os("PATH").is_none());
        assert_eq!(std::env::var("EMPTY_VALUE").unwrap(), "");
        assert_eq!(std::env::var("EXACT_VALUE").unwrap(), "= literal $HOME;λ");
        assert_eq!(std::env::var("TERM_PROGRAM").unwrap(), "Termeric");
        assert_eq!(
            std::env::var_os("PWD").unwrap(),
            std::env::current_dir().unwrap().as_os_str()
        );
        std::fs::write(marker, b"environment policy applied").unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn control_environment_policy_reaches_tab_and_prepared_window_children() {
        use crate::control::protocol::{EnvironmentVariable, LaunchState};
        use termeric_core::pty::EnvironmentPolicy;
        let executable = std::env::current_exe().unwrap();
        let args = [
            "--exact".into(),
            "app::application::tests::control_environment_policy_child_probe".into(),
            "--nocapture".into(),
        ];
        for window_launch in [false, true] {
            for clear in [false, true] {
                let directory = tempfile::tempdir().unwrap();
                let marker = directory.path().join("observed");
                let policy = EnvironmentPolicy {
                    clear,
                    unset: if clear { vec![] } else { vec!["PATH".into()] },
                };
                let environment = [
                    EnvironmentVariable {
                        name: "TERMERIC_TEST_CONTROL_ENV_POLICY_MARKER".into(),
                        value: marker.to_str().unwrap().into(),
                    },
                    EnvironmentVariable {
                        name: "EMPTY_VALUE".into(),
                        value: String::new(),
                    },
                    EnvironmentVariable {
                        name: "EXACT_VALUE".into(),
                        value: "= literal $HOME;λ".into(),
                    },
                ];
                let mut application =
                    Application::from_config(Config::default(), None, LaunchPlan::default())
                        .unwrap();
                application.pending.clear();
                let native_id = WindowId::dummy();
                let mut window = App::new_headless(Config::default()).unwrap();
                window.tabs = Some(crate::tabs::TabManager::new(
                    80,
                    24,
                    Config::default().shell,
                    100,
                    1024 * 1024,
                ));
                application.windows.insert(native_id, window);
                application.control_window_ids.insert(native_id, 7);
                application.window_order.push(native_id);
                let request_id = "a".repeat(32);
                if window_launch {
                    let index = application
                        .prepare_control_launch_window(
                            7,
                            executable.to_str().unwrap(),
                            &args,
                            directory.path().to_str(),
                            None,
                            &environment,
                            &policy,
                        )
                        .unwrap();
                    let mut prepared = application.pending.remove(index);
                    assert_eq!(prepared.launch.tabs[0].environment_policy, policy);
                    prepared.init_tabs().unwrap();
                    // Native creation itself remains deferred; exercise its real
                    // prepared startup plan, then the same completion registrar.
                    application.windows.insert(native_id, prepared);
                    application.control_launches.prepare(&request_id).unwrap();
                    application.record_control_launch(&request_id, 7, 0);
                } else {
                    application
                        .launch_control_tab(
                            false,
                            &request_id,
                            7,
                            executable.to_str().unwrap(),
                            &args,
                            directory.path().to_str(),
                            None,
                            &environment,
                            &policy,
                        )
                        .unwrap();
                }
                let deadline = Instant::now() + std::time::Duration::from_secs(5);
                loop {
                    application
                        .windows
                        .get_mut(&native_id)
                        .unwrap()
                        .process_pty_events();
                    if let (LaunchState::Exited { exit_code, success }, _) =
                        application.control_launches.query(&request_id).unwrap()
                    {
                        assert_eq!(exit_code, 0, "controlled environment probe failed");
                        assert!(success);
                        break;
                    }
                    assert!(Instant::now() < deadline);
                    std::thread::sleep(std::time::Duration::from_millis(5));
                }
                assert_eq!(
                    std::fs::read(marker).unwrap(),
                    b"environment policy applied"
                );
            }
        }
    }
    use super::*;
    use crate::plugin::permission::PluginPermission;

    /// Every capability, scoped ones at `scope`.
    fn every_capability(scope: PermissionScope) -> PluginPermissions {
        PluginPermissions::from_permissions(Capability::ALL.into_iter().map(|capability| {
            PluginPermission {
                capability,
                scope: if capability.is_scoped() {
                    scope
                } else {
                    PermissionScope::Window
                },
            }
        }))
        .unwrap()
    }

    /// Every capability, scoped ones at all-windows scope.
    fn all_windows_grants() -> PluginPermissions {
        every_capability(PermissionScope::AllWindows)
    }

    /// Execute one plugin host operation for origin window `origin_window_id`,
    /// which need not be registered, and decode its result event.
    fn plugin_operation(
        application: &mut Application,
        plugin_id: &str,
        origin_window_id: u64,
        grants: &PluginPermissions,
        operation: StructuredResponseAction,
    ) -> (serde_json::Value, Option<WindowId>) {
        let native_id = application
            .control_native_window_id(origin_window_id)
            .unwrap_or_else(|_| WindowId::from(u64::MAX - origin_window_id));
        let origin = PluginOrigin {
            native_id,
            window_id: origin_window_id,
        };
        let (event, close_target) = application
            .plugin_host_operation_event(plugin_id, origin, grants, &operation)
            .unwrap();
        (serde_json::from_str(&event).unwrap(), close_target)
    }

    /// [`plugin_operation`] with every capability granted for all windows.
    fn plugin_event(
        application: &mut Application,
        plugin_id: &str,
        origin_window_id: u64,
        operation: StructuredResponseAction,
    ) -> serde_json::Value {
        plugin_operation(
            application,
            plugin_id,
            origin_window_id,
            &all_windows_grants(),
            operation,
        )
        .0
    }

    #[test]
    fn a_tab_dropped_on_another_window_moves_there_after_its_pinned_tabs() {
        let window = || {
            let mut app = App::new_headless(Config::default()).unwrap();
            let mut tabs =
                crate::tabs::TabManager::new(80, 24, Config::default().shell, 1_000, 1024 * 1024);
            tabs.new_tab().unwrap();
            tabs.new_tab().unwrap();
            app.tabs = Some(tabs);
            app
        };
        let mut application =
            Application::from_config(Config::default(), None, LaunchPlan::default()).unwrap();
        application.pending.clear();
        let (source_id, destination_id) = (WindowId::dummy(), WindowId::from(1u64));
        let source = window();
        let dragged = source.tabs.as_ref().unwrap().iter().nth(1).unwrap().1.id;
        let mut destination = window();
        destination.tabs.as_mut().unwrap().all_tabs_mut()[0].pinned = true;
        application.windows.insert(source_id, source);
        application.windows.insert(destination_id, destination);
        let drop = PendingTabDrop {
            source: source_id,
            tab_id: dragged,
            deadline: Instant::now() + TAB_DROP_CLAIM,
        };
        // Without a tab bar under the pointer, it goes to the end.
        application.drop_tab_into(drop, destination_id, (10.0, 400.0));
        let source_tabs = application.windows[&source_id].tabs.as_ref().unwrap();
        assert_eq!(source_tabs.count(), 1);
        let destination_tabs = application.windows[&destination_id].tabs.as_ref().unwrap();
        assert_eq!(destination_tabs.count(), 3);
        assert_eq!(
            destination_tabs.active_index(),
            2,
            "the dropped tab is shown"
        );
        assert!(!destination_tabs.active_tab().unwrap().pinned);

        // A tab that is gone by the time a window claims it moves nowhere.
        application.drop_tab_into(drop, destination_id, (10.0, 400.0));
        assert_eq!(
            application.windows[&destination_id]
                .tabs
                .as_ref()
                .unwrap()
                .count(),
            3
        );
    }

    #[test]
    fn a_joining_launch_opens_its_tabs_in_a_new_window_on_this_display() {
        use crate::control::protocol::JoinTab;
        let mut application =
            Application::from_config(Config::default(), None, LaunchPlan::default()).unwrap();
        application.pending.clear();
        let tabs = [
            JoinTab {
                working_directory: Some("/tmp".into()),
                title: Some("Logs".into()),
                program: None,
                args: Vec::new(),
            },
            JoinTab {
                working_directory: None,
                title: None,
                program: Some("htop".into()),
                args: vec!["-d".into(), "10".into()],
            },
        ];
        // Nothing to join yet.
        assert!(
            application
                .prepare_join_launch(Some("wayland:w-1"), "wayland:w-1", None, &tabs)
                .unwrap_err()
                .contains("no open window")
        );
        let native_id = WindowId::dummy();
        let mut open = App::new_headless(Config::default()).unwrap();
        open.config.window.opacity = 0.9;
        application.windows.insert(native_id, open);
        application.window_order.push(native_id);

        assert_eq!(
            application.prepare_join_launch(Some("wayland:w-1"), "wayland:w-0", None, &tabs),
            Err("This Termeric runs on another display".into())
        );
        let index = application
            .prepare_join_launch(
                Some("wayland:w-1"),
                "wayland:w-1",
                Some("token".into()),
                &tabs,
            )
            .unwrap();
        let window = &application.pending[index];
        assert_eq!(window.activation_token.as_deref(), Some("token"));
        assert!(
            !window.allow_auto_restore,
            "a joined window restores nothing"
        );
        assert_eq!(
            window.config.window.opacity, 0.9,
            "the process's settings apply"
        );
        let launched: Vec<_> = window
            .launch
            .tabs
            .iter()
            .map(|tab| {
                (
                    tab.working_directory.clone(),
                    tab.title.clone(),
                    tab.target.clone(),
                )
            })
            .collect();
        assert_eq!(
            launched,
            [
                (
                    Some("/tmp".into()),
                    Some("Logs".into()),
                    crate::cli::LaunchTarget::Shell
                ),
                (
                    None,
                    None,
                    crate::cli::LaunchTarget::Command {
                        program: "htop".into(),
                        args: vec!["-d".into(), "10".into()],
                    }
                ),
            ]
        );
    }

    #[test]
    fn native_window_focus_policy_is_truthful_and_non_restoring() {
        assert_eq!(
            native_window_focus_disposition(7, true, true, Some(false), Some(true)).unwrap(),
            NativeWindowFocusDisposition::AlreadyFocused
        );
        assert_eq!(
            native_window_focus_disposition(7, false, false, Some(true), Some(false)).unwrap(),
            NativeWindowFocusDisposition::Request
        );
        assert!(
            native_window_focus_disposition(7, false, true, Some(true), Some(false))
                .unwrap_err()
                .contains("Wayland compositor")
        );
        assert_eq!(
            native_window_focus_disposition(7, false, false, Some(false), Some(false)).unwrap_err(),
            "GUI window 7 is not visible"
        );
        assert_eq!(
            native_window_focus_disposition(7, false, false, Some(true), Some(true)).unwrap_err(),
            "GUI window 7 is minimized"
        );
    }

    #[test]
    fn quick_access_launch_is_excluded_from_automatic_workspace_persistence() {
        let launch = LaunchPlan {
            windows: vec![WindowLaunch {
                force_new: true,
                quick_access: true,
                ..WindowLaunch::default()
            }],
            separate_process: false,
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

        let launch_request = Request::LaunchTab {
            background: false,
            request_id: "2".repeat(32),
            window_id: 7,
            program: "/usr/bin/printf".into(),
            args: vec!["%s".into(), "exact".into()],
            working_directory: Some("/tmp".into()),
            title: Some("build".into()),
            environment_policy: Default::default(),
            environment: vec![crate::control::protocol::EnvironmentVariable {
                name: "BUILD_MODE".into(),
                value: "exact".into(),
            }],
        };
        let launch_response = Ok(Response::TabCreated {
            process_id: 42,
            request_id: "2".repeat(32),
            window_id: 7,
            tab_id: 4,
        });
        cache.record(launch_request.clone(), launch_response.clone());
        assert_eq!(cache.replay(&launch_request), Some(launch_response.clone()));
        let mut background_request = launch_request.clone();
        if let Request::LaunchTab { background, .. } = &mut background_request {
            *background = true;
        }
        assert!(
            matches!(cache.replay(&background_request), Some(Err(error)) if error.contains("different action"))
        );
        let mut background_cache = ControlReplayCache::default();
        background_cache.record(background_request.clone(), launch_response.clone());
        assert_eq!(
            background_cache.replay(&background_request),
            Some(launch_response)
        );
        assert!(matches!(
            cache.replay(&Request::LaunchTab {
                background: false,
                request_id: "2".repeat(32),
                window_id: 7,
                program: "/usr/bin/printf".into(),
                args: vec!["%s".into(), "different".into()],
                working_directory: Some("/tmp".into()),
                title: Some("build".into()),
                environment_policy: Default::default(),
                environment: vec![crate::control::protocol::EnvironmentVariable {
                    name: "BUILD_MODE".into(),
                    value: "exact".into(),
                }],
            }),
            Some(Err(error)) if error.contains("different action")
        ));

        let mut window_launch_request = Request::LaunchWindow {
            request_id: "3".repeat(32),
            source_window: 7,
            program: "/usr/bin/printf".into(),
            args: vec!["%s".into(), "exact".into()],
            working_directory: Some("/tmp".into()),
            title: Some("window build".into()),
            environment_policy: termeric_core::pty::EnvironmentPolicy {
                clear: false,
                unset: vec!["REMOVE_Z".into(), "REMOVE_A".into()],
            },
            environment: vec![
                crate::control::protocol::EnvironmentVariable {
                    name: "WINDOW_Z".into(),
                    value: "last".into(),
                },
                crate::control::protocol::EnvironmentVariable {
                    name: "WINDOW_A".into(),
                    value: "exact".into(),
                },
            ],
        };
        window_launch_request.canonicalize();
        let window_launch_response = Ok(Response::WindowLaunched {
            process_id: 42,
            request_id: "3".repeat(32),
            window_id: 9,
            tab_id: 0,
        });
        cache.record(
            window_launch_request.clone(),
            window_launch_response.clone(),
        );
        assert_eq!(
            cache.replay(&window_launch_request),
            Some(window_launch_response)
        );
        let mut reordered_window_launch_request = Request::LaunchWindow {
            request_id: "3".repeat(32),
            source_window: 7,
            program: "/usr/bin/printf".into(),
            args: vec!["%s".into(), "exact".into()],
            working_directory: Some("/tmp".into()),
            title: Some("window build".into()),
            environment_policy: termeric_core::pty::EnvironmentPolicy {
                clear: false,
                unset: vec!["REMOVE_A".into(), "REMOVE_Z".into()],
            },
            environment: vec![
                crate::control::protocol::EnvironmentVariable {
                    name: "WINDOW_A".into(),
                    value: "exact".into(),
                },
                crate::control::protocol::EnvironmentVariable {
                    name: "WINDOW_Z".into(),
                    value: "last".into(),
                },
            ],
        };
        reordered_window_launch_request.canonicalize();
        assert_eq!(
            cache.replay(&reordered_window_launch_request),
            Some(Ok(Response::WindowLaunched {
                process_id: 42,
                request_id: "3".repeat(32),
                window_id: 9,
                tab_id: 0,
            }))
        );
        let mut changed_window_launch_request = Request::LaunchWindow {
            request_id: "3".repeat(32),
            source_window: 7,
            program: "/usr/bin/printf".into(),
            args: vec!["%s".into(), "exact".into()],
            working_directory: Some("/tmp".into()),
            title: Some("window build".into()),
            environment_policy: termeric_core::pty::EnvironmentPolicy {
                clear: false,
                unset: vec!["REMOVE_A".into(), "REMOVE_Z".into()],
            },
            environment: vec![
                crate::control::protocol::EnvironmentVariable {
                    name: "WINDOW_Z".into(),
                    value: "last".into(),
                },
                crate::control::protocol::EnvironmentVariable {
                    name: "WINDOW_A".into(),
                    value: "different".into(),
                },
            ],
        };
        changed_window_launch_request.canonicalize();
        assert!(matches!(
            cache.replay(&changed_window_launch_request),
            Some(Err(error)) if error.contains("different action")
        ));

        let mut changed_policy = window_launch_request.clone();
        if let Request::LaunchWindow {
            environment_policy, ..
        } = &mut changed_policy
        {
            *environment_policy = termeric_core::pty::EnvironmentPolicy {
                clear: true,
                unset: vec![],
            };
        }
        assert!(
            matches!(cache.replay(&changed_policy), Some(Err(error)) if error.contains("different action"))
        );

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

    #[cfg(unix)]
    #[test]
    fn control_launch_tab_targets_one_stable_window_and_returns_its_tab_id() {
        if !std::path::Path::new("/bin/sh").exists() {
            return;
        }
        let mut application =
            Application::from_config(Config::default(), None, LaunchPlan::default()).unwrap();
        application.pending.clear();
        let native_id = WindowId::dummy();
        let mut window = App::new_headless(Config::default()).unwrap();
        window.tabs = Some(crate::tabs::TabManager::new(
            80,
            24,
            Config::default().shell,
            1_000,
            1024 * 1024,
        ));
        application.windows.insert(native_id, window);
        application.window_order.push(native_id);
        application.control_window_ids.insert(native_id, 7);

        let missing = application
            .launch_control_tab(
                false,
                "33333333333333333333333333333333",
                8,
                "/bin/sh",
                &["-c".into(), "sleep 1".into()],
                None,
                Some("wrong target"),
                &[],
                &Default::default(),
            )
            .unwrap_err();
        assert!(missing.contains("does not exist"), "{missing}");
        assert_eq!(
            application.windows[&native_id]
                .tabs
                .as_ref()
                .unwrap()
                .count(),
            0
        );

        let response = application
            .launch_control_tab(
                true,
                "44444444444444444444444444444444",
                7,
                "/bin/sh",
                &["-c".into(), "sleep 1".into()],
                None,
                Some("targeted"),
                &[],
                &Default::default(),
            )
            .unwrap();
        assert!(matches!(
            response,
            crate::control::protocol::Response::TabCreated {
                window_id: 7,
                tab_id: 0,
                ..
            }
        ));
        let tabs = application.windows[&native_id].tabs.as_ref().unwrap();
        assert_eq!(tabs.count(), 1);
        assert_eq!(tabs.active_tab().unwrap().title, "targeted");
        assert!(
            application
                .control_launches
                .contains("44444444444444444444444444444444")
        );
        assert!(
            !application
                .control_launches
                .contains("33333333333333333333333333333333")
        );
    }

    #[cfg(unix)]
    #[test]
    fn launch_status_survives_fast_exit_window_cleanup_and_replay_expiry() {
        use crate::control::protocol::{LaunchState, Request};
        for background in [false, true] {
            let mut application =
                Application::from_config(Config::default(), None, LaunchPlan::default()).unwrap();
            application.pending.clear();
            let native_id = WindowId::dummy();
            let mut window = App::new_headless(Config::default()).unwrap();
            window.tabs = Some(crate::tabs::TabManager::new(
                80,
                24,
                Config::default().shell,
                100,
                1024 * 1024,
            ));
            application.windows.insert(native_id, window);
            application.window_order.push(native_id);
            application.control_window_ids.insert(native_id, 7);
            let id = "a".repeat(32);
            let original = application
                .windows
                .get_mut(&native_id)
                .unwrap()
                .tabs
                .as_mut()
                .unwrap()
                .new_tab_with_command(
                    "/bin/sh",
                    &["-c".into(), "read unused".into()],
                    None,
                    Some("original"),
                    None,
                    Default::default(),
                )
                .unwrap();
            application
                .launch_control_tab(
                    background,
                    &id,
                    7,
                    "/bin/sh",
                    &["-c".into(), "exit 23".into()],
                    None,
                    None,
                    &[],
                    &Default::default(),
                )
                .unwrap();
            let selected = application.windows[&native_id]
                .tabs
                .as_ref()
                .unwrap()
                .active_tab_id();
            if background {
                assert_eq!(selected, Some(original));
            } else {
                assert_ne!(selected, Some(original));
            }
            let deadline = Instant::now() + std::time::Duration::from_secs(2);
            loop {
                application
                    .windows
                    .get_mut(&native_id)
                    .unwrap()
                    .process_pty_events();
                let (state, closed) = application.control_launches.query(&id).unwrap();
                if background {
                    assert_eq!(
                        application.windows[&native_id]
                            .tabs
                            .as_ref()
                            .unwrap()
                            .active_tab_id(),
                        Some(original)
                    );
                }
                if closed && matches!(state, LaunchState::Exited { .. }) {
                    assert_eq!(
                        state,
                        LaunchState::Exited {
                            exit_code: 23,
                            success: false
                        }
                    );
                    break;
                }
                assert!(Instant::now() < deadline);
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
            application.windows.remove(&native_id);
            assert_eq!(
                application.control_launches.query(&id).unwrap(),
                (
                    LaunchState::Exited {
                        exit_code: 23,
                        success: false
                    },
                    true
                )
            );
            let (tx, rx) = std::sync::mpsc::sync_channel(1);
            let reused = Request::CreateTab {
                request_id: id,
                window_id: 7,
            };
            assert!(application.defer_live_domain_control(&reused, tx).is_none());
            assert!(rx.recv().unwrap().unwrap_err().contains("replay expired"));
        }
    }

    #[cfg(unix)]
    #[test]
    fn control_launch_window_prepares_one_exact_local_command_and_real_pty() {
        use std::os::unix::fs::PermissionsExt;

        if !std::path::Path::new("/bin/sh").exists() {
            return;
        }
        let temp = tempfile::tempdir().unwrap();
        let helper = temp.path().join("capture-window-argv.sh");
        std::fs::write(
            &helper,
            "#!/bin/sh\nprintf '[%s]|[%s]|[%s]|[%s]\\n' \"$1\" \"$2\" \"$3\" \"$4\"\nprintf 'WINDOW_ENV=[%s]|[%s]\\n' \"${EMPTY_VALUE-unset}\" \"${WINDOW_VALUE-unset}\"\npwd\nsleep 1\n",
        )
        .unwrap();
        let mut permissions = std::fs::metadata(&helper).unwrap().permissions();
        permissions.set_mode(0o700);
        std::fs::set_permissions(&helper, permissions).unwrap();

        let mut source_config = Config::default();
        source_config.font.size = 19.0;
        let mut source = App::new_headless(source_config.clone()).unwrap();
        let mut source_tabs =
            crate::tabs::TabManager::new(80, 24, source_config.shell.clone(), 1_000, 1024 * 1024);
        source_tabs
            .new_tab_with_command(
                "/bin/sh",
                &["-c".into(), "sleep 2".into()],
                temp.path().to_str(),
                Some("source"),
                None,
                HashMap::new(),
            )
            .unwrap();
        source.tabs = Some(source_tabs);

        let mut application =
            Application::from_config(Config::default(), None, LaunchPlan::default()).unwrap();
        application.pending.clear();
        let native_id = WindowId::dummy();
        application.windows.insert(native_id, source);
        application.window_order.push(native_id);
        application.control_window_ids.insert(native_id, 7);

        let missing_cwd = application
            .prepare_control_launch_window(
                7,
                "/bin/true",
                &[],
                Some("/definitely/missing/termeric-launch-window-cwd"),
                None,
                &[],
                &Default::default(),
            )
            .unwrap_err();
        assert!(missing_cwd.contains("pass an existing absolute directory"));
        assert!(application.pending.is_empty());

        let helper = helper.to_str().unwrap();
        let result_index = application
            .prepare_control_launch_window(
                7,
                helper,
                &[
                    "".into(),
                    "two words".into(),
                    "$HOME;🙂".into(),
                    "--json".into(),
                ],
                None,
                Some("Exact window 🙂"),
                &[
                    crate::control::protocol::EnvironmentVariable {
                        name: "EMPTY_VALUE".into(),
                        value: String::new(),
                    },
                    crate::control::protocol::EnvironmentVariable {
                        name: "WINDOW_VALUE".into(),
                        value: "= exact $HOME;🙂".into(),
                    },
                ],
                &Default::default(),
            )
            .unwrap();
        assert_eq!(result_index, 0);
        assert_eq!(application.pending.len(), 1);
        let prepared = &application.pending[0];
        assert_eq!(prepared.config.font.size, 19.0);
        assert!(!prepared.allow_auto_restore);
        assert!(prepared.redact_startup_error_log);
        assert!(prepared.launch.force_new);
        assert_eq!(prepared.launch.tabs.len(), 1);
        let tab = &prepared.launch.tabs[0];
        assert_eq!(tab.working_directory.as_deref(), Some(temp.path()));
        assert_eq!(tab.title.as_deref(), Some("Exact window 🙂"));
        assert!(tab.splits.is_empty());
        assert_eq!(tab.environment.get("EMPTY_VALUE"), Some(&String::new()));
        assert_eq!(
            tab.environment.get("WINDOW_VALUE").map(String::as_str),
            Some("= exact $HOME;🙂")
        );
        assert!(matches!(
            &tab.target,
            crate::cli::LaunchTarget::Command { program, args }
                if program == helper
                    && args == &["", "two words", "$HOME;🙂", "--json"]
        ));
        assert!(prepared.launch.workspace.is_none());
        assert!(prepared.launch.domain.is_none());
        assert!(prepared.launch.domain_snapshot.is_none());

        let mut prepared = application.pending.pop().unwrap();
        prepared.init_tabs().unwrap();
        let tabs = prepared.tabs.as_ref().unwrap();
        assert_eq!(tabs.count(), 1);
        assert_eq!(tabs.active_tab_id(), Some(0));
        assert_eq!(tabs.active_tab().unwrap().title, "Exact window 🙂");

        let deadline = Instant::now() + std::time::Duration::from_secs(2);
        while Instant::now() < deadline {
            prepared.process_pty_events();
            let output = prepared
                .tabs
                .as_ref()
                .and_then(crate::tabs::TabManager::active_tab)
                .and_then(crate::tabs::Tab::focused_terminal)
                .map(|terminal| terminal.grid().plain_lines().join("\n"))
                .unwrap_or_default();
            if output.contains("[]|[two words]|[$HOME;🙂]|[--json]")
                && output.contains("WINDOW_ENV=[]|[= exact $HOME;🙂]")
                && output.contains(temp.path().to_str().unwrap())
            {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        panic!("exact argv or inherited cwd output did not reach the command window");
    }

    #[cfg(unix)]
    #[test]
    fn control_launch_window_preparation_is_atomic_for_stale_and_spawn_failures() {
        let temp = tempfile::tempdir().unwrap();
        let mut application =
            Application::from_config(Config::default(), None, LaunchPlan::default()).unwrap();
        application.pending.clear();
        let native_id = WindowId::dummy();
        let source = App::new_headless(Config::default()).unwrap();
        application.windows.insert(native_id, source);
        application.window_order.push(native_id);
        application.control_window_ids.insert(native_id, 7);

        let private_environment = [crate::control::protocol::EnvironmentVariable {
            name: "PRIVATE_BUILD_VALUE".into(),
            value: "must-not-appear-in-errors".into(),
        }];
        let result_index = application
            .prepare_control_launch_window(
                7,
                "/definitely/missing/termeric-launch-window-program",
                &[],
                temp.path().to_str(),
                None,
                &private_environment,
                &Default::default(),
            )
            .unwrap();
        let mut prepared = application.pending.remove(result_index);
        let error = prepared.init_tabs().unwrap_err();
        let error = format!("{error:#}");
        assert!(error.contains("spawn"));
        assert!(!error.contains("PRIVATE_BUILD_VALUE"));
        assert!(!error.contains("must-not-appear-in-errors"));
        assert_eq!(prepared.tabs.as_ref().unwrap().count(), 0);
        assert!(application.pending.is_empty());

        application.windows.get_mut(&native_id).unwrap().should_exit = true;
        let error = application
            .prepare_control_launch_window(
                7,
                "/bin/true",
                &[],
                temp.path().to_str(),
                None,
                &[],
                &Default::default(),
            )
            .unwrap_err();
        assert!(error.contains("closing"));
        assert!(application.pending.is_empty());

        let error = application
            .prepare_control_launch_window(
                8,
                "/bin/true",
                &[],
                temp.path().to_str(),
                None,
                &[],
                &Default::default(),
            )
            .unwrap_err();
        assert!(error.contains("does not exist"));
        assert!(application.pending.is_empty());
    }

    #[test]
    fn control_errors_are_single_line_control_free_and_bounded() {
        let error = bounded_control_error(&format!("bad\n\u{1b}{}", "x".repeat(2_000)));
        assert!(error.len() <= 1_024);
        assert!(!error.chars().any(char::is_control));
    }

    #[test]
    fn control_window_capacity_includes_its_mandatory_initial_tab() {
        assert!(
            validate_control_window_creation_capacity(
                1,
                0,
                crate::control::protocol::MAX_TOTAL_TABS - 1,
            )
            .is_ok()
        );
        assert!(
            validate_control_window_creation_capacity(
                1,
                0,
                crate::control::protocol::MAX_TOTAL_TABS,
            )
            .unwrap_err()
            .contains("process tab limit")
        );
        assert!(
            validate_control_window_creation_capacity(1, 0, usize::MAX)
                .unwrap_err()
                .contains("process tab limit")
        );
        assert!(
            validate_control_window_creation_capacity(
                crate::control::protocol::MAX_WINDOWS - 1,
                1,
                0,
            )
            .unwrap_err()
            .contains("process window limit")
        );
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
        let domain_tab_count = window.tabs.as_ref().unwrap().count();
        let local_launch = window
            .launch_control_tab(
                false,
                "/bin/true",
                &[],
                None,
                Some("must not launch"),
                &[],
                &Default::default(),
            )
            .unwrap_err();
        assert!(local_launch.contains("domain spawn"), "{local_launch}");
        assert_eq!(window.tabs.as_ref().unwrap().count(), domain_tab_count);

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
            separate_process: false,
        };
        let application = Application::from_config(Config::default(), None, plan).unwrap();
        assert_eq!(application.pending.len(), 2);
        assert!(
            application
                .pending
                .iter()
                .all(|window| !window.allow_auto_restore)
        );
    }

    #[test]
    fn plugin_workspace_query_inventory_is_ordered_bounded_and_excludes_closing_windows() {
        let plan = LaunchPlan {
            windows: vec![WindowLaunch::default(); super::super::PLUGIN_PROCESS_MAX_WINDOWS + 2],
            separate_process: false,
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

        let encoded = super::super::plugin_actions::plugin_workspace_query_event(
            "window-inspector",
            102,
            "all-windows",
            &inventory,
        )
        .unwrap();
        assert!(encoded.len() <= crate::plugin::structured::MAX_PERSISTENT_SOURCE_EVENT_BYTES);
        let value: serde_json::Value = serde_json::from_str(&encoded).unwrap();
        assert_eq!(value["event"]["type"], "workspace-query-result");
        assert_eq!(value["event"]["query_id"], "all-windows");
        assert_eq!(value["origin_window_id"], 102);
        assert_eq!(value["event"]["windows"][0]["id"], 100);
        assert_eq!(value["event"]["windows"][1]["id"], 102);
        assert_eq!(value["event"]["truncated"], true);
    }

    #[cfg(unix)]
    #[test]
    fn plugin_navigation_is_exact_atomic_non_activating_and_hook_suppressed() {
        if !std::path::Path::new("/bin/sh").is_file() {
            return;
        }

        #[derive(Debug)]
        struct TabSwitchObserver(std::sync::Arc<std::sync::atomic::AtomicUsize>);

        impl crate::plugin::hooks::Plugin for TabSwitchObserver {
            fn id(&self) -> &str {
                "navigator"
            }

            fn name(&self) -> &str {
                "Navigation Observer"
            }

            fn on_hook(
                &mut self,
                hook: &crate::plugin::Hook,
                _context: &crate::plugin::PluginContext,
            ) -> Option<crate::plugin::PluginAction> {
                if matches!(hook, crate::plugin::Hook::TabSwitched { .. }) {
                    self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                }
                None
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
        target
            .plugin_manager
            .register(Box::new(TabSwitchObserver(std::sync::Arc::clone(
                &observed_switches,
            ))))
            .unwrap();

        let mut application =
            Application::from_config(Config::default(), None, LaunchPlan::default()).unwrap();
        application.pending.clear();
        let native_id = WindowId::dummy();
        application.windows.insert(native_id, target);
        application.window_order.push(native_id);
        application.control_window_ids.insert(native_id, 7);

        let selected = plugin_event(
            &mut application,
            "navigator",
            9,
            crate::plugin::structured::StructuredResponseAction::SelectTab {
                action_id: "select-sibling".into(),
                window_id: Some(7),
                tab_id: first as u64,
            },
        );
        assert_eq!(selected["plugin_id"], "navigator");
        assert_eq!(selected["event"]["type"], "action-result");
        assert_eq!(selected["origin_window_id"], 9);
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

        let selected_same_window = plugin_event(
            &mut application,
            "navigator",
            7,
            crate::plugin::structured::StructuredResponseAction::SelectTab {
                action_id: "select-current".into(),
                window_id: Some(7),
                tab_id: second as u64,
            },
        );
        assert_eq!(selected_same_window["event"]["ok"], true);
        assert_eq!(selected_same_window["origin_window_id"], 7);
        assert_eq!(selected_same_window["event"]["changed"], true);

        let generation = application.windows[&native_id].plugin_runtime_generation;
        application.handle_structured_plugin_completed(
            crate::plugin::structured::StructuredHandlerCompletion {
                window_id: native_id,
                plugin_generation: generation,
                plugin_id: "navigator".into(),
                request_id: 99,
                continuation: None,
                result: Ok(vec![
                    crate::plugin::structured::StructuredResponseAction::SelectTab {
                        action_id: "missing-continuation".into(),
                        window_id: Some(7),
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

        let missing_pane = plugin_event(
            &mut application,
            "navigator",
            7,
            crate::plugin::structured::StructuredResponseAction::FocusPane {
                action_id: "missing-pane".into(),
                window_id: Some(7),
                tab_id: first as u64,
                pane_id: u32::MAX,
            },
        );
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

        let focused = plugin_event(
            &mut application,
            "navigator",
            9,
            crate::plugin::structured::StructuredResponseAction::FocusPane {
                action_id: "focus-sibling".into(),
                window_id: Some(7),
                tab_id: first as u64,
                pane_id: first_pane,
            },
        );
        assert_eq!(focused["event"]["ok"], true);
        assert_eq!(focused["event"]["tab_changed"], true);
        assert_eq!(focused["event"]["pane_changed"], false);
        assert_eq!(
            observed_switches.load(std::sync::atomic::Ordering::SeqCst),
            0
        );

        let stale = plugin_event(
            &mut application,
            "navigator",
            9,
            crate::plugin::structured::StructuredResponseAction::SelectTab {
                action_id: "stale-window".into(),
                window_id: Some(8),
                tab_id: first as u64,
            },
        );
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
        let closed = plugin_event(
            &mut application,
            "navigator",
            9,
            crate::plugin::structured::StructuredResponseAction::SelectTab {
                action_id: "closed-tab".into(),
                window_id: Some(7),
                tab_id: second as u64,
            },
        );
        assert_eq!(closed["event"]["ok"], false);
        assert!(
            closed["event"]["error"]
                .as_str()
                .unwrap()
                .contains("closing")
        );

        application.windows.get_mut(&native_id).unwrap().should_exit = true;
        let closing = plugin_event(
            &mut application,
            "navigator",
            9,
            crate::plugin::structured::StructuredResponseAction::SelectTab {
                action_id: "closing-window".into(),
                window_id: Some(7),
                tab_id: second as u64,
            },
        );
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
        let uninitialized = plugin_event(
            &mut application,
            "navigator",
            9,
            crate::plugin::structured::StructuredResponseAction::SelectTab {
                action_id: "uninitialized-window".into(),
                window_id: Some(7),
                tab_id: first as u64,
            },
        );
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
    fn plugin_tab_creation_is_exact_non_activating_and_hook_suppressed() {
        if !std::path::Path::new("/bin/sh").is_file() {
            return;
        }

        #[derive(Debug)]
        struct TabCreatedObserver(std::sync::Arc<std::sync::atomic::AtomicUsize>);

        impl crate::plugin::hooks::Plugin for TabCreatedObserver {
            fn id(&self) -> &str {
                "tab-builder"
            }

            fn name(&self) -> &str {
                "Tab Builder Observer"
            }

            fn on_hook(
                &mut self,
                hook: &crate::plugin::Hook,
                _context: &crate::plugin::PluginContext,
            ) -> Option<crate::plugin::PluginAction> {
                if matches!(hook, crate::plugin::Hook::TabCreated { .. }) {
                    self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                }
                None
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
        target
            .plugin_manager
            .register(Box::new(TabCreatedObserver(std::sync::Arc::clone(
                &observed_creations,
            ))))
            .unwrap();

        let mut application =
            Application::from_config(Config::default(), None, LaunchPlan::default()).unwrap();
        application.pending.clear();
        let native_id = WindowId::dummy();
        application.windows.insert(native_id, target);
        application.window_order.push(native_id);
        application.control_window_ids.insert(native_id, 7);

        let created = plugin_event(
            &mut application,
            "tab-builder",
            9,
            crate::plugin::structured::StructuredResponseAction::CreateTab {
                action_id: "new-sibling-task".into(),
                window_id: Some(7),
            },
        );
        assert_eq!(created["plugin_id"], "tab-builder");
        assert_eq!(created["event"]["type"], "action-result");
        assert_eq!(created["event"]["action"], "create-tab");
        assert_eq!(created["event"]["action_id"], "new-sibling-task");
        assert_eq!(created["event"]["ok"], true);
        assert_eq!(created["origin_window_id"], 9);
        assert_eq!(created["event"]["window_id"], 7);
        let created_tab = created["event"]["created_tab_id"].as_u64().unwrap() as usize;
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

        let created_same_window = plugin_event(
            &mut application,
            "tab-builder",
            7,
            crate::plugin::structured::StructuredResponseAction::CreateTab {
                action_id: "new-current-task".into(),
                window_id: Some(7),
            },
        );
        assert_eq!(created_same_window["event"]["ok"], true);
        assert_eq!(created_same_window["origin_window_id"], 7);
        assert_eq!(created_same_window["event"]["window_id"], 7);
        let same_window_tab = created_same_window["event"]["created_tab_id"]
            .as_u64()
            .unwrap() as usize;
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

        let stale = plugin_event(
            &mut application,
            "tab-builder",
            9,
            crate::plugin::structured::StructuredResponseAction::CreateTab {
                action_id: "stale-window".into(),
                window_id: Some(8),
            },
        );
        assert_eq!(stale["event"]["ok"], false);
        assert!(stale["event"]["created_tab_id"].is_null());
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
        let closing = plugin_event(
            &mut application,
            "tab-builder",
            9,
            crate::plugin::structured::StructuredResponseAction::CreateTab {
                action_id: "closing-window".into(),
                window_id: Some(7),
            },
        );
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
        let live_domain = plugin_event(
            &mut application,
            "tab-builder",
            9,
            crate::plugin::structured::StructuredResponseAction::CreateTab {
                action_id: "live-domain-window".into(),
                window_id: Some(7),
            },
        );
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
        assert!(
            validate_process_tab_creation_capacity(crate::control::protocol::MAX_TOTAL_TABS)
                .unwrap_err()
                .contains("process tab limit")
        );
    }

    #[cfg(unix)]
    #[test]
    fn plugin_pane_creation_is_exact_atomic_and_non_activating() {
        if !std::path::Path::new("/bin/sh").is_file() {
            return;
        }

        let shell = crate::config::ShellConfig {
            program: Some("/bin/sh".into()),
            args: vec!["-c".into(), "exit 0".into()],
            env: Default::default(),
        };
        let mut tabs = crate::tabs::TabManager::new(80, 24, shell.clone(), 1_000, 8 * 1024 * 1024);
        let tab_id = tabs.new_tab().unwrap();
        let tab = tabs.tab_mut_by_id(tab_id).unwrap();
        let owner = tab.take_terminal_owner().unwrap();
        let mut panes = crate::panes::PaneManager::from_existing(
            owner,
            shell,
            1_000,
            8 * 1024 * 1024,
            800.0,
            480.0,
            10.0,
            20.0,
        );
        panes
            .split(crate::panes::SplitDirection::Horizontal)
            .unwrap();
        tab.pane_manager = Some(panes);

        let mut target = App::new_headless(Config::default()).unwrap();
        target.tabs = Some(tabs);
        target.control_window_id = 7;
        let mut application =
            Application::from_config(Config::default(), None, LaunchPlan::default()).unwrap();
        application.pending.clear();
        let native_id = WindowId::dummy();
        application.windows.insert(native_id, target);
        application.window_order.push(native_id);
        application.control_window_ids.insert(native_id, 7);

        let created = plugin_event(
            &mut application,
            "pane-builder",
            9,
            crate::plugin::structured::StructuredResponseAction::CreatePane {
                action_id: "split-sibling-task".into(),
                window_id: Some(7),
                tab_id: tab_id as u64,
                pane_id: 0,
                direction: crate::control::protocol::SplitDirection::Vertical,
            },
        );
        assert_eq!(created["plugin_id"], "pane-builder");
        assert_eq!(created["event"]["action"], "create-pane");
        assert_eq!(created["event"]["action_id"], "split-sibling-task");
        assert_eq!(created["event"]["ok"], true);
        assert_eq!(created["origin_window_id"], 9);
        assert_eq!(created["event"]["window_id"], 7);
        assert_eq!(created["event"]["tab_id"], tab_id as u64);
        assert_eq!(created["event"]["pane_id"], 0);
        assert_eq!(created["event"]["direction"], "vertical");
        let created_pane = created["event"]["created_pane_id"].as_u64().unwrap() as u32;
        let target = &application.windows[&native_id];
        assert!(target.window.is_none());
        assert_eq!(target.tabs.as_ref().unwrap().active_tab_id(), Some(tab_id));
        let manager = target
            .tabs
            .as_ref()
            .unwrap()
            .tab_by_id(tab_id)
            .unwrap()
            .pane_manager
            .as_ref()
            .unwrap();
        assert_eq!(manager.pane_count(), 3);
        assert!(
            manager
                .get_pane(crate::panes::PaneId(created_pane))
                .is_some()
        );
        assert_eq!(manager.focused_id(), crate::panes::PaneId(created_pane));

        let created_same_window = plugin_event(
            &mut application,
            "pane-builder",
            7,
            crate::plugin::structured::StructuredResponseAction::CreatePane {
                action_id: "split-current-task".into(),
                window_id: Some(7),
                tab_id: tab_id as u64,
                pane_id: 0,
                direction: crate::control::protocol::SplitDirection::Horizontal,
            },
        );
        assert_eq!(created_same_window["event"]["ok"], true);
        assert_eq!(created_same_window["origin_window_id"], 7);
        assert_eq!(created_same_window["event"]["window_id"], 7);
        assert_eq!(
            application.windows[&native_id]
                .tabs
                .as_ref()
                .unwrap()
                .tab_by_id(tab_id)
                .unwrap()
                .pane_manager
                .as_ref()
                .unwrap()
                .pane_count(),
            4
        );

        let stale_window = plugin_event(
            &mut application,
            "pane-builder",
            9,
            crate::plugin::structured::StructuredResponseAction::CreatePane {
                action_id: "stale-window".into(),
                window_id: Some(8),
                tab_id: tab_id as u64,
                pane_id: 0,
                direction: crate::control::protocol::SplitDirection::Vertical,
            },
        );
        assert_eq!(stale_window["event"]["ok"], false);
        assert!(
            stale_window["event"]["error"]
                .as_str()
                .unwrap()
                .contains("does not exist")
        );

        let stale_pane = plugin_event(
            &mut application,
            "pane-builder",
            9,
            crate::plugin::structured::StructuredResponseAction::CreatePane {
                action_id: "stale-pane".into(),
                window_id: Some(7),
                tab_id: tab_id as u64,
                pane_id: u32::MAX,
                direction: crate::control::protocol::SplitDirection::Vertical,
            },
        );
        assert_eq!(stale_pane["event"]["ok"], false);
        assert!(stale_pane["event"]["created_pane_id"].is_null());
        assert_eq!(
            application.windows[&native_id]
                .tabs
                .as_ref()
                .unwrap()
                .tab_by_id(tab_id)
                .unwrap()
                .pane_manager
                .as_ref()
                .unwrap()
                .pane_count(),
            4
        );

        let tab = application
            .windows
            .get_mut(&native_id)
            .unwrap()
            .tabs
            .as_mut()
            .unwrap()
            .tab_mut_by_id(tab_id)
            .unwrap();
        tab.live_domain = Some(crate::tabs::LiveDomainTab {
            domain_id: "fake-domain".into(),
            tab_id: 12,
            pane_ids: HashMap::new(),
        });
        let live_domain = plugin_event(
            &mut application,
            "pane-builder",
            9,
            crate::plugin::structured::StructuredResponseAction::CreatePane {
                action_id: "live-domain".into(),
                window_id: Some(7),
                tab_id: tab_id as u64,
                pane_id: 0,
                direction: crate::control::protocol::SplitDirection::Vertical,
            },
        );
        assert_eq!(live_domain["event"]["ok"], false);
        assert!(
            live_domain["event"]["error"]
                .as_str()
                .unwrap()
                .contains("live-domain")
        );

        let tab = application
            .windows
            .get_mut(&native_id)
            .unwrap()
            .tabs
            .as_mut()
            .unwrap()
            .tab_mut_by_id(tab_id)
            .unwrap();
        tab.live_domain = None;
        tab.tab_type = crate::tabs::TabType::Serial {
            name: "fixture".into(),
        };
        let serial = plugin_event(
            &mut application,
            "pane-builder",
            9,
            crate::plugin::structured::StructuredResponseAction::CreatePane {
                action_id: "serial".into(),
                window_id: Some(7),
                tab_id: tab_id as u64,
                pane_id: 0,
                direction: crate::control::protocol::SplitDirection::Vertical,
            },
        );
        assert_eq!(serial["event"]["ok"], false);
        assert!(
            serial["event"]["error"]
                .as_str()
                .unwrap()
                .contains("Serial")
        );

        application.windows.get_mut(&native_id).unwrap().should_exit = true;
        let closing = plugin_event(
            &mut application,
            "pane-builder",
            9,
            crate::plugin::structured::StructuredResponseAction::CreatePane {
                action_id: "closing-window".into(),
                window_id: Some(7),
                tab_id: tab_id as u64,
                pane_id: 0,
                direction: crate::control::protocol::SplitDirection::Vertical,
            },
        );
        assert_eq!(closing["event"]["ok"], false);
        assert!(
            closing["event"]["error"]
                .as_str()
                .unwrap()
                .contains("closing")
        );
        assert!(
            created.to_string().len()
                <= crate::plugin::structured::MAX_PERSISTENT_SOURCE_EVENT_BYTES
        );
        assert!(
            validate_process_pane_creation_capacity(crate::control::protocol::MAX_TOTAL_PANES)
                .unwrap_err()
                .contains("process pane limit")
        );
    }

    #[cfg(unix)]
    #[test]
    fn plugin_terminal_queries_are_exact_bounded_and_non_activating() {
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
        target.control_window_id = 7;
        let mut application =
            Application::from_config(Config::default(), None, LaunchPlan::default()).unwrap();
        application.pending.clear();
        let native_id = WindowId::dummy();
        application.windows.insert(native_id, target);
        application.window_order.push(native_id);
        application.control_window_ids.insert(native_id, 7);

        let encoded = plugin_event(
            &mut application,
            "terminal-reader",
            9,
            StructuredResponseAction::QueryTerminal {
                query_id: "sibling-output".into(),
                window_id: Some(7),
                tab_id: target_tab as u64,
                pane_id: 0,
                history_lines: 20,
                max_bytes: 2048,
            },
        );
        assert!(
            encoded.to_string().len()
                <= crate::plugin::structured::MAX_PERSISTENT_SOURCE_EVENT_BYTES
        );
        let value = encoded;
        assert_eq!(value["schema_version"], 1);
        assert_eq!(value["plugin_id"], "terminal-reader");
        assert_eq!(value["event"]["type"], "terminal-query-result");
        assert_eq!(value["event"]["query_id"], "sibling-output");
        assert_eq!(value["event"]["ok"], true);
        assert_eq!(value["origin_window_id"], 9);
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

        let missing = plugin_event(
            &mut application,
            "terminal-reader",
            9,
            StructuredResponseAction::QueryTerminal {
                query_id: "stale-target".into(),
                window_id: Some(8),
                tab_id: target_tab as u64,
                pane_id: 0,
                history_lines: 0,
                max_bytes: 64,
            },
        );
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
        let escaped = plugin_event(
            &mut application,
            "terminal-reader",
            9,
            StructuredResponseAction::QueryTerminal {
                query_id: "escaped-text".into(),
                window_id: Some(7),
                tab_id: target_tab as u64,
                pane_id: 0,
                history_lines: 256,
                max_bytes: crate::plugin::structured::MAX_TERMINAL_QUERY_BYTES,
            },
        );
        assert!(
            escaped.to_string().len()
                <= crate::plugin::structured::MAX_PERSISTENT_SOURCE_EVENT_BYTES
        );
        assert_eq!(escaped["event"]["ok"], true);
        assert_eq!(escaped["event"]["truncated"], true);
        assert!(
            escaped["event"]["text_bytes"].as_u64().unwrap()
                < u64::from(crate::plugin::structured::MAX_TERMINAL_QUERY_BYTES)
        );

        assert!(
            application
                .plugin_host_operation_event(
                    "terminal-reader",
                    PluginOrigin {
                        native_id,
                        window_id: 9,
                    },
                    &all_windows_grants(),
                    &StructuredResponseAction::QueryTerminal {
                        query_id: "invalid-history".into(),
                        window_id: Some(7),
                        tab_id: target_tab as u64,
                        pane_id: 0,
                        history_lines: crate::control::protocol::MAX_TEXT_HISTORY_LINES + 1,
                        max_bytes: 64,
                    },
                )
                .unwrap_err()
                .contains("limited")
        );
        assert!(
            application
                .plugin_host_operation_event(
                    "terminal-reader",
                    PluginOrigin {
                        native_id,
                        window_id: 9,
                    },
                    &all_windows_grants(),
                    &StructuredResponseAction::QueryTerminal {
                        query_id: "invalid-bytes".into(),
                        window_id: Some(7),
                        tab_id: target_tab as u64,
                        pane_id: 0,
                        history_lines: 0,
                        max_bytes: 0,
                    },
                )
                .unwrap_err()
                .contains("between")
        );

        application.windows.get_mut(&native_id).unwrap().should_exit = true;
        let closing = plugin_event(
            &mut application,
            "terminal-reader",
            9,
            StructuredResponseAction::QueryTerminal {
                query_id: "closing-window".into(),
                window_id: Some(7),
                tab_id: target_tab as u64,
                pane_id: 0,
                history_lines: 0,
                max_bytes: 64,
            },
        );
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
    fn plugin_terminal_input_is_exact_guarded_policy_gated_and_non_activating() {
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
        target.control_window_id = 7;
        let mut application =
            Application::from_config(Config::default(), None, LaunchPlan::default()).unwrap();
        application.pending.clear();
        let native_id = WindowId::dummy();
        application.windows.insert(native_id, target);
        application.window_order.push(native_id);
        application.control_window_ids.insert(native_id, 7);

        let disabled = plugin_event(
            &mut application,
            "terminal-writer",
            9,
            StructuredResponseAction::SendText {
                action_id: "policy-disabled".into(),
                window_id: Some(7),
                tab_id: target_tab as u64,
                pane_id: 0,
                text: "safe".into(),
                paste: true,
                enter: true,
            },
        );
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
        let sent = plugin_event(
            &mut application,
            "terminal-writer",
            9,
            StructuredResponseAction::SendText {
                action_id: "safe-input".into(),
                window_id: Some(7),
                tab_id: target_tab as u64,
                pane_id: 0,
                text: "safe".into(),
                paste: true,
                enter: true,
            },
        );
        assert!(
            sent.to_string().len() <= crate::plugin::structured::MAX_PERSISTENT_SOURCE_EVENT_BYTES
        );
        assert_eq!(sent["plugin_id"], "terminal-writer");
        assert_eq!(sent["event"]["type"], "action-result");
        assert_eq!(sent["event"]["action"], "send-text");
        assert_eq!(sent["event"]["action_id"], "safe-input");
        assert_eq!(sent["event"]["ok"], true);
        assert_eq!(sent["origin_window_id"], 9);
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

        let stale = plugin_event(
            &mut application,
            "terminal-writer",
            9,
            StructuredResponseAction::SendText {
                action_id: "stale-window".into(),
                window_id: Some(8),
                tab_id: target_tab as u64,
                pane_id: 0,
                text: "safe".into(),
                paste: false,
                enter: false,
            },
        );
        assert_eq!(stale["event"]["ok"], false);
        assert!(
            stale["event"]["error"]
                .as_str()
                .unwrap()
                .contains("does not exist")
        );

        application.windows.get_mut(&native_id).unwrap().should_exit = true;
        let closing = plugin_event(
            &mut application,
            "terminal-writer",
            9,
            StructuredResponseAction::SendText {
                action_id: "closing-window".into(),
                window_id: Some(7),
                tab_id: target_tab as u64,
                pane_id: 0,
                text: "safe".into(),
                paste: false,
                enter: false,
            },
        );
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
        let read_only = plugin_event(
            &mut application,
            "terminal-writer",
            7,
            StructuredResponseAction::SendText {
                action_id: "read-only".into(),
                window_id: Some(7),
                tab_id: target_tab as u64,
                pane_id: 0,
                text: "safe".into(),
                paste: false,
                enter: false,
            },
        );
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

        let guarded = plugin_event(
            &mut application,
            "terminal-writer",
            9,
            StructuredResponseAction::SendText {
                action_id: "guarded-input".into(),
                window_id: Some(7),
                tab_id: target_tab as u64,
                pane_id: 0,
                text: "rm -rf /".into(),
                paste: false,
                enter: true,
            },
        );
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
    }

    fn window_scoped_grants(values: &[&str]) -> PluginPermissions {
        PluginPermissions::from_permissions(values.iter().map(|value| value.parse().unwrap()))
            .unwrap()
    }

    #[cfg(unix)]
    #[test]
    fn plugin_window_scope_is_enforced_where_operations_execute() {
        if !std::path::Path::new("/bin/sh").is_file() {
            return;
        }
        let (origin, other, _) = pane_transfer_fixture();
        let mut application =
            Application::from_config(Config::default(), None, LaunchPlan::default()).unwrap();
        application.pending.clear();
        let origin_native = WindowId::from(2_101_u64);
        let other_native = WindowId::from(2_202_u64);
        application.windows.insert(origin_native, origin);
        application.windows.insert(other_native, other);
        application.window_order = vec![origin_native, other_native];
        application.control_window_ids.insert(origin_native, 7);
        application.control_window_ids.insert(other_native, 8);
        let window_scoped = window_scoped_grants(&["workspace-query", "navigate", "window-close"]);
        let select = |window_id| StructuredResponseAction::SelectTab {
            action_id: "select".into(),
            window_id,
            tab_id: 0,
        };

        // Omitted and explicit origin window IDs are the plugin's own window.
        for window_id in [None, Some(7)] {
            let (event, _) = plugin_operation(
                &mut application,
                "scoped",
                7,
                &window_scoped,
                select(window_id),
            );
            assert_eq!(event["event"]["ok"], true, "{event}");
            assert_eq!(event["event"]["window_id"], 7);
        }

        // Any other window needs the all-windows scope, checked before the
        // window is even looked up.
        for window_id in [8, 99] {
            let (event, _) = plugin_operation(
                &mut application,
                "scoped",
                7,
                &window_scoped,
                select(Some(window_id)),
            );
            assert_eq!(event["event"]["ok"], false);
            let error = event["event"]["error"].as_str().unwrap();
            assert!(
                error.contains(&format!(
                    "window {window_id} is not the plugin's own window; targeting it requires navigate:all-windows"
                )),
                "{error}"
            );
        }

        // Workspace queries see only the origin window at window scope.
        let query = StructuredResponseAction::QueryWorkspace {
            query_id: "windows".into(),
        };
        let (scoped_query, _) =
            plugin_operation(&mut application, "scoped", 7, &window_scoped, query.clone());
        let ids = |event: &serde_json::Value| {
            event["event"]["windows"]
                .as_array()
                .unwrap()
                .iter()
                .map(|window| window["id"].as_u64().unwrap())
                .collect::<Vec<_>>()
        };
        assert_eq!(scoped_query["origin_window_id"], 7);
        assert_eq!(ids(&scoped_query), vec![7]);
        let (all_query, _) =
            plugin_operation(&mut application, "all", 7, &all_windows_grants(), query);
        assert_eq!(ids(&all_query), vec![7, 8]);

        // The same actions succeed with all-windows scope.
        let (selected, _) = plugin_operation(
            &mut application,
            "all",
            7,
            &all_windows_grants(),
            select(Some(8)),
        );
        assert_eq!(selected["event"]["ok"], true);
        assert_eq!(selected["event"]["window_id"], 8);

        // Window-scoped closure closes only the origin window, and may close
        // the last one; all-windows closure never closes the last window.
        let close = |window_id| StructuredResponseAction::CloseWindow {
            action_id: "close".into(),
            window_id,
        };
        let (refused, target) = plugin_operation(
            &mut application,
            "scoped",
            7,
            &window_scoped,
            close(Some(8)),
        );
        assert_eq!(refused["event"]["ok"], false);
        assert!(target.is_none());
        assert!(
            refused["event"]["error"]
                .as_str()
                .unwrap()
                .contains("window-close:all-windows")
        );
        let (origin_close, target) =
            plugin_operation(&mut application, "scoped", 7, &window_scoped, close(None));
        assert_eq!(origin_close["event"]["ok"], true);
        assert_eq!(origin_close["event"]["scheduled"], true);
        assert_eq!(origin_close["event"]["remaining_windows"], 1);
        assert_eq!(target, Some(origin_native));

        application
            .windows
            .get_mut(&other_native)
            .unwrap()
            .should_exit = true;
        let (last_close, target) =
            plugin_operation(&mut application, "scoped", 7, &window_scoped, close(None));
        assert_eq!(last_close["event"]["ok"], true);
        assert_eq!(last_close["event"]["remaining_windows"], 0);
        assert_eq!(target, Some(origin_native));
        let (final_refused, target) = plugin_operation(
            &mut application,
            "all",
            7,
            &all_windows_grants(),
            close(None),
        );
        assert_eq!(final_refused["event"]["ok"], false);
        assert!(target.is_none());
        assert!(
            final_refused["event"]["error"]
                .as_str()
                .unwrap()
                .contains("final live window")
        );
        assert!(!application.windows[&origin_native].should_exit);
    }

    /// Every action that targets a window, tab, or pane, aimed at window 8,
    /// with the capability whose scope governs it and its outcome once that
    /// capability is granted for all windows. Windows 7 (the origin) and 8
    /// each hold tab 0, split into panes 0 and 1 by divider `divider_id`, and
    /// a single-pane tab 1.
    fn actions_targeting_window_8(
        divider_id: u64,
    ) -> Vec<(
        StructuredResponseAction,
        Capability,
        std::result::Result<(), &'static str>,
    )> {
        use crate::control::protocol::SplitDirection;
        let action_id = || "scoped".to_string();
        vec![
            (
                StructuredResponseAction::SelectTab {
                    action_id: action_id(),
                    window_id: Some(8),
                    tab_id: 1,
                },
                Capability::Navigate,
                Ok(()),
            ),
            (
                StructuredResponseAction::FocusPane {
                    action_id: action_id(),
                    window_id: Some(8),
                    tab_id: 0,
                    pane_id: 0,
                },
                Capability::Navigate,
                Ok(()),
            ),
            (
                StructuredResponseAction::QueryTerminal {
                    query_id: "scoped".into(),
                    window_id: Some(8),
                    tab_id: 0,
                    pane_id: 0,
                    history_lines: 0,
                    max_bytes: 64,
                },
                Capability::TerminalRead,
                Ok(()),
            ),
            (
                StructuredResponseAction::SendText {
                    action_id: action_id(),
                    window_id: Some(8),
                    tab_id: 0,
                    pane_id: 0,
                    text: "scoped".into(),
                    paste: false,
                    enter: false,
                },
                Capability::TerminalInput,
                Ok(()),
            ),
            (
                StructuredResponseAction::CreateTab {
                    action_id: action_id(),
                    window_id: Some(8),
                },
                Capability::TabCreate,
                Ok(()),
            ),
            (
                StructuredResponseAction::CloseTab {
                    action_id: action_id(),
                    window_id: Some(8),
                    tab_id: 1,
                },
                Capability::TabClose,
                Ok(()),
            ),
            (
                StructuredResponseAction::MoveTab {
                    action_id: action_id(),
                    source_window_id: Some(8),
                    tab_id: 1,
                    destination_window_id: None,
                    destination_index: 0,
                },
                Capability::TabMove,
                Ok(()),
            ),
            (
                StructuredResponseAction::MoveTab {
                    action_id: action_id(),
                    source_window_id: None,
                    tab_id: 1,
                    destination_window_id: Some(8),
                    destination_index: 0,
                },
                Capability::TabMove,
                Ok(()),
            ),
            (
                StructuredResponseAction::CreatePane {
                    action_id: action_id(),
                    window_id: Some(8),
                    tab_id: 1,
                    pane_id: 0,
                    direction: SplitDirection::Vertical,
                },
                Capability::PaneCreate,
                // Headless windows cannot size a new pane.
                Err("Target window has no initialized renderer"),
            ),
            (
                StructuredResponseAction::ClosePane {
                    action_id: action_id(),
                    window_id: Some(8),
                    tab_id: 0,
                    pane_id: 1,
                },
                Capability::PaneClose,
                Ok(()),
            ),
            (
                StructuredResponseAction::MovePane {
                    action_id: action_id(),
                    source_window_id: Some(8),
                    source_tab_id: 0,
                    pane_id: 1,
                    destination_window_id: None,
                    destination_tab_id: 0,
                    target_pane_id: 0,
                    direction: SplitDirection::Vertical,
                },
                Capability::PaneMove,
                Ok(()),
            ),
            (
                StructuredResponseAction::MovePane {
                    action_id: action_id(),
                    source_window_id: None,
                    source_tab_id: 0,
                    pane_id: 1,
                    destination_window_id: Some(8),
                    destination_tab_id: 0,
                    target_pane_id: 0,
                    direction: SplitDirection::Vertical,
                },
                Capability::PaneMove,
                Ok(()),
            ),
            (
                StructuredResponseAction::SwapPanes {
                    action_id: action_id(),
                    window_id: Some(8),
                    tab_id: 0,
                    first_pane_id: 0,
                    second_pane_id: 1,
                },
                Capability::PaneLayout,
                Ok(()),
            ),
            (
                StructuredResponseAction::BalancePanes {
                    action_id: action_id(),
                    window_id: Some(8),
                    tab_id: 0,
                },
                Capability::PaneLayout,
                Ok(()),
            ),
            (
                StructuredResponseAction::ApplyPaneLayout {
                    action_id: action_id(),
                    window_id: Some(8),
                    tab_id: 0,
                    preset: crate::plugin::structured::PluginPaneLayoutPreset::Rows,
                },
                Capability::PaneLayout,
                Ok(()),
            ),
            (
                StructuredResponseAction::ResizeDivider {
                    action_id: action_id(),
                    window_id: Some(8),
                    tab_id: 0,
                    divider_id,
                    ratio_millis: 400,
                },
                Capability::PaneLayout,
                Ok(()),
            ),
            (
                StructuredResponseAction::CloseWindow {
                    action_id: action_id(),
                    window_id: Some(8),
                },
                Capability::WindowClose,
                Ok(()),
            ),
        ]
    }

    /// Windows 7 and 8 as described by [`actions_targeting_window_8`], both
    /// accepting external terminal input so only scope can refuse it.
    #[cfg(unix)]
    fn two_window_scope_fixture() -> Application {
        let mut application =
            Application::from_config(Config::default(), None, LaunchPlan::default()).unwrap();
        application.pending.clear();
        for (native_id, control_id) in [(2_101_u64, 7), (2_202, 8)] {
            let (mut window, _, _) = pane_transfer_fixture();
            window.config.security.allow_gui_control_input = true;
            window
                .tabs
                .as_mut()
                .unwrap()
                .new_tab_with_command(
                    "/bin/sh",
                    &["-c".into(), "sleep 5".into()],
                    None,
                    Some("second"),
                    None,
                    std::collections::HashMap::new(),
                )
                .unwrap();
            let native_id = WindowId::from(native_id);
            application.windows.insert(native_id, window);
            application.window_order.push(native_id);
            application.control_window_ids.insert(native_id, control_id);
        }
        application
    }

    #[cfg(unix)]
    #[test]
    fn window_scoped_grants_refuse_every_action_targeting_another_window() {
        if !std::path::Path::new("/bin/sh").is_file() {
            return;
        }
        let inventory = |application: &Application| {
            serde_json::to_value(application.plugin_process_inventory()).unwrap()
        };
        let divider_id =
            inventory(&two_window_scope_fixture())["windows"][1]["tabs"][0]["dividers"][0]["id"]
                .as_u64()
                .unwrap();
        let window_scoped = every_capability(PermissionScope::Window);
        let all_windows = all_windows_grants();
        for (action, capability, allowed) in actions_targeting_window_8(divider_id) {
            let name = action.name();
            let mut application = two_window_scope_fixture();
            let before = inventory(&application);
            let (refused, close_target) = plugin_operation(
                &mut application,
                "scoped",
                7,
                &window_scoped,
                action.clone(),
            );
            assert_eq!(refused["event"]["ok"], false, "{name}: {refused}");
            assert_eq!(
                refused["event"]["error"],
                format!(
                    "window 8 is not the plugin's own window; targeting it requires {capability}:all-windows"
                ),
                "{name}"
            );
            assert!(close_target.is_none(), "{name}");
            assert_eq!(inventory(&application), before, "{name} changed a window");

            let (result, _) = plugin_operation(&mut application, "all", 7, &all_windows, action);
            match allowed {
                Ok(()) => assert_eq!(result["event"]["ok"], true, "{name}: {result}"),
                Err(error) => {
                    assert_eq!(result["event"]["ok"], false, "{name}: {result}");
                    assert_eq!(result["event"]["error"], error, "{name}");
                }
            }
        }
    }

    #[cfg(unix)]
    #[test]
    fn plugin_host_results_continue_under_the_follow_up_rule() {
        use crate::plugin::structured::{HandlerAuthority, StructuredHandlerMode};
        if !std::path::Path::new("/bin/sh").is_file() {
            return;
        }
        let mut application = two_window_scope_fixture();
        let origin = PluginOrigin {
            native_id: application.control_native_window_id(7).unwrap(),
            window_id: 7,
        };
        let grants = all_windows_grants();
        let event_continuation = StructuredHandlerInvocation {
            plugin_id: "follow-up".into(),
            program: "handler".into(),
            args: Vec::new(),
            event_json: "{}".into(),
            mode: StructuredHandlerMode::Persistent,
            authority: HandlerAuthority::for_event(
                grants.clone(),
                StructuredHandlerMode::Persistent,
                false,
            ),
        };
        let query = StructuredResponseAction::QueryWorkspace {
            query_id: "windows".into(),
        };
        let select = StructuredResponseAction::SelectTab {
            action_id: "select".into(),
            window_id: None,
            tab_id: 1,
        };
        let notice = StructuredResponseAction::ShowMessage {
            message: "done".into(),
        };
        let continue_after =
            |application: &mut Application, operation: &StructuredResponseAction, from| {
                let (continuation, close_target) = application
                    .plugin_host_operation_continuation("follow-up", origin, operation, from)
                    .unwrap();
                assert!(close_target.is_none());
                continuation
            };

        // A workspace query result may still perform one host action, but not
        // another query.
        let after_query = continue_after(&mut application, &query, event_continuation.clone());
        let result: serde_json::Value = serde_json::from_str(&after_query.event_json).unwrap();
        assert_eq!(result["event"]["type"], "workspace-query-result");
        assert_eq!(after_query.authority.grants(), &grants);
        assert!(
            after_query
                .authority
                .check(std::slice::from_ref(&select))
                .is_ok()
        );
        assert!(
            after_query
                .authority
                .check(std::slice::from_ref(&query))
                .unwrap_err()
                .contains("may perform only one host action")
        );

        // An action result ends the chain, whether the action came straight
        // from an event or from a workspace query result: it may only show UI.
        for from in [event_continuation, after_query] {
            let after_action = continue_after(&mut application, &select, from);
            let result: serde_json::Value = serde_json::from_str(&after_action.event_json).unwrap();
            assert_eq!(result["event"]["type"], "action-result");
            assert_eq!(result["event"]["ok"], true, "{result}");
            assert!(
                after_action
                    .authority
                    .check(std::slice::from_ref(&notice))
                    .is_ok()
            );
            for next in [&select, &query] {
                assert!(
                    after_action
                        .authority
                        .check(std::slice::from_ref(next))
                        .unwrap_err()
                        .contains("cannot start host operations"),
                    "{} after an action result",
                    next.name()
                );
            }
        }
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
                separate_process: false,
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

    #[cfg(unix)]
    #[test]
    fn each_window_reports_app_ready_under_its_own_stable_id() {
        if !std::path::Path::new("/bin/sh").is_file() {
            return;
        }

        #[derive(Debug)]
        struct AppReadyObserver(std::sync::Arc<std::sync::Mutex<Vec<u64>>>);

        impl crate::plugin::hooks::Plugin for AppReadyObserver {
            fn id(&self) -> &str {
                "app-ready-observer"
            }

            fn name(&self) -> &str {
                "App Ready Observer"
            }

            fn on_hook(
                &mut self,
                hook: &crate::plugin::Hook,
                context: &crate::plugin::PluginContext,
            ) -> Option<crate::plugin::PluginAction> {
                if matches!(hook, crate::plugin::Hook::AppReady) {
                    self.0.lock().unwrap().push(context.window_id);
                }
                None
            }
        }

        let mut application =
            Application::from_config(Config::default(), None, LaunchPlan::default()).unwrap();
        application.pending.clear();
        let observed = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        for _ in 0..2 {
            let mut window = App::new_headless(Config::default()).unwrap();
            window.launch.tabs[0].target = crate::cli::LaunchTarget::Command {
                program: "/bin/sh".into(),
                args: vec!["-c".into(), "exit 0".into()],
            };
            window
                .plugin_manager
                .register(Box::new(AppReadyObserver(observed.clone())))
                .unwrap();
            application.pending.push(window);
        }

        // Headless windows never gain a native window, so both starts fail
        // after app-ready and neither lends its ID to a later window.
        let results = application.start_pending_windows(|window| window.init_tabs().unwrap());
        assert!(results.iter().all(std::result::Result::is_err));
        assert_eq!(*observed.lock().unwrap(), vec![1, 2]);
        assert_eq!(application.next_control_window_id, 3);
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
    fn plugin_pane_movement_routes_exactly_across_windows_and_is_atomic() {
        if !std::path::Path::new("/bin/sh").is_file() {
            return;
        }
        let (source, destination, moved_child) = pane_transfer_fixture();
        let mut application =
            Application::from_config(Config::default(), None, LaunchPlan::default()).unwrap();
        application.pending.clear();
        let source_native = WindowId::from(101_u64);
        let destination_native = WindowId::from(202_u64);
        application.windows.insert(source_native, source);
        application.windows.insert(destination_native, destination);
        application.window_order = vec![source_native, destination_native];
        application.control_window_ids.insert(source_native, 7);
        application.control_window_ids.insert(destination_native, 8);

        let refused = plugin_event(
            &mut application,
            "pane-mover",
            7,
            crate::plugin::structured::StructuredResponseAction::MovePane {
                action_id: "stale-target".into(),
                source_window_id: Some(7),
                source_tab_id: 0,
                pane_id: 1,
                destination_window_id: Some(8),
                destination_tab_id: 0,
                target_pane_id: 99,
                direction: crate::control::protocol::SplitDirection::Vertical,
            },
        );
        assert_eq!(refused["event"]["ok"], false);
        assert!(refused["event"]["destination_pane_id"].is_null());
        assert!(
            refused["event"]["error"]
                .as_str()
                .unwrap()
                .contains("does not exist")
        );
        assert_eq!(
            application.windows[&source_native]
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

        let moved = plugin_event(
            &mut application,
            "pane-mover",
            7,
            crate::plugin::structured::StructuredResponseAction::MovePane {
                action_id: "move-sibling-task".into(),
                source_window_id: Some(7),
                source_tab_id: 0,
                pane_id: 1,
                destination_window_id: Some(8),
                destination_tab_id: 0,
                target_pane_id: 0,
                direction: crate::control::protocol::SplitDirection::Horizontal,
            },
        );
        assert_eq!(moved["plugin_id"], "pane-mover");
        assert_eq!(moved["event"]["type"], "action-result");
        assert_eq!(moved["event"]["action"], "move-pane");
        assert_eq!(moved["event"]["action_id"], "move-sibling-task");
        assert_eq!(moved["event"]["ok"], true);
        assert_eq!(moved["origin_window_id"], 7);
        assert_eq!(moved["event"]["source_window_id"], 7);
        assert_eq!(moved["event"]["source_tab_id"], 0);
        assert_eq!(moved["event"]["pane_id"], 1);
        assert_eq!(moved["event"]["destination_window_id"], 8);
        assert_eq!(moved["event"]["destination_tab_id"], 0);
        assert_eq!(moved["event"]["target_pane_id"], 0);
        assert_eq!(moved["event"]["direction"], "horizontal");
        let destination_pane_id = moved["event"]["destination_pane_id"].as_u64().unwrap() as u32;
        assert_eq!(
            application.windows[&source_native]
                .tabs
                .as_ref()
                .unwrap()
                .active_tab()
                .unwrap()
                .pane_manager
                .as_ref()
                .unwrap()
                .pane_count(),
            1
        );
        let destination_manager = application.windows[&destination_native]
            .tabs
            .as_ref()
            .unwrap()
            .active_tab()
            .unwrap()
            .pane_manager
            .as_ref()
            .unwrap();
        assert_eq!(destination_manager.pane_count(), 2);
        assert_eq!(destination_manager.focused_id().0, destination_pane_id);
        assert_eq!(
            destination_manager
                .get_pane(crate::panes::PaneId(destination_pane_id))
                .unwrap()
                .pty
                .child_id(),
            moved_child
        );
        assert!(application.windows[&source_native].window.is_none());
        assert!(application.windows[&destination_native].window.is_none());
        assert_eq!(
            application.window_order,
            vec![source_native, destination_native]
        );
        assert!(
            moved.to_string().len() <= crate::plugin::structured::MAX_PERSISTENT_SOURCE_EVENT_BYTES
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
        let moved = Application::transfer_focused_pane(&mut plugin_source, &mut plugin_destination)
            .unwrap();
        assert_eq!(
            plugin_destination
                .tabs
                .as_ref()
                .unwrap()
                .active_tab()
                .unwrap()
                .pane_plugin_id(moved),
            Some("fake")
        );
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
            1
        );
        assert!(
            plugin_destination
                .tabs
                .as_ref()
                .unwrap()
                .active_tab()
                .unwrap()
                .pane_manager
                .is_some()
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
    fn a_moved_tab_takes_its_agent_view_conversation_to_the_next_window() {
        use crate::agent_view::event::{AgentEvent, Handshake};
        use crate::agent_view::session::AgentSession;
        use crate::agent_view::transcript::Transcript;
        if !std::path::Path::new("/bin/sh").is_file() {
            return;
        }
        let (mut source, mut destination, _) = pane_transfer_fixture();
        let mut transcript = Transcript::new("Claude Code", "default");
        transcript.apply(AgentEvent::Ready(Handshake::default()));
        transcript.push_user("fix the build".into(), Vec::new());
        let session = AgentSession::detached(std::env::temp_dir(), transcript);
        let id = session.id();
        source.agent_view.sessions.push(session);
        source.open_agent_view(true);
        source.agent_view.state.show(id);

        let moved = Application::transfer_active_tab(&mut source, &mut destination).unwrap();
        source.reconcile_agent_view_tabs();
        assert!(source.agent_view.session(id).is_none());
        assert!(destination.agent_view.session(id).is_some());
        assert_eq!(destination.agent_session_tab(id), Some(moved));
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
    fn plugin_tab_movement_preserves_state_and_refuses_final_source_tab() {
        if !std::path::Path::new("/bin/sh").is_file() {
            return;
        }
        let (source, destination, moved_child) = pane_transfer_fixture();
        let mut application =
            Application::from_config(Config::default(), None, LaunchPlan::default()).unwrap();
        application.pending.clear();
        let source_native = WindowId::from(303_u64);
        let destination_native = WindowId::from(404_u64);
        application.windows.insert(source_native, source);
        application.windows.insert(destination_native, destination);
        application.window_order = vec![source_native, destination_native];
        application.control_window_ids.insert(source_native, 7);
        application.control_window_ids.insert(destination_native, 8);

        let action = || crate::plugin::structured::StructuredResponseAction::MoveTab {
            action_id: "move-sibling-tab".into(),
            source_window_id: Some(7),
            tab_id: 0,
            destination_window_id: Some(8),
            destination_index: 1,
        };
        let refused = plugin_event(&mut application, "tab-mover", 7, action());
        assert_eq!(refused["event"]["ok"], false);
        assert!(refused["event"]["destination_tab_id"].is_null());
        assert!(
            refused["event"]["error"]
                .as_str()
                .unwrap()
                .contains("final non-closing tab")
        );
        assert_eq!(
            application.windows[&source_native]
                .tabs
                .as_ref()
                .unwrap()
                .count(),
            1
        );
        assert_eq!(
            application.windows[&destination_native]
                .tabs
                .as_ref()
                .unwrap()
                .count(),
            1
        );

        application
            .windows
            .get_mut(&source_native)
            .unwrap()
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
        let reordered = plugin_event(
            &mut application,
            "tab-mover",
            7,
            crate::plugin::structured::StructuredResponseAction::MoveTab {
                action_id: "reorder-source-tab".into(),
                source_window_id: Some(7),
                tab_id: 0,
                destination_window_id: Some(7),
                destination_index: 1,
            },
        );
        assert_eq!(reordered["event"]["ok"], true);
        assert_eq!(reordered["event"]["destination_tab_id"], 0);
        assert_eq!(reordered["event"]["changed"], true);
        assert_eq!(reordered["event"]["source_window_closing"], false);
        let moved = plugin_event(&mut application, "tab-mover", 7, action());
        assert_eq!(moved["plugin_id"], "tab-mover");
        assert_eq!(moved["event"]["action"], "move-tab");
        assert_eq!(moved["event"]["ok"], true);
        assert_eq!(moved["event"]["tab_id"], 0);
        assert_eq!(moved["event"]["destination_window_id"], 8);
        assert_eq!(moved["event"]["destination_tab_id"], 1);
        assert_eq!(moved["event"]["destination_index"], 1);
        assert_eq!(moved["event"]["source_window_closing"], false);
        assert_eq!(moved["event"]["changed"], true);
        assert_eq!(
            application.windows[&source_native]
                .tabs
                .as_ref()
                .unwrap()
                .count(),
            1
        );
        assert!(!application.windows[&source_native].should_exit);
        let destination_tabs = application.windows[&destination_native]
            .tabs
            .as_ref()
            .unwrap();
        assert_eq!(destination_tabs.count(), 2);
        let transferred = destination_tabs.tab_by_id(1).unwrap();
        assert!(
            transferred
                .pane_manager
                .as_ref()
                .unwrap()
                .iter_with_layout()
                .any(|(pane, _)| pane.pty.child_id() == moved_child)
        );
        assert_eq!(
            application.window_order,
            vec![source_native, destination_native]
        );
        assert!(
            moved.to_string().len() <= crate::plugin::structured::MAX_PERSISTENT_SOURCE_EVENT_BYTES
        );
    }

    #[cfg(unix)]
    #[test]
    fn plugin_pane_closure_targets_exact_same_or_sibling_window_and_preserves_tab() {
        if !std::path::Path::new("/bin/sh").is_file() {
            return;
        }
        let (target, origin, _) = pane_transfer_fixture();
        let mut application =
            Application::from_config(Config::default(), None, LaunchPlan::default()).unwrap();
        application.pending.clear();
        let target_native = WindowId::from(505_u64);
        let origin_native = WindowId::from(606_u64);
        application.windows.insert(target_native, target);
        application.windows.insert(origin_native, origin);
        application.window_order = vec![target_native, origin_native];
        application.control_window_ids.insert(target_native, 8);
        application.control_window_ids.insert(origin_native, 7);

        let target_tab_id = application.windows[&target_native]
            .tabs
            .as_ref()
            .unwrap()
            .active_tab()
            .unwrap()
            .id as u64;
        let target_pane_id = application.windows[&target_native]
            .tabs
            .as_ref()
            .unwrap()
            .active_tab()
            .unwrap()
            .pane_manager
            .as_ref()
            .unwrap()
            .focused_id()
            .0;
        let stale = plugin_event(
            &mut application,
            "pane-pruner",
            7,
            crate::plugin::structured::StructuredResponseAction::ClosePane {
                action_id: "stale-pane".into(),
                window_id: Some(8),
                tab_id: target_tab_id,
                pane_id: u32::MAX,
            },
        );
        assert_eq!(stale["event"]["ok"], false);
        assert!(
            stale["event"]["error"]
                .as_str()
                .unwrap()
                .contains("does not exist")
        );

        let closed = plugin_event(
            &mut application,
            "pane-pruner",
            7,
            crate::plugin::structured::StructuredResponseAction::ClosePane {
                action_id: "close-sibling-pane".into(),
                window_id: Some(8),
                tab_id: target_tab_id,
                pane_id: target_pane_id,
            },
        );
        assert_eq!(closed["plugin_id"], "pane-pruner");
        assert_eq!(closed["event"]["action"], "close-pane");
        assert_eq!(closed["event"]["ok"], true);
        assert_eq!(closed["origin_window_id"], 7);
        assert_eq!(closed["event"]["window_id"], 8);
        assert_eq!(closed["event"]["tab_id"], target_tab_id);
        assert_eq!(closed["event"]["pane_id"], target_pane_id);
        assert_eq!(closed["event"]["remaining_panes"], 1);
        assert!(!closed["event"]["focused_pane_id"].is_null());
        let target_tabs = application.windows[&target_native].tabs.as_ref().unwrap();
        assert_eq!(target_tabs.count(), 1);
        assert_eq!(target_tabs.active_tab().unwrap().id as u64, target_tab_id);
        assert!(!application.windows[&target_native].should_exit);
        assert!(!application.windows[&origin_native].should_exit);

        let sole_pane_id = closed["event"]["focused_pane_id"].as_u64().unwrap() as u32;
        let refused = plugin_event(
            &mut application,
            "pane-pruner",
            8,
            crate::plugin::structured::StructuredResponseAction::ClosePane {
                action_id: "refuse-final-pane".into(),
                window_id: Some(8),
                tab_id: target_tab_id,
                pane_id: sole_pane_id,
            },
        );
        assert_eq!(refused["event"]["ok"], false);
        assert_eq!(refused["origin_window_id"], 8);
        assert!(refused["event"]["remaining_panes"].is_null());
        assert!(
            refused["event"]["error"]
                .as_str()
                .unwrap()
                .contains("sole pane")
        );
        let target_tab = application.windows[&target_native]
            .tabs
            .as_ref()
            .unwrap()
            .active_tab()
            .unwrap();
        assert_eq!(
            target_tab
                .pane_manager
                .as_ref()
                .map_or(1, crate::panes::PaneManager::pane_count),
            1
        );

        assert!(
            refused.to_string().len()
                <= crate::plugin::structured::MAX_PERSISTENT_SOURCE_EVENT_BYTES
        );
    }

    #[cfg(unix)]
    #[test]
    fn plugin_tab_closure_targets_exact_window_and_preserves_final_tab() {
        if !std::path::Path::new("/bin/sh").is_file() {
            return;
        }
        let (mut target, origin, _) = pane_transfer_fixture();
        let target_tab_id = target.tabs.as_ref().unwrap().active_tab().unwrap().id;
        let survivor_id = target
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
        let mut application =
            Application::from_config(Config::default(), None, LaunchPlan::default()).unwrap();
        application.pending.clear();
        let target_native = WindowId::from(707_u64);
        let origin_native = WindowId::from(808_u64);
        application.windows.insert(target_native, target);
        application.windows.insert(origin_native, origin);
        application.window_order = vec![target_native, origin_native];
        application.control_window_ids.insert(target_native, 8);
        application.control_window_ids.insert(origin_native, 7);

        let stale = plugin_event(
            &mut application,
            "tab-pruner",
            7,
            crate::plugin::structured::StructuredResponseAction::CloseTab {
                action_id: "stale-tab".into(),
                window_id: Some(8),
                tab_id: u64::MAX,
            },
        );
        assert_eq!(stale["event"]["ok"], false);
        assert!(
            stale["event"]["error"]
                .as_str()
                .unwrap()
                .contains("does not exist")
        );

        let closed = plugin_event(
            &mut application,
            "tab-pruner",
            7,
            crate::plugin::structured::StructuredResponseAction::CloseTab {
                action_id: "close-sibling-tab".into(),
                window_id: Some(8),
                tab_id: target_tab_id as u64,
            },
        );
        assert_eq!(closed["plugin_id"], "tab-pruner");
        assert_eq!(closed["event"]["action"], "close-tab");
        assert_eq!(closed["event"]["ok"], true);
        assert_eq!(closed["origin_window_id"], 7);
        assert_eq!(closed["event"]["window_id"], 8);
        assert_eq!(closed["event"]["tab_id"], target_tab_id);
        assert_eq!(closed["event"]["remaining_tabs"], 1);
        assert_eq!(closed["event"]["active_tab_id"], survivor_id);
        assert_eq!(closed["event"]["active_changed"], false);
        let tabs = application.windows[&target_native].tabs.as_ref().unwrap();
        assert!(tabs.tab_by_id(target_tab_id).unwrap().closing_at.is_some());
        assert_eq!(tabs.active_tab_id(), Some(survivor_id));
        assert!(!application.windows[&target_native].should_exit);

        let refused = plugin_event(
            &mut application,
            "tab-pruner",
            8,
            crate::plugin::structured::StructuredResponseAction::CloseTab {
                action_id: "refuse-final-tab".into(),
                window_id: Some(8),
                tab_id: survivor_id as u64,
            },
        );
        assert_eq!(refused["event"]["ok"], false);
        assert_eq!(refused["origin_window_id"], 8);
        assert!(refused["event"]["remaining_tabs"].is_null());
        assert!(
            refused["event"]["error"]
                .as_str()
                .unwrap()
                .contains("final non-closing tab")
        );
        assert!(!application.windows[&target_native].should_exit);
        assert!(
            refused.to_string().len()
                <= crate::plugin::structured::MAX_PERSISTENT_SOURCE_EVENT_BYTES
        );
    }

    #[cfg(unix)]
    #[test]
    fn plugin_pane_layout_targets_exact_window_and_preserves_identity_and_focus() {
        if !std::path::Path::new("/bin/sh").is_file() {
            return;
        }
        let (target, origin, _) = pane_transfer_fixture();
        let mut application =
            Application::from_config(Config::default(), None, LaunchPlan::default()).unwrap();
        application.pending.clear();
        let target_native = WindowId::from(909_u64);
        let origin_native = WindowId::from(1_010_u64);
        application.windows.insert(target_native, target);
        application.windows.insert(origin_native, origin);
        application.window_order = vec![target_native, origin_native];
        application.control_window_ids.insert(target_native, 8);
        application.control_window_ids.insert(origin_native, 7);

        let target_tab_id = application.windows[&target_native]
            .tabs
            .as_ref()
            .unwrap()
            .active_tab_id()
            .unwrap();
        let (initial_order, initial_focus) = {
            let manager = application.windows[&target_native]
                .tabs
                .as_ref()
                .unwrap()
                .tab_by_id(target_tab_id)
                .unwrap()
                .pane_manager
                .as_ref()
                .unwrap();
            (
                manager
                    .pane_ids()
                    .into_iter()
                    .map(|pane_id| pane_id.0)
                    .collect::<Vec<_>>(),
                manager.focused_id().0,
            )
        };
        assert_eq!(initial_order.len(), 2);

        let stale = plugin_event(
            &mut application,
            "pane-arranger",
            7,
            crate::plugin::structured::StructuredResponseAction::BalancePanes {
                action_id: "stale-window".into(),
                window_id: Some(99),
                tab_id: target_tab_id as u64,
            },
        );
        assert_eq!(stale["event"]["ok"], false);
        assert!(stale["event"]["error"].as_str().unwrap().contains("99"));

        let swapped = plugin_event(
            &mut application,
            "pane-arranger",
            7,
            crate::plugin::structured::StructuredResponseAction::SwapPanes {
                action_id: "swap-sibling-panes".into(),
                window_id: Some(8),
                tab_id: target_tab_id as u64,
                first_pane_id: initial_order[0],
                second_pane_id: initial_order[1],
            },
        );
        assert_eq!(swapped["plugin_id"], "pane-arranger");
        assert_eq!(swapped["event"]["action"], "swap-panes");
        assert_eq!(swapped["event"]["ok"], true);
        assert_eq!(swapped["origin_window_id"], 7);
        assert_eq!(swapped["event"]["window_id"], 8);
        assert_eq!(swapped["event"]["tab_id"], target_tab_id);
        assert_eq!(swapped["event"]["focused_pane_id"], initial_focus);
        assert_eq!(
            swapped["event"]["pane_order"],
            serde_json::json!([initial_order[1], initial_order[0]])
        );
        assert_eq!(
            application.windows[&target_native]
                .tabs
                .as_ref()
                .unwrap()
                .active_tab_id(),
            Some(target_tab_id)
        );

        let balanced = plugin_event(
            &mut application,
            "pane-arranger",
            8,
            crate::plugin::structured::StructuredResponseAction::BalancePanes {
                action_id: "balance-current-panes".into(),
                window_id: Some(8),
                tab_id: target_tab_id as u64,
            },
        );
        assert_eq!(balanced["event"]["action"], "balance-panes");
        assert_eq!(balanced["event"]["ok"], true);
        assert_eq!(balanced["origin_window_id"], 8);
        assert_eq!(balanced["event"]["focused_pane_id"], initial_focus);

        let applied = plugin_event(
            &mut application,
            "pane-arranger",
            7,
            crate::plugin::structured::StructuredResponseAction::ApplyPaneLayout {
                action_id: "fat-sibling-panes".into(),
                window_id: Some(8),
                tab_id: target_tab_id as u64,
                preset: crate::plugin::structured::PluginPaneLayoutPreset::Fat,
            },
        );
        assert_eq!(applied["event"]["action"], "apply-pane-layout");
        assert_eq!(applied["event"]["preset"], "fat");
        assert_eq!(applied["event"]["ok"], true);
        assert_eq!(applied["event"]["focused_pane_id"], initial_focus);
        let target_manager = application.windows[&target_native]
            .tabs
            .as_ref()
            .unwrap()
            .tab_by_id(target_tab_id)
            .unwrap()
            .pane_manager
            .as_ref()
            .unwrap();
        let target_layout = target_manager.calculate_layout();
        let detail = target_manager
            .pane_ids()
            .into_iter()
            .find(|pane| pane.0 != initial_focus)
            .unwrap();
        assert!(target_layout[&detail].y > target_layout[&crate::panes::PaneId(initial_focus)].y);
        assert_eq!(
            application.windows[&target_native]
                .tabs
                .as_ref()
                .unwrap()
                .active_tab_id(),
            Some(target_tab_id)
        );

        let origin_tab_id = application.windows[&origin_native]
            .tabs
            .as_ref()
            .unwrap()
            .active_tab_id()
            .unwrap();
        let unsplit = plugin_event(
            &mut application,
            "pane-arranger",
            7,
            crate::plugin::structured::StructuredResponseAction::BalancePanes {
                action_id: "refuse-unsplit".into(),
                window_id: Some(7),
                tab_id: origin_tab_id as u64,
            },
        );
        assert_eq!(unsplit["event"]["ok"], false);
        assert!(
            unsplit["event"]["error"]
                .as_str()
                .unwrap()
                .contains("not split")
        );
        assert!(!application.windows[&target_native].should_exit);
        assert!(!application.windows[&origin_native].should_exit);
        assert!(
            unsplit.to_string().len()
                <= crate::plugin::structured::MAX_PERSISTENT_SOURCE_EVENT_BYTES
        );
    }

    #[cfg(unix)]
    #[test]
    fn plugin_window_closure_is_exact_non_final_and_result_gated() {
        if !std::path::Path::new("/bin/sh").is_file() {
            return;
        }
        let (target, origin, _) = pane_transfer_fixture();
        let mut application =
            Application::from_config(Config::default(), None, LaunchPlan::default()).unwrap();
        application.pending.clear();
        let target_native = WindowId::from(1_111_u64);
        let origin_native = WindowId::from(1_212_u64);
        application.windows.insert(target_native, target);
        application.windows.insert(origin_native, origin);
        application.window_order = vec![target_native, origin_native];
        application.control_window_ids.insert(target_native, 8);
        application.control_window_ids.insert(origin_native, 7);

        let stale = plugin_event(
            &mut application,
            "window-closer",
            7,
            crate::plugin::structured::StructuredResponseAction::CloseWindow {
                action_id: "stale-window".into(),
                window_id: Some(99),
            },
        );
        assert_eq!(stale["event"]["ok"], false);
        assert!(stale["event"]["scheduled"].is_null());
        assert!(stale["event"]["error"].as_str().unwrap().contains("99"));

        application
            .windows
            .get_mut(&target_native)
            .unwrap()
            .pending_plugin_window_close = Some(PendingPluginWindowClose {
            plugin_id: "other-closer".into(),
            plugin_generation: 3,
            result_request_id: 9,
        });
        let already_pending = plugin_event(
            &mut application,
            "window-closer",
            7,
            crate::plugin::structured::StructuredResponseAction::CloseWindow {
                action_id: "duplicate-close".into(),
                window_id: Some(8),
            },
        );
        assert_eq!(already_pending["event"]["ok"], false);
        assert!(
            already_pending["event"]["error"]
                .as_str()
                .unwrap()
                .contains("pending plugin closure")
        );
        application
            .windows
            .get_mut(&target_native)
            .unwrap()
            .pending_plugin_window_close = None;

        let sibling = plugin_event(
            &mut application,
            "window-closer",
            7,
            crate::plugin::structured::StructuredResponseAction::CloseWindow {
                action_id: "close-sibling-window".into(),
                window_id: Some(8),
            },
        );
        assert_eq!(sibling["plugin_id"], "window-closer");
        assert_eq!(sibling["event"]["action"], "close-window");
        assert_eq!(sibling["event"]["ok"], true);
        assert_eq!(sibling["origin_window_id"], 7);
        assert_eq!(sibling["event"]["window_id"], 8);
        assert_eq!(sibling["event"]["scheduled"], true);
        assert_eq!(sibling["event"]["tab_count"], 1);
        assert_eq!(sibling["event"]["remaining_windows"], 1);
        assert!(!application.windows[&target_native].should_exit);
        application.arm_plugin_window_close(target_native, origin_native, "window-closer", 4, 41);
        assert!(application.windows[&target_native].should_exit);
        assert!(!application.windows[&origin_native].should_exit);
        assert!(
            application.windows[&origin_native]
                .pending_plugin_window_close
                .is_none()
        );

        application
            .windows
            .get_mut(&target_native)
            .unwrap()
            .should_exit = false;
        let origin_event = plugin_event(
            &mut application,
            "window-closer",
            7,
            crate::plugin::structured::StructuredResponseAction::CloseWindow {
                action_id: "close-origin-window".into(),
                window_id: Some(7),
            },
        );
        assert_eq!(origin_event["event"]["ok"], true);
        let generation = application.windows[&origin_native].plugin_runtime_generation;
        application.arm_plugin_window_close(
            origin_native,
            origin_native,
            "window-closer",
            generation,
            42,
        );
        let completion = |request_id| crate::plugin::structured::StructuredHandlerCompletion {
            window_id: origin_native,
            plugin_generation: generation,
            plugin_id: "window-closer".into(),
            request_id,
            continuation: None,
            result: Ok(Vec::new()),
        };
        application
            .windows
            .get_mut(&origin_native)
            .unwrap()
            .accept_structured_plugin_completion(completion(41));
        assert!(!application.windows[&origin_native].should_exit);
        assert!(
            application.windows[&origin_native]
                .pending_plugin_window_close
                .is_some()
        );
        application
            .windows
            .get_mut(&origin_native)
            .unwrap()
            .accept_structured_plugin_completion(completion(42));
        assert!(application.windows[&origin_native].should_exit);
        assert!(
            application.windows[&origin_native]
                .pending_plugin_window_close
                .is_none()
        );
        assert!(
            origin_event.to_string().len()
                <= crate::plugin::structured::MAX_PERSISTENT_SOURCE_EVENT_BYTES
        );

        application
            .windows
            .get_mut(&origin_native)
            .unwrap()
            .should_exit = false;
        application
            .windows
            .get_mut(&target_native)
            .unwrap()
            .should_exit = true;
        let final_window = plugin_event(
            &mut application,
            "window-closer",
            7,
            crate::plugin::structured::StructuredResponseAction::CloseWindow {
                action_id: "refuse-final-window".into(),
                window_id: Some(7),
            },
        );
        assert_eq!(final_window["event"]["ok"], false);
        assert_eq!(final_window["event"]["scheduled"], serde_json::Value::Null);
        assert!(
            final_window["event"]["error"]
                .as_str()
                .unwrap()
                .contains("final live")
        );
        assert!(!application.windows[&origin_native].should_exit);
    }

    #[cfg(unix)]
    #[test]
    fn plugin_window_focus_is_exact_bounded_and_reuses_native_policy() {
        if !std::path::Path::new("/bin/sh").is_file() {
            return;
        }
        let (target, origin, _) = pane_transfer_fixture();
        let mut application =
            Application::from_config(Config::default(), None, LaunchPlan::default()).unwrap();
        application.pending.clear();
        let target_native = WindowId::from(1_313_u64);
        let origin_native = WindowId::from(1_414_u64);
        application.windows.insert(target_native, target);
        application.windows.insert(origin_native, origin);
        application.window_order = vec![target_native, origin_native];
        application.control_window_ids.insert(target_native, 8);
        application.control_window_ids.insert(origin_native, 7);

        let stale = plugin_event(
            &mut application,
            "window-focuser",
            7,
            crate::plugin::structured::StructuredResponseAction::FocusWindow {
                action_id: "stale-window".into(),
                window_id: 99,
            },
        );
        assert_eq!(stale["event"]["action"], "focus-window");
        assert_eq!(stale["event"]["ok"], false);
        assert_eq!(stale["origin_window_id"], 7);
        assert_eq!(stale["event"]["window_id"], 99);
        assert_eq!(stale["event"]["requested"], serde_json::Value::Null);
        assert!(stale["event"]["already_focused"].is_null());
        assert!(stale["event"]["error"].as_str().unwrap().contains("99"));

        let native_less = plugin_event(
            &mut application,
            "window-focuser",
            7,
            crate::plugin::structured::StructuredResponseAction::FocusWindow {
                action_id: "exact-sibling".into(),
                window_id: 8,
            },
        );
        assert_eq!(native_less["event"]["ok"], false);
        assert_eq!(native_less["event"]["window_id"], 8);
        assert_eq!(native_less["event"]["requested"], serde_json::Value::Null);
        assert!(
            native_less["event"]["error"]
                .as_str()
                .unwrap()
                .contains("has no native window")
        );

        application
            .windows
            .get_mut(&target_native)
            .unwrap()
            .should_exit = true;
        let closing = plugin_event(
            &mut application,
            "window-focuser",
            7,
            crate::plugin::structured::StructuredResponseAction::FocusWindow {
                action_id: "closing-sibling".into(),
                window_id: 8,
            },
        );
        assert_eq!(closing["event"]["ok"], false);
        assert!(
            closing["event"]["error"]
                .as_str()
                .unwrap()
                .contains("closing")
        );
        assert!(
            closing.to_string().len()
                <= crate::plugin::structured::MAX_PERSISTENT_SOURCE_EVENT_BYTES
        );
        assert!(!application.windows[&origin_native].should_exit);
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
