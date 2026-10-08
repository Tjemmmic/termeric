//! Shared types for renderer data exchange
//!
//! Info structs, click result enums, and constants used by both the renderer
//! and the application layer to pass rendering data.

use crate::ui::geometry::Bounds;

/// Global paint phases for SGR slow and rapid blink. Terminal content remains
/// unchanged; only foreground paint is gated by these values.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TextBlinkPhases {
    pub slow_visible: bool,
    pub rapid_visible: bool,
}

/// A visible tab's index and its slot's start and length along the tab bar.
pub type TabSpan = (usize, f32, f32);

/// A tab being dragged along the tab bar, and tabs sliding into new slots.
/// Positions run along the bar: x for the top bar, y for the side rail.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TabDragVisual {
    /// The dragged tab's index and its leading edge, which follows the
    /// pointer. It is drawn over the others.
    pub dragged: Option<(usize, f32)>,
    /// Tabs drawn this far from their slots while they slide into them.
    pub sliding: Vec<(usize, f32)>,
    /// Copy the dragged tab out of this frame, for the pointer to carry
    /// outside the window.
    pub capture: bool,
}

/// A still picture of the dragged tab, copied from the frame that drew it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DraggedTabPicture {
    /// Straight-alpha RGBA rows, `width` × `height` pixels.
    pub rgba: Vec<u8>,
    pub width: u32,
    pub height: u32,
    /// Where its top-left pixel was drawn in the window.
    pub origin: (u32, u32),
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
    /// Clicked the Workspaces menu (▾) button
    WorkspacesMenu,
    /// Clicked the commands (›_) button
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
    /// Inert painted bar space, consumed without reaching the terminal.
    Background,
    /// Focus the existing query at a whole-grapheme byte boundary.
    Query(usize),
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
    pub completions: crate::git::BranchCompletions,
    /// Currently selected index
    pub selected_index: Option<usize>,
}

/// One frame of the open dialog: what it shows, its shared geometry, and
/// where focus, the scroll, and the pointer are.
#[derive(Debug, Clone)]
pub struct DialogInfo {
    pub layout: crate::ui::dialog::DialogLayout,
    pub body: Vec<crate::ui::dialog::DialogBlock>,
    pub field: Option<crate::ui::dialog::DialogFieldView>,
    pub rows: Vec<crate::ui::dialog::DialogRow>,
    pub buttons: Vec<crate::ui::dialog::DialogButton>,
    pub focus: Option<crate::ui::dialog::DialogFocus>,
    /// The body's scroll offset in physical pixels.
    pub scroll: f32,
    pub awaiting: bool,
    pub animation: f32,
    /// The pointer, for hover.
    pub mouse_pos: Option<(f32, f32)>,
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
    /// The button with keyboard focus, which draws the focus ring.
    pub focused_button: Option<crate::ui::diff_review::DiffReviewButton>,
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
    /// The terminal the command is for, e.g. "GUI tab 3, pane 1".
    pub target: String,
    pub layout: crate::ui::guardrail::GuardrailLayout,
}

/// Whether a pane is a broadcast target, and whether broadcasting is live.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BroadcastTargetState {
    Armed,
    Active,
}

/// Where the terminal area sits in the window, in physical pixels. It
/// places every pane, and is the one place that says how a tab's only pane
/// differs from split panes.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PanePlacement {
    /// Left edge of the terminal area: the width of a vertical tab rail.
    pub left: f32,
    /// Top edge of the terminal area: the height of the tab bar.
    pub top: f32,
    /// Right edge of the terminal area, left of the agent view's dock.
    pub right: f32,
    /// Bottom edge of the window.
    pub bottom: f32,
    pub padding: f32,
    /// The display scale, for the logical lengths of the chrome it places
    /// over panes (their scrollbars).
    pub scale_factor: f32,
}

