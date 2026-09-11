//! Shared types for renderer data exchange
//!
//! Info structs, click result enums, and constants used by both the renderer
//! and the application layer to pass rendering data.

/// Global paint phases for SGR slow and rapid blink. Terminal content remains
/// unchanged; only foreground paint is gated by these values.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TextBlinkPhases {
    pub slow_visible: bool,
    pub rapid_visible: bool,
}

/// Platform input-method composition displayed at the focused text cursor.
/// The text remains local UI state and is not terminal output.
#[derive(Clone, Debug, PartialEq)]
pub struct ImePreeditRenderInfo {
    pub text: String,
    pub cursor: Option<(usize, usize)>,
    pub x: f32,
    pub y: f32,
    pub max_width: f32,
}

impl Default for TextBlinkPhases {
    fn default() -> Self {
        Self {
            slow_visible: true,
            rapid_visible: true,
        }
    }
}

/// Which SGR blink rates occur in the currently visible terminal viewports.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct VisibleTextBlink {
    pub slow: bool,
    pub rapid: bool,
}

/// User-facing position of one explicitly configured font candidate.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FontCoverageSource {
    /// A matching inclusive Unicode-range preference.
    Range { start: u32, end: u32 },
    /// The configured primary family.
    Primary,
    /// One-based position in the configured fallback chain.
    Fallback { index: usize },
}

/// One family actually examined by the renderer's explicit routing policy.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FontCoverageAttempt {
    pub source: FontCoverageSource,
    pub family: String,
    pub installed: bool,
    pub complete: bool,
}

/// Final route selected after the explicit family attempts.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FontCoverageRoute {
    /// One configured candidate covers the complete grapheme.
    Explicit {
        source: FontCoverageSource,
        family: String,
    },
    /// Cosmic-text resolved one or more platform fallback faces.
    Platform {
        families: Vec<String>,
        names_truncated: bool,
    },
    /// No explicit candidate or platform shaping face produced a glyph.
    Missing,
    /// The grapheme contains only shaping controls that need no visible glyph.
    DefaultIgnorable { family: String },
}

/// Bounded explanation for one extended grapheme.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FontCoverageEntry {
    pub grapheme: String,
    pub codepoints: Vec<u32>,
    pub attempts: Vec<FontCoverageAttempt>,
    pub route: FontCoverageRoute,
}

/// Explicitly requested, read-only coverage report.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FontCoverageReport {
    pub entries: Vec<FontCoverageEntry>,
    pub graphemes_truncated: bool,
}

/// Result of clicking in the tab bar
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TabBarClick {
    /// Clicked on tab body at given index
    Tab(usize),
    /// Clicked close (×) button on tab at given index
    CloseTab(usize),
    /// Clicked the + (new tab) button
    NewTab,
    /// Move to the previous tab from the overflow strip
    PreviousTab,
    /// Move to the next tab from the overflow strip
    NextTab,
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
    /// Toggle case sensitivity `[Aa]`
    ToggleCaseSensitive,
    /// Toggle regex mode `[.*]`
    ToggleRegex,
    /// Toggle filter mode `[⊜]`
    ToggleFilter,
    /// Previous match ▲
    PrevMatch,
    /// Next match ▼
    NextMatch,
    /// Dismiss search and clear its result state.
    Close,
}

/// Git branch completion information for the dropdown overlay
#[derive(Debug, Clone)]
pub struct GitCompletionInfo {
    /// The list of matching branch names
    pub completions: Vec<String>,
    /// Currently selected index
    pub selected_index: Option<usize>,
}

/// Bounded information/error/choice overlay data for rendering.
#[derive(Debug, Clone)]
pub struct InfoOverlayInfo {
    pub title: String,
    pub visible_lines: Vec<(usize, String)>,
    pub choices: Vec<crate::ui::info_overlay::InfoOverlayChoice>,
    pub input: Option<crate::ui::info_overlay::InfoOverlayInputInfo>,
    pub total_lines: usize,
    pub scroll_offset: usize,
    pub horizontal_offset: usize,
    pub awaiting: bool,
    pub animation: f32,
    pub layout: crate::ui::info_overlay::InfoOverlayLayout,
    /// Current mouse position (for hover effects)
    pub mouse_pos: Option<(f32, f32)>,
    /// Whether the mouse is currently pressed (for click effects)
    pub mouse_pressed: bool,
}

/// Info for rendering the diff review overlay
#[derive(Debug, Clone)]
pub struct DiffReviewInfo {
    pub filename: String,
    pub visible_rows: Vec<(usize, crate::ui::diff_review::DiffReviewRow)>,
    pub total_rows: usize,
    pub scroll_offset: usize,
    pub horizontal_offset: usize,
    pub awaiting: bool,
    pub animation: f32,
    pub layout: crate::ui::diff_review::DiffReviewLayout,
    pub mouse_pos: Option<(f32, f32)>,
    pub mouse_pressed: bool,
}

