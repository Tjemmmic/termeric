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

pub mod ai_common;
pub mod claude_code;
pub mod claude_profiles;
pub mod codex;
pub mod codex_profiles;
pub mod hooks;
pub mod manager;
pub mod runtime;
pub mod structured;

// Re-export core types for convenience
pub use hooks::{Hook, PluginAction, PluginContext};
pub use manager::PluginManager;
