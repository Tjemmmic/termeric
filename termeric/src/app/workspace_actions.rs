//! Pane, Harvest, and focused-terminal workspace actions.

use super::*;

impl App {
    /// Get pane render info from the active tab's pane manager (if in pane mode)
    pub(super) fn pane_render_info(&self) -> Vec<PaneRenderInfo> {
        let Some(tabs) = &self.tabs else {
            return Vec::new();
        };
        let Some(tab) = tabs.active_tab() else {
            return Vec::new();
        };
        let Some(ref pm) = tab.pane_manager else {
            return Vec::new();
        };
        let layout = pm.calculate_layout();
        let focused = pm.focused_id();
        layout
            .into_iter()
            .map(|(id, rect)| PaneRenderInfo {
                rect,
                is_focused: id == focused,
                id,
            })
            .collect()
    }

    /// Access the focused terminal through the TerminalContainer trait
    pub(super) fn focused_terminal_via_container(&self) -> Option<&termeric_core::Terminal> {
        let tabs = self.tabs.as_ref()?;
        let tab = tabs.active_tab()?;
        // Try pane manager first (SplitContainer path)
        if let Some(ref pm) = tab.pane_manager {
            let sc: &dyn SplitContainer = pm;
            if !sc.is_closed() {
                return sc.focused_terminal();
            }
        }
        // Fall back to single-tab container
        let tc: &dyn TerminalContainer = tab;
        tc.focused_terminal()
    }

    /// Split the current tab into panes
    pub(super) fn split_pane(&mut self, direction: SplitDirection) {
        let Some(renderer) = &self.renderer else {
            return;
        };
        let Some(tabs) = &mut self.tabs else { return };
        let Some(tab) = tabs.active_tab_mut() else {
            return;
        };

        let cols = tab.terminal.grid().cols();
        let rows = tab.terminal.grid().rows();
        let cell_width = renderer.cell_width();
        let cell_height = renderer.cell_height();
        let total_width = cols as f32 * cell_width;
        let total_height = rows as f32 * cell_height;

        if let Some(ref mut pm) = tab.pane_manager {
            // Already in pane mode — split the focused pane
            match pm.split(direction) {
                Ok(new_id) => {
                    log::info!("Split pane {:?}, created pane {:?}", direction, new_id);
                }
                Err(e) => log::error!("Failed to split pane: {}", e),
            }
        } else {
            // Create pane manager for the first split
            match PaneManager::new(
                cols,
                rows,
                self.config.shell.clone(),
                self.config.scrollback.lines,
                total_width,
                total_height,
                cell_width,
                cell_height,
            ) {
                Ok(mut pm) => {
                    // Swap pane 0's fresh terminal/pty with the tab's existing ones,
                    // so pane 0 inherits the tab's live session and content.
                    // The tab gets the fresh placeholder (dormant while in pane mode).
                    if let Some(pane0) = pm.get_pane_mut(PaneId(0)) {
                        std::mem::swap(&mut tab.terminal, &mut pane0.terminal);
                        std::mem::swap(&mut tab.pty, &mut pane0.pty);
                        std::mem::swap(&mut tab.pty_rx, &mut pane0.pty_rx);
                        std::mem::swap(&mut tab.harvest, &mut pane0.harvest);
                        std::mem::swap(&mut tab.shell_integration, &mut pane0.shell_integration);
                    }
                    match pm.split(direction) {
                        Ok(new_id) => {
                            log::info!("Entered pane mode, created pane {:?}", new_id);
                            tab.pane_manager = Some(pm);
                        }
                        Err(e) => log::error!("Failed initial pane split: {}", e),
                    }
                }
                Err(e) => log::error!("Failed to create pane manager: {}", e),
            }
        }

        // Request redraw to show new pane layout
        if let Some(window) = &self.window {
            window.request_redraw();
        }
    }

