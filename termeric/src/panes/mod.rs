//! Pane management for split terminal views
//!
//! This module provides support for splitting a tab into multiple terminal panes,
//! arranged in a binary tree structure for flexible horizontal and vertical splits.

mod layout;
mod tree;

pub use layout::PaneRect;
pub(crate) use tree::PaneNode;
pub use tree::{PaneId, PaneTree, SplitDirection};

pub const PANE_DIVIDER_THICKNESS: f32 = 4.0;
pub const PANE_DIVIDER_HIT_MARGIN: f32 = 8.0;

/// Tab-local allocation history, including IDs no longer present in the tree.
/// This follows the tab's ownership mode, never an individually moved pane.
#[derive(Debug, Clone, Copy)]
pub(crate) struct PaneAllocationState {
    pub next_pane: u32,
    pub next_divider: u64,
}

impl Default for PaneAllocationState {
    fn default() -> Self {
        Self {
            next_pane: 1,
            next_divider: 1,
        }
    }
}

/// Dissolving pane mode must transfer its allocation history as well as its
/// final live owner. Keeping these together makes every collapse path explicit.
pub(crate) struct CollapsedPaneOwner {
    pub pane: Pane,
    pub allocation: PaneAllocationState,
}

use crate::config::ShellConfig;
use crate::terminal::container::{SplitContainer, TerminalContainer};
use crate::terminal::events::{MAX_FORWARDED_TERMINAL_EVENTS_PER_POLL, drain_terminal_events};
use crate::terminal::geometry::TerminalGeometry;
use crate::terminal::shell_integration::{
    ShellIntegrationSupport, ShellLaunch, prepare_shell_launch,
};
use anyhow::Context;
use std::collections::HashMap;
use std::io;
use std::sync::mpsc::Receiver;
use termeric_core::pty::{PtyEvent, PtyHandle, spawn_reader_thread_with_waker};
use termeric_core::{Pty, Terminal, TerminalEvent};

/// Four-way direction for pane navigation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PaneDir {
    Up,
    Down,
    Left,
    Right,
}

/// Deterministic layouts that preserve existing pane identities and PTYs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PaneLayoutPreset {
    Columns,
    Rows,
    Grid,
    Tall,
    Fat,
}

/// App-facing terminal event tagged with the pane that produced it. Protocol
/// replies are consumed inside [`Pane::poll_events`] and never appear here.
#[derive(Debug, Clone)]
pub struct PaneTerminalEvent {
    pub pane_id: PaneId,
    pub event: TerminalEvent,
}

impl PaneDir {
    /// Parse "up" / "down" / "left" / "right" (case-insensitive).
    pub fn parse(s: &str) -> Option<Self> {
        match s.to_ascii_lowercase().as_str() {
            "up" => Some(Self::Up),
            "down" => Some(Self::Down),
            "left" => Some(Self::Left),
            "right" => Some(Self::Right),
            _ => None,
        }
    }

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
    pub(crate) integration: crate::session::terminal_state::TerminalSession,
    event_overflow_reported: bool,
    /// Daemon-owned terminals consume protocol replies at the real PTY; their
    /// GUI replicas must not send a duplicate reply through the relay.
    suppress_protocol_replies: bool,
}

/// One resolved local launch; process arguments are never inferred from focus.
pub(crate) struct LocalPaneLaunch {
    pub identity: crate::workspace::WorkspaceLaunch,
    pub shell: ShellConfig,
    pub cwd: Option<String>,
}

impl Pane {
    /// The profile of a pane launched from a plugin or agent profile.
    #[cfg(test)]
    pub(crate) fn launch_profile(&self) -> Option<&str> {
        match &self.launch {
            crate::workspace::WorkspaceLaunch::Plugin { profile, .. } => profile.as_deref(),
            _ => None,
        }
    }

    /// Create a new pane with a shell
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
            (scrollback_lines, scrollback_bytes),
            working_dir,
            false,
        )
    }

    fn new_local(
        id: PaneId,
        geometry: TerminalGeometry,
        shell: &ShellConfig,
        scrollback: (usize, usize),
        working_dir: Option<&str>,
        plugin: bool,
    ) -> anyhow::Result<Self> {
        let mut terminal = Terminal::new(geometry.cols, geometry.rows);
        terminal.set_pixel_geometry(geometry.pixel_width.into(), geometry.pixel_height.into());
        terminal.set_scrollback_limits(scrollback.0, scrollback.1);
        let mut launch = if plugin {
            ShellLaunch {
                program: shell.program.clone(),
                args: shell.args.clone(),
                env: shell
                    .env
                    .iter()
                    .map(|(key, value)| (key.clone(), value.clone()))
                    .collect(),
                integration: ShellIntegrationSupport::Unavailable(
                    "Plugin launches do not install local shell hooks".into(),
                ),
            }
        } else {
            prepare_shell_launch(shell)
        };
        if let Some(working_dir) = working_dir {
            launch.env.retain(|(key, _)| key != "PWD");
            launch.env.push(("PWD".into(), working_dir.into()));
        }
        let pty = Pty::new_with_env_and_size(
            geometry.pty_size(),
            launch.program.as_deref(),
            &launch.args,
            &launch.env,
        )?;
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
            shell_integration: launch.integration,
            last_command: None,
            integration: crate::session::terminal_state::TerminalSession::default(),
            launch: crate::workspace::WorkspaceLaunch::Shell,
            launch_shell: shell.clone(),
            event_overflow_reported: false,
            suppress_protocol_replies: false,
        })
    }

    /// Create a pane backed by a direct command instead of the configured
    /// shell. Persistent-domain GUI attachments use this for one authenticated
    /// relay process per remote pane.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new_with_command(
        id: PaneId,
        geometry: TerminalGeometry,
        program: &str,
        args: &[String],
        scrollback_lines: usize,
        scrollback_bytes: usize,
        title: Option<String>,
    ) -> anyhow::Result<Self> {
        let mut terminal = Terminal::new(geometry.cols, geometry.rows);
        terminal.set_pixel_geometry(geometry.pixel_width.into(), geometry.pixel_height.into());
        terminal.set_scrollback_limits(scrollback_lines, scrollback_bytes);
        terminal.set_graphics_relay_snapshot_allowed(true);
        let pty = Pty::new_with_env_and_size(geometry.pty_size(), Some(program), args, &[])?;
        terminal.set_term_name(pty.term_name());
        let (handle, reader) = pty.take_reader();
        let rx = spawn_reader_thread_with_waker(reader, crate::app::pty_event_waker());
        Ok(Self {
            id,
            title,
            terminal,
            pty: handle,
            pty_rx: rx,
            closed: false,
            read_only: false,
            geometry,
            harvest: crate::harvest::HarvestTerminalState::default(),
            shell_integration: ShellIntegrationSupport::Unavailable(
                "Persistent-domain relays do not install a local shell hook".into(),
            ),
            last_command: Some("termeric domain relay".into()),
            integration: Default::default(),
            launch: crate::workspace::WorkspaceLaunch::Shell,
            launch_shell: ShellConfig::default(),
            event_overflow_reported: false,
            suppress_protocol_replies: true,
        })
    }

    /// Poll PTY output and immediately consume terminal-generated replies on
    /// this pane's own PTY. App-facing events are drained on every 4 KiB output
    /// chunk and bounded before being returned.
    pub fn poll_events(
        &mut self,
        mut on_output: impl FnMut(PaneId, &[u8]),
    ) -> (bool, Vec<TerminalEvent>) {
        let mut changed = self.integration.poll_notices();
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
                    self.integration.observe_output(
                        &data,
                        matches!(
                            self.launch,
                            crate::workspace::WorkspaceLaunch::Plugin { .. }
                        ),
                    );
                    on_output(self.id, &data);
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
                .and_then(crate::tabs::get_foreground_process)
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

        (
            changed || self.closed != was_closed || !app_events.is_empty(),
            app_events,
        )
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

fn replay_workspace_scrollback(terminal: &mut Terminal, lines: &[String]) {
    if !lines.is_empty() {
        terminal.process((lines.join("\r\n") + "\r\n").as_bytes());
    }
}

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
            direction: match direction {
                crate::domain::model::SplitDirection::Horizontal => SplitDirection::Horizontal,
                crate::domain::model::SplitDirection::Vertical => SplitDirection::Vertical,
            },
            ratio: f32::from(*ratio_millis) / 1_000.0,
            first: Box::new(domain_layout_to_local(first, ids)?),
            second: Box::new(domain_layout_to_local(second, ids)?),
        },
    })
}

