//! The line icons chrome and the agent view draw. Each is a set of strokes
//! in `renderer/icons.rs`, scaled to the size it is drawn at.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Icon {
    Plus,
    Minus,
    Close,
    ChevronDown,
    ChevronRight,
    ChevronUp,
    Back,
    History,
    More,
    Send,
    Stop,
    Terminal,
    /// A shell prompt, "›_".
    Prompt,
    File,
    Pencil,
    Search,
    /// Three narrowing lines: show only matching lines.
    Filter,
    /// Three equal lines: a menu of settings.
    Menu,
    /// A Git branch.
    Branch,
    Shield,
    Check,
    Cross,
    Eye,
    Plan,
    Copy,
    Globe,
    Agent,
    Folder,
    Warning,
    Expand,
}
