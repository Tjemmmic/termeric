//! Terminal state machine
//!
//! This module contains the main terminal state including modes,
//! cursor handling, and the interface for processing escape sequences.

mod cursor;
mod graphics_commands;
mod graphics_relay;
mod keyboard;
mod mode;
mod multicell;
mod multiple_cursors;
mod notification;
pub(crate) mod pointer;
mod prompts;
mod rect_ops;
mod reports;
mod resize;
mod selection;
mod selection_damage;
mod sgr;
mod title;
mod xterm_keys;

pub(crate) use cursor::SavedCursor;
pub use cursor::{TerminalCursorShape, TerminalCursorStyle};
pub use keyboard::{
    KittyKeyboardFlags, XtermKeyFormat, XtermKeyMode, XtermKeyResource, XtermKeyResources,
    XtermKeySetting, XtermKeyboardResource, XtermKeyboardType, XtermModifyOtherKeys,
};
pub(crate) use keyboard::{MAX_USER_DEFINED_KEY_BYTES, XtermUserDefinedKeys};
pub use mode::TerminalMode;
use multicell::Osc66Command;
pub use multiple_cursors::{
    ExtraCursorColor, ExtraCursorColors, ExtraCursorShape, ExtraCursorState,
};
pub use notification::MAX_NOTIFICATION_ICON_BYTES;
pub(crate) use notification::{MAX_NOTIFICATION_BODY_BYTES, MAX_NOTIFICATION_TITLE_BYTES};
use notification::{NotificationProtocol, Osc99Action};
#[cfg(test)]
pub(crate) use pointer::MAX_POINTER_SHAPE_PAYLOAD_BYTES;
pub use pointer::PointerShape;
pub use selection::{Point, Selection, SelectionType};
pub(crate) use title::MAX_TITLE_BYTES;
pub(crate) use title::XTERM_TITLE_STACK_DEPTH;
use title::{ApplicationTitles, TitleTarget};

const MAX_PENDING_NOTIFICATION_ICON_EVENT_BYTES: usize = 2 * MAX_NOTIFICATION_ICON_BYTES;

use crate::ansi::Processor;
use crate::event::TerminalEvent;
use crate::graphics::{
    GraphicsManager, GraphicsRelaySnapshot, GraphicsResponse, GraphicsSnapshot, ImageId,
    ImagePlacement, KittyCommand, MAX_GRAPHICS_RELAY_METADATA_BYTES, MAX_KITTY_APC_BYTES,
    PresentationImage, StoredImage,
};
use crate::grid::{
    Cell, CellAttrs, CellContent, CellFlags, Color, ColorSpec, Grid, HyperlinkRegistry, MultiCell,
};
use crate::shell::{MAX_COMMAND_TEXT_BYTES, PromptTracker, PromptType};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};
use unicode_segmentation::UnicodeSegmentation;

/// Tab stop interval
const TAB_INTERVAL: usize = 8;
/// REP remembers one normal grapheme but does not duplicate a pathological,
/// attacker-sized combining sequence into a compact control sequence.
const MAX_REPEAT_GRAPHEME_BYTES: usize = 1024;
/// Safety valve for applications that enter synchronized output and never
/// leave it. The protocol deliberately does not prescribe a timeout; one
/// second bounds stale presentation and snapshot memory without affecting a
/// normal frame-sized update.
pub(crate) const SYNCHRONIZED_OUTPUT_TIMEOUT: Duration = Duration::from_secs(1);

/// Bounded shell-reported working-directory metadata from OSC 7.
///
/// The path is decoded UTF-8 and always absolute in the URI's path syntax.
/// Consumers must still decide whether `host` names their local machine before
/// using the path for filesystem access.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReportedWorkingDirectory {
    uri: String,
    host: Option<String>,
    path: String,
}

impl ReportedWorkingDirectory {
    pub(crate) fn new(uri: String, host: Option<String>, path: String) -> Self {
        Self { uri, host, path }
    }

    pub fn uri(&self) -> &str {
        &self.uri
    }

    pub fn host(&self) -> Option<&str> {
        self.host.as_deref()
    }

    pub fn path(&self) -> &str {
        &self.path
    }
}

struct SynchronizedOutputSnapshot {
    grid: Grid,
    selection: Option<Selection>,
    reported_working_directory: Option<ReportedWorkingDirectory>,
    alternate_screen: bool,
    dynamic_colors: DynamicColors,
    cursor_style: Option<TerminalCursorStyle>,
    cursor_blink_override: Option<bool>,
    extra_cursors: ExtraCursorState,
    graphics: GraphicsSnapshot,
    started_at: Instant,
}

struct PendingGraphicsRelayImport {
    expected_bytes: usize,
    expected_checksum: u64,
    metadata: Vec<u8>,
    started_at: Instant,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct PromptNavigationState {
    row: i32,
    scroll_offset: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct ShellLogicalAnchor {
    line: usize,
    cell: usize,
}

impl ShellLogicalAnchor {
    fn retained_after_resize(self, outcome: crate::grid::ResizeOutcome) -> Option<Self> {
        let (line, cell) = outcome.retained_anchor(self.line, self.cell)?;
        Some(Self { line, cell })
    }
}

fn shell_anchor_rows(grid: &Grid) -> impl Iterator<Item = (i32, crate::grid::RowView<'_>)> {
    let oldest = -(grid.scrollback_len() as i32);
    let newest = grid.rows().saturating_sub(1) as i32;
    let rows =
        (oldest..=newest).map_while(|index| grid.row_at_offset(index).map(|row| (index, row)));
    #[cfg(test)]
    let rows = rows.inspect(|_| tests::reflow_prompts::count_shell_reflow_row_visit());
    rows
}

fn capture_shell_anchors(grid: &Grid, mut rows: Vec<i32>) -> Vec<(i32, ShellLogicalAnchor)> {
    let oldest = -(grid.scrollback_len() as i32);
    let newest = grid.rows().saturating_sub(1) as i32;
    rows.retain(|row| (oldest..=newest).contains(row));
    rows.sort_unstable();
    rows.dedup();
    let mut result = Vec::with_capacity(rows.len());
    let mut targets = rows.into_iter().peekable();
    if targets.peek().is_none() {
        return result;
    }
    let mut anchor = ShellLogicalAnchor { line: 0, cell: 0 };
    // Sorted physical rows share one logical-line walk, including boundaries
    // on blank cells. Never scan all history separately for each command.
    for (index, row) in shell_anchor_rows(grid) {
        if targets.peek() == Some(&index) {
            result.push((index, anchor));
            targets.next();
            if targets.peek().is_none() {
                break;
            }
        }
        if row.is_wrapped() {
            anchor.cell = anchor.cell.saturating_add(row.reflow_unit_count());
        } else {
            anchor.line = anchor.line.saturating_add(1);
            anchor.cell = 0;
        }
    }
    result
}

fn restore_shell_anchors(
    grid: &Grid,
    mut anchors: Vec<(i32, ShellLogicalAnchor)>,
) -> HashMap<i32, i32> {
    anchors.sort_unstable_by_key(|(old_row, anchor)| (anchor.line, anchor.cell, *old_row));
    let mut result = HashMap::with_capacity(anchors.len());
    let mut targets = anchors.into_iter().peekable();
    if targets.peek().is_none() {
        return result;
    }
    let mut start = ShellLogicalAnchor { line: 0, cell: 0 };
    for (index, row) in shell_anchor_rows(grid) {
        let end_cell = start.cell.saturating_add(row.reflow_unit_count());
        while let Some(&(old_row, target)) = targets.peek() {
            // At a hard line end, preserve the old resolver's clamping of a
            // boundary beyond trimmed trailing blanks. Wrapped boundaries
            // belong to the next row, using wide-grapheme-independent units.
            if target.line != start.line || (row.is_wrapped() && target.cell >= end_cell) {
                break;
            }
            result.insert(old_row, index);
            targets.next();
        }
        if targets.peek().is_none() {
            break;
        }
        if row.is_wrapped() {
            start.cell = end_cell;
        } else {
            start.line = start.line.saturating_add(1);
            start.cell = 0;
        }
    }
    result
}

fn stored_character_set(cell: &Cell) -> CharacterSet {
    if cell.flags.contains(CellFlags::DEC_SPECIAL_GRAPHICS) {
        CharacterSet::DecSpecialGraphics
    } else if cell.flags.contains(CellFlags::DEC_UK_CHARSET) {
        CharacterSet::UnitedKingdom
    } else {
        CharacterSet::Ascii
    }
}

/// Estimated memory owned by terminal grids and hyperlink metadata.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TerminalMemoryUsage {
    pub visible_bytes: usize,
    pub scrollback_bytes: usize,
    pub grapheme_bytes: usize,
    pub style_bytes: usize,
    pub hyperlink_bytes: usize,
    pub extra_cursor_bytes: usize,
    pub scrollback_lines: usize,
    pub scrollback_line_limit: usize,
    pub scrollback_byte_limit: usize,
}

/// Terminal-local OSC palette/default color overrides. Theme colors remain the
/// base layer and become visible again when an override is reset.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DynamicColors {
    palette: [Option<Color>; 256],
    foreground: Option<Color>,
    background: Option<Color>,
    cursor: Option<Color>,
    generation: u64,
}

impl Default for DynamicColors {
    fn default() -> Self {
        Self {
            palette: [None; 256],
            foreground: None,
            background: None,
            cursor: None,
            generation: 0,
        }
    }
}

impl DynamicColors {
    pub fn palette(&self, index: u8) -> Option<Color> {
        self.palette[index as usize]
    }

    pub fn foreground(&self) -> Option<Color> {
        self.foreground
    }

    pub fn background(&self) -> Option<Color> {
        self.background
    }

    pub fn cursor(&self) -> Option<Color> {
        self.cursor
    }

    pub fn generation(&self) -> u64 {
        self.generation
    }

    fn set(&mut self, target: crate::event::DynamicColorTarget, color: Color) {
        let slot = match target {
            crate::event::DynamicColorTarget::Palette(index) => &mut self.palette[index as usize],
            crate::event::DynamicColorTarget::Foreground => &mut self.foreground,
            crate::event::DynamicColorTarget::Background => &mut self.background,
            crate::event::DynamicColorTarget::Cursor => &mut self.cursor,
        };
        if *slot != Some(color) {
            *slot = Some(color);
            self.generation = self.generation.wrapping_add(1);
        }
    }

    fn reset(&mut self, target: crate::event::DynamicColorTarget) {
        let slot = match target {
            crate::event::DynamicColorTarget::Palette(index) => &mut self.palette[index as usize],
            crate::event::DynamicColorTarget::Foreground => &mut self.foreground,
            crate::event::DynamicColorTarget::Background => &mut self.background,
            crate::event::DynamicColorTarget::Cursor => &mut self.cursor,
        };
        if slot.take().is_some() {
            self.generation = self.generation.wrapping_add(1);
        }
    }

    fn reset_palette(&mut self) {
        if self.palette.iter().any(Option::is_some) {
            self.palette.fill(None);
            self.generation = self.generation.wrapping_add(1);
        }
    }
}

/// Resident 94-character sets supported in the G0-G3 registers.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum CharacterSet {
    #[default]
    Ascii,
    UnitedKingdom,
    DecSpecialGraphics,
}

impl CharacterSet {
    fn from_designator(designator: u8) -> Option<Self> {
        match designator {
            b'A' => Some(Self::UnitedKingdom),
            b'B' => Some(Self::Ascii),
            b'0' => Some(Self::DecSpecialGraphics),
            _ => None,
        }
    }

    fn designator(self) -> u8 {
        match self {
            Self::Ascii => b'B',
            Self::UnitedKingdom => b'A',
            Self::DecSpecialGraphics => b'0',
        }
    }

    fn translate(self, character: char) -> char {
        match self {
            Self::Ascii => character,
            Self::UnitedKingdom if character == '#' => '£',
            Self::UnitedKingdom => character,
            Self::DecSpecialGraphics => match character {
                '_' => ' ',
                '`' => '◆',
                'a' => '▒',
                'b' => '␉',
                'c' => '␌',
                'd' => '␍',
                'e' => '␊',
                'f' => '°',
                'g' => '±',
                'h' => '␤',
                'i' => '␋',
                'j' => '┘',
                'k' => '┐',
                'l' => '┌',
                'm' => '└',
                'n' => '┼',
                'o' => '⎺',
                'p' => '⎻',
                'q' => '─',
                'r' => '⎼',
                's' => '⎽',
                't' => '├',
                'u' => '┤',
                'v' => '┴',
                'w' => '┬',
                'x' => '│',
                'y' => '≤',
                'z' => '≥',
                '{' => 'π',
                '|' => '≠',
                '}' => '£',
                '~' => '·',
                _ => character,
            },
        }
    }
}

