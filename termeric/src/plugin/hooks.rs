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
        /// Stable terminal-local OSC 133 region identity.
        region_id: u64,
        /// Stable application-local tab identity.
        tab_id: usize,
        /// Stable pane identity when the tab is split.
        pane_id: Option<u32>,
        /// Bounded command-region text. Empty when backing rows were evicted.
        command: String,
        /// Whether command text was truncated at the extraction boundary.
        command_truncated: bool,
        /// Bounded output-region text. Empty when backing rows were evicted.
        output: String,
        /// Whether output text was truncated at the extraction boundary.
        output_truncated: bool,
        /// Whether the completed region still had authoritative grid content.
        content_available: bool,
        exit_code: Option<i32>,
    },
    /// A plugin command was invoked (from command palette or keybinding)
    CommandInvoked { command_id: String },
    /// Resolve a fresh plugin-owned pane requested explicitly by the user.
    /// The host validates and places the returned launch without executing
    /// unrelated actions or emitting a tab-created event.
    PaneLaunch {
        plugin_id: String,
        profile: Option<String>,
        cwd: Option<String>,
    },
    /// Recreate a plugin-owned tab from a persisted workspace. The named
    /// profile is an explicit archive reference and therefore takes priority
    /// over repository and default profile selection.
    WorkspaceRestore {
        plugin_id: String,
        profile: Option<String>,
        cwd: Option<String>,
    },
}

impl Hook {
    /// Payload-free name suitable for diagnostics. Hook payloads can contain
    /// terminal text or command data and must never be formatted into logs.
    pub fn name(&self) -> &'static str {
        match self {
            Self::AppReady => "app-ready",
            Self::TabCreated { .. } => "tab-created",
            Self::TabClosed { .. } => "tab-closed",
            Self::TabSwitched { .. } => "tab-switched",
            Self::CommandExecuted { .. } => "command-executed",
            Self::CommandInvoked { .. } => "command-invoked",
            Self::PaneLaunch { .. } => "pane-launch",
            Self::WorkspaceRestore { .. } => "workspace-restore",
        }
    }
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
    /// Plugin wants to execute several actions (notice + open, etc.)
    Actions(Vec<PluginAction>),
}

/// Actions a plugin can request the app to perform.
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub enum PluginAction {
    /// Invoke a versioned declarative lifecycle handler on the bounded
    /// background plugin worker.
    InvokeStructuredHandler {
        invocation: crate::plugin::structured::StructuredHandlerInvocation,
    },
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
        /// Resolved named profile, when the launch used one.
        profile: Option<String>,
        /// Additional environment variables to set for the spawned process
        env: HashMap<String, String>,
    },
    /// Write data to the active PTY
    WriteToPty { data: Vec<u8> },
    /// Show a desktop notification
    Notify { title: String, body: String },
    /// Show a message in the command palette area
    ShowMessage { message: String },
    /// Show a modal info overlay (title + body lines). Use for anything the
    /// user needs to actually see — failed profile lookups, config warnings,
    /// etc.
    ShowNotice { title: String, lines: Vec<String> },
    /// Clear a plugin-owned tab (send Ctrl+C + clear)
    ClearPluginTab { plugin_id: String },
    /// Launch a fresh replacement, then retire only the selected plugin owner.
    RestartPlugin { plugin_id: String },
    /// Copy N lines from a plugin tab to clipboard
    CopyFromPluginTab { plugin_id: String, lines: usize },
    /// Apply a detected diff to the filesystem via `git apply`
    ApplyDiff {
        diff_content: String,
        cwd: Option<String>,
    },
    /// Stage a detected diff via `git apply --cached`
    StageDiff {
        diff_content: String,
        cwd: Option<String>,
    },
    /// Revert a detected diff via `git apply --reverse`
    RevertDiff {
        diff_content: String,
        cwd: Option<String>,
    },
    /// Toggle transcript recording for the active AI plugin session.
    ToggleTranscript,
    /// Inject context text into a specific plugin-owned tab.
    InjectContext { plugin_id: String, text: String },
    /// List available profiles for a plugin (shows selection overlay)
    ListProfiles { plugin_id: String },
    /// Apply a named profile to a specific plugin session.
    ApplyProfile {
        plugin_id: String,
        profile_name: String,
    },
}

impl PluginAction {
    /// Payload-free diagnostic name. Action payloads can contain terminal
    /// text, environment values, context, or diffs and must not enter logs.
    pub fn name(&self) -> &'static str {
        match self {
            Self::InvokeStructuredHandler { .. } => "invoke-structured-handler",
            Self::OpenTab { .. } => "open-tab",
            Self::WriteToPty { .. } => "write-to-pty",
            Self::Notify { .. } => "notify",
            Self::ShowMessage { .. } => "show-message",
            Self::ShowNotice { .. } => "show-notice",
            Self::ClearPluginTab { .. } => "clear-plugin-tab",
            Self::RestartPlugin { .. } => "restart-plugin",
            Self::CopyFromPluginTab { .. } => "copy-from-plugin-tab",
            Self::ApplyDiff { .. } => "apply-diff",
            Self::StageDiff { .. } => "stage-diff",
            Self::RevertDiff { .. } => "revert-diff",
            Self::ToggleTranscript => "toggle-transcript",
            Self::InjectContext { .. } => "inject-context",
            Self::ListProfiles { .. } => "list-profiles",
            Self::ApplyProfile { .. } => "apply-profile",
        }
    }
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

    /// Opt into explicit local pane creation. Names refer to launch profiles,
    /// not running sessions. None keeps event-only plugins out of this UI.
    fn pane_profiles(&self) -> Option<Vec<String>> {
        None
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
        let hooks = [
            Hook::AppReady,
            Hook::TabCreated { tab_id: 1 },
            Hook::TabClosed { tab_id: 1 },
            Hook::TabSwitched { from: 0, to: 1 },
            Hook::CommandExecuted {
                region_id: 1,
                tab_id: 1,
                pane_id: None,
                command: "ls".to_string(),
                command_truncated: false,
                output: String::new(),
                output_truncated: false,
                content_available: true,
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
        let actions = [
            PluginAction::OpenTab {
                program: "claude".to_string(),
                args: vec![],
                cwd: None,
                title: Some("Test".to_string()),
                plugin_id: None,
                profile: None,
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
