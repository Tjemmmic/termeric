//! A tab's terminals as panes in a binary split tree. Every tab has one
//! tree; a tab with a single terminal is a one-leaf tree.

mod tree;
pub use termeric_core::split::PaneLayoutPreset;
pub(crate) use tree::PaneNode;
pub use tree::{PaneId, PaneTree, SplitDirection};

pub const PANE_DIVIDER_THICKNESS: f32 = 4.0;
pub const PANE_DIVIDER_HIT_MARGIN: f32 = 8.0;

/// The most panes one tab holds, however they are added. It is what a
/// workspace saves, so every tab can be saved and restored; the control
/// protocol's limit is this one.
pub const MAX_PANES_PER_TAB: usize = 64;

// A live-domain tab shows every pane of its daemon, so they must all fit.
const _: () = assert!(crate::domain::model::MAX_DAEMON_PANES <= MAX_PANES_PER_TAB);

use crate::config::{PaneCwdMode, ShellConfig};
use crate::session::terminal_state::DiffWatch;
use crate::terminal::events::{MAX_FORWARDED_TERMINAL_EVENTS_PER_POLL, drain_terminal_events};
use crate::terminal::geometry::{TerminalArea, TerminalGeometry};
use crate::terminal::shell_integration::{
    ShellIntegrationSupport, ShellLaunch, prepare_shell_launch,
};
use crate::ui::geometry::Bounds;
use anyhow::Context;
use std::collections::HashMap;
use std::io;
use std::sync::mpsc::Receiver;
use termeric_core::pty::{PtyCommand, PtyEvent, PtyHandle, spawn_reader_thread_with_waker};
use termeric_core::{Pty, Terminal, TerminalEvent};

use crate::terminal::policy::TerminalPolicy;

/// Four-way direction for pane navigation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PaneDir {
    Up,
    Down,
    Left,
    Right,
}

/// App-facing terminal event tagged with the pane that produced it. Protocol
/// replies are consumed inside [`Pane::poll_events`] and never appear here.
#[derive(Debug, Clone)]
pub struct PaneTerminalEvent {
    pub pane_id: PaneId,
    pub event: TerminalEvent,
}

/// What polling one pane, or every pane of a tab, found.
#[derive(Debug, Default)]
pub struct PanePoll {
    /// Whether anything shown changed.
    pub changed: bool,
    /// Whether any terminal output was read.
    pub saw_output: bool,
    /// App-facing events, at most [`MAX_FORWARDED_TERMINAL_EVENTS_PER_POLL`]
    /// from each pane.
    pub events: Vec<PaneTerminalEvent>,
}

impl PaneDir {
    fn opposite(self) -> Self {
        match self {
            Self::Up => Self::Down,
            Self::Down => Self::Up,
            Self::Left => Self::Right,
            Self::Right => Self::Left,
        }
    }
}

/// A single pane containing a terminal
pub struct Pane {
    /// Unique pane ID
    pub id: PaneId,
    /// Last OSC title emitted by this pane.
    pub(crate) title: Option<String>,
    /// Terminal state
    pub terminal: Terminal,
    /// PTY handle for writing
    pub pty: PtyHandle,
    /// PTY output receiver
    pub pty_rx: Receiver<PtyEvent>,
    /// Whether the shell process has exited
    pub closed: bool,
    /// Transient user-input lock. Terminal protocol replies remain writable.
    pub(crate) read_only: bool,
    /// Current cell and physical-pixel geometry.
    pub(crate) geometry: TerminalGeometry,
    pub harvest: crate::harvest::HarvestTerminalState,
    pub shell_integration: ShellIntegrationSupport,
    /// Last foreground command observed for this pane. Pane-local ownership
    /// prevents focus changes from persisting another pane's metadata.
    pub(crate) last_command: Option<String>,
    /// Relaunch identity and frozen process configuration travel with the live
    /// owner when a pane is reparented.
    pub(crate) launch: crate::workspace::WorkspaceLaunch,
    pub(crate) launch_shell: ShellConfig,
    /// The PTY runs a program named when the tab opened (`termeric --
    /// COMMAND`, a plugin's command), not an interactive shell, so that
    /// program is running for as long as the pane is open.
    pub(crate) runs_command: bool,
    pub(crate) integration: crate::session::terminal_state::TerminalIntegration,
    event_overflow_reported: bool,
    /// Daemon-owned terminals consume protocol replies at the real PTY; their
    /// GUI replicas must not send a duplicate reply through the relay.
    pub(crate) suppress_protocol_replies: bool,
}

/// One resolved local launch; process arguments are never inferred from focus.
pub(crate) struct LocalPaneLaunch {
    pub identity: crate::workspace::WorkspaceLaunch,
    pub shell: ShellConfig,
    pub cwd: Option<String>,
}

/// What a split launches.
enum SplitLaunch<'a> {
    /// The tab's split shell, in the directory the mode picks.
    Shell(PaneCwdMode),
    /// Exactly this launch.
    Given(&'a LocalPaneLaunch),
}

impl Pane {
    /// Whether this terminal's shell reports prompts: Termeric installed the
    /// hooks, or the program marked a prompt itself (a manually enabled
    /// Elvish module, or the user's own integration).
    pub(crate) fn shell_integration_active(&self) -> bool {
        matches!(self.shell_integration, ShellIntegrationSupport::Enabled(_))
            || self.terminal.prompt_tracker().prompt_marks_seen()
    }

    /// Where this pane's program works: the directory its shell last
    /// reported for this machine, else its child's current directory.
    pub(crate) fn working_directory(&self) -> Option<String> {
        crate::terminal::process::working_directory(&self.terminal, &self.pty)
    }

    /// The profile of a pane launched from a plugin or agent profile.
    #[cfg(test)]
    pub(crate) fn launch_profile(&self) -> Option<&str> {
        match &self.launch {
            crate::workspace::WorkspaceLaunch::Plugin { profile, .. } => profile.as_deref(),
            _ => None,
        }
    }

    /// Create a new pane with a shell
    #[cfg(test)]
    pub fn new(
        id: PaneId,
        geometry: TerminalGeometry,
        shell: &ShellConfig,
        scrollback_lines: usize,
        scrollback_bytes: usize,
        working_dir: Option<&str>,
    ) -> anyhow::Result<Self> {
        Self::new_local(
            id,
            geometry,
            shell,
            &TerminalPolicy::with_scrollback(scrollback_lines, scrollback_bytes),
            working_dir,
            false,
        )
    }

    /// A pane of `geometry` running `command` on a new PTY under `policy`,
    /// launched as a shell with nothing yet known about it: no title, no
    /// command, and the default shell configuration. Each constructor sets
    /// what it knows.
    fn spawn(
        id: PaneId,
        geometry: TerminalGeometry,
        policy: &TerminalPolicy,
        command: PtyCommand<'_>,
        shell_integration: ShellIntegrationSupport,
    ) -> anyhow::Result<Self> {
        let mut terminal = Terminal::new(geometry.cols, geometry.rows);
        terminal.set_pixel_geometry(geometry.pixel_width.into(), geometry.pixel_height.into());
        policy.apply(&mut terminal, true);
        let pty = Pty::spawn(geometry.pty_size(), command)?;
        terminal.set_term_name(pty.term_name());
        let (handle, reader) = pty.take_reader();
        let rx = spawn_reader_thread_with_waker(reader, crate::app::pty_event_waker());
        Ok(Self {
            id,
            title: None,
            terminal,
            pty: handle,
            pty_rx: rx,
            closed: false,
            read_only: false,
            geometry,
            harvest: crate::harvest::HarvestTerminalState::default(),
            shell_integration,
            last_command: None,
            integration: crate::session::terminal_state::TerminalIntegration::default(),
            launch: crate::workspace::WorkspaceLaunch::Shell,
            launch_shell: ShellConfig::default(),
            runs_command: false,
            event_overflow_reported: false,
            suppress_protocol_replies: false,
        })
    }

    pub(crate) fn new_local(
        id: PaneId,
        geometry: TerminalGeometry,
        shell: &ShellConfig,
        policy: &TerminalPolicy,
        working_dir: Option<&str>,
        plugin: bool,
    ) -> anyhow::Result<Self> {
        let launch = if plugin {
            ShellLaunch::without_integration(
                shell,
                "Plugin launches do not install local shell hooks",
            )
        } else {
            prepare_shell_launch(shell)
        };
        let mut pane = Self::spawn(
            id,
            geometry,
            policy,
            PtyCommand {
                program: launch.program.as_deref(),
                args: &launch.args,
                cwd: working_dir.map(std::path::Path::new),
                env: &launch.env,
                ..PtyCommand::default()
            },
            launch.integration,
        )?;
        pane.launch_shell = shell.clone();
        Ok(pane)
    }

    /// Create a pane running a program directly, without a shell or its
    /// prompt marks: a command tab's terminal.
    pub(crate) fn new_command(
        id: PaneId,
        geometry: TerminalGeometry,
        launch: &crate::tabs::CommandLaunch,
        policy: &TerminalPolicy,
    ) -> anyhow::Result<Self> {
        let env: Vec<(String, String)> = launch
            .env
            .iter()
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect();
        let mut pane = Self::spawn(
            id,
            geometry,
            policy,
            PtyCommand {
                program: Some(&launch.program),
                args: &launch.args,
                cwd: launch.working_dir.as_deref().map(std::path::Path::new),
                env: &env,
                policy: Some(&launch.environment_policy),
            },
            ShellIntegrationSupport::Unavailable(
                "Direct command tabs do not provide shell prompt marks".into(),
            ),
        )?;
        pane.last_command = Some(launch.program.clone());
        pane.runs_command = true;
        if let Some(crate::tabs::TabType::Plugin { plugin_id }) = &launch.tab_type {
            pane.launch = crate::workspace::WorkspaceLaunch::Plugin {
                plugin_id: plugin_id.clone(),
                profile: None,
            };
        }
        Ok(pane)
    }

    /// A pane showing the live domain's pane `remote_pane` through an
    /// authenticated relay process. The daemon answers protocol queries at
    /// the real PTY, so the relay sends no replies of its own, and it takes
    /// the daemon's graphics snapshot.
    pub(crate) fn new_relay(
        id: PaneId,
        geometry: TerminalGeometry,
        relay_command: &crate::domain::DomainRelayCommand,
        domain_id: &str,
        remote_pane: u32,
        policy: &TerminalPolicy,
        title: Option<String>,
    ) -> anyhow::Result<Self> {
        let arguments = relay_command.pane_arguments(domain_id, remote_pane);
        let mut pane = Self::spawn(
            id,
            geometry,
            policy,
            PtyCommand {
                program: Some(relay_command.program()),
                args: &arguments,
                ..PtyCommand::default()
            },
            ShellIntegrationSupport::Unavailable(
                "Persistent-domain relays do not install a local shell hook".into(),
            ),
        )?;
        pane.terminal.set_graphics_relay_snapshot_allowed(true);
        pane.title = title;
        pane.last_command = Some("termeric domain relay".into());
        pane.suppress_protocol_replies = true;
        Ok(pane)
    }

    /// Poll PTY output and immediately consume terminal-generated replies on
    /// this pane's own PTY. App-facing events are drained on every 4 KiB output
    /// chunk and bounded before being returned. `agent_features` is whether
    /// they are on in the window that owns the pane.
    pub fn poll_events(&mut self, agent_features: bool) -> PanePoll {
        let mut changed = self.integration.poll_notices();
        let mut saw_output = false;
        let was_closed = self.closed;
        let mut app_events = Vec::new();
        let mut reader_disconnected = false;
        let mut output_ended = false;

        loop {
            let event = match self.pty_rx.try_recv() {
                Ok(event) => event,
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    reader_disconnected = true;
                    break;
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => break,
            };
            match event {
                PtyEvent::Output(data) => {
                    self.integration
                        .observe_output(&data, self.diff_watch(agent_features));
                    saw_output |= !data.is_empty();
                    self.terminal.process(&data);
                    changed = true;
                    changed |= self.drain_terminal_events(&mut app_events);
                }
                PtyEvent::Closed => {
                    log::info!("Pane {:?} shell exited", self.id);
                    self.closed = true;
                    output_ended = true;
                }
                PtyEvent::Error(e) => {
                    log::error!("Pane {:?} PTY error: {}", self.id, e);
                    self.closed = true;
                    output_ended = true;
                }
            }
        }

        // Catch events explicitly queued outside the parser and replies left by
        // the final output chunk.
        if output_ended || reader_disconnected {
            self.integration.finish_stream();
        }
        changed |= self.drain_terminal_events(&mut app_events);

        if changed
            && let Some(command) = self
                .pty
                .child_id()
                .and_then(crate::terminal::process::get_foreground_process)
        {
            self.last_command = Some(command);
        }

        if !self.closed && reader_disconnected && !self.pty.is_running() {
            log::info!(
                "Pane {:?} child process exited (detected via wait)",
                self.id
            );
            self.closed = true;
        }

        PanePoll {
            changed: changed || self.closed != was_closed || !app_events.is_empty(),
            saw_output,
            events: app_events
                .into_iter()
                .map(|event| PaneTerminalEvent {
                    pane_id: self.id,
                    event,
                })
                .collect(),
        }
    }