    /// Run a sprout-menu action against the terminal that produced it.
    pub(super) fn execute_sprout_action(
        &mut self,
        action: crate::harvest::sprouts::SproutAction,
        target: Option<crate::harvest::CommandEntry>,
    ) {
        use crate::harvest::sprouts::SproutAction;
        let target = target.or_else(|| {
            let source = self.active_harvest_source()?;
            self.harvest
                .as_ref()?
                .cellar
                .latest_for_source(source)
                .cloned()
        });
        let Some(target) = target else {
            self.toasts.warning("No command under cursor");
            return;
        };

        let (region, command_text, output_text) = {
            let Some(terminal) = self.terminal_for_harvest_source(target.source) else {
                self.toasts
                    .warning("The command's terminal is no longer open");
                return;
            };
            let Some(region) = terminal
                .shell_integration()
                .command_at(target.row as i32)
                .cloned()
            else {
                self.toasts
                    .warning("No shell-integration data for that command");
                return;
            };
            let command = Self::extract_command_text(terminal, &region);
            let output = Self::extract_output_text(terminal, &region);
            (region, command, output)
        };

        match action {
            SproutAction::CopyCommand => {
                let text = command_text;
                if text.trim().is_empty() {
                    self.toasts.warning("Command text is empty");
                    return;
                }
                self.copy_to_clipboard(text, "Copied command");
            }
            SproutAction::CopyOutput => {
                let text = output_text;
                if text.trim().is_empty() {
                    self.toasts.warning("Output is empty");
                    return;
                }
                self.copy_to_clipboard(text, "Copied output");
            }
            SproutAction::Rerun => {
                let trimmed = command_text.trim();
                if trimmed.is_empty() {
                    self.toasts.warning("Nothing to rerun");
                    return;
                }
                self.run_custom_command(trimmed);
                self.toasts.info("Re-ran command");
            }
            SproutAction::ScrollTo => {
                // Scroll the terminal so the prompt row is visible at the
                // top of the viewport (modulo scrollback mechanics).
                if let Some(terminal) = self.active_terminal_mut() {
                    let scrollback_len = terminal.grid().scrollback_len() as i32;
                    // `row` is an absolute prompt row from the time of
                    // capture. Convert it to a scroll offset; anything
                    // older than the current viewport requires a positive
                    // offset.
                    let rows_on_screen = terminal.rows() as i32;
                    let target_row = (target.row as i32).max(0);
                    let offset = (scrollback_len + rows_on_screen - 1 - target_row).max(0) as usize;
                    terminal.grid_mut().set_scroll_offset(offset);
                }
                if let Some(window) = &self.window {
                    window.request_redraw();
                }
            }
            SproutAction::SearchOutput => {
                let first = region
                    .output_row
                    .or(region.command_row)
                    .unwrap_or(region.prompt_row);
                let last = region.finish_row.unwrap_or(first);
                self.search.start_in_rows(first as i32, last as i32);
                self.toasts.info("Search scoped to command output");
                if let Some(window) = &self.window {
                    window.request_redraw();
                }
            }
            SproutAction::Explain => {
                let cmd = command_text;
                let out = output_text;
                if cmd.trim().is_empty() {
                    self.toasts.warning("No command to explain");
                    return;
                }
                let context = format!(
                    "Please explain this command and its output.\n\n$ {}\n\n{}",
                    cmd.trim(),
                    out.trim(),
                );
                let plugin_id = match self.preferred_plugin_target() {
                    Ok(plugin_id) => plugin_id,
                    Err(message) => {
                        self.toasts.warning(message);
                        return;
                    }
                };
                self.execute_plugin_actions(vec![crate::plugin::PluginAction::InjectContext {
                    plugin_id,
                    text: context,
                }]);
            }
        }
    }

    pub(super) fn active_harvest_source(&self) -> Option<crate::harvest::HarvestSource> {
        let tab = self.tabs.as_ref()?.active_tab()?;
        Some(crate::harvest::HarvestSource {
            tab_id: tab.id,
            pane_id: tab.pane_manager.as_ref().map(|panes| panes.focused_id().0),
        })
    }

    pub(super) fn terminal_for_harvest_source(
        &self,
        source: crate::harvest::HarvestSource,
    ) -> Option<&termeric_core::Terminal> {
        let (_, tab) = self
            .tabs
            .as_ref()?
            .iter()
            .find(|(_, tab)| tab.id == source.tab_id)?;
        match source.pane_id {
            Some(pane_id) => Some(
                &tab.pane_manager
                    .as_ref()?
                    .get_pane(PaneId(pane_id))?
                    .terminal,
            ),
            None => Some(&tab.terminal),
        }
    }