/// Info for rendering the context picker overlay
#[derive(Debug, Clone)]
pub struct ContextPickerInfo {
    pub snapshot: crate::ui::context_picker::ContextPickerSnapshot,
    pub layout: crate::ui::context_picker::ContextPickerLayout,
    pub mouse_pos: Option<(f32, f32)>,
    pub mouse_pressed: bool,
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

/// Render data for a single pane in multi-pane mode.
///
/// Each pane holds a reference to its terminal grid and its pixel-space
/// rectangle within the content area.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BroadcastTargetState {
    Armed,
    Active,
}

pub struct PaneRenderData<'a> {
    /// Stable identity of the live terminal, independent of whether its live
    /// grid or a synchronized-output snapshot is being presented.
    pub terminal_key: usize,
    /// The terminal grid to render
    pub grid: &'a termeric_core::Grid,
    /// Selection state for this pane (if any)
    pub selection: Option<&'a termeric_core::Selection>,
    /// Terminal-local OSC palette/default color overrides.
    pub dynamic_colors: &'a termeric_core::DynamicColors,
    /// Pixel rectangle within the content area (relative to content_top)
    pub rect: crate::panes::PaneRect,
    /// Whether this pane currently has focus
    pub is_focused: bool,
    /// This pane is selected for an armed or active broadcast operation.
    pub broadcast_target: Option<BroadcastTargetState>,
    /// User input is locked while terminal output/replies remain live.
    pub read_only: bool,
    /// The pane is filling the content area while its split tree is preserved.
    pub is_zoomed: bool,
    /// Cursor position in grid coordinates (animated for focused pane, raw for unfocused)
    pub cursor_pos: (f32, f32),
    /// Incremental auto-detected URL cache for this pane's visible rows.
    pub url_cache: &'a crate::terminal::url::UrlCache,
    /// Keyboard copy cursor and quick-select labels for the focused pane.
    pub copy_mode: Option<&'a CopyModeRenderInfo>,
    /// Graphics placements owned by this pane's terminal.
    pub images: Vec<termeric_core::PresentationImage>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct QuickSelectHintRenderInfo {
    pub label: String,
    pub row: i32,
    pub col: usize,
    pub matches_prefix: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CopyModeRenderInfo {
    pub cursor: termeric_core::Point,
    pub status: String,
    pub hints: Vec<QuickSelectHintRenderInfo>,
}

/// Info for rendering harvest mode indicators in the gutter
pub struct HarvestRenderInfo {
    /// Growth indicators to draw in the left gutter
    pub indicators: Vec<IndicatorRenderInfo>,
    /// Root cellar panel (command history), if visible
    pub cellar: Option<CellarRenderInfo>,
    /// Sprout context menu, if visible
    pub sprout_menu: Option<SproutMenuRenderInfo>,
}

/// A single growth indicator in the gutter
pub struct IndicatorRenderInfo {
    /// Terminal row where this indicator sits
    pub row: usize,
    /// Character to render (from GrowthPhase::character())
    pub character: char,
    /// RGBA color (from GrowthPhase::color())
    pub color: [f32; 4],
    /// Scale factor for the indicator (pulse animation)
    pub scale: f32,
    /// Opacity (fades during Finished phase)
    pub opacity: f32,
}

/// Root cellar panel render info
pub struct CellarRenderInfo {
    /// Command entries to display
    pub entries: Vec<CellarEntryInfo>,
    pub total_entries: usize,
    /// Older entries discarded by the bounded history since clear.
    pub evicted_entries: u64,
    /// Slide-in animation progress (0.0 = hidden, 1.0 = fully visible)
    pub animation_progress: f32,
    /// Overall success rate (0.0 - 1.0)
    pub success_rate: f32,
    /// Current scroll offset
    pub scroll: usize,
    /// Selected stable entry identity.
    pub selected_id: Option<u64>,
    /// Exact shared panel geometry.
    pub layout: crate::harvest::CellarLayout,
}

/// A single entry in the root cellar
#[allow(dead_code)]
pub struct CellarEntryInfo {
    pub id: u64,
    pub command: String,
    /// Terminal row of the command
    pub row: i32,
    /// Exit code (None if still running)
    pub exit_code: Option<i32>,
    /// Whether the command succeeded
    pub success: bool,
    /// Whether only a clipped 64-KiB output snapshot is retained.
    pub output_truncated: bool,
    /// Health level (0 = healthy, 1 = warning, 2 = sick)
    pub health: u8,
    /// Health color (RGBA)
    pub health_color: [f32; 4],
    /// Visual opacity
    pub opacity: f32,
    /// Depth/age value
    pub depth: f32,
}

/// Sprout context menu render info
pub struct SproutMenuRenderInfo {
    /// Actions with (label, icon_char, enabled) tuples.
    pub actions: Vec<(String, char, bool)>,
    /// Currently selected action index
    pub selected: usize,
    /// Menu opacity (for fade animation)
    pub opacity: f32,
    /// Exact shared menu geometry.
    pub layout: crate::harvest::SproutMenuLayout,
}