/// Manager for panes within a single tab
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
    /// Shell configuration for new panes
    shell: ShellConfig,
    /// Persisted identity paired with `shell` for future panes.
    split_launch: crate::workspace::WorkspaceLaunch,
    /// Scrollback limit for new panes
    scrollback_lines: usize,
    /// Scrollback byte budget for new panes
    scrollback_bytes: usize,
    /// Host policy inherited by panes created after a split or restore.
    screen_checksum_reporting: bool,
    /// Host policy for Kitty persistent-file reads inherited by future panes.
    kitty_file_transmission: bool,
    /// Configured xterm keyboard profile inherited by future panes.
    xterm_keyboard_type: termeric_core::term::XtermKeyboardType,
    /// Next pane ID
    next_id: u32,
    /// Total available width in pixels (for layout calculations)
    total_width: f32,
    /// Total available height in pixels
    total_height: f32,
    /// Cell width in pixels
    cell_width: f32,
    /// Cell height in pixels
    cell_height: f32,
    event_overflow_reported: bool,
    /// Shells spawned by this manager (the adopted first pane is excluded).
    spawned_shells: usize,
    cwd_mode: crate::config::PaneCwdMode,
}

impl PaneManager {
    /// Create a new pane manager with a single pane
    pub fn new(
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
        let id = PaneId(0);
        let geometry = TerminalGeometry::from_cells(cols, rows, cell_width, cell_height);
        let pane = Pane::new(
            id,
            geometry,
            &shell,
            scrollback_lines,
            scrollback_bytes,
            None,
        )?;

        let mut panes = HashMap::new();
        panes.insert(id, pane);

        Ok(Self {
            tree: PaneTree::new(id),
            panes,
            focused: id,
            primary: id,
            zoomed: None,
            navigation_memory: HashMap::new(),
            shell,
            split_launch: crate::workspace::WorkspaceLaunch::Shell,
            scrollback_lines,
            scrollback_bytes,
            screen_checksum_reporting: false,
            kitty_file_transmission: false,
            xterm_keyboard_type: termeric_core::term::XtermKeyboardType::Normal,
            next_id: id.0.saturating_add(1),
            total_width,
            total_height,
            cell_width,
            cell_height,
            event_overflow_reported: false,
            spawned_shells: 1,
            cwd_mode: crate::config::PaneCwdMode::Focused,
        })
    }