    /// Whose diffs this pane's output is watched for: none while agent
    /// features are off, everything in a terminal Termeric started as a
    /// plugin or agent, and otherwise the agent a scan has seen in it.
    fn diff_watch(&self, agent_features: bool) -> DiffWatch {
        if !agent_features {
            DiffWatch::Off
        } else if matches!(
            self.launch,
            crate::workspace::WorkspaceLaunch::Plugin { .. }
        ) {
            DiffWatch::All
        } else {
            DiffWatch::SeenAgent
        }
    }

    fn drain_terminal_events(&mut self, app_events: &mut Vec<TerminalEvent>) -> bool {
        let suppress_protocol_replies = self.suppress_protocol_replies;
        let result = drain_terminal_events(
            &mut self.terminal,
            app_events,
            MAX_FORWARDED_TERMINAL_EVENTS_PER_POLL,
            |data| {
                if suppress_protocol_replies {
                    Ok(())
                } else {
                    self.pty.write_all(data)
                }
            },
        );
        if let Some(error) = result.reply_error {
            log::error!(
                "Pane {:?} failed to write terminal protocol reply: {}",
                self.id,
                error
            );
            self.closed = true;
        }
        if result.dropped_app_events > 0 && !self.event_overflow_reported {
            self.event_overflow_reported = true;
            log::warn!(
                "Pane {:?} dropped {} app-facing terminal events after reaching the per-poll limit of {}",
                self.id,
                result.dropped_app_events,
                MAX_FORWARDED_TERMINAL_EVENTS_PER_POLL
            );
        }
        result.saw_event
    }

    /// Write data to the PTY
    pub fn write(&mut self, data: &[u8]) -> std::io::Result<()> {
        if self.read_only {
            return Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "pane is read-only",
            ));
        }
        self.pty.write_all(data)?;
        self.integration.record_input(data);
        // Typing returns to the live prompt, so a prompt jump's mark ends.
        self.terminal.end_prompt_navigation();
        Ok(())
    }

    /// Write an app-mediated terminal reply without bypassing the read-only
    /// policy for user input elsewhere.
    pub(crate) fn write_protocol_reply(&mut self, data: &[u8]) -> std::io::Result<()> {
        self.pty.write_all(data)
    }

    /// Resize the terminal and PTY
    pub(crate) fn resize_geometry(&mut self, geometry: TerminalGeometry) -> anyhow::Result<()> {
        if geometry != self.geometry {
            self.terminal.resize_with_pixel_geometry(
                geometry.cols,
                geometry.rows,
                geometry.pixel_width.into(),
                geometry.pixel_height.into(),
            );
            self.pty.resize(geometry.pty_size())?;
            self.geometry = geometry;
        }
        Ok(())
    }
}

/// Replays a saved pane's scrollback into its freshly launched terminal.
pub(crate) fn replay_workspace_scrollback(terminal: &mut Terminal, lines: &[String]) {
    if !lines.is_empty() {
        terminal.process((lines.join("\r\n") + "\r\n").as_bytes());
    }
}

/// The launch and shell each saved pane relaunches with, by saved pane ID.
pub(crate) type WorkspacePaneLaunches =
    HashMap<u32, (crate::workspace::WorkspaceLaunch, ShellConfig)>;

/// The saved tab's primary pane's working directory, as
/// [`usable_workspace_cwd`] finds it.
pub(crate) fn usable_primary_workspace_cwd(
    snapshot: &crate::workspace::WorkspaceTabSnapshot,
    warnings: &mut Vec<String>,
) -> anyhow::Result<Option<String>> {
    let primary = snapshot
        .panes
        .iter()
        .find(|pane| pane.id == snapshot.primary_pane)
        .with_context(|| format!("workspace tab {} has no primary pane", snapshot.id))?;
    Ok(usable_workspace_cwd(
        primary.working_directory.as_deref(),
        snapshot.id,
        snapshot.primary_pane,
        warnings,
    ))
}

/// A saved pane's working directory if it still exists; otherwise a warning
/// and `None`, so the relaunch starts in the process default.
fn usable_workspace_cwd(
    cwd: Option<&str>,
    tab_id: u32,
    pane_id: u32,
    warnings: &mut Vec<String>,
) -> Option<String> {
    let cwd = cwd?;
    if std::path::Path::new(cwd).is_dir() {
        Some(cwd.to_string())
    } else {
        warnings.push(format!(
            "tab {tab_id} pane {pane_id} cwd {cwd:?} is unavailable; used the process default"
        ));
        None
    }
}

fn domain_layout_to_local(
    layout: &crate::domain::model::LivePaneLayout,
    ids: &HashMap<u32, PaneId>,
) -> anyhow::Result<PaneNode> {
    Ok(match layout {
        crate::domain::model::LivePaneLayout::Pane { pane } => PaneNode::Leaf(
            *ids.get(pane)
                .context("live domain layout references a missing pane")?,
        ),
        crate::domain::model::LivePaneLayout::Split {
            split_id,
            direction,
            ratio_millis,
            first,
            second,
        } => PaneNode::Split {
            local_split_id: 0,
            domain_split_id: Some(*split_id),
            direction: *direction,
            ratio: f32::from(*ratio_millis) / 1_000.0,
            first: Box::new(domain_layout_to_local(first, ids)?),
            second: Box::new(domain_layout_to_local(second, ids)?),
        },
    })
}

/// A tab's panes: the split tree, the terminal in each leaf, and focus.
pub struct PaneManager {
    /// The pane tree structure
    tree: PaneTree,
    /// Map of pane IDs to panes
    panes: HashMap<PaneId, Pane>,
    /// Currently focused pane
    focused: PaneId,
    /// Saved/adopted launch primary, retained independently of numeric IDs.
    primary: PaneId,
    /// View-only maximization; the underlying split tree remains unchanged.
    zoomed: Option<PaneId>,
    /// Last intentional route from a pane in a direction. This makes crossing
    /// an asymmetric split reversible: bottom-right -> left -> right returns
    /// to bottom-right instead of re-running an ambiguous geometric tie.
    navigation_memory: HashMap<(PaneId, PaneDir), PaneId>,
    /// The shell a split launches when it is given no launch of its own. It
    /// follows the primary pane's (see [`Self::adopt_primary_shell`]).
    split_shell: ShellConfig,
    /// Host settings of every pane, inherited by panes created later.
    policy: TerminalPolicy,
    /// Next pane ID. IDs are never reused within a tab, so this only grows.
    next_id: u32,
    /// The tab's terminal area, which the tree divides.
    area: TerminalArea,
}

impl PaneManager {
    /// A tab's tree around its first terminal: one leaf, focused and primary.
    /// `pane` already fills `area`; later splits launch `split_shell` under
    /// `policy`.
    pub(crate) fn new(
        pane: Pane,
        split_shell: ShellConfig,
        policy: TerminalPolicy,
        area: TerminalArea,
    ) -> Self {
        let id = pane.id;
        Self {
            tree: PaneTree::new(id),
            panes: HashMap::from([(id, pane)]),
            focused: id,
            primary: id,
            zoomed: None,
            navigation_memory: HashMap::new(),
            split_shell,
            policy,
            next_id: id.0.saturating_add(1),
            area,
        }
    }

    /// A one-leaf tree whose `cols` by `rows` shell runs in a `total_width`
    /// by `total_height` area.
    #[cfg(test)]
    pub fn for_test(
        cols: usize,
        rows: usize,
        shell: ShellConfig,
        scrollback_lines: usize,
        scrollback_bytes: usize,
        total_width: f32,
        total_height: f32,
        cell_width: f32,
        cell_height: f32,
    ) -> anyhow::Result<Self> {
        let geometry = TerminalGeometry::from_cells(cols, rows, cell_width, cell_height);
        let pane = Pane::new(
            PaneId(0),
            geometry,
            &shell,
            scrollback_lines,
            scrollback_bytes,
            None,
        )?;
        Ok(Self::new(
            pane,
            shell,
            TerminalPolicy::with_scrollback(scrollback_lines, scrollback_bytes),
            TerminalArea::from_available_pixels(total_width, total_height, cell_width, cell_height),
        ))
    }

    /// Continue the tab's pane ID history from `next_pane`, as a restored
    /// snapshot carrying large pane IDs would.
    #[cfg(test)]
    pub(crate) fn set_next_pane_id_for_test(&mut self, next_pane: u32) {
        self.next_id = next_pane;
    }

    #[cfg(test)]
    pub(crate) fn split_shell(&self) -> &ShellConfig {
        &self.split_shell
    }

    /// The tab's only pane, while it has just one.
    pub(crate) fn lone_pane(&self) -> Option<&Pane> {
        (self.panes.len() == 1)
            .then(|| self.panes.values().next())
            .flatten()
    }

    pub(crate) fn lone_pane_mut(&mut self) -> Option<&mut Pane> {
        (self.panes.len() == 1)
            .then(|| self.panes.values_mut().next())
            .flatten()
    }

    /// Rebuild a saved tab's layout around this one-leaf tree. Its terminal
    /// becomes the saved primary pane, keeping the scrollback and command
    /// the snapshot remembers; every other saved pane is relaunched fresh.
    /// Returns warnings about saved state that could not be used.
    pub(crate) fn restore_workspace_layout(
        &mut self,
        snapshot: &crate::workspace::WorkspaceTabSnapshot,
        pane_launches: &WorkspacePaneLaunches,
    ) -> anyhow::Result<Vec<String>> {
        snapshot.validate()?;
        anyhow::ensure!(
            self.panes.len() == 1,
            "a saved layout is restored only into a tab's first terminal"
        );
        let saved_primary = snapshot
            .panes
            .iter()
            .find(|pane| pane.id == snapshot.primary_pane)
            .context("workspace tab has no primary pane")?;
        let primary_id = PaneId(snapshot.primary_pane);
        let mut primary = self
            .panes
            .drain()
            .map(|(_, pane)| pane)
            .next()
            .context("workspace primary owner disappeared during restore")?;
        primary.id = primary_id;
        // A tab's only pane shows the tab's title rather than its own.
        if snapshot.panes.len() > 1 {
            primary.title.clone_from(&saved_primary.title);
        }
        primary.last_command.clone_from(&saved_primary.last_command);
        replay_workspace_scrollback(&mut primary.terminal, &saved_primary.scrollback_lines);
        self.panes.insert(primary_id, primary);
        self.tree = PaneTree::from_root(PaneNode::from(&snapshot.layout));
        self.focused = PaneId(snapshot.focused_pane);
        self.primary = primary_id;
        self.next_id = snapshot
            .panes
            .iter()
            .map(|pane| pane.id)
            .max()
            .unwrap_or(0)
            .saturating_add(1);
        self.recalculate_sizes();

        let mut warnings = Vec::new();
        let layout = self.calculate_layout();
        for pane_snapshot in snapshot
            .panes
            .iter()
            .filter(|pane| pane.id != snapshot.primary_pane)
        {
            let id = PaneId(pane_snapshot.id);
            let rect = layout
                .get(&id)
                .with_context(|| format!("workspace layout omits pane {}", pane_snapshot.id))?;
            let geometry = TerminalGeometry::from_available_pixels(
                rect.width,
                rect.height,
                self.area.cell_width,
                self.area.cell_height,
            );
            let cwd = usable_workspace_cwd(
                pane_snapshot.working_directory.as_deref(),
                snapshot.id,
                pane_snapshot.id,
                &mut warnings,
            );
            let (launch, shell) = pane_launches.get(&pane_snapshot.id).with_context(|| {
                format!(
                    "workspace tab {} pane {} has no resolved launch",
                    snapshot.id, pane_snapshot.id
                )
            })?;
            let mut pane = Pane::new_local(
                id,
                geometry,
                shell,
                &self.policy,
                cwd.as_deref(),
                matches!(launch, crate::workspace::WorkspaceLaunch::Plugin { .. }),
            )?;
            pane.launch.clone_from(launch);
            pane.launch_shell.clone_from(shell);
            pane.title.clone_from(&pane_snapshot.title);
            pane.last_command.clone_from(&pane_snapshot.last_command);
            replay_workspace_scrollback(&mut pane.terminal, &pane_snapshot.scrollback_lines);
            self.panes.insert(id, pane);
        }
        self.recalculate_sizes();
        Ok(warnings)
    }