/// The main terminal state
pub struct Terminal {
    /// Terminal-database identity exported to the attached child. XTGETTCAP
    /// reports this exact value rather than assuming the native entry exists.
    term_name: &'static str,
    /// Primary screen grid
    grid: Grid,
    /// Alternate screen grid (for vim, less, etc.)
    alt_grid: Grid,
    /// Whether we're using the alternate screen
    alt_screen_active: bool,
    /// Terminal mode flags
    mode: TerminalMode,
    /// Whether terminal-generated C1 replies use their single-byte 8-bit
    /// representation. Input acceptance is independent of this S7C1T/S8C1T
    /// transmission preference.
    use_8bit_controls: bool,
    /// DEC operating level selected by DECSCL, represented as 1 through 5.
    /// This is negotiation/reporting state; it does not mask the terminal's
    /// implemented parser features.
    dec_conformance_level: u8,
    /// Optional cursor shape/blink override selected by DECSCUSR.
    cursor_style: Option<TerminalCursorStyle>,
    /// Optional cursor-blink override selected by DECSET/DECRST 12.
    /// `None` leaves the user's configured blink policy in control.
    cursor_blink_override: Option<bool>,
    /// Kitty multiple cursors are ephemeral, viewport-local presentation state.
    extra_cursors: ExtraCursorState,
    /// Kitty keyboard negotiation is independent for the main and alternate screens.
    keyboard: keyboard::KittyKeyboardState,
    alt_keyboard: keyboard::KittyKeyboardState,
    /// Kitty OSC 22 pointer-shape stacks are independent per screen.
    pointer_shapes: pointer::PointerShapeStack,
    alt_pointer_shapes: pointer::PointerShapeStack,
    /// Xterm's global keyboard type. DECSET 1060/1061 selects a historical
    /// profile; resetting either mode returns to the normal type.
    xterm_keyboard_type: XtermKeyboardType,
    /// User-configured type restored by RIS and used by unsaved DEC private
    /// mode restores.
    initial_xterm_keyboard_type: XtermKeyboardType,
    /// Shared save slot used by xterm's private modes 1060 and 1061.
    saved_xterm_keyboard_type: Option<XtermKeyboardType>,
    /// DECUDK definitions are global to the terminal owner and are consulted
    /// only by Shift-modified keys in the VT220 keyboard profile.
    xterm_user_defined_keys: XtermUserDefinedKeys,
    /// Xterm resource-zero modifyKeyboard/formatKeyboard state applies across
    /// both screen buffers. Under Termeric's normal keyboard profile it is
    /// queryable compatibility state and does not alter key encoding.
    xterm_keyboard_resource: XtermKeyboardResource,
    /// Xterm's mode-and-format key resources 1-3 and 5-7.
    xterm_key_resources: XtermKeyResources,
    /// Xterm modifyOtherKeys applies to this terminal owner across both
    /// screen buffers and is reset by DECSTR/RIS like xterm's modifier resources.
    xterm_modify_other_keys: XtermModifyOtherKeys,
    /// Tab stop positions
    tabs: Vec<bool>,
    /// Saved cursor state for primary screen
    saved_cursor: Option<SavedCursor>,
    /// Saved cursor state for alternate screen
    alt_saved_cursor: Option<SavedCursor>,
    /// Current text attributes for new characters
    current_attrs: CellAttrs,
    /// Whether newly printed characters are protected from DEC selective
    /// erase operations (DECSCA).
    current_character_protected: bool,
    /// DECSACE selects exact rectangles rather than the default wrapped
    /// stream extent for DECCARA and DECRARA.
    rectangular_change_extent: bool,
    /// DECRQCRA can reveal screen contents one cell at a time. Keep reports
    /// disabled until the embedding application explicitly permits them.
    screen_checksum_reporting: bool,
    /// G0 through G3 94-character-set designations.
    g_sets: [CharacterSet; 4],
    /// G-set currently invoked into GL by LS0 through LS3.
    gl_register: u8,
    /// G-set currently invoked into GR by LS1R through LS3R. UTF-8 input does
    /// not remap non-ASCII scalars, but the ISO-2022 state remains reportable.
    gr_register: u8,
    /// One-shot G2/G3 invocation for the next graphic character.
    single_shift: Option<u8>,
    /// Most recently printed grapheme used by REP. `CellContent::Char` keeps
    /// the overwhelmingly common one-scalar case inline and allocation-free.
    last_graphic: Option<(CellContent, CharacterSet)>,
    /// Current foreground color (resolved RGB)
    current_fg: Color,
    /// Current background color (resolved RGB)
    current_bg: Color,
    /// How the current foreground was specified (indexed, true color, or default)
    current_fg_spec: ColorSpec,
    /// How the current background was specified (indexed, true color, or default)
    current_bg_spec: ColorSpec,
    /// Current underline color (None means use fg color)
    current_underline_color: Option<Color>,
    /// Original underline-color encoding, retained for exact replay and Kitty
    /// Unicode placeholder placement IDs.
    current_underline_color_spec: Option<ColorSpec>,
    /// Current hyperlink ID (if inside a hyperlink)
    current_hyperlink_id: Option<u32>,
    /// Registry of all hyperlinks
    hyperlink_registry: HyperlinkRegistry,
    /// Evictions accumulated before the next reference-aware registry compaction
    hyperlink_gc_evictions: usize,
    /// Current selection
    selection: Option<Selection>,
    /// Invisible modal navigation point, sharing live selection damage rules.
    navigation_cursor: Option<Selection>,
    /// Scroll region top (inclusive, 0-indexed)
    scroll_top: usize,
    /// Scroll region bottom (exclusive, 0-indexed)
    scroll_bottom: usize,
    /// Horizontal scroll region left (inclusive, 0-indexed)
    scroll_left: usize,
    /// Horizontal scroll region right (exclusive, 0-indexed)
    scroll_right: usize,
    /// Escape sequence processor
    processor: Processor,
    /// Number of columns
    cols: usize,
    /// Number of rows
    rows: usize,
    /// Last valid OSC 7 directory report. This is shell/application metadata,
    /// not presentation state, so DECSTR/RIS and screen changes preserve it.
    reported_working_directory: Option<ReportedWorkingDirectory>,
    /// Bounded, terminal-global in-progress Kitty OSC 99 notifications.
    notification_protocol: NotificationProtocol,
    /// Application-authored window/icon labels and xterm's global title ring.
    /// This excludes user/native title metadata by construction.
    application_titles: ApplicationTitles,
    /// Physical cell extent used to normalize pixel-addressed image protocols.
    grid_pixel_width: u32,
    grid_pixel_height: u32,
    cell_pixel_width: u32,
    cell_pixel_height: u32,
    /// Graphics manager for inline images
    graphics: GraphicsManager,
    /// Kitty APC parsing must survive calls to `process`, which may split any byte sequence.
    graphics_apc_data: Vec<u8>,
    graphics_apc_active: bool,
    graphics_apc_overflow: bool,
    graphics_relay_import_allowed: bool,
    graphics_relay_apc_data: Vec<u8>,
    graphics_relay_apc_active: bool,
    graphics_relay_apc_overflow: bool,
    pending_graphics_relay_import: Option<PendingGraphicsRelayImport>,
    /// Shell integration for prompt tracking
    prompt_tracker: PromptTracker,
    /// Exact prompt selected by the previous/next prompt actions. The saved
    /// offset lets direct/manual scrolling invalidate this navigation cursor.
    prompt_navigation: Option<PromptNavigationState>,
    /// Where the cursor was at the last prompt-start mark, as (row, column).
    prompt_start_cursor: Option<(usize, usize)>,
    /// The shell marked the command's start where its prompt starts, before
    /// drawing the prompt (Elvish, Fish 3), so the command really begins where
    /// the cursor is when the person first types.
    command_start_awaits_input: bool,
    /// Pending events to be handled by the application
    pending_events: Vec<TerminalEvent>,
    pending_notification_icon_event_bytes: usize,
    dynamic_colors: DynamicColors,
    synchronized_output_snapshot: Option<SynchronizedOutputSnapshot>,
}

impl Terminal {
    /// Create a new terminal with the specified dimensions
    pub fn new(cols: usize, rows: usize) -> Self {
        let mut tabs = vec![false; cols];
        // Set default tab stops every 8 columns
        for i in (TAB_INTERVAL..cols).step_by(TAB_INTERVAL) {
            tabs[i] = true;
        }
        let mut mode = TerminalMode::default();
        mode.insert(TerminalMode::AUTOWRAP | TerminalMode::CURSOR_VISIBLE);
        let grid = Grid::new(cols, rows);
        let mut alt_grid = Grid::new(cols, rows);
        // Alternate-screen applications own a viewport, not a second history
        // archive. Keeping both limits at zero also prevents a terminal from
        // retaining twice its configured per-terminal scrollback budget.
        alt_grid.set_scrollback_limit(0);
        alt_grid.set_scrollback_byte_limit(0);

        Self {
            term_name: crate::pty::TERMERIC_TERM,
            grid,
            alt_grid,
            alt_screen_active: false,
            mode,
            use_8bit_controls: false,
            dec_conformance_level: 4,
            cursor_style: None,
            cursor_blink_override: None,
            extra_cursors: ExtraCursorState::new(cols, rows),
            keyboard: keyboard::KittyKeyboardState::default(),
            alt_keyboard: keyboard::KittyKeyboardState::default(),
            pointer_shapes: pointer::PointerShapeStack::default(),
            alt_pointer_shapes: pointer::PointerShapeStack::default(),
            xterm_keyboard_type: XtermKeyboardType::default(),
            initial_xterm_keyboard_type: XtermKeyboardType::default(),
            saved_xterm_keyboard_type: None,
            xterm_user_defined_keys: XtermUserDefinedKeys::default(),
            xterm_keyboard_resource: XtermKeyboardResource::default(),
            xterm_key_resources: XtermKeyResources::default(),
            xterm_modify_other_keys: XtermModifyOtherKeys::default(),
            tabs,
            saved_cursor: None,
            alt_saved_cursor: None,
            current_attrs: CellAttrs::empty(),
            current_character_protected: false,
            rectangular_change_extent: false,
            screen_checksum_reporting: false,
            g_sets: [CharacterSet::Ascii; 4],
            gl_register: 0,
            gr_register: 2,
            single_shift: None,
            last_graphic: None,
            current_fg: Color::default_fg(),
            current_bg: Color::default_bg(),
            current_fg_spec: ColorSpec::Default,
            current_bg_spec: ColorSpec::Default,
            current_underline_color: None,
            current_underline_color_spec: None,
            current_hyperlink_id: None,
            hyperlink_registry: HyperlinkRegistry::new(),
            hyperlink_gc_evictions: 0,
            selection: None,
            navigation_cursor: None,
            scroll_top: 0,
            scroll_bottom: rows,
            scroll_left: 0,
            scroll_right: cols,
            processor: Processor::new(),
            cols,
            rows,
            reported_working_directory: None,
            notification_protocol: NotificationProtocol::default(),
            application_titles: ApplicationTitles::default(),
            grid_pixel_width: u32::try_from(cols)
                .unwrap_or(u32::MAX)
                .saturating_mul(8)
                .max(1),
            grid_pixel_height: u32::try_from(rows)
                .unwrap_or(u32::MAX)
                .saturating_mul(16)
                .max(1),
            cell_pixel_width: 8,
            cell_pixel_height: 16,
            graphics: GraphicsManager::new(),
            graphics_apc_data: Vec::new(),
            graphics_apc_active: false,
            graphics_apc_overflow: false,
            graphics_relay_import_allowed: false,
            graphics_relay_apc_data: Vec::new(),
            graphics_relay_apc_active: false,
            graphics_relay_apc_overflow: false,
            pending_graphics_relay_import: None,
            prompt_tracker: PromptTracker::new(),
            prompt_navigation: None,
            prompt_start_cursor: None,
            command_start_awaits_input: false,
            pending_events: Vec::new(),
            pending_notification_icon_event_bytes: 0,
            dynamic_colors: DynamicColors::default(),
            synchronized_output_snapshot: None,
        }
    }

    /// Set the exact terminal-database identity exported to the attached PTY.
    pub fn set_term_name(&mut self, term_name: &'static str) {
        self.term_name = term_name;
    }

