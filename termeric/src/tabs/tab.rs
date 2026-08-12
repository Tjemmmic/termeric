//! Single terminal tab
//!
//! A `Tab` wraps a terminal + PTY pair and provides session save/restore support.

use crate::config::ShellConfig;
use crate::session::Session;
use crate::session::transcript::TranscriptRecorder;
use crate::session::transcript::TranscriptWrite;
use crate::terminal::container::TerminalContainer;
use crate::terminal::diff_detector::{DiffDetector, strip_ansi};
use crate::terminal::shell_integration::{ShellIntegrationSupport, prepare_shell_launch};
use crate::ui::claude_chrome::AiSessionState;
use anyhow::Context;
use std::collections::HashMap;
use std::sync::mpsc::Receiver;
use std::time::Instant;
use termeric_core::pty::{PtyEvent, PtyHandle, spawn_reader_thread};
use termeric_core::{Pty, Terminal, TerminalEvent};

use super::title::{abbreviate_path, dir_name_from_path, get_foreground_process, get_process_cwd};

#[derive(Debug, Clone)]
pub struct SessionSaveOutcome {
    pub id: String,
    pub scrollback_truncated: bool,
}

/// What kind of tab this is
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TabType {
    /// Regular shell terminal
    Shell,
    /// Plugin-spawned tab (e.g., Claude Code)
    Plugin { plugin_id: String },
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
    /// Terminal state
    pub terminal: Terminal,
    /// PTY handle for writing
    pub pty: PtyHandle,
    /// PTY output receiver
    pub pty_rx: Receiver<PtyEvent>,
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
    /// Diff detector for plugin tabs (watches output for unified diffs)
    pub diff_detector: DiffDetector,
    /// AI session state (for plugin tabs running AI tools like Claude Code, Codex, etc.)
    pub ai_session: Option<AiSessionState>,
    /// Active transcript recorder (writes JSONL to disk while recording)
    pub transcript: Option<TranscriptRecorder>,
    /// Pane manager for split panes (None = single terminal, no splits)
    pub pane_manager: Option<crate::panes::PaneManager>,
    /// Harvest cursor/indicator state owned by this terminal.
    pub harvest: crate::harvest::HarvestTerminalState,
    /// Whether Termeric installed semantic prompt hooks for this shell.
    pub shell_integration: ShellIntegrationSupport,
    user_notices: Vec<String>,
    transcript_failure_reported: bool,
}

impl Tab {
    /// Create a new tab with a shell
    pub fn new(
        id: usize,
        cols: usize,
        rows: usize,
        shell: &ShellConfig,
        scrollback_lines: usize,
    ) -> anyhow::Result<Self> {
        let mut terminal = Terminal::new(cols, rows);
        terminal.grid_mut().set_scrollback_limit(scrollback_lines);
        let launch = prepare_shell_launch(shell);
        let pty = Pty::new_with_env(
            cols as u16,
            rows as u16,
            launch.program.as_deref(),
            &launch.args,
            &launch.env,
        )?;
        let (handle, reader) = pty.take_reader();
        let rx = spawn_reader_thread(reader);

        // Get initial title from current working directory
        let title = handle
            .child_id()
            .and_then(get_process_cwd)
            .map(|cwd| dir_name_from_path(&cwd))
            .unwrap_or_else(|| format!("Tab {}", id + 1));

        Ok(Self {
            id,
            title,
            terminal,
            pty: handle,
            pty_rx: rx,
            has_activity: false,
            closed: false,
            created_at: Instant::now(),
            closing_at: None,
            session_id: None,
            last_command: None,
            tab_type: TabType::Shell,
            diff_detector: DiffDetector::new(),
            ai_session: None,
            transcript: None,
            pane_manager: None,
            harvest: crate::harvest::HarvestTerminalState::default(),
            shell_integration: launch.integration,
            user_notices: Vec::new(),
            transcript_failure_reported: false,
        })
    }