    /// Lay a live-domain tab's panes out around this one-leaf tree, whose
    /// relay shows the remote pane with the lowest ID. Every other remote
    /// pane gets its own relay; local pane IDs number the remote panes in
    /// ascending order.
    pub(crate) fn adopt_domain_layout(
        &mut self,
        domain_id: &str,
        snapshot: &crate::domain::model::LiveTab,
        relay_command: &crate::domain::DomainRelayCommand,
    ) -> anyhow::Result<()> {
        anyhow::ensure!(
            self.panes.len() == 1 && self.panes.contains_key(&PaneId(0)),
            "a live-domain layout is adopted only by a tab's first relay"
        );
        anyhow::ensure!(
            snapshot.panes.len() <= MAX_PANES_PER_TAB,
            "a tab holds at most {MAX_PANES_PER_TAB} panes"
        );
        let mut remote_panes = snapshot.panes.clone();
        remote_panes.sort_by_key(|pane| pane.id);
        let primary = remote_panes
            .first()
            .context("live domain tab has no panes")?;
        let id_map: HashMap<u32, PaneId> = remote_panes
            .iter()
            .enumerate()
            .map(|(index, pane)| (pane.id, PaneId(index as u32)))
            .collect();
        self.tree = PaneTree::from_root(domain_layout_to_local(&snapshot.layout, &id_map)?);
        self.focused = *id_map
            .get(&snapshot.focused_pane)
            .context("live domain focus references a missing pane")?;
        self.next_id = remote_panes.len() as u32;
        self.recalculate_sizes();
        if remote_panes.len() > 1
            && let Some(pane) = self.panes.get_mut(&PaneId(0))
        {
            pane.title = Some(primary.title.clone());
        }

        let layout = self.calculate_layout();
        for remote in remote_panes.iter().skip(1) {
            let local_id = id_map[&remote.id];
            let rect = layout
                .get(&local_id)
                .with_context(|| format!("live domain layout omits pane {}", remote.id))?;
            let geometry = TerminalGeometry::from_available_pixels(
                rect.width,
                rect.height,
                self.area.cell_width,
                self.area.cell_height,
            );
            let pane = Pane::new_relay(
                local_id,
                geometry,
                relay_command,
                domain_id,
                remote.id,
                &self.policy,
                Some(remote.title.clone()),
            )?;
            self.panes.insert(local_id, pane);
        }
        self.recalculate_sizes();
        Ok(())
    }

    /// Reconcile a daemon-published tab snapshot without replacing relay PTYs
    /// that still represent the same stable remote panes. New remote panes get
    /// one relay; removed panes are dropped; the daemon remains authoritative
    /// for layout ratios and focus.
    pub(crate) fn reconcile_domain(
        &mut self,
        pane_ids: &mut HashMap<PaneId, u32>,
        domain_id: &str,
        snapshot: &crate::domain::model::LiveTopologyTab,
        relay_command: &crate::domain::DomainRelayCommand,
    ) -> anyhow::Result<()> {
        if snapshot.panes.is_empty() {
            anyhow::bail!("live domain tab has no panes");
        }
        anyhow::ensure!(
            snapshot.panes.len() <= MAX_PANES_PER_TAB,
            "a tab holds at most {MAX_PANES_PER_TAB} panes"
        );
        let remote_ids: std::collections::HashSet<u32> =
            snapshot.panes.iter().map(|pane| pane.id).collect();
        if remote_ids.len() != snapshot.panes.len() {
            anyhow::bail!("live domain tab contains duplicate pane identifiers");
        }

        let mut planned_ids: HashMap<PaneId, u32> = pane_ids
            .iter()
            .filter(|(local, remote)| remote_ids.contains(remote) && self.panes.contains_key(local))
            .map(|(local, remote)| (*local, *remote))
            .collect();
        let mut next_id = self.next_id;
        let mut additions = Vec::new();
        for remote in &snapshot.panes {
            if planned_ids.values().any(|existing| *existing == remote.id) {
                continue;
            }
            let local_id = PaneId(next_id);
            next_id = next_id.checked_add(1).context("pane ID space exhausted")?;
            // Laying the tree out below gives the relay its own share.
            let pane = Pane::new_relay(
                local_id,
                self.area.geometry,
                relay_command,
                domain_id,
                remote.id,
                &self.policy,
                Some(remote.title.clone()),
            )?;
            additions.push((local_id, pane));
            planned_ids.insert(local_id, remote.id);
        }

        let remote_to_local: HashMap<_, _> = planned_ids
            .iter()
            .map(|(local, remote)| (*remote, *local))
            .collect();
        let tree = PaneTree::from_root(domain_layout_to_local(&snapshot.layout, &remote_to_local)?);
        let layout_leaves = tree.get_all_ids();
        let layout_ids: std::collections::HashSet<_> = layout_leaves.iter().copied().collect();
        let planned_locals: std::collections::HashSet<_> = planned_ids.keys().copied().collect();
        if layout_ids != planned_locals || layout_leaves.len() != planned_locals.len() {
            anyhow::bail!("live domain layout does not contain every pane exactly once");
        }
        let focused = *remote_to_local
            .get(&snapshot.focused_pane)
            .context("live domain focus references a missing pane")?;

        self.panes.retain(|local, _| planned_locals.contains(local));
        self.panes.extend(additions);
        *pane_ids = planned_ids;
        self.next_id = next_id;
        self.tree = tree;
        self.focused = focused;
        self.zoomed = self.zoomed.filter(|pane| self.panes.contains_key(pane));
        self.navigation_memory.clear();
        for remote in &snapshot.panes {
            if let Some(local) = remote_to_local.get(&remote.id)
                && let Some(pane) = self.panes.get_mut(local)
            {
                pane.title = Some(remote.title.clone());
            }
        }
        self.settle_lone_pane();
        self.recalculate_sizes();
        Ok(())
    }

    /// Get the focused pane
    pub fn focused_pane(&self) -> Option<&Pane> {
        self.panes.get(&self.focused)
    }

    /// Get the focused pane mutably
    pub fn focused_pane_mut(&mut self) -> Option<&mut Pane> {
        self.panes.get_mut(&self.focused)
    }

    /// Get the focused pane ID
    pub fn focused_id(&self) -> PaneId {
        self.focused
    }

    pub fn zoomed_id(&self) -> Option<PaneId> {
        self.zoomed
    }

    pub fn pane_ids(&self) -> Vec<PaneId> {
        self.tree.get_all_ids()
    }

    pub fn primary_id(&self) -> PaneId {
        if self.panes.contains_key(&self.primary) {
            self.primary
        } else if self.panes.contains_key(&PaneId(0)) {
            PaneId(0)
        } else {
            self.tree
                .get_all_ids()
                .into_iter()
                .next()
                .expect("a pane manager always has a primary leaf")
        }
    }

    pub(crate) fn workspace_layout(&self) -> crate::workspace::WorkspacePaneLayout {
        crate::workspace::WorkspacePaneLayout::from(self.tree.root())
    }

    pub(crate) fn workspace_panes(
        &self,
        tab_title: &str,
        save_scrollback: bool,
    ) -> Vec<crate::workspace::WorkspacePaneSnapshot> {
        let primary_id = self.primary_id();
        let mut panes: Vec<_> = self
            .panes
            .values()
            .map(|pane| {
                let working_directory = pane.working_directory();
                crate::workspace::WorkspacePaneSnapshot {
                    id: pane.id.0,
                    launch: pane.launch.clone(),
                    title: pane
                        .title
                        .as_deref()
                        .or((pane.id == primary_id).then_some(tab_title))
                        .map(crate::workspace::bounded_workspace_title),
                    working_directory,
                    last_command: pane
                        .pty
                        .child_id()
                        .and_then(crate::terminal::process::get_foreground_process)
                        .or_else(|| pane.last_command.clone()),
                    scrollback_file: None,
                    scrollback_truncated: false,
                    scrollback_lines: if save_scrollback {
                        crate::workspace::restorable_scrollback(pane.terminal.grid())
                    } else {
                        Vec::new()
                    },
                }
            })
            .collect();
        panes.sort_by_key(|pane| pane.id);
        panes
    }

    /// Set focus to a specific pane
    pub fn focus(&mut self, id: PaneId) {
        if self.panes.contains_key(&id) {
            self.focused = id;
            if self.zoomed.is_some() {
                self.zoomed = Some(id);
                self.recalculate_sizes();
            }
        }
    }

    /// Get a pane by ID
    pub fn get_pane(&self, id: PaneId) -> Option<&Pane> {
        self.panes.get(&id)
    }

    /// Get a pane by ID mutably
    pub fn get_pane_mut(&mut self, id: PaneId) -> Option<&mut Pane> {
        self.panes.get_mut(&id)
    }

    /// Apply host settings to every pane and to panes created later.
    /// `apply_keyboard_type` is as in [`TerminalPolicy::apply`].
    pub(crate) fn apply_terminal_policy(
        &mut self,
        policy: TerminalPolicy,
        apply_keyboard_type: bool,
    ) {
        self.policy = policy;
        for pane in self.panes.values_mut() {
            policy.apply(&mut pane.terminal, apply_keyboard_type);
        }
    }

    /// Prospective geometry uses the unzoomed tree, exactly as splitting does.
    pub(crate) fn split_geometry(
        &self,
        target: PaneId,
        direction: SplitDirection,
    ) -> anyhow::Result<TerminalGeometry> {
        let layout = self.tree.calculate_layout(self.root_rect());
        let rect = layout.get(&target).context("split target is missing")?;
        Ok(Self::child_geometry(
            rect.width,
            rect.height,
            self.area.cell_width,
            self.area.cell_height,
            direction,
        ))
    }

    fn child_geometry(
        width: f32,
        height: f32,
        cell_width: f32,
        cell_height: f32,
        direction: SplitDirection,
    ) -> TerminalGeometry {
        let (width, height) = match direction {
            SplitDirection::Horizontal => (width, (height - PANE_DIVIDER_THICKNESS).max(0.0) / 2.0),
            SplitDirection::Vertical => ((width - PANE_DIVIDER_THICKNESS).max(0.0) / 2.0, height),
        };
        TerminalGeometry::from_available_pixels(width, height, cell_width, cell_height)
    }

    /// Split the focused pane, starting the new shell in the directory
    /// `cwd_mode` picks.
    pub fn split(
        &mut self,
        direction: SplitDirection,
        cwd_mode: PaneCwdMode,
    ) -> anyhow::Result<PaneId> {
        self.split_local(direction, SplitLaunch::Shell(cwd_mode))
    }

    fn split_local(
        &mut self,
        direction: SplitDirection,
        launch: SplitLaunch<'_>,
    ) -> anyhow::Result<PaneId> {
        self.ensure_room_for_pane()?;
        let next_id = self
            .next_id
            .checked_add(1)
            .context("pane ID space exhausted")?;
        let was_zoomed = self.zoomed.take();
        let previous_focus = self.focused;
        let new_id = PaneId(self.next_id);
        self.next_id = next_id;

        let geometry = self
            .split_geometry(self.focused, direction)
            .expect("validated focused split target");
        let (shell, cwd, identity) = match launch {
            SplitLaunch::Shell(cwd_mode) => {
                let pane_cwd = |id: PaneId| self.panes.get(&id).and_then(Pane::working_directory);
                let cwd =
                    cwd_mode.resolve(|| pane_cwd(self.focused), || pane_cwd(self.primary_id()));
                (&self.split_shell, cwd, self.split_launch())
            }
            SplitLaunch::Given(launch) => {
                (&launch.shell, launch.cwd.clone(), launch.identity.clone())
            }
        };
        let plugin = matches!(identity, crate::workspace::WorkspaceLaunch::Plugin { .. });
        let mut pane = match Pane::new_local(
            new_id,
            geometry,
            shell,
            &self.policy,
            cwd.as_deref(),
            plugin,
        ) {
            Ok(pane) => pane,
            Err(error) => {
                self.next_id = new_id.0;
                self.zoomed = was_zoomed;
                return Err(error);
            }
        };
        pane.launch = identity;
        self.panes.insert(new_id, pane);

        // Split the tree
        assert!(
            self.tree.split(self.focused, new_id, direction),
            "validated focused pane remains splittable"
        );

        // Focus the new pane
        self.focused = new_id;
        let direction = match direction {
            SplitDirection::Vertical => PaneDir::Right,
            SplitDirection::Horizontal => PaneDir::Down,
        };
        self.navigation_memory
            .insert((previous_focus, direction), new_id);
        self.navigation_memory
            .insert((new_id, direction.opposite()), previous_focus);

        // Recalculate all pane sizes
        self.recalculate_sizes();

        log::info!(
            "Split pane {:?} {:?}, created pane {:?}",
            self.focused,
            direction,
            new_id
        );
        Ok(new_id)
    }

