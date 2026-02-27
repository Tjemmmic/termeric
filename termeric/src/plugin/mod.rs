//! Plugin system for Termeric
//!
//! Provides a trait-based plugin architecture that lets external tools
//! integrate with Termeric non-intrusively. Plugins can:
//!
//! - React to lifecycle hooks (tab creation, command execution, etc.)
//! - Add commands to the command palette
//! - Register keybindings
//! - Request actions (open tabs, write to PTY, show notifications)
//!
//! # Built-in Plugins
//!
//! - **Claude Code** (`claude_code`) — Launch Claude Code AI assistant

pub mod claude_code;
pub mod hooks;
pub mod manager;

// Re-export core types for convenience
pub use hooks::{Hook, HookResponse, Plugin, PluginAction, PluginCommand, PluginContext, PluginKeybinding};
pub use manager::PluginManager;
