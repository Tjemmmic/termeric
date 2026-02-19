//! Tab management for multiple terminal instances
//!
//! Provides support for tabbed terminal sessions within a single window.

use std::path::Path;
use std::sync::mpsc::Receiver;
use std::time::Instant;
use termeric_core::{Pty, Terminal};
use termeric_core::pty::{PtyEvent, PtyHandle, spawn_reader_thread};
use crate::config::ShellConfig;

/// Duration of tab slide-in animation in seconds
const TAB_SLIDE_DURATION: f32 = 0.15;

/// Duration of tab close/shrink animation in seconds
const TAB_CLOSE_DURATION: f32 = 0.12;

/// Get the current working directory of a process by its PID
#[cfg(target_os = "linux")]
fn get_process_cwd(pid: u32) -> Option<String> {
    let cwd_path = format!("/proc/{}/cwd", pid);
    std::fs::read_link(&cwd_path)
        .ok()
        .and_then(|p| p.to_str().map(|s| s.to_string()))
}

#[cfg(not(target_os = "linux"))]
fn get_process_cwd(_pid: u32) -> Option<String> {
    None
}

/// Extract the directory name from a path, showing ~ for home directory
fn dir_name_from_path(path: &str) -> String {
    // Check if this is the home directory
    if let Some(home) = std::env::var_os("HOME") {
        if path == home.to_str().unwrap_or("") {
            return "~".to_string();
        }
    }

    Path::new(path)
        .file_name()
        .and_then(|n| n.to_str())
        .map(|s| s.to_string())
        .unwrap_or_else(|| path.to_string())
}

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
}