    /// Split one exact stable pane ID without requiring callers to change
    /// focus first. A failed child launch restores the prior focus and zoom
    /// state; success follows ordinary split behavior and focuses the new pane.
    pub(crate) fn split_target(
        &mut self,
        target: PaneId,
        direction: SplitDirection,
        cwd_mode: PaneCwdMode,
    ) -> anyhow::Result<PaneId> {
        self.split_target_local(target, direction, SplitLaunch::Shell(cwd_mode))
    }

    pub(crate) fn split_target_with_launch(
        &mut self,
        target: PaneId,
        direction: SplitDirection,
        launch: &LocalPaneLaunch,
    ) -> anyhow::Result<PaneId> {
        self.split_target_local(target, direction, SplitLaunch::Given(launch))
    }

    fn split_target_local(
        &mut self,
        target: PaneId,
        direction: SplitDirection,
        launch: SplitLaunch<'_>,
    ) -> anyhow::Result<PaneId> {
        if !self.panes.contains_key(&target) {
            anyhow::bail!("pane {} does not exist", target.0);
        }
        let previous_focus = self.focused;
        self.focused = target;
        match self.split_local(direction, launch) {
            Ok(created) => Ok(created),
            Err(error) => {
                self.focused = previous_focus;
                Err(error)
            }
        }
    }

    /// Close the focused pane
    pub fn close_focused(&mut self) -> bool {
        self.close(self.focused)
    }

    /// Close one stable local pane. Domain reconciliation uses this instead of
    /// current focus because the daemon response may arrive after focus moves.
    pub(crate) fn close(&mut self, to_close: PaneId) -> bool {
        if self.panes.len() <= 1 {
            return false; // Can't close the last pane
        }
        if !self.panes.contains_key(&to_close) {
            return false;
        }

        // Closing a background pane must not steal focus. If the focused pane
        // is the target, choose its nearest sibling before removing it.
        if self.focused == to_close {
            if let Some(sibling) = self.tree.find_sibling(to_close) {
                self.focused = sibling;
            } else if let Some(&id) = self.panes.keys().find(|&&id| id != to_close) {
                self.focused = id;
            } else {
                return false;
            }
        }

        // Remove from tree and panes. Dropping the receiver makes a blocked
        // bounded reader send fail, so close deliberately discards any output
        // that races after this user-requested teardown instead of leaking a
        // detached queue/thread.
        self.tree.remove(to_close);
        self.panes.remove(&to_close);
        if self.zoomed == Some(to_close) {
            self.zoomed = None;
        }
        self.navigation_memory
            .retain(|(from, _), to| *from != to_close && *to != to_close);
        self.settle_lone_pane();

        // Recalculate sizes
        self.recalculate_sizes();

        log::info!("Closed pane {:?}, focused {:?}", to_close, self.focused);
        true
    }

    pub(crate) fn contains(&self, id: PaneId) -> bool {
        self.panes.contains_key(&id)
    }

    /// Navigate to the nearest adjacent pane in `dir`. Returns true if
    /// focus actually moved — the caller can use the bool to give the
    /// user a cue when they hit the edge of the layout.
    pub fn navigate(&mut self, dir: PaneDir) -> bool {
        let layout = self.calculate_layout();
        if !layout.contains_key(&self.focused) {
            return false;
        }

        let remembered = self.navigation_memory.get(&(self.focused, dir)).copied();
        let target = navigation_target(&layout, self.focused, dir, remembered);

        match target {
            Some(id) => {
                let previous = self.focused;
                self.focused = id;
                self.navigation_memory.insert((previous, dir), id);
                self.navigation_memory
                    .insert((id, dir.opposite()), previous);
                true
            }
            None => false,
        }
    }

    /// Poll every pane, including unfocused panes, through the same bounded
    /// event path.
    pub fn poll_events(&mut self) -> PanePoll {
        let mut poll = PanePoll::default();
        for pane in self.panes.values_mut() {
            let pane_poll = pane.poll_events(self.policy.agent_features);
            poll.changed |= pane_poll.changed;
            poll.saw_output |= pane_poll.saw_output;
            poll.events.extend(pane_poll.events);
        }
        poll
    }

    /// Whether the panes are already laid out for `area`.
    pub(crate) fn is_laid_out_for(&self, area: TerminalArea) -> bool {
        self.area == area
    }

    /// Lay the panes out in a new terminal area.
    pub(crate) fn resize(&mut self, area: TerminalArea) {
        self.area = area;
        self.recalculate_sizes();
    }

    /// The whole pane area, in pane-local coordinates.
    fn root_rect(&self) -> Bounds {
        Bounds {
            x: 0.0,
            y: 0.0,
            width: self.area.width(),
            height: self.area.height(),
        }
    }

    /// Calculate the layout for all panes
    pub fn calculate_layout(&self) -> HashMap<PaneId, Bounds> {
        let root_rect = self.root_rect();
        if let Some(id) = self.zoomed.filter(|id| self.panes.contains_key(id)) {
            return HashMap::from([(id, root_rect)]);
        }
        self.tree.calculate_layout(root_rect)
    }

    /// Recalculate and apply sizes to all panes
    fn recalculate_sizes(&mut self) {
        // A tree of one leaf fills the area, so its terminal takes the area's
        // own geometry rather than one re-derived from its pixel extent.
        if let PaneNode::Leaf(id) = self.tree.root() {
            if let Some(pane) = self.panes.get_mut(id)
                && let Err(e) = pane.resize_geometry(self.area.geometry)
            {
                log::error!("Failed to resize pane {:?}: {}", id, e);
            }
            return;
        }
        let layout = self.calculate_layout();

        for (id, rect) in layout {
            if let Some(pane) = self.panes.get_mut(&id) {
                let geometry = TerminalGeometry::from_available_pixels(
                    rect.width,
                    rect.height,
                    self.area.cell_width,
                    self.area.cell_height,
                );
                if let Err(e) = pane.resize_geometry(geometry) {
                    log::error!("Failed to resize pane {:?}: {}", id, e);
                }
            }
        }
    }

    /// Get dividers for rendering
    pub fn get_dividers(&self) -> Vec<Divider> {
        if self.zoomed.is_some() {
            return Vec::new();
        }
        self.tree.get_dividers(self.root_rect())
    }

    /// Resize the exact divider selected at mouse-down. This prevents a nested
    /// drag from jumping to the root divider once the pointer moves away from
    /// the original hit target.
    pub fn resize_divider_target(
        &mut self,
        target: DividerTarget,
        pos_x: f32,
        pos_y: f32,
    ) -> Option<u16> {
        if self.zoomed.is_some() {
            return None;
        }
        let ratio = self
            .tree
            .resize_target(self.root_rect(), pos_x, pos_y, target)?;
        self.recalculate_sizes();
        Some(ratio)
    }

    /// Resize one exact tab-local divider to a protocol-thousandths ratio.
    /// Unlike pointer coordinates, the stable ID cannot drift to a sibling.
    pub(crate) fn resize_local_divider(
        &mut self,
        divider_id: u64,
        ratio_millis: u16,
    ) -> anyhow::Result<(u16, bool)> {
        if self.zoomed.is_some() {
            anyhow::bail!("Cannot resize a divider while the tab is zoomed");
        }
        if !termeric_core::split::RATIO_MILLIS.contains(&ratio_millis) {
            anyhow::bail!("Divider ratio must be between 100 and 900 thousandths");
        }
        let previous = self
            .tree
            .set_target_ratio(DividerTarget::Local(divider_id), ratio_millis)
            .with_context(|| format!("GUI divider {divider_id} does not exist"))?;
        let changed = previous != ratio_millis;
        if changed {
            self.recalculate_sizes();
        }
        Ok((previous, changed))
    }

    /// The divider within [`PANE_DIVIDER_HIT_MARGIN`] of a point, along
    /// its length. A zoomed tab shows no dividers, so has none to hit.
    pub fn divider_at(&self, pos_x: f32, pos_y: f32) -> Option<Divider> {
        self.get_dividers().into_iter().find(|divider| {
            if divider.vertical {
                (pos_x - divider.position).abs() < PANE_DIVIDER_HIT_MARGIN
                    && pos_y >= divider.start
                    && pos_y <= divider.end
            } else {
                (pos_y - divider.position).abs() < PANE_DIVIDER_HIT_MARGIN
                    && pos_x >= divider.start
                    && pos_x <= divider.end
            }
        })
    }

    /// Get number of panes
    pub fn pane_count(&self) -> usize {
        self.panes.len()
    }

    /// Count panes that still own a live presentation. Closed records remain
    /// briefly until lifecycle cleanup and must not satisfy transfer safety.
    pub(crate) fn open_pane_count(&self) -> usize {
        self.panes.values().filter(|pane| !pane.closed).count()
    }

    /// Toggle view-only zoom for the focused pane.
    pub fn toggle_zoom(&mut self) -> bool {
        if self.panes.len() <= 1 {
            return false;
        }
        self.zoomed = if self.zoomed.is_some() {
            None
        } else {
            Some(self.focused)
        };
        self.recalculate_sizes();
        true
    }

    /// Swap the focused pane with the next leaf in visual tree order.
    pub fn swap_focused_next(&mut self) -> bool {
        let ids = self.tree.get_all_ids();
        if ids.len() <= 1 {
            return false;
        }
        let Some(index) = ids.iter().position(|id| *id == self.focused) else {
            return false;
        };
        let other = ids[(index + 1) % ids.len()];
        let swapped = self.tree.swap(self.focused, other);
        if swapped {
            self.navigation_memory.clear();
            self.recalculate_sizes();
        }
        swapped
    }

    /// Swap two exact live pane identities within the existing split tree.
    /// This preserves PTYs, focus, zoom ownership, and divider geometry.
    pub(crate) fn swap_exact(&mut self, first: PaneId, second: PaneId) -> anyhow::Result<()> {
        self.validate_complete_live_layout()?;
        if first == second {
            anyhow::bail!("Pane swap requires two distinct pane IDs");
        }
        for pane_id in [first, second] {
            let pane = self
                .panes
                .get(&pane_id)
                .with_context(|| format!("GUI pane {} does not exist", pane_id.0))?;
            if pane.closed {
                anyhow::bail!("GUI pane {} is closed", pane_id.0);
            }
        }
        if !self.tree.swap(first, second) {
            anyhow::bail!("Pane swap target disappeared from the layout tree");
        }
        self.navigation_memory.clear();
        self.recalculate_sizes();
        Ok(())
    }

    /// Reset every divider ratio in one complete live layout. The returned
    /// flag distinguishes an already-balanced no-op from a geometry change.
    pub(crate) fn balance_exact(&mut self) -> anyhow::Result<bool> {
        self.validate_complete_live_layout()?;
        let changed = self
            .tree
            .get_dividers(self.root_rect())
            .iter()
            .any(|divider| divider.ratio_millis != 500);
        if changed {
            self.rebalance();
        }
        Ok(changed)
    }

    /// Rebuild one complete live layout from its current visual stable-ID
    /// order. Presets deliberately clear zoom because the geometry is replaced.
    pub(crate) fn apply_layout_preset_exact(
        &mut self,
        preset: PaneLayoutPreset,
    ) -> anyhow::Result<()> {
        self.validate_complete_live_layout()?;
        assert!(
            self.rebuild_with_preset(preset),
            "a preset applies to a validated layout of two or more panes"
        );
        Ok(())
    }

    fn validate_complete_live_layout(&self) -> anyhow::Result<()> {
        if self.open_pane_count() != self.pane_count() {
            anyhow::bail!("Pane layout contains closed pane records awaiting cleanup");
        }
        if self.open_pane_count() < 2 {
            anyhow::bail!("Pane layout requires at least two live panes");
        }
        Ok(())
    }

