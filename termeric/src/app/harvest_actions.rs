//! Harvest mode's actions: the cellar, the sprout menu and what its actions
//! run, and reading a recorded command and its output.

use super::redraw_scheduler::RedrawReason;
use super::*;

const MAX_COMMAND_REGION_OUTPUT_BYTES: usize = 64 * 1024;

fn harvest_explain_context(
    command: &str,
    command_truncated: bool,
    output: &str,
    output_truncated: bool,
) -> String {
    let command_note = if command_truncated {
        "\n\n[Command text truncated at 4 KiB; this is not the complete command.]"
    } else {
        ""
    };
    let output_note = if output_truncated {
        "\n\n[Only the last 64 KiB of output is included.]"
    } else {
        ""
    };
    format!(
        "Please explain this command and its output.\n\n$ {}{command_note}\n\n{}{output_note}",
        command.trim(),
        output.trim()
    )
}

impl App {
    /// The Root Cellar's geometry, shared by drawing, pointer hits, and
    /// accessibility; it fits the cellar's viewport to the window first.
    pub(super) fn cellar_layout(&mut self) -> Option<crate::harvest::CellarLayout> {
        let metrics = self.ui_metrics();
        let harvest = self.harvest.as_mut()?;
        Some(harvest.cellar.layout(
            metrics.width,
            metrics.height,
            metrics.cell_width,
            metrics.cell_height,
        ))
    }

    /// The sprout menu's geometry, shared by drawing, pointer hits, and
    /// accessibility. The menu stays inside the terminal area, clear of the
    /// tab rail and the agent dock.
    pub(super) fn sprout_menu_layout(&self) -> Option<crate::harvest::SproutMenuLayout> {
        let menu = self.harvest.as_ref()?.sprout_menu.as_ref()?;
        let metrics = self.ui_metrics();
        let area = self.pane_placement();
        Some(crate::harvest::SproutMenuLayout::calculate(
            menu.row,
            menu.col,
            menu.actions.len(),
            menu.width(),
            area.right,
            metrics.height,
            area.left,
            area.top,
            area.padding,
            metrics.cell_width,
            metrics.cell_height,
        ))
    }

    /// Keys for the Harvest cellar list.
    pub(super) fn handle_cellar_key(&mut self, key: &winit::keyboard::Key) {
        use winit::keyboard::{Key, NamedKey};
        let activate = match key {
            Key::Named(NamedKey::Escape) => {
                if let Some(harvest) = &mut self.harvest {
                    harvest.cellar.close();
                }
                None
            }
            Key::Named(NamedKey::ArrowUp) => {
                if let Some(harvest) = &mut self.harvest {
                    harvest.cellar.select_up();
                }
                None
            }
            Key::Named(NamedKey::ArrowDown) => {
                if let Some(harvest) = &mut self.harvest {
                    harvest.cellar.select_down();
                }
                None
            }
            Key::Named(NamedKey::PageUp) => {
                if let Some(harvest) = &mut self.harvest {
                    harvest.cellar.select_page(-1);
                }
                None
            }
            Key::Named(NamedKey::PageDown) => {
                if let Some(harvest) = &mut self.harvest {
                    harvest.cellar.select_page(1);
                }
                None
            }
            Key::Named(NamedKey::Home) => {
                if let Some(harvest) = &mut self.harvest {
                    harvest.cellar.select_first();
                }
                None
            }
            Key::Named(NamedKey::End) => {
                if let Some(harvest) = &mut self.harvest {
                    harvest.cellar.select_last();
                }
                None
            }
            Key::Named(NamedKey::Enter) => self
                .harvest
                .as_ref()
                .and_then(|harvest| harvest.cellar.selected_id()),
            _ => None,
        };
        if let Some(id) = activate {
            self.apply_cellar_action(crate::harvest::CellarAction::Activate(id));
        }
        self.request_redraw(RedrawReason::KeyboardInput);
    }