    /// Create a new tab running a specific command (not a shell).
    ///
    /// The program runs directly as the tab's process — no shell prompt or
    /// command echo. When the program exits, the tab closes.
    pub fn new_with_command(
        id: usize,
        cols: usize,
        rows: usize,
        program: &str,
        args: &[String],
        working_dir: Option<&str>,
        title: Option<&str>,
        scrollback_lines: usize,
        tab_type: Option<TabType>,
        extra_env: HashMap<String, String>,
    ) -> anyhow::Result<Self> {
        let mut terminal = Terminal::new(cols, rows);
        terminal.grid_mut().set_scrollback_limit(scrollback_lines);

        // Build environment: start with PWD if a working directory was specified,
        // then merge any additional environment variables from the plugin
        let mut env: Vec<(String, String)> = working_dir
            .map(|pwd| vec![("PWD".to_string(), pwd.to_string())])
            .unwrap_or_default();
        for (key, value) in extra_env {
            env.push((key, value));
        }

        let pty = Pty::new_with_env(cols as u16, rows as u16, Some(program), args, &env)?;
        let (handle, reader) = pty.take_reader();
        let rx = spawn_reader_thread(reader);

        let tab_title = title
            .map(|t| t.to_string())
            .unwrap_or_else(|| program.to_string());

        Ok(Self {
            id,
            title: tab_title,
            terminal,
            pty: handle,
            pty_rx: rx,
            has_activity: false,
            closed: false,
            created_at: Instant::now(),
            closing_at: None,
            session_id: None,
            last_command: Some(program.to_string()),
            tab_type: tab_type.clone().unwrap_or(TabType::Shell),
            diff_detector: DiffDetector::new(),
            ai_session: if matches!(tab_type, Some(TabType::Plugin { .. })) {
                Some(AiSessionState::default())
            } else {
                None
            },
            transcript: None,
            pane_manager: None,
            harvest: crate::harvest::HarvestTerminalState::default(),
            shell_integration: ShellIntegrationSupport::Unavailable(
                "Direct command tabs do not provide shell prompt marks".into(),
            ),
            user_notices: Vec::new(),
            transcript_failure_reported: false,
        })
    }

    /// Process any pending PTY events, returns (had_output, terminal_events)
    pub fn process_events(&mut self) -> (bool, Vec<TerminalEvent>) {
        let mut had_output = false;

        while let Ok(event) = self.pty_rx.try_recv() {
            match event {
                PtyEvent::Output(data) => {
                    // Feed output to diff detector for plugin tabs
                    if matches!(self.tab_type, TabType::Plugin { .. }) {
                        if let Ok(text) = std::str::from_utf8(&data) {
                            for line in text.lines() {
                                let clean = strip_ansi(line);
                                self.diff_detector.feed_line(&clean);
                            }
                        }
                    }
                    // Record to transcript if active
                    if let Some(ref mut recorder) = self.transcript {
                        if let Ok(text) = std::str::from_utf8(&data) {
                            let result = recorder.record_output(text);
                            self.handle_transcript_write(result);
                        }
                    }
                    self.terminal.process(&data);
                    had_output = true;
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
            // Flush the diff detector to finalize any in-progress diffs
            if matches!(self.tab_type, TabType::Plugin { .. }) {
                let pre_count = self.diff_detector.unreviewed_count();
                self.diff_detector.flush();
                // Record newly detected diffs to transcript
                if self.diff_detector.unreviewed_count() > pre_count {
                    if let Some(ref mut recorder) = self.transcript {
                        if let Some(diff) = self.diff_detector.latest_unreviewed() {
                            let result = recorder.record_diff(&diff.filename, &diff.raw);
                            self.handle_transcript_write(result);
                        }
                    }
                }
            }
        }

        // Fallback: detect child process exit even if the reader thread
        // didn't send PtyEvent::Closed (can happen with dup'd master fds
        // or when a dead pane is transplanted back onto the tab).
        if !self.closed && !self.pty.is_running() {
            log::info!("Tab {} child process exited (detected via wait)", self.id);
            self.closed = true;
        }

        // Drain any terminal events that were emitted during processing
        let mut events = self.terminal.drain_events();

        // Handle PtyWrite events locally — write response back to this tab's PTY
        events.retain(|ev| {
            if let TerminalEvent::PtyWrite(data) = ev {
                if let Err(e) = self.pty.write_all(data) {
                    log::error!("Tab {} failed to write DSR response: {}", self.id, e);
                }
                false // consumed, don't forward
            } else {
                true // forward to app
            }
        });

        (had_output, events)
    }

    /// Write data to the PTY
    pub fn write(&mut self, data: &[u8]) -> std::io::Result<()> {
        if let Some(ref mut recorder) = self.transcript {
            if let Ok(text) = std::str::from_utf8(data) {
                let result = recorder.record_input(text);
                self.handle_transcript_write(result);
            }
        }
        self.pty.write_all(data)
    }

    /// Resize the terminal and PTY
    pub fn resize(&mut self, cols: usize, rows: usize) -> anyhow::Result<()> {
        self.terminal.resize(cols, rows);
        self.pty.resize(cols as u16, rows as u16)?;
        Ok(())
    }

    /// Update tab title based on the shell's current working directory
    pub fn update_title(&mut self) {
        if let Some(pid) = self.pty.child_id() {
            if let Some(cwd) = get_process_cwd(pid) {
                self.title = dir_name_from_path(&cwd);

                // Track the last foreground command (preserved after it exits)
                if let Some(cmd) = get_foreground_process(pid) {
                    self.last_command = Some(cmd);
                }
            }
        }
    }

    /// Create a new tab with an optional working directory (for session restore)
    pub fn new_with_cwd(
        id: usize,
        cols: usize,
        rows: usize,
        shell: &ShellConfig,
        scrollback_lines: usize,
        working_dir: Option<&str>,
        title: Option<&str>,
    ) -> anyhow::Result<Self> {
        let mut terminal = Terminal::new(cols, rows);
        terminal.grid_mut().set_scrollback_limit(scrollback_lines);

        let mut launch = prepare_shell_launch(shell);
        // Build environment variables for session restore
        let mut env: Vec<(String, String)> = working_dir
            .map(|pwd| vec![("PWD".to_string(), pwd.to_string())])
            .unwrap_or_default();
        env.append(&mut launch.env);

        let pty = Pty::new_with_env(
            cols as u16,
            rows as u16,
            launch.program.as_deref(),
            &launch.args,
            &env,
        )?;
        let (handle, reader) = pty.take_reader();
        let rx = spawn_reader_thread(reader);

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
            terminal,
            pty: handle,
            pty_rx: rx,
            has_activity: false,
            closed: false,
            created_at: Instant::now(),
            closing_at: None,
            session_id: None,
            last_command: None,
            tab_type: TabType::Shell,
            diff_detector: DiffDetector::new(),
            ai_session: None,
            transcript: None,
            pane_manager: None,
            harvest: crate::harvest::HarvestTerminalState::default(),
            shell_integration: launch.integration,
            user_notices: Vec::new(),
            transcript_failure_reported: false,
        })
    }