    /// Balance every divider. Unlike [`Self::balance_exact`], this counts
    /// panes awaiting cleanup and reports only whether there was a split.
    pub fn balance(&mut self) -> bool {
        if self.panes.len() <= 1 {
            return false;
        }
        self.rebalance();
        true
    }

    /// Apply a layout preset. Unlike [`Self::apply_layout_preset_exact`], this
    /// counts panes awaiting cleanup and reports failure as `false`.
    pub fn apply_layout_preset(&mut self, preset: PaneLayoutPreset) -> bool {
        self.panes.len() > 1 && self.rebuild_with_preset(preset)
    }

    fn rebalance(&mut self) {
        self.tree.balance();
        self.recalculate_sizes();
    }

    /// Presets clear zoom because they replace the geometry.
    fn rebuild_with_preset(&mut self, preset: PaneLayoutPreset) -> bool {
        if !self.tree.apply_layout_preset(preset, self.focused) {
            return false;
        }
        self.zoomed = None;
        self.navigation_memory.clear();
        self.recalculate_sizes();
        true
    }

    /// Remove the focused pane without dropping its PTY. The last pane cannot
    /// be detached because that would leave an invalid tab owner.
    pub(crate) fn detach_focused(&mut self) -> Option<Pane> {
        self.detach(self.focused)
    }

    /// Remove an identified pane without dropping its PTY. This supports an
    /// authoritative live-domain handoff even when another attached window
    /// currently has a different local focus.
    pub(crate) fn detach(&mut self, detached: PaneId) -> Option<Pane> {
        let detached_is_open = !self.panes.get(&detached)?.closed;
        if self.panes.len() <= 1 || (detached_is_open && self.open_pane_count() <= 1) {
            return None;
        }
        if self.focused == detached {
            self.focused = self
                .tree
                .find_sibling(detached)
                .filter(|id| self.panes.get(id).is_some_and(|pane| !pane.closed))
                .or_else(|| {
                    self.panes
                        .iter()
                        .find_map(|(id, pane)| (*id != detached && !pane.closed).then_some(*id))
                })?;
        }
        self.tree.remove(detached);
        self.zoomed = None;
        self.navigation_memory
            .retain(|(from, _), to| *from != detached && *to != detached);
        let pane = self.panes.remove(&detached)?;
        self.settle_lone_pane();
        self.recalculate_sizes();
        Some(pane)
    }

    /// Fails when the tab already holds [`MAX_PANES_PER_TAB`] panes. Every
    /// way a pane is added asks this first.
    pub(crate) fn ensure_room_for_pane(&self) -> anyhow::Result<()> {
        anyhow::ensure!(
            self.panes.len() < MAX_PANES_PER_TAB,
            "a tab holds at most {MAX_PANES_PER_TAB} panes"
        );
        Ok(())
    }

    /// Validate an exact destination anchor and stable-ID allocation without
    /// mutating the layout. Call this before detaching a pane from its source.
    pub(crate) fn validate_attach_target(&self, target: PaneId) -> anyhow::Result<()> {
        let target_pane = self
            .panes
            .get(&target)
            .with_context(|| format!("destination pane {} does not exist", target.0))?;
        if target_pane.closed {
            anyhow::bail!("destination pane {} is closed", target.0);
        }
        self.ensure_room_for_pane()?;
        if self.next_id.checked_add(1).is_none() || self.panes.contains_key(&PaneId(self.next_id)) {
            anyhow::bail!("destination tab exhausted its stable pane ID space");
        }
        Ok(())
    }

    /// Attach a detached owner beside an exact stable destination pane,
    /// under a new tab-local ID. No process or reader thread is recreated.
    pub(crate) fn attach_target(
        &mut self,
        mut pane: Pane,
        target: PaneId,
        direction: SplitDirection,
    ) -> anyhow::Result<PaneId> {
        self.validate_attach_target(target)?;
        let id = PaneId(self.next_id);
        self.next_id = self
            .next_id
            .checked_add(1)
            .context("destination tab exhausted its stable pane ID space")?;
        pane.id = id;
        // A pane moved here runs under this tab's host settings.
        self.policy.apply(&mut pane.terminal, false);
        self.panes.insert(id, pane);
        assert!(
            self.tree.split(target, id, direction),
            "a validated target pane is splittable"
        );
        self.focused = id;
        self.zoomed = None;
        self.navigation_memory.clear();
        self.recalculate_sizes();
        Ok(id)
    }

    pub fn write_to(&mut self, id: PaneId, data: &[u8]) -> io::Result<()> {
        self.panes
            .get_mut(&id)
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "pane no longer exists"))?
            .write(data)
    }

    /// Settle a tree that has come down to one pane: it is the tab's primary
    /// and focused pane, and unzoomed. As the tab's only pane it shows the
    /// tab's title, so it keeps none of its own.
    fn settle_lone_pane(&mut self) {
        if self.panes.len() != 1 {
            return;
        }
        let Some((&id, pane)) = self.panes.iter_mut().next() else {
            return;
        };
        pane.title = None;
        self.primary = id;
        self.focused = id;
        self.zoomed = None;
        self.navigation_memory.clear();
    }

    /// How a split launches when it is given no launch of its own: as the
    /// primary pane's terminal profile when it has one, else as a shell.
    fn split_launch(&self) -> crate::workspace::WorkspaceLaunch {
        match self.panes.get(&self.primary_id()).map(|pane| &pane.launch) {
            Some(crate::workspace::WorkspaceLaunch::Profile { name }) => {
                crate::workspace::WorkspaceLaunch::Profile { name: name.clone() }
            }
            _ => crate::workspace::WorkspaceLaunch::Shell,
        }
    }

    /// Have later splits launch `shell`, as the tab's primary pane now does.
    pub(crate) fn adopt_primary_shell(&mut self, shell: ShellConfig) {
        self.split_shell = shell;
    }

    /// Iterate over all panes with their layouts
    pub fn iter_with_layout(&self) -> impl Iterator<Item = (&Pane, Bounds)> {
        let layout = self.calculate_layout();
        self.panes
            .values()
            .filter_map(move |pane| layout.get(&pane.id).map(|&rect| (pane, rect)))
    }

    pub(crate) fn iter_panes(&self) -> impl ExactSizeIterator<Item = &Pane> {
        self.panes.values()
    }

    pub fn iter_panes_identified(&self) -> Vec<(PaneId, &Terminal, Bounds, bool)> {
        let layout = self.calculate_layout();
        self.panes
            .values()
            .filter_map(|pane| {
                layout
                    .get(&pane.id)
                    .map(|&rect| (pane.id, &pane.terminal, rect, pane.id == self.focused))
            })
            .collect()
    }

    pub fn all_panes_mut(&mut self) -> impl Iterator<Item = &mut Pane> {
        self.panes.values_mut()
    }

    /// Remove closed panes and resize remaining panes to fill the space.
    ///
    /// Returns true if panes remain, false if the map is now empty.
    pub fn cleanup_closed(&mut self) -> bool {
        let closed: Vec<PaneId> = self
            .panes
            .iter()
            .filter(|(_, pane)| pane.closed)
            .map(|(&id, _)| id)
            .collect();

        let mut removed_any = false;
        for id in closed {
            if self.panes.len() > 1 {
                if id == self.focused {
                    // Focus another pane first
                    if let Some(&new_focus) = self.panes.keys().find(|&&pid| pid != id) {
                        self.focused = new_focus;
                    }
                }
                self.tree.remove(id);
                self.panes.remove(&id);
                if self.zoomed == Some(id) {
                    self.zoomed = None;
                }
                self.navigation_memory
                    .retain(|(from, _), to| *from != id && *to != id);
                removed_any = true;
            }
        }

        // Resize remaining panes to fill the space vacated by closed panes
        if removed_any {
            self.settle_lone_pane();
            self.recalculate_sizes();
        }

        !self.panes.is_empty()
    }

    /// Check if all panes are closed
    pub fn all_closed(&self) -> bool {
        self.panes.is_empty() || self.panes.values().all(|p| p.closed)
    }
}