impl Tab {
    /// Create a new tab with a shell
    pub fn new(id: usize, cols: usize, rows: usize, shell: &ShellConfig, scrollback_lines: usize) -> anyhow::Result<Self> {
        let mut terminal = Terminal::new(cols, rows);
        terminal.grid_mut().set_scrollback_limit(scrollback_lines);
        let pty = Pty::new_with_command(
            cols as u16,
            rows as u16,
            shell.program.as_deref(),
            &shell.args,
        )?;
        let (handle, reader) = pty.take_reader();
        let rx = spawn_reader_thread(reader);

        // Get initial title from current working directory
        let title = handle.child_id()
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
        })
    }

    /// Process any pending PTY events
    pub fn process_events(&mut self) -> bool {
        let mut had_output = false;

        while let Ok(event) = self.pty_rx.try_recv() {
            match event {
                PtyEvent::Output(data) => {
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
        }

        had_output
    }

    /// Write data to the PTY
    pub fn write(&mut self, data: &[u8]) -> std::io::Result<()> {
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
            }
        }
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

/// Manager for multiple tabs
pub struct TabManager {
    /// All tabs
    tabs: Vec<Tab>,
    /// Currently active tab index
    active: usize,
    /// Next tab ID to assign
    next_id: usize,
    /// Terminal dimensions
    cols: usize,
    rows: usize,
    /// Shell configuration for new tabs
    shell: ShellConfig,
    /// Scrollback limit for new tabs
    scrollback_lines: usize,
}

impl TabManager {
    /// Create a new tab manager
    pub fn new(cols: usize, rows: usize, shell: ShellConfig, scrollback_lines: usize) -> Self {
        Self {
            tabs: Vec::new(),
            active: 0,
            next_id: 0,
            cols,
            rows,
            shell,
            scrollback_lines,
        }
    }

    /// Create a new tab and make it active
    pub fn new_tab(&mut self) -> anyhow::Result<usize> {
        let id = self.next_id;
        self.next_id += 1;

        let tab = Tab::new(id, self.cols, self.rows, &self.shell, self.scrollback_lines)?;
        self.tabs.push(tab);
        self.active = self.tabs.len() - 1;

        log::info!("Created new tab {}", id);
        Ok(id)
    }

    /// Start closing a tab by index (begins shrink animation)
    pub fn close_tab(&mut self, index: usize) -> bool {
        if index >= self.tabs.len() {
            return false;
        }

        // Don't close if already closing
        if self.tabs[index].is_closing() {
            return false;
        }

        self.tabs[index].start_closing();
        log::info!("Starting close animation for tab {}", self.tabs[index].id);

        // If closing the active tab, switch to another tab
        if index == self.active {
            // Find the next non-closing tab to switch to
            let next_active = self.find_next_active_tab(index);
            if let Some(new_active) = next_active {
                self.active = new_active;
            }
        }

        true
    }

    /// Find the next suitable active tab when the current one is closing
    fn find_next_active_tab(&self, closing_index: usize) -> Option<usize> {
        // Try to find a non-closing tab, preferring ones after the current
        let non_closing: Vec<usize> = self.tabs
            .iter()
            .enumerate()
            .filter(|(i, tab)| *i != closing_index && !tab.is_closing())
            .map(|(i, _)| i)
            .collect();

        if non_closing.is_empty() {
            return None;
        }

        // Prefer tab after current, otherwise before
        non_closing
            .iter()
            .find(|&&i| i > closing_index)
            .or_else(|| non_closing.last())
            .copied()
    }

    /// Close the active tab
    pub fn close_active_tab(&mut self) -> bool {
        if !self.tabs.is_empty() && !self.tabs[self.active].is_closing() {
            self.close_tab(self.active)
        } else {
            false
        }
    }

    /// Get the active tab
    pub fn active_tab(&self) -> Option<&Tab> {
        self.tabs.get(self.active)
    }

    /// Get the active tab mutably
    pub fn active_tab_mut(&mut self) -> Option<&mut Tab> {
        self.tabs.get_mut(self.active)
    }

    /// Switch to a specific tab
    pub fn switch_to(&mut self, index: usize) {
        if index < self.tabs.len() {
            self.active = index;
            // Clear activity indicator when switching to tab
            if let Some(tab) = self.tabs.get_mut(index) {
                tab.has_activity = false;
            }
        }
    }

    /// Switch to next tab
    pub fn next_tab(&mut self) {
        if !self.tabs.is_empty() {
            self.switch_to((self.active + 1) % self.tabs.len());
        }
    }

    /// Switch to previous tab
    pub fn prev_tab(&mut self) {
        if !self.tabs.is_empty() {
            self.switch_to(self.active.checked_sub(1).unwrap_or(self.tabs.len() - 1));
        }
    }

    /// Get number of tabs
    pub fn count(&self) -> usize {
        self.tabs.len()
    }

    /// Get active tab index
    pub fn active_index(&self) -> usize {
        self.active
    }

    /// Check if there are any tabs
    pub fn is_empty(&self) -> bool {
        self.tabs.is_empty()
    }

    /// Process events for all tabs, return true if any tab had output
    pub fn process_all_events(&mut self) -> bool {
        let mut any_output = false;

        for (i, tab) in self.tabs.iter_mut().enumerate() {
            let had_output = tab.process_events();
            if had_output && i != self.active {
                tab.has_activity = true;
            }
            any_output = any_output || had_output;
        }

        any_output
    }

    /// Resize all tabs
    pub fn resize(&mut self, cols: usize, rows: usize) {
        self.cols = cols;
        self.rows = rows;

        for tab in &mut self.tabs {
            if let Err(e) = tab.resize(cols, rows) {
                log::error!("Failed to resize tab {}: {}", tab.id, e);
            }
        }
    }

    /// Remove closed tabs and tabs with completed close animations
    /// Returns true if any tabs were removed
    pub fn cleanup_closed(&mut self) -> bool {
        let initial_count = self.tabs.len();
        // Remove tabs that are either:
        // - Marked as closed (shell exited)
        // - Have completed their close animation
        self.tabs.retain(|tab| !tab.closed && !tab.is_close_complete());

        if self.tabs.len() != initial_count {
            if self.active >= self.tabs.len() && !self.tabs.is_empty() {
                self.active = self.tabs.len() - 1;
            }
            true
        } else {
            false
        }
    }

    /// Check if all remaining tabs are closing (app should exit)
    pub fn all_tabs_closing(&self) -> bool {
        !self.tabs.is_empty() && self.tabs.iter().all(|tab| tab.is_closing())
    }

    /// Iterate over all tabs with their indices
    pub fn iter(&self) -> impl Iterator<Item = (usize, &Tab)> {
        self.tabs.iter().enumerate()
    }

    /// Get tab info for display (index, title, is_active, has_activity)
    pub fn tab_info(&self) -> Vec<TabInfo> {
        self.tabs
            .iter()
            .enumerate()
            .map(|(i, tab)| TabInfo {
                index: i,
                title: tab.title.clone(),
                is_active: i == self.active && !tab.is_closing(),
                has_activity: tab.has_activity,
                is_closed: tab.closed,
                slide_progress: tab.slide_progress(),
                close_progress: tab.close_progress(),
            })
            .collect()
    }

    /// Update scrollback limit for existing and future tabs
    pub fn set_scrollback_limit(&mut self, limit: usize) {
        self.scrollback_lines = limit;
        for tab in &mut self.tabs {
            tab.terminal.grid_mut().set_scrollback_limit(limit);
        }
    }
}

/// Tab information for display
#[derive(Debug, Clone)]
pub struct TabInfo {
    pub index: usize,
    pub title: String,
    pub is_active: bool,
    pub has_activity: bool,
    pub is_closed: bool,
    /// Slide-in animation progress (0.0 = just created, 1.0 = fully visible)
    pub slide_progress: f32,
    /// Close animation progress (0.0 = not closing, 1.0 = fully closed)
    pub close_progress: f32,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tab_manager_navigation() {
        let manager = TabManager::new(80, 24, ShellConfig::default(), 10_000);

        // Can't test much without actual PTY, but test navigation logic
        assert!(manager.is_empty());
        assert_eq!(manager.count(), 0);
    }
}
