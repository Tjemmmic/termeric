//! Shared types for renderer data exchange
//!
//! Info structs, click result enums, and constants used by both the renderer
//! and the application layer to pass rendering data.

/// Result of clicking in the tab bar
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TabBarClick {
    /// Clicked on tab body at given index
    Tab(usize),
    /// Clicked close (×) button on tab at given index
    CloseTab(usize),
    /// Clicked the + (new tab) button
    NewTab,
    /// Clicked the session dropdown (▾) button
    SessionDropdown,
    /// Clicked the commands (⚡) button
    CommandsButton,
    /// Clicked the gear icon
    Gear,
    /// No element was clicked
    None,
}

/// Result of clicking in the search bar
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SearchBarClick {
    /// No element was clicked
    None,
    /// Toggle case sensitivity [Aa]
    ToggleCaseSensitive,
    /// Toggle regex mode [.*]
    ToggleRegex,
    /// Toggle filter mode [⊜]
    ToggleFilter,
    /// Previous match ▲
    PrevMatch,
    /// Next match ▼
    NextMatch,
}

/// Git branch completion information for the dropdown overlay
#[derive(Debug, Clone)]
pub struct GitCompletionInfo {
    /// The list of matching branch names
    pub completions: Vec<String>,
    /// Currently selected index
    pub selected_index: Option<usize>,
}

/// Info overlay data for rendering (About/Version)
#[derive(Debug, Clone)]
pub struct InfoOverlayInfo {
    pub title: String,
    pub lines: Vec<String>,
    pub animation: f32,
    /// Current mouse position (for hover effects)
    pub mouse_pos: Option<(f32, f32)>,
    /// Whether the mouse is currently pressed (for click effects)
    pub mouse_pressed: bool,
}

/// Info for rendering the diff review overlay
#[derive(Debug, Clone)]
pub struct DiffReviewInfo {
    pub filename: String,
    pub lines: Vec<DiffReviewLine>,
    pub scroll_offset: usize,
    pub animation: f32,
}

/// A single line in the diff review overlay
#[derive(Debug, Clone)]
pub enum DiffReviewLine {
    /// Section header (e.g., @@ -1,3 +1,4 @@)
    Header(String),
    /// Context (unchanged) line
    Context(String),
    /// Added line (+)
    Added(String),
    /// Removed line (-)
    Removed(String),
    /// Blank separator
    Blank,
}

/// Info for rendering the context picker overlay
#[derive(Debug, Clone)]
pub struct ContextPickerInfo {
    pub sources: Vec<ContextPickerSource>,
    pub selected_index: usize,
    pub animation: f32,
}

/// A context source item in the picker
#[derive(Debug, Clone)]
pub struct ContextPickerSource {
    pub label: String,
    pub selected: bool,
    pub available: bool,
}

/// Info for rendering the guardrail confirmation overlay
#[derive(Debug, Clone)]
pub struct GuardrailInfo {
    pub command: String,
    pub reason: String,
    pub is_critical: bool,
    pub animation: f32,
    /// Where the command will run (e.g., "Local", "SSH: dev01")
    pub target: String,
    /// As which user (e.g., "root", "deploy")
    pub user: String,
}

/// Info for rendering the session dropdown
pub struct SessionDropdownInfo {
    /// List of sessions as (id, display_title) pairs
    pub sessions: Vec<(String, String)>,
    /// Mouse-hovered row index (None if not hovering; index = session rows then "Manage Sessions")
    pub hovered_index: Option<usize>,
}

/// Result of clicking in the session dropdown
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionDropdownClick {
    /// Clicked on a session by name
    LoadSession(String),
    /// Clicked "Manage Sessions…"
    ManageSessions,
    /// Clicked outside the dropdown
    Dismiss,
    /// No click in dropdown area
    None,
}

/// Maximum items shown in session dropdown before scrolling
pub const SESSION_DROPDOWN_MAX_ITEMS: usize = 5;
