//! PTY events

/// Events from the PTY reader thread
#[derive(Debug, Clone)]
pub enum PtyEvent {
    /// Data received from the shell
    Output(Vec<u8>),

    /// The PTY output stream reached EOF. Child exit is observed separately;
    /// a child can close its streams while still running.
    Closed,

    /// An error occurred reading from the PTY
    Error(String),
}
