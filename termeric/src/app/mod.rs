//! Application state and event handling

mod accessibility;
mod agent_changes;
mod agent_features;
mod agent_hooks;
mod agent_view;
mod agent_view_input;
mod agent_view_tabs;
mod agents;
mod agents_panel;
mod application;
mod broadcast;
mod clipboard;
mod commands_input;
mod configuration;
mod context_picker;
mod control_actions;
mod copy_mode;
mod desktop_io;
mod diff_review;
mod domain_picker;
mod event_loop;
mod file_drop;
mod font_feature_editor;
mod font_range_editor;
mod frame;
mod git_completion;
mod guardrail_input;
mod harvest_actions;
mod ime;
mod info_overlay;
mod kitty_file_transfer;
mod live_domain_control;
mod modal_layer;
mod mouse;
mod mouse_selection;
mod overlay_scroll;
mod palette_input;
mod plugin_actions;
mod plugin_launch;
mod plugin_panes;
mod plugin_restart;
mod query_paste;
mod redraw_scheduler;
mod renderer_recovery;
mod runtime_plugins;
mod search_input;
mod search_presentation;
mod session_export;
mod session_input;
mod sessions;
mod settings_input;
mod shortcuts_input;
pub mod state;
mod supervisor;
mod system_accessibility;
mod tab_drag;
mod tab_rename;
mod terminal_events;
mod terminal_input;
#[cfg(test)]
mod tests;
mod text_field_keys;
mod theme_picker;
mod transcripts;
mod unicode_picker;
mod workspace_actions;
mod workspace_menu;
mod workspaces;

use anyhow::Result;
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

use self::control_actions::Requester;
use self::redraw_scheduler::{RedrawReason, RedrawScheduler};
use self::state::AppState;
use crate::config::theme::Theme;
use crate::config::{Config, SplashMode};
use crate::harvest::HarvestMode;
use crate::input::keybindings::{KeyAction, KeyBindingManager};
use crate::input::{Action, MouseReportButton, MouseState, ShortcutRegistry};
use crate::notifications::{Bell, Notifications};
use crate::panes::{PaneId, PaneManager, SplitDirection};
use crate::plugin::{PluginContext, PluginManager};
use crate::renderer::types::{
    CellarEntryInfo, CellarRenderInfo, HarvestRenderInfo, IndicatorRenderInfo, PaneRenderData,
    SproutMenuRenderInfo,
};
use crate::renderer::{Renderer, TextBlinkPhases, VisibleTextBlink};
use crate::tabs::{RoutedTerminalEvent, TabManager};
use crate::terminal::copy_mode::CopyMode;
use crate::terminal::guardrails::GuardrailOverlay;
use crate::terminal::search::Search;
use crate::terminal::splash::show_splash;
use crate::terminal::url::{UrlCache, UrlDetector};
use crate::ui::command_palette::PaletteOwner;
use crate::ui::command_palette::{CommandPalette, PaletteResult};
use crate::ui::context_picker::ContextPicker;
use crate::ui::diff_review::DiffReviewOverlay;
use crate::ui::domain_picker::DomainPicker;
use crate::ui::info_overlay::InfoOverlay;
use crate::ui::settings::{SettingsAction, SettingsPanel};
use crate::ui::shortcuts_panel::{ShortcutsAction, ShortcutsPanel};
use crate::ui::unicode_picker::UnicodePicker;

pub use application::Application;

/// The Wayland app ID and X11 `WM_CLASS` of ordinary windows. Desktops match
/// it to the installed desktop entry of the same name for the window's icon
/// and taskbar grouping, so it never changes once released.
pub const APP_ID: &str = "org.termeric.Termeric";
/// The quick-access window's identity, separate so compositor rules can treat
/// it apart from ordinary windows. A hidden desktop entry of the same name
/// gives it the icon.
pub const QUICK_ACCESS_APP_ID: &str = "org.termeric.Termeric.QuickAccess";

/// Window and cell sizes for laying out UI outside a frame. Before a
/// renderer exists (headless tests) a nominal 800×600 window with 8×16
/// cells and no tab bar stands in.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct UiMetrics {
    pub width: f32,
    pub height: f32,
    pub tab_bar_height: f32,
    pub cell_width: f32,
    pub cell_height: f32,
}

/// The window a background task returns its result to: the window's control
/// ID, which no later window of the process takes. A winit `WindowId` can't
/// name it, since on Wayland that comes from the surface's address, which a
/// window opened after this one closes can reuse, and the request counters a
/// result carries start alike in every window.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct WindowKey(u64);

#[derive(Debug)]
pub(crate) enum AppEvent {
    Pty,
    Git,
    Transcript,
    /// A docked agent view's agent wrote output.
    AgentView,
    ControlRequest {
        request: crate::control::protocol::Request,
        /// The requesting process, where the platform reports it.
        peer_process: Option<u32>,
        respond_to: std::sync::mpsc::SyncSender<
            std::result::Result<crate::control::protocol::Response, String>,
        >,
    },
    LiveDomainControlCompleted {
        window: WindowKey,
        control: LiveDomainControl,
        result: std::result::Result<crate::domain::model::DomainSnapshot, String>,
    },
    LiveDomainTopology {
        window: WindowKey,
        domain_id: String,
        result: std::result::Result<crate::domain::DomainTopologyEvent, String>,
    },
    DomainPickerLoaded {
        window: WindowKey,
        generation: u64,
        result: std::result::Result<Vec<crate::domain::model::DomainSummary>, String>,
    },
    DomainPickerPrepared {
        window: WindowKey,
        generation: u64,
        result: std::result::Result<crate::domain::model::DomainSnapshot, String>,
    },
    ContextGathered {
        window: WindowKey,
        request_id: u64,
        result: std::result::Result<String, String>,
    },
    FileUploadCompleted {
        window: WindowKey,
        request_id: u64,
        result: std::result::Result<FileUploadCompletion, String>,
    },
    FileUploadProgress {
        window: WindowKey,
        request_id: u64,
        bytes_sent: u64,
        total_bytes: u64,
    },
    KittyFileTransferCompleted {
        window: WindowKey,
        token: u64,
        result: kitty_file_transfer::WorkerCompletion,
    },
    KittyFileUploadCompleted {
        window: WindowKey,
        token: u64,
        result: kitty_file_transfer::UploadWorkerCompletion,
    },
    StructuredPluginCompleted(crate::plugin::structured::StructuredHandlerCompletion),
    AccessKit(accesskit_winit::Event),
    RendererDeviceLost {
        window: WindowKey,
        generation: u64,
        failure: crate::renderer::RendererDeviceFailure,
    },
    BackgroundImageLoaded {
        window: WindowKey,
        generation: u64,
        path: PathBuf,
        result: std::result::Result<crate::renderer::DecodedBackgroundImage, String>,
    },
    SystemAccessibilityChanged(system_accessibility::SystemAccessibilityPreferences),
    SystemAccessibilityUnavailable(String),
}

impl AppEvent {
    /// For a result a background task returns to one window and nothing
    /// else: that window, and why it is redrawn.
    fn window_target(&self) -> Option<(WindowKey, RedrawReason)> {
        Some(match self {
            Self::RendererDeviceLost { window, .. } => (*window, RedrawReason::SurfaceRecovery),
            Self::BackgroundImageLoaded { window, .. }
            | Self::LiveDomainTopology { window, .. }
            | Self::DomainPickerLoaded { window, .. }
            | Self::ContextGathered { window, .. }
            | Self::FileUploadCompleted { window, .. }
            | Self::FileUploadProgress { window, .. }
            | Self::KittyFileTransferCompleted { window, .. }
            | Self::KittyFileUploadCompleted { window, .. } => {
                (*window, RedrawReason::BackgroundTask)
            }
            _ => return None,
        })
    }
}

impl App {
    /// The key background work this window starts tags its results with.
    /// Application assigns the control ID before the window is created, and
    /// every such task needs a created window, so no task sees another ID.
    pub(crate) fn window_key(&self) -> WindowKey {
        WindowKey(self.control_window_id)
    }

