//! Harvest mode's actions: the cellar, the sprout menu and what its actions
//! run, and reading a recorded command and its output.

use super::redraw_scheduler::RedrawReason;
use super::*;

use crate::harvest::{CommandEntry, MAX_HARVEST_COMMAND_BYTES, MAX_HARVEST_OUTPUT_BYTES};
use termeric_core::{CommandRegion, Terminal};

/// Live reads keep as much output as the cellar's snapshot does, so a
/// command's output reads the same before and after its rows scroll away.
const MAX_COMMAND_REGION_OUTPUT_BYTES: usize = MAX_HARVEST_OUTPUT_BYTES;

fn harvest_explain_context(
    command: &str,
    command_truncated: bool,
    output: &str,
    output_truncated: bool,
) -> String {
    let command_note = if command_truncated {
        format!(
            "\n\n[Command text truncated at {} KiB; this is not the complete command.]",
            MAX_HARVEST_COMMAND_BYTES / 1024
        )
    } else {
        String::new()
    };
    let output_note = if output_truncated {
        format!(
            "\n\n[Only the last {} KiB of output is included.]",
            MAX_COMMAND_REGION_OUTPUT_BYTES / 1024
        )
    } else {
        String::new()
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
        let activate = self.harvest.as_mut().and_then(|harvest| {
            let cellar = &mut harvest.cellar;
            match key {
                Key::Named(NamedKey::Escape) => cellar.close(),
                Key::Named(NamedKey::ArrowUp) => cellar.select_up(),
                Key::Named(NamedKey::ArrowDown) => cellar.select_down(),
                Key::Named(NamedKey::PageUp) => cellar.select_page(-1),
                Key::Named(NamedKey::PageDown) => cellar.select_page(1),
                Key::Named(NamedKey::Home) => cellar.select_first(),
                Key::Named(NamedKey::End) => cellar.select_last(),
                Key::Named(NamedKey::Enter) => return cellar.selected_id(),
                _ => {}
            }
            None
        });
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
    pub(super) fn harvest_command_text(&self, target: &CommandEntry) -> (String, bool) {
        self.live_region(target)
            .map(|(_, region)| Self::region_command_text(region))
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
        target: &CommandEntry,
    ) -> crate::harvest::SproutAvailability {
        let source_terminal_available = self.terminal_for_harvest_source(target.source).is_some();
        let source_alternate_screen = self.harvest_source_on_alternate_screen(target.source);
        let live_region = self.live_region(target);
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
        if matches!(key, Key::Named(NamedKey::Escape)) {
            self.dismiss_sprout_request(request_id);
        } else if let Some(menu) = self
            .harvest
            .as_mut()
            .and_then(|harvest| harvest.sprout_menu.as_mut())
        {
            let chosen = match key {
                Key::Named(NamedKey::ArrowUp) => {
                    menu.select_up();
                    None
                }
                Key::Named(NamedKey::ArrowDown) => {
                    menu.select_down();
                    None
                }
                Key::Named(NamedKey::Home) => {
                    menu.select_first();
                    None
                }
                Key::Named(NamedKey::End) => {
                    menu.select_last();
                    None
                }
                Key::Named(NamedKey::Enter) => menu.selected_action().copied(),
                _ => None,
            };
            if let Some(action) = chosen {
                self.execute_sprout_request(request_id, action);
            }
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

        let source_alternate_screen = self.harvest_source_on_alternate_screen(target.source);
        let (region, (live_output, live_output_truncated)) = self
            .live_region(&target)
            .map(|(terminal, region)| (region.clone(), Self::extract_output_text(terminal, region)))
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
                let message = if command_truncated {
                    format!(
                        "Copied partial command (truncated at {} KiB)",
                        MAX_HARVEST_COMMAND_BYTES / 1024
                    )
                } else {
                    "Copied command".to_owned()
                };
                self.copy_to_clipboard(text, &message);
            }
            SproutAction::CopyOutput => {
                let text = output_text;
                if text.trim().is_empty() {
                    self.toasts.warning("Output is empty");
                    return;
                }
                let message = if output_truncated {
                    format!(
                        "Copied the last {} KiB of output",
                        MAX_COMMAND_REGION_OUTPUT_BYTES / 1024
                    )
                } else {
                    "Copied output".to_owned()
                };
                self.copy_to_clipboard(text, &message);
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
                // The tracker drops a cycle that finishes before its output
                // starts, so a completed region always has output rows.
                let Some((first, last)) = region_output_rows(region) else {
                    self.toasts.warning("That command has no output to search");
                    return;
                };
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
    ) -> Option<&Terminal> {
        let origin = self.origin_for_harvest_source(source)?;
        self.tabs.as_ref()?.terminal_for_origin(origin)
    }

    fn harvest_source_on_alternate_screen(&self, source: crate::harvest::HarvestSource) -> bool {
        self.terminal_for_harvest_source(source)
            .is_some_and(|terminal| {
                terminal
                    .mode()
                    .contains(termeric_core::term::TerminalMode::ALT_SCREEN)
            })
    }

    /// The terminal that ran `target` and its region, while the region's
    /// rows are still on that terminal's primary screen: `None` once the
    /// terminal is closed, while a full-screen program has the screen, or
    /// after the rows were evicted.
    fn live_region(&self, target: &CommandEntry) -> Option<(&Terminal, &CommandRegion)> {
        if self.harvest_source_on_alternate_screen(target.source) {
            return None;
        }
        let terminal = self.terminal_for_harvest_source(target.source)?;
        let region = terminal
            .prompt_tracker()
            .completed_region_by_id(target.region_id)
            .filter(|region| region.content_available)?;
        Some((terminal, region))
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
    /// A region's command, as read when its output started; it outlives the
    /// rows it was read from.
    pub(super) fn region_command_text(region: &CommandRegion) -> (String, bool) {
        (region.command_text.clone(), region.command_truncated)
    }

    /// Extract the output block of a command (between output start and
    /// finish), keeping its end when it is longer than the budget. When its
    /// top has scrolled out of history, what is left is read and marked as
    /// cut off.
    pub(super) fn extract_output_text(
        terminal: &Terminal,
        region: &CommandRegion,
    ) -> (String, bool) {
        let Some((start, end)) = region_output_rows(region) else {
            return (String::new(), false);
        };
        if region.content_available {
            return terminal.bounded_rows_text_tail(start, end, MAX_COMMAND_REGION_OUTPUT_BYTES);
        }
        if !region.tail_retained {
            return (String::new(), false);
        }
        let oldest =
            i32::try_from(terminal.grid().scrollback_len()).map_or(i32::MIN, i32::saturating_neg);
        if end < oldest {
            return (String::new(), true);
        }
        let (text, _) = terminal.bounded_rows_text_tail(
            start.max(oldest),
            end,
            MAX_COMMAND_REGION_OUTPUT_BYTES,
        );
        (text, true)
    }
}

/// The first and last rows of a region's output: from its output-start mark
/// to the row before its finish mark, and at least one row.
fn region_output_rows(region: &CommandRegion) -> Option<(i32, i32)> {
    let start = region.output_row?;
    let end = region
        .finish_row
        .map_or(start, |row| row.saturating_sub(1))
        .max(start);
    Some((start, end))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_command_longer_than_history_keeps_its_text_and_the_end_of_its_output() {
        let mut terminal = Terminal::new(20, 5);
        terminal.set_scrollback_limits(10, 1024 * 1024);
        terminal.process(b"\x1b]133;A\x07$ \x1b]133;B\x07seq 40\r\n\x1b]133;C\x07");
        for line in 1..=40 {
            terminal.process(format!("{line}\r\n").as_bytes());
        }
        terminal.process(b"\x1b]133;D;0\x07");
        let region = terminal
            .prompt_tracker()
            .completed_region(0)
            .unwrap()
            .clone();
        assert!(!region.content_available, "its prompt left history");
        assert!(region.tail_retained);

        assert_eq!(App::region_command_text(&region), ("seq 40".into(), false));
        let (output, truncated) = App::extract_output_text(&terminal, &region);
        assert!(truncated);
        assert!(output.trim_end().ends_with("40"), "{output:?}");
        assert!(!output.contains("\n1\n"), "{output:?}");

        // Erased output is gone, not misread from whatever replaced it.
        let mut erased = Terminal::new(20, 5);
        erased.process(
            b"\x1b]133;A\x07$ \x1b]133;B\x07echo hi\r\n\x1b]133;C\x07hi\r\n\x1b]133;D;0\x07",
        );
        erased.process(b"\x1b[H\x1b[2J");
        let region = erased.prompt_tracker().completed_region(0).unwrap().clone();
        assert!(!region.tail_retained);
        assert_eq!(
            App::extract_output_text(&erased, &region),
            (String::new(), false)
        );
        assert_eq!(App::region_command_text(&region), ("echo hi".into(), false));
    }

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
