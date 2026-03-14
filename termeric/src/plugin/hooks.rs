//! Plugin trait and hook/event types
//!
//! Defines the core [`Plugin`] trait that all plugins implement,
//! along with lifecycle hooks and response types.

use std::collections::HashMap;

/// Read-only context passed to plugins during hook dispatch.
#[derive(Debug, Clone)]
pub struct PluginContext {
    /// Current working directory of the active terminal
    pub cwd: Option<String>,
    /// Number of open tabs
    pub tab_count: usize,
    /// Active tab index
    pub active_tab: usize,
    /// Terminal dimensions (cols, rows)
    pub terminal_size: (usize, usize),
}

/// Lifecycle events that plugins can react to.
#[derive(Debug, Clone)]
pub enum Hook {
    /// App has fully initialized (window, renderer, tabs ready)
    AppReady,
    /// A new tab was created
    TabCreated { tab_id: usize },
    /// A tab was closed
    TabClosed { tab_id: usize },
    /// The active tab changed
    TabSwitched { from: usize, to: usize },
    /// A command was executed in the terminal (from shell integration)
    CommandExecuted {
        command: String,
        exit_code: Option<i32>,
    },
    /// A plugin command was invoked (from command palette or keybinding)
    CommandInvoked { command_id: String },
}

/// What a plugin returns after processing a hook.
#[derive(Debug)]
pub enum HookResponse {
    /// Plugin didn't handle this hook
    Ignored,
    /// Plugin handled it, continue processing other plugins
    Handled,
    /// Plugin wants to execute an action
    Action(PluginAction),
}

/// Actions a plugin can request the app to perform.
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub enum PluginAction {
    /// Open a new tab running a specific program (no shell echo)
    OpenTab {
        /// Program to run (e.g., "claude")
        program: String,
        /// Arguments to pass to the program
        args: Vec<String>,
        /// Working directory (None = inherit from active terminal)
        cwd: Option<String>,
        /// Tab title (None = use program name)
        title: Option<String>,
        /// Plugin ID that owns this tab (None = generic command tab)
        plugin_id: Option<String>,
        /// Additional environment variables to set for the spawned process
        env: HashMap<String, String>,
    },
    /// Write data to the active PTY
    WriteToPty { data: Vec<u8> },
    /// Show a desktop notification
    Notify { title: String, body: String },
    /// Show a message in the command palette area
    ShowMessage { message: String },
    /// Clear a plugin-owned tab (send Ctrl+C + clear)
    ClearPluginTab { plugin_id: String },
    /// Close all tabs of a plugin and reopen fresh
    RestartPlugin { plugin_id: String },
    /// Copy N lines from a plugin tab to clipboard
    CopyFromPluginTab { plugin_id: String, lines: usize },
    /// Apply a detected diff to the filesystem via `git apply`
    ApplyDiff { diff_content: String, cwd: Option<String> },
    /// Stage a detected diff via `git apply --cached`
    StageDiff { diff_content: String, cwd: Option<String> },
    /// Revert a detected diff via `git apply --reverse`
    RevertDiff { diff_content: String, cwd: Option<String> },
    /// Toggle transcript recording for the active Claude session
    ToggleTranscript,
    /// Inject context text into the active plugin tab
    InjectContext { text: String },
    /// List available profiles for a plugin (shows selection overlay)
    ListProfiles { plugin_id: String },
    /// Apply a named profile to the active plugin session
    ApplyProfile { plugin_id: String, profile_name: String },
}

/// Metadata for a command a plugin contributes to the command palette.
#[derive(Debug, Clone)]
pub struct PluginCommand {
    /// Unique command identifier (e.g., "claude-code.open")
    pub id: String,
    /// Display name in command palette (e.g., "Open Claude Code")
    pub name: String,
    /// Description shown in command palette
    pub description: String,
    /// Category for grouping (e.g., "AI")
    pub category: Option<String>,
}

/// A keybinding a plugin wants to register.
#[derive(Debug, Clone)]
pub struct PluginKeybinding {
    /// Key string (e.g., "i")
    pub key: String,
    /// Ctrl modifier required
    pub ctrl: bool,
    /// Alt modifier required
    pub alt: bool,
    /// Shift modifier required
    pub shift: bool,
    /// The command ID to trigger when this keybinding fires
    pub command_id: String,
}

/// The core plugin trait. Every plugin implements this.
///
/// All methods have default no-op implementations so plugins only need
/// to override what they care about.
pub trait Plugin: std::fmt::Debug + Send {
    /// Unique identifier (e.g., "claude-code")
    fn id(&self) -> &str;

    /// Human-readable name (e.g., "Claude Code")
    fn name(&self) -> &str;

    /// Version string
    fn version(&self) -> &str {
        "0.1.0"
    }

    /// Called once when the plugin is loaded.
    fn on_load(&mut self, _ctx: &PluginContext) -> anyhow::Result<()> {
        Ok(())
    }

    /// Called when the plugin is unloaded.
    fn on_unload(&mut self) {}

    /// Respond to a hook event. Return [`HookResponse`] to indicate behavior.
    fn on_hook(&mut self, _hook: &Hook, _ctx: &PluginContext) -> HookResponse {
        HookResponse::Ignored
    }

    /// Commands this plugin adds to the command palette.
    fn commands(&self) -> Vec<PluginCommand> {
        vec![]
    }

    /// Keybindings this plugin wants to register.
    fn keybindings(&self) -> Vec<PluginKeybinding> {
        vec![]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A minimal test plugin for verifying trait defaults
    #[derive(Debug)]
    struct TestPlugin;

    impl Plugin for TestPlugin {
        fn id(&self) -> &str {
            "test"
        }
        fn name(&self) -> &str {
            "Test Plugin"
        }
    }

    #[test]
    fn test_default_trait_methods() {
        let mut plugin = TestPlugin;
        assert_eq!(plugin.version(), "0.1.0");
        assert!(plugin.commands().is_empty());
        assert!(plugin.keybindings().is_empty());

        let ctx = PluginContext {
            cwd: None,
            tab_count: 1,
            active_tab: 0,
            terminal_size: (80, 24),
        };
        assert!(plugin.on_load(&ctx).is_ok());
        assert!(matches!(
            plugin.on_hook(&Hook::AppReady, &ctx),
            HookResponse::Ignored
        ));
        plugin.on_unload(); // Should not panic
    }

    #[test]
    fn test_hook_variants() {
        let hooks = vec![
            Hook::AppReady,
            Hook::TabCreated { tab_id: 1 },
            Hook::TabClosed { tab_id: 1 },
            Hook::TabSwitched { from: 0, to: 1 },
            Hook::CommandExecuted {
                command: "ls".to_string(),
                exit_code: Some(0),
            },
            Hook::CommandInvoked {
                command_id: "test.cmd".to_string(),
            },
        ];
        // All variants should be constructible (compile-time check)
        assert_eq!(hooks.len(), 6);
    }

    #[test]
    fn test_plugin_action_variants() {
        let actions = vec![
            PluginAction::OpenTab {
                program: "claude".to_string(),
                args: vec![],
                cwd: None,
                title: Some("Test".to_string()),
                plugin_id: None,
                env: HashMap::new(),
            },
            PluginAction::WriteToPty {
                data: b"hello".to_vec(),
            },
            PluginAction::Notify {
                title: "Title".to_string(),
                body: "Body".to_string(),
            },
            PluginAction::ShowMessage {
                message: "msg".to_string(),
            },
        ];
        assert_eq!(actions.len(), 4);
    }
}