    /// Exact terminal-database identity reported through XTGETTCAP TN/name.
    pub fn term_name(&self) -> &'static str {
        self.term_name
    }

    /// Last valid OSC 7 working-directory report for this terminal owner.
    pub fn reported_working_directory(&self) -> Option<&ReportedWorkingDirectory> {
        self.reported_working_directory.as_ref()
    }

    /// OSC 7 metadata corresponding to the displayed content, including an
    /// absent report at the start of a synchronized-output snapshot.
    pub fn presentation_working_directory(&self) -> Option<&ReportedWorkingDirectory> {
        self.synchronized_output_snapshot.as_ref().map_or_else(
            || self.reported_working_directory(),
            |snapshot| snapshot.reported_working_directory.as_ref(),
        )
    }

    pub(crate) fn set_reported_working_directory(&mut self, directory: ReportedWorkingDirectory) {
        self.reported_working_directory = Some(directory);
    }

    /// Last application-authored OSC 1 icon label. This never reflects UI or
    /// native-window metadata.
    #[cfg(test)]
    pub(crate) fn application_icon_name(&self) -> &str {
        self.application_titles.icon()
    }

    /// Last application-authored OSC 0/2 window label. This never reflects a
    /// user-renamed tab, process-derived label, or working directory.
    #[cfg(test)]
    pub(crate) fn application_window_title(&self) -> &str {
        self.application_titles.window()
    }

    pub(crate) fn set_application_icon_name(&mut self, icon: String) {
        self.application_titles.set_icon(icon);
    }

    pub(crate) fn set_application_window_title(&mut self, window: String) {
        if self.application_titles.set_window(window) {
            self.pending_events.push(TerminalEvent::TitleChanged(
                self.application_titles.window().to_string(),
            ));
        }
    }

    pub(crate) fn set_application_title_and_icon(&mut self, title: String) {
        if self.application_titles.set_both(title) {
            self.pending_events.push(TerminalEvent::TitleChanged(
                self.application_titles.window().to_string(),
            ));
        }
    }

    /// Parse one Kitty OSC 99 fragment and emit only the subset advertised by
    /// our capability response. Replies and completed notifications remain
    /// owned by this exact terminal.
    pub(crate) fn handle_osc99_notification(&mut self, params: &[&[u8]]) {
        match self.notification_protocol.handle(params) {
            Osc99Action::None => {}
            Osc99Action::Query(identifier) => {
                let capabilities = "p=title,body,close,alive,buttons,icon:a=report,focus:c=1:o=always,unfocused,invisible:s=system,silent,error,warn,warning,info,question:u=0,1,2:w=1";
                let response = format!("\x1b]99;i={identifier}:p=?;{capabilities}\x1b\\");
                self.push_event(TerminalEvent::PtyWrite(response.into_bytes()));
            }
            Osc99Action::AliveQuery(identifier) => {
                self.push_event(TerminalEvent::NotificationAliveQuery {
                    identifier,
                    reply_uses_8bit_controls: self.use_8bit_controls,
                });
            }
            Osc99Action::Notify(mut request) => {
                request.reply_uses_8bit_controls = self.use_8bit_controls;
                self.push_event(TerminalEvent::Notification(request));
            }
            Osc99Action::Close(identifier) => {
                self.push_event(TerminalEvent::NotificationClose { identifier });
            }
        }
    }

    /// Parse and place one bounded Kitty OSC 66 text-sizing operation.
    pub(crate) fn handle_osc66_multicell(&mut self, params: &[&[u8]]) {
        let Some(command) = Osc66Command::parse(params) else {
            return;
        };
        if command.width == 0 {
            for grapheme in command.text.graphemes(true) {
                let grapheme_width = crate::unicode::grapheme_width(grapheme);
                if grapheme_width == 0 {
                    continue;
                }
                let width = grapheme_width.clamp(1, 2) as u8;
                if !self.place_multicell(&command, grapheme, width) {
                    break;
                }
            }
        } else {
            self.place_multicell(&command, &command.text, command.width);
        }
    }

    fn place_multicell(&mut self, command: &Osc66Command, text: &str, width: u8) -> bool {
        let Some(leader_descriptor) = MultiCell::new(
            command.scale,
            width,
            command.numerator,
            command.denominator,
            command.horizontal,
            command.vertical,
            0,
            0,
        ) else {
            return false;
        };
        let columns = leader_descriptor.columns();
        let rows = leader_descriptor.rows();
        let cols = self.cols;
        if columns > cols || rows > self.rows {
            return false;
        }

        let Some((top, left)) = self.prepare_multicell_position(columns, rows) else {
            return false;
        };
        let bottom = top + rows;
        let right = left + columns;
        let blank = self.erase_cell();

        // Resolve every collision before mutating any cell. This ensures a
        // partial intersection cannot leave a stale descriptor behind.
        let mut collisions = HashSet::new();
        for row in top..bottom {
            for col in left..right {
                if let Some(region) = self.grid().multicell_region_at(row, col) {
                    collisions.insert((region.top, region.left));
                }
            }
        }
        for (row, col) in collisions {
            let replace_with_spaces = !(top <= row && row < bottom && left <= col && col < right);
            let (grid, damage) = self.grid_with_selection_damage();
            grid.clear_multicell_at_with_damage(row, col, &blank, replace_with_spaces, damage);
        }

        if self.mode.contains(TerminalMode::INSERT) {
            // Match ordinary insert-mode printing: shifted selections are
            // retired, not remapped. Block erasure can extend beyond this span.
            self.invalidate_selection_in_region(top..bottom, left..cols);
            let (grid, mut damage) = self.grid_with_selection_damage();
            grid.erase_multicells_with_damage(top..bottom, left..cols, &blank, false, &mut damage);
            for row_index in top..bottom {
                if let Some(row) = grid.row_mut(row_index) {
                    row.insert_cells_bounded_with_damage(left, cols, columns, &blank, |cols| {
                        damage(row_index..row_index + 1, cols)
                    });
                }
            }
        }

        let mut template = Cell {
            fg: self.current_fg,
            bg: self.current_bg,
            fg_spec: self.current_fg_spec,
            bg_spec: self.current_bg_spec,
            underline_color: self.current_underline_color,
            underline_color_spec: self.current_underline_color_spec,
            attrs: self.current_attrs,
            hyperlink_id: self.current_hyperlink_id,
            ..Cell::default()
        };
        template
            .flags
            .set(CellFlags::HYPERLINK, self.current_hyperlink_id.is_some());
        template
            .flags
            .set(CellFlags::PROTECTED, self.current_character_protected);

        self.invalidate_selection_in_region(top..bottom, left..right);
        for relative_row in 0..rows {
            for relative_col in 0..columns {
                let Some(descriptor) = leader_descriptor.at(relative_col, relative_row) else {
                    return false;
                };
                let Some(cell) = self
                    .grid_mut()
                    .cell_mut(top + relative_row, left + relative_col)
                else {
                    return false;
                };
                *cell = template.clone();
                cell.multicell = descriptor;
                if relative_row == 0 && relative_col == 0 {
                    let mut scalars = text.chars();
                    cell.content = match (scalars.next(), scalars.next()) {
                        (Some(character), None) => CellContent::Char(character),
                        (Some(_), Some(_)) => CellContent::Grapheme(text.into()),
                        _ => return false,
                    };
                }
            }
            // Replacing one ordinary wide half with a multicell must remove
            // the other half, even when it lies outside the placed rectangle.
            let row_index = top + relative_row;
            let (grid, mut damage) = self.grid_with_selection_damage();
            if let Some(row) = grid.row_mut(row_index) {
                row.repair_multicell_placement_edges(left, right, &blank, |cols| {
                    damage(row_index..row_index + 1, cols)
                });
            }
        }

        if right == cols {
            self.grid_mut().set_cursor(top, cols - 1);
            self.grid_mut().set_wrap_pending(true);
        } else {
            self.grid_mut().set_cursor(top, right);
        }
        self.remember_last_graphic(text, CharacterSet::Ascii);
        true
    }

    fn prepare_multicell_position(
        &mut self,
        columns: usize,
        rows: usize,
    ) -> Option<(usize, usize)> {
        let cols = self.cols;
        for _ in 0..=cols {
            let cursor = self.grid().cursor();
            let (line_left, line_right) = self.write_horizontal_bounds(cursor.row, cursor.col);
            if columns > line_right.saturating_sub(line_left) {
                return None;
            }
            if cursor.col < line_left {
                self.grid_mut().set_cursor(cursor.row, line_left);
                continue;
            }
            if cursor.wrap_pending || cursor.col + columns > line_right {
                if self.mode.contains(TerminalMode::AUTOWRAP) || cursor.wrap_pending {
                    self.multicell_wrap_to_next_line(true);
                } else {
                    self.grid_mut()
                        .set_cursor(cursor.row, line_right.saturating_sub(columns));
                }
                continue;
            }

            let skip_to = (cursor.col..cursor.col + columns)
                .filter_map(|col| self.grid().multicell_region_at(cursor.row, col))
                .filter(|region| {
                    self.grid()
                        .cell(cursor.row, cursor.col.max(region.left))
                        .is_some_and(|cell| cell.multicell.relative_row() > 0)
                })
                .map(|region| region.right())
                .max();
            if let Some(next_col) = skip_to {
                if next_col + columns <= line_right {
                    self.grid_mut().set_cursor(cursor.row, next_col);
                } else {
                    self.multicell_wrap_to_next_line(false);
                }
                continue;
            }

            let mut top = cursor.row;
            if self.row_inside_vertical_margins(top) {
                let height = self.scroll_bottom - self.scroll_top;
                if rows > height {
                    return None;
                }
                let overflow = top.saturating_add(rows).saturating_sub(self.scroll_bottom);
                if overflow > 0 {
                    self.scroll_up(overflow);
                    top = top.saturating_sub(overflow);
                    self.grid_mut().set_cursor(top, cursor.col);
                }
            } else if top + rows > self.rows {
                return None;
            }
            return Some((top, self.grid().cursor().col));
        }
        None
    }

    fn multicell_wrap_to_next_line(&mut self, wrapped: bool) {
        let cursor = self.grid().cursor();
        let (left, right) = self.write_horizontal_bounds(cursor.row, cursor.col);
        let full_width = left == 0 && right == self.cols;
        if let Some(row) = self.grid_mut().row_mut(cursor.row) {
            row.set_wrapped(wrapped && full_width);
        }
        self.grid_mut().set_cursor(cursor.row, left);
        self.linefeed();
    }

    /// Get the active grid
    pub fn grid(&self) -> &Grid {
        if self.alt_screen_active {
            &self.alt_grid
        } else {
            &self.grid
        }
    }

    /// Grid that is safe to present to the user. While synchronized output is
    /// active this remains the viewport captured at DECSET 2026, even though
    /// the live grid continues processing PTY bytes.
    pub fn presentation_grid(&self) -> &Grid {
        self.synchronized_output_snapshot
            .as_ref()
            .map_or_else(|| self.grid(), |snapshot| &snapshot.grid)
    }

    /// Mutable displayed grid for user navigation only. Parser/output updates
    /// must use `grid_mut()` so synchronized output remains isolated. The
    /// frozen grid contains just the captured viewport, not hidden history.
    pub fn presentation_grid_mut(&mut self) -> &mut Grid {
        if let Some(ref mut snapshot) = self.synchronized_output_snapshot {
            &mut snapshot.grid
        } else if self.alt_screen_active {
            &mut self.alt_grid
        } else {
            &mut self.grid
        }
    }

    /// Whether the terminal is currently presenting its alternate screen.
    #[cfg(test)]
    pub(crate) fn alternate_screen_active(&self) -> bool {
        self.alt_screen_active
    }

    /// Screen identity corresponding to [`Self::presentation_grid`].
    pub fn presentation_alternate_screen_active(&self) -> bool {
        self.synchronized_output_snapshot
            .as_ref()
            .map_or(self.alt_screen_active, |snapshot| snapshot.alternate_screen)
    }

    /// Return the newest presentation text within explicit row and UTF-8 byte
    /// budgets. Primary-screen history is included oldest-to-newest before the
    /// visible viewport; alternate screens never expose primary history.
    ///
    /// The presentation grid is used intentionally so a query cannot reveal
    /// output hidden behind synchronized-output mode. When the byte budget is
    /// reached, older content is discarded first and the returned string
    /// always begins on a UTF-8 boundary.
    pub fn bounded_presentation_text(
        &self,
        history_lines: usize,
        max_bytes: usize,
    ) -> (String, usize, bool) {
        let grid = self.presentation_grid();
        let history_lines = if self.presentation_alternate_screen_active() {
            0
        } else {
            history_lines
                .min(grid.scrollback_len())
                .min(i32::MAX as usize)
        };
        let start = -(history_lines as i32);
        let end = self.rows.saturating_sub(1) as i32;
        let mut newest_first = Vec::new();
        let mut used = 0_usize;
        let mut included_history = 0_usize;
        let mut truncated = false;

        for row_index in (start..=end).rev() {
            let line = grid
                .row_at_offset(row_index)
                .map(|row| row.plain_text(self.cols))
                .unwrap_or_default();
            let line = line.trim_end();
            if line.is_empty() && newest_first.is_empty() {
                continue;
            }
            let separator = usize::from(!newest_first.is_empty());
            let prefix = used.saturating_add(separator);
            let remaining = max_bytes.saturating_sub(prefix);
            if prefix <= max_bytes && line.len() <= remaining {
                newest_first.push(line.to_string());
                used = used.saturating_add(separator).saturating_add(line.len());
                if row_index < 0 {
                    included_history += 1;
                }
                continue;
            }

            truncated = true;
            if remaining > 0 {
                let start_byte = crate::safe_text::ceil_char_boundary(
                    line,
                    line.len().saturating_sub(remaining),
                );
                if start_byte < line.len() {
                    newest_first.push(line[start_byte..].to_string());
                    if row_index < 0 {
                        included_history += 1;
                    }
                }
            }
            break;
        }

        newest_first.reverse();
        (newest_first.join("\n"), included_history, truncated)
    }

    /// Get the active grid mutably
    pub fn grid_mut(&mut self) -> &mut Grid {
        if self.alt_screen_active {
            &mut self.alt_grid
        } else {
            &mut self.grid
        }
    }

    /// Get terminal mode flags
    pub fn mode(&self) -> TerminalMode {
        self.mode
    }

    /// Current application-selected mouse pointer shape for the active screen.
    pub(crate) fn pointer_shape(&self) -> Option<PointerShape> {
        if self.alt_screen_active {
            self.alt_pointer_shapes.current()
        } else {
            self.pointer_shapes.current()
        }
    }

    /// Pointer shape presented when no Termeric-owned interaction overrides
    /// the terminal. Mouse-reporting mode uses the documented grabbed policy.
    pub fn effective_pointer_shape(&self) -> PointerShape {
        self.pointer_shape().unwrap_or_else(|| {
            if self.mode.has_mouse_mode() {
                PointerShape::Default
            } else {
                PointerShape::Text
            }
        })
    }

    fn active_pointer_shapes_mut(&mut self) -> &mut pointer::PointerShapeStack {
        if self.alt_screen_active {
            &mut self.alt_pointer_shapes
        } else {
            &mut self.pointer_shapes
        }
    }

    fn emit_pointer_shape_change(&mut self, previous: PointerShape) {
        if self.effective_pointer_shape() != previous {
            self.push_event(TerminalEvent::PointerShapeChanged);
        }
    }

    /// Apply one bounded Kitty OSC 22 pointer-shape operation.
    pub(crate) fn handle_pointer_shape_osc(&mut self, payload: &[u8]) {
        use std::fmt::Write as _;

        if payload.len() > pointer::MAX_POINTER_SHAPE_PAYLOAD_BYTES {
            return;
        }
        match payload.first().copied() {
            Some(b'?') => {
                let query = &payload[1..];
                if query.is_empty() {
                    return;
                }
                let mut response = String::with_capacity(
                    query
                        .len()
                        .min(pointer::MAX_POINTER_SHAPE_LIST_ENTRIES.saturating_mul(14)),
                );
                for (count, name) in query.split(|byte| *byte == b',').enumerate() {
                    if count == pointer::MAX_POINTER_SHAPE_LIST_ENTRIES
                        || !pointer::valid_query_name(name)
                    {
                        return;
                    }
                    if count > 0 {
                        response.push(',');
                    }
                    match name {
                        b"__current__" => {
                            response.push_str(self.pointer_shape().map_or("0", PointerShape::name))
                        }
                        b"__default__" => response.push_str(PointerShape::Text.name()),
                        b"__grabbed__" => response.push_str(PointerShape::Default.name()),
                        _ => response.push(if PointerShape::from_name(name).is_some() {
                            '1'
                        } else {
                            '0'
                        }),
                    }
                }
                let mut reply = String::with_capacity(response.len().saturating_add(9));
                let _ = write!(reply, "\x1b]22;{response}\x1b\\");
                self.push_event(TerminalEvent::PtyWrite(reply.into_bytes()));
            }
            Some(b'>') => {
                let Some((shapes, len)) = pointer::parse_shape_list(&payload[1..]) else {
                    return;
                };
                let previous = self.effective_pointer_shape();
                for shape in &shapes[..len] {
                    self.active_pointer_shapes_mut().push(*shape);
                }
                self.emit_pointer_shape_change(previous);
            }
            Some(b'<') => {
                let previous = self.effective_pointer_shape();
                self.active_pointer_shapes_mut().pop();
                self.emit_pointer_shape_change(previous);
            }
            Some(b'=') => {
                self.set_or_reset_pointer_shape(&payload[1..]);
            }
            _ => self.set_or_reset_pointer_shape(payload),
        }
    }

    fn set_or_reset_pointer_shape(&mut self, name: &[u8]) {
        let previous = self.effective_pointer_shape();
        if name.is_empty() {
            self.active_pointer_shapes_mut().clear();
        } else if let Some(shape) = PointerShape::from_name(name) {
            self.active_pointer_shapes_mut().set(shape);
        } else {
            return;
        }
        self.emit_pointer_shape_change(previous);
    }

    /// Get the application-selected cursor presentation, if any.
    pub fn cursor_style(&self) -> Option<TerminalCursorStyle> {
        self.cursor_style
    }

    /// Cursor presentation corresponding to [`Self::presentation_grid`].
    pub fn presentation_cursor_style(&self) -> Option<TerminalCursorStyle> {
        self.synchronized_output_snapshot
            .as_ref()
            .map_or(self.cursor_style, |snapshot| snapshot.cursor_style)
    }

    /// Application-selected cursor blink policy, when mode 12 has explicitly
    /// overridden the user's configured default.
    pub fn cursor_blink_override(&self) -> Option<bool> {
        self.cursor_blink_override
    }

    /// Cursor blink policy corresponding to [`Self::presentation_grid`].
    pub fn presentation_cursor_blink_override(&self) -> Option<bool> {
        self.synchronized_output_snapshot
            .as_ref()
            .map_or(self.cursor_blink_override, |snapshot| {
                snapshot.cursor_blink_override
            })
    }

    /// Apply DECSCUSR. Ps 0 hands the cursor back to the user's configured
    /// shape and blink, as foot and kitty do, so a program's terminfo `Se`
    /// restores the person's choice rather than xterm's blinking block.
    /// Invalid parameters leave the current style unchanged.
    pub(crate) fn set_cursor_style(&mut self, parameter: u16) {
        if parameter == 0 {
            self.cursor_style = None;
            self.cursor_blink_override = None;
            self.set_mode(TerminalMode::CURSOR_BLINKING, false);
        } else if let Some(style) = TerminalCursorStyle::from_parameter(parameter) {
            self.cursor_style = Some(style);
            self.cursor_blink_override = None;
            self.set_mode(TerminalMode::CURSOR_BLINKING, style.blinking);
        }
    }

    pub fn cursor_style_parameter(&self) -> u8 {
        self.cursor_style.map_or(0, TerminalCursorStyle::parameter)
    }

    /// Get the Kitty keyboard enhancements active for the current screen.
    pub fn kitty_keyboard_flags(&self) -> KittyKeyboardFlags {
        self.keyboard_state().flags()
    }

    fn keyboard_state(&self) -> &keyboard::KittyKeyboardState {
        if self.alt_screen_active {
            &self.alt_keyboard
        } else {
            &self.keyboard
        }
    }

    fn keyboard_state_mut(&mut self) -> &mut keyboard::KittyKeyboardState {
        if self.alt_screen_active {
            &mut self.alt_keyboard
        } else {
            &mut self.keyboard
        }
    }

    pub(crate) fn set_kitty_keyboard_flags(&mut self, flags: u16, mode: u16) {
        self.keyboard_state_mut().set(flags, mode);
    }

    pub(crate) fn push_kitty_keyboard_flags(&mut self, flags: u16) {
        self.keyboard_state_mut().push(flags);
    }

    pub(crate) fn pop_kitty_keyboard_flags(&mut self, count: usize) {
        self.keyboard_state_mut().pop(count);
    }

    /// Get the selection belonging to the displayed grid.
    pub fn selection(&self) -> Option<&Selection> {
        self.synchronized_output_snapshot.as_ref().map_or_else(
            || self.selection.as_ref(),
            |snapshot| snapshot.selection.as_ref(),
        )
    }

    /// Set a user selection in presentation-grid coordinates. Frozen selections
    /// never become live-grid coordinates after the snapshot is released.
    pub fn set_selection(&mut self, selection: Option<Selection>) {
        if let Some(snapshot) = &mut self.synchronized_output_snapshot {
            snapshot.selection = selection;
            self.selection = None;
        } else {
            self.selection = selection;
        }
    }

    /// Register an invisible live-grid modal cursor. It follows output moves
    /// and disappears on actual damage/eviction; it is never a copy selection.
    /// Width resize retires it: callers use a scoped grid resize transaction.
    /// Frozen presentation coordinates must not be registered on live state.
    pub fn set_navigation_cursor(&mut self, point: Option<Point>) {
        self.navigation_cursor = point
            .filter(|point| {
                self.synchronized_output_snapshot.is_none()
                    && self
                        .grid()
                        .row_at_offset(point.row)
                        .is_some_and(|row| point.col < row.len())
            })
            .map(|point| Selection::new(point, SelectionType::Simple));
    }

    pub fn navigation_cursor(&self) -> Option<Point> {
        self.navigation_cursor
            .as_ref()
            .map(|selection| selection.start)
    }

    fn selection_for_snapshot(&self) -> Option<Selection> {
        let mut selection = self.selection.clone()?;
        let grid = self.grid();
        let offset = i32::try_from(grid.scroll_offset()).ok()?;
        let first = -offset;
        let last = (grid.rows().saturating_sub(1) as i32).saturating_sub(offset);
        let max_col = grid.cols().saturating_sub(1);
        let (mut start, mut end) = selection.bounds();
        if end.row < first || start.row > last {
            return None;
        }
        if selection.ty == SelectionType::Block {
            start.row = start.row.max(first);
            end.row = end.row.min(last);
        } else {
            start = start.max(Point::new(first, 0));
            end = end.min(Point::new(last, max_col));
        }
        start.row = start.row.saturating_add(offset);
        end.row = end.row.saturating_add(offset);
        start.col = start.col.min(max_col);
        end.col = end.col.min(max_col);
        let reversed = selection.start > selection.end;
        selection.start = if reversed { end } else { start };
        selection.end = if reversed { start } else { end };
        selection.finish();
        Some(selection)
    }

    /// Get the number of columns
    pub fn cols(&self) -> usize {
        self.cols
    }

    /// Get the number of rows
    pub fn rows(&self) -> usize {
        self.rows
    }

    /// Apply line and byte budgets to primary history. The alternate screen is
    /// viewport-only and therefore always retains zero archived rows.
    pub fn set_scrollback_limits(&mut self, lines: usize, bytes: usize) {
        self.invalidate_prompt_navigation_after_manual_scroll();
        self.grid.set_scrollback_limit(lines);
        self.grid.set_scrollback_byte_limit(bytes);
        self.alt_grid.set_scrollback_limit(0);
        self.alt_grid.set_scrollback_byte_limit(0);
        self.graphics
            .prune_scrollback(false, self.grid.scrollback_len());
        let oldest = -(self.grid.scrollback_len() as i32);
        self.prompt_tracker.adjust_for_scroll(0, oldest);
        self.retain_prompt_navigation(oldest);
        self.clear_selection_if_evicted();
        self.prune_unreferenced_hyperlinks();
    }

    /// Clear saved lines from the active screen and retire graphics placements
    /// and primary-shell metadata whose backing history was removed.
    pub fn clear_scrollback(&mut self) {
        self.grid_mut().clear_scrollback();
        self.graphics.prune_scrollback(self.alt_screen_active, 0);
        self.clear_selection_if_evicted();
        if !self.alt_screen_active {
            self.prompt_tracker.adjust_for_scroll(0, 0);
            self.retain_prompt_navigation(0);
        }
    }

    /// Return a categorized estimate of terminal-owned row and hyperlink memory.
    pub fn memory_usage(&self) -> TerminalMemoryUsage {
        let primary = self.grid.memory_usage();
        let alternate = self.alt_grid.memory_usage();
        TerminalMemoryUsage {
            visible_bytes: primary.visible_bytes + alternate.visible_bytes,
            scrollback_bytes: primary.scrollback_bytes + alternate.scrollback_bytes,
            grapheme_bytes: primary.grapheme_bytes + alternate.grapheme_bytes,
            style_bytes: primary.style_bytes + alternate.style_bytes,
            hyperlink_bytes: primary.hyperlink_reference_bytes
                + alternate.hyperlink_reference_bytes
                + self.hyperlink_registry.memory_usage_bytes(),
            extra_cursor_bytes: self.extra_cursors.owned_bytes()
                + self
                    .synchronized_output_snapshot
                    .as_ref()
                    .map_or(0, |snapshot| snapshot.extra_cursors.owned_bytes()),
            scrollback_lines: primary.scrollback_lines + alternate.scrollback_lines,
            scrollback_line_limit: primary.scrollback_line_limit,
            scrollback_byte_limit: primary.scrollback_byte_limit,
        }
    }

    fn clear_selection_if_evicted(&mut self) {
        let oldest_row = -(self.grid().scrollback_len() as i32);
        for slot in [&mut self.selection, &mut self.navigation_cursor] {
            if slot.as_ref().is_some_and(|selection| {
                selection.start.row < oldest_row || selection.end.row < oldest_row
            }) {
                *slot = None;
            }
        }
    }

    fn shift_selection_after_full_scroll(&mut self, lines: usize) {
        let lines = i32::try_from(lines).unwrap_or(i32::MAX);
        for selection in [&mut self.selection, &mut self.navigation_cursor]
            .into_iter()
            .flatten()
        {
            selection.start.row = selection.start.row.saturating_sub(lines);
            selection.end.row = selection.end.row.saturating_sub(lines);
        }
        self.clear_selection_if_evicted();
    }

    fn shift_selection_in_region(
        &mut self,
        rows: std::ops::Range<usize>,
        cols: std::ops::Range<usize>,
        delta: i32,
    ) {
        // Only live state follows output; a synchronized snapshot owns its
        // independent visible selection until the snapshot is released.
        for slot in [&mut self.selection, &mut self.navigation_cursor] {
            if let Some(selection) = slot
                && !selection.scroll_in_region(rows.clone(), cols.clone(), delta, self.cols)
            {
                *slot = None;
            }
        }
    }

    fn note_history_eviction(&mut self, evicted: usize) {
        if evicted == 0 {
            return;
        }
        self.hyperlink_gc_evictions = self.hyperlink_gc_evictions.saturating_add(evicted);
        if self.hyperlink_gc_evictions >= 256 {
            self.prune_unreferenced_hyperlinks();
            self.hyperlink_gc_evictions = 0;
        }
    }

    fn prune_unreferenced_hyperlinks(&mut self) {
        let mut retained = HashSet::new();
        self.grid.collect_hyperlink_ids(&mut retained);
        self.alt_grid.collect_hyperlink_ids(&mut retained);
        if let Some(snapshot) = &self.synchronized_output_snapshot {
            snapshot.grid.collect_hyperlink_ids(&mut retained);
        }
        if let Some(identifier) = self.current_hyperlink_id {
            retained.insert(identifier);
        }
        self.hyperlink_registry.retain_ids(&retained);
    }

    /// Push a terminal event to be handled by the application. `PtyWrite`
    /// payloads are authored canonically with 7-bit introducers and framed at
    /// this boundary according to the current S7C1T/S8C1T preference.
    pub(crate) fn push_event(&mut self, event: TerminalEvent) {
        let event = match event {
            TerminalEvent::PtyWrite(reply) => TerminalEvent::PtyWrite(self.frame_pty_reply(&reply)),
            TerminalEvent::Notification(mut request) => {
                if let Some(icon) = request.icon_data.as_ref() {
                    if self
                        .pending_notification_icon_event_bytes
                        .saturating_add(icon.len())
                        > MAX_PENDING_NOTIFICATION_ICON_EVENT_BYTES
                    {
                        request.icon_data = None;
                        request.icon_cache_id = None;
                    } else {
                        self.pending_notification_icon_event_bytes += icon.len();
                    }
                }
                TerminalEvent::Notification(request)
            }
            event => event,
        };
        self.pending_events.push(event);
    }

    /// Frame a canonical 7-bit terminal reply for the active C1 transmission
    /// mode. This is public so app-mediated OSC replies use the exact same
    /// terminal-local contract as core-generated CSI/DCS/APC replies.
    pub fn frame_pty_reply(&self, canonical: &[u8]) -> Vec<u8> {
        Self::frame_pty_reply_for_mode(canonical, self.use_8bit_controls)
    }

    /// Frame a canonical reply using the mode captured when its request was
    /// parsed. App-mediated replies use this to preserve PTY protocol order.
    pub fn frame_pty_reply_for_mode(canonical: &[u8], use_8bit_controls: bool) -> Vec<u8> {
        if !use_8bit_controls || canonical.len() < 2 || canonical[0] != 0x1b {
            return canonical.to_vec();
        }

        let introducer = match canonical[1] {
            b'[' => 0x9b,
            b'P' => 0x90,
            b']' => 0x9d,
            b'_' => 0x9f,
            _ => return canonical.to_vec(),
        };
        let has_string_terminator =
            matches!(introducer, 0x90 | 0x9d | 0x9f) && canonical.ends_with(b"\x1b\\");
        let body_end = canonical.len() - usize::from(has_string_terminator) * 2;
        let mut framed = Vec::with_capacity(canonical.len());
        framed.push(introducer);
        framed.extend_from_slice(&canonical[2..body_end]);
        if has_string_terminator {
            framed.push(0x9c);
        }
        framed
    }

    pub fn uses_8bit_controls(&self) -> bool {
        self.use_8bit_controls
    }

    pub(crate) fn set_8bit_controls(&mut self, enabled: bool) {
        self.use_8bit_controls = enabled;
    }

    /// Select the bounded DECSCL operating profile.
    ///
    /// Levels 1 through 5 are accepted as parameters 61 through 65. Level 1
    /// always transmits 7-bit controls; higher levels use 8-bit controls when
    /// the optional second parameter is absent, 0, or 2, and 7-bit controls
    /// for parameter 1. Invalid input is rejected atomically.
    pub(crate) fn select_conformance_level(
        &mut self,
        level_parameter: u16,
        control_parameter: Option<u16>,
    ) {
        let Ok(level) = u8::try_from(level_parameter.saturating_sub(60)) else {
            return;
        };
        if !(1..=5).contains(&level) {
            return;
        }

        let use_8bit_controls = if level == 1 {
            false
        } else {
            match control_parameter.unwrap_or(0) {
                0 | 2 => true,
                1 => false,
                _ => return,
            }
        };

        // DEC STD 070 and the VT220 documentation define DECSCL as a soft
        // reset. Apply that reset before installing the newly selected state.
        self.soft_reset();
        self.dec_conformance_level = level;
        self.use_8bit_controls = use_8bit_controls;
    }

    pub(crate) fn dec_conformance_level(&self) -> u8 {
        self.dec_conformance_level
    }

    /// Drain all pending events
    pub fn drain_events(&mut self) -> Vec<TerminalEvent> {
        self.pending_notification_icon_event_bytes = 0;
        std::mem::take(&mut self.pending_events)
    }

    pub fn dynamic_colors(&self) -> &DynamicColors {
        &self.dynamic_colors
    }

    /// Terminal-local colors corresponding to [`Self::presentation_grid`].
    pub fn presentation_dynamic_colors(&self) -> &DynamicColors {
        self.synchronized_output_snapshot
            .as_ref()
            .map_or(&self.dynamic_colors, |snapshot| &snapshot.dynamic_colors)
    }

    pub fn apply_dynamic_color(&mut self, operation: crate::event::DynamicColorOperation) {
        use crate::event::DynamicColorOperation;
        match operation {
            DynamicColorOperation::Set { target, color } => self.dynamic_colors.set(target, color),
            DynamicColorOperation::Reset(target) => self.dynamic_colors.reset(target),
            DynamicColorOperation::ResetPalette => self.dynamic_colors.reset_palette(),
            DynamicColorOperation::Query { .. } => {}
        }
    }

    pub(crate) fn designate_character_set(&mut self, register: u8, designator: u8) {
        let Some(charset) = CharacterSet::from_designator(designator) else {
            return;
        };
        if let Some(slot) = self.g_sets.get_mut(usize::from(register)) {
            *slot = charset;
        }
    }

    pub(crate) fn invoke_gl(&mut self, register: u8) {
        if register < self.g_sets.len() as u8 {
            self.gl_register = register;
        }
    }

    pub(crate) fn invoke_gr(&mut self, register: u8) {
        if (1..self.g_sets.len() as u8).contains(&register) {
            self.gr_register = register;
        }
    }

    pub(crate) fn single_shift(&mut self, register: u8) {
        if matches!(register, 2 | 3) {
            self.single_shift = Some(register);
        }
    }

    fn active_character_set(&self) -> CharacterSet {
        self.g_sets[usize::from(self.gl_register)]
    }

    pub(crate) fn write_translated_character(&mut self, character: char) {
        let register = self.single_shift.take().unwrap_or(self.gl_register);
        let charset = self.g_sets[usize::from(register)];
        self.write_char_with_charset(charset.translate(character), charset);
    }

    /// Process input bytes from PTY
    pub fn process(&mut self, bytes: &[u8]) {
        // Take the processor temporarily to avoid borrow conflict
        let mut processor = std::mem::take(&mut self.processor);
        processor.process(self, bytes);
        self.processor = processor;
    }

    /// Save or restore xterm's global window/icon title ring.
    pub(crate) fn update_title_stack(&mut self, save: bool, target_parameter: u16, slot: u16) {
        let Some(target) = TitleTarget::from_parameter(target_parameter) else {
            return;
        };
        if slot > XTERM_TITLE_STACK_DEPTH as u16 {
            return;
        }
        if save {
            self.application_titles.save(target, slot);
            return;
        }

        let restored = self.application_titles.restore(target, slot);
        if let Some(icon) = restored.icon {
            self.application_titles.set_icon(icon);
        }
        if let Some(window) = restored.window
            && self.application_titles.set_window(window)
        {
            self.pending_events.push(TerminalEvent::TitleChanged(
                self.application_titles.window().to_string(),
            ));
        }
    }

    fn row_inside_vertical_margins(&self, row: usize) -> bool {
        row >= self.scroll_top && row < self.scroll_bottom
    }

    fn col_inside_horizontal_margins(&self, col: usize) -> bool {
        col >= self.scroll_left && col < self.scroll_right
    }

    fn write_horizontal_bounds(&self, row: usize, col: usize) -> (usize, usize) {
        if self.row_inside_vertical_margins(row) && col < self.scroll_right {
            (self.scroll_left, self.scroll_right)
        } else {
            (0, self.cols)
        }
    }

    fn carriage_return_column(&self, row: usize, col: usize) -> usize {
        if self.row_inside_vertical_margins(row) && col >= self.scroll_left {
            self.scroll_left
        } else {
            0
        }
    }

    /// Write a character at the current cursor position
    pub fn write_char(&mut self, c: char) {
        self.write_char_with_charset(c, CharacterSet::Ascii);
    }

    fn write_char_with_charset(&mut self, c: char, charset: CharacterSet) {
        // VTE delivers Unicode scalars. Re-segment the preceding stored cluster
        // plus this scalar so combining marks, variation selectors, emoji
        // modifiers, regional indicators, and ZWJ sequences remain one cell.
        // Printable ASCII cannot extend an earlier grapheme. Keep this common
        // path allocation-free and avoid running the Unicode segmenter for
        // every byte of ordinary shell output.
        if !c.is_ascii() && self.try_extend_previous_grapheme(c) {
            return;
        }

        let mut encoded = [0; 4];
        let scalar = c.encode_utf8(&mut encoded);
        let mut orphan = String::new();
        let mut width = crate::unicode::grapheme_width(scalar);
        let grapheme = if width == 0 {
            // Defined orphan-combining behavior: compose it over a blank cell
            // rather than losing it or attaching across an explicit cursor
            // movement/control boundary.
            orphan.push(' ');
            orphan.push(c);
            width = 1;
            orphan.as_str()
        } else {
            scalar
        };

        let cursor = self.grid().cursor();
        let cols = self.cols;

        // Handle pending line wrap: if the previous character was written to
        // the last column, wrap_pending is set. Now that a new printable char
        // arrives, perform the actual wrap (mark the row as soft-wrapped,
        // carriage-return, and linefeed).
        if cursor.wrap_pending && self.mode.contains(TerminalMode::AUTOWRAP) {
            let (left, right) = self.write_horizontal_bounds(cursor.row, cursor.col);
            // Partial-width wraps are presentation layout rather than a
            // whole-row logical continuation, so they must not enter reflow.
            if let Some(row) = self.grid_mut().row_mut(cursor.row) {
                row.set_wrapped(left == 0 && right == cols);
            }
            self.grid_mut().set_cursor(cursor.row, left);
            self.linefeed();
        }
        // If not autowrap, cursor stays at last column (overwrite mode)

        width = self.prepare_normal_text_position(width);
        let cursor = self.grid().cursor();
        let (_, right) = self.write_horizontal_bounds(cursor.row, cursor.col);

        let fg = self.current_fg;
        let bg = self.current_bg;
        let fg_spec = self.current_fg_spec;
        let bg_spec = self.current_bg_spec;
        let attrs = self.current_attrs;
        let protected = self.current_character_protected;

        let blank = self.erase_cell();
        if self.mode.contains(TerminalMode::INSERT) {
            self.invalidate_selection_for_write(cursor.row, cursor.col, right);
            let (grid, mut damage) = self.grid_with_selection_damage();
            grid.erase_multicells_with_damage(
                cursor.row..cursor.row + 1,
                cursor.col..right,
                &blank,
                false,
                &mut damage,
            );
            if let Some(row) = grid.row_mut(cursor.row) {
                row.insert_cells_bounded_with_damage(cursor.col, right, width, &blank, |cols| {
                    damage(cursor.row..cursor.row + 1, cols)
                });
            }
        }

        // Clear either half of any existing wide pair, then write the leader
        // and (for width two) an attributed empty spacer atomically.
        let underline_color = self.current_underline_color;
        let underline_color_spec = self.current_underline_color_spec;
        let hyperlink_id = self.current_hyperlink_id;
        self.invalidate_selection_for_write(cursor.row, cursor.col, cursor.col + width);
        if let Some(row) = self.grid_mut().row_mut(cursor.row) {
            row.clear_grapheme_at(cursor.col);
            if width == 2 {
                row.clear_grapheme_at(cursor.col + 1);
            }
            if let Some(cell) = row.get_mut(cursor.col) {
                cell.set_grapheme(grapheme);
                cell.fg = fg;
                cell.bg = bg;
                cell.fg_spec = fg_spec;
                cell.bg_spec = bg_spec;
                cell.underline_color = underline_color;
                cell.underline_color_spec = underline_color_spec;
                cell.attrs = attrs;
                cell.hyperlink_id = hyperlink_id;
                cell.flags.remove(CellFlags::WIDE_CHAR_SPACER);
                cell.flags
                    .remove(CellFlags::DEC_SPECIAL_GRAPHICS | CellFlags::DEC_UK_CHARSET);
                cell.flags.set(
                    CellFlags::DEC_SPECIAL_GRAPHICS,
                    charset == CharacterSet::DecSpecialGraphics,
                );
                cell.flags.set(
                    CellFlags::DEC_UK_CHARSET,
                    charset == CharacterSet::UnitedKingdom,
                );
                cell.flags.set(CellFlags::WIDE_CHAR, width == 2);
                cell.flags.set(CellFlags::HYPERLINK, hyperlink_id.is_some());
                cell.flags.set(CellFlags::PROTECTED, protected);
            }
            if width == 2
                && let Some(spacer) = row.get_mut(cursor.col + 1)
            {
                spacer.content = CellContent::Empty;
                spacer.fg = fg;
                spacer.bg = bg;
                spacer.fg_spec = fg_spec;
                spacer.bg_spec = bg_spec;
                spacer.underline_color = underline_color;
                spacer.underline_color_spec = underline_color_spec;
                spacer.attrs = attrs;
                spacer.hyperlink_id = hyperlink_id;
                spacer.flags.remove(CellFlags::WIDE_CHAR);
                spacer
                    .flags
                    .remove(CellFlags::DEC_SPECIAL_GRAPHICS | CellFlags::DEC_UK_CHARSET);
                spacer.flags.set(
                    CellFlags::DEC_SPECIAL_GRAPHICS,
                    charset == CharacterSet::DecSpecialGraphics,
                );
                spacer.flags.set(
                    CellFlags::DEC_UK_CHARSET,
                    charset == CharacterSet::UnitedKingdom,
                );
                spacer.flags.insert(CellFlags::WIDE_CHAR_SPACER);
                spacer
                    .flags
                    .set(CellFlags::HYPERLINK, hyperlink_id.is_some());
                spacer.flags.set(CellFlags::PROTECTED, protected);
            }
        }

        // Advance cursor. If we just wrote to the last column, set
        // wrap_pending instead of advancing past the edge (which would
        // be clamped by set_cursor anyway).
        if cursor.col + width >= right {
            // Stay at last column but flag that a wrap is pending
            self.grid_mut().set_wrap_pending(true);
        } else {
            self.grid_mut().set_cursor(cursor.row, cursor.col + width);
        }
        self.remember_last_graphic(grapheme, charset);
    }

    fn prepare_normal_text_position(&mut self, mut width: usize) -> usize {
        let cols = self.cols;
        // Normal text cannot split a valid multicell block. Lower rows are
        // skipped, while the leader and other top-row cells use Kitty's
        // erase-versus-space overwrite distinction.
        loop {
            let cursor = self.grid().cursor();
            let (current_left, current_right) =
                self.write_horizontal_bounds(cursor.row, cursor.col);
            if width == 2 && cursor.col + 1 >= current_right {
                if self.mode.contains(TerminalMode::AUTOWRAP) && current_right - current_left >= 2 {
                    if let Some(row) = self.grid_mut().row_mut(cursor.row) {
                        row.set_wrapped(current_left == 0 && current_right == cols);
                        row.mark_wide_wrap_padding();
                    }
                    self.grid_mut().set_cursor(cursor.row, current_left);
                    self.linefeed();
                    continue;
                }
                // A one-column grid cannot represent a wide pair. Keep the
                // grapheme visible in the sole cell without creating an orphan.
                width = 1;
            }
            let collision = (cursor.col..cursor.col.saturating_add(width).min(current_right))
                .find_map(|col| {
                    self.grid()
                        .multicell_region_at(cursor.row, col)
                        .map(|region| (col, region))
                });
            let Some((collision_col, region)) = collision else {
                return width;
            };
            let descriptor = self
                .grid()
                .cell(cursor.row, collision_col)
                .expect("resolved multicell collision is in bounds")
                .multicell;
            if descriptor.relative_row() > 0 {
                if region.right() + width <= current_right {
                    self.grid_mut().set_cursor(cursor.row, region.right());
                } else {
                    self.multicell_wrap_to_next_line(false);
                }
                continue;
            }
            let blank = self.erase_cell();
            self.invalidate_selection_in_region(
                region.top..region.bottom(),
                region.left..region.right(),
            );
            self.grid_mut().clear_multicell_at(
                cursor.row,
                collision_col,
                &blank,
                !descriptor.is_leader(),
            );
        }
    }

    fn remember_last_graphic(&mut self, grapheme: &str, charset: CharacterSet) {
        if grapheme.len() > MAX_REPEAT_GRAPHEME_BYTES {
            self.last_graphic = None;
            return;
        }

        let mut scalars = grapheme.chars();
        self.last_graphic = match (scalars.next(), scalars.next()) {
            (Some(scalar), None) => Some((CellContent::Char(scalar), charset)),
            (Some(_), Some(_)) => Some((CellContent::Grapheme(grapheme.into()), charset)),
            (None, _) => None,
        };
    }

    fn try_extend_previous_grapheme(&mut self, next: char) -> bool {
        let cursor = self.grid().cursor();
        let candidate_col = if cursor.wrap_pending {
            Some(cursor.col)
        } else {
            cursor.col.checked_sub(1)
        };
        let Some(mut col) = candidate_col else {
            return false;
        };
        if self
            .grid()
            .cell(cursor.row, col)
            .is_some_and(Cell::is_wide_char_spacer)
        {
            let Some(leader) = col.checked_sub(1) else {
                return false;
            };
            col = leader;
        }
        if let Some(region) = self.grid().multicell_region_at(cursor.row, col) {
            let candidate = self.grid().cell(cursor.row, col).unwrap();
            if candidate.multicell.relative_row() != 0 {
                return false;
            }
            let leader = self.grid().cell(region.top, region.left).unwrap();
            let original = leader.content.to_text();
            if !crate::unicode::extends_grapheme(&original, next) {
                return false;
            }
            let mut combined = original;
            combined.push(next);
            let descriptor = leader.multicell;
            self.invalidate_selection_in_region(
                region.top..region.bottom(),
                region.left..region.right(),
            );
            if let Some(leader) = self.grid_mut().cell_mut(region.top, region.left) {
                leader.set_grapheme(&combined);
                leader.multicell = descriptor;
            }
            self.remember_last_graphic(&combined, CharacterSet::Ascii);
            return true;
        }
        let Some(cell) = self.grid().cell(cursor.row, col) else {
            return false;
        };
        if cell.content.is_empty() || cell.is_wide_char_spacer() {
            return false;
        }
        let charset = stored_character_set(cell);
        let original = cell.content.to_text();
        if !crate::unicode::extends_grapheme(&original, next) {
            return false;
        }

        let mut combined = original;
        combined.push(next);
        let old_width = cell.width();
        let requested_width = crate::unicode::grapheme_width(&combined).max(1);
        let template = cell.clone();

        // Remove the old cluster before resolving its new placement. Growth
        // can collide with a top-row multicell, skip lower rows, or wrap/scroll;
        // all of those paths must use the same atomic rules as normal text.
        self.invalidate_selection_for_write(cursor.row, col, col + old_width);
        if let Some(row) = self.grid_mut().row_mut(cursor.row) {
            row.clear_grapheme_at(col);
        }
        self.grid_mut().set_cursor(cursor.row, col);
        let new_width = self.prepare_normal_text_position(requested_width);
        let destination = self.grid().cursor();
        let (_, right) = self.write_horizontal_bounds(destination.row, destination.col);
        let col = destination.col;
        self.invalidate_selection_for_write(destination.row, col, col + new_width);
        if let Some(row) = self.grid_mut().row_mut(destination.row) {
            // Atomic clearing handles old ordinary wide pairs without a
            // full-row repair scan, including a neighbor outside the write.
            row.clear_grapheme_at(col);
            if new_width == 2 {
                row.clear_grapheme_at(col + 1);
            }
            if let Some(leader) = row.get_mut(col) {
                *leader = template.clone();
                leader.set_grapheme(&combined);
                leader.flags.remove(CellFlags::WIDE_CHAR_SPACER);
                leader.flags.set(CellFlags::WIDE_CHAR, new_width == 2);
            }
            if new_width == 2
                && let Some(spacer) = row.get_mut(col + 1)
            {
                *spacer = template;
                spacer.content = CellContent::Empty;
                spacer.flags.remove(CellFlags::WIDE_CHAR);
                spacer.flags.insert(CellFlags::WIDE_CHAR_SPACER);
            }
        }
        if col + new_width >= right {
            self.grid_mut().set_cursor(destination.row, right - 1);
            self.grid_mut().set_wrap_pending(true);
        } else {
            self.grid_mut().set_cursor(destination.row, col + new_width);
        }
        self.remember_last_graphic(&combined, charset);
        true
    }

    /// Repeat the most recently printed grapheme using the current rendition.
    /// A missing or deliberately unremembered pathological grapheme is a no-op.
    pub(crate) fn repeat_last_graphic(&mut self, count: usize) {
        let Some((grapheme, charset)) = self.last_graphic.clone() else {
            return;
        };

        match grapheme {
            CellContent::Empty => {}
            CellContent::Char(character) => {
                for _ in 0..count {
                    self.write_char_with_charset(character, charset);
                }
            }
            CellContent::Grapheme(grapheme) => {
                if grapheme.len() > MAX_REPEAT_GRAPHEME_BYTES {
                    return;
                }
                for _ in 0..count {
                    for character in grapheme.chars() {
                        self.write_char_with_charset(character, charset);
                    }
                }
            }
        }
    }

    /// Perform a carriage return, honoring the left margin when applicable.
    pub fn carriage_return(&mut self) {
        let cursor = self.grid().cursor();
        let col = self.carriage_return_column(cursor.row, cursor.col);
        // Clear wrapped flag — explicit CR means this row ends here
        if let Some(row) = self.grid_mut().row_mut(cursor.row) {
            row.set_wrapped(false);
        }
        self.grid_mut().set_cursor(cursor.row, col);
    }

    /// Perform a line feed (move cursor down, scroll if at bottom)
    pub(crate) fn linefeed(&mut self) {
        let cursor = self.grid().cursor();
        let col = if self.mode.contains(TerminalMode::LINEFEED_NEWLINE) {
            self.carriage_return_column(cursor.row, cursor.col)
        } else {
            cursor.col
        };
        self.index_to_column(col);
    }

    /// Move the cursor one line down to `col`, scrolling the region when the
    /// cursor is on its bottom margin.
    pub(crate) fn index_to_column(&mut self, col: usize) {
        let cursor = self.grid().cursor();
        if cursor.row + 1 == self.scroll_bottom && self.col_inside_horizontal_margins(col) {
            // At bottom of scroll region, scroll up
            self.scroll_up(1);
            self.grid_mut().set_cursor(cursor.row, col);
            // Clear wrap_pending — we've moved to a new logical position
            self.grid_mut().set_wrap_pending(false);
        } else {
            let row = cursor
                .row
                .saturating_add(1)
                .min(self.rows.saturating_sub(1));
            self.grid_mut().set_cursor(row, col);
        }
    }

    /// Scroll the screen up by n lines
    pub fn scroll_up(&mut self, n: usize) {
        // A single scroll request cannot move more than the region's height.
        // Keep history, selection, and image movement on the same capped count.
        let n = n.min(self.scroll_bottom.saturating_sub(self.scroll_top));
        if n == 0 {
            return;
        }
        self.invalidate_prompt_navigation_after_manual_scroll();
        let scroll_top = self.scroll_top;
        let scroll_bottom = self.scroll_bottom;
        let scroll_left = self.scroll_left;
        let scroll_right = self.scroll_right;
        let full_width = scroll_left == 0 && scroll_right == self.cols;
        let blank = self.erase_cell();
        for _ in 0..n {
            if full_width {
                self.erase_multicells_crossing_row_with_selection(
                    scroll_top.saturating_add(1),
                    scroll_left,
                    scroll_right,
                    &blank,
                );
                self.erase_multicells_crossing_row_with_selection(
                    scroll_bottom.saturating_sub(1),
                    scroll_left,
                    scroll_right,
                    &blank,
                );
            } else {
                self.erase_multicells_in_region_with_selection(
                    scroll_top..scroll_bottom,
                    scroll_left..scroll_right,
                    &blank,
                );
            }
            if full_width && scroll_top == 0 && scroll_bottom == self.rows {
                let evicted = self.grid_mut().scroll_up();
                self.shift_selection_after_full_scroll(1);
                self.note_history_eviction(evicted);
                self.shift_prompts_after_archival_scroll(scroll_bottom);
            } else if full_width {
                let evicted = self.grid_mut().scroll_region_up(scroll_top, scroll_bottom);
                self.shift_selection_after_region_scroll_up(scroll_top..scroll_bottom, evicted);
                if scroll_top == 0 {
                    self.shift_prompts_after_archival_scroll(scroll_bottom);
                }
            } else {
                self.shift_selection_in_region(
                    scroll_top..scroll_bottom,
                    scroll_left..scroll_right,
                    -1,
                );
                self.scroll_rect_with_selection_damage(
                    scroll_top..scroll_bottom,
                    scroll_left..scroll_right,
                    &blank,
                    false,
                );
            }
            if full_width {
                self.apply_scroll_row_background(scroll_bottom - 1, &blank);
            }
        }
        self.repair_multicells_with_selection(&blank);
        self.scroll_graphics_up(scroll_top..scroll_bottom, scroll_left..scroll_right, n);
    }

    /// Match graphics archival to the actual full-width grid movement. DL
    /// from row zero archives too, including when it covers the whole screen.
    fn scroll_graphics_up(
        &mut self,
        rows: std::ops::Range<usize>,
        columns: std::ops::Range<usize>,
        count: usize,
    ) {
        let preserve_history = !self.alt_screen_active
            && rows.start == 0
            && columns.start == 0
            && columns.end == self.cols;
        let history_lines = self.grid().scrollback_len();
        if preserve_history && rows.end < self.rows {
            self.graphics
                .scroll_archival_region_up(rows.end, count, history_lines);
        } else {
            self.graphics.scroll_up(
                self.alt_screen_active,
                rows,
                columns,
                count,
                preserve_history,
                history_lines,
            );
        }
    }

    /// Scroll the screen down by n lines
    pub fn scroll_down(&mut self, n: usize) {
        let n = n.min(self.scroll_bottom.saturating_sub(self.scroll_top));
        if n == 0 {
            return;
        }
        let scroll_top = self.scroll_top;
        let scroll_bottom = self.scroll_bottom;
        let scroll_left = self.scroll_left;
        let scroll_right = self.scroll_right;
        let full_width = scroll_left == 0 && scroll_right == self.cols;
        let blank = self.erase_cell();
        for _ in 0..n {
            if full_width {
                self.erase_multicells_crossing_row_with_selection(
                    scroll_top.saturating_add(1),
                    scroll_left,
                    scroll_right,
                    &blank,
                );
                self.erase_multicells_crossing_row_with_selection(
                    scroll_bottom.saturating_sub(1),
                    scroll_left,
                    scroll_right,
                    &blank,
                );
            } else {
                self.erase_multicells_in_region_with_selection(
                    scroll_top..scroll_bottom,
                    scroll_left..scroll_right,
                    &blank,
                );
            }
            self.shift_selection_in_region(scroll_top..scroll_bottom, scroll_left..scroll_right, 1);
            if full_width && scroll_top == 0 && scroll_bottom == self.rows {
                self.grid_mut().scroll_down();
            } else if full_width {
                self.grid_mut()
                    .scroll_region_down(scroll_top, scroll_bottom);
            } else {
                self.scroll_rect_with_selection_damage(
                    scroll_top..scroll_bottom,
                    scroll_left..scroll_right,
                    &blank,
                    true,
                );
            }
            if full_width {
                self.apply_scroll_row_background(scroll_top, &blank);
            }
        }
        self.repair_multicells_with_selection(&blank);
        self.graphics.scroll_down(
            self.alt_screen_active,
            scroll_top..scroll_bottom,
            scroll_left..scroll_right,
            n,
        );
    }

    /// Move to the next tab stop
    pub(crate) fn horizontal_tab(&mut self) {
        self.cursor_forward_tabs(1);
    }

    /// Move forward across the requested number of horizontal tab stops.
    pub(crate) fn cursor_forward_tabs(&mut self, count: usize) {
        let cursor = self.grid().cursor();
        let max_col =
            if self.row_inside_vertical_margins(cursor.row) && cursor.col < self.scroll_right {
                self.scroll_right - 1
            } else {
                self.cols.saturating_sub(1)
            };
        let mut col = cursor.col;
        for _ in 0..count {
            col = ((col + 1)..=max_col)
                .find(|candidate| self.tabs.get(*candidate).copied().unwrap_or(false))
                .unwrap_or(max_col);
        }
        self.grid_mut().set_cursor(cursor.row, col);
    }

    /// Move backward across the requested number of horizontal tab stops.
    pub(crate) fn cursor_backward_tabs(&mut self, count: usize) {
        let cursor = self.grid().cursor();
        let min_col =
            if self.row_inside_vertical_margins(cursor.row) && cursor.col >= self.scroll_left {
                self.scroll_left
            } else {
                0
            };
        let mut col = cursor.col;
        for _ in 0..count {
            col = (min_col..col)
                .rev()
                .find(|candidate| self.tabs.get(*candidate).copied().unwrap_or(false))
                .unwrap_or(min_col);
        }
        self.grid_mut().set_cursor(cursor.row, col);
    }

    /// Move cursor up
    pub(crate) fn cursor_up(&mut self, n: usize) {
        let cursor = self.grid().cursor();
        let minimum =
            if self.col_inside_horizontal_margins(cursor.col) && cursor.row >= self.scroll_top {
                self.scroll_top
            } else {
                0
            };
        let row = cursor.row.saturating_sub(n).max(minimum);
        self.grid_mut().set_cursor(row, cursor.col);
    }

    /// Move cursor down
    pub(crate) fn cursor_down(&mut self, n: usize) {
        let cursor = self.grid().cursor();
        let maximum =
            if self.col_inside_horizontal_margins(cursor.col) && cursor.row < self.scroll_bottom {
                self.scroll_bottom - 1
            } else {
                self.rows - 1
            };
        let new_row = cursor.row.saturating_add(n).min(maximum);
        self.grid_mut().set_cursor(new_row, cursor.col);
    }

    /// Move cursor forward (right)
    pub(crate) fn cursor_forward(&mut self, n: usize) {
        let cursor = self.grid().cursor();
        let maximum =
            if self.row_inside_vertical_margins(cursor.row) && cursor.col < self.scroll_right {
                self.scroll_right - 1
            } else {
                self.cols - 1
            };
        let new_col = cursor.col.saturating_add(n).min(maximum);
        self.grid_mut().set_cursor(cursor.row, new_col);
    }

    /// Move cursor backward (left)
    pub(crate) fn cursor_backward(&mut self, n: usize) {
        let cursor = self.grid().cursor();
        let minimum =
            if self.row_inside_vertical_margins(cursor.row) && cursor.col >= self.scroll_left {
                self.scroll_left
            } else {
                0
            };
        self.grid_mut()
            .set_cursor(cursor.row, cursor.col.saturating_sub(n).max(minimum));
    }

    /// HPR moves horizontally without the CUF margin clamp.
    pub(crate) fn horizontal_position_relative(&mut self, n: usize) {
        if self.mode.contains(TerminalMode::ORIGIN) {
            self.cursor_forward(n);
            return;
        }
        let cursor = self.grid().cursor();
        let col = cursor.col.saturating_add(n).min(self.cols - 1);
        self.grid_mut().set_cursor(cursor.row, col);
    }

    /// VPR moves vertically without the CUD margin clamp.
    pub(crate) fn vertical_position_relative(&mut self, n: usize) {
        if self.mode.contains(TerminalMode::ORIGIN) {
            self.cursor_down(n);
            return;
        }
        let cursor = self.grid().cursor();
        let row = cursor.row.saturating_add(n).min(self.rows - 1);
        self.grid_mut().set_cursor(row, cursor.col);
    }

    /// CHA/HPA move only the column while respecting DECOM's origin.
    pub(crate) fn set_cursor_column(&mut self, col: usize) {
        let cursor = self.grid().cursor();
        let col = if self.mode.contains(TerminalMode::ORIGIN) {
            self.scroll_left
                .saturating_add(col.saturating_sub(1))
                .min(self.scroll_right - 1)
        } else {
            col.saturating_sub(1).min(self.cols - 1)
        };
        self.grid_mut().set_cursor(cursor.row, col);
    }

    /// VPA moves only the row while respecting DECOM's origin.
    pub(crate) fn set_cursor_row(&mut self, row: usize) {
        let cursor = self.grid().cursor();
        let row = if self.mode.contains(TerminalMode::ORIGIN) {
            self.scroll_top
                .saturating_add(row.saturating_sub(1))
                .min(self.scroll_bottom - 1)
        } else {
            row.saturating_sub(1).min(self.rows - 1)
        };
        self.grid_mut().set_cursor(row, cursor.col);
    }

    /// Set cursor position (1-indexed input, converted to 0-indexed)
    pub fn set_cursor_position(&mut self, row: usize, col: usize) {
        let (row, col) = if self.mode.contains(TerminalMode::ORIGIN) {
            (
                self.scroll_top
                    .saturating_add(row.saturating_sub(1))
                    .min(self.scroll_bottom - 1),
                self.scroll_left
                    .saturating_add(col.saturating_sub(1))
                    .min(self.scroll_right - 1),
            )
        } else {
            (
                row.saturating_sub(1).min(self.rows - 1),
                col.saturating_sub(1).min(self.cols - 1),
            )
        };
        self.grid_mut().set_cursor(row, col);
    }

    /// Erase in display
    pub(crate) fn erase_in_display(&mut self, mode: u16) {
        let cursor = self.grid().cursor();
        let cols = self.cols;
        let rows = self.rows;

        match mode {
            0 => {
                self.erase_cells_in_region(cursor.row..cursor.row + 1, cursor.col..cols, false);
                self.erase_cells_in_region(cursor.row + 1..rows, 0..cols, false);
            }
            1 => {
                self.erase_cells_in_region(0..cursor.row, 0..cols, false);
                self.erase_cells_in_region(cursor.row..cursor.row + 1, 0..cursor.col + 1, false);
            }
            2 | 22 => {
                if mode == 22 && !self.alt_screen_active {
                    let evicted = self.grid_mut().archive_visible();
                    self.shift_selection_after_full_scroll(rows);
                    self.note_history_eviction(evicted);
                    let oldest = -(self.grid().scrollback_len() as i32);
                    let scroll_amount = i32::try_from(rows).unwrap_or(i32::MAX);
                    self.prompt_tracker.adjust_for_scroll(scroll_amount, oldest);
                    if let Some(navigation) = &mut self.prompt_navigation {
                        navigation.row = navigation.row.saturating_sub(scroll_amount);
                    }
                    self.retain_prompt_navigation(oldest);
                }
                // Erase the viewport without moving the cursor or clearing
                // scrollback.
                self.erase_cells_in_region(0..rows, 0..cols, false);
                self.graphics.clear_screen(self.alt_screen_active);
                if !self.alt_screen_active && rows > 0 {
                    self.prompt_tracker.retire_content_in_rows(
                        0,
                        i32::try_from(rows.saturating_sub(1)).unwrap_or(i32::MAX),
                    );
                    let oldest = -(self.grid.scrollback_len() as i32);
                    self.retain_prompt_navigation(oldest);
                }
            }
            3 => {
                // Xterm extension: erase saved lines only.
                self.clear_scrollback();
            }
            _ => {}
        }
        if matches!(mode, 2 | 3 | 22) {
            self.clear_extra_cursors();
        }
    }

    /// Erase in line
    pub(crate) fn erase_in_line(&mut self, mode: u16) {
        let cursor = self.grid().cursor();

        let (start, end) = match mode {
            0 => (cursor.col, self.cols),
            1 => (0, cursor.col + 1),
            2 => (0, self.cols),
            _ => return,
        };

        self.erase_cells_in_region(cursor.row..cursor.row + 1, start..end, false);
    }

    /// Selectively erase erasable cells in the display (DECSED). Protected
    /// cells survive with their complete grapheme and presentation state.
    pub(crate) fn selective_erase_in_display(&mut self, mode: u16) {
        let cursor = self.grid().cursor();
        let cols = self.cols;
        let rows = self.rows;

        match mode {
            0 => {
                self.erase_cells_in_region(cursor.row..cursor.row + 1, cursor.col..cols, true);
                self.erase_cells_in_region(cursor.row + 1..rows, 0..cols, true);
            }
            1 => {
                self.erase_cells_in_region(0..cursor.row, 0..cols, true);
                self.erase_cells_in_region(cursor.row..cursor.row + 1, 0..cursor.col + 1, true);
            }
            2 => {
                self.erase_cells_in_region(0..rows, 0..cols, true);
            }
            _ => {}
        }
    }

    /// Selectively erase erasable cells in the current line (DECSEL).
    pub(crate) fn selective_erase_in_line(&mut self, mode: u16) {
        let cursor = self.grid().cursor();
        let (start, end) = match mode {
            0 => (cursor.col, self.cols),
            1 => (0, cursor.col + 1),
            2 => (0, self.cols),
            _ => return,
        };
        self.erase_cells_in_region(cursor.row..cursor.row + 1, start..end, true);
    }

    /// Permit or deny Kitty's ordinary- and temporary-file transmission media.
    /// Like other host security policy, this survives DECSTR and RIS resets.
    pub fn set_kitty_file_transmission(&mut self, allowed: bool) {
        self.graphics.set_file_transmission_allowed(allowed);
    }

    pub fn kitty_file_transmission(&self) -> bool {
        self.graphics.file_transmission_allowed()
    }

    /// Apply DECSCA. Invalid parameters leave the current protection state
    /// unchanged, while 0 and 2 select erasable text and 1 selects protected.
    pub(crate) fn set_character_protection(&mut self, parameter: u16) {
        match parameter {
            0 | 2 => self.current_character_protected = false,
            1 => self.current_character_protected = true,
            _ => {}
        }
    }

    pub fn character_protection_parameter(&self) -> u8 {
        u8::from(self.current_character_protected)
    }

    /// Blank cells created by erase/insert/delete operations carry only the
    /// active background color, matching BCE terminals and avoiding text-style
    /// leakage into empty cells.
    fn erase_cell(&self) -> Cell {
        Cell {
            bg: self.current_bg,
            bg_spec: self.current_bg_spec,
            ..Cell::default()
        }
    }

    /// Full-width grid scrolling recycles an already-cleared default row.
    /// Apply only BCE's background; do not inherit live text attributes or
    /// touch surviving cells/selection. Default backgrounds need no extra pass.
    fn apply_scroll_row_background(&mut self, row: usize, blank: &Cell) {
        if blank.bg_spec == ColorSpec::Default && blank.bg == Color::default_bg() {
            return;
        }
        if let Some(row) = self.grid_mut().row_mut(row) {
            for cell in row.iter_mut() {
                cell.bg = blank.bg;
                cell.bg_spec = blank.bg_spec;
            }
        }
    }

    /// Set the current hyperlink (OSC 8)
    ///
    /// All characters written after this will be part of this hyperlink
    /// until `reset_hyperlink` is called.
    pub(crate) fn set_hyperlink(&mut self, url: String, id: Option<String>) {
        const MAX_HYPERLINK_BYTES: usize = 8 * 1024;
        if url.len() > MAX_HYPERLINK_BYTES
            || id
                .as_ref()
                .is_some_and(|identifier| identifier.len() > MAX_HYPERLINK_BYTES)
        {
            self.current_hyperlink_id = None;
            return;
        }
        if let Some(existing) = self.hyperlink_registry.find_id(&url, id.as_deref()) {
            self.current_hyperlink_id = Some(existing);
            return;
        }
        let registry_budget = (self.grid.memory_usage().scrollback_byte_limit / 4).max(1024 * 1024);
        if self.hyperlink_registry.memory_usage_bytes() >= registry_budget {
            self.prune_unreferenced_hyperlinks();
            if self.hyperlink_registry.memory_usage_bytes() >= registry_budget {
                self.current_hyperlink_id = None;
                return;
            }
        }
        let hyperlink_id = self.hyperlink_registry.get_or_insert(url, id);
        self.current_hyperlink_id = Some(hyperlink_id);
    }

    /// Clear the current hyperlink
    ///
    /// Characters written after this will not be part of any hyperlink.
    pub(crate) fn reset_hyperlink(&mut self) {
        self.current_hyperlink_id = None;
    }

    /// Get the URL for a hyperlink ID
    pub fn get_hyperlink_url(&self, id: u32) -> Option<&str> {
        self.hyperlink_registry.get_url(id)
    }

    /// Resolve an OSC 8 target at a visual cell in the presented viewport,
    /// including scrolled history and a frozen synchronized-output snapshot.
    #[cfg(test)]
    pub(crate) fn presentation_hyperlink_at(&self, row: usize, col: usize) -> Option<&str> {
        let grid = self.presentation_grid();
        if row >= grid.rows() || col >= grid.cols() {
            return None;
        }
        let id = grid.cell_scrolled(row, col)?.hyperlink_id?;
        self.get_hyperlink_url(id)
    }

    /// Get access to the hyperlink registry
    pub fn hyperlink_registry(&self) -> &HyperlinkRegistry {
        &self.hyperlink_registry
    }

    // === Graphics methods ===

    /// Check if synchronized output mode is enabled
    pub fn is_synchronized_output(&self) -> bool {
        self.mode.contains(TerminalMode::SYNCHRONIZED_OUTPUT)
    }

    /// Monotonic recovery deadline for an active synchronized-output batch.
    pub fn synchronized_output_deadline(&self) -> Option<Instant> {
        self.synchronized_output_snapshot
            .as_ref()
            .map(|snapshot| snapshot.started_at + SYNCHRONIZED_OUTPUT_TIMEOUT)
    }

    /// End a synchronized-output batch that outlived the safety timeout.
    /// Returns whether presentation was released and therefore needs a frame.
    pub fn expire_synchronized_output(&mut self, now: Instant) -> bool {
        let expired = self
            .synchronized_output_deadline()
            .is_some_and(|deadline| now >= deadline);
        if expired {
            self.set_mode(TerminalMode::SYNCHRONIZED_OUTPUT, false);
        }
        expired
    }

    /// Set a tab stop at the current column
    pub(crate) fn set_tab_stop(&mut self) {
        let cursor = self.grid().cursor();
        if let Some(tab) = self.tabs.get_mut(cursor.col) {
            *tab = true;
        }
    }

    /// Clear the tab stop at the current column.
    pub(crate) fn clear_tab_stop(&mut self) {
        let cursor = self.grid().cursor();
        if let Some(tab) = self.tabs.get_mut(cursor.col) {
            *tab = false;
        }
    }

    /// Clear all tab stops
    pub(crate) fn clear_tab_stops(&mut self) {
        self.tabs.fill(false);
    }

    /// Restore DEC's default horizontal tab stops at columns 9, 17, ...
    pub(crate) fn reset_tab_stops(&mut self) {
        self.clear_tab_stops();
        for i in (TAB_INTERVAL..self.cols).step_by(TAB_INTERVAL) {
            self.tabs[i] = true;
        }
    }

    /// Set the scroll region (1-indexed, inclusive) for DECSTBM
    pub(crate) fn set_scroll_region(&mut self, top: usize, bottom: usize) {
        if top == 0 || top >= bottom || bottom > self.rows {
            return;
        }
        self.scroll_top = top - 1;
        self.scroll_bottom = bottom;
        self.set_cursor_position(1, 1);
    }

    /// Return the active scrolling region as zero-based, end-exclusive rows.
    ///
    /// Exposing this state keeps conformance and fuzz harnesses from having to
    /// infer parser invariants through cursor side effects.
    pub fn scroll_region(&self) -> std::ops::Range<usize> {
        self.scroll_top..self.scroll_bottom
    }

    /// Set DECSLRM's 1-indexed inclusive horizontal margins. Invalid pairs
    /// leave both margins and the cursor unchanged.
    pub(crate) fn set_left_right_margins(&mut self, left: usize, right: usize) {
        if !self.mode.contains(TerminalMode::LEFT_RIGHT_MARGIN)
            || left == 0
            || left >= right
            || right > self.cols
        {
            return;
        }
        self.scroll_left = left - 1;
        self.scroll_right = right;
        self.set_cursor_position(1, 1);
    }

    /// Return DECSLRM's zero-based, end-exclusive horizontal region.
    pub fn horizontal_scroll_region(&self) -> std::ops::Range<usize> {
        self.scroll_left..self.scroll_right
    }

    /// Set or reset DECLRMM. Either transition restores page-width margins,
    /// but does not move the cursor.
    pub(crate) fn set_left_right_margin_mode(&mut self, enable: bool) {
        self.set_mode(TerminalMode::LEFT_RIGHT_MARGIN, enable);
        self.scroll_left = 0;
        self.scroll_right = self.cols;
    }

    /// Reverse index (move up, scroll down if at top of region)
    pub(crate) fn reverse_index(&mut self) {
        let cursor = self.grid().cursor();
        if cursor.row == self.scroll_top && self.col_inside_horizontal_margins(cursor.col) {
            self.scroll_down(1);
        } else if cursor.row > 0 {
            self.grid_mut().set_cursor(cursor.row - 1, cursor.col);
        }
    }

    /// Insert blank characters at cursor position
    pub(crate) fn insert_chars(&mut self, count: usize) {
        self.shift_chars(count, true);
    }

    /// Delete characters at cursor position
    pub(crate) fn delete_chars(&mut self, count: usize) {
        self.shift_chars(count, false);
    }

    fn shift_chars(&mut self, count: usize, inserting: bool) {
        let cursor = self.grid().cursor();
        let (left, right) = if self.row_inside_vertical_margins(cursor.row) {
            (self.scroll_left, self.scroll_right)
        } else {
            (0, self.cols)
        };
        let count = count.min(right.saturating_sub(cursor.col));
        if count == 0 || cursor.col < left || cursor.col >= right {
            return;
        }
        let blank = self.erase_cell();
        // Whole-block erasure happens in the old coordinates, before moving
        // surviving anchors. Wide-pair repair below reports new coordinates.
        self.erase_multicells_for_horizontal_shift(
            cursor.row, cursor.col, right, count, inserting, &blank,
        );
        let magnitude = isize::try_from(count).unwrap_or(isize::MAX);
        let delta = if inserting { magnitude } else { -magnitude };
        for slot in [&mut self.selection, &mut self.navigation_cursor] {
            if let Some(selection) = slot
                && !selection.shift_columns_in_row(cursor.row, cursor.col..right, delta, self.cols)
            {
                *slot = None;
            }
        }
        let (grid, mut damage) = self.grid_with_selection_damage();
        if let Some(row) = grid.row_mut(cursor.row) {
            let report = |cols| damage(cursor.row..cursor.row + 1, cols);
            if inserting {
                row.insert_cells_bounded_with_damage(cursor.col, right, count, &blank, report);
            } else {
                row.delete_cells_bounded_with_damage(cursor.col, right, count, &blank, report);
            }
            grid.set_wrap_pending(false);
        }
    }

    /// Erase characters at the cursor without moving it.
    pub(crate) fn erase_chars(&mut self, count: usize) {
        let cursor = self.grid().cursor();
        self.erase_cells_in_region(
            cursor.row..cursor.row + 1,
            cursor.col..cursor.col.saturating_add(count),
            false,
        );
    }

    fn erase_multicells_for_horizontal_shift(
        &mut self,
        row: usize,
        start: usize,
        right: usize,
        count: usize,
        inserting: bool,
        blank: &Cell,
    ) {
        let count = count.min(right.saturating_sub(start));
        let mut erase = HashSet::new();
        for col in start..right {
            let Some(region) = self.grid().multicell_region_at(row, col) else {
                continue;
            };
            let split = if region.descriptor.rows() > 1 {
                true
            } else if inserting {
                region.left < start || region.right() > right.saturating_sub(count)
            } else {
                (region.left < start.saturating_add(count) && region.right() > start)
                    || region.right() > right
            };
            if split {
                erase.insert((region.top, region.left));
            }
        }
        for (top, left) in erase {
            let (grid, damage) = self.grid_with_selection_damage();
            grid.clear_multicell_at_with_damage(top, left, blank, false, damage);
        }
    }

    /// Insert blank lines at cursor row within the scroll region
    pub fn insert_lines(&mut self, count: usize) {
        if count == 0 {
            return;
        }
        let cursor = self.grid().cursor();
        let scroll_bottom = self.scroll_bottom;
        let scroll_left = self.scroll_left;
        let scroll_right = self.scroll_right;
        if self.row_inside_vertical_margins(cursor.row)
            && self.col_inside_horizontal_margins(cursor.col)
        {
            let count = count.min(scroll_bottom - cursor.row);
            let blank = self.erase_cell();
            let full_width = scroll_left == 0 && scroll_right == self.cols;
            if !full_width {
                self.erase_multicells_in_region_with_selection(
                    cursor.row..scroll_bottom,
                    scroll_left..scroll_right,
                    &blank,
                );
            }
            for _ in 0..count {
                if full_width {
                    self.erase_multicells_crossing_row_with_selection(
                        cursor.row.saturating_add(1),
                        scroll_left,
                        scroll_right,
                        &blank,
                    );
                    self.erase_multicells_crossing_row_with_selection(
                        scroll_bottom.saturating_sub(1),
                        scroll_left,
                        scroll_right,
                        &blank,
                    );
                    self.grid_mut()
                        .scroll_region_down(cursor.row, scroll_bottom);
                    self.apply_scroll_row_background(cursor.row, &blank);
                    self.shift_selection_in_region(
                        cursor.row..scroll_bottom,
                        scroll_left..scroll_right,
                        1,
                    );
                } else {
                    self.shift_selection_in_region(
                        cursor.row..scroll_bottom,
                        scroll_left..scroll_right,
                        1,
                    );
                    self.scroll_rect_with_selection_damage(
                        cursor.row..scroll_bottom,
                        scroll_left..scroll_right,
                        &blank,
                        true,
                    );
                }
            }
            self.repair_multicells_with_selection(&blank);
            self.graphics.scroll_down(
                self.alt_screen_active,
                cursor.row..scroll_bottom,
                scroll_left..scroll_right,
                count,
            );
            self.grid_mut().set_cursor(cursor.row, scroll_left);
        }
    }

    /// Delete lines at cursor row within the scroll region
    pub fn delete_lines(&mut self, count: usize) {
        if count == 0 {
            return;
        }
        let cursor = self.grid().cursor();
        let scroll_bottom = self.scroll_bottom;
        let scroll_left = self.scroll_left;
        let scroll_right = self.scroll_right;
        if self.row_inside_vertical_margins(cursor.row)
            && self.col_inside_horizontal_margins(cursor.col)
        {
            self.invalidate_prompt_navigation_after_manual_scroll();
            let count = count.min(scroll_bottom - cursor.row);
            let blank = self.erase_cell();
            let full_width = scroll_left == 0 && scroll_right == self.cols;
            if !full_width {
                self.erase_multicells_in_region_with_selection(
                    cursor.row..scroll_bottom,
                    scroll_left..scroll_right,
                    &blank,
                );
            }
            for _ in 0..count {
                if full_width {
                    self.erase_multicells_crossing_row_with_selection(
                        cursor.row.saturating_add(1),
                        scroll_left,
                        scroll_right,
                        &blank,
                    );
                    self.erase_multicells_crossing_row_with_selection(
                        scroll_bottom.saturating_sub(1),
                        scroll_left,
                        scroll_right,
                        &blank,
                    );
                    let evicted = self.grid_mut().scroll_region_up(cursor.row, scroll_bottom);
                    self.apply_scroll_row_background(scroll_bottom - 1, &blank);
                    self.shift_selection_after_region_scroll_up(cursor.row..scroll_bottom, evicted);
                    if cursor.row == 0 {
                        self.shift_prompts_after_archival_scroll(scroll_bottom);
                    }
                } else {
                    self.shift_selection_in_region(
                        cursor.row..scroll_bottom,
                        scroll_left..scroll_right,
                        -1,
                    );
                    self.scroll_rect_with_selection_damage(
                        cursor.row..scroll_bottom,
                        scroll_left..scroll_right,
                        &blank,
                        false,
                    );
                }
            }
            self.repair_multicells_with_selection(&blank);
            self.scroll_graphics_up(cursor.row..scroll_bottom, scroll_left..scroll_right, count);
            self.grid_mut().set_cursor(cursor.row, scroll_left);
        }
    }

    /// Switch to alternate screen buffer
    pub fn enter_alt_screen(&mut self) {
        let previous_pointer_shape = self.effective_pointer_shape();
        self.mode.insert(TerminalMode::ALT_SCREEN);
        if !self.alt_screen_active {
            self.navigation_cursor = None;
            self.clear_extra_cursors();
            self.graphics.clear_screen_all(true);
            self.alt_screen_active = true;
            self.alt_grid.clear();
            self.alt_grid.clear_scrollback();
        }
        self.emit_pointer_shape_change(previous_pointer_shape);
    }

    /// Switch back to primary screen buffer
    pub(crate) fn exit_alt_screen(&mut self) {
        let previous_pointer_shape = self.effective_pointer_shape();
        self.mode.remove(TerminalMode::ALT_SCREEN);
        if self.alt_screen_active {
            self.clear_extra_cursors();
            self.navigation_cursor = None;
            self.graphics.clear_screen_all(true);
            self.alt_screen_active = false;
        }
        self.emit_pointer_shape_change(previous_pointer_shape);
    }

    /// Save cursor position
    pub(crate) fn save_cursor(&mut self) {
        let cursor = self.grid().cursor();
        let saved = SavedCursor {
            row: cursor.row,
            col: cursor.col,
            attrs: self.current_attrs,
            fg: self.current_fg,
            bg: self.current_bg,
            fg_spec: self.current_fg_spec,
            bg_spec: self.current_bg_spec,
            underline_color: self.current_underline_color,
            underline_color_spec: self.current_underline_color_spec,
            g_sets: self.g_sets,
            gl_register: self.gl_register,
            gr_register: self.gr_register,
            single_shift: self.single_shift,
            character_protected: self.current_character_protected,
            wrap_pending: cursor.wrap_pending,
            origin_mode: self.mode.contains(TerminalMode::ORIGIN),
        };

        if self.alt_screen_active {
            self.alt_saved_cursor = Some(saved);
        } else {
            self.saved_cursor = Some(saved);
        }
    }

    /// Restore cursor position
    pub(crate) fn restore_cursor(&mut self) {
        let saved = if self.alt_screen_active {
            self.alt_saved_cursor.clone()
        } else {
            self.saved_cursor.clone()
        }
        .unwrap_or_default();

        self.grid_mut().set_cursor(saved.row, saved.col);
        self.grid_mut().set_wrap_pending(saved.wrap_pending);
        self.current_attrs = saved.attrs;
        self.current_fg = saved.fg;
        self.current_bg = saved.bg;
        self.current_fg_spec = saved.fg_spec;
        self.current_bg_spec = saved.bg_spec;
        self.current_underline_color = saved.underline_color;
        self.current_underline_color_spec = saved.underline_color_spec;
        self.g_sets = saved.g_sets;
        self.gl_register = saved.gl_register;
        self.gr_register = saved.gr_register;
        self.single_shift = saved.single_shift;
        self.current_character_protected = saved.character_protected;
        self.set_mode(TerminalMode::ORIGIN, saved.origin_mode);
    }

    /// Set cursor visibility
    pub(crate) fn set_cursor_visible(&mut self, visible: bool) {
        self.grid.set_cursor_visible(visible);
        self.alt_grid.set_cursor_visible(visible);
        self.set_mode(TerminalMode::CURSOR_VISIBLE, visible);
    }

    /// Apply the ATT610 cursor-blink mode without replacing the cursor shape
    /// selected by DECSCUSR. The explicit override survives screen switches
    /// and is cleared by RIS.
    pub(crate) fn set_cursor_blinking(&mut self, blinking: bool) {
        self.set_mode(TerminalMode::CURSOR_BLINKING, blinking);
        self.cursor_blink_override = Some(blinking);
    }

    /// Set a terminal mode
    pub fn set_mode(&mut self, mode: TerminalMode, enable: bool) {
        if mode == TerminalMode::SYNCHRONIZED_OUTPUT {
            if enable {
                if self.synchronized_output_snapshot.is_none() {
                    let graphics = GraphicsSnapshot::capture(
                        &self.graphics,
                        self.grid(),
                        self.alt_screen_active,
                        self.cell_pixel_width,
                        self.cell_pixel_height,
                    );
                    self.synchronized_output_snapshot = Some(SynchronizedOutputSnapshot {
                        grid: self.grid().visible_snapshot(),
                        selection: self.selection_for_snapshot(),
                        reported_working_directory: self.reported_working_directory.clone(),
                        alternate_screen: self.alt_screen_active,
                        dynamic_colors: self.dynamic_colors.clone(),
                        cursor_style: self.cursor_style,
                        cursor_blink_override: self.cursor_blink_override,
                        extra_cursors: self.extra_cursors.clone(),
                        graphics,
                        started_at: Instant::now(),
                    });
                }
                self.mode.insert(mode);
            } else {
                self.mode.remove(mode);
                self.synchronized_output_snapshot = None;
            }
            return;
        }
        let previous_pointer_shape = self.effective_pointer_shape();
        if enable {
            self.mode.insert(mode);
        } else {
            self.mode.remove(mode);
        }
        self.emit_pointer_shape_change(previous_pointer_shape);
    }

    /// Soft terminal reset (DECSTR).
    ///
    /// DECSTR restores the session control state used by ordinary terminal
    /// applications without erasing either screen, scrollback, tab stops, or
    /// unrelated modern input modes. Kitty's screen-local flags are preserved,
    /// while xterm modifier resources return to their startup values. As in
    /// xterm, autowrap returns to this terminal's startup default (enabled),
    /// rather than the VT420 hardware default (disabled).
    pub(crate) fn soft_reset(&mut self) {
        let previous_pointer_shape = self.effective_pointer_shape();
        self.pointer_shapes.clear();
        self.alt_pointer_shapes.clear();
        self.mode.remove(
            TerminalMode::INSERT
                | TerminalMode::CURSOR_KEYS_APPLICATION
                | TerminalMode::ORIGIN
                | TerminalMode::CURSOR_BLINKING
                | TerminalMode::KEYPAD_APPLICATION
                | TerminalMode::LEFT_RIGHT_MARGIN,
        );
        self.mode.insert(TerminalMode::AUTOWRAP);
        self.set_cursor_visible(true);
        self.cursor_style = None;
        self.cursor_blink_override = None;
        self.clear_extra_cursors();
        self.xterm_keyboard_resource = XtermKeyboardResource::default();
        self.xterm_key_resources = XtermKeyResources::default();
        self.xterm_modify_other_keys = XtermModifyOtherKeys::default();

        self.scroll_top = 0;
        self.scroll_bottom = self.rows;
        self.scroll_left = 0;
        self.scroll_right = self.cols;
        self.grid_mut().set_wrap_pending(false);

        self.reset_attrs();
        self.current_character_protected = false;
        self.g_sets = [CharacterSet::Ascii; 4];
        self.gl_register = 0;
        self.gr_register = 2;
        self.single_shift = None;

        let saved = Some(SavedCursor::default());
        if self.alt_screen_active {
            self.alt_saved_cursor = saved;
        } else {
            self.saved_cursor = saved;
        }
        self.emit_pointer_shape_change(previous_pointer_shape);
    }

    /// Full reset (RIS)
    pub fn reset(&mut self) {
        let previous_pointer_shape = self.effective_pointer_shape();
        if self.rows > 0 {
            self.prompt_tracker.retire_content_in_rows(
                0,
                i32::try_from(self.rows.saturating_sub(1)).unwrap_or(i32::MAX),
            );
        }
        self.prompt_navigation = None;
        self.notification_protocol.clear();
        self.grid.clear();
        self.alt_grid.clear();
        self.grid.set_scroll_offset(0);
        self.alt_grid.set_scroll_offset(0);
        self.alt_screen_active = false;
        self.mode = TerminalMode::default();
        self.mode
            .insert(TerminalMode::AUTOWRAP | TerminalMode::CURSOR_VISIBLE);
        self.use_8bit_controls = false;
        self.dec_conformance_level = 4;
        self.grid.set_cursor_visible(true);
        self.alt_grid.set_cursor_visible(true);
        self.cursor_style = None;
        self.cursor_blink_override = None;
        self.clear_extra_cursors();
        self.keyboard = keyboard::KittyKeyboardState::default();
        self.alt_keyboard = keyboard::KittyKeyboardState::default();
        self.pointer_shapes.clear();
        self.alt_pointer_shapes.clear();
        self.emit_pointer_shape_change(previous_pointer_shape);
        self.xterm_keyboard_type = self.initial_xterm_keyboard_type;
        self.saved_xterm_keyboard_type = None;
        self.xterm_user_defined_keys.clear();
        self.xterm_keyboard_resource = XtermKeyboardResource::default();
        self.xterm_key_resources = XtermKeyResources::default();
        self.xterm_modify_other_keys = XtermModifyOtherKeys::default();
        self.reset_attrs();
        self.current_character_protected = false;
        self.rectangular_change_extent = false;
        self.g_sets = [CharacterSet::Ascii; 4];
        self.gl_register = 0;
        self.gr_register = 2;
        self.single_shift = None;
        self.last_graphic = None;
        self.saved_cursor = None;
        self.alt_saved_cursor = None;
        self.scroll_top = 0;
        self.scroll_bottom = self.rows;
        self.scroll_left = 0;
        self.scroll_right = self.cols;
        self.selection = None;
        self.graphics.clear_screen(false);
        self.navigation_cursor = None;
        self.graphics.clear_screen_all(true);
        self.graphics_apc_data.clear();
        self.graphics_apc_active = false;
        self.graphics_apc_overflow = false;
        self.graphics_relay_apc_data.clear();
        self.graphics_relay_apc_active = false;
        self.graphics_relay_apc_overflow = false;
        self.pending_graphics_relay_import = None;
        self.synchronized_output_snapshot = None;

        self.reset_tab_stops();
    }

    /// Extract the text on rows `[start_row..=end_row_inclusive]` from the
    /// grid (scrollback-aware). Rows are row-at-offset indices: 0..rows-1
    /// are visible, negative values reach into scrollback. Trailing
    /// whitespace on each row is stripped.
    pub fn rows_text(&self, start_row: i32, end_row_inclusive: i32) -> String {
        if end_row_inclusive < start_row {
            return String::new();
        }
        let mut out = String::new();
        for row in start_row..=end_row_inclusive {
            if let Some(grid_row) = self.grid().row_at_offset(row) {
                let line = grid_row.plain_text(self.cols);
                out.push_str(line.trim_end());
            }
            if row != end_row_inclusive {
                out.push('\n');
            }
        }
        out
    }

    /// A command line as the shell showed it: the text from `start` up to,
    /// not including, `end`, both (row, column). Soft-wrapped rows join with
    /// no break, other row ends become newlines, and surrounding blanks are
    /// dropped. `truncated` is true when `start` is no longer retained or
    /// the text was cut at `max_bytes`.
    pub(crate) fn command_text_between(
        &self,
        (start_row, start_col): (i32, usize),
        (end_row, end_col): (i32, usize),
        max_bytes: usize,
    ) -> (String, bool) {
        let oldest = -i32::try_from(self.grid().scrollback_len()).unwrap_or(i32::MAX);
        if start_row < oldest {
            return (String::new(), true);
        }
        let mut text = String::new();
        let mut truncated = false;
        for row_index in start_row..=end_row {
            let Some(row) = self.grid().row_at_offset(row_index) else {
                break;
            };
            let from = if row_index == start_row { start_col } else { 0 };
            let to = if row_index == end_row {
                end_col
            } else {
                self.cols
            };
            let piece = row.plain_text_range(from, to);
            if row_index != end_row && !row.is_wrapped() {
                text.push_str(piece.trim_end());
                text.push('\n');
            } else {
                text.push_str(&piece);
            }
            // Stop reading once the budget is spent; only the rows read so
            // far are ever held.
            if text.len() > max_bytes {
                truncated = true;
                break;
            }
        }
        let text = text.trim();
        let kept = crate::safe_text::floor_char_boundary(text, max_bytes);
        truncated |= kept < text.len();
        (text[..kept].to_owned(), truncated)
    }

    /// Extract a row range without allocating beyond an explicit UTF-8 byte
    /// budget. This is intended for semantic-region consumers that must not
    /// materialize an arbitrarily large command or output block before
    /// applying their own retention policy.
    ///
    /// The oldest requested text is retained. `truncated` is true when rows
    /// were no longer retained or the byte budget cut the requested range.
    pub fn bounded_rows_text(
        &self,
        start_row: i32,
        end_row_inclusive: i32,
        max_bytes: usize,
    ) -> (String, bool) {
        if end_row_inclusive < start_row {
            return (String::new(), false);
        }
        let oldest = -i32::try_from(self.grid().scrollback_len()).unwrap_or(i32::MAX);
        let newest = i32::try_from(self.rows.saturating_sub(1)).unwrap_or(i32::MAX);
        let start = start_row.max(oldest);
        let end = end_row_inclusive.min(newest);
        let mut truncated = start != start_row || end != end_row_inclusive;
        if start > end || max_bytes == 0 {
            return (String::new(), truncated || start <= end);
        }

        let mut output = String::with_capacity(max_bytes.min(4096));
        for row_index in start..=end {
            if row_index != start {
                if output.len() == max_bytes {
                    return (output, true);
                }
                output.push('\n');
            }
            let line = self
                .grid()
                .row_at_offset(row_index)
                .map(|row| row.plain_text(self.cols))
                .unwrap_or_default();
            let line = line.trim_end();
            let remaining = max_bytes.saturating_sub(output.len());
            if line.len() <= remaining {
                output.push_str(line);
                continue;
            }
            output.push_str(&line[..crate::safe_text::floor_char_boundary(line, remaining)]);
            truncated = true;
            break;
        }
        (output, truncated)
    }

    /// Like [`Self::bounded_rows_text`], but the newest requested text is
    /// retained: rows are read backwards from `end_row_inclusive`, and the
    /// row that crosses the budget keeps only its end, cut on a character
    /// boundary. Output is read this way because its end (the error, the
    /// summary) matters most. `truncated` is true when the start of the
    /// requested range was dropped, by scrollback eviction or the budget.
    pub fn bounded_rows_text_tail(
        &self,
        start_row: i32,
        end_row_inclusive: i32,
        max_bytes: usize,
    ) -> (String, bool) {
        if end_row_inclusive < start_row {
            return (String::new(), false);
        }
        let oldest = -i32::try_from(self.grid().scrollback_len()).unwrap_or(i32::MAX);
        let newest = i32::try_from(self.rows.saturating_sub(1)).unwrap_or(i32::MAX);
        let start = start_row.max(oldest);
        let end = end_row_inclusive.min(newest);
        let mut truncated = start != start_row || end != end_row_inclusive;
        if start > end || max_bytes == 0 {
            return (String::new(), truncated || start <= end);
        }

        // Newest first; reversed and joined once the budget is spent.
        let mut lines = Vec::new();
        let mut used = 0_usize;
        for row_index in (start..=end).rev() {
            let separator = usize::from(row_index != end);
            let line = self
                .grid()
                .row_at_offset(row_index)
                .map(|row| row.plain_text(self.cols))
                .unwrap_or_default();
            let line = line.trim_end();
            let remaining = max_bytes - used;
            if line.len() + separator <= remaining {
                lines.push(line.to_owned());
                used += line.len() + separator;
                continue;
            }
            truncated = true;
            let room = remaining.saturating_sub(separator);
            let cut = crate::safe_text::ceil_char_boundary(line, line.len() - room.min(line.len()));
            if cut < line.len() {
                lines.push(line[cut..].to_owned());
            }
            break;
        }
        lines.reverse();
        (lines.join("\n"), truncated)
    }

    /// Expand a click point to its word boundaries (for double-click semantic
    /// selection). Returns `(start, end_inclusive)`. If the clicked cell is
    /// whitespace or empty, returns `(point, point)` unchanged so the caller
    /// falls back to a zero-width selection.
    pub fn word_at(&self, point: Point) -> (Point, Point) {
        let grid = self.presentation_grid();
        let Some(row) = grid.row_at_offset(point.row) else {
            return (point, point);
        };
        let cell_char = |col: usize| -> Option<char> {
            let cell = row.get(col)?;
            if cell.is_wide_char_spacer() {
                col.checked_sub(1)
                    .and_then(|leader| row.get(leader))
                    .and_then(|leader| leader.content.char())
            } else if cell.multicell.is_present() {
                col.checked_sub(cell.multicell.relative_col())
                    .and_then(|leader| row.get(leader))
                    .and_then(|leader| leader.content.char())
            } else {
                cell.content.char()
            }
        };

        // Anchor must itself be a word char, otherwise don't expand.
        match cell_char(point.col) {
            Some(c) if is_word_char(c) => {}
            _ => return (point, point),
        }

        let max_col = grid.cols().saturating_sub(1);
        // Walk left
        let mut start_col = point.col;
        while start_col > 0 {
            match cell_char(start_col - 1) {
                Some(c) if is_word_char(c) => start_col -= 1,
                _ => break,
            }
        }
        // Walk right
        let mut end_col = point.col;
        while end_col < max_col {
            match cell_char(end_col + 1) {
                Some(c) if is_word_char(c) => end_col += 1,
                _ => break,
            }
        }

        (
            Point::new(point.row, start_col),
            Point::new(point.row, end_col),
        )
    }

    /// Expand a click point to its full line (for triple-click line selection).
    /// Trailing whitespace is excluded so the selection doesn't drag past the
    /// visible end of the line.
    pub fn line_at(&self, point: Point) -> (Point, Point) {
        let grid = self.presentation_grid();
        let Some(row) = grid.row_at_offset(point.row) else {
            return (point, point);
        };
        let max_col = grid.cols().saturating_sub(1);
        let mut end_col = max_col;
        while end_col > 0 {
            let ch = row.get(end_col).and_then(|c| c.content.char());
            match ch {
                Some(c) if !c.is_whitespace() => break,
                _ => end_col -= 1,
            }
        }
        (Point::new(point.row, 0), Point::new(point.row, end_col))
    }

    /// Get text content for selection. Honors the selection type:
    /// - Simple/Semantic: contiguous logical text, joining soft-wrapped rows.
    /// - Lines: full rows from start to end.
    /// - Block: rectangular region (one newline between rows).
    pub fn get_selection_text(&self) -> Option<String> {
        let selection = self.selection()?;
        Some(Self::selection_text(self.presentation_grid(), selection))
    }

    /// Extract an explicit selection in presentation-grid coordinates without
    /// reading or changing the live terminal's selection.
    pub fn presentation_selection_text(&self, selection: &Selection) -> String {
        Self::selection_text(self.presentation_grid(), selection)
    }

    /// Extract only selected rows from a sorted, unique projection index. Gaps
    /// always separate lines, even if the preceding source row was soft-wrapped.
    pub fn presentation_selection_text_in_rows(
        &self,
        selection: &Selection,
        rows: &[i32],
    ) -> String {
        let (start, end) = selection.bounds();
        let first = rows.partition_point(|row| *row < start.row);
        let last = rows.partition_point(|row| *row <= end.row);
        Self::selection_text_rows(
            self.presentation_grid(),
            selection,
            rows[first..last].iter().copied(),
        )
    }

    fn selection_text(grid: &Grid, selection: &Selection) -> String {
        let (start, end) = selection.bounds();
        Self::selection_text_rows(grid, selection, start.row..=end.row)
    }

    fn selection_text_rows(
        grid: &Grid,
        selection: &Selection,
        rows: impl Iterator<Item = i32>,
    ) -> String {
        let (start, end) = selection.bounds();

        let max_col = grid.cols().saturating_sub(1);
        let mut text = String::new();
        let logical_text = matches!(
            selection.ty,
            SelectionType::Simple | SelectionType::Semantic
        );

        let mut rows = rows.peekable();
        while let Some(row) = rows.next() {
            let Some(grid_row) = grid.row_at_offset(row) else {
                continue;
            };

            // Resolve the column range based on selection type.
            let (mut start_col, end_col) = match selection.ty {
                SelectionType::Simple | SelectionType::Semantic => {
                    let s = if row == start.row { start.col } else { 0 };
                    let e = if row == end.row { end.col } else { max_col };
                    (s.min(max_col), e.min(max_col))
                }
                SelectionType::Lines => (0, max_col),
                SelectionType::Block => {
                    let s = start.col.min(end.col);
                    let e = start.col.max(end.col);
                    (s.min(max_col), e.min(max_col))
                }
            };

            if grid_row
                .get(start_col)
                .is_some_and(|cell| cell.is_wide_char_spacer())
            {
                start_col = start_col.saturating_sub(1);
            } else if let Some(cell) = grid_row.get(start_col)
                && cell.multicell.is_present()
                && cell.multicell.relative_row() == 0
            {
                start_col = start_col.saturating_sub(cell.multicell.relative_col());
            }

            let skip_wrap_padding = logical_text && grid_row.has_wide_wrap_padding();
            for col in start_col..=end_col {
                if skip_wrap_padding && col + 1 == grid_row.len() {
                    continue;
                }
                if let Some(cell) = grid_row.get(col) {
                    if cell.is_wide_char_spacer()
                        || (cell.multicell.is_present() && !cell.multicell.is_leader())
                    {
                        continue;
                    }
                    match &cell.content {
                        CellContent::Empty => text.push(' '),
                        CellContent::Char(c) => text.push(*c),
                        CellContent::Grapheme(g) => text.push_str(g),
                    }
                }
            }

            if let Some(next) = rows.peek()
                && (!logical_text || !grid_row.is_wrapped() || row.checked_add(1) != Some(*next))
            {
                text.push('\n');
            }
        }

        text.trim_end().to_string()
    }
}

/// Whether `c` counts as part of a word for double-click expansion.
/// Covers alphanumerics plus the common path / URL / identifier punctuation
/// so filenames, URLs, and code identifiers select as a single unit.
fn is_word_char(c: char) -> bool {
    c.is_alphanumeric()
        || matches!(
            c,
            '_' | '-' | '.' | '/' | ':' | '@' | '+' | '~' | '%' | '?' | '&' | '=' | '#'
        )
}

#[cfg(test)]
mod tests;