    /// Apply a result `AppEvent::window_target` routed to this window.
    fn handle_window_event_result(&mut self, event: AppEvent) {
        match event {
            AppEvent::RendererDeviceLost {
                generation,
                failure,
                ..
            } => self.handle_renderer_device_lost(generation, failure),
            AppEvent::BackgroundImageLoaded {
                generation,
                path,
                result,
                ..
            } => self.handle_background_image_loaded(generation, path, result),
            AppEvent::LiveDomainTopology {
                domain_id, result, ..
            } => self.handle_live_domain_topology(domain_id, result),
            AppEvent::DomainPickerLoaded {
                generation, result, ..
            } => self.handle_domain_picker_loaded(generation, result),
            AppEvent::ContextGathered {
                request_id, result, ..
            } => self.handle_context_gathered(request_id, result),
            AppEvent::FileUploadCompleted {
                request_id, result, ..
            } => self.handle_file_upload_completed(request_id, result),
            AppEvent::FileUploadProgress {
                request_id,
                bytes_sent,
                total_bytes,
                ..
            } => self.handle_file_upload_progress(request_id, bytes_sent, total_bytes),
            AppEvent::KittyFileTransferCompleted { token, result, .. } => {
                self.handle_kitty_file_transfer_completed(token, result)
            }
            AppEvent::KittyFileUploadCompleted { token, result, .. } => {
                self.handle_kitty_file_upload_completed(token, result)
            }
            _ => unreachable!("not a single-window result: {event:?}"),
        }
    }
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
        direction: crate::panes::SplitDirection,
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
        preset: crate::panes::PaneLayoutPreset,
    },
    MovePane {
        domain_id: String,
        remote_pane_id: u32,
        source_tab_id: u32,
        destination_tab_id: u32,
        target_pane_id: u32,
        direction: crate::panes::SplitDirection,
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

const PLUGIN_PROCESS_MAX_WINDOWS: usize = 8;
const PLUGIN_PROCESS_MAX_TABS: usize = 16;
const PLUGIN_PROCESS_MAX_PANES: usize = 32;
const PLUGIN_PROCESS_MAX_DIVIDERS: usize = 31;
const PLUGIN_PROCESS_MAX_METADATA_BYTES: usize = 2 * 1024;

fn plugin_process_inventory_budget() -> ControlInventoryBudget {
    ControlInventoryBudget {
        tabs: PLUGIN_PROCESS_MAX_TABS,
        panes: PLUGIN_PROCESS_MAX_PANES,
        dividers: PLUGIN_PROCESS_MAX_DIVIDERS,
        metadata_bytes: PLUGIN_PROCESS_MAX_METADATA_BYTES,
    }
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
        use crate::control::protocol::WindowInventory;

        let (pixel_width, pixel_height) = self
            .renderer
            .as_ref()
            .map(crate::renderer::Renderer::window_size)
            .unwrap_or((self.config.window.width, self.config.window.height));

        let (tabs, mut truncated) = match self.tabs.as_ref() {
            Some(tabs) => self.control_tab_inventory(tabs, budget),
            None => (Vec::new(), false),
        };
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

    /// The window's tabs as control inventory, and whether any were left out
    /// for the budget.
    fn control_tab_inventory(
        &self,
        tabs: &TabManager,
        budget: &mut ControlInventoryBudget,
    ) -> (Vec<crate::control::protocol::TabInventory>, bool) {
        use crate::control::protocol::{
            DividerInventory, DividerKind, MAX_DIVIDERS_PER_TAB, MAX_PANES_PER_TAB,
            MAX_TABS_PER_WINDOW, PaneInventory, TabInventory, TabKind,
        };

        let active = tabs.active_tab_id();
        let all_tabs = tabs.all_tabs();
        let tab_limit = MAX_TABS_PER_WINDOW.min(budget.tabs);
        let truncated = all_tabs.len() > tab_limit;
        budget.tabs -= all_tabs.len().min(tab_limit);
        let tabs = all_tabs
            .iter()
            .take(tab_limit)
            .map(|tab| {
                let mut tab_truncated = false;
                let manager = &tab.panes;
                let focused = manager.focused_id();
                let all = manager.iter_panes();
                let pane_limit = MAX_PANES_PER_TAB.min(budget.panes);
                tab_truncated |= all.len() > pane_limit;
                budget.panes -= all.len().min(pane_limit);
                let mut panes = all
                    .into_iter()
                    .take(pane_limit)
                    .map(|pane| PaneInventory {
                        id: pane.id.0,
                        // A tab's only pane has no title of its own.
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
                    .collect::<Vec<_>>();
                panes.sort_by_key(|pane| pane.id);
                let all = manager.get_dividers();
                let divider_limit = MAX_DIVIDERS_PER_TAB
                    .min(budget.dividers)
                    .min(panes.len().saturating_sub(1));
                tab_truncated |= all.len() > divider_limit;
                budget.dividers -= all.len().min(divider_limit);
                let mut dividers = all
                    .into_iter()
                    .take(divider_limit)
                    .map(|divider| {
                        let (id, kind) = match divider.target {
                            crate::panes::DividerTarget::Local(id) => (id, DividerKind::Local),
                            crate::panes::DividerTarget::Domain(id) => {
                                (u64::from(id), DividerKind::LiveDomain)
                            }
                        };
                        DividerInventory {
                            id,
                            kind,
                            direction: if divider.vertical {
                                crate::panes::SplitDirection::Vertical
                            } else {
                                crate::panes::SplitDirection::Horizontal
                            },
                            ratio_millis: divider.ratio_millis,
                        }
                    })
                    .collect::<Vec<_>>();
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
        (tabs, truncated)
    }
}

fn control_metadata(value: &str, remaining: &mut usize) -> (String, bool) {
    const MAX_FIELD_BYTES: usize = 1_024;
    let limit = (*remaining).min(MAX_FIELD_BYTES);
    let mut output = String::with_capacity(value.len().min(limit));
    let mut truncated = false;
    for character in value.chars() {
        if crate::safe_text::is_unsafe_display_char(character) {
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
static AGENT_VIEW_WAKE_PENDING: AtomicBool = AtomicBool::new(false);

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

/// Wakes the event loop when an agent view's agent writes output. Wakes
/// coalesce until the loop drains every window's sessions.
pub(crate) fn agent_view_event_waker() -> Arc<dyn Fn() + Send + Sync> {
    Arc::new(|| {
        if AGENT_VIEW_WAKE_PENDING.swap(true, Ordering::AcqRel) {
            return;
        }
        let sent = EVENT_LOOP_PROXY
            .get()
            .is_some_and(|proxy| proxy.send_event(AppEvent::AgentView).is_ok());
        if !sent {
            AGENT_VIEW_WAKE_PENDING.store(false, Ordering::Release);
        }
    })
}

#[derive(Debug, Clone)]
struct TabRenameState {
    tab_id: usize,
    original: Option<String>,
    /// The draft title. The tab shows it in place with no caret, so typing
    /// and Backspace always work at its end.
    input: crate::ui::text_input::TextInput,
}

impl TabRenameState {
    /// A draft starting from `title`, cut to one line of `max_chars`.
    fn new(tab_id: usize, original: Option<String>, title: &str, max_chars: usize) -> Self {
        use crate::ui::text_input::{TextInput, TextLimit};
        let title: String = title
            .chars()
            .filter(|character| !crate::safe_text::breaks_single_line(*character))
            .take(max_chars)
            .collect();
        let mut input = TextInput::new(TextLimit::Chars(max_chars));
        input.replace(&title);
        Self {
            tab_id,
            original,
            input,
        }
    }
}

#[derive(Debug, Clone)]
struct DividerDrag {
    tab_id: usize,
    domain: Option<(String, u32)>,
    target: crate::panes::DividerTarget,
    last_ratio_millis: u16,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct TerminalMouseCapture {
    owner: crate::tabs::TerminalIdentity,
    button: MouseReportButton,
}

#[derive(Debug, Clone)]
struct TabDrag {
    tab_id: usize,
    live: Option<LiveDomainTabDrag>,
    /// Where the pointer holds the tab, once pressed on it.
    grab: Option<TabGrab>,
}

/// Where the pointer holds a dragged tab, as positions along the tab bar
/// (x for the top bar, y for the side rail).
#[derive(Debug, Clone, Copy)]
struct TabGrab {
    pressed_at: f32,
    /// From the tab's leading edge to the pointer.
    offset: f32,
    pointer: f32,
    /// The pointer moved far enough for a drag rather than a click.
    moving: bool,
}

/// A tab sliding into its slot after the dragged tab passed it, or the
/// dragged tab after its release.
#[derive(Debug, Clone, Copy)]
struct TabSlide {
    tab_id: usize,
    /// Its distance from the slot when the slide began.
    from: f32,
    started: Instant,
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

/// What a request to copy the selection did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CopyOutcome {
    NothingSelected,
    Copied,
    /// The clipboard refused the text; the person has been told why.
    Failed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct PendingUnicodePicker {
    terminal: crate::tabs::TerminalIdentity,
}

/// A multi-step flow in the info overlay. There is at most one, so starting
/// a flow ends whichever ran before.
enum OverlayFlow {
    /// Navigation and confirmation for installed third-party plugins.
    RuntimePlugins(PendingRuntimePluginManagement),
    /// Navigation and edits of Unicode-range font overrides.
    FontRanges(font_range_editor::PendingFontRangeManagement),
    /// Navigation and edits of OpenType feature overrides.
    FontFeatures(font_feature_editor::PendingFontFeatureManagement),
    /// An exact source and live-domain destination awaiting upload consent.
    FileUpload(PendingFileUpload),
    /// Reset Configuration awaiting confirmation.
    ConfigReset { request_id: u64 },
    /// A recorded shortcut that another action uses, awaiting confirmation
    /// before `holder` loses it to `action`.
    ShortcutReassign {
        request_id: u64,
        action: crate::input::keybindings::BindableAction,
        chord: crate::input::keybindings::KeyChord,
        holder: crate::input::keybindings::BindableAction,
    },
    /// Opening a workspace over tabs running programs, awaiting confirmation.
    WorkspaceOpen {
        request_id: u64,
        workspace_id: String,
    },
    /// Deleting a workspace or closed-tab snapshot, awaiting confirmation.
    WorkspaceDelete {
        request_id: u64,
        workspace_id: String,
        closed_tab: bool,
    },
}

#[derive(Debug, Clone)]
struct PendingRuntimePluginManagement {
    request_id: u64,
    view: RuntimePluginManagerView,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct PendingPluginWindowClose {
    plugin_id: String,
    plugin_generation: u64,
    result_request_id: u64,
}

impl PendingPluginWindowClose {
    fn matches(&self, completion: &crate::plugin::structured::StructuredHandlerCompletion) -> bool {
        self.plugin_id == completion.plugin_id
            && self.plugin_generation == completion.plugin_generation
            && self.result_request_id == completion.request_id
    }
}

#[derive(Debug, Clone)]
enum RuntimePluginManagerView {
    List {
        page: usize,
    },
    InstallPath {
        page: usize,
    },
    ReviewInstall {
        page: usize,
        preview: crate::plugin::runtime::PluginInstallPreview,
        grants: crate::plugin::permission::PluginPermissions,
    },
    ReviewUpdate {
        page: usize,
        preview: Box<crate::plugin::runtime::PluginUpdatePreview>,
        grants: crate::plugin::permission::PluginPermissions,
    },
    ReviewPermissions {
        page: usize,
        preview: Box<crate::plugin::runtime::PluginGrantsPreview>,
        grants: crate::plugin::permission::PluginPermissions,
    },
    Detail {
        plugin_id: String,
        page: usize,
    },
    DefaultProfile {
        plugin_id: String,
        page: usize,
        profile_page: usize,
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
    source_identity: (u64, u64),
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

fn clear_agent_view_wake_pending() {
    AGENT_VIEW_WAKE_PENDING.store(false, Ordering::Release);
}

/// Main application state
///
/// IMPORTANT: Drop order matters! The renderer's wgpu Surface holds a reference
/// to the window, so the renderer MUST be dropped before the window.
/// We implement Drop explicitly to guarantee this ordering.
pub struct App {
    /// Window-local startup controls parsed before the event loop starts.
    launch: crate::cli::WindowLaunch,
    /// The desktop's activation token for a window opened for a launch
    /// that joined this process, so it takes focus.
    activation_token: Option<String>,
    /// True when the process-level application owns event-loop exit policy.
    /// Set by the New Window command and drained by the process controller.
    request_new_window: bool,
    /// Set by the cross-window pane command and drained atomically by the
    /// process-level controller after the originating window event.
    request_move_pane_to_next_window: bool,
    /// Set by the cross-window tab command and drained atomically by the
    /// process-level controller after the originating window event.
    request_move_tab_to_next_window: bool,
    /// A dragged tab released outside this window, for the process to place
    /// in the window the pointer enters next, or in a new one.
    tab_dropped_outside: Option<usize>,
    /// What the pointer shows for a dragged tab outside this window.
    tab_drag_pointer: tab_drag::TabDragPointer,
    /// Explicit launch requested by an in-app workflow such as domain attach.
    requested_window_launch: Option<crate::cli::WindowLaunch>,
    /// Only the sole default startup window may consume automatic restore.
    allow_auto_restore: bool,
    /// Auto-expanded workspace windows fall back to a shell on recovery
    /// failure; an explicit CLI --workspace remains a hard startup error.
    allow_workspace_fallback: bool,
    /// False when one window is intentionally closed while siblings remain.
    retain_workspace_on_exit: bool,
    /// The process's `--renderer` choice, which wins over
    /// `renderer.backend` without ever entering the configuration.
    renderer_backend_override: Option<crate::config::RendererBackend>,
    /// Archive identity owned by this native window, independent of global
    /// most-recent state used for startup compatibility.
    current_workspace_id: Option<String>,
    /// Headless integration controllers never perform implicit disk writes.
    automatic_persistence: bool,
    /// Application configuration
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
    last_background_image_stamp: Option<crate::renderer::BackgroundImageStamp>,
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
    /// Private control launches return detailed startup diagnostics only to
    /// their authenticated caller; submitted program/cwd data must not enter
    /// process logs through the shared native startup path.
    redact_startup_error_log: bool,
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
    /// Absolute-clock cursor transition, independent of redraw cadence.
    cursor_motion: crate::ui::cursor_motion::CursorMotion,
    /// Last rendered frame time for presentation-deadline scheduling.
    last_frame_time: Instant,
    /// Last time we polled the config file for hot-reload
    last_config_check: Instant,
    /// Last time terminals were re-examined for a foreground coding agent.
    last_agent_scan: Instant,
    /// Enter keys waiting to submit text typed into agents.
    pending_agent_submits: Vec<agents::PendingAgentSubmit>,
    /// The configuration file this window loads, saves, and watches.
    config_path: PathBuf,
    /// Last observed mtime of the config file (for change detection)
    last_config_mtime: Option<std::time::SystemTime>,
    /// Coding agents with configuration applied; rebuilt on every config change.
    agents: crate::agents::AgentCatalog,
    /// The configuration last known to match the file on disk, as
    /// `config::save::config_table` gives it: Settings compares it to say
    /// when live changes have not been saved, and a save writes only what
    /// differs from it.
    saved_config: Option<toml::Table>,
    /// The notice about the configuration file's current problem: it stays
    /// until the file next loads.
    config_problem: crate::ui::toast::StandingNotice,
    /// The notice that the configured theme does not load, so another one is
    /// on screen: it stays until a configured theme loads.
    theme_problem: crate::ui::toast::StandingNotice,
    /// Which actions a `[[shortcuts]]` entry left without their default
    /// shortcut, until that changes.
    displaced_shortcuts: crate::ui::toast::StandingNotice,
    /// The file the configured theme is read from whenever it exists,
    /// polled so the theme follows edits to it and loads when it appears.
    theme_file: Option<std::path::PathBuf>,
    /// When `theme_file` was last modified as of the last load, or `None`
    /// while it does not exist.
    theme_file_modified: Option<std::time::SystemTime>,
    /// Last known cursor visibility (for snap detection)
    last_cursor_visible: bool,
    /// Highest tab ID we've seen (to detect truly new tabs)
    max_seen_tab_id: Option<usize>,
    /// Terminal the cursor was last drawn in (to detect pane focus changes)
    last_cursor_terminal: Option<crate::tabs::TerminalIdentity>,
    /// Mouse input state
    mouse_state: MouseState,
    /// Current keyboard modifiers
    modifiers: ModifiersState,
    /// Platform input-method state. Preedit remains local until commit.
    ime: ime::ImeState,
    /// Whether the layout's AltGraph modifier key is currently held.
    alt_graph_active: bool,
    /// The desktop clipboard and primary selection.
    clipboard: clipboard::Clipboard,
    /// Last cursor screen position (for mouse input events)
    last_cursor_pos: Option<(f64, f64)>,
    /// Native file drag is hovering this window. It owns the cursor until the
    /// compositor reports a drop or cancellation.
    file_drop_hovered: bool,
    /// Whether mouse is currently pressed (for selection)
    mouse_pressed: bool,
    mouse_selection_source: Option<mouse_selection::MouseSelectionPresentation>,
    /// Exact terminal that received the held application mouse button.
    terminal_mouse_capture: Option<TerminalMouseCapture>,
    /// Whether we're dragging the scrollbar
    scrollbar_dragging: Option<crate::tabs::TerminalIdentity>,
    /// Exact pane divider selected at mouse-down.
    divider_dragging: Option<DividerDrag>,
    /// Drag source for direct tab-bar reordering.
    tab_dragging: Option<TabDrag>,
    tab_slides: Vec<TabSlide>,
    tab_drag_scroll: Option<tab_drag::OverflowDwell>,
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
    /// This window's Ctrl+=/Ctrl+- font size, in points, until Ctrl+0 or a
    /// change to the configured `font.size`. View state: never saved.
    font_zoom: Option<f32>,
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
    search_presentation: Option<search_presentation::SearchPresentation>,
    /// Cursor position in search input
    search_cursor_pos: usize,
    /// Keyboard copy mode is bound to the pane that owned focus on entry.
    copy_mode: CopyMode,
    copy_mode_target: Option<crate::tabs::TerminalIdentity>,
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
    /// Last workspace-manager row click, for double-click opening.
    session_row_click: crate::input::DoubleClick<String>,
    /// Presses on tabs, by tab ID, for double-click renaming.
    tab_click: crate::input::DoubleClick<usize>,
    /// The theme in effect while "Change Theme…" previews others.
    theme_preview: Option<theme_picker::ThemePreview>,
    /// The toolbar Workspaces menu while it is open.
    workspace_menu:
        Option<crate::ui::popup_menu::PopupMenu<crate::ui::workspace_menu::WorkspaceMenuAction>>,
    /// Session management panel
    session_panel: crate::ui::session_panel::SessionPanel,
    /// Custom commands panel
    commands_panel: crate::ui::commands::panel::CommandsPanel,
    /// This window's agents and supervisor at a glance.
    agents_panel: crate::ui::agents_panel::AgentsPanel,
    /// The docked agent view and its sessions.
    agent_view: agent_view::AgentViewWindow,
    /// Buffer tracking what the user has typed on the current line (for alias expansion)
    input_line_buffer: String,
    /// Git repository watcher (tracks branch, dirty status per tab)
    git_watcher: crate::git::GitWatcher,
    /// Current git completion candidates
    git_completions: crate::git::BranchCompletions,
    /// Selected index in git completion dropdown (None = no selection)
    git_completion_index: Option<usize>,
    /// Whether git completion dropdown is visible
    git_completions_visible: bool,
    /// Which tab bar element is currently hovered (for icon animations)
    tab_bar_hover: Option<crate::renderer::TabBarClick>,
    /// Smooth hover animation progress (0.0 = not hovered, 1.0 = fully hovered)
    /// When the current tab bar hover target began, for its fade-in.
    tab_bar_hover_since: Instant,
    /// Plugin manager
    plugin_manager: PluginManager,
    /// The process's plugin handler host, shared with every other window.
    plugin_host: Arc<crate::plugin::structured::StructuredPluginHost>,
    /// Whether this window is open, as its queued handler requests see it.
    plugin_requests: crate::plugin::structured::WindowRequests,
    /// The host generation this window's plugins were built at; responses
    /// from any other generation are discarded.
    plugin_runtime_generation: u64,
    /// Exact plugin result request that must finish before this window closes.
    pending_plugin_window_close: Option<PendingPluginWindowClose>,
    /// The active tab plugins last heard about in a tab-switched event, or
    /// that a plugin's own action selected.
    reported_active_tab: Option<usize>,
    /// Command palette
    command_palette: CommandPalette,
    /// Searchable common-character and exact-code-point selector.
    unicode_picker: UnicodePicker,
    /// Exact focused terminal captured when the Unicode picker opened.
    pending_unicode_picker: Option<PendingUnicodePicker>,
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
    /// The multi-step flow the info overlay is running, if any.
    overlay_flow: Option<OverlayFlow>,
    /// Where third-party plugins and their registry live; tests point this
    /// at a temporary directory.
    runtime_plugin_paths: crate::plugin::runtime::PluginPaths,
    /// Agent status hooks awaiting the person's confirmation.
    pending_hook_change: Option<agent_hooks::PendingHookChange>,
    /// Git work comparing an agent's changes, running on a worker thread.
    agent_change_job: Option<agent_changes::ChangeJob>,
    /// The terminal whose changed files the palette lists.
    agent_change_list: Option<agent_changes::ChangeList>,
    /// A file revert awaiting the person's confirmation.
    pending_revert: Option<agent_changes::PendingRevert>,
    /// The agent change the diff viewer is showing.
    agent_change_review: Option<agent_changes::ChangeReview>,
    /// The agent overseeing this window's other agents, if started.
    supervisor: Option<supervisor::Supervisor>,
    /// The plain-language rules given to a new supervisor.
    supervisor_rules_path: PathBuf,
    /// This window's ID in the control protocol.
    control_window_id: u64,
    /// At most one background file upload may be active per native window.
    active_file_upload: Option<ActiveFileUpload>,
    /// One export owns its captured source and this window's completion route.
    session_export: Option<crate::session::export_job::ExportJob>,
    /// Consent and transaction state for bounded Kitty OSC 5113 downloads.
    kitty_file_transfers: kitty_file_transfer::KittyFileTransferManager,
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
        Self::from_config(config, None, false, Arc::default())
    }

    /// A headless app whose only tab runs `script` under `/bin/sh -c` in an
    /// 80×24 terminal: a probe for exactly what reaches a PTY.
    #[cfg(test)]
    pub(crate) fn new_headless_running(script: &str, title: &str) -> Self {
        let mut tabs = TabManager::new(
            80,
            24,
            crate::config::ShellConfig::default(),
            100,
            1024 * 1024,
        );
        tabs.new_tab_with_command(
            "/bin/sh",
            &["-c".into(), script.into()],
            None,
            Some(title),
            None,
            Default::default(),
        )
        .unwrap();
        let mut app = Self::new_headless(Config::default()).unwrap();
        app.tabs = Some(tabs);
        app
    }

    /// Process PTY output until the active terminal's screen shows
    /// `expected`; fails the test after three seconds with what it shows.
    #[cfg(test)]
    pub(crate) fn wait_for_screen_text(&mut self, expected: &str) {
        let mut output = String::new();
        let shown = crate::test_support::poll_until(Duration::from_secs(3), || {
            self.process_pty_events();
            output = self.active_terminal().map_or_else(String::new, |terminal| {
                terminal.rows_text(0, terminal.grid().rows() as i32 - 1)
            });
            output.contains(expected)
        });
        assert!(shown, "missing {expected}: {output}");
    }

    /// A headless app with its first tab running, and no workspace restored
    /// over it.
    #[cfg(test)]
    pub(crate) fn new_headless_with_tabs(config: Config) -> Self {
        let mut app = Self::new_headless(config).unwrap();
        app.allow_auto_restore = false;
        app.init_tabs().unwrap();
        app
    }

    /// The active tab's panes.
    #[cfg(test)]
    pub(crate) fn active_panes(&self) -> &PaneManager {
        &self
            .tabs
            .as_ref()
            .and_then(TabManager::active_tab)
            .expect("a tab is active")
            .panes
    }

    #[cfg(test)]
    pub(crate) fn active_panes_mut(&mut self) -> &mut PaneManager {
        &mut self
            .tabs
            .as_mut()
            .and_then(TabManager::active_tab_mut)
            .expect("a tab is active")
            .panes
    }

    fn from_config(
        config: Config,
        startup_config_warning: Option<String>,
        use_system_state: bool,
        plugin_host: Arc<crate::plugin::structured::StructuredPluginHost>,
    ) -> Result<Self> {
        let config_path = Config::config_path();
        let background_image_path = config.window.background_image.path.clone();
        let last_background_image_stamp = background_image_path.as_deref().and_then(|path| {
            crate::renderer::BackgroundImageStamp::for_path(
                &crate::renderer::resolve_background_image_path(path),
            )
        });
        let cursor_blink_period = (config.cursor.blink_rate as f32 / 1000.0) * 2.0;
        // A configured theme that does not load stays configured, so a save
        // never replaces it; turmeric is shown meanwhile, and applying the
        // configuration below says why.
        let theme = Theme::resolve(&config.theme.name, config.theme.path.as_deref())
            .unwrap_or_else(|_| Theme::default());
        let runtime_plugin_paths = crate::plugin::runtime::PluginPaths::configured();

        // Initialize bell from config
        let bell = Bell::new(config.bell.clone());

        // A clipboard that cannot be reached now is retried on the next
        // copy or paste.
        let clipboard = if use_system_state {
            clipboard::Clipboard::system()
        } else {
            clipboard::Clipboard::unavailable()
        };

        // Initialize unified shortcut registry with defaults + user overrides
        let mut shortcut_registry = ShortcutRegistry::new();
        shortcut_registry.apply_overrides(&config.shortcuts);

        // Initialize plugin/custom keybinding manager
        let mut keybinding_manager = KeyBindingManager::new();
        keybinding_manager.load(config.keybindings.clone());

        let result: Result<Self> = Ok(Self {
            launch: crate::cli::WindowLaunch::default(),
            activation_token: None,
            request_new_window: false,
            request_move_pane_to_next_window: false,
            request_move_tab_to_next_window: false,
            tab_dropped_outside: None,
            tab_drag_pointer: tab_drag::TabDragPointer::default(),
            requested_window_launch: None,
            allow_auto_restore: true,
            allow_workspace_fallback: false,
            retain_workspace_on_exit: true,
            renderer_backend_override: None,
            current_workspace_id: None,
            automatic_persistence: use_system_state,
            config,
            window: None,
            last_native_window_title: None,
            fullscreen_requested: false,
            quick_access_visible: true,
            renderer: None,
            renderer_generation: 0,
            background_image_generation: 0,
            background_image_loading: false,
            background_image_pending: false,
            background_image_path,
            last_background_image_stamp,
            renderer_recovery: renderer_recovery::RendererRecoveryPolicy::default(),
            accessibility_adapter: None,
            accessibility: accessibility::AccessibilityState::default(),
            system_accessibility: system_accessibility::SystemAccessibilityPreferences::default(),
            tabs: None,
            should_exit: false,
            startup_error: None,
            redact_startup_error_log: false,
            surface_lost: false,
            surface_lost_at: None,
            cols: 80,
            rows: 24,
            cursor_blink_start: Instant::now(),
            cursor_blink_period,
            last_cursor_blink_visible: true,
            text_blink_start: Instant::now(),
            last_text_blink_phases: TextBlinkPhases::default(),
            cursor_motion: crate::ui::cursor_motion::CursorMotion::default(),
            last_frame_time: Instant::now(),
            last_config_check: Instant::now(),
            last_agent_scan: Instant::now(),
            pending_agent_submits: Vec::new(),
            saved_config: None,
            config_problem: Default::default(),
            theme_problem: Default::default(),
            displaced_shortcuts: Default::default(),
            agents: crate::agents::AgentCatalog::default(),
            last_config_mtime: use_system_state
                .then(|| std::fs::metadata(&config_path))
                .and_then(Result::ok)
                .and_then(|metadata| metadata.modified().ok()),
            config_path,
            theme_file: None,
            theme_file_modified: None,
            last_cursor_visible: true,
            max_seen_tab_id: None,
            last_cursor_terminal: None,
            mouse_state: MouseState::new(),
            modifiers: ModifiersState::empty(),
            ime: ime::ImeState::default(),
            alt_graph_active: false,
            clipboard,
            last_cursor_pos: None,
            file_drop_hovered: false,
            mouse_pressed: false,
            mouse_selection_source: None,
            terminal_mouse_capture: None,
            scrollbar_dragging: None,
            divider_dragging: None,
            tab_dragging: None,
            tab_slides: Vec::new(),
            tab_drag_scroll: None,
            tab_rename: None,
            broadcast: BroadcastState::Inactive,
            pending_live_domain_controls: std::collections::HashSet::new(),
            live_domain_revisions: std::collections::HashMap::new(),
            disconnected_live_domains: std::collections::HashSet::new(),
            live_domain_topologies: std::collections::HashMap::new(),
            live_domain_topology_alive: Arc::new(AtomicBool::new(true)),
            live_domain_authority: None,
            live_domain_relay_command: None,
            font_zoom: None,
            copy_flash_start: None,
            window_focused: true,
            focus_change_time: Instant::now(),
            theme,
            settings_panel: SettingsPanel::new(),
            search: Search::new(),
            search_presentation: None,
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
            workspace_menu: None,
            session_row_click: Default::default(),
            tab_click: Default::default(),
            theme_preview: None,
            session_panel: crate::ui::session_panel::SessionPanel::new(),
            commands_panel: crate::ui::commands::panel::CommandsPanel::new(),
            agents_panel: crate::ui::agents_panel::AgentsPanel::new(),
            agent_view: agent_view::AgentViewWindow::default(),
            input_line_buffer: String::new(),
            git_watcher: crate::git::GitWatcher::new(git_event_waker()),
            git_completions: crate::git::BranchCompletions::default(),
            git_completion_index: None,
            git_completions_visible: false,
            tab_bar_hover: None,
            tab_bar_hover_since: Instant::now(),
            plugin_manager: PluginManager::new(),
            plugin_runtime_generation: plugin_host.generation(),
            plugin_host,
            plugin_requests: Default::default(),
            pending_plugin_window_close: None,
            reported_active_tab: None,
            command_palette: {
                let mut palette = CommandPalette::new();
                palette.set_show_category(false);
                palette
            },
            unicode_picker: UnicodePicker::new(),
            pending_unicode_picker: None,
            domain_picker: DomainPicker::new(),
            shortcuts_panel: ShortcutsPanel::new(),
            info_overlay: InfoOverlay::new(),
            toasts: crate::ui::toast::ToastManager::new(),
            diff_review: DiffReviewOverlay::new(),
            context_picker: ContextPicker::new(),
            guardrail: GuardrailOverlay::new(),

            overlay_flow: None,
            runtime_plugin_paths,
            pending_hook_change: None,
            agent_change_job: None,
            agent_change_list: None,
            pending_revert: None,
            agent_change_review: None,
            supervisor: None,
            supervisor_rules_path: Config::config_path().with_file_name("supervisor.md"),
            control_window_id: 1,
            active_file_upload: None,
            session_export: None,
            kitty_file_transfers: kitty_file_transfer::KittyFileTransferManager::default(),
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
        app.refresh_core_commands();
        app.apply_config_side_effects(false);
        app.sync_plugins_from_config();
        app.mark_config_saved();
        if let Some(warning) = startup_config_warning {
            app.report_config_problem(crate::ui::toast::ToastSeverity::Error, warning);
        }

        Ok(app)
    }

    /// Load window icon from embedded PNG data
    ///
    /// The icon is embedded directly in the binary at compile time,
    /// so it works on all platforms without requiring external files.
    /// Wayland compositors ignore it and resolve the icon from the desktop
    /// entry matching the app ID instead.
    fn load_icon() -> Option<Icon> {
        // The pre-rendered 256px theme icon avoids embedding the 1 MiB master
        // artwork and resampling it on every launch.
        let icon_bytes =
            include_bytes!("../../../assets/icons/hicolor/256x256/apps/org.termeric.Termeric.png");

        match image::load_from_memory(icon_bytes) {
            Ok(img) => {
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
        let window_key = self.window_key();
        let generation = self.renderer_generation;
        let failure_callback = EVENT_LOOP_PROXY.get().cloned().map(|proxy| {
            Arc::new(move |failure| {
                let _ = proxy.send_event(AppEvent::RendererDeviceLost {
                    window: window_key,
                    generation,
                    failure,
                });
            }) as crate::renderer::RendererFailureCallback
        });
        let mut renderer = Renderer::new(
            window.clone(),
            self.effective_font_size(),
            &self.config.font.family,
            &self.config.font.fallback,
            &self.config.font.range_overrides,
            self.config.font.weight,
            self.config.font.bold_weight,
            self.config.font.line_height,
            self.config.window.padding as f32,
            scale_factor,
            &self.theme,
            self.high_contrast_enabled(),
            self.config.font.ligatures,
            &self.config.font.features,
            self.renderer_backend(),
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
                self.request_redraw(RedrawReason::SurfaceRecovery);
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
        let lifecycle_completions = self.notifications.drain_completions();
        let mut lifecycle_changed = false;
        for completion in lifecycle_completions {
            lifecycle_changed |= self.handle_notification_completion(completion);
        }
        // Process events and collect results
        let (had_output, events, quick_exits) = {
            let Some(tabs) = &mut self.tabs else {
                return lifecycle_changed;
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

        let (persistence_notices, user_notices, topology_changed, should_exit, active_changed) = {
            let Some(tabs) = &mut self.tabs else {
                return lifecycle_changed;
            };
            let auto_save = self.automatic_persistence
                && self.config.session.enabled
                && self.config.session.auto_save;
            let active_before = tabs.active_tab_id();
            let topology_changed =
                tabs.cleanup_closed(auto_save, self.config.session.save_scrollback);
            let active_changed = tabs.active_tab_id() != active_before;
            let should_exit = topology_changed && tabs.is_empty();
            let persistence_notices = tabs.drain_persistence_notices();
            let user_notices = tabs.drain_user_notices();
            (
                persistence_notices,
                user_notices,
                topology_changed,
                should_exit,
                active_changed,
            )
        };
        if topology_changed {
            self.reconcile_broadcast_state();
        }
        if active_changed && !should_exit {
            // Another tab now shows; everything describing the focused one
            // follows it.
            self.refresh_focused_terminal_surfaces();
        }
        let file_transfers_pruned = self.prune_stale_kitty_file_transfers();
        let mut changed = lifecycle_changed
            || had_output
            || had_terminal_events
            || !quick_exits.is_empty()
            || !persistence_notices.is_empty()
            || !user_notices.is_empty()
            || topology_changed
            || file_transfers_pruned
            || should_exit;

        self.show_persistence_notices(persistence_notices);
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
            self.request_redraw(RedrawReason::TerminalOutput);
            log::warn!("Plugin '{}' exited within 5 seconds of launch", plugin_id);
        }

        // Observe every terminal independently so inactive tabs and panes do
        // not lose or duplicate semantic command regions.
        let mut completed_hooks = Vec::new();
        let show_indicators = self.harvest.is_some() && self.config.harvest.show_indicators;
        if let Some(tabs) = &mut self.tabs {
            for tab in tabs.all_tabs_mut() {
                // Hooks name the pane only in a split tab.
                let split = tab.panes.pane_count() > 1;
                for pane in tab.panes.all_panes_mut() {
                    let regions = pane
                        .harvest
                        .observe(pane.terminal.prompt_tracker(), show_indicators);
                    for region in regions {
                        let (command, command_truncated) =
                            Self::extract_command_text(&pane.terminal, &region);
                        let (output, output_truncated) =
                            Self::extract_output_text(&pane.terminal, &region);
                        if let Some(harvest) = &mut self.harvest {
                            harvest.record_command(
                                crate::harvest::HarvestSource {
                                    tab_id: tab.id,
                                    pane_id: pane.id.0,
                                },
                                &region,
                                command.clone(),
                                command_truncated,
                                output.clone(),
                                output_truncated,
                            );
                        }
                        completed_hooks.push(crate::plugin::Hook::CommandExecuted {
                            region_id: region.id,
                            tab_id: tab.id,
                            pane_id: split.then_some(pane.id.0),
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

    fn handle_notification_completion(
        &mut self,
        completion: crate::notifications::NotificationCompletion,
    ) -> bool {
        match completion {
            crate::notifications::NotificationCompletion::Warning(error) => {
                self.toasts.warning(error);
                true
            }
            crate::notifications::NotificationCompletion::ProtocolReply { origin, data } => {
                if let Some(tabs) = &mut self.tabs
                    && let Some(route) = tabs.origin_for_pane(origin.tab_id, origin.pane_id)
                    && let Err(error) = tabs.write_to_origin(route, &data)
                {
                    log::debug!(
                        "Dropped notification lifecycle reply for stale origin {origin:?}: {error}"
                    );
                }
                false
            }
            crate::notifications::NotificationCompletion::Activation {
                origin,
                focus,
                protocol_reply,
            } => {
                let changed = focus && self.activate_notification_origin(origin);
                if let Some(data) = protocol_reply
                    && let Some(tabs) = &mut self.tabs
                    && let Some(route) = tabs.origin_for_pane(origin.tab_id, origin.pane_id)
                    && let Err(error) = tabs.write_to_origin(route, &data)
                {
                    log::debug!(
                        "Dropped notification activation reply for stale origin {origin:?}: {error}"
                    );
                }
                changed
            }
        }
    }

    /// Handle window resize
    fn handle_resize(&mut self, width: u32, height: u32) {
        // Resize renderer
        if let Some(renderer) = &mut self.renderer {
            renderer.resize(width, height);
        }
        // The dock's width depends on the window's; settle it first so the
        // terminals are sized once.
        self.apply_agent_dock_width();
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
        self.apply_agent_dock_width();
        self.sync_terminal_geometry(new_inner_size.width, new_inner_size.height);
    }

    /// Synchronize every PTY from the renderer's physical grid geometry. This
    /// runs even when rows/columns are unchanged so scale/font pixel changes
    /// reach the kernel; Tab and Pane deduplicate exact four-field repeats.
    fn sync_terminal_geometry(&mut self, width: u32, height: u32) {
        // Every geometry entry point (resize, scale, font/configuration, and
        // topology) invalidates native drag coordinates. Finalize the last
        // preview through the normal domain acknowledgment/rollback path
        // instead of silently dropping a live-domain divider change.
        self.finish_native_pointer_gesture();
        let Some(renderer) = &self.renderer else {
            return;
        };
        let area = renderer.terminal_area(width, height);
        let geometry = area.geometry;
        self.cols = geometry.cols;
        self.rows = geometry.rows;

        self.prepare_copy_mode_reflow();

        if let Some(tabs) = &mut self.tabs {
            tabs.resize(area);
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

    /// Fits only the active tab to the window, as when the agent view's dock
    /// differs between tabs. Every other geometry change resizes all tabs
    /// through `sync_terminal_geometry`.
    pub(super) fn sync_active_terminal_geometry(&mut self) {
        let (Some(window), Some(renderer)) = (&self.window, &self.renderer) else {
            return;
        };
        let size = window.inner_size();
        let area = renderer.terminal_area(size.width, size.height);
        if self
            .tabs
            .as_ref()
            .is_none_or(|tabs| tabs.active_has_area(area))
        {
            return;
        }
        // A geometry change invalidates native drag coordinates.
        self.finish_native_pointer_gesture();
        self.cols = area.geometry.cols;
        self.rows = area.geometry.rows;
        self.prepare_copy_mode_reflow();
        if let Some(tabs) = &mut self.tabs {
            tabs.resize_active(area);
        }
        self.finish_copy_mode_reflow();
    }

    fn pending_runtime_plugin_management(&self) -> Option<&PendingRuntimePluginManagement> {
        match &self.overlay_flow {
            Some(OverlayFlow::RuntimePlugins(pending)) => Some(pending),
            _ => None,
        }
    }

    fn pending_font_range_management(
        &self,
    ) -> Option<&font_range_editor::PendingFontRangeManagement> {
        match &self.overlay_flow {
            Some(OverlayFlow::FontRanges(pending)) => Some(pending),
            _ => None,
        }
    }

    fn pending_font_feature_management(
        &self,
    ) -> Option<&font_feature_editor::PendingFontFeatureManagement> {
        match &self.overlay_flow {
            Some(OverlayFlow::FontFeatures(pending)) => Some(pending),
            _ => None,
        }
    }

    fn pending_file_upload(&self) -> Option<&PendingFileUpload> {
        match &self.overlay_flow {
            Some(OverlayFlow::FileUpload(pending)) => Some(pending),
            _ => None,
        }
    }

    fn take_pending_file_upload(&mut self) -> Option<PendingFileUpload> {
        match self.overlay_flow.take() {
            Some(OverlayFlow::FileUpload(pending)) => Some(pending),
            other => {
                self.overlay_flow = other;
                None
            }
        }
    }

    pub(super) fn ui_metrics(&self) -> UiMetrics {
        self.renderer.as_ref().map_or(
            UiMetrics {
                width: 800.0,
                height: 600.0,
                tab_bar_height: 0.0,
                cell_width: 8.0,
                cell_height: 16.0,
            },
            |renderer| {
                let (width, height) = renderer.window_size();
                UiMetrics {
                    width: width as f32,
                    height: height as f32,
                    tab_bar_height: renderer.tab_bar_height(),
                    cell_width: renderer.cell_width(),
                    cell_height: renderer.cell_height(),
                }
            },
        )
    }

    /// Record why this window must be redrawn and ask for a frame.
    fn request_redraw(&mut self, reason: RedrawReason) {
        self.redraw_scheduler.request(reason);
        if let Some(window) = &self.window {
            window.request_redraw();
        }
    }

    /// Render the terminal
    fn render(&mut self) {
        self.finish_obscured_native_pointer_gesture();
        let reduced_motion = self.reduced_motion_enabled();
        self.sync_native_window_title();
        // Do not run animation/update paths that enqueue more redraws while a
        // lost surface is waiting for Winit's next real resize event.
        if self.surface_lost {
            return;
        }

        self.reconcile_mouse_selection_presentation();
        self.reconcile_copy_mode_presentation();
        self.reconcile_quick_select_presentation();
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
            self.request_redraw(RedrawReason::Search);
        }

        // Completing a filter scan may replace the displayed row projection.
        self.reconcile_mouse_selection_presentation();

        let tab_bar_hover_progress = self.tab_bar_hover_progress();

        // While the agent view has the keyboard, the terminal cursor rests
        // dim and steady, so typing clearly goes to the panel.
        let terminal_keyboard = !self.agent_view.state.has_keyboard();
        let resting_cursor = |opacity: f32| {
            if terminal_keyboard {
                opacity
            } else {
                AGENT_VIEW_RESTING_CURSOR
            }
        };
        let cursor_elapsed = self.cursor_blink_start.elapsed().as_secs_f32();
        let configured_cursor_style = self.config.cursor.style;
        let configured_cursor_blink = self.config.cursor.blink;
        let cursor_blink_period = self.cursor_blink_period;
        let copy_flash = self.copy_flash_intensity();
        let focus_dim = self.focus_dim();
        let text_blink = self.text_blink_phases(Instant::now());

        // The cursor glides toward the focused pane's presented cursor.
        let cursor_info = self
            .tabs
            .as_ref()
            .and_then(|tabs| tabs.active_tab())
            .and_then(|tab| {
                let cursor = tab.focused_terminal()?.presentation_grid().cursor();
                Some((
                    cursor.col,
                    cursor.row,
                    cursor.visible,
                    tab.focused_terminal_identity(),
                ))
            });

        // Update cursor animation if we have a target
        let cursor_display_pos = if let Some((col, row, visible, terminal)) = cursor_info {
            self.update_cursor_animation(col, row, visible, Some(terminal))
        } else {
            // No active tab - snap to origin and mark as not visible
            self.update_cursor_animation(0, 0, false, None)
        };
        let copy_mode_info = self.copy_mode_render_info();
        let copy_mode_selection = self.copy_mode.selection();
        let frozen_copy_selection = copy_mode_info.is_some()
            && self
                .active_terminal()
                .is_some_and(|term| term.synchronized_output_deadline().is_some());
        // The IME places its window at the panel's caret, so the scene comes
        // first.
        self.prepare_agent_view_frame();
        let ime_preedit_info = self.ime_preedit_render_info();
        let chrome_focus_target = self.chrome_focus_render_target();
        let broadcast_render = self.broadcast_render_targets();
        // Before the frame borrows the renderer: a picker preview may change
        // the theme it draws with.
        self.sync_theme_preview();
        if self.renderer.is_none() || self.active_terminal().is_none() {
            return;
        }
        let metrics = self.ui_metrics();
        let (chrome, animating) = self.build_chrome(&metrics, reduced_motion);
        let harvest_render_info = self.harvest_frame_info(&metrics, reduced_motion);

        let tab_drag = self.tab_drag_visual(Instant::now());
        let Some(renderer) = &mut self.renderer else {
            return;
        };
        renderer.set_tab_drag(tab_drag);
        renderer.set_tab_bar_keyboard_focus(chrome_focus_target);
        let Some(tabs) = &self.tabs else { return };
        let Some(tab) = tabs.active_tab() else {
            return;
        };
        let Some(terminal) = tab.focused_terminal() else {
            return;
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

        renderer.set_info_overlay(chrome.info_overlay);
        renderer.set_toasts(chrome.toasts);
        renderer.set_diff_review(chrome.diff_review);
        renderer.set_context_picker(chrome.context_picker);
        renderer.set_guardrail(chrome.guardrail);
        renderer.set_tab_bar_hover(self.tab_bar_hover, tab_bar_hover_progress);
        if !renderer.git_completion_fits() {
            self.git_completions_visible = false;
            self.git_completions.clear();
            self.git_completion_index = None;
        }
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
            update_visible_url_cache(
                &mut self.url_cache,
                grid,
                &self.url_detector,
                self.search.filtered_rows(grid.rows()),
            );
            log::trace!("URL cache metrics: {:?}", self.url_cache.metrics());
        }

        // The focused pane's URLs are `url_cache`, which hit tests read too;
        // every other visible pane keeps its own.
        let panes = &tab.panes;
        let pane_grids = panes.iter_panes_identified();
        let unfocused_terminals: std::collections::HashSet<usize> = pane_grids
            .iter()
            .filter(|(_, _, _, focused)| !focused)
            .map(|(_, pane_terminal, _, _)| *pane_terminal as *const _ as usize)
            .collect();
        self.pane_url_caches
            .retain(|terminal_id, _| unfocused_terminals.contains(terminal_id));
        for (_, pane_terminal, _, _) in pane_grids.iter().filter(|(_, _, _, focused)| !focused) {
            let grid = pane_terminal.presentation_grid();
            let cache = self
                .pane_url_caches
                .entry(*pane_terminal as *const _ as usize)
                .or_insert_with(|| UrlCache::new(grid.rows()));
            update_visible_url_cache(cache, grid, &self.url_detector, &[]);
        }

        // Every visible pane, the focused one with the animated cursor.
        let pane_render_data: Vec<PaneRenderData> = pane_grids
            .iter()
            .map(|(pane_id, term, rect, focused)| {
                let cpos = if *focused {
                    cursor_display_pos
                } else {
                    let c = term.presentation_grid().cursor();
                    (c.col as f32, c.row as f32)
                };
                let terminal_key = *term as *const _ as usize;
                let (pane_cursor_style, pane_cursor_blink) = resolve_cursor_presentation(
                    configured_cursor_style,
                    configured_cursor_blink,
                    term.presentation_cursor_style(),
                    term.presentation_cursor_blink_override(),
                );
                PaneRenderData {
                    terminal_key,
                    grid: term.presentation_grid(),
                    selection: if *focused && frozen_copy_selection {
                        copy_mode_selection.as_ref()
                    } else {
                        term.selection()
                    },
                    dynamic_colors: term.presentation_dynamic_colors(),
                    extra_cursors: term.presentation_extra_cursors(),
                    rect: *rect,
                    is_focused: *focused,
                    broadcast_target: broadcast_render.as_ref().and_then(|(_, targets, state)| {
                        targets.contains(pane_id).then_some(*state)
                    }),
                    read_only: panes.get_pane(*pane_id).is_some_and(|pane| pane.read_only),
                    is_zoomed: panes.zoomed_id() == Some(*pane_id),
                    cursor_pos: cpos,
                    cursor_style: pane_cursor_style,
                    cursor_opacity: resting_cursor(cursor_opacity_at(
                        pane_cursor_blink,
                        cursor_elapsed,
                        cursor_blink_period,
                    )),
                    url_cache: if *focused {
                        &self.url_cache
                    } else {
                        &self.pane_url_caches[&terminal_key]
                    },
                    copy_mode: (*focused).then_some(copy_mode_info.as_ref()).flatten(),
                    images: term.presentation_images(),
                }
            })
            .collect();
        let pane_dividers = panes.get_dividers();

        let frame = crate::renderer::Frame {
            panes: &pane_render_data,
            lone_pane: panes.pane_count() == 1,
            pane_dividers: &pane_dividers,
            search: &self.search,
            text_blink,
            chrome: crate::renderer::ChromeFrame {
                tabs: &tab_info,
                copy_flash,
                focus_dim,
                settings_panel: chrome.settings.as_ref(),
                search_bar: chrome.search_bar.as_ref(),
                ime_preedit: ime_preedit_info.as_ref(),
                workspace_menu: chrome.workspace_menu.as_ref(),
                session_panel: chrome.session_panel.as_ref(),
                commands_panel: chrome.commands_panel.as_ref(),
                agents_panel: chrome.agents_panel.as_ref(),
                command_palette: chrome.palette.as_ref(),
                shortcuts_panel: chrome.shortcuts.as_ref(),
                harvest: harvest_render_info.as_ref(),
            },
        };
        let presented = match renderer.render(&frame) {
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
        self.keep_dragged_tab_picture(presented);
        self.refresh_accessibility();
        if animating {
            self.request_redraw(RedrawReason::Animation);
        }
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

    /// Mutate search using the focused terminal's displayed grid,
    /// without extending a whole-App borrow across the caller's input handling.
    fn with_search_and_active_grid<R>(
        &mut self,
        update: impl FnOnce(&mut Search, &termeric_core::Grid) -> R,
    ) -> Option<R> {
        if self.reconcile_search_presentation() {
            return None;
        }
        let search = &mut self.search;
        let terminal = self.tabs.as_ref()?.active_tab()?.focused_terminal()?;
        Some(update(search, terminal.presentation_grid()))
    }

    /// Whether any cursor currently painted in the active tab needs the
    /// shared blink clock. Extra cursors remain visible when the main cursor
    /// is hidden and can exist in unfocused panes, so focused-terminal state
    /// alone is insufficient here.
    fn cursor_blink_required(&self) -> bool {
        let configured_style = self.config.cursor.style;
        let configured_blink = self.config.cursor.blink;
        let requires = |terminal: &termeric_core::Terminal, main_cursor: bool| {
            let grid = terminal.presentation_grid();
            let (_, blinking) = resolve_cursor_presentation(
                configured_style,
                configured_blink,
                terminal.presentation_cursor_style(),
                terminal.presentation_cursor_blink_override(),
            );
            cursor_state_requires_blink(
                blinking,
                grid.scroll_offset() != 0,
                main_cursor && grid.cursor().visible,
                !terminal.presentation_extra_cursors().is_empty(),
            )
        };
        let Some(tab) = self.tabs.as_ref().and_then(TabManager::active_tab) else {
            return false;
        };
        let focused = tab.panes.focused_id();
        tab.panes
            .iter_panes()
            .any(|pane| requires(&pane.terminal, pane.id == focused))
    }

    fn cursor_blink_phase(&self, now: Instant) -> bool {
        if !self.cursor_blink_required() {
            return true;
        }
        let half_period = (self.cursor_blink_period / 2.0).max(0.001);
        let elapsed = now
            .saturating_duration_since(self.cursor_blink_start)
            .as_secs_f32();
        ((elapsed / half_period) as u64).is_multiple_of(2)
    }

    fn next_cursor_blink_deadline(&self, now: Instant) -> Option<Instant> {
        if !self.cursor_blink_required() || !self.window_focused || self.presentation_suspended() {
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
        // Only the presented cursor may sustain a transition. A synchronized
        // output snapshot can intentionally differ from the live grid.
        self.cursor_motion.is_active()
    }

    fn has_deadline_animation(&self) -> bool {
        !self.reduced_motion_enabled()
            && (self
                .tabs
                .as_ref()
                .is_some_and(TabManager::has_active_animation)
                || self.cursor_position_animation_active()
                || !self.tab_slides.is_empty()
                || self
                    .copy_flash_start
                    .is_some_and(|start| start.elapsed().as_secs_f32() < 0.15)
                || self.focus_change_time.elapsed().as_secs_f32() < 0.1
                || (self.tab_bar_hover.is_some() && self.tab_bar_hover_progress() < 1.0))
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
            for pane in tab.panes.all_panes_mut() {
                expired += usize::from(pane.terminal.expire_synchronized_output(now));
            }
        }
        expired
    }

    fn next_synchronized_output_deadline(&self) -> Option<Instant> {
        let tabs = self.tabs.as_ref()?;
        tabs.iter()
            .flat_map(|(_, tab)| {
                tab.panes
                    .iter_panes_identified()
                    .into_iter()
                    .filter_map(|(_, terminal, _, _)| terminal.synchronized_output_deadline())
            })
            .min()
    }

    fn advance_graphics_animations(&mut self, now: Instant) -> usize {
        let Some(tab) = self.tabs.as_mut().and_then(TabManager::active_tab_mut) else {
            return 0;
        };
        tab.panes
            .all_panes_mut()
            .map(|pane| usize::from(pane.terminal.advance_graphics_animations(now)))
            .sum()
    }

    fn next_graphics_animation_deadline(&self) -> Option<Instant> {
        let tab = self.tabs.as_ref()?.active_tab()?;
        tab.panes
            .iter_panes_identified()
            .into_iter()
            .filter_map(|(_, terminal, _, _)| terminal.graphics_animation_deadline())
            .min()
    }

    fn poll_idle_sources(&mut self, now: Instant) {
        self.poll_agent_view(now);
        if self.unicode_picker.poll_search() {
            self.redraw_scheduler
                .request(redraw_scheduler::RedrawReason::Search);
        }
        self.poll_tab_drag_scroll(now);
        self.poll_session_export();
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
        if self.refresh_agent_sessions() {
            self.redraw_scheduler
                .request(redraw_scheduler::RedrawReason::TerminalOutput);
        }
        self.submit_due_agent_input(Instant::now());
        if self.retire_closed_supervisor()
            | self.settle_orphaned_supervisor_approval()
            | self.deliver_supervisor_messages()
            | self.agents_panel.take_refresh_due(Instant::now())
        {
            self.redraw_scheduler
                .request(redraw_scheduler::RedrawReason::WindowEvent);
        }
        if self.poll_agent_change_job() {
            self.redraw_scheduler
                .request(redraw_scheduler::RedrawReason::WindowEvent);
        }
        if self.acknowledge_focused_agent() {
            self.redraw_scheduler
                .request(redraw_scheduler::RedrawReason::WindowEvent);
        }

        let working_dir = self
            .tabs
            .as_ref()
            .and_then(TabManager::active_tab)
            .and_then(|tab| tab.working_directory());
        if self.git_watcher.update(working_dir.as_deref()) {
            if self.git_watcher.completions_limited() {
                self.dismiss_git_completion_popup();
            }
            self.redraw_scheduler
                .request(redraw_scheduler::RedrawReason::GitStatus);
        }
        if let Some(error) = self.git_watcher.take_error() {
            self.toasts.warning(format!("Git inspection: {error}"));
            self.redraw_scheduler
                .request(redraw_scheduler::RedrawReason::GitStatus);
        }

        if !self.presentation_suspended() {
            let blink_visible = self.cursor_blink_phase(now);
            if blink_visible != self.last_cursor_blink_visible
                && self.window_focused
                && self.cursor_blink_required()
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
        deadlines.extend(self.next_agent_scan_deadline());
        deadlines.extend(self.next_agent_submit_deadline());
        deadlines.extend(self.agents_panel.refresh_deadline());
        deadlines.extend(self.next_agent_change_deadline(now));
        deadlines.extend(self.next_agent_view_deadline(now));
        if self.unicode_picker.search_pending() {
            deadlines.push(now + Duration::from_millis(16));
        }
        if !self.presentation_suspended() {
            if let Some(dwell) = &self.tab_drag_scroll {
                deadlines.push(dwell.deadline);
            }
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
    /// - Focus moves to another pane of the same tab (its cells are another grid's)
    ///
    /// Animates smoothly for:
    /// - Switching between existing tabs
    /// - All cursor movement including large jumps (e.g., after `cd`, `clear`)
    fn update_cursor_animation(
        &mut self,
        target_col: usize,
        target_row: usize,
        cursor_visible: bool,
        focused: Option<crate::tabs::TerminalIdentity>,
    ) -> (f32, f32) {
        self.update_cursor_animation_at(
            target_col,
            target_row,
            cursor_visible,
            focused,
            Instant::now(),
        )
    }

    fn update_cursor_animation_at(
        &mut self,
        target_col: usize,
        target_row: usize,
        cursor_visible: bool,
        focused: Option<crate::tabs::TerminalIdentity>,
        now: Instant,
    ) -> (f32, f32) {
        self.last_frame_time = now;

        // Check for snap conditions:
        // - Visibility change: cursor was hidden and reappears (e.g., command finished)
        // - New tab: tab ID is higher than any we've EVER seen (truly new tab)
        // We explicitly DON'T snap on tab switches - animate between existing tabs!
        // We explicitly DON'T snap on large jumps - that's the animation we want!
        let visibility_changed = cursor_visible != self.last_cursor_visible;

        // Detect truly new tab: compare against the highest ID we've ever seen,
        // not just the last active tab (which could be lower when switching tabs)
        let active_tab_id = focused.map(|terminal| terminal.tab_id);
        let is_new_tab = match (active_tab_id, self.max_seen_tab_id) {
            (Some(current), Some(max)) => current > max,
            (Some(_), None) => true, // First tab ever
            _ => false,
        };

        // Cursor positions are cells of the focused pane's own grid, so a
        // glide from another pane of the same tab would start from a cell of
        // the wrong grid.
        let pane_changed = matches!(
            (self.last_cursor_terminal, focused),
            (Some(last), Some(current))
                if last.tab_id == current.tab_id && last.pane_id != current.pane_id
        );

        // Update tracking state
        self.last_cursor_visible = cursor_visible;
        self.last_cursor_terminal = focused;
        // Update max seen tab ID
        if let Some(current) = active_tab_id {
            self.max_seen_tab_id =
                Some(self.max_seen_tab_id.map_or(current, |max| max.max(current)));
        }

        // Hidden movement has no pixels to animate. New tabs, visibility
        // transitions, and reduced motion retain their immediate placement.
        let snap = self.reduced_motion_enabled()
            || !cursor_visible
            || visibility_changed
            || is_new_tab
            || pane_changed;
        self.cursor_motion
            .update((target_col as f32, target_row as f32), now, snap)
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

    /// Scroll the focused terminal to the nearest earlier or later command
    /// that exited non-zero, as recorded by shell integration.
    fn jump_to_failed_command(&mut self, previous: bool) {
        let integrated = self
            .tabs
            .as_ref()
            .and_then(TabManager::active_tab)
            .and_then(|tab| tab.focused_pane())
            .is_some_and(crate::panes::Pane::shell_integration_active);
        let Some(terminal) = self.active_terminal_mut() else {
            return;
        };
        let found = if previous {
            terminal.previous_failed_command()
        } else {
            terminal.next_failed_command()
        };
        if found.is_some() {
            self.request_redraw(RedrawReason::Action);
        } else if !integrated {
            self.toasts.info(format!(
                "Failed-command jumps need shell integration ({})",
                crate::terminal::shell_integration::SUPPORTED_SHELLS
            ));
        } else if previous {
            self.toasts.info("No earlier failed command");
        } else {
            self.toasts.info("No later failed command");
        }
    }

    /// Handle an action from a key binding or the command palette.
    /// Returns true if the action was handled and the key should be consumed.
    fn handle_action(&mut self, action: Action) -> bool {
        match action {
            Action::Copy => {
                // With nothing selected the keystroke passes through (e.g.
                // Ctrl+Shift+C reaches the PTY). A failed copy keeps it: the
                // program must not receive ^C for a copy the person asked for.
                if self.copy_selection() == CopyOutcome::NothingSelected {
                    return false;
                }
            }
            Action::Paste => {
                self.paste_clipboard();
            }
            Action::ScrollPageUp => {
                if let Some(terminal) = self.active_terminal_mut() {
                    let grid = terminal.grid_mut();
                    let offset = grid.scroll_offset() + grid.rows();
                    grid.set_scroll_offset(offset);
                }
            }
            Action::ScrollPageDown => {
                if let Some(terminal) = self.active_terminal_mut() {
                    let grid = terminal.grid_mut();
                    let offset = grid.scroll_offset().saturating_sub(grid.rows());
                    grid.set_scroll_offset(offset);
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
                if self.mouse_state.current_selection().is_none()
                    && self
                        .active_terminal()
                        .and_then(termeric_core::Terminal::selection)
                        .is_none()
                {
                    return false;
                }
                self.mouse_state.clear_selection();
                if let Some(terminal) = self.active_terminal_mut() {
                    terminal.set_selection(None);
                }
            }
            Action::IncreaseFontSize => {
                self.zoom_font(2.0);
            }
            Action::DecreaseFontSize => {
                self.zoom_font(-2.0);
            }
            Action::ResetFontSize => {
                self.reset_font_zoom();
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
                }
            }
            Action::ToggleSearch => {
                if self.search.is_active() {
                    self.search.cancel();
                } else {
                    self.search.start();
                    self.search_cursor_pos = 0;
                }
                self.request_redraw(RedrawReason::Action);
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
            Action::OpenUnicodePicker => {
                self.open_unicode_picker();
            }
            Action::ToggleSettings => {
                self.toggle_settings_surface();
                self.request_redraw(RedrawReason::Action);
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
            Action::NewAgent | Action::ToggleAgentView if !self.agent_features_enabled() => {
                self.report_agent_features_off();
            }
            Action::NewAgent => {
                self.open_agent_picker(crate::ui::command_palette::palette::AgentPlacement::NewTab);
                self.request_redraw(RedrawReason::Action);
            }
            Action::ToggleAgentView => self.toggle_agent_view(),
            Action::PreviousFailedCommand | Action::NextFailedCommand => {
                self.jump_to_failed_command(matches!(action, Action::PreviousFailedCommand));
            }
            Action::PluginCommand(command_id) => {
                self.execute_plugin_command(&command_id);
            }
            Action::ToggleCommandPalette => {
                if self.command_palette.is_active() {
                    self.command_palette.close();
                } else {
                    self.open_command_palette();
                }
                self.request_redraw(RedrawReason::Action);
            }
            Action::SplitVertical => {
                self.split_pane(SplitDirection::Vertical);
            }
            Action::SplitHorizontal => {
                self.split_pane(SplitDirection::Horizontal);
            }
            Action::NavigatePane(direction) => {
                self.navigate_focused_pane(direction);
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
            Action::LayoutTall => {
                self.request_active_tab_layout(crate::panes::PaneLayoutPreset::Tall)
            }
            Action::LayoutFat => {
                self.request_active_tab_layout(crate::panes::PaneLayoutPreset::Fat)
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
            Action::NewWindow => self.request_new_window = true,
            Action::AttachDomain => self.open_domain_picker(),
            Action::CancelFileUpload => self.cancel_file_upload(),
            Action::ToggleShortcutsPanel => {
                self.shortcuts_panel.toggle();
                self.request_redraw(RedrawReason::Action);
            }
            Action::SaveConfig => self.save_config_with_toast(),
            Action::ReloadConfig => self.reload_config_from_disk(),
            Action::ResetConfig => self.confirm_config_reset(),
            Action::ToggleWorkspaceManager => self.toggle_workspace_manager(),
            Action::SaveWorkspace => self.force_save_workspace(workspaces::WorkspaceSave::Update),
            Action::SaveWorkspaceAs => self.force_save_workspace(workspaces::WorkspaceSave::New),
            Action::ConfigurePlugins => {
                self.settings_panel
                    .open_to_section(crate::ui::settings::SettingsSection::Plugins);
                self.request_redraw(RedrawReason::Action);
            }
            Action::ShowAbout => {
                self.info_overlay.show_about();
            }
            Action::ToggleHarvestCellar => {
                if let Some(harvest) = &mut self.harvest {
                    harvest.cellar.toggle_expanded();
                }
            }
            Action::ShowSproutMenu => self.show_sprout_menu_for_latest_command(),
            Action::HideSproutMenu => {
                if let Some(harvest) = &mut self.harvest {
                    harvest.hide_sprout_menu();
                }
            }
            Action::ClearHarvest => {
                if let Some(harvest) = &mut self.harvest {
                    harvest.cellar.clear();
                }
            }
            Action::ToggleTranscript => self.toggle_transcript(),
            Action::ChangeTheme => self.open_theme_picker(),
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
        self.agents_panel.close();
        self.session_panel.close();
        self.close_workspace_menu();
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
        }
        self.request_redraw(RedrawReason::Action);
    }

    /// Handle the result of executing a command from the palette
    fn handle_palette_result(&mut self, result: PaletteResult) {
        use crate::ui::command_palette::palette::PickKind;
        match result {
            PaletteResult::Picked {
                kind: PickKind::Theme,
                id,
            } => self.commit_theme(&id),
            PaletteResult::Picked {
                kind: PickKind::Agent(placement),
                id,
            } => self.start_picked_agent(placement, &id),
            PaletteResult::Picked {
                kind: PickKind::AgentTerminal,
                id,
            } => self.focus_agent_terminal(&id),
            PaletteResult::Picked {
                kind: PickKind::AgentChange,
                id,
            } => self.pick_agent_change(&id),
            PaletteResult::Picked {
                kind: PickKind::SupervisorMode,
                id,
            } => self.pick_supervisor_mode(&id),
            PaletteResult::Action(action) => {
                self.handle_action(action);
            }
            PaletteResult::RunCommand(id) => self.run_palette_command(&id),
            PaletteResult::RunPluginCommand(id) => {
                self.execute_plugin_command(&id);
            }
            PaletteResult::Cancelled => {}
        }
        self.request_redraw(RedrawReason::Action);
    }

    /// Run a palette command the app resolves by ID: an agent command, a
    /// terminal profile, or a plugin pane.
    fn run_palette_command(&mut self, id: &str) {
        if self.run_agent_command(id) {
            return;
        }
        if let Some(value) = id.strip_prefix("plugin-pane.") {
            self.open_plugin_pane_command(value);
        } else if let Some(name) = id.strip_prefix("profile.open.") {
            self.open_terminal_profile(name);
        } else {
            log::warn!("Unknown palette command: {id}");
        }
    }

    /// Save the configuration and report where it went.
    fn save_config_with_toast(&mut self) {
        match self.save_config() {
            Ok(path) => self.toasts.success(format!("Configuration saved: {path}")),
            Err(error) => self.toasts.error(format!("{error:#}")),
        }
    }

    /// Replace the running configuration with the one on disk.
    fn reload_config_from_disk(&mut self) {
        match Config::load_from(&self.config_path) {
            Ok(config) => {
                if let Err(error) = self.apply_config(config) {
                    self.report_config_problem(
                        crate::ui::toast::ToastSeverity::Error,
                        format!("Reload failed; keeping the current settings: {error}"),
                    );
                } else {
                    self.mark_config_saved();
                    self.clear_config_problem();
                    log::info!("Configuration reloaded");
                    self.toasts.success("Configuration reloaded");
                }
            }
            Err(error) => self.report_config_problem(
                crate::ui::toast::ToastSeverity::Error,
                format!("Reload failed; keeping the current settings: {error:#}"),
            ),
        }
    }

    /// Replace the configuration file with one that sets nothing, keeping
    /// the old file beside it, and then use the defaults. Only the confirmed
    /// Reset Configuration does this.
    fn reset_config_to_defaults(&mut self) {
        // Asked before the file changes, so a refusal leaves it as it was.
        if let Err(refusal) = self.agent_features_change_refusal(&Config::default()) {
            self.toasts.error(format!("Reset refused: {refusal}"));
            return;
        }
        let backup =
            match crate::config::save::reset(&self.config_path, std::time::SystemTime::now()) {
                Ok(backup) => backup,
                Err(error) => {
                    self.toasts.error(format!("Reset failed: {error:#}"));
                    return;
                }
            };
        self.last_config_mtime = std::fs::metadata(&self.config_path)
            .and_then(|metadata| metadata.modified())
            .ok();
        if let Err(error) = self.apply_config(Config::default()) {
            self.toasts.error(format!(
                "The file was reset, but the defaults failed: {error}"
            ));
            return;
        }
        self.mark_config_saved();
        self.clear_config_problem();
        match backup {
            Some(backup) => self.toasts.success(format!(
                "Configuration reset; the old file is {}",
                backup.display()
            )),
            None => self.toasts.success("Configuration reset"),
        }
    }

    /// Show the workspace manager in place of any other side panel, or hide it.
    fn toggle_workspace_manager(&mut self) {
        if self.session_panel.is_open() {
            self.session_panel.close();
        } else {
            self.settings_panel.close();
            self.commands_panel.close();
            self.agents_panel.close();
            self.session_panel
                .open(crate::session::listing::SessionListing::workspaces());
        }
        self.request_redraw(RedrawReason::Action);
    }

    /// Show the sprout menu for the latest command Harvest recorded, so its
    /// actions target something the user can reason about.
    fn show_sprout_menu_for_latest_command(&mut self) {
        let source = self.active_harvest_source();
        let latest = self
            .harvest
            .as_ref()
            .zip(source)
            .and_then(|(harvest, source)| harvest.cellar.latest_for_source(source).cloned());
        let Some(entry) = latest else {
            let message = self.active_shell_integration_message();
            self.toasts.warning(message);
            return;
        };
        let availability = self.sprout_availability_for_entry(&entry);
        let anchor_row = entry.row.max(0) as usize;
        if let Some(ref mut harvest) = self.harvest {
            harvest.show_sprout_menu(anchor_row, 0, Some(entry), availability);
        }
    }

    /// Copy the selection to the clipboard. A failure is reported here, with
    /// its reason; the caller decides only what happens to the keystroke.
    fn copy_selection(&mut self) -> CopyOutcome {
        // Output may have invalidated a filtered projection since the last
        // frame. Never copy its old row coordinates as an ordinary selection.
        self.reconcile_mouse_selection_presentation();
        let Some(text) = (if self.copy_mode.is_active() {
            self.copy_mode_selection_text()
        } else {
            self.native_selection_text()
        }) else {
            return CopyOutcome::NothingSelected;
        };
        if text.is_empty() {
            return CopyOutcome::NothingSelected;
        }

        match self.clipboard.set_text(
            termeric_core::ClipboardRequest::Clipboard,
            &text,
            clipboard::ClipboardAccess::Explicit,
        ) {
            Ok(()) => {
                log::debug!("Copied {} bytes to clipboard", text.len());
                self.copy_flash_start = Some(Instant::now());
                CopyOutcome::Copied
            }
            Err(error) => {
                log::warn!("Copy failed: {error}");
                self.toasts.error(format!("Copy failed: {error}"));
                CopyOutcome::Failed
            }
        }
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

    fn handle_window_occlusion_change(&mut self, occluded: bool) {
        self.window_occluded = occluded;
        if occluded {
            // Occlusion/hiding need not deliver a focus change or button-up.
            // Finish previews through their normal authority/rollback path,
            // then require a new press when the surface becomes visible.
            self.terminal_mouse_capture = None;
            self.tab_bar_hover = None;
            self.finish_native_pointer_gesture();
        }
    }

    /// Apply one native window focus transition. Repeated notifications are
    /// ignored so DECSET 1004 applications receive exactly one report.
    fn handle_window_focus_change(&mut self, focused: bool) {
        if !focused {
            self.alt_graph_active = false;
            self.terminal_mouse_capture = None;
            self.finish_native_pointer_gesture();
            self.cancel_agent_view_gestures();
        }
        if self.window_focused == focused {
            return;
        }
        // Terminals track the keyboard: while the agent view holds it, a
        // window focus change is no focus change for them.
        let terminal_had_focus = self.terminal_has_focus();
        self.window_focused = focused;
        if self.terminal_has_focus() != terminal_had_focus
            && let Some(report) = crate::input::encode_focus_report(
                self.active_keyboard_mode(),
                self.terminal_has_focus(),
            )
        {
            self.write_terminal_protocol_input(report);
        }
        self.focus_change_time = Instant::now();
        if focused {
            self.reset_cursor_blink();
        }
        log::debug!("Window focus changed: {}", focused);
    }

    /// Paste from clipboard
    fn paste_clipboard(&mut self) {
        if self.clipboard_paste_blocked() {
            return;
        }
        let text = match self.clipboard.text(
            termeric_core::ClipboardRequest::Clipboard,
            clipboard::ClipboardAccess::Explicit,
        ) {
            Ok(text) => text,
            Err(error) => {
                log::warn!("Paste failed: {error}");
                self.toasts.error(format!("Paste failed: {error}"));
                return;
            }
        };
        self.paste_clipboard_payload(&text);
    }

    /// A middle click pastes the primary selection into the pane it just
    /// focused, through the same checks and framing as a keyboard paste.
    fn paste_primary_selection(&mut self) {
        // A field that holds the keyboard does not take a paste aimed at the
        // terminal beneath the pointer, nor does that terminal (the find box
        // and copy mode are among `file_drop_input_blocked`'s surfaces).
        if self.tab_rename.is_some()
            || self.agent_view_has_keyboard()
            || self.file_drop_input_blocked()
        {
            return;
        }
        if let Some(text) = self.primary_selection_text()
            && !text.is_empty()
        {
            self.write_paste_text(&text);
        }
    }

    /// The primary selection's text for a middle-click paste, or `None`
    /// once any failure other than an empty selection has been reported.
    fn primary_selection_text(&mut self) -> Option<String> {
        match self.clipboard.text(
            termeric_core::ClipboardRequest::Primary,
            clipboard::ClipboardAccess::Explicit,
        ) {
            Ok(text) => Some(text),
            // Usually nothing is selected anywhere, which pastes nothing.
            Err(error @ clipboard::ClipboardError::Failed(_)) => {
                log::debug!("Primary selection not pasted: {error}");
                None
            }
            Err(error) => {
                log::warn!("Paste failed: {error}");
                self.toasts.error(format!("Paste failed: {error}"));
                None
            }
        }
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
            accepted = self.write_terminal_protocol_input(&framed);
            self.input_line_buffer.clear();
            if accepted {
                // A paste is an answer, whatever it starts with.
                self.note_person_typed(b"");
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

    /// Scroll the terminal view to show the current search match
    fn scroll_to_current_match(&mut self) {
        if self.reconcile_search_presentation() {
            return;
        }
        if self.copy_mode.is_active() {
            self.jump_copy_cursor_to_search();
            return;
        }
        let Some(search_match) = self.search.current() else {
            return;
        };
        let match_row = search_match.row;

        let Some(grid) = self
            .active_terminal_mut()
            .map(termeric_core::Terminal::presentation_grid_mut)
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
        self.write_saved_command(command);
    }

    /// Re-sync the command palette with currently registered plugins.
    ///
    /// Called whenever the registered plugins are rebuilt from configuration.
    fn refresh_plugin_commands(&mut self) {
        use crate::ui::command_palette::palette::{CommandSource, RegisteredCommand};

        let plugin_commands = self.plugin_manager.all_commands();
        for pc in &plugin_commands {
            log::debug!("Refreshing plugin command: {} - {}", pc.id, pc.description);
        }

        let palette_cmds: Vec<RegisteredCommand> = plugin_commands
            .iter()
            .map(|pc| RegisteredCommand {
                id: pc.id.clone(),
                title: pc.name.clone(),
                category: pc.category.clone().unwrap_or_else(|| "Plugins".to_string()),
                shortcut: None,
                source: CommandSource::Plugin,
                destructive: false,
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
        use crate::safe_text::truncate_with_ellipsis;
        use crate::ui::command_palette::palette::{CommandSource, RegisteredCommand};

        let mut commands: Vec<RegisteredCommand> = self
            .config
            .profiles
            .iter()
            .map(|(name, profile)| {
                let detail = profile
                    .description
                    .as_deref()
                    .map(|description| truncate_with_ellipsis(description, 72));
                RegisteredCommand {
                    id: format!("profile.open.{name}"),
                    title: detail.map_or_else(
                        || format!("New Tab: {name}"),
                        |detail| format!("New Tab: {name} — {detail}"),
                    ),
                    category: "Terminal Profiles".into(),
                    shortcut: None,
                    source: CommandSource::LaunchProfiles,
                    destructive: false,
                }
            })
            .collect();
        commands.extend(self.plugin_pane_commands());
        self.command_palette
            .replace_launch_profile_commands(commands);
    }

    /// The renderer this window asks for: the process's `--renderer` choice,
    /// else the configured one.
    fn renderer_backend(&self) -> crate::config::RendererBackend {
        self.renderer_backend_override
            .unwrap_or(self.config.renderer.backend)
    }

    /// Show `message`, a problem with the configuration file, until the file
    /// next loads, in place of any earlier one.
    fn report_config_problem(
        &mut self,
        severity: crate::ui::toast::ToastSeverity,
        message: String,
    ) {
        log::error!("{message}");
        self.config_problem
            .show(&mut self.toasts, severity, message);
    }

    /// The configuration file loaded: its problem notice is out of date.
    fn clear_config_problem(&mut self) {
        self.config_problem.clear(&mut self.toasts);
    }

    /// The live configuration as `config::save::config_table` gives it.
    fn persisted_config_table(&self) -> Result<toml::Table> {
        crate::config::save::config_table(&self.config)
    }

    /// Record the live configuration as matching the file, after it was
    /// loaded or reloaded from disk.
    fn mark_config_saved(&mut self) {
        self.saved_config = match self.persisted_config_table() {
            Ok(table) => Some(table),
            Err(error) => {
                log::error!("Could not record the saved configuration: {error:#}");
                None
            }
        };
    }

    /// Whether live settings differ from what was last loaded or saved.
    fn config_has_unsaved_changes(&self) -> bool {
        self.saved_config.as_ref().is_some_and(|saved| {
            self.persisted_config_table()
                .is_ok_and(|live| live != *saved)
        })
    }

    /// Write the live settings that differ from the file's to the file,
    /// unless it does not load, and return its path.
    fn save_config(&mut self) -> Result<String> {
        self.config.validate()?;
        let live = self.persisted_config_table()?;
        let baseline = self.saved_config.clone().unwrap_or_default();
        let file = crate::config::save::save(&self.config_path, &baseline, &self.config)?;
        // When the file now holds exactly the live settings, the poller need
        // not reload this write. Otherwise another program changed the file
        // too, and the reload brings in its changes.
        if file == live {
            self.last_config_mtime = std::fs::metadata(&self.config_path)
                .and_then(|metadata| metadata.modified())
                .ok();
        }
        self.saved_config = Some(live);
        Ok(self.config_path.display().to_string())
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
    projected_rows: &[i32],
) {
    if projected_rows.is_empty() {
        cache.update_grid(grid, detector);
    } else {
        cache.update_projected_grid(grid, projected_rows, detector);
    }
}

/// The terminal cursor's opacity while the agent view has the keyboard.
const AGENT_VIEW_RESTING_CURSOR: f32 = 0.35;

fn cursor_opacity_at(blink: bool, elapsed_seconds: f32, period_seconds: f32) -> f32 {
    if !blink {
        return 1.0;
    }
    let half_period = (period_seconds / 2.0).max(0.001);
    if ((elapsed_seconds / half_period) as u64).is_multiple_of(2) {
        1.0
    } else {
        0.3
    }
}

fn cursor_state_requires_blink(
    blinking: bool,
    scrolled_back: bool,
    main_cursor_painted: bool,
    extra_cursors_painted: bool,
) -> bool {
    blinking && !scrolled_back && (main_cursor_painted || extra_cursors_painted)
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
    (elapsed.as_nanos() / half_period.as_nanos()).is_multiple_of(2)
}

fn next_blink_boundary(start: Instant, now: Instant, half_period: std::time::Duration) -> Instant {
    let elapsed = now.saturating_duration_since(start);
    let completed = elapsed.as_nanos() / half_period.as_nanos();
    start + half_period.mul_f64((completed + 1) as f64)
}

/// The agent terminal to send to: the focused one, else the only one open.
fn select_agent_target<T: Clone>(focused: Option<T>, open: &[T]) -> std::result::Result<T, String> {
    if let Some(focused) = focused {
        return Ok(focused);
    }
    match open {
        [only] => Ok(only.clone()),
        [] => Err("Start an agent before using Explain".into()),
        _ => Err("Several agents are running; focus the one to ask first".into()),
    }
}

fn command_keystrokes(command: &str) -> [Vec<u8>; 3] {
    [
        b"\x15".to_vec(),
        command.trim_end_matches(['\r', '\n']).as_bytes().to_vec(),
        b"\r".to_vec(),
    ]
}

/// `window_width` is the right edge of the terminal area, which the menu
/// stays inside.
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

#[cfg(test)]
mod helper_tests {
    use super::{
        blink_phase_visible, command_keystrokes, cursor_opacity_at, cursor_state_requires_blink,
        next_blink_boundary, resolve_cursor_presentation, select_agent_target,
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
    fn extra_cursors_blink_without_a_visible_or_focused_main_cursor() {
        assert!(cursor_state_requires_blink(true, false, false, true));
        assert!(cursor_state_requires_blink(true, false, true, false));
        assert!(!cursor_state_requires_blink(false, false, false, true));
        assert!(!cursor_state_requires_blink(true, true, true, true));
        assert!(!cursor_state_requires_blink(true, false, false, false));
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
        // Ps 0 returns to the configured cursor rather than xterm's
        // blinking block, so a program's terminfo Se restores it.
        terminal.process(b"\x1b[0 q");
        assert_eq!(terminal.cursor_style(), None);
        assert_eq!(
            resolve_cursor_presentation(
                CursorStyle::Beam,
                false,
                terminal.cursor_style(),
                terminal.cursor_blink_override(),
            ),
            (CursorStyle::Beam, false)
        );
        terminal.process(b"\x1b[1 q");
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
    fn agent_target_prefers_focus_then_the_only_open_agent() {
        let open = [1, 2];
        assert_eq!(select_agent_target(Some(2), &open).unwrap(), 2);
        assert_eq!(select_agent_target(None, &[7]).unwrap(), 7);
        assert!(select_agent_target(None, &open).is_err());
        assert!(select_agent_target::<u8>(None, &[]).is_err());
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
}
