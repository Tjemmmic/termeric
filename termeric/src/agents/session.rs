//! The agent running in one terminal, as last observed.

/// Which agent a terminal is running and how it got there. Refreshed from
/// the terminal's foreground process, so it follows the terminal when its
/// pane moves and clears when the agent exits back to the shell.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentSession {
    pub agent_id: String,
    /// Termeric started this terminal as the agent (it can be restarted).
    /// Otherwise the agent was started from a shell in this terminal.
    pub launched: bool,
    /// Profile of a launched agent.
    pub profile: Option<String>,
    /// The agent's process: its foreground process group, or for a launched
    /// agent the terminal's own child. `None` where the platform cannot tell.
    pub process: Option<i64>,
}
