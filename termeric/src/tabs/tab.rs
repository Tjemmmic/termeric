//! A tab: its terminals as a tree of panes (one leaf until it is split),
//! plus its title, lifecycle, and session save/restore.

use crate::config::ShellConfig;
use crate::panes::{Pane, PaneId, PaneManager, PaneTerminalEvent};
use crate::session::terminal_state::TerminalIntegration;
#[cfg(test)]
use crate::session::transcript::TranscriptRecorder;
use crate::terminal::geometry::TerminalArea;
use crate::terminal::policy::TerminalPolicy;
use crate::terminal::shell_integration::ShellIntegrationSupport;
use anyhow::Context;
use std::collections::HashMap;
#[cfg(test)]
use std::sync::mpsc::Receiver;
use std::time::Instant;
use termeric_core::Terminal;
#[cfg(test)]
use termeric_core::pty::PtyEvent;
use termeric_core::pty::PtyHandle;

use super::title::{
    abbreviate_path, dir_name_from_path, get_process_cwd, terminal_reported_directory_path,
    terminal_working_directory,
};

/// A closed tab's saved snapshot.
#[derive(Debug, Clone)]
pub struct ClosedTabSave {
    pub id: String,
    pub scrollback_truncated: bool,
}

/// The closed-tab snapshot a tab was reopened from or last saved as, which
/// closing the tab again replaces.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SavedSnapshot {
    pub id: String,
    /// When the snapshot was first saved, kept by every later save.
    pub created_at: String,
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

/// Whether a terminal's own child, its shell or the program the tab
/// launched, holds the terminal's foreground rather than a program it
/// started.
fn child_holds_foreground(child: Option<i64>, foreground_group: Option<i32>) -> bool {
    child.is_some() && child == foreground_group.map(i64::from)
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

/// How a tab's agent view conversation shows in the tab bar.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct TabAgentView {
    /// Set while the agent view fills the tab.
    pub title: Option<String>,
    pub badge: Option<crate::agents::AgentBadge>,
}

/// A terminal tab
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
    /// The tab's terminals. A tab with one terminal is a one-leaf tree;
    /// splitting and closing panes change only the tree.
    pub(crate) panes: PaneManager,
    /// Whether the tab has unread output
    pub has_activity: bool,
    /// Whether every terminal in the tab has exited
    pub closed: bool,
    /// When this tab was created (for slide-in animation)
    pub created_at: Instant,
    /// When this tab started closing (for shrink animation), None if not closing
    pub closing_at: Option<Instant>,
    /// The closed-tab snapshot this tab was reopened from or last saved as.
    pub saved_snapshot: Option<SavedSnapshot>,
    /// What kind of tab this is (shell, plugin, etc.)
    pub tab_type: TabType,
    /// Live domain backing this tab, when its PTYs are authenticated relay
    /// processes rather than locally owned terminal children.
    pub(crate) live_domain: Option<LiveDomainTab>,
    /// The tab's agent view conversation, as the tab bar shows it.
    pub(crate) agent_view: TabAgentView,
    user_notices: Vec<String>,
}

impl Tab {
    /// The process a freshly launched tab's terminal started, while the tab
    /// has just that one terminal.
    pub(crate) fn child_observer(&self) -> Option<termeric_core::pty::ChildObserver> {
        self.panes.lone_pane().map(|pane| pane.pty.child_observer())
    }