    pub(super) fn apply_cellar_action(&mut self, action: crate::harvest::CellarAction) {
        use crate::harvest::CellarAction;
        match action {
            CellarAction::None => {}
            CellarAction::Close => {
                if let Some(harvest) = &mut self.harvest {
                    harvest.cellar.close();
                }
            }
            CellarAction::Select(id) => {
                if let Some(harvest) = &mut self.harvest {
                    harvest.cellar.select(id);
                }
            }
            CellarAction::Activate(id) => {
                let entry = self
                    .harvest
                    .as_ref()
                    .and_then(|harvest| harvest.cellar.entry(id))
                    .cloned();
                let Some(entry) = entry else {
                    return;
                };
                let availability = self.sprout_availability_for_entry(&entry);
                if let Some(harvest) = &mut self.harvest {
                    harvest.cellar.close();
                    harvest.show_sprout_menu(
                        entry.row.max(0) as usize,
                        0,
                        Some(entry),
                        availability,
                    );
                }
            }
        }
    }

    /// Resolve command action text independently of its presentation caption.
    /// An incomplete live extraction remains incomplete even when a snapshot
    /// exists; never silently substitute a plausible shorter command.
    pub(super) fn harvest_command_text(
        &self,
        target: &crate::harvest::CommandEntry,
    ) -> (String, bool) {
        self.terminal_for_harvest_source(target.source)
            .filter(|terminal| {
                !terminal
                    .mode()
                    .contains(termeric_core::term::TerminalMode::ALT_SCREEN)
            })
            .and_then(|terminal| {
                terminal
                    .prompt_tracker()
                    .completed_region_by_id(target.region_id)
                    .filter(|region| region.content_available)
                    .map(Self::region_command_text)
            })
            .filter(|(text, _)| !text.trim().is_empty())
            .unwrap_or_else(|| {
                (
                    if target.command_available() {
                        target.command_text().to_owned()
                    } else {
                        String::new()
                    },
                    target.command_truncated,
                )
            })
    }

    pub(super) fn sprout_availability_for_entry(
        &self,
        target: &crate::harvest::CommandEntry,
    ) -> crate::harvest::SproutAvailability {
        let terminal = self.terminal_for_harvest_source(target.source);
        let source_terminal_available = terminal.is_some();
        let source_alternate_screen = terminal.is_some_and(|terminal| {
            terminal
                .mode()
                .contains(termeric_core::term::TerminalMode::ALT_SCREEN)
        });
        let live_region = terminal.and_then(|terminal| {
            if source_alternate_screen {
                return None;
            }
            terminal
                .prompt_tracker()
                .completed_region_by_id(target.region_id)
                .filter(|region| region.content_available)
                .map(|region| (terminal, region))
        });
        let live_output_available = live_region.is_some_and(|(terminal, region)| {
            !Self::extract_output_text(terminal, region)
                .0
                .trim()
                .is_empty()
        });
        let (command_text, command_truncated) = self.harvest_command_text(target);

        crate::harvest::SproutAvailability {
            command_available: !command_text.trim().is_empty(),
            command_truncated,
            output_available: !target.output.trim().is_empty() || live_output_available,
            source_terminal_available,
            live_content_available: live_region.is_some(),
            source_alternate_screen,
            retained_output_truncated: target.output_truncated,
            agent_features: self.agent_features_enabled(),
        }
    }

    /// Keys for the sprout menu of request `request_id`.
    pub(super) fn handle_sprout_menu_key(&mut self, request_id: u64, key: &winit::keyboard::Key) {
        use winit::keyboard::{Key, NamedKey};
        match key {
            Key::Named(NamedKey::Escape) => {
                self.dismiss_sprout_request(request_id);
            }
            Key::Named(NamedKey::ArrowUp) => {
                if let Some(menu) = self
                    .harvest
                    .as_mut()
                    .and_then(|harvest| harvest.sprout_menu.as_mut())
                {
                    menu.select_up();
                }
            }
            Key::Named(NamedKey::ArrowDown) => {
                if let Some(menu) = self
                    .harvest
                    .as_mut()
                    .and_then(|harvest| harvest.sprout_menu.as_mut())
                {
                    menu.select_down();
                }
            }
            Key::Named(NamedKey::Home) => {
                if let Some(menu) = self
                    .harvest
                    .as_mut()
                    .and_then(|harvest| harvest.sprout_menu.as_mut())
                {
                    menu.select_first();
                }
            }
            Key::Named(NamedKey::End) => {
                if let Some(menu) = self
                    .harvest
                    .as_mut()
                    .and_then(|harvest| harvest.sprout_menu.as_mut())
                {
                    menu.select_last();
                }
            }
            Key::Named(NamedKey::Enter) => {
                let action = self
                    .harvest
                    .as_ref()
                    .and_then(|harvest| harvest.sprout_menu.as_ref())
                    .and_then(|menu| menu.selected_action().copied());
                if let Some(action) = action {
                    self.execute_sprout_request(request_id, action);
                }
            }
            _ => {}
        }
        self.request_redraw(RedrawReason::KeyboardInput);
    }

