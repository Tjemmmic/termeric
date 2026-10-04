//! A tab: one terminal it owns directly as a `Pane`, or panes split from
//! it, plus its title, lifecycle, and session save/restore.

use crate::config::ShellConfig;
use crate::panes::{Pane, PaneId, PaneOwner};
use crate::session::Session;
use crate::session::terminal_state::TerminalSession;
#[cfg(test)]
use crate::session::transcript::TranscriptRecorder;
use crate::terminal::geometry::TerminalGeometry;
use crate::terminal::policy::TerminalPolicy;
use crate::terminal::shell_integration::ShellIntegrationSupport;
use anyhow::Context;
use std::collections::HashMap;
#[cfg(test)]
use std::sync::mpsc::Receiver;
use std::time::Instant;
#[cfg(test)]
use termeric_core::pty::PtyEvent;
use termeric_core::pty::PtyHandle;
use termeric_core::{Terminal, TerminalEvent};

use super::title::{
    abbreviate_path, dir_name_from_path, get_foreground_process, get_process_cwd,
    terminal_reported_directory_path, terminal_working_directory,
};

#[derive(Debug, Clone)]
pub struct SessionSaveOutcome {
    pub id: String,
    pub scrollback_truncated: bool,
}

pub(crate) struct SessionCapture {
    pub(crate) working_directory: Option<String>,
    pub(crate) last_command: Option<String>,
    pub(crate) scrollback: Vec<String>,
    pub(crate) scope: crate::session::SessionCaptureScope,
}

/// A program a tab runs directly, without a shell or its prompt marks.
#[derive(Clone, Debug, Default)]
pub struct CommandLaunch {
    pub program: String,
    pub args: Vec<String>,
    pub working_dir: Option<String>,
    /// The tab title; the program name when absent.
    pub title: Option<String>,
    /// The tab's kind; a shell tab when absent.
    pub tab_type: Option<TabType>,
    pub env: HashMap<String, String>,
    pub environment_policy: termeric_core::pty::EnvironmentPolicy,
}

/// What kind of tab this is
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TabType {
    /// Regular shell terminal
    Shell,
    /// A reusable user-configured terminal launch profile.
    Profile { name: String },
    /// A Linux serial device opened by a reusable terminal profile.
    Serial { name: String },
    /// Plugin-spawned tab (e.g., Claude Code)
    Plugin { plugin_id: String },
}

/// Mapping from window-local pane identifiers to the daemon's stable pane
/// identifiers for one attached live-domain tab.
pub(crate) struct LiveDomainTab {
    pub(crate) domain_id: String,
    pub(crate) tab_id: u32,
    pub(crate) pane_ids: HashMap<PaneId, u32>,
}

/// Duration of tab slide-in animation in seconds
const TAB_SLIDE_DURATION: f32 = 0.15;

/// Duration of tab close/shrink animation in seconds
const TAB_CLOSE_DURATION: f32 = 0.12;

/// A single terminal tab
/// How a tab's agent view conversation shows in the tab bar.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct TabAgentView {
    /// Set while the agent view fills the tab.
    pub title: Option<String>,
    pub badge: Option<crate::agents::AgentBadge>,
}

pub struct Tab {
    /// Unique tab ID
    pub id: usize,
    /// Tab title (usually shell name or current command)
    pub title: String,
    /// Explicit user title. This outranks OSC/process/cwd discovery until cleared.
    pub(crate) user_title: Option<String>,
    /// Pinned tabs occupy the stable prefix of the visual tab order.
    pub pinned: bool,
    /// Explicit OSC 0/2 title. While present it takes precedence over process
    /// and working-directory title discovery.
    osc_title: Option<String>,
    /// The terminal the tab owns directly outside pane mode. Splitting moves
    /// it into the pane manager as its first pane; collapsing moves it back.
    single: Option<Pane>,
    geometry: TerminalGeometry,
    /// Whether the tab has unread output
    pub has_activity: bool,
    /// Whether the shell process has exited
    pub closed: bool,
    /// When this tab was created (for slide-in animation)
    pub created_at: Instant,
    /// When this tab started closing (for shrink animation), None if not closing
    pub closing_at: Option<Instant>,
    /// Session ID this tab belongs to (None = fresh unsaved tab)
    pub session_id: Option<String>,
    /// What kind of tab this is (shell, plugin, etc.)
    pub tab_type: TabType,
    /// Live domain backing this tab, when its PTYs are authenticated relay
    /// processes rather than locally owned terminal children.
    pub(crate) live_domain: Option<LiveDomainTab>,
    /// Pane manager for split panes (None = single terminal, no splits)
    pub pane_manager: Option<crate::panes::PaneManager>,
    /// Allocation history while the tab directly owns its surviving terminal.
    pane_allocation: crate::panes::PaneAllocationState,
    /// The tab's agent view conversation, as the tab bar shows it.
    pub(crate) agent_view: TabAgentView,
    /// Frozen shell/command profile inherited by future panes in this tab.
    split_shell: ShellConfig,
    user_notices: Vec<String>,
}

