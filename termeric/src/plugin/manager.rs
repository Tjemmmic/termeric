//! Plugin manager — holds registered plugins and dispatches hooks.
//!
//! The [`PluginManager`] is the central coordinator for the plugin system.
//! It owns all plugin instances, dispatches lifecycle hooks, and collects
//! actions that plugins want the app to perform.

use super::hooks::{Hook, HookResponse, Plugin, PluginAction, PluginCommand, PluginContext, PluginKeybinding};

/// Manages all registered plugins and dispatches hooks.
#[derive(Debug)]
pub struct PluginManager {
    plugins: Vec<Box<dyn Plugin>>,
}

impl PluginManager {
    /// Create a new empty plugin manager.
    pub fn new() -> Self {
        Self {
            plugins: Vec::new(),
        }
    }

    /// Register and load a plugin.
    ///
    /// Calls `on_load` on the plugin. If loading fails, the plugin
    /// is not added and the error is returned.
    pub fn register(
        &mut self,
        mut plugin: Box<dyn Plugin>,
        ctx: &PluginContext,
    ) -> anyhow::Result<()> {
        // Check for duplicate IDs
        let id = plugin.id().to_string();
        if self.plugins.iter().any(|p| p.id() == id) {
            anyhow::bail!("Plugin with id '{}' is already registered", id);
        }

        plugin.on_load(ctx)?;
        log::info!("Loaded plugin: {} v{} ({})", plugin.name(), plugin.version(), id);
        self.plugins.push(plugin);
        Ok(())
    }

    /// Dispatch a hook to all plugins, collecting any requested actions.
    ///
    /// Plugins are called in registration order. Each plugin can return
    /// a [`PluginAction`] which the caller (App) should execute.
    pub fn dispatch(&mut self, hook: &Hook, ctx: &PluginContext) -> Vec<PluginAction> {
        let mut actions = Vec::new();

        for plugin in &mut self.plugins {
            match plugin.on_hook(hook, ctx) {
                HookResponse::Ignored => {}
                HookResponse::Handled => {
                    log::debug!("Hook {:?} handled by plugin '{}'", hook, plugin.id());
                }
                HookResponse::Action(action) => {
                    log::debug!(
                        "Hook {:?} produced action {:?} from plugin '{}'",
                        hook,
                        action,
                        plugin.id()
                    );
                    actions.push(action);
                }
            }
        }

        actions
    }

    /// Get all commands from all registered plugins.
    pub fn all_commands(&self) -> Vec<PluginCommand> {
        self.plugins.iter().flat_map(|p| p.commands()).collect()
    }

    /// Get all keybindings from all registered plugins.
    pub fn all_keybindings(&self) -> Vec<PluginKeybinding> {
        self.plugins
            .iter()
            .flat_map(|p| p.keybindings())
            .collect()
    }

    /// Execute a plugin command by ID.
    ///
    /// Dispatches a `CommandInvoked` hook and returns the first action
    /// produced, if any.
    pub fn execute_command(
        &mut self,
        command_id: &str,
        ctx: &PluginContext,
    ) -> Option<PluginAction> {
        let hook = Hook::CommandInvoked {
            command_id: command_id.to_string(),
        };
        let actions = self.dispatch(&hook, ctx);
        actions.into_iter().next()
    }

    /// Unload all plugins (calls `on_unload` on each).
    pub fn unload_all(&mut self) {
        for plugin in &mut self.plugins {
            log::info!("Unloading plugin: {}", plugin.id());
            plugin.on_unload();
        }
        self.plugins.clear();
    }

    /// Get the number of registered plugins.
    pub fn count(&self) -> usize {
        self.plugins.len()
    }
}

