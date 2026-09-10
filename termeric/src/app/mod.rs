//! Application state and event handling

mod accessibility;
mod application;
mod configuration;
mod copy_mode;
mod desktop_io;
mod domain_picker;
mod event_loop;
mod ime;
mod mouse;
mod plugin_actions;
mod redraw_scheduler;
mod renderer_recovery;
mod runtime_plugins;
mod sessions;
pub mod state;
mod system_accessibility;
#[cfg(test)]
mod tests;
mod workspace_actions;
mod workspaces;

use anyhow::Result;
use std::io::Write as _;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};
use winit::{
    event::{ElementState, MouseButton},
    event_loop::EventLoopProxy,
    keyboard::ModifiersState,
    window::{Icon, Window},
};

use termeric_core::TerminalEvent;

use self::redraw_scheduler::RedrawScheduler;
use self::state::AppState;
use crate::config::theme::Theme;
use crate::config::{Config, SplashMode};
use crate::harvest::HarvestMode;
use crate::input::keybindings::{KeyAction, KeyBindingManager};
use crate::input::{Action, MouseReportButton, MouseState, ShortcutRegistry, screen_to_grid};
use crate::notifications::{Bell, BellConfig, Notifications};
use crate::panes::{PaneId, PaneManager, SplitDirection};
use crate::plugin::claude_code::ClaudeCodePlugin;
use crate::plugin::codex::CodexPlugin;
use crate::plugin::{PluginContext, PluginManager};
use crate::renderer::types::{
    CellarEntryInfo, CellarRenderInfo, HarvestRenderInfo, IndicatorRenderInfo, PaneRenderData,
    SproutMenuRenderInfo,
};
use crate::renderer::{Renderer, TextBlinkPhases, VisibleTextBlink};
use crate::tabs::{RoutedTerminalEvent, TabManager};
use crate::terminal::container::{PaneRenderInfo, TerminalContainer};
use crate::terminal::copy_mode::CopyMode;
use crate::terminal::guardrails::GuardrailOverlay;
use crate::terminal::search::Search;
use crate::terminal::splash::show_splash;
use crate::terminal::url::{UrlCache, UrlDetector};
use crate::ui::command_palette::palette::core_commands;
use crate::ui::command_palette::{CommandPalette, PaletteResult};
use crate::ui::context_picker::ContextPicker;
use crate::ui::diff_review::DiffReviewOverlay;
use crate::ui::domain_picker::DomainPicker;
use crate::ui::info_overlay::InfoOverlay;
use crate::ui::settings::{SettingsAction, SettingsPanel};
use crate::ui::shortcuts_panel::{ShortcutsAction, ShortcutsPanel};

pub use application::Application;

#[derive(Debug)]
pub(crate) enum AppEvent {
    Pty,
    Git,
    Transcript,
    ControlRequest {
        request: crate::control::protocol::Request,
        respond_to: std::sync::mpsc::SyncSender<
            std::result::Result<crate::control::protocol::Response, String>,
        >,
    },
    LiveDomainControlCompleted {
        window_id: winit::window::WindowId,
        control: LiveDomainControl,
        result: std::result::Result<crate::domain::model::DomainSnapshot, String>,
    },
    LiveDomainTopology {
        window_id: winit::window::WindowId,
        domain_id: String,
        result: std::result::Result<crate::domain::DomainTopologyEvent, String>,
    },
    DomainPickerLoaded {
        window_id: winit::window::WindowId,
        generation: u64,
        result: std::result::Result<Vec<crate::domain::model::DomainSummary>, String>,
    },
    DomainPickerPrepared {
        window_id: winit::window::WindowId,
        generation: u64,
        result: std::result::Result<crate::domain::model::DomainSnapshot, String>,
    },
    ContextGathered {
        window_id: winit::window::WindowId,
        request_id: u64,
        result: std::result::Result<String, String>,
    },
    FileUploadCompleted {
        window_id: winit::window::WindowId,
        request_id: u64,
        result: std::result::Result<FileUploadCompletion, String>,
    },
    FileUploadProgress {
        window_id: winit::window::WindowId,
        request_id: u64,
        bytes_sent: u64,
        total_bytes: u64,
    },
    StructuredPluginCompleted(crate::plugin::structured::StructuredHandlerCompletion),
    AccessKit(accesskit_winit::Event),
    RendererDeviceLost {
        window_id: winit::window::WindowId,
        generation: u64,
        failure: crate::renderer::RendererDeviceFailure,
    },
    BackgroundImageLoaded {
        window_id: winit::window::WindowId,
        generation: u64,
        path: PathBuf,
        result: std::result::Result<crate::renderer::DecodedBackgroundImage, String>,
    },
    SystemAccessibilityChanged(system_accessibility::SystemAccessibilityPreferences),
    SystemAccessibilityUnavailable(String),
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) enum LiveDomainControl {
    SpawnTab {
        domain_id: String,
        spawn: crate::domain::model::SpawnSpec,
    },
    SplitPane {
        domain_id: String,
        remote_pane_id: u32,
        direction: crate::domain::model::SplitDirection,
        spawn: crate::domain::model::SpawnSpec,
    },
    ResizeSplit {
        domain_id: String,
        remote_split_id: u32,
        ratio_millis: u16,
    },
    RenameTab {
        domain_id: String,
        remote_tab_id: u32,
        title: Option<String>,
    },
    SetTabPinned {
        domain_id: String,
        remote_tab_id: u32,
        pinned: bool,
    },
    MoveTab {
        domain_id: String,
        remote_tab_id: u32,
        index: u16,
    },
    SwapPaneNext {
        domain_id: String,
        remote_pane_id: u32,
    },
    BalanceTab {
        domain_id: String,
        remote_tab_id: u32,
    },
    ApplyPaneLayout {
        domain_id: String,
        remote_tab_id: u32,
        preset: crate::domain::model::LivePaneLayoutPreset,
    },
    MovePane {
        domain_id: String,
        remote_pane_id: u32,
        source_tab_id: u32,
        destination_tab_id: u32,
        target_pane_id: u32,
        direction: crate::domain::model::SplitDirection,
    },
    ClosePane {
        domain_id: String,
        remote_pane_id: u32,
        local_tab_id: usize,
    },
    CloseTab {
        domain_id: String,
        remote_tab_id: u32,
        local_tab_id: usize,
    },
}

impl LiveDomainControl {
    fn domain_id(&self) -> &str {
        match self {
            Self::SpawnTab { domain_id, .. }
            | Self::SplitPane { domain_id, .. }
            | Self::ResizeSplit { domain_id, .. }
            | Self::RenameTab { domain_id, .. }
            | Self::SetTabPinned { domain_id, .. }
            | Self::MoveTab { domain_id, .. }
            | Self::SwapPaneNext { domain_id, .. }
            | Self::BalanceTab { domain_id, .. }
            | Self::ApplyPaneLayout { domain_id, .. }
            | Self::MovePane { domain_id, .. }
            | Self::ClosePane { domain_id, .. }
            | Self::CloseTab { domain_id, .. } => domain_id,
        }
    }

    fn request(&self) -> crate::domain::protocol::Request {
        match self {
            Self::SpawnTab { domain_id, spawn } => crate::domain::protocol::Request::SpawnTab {
                domain: domain_id.clone(),
                spawn: spawn.clone(),
            },
            Self::SplitPane {
                domain_id,
                remote_pane_id,
                direction,
                spawn,
            } => crate::domain::protocol::Request::SplitPane {
                domain: domain_id.clone(),
                pane: *remote_pane_id,
                direction: *direction,
                spawn: spawn.clone(),
            },
            Self::ResizeSplit {
                domain_id,
                remote_split_id,
                ratio_millis,
            } => crate::domain::protocol::Request::ResizeSplit {
                domain: domain_id.clone(),
                split: *remote_split_id,
                ratio_millis: *ratio_millis,
            },
            Self::RenameTab {
                domain_id,
                remote_tab_id,
                title,
            } => crate::domain::protocol::Request::RenameTab {
                domain: domain_id.clone(),
                tab: *remote_tab_id,
                title: title.clone(),
            },
            Self::SetTabPinned {
                domain_id,
                remote_tab_id,
                pinned,
            } => crate::domain::protocol::Request::SetTabPinned {
                domain: domain_id.clone(),
                tab: *remote_tab_id,
                pinned: *pinned,
            },
            Self::MoveTab {
                domain_id,
                remote_tab_id,
                index,
            } => crate::domain::protocol::Request::MoveTab {
                domain: domain_id.clone(),
                tab: *remote_tab_id,
                index: *index,
            },
            Self::SwapPaneNext {
                domain_id,
                remote_pane_id,
            } => crate::domain::protocol::Request::SwapPaneNext {
                domain: domain_id.clone(),
                pane: *remote_pane_id,
            },
            Self::BalanceTab {
                domain_id,
                remote_tab_id,
            } => crate::domain::protocol::Request::BalanceTab {
                domain: domain_id.clone(),
                tab: *remote_tab_id,
            },
            Self::ApplyPaneLayout {
                domain_id,
                remote_tab_id,
                preset,
            } => crate::domain::protocol::Request::ApplyPaneLayout {
                domain: domain_id.clone(),
                tab: *remote_tab_id,
                preset: *preset,
            },
            Self::MovePane {
                domain_id,
                remote_pane_id,
                source_tab_id: _,
                destination_tab_id,
                target_pane_id,
                direction,
            } => crate::domain::protocol::Request::MovePane {
                domain: domain_id.clone(),
                pane: *remote_pane_id,
                tab: *destination_tab_id,
                target: Some(*target_pane_id),
                direction: *direction,
            },
            Self::ClosePane {
                domain_id,
                remote_pane_id,
                ..
            } => crate::domain::protocol::Request::ClosePane {
                domain: domain_id.clone(),
                pane: *remote_pane_id,
            },
            Self::CloseTab {
                domain_id,
                remote_tab_id,
                ..
            } => crate::domain::protocol::Request::CloseTab {
                domain: domain_id.clone(),
                tab: *remote_tab_id,
            },
        }
    }
}

impl From<accesskit_winit::Event> for AppEvent {
    fn from(event: accesskit_winit::Event) -> Self {
        Self::AccessKit(event)
    }
}

pub(super) struct ControlInventoryBudget {
    tabs: usize,
    panes: usize,
    dividers: usize,
    metadata_bytes: usize,
}

impl Default for ControlInventoryBudget {
    fn default() -> Self {
        Self {
            tabs: crate::control::protocol::MAX_TOTAL_TABS,
            panes: crate::control::protocol::MAX_TOTAL_PANES,
            dividers: crate::control::protocol::MAX_TOTAL_DIVIDERS,
            metadata_bytes: crate::control::protocol::MAX_METADATA_BYTES,
        }
    }
}