/// A divider between panes
#[derive(Debug, Clone, Copy)]
pub struct Divider {
    /// Start position (x for vertical, y for horizontal)
    pub start: f32,
    /// End position
    pub end: f32,
    /// Position perpendicular to orientation
    pub position: f32,
    /// Whether this is a vertical divider
    pub vertical: bool,
    /// Current first-child ratio in protocol thousandths.
    pub ratio_millis: u16,
    /// Stable target used for the lifetime of an active drag.
    pub target: DividerTarget,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DividerTarget {
    Local(u64),
    Domain(u32),
}

/// Pick a directional neighbor with stable geometric ordering. A remembered
/// target wins when it is still a valid neighbor; otherwise ambiguous T-split
/// ties resolve top-to-bottom or left-to-right, then by pane ID.
fn navigation_target(
    layout: &HashMap<PaneId, Bounds>,
    current_id: PaneId,
    dir: PaneDir,
    remembered: Option<PaneId>,
) -> Option<PaneId> {
    let current = layout.get(&current_id)?;
    let is_neighbor = |rect: &Bounds| match dir {
        PaneDir::Right => {
            rect.x >= current.right() - 1.0 && rects_overlap_vertically(current, rect)
        }
        PaneDir::Left => rect.right() <= current.x + 1.0 && rects_overlap_vertically(current, rect),
        PaneDir::Down => {
            rect.y >= current.bottom() - 1.0 && rects_overlap_horizontally(current, rect)
        }
        PaneDir::Up => {
            rect.bottom() <= current.y + 1.0 && rects_overlap_horizontally(current, rect)
        }
    };

    if let Some(id) = remembered
        && id != current_id
        && layout.get(&id).is_some_and(&is_neighbor)
    {
        return Some(id);
    }

    let primary_distance = |rect: &Bounds| match dir {
        PaneDir::Right => rect.x - current.right(),
        PaneDir::Left => current.x - rect.right(),
        PaneDir::Down => rect.y - current.bottom(),
        PaneDir::Up => current.y - rect.bottom(),
    };
    let cross_distance = |rect: &Bounds| match dir {
        PaneDir::Right | PaneDir::Left => (rect.center()[1] - current.center()[1]).abs(),
        PaneDir::Up | PaneDir::Down => (rect.center()[0] - current.center()[0]).abs(),
    };
    let cross_position = |rect: &Bounds| match dir {
        PaneDir::Right | PaneDir::Left => rect.y,
        PaneDir::Up | PaneDir::Down => rect.x,
    };

    layout
        .iter()
        .filter(|(id, rect)| **id != current_id && is_neighbor(rect))
        .min_by(|(id_a, a), (id_b, b)| {
            primary_distance(a)
                .total_cmp(&primary_distance(b))
                .then_with(|| cross_distance(a).total_cmp(&cross_distance(b)))
                .then_with(|| cross_position(a).total_cmp(&cross_position(b)))
                .then_with(|| id_a.0.cmp(&id_b.0))
        })
        .map(|(id, _)| *id)
}

/// Check if two rectangles overlap vertically
fn rects_overlap_vertically(a: &Bounds, b: &Bounds) -> bool {
    a.y < b.y + b.height && a.y + a.height > b.y
}

/// Check if two rectangles overlap horizontally
fn rects_overlap_horizontally(a: &Bounds, b: &Bounds) -> bool {
    a.x < b.x + b.width && a.x + a.width > b.x
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::{BTreeSet, HashSet};
    use std::time::{Duration, Instant};

    fn property_manager(panes: usize) -> PaneManager {
        let shell = crate::test_support::sleeping_shell();
        let mut manager = PaneManager::for_test(
            120,
            40,
            shell,
            1_000,
            8 * 1024 * 1024,
            1_200.0,
            800.0,
            10.0,
            20.0,
        )
        .unwrap();
        for index in 1..panes {
            let direction = if index % 2 == 0 {
                SplitDirection::Horizontal
            } else {
                SplitDirection::Vertical
            };
            manager.split(direction, PaneCwdMode::Focused).unwrap();
        }
        manager
    }

    fn assert_manager_invariants(manager: &PaneManager) {
        let ids = manager.pane_ids();
        let unique: HashSet<_> = ids.iter().copied().collect();
        assert_eq!(ids.len(), manager.pane_count());
        assert_eq!(unique.len(), manager.pane_count());
        assert!(unique.contains(&manager.focused_id()));
        assert!(ids.iter().all(|id| manager.get_pane(*id).is_some()));

        let layout = manager.calculate_layout();
        if let Some(zoomed) = manager.zoomed_id() {
            assert_eq!(zoomed, manager.focused_id());
            assert_eq!(
                layout.keys().copied().collect::<HashSet<_>>(),
                HashSet::from([zoomed])
            );
            assert!(manager.get_dividers().is_empty());
        } else {
            assert_eq!(layout.keys().copied().collect::<HashSet<_>>(), unique);
            assert_eq!(manager.get_dividers().len(), manager.pane_count() - 1);
        }
    }

    #[test]
    fn exact_layout_primitives_preserve_owners_focus_and_atomicity() {
        let mut manager = property_manager(4);
        let original = manager.pane_ids();
        let child_ids: HashMap<_, _> = original
            .iter()
            .map(|id| (*id, manager.get_pane(*id).unwrap().pty.child_id()))
            .collect();
        let focused = original[1];
        manager.focus(focused);
        assert!(manager.toggle_zoom());

        manager.swap_exact(original[0], original[3]).unwrap();
        let mut expected = original.clone();
        expected.swap(0, 3);
        assert_eq!(manager.pane_ids(), expected);
        assert_eq!(manager.focused_id(), focused);
        assert_eq!(manager.zoomed_id(), Some(focused));
        for (id, child_id) in &child_ids {
            assert_eq!(manager.get_pane(*id).unwrap().pty.child_id(), *child_id);
        }

        assert!(manager.toggle_zoom());
        let target = manager.get_dividers()[0].target;
        let DividerTarget::Local(divider_id) = target else {
            panic!("ordinary layout unexpectedly used a domain divider")
        };
        manager.resize_local_divider(divider_id, 700).unwrap();
        assert!(manager.toggle_zoom());
        assert!(manager.balance_exact().unwrap());
        assert_eq!(manager.zoomed_id(), Some(focused));
        assert!(manager.toggle_zoom());
        assert!(
            manager
                .get_dividers()
                .iter()
                .all(|divider| divider.ratio_millis == 500)
        );
        assert!(!manager.balance_exact().unwrap());
        assert_eq!(manager.pane_ids(), expected);
        assert_eq!(manager.focused_id(), focused);

        assert!(manager.toggle_zoom());
        manager
            .apply_layout_preset_exact(PaneLayoutPreset::Columns)
            .unwrap();
        assert_eq!(manager.pane_ids(), expected);
        assert_eq!(manager.focused_id(), focused);
        assert_eq!(manager.zoomed_id(), None);
        assert!(
            manager
                .get_dividers()
                .iter()
                .all(|divider| divider.vertical)
        );

        manager
            .apply_layout_preset_exact(PaneLayoutPreset::Rows)
            .unwrap();
        assert!(
            manager
                .get_dividers()
                .iter()
                .all(|divider| !divider.vertical)
        );
        manager
            .apply_layout_preset_exact(PaneLayoutPreset::Grid)
            .unwrap();
        let dividers = manager.get_dividers();
        assert!(dividers.iter().any(|divider| divider.vertical));
        assert!(dividers.iter().any(|divider| !divider.vertical));

        manager
            .apply_layout_preset_exact(PaneLayoutPreset::Tall)
            .unwrap();
        let details = expected
            .iter()
            .copied()
            .filter(|id| *id != focused)
            .collect::<Vec<_>>();
        assert_eq!(
            manager.pane_ids(),
            [vec![focused], details.clone()].concat()
        );
        let layout = manager.calculate_layout();
        let master = layout[&focused];
        assert!(details.iter().all(|id| layout[id].x > master.x));
        assert!(
            details
                .windows(2)
                .all(|pair| layout[&pair[0]].y < layout[&pair[1]].y)
        );
        assert!(details.iter().all(|id| {
            (layout[id].height - layout[&details[0]].height).abs() <= PANE_DIVIDER_THICKNESS
        }));
        let dividers = manager.get_dividers();
        assert!(dividers[0].vertical);
        assert_eq!(dividers[0].ratio_millis, 500);
        assert!(dividers[1..].iter().all(|divider| !divider.vertical));

        manager
            .apply_layout_preset_exact(PaneLayoutPreset::Fat)
            .unwrap();
        assert_eq!(
            manager.pane_ids(),
            [vec![focused], details.clone()].concat()
        );
        let layout = manager.calculate_layout();
        let master = layout[&focused];
        assert!(details.iter().all(|id| layout[id].y > master.y));
        assert!(
            details
                .windows(2)
                .all(|pair| layout[&pair[0]].x < layout[&pair[1]].x)
        );
        assert!(details.iter().all(|id| {
            (layout[id].width - layout[&details[0]].width).abs() <= PANE_DIVIDER_THICKNESS
        }));
        let dividers = manager.get_dividers();
        assert!(!dividers[0].vertical);
        assert_eq!(dividers[0].ratio_millis, 500);
        assert!(dividers[1..].iter().all(|divider| divider.vertical));
        assert_eq!(manager.focused_id(), focused);
        for (id, child_id) in &child_ids {
            assert_eq!(manager.get_pane(*id).unwrap().pty.child_id(), *child_id);
        }

        let before = manager.pane_ids();
        manager.get_pane_mut(focused).unwrap().closed = true;
        assert!(
            manager
                .balance_exact()
                .unwrap_err()
                .to_string()
                .contains("closed")
        );
        assert!(
            manager
                .apply_layout_preset_exact(PaneLayoutPreset::Columns)
                .unwrap_err()
                .to_string()
                .contains("closed")
        );
        assert!(
            manager
                .swap_exact(before[0], before[1])
                .unwrap_err()
                .to_string()
                .contains("closed")
        );
        assert_eq!(manager.pane_ids(), before);

        let mut unsplit = property_manager(1);
        assert!(
            unsplit
                .balance_exact()
                .unwrap_err()
                .to_string()
                .contains("at least two")
        );
        assert!(
            unsplit
                .apply_layout_preset_exact(PaneLayoutPreset::Tall)
                .unwrap_err()
                .to_string()
                .contains("at least two")
        );

        let mut pair = property_manager(2);
        let pair_focus = pair.focused_id();
        pair.apply_layout_preset_exact(PaneLayoutPreset::Tall)
            .unwrap();
        assert_eq!(pair.pane_ids()[0], pair_focus);
        assert!(pair.get_dividers()[0].vertical);
        assert_eq!(pair.get_dividers()[0].ratio_millis, 500);
        pair.apply_layout_preset_exact(PaneLayoutPreset::Fat)
            .unwrap();
        assert_eq!(pair.pane_ids()[0], pair_focus);
        assert!(!pair.get_dividers()[0].vertical);
        assert_eq!(pair.get_dividers()[0].ratio_millis, 500);
    }

    #[test]
    fn live_reconciliation_rejects_exhausted_local_pane_ids_before_relay_spawn() {
        use crate::domain::model::{LivePaneLayout, LiveTopologyPane, LiveTopologyTab};
        let mut manager = property_manager(1);
        manager.next_id = u32::MAX;
        let mut mapping = HashMap::from([(PaneId(0), 10)]);
        let snapshot = LiveTopologyTab {
            id: 0,
            title: "replacement".into(),
            user_title: None,
            pinned: false,
            focused_pane: 11,
            layout: LivePaneLayout::Pane { pane: 11 },
            panes: vec![LiveTopologyPane {
                id: 11,
                title: "new".into(),
                closed: false,
            }],
        };
        // A nonexistent executable proves rejection precedes any relay launch.
        let relay = crate::domain::DomainAuthority::Local
            .relay_command("/definitely/missing/termeric-relay".into());
        for _ in 0..2 {
            assert!(
                manager
                    .reconcile_domain(&mut mapping, "test", &snapshot, &relay)
                    .unwrap_err()
                    .to_string()
                    .contains("pane ID space exhausted")
            );
            assert_eq!(mapping, HashMap::from([(PaneId(0), 10)]));
            assert_eq!(manager.pane_ids(), vec![PaneId(0)]);
            assert_eq!(manager.focused_id(), PaneId(0));
            assert_eq!(manager.next_id, u32::MAX);
        }
    }

    #[test]
    fn split_target_uses_exact_stable_id_and_preserves_focus_on_rejection() {
        let mut manager = property_manager(2);
        assert_eq!(manager.focused_id(), PaneId(1));
        assert!(
            manager
                .split_target(PaneId(99), SplitDirection::Vertical, PaneCwdMode::Focused)
                .is_err()
        );
        assert_eq!(manager.focused_id(), PaneId(1));

        manager.split_shell.program = Some("/definitely/missing/termeric-shell".into());
        assert!(
            manager
                .split_target(PaneId(0), SplitDirection::Horizontal, PaneCwdMode::Focused)
                .is_err()
        );
        assert_eq!(manager.focused_id(), PaneId(1));
        assert_eq!(manager.pane_count(), 2);
        manager.split_shell.program = Some("/bin/sh".into());
        manager.next_id = u32::MAX;
        assert!(
            manager
                .split_target(PaneId(0), SplitDirection::Horizontal, PaneCwdMode::Focused)
                .is_err()
        );
        assert_eq!(manager.focused_id(), PaneId(1));
        assert_eq!(manager.pane_count(), 2);
        manager.next_id = 2;

        let created = manager
            .split_target(PaneId(0), SplitDirection::Horizontal, PaneCwdMode::Focused)
            .unwrap();
        assert_eq!(created, PaneId(2));
        assert_eq!(manager.focused_id(), created);
        assert_eq!(manager.pane_count(), 3);
        assert_manager_invariants(&manager);
    }

    #[test]
    fn exact_local_divider_ratio_resize_reports_previous_noop_and_stale_id() {
        let mut manager = property_manager(3);
        let divider_id = manager
            .get_dividers()
            .into_iter()
            .find_map(|divider| match divider.target {
                DividerTarget::Local(id) => Some(id),
                DividerTarget::Domain(_) => None,
            })
            .unwrap();
        assert_eq!(
            manager.resize_local_divider(divider_id, 725).unwrap(),
            (500, true)
        );
        assert_eq!(
            manager.resize_local_divider(divider_id, 725).unwrap(),
            (725, false)
        );
        assert!(
            manager
                .resize_local_divider(u64::MAX, 600)
                .unwrap_err()
                .to_string()
                .contains("does not exist")
        );
        assert!(manager.resize_local_divider(divider_id, 99).is_err());
        assert!(manager.toggle_zoom());
        assert!(manager.resize_local_divider(divider_id, 600).is_err());
    }

    fn next_manager_property_value(state: &mut u64) -> u64 {
        *state ^= *state << 13;
        *state ^= *state >> 7;
        *state ^= *state << 17;
        *state
    }

    #[test]
    fn a_lone_pane_takes_the_tab_geometry_exactly() {
        // Floating point divides 27 cells of this width, multiplied out to
        // pixels, back into 26.999998 cells: re-deriving the geometry from
        // the area's pixels would lose a column.
        let cell_width = 42.909_317;
        let geometry = TerminalGeometry::from_cells(27, 10, cell_width, 20.0);
        let area = TerminalArea::new(geometry, cell_width, 20.0);
        assert_eq!(
            TerminalGeometry::from_available_pixels(area.width(), area.height(), cell_width, 20.0)
                .cols,
            26
        );
        let mut panes = PaneManager::for_test(
            80,
            24,
            crate::test_support::sleeping_shell(),
            1_000,
            8 * 1024 * 1024,
            800.0,
            480.0,
            10.0,
            20.0,
        )
        .unwrap();
        panes.resize(area);
        let lone = panes.get_pane(PaneId(0)).unwrap();
        assert_eq!(lone.geometry, geometry);
        assert_eq!(lone.pty.size().unwrap(), geometry.pty_size());

        let right = panes
            .split(SplitDirection::Vertical, PaneCwdMode::Focused)
            .unwrap();
        assert_ne!(panes.get_pane(PaneId(0)).unwrap().geometry, geometry);
        assert!(panes.close(right));
        let lone = panes.get_pane(PaneId(0)).unwrap();
        assert_eq!(lone.geometry, geometry);
        assert_eq!(lone.pty.size().unwrap(), geometry.pty_size());
    }

    #[test]
    fn closing_back_to_one_pane_leaves_an_unzoomed_untitled_primary() {
        let mut panes = property_manager(2);
        assert_eq!(panes.focused_id(), PaneId(1));
        panes.get_pane_mut(PaneId(1)).unwrap().title = Some("right".into());
        assert!(panes.toggle_zoom());
        assert!(panes.close(PaneId(0)));

        assert_eq!(panes.pane_count(), 1);
        assert_eq!(panes.focused_id(), PaneId(1));
        assert_eq!(panes.primary_id(), PaneId(1));
        assert_eq!(panes.zoomed_id(), None);
        assert_eq!(panes.get_pane(PaneId(1)).unwrap().title, None);
        assert!(panes.lone_pane().is_some());

        // IDs are never reused, and the survivor stays primary wherever a
        // later split moves it in the tree.
        let added = panes
            .split(SplitDirection::Vertical, PaneCwdMode::Focused)
            .unwrap();
        assert_eq!(added, PaneId(2));
        assert!(panes.swap_focused_next());
        assert_eq!(panes.pane_ids(), vec![PaneId(2), PaneId(1)]);
        assert_eq!(panes.primary_id(), PaneId(1));
    }

    #[test]
    fn test_pane_id() {
        let id1 = PaneId(0);
        let id2 = PaneId(1);
        assert_ne!(id1, id2);
        assert_eq!(id1, PaneId(0));
    }

    #[test]
    fn generated_workspace_actions_preserve_focus_zoom_and_live_owner_invariants() {
        let mut manager = property_manager(7);
        let owners: HashMap<_, _> = manager
            .pane_ids()
            .into_iter()
            .map(|id| (id, manager.get_pane(id).unwrap().pty.child_id()))
            .collect();
        let mut state = 0x35_5eed_u64;

        for step in 0..256 {
            let value = next_manager_property_value(&mut state);
            let ids = manager.pane_ids();
            let selected = ids[(value as usize >> 8) % ids.len()];
            match value % 7 {
                0 => manager.focus(selected),
                1 => {
                    assert!(manager.toggle_zoom());
                    if manager.zoomed_id().is_some() {
                        let next = ids[(value as usize >> 16) % ids.len()];
                        manager.focus(next);
                        assert_eq!(manager.zoomed_id(), Some(next));
                    }
                }
                2 => {
                    manager.focus(selected);
                    assert!(manager.swap_focused_next());
                }
                3 => assert!(manager.balance()),
                4 => {
                    let preset = match (value >> 8) % 5 {
                        0 => PaneLayoutPreset::Columns,
                        1 => PaneLayoutPreset::Rows,
                        2 => PaneLayoutPreset::Grid,
                        3 => PaneLayoutPreset::Tall,
                        _ => PaneLayoutPreset::Fat,
                    };
                    assert!(manager.apply_layout_preset(preset));
                }
                5 => {
                    let direction = match (value >> 8) % 4 {
                        0 => PaneDir::Up,
                        1 => PaneDir::Down,
                        2 => PaneDir::Left,
                        _ => PaneDir::Right,
                    };
                    manager.navigate(direction);
                }
                _ => manager.resize(TerminalArea::from_available_pixels(
                    900.0 + (value % 600) as f32,
                    600.0 + ((value >> 10) % 400) as f32,
                    8.0 + ((value >> 20) % 5) as f32,
                    16.0 + ((value >> 24) % 8) as f32,
                )),
            }

            if step % 17 == 0 {
                let focus = manager.focused_id();
                manager.focus(PaneId(u32::MAX));
                assert_eq!(manager.focused_id(), focus);
            }
            assert_manager_invariants(&manager);
            for (id, child) in &owners {
                assert_eq!(manager.get_pane(*id).unwrap().pty.child_id(), *child);
            }
        }
    }

    #[test]
    fn generated_detach_attach_moves_preserve_processes_and_valid_focus() {
        let mut source = property_manager(7);
        let mut destination = property_manager(1);
        let all_children: BTreeSet<_> = source
            .pane_ids()
            .into_iter()
            .map(|id| {
                source
                    .get_pane(id)
                    .and_then(|pane| pane.pty.child_id())
                    .expect("local source test shell has a child process")
            })
            .chain(destination.pane_ids().into_iter().map(|id| {
                destination
                    .get_pane(id)
                    .and_then(|pane| pane.pty.child_id())
                    .expect("local destination test shell has a child process")
            }))
            .collect();
        let mut state = 0x35_cafe_u64;

        while source.pane_count() > 1 {
            let ids = source.pane_ids();
            let selected = ids[next_manager_property_value(&mut state) as usize % ids.len()];
            source.focus(selected);
            let child = source.get_pane(selected).unwrap().pty.child_id();
            let pane = source.detach_focused().unwrap();
            let attached = destination
                .attach_target(
                    pane,
                    destination.focused_id(),
                    if state & 1 == 0 {
                        SplitDirection::Horizontal
                    } else {
                        SplitDirection::Vertical
                    },
                )
                .unwrap();
            assert_eq!(
                destination.get_pane(attached).unwrap().pty.child_id(),
                child
            );
            assert_manager_invariants(&source);
            assert_manager_invariants(&destination);
        }

        while destination.pane_count() > 1 {
            let ids = destination.pane_ids();
            let selected = ids[next_manager_property_value(&mut state) as usize % ids.len()];
            destination.focus(selected);
            let child = destination.get_pane(selected).unwrap().pty.child_id();
            let pane = destination.detach_focused().unwrap();
            let attached = source
                .attach_target(
                    pane,
                    source.focused_id(),
                    if state & 1 == 0 {
                        SplitDirection::Horizontal
                    } else {
                        SplitDirection::Vertical
                    },
                )
                .unwrap();
            assert_eq!(source.get_pane(attached).unwrap().pty.child_id(), child);
            assert_manager_invariants(&source);
            assert_manager_invariants(&destination);
        }

        let remaining_children: BTreeSet<_> = source
            .pane_ids()
            .into_iter()
            .filter_map(|id| source.get_pane(id).and_then(|pane| pane.pty.child_id()))
            .chain(destination.pane_ids().into_iter().filter_map(|id| {
                destination
                    .get_pane(id)
                    .and_then(|pane| pane.pty.child_id())
            }))
            .collect();
        // Moves relaunch nothing: the processes are exactly the original ones.
        assert_eq!(remaining_children, all_children);
    }

    #[test]
    fn screen_checksum_policy_updates_existing_and_future_panes() {
        let shell = ShellConfig {
            program: Some("/bin/sh".into()),
            args: vec!["-c".into(), "sleep 1".into()],
            env: Default::default(),
        };
        let mut panes = PaneManager::for_test(
            80,
            24,
            shell,
            1_000,
            8 * 1024 * 1024,
            800.0,
            480.0,
            10.0,
            20.0,
        )
        .unwrap();

        assert!(panes.focused_pane().is_some_and(|pane| {
            !pane.terminal.screen_checksum_reporting() && !pane.terminal.kitty_file_transmission()
        }));
        let allowed = TerminalPolicy {
            screen_checksum_reporting: true,
            kitty_file_transmission: true,
            ..TerminalPolicy::with_scrollback(1_000, 8 * 1024 * 1024)
        };
        panes.apply_terminal_policy(allowed, false);
        let split = panes
            .split(SplitDirection::Vertical, PaneCwdMode::Focused)
            .unwrap();
        assert!(
            panes
                .get_pane(split)
                .is_some_and(|pane| pane.terminal.screen_checksum_reporting())
        );
        assert!(
            panes
                .get_pane(split)
                .is_some_and(|pane| pane.terminal.kitty_file_transmission())
        );
        assert!(panes.panes.values().all(|pane| {
            pane.terminal.screen_checksum_reporting() && pane.terminal.kitty_file_transmission()
        }));

        panes.apply_terminal_policy(
            TerminalPolicy::with_scrollback(1_000, 8 * 1024 * 1024),
            false,
        );
        assert!(panes.panes.values().all(|pane| {
            !pane.terminal.screen_checksum_reporting() && !pane.terminal.kitty_file_transmission()
        }));
    }

    #[test]
    fn xterm_keyboard_profile_updates_existing_and_future_panes() {
        let shell = ShellConfig {
            program: Some("/bin/sh".into()),
            args: vec!["-c".into(), "sleep 1".into()],
            env: Default::default(),
        };
        let mut panes = PaneManager::for_test(
            80,
            24,
            shell,
            1_000,
            8 * 1024 * 1024,
            800.0,
            480.0,
            10.0,
            20.0,
        )
        .unwrap();

        let legacy = TerminalPolicy {
            xterm_keyboard_type: termeric_core::term::XtermKeyboardType::Legacy,
            ..TerminalPolicy::with_scrollback(1_000, 8 * 1024 * 1024)
        };
        panes.apply_terminal_policy(legacy, true);
        panes
            .focused_pane_mut()
            .unwrap()
            .terminal
            .process(b"\x1b[?1061h");
        panes.apply_terminal_policy(legacy, false);
        assert_eq!(
            panes.focused_pane().unwrap().terminal.xterm_keyboard_type(),
            termeric_core::term::XtermKeyboardType::Vt220
        );

        let split = panes
            .split(SplitDirection::Vertical, PaneCwdMode::Focused)
            .unwrap();
        let new_terminal = &panes.get_pane(split).unwrap().terminal;
        assert_eq!(
            new_terminal.xterm_keyboard_type(),
            termeric_core::term::XtermKeyboardType::Legacy
        );
        assert_eq!(
            new_terminal.initial_xterm_keyboard_type(),
            termeric_core::term::XtermKeyboardType::Legacy
        );
    }

    fn asymmetric_three_pane_layout() -> HashMap<PaneId, Bounds> {
        HashMap::from([
            (PaneId(0), Bounds::new(0.0, 0.0, 500.0, 800.0)),
            (PaneId(1), Bounds::new(500.0, 0.0, 500.0, 400.0)),
            (PaneId(2), Bounds::new(500.0, 400.0, 500.0, 400.0)),
        ])
    }

    #[test]
    fn ambiguous_navigation_has_a_stable_visual_tiebreaker() {
        let layout = asymmetric_three_pane_layout();
        assert_eq!(
            navigation_target(&layout, PaneId(0), PaneDir::Right, None),
            Some(PaneId(1)),
            "an equal T-split should consistently choose the top pane"
        );
    }

    #[test]
    fn remembered_reverse_route_wins_across_an_asymmetric_split() {
        let layout = asymmetric_three_pane_layout();
        // After bottom-right -> left, the reverse right route remembers the
        // bottom pane rather than falling back to the top-pane tiebreaker.
        assert_eq!(
            navigation_target(&layout, PaneId(0), PaneDir::Right, Some(PaneId(2))),
            Some(PaneId(2))
        );
    }

    #[test]
    fn zoom_is_view_only_and_hidden_ptys_keep_their_geometry() {
        let shell = ShellConfig {
            program: Some("/bin/sh".into()),
            args: vec!["-c".into(), "sleep 1".into()],
            env: Default::default(),
        };
        let mut panes = PaneManager::for_test(
            80,
            24,
            shell,
            1_000,
            8 * 1024 * 1024,
            800.0,
            480.0,
            10.0,
            20.0,
        )
        .unwrap();
        panes
            .split(SplitDirection::Vertical, PaneCwdMode::Focused)
            .unwrap();
        panes
            .split(SplitDirection::Horizontal, PaneCwdMode::Focused)
            .unwrap();
        let saved_tree = format!("{:?}", panes.workspace_layout());
        let hidden_geometry = panes.get_pane(PaneId(0)).unwrap().geometry;

        assert!(panes.toggle_zoom());
        assert_eq!(panes.calculate_layout().len(), 1);
        assert!(panes.get_dividers().is_empty());
        panes.resize(TerminalArea::from_available_pixels(
            1_000.0, 700.0, 10.0, 20.0,
        ));
        assert_eq!(panes.get_pane(PaneId(0)).unwrap().geometry, hidden_geometry);

        assert!(panes.toggle_zoom());
        assert_eq!(panes.calculate_layout().len(), 3);
        assert_eq!(format!("{:?}", panes.workspace_layout()), saved_tree);
        assert_ne!(panes.get_pane(PaneId(0)).unwrap().geometry, hidden_geometry);
    }

    #[test]
    fn detach_and_attach_transfer_the_live_pty_without_recreation() {
        let shell = ShellConfig {
            program: Some("/bin/sh".into()),
            args: vec!["-c".into(), "sleep 1".into()],
            env: Default::default(),
        };
        let make = || {
            PaneManager::for_test(
                80,
                24,
                shell.clone(),
                1_000,
                8 * 1024 * 1024,
                800.0,
                480.0,
                10.0,
                20.0,
            )
            .unwrap()
        };
        let mut source = make();
        let moved = source
            .split(SplitDirection::Vertical, PaneCwdMode::Focused)
            .unwrap();
        let child = source.get_pane(moved).unwrap().pty.child_id();
        let pane = source.detach_focused().unwrap();
        assert_eq!(pane.pty.child_id(), child);
        assert_eq!(source.pane_count(), 1);

        let mut destination = make();
        let target = destination
            .attach_target(pane, destination.focused_id(), SplitDirection::Vertical)
            .unwrap();
        assert_eq!(destination.get_pane(target).unwrap().pty.child_id(), child);
        assert_eq!(destination.focused_id(), target);
        assert_eq!(destination.pane_count(), 2);
    }

    #[test]
    fn transfer_guards_do_not_treat_closed_records_as_live_panes_or_anchors() {
        let shell = ShellConfig {
            program: Some("/bin/sh".into()),
            args: vec!["-c".into(), "sleep 1".into()],
            env: Default::default(),
        };
        let make = || {
            PaneManager::for_test(
                80,
                24,
                shell.clone(),
                1_000,
                8 * 1024 * 1024,
                800.0,
                480.0,
                10.0,
                20.0,
            )
            .unwrap()
        };

        let mut source = make();
        let first_closed = source
            .split(SplitDirection::Vertical, PaneCwdMode::Focused)
            .unwrap();
        let second_closed = source
            .split(SplitDirection::Horizontal, PaneCwdMode::Focused)
            .unwrap();
        source.get_pane_mut(first_closed).unwrap().closed = true;
        source.get_pane_mut(second_closed).unwrap().closed = true;
        assert_eq!(source.pane_count(), 3);
        assert_eq!(source.open_pane_count(), 1);
        assert!(source.detach(PaneId(0)).is_none());
        assert!(source.get_pane(PaneId(0)).is_some());

        let mut destination = make();
        let closed_anchor = destination
            .split(SplitDirection::Vertical, PaneCwdMode::Focused)
            .unwrap();
        destination.get_pane_mut(closed_anchor).unwrap().closed = true;
        let error = destination
            .validate_attach_target(closed_anchor)
            .unwrap_err()
            .to_string();
        assert!(error.contains("is closed"), "{error}");
    }

    #[test]
    fn each_new_pane_cwd_policy_uses_its_declared_process_source() {
        let temporary = tempfile::tempdir().unwrap();
        let primary_dir = temporary.path().join("primary");
        let focused_dir = temporary.path().join("focused");
        let reported_primary_dir = temporary.path().join("reported-primary");
        let reported_focused_dir = temporary.path().join("reported-focused");
        std::fs::create_dir_all(&primary_dir).unwrap();
        std::fs::create_dir_all(&focused_dir).unwrap();
        std::fs::create_dir_all(&reported_primary_dir).unwrap();
        std::fs::create_dir_all(&reported_focused_dir).unwrap();
        let shell = ShellConfig {
            program: Some("/bin/sh".into()),
            args: Vec::new(),
            env: Default::default(),
        };
        let mut panes = PaneManager::for_test(
            80,
            24,
            shell,
            1_000,
            8 * 1024 * 1024,
            800.0,
            480.0,
            10.0,
            20.0,
        )
        .unwrap();
        let focused = panes
            .split(SplitDirection::Vertical, PaneCwdMode::Focused)
            .unwrap();
        panes
            .get_pane_mut(PaneId(0))
            .unwrap()
            .write(format!("cd '{}'\n", primary_dir.display()).as_bytes())
            .unwrap();
        panes
            .get_pane_mut(focused)
            .unwrap()
            .write(format!("cd '{}'\n", focused_dir.display()).as_bytes())
            .unwrap();

        let wait_for_cwd = |panes: &mut PaneManager, id: PaneId, expected: &std::path::Path| {
            let deadline = Instant::now() + Duration::from_secs(2);
            loop {
                panes.poll_events();
                let cwd = panes
                    .get_pane(id)
                    .unwrap()
                    .pty
                    .child_id()
                    .and_then(crate::terminal::process::get_process_cwd);
                if cwd.as_deref() == expected.to_str() {
                    break;
                }
                assert!(
                    Instant::now() < deadline,
                    "cwd {cwd:?} did not become {expected:?}"
                );
                std::thread::sleep(Duration::from_millis(2));
            }
        };
        wait_for_cwd(&mut panes, PaneId(0), &primary_dir);
        wait_for_cwd(&mut panes, focused, &focused_dir);

        panes
            .get_pane_mut(PaneId(0))
            .unwrap()
            .terminal
            .process(format!("\x1b]7;file://{}\x07", reported_primary_dir.display()).as_bytes());
        panes
            .get_pane_mut(focused)
            .unwrap()
            .terminal
            .process(format!("\x1b]7;file://{}\x07", reported_focused_dir.display()).as_bytes());

        panes.focus(focused);
        let inherited_focus = panes
            .split(SplitDirection::Horizontal, PaneCwdMode::Focused)
            .unwrap();
        wait_for_cwd(&mut panes, inherited_focus, &reported_focused_dir);

        let inherited_tab = panes
            .split(SplitDirection::Horizontal, PaneCwdMode::Tab)
            .unwrap();
        wait_for_cwd(&mut panes, inherited_tab, &reported_primary_dir);

        let inherited_process = panes
            .split(SplitDirection::Horizontal, PaneCwdMode::Process)
            .unwrap();
        wait_for_cwd(
            &mut panes,
            inherited_process,
            &std::env::current_dir().unwrap(),
        );
    }

    #[test]
    fn fractional_asymmetric_nested_panes_share_renderer_and_kernel_geometry() {
        let shell = ShellConfig {
            program: Some("/bin/sh".into()),
            args: vec!["-c".into(), "sleep 1".into()],
            env: Default::default(),
        };
        let mut panes = PaneManager::for_test(
            100,
            40,
            shell,
            1_000,
            8 * 1024 * 1024,
            925.0,
            740.0,
            9.25,
            18.5,
        )
        .unwrap();
        panes
            .split(SplitDirection::Vertical, PaneCwdMode::Focused)
            .unwrap();
        let divider = panes.get_dividers()[0];
        assert!(
            panes
                .resize_divider_target(divider.target, 337.25, 200.0)
                .is_some()
        );
        panes
            .split(SplitDirection::Horizontal, PaneCwdMode::Focused)
            .unwrap();

        let layout = panes.calculate_layout();
        assert_eq!(layout.len(), 3);
        for (id, rect) in layout {
            let expected = TerminalGeometry::from_available_pixels(
                rect.width,
                rect.height,
                panes.area.cell_width,
                panes.area.cell_height,
            );
            let pane = panes.get_pane(id).unwrap();
            assert_eq!(pane.geometry, expected);
            assert_eq!(
                (pane.terminal.grid().cols(), pane.terminal.grid().rows()),
                (expected.cols, expected.rows)
            );
            assert_eq!(pane.pty.size().unwrap(), expected.pty_size());
        }
        let pane = panes.get_pane_mut(PaneId(0)).unwrap();
        pane.resize_geometry(TerminalGeometry::from_cells(8, 5, 2.0, 3.0))
            .unwrap();
        crate::test_support::seed_pixel_geometry_retention_probe(&mut pane.terminal);
        let scaled = TerminalGeometry::from_cells(8, 2, 1.0, 1.0);
        pane.resize_geometry(scaled).unwrap();
        assert_eq!(pane.pty.size().unwrap(), scaled.pty_size());
        crate::test_support::assert_pixel_geometry_retention_probe(&pane.terminal);
    }

    #[test]
    fn simultaneous_real_pane_dsr_replies_return_to_each_origin() {
        let script = concat!(
            "stty raw -echo; ",
            "printf '\\033[6n'; ",
            "reply=$(dd bs=1 count=6 2>/dev/null); ",
            "printf '\\rreply='; ",
            "printf '%s' \"$reply\" | od -An -tx1; ",
            "printf '\\r\\n'"
        );
        let shell = ShellConfig {
            program: Some("/bin/sh".into()),
            args: vec!["-c".into(), script.into()],
            env: Default::default(),
        };
        let mut panes = PaneManager::for_test(
            80,
            24,
            shell,
            1_000,
            8 * 1024 * 1024,
            800.0,
            480.0,
            10.0,
            20.0,
        )
        .unwrap();
        panes
            .split(SplitDirection::Vertical, PaneCwdMode::Focused)
            .unwrap();

        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            let events = panes.poll_events().events;
            assert!(
                events
                    .iter()
                    .all(|event| !matches!(event.event, TerminalEvent::PtyWrite(_)))
            );
            let all_replied = panes.panes.values().all(|pane| {
                pane.terminal
                    .grid()
                    .plain_lines()
                    .join("\n")
                    .contains("1b 5b 31 3b 31 52")
            });
            if all_replied && panes.panes.values().all(|pane| pane.closed) {
                break;
            }
            std::thread::sleep(Duration::from_millis(2));
        }

        assert_eq!(panes.pane_count(), 2);
        for pane in panes.panes.values() {
            let text = pane.terminal.grid().plain_lines().join("\n");
            assert!(
                text.contains("1b 5b 31 3b 31 52"),
                "pane {:?} did not receive its own DSR reply: {text:?}",
                pane.id
            );
            assert!(pane.closed, "pane {:?} did not exit", pane.id);
        }
    }

