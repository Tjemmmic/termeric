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

/// The longest ID a plugin registers under.
pub const MAX_PLUGIN_ID_BYTES: usize = 64;

/// Checks an ID that names a registered plugin, as a launch does: a catalog
/// agent (`[agents.<id>]`) or a third-party plugin. Third-party plugins take
/// a stricter form of their own (`runtime::validate_third_party_identifier`),
/// so this is the catalog agents' rule.
pub fn validate_plugin_id(label: &str, value: &str) -> anyhow::Result<()> {
    if value.is_empty()
        || value.len() > MAX_PLUGIN_ID_BYTES
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    {
        anyhow::bail!(
            "{label} {value:?} must be 1-{MAX_PLUGIN_ID_BYTES} ASCII letters, digits, '-' or '_'"
        );
    }
    Ok(())
}