impl PanePlacement {
    /// The window rectangle of the pane laid out at `rect`. A tab's `lone`
    /// pane fills the terminal area inside the padding on every side, as an
    /// unsplit terminal always has, so its grid sits one padding below the
    /// tab bar. Split panes sit at their layout rectangles, which are
    /// relative to the area's left padding and its top edge.
    pub fn pane_bounds(self, rect: Bounds, lone: bool) -> Bounds {
        if lone {
            Bounds {
                x: self.left + self.padding,
                y: self.top + self.padding,
                width: self.right - self.left - self.padding * 2.0,
                height: self.bottom - self.top - self.padding * 2.0,
            }
        } else {
            Bounds {
                x: self.left + self.padding + rect.x,
                y: self.top + rect.y,
                width: rect.width,
                height: rect.height,
            }
        }
    }

    /// The columns and rows the pane laid out at `rect` shows of its
    /// `grid_size` grid in cells of `cell_size`: a `lone` pane shows every
    /// cell of its grid, as an unsplit terminal did; a split pane shows the
    /// whole cells its rectangle holds.
    pub fn pane_cells(
        rect: Bounds,
        lone: bool,
        grid_size: (usize, usize),
        cell_size: (f32, f32),
    ) -> (usize, usize) {
        if lone {
            grid_size
        } else {
            (
                ((rect.width / cell_size.0).floor() as usize).max(1),
                ((rect.height / cell_size.1).floor() as usize).max(1),
            )
        }
    }

    /// The cell where Harvest marks row `row` of the pane laid out at
    /// `rect`: just left of the pane's grid, in the margin there (the
    /// padding at the terminal area's left edge, a divider elsewhere). A
    /// margin narrower than a cell pushes the marker over the pane's own
    /// first column rather than onto its neighbour.
    pub fn row_marker(self, rect: Bounds, lone: bool, row: usize, cell_size: (f32, f32)) -> Bounds {
        let pane = self.pane_bounds(rect, lone);
        let margin = if lone || rect.x <= 0.0 {
            self.padding
        } else {
            crate::panes::PANE_DIVIDER_THICKNESS
        };
        Bounds::new(
            pane.x - cell_size.0.min(margin),
            pane.y + row as f32 * cell_size.1,
            cell_size.0,
            cell_size.1,
        )
    }

    /// The scrollbar the pane laid out at `rect` draws for `grid`, if any.
    pub fn pane_scrollbar(
        self,
        rect: Bounds,
        lone: bool,
        grid: &termeric_core::Grid,
        cell_size: (f32, f32),
    ) -> Option<PaneScrollbar> {
        let bounds = self.pane_bounds(rect, lone);
        let (_, rows) = Self::pane_cells(rect, lone, (grid.cols(), grid.rows()), cell_size);
        PaneScrollbar::of_pane(
            Bounds::new(
                bounds.x,
                bounds.y,
                bounds.width.max(0.0),
                rows as f32 * cell_size.1,
            ),
            grid.scrollback_len() > 0,
            self.scale_factor,
        )
    }
}

/// A pane's scrollbar, drawn along its right edge while it has scrollback.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PaneScrollbar {
    /// The drawn track, which the thumb moves along.
    pub track: Bounds,
    /// The strip at the pane's right edge that takes the pointer: as tall
    /// as the track and wider, so the thin track is easy to hit.
    pub hit: Bounds,
}

impl PaneScrollbar {
    // Logical pixels, scaled to the display.
    const WIDTH: f32 = 6.0;
    /// The gap between the track and the pane's right edge.
    const MARGIN: f32 = 2.0;
    const HIT_WIDTH: f32 = 16.0;
    /// A track no taller than this is not drawn.
    const MIN_HEIGHT: f32 = 20.0;

    /// The scrollbar of a pane whose shown rows cover `rows_area`, when it
    /// has scrollback and room to draw one, at `scale_factor`. Callers go
    /// through [`PanePlacement::pane_scrollbar`], so drawing, hits, and
    /// drags agree.
    fn of_pane(rows_area: Bounds, has_scrollback: bool, scale_factor: f32) -> Option<Self> {
        let px = |logical| super::physical_px(logical, scale_factor);
        if !has_scrollback || rows_area.height <= px(Self::MIN_HEIGHT) {
            return None;
        }
        let right = rows_area.right();
        let (width, hit_width) = (px(Self::WIDTH), px(Self::HIT_WIDTH));
        Some(Self {
            track: Bounds::new(
                right - width - px(Self::MARGIN),
                rows_area.y,
                width,
                rows_area.height,
            ),
            hit: Bounds::new(right - hit_width, rows_area.y, hit_width, rows_area.height),
        })
    }
}