impl Tab {
    pub(crate) fn validate_single_pane_attachment(&self) -> anyhow::Result<()> {
        let Some(single) = &self.single else {
            anyhow::bail!("Destination tab does not own a terminal");
        };
        if self
            .pane_allocation
            .next_pane
            .max(single.id.0.saturating_add(1))
            .checked_add(1)
            .is_none()
        {
            anyhow::bail!("Destination tab exhausted its stable pane ID space");
        }
        if self.pane_allocation.next_divider.checked_add(1).is_none() {
            anyhow::bail!("Destination tab exhausted its stable divider ID space");
        }
        Ok(())
    }

    pub(crate) fn child_observer(&self) -> Option<termeric_core::pty::ChildObserver> {
        self.single
            .as_ref()
            .map(|single| single.pty.child_observer())
    }

    /// A tab around its first terminal, owned directly.
    fn with_single(
        id: usize,
        title: String,
        single: Pane,
        geometry: TerminalGeometry,
        tab_type: TabType,
        split_shell: ShellConfig,
    ) -> Self {
        Self {
            id,
            title,
            user_title: None,
            pinned: false,
            osc_title: None,
            single: Some(single),
            geometry,
            has_activity: false,
            closed: false,
            created_at: Instant::now(),
            closing_at: None,
            session_id: None,
            tab_type,
            live_domain: None,
            pane_manager: None,
            pane_allocation: Default::default(),
            agent_view: TabAgentView::default(),
            split_shell,
            user_notices: Vec::new(),
        }
    }

    pub(crate) fn new_with_geometry(
        id: usize,
        geometry: TerminalGeometry,
        shell: &ShellConfig,
        policy: &TerminalPolicy,
    ) -> anyhow::Result<Self> {
        Self::new_with_cwd_geometry(id, geometry, shell, policy, None, None)
    }

    /// Create a new tab running a specific command (not a shell).
    ///
    /// The program runs directly as the tab's process — no shell prompt or
    /// command echo. When the program exits, the tab closes.
    #[cfg(test)]
    pub fn new_with_command(
        id: usize,
        cols: usize,
        rows: usize,
        launch: &CommandLaunch,
        scrollback_lines: usize,
        scrollback_bytes: usize,
    ) -> anyhow::Result<Self> {
        Self::new_command(
            id,
            TerminalGeometry::from_cells(cols, rows, 1.0, 1.0),
            launch,
            &TerminalPolicy::with_scrollback(scrollback_lines, scrollback_bytes),
        )
    }

    pub(crate) fn new_command(
        id: usize,
        geometry: TerminalGeometry,
        launch: &CommandLaunch,
        policy: &TerminalPolicy,
    ) -> anyhow::Result<Self> {
        let single = Pane::new_command(PaneId(0), geometry, launch, policy)?;
        Ok(Self::with_single(
            id,
            launch
                .title
                .clone()
                .unwrap_or_else(|| launch.program.clone()),
            single,
            geometry,
            launch.tab_type.clone().unwrap_or(TabType::Shell),
            ShellConfig::default(),
        ))
    }

    /// Poll the directly owned terminal, consuming its protocol replies on its
    /// own PTY. `on_output` sees each chunk of output it reads.
    pub(crate) fn poll_single(
        &mut self,
        on_output: impl FnMut(PaneId, &[u8]),
    ) -> (bool, Vec<TerminalEvent>) {
        debug_assert!(self.pane_manager.is_none());
        let Some(single) = &mut self.single else {
            return (false, Vec::new());
        };
        // A relay's daemon answers protocol queries at the real PTY.
        single.suppress_protocol_replies = self.live_domain.is_some();
        let (changed, events) = single.poll_events(on_output);
        let newly_closed = single.closed && !self.closed;
        if newly_closed {
            log::info!("Tab {} terminal exited", self.id);
            self.closed = true;
        }
        (changed || newly_closed, events)
    }

    /// The pane holding the focused terminal, in or out of pane mode.
    pub(crate) fn focused_pane(&self) -> Option<&Pane> {
        match &self.pane_manager {
            Some(panes) => panes.focused_pane(),
            None => self.single.as_ref(),
        }
    }

    pub(crate) fn focused_pane_mut(&mut self) -> Option<&mut Pane> {
        match &mut self.pane_manager {
            Some(panes) => panes.focused_pane_mut(),
            None => self.single.as_mut(),
        }
    }

    /// The pane with `id`, in or out of pane mode.
    fn pane(&self, id: PaneId) -> Option<&Pane> {
        match &self.pane_manager {
            Some(panes) => panes.get_pane(id),
            None => self.single.as_ref().filter(|single| single.id == id),
        }
    }

    fn pane_mut(&mut self, id: PaneId) -> Option<&mut Pane> {
        match &mut self.pane_manager {
            Some(panes) => panes.get_pane_mut(id),
            None => self.single.as_mut().filter(|single| single.id == id),
        }
    }

    /// Every pane the tab owns, in or out of pane mode.
    fn panes_mut(&mut self) -> Vec<&mut Pane> {
        match &mut self.pane_manager {
            Some(panes) => panes.all_panes_mut().collect(),
            None => self.single.iter_mut().collect(),
        }
    }

