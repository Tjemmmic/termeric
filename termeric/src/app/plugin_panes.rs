//! User-requested plugin panes, separate from provider execution and tab opens.
use super::*;
use crate::panes::LocalPaneLaunch;
use crate::tabs::TerminalIdentity;

impl App {
    pub(super) fn plugin_pane_commands(
        &self,
    ) -> Vec<crate::ui::command_palette::palette::RegisteredCommand> {
        use crate::ui::command_palette::palette::{CommandSource, RegisteredCommand};
        let mut commands = Vec::new();
        for (id, name, mut profiles) in self.plugin_manager.pane_launch_profiles() {
            // Agents are offered once, through the agent picker.
            if self.agents.get(&id).is_some() {
                continue;
            }
            profiles.sort();
            for profile in std::iter::once(None).chain(profiles.iter().map(Some)) {
                for (direction, placement) in [("vertical", "Right"), ("horizontal", "Down")] {
                    let selection =
                        profile.map_or_else(String::new, |name| format!(" ({name} profile)"));
                    commands.push(RegisteredCommand {
                        id: format!(
                            "plugin-pane.{direction}.{id}.{}",
                            profile.map_or("", String::as_str)
                        ),
                        title: format!("Split {placement} with {name}{selection}"),
                        category: "Plugin Panes".into(),
                        shortcut: None,
                        source: CommandSource::LaunchProfiles,
                        destructive: false,
                    });
                }
            }
        }
        commands
    }

    pub(super) fn open_plugin_pane_command(&mut self, value: &str) {
        let result = (|| -> anyhow::Result<()> {
            let (direction, rest) = value
                .split_once('.')
                .ok_or_else(|| anyhow::anyhow!("invalid plugin pane command"))?;
            let direction = match direction {
                "vertical" => SplitDirection::Vertical,
                "horizontal" => SplitDirection::Horizontal,
                _ => anyhow::bail!("invalid pane direction"),
            };
            let (plugin, profile) = rest
                .split_once('.')
                .ok_or_else(|| anyhow::anyhow!("invalid plugin pane selection"))?;
            if !self
                .plugin_manager
                .pane_launch_profiles()
                .iter()
                .any(|(id, _, _)| id == plugin)
            {
                anyhow::bail!("plugin pane command is unavailable");
            }
            let tab = self
                .tabs
                .as_ref()
                .and_then(TabManager::active_tab)
                .ok_or_else(|| anyhow::anyhow!("no active terminal"))?;
            let target = TerminalIdentity {
                tab_id: tab.id,
                pane_id: tab
                    .pane_manager
                    .as_ref()
                    .map_or(tab.single_pane_id(), PaneManager::focused_id),
            };
            self.launch_plugin_pane(
                target,
                direction,
                plugin,
                (!profile.is_empty()).then_some(profile),
            )?;
            Ok(())
        })();
        if let Err(error) = result {
            self.toasts
                .error(format!("Could not open plugin pane: {error:#}"));
        }
    }

    pub(super) fn launch_plugin_pane(
        &mut self,
        target: TerminalIdentity,
        direction: SplitDirection,
        plugin_id: &str,
        profile: Option<&str>,
    ) -> anyhow::Result<PaneId> {
        if !matches!(self.broadcast, BroadcastState::Inactive) {
            anyhow::bail!("turn off broadcast input before opening a plugin pane");
        }
        let tabs = self
            .tabs
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("no tab manager"))?;
        let geometry = tabs.plugin_pane_geometry(target, direction)?;
        let tab = tabs.tab_by_id(target.tab_id).expect("validated target");
        let cwd = self.config.panes.cwd.resolve(
            || tab.pane_working_directory(target.pane_id),
            || {
                tab.pane_working_directory(
                    tab.pane_manager
                        .as_ref()
                        .map_or(tab.single_pane_id(), PaneManager::primary_id),
                )
            },
        );
        if cwd
            .as_ref()
            .is_some_and(|cwd| !std::path::Path::new(cwd).is_dir())
        {
            anyhow::bail!("target working directory is unavailable");
        }
        let context = PluginContext {
            window_id: self.control_window_id,
            cwd: cwd.clone(),
            tab_count: tabs.count(),
            active_tab: tabs
                .iter()
                .find_map(|(index, tab)| (tab.id == target.tab_id).then_some(index))
                .expect("validated target"),
            terminal_size: (geometry.cols, geometry.rows),
        };
        let (identity, shell) = self.resolve_plugin_terminal_launch(
            &crate::plugin::Hook::PaneLaunch {
                plugin_id: plugin_id.into(),
                profile: profile.map(str::to_owned),
                cwd: cwd.clone(),
            },
            &context,
        )?;
        let id = self
            .tabs
            .as_mut()
            .expect("tab manager retained")
            .create_plugin_pane(
                target,
                direction,
                &LocalPaneLaunch {
                    identity,
                    shell,
                    cwd,
                },
            )?;
        self.refresh_focused_terminal_surfaces();
        self.request_redraw(RedrawReason::Action);
        Ok(id)
    }
}
