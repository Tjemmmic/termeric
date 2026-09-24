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
        let actions = self.plugin_manager.dispatch_to(plugin_id, hook, ctx);
        let mut launch = None;
        for action in actions {
            match action {
                PluginAction::OpenTab {
                    program,
                    args,
                    cwd: actual_cwd,
                    plugin_id: actual_id,
                    profile: actual_profile,
                    env,
                    ..
                } => {
                    if launch.is_some()
                        || actual_id.as_deref() != Some(plugin_id)
                        || profile.is_some_and(|saved| actual_profile.as_deref() != Some(saved))
                        || actual_cwd.as_ref() != cwd.as_ref()
                        || program.is_empty()
                    {
                        anyhow::bail!("plugin returned an ambiguous or mismatched terminal launch");
                    }
                    self.validate_plugin_launch_profile(plugin_id, actual_profile.as_deref())?;
                    // Requested cwd is authoritative, including restoration's
                    // missing-cwd fallback. PWD must not override it indirectly.
                    let env = env.into_iter().filter(|(key, _)| key != "PWD").collect();
                    launch = Some((
                        WorkspaceLaunch::Plugin {
                            plugin_id: plugin_id.into(),
                            profile: actual_profile,
                        },
                        crate::config::ShellConfig {
                            program: Some(program),
                            args,
                            env,
                        },
                    ));
                }
                PluginAction::ShowNotice { .. } => anyhow::bail!(
                    "plugin did not provide a clean terminal launch; configuration needs attention"
                ),
                _ => anyhow::bail!("plugin returned an unsupported terminal launch action"),
            }
        }
        launch.ok_or_else(|| anyhow::anyhow!("plugin {plugin_id:?} is not available"))
    }
}
