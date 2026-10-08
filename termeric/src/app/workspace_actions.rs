//! Tab and pane actions from the keyboard, menus, and palette: moving,
//! pinning, splitting, layouts, navigation, read-only, and closing.

use super::redraw_scheduler::RedrawReason;
use super::*;
use anyhow::Context;

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
            .is_some_and(|tab| action(&mut tab.panes));
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
            .filter(|tab| tab.split_panes().is_some())
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
            .filter(|tab| tab.split_panes().is_some())
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
            .filter(|tab| tab.split_panes().is_some())
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
            let has_multiple_panes = source.panes.pane_count() > 1;
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
            .and_then(TabManager::move_focused_pane_to_next_tab);
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
                tab.panes.set_cwd_mode(self.config.panes.cwd);
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
    /// Split the focused pane of the current tab. A live-domain pane's
    /// split is only queued here; the daemon reports the result later.
    pub(super) fn split_pane(&mut self, direction: SplitDirection) -> anyhow::Result<()> {
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
            return Ok(());
        }
        let tab = self
            .tabs
            .as_mut()
            .and_then(TabManager::active_tab_mut)
            .context("no tab is open")?;
        if !tab.allows_pane_splits() {
            anyhow::bail!("serial terminal tabs cannot be split into panes");
        }
        tab.panes.set_cwd_mode(self.config.panes.cwd);
        let new_id = tab.panes.split(direction)?;
        log::info!("Split pane {direction:?}, created pane {new_id:?}");
        self.refresh_focused_terminal_surfaces();
        self.request_redraw(RedrawReason::Action);
        Ok(())
    }

    /// Split the focused pane for a shortcut, menu, or palette command,
    /// telling the person when it cannot be split.
    pub(super) fn split_pane_or_report(&mut self, direction: SplitDirection) {
        if let Err(error) = self.split_pane(direction) {
            self.toasts
                .error(format!("Could not split the pane: {error:#}"));
            self.request_redraw(RedrawReason::Action);
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
        let cwd_mode = self.config.panes.cwd;
        let tab = self.control_tab_mut(tab_id)?;
        if tab.live_domain_tab_route().is_some() {
            return Err("Control-created panes are not yet supported for live-domain tabs".into());
        }
        if !tab.allows_pane_splits() {
            return Err("Serial terminal tabs cannot be split into panes".into());
        }

        let direction_name = direction.name();
        let target = super::control_actions::control_running_pane(tab, pane_id)?.id;
        let manager = &mut tab.panes;
        manager.set_cwd_mode(cwd_mode);
        let new_id = manager.split_target(target, direction).map_err(|error| {
            format!("Could not split pane {pane_id} {direction_name}: {error:#}")
        })?;

        log::info!(
            "Control split pane {pane_id} {direction_name}, created pane {}",
            new_id.0
        );
        self.refresh_focused_terminal_surfaces();
        self.request_redraw(RedrawReason::Action);
        Ok(new_id.0)
    }

    /// Move focus to the adjacent pane in `dir`. With no pane in that
    /// direction we surface a short info toast so the user knows the key was
    /// registered but the layout has nowhere to go.
    pub(super) fn navigate_focused_pane(&mut self, dir: crate::panes::PaneDir) {
        let moved = {
            let mut moved = false;
            if let Some(tab) = self.tabs.as_mut().and_then(TabManager::active_tab_mut) {
                moved = tab.panes.navigate(dir);
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
            self.toasts.error(error);
            self.request_redraw(RedrawReason::Action);
        }
    }

    /// Close the focused pane. A tab's last pane is closed with the tab.
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
                        tab.panes.pane_count(),
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
        if let Some(tab) = self.tabs.as_mut().and_then(TabManager::active_tab_mut) {
            changed = tab.panes.close_focused();
            if changed {
                tab.reconcile_primary_pane_launch();
            }
        }
        if changed {
            if !matches!(self.broadcast, BroadcastState::Inactive) {
                self.broadcast = BroadcastState::Inactive;
                self.toasts.info("Broadcast cancelled after pane closure");
            }
            self.refresh_focused_terminal_surfaces();
        } else {
            self.toasts
                .warning("Cannot close the last pane; close the tab instead");
        }
        self.request_redraw(RedrawReason::Action);
    }
}
