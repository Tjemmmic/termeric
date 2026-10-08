//! Process-level ownership for independent native windows.

use super::{
    AGENT_VIEW_WAKE_PENDING, App, AppEvent, PTY_WAKE_PENDING, PendingPluginWindowClose, Requester,
    TRANSCRIPT_WAKE_PENDING, WindowKey, WindowResult, clear_wake_pending,
    redraw_scheduler::RedrawReason,
};
use crate::cli::{LaunchPlan, WindowLaunch};
use crate::config::Config;
use crate::plugin::permission::{Capability, PermissionScope, PluginPermissions};
use crate::plugin::structured::{StructuredHandlerInvocation, StructuredResponseAction};
use anyhow::Result;
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use std::time::Instant;
use winit::application::ApplicationHandler;
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, ControlFlow};
use winit::window::WindowId;

mod control;
pub(super) mod inventory;
mod live_domain;
mod plugin_host;
mod transfer;

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
    /// The one plugin handler host every window shares.
    plugin_host: Arc<crate::plugin::structured::StructuredPluginHost>,
    /// The plugin generation every window was last brought to.
    plugin_generation_seen: u64,
    /// The plugin registry, polled once for the whole process.
    plugin_paths: Option<crate::plugin::runtime::PluginPaths>,
    last_plugin_registry_mtime: Option<std::time::SystemTime>,
    last_plugin_registry_check: Instant,
    /// The renderer `--renderer` chose for every window of this process,
    /// kept apart from the configuration so a reload cannot drop it and a
    /// save cannot write it.
    renderer_backend: Option<crate::config::RendererBackend>,
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

