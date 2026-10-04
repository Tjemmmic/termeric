//! Plugin manager — holds registered plugins and dispatches hooks.
//!
//! The [`PluginManager`] is the central coordinator for the plugin system.
//! It owns all plugin instances, dispatches lifecycle hooks, and collects
//! actions that plugins want the app to perform.

use super::hooks::{Hook, Plugin, PluginAction, PluginCommand, PluginContext};

/// Manages all registered plugins and dispatches hooks.
#[derive(Debug)]
pub struct PluginManager {
    plugins: Vec<Box<dyn Plugin>>,
}

impl PluginManager {
    pub(crate) fn pane_launch_profiles(&self) -> Vec<(String, String, Vec<String>)> {
        self.plugins
            .iter()
            .filter_map(|plugin| {
                plugin
                    .pane_profiles()
                    .map(|profiles| (plugin.id().into(), plugin.name().into(), profiles))
            })
            .collect()
    }
    /// Create a new empty plugin manager.
    pub fn new() -> Self {
        Self {
            plugins: Vec::new(),
        }
    }

    /// Register a plugin. A duplicate ID or a command outside the plugin's
    /// namespace, or one that collides with a registered command, rejects it.
    pub fn register(&mut self, plugin: Box<dyn Plugin>) -> anyhow::Result<()> {
        // Check for duplicate IDs
        let id = plugin.id().to_string();
        if self.plugins.iter().any(|p| p.id() == id) {
            anyhow::bail!("Plugin with id '{}' is already registered", id);
        }
        let commands = plugin.commands();
        if commands.len() > 64 {
            anyhow::bail!("Plugin {id:?} contributes more than 64 commands");
        }
        let mut command_ids = std::collections::BTreeSet::new();
        let existing: std::collections::BTreeSet<_> = self
            .plugins
            .iter()
            .flat_map(|loaded| loaded.commands())
            .map(|command| command.id)
            .collect();
        for command in commands {
            if !command.id.starts_with(&format!("{id}.")) {
                anyhow::bail!(
                    "Plugin {id:?} command {:?} is outside its namespace",
                    command.id
                );
            }
            if !command_ids.insert(command.id.clone()) || existing.contains(&command.id) {
                anyhow::bail!(
                    "Plugin command {:?} conflicts with an existing command",
                    command.id
                );
            }
        }

        log::info!(
            "Loaded plugin: {} v{} ({})",
            plugin.name(),
            plugin.version(),
            id
        );
        self.plugins.push(plugin);
        Ok(())
    }

    /// Dispatch a hook to all plugins, collecting any requested actions.
    ///
    /// Plugins are called in registration order. Each plugin can return
    /// one [`PluginAction`] which the caller (App) should execute.
    pub fn dispatch(&mut self, hook: &Hook, ctx: &PluginContext) -> Vec<PluginAction> {
        // Log hook-specific details for debugging
        match hook {
            Hook::TabCreated { tab_id } => log::trace!("Dispatching TabCreated(tab_id={})", tab_id),
            Hook::TabClosed { tab_id } => log::trace!("Dispatching TabClosed(tab_id={})", tab_id),
            Hook::TabSwitched { from, to } => {
                log::trace!("Dispatching TabSwitched(from={:?}, to={})", from, to)
            }
            Hook::CommandExecuted {
                region_id,
                tab_id,
                pane_id,
                exit_code,
                ..
            } => {
                log::trace!(
                    "Dispatching CommandExecuted(region={}, tab={}, pane={:?}, exit={:?})",
                    region_id,
                    tab_id,
                    pane_id,
                    exit_code
                );
            }
            _ => {}
        }
        self.plugins
            .iter_mut()
            .filter_map(|plugin| Self::dispatch_plugin(plugin.as_mut(), hook, ctx))
            .collect()
    }

    /// Dispatch a hook to one exact plugin ID.
    pub fn dispatch_to(
        &mut self,
        plugin_id: &str,
        hook: &Hook,
        ctx: &PluginContext,
    ) -> Option<PluginAction> {
        let plugin = self
            .plugins
            .iter_mut()
            .find(|plugin| plugin.id() == plugin_id)?;
        Self::dispatch_plugin(plugin.as_mut(), hook, ctx)
    }

    fn dispatch_plugin(
        plugin: &mut dyn Plugin,
        hook: &Hook,
        ctx: &PluginContext,
    ) -> Option<PluginAction> {
        let action = plugin.on_hook(hook, ctx)?;
        log::debug!(
            "Hook {} produced action {} from plugin '{}'",
            hook.name(),
            action.name(),
            plugin.id()
        );
        Some(action)
    }

    /// Get all commands from all registered plugins.
    pub fn all_commands(&self) -> Vec<PluginCommand> {
        self.plugins.iter().flat_map(|p| p.commands()).collect()
    }

    /// Execute a plugin command by ID.
    ///
    /// Dispatches a `CommandInvoked` hook only to the command namespace owner.
    pub fn execute_command(
        &mut self,
        command_id: &str,
        ctx: &PluginContext,
    ) -> Option<PluginAction> {
        let (plugin_id, _) = command_id.split_once('.')?;
        let hook = Hook::CommandInvoked {
            command_id: command_id.to_string(),
        };
        self.dispatch_to(plugin_id, &hook, ctx)
    }

    /// Remove every registered plugin.
    pub fn clear(&mut self) {
        self.plugins.clear();
    }

    /// Get the number of registered plugins.
    pub fn count(&self) -> usize {
        self.plugins.len()
    }

    /// Whether a plugin ID is currently registered.
    pub fn contains(&self, plugin_id: &str) -> bool {
        self.plugins.iter().any(|plugin| plugin.id() == plugin_id)
    }
}