    #[test]
    fn close_during_event_heavy_output_discards_the_closed_origin_and_drains_survivor() {
        let script = concat!(
            "i=0; ",
            "while [ $i -lt 5000 ]; do printf '\\007'; i=$((i + 1)); done; ",
            "printf 'done\\r\\n'"
        );
        let shell = ShellConfig {
            program: Some("/bin/sh".into()),
            args: vec!["-c".into(), script.into()],
            env: Default::default(),
        };
        let mut panes = PaneManager::for_test(
            80,
            24,
            shell,
            1_000,
            8 * 1024 * 1024,
            800.0,
            480.0,
            10.0,
            20.0,
        )
        .unwrap();
        panes
            .split(SplitDirection::Vertical, PaneCwdMode::Focused)
            .unwrap();
        let closed_id = panes.focused_id();
        assert!(panes.close_focused());
        assert!(panes.get_pane(closed_id).is_none());

        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            let events = panes.poll_events().events;
            assert!(events.len() <= MAX_FORWARDED_TERMINAL_EVENTS_PER_POLL);
            assert!(
                events
                    .iter()
                    .all(|event| !matches!(event.event, TerminalEvent::PtyWrite(_)))
            );
            if panes.all_closed() {
                break;
            }
            std::thread::sleep(Duration::from_millis(2));
        }