    pub(super) fn execute_sprout_request(
        &mut self,
        request_id: u64,
        action: crate::harvest::sprouts::SproutAction,
    ) {
        let request = self
            .harvest
            .as_ref()
            .and_then(|harvest| harvest.sprout_menu.as_ref())
            .filter(|menu| {
                menu.request_id == request_id
                    && menu.actions.contains(&action)
                    && menu.action_enabled(action)
            })
            .map(|menu| menu.command.clone());
        let Some(target) = request else {
            return;
        };
        if let Some(harvest) = &mut self.harvest {
            harvest.hide_sprout_menu();
        }
        let Some(target) = target else {
            self.toasts.warning("No command under cursor");
            return;
        };
        self.execute_sprout_action(action, Some(target));
    }

    pub(super) fn dismiss_sprout_request(&mut self, request_id: u64) {
        if let Some(harvest) = &mut self.harvest
            && harvest
                .sprout_menu
                .as_ref()
                .is_some_and(|menu| menu.request_id == request_id)
        {
            harvest.hide_sprout_menu();
        }
    }

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

        let source_alternate_screen =
            self.terminal_for_harvest_source(target.source)
                .is_some_and(|terminal| {
                    terminal
                        .mode()
                        .contains(termeric_core::term::TerminalMode::ALT_SCREEN)
                });
        let (region, (live_output, live_output_truncated)) = self
            .terminal_for_harvest_source(target.source)
            .and_then(|terminal| {
                if source_alternate_screen {
                    return None;
                }
                let region = terminal
                    .prompt_tracker()
                    .completed_region_by_id(target.region_id)?;
                if !region.content_available {
                    return None;
                }
                let region = region.clone();
                let output = Self::extract_output_text(terminal, &region);
                Some((region, output))
            })
            .map_or_else(
                || (None, (String::new(), false)),
                |(region, output)| (Some(region), output),
            );
        let (command_text, command_truncated) = self.harvest_command_text(&target);
        let used_output_snapshot =
            live_output.trim().is_empty() && !target.output.trim().is_empty();
        let (output_text, output_truncated) = if used_output_snapshot {
            (target.output.clone(), target.output_truncated)
        } else {
            (live_output, live_output_truncated)
        };