impl Default for PluginManager {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A plugin that only contributes one palette command.
    #[derive(Debug)]
    struct CommandPlugin;

    impl Plugin for CommandPlugin {
        fn id(&self) -> &str {
            "commands"
        }
        fn name(&self) -> &str {
            "Command Plugin"
        }

        fn commands(&self) -> Vec<PluginCommand> {
            vec![PluginCommand {
                id: "commands.test".to_string(),
                name: "Test Command".to_string(),
                description: "A test command".to_string(),
                category: None,
            }]
        }
    }

    /// A plugin that returns actions on specific hooks
    #[derive(Debug)]
    struct ActionPlugin;

    impl Plugin for ActionPlugin {
        fn id(&self) -> &str {
            "action"
        }
        fn name(&self) -> &str {
            "Action Plugin"
        }

        fn on_hook(&mut self, hook: &Hook, _ctx: &PluginContext) -> Option<PluginAction> {
            match hook {
                Hook::CommandInvoked { command_id } if command_id == "action.open" => {
                    Some(PluginAction::OpenTab {
                        program: "echo".to_string(),
                        args: vec!["hello".to_string()],
                        cwd: None,
                        title: Some("Test Tab".to_string()),
                        plugin_id: None,
                        profile: None,
                        env: std::collections::HashMap::new(),
                    })
                }
                _ => None,
            }
        }

        fn commands(&self) -> Vec<PluginCommand> {
            vec![PluginCommand {
                id: "action.open".to_string(),
                name: "Open Test".to_string(),
                description: "Open a test tab".to_string(),
                category: Some("Test".to_string()),
            }]
        }
    }

    #[derive(Debug)]
    struct InvalidCommandPlugin {
        commands: Vec<PluginCommand>,
    }

    impl Plugin for InvalidCommandPlugin {
        fn id(&self) -> &str {
            "invalid"
        }

        fn name(&self) -> &str {
            "Invalid Commands"
        }

        fn commands(&self) -> Vec<PluginCommand> {
            self.commands.clone()
        }
    }

    #[derive(Debug)]
    struct EavesdroppingPlugin;

    impl Plugin for EavesdroppingPlugin {
        fn id(&self) -> &str {
            "eavesdrop"
        }

        fn name(&self) -> &str {
            "Eavesdropping Plugin"
        }

        fn on_hook(&mut self, hook: &Hook, _ctx: &PluginContext) -> Option<PluginAction> {
            if matches!(hook, Hook::CommandInvoked { .. }) {
                Some(PluginAction::ShowMessage {
                    message: "wrong owner received command".into(),
                })
            } else {
                None
            }
        }

        fn commands(&self) -> Vec<PluginCommand> {
            vec![PluginCommand {
                id: "eavesdrop.test".into(),
                name: "Eavesdrop".into(),
                description: String::new(),
                category: None,
            }]
        }
    }

    fn test_ctx() -> PluginContext {
        PluginContext {
            window_id: 1,
            cwd: None,
            tab_count: 1,
            active_tab: 0,
            active_tab_id: Some(1),
            terminal_size: (80, 24),
        }
    }

    #[test]
    fn test_duplicate_id_rejected() {
        let mut mgr = PluginManager::new();
        mgr.register(Box::new(CommandPlugin)).unwrap();
        assert!(mgr.register(Box::new(CommandPlugin)).is_err());
        assert_eq!(mgr.count(), 1);
    }

    #[test]
    fn conflicting_and_out_of_namespace_commands_are_rejected() {
        let command = |id: &str| PluginCommand {
            id: id.into(),
            name: "Conflict".into(),
            description: String::new(),
            category: None,
        };
        let mut manager = PluginManager::new();
        assert!(
            manager
                .register(Box::new(InvalidCommandPlugin {
                    commands: vec![command("other.command")],
                }))
                .is_err()
        );
        assert!(
            manager
                .register(Box::new(InvalidCommandPlugin {
                    commands: vec![command("invalid.same"), command("invalid.same")],
                }))
                .is_err()
        );
        assert_eq!(manager.count(), 0);
    }

    #[test]
    fn test_dispatch_collects_actions() {
        let mut mgr = PluginManager::new();
        let ctx = test_ctx();

        mgr.register(Box::new(ActionPlugin)).unwrap();
        mgr.register(Box::new(CommandPlugin)).unwrap();
        let actions = mgr.dispatch(
            &Hook::CommandInvoked {
                command_id: "action.open".to_string(),
            },
            &ctx,
        );
        assert_eq!(actions.len(), 1);
        assert!(matches!(&actions[0], PluginAction::OpenTab { program, .. } if program == "echo"));
    }

    #[test]
    fn test_all_commands() {
        let mut mgr = PluginManager::new();
        mgr.register(Box::new(CommandPlugin)).unwrap();
        mgr.register(Box::new(ActionPlugin)).unwrap();
        assert_eq!(mgr.all_commands().len(), 2);
    }

    #[test]
    fn test_execute_command() {
        let mut mgr = PluginManager::new();
        let ctx = test_ctx();

        mgr.register(Box::new(ActionPlugin)).unwrap();
        mgr.register(Box::new(EavesdroppingPlugin)).unwrap();

        assert!(matches!(
            mgr.execute_command("action.open", &ctx),
            Some(PluginAction::OpenTab { .. })
        ));
        assert!(matches!(
            mgr.execute_command("eavesdrop.test", &ctx),
            Some(PluginAction::ShowMessage { .. })
        ));
        assert!(mgr.execute_command("nonexistent", &ctx).is_none());
    }
}