    pub(super) fn active_shell_integration_message(&self) -> String {
        let Some(tab) = self.tabs.as_ref().and_then(|tabs| tabs.active_tab()) else {
            return "Open a shell tab before using Harvest".into();
        };
        let support = if let Some(panes) = &tab.pane_manager {
            panes.focused_pane().map(|pane| &pane.shell_integration)
        } else {
            Some(&tab.shell_integration)
        };
        match support {
            Some(crate::terminal::shell_integration::ShellIntegrationSupport::Enabled(shell)) => {
                format!("No completed command yet; {shell} integration is active")
            }
            Some(crate::terminal::shell_integration::ShellIntegrationSupport::Unavailable(
                reason,
            )) => reason.clone(),
            None => "The focused terminal is unavailable".into(),
        }
    }

    pub(super) fn preferred_plugin_target(&self) -> std::result::Result<String, String> {
        let Some(tabs) = &self.tabs else {
            return Err("No plugin tab is open".into());
        };
        let active = tabs.active_tab().and_then(|tab| match &tab.tab_type {
            crate::tabs::TabType::Plugin { plugin_id } => Some(plugin_id.as_str()),
            crate::tabs::TabType::Shell => None,
        });
        let available: Vec<String> = tabs
            .iter()
            .filter_map(|(_, tab)| match &tab.tab_type {
                crate::tabs::TabType::Plugin { plugin_id } if !tab.closed => {
                    Some(plugin_id.clone())
                }
                _ => None,
            })
            .collect();
        select_plugin_target(active, &available)
    }

    /// Extract just the command line entered by the user (between command
    /// start and output start).
    pub(super) fn extract_command_text(
        terminal: &termeric_core::Terminal,
        region: &termeric_core::CommandRegion,
    ) -> String {
        let start = region.command_row.unwrap_or(region.prompt_row) as i32;
        // End at the row just before output starts, or the prompt row if
        // output never began.
        let end = region
            .output_row
            .map(|r| r.saturating_sub(1) as i32)
            .unwrap_or(start);
        let end = end.max(start);
        terminal.rows_text(start, end)
    }

    /// Extract the output block of a command (between output start and
    /// finish).
    pub(super) fn extract_output_text(
        terminal: &termeric_core::Terminal,
        region: &termeric_core::CommandRegion,
    ) -> String {
        let Some(start) = region.output_row else {
            return String::new();
        };
        let end = region
            .finish_row
            .map(|r| r.saturating_sub(1) as i32)
            .unwrap_or(start as i32);
        let start = start as i32;
        let end = end.max(start);
        terminal.rows_text(start, end)
    }

    /// Move focus to the adjacent pane in `dir`. No-op if no pane manager
    /// or no pane in that direction; in the no-neighbor case we surface a
    /// short info toast so the user knows the key was registered but the
    /// layout has nowhere to go.
    pub(super) fn navigate_focused_pane(&mut self, dir: crate::panes::PaneDir) {
        let moved = {
            let mut moved = false;
            if let Some(tabs) = &mut self.tabs {
                if let Some(tab) = tabs.active_tab_mut() {
                    if let Some(ref mut pm) = tab.pane_manager {
                        moved = pm.navigate(dir);
                    }
                }
            }
            moved
        };
        if !moved {
            use crate::panes::PaneDir;
            let label = match dir {
                PaneDir::Up => "No pane above",
                PaneDir::Down => "No pane below",
                PaneDir::Left => "No pane to the left",
                PaneDir::Right => "No pane to the right",
            };
            self.toasts.info(label);
        }
        if let Some(window) = &self.window {
            window.request_redraw();
        }
    }

    /// Close the focused pane, collapsing back to a single-terminal tab if
    /// only one pane remains.
    pub(super) fn close_focused_pane(&mut self) {
        if let Some(tabs) = &mut self.tabs {
            if let Some(tab) = tabs.active_tab_mut() {
                if let Some(ref mut pm) = tab.pane_manager {
                    pm.close_focused();
                    if pm.pane_count() <= 1 {
                        if let Some(remaining) = pm.take_remaining_pane() {
                            tab.terminal = remaining.terminal;
                            tab.pty = remaining.pty;
                            tab.pty_rx = remaining.pty_rx;
                            tab.harvest = remaining.harvest;
                            tab.shell_integration = remaining.shell_integration;
                        }
                        tab.pane_manager = None;
                        if let Err(e) = tab.resize(self.cols, self.rows) {
                            log::error!("Failed to resize tab after pane close: {}", e);
                        }
                    }
                }
            }
        }
        if let Some(window) = &self.window {
            window.request_redraw();
        }
    }
}