/// A request ID that names an earlier, different request.
const REQUEST_ID_REUSED: &str = "GUI control request ID was already used for a different action";
/// A launch whose request ID is still held, though its replay was evicted.
const LAUNCH_REPLAY_EXPIRED: &str = "Launch request ID remains retained but its mutation replay expired; query launch-status or use a new request ID";

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
        .map(|tab| tab.panes.pane_count())
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
    window: WindowKey,
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
            Err(REQUEST_ID_REUSED.into())
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
    let output = crate::safe_text::single_line_within(error, 1_024);
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
    direction: crate::panes::SplitDirection,
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
        // Automatic archives from earlier exits are kept only while the
        // startup state can still restore them.
        let pruned = crate::workspace::prune_automatic_workspaces(&state.last_workspace_ids);
        if pruned > 0 {
            log::info!("Pruned {pruned} automatic workspace archives no longer restorable");
        }
        Self::for_startup(config, warning, &state, launch)
    }

    /// The process `launch` starts with `config` and the startup `state`:
    /// the launch itself, or the workspaces open at the last exit.
    fn for_startup(
        config: Config,
        warning: Option<String>,
        state: &super::state::AppState,
        launch: LaunchPlan,
    ) -> Result<Self> {
        let restoring = auto_restore_plan(&config, state, &launch);
        let automatic_restore = restoring.is_some();
        let mut application = Self::from_config(config, warning, restoring.unwrap_or(launch))?;
        // An automatic archive that no longer opens costs a shell, never
        // the startup the person asked for.
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
        let renderer_backend = launch.renderer_backend;
        let restores_last_session = launch.restores_last_session();
        let plugin_host = Arc::new(crate::plugin::structured::StructuredPluginHost::default());
        let plugin_paths = crate::plugin::runtime::PluginPaths::configured();
        let mut pending = Vec::with_capacity(launch.windows.len());
        for window_launch in launch.windows {
            let quick_access = window_launch.quick_access;
            let mut window = App::from_config(
                config.clone(),
                warning.take(),
                true,
                Arc::clone(&plugin_host),
            )?;
            window.renderer_backend_override = renderer_backend;
            window.launch = window_launch;
            window.allow_auto_restore = restores_last_session;
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
            plugin_generation_seen: plugin_host.generation(),
            plugin_host,
            last_plugin_registry_mtime: plugin_paths
                .as_ref()
                .and_then(crate::plugin::runtime::PluginPaths::registry_mtime),
            last_plugin_registry_check: Instant::now(),
            plugin_paths,
            renderer_backend,
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
        let mut window = App::from_config(config, None, true, Arc::clone(&self.plugin_host))?;
        window.renderer_backend_override = self.renderer_backend;
        window.set_system_accessibility_preferences(self.system_accessibility);
        window.launch = launch;
        window.allow_auto_restore = false;
        self.pending.push(window);
        Ok(())
    }

    fn control_inventory(&self) -> crate::control::protocol::ProcessInventory {
        let (windows, incomplete) = self.inventory_windows();
        let mut inventory = inventory::process_inventory(
            &windows,
            crate::control::protocol::MAX_WINDOWS,
            inventory::ControlInventoryBudget::default(),
        );
        inventory.truncated |= incomplete;
        inventory
    }

    /// Every window with its control ID, in creation order, and whether an
    /// open window had to be left out for lacking one.
    fn inventory_windows(&self) -> (Vec<(u64, &App)>, bool) {
        let mut incomplete = false;
        let windows = self
            .window_order
            .iter()
            .filter_map(|native_id| {
                let window = self.windows.get(native_id);
                match (window, self.control_window_ids.get(native_id)) {
                    (Some(window), Some(control_id)) => Some((*control_id, window)),
                    // Closing windows are left out of every inventory.
                    (Some(window), None) if window.should_exit => None,
                    _ => {
                        incomplete = true;
                        None
                    }
                }
            })
            .collect();
        (windows, incomplete)
    }

    /// The window a request names by control ID `window_id`, unless it is
    /// closing.
    fn live_window(&self, native_id: WindowId, window_id: u64) -> Result<&App, String> {
        self.windows
            .get(&native_id)
            .filter(|window| !window.should_exit)
            .ok_or_else(|| format!("GUI window {window_id} is closing"))
    }

    fn live_window_mut(&mut self, native_id: WindowId, window_id: u64) -> Result<&mut App, String> {
        self.windows
            .get_mut(&native_id)
            .filter(|window| !window.should_exit)
            .ok_or_else(|| format!("GUI window {window_id} is closing"))
    }

    /// The live window a request names by control ID, with its native ID.
    fn live_control_window(&self, window_id: u64) -> Result<(WindowId, &App), String> {
        let native_id = self.control_native_window_id(window_id)?;
        Ok((native_id, self.live_window(native_id, window_id)?))
    }

    /// The open window background work tagged with `key`. Once that window
    /// has closed there is none, even when a later window took its native ID.
    fn keyed_window(&self, key: WindowKey) -> Option<&App> {
        let native_id = self.control_native_window_id(key.0).ok()?;
        self.windows.get(&native_id)
    }

    fn keyed_window_mut(&mut self, key: WindowKey) -> Option<&mut App> {
        let native_id = self.control_native_window_id(key.0).ok()?;
        self.windows.get_mut(&native_id)
    }

    /// Hand a background result to the window `key` names, or drop it if
    /// that window has closed.
    fn deliver_window_result(&mut self, key: WindowKey, result: WindowResult) {
        let Some(window) = self.keyed_window_mut(key) else {
            log::debug!("Discarding a background result for closed window {key:?}");
            return;
        };
        window.redraw_scheduler.request(result.redraw_reason());
        window.handle_window_result(result);
    }

    /// The daemon's answer to a live-domain control reaches the window that
    /// sent it, if still open, and then every control client waiting on it.
    fn deliver_live_domain_control_completed(
        &mut self,
        key: WindowKey,
        control: &super::LiveDomainControl,
        result: &std::result::Result<crate::domain::model::DomainSnapshot, String>,
    ) {
        match self.keyed_window_mut(key) {
            Some(window) => {
                window
                    .redraw_scheduler
                    .request(RedrawReason::BackgroundTask);
                window.handle_live_domain_control_completed(control.clone(), result.clone());
            }
            None => {
                log::debug!("Discarding a live-domain control result for closed window {key:?}");
            }
        }
        self.complete_live_domain_control(key, control, result);
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

/// The launch that reopens the workspaces open at the last exit, one window
/// each, when `launch` restores the last session and automatic restore is
/// on and has something to restore.
fn auto_restore_plan(
    config: &Config,
    state: &super::state::AppState,
    launch: &LaunchPlan,
) -> Option<LaunchPlan> {
    if !launch.restores_last_session()
        || !config.session.enabled
        || !config.session.auto_restore
        || state.last_workspace_ids.is_empty()
    {
        return None;
    }
    Some(LaunchPlan {
        windows: state
            .last_workspace_ids
            .iter()
            .map(|id| WindowLaunch {
                workspace: Some(id.clone()),
                ..WindowLaunch::default()
            })
            .collect(),
        ..launch.clone()
    })
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
            AppEvent::WindowResult { window, result } => {
                self.deliver_window_result(window, result);
                self.remove_closed_windows(event_loop);
                return;
            }
            // AccessKit names its window by native ID. Each window's adapter
            // ends with the window, so it only speaks for the window that
            // holds that ID.
            AppEvent::AccessKit(event) => {
                if let Some(window) = self.windows.get_mut(&event.window_id) {
                    window.redraw_scheduler.request(RedrawReason::Accessibility);
                    window.handle_accesskit_event(event);
                }
                self.remove_closed_windows(event_loop);
                return;
            }
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
                // Replays and deferred live-domain controls compare requests,
                // so every path sees the one canonical spelling.
                request.canonicalize();
                let Some(respond_to) = self.defer_live_domain_control(&request, respond_to) else {
                    return;
                };
                let result = self.handle_control_request(event_loop, request, peer_process);
                let _ = respond_to.send(result);
                return;
            }
            AppEvent::LiveDomainControlCompleted {
                window,
                control,
                result,
            } => {
                self.deliver_live_domain_control_completed(window, &control, &result);
                self.remove_closed_windows(event_loop);
                return;
            }
            AppEvent::DomainPickerPrepared {
                window,
                generation,
                result,
            } => {
                let requested = match self.keyed_window_mut(window) {
                    Some(window) => {
                        window
                            .redraw_scheduler
                            .request(RedrawReason::BackgroundTask);
                        window.handle_domain_picker_prepared(generation, result);
                        window
                            .requested_window_launch
                            .take()
                            .map(|launch| (window.config.clone(), launch))
                    }
                    None => {
                        log::debug!("Discarding a domain preparation for closed window {window:?}");
                        None
                    }
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
            AppEvent::StructuredPluginCompleted(completion) => {
                self.handle_structured_plugin_completed(completion);
                self.remove_closed_windows(event_loop);
                return;
            }
            AppEvent::AgentView => {
                clear_wake_pending(&AGENT_VIEW_WAKE_PENDING);
                for window in self.windows.values_mut() {
                    window.process_agent_view_events();
                }
                self.remove_closed_windows(event_loop);
                return;
            }
            AppEvent::Pty => {
                clear_wake_pending(&PTY_WAKE_PENDING);
                true
            }
            AppEvent::Transcript => {
                clear_wake_pending(&TRANSCRIPT_WAKE_PENDING);
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
        self.sync_plugin_generations(now);
        for window in self.windows.values_mut() {
            window.poll_idle_sources(now);
            if window.process_pty_events() {
                window
                    .redraw_scheduler
                    .request(RedrawReason::TerminalOutput);
            }
            if !window.should_exit {
                window.report_active_tab_switch();
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
mod tests;
