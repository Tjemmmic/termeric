//! Single terminal tab
//!
//! A `Tab` wraps a terminal + PTY pair and provides session save/restore support.

use crate::config::ShellConfig;
use crate::session::Session;
use crate::session::transcript::{TranscriptNotice, TranscriptRecorder, TranscriptWrite};
use crate::terminal::container::TerminalContainer;
use crate::terminal::diff_detector::{DiffDetector, strip_ansi};
use crate::terminal::events::{MAX_FORWARDED_TERMINAL_EVENTS_PER_POLL, drain_terminal_events};
use crate::terminal::geometry::TerminalGeometry;
use crate::terminal::shell_integration::{ShellIntegrationSupport, prepare_shell_launch};
use crate::ui::claude_chrome::AiSessionState;
use anyhow::Context;
use std::collections::HashMap;
use std::sync::mpsc::Receiver;
use std::time::Instant;
use termeric_core::pty::{PtyEvent, PtyHandle, spawn_reader_thread_with_waker};
use termeric_core::{Pty, Terminal, TerminalEvent};

use super::title::{
    abbreviate_path, dir_name_from_path, get_foreground_process, get_process_cwd,
    terminal_reported_directory_path, terminal_working_directory,
};

#[derive(Debug, Clone)]
pub struct SessionSaveOutcome {
    pub id: String,
    pub scrollback_truncated: bool,
    pub capture_scope: crate::session::SessionCaptureScope,
}

pub(crate) struct SessionCapture {
    pub(crate) working_directory: Option<String>,
    pub(crate) last_command: Option<String>,
    pub(crate) scrollback: Vec<String>,
    pub(crate) scope: crate::session::SessionCaptureScope,
}

/// Complete ownership bundle for a tab that has not entered pane mode.
pub(crate) struct TabTerminalOwner {
    pub pane_id: crate::panes::PaneId,
    pub terminal: Terminal,
    pub pty: PtyHandle,
    pub pty_rx: Receiver<PtyEvent>,
    pub harvest: crate::harvest::HarvestTerminalState,
    pub shell_integration: ShellIntegrationSupport,
    pub last_command: Option<String>,
    pub suppress_protocol_replies: bool,
    pub read_only: bool,
    pub launch: crate::workspace::WorkspaceLaunch,
    pub launch_shell: ShellConfig,
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
    pub(crate) pane_ids: HashMap<crate::panes::PaneId, u32>,
}

/// Duration of tab slide-in animation in seconds
const TAB_SLIDE_DURATION: f32 = 0.15;

/// Duration of tab close/shrink animation in seconds
const TAB_CLOSE_DURATION: f32 = 0.12;

/// A single terminal tab
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
    /// Terminal state
    terminal: Option<Terminal>,
    /// PTY handle for writing
    pty: Option<PtyHandle>,
    /// PTY output receiver
    pty_rx: Option<Receiver<PtyEvent>>,
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
    /// Last known foreground command (e.g. "cargo"), preserved after command finishes
    pub(crate) last_command: Option<String>,
    /// What kind of tab this is (shell, plugin, etc.)
    pub tab_type: TabType,
    /// Live domain backing this tab, when its PTYs are authenticated relay
    /// processes rather than locally owned terminal children.
    pub(crate) live_domain: Option<LiveDomainTab>,
    /// Diff detector for plugin tabs (watches output for unified diffs)
    pub diff_detector: DiffDetector,
    /// AI session state (for plugin tabs running AI tools like Claude Code, Codex, etc.)
    pub ai_session: Option<AiSessionState>,
    /// Active transcript recorder (writes JSONL to disk while recording)
    pub transcript: Option<TranscriptRecorder>,
    /// Pane manager for split panes (None = single terminal, no splits)
    pub pane_manager: Option<crate::panes::PaneManager>,
    /// Stable tab-local identity retained when pane mode collapses back to one
    /// terminal owner.
    single_pane_id: crate::panes::PaneId,
    /// Transient user-input lock while the tab owns one terminal directly.
    single_read_only: bool,
    /// Harvest cursor/indicator state owned by this terminal.
    pub harvest: crate::harvest::HarvestTerminalState,
    /// Whether Termeric installed semantic prompt hooks for this shell.
    pub shell_integration: ShellIntegrationSupport,
    /// Frozen shell/command profile inherited by future panes in this tab.
    split_shell: ShellConfig,
    /// Launch identity/configuration for the single owner. Pane mode transfers
    /// these fields into the adopted primary pane.
    terminal_launch: crate::workspace::WorkspaceLaunch,
    terminal_launch_shell: ShellConfig,
    user_notices: Vec<String>,
    transcript_failure_reported: bool,
    event_overflow_reported: bool,
}

impl Tab {
    pub(crate) fn child_observer(&self) -> Option<termeric_core::pty::ChildObserver> {
        self.pty.as_ref().map(PtyHandle::child_observer)
    }

