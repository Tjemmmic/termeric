//! Terminal events
//!
//! Events that can be emitted by the terminal for the application to handle.

/// Events emitted by the terminal
#[derive(Debug, Clone)]
pub enum TerminalEvent {
    /// The terminal title was changed (via OSC 0 or OSC 2)
    TitleChanged(String),

    /// A bell character was received
    Bell,

    /// The terminal requests clipboard access
    ClipboardRequest(ClipboardRequest),

    /// The terminal wants to write to clipboard
    ClipboardWrite(String),

    /// A hyperlink was activated
    HyperlinkActivated(String),

    /// The cursor visibility changed
    CursorVisibilityChanged(bool),

    /// Desktop notification request (OSC 9, OSC 777)
    Notification(NotificationRequest),

    /// Data that should be written back to the PTY (e.g., DSR responses)
    PtyWrite(Vec<u8>),

    /// The terminal exited
    Exit,
}

/// Desktop notification request
#[derive(Debug, Clone)]
pub struct NotificationRequest {
    /// Notification title
    pub title: Option<String>,
    /// Notification body/message
    pub body: String,
}

/// Clipboard request type
#[derive(Debug, Clone)]
pub enum ClipboardRequest {
    /// Request primary selection (X11)
    Primary,
    /// Request clipboard contents
    Clipboard,
}
