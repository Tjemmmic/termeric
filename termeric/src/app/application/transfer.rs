//! Moving tabs and panes between windows: the next window, drops, and
//! tearing a tab off into a new window.

use super::*;

impl Application {
    pub(super) fn next_live_window(&self, source: WindowId) -> Option<WindowId> {
        let source_index = self.window_order.iter().position(|id| *id == source)?;
        (1..self.window_order.len())
            .map(|offset| self.window_order[(source_index + offset) % self.window_order.len()])
            .find(|candidate| {
                // A hidden quick-access terminal would take a tab out of sight.
                self.windows.get(candidate).is_some_and(|window| {
                    !window.should_exit
                        && !(window.launch.quick_access && !window.quick_access_visible)
                })
            })
    }

    pub(super) fn transfer_focused_pane(
        source: &mut App,
        destination: &mut App,
    ) -> Result<crate::panes::PaneId> {
        source
            .tabs
            .as_ref()
            .expect("source tabs were inspected")
            .validate_window_pane_transfer_source()?;
        destination
            .tabs
            .as_ref()
            .expect("destination tabs were inspected")
            .validate_window_pane_transfer_destination()?;

        let pane = source
            .tabs
            .as_mut()
            .expect("source tabs were validated")
            .detach_focused_pane_for_window_transfer()?;
        let id = destination
            .tabs
            .as_mut()
            .expect("destination tabs were validated")
            .attach_window_transferred_pane(pane);

        source.broadcast = crate::app::BroadcastState::Inactive;
        destination.broadcast = crate::app::BroadcastState::Inactive;
        source.leave_copy_mode(false);
        destination.leave_copy_mode(false);
        source.terminal_mouse_capture = None;
        destination.terminal_mouse_capture = None;
        source.refresh_focused_terminal_surfaces();
        destination.refresh_focused_terminal_surfaces();
        source.toasts.success("Pane moved to the next window");
        destination.toasts.info("Pane received from another window");
        source.request_redraw(RedrawReason::Action);
        destination.request_redraw(RedrawReason::Action);
        Ok(id)
    }

    pub(super) fn fulfill_pane_window_move(&mut self, source_id: WindowId) {
        let Some(destination_id) = self.next_live_window(source_id) else {
            if let Some(source) = self.windows.get_mut(&source_id) {
                source
                    .toasts
                    .warning("Pane was not moved: A second window is required");
            }
            return;
        };
        let Some(mut destination) = self.windows.remove(&destination_id) else {
            return;
        };
        let result = self
            .windows
            .get_mut(&source_id)
            .ok_or_else(|| anyhow::anyhow!("The source window closed before the move"))
            .and_then(|source| Self::transfer_focused_pane(source, &mut destination));
        if let Err(error) = result
            && let Some(source) = self.windows.get_mut(&source_id)
        {
            source
                .toasts
                .warning(format!("Pane was not moved: {error:#}"));
        }
        self.windows.insert(destination_id, destination);
    }

    pub(super) fn transfer_active_tab(source: &mut App, destination: &mut App) -> Result<usize> {
        source
            .tabs
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("No source workspace is active"))?
            .validate_window_tab_transfer_source()?;
        if destination.tabs.is_none() {
            anyhow::bail!("No destination workspace is active");
        }
        if let Some(refusal) = destination.live_domain_tab_move_refusal() {
            anyhow::bail!(refusal);
        }
        let moving = source
            .tabs
            .as_ref()
            .and_then(crate::tabs::TabManager::active_tab_id)
            .expect("the source tab was validated");
        if let Some(refusal) = source.agent_view_tab_move_refusal(destination, moving) {
            anyhow::bail!(refusal);
        }
        // A draft never reached its tab, so dropping it leaves the title.
        source.tab_rename = None;
        let tab = source
            .tabs
            .as_mut()
            .expect("source tabs were validated")
            .detach_active_tab_for_window_transfer()?;
        let id = destination
            .tabs
            .as_mut()
            .expect("destination tabs were validated")
            .attach_window_transferred_tab(tab);
        // The tab's agent view conversation moves with it.
        App::move_agent_view_tab(source, destination, moving, id);

