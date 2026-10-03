//! Plugin system for Termeric
//!
//! Every plugin implements the [`hooks::Plugin`] trait: it reacts to
//! lifecycle hooks (tab creation, command completion, and so on), may add
//! command-palette commands and pane launch profiles, and answers a hook with
//! at most one [`PluginAction`] (open a tab, show a message or notice, or run
//! a structured handler).
//!
//! - **Agents** (`agent`) — one built-in launch integration per coding agent
//!   in the agent catalog, including configured custom agents
//! - **Third-party plugins** (`runtime`) — installed declarative manifests;
//!   `structured` runs their handler processes and `permission` holds their
//!   capabilities

pub mod agent;
pub mod hooks;
pub mod manager;
pub mod permission;
pub mod runtime;
pub mod structured;

// Re-export core types for convenience
pub use hooks::{Hook, PluginAction, PluginContext};
pub use manager::PluginManager;