    /// Read the terminal currently owned by this tab's focused route.
    pub fn focused_terminal(&self) -> Option<&Terminal> {
        self.focused_pane().map(|pane| &pane.terminal)
    }

    /// Mutably access the focused live terminal.
    pub fn focused_terminal_mut(&mut self) -> Option<&mut Terminal> {
        self.focused_pane_mut().map(|pane| &mut pane.terminal)
    }

    /// Apply host settings to every terminal this tab owns and to those its
    /// future splits create. `apply_keyboard_type` is as in
    /// [`TerminalPolicy::apply`].
    pub(crate) fn apply_terminal_policy(
        &mut self,
        policy: &TerminalPolicy,
        apply_keyboard_type: bool,
    ) {
        let policy = if self.live_domain.is_some() {
            policy.for_relay()
        } else {
            *policy
        };
        if let Some(panes) = &mut self.pane_manager {
            panes.apply_terminal_policy(policy, apply_keyboard_type);
        } else if let Some(single) = &mut self.single {
            policy.apply(&mut single.terminal, apply_keyboard_type);
        }
    }

    pub(crate) fn set_graphics_relay_snapshot_allowed(&mut self, allowed: bool) {
        for pane in self.panes_mut() {
            pane.terminal.set_graphics_relay_snapshot_allowed(allowed);
        }
    }

    pub(crate) fn live_domain_focus_route(&self) -> Option<(&str, u32)> {
        let live = self.live_domain.as_ref()?;
        let local = self.focused_terminal_identity().pane_id;
        Some((live.domain_id.as_str(), *live.pane_ids.get(&local)?))
    }

    /// The directly owned terminal's pane ID, outside pane mode. Only a
    /// transition between modes leaves the tab without one; pane zero then
    /// stands in.
    pub(crate) fn single_pane_id(&self) -> PaneId {
        self.single.as_ref().map_or(PaneId(0), |single| single.id)
    }

    pub(crate) fn focused_terminal_identity(&self) -> super::TerminalIdentity {
        super::TerminalIdentity {
            tab_id: self.id,
            pane_id: match &self.pane_manager {
                Some(panes) => panes.focused_id(),
                None => self.single_pane_id(),
            },
        }
    }

    /// Plugin identity belongs to the live owner, not its tab's presentation.
    pub(crate) fn pane_plugin_id(&self, pane_id: PaneId) -> Option<&str> {
        self.pane_plugin_launch(pane_id).map(|(id, _)| id)
    }

    pub(crate) fn pane_plugin_launch(&self, pane_id: PaneId) -> Option<(&str, Option<&str>)> {
        if self.closed || self.is_closing() {
            return None;
        }
        match &self.pane(pane_id).filter(|pane| !pane.closed)?.launch {
            crate::workspace::WorkspaceLaunch::Plugin { plugin_id, profile } => {
                Some((plugin_id, profile.as_deref()))
            }
            _ => None,
        }
    }

    #[cfg(test)]
    pub(crate) fn plugin_pane_id(&self, plugin_id: &str) -> Option<PaneId> {
        let focused = self.focused_terminal_identity().pane_id;
        if self.pane_plugin_id(focused) == Some(plugin_id) {
            return Some(focused);
        }
        self.pane_manager
            .as_ref()?
            .pane_ids()
            .into_iter()
            .find(|pane_id| self.pane_plugin_id(*pane_id) == Some(plugin_id))
    }

    pub(crate) fn set_single_pane_id(&mut self, id: PaneId) {
        debug_assert!(self.pane_manager.is_none());
        if let Some(single) = &mut self.single {
            single.id = id;
        }
        self.pane_allocation.next_pane = self.pane_allocation.next_pane.max(id.0.saturating_add(1));
    }

    /// Restore the directly owned terminal's remembered command, as saved.
    pub(crate) fn set_single_last_command(&mut self, command: Option<String>) {
        if let Some(single) = &mut self.single {
            single.last_command = command;
        }
    }

    pub(crate) fn focused_pane_read_only(&self) -> bool {
        self.focused_pane().is_some_and(|pane| pane.read_only)
    }

    pub(crate) fn pane_read_only(&self, pane_id: PaneId) -> Option<bool> {
        self.pane(pane_id).map(|pane| pane.read_only)
    }

    /// Toggle the focused pane's transient user-input lock and return its new
    /// state. The lock follows a live pane when ownership is moved or collapsed.
    pub(crate) fn toggle_focused_pane_read_only(&mut self) -> bool {
        let pane = self
            .focused_pane_mut()
            .expect("focused pane remains present");
        pane.read_only = !pane.read_only;
        pane.read_only
    }

    pub(crate) fn live_domain_tab_route(&self) -> Option<(&str, u32)> {
        let live = self.live_domain.as_ref()?;
        Some((live.domain_id.as_str(), live.tab_id))
    }

