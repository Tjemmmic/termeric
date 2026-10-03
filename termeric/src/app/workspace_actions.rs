//! Pane, Harvest, and focused-terminal workspace actions.

use super::redraw_scheduler::RedrawReason;
use super::*;

const MAX_COMMAND_REGION_COMMAND_BYTES: usize = crate::harvest::MAX_HARVEST_COMMAND_BYTES;
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
        "\n\n[Output snapshot truncated at 64 KiB.]"
    } else {
        ""
    };
    format!(
        "Please explain this command and its output.\n\n$ {}{command_note}\n\n{}{output_note}",
        command.trim(),
        output.trim()
    )
}

pub(super) fn validate_ordinary_tab_creation_capacity(
    tab_count: usize,
) -> std::result::Result<(), String> {
    if tab_count >= crate::control::protocol::MAX_TABS_PER_WINDOW {
        return Err(format!(
            "GUI window tab limit of {} reached",
            crate::control::protocol::MAX_TABS_PER_WINDOW
        ));
    }
    Ok(())
}

fn bounded_tab_rename_text(text: &str, max_chars: usize) -> String {
    text.chars()
        .filter(|character| !crate::safe_text::breaks_single_line(*character))
        .take(max_chars)
        .collect()
}

fn append_tab_rename_text(input: &mut String, text: &str, max_chars: usize) -> bool {
    if text.is_empty()
        || text.len() > max_chars.saturating_mul(4)
        || text.chars().any(crate::safe_text::breaks_single_line)
        || input.chars().count().saturating_add(text.chars().count()) > max_chars
    {
        return false;
    }
    input.push_str(text);
    true
}

impl App {
    pub(super) fn request_move_active_tab_by(&mut self, delta: isize) {
        let live_move = self.tabs.as_ref().and_then(|tabs| {
            let active = tabs.active_tab()?;
            let (domain, remote_tab_id) = active.live_domain_tab_route()?;
            let domain_tabs: Vec<_> = tabs
                .iter()
                .filter_map(|(_, tab)| {
                    tab.live_domain_tab_route().and_then(|(candidate, remote)| {
                        (candidate == domain).then_some((remote, tab.pinned))
                    })
                })
                .collect();
            let source = domain_tabs
                .iter()
                .position(|(remote, _)| *remote == remote_tab_id)?;
            let target = source.checked_add_signed(delta)?;
            let (_, target_pinned) = domain_tabs.get(target)?;
            (active.pinned == *target_pinned).then_some((
                domain.to_string(),
                remote_tab_id,
                u16::try_from(target).ok()?,
            ))
        });
        if let Some((domain_id, remote_tab_id, index)) = live_move {
            self.queue_live_domain_control(LiveDomainControl::MoveTab {
                domain_id,
                remote_tab_id,
                index,
            });
        } else if self
            .tabs
            .as_ref()
            .and_then(TabManager::active_tab)
            .is_some_and(|tab| tab.live_domain.is_some())
            || !self
                .tabs
                .as_mut()
                .is_some_and(|tabs| tabs.move_active_by(delta))
        {
            self.toasts
                .warning("Tab is already at the edge of its pinned group");
        }
    }

    pub(super) fn request_toggle_active_tab_pin(&mut self) {
        let live_pin = self
            .tabs
            .as_ref()
            .and_then(TabManager::active_tab)
            .and_then(|tab| {
                tab.live_domain_tab_route()
                    .map(|(domain, remote_tab_id)| (domain.to_string(), remote_tab_id, !tab.pinned))
            });
        if let Some((domain_id, remote_tab_id, pinned)) = live_pin {
            self.queue_live_domain_control(LiveDomainControl::SetTabPinned {
                domain_id,
                remote_tab_id,
                pinned,
            });
        } else if self
            .tabs
            .as_mut()
            .is_some_and(TabManager::toggle_pin_active)
        {
            self.toasts.success("Tab pin state changed");
        }
    }

    pub(super) fn begin_tab_rename(&mut self) {
        let Some(tab) = self.tabs.as_ref().and_then(TabManager::active_tab) else {
            self.toasts.warning("No tab is available to rename");
            return;
        };
        if tab.live_domain_tab_route().is_some_and(|(domain, _)| {
            self.pending_live_domain_controls
                .iter()
                .any(|pending| pending.domain_id() == domain)
        }) {
            self.toasts
                .info("Wait for the pending live-domain change before renaming");
            return;
        }
        let max_chars = if tab.live_domain.is_some() { 128 } else { 256 };
        self.tab_rename = Some(TabRenameState {
            tab_id: tab.id,
            original: tab.user_title().map(str::to_owned),
            input: bounded_tab_rename_text(tab.user_title().unwrap_or(&tab.title), max_chars),
            max_chars,
        });
        self.ime.clear_preedit();
        self.toasts
            .info("Rename tab: type a title, Enter saves, Escape cancels");
    }

    pub(super) fn handle_tab_rename_input(&mut self, event: &winit::event::KeyEvent) -> bool {
        if self.tab_rename.is_none() {
            return false;
        }
        if event.state != winit::event::ElementState::Pressed {
            return true;
        }
        self.handle_tab_rename_key(&event.logical_key)
    }

    fn update_tab_rename_preview(&mut self) {
        let Some(rename) = &self.tab_rename else {
            return;
        };
        if let Some(tab) = self.tabs.as_mut().and_then(|tabs| {
            tabs.all_tabs_mut()
                .iter_mut()
                .find(|tab| tab.id == rename.tab_id)
        }) {
            tab.set_user_title(Some(rename.input.clone()));
        }
    }

    pub(super) fn insert_tab_rename_text(&mut self, text: &str) -> bool {
        let Some(rename) = &mut self.tab_rename else {
            return false;
        };
        if !append_tab_rename_text(&mut rename.input, text, rename.max_chars) {
            return false;
        }
        self.update_tab_rename_preview();
        true
    }

    pub(super) fn handle_tab_rename_key(&mut self, key: &winit::keyboard::Key) -> bool {
        if self.tab_rename.is_none() {
            return false;
        }
        use winit::keyboard::{Key, NamedKey};
        let mut commit = false;
        let mut cancel = false;
        let plain = self.modifiers.is_empty() && !self.alt_graph_active;
        let text_allowed = self.alt_graph_active
            || !self
                .modifiers
                .intersects(ModifiersState::CONTROL | ModifiersState::ALT | ModifiersState::SUPER);
        match key {
            Key::Named(NamedKey::Enter) if plain => commit = true,
            Key::Named(NamedKey::Escape) if plain => cancel = true,
            Key::Named(NamedKey::Backspace) if plain => {
                let rename = self.tab_rename.as_mut().expect("rename checked above");
                let mut cursor = rename.input.len();
                crate::ui::text_input::edit_grapheme(
                    &mut rename.input,
                    &mut cursor,
                    crate::ui::text_input::TextInputEdit::Backspace,
                );
                self.update_tab_rename_preview();
            }
            Key::Named(NamedKey::Space) if text_allowed => {
                if !self.insert_tab_rename_text(" ") {
                    return true;
                }
            }
            Key::Character(text) if text_allowed => {
                if !self.insert_tab_rename_text(text) {
                    return true;
                }
            }
            _ => return true,
        }
        if commit {
            if let Some(rename) = self.tab_rename.take() {
                let title = (!rename.input.trim().is_empty()).then_some(rename.input);
                let live_route = self.tabs.as_ref().and_then(|tabs| {
                    tabs.iter().find_map(|(_, tab)| {
                        (tab.id == rename.tab_id)
                            .then(|| tab.live_domain_tab_route())
                            .flatten()
                            .map(|(domain, remote_tab)| (domain.to_string(), remote_tab))
                    })
                });
                if let Some(tab) = self.tabs.as_mut().and_then(|tabs| {
                    tabs.all_tabs_mut()
                        .iter_mut()
                        .find(|tab| tab.id == rename.tab_id)
                }) {
                    tab.set_user_title(title.clone());
                    if let Some((domain_id, remote_tab_id)) = live_route {
                        self.queue_live_domain_control(LiveDomainControl::RenameTab {
                            domain_id,
                            remote_tab_id,
                            title,
                        });
                    } else {
                        self.toasts.success("Tab renamed");
                    }
                }
            }
        } else if cancel && let Some(rename) = self.tab_rename.take() {
            if let Some(tab) = self.tabs.as_mut().and_then(|tabs| {
                tabs.all_tabs_mut()
                    .iter_mut()
                    .find(|tab| tab.id == rename.tab_id)
            }) {
                tab.set_user_title(rename.original);
            }
            self.toasts.info("Tab rename cancelled");
        }
        self.ime.clear_preedit();
        self.redraw_scheduler.request(RedrawReason::KeyboardInput);
        if let Some(window) = &self.window {
            window.request_redraw();
        }
        true
    }

    pub(super) fn workspace_pane_action(
        &mut self,
        action: impl FnOnce(&mut PaneManager) -> bool,
        success: &'static str,
    ) {
        let changed = self
            .tabs
            .as_mut()
            .and_then(TabManager::active_tab_mut)
            .and_then(|tab| tab.pane_manager.as_mut())
            .is_some_and(action);
        if changed {
            self.refresh_focused_terminal_surfaces();
            self.toasts.success(success);
        } else {
            self.toasts
                .warning("This action requires at least two panes");
        }
        if let Some(window) = &self.window {
            window.request_redraw();
        }
    }

    pub(super) fn request_swap_focused_pane_next(&mut self) {
        let live = self
            .tabs
            .as_ref()
            .and_then(TabManager::active_tab)
            .filter(|tab| tab.pane_manager.is_some())
            .and_then(|tab| {
                tab.live_domain_focus_route()
                    .map(|(domain, pane)| (domain.to_string(), pane))
            });
        if let Some((domain_id, remote_pane_id)) = live {
            self.queue_live_domain_control(LiveDomainControl::SwapPaneNext {
                domain_id,
                remote_pane_id,
            });
        } else {
            self.workspace_pane_action(PaneManager::swap_focused_next, "Panes swapped");
        }
    }

    pub(super) fn request_balance_active_tab(&mut self) {
        let live = self
            .tabs
            .as_ref()
            .and_then(TabManager::active_tab)
            .filter(|tab| tab.pane_manager.is_some())
            .and_then(|tab| {
                tab.live_domain_tab_route()
                    .map(|(domain, tab)| (domain.to_string(), tab))
            });
        if let Some((domain_id, remote_tab_id)) = live {
            self.queue_live_domain_control(LiveDomainControl::BalanceTab {
                domain_id,
                remote_tab_id,
            });
        } else {
            self.workspace_pane_action(PaneManager::balance, "Panes balanced");
        }
    }

    pub(super) fn request_active_tab_layout(&mut self, preset: crate::panes::PaneLayoutPreset) {
        let live = self
            .tabs
            .as_ref()
            .and_then(TabManager::active_tab)
            .filter(|tab| tab.pane_manager.is_some())
            .and_then(|tab| {
                tab.live_domain_tab_route()
                    .map(|(domain, tab)| (domain.to_string(), tab))
            });
        if let Some((domain_id, remote_tab_id)) = live {
            let preset = match preset {
                crate::panes::PaneLayoutPreset::Columns => {
                    crate::domain::model::LivePaneLayoutPreset::Columns
                }
                crate::panes::PaneLayoutPreset::Rows => {
                    crate::domain::model::LivePaneLayoutPreset::Rows
                }
                crate::panes::PaneLayoutPreset::Grid => {
                    crate::domain::model::LivePaneLayoutPreset::Grid
                }
                crate::panes::PaneLayoutPreset::Tall => {
                    crate::domain::model::LivePaneLayoutPreset::Tall
                }
                crate::panes::PaneLayoutPreset::Fat => {
                    crate::domain::model::LivePaneLayoutPreset::Fat
                }
            };
            self.queue_live_domain_control(LiveDomainControl::ApplyPaneLayout {
                domain_id,
                remote_tab_id,
                preset,
            });
        } else {
            let success = match preset {
                crate::panes::PaneLayoutPreset::Columns => "Column layout applied",
                crate::panes::PaneLayoutPreset::Rows => "Row layout applied",
                crate::panes::PaneLayoutPreset::Grid => "Grid layout applied",
                crate::panes::PaneLayoutPreset::Tall => "Tall layout applied",
                crate::panes::PaneLayoutPreset::Fat => "Fat layout applied",
            };
            self.workspace_pane_action(|manager| manager.apply_layout_preset(preset), success);
        }
    }

    pub(super) fn move_pane_to_next_tab(&mut self) {
        let live = self.tabs.as_ref().and_then(|tabs| {
            let source_index = tabs.active_index();
            let source = tabs.active_tab()?;
            let (domain_id, source_tab_id) = source.live_domain_tab_route()?;
            let (_, remote_pane_id) = source.live_domain_focus_route()?;
            let has_multiple_panes = source.pane_manager.is_some();
            let tab_count = tabs.iter().count();
            let destination = tabs
                .iter()
                .filter_map(|(index, tab)| {
                    let (candidate_domain, remote_tab_id) = tab.live_domain_tab_route()?;
                    let (_, target_pane_id) = tab.live_domain_focus_route()?;
                    (index != source_index && !tab.is_closing() && candidate_domain == domain_id)
                        .then_some((
                            (index + tab_count - source_index) % tab_count,
                            remote_tab_id,
                            target_pane_id,
                        ))
                })
                .min_by_key(|(distance, _, _)| *distance)
                .map(|(_, remote_tab_id, target_pane_id)| (remote_tab_id, target_pane_id));
            Some((
                domain_id.to_string(),
                remote_pane_id,
                source_tab_id,
                destination,
                has_multiple_panes,
            ))
        });
        if let Some((domain_id, remote_pane_id, source_tab_id, destination, has_multiple_panes)) =
            live
        {
            if !has_multiple_panes {
                self.toasts
                    .warning("Pane was not moved: The last pane in a tab cannot be moved");
            } else if let Some((destination_tab_id, target_pane_id)) = destination {
                self.queue_live_domain_control(LiveDomainControl::MovePane {
                    domain_id,
                    remote_pane_id,
                    source_tab_id,
                    destination_tab_id,
                    target_pane_id,
                    direction: crate::domain::model::SplitDirection::Vertical,
                });
            } else {
                self.toasts
                    .warning("Pane was not moved: A second live-domain tab is required");
            }
            if let Some(window) = &self.window {
                window.request_redraw();
            }
            return;
        }
        let result = self
            .tabs
            .as_mut()
            .ok_or_else(|| anyhow::anyhow!("No workspace is active"))
            .and_then(|tabs| tabs.move_focused_pane_to_next_tab(self.config.panes.cwd));
        match result {
            Ok(_) => {
                self.broadcast = BroadcastState::Inactive;
                self.leave_copy_mode(false);
                self.terminal_mouse_capture = None;
                self.refresh_focused_terminal_surfaces();
                self.toasts.success("Pane moved to the next tab");
            }
            Err(error) => self
                .toasts
                .warning(format!("Pane was not moved: {error:#}")),
        }
    }

