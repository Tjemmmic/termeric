//! Launch integration for one coding agent from the agent catalog.
//!
//! Each enabled agent registers as a built-in plugin so terminal launches,
//! split panes, workspace restoration, and restarts share the same checked
//! launch path as other plugin terminals. It adds no palette commands: the
//! application offers agents through one picker instead of a copy of every
//! command per agent.

use super::hooks::{Hook, HookResponse, Plugin, PluginAction, PluginContext};
use crate::agents::AgentDefinition;

#[derive(Debug)]
pub struct AgentPlugin {
    agent: AgentDefinition,
}

impl AgentPlugin {
    pub fn new(agent: AgentDefinition) -> Self {
        Self { agent }
    }

    /// The command that opens this agent in a new tab. Invoked by the
    /// application's agent picker, never listed in the palette.
    pub fn open_command(agent_id: &str) -> String {
        format!("{agent_id}.open")
    }

    /// The command that opens this agent resuming its latest conversation.
    pub fn resume_command(agent_id: &str) -> String {
        format!("{agent_id}.resume")
    }

    fn launch(
        &self,
        ctx: &PluginContext,
        profile: Option<&str>,
        resume: bool,
        cwd: Option<&str>,
    ) -> HookResponse {
        match open_tab_action(&self.agent, ctx, profile, resume, cwd) {
            Ok(action) => HookResponse::Action(action),
            Err(error) => HookResponse::Action(PluginAction::ShowNotice {
                title: format!("{} could not start", self.agent.name),
                lines: vec![error],
            }),
        }
    }
}

/// The tab launch for `agent`: its program, arguments for `profile` (and
/// resuming), environment including the terminal size, and working
/// directory — `cwd` when given, else the context's when the agent inherits
/// it.
pub fn open_tab_action(
    agent: &AgentDefinition,
    ctx: &PluginContext,
    profile: Option<&str>,
    resume: bool,
    cwd: Option<&str>,
) -> Result<PluginAction, String> {
    let launch = agent.launch(profile, resume)?;
    let cwd = cwd
        .map(str::to_owned)
        .or_else(|| agent.inherit_cwd.then(|| ctx.cwd.clone()).flatten());
    let mut env: std::collections::HashMap<String, String> = launch.env.into_iter().collect();
    env.insert("COLUMNS".into(), ctx.terminal_size.0.to_string());
    env.insert("LINES".into(), ctx.terminal_size.1.to_string());
    Ok(PluginAction::OpenTab {
        program: launch.program,
        args: launch.args,
        cwd,
        title: Some(agent.name.clone()),
        plugin_id: Some(agent.id.clone()),
        profile: launch.profile,
        env,
    })
}

impl Plugin for AgentPlugin {
    fn id(&self) -> &str {
        &self.agent.id
    }

    fn name(&self) -> &str {
        &self.agent.name
    }

    fn on_hook(&mut self, hook: &Hook, ctx: &PluginContext) -> HookResponse {
        match hook {
            Hook::CommandInvoked { command_id } if *command_id == Self::open_command(self.id()) => {
                self.launch(ctx, None, false, None)
            }
            Hook::CommandInvoked { command_id }
                if *command_id == Self::resume_command(self.id()) =>
            {
                self.launch(ctx, None, true, None)
            }
            Hook::PaneLaunch {
                plugin_id,
                profile,
                cwd,
            }
            | Hook::WorkspaceRestore {
                plugin_id,
                profile,
                cwd,
            } if *plugin_id == self.agent.id => {
                self.launch(ctx, profile.as_deref(), false, cwd.as_deref())
            }
            _ => HookResponse::Ignored,
        }
    }

    fn pane_profiles(&self) -> Option<Vec<String>> {
        Some(self.agent.profiles.keys().cloned().collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agents::AgentCatalog;

    fn context() -> PluginContext {
        PluginContext {
            cwd: Some("/work".into()),
            tab_count: 1,
            active_tab: 0,
            terminal_size: (120, 40),
        }
    }

    fn open_tab(response: HookResponse) -> (String, Vec<String>, Option<String>, Option<String>) {
        let HookResponse::Action(PluginAction::OpenTab {
            program,
            args,
            cwd,
            profile,
            plugin_id,
            env,
            ..
        }) = response
        else {
            panic!("expected a launch, got {response:?}");
        };
        assert_eq!(env.get("COLUMNS").map(String::as_str), Some("120"));
        assert!(plugin_id.is_some());
        (program, args, cwd, profile)
    }

    #[test]
    fn launches_follow_the_catalog_and_restores_keep_their_directory() {
        let configured = toml::from_str(
            r#"
            [codex]
            args = ["--ask-for-approval", "never"]
            [codex.profiles.safe]
            args = ["--sandbox", "read-only"]
            "#,
        )
        .unwrap();
        let catalog = AgentCatalog::from_config(&configured);
        let mut plugin = AgentPlugin::new(catalog.get("codex").unwrap().clone());
        assert!(
            plugin.commands().is_empty(),
            "no per-agent palette commands"
        );

        let (program, args, cwd, profile) = open_tab(plugin.on_hook(
            &Hook::CommandInvoked {
                command_id: AgentPlugin::resume_command("codex"),
            },
            &context(),
        ));
        assert_eq!(program, "codex");
        assert_eq!(args, ["--ask-for-approval", "never", "resume", "--last"]);
        assert_eq!(cwd.as_deref(), Some("/work"));
        assert_eq!(profile, None);

        let (_, args, cwd, profile) = open_tab(plugin.on_hook(
            &Hook::WorkspaceRestore {
                plugin_id: "codex".into(),
                profile: Some("safe".into()),
                cwd: Some("/restored".into()),
            },
            &context(),
        ));
        assert_eq!(
            args,
            ["--ask-for-approval", "never", "--sandbox", "read-only"]
        );
        assert_eq!(cwd.as_deref(), Some("/restored"));
        assert_eq!(profile.as_deref(), Some("safe"));

        let missing = plugin.on_hook(
            &Hook::PaneLaunch {
                plugin_id: "codex".into(),
                profile: Some("gone".into()),
                cwd: None,
            },
            &context(),
        );
        assert!(matches!(
            missing,
            HookResponse::Action(PluginAction::ShowNotice { .. })
        ));
        assert!(matches!(
            plugin.on_hook(
                &Hook::PaneLaunch {
                    plugin_id: "claude-code".into(),
                    profile: None,
                    cwd: None
                },
                &context()
            ),
            HookResponse::Ignored
        ));
    }
}