/// One pane to draw: its terminal's presented state and its layout
/// rectangle within the terminal area.
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
    /// Kitty multiple cursors at the same presentation boundary as the grid.
    pub extra_cursors: &'a termeric_core::ExtraCursorState,
    /// Layout rectangle within the terminal area, placed in the window by
    /// [`PanePlacement::pane_bounds`].
    pub rect: Bounds,
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
    /// Effective main-cursor shape used by follow-main extra cursors.
    pub cursor_style: crate::config::CursorStyle,
    /// Pane-local blink policy evaluated on the shared cursor clock.
    pub cursor_opacity: f32,
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
    /// Growth indicators to draw beside their pane's rows
    pub indicators: Vec<IndicatorRenderInfo>,
    /// Root cellar panel (command history), if visible
    pub cellar: Option<CellarRenderInfo>,
    /// Sprout context menu, if visible
    pub sprout_menu: Option<SproutMenuRenderInfo>,
}

/// A single growth indicator beside the row its command started on
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct IndicatorRenderInfo {
    /// The cell it is drawn in, from [`PanePlacement::row_marker`].
    pub cell: Bounds,
    /// Its phase, which picks its character and color.
    pub phase: crate::harvest::GrowthPhase,
    /// 1.0 until it fades out after its command finishes.
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
pub struct CellarEntryInfo {
    pub id: u64,
    pub command: String,
    /// Exit code (None if still running)
    pub exit_code: Option<i32>,
    /// Whether only a clipped 64-KiB output snapshot is retained.
    pub output_truncated: bool,
    /// How the command fared, which picks its marker and color.
    pub health: crate::harvest::CommandHealth,
}

/// Sprout context menu render info
pub struct SproutMenuRenderInfo {
    /// Actions with (label, icon, enabled) tuples.
    pub actions: Vec<(String, crate::ui::icon::Icon, bool)>,
    /// Currently selected action index
    pub selected: usize,
    /// Menu opacity (for fade animation)
    pub opacity: f32,
    /// Exact shared menu geometry.
    pub layout: crate::harvest::SproutMenuLayout,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_lone_pane_fills_the_padded_area_and_split_panes_sit_at_their_rectangles() {
        let placement = PanePlacement {
            left: 40.0,
            top: 30.0,
            right: 840.0,
            bottom: 630.0,
            padding: 8.0,
            scale_factor: 1.0,
        };
        let rect = Bounds {
            x: 100.0,
            y: 50.0,
            width: 300.0,
            height: 200.0,
        };
        // An unsplit terminal's grid starts one padding below the tab bar and
        // spans the padded width, whatever its layout rectangle.
        let lone = Bounds {
            x: 48.0,
            y: 38.0,
            width: 784.0,
            height: 584.0,
        };
        assert_eq!(placement.pane_bounds(rect, true), lone);
        assert_eq!(placement.pane_bounds(Bounds::default(), true), lone);
        // Split panes have the left padding but no top padding.
        assert_eq!(
            placement.pane_bounds(rect, false),
            Bounds {
                x: 148.0,
                y: 80.0,
                width: 300.0,
                height: 200.0,
            }
        );
    }

    #[test]
    fn harvest_marks_a_row_beside_its_own_pane() {
        let placement = PanePlacement {
            left: 40.0,
            top: 30.0,
            right: 840.0,
            bottom: 630.0,
            padding: 12.0,
            scale_factor: 1.0,
        };
        let cell = (10.0, 20.0);
        let left = Bounds::new(0.0, 0.0, 398.0, 600.0);
        let right = Bounds::new(402.0, 0.0, 386.0, 290.0);
        let lower_right = Bounds::new(402.0, 294.0, 386.0, 306.0);
        // A lone pane's marker sits in the left padding, a cell left of its
        // grid, which starts one padding below the tab bar.
        assert_eq!(
            placement.row_marker(left, true, 3, cell),
            Bounds::new(42.0, 102.0, 10.0, 20.0)
        );
        // Split panes are marked beside their own rows: at the area's left
        // edge in the padding, elsewhere as far into the divider as it
        // reaches.
        assert_eq!(
            placement.row_marker(left, false, 3, cell),
            Bounds::new(42.0, 90.0, 10.0, 20.0)
        );
        assert_eq!(
            placement.row_marker(lower_right, false, 2, cell),
            Bounds::new(450.0, 364.0, 10.0, 20.0)
        );
        assert_eq!(
            placement.row_marker(right, false, 0, cell).y,
            placement.pane_bounds(right, false).y
        );
        // A padding narrower than a cell keeps the marker off the tab rail.
        let narrow = PanePlacement {
            padding: 4.0,
            ..placement
        };
        assert_eq!(narrow.row_marker(left, true, 0, cell).x, 40.0);
    }

    #[test]
    fn a_pane_scrollbar_runs_down_its_shown_rows_at_its_right_edge() {
        let placement = PanePlacement {
            left: 40.0,
            top: 30.0,
            right: 840.0,
            bottom: 630.0,
            padding: 8.0,
            scale_factor: 1.0,
        };
        let rect = Bounds::new(100.0, 50.0, 305.0, 210.0);
        let cell = (10.0, 20.0);
        // A lone pane shows its whole grid; a split one the whole cells of
        // its rectangle, and at least one.
        assert_eq!(
            PanePlacement::pane_cells(rect, true, (80, 24), cell),
            (80, 24)
        );
        assert_eq!(
            PanePlacement::pane_cells(rect, false, (80, 24), cell),
            (30, 10)
        );
        assert_eq!(
            PanePlacement::pane_cells(Bounds::default(), false, (80, 24), cell),
            (1, 1)
        );

        let mut grid = termeric_core::Grid::new(80, 20);
        assert_eq!(placement.pane_scrollbar(rect, true, &grid, cell), None);
        grid.scroll_up();
        assert!(grid.scrollback_len() > 0);
        // Inside the lone pane's padding, beside its 20 rows; the pointer
        // finds it in a wider strip ending at the pane's edge.
        let scrollbar = placement.pane_scrollbar(rect, true, &grid, cell).unwrap();
        assert_eq!(scrollbar.track, Bounds::new(824.0, 38.0, 6.0, 400.0));
        assert_eq!(scrollbar.hit, Bounds::new(816.0, 38.0, 16.0, 400.0));
        // A split pane's runs down the rows its rectangle shows.
        let scrollbar = placement.pane_scrollbar(rect, false, &grid, cell).unwrap();
        assert_eq!(scrollbar.track, Bounds::new(445.0, 80.0, 6.0, 200.0));
        assert_eq!(scrollbar.hit, Bounds::new(437.0, 80.0, 16.0, 200.0));
        // Too short a pane draws none.
        let short = Bounds::new(100.0, 50.0, 305.0, 20.0);
        assert_eq!(placement.pane_scrollbar(short, false, &grid, cell), None);

        // Its lengths are logical: at twice the scale the track, its margin,
        // the hit strip, and the shortest pane that draws one all double.
        let doubled = PanePlacement {
            scale_factor: 2.0,
            ..placement
        };
        let scrollbar = doubled.pane_scrollbar(rect, false, &grid, cell).unwrap();
        assert_eq!(scrollbar.track, Bounds::new(437.0, 80.0, 12.0, 200.0));
        assert_eq!(scrollbar.hit, Bounds::new(421.0, 80.0, 32.0, 200.0));
        let shortish = Bounds::new(100.0, 50.0, 305.0, 40.0);
        assert!(
            placement
                .pane_scrollbar(shortish, false, &grid, cell)
                .is_some()
        );
        assert_eq!(doubled.pane_scrollbar(shortish, false, &grid, cell), None);
    }
}
