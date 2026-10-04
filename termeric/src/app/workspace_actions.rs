//! Tab and pane actions from the keyboard, menus, and palette: moving,
//! pinning, splitting, layouts, navigation, read-only, and closing.

use super::redraw_scheduler::RedrawReason;
use super::*;

impl App {
    pub(super) fn request_move_active_tab_by(&mut self, delta: isize) {
        let live_move = self.tabs.as_ref().and_then(|tabs| {
            let active = tabs.active_tab()?;
            let (domain, remote_tab_id) = active.live_domain_tab_route()?;
            let source = tabs.live_domain_tab_index(domain, remote_tab_id)?;
            let target = source.checked_add_signed(delta)?;
            let target_pinned = tabs.live_domain_tabs(domain).nth(target)?.pinned;
            (active.pinned == target_pinned).then_some((
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
        self.request_redraw(RedrawReason::Action);
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
                    direction: crate::panes::SplitDirection::Vertical,
                });
            } else {
                self.toasts
                    .warning("Pane was not moved: A second live-domain tab is required");
            }
            self.request_redraw(RedrawReason::Action);
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
        self.request_redraw(RedrawReason::Action);
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
            self.request_redraw(RedrawReason::Action);
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
        self.request_redraw(RedrawReason::Action);
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

        let direction_name = direction.name();
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
        self.request_redraw(RedrawReason::Action);
        Ok(new_id.0)
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
        self.request_redraw(RedrawReason::Action);
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
        if let Err(error) = self.create_tab_for(Requester::Control) {
            log::error!("Failed to create new tab: {error}");
        }
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
        self.request_redraw(RedrawReason::Action);
    }
}
