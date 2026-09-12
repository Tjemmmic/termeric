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

    /// Close one exact identified Kitty OSC 99 desktop notification.
    NotificationClose { identifier: String },

    /// Query origin-owned live Kitty OSC 99 notification identifiers.
    NotificationAliveQuery {
        identifier: String,
        reply_uses_8bit_controls: bool,
    },

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

/// Desktop sound policy from Kitty OSC 99 `s` metadata.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum NotificationSound {
    /// Defer to the desktop notification service.
    #[default]
    System,
    /// Suppress notification sound.
    Silent,
    /// Error-associated sound.
    Error,
    /// Warning-associated sound (`warn` and `warning`).
    Warning,
    /// Informational sound.
    Information,
    /// Question-associated sound.
    Question,
}

/// Desktop expiry policy from Kitty OSC 99 `w` metadata.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum NotificationExpiry {
    /// Defer to the desktop notification service (`w=-1` or absent).
    #[default]
    System,
    /// Ask the desktop to retain the notification indefinitely (`w=0`).
    Never,
    /// Close the notification after this positive number of milliseconds.
    AfterMillis(u32),
}

/// Presentation condition from Kitty OSC 99 `o` metadata.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum NotificationOccasion {
    /// Present regardless of the originating terminal's focus or visibility.
    #[default]
    Always,
    /// Present only when the originating terminal lacks keyboard focus.
    Unfocused,
    /// Present only when the originating terminal is both unfocused and hidden.
    Invisible,
}

/// Desktop notification request
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct NotificationRequest {
    /// Notification title. Kitty OSC 99 body-only requests promote their body
    /// here as required by that protocol.
    pub title: Option<String>,
    /// Notification body/message
    pub body: String,
    /// Optional Kitty OSC 99 notification identifier used for replacement.
    pub identifier: Option<String>,
    /// Application identity supplied by Kitty OSC 99 `f` metadata.
    pub application_name: Option<String>,
    /// Ordered notification categories supplied by Kitty OSC 99 `t` metadata.
    pub notification_types: Vec<String>,
    /// Ordered icon-name candidates supplied by Kitty OSC 99 `n` metadata.
    pub icon_names: Vec<String>,
    /// Optional Kitty OSC 99 cache identifier for transmitted icon data.
    pub icon_cache_id: Option<String>,
    /// Optional base64-decoded PNG/JPEG/GIF bytes from Kitty OSC 99 `p=icon`.
    pub icon_data: Option<Box<[u8]>>,
    /// Optional Kitty OSC 99 urgency. Absence retains the desktop default.
    pub urgency: Option<NotificationUrgency>,
    /// Kitty OSC 99 expiry policy.
    pub expiry: NotificationExpiry,
    /// Kitty OSC 99 presentation occasion.
    pub occasion: NotificationOccasion,
    /// Kitty OSC 99 sound policy.
    pub sound: NotificationSound,
    /// Whether Kitty OSC 99 requested an asynchronous close report with `c=1`.
    pub report_close: bool,
    /// Whether Kitty OSC 99 requested activation/button reports with
    /// `a=report`.
    pub report_activation: bool,
    /// Whether Kitty OSC 99 requested that a user activation focus the exact
    /// originating terminal with `a=focus`. Completed OSC 99 requests default
    /// this to true; legacy OSC 9/777 requests retain the struct default of
    /// false.
    pub focus_on_activation: bool,
    /// Ordered Kitty OSC 99 button labels from a completed `p=buttons` payload.
    pub buttons: Vec<String>,
    /// Control framing captured when an OSC 99 request that can produce a
    /// delayed reply was parsed.
    pub reply_uses_8bit_controls: bool,
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