    /// Get the current working directory of this tab's shell process
    pub fn working_directory(&self) -> Option<String> {
        self.pty.child_id().and_then(get_process_cwd)
    }

    fn handle_transcript_write(&mut self, result: std::io::Result<TranscriptWrite>) {
        match result {
            Ok(TranscriptWrite::CapReached) => self.user_notices.push(
                "Transcript reached 50 MB; recording stopped before data was truncated".into(),
            ),
            Err(error) if !self.transcript_failure_reported => {
                self.transcript_failure_reported = true;
                self.user_notices
                    .push(format!("Transcript recording failed: {error}"));
            }
            _ => {}
        }
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
        self.last_command.is_some()
            || self.session_id.is_some()
            || self.terminal.grid().scrollback_len() > 0
    }

    /// Save this tab as a session
    ///
    /// If the tab already has a session_id, updates that session.
    /// Otherwise, creates a new session with a generated ID.
    /// Returns a report that includes any scrollback truncation.
    pub fn save_as_session(&mut self, save_scrollback: bool) -> anyhow::Result<SessionSaveOutcome> {
        let working_dir = self.working_directory();

        // Build session title: "~/path | last_command" or "~/path"
        let path = working_dir
            .as_deref()
            .map(abbreviate_path)
            .unwrap_or_else(|| self.title.clone());
        let session_title = match &self.last_command {
            Some(cmd) => format!("{} | {}", path, cmd),
            None => path,
        };

        let mut session = if let Some(ref id) = self.session_id {
            // Update existing session
            Session::with_id(id.clone(), session_title, working_dir)
        } else {
            // Create new session
            let s = Session::new(session_title, working_dir);
            self.session_id = Some(s.id.clone());
            s
        };

        // Preserve last_command in the session metadata
        session.last_command = self.last_command.clone();

        let session_id = session.id.clone();
        let mut scrollback_truncated = false;

        // Save scrollback
        if save_scrollback {
            let lines = self.terminal.grid().export_scrollback_text();
            if !lines.is_empty() {
                match crate::session::save::save_scrollback_text(&session_id, &lines) {
                    Ok(report) => {
                        session.scrollback_file = Some(report.filename);
                        scrollback_truncated = report.truncated;
                    }
                    Err(e) => {
                        return Err(
                            e.context(format!("save scrollback for session '{session_id}'"))
                        );
                    }
                }
            }
        }

        // Save session metadata
        crate::session::save::save_session(&session)
            .context(format!("save metadata for session '{session_id}'"))?;
        Ok(SessionSaveOutcome {
            id: session_id,
            scrollback_truncated,
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
        Some(&self.terminal)
    }

    fn focused_terminal_mut(&mut self) -> Option<&mut Terminal> {
        Some(&mut self.terminal)
    }

    fn write_input(&mut self, data: &[u8]) -> std::io::Result<()> {
        self.write(data)
    }

    fn process_events(&mut self) -> (bool, Vec<TerminalEvent>) {
        Tab::process_events(self)
    }

    fn resize(&mut self, cols: usize, rows: usize) -> anyhow::Result<()> {
        Tab::resize(self, cols, rows)
    }

    fn is_closed(&self) -> bool {
        self.closed
    }
}