    pub(crate) fn new_with_geometry(
        id: usize,
        geometry: TerminalGeometry,
        shell: &ShellConfig,
        scrollback_lines: usize,
        scrollback_bytes: usize,
    ) -> anyhow::Result<Self> {
        let mut terminal = Terminal::new(geometry.cols, geometry.rows);
        terminal.set_pixel_geometry(geometry.pixel_width.into(), geometry.pixel_height.into());
        terminal.set_scrollback_limits(scrollback_lines, scrollback_bytes);
        let launch = prepare_shell_launch(shell);
        let pty = Pty::new_with_env_and_size(
            geometry.pty_size(),
            launch.program.as_deref(),
            &launch.args,
            &launch.env,
        )?;
        terminal.set_term_name(pty.term_name());
        let (handle, reader) = pty.take_reader();
        let rx = spawn_reader_thread_with_waker(reader, crate::app::pty_event_waker());

        // Get initial title from current working directory
        let title = handle
            .child_id()
            .and_then(get_process_cwd)
            .map(|cwd| dir_name_from_path(&cwd))
            .unwrap_or_else(|| format!("Tab {}", id + 1));

        Ok(Self {
            id,
            title,
            user_title: None,
            pinned: false,
            osc_title: None,
            terminal: Some(terminal),
            pty: Some(handle),
            pty_rx: Some(rx),
            geometry,
            has_activity: false,
            closed: false,
            created_at: Instant::now(),
            closing_at: None,
            session_id: None,
            last_command: None,
            tab_type: TabType::Shell,
            live_domain: None,
            diff_detector: DiffDetector::new(),
            ai_session: None,
            transcript: None,
            pane_manager: None,
            single_pane_id: crate::panes::PaneId(0),
            single_read_only: false,
            harvest: crate::harvest::HarvestTerminalState::default(),
            shell_integration: launch.integration,
            split_shell: shell.clone(),
            terminal_launch: crate::workspace::WorkspaceLaunch::Shell,
            terminal_launch_shell: shell.clone(),
            user_notices: Vec::new(),
            transcript_failure_reported: false,
            event_overflow_reported: false,
        })
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
        program: &str,
        args: &[String],
        working_dir: Option<&str>,
        title: Option<&str>,
        scrollback_lines: usize,
        scrollback_bytes: usize,
        tab_type: Option<TabType>,
        extra_env: HashMap<String, String>,
    ) -> anyhow::Result<Self> {
        Self::new_with_command_geometry(
            id,
            TerminalGeometry::from_cells(cols, rows, 1.0, 1.0),
            program,
            args,
            working_dir,
            title,
            scrollback_lines,
            scrollback_bytes,
            tab_type,
            extra_env,
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new_with_command_geometry(
        id: usize,
        geometry: TerminalGeometry,
        program: &str,
        args: &[String],
        working_dir: Option<&str>,
        title: Option<&str>,
        scrollback_lines: usize,
        scrollback_bytes: usize,
        tab_type: Option<TabType>,
        extra_env: HashMap<String, String>,
    ) -> anyhow::Result<Self> {
        Self::new_with_command_environment_policy(
            id,
            geometry,
            program,
            args,
            working_dir,
            title,
            scrollback_lines,
            scrollback_bytes,
            tab_type,
            extra_env,
            &termeric_core::pty::EnvironmentPolicy::default(),
        )
    }

    pub(crate) fn new_with_command_environment_policy(
        id: usize,
        geometry: TerminalGeometry,
        program: &str,
        args: &[String],
        working_dir: Option<&str>,
        title: Option<&str>,
        scrollback_lines: usize,
        scrollback_bytes: usize,
        tab_type: Option<TabType>,
        extra_env: HashMap<String, String>,
        policy: &termeric_core::pty::EnvironmentPolicy,
    ) -> anyhow::Result<Self> {
        let mut terminal = Terminal::new(geometry.cols, geometry.rows);
        terminal.set_pixel_geometry(geometry.pixel_width.into(), geometry.pixel_height.into());
        terminal.set_scrollback_limits(scrollback_lines, scrollback_bytes);

        // Build environment: start with PWD if a working directory was specified,
        // then merge any additional environment variables from the plugin
        let mut env: Vec<(String, String)> = working_dir
            .map(|pwd| vec![("PWD".to_string(), pwd.to_string())])
            .unwrap_or_default();
        for (key, value) in extra_env {
            env.push((key, value));
        }

        let pty = Pty::new_with_environment_policy(
            geometry.pty_size(),
            Some(program),
            args,
            &env,
            policy,
        )?;
        terminal.set_term_name(pty.term_name());
        let (handle, reader) = pty.take_reader();
        let rx = spawn_reader_thread_with_waker(reader, crate::app::pty_event_waker());

        let tab_title = title
            .map(|t| t.to_string())
            .unwrap_or_else(|| program.to_string());

        Ok(Self {
            id,
            title: tab_title,
            user_title: None,
            pinned: false,
            osc_title: None,
            terminal: Some(terminal),
            pty: Some(handle),
            pty_rx: Some(rx),
            geometry,
            has_activity: false,
            closed: false,
            created_at: Instant::now(),
            closing_at: None,
            session_id: None,
            last_command: Some(program.to_string()),
            tab_type: tab_type.clone().unwrap_or(TabType::Shell),
            live_domain: None,
            diff_detector: DiffDetector::new(),
            ai_session: if matches!(tab_type, Some(TabType::Plugin { .. })) {
                Some(AiSessionState::default())
            } else {
                None
            },
            transcript: None,
            pane_manager: None,
            single_pane_id: crate::panes::PaneId(0),
            single_read_only: false,
            harvest: crate::harvest::HarvestTerminalState::default(),
            shell_integration: ShellIntegrationSupport::Unavailable(
                "Direct command tabs do not provide shell prompt marks".into(),
            ),
            split_shell: ShellConfig::default(),
            terminal_launch: match &tab_type {
                Some(TabType::Plugin { plugin_id }) => crate::workspace::WorkspaceLaunch::Plugin {
                    plugin_id: plugin_id.clone(),
                    profile: None,
                },
                _ => crate::workspace::WorkspaceLaunch::Shell,
            },
            terminal_launch_shell: ShellConfig::default(),
            user_notices: Vec::new(),
            transcript_failure_reported: false,
            event_overflow_reported: false,
        })
    }

    /// Poll pending PTY events, consuming protocol replies on this tab's PTY
    /// and returning only bounded app-facing events.
    pub fn poll_events(&mut self) -> (bool, Vec<TerminalEvent>) {
        debug_assert!(self.pane_manager.is_none());
        let mut had_output = self.poll_transcript_notices();
        let was_closed = self.closed;
        let mut app_events = Vec::new();
        let mut reader_disconnected = false;

        loop {
            let event = match self.pty_rx.as_ref().map(Receiver::try_recv) {
                Some(Ok(event)) => event,
                Some(Err(std::sync::mpsc::TryRecvError::Disconnected)) => {
                    reader_disconnected = true;
                    break;
                }
                Some(Err(std::sync::mpsc::TryRecvError::Empty)) | None => break,
            };
            match event {
                PtyEvent::Output(data) => {
                    self.observe_output(&data);
                    if let Some(terminal) = &mut self.terminal {
                        terminal.process(&data);
                    }
                    had_output = true;
                    had_output |= self.drain_terminal_events(&mut app_events);
                }
                PtyEvent::Closed => {
                    log::info!("Tab {} shell exited", self.id);
                    self.closed = true;
                }
                PtyEvent::Error(e) => {
                    log::error!("Tab {} PTY error: {}", self.id, e);
                    self.closed = true;
                }
            }
        }

        // Update title when there's output (directory may have changed)
        if had_output {
            self.update_title();
            self.finish_output_observation();
        }

        // Only use process state after the reader disconnects. A child may
        // exit before its final output reaches this bounded channel.
        if !self.closed
            && reader_disconnected
            && self.pty.as_mut().is_some_and(|pty| !pty.is_running())
        {
            log::info!("Tab {} child process exited (detected via wait)", self.id);
            self.closed = true;
        }

        had_output |= self.drain_terminal_events(&mut app_events);

        (
            had_output || self.closed != was_closed || !app_events.is_empty(),
            app_events,
        )
    }

    /// Observe a raw output chunk for tab-level transcript/diff integrations.
    /// Pane polling calls this same hook without retaining another copy.
    pub(crate) fn observe_output(&mut self, data: &[u8]) {
        if matches!(self.tab_type, TabType::Plugin { .. }) {
            if let Ok(text) = std::str::from_utf8(data) {
                for line in text.lines() {
                    let clean = strip_ansi(line);
                    self.diff_detector.feed_line(&clean);
                }
            }
        }
        if let Some(ref mut recorder) = self.transcript {
            if let Ok(text) = std::str::from_utf8(data) {
                let result = recorder.record_output(text);
                self.handle_transcript_write(result);
            }
        }
    }

    /// Finalize tab-level output observers after one complete poll. Harvest is
    /// state-owned by each terminal and is observed separately by App.
    pub(crate) fn finish_output_observation(&mut self) {
        if !matches!(self.tab_type, TabType::Plugin { .. }) {
            return;
        }
        let pre_count = self.diff_detector.unreviewed_count();
        self.diff_detector.flush();
        if self.diff_detector.unreviewed_count() > pre_count {
            if let Some(ref mut recorder) = self.transcript {
                if let Some(diff) = self.diff_detector.latest_unreviewed() {
                    let result = recorder.record_diff(&diff.filename, &diff.raw);
                    self.handle_transcript_write(result);
                }
            }
        }
    }

    fn drain_terminal_events(&mut self, app_events: &mut Vec<TerminalEvent>) -> bool {
        let (Some(terminal), Some(pty)) = (&mut self.terminal, &mut self.pty) else {
            return false;
        };
        let suppress_protocol_replies = self.live_domain.is_some();
        let result = drain_terminal_events(
            terminal,
            app_events,
            MAX_FORWARDED_TERMINAL_EVENTS_PER_POLL,
            |data| {
                if suppress_protocol_replies {
                    Ok(())
                } else {
                    pty.write_all(data)
                }
            },
        );
        if let Some(error) = result.reply_error {
            log::error!(
                "Tab {} failed to write terminal protocol reply: {}",
                self.id,
                error
            );
            self.closed = true;
        }
        if result.dropped_app_events > 0 && !self.event_overflow_reported {
            self.event_overflow_reported = true;
            log::warn!(
                "Tab {} dropped {} app-facing terminal events after reaching the per-poll limit of {}",
                self.id,
                result.dropped_app_events,
                MAX_FORWARDED_TERMINAL_EVENTS_PER_POLL
            );
        }
        result.saw_event
    }

    /// Read the terminal currently owned by this tab's focused route.
    pub fn focused_terminal(&self) -> Option<&Terminal> {
        if let Some(panes) = &self.pane_manager {
            panes.focused_pane().map(|pane| &pane.terminal)
        } else {
            self.terminal.as_ref()
        }
    }

    /// Mutably access the focused live terminal.
    pub fn focused_terminal_mut(&mut self) -> Option<&mut Terminal> {
        if let Some(panes) = &mut self.pane_manager {
            panes.focused_pane_mut().map(|pane| &mut pane.terminal)
        } else {
            self.terminal.as_mut()
        }
    }

    /// Apply the host's screen-report policy to every terminal owned by this
    /// tab and to terminals created by future pane splits.
    pub(crate) fn set_screen_checksum_reporting(&mut self, allowed: bool) {
        if let Some(panes) = &mut self.pane_manager {
            panes.set_screen_checksum_reporting(allowed);
        } else if let Some(terminal) = &mut self.terminal {
            terminal.set_screen_checksum_reporting(allowed);
        }
    }

    /// Apply the Kitty persistent-file policy to this tab and terminals
    /// created by future pane splits.
    pub(crate) fn set_kitty_file_transmission(&mut self, allowed: bool) {
        // A live-domain replica receives raw terminal bytes from another PTY
        // owner. It must never interpret a daemon's `t=f` path as permission
        // to read a same-named file on the GUI host.
        let allowed = allowed && self.live_domain.is_none();
        if let Some(panes) = &mut self.pane_manager {
            panes.set_kitty_file_transmission(allowed);
        } else if let Some(terminal) = &mut self.terminal {
            terminal.set_kitty_file_transmission(allowed);
        }
    }

    pub(crate) fn set_graphics_relay_snapshot_allowed(&mut self, allowed: bool) {
        if let Some(panes) = &mut self.pane_manager {
            for pane in panes.all_panes_mut() {
                pane.terminal.set_graphics_relay_snapshot_allowed(allowed);
            }
        } else if let Some(terminal) = &mut self.terminal {
            terminal.set_graphics_relay_snapshot_allowed(allowed);
        }
    }

    /// Apply the configured xterm keyboard type to this tab and any panes it
    /// owns. Existing application-selected state is preserved unless the
    /// configuration value itself changed.
    pub(crate) fn set_xterm_keyboard_type(
        &mut self,
        keyboard_type: termeric_core::term::XtermKeyboardType,
        apply_current: bool,
    ) {
        if let Some(panes) = &mut self.pane_manager {
            panes.set_xterm_keyboard_type(keyboard_type, apply_current);
        } else if let Some(terminal) = &mut self.terminal {
            terminal.configure_xterm_keyboard_type(keyboard_type, apply_current);
        }
    }

    pub(crate) fn live_domain_focus_route(&self) -> Option<(&str, u32)> {
        let live = self.live_domain.as_ref()?;
        let local = self
            .pane_manager
            .as_ref()
            .map_or(self.single_pane_id, crate::panes::PaneManager::focused_id);
        Some((live.domain_id.as_str(), *live.pane_ids.get(&local)?))
    }

    pub(crate) fn single_pane_id(&self) -> crate::panes::PaneId {
        self.single_pane_id
    }

    pub(crate) fn set_single_pane_id(&mut self, id: crate::panes::PaneId) {
        debug_assert!(self.pane_manager.is_none());
        self.single_pane_id = id;
    }

    pub(crate) fn focused_pane_read_only(&self) -> bool {
        self.pane_manager
            .as_ref()
            .map_or(self.single_read_only, |panes| {
                panes
                    .get_pane(panes.focused_id())
                    .is_some_and(|pane| pane.read_only)
            })
    }

    pub(crate) fn pane_read_only(&self, pane_id: crate::panes::PaneId) -> Option<bool> {
        if let Some(panes) = &self.pane_manager {
            panes.get_pane(pane_id).map(|pane| pane.read_only)
        } else {
            (pane_id == self.single_pane_id).then_some(self.single_read_only)
        }
    }

    /// Toggle the focused pane's transient user-input lock and return its new
    /// state. The lock follows a live pane when ownership is moved or collapsed.
    pub(crate) fn toggle_focused_pane_read_only(&mut self) -> bool {
        if let Some(panes) = &mut self.pane_manager {
            let focused = panes.focused_id();
            let pane = panes
                .get_pane_mut(focused)
                .expect("focused pane remains present");
            pane.read_only = !pane.read_only;
            pane.read_only
        } else {
            self.single_read_only = !self.single_read_only;
            self.single_read_only
        }
    }

    pub(crate) fn live_domain_tab_route(&self) -> Option<(&str, u32)> {
        let live = self.live_domain.as_ref()?;
        Some((live.domain_id.as_str(), live.tab_id))
    }

    /// Observe semantic command regions for the single live terminal. Pane
    /// callers keep per-pane Harvest state beside each pane instead.
    pub fn observe_harvest(&mut self, show_indicators: bool) -> Vec<termeric_core::CommandRegion> {
        debug_assert!(self.pane_manager.is_none());
        let Some(terminal) = &self.terminal else {
            return Vec::new();
        };
        self.harvest.observe(
            &terminal.shell_integration().prompt_tracker,
            show_indicators,
        )
    }

    /// Write user/application input to the focused live PTY and record it once
    /// at tab transcript scope.
    pub fn write(&mut self, data: &[u8]) -> std::io::Result<()> {
        if self.focused_pane_read_only() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "pane is read-only",
            ));
        }
        if let Some(ref mut recorder) = self.transcript {
            if let Ok(text) = std::str::from_utf8(data) {
                let result = recorder.record_input(text);
                self.handle_transcript_write(result);
            }
        }
        if let Some(panes) = &mut self.pane_manager {
            panes
                .focused_pane_mut()
                .ok_or_else(|| {
                    std::io::Error::new(
                        std::io::ErrorKind::NotFound,
                        "focused pane no longer exists",
                    )
                })?
                .write(data)
        } else {
            self.pty
                .as_mut()
                .ok_or_else(|| {
                    std::io::Error::new(
                        std::io::ErrorKind::NotFound,
                        "single terminal owner is detached",
                    )
                })?
                .write_all(data)
        }
    }

    pub(crate) fn write_protocol_reply(&mut self, data: &[u8]) -> std::io::Result<()> {
        if let Some(panes) = &mut self.pane_manager {
            panes
                .focused_pane_mut()
                .ok_or_else(|| {
                    std::io::Error::new(
                        std::io::ErrorKind::NotFound,
                        "focused pane no longer exists",
                    )
                })?
                .write_protocol_reply(data)
        } else {
            self.pty
                .as_mut()
                .ok_or_else(|| {
                    std::io::Error::new(
                        std::io::ErrorKind::NotFound,
                        "single terminal owner is detached",
                    )
                })?
                .write_all(data)
        }
    }

    /// Resize the terminal model and PTY from one canonical geometry.
    pub(crate) fn resize_geometry(&mut self, geometry: TerminalGeometry) -> anyhow::Result<()> {
        if self.geometry == geometry {
            return Ok(());
        }
        if (self.geometry.cols, self.geometry.rows) != (geometry.cols, geometry.rows) {
            self.terminal
                .as_mut()
                .context("single terminal owner is detached")?
                .resize(geometry.cols, geometry.rows);
        }
        self.terminal
            .as_mut()
            .context("single terminal owner is detached")?
            .set_pixel_geometry(geometry.pixel_width.into(), geometry.pixel_height.into());
        self.pty
            .as_ref()
            .context("single PTY owner is detached")?
            .resize(geometry.pty_size())?;
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
        let child_id = self
            .pane_manager
            .as_ref()
            .and_then(|panes| panes.focused_pane())
            .and_then(|pane| pane.pty.child_id())
            .or_else(|| {
                self.pane_manager
                    .is_none()
                    .then(|| self.pty.as_ref().and_then(PtyHandle::child_id))
                    .flatten()
            });
        if let Some(pid) = child_id {
            if let Some(cwd) = get_process_cwd(pid) {
                self.title = dir_name_from_path(&cwd);

                // Track the last foreground command (preserved after it exits)
                if self.pane_manager.is_none()
                    && let Some(cmd) = get_foreground_process(pid)
                {
                    self.last_command = Some(cmd);
                }
            }
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

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new_with_cwd_geometry(
        id: usize,
        geometry: TerminalGeometry,
        shell: &ShellConfig,
        scrollback_lines: usize,
        scrollback_bytes: usize,
        working_dir: Option<&str>,
        title: Option<&str>,
    ) -> anyhow::Result<Self> {
        let mut terminal = Terminal::new(geometry.cols, geometry.rows);
        terminal.set_pixel_geometry(geometry.pixel_width.into(), geometry.pixel_height.into());
        terminal.set_scrollback_limits(scrollback_lines, scrollback_bytes);

        let mut launch = prepare_shell_launch(shell);
        // Build environment variables for session restore
        let mut env: Vec<(String, String)> = working_dir
            .map(|pwd| vec![("PWD".to_string(), pwd.to_string())])
            .unwrap_or_default();
        env.append(&mut launch.env);

        let pty = Pty::new_with_env_and_size(
            geometry.pty_size(),
            launch.program.as_deref(),
            &launch.args,
            &env,
        )?;
        terminal.set_term_name(pty.term_name());
        let (handle, reader) = pty.take_reader();
        let rx = spawn_reader_thread_with_waker(reader, crate::app::pty_event_waker());

        // Use provided title or derive from working directory
        let tab_title = title
            .map(|t| t.to_string())
            .or_else(|| working_dir.map(dir_name_from_path))
            .or_else(|| {
                handle
                    .child_id()
                    .and_then(get_process_cwd)
                    .map(|cwd| dir_name_from_path(&cwd))
            })
            .unwrap_or_else(|| format!("Tab {}", id + 1));

        Ok(Self {
            id,
            title: tab_title,
            user_title: None,
            pinned: false,
            osc_title: None,
            terminal: Some(terminal),
            pty: Some(handle),
            pty_rx: Some(rx),
            geometry,
            has_activity: false,
            closed: false,
            created_at: Instant::now(),
            closing_at: None,
            session_id: None,
            last_command: None,
            tab_type: TabType::Shell,
            live_domain: None,
            diff_detector: DiffDetector::new(),
            ai_session: None,
            transcript: None,
            pane_manager: None,
            single_pane_id: crate::panes::PaneId(0),
            single_read_only: false,
            harvest: crate::harvest::HarvestTerminalState::default(),
            shell_integration: launch.integration,
            split_shell: shell.clone(),
            terminal_launch: crate::workspace::WorkspaceLaunch::Shell,
            terminal_launch_shell: shell.clone(),
            user_notices: Vec::new(),
            transcript_failure_reported: false,
            event_overflow_reported: false,
        })
    }

    /// Get the current working directory of this tab's shell process
    pub fn working_directory(&self) -> Option<String> {
        if matches!(self.tab_type, TabType::Serial { .. }) {
            return None;
        }
        if let Some(cwd) = self.focused_terminal().and_then(terminal_working_directory) {
            return Some(cwd);
        }
        let child_id = self
            .pane_manager
            .as_ref()
            .and_then(|panes| panes.focused_pane())
            .and_then(|pane| pane.pty.child_id())
            .or_else(|| {
                if self.pane_manager.is_none() {
                    self.pty.as_ref().and_then(PtyHandle::child_id)
                } else {
                    None
                }
            });
        child_id.and_then(get_process_cwd)
    }

    pub(crate) fn split_shell_config(&self) -> ShellConfig {
        self.split_shell.clone()
    }

    pub(crate) fn set_split_shell_config(&mut self, shell: ShellConfig) {
        if self.pane_manager.is_none()
            && matches!(
                self.terminal_launch,
                crate::workspace::WorkspaceLaunch::Shell
            )
        {
            self.terminal_launch_shell = shell.clone();
        }
        self.split_shell = shell;
    }

    pub(crate) fn set_terminal_profile_launch(&mut self, name: &str, shell: ShellConfig) {
        self.terminal_launch = crate::workspace::WorkspaceLaunch::Profile {
            name: name.to_string(),
        };
        self.terminal_launch_shell = shell.clone();
        self.split_shell = shell;
    }

    pub(crate) fn set_serial_profile_launch(&mut self, name: &str, title: String) {
        self.tab_type = TabType::Serial {
            name: name.to_string(),
        };
        self.terminal_launch = crate::workspace::WorkspaceLaunch::Profile {
            name: name.to_string(),
        };
        self.terminal_launch_shell = ShellConfig::default();
        self.split_shell = ShellConfig::default();
        self.last_command = None;
        self.set_user_title(Some(title));
        self.shell_integration = ShellIntegrationSupport::Unavailable(
            "Serial terminals do not provide shell prompt marks".into(),
        );
    }

    pub(crate) fn allows_pane_splits(&self) -> bool {
        !matches!(self.tab_type, TabType::Serial { .. })
    }

    pub(crate) fn set_plugin_profile(&mut self, profile: String) {
        if let Some(session) = &mut self.ai_session {
            session.profile.clone_from(&profile);
        }
        let saved_profile = (profile != "default").then_some(profile);
        let update = |launch: &mut crate::workspace::WorkspaceLaunch| {
            if let crate::workspace::WorkspaceLaunch::Plugin {
                profile: current, ..
            } = launch
            {
                *current = saved_profile.clone();
            }
        };
        if let Some(panes) = &mut self.pane_manager {
            panes.update_plugin_launch_profile(saved_profile);
        } else {
            update(&mut self.terminal_launch);
        }
    }

    /// Detach the single-terminal owner exactly once before entering pane mode.
    pub(crate) fn take_terminal_owner(&mut self) -> Option<TabTerminalOwner> {
        if self.pane_manager.is_some()
            || self.terminal.is_none()
            || self.pty.is_none()
            || self.pty_rx.is_none()
        {
            return None;
        }
        Some(TabTerminalOwner {
            pane_id: self.single_pane_id,
            terminal: self.terminal.take().unwrap(),
            pty: self.pty.take().unwrap(),
            pty_rx: self.pty_rx.take().unwrap(),
            harvest: std::mem::take(&mut self.harvest),
            shell_integration: std::mem::replace(
                &mut self.shell_integration,
                ShellIntegrationSupport::Unavailable("terminal is owned by pane mode".into()),
            ),
            last_command: self.last_command.take(),
            suppress_protocol_replies: self.live_domain.is_some(),
            read_only: self.single_read_only,
            launch: self.terminal_launch.clone(),
            launch_shell: self.terminal_launch_shell.clone(),
        })
    }

    /// Restore the final pane as the tab's single live terminal owner.
    pub(crate) fn restore_terminal_owner(&mut self, pane: crate::panes::Pane) {
        assert!(self.terminal.is_none());
        assert!(self.pty.is_none());
        assert!(self.pty_rx.is_none());
        self.single_pane_id = pane.id;
        self.single_read_only = pane.read_only;
        self.closed = pane.closed;
        self.geometry = pane.geometry;
        self.terminal = Some(pane.terminal);
        self.pty = Some(pane.pty);
        self.pty_rx = Some(pane.pty_rx);
        self.harvest = pane.harvest;
        self.shell_integration = pane.shell_integration;
        self.last_command = pane.last_command;
        self.terminal_launch = pane.launch;
        self.terminal_launch_shell = pane.launch_shell;
        match &self.terminal_launch {
            crate::workspace::WorkspaceLaunch::Shell => {
                self.tab_type = TabType::Shell;
                self.ai_session = None;
                self.split_shell = self.terminal_launch_shell.clone();
            }
            crate::workspace::WorkspaceLaunch::Profile { name } => {
                self.tab_type = TabType::Profile { name: name.clone() };
                self.ai_session = None;
                self.split_shell = self.terminal_launch_shell.clone();
            }
            crate::workspace::WorkspaceLaunch::Plugin { plugin_id, .. } => {
                self.tab_type = TabType::Plugin {
                    plugin_id: plugin_id.clone(),
                };
            }
        }
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
                self.ai_session = None;
                self.split_shell = shell;
            }
            crate::workspace::WorkspaceLaunch::Profile { name } => {
                self.tab_type = TabType::Profile { name };
                self.ai_session = None;
                self.split_shell = shell;
            }
            crate::workspace::WorkspaceLaunch::Plugin { plugin_id, .. } => {
                self.tab_type = TabType::Plugin { plugin_id };
            }
        }
    }

    #[cfg(test)]
    pub(crate) fn has_single_terminal_owner(&self) -> bool {
        self.terminal.is_some() && self.pty.is_some() && self.pty_rx.is_some()
    }

    #[cfg(test)]
    pub(crate) fn single_child_id(&self) -> Option<u32> {
        self.pty.as_ref().and_then(PtyHandle::child_id)
    }

    #[cfg(test)]
    pub(crate) fn single_pty_size(&self) -> Option<termeric_core::pty::PtySize> {
        self.pty.as_ref()?.size().ok()
    }

    fn handle_transcript_write(&mut self, result: std::io::Result<TranscriptWrite>) {
        match result {
            Ok(TranscriptWrite::Backpressure) => self.user_notices.push(
                "Transcript writer is behind; recording dropped data instead of blocking the terminal"
                    .into(),
            ),
            Err(error) if !self.transcript_failure_reported => {
                self.transcript_failure_reported = true;
                self.user_notices
                    .push(format!("Transcript recording failed: {error}"));
            }
            _ => {}
        }
    }

    fn poll_transcript_notices(&mut self) -> bool {
        let Some(recorder) = &mut self.transcript else {
            return false;
        };
        let notices = recorder.poll_notices();
        for notice in &notices {
            match notice {
                TranscriptNotice::CapReached { max_bytes } => self.user_notices.push(format!(
                    "Transcript reached its {} MiB cap; recording stopped before data was truncated",
                    max_bytes / (1024 * 1024)
                )),
                TranscriptNotice::Failed(error) if !self.transcript_failure_reported => {
                    self.transcript_failure_reported = true;
                    self.user_notices.push(format!(
                        "Transcript recording failed; output may be incomplete: {error}"
                    ));
                }
                TranscriptNotice::Failed(_) => {}
            }
        }
        if !notices.is_empty()
            && let Some(session) = &mut self.ai_session
        {
            session.recording = false;
        }
        !notices.is_empty()
    }

    /// Attach a fresh recorder and reset per-recorder error reporting.
    pub fn start_transcript(&mut self, recorder: TranscriptRecorder) {
        self.transcript = Some(recorder);
        self.transcript_failure_reported = false;
    }

    /// Stop recording and return the recorder so the caller can report its path.
    pub fn stop_transcript(&mut self) -> Option<TranscriptRecorder> {
        self.transcript.take()
    }

    pub fn drain_user_notices(&mut self) -> Vec<String> {
        std::mem::take(&mut self.user_notices)
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
            capture_scope: session.capture_scope,
        })
    }

    fn focused_last_command(&self) -> Option<&String> {
        self.pane_manager
            .as_ref()
            .and_then(|panes| panes.focused_pane())
            .and_then(|pane| pane.last_command.as_ref())
            .or_else(|| {
                self.pane_manager
                    .is_none()
                    .then_some(self.last_command.as_ref())
                    .flatten()
            })
    }

    /// Capture every persisted field from one stable focused-terminal source.
    /// This is the sole policy boundary used by explicit, automatic, and
    /// close-time saves.
    pub(crate) fn capture_focused_session(
        &self,
        save_scrollback: bool,
    ) -> anyhow::Result<SessionCapture> {
        let (terminal, child_id, remembered_command, scope) =
            if let Some(panes) = &self.pane_manager {
                let pane = panes
                    .focused_pane()
                    .context("focused pane no longer exists while saving session")?;
                (
                    &pane.terminal,
                    pane.pty.child_id(),
                    pane.last_command.clone(),
                    crate::session::SessionCaptureScope::FocusedPane,
                )
            } else {
                (
                    self.terminal
                        .as_ref()
                        .context("single terminal owner is detached while saving session")?,
                    self.pty.as_ref().and_then(PtyHandle::child_id),
                    self.last_command.clone(),
                    crate::session::SessionCaptureScope::SingleTerminal,
                )
            };

        Ok(SessionCapture {
            working_directory: terminal_working_directory(terminal)
                .or_else(|| child_id.and_then(get_process_cwd)),
            last_command: child_id
                .and_then(get_foreground_process)
                .or(remembered_command),
            scrollback: if save_scrollback {
                terminal.grid().export_scrollback_text()
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
                let terminal = self
                    .terminal
                    .as_ref()
                    .context("single terminal owner is detached while saving workspace")?;
                let child_id = self.pty.as_ref().and_then(PtyHandle::child_id);
                let pane_id = self.single_pane_id.0;
                (
                    pane_id,
                    pane_id,
                    crate::workspace::WorkspacePaneLayout::Pane { pane: pane_id },
                    vec![crate::workspace::WorkspacePaneSnapshot {
                        id: pane_id,
                        launch: Some(self.terminal_launch.clone()),
                        title: Some(self.title.clone()),
                        working_directory: terminal_working_directory(terminal)
                            .or_else(|| child_id.and_then(get_process_cwd)),
                        last_command: child_id
                            .and_then(get_foreground_process)
                            .or_else(|| self.last_command.clone()),
                        scrollback_file: None,
                        scrollback_truncated: false,
                        scrollback_lines: if save_scrollback {
                            terminal.grid().export_scrollback_text()
                        } else {
                            Vec::new()
                        },
                    }],
                    self.terminal_launch.clone(),
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

/// Single-terminal tab implements TerminalContainer for uniform access.
impl TerminalContainer for Tab {
    fn focused_terminal(&self) -> Option<&Terminal> {
        Tab::focused_terminal(self)
    }

    fn focused_terminal_mut(&mut self) -> Option<&mut Terminal> {
        Tab::focused_terminal_mut(self)
    }

    fn write_input(&mut self, data: &[u8]) -> std::io::Result<()> {
        self.write(data)
    }

    fn resize(&mut self, cols: usize, rows: usize) -> anyhow::Result<()> {
        let cell_width = f32::from(self.geometry.pixel_width) / self.geometry.cols as f32;
        let cell_height = f32::from(self.geometry.pixel_height) / self.geometry.rows as f32;
        self.resize_geometry(TerminalGeometry::from_cells(
            cols,
            rows,
            cell_width,
            cell_height,
        ))
    }

    fn is_closed(&self) -> bool {
        self.closed
    }
}
