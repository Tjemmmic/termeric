//! Explicit plugin creation splits the target pane and spawns one child.
use super::*;
use crate::panes::{LocalPaneLaunch, SplitDirection};

impl TabManager {
    pub(crate) fn plugin_pane_geometry(
        &self,
        target: TerminalIdentity,
        direction: SplitDirection,
    ) -> anyhow::Result<crate::terminal::geometry::TerminalGeometry> {
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
        if tab.panes.pane_count() >= crate::workspace::MAX_WORKSPACE_PANES_PER_TAB {
            anyhow::bail!("workspace pane limit reached");
        }
        tab.panes.split_geometry(target.pane_id, direction)
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
        self.tab_mut_by_id(target.tab_id)
            .expect("validated target")
            .panes
            .split_target_with_launch(target.pane_id, direction, launch)
    }
}