    /// Observe semantic command regions for the single live terminal. Pane
    /// callers keep per-pane Harvest state beside each pane instead.
    pub fn observe_harvest(&mut self, show_indicators: bool) -> Vec<termeric_core::CommandRegion> {
        debug_assert!(self.pane_manager.is_none());
        let Some(single) = &mut self.single else {
            return Vec::new();
        };
        single
            .harvest
            .observe(single.terminal.prompt_tracker(), show_indicators)
    }

    /// Write user input and record it only on the receiving terminal.
    pub fn write(&mut self, data: &[u8]) -> std::io::Result<()> {
        self.focused_pane_for_write()?.write(data)
    }

    pub(crate) fn write_protocol_reply(&mut self, data: &[u8]) -> std::io::Result<()> {
        self.focused_pane_for_write()?.write_protocol_reply(data)
    }

    fn focused_pane_for_write(&mut self) -> std::io::Result<&mut Pane> {
        self.focused_pane_mut().ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "focused pane no longer exists",
            )
        })
    }

    /// Resize the terminal model and PTY from one canonical geometry.
    /// Whether the tab's own terminal already has `geometry`.
    pub(crate) fn has_geometry(&self, geometry: TerminalGeometry) -> bool {
        self.geometry == geometry
    }

    pub(crate) fn resize_geometry(&mut self, geometry: TerminalGeometry) -> anyhow::Result<()> {
        if self.geometry == geometry {
            return Ok(());
        }
        self.single
            .as_mut()
            .context("single terminal owner is detached")?
            .resize_geometry(geometry)?;
        self.geometry = geometry;
        Ok(())
    }

    /// Update tab title based on the shell's current working directory
    pub fn update_title(&mut self) {
        if let Some(title) = &self.user_title {
            self.title.clone_from(title);
            return;
        }
        if let Some(title) = &self.osc_title {
            self.title.clone_from(title);
            return;
        }
        if let Some(cwd) = self
            .focused_terminal()
            .and_then(terminal_reported_directory_path)
        {
            self.title = dir_name_from_path(cwd);
            return;
        }
        if let Some(cwd) = self
            .focused_pane()
            .and_then(|pane| pane.pty.child_id())
            .and_then(get_process_cwd)
        {
            self.title = dir_name_from_path(&cwd);
        }
    }

    pub(crate) fn set_osc_title(&mut self, title: String) {
        if title.is_empty() {
            self.osc_title = None;
            if self.user_title.is_none() {
                self.title = format!("Tab {}", self.id + 1);
            }
            self.update_title();
        } else {
            self.osc_title = Some(title);
            self.update_title();
        }
    }

    pub fn set_user_title(&mut self, title: Option<String>) {
        self.user_title = title.filter(|title| !title.trim().is_empty());
        if let Some(title) = &self.user_title {
            self.title.clone_from(title);
        } else {
            self.title = format!("Tab {}", self.id + 1);
            self.update_title();
        }
    }

    pub fn user_title(&self) -> Option<&str> {
        self.user_title.as_deref()
    }

    pub(crate) fn set_live_domain_metadata(
        &mut self,
        title: &str,
        user_title: Option<&str>,
        pinned: bool,
    ) {
        self.title = title.to_string();
        self.user_title = user_title.map(str::to_string);
        self.osc_title = None;
        self.pinned = pinned;
    }

    pub(crate) fn new_with_cwd_geometry(
        id: usize,
        geometry: TerminalGeometry,
        shell: &ShellConfig,
        policy: &TerminalPolicy,
        working_dir: Option<&str>,
        title: Option<&str>,
    ) -> anyhow::Result<Self> {
        let single = Pane::new_local(PaneId(0), geometry, shell, policy, working_dir, false)?;
        // Use the given title, else the working directory's name.
        let title = title
            .map(str::to_string)
            .or_else(|| working_dir.map(dir_name_from_path))
            .or_else(|| {
                single
                    .pty
                    .child_id()
                    .and_then(get_process_cwd)
                    .map(|cwd| dir_name_from_path(&cwd))
            })
            .unwrap_or_else(|| format!("Tab {}", id + 1));
        Ok(Self::with_single(
            id,
            title,
            single,
            geometry,
            TabType::Shell,
            shell.clone(),
        ))
    }

    /// Get the current working directory of this tab's shell process
    pub fn working_directory(&self) -> Option<String> {
        if matches!(self.tab_type, TabType::Serial { .. }) {
            return None;
        }
        let pane = self.focused_pane()?;
        terminal_working_directory(&pane.terminal)
            .or_else(|| pane.pty.child_id().and_then(get_process_cwd))
    }

    pub(crate) fn split_shell_config(&self) -> ShellConfig {
        self.split_shell.clone()
    }

    pub(crate) fn pane_working_directory(&self, pane_id: PaneId) -> Option<String> {
        if self.closed || self.is_closing() || matches!(self.tab_type, TabType::Serial { .. }) {
            return None;
        }
        let pane = self.pane(pane_id).filter(|pane| !pane.closed)?;
        terminal_working_directory(&pane.terminal)
            .or_else(|| pane.pty.child_id().and_then(get_process_cwd))
    }

    pub(crate) fn set_split_shell_config(&mut self, shell: ShellConfig) {
        if let Some(single) = &mut self.single
            && matches!(single.launch, crate::workspace::WorkspaceLaunch::Shell)
        {
            single.launch_shell = shell.clone();
        }
        self.split_shell = shell;
    }

    pub(crate) fn set_terminal_profile_launch(&mut self, name: &str, shell: ShellConfig) {
        if let Some(single) = &mut self.single {
            single.launch = crate::workspace::WorkspaceLaunch::Profile {
                name: name.to_string(),
            };
            single.launch_shell = shell.clone();
        }
        self.split_shell = shell;
    }

    pub(crate) fn set_serial_profile_launch(&mut self, name: &str, title: String) {
        self.tab_type = TabType::Serial {
            name: name.to_string(),
        };
        if let Some(single) = &mut self.single {
            single.launch = crate::workspace::WorkspaceLaunch::Profile {
                name: name.to_string(),
            };
            single.launch_shell = ShellConfig::default();
            single.last_command = None;
            single.shell_integration = ShellIntegrationSupport::Unavailable(
                "Serial terminals do not provide shell prompt marks".into(),
            );
        }
        self.split_shell = ShellConfig::default();
        self.set_user_title(Some(title));
    }

    pub(crate) fn allows_pane_splits(&self) -> bool {
        !matches!(self.tab_type, TabType::Serial { .. })
    }

    /// Change one owner's next-launch reference, never sibling launch metadata.
    /// None means automatic; every Some name (including "default") is explicit.
    pub(crate) fn set_plugin_profile_reference(
        &mut self,
        pane_id: PaneId,
        plugin_id: &str,
        profile: Option<String>,
    ) -> bool {
        if self.pane_plugin_id(pane_id) != Some(plugin_id) {
            return false;
        }
        let launch = &mut self
            .pane_mut(pane_id)
            .expect("validated plugin pane")
            .launch;
        if let crate::workspace::WorkspaceLaunch::Plugin { profile: saved, .. } = launch {
            *saved = profile;
        }
        true
    }

    /// Detach the single-terminal owner exactly once before entering pane mode.
    pub(crate) fn take_terminal_owner(&mut self) -> Option<PaneOwner> {
        if self.pane_manager.is_some() {
            return None;
        }
        let mut pane = self.single.take()?;
        pane.suppress_protocol_replies = self.live_domain.is_some();
        Some(PaneOwner {
            pane,
            allocation: self.pane_allocation,
        })
    }

    /// Restore the final pane as the tab's single live terminal owner.
    pub(crate) fn restore_terminal_owner(&mut self, owner: PaneOwner) {
        assert!(self.single.is_none());
        let PaneOwner { pane, allocation } = owner;
        self.pane_allocation = allocation;
        self.closed = pane.closed;
        self.geometry = pane.geometry;
        match &pane.launch {
            crate::workspace::WorkspaceLaunch::Shell => {
                self.tab_type = TabType::Shell;
                self.split_shell = pane.launch_shell.clone();
            }
            crate::workspace::WorkspaceLaunch::Profile { name } => {
                self.tab_type = TabType::Profile { name: name.clone() };
                self.split_shell = pane.launch_shell.clone();
            }
            crate::workspace::WorkspaceLaunch::Plugin { plugin_id, .. } => {
                self.tab_type = TabType::Plugin {
                    plugin_id: plugin_id.clone(),
                };
            }
        }
        self.single = Some(pane);
    }

    /// Keep tab-level presentation and future-split policy aligned with the
    /// current primary pane after detach/close operations. The pane remains
    /// the source of truth while pane mode is active.
    pub(crate) fn reconcile_primary_pane_launch(&mut self) {
        let Some(panes) = &self.pane_manager else {
            return;
        };
        let Some(primary) = panes.get_pane(panes.primary_id()) else {
            return;
        };
        let launch = primary.launch.clone();
        let shell = primary.launch_shell.clone();
        match launch {
            crate::workspace::WorkspaceLaunch::Shell => {
                self.tab_type = TabType::Shell;
                self.split_shell = shell;
            }
            crate::workspace::WorkspaceLaunch::Profile { name } => {
                self.tab_type = TabType::Profile { name };
                self.split_shell = shell;
            }
            crate::workspace::WorkspaceLaunch::Plugin { plugin_id, .. } => {
                self.tab_type = TabType::Plugin { plugin_id };
            }
        }
    }

    #[cfg(test)]
    pub(crate) fn replace_single_test_output(&mut self, receiver: Receiver<PtyEvent>) {
        assert!(self.pane_manager.is_none());
        self.single
            .as_mut()
            .expect("tab owns its terminal directly")
            .pty_rx = receiver;
    }

    #[cfg(test)]
    pub(crate) fn has_single_terminal_owner(&self) -> bool {
        self.single.is_some()
    }

    #[cfg(test)]
    pub(crate) fn single_child_id(&self) -> Option<u32> {
        self.single.as_ref()?.pty.child_id()
    }

    #[cfg(test)]
    pub(crate) fn single_pty_size(&self) -> Option<termeric_core::pty::PtySize> {
        self.single.as_ref()?.pty.size().ok()
    }

    pub(crate) fn pane_session(&self, id: PaneId) -> Option<&TerminalSession> {
        self.pane(id).map(|pane| &pane.integration)
    }

    pub(crate) fn pane_terminal(&self, id: PaneId) -> Option<&Terminal> {
        self.pane(id).map(|pane| &pane.terminal)
    }

    pub(crate) fn pane_session_mut(&mut self, id: PaneId) -> Option<&mut TerminalSession> {
        self.pane_mut(id).map(|pane| &mut pane.integration)
    }

    #[cfg(test)]
    pub(crate) fn focused_session(&self) -> Option<&TerminalSession> {
        self.pane_session(self.focused_terminal_identity().pane_id)
    }

    #[cfg(test)]
    pub(crate) fn focused_session_mut(&mut self) -> Option<&mut TerminalSession> {
        self.pane_session_mut(self.focused_terminal_identity().pane_id)
    }

    /// Re-identify the coding agent in each of this tab's terminals from its
    /// foreground process. A terminal Termeric launched as an agent keeps
    /// that identity even where the platform cannot report the foreground
    /// process. Returns whether any terminal changed.
    pub(crate) fn refresh_agents(&mut self, catalog: &crate::agents::AgentCatalog) -> bool {
        fn observe(
            catalog: &crate::agents::AgentCatalog,
            pty: Option<&PtyHandle>,
            launch: &crate::workspace::WorkspaceLaunch,
            session: &mut crate::session::terminal_state::TerminalSession,
        ) -> bool {
            let (launched_id, profile) = match launch {
                crate::workspace::WorkspaceLaunch::Plugin { plugin_id, profile } => (
                    catalog
                        .get(plugin_id)
                        .filter(|agent| agent.enabled)
                        .map(|agent| agent.id.as_str()),
                    profile,
                ),
                _ => (None, &None),
            };
            let foreground_group = pty.and_then(PtyHandle::foreground_process_group);
            let foreground = foreground_group
                .and_then(crate::agents::detect::foreground_argv)
                .and_then(|argv| catalog.identify(&argv).map(|agent| agent.id.clone()));
            // An agent that reports through hooks stays recognized, even when
            // its process name is unfamiliar (for example one started through
            // `npx`), only while the process group it reported from still
            // holds the foreground. Anything started after it, such as a
            // shell or `ssh`, is not the agent.
            let child = pty.and_then(PtyHandle::child_id).map(i64::from);
            // The terminal's own child (its shell) in front means no agent is,
            // whatever a program elsewhere that inherited this terminal's
            // identity reports.
            let child_in_front = child.is_some() && child == foreground_group.map(i64::from);
            let reported = session
                .agent_status
                .as_ref()
                .filter(|status| {
                    !child_in_front
                        && foreground_group.is_some()
                        && status.process_group == foreground_group
                })
                .map(|status| status.agent_id.clone());
            // The agent Termeric started, itself in front, is known exactly;
            // a guess from its program name could name another agent that
            // shares the program.
            let launched_in_front = launched_id.filter(|_| child_in_front);
            let observed = launched_in_front
                .map(|agent_id| (agent_id.to_owned(), child))
                .or_else(|| foreground.map(|agent_id| (agent_id, foreground_group.map(i64::from))))
                .or_else(|| reported.map(|agent_id| (agent_id, foreground_group.map(i64::from))))
                .or_else(|| launched_id.map(|agent_id| (agent_id.to_owned(), child)))
                .map(|(agent_id, process)| {
                    let launched = launched_id == Some(agent_id.as_str());
                    crate::agents::AgentSession {
                        profile: launched.then(|| profile.clone()).flatten(),
                        launched,
                        agent_id,
                        process,
                    }
                });
            let mut changed = false;
            if session.agent_status.as_ref().is_some_and(|status| {
                observed
                    .as_ref()
                    .is_none_or(|agent| agent.agent_id != status.agent_id)
            }) {
                // The agent that reported is gone, even if it never said so.
                session.agent_status = None;
                changed = true;
            }
            if session.agent != observed {
                session.agent = observed;
                changed = true;
            }
            changed
        }
        let mut changed = false;
        for pane in self.panes_mut().into_iter().filter(|pane| !pane.closed) {
            changed |= observe(
                catalog,
                Some(&pane.pty),
                &pane.launch,
                &mut pane.integration,
            );
        }
        self.track_agent_changes();
        changed
    }

    /// Start a change baseline for each agent that just appeared, and
    /// collect finished ones.
    fn track_agent_changes(&mut self) {
        for id in self.owned_pane_ids() {
            let Some(session) = self.pane_session(id) else {
                continue;
            };
            let agent = session
                .agent
                .as_ref()
                .map(|agent| (agent.agent_id.clone(), agent.process));
            let directory = session
                .agent_changes
                .needs_capture(agent.as_ref().map(|(id, process)| (id.as_str(), *process)))
                .then(|| {
                    self.pane_working_directory(id)
                        .map(std::path::PathBuf::from)
                });
            let Some(session) = self.pane_session_mut(id) else {
                continue;
            };
            if let (Some(directory), Some((agent_id, process))) = (directory, &agent) {
                session
                    .agent_changes
                    .start_capture((agent_id, *process), directory);
            }
            session.agent_changes.poll();
        }
    }

    /// The most urgent agent badge among this tab's terminals.
    pub(crate) fn agent_badge(&self) -> Option<crate::agents::AgentBadge> {
        self.owned_pane_ids()
            .into_iter()
            .filter_map(|id| self.pane_session(id)?.agent_status.as_ref()?.badge())
            .chain(self.agent_view.badge)
            .max()
    }

    /// The title the tab bar shows: the conversation's while the agent view
    /// fills the tab, else the terminal's.
    pub(crate) fn display_title(&self) -> &str {
        self.agent_view.title.as_deref().unwrap_or(&self.title)
    }

    #[cfg(test)]
    pub(crate) fn single_pty(&self) -> Option<&PtyHandle> {
        self.single.as_ref().map(|single| &single.pty)
    }

    /// The identity `pane_id`'s processes inherited.
    #[cfg(test)]
    pub(crate) fn pane_terminal_id(&self, pane_id: PaneId) -> Option<&str> {
        self.pane(pane_id)?.pty.terminal_id()
    }

    /// The process `pane_id`'s terminal started, such as its shell.
    pub(crate) fn pane_child_process(&self, pane_id: PaneId) -> Option<u32> {
        self.pane(pane_id)?.pty.child_id()
    }

    /// The process group in the foreground of `pane_id`'s terminal.
    pub(crate) fn pane_foreground_group(&self, pane_id: PaneId) -> Option<i32> {
        self.pane(pane_id)?.pty.foreground_process_group()
    }

    /// The pane whose processes inherited `terminal_id`.
    pub(crate) fn pane_with_terminal_id(&self, terminal_id: &str) -> Option<PaneId> {
        self.owned_pane_ids().into_iter().find(|id| {
            self.pane(*id)
                .is_some_and(|pane| !pane.closed && pane.pty.terminal_id() == Some(terminal_id))
        })
    }

    pub(crate) fn owned_pane_ids(&self) -> Vec<PaneId> {
        match &self.pane_manager {
            Some(panes) => panes.pane_ids(),
            None => self.single.iter().map(|single| single.id).collect(),
        }
    }

    pub(crate) fn unreviewed_diff_count(&self) -> usize {
        self.owned_pane_ids()
            .into_iter()
            .filter_map(|id| self.pane_session(id))
            .map(|state| state.diff_detector.unreviewed_count())
            .sum()
    }

    /// Attach a recorder to the focused owner only.
    #[cfg(test)]
    pub fn start_transcript(&mut self, recorder: TranscriptRecorder) {
        if let Some(state) = self.focused_session_mut() {
            state.start_transcript(recorder);
        }
    }

    #[cfg(test)]
    pub fn stop_transcript(&mut self) -> Option<TranscriptRecorder> {
        self.focused_session_mut()?.stop_transcript()
    }

    pub fn drain_user_notices(&mut self) -> Vec<String> {
        let mut notices = std::mem::take(&mut self.user_notices);
        if let Some(panes) = &mut self.pane_manager {
            for id in panes.pane_ids() {
                if let Some(pane) = panes.get_pane_mut(id) {
                    notices.extend(
                        pane.integration
                            .drain_notices()
                            .into_iter()
                            .map(|text| format!("Pane {}: {text}", id.0)),
                    );
                }
            }
        } else if let Some(single) = &mut self.single {
            notices.extend(single.integration.drain_notices());
        }
        notices
    }

    /// Check if this tab has meaningful content worth saving
    ///
    /// Returns true when the tab has command history, prior session identity,
    /// or scrolled-off output. Short commands that fit on screen still count.
    /// This avoids false positives from splash art or a pristine prompt while
    /// keeping useful working-directory/command state eligible for auto-save.
    pub fn has_content(&self) -> bool {
        self.focused_last_command().is_some()
            || self.session_id.is_some()
            || self
                .focused_terminal()
                .is_some_and(|terminal| terminal.grid().scrollback_len() > 0)
    }

    /// Save this tab as a session
    ///
    /// If the tab already has a session_id, updates that session.
    /// Otherwise, creates a new session with a generated ID.
    /// Returns a report that includes any scrollback truncation.
    pub fn save_as_session(&mut self, save_scrollback: bool) -> anyhow::Result<SessionSaveOutcome> {
        let capture = self.capture_focused_session(save_scrollback)?;
        let working_dir = capture.working_directory;

        // Build session title: "~/path | last_command" or "~/path"
        let path = working_dir
            .as_deref()
            .map(abbreviate_path)
            .unwrap_or_else(|| self.title.clone());
        let session_title = match &capture.last_command {
            Some(cmd) => format!("{} | {}", path, cmd),
            None => path,
        };

        let mut session = if let Some(ref id) = self.session_id {
            // Update existing session
            Session::with_id(id.clone(), session_title, working_dir)
        } else {
            // Create new session
            Session::new(session_title, working_dir)
        };

        // Preserve last_command in the session metadata
        session.last_command = capture.last_command;
        session.capture_scope = capture.scope;

        let session_id = session.id.clone();
        let scrollback_truncated =
            crate::session::save::save_snapshot(&session, &capture.scrollback)
                .context(format!("save snapshot for session '{session_id}'"))?;
        // Identity denotes a published snapshot, never a failed first attempt.
        self.session_id = Some(session_id.clone());
        Ok(SessionSaveOutcome {
            id: session_id,
            scrollback_truncated,
        })
    }

    fn focused_last_command(&self) -> Option<&String> {
        self.focused_pane()?.last_command.as_ref()
    }

    /// Capture every persisted field from one stable focused-terminal source.
    /// This is the sole policy boundary used by every close-time save.
    pub(crate) fn capture_focused_session(
        &self,
        save_scrollback: bool,
    ) -> anyhow::Result<SessionCapture> {
        let pane = self
            .focused_pane()
            .context("focused pane no longer exists while saving session")?;
        let (terminal, child_id, remembered_command) = (
            &pane.terminal,
            pane.pty.child_id(),
            pane.last_command.clone(),
        );
        let scope = if self.pane_manager.is_some() {
            crate::session::SessionCaptureScope::FocusedPane
        } else {
            crate::session::SessionCaptureScope::SingleTerminal
        };

        Ok(SessionCapture {
            working_directory: terminal_working_directory(terminal)
                .or_else(|| child_id.and_then(get_process_cwd)),
            last_command: child_id
                .and_then(get_foreground_process)
                .or(remembered_command),
            scrollback: if save_scrollback {
                crate::session::save::restorable_scrollback(terminal.grid())
            } else {
                Vec::new()
            },
            scope,
        })
    }

    pub(crate) fn capture_workspace_tab(
        &self,
        workspace_tab_id: u32,
        save_scrollback: bool,
    ) -> anyhow::Result<crate::workspace::WorkspaceTabSnapshot> {
        let (primary_pane, focused_pane, layout, panes, launch) =
            if let Some(panes) = &self.pane_manager {
                let primary_id = panes.primary_id();
                let launch = panes
                    .get_pane(primary_id)
                    .context("workspace primary pane no longer exists")?
                    .launch
                    .clone();
                (
                    primary_id.0,
                    panes.focused_id().0,
                    panes.workspace_layout(),
                    panes.workspace_panes(&self.title, save_scrollback),
                    launch,
                )
            } else {
                let single = self
                    .single
                    .as_ref()
                    .context("single terminal owner is detached while saving workspace")?;
                let terminal = &single.terminal;
                let child_id = single.pty.child_id();
                let pane_id = single.id.0;
                (
                    pane_id,
                    pane_id,
                    crate::workspace::WorkspacePaneLayout::Pane { pane: pane_id },
                    vec![crate::workspace::WorkspacePaneSnapshot {
                        id: pane_id,
                        launch: Some(single.launch.clone()),
                        title: Some(self.title.clone()),
                        working_directory: terminal_working_directory(terminal)
                            .or_else(|| child_id.and_then(get_process_cwd)),
                        last_command: child_id
                            .and_then(get_foreground_process)
                            .or_else(|| single.last_command.clone()),
                        scrollback_file: None,
                        scrollback_truncated: false,
                        scrollback_lines: if save_scrollback {
                            crate::session::save::restorable_scrollback(terminal.grid())
                        } else {
                            Vec::new()
                        },
                    }],
                    single.launch.clone(),
                )
            };
        Ok(crate::workspace::WorkspaceTabSnapshot {
            id: workspace_tab_id,
            title: self.title.clone(),
            user_title: self.user_title.clone(),
            pinned: self.pinned,
            launch,
            primary_pane,
            focused_pane,
            layout,
            panes,
        })
    }

    /// Get slide-in animation progress (0.0 = just created, 1.0 = fully visible)
    pub fn slide_progress(&self) -> f32 {
        let elapsed = self.created_at.elapsed().as_secs_f32();
        (elapsed / TAB_SLIDE_DURATION).min(1.0)
    }

    /// Start the close animation
    pub fn start_closing(&mut self) {
        if self.closing_at.is_none() {
            self.closing_at = Some(Instant::now());
        }
    }

    /// Check if this tab is currently in the closing animation
    pub fn is_closing(&self) -> bool {
        self.closing_at.is_some()
    }

    /// Get close animation progress (0.0 = just started closing, 1.0 = fully closed)
    pub fn close_progress(&self) -> f32 {
        match self.closing_at {
            Some(started) => {
                let elapsed = started.elapsed().as_secs_f32();
                (elapsed / TAB_CLOSE_DURATION).min(1.0)
            }
            None => 0.0,
        }
    }

    /// Check if close animation is complete and tab should be removed
    pub fn is_close_complete(&self) -> bool {
        self.close_progress() >= 1.0
    }
}