        source.broadcast = crate::app::BroadcastState::Inactive;
        destination.broadcast = crate::app::BroadcastState::Inactive;
        source.leave_copy_mode(false);
        destination.leave_copy_mode(false);
        source.terminal_mouse_capture = None;
        destination.terminal_mouse_capture = None;
        source.tab_dragging = None;
        destination.tab_dragging = None;
        source.refresh_focused_terminal_surfaces();
        destination.refresh_focused_terminal_surfaces();
        source.should_exit = source
            .tabs
            .as_ref()
            .is_none_or(crate::tabs::TabManager::is_empty);
        if !source.should_exit {
            source.toasts.success("Tab moved to the next window");
        }
        destination.toasts.info("Tab received from another window");
        source.request_redraw(RedrawReason::Action);
        destination.request_redraw(RedrawReason::Action);
        Ok(id)
    }

    /// Moves a dropped tab into window `destination`, where the pointer is
    /// at `position`.
    pub(super) fn drop_tab_into(
        &mut self,
        drop: PendingTabDrop,
        destination_id: WindowId,
        position: (f64, f64),
    ) {
        let Some(pinned) = self
            .windows
            .get(&drop.source)
            .and_then(|source| source.tabs.as_ref()?.tab_by_id(drop.tab_id))
            .map(|tab| tab.pinned)
        else {
            return;
        };
        let Some(mut destination) = self
            .windows
            .remove(&destination_id)
            .filter(|window| !window.should_exit)
        else {
            return;
        };
        let index = destination.tab_drop_index(position, pinned);
        let result = self
            .windows
            .get_mut(&drop.source)
            .ok_or_else(|| "The tab's window closed".to_string())
            .and_then(|source| {
                Self::transfer_control_tab(source, &mut destination, drop.tab_id as u64, index)
            });
        match result {
            Ok(_) => {
                if let Some(window) = &destination.window {
                    window.focus_window();
                }
            }
            Err(error) => destination
                .toasts
                .warning(format!("Tab was not moved: {error}")),
        }
        self.windows.insert(destination_id, destination);
    }

    /// Opens a window of its own for a tab dropped outside every Termeric
    /// window. A window's only tab stays: it already has a window.
    pub(super) fn tear_off_tab(&mut self, drop: PendingTabDrop, event_loop: &ActiveEventLoop) {
        let Some(source) = self
            .windows
            .get(&drop.source)
            .filter(|window| !window.should_exit)
        else {
            return;
        };
        let alone = source
            .tabs
            .as_ref()
            .is_none_or(|tabs| tabs.iter().count() <= 1);
        if alone {
            return;
        }
        let config = source.config.clone();
        let launch = WindowLaunch {
            force_new: true,
            adopts_tab: true,
            ..WindowLaunch::default()
        };
        let result_index = self.pending.len();
        if let Err(error) = self.queue_window(config, launch) {
            log::error!("Could not prepare a window for the torn-off tab: {error:#}");
            return;
        }
        // The window exists before the tab leaves its old one, so a window
        // that cannot open loses nothing.
        let created = self
            .create_pending_windows(event_loop)
            .into_iter()
            .nth(result_index);
        let Some(Ok(created)) = created else {
            if let Some(source) = self.windows.get_mut(&drop.source) {
                source
                    .toasts
                    .warning("Tab was not moved: a new window could not open");
            }
            return;
        };
        let Some(destination_id) = self
            .control_window_ids
            .iter()
            .find_map(|(native, id)| (*id == created.window_id).then_some(*native))
        else {
            return;
        };
        let Some(mut destination) = self.windows.remove(&destination_id) else {
            return;
        };
        let result = self
            .windows
            .get_mut(&drop.source)
            .ok_or_else(|| "The tab's window closed".to_string())
            .and_then(|source| {
                Self::transfer_control_tab(source, &mut destination, drop.tab_id as u64, 0)
            });
        if let Err(error) = result {
            // Nothing moved in: the empty window has no reason to stay.
            destination.should_exit = true;
            if let Some(source) = self.windows.get_mut(&drop.source) {
                source.toasts.warning(format!("Tab was not moved: {error}"));
            }
        }
        self.windows.insert(destination_id, destination);
    }

    pub(super) fn fulfill_tab_window_move(&mut self, source_id: WindowId) {
        let Some(destination_id) = self.next_live_window(source_id) else {
            if let Some(source) = self.windows.get_mut(&source_id) {
                source
                    .toasts
                    .warning("Tab was not moved: A second window is required");
            }
            return;
        };
        let Some(mut destination) = self.windows.remove(&destination_id) else {
            return;
        };
        let result = self
            .windows
            .get_mut(&source_id)
            .ok_or_else(|| anyhow::anyhow!("The source window closed before the move"))
            .and_then(|source| Self::transfer_active_tab(source, &mut destination));
        if let Err(error) = result
            && let Some(source) = self.windows.get_mut(&source_id)
        {
            source
                .toasts
                .warning(format!("Tab was not moved: {error:#}"));
        }
        self.windows.insert(destination_id, destination);
    }
}
