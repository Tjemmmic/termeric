//! Explicit plugin creation adopts the original terminal and spawns one child.
use super::*;
use crate::panes::{LocalPaneLaunch, PaneManager, SplitDirection};

impl TabManager {
    pub(crate) fn plugin_pane_geometry(
        &self,
        target: TerminalIdentity,
        direction: SplitDirection,
    ) -> anyhow::Result<TerminalGeometry> {
        let tab = self
            .tab_by_id(target.tab_id)
            .context("target tab is unavailable")?;
        if tab.closed
            || tab.closing_at.is_some()
            || tab.live_domain.is_some()
            || !tab.allows_pane_splits()
        {
            anyhow::bail!("plugin panes require an open ordinary local process tab");
        }
        let origin = self
            .origin_for_pane(target.tab_id, target.pane_id)
            .context("target pane is unavailable")?;
        if self.origin_read_only(origin) != Some(false) {
            anyhow::bail!("target pane is read-only");
        }
        if let Some(panes) = &tab.pane_manager {
            if panes.pane_count() >= crate::workspace::MAX_WORKSPACE_PANES_PER_TAB {
                anyhow::bail!("workspace pane limit reached");
            }
            panes.split_geometry(target.pane_id, direction)
        } else {
            let geometry = self.geometry;
            Ok(PaneManager::child_geometry(
                f32::from(geometry.pixel_width),
                f32::from(geometry.pixel_height),
                f32::from(geometry.pixel_width) / geometry.cols as f32,
                f32::from(geometry.pixel_height) / geometry.rows as f32,
                direction,
            ))
        }
    }

    pub(crate) fn create_plugin_pane(
        &mut self,
        target: TerminalIdentity,
        direction: SplitDirection,
        launch: &LocalPaneLaunch,
    ) -> anyhow::Result<PaneId> {
        self.plugin_pane_geometry(target, direction)?;
        if !matches!(
            launch.identity,
            crate::workspace::WorkspaceLaunch::Plugin { .. }
        ) || launch.shell.program.as_deref().is_none_or(str::is_empty)
        {
            anyhow::bail!("expected a resolved plugin launch");
        }
        let geometry = self.geometry;
        let lines = self.scrollback_lines;
        let bytes = self.scrollback_bytes;
        let tab = self.tab_mut_by_id(target.tab_id).expect("validated target");
        if let Some(panes) = &mut tab.pane_manager {
            return panes.split_target_with_launch(target.pane_id, direction, launch);
        }
        let shell = tab.split_shell_config();
        let owner = tab
            .take_terminal_owner()
            .context("target terminal owner is unavailable")?;
        let mut panes = PaneManager::from_existing(
            owner,
            shell,
            lines,
            bytes,
            f32::from(geometry.pixel_width),
            f32::from(geometry.pixel_height),
            f32::from(geometry.pixel_width) / geometry.cols as f32,
            f32::from(geometry.pixel_height) / geometry.rows as f32,
        );
        match panes.split_target_with_launch(target.pane_id, direction, launch) {
            Ok(id) => {
                tab.pane_manager = Some(panes);
                Ok(id)
            }
            Err(error) => {
                let owner = panes
                    .take_remaining_pane()
                    .expect("failed split retained original owner");
                tab.restore_terminal_owner(owner);
                Err(error)
            }
        }
    }
}