    pub(super) fn request_move_pane_to_next_window(&mut self) {
        if self.window.is_none() {
            self.toasts
                .warning("Pane was not moved: a native window is required");
            return;
        }
        let result = self
            .tabs
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("No workspace is active"))
            .and_then(TabManager::validate_window_pane_transfer_source);
        match result {
            Ok(()) => self.request_move_pane_to_next_window = true,
            Err(error) => self
                .toasts
                .warning(format!("Pane was not moved: {error:#}")),
        }
    }

    pub(super) fn request_move_tab_to_next_window(&mut self) {
        if self.window.is_none() {
            self.toasts
                .warning("Tab was not moved: a native window is required");
            return;
        }
        let result = self
            .tabs
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("No workspace is active"))
            .and_then(TabManager::validate_window_tab_transfer_source);
        match result {
            Ok(()) => self.request_move_tab_to_next_window = true,
            Err(error) => self.toasts.warning(format!("Tab was not moved: {error:#}")),
        }
    }

    pub(super) fn cycle_pane_cwd_mode(&mut self) {
        self.config.panes.cwd = self.config.panes.cwd.next();
        if let Some(tabs) = &mut self.tabs {
            for tab in tabs.all_tabs_mut() {
                if let Some(manager) = &mut tab.pane_manager {
                    manager.set_cwd_mode(self.config.panes.cwd);
                }
            }
        }
        let label = self.config.panes.cwd.label();
        match self.save_config() {
            Ok(_) => self.toasts.success(format!("New panes use {label}")),
            Err(error) => self.toasts.error(format!(
                "Directory mode changed but was not saved: {error:#}"
            )),
        }
    }

    pub(super) fn toggle_broadcast(&mut self) {
        const ARM_WINDOW: Duration = Duration::from_secs(10);

        if matches!(self.broadcast, BroadcastState::Active { .. }) {
            self.broadcast = BroadcastState::Inactive;
            self.toasts.success("Broadcast input cancelled");
            return;
        }
        let Some(tab) = self.tabs.as_ref().and_then(TabManager::active_tab) else {
            self.toasts.warning("No tab is active");
            return;
        };
        let Some(manager) = &tab.pane_manager else {
            self.toasts.warning("Broadcast requires at least two panes");
            return;
        };
        let tab_id = tab.id;
        let targets: Vec<_> = manager
            .pane_ids()
            .into_iter()
            .filter(|id| manager.get_pane(*id).is_some_and(|pane| !pane.read_only))
            .collect();
        if targets.len() < 2 {
            self.toasts
                .warning("Broadcast requires at least two writable panes");
            return;
        }
        self.broadcast = match self.broadcast.clone() {
            BroadcastState::Inactive => {
                self.ime.clear_preedit();
                self.input_line_buffer.clear();
                self.git_completions_visible = false;
                self.git_completions.clear();
                self.git_completion_index = None;
                self.toasts.warning(
                    "Broadcast armed for 10 seconds: click panes or focus one and press Space to change targets; press Enter or invoke Toggle Broadcast Input again to confirm",
                );
                BroadcastState::Armed {
                    tab_id,
                    expires: Instant::now() + ARM_WINDOW,
                    targets,
                }
            }
            BroadcastState::Armed {
                tab_id: armed_tab,
                expires,
                targets: selected,
            } if armed_tab == tab_id && Instant::now() <= expires => {
                if selected.len() < 2 {
                    self.toasts.warning(format!(
                        "Select at least two broadcast targets ({} selected)",
                        selected.len()
                    ));
                    BroadcastState::Armed {
                        tab_id,
                        expires: Instant::now() + ARM_WINDOW,
                        targets: selected,
                    }
                } else {
                    self.toasts.warning(format!(
                        "BROADCAST ACTIVE to {} panes — press Escape to cancel immediately",
                        selected.len()
                    ));
                    BroadcastState::Active {
                        tab_id,
                        targets: selected,
                    }
                }
            }
            BroadcastState::Active { .. } => unreachable!("active state returned above"),
            BroadcastState::Armed { .. } => {
                self.toasts
                    .warning("Broadcast confirmation expired; armed again for 10 seconds");
                BroadcastState::Armed {
                    tab_id,
                    expires: Instant::now() + ARM_WINDOW,
                    targets,
                }
            }
        };
    }

    pub(super) fn toggle_armed_broadcast_target(&mut self, target: PaneId) -> bool {
        const ARM_WINDOW: Duration = Duration::from_secs(10);

        let BroadcastState::Armed {
            tab_id,
            expires,
            mut targets,
        } = self.broadcast.clone()
        else {
            return false;
        };
        if Instant::now() > expires {
            self.broadcast = BroadcastState::Inactive;
            self.toasts
                .warning("Broadcast arming expired; input was not sent");
            return true;
        }
        let target_state = self
            .tabs
            .as_ref()
            .and_then(TabManager::active_tab)
            .filter(|tab| tab.id == tab_id)
            .and_then(|tab| tab.pane_manager.as_ref())
            .and_then(|manager| manager.get_pane(target))
            .map(|pane| pane.read_only);
        let Some(target_read_only) = target_state else {
            return false;
        };
        if target_read_only {
            self.toasts
                .warning("Read-only panes cannot receive broadcast input");
            return true;
        }
        if let Some(index) = targets.iter().position(|candidate| *candidate == target) {
            targets.remove(index);
        } else {
            targets.push(target);
        }
        targets.sort_by_key(|id| id.0);
        self.toasts.info(format!(
            "Broadcast armed: {} target{} selected",
            targets.len(),
            if targets.len() == 1 { "" } else { "s" }
        ));
        self.broadcast = BroadcastState::Armed {
            tab_id,
            expires: Instant::now() + ARM_WINDOW,
            targets,
        };
        true
    }

    pub(super) fn handle_broadcast_arming_key(&mut self, event: &winit::event::KeyEvent) -> bool {
        let BroadcastState::Armed { expires, .. } = self.broadcast else {
            return false;
        };
        if event.state != winit::event::ElementState::Pressed {
            return true;
        }
        if Instant::now() > expires {
            self.broadcast = BroadcastState::Inactive;
            self.toasts
                .warning("Broadcast arming expired; input was not sent");
            return true;
        }

        use crate::input::keybindings::BindableAction;
        use winit::keyboard::{Key, NamedKey};
        let plain = !self.modifiers.control_key()
            && !self.modifiers.alt_key()
            && !self.modifiers.super_key();
        if plain && matches!(event.logical_key, Key::Named(NamedKey::Enter)) {
            self.toggle_broadcast();
            return true;
        }
        if plain && matches!(event.logical_key, Key::Named(NamedKey::Space)) {
            if let Some(target) = self
                .tabs
                .as_ref()
                .and_then(TabManager::active_tab)
                .and_then(|tab| tab.pane_manager.as_ref())
                .map(PaneManager::focused_id)
            {
                self.toggle_armed_broadcast_target(target);
            }
            return true;
        }
        if let Some(action) = self
            .shortcut_registry
            .check(&event.logical_key, self.modifiers)
            .filter(|action| {
                matches!(
                    action,
                    BindableAction::ToggleBroadcast
                        | BindableAction::NavigatePaneUp
                        | BindableAction::NavigatePaneDown
                        | BindableAction::NavigatePaneLeft
                        | BindableAction::NavigatePaneRight
                )
            })
        {
            self.handle_action(action.to_action());
        }
        true
    }

    pub(super) fn cancel_broadcast_on_escape(&mut self, event: &winit::event::KeyEvent) -> bool {
        if matches!(self.broadcast, BroadcastState::Inactive)
            || event.state != winit::event::ElementState::Pressed
            || event.logical_key != winit::keyboard::Key::Named(winit::keyboard::NamedKey::Escape)
        {
            return false;
        }
        self.broadcast = BroadcastState::Inactive;
        self.toasts.success("Broadcast input cancelled");
        true
    }

    pub(super) fn broadcast_targets(&self) -> Option<(usize, Vec<PaneId>)> {
        let BroadcastState::Active { tab_id, targets } = &self.broadcast else {
            return None;
        };
        let tab = self.tabs.as_ref()?.active_tab()?;
        (tab.id == *tab_id).then(|| (*tab_id, targets.clone()))
    }

    /// Broadcast input is the person's typing in every pane it reached.
    fn note_broadcast_input(
        &mut self,
        tab_id: usize,
        panes: &[PaneId],
        input: super::agents::PersonInput,
    ) {
        for pane_id in panes {
            self.note_person_input(
                crate::tabs::TerminalIdentity {
                    tab_id,
                    pane_id: *pane_id,
                },
                input,
            );
        }
    }

    fn finish_broadcast_writes(
        &mut self,
        tab_id: usize,
        mut remaining: Vec<PaneId>,
        failed: usize,
    ) {
        remaining.sort_by_key(|id| id.0);
        remaining.dedup();
        if remaining.len() < 2 {
            self.broadcast = BroadcastState::Inactive;
            self.toasts
                .warning("Broadcast cancelled because fewer than two targets remain");
        } else {
            self.broadcast = BroadcastState::Active {
                tab_id,
                targets: remaining,
            };
            if failed > 0 {
                self.toasts.warning(format!(
                    "Broadcast dropped {failed} unavailable target{}",
                    if failed == 1 { "" } else { "s" }
                ));
            }
        }
    }

    pub(super) fn broadcast_render_targets(
        &self,
    ) -> Option<(usize, Vec<PaneId>, crate::renderer::BroadcastTargetState)> {
        let (tab_id, targets, state) = match &self.broadcast {
            BroadcastState::Armed {
                tab_id,
                expires,
                targets,
            } if Instant::now() <= *expires => (
                *tab_id,
                targets.clone(),
                crate::renderer::BroadcastTargetState::Armed,
            ),
            BroadcastState::Active { tab_id, targets } => (
                *tab_id,
                targets.clone(),
                crate::renderer::BroadcastTargetState::Active,
            ),
            _ => return None,
        };
        let tab = self.tabs.as_ref()?.active_tab()?;
        (tab.id == tab_id).then_some((tab_id, targets, state))
    }

    pub(super) fn reconcile_broadcast_state(&mut self) {
        let active_tab = self.tabs.as_ref().and_then(TabManager::active_tab);
        let active_id = active_tab.map(|tab| tab.id);
        match self.broadcast.clone() {
            BroadcastState::Inactive => {}
            BroadcastState::Armed { tab_id, .. } if active_id != Some(tab_id) => {
                self.broadcast = BroadcastState::Inactive;
                self.toasts
                    .info("Broadcast arming cancelled after focus changed");
            }
            BroadcastState::Armed { expires, .. } if Instant::now() > expires => {
                self.broadcast = BroadcastState::Inactive;
                self.toasts.info("Broadcast arming expired");
            }
            BroadcastState::Armed {
                tab_id,
                expires,
                mut targets,
            } => {
                let manager = active_tab
                    .filter(|tab| tab.id == tab_id)
                    .and_then(|tab| tab.pane_manager.as_ref());
                if manager.is_none_or(|manager| manager.pane_ids().len() < 2) {
                    self.broadcast = BroadcastState::Inactive;
                    self.toasts
                        .warning("Broadcast arming cancelled because fewer than two panes remain");
                } else {
                    targets.retain(|id| {
                        manager.is_some_and(|manager| {
                            manager.get_pane(*id).is_some_and(|pane| !pane.read_only)
                        })
                    });
                    self.broadcast = BroadcastState::Armed {
                        tab_id,
                        expires,
                        targets,
                    };
                }
            }
            BroadcastState::Active {
                tab_id,
                mut targets,
            } => {
                let manager = active_tab
                    .filter(|tab| tab.id == tab_id)
                    .and_then(|tab| tab.pane_manager.as_ref());
                targets.retain(|id| {
                    manager.is_some_and(|manager| {
                        manager.get_pane(*id).is_some_and(|pane| !pane.read_only)
                    })
                });
                if targets.len() < 2 {
                    self.broadcast = BroadcastState::Inactive;
                    self.toasts
                        .warning("Broadcast cancelled because its target set changed");
                } else {
                    self.broadcast = BroadcastState::Active { tab_id, targets };
                }
            }
        }
    }

    pub(super) fn write_broadcast_bytes(&mut self, data: &[u8]) -> bool {
        let Some((tab_id, targets)) = self.broadcast_targets() else {
            if matches!(self.broadcast, BroadcastState::Active { .. }) {
                self.broadcast = BroadcastState::Inactive;
                self.toasts
                    .warning("Broadcast cancelled because the active tab changed");
            }
            return false;
        };
        let Some(tab) = self.tabs.as_mut().and_then(TabManager::active_tab_mut) else {
            return false;
        };
        debug_assert_eq!(tab.id, tab_id);
        let Some(manager) = &mut tab.pane_manager else {
            self.broadcast = BroadcastState::Inactive;
            return false;
        };
        let mut remaining = targets;
        let mut failed = 0;
        remaining.retain(|id| {
            let written = manager.write_to(*id, data).is_ok();
            failed += usize::from(!written);
            written
        });
        self.note_broadcast_input(tab_id, &remaining, super::agents::PersonInput::of(data));
        self.finish_broadcast_writes(tab_id, remaining, failed);
        true
    }

    pub(super) fn write_broadcast_key(&mut self, event: &winit::event::KeyEvent) -> bool {
        let Some((tab_id, targets)) = self.broadcast_targets() else {
            if matches!(self.broadcast, BroadcastState::Active { .. }) {
                self.broadcast = BroadcastState::Inactive;
                self.toasts
                    .warning("Broadcast cancelled because the active tab changed");
            }
            return false;
        };
        let Some(manager) = self
            .tabs
            .as_ref()
            .and_then(TabManager::active_tab)
            .filter(|tab| tab.id == tab_id)
            .and_then(|tab| tab.pane_manager.as_ref())
        else {
            self.broadcast = BroadcastState::Inactive;
            return true;
        };
        let existing = targets
            .iter()
            .filter(|id| manager.get_pane(**id).is_some())
            .count();
        if existing < 2 {
            self.broadcast = BroadcastState::Inactive;
            self.toasts
                .warning("Broadcast cancelled because fewer than two targets remain");
            return true;
        }
        let encoded: Vec<_> = targets
            .iter()
            .filter_map(|id| {
                let terminal = &manager.get_pane(*id)?.terminal;
                crate::input::key_to_bytes_for_terminal(
                    event,
                    self.modifiers,
                    self.alt_graph_active,
                    terminal,
                )
                .map(|bytes| (*id, bytes))
            })
            .collect();
        let Some(manager) = self
            .tabs
            .as_mut()
            .and_then(TabManager::active_tab_mut)
            .and_then(|tab| tab.pane_manager.as_mut())
        else {
            self.broadcast = BroadcastState::Inactive;
            return true;
        };
        let mut remaining = targets;
        let mut failed = 0;
        let mut input = super::agents::PersonInput::Navigation;
        for (id, bytes) in encoded {
            if manager.write_to(id, &bytes).is_err() {
                remaining.retain(|target| *target != id);
                failed += 1;
            } else {
                // One key, encoded per pane; any pane's reading will do.
                input = super::agents::PersonInput::of(&bytes);
            }
        }
        self.note_broadcast_input(tab_id, &remaining, input);
        self.finish_broadcast_writes(tab_id, remaining, failed);
        true
    }

    pub(super) fn write_broadcast_paste(&mut self, text: &str) -> bool {
        let Some((tab_id, targets)) = self.broadcast_targets() else {
            if matches!(self.broadcast, BroadcastState::Active { .. }) {
                self.broadcast = BroadcastState::Inactive;
                self.toasts
                    .warning("Broadcast cancelled because the active tab changed");
            }
            return false;
        };
        let framed: Vec<_> = self
            .tabs
            .as_ref()
            .and_then(TabManager::active_tab)
            .filter(|tab| tab.id == tab_id)
            .and_then(|tab| tab.pane_manager.as_ref())
            .map(|manager| {
                targets
                    .iter()
                    .filter_map(|id| {
                        let mode = manager.get_pane(*id)?.terminal.mode();
                        Some((*id, crate::input::encode_paste(mode, text)))
                    })
                    .collect()
            })
            .unwrap_or_default();
        if framed.len() < 2 {
            self.broadcast = BroadcastState::Inactive;
            self.toasts
                .warning("Broadcast cancelled because fewer than two targets remain");
            return true;
        }
        let Some(manager) = self
            .tabs
            .as_mut()
            .and_then(TabManager::active_tab_mut)
            .and_then(|tab| tab.pane_manager.as_mut())
        else {
            self.broadcast = BroadcastState::Inactive;
            return true;
        };
        let mut remaining = targets;
        let mut failed = 0;
        for (id, bytes) in framed {
            if manager.write_to(id, &bytes).is_err() {
                remaining.retain(|target| *target != id);
                failed += 1;
            }
        }
        self.note_broadcast_input(tab_id, &remaining, super::agents::PersonInput::Answer);
        self.finish_broadcast_writes(tab_id, remaining, failed);
        self.input_line_buffer.clear();
        true
    }

    pub(super) fn toggle_focused_pane_read_only(&mut self) {
        let Some(tab) = self.tabs.as_mut().and_then(TabManager::active_tab_mut) else {
            self.toasts.warning("No pane is active");
            return;
        };
        let read_only = tab.toggle_focused_pane_read_only();
        if read_only {
            self.ime.clear_preedit();
            self.clear_terminal_input_tracking();
        }
        self.reconcile_broadcast_state();
        self.toasts.info(if read_only {
            "Pane is read-only; terminal replies and output remain active"
        } else {
            "Pane input unlocked"
        });
        if let Some(window) = &self.window {
            window.request_redraw();
        }
    }
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

    /// Split the current tab into panes
    pub(super) fn split_pane(&mut self, direction: SplitDirection) {
        let live_route = self
            .tabs
            .as_ref()
            .and_then(TabManager::active_tab)
            .and_then(crate::tabs::Tab::live_domain_focus_route)
            .map(|(domain, pane)| (domain.to_string(), pane));
        if let Some((domain_id, remote_pane_id)) = live_route {
            let direction = match direction {
                SplitDirection::Horizontal => crate::domain::model::SplitDirection::Horizontal,
                SplitDirection::Vertical => crate::domain::model::SplitDirection::Vertical,
            };
            let spawn = self.live_domain_spawn_spec(false);
            self.queue_live_domain_control(LiveDomainControl::SplitPane {
                domain_id,
                remote_pane_id,
                direction,
                spawn,
            });
            return;
        }
        let Some(renderer) = &self.renderer else {
            return;
        };
        let Some(tabs) = &mut self.tabs else { return };
        let Some(tab) = tabs.active_tab_mut() else {
            return;
        };
        if !tab.allows_pane_splits() {
            self.toasts
                .warning("Serial terminal tabs cannot be split into panes");
            if let Some(window) = &self.window {
                window.request_redraw();
            }
            return;
        }

        let Some(focused_terminal) = tab.focused_terminal() else {
            return;
        };
        let cols = focused_terminal.grid().cols();
        let rows = focused_terminal.grid().rows();
        let cell_width = renderer.cell_width();
        let cell_height = renderer.cell_height();
        let total_width = cols as f32 * cell_width;
        let total_height = rows as f32 * cell_height;

        if let Some(ref mut pm) = tab.pane_manager {
            // Already in pane mode — split the focused pane
            pm.set_cwd_mode(self.config.panes.cwd);
            match pm.split(direction) {
                Ok(new_id) => {
                    log::info!("Split pane {:?}, created pane {:?}", direction, new_id);
                }
                Err(e) => log::error!("Failed to split pane: {}", e),
            }
        } else {
            // Adopt the tab's live terminal directly. The first split creates
            // only the one new pane requested by the user.
            let split_shell = tab.split_shell_config();
            let Some(owner) = tab.take_terminal_owner() else {
                log::error!("Cannot enter pane mode without a single terminal owner");
                return;
            };
            let mut pm = PaneManager::from_existing(
                owner,
                split_shell,
                self.config.scrollback.lines,
                self.config.scrollback.max_memory_bytes(),
                total_width,
                total_height,
                cell_width,
                cell_height,
            );
            pm.set_cwd_mode(self.config.panes.cwd);
            match pm.split(direction) {
                Ok(new_id) => {
                    log::info!("Entered pane mode, created pane {:?}", new_id);
                    tab.pane_manager = Some(pm);
                }
                Err(error) => {
                    log::error!("Failed initial pane split: {}", error);
                    if let Some(original) = pm.take_remaining_pane() {
                        tab.restore_terminal_owner(original);
                    }
                }
            }
        }

        self.refresh_focused_terminal_surfaces();

        // Request redraw to show new pane layout
        if let Some(window) = &self.window {
            window.request_redraw();
        }
    }

    /// Split one exact local pane for the private running-GUI control plane.
    /// Live-domain topology changes are asynchronous and deliberately rejected
    /// until their resulting stable local IDs can be acknowledged safely.
    pub(super) fn create_control_pane(
        &mut self,
        tab_id: u64,
        pane_id: u32,
        direction: SplitDirection,
    ) -> std::result::Result<u32, String> {
        let tab_id = usize::try_from(tab_id)
            .map_err(|_| "Target tab ID does not fit this platform".to_string())?;
        let tabs = self
            .tabs
            .as_mut()
            .ok_or_else(|| "Target window has no initialized tab manager".to_string())?;
        let tab = tabs
            .tab_mut_by_id(tab_id)
            .ok_or_else(|| format!("GUI tab {tab_id} does not exist in the target window"))?;
        if tab.closed || tab.closing_at.is_some() {
            return Err(format!("GUI tab {tab_id} is closing"));
        }
        if tab.live_domain_tab_route().is_some() {
            return Err("Control-created panes are not yet supported for live-domain tabs".into());
        }
        if !tab.allows_pane_splits() {
            return Err("Serial terminal tabs cannot be split into panes".into());
        }

        let direction_name = match direction {
            SplitDirection::Horizontal => "horizontal",
            SplitDirection::Vertical => "vertical",
        };
        let target = PaneId(pane_id);
        let new_id = if let Some(manager) = &mut tab.pane_manager {
            let pane = manager
                .get_pane(target)
                .ok_or_else(|| format!("GUI pane {pane_id} does not exist in tab {tab_id}"))?;
            if pane.closed {
                return Err(format!("GUI pane {pane_id} in tab {tab_id} is closed"));
            }
            if manager.pane_count() >= crate::control::protocol::MAX_PANES_PER_TAB {
                return Err(format!(
                    "GUI tab pane limit of {} reached",
                    crate::control::protocol::MAX_PANES_PER_TAB
                ));
            }
            manager.set_cwd_mode(self.config.panes.cwd);
            manager.split_target(target, direction).map_err(|error| {
                format!("Could not split pane {pane_id} {direction_name}: {error:#}")
            })?
        } else {
            let (cell_width, cell_height) = self
                .renderer
                .as_ref()
                .map(|renderer| (renderer.cell_width(), renderer.cell_height()))
                .ok_or_else(|| "Target window has no initialized renderer".to_string())?;
            let sole_pane = tab.single_pane_id();
            if target != sole_pane {
                return Err(format!(
                    "GUI pane {pane_id} does not exist in unsplit tab {tab_id}; its sole pane ID is {}",
                    sole_pane.0
                ));
            }
            let terminal = tab
                .focused_terminal()
                .ok_or_else(|| format!("GUI tab {tab_id} has no live terminal"))?;
            let total_width = terminal.grid().cols() as f32 * cell_width;
            let total_height = terminal.grid().rows() as f32 * cell_height;
            let split_shell = tab.split_shell_config();
            let owner = tab.take_terminal_owner().ok_or_else(|| {
                format!("GUI tab {tab_id} cannot transfer its terminal into pane mode")
            })?;
            let mut manager = PaneManager::from_existing(
                owner,
                split_shell,
                self.config.scrollback.lines,
                self.config.scrollback.max_memory_bytes(),
                total_width,
                total_height,
                cell_width,
                cell_height,
            );
            manager.set_cwd_mode(self.config.panes.cwd);
            match manager.split_target(sole_pane, direction) {
                Ok(new_id) => {
                    tab.pane_manager = Some(manager);
                    new_id
                }
                Err(error) => {
                    if let Some(original) = manager.take_remaining_pane() {
                        tab.restore_terminal_owner(original);
                    }
                    return Err(format!(
                        "Could not split pane {pane_id} {direction_name}: {error:#}"
                    ));
                }
            }
        };

        log::info!(
            "Control split pane {pane_id} {direction_name}, created pane {}",
            new_id.0
        );
        self.refresh_focused_terminal_surfaces();
        if let Some(window) = &self.window {
            window.request_redraw();
        }
        Ok(new_id.0)
    }

    /// Run a sprout-menu action against the terminal that produced it.
    pub(super) fn harvest_cellar_layout(&self) -> Option<crate::harvest::CellarLayout> {
        let harvest = self.harvest.as_ref()?;
        if !harvest.cellar.is_expanded() {
            return None;
        }
        let renderer = self.renderer.as_ref()?;
        let (window_width, window_height) = renderer.window_size();
        let preview = crate::harvest::CellarLayout::calculate(
            window_width as f32,
            window_height as f32,
            renderer.cell_width(),
            renderer.cell_height(),
            harvest.cellar.animation.progress(),
            std::iter::empty(),
        );
        let visible_ids = harvest
            .cellar
            .entries()
            .skip(harvest.cellar.scroll)
            .take(preview.max_visible)
            .map(|entry| entry.id)
            .collect::<Vec<_>>();
        Some(crate::harvest::CellarLayout::calculate(
            window_width as f32,
            window_height as f32,
            renderer.cell_width(),
            renderer.cell_height(),
            harvest.cellar.animation.progress(),
            visible_ids,
        ))
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
                    .shell_integration()
                    .prompt_tracker
                    .completed_region_by_id(target.region_id)
                    .filter(|region| region.content_available)
                    .map(|region| Self::extract_command_text(terminal, region))
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
                .shell_integration()
                .prompt_tracker
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
        }
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
        let (region, live_output) = self
            .terminal_for_harvest_source(target.source)
            .and_then(|terminal| {
                if source_alternate_screen {
                    return None;
                }
                let region = terminal
                    .shell_integration()
                    .prompt_tracker
                    .completed_region_by_id(target.region_id)?;
                if !region.content_available {
                    return None;
                }
                let region = region.clone();
                let output = Self::extract_output_text(terminal, &region).0;
                Some((region, output))
            })
            .map_or_else(
                || (None, String::new()),
                |(region, output)| (Some(region), output),
            );
        let (command_text, command_truncated) = self.harvest_command_text(&target);
        let used_output_snapshot =
            live_output.trim().is_empty() && !target.output.trim().is_empty();
        let output_text = if used_output_snapshot {
            target.output.clone()
        } else {
            live_output
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
                let message = if used_output_snapshot && target.output_truncated {
                    "Copied retained output (truncated at 64 KiB)"
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
                if let Some(window) = &self.window {
                    window.request_redraw();
                }
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
                let context = harvest_explain_context(
                    &cmd,
                    command_truncated,
                    &out,
                    used_output_snapshot && target.output_truncated,
                );
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
            pane_id: tab
                .pane_manager
                .as_ref()
                .map_or(tab.single_pane_id().0, |panes| panes.focused_id().0),
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
        if let Some(tab) = self.tabs.as_mut().and_then(TabManager::active_tab_mut)
            && let Some(panes) = &mut tab.pane_manager
        {
            let pane_id = PaneId(source.pane_id);
            pane_changed = panes.focused_id() != pane_id;
            panes.focus(pane_id);
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

    /// Extract just the command line entered by the user (between command
    /// start and output start).
    pub(super) fn extract_command_text(
        terminal: &termeric_core::Terminal,
        region: &termeric_core::CommandRegion,
    ) -> (String, bool) {
        if !region.content_available {
            return (String::new(), false);
        }
        let start = region.command_row.unwrap_or(region.prompt_row);
        // End at the row just before output starts, or the prompt row if
        // output never began.
        let end = region
            .output_row
            .map(|r| r.saturating_sub(1))
            .unwrap_or(start);
        let end = end.max(start);
        terminal.bounded_rows_text(start, end, MAX_COMMAND_REGION_COMMAND_BYTES)
    }

    /// Extract the output block of a command (between output start and
    /// finish).
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
        terminal.bounded_rows_text(start, end, MAX_COMMAND_REGION_OUTPUT_BYTES)
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
        } else {
            self.persist_live_domain_focus();
            self.refresh_focused_terminal_surfaces();
        }
        if let Some(window) = &self.window {
            window.request_redraw();
        }
    }

    pub(super) fn persist_live_domain_focus(&self) {
        let route = self
            .tabs
            .as_ref()
            .and_then(TabManager::active_tab)
            .and_then(crate::tabs::Tab::live_domain_focus_route)
            .map(|(domain, pane)| (domain.to_string(), pane));
        if let (Some(authority), Some((domain, pane))) = (self.live_domain_authority.clone(), route)
        {
            crate::domain::request_focus_background(authority, domain, pane);
        }
    }

    pub(super) fn queue_live_domain_control(&mut self, control: LiveDomainControl) {
        if let Err(error) = self.start_live_domain_control(control) {
            self.toasts.error(error);
        }
    }

    pub(super) fn start_live_domain_control(
        &mut self,
        control: LiveDomainControl,
    ) -> std::result::Result<(), String> {
        if self
            .pending_live_domain_controls
            .iter()
            .any(|pending| pending.domain_id() == control.domain_id())
        {
            self.restore_live_domain_preview(&control);
            return Err("Another change for this live domain is already pending".into());
        }
        let inserted = self.pending_live_domain_controls.insert(control.clone());
        debug_assert!(inserted);
        let Some(window_id) = self.window.as_ref().map(|window| window.id()) else {
            self.pending_live_domain_controls.remove(&control);
            self.restore_live_domain_preview(&control);
            return Err("Live-domain controls require an attached native window".into());
        };
        let Some(proxy) = EVENT_LOOP_PROXY.get().cloned() else {
            self.pending_live_domain_controls.remove(&control);
            self.restore_live_domain_preview(&control);
            return Err("Live-domain control channel is unavailable".into());
        };
        let Some(authority) = self.live_domain_authority.clone() else {
            self.pending_live_domain_controls.remove(&control);
            self.restore_live_domain_preview(&control);
            return Err("Live-domain control authority is unavailable".into());
        };
        let worker_control = control.clone();
        let spawn = std::thread::Builder::new()
            .name("termeric-domain-control".into())
            .spawn(move || {
                let result = authority
                    .client()
                    .and_then(|client| client.request(worker_control.request()))
                    .and_then(|response| match response {
                        crate::domain::protocol::Response::Domain { domain } => Ok(domain),
                        _ => anyhow::bail!(
                            "domain server returned an invalid topology-control response"
                        ),
                    })
                    .map_err(|error| format!("{error:#}"));
                let _ = proxy.send_event(AppEvent::LiveDomainControlCompleted {
                    window_id,
                    control: worker_control,
                    result,
                });
            });
        if let Err(error) = spawn {
            self.pending_live_domain_controls.remove(&control);
            self.restore_live_domain_preview(&control);
            return Err(format!("Could not start live-domain control: {error}"));
        }
        Ok(())
    }

    fn restore_live_domain_preview(&mut self, control: &LiveDomainControl) {
        let domain_id = match control {
            LiveDomainControl::ResizeSplit { domain_id, .. }
            | LiveDomainControl::RenameTab { domain_id, .. }
            | LiveDomainControl::MoveTab { domain_id, .. } => domain_id,
            LiveDomainControl::SpawnTab { .. }
            | LiveDomainControl::SplitPane { .. }
            | LiveDomainControl::SetTabPinned { .. }
            | LiveDomainControl::SwapPaneNext { .. }
            | LiveDomainControl::BalanceTab { .. }
            | LiveDomainControl::ApplyPaneLayout { .. }
            | LiveDomainControl::MovePane { .. }
            | LiveDomainControl::ClosePane { .. }
            | LiveDomainControl::CloseTab { .. } => return,
        };
        let Some(topology) = self.live_domain_topologies.get(domain_id).cloned() else {
            return;
        };
        let Some(relay_command) = self.live_domain_relay_command.as_ref() else {
            return;
        };
        let Some(tabs) = &mut self.tabs else {
            return;
        };
        if let Err(error) = tabs.reconcile_live_domain(&topology, relay_command) {
            log::error!("Could not restore rejected live-domain preview: {error:#}");
        }
    }

    pub(super) fn live_domain_spawn_spec(
        &self,
        inherit_visible_cwd: bool,
    ) -> crate::domain::model::SpawnSpec {
        let (cols, rows, working_directory) = self
            .active_terminal()
            .map(|terminal| {
                (
                    terminal.grid().cols().min(1_000) as u16,
                    terminal.grid().rows().min(1_000) as u16,
                    inherit_visible_cwd
                        .then(|| crate::tabs::terminal_working_directory(terminal))
                        .flatten(),
                )
            })
            .unwrap_or((80, 24, None));
        crate::domain::model::SpawnSpec {
            program: self.config.shell.program.clone(),
            args: self.config.shell.args.clone(),
            working_directory,
            title: None,
            cols,
            rows,
        }
    }

    pub(super) fn control_live_domain_create_tab(
        &self,
    ) -> std::result::Result<Option<LiveDomainControl>, String> {
        let Some(tab) = self.tabs.as_ref().and_then(TabManager::active_tab) else {
            return Err("Target window has no active tab".into());
        };
        if tab.closed || tab.closing_at.is_some() {
            return Err(format!("GUI tab {} is closing", tab.id));
        }
        let Some((domain_id, _)) = tab.live_domain_tab_route() else {
            return Ok(None);
        };
        Ok(Some(LiveDomainControl::SpawnTab {
            domain_id: domain_id.to_string(),
            spawn: self.live_domain_spawn_spec(true),
        }))
    }

    pub(super) fn control_live_domain_split(
        &self,
        tab_id: u64,
        pane_id: u32,
        direction: crate::control::protocol::SplitDirection,
    ) -> std::result::Result<Option<LiveDomainControl>, String> {
        let tab_id = usize::try_from(tab_id)
            .map_err(|_| "Target tab ID does not fit this platform".to_string())?;
        let tabs = self
            .tabs
            .as_ref()
            .ok_or_else(|| "Target window has no initialized tab manager".to_string())?;
        let tab = tabs
            .tab_by_id(tab_id)
            .ok_or_else(|| format!("GUI tab {tab_id} does not exist in the target window"))?;
        if tab.closed || tab.closing_at.is_some() {
            return Err(format!("GUI tab {tab_id} is closing"));
        }
        let Some(live) = &tab.live_domain else {
            return Ok(None);
        };
        let remote_pane_id = *live
            .pane_ids
            .get(&PaneId(pane_id))
            .ok_or_else(|| format!("GUI pane {pane_id} does not exist in tab {tab_id}"))?;
        let terminal = if let Some(manager) = &tab.pane_manager {
            &manager
                .get_pane(PaneId(pane_id))
                .ok_or_else(|| format!("GUI pane {pane_id} does not exist in tab {tab_id}"))?
                .terminal
        } else {
            if tab.single_pane_id() != PaneId(pane_id) {
                return Err(format!("GUI pane {pane_id} does not exist in tab {tab_id}"));
            }
            tab.focused_terminal()
                .ok_or_else(|| format!("GUI pane {pane_id} has no live terminal"))?
        };
        let spawn = crate::domain::model::SpawnSpec {
            program: self.config.shell.program.clone(),
            args: self.config.shell.args.clone(),
            working_directory: None,
            title: None,
            cols: terminal.cols().min(1_000) as u16,
            rows: terminal.rows().min(1_000) as u16,
        };
        Ok(Some(LiveDomainControl::SplitPane {
            domain_id: live.domain_id.clone(),
            remote_pane_id,
            direction: match direction {
                crate::control::protocol::SplitDirection::Horizontal => {
                    crate::domain::model::SplitDirection::Horizontal
                }
                crate::control::protocol::SplitDirection::Vertical => {
                    crate::domain::model::SplitDirection::Vertical
                }
            },
            spawn,
        }))
    }

    pub(super) fn control_live_domain_close_tab(
        &self,
        tab_id: u64,
    ) -> std::result::Result<Option<LiveDomainControl>, String> {
        let tab_id = usize::try_from(tab_id)
            .map_err(|_| "Target tab ID does not fit this platform".to_string())?;
        let tabs = self
            .tabs
            .as_ref()
            .ok_or_else(|| "Target window has no initialized tab manager".to_string())?;
        let tab = tabs
            .tab_by_id(tab_id)
            .ok_or_else(|| format!("GUI tab {tab_id} does not exist in the target window"))?;
        if tab.closed || tab.closing_at.is_some() {
            return Err(format!("GUI tab {tab_id} is already closing"));
        }
        let Some((domain_id, remote_tab_id)) = tab.live_domain_tab_route() else {
            return Ok(None);
        };
        let domain_tabs = self.live_domain_topologies.get(domain_id).map_or_else(
            || tabs.live_domain_tab_count(domain_id),
            |topology| topology.tabs.len(),
        );
        if domain_tabs <= 1 {
            return Err(
                "Cannot close the domain's last tab; use `termeric domain kill` to terminate it"
                    .into(),
            );
        }
        Ok(Some(LiveDomainControl::CloseTab {
            domain_id: domain_id.to_string(),
            remote_tab_id,
            local_tab_id: tab_id,
        }))
    }

    pub(super) fn control_live_domain_close_pane(
        &self,
        tab_id: u64,
        pane_id: u32,
    ) -> std::result::Result<Option<LiveDomainControl>, String> {
        let tab_id = usize::try_from(tab_id)
            .map_err(|_| "Target tab ID does not fit this platform".to_string())?;
        let tabs = self
            .tabs
            .as_ref()
            .ok_or_else(|| "Target window has no initialized tab manager".to_string())?;
        let tab = tabs
            .tab_by_id(tab_id)
            .ok_or_else(|| format!("GUI tab {tab_id} does not exist in the target window"))?;
        if tab.closed || tab.closing_at.is_some() {
            return Err(format!("GUI tab {tab_id} is closing"));
        }
        let Some(live) = &tab.live_domain else {
            return Ok(None);
        };
        let target = PaneId(pane_id);
        let remote_pane_id = *live
            .pane_ids
            .get(&target)
            .ok_or_else(|| format!("GUI pane {pane_id} does not exist in tab {tab_id}"))?;
        let pane_count = tab.pane_manager.as_ref().map_or(1, PaneManager::pane_count);
        if pane_count <= 1 {
            return Err(format!(
                "Cannot close the sole pane {pane_id} in unsplit tab {tab_id}; close the tab instead"
            ));
        }
        let pane = tab
            .pane_manager
            .as_ref()
            .and_then(|panes| panes.get_pane(target))
            .ok_or_else(|| format!("GUI pane {pane_id} does not exist in tab {tab_id}"))?;
        if pane.closed {
            return Err(format!(
                "GUI pane {pane_id} in tab {tab_id} is already closed"
            ));
        }
        Ok(Some(LiveDomainControl::ClosePane {
            domain_id: live.domain_id.clone(),
            remote_pane_id,
            local_tab_id: tab_id,
        }))
    }

    pub(super) fn control_live_pane_status(
        &self,
        tab_id: u64,
    ) -> std::result::Result<(u16, u32), String> {
        let tab_id = usize::try_from(tab_id)
            .map_err(|_| "Target tab ID does not fit this platform".to_string())?;
        let tab = self
            .tabs
            .as_ref()
            .and_then(|tabs| tabs.tab_by_id(tab_id))
            .ok_or_else(|| format!("GUI tab {tab_id} disappeared after live-domain closure"))?;
        let (count, focused) = tab.pane_manager.as_ref().map_or_else(
            || (1, tab.single_pane_id().0),
            |panes| (panes.pane_count(), panes.focused_id().0),
        );
        Ok((
            u16::try_from(count).expect("pane count is bounded below u16::MAX"),
            focused,
        ))
    }

    pub(super) fn control_live_domain_resize_divider(
        &self,
        tab_id: u64,
        divider_id: u64,
        ratio_millis: u16,
    ) -> std::result::Result<Option<(LiveDomainControl, u16)>, String> {
        if !(100..=900).contains(&ratio_millis) {
            return Err("Split ratio must be between 100 and 900 thousandths".into());
        }
        let tab_id = usize::try_from(tab_id)
            .map_err(|_| "Target tab ID does not fit this platform".to_string())?;
        let tabs = self
            .tabs
            .as_ref()
            .ok_or_else(|| "Target window has no initialized tab manager".to_string())?;
        let tab = tabs
            .tab_by_id(tab_id)
            .ok_or_else(|| format!("GUI tab {tab_id} does not exist in the target window"))?;
        if tab.closed || tab.closing_at.is_some() {
            return Err(format!("GUI tab {tab_id} is closing"));
        }
        let Some((domain_id, _)) = tab.live_domain_tab_route() else {
            return Ok(None);
        };
        let remote_split_id = u32::try_from(divider_id)
            .map_err(|_| format!("GUI divider {divider_id} is not a live-domain divider"))?;
        let previous_ratio_millis = tab
            .pane_manager
            .as_ref()
            .ok_or_else(|| format!("GUI tab {tab_id} has no split dividers"))?
            .get_dividers()
            .into_iter()
            .find_map(|divider| {
                (divider.target == crate::panes::DividerTarget::Domain(remote_split_id))
                    .then_some(divider.ratio_millis)
            })
            .ok_or_else(|| format!("GUI divider {divider_id} does not exist in tab {tab_id}"))?;
        Ok(Some((
            LiveDomainControl::ResizeSplit {
                domain_id: domain_id.to_string(),
                remote_split_id,
                ratio_millis,
            },
            previous_ratio_millis,
        )))
    }

    pub(super) fn control_live_divider_ratio(
        &self,
        tab_id: u64,
        divider_id: u64,
    ) -> std::result::Result<u16, String> {
        let tab_id = usize::try_from(tab_id)
            .map_err(|_| "Target tab ID does not fit this platform".to_string())?;
        let remote_split_id = u32::try_from(divider_id)
            .map_err(|_| format!("GUI divider {divider_id} is not a live-domain divider"))?;
        self.tabs
            .as_ref()
            .and_then(|tabs| tabs.tab_by_id(tab_id))
            .and_then(|tab| tab.pane_manager.as_ref())
            .and_then(|panes| {
                panes.get_dividers().into_iter().find_map(|divider| {
                    (divider.target == crate::panes::DividerTarget::Domain(remote_split_id))
                        .then_some(divider.ratio_millis)
                })
            })
            .ok_or_else(|| {
                format!("GUI divider {divider_id} is unavailable after live-domain resize")
            })
    }

    pub(super) fn control_live_domain_move_tab(
        &self,
        tab_id: u64,
        destination_index: u16,
    ) -> std::result::Result<Option<(LiveDomainControl, bool)>, String> {
        let tab_id = usize::try_from(tab_id)
            .map_err(|_| "Target tab ID does not fit this platform".to_string())?;
        let destination_index = usize::from(destination_index);
        let tabs = self
            .tabs
            .as_ref()
            .ok_or_else(|| "Target window has no initialized tab manager".to_string())?;
        if destination_index >= tabs.count() {
            return Err(format!(
                "Destination tab index {destination_index} is outside this window's {} tabs",
                tabs.count()
            ));
        }
        let (source_index, source) = tabs
            .iter()
            .find(|(_, tab)| tab.id == tab_id)
            .ok_or_else(|| format!("GUI tab {tab_id} does not exist in the source window"))?;
        if source.closed || source.closing_at.is_some() {
            return Err(format!("GUI tab {tab_id} is closing"));
        }
        let Some((domain_id, remote_tab_id)) = source.live_domain_tab_route() else {
            return Ok(None);
        };
        let destination = tabs
            .iter()
            .find_map(|(index, tab)| (index == destination_index).then_some(tab))
            .expect("bounded destination tab index remains present");
        let Some((destination_domain, destination_remote_tab)) =
            destination.live_domain_tab_route()
        else {
            return Err(
                "A live-domain tab can move only to a visual slot owned by the same domain".into(),
            );
        };
        if destination_domain != domain_id {
            return Err(
                "A live-domain tab can move only to a visual slot owned by the same domain".into(),
            );
        }
        if destination.pinned != source.pinned {
            return Err("Destination tab index crosses the pinned-tab boundary".into());
        }
        let domain_index = tabs
            .iter()
            .take(destination_index)
            .filter(|(_, tab)| {
                tab.live_domain_tab_route()
                    .is_some_and(|(candidate, _)| candidate == domain_id)
            })
            .count();
        let topology = self
            .live_domain_topologies
            .get(domain_id)
            .ok_or_else(|| "Authoritative live-domain topology is unavailable".to_string())?;
        if topology.tabs.get(domain_index).map(|tab| tab.id) != Some(destination_remote_tab) {
            return Err(
                "GUI live-domain tab order is stale; wait for topology reconciliation".into(),
            );
        }
        let domain_index = u16::try_from(domain_index)
            .map_err(|_| "Live-domain tab index does not fit the control protocol".to_string())?;
        Ok(Some((
            LiveDomainControl::MoveTab {
                domain_id: domain_id.to_string(),
                remote_tab_id,
                index: domain_index,
            },
            source_index != destination_index,
        )))
    }

    pub(super) fn control_tab_visual_index(&self, tab_id: u64) -> std::result::Result<u16, String> {
        let tab_id = usize::try_from(tab_id)
            .map_err(|_| "Target tab ID does not fit this platform".to_string())?;
        let index = self
            .tabs
            .as_ref()
            .and_then(|tabs| {
                tabs.iter()
                    .find_map(|(index, tab)| (tab.id == tab_id).then_some(index))
            })
            .ok_or_else(|| format!("GUI tab {tab_id} disappeared after live-domain movement"))?;
        u16::try_from(index).map_err(|_| "GUI tab index does not fit the control protocol".into())
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn control_live_domain_move_pane(
        &self,
        source_tab_id: u64,
        pane_id: u32,
        destination_tab_id: u64,
        target_pane_id: u32,
        direction: crate::control::protocol::SplitDirection,
    ) -> std::result::Result<Option<LiveDomainControl>, String> {
        let source_tab_id = usize::try_from(source_tab_id)
            .map_err(|_| "Source tab ID does not fit this platform".to_string())?;
        let destination_tab_id = usize::try_from(destination_tab_id)
            .map_err(|_| "Destination tab ID does not fit this platform".to_string())?;
        if source_tab_id == destination_tab_id {
            return Err("Moving a pane within the same tab is not supported".into());
        }
        let tabs = self
            .tabs
            .as_ref()
            .ok_or_else(|| "Source window has no initialized tab manager".to_string())?;
        let source = tabs.tab_by_id(source_tab_id).ok_or_else(|| {
            format!("GUI tab {source_tab_id} does not exist in the source window")
        })?;
        let destination = tabs.tab_by_id(destination_tab_id).ok_or_else(|| {
            format!("GUI tab {destination_tab_id} does not exist in the destination window")
        })?;
        if source.closed || source.closing_at.is_some() {
            return Err(format!("GUI source tab {source_tab_id} is closing"));
        }
        if destination.closed || destination.closing_at.is_some() {
            return Err(format!(
                "GUI destination tab {destination_tab_id} is closing"
            ));
        }
        let (source_live, destination_live) = (&source.live_domain, &destination.live_domain);
        let (Some(source_live), Some(destination_live)) = (source_live, destination_live) else {
            if source_live.is_none() && destination_live.is_none() {
                return Ok(None);
            }
            return Err("Live-domain panes can move only between tabs in the same domain".into());
        };
        if source_live.domain_id != destination_live.domain_id {
            return Err("Live-domain panes can move only between tabs in the same domain".into());
        }
        let source_pane = PaneId(pane_id);
        let remote_pane_id = *source_live.pane_ids.get(&source_pane).ok_or_else(|| {
            format!("GUI pane {pane_id} does not exist in source tab {source_tab_id}")
        })?;
        let source_manager = source.pane_manager.as_ref().ok_or_else(|| {
            format!("Cannot move the sole pane {pane_id} from unsplit tab {source_tab_id}")
        })?;
        if source_manager.pane_count() <= 1 {
            return Err(format!(
                "Cannot move the last pane {pane_id} from tab {source_tab_id}"
            ));
        }
        if source_manager
            .get_pane(source_pane)
            .is_none_or(|pane| pane.closed)
        {
            return Err(format!(
                "GUI pane {pane_id} in source tab {source_tab_id} is closed or missing"
            ));
        }
        let target = PaneId(target_pane_id);
        let remote_target_pane_id = *destination_live
            .pane_ids
            .get(&target)
            .ok_or_else(|| {
                format!(
                    "GUI target pane {target_pane_id} does not exist in destination tab {destination_tab_id}"
                )
            })?;
        if destination
            .pane_manager
            .as_ref()
            .is_some_and(|panes| panes.get_pane(target).is_none_or(|pane| pane.closed))
        {
            return Err(format!(
                "GUI target pane {target_pane_id} in destination tab {destination_tab_id} is closed or missing"
            ));
        }
        Ok(Some(LiveDomainControl::MovePane {
            domain_id: source_live.domain_id.clone(),
            remote_pane_id,
            source_tab_id: source_live.tab_id,
            destination_tab_id: destination_live.tab_id,
            target_pane_id: remote_target_pane_id,
            direction: match direction {
                crate::control::protocol::SplitDirection::Horizontal => {
                    crate::domain::model::SplitDirection::Horizontal
                }
                crate::control::protocol::SplitDirection::Vertical => {
                    crate::domain::model::SplitDirection::Vertical
                }
            },
        }))
    }

    pub(super) fn control_local_live_tab_id(
        &self,
        domain_id: &str,
        remote_tab_id: u32,
    ) -> Option<u64> {
        self.tabs.as_ref()?.all_tabs().iter().find_map(|tab| {
            (tab.live_domain_tab_route() == Some((domain_id, remote_tab_id)))
                .then_some(tab.id as u64)
        })
    }

    pub(super) fn control_local_live_pane_id(
        &self,
        domain_id: &str,
        remote_tab_id: u32,
        remote_pane_id: u32,
    ) -> Option<u32> {
        self.tabs.as_ref()?.all_tabs().iter().find_map(|tab| {
            let live = tab.live_domain.as_ref()?;
            if live.domain_id != domain_id || live.tab_id != remote_tab_id {
                return None;
            }
            live.pane_ids
                .iter()
                .find_map(|(local, remote)| (*remote == remote_pane_id).then_some(local.0))
        })
    }

    pub(super) fn handle_live_domain_control_completed(
        &mut self,
        control: LiveDomainControl,
        result: std::result::Result<crate::domain::model::DomainSnapshot, String>,
    ) {
        self.pending_live_domain_controls.remove(&control);
        let Ok(snapshot) = result else {
            let error = result.unwrap_err();
            self.restore_live_domain_preview(&control);
            self.toasts
                .error(format!("Live-domain change failed: {error}"));
            if let Some(window) = &self.window {
                window.request_redraw();
            }
            return;
        };
        let topology = crate::domain::model::DomainTopology::from(&snapshot);
        let before: std::collections::HashMap<_, _> = self
            .tabs
            .as_ref()
            .into_iter()
            .flat_map(TabManager::iter)
            .filter_map(|(_, tab)| {
                tab.live_domain_tab_route().and_then(|(candidate, remote)| {
                    (candidate == topology.domain_id).then_some((remote, tab.id))
                })
            })
            .collect();
        let remote_tabs: std::collections::HashSet<_> =
            topology.tabs.iter().map(|tab| tab.id).collect();
        let reconcile = self
            .tabs
            .as_mut()
            .zip(self.live_domain_relay_command.as_ref())
            .ok_or_else(|| anyhow::anyhow!("live-domain relay state is unavailable"))
            .and_then(|(tabs, relay)| tabs.reconcile_live_domain(&topology, relay));
        match reconcile {
            Ok(changed) => {
                self.live_domain_topologies
                    .insert(topology.domain_id.clone(), topology.clone());
                let after: std::collections::HashMap<_, _> = self
                    .tabs
                    .as_ref()
                    .into_iter()
                    .flat_map(TabManager::iter)
                    .filter_map(|(_, tab)| {
                        tab.live_domain_tab_route().and_then(|(candidate, remote)| {
                            (candidate == topology.domain_id).then_some((remote, tab.id))
                        })
                    })
                    .collect();
                for tab_id in after
                    .iter()
                    .filter_map(|(remote, local)| (!before.contains_key(remote)).then_some(*local))
                {
                    self.dispatch_plugin_hook(crate::plugin::Hook::TabCreated { tab_id });
                }
                for tab_id in before
                    .iter()
                    .filter_map(|(remote, local)| (!remote_tabs.contains(remote)).then_some(*local))
                {
                    self.dispatch_plugin_hook(crate::plugin::Hook::TabClosed { tab_id });
                }
                if changed {
                    self.reconcile_broadcast_state();
                    if let Some(size) = self.window.as_ref().map(|window| window.inner_size()) {
                        self.sync_terminal_geometry(size.width, size.height);
                    }
                    self.refresh_focused_terminal_surfaces();
                }
            }
            Err(error) => self.toasts.error(format!(
                "Live-domain mutation succeeded but GUI reconciliation failed: {error:#}"
            )),
        }
        {
            let (changed, message, closed_tab) = match control {
                LiveDomainControl::SpawnTab { .. } => {
                    (false, "Created live-domain tab".to_string(), None)
                }
                LiveDomainControl::SplitPane { remote_pane_id, .. } => (
                    false,
                    format!("Split live-domain pane {remote_pane_id}"),
                    None,
                ),
                LiveDomainControl::ResizeSplit {
                    remote_split_id,
                    ratio_millis,
                    ..
                } => (
                    false,
                    format!("Resized live-domain split {remote_split_id} to {ratio_millis}‰"),
                    None,
                ),
                LiveDomainControl::RenameTab {
                    remote_tab_id,
                    title,
                    ..
                } => (
                    false,
                    title.map_or_else(
                        || format!("Cleared live-domain tab {remote_tab_id} title"),
                        |title| format!("Renamed live-domain tab {remote_tab_id} to {title}"),
                    ),
                    None,
                ),
                LiveDomainControl::SetTabPinned {
                    remote_tab_id,
                    pinned,
                    ..
                } => (
                    false,
                    format!(
                        "{} live-domain tab {remote_tab_id}",
                        if pinned { "Pinned" } else { "Unpinned" }
                    ),
                    None,
                ),
                LiveDomainControl::MoveTab {
                    remote_tab_id,
                    index,
                    ..
                } => (
                    false,
                    format!("Moved live-domain tab {remote_tab_id} to position {index}"),
                    None,
                ),
                LiveDomainControl::SwapPaneNext { remote_pane_id, .. } => (
                    false,
                    format!("Swapped live-domain pane {remote_pane_id} with its next pane"),
                    None,
                ),
                LiveDomainControl::BalanceTab { remote_tab_id, .. } => (
                    false,
                    format!("Balanced live-domain tab {remote_tab_id}"),
                    None,
                ),
                LiveDomainControl::ApplyPaneLayout {
                    remote_tab_id,
                    preset,
                    ..
                } => (
                    false,
                    format!("Applied {preset:?} layout to live-domain tab {remote_tab_id}"),
                    None,
                ),
                LiveDomainControl::MovePane {
                    remote_pane_id,
                    destination_tab_id,
                    ..
                } => (
                    false,
                    format!("Moved live-domain pane {remote_pane_id} to tab {destination_tab_id}"),
                    None,
                ),
                LiveDomainControl::ClosePane {
                    remote_pane_id,
                    local_tab_id,
                    ..
                } => (
                    self.tabs.as_mut().is_some_and(|tabs| {
                        tabs.reconcile_closed_live_domain_pane(local_tab_id, remote_pane_id)
                    }),
                    format!("Closed live-domain pane {remote_pane_id}"),
                    None,
                ),
                LiveDomainControl::CloseTab {
                    remote_tab_id,
                    local_tab_id,
                    ..
                } => (
                    self.tabs
                        .as_mut()
                        .is_some_and(|tabs| tabs.close_tab_by_id(local_tab_id)),
                    format!("Closed live-domain tab {remote_tab_id}"),
                    Some(local_tab_id),
                ),
            };
            if changed && let Some(tab_id) = closed_tab {
                self.dispatch_plugin_hook(crate::plugin::Hook::TabClosed { tab_id });
            }
            if changed {
                self.reconcile_broadcast_state();
                if let Some(size) = self.window.as_ref().map(|window| window.inner_size()) {
                    self.sync_terminal_geometry(size.width, size.height);
                }
                self.refresh_focused_terminal_surfaces();
            }
            self.toasts.success(message);
            if let Some(window) = &self.window {
                window.request_redraw();
            }
        }
    }

    pub(super) fn handle_live_domain_topology(
        &mut self,
        domain_id: String,
        result: std::result::Result<crate::domain::DomainTopologyEvent, String>,
    ) {
        let event = match result {
            Ok(event) => event,
            Err(error) => {
                if self.disconnected_live_domains.insert(domain_id.clone()) {
                    let route = self.live_domain_route_label(&domain_id);
                    self.toasts
                        .warning(format!("Live {route} disconnected; retrying: {error}"));
                }
                if let Some(window) = &self.window {
                    window.request_redraw();
                }
                return;
            }
        };
        match event {
            crate::domain::DomainTopologyEvent::Snapshot { revision, domain } => {
                if domain.domain_id != domain_id {
                    self.toasts.error(format!(
                        "Rejected live-domain topology for {}; expected {domain_id}",
                        domain.domain_id
                    ));
                    if let Some(window) = &self.window {
                        window.request_redraw();
                    }
                    return;
                }
                let applied_revision = self.live_domain_revisions.get(&domain.domain_id).copied();
                let recovering = self.disconnected_live_domains.contains(&domain_id);
                if applied_revision.is_some_and(|applied| {
                    applied > revision || (applied == revision && !recovering)
                }) {
                    return;
                }
                let Some(relay_command) = self.live_domain_relay_command.as_ref() else {
                    self.toasts.error(
                        "Live-domain topology arrived without a configured relay executable",
                    );
                    return;
                };
                let before: std::collections::HashMap<_, _> = self
                    .tabs
                    .as_ref()
                    .into_iter()
                    .flat_map(TabManager::iter)
                    .filter_map(|(_, tab)| {
                        tab.live_domain_tab_route().and_then(|(candidate, remote)| {
                            (candidate == domain.domain_id).then_some((remote, tab.id))
                        })
                    })
                    .collect();
                let remote_tabs: std::collections::HashSet<_> =
                    domain.tabs.iter().map(|tab| tab.id).collect();
                let reconcile = self
                    .tabs
                    .as_mut()
                    .ok_or_else(|| anyhow::anyhow!("tab manager is unavailable"))
                    .and_then(|tabs| tabs.reconcile_live_domain(&domain, relay_command));
                match reconcile {
                    Ok(changed) => {
                        self.live_domain_revisions
                            .insert(domain.domain_id.clone(), revision);
                        self.live_domain_topologies
                            .insert(domain.domain_id.clone(), domain.clone());
                        if let Some(rename) = &self.tab_rename
                            && let Some(tab) = self
                                .tabs
                                .as_mut()
                                .and_then(|tabs| tabs.tab_mut_by_id(rename.tab_id))
                        {
                            tab.set_user_title(Some(rename.input.clone()));
                        }
                        let after: std::collections::HashMap<_, _> = self
                            .tabs
                            .as_ref()
                            .into_iter()
                            .flat_map(TabManager::iter)
                            .filter_map(|(_, tab)| {
                                tab.live_domain_tab_route().and_then(|(candidate, remote)| {
                                    (candidate == domain.domain_id).then_some((remote, tab.id))
                                })
                            })
                            .collect();
                        for tab_id in after.iter().filter_map(|(remote, local)| {
                            (!before.contains_key(remote)).then_some(*local)
                        }) {
                            self.dispatch_plugin_hook(crate::plugin::Hook::TabCreated { tab_id });
                        }
                        for tab_id in before.iter().filter_map(|(remote, local)| {
                            (!remote_tabs.contains(remote)).then_some(*local)
                        }) {
                            self.dispatch_plugin_hook(crate::plugin::Hook::TabClosed { tab_id });
                        }
                        if changed {
                            self.reconcile_broadcast_state();
                            if let Some(size) =
                                self.window.as_ref().map(|window| window.inner_size())
                            {
                                self.sync_terminal_geometry(size.width, size.height);
                            }
                            self.refresh_focused_terminal_surfaces();
                        }
                        if self.disconnected_live_domains.remove(&domain_id) {
                            let route = self.live_domain_route_label(&domain_id);
                            self.toasts.success(format!("Reconnected live {route}"));
                        }
                    }
                    Err(error) => self.toasts.error(format!(
                        "Could not apply live-domain topology revision {revision}: {error:#}"
                    )),
                }
            }
            crate::domain::DomainTopologyEvent::Closed { domain, message } => {
                if domain != domain_id {
                    self.toasts.error(format!(
                        "Rejected live-domain close for {domain}; expected {domain_id}"
                    ));
                    if let Some(window) = &self.window {
                        window.request_redraw();
                    }
                    return;
                }
                self.disconnected_live_domains.remove(&domain);
                self.live_domain_revisions.remove(&domain);
                self.live_domain_topologies.remove(&domain);
                let closed = self
                    .tabs
                    .as_mut()
                    .map_or(0, |tabs| tabs.close_live_domain_tabs(&domain));
                if closed > 0 {
                    self.reconcile_broadcast_state();
                    self.refresh_focused_terminal_surfaces();
                }
                self.toasts
                    .info(format!("Live domain {domain} closed: {message}"));
            }
        }
        if let Some(window) = &self.window {
            window.request_redraw();
        }
    }

    fn live_domain_route_label(&self, domain_id: &str) -> String {
        match &self.live_domain_authority {
            Some(crate::domain::DomainAuthority::Local) => {
                format!("domain {domain_id} on this host")
            }
            Some(crate::domain::DomainAuthority::Ssh { destination }) => {
                format!("domain {domain_id} via SSH to {destination}")
            }
            None => format!("domain {domain_id}"),
        }
    }

    pub(super) fn request_close_tab(&mut self, index: usize) {
        let route = self.tabs.as_ref().and_then(|tabs| {
            tabs.iter()
                .find(|(candidate, _)| *candidate == index)
                .and_then(|(_, tab)| {
                    tab.live_domain_tab_route().map(|(domain, remote_tab)| {
                        (
                            tab.id,
                            domain.to_string(),
                            remote_tab,
                            tabs.live_domain_tab_count(domain),
                        )
                    })
                })
        });
        if let Some((local_tab_id, domain_id, remote_tab_id, domain_tabs)) = route {
            if domain_tabs <= 1 {
                self.toasts.warning(
                    "Cannot close the domain's last tab; use `termeric domain kill` to terminate it",
                );
                return;
            }
            self.queue_live_domain_control(LiveDomainControl::CloseTab {
                domain_id,
                remote_tab_id,
                local_tab_id,
            });
            return;
        }

        let tab_id = self.tabs.as_ref().and_then(|tabs| {
            tabs.iter()
                .find(|(candidate, _)| *candidate == index)
                .map(|(_, tab)| tab.id)
        });
        let closed = self.tabs.as_mut().is_some_and(|tabs| tabs.close_tab(index));
        if closed {
            if let Some(tab_id) = tab_id {
                self.dispatch_plugin_hook(crate::plugin::Hook::TabClosed { tab_id });
            }
            self.refresh_focused_terminal_surfaces();
        }
    }

    pub(super) fn request_new_tab(&mut self) {
        let live_domain = self
            .tabs
            .as_ref()
            .and_then(TabManager::active_tab)
            .and_then(crate::tabs::Tab::live_domain_tab_route)
            .map(|(domain, _)| domain.to_string());
        if let Some(domain_id) = live_domain {
            let spawn = self.live_domain_spawn_spec(true);
            self.queue_live_domain_control(LiveDomainControl::SpawnTab { domain_id, spawn });
            return;
        }
        if let Err(error) = self.create_ordinary_tab(true) {
            log::error!("Failed to create new tab: {error}");
        }
    }

    /// Create a tab synchronously for the private running-GUI control plane.
    /// Live-domain tabs use an asynchronous topology round trip, so they are
    /// rejected until the control protocol can acknowledge the resulting
    /// stable local ID without blocking the event loop.
    pub(super) fn create_control_tab(&mut self) -> std::result::Result<u64, String> {
        self.create_ordinary_tab(true)
    }

    /// Launch exact argv in an ordinary local tab for the private running-GUI
    /// control plane. The caller has already passed bounded wire validation.
    pub(super) fn launch_control_tab(
        &mut self,
        background: bool,
        program: &str,
        args: &[String],
        working_directory: Option<&str>,
        title: Option<&str>,
        environment: &[crate::control::protocol::EnvironmentVariable],
        environment_policy: &termeric_core::pty::EnvironmentPolicy,
    ) -> std::result::Result<u64, String> {
        let tab_count = self.tabs.as_ref().map_or(0, TabManager::count);
        validate_ordinary_tab_creation_capacity(tab_count)?;
        if self
            .tabs
            .as_ref()
            .and_then(TabManager::active_tab)
            .is_some_and(|tab| tab.live_domain_tab_route().is_some())
        {
            return Err(
                "Exact local command launch is unavailable while the target window's active tab belongs to a live domain; use `termeric domain spawn`"
                    .into(),
            );
        }
        if let Some(directory) = working_directory
            && !std::path::Path::new(directory).is_dir()
        {
            return Err(format!(
                "GUI control launch working directory {directory:?} does not exist or is not a directory"
            ));
        }
        let tabs = self
            .tabs
            .as_mut()
            .ok_or_else(|| "Target window has no initialized tab manager".to_string())?;
        let id = tabs
            .new_tab_with_command_placement(
                program,
                args,
                working_directory,
                title,
                None,
                environment
                    .iter()
                    .map(|entry| (entry.name.clone(), entry.value.clone()))
                    .collect(),
                environment_policy,
                background,
            )
            .map_err(|error| format!("Could not launch command tab: {error:#}"))?;
        self.dispatch_plugin_hook(crate::plugin::Hook::TabCreated { tab_id: id });
        if !background || tab_count == 0 {
            self.refresh_focused_terminal_surfaces();
        } else {
            // A quiet child may never emit a PTY wake. Update tab chrome without
            // invalidating the current pane's IME/search/selection surfaces.
            self.redraw_scheduler.request(RedrawReason::WindowEvent);
            if let Some(window) = &self.window {
                window.request_redraw();
            }
        }
        u64::try_from(id).map_err(|_| "Created tab ID does not fit the control protocol".into())
    }

    /// Create one ordinary tab for a declarative plugin without redispatching
    /// the derived tab-created hook. The correlated action result is the only
    /// follow-up event, preventing lifecycle-driven creation loops.
    pub(super) fn create_plugin_tab(&mut self) -> std::result::Result<u64, String> {
        self.create_ordinary_tab(false)
    }

    fn create_ordinary_tab(
        &mut self,
        dispatch_plugin_hook: bool,
    ) -> std::result::Result<u64, String> {
        let tab_count = self.tabs.as_ref().map_or(0, TabManager::count);
        validate_ordinary_tab_creation_capacity(tab_count)?;
        if self
            .tabs
            .as_ref()
            .and_then(TabManager::active_tab)
            .is_some_and(|tab| tab.live_domain_tab_route().is_some())
        {
            return Err(
                "Ordinary local tab creation is unavailable while the target window's active tab belongs to a live domain"
                    .into(),
            );
        }
        let id = self.create_local_tab_in(dispatch_plugin_hook, None)?;
        u64::try_from(id).map_err(|_| "Created tab ID does not fit the control protocol".into())
    }

    /// Opens a shell tab, in `cwd` when given.
    pub(super) fn create_local_tab_in(
        &mut self,
        dispatch_plugin_hook: bool,
        cwd: Option<&std::path::Path>,
    ) -> std::result::Result<usize, String> {
        let tab_count = self.tabs.as_ref().map_or(0, TabManager::count);
        validate_ordinary_tab_creation_capacity(tab_count)?;
        let tabs = self
            .tabs
            .as_mut()
            .ok_or_else(|| "Target window has no initialized tab manager".to_string())?;
        let id = match cwd {
            Some(cwd) => tabs.new_tab_with_cwd(Some(&cwd.to_string_lossy()), None),
            None => tabs.new_tab(),
        }
        .map_err(|error| format!("Could not create a new tab: {error:#}"))?;
        log::info!("Created new tab {id}");
        self.maybe_show_splash(true);
        if dispatch_plugin_hook {
            self.dispatch_plugin_hook(crate::plugin::Hook::TabCreated { tab_id: id });
        }
        self.refresh_focused_terminal_surfaces();
        Ok(id)
    }

    pub(super) fn select_control_tab(&mut self, tab_id: u64) -> std::result::Result<bool, String> {
        self.select_tab_by_stable_id(tab_id, true)
    }

    /// Select a stable tab for a declarative plugin without redispatching the
    /// derived tab-switched hook. The correlated action-result is
    /// the only follow-up event, preventing hook-driven navigation loops.
    pub(super) fn select_plugin_tab(&mut self, tab_id: u64) -> std::result::Result<bool, String> {
        self.select_tab_by_stable_id(tab_id, false)
    }

    pub(super) fn select_tab_by_stable_id(
        &mut self,
        tab_id: u64,
        dispatch_hook: bool,
    ) -> std::result::Result<bool, String> {
        let tab_id = usize::try_from(tab_id)
            .map_err(|_| "Target tab ID does not fit this platform".to_string())?;
        let tabs = self
            .tabs
            .as_ref()
            .ok_or_else(|| "Target window has no initialized tab manager".to_string())?;
        let index = tabs
            .iter()
            .find_map(|(index, tab)| (tab.id == tab_id).then_some(index))
            .ok_or_else(|| format!("GUI tab {tab_id} does not exist in the target window"))?;
        let tab = tabs
            .iter()
            .nth(index)
            .map(|(_, tab)| tab)
            .expect("validated control tab index disappeared");
        if tab.closed || tab.closing_at.is_some() {
            return Err(format!("GUI tab {tab_id} is closing"));
        }
        let from = tabs.active_index();
        let changed = from != index;
        if changed {
            self.tabs
                .as_mut()
                .expect("validated tab manager disappeared")
                .switch_to(index);
            self.refresh_focused_terminal_surfaces();
            if dispatch_hook {
                self.dispatch_plugin_hook(crate::plugin::Hook::TabSwitched { from, to: index });
            }
            if let Some(window) = &self.window {
                window.request_redraw();
            }
        }
        Ok(changed)
    }

    /// Reorder one exact local tab without making it active. The tab manager
    /// validates the complete move before changing visible order.
    pub(super) fn move_control_tab_within_window(
        &mut self,
        tab_id: u64,
        destination_index: u16,
    ) -> std::result::Result<bool, String> {
        let tab_id = usize::try_from(tab_id)
            .map_err(|_| "Target tab ID does not fit this platform".to_string())?;
        let changed = self
            .tabs
            .as_mut()
            .ok_or_else(|| "Target window has no initialized tab manager".to_string())?
            .move_tab_by_id_to(tab_id, usize::from(destination_index))
            .map_err(|error| format!("Could not move tab {tab_id}: {error:#}"))?;
        if changed {
            self.tab_dragging = None;
            if let Some(window) = &self.window {
                window.request_redraw();
            }
        }
        Ok(changed)
    }

    pub(super) fn move_control_pane_within_window(
        &mut self,
        source_tab_id: u64,
        pane_id: u32,
        destination_tab_id: u64,
        target_pane_id: u32,
        direction: SplitDirection,
    ) -> std::result::Result<u32, String> {
        let source_tab_id = usize::try_from(source_tab_id)
            .map_err(|_| "Source tab ID does not fit this platform".to_string())?;
        let destination_tab_id = usize::try_from(destination_tab_id)
            .map_err(|_| "Destination tab ID does not fit this platform".to_string())?;
        let id = self
            .tabs
            .as_mut()
            .ok_or_else(|| "Target window has no initialized tab manager".to_string())?
            .transfer_pane_by_id(
                source_tab_id,
                PaneId(pane_id),
                destination_tab_id,
                PaneId(target_pane_id),
                direction,
                self.config.panes.cwd,
            )
            .map_err(|error| format!("Could not move pane {pane_id}: {error:#}"))?;
        self.broadcast = BroadcastState::Inactive;
        self.leave_copy_mode(false);
        self.terminal_mouse_capture = None;
        self.refresh_focused_terminal_surfaces();
        if let Some(window) = &self.window {
            window.request_redraw();
        }
        Ok(id.0)
    }

    pub(super) fn swap_plugin_panes(
        &mut self,
        tab_id: u64,
        first_pane_id: u32,
        second_pane_id: u32,
    ) -> std::result::Result<(Vec<u32>, u32, Option<u32>), String> {
        let active_before = self.tabs.as_ref().and_then(TabManager::active_tab_id);
        let state = {
            let manager = self.plugin_pane_layout_manager(tab_id)?;
            manager
                .swap_exact(PaneId(first_pane_id), PaneId(second_pane_id))
                .map_err(|error| format!("Could not swap panes: {error:#}"))?;
            plugin_pane_layout_state(manager)
        };
        self.finish_plugin_pane_layout(active_before);
        Ok(state)
    }

    pub(super) fn balance_plugin_panes(
        &mut self,
        tab_id: u64,
    ) -> std::result::Result<(bool, Vec<u32>, u32, Option<u32>), String> {
        let active_before = self.tabs.as_ref().and_then(TabManager::active_tab_id);
        let (changed, state) = {
            let manager = self.plugin_pane_layout_manager(tab_id)?;
            let changed = manager
                .balance_exact()
                .map_err(|error| format!("Could not balance panes: {error:#}"))?;
            (changed, plugin_pane_layout_state(manager))
        };
        self.finish_plugin_pane_layout(active_before);
        Ok((changed, state.0, state.1, state.2))
    }

    pub(super) fn apply_plugin_pane_layout(
        &mut self,
        tab_id: u64,
        preset: crate::panes::PaneLayoutPreset,
    ) -> std::result::Result<(Vec<u32>, u32, Option<u32>), String> {
        let active_before = self.tabs.as_ref().and_then(TabManager::active_tab_id);
        let state = {
            let manager = self.plugin_pane_layout_manager(tab_id)?;
            manager
                .apply_layout_preset_exact(preset)
                .map_err(|error| format!("Could not apply pane layout: {error:#}"))?;
            plugin_pane_layout_state(manager)
        };
        self.finish_plugin_pane_layout(active_before);
        Ok(state)
    }

    fn plugin_pane_layout_manager(
        &mut self,
        tab_id: u64,
    ) -> std::result::Result<&mut PaneManager, String> {
        let tab_id = usize::try_from(tab_id)
            .map_err(|_| "Target tab ID does not fit this platform".to_string())?;
        let tab = self
            .tabs
            .as_mut()
            .ok_or_else(|| "Target window has no initialized tab manager".to_string())?
            .tab_mut_by_id(tab_id)
            .ok_or_else(|| format!("GUI tab {tab_id} does not exist in the target window"))?;
        if tab.closed || tab.closing_at.is_some() {
            return Err(format!("GUI tab {tab_id} is closing"));
        }
        if tab.live_domain_tab_route().is_some() {
            return Err(format!(
                "GUI tab {tab_id} belongs to daemon-authoritative live-domain topology"
            ));
        }
        if matches!(tab.tab_type, crate::tabs::TabType::Serial { .. }) {
            return Err(format!("GUI tab {tab_id} is a serial terminal"));
        }
        tab.pane_manager
            .as_mut()
            .ok_or_else(|| format!("GUI tab {tab_id} is not split"))
    }

    fn finish_plugin_pane_layout(&mut self, active_before: Option<usize>) {
        debug_assert_eq!(
            self.tabs.as_ref().and_then(TabManager::active_tab_id),
            active_before,
            "plugin pane layout must not change the active tab"
        );
        self.divider_dragging = None;
        self.refresh_focused_terminal_surfaces();
        if let Some(window) = &self.window {
            window.request_redraw();
        }
    }

    pub(super) fn resize_control_divider(
        &mut self,
        tab_id: u64,
        divider_id: u64,
        ratio_millis: u16,
    ) -> std::result::Result<(u16, bool), String> {
        let tab_id = usize::try_from(tab_id)
            .map_err(|_| "Target tab ID does not fit this platform".to_string())?;
        let tabs = self
            .tabs
            .as_mut()
            .ok_or_else(|| "Target window has no initialized tab manager".to_string())?;
        let tab = tabs
            .tab_mut_by_id(tab_id)
            .ok_or_else(|| format!("GUI tab {tab_id} does not exist in the target window"))?;
        if tab.closed || tab.closing_at.is_some() {
            return Err(format!("GUI tab {tab_id} is closing"));
        }
        if tab.live_domain_tab_route().is_some() {
            return Err(
                "Control-requested divider resize is not yet supported for live-domain tabs".into(),
            );
        }
        let manager = tab
            .pane_manager
            .as_mut()
            .ok_or_else(|| format!("GUI tab {tab_id} has no split dividers"))?;
        let result = manager
            .resize_local_divider(divider_id, ratio_millis)
            .map_err(|error| format!("Could not resize divider {divider_id}: {error:#}"))?;
        if result.1 {
            self.divider_dragging = None;
            if let Some(window) = &self.window {
                window.request_redraw();
            }
        }
        Ok(result)
    }

    pub(super) fn read_control_text(
        &self,
        tab_id: u64,
        pane_id: u32,
        history_lines: u16,
        max_bytes: u32,
    ) -> std::result::Result<(crate::control::protocol::TextScreen, u16, String, bool), String>
    {
        let tab_id = usize::try_from(tab_id)
            .map_err(|_| "Target tab ID does not fit this platform".to_string())?;
        let tabs = self
            .tabs
            .as_ref()
            .ok_or_else(|| "Target window has no initialized tab manager".to_string())?;
        let tab = tabs
            .tab_by_id(tab_id)
            .ok_or_else(|| format!("GUI tab {tab_id} does not exist in the target window"))?;
        if tab.closed || tab.closing_at.is_some() {
            return Err(format!("GUI tab {tab_id} is closing"));
        }
        let terminal = if let Some(manager) = &tab.pane_manager {
            manager
                .get_pane(PaneId(pane_id))
                .map(|pane| &pane.terminal)
                .ok_or_else(|| format!("GUI pane {pane_id} does not exist in tab {tab_id}"))?
        } else {
            if PaneId(pane_id) != tab.single_pane_id() {
                return Err(format!(
                    "GUI pane {pane_id} does not exist in tab {tab_id}; its sole pane ID is {}",
                    tab.single_pane_id().0
                ));
            }
            tab.focused_terminal()
                .ok_or_else(|| format!("GUI pane {pane_id} has no live terminal"))?
        };
        let alternate = terminal.presentation_alternate_screen_active();
        let (text, included_history, truncated) = terminal.bounded_presentation_text(
            usize::from(history_lines),
            usize::try_from(max_bytes).expect("u32 byte limit fits usize"),
        );
        let included_history =
            u16::try_from(included_history).expect("bounded terminal-text history fits u16");
        Ok((
            if alternate {
                crate::control::protocol::TextScreen::Alternate
            } else {
                crate::control::protocol::TextScreen::Primary
            },
            included_history,
            text,
            truncated,
        ))
    }

    pub(super) fn send_control_text(
        &mut self,
        tab_id: u64,
        pane_id: u32,
        text: &str,
        paste: bool,
        enter: bool,
    ) -> std::result::Result<
        (
            u32,
            u32,
            crate::control::protocol::InputDisposition,
            Option<u64>,
        ),
        String,
    > {
        self.send_external_text(tab_id, pane_id, text, paste, enter, false, None)
    }

    /// Send bounded plugin text to an exact terminal in this window. Plugin
    /// input always passes destructive-command guardrails, which name the
    /// window because a plugin may target a window other than the focused one.
    pub(super) fn send_plugin_text(
        &mut self,
        tab_id: u64,
        pane_id: u32,
        text: &str,
        paste: bool,
        enter: bool,
    ) -> std::result::Result<
        (
            u32,
            u32,
            crate::control::protocol::InputDisposition,
            Option<u64>,
        ),
        String,
    > {
        self.send_external_text(
            tab_id,
            pane_id,
            text,
            paste,
            enter,
            true,
            Some(self.control_window_id),
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn send_external_text(
        &mut self,
        tab_id: u64,
        pane_id: u32,
        text: &str,
        paste: bool,
        enter: bool,
        always_guard_destructive: bool,
        target_window_id: Option<u64>,
    ) -> std::result::Result<
        (
            u32,
            u32,
            crate::control::protocol::InputDisposition,
            Option<u64>,
        ),
        String,
    > {
        if !self.config.security.allow_gui_control_input {
            return Err(
                "External terminal input is disabled; set security.allow_gui_control_input = true"
                    .into(),
            );
        }
        if self.guardrail.is_awaiting_current() {
            return Err("A destructive-command confirmation is already pending".into());
        }
        let tab_id = usize::try_from(tab_id)
            .map_err(|_| "Target tab ID does not fit this platform".to_string())?;
        let (origin, mode, guarded, read_only) = {
            let tabs = self
                .tabs
                .as_ref()
                .ok_or_else(|| "Target window has no initialized tab manager".to_string())?;
            let tab = tabs
                .tab_by_id(tab_id)
                .ok_or_else(|| format!("GUI tab {tab_id} does not exist in the target window"))?;
            if tab.closed || tab.closing_at.is_some() {
                return Err(format!("GUI tab {tab_id} is closing"));
            }
            let origin = if let Some(manager) = &tab.pane_manager {
                let pane = manager
                    .get_pane(PaneId(pane_id))
                    .ok_or_else(|| format!("GUI pane {pane_id} does not exist in tab {tab_id}"))?;
                if pane.closed {
                    return Err(format!("GUI pane {pane_id} is closed in tab {tab_id}"));
                }
                (
                    crate::tabs::TerminalEventOrigin::Pane {
                        tab_id,
                        pane_id: PaneId(pane_id),
                    },
                    pane.terminal.mode(),
                    pane.read_only,
                )
            } else {
                if PaneId(pane_id) != tab.single_pane_id() {
                    return Err(format!(
                        "GUI pane {pane_id} does not exist in tab {tab_id}; its sole pane ID is {}",
                        tab.single_pane_id().0
                    ));
                }
                (
                    crate::tabs::TerminalEventOrigin::Tab { tab_id },
                    tab.focused_terminal()
                        .ok_or_else(|| format!("GUI pane {pane_id} has no live terminal"))?
                        .mode(),
                    tab.focused_pane_read_only(),
                )
            };
            (
                origin.0,
                origin.1,
                always_guard_destructive
                    || matches!(tab.tab_type, crate::tabs::TabType::Plugin { .. }),
                origin.2,
            )
        };

        if read_only {
            return Err(format!("GUI tab {tab_id}, pane {pane_id} is read-only"));
        }

        let input_bytes = text.len().saturating_add(usize::from(enter));
        let mut wire = if paste {
            crate::input::encode_paste(mode, text)
        } else {
            text.as_bytes().to_vec()
        };
        if enter {
            wire.push(b'\r');
        }
        let input_bytes = u32::try_from(input_bytes).expect("bounded control input fits u32");
        let wire_bytes = u32::try_from(wire.len()).expect("bounded control wire input fits u32");

        if guarded && let Some(mut matched) = crate::terminal::guardrails::check_command(text) {
            matched.target = target_window_id.map_or_else(
                || format!("GUI tab {tab_id}, pane {pane_id}"),
                |window_id| format!("GUI window {window_id}, tab {tab_id}, pane {pane_id}"),
            );
            let confirmation_id = self.guardrail.show(
                matched,
                vec![crate::terminal::guardrails::GuardedWrite::for_pane(
                    tab_id,
                    PaneId(pane_id),
                    wire,
                )],
            );
            if let Some(window) = &self.window {
                window.request_redraw();
            }
            return Ok((
                input_bytes,
                wire_bytes,
                crate::control::protocol::InputDisposition::PendingConfirmation,
                Some(confirmation_id),
            ));
        }

        self.tabs
            .as_mut()
            .ok_or_else(|| "Target window has no initialized tab manager".to_string())?
            .write_user_to_origin(origin, &wire)
            .map_err(|error| format!("Could not write GUI control input: {error}"))?;
        Ok((
            input_bytes,
            wire_bytes,
            crate::control::protocol::InputDisposition::Sent,
            None,
        ))
    }

    /// Select an exact tab and focus one exact stable pane within it. The
    /// complete path is validated before either focus component changes.
    pub(super) fn focus_control_pane(
        &mut self,
        tab_id: u64,
        pane_id: u32,
    ) -> std::result::Result<(bool, bool), String> {
        self.focus_pane_by_stable_id(tab_id, pane_id, true)
    }

    /// Focus a stable pane for a declarative plugin without recursively
    /// emitting a tab-switched subscription when the containing tab changes.
    pub(super) fn focus_plugin_pane(
        &mut self,
        tab_id: u64,
        pane_id: u32,
    ) -> std::result::Result<(bool, bool), String> {
        self.focus_pane_by_stable_id(tab_id, pane_id, false)
    }

    fn focus_pane_by_stable_id(
        &mut self,
        tab_id: u64,
        pane_id: u32,
        dispatch_hook: bool,
    ) -> std::result::Result<(bool, bool), String> {
        let tab_id = usize::try_from(tab_id)
            .map_err(|_| "Target tab ID does not fit this platform".to_string())?;
        let tabs = self
            .tabs
            .as_ref()
            .ok_or_else(|| "Target window has no initialized tab manager".to_string())?;
        let index = tabs
            .iter()
            .find_map(|(index, tab)| (tab.id == tab_id).then_some(index))
            .ok_or_else(|| format!("GUI tab {tab_id} does not exist in the target window"))?;
        let tab = tabs
            .iter()
            .nth(index)
            .map(|(_, tab)| tab)
            .expect("validated control tab index disappeared");
        if tab.closed || tab.closing_at.is_some() {
            return Err(format!("GUI tab {tab_id} is closing"));
        }
        let target = PaneId(pane_id);
        let pane_changed = if let Some(manager) = &tab.pane_manager {
            let pane = manager
                .get_pane(target)
                .ok_or_else(|| format!("GUI pane {pane_id} does not exist in tab {tab_id}"))?;
            if pane.closed {
                return Err(format!("GUI pane {pane_id} in tab {tab_id} is closed"));
            }
            manager.focused_id() != target
        } else {
            let sole_pane = tab.single_pane_id();
            if target != sole_pane {
                return Err(format!(
                    "GUI pane {pane_id} does not exist in unsplit tab {tab_id}; its sole pane ID is {}",
                    sole_pane.0
                ));
            }
            false
        };
        let from = tabs.active_index();
        let tab_changed = from != index;

        let live_route = {
            let tabs = self
                .tabs
                .as_mut()
                .expect("validated tab manager disappeared");
            if tab_changed {
                tabs.switch_to(index);
            }
            let tab = tabs
                .tab_mut_by_id(tab_id)
                .expect("validated control tab disappeared");
            if pane_changed {
                tab.pane_manager
                    .as_mut()
                    .expect("validated pane manager disappeared")
                    .focus(target);
            }
            tab.live_domain_focus_route()
                .map(|(domain, pane)| (domain.to_string(), pane))
        };
        if let (Some(authority), Some((domain, pane))) =
            (self.live_domain_authority.clone(), live_route)
        {
            crate::domain::request_focus_background(authority, domain, pane);
        }
        if tab_changed || pane_changed {
            self.refresh_focused_terminal_surfaces();
            if tab_changed && dispatch_hook {
                self.dispatch_plugin_hook(crate::plugin::Hook::TabSwitched { from, to: index });
            }
            if let Some(window) = &self.window {
                window.request_redraw();
            }
        }
        Ok((tab_changed, pane_changed))
    }

    pub(super) fn close_control_tab(&mut self, tab_id: u64) -> std::result::Result<(), String> {
        self.close_ordinary_tab(tab_id, "Control-requested", true, true)
            .map(|_| ())
    }

    pub(super) fn close_plugin_tab(
        &mut self,
        tab_id: u64,
    ) -> std::result::Result<(u16, u64, bool), String> {
        let (remaining, active, active_changed) =
            self.close_ordinary_tab(tab_id, "Plugin-requested", false, false)?;
        Ok((
            remaining,
            active.expect("last-tab protection leaves one active ordinary or live-domain tab"),
            active_changed,
        ))
    }

    fn close_ordinary_tab(
        &mut self,
        tab_id: u64,
        request_origin: &str,
        dispatch_plugin_hook: bool,
        allow_last_tab: bool,
    ) -> std::result::Result<(u16, Option<u64>, bool), String> {
        let tab_id = usize::try_from(tab_id)
            .map_err(|_| "Target tab ID does not fit this platform".to_string())?;
        let tabs = self
            .tabs
            .as_ref()
            .ok_or_else(|| "Target window has no initialized tab manager".to_string())?;
        let (index, tab) = tabs
            .iter()
            .find(|(_, tab)| tab.id == tab_id)
            .ok_or_else(|| format!("GUI tab {tab_id} does not exist in the target window"))?;
        if tab.closed || tab.closing_at.is_some() {
            return Err(format!("GUI tab {tab_id} is already closing"));
        }
        if tab.live_domain_tab_route().is_some() {
            return Err(format!(
                "{request_origin} tab closure is not yet supported for live-domain tabs"
            ));
        }
        let non_closing_tabs = tabs
            .iter()
            .filter(|(_, tab)| !tab.closed && tab.closing_at.is_none())
            .count();
        if !allow_last_tab && non_closing_tabs <= 1 {
            return Err(format!(
                "Cannot close the final non-closing tab {tab_id}; window closure requires separate authority"
            ));
        }
        let previous_active = tabs.active_tab_id();
        let close_copy_mode = self
            .copy_mode_target
            .is_some_and(|target| target.tab == tab_id);
        if !self
            .tabs
            .as_mut()
            .expect("validated tab manager disappeared")
            .close_tab(index)
        {
            return Err(format!("GUI tab {tab_id} could not begin closing"));
        }
        if close_copy_mode {
            self.leave_copy_mode(false);
        }
        if dispatch_plugin_hook {
            self.dispatch_plugin_hook(crate::plugin::Hook::TabClosed { tab_id });
        }
        self.refresh_focused_terminal_surfaces();
        if let Some(window) = &self.window {
            window.request_redraw();
        }
        let active_local = self
            .tabs
            .as_ref()
            .and_then(TabManager::active_tab)
            .filter(|tab| !tab.closed && tab.closing_at.is_none())
            .map(|tab| tab.id);
        let active = active_local.map(|id| u64::try_from(id).expect("tab ID fits u64"));
        Ok((
            u16::try_from(non_closing_tabs - 1).expect("tab count is bounded below u16::MAX"),
            active,
            previous_active != active_local,
        ))
    }

    pub(super) fn close_control_pane(
        &mut self,
        tab_id: u64,
        pane_id: u32,
    ) -> std::result::Result<(u16, u32), String> {
        self.close_ordinary_pane(tab_id, pane_id, "Control-requested")
    }

    pub(super) fn close_plugin_pane(
        &mut self,
        tab_id: u64,
        pane_id: u32,
    ) -> std::result::Result<(u16, u32), String> {
        self.close_ordinary_pane(tab_id, pane_id, "Plugin-requested")
    }

    fn close_ordinary_pane(
        &mut self,
        tab_id: u64,
        pane_id: u32,
        request_origin: &str,
    ) -> std::result::Result<(u16, u32), String> {
        let tab_id = usize::try_from(tab_id)
            .map_err(|_| "Target tab ID does not fit this platform".to_string())?;
        let tabs = self
            .tabs
            .as_ref()
            .ok_or_else(|| "Target window has no initialized tab manager".to_string())?;
        let tab = tabs
            .iter()
            .find_map(|(_, tab)| (tab.id == tab_id).then_some(tab))
            .ok_or_else(|| format!("GUI tab {tab_id} does not exist in the target window"))?;
        if tab.closed || tab.closing_at.is_some() {
            return Err(format!("GUI tab {tab_id} is closing"));
        }
        if tab.live_domain_tab_route().is_some() {
            return Err(format!(
                "{request_origin} pane closure is not yet supported for live-domain tabs"
            ));
        }
        if !tab.allows_pane_splits() {
            return Err("Serial terminal tabs do not contain closable panes".into());
        }
        let target = PaneId(pane_id);
        let manager = tab.pane_manager.as_ref().ok_or_else(|| {
            format!("Cannot close the sole pane {pane_id} in unsplit tab {tab_id}; close the tab instead")
        })?;
        if manager.pane_count() <= 1 {
            return Err(format!(
                "Cannot close the last pane {pane_id} in tab {tab_id}; close the tab instead"
            ));
        }
        let pane = manager
            .get_pane(target)
            .ok_or_else(|| format!("GUI pane {pane_id} does not exist in tab {tab_id}"))?;
        if pane.closed {
            return Err(format!(
                "GUI pane {pane_id} in tab {tab_id} is already closed"
            ));
        }
        let close_copy_mode = self.copy_mode_target
            == Some(CopyModeTarget {
                tab: tab_id,
                pane: Some(target),
            });

        let (remaining, focused, dissolved) = {
            let tab = self
                .tabs
                .as_mut()
                .expect("validated tab manager disappeared")
                .tab_mut_by_id(tab_id)
                .expect("validated control tab disappeared");
            let manager = tab
                .pane_manager
                .as_mut()
                .expect("validated pane manager disappeared");
            if !manager.close(target) {
                return Err(format!("GUI pane {pane_id} could not be closed"));
            }
            let remaining = manager.pane_count();
            let focused = manager.focused_id().0;
            tab.reconcile_primary_pane_launch();
            let dissolved = remaining == 1;
            if dissolved {
                let mut manager = tab.pane_manager.take().unwrap();
                let owner = manager
                    .take_remaining_pane()
                    .ok_or_else(|| "Remaining pane disappeared during collapse".to_string())?;
                tab.restore_terminal_owner(owner);
            }
            (
                remaining,
                if dissolved {
                    tab.single_pane_id().0
                } else {
                    focused
                },
                dissolved,
            )
        };
        if dissolved && let Some(size) = self.window.as_ref().map(|window| window.inner_size()) {
            self.sync_terminal_geometry(size.width, size.height);
        }
        if close_copy_mode {
            self.leave_copy_mode(false);
        }
        if !matches!(self.broadcast, BroadcastState::Inactive) {
            self.broadcast = BroadcastState::Inactive;
            self.toasts.info("Broadcast cancelled after pane closure");
        }
        self.refresh_focused_terminal_surfaces();
        if let Some(window) = &self.window {
            window.request_redraw();
        }
        Ok((
            u16::try_from(remaining).expect("pane count is bounded below u16::MAX"),
            focused,
        ))
    }

    /// Close the focused pane, collapsing back to a single-terminal tab if
    /// only one pane remains.
    pub(super) fn close_focused_pane(&mut self) {
        let live_route = self
            .tabs
            .as_ref()
            .and_then(TabManager::active_tab)
            .and_then(|tab| {
                tab.live_domain_focus_route().map(|(domain, remote_pane)| {
                    (
                        tab.id,
                        domain.to_string(),
                        remote_pane,
                        tab.pane_manager.as_ref().map_or(1, PaneManager::pane_count),
                    )
                })
            });
        if let Some((local_tab_id, domain_id, remote_pane_id, pane_count)) = live_route {
            if pane_count <= 1 {
                self.toasts
                    .warning("Cannot close the last pane; close the domain tab instead");
                return;
            }
            self.queue_live_domain_control(LiveDomainControl::ClosePane {
                domain_id,
                remote_pane_id,
                local_tab_id,
            });
            return;
        }
        let mut changed = false;
        let mut dissolved = false;
        if let Some(tabs) = &mut self.tabs {
            if let Some(tab) = tabs.active_tab_mut() {
                if let Some(ref mut pm) = tab.pane_manager {
                    changed = pm.close_focused();
                }
                if changed {
                    tab.reconcile_primary_pane_launch();
                }
                let dissolve = tab
                    .pane_manager
                    .as_ref()
                    .is_some_and(|panes| panes.pane_count() <= 1);
                if dissolve {
                    let mut panes = tab.pane_manager.take().unwrap();
                    if let Some(remaining) = panes.take_remaining_pane() {
                        tab.restore_terminal_owner(remaining);
                    }
                    dissolved = true;
                }
            }
        }
        if dissolved {
            if let Some(size) = self.window.as_ref().map(|window| window.inner_size()) {
                self.sync_terminal_geometry(size.width, size.height);
            }
        }
        if changed {
            if !matches!(self.broadcast, BroadcastState::Inactive) {
                self.broadcast = BroadcastState::Inactive;
                self.toasts.info("Broadcast cancelled after pane closure");
            }
            self.refresh_focused_terminal_surfaces();
        } else if dissolved {
            self.toasts
                .info("The last pane was retained as the tab's terminal");
        } else {
            self.toasts
                .warning("Cannot close the last pane; close the tab instead");
        }
        if let Some(window) = &self.window {
            window.request_redraw();
        }
    }
}

fn plugin_pane_layout_state(manager: &PaneManager) -> (Vec<u32>, u32, Option<u32>) {
    (
        manager.pane_ids().into_iter().map(|id| id.0).collect(),
        manager.focused_id().0,
        manager.zoomed_id().map(|id| id.0),
    )
}

#[cfg(test)]
mod workspace_control_tests {
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
                    context.contains("Output snapshot truncated"),
                    output_truncated
                );
            }
        }
    }

    #[test]
    fn tab_rename_text_is_single_line_control_free_and_strictly_bounded() {
        assert_eq!(bounded_tab_rename_text("alpha\nbeta", 20), "alphabeta");
        assert_eq!(bounded_tab_rename_text("界🙂abc", 3), "界🙂a");

        let mut title = "x".repeat(3);
        assert!(!append_tab_rename_text(&mut title, " \nmore", 5));
        assert_eq!(title, "xxx");
        assert!(append_tab_rename_text(&mut title, "界🙂", 5));
        assert_eq!(title, "xxx界🙂");
        assert!(!append_tab_rename_text(&mut title, "ignored", 5));
        assert_eq!(title, "xxx界🙂");
        assert_eq!(bounded_tab_rename_text("a\u{2028}b\u{2029}c", 20), "abc");
    }

    #[test]
    fn tab_rename_keyboard_and_ime_are_atomic_at_local_and_domain_limits() {
        use winit::{
            event::Ime,
            keyboard::{Key, NamedKey},
        };
        for max_chars in [128, 256] {
            let mut app = App::new_headless(Config::default()).unwrap();
            let original = "界".repeat(max_chars - 1);
            app.tab_rename = Some(TabRenameState {
                tab_id: 0,
                original: None,
                input: original.clone(),
                max_chars,
            });
            for value in [
                "",
                "xy",
                "a\nb",
                "\r",
                "\t",
                "\0",
                "\x1b",
                "\u{85}",
                "\u{2028}",
                "\u{2029}",
                &"x".repeat(max_chars + 1),
            ] {
                app.handle_ime_event(Ime::Preedit("keep".into(), None));
                assert!(app.handle_tab_rename_key(&Key::Character(value.into())));
                assert_eq!(app.tab_rename.as_ref().unwrap().input, original);
                assert!(app.ime.preedit().is_some());
                app.handle_ime_event(Ime::Commit(value.into()));
                assert_eq!(app.tab_rename.as_ref().unwrap().input, original);
                assert!(app.ime.preedit().is_none());
            }
            app.handle_ime_event(Ime::Commit("🫚".into()));
            assert_eq!(
                app.tab_rename.as_ref().unwrap().input,
                format!("{original}🫚")
            );
            app.handle_ime_event(Ime::Preedit("keep".into(), None));
            assert!(app.handle_tab_rename_key(&Key::Named(NamedKey::Space)));
            assert_eq!(
                app.tab_rename.as_ref().unwrap().input.chars().count(),
                max_chars
            );
            assert!(app.ime.preedit().is_some());
            app.tab_rename.as_mut().unwrap().input = "e\u{301}👩\u{200d}💻🇺🇸".into();
            for expected in ["e\u{301}👩\u{200d}💻", "e\u{301}", "", ""] {
                assert!(app.handle_tab_rename_key(&Key::Named(NamedKey::Backspace)));
                assert_eq!(app.tab_rename.as_ref().unwrap().input, expected);
                assert!(app.ime.preedit().is_none());
            }
            assert!(app.handle_tab_rename_key(&Key::Character("e\u{301}👩\u{200d}💻".into())));
            assert_eq!(
                app.tab_rename.as_ref().unwrap().input,
                "e\u{301}👩\u{200d}💻"
            );
            assert!(app.pending_live_domain_controls.is_empty());
        }
    }

    #[test]
    fn tab_rename_modified_keys_cannot_delete_commit_or_cancel() {
        use winit::{
            event::Ime,
            keyboard::{Key, NamedKey},
        };
        let mut app = App::new_headless(Config::default()).unwrap();
        assert!(!app.handle_tab_rename_key(&Key::Named(NamedKey::Backspace)));
        app.tab_rename = Some(TabRenameState {
            tab_id: 0,
            original: None,
            input: "Roots".into(),
            max_chars: 256,
        });
        app.handle_ime_event(Ime::Preedit("keep".into(), None));
        for modifiers in [
            ModifiersState::CONTROL,
            ModifiersState::ALT,
            ModifiersState::SUPER,
            ModifiersState::SHIFT,
            ModifiersState::CONTROL | ModifiersState::ALT,
        ] {
            app.modifiers = modifiers;
            for key in [
                NamedKey::Backspace,
                NamedKey::Enter,
                NamedKey::Escape,
                NamedKey::ArrowLeft,
                NamedKey::Delete,
            ] {
                assert!(app.handle_tab_rename_key(&Key::Named(key)));
                assert_eq!(app.tab_rename.as_ref().unwrap().input, "Roots");
                assert!(app.ime.preedit().is_some());
            }
            if modifiers != ModifiersState::SHIFT {
                assert!(app.handle_tab_rename_key(&Key::Character("x".into())));
                assert!(app.handle_tab_rename_key(&Key::Named(NamedKey::Space)));
                assert_eq!(app.tab_rename.as_ref().unwrap().input, "Roots");
                assert!(app.ime.preedit().is_some());
            }
        }
        app.alt_graph_active = true;
        assert!(app.handle_tab_rename_key(&Key::Named(NamedKey::Backspace)));
        assert_eq!(app.tab_rename.as_ref().unwrap().input, "Roots");
        assert!(app.handle_tab_rename_key(&Key::Character("@".into())));
        assert_eq!(app.tab_rename.as_ref().unwrap().input, "Roots@");
        assert!(app.ime.preedit().is_none());
        app.alt_graph_active = false;
        app.modifiers = ModifiersState::SHIFT;
        assert!(app.handle_tab_rename_key(&Key::Character("A".into())));
        assert_eq!(app.tab_rename.as_ref().unwrap().input, "Roots@A");
        app.modifiers = ModifiersState::empty();
        assert!(app.handle_tab_rename_key(&Key::Named(NamedKey::Escape)));
        assert!(app.tab_rename.is_none());
    }

    #[test]
    fn live_domain_disconnect_feedback_names_the_ssh_route() {
        let mut app = App::new_headless(Config::default()).unwrap();
        app.live_domain_authority = Some(crate::domain::DomainAuthority::Ssh {
            destination: "builder@example.test".into(),
        });

        app.handle_live_domain_topology("domain-ssh".into(), Err("connection reset".into()));

        let toast = app.toasts.iter().last().unwrap();
        assert_eq!(toast.severity, crate::ui::toast::ToastSeverity::Warning);
        assert_eq!(
            toast.message,
            "Live domain domain-ssh via SSH to builder@example.test disconnected; retrying: connection reset"
        );
    }

    #[test]
    fn failed_topology_reconciliation_does_not_declare_recovery() {
        let mut app = App::new_headless(Config::default()).unwrap();
        app.live_domain_authority = Some(crate::domain::DomainAuthority::Local);
        app.live_domain_relay_command = Some(crate::domain::DomainRelayCommand::local("ignored"));
        app.handle_live_domain_topology("domain-reconcile".into(), Err("socket closed".into()));

        app.handle_live_domain_topology(
            "domain-reconcile".into(),
            Ok(crate::domain::DomainTopologyEvent::Snapshot {
                revision: 1,
                domain: crate::domain::model::DomainTopology {
                    domain_id: "domain-reconcile".into(),
                    active_tab: 0,
                    tabs: Vec::new(),
                },
            }),
        );

        assert!(app.disconnected_live_domains.contains("domain-reconcile"));
        assert!(app.toasts.iter().any(|toast| {
            toast.severity == crate::ui::toast::ToastSeverity::Error
                && toast.message.contains("tab manager is unavailable")
        }));
        assert!(
            app.toasts
                .iter()
                .all(|toast| toast.severity != crate::ui::toast::ToastSeverity::Success)
        );
    }

    #[test]
    fn failed_broadcast_targets_are_pruned_and_the_two_target_floor_is_enforced() {
        let mut app = App::new_headless(Config::default()).unwrap();
        app.broadcast = BroadcastState::Active {
            tab_id: 7,
            targets: vec![PaneId(1), PaneId(2), PaneId(3)],
        };
        app.finish_broadcast_writes(7, vec![PaneId(1), PaneId(3)], 1);
        assert!(matches!(
            &app.broadcast,
            BroadcastState::Active { tab_id: 7, targets }
                if targets == &[PaneId(1), PaneId(3)]
        ));
        app.finish_broadcast_writes(7, vec![PaneId(3)], 1);
        assert!(matches!(app.broadcast, BroadcastState::Inactive));
    }

    #[cfg(unix)]
    #[test]
    fn domain_control_completion_is_stable_id_routed_idempotent_and_error_safe() {
        if !std::path::Path::new("/bin/true").exists() {
            return;
        }
        use crate::domain::model::{
            DomainSnapshot, LivePane, LivePaneLayout, LiveTab, LiveWorkspace,
        };
        let remote_tab = |tab_id, pane_id| LiveTab {
            id: tab_id,
            title: format!("remote-tab-{tab_id}"),
            user_title: None,
            pinned: false,
            focused_pane: pane_id,
            layout: LivePaneLayout::Pane { pane: pane_id },
            panes: vec![LivePane {
                id: pane_id,
                title: format!("remote-pane-{pane_id}"),
                working_directory: None,
                command: vec!["shell".into()],
                cols: 80,
                rows: 24,
                closed: false,
                attachments: 0,
            }],
        };
        let snapshot = DomainSnapshot {
            id: "domain-control".into(),
            name: "control".into(),
            created_at: "2026-01-01T00:00:00Z".into(),
            workspace: LiveWorkspace {
                id: "live-domain-control".into(),
                name: "control".into(),
                active_tab: 20,
                tabs: vec![remote_tab(10, 100), remote_tab(20, 200)],
            },
        };
        let mut tabs = TabManager::new(
            80,
            24,
            crate::config::ShellConfig::default(),
            1_000,
            1024 * 1024,
        );
        tabs.attach_domain(
            &snapshot,
            &crate::domain::DomainRelayCommand::local("/bin/true"),
        )
        .unwrap();
        let local_tab_id = tabs
            .iter()
            .find_map(|(_, tab)| {
                (tab.live_domain_tab_route() == Some(("domain-control", 10))).then_some(tab.id)
            })
            .unwrap();
        let mut app = App::new_headless(crate::config::Config::default()).unwrap();
        app.tabs = Some(tabs);

        let close = LiveDomainControl::CloseTab {
            domain_id: "domain-control".into(),
            remote_tab_id: 10,
            local_tab_id,
        };
        // A control can become pending after a drag starts. Interruption must
        // restore its rejected preview without canceling the earlier request.
        app.live_domain_relay_command = Some(crate::domain::DomainRelayCommand::local("/bin/true"));
        app.live_domain_topologies
            .insert(snapshot.id.clone(), (&snapshot).into());
        let dragged_id = app.tabs.as_ref().unwrap().active_tab().unwrap().id;
        app.pending_live_domain_controls.insert(close.clone());
        let interruptions: [fn(&mut App); 4] = [
            |app| app.handle_window_focus_change(false),
            |app| app.handle_resize(960, 720),
            |app| app.handle_scale_factor_change(1.5, winit::dpi::PhysicalSize::new(1440, 1080)),
            |app| app.sync_terminal_geometry(960, 720),
        ];
        for interrupt in interruptions {
            assert!(app.tabs.as_mut().unwrap().move_active_to(0));
            app.tab_dragging = Some(TabDrag {
                tab_id: dragged_id,
                grab: None,
                live: Some(LiveDomainTabDrag {
                    domain_id: snapshot.id.clone(),
                    remote_tab_id: 20,
                    initial_index: 1,
                }),
            });
            interrupt(&mut app);
            assert!(app.tab_dragging.is_none());
            assert_eq!(app.pending_live_domain_controls.len(), 1);
            assert!(app.pending_live_domain_controls.contains(&close));
            assert_eq!(
                app.tabs
                    .as_ref()
                    .unwrap()
                    .iter()
                    .map(|(_, tab)| tab.live_domain_tab_route().unwrap().1)
                    .collect::<Vec<_>>(),
                vec![10, 20]
            );
        }
        assert!(
            app.toasts
                .iter()
                .any(|toast| toast.message.contains("already pending"))
        );
        app.pending_live_domain_controls.remove(&close);
        assert_eq!(
            close.request(),
            crate::domain::protocol::Request::CloseTab {
                domain: "domain-control".into(),
                tab: 10,
            }
        );
        let spawn = crate::domain::model::SpawnSpec::default();
        assert_eq!(
            LiveDomainControl::SpawnTab {
                domain_id: "domain-control".into(),
                spawn: spawn.clone(),
            }
            .request(),
            crate::domain::protocol::Request::SpawnTab {
                domain: "domain-control".into(),
                spawn: spawn.clone(),
            }
        );
        assert_eq!(
            LiveDomainControl::SplitPane {
                domain_id: "domain-control".into(),
                remote_pane_id: 200,
                direction: crate::domain::model::SplitDirection::Vertical,
                spawn: spawn.clone(),
            }
            .request(),
            crate::domain::protocol::Request::SplitPane {
                domain: "domain-control".into(),
                pane: 200,
                direction: crate::domain::model::SplitDirection::Vertical,
                spawn,
            }
        );
        assert_eq!(
            LiveDomainControl::ResizeSplit {
                domain_id: "domain-control".into(),
                remote_split_id: 44,
                ratio_millis: 375,
            }
            .request(),
            crate::domain::protocol::Request::ResizeSplit {
                domain: "domain-control".into(),
                split: 44,
                ratio_millis: 375,
            }
        );
        assert_eq!(
            LiveDomainControl::RenameTab {
                domain_id: "domain-control".into(),
                remote_tab_id: 10,
                title: Some("Build λ".into()),
            }
            .request(),
            crate::domain::protocol::Request::RenameTab {
                domain: "domain-control".into(),
                tab: 10,
                title: Some("Build λ".into()),
            }
        );
        assert_eq!(
            LiveDomainControl::SetTabPinned {
                domain_id: "domain-control".into(),
                remote_tab_id: 10,
                pinned: true,
            }
            .request(),
            crate::domain::protocol::Request::SetTabPinned {
                domain: "domain-control".into(),
                tab: 10,
                pinned: true,
            }
        );
        assert_eq!(
            LiveDomainControl::MoveTab {
                domain_id: "domain-control".into(),
                remote_tab_id: 10,
                index: 1,
            }
            .request(),
            crate::domain::protocol::Request::MoveTab {
                domain: "domain-control".into(),
                tab: 10,
                index: 1,
            }
        );
        assert_eq!(
            LiveDomainControl::SwapPaneNext {
                domain_id: "domain-control".into(),
                remote_pane_id: 100,
            }
            .request(),
            crate::domain::protocol::Request::SwapPaneNext {
                domain: "domain-control".into(),
                pane: 100,
            }
        );
        assert_eq!(
            LiveDomainControl::BalanceTab {
                domain_id: "domain-control".into(),
                remote_tab_id: 10,
            }
            .request(),
            crate::domain::protocol::Request::BalanceTab {
                domain: "domain-control".into(),
                tab: 10,
            }
        );
        assert_eq!(
            LiveDomainControl::ApplyPaneLayout {
                domain_id: "domain-control".into(),
                remote_tab_id: 10,
                preset: crate::domain::model::LivePaneLayoutPreset::Tall,
            }
            .request(),
            crate::domain::protocol::Request::ApplyPaneLayout {
                domain: "domain-control".into(),
                tab: 10,
                preset: crate::domain::model::LivePaneLayoutPreset::Tall,
            }
        );
        assert_eq!(
            LiveDomainControl::MovePane {
                domain_id: "domain-control".into(),
                remote_pane_id: 100,
                source_tab_id: 10,
                destination_tab_id: 11,
                target_pane_id: 110,
                direction: crate::domain::model::SplitDirection::Horizontal,
            }
            .request(),
            crate::domain::protocol::Request::MovePane {
                domain: "domain-control".into(),
                pane: 100,
                tab: 11,
                target: Some(110),
                direction: crate::domain::model::SplitDirection::Horizontal,
            }
        );
        app.pending_live_domain_controls.insert(close.clone());
        app.handle_live_domain_control_completed(close.clone(), Ok(snapshot.clone()));
        assert!(!app.pending_live_domain_controls.contains(&close));
        assert!(
            app.tabs
                .as_ref()
                .unwrap()
                .iter()
                .find(|(_, tab)| tab.id == local_tab_id)
                .unwrap()
                .1
                .is_closing()
        );

        let failed = LiveDomainControl::ClosePane {
            domain_id: "domain-control".into(),
            remote_pane_id: 200,
            local_tab_id: usize::MAX,
        };
        app.pending_live_domain_controls.insert(failed.clone());
        app.handle_live_domain_control_completed(failed.clone(), Err("denied".into()));
        assert!(!app.pending_live_domain_controls.contains(&failed));
        assert!(app.toasts.iter().any(|toast| {
            toast.severity == crate::ui::toast::ToastSeverity::Error
                && toast.message.contains("denied")
        }));
    }

    #[cfg(unix)]
    #[test]
    fn topology_handler_applies_only_newer_revisions_and_closes_the_exact_domain() {
        if !std::path::Path::new("/bin/true").exists() {
            return;
        }
        use crate::domain::model::{
            DomainSnapshot, LivePane, LivePaneLayout, LiveTab, LiveWorkspace,
        };
        let remote_tab = |tab_id, pane_id| LiveTab {
            id: tab_id,
            title: format!("remote-tab-{tab_id}"),
            user_title: None,
            pinned: false,
            focused_pane: pane_id,
            layout: LivePaneLayout::Pane { pane: pane_id },
            panes: vec![LivePane {
                id: pane_id,
                title: format!("remote-pane-{pane_id}"),
                working_directory: None,
                command: vec!["shell".into()],
                cols: 80,
                rows: 24,
                closed: false,
                attachments: 0,
            }],
        };
        let initial = DomainSnapshot {
            id: "domain-topology".into(),
            name: "topology".into(),
            created_at: "2026-01-01T00:00:00Z".into(),
            workspace: LiveWorkspace {
                id: "live-domain-topology".into(),
                name: "topology".into(),
                active_tab: 10,
                tabs: vec![remote_tab(10, 100)],
            },
        };
        let mut tabs = TabManager::new(
            80,
            24,
            crate::config::ShellConfig::default(),
            1_000,
            1024 * 1024,
        );
        tabs.attach_domain(
            &initial,
            &crate::domain::DomainRelayCommand::local("/bin/true"),
        )
        .unwrap();
        let mut app = App::new_headless(crate::config::Config::default()).unwrap();
        app.tabs = Some(tabs);
        app.live_domain_authority = Some(crate::domain::DomainAuthority::Local);
        app.live_domain_relay_command = Some(crate::domain::DomainRelayCommand::local("/bin/true"));

        let mut revision_two = initial.clone();
        revision_two.workspace.active_tab = 20;
        revision_two.workspace.tabs.push(remote_tab(20, 200));
        revision_two.workspace.tabs[1].focused_pane = 201;
        revision_two.workspace.tabs[1].layout = LivePaneLayout::Split {
            split_id: 44,
            direction: crate::domain::model::SplitDirection::Vertical,
            ratio_millis: 500,
            first: Box::new(LivePaneLayout::Pane { pane: 200 }),
            second: Box::new(LivePaneLayout::Pane { pane: 201 }),
        };
        revision_two.workspace.tabs[1].panes.push(LivePane {
            id: 201,
            title: "remote-pane-201".into(),
            working_directory: None,
            command: vec!["shell".into()],
            cols: 80,
            rows: 24,
            closed: false,
            attachments: 0,
        });
        app.handle_live_domain_topology(
            "domain-topology".into(),
            Ok(crate::domain::DomainTopologyEvent::Snapshot {
                revision: 2,
                domain: crate::domain::model::DomainTopology::from(&revision_two),
            }),
        );
        assert_eq!(app.live_domain_revisions["domain-topology"], 2);
        assert_eq!(app.tabs.as_ref().unwrap().iter().count(), 2);
        let stable_routes: std::collections::HashMap<_, _> = app
            .tabs
            .as_ref()
            .unwrap()
            .iter()
            .filter_map(|(_, tab)| {
                tab.live_domain_tab_route()
                    .map(|(_, remote)| (remote, tab.id))
            })
            .collect();
        assert_eq!(
            app.tabs
                .as_ref()
                .unwrap()
                .active_tab()
                .unwrap()
                .pane_manager
                .as_ref()
                .unwrap()
                .get_dividers()[0]
                .ratio_millis,
            500
        );

        let mut revision_three = revision_two.clone();
        let mut pinned = revision_three.workspace.tabs.remove(1);
        pinned.title = "daemon build".into();
        pinned.user_title = Some("daemon build".into());
        pinned.pinned = true;
        revision_three.workspace.tabs.insert(0, pinned);
        app.handle_live_domain_topology(
            "domain-topology".into(),
            Ok(crate::domain::DomainTopologyEvent::Snapshot {
                revision: 3,
                domain: crate::domain::model::DomainTopology::from(&revision_three),
            }),
        );
        let tabs = app.tabs.as_ref().unwrap();
        let order: Vec<_> = tabs
            .iter()
            .filter_map(|(_, tab)| tab.live_domain_tab_route().map(|(_, remote)| remote))
            .collect();
        assert_eq!(order, vec![20, 10]);
        assert_eq!(tabs.active_tab().unwrap().title, "daemon build");
        assert_eq!(
            tabs.active_tab().unwrap().user_title(),
            Some("daemon build")
        );
        assert!(tabs.active_tab().unwrap().pinned);
        for (_, tab) in tabs.iter() {
            let (_, remote) = tab.live_domain_tab_route().unwrap();
            assert_eq!(tab.id, stable_routes[&remote]);
        }
        let active_local_id = tabs.active_tab().unwrap().id;
        let active_pane_id = tabs
            .active_tab()
            .unwrap()
            .pane_manager
            .as_ref()
            .unwrap()
            .focused_id();
        assert!(app.tabs.as_mut().unwrap().set_title_for_origin(
            crate::tabs::TerminalEventOrigin::Pane {
                tab_id: active_local_id,
                pane_id: active_pane_id,
            },
            "relay bypass".into(),
        ));
        assert_eq!(
            app.tabs.as_ref().unwrap().active_tab().unwrap().title,
            "daemon build"
        );

        {
            let tabs = app.tabs.as_mut().unwrap();
            let tab = tabs.active_tab_mut().unwrap();
            tab.set_user_title(Some("optimistic".into()));
            tab.pinned = false;
            assert!(tabs.move_active_to(1));
        }
        app.handle_live_domain_control_completed(
            LiveDomainControl::MoveTab {
                domain_id: "domain-topology".into(),
                remote_tab_id: 20,
                index: 1,
            },
            Err("stale tab order".into()),
        );
        let tabs = app.tabs.as_ref().unwrap();
        let order: Vec<_> = tabs
            .iter()
            .filter_map(|(_, tab)| tab.live_domain_tab_route().map(|(_, remote)| remote))
            .collect();
        assert_eq!(order, vec![20, 10]);
        assert_eq!(tabs.active_tab().unwrap().title, "daemon build");
        assert_eq!(
            tabs.active_tab().unwrap().user_title(),
            Some("daemon build")
        );
        assert!(tabs.active_tab().unwrap().pinned);

        let relay_routes = tabs
            .active_tab()
            .unwrap()
            .live_domain
            .as_ref()
            .unwrap()
            .pane_ids
            .clone();
        let mut revision_four = revision_three.clone();
        revision_four.workspace.tabs[0].layout = LivePaneLayout::Split {
            split_id: 55,
            direction: crate::domain::model::SplitDirection::Horizontal,
            ratio_millis: 500,
            first: Box::new(LivePaneLayout::Pane { pane: 201 }),
            second: Box::new(LivePaneLayout::Pane { pane: 200 }),
        };
        app.handle_live_domain_topology(
            "domain-topology".into(),
            Ok(crate::domain::DomainTopologyEvent::Snapshot {
                revision: 4,
                domain: crate::domain::model::DomainTopology::from(&revision_four),
            }),
        );
        let active = app.tabs.as_ref().unwrap().active_tab().unwrap();
        assert_eq!(active.live_domain.as_ref().unwrap().pane_ids, relay_routes);
        let divider = active.pane_manager.as_ref().unwrap().get_dividers()[0];
        assert_eq!(divider.target, crate::panes::DividerTarget::Domain(55));
        assert!(!divider.vertical);

        app.tabs
            .as_mut()
            .unwrap()
            .active_tab_mut()
            .unwrap()
            .pane_manager
            .as_mut()
            .unwrap()
            .resize_divider_target(crate::panes::DividerTarget::Domain(55), 60.0, 10.0)
            .unwrap();
        app.handle_live_domain_control_completed(
            LiveDomainControl::ResizeSplit {
                domain_id: "domain-topology".into(),
                remote_split_id: 55,
                ratio_millis: 789,
            },
            Err("stale split".into()),
        );
        assert_eq!(
            app.tabs
                .as_ref()
                .unwrap()
                .active_tab()
                .unwrap()
                .pane_manager
                .as_ref()
                .unwrap()
                .get_dividers()[0]
                .ratio_millis,
            500
        );

        // Hiding a tab during a divider preview must finalize through the
        // control path, not silently discard the preview. With an earlier
        // control pending, rejection restores the cached authoritative ratio
        // and does not remove that earlier request.
        let dragged_tab = app.tabs.as_ref().unwrap().active_tab().unwrap().id;
        let pending = LiveDomainControl::ResizeSplit {
            domain_id: "domain-topology".into(),
            remote_split_id: 55,
            ratio_millis: 600,
        };
        for refresh in [true, false] {
            let source_index = app
                .tabs
                .as_ref()
                .unwrap()
                .iter()
                .find_map(|(index, tab)| (tab.id == dragged_tab).then_some(index))
                .unwrap();
            app.tabs.as_mut().unwrap().switch_to(source_index);
            app.tabs
                .as_mut()
                .unwrap()
                .active_tab_mut()
                .unwrap()
                .pane_manager
                .as_mut()
                .unwrap()
                .resize(800.0, 480.0, 10.0, 20.0);
            app.divider_dragging = Some(DividerDrag {
                tab_id: dragged_tab,
                domain: Some(("domain-topology".into(), 20)),
                target: crate::panes::DividerTarget::Domain(55),
                last_ratio_millis: 500,
            });
            app.apply_divider_drag_position(60.0, 320.0);
            assert_ne!(
                app.divider_dragging.as_ref().unwrap().last_ratio_millis,
                500
            );
            app.pending_live_domain_controls.insert(pending.clone());
            app.tabs.as_mut().unwrap().next_tab();
            if refresh {
                app.refresh_focused_terminal_surfaces();
            } else {
                app.apply_divider_drag_position(60.0, 20.0);
            }
            assert!(app.divider_dragging.is_none());
            assert!(app.take_native_drag_controls().is_empty());
            assert_eq!(app.pending_live_domain_controls.len(), 1);
            assert!(app.pending_live_domain_controls.contains(&pending));
            assert_eq!(
                app.tabs
                    .as_ref()
                    .unwrap()
                    .tab_by_id(dragged_tab)
                    .unwrap()
                    .pane_manager
                    .as_ref()
                    .unwrap()
                    .get_dividers()[0]
                    .ratio_millis,
                500
            );
            app.pending_live_domain_controls.remove(&pending);
        }

        let mut revision_five = revision_four.clone();
        let moved = revision_five.workspace.tabs[0]
            .panes
            .iter()
            .position(|pane| pane.id == 201)
            .map(|index| revision_five.workspace.tabs[0].panes.remove(index))
            .unwrap();
        revision_five.workspace.tabs[0].layout = LivePaneLayout::Pane { pane: 200 };
        revision_five.workspace.tabs[0].focused_pane = 200;
        revision_five.workspace.tabs[1].panes.push(moved);
        revision_five.workspace.tabs[1].layout = LivePaneLayout::Split {
            split_id: 56,
            direction: crate::domain::model::SplitDirection::Vertical,
            ratio_millis: 500,
            first: Box::new(LivePaneLayout::Pane { pane: 100 }),
            second: Box::new(LivePaneLayout::Pane { pane: 201 }),
        };
        revision_five.workspace.tabs[1].focused_pane = 201;
        revision_five.workspace.active_tab = 10;
        app.handle_live_domain_topology(
            "domain-topology".into(),
            Ok(crate::domain::DomainTopologyEvent::Snapshot {
                revision: 5,
                domain: crate::domain::model::DomainTopology::from(&revision_five),
            }),
        );
        let tabs = app.tabs.as_ref().unwrap();
        let source = tabs
            .iter()
            .find_map(|(_, tab)| {
                tab.live_domain_tab_route()
                    .and_then(|(_, remote)| (remote == 20).then_some(tab))
            })
            .unwrap();
        assert!(source.pane_manager.is_none());
        assert_eq!(
            source.live_domain.as_ref().unwrap().pane_ids,
            std::collections::HashMap::from([(crate::panes::PaneId(0), 200)])
        );
        let destination = tabs.active_tab().unwrap();
        assert_eq!(destination.live_domain_tab_route().unwrap().1, 10);
        let destination_panes = destination.pane_manager.as_ref().unwrap();
        assert_eq!(destination_panes.pane_count(), 2);
        assert_eq!(
            destination_panes.spawned_shell_count(),
            0,
            "the existing relay must move instead of being respawned"
        );
        let destination_routes: std::collections::HashSet<_> = destination
            .live_domain
            .as_ref()
            .unwrap()
            .pane_ids
            .values()
            .copied()
            .collect();
        assert_eq!(
            destination_routes,
            std::collections::HashSet::from([100, 201])
        );
        assert_eq!(destination.live_domain_focus_route().unwrap().1, 201);

        app.handle_live_domain_topology("domain-topology".into(), Err("transport reset".into()));
        assert!(app.disconnected_live_domains.contains("domain-topology"));
        let disconnect_warnings = app
            .toasts
            .iter()
            .filter(|toast| {
                toast.severity == crate::ui::toast::ToastSeverity::Warning
                    && toast
                        .message
                        .contains("Live domain domain-topology on this host disconnected")
            })
            .count();
        assert_eq!(disconnect_warnings, 1);
        app.handle_live_domain_topology("domain-topology".into(), Err("same outage".into()));
        assert_eq!(
            app.toasts
                .iter()
                .filter(|toast| {
                    toast.severity == crate::ui::toast::ToastSeverity::Warning
                        && toast
                            .message
                            .contains("Live domain domain-topology on this host disconnected")
                })
                .count(),
            disconnect_warnings,
            "one outage should produce one warning"
        );

        app.handle_live_domain_topology(
            "domain-topology".into(),
            Ok(crate::domain::DomainTopologyEvent::Snapshot {
                revision: 5,
                domain: crate::domain::model::DomainTopology::from(&revision_five),
            }),
        );
        assert!(
            !app.disconnected_live_domains.contains("domain-topology"),
            "an equal fresh snapshot must still complete recovery"
        );
        assert!(app.toasts.iter().any(|toast| {
            toast.severity == crate::ui::toast::ToastSeverity::Success
                && toast.message == "Reconnected live domain domain-topology on this host"
        }));

        app.handle_live_domain_topology(
            "domain-topology".into(),
            Err("transport reset again".into()),
        );
        let mut wrong_route = crate::domain::model::DomainTopology::from(&revision_five);
        wrong_route.domain_id = "different-domain".into();
        app.handle_live_domain_topology(
            "domain-topology".into(),
            Ok(crate::domain::DomainTopologyEvent::Snapshot {
                revision: 6,
                domain: wrong_route,
            }),
        );
        assert!(
            app.disconnected_live_domains.contains("domain-topology"),
            "a mismatched snapshot must not declare the expected route recovered"
        );
        assert!(app.toasts.iter().any(|toast| {
            toast.severity == crate::ui::toast::ToastSeverity::Error
                && toast.message.contains("Rejected live-domain topology")
        }));

        app.handle_live_domain_topology(
            "domain-topology".into(),
            Ok(crate::domain::DomainTopologyEvent::Snapshot {
                revision: 1,
                domain: crate::domain::model::DomainTopology::from(&initial),
            }),
        );
        assert_eq!(app.tabs.as_ref().unwrap().iter().count(), 2);

        app.handle_live_domain_topology(
            "domain-topology".into(),
            Ok(crate::domain::DomainTopologyEvent::Closed {
                domain: "domain-topology".into(),
                message: "terminated".into(),
            }),
        );
        assert!(!app.live_domain_revisions.contains_key("domain-topology"));
        assert!(
            !app.disconnected_live_domains.contains("domain-topology"),
            "a definitive close ends the outage without a reconnect notice"
        );
        assert!(
            app.tabs
                .as_ref()
                .unwrap()
                .iter()
                .filter(|(_, tab)| tab.live_domain.is_some())
                .all(|(_, tab)| tab.is_closing())
        );
    }

    #[cfg(unix)]
    #[test]
    fn broadcast_targeting_excludes_and_rejects_read_only_panes() {
        use std::collections::HashMap;

        if !std::path::Path::new("/bin/sh").is_file() {
            return;
        }
        let shell = crate::config::ShellConfig {
            program: Some("/bin/sh".into()),
            args: vec!["-c".into(), "sleep 2".into()],
            env: Default::default(),
        };
        let mut tabs = TabManager::new(40, 8, shell.clone(), 100, 1024 * 1024);
        tabs.new_tab_with_command(
            "/bin/sh",
            &shell.args,
            None,
            Some("read-only-broadcast"),
            None,
            HashMap::new(),
        )
        .unwrap();
        let owner = tabs
            .active_tab_mut()
            .unwrap()
            .take_terminal_owner()
            .unwrap();
        let mut panes =
            PaneManager::from_existing(owner, shell, 100, 1024 * 1024, 400.0, 160.0, 10.0, 20.0);
        panes.split(SplitDirection::Vertical).unwrap();
        let locked = panes.split(SplitDirection::Horizontal).unwrap();
        panes.get_pane_mut(locked).unwrap().read_only = true;
        tabs.active_tab_mut().unwrap().pane_manager = Some(panes);
        let mut app = App::new_headless(Config::default()).unwrap();
        app.tabs = Some(tabs);

        app.toggle_broadcast();
        let BroadcastState::Armed { targets, .. } = &app.broadcast else {
            panic!("two writable panes should arm broadcast");
        };
        assert_eq!(targets.len(), 2);
        assert!(!targets.contains(&locked));
        assert!(app.toggle_armed_broadcast_target(locked));
        let BroadcastState::Armed { targets, .. } = &app.broadcast else {
            panic!("rejected read-only target should leave broadcast armed");
        };
        assert_eq!(targets.len(), 2);
        assert!(!targets.contains(&locked));
    }

    #[cfg(unix)]
    #[test]
    fn selected_broadcast_reaches_only_targets_and_closure_reconciles_it() {
        use std::collections::HashMap;
        use std::time::{Duration, Instant};

        if !std::path::Path::new("/bin/sh").is_file() {
            return;
        }
        let shell = crate::config::ShellConfig {
            program: Some("/bin/sh".into()),
            args: vec![
                "-c".into(),
                "stty raw -echo; dd bs=1 count=1 2>/dev/null | od -An -tx1; sleep 1".into(),
            ],
            env: Default::default(),
        };
        let mut tabs = TabManager::new(
            40,
            8,
            crate::config::ShellConfig::default(),
            100,
            1024 * 1024,
        );
        tabs.new_tab_with_command(
            "/bin/sh",
            &shell.args,
            None,
            Some("broadcast-routing"),
            None,
            HashMap::new(),
        )
        .unwrap();
        let owner = tabs
            .active_tab_mut()
            .unwrap()
            .take_terminal_owner()
            .unwrap();
        let mut panes =
            PaneManager::from_existing(owner, shell, 100, 1024 * 1024, 400.0, 160.0, 10.0, 20.0);
        panes.split(SplitDirection::Vertical).unwrap();
        panes.split(SplitDirection::Horizontal).unwrap();
        tabs.active_tab_mut().unwrap().pane_manager = Some(panes);
        let mut app = App::new_headless(Config::default()).unwrap();
        app.tabs = Some(tabs);

        app.toggle_broadcast();
        assert!(matches!(app.broadcast, BroadcastState::Armed { .. }));
        let (pane_ids, excluded) = {
            let manager = app
                .tabs
                .as_ref()
                .unwrap()
                .active_tab()
                .unwrap()
                .pane_manager
                .as_ref()
                .unwrap();
            (manager.pane_ids(), manager.focused_id())
        };
        assert!(app.toggle_armed_broadcast_target(excluded));
        assert!(matches!(
            app.broadcast,
            BroadcastState::Armed { ref targets, .. }
                if targets.len() == 2 && !targets.contains(&excluded)
        ));
        let temporarily_excluded = pane_ids.iter().copied().find(|id| *id != excluded).unwrap();
        assert!(app.toggle_armed_broadcast_target(temporarily_excluded));
        app.toggle_broadcast();
        assert!(matches!(
            app.broadcast,
            BroadcastState::Armed { ref targets, .. } if targets.len() == 1
        ));
        assert!(app.toggle_armed_broadcast_target(temporarily_excluded));
        app.write_to_pty(b"z");
        app.write_paste_text("z");
        app.handle_ime_event(winit::event::Ime::Commit("z".into()));
        app.toggle_broadcast();
        assert!(matches!(
            app.broadcast,
            BroadcastState::Active { ref targets, .. }
                if targets.len() == 2 && !targets.contains(&excluded)
        ));
        assert!(app.write_broadcast_bytes(b"x"));

        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            app.process_pty_events();
            let manager = app
                .tabs
                .as_ref()
                .unwrap()
                .active_tab()
                .unwrap()
                .pane_manager
                .as_ref()
                .unwrap();
            let reached = pane_ids.iter().all(|id| {
                let output = manager.get_pane(*id).unwrap().terminal.rows_text(0, 7);
                output.contains("78") == (*id != excluded)
            });
            if reached || Instant::now() >= deadline {
                assert!(
                    reached,
                    "broadcast byte did not reach exactly the selected targets"
                );
                break;
            }
            std::thread::yield_now();
        }

        app.tabs
            .as_mut()
            .unwrap()
            .active_tab_mut()
            .unwrap()
            .pane_manager
            .as_mut()
            .unwrap()
            .close_focused();
        app.reconcile_broadcast_state();
        assert!(matches!(
            app.broadcast,
            BroadcastState::Active { ref targets, .. } if targets.len() == 2
        ));

        app.close_focused_pane();
        assert!(matches!(app.broadcast, BroadcastState::Inactive));
    }
}
