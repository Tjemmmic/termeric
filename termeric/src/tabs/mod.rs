//! Tab management for multiple terminal instances
//!
//! Provides support for tabbed terminal sessions within a single window.

mod tab;
mod title;

pub use tab::{Tab, TabType};

use crate::config::ShellConfig;
use crate::session::Session;
use termeric_core::TerminalEvent;

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

    /// Create a new tab running a specific command and make it active.
    ///
    /// The program runs directly — no shell prompt or command echo.
    pub fn new_tab_with_command(
        &mut self,
        program: &str,
        args: &[String],
        working_dir: Option<&str>,
        title: Option<&str>,
        tab_type: Option<TabType>,
    ) -> anyhow::Result<usize> {
        let id = self.next_id;
        self.next_id += 1;

        let tab = Tab::new_with_command(
            id,
            self.cols,
            self.rows,
            program,
            args,
            working_dir,
            title,
            self.scrollback_lines,
            tab_type,
        )?;
        self.tabs.push(tab);
        self.active = self.tabs.len() - 1;

        log::info!("Created command tab {} running '{}'", id, program);
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

    /// Process events for all tabs, return (any_output, all_terminal_events)
    pub fn process_all_events(&mut self) -> (bool, Vec<TerminalEvent>) {
        let mut any_output = false;
        let mut all_events = Vec::new();

        for (i, tab) in self.tabs.iter_mut().enumerate() {
            let (had_output, events) = tab.process_events();
            if had_output && i != self.active {
                tab.has_activity = true;
            }
            any_output = any_output || had_output;
            all_events.extend(events);
        }

        (any_output, all_events)
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

    /// Remove closed tabs and tabs with completed close animations.
    /// Before removing, saves each non-empty tab as a session.
    /// Returns true if any tabs were removed.
    pub fn cleanup_closed(&mut self, save_scrollback: bool) -> bool {
        let initial_count = self.tabs.len();

        // Save sessions for tabs about to be removed
        for tab in self.tabs.iter_mut() {
            if tab.closed || tab.is_close_complete() {
                if tab.has_content() {
                    match tab.save_as_session(save_scrollback) {
                        Some(id) => log::info!("Auto-saved closing tab {} as session '{}'", tab.id, id),
                        None => log::error!("Failed to auto-save closing tab {}", tab.id),
                    }
                } else {
                    log::debug!("Skipping empty closing tab {} (no content)", tab.id);
                }
            }
        }

        // Now remove them
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

    /// Find the index of the first tab owned by a given plugin
    pub fn find_plugin_tab_index(&self, plugin_id: &str) -> Option<usize> {
        self.tabs.iter().position(|tab| {
            matches!(&tab.tab_type, TabType::Plugin { plugin_id: id } if id == plugin_id)
        })
    }

    /// Write data to a plugin-owned tab's PTY
    pub fn write_to_plugin_tab(&mut self, plugin_id: &str, data: &[u8]) -> bool {
        if let Some(idx) = self.find_plugin_tab_index(plugin_id) {
            let _ = self.tabs[idx].write(data);
            true
        } else {
            false
        }
    }

    /// Copy the last N lines from a plugin-owned tab's terminal
    pub fn copy_from_plugin_tab(&self, plugin_id: &str, lines: usize) -> Option<String> {
        let idx = self.find_plugin_tab_index(plugin_id)?;
        let tab = &self.tabs[idx];
        let all_lines = tab.terminal.grid().export_scrollback_text();
        let start = all_lines.len().saturating_sub(lines);
        Some(all_lines[start..].join("\n"))
    }

    /// Get the working directory of a plugin-owned tab
    pub fn plugin_tab_cwd(&self, plugin_id: &str) -> Option<String> {
        let idx = self.find_plugin_tab_index(plugin_id)?;
        self.tabs[idx].working_directory()
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
                tab_type: tab.tab_type.clone(),
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

    /// Create a tab from a saved session (single-tab) and add it
    ///
    /// The new tab inherits the session's ID so subsequent saves update
    /// the same session entry.
    pub fn new_tab_from_session(&mut self, session: &Session) -> anyhow::Result<usize> {
        let id = self.next_id;
        self.next_id += 1;

        let mut tab = Tab::new_with_cwd(
            id,
            self.cols,
            self.rows,
            &self.shell,
            self.scrollback_lines,
            session.working_directory.as_deref(),
            Some(&session.title),
        )?;

        // Link this tab to its session
        tab.session_id = Some(session.id.clone());

        // Restore last_command so the session title is preserved across open/close cycles
        tab.last_command = session.last_command.clone();

        // Replay scrollback
        if let Some(ref scrollback_file) = session.scrollback_file {
            let lines = crate::session::restore::load_scrollback_text(
                &session.id,
                scrollback_file,
            );
            if !lines.is_empty() {
                // Join lines with \r\n and add a trailing \r\n so the shell's
                // new prompt appears on a fresh line below the scrollback.
                // This is safe because export_scrollback_text() only includes
                // scrollback lines, not the visible area (which has the prompt).
                let text = lines.join("\r\n") + "\r\n";
                tab.terminal.process(text.as_bytes());
                log::info!("Replayed {} scrollback lines into restored tab", lines.len());
            }
        }

        self.tabs.push(tab);
        // Switch to the newly restored tab
        self.active = self.tabs.len() - 1;

        log::info!("Restored tab {} from session '{}'", id, session.id);
        Ok(id)
    }

    /// Save all non-empty tabs as individual sessions
    ///
    /// Each tab with content gets its own session entry.
    /// Empty tabs are skipped.
    pub fn save_all_sessions(&mut self, save_scrollback: bool) {
        for tab in self.tabs.iter_mut() {
            if !tab.has_content() {
                log::debug!("Skipping empty tab {} (no content)", tab.id);
                continue;
            }

            match tab.save_as_session(save_scrollback) {
                Some(id) => log::info!("Saved tab {} as session '{}'", tab.id, id),
                None => log::error!("Failed to save tab {}", tab.id),
            }
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
    /// What type of tab this is
    pub tab_type: TabType,
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
