//! Plugin trait, hooks, and actions
//!
//! Defines the core [`Plugin`] trait that all plugins implement, the
//! lifecycle hooks it receives, and the actions it may answer with.

use std::collections::HashMap;

/// Read-only context passed to plugins during hook dispatch.
#[derive(Debug, Clone)]
pub struct PluginContext {
    /// Stable process-local ID of the window dispatching the hook: the origin
    /// window of any handler conversation the hook starts.
    pub window_id: u64,
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

/// An action a plugin asks the app to perform in answer to a hook.
#[derive(Debug, Clone)]
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
    /// Show a brief informational toast
    ShowMessage { message: String },
    /// Show a modal info overlay (title + body lines). Use for anything the
    /// user needs to actually see — failed profile lookups, config warnings,
    /// etc.
    ShowNotice { title: String, lines: Vec<String> },
}

impl PluginAction {
    /// Payload-free diagnostic name. Action payloads can contain terminal
    /// text, environment values, or context and must not enter logs.
    pub fn name(&self) -> &'static str {
        match self {
            Self::InvokeStructuredHandler { .. } => "invoke-structured-handler",
            Self::OpenTab { .. } => "open-tab",
            Self::ShowMessage { .. } => "show-message",
            Self::ShowNotice { .. } => "show-notice",
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

    /// Respond to a hook event with at most one action; `None` ignores it.
    fn on_hook(&mut self, _hook: &Hook, _ctx: &PluginContext) -> Option<PluginAction> {
        None
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
}