        match action {
            SproutAction::CopyCommand => {
                let text = command_text;
                if text.trim().is_empty() {
                    self.toasts.warning("Command text is empty");
                    return;
                }
                self.copy_to_clipboard(
                    text,
                    if command_truncated {
                        "Copied partial command (truncated at 4 KiB)"
                    } else {
                        "Copied command"
                    },
                );
            }
            SproutAction::CopyOutput => {
                let text = output_text;
                if text.trim().is_empty() {
                    self.toasts.warning("Output is empty");
                    return;
                }
                let message = if output_truncated {
                    "Copied the last 64 KiB of output"
                } else {
                    "Copied output"
                };
                self.copy_to_clipboard(text, message);
            }
            SproutAction::Rerun => {
                if command_truncated {
                    self.toasts
                        .warning("Re-run was not sent: the command text is truncated");
                    return;
                }
                let trimmed = command_text.trim();
                if trimmed.is_empty() {
                    self.toasts.warning("Nothing to rerun");
                    return;
                }
                if matches!(self.broadcast, BroadcastState::Armed { .. }) {
                    self.toasts
                        .warning("Re-run was not sent while broadcast is armed");
                    return;
                }
                let Some(origin) = self.origin_for_harvest_source(target.source) else {
                    self.toasts
                        .warning("The command's terminal is no longer open");
                    return;
                };
                if self
                    .tabs
                    .as_ref()
                    .and_then(|tabs| tabs.origin_read_only(origin))
                    != Some(false)
                {
                    self.toasts
                        .warning("Re-run was not sent: the source terminal is read-only");
                    return;
                }
                if !self.focus_harvest_source(target.source) {
                    self.toasts
                        .warning("The command's terminal is no longer open");
                    return;
                }
                let Some(origin) = self.origin_for_harvest_source(target.source) else {
                    self.toasts
                        .warning("The command's terminal is no longer open");
                    return;
                };
                // Focus callbacks are not delivery authority. Capture all bytes,
                // including line clearing, in one exact-owner user operation.
                let wire = command_keystrokes(trimmed).concat();
                let result = self
                    .tabs
                    .as_mut()
                    .ok_or_else(|| {
                        std::io::Error::new(
                            std::io::ErrorKind::NotFound,
                            "source terminal no longer exists",
                        )
                    })
                    .and_then(|tabs| tabs.write_user_to_origin(origin, &wire));
                match result {
                    Ok(()) => {
                        if self.active_harvest_source() == Some(target.source) {
                            self.clear_terminal_input_tracking();
                            self.reset_cursor_blink();
                        }
                        self.toasts.info("Sent command to its source terminal");
                    }
                    Err(error) => self.toasts.error(format!("Re-run was not sent: {error}")),
                }
            }
            SproutAction::ScrollTo => {
                let Some(region) = region.as_ref() else {
                    self.toasts.warning(if source_alternate_screen {
                        "Leave the source terminal's alternate screen before scrolling to the command"
                    } else {
                        "That command is no longer present in retained terminal scrollback"
                    });
                    return;
                };
                if !self.focus_harvest_source(target.source) {
                    self.toasts
                        .warning("The command's terminal is no longer open");
                    return;
                }
                // Scroll the terminal so the prompt row is visible at the
                // top of the viewport (modulo scrollback mechanics).
                if let Some(terminal) = self.active_terminal_mut() {
                    let offset = usize::try_from(region.prompt_row.saturating_neg()).unwrap_or(0);
                    terminal.grid_mut().set_scroll_offset(offset);
                }
                self.request_redraw(RedrawReason::Action);
            }
            SproutAction::SearchOutput => {
                let Some(region) = region.as_ref() else {
                    self.toasts.warning(if source_alternate_screen {
                        "Leave the source terminal's alternate screen before searching command output"
                    } else {
                        "That command output is no longer present in retained terminal scrollback"
                    });
                    return;
                };
                if !self.focus_harvest_source(target.source) {
                    self.toasts
                        .warning("The command's terminal is no longer open");
                    return;
                }
                let first = region
                    .output_row
                    .or(region.command_row)
                    .unwrap_or(region.prompt_row);
                let last = region
                    .finish_row
                    .map(|row| row.saturating_sub(1))
                    .unwrap_or(first)
                    .max(first);
                if !self.start_command_output_search(first, last) {
                    return;
                }
                self.toasts.info("Search scoped to command output");
                self.request_redraw(RedrawReason::Action);
            }
            // A menu shown before agent features were turned off still
            // lists Explain.
            SproutAction::Explain if !self.agent_features_enabled() => {
                self.report_agent_features_off();
            }
            SproutAction::Explain => {
                let cmd = command_text;
                let out = output_text;
                if cmd.trim().is_empty() {
                    self.toasts.warning("No command to explain");
                    return;
                }
                let context =
                    harvest_explain_context(&cmd, command_truncated, &out, output_truncated);
                match self.preferred_agent_target() {
                    Ok(target) => {
                        self.send_to_agent(target, &context);
                    }
                    Err(message) => self.toasts.warning(message),
                }
            }
        }
    }

    pub(super) fn active_harvest_source(&self) -> Option<crate::harvest::HarvestSource> {
        let tab = self.tabs.as_ref()?.active_tab()?;
        Some(crate::harvest::HarvestSource {
            tab_id: tab.id,
            pane_id: tab.panes.focused_id().0,
        })
    }

    fn origin_for_harvest_source(
        &self,
        source: crate::harvest::HarvestSource,
    ) -> Option<crate::tabs::TerminalEventOrigin> {
        self.tabs
            .as_ref()?
            .origin_for_pane(source.tab_id, PaneId(source.pane_id))
    }

    pub(super) fn terminal_for_harvest_source(
        &self,
        source: crate::harvest::HarvestSource,
    ) -> Option<&termeric_core::Terminal> {
        let origin = self.origin_for_harvest_source(source)?;
        self.tabs.as_ref()?.terminal_for_origin(origin)
    }

    /// Focus the exact live terminal that produced a harvested command before
    /// running an action whose result is visible or writes input. This keeps
    /// old cellar entries from retargeting whichever terminal is currently
    /// active.
    fn focus_harvest_source(&mut self, source: crate::harvest::HarvestSource) -> bool {
        if self.origin_for_harvest_source(source).is_none() {
            return false;
        }
        let Some((index, _)) = self
            .tabs
            .as_ref()
            .and_then(|tabs| tabs.iter().find(|(_, tab)| tab.id == source.tab_id))
        else {
            return false;
        };

        self.handle_action(crate::input::Action::SwitchToTab(index));

        // Focus callbacks may alter topology or selection; they cannot grant
        // permission to act on a different terminal.
        if self.tabs.as_ref().and_then(TabManager::active_tab_id) != Some(source.tab_id)
            || self.origin_for_harvest_source(source).is_none()
        {
            return false;
        }

        let mut pane_changed = false;
        let mut live_focus = None;
        if let Some(tab) = self.tabs.as_mut().and_then(TabManager::active_tab_mut) {
            let pane_id = PaneId(source.pane_id);
            pane_changed = tab.panes.focused_id() != pane_id;
            tab.panes.focus(pane_id);
            live_focus = tab
                .live_domain_focus_route()
                .map(|(domain, pane)| (domain.to_string(), pane));
        }
        if pane_changed {
            self.refresh_focused_terminal_surfaces();
        }
        if pane_changed
            && let (Some(authority), Some((domain, pane))) =
                (self.live_domain_authority.clone(), live_focus)
        {
            crate::domain::request_focus_background(authority, domain, pane);
        }
        self.active_harvest_source() == Some(source)
            && self.origin_for_harvest_source(source).is_some()
    }

    pub(super) fn active_shell_integration_message(&self) -> String {
        let Some(tab) = self.tabs.as_ref().and_then(|tabs| tabs.active_tab()) else {
            return "Open a shell tab before using Harvest".into();
        };
        let Some(pane) = tab.focused_pane() else {
            return "The focused terminal is unavailable".into();
        };
        if !pane.shell_integration_active()
            && let Some(hint) = pane.shell_integration.setup_hint()
        {
            return hint.into();
        }
        let shell = pane.shell_integration.shell_name().unwrap_or("shell");
        format!("No completed command yet; {shell} integration is active")
    }

    /// The command a region ran, as the terminal read it when its output
    /// began; empty once the region's rows are no longer retained.
    pub(super) fn region_command_text(region: &termeric_core::CommandRegion) -> (String, bool) {
        if !region.content_available {
            return (String::new(), false);
        }
        (region.command_text.clone(), region.command_truncated)
    }

    /// Extract the output block of a command (between output start and
    /// finish), keeping its end when it is longer than the budget.
    pub(super) fn extract_output_text(
        terminal: &termeric_core::Terminal,
        region: &termeric_core::CommandRegion,
    ) -> (String, bool) {
        if !region.content_available {
            return (String::new(), false);
        }
        let Some(start) = region.output_row else {
            return (String::new(), false);
        };
        let end = region
            .finish_row
            .map(|r| r.saturating_sub(1))
            .unwrap_or(start);
        let end = end.max(start);
        terminal.bounded_rows_text_tail(start, end, MAX_COMMAND_REGION_OUTPUT_BYTES)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn harvest_explain_context_discloses_each_partial_snapshot_without_provider_execution() {
        for command_truncated in [false, true] {
            for output_truncated in [false, true] {
                let context = harvest_explain_context(
                    "printf 'two  spaces'",
                    command_truncated,
                    "output",
                    output_truncated,
                );
                assert!(context.contains("printf 'two  spaces'"));
                assert_eq!(
                    context.contains("Command text truncated"),
                    command_truncated
                );
                assert_eq!(
                    context.contains("not the complete command"),
                    command_truncated
                );
                assert_eq!(
                    context.contains("Only the last 64 KiB of output"),
                    output_truncated
                );
            }
        }
    }
}