impl Default for PluginManager {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for PluginManager {
    fn drop(&mut self) {
        self.unload_all();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plugin::hooks::HookResponse;

    /// A test plugin that tracks which hooks it received
    #[derive(Debug)]
    struct TrackingPlugin {
        hooks_received: Vec<String>,
    }

    impl TrackingPlugin {
        fn new() -> Self {
            Self {
                hooks_received: Vec::new(),
            }
        }
    }

    impl Plugin for TrackingPlugin {
        fn id(&self) -> &str {
            "tracking"
        }
        fn name(&self) -> &str {
            "Tracking Plugin"
        }

        fn on_hook(&mut self, hook: &Hook, _ctx: &PluginContext) -> HookResponse {
            self.hooks_received.push(format!("{:?}", hook));
            HookResponse::Handled
        }

        fn commands(&self) -> Vec<PluginCommand> {
            vec![PluginCommand {
                id: "tracking.test".to_string(),
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

        fn on_hook(&mut self, hook: &Hook, _ctx: &PluginContext) -> HookResponse {
            match hook {
                Hook::CommandInvoked { command_id } if command_id == "action.open" => {
                    HookResponse::Action(PluginAction::OpenTab {
                        program: "echo".to_string(),
                        args: vec!["hello".to_string()],
                        cwd: None,
                        title: Some("Test Tab".to_string()),
                        plugin_id: None,
                        env: std::collections::HashMap::new(),
                    })
                }
                _ => HookResponse::Ignored,
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

        fn keybindings(&self) -> Vec<PluginKeybinding> {
            vec![PluginKeybinding {
                key: "t".to_string(),
                ctrl: true,
                alt: false,
                shift: true,
                command_id: "action.open".to_string(),
            }]
        }
    }

    fn test_ctx() -> PluginContext {
        PluginContext {
            cwd: None,
            tab_count: 1,
            active_tab: 0,
            terminal_size: (80, 24),
        }
    }

    #[test]
    fn test_register_and_count() {
        let mut mgr = PluginManager::new();
        let ctx = test_ctx();

        assert_eq!(mgr.count(), 0);
        mgr.register(Box::new(TrackingPlugin::new()), &ctx).unwrap();
        assert_eq!(mgr.count(), 1);
    }

    #[test]
    fn test_duplicate_id_rejected() {
        let mut mgr = PluginManager::new();
        let ctx = test_ctx();

        mgr.register(Box::new(TrackingPlugin::new()), &ctx).unwrap();
        let result = mgr.register(Box::new(TrackingPlugin::new()), &ctx);
        assert!(result.is_err());
        assert_eq!(mgr.count(), 1);
    }

    #[test]
    fn test_dispatch_hook() {
        let mut mgr = PluginManager::new();
        let ctx = test_ctx();

        mgr.register(Box::new(TrackingPlugin::new()), &ctx).unwrap();
        let actions = mgr.dispatch(&Hook::AppReady, &ctx);
        assert!(actions.is_empty()); // TrackingPlugin returns Handled, not Action
    }

    #[test]
    fn test_dispatch_collects_actions() {
        let mut mgr = PluginManager::new();
        let ctx = test_ctx();

        mgr.register(Box::new(ActionPlugin), &ctx).unwrap();
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
        let ctx = test_ctx();

        mgr.register(Box::new(TrackingPlugin::new()), &ctx).unwrap();
        mgr.register(Box::new(ActionPlugin), &ctx).unwrap();

        let commands = mgr.all_commands();
        assert_eq!(commands.len(), 2);
    }

    #[test]
    fn test_all_keybindings() {
        let mut mgr = PluginManager::new();
        let ctx = test_ctx();

        mgr.register(Box::new(ActionPlugin), &ctx).unwrap();

        let bindings = mgr.all_keybindings();
        assert_eq!(bindings.len(), 1);
        assert_eq!(bindings[0].key, "t");
        assert!(bindings[0].ctrl);
    }

    #[test]
    fn test_execute_command() {
        let mut mgr = PluginManager::new();
        let ctx = test_ctx();

        mgr.register(Box::new(ActionPlugin), &ctx).unwrap();

        let action = mgr.execute_command("action.open", &ctx);
        assert!(action.is_some());

        let action = mgr.execute_command("nonexistent", &ctx);
        assert!(action.is_none());
    }

    #[test]
    fn test_unload_all() {
        let mut mgr = PluginManager::new();
        let ctx = test_ctx();

        mgr.register(Box::new(TrackingPlugin::new()), &ctx).unwrap();
        mgr.register(Box::new(ActionPlugin), &ctx).unwrap();
        assert_eq!(mgr.count(), 2);

        mgr.unload_all();
        assert_eq!(mgr.count(), 0);
    }
}
