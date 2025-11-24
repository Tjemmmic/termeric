//! PTY events

/// Events from the PTY reader thread
#[derive(Debug, Clone)]
pub enum PtyEvent {
    /// Data received from the shell
    Output(Vec<u8>),

    /// The PTY was closed (shell exited)
    Closed,

    /// An error occurred reading from the PTY
    Error(String),
}