    /// Enter pane mode by adopting a tab's existing live owner. This performs
    /// no PTY or reader-thread creation; only subsequent splits spawn shells.
    pub(crate) fn from_existing(
        owner: crate::tabs::TabTerminalOwner,
        shell: ShellConfig,
        scrollback_lines: usize,
        scrollback_bytes: usize,
        total_width: f32,
        total_height: f32,
        cell_width: f32,
        cell_height: f32,
    ) -> Self {
        let id = owner.pane_id;
        let allocation = owner.pane_allocation;
        let split_launch = match &owner.launch {
            crate::workspace::WorkspaceLaunch::Profile { name } => {
                crate::workspace::WorkspaceLaunch::Profile { name: name.clone() }
            }
            _ => crate::workspace::WorkspaceLaunch::Shell,
        };
        let screen_checksum_reporting = owner.terminal.screen_checksum_reporting();
        let kitty_file_transmission = owner.terminal.kitty_file_transmission();
        let xterm_keyboard_type = owner.terminal.initial_xterm_keyboard_type();
        let geometry = TerminalGeometry::from_cells(
            owner.terminal.grid().cols(),
            owner.terminal.grid().rows(),
            cell_width,
            cell_height,
        );
        let pane = Pane {
            id,
            title: None,
            terminal: owner.terminal,
            pty: owner.pty,
            pty_rx: owner.pty_rx,
            closed: false,
            read_only: owner.read_only,
            geometry,
            harvest: owner.harvest,
            shell_integration: owner.shell_integration,
            last_command: owner.last_command,
            launch: owner.launch,
            launch_shell: owner.launch_shell,
            integration: owner.integration,
            event_overflow_reported: false,
            suppress_protocol_replies: owner.suppress_protocol_replies,
        };
        let mut panes = HashMap::new();
        panes.insert(id, pane);
        Self {
            tree: PaneTree::with_next_local_split_id(id, allocation.next_divider),
            panes,
            focused: id,
            primary: id,
            zoomed: None,
            navigation_memory: HashMap::new(),
            shell,
            split_launch,
            scrollback_lines,
            scrollback_bytes,
            screen_checksum_reporting,
            kitty_file_transmission,
            xterm_keyboard_type,
            next_id: allocation.next_pane.max(id.0.saturating_add(1)),
            total_width,
            total_height,
            cell_width,
            cell_height,
            event_overflow_reported: false,
            spawned_shells: 0,
            cwd_mode: crate::config::PaneCwdMode::Focused,
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn from_workspace(
        owner: crate::tabs::TabTerminalOwner,
        shell: ShellConfig,
        scrollback_lines: usize,
        scrollback_bytes: usize,
        total_width: f32,
        total_height: f32,
        cell_width: f32,
        cell_height: f32,
        snapshot: &crate::workspace::WorkspaceTabSnapshot,
        pane_launches: &HashMap<u32, (crate::workspace::WorkspaceLaunch, ShellConfig)>,
    ) -> anyhow::Result<(Self, Vec<String>)> {
        snapshot.validate()?;
        let mut manager = Self::from_existing(
            owner,
            shell,
            scrollback_lines,
            scrollback_bytes,
            total_width,
            total_height,
            cell_width,
            cell_height,
        );
        manager.tree = PaneTree::from_root(PaneNode::from(&snapshot.layout));
        manager.focused = PaneId(snapshot.focused_pane);
        manager.next_id = snapshot
            .panes
            .iter()
            .map(|pane| pane.id)
            .max()
            .unwrap_or(0)
            .saturating_add(1);
        let primary_id = PaneId(snapshot.primary_pane);
        manager.primary = primary_id;
        if primary_id != PaneId(0) {
            let mut primary = manager
                .panes
                .remove(&PaneId(0))
                .context("workspace primary owner disappeared during restore")?;
            primary.id = primary_id;
            manager.panes.insert(primary_id, primary);
        }
        manager.recalculate_sizes();

        let mut warnings = Vec::new();
        let primary = snapshot
            .panes
            .iter()
            .find(|pane| pane.id == snapshot.primary_pane)
            .context("workspace tab has no primary pane")?;
        if let Some(pane) = manager.panes.get_mut(&primary_id) {
            pane.title.clone_from(&primary.title);
            pane.last_command.clone_from(&primary.last_command);
            replay_workspace_scrollback(&mut pane.terminal, &primary.scrollback_lines);
        }

        let layout = manager.calculate_layout();
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
                cell_width,
                cell_height,
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
                (scrollback_lines, scrollback_bytes),
                cwd.as_deref(),
                matches!(launch, crate::workspace::WorkspaceLaunch::Plugin { .. }),
            )?;
            pane.launch.clone_from(launch);
            pane.launch_shell.clone_from(shell);
            pane.terminal
                .set_screen_checksum_reporting(manager.screen_checksum_reporting);
            pane.terminal
                .set_kitty_file_transmission(manager.kitty_file_transmission);
            pane.terminal
                .configure_xterm_keyboard_type(manager.xterm_keyboard_type, true);
            pane.title.clone_from(&pane_snapshot.title);
            pane.last_command.clone_from(&pane_snapshot.last_command);
            replay_workspace_scrollback(&mut pane.terminal, &pane_snapshot.scrollback_lines);
            manager.panes.insert(id, pane);
            manager.spawned_shells += 1;
        }
        manager.recalculate_sizes();
        Ok((manager, warnings))
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn from_domain(
        owner: crate::tabs::TabTerminalOwner,
        shell: ShellConfig,
        scrollback_lines: usize,
        scrollback_bytes: usize,
        total_width: f32,
        total_height: f32,
        cell_width: f32,
        cell_height: f32,
        domain_id: &str,
        snapshot: &crate::domain::model::LiveTab,
        relay_command: &crate::domain::DomainRelayCommand,
    ) -> anyhow::Result<Self> {
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
        let mut manager = Self::from_existing(
            owner,
            shell,
            scrollback_lines,
            scrollback_bytes,
            total_width,
            total_height,
            cell_width,
            cell_height,
        );
        manager.tree = PaneTree::from_root(domain_layout_to_local(&snapshot.layout, &id_map)?);
        manager.focused = *id_map
            .get(&snapshot.focused_pane)
            .context("live domain focus references a missing pane")?;
        manager.next_id = remote_panes.len() as u32;
        manager.recalculate_sizes();
        if let Some(pane) = manager.panes.get_mut(&PaneId(0)) {
            pane.title = Some(primary.title.clone());
        }

        let layout = manager.calculate_layout();
        for remote in remote_panes.iter().skip(1) {
            let local_id = id_map[&remote.id];
            let rect = layout
                .get(&local_id)
                .with_context(|| format!("live domain layout omits pane {}", remote.id))?;
            let geometry = TerminalGeometry::from_available_pixels(
                rect.width,
                rect.height,
                cell_width,
                cell_height,
            );
            let arguments = relay_command.pane_arguments(domain_id, remote.id);
            let mut pane = Pane::new_with_command(
                local_id,
                geometry,
                relay_command.program(),
                &arguments,
                scrollback_lines,
                scrollback_bytes,
                Some(remote.title.clone()),
            )?;
            pane.terminal
                .set_screen_checksum_reporting(manager.screen_checksum_reporting);
            pane.terminal
                .set_kitty_file_transmission(manager.kitty_file_transmission);
            pane.terminal
                .configure_xterm_keyboard_type(manager.xterm_keyboard_type, true);
            manager.panes.insert(local_id, pane);
            manager.spawned_shells += 1;
        }
        manager.recalculate_sizes();
        Ok(manager)
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
            let geometry = TerminalGeometry::from_available_pixels(
                self.total_width,
                self.total_height,
                self.cell_width,
                self.cell_height,
            );
            let arguments = relay_command.pane_arguments(domain_id, remote.id);
            let mut pane = Pane::new_with_command(
                local_id,
                geometry,
                relay_command.program(),
                &arguments,
                self.scrollback_lines,
                self.scrollback_bytes,
                Some(remote.title.clone()),
            )?;
            pane.terminal
                .set_screen_checksum_reporting(self.screen_checksum_reporting);
            pane.terminal
                .set_kitty_file_transmission(self.kitty_file_transmission);
            pane.terminal
                .configure_xterm_keyboard_type(self.xterm_keyboard_type, true);
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
        self.spawned_shells += additions.len();
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

    pub fn set_cwd_mode(&mut self, mode: crate::config::PaneCwdMode) {
        self.cwd_mode = mode;
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
                let working_directory = crate::tabs::terminal_working_directory(&pane.terminal)
                    .or_else(|| pane.pty.child_id().and_then(crate::tabs::get_process_cwd));
                crate::workspace::WorkspacePaneSnapshot {
                    id: pane.id.0,
                    launch: Some(pane.launch.clone()),
                    title: pane
                        .title
                        .clone()
                        .or_else(|| (pane.id == primary_id).then(|| tab_title.to_string())),
                    working_directory,
                    last_command: pane
                        .pty
                        .child_id()
                        .and_then(crate::tabs::get_foreground_process)
                        .or_else(|| pane.last_command.clone()),
                    scrollback_file: None,
                    scrollback_truncated: false,
                    scrollback_lines: if save_scrollback {
                        pane.terminal.grid().export_scrollback_text()
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

    /// Apply updated scrollback budgets to every pane and future splits.
    pub fn set_scrollback_limits(&mut self, lines: usize, bytes: usize) {
        self.scrollback_lines = lines;
        self.scrollback_bytes = bytes;
        for pane in self.panes.values_mut() {
            pane.terminal.set_scrollback_limits(lines, bytes);
        }
    }

    /// Update every pane and the policy inherited by future splits.
    pub fn set_screen_checksum_reporting(&mut self, allowed: bool) {
        self.screen_checksum_reporting = allowed;
        for pane in self.panes.values_mut() {
            pane.terminal.set_screen_checksum_reporting(allowed);
        }
    }

    /// Update every pane and the Kitty persistent-file policy inherited by
    /// future splits.
    pub fn set_kitty_file_transmission(&mut self, allowed: bool) {
        self.kitty_file_transmission = allowed;
        for pane in self.panes.values_mut() {
            pane.terminal.set_kitty_file_transmission(allowed);
        }
    }

    /// Update every pane and the profile inherited by future splits.
    pub fn set_xterm_keyboard_type(
        &mut self,
        keyboard_type: termeric_core::term::XtermKeyboardType,
        apply_current: bool,
    ) {
        self.xterm_keyboard_type = keyboard_type;
        for pane in self.panes.values_mut() {
            pane.terminal
                .configure_xterm_keyboard_type(keyboard_type, apply_current);
        }
    }

    /// Prospective geometry uses the unzoomed tree, exactly as splitting does.
    pub(crate) fn split_geometry(
        &self,
        target: PaneId,
        direction: SplitDirection,
    ) -> anyhow::Result<TerminalGeometry> {
        let layout = self.tree.calculate_layout(PaneRect {
            x: 0.0,
            y: 0.0,
            width: self.total_width,
            height: self.total_height,
        });
        let rect = layout.get(&target).context("split target is missing")?;
        Ok(Self::child_geometry(
            rect.width,
            rect.height,
            self.cell_width,
            self.cell_height,
            direction,
        ))
    }

    pub(crate) fn child_geometry(
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

    /// Split the focused pane
    pub fn split(&mut self, direction: SplitDirection) -> anyhow::Result<PaneId> {
        self.split_local(direction, None)
    }

    fn split_local(
        &mut self,
        direction: SplitDirection,
        launch: Option<&LocalPaneLaunch>,
    ) -> anyhow::Result<PaneId> {
        if !self.tree.has_split_capacity() {
            anyhow::bail!("divider ID space exhausted");
        }
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
        let working_dir = match self.cwd_mode {
            crate::config::PaneCwdMode::Focused => self.panes.get(&self.focused).and_then(|pane| {
                crate::tabs::terminal_working_directory(&pane.terminal)
                    .or_else(|| pane.pty.child_id().and_then(crate::tabs::get_process_cwd))
            }),
            crate::config::PaneCwdMode::Tab => {
                self.panes.get(&self.primary_id()).and_then(|pane| {
                    crate::tabs::terminal_working_directory(&pane.terminal)
                        .or_else(|| pane.pty.child_id().and_then(crate::tabs::get_process_cwd))
                })
            }
            crate::config::PaneCwdMode::Process => std::env::current_dir()
                .ok()
                .map(|path| path.to_string_lossy().into_owned()),
        };
        let shell = launch.map_or(&self.shell, |launch| &launch.shell);
        let cwd = launch.map_or(working_dir.as_deref(), |launch| launch.cwd.as_deref());
        let mut pane = match Pane::new_local(
            new_id,
            geometry,
            shell,
            (self.scrollback_lines, self.scrollback_bytes),
            cwd,
            launch.is_some_and(|launch| {
                matches!(
                    launch.identity,
                    crate::workspace::WorkspaceLaunch::Plugin { .. }
                )
            }),
        ) {
            Ok(pane) => pane,
            Err(error) => {
                self.next_id = new_id.0;
                self.zoomed = was_zoomed;
                return Err(error);
            }
        };
        pane.launch
            .clone_from(launch.map_or(&self.split_launch, |launch| &launch.identity));
        pane.terminal
            .set_screen_checksum_reporting(self.screen_checksum_reporting);
        pane.terminal
            .set_kitty_file_transmission(self.kitty_file_transmission);
        pane.terminal
            .configure_xterm_keyboard_type(self.xterm_keyboard_type, true);
        self.spawned_shells += 1;
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
    ) -> anyhow::Result<PaneId> {
        self.split_target_local(target, direction, None)
    }

    pub(crate) fn split_target_with_launch(
        &mut self,
        target: PaneId,
        direction: SplitDirection,
        launch: &LocalPaneLaunch,
    ) -> anyhow::Result<PaneId> {
        self.split_target_local(target, direction, Some(launch))
    }

    fn split_target_local(
        &mut self,
        target: PaneId,
        direction: SplitDirection,
        launch: Option<&LocalPaneLaunch>,
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
    /// event path. The callback observes each raw PTY output chunk without
    /// retaining another unbounded copy.
    pub fn poll_events(
        &mut self,
        mut on_output: impl FnMut(PaneId, &[u8]),
    ) -> (bool, Vec<PaneTerminalEvent>) {
        let mut changed = false;
        let mut all_events = Vec::new();
        let mut dropped_events = 0usize;

        for pane in self.panes.values_mut() {
            let pane_id = pane.id;
            let (pane_changed, events) = pane.poll_events(&mut on_output);
            changed |= pane_changed;
            for event in events {
                if all_events.len() < MAX_FORWARDED_TERMINAL_EVENTS_PER_POLL {
                    all_events.push(PaneTerminalEvent { pane_id, event });
                } else {
                    dropped_events += 1;
                }
            }
        }

        if dropped_events > 0 && !self.event_overflow_reported {
            self.event_overflow_reported = true;
            log::warn!(
                "Pane manager dropped {} app-facing terminal events after reaching the per-poll limit of {}",
                dropped_events,
                MAX_FORWARDED_TERMINAL_EVENTS_PER_POLL
            );
        }

        (changed || !all_events.is_empty(), all_events)
    }

    /// Resize the total available space
    /// Whether the panes are already laid out for this area and cell size.
    pub fn is_laid_out_for(
        &self,
        total_width: f32,
        total_height: f32,
        cell_width: f32,
        cell_height: f32,
    ) -> bool {
        self.total_width == total_width
            && self.total_height == total_height
            && self.cell_width == cell_width
            && self.cell_height == cell_height
    }

    pub fn resize(
        &mut self,
        total_width: f32,
        total_height: f32,
        cell_width: f32,
        cell_height: f32,
    ) {
        self.total_width = total_width;
        self.total_height = total_height;
        self.cell_width = cell_width;
        self.cell_height = cell_height;
        self.recalculate_sizes();
    }

    /// Calculate the layout for all panes
    pub fn calculate_layout(&self) -> HashMap<PaneId, PaneRect> {
        let root_rect = PaneRect {
            x: 0.0,
            y: 0.0,
            width: self.total_width,
            height: self.total_height,
        };
        if let Some(id) = self.zoomed.filter(|id| self.panes.contains_key(id)) {
            return HashMap::from([(id, root_rect)]);
        }
        self.tree.calculate_layout(root_rect)
    }

    /// Recalculate and apply sizes to all panes
    fn recalculate_sizes(&mut self) {
        let layout = self.calculate_layout();

        for (id, rect) in layout {
            if let Some(pane) = self.panes.get_mut(&id) {
                let geometry = TerminalGeometry::from_available_pixels(
                    rect.width,
                    rect.height,
                    self.cell_width,
                    self.cell_height,
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
        self.tree.get_dividers(PaneRect {
            x: 0.0,
            y: 0.0,
            width: self.total_width,
            height: self.total_height,
        })
    }

    /// Resize a split divider at the given position (in content-local coordinates).
    ///
    /// `threshold` controls how close the pointer must be for the initial hit
    /// test. Active UI drags retain that divider's [`DividerTarget`] and call
    /// [`Self::resize_divider_target`] directly.
    pub fn resize_divider_at(&mut self, pos_x: f32, pos_y: f32, threshold: f32) -> bool {
        let Some(divider) = self.divider_at(pos_x, pos_y, threshold) else {
            return false;
        };
        self.resize_divider_target(divider.target, pos_x, pos_y)
            .is_some()
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
        let ratio = self.tree.resize_target(
            PaneRect {
                x: 0.0,
                y: 0.0,
                width: self.total_width,
                height: self.total_height,
            },
            pos_x,
            pos_y,
            target,
        )?;
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
        if !(100..=900).contains(&ratio_millis) {
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

    pub fn divider_at(&self, pos_x: f32, pos_y: f32, threshold: f32) -> Option<Divider> {
        self.get_dividers().into_iter().find(|divider| {
            if divider.vertical {
                (pos_x - divider.position).abs() < threshold
                    && pos_y >= divider.start
                    && pos_y <= divider.end
            } else {
                (pos_y - divider.position).abs() < threshold
                    && pos_x >= divider.start
                    && pos_x <= divider.end
            }
        })
    }

    /// Hit-test dividers. Returns Some(is_vertical) if near a divider.
    pub fn hit_test_divider(&self, pos_x: f32, pos_y: f32) -> Option<bool> {
        if self.zoomed.is_some() {
            return None;
        }
        self.divider_at(pos_x, pos_y, PANE_DIVIDER_HIT_MARGIN)
            .map(|divider| divider.vertical)
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
            .get_dividers(PaneRect {
                x: 0.0,
                y: 0.0,
                width: self.total_width,
                height: self.total_height,
            })
            .iter()
            .any(|divider| divider.ratio_millis != 500);
        if changed {
            self.tree.balance();
            self.recalculate_sizes();
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
        let applied = match preset {
            PaneLayoutPreset::Columns => self.tree.apply_preset(SplitDirection::Vertical, false),
            PaneLayoutPreset::Rows => self.tree.apply_preset(SplitDirection::Horizontal, false),
            PaneLayoutPreset::Grid => self.tree.apply_preset(SplitDirection::Vertical, true),
            PaneLayoutPreset::Tall => self
                .tree
                .apply_master_detail_preset(self.focused, SplitDirection::Vertical),
            PaneLayoutPreset::Fat => self
                .tree
                .apply_master_detail_preset(self.focused, SplitDirection::Horizontal),
        };
        if !applied {
            anyhow::bail!("Pane layout split ID space is exhausted");
        }
        self.zoomed = None;
        self.navigation_memory.clear();
        self.recalculate_sizes();
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

    pub fn balance(&mut self) -> bool {
        if self.panes.len() <= 1 {
            return false;
        }
        self.tree.balance();
        self.recalculate_sizes();
        true
    }

    pub fn apply_layout_preset(&mut self, preset: PaneLayoutPreset) -> bool {
        if self.panes.len() <= 1 {
            return false;
        }
        match preset {
            PaneLayoutPreset::Columns => {
                if !self.tree.apply_preset(SplitDirection::Vertical, false) {
                    return false;
                }
            }
            PaneLayoutPreset::Rows => {
                if !self.tree.apply_preset(SplitDirection::Horizontal, false) {
                    return false;
                }
            }
            PaneLayoutPreset::Grid => {
                if !self.tree.apply_preset(SplitDirection::Vertical, true) {
                    return false;
                }
            }
            PaneLayoutPreset::Tall => {
                if !self
                    .tree
                    .apply_master_detail_preset(self.focused, SplitDirection::Vertical)
                {
                    return false;
                }
            }
            PaneLayoutPreset::Fat => {
                if !self
                    .tree
                    .apply_master_detail_preset(self.focused, SplitDirection::Horizontal)
                {
                    return false;
                }
            }
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
        self.recalculate_sizes();
        Some(pane)
    }

    /// Attach an existing live PTY beside the focused pane under a new stable,
    /// tab-local ID. No process or reader thread is recreated.
    pub(crate) fn attach(&mut self, mut pane: Pane, direction: SplitDirection) -> PaneId {
        let id = PaneId(self.next_id);
        self.next_id = self.next_id.saturating_add(1);
        pane.id = id;
        self.panes.insert(id, pane);
        assert!(
            self.tree.split(self.focused, id, direction),
            "focused pane and divider ID remain splittable"
        );
        self.focused = id;
        self.zoomed = None;
        self.navigation_memory.clear();
        self.recalculate_sizes();
        id
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
        if self.panes.len() >= crate::control::protocol::MAX_PANES_PER_TAB {
            anyhow::bail!(
                "GUI tab pane limit of {} reached",
                crate::control::protocol::MAX_PANES_PER_TAB
            );
        }
        if self.next_id.checked_add(1).is_none() || self.panes.contains_key(&PaneId(self.next_id)) {
            anyhow::bail!("destination tab exhausted its stable pane ID space");
        }
        if !self.tree.has_split_capacity() {
            anyhow::bail!("destination tab exhausted its stable divider ID space");
        }
        Ok(())
    }

    /// Attach a detached owner beside an exact stable destination pane.
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
        self.panes.insert(id, pane);
        assert!(
            self.tree.split(target, id, direction),
            "validated target pane and divider ID remain splittable"
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

    #[cfg(test)]
    pub fn spawned_shell_count(&self) -> usize {
        self.spawned_shells
    }

    /// Take the last remaining pane out of the manager.
    ///
    /// Returns the sole pane and both allocation high-water marks, consuming
    /// the pane from the internal map. Used when dissolving pane mode back to a
    /// single-terminal tab; retired IDs must remain retired on the next split.
    pub(crate) fn take_remaining_pane(&mut self) -> Option<CollapsedPaneOwner> {
        if self.panes.len() != 1 {
            return None;
        }
        let id = *self.panes.keys().next()?;
        Some(CollapsedPaneOwner {
            pane: self.panes.remove(&id)?,
            allocation: PaneAllocationState {
                next_pane: self.next_id,
                next_divider: self.tree.next_local_split_id(),
            },
        })
    }

    /// Iterate over all panes with their layouts
    pub fn iter_with_layout(&self) -> impl Iterator<Item = (&Pane, PaneRect)> {
        let layout = self.calculate_layout();
        self.panes
            .values()
            .filter_map(move |pane| layout.get(&pane.id).map(|&rect| (pane, rect)))
    }

    pub(crate) fn iter_panes(&self) -> impl ExactSizeIterator<Item = &Pane> {
        self.panes.values()
    }

    pub fn iter_panes_identified(&self) -> Vec<(PaneId, &Terminal, PaneRect, bool)> {
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
            self.recalculate_sizes();
        }

        !self.panes.is_empty()
    }

    /// Check if all panes are closed
    pub fn all_closed(&self) -> bool {
        self.panes.is_empty() || self.panes.values().all(|p| p.closed)
    }
}

/// Implement TerminalContainer for PaneManager
impl TerminalContainer for PaneManager {
    fn focused_terminal(&self) -> Option<&Terminal> {
        self.focused_pane().map(|p| &p.terminal)
    }

    fn focused_terminal_mut(&mut self) -> Option<&mut Terminal> {
        self.focused_pane_mut().map(|p| &mut p.terminal)
    }

    fn write_input(&mut self, data: &[u8]) -> io::Result<()> {
        if let Some(pane) = self.focused_pane_mut() {
            pane.write(data)
        } else {
            Ok(())
        }
    }

    fn resize(&mut self, _cols: usize, _rows: usize) -> anyhow::Result<()> {
        // PaneManager handles resize differently - via resize() method with pixel dimensions
        // This is called with cols/rows but PaneManager needs pixel dimensions
        // Recalculate all pane sizes based on current total dimensions
        self.recalculate_sizes();
        Ok(())
    }

    fn is_closed(&self) -> bool {
        self.all_closed()
    }
}

/// Implement SplitContainer for PaneManager
impl SplitContainer for PaneManager {
    fn split(&mut self, direction: SplitDirection) -> anyhow::Result<PaneId> {
        PaneManager::split(self, direction)
    }

    fn close_focused_pane(&mut self) -> bool {
        self.close_focused()
    }

    fn navigate(&mut self, dir: PaneDir) -> bool {
        PaneManager::navigate(self, dir)
    }

    fn pane_count(&self) -> usize {
        PaneManager::pane_count(self)
    }

    fn pane_layout(&self) -> HashMap<PaneId, PaneRect> {
        self.calculate_layout()
    }

    fn dividers(&self) -> Vec<Divider> {
        self.get_dividers()
    }

    fn focused_pane_id(&self) -> PaneId {
        self.focused_id()
    }

    fn iter_panes(&self) -> Vec<(&Terminal, PaneRect, bool)> {
        let layout = self.calculate_layout();
        self.panes
            .values()
            .filter_map(|pane| {
                layout
                    .get(&pane.id)
                    .map(|&rect| (&pane.terminal, rect, pane.id == self.focused))
            })
            .collect()
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
    layout: &HashMap<PaneId, PaneRect>,
    current_id: PaneId,
    dir: PaneDir,
    remembered: Option<PaneId>,
) -> Option<PaneId> {
    let current = layout.get(&current_id)?;
    let is_neighbor = |rect: &PaneRect| match dir {
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

    let primary_distance = |rect: &PaneRect| match dir {
        PaneDir::Right => rect.x - current.right(),
        PaneDir::Left => current.x - rect.right(),
        PaneDir::Down => rect.y - current.bottom(),
        PaneDir::Up => current.y - rect.bottom(),
    };
    let cross_distance = |rect: &PaneRect| match dir {
        PaneDir::Right | PaneDir::Left => (rect.center_y() - current.center_y()).abs(),
        PaneDir::Up | PaneDir::Down => (rect.center_x() - current.center_x()).abs(),
    };
    let cross_position = |rect: &PaneRect| match dir {
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
fn rects_overlap_vertically(a: &PaneRect, b: &PaneRect) -> bool {
    a.y < b.y + b.height && a.y + a.height > b.y
}

/// Check if two rectangles overlap horizontally
fn rects_overlap_horizontally(a: &PaneRect, b: &PaneRect) -> bool {
    a.x < b.x + b.width && a.x + a.width > b.x
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::{BTreeSet, HashSet};
    use std::time::{Duration, Instant};

    #[cfg(unix)]
    fn property_manager(panes: usize) -> PaneManager {
        let shell = ShellConfig {
            program: Some("/bin/sh".into()),
            args: vec!["-c".into(), "sleep 2".into()],
            env: Default::default(),
        };
        let mut manager = PaneManager::new(
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
            manager
                .split(if index % 2 == 0 {
                    SplitDirection::Horizontal
                } else {
                    SplitDirection::Vertical
                })
                .unwrap();
        }
        manager
    }

    #[cfg(unix)]
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

    #[cfg(unix)]
    #[test]
    fn exact_layout_primitives_preserve_owners_focus_and_atomicity() {
        if !std::path::Path::new("/bin/sh").exists() {
            return;
        }
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

    #[cfg(unix)]
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

    #[cfg(unix)]
    #[test]
    fn split_target_uses_exact_stable_id_and_preserves_focus_on_rejection() {
        if !std::path::Path::new("/bin/sh").exists() {
            return;
        }
        let mut manager = property_manager(2);
        assert_eq!(manager.focused_id(), PaneId(1));
        assert!(
            manager
                .split_target(PaneId(99), SplitDirection::Vertical)
                .is_err()
        );
        assert_eq!(manager.focused_id(), PaneId(1));

        manager.shell.program = Some("/definitely/missing/termeric-shell".into());
        assert!(
            manager
                .split_target(PaneId(0), SplitDirection::Horizontal)
                .is_err()
        );
        assert_eq!(manager.focused_id(), PaneId(1));
        assert_eq!(manager.pane_count(), 2);
        manager.shell.program = Some("/bin/sh".into());
        manager.next_id = u32::MAX;
        assert!(
            manager
                .split_target(PaneId(0), SplitDirection::Horizontal)
                .is_err()
        );
        assert_eq!(manager.focused_id(), PaneId(1));
        assert_eq!(manager.pane_count(), 2);
        manager.next_id = 2;

        let created = manager
            .split_target(PaneId(0), SplitDirection::Horizontal)
            .unwrap();
        assert_eq!(created, PaneId(2));
        assert_eq!(manager.focused_id(), created);
        assert_eq!(manager.pane_count(), 3);
        assert_manager_invariants(&manager);
    }

    #[cfg(unix)]
    #[test]
    fn exact_local_divider_ratio_resize_reports_previous_noop_and_stale_id() {
        if !std::path::Path::new("/bin/sh").exists() {
            return;
        }
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

    #[cfg(unix)]
    fn next_manager_property_value(state: &mut u64) -> u64 {
        *state ^= *state << 13;
        *state ^= *state >> 7;
        *state ^= *state << 17;
        *state
    }

    #[test]
    fn test_pane_id() {
        let id1 = PaneId(0);
        let id2 = PaneId(1);
        assert_ne!(id1, id2);
        assert_eq!(id1, PaneId(0));
    }

    #[test]
    fn test_pane_dir_parse() {
        assert_eq!(PaneDir::parse("up"), Some(PaneDir::Up));
        assert_eq!(PaneDir::parse("DOWN"), Some(PaneDir::Down));
        assert_eq!(PaneDir::parse("Left"), Some(PaneDir::Left));
        assert_eq!(PaneDir::parse("right"), Some(PaneDir::Right));
        assert_eq!(PaneDir::parse("diagonal"), None);
        assert_eq!(PaneDir::parse(""), None);
    }

    #[cfg(unix)]
    #[test]
    fn generated_workspace_actions_preserve_focus_zoom_and_live_owner_invariants() {
        if !std::path::Path::new("/bin/sh").exists() {
            return;
        }
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
                _ => manager.resize(
                    900.0 + (value % 600) as f32,
                    600.0 + ((value >> 10) % 400) as f32,
                    8.0 + ((value >> 20) % 5) as f32,
                    16.0 + ((value >> 24) % 8) as f32,
                ),
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

    #[cfg(unix)]
    #[test]
    fn generated_detach_attach_moves_preserve_processes_and_valid_focus() {
        if !std::path::Path::new("/bin/sh").exists() {
            return;
        }
        let mut source = property_manager(7);
        let mut destination = property_manager(1);
        let source_spawned = source.spawned_shell_count();
        let destination_spawned = destination.spawned_shell_count();
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
            let attached = destination.attach(
                pane,
                if state & 1 == 0 {
                    SplitDirection::Horizontal
                } else {
                    SplitDirection::Vertical
                },
            );
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
            let attached = source.attach(
                pane,
                if state & 1 == 0 {
                    SplitDirection::Horizontal
                } else {
                    SplitDirection::Vertical
                },
            );
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
        assert_eq!(remaining_children, all_children);
        assert_eq!(source.spawned_shell_count(), source_spawned);
        assert_eq!(destination.spawned_shell_count(), destination_spawned);
    }

    #[cfg(unix)]
    #[test]
    fn screen_checksum_policy_updates_existing_and_future_panes() {
        if !std::path::Path::new("/bin/sh").exists() {
            return;
        }
        let shell = ShellConfig {
            program: Some("/bin/sh".into()),
            args: vec!["-c".into(), "sleep 1".into()],
            env: Default::default(),
        };
        let mut panes = PaneManager::new(
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
        panes.set_screen_checksum_reporting(true);
        panes.set_kitty_file_transmission(true);
        let split = panes.split(SplitDirection::Vertical).unwrap();
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

        panes.set_screen_checksum_reporting(false);
        panes.set_kitty_file_transmission(false);
        assert!(panes.panes.values().all(|pane| {
            !pane.terminal.screen_checksum_reporting() && !pane.terminal.kitty_file_transmission()
        }));
    }

    #[cfg(unix)]
    #[test]
    fn xterm_keyboard_profile_updates_existing_and_future_panes() {
        if !std::path::Path::new("/bin/sh").exists() {
            return;
        }
        let shell = ShellConfig {
            program: Some("/bin/sh".into()),
            args: vec!["-c".into(), "sleep 1".into()],
            env: Default::default(),
        };
        let mut panes = PaneManager::new(
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

        panes.set_xterm_keyboard_type(termeric_core::term::XtermKeyboardType::Legacy, true);
        panes
            .focused_pane_mut()
            .unwrap()
            .terminal
            .process(b"\x1b[?1061h");
        panes.set_xterm_keyboard_type(termeric_core::term::XtermKeyboardType::Legacy, false);
        assert_eq!(
            panes.focused_pane().unwrap().terminal.xterm_keyboard_type(),
            termeric_core::term::XtermKeyboardType::Vt220
        );

        let split = panes.split(SplitDirection::Vertical).unwrap();
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

    fn asymmetric_three_pane_layout() -> HashMap<PaneId, PaneRect> {
        HashMap::from([
            (PaneId(0), PaneRect::new(0.0, 0.0, 500.0, 800.0)),
            (PaneId(1), PaneRect::new(500.0, 0.0, 500.0, 400.0)),
            (PaneId(2), PaneRect::new(500.0, 400.0, 500.0, 400.0)),
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

    #[cfg(unix)]
    #[test]
    fn zoom_is_view_only_and_hidden_ptys_keep_their_geometry() {
        if !std::path::Path::new("/bin/sh").exists() {
            return;
        }
        let shell = ShellConfig {
            program: Some("/bin/sh".into()),
            args: vec!["-c".into(), "sleep 1".into()],
            env: Default::default(),
        };
        let mut panes = PaneManager::new(
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
        panes.split(SplitDirection::Vertical).unwrap();
        panes.split(SplitDirection::Horizontal).unwrap();
        let saved_tree = format!("{:?}", panes.workspace_layout());
        let hidden_geometry = panes.get_pane(PaneId(0)).unwrap().geometry;

        assert!(panes.toggle_zoom());
        assert_eq!(panes.calculate_layout().len(), 1);
        assert!(panes.get_dividers().is_empty());
        panes.resize(1_000.0, 700.0, 10.0, 20.0);
        assert_eq!(panes.get_pane(PaneId(0)).unwrap().geometry, hidden_geometry);

        assert!(panes.toggle_zoom());
        assert_eq!(panes.calculate_layout().len(), 3);
        assert_eq!(format!("{:?}", panes.workspace_layout()), saved_tree);
        assert_ne!(panes.get_pane(PaneId(0)).unwrap().geometry, hidden_geometry);
    }

    #[cfg(unix)]
    #[test]
    fn detach_and_attach_transfer_the_live_pty_without_recreation() {
        if !std::path::Path::new("/bin/sh").exists() {
            return;
        }
        let shell = ShellConfig {
            program: Some("/bin/sh".into()),
            args: vec!["-c".into(), "sleep 1".into()],
            env: Default::default(),
        };
        let make = || {
            PaneManager::new(
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
        let moved = source.split(SplitDirection::Vertical).unwrap();
        let child = source.get_pane(moved).unwrap().pty.child_id();
        let pane = source.detach_focused().unwrap();
        assert_eq!(pane.pty.child_id(), child);
        assert_eq!(source.pane_count(), 1);

        let mut destination = make();
        let target = destination.attach(pane, SplitDirection::Vertical);
        assert_eq!(destination.get_pane(target).unwrap().pty.child_id(), child);
        assert_eq!(destination.focused_id(), target);
        assert_eq!(destination.pane_count(), 2);
    }

    #[cfg(unix)]
    #[test]
    fn transfer_guards_do_not_treat_closed_records_as_live_panes_or_anchors() {
        if !std::path::Path::new("/bin/sh").exists() {
            return;
        }
        let shell = ShellConfig {
            program: Some("/bin/sh".into()),
            args: vec!["-c".into(), "sleep 1".into()],
            env: Default::default(),
        };
        let make = || {
            PaneManager::new(
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
        let first_closed = source.split(SplitDirection::Vertical).unwrap();
        let second_closed = source.split(SplitDirection::Horizontal).unwrap();
        source.get_pane_mut(first_closed).unwrap().closed = true;
        source.get_pane_mut(second_closed).unwrap().closed = true;
        assert_eq!(source.pane_count(), 3);
        assert_eq!(source.open_pane_count(), 1);
        assert!(source.detach(PaneId(0)).is_none());
        assert!(source.get_pane(PaneId(0)).is_some());

        let mut destination = make();
        let closed_anchor = destination.split(SplitDirection::Vertical).unwrap();
        destination.get_pane_mut(closed_anchor).unwrap().closed = true;
        let error = destination
            .validate_attach_target(closed_anchor)
            .unwrap_err()
            .to_string();
        assert!(error.contains("is closed"), "{error}");
    }

    #[cfg(unix)]
    #[test]
    fn each_new_pane_cwd_policy_uses_its_declared_process_source() {
        if !std::path::Path::new("/bin/sh").exists() {
            return;
        }
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
        let mut panes = PaneManager::new(
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
        let focused = panes.split(SplitDirection::Vertical).unwrap();
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
                panes.poll_events(|_, _| {});
                let cwd = panes
                    .get_pane(id)
                    .unwrap()
                    .pty
                    .child_id()
                    .and_then(crate::tabs::get_process_cwd);
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
        panes.set_cwd_mode(crate::config::PaneCwdMode::Focused);
        let inherited_focus = panes.split(SplitDirection::Horizontal).unwrap();
        wait_for_cwd(&mut panes, inherited_focus, &reported_focused_dir);

        panes.set_cwd_mode(crate::config::PaneCwdMode::Tab);
        let inherited_tab = panes.split(SplitDirection::Horizontal).unwrap();
        wait_for_cwd(&mut panes, inherited_tab, &reported_primary_dir);

        panes.set_cwd_mode(crate::config::PaneCwdMode::Process);
        let inherited_process = panes.split(SplitDirection::Horizontal).unwrap();
        wait_for_cwd(
            &mut panes,
            inherited_process,
            &std::env::current_dir().unwrap(),
        );
    }

    #[cfg(unix)]
    #[test]
    fn fractional_asymmetric_nested_panes_share_renderer_and_kernel_geometry() {
        if !std::path::Path::new("/bin/sh").exists() {
            return;
        }
        let shell = ShellConfig {
            program: Some("/bin/sh".into()),
            args: vec!["-c".into(), "sleep 1".into()],
            env: Default::default(),
        };
        let mut panes = PaneManager::new(
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
        panes.split(SplitDirection::Vertical).unwrap();
        let divider = panes.get_dividers()[0];
        assert!(
            panes
                .resize_divider_target(divider.target, 337.25, 200.0)
                .is_some()
        );
        panes.split(SplitDirection::Horizontal).unwrap();

        let layout = panes.calculate_layout();
        assert_eq!(layout.len(), 3);
        for (id, rect) in layout {
            let expected = TerminalGeometry::from_available_pixels(
                rect.width,
                rect.height,
                panes.cell_width,
                panes.cell_height,
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

    #[cfg(unix)]
    #[test]
    fn simultaneous_real_pane_dsr_replies_return_to_each_origin() {
        if !std::path::Path::new("/bin/sh").exists() {
            return;
        }
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
        let mut panes = PaneManager::new(
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
        panes.split(SplitDirection::Vertical).unwrap();

        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            let (_, events) = panes.poll_events(|_, _| {});
            assert!(
                events
                    .iter()
                    .all(|event| !matches!(event.event, TerminalEvent::PtyWrite(_)))
            );
            let all_replied = panes.panes.values().all(|pane| {
                pane.terminal
                    .grid()
                    .export_scrollback_text()
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
            let text = pane.terminal.grid().export_scrollback_text().join("\n");
            assert!(
                text.contains("1b 5b 31 3b 31 52"),
                "pane {:?} did not receive its own DSR reply: {text:?}",
                pane.id
            );
            assert!(pane.closed, "pane {:?} did not exit", pane.id);
        }
    }

    #[cfg(unix)]
    #[test]
    fn close_during_event_heavy_output_discards_the_closed_origin_and_drains_survivor() {
        if !std::path::Path::new("/bin/sh").exists() {
            return;
        }
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
        let mut panes = PaneManager::new(
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
        panes.split(SplitDirection::Vertical).unwrap();
        let closed_id = panes.focused_id();
        assert!(panes.close_focused());
        assert!(panes.get_pane(closed_id).is_none());

        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            let (_, events) = panes.poll_events(|_, _| {});
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
}
