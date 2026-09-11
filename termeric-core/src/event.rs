//! Terminal events
//!
//! Events that can be emitted by the terminal for the application to handle.

/// Events emitted by the terminal
use crate::Color;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TerminalEvent {
    /// The terminal title was changed (via OSC 0 or OSC 2)
    TitleChanged(String),

    /// A bell character was received
    Bell,

    /// The terminal requests clipboard access. Reply framing is captured when
    /// the request is parsed so a later S7C1T/S8C1T cannot reorder semantics.
    ClipboardRequest {
        selection: ClipboardRequest,
        reply_uses_8bit_controls: bool,
    },

    /// The terminal wants to write to clipboard
    ClipboardWrite {
        selection: ClipboardRequest,
        text: String,
    },

    /// A terminal-local xterm dynamic color operation.
    DynamicColor(DynamicColorOperation),

    /// A hyperlink was activated
    HyperlinkActivated(String),

    /// The cursor visibility changed
    CursorVisibilityChanged(bool),

    /// The active screen's OSC 22 mouse pointer shape changed.
    PointerShapeChanged,

    /// Desktop notification request (OSC 9, OSC 99, or OSC 777)
    Notification(NotificationRequest),

    /// Data that should be written back to the PTY (e.g., DSR responses)
    PtyWrite(Vec<u8>),

    /// The terminal exited
    Exit,
}

/// Desktop notification urgency from Kitty OSC 99.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NotificationUrgency {
    /// Low-priority information.
    Low,
    /// Normal-priority information.
    Normal,
    /// Critical information that should be emphasized by the desktop.
    Critical,
}

/// Desktop notification request
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct NotificationRequest {
    /// Notification title. Kitty OSC 99 body-only requests promote their body
    /// here as required by that protocol.
    pub title: Option<String>,
    /// Notification body/message
    pub body: String,
    /// Application identity supplied by Kitty OSC 99 `f` metadata.
    pub application_name: Option<String>,
    /// Ordered notification categories supplied by Kitty OSC 99 `t` metadata.
    pub notification_types: Vec<String>,
    /// Ordered icon-name candidates supplied by Kitty OSC 99 `n` metadata.
    pub icon_names: Vec<String>,
    /// Optional Kitty OSC 99 urgency. Absence retains the desktop default.
    pub urgency: Option<NotificationUrgency>,
}

/// Clipboard request type
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClipboardRequest {
    /// Request primary selection (X11)
    Primary,
    /// Request clipboard contents
    Clipboard,
}

impl ClipboardRequest {
    pub const fn osc_code(self) -> char {
        match self {
            Self::Primary => 'p',
            Self::Clipboard => 'c',
        }
    }
}

/// Addressable xterm palette/default color slots.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DynamicColorTarget {
    Palette(u8),
    Foreground,
    Background,
    Cursor,
}

/// Typed OSC 4/10/11/12/104/110/111/112 operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DynamicColorOperation {
    Set {
        target: DynamicColorTarget,
        color: Color,
    },
    Query {
        target: DynamicColorTarget,
        reply_uses_8bit_controls: bool,
    },
    Reset(DynamicColorTarget),
    ResetPalette,
}
