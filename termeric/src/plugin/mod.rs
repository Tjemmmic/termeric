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
//! - **Agents** (`agent`) — one launch integration per coding agent in the
//!   agent catalog (Claude Code, Codex, Gemini CLI, OpenCode, and configured
//!   custom agents)

pub mod agent;
pub mod hooks;
pub mod manager;
pub mod permission;
pub mod runtime;
pub mod structured;

// Re-export core types for convenience
pub use hooks::{Hook, PluginAction, PluginContext};
pub use manager::PluginManager;