impl App {
    fn control_inventory(
        &self,
        id: u64,
        budget: &mut ControlInventoryBudget,
    ) -> crate::control::protocol::WindowInventory {
        use crate::control::protocol::{
            DividerInventory, DividerKind, MAX_DIVIDERS_PER_TAB, MAX_PANES_PER_TAB,
            MAX_TABS_PER_WINDOW, PaneInventory, TabInventory, TabKind, WindowInventory,
        };

        let (pixel_width, pixel_height) = self
            .renderer
            .as_ref()
            .map(crate::renderer::Renderer::window_size)
            .unwrap_or((self.config.window.width, self.config.window.height));

        let Some(tabs) = self.tabs.as_ref() else {
            let (workspace_id, truncated) =
                self.current_workspace_id
                    .as_deref()
                    .map_or((None, false), |workspace| {
                        let (workspace, truncated) =
                            control_metadata(workspace, &mut budget.metadata_bytes);
                        (Some(workspace), truncated)
                    });
            return WindowInventory {
                id,
                focused: self.window_focused,
                visible: if self.launch.quick_access {
                    self.quick_access_visible
                } else {
                    self.window
                        .as_ref()
                        .and_then(|window| window.is_visible())
                        .unwrap_or(self.window.is_some())
                },
                quick_access: self.launch.quick_access,
                workspace_id,
                pixel_width,
                pixel_height,
                tabs: Vec::new(),
                truncated,
            };
        };
        let active = tabs.active_tab_id();
        let all_tabs = tabs.all_tabs();
        let tab_limit = MAX_TABS_PER_WINDOW.min(budget.tabs);
        let mut truncated = all_tabs.len() > tab_limit;
        budget.tabs -= all_tabs.len().min(tab_limit);
        let tabs = all_tabs
            .iter()
            .take(tab_limit)
            .map(|tab| {
                let mut tab_truncated = false;
                let mut panes = if let Some(manager) = &tab.pane_manager {
                    let focused = manager.focused_id();
                    let all = manager.iter_panes();
                    let pane_limit = MAX_PANES_PER_TAB.min(budget.panes);
                    tab_truncated |= all.len() > pane_limit;
                    budget.panes -= all.len().min(pane_limit);
                    all.into_iter()
                        .take(pane_limit)
                        .map(|pane| PaneInventory {
                            id: pane.id.0,
                            title: pane.title.as_deref().map(|title| {
                                let (title, title_truncated) =
                                    control_metadata(title, &mut budget.metadata_bytes);
                                tab_truncated |= title_truncated;
                                title
                            }),
                            focused: pane.id == focused,
                            closed: pane.closed,
                            cols: pane.terminal.cols().min(u16::MAX as usize) as u16,
                            rows: pane.terminal.rows().min(u16::MAX as usize) as u16,
                        })
                        .collect::<Vec<_>>()
                } else {
                    let pane = tab.focused_terminal().and_then(|terminal| {
                        if budget.panes == 0 {
                            tab_truncated = true;
                            return None;
                        }
                        budget.panes -= 1;
                        Some(PaneInventory {
                            id: tab.single_pane_id().0,
                            title: None,
                            focused: true,
                            closed: tab.closed,
                            cols: terminal.cols().min(u16::MAX as usize) as u16,
                            rows: terminal.rows().min(u16::MAX as usize) as u16,
                        })
                    });
                    pane.into_iter().collect()
                };
                panes.sort_by_key(|pane| pane.id);
                let mut dividers = tab
                    .pane_manager
                    .as_ref()
                    .map(|manager| {
                        let all = manager.get_dividers();
                        let divider_limit = MAX_DIVIDERS_PER_TAB
                            .min(budget.dividers)
                            .min(panes.len().saturating_sub(1));
                        tab_truncated |= all.len() > divider_limit;
                        budget.dividers -= all.len().min(divider_limit);
                        all.into_iter()
                            .take(divider_limit)
                            .map(|divider| {
                                let (id, kind) = match divider.target {
                                    crate::panes::DividerTarget::Local(id) => {
                                        (id, DividerKind::Local)
                                    }
                                    crate::panes::DividerTarget::Domain(id) => {
                                        (u64::from(id), DividerKind::LiveDomain)
                                    }
                                };
                                DividerInventory {
                                    id,
                                    kind,
                                    direction: if divider.vertical {
                                        crate::control::protocol::SplitDirection::Vertical
                                    } else {
                                        crate::control::protocol::SplitDirection::Horizontal
                                    },
                                    ratio_millis: divider.ratio_millis,
                                }
                            })
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_default();
                dividers.sort_by_key(|divider| divider.id);
                let domain_id = tab.live_domain_tab_route().map(|(domain, _)| {
                    let (domain, domain_truncated) =
                        control_metadata(domain, &mut budget.metadata_bytes);
                    tab_truncated |= domain_truncated;
                    domain
                });
                let kind = if domain_id.is_some() {
                    TabKind::LiveDomain
                } else {
                    match &tab.tab_type {
                        crate::tabs::TabType::Shell => TabKind::Shell,
                        crate::tabs::TabType::Profile { .. }
                        | crate::tabs::TabType::Serial { .. } => TabKind::Profile,
                        crate::tabs::TabType::Plugin { .. } => TabKind::Plugin,
                    }
                };
                let (title, title_truncated) =
                    control_metadata(&tab.title, &mut budget.metadata_bytes);
                tab_truncated |= title_truncated;
                TabInventory {
                    id: tab.id as u64,
                    title,
                    kind,
                    active: active == Some(tab.id),
                    pinned: tab.pinned,
                    activity: tab.has_activity,
                    closed: tab.closed || tab.closing_at.is_some(),
                    domain_id,
                    panes,
                    dividers,
                    truncated: tab_truncated,
                }
            })
            .collect();
        let (workspace_id, workspace_truncated) =
            self.current_workspace_id
                .as_deref()
                .map_or((None, false), |workspace| {
                    let (workspace, was_truncated) =
                        control_metadata(workspace, &mut budget.metadata_bytes);
                    (Some(workspace), was_truncated)
                });
        truncated |= workspace_truncated;
        WindowInventory {
            id,
            focused: self.window_focused,
            visible: if self.launch.quick_access {
                self.quick_access_visible
            } else {
                self.window
                    .as_ref()
                    .and_then(|window| window.is_visible())
                    .unwrap_or(self.window.is_some())
            },
            quick_access: self.launch.quick_access,
            workspace_id,
            pixel_width,
            pixel_height,
            tabs,
            truncated,
        }
    }
}

fn control_metadata(value: &str, remaining: &mut usize) -> (String, bool) {
    const MAX_FIELD_BYTES: usize = 1_024;
    let limit = (*remaining).min(MAX_FIELD_BYTES);
    let mut output = String::with_capacity(value.len().min(limit));
    let mut truncated = false;
    for character in value.chars() {
        if character.is_control() {
            truncated = true;
            continue;
        }
        if output.len() + character.len_utf8() > limit {
            truncated = true;
            break;
        }
        output.push(character);
    }
    *remaining -= output.len();
    (output, truncated)
}

#[cfg(test)]
mod control_inventory_tests {
    use super::control_metadata;

    #[test]
    fn metadata_removes_controls_without_splitting_utf8_or_exceeding_budget() {
        let mut remaining = 5;
        let (output, truncated) = control_metadata("a\u{1b}界bc", &mut remaining);
        assert_eq!(output, "a界b");
        assert_eq!(remaining, 0);
        assert!(truncated);
    }
}

static EVENT_LOOP_PROXY: OnceLock<EventLoopProxy<AppEvent>> = OnceLock::new();
static PTY_WAKE_PENDING: AtomicBool = AtomicBool::new(false);
static TRANSCRIPT_WAKE_PENDING: AtomicBool = AtomicBool::new(false);

pub(crate) fn install_event_loop_proxy(proxy: EventLoopProxy<AppEvent>) {
    if EVENT_LOOP_PROXY.set(proxy).is_err() {
        log::warn!("Event-loop proxy was already installed");
    }
}

pub(crate) fn pty_event_waker() -> Option<Arc<dyn Fn() + Send + Sync>> {
    EVENT_LOOP_PROXY.get()?;
    Some(Arc::new(|| {
        if PTY_WAKE_PENDING.swap(true, Ordering::AcqRel) {
            return;
        }
        let sent = EVENT_LOOP_PROXY
            .get()
            .is_some_and(|proxy| proxy.send_event(AppEvent::Pty).is_ok());
        if !sent {
            PTY_WAKE_PENDING.store(false, Ordering::Release);
        }
    }))
}

pub(crate) fn git_event_waker() -> Option<Arc<dyn Fn() + Send + Sync>> {
    let proxy = EVENT_LOOP_PROXY.get()?.clone();
    Some(Arc::new(move || {
        let _ = proxy.send_event(AppEvent::Git);
    }))
}

pub(crate) fn transcript_event_waker() -> Option<Arc<dyn Fn() + Send + Sync>> {
    EVENT_LOOP_PROXY.get()?;
    Some(Arc::new(|| {
        if TRANSCRIPT_WAKE_PENDING.swap(true, Ordering::AcqRel) {
            return;
        }
        let sent = EVENT_LOOP_PROXY
            .get()
            .is_some_and(|proxy| proxy.send_event(AppEvent::Transcript).is_ok());
        if !sent {
            TRANSCRIPT_WAKE_PENDING.store(false, Ordering::Release);
        }
    }))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct CopyModeTarget {
    tab: usize,
    pane: Option<PaneId>,
}

#[derive(Debug, Clone)]
struct TabRenameState {
    tab_id: usize,
    original: Option<String>,
    input: String,
    max_chars: usize,
}

#[derive(Debug, Clone, Copy)]
struct DividerDrag {
    tab_id: usize,
    target: crate::panes::DividerTarget,
    last_ratio_millis: u16,
}

#[derive(Debug, Clone)]
struct TabDrag {
    tab_id: usize,
    live: Option<LiveDomainTabDrag>,
}

#[derive(Debug, Clone)]
struct LiveDomainTabDrag {
    domain_id: String,
    remote_tab_id: u32,
    initial_index: u16,
}

#[derive(Debug, Clone, Default)]
enum BroadcastState {
    #[default]
    Inactive,
    Armed {
        tab_id: usize,
        expires: Instant,
        targets: Vec<PaneId>,
    },
    Active {
        tab_id: usize,
        targets: Vec<PaneId>,
    },
}

#[derive(Debug, Clone)]
struct PendingProfileSelection {
    request_id: u64,
    plugin_id: String,
    profiles: Vec<String>,
}

#[derive(Debug, Clone)]
struct PendingRuntimePluginManagement {
    request_id: u64,
    view: RuntimePluginManagerView,
}

#[derive(Debug, Clone)]
enum RuntimePluginManagerView {
    List {
        page: usize,
    },
    Detail {
        plugin_id: String,
        page: usize,
    },
    Settings {
        plugin_id: String,
        page: usize,
        settings_page: usize,
    },
    Setting {
        plugin_id: String,
        page: usize,
        settings_page: usize,
        value_page: usize,
        setting: crate::plugin::runtime::DeclarativeSetting,
    },
    EditSetting {
        plugin_id: String,
        page: usize,
        settings_page: usize,
        setting: crate::plugin::runtime::DeclarativeSetting,
    },
    Confirm {
        plugin_id: String,
        page: usize,
        operation: RuntimePluginOperation,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RuntimePluginOperation {
    Update,
    ResetSettings,
    Remove,
}

#[derive(Debug, Clone)]
struct PendingFileUpload {
    request_id: u64,
    source: PathBuf,
    authority: crate::domain::DomainAuthority,
    domain_id: String,
    pane_id: u32,
    name: String,
    source_kind: FileUploadSourceKind,
    source_identity: Option<(u64, u64)>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FileUploadSourceKind {
    File { bytes: u64 },
    Directory,
}

#[derive(Debug, Clone)]
pub(crate) struct FileUploadOutcome {
    path: String,
    bytes: u64,
    entries: Option<u32>,
    insertion_error: Option<String>,
}

#[derive(Debug, Clone)]
pub(crate) enum FileUploadCompletion {
    Uploaded(FileUploadOutcome),
    Cancelled { bytes_received: u64 },
}

#[derive(Debug)]
struct ActiveFileUpload {
    request_id: u64,
    name: String,
    bytes_sent: u64,
    total_bytes: Option<u64>,
    cancellation: Arc<AtomicBool>,
    cancel_requested: bool,
    progress_toast_id: u64,
}

fn clear_pty_wake_pending() {
    PTY_WAKE_PENDING.store(false, Ordering::Release);
}

fn clear_transcript_wake_pending() {
    TRANSCRIPT_WAKE_PENDING.store(false, Ordering::Release);
}

/// Opt-in native key-event-to-surface-present probe used by the release
/// benchmark runner. The disabled production path is one `Option` check.
#[derive(Debug, Default)]
struct NativePresentationLatencyProbe {
    output: Option<PathBuf>,
    pending: Option<Instant>,
}

impl NativePresentationLatencyProbe {
    fn from_environment(enabled: bool) -> Self {
        Self {
            output: enabled
                .then(|| std::env::var_os("TERMERIC_BENCHMARK_KEY_TO_PRESENT_PATH"))
                .flatten()
                .map(PathBuf::from),
            pending: None,
        }
    }

    fn begin(&mut self) {
        if self.output.is_some() {
            self.pending = Some(Instant::now());
        }
    }

    fn finish_after_terminal_present(&mut self, frame_reasons: &str) {
        if !frame_reasons
            .split(',')
            .any(|reason| reason == "terminal-output")
        {
            return;
        }
        let (Some(output), Some(started)) = (&self.output, self.pending.take()) else {
            return;
        };
        let elapsed_ms = started.elapsed().as_secs_f64() * 1_000.0;
        let result = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(output)
            .and_then(|mut file| writeln!(file, "{elapsed_ms:.6}"));
        if let Err(error) = result {
            log::error!(
                "Failed to write native presentation-latency sample to {}: {error}",
                output.display()
            );
            self.output = None;
        }
    }
}

/// Opt-in rendered-frame counter for the displayed idle benchmark. Production
/// does no filesystem work unless the explicit benchmark path is present.
#[derive(Debug, Default)]
struct NativeFrameStatsProbe {
    output: Option<PathBuf>,
}

impl NativeFrameStatsProbe {
    fn from_environment(enabled: bool) -> Self {
        Self {
            output: enabled
                .then(|| std::env::var_os("TERMERIC_BENCHMARK_FRAME_STATS_PATH"))
                .flatten()
                .map(PathBuf::from),
        }
    }

    fn record(&mut self, rendered_frames: u64) {
        let Some(output) = &self.output else {
            return;
        };
        let temporary = output.with_extension("tmp");
        let result = std::fs::write(&temporary, format!("{rendered_frames}\n"))
            .and_then(|()| std::fs::rename(&temporary, output));
        if let Err(error) = result {
            log::error!(
                "Failed to write native frame-count sample to {}: {error}",
                output.display()
            );
            self.output = None;
        }
    }
}

/// Main application state
///
/// IMPORTANT: Drop order matters! The renderer's wgpu Surface holds a reference
/// to the window, so the renderer MUST be dropped before the window.
/// We implement Drop explicitly to guarantee this ordering.
pub struct App {
    /// Window-local startup controls parsed before the event loop starts.
    launch: crate::cli::WindowLaunch,
    /// True when the process-level application owns event-loop exit policy.
    managed_window: bool,
    /// Set by the New Window command and drained by the process controller.
    request_new_window: bool,
    /// Set by the cross-window pane command and drained atomically by the
    /// process-level controller after the originating window event.
    request_move_pane_to_next_window: bool,
    /// Set by the cross-window tab command and drained atomically by the
    /// process-level controller after the originating window event.
    request_move_tab_to_next_window: bool,
    /// Explicit launch requested by an in-app workflow such as domain attach.
    requested_window_launch: Option<crate::cli::WindowLaunch>,
    /// Only the sole default startup window may consume automatic restore.
    allow_auto_restore: bool,
    /// Auto-expanded workspace windows fall back to a shell on recovery
    /// failure; an explicit CLI --workspace remains a hard startup error.
    allow_workspace_fallback: bool,
    /// False when one window is intentionally closed while siblings remain.
    retain_workspace_on_exit: bool,
    /// Archive identity owned by this native window, independent of global
    /// most-recent state used for startup compatibility.
    current_workspace_id: Option<String>,
    /// Headless integration controllers never perform implicit disk writes.
    automatic_persistence: bool,
    /// Application configuration
    #[allow(dead_code)]
    config: Config,
    /// The window (created on resume)
    window: Option<Arc<Window>>,
    /// Last title applied to the native window. Tab/process title updates can
    /// arrive at PTY speed, so avoid redundant compositor round trips.
    last_native_window_title: Option<String>,
    /// Requested borderless-fullscreen state for this native window. This is
    /// transient view state and is deliberately absent from workspace files.
    fullscreen_requested: bool,
    /// Requested visibility for the dedicated quick-access window. Native
    /// visibility queries are optional on some compositors.
    quick_access_visible: bool,
    /// Disabled unless the process-level release benchmark requests native
    /// key-to-present samples through an isolated output path.
    native_presentation_latency: NativePresentationLatencyProbe,
    /// Disabled unless the process-level release benchmark requests exact
    /// rendered-frame counts through an isolated output path.
    native_frame_stats: NativeFrameStatsProbe,
    /// The renderer
    renderer: Option<Renderer>,
    /// Identifies callbacks belonging to the currently installed WGPU device.
    renderer_generation: u64,
    /// Invalidates stale asynchronous background-image decodes.
    background_image_generation: u64,
    /// Only one bounded decoder worker may be active per native window.
    background_image_loading: bool,
    /// Coalesces changes observed while the decoder worker is active.
    background_image_pending: bool,
    /// Configured path represented by the current generation.
    background_image_path: Option<PathBuf>,
    /// Last source-file revision observed by the hot-reload poller.
    last_background_image_mtime: Option<std::time::SystemTime>,
    /// Prevents repeated driver failures from creating a recovery spin loop.
    renderer_recovery: renderer_recovery::RendererRecoveryPolicy,
    /// Native screen-reader bridge, created before the window is shown.
    accessibility_adapter: Option<accesskit_winit::Adapter>,
    /// Incremental semantic-tree state.
    accessibility: accessibility::AccessibilityState,
    /// Latest preferences reported by the native desktop. Missing values are
    /// treated as no preference when the persisted policy follows the system.
    system_accessibility: system_accessibility::SystemAccessibilityPreferences,
    /// Tab manager for multiple terminals
    tabs: Option<TabManager>,
    /// Whether the application should exit
    should_exit: bool,
    /// Actionable native startup failure returned after Winit stops. Keeping
    /// this separate from `should_exit` prevents renderer/window errors from
    /// being reduced to a successful process exit after an invisible window.
    startup_error: Option<String>,
    /// Surface rendering is paused until a real resize safely reconfigures it.
    surface_lost: bool,
    /// First observation of the current surface loss. A single deadline-driven
    /// whole-renderer recovery prevents indefinite suspension when no resize
    /// follows, while still allowing Winit's close/resize event to win.
    surface_lost_at: Option<Instant>,
    /// Terminal dimensions
    cols: usize,
    rows: usize,
    /// Cursor animation start time (for smooth blink)
    cursor_blink_start: Instant,
    /// Cursor blink period in seconds
    cursor_blink_period: f32,
    /// Blink phase drawn in the most recent frame.
    last_cursor_blink_visible: bool,
    /// Stable origin for SGR slow/rapid blink paint phases.
    text_blink_start: Instant,
    /// SGR blink phases drawn in the most recent frame.
    last_text_blink_phases: TextBlinkPhases,
    /// Animated cursor display position (for smooth movement)
    cursor_display_col: f32,
    cursor_display_row: f32,
    /// Last frame time for delta calculation
    last_frame_time: Instant,
    /// Last time we polled the config file for hot-reload
    last_config_check: Instant,
    /// Last observed mtime of the config file (for change detection)
    last_config_mtime: Option<std::time::SystemTime>,
    /// Last installed-plugin registry mtime for live enable/update/remove.
    last_plugin_registry_mtime: Option<std::time::SystemTime>,
    /// File backing the active custom/explicit theme, when applicable.
    active_theme_path: Option<std::path::PathBuf>,
    /// Last observed mtime of the active file-backed theme.
    last_theme_mtime: Option<std::time::SystemTime>,
    /// Last known cursor visibility (for snap detection)
    last_cursor_visible: bool,
    /// Last active tab ID (for animation state)
    last_active_tab_id: Option<usize>,
    /// Highest tab ID we've seen (to detect truly new tabs)
    max_seen_tab_id: Option<usize>,
    /// Mouse input state
    mouse_state: MouseState,
    /// Current keyboard modifiers
    modifiers: ModifiersState,
    /// Platform input-method state. Preedit remains local until commit.
    ime: ime::ImeState,
    /// Whether the layout's AltGraph modifier key is currently held.
    alt_graph_active: bool,
    /// Clipboard
    clipboard: Option<arboard::Clipboard>,
    /// Last cursor screen position (for mouse input events)
    last_cursor_pos: Option<(f64, f64)>,
    /// Native file drag is hovering this window. It owns the cursor until the
    /// compositor reports a drop or cancellation.
    file_drop_hovered: bool,
    /// Whether mouse is currently pressed (for selection)
    mouse_pressed: bool,
    /// Button currently held by an application mouse-reporting sequence.
    terminal_mouse_button: Option<MouseReportButton>,
    /// Whether we're dragging the scrollbar
    scrollbar_dragging: bool,
    /// Exact pane divider selected at mouse-down.
    divider_dragging: Option<DividerDrag>,
    /// Drag source for direct tab-bar reordering.
    tab_dragging: Option<TabDrag>,
    /// Inline tab rename capture, committed by Enter and cancelled by Escape.
    tab_rename: Option<TabRenameState>,
    /// Deliberately confirmed multi-pane keyboard broadcast state.
    broadcast: BroadcastState,
    /// Live-domain mutations awaiting a daemon response. One in-flight change
    /// per domain prevents local controls from arriving out of user order.
    pending_live_domain_controls: std::collections::HashSet<LiveDomainControl>,
    /// Latest whole-domain revision applied by this window. Replayed or
    /// reordered subscriber frames are ignored before they touch local PTYs.
    live_domain_revisions: std::collections::HashMap<String, u64>,
    /// Domain subscriptions that have reported a transport failure and have
    /// not yet reconciled a fresh authoritative snapshot.
    disconnected_live_domains: std::collections::HashSet<String>,
    /// Last authoritative snapshot, used to roll back optimistic live-domain
    /// divider and tab-metadata/order previews when a request is rejected.
    live_domain_topologies: std::collections::HashMap<String, crate::domain::model::DomainTopology>,
    /// Shared cancellation flag for this window's blocking topology reader.
    live_domain_topology_alive: Arc<AtomicBool>,
    /// Authority used by every control, focus, and topology channel for this
    /// attached domain window. Domain IDs are not globally unique across hosts.
    live_domain_authority: Option<crate::domain::DomainAuthority>,
    /// Exact local relay command, including any SSH authority prefix.
    live_domain_relay_command: Option<crate::domain::DomainRelayCommand>,
    /// Default font size (for reset)
    default_font_size: f32,
    /// When copy flash started (for animation), None if not flashing
    copy_flash_start: Option<Instant>,
    /// Whether the window is currently focused
    window_focused: bool,
    /// When focus state last changed (for smooth transition)
    focus_change_time: Instant,
    /// Active UI theme
    theme: Theme,
    /// Settings panel state
    settings_panel: SettingsPanel,
    /// Search state
    search: Search,
    /// Cursor position in search input
    search_cursor_pos: usize,
    /// Keyboard copy mode is bound to the pane that owned focus on entry.
    copy_mode: CopyMode,
    copy_mode_target: Option<CopyModeTarget>,
    /// Validated and precompiled semantic patterns used by quick select.
    compiled_copy_patterns: Vec<(String, regex::Regex)>,
    /// Notification manager for desktop notifications
    notifications: Notifications,
    /// Bell handler (visual/audio bell)
    bell: Bell,
    /// Unified shortcut registry (all bindable actions)
    shortcut_registry: ShortcutRegistry,
    /// Plugin/custom keybinding manager (SendText, RunCommand, PluginCommand)
    keybinding_manager: KeyBindingManager,
    /// Application runtime state (for first boot tracking, etc.)
    app_state: AppState,
    /// Whether the session dropdown is currently open
    session_dropdown_open: bool,
    /// Cached list of session names for the dropdown
    session_dropdown_items: Vec<(String, String)>,
    /// Mouse-hovered row in the session dropdown
    session_dropdown_hovered: Option<usize>,
    /// Session management panel
    session_panel: crate::session::panel::SessionPanel,
    /// Custom commands panel
    commands_panel: crate::ui::commands::panel::CommandsPanel,
    /// Buffer tracking what the user has typed on the current line (for alias expansion)
    input_line_buffer: String,
    /// Git repository watcher (tracks branch, dirty status per tab)
    git_watcher: crate::git::GitWatcher,
    /// Current git completion candidates
    git_completions: Vec<String>,
    /// Selected index in git completion dropdown (None = no selection)
    git_completion_index: Option<usize>,
    /// Whether git completion dropdown is visible
    git_completions_visible: bool,
    /// Which tab bar element is currently hovered (for icon animations)
    tab_bar_hover: Option<crate::renderer::TabBarClick>,
    /// Smooth hover animation progress (0.0 = not hovered, 1.0 = fully hovered)
    tab_bar_hover_progress: f32,
    /// Plugin manager
    plugin_manager: PluginManager,
    /// Bounded off-UI-thread executor for declarative lifecycle handlers.
    structured_plugin_worker: crate::plugin::structured::StructuredPluginWorker,
    /// Invalidates responses queued before a plugin registry/config reload.
    plugin_runtime_generation: u64,
    /// Command palette
    command_palette: CommandPalette,
    /// Local/saved-SSH live-domain discovery and attachment palette.
    domain_picker: DomainPicker,
    /// Shortcuts panel (standalone centered modal)
    shortcuts_panel: ShortcutsPanel,
    /// Bounded information, error, and choice overlay
    info_overlay: InfoOverlay,
    /// Transient toast notifications stacked in the bottom-right
    toasts: crate::ui::toast::ToastManager,
    /// Diff review overlay
    diff_review: DiffReviewOverlay,
    /// Context picker modal
    context_picker: ContextPicker,
    /// Guardrail confirmation overlay
    guardrail: GuardrailOverlay,
    /// Request-scoped action context for an info-overlay profile choice.
    pending_profile_selection: Option<PendingProfileSelection>,
    /// Request-scoped navigation and confirmation state for management of
    /// installed third-party plugins.
    pending_runtime_plugin_management: Option<PendingRuntimePluginManagement>,
    /// Temporary registry and installation roots used only by hermetic tests.
    #[cfg(test)]
    runtime_plugin_test_paths: Option<(PathBuf, PathBuf)>,
    /// Exact source and live-domain destination awaiting explicit upload consent.
    pending_file_upload: Option<PendingFileUpload>,
    /// At most one background file upload may be active per native window.
    active_file_upload: Option<ActiveFileUpload>,
    /// Harvest mode (command visualization)
    harvest: Option<HarvestMode>,
    /// URL detector for auto-detecting URLs in terminal output
    url_detector: UrlDetector,
    /// Cached URL lookup table, reused between frames
    url_cache: UrlCache,
    /// Per-grid URL caches used when multiple panes are visible.
    pane_url_caches: std::collections::HashMap<usize, UrlCache>,
    /// Central dirty-state aggregator and frame diagnostics.
    redraw_scheduler: RedrawScheduler,
    /// Whether the native window is currently occluded.
    window_occluded: bool,
}

impl App {
    /// Construct the controller without a window, renderer, PTY, clipboard,
    /// or persisted app-state dependency. This is the integration-test spine
    /// for dispatch and live configuration behavior.
    #[cfg(test)]
    pub(crate) fn new_headless(config: Config) -> Result<Self> {
        config.validate()?;
        Self::from_config(config, None, false)
    }

    fn from_config(
        mut config: Config,
        mut startup_config_warning: Option<String>,
        use_system_state: bool,
    ) -> Result<Self> {
        let default_font_size = config.font.size;
        let background_image_path = config.window.background_image.path.clone();
        let last_background_image_mtime = background_image_path.as_deref().and_then(|path| {
            std::fs::metadata(crate::renderer::resolve_background_image_path(path))
                .and_then(|metadata| metadata.modified())
                .ok()
        });
        let cursor_blink_period = (config.cursor.blink_rate as f32 / 1000.0) * 2.0;
        let (theme, active_theme_path) =
            match Theme::resolve(&config.theme.name, config.theme.path.as_deref()) {
                Ok(resolved) => resolved,
                Err(error) => {
                    let warning = format!("Theme could not be loaded; using turmeric: {error:#}");
                    startup_config_warning = Some(match startup_config_warning {
                        Some(existing) => format!("{existing}\n{warning}"),
                        None => warning,
                    });
                    config.theme = crate::config::ThemeConfig::default();
                    (Theme::default(), None)
                }
            };
        let last_theme_mtime = active_theme_path.as_ref().and_then(|path| {
            std::fs::metadata(path)
                .and_then(|metadata| metadata.modified())
                .ok()
        });

        // Initialize bell from config
        let bell = Bell::new(BellConfig {
            visual: config.bell.visual,
            audio: config.bell.audio,
            visual_duration_ms: config.bell.visual_duration_ms,
            ..Default::default()
        });

        // Initialize clipboard (may fail on some systems)
        let clipboard = use_system_state
            .then(arboard::Clipboard::new)
            .transpose()
            .ok()
            .flatten();

        // Initialize unified shortcut registry with defaults + user overrides
        let mut shortcut_registry = ShortcutRegistry::new();
        shortcut_registry.apply_overrides(&config.shortcuts);

        // Initialize plugin/custom keybinding manager
        let mut keybinding_manager = KeyBindingManager::new();
        keybinding_manager.load(config.keybindings.clone());

        let result: Result<Self> = Ok(Self {
            launch: crate::cli::WindowLaunch::default(),
            managed_window: false,
            request_new_window: false,
            request_move_pane_to_next_window: false,
            request_move_tab_to_next_window: false,
            requested_window_launch: None,
            allow_auto_restore: true,
            allow_workspace_fallback: false,
            retain_workspace_on_exit: true,
            current_workspace_id: None,
            automatic_persistence: use_system_state,
            config,
            window: None,
            last_native_window_title: None,
            fullscreen_requested: false,
            quick_access_visible: true,
            native_presentation_latency: NativePresentationLatencyProbe::from_environment(
                use_system_state,
            ),
            native_frame_stats: NativeFrameStatsProbe::from_environment(use_system_state),
            renderer: None,
            renderer_generation: 0,
            background_image_generation: 0,
            background_image_loading: false,
            background_image_pending: false,
            background_image_path,
            last_background_image_mtime,
            renderer_recovery: renderer_recovery::RendererRecoveryPolicy::default(),
            accessibility_adapter: None,
            accessibility: accessibility::AccessibilityState::default(),
            system_accessibility: system_accessibility::SystemAccessibilityPreferences::default(),
            tabs: None,
            should_exit: false,
            startup_error: None,
            surface_lost: false,
            surface_lost_at: None,
            cols: 80,
            rows: 24,
            cursor_blink_start: Instant::now(),
            cursor_blink_period,
            last_cursor_blink_visible: true,
            text_blink_start: Instant::now(),
            last_text_blink_phases: TextBlinkPhases::default(),
            cursor_display_col: 0.0,
            cursor_display_row: 0.0,
            last_frame_time: Instant::now(),
            last_config_check: Instant::now(),
            last_config_mtime: use_system_state
                .then(|| std::fs::metadata(crate::config::Config::config_path()))
                .and_then(Result::ok)
                .and_then(|metadata| metadata.modified().ok()),
            last_plugin_registry_mtime: use_system_state
                .then(|| std::fs::metadata(crate::plugin::runtime::registry_path()))
                .and_then(Result::ok)
                .and_then(|metadata| metadata.modified().ok()),
            active_theme_path,
            last_theme_mtime,
            last_cursor_visible: true,
            last_active_tab_id: None,
            max_seen_tab_id: None,
            mouse_state: MouseState::new(),
            modifiers: ModifiersState::empty(),
            ime: ime::ImeState::default(),
            alt_graph_active: false,
            clipboard,
            last_cursor_pos: None,
            file_drop_hovered: false,
            mouse_pressed: false,
            terminal_mouse_button: None,
            scrollbar_dragging: false,
            divider_dragging: None,
            tab_dragging: None,
            tab_rename: None,
            broadcast: BroadcastState::Inactive,
            pending_live_domain_controls: std::collections::HashSet::new(),
            live_domain_revisions: std::collections::HashMap::new(),
            disconnected_live_domains: std::collections::HashSet::new(),
            live_domain_topologies: std::collections::HashMap::new(),
            live_domain_topology_alive: Arc::new(AtomicBool::new(true)),
            live_domain_authority: None,
            live_domain_relay_command: None,
            default_font_size,
            copy_flash_start: None,
            window_focused: true,
            focus_change_time: Instant::now(),
            theme,
            settings_panel: SettingsPanel::new(),
            search: Search::new(),
            search_cursor_pos: 0,
            copy_mode: CopyMode::default(),
            copy_mode_target: None,
            compiled_copy_patterns: Vec::new(),
            notifications: Notifications::new(),
            bell,
            shortcut_registry,
            keybinding_manager,
            app_state: if use_system_state {
                AppState::load()
            } else {
                AppState::default()
            },
            session_dropdown_open: false,
            session_dropdown_items: Vec::new(),
            session_dropdown_hovered: None,
            session_panel: crate::session::panel::SessionPanel::new(),
            commands_panel: crate::ui::commands::panel::CommandsPanel::new(),
            input_line_buffer: String::new(),
            git_watcher: crate::git::GitWatcher::new(git_event_waker()),
            git_completions: Vec::new(),
            git_completion_index: None,
            git_completions_visible: false,
            tab_bar_hover: None,
            tab_bar_hover_progress: 0.0,
            plugin_manager: PluginManager::new(),
            structured_plugin_worker: crate::plugin::structured::StructuredPluginWorker::new()
                .map_err(anyhow::Error::msg)?,
            plugin_runtime_generation: 1,
            command_palette: CommandPalette::new(),
            domain_picker: DomainPicker::new(),
            shortcuts_panel: ShortcutsPanel::new(),
            info_overlay: InfoOverlay::new(),
            toasts: crate::ui::toast::ToastManager::new(),
            diff_review: DiffReviewOverlay::new(),
            context_picker: ContextPicker::new(),
            guardrail: GuardrailOverlay::new(),

            pending_profile_selection: None,
            pending_runtime_plugin_management: None,
            #[cfg(test)]
            runtime_plugin_test_paths: None,
            pending_file_upload: None,
            active_file_upload: None,
            harvest: None,
            url_detector: UrlDetector::new(),
            url_cache: UrlCache::new(0),
            pane_url_caches: std::collections::HashMap::new(),
            redraw_scheduler: RedrawScheduler::default(),
            window_occluded: false,
        });

        let mut app = result?;

        // Initialize command palette and apply all runtime config through the
        // same path used by reload, settings changes, and reset.
        app.command_palette.register_commands(core_commands(
            app.config.harvest.enabled,
            &app.shortcut_registry,
        ));
        app.apply_config_side_effects();
        if let Some(warning) = startup_config_warning {
            log::error!("{}", warning);
            app.toasts.error(warning);
        }

        Ok(app)
    }

    /// Load window icon from embedded PNG data
    ///
    /// The icon is embedded directly in the binary at compile time,
    /// so it works on all platforms without requiring external files.
    fn load_icon() -> Option<Icon> {
        // Icon is embedded in the binary at compile time
        let icon_bytes = include_bytes!("../../../assets/icons/termeric.png");

        match image::load_from_memory(icon_bytes) {
            Ok(img) => {
                // Resize to 256x256 for optimal display across platforms
                let img = img.resize_exact(256, 256, image::imageops::FilterType::Lanczos3);
                let rgba = img.to_rgba8();
                let (width, height) = rgba.dimensions();
                let icon_data = rgba.into_raw();

                match Icon::from_rgba(icon_data, width, height) {
                    Ok(icon) => {
                        log::info!("Successfully loaded window icon ({}x{})", width, height);
                        Some(icon)
                    }
                    Err(e) => {
                        log::error!("Failed to create icon from image data: {}", e);
                        None
                    }
                }
            }
            Err(e) => {
                log::error!("Failed to decode embedded icon image: {}", e);
                None
            }
        }
    }

    /// Initialize the renderer
    fn init_renderer(&mut self, window: Arc<Window>) -> Result<()> {
        let scale_factor = window.scale_factor() as f32;
        let window_id = window.id();
        let generation = self.renderer_generation;
        let failure_callback = EVENT_LOOP_PROXY.get().cloned().map(|proxy| {
            Arc::new(move |failure| {
                let _ = proxy.send_event(AppEvent::RendererDeviceLost {
                    window_id,
                    generation,
                    failure,
                });
            }) as crate::renderer::RendererFailureCallback
        });
        let mut renderer = Renderer::new(
            window.clone(),
            self.config.font.size,
            &self.config.font.family,
            &self.config.font.fallback,
            self.config.font.line_height,
            self.config.window.padding as f32,
            scale_factor,
            &self.theme,
            self.high_contrast_enabled(),
            self.config.font.ligatures,
            self.config.renderer.backend,
            failure_callback,
        )?;

        // Apply initial opacity from config
        renderer.set_opacity(self.config.window.opacity);
        renderer.set_bold_is_bright(self.config.font.bold_is_bright);
        renderer.set_tab_bar_position(self.config.window.tab_bar_position);
        renderer.set_background_image_layout(
            self.config.window.background_image.fit,
            self.config.window.background_image.opacity,
        );

        // Discover available monospace fonts and update settings panel
        let fonts = renderer.get_monospace_fonts();
        log::info!("Discovered {} monospace fonts", fonts.len());
        self.settings_panel.set_available_fonts(fonts);

        // Calculate grid size from renderer's cell size
        let size = window.inner_size();
        let (cols, rows) = renderer.calculate_grid_size(size.width, size.height);
        self.cols = cols;
        self.rows = rows;

        self.renderer = Some(renderer);
        self.window = Some(window);
        self.sync_background_image(true);

        log::info!("Renderer initialized, grid size: {}x{}", cols, rows);

        Ok(())
    }

    fn handle_renderer_device_lost(
        &mut self,
        generation: u64,
        failure: crate::renderer::RendererDeviceFailure,
    ) {
        if !renderer_recovery::is_current_unexpected_loss(
            generation,
            self.renderer_generation,
            failure.reason,
        ) {
            return;
        }
        self.recover_renderer(format!(
            "WGPU device lost ({:?}: {})",
            failure.reason, failure.message
        ));
    }

    fn recover_renderer(&mut self, cause: String) {
        if !self.renderer_recovery.admit(Instant::now()) {
            log::error!(
                "Renderer recovery stopped after three losses in 30 seconds; \
                 try `termeric --renderer software` or inspect graphics drivers"
            );
            self.should_exit = true;
            return;
        }
        let previous = self
            .renderer
            .as_ref()
            .map(|renderer| renderer.diagnostics().summary())
            .unwrap_or_else(|| "unknown renderer".into());
        log::warn!("{cause}; recreating renderer without restarting PTYs ({previous})");
        self.renderer.take();
        self.surface_lost = false;
        self.surface_lost_at = None;
        self.renderer_generation = self.renderer_generation.wrapping_add(1);
        let Some(window) = self.window.clone() else {
            self.should_exit = true;
            return;
        };
        match self.init_renderer(window.clone()) {
            Ok(()) => {
                let size = window.inner_size();
                self.handle_resize(size.width, size.height);
                if let Some(renderer) = &self.renderer {
                    log::info!(
                        "Renderer recovery complete: {}",
                        renderer.diagnostics().summary()
                    );
                }
                self.redraw_scheduler
                    .request(redraw_scheduler::RedrawReason::SurfaceRecovery);
                window.request_redraw();
            }
            Err(error) => {
                log::error!(
                    "Renderer recovery failed: {error:#}. Shells were preserved during the attempt, \
                     but no usable adapter remains; try `termeric --renderer software`"
                );
                self.should_exit = true;
            }
        }
    }

    /// Process PTY events for all tabs and panes
    fn process_pty_events(&mut self) -> bool {
        // Process events and collect results
        let (had_output, events, quick_exits) = {
            let Some(tabs) = &mut self.tabs else {
                return false;
            };
            let (had_output, events) = tabs.poll_events();

            // Detect plugin tabs that exited too quickly (< 5s) — before cleanup removes them
            let quick_exits = tabs.drain_quick_exit_plugin_info();
            (had_output, events, quick_exits)
        };

        let had_terminal_events = !events.is_empty();
        // App-mediated OSC 52 and dynamic-color replies must run before pane
        // cleanup so the stable origin remains addressable.
        for event in events {
            self.handle_terminal_event(event);
        }

        let (persistence_notices, user_notices, topology_changed, should_exit) = {
            let Some(tabs) = &mut self.tabs else {
                return false;
            };
            let auto_save = self.automatic_persistence
                && self.config.session.enabled
                && self.config.session.auto_save;
            let topology_changed =
                tabs.cleanup_closed(auto_save, self.config.session.save_scrollback);
            let should_exit = topology_changed && tabs.is_empty();
            let persistence_notices = tabs.drain_persistence_notices();
            let user_notices = tabs.drain_user_notices();
            (
                persistence_notices,
                user_notices,
                topology_changed,
                should_exit,
            )
        };
        if topology_changed {
            self.reconcile_broadcast_state();
        }
        let mut changed = had_output
            || had_terminal_events
            || !quick_exits.is_empty()
            || !persistence_notices.is_empty()
            || !user_notices.is_empty()
            || topology_changed
            || should_exit;

        self.show_persistence_notices(persistence_notices, true);
        for notice in user_notices {
            self.toasts.warning(notice);
        }

        // Show overlay for any plugin that exited immediately
        for (plugin_id, output_lines) in quick_exits {
            let display_name = match plugin_id.as_str() {
                "claude-code" => "Claude Code",
                "codex" => "Codex",
                _ => &plugin_id,
            };

            let mut overlay_lines = vec![
                format!("{} exited immediately.", display_name),
                String::new(),
            ];

            if output_lines.is_empty() {
                overlay_lines.push("No output was captured.".into());
                overlay_lines.push("Check that the tool is installed and".into());
                overlay_lines.push("configured correctly.".into());
            } else {
                overlay_lines.push("Output:".into());
                // Show up to 8 lines of output
                for line in output_lines.iter().take(8) {
                    overlay_lines.push(format!("  {}", line));
                }
                if output_lines.len() > 8 {
                    overlay_lines.push(format!("  ... ({} more lines)", output_lines.len() - 8));
                }
            }

            self.info_overlay
                .show(format!("{} failed to start", display_name), overlay_lines);
            if let Some(window) = &self.window {
                window.request_redraw();
            }
            log::warn!("Plugin '{}' exited within 5 seconds of launch", plugin_id);
        }

        // Observe every terminal independently so inactive tabs and panes do
        // not lose or duplicate semantic command regions.
        let mut completed_hooks = Vec::new();
        let show_indicators = self.harvest.is_some() && self.config.harvest.show_indicators;
        if let Some(tabs) = &mut self.tabs {
            for tab in tabs.all_tabs_mut() {
                if let Some(panes) = &mut tab.pane_manager {
                    for pane in panes.all_panes_mut() {
                        let regions = pane.harvest.observe(
                            &pane.terminal.shell_integration().prompt_tracker,
                            show_indicators,
                        );
                        for region in regions {
                            let (command, command_truncated) =
                                Self::extract_command_text(&pane.terminal, &region);
                            let (output, output_truncated) =
                                Self::extract_output_text(&pane.terminal, &region);
                            if let Some(harvest) = &mut self.harvest {
                                harvest.record_command(
                                    crate::harvest::HarvestSource {
                                        tab_id: tab.id,
                                        pane_id: Some(pane.id.0),
                                    },
                                    &region,
                                    command.clone(),
                                    output.clone(),
                                );
                            }
                            completed_hooks.push(crate::plugin::Hook::CommandExecuted {
                                region_id: region.id,
                                tab_id: tab.id,
                                pane_id: Some(pane.id.0),
                                command,
                                command_truncated,
                                output,
                                output_truncated,
                                content_available: region.content_available,
                                exit_code: region.exit_code,
                            });
                        }
                    }
                } else {
                    let regions = tab.observe_harvest(show_indicators);
                    for region in regions {
                        let ((command, command_truncated), (output, output_truncated)) =
                            tab.focused_terminal().map_or_else(
                                || ((String::new(), false), (String::new(), false)),
                                |terminal| {
                                    (
                                        Self::extract_command_text(terminal, &region),
                                        Self::extract_output_text(terminal, &region),
                                    )
                                },
                            );
                        if let Some(harvest) = &mut self.harvest {
                            harvest.record_command(
                                crate::harvest::HarvestSource {
                                    tab_id: tab.id,
                                    pane_id: None,
                                },
                                &region,
                                command.clone(),
                                output.clone(),
                            );
                        }
                        completed_hooks.push(crate::plugin::Hook::CommandExecuted {
                            region_id: region.id,
                            tab_id: tab.id,
                            pane_id: None,
                            command,
                            command_truncated,
                            output,
                            output_truncated,
                            content_available: region.content_available,
                            exit_code: region.exit_code,
                        });
                    }
                }
            }
        }
        changed |= !completed_hooks.is_empty();
        for hook in completed_hooks {
            self.dispatch_plugin_hook(hook);
        }

        // Check if all tabs are closed
        if should_exit {
            log::info!("All tabs closed, exiting");
            self.should_exit = true;
        }

        changed
    }

    /// Handle window resize
    fn handle_resize(&mut self, width: u32, height: u32) {
        // An in-flight divider drag would leave stale hit-test rects after
        // the geometry changes — cancel it so the next click starts fresh.
        self.divider_dragging = None;
        // Same idea for the scrollbar drag (the scrollbar extents move).
        self.scrollbar_dragging = false;
        // Resize renderer
        if let Some(renderer) = &mut self.renderer {
            renderer.resize(width, height);
        }
        self.sync_terminal_geometry(width, height);
    }

    fn handle_scale_factor_change(
        &mut self,
        scale_factor: f64,
        new_inner_size: winit::dpi::PhysicalSize<u32>,
    ) {
        if let Some(renderer) = &mut self.renderer {
            renderer.set_scale_factor(scale_factor as f32);
            renderer.resize(new_inner_size.width, new_inner_size.height);
        }
        self.sync_terminal_geometry(new_inner_size.width, new_inner_size.height);
    }

    /// Synchronize every PTY from the renderer's physical grid geometry. This
    /// runs even when rows/columns are unchanged so scale/font pixel changes
    /// reach the kernel; Tab and Pane deduplicate exact four-field repeats.
    fn sync_terminal_geometry(&mut self, width: u32, height: u32) {
        let Some(renderer) = &self.renderer else {
            return;
        };
        let geometry = renderer.calculate_grid_geometry(width, height);
        let cell_width = renderer.cell_width();
        let cell_height = renderer.cell_height();
        self.cols = geometry.cols;
        self.rows = geometry.rows;

        self.prepare_copy_mode_reflow();

        if let Some(tabs) = &mut self.tabs {
            tabs.resize_geometry(geometry);
            let content_width = geometry.cols as f32 * cell_width;
            let content_height = geometry.rows as f32 * cell_height;
            for tab in tabs.all_tabs_mut() {
                if let Some(panes) = &mut tab.pane_manager {
                    panes.resize(content_width, content_height, cell_width, cell_height);
                }
            }
        }
        self.finish_copy_mode_reflow();
        log::debug!(
            "Terminal geometry: {}x{} cells, {}x{} physical pixels",
            geometry.cols,
            geometry.rows,
            geometry.pixel_width,
            geometry.pixel_height
        );
    }

    /// Render the terminal
    fn render(&mut self) -> bool {
        let reduced_motion = self.reduced_motion_enabled();
        self.sync_native_window_title();
        // Do not run animation/update paths that enqueue more redraws while a
        // lost surface is waiting for Winit's next real resize event.
        if self.surface_lost {
            return false;
        }

        self.reconcile_broadcast_state();

        // Keep initial full-history search off the keyboard event path. Each
        // rendered frame consumes a small time slice and explicitly requests
        // another frame only while work remains.
        let search_was_pending = self.search.is_searching();
        let search_pending = self
            .with_search_and_active_grid(|search, grid| {
                search.advance_for(grid, Duration::from_millis(4))
            })
            .unwrap_or(false);
        if search_was_pending
            && !search_pending
            && !self.search.is_active()
            && self.copy_mode.is_active()
        {
            // Enter confirms copy-mode search even when the final bounded
            // slice has not run yet. Apply the same jump once it completes.
            self.jump_copy_cursor_to_search();
        }
        if search_pending {
            self.redraw_scheduler
                .request(redraw_scheduler::RedrawReason::Search);
            if let Some(window) = &self.window {
                window.request_redraw();
            }
        }

        // Smooth hover animation progress for tab bar icons unless the user
        // requested static application chrome.
        if reduced_motion {
            self.tab_bar_hover_progress = f32::from(self.tab_bar_hover.is_some());
        } else if self.tab_bar_hover.is_some() {
            self.tab_bar_hover_progress = (self.tab_bar_hover_progress + 0.2).min(1.0);
        } else {
            self.tab_bar_hover_progress = (self.tab_bar_hover_progress - 0.2).max(0.0);
        }

        // Update git watcher with active tab's working directory
        let working_dir = self
            .tabs
            .as_ref()
            .and_then(|t| t.active_tab())
            .and_then(|tab| tab.working_directory());
        // Build pane render info (before renderer borrow)
        let pane_info = self.pane_render_info();
        for pi in &pane_info {
            log::trace!(
                "Pane {:?}: ({}, {}, {}x{}) focused={}",
                pi.id,
                pi.rect.x,
                pi.rect.y,
                pi.rect.width,
                pi.rect.height,
                pi.is_focused
            );
        }

        // Verify container trait access (before renderer borrow)
        let _has_terminal = self.focused_terminal_via_container().is_some();

        // Update AI session chrome bar for plugin tabs
        let chrome_bar_data = {
            let branch = self.git_watcher.branch().map(|s| s.to_string());
            let dirty = self.git_watcher.is_dirty();
            let git_status = self.git_watcher.status();
            let wd = working_dir.clone();

            if let Some(tabs) = &mut self.tabs {
                if let Some(tab) = tabs.active_tab_mut() {
                    if let Some(ref mut session) = tab.ai_session {
                        session.update_git(branch.as_deref(), dirty, wd.as_deref());
                        Some(crate::ui::claude_chrome::ChromeBarInfo {
                            repo_name: session.repo_name.clone(),
                            branch: session.branch.clone(),
                            dirty: session.dirty,
                            ahead: git_status.map_or(0, |s| s.ahead),
                            behind: git_status.map_or(0, |s| s.behind),
                            host_label: session.host.badge_label(),
                            profile: session.profile.clone(),
                            recording: session.recording,
                        })
                    } else {
                        None
                    }
                } else {
                    None
                }
            } else {
                None
            }
        };

        // Get cursor target position and calculate animations before borrowing
        let (cursor_style, _) = self.cursor_presentation();
        let cursor_opacity = self.cursor_opacity();
        let copy_flash = self.copy_flash_intensity();
        let focus_dim = self.focus_dim();
        let text_blink = self.text_blink_phases(Instant::now());

        // Get cursor target position and active tab ID from grid
        // When tab has pane_manager, read cursor from the focused pane's terminal
        let cursor_info = self
            .tabs
            .as_ref()
            .and_then(|tabs| tabs.active_tab())
            .and_then(|tab| {
                let cursor = tab.focused_terminal()?.presentation_grid().cursor();
                Some((cursor.col, cursor.row, cursor.visible, tab.id))
            });

        // Update cursor animation if we have a target
        let cursor_display_pos = if let Some((col, row, visible, tab_id)) = cursor_info {
            self.update_cursor_animation(col, row, visible, Some(tab_id))
        } else {
            // No active tab - snap to origin and mark as not visible
            self.update_cursor_animation(0, 0, false, None)
        };
        let copy_mode_info = self.copy_mode_render_info();
        let ime_preedit_info = self.ime_preedit_render_info(&pane_info);
        let chrome_focus_target = self.chrome_focus_render_target();
        let broadcast_render = self.broadcast_render_targets();

        let Some(renderer) = &mut self.renderer else {
            return false;
        };
        renderer.set_tab_bar_keyboard_focus(chrome_focus_target);
        let Some(tabs) = &self.tabs else { return false };
        let Some(tab) = tabs.active_tab() else {
            return false;
        };
        let harvest_indicators: &[crate::harvest::GrowthIndicator] =
            if let Some(panes) = &tab.pane_manager {
                panes
                    .focused_pane()
                    .map(|pane| pane.harvest.indicators.as_slice())
                    .unwrap_or_default()
            } else {
                &tab.harvest.indicators
            };

        let Some(terminal) = tab.focused_terminal() else {
            return false;
        };
        let mut tab_info = tabs.tab_info();
        if let Some((tab_id, _, state)) = &broadcast_render
            && tab.id == *tab_id
            && let Some(active) = tab_info.iter_mut().find(|info| info.is_active)
        {
            match state {
                crate::renderer::BroadcastTargetState::Armed => active.broadcast_armed = true,
                crate::renderer::BroadcastTargetState::Active => active.broadcast_active = true,
            }
        }

        // Build settings panel info if visible
        let settings_info = if self.settings_panel.is_visible() {
            // Tell the panel how many rows it can scroll through so its
            // scroll/selection logic stays in sync with the rendered viewport.
            let (sw, sh) = renderer.window_size();
            let layout = crate::ui::settings::SettingsModalLayout::calculate(
                sw as f32,
                sh as f32,
                renderer.cell_width(),
                renderer.cell_height(),
                self.settings_panel.animation_progress(),
                0,
            );
            self.settings_panel.set_max_visible(layout.rows_that_fit);
            Some(
                self.settings_panel
                    .build_info(&self.config, &self.shortcut_registry),
            )
        } else {
            None
        };

        // Build search bar info if search is active or has results
        let search_info =
            if self.search.is_active() || self.search.has_results() || self.search.is_searching() {
                Some(self.search.build_info(self.search_cursor_pos))
            } else {
                None
            };

        // Update settings panel animation
        if self.settings_panel.update_animation() {
            self.redraw_scheduler
                .request(redraw_scheduler::RedrawReason::Animation);
            if let Some(window) = &self.window {
                window.request_redraw();
            }
        }

        // Update session panel animation
        if self.session_panel.needs_redraw() {
            self.redraw_scheduler
                .request(redraw_scheduler::RedrawReason::Animation);
            if let Some(window) = &self.window {
                window.request_redraw();
            }
        }

        // Collect image placements for rendering
        let image_data: Vec<_> = if tab.pane_manager.is_none() {
            terminal.presentation_images()
        } else {
            Vec::new()
        };
        let images = if image_data.is_empty() {
            None
        } else {
            Some(image_data.as_slice())
        };

        // Build session dropdown info if dropdown is open
        let session_dropdown_info = if self.session_dropdown_open {
            Some(crate::renderer::SessionDropdownInfo {
                sessions: self.session_dropdown_items.clone(),
                hovered_index: self.session_dropdown_hovered,
            })
        } else {
            None
        };
        // Build session panel info if panel is open or animating
        self.session_panel.update();
        let session_panel_info = if !self.session_panel.is_closed() {
            let preview = self.session_panel.build_info();
            let (window_width, window_height) = renderer.window_size();
            let layout = crate::session::panel::SessionPanelLayout::calculate(
                window_width as f32,
                window_height as f32,
                renderer.cell_width(),
                renderer.cell_height(),
                preview.animation,
                preview.rename_text.is_some(),
                preview.workspaces,
                preview.sessions.len(),
            );
            self.session_panel.set_max_visible(layout.rows_that_fit);
            Some(self.session_panel.build_info())
        } else {
            None
        };
        // Update commands panel animation
        if self.commands_panel.needs_redraw() {
            self.redraw_scheduler
                .request(redraw_scheduler::RedrawReason::Animation);
            if let Some(window) = &self.window {
                window.request_redraw();
            }
        }
        // Update visual bell animation
        self.bell.update();
        if self.bell.is_visual_active() {
            self.redraw_scheduler
                .request(redraw_scheduler::RedrawReason::Animation);
            if let Some(window) = &self.window {
                window.request_redraw();
            }
        }

        // Update harvest mode animations
        let has_active_harvest_indicator = !harvest_indicators.is_empty();
        if let Some(ref mut harvest) = self.harvest {
            let cellar_changed = harvest.cellar.update();
            let animation_active = if reduced_motion {
                cellar_changed
            } else {
                cellar_changed
                    || harvest.cellar.is_visible()
                    || harvest.sprout_menu.is_some()
                    || has_active_harvest_indicator
            };
            if animation_active {
                self.redraw_scheduler
                    .request(redraw_scheduler::RedrawReason::Animation);
                if let Some(window) = &self.window {
                    window.request_redraw();
                }
            }
        }

        // Build commands panel info if panel is open or animating
        let commands_panel_info = if !self.commands_panel.is_closed() {
            Some(self.commands_panel.build_info(&self.config.commands))
        } else {
            None
        };

        // Update command palette animation
        if self.command_palette.update_animation() {
            self.redraw_scheduler
                .request(redraw_scheduler::RedrawReason::Animation);
            if let Some(window) = &self.window {
                window.request_redraw();
            }
        }
        if self.domain_picker.update_animation() {
            self.redraw_scheduler
                .request(redraw_scheduler::RedrawReason::Animation);
            if let Some(window) = &self.window {
                window.request_redraw();
            }
        }
        // Both workflows use the same proven palette renderer and geometry,
        // but only one modal can own input and rendering at a time.
        let palette_info = if self.domain_picker.is_active() {
            let (window_width, window_height) = renderer.window_size();
            let layout = crate::ui::command_palette::PaletteLayout::calculate(
                window_width as f32,
                window_height as f32,
                renderer.cell_width(),
                renderer.cell_height(),
                1.0,
                0,
            );
            self.domain_picker.set_max_visible(layout.rows_that_fit);
            Some(self.domain_picker.build_info())
        } else if self.command_palette.is_active() {
            let (window_width, window_height) = renderer.window_size();
            let layout = crate::ui::command_palette::PaletteLayout::calculate(
                window_width as f32,
                window_height as f32,
                renderer.cell_width(),
                renderer.cell_height(),
                1.0,
                0,
            );
            self.command_palette.set_max_visible(layout.rows_that_fit);
            Some(self.command_palette.build_info())
        } else {
            None
        };

        // Update shortcuts panel animation
        if self.shortcuts_panel.update_animation() {
            self.redraw_scheduler
                .request(redraw_scheduler::RedrawReason::Animation);
            if let Some(window) = &self.window {
                window.request_redraw();
            }
        }
        // Auto-cancel shortcut recording after 15s of no input so users
        // can't get stuck in recording mode.
        if self
            .shortcuts_panel
            .tick_recording_timeout(std::time::Duration::from_secs(15))
        {
            self.redraw_scheduler
                .request(redraw_scheduler::RedrawReason::Animation);
            if let Some(window) = &self.window {
                window.request_redraw();
            }
        }
        // Build shortcuts panel info if visible
        let shortcuts_info = if self.shortcuts_panel.is_visible() {
            let (window_width, window_height) = renderer.window_size();
            let layout = crate::ui::shortcuts_panel::ShortcutsPanelLayout::calculate(
                window_width as f32,
                window_height as f32,
                renderer.cell_width(),
                renderer.cell_height(),
                1.0,
                0,
            );
            self.shortcuts_panel.set_max_visible(layout.rows_that_fit);
            Some(self.shortcuts_panel.build_info(&self.shortcut_registry))
        } else {
            None
        };

        // Build info overlay if visible
        let info_overlay_data = if self.info_overlay.is_visible() {
            let (window_width, window_height) = renderer.window_size();
            let layout = crate::ui::info_overlay::InfoOverlayLayout::calculate(
                window_width as f32,
                window_height as f32,
                renderer.cell_width(),
                renderer.cell_height(),
                self.info_overlay.animation(),
                self.info_overlay.title(),
                self.info_overlay.lines().len(),
                self.info_overlay.max_line_chars(),
            );
            self.info_overlay
                .set_viewport(layout.rows_that_fit, layout.columns_that_fit);
            let scroll_offset = self.info_overlay.scroll_offset();
            let horizontal_offset = self.info_overlay.horizontal_offset();
            let visible_lines = self
                .info_overlay
                .visible_lines()
                .map(|(source_line, line)| {
                    (
                        source_line,
                        line.chars()
                            .skip(horizontal_offset)
                            .take(layout.columns_that_fit)
                            .collect(),
                    )
                })
                .collect();
            let choices = self
                .info_overlay
                .choices()
                .iter()
                .filter(|choice| {
                    choice.source_line >= scroll_offset
                        && choice.source_line < scroll_offset.saturating_add(layout.rows_that_fit)
                })
                .cloned()
                .collect();
            Some(crate::renderer::InfoOverlayInfo {
                title: self.info_overlay.title().to_string(),
                visible_lines,
                choices,
                input: self.info_overlay.input_info(self.info_overlay.request_id()),
                total_lines: self.info_overlay.lines().len(),
                scroll_offset,
                horizontal_offset,
                awaiting: self.info_overlay.is_awaiting_current(),
                animation: self.info_overlay.animation(),
                layout,
                mouse_pos: self.last_cursor_pos.map(|(x, y)| (x as f32, y as f32)),
                mouse_pressed: self.mouse_pressed,
            })
        } else {
            None
        };
        renderer.set_chrome_bar(chrome_bar_data);
        renderer.set_info_overlay(info_overlay_data);

        // Update info overlay animation
        if self.info_overlay.update_animation() {
            self.redraw_scheduler
                .request(redraw_scheduler::RedrawReason::Animation);
            if let Some(window) = &self.window {
                window.request_redraw();
            }
        }

        // Toast lifecycle: prune expired, push snapshot into renderer.
        if self.toasts.update() {
            self.redraw_scheduler
                .request(redraw_scheduler::RedrawReason::Animation);
            if let Some(window) = &self.window {
                window.request_redraw();
            }
        }
        renderer.set_toasts(self.toasts.build_info());

        // Build diff review overlay if visible
        let diff_review_data = if self.diff_review.is_visible() {
            let (window_width, window_height) = renderer.window_size();
            let layout = crate::ui::diff_review::DiffReviewLayout::calculate(
                window_width as f32,
                window_height as f32,
                renderer.cell_width(),
                renderer.cell_height(),
                self.diff_review.animation(),
                self.diff_review.filename(),
                self.diff_review.rows().len(),
                self.diff_review.max_line_chars(),
            );
            self.diff_review
                .set_viewport(layout.rows_that_fit, layout.columns_that_fit);
            Some(crate::renderer::DiffReviewInfo {
                filename: self.diff_review.filename().to_string(),
                visible_rows: self
                    .diff_review
                    .visible_rows()
                    .map(|(source_row, row)| (source_row, row.clone()))
                    .collect(),
                total_rows: self.diff_review.rows().len(),
                scroll_offset: self.diff_review.scroll_offset(),
                horizontal_offset: self.diff_review.horizontal_offset(),
                awaiting: self.diff_review.is_awaiting_current(),
                animation: self.diff_review.animation(),
                layout,
                mouse_pos: self.last_cursor_pos.map(|(x, y)| (x as f32, y as f32)),
                mouse_pressed: self.mouse_pressed,
            })
        } else {
            None
        };
        renderer.set_diff_review(diff_review_data);

        // Update diff review animation
        if self.diff_review.update_animation() {
            self.redraw_scheduler
                .request(redraw_scheduler::RedrawReason::Animation);
            if let Some(window) = &self.window {
                window.request_redraw();
            }
        }

        // Build context picker overlay if visible
        let context_picker_data = if self.context_picker.is_visible() {
            let (window_width, window_height) = renderer.window_size();
            let layout = crate::ui::context_picker::ContextPickerLayout::calculate(
                window_width as f32,
                window_height as f32,
                renderer.cell_width(),
                renderer.cell_height(),
                self.context_picker.animation(),
                self.context_picker.phase(),
                self.context_picker.sources().len(),
            );
            self.context_picker.set_viewport(layout.visible_count);
            Some(crate::renderer::ContextPickerInfo {
                snapshot: self.context_picker.snapshot(),
                layout,
                mouse_pos: self.last_cursor_pos.map(|(x, y)| (x as f32, y as f32)),
                mouse_pressed: self.mouse_pressed,
            })
        } else {
            None
        };
        renderer.set_context_picker(context_picker_data);

        // Update context picker animation
        if self.context_picker.update_animation() {
            self.redraw_scheduler
                .request(redraw_scheduler::RedrawReason::Animation);
            if let Some(window) = &self.window {
                window.request_redraw();
            }
        }

        // Build guardrail overlay if visible
        let guardrail_data = if self.guardrail.is_visible() {
            self.guardrail
                .matched
                .as_ref()
                .map(|m| crate::renderer::GuardrailInfo {
                    command: m.command.clone(),
                    reason: m.reason.clone(),
                    is_critical: m.severity == crate::terminal::guardrails::Severity::Critical,
                    animation: self.guardrail.animation(),
                    target: m.target.clone(),
                    user: m.user.clone(),
                })
        } else {
            None
        };
        renderer.set_guardrail(guardrail_data);

        // Update guardrail animation
        if self.guardrail.update_animation() {
            self.redraw_scheduler
                .request(redraw_scheduler::RedrawReason::Animation);
            if let Some(window) = &self.window {
                window.request_redraw();
            }
        }

        renderer.set_tab_bar_hover(self.tab_bar_hover, self.tab_bar_hover_progress);
        renderer.set_git_completions(
            if self.git_completions_visible && !self.git_completions.is_empty() {
                Some(crate::renderer::GitCompletionInfo {
                    completions: self.git_completions.clone(),
                    selected_index: self.git_completion_index,
                })
            } else {
                None
            },
        );
        renderer.set_bell_intensity(self.bell.visual_intensity());

        // Scan visible rows for auto-detected URLs (reuse cached storage)
        {
            let grid = terminal.presentation_grid();
            update_visible_url_cache(&mut self.url_cache, grid, &self.url_detector);
            log::trace!("URL cache metrics: {:?}", self.url_cache.metrics());
        }

        // Build pane render data for multi-pane mode (per-tab)
        let pane_render_data: Vec<PaneRenderData> = if let Some(ref pm) = tab.pane_manager {
            let pane_grids = pm.iter_panes_identified();
            let active_terminal_ids: std::collections::HashSet<usize> = pane_grids
                .iter()
                .map(|(_, pane_terminal, _, _)| *pane_terminal as *const _ as usize)
                .collect();
            self.pane_url_caches
                .retain(|terminal_id, _| active_terminal_ids.contains(terminal_id));
            for (_, pane_terminal, _, _) in &pane_grids {
                let grid = pane_terminal.presentation_grid();
                let terminal_id = *pane_terminal as *const _ as usize;
                let cache = self
                    .pane_url_caches
                    .entry(terminal_id)
                    .or_insert_with(|| UrlCache::new(grid.rows()));
                update_visible_url_cache(cache, grid, &self.url_detector);
            }

            // Build per-pane render data with animated cursor for focused pane
            pane_grids
                .iter()
                .map(|(pane_id, term, rect, focused)| {
                    let cpos = if *focused {
                        cursor_display_pos
                    } else {
                        let c = term.presentation_grid().cursor();
                        (c.col as f32, c.row as f32)
                    };
                    let terminal_key = *term as *const _ as usize;
                    PaneRenderData {
                        terminal_key,
                        grid: term.presentation_grid(),
                        selection: term.selection(),
                        dynamic_colors: term.presentation_dynamic_colors(),
                        rect: *rect,
                        is_focused: *focused,
                        broadcast_target: broadcast_render.as_ref().and_then(
                            |(_, targets, state)| targets.contains(pane_id).then_some(*state),
                        ),
                        read_only: pm.get_pane(*pane_id).is_some_and(|pane| pane.read_only),
                        is_zoomed: pm.zoomed_id() == Some(*pane_id),
                        cursor_pos: cpos,
                        url_cache: &self.pane_url_caches[&terminal_key],
                        copy_mode: (*focused).then_some(copy_mode_info.as_ref()).flatten(),
                        images: term.presentation_images(),
                    }
                })
                .collect()
        } else {
            Vec::new()
        };
        let pane_dividers = tab
            .pane_manager
            .as_ref()
            .map(crate::panes::PaneManager::get_dividers)
            .unwrap_or_default();

        // Build Harvest geometry once, then share it with painting, pointer
        // hit-testing, and accessibility.
        let show_indicators = self.config.harvest.show_indicators;
        let (harvest_window_width, harvest_window_height) = renderer.window_size();
        let cellar_layout = self.harvest.as_mut().and_then(|harvest| {
            if !harvest.cellar.is_visible() {
                return None;
            }
            let preview = crate::harvest::CellarLayout::calculate(
                harvest_window_width as f32,
                harvest_window_height as f32,
                renderer.cell_width(),
                renderer.cell_height(),
                harvest.cellar.animation.progress(),
                std::iter::empty(),
            );
            harvest.cellar.set_viewport_rows(preview.max_visible);
            let visible_ids = harvest
                .cellar
                .visible_entries()
                .map(|entry| entry.id)
                .collect::<Vec<_>>();
            Some(crate::harvest::CellarLayout::calculate(
                harvest_window_width as f32,
                harvest_window_height as f32,
                renderer.cell_width(),
                renderer.cell_height(),
                harvest.cellar.animation.progress(),
                visible_ids,
            ))
        });
        let harvest_render_info = self.harvest.as_ref().map(|harvest| {
            // Growth indicators (respect the config toggle)
            let indicators: Vec<IndicatorRenderInfo> = if show_indicators {
                harvest_indicators
                    .iter()
                    .filter(|ind| !ind.is_finished())
                    .map(|ind| IndicatorRenderInfo {
                        row: ind.prompt_row,
                        character: ind.phase.character(),
                        color: if reduced_motion {
                            ind.phase.color()
                        } else {
                            ind.color()
                        },
                        scale: if reduced_motion { 1.0 } else { ind.scale() },
                        opacity: if ind.is_finished() { 0.0 } else { 1.0 },
                    })
                    .collect()
            } else {
                Vec::new()
            };
            // Cellar panel (visible while open or animating out)
            let cellar = cellar_layout.clone().map(|layout| CellarRenderInfo {
                entries: harvest
                    .cellar
                    .visible_entries()
                    .map(|e| CellarEntryInfo {
                        id: e.id,
                        command: e.command.clone(),
                        row: e.row,
                        exit_code: e.exit_code,
                        success: e.success,
                        output_truncated: e.output_truncated,
                        health: e.health(),
                        health_color: e.health_color(),
                        opacity: e.visual_state.opacity,
                        depth: e.visual_state.depth,
                    })
                    .collect(),
                total_entries: harvest.cellar.len(),
                evicted_entries: harvest.cellar.evicted_commands(),
                animation_progress: harvest.cellar.animation.progress(),
                success_rate: harvest.cellar.success_rate(),
                scroll: harvest.cellar.scroll,
                selected_id: harvest.cellar.selected_id(),
                layout,
            });

            // Sprout menu (if visible)
            let sprout_menu = harvest.sprout_menu.as_ref().map(|menu| {
                sprout_menu_render_info(
                    menu,
                    renderer.padding(),
                    renderer.cell_width(),
                    renderer.cell_height(),
                    renderer.tab_bar_width(),
                    renderer.tab_bar_height() + renderer.chrome_bar_height(),
                    harvest_window_width as f32,
                    harvest_window_height as f32,
                )
            });

            HarvestRenderInfo {
                indicators,
                cellar,
                sprout_menu,
            }
        });

        let presented = match renderer.render(
            terminal as *const _ as usize,
            terminal.presentation_grid(),
            terminal.selection(),
            terminal.presentation_dynamic_colors(),
            cursor_opacity,
            cursor_display_pos,
            &tab_info,
            copy_flash,
            focus_dim,
            settings_info.as_ref(),
            search_info.as_ref(),
            ime_preedit_info.as_ref(),
            &self.search,
            &self.url_cache,
            copy_mode_info.as_ref(),
            images,
            session_dropdown_info.as_ref(),
            session_panel_info.as_ref(),
            commands_panel_info.as_ref(),
            palette_info.as_ref(),
            shortcuts_info.as_ref(),
            cursor_style,
            text_blink,
            &pane_render_data,
            &pane_dividers,
            harvest_render_info.as_ref(),
        ) {
            Ok(()) => {
                self.last_text_blink_phases = text_blink;
                true
            }
            Err(error) if renderer_recovery::requires_surface_recovery(&error) => {
                // A final X11 redraw can race WM_DELETE after the drawable is
                // already gone. Reconfiguring then is a WGPU validation error.
                // An outdated live surface also requires swap-chain
                // reconfiguration. Both receive a subsequent Resized event or
                // advance to one bounded whole-renderer recovery deadline.
                if !self.surface_lost {
                    let reason = match error {
                        wgpu::SurfaceError::Lost => "lost",
                        wgpu::SurfaceError::Outdated => "outdated",
                        _ => unreachable!("surface recovery classification drifted"),
                    };
                    log::warn!(
                        "GPU surface {reason}; waiting briefly for a resize or close event before bounded recovery"
                    );
                    self.surface_lost_at = Some(Instant::now());
                }
                self.surface_lost = true;
                false
            }
            Err(wgpu::SurfaceError::OutOfMemory) => {
                log::error!("Out of GPU memory");
                self.should_exit = true;
                false
            }
            Err(e) => {
                log::warn!("Render error: {:?}", e);
                false
            }
        };
        self.refresh_accessibility();
        presented
    }

    fn desired_native_window_title(&self) -> String {
        native_window_title(
            self.launch.quick_access,
            self.tabs
                .as_ref()
                .and_then(TabManager::active_tab)
                .map(|tab| tab.title.as_str()),
        )
    }

    fn sync_native_window_title(&mut self) {
        let title = self.desired_native_window_title();
        if self.last_native_window_title.as_deref() == Some(title.as_str()) {
            return;
        }
        let Some(window) = &self.window else {
            return;
        };
        window.set_title(&title);
        self.last_native_window_title = Some(title);
    }

    /// Get the active tab's focused live terminal.
    fn active_terminal(&self) -> Option<&termeric_core::Terminal> {
        self.tabs.as_ref()?.active_tab()?.focused_terminal()
    }

    /// Get the active tab's focused live terminal mutably.
    fn active_terminal_mut(&mut self) -> Option<&mut termeric_core::Terminal> {
        self.tabs.as_mut()?.active_tab_mut()?.focused_terminal_mut()
    }

    /// Return whether user input is locked for the exact focused pane.
    fn focused_pane_read_only(&self) -> bool {
        self.tabs
            .as_ref()
            .and_then(TabManager::active_tab)
            .is_some_and(crate::tabs::Tab::focused_pane_read_only)
    }

    /// Snapshot the keyboard modes from the same focused terminal that will
    /// receive the encoded bytes.
    fn active_keyboard_mode(&self) -> termeric_core::term::TerminalMode {
        self.active_terminal()
            .map(termeric_core::Terminal::mode)
            .unwrap_or_default()
    }

    /// Snapshot Kitty keyboard negotiation from the focused receiving pane.
    fn active_kitty_keyboard_flags(&self) -> termeric_core::term::KittyKeyboardFlags {
        self.active_terminal()
            .map(termeric_core::Terminal::kitty_keyboard_flags)
            .unwrap_or_default()
    }

    /// Mutate search state using the focused live grid without extending a
    /// whole-App borrow across the caller's remaining input handling.
    fn with_search_and_active_grid<R>(
        &mut self,
        update: impl FnOnce(&mut Search, &termeric_core::Grid) -> R,
    ) -> Option<R> {
        let search = &mut self.search;
        let grid = self.tabs.as_ref()?.active_tab()?.focused_terminal()?.grid();
        Some(update(search, grid))
    }

    /// Calculate cursor opacity for smooth blink animation
    /// Returns a value between 0.0 (invisible) and 1.0 (fully visible)
    fn cursor_opacity(&self) -> f32 {
        let elapsed = self.cursor_blink_start.elapsed().as_secs_f32();
        cursor_opacity_at(
            self.cursor_presentation().1,
            elapsed,
            self.cursor_blink_period,
        )
    }

    fn cursor_presentation(&self) -> (crate::config::CursorStyle, bool) {
        resolve_cursor_presentation(
            self.config.cursor.style,
            self.config.cursor.blink,
            self.active_terminal()
                .and_then(termeric_core::Terminal::presentation_cursor_style),
            self.active_terminal()
                .and_then(termeric_core::Terminal::presentation_cursor_blink_override),
        )
    }

    fn cursor_is_visible(&self) -> bool {
        self.active_terminal()
            .map(|terminal| terminal.presentation_grid().cursor().visible)
            .unwrap_or(false)
    }

    fn cursor_blink_phase(&self, now: Instant) -> bool {
        if !self.cursor_presentation().1 {
            return true;
        }
        let half_period = (self.cursor_blink_period / 2.0).max(0.001);
        let elapsed = now
            .saturating_duration_since(self.cursor_blink_start)
            .as_secs_f32();
        (elapsed / half_period) as u64 % 2 == 0
    }

    fn next_cursor_blink_deadline(&self, now: Instant) -> Option<Instant> {
        if !self.cursor_presentation().1
            || !self.window_focused
            || self.presentation_suspended()
            || !self.cursor_is_visible()
        {
            return None;
        }
        let half_period =
            std::time::Duration::from_secs_f32((self.cursor_blink_period / 2.0).max(0.001));
        let elapsed = now.saturating_duration_since(self.cursor_blink_start);
        let completed = elapsed.as_nanos() / half_period.as_nanos();
        Some(self.cursor_blink_start + half_period.mul_f64((completed + 1) as f64))
    }

    fn text_blink_phases(&self, now: Instant) -> TextBlinkPhases {
        let elapsed = now.saturating_duration_since(self.text_blink_start);
        TextBlinkPhases {
            slow_visible: blink_phase_visible(elapsed, std::time::Duration::from_millis(500)),
            rapid_visible: blink_phase_visible(elapsed, std::time::Duration::from_millis(250)),
        }
    }

    fn visible_text_blink(&self) -> VisibleTextBlink {
        self.renderer
            .as_ref()
            .map(Renderer::visible_text_blink)
            .unwrap_or_default()
    }

    fn next_text_blink_deadline(&self, now: Instant) -> Option<Instant> {
        if !self.window_focused || self.presentation_suspended() {
            return None;
        }
        let visible = self.visible_text_blink();
        let mut deadlines = Vec::with_capacity(2);
        if visible.slow {
            deadlines.push(next_blink_boundary(
                self.text_blink_start,
                now,
                std::time::Duration::from_millis(500),
            ));
        }
        if visible.rapid {
            deadlines.push(next_blink_boundary(
                self.text_blink_start,
                now,
                std::time::Duration::from_millis(250),
            ));
        }
        deadlines.into_iter().min()
    }

    fn cursor_position_animation_active(&self) -> bool {
        let cursor = self
            .active_terminal()
            .map(|terminal| terminal.grid().cursor());
        cursor.is_some_and(|cursor| {
            (self.cursor_display_col - cursor.col as f32).abs() >= 0.01
                || (self.cursor_display_row - cursor.row as f32).abs() >= 0.01
        })
    }

    fn has_deadline_animation(&self) -> bool {
        !self.reduced_motion_enabled()
            && (self
                .tabs
                .as_ref()
                .is_some_and(TabManager::has_active_animation)
                || self.cursor_position_animation_active()
                || self
                    .copy_flash_start
                    .is_some_and(|start| start.elapsed().as_secs_f32() < 0.15)
                || self.focus_change_time.elapsed().as_secs_f32() < 0.1
                || (self.tab_bar_hover.is_some() && self.tab_bar_hover_progress < 1.0)
                || (self.tab_bar_hover.is_none() && self.tab_bar_hover_progress > 0.0))
    }

    fn presentation_suspended(&self) -> bool {
        self.window_occluded || self.surface_lost
    }

    fn expire_synchronized_outputs(&mut self, now: Instant) -> usize {
        let Some(tabs) = &mut self.tabs else {
            return 0;
        };
        let mut expired = 0;
        for tab in tabs.all_tabs_mut() {
            if let Some(panes) = &mut tab.pane_manager {
                for pane in panes.all_panes_mut() {
                    expired += usize::from(pane.terminal.expire_synchronized_output(now));
                }
            } else if let Some(terminal) = tab.focused_terminal_mut() {
                expired += usize::from(terminal.expire_synchronized_output(now));
            }
        }
        expired
    }

    fn next_synchronized_output_deadline(&self) -> Option<Instant> {
        let tabs = self.tabs.as_ref()?;
        tabs.iter()
            .flat_map(|(_, tab)| {
                if let Some(panes) = &tab.pane_manager {
                    panes
                        .iter_panes_identified()
                        .into_iter()
                        .filter_map(|(_, terminal, _, _)| terminal.synchronized_output_deadline())
                        .collect::<Vec<_>>()
                } else {
                    tab.focused_terminal()
                        .and_then(termeric_core::Terminal::synchronized_output_deadline)
                        .into_iter()
                        .collect()
                }
            })
            .min()
    }

    fn advance_graphics_animations(&mut self, now: Instant) -> usize {
        let Some(tab) = self.tabs.as_mut().and_then(TabManager::active_tab_mut) else {
            return 0;
        };
        if let Some(panes) = &mut tab.pane_manager {
            panes
                .all_panes_mut()
                .map(|pane| usize::from(pane.terminal.advance_graphics_animations(now)))
                .sum()
        } else {
            usize::from(
                tab.focused_terminal_mut()
                    .is_some_and(|terminal| terminal.advance_graphics_animations(now)),
            )
        }
    }

    fn next_graphics_animation_deadline(&self) -> Option<Instant> {
        let tab = self.tabs.as_ref()?.active_tab()?;
        if let Some(panes) = &tab.pane_manager {
            panes
                .iter_panes_identified()
                .into_iter()
                .filter_map(|(_, terminal, _, _)| terminal.graphics_animation_deadline())
                .min()
        } else {
            tab.focused_terminal()
                .and_then(termeric_core::Terminal::graphics_animation_deadline)
        }
    }

    fn poll_idle_sources(&mut self, now: Instant) {
        if self.surface_lost
            && self.surface_lost_at.is_some_and(|lost_at| {
                now.saturating_duration_since(lost_at) >= renderer_recovery::SURFACE_RECOVERY_DELAY
            })
        {
            self.recover_renderer(
                "GPU surface remained lost after the compositor grace period".into(),
            );
        }
        if matches!(
            &self.broadcast,
            BroadcastState::Armed { expires, .. } if now >= *expires
        ) {
            self.broadcast = BroadcastState::Inactive;
            self.toasts.info("Broadcast arming expired");
            self.redraw_scheduler
                .request(redraw_scheduler::RedrawReason::WindowEvent);
        }

        let expired_synchronized_outputs = self.expire_synchronized_outputs(now);
        if expired_synchronized_outputs > 0 {
            log::warn!(
                "Released {expired_synchronized_outputs} synchronized-output batch(es) after the safety timeout"
            );
            self.redraw_scheduler
                .request(redraw_scheduler::RedrawReason::TerminalOutput);
        }

        if self.poll_config_for_hot_reload() {
            self.redraw_scheduler
                .request(redraw_scheduler::RedrawReason::ConfigReload);
        }

        let working_dir = self
            .tabs
            .as_ref()
            .and_then(TabManager::active_tab)
            .and_then(|tab| tab.working_directory());
        if self.git_watcher.update(working_dir.as_deref()) {
            self.redraw_scheduler
                .request(redraw_scheduler::RedrawReason::GitStatus);
        }
        if let Some(error) = self.git_watcher.take_error() {
            self.toasts
                .warning(format!("Git inspection failed: {error}"));
            self.redraw_scheduler
                .request(redraw_scheduler::RedrawReason::GitStatus);
        }

        if !self.presentation_suspended() {
            let blink_visible = self.cursor_blink_phase(now);
            if blink_visible != self.last_cursor_blink_visible
                && self.window_focused
                && self.cursor_is_visible()
            {
                self.last_cursor_blink_visible = blink_visible;
                self.redraw_scheduler
                    .request(redraw_scheduler::RedrawReason::CursorBlink);
            }

            let text_blink = self.text_blink_phases(now);
            let visible_text_blink = self.visible_text_blink();
            let text_blink_changed = (visible_text_blink.slow
                && text_blink.slow_visible != self.last_text_blink_phases.slow_visible)
                || (visible_text_blink.rapid
                    && text_blink.rapid_visible != self.last_text_blink_phases.rapid_visible);
            if text_blink_changed && self.window_focused {
                self.last_text_blink_phases = text_blink;
                self.redraw_scheduler
                    .request(redraw_scheduler::RedrawReason::TextBlink);
            }

            if self.advance_graphics_animations(now) > 0 {
                self.redraw_scheduler
                    .request(redraw_scheduler::RedrawReason::Animation);
            }

            if self.has_deadline_animation()
                && now >= self.last_frame_time + std::time::Duration::from_millis(16)
            {
                self.redraw_scheduler
                    .request(redraw_scheduler::RedrawReason::Animation);
            }
        }
        if self.toasts.update() && !self.presentation_suspended() {
            self.redraw_scheduler
                .request(redraw_scheduler::RedrawReason::Animation);
        }
        if self
            .shortcuts_panel
            .tick_recording_timeout(std::time::Duration::from_secs(15))
        {
            self.redraw_scheduler
                .request(redraw_scheduler::RedrawReason::WindowEvent);
        }
    }

    fn next_idle_deadline(&self, now: Instant) -> Option<Instant> {
        let mut deadlines = vec![
            self.last_config_check + Self::CONFIG_POLL_INTERVAL,
            self.git_watcher.next_poll_deadline(),
        ];
        if !self.presentation_suspended() {
            if let Some(cursor) = self.next_cursor_blink_deadline(now) {
                deadlines.push(cursor);
            }
            if let Some(text) = self.next_text_blink_deadline(now) {
                deadlines.push(text);
            }
            if self.has_deadline_animation() {
                deadlines.push(self.last_frame_time + std::time::Duration::from_millis(16));
            }
            if let Some(toast) = self.toasts.next_redraw_deadline(now) {
                deadlines.push(toast);
            }
            if let Some(graphics_animation) = self.next_graphics_animation_deadline() {
                deadlines.push(graphics_animation);
            }
        }
        if let Some(recording) = self
            .shortcuts_panel
            .recording_deadline(std::time::Duration::from_secs(15))
        {
            deadlines.push(recording);
        }
        if let Some(synchronized_output) = self.next_synchronized_output_deadline() {
            deadlines.push(synchronized_output);
        }
        if let Some(surface_lost_at) = self.surface_lost_at {
            deadlines.push(surface_lost_at + renderer_recovery::SURFACE_RECOVERY_DELAY);
        }
        if let BroadcastState::Armed { expires, .. } = &self.broadcast {
            deadlines.push(*expires);
        }
        RedrawScheduler::next_deadline(now, deadlines)
    }

    /// Reset cursor blink animation (e.g., after typing)
    fn reset_cursor_blink(&mut self) {
        self.cursor_blink_start = Instant::now();
        self.last_cursor_blink_visible = true;
    }

    /// Update animated cursor position, interpolating toward target
    /// Returns the current display position (col, row) as floats
    ///
    /// Snaps immediately (no animation) when:
    /// - Cursor visibility changes (reappears after being hidden)
    /// - A new tab is created (to avoid animating from old tab's position)
    ///
    /// Animates smoothly for:
    /// - Switching between existing tabs
    /// - All cursor movement including large jumps (e.g., after `cd`, `clear`)
    fn update_cursor_animation(
        &mut self,
        target_col: usize,
        target_row: usize,
        cursor_visible: bool,
        active_tab_id: Option<usize>,
    ) -> (f32, f32) {
        let now = Instant::now();
        let delta = now.duration_since(self.last_frame_time).as_secs_f32();
        self.last_frame_time = now;

        let target_col_f = target_col as f32;
        let target_row_f = target_row as f32;

        // Check for snap conditions:
        // - Visibility change: cursor was hidden and reappears (e.g., command finished)
        // - New tab: tab ID is higher than any we've EVER seen (truly new tab)
        // We explicitly DON'T snap on tab switches - animate between existing tabs!
        // We explicitly DON'T snap on large jumps - that's the animation we want!
        let visibility_changed = cursor_visible != self.last_cursor_visible;

        // Detect truly new tab: compare against the highest ID we've ever seen,
        // not just the last active tab (which could be lower when switching tabs)
        let is_new_tab = match (active_tab_id, self.max_seen_tab_id) {
            (Some(current), Some(max)) => current > max,
            (Some(_), None) => true, // First tab ever
            _ => false,
        };

        // Update tracking state
        self.last_cursor_visible = cursor_visible;
        self.last_active_tab_id = active_tab_id;
        // Update max seen tab ID
        if let Some(current) = active_tab_id {
            self.max_seen_tab_id =
                Some(self.max_seen_tab_id.map_or(current, |max| max.max(current)));
        }

        // Snap immediately if cursor reappears or it's a truly new tab
        if self.reduced_motion_enabled() || visibility_changed || is_new_tab {
            self.cursor_display_col = target_col_f;
            self.cursor_display_row = target_row_f;
            return (self.cursor_display_col, self.cursor_display_row);
        }

        // Easing speed - higher = faster snap to target
        // 15.0 gives a quick but visible ease (reaches target in ~100-150ms)
        let ease_speed = 15.0;
        let t = (ease_speed * delta).min(1.0);

        // Lerp toward target
        self.cursor_display_col += (target_col_f - self.cursor_display_col) * t;
        self.cursor_display_row += (target_row_f - self.cursor_display_row) * t;

        // Snap if very close to avoid endless tiny movements
        if (self.cursor_display_col - target_col_f).abs() < 0.01 {
            self.cursor_display_col = target_col_f;
        }
        if (self.cursor_display_row - target_row_f).abs() < 0.01 {
            self.cursor_display_row = target_row_f;
        }

        (self.cursor_display_col, self.cursor_display_row)
    }

    /// Handle a plugin/custom keybinding action (SendText, RunCommand, PluginCommand).
    /// Returns true if the action was handled.
    fn handle_key_action(&mut self, key_action: KeyAction) -> bool {
        // Try to convert to internal Action first (PluginCommand)
        if let Some(action) = key_action.to_action() {
            self.handle_action(action);
            return true;
        }

        match key_action {
            KeyAction::SendText { text } => {
                self.write_to_pty(text.as_bytes());
                true
            }
            KeyAction::RunCommand { command } => {
                self.run_custom_command(&command);
                true
            }
            _ => false,
        }
    }

    /// Handle an action triggered by a keybinding.
    /// Returns true if the action was handled and the key should be consumed.
    fn handle_action(&mut self, action: Action) -> bool {
        match action {
            Action::Copy => {
                // If nothing was copied, don't consume the keystroke — let it
                // pass through (e.g. Ctrl+Shift+C falls through to the PTY).
                if !self.copy_selection() {
                    return false;
                }
            }
            Action::Paste => {
                self.paste_clipboard();
            }
            Action::ScrollUp(lines) => {
                if let Some(terminal) = self.active_terminal_mut() {
                    let offset = terminal.grid().scroll_offset();
                    terminal.grid_mut().set_scroll_offset(offset + lines);
                }
            }
            Action::ScrollDown(lines) => {
                if let Some(terminal) = self.active_terminal_mut() {
                    let offset = terminal.grid().scroll_offset();
                    terminal
                        .grid_mut()
                        .set_scroll_offset(offset.saturating_sub(lines));
                }
            }
            Action::ScrollToTop => {
                if let Some(terminal) = self.active_terminal_mut() {
                    let max_offset = terminal.grid().scrollback_len();
                    terminal.grid_mut().set_scroll_offset(max_offset);
                }
            }
            Action::ScrollToBottom => {
                if let Some(terminal) = self.active_terminal_mut() {
                    terminal.grid_mut().set_scroll_offset(0);
                }
            }
            Action::ClearSelection => {
                // Only consume the key if there's actually a selection to clear
                if self.mouse_state.current_selection().is_none() {
                    return false;
                }
                self.mouse_state.clear_selection();
                if let Some(terminal) = self.active_terminal_mut() {
                    terminal.set_selection(None);
                }
            }
            Action::IncreaseFontSize => {
                self.change_font_size(2.0);
            }
            Action::DecreaseFontSize => {
                self.change_font_size(-2.0);
            }
            Action::ResetFontSize => {
                self.set_font_size(self.default_font_size);
            }
            Action::ToggleFullscreen => {
                self.toggle_fullscreen();
            }
            Action::NewTab => {
                self.request_new_tab();
            }
            Action::CloseTab => {
                if let Some(active) = self.tabs.as_ref().map(TabManager::active_index) {
                    self.request_close_tab(active);
                }
            }
            Action::NextTab => {
                let from = self.tabs.as_ref().map(|t| t.active_index()).unwrap_or(0);
                if let Some(tabs) = &mut self.tabs {
                    tabs.next_tab();
                }
                let to = self.tabs.as_ref().map(|t| t.active_index()).unwrap_or(0);
                if from != to {
                    self.refresh_focused_terminal_surfaces();
                    self.dispatch_plugin_hook(crate::plugin::Hook::TabSwitched { from, to });
                }
            }
            Action::PrevTab => {
                let from = self.tabs.as_ref().map(|t| t.active_index()).unwrap_or(0);
                if let Some(tabs) = &mut self.tabs {
                    tabs.prev_tab();
                }
                let to = self.tabs.as_ref().map(|t| t.active_index()).unwrap_or(0);
                if from != to {
                    self.refresh_focused_terminal_surfaces();
                    self.dispatch_plugin_hook(crate::plugin::Hook::TabSwitched { from, to });
                }
            }
            Action::SwitchToTab(index) => {
                let from = self.tabs.as_ref().map(|t| t.active_index()).unwrap_or(0);
                if let Some(tabs) = &mut self.tabs {
                    tabs.switch_to(index);
                }
                let to = self.tabs.as_ref().map(|t| t.active_index()).unwrap_or(0);
                if from != to {
                    self.refresh_focused_terminal_surfaces();
                    self.dispatch_plugin_hook(crate::plugin::Hook::TabSwitched { from, to });
                }
            }
            Action::ToggleSearch => {
                if self.search.is_active() {
                    self.search.cancel();
                } else {
                    self.search.start();
                    self.search_cursor_pos = 0;
                }
                if let Some(window) = &self.window {
                    window.request_redraw();
                }
            }
            Action::ToggleCopyMode => {
                if self.copy_mode.is_active() {
                    self.leave_copy_mode(true);
                } else {
                    self.enter_copy_mode();
                }
            }
            Action::QuickSelect => {
                self.begin_quick_select();
            }
            Action::ToggleSettings => {
                self.toggle_settings_surface();
                if let Some(window) = &self.window {
                    window.request_redraw();
                }
            }
            Action::PreviousPrompt => {
                if let Some(terminal) = self.active_terminal_mut() {
                    if let Some(row) = terminal.previous_prompt() {
                        log::debug!("Navigate to previous prompt at row {}", row);
                    }
                }
            }
            Action::NextPrompt => {
                if let Some(terminal) = self.active_terminal_mut() {
                    if let Some(row) = terminal.next_prompt() {
                        log::debug!("Navigate to next prompt at row {}", row);
                    }
                }
            }
            Action::PluginCommand(command_id) => {
                self.execute_plugin_command(&command_id);
            }
            Action::ToggleCommandPalette => {
                if self.command_palette.is_active() {
                    self.command_palette.close();
                } else {
                    self.command_palette.open();
                }
                if let Some(window) = &self.window {
                    window.request_redraw();
                }
            }
            Action::SplitVertical => {
                self.split_pane(SplitDirection::Vertical);
            }
            Action::SplitHorizontal => {
                self.split_pane(SplitDirection::Horizontal);
            }
            Action::NavigatePane(dir) => {
                if let Some(d) = crate::panes::PaneDir::parse(&dir) {
                    self.navigate_focused_pane(d);
                } else {
                    log::warn!("Unknown pane navigation direction: {}", dir);
                }
            }
            Action::ClosePane => {
                self.close_focused_pane();
            }
            Action::RenameTab => self.begin_tab_rename(),
            Action::MoveTabLeft => self.request_move_active_tab_by(-1),
            Action::MoveTabRight => self.request_move_active_tab_by(1),
            Action::TogglePinTab => self.request_toggle_active_tab_pin(),
            Action::TogglePaneZoom => {
                self.workspace_pane_action(PaneManager::toggle_zoom, "Pane zoom toggled");
            }
            Action::TogglePaneReadOnly => self.toggle_focused_pane_read_only(),
            Action::SwapPaneNext => self.request_swap_focused_pane_next(),
            Action::BalancePanes => self.request_balance_active_tab(),
            Action::MovePaneToNextTab => self.move_pane_to_next_tab(),
            Action::MovePaneToNextWindow => self.request_move_pane_to_next_window(),
            Action::MoveTabToNextWindow => self.request_move_tab_to_next_window(),
            Action::LayoutColumns => {
                self.request_active_tab_layout(crate::panes::PaneLayoutPreset::Columns)
            }
            Action::LayoutRows => {
                self.request_active_tab_layout(crate::panes::PaneLayoutPreset::Rows)
            }
            Action::LayoutGrid => {
                self.request_active_tab_layout(crate::panes::PaneLayoutPreset::Grid)
            }
            Action::CyclePaneCwdMode => self.cycle_pane_cwd_mode(),
            Action::ToggleBroadcast => self.toggle_broadcast(),
            Action::ClearScrollback => {
                if let Some(terminal) = self.active_terminal_mut() {
                    terminal.clear_scrollback();
                }
            }
            Action::ResetTerminal => {
                if let Some(terminal) = self.active_terminal_mut() {
                    terminal.reset();
                }
            }
        }
        true
    }

    /// Toggle Settings as an exclusive chrome surface. Side panels can cover
    /// the settings button and previously consumed the first click merely to
    /// close themselves, making Settings appear intermittently unresponsive.
    fn toggle_settings_surface(&mut self) {
        if self.settings_panel.is_active() {
            self.settings_panel.close();
            return;
        }

        self.commands_panel.close();
        self.session_panel.close();
        self.session_dropdown_open = false;
        self.settings_panel.toggle();
    }

    /// Toggle borderless fullscreen for only this native window.
    ///
    /// Winit's live state wins when a window exists so an external window
    /// manager transition does not leave the next toggle inverted. Headless
    /// controllers retain the requested state for deterministic action tests.
    fn toggle_fullscreen(&mut self) {
        let next = self
            .window
            .as_ref()
            .map_or(!self.fullscreen_requested, |window| {
                window.fullscreen().is_none()
            });
        self.fullscreen_requested = next;
        if let Some(window) = &self.window {
            log::info!(
                "Fullscreen request: window={:?}, enabled={next}",
                window.id()
            );
            window.set_fullscreen(next.then_some(winit::window::Fullscreen::Borderless(None)));
            window.request_redraw();
        }
    }

    /// Handle the result of executing a command from the palette
    fn handle_palette_result(&mut self, result: PaletteResult) {
        match result {
            PaletteResult::RunAction(id) => {
                if let Some(name) = id.strip_prefix("profile.open.") {
                    self.open_terminal_profile(name);
                    return;
                }
                let action = match id.as_str() {
                    "core.new-window" => {
                        self.request_new_window = true;
                        return;
                    }
                    "core.toggle-fullscreen" => Action::ToggleFullscreen,
                    "core.attach-domain" => {
                        self.open_domain_picker();
                        return;
                    }
                    "core.cancel-file-upload" => {
                        self.cancel_file_upload();
                        return;
                    }
                    "core.new-tab" => Action::NewTab,
                    "core.close-tab" => Action::CloseTab,
                    "core.next-tab" => Action::NextTab,
                    "core.prev-tab" => Action::PrevTab,
                    "core.find" => Action::ToggleSearch,
                    "core.copy-mode" => Action::ToggleCopyMode,
                    "core.quick-select" => Action::QuickSelect,
                    "core.settings" => Action::ToggleSettings,
                    "core.shortcuts" => {
                        self.shortcuts_panel.toggle();
                        if let Some(window) = &self.window {
                            window.request_redraw();
                        }
                        return;
                    }
                    "core.save-config" => {
                        if let Ok(msg) = self.save_config() {
                            log::info!("{}", msg);
                        }
                        return;
                    }
                    "core.reload-config" => {
                        match crate::config::Config::load_or_default() {
                            Ok(config) => {
                                if let Err(error) = self.apply_config(config) {
                                    self.toasts.error(format!("Reload failed: {error}"));
                                } else {
                                    log::info!("Configuration reloaded");
                                    self.toasts.success("Configuration reloaded");
                                }
                            }
                            Err(e) => {
                                log::error!("Failed to reload config: {}", e);
                                self.toasts.error(format!("Reload failed: {}", e));
                            }
                        }
                        return;
                    }
                    "core.reset-config" => {
                        if let Err(error) = self.apply_config(Config::default()) {
                            self.toasts.error(format!("Reset failed: {error}"));
                        } else {
                            match self.save_config() {
                                Ok(path) => {
                                    self.toasts.success(format!("Configuration reset: {path}"))
                                }
                                Err(error) => self
                                    .toasts
                                    .error(format!("Reset could not be saved: {error}")),
                            };
                        }
                        return;
                    }
                    "core.workspace-manager" | "core.session-manager" => {
                        if self.session_panel.is_open() {
                            self.session_panel.close();
                        } else {
                            self.settings_panel.close();
                            self.commands_panel.close();
                            if id == "core.workspace-manager" {
                                self.session_panel.open_workspaces();
                            } else {
                                self.session_panel.open_sessions();
                            }
                        }
                        if let Some(window) = &self.window {
                            window.request_redraw();
                        }
                        return;
                    }
                    "core.save-workspace" => {
                        self.force_save_workspace();
                        return;
                    }
                    "core.save-session" => {
                        self.force_save_session();
                        return;
                    }
                    "core.plugins" => {
                        use crate::ui::settings::SettingsSection;
                        self.settings_panel
                            .open_to_section(SettingsSection::Plugins);
                        if let Some(window) = &self.window {
                            window.request_redraw();
                        }
                        return;
                    }
                    "core.font-size-increase" => Action::IncreaseFontSize,
                    "core.font-size-decrease" => Action::DecreaseFontSize,
                    "core.font-size-reset" => Action::ResetFontSize,
                    "core.copy" => Action::Copy,
                    "core.paste" => Action::Paste,
                    "core.version" => {
                        self.info_overlay.show_version();
                        return;
                    }
                    "core.about" => {
                        self.info_overlay.show_about();
                        return;
                    }
                    "core.export-session" => {
                        // Export the active plugin tab's transcript
                        let plugin_id = self
                            .tabs
                            .as_ref()
                            .and_then(|tabs| tabs.active_tab())
                            .and_then(|tab| {
                                if let crate::tabs::TabType::Plugin { plugin_id } = &tab.tab_type {
                                    Some(plugin_id.clone())
                                } else {
                                    None
                                }
                            });
                        if let Some(id) = plugin_id {
                            self.export_plugin_session(&id);
                        } else {
                            log::info!("No plugin tab active for export");
                            self.toasts
                                .warning("Activate an AI plugin tab before exporting");
                        }
                        return;
                    }
                    "core.harvest-cellar" => {
                        if let Some(ref mut harvest) = self.harvest {
                            harvest.cellar.toggle_expanded();
                        }
                        return;
                    }
                    "core.show-sprout-menu" => {
                        // Anchor the menu to the latest recorded command
                        // so the actions target something the user can
                        // reason about. If no command has completed yet
                        // (e.g. fresh shell), fall back to cursor row.
                        let source = self.active_harvest_source();
                        let (anchor_row, anchor_col, entry) =
                            if let (Some(h), Some(source)) = (self.harvest.as_ref(), source) {
                                if let Some(latest) = h.cellar.latest_for_source(source).cloned() {
                                    let col = 0;
                                    (latest.row.max(0) as usize, col, Some(latest))
                                } else {
                                    let cur = self
                                        .active_terminal_mut()
                                        .map(|t| t.grid().cursor())
                                        .map(|c| (c.row, c.col))
                                        .unwrap_or((0, 0));
                                    (cur.0, cur.1, None)
                                }
                            } else {
                                (0, 0, None)
                            };
                        if entry.is_none() {
                            let message = self.active_shell_integration_message();
                            self.toasts.warning(message);
                            return;
                        }
                        let availability = entry
                            .as_ref()
                            .map(|entry| self.sprout_availability_for_entry(entry))
                            .unwrap_or_default();
                        if let Some(ref mut harvest) = self.harvest {
                            harvest.show_sprout_menu(anchor_row, anchor_col, entry, availability);
                        }
                        return;
                    }
                    "core.hide-sprout-menu" => {
                        if let Some(ref mut harvest) = self.harvest {
                            harvest.hide_sprout_menu();
                        }
                        return;
                    }
                    "core.clear-harvest" => {
                        if let Some(ref mut harvest) = self.harvest {
                            harvest.cellar.clear();
                        }
                        return;
                    }
                    "core.toggle-transcript" => {
                        self.toggle_transcript();
                        return;
                    }
                    "core.split-vertical" => {
                        self.split_pane(SplitDirection::Vertical);
                        return;
                    }
                    "core.split-horizontal" => {
                        self.split_pane(SplitDirection::Horizontal);
                        return;
                    }
                    "core.close-pane" => {
                        self.close_focused_pane();
                        return;
                    }
                    "core.rename-tab" => Action::RenameTab,
                    "core.move-tab-left" => Action::MoveTabLeft,
                    "core.move-tab-right" => Action::MoveTabRight,
                    "core.toggle-pin-tab" => Action::TogglePinTab,
                    "core.toggle-pane-zoom" => Action::TogglePaneZoom,
                    "core.toggle-pane-read-only" => Action::TogglePaneReadOnly,
                    "core.swap-pane-next" => Action::SwapPaneNext,
                    "core.balance-panes" => Action::BalancePanes,
                    "core.move-pane-next-tab" => Action::MovePaneToNextTab,
                    "core.move-pane-next-window" => Action::MovePaneToNextWindow,
                    "core.move-tab-next-window" => Action::MoveTabToNextWindow,
                    "core.layout-columns" => Action::LayoutColumns,
                    "core.layout-rows" => Action::LayoutRows,
                    "core.layout-grid" => Action::LayoutGrid,
                    "core.cycle-pane-cwd" => Action::CyclePaneCwdMode,
                    "core.toggle-broadcast" => Action::ToggleBroadcast,
                    "core.navigate-pane-up" => {
                        self.navigate_focused_pane(crate::panes::PaneDir::Up);
                        return;
                    }
                    "core.navigate-pane-down" => {
                        self.navigate_focused_pane(crate::panes::PaneDir::Down);
                        return;
                    }
                    "core.navigate-pane-left" => {
                        self.navigate_focused_pane(crate::panes::PaneDir::Left);
                        return;
                    }
                    "core.navigate-pane-right" => {
                        self.navigate_focused_pane(crate::panes::PaneDir::Right);
                        return;
                    }
                    id if id.starts_with("core.theme.") => {
                        let theme_name = id.strip_prefix("core.theme.").unwrap_or("");
                        match Theme::resolve(theme_name, None) {
                            Ok((theme, source)) => {
                                self.config.theme.name = theme_name.to_string();
                                self.config.theme.path = None;
                                self.apply_theme(theme, source);
                                match self.save_config() {
                                    Ok(msg) => {
                                        log::info!("Theme changed to {}: {}", theme_name, msg);
                                        self.toasts.info(format!("Theme: {}", theme_name));
                                    }
                                    Err(e) => {
                                        log::error!("Theme applied but save failed: {}", e);
                                        self.toasts.warning(format!(
                                            "Theme '{}' applied but not saved: {}",
                                            theme_name, e
                                        ));
                                    }
                                }
                            }
                            Err(err) => {
                                self.toasts
                                    .error(format!("Theme '{}' failed: {}", theme_name, err));
                            }
                        }
                        return;
                    }
                    _ => {
                        log::warn!("Unknown palette action: {}", id);
                        return;
                    }
                };
                self.handle_action(action);
            }
            PaletteResult::RunPluginCommand(id) => {
                self.execute_plugin_command(&id);
            }
            PaletteResult::Cancelled => {}
        }
        if let Some(window) = &self.window {
            window.request_redraw();
        }
    }

    /// Copy selection to clipboard. Returns true if anything was copied so
    /// the caller can decide whether to consume the keybinding.
    fn copy_selection(&mut self) -> bool {
        let Some(text) = self
            .active_terminal()
            .and_then(termeric_core::Terminal::get_selection_text)
        else {
            return false;
        };
        if text.is_empty() {
            return false;
        }

        if let Some(clipboard) = &mut self.clipboard {
            if let Err(e) = clipboard.set_text(&text) {
                log::error!("Failed to copy to clipboard: {}", e);
                return false;
            }
            log::debug!("Copied {} bytes to clipboard", text.len());
            self.copy_flash_start = Some(Instant::now());
            return true;
        }
        false
    }

    /// Calculate copy flash intensity (0.0 = no flash, 1.0 = full flash)
    /// Flash fades out over ~150ms
    fn copy_flash_intensity(&self) -> f32 {
        const FLASH_DURATION: f32 = 0.15; // 150ms

        if self.reduced_motion_enabled() {
            return 0.0;
        }
        match self.copy_flash_start {
            Some(start) => {
                let elapsed = start.elapsed().as_secs_f32();
                if elapsed >= FLASH_DURATION {
                    0.0
                } else {
                    // Fade out: 1.0 -> 0.0
                    1.0 - (elapsed / FLASH_DURATION)
                }
            }
            None => 0.0,
        }
    }

    /// Calculate focus dim amount (0.0 = fully bright/focused, 1.0 = dimmed/unfocused)
    /// Smoothly transitions over ~100ms when focus changes
    fn focus_dim(&self) -> f32 {
        const TRANSITION_DURATION: f32 = 0.1; // 100ms transition
        const DIM_AMOUNT: f32 = 0.3; // How much to dim when unfocused (30%)

        if self.reduced_motion_enabled() {
            return if self.window_focused { 0.0 } else { DIM_AMOUNT };
        }
        let elapsed = self.focus_change_time.elapsed().as_secs_f32();
        let t = (elapsed / TRANSITION_DURATION).min(1.0);

        // Ease-out for smooth transition
        let eased_t = 1.0 - (1.0 - t) * (1.0 - t);

        if self.window_focused {
            // Transitioning to focused: dim -> bright
            DIM_AMOUNT * (1.0 - eased_t)
        } else {
            // Transitioning to unfocused: bright -> dim
            DIM_AMOUNT * eased_t
        }
    }

    /// Apply one native window focus transition. Repeated notifications are
    /// ignored so DECSET 1004 applications receive exactly one report.
    fn handle_window_focus_change(&mut self, focused: bool) {
        if !focused {
            self.alt_graph_active = false;
            self.terminal_mouse_button = None;
        }
        if self.window_focused == focused {
            return;
        }
        if let Some(report) =
            crate::input::encode_focus_report(self.active_keyboard_mode(), focused)
        {
            self.write_terminal_protocol_input(report);
        }
        self.window_focused = focused;
        self.focus_change_time = Instant::now();
        if focused {
            self.reset_cursor_blink();
        }
        log::debug!("Window focus changed: {}", focused);
    }

    /// Paste from clipboard
    fn paste_clipboard(&mut self) {
        let text = match self.clipboard.as_mut().map(|c| c.get_text()) {
            Some(Ok(text)) => text,
            Some(Err(e)) => {
                log::error!("Failed to paste from clipboard: {}", e);
                self.toasts.error(format!("Paste failed: {}", e));
                return;
            }
            None => {
                self.toasts.warning("Clipboard not available");
                return;
            }
        };
        self.write_paste_text(&text);
    }

    fn write_paste_text(&mut self, text: &str) -> bool {
        if matches!(self.broadcast, BroadcastState::Armed { .. }) {
            self.toasts
                .warning("Paste was not sent while broadcast is armed");
            return false;
        }
        if self.focused_pane_read_only() {
            self.clear_terminal_input_tracking();
            return false;
        }
        let mut accepted = true;
        if !self.write_broadcast_paste(text) {
            let framed = crate::input::encode_paste(self.active_keyboard_mode(), text);
            let candidate = format!("{}{}", self.input_line_buffer, text);
            let guarded_origin = self.active_guarded_origin();
            let intercepted = guarded_origin.is_some_and(|origin| {
                self.guard_plugin_writes(
                    &candidate,
                    vec![crate::terminal::guardrails::GuardedWrite::new(
                        origin,
                        framed.clone(),
                    )],
                )
            });
            if !intercepted {
                accepted = self.write_terminal_protocol_input(&framed);
            }
            if guarded_origin.is_some() && !intercepted {
                self.track_guarded_paste_suffix(text);
            } else {
                self.input_line_buffer.clear();
            }
        }
        self.git_completions_visible = false;
        self.git_completions.clear();
        self.git_completion_index = None;
        // Clear any active selection — users expect paste to replace the
        // visual selection state, not leave it dangling.
        self.mouse_state.clear_selection();
        if let Some(terminal) = self.active_terminal_mut() {
            terminal.set_selection(None);
        }
        accepted
    }

    /// Handle keyboard input when in search mode
    /// Returns true if the input was handled by search mode
    fn handle_search_mode_input(&mut self, event: &winit::event::KeyEvent) -> bool {
        use winit::keyboard::{Key, NamedKey};

        if event.state != winit::event::ElementState::Pressed {
            return self.search.is_active();
        }

        if !self.search.is_active() {
            return false;
        }

        match &event.logical_key {
            Key::Named(NamedKey::Escape) => {
                self.search.cancel();
                if let Some(window) = &self.window {
                    window.request_redraw();
                }
            }
            Key::Named(NamedKey::Enter) => {
                // Confirm search and close input (but keep highlights)
                self.search.confirm();
                if self.copy_mode.is_active() {
                    self.jump_copy_cursor_to_search();
                }
                if let Some(window) = &self.window {
                    window.request_redraw();
                }
            }
            Key::Named(NamedKey::Backspace) => {
                if self.search_cursor_pos > 0 {
                    let cursor = self.search_cursor_pos;
                    if let Some(Some(previous)) =
                        self.with_search_and_active_grid(|search, grid| {
                            search.remove_char_before(cursor, grid)
                        })
                    {
                        self.search_cursor_pos = previous;
                    }
                }
                if let Some(window) = &self.window {
                    window.request_redraw();
                }
            }
            Key::Named(NamedKey::ArrowUp) => {
                // Previous match
                self.search.prev();
                self.scroll_to_current_match();
                if let Some(window) = &self.window {
                    window.request_redraw();
                }
            }
            Key::Named(NamedKey::ArrowDown) => {
                // Next match
                self.search.next();
                self.scroll_to_current_match();
                if let Some(window) = &self.window {
                    window.request_redraw();
                }
            }
            Key::Named(NamedKey::ArrowLeft) => {
                // Move to previous character boundary
                let query = self.search.query();
                if self.search_cursor_pos > 0 {
                    if let Some((prev_pos, _)) =
                        query[..self.search_cursor_pos].char_indices().next_back()
                    {
                        self.search_cursor_pos = prev_pos;
                    }
                }
                if let Some(window) = &self.window {
                    window.request_redraw();
                }
            }
            Key::Named(NamedKey::ArrowRight) => {
                // Move to next character boundary
                let query = self.search.query();
                if self.search_cursor_pos < query.len() {
                    if let Some((_, ch)) = query[self.search_cursor_pos..].char_indices().next() {
                        self.search_cursor_pos += ch.len_utf8();
                    }
                }
                if let Some(window) = &self.window {
                    window.request_redraw();
                }
            }
            Key::Named(NamedKey::Home) => {
                self.search_cursor_pos = 0;
                if let Some(window) = &self.window {
                    window.request_redraw();
                }
            }
            Key::Named(NamedKey::End) => {
                self.search_cursor_pos = self.search.query().len();
                if let Some(window) = &self.window {
                    window.request_redraw();
                }
            }
            Key::Character(c) => {
                // Check for special key combinations
                if self.modifiers.alt_key() {
                    match c.as_str() {
                        "c" | "C" => {
                            // Toggle case sensitivity (Alt+C)
                            self.with_search_and_active_grid(|search, grid| {
                                search.toggle_case_sensitive(grid);
                            });
                            if let Some(window) = &self.window {
                                window.request_redraw();
                            }
                            return true;
                        }
                        "r" | "R" => {
                            // Toggle regex mode (Alt+R)
                            self.with_search_and_active_grid(|search, grid| {
                                search.toggle_regex(grid);
                            });
                            if let Some(window) = &self.window {
                                window.request_redraw();
                            }
                            return true;
                        }
                        "f" | "F" => {
                            // Toggle filter mode (Alt+F)
                            if self.copy_mode.is_active() {
                                self.toasts.warning(
                                    "Filter mode is unavailable inside copy mode; matches remain highlighted",
                                );
                            } else {
                                self.search.toggle_filter();
                            }
                            if let Some(window) = &self.window {
                                window.request_redraw();
                            }
                            return true;
                        }
                        _ => {}
                    }
                }

                // Regular character input
                let cursor = self.search_cursor_pos;
                if self
                    .with_search_and_active_grid(|search, grid| search.insert_text(cursor, c, grid))
                    .unwrap_or(false)
                {
                    self.search_cursor_pos += c.len();
                }
                if let Some(window) = &self.window {
                    window.request_redraw();
                }
            }
            _ => {}
        }

        true // Input was handled by search mode
    }

    /// Scroll the terminal view to show the current search match
    fn scroll_to_current_match(&mut self) {
        let Some(search_match) = self.search.current() else {
            return;
        };
        let match_row = search_match.row;

        let Some(grid) = self
            .active_terminal_mut()
            .map(termeric_core::Terminal::grid_mut)
        else {
            return;
        };
        let visible_rows = grid.rows() as i32;
        let scrollback_len = grid.scrollback_len() as i32;

        // Calculate the required scroll offset to bring the match into view
        // match_row is negative for scrollback, 0+ for visible grid
        if match_row < 0 {
            // Match is in scrollback - need to scroll up
            // Convert negative row index to scroll offset
            let lines_back = (-match_row) as usize;
            // Add a few lines of context
            let target_offset = lines_back.saturating_add(visible_rows as usize / 2);
            let clamped = target_offset.min(scrollback_len as usize);
            grid.set_scroll_offset(clamped);
        } else {
            // Match is in visible area or below current view
            let current_offset = grid.scroll_offset();
            if current_offset > 0 {
                // If scrolled up, check if match is visible
                let effective_row = match_row + current_offset as i32;
                if effective_row < 0 || effective_row >= visible_rows {
                    // Match is not in view, adjust scroll
                    grid.set_scroll_offset(0);
                }
            }
        }
    }

    /// Run a custom command in the current terminal tab
    fn run_custom_command(&mut self, command: &str) {
        log::info!("Running custom command: {}", command);
        for chunk in command_keystrokes(command) {
            self.write_to_pty(&chunk);
        }
    }

    /// Re-sync the command palette with currently registered plugins.
    ///
    /// Called after a plugin is registered or deregistered at runtime.
    fn refresh_plugin_commands(&mut self) {
        use crate::ui::command_palette::palette::{CommandSource, RegisteredCommand};

        let plugin_commands = self.plugin_manager.all_commands();
        for pc in &plugin_commands {
            log::debug!("Refreshing plugin command: {} - {}", pc.id, pc.description);
        }
        let keybindings = self.plugin_manager.all_keybindings();

        let palette_cmds: Vec<RegisteredCommand> = plugin_commands
            .iter()
            .map(|pc| {
                let shortcut = keybindings.iter().find(|k| k.command_id == pc.id).map(|k| {
                    let mut parts = Vec::new();
                    if k.ctrl {
                        parts.push("Ctrl".to_string());
                    }
                    if k.alt {
                        parts.push("Alt".to_string());
                    }
                    if k.shift {
                        parts.push("Shift".to_string());
                    }
                    parts.push(k.key.to_uppercase());
                    parts.join("+")
                });
                let plugin_source = pc
                    .id
                    .split_once('.')
                    .map(|(prefix, _)| prefix.to_string())
                    .unwrap_or_else(|| "plugin".to_string());
                RegisteredCommand {
                    id: pc.id.clone(),
                    title: pc.name.clone(),
                    category: pc.category.clone().unwrap_or_else(|| "Plugins".to_string()),
                    shortcut,
                    icon: None,
                    source: CommandSource::Plugin(plugin_source),
                    destructive: false,
                }
            })
            .collect();

        self.command_palette.replace_plugin_commands(palette_cmds);

        log::info!(
            "Refreshed plugin commands: {} plugin(s), {} command(s)",
            self.plugin_manager.count(),
            plugin_commands.len(),
        );
    }

    fn refresh_launch_profile_commands(&mut self) {
        use crate::ui::command_palette::palette::{CommandSource, RegisteredCommand};

        let commands = self
            .config
            .profiles
            .iter()
            .map(|(name, profile)| {
                let detail = profile.description.as_deref().map(|description| {
                    let mut value: String = description.chars().take(72).collect();
                    if description.chars().count() > 72 {
                        value.push('…');
                    }
                    value
                });
                RegisteredCommand {
                    id: format!("profile.open.{name}"),
                    title: detail.map_or_else(
                        || format!("New Tab: {name}"),
                        |detail| format!("New Tab: {name} — {detail}"),
                    ),
                    category: "Terminal Profiles".into(),
                    shortcut: None,
                    icon: None,
                    source: CommandSource::LaunchProfiles,
                    destructive: false,
                }
            })
            .collect();
        self.command_palette
            .replace_launch_profile_commands(commands);
    }

    /// Save current configuration to file
    fn save_config(&mut self) -> Result<String> {
        let config_path = Config::config_path();

        // Ensure config directory exists
        if let Some(parent) = config_path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        // Build current config, syncing any live values
        let mut config = self.config.clone();
        if let Some(renderer) = &self.renderer {
            config.font.size = renderer.font_size();
        }
        config.shortcuts = self.shortcut_registry.user_overrides();
        config.validate()?;

        let config_content = toml::to_string_pretty(&config)?;

        std::fs::write(&config_path, config_content)?;
        // Capture the new mtime so the hot-reload poller doesn't treat
        // our own write as an external change.
        self.last_config_mtime = std::fs::metadata(&config_path)
            .and_then(|m| m.modified())
            .ok();
        Ok(config_path.display().to_string())
    }
}

fn native_window_title(quick_access: bool, active_tab_title: Option<&str>) -> String {
    let application = if quick_access {
        "Termeric Quick Access"
    } else {
        "Termeric"
    };
    match active_tab_title.filter(|title| !title.trim().is_empty()) {
        Some(title) => format!("{title} — {application}"),
        None => application.to_string(),
    }
}

fn update_visible_url_cache(
    cache: &mut UrlCache,
    grid: &termeric_core::Grid,
    detector: &UrlDetector,
) {
    let rows = grid.rows();
    let cols = grid.cols();
    cache.resize(rows);
    for row in 0..rows {
        let mut line = String::with_capacity(cols);
        let mut byte_to_col = vec![0];
        for col in 0..cols {
            let Some(cell) = grid.cell_scrolled(row, col) else {
                continue;
            };
            if cell.is_wide_char_spacer() {
                continue;
            }
            let start = line.len();
            match &cell.content {
                termeric_core::CellContent::Empty => line.push(' '),
                termeric_core::CellContent::Char(character) => line.push(*character),
                termeric_core::CellContent::Grapheme(grapheme) => line.push_str(grapheme),
            }
            let end = line.len();
            byte_to_col.resize(end + 1, col);
            byte_to_col[start] = col;
            byte_to_col[end] = col + cell.width();
        }
        cache.update_row_mapped(row, &line, &byte_to_col, detector);
    }
}

fn cursor_opacity_at(blink: bool, elapsed_seconds: f32, period_seconds: f32) -> f32 {
    if !blink {
        return 1.0;
    }
    let half_period = (period_seconds / 2.0).max(0.001);
    if (elapsed_seconds / half_period) as u64 % 2 == 0 {
        1.0
    } else {
        0.3
    }
}

fn resolve_cursor_presentation(
    configured_style: crate::config::CursorStyle,
    configured_blink: bool,
    terminal_style: Option<termeric_core::term::TerminalCursorStyle>,
    terminal_blink: Option<bool>,
) -> (crate::config::CursorStyle, bool) {
    let (style, blink) = match terminal_style {
        Some(terminal_style) => {
            let style = match terminal_style.shape {
                termeric_core::term::TerminalCursorShape::Block => {
                    crate::config::CursorStyle::Block
                }
                termeric_core::term::TerminalCursorShape::Underline => {
                    crate::config::CursorStyle::Underline
                }
                termeric_core::term::TerminalCursorShape::Beam => crate::config::CursorStyle::Beam,
            };
            (style, terminal_style.blinking)
        }
        None => (configured_style, configured_blink),
    };
    (style, terminal_blink.unwrap_or(blink))
}

fn blink_phase_visible(elapsed: std::time::Duration, half_period: std::time::Duration) -> bool {
    (elapsed.as_nanos() / half_period.as_nanos()) % 2 == 0
}

fn next_blink_boundary(start: Instant, now: Instant, half_period: std::time::Duration) -> Instant {
    let elapsed = now.saturating_duration_since(start);
    let completed = elapsed.as_nanos() / half_period.as_nanos();
    start + half_period.mul_f64((completed + 1) as f64)
}

fn select_plugin_target(
    active_plugin_id: Option<&str>,
    available_plugin_ids: &[String],
) -> std::result::Result<String, String> {
    if let Some(active) = active_plugin_id {
        if available_plugin_ids.iter().any(|id| id == active) {
            return Ok(active.to_string());
        }
    }

    let mut unique = available_plugin_ids.to_vec();
    unique.sort();
    unique.dedup();
    match unique.as_slice() {
        [only] => Ok(only.clone()),
        [] => Err("Open an AI plugin tab before using Explain".into()),
        _ => Err("Multiple AI plugin tabs are open; activate the intended one first".into()),
    }
}

fn command_keystrokes(command: &str) -> [Vec<u8>; 3] {
    [
        b"\x15".to_vec(),
        command.trim_end_matches(['\r', '\n']).as_bytes().to_vec(),
        b"\r".to_vec(),
    ]
}

fn sprout_menu_render_info(
    menu: &crate::harvest::SproutMenu,
    padding: f32,
    cell_width: f32,
    cell_height: f32,
    content_left: f32,
    content_top: f32,
    window_width: f32,
    window_height: f32,
) -> SproutMenuRenderInfo {
    let layout = crate::harvest::SproutMenuLayout::calculate(
        menu.row,
        menu.col,
        menu.actions.len(),
        menu.width(),
        window_width,
        window_height,
        content_left,
        content_top,
        padding,
        cell_width,
        cell_height,
    );
    SproutMenuRenderInfo {
        actions: menu
            .actions
            .iter()
            .map(|action| {
                (
                    menu.action_label(*action),
                    action.icon(),
                    menu.action_enabled(*action),
                )
            })
            .collect(),
        selected: menu.selected,
        opacity: menu.opacity(),
        layout,
    }
}

/// Minimal RFC 4648 base64 encoder (standard alphabet) used for the OSC 52
/// clipboard reply. Keeps us off an external `base64` dependency.
fn base64_encode_standard(input: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(input.len().div_ceil(3) * 4);
    let mut chunks = input.chunks_exact(3);
    for chunk in &mut chunks {
        let n = ((chunk[0] as u32) << 16) | ((chunk[1] as u32) << 8) | (chunk[2] as u32);
        out.push(ALPHABET[((n >> 18) & 0x3F) as usize] as char);
        out.push(ALPHABET[((n >> 12) & 0x3F) as usize] as char);
        out.push(ALPHABET[((n >> 6) & 0x3F) as usize] as char);
        out.push(ALPHABET[(n & 0x3F) as usize] as char);
    }
    let rem = chunks.remainder();
    match rem.len() {
        1 => {
            let n = (rem[0] as u32) << 16;
            out.push(ALPHABET[((n >> 18) & 0x3F) as usize] as char);
            out.push(ALPHABET[((n >> 12) & 0x3F) as usize] as char);
            out.push('=');
            out.push('=');
        }
        2 => {
            let n = ((rem[0] as u32) << 16) | ((rem[1] as u32) << 8);
            out.push(ALPHABET[((n >> 18) & 0x3F) as usize] as char);
            out.push(ALPHABET[((n >> 12) & 0x3F) as usize] as char);
            out.push(ALPHABET[((n >> 6) & 0x3F) as usize] as char);
            out.push('=');
        }
        _ => {}
    }
    out
}

#[cfg(test)]
mod base64_tests {
    use super::{
        base64_encode_standard, blink_phase_visible, command_keystrokes, cursor_opacity_at,
        next_blink_boundary, resolve_cursor_presentation, select_plugin_target,
    };
    use std::time::{Duration, Instant};

    #[test]
    fn disabled_cursor_blink_is_fully_opaque() {
        assert_eq!(cursor_opacity_at(false, 50.0, 0.2), 1.0);
    }

    #[test]
    fn cursor_blink_rate_controls_the_cycle() {
        assert!((cursor_opacity_at(true, 0.0, 1.0) - 1.0).abs() < f32::EPSILON);
        assert!((cursor_opacity_at(true, 0.5, 1.0) - 0.3).abs() < f32::EPSILON);
    }

    #[test]
    fn terminal_cursor_style_overrides_shape_and_blink_until_reset() {
        use crate::config::CursorStyle;
        use termeric_core::term::{TerminalCursorShape, TerminalCursorStyle};

        assert_eq!(
            resolve_cursor_presentation(CursorStyle::Beam, false, None, None),
            (CursorStyle::Beam, false)
        );
        let mut terminal = termeric_core::Terminal::new(10, 2);
        terminal.process(b"\x1b[3 q");
        assert_eq!(
            resolve_cursor_presentation(
                CursorStyle::Beam,
                false,
                terminal.cursor_style(),
                terminal.cursor_blink_override(),
            ),
            (CursorStyle::Underline, true)
        );
        assert_eq!(
            terminal.cursor_style().map(|style| style.shape),
            Some(TerminalCursorShape::Underline)
        );
        terminal.process(b"\x1b[0 q");
        assert_eq!(
            resolve_cursor_presentation(
                CursorStyle::Beam,
                false,
                terminal.cursor_style(),
                terminal.cursor_blink_override(),
            ),
            (CursorStyle::Block, true)
        );
        terminal.process(b"\x1b[?12l");
        assert_eq!(
            resolve_cursor_presentation(
                CursorStyle::Beam,
                true,
                terminal.cursor_style(),
                terminal.cursor_blink_override(),
            ),
            (CursorStyle::Block, false)
        );
        terminal.process(b"\x1b[?2026h\x1b[?12h");
        assert_eq!(terminal.cursor_blink_override(), Some(true));
        assert_eq!(terminal.presentation_cursor_blink_override(), Some(false));
        terminal.process(b"\x1b[?2026l");
        assert_eq!(terminal.presentation_cursor_blink_override(), Some(true));
        terminal.process(b"\x1b[4 q");
        assert_eq!(terminal.cursor_blink_override(), None);
        assert_eq!(
            resolve_cursor_presentation(
                CursorStyle::Beam,
                true,
                terminal.cursor_style(),
                terminal.cursor_blink_override(),
            ),
            (CursorStyle::Underline, false)
        );
        terminal.process(b"\x1bc");
        assert_eq!(
            resolve_cursor_presentation(
                CursorStyle::Beam,
                false,
                terminal.cursor_style(),
                terminal.cursor_blink_override(),
            ),
            (CursorStyle::Beam, false)
        );
        let _type_check: Option<TerminalCursorStyle> = terminal.cursor_style();
    }

    #[test]
    fn text_blink_phases_and_deadlines_are_instant_based() {
        let half = Duration::from_millis(250);
        assert!(blink_phase_visible(Duration::ZERO, half));
        assert!(!blink_phase_visible(Duration::from_millis(250), half));
        assert!(blink_phase_visible(Duration::from_millis(500), half));

        let start = Instant::now();
        assert_eq!(
            next_blink_boundary(start, start + Duration::from_millis(251), half),
            start + Duration::from_millis(500)
        );
    }

    #[test]
    fn plugin_target_prefers_active_then_unambiguous_tab() {
        let plugins = vec!["claude-code".to_string(), "codex".to_string()];
        assert_eq!(
            select_plugin_target(Some("codex"), &plugins).unwrap(),
            "codex"
        );
        assert_eq!(
            select_plugin_target(None, &["claude-code".to_string()]).unwrap(),
            "claude-code"
        );
        assert!(select_plugin_target(None, &plugins).is_err());
        assert!(select_plugin_target(None, &[]).is_err());
    }

    #[test]
    fn dynamic_run_command_builds_single_enter_payload() {
        assert_eq!(
            command_keystrokes("cargo test"),
            [b"\x15".to_vec(), b"cargo test".to_vec(), b"\r".to_vec()]
        );
        assert_eq!(
            command_keystrokes("cargo test\n"),
            [b"\x15".to_vec(), b"cargo test".to_vec(), b"\r".to_vec()]
        );
    }

    #[test]
    fn test_base64_empty() {
        assert_eq!(base64_encode_standard(b""), "");
    }

    #[test]
    fn test_base64_rfc_vectors() {
        assert_eq!(base64_encode_standard(b"f"), "Zg==");
        assert_eq!(base64_encode_standard(b"fo"), "Zm8=");
        assert_eq!(base64_encode_standard(b"foo"), "Zm9v");
        assert_eq!(base64_encode_standard(b"foob"), "Zm9vYg==");
        assert_eq!(base64_encode_standard(b"fooba"), "Zm9vYmE=");
        assert_eq!(base64_encode_standard(b"foobar"), "Zm9vYmFy");
    }

    #[test]
    fn test_base64_binary() {
        // 00 FF 80 → AP+A, 7F 01 → fwE=
        let bytes = [0x00, 0xff, 0x80, 0x7f, 0x01];
        assert_eq!(base64_encode_standard(&bytes), "AP+AfwE=");
    }
}