        assert_eq!(panes.pane_count(), 1);
        assert!(panes.all_closed());
        for pane in panes.panes.values_mut() {
            assert!(pane.terminal.drain_events().is_empty());
        }
    }

    #[test]
    fn divider_hits_reach_the_margin_along_the_divider_and_stop_when_zoomed() {
        let mut manager = property_manager(3);
        let dividers = manager.get_dividers();
        let vertical = dividers.iter().find(|divider| divider.vertical).unwrap();
        let horizontal = dividers.iter().find(|divider| !divider.vertical).unwrap();
        let near = PANE_DIVIDER_HIT_MARGIN - 0.5;
        for divider in [vertical, horizontal] {
            // A quarter of the way along, clear of where the dividers meet.
            let along = divider.start + (divider.end - divider.start) / 4.0;
            let at = |across: f32| {
                if divider.vertical {
                    (across, along)
                } else {
                    (along, across)
                }
            };
            for across in [divider.position - near, divider.position + near] {
                let (x, y) = at(across);
                let hit = manager.divider_at(x, y).unwrap();
                assert_eq!(hit.target, divider.target);
                assert_eq!(hit.vertical, divider.vertical);
            }
            for across in [
                divider.position - PANE_DIVIDER_HIT_MARGIN,
                divider.position + PANE_DIVIDER_HIT_MARGIN,
            ] {
                let (x, y) = at(across);
                assert_ne!(
                    manager.divider_at(x, y).map(|hit| hit.target),
                    Some(divider.target)
                );
            }
            // Past either end, the divider is not there.
            let (x, y) = if divider.vertical {
                (divider.position, divider.end + 1.0)
            } else {
                (divider.end + 1.0, divider.position)
            };
            assert_ne!(
                manager.divider_at(x, y).map(|hit| hit.target),
                Some(divider.target)
            );
        }

        assert!(manager.toggle_zoom());
        for divider in [vertical, horizontal] {
            let along = divider.start + (divider.end - divider.start) / 4.0;
            let (x, y) = if divider.vertical {
                (divider.position, along)
            } else {
                (along, divider.position)
            };
            assert!(manager.divider_at(x, y).is_none());
        }
    }
}