    /// A tab around its first terminal, which fills `area`.
    fn with_terminal(
        id: usize,
        title: String,
        pane: Pane,
        area: TerminalArea,
        policy: &TerminalPolicy,
        tab_type: TabType,
        split_shell: ShellConfig,
    ) -> Self {
        Self {
            id,
            title,
            user_title: None,
            pinned: false,
            osc_title: None,
            panes: PaneManager::new(pane, split_shell, *policy, area),
            has_activity: false,
            closed: false,
            created_at: Instant::now(),
            closing_at: None,
            saved_snapshot: None,
            tab_type,
            live_domain: None,
            agent_view: TabAgentView::default(),
            user_notices: Vec::new(),
        }
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
            TerminalArea::new(
                crate::terminal::geometry::TerminalGeometry::from_cells(cols, rows, 1.0, 1.0),
                1.0,
                1.0,
            ),
            launch,
            &TerminalPolicy::with_scrollback(scrollback_lines, scrollback_bytes),
        )
    }

    pub(crate) fn new_command(
        id: usize,
        area: TerminalArea,
        launch: &CommandLaunch,
        policy: &TerminalPolicy,
    ) -> anyhow::Result<Self> {
        let pane = Pane::new_command(PaneId(0), area.geometry, launch, policy)?;
        Ok(Self::with_terminal(
            id,
            launch
                .title
                .clone()
                .unwrap_or_else(|| launch.program.clone()),
            pane,
            area,
            policy,
            launch.tab_type.clone().unwrap_or(TabType::Shell),
            ShellConfig::default(),
        ))
    }

    /// Poll every terminal in the tab, consuming each one's protocol replies
    /// on its own PTY. `on_output` sees each chunk of output read. The tab
    /// closes once every terminal in it has exited.
    pub(crate) fn poll_events(
        &mut self,
        on_output: impl FnMut(PaneId, &[u8]),
    ) -> (bool, Vec<PaneTerminalEvent>) {
        let (changed, events) = self.panes.poll_events(on_output);
        let newly_closed = !self.closed && self.panes.all_closed();
        if newly_closed {
            log::info!("Every terminal in tab {} exited", self.id);
            self.closed = true;
        }
        (changed || newly_closed, events)
    }

    /// Make this tab show a live domain's tab through relay processes. The
    /// daemon answers protocol queries at the real PTYs, so the relays send
    /// no replies of their own.
    pub(crate) fn attach_live_domain(&mut self, live: LiveDomainTab) {
        for pane in self.panes.all_panes_mut() {
            pane.suppress_protocol_replies = true;
            pane.terminal.set_graphics_relay_snapshot_allowed(true);
        }
        self.live_domain = Some(live);
    }

    /// The tab's panes while it is split into more than one. Pane-only
    /// features (broadcast, dividers, zoom, focus outlines) exist only then.
    pub(crate) fn split_panes(&self) -> Option<&PaneManager> {
        (self.panes.pane_count() > 1).then_some(&self.panes)
    }

    pub(crate) fn split_panes_mut(&mut self) -> Option<&mut PaneManager> {
        (self.panes.pane_count() > 1).then_some(&mut self.panes)
    }

    /// The pane holding the focused terminal.
    pub(crate) fn focused_pane(&self) -> Option<&Pane> {
        self.panes.focused_pane()
    }

    pub(crate) fn focused_pane_mut(&mut self) -> Option<&mut Pane> {
        self.panes.focused_pane_mut()
    }

    pub(crate) fn pane(&self, id: PaneId) -> Option<&Pane> {
        self.panes.get_pane(id)
    }

    pub(crate) fn pane_mut(&mut self, id: PaneId) -> Option<&mut Pane> {
        self.panes.get_pane_mut(id)
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
        self.panes
            .apply_terminal_policy(policy, apply_keyboard_type);
    }

    pub(crate) fn live_domain_focus_route(&self) -> Option<(&str, u32)> {
        let live = self.live_domain.as_ref()?;
        let local = self.focused_terminal_identity().pane_id;
        Some((live.domain_id.as_str(), *live.pane_ids.get(&local)?))
    }

    pub(crate) fn focused_terminal_identity(&self) -> super::TerminalIdentity {
        super::TerminalIdentity {
            tab_id: self.id,
            pane_id: self.panes.focused_id(),
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
        self.panes
            .pane_ids()
            .into_iter()
            .find(|pane_id| self.pane_plugin_id(*pane_id) == Some(plugin_id))
    }

    pub(crate) fn focused_pane_read_only(&self) -> bool {
        self.focused_pane().is_some_and(|pane| pane.read_only)
    }

    /// Toggle the focused pane's transient user-input lock and return its new
    /// state. The lock follows a live pane when it moves to another tab.
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

    /// Write user input and record it only on the receiving terminal.
    pub fn write(&mut self, data: &[u8]) -> std::io::Result<()> {
        self.focused_pane_for_write()?.write(data)
    }

    fn focused_pane_for_write(&mut self) -> std::io::Result<&mut Pane> {
        self.focused_pane_mut().ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "focused pane no longer exists",
            )
        })
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

    /// Record an OSC title emitted by pane `id`, which a split pane keeps
    /// as its own; a tab's only pane shows the tab's title instead. Returns
    /// false when the tab has no such pane.
    pub(crate) fn set_pane_osc_title(&mut self, id: PaneId, title: &str) -> bool {
        let split = self.panes.pane_count() > 1;
        let Some(pane) = self.pane_mut(id) else {
            return false;
        };
        if split {
            pane.title = (!title.is_empty()).then(|| title.to_string());
        }
        true
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

    /// A tab running `shell`, titled `title`, else after `working_dir`, else
    /// after the shell's own working directory.
    pub(crate) fn new_shell(
        id: usize,
        area: TerminalArea,
        shell: &ShellConfig,
        policy: &TerminalPolicy,
        working_dir: Option<&str>,
        title: Option<&str>,
    ) -> anyhow::Result<Self> {
        let pane = Pane::new_local(PaneId(0), area.geometry, shell, policy, working_dir, false)?;
        // Use the given title, else the working directory's name.
        let title = title
            .map(str::to_string)
            .or_else(|| working_dir.map(dir_name_from_path))
            .or_else(|| {
                pane.pty
                    .child_id()
                    .and_then(get_process_cwd)
                    .map(|cwd| dir_name_from_path(&cwd))
            })
            .unwrap_or_else(|| format!("Tab {}", id + 1));
        Ok(Self::with_terminal(
            id,
            title,
            pane,
            area,
            policy,
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

    pub(crate) fn pane_working_directory(&self, pane_id: PaneId) -> Option<String> {
        if self.closed || self.is_closing() || matches!(self.tab_type, TabType::Serial { .. }) {
            return None;
        }
        let pane = self.pane(pane_id).filter(|pane| !pane.closed)?;
        terminal_working_directory(&pane.terminal)
            .or_else(|| pane.pty.child_id().and_then(get_process_cwd))
    }

    /// Have this tab's splits launch `shell`, and record it as the launch
    /// of the tab's only terminal when that is a shell.
    pub(crate) fn set_split_shell_config(&mut self, shell: ShellConfig) {
        if let Some(pane) = self.panes.lone_pane_mut()
            && matches!(pane.launch, crate::workspace::WorkspaceLaunch::Shell)
        {
            pane.launch_shell = shell.clone();
        }
        self.panes.adopt_primary_shell(shell);
    }

    pub(crate) fn set_terminal_profile_launch(&mut self, name: &str, shell: ShellConfig) {
        if let Some(pane) = self.panes.lone_pane_mut() {
            pane.launch = crate::workspace::WorkspaceLaunch::Profile {
                name: name.to_string(),
            };
            pane.launch_shell = shell.clone();
        }
        self.panes.adopt_primary_shell(shell);
    }

    pub(crate) fn set_serial_profile_launch(&mut self, name: &str, title: String) {
        self.tab_type = TabType::Serial {
            name: name.to_string(),
        };
        if let Some(pane) = self.panes.lone_pane_mut() {
            pane.launch = crate::workspace::WorkspaceLaunch::Profile {
                name: name.to_string(),
            };
            pane.launch_shell = ShellConfig::default();
            pane.last_command = None;
            pane.shell_integration = ShellIntegrationSupport::Unavailable(
                "Serial terminals do not provide shell prompt marks".into(),
            );
        }
        self.panes.adopt_primary_shell(ShellConfig::default());
        self.set_user_title(Some(title));
    }

    pub(crate) fn allows_pane_splits(&self) -> bool {
        !matches!(self.tab_type, TabType::Serial { .. })
    }

    /// Whether a file or folder the person drops on `pane_id` may be typed
    /// into it as a quoted path. A drop is a deliberate act, so agent and
    /// plugin panes take it just as a shell does; a serial device is not a
    /// program on this computer, so it is never told this computer's paths.
    /// Live-domain panes are accepted; a path dropped on one becomes an
    /// upload the person confirms.
    pub(crate) fn pane_accepts_dropped_paths(&self, pane_id: PaneId) -> bool {
        self.pane(pane_id).is_some()
            && (self.live_domain.is_some() || !matches!(self.tab_type, TabType::Serial { .. }))
    }

    /// Whether the program in `pane_id` may ask to read or write this
    /// computer's files through Kitty file transfer. Unlike a drop, the
    /// program starts the transfer, so plugin and agent programs are refused
    /// wherever the pane sits (a split or a move puts them in any tab), and
    /// a serial device is refused as for drops. Live-domain panes are
    /// accepted.
    pub(crate) fn pane_accepts_program_file_transfers(&self, pane_id: PaneId) -> bool {
        let Some(pane) = self.pane(pane_id) else {
            return false;
        };
        if self.live_domain.is_some() {
            return true;
        }
        !matches!(self.tab_type, TabType::Serial { .. })
            && !matches!(
                pane.launch,
                crate::workspace::WorkspaceLaunch::Plugin { .. }
            )
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

    /// Keep tab-level presentation and the launch of later splits aligned
    /// with the primary pane after a pane closes or moves away.
    pub(crate) fn reconcile_primary_pane_launch(&mut self) {
        let Some(primary) = self.panes.get_pane(self.panes.primary_id()) else {
            return;
        };
        let launch = primary.launch.clone();
        let shell = primary.launch_shell.clone();
        match launch {
            crate::workspace::WorkspaceLaunch::Shell => {
                self.tab_type = TabType::Shell;
                self.panes.adopt_primary_shell(shell);
            }
            crate::workspace::WorkspaceLaunch::Profile { name } => {
                self.tab_type = TabType::Profile { name };
                self.panes.adopt_primary_shell(shell);
            }
            crate::workspace::WorkspaceLaunch::Plugin { plugin_id, .. } => {
                self.tab_type = TabType::Plugin { plugin_id };
            }
        }
    }

    /// This tab's panes, laid out in a `width` by `height` area of
    /// `cell_width` by `cell_height` cells, with later splits launching
    /// `shell`: a tab set up for a test to split.
    #[cfg(test)]
    pub(crate) fn panes_for_test(
        &mut self,
        shell: ShellConfig,
        width: f32,
        height: f32,
        cell_width: f32,
        cell_height: f32,
    ) -> &mut PaneManager {
        self.panes.resize(TerminalArea::from_available_pixels(
            width,
            height,
            cell_width,
            cell_height,
        ));
        self.panes.adopt_primary_shell(shell);
        &mut self.panes
    }

    /// Feed the tab's only terminal from `receiver` instead of its PTY.
    #[cfg(test)]
    pub(crate) fn replace_lone_test_output(&mut self, receiver: Receiver<PtyEvent>) {
        self.panes
            .lone_pane_mut()
            .expect("tab has one terminal")
            .pty_rx = receiver;
    }

    #[cfg(test)]
    pub(crate) fn lone_child_id(&self) -> Option<u32> {
        self.panes.lone_pane()?.pty.child_id()
    }

    #[cfg(test)]
    pub(crate) fn lone_pty_size(&self) -> Option<termeric_core::pty::PtySize> {
        self.panes.lone_pane()?.pty.size().ok()
    }

    pub(crate) fn pane_integration(&self, id: PaneId) -> Option<&TerminalIntegration> {
        self.pane(id).map(|pane| &pane.integration)
    }

    pub(crate) fn pane_terminal(&self, id: PaneId) -> Option<&Terminal> {
        self.pane(id).map(|pane| &pane.terminal)
    }

    pub(crate) fn pane_integration_mut(&mut self, id: PaneId) -> Option<&mut TerminalIntegration> {
        self.pane_mut(id).map(|pane| &mut pane.integration)
    }

    #[cfg(test)]
    pub(crate) fn focused_integration(&self) -> Option<&TerminalIntegration> {
        self.pane_integration(self.focused_terminal_identity().pane_id)
    }

    #[cfg(test)]
    pub(crate) fn focused_integration_mut(&mut self) -> Option<&mut TerminalIntegration> {
        self.pane_integration_mut(self.focused_terminal_identity().pane_id)
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
            session: &mut crate::session::terminal_state::TerminalIntegration,
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
            let child_in_front = child_holds_foreground(child, foreground_group);
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
            changed |= session.set_agent(observed);
            changed
        }
        let mut changed = false;
        for pane in self.panes.all_panes_mut().filter(|pane| !pane.closed) {
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

    /// Forget the agent in each of this tab's terminals and what it
    /// reported. Change baselines stay, inert, so an agent recognized again
    /// later keeps the one taken when it started. Returns whether any
    /// terminal changed.
    pub(crate) fn forget_agents(&mut self) -> bool {
        let mut changed = false;
        for pane in self.panes.all_panes_mut() {
            let session = &mut pane.integration;
            changed |= session.agent_status.take().is_some();
            changed |= session.set_agent(None);
        }
        changed
    }

    /// Start a change baseline for each agent that just appeared, and
    /// collect finished ones.
    fn track_agent_changes(&mut self) {
        for id in self.owned_pane_ids() {
            let Some(session) = self.pane_integration(id) else {
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
            let Some(session) = self.pane_integration_mut(id) else {
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

    /// Whether any of this tab's terminals runs a program besides an idle
    /// shell: a profile or agent the tab launched, or a program started at
    /// a shell's prompt, which then holds the terminal's foreground. Closing
    /// the tab ends that program.
    pub(crate) fn has_running_program(&self) -> bool {
        self.panes.iter_panes().any(|pane| {
            !pane.closed
                && (!matches!(pane.launch, crate::workspace::WorkspaceLaunch::Shell)
                    || pane.pty.foreground_process_group().is_some_and(|group| {
                        !child_holds_foreground(pane.pty.child_id().map(i64::from), Some(group))
                    }))
        })
    }

    /// The most urgent agent badge among this tab's terminals.
    pub(crate) fn agent_badge(&self) -> Option<crate::agents::AgentBadge> {
        self.owned_pane_ids()
            .into_iter()
            .filter_map(|id| self.pane_integration(id)?.agent_status.as_ref()?.badge())
            .chain(self.agent_view.badge)
            .max()
    }

    /// The title the tab bar shows: the conversation's while the agent view
    /// fills the tab, else the terminal's.
    pub(crate) fn display_title(&self) -> &str {
        self.agent_view.title.as_deref().unwrap_or(&self.title)
    }

    #[cfg(test)]
    pub(crate) fn lone_pty(&self) -> Option<&PtyHandle> {
        self.panes.lone_pane().map(|pane| &pane.pty)
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
        self.panes.pane_ids()
    }

    pub(crate) fn unreviewed_diff_count(&self) -> usize {
        self.owned_pane_ids()
            .into_iter()
            .filter_map(|id| self.pane_integration(id))
            .map(|state| state.diff_detector.unreviewed_count())
            .sum()
    }

    /// Attach a recorder to the focused owner only.
    #[cfg(test)]
    pub fn start_transcript(&mut self, recorder: TranscriptRecorder) {
        if let Some(state) = self.focused_integration_mut() {
            state.start_transcript(recorder);
        }
    }

    #[cfg(test)]
    pub fn stop_transcript(&mut self) -> Option<TranscriptRecorder> {
        self.focused_integration_mut()?.stop_transcript()
    }

    pub fn drain_user_notices(&mut self) -> Vec<String> {
        let mut notices = std::mem::take(&mut self.user_notices);
        // A split tab's notices say which pane they are about.
        let split = self.panes.pane_count() > 1;
        for id in self.panes.pane_ids() {
            if let Some(pane) = self.panes.get_pane_mut(id) {
                notices.extend(pane.integration.drain_notices().into_iter().map(|text| {
                    if split {
                        format!("Pane {}: {text}", id.0)
                    } else {
                        text
                    }
                }));
            }
        }
        notices
    }

    /// Whether the tab is worth keeping as a closed-tab snapshot: it was
    /// reopened from one, or any of its panes ran a command or scrolled
    /// output off screen. Short commands that fit on screen still count;
    /// splash art or a pristine prompt does not.
    pub fn has_content(&self) -> bool {
        self.saved_snapshot.is_some()
            || self.panes.iter_panes().any(|pane| {
                pane.last_command.is_some() || pane.terminal.grid().scrollback_len() > 0
            })
    }

    /// This tab as the one-tab workspace kept after it closes: every pane,
    /// named for the focused pane's directory (else the tab's title) and last
    /// command. A tab reopened from such a snapshot keeps that snapshot's ID
    /// and creation time.
    pub(crate) fn closed_tab_snapshot(
        &self,
        save_scrollback: bool,
    ) -> anyhow::Result<crate::workspace::WorkspaceSnapshot> {
        let tab = self.capture_workspace_tab(0, save_scrollback)?;
        let focused = tab
            .panes
            .iter()
            .find(|pane| pane.id == tab.focused_pane)
            .context("focused pane no longer exists while saving the closed tab")?;
        let name = closed_tab_name(
            focused.working_directory.as_deref(),
            &self.title,
            focused.last_command.as_deref(),
        );
        let id = self.saved_snapshot.as_ref().map_or_else(
            || format!("workspace-{}", crate::session::generate_session_id()),
            |saved| saved.id.clone(),
        );
        let mut workspace = crate::workspace::WorkspaceSnapshot::new(id, name, vec![tab]);
        if let Some(saved) = &self.saved_snapshot {
            workspace.created_at.clone_from(&saved.created_at);
        }
        workspace.closed_tab = true;
        Ok(workspace)
    }

    /// Save [`Self::closed_tab_snapshot`], replacing the snapshot this tab
    /// was reopened from, if any.
    pub fn save_closed_tab(&mut self, save_scrollback: bool) -> anyhow::Result<ClosedTabSave> {
        let mut workspace = self.closed_tab_snapshot(save_scrollback)?;
        let report = crate::workspace::save_workspace(&mut workspace)
            .with_context(|| format!("save closed tab '{}'", workspace.id))?;
        // Identity denotes a published snapshot, never a failed first attempt.
        self.saved_snapshot = Some(SavedSnapshot {
            id: report.id.clone(),
            created_at: workspace.created_at,
        });
        Ok(ClosedTabSave {
            scrollback_truncated: !report.truncated_panes.is_empty(),
            id: report.id,
        })
    }

    pub(crate) fn capture_workspace_tab(
        &self,
        workspace_tab_id: u32,
        save_scrollback: bool,
    ) -> anyhow::Result<crate::workspace::WorkspaceTabSnapshot> {
        Ok(crate::workspace::WorkspaceTabSnapshot {
            id: workspace_tab_id,
            title: crate::workspace::bounded_workspace_title(&self.title),
            user_title: self
                .user_title
                .as_deref()
                .map(crate::workspace::bounded_workspace_title),
            pinned: self.pinned,
            primary_pane: self.panes.primary_id().0,
            focused_pane: self.panes.focused_id().0,
            layout: self.panes.workspace_layout(),
            panes: self.panes.workspace_panes(&self.title, save_scrollback),
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

/// A closed tab's snapshot name: its focused pane's directory, else the
/// tab's title, and the pane's last command, as one valid workspace name.
fn closed_tab_name(directory: Option<&str>, title: &str, last_command: Option<&str>) -> String {
    let place = directory.map_or_else(|| title.to_string(), abbreviate_path);
    let name = match last_command {
        Some(command) => format!("{place} | {command}"),
        None => place,
    };
    let name: String = name
        .chars()
        .filter(|character| !character.is_control())
        .take(crate::workspace::MAX_WORKSPACE_NAME_CHARS)
        .collect();
    // A title can be blank, such as an OSC title of only spaces.
    if name.trim().is_empty() {
        "Closed tab".into()
    } else {
        name
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn closed_tab_names_are_always_valid_workspace_names() {
        assert_eq!(
            closed_tab_name(Some("/srv/project"), "ignored", Some("make")),
            "/srv/project | make"
        );
        assert_eq!(closed_tab_name(None, "build", None), "build");
        for blank in ["   ", "", "\u{7}\t"] {
            assert_eq!(
                closed_tab_name(None, blank, None),
                "Closed tab",
                "{blank:?}"
            );
        }
        assert_eq!(closed_tab_name(None, "  ", Some("make")), "   | make");
        let long = closed_tab_name(None, &"x".repeat(300), Some("make"));
        assert_eq!(
            long.chars().count(),
            crate::workspace::MAX_WORKSPACE_NAME_CHARS
        );
        for name in [
            closed_tab_name(None, "   ", None),
            closed_tab_name(None, "a\nb", None),
            long,
        ] {
            crate::workspace::validate_workspace_name(&name).unwrap();
        }
    }
}
