//! Strict, targeted launch resolution shared by restoration and explicit panes.
use super::*;
use crate::plugin::PluginAction;
use crate::workspace::WorkspaceLaunch;

impl App {
    pub(super) fn validate_plugin_launch_profile(
        &self,
        plugin_id: &str,
        profile: Option<&str>,
    ) -> anyhow::Result<()> {
        let configured = match self.agents.get(plugin_id) {
            Some(agent) if !self.agent_features_enabled() => anyhow::bail!(
                "{} is a coding agent: {}",
                agent.name,
                crate::agents::config::AGENT_FEATURES_OFF_REPLY
            ),
            Some(agent) => {
                agent.enabled && profile.is_none_or(|name| agent.profiles.contains_key(name))
            }
            None => true, // Other registered plugins validate their own profile namespace.
        };
        if !configured {
            anyhow::bail!(
                "plugin {plugin_id:?} or its requested/resolved profile {profile:?} is no longer configured"
            );
        }
        Ok(())
    }

    pub(super) fn resolve_plugin_terminal_launch(
        &mut self,
        hook: &crate::plugin::Hook,
        ctx: &PluginContext,
    ) -> anyhow::Result<(WorkspaceLaunch, crate::config::ShellConfig)> {
        let (crate::plugin::Hook::WorkspaceRestore {
            plugin_id,
            profile,
            cwd,
        }
        | crate::plugin::Hook::PaneLaunch {
            plugin_id,
            profile,
            cwd,
        }) = hook
        else {
            anyhow::bail!("expected a terminal launch hook");
        };
        let profile = profile.as_deref();
        self.validate_plugin_launch_profile(plugin_id, profile)?;
        let action = self.plugin_manager.dispatch_to(plugin_id, hook, ctx);
        let launch = self.check_plugin_terminal_launch(plugin_id, profile, cwd.as_ref(), action)?;
        Ok((
            WorkspaceLaunch::Plugin {
                plugin_id: plugin_id.into(),
                profile: launch.profile,
            },
            launch.shell,
        ))
    }

    /// Check that `action`, what `plugin_id` answered when asked for a
    /// terminal in `cwd` (with its saved `profile`, if any), launches
    /// exactly that.
    pub(super) fn check_plugin_terminal_launch(
        &self,
        plugin_id: &str,
        profile: Option<&str>,
        cwd: Option<&String>,
        action: Option<PluginAction>,
    ) -> anyhow::Result<PluginTerminalLaunch> {
        match action {
            Some(PluginAction::OpenTab {
                program,
                args,
                cwd: actual_cwd,
                title,
                plugin_id: actual_id,
                profile: actual_profile,
                env,
            }) => {
                if actual_id.as_deref() != Some(plugin_id)
                    || profile.is_some_and(|saved| actual_profile.as_deref() != Some(saved))
                    || actual_cwd.as_ref() != cwd
                    || program.is_empty()
                {
                    anyhow::bail!("plugin returned a mismatched terminal launch");
                }
                self.validate_plugin_launch_profile(plugin_id, actual_profile.as_deref())?;
                Ok(PluginTerminalLaunch {
                    profile: actual_profile,
                    shell: crate::config::ShellConfig {
                        program: Some(program),
                        args,
                        env: env.into_iter().collect(),
                    },
                    title,
                })
            }
            Some(PluginAction::ShowNotice { .. }) => anyhow::bail!(
                "plugin did not provide a clean terminal launch; configuration needs attention"
            ),
            Some(_) => anyhow::bail!("plugin returned an unsupported terminal launch action"),
            None => anyhow::bail!("plugin {plugin_id:?} is not available"),
        }
    }
}

/// A terminal launch a plugin returned, checked against what it was asked.
pub(super) struct PluginTerminalLaunch {
    /// The profile it launches, which may be the plugin's default.
    pub profile: Option<String>,
    pub shell: crate::config::ShellConfig,
    /// What the plugin titled the terminal.
    pub title: Option<String>,
}
