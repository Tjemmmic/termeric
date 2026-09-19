//! Terminal state machine
//!
//! This module contains the main terminal state including modes,
//! cursor handling, and the interface for processing escape sequences.

mod cursor;
#[cfg(test)]
mod graphics_history;
#[cfg(test)]
mod graphics_resize;
mod keyboard;
mod mode;
mod multicell;
mod multiple_cursors;
mod notification;
mod pointer;
#[cfg(test)]
mod reflow_prompts;
#[cfg(test)]
mod scroll_bce;
#[cfg(test)]
mod scroll_graphics;
#[cfg(test)]
mod scroll_limits;
#[cfg(test)]
mod scroll_prompts;
#[cfg(test)]
mod scroll_selection;
mod selection;
mod selection_damage;
mod title;

pub use cursor::{SavedCursor, TerminalCursorShape, TerminalCursorStyle};
pub use keyboard::{
    KITTY_KEYBOARD_STACK_DEPTH, KittyKeyboardFlags, MAX_USER_DEFINED_KEY_BYTES,
    MAX_USER_DEFINED_KEYS_BYTES, XTERM_USER_DEFINED_KEY_COUNT, XtermCursorKeys, XtermFunctionKeys,
    XtermKeyFormat, XtermKeyMode, XtermKeyboardResource, XtermKeyboardType, XtermKeypadKeys,
    XtermModifierKeys, XtermModifyOtherKeys, XtermSpecialKeys, XtermStringKeys,
    XtermUserDefinedKeys,
};
pub use mode::TerminalMode;
pub use multicell::MAX_OSC66_TEXT_BYTES;
use multicell::Osc66Command;
pub use multiple_cursors::{
    ExtraCursor, ExtraCursorColor, ExtraCursorColors, ExtraCursorShape, ExtraCursorState,
    MAX_EXTRA_CURSOR_CELLS, MAX_EXTRA_CURSOR_DIMENSION,
};
pub use notification::{
    MAX_NOTIFICATION_BODY_BYTES, MAX_NOTIFICATION_ICON_BYTES, MAX_NOTIFICATION_IDENTIFIER_BYTES,
    MAX_NOTIFICATION_TITLE_BYTES, MAX_OSC99_DECODED_CHUNK_BYTES, MAX_OSC99_ENCODED_CHUNK_BYTES,
    MAX_PENDING_NOTIFICATION_BYTES, MAX_PENDING_NOTIFICATIONS,
};
use notification::{NotificationProtocol, Osc99Action};
#[cfg(test)]
pub(crate) use pointer::MAX_POINTER_SHAPE_PAYLOAD_BYTES;
pub use pointer::{POINTER_SHAPE_STACK_DEPTH, PointerShape};
pub use selection::{Point, Selection, SelectionType};
pub(crate) use title::MAX_TITLE_BYTES;
pub use title::XTERM_TITLE_STACK_DEPTH;
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
use crate::shell::ShellIntegration;
use base64::{Engine as _, engine::general_purpose::STANDARD};
use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};
use unicode_segmentation::UnicodeSegmentation;

/// Tab stop interval
const TAB_INTERVAL: usize = 8;
/// REP remembers one normal grapheme but does not duplicate a pathological,
/// attacker-sized combining sequence into a compact control sequence.
const MAX_REPEAT_GRAPHEME_BYTES: usize = 1024;
const MAX_GRAPHICS_RELAY_APC_BYTES: usize = 64 * 1024;
/// Safety valve for applications that enter synchronized output and never
/// leave it. The protocol deliberately does not prescribe a timeout; one
/// second bounds stale presentation and snapshot memory without affecting a
/// normal frame-sized update.
pub const SYNCHRONIZED_OUTPUT_TIMEOUT: Duration = Duration::from_secs(1);

fn graphics_relay_checksum(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf29ce484222325, |hash, byte| {
        (hash ^ u64::from(*byte)).wrapping_mul(0x100000001b3)
    })
}

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
struct TerminalRect {
    top: usize,
    bottom: usize,
    left: usize,
    right: usize,
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct PromptNavigationState {
    row: i32,
    scroll_offset: usize,
}

#[cfg(test)]
fn shell_logical_anchor(grid: &Grid, target_row: i32) -> Option<ShellLogicalAnchor> {
    let oldest = -(grid.scrollback_len() as i32);
    let newest = grid.rows().saturating_sub(1) as i32;
    if !(oldest..=newest).contains(&target_row) {
        return None;
    }
    let mut line = 0_usize;
    let mut cell = 0_usize;
    for (row_index, row) in shell_anchor_rows(grid) {
        if row_index == target_row {
            return Some(ShellLogicalAnchor { line, cell });
        }
        if row.is_wrapped() {
            cell = cell.saturating_add(row.reflow_unit_count());
        } else {
            line = line.saturating_add(1);
            cell = 0;
        }
    }
    None
}

#[cfg(test)]
fn shell_row_from_anchor(grid: &Grid, target: ShellLogicalAnchor) -> Option<i32> {
    let mut line = 0_usize;
    let mut remaining = target.cell;
    for (row_index, row) in shell_anchor_rows(grid) {
        if line == target.line {
            let units = row.reflow_unit_count();
            if remaining < units || !row.is_wrapped() {
                return Some(row_index);
            }
            remaining = remaining.saturating_sub(units);
        }
        if !row.is_wrapped() {
            if line == target.line {
                return Some(row_index);
            }
            line = line.saturating_add(1);
            remaining = target.cell;
        }
    }
    None
}

#[cfg(test)]
thread_local! {
    static SHELL_REFLOW_ROW_VISITS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

fn shell_anchor_rows(grid: &Grid) -> impl Iterator<Item = (i32, crate::grid::RowView<'_>)> {
    let oldest = -(grid.scrollback_len() as i32);
    let newest = grid.rows().saturating_sub(1) as i32;
    (oldest..=newest)
        .map_while(|index| grid.row_at_offset(index).map(|row| (index, row)))
        .inspect(|_| {
            #[cfg(test)]
            SHELL_REFLOW_ROW_VISITS.with(|visits| visits.set(visits.get() + 1));
        })
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

impl TerminalRect {
    fn width(self) -> usize {
        self.right - self.left
    }

    fn height(self) -> usize {
        self.bottom - self.top
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct DecAttributeChange {
    set: CellAttrs,
    clear: CellAttrs,
}

impl DecAttributeChange {
    fn set(&mut self, attrs: CellAttrs) {
        self.clear.remove(attrs);
        self.set.insert(attrs);
    }

    fn clear(&mut self, attrs: CellAttrs) {
        self.set.remove(attrs);
        self.clear.insert(attrs);
    }

    fn apply(self, attrs: &mut CellAttrs) {
        attrs.remove(self.clear);
        attrs.insert(self.set);
    }

    fn is_empty(self) -> bool {
        self.set.is_empty() && self.clear.is_empty()
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct DecAttributeReverse {
    bold: bool,
    underline: bool,
    blink: bool,
    inverse: bool,
    hidden: bool,
}

impl DecAttributeReverse {
    fn is_empty(self) -> bool {
        !(self.bold || self.underline || self.blink || self.inverse || self.hidden)
    }

    fn apply(self, attrs: &mut CellAttrs) {
        if self.bold {
            attrs.toggle(CellAttrs::BOLD);
        }
        if self.underline {
            if attrs.intersects(CellAttrs::ALL_UNDERLINES) {
                attrs.remove(CellAttrs::ALL_UNDERLINES);
            } else {
                attrs.insert(CellAttrs::UNDERLINE);
            }
        }
        if self.blink {
            attrs.toggle(CellAttrs::BLINK);
        }
        if self.inverse {
            attrs.toggle(CellAttrs::INVERSE);
        }
        if self.hidden {
            attrs.toggle(CellAttrs::HIDDEN);
        }
    }
}

fn dec_checksum_attributes(cell: &Cell) -> u16 {
    let mut value = 0_u16;
    if cell.flags.contains(CellFlags::PROTECTED) {
        value |= 0x04;
    }
    if cell.attrs.contains(CellAttrs::HIDDEN) {
        value |= 0x08;
    }
    if cell.attrs.has_underline() {
        value |= 0x10;
    }
    if cell.attrs.contains(CellAttrs::INVERSE) {
        value |= 0x20;
    }
    if cell.attrs.contains(CellAttrs::BLINK) {
        value |= 0x40;
    }
    if cell.attrs.contains(CellAttrs::BOLD) {
        value |= 0x80;
    }
    value
}

fn dec_checksum_character(cell: &Cell, character: char) -> u16 {
    let dec = if cell.flags.contains(CellFlags::DEC_SPECIAL_GRAPHICS) {
        match character {
            ' ' => b'_',
            '◆' => b'`',
            '▒' => b'a',
            '␉' => b'b',
            '␌' => b'c',
            '␍' => b'd',
            '␊' => b'e',
            '°' => b'f',
            '±' => b'g',
            '␤' => b'h',
            '␋' => b'i',
            '┘' => b'j',
            '┐' => b'k',
            '┌' => b'l',
            '└' => b'm',
            '┼' => b'n',
            '⎺' => b'o',
            '⎻' => b'p',
            '─' => b'q',
            '⎼' => b'r',
            '⎽' => b's',
            '├' => b't',
            '┤' => b'u',
            '┴' => b'v',
            '┬' => b'w',
            '│' => b'x',
            '≤' => b'y',
            '≥' => b'z',
            'π' => b'{',
            '≠' => b'|',
            '£' => b'}',
            '·' => b'~',
            _ => (character as u32 & 0xff) as u8,
        }
    } else if cell.flags.contains(CellFlags::DEC_UK_CHARSET) && character == '£' {
        b'#'
    } else if character == '␦' {
        0x1b
    } else {
        (character as u32 & 0xff) as u8
    };
    u16::from(dec)
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct CursorPresentationState {
    row: usize,
    col: usize,
    bold: bool,
    underline: bool,
    blink: bool,
    inverse: bool,
    protected: bool,
    wrap_pending: bool,
    origin_mode: bool,
    gl_register: u8,
    gr_register: u8,
    single_shift: Option<u8>,
    g_sets: [CharacterSet; 4],
}

impl CursorPresentationState {
    fn parse(data: &[u8], rows: usize, cols: usize) -> Option<Self> {
        // DECCIR has ten small, fixed-format fields. Bound this separately from
        // the much larger DCS allowance required by graphics payloads so a
        // malformed report cannot amplify into a large temporary field vector.
        if data.len() > 128 {
            return None;
        }
        let fields: Vec<&[u8]> = data.split(|byte| *byte == b';').collect();
        if fields.len() != 10 {
            return None;
        }
        let row = parse_decimal(fields[0])?;
        let col = parse_decimal(fields[1])?;
        if row == 0 || row > rows || col == 0 || col > cols || parse_decimal(fields[2])? != 1 {
            return None;
        }

        let rendition = report_byte(fields[3], 0x0f)?;
        let protection = report_byte(fields[4], 0x01)?;
        let flags = report_byte(fields[5], 0x0f)?;
        if flags & 0x06 == 0x06 {
            // SS2 and SS3 are mutually exclusive pending states.
            return None;
        }
        let gl_register = u8::try_from(parse_decimal(fields[6])?).ok()?;
        let gr_register = u8::try_from(parse_decimal(fields[7])?).ok()?;
        if gl_register >= 4 || gr_register >= 4 || fields[8] != b"@" {
            return None;
        }
        let [g0, g1, g2, g3] = fields[9] else {
            return None;
        };
        let g_sets = [
            CharacterSet::from_designator(*g0)?,
            CharacterSet::from_designator(*g1)?,
            CharacterSet::from_designator(*g2)?,
            CharacterSet::from_designator(*g3)?,
        ];

        Some(Self {
            row: row - 1,
            col: col - 1,
            bold: rendition & 0x01 != 0,
            underline: rendition & 0x02 != 0,
            blink: rendition & 0x04 != 0,
            inverse: rendition & 0x08 != 0,
            protected: protection & 0x01 != 0,
            wrap_pending: flags & 0x08 != 0,
            origin_mode: flags & 0x01 != 0,
            gl_register,
            gr_register,
            single_shift: if flags & 0x04 != 0 {
                Some(3)
            } else if flags & 0x02 != 0 {
                Some(2)
            } else {
                None
            },
            g_sets,
        })
    }
}

fn report_byte(field: &[u8], allowed_bits: u8) -> Option<u8> {
    let [value] = field else {
        return None;
    };
    (*value & !allowed_bits == b'@').then_some(*value)
}

fn parse_decimal(field: &[u8]) -> Option<usize> {
    if field.is_empty() {
        return None;
    }
    field.iter().try_fold(0_usize, |value, byte| {
        let digit = byte.checked_sub(b'0').filter(|digit| *digit <= 9)?;
        value.checked_mul(10)?.checked_add(usize::from(digit))
    })
}

fn parse_tab_stop_report(data: &[u8], cols: usize) -> Option<Vec<bool>> {
    let digits = cols.max(1).to_string().len();
    if data.len() > cols.saturating_mul(digits.saturating_add(1)) {
        return None;
    }
    let mut tabs = vec![false; cols];
    if data.is_empty() {
        return Some(tabs);
    }
    for (count, field) in data.split(|byte| *byte == b'/').enumerate() {
        if count >= cols {
            return None;
        }
        let column = parse_decimal(field)?;
        if column == 0 || column > cols {
            return None;
        }
        tabs[column - 1] = true;
    }
    Some(tabs)
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
    /// Xterm modifyCursorKeys applies across both screen buffers.
    xterm_cursor_keys: XtermCursorKeys,
    /// Xterm modifyFunctionKeys applies across both screen buffers.
    xterm_function_keys: XtermFunctionKeys,
    /// Xterm modifyKeypadKeys applies across both screen buffers.
    xterm_keypad_keys: XtermKeypadKeys,
    /// Xterm's reserved modifyStringKeys/formatStringKeys resource 5.
    xterm_string_keys: XtermStringKeys,
    /// Xterm modifyModifierKeys applies across both screen buffers.
    xterm_modifier_keys: XtermModifierKeys,
    /// Xterm modifySpecialKeys applies across both screen buffers.
    xterm_special_keys: XtermSpecialKeys,
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
    shell: ShellIntegration,
    /// Exact prompt selected by the previous/next prompt actions. The saved
    /// offset lets direct/manual scrolling invalidate this navigation cursor.
    prompt_navigation: Option<PromptNavigationState>,
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
            xterm_cursor_keys: XtermCursorKeys::default(),
            xterm_function_keys: XtermFunctionKeys::default(),
            xterm_keypad_keys: XtermKeypadKeys::default(),
            xterm_string_keys: XtermStringKeys::default(),
            xterm_modifier_keys: XtermModifierKeys::default(),
            xterm_special_keys: XtermSpecialKeys::default(),
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
            shell: ShellIntegration::new(),
            prompt_navigation: None,
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
    pub fn application_icon_name(&self) -> &str {
        self.application_titles.icon()
    }

    /// Last application-authored OSC 0/2 window label. This never reflects a
    /// user-renamed tab, process-derived label, or working directory.
    pub fn application_window_title(&self) -> &str {
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
                #[cfg(target_os = "linux")]
                let capabilities = "p=title,body,close,alive,buttons,icon:a=report,focus:c=1:o=always,unfocused,invisible:s=system,silent,error,warn,warning,info,question:u=0,1,2:w=1";
                #[cfg(not(target_os = "linux"))]
                let capabilities = "p=title,body:o=always,unfocused,invisible";
                let response = format!("\x1b]99;i={identifier}:p=?;{capabilities}\x1b\\");
                self.push_event(TerminalEvent::PtyWrite(response.into_bytes()));
            }
            Osc99Action::AliveQuery(identifier) => {
                #[cfg(target_os = "linux")]
                self.push_event(TerminalEvent::NotificationAliveQuery {
                    identifier,
                    reply_uses_8bit_controls: self.use_8bit_controls,
                });
                #[cfg(not(target_os = "linux"))]
                let _ = identifier;
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
        template.flags.insert(CellFlags::DIRTY);
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
    pub fn alternate_screen_active(&self) -> bool {
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
                let mut start_byte = line.len().saturating_sub(remaining);
                while start_byte < line.len() && !line.is_char_boundary(start_byte) {
                    start_byte += 1;
                }
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
    pub fn pointer_shape(&self) -> Option<PointerShape> {
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

    /// Apply DECSCUSR. Invalid parameters leave the current style unchanged.
    pub(crate) fn set_cursor_style(&mut self, parameter: u16) {
        if let Some(style) = TerminalCursorStyle::from_parameter(parameter) {
            self.cursor_style = Some(style);
            self.cursor_blink_override = None;
            self.set_mode(TerminalMode::CURSOR_BLINKING, style.blinking);
        }
    }

    pub fn cursor_style_parameter(&self) -> u8 {
        self.cursor_style.map_or(0, TerminalCursorStyle::parameter)
    }

    /// Return a DECRPM status value for a mode whose behavior is represented
    /// authoritatively by this terminal. Unknown or momentary modes report 0.
    pub(crate) fn mode_status(&self, private: bool, number: u16) -> u8 {
        let enabled = if private {
            match number {
                1 => Some(self.mode.contains(TerminalMode::CURSOR_KEYS_APPLICATION)),
                6 => Some(self.mode.contains(TerminalMode::ORIGIN)),
                7 => Some(self.mode.contains(TerminalMode::AUTOWRAP)),
                9 => Some(self.mode.contains(TerminalMode::MOUSE_X10)),
                12 => Some(self.mode.contains(TerminalMode::CURSOR_BLINKING)),
                25 => Some(self.mode.contains(TerminalMode::CURSOR_VISIBLE)),
                69 => Some(self.mode.contains(TerminalMode::LEFT_RIGHT_MARGIN)),
                1000 => Some(self.mode.contains(TerminalMode::MOUSE_TRACKING)),
                1002 => Some(self.mode.contains(TerminalMode::MOUSE_BUTTON_EVENT)),
                1003 => Some(self.mode.contains(TerminalMode::MOUSE_ANY_EVENT)),
                1004 => Some(self.mode.contains(TerminalMode::FOCUS_EVENTS)),
                1005 => Some(self.mode.contains(TerminalMode::MOUSE_UTF8)),
                1006 => Some(self.mode.contains(TerminalMode::MOUSE_SGR)),
                1007 => Some(self.mode.contains(TerminalMode::ALTERNATE_SCROLL)),
                1060 => Some(self.xterm_keyboard_type == XtermKeyboardType::Legacy),
                1061 => Some(self.xterm_keyboard_type == XtermKeyboardType::Vt220),
                2004 => Some(self.mode.contains(TerminalMode::BRACKETED_PASTE)),
                2026 => Some(self.mode.contains(TerminalMode::SYNCHRONIZED_OUTPUT)),
                _ => None,
            }
        } else {
            match number {
                4 => Some(self.mode.contains(TerminalMode::INSERT)),
                20 => Some(self.mode.contains(TerminalMode::LINEFEED_NEWLINE)),
                _ => None,
            }
        };
        enabled.map_or(0, |enabled| if enabled { 1 } else { 2 })
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

    /// Get xterm's terminal-local resource-zero `modifyKeyboard` and
    /// `formatKeyboard` state.
    pub fn xterm_keyboard_resource(&self) -> XtermKeyboardResource {
        self.xterm_keyboard_resource
    }

    /// Get the global xterm-compatible keyboard type selected by private mode
    /// 1060 or 1061.
    pub fn xterm_keyboard_type(&self) -> XtermKeyboardType {
        self.xterm_keyboard_type
    }

    /// Get the configured keyboard type restored by RIS.
    pub fn initial_xterm_keyboard_type(&self) -> XtermKeyboardType {
        self.initial_xterm_keyboard_type
    }

    /// Get the decoded DECUDK definition for a DEC function-key code.
    pub fn xterm_user_defined_key(&self, code: u8) -> Option<&[u8]> {
        self.xterm_user_defined_keys.get(code)
    }

    /// Iterate over defined DECUDK entries in ascending DEC key-code order.
    pub fn xterm_user_defined_keys(&self) -> impl Iterator<Item = (u8, &[u8])> {
        self.xterm_user_defined_keys.iter()
    }

    pub fn xterm_user_defined_key_bytes(&self) -> usize {
        self.xterm_user_defined_keys.bytes()
    }

    pub(crate) fn clear_xterm_user_defined_keys(&mut self) {
        self.xterm_user_defined_keys.clear();
    }

    pub(crate) fn define_xterm_user_defined_key(&mut self, code: u16, definition: Vec<u8>) {
        self.xterm_user_defined_keys.define(code, definition);
    }

    /// Change the configured RIS baseline. Callers applying a changed live
    /// configuration may also replace the current application-selected type;
    /// callers merely propagating policy to an existing owner can preserve it.
    pub fn configure_xterm_keyboard_type(
        &mut self,
        keyboard_type: XtermKeyboardType,
        apply_current: bool,
    ) {
        self.initial_xterm_keyboard_type = keyboard_type;
        if apply_current {
            self.xterm_keyboard_type = keyboard_type;
            self.saved_xterm_keyboard_type = None;
        }
    }

    /// Apply xterm's mutually exclusive keyboard-type private modes. Resetting
    /// either mode returns to the normal type, even if the other type was
    /// active, matching xterm's `set_keyboard_type` behavior.
    pub(crate) fn set_xterm_keyboard_type(
        &mut self,
        keyboard_type: XtermKeyboardType,
        enable: bool,
    ) {
        self.xterm_keyboard_type = if enable {
            keyboard_type
        } else {
            XtermKeyboardType::Normal
        };
    }

    pub(crate) fn save_xterm_keyboard_type(&mut self) {
        self.saved_xterm_keyboard_type = Some(self.xterm_keyboard_type);
    }

    pub(crate) fn restore_xterm_keyboard_type(&mut self) {
        self.xterm_keyboard_type = self
            .saved_xterm_keyboard_type
            .unwrap_or(self.initial_xterm_keyboard_type);
    }

    pub(crate) fn set_xterm_keyboard_modify(&mut self, parameter: u16) {
        self.xterm_keyboard_resource = self.xterm_keyboard_resource.with_modify(parameter);
    }

    pub(crate) fn reset_xterm_keyboard_modify(&mut self) {
        self.xterm_keyboard_resource = self
            .xterm_keyboard_resource
            .with_modify(XtermKeyboardResource::default().modify());
    }

    pub(crate) fn report_xterm_keyboard_modify(&mut self) {
        self.push_event(TerminalEvent::PtyWrite(
            format!("\x1b[>0;{}m", self.xterm_keyboard_resource.modify()).into_bytes(),
        ));
    }

    pub(crate) fn set_xterm_keyboard_format(&mut self, parameter: u16) {
        self.xterm_keyboard_resource = self.xterm_keyboard_resource.with_format(parameter);
    }

    pub(crate) fn reset_xterm_keyboard_format(&mut self) {
        self.xterm_keyboard_resource = self
            .xterm_keyboard_resource
            .with_format(XtermKeyboardResource::default().format());
    }

    pub(crate) fn report_xterm_keyboard_format(&mut self) {
        self.push_event(TerminalEvent::PtyWrite(
            format!("\x1b[>0;{}f", self.xterm_keyboard_resource.format()).into_bytes(),
        ));
    }

    /// Get xterm's terminal-local `modifyCursorKeys` and `formatCursorKeys`
    /// policy.
    pub fn xterm_cursor_keys(&self) -> XtermCursorKeys {
        self.xterm_cursor_keys
    }

    pub(crate) fn set_xterm_cursor_key_mode(&mut self, parameter: u16) {
        self.xterm_cursor_keys.set_mode(parameter);
    }

    pub(crate) fn reset_xterm_cursor_key_mode(&mut self) {
        self.xterm_cursor_keys = self.xterm_cursor_keys.with_mode(XtermKeyMode::default());
    }

    pub(crate) fn report_xterm_cursor_key_mode(&mut self) {
        self.push_event(TerminalEvent::PtyWrite(
            format!("\x1b[>1;{}m", self.xterm_cursor_keys.mode().parameter()).into_bytes(),
        ));
    }

    pub(crate) fn set_xterm_cursor_key_format(&mut self, parameter: u16) {
        self.xterm_cursor_keys.set_format(parameter);
    }

    pub(crate) fn reset_xterm_cursor_key_format(&mut self) {
        self.xterm_cursor_keys = self
            .xterm_cursor_keys
            .with_format(XtermKeyFormat::default());
    }

    pub(crate) fn report_xterm_cursor_key_format(&mut self) {
        self.push_event(TerminalEvent::PtyWrite(
            format!("\x1b[>1;{}f", self.xterm_cursor_keys.format().parameter()).into_bytes(),
        ));
    }

    /// Get xterm's terminal-local `modifyFunctionKeys` and
    /// `formatFunctionKeys` policy.
    pub fn xterm_function_keys(&self) -> XtermFunctionKeys {
        self.xterm_function_keys
    }

    pub(crate) fn set_xterm_function_key_mode(&mut self, parameter: u16) {
        self.xterm_function_keys.set_mode(parameter);
    }

    pub(crate) fn reset_xterm_function_key_mode(&mut self) {
        self.xterm_function_keys = self.xterm_function_keys.with_mode(XtermKeyMode::default());
    }

    pub(crate) fn report_xterm_function_key_mode(&mut self) {
        self.push_event(TerminalEvent::PtyWrite(
            format!("\x1b[>2;{}m", self.xterm_function_keys.mode().parameter()).into_bytes(),
        ));
    }

    pub(crate) fn set_xterm_function_key_format(&mut self, parameter: u16) {
        self.xterm_function_keys.set_format(parameter);
    }

    pub(crate) fn reset_xterm_function_key_format(&mut self) {
        self.xterm_function_keys = self
            .xterm_function_keys
            .with_format(XtermKeyFormat::default());
    }

    pub(crate) fn report_xterm_function_key_format(&mut self) {
        self.push_event(TerminalEvent::PtyWrite(
            format!("\x1b[>2;{}f", self.xterm_function_keys.format().parameter()).into_bytes(),
        ));
    }

    /// Get xterm's terminal-local `modifyKeypadKeys` and
    /// `formatKeypadKeys` policy.
    pub fn xterm_keypad_keys(&self) -> XtermKeypadKeys {
        self.xterm_keypad_keys
    }

    pub(crate) fn set_xterm_keypad_key_mode(&mut self, parameter: u16) {
        self.xterm_keypad_keys.set_mode(parameter);
    }

    pub(crate) fn reset_xterm_keypad_key_mode(&mut self) {
        self.xterm_keypad_keys = self
            .xterm_keypad_keys
            .with_mode(XtermKeypadKeys::default().mode());
    }

    pub(crate) fn report_xterm_keypad_key_mode(&mut self) {
        self.push_event(TerminalEvent::PtyWrite(
            format!("\x1b[>3;{}m", self.xterm_keypad_keys.mode().parameter()).into_bytes(),
        ));
    }

    pub(crate) fn set_xterm_keypad_key_format(&mut self, parameter: u16) {
        self.xterm_keypad_keys.set_format(parameter);
    }

    pub(crate) fn reset_xterm_keypad_key_format(&mut self) {
        self.xterm_keypad_keys = self
            .xterm_keypad_keys
            .with_format(XtermKeyFormat::default());
    }

    pub(crate) fn report_xterm_keypad_key_format(&mut self) {
        self.push_event(TerminalEvent::PtyWrite(
            format!("\x1b[>3;{}f", self.xterm_keypad_keys.format().parameter()).into_bytes(),
        ));
    }

    /// Get xterm's reserved `modifyStringKeys` and `formatStringKeys` state.
    pub fn xterm_string_keys(&self) -> XtermStringKeys {
        self.xterm_string_keys
    }

    pub(crate) fn set_xterm_string_key_mode(&mut self, parameter: u16) {
        self.xterm_string_keys.set_mode(parameter);
    }

    pub(crate) fn reset_xterm_string_key_mode(&mut self) {
        self.xterm_string_keys = self
            .xterm_string_keys
            .with_mode(XtermStringKeys::default().mode());
    }

    pub(crate) fn report_xterm_string_key_mode(&mut self) {
        self.push_event(TerminalEvent::PtyWrite(
            format!("\x1b[>5;{}m", self.xterm_string_keys.mode().parameter()).into_bytes(),
        ));
    }

    pub(crate) fn set_xterm_string_key_format(&mut self, parameter: u16) {
        self.xterm_string_keys.set_format(parameter);
    }

    pub(crate) fn reset_xterm_string_key_format(&mut self) {
        self.xterm_string_keys = self
            .xterm_string_keys
            .with_format(XtermKeyFormat::default());
    }

    pub(crate) fn report_xterm_string_key_format(&mut self) {
        self.push_event(TerminalEvent::PtyWrite(
            format!("\x1b[>5;{}f", self.xterm_string_keys.format().parameter()).into_bytes(),
        ));
    }

    /// Get xterm's terminal-local `modifyModifierKeys` and
    /// `formatModifierKeys` policy.
    pub fn xterm_modifier_keys(&self) -> XtermModifierKeys {
        self.xterm_modifier_keys
    }

    pub(crate) fn set_xterm_modifier_key_mode(&mut self, parameter: u16) {
        self.xterm_modifier_keys.set_mode(parameter);
    }

    pub(crate) fn reset_xterm_modifier_key_mode(&mut self) {
        self.xterm_modifier_keys = self
            .xterm_modifier_keys
            .with_mode(XtermModifierKeys::default().mode());
    }

    pub(crate) fn report_xterm_modifier_key_mode(&mut self) {
        self.push_event(TerminalEvent::PtyWrite(
            format!("\x1b[>6;{}m", self.xterm_modifier_keys.mode().parameter()).into_bytes(),
        ));
    }

    pub(crate) fn set_xterm_modifier_key_format(&mut self, parameter: u16) {
        self.xterm_modifier_keys.set_format(parameter);
    }

    pub(crate) fn reset_xterm_modifier_key_format(&mut self) {
        self.xterm_modifier_keys = self
            .xterm_modifier_keys
            .with_format(XtermKeyFormat::default());
    }

    pub(crate) fn report_xterm_modifier_key_format(&mut self) {
        self.push_event(TerminalEvent::PtyWrite(
            format!("\x1b[>6;{}f", self.xterm_modifier_keys.format().parameter()).into_bytes(),
        ));
    }

    /// Get xterm's terminal-local `modifySpecialKeys` and
    /// `formatSpecialKeys` policy.
    pub fn xterm_special_keys(&self) -> XtermSpecialKeys {
        self.xterm_special_keys
    }

    pub(crate) fn set_xterm_special_key_mode(&mut self, parameter: u16) {
        self.xterm_special_keys.set_mode(parameter);
    }

    pub(crate) fn reset_xterm_special_key_mode(&mut self) {
        self.xterm_special_keys = self
            .xterm_special_keys
            .with_mode(XtermSpecialKeys::default().mode());
    }

    pub(crate) fn report_xterm_special_key_mode(&mut self) {
        self.push_event(TerminalEvent::PtyWrite(
            format!("\x1b[>7;{}m", self.xterm_special_keys.mode().parameter()).into_bytes(),
        ));
    }

    pub(crate) fn set_xterm_special_key_format(&mut self, parameter: u16) {
        self.xterm_special_keys.set_format(parameter);
    }

    pub(crate) fn reset_xterm_special_key_format(&mut self) {
        self.xterm_special_keys = self
            .xterm_special_keys
            .with_format(XtermKeyFormat::default());
    }

    pub(crate) fn report_xterm_special_key_format(&mut self) {
        self.push_event(TerminalEvent::PtyWrite(
            format!("\x1b[>7;{}f", self.xterm_special_keys.format().parameter()).into_bytes(),
        ));
    }

    /// Get xterm's terminal-local `modifyOtherKeys` policy.
    pub fn xterm_modify_other_keys(&self) -> XtermModifyOtherKeys {
        self.xterm_modify_other_keys
    }

    pub(crate) fn set_xterm_modify_other_keys(&mut self, level: u16, modifier_mask: u16) {
        self.xterm_modify_other_keys.set(level, modifier_mask);
    }

    pub(crate) fn report_xterm_modify_other_keys(&mut self) {
        self.push_event(TerminalEvent::PtyWrite(
            format!("\x1b[>4;{}m", self.xterm_modify_other_keys.level()).into_bytes(),
        ));
    }

    pub(crate) fn set_xterm_other_key_format(&mut self, parameter: u16) {
        self.xterm_modify_other_keys.set_format(parameter);
    }

    pub(crate) fn report_xterm_other_key_format(&mut self) {
        self.push_event(TerminalEvent::PtyWrite(
            format!(
                "\x1b[>4;{}f",
                self.xterm_modify_other_keys.format().parameter()
            )
            .into_bytes(),
        ));
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
        self.shell.prompt_tracker.adjust_for_scroll(0, oldest);
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
            self.shell.prompt_tracker.adjust_for_scroll(0, 0);
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
    pub fn push_event(&mut self, event: TerminalEvent) {
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

    pub fn dec_conformance_level(&self) -> u8 {
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

    /// Resize the terminal
    pub fn resize(&mut self, cols: usize, rows: usize) {
        let prompt_navigation_row = self
            .prompt_navigation
            .filter(|navigation| navigation.scroll_offset == self.grid.scroll_offset())
            .map(|navigation| navigation.row);
        let mut shell_rows = self.shell.prompt_tracker.tracked_rows();
        shell_rows.extend(prompt_navigation_row);
        let shell_anchors = capture_shell_anchors(&self.grid, shell_rows);
        let same_columns = self.cols == cols;
        if !same_columns {
            self.navigation_cursor = None;
        }
        let mut primary_points = if !same_columns && !self.alt_screen_active {
            self.selection_reflow_points()
        } else {
            [None; 2]
        };
        let primary_resize = self
            .grid
            .resize_with_points(cols, rows, &mut primary_points);
        let primary_shift = primary_resize.archived_rows;
        let resize_blank = Cell {
            flags: CellFlags::DIRTY,
            ..Cell::default()
        };
        if !self.alt_screen_active {
            if same_columns {
                self.remap_selection_after_height_resize(primary_shift, rows);
            } else {
                self.restore_reflow_selection(primary_points);
            }
            self.repair_multicells_with_selection(&resize_blank);
        } else {
            self.grid.repair_multicells(&resize_blank);
        }
        let retained_shell_anchors = shell_anchors
            .into_iter()
            .filter_map(|(old_row, anchor)| {
                anchor
                    .retained_after_resize(primary_resize)
                    .map(|anchor| (old_row, anchor))
            })
            .collect();
        let remapped_shell_rows = restore_shell_anchors(&self.grid, retained_shell_anchors);
        self.shell.prompt_tracker.remap_rows(&remapped_shell_rows);
        self.prompt_navigation = prompt_navigation_row
            .and_then(|row| remapped_shell_rows.get(&row).copied())
            .filter(|row| {
                self.shell
                    .prompt_tracker
                    .all_prompts()
                    .any(|prompt| prompt == *row)
            })
            .map(|row| PromptNavigationState {
                row,
                scroll_offset: self.grid.scroll_offset(),
            });
        let mut alternate_points = if !same_columns && self.alt_screen_active {
            self.selection_reflow_points()
        } else {
            [None; 2]
        };
        let alternate_shift = self
            .alt_grid
            .resize_with_points(cols, rows, &mut alternate_points)
            .archived_rows;
        if self.alt_screen_active {
            if same_columns {
                self.remap_selection_after_height_resize(alternate_shift, rows);
            } else {
                self.restore_reflow_selection(alternate_points);
            }
            self.repair_multicells_with_selection(&resize_blank);
        } else {
            self.alt_grid.repair_multicells(&resize_blank);
        }
        if same_columns {
            self.graphics.adjust_for_height_resize(
                false,
                primary_shift,
                self.grid.scrollback_len(),
            );
            self.graphics.adjust_for_height_resize(
                true,
                alternate_shift,
                self.alt_grid.scrollback_len(),
            );
        } else {
            // Ordinary raster placements retain fixed coordinates across a
            // width change (unlike grid-owned Unicode placeholders). Reflow
            // can still shorten history, so retire now-unreachable roots.
            self.graphics
                .prune_scrollback(false, self.grid.scrollback_len());
            self.graphics
                .prune_scrollback(true, self.alt_grid.scrollback_len());
        }
        if let Some(snapshot) = &mut self.synchronized_output_snapshot {
            if snapshot.grid.cols() != cols || snapshot.grid.rows() != rows {
                snapshot.selection = None;
            }
            let same_snapshot_columns = snapshot.grid.cols() == cols;
            let archived = snapshot.grid.resize_with_row_shift(cols, rows);
            snapshot.graphics.resize(
                same_snapshot_columns,
                archived,
                snapshot.grid.scrollback_len(),
            );
            snapshot.grid.repair_multicells(&resize_blank);
        }
        self.resize_extra_cursors(cols, rows);

        // Preserve customized stops that still fit. Newly exposed columns get
        // the DEC default every eight columns; a shrink followed by a grow
        // therefore restores defaults only in the reintroduced region.
        let old_cols = self.tabs.len();
        self.tabs.resize(cols, false);
        if cols > old_cols {
            let first_default = old_cols.div_ceil(TAB_INTERVAL) * TAB_INTERVAL;
            for i in (first_default.max(TAB_INTERVAL)..cols).step_by(TAB_INTERVAL) {
                self.tabs[i] = true;
            }
        }

        // Update scroll region
        self.scroll_top = 0;
        self.scroll_bottom = rows;
        self.scroll_left = 0;
        self.scroll_right = cols;

        self.cols = cols;
        self.rows = rows;
    }

    /// Update the physical drawable extent represented by the current grid.
    /// Cell values are rounded up and never become zero.
    pub fn set_pixel_geometry(&mut self, pixel_width: u32, pixel_height: u32) {
        self.grid_pixel_width = pixel_width.max(1);
        self.grid_pixel_height = pixel_height.max(1);
        self.cell_pixel_width = self
            .grid_pixel_width
            .div_ceil(self.cols.max(1) as u32)
            .max(1);
        self.cell_pixel_height = self
            .grid_pixel_height
            .div_ceil(self.rows.max(1) as u32)
            .max(1);
        self.graphics
            .set_cell_pixel_geometry(self.cell_pixel_width, self.cell_pixel_height);
    }

    /// Emit one of the safe, terminal-local XTWINOPS geometry reports.
    /// Screen, outer-window, and mutation operations require app/window policy
    /// and are deliberately not handled here. Bounded application-label
    /// operations are implemented separately below.
    pub(crate) fn report_text_area_geometry(&mut self, operation: u16) {
        let response = match operation {
            14 => format!(
                "\x1b[4;{};{}t",
                self.grid_pixel_height, self.grid_pixel_width
            ),
            16 => format!(
                "\x1b[6;{};{}t",
                self.cell_pixel_height, self.cell_pixel_width
            ),
            18 => format!("\x1b[8;{};{}t", self.rows, self.cols),
            _ => return,
        };
        self.push_event(TerminalEvent::PtyWrite(response.into_bytes()));
    }

    /// Report application-authored labels for xterm XTWINOPS 20/21.
    pub(crate) fn report_application_title(&mut self, operation: u16) {
        let response = match operation {
            20 => format!("\x1b]L{}\x1b\\", self.application_titles.icon()),
            21 => format!("\x1b]l{}\x1b\\", self.application_titles.window()),
            _ => return,
        };
        self.push_event(TerminalEvent::PtyWrite(response.into_bytes()));
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

    /// Report xterm's title-stack position and fixed capacity (CSI # S).
    pub(crate) fn report_title_stack_position(&mut self) {
        let response = format!(
            "\x1b[{};{}#S",
            self.application_titles.used(),
            XTERM_TITLE_STACK_DEPTH
        );
        self.push_event(TerminalEvent::PtyWrite(response.into_bytes()));
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
                spacer
                    .flags
                    .insert(CellFlags::WIDE_CHAR_SPACER | CellFlags::DIRTY);
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
                spacer
                    .flags
                    .insert(CellFlags::WIDE_CHAR_SPACER | CellFlags::DIRTY);
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
    pub fn repeat_last_graphic(&mut self, count: usize) {
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
    pub fn linefeed(&mut self) {
        let cursor = self.grid().cursor();
        let col = if self.mode.contains(TerminalMode::LINEFEED_NEWLINE) {
            self.carriage_return_column(cursor.row, cursor.col)
        } else {
            cursor.col
        };

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
    pub fn horizontal_tab(&mut self) {
        self.cursor_forward_tabs(1);
    }

    /// Move forward across the requested number of horizontal tab stops.
    pub fn cursor_forward_tabs(&mut self, count: usize) {
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
    pub fn cursor_backward_tabs(&mut self, count: usize) {
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
    pub fn cursor_up(&mut self, n: usize) {
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
    pub fn cursor_down(&mut self, n: usize) {
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
    pub fn cursor_forward(&mut self, n: usize) {
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
    pub fn cursor_backward(&mut self, n: usize) {
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
    pub fn horizontal_position_relative(&mut self, n: usize) {
        if self.mode.contains(TerminalMode::ORIGIN) {
            self.cursor_forward(n);
            return;
        }
        let cursor = self.grid().cursor();
        let col = cursor.col.saturating_add(n).min(self.cols - 1);
        self.grid_mut().set_cursor(cursor.row, col);
    }

    /// VPR moves vertically without the CUD margin clamp.
    pub fn vertical_position_relative(&mut self, n: usize) {
        if self.mode.contains(TerminalMode::ORIGIN) {
            self.cursor_down(n);
            return;
        }
        let cursor = self.grid().cursor();
        let row = cursor.row.saturating_add(n).min(self.rows - 1);
        self.grid_mut().set_cursor(row, cursor.col);
    }

    /// CHA/HPA move only the column while respecting DECOM's origin.
    pub fn set_cursor_column(&mut self, col: usize) {
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
    pub fn set_cursor_row(&mut self, row: usize) {
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
    pub fn erase_in_display(&mut self, mode: u16) {
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
                    self.shell
                        .prompt_tracker
                        .adjust_for_scroll(scroll_amount, oldest);
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
                    self.shell.prompt_tracker.retire_content_in_rows(
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
    pub fn erase_in_line(&mut self, mode: u16) {
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
    pub fn selective_erase_in_display(&mut self, mode: u16) {
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
    pub fn selective_erase_in_line(&mut self, mode: u16) {
        let cursor = self.grid().cursor();
        let (start, end) = match mode {
            0 => (cursor.col, self.cols),
            1 => (0, cursor.col + 1),
            2 => (0, self.cols),
            _ => return,
        };
        self.erase_cells_in_region(cursor.row..cursor.row + 1, start..end, true);
    }

    /// Resolve VT's 1-indexed inclusive rectangle coordinates into an
    /// end-exclusive active-page rectangle. With DECOM set, coordinates are
    /// relative to and clipped by both margin pairs; otherwise margins do not
    /// affect rectangular operations.
    fn rectangular_area(
        &self,
        top: usize,
        left: usize,
        bottom: usize,
        right: usize,
    ) -> Option<TerminalRect> {
        let origin = self.mode.contains(TerminalMode::ORIGIN);
        let (row_offset, row_limit) = if origin {
            (self.scroll_top, self.scroll_bottom)
        } else {
            (0, self.rows)
        };
        let (col_offset, col_limit) = if origin {
            (self.scroll_left, self.scroll_right)
        } else {
            (0, self.cols)
        };
        if row_offset >= row_limit || col_offset >= col_limit {
            return None;
        }

        let top = row_offset.saturating_add(top.max(1) - 1).min(row_limit - 1);
        let left = col_offset
            .saturating_add(left.max(1) - 1)
            .min(col_limit - 1);
        let bottom = if bottom == 0 {
            row_limit - 1
        } else {
            row_offset.saturating_add(bottom - 1).min(row_limit - 1)
        };
        let right = if right == 0 {
            col_limit - 1
        } else {
            col_offset.saturating_add(right - 1).min(col_limit - 1)
        };
        (top <= bottom && left <= right).then_some(TerminalRect {
            top,
            bottom: bottom + 1,
            left,
            right: right + 1,
        })
    }

    /// Permit or deny DECRQCRA screen checksum replies. This host security
    /// policy is intentionally independent of DECSTR and RIS terminal state.
    pub fn set_screen_checksum_reporting(&mut self, allowed: bool) {
        self.screen_checksum_reporting = allowed;
    }

    pub fn screen_checksum_reporting(&self) -> bool {
        self.screen_checksum_reporting
    }

    /// Permit or deny Kitty's persistent ordinary-file transmission medium.
    /// Like other host security policy, this survives DECSTR and RIS resets.
    pub fn set_kitty_file_transmission(&mut self, allowed: bool) {
        self.graphics.set_file_transmission_allowed(allowed);
    }

    pub fn kitty_file_transmission(&self) -> bool {
        self.graphics.file_transmission_allowed()
    }

    /// DECRQCRA — report the VT420 checksum for a rectangular area. Termeric
    /// has one page per screen buffer, so the requested page is normalized to
    /// the active page. Invalid/reversed rectangles return the DEC zero result.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn request_checksum_rectangular_area(
        &mut self,
        request_id: u16,
        _page: usize,
        top: usize,
        left: usize,
        bottom: usize,
        right: usize,
    ) {
        if !self.screen_checksum_reporting {
            return;
        }
        let checksum = self
            .rectangular_area(top, left, bottom, right)
            .map_or(0, |area| self.checksum_area(area));
        self.push_event(TerminalEvent::PtyWrite(
            format!("\x1bP{request_id}!~{checksum:04X}\x1b\\").into_bytes(),
        ));
    }

    /// Match the default DEC/xterm algorithm: skip cells that have never held
    /// a drawn character, omit ordinary blanks after the first drawn cell,
    /// include the legacy presentation bits, subtract the 8-bit cell values,
    /// and wrap the result to 16 bits. Modern colors and combining scalars are
    /// deliberately outside the VT420 checksum.
    fn checksum_area(&self, area: TerminalRect) -> u16 {
        let mut total = 0_u16;
        let mut first_drawn = true;
        for row in area.top..area.bottom {
            for col in area.left..area.right {
                let Some(cell) = self.grid().cell(row, col) else {
                    continue;
                };
                let Some(character) = cell.content.char() else {
                    continue;
                };
                let attributes = dec_checksum_attributes(cell);
                let value = dec_checksum_character(cell, character).wrapping_add(attributes);
                if first_drawn || value != b' ' as u16 || attributes != 0 {
                    total = total.wrapping_add(value);
                }
                first_drawn = false;
            }
        }
        0_u16.wrapping_sub(total)
    }

    /// DECRQPSR — report cursor/rendition/character-set state or horizontal
    /// tab stops using the VT420 DECCIR/DECTABSR DCS formats.
    pub(crate) fn request_presentation_state(&mut self, report: u16) {
        let response = match report {
            1 => self.cursor_information_report(),
            2 => self.tab_stop_report(),
            _ => return,
        };
        self.push_event(TerminalEvent::PtyWrite(response));
    }

    fn cursor_information_report(&self) -> Vec<u8> {
        let cursor = self.grid().cursor();
        let mut rendition = b'@';
        rendition |= u8::from(self.current_attrs.contains(CellAttrs::INVERSE)) << 3;
        rendition |= u8::from(
            self.current_attrs
                .intersects(CellAttrs::BLINK | CellAttrs::RAPID_BLINK),
        ) << 2;
        rendition |= u8::from(self.current_attrs.has_underline()) << 1;
        rendition |= u8::from(self.current_attrs.contains(CellAttrs::BOLD));

        let protection = b'@' | u8::from(self.current_character_protected);
        let mut flags = b'@';
        flags |= u8::from(cursor.wrap_pending) << 3;
        flags |= u8::from(self.single_shift == Some(3)) << 2;
        flags |= u8::from(self.single_shift == Some(2)) << 1;
        flags |= u8::from(self.mode.contains(TerminalMode::ORIGIN));

        let designators: String = self
            .g_sets
            .iter()
            .map(|charset| char::from(charset.designator()))
            .collect();
        format!(
            "\x1bP1$u{};{};1;{};{};{};{};{};@;{}\x1b\\",
            cursor.row + 1,
            cursor.col + 1,
            char::from(rendition),
            char::from(protection),
            char::from(flags),
            self.gl_register,
            self.gr_register,
            designators,
        )
        .into_bytes()
    }

    fn tab_stop_report(&self) -> Vec<u8> {
        let mut response = b"\x1bP2$u".to_vec();
        for (column, _) in self.tabs.iter().enumerate().filter(|(_, set)| **set) {
            if response.len() > b"\x1bP2$u".len() {
                response.push(b'/');
            }
            response.extend_from_slice((column + 1).to_string().as_bytes());
        }
        response.extend_from_slice(b"\x1b\\");
        response
    }

    /// DECRSPS — restore a previously reported cursor or tab-stop state.
    /// Parsing is deliberately atomic: malformed or unsupported data leaves
    /// the complete live state unchanged.
    pub(crate) fn restore_presentation_state(&mut self, report: u8, data: &[u8]) {
        match report {
            1 => {
                let Some(state) = CursorPresentationState::parse(data, self.rows, self.cols) else {
                    return;
                };
                self.grid_mut().set_cursor(state.row, state.col);
                self.grid_mut().set_wrap_pending(state.wrap_pending);
                self.current_attrs.remove(
                    CellAttrs::BOLD
                        | CellAttrs::BLINK
                        | CellAttrs::RAPID_BLINK
                        | CellAttrs::INVERSE
                        | CellAttrs::ALL_UNDERLINES,
                );
                self.current_attrs.set(CellAttrs::BOLD, state.bold);
                self.current_attrs
                    .set(CellAttrs::UNDERLINE, state.underline);
                self.current_attrs.set(CellAttrs::BLINK, state.blink);
                self.current_attrs.set(CellAttrs::INVERSE, state.inverse);
                self.current_character_protected = state.protected;
                self.set_mode(TerminalMode::ORIGIN, state.origin_mode);
                self.g_sets = state.g_sets;
                self.gl_register = state.gl_register;
                self.gr_register = state.gr_register;
                self.single_shift = state.single_shift;
            }
            2 => {
                let Some(tabs) = parse_tab_stop_report(data, self.cols) else {
                    return;
                };
                self.tabs = tabs;
            }
            _ => {}
        }
    }

    /// DECCRA — copy an overlap-safe rectangular cell snapshot. Termeric has
    /// one page per screen buffer, so every source/destination page value is
    /// clamped to that sole page as DEC specifies for unavailable page numbers.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn copy_rectangular_area(
        &mut self,
        source_top: usize,
        source_left: usize,
        source_bottom: usize,
        source_right: usize,
        _source_page: usize,
        destination_top: usize,
        destination_left: usize,
        _destination_page: usize,
    ) {
        let Some(source) =
            self.rectangular_area(source_top, source_left, source_bottom, source_right)
        else {
            return;
        };
        let destination_top = destination_top.max(1);
        let destination_left = destination_left.max(1);
        let destination_bottom = destination_top.saturating_add(source.height() - 1);
        let destination_right = destination_left.saturating_add(source.width() - 1);
        let Some(destination) = self.rectangular_area(
            destination_top,
            destination_left,
            destination_bottom,
            destination_right,
        ) else {
            return;
        };
        let height = source.height().min(destination.height());
        let width = source.width().min(destination.width());
        if height == 0
            || width == 0
            || (source.top == destination.top
                && source.left == destination.left
                && height == source.height()
                && width == source.width())
        {
            return;
        }

        let snapshot: Vec<Vec<Cell>> = (0..height)
            .map(|offset| {
                self.grid()
                    .row(source.top + offset)
                    .map(|row| row.iter().skip(source.left).take(width).cloned().collect())
                    .unwrap_or_default()
            })
            .collect();
        let blank = self.erase_cell();
        {
            let (grid, mut damage) = self.grid_with_selection_damage();
            grid.erase_multicells_with_damage(
                destination.top..destination.top + height,
                destination.left..destination.left + width,
                &blank,
                false,
                &mut damage,
            );
            for (offset, cells) in snapshot.iter().enumerate() {
                let row_index = destination.top + offset;
                if let Some(row) = grid.row_mut(row_index) {
                    row.replace_range(destination.left, cells, &blank, |cols| {
                        damage(row_index..row_index + 1, cols)
                    });
                }
            }
            grid.repair_multicells_with_damage(&blank, damage);
        }
        self.graphics.copy_rect(
            self.alt_screen_active,
            source.top..source.top + height,
            source.left..source.left + width,
            destination.top,
            destination.left,
        );
    }

    /// DECFRA — fill a rectangle with a printable GL/GR character using the
    /// current SGR and DECSCA attributes.
    pub(crate) fn fill_rectangular_area(
        &mut self,
        character: u16,
        top: usize,
        left: usize,
        bottom: usize,
        right: usize,
    ) {
        let character = if character == 0 { 32 } else { character };
        if !((32..=126).contains(&character) || (160..=255).contains(&character)) {
            return;
        }
        let Some(area) = self.rectangular_area(top, left, bottom, right) else {
            return;
        };
        let mut character = char::from_u32(u32::from(character)).unwrap_or(' ');
        let charset = if character.is_ascii() {
            self.active_character_set()
        } else {
            CharacterSet::Ascii
        };
        character = charset.translate(character);
        let mut flags = CellFlags::DIRTY;
        flags.set(CellFlags::PROTECTED, self.current_character_protected);
        flags.set(
            CellFlags::DEC_SPECIAL_GRAPHICS,
            charset == CharacterSet::DecSpecialGraphics,
        );
        flags.set(
            CellFlags::DEC_UK_CHARSET,
            charset == CharacterSet::UnitedKingdom,
        );
        let fill = Cell {
            content: CellContent::Char(character),
            fg: self.current_fg,
            bg: self.current_bg,
            fg_spec: self.current_fg_spec,
            bg_spec: self.current_bg_spec,
            underline_color: self.current_underline_color,
            underline_color_spec: self.current_underline_color_spec,
            attrs: self.current_attrs,
            flags,
            hyperlink_id: None,
            multicell: Default::default(),
        };
        let blank = self.erase_cell();
        {
            let (grid, mut damage) = self.grid_with_selection_damage();
            grid.erase_multicells_with_damage(
                area.top..area.bottom,
                area.left..area.right,
                &blank,
                false,
                &mut damage,
            );
            for row_index in area.top..area.bottom {
                if let Some(row) = grid.row_mut(row_index) {
                    row.fill_range(area.left, area.right, &fill, &blank, |cols| {
                        damage(row_index..row_index + 1, cols)
                    });
                }
            }
        }
        self.graphics.erase_rect(
            self.alt_screen_active,
            area.top..area.bottom,
            area.left..area.right,
        );
    }

    /// DECERA — replace every cell in a rectangle with a BCE blank, ignoring
    /// DECSCA protection and preserving the active cursor position.
    pub(crate) fn erase_rectangular_area(
        &mut self,
        top: usize,
        left: usize,
        bottom: usize,
        right: usize,
    ) {
        let Some(area) = self.rectangular_area(top, left, bottom, right) else {
            return;
        };
        self.erase_cells_in_region(area.top..area.bottom, area.left..area.right, false);
        self.graphics.erase_rect(
            self.alt_screen_active,
            area.top..area.bottom,
            area.left..area.right,
        );
    }

    /// DECSERA — erase only unprotected grapheme content while retaining each
    /// affected cell's visual and protection attributes.
    pub(crate) fn selective_erase_rectangular_area(
        &mut self,
        top: usize,
        left: usize,
        bottom: usize,
        right: usize,
    ) {
        let Some(area) = self.rectangular_area(top, left, bottom, right) else {
            return;
        };
        self.erase_cells_in_region(area.top..area.bottom, area.left..area.right, true);
    }

    /// DECSACE — select whether DECCARA/DECRARA treat their endpoints as a
    /// wrapped stream or as an exact rectangle. Invalid values retain the
    /// existing setting.
    pub(crate) fn select_attribute_change_extent(&mut self, parameter: u16) {
        match parameter {
            0 | 1 => self.rectangular_change_extent = false,
            2 => self.rectangular_change_extent = true,
            _ => {}
        }
    }

    pub fn attribute_change_extent_parameter(&self) -> u8 {
        if self.rectangular_change_extent { 2 } else { 1 }
    }

    /// DECCARA — set or clear the DEC rendition subset in the selected area.
    /// Parameter order is significant, matching SGR-style last-change-wins
    /// behavior. SGR 8/28 are retained as the established xterm extension.
    pub(crate) fn change_attributes_area(
        &mut self,
        top: usize,
        left: usize,
        bottom: usize,
        right: usize,
        parameters: &[u16],
    ) {
        let mut change = DecAttributeChange::default();
        for parameter in parameters {
            match parameter {
                0 => change.clear(
                    CellAttrs::BOLD
                        | CellAttrs::ALL_UNDERLINES
                        | CellAttrs::BLINK
                        | CellAttrs::INVERSE,
                ),
                1 => change.set(CellAttrs::BOLD),
                4 => {
                    change.clear(CellAttrs::ALL_UNDERLINES);
                    change.set(CellAttrs::UNDERLINE);
                }
                5 => change.set(CellAttrs::BLINK),
                7 => change.set(CellAttrs::INVERSE),
                8 => change.set(CellAttrs::HIDDEN),
                22 => change.clear(CellAttrs::BOLD),
                24 => change.clear(CellAttrs::ALL_UNDERLINES),
                25 => change.clear(CellAttrs::BLINK),
                27 => change.clear(CellAttrs::INVERSE),
                28 => change.clear(CellAttrs::HIDDEN),
                _ => {}
            }
        }
        if change.is_empty() {
            return;
        }
        let Some(area) = self.rectangular_area(top, left, bottom, right) else {
            return;
        };
        self.apply_attribute_area(area, |attrs| change.apply(attrs));
    }

    /// DECRARA — reverse the requested DEC rendition subset. Repeated
    /// parameters cancel one another, and parameter 0 reverses the four DEC
    /// base attributes without implicitly toggling xterm's hidden extension.
    pub(crate) fn reverse_attributes_area(
        &mut self,
        top: usize,
        left: usize,
        bottom: usize,
        right: usize,
        parameters: &[u16],
    ) {
        let mut reverse = DecAttributeReverse::default();
        for parameter in parameters {
            match parameter {
                0 => {
                    reverse.bold ^= true;
                    reverse.underline ^= true;
                    reverse.blink ^= true;
                    reverse.inverse ^= true;
                }
                1 => reverse.bold ^= true,
                4 => reverse.underline ^= true,
                5 => reverse.blink ^= true,
                7 => reverse.inverse ^= true,
                8 => reverse.hidden ^= true,
                _ => {}
            }
        }
        if reverse.is_empty() {
            return;
        }
        let Some(area) = self.rectangular_area(top, left, bottom, right) else {
            return;
        };
        self.apply_attribute_area(area, |attrs| reverse.apply(attrs));
    }

    fn apply_attribute_area(&mut self, area: TerminalRect, mut apply: impl FnMut(&mut CellAttrs)) {
        let rectangular = self.rectangular_change_extent;
        let cols = self.cols;
        for row_index in area.top..area.bottom {
            let (start, end) = if rectangular || area.height() == 1 {
                (area.left, area.right)
            } else if row_index == area.top {
                (area.left, cols)
            } else if row_index + 1 == area.bottom {
                (0, area.right)
            } else {
                (0, cols)
            };
            let Some(row) = self.grid_mut().row_mut(row_index) else {
                continue;
            };
            row.change_attributes_range(start, end, rectangular, &mut apply);
        }
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
            flags: CellFlags::DIRTY,
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

    /// Return the setting body used by a successful DECRQSS response.
    pub(crate) fn decrqss_status(&self, request: &[u8]) -> Option<Vec<u8>> {
        match request {
            b"m" => Some(self.sgr_status()),
            b"r" => Some(format!("{};{}r", self.scroll_top + 1, self.scroll_bottom).into_bytes()),
            b"s" => Some(format!("{};{}s", self.scroll_left + 1, self.scroll_right).into_bytes()),
            b" q" => Some(format!("{} q", self.cursor_style_parameter()).into_bytes()),
            b"\"q" => Some(format!("{}\"q", self.character_protection_parameter()).into_bytes()),
            b"\"p" => Some(
                format!(
                    "{};{}\"p",
                    self.dec_conformance_level + 60,
                    u8::from(!self.use_8bit_controls)
                )
                .into_bytes(),
            ),
            b"*x" => Some(format!("{}*x", self.attribute_change_extent_parameter()).into_bytes()),
            // Xterm answers these legacy page/screen geometry settings even
            // when its window-operation policy denies the corresponding
            // mutation controls. Preserve its DECSLPP 24-line floor and
            // max-row convention; Termeric has no selectable 132-column mode,
            // so the report remains at that setting's 80-column default.
            b"t" => Some(format!("{}t", self.rows.saturating_sub(1).max(24)).into_bytes()),
            b"$|" => Some(b"80$|".to_vec()),
            b"*|" => Some(format!("{}*|", self.rows).into_bytes()),
            b">0m" => Some(format!(">0;{}m", self.xterm_keyboard_resource.modify()).into_bytes()),
            b">0f" => Some(format!(">0;{}f", self.xterm_keyboard_resource.format()).into_bytes()),
            b">1m" => {
                Some(format!(">1;{}m", self.xterm_cursor_keys.mode().parameter()).into_bytes())
            }
            b">1f" => {
                Some(format!(">1;{}f", self.xterm_cursor_keys.format().parameter()).into_bytes())
            }
            b">2m" => {
                Some(format!(">2;{}m", self.xterm_function_keys.mode().parameter()).into_bytes())
            }
            b">2f" => {
                Some(format!(">2;{}f", self.xterm_function_keys.format().parameter()).into_bytes())
            }
            b">3m" => {
                Some(format!(">3;{}m", self.xterm_keypad_keys.mode().parameter()).into_bytes())
            }
            b">3f" => {
                Some(format!(">3;{}f", self.xterm_keypad_keys.format().parameter()).into_bytes())
            }
            b">5m" => {
                Some(format!(">5;{}m", self.xterm_string_keys.mode().parameter()).into_bytes())
            }
            b">5f" => {
                Some(format!(">5;{}f", self.xterm_string_keys.format().parameter()).into_bytes())
            }
            b">6m" => {
                Some(format!(">6;{}m", self.xterm_modifier_keys.mode().parameter()).into_bytes())
            }
            b">6f" => {
                Some(format!(">6;{}f", self.xterm_modifier_keys.format().parameter()).into_bytes())
            }
            b">7m" => {
                Some(format!(">7;{}m", self.xterm_special_keys.mode().parameter()).into_bytes())
            }
            b">7f" => {
                Some(format!(">7;{}f", self.xterm_special_keys.format().parameter()).into_bytes())
            }
            b">4f" => Some(
                format!(">4;{}f", self.xterm_modify_other_keys.format().parameter()).into_bytes(),
            ),
            _ => None,
        }
    }

    fn sgr_status(&self) -> Vec<u8> {
        let mut params = Vec::new();
        let attrs = self.current_attrs;
        if attrs.contains(CellAttrs::BOLD) {
            params.push("1".to_string());
        }
        if attrs.contains(CellAttrs::DIM) {
            params.push("2".to_string());
        }
        if attrs.contains(CellAttrs::ITALIC) {
            params.push("3".to_string());
        }
        if attrs.contains(CellAttrs::CURLY_UNDERLINE) {
            params.push("4:3".to_string());
        } else if attrs.contains(CellAttrs::DOUBLE_UNDERLINE) {
            params.push("4:2".to_string());
        } else if attrs.contains(CellAttrs::DOTTED_UNDERLINE) {
            params.push("4:4".to_string());
        } else if attrs.contains(CellAttrs::DASHED_UNDERLINE) {
            params.push("4:5".to_string());
        } else if attrs.contains(CellAttrs::UNDERLINE) {
            params.push("4".to_string());
        }
        if attrs.contains(CellAttrs::BLINK) {
            params.push("5".to_string());
        }
        if attrs.contains(CellAttrs::RAPID_BLINK) {
            params.push("6".to_string());
        }
        if attrs.contains(CellAttrs::INVERSE) {
            params.push("7".to_string());
        }
        if attrs.contains(CellAttrs::HIDDEN) {
            params.push("8".to_string());
        }
        if attrs.contains(CellAttrs::STRIKE) {
            params.push("9".to_string());
        }
        Self::push_sgr_color(&mut params, self.current_fg_spec, true);
        Self::push_sgr_color(&mut params, self.current_bg_spec, false);
        if let Some(spec) = self.current_underline_color_spec {
            Self::push_sgr_extended_color(&mut params, spec, 58);
        }
        if params.is_empty() {
            params.push("0".to_string());
        }
        format!("{}m", params.join(";")).into_bytes()
    }

    fn push_sgr_color(params: &mut Vec<String>, spec: ColorSpec, foreground: bool) {
        match spec {
            ColorSpec::Default => {}
            ColorSpec::Indexed(index @ 0..=7) => {
                params.push((if foreground { 30 + index } else { 40 + index }).to_string());
            }
            ColorSpec::Indexed(index @ 8..=15) => {
                params.push(
                    (if foreground {
                        90 + index - 8
                    } else {
                        100 + index - 8
                    })
                    .to_string(),
                );
            }
            ColorSpec::Indexed(index) => {
                params.push(format!("{};5;{index}", if foreground { 38 } else { 48 }))
            }
            ColorSpec::TrueColor(color) => params.push(format!(
                "{};2;{};{};{}",
                if foreground { 38 } else { 48 },
                color.r,
                color.g,
                color.b
            )),
        }
    }

    fn push_sgr_extended_color(params: &mut Vec<String>, spec: ColorSpec, selector: u8) {
        match spec {
            ColorSpec::Default => {}
            ColorSpec::Indexed(index) => params.push(format!("{selector};5;{index}")),
            ColorSpec::TrueColor(color) => {
                params.push(format!("{selector};2;{};{};{}", color.r, color.g, color.b))
            }
        }
    }

    /// Set text attributes (SGR)
    pub fn set_attrs(&mut self, attrs: CellAttrs) {
        self.current_attrs = attrs;
    }

    /// Set foreground color with its original specification
    pub fn set_fg(&mut self, color: Color, spec: ColorSpec) {
        self.current_fg = color;
        self.current_fg_spec = spec;
    }

    /// Set background color with its original specification
    pub fn set_bg(&mut self, color: Color, spec: ColorSpec) {
        self.current_bg = color;
        self.current_bg_spec = spec;
    }

    /// Reset all attributes to default
    pub fn reset_attrs(&mut self) {
        self.current_attrs = CellAttrs::empty();
        self.current_fg = Color::default_fg();
        self.current_bg = Color::default_bg();
        self.current_fg_spec = ColorSpec::Default;
        self.current_bg_spec = ColorSpec::Default;
        self.current_underline_color = None;
        self.current_underline_color_spec = None;
    }

    /// Add text attributes to the current set
    pub fn add_attrs(&mut self, attrs: CellAttrs) {
        self.current_attrs.insert(attrs);
    }

    /// Remove text attributes from the current set
    pub fn remove_attrs(&mut self, attrs: CellAttrs) {
        self.current_attrs.remove(attrs);
    }

    /// Set underline color
    pub fn set_underline_color(&mut self, color: Color) {
        self.set_underline_color_spec(color, ColorSpec::TrueColor(color));
    }

    /// Set an underline color while retaining its original SGR encoding.
    pub fn set_underline_color_spec(&mut self, color: Color, spec: ColorSpec) {
        self.current_underline_color = Some(color);
        self.current_underline_color_spec = Some(spec);
    }

    /// Reset underline color (use fg color)
    pub fn reset_underline_color(&mut self) {
        self.current_underline_color = None;
        self.current_underline_color_spec = None;
    }

    /// Set underline style (clears all underline flags and sets the specified style)
    pub fn set_underline_style(&mut self, style: u16) {
        // Clear all underline-related flags first
        self.current_attrs.remove(CellAttrs::ALL_UNDERLINES);

        // Set the specified style
        match style {
            0 => {} // No underline
            1 => self.current_attrs.insert(CellAttrs::UNDERLINE),
            2 => self.current_attrs.insert(CellAttrs::DOUBLE_UNDERLINE),
            3 => self.current_attrs.insert(CellAttrs::CURLY_UNDERLINE),
            4 => self.current_attrs.insert(CellAttrs::DOTTED_UNDERLINE),
            5 => self.current_attrs.insert(CellAttrs::DASHED_UNDERLINE),
            _ => self.current_attrs.insert(CellAttrs::UNDERLINE), // Default to single
        }
    }

    /// Set the current hyperlink (OSC 8)
    ///
    /// All characters written after this will be part of this hyperlink
    /// until `reset_hyperlink` is called.
    pub fn set_hyperlink(&mut self, url: String, id: Option<String>) {
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
    pub fn reset_hyperlink(&mut self) {
        self.current_hyperlink_id = None;
    }

    /// Get the URL for a hyperlink ID
    pub fn get_hyperlink_url(&self, id: u32) -> Option<&str> {
        self.hyperlink_registry.get_url(id)
    }

    /// Resolve an OSC 8 target at a visual cell in the presented viewport,
    /// including scrolled history and a frozen synchronized-output snapshot.
    pub fn presentation_hyperlink_at(&self, row: usize, col: usize) -> Option<&str> {
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

    /// Permit one authenticated live-domain relay to restore decoded graphics
    /// state. The permission survives the replica's leading RIS, consumes
    /// itself on success or failure, and is never enabled for ordinary PTYs.
    pub fn set_graphics_relay_snapshot_allowed(&mut self, allowed: bool) {
        self.graphics_relay_import_allowed = allowed;
        if !allowed {
            self.graphics_relay_apc_data.clear();
            self.graphics_relay_apc_active = false;
            self.graphics_relay_apc_overflow = false;
            self.pending_graphics_relay_import = None;
        }
    }

    /// Capture the complete decoded graphics namespace without copying pixel
    /// buffers. Animation clocks are advanced to the atomic attachment point.
    pub fn graphics_relay_snapshot(&mut self, now: Instant) -> GraphicsRelaySnapshot {
        self.graphics
            .advance_animations(self.alt_screen_active, now);
        self.graphics.relay_snapshot(now)
    }

    pub(crate) fn begin_graphics_relay_apc(&mut self) {
        self.graphics_relay_apc_data.clear();
        self.graphics_relay_apc_active = self.graphics_relay_import_allowed;
        self.graphics_relay_apc_overflow = false;
    }

    pub(crate) fn push_graphics_relay_apc_byte(&mut self, byte: u8) {
        if !self.graphics_relay_apc_active || self.graphics_relay_apc_overflow {
            return;
        }
        if self.graphics_relay_apc_data.len() >= MAX_GRAPHICS_RELAY_APC_BYTES {
            self.graphics_relay_apc_data.clear();
            self.graphics_relay_apc_overflow = true;
        } else {
            self.graphics_relay_apc_data.push(byte);
        }
    }

    pub(crate) fn finish_graphics_relay_apc(&mut self) {
        if !self.graphics_relay_apc_active {
            return;
        }
        self.graphics_relay_apc_active = false;
        if self.graphics_relay_apc_overflow {
            self.graphics_relay_apc_overflow = false;
            self.fail_graphics_relay_import("graphics relay APC exceeds its resource limit");
            return;
        }
        let payload = std::mem::take(&mut self.graphics_relay_apc_data);
        if let Err(error) = self.handle_graphics_relay_apc(&payload) {
            self.fail_graphics_relay_import(&error);
        }
    }

    pub(crate) fn cancel_graphics_relay_apc(&mut self) {
        self.graphics_relay_apc_data.clear();
        self.graphics_relay_apc_active = false;
        self.graphics_relay_apc_overflow = false;
        if self.pending_graphics_relay_import.is_some() {
            self.fail_graphics_relay_import("graphics relay APC was interrupted");
        }
    }

    fn handle_graphics_relay_apc(&mut self, payload: &[u8]) -> Result<(), String> {
        let payload = std::str::from_utf8(payload)
            .map_err(|_| "graphics relay APC is not ASCII".to_string())?;
        let mut fields = payload.split(';');
        if fields.next() != Some("G") {
            return Err("graphics relay APC has an invalid namespace".into());
        }
        match fields.next() {
            Some("begin") => {
                let expected_bytes = fields
                    .next()
                    .and_then(|value| value.parse::<usize>().ok())
                    .ok_or_else(|| "graphics relay begin has an invalid length".to_string())?;
                let expected_checksum = fields
                    .next()
                    .and_then(|value| u64::from_str_radix(value, 16).ok())
                    .ok_or_else(|| "graphics relay begin has an invalid checksum".to_string())?;
                if fields.next().is_some() || expected_bytes > MAX_GRAPHICS_RELAY_METADATA_BYTES {
                    return Err("graphics relay begin exceeds its resource limit".into());
                }
                self.graphics.reset_for_relay_snapshot();
                self.pending_graphics_relay_import = Some(PendingGraphicsRelayImport {
                    expected_bytes,
                    expected_checksum,
                    metadata: Vec::new(),
                    started_at: Instant::now(),
                });
            }
            Some("chunk") => {
                let encoded = fields
                    .next()
                    .ok_or_else(|| "graphics relay chunk omits data".to_string())?;
                if fields.next().is_some() {
                    return Err("graphics relay chunk has trailing fields".into());
                }
                let decoded = STANDARD
                    .decode(encoded)
                    .map_err(|_| "graphics relay chunk is not valid base64".to_string())?;
                let pending = self
                    .pending_graphics_relay_import
                    .as_mut()
                    .ok_or_else(|| "graphics relay chunk arrived before begin".to_string())?;
                let new_len = pending
                    .metadata
                    .len()
                    .checked_add(decoded.len())
                    .ok_or_else(|| "graphics relay metadata length overflow".to_string())?;
                if new_len > pending.expected_bytes || new_len > MAX_GRAPHICS_RELAY_METADATA_BYTES {
                    return Err("graphics relay metadata exceeds its declared length".into());
                }
                pending.metadata.extend_from_slice(&decoded);
            }
            Some("end") => {
                if fields.next().is_some() {
                    return Err("graphics relay end has trailing fields".into());
                }
                let pending = self
                    .pending_graphics_relay_import
                    .take()
                    .ok_or_else(|| "graphics relay end arrived before begin".to_string())?;
                if pending.metadata.len() != pending.expected_bytes
                    || graphics_relay_checksum(&pending.metadata) != pending.expected_checksum
                {
                    return Err("graphics relay metadata is truncated or corrupt".into());
                }
                self.graphics
                    .apply_relay_metadata(&pending.metadata, pending.started_at)?;
                self.graphics
                    .advance_animations(self.alt_screen_active, Instant::now());
                log::debug!(
                    "Restored live-domain graphics snapshot: {} images, {} placements, {} decoded bytes",
                    self.graphics.image_count(),
                    self.graphics.placement_count(),
                    self.graphics.memory_usage()
                );
                self.graphics_relay_import_allowed = false;
            }
            _ => return Err("graphics relay APC has an invalid operation".into()),
        }
        Ok(())
    }

    fn fail_graphics_relay_import(&mut self, message: &str) {
        log::warn!("Rejected live-domain graphics snapshot: {message}");
        self.pending_graphics_relay_import = None;
        self.graphics_relay_import_allowed = false;
        self.graphics.reset_for_relay_snapshot();
    }

    pub(crate) fn begin_graphics_apc(&mut self) {
        self.graphics_apc_data.clear();
        self.graphics_apc_active = true;
        self.graphics_apc_overflow = false;
    }

    pub(crate) fn push_graphics_apc_byte(&mut self, byte: u8) {
        if !self.graphics_apc_active || self.graphics_apc_overflow {
            return;
        }
        if self.graphics_apc_data.len() >= MAX_KITTY_APC_BYTES {
            self.graphics_apc_overflow = true;
        } else {
            self.graphics_apc_data.push(byte);
        }
    }

    pub(crate) fn finish_graphics_apc(&mut self) {
        if !self.graphics_apc_active {
            return;
        }
        self.graphics_apc_active = false;
        if self.graphics_apc_overflow {
            self.graphics_apc_overflow = false;
            if KittyCommand::suppresses_response(&self.graphics_apc_data) {
                self.graphics_apc_data.clear();
                return;
            }
            self.graphics_apc_data.clear();
            let response = GraphicsResponse::too_large("graphics APC is too large");
            self.push_event(TerminalEvent::PtyWrite(
                response.to_escape_sequence().into_bytes(),
            ));
            return;
        }
        let payload = std::mem::take(&mut self.graphics_apc_data);
        self.handle_graphics_command(&payload);
    }

    pub(crate) fn cancel_graphics_apc(&mut self) {
        self.graphics_apc_data.clear();
        self.graphics_apc_active = false;
        self.graphics_apc_overflow = false;
    }

    /// Handle one complete Kitty graphics APC command.
    pub fn handle_graphics_command(&mut self, payload: &[u8]) {
        match KittyCommand::parse(payload) {
            Ok(mut cmd) => {
                // Record cursor position for placement
                let cursor = self.grid().cursor();
                cmd.placement.col = Some(cursor.col as u32);
                cmd.placement.row = Some(cursor.row as u32);
                let result = self.graphics.handle_command(cmd, self.alt_screen_active);
                if let Some((cols, rows)) = result.cursor_movement {
                    let terminal_rows = self.rows;
                    let terminal_cols = self.cols;
                    self.grid_mut().set_cursor(
                        cursor
                            .row
                            .saturating_add(rows as usize)
                            .min(terminal_rows - 1),
                        cursor
                            .col
                            .saturating_add(cols as usize)
                            .min(terminal_cols - 1),
                    );
                }
                if let Some(response) = result.response {
                    log::trace!("Graphics response: {:?}", response);
                    // Emit the response as Kitty graphics ack back to the PTY,
                    // so apps probing for image-protocol support (e.g. `icat`
                    // in Kitty-compatible mode) get the expected reply.
                    let bytes = response.to_escape_sequence().into_bytes();
                    self.push_event(crate::event::TerminalEvent::PtyWrite(bytes));
                }
            }
            Err(error) => {
                log::debug!("Rejected Kitty graphics command: {error}");
                self.graphics.abort_pending();
                if KittyCommand::suppresses_response(payload) {
                    return;
                }
                let response = GraphicsResponse::parse_error(&error.to_string());
                self.push_event(TerminalEvent::PtyWrite(
                    response.to_escape_sequence().into_bytes(),
                ));
            }
        }
    }

    pub(crate) fn handle_sixel_command(&mut self, payload: &[u8]) {
        let cursor = self.grid().cursor();
        match self.graphics.handle_sixel(
            payload,
            cursor.col as u32,
            cursor.row as i64,
            (self.cell_pixel_width, self.cell_pixel_height),
            self.alt_screen_active,
        ) {
            Ok((cols, rows)) => {
                log::debug!("Accepted Sixel image: cursor advance {cols}x{rows} cells");
                self.move_after_sixel_image(cursor, rows);
            }
            Err(error) => log::debug!("Rejected Sixel image: {}", error.message),
        }
    }

    pub(crate) fn handle_iterm2_image(&mut self, params: &[&[u8]]) {
        let cursor = self.grid().cursor();
        match self.graphics.handle_iterm2(
            params,
            (cursor.col as u32, cursor.row as i64),
            (self.cols as u32, self.rows as u32),
            (self.cell_pixel_width, self.cell_pixel_height),
            self.alt_screen_active,
        ) {
            Ok(Some((cols, rows))) => {
                log::debug!("Accepted iTerm2 inline image: cursor advance {cols}x{rows} cells");
                self.move_after_inline_image(cursor, cols, rows);
            }
            Ok(None) => {}
            Err(error) => log::debug!("Rejected iTerm2 inline image: {}", error.message),
        }
    }

    fn move_after_inline_image(&mut self, cursor: crate::grid::Cursor, cols: u32, rows: u32) {
        let row = cursor
            .row
            .saturating_add(rows as usize)
            .min(self.rows.saturating_sub(1));
        let col = if cols == 0 {
            0
        } else {
            cursor
                .col
                .saturating_add(cols as usize)
                .min(self.cols.saturating_sub(1))
        };
        self.grid_mut().set_cursor(row, col);
    }

    fn move_after_sixel_image(&mut self, cursor: crate::grid::Cursor, rows: u32) {
        let row = cursor
            .row
            .saturating_add(rows as usize)
            .min(self.rows.saturating_sub(1));
        let col = cursor.col.min(self.cols.saturating_sub(1));
        self.grid_mut().set_cursor(row, col);
    }

    /// Get all image placements for rendering
    pub fn graphics_placements(&self) -> impl Iterator<Item = &ImagePlacement> {
        self.graphics.placements(self.alt_screen_active)
    }

    /// Image placements and decoded data corresponding to the presentation
    /// grid. Snapshot ownership keeps deletions and ID reuse during a
    /// synchronized update from leaking into an intermediate frame.
    pub fn presentation_images(&self) -> Vec<PresentationImage> {
        if let Some(snapshot) = &self.synchronized_output_snapshot {
            snapshot.graphics.presentation_images(&snapshot.grid)
        } else {
            self.graphics.presentation_images(
                self.grid(),
                self.alt_screen_active,
                self.cell_pixel_width,
                self.cell_pixel_height,
            )
        }
    }

    /// Next exact frame boundary for a visible Kitty animation.
    pub fn graphics_animation_deadline(&self) -> Option<Instant> {
        self.graphics
            .next_animation_deadline(self.alt_screen_active)
    }

    /// Advance visible Kitty animations through every elapsed frame boundary.
    pub fn advance_graphics_animations(&mut self, now: Instant) -> bool {
        self.graphics
            .advance_animations(self.alt_screen_active, now)
    }

    /// Get image data for rendering
    pub fn get_image(&self, id: ImageId) -> Option<&StoredImage> {
        self.graphics.get_image(id)
    }

    /// Check if there are images to render
    pub fn has_images(&self) -> bool {
        self.graphics.has_images(self.alt_screen_active)
    }

    /// Decoded image bytes retained by this terminal's bounded image cache.
    pub fn graphics_memory_usage(&self) -> usize {
        self.graphics.memory_usage()
    }

    /// Decoded images retained by this terminal's bounded image cache.
    pub fn graphics_image_count(&self) -> usize {
        self.graphics.image_count()
    }

    /// Live image placements across both screen buffers.
    pub fn graphics_placement_count(&self) -> usize {
        self.graphics.placement_count()
    }

    // === Shell integration methods ===

    /// Handle OSC 133 shell integration sequence
    pub fn handle_osc_133(&mut self, params: &[&[u8]]) {
        if self.alt_screen_active {
            log::trace!("Ignoring OSC 133 from the alternate screen");
            return;
        }
        let cursor_row = self.grid().cursor().row;
        if params.first().is_some_and(|value| *value == b"A") {
            self.prompt_navigation = None;
        }
        self.shell.handle_osc_133(params, cursor_row);
    }

    /// Navigate to the previous retained OSC 133 prompt from the current
    /// prompt-navigation cursor or the top of a manually scrolled viewport.
    pub fn previous_prompt(&mut self) -> Option<i32> {
        if self.alt_screen_active {
            self.prompt_navigation = None;
            return None;
        }
        let from_row = self.prompt_navigation_origin(true);
        let row = self.shell.previous_prompt(from_row)?;
        self.reveal_prompt(row);
        Some(row)
    }

    /// Navigate to the next retained OSC 133 prompt from the current
    /// prompt-navigation cursor or the bottom of a manually scrolled viewport.
    pub fn next_prompt(&mut self) -> Option<i32> {
        if self.alt_screen_active {
            self.prompt_navigation = None;
            return None;
        }
        let from_row = self.prompt_navigation_origin(false);
        let row = self.shell.next_prompt(from_row)?;
        self.reveal_prompt(row);
        Some(row)
    }

    fn prompt_navigation_origin(&mut self, previous: bool) -> i32 {
        let offset = self.grid().scroll_offset();
        if let Some(navigation) = self.prompt_navigation
            && navigation.scroll_offset == offset
            && self
                .shell
                .prompt_tracker
                .all_prompts()
                .any(|row| row == navigation.row)
        {
            return navigation.row;
        }
        self.prompt_navigation = None;

        if offset == 0 {
            return i32::try_from(self.grid().cursor().row).unwrap_or(i32::MAX);
        }
        let offset = i32::try_from(offset).unwrap_or(i32::MAX);
        if previous {
            offset.saturating_neg()
        } else {
            i32::try_from(self.rows)
                .unwrap_or(i32::MAX)
                .saturating_sub(1)
                .saturating_sub(offset)
        }
    }

    fn reveal_prompt(&mut self, row: i32) {
        let offset = if row < 0 {
            usize::try_from(row.saturating_neg()).unwrap_or(usize::MAX)
        } else {
            0
        };
        self.grid_mut().set_scroll_offset(offset);
        self.prompt_navigation = Some(PromptNavigationState {
            row,
            scroll_offset: self.grid().scroll_offset(),
        });
    }

    fn shift_prompts_after_archival_scroll(&mut self, bottom: usize) {
        if self.alt_screen_active {
            return;
        }
        let oldest = -(self.grid().scrollback_len() as i32);
        let cursor_row = self.grid().cursor().row;
        self.shell
            .prompt_tracker
            .adjust_for_archival_region(bottom, oldest, cursor_row);
        if let Some(navigation) = &mut self.prompt_navigation
            && navigation.row < i32::try_from(bottom).unwrap_or(i32::MAX)
        {
            navigation.row = navigation.row.saturating_sub(1);
        }
        self.retain_prompt_navigation(oldest);
    }

    fn retain_prompt_navigation(&mut self, oldest_retained_row: i32) {
        let retained = self.prompt_navigation.is_some_and(|navigation| {
            navigation.row >= oldest_retained_row
                && self
                    .shell
                    .prompt_tracker
                    .all_prompts()
                    .any(|row| row == navigation.row)
        });
        if retained {
            let row = self.prompt_navigation.unwrap().row;
            if row < 0 && self.grid.scroll_offset() == 0 {
                let offset = usize::try_from(row.saturating_neg()).unwrap_or(usize::MAX);
                self.grid.set_scroll_offset(offset);
            }
            if let Some(navigation) = &mut self.prompt_navigation {
                navigation.scroll_offset = self.grid.scroll_offset();
            }
        } else {
            self.prompt_navigation = None;
        }
    }

    fn invalidate_prompt_navigation_after_manual_scroll(&mut self) {
        if self
            .prompt_navigation
            .is_some_and(|navigation| navigation.scroll_offset != self.grid.scroll_offset())
        {
            self.prompt_navigation = None;
        }
    }

    /// Get shell integration state
    pub fn shell_integration(&self) -> &ShellIntegration {
        &self.shell
    }

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
    pub fn set_tab_stop(&mut self) {
        let cursor = self.grid().cursor();
        if let Some(tab) = self.tabs.get_mut(cursor.col) {
            *tab = true;
        }
    }

    /// Clear the tab stop at the current column.
    pub fn clear_tab_stop(&mut self) {
        let cursor = self.grid().cursor();
        if let Some(tab) = self.tabs.get_mut(cursor.col) {
            *tab = false;
        }
    }

    /// Clear all tab stops
    pub fn clear_tab_stops(&mut self) {
        self.tabs.fill(false);
    }

    /// Restore DEC's default horizontal tab stops at columns 9, 17, ...
    pub fn reset_tab_stops(&mut self) {
        self.clear_tab_stops();
        for i in (TAB_INTERVAL..self.cols).step_by(TAB_INTERVAL) {
            self.tabs[i] = true;
        }
    }

    /// Set the scroll region (1-indexed, inclusive) for DECSTBM
    pub fn set_scroll_region(&mut self, top: usize, bottom: usize) {
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
    pub fn set_left_right_margins(&mut self, left: usize, right: usize) {
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
    pub fn reverse_index(&mut self) {
        let cursor = self.grid().cursor();
        if cursor.row == self.scroll_top && self.col_inside_horizontal_margins(cursor.col) {
            self.scroll_down(1);
        } else if cursor.row > 0 {
            self.grid_mut().set_cursor(cursor.row - 1, cursor.col);
        }
    }

    /// Insert blank characters at cursor position
    pub fn insert_chars(&mut self, count: usize) {
        self.shift_chars(count, true);
    }

    /// Delete characters at cursor position
    pub fn delete_chars(&mut self, count: usize) {
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
    pub fn erase_chars(&mut self, count: usize) {
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
    pub fn exit_alt_screen(&mut self) {
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
    pub fn save_cursor(&mut self) {
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
    pub fn restore_cursor(&mut self) {
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
    pub fn set_cursor_visible(&mut self, visible: bool) {
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
    pub fn soft_reset(&mut self) {
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
        self.xterm_cursor_keys = XtermCursorKeys::default();
        self.xterm_function_keys = XtermFunctionKeys::default();
        self.xterm_keypad_keys = XtermKeypadKeys::default();
        self.xterm_string_keys = XtermStringKeys::default();
        self.xterm_modifier_keys = XtermModifierKeys::default();
        self.xterm_special_keys = XtermSpecialKeys::default();
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
            self.shell.prompt_tracker.retire_content_in_rows(
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
        self.xterm_cursor_keys = XtermCursorKeys::default();
        self.xterm_function_keys = XtermFunctionKeys::default();
        self.xterm_keypad_keys = XtermKeypadKeys::default();
        self.xterm_string_keys = XtermStringKeys::default();
        self.xterm_modifier_keys = XtermModifierKeys::default();
        self.xterm_special_keys = XtermSpecialKeys::default();
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
            let mut end = remaining;
            while !line.is_char_boundary(end) {
                end = end.saturating_sub(1);
            }
            output.push_str(&line[..end]);
            truncated = true;
            break;
        }
        (output, truncated)
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
mod tests {
    use super::*;

    #[test]
    fn navigation_anchor_shares_exact_live_selection_moves_and_damage() {
        let commands: &[&[u8]] = &[
            b"\r\nnext",
            b"\x1b[2S",
            b"\x1b[2T",
            b"\x1b[2;1H\x1b[L",
            b"\x1b[2;1H\x1b[M",
            b"\x1b[1;3H\x1b[2@",
            b"\x1b[1;3H\x1b[2P",
            b"\x1b[Hxx",
            b"\x1b[H\x1b[2X",
            b"\x1b[2K",
            b"\x1b[2J",
            b"\x1b[?2J",
            b"\x1b[1;2;2;5$z",
            b"\x1b[65;1;2;2;5$x",
            b"\x1b[1;1;1;4;1;2;2;1$v",
            b"\x1b[?69h\x1b[2;6s\x1b[2;4r\x1b[S",
            b"\x1b[?69h\x1b[2;6s\x1b[2;4r\x1b[T",
            b"\x1b[H\x1b]66;s=2;X\x07",
        ];
        for command in commands {
            for row in -1..4 {
                for col in 0..8 {
                    let mut term = Terminal::new(8, 4);
                    term.process(
                        "history\r\nab界Zdef\r\n\x1b[1\"qprotected\x1b[0\"q\r\nlast".as_bytes(),
                    );
                    let point = Point::new(row, col);
                    if term.grid().row_at_offset(row).is_none() {
                        continue;
                    }
                    term.set_selection(Some(Selection::new(point, SelectionType::Simple)));
                    term.set_navigation_cursor(Some(point));
                    term.process(command);
                    assert_eq!(
                        term.navigation_cursor(),
                        term.selection().map(|selection| selection.start),
                        "command={command:?} point={point:?}"
                    );
                }
            }
        }
        let mut term = Terminal::new(8, 3);
        term.process(b"first\r\nsecond\r\nthird\r\nlast");
        term.set_navigation_cursor(Some(Point::new(-1, 0)));
        assert!(
            term.selection().is_none(),
            "navigation must never become a copy selection"
        );
        term.set_scrollback_limits(0, 0);
        assert!(term.navigation_cursor().is_none());
        for command in [b"\x1bc".as_slice(), b"\x1b[?1049h", b"\x1b[?1049l"] {
            term.set_navigation_cursor(Some(Point::new(0, 0)));
            term.process(command);
            assert!(term.navigation_cursor().is_none());
        }
    }

    fn assert_wide_invariants(term: &Terminal) {
        for row in 0..term.rows() {
            for col in 0..term.cols() {
                let cell = term.grid().cell(row, col).unwrap();
                if cell.is_wide_char() {
                    assert!(col + 1 < term.cols(), "wide leader at final column");
                    assert!(
                        term.grid()
                            .cell(row, col + 1)
                            .unwrap()
                            .is_wide_char_spacer(),
                        "wide leader at {row}:{col} has no spacer"
                    );
                }
                if cell.is_wide_char_spacer() {
                    assert!(col > 0, "wide spacer at column zero");
                    assert!(
                        term.grid().cell(row, col - 1).unwrap().is_wide_char(),
                        "wide spacer at {row}:{col} has no leader"
                    );
                    assert!(cell.content.is_empty());
                }
            }
        }
    }

    #[test]
    fn test_terminal_creation() {
        let term = Terminal::new(80, 24);
        assert_eq!(term.cols(), 80);
        assert_eq!(term.rows(), 24);
    }

    #[test]
    fn multicell_placement_selection_matches_actual_cell_damage() {
        let commands: &[&[u8]] = &[
            b"\x1b[2;3H\x1b]66;s=2:w=2;X\x07",
            "\x1b[2;3H\x1b]66;s=2;A界\x07".as_bytes(),
            b"\x1b[4h\x1b[2;3H\x1b]66;s=2:w=1;X\x07",
            b"\x1b[2;3H\x1b]66;s=2:w=2:n=1:d=2:v=1:h=2;X\x07",
            b"\x1b]66;s=0;invalid\x07",
            b"\x1b]66;s=7:w=7;oversized\x07",
            b"\x1b[H\x1b]66;s=1:w=1;a\x07", // Same scalar, new descriptor.
        ];
        let points = [
            Point::new(0, 0),
            Point::new(1, 2),
            Point::new(1, 8),
            Point::new(2, 1),
            Point::new(3, 9),
        ];
        for &command in commands {
            for ty in [
                SelectionType::Simple,
                SelectionType::Semantic,
                SelectionType::Lines,
                SelectionType::Block,
            ] {
                for start in points {
                    for end in points {
                        let mut term = Terminal::new(10, 4);
                        term.process(b"abcdefghij\r\nklmnopqrst\r\nuvwxyz0123\r\n456789ABCD");
                        let original: Vec<_> = (0..4)
                            .map(|row| term.grid().row(row).unwrap().clone())
                            .collect();
                        let mut selection = Selection::new(start, ty);
                        selection.update(end);
                        term.set_selection(Some(selection.clone()));
                        term.process(command);
                        let damaged = (0..4).any(|row| {
                            (0..10).any(|col| {
                                let before = original[row].get(col).unwrap();
                                let after = term.grid().cell(row, col).unwrap();
                                selection.contains(row as i32, col)
                                    && (before.content != after.content
                                        || before.multicell != after.multicell)
                            })
                        });
                        assert_eq!(
                            term.selection().is_none(),
                            damaged,
                            "command={command:?} ty={ty:?} start={start:?} end={end:?}"
                        );
                        if !damaged {
                            let actual = term.selection().unwrap();
                            assert_eq!(
                                (actual.start, actual.end, actual.ty, actual.active),
                                (
                                    selection.start,
                                    selection.end,
                                    selection.ty,
                                    selection.active
                                )
                            );
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn multicell_placement_repairs_wide_halves_and_invalidates_complete_collisions() {
        for target in [2, 3] {
            let mut term = Terminal::new(12, 4);
            term.process("..\x1b[1\"q界\x1b[0\"q".as_bytes());
            let selected = if target == 2 { 3 } else { 2 };
            term.set_selection(Some(Selection::new(
                Point::new(0, selected),
                SelectionType::Block,
            )));
            term.grid_mut().set_cursor(0, target);
            term.process(b"\x1b]66;s=1:w=1;X\x07");
            assert!(term.selection().is_none());
            let orphan = term.grid().cell(0, selected).unwrap();
            assert!(orphan.content.is_empty());
            assert!(!orphan.is_wide_char() && !orphan.is_wide_char_spacer());
            assert_eq!(term.grid().cell(0, target).unwrap().content.to_text(), "X");
            assert!(term.grid().multicell_region_at(0, target).is_some());
        }
        for target in [0, 2] {
            for selected in [Point::new(0, 0), Point::new(1, 3)] {
                let mut term = Terminal::new(12, 4);
                term.process(b"\x1b]66;s=2:w=2;old\x07");
                term.set_selection(Some(Selection::new(selected, SelectionType::Block)));
                term.grid_mut().set_cursor(0, target);
                term.process(b"\x1b]66;s=1:w=1;X\x07");
                assert!(term.selection().is_none());
                assert!(term.grid().multicell_region_at(1, 3).is_none());
                assert!(term.grid().multicell_region_at(0, target).is_some());
            }
        }
    }

    #[test]
    fn multicell_placement_skips_selected_lower_rows_but_insert_mode_erases_whole_blocks() {
        let mut skipped = Terminal::new(12, 4);
        skipped.process(b"\x1b]66;s=2:w=2;old\x07\x1b[2;1H");
        skipped.set_selection(Some(Selection::new(Point::new(0, 0), SelectionType::Block)));
        skipped.process(b"\x1b]66;s=1:w=1;X\x07");
        assert_eq!(skipped.get_selection_text().as_deref(), Some("old"));
        assert_eq!(skipped.grid().cell(1, 4).unwrap().content.to_text(), "X");

        let mut shifted = Terminal::new(12, 4);
        shifted.process(b"......\x1b]66;s=2:w=2;old\x07\x1b[2;1H\x1b[4h");
        shifted.set_selection(Some(Selection::new(Point::new(0, 6), SelectionType::Block)));
        shifted.process(b"\x1b]66;s=1:w=1;X\x07");
        assert!(shifted.selection().is_none()); // Old leader is outside the inserted row/span.
        assert!(shifted.grid().multicell_region_at(0, 6).is_none());
    }

    #[test]
    fn multicell_placement_wraps_selection_into_history_and_isolates_frozen_alternate_screen() {
        let mut term = Terminal::new(12, 3);
        term.process(b"anchor");
        let mut selection = Selection::new(Point::new(0, 5), SelectionType::Simple);
        selection.update(Point::new(0, 0));
        term.set_selection(Some(selection.clone()));
        term.process(b"\x1b[3;11H\x1b]66;s=2:w=2;X\x07");
        assert_eq!(term.selection().unwrap().start, Point::new(-2, 5));
        assert_eq!(term.get_selection_text().as_deref(), Some("anchor"));
        term.process(b"\x1b[?1049hanchor\x1b[H");
        term.set_selection(Some(selection));
        term.process(b"\x1b[?2026h\x1b]66;s=2:w=2;X\x07");
        assert_eq!(term.get_selection_text().as_deref(), Some("anchor"));
        term.process(b"\x1b[?2026l");
        assert!(term.selection().is_none());
    }

    #[test]
    fn vertical_commands_clear_selection_for_multicell_boundary_and_final_repair_damage() {
        let commands: &[&[u8]] = &[
            b"\x1b[S",
            b"\x1b[T",
            b"\x1b[L",
            b"\x1b[M",
            b"\x1b[6;1H\n",
            b"\x1bM",
        ];
        for &command in commands {
            for final_repair in [false, true] {
                let mut term = Terminal::new(12, 6);
                let selected = if final_repair {
                    // Only the lower half lies in the moved region. Neither
                    // early crossing-row scan sees the orphaned upper half.
                    term.process(b"\x1b[3;1H\x1b]66;s=2;X\x07\x1b[4;6r\x1b[4;1H");
                    Point::new(2, 0)
                } else {
                    term.process(b"\x1b]66;s=3;X\x07\x1b[2;6r\x1b[2;1H");
                    Point::new(0, 0)
                };
                term.set_selection(Some(Selection::new(selected, SelectionType::Block)));
                term.process(command);
                assert!(
                    term.selection().is_none(),
                    "command={command:?} final_repair={final_repair}"
                );
                assert!(
                    term.grid()
                        .cell(selected.row as usize, 0)
                        .unwrap()
                        .content
                        .is_empty()
                );
            }
        }
    }

    #[test]
    fn partial_width_vertical_edits_report_wide_damage_after_selection_motion() {
        let commands: &[&[u8]] = &[
            b"\x1b[S",
            b"\x1b[T",
            b"\x1b[L",
            b"\x1b[M",
            b"\x1b[5;4H\n",
            b"\x1bM",
        ];
        for &command in commands {
            for row in 0..5 {
                for leader in [2, 7] {
                    // Straddle either horizontal margin.
                    for selected_col in [leader, leader + 1] {
                        let mut term = Terminal::new(12, 5);
                        term.grid_mut().set_cursor(row, leader);
                        term.process("\x1b[1\"q界\x1b[0\"q".as_bytes());
                        term.process(b"\x1b[?69h\x1b[4;8s\x1b[1;4H");
                        term.set_selection(Some(Selection::new(
                            Point::new(row as i32, selected_col),
                            SelectionType::Block,
                        )));
                        term.process(command);
                        assert!(
                            term.selection().is_none(),
                            "command={command:?} row={row} col={selected_col}"
                        );
                        assert!((0..5).all(|r| (0..12).all(|c| {
                            let cell = term.grid().cell(r, c).unwrap();
                            !cell.is_wide_char() && !cell.is_wide_char_spacer()
                        })));
                    }
                }
            }
        }
    }

    #[test]
    fn vertical_glyph_damage_preserves_survivors_history_and_frozen_selection() {
        for down in [false, true] {
            let mut term = Terminal::new(12, 6);
            term.process(b"\x1b[3;4H\x1b]66;s=2;X\x07");
            term.set_selection(Some(Selection::new(Point::new(2, 3), SelectionType::Block)));
            if down {
                term.scroll_down(1);
            } else {
                term.scroll_up(1);
            }
            assert_eq!(
                term.selection().unwrap().start.row,
                if down { 3 } else { 1 }
            );
            assert_eq!(term.get_selection_text().as_deref(), Some("X"));

            let mut wide = Terminal::new(12, 5);
            wide.process("\x1b[3;5H界\x1b[?69h\x1b[4;8s\x1b[1;4H".as_bytes());
            wide.set_selection(Some(Selection::new(Point::new(2, 4), SelectionType::Block)));
            if down {
                wide.scroll_down(1);
            } else {
                wide.scroll_up(1);
            }
            assert_eq!(
                wide.selection().unwrap().start.row,
                if down { 3 } else { 1 }
            );
            assert_eq!(wide.get_selection_text().as_deref(), Some("界"));
        }

        let mut history = Terminal::new(12, 6);
        history.process(b"anchor\x1b[5;1H\x1b]66;s=2;X\x07");
        let mut selection = Selection::new(Point::new(0, 5), SelectionType::Simple);
        selection.update(Point::new(0, 0));
        history.set_selection(Some(selection));
        history.scroll_up(1);
        assert_eq!(history.selection().unwrap().start.row, -1);
        assert_eq!(history.get_selection_text().as_deref(), Some("anchor"));

        let mut frozen = Terminal::new(12, 5);
        frozen.process(b"\x1b[?1049h\x1b]66;s=3;X\x07\x1b[2;5r");
        frozen.set_selection(Some(Selection::new(Point::new(0, 0), SelectionType::Block)));
        frozen.process(b"\x1b[?2026h\x1b[S");
        assert_eq!(frozen.get_selection_text().as_deref(), Some("X"));
        frozen.process(b"\x1b[?2026l");
        assert!(frozen.selection().is_none());
    }

    #[test]
    fn partial_width_multicell_erasure_reports_outside_damage_but_zero_counts_do_nothing() {
        for operation in 0..4 {
            let mut term = Terminal::new(12, 5);
            term.process(b"\x1b]66;s=2:w=2;X\x07\x1b[?69h\x1b[3;8s\x1b[1;3H");
            term.set_selection(Some(Selection::new(Point::new(0, 0), SelectionType::Block)));
            for count in [0, 1] {
                match operation {
                    0 => term.scroll_up(count),
                    1 => term.scroll_down(count),
                    2 => term.insert_lines(count),
                    _ => term.delete_lines(count),
                }
                if count == 0 {
                    assert_eq!(term.get_selection_text().as_deref(), Some("X"));
                } else {
                    assert!(term.selection().is_none());
                    assert!(term.grid().cell(0, 0).unwrap().content.is_empty());
                }
            }
        }
    }

    #[test]
    fn vertical_boundary_erasure_retires_selection_outside_the_scrolled_rows() {
        let mut term = Terminal::new(12, 5);
        term.process(b"\x1b]66;s=3:w=1;X\x07\x1b[2;5r");
        term.set_selection(Some(Selection::new(Point::new(0, 0), SelectionType::Block)));
        term.process(b"\x1b[S");
        assert!(term.grid().cell(0, 0).unwrap().content.is_empty());
        assert!(term.selection().is_none());
    }

    #[test]
    fn new_multicell_placement_retires_overwritten_selection() {
        let mut term = Terminal::new(12, 4);
        term.process(b"anchor\x1b[H");
        let mut selection = Selection::new(Point::new(0, 0), SelectionType::Simple);
        selection.update(Point::new(0, 5));
        term.set_selection(Some(selection));
        term.process(b"\x1b]66;s=2:w=2;X\x07");
        assert!(term.selection().is_none());
    }

    #[test]
    fn osc66_places_atomic_scaled_blocks_and_advances_by_their_width() {
        let mut term = Terminal::new(12, 5);
        term.process(b"\x1b]66;s=2:w=3:n=1:d=2:v=1:h=2;hello\x07");

        let region = term
            .grid()
            .multicell_region_at(1, 5)
            .expect("covered cell resolves to its block");
        assert_eq!((region.top, region.left), (0, 0));
        assert_eq!(region.descriptor.columns(), 6);
        assert_eq!(region.descriptor.rows(), 2);
        assert_eq!(region.descriptor.font_scale(), 1.0);
        assert_eq!(
            region.descriptor.vertical_alignment(),
            crate::MultiCellAlignment::End
        );
        assert_eq!(
            region.descriptor.horizontal_alignment(),
            crate::MultiCellAlignment::Center
        );
        assert_eq!(term.grid().cell(0, 0).unwrap().content.to_text(), "hello");
        assert!(term.grid().cell(0, 1).unwrap().content.is_empty());
        assert_eq!(term.grid().cursor().col, 6);
    }

    #[test]
    fn osc66_natural_width_segments_graphemes_into_separate_blocks() {
        let mut term = Terminal::new(12, 5);
        assert!(Osc66Command::parse(&[b"s=2", "A界".as_bytes()]).is_some());
        term.process("\x1b]66;s=2;A界\x07".as_bytes());

        let ascii = term.grid().multicell_region_at(0, 1).unwrap();
        let wide = term.grid().multicell_region_at(1, 5).unwrap();
        assert_eq!((ascii.left, ascii.descriptor.columns()), (0, 2));
        assert_eq!((wide.left, wide.descriptor.columns()), (2, 4));
        assert_eq!(term.grid().cell(0, 2).unwrap().content.to_text(), "界");
        assert_eq!(term.grid().cursor().col, 6);
    }

    #[test]
    fn normal_text_obeys_multicell_overwrite_and_lower_row_skip_rules() {
        let mut term = Terminal::new(10, 4);
        term.process(b"\x1b]66;s=2:w=2;A\x07");
        term.grid_mut().set_cursor(1, 0);
        term.write_char('x');
        assert!(term.grid().multicell_region_at(0, 0).is_some());
        assert_eq!(term.grid().cell(1, 4).unwrap().content.to_text(), "x");

        term.grid_mut().set_cursor(0, 1);
        term.write_char('y');
        assert!(term.grid().multicell_region_at(0, 0).is_none());
        assert_eq!(term.grid().cell(0, 0).unwrap().content.to_text(), " ");
        assert_eq!(term.grid().cell(0, 1).unwrap().content.to_text(), "y");
        assert_eq!(term.grid().cell(1, 0).unwrap().content.to_text(), " ");
    }

    #[test]
    fn combining_scalar_extends_multicell_leader_without_splitting_block() {
        let mut term = Terminal::new(10, 4);
        term.process(b"\x1b]66;s=2;A\x07");
        term.process("\u{301}".as_bytes());

        assert_eq!(
            term.grid().cell(0, 0).unwrap().content.to_text(),
            "A\u{301}"
        );
        assert!(term.grid().multicell_region_at(1, 1).is_some());
        assert_eq!(term.grid().cursor().col, 2);
    }

    #[test]
    fn osc66_discards_blocks_larger_than_the_screen() {
        let mut term = Terminal::new(4, 2);
        term.process(b"\x1b]66;s=3:w=2;x\x07");
        assert!(term.grid().cell(0, 0).unwrap().content.is_empty());
        assert_eq!(term.grid().cursor().col, 0);
    }

    #[test]
    fn osc66_wrap_and_no_wrap_respect_horizontal_margins() {
        let mut wrapped = Terminal::new(12, 4);
        wrapped.set_left_right_margin_mode(true);
        wrapped.set_left_right_margins(2, 10);
        wrapped.grid_mut().set_cursor(0, 8);
        wrapped.process(b"\x1b]66;w=3;x\x07");
        let region = wrapped.grid().multicell_region_at(1, 1).unwrap();
        assert_eq!((region.top, region.left), (1, 1));

        let mut no_wrap = Terminal::new(12, 4);
        no_wrap.set_left_right_margin_mode(true);
        no_wrap.set_left_right_margins(2, 10);
        no_wrap.process(b"\x1b[?7l");
        no_wrap.grid_mut().set_cursor(0, 8);
        no_wrap.process(b"\x1b]66;w=3;x\x07");
        let region = no_wrap.grid().multicell_region_at(0, 8).unwrap();
        assert_eq!((region.top, region.left), (0, 7));
    }

    #[test]
    fn osc66_horizontal_edits_preserve_whole_single_row_blocks_and_erase_multiline_blocks() {
        let mut one_row = Terminal::new(10, 4);
        one_row.process(b"..\x1b]66;s=1:w=2;X\x07");
        one_row.grid_mut().set_cursor(0, 0);
        one_row.insert_chars(1);
        let shifted = one_row.grid().multicell_region_at(0, 4).unwrap();
        assert_eq!((shifted.top, shifted.left), (0, 3));
        one_row.grid_mut().set_cursor(0, 4);
        one_row.erase_chars(1);
        assert!(one_row.grid().multicell_region_at(0, 3).is_none());

        let mut multiline = Terminal::new(10, 4);
        multiline.process(b"..\x1b]66;s=2;X\x07");
        multiline.grid_mut().set_cursor(1, 0);
        multiline.delete_chars(1);
        assert!(multiline.grid().multicell_region_at(0, 2).is_none());
    }

    #[test]
    fn osc66_resize_retains_fitting_blocks_and_discards_clipped_blocks() {
        let mut term = Terminal::new(8, 4);
        term.process(b".\x1b]66;s=2:w=2;wide\x07");
        term.resize(10, 5);
        assert!(term.grid().multicell_region_at(1, 4).is_some());

        term.resize(3, 5);
        assert!(
            term.grid()
                .iter_cells()
                .all(|(_, _, cell)| !cell.multicell.is_present())
        );
    }

    #[test]
    fn osc66_ansi_export_replays_descriptor_and_text_once() {
        let mut source = Terminal::new(10, 3);
        source.process(b"\x1b]66;s=1:w=3:n=1:d=2:h=1;abc\x07!");
        let line = source.grid().export_scrollback_text().remove(0);
        assert!(line.contains("\x1b]66;s=1:w=3:n=1:d=2:h=1;abc\x07"));
        assert_eq!(line.matches("abc").count(), 1);

        let mut restored = Terminal::new(10, 3);
        restored.process(line.as_bytes());
        let region = restored.grid().multicell_region_at(0, 2).unwrap();
        assert_eq!((region.left, region.descriptor.columns()), (0, 3));
        assert_eq!(restored.grid().cell(0, 3).unwrap().content.to_text(), "!");
    }

    #[test]
    fn osc66_selection_emits_leader_text_once_without_covered_spaces() {
        let mut term = Terminal::new(10, 3);
        term.process(b"\x1b]66;s=2:w=2;title\x07!");
        let mut selection = Selection::new(Point::new(0, 1), SelectionType::Simple);
        selection.update(Point::new(1, 4));
        term.set_selection(Some(selection));
        assert_eq!(term.get_selection_text().as_deref(), Some("title!"));
    }

    #[test]
    fn osc66_scroll_moves_complete_blocks_and_clears_only_split_multiline_blocks() {
        let mut term = Terminal::new(8, 4);
        term.grid_mut().set_cursor(1, 1);
        term.process(b"\x1b]66;s=2:w=2;move\x07");
        term.scroll_up(1);
        assert!(term.grid().multicell_region_at(1, 4).is_some());
        assert_eq!(term.grid().cell(0, 1).unwrap().content.to_text(), "move");

        term.scroll_up(1);
        assert!(
            term.grid()
                .iter_cells()
                .all(|(_, _, cell)| !cell.multicell.is_present())
        );

        let mut history = Terminal::new(8, 2);
        history.process(b"\x1b]66;w=2;kept\x07");
        history.scroll_up(1);
        let archived = history.grid().row_at_offset(-1).unwrap();
        assert!(archived.get(0).unwrap().multicell.is_leader());
        assert_eq!(archived.get(0).unwrap().content.to_text(), "kept");
    }

    #[test]
    fn osc66_ed22_archives_complete_blocks_before_erasing_the_viewport() {
        let mut term = Terminal::new(8, 3);
        term.process(b"\x1b]66;s=2:w=2;kept\x07!");
        term.erase_in_display(22);

        assert_eq!(term.grid().scrollback_len(), 3);
        assert!(
            term.grid()
                .iter_cells()
                .all(|(_, _, cell)| !cell.multicell.is_present())
        );
        let top = term.grid().row_at_offset(-3).unwrap();
        let lower = term.grid().row_at_offset(-2).unwrap();
        assert!(top.get(0).unwrap().multicell.is_leader());
        assert_eq!(top.get(0).unwrap().content.to_text(), "kept");
        assert!(lower.get(3).unwrap().multicell.is_present());
        assert_eq!(top.get(4).unwrap().content.to_text(), "!");
    }

    #[test]
    fn osc66_screen_switches_and_synchronized_output_keep_blocks_isolated() {
        let mut screens = Terminal::new(8, 3);
        screens.process(b"\x1b]66;s=2:w=2;primary\x07");
        screens.enter_alt_screen();
        assert!(
            screens
                .grid()
                .iter_cells()
                .all(|(_, _, cell)| !cell.multicell.is_present())
        );
        screens.process(b"\x1b]66;w=2;alternate\x07");
        assert!(screens.grid().multicell_region_at(0, 1).is_some());
        screens.exit_alt_screen();
        assert!(screens.grid().multicell_region_at(1, 3).is_some());
        assert_eq!(
            screens.grid().cell(0, 0).unwrap().content.to_text(),
            "primary"
        );

        let mut synchronized = Terminal::new(8, 3);
        synchronized.process(b"\x1b[?2026h\x1b]66;s=2:w=2;pending\x07");
        assert!(
            synchronized
                .presentation_grid()
                .iter_cells()
                .all(|(_, _, cell)| !cell.multicell.is_present())
        );
        synchronized.process(b"\x1b[?2026l");
        assert!(
            synchronized
                .presentation_grid()
                .multicell_region_at(1, 3)
                .is_some()
        );
    }

    #[test]
    fn osc66_selective_erase_preserves_protected_blocks_atomically() {
        let mut term = Terminal::new(8, 3);
        term.process(b"\x1b[1\"q\x1b]66;s=2:w=2;safe\x07");
        term.grid_mut().set_cursor(0, 1);
        term.selective_erase_in_line(0);
        assert!(term.grid().multicell_region_at(1, 3).is_some());

        term.process(b"\x1b[0\"q");
        term.erase_in_line(2);
        assert!(term.grid().multicell_region_at(0, 0).is_none());
    }

    #[test]
    fn fragmented_utf8_osc66_remains_streaming_safe() {
        let mut term = Terminal::new(10, 4);
        for byte in "\x1b]66;s=2:w=2;界\x1b\\".as_bytes() {
            term.process(std::slice::from_ref(byte));
        }
        assert_eq!(term.grid().cell(0, 0).unwrap().content.to_text(), "界");
        assert!(term.grid().multicell_region_at(1, 3).is_some());
    }

    #[test]
    fn bounded_row_extraction_never_splits_utf8_or_allocates_the_unread_suffix() {
        let mut term = Terminal::new(10, 3);
        term.process("界abc\r\nsecond".as_bytes());

        let (text, truncated) = term.bounded_rows_text(0, 1, 5);
        assert_eq!(text, "界ab");
        assert!(truncated);
        assert!(text.len() <= 5);

        let (text, truncated) = term.bounded_rows_text(0, 1, 64);
        assert_eq!(text, "界abc\nsecond");
        assert!(!truncated);

        let (text, truncated) = term.bounded_rows_text(-100, 1, 64);
        assert_eq!(text, "界abc\nsecond");
        assert!(truncated);
    }

    #[test]
    fn test_write_char() {
        let mut term = Terminal::new(80, 24);
        term.write_char('A');
        let cursor = term.grid().cursor();
        assert_eq!(cursor.col, 1);

        let cell = term.grid().cell(0, 0).unwrap();
        assert_eq!(cell.content.char(), Some('A'));
    }

    #[test]
    fn unicode_clusters_use_terminal_width_and_one_leader() {
        for (text, width) in [
            ("界", 2),
            ("e\u{301}", 1),
            ("·", 1),
            ("☺\u{fe0e}", 1),
            ("☺\u{fe0f}", 2),
            ("🇺🇸", 2),
            ("👍🏽", 2),
            ("👩\u{200d}💻", 2),
        ] {
            let mut term = Terminal::new(20, 3);
            term.process(text.as_bytes());
            assert_eq!(term.grid().cursor().col, width, "{text:?}");
            assert_eq!(term.grid().cell(0, 0).unwrap().content.to_text(), text);
            assert_eq!(term.grid().cell(0, 0).unwrap().width(), width);
            assert_wide_invariants(&term);
        }
    }

    #[test]
    fn orphan_combining_mark_composes_over_a_blank_cell() {
        let mut term = Terminal::new(4, 2);
        term.process("\u{301}".as_bytes());
        assert_eq!(term.grid().cursor().col, 1);
        assert_eq!(
            term.grid().cell(0, 0).unwrap().content.to_text(),
            " \u{301}"
        );
    }

    #[test]
    fn wide_clusters_wrap_before_the_final_column() {
        let mut term = Terminal::new(4, 3);
        term.process("abc界".as_bytes());
        assert!(term.grid().row(0).unwrap().is_wrapped());
        assert_eq!(term.grid().cell(1, 0).unwrap().content.to_text(), "界");
        assert_eq!(term.grid().cursor().col, 2);
        assert_wide_invariants(&term);

        let mut presentation_change = Terminal::new(4, 3);
        presentation_change.process("abc☺\u{fe0f}".as_bytes());
        assert!(presentation_change.grid().row(0).unwrap().is_wrapped());
        assert_eq!(
            presentation_change
                .grid()
                .cell(1, 0)
                .unwrap()
                .content
                .to_text(),
            "☺\u{fe0f}"
        );
        assert_wide_invariants(&presentation_change);
    }

    #[test]
    fn cell_edits_and_reflow_never_orphan_wide_pairs() {
        let mut term = Terminal::new(12, 4);
        term.process("A界B👍🏽C👩\u{200d}💻D".as_bytes());
        assert_wide_invariants(&term);

        for col in 0..12 {
            term.grid_mut().set_cursor(0, col);
            term.insert_chars(1);
            assert_wide_invariants(&term);
            term.delete_chars(1);
            assert_wide_invariants(&term);
        }

        for cols in [7, 3, 1, 16, 5, 12] {
            term.resize(cols, 4);
            assert_wide_invariants(&term);
        }

        let mut one_column = Terminal::new(4, 4);
        one_column.process("界".as_bytes());
        one_column.resize(1, 4);
        assert_eq!(one_column.rows_text(0, 3), "界\n\n\n");
        one_column.resize(4, 4);
        assert!(one_column.rows_text(0, 3).contains('界'));
        assert_wide_invariants(&one_column);
    }

    #[test]
    fn shell_row_bookmark_tracks_wide_reflow_padding() {
        let mut term = Terminal::new(5, 4);
        term.process("ab界ZQ".as_bytes());
        let anchor = shell_logical_anchor(term.grid(), 1).unwrap();
        term.resize(3, 4);
        assert_eq!(shell_row_from_anchor(term.grid(), anchor), Some(2));
        assert_eq!(term.grid().row(2).unwrap().to_string(), "Q");
    }

    #[test]
    fn stream_selection_joins_soft_wraps_and_skips_synthetic_wide_padding() {
        for ty in [SelectionType::Simple, SelectionType::Semantic] {
            for reversed in [false, true] {
                for history in [false, true] {
                    let mut term = Terminal::new(3, 2);
                    term.process("ab界Z".as_bytes());
                    let row = if history {
                        term.process(b"\r\nx");
                        -1
                    } else {
                        0
                    };
                    let (start, end) = (Point::new(row, 0), Point::new(row + 1, 2));
                    let (start, end) = if reversed { (end, start) } else { (start, end) };
                    let mut selection = Selection::new(start, ty);
                    selection.update(end);
                    term.set_selection(Some(selection));
                    assert_eq!(
                        term.get_selection_text().as_deref(),
                        Some("ab界Z"),
                        "type={ty:?} reversed={reversed} history={history}"
                    );
                }
            }
        }
    }

    #[test]
    fn stream_selection_preserves_hard_breaks_and_partial_bounds() {
        let mut term = Terminal::new(3, 4);
        term.process(b"abcdef\r\nghi");
        for (start, end, expected) in [
            (Point::new(0, 0), Point::new(2, 2), "abcdef\nghi"),
            (Point::new(0, 1), Point::new(1, 1), "bcde"),
            (Point::new(1, 1), Point::new(2, 1), "ef\ngh"),
        ] {
            let mut selection = Selection::new(start, SelectionType::Simple);
            selection.update(end);
            term.set_selection(Some(selection));
            assert_eq!(term.get_selection_text().as_deref(), Some(expected));
        }
    }

    #[test]
    fn explicit_presentation_selection_preserves_unicode_and_physical_row_semantics() {
        let mut term = Terminal::new(3, 4);
        term.process("ab界Z\x1b[?2026h\x1b[?1049hsecret".as_bytes());
        for (ty, expected) in [
            (SelectionType::Simple, "ab界Z"),
            (SelectionType::Semantic, "ab界Z"),
            (SelectionType::Lines, "ab \n界Z"),
            (SelectionType::Block, "ab \n界Z"),
        ] {
            for reverse in [false, true] {
                let (start, end) = (Point::new(0, 0), Point::new(1, 2));
                let mut selection = Selection::new(if reverse { end } else { start }, ty);
                selection.update(if reverse { start } else { end });
                assert_eq!(term.presentation_selection_text(&selection), expected);
                assert!(term.selection().is_none());
            }
        }
    }

    #[test]
    fn native_selection_reads_frozen_text_and_word_boundaries() {
        let mut term = Terminal::new(16, 3);
        term.process(b"visible word");
        let mut selection = Selection::new(Point::new(0, 0), SelectionType::Simple);
        selection.update(Point::new(0, 6));
        term.set_selection(Some(selection));
        term.process(b"\x1b[?2026h\x1b[H\x1b[2Khidden text");
        assert_eq!(term.get_selection_text().as_deref(), Some("visible"));
        assert_eq!(
            term.word_at(Point::new(0, 6)),
            (Point::new(0, 0), Point::new(0, 6))
        );
    }

    #[test]
    fn projected_selection_skips_rows_and_joins_only_adjacent_soft_wraps() {
        let mut term = Terminal::new(3, 4);
        term.process("ab界Zqqq".as_bytes());
        for ty in [
            SelectionType::Simple,
            SelectionType::Semantic,
            SelectionType::Lines,
            SelectionType::Block,
        ] {
            for reversed in [false, true] {
                let (start, end) = (Point::new(0, 0), Point::new(2, 2));
                let mut selection = Selection::new(if reversed { end } else { start }, ty);
                selection.update(if reversed { start } else { end });
                let logical = matches!(ty, SelectionType::Simple | SelectionType::Semantic);
                assert_eq!(
                    term.presentation_selection_text_in_rows(&selection, &[0, 2]),
                    if logical { "ab\nqqq" } else { "ab \nqqq" }
                );
                assert_eq!(
                    term.presentation_selection_text_in_rows(&selection, &[0, 1, 2]),
                    if logical {
                        "ab界Zqqq"
                    } else {
                        "ab \n界Z\nqqq"
                    }
                );
                assert_eq!(
                    term.presentation_selection_text_in_rows(&selection, &[]),
                    ""
                );
            }
        }
    }

    #[test]
    fn snapshot_selection_clips_scrollback_and_preserves_block_columns() {
        for ty in [
            SelectionType::Simple,
            SelectionType::Lines,
            SelectionType::Block,
        ] {
            for reversed in [false, true] {
                let mut term = Terminal::new(3, 3);
                term.process(b"abc\r\ndef\r\nghi\r\njkl\r\nmno");
                term.grid_mut().set_scroll_offset(1);
                let (start, end) = (Point::new(-2, 1), Point::new(2, 1));
                let mut selection = Selection::new(if reversed { end } else { start }, ty);
                selection.update(if reversed { start } else { end });
                term.set_selection(Some(selection));
                term.process(b"\x1b[?2026h\x1b[?1049hSECRET");
                assert_eq!(
                    term.get_selection_text().as_deref(),
                    Some(if ty == SelectionType::Block {
                        "e\nh\nk"
                    } else {
                        "def\nghi\njkl"
                    })
                );
                let selection = term.selection().unwrap();
                assert_eq!((selection.bounds().0.row, selection.bounds().1.row), (0, 2));
                assert!(!selection.active);
            }
        }
    }

    #[test]
    fn frozen_selection_never_falls_back_to_hidden_selection_and_retires_on_resize() {
        let mut term = Terminal::new(8, 3);
        term.process(b"visible\x1b[?2026h\x1b[H\x1b[2Khidden");
        let mut selection = Selection::new(Point::new(0, 0), SelectionType::Simple);
        selection.update(Point::new(0, 6));
        term.set_selection(Some(selection.clone()));
        assert_eq!(term.get_selection_text().as_deref(), Some("visible"));
        assert!(
            term.selection.is_none(),
            "frozen coordinates must not survive on the live grid"
        );
        term.resize(8, 3);
        assert_eq!(term.get_selection_text().as_deref(), Some("visible"));
        term.resize(4, 6);
        assert!(term.selection().is_none());
        term.set_selection(Some(selection));
        term.process(b"\x1b[?2026l");
        assert!(term.get_selection_text().is_none());
    }

    #[test]
    fn physical_selections_keep_rows_and_stream_selections_keep_edited_padding() {
        let mut term = Terminal::new(3, 3);
        term.process("ab界Z".as_bytes());
        for ty in [SelectionType::Block, SelectionType::Lines] {
            let mut selection = Selection::new(Point::new(0, 0), ty);
            selection.update(Point::new(1, 2));
            term.set_selection(Some(selection));
            assert_eq!(term.get_selection_text().as_deref(), Some("ab \n界Z"));
        }
        for styled in [false, true] {
            let mut term = Terminal::new(3, 3);
            term.process("ab界Z".as_bytes());
            let cell = term.grid_mut().cell_mut(0, 2).unwrap();
            if styled {
                cell.bg_spec = ColorSpec::Indexed(4);
            } else {
                cell.set_char(' ');
            }
            let mut selection = Selection::new(Point::new(0, 0), SelectionType::Simple);
            selection.update(Point::new(1, 2));
            term.set_selection(Some(selection));
            assert_eq!(term.get_selection_text().as_deref(), Some("ab 界Z"));
        }
    }

    #[test]
    fn stream_selection_reflow_round_trips_unicode_and_endpoint_direction() {
        for ty in [SelectionType::Simple, SelectionType::Semantic] {
            for reverse in [false, true] {
                for spacer_start in [false, true] {
                    let mut term = Terminal::new(5, 6);
                    term.process("ab界Zxy".as_bytes());
                    let mut start = Point::new(0, if spacer_start { 3 } else { 0 });
                    let mut end = Point::new(1, 1);
                    if reverse {
                        std::mem::swap(&mut start, &mut end);
                    }
                    let mut selection = Selection::new(start, ty);
                    selection.update(end);
                    selection.finish();
                    term.set_selection(Some(selection));
                    let text = term.get_selection_text();
                    for cols in [3, 1, 7, 2, 5] {
                        term.resize(cols, 20);
                        assert_eq!(
                            term.get_selection_text(),
                            text,
                            "type={ty:?} reverse={reverse} spacer={spacer_start} cols={cols}"
                        );
                        let selection = term.selection().unwrap();
                        assert_eq!(selection.ty, ty);
                        assert!(!selection.active);
                        assert_eq!(selection.start > selection.end, reverse);
                    }
                }
            }
        }
    }

    #[test]
    fn reflow_selection_crosses_compact_history_viewport_boundary_without_aliasing() {
        for reverse in [false, true] {
            let mut term = Terminal::new(5, 2);
            term.process("prefix\r\nab界ZxyzQrst".as_bytes());
            let locate = |term: &Terminal, character| {
                (-(term.grid().scrollback_len() as i32)..term.rows() as i32)
                    .find_map(|row| {
                        let cells = term.grid().row_at_offset(row).unwrap();
                        (0..cells.len())
                            .find(|&col| {
                                cells
                                    .get(col)
                                    .is_some_and(|cell| cell.content.char() == Some(character))
                            })
                            .map(|col| Point::new(row, col))
                    })
                    .unwrap()
            };
            let mut start = locate(&term, 'a');
            let mut end = locate(&term, 't');
            if reverse {
                std::mem::swap(&mut start, &mut end);
            }
            let mut selection = Selection::new(start, SelectionType::Simple);
            selection.update(end);
            term.set_selection(Some(selection));
            for cols in [3, 1, 9, 4, 5] {
                term.resize(cols, 2);
                assert_eq!(
                    term.get_selection_text().as_deref(),
                    Some("ab界ZxyzQrst"),
                    "cols={cols} reverse={reverse}"
                );
                assert_eq!(
                    term.selection().unwrap().start > term.selection().unwrap().end,
                    reverse
                );
            }
        }
    }

    #[test]
    fn reflow_selection_matches_retained_character_identities_across_history_limits() {
        let locate = |term: &Terminal, character| {
            (-(term.grid().scrollback_len() as i32)..term.rows() as i32).find_map(|row| {
                let cells = term.grid().row_at_offset(row).unwrap();
                (0..cells.len())
                    .find(|&col| {
                        cells
                            .get(col)
                            .is_some_and(|cell| cell.content.char() == Some(character))
                    })
                    .map(|col| Point::new(row, col))
            })
        };
        for limit in [0, 1, 8] {
            for cols in [1, 2, 4, 7, 12] {
                for rows in [1, 3, 8] {
                    for (first, last) in
                        [('A', 'B'), ('C', 'J'), ('K', 'N'), ('O', 'V'), ('S', 'V')]
                    {
                        for reverse in [false, true] {
                            let mut term = Terminal::new(8, 3);
                            term.grid_mut().set_scrollback_limit(limit);
                            term.process(b"AB\r\nCDEFGHIJ\r\nKLMN\r\nOPQRSTUV");
                            let (Some(mut start), Some(mut end)) =
                                (locate(&term, first), locate(&term, last))
                            else {
                                continue;
                            };
                            if reverse {
                                std::mem::swap(&mut start, &mut end);
                            }
                            let mut selection = Selection::new(start, SelectionType::Simple);
                            selection.update(end);
                            term.set_selection(Some(selection));
                            let original = term.get_selection_text();
                            term.resize(cols, rows);
                            if let (Some(mut start), Some(mut end)) =
                                (locate(&term, first), locate(&term, last))
                            {
                                if reverse {
                                    std::mem::swap(&mut start, &mut end);
                                }
                                let selection =
                                    term.selection().expect("both selected endpoints survived");
                                assert_eq!(
                                    (selection.start, selection.end),
                                    (start, end),
                                    "limit={limit} cols={cols} rows={rows}"
                                );
                                assert_eq!(term.get_selection_text(), original);
                            } else {
                                assert!(
                                    term.selection().is_none(),
                                    "lost endpoint must not alias retained text"
                                );
                            }
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn width_reflow_retires_evicted_clipped_padding_and_geometric_selections() {
        for limit in [0, 1] {
            let mut term = Terminal::new(8, 3);
            term.grid_mut().set_scrollback_limit(limit);
            term.process(b"abcdefgh\r\nijklmnop\r\nqrstuvwx");
            let mut selection = Selection::new(Point::new(0, 0), SelectionType::Simple);
            selection.update(Point::new(0, 3));
            term.set_selection(Some(selection));
            term.resize(2, 2);
            assert!(term.selection().is_none());
        }
        let mut clipped = Terminal::new(8, 3);
        clipped.process(b"abcdefgh\r\nijklmnop\r\nqrstuvwx\x1b[H");
        clipped.set_selection(Some(Selection::new(
            Point::new(2, 1),
            SelectionType::Simple,
        )));
        clipped.resize(2, 2);
        assert!(clipped.selection().is_none());

        for ty in [SelectionType::Block, SelectionType::Lines] {
            let mut term = Terminal::new(8, 3);
            term.process(b"abcdefgh");
            let mut selection = Selection::new(Point::new(0, 0), ty);
            selection.update(Point::new(0, 3));
            term.set_selection(Some(selection));
            term.resize(8, 4);
            assert!(term.selection().is_some());
            term.resize(4, 4);
            assert!(term.selection().is_none());
        }
        let mut padding = Terminal::new(8, 3);
        padding.process(b"a");
        padding.set_selection(Some(Selection::new(
            Point::new(0, 6),
            SelectionType::Simple,
        )));
        padding.resize(4, 3);
        assert!(padding.selection().is_none());
    }

    #[test]
    fn width_reflow_maps_live_stream_selection_but_retires_frozen_projection() {
        for alternate in [false, true] {
            let mut term = Terminal::new(5, 6);
            if alternate {
                term.process(b"\x1b[?1049h");
            }
            term.process("ab界Zxy".as_bytes());
            let mut selection = Selection::new(Point::new(0, 0), SelectionType::Simple);
            selection.update(Point::new(1, 1));
            term.set_selection(Some(selection));
            term.process(b"\x1b[?2026h");
            term.resize(3, 6);
            assert!(term.selection().is_none());
            term.process(b"\x1b[?2026l");
            assert_eq!(term.get_selection_text().as_deref(), Some("ab界Zxy"));
        }
    }

    #[test]
    fn width_resize_maps_existing_stream_selection_through_wide_padding() {
        let mut term = Terminal::new(5, 4);
        term.process("ab界Z".as_bytes());
        let mut selection = Selection::new(Point::new(0, 0), SelectionType::Simple);
        selection.update(Point::new(0, 4));
        term.set_selection(Some(selection));
        term.resize(3, 4);
        assert_eq!(term.selection().unwrap().end, Point::new(1, 2));
        assert_eq!(term.get_selection_text().as_deref(), Some("ab界Z"));
    }

    #[test]
    fn height_resize_selection_matches_retained_row_identity() {
        for limit in [0, 1, 4] {
            for cursor_row in 0..5 {
                for new_rows in 1..=7 {
                    for ty in [
                        SelectionType::Simple,
                        SelectionType::Semantic,
                        SelectionType::Block,
                        SelectionType::Lines,
                    ] {
                        for selected_row in -2..5 {
                            for multiline in [false, true] {
                                for reverse in [false, true] {
                                    let mut term = Terminal::new(4, 5);
                                    term.grid_mut().set_scrollback_limit(limit);
                                    term.process(
                                        b"aaaa\r\nbbbb\r\ncccc\r\ndddd\r\neeee\r\nffff\r\ngggg",
                                    );
                                    let oldest = -(term.grid().scrollback_len() as i32);
                                    if selected_row < oldest {
                                        continue;
                                    }
                                    let mut start = Point::new(selected_row, 1);
                                    let mut end =
                                        Point::new(if multiline { 4 } else { selected_row }, 2);
                                    if reverse {
                                        std::mem::swap(&mut start, &mut end);
                                    }
                                    let mut selection = Selection::new(start, ty);
                                    selection.update(end);
                                    selection.finish();
                                    let original: Vec<_> = selection
                                        .row_range()
                                        .map(|row| {
                                            (
                                                row,
                                                term.grid().row_at_offset(row).unwrap().to_string(),
                                            )
                                        })
                                        .collect();
                                    term.set_selection(Some(selection.clone()));
                                    let text = term.get_selection_text();
                                    term.grid_mut().set_cursor(cursor_row, 0);
                                    term.resize(4, new_rows);
                                    let new_oldest = -(term.grid().scrollback_len() as i32);
                                    let mut uniform = None;
                                    let mut survives = true;
                                    for (old_row, text) in original {
                                        let mapped = (new_oldest..new_rows as i32).find(|&row| {
                                            term.grid().row_at_offset(row).unwrap().to_string()
                                                == text
                                        });
                                        if let Some(row) = mapped {
                                            let shift = row - old_row;
                                            if uniform.is_some_and(|prior| prior != shift) {
                                                survives = false;
                                            }
                                            uniform = Some(shift);
                                        } else {
                                            survives = false;
                                        }
                                    }
                                    assert_eq!(
                                        term.selection().is_some(),
                                        survives,
                                        "limit={limit} cursor={cursor_row} rows={new_rows} type={ty:?} {start:?}..{end:?}"
                                    );
                                    if survives {
                                        let shift = uniform.unwrap();
                                        let actual = term.selection().unwrap();
                                        assert_eq!(
                                            actual.start,
                                            Point::new(start.row + shift, start.col)
                                        );
                                        assert_eq!(
                                            actual.end,
                                            Point::new(end.row + shift, end.col)
                                        );
                                        assert_eq!(actual.ty, ty);
                                        assert!(!actual.active);
                                        assert_eq!(term.get_selection_text(), text);
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn height_resize_retires_clipped_multicells_and_isolates_frozen_selection() {
        for alternate in [false, true] {
            let mut term = Terminal::new(12, 5);
            if alternate {
                term.process(b"\x1b[?1049h");
            }
            term.process(b"\x1b]66;s=3;X\x07");
            term.set_selection(Some(Selection::new(Point::new(0, 0), SelectionType::Block)));
            term.resize(12, 5);
            assert_eq!(term.get_selection_text().as_deref(), Some("X"));
            term.resize(12, 2);
            assert!(term.selection().is_none());
            assert!(term.grid().cell(0, 0).unwrap().content.is_empty());

            let mut frozen = Terminal::new(12, 4);
            if alternate {
                frozen.process(b"\x1b[?1049h");
            }
            frozen.process(b"anchor\r\nsecond\r\nthird\r\nlast");
            let mut selection = Selection::new(Point::new(0, 0), SelectionType::Simple);
            selection.update(Point::new(0, 5));
            frozen.set_selection(Some(selection));
            frozen.process(b"\x1b[?2026h");
            frozen.resize(12, 4);
            assert_eq!(frozen.get_selection_text().as_deref(), Some("anchor"));
            frozen.resize(12, 2);
            assert!(frozen.selection().is_none()); // Resized frozen projection is retired.
            frozen.process(b"\x1b[?2026l");
            if alternate {
                assert!(frozen.selection().is_none()); // Alternate grid has no history retention.
            } else {
                assert_eq!(frozen.get_selection_text().as_deref(), Some("anchor"));
                assert_eq!(frozen.selection().unwrap().start.row, -2);
            }
        }
    }

    #[test]
    fn height_resize_moves_selection_with_archived_rows() {
        let mut term = Terminal::new(12, 4);
        term.process(b"anchor\r\nsecond\r\nthird\r\nlast");
        let mut selection = Selection::new(Point::new(0, 0), SelectionType::Simple);
        selection.update(Point::new(0, 5));
        term.set_selection(Some(selection));
        term.resize(12, 2);
        assert_eq!(term.selection().unwrap().start, Point::new(-2, 0));
        assert_eq!(term.get_selection_text().as_deref(), Some("anchor"));
    }

    #[test]
    fn stream_selection_after_resize_keeps_logical_unicode_text() {
        let mut term = Terminal::new(5, 4);
        term.process("ab界Z".as_bytes());
        term.resize(3, 4);
        let mut selection = Selection::new(Point::new(0, 0), SelectionType::Simple);
        selection.update(Point::new(1, 2));
        term.set_selection(Some(selection));
        assert_eq!(term.get_selection_text().as_deref(), Some("ab界Z"));
        // Starting on a wide spacer still copies the complete grapheme once.
        let mut selection = Selection::new(Point::new(1, 1), SelectionType::Simple);
        selection.update(Point::new(1, 2));
        term.set_selection(Some(selection));
        assert_eq!(term.get_selection_text().as_deref(), Some("界Z"));
    }

    #[test]
    fn wide_text_resize_round_trip_does_not_materialize_wrap_padding() {
        let mut term = Terminal::new(5, 4);
        term.process("ab界Z".as_bytes());
        term.process(b"\x1b[1;5H");
        for cols in [3, 5, 3, 5] {
            term.resize(cols, 4);
        }
        assert_eq!(term.grid().row(0).unwrap().to_string(), "ab界Z");
        let cursor = term.grid().cursor();
        assert_eq!((cursor.row, cursor.col), (0, 4));
    }

    #[test]
    fn native_wide_wrap_padding_survives_history_and_visible_snapshots() {
        let mut term = Terminal::new(3, 3);
        term.process("ab界Z".as_bytes());
        assert!(term.grid().row(0).unwrap().has_wide_wrap_padding());
        let mut snapshot = term.grid().visible_snapshot();
        snapshot.resize(5, 3);
        assert_eq!(snapshot.row(0).unwrap().to_string(), "ab界Z");
        term.process(b"\r\nx\r\ny\r\nz");
        assert_eq!(term.grid().scrollback_len(), 2);
        term.grid_mut().set_scroll_offset(2);
        let mut history_snapshot = term.grid().visible_snapshot();
        history_snapshot.resize(5, 3);
        assert_eq!(history_snapshot.row(0).unwrap().to_string(), "ab界Z");
        term.grid_mut().set_scroll_offset(0);
        for cols in [5, 3, 5] {
            term.resize(cols, 3);
        }
        assert_eq!(
            term.grid().scrollback_rows().next().unwrap().to_string(),
            "ab界Z"
        );

        // A variation selector can turn the last-column narrow scalar into a
        // wide cluster. Its vacated default cell is the same synthetic padding.
        let mut variation = Terminal::new(3, 3);
        variation.process("ab☺\u{fe0f}Z".as_bytes());
        variation.resize(5, 3);
        assert_eq!(variation.grid().row(0).unwrap().to_string(), "ab☺\u{fe0f}Z");
    }

    #[test]
    fn wide_wrap_never_discards_explicit_or_edited_blank_cells() {
        for styled in [false, true] {
            let mut term = Terminal::new(3, 3);
            term.process("ab界Z".as_bytes());
            let cell = term.grid_mut().cell_mut(0, 2).unwrap();
            if styled {
                cell.bg_spec = ColorSpec::Indexed(4);
            } else {
                cell.set_char(' ');
            }
            term.resize(6, 3);
            assert_eq!(term.grid().row(0).unwrap().to_string(), "ab 界Z");
            if styled {
                assert_eq!(
                    term.grid().cell(0, 2).unwrap().bg_spec,
                    ColorSpec::Indexed(4)
                );
            }
        }
    }

    #[test]
    fn resize_round_trip_retains_background_color_erasure_cells() {
        let mut term = Terminal::new(5, 4);
        term.process(b"A\x1b[44m\x1b[K\x1b[0m");
        for cols in [3, 5] {
            term.resize(cols, 4);
            assert_eq!(term.grid().cell(0, 0).unwrap().content.to_text(), "A");
            for offset in 1..5 {
                let cell = term.grid().cell(offset / cols, offset % cols).unwrap();
                assert!(cell.content.is_empty());
                assert_eq!(cell.bg_spec, ColorSpec::Indexed(4));
            }
            assert_eq!(
                term.grid().cell(1, cols - 1).unwrap().bg_spec,
                ColorSpec::Default
            );
        }
    }

    #[test]
    fn resize_keeps_cursor_on_text_after_wide_character_reflow() {
        for (cols, expected) in [(3, (1, 2)), (1, (3, 0))] {
            let mut term = Terminal::new(5, 4);
            term.process("ab界Z".as_bytes());
            term.process(b"\x1b[1;5H");
            term.resize(cols, 4);
            let cursor = term.grid().cursor();
            assert_eq!((cursor.row, cursor.col), expected);
            assert_eq!(
                term.grid()
                    .cell(cursor.row, cursor.col)
                    .unwrap()
                    .content
                    .to_text(),
                "Z"
            );
            term.process(b"Q");
            assert_eq!(
                term.grid()
                    .cell(expected.0, expected.1)
                    .unwrap()
                    .content
                    .to_text(),
                "Q"
            );
            assert!(term.rows_text(0, 3).contains('界'));
            assert_wide_invariants(&term);
        }
    }

    #[test]
    fn classic_csi_repetition_and_erasure_preserve_unicode_cells() {
        let mut term = Terminal::new(20, 3);
        term.process(b"\x1b[5b");
        assert!(term.grid().row(0).unwrap().to_string().is_empty());

        term.process("e\u{301}".as_bytes());
        term.process(b"\x1b[2b");
        assert_eq!(
            term.grid().row(0).unwrap().to_string(),
            "e\u{301}e\u{301}e\u{301}"
        );
        assert_eq!(term.grid().cursor().col, 3);
        for col in 0..3 {
            assert_eq!(
                term.grid().cell(0, col).unwrap().content.to_text(),
                "e\u{301}"
            );
        }

        // REP uses the current rendition rather than cloning the source cell.
        term.process(b"\r\x1b[2K\x1b[31mA\x1b[32m\x1b[2b");
        assert_eq!(
            term.grid().cell(0, 0).unwrap().fg_spec,
            ColorSpec::Indexed(1)
        );
        assert_eq!(
            term.grid().cell(0, 1).unwrap().fg_spec,
            ColorSpec::Indexed(2)
        );
        assert_eq!(
            term.grid().cell(0, 2).unwrap().fg_spec,
            ColorSpec::Indexed(2)
        );

        let mut wide = Terminal::new(10, 2);
        wide.process("A界B".as_bytes());
        wide.process(b"\x1b[41m\x1b[1;3H\x1b[X");
        assert_eq!(wide.grid().cursor().col, 2);
        assert!(wide.grid().cell(0, 1).unwrap().is_empty());
        assert!(wide.grid().cell(0, 2).unwrap().is_empty());
        assert_eq!(
            wide.grid().cell(0, 1).unwrap().bg_spec,
            ColorSpec::Indexed(1)
        );
        assert_eq!(
            wide.grid().cell(0, 2).unwrap().bg_spec,
            ColorSpec::Indexed(1)
        );
        assert_eq!(wide.grid().cell(0, 3).unwrap().content.char(), Some('B'));
        assert_wide_invariants(&wide);

        // Singular scalar parameters are required for these controls.
        wide.process(b"\x1b[1;1H\x1b[2;3b\x1b[1:2X");
        assert_eq!(wide.grid().cell(0, 0).unwrap().content.char(), Some('A'));

        wide.process(b"\x1bc\x1b[3b");
        assert!(wide.grid().row(0).unwrap().to_string().is_empty());
        wide.last_graphic = Some((
            CellContent::Grapheme("x".repeat(MAX_REPEAT_GRAPHEME_BYTES + 1).into_boxed_str()),
            CharacterSet::Ascii,
        ));
        wide.repeat_last_graphic(1);
        assert!(wide.grid().row(0).unwrap().to_string().is_empty());
    }

    #[test]
    fn repeat_state_keeps_single_scalars_inline_and_clusters_owned() {
        let mut term = Terminal::new(10, 2);

        term.process(b"A");
        assert!(matches!(
            term.last_graphic,
            Some((CellContent::Char('A'), CharacterSet::Ascii))
        ));

        term.process("e\u{301}".as_bytes());
        assert!(matches!(
            term.last_graphic,
            Some((CellContent::Grapheme(ref cluster), CharacterSet::Ascii))
                if cluster.as_ref() == "e\u{301}"
        ));
        term.process(b"\x1b[2b");
        assert_eq!(
            term.grid().row(0).unwrap().to_string(),
            "Ae\u{301}e\u{301}e\u{301}"
        );
    }

    #[test]
    fn erase_and_character_edits_honor_bce_without_moving_the_cursor() {
        let mut term = Terminal::new(8, 3);
        term.process(b"one\r\ntwo\r\nthree\r\nfour");
        assert!(term.grid().scrollback_len() > 0);
        let history = term.grid().scrollback_len();
        let cursor = term.grid().cursor();
        let cursor_state = (cursor.row, cursor.col, cursor.wrap_pending, cursor.visible);

        term.process(b"\x1b[44m\x1b[2J");
        let cursor = term.grid().cursor();
        assert_eq!(
            (cursor.row, cursor.col, cursor.wrap_pending, cursor.visible),
            cursor_state
        );
        assert_eq!(term.grid().scrollback_len(), history);
        for row in 0..term.rows() {
            assert!(!term.grid().row(row).unwrap().is_wrapped());
            for col in 0..term.cols() {
                let cell = term.grid().cell(row, col).unwrap();
                assert!(cell.is_empty());
                assert_eq!(cell.bg_spec, ColorSpec::Indexed(4));
                assert!(cell.attrs.is_empty());
            }
        }

        term.process(b"Z");
        let visible = term.rows_text(0, term.rows() as i32 - 1);
        let cursor = term.grid().cursor();
        let cursor_state = (cursor.row, cursor.col, cursor.wrap_pending, cursor.visible);
        term.process(b"\x1b[3J");
        assert_eq!(term.grid().scrollback_len(), 0);
        assert_eq!(term.rows_text(0, term.rows() as i32 - 1), visible);
        let cursor = term.grid().cursor();
        assert_eq!(
            (cursor.row, cursor.col, cursor.wrap_pending, cursor.visible),
            cursor_state
        );

        let mut edits = Terminal::new(8, 2);
        edits.process(b"ABCD\x1b[1;2H\x1b[42m\x1b[2@");
        assert_eq!(
            edits.grid().cell(0, 1).unwrap().bg_spec,
            ColorSpec::Indexed(2)
        );
        assert_eq!(
            edits.grid().cell(0, 2).unwrap().bg_spec,
            ColorSpec::Indexed(2)
        );
        edits.process(b"\x1b[2P");
        assert_eq!(
            edits.grid().cell(0, 6).unwrap().bg_spec,
            ColorSpec::Indexed(2)
        );
        assert_eq!(
            edits.grid().cell(0, 7).unwrap().bg_spec,
            ColorSpec::Indexed(2)
        );

        let mut wide_edits = Terminal::new(8, 2);
        wide_edits.process("A界B".as_bytes());
        wide_edits.process(b"\x1b[1;3H\x1b[43m\x1b[@");
        assert_wide_invariants(&wide_edits);
        assert_eq!(
            wide_edits.grid().cell(0, 1).unwrap().bg_spec,
            ColorSpec::Indexed(3)
        );
        assert_eq!(
            wide_edits.grid().cell(0, 3).unwrap().bg_spec,
            ColorSpec::Indexed(3)
        );
    }

    #[test]
    fn tabulation_controls_preserve_custom_stops_across_resize_and_ris() {
        let mut term = Terminal::new(24, 3);
        term.process(b"\t\x1b[I\x1b[Z");
        assert_eq!(term.grid().cursor().col, 8);

        term.process(b"\x1b[g\r\x1b[I");
        assert_eq!(term.grid().cursor().col, 16);
        term.process(b"\x1b[3g\r\x1b[I");
        assert_eq!(term.grid().cursor().col, 23);

        term.process(b"\x1b[?5W\r\x1b[2I");
        assert_eq!(term.grid().cursor().col, 16);
        term.process(b"\x1b[2Z");
        assert_eq!(term.grid().cursor().col, 0);

        // A retained custom stop survives growth, while newly exposed columns
        // receive defaults. Stops lost to a shrink are defaulted when regrown.
        term.process(b"\x1b[3g\x1b[5G\x1bH");
        term.resize(30, 3);
        term.process(b"\r\x1b[I");
        assert_eq!(term.grid().cursor().col, 4);
        term.process(b"\x1b[I");
        assert_eq!(term.grid().cursor().col, 24);
        term.resize(3, 3);
        term.resize(20, 3);
        term.process(b"\r\x1b[2I");
        assert_eq!(term.grid().cursor().col, 16);

        term.process(b"\x1b[2;2H\x1b[3a\x1b[2e\x1b[7`");
        assert_eq!(term.grid().cursor().row, 2);
        assert_eq!(term.grid().cursor().col, 6);

        term.process(b"\x1b[3g\x1bc\t");
        assert_eq!(term.grid().cursor().col, 8);
    }

    #[test]
    fn declrmm_negotiates_queries_resets_and_disambiguates_csi_s() {
        let mut term = Terminal::new(10, 5);
        term.grid_mut().set_cursor(2, 4);
        term.process(b"\x1b[s");
        term.grid_mut().set_cursor(0, 0);
        term.process(b"\x1b[u");
        assert_eq!((term.grid().cursor().row, term.grid().cursor().col), (2, 4));

        term.process(b"\x1b[?69h\x1b[3;8s");
        assert!(term.mode().contains(TerminalMode::LEFT_RIGHT_MARGIN));
        assert_eq!(term.horizontal_scroll_region(), 2..8);
        assert_eq!((term.grid().cursor().row, term.grid().cursor().col), (0, 0));

        term.process(b"\x1bP$qs\x1b\\");
        assert_eq!(
            term.drain_events(),
            vec![TerminalEvent::PtyWrite(b"\x1bP1$r3;8s\x1b\\".to_vec())]
        );

        term.grid_mut().set_cursor(3, 6);
        term.process(b"\x1b[8;3s\x1b[2:4s\x1b[1;2;3s");
        assert_eq!(term.horizontal_scroll_region(), 2..8);
        assert_eq!((term.grid().cursor().row, term.grid().cursor().col), (3, 6));

        term.process(b"\x1b[s");
        assert_eq!(term.horizontal_scroll_region(), 0..10);
        assert_eq!((term.grid().cursor().row, term.grid().cursor().col), (0, 0));
        term.process(b"\x1b[3;8s");
        term.grid_mut().set_cursor(3, 6);

        term.process(b"\x1b[?69l");
        assert!(!term.mode().contains(TerminalMode::LEFT_RIGHT_MARGIN));
        assert_eq!(term.horizontal_scroll_region(), 0..10);
        assert_eq!((term.grid().cursor().row, term.grid().cursor().col), (3, 6));

        // A parameterized DECSLRM-shaped sequence is not a cursor save alias
        // once mode 69 is off.
        term.process(b"\x1b[3;8s");
        term.grid_mut().set_cursor(0, 0);
        term.process(b"\x1b[u");
        assert_eq!((term.grid().cursor().row, term.grid().cursor().col), (2, 4));

        term.process(b"\x1b[?69h\x1b[2;9s");
        term.resize(8, 5);
        assert!(term.mode().contains(TerminalMode::LEFT_RIGHT_MARGIN));
        assert_eq!(term.horizontal_scroll_region(), 0..8);
        term.process(b"\x1bc");
        assert!(!term.mode().contains(TerminalMode::LEFT_RIGHT_MARGIN));
        assert_eq!(term.horizontal_scroll_region(), 0..8);
    }

    #[test]
    fn horizontal_margins_constrain_dec_cursor_motion_origin_and_tabs() {
        let mut term = Terminal::new(10, 5);
        term.process(b"\x1b[2;4r\x1b[?69h\x1b[3;8s\x1b[?6h");
        assert_eq!((term.grid().cursor().row, term.grid().cursor().col), (1, 2));

        term.process(b"\x1b[99;99H");
        assert_eq!((term.grid().cursor().row, term.grid().cursor().col), (3, 7));
        term.process(b"\x1b[99A\x1b[99D");
        assert_eq!((term.grid().cursor().row, term.grid().cursor().col), (1, 2));
        term.process(b"\x1b[99a\x1b[99e");
        assert_eq!((term.grid().cursor().row, term.grid().cursor().col), (3, 7));

        term.process(b"\x1b[?6l\x1b[3;1H\x1b[99C");
        assert_eq!(term.grid().cursor().col, 7);
        term.process(b"\x1b[99D");
        assert_eq!(term.grid().cursor().col, 2);
        term.process(b"\x1b[99a");
        assert_eq!(term.grid().cursor().col, 9);

        term.process(b"\x1b[3;3H\x1b[I");
        assert_eq!(term.grid().cursor().col, 7);
        term.process(b"\x1b[Z");
        assert_eq!(term.grid().cursor().col, 2);

        term.process(b"\x1b[3;10H\r");
        assert_eq!(term.grid().cursor().col, 2);
        term.process(b"\x1b[3;2H\r");
        assert_eq!(term.grid().cursor().col, 0);
    }

    #[test]
    fn margin_wrap_and_scroll_preserve_cells_outside_the_rectangle() {
        let mut term = Terminal::new(10, 4);
        term.process(
            b"AAAAAAAAAA\r\nBBBBBBBBBB\r\nCCCCCCCCCC\r\nDDDDDDDDDD\x1b[2;4r\x1b[?69h\x1b[3;8s\x1b[4;3H123456X",
        );

        assert_eq!(term.grid().row(0).unwrap().to_string(), "AAAAAAAAAA");
        assert_eq!(term.grid().row(1).unwrap().to_string(), "BBCCCCCCBB");
        assert_eq!(term.grid().row(2).unwrap().to_string(), "CC123456CC");
        assert_eq!(term.grid().row(3).unwrap().to_string(), "DDX     DD");
        assert_eq!((term.grid().cursor().row, term.grid().cursor().col), (3, 3));
        assert_eq!(term.grid().scrollback_len(), 0);
        assert!(!term.grid().row(2).unwrap().is_wrapped());

        term.process(b"\x1b[3;8H");
        term.process("界".as_bytes());
        assert_eq!(term.grid().cell(3, 2).unwrap().content.to_text(), "界");
        assert_wide_invariants(&term);

        let mut boundary = Terminal::new(6, 3);
        boundary.process(b"\x1b[2;2H");
        boundary.process("界".as_bytes());
        boundary.process(b"\x1b[?69h\x1b[3;5s\x1b[S");
        assert!(boundary.grid().cell(1, 1).unwrap().is_empty());
        assert!(boundary.grid().cell(1, 2).unwrap().is_empty());
        assert_wide_invariants(&boundary);
    }

    #[test]
    fn margin_character_and_line_edits_are_bounded_and_use_bce() {
        let mut term = Terminal::new(10, 4);
        term.process(
            b"AAAAAAAAAA\r\nBBBBBBBBBB\r\nCCCCCCCCCC\r\nDDDDDDDDDD\x1b[2;4r\x1b[?69h\x1b[3;8s\x1b[2;4H\x1b[44m\x1b[2@",
        );
        assert_eq!(term.grid().row(1).unwrap().to_string(), "BBB  BBBBB");
        for col in 3..5 {
            assert_eq!(
                term.grid().cell(1, col).unwrap().bg_spec,
                ColorSpec::Indexed(4)
            );
        }

        term.process(b"\x1b[2P");
        assert_eq!(term.grid().row(1).unwrap().to_string(), "BBBBBB  BB");
        term.process(b"\x1b[L");
        assert_eq!(term.grid().cursor().col, 2);
        assert_eq!(term.grid().row(1).unwrap().to_string(), "BB      BB");
        assert_eq!(term.grid().row(2).unwrap().to_string(), "CCBBBB  CC");
        assert_eq!(term.grid().row(3).unwrap().to_string(), "DDCCCCCCDD");

        let preserved = term.rows_text(0, 3);
        term.process(b"\x1b[1;4H\x1b[L\x1b[M");
        assert_eq!(term.rows_text(0, 3), preserved);
        assert_wide_invariants(&term);
    }

    #[test]
    fn margin_index_reverse_index_and_explicit_scroll_share_the_rectangle() {
        fn populated() -> Terminal {
            let mut term = Terminal::new(8, 4);
            term.process(
                b"AAAAAAAA\r\nBBBBBBBB\r\nCCCCCCCC\r\nDDDDDDDD\x1b[2;4r\x1b[?69h\x1b[3;6s",
            );
            term
        }

        for sequence in [b"\x1bD".as_slice(), b"\x1b[S".as_slice()] {
            let mut term = populated();
            term.process(b"\x1b[4;4H");
            term.process(sequence);
            assert_eq!(term.grid().row(1).unwrap().to_string(), "BBCCCCBB");
            assert_eq!(term.grid().row(2).unwrap().to_string(), "CCDDDDCC");
            assert_eq!(term.grid().row(3).unwrap().to_string(), "DD    DD");
        }

        let mut next_line = populated();
        next_line.process(b"\x1b[4;5H\x1bE");
        assert_eq!(next_line.grid().row(1).unwrap().to_string(), "BBCCCCBB");
        assert_eq!(next_line.grid().cursor().col, 2);

        for sequence in [b"\x1bM".as_slice(), b"\x1b[T".as_slice()] {
            let mut term = populated();
            term.process(b"\x1b[2;4H");
            term.process(sequence);
            assert_eq!(term.grid().row(1).unwrap().to_string(), "BB    BB");
            assert_eq!(term.grid().row(2).unwrap().to_string(), "CCBBBBCC");
            assert_eq!(term.grid().row(3).unwrap().to_string(), "DDCCCCDD");
        }
    }

    #[test]
    fn images_scroll_only_when_fully_inside_horizontal_margins() {
        let mut term = Terminal::new(10, 4);
        term.grid_mut().set_cursor(2, 3);
        term.handle_graphics_command(b"a=T,f=32,s=1,v=1,i=1,c=1,r=1,C=1;AQIDBA==");
        term.grid_mut().set_cursor(2, 0);
        term.handle_graphics_command(b"a=T,f=32,s=1,v=1,i=2,c=1,r=1,C=1;AQIDBA==");
        term.grid_mut().set_cursor(2, 7);
        term.handle_graphics_command(b"a=T,f=32,s=1,v=1,i=3,c=2,r=1,C=1;AQIDBA==");

        term.process(b"\x1b[2;4r\x1b[?69h\x1b[3;8s\x1b[S");
        let placements: Vec<_> = term
            .graphics_placements()
            .map(|placement| (placement.image_id, placement.col, placement.row))
            .collect();
        assert!(placements.contains(&(ImageId(1), 3, 1)));
        assert!(placements.contains(&(ImageId(2), 0, 2)));
        assert!(placements.contains(&(ImageId(3), 7, 2)));
    }

    #[test]
    fn decstr_resets_only_the_documented_session_state() {
        let mut term = Terminal::new(12, 3);
        term.process(b"one\r\ntwo\r\nthree\r\nfour");
        assert!(term.grid().scrollback_len() > 0);

        // Install a non-default tab stop, then establish every supported
        // DECSTR-controlled state and several unrelated modern modes.
        term.process(
            b"\x1b[3g\x1b[5G\x1bH\x1b[2;3r\x1b[?69h\x1b[3;10s\x1b[?1;6;12h\x1b[?7;25l\x1b[4;20h\x1b=\x1b[?1000;1004;1006;1007;2004h\x1b[31;42;1m\x1b[1\"q\x1b(0\x1b[6 q",
        );
        term.save_cursor();
        term.grid_mut().set_cursor(1, 4);
        term.grid_mut().set_wrap_pending(true);

        let preserved_text = term.rows_text(0, term.rows() as i32 - 1);
        let preserved_scrollback = term.grid().scrollback_len();
        let preserved_modes = TerminalMode::LINEFEED_NEWLINE
            | TerminalMode::MOUSE_TRACKING
            | TerminalMode::FOCUS_EVENTS
            | TerminalMode::MOUSE_SGR
            | TerminalMode::ALTERNATE_SCROLL
            | TerminalMode::BRACKETED_PASTE;
        assert!(term.mode().contains(preserved_modes));

        // Feed DECSTR one byte at a time so the parser's intermediate survives
        // arbitrary PTY read boundaries.
        for byte in b"\x1b[!p" {
            term.process(&[*byte]);
        }

        assert_eq!(term.rows_text(0, term.rows() as i32 - 1), preserved_text);
        assert_eq!(term.grid().scrollback_len(), preserved_scrollback);
        assert_eq!(term.scroll_region(), 0..term.rows());
        assert_eq!(term.horizontal_scroll_region(), 0..term.cols());
        let cursor = term.grid().cursor();
        assert_eq!((cursor.row, cursor.col), (1, 4));
        assert!(!cursor.wrap_pending);
        assert!(cursor.visible);
        assert!(term.mode().contains(TerminalMode::AUTOWRAP));
        assert!(term.mode().contains(preserved_modes));
        assert!(!term.mode().intersects(
            TerminalMode::INSERT
                | TerminalMode::CURSOR_KEYS_APPLICATION
                | TerminalMode::ORIGIN
                | TerminalMode::CURSOR_BLINKING
                | TerminalMode::KEYPAD_APPLICATION
                | TerminalMode::LEFT_RIGHT_MARGIN,
        ));
        assert_eq!(term.cursor_style(), None);
        assert_eq!(term.cursor_blink_override(), None);
        assert_eq!(term.character_protection_parameter(), 0);

        // SGR, DECSCA, and G0/GL return to defaults for subsequent text.
        term.write_char('q');
        let cell = term.grid().cell(1, 4).unwrap();
        assert_eq!(cell.content.char(), Some('q'));
        assert_eq!(cell.attrs, CellAttrs::empty());
        assert_eq!(cell.fg_spec, ColorSpec::Default);
        assert_eq!(cell.bg_spec, ColorSpec::Default);
        assert!(!cell.flags.contains(crate::grid::CellFlags::PROTECTED));

        // DECSTR replaces the active screen's DECSC state with home/defaults,
        // while preserving the custom tab-stop table.
        term.restore_cursor();
        assert_eq!((term.grid().cursor().row, term.grid().cursor().col), (0, 0));
        term.process(b"\t");
        assert_eq!(term.grid().cursor().col, 4);
    }

    #[test]
    fn decstr_preserves_the_active_screen_and_rejects_malformed_forms() {
        let mut term = Terminal::new(8, 3);
        term.process(b"P\x1b[2;3H\x1b7\x1b[?1047hA\x1b[3;4H\x1b7");
        assert!(term.mode().contains(TerminalMode::ALT_SCREEN));

        // Nonzero, plural, and subparameter forms are not DECSTR.
        term.process(b"\x1b[4h\x1b[1!p\x1b[0;0!p\x1b[0:0!p");
        assert!(term.mode().contains(TerminalMode::INSERT));
        assert_eq!(term.grid().row(0).unwrap().to_string(), "A");

        term.process(b"\x1b[!p");
        assert!(term.mode().contains(TerminalMode::ALT_SCREEN));
        assert!(!term.mode().contains(TerminalMode::INSERT));
        assert_eq!(term.grid().row(0).unwrap().to_string(), "A");
        term.restore_cursor();
        assert_eq!((term.grid().cursor().row, term.grid().cursor().col), (0, 0));

        // The inactive primary screen and its saved cursor remain untouched.
        term.exit_alt_screen();
        assert_eq!(term.grid().row(0).unwrap().to_string(), "P");
        term.restore_cursor();
        assert_eq!((term.grid().cursor().row, term.grid().cursor().col), (1, 2));
    }

    #[test]
    fn decscl_soft_resets_and_reports_bounded_level_and_c1_state() {
        let mut term = Terminal::new(12, 3);
        assert_eq!(term.dec_conformance_level(), 4);
        assert!(!term.uses_8bit_controls());

        term.process(b"keep\x1b[3g\x1b[6G\x1bH\x1b[4h\x1b[62;0\"p");
        assert_eq!(term.grid().row(0).unwrap().to_string(), "keep");
        assert!(!term.mode().contains(TerminalMode::INSERT));
        assert_eq!(term.dec_conformance_level(), 2);
        assert!(term.uses_8bit_controls());
        term.process(b"\x1bP$q\"p\x1b\\");
        assert_eq!(
            term.drain_events(),
            vec![TerminalEvent::PtyWrite(b"\x901$r62;0\"p\x9c".to_vec())]
        );

        // An invalid level or level-2+ C1 selector changes nothing, including
        // state that would have been cleared by the DECSCL soft reset.
        term.process(b"\x1b[4h\x1b[66;1\"p\x1b[63;3\"p\x1b[64;1;2\"p\x1b[64:1\"p");
        assert!(term.mode().contains(TerminalMode::INSERT));
        assert_eq!(term.dec_conformance_level(), 2);
        assert!(term.uses_8bit_controls());

        // Pc is ignored at level 1, whose replies are necessarily 7-bit.
        term.process(b"\x1b[61;9\"p\x1bP$q\"p\x1b\\");
        assert!(!term.mode().contains(TerminalMode::INSERT));
        assert_eq!(term.dec_conformance_level(), 1);
        assert!(!term.uses_8bit_controls());
        assert_eq!(
            term.drain_events(),
            vec![TerminalEvent::PtyWrite(b"\x1bP1$r61;1\"p\x1b\\".to_vec())]
        );

        // An omitted Pc defaults to 8-bit controls above level 1. RIS returns
        // to Termeric's level-4, 7-bit startup profile.
        term.process(b"\x1b[65\"p");
        assert_eq!(term.dec_conformance_level(), 5);
        assert!(term.uses_8bit_controls());
        term.process(b"\x1bc");
        assert_eq!(term.dec_conformance_level(), 4);
        assert!(!term.uses_8bit_controls());
    }

    #[test]
    fn selection_and_export_emit_each_grapheme_once_without_spacers() {
        let mut term = Terminal::new(8, 2);
        term.process("A界e\u{301}👍🏽".as_bytes());
        let mut selection = Selection::new(
            Point::new(0, 2), // select from the spacer half of 界
            SelectionType::Simple,
        );
        selection.update(Point::new(0, 5));
        term.set_selection(Some(selection));
        assert_eq!(term.get_selection_text().as_deref(), Some("界e\u{301}👍🏽"));
        let exported = term.grid().export_scrollback_text().join("\n");
        assert_eq!(exported, "A界e\u{301}👍🏽");
    }

    #[test]
    fn hidden_text_remains_copyable() {
        let mut term = Terminal::new(20, 2);
        term.process(b"\x1b[8msecret\x1b[28m");
        let mut selection = Selection::new(Point::new(0, 0), SelectionType::Simple);
        selection.update(Point::new(0, 5));
        term.set_selection(Some(selection));

        assert_eq!(term.get_selection_text().as_deref(), Some("secret"));
        assert!(
            term.grid()
                .cell(0, 0)
                .unwrap()
                .attrs
                .contains(CellAttrs::HIDDEN)
        );
    }

    #[test]
    fn wide_pair_property_holds_across_seeded_edit_sequences() {
        for seed in 0..64_u64 {
            let mut state = seed + 1;
            let mut term = Terminal::new(12, 4);
            term.process("A界e\u{301}👍🏽👩\u{200d}💻Z".as_bytes());
            for _ in 0..128 {
                state = state
                    .wrapping_mul(6_364_136_223_846_793_005)
                    .wrapping_add(1_442_695_040_888_963_407);
                let cols = term.cols();
                let col = (state as usize >> 8) % cols.max(1);
                term.grid_mut().set_cursor(0, col);
                match state % 6 {
                    0 => term.insert_chars((state as usize % 3) + 1),
                    1 => term.delete_chars((state as usize % 3) + 1),
                    2 => term.erase_in_line((state % 3) as u16),
                    3 => term.write_char('界'),
                    4 => term.write_char('\u{301}'),
                    _ => term.resize((state as usize % 16) + 1, 4),
                }
                assert_wide_invariants(&term);
            }
        }
    }

    fn write_line(term: &mut Terminal, s: &str) {
        for c in s.chars() {
            term.write_char(c);
        }
    }

    #[test]
    fn test_word_at_simple_word() {
        let mut term = Terminal::new(80, 24);
        write_line(&mut term, "hello world");
        let (start, end) = term.word_at(Point::new(0, 2)); // inside "hello"
        assert_eq!(start, Point::new(0, 0));
        assert_eq!(end, Point::new(0, 4));
    }

    #[test]
    fn test_word_at_whitespace_is_noop() {
        let mut term = Terminal::new(80, 24);
        write_line(&mut term, "hello world");
        let (start, end) = term.word_at(Point::new(0, 5)); // the space
        assert_eq!(start, Point::new(0, 5));
        assert_eq!(end, Point::new(0, 5));
    }

    #[test]
    fn test_word_at_url_like_token() {
        // path- and URL-like punctuation is part of the word.
        let mut term = Terminal::new(80, 24);
        write_line(&mut term, "cat /tmp/foo.txt done");
        let (start, end) = term.word_at(Point::new(0, 8)); // inside "/tmp/foo.txt"
        assert_eq!(start, Point::new(0, 4));
        assert_eq!(end, Point::new(0, 15));
    }

    #[test]
    fn test_line_at_trims_trailing_space() {
        let mut term = Terminal::new(80, 24);
        write_line(&mut term, "hi   ");
        let (start, end) = term.line_at(Point::new(0, 3));
        assert_eq!(start, Point::new(0, 0));
        assert_eq!(end, Point::new(0, 1)); // "hi" ends at col 1
    }

    #[test]
    fn test_cursor_movement() {
        let mut term = Terminal::new(80, 24);
        term.set_cursor_position(5, 10);
        let cursor = term.grid().cursor();
        assert_eq!(cursor.row, 4);
        assert_eq!(cursor.col, 9);
    }

    #[test]
    fn test_alt_screen() {
        let mut term = Terminal::new(80, 24);
        term.write_char('A');
        term.enter_alt_screen();
        assert!(term.mode().contains(TerminalMode::ALT_SCREEN));
        assert!(term.grid().cell(0, 0).unwrap().is_empty());
        term.exit_alt_screen();
        assert!(!term.mode().contains(TerminalMode::ALT_SCREEN));
        assert_eq!(term.grid().cell(0, 0).unwrap().content.char(), Some('A'));
    }

    #[test]
    fn mouse_focus_paste_and_alternate_scroll_modes_parse_and_reset() {
        let mut term = Terminal::new(80, 24);
        term.process(b"\x1b[?9;1000;1002;1003;1004;1005;1006;1007;2004h");
        let expected = TerminalMode::MOUSE_X10
            | TerminalMode::MOUSE_TRACKING
            | TerminalMode::MOUSE_BUTTON_EVENT
            | TerminalMode::MOUSE_ANY_EVENT
            | TerminalMode::FOCUS_EVENTS
            | TerminalMode::MOUSE_UTF8
            | TerminalMode::MOUSE_SGR
            | TerminalMode::ALTERNATE_SCROLL
            | TerminalMode::BRACKETED_PASTE;
        assert!(term.mode().contains(expected));

        term.process(b"\x1b[?9;1000;1002;1003;1004;1005;1006;1007;2004l");
        assert!(!term.mode().intersects(expected));
    }

    #[test]
    fn application_keyboard_modes_set_and_restore_through_the_parser() {
        let mut term = Terminal::new(80, 24);
        term.process(b"\x1b[?1h\x1b=");
        assert!(term.mode().contains(TerminalMode::CURSOR_KEYS_APPLICATION));
        assert!(term.mode().contains(TerminalMode::KEYPAD_APPLICATION));

        term.process(b"\x1b[?1l\x1b>");
        assert!(
            !term.mode().intersects(
                TerminalMode::CURSOR_KEYS_APPLICATION | TerminalMode::KEYPAD_APPLICATION
            )
        );
    }

    #[test]
    fn xterm_keyboard_types_are_exclusive_queryable_and_reset_with_xterm_lifecycle() {
        let mut term = Terminal::new(80, 24);
        assert_eq!(term.xterm_keyboard_type(), XtermKeyboardType::Normal);

        term.process(b"\x1b[?1060h");
        assert_eq!(term.xterm_keyboard_type(), XtermKeyboardType::Legacy);
        assert_eq!(term.mode_status(true, 1060), 1);
        assert_eq!(term.mode_status(true, 1061), 2);

        term.process(b"\x1b[?1061h");
        assert_eq!(term.xterm_keyboard_type(), XtermKeyboardType::Vt220);
        assert_eq!(term.mode_status(true, 1060), 2);
        assert_eq!(term.mode_status(true, 1061), 1);

        // Xterm leaves the configured/runtime keyboard type alone on DECSTR,
        // while resetting either type mode selects the normal type.
        term.process(b"\x1b[!p");
        assert_eq!(term.xterm_keyboard_type(), XtermKeyboardType::Vt220);
        term.process(b"\x1b[?1060l");
        assert_eq!(term.xterm_keyboard_type(), XtermKeyboardType::Normal);

        term.process(b"\x1b[?1060h\x1bc");
        assert_eq!(term.xterm_keyboard_type(), XtermKeyboardType::Normal);
    }

    #[test]
    fn configured_xterm_keyboard_type_and_private_save_restore_have_distinct_lifecycles() {
        let mut term = Terminal::new(80, 24);
        term.configure_xterm_keyboard_type(XtermKeyboardType::Legacy, true);
        assert_eq!(
            term.initial_xterm_keyboard_type(),
            XtermKeyboardType::Legacy
        );
        assert_eq!(term.xterm_keyboard_type(), XtermKeyboardType::Legacy);

        // DECRST still selects xterm's normal/default type. With no explicit
        // save, a private-mode restore falls back to the configured baseline.
        term.process(b"\x1b[?1061l\x1b[?1060r");
        assert_eq!(term.xterm_keyboard_type(), XtermKeyboardType::Legacy);

        // Both mode numbers address the same saved keyboard-type slot.
        term.process(b"\x1b[?1061h\x1b[?1060s\x1b[?1060h\x1b[?1061r");
        assert_eq!(term.xterm_keyboard_type(), XtermKeyboardType::Vt220);

        // Malformed subparameters do not overwrite the saved VT220 value.
        term.process(b"\x1b[?1060:1061s\x1b[?1060h\x1b[?1060r");
        assert_eq!(term.xterm_keyboard_type(), XtermKeyboardType::Vt220);

        term.soft_reset();
        assert_eq!(term.xterm_keyboard_type(), XtermKeyboardType::Vt220);
        term.reset();
        assert_eq!(term.xterm_keyboard_type(), XtermKeyboardType::Legacy);

        // A changed live configuration establishes a new current/RIS baseline
        // and clears an obsolete saved mode value.
        term.configure_xterm_keyboard_type(XtermKeyboardType::Normal, true);
        term.process(b"\x1b[?1060r");
        assert_eq!(term.xterm_keyboard_type(), XtermKeyboardType::Normal);
    }

    #[test]
    fn decudk_definitions_are_bounded_preservable_and_reset_only_by_ris() {
        let mut term = Terminal::new(80, 24);
        term.process(b"\x1bP0;1|17/68656c6c6f;1/1b5b41;34/00ff;35/aa;18/0;19/zz\x1b\\");
        assert_eq!(term.xterm_user_defined_key(17), Some(b"hello".as_slice()));
        assert_eq!(term.xterm_user_defined_key(1), Some(b"\x1b[A".as_slice()));
        assert_eq!(term.xterm_user_defined_key(34), Some(b"\0\xff".as_slice()));
        assert_eq!(term.xterm_user_defined_key(18), None);
        assert_eq!(term.xterm_user_defined_key(19), None);
        assert_eq!(term.xterm_user_defined_key(35), None);

        // Nonzero first parameters preserve other entries. Xterm accepts the
        // lock parameter but keeps UDKs unlocked, so a later definition wins.
        term.process(b"\x1bP1;0|17/776f726c64;18/21\x1b\\");
        assert_eq!(term.xterm_user_defined_key(17), Some(b"world".as_slice()));
        assert_eq!(term.xterm_user_defined_key(18), Some(b"!".as_slice()));
        assert_eq!(term.xterm_user_defined_key(1), Some(b"\x1b[A".as_slice()));

        term.soft_reset();
        assert_eq!(term.xterm_user_defined_key(17), Some(b"world".as_slice()));
        term.process(b"\x1bc");
        assert_eq!(term.xterm_user_defined_key(17), None);
        assert_eq!(term.xterm_user_defined_key_bytes(), 0);

        // Level 1 predates DECUDK. Returning to level 2 admits it again.
        term.process(b"\x1b[61\"p\x1bP0;1|17/61\x1b\\");
        assert_eq!(term.xterm_user_defined_key(17), None);
        term.process(b"\x1b[62;1\"p\x1bP0;1|17/62\x1b\\");
        assert_eq!(term.xterm_user_defined_key(17), Some(b"b".as_slice()));

        let oversized = "ab".repeat(MAX_USER_DEFINED_KEY_BYTES + 1);
        term.process(format!("\x1bP1;1|18/{oversized}\x1b\\").as_bytes());
        assert_eq!(term.xterm_user_defined_key(18), None);

        let maximum = "ab".repeat(MAX_USER_DEFINED_KEY_BYTES);
        for code in 0..XTERM_USER_DEFINED_KEY_COUNT as u16 {
            term.process(format!("\x1bP1;1|{code}/{maximum}\x1b\\").as_bytes());
        }
        assert!(term.xterm_user_defined_key_bytes() <= MAX_USER_DEFINED_KEYS_BYTES);
        assert_eq!(
            term.xterm_user_defined_keys().count(),
            MAX_USER_DEFINED_KEYS_BYTES / MAX_USER_DEFINED_KEY_BYTES
        );
    }

    #[test]
    fn reserved_xterm_string_key_resource_is_queryable_resettable_and_encoder_neutral() {
        let mut term = Terminal::new(80, 24);
        term.process(
            b"\x1b[?5m\x1b[?5g\x1b[>5;4m\x1b[>5;1f\x1b[?5m\x1b[?5g\x1bP$q>5m\x1b\\\x1bP$q>5f\x1b\\",
        );
        assert_eq!(
            term.xterm_string_keys(),
            XtermStringKeys::new(XtermKeyMode::Extended, XtermKeyFormat::CsiU)
        );
        assert_eq!(
            term.drain_events(),
            vec![
                TerminalEvent::PtyWrite(b"\x1b[>5;0m".to_vec()),
                TerminalEvent::PtyWrite(b"\x1b[>5;0f".to_vec()),
                TerminalEvent::PtyWrite(b"\x1b[>5;4m".to_vec()),
                TerminalEvent::PtyWrite(b"\x1b[>5;1f".to_vec()),
                TerminalEvent::PtyWrite(b"\x1bP1$r>5;4m\x1b\\".to_vec()),
                TerminalEvent::PtyWrite(b"\x1bP1$r>5;1f\x1b\\".to_vec()),
            ]
        );

        // Out-of-range and malformed shapes are atomic. The reserved state is
        // global across screens and has no effect on ordinary screen content.
        term.process(b"\x1b[>5;9m\x1b[>5;2f\x1b[>5:1m\x1b[?1049hX");
        assert_eq!(
            term.xterm_string_keys(),
            XtermStringKeys::new(XtermKeyMode::Extended, XtermKeyFormat::CsiU)
        );
        assert_eq!(term.grid().cell(0, 0).unwrap().content.char(), Some('X'));

        term.process(b"\x1b[>5n");
        assert_eq!(term.xterm_string_keys().mode(), XtermKeyMode::Disabled);
        term.process(b"\x1b[>5m");
        assert_eq!(term.xterm_string_keys().mode(), XtermKeyMode::Original);
        term.process(b"\x1b[>5;4m\x1b[>5;1f\x1b[!p");
        assert_eq!(term.xterm_string_keys(), XtermStringKeys::default());
        term.process(b"\x1b[>5;4m\x1b[>5;1f\x1bc");
        assert_eq!(term.xterm_string_keys(), XtermStringKeys::default());
    }

    #[test]
    fn decrqss_reports_legacy_geometry_without_enabling_window_mutation() {
        let mut term = Terminal::new(132, 40);
        term.process(b"\x1bP$qt\x1b\\\x1bP$q$|\x1b\\\x1bP$q*|\x1b\\\x1bP$q$}\x1b\\\x1bP$q$~\x1b\\");
        assert_eq!(
            term.drain_events(),
            vec![
                TerminalEvent::PtyWrite(b"\x1bP1$r39t\x1b\\".to_vec()),
                TerminalEvent::PtyWrite(b"\x1bP1$r80$|\x1b\\".to_vec()),
                TerminalEvent::PtyWrite(b"\x1bP1$r40*|\x1b\\".to_vec()),
                TerminalEvent::PtyWrite(b"\x1bP0$r\x1b\\".to_vec()),
                TerminalEvent::PtyWrite(b"\x1bP0$r\x1b\\".to_vec()),
            ]
        );

        // Geometry and DEC status-line mutations remain application/window
        // policy operations and are deliberately ignored by the core.
        term.process(b"\x1b[50t\x1b[132$|\x1b[99*|\x1b[1$}\x1b[1$~");
        assert_eq!((term.cols(), term.rows()), (132, 40));
        assert!(term.drain_events().is_empty());

        // External geometry remains observable. Current xterm reports a
        // minimum DECSLPP value of 24, while DECSNLS reports the exact rows.
        term.resize(100, 24);
        term.process(b"\x1b G\x90$qt\x9c\x90$q$|\x9c\x90$q*|\x9c");
        assert_eq!(
            term.drain_events(),
            vec![
                TerminalEvent::PtyWrite(b"\x901$r24t\x9c".to_vec()),
                TerminalEvent::PtyWrite(b"\x901$r80$|\x9c".to_vec()),
                TerminalEvent::PtyWrite(b"\x901$r24*|\x9c".to_vec()),
            ]
        );
    }

    /// Helper: collect all non-empty characters from scrollback + visible area.
    /// This is a content-only check — ignores line boundaries.
    fn extract_all_chars(term: &Terminal) -> String {
        let grid = term.grid();
        let mut chars = String::new();

        // Scrollback
        for row in grid.scrollback_rows() {
            for cell in row.iter() {
                if let Some(c) = cell.content.char() {
                    chars.push(c);
                }
            }
        }

        // Visible area
        for r in 0..grid.rows() {
            if let Some(row) = grid.row(r) {
                for cell in row.iter() {
                    if let Some(c) = cell.content.char() {
                        chars.push(c);
                    }
                }
            }
        }

        chars
    }

    #[test]
    fn test_process_and_resize_preserves_content() {
        // Process a few lines of text at 40 cols, then shrink to 10 cols.
        // All characters must still be present.
        let mut term = Terminal::new(40, 24);
        term.process(b"Hello World\r\n");
        term.process(b"ABCDEFGHIJKLMNOPQRSTUVWXYZ\r\n");
        term.process(b"1234567890\r\n");

        let before = extract_all_chars(&term);

        // Shrink to 10 columns
        term.resize(10, 24);

        let after = extract_all_chars(&term);
        assert_eq!(before, after, "Characters lost after shrink to 10 cols");

        // Grow back to 40 columns
        term.resize(40, 24);

        let after_grow = extract_all_chars(&term);
        assert_eq!(
            before, after_grow,
            "Characters lost after grow back to 40 cols"
        );
    }

    #[test]
    fn test_process_long_line_reflow_to_small_width() {
        // Write a 50-char line in a 50-col terminal, then shrink to 5 cols.
        // All 50 characters must be preserved.
        let mut term = Terminal::new(50, 24);
        let text = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuv";
        term.process(text.as_bytes());
        term.process(b"\r\n");

        let before = extract_all_chars(&term);
        assert_eq!(before.len(), text.len());

        term.resize(5, 24);

        let after = extract_all_chars(&term);
        assert_eq!(before, after, "Characters lost after shrink to 5 cols");
    }

    #[test]
    fn test_process_resize_to_very_small_height() {
        // Write multiple lines, then shrink to 3 rows.
        // Content should go to scrollback, not be lost.
        let mut term = Terminal::new(20, 10);
        for i in 0..8 {
            term.process(format!("Line {}\r\n", i).as_bytes());
        }

        let before = extract_all_chars(&term);

        term.resize(20, 3);

        let after = extract_all_chars(&term);
        assert_eq!(before, after, "Characters lost after shrink to 3 rows");
    }

    #[test]
    fn test_process_resize_both_dimensions_small() {
        // Start at 40x10, write content, shrink to 5x3.
        // This is the scenario the user reported as corrupting text.
        let mut term = Terminal::new(40, 10);
        term.process(b"Hello World\r\n");
        term.process(b"ABCDEFGHIJKLMNOPQRSTUVWXYZ\r\n");
        term.process(b"$ ");

        let before = extract_all_chars(&term);

        term.resize(5, 3);

        let after = extract_all_chars(&term);
        assert_eq!(before, after, "Characters lost after shrink to 5x3");
    }

    #[test]
    fn test_autowrap_sets_wrapped_flag() {
        // Write enough chars to fill exactly one row and trigger autowrap.
        // The first row should be marked as wrapped.
        let mut term = Terminal::new(10, 5);
        // Write exactly 10 chars to fill row 0
        term.process(b"ABCDEFGHIJ");
        // The cursor is now at col 10 (past end). Write one more char to trigger autowrap.
        term.process(b"K");

        // Row 0 should be wrapped
        assert!(
            term.grid().row(0).unwrap().is_wrapped(),
            "Row 0 should be marked as wrapped after autowrap"
        );
        // Row 1 should have 'K'
        assert_eq!(
            term.grid().cell(1, 0).unwrap().content.char(),
            Some('K'),
            "Character after autowrap should be on row 1"
        );
    }

    #[test]
    fn test_autowrap_reflow_round_trip() {
        // Write 20 chars in a 10-col terminal (autowrap creates 2 rows).
        // Grow to 20 cols — should merge back to 1 row.
        // Shrink back to 10 cols — should split to 2 rows again.
        let mut term = Terminal::new(10, 5);
        term.process(b"ABCDEFGHIJKLMNOPQRST"); // 20 chars, fills 2 rows

        assert!(term.grid().row(0).unwrap().is_wrapped());

        let original_chars = extract_all_chars(&term);
        assert_eq!(original_chars.len(), 20);

        // Grow to 20 cols — should merge into 1 row
        term.resize(20, 5);
        let after_grow = extract_all_chars(&term);
        assert_eq!(original_chars, after_grow, "Chars lost on grow");

        let row0_text = term.grid().row(0).unwrap().to_string();
        assert_eq!(row0_text, "ABCDEFGHIJKLMNOPQRST");

        // Shrink back to 10
        term.resize(10, 5);
        let after_shrink = extract_all_chars(&term);
        assert_eq!(original_chars, after_shrink, "Chars lost on shrink back");
    }

    #[test]
    fn test_initial_small_terminal_no_corruption() {
        // Create a very small terminal and write text that exceeds its width.
        // This tests the scenario where the pane is "too small to begin with."
        let mut term = Terminal::new(5, 3);
        term.process(b"Hello World!\r\n");
        term.process(b"ABCDEFGHIJ\r\n");

        let chars = extract_all_chars(&term);
        // "Hello World!" is 12 chars, "ABCDEFGHIJ" is 10 chars — total 22
        assert_eq!(chars.len(), 22, "Expected 22 chars, got: '{}'", chars);
        assert!(chars.contains("Hello"), "Missing 'Hello' in output");
        assert!(chars.contains("World"), "Missing 'World' in output");
        assert!(
            chars.contains("ABCDEFGHIJ"),
            "Missing 'ABCDEFGHIJ' in output"
        );
    }

    #[test]
    fn test_small_terminal_then_grow() {
        // Write into a 5x3 terminal, then grow to 40x24.
        // All content must be preserved (in scrollback or visible).
        let mut term = Terminal::new(5, 3);
        term.process(b"ABCDEFGHIJKLMNO\r\n"); // 15 chars
        term.process(b"XYZ\r\n");

        let before = extract_all_chars(&term);

        term.resize(40, 24);

        let after = extract_all_chars(&term);
        assert_eq!(
            before, after,
            "Characters lost after growing from 5x3 to 40x24"
        );
    }

    #[test]
    fn test_pane_close_reflow_simulation() {
        // Simulate the pane lifecycle: start at full width, shrink to pane width,
        // write text that wraps, then grow back to full width.
        // This mirrors what happens when a pane is created and then closed.
        let mut term = Terminal::new(80, 24);

        // Phase 1: Resize to pane width (simulating split)
        term.resize(40, 24);
        assert_eq!(term.cols(), 40);

        // Phase 2: Write text that wraps at 40 cols
        // A 60-char line will wrap: 40 chars on row 0 (wrapped), 20 chars on row 1
        let long_line = "ABCDEFGHIJ".repeat(6); // 60 chars
        term.process(long_line.as_bytes());
        term.process(b"\r\n");
        term.process(b"Short line\r\n");
        term.process(b"$ "); // prompt

        let before = extract_all_chars(&term);
        assert!(
            before.contains(&long_line),
            "Long line should be fully present"
        );

        // Phase 3: Resize back to full width (simulating pane close)
        term.resize(80, 24);

        let after = extract_all_chars(&term);
        assert_eq!(
            before, after,
            "Characters lost after expanding from 40 to 80 cols"
        );

        // The 60-char line should now fit on a single row (no wrapping needed at 80 cols)
        // Check that row 0 has the full content (either in scrollback or visible)
        let grid = term.grid();
        let mut found_full_line = false;
        for r in 0..grid.rows() {
            if let Some(row) = grid.row(r) {
                let text = row.to_string();
                if text == long_line {
                    found_full_line = true;
                    assert!(
                        !row.is_wrapped(),
                        "60-char line should not be wrapped at 80 cols"
                    );
                    break;
                }
            }
        }
        // Also check scrollback
        for row in grid.scrollback_rows() {
            let text = row.to_string();
            if text == long_line {
                found_full_line = true;
                assert!(
                    !row.is_wrapped(),
                    "60-char line in scrollback should not be wrapped at 80 cols"
                );
                break;
            }
        }
        assert!(
            found_full_line,
            "Should find the full 60-char line on a single row after resize to 80 cols"
        );
    }

    #[test]
    fn test_pane_reflow_multiple_wrapped_lines() {
        // Multiple auto-wrapped lines should all reflow when growing.
        let mut term = Terminal::new(20, 24);

        // Write 3 lines that wrap at 20 cols
        let line1 = "A".repeat(30); // wraps to 2 rows
        let line2 = "B".repeat(25); // wraps to 2 rows
        let line3 = "C".repeat(40); // wraps to 2 rows

        term.process(line1.as_bytes());
        term.process(b"\r\n");
        term.process(line2.as_bytes());
        term.process(b"\r\n");
        term.process(line3.as_bytes());
        term.process(b"\r\n");

        let before = extract_all_chars(&term);

        // Grow to 80 cols — all lines should unwrap
        term.resize(80, 24);

        let after = extract_all_chars(&term);
        assert_eq!(
            before, after,
            "Characters lost after expanding from 20 to 80 cols"
        );
    }

    #[test]
    fn horizontal_protocol_selection_mapping_obeys_counts_and_margins() {
        let scenarios: &[(&[u8], usize, usize, usize, bool)] = &[
            (b"\x1b[H", 0, 0, 8, true),
            (b"\x1b[2;4H", 1, 3, 8, true),
            (b"\x1b[?69h\x1b[2;7s\x1b[2;3r\x1b[2;3H", 1, 2, 7, true),
            (b"\x1b[?69h\x1b[2;7s\x1b[2;3r\x1b[1;3H", 0, 2, 8, true),
            (b"\x1b[?69h\x1b[2;7s\x1b[2;3r\x1b[2;1H", 1, 0, 7, false),
            (b"\x1b[?69h\x1b[2;7s\x1b[2;3r\x1b[2;8H", 1, 7, 7, false),
            (
                b"\x1b[?69h\x1b[2;7s\x1b[2;3r\x1b[?6h\x1b[1;2H",
                1,
                2,
                7,
                true,
            ),
        ];
        for &(prefix, edit_row, left, right, enabled) in scenarios {
            for inserting in [false, true] {
                for count in [0_usize, 1, 2, 99] {
                    for index in 0..24 {
                        let mut term = Terminal::new(8, 3);
                        term.process(b"abcdefgh\r\nijklmnop\r\nqrstuvwx");
                        term.process(prefix);
                        let point = Point::new(index / 8, index as usize % 8);
                        term.set_selection(Some(Selection::new(point, SelectionType::Block)));
                        let text = term.get_selection_text();
                        let cursor = term.grid().cursor();
                        term.process(
                            format!("\x1b[{count}{}", if inserting { '@' } else { 'P' }).as_bytes(),
                        );
                        let expected = if enabled
                            && point.row == edit_row as i32
                            && (left..right).contains(&point.col)
                        {
                            let distance = count.max(1).min(right - left) as isize;
                            point
                                .col
                                .checked_add_signed(if inserting { distance } else { -distance })
                                .filter(|col| (left..right).contains(col))
                                .map(|col| Point::new(point.row, col))
                        } else {
                            Some(point)
                        };
                        assert_eq!(
                            term.selection().map(|selection| selection.start),
                            expected,
                            "prefix={prefix:?} insert={inserting} count={count} point={point:?}"
                        );
                        if expected.is_some() {
                            assert_eq!(term.get_selection_text(), text);
                        }
                        assert_eq!(
                            (term.grid().cursor().row, term.grid().cursor().col),
                            (cursor.row, cursor.col)
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn horizontal_edits_preserve_whole_glyphs_and_retire_boundary_damage() {
        for inserting in [false, true] {
            for wide in [false, true] {
                let mut term = Terminal::new(12, 4);
                term.process(b"..\x1b[1\"q");
                if wide {
                    term.process("界".as_bytes());
                } else {
                    term.process(b"\x1b]66;s=1:w=2;A\x07");
                }
                term.grid_mut().set_cursor(0, 0);
                term.set_selection(Some(Selection::new(Point::new(0, 2), SelectionType::Block)));
                let text = term.get_selection_text();
                term.shift_chars(1, inserting);
                assert_eq!(
                    term.selection().unwrap().start.col,
                    if inserting { 3 } else { 1 }
                );
                assert_eq!(term.get_selection_text(), text);
            }

            for selected_col in [2, 3] {
                let mut term = Terminal::new(12, 4);
                term.process("..界".as_bytes());
                term.process(b"\x1b[?69h\x1b[1;3s\x1b[H");
                term.set_selection(Some(Selection::new(
                    Point::new(0, selected_col),
                    SelectionType::Block,
                )));
                term.shift_chars(1, inserting);
                assert!(term.selection().is_none()); // Repair covers both moved and stationary halves.
            }

            let mut block = Terminal::new(12, 4);
            block.process(b"....\x1b]66;s=1:w=4;A\x07\x1b[?69h\x1b[1;6s\x1b[H");
            block.set_selection(Some(Selection::new(Point::new(0, 7), SelectionType::Block)));
            block.shift_chars(1, inserting);
            assert!(block.selection().is_none());
            assert!((0..12).all(|col| !block.grid().cell(0, col).unwrap().multicell.is_present()));

            let mut tall = Terminal::new(12, 4);
            tall.process(b"..\x1b]66;s=2;A\x07\x1b[2;1H");
            tall.set_selection(Some(Selection::new(Point::new(0, 2), SelectionType::Block)));
            tall.shift_chars(0, inserting);
            assert_eq!(tall.get_selection_text().as_deref(), Some("A"));
            tall.shift_chars(1, inserting);
            assert!(tall.selection().is_none()); // Whole block erased outside the edited row.
            assert!(tall.grid().cell(0, 2).unwrap().content.is_empty());
        }
    }

    #[test]
    fn horizontal_edits_isolate_frozen_history_and_mixed_motion_selections() {
        for inserting in [false, true] {
            let mut term = Terminal::new(12, 3);
            term.process(b"..anchor\r\nother");
            let mut selection = Selection::new(Point::new(0, 7), SelectionType::Simple);
            selection.update(Point::new(0, 2));
            term.set_selection(Some(selection.clone()));
            term.process(b"\x1b[22J");
            term.shift_chars(1, inserting);
            assert_eq!(term.get_selection_text().as_deref(), Some("anchor"));
            term.process(b"\x1b[?1049h..anchor\x1b[H");
            term.set_selection(Some(selection));
            term.process(b"\x1b[?2026h");
            term.shift_chars(1, inserting);
            assert_eq!(term.selection().unwrap().start, Point::new(0, 7));
            assert_eq!(term.get_selection_text().as_deref(), Some("anchor"));
            term.process(b"\x1b[?2026l");
            assert_eq!(
                term.selection().unwrap().start.col,
                if inserting { 8 } else { 6 }
            );
            assert_eq!(term.get_selection_text().as_deref(), Some("anchor"));

            let mut mixed = Selection::new(Point::new(0, 2), SelectionType::Block);
            mixed.update(Point::new(1, 4));
            term.set_selection(Some(mixed));
            term.shift_chars(1, inserting);
            assert!(term.selection().is_none());
        }
    }

    #[test]
    fn horizontal_edits_follow_surviving_selected_text() {
        let mut term = Terminal::new(12, 3);
        term.process(b"..anchor\x1b[H");
        let mut selection = Selection::new(Point::new(0, 2), SelectionType::Simple);
        selection.update(Point::new(0, 7));
        term.set_selection(Some(selection));
        term.process(b"\x1b[2@");
        assert_eq!(term.selection().unwrap().start, Point::new(0, 4));
        assert_eq!(term.get_selection_text().as_deref(), Some("anchor"));
        term.process(b"\x1b[2P");
        assert_eq!(term.selection().unwrap().start, Point::new(0, 2));
        assert_eq!(term.get_selection_text().as_deref(), Some("anchor"));
    }

    #[test]
    fn rectangular_write_selection_matches_cellwise_damage_across_selection_shapes() {
        let commands: &[&[u8]] = &[
            b"\x1b[88;1;3;2;5$x",
            b"\x1b[1;1;2;4;1;2;3;1$v",
            b"\x1b[2;3;3;6;1;1;1;1$v",
            b"\x1b[1;1;3;8;1;99;99;1$v",
            b"\x1b[9;1;1;3;8$x",       // Nonprintable fill: no-op.
            b"\x1b[88;3;5;1;2$x",      // Reversed rectangle: no-op.
            b"\x1b[1;1;3;8;1;1;1;1$v", // Self-copy: no-op.
            b"\x1b[?69h\x1b[2;6s\x1b[2;3r\x1b[?6h\x1b[88;1;2;2;3$x",
            b"\x1b[?69h\x1b[2;6s\x1b[2;3r\x1b[?6h\x1b[1;1;1;2;1;2;3;1$v",
        ];
        for &command in commands {
            for ty in [
                SelectionType::Simple,
                SelectionType::Semantic,
                SelectionType::Lines,
                SelectionType::Block,
            ] {
                for index in 0..24 {
                    for reverse in [false, true] {
                        let mut term = Terminal::new(8, 3);
                        term.process(b"abcdefgh\r\nijklmnop\r\nqrstuvwx");
                        let point = Point::new(index / 8, index as usize % 8);
                        let other = Point::new(1, 3);
                        let (start, end) = if reverse {
                            (other, point)
                        } else {
                            (point, other)
                        };
                        let mut selection = Selection::new(start, ty);
                        selection.update(end);
                        let original: Vec<_> = (0..3)
                            .map(|row| term.grid().row(row).unwrap().clone())
                            .collect();
                        term.set_selection(Some(selection.clone()));
                        term.process(command);
                        let damaged = (0..3).any(|row| {
                            (0..8).any(|col| {
                                selection.contains(row as i32, col)
                                    && original[row].get(col).unwrap().content
                                        != term.grid().cell(row, col).unwrap().content
                            })
                        });
                        assert_eq!(
                            term.selection().is_none(),
                            damaged,
                            "command={command:?} type={ty:?} start={start:?} end={end:?}"
                        );
                        if !damaged {
                            let current = term.selection().unwrap();
                            assert_eq!(
                                (current.start, current.end, current.ty, current.active),
                                (
                                    selection.start,
                                    selection.end,
                                    selection.ty,
                                    selection.active
                                )
                            );
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn rectangular_writes_invalidate_erased_wide_and_multicell_parts_outside_destination() {
        for copy in [false, true] {
            for wide in [false, true] {
                for target_leader in [false, true] {
                    let mut term = Terminal::new(12, 4);
                    // Protection does not prevent DECFRA/DECCRA overwriting.
                    term.process(b"\x1b[1\"q");
                    if wide {
                        term.process("界".as_bytes());
                    } else {
                        term.process(b"\x1b]66;s=2:w=2;A\x07");
                    }
                    term.process(b"\x1b[0\"q\x1b[4;10HZ");
                    let covered = Point::new(if wide { 0 } else { 1 }, 1);
                    let leader = Point::new(0, 0);
                    let (target, selected) = if target_leader {
                        (leader, covered)
                    } else {
                        (covered, leader)
                    };
                    term.set_selection(Some(Selection::new(selected, SelectionType::Block)));
                    if copy {
                        term.copy_rectangular_area(
                            4,
                            10,
                            4,
                            10,
                            1,
                            target.row as usize + 1,
                            target.col + 1,
                            1,
                        );
                    } else {
                        term.fill_rectangular_area(
                            b'Z' as u16,
                            target.row as usize + 1,
                            target.col + 1,
                            target.row as usize + 1,
                            target.col + 1,
                        );
                    }
                    assert!(
                        term.selection().is_none(),
                        "copy={copy} wide={wide} target_leader={target_leader}"
                    );
                    assert!(
                        term.grid()
                            .cell(selected.row as usize, selected.col)
                            .unwrap()
                            .content
                            .is_empty()
                    );
                }
            }
        }
    }

    #[test]
    fn rectangular_copy_preserves_source_selection_and_valid_copied_multicells() {
        let mut term = Terminal::new(12, 5);
        term.process(b"\x1b]66;s=2:w=2;A\x07");
        let source = term.grid().multicell_region_at(0, 0).unwrap();
        term.set_selection(Some(Selection::new(Point::new(0, 0), SelectionType::Block)));
        // A clipped source produces no valid destination block, but reading it
        // must not invalidate the selected, untouched original block.
        term.copy_rectangular_area(1, 1, 1, 1, 1, 4, 8, 1);
        assert_eq!(term.get_selection_text().as_deref(), Some("A"));
        assert!(!term.grid().cell(3, 7).unwrap().multicell.is_present());
        term.copy_rectangular_area(1, 1, source.bottom(), source.right(), 1, 4, 8, 1);
        assert_eq!(term.get_selection_text().as_deref(), Some("A"));
        assert!(term.grid().multicell_region_at(3, 7).is_some());
    }

    #[test]
    fn rectangular_writes_keep_history_and_frozen_selection_but_retire_identical_replacement() {
        for copy in [false, true] {
            let command: &[u8] = if copy {
                b"\x1b[2;1;2;6;1;1;1;1$v"
            } else {
                b"\x1b[97;1;1;1;6$x"
            };
            let mut term = Terminal::new(12, 3);
            term.process(b"aaaaaa\r\naaaaaa");
            let mut selection = Selection::new(Point::new(0, 5), SelectionType::Simple);
            selection.update(Point::new(0, 0));
            term.set_selection(Some(selection.clone()));
            term.process(command);
            assert!(term.selection().is_none()); // Even identical replacement retires the anchor.

            term.set_selection(Some(selection.clone()));
            term.process(b"\x1b[22J");
            term.process(command);
            assert_eq!(term.get_selection_text().as_deref(), Some("aaaaaa"));

            term.process(b"\x1b[?1049hanchor\r\nsource");
            term.set_selection(Some(selection));
            term.process(b"\x1b[?2026h");
            term.process(command);
            assert_eq!(term.get_selection_text().as_deref(), Some("anchor"));
            term.process(b"\x1b[?2026l");
            assert!(term.selection().is_none());
        }
    }

    #[test]
    fn rectangular_fill_and_copy_invalidate_selected_destination() {
        for command in [b"\x1b[88;1;1;1;3$x".as_slice(), b"\x1b[1;5;1;7;1;1;1;1$v"] {
            let mut term = Terminal::new(12, 3);
            term.process(b"old new");
            let mut selection = Selection::new(Point::new(0, 0), SelectionType::Simple);
            selection.update(Point::new(0, 2));
            term.set_selection(Some(selection));
            term.process(command);
            assert!(term.selection().is_none(), "command={command:?}");
        }
    }

    #[test]
    fn explicit_erase_invalidates_a_selection_over_removed_text() {
        let mut term = Terminal::new(12, 3);
        term.process(b"selected");
        let mut selection = Selection::new(Point::new(0, 0), SelectionType::Simple);
        selection.update(Point::new(0, 7));
        term.set_selection(Some(selection));
        term.process(b"\x1b[2K");
        assert!(term.selection().is_none());
    }

    #[test]
    fn erase_command_selection_damage_matches_actual_cell_changes() {
        let commands: &[&[u8]] = &[
            b"\x1b[J",
            b"\x1b[1J",
            b"\x1b[2J",
            b"\x1b[K",
            b"\x1b[1K",
            b"\x1b[2K",
            b"\x1b[2X",
            b"\x1b[?J",
            b"\x1b[?1J",
            b"\x1b[?2J",
            b"\x1b[?K",
            b"\x1b[?1K",
            b"\x1b[?2K",
            b"\x1b[2;4;2;5$z",
            b"\x1b[2;4;2;5${",
        ];
        for &command in commands {
            for row in 0..3 {
                for col in 0..8 {
                    let mut term = Terminal::new(8, 3);
                    term.process(b"abcdefgh\r\nijklmnop\r\nqrstuvwx\x1b[2;4H");
                    for protected_row in 0..3 {
                        for protected_col in [1, 4, 7] {
                            term.grid_mut()
                                .cell_mut(protected_row, protected_col)
                                .unwrap()
                                .flags
                                .insert(CellFlags::PROTECTED);
                        }
                    }
                    let original = term.grid().cell(row, col).unwrap().content.to_text();
                    term.set_selection(Some(Selection::new(
                        Point::new(row as i32, col),
                        SelectionType::Block,
                    )));
                    term.process(command);
                    let unchanged =
                        term.grid().cell(row, col).unwrap().content.to_text() == original;
                    assert_eq!(
                        term.selection().is_some(),
                        unchanged,
                        "command={command:?} row={row} col={col}"
                    );
                    assert_eq!((term.grid().cursor().row, term.grid().cursor().col), (1, 3));
                }
            }
        }
    }

    #[test]
    fn selective_erase_preserves_protected_selection_and_invalidates_expanded_glyph_damage() {
        for protected in [false, true] {
            for selective in [false, true] {
                for wide in [false, true] {
                    let mut term = Terminal::new(10, 4);
                    if protected {
                        term.process(b"\x1b[1\"q");
                    }
                    if wide {
                        term.process("界".as_bytes());
                    } else {
                        term.process(b"\x1b]66;s=2:w=2;A\x07");
                    }
                    term.process(b"\x1b[0\"q");
                    term.set_selection(Some(Selection::new(
                        Point::new(0, 0),
                        SelectionType::Block,
                    )));
                    let text = term.get_selection_text();
                    // Target only the wide spacer or a covered lower-row cell,
                    // outside the selection's requested row/column rectangle.
                    let row = if wide { 1 } else { 2 };
                    if selective {
                        term.selective_erase_rectangular_area(row, 2, row, 2);
                    } else {
                        term.erase_rectangular_area(row, 2, row, 2);
                    }
                    assert_eq!(
                        term.selection().is_some(),
                        protected && selective,
                        "protected={protected} selective={selective} wide={wide}"
                    );
                    if protected && selective {
                        assert_eq!(term.get_selection_text(), text);
                    }
                }
            }
        }
    }

    #[test]
    fn erasure_keeps_archived_and_frozen_selection_but_clears_erased_alternate_content() {
        let mut term = Terminal::new(12, 3);
        term.process(b"anchor");
        let mut selection = Selection::new(Point::new(0, 5), SelectionType::Simple);
        selection.update(Point::new(0, 0));
        term.set_selection(Some(selection));
        term.process(b"\x1b[22J");
        assert_eq!(term.selection().unwrap().start, Point::new(-3, 5));
        assert_eq!(term.get_selection_text().as_deref(), Some("anchor"));
        term.process(b"\x1b[2J\x1b[?2J");
        assert_eq!(term.get_selection_text().as_deref(), Some("anchor"));

        term.process(b"\x1b[?1049hanchor");
        let mut selection = Selection::new(Point::new(0, 0), SelectionType::Simple);
        selection.update(Point::new(0, 5));
        term.set_selection(Some(selection));
        term.process(b"\x1b[?2026h\x1b[2J");
        assert_eq!(term.get_selection_text().as_deref(), Some("anchor"));
        term.process(b"\x1b[?2026l");
        assert!(term.selection().is_none());

        term.process("\x1b[H界".as_bytes());
        term.set_selection(Some(Selection::new(Point::new(0, 0), SelectionType::Block)));
        term.grid_mut().set_cursor(0, 1);
        term.erase_chars(0); // The core API's empty range must not erase half a pair.
        term.erase_in_display(99);
        term.selective_erase_in_line(99);
        term.erase_rectangular_area(3, 3, 1, 1);
        assert_eq!(term.get_selection_text().as_deref(), Some("界"));
    }

    #[test]
    fn ordinary_printing_invalidates_a_selection_over_replaced_text() {
        let mut term = Terminal::new(12, 3);
        term.process(b"selected");
        let mut selection = Selection::new(Point::new(0, 0), SelectionType::Simple);
        selection.update(Point::new(0, 7));
        selection.finish();
        term.set_selection(Some(selection));
        term.process(b"\x1b[Hchanged!");
        assert!(term.selection().is_none());
    }

    #[test]
    fn ordinary_writes_preserve_unaffected_history_and_frozen_selections() {
        for kind in [
            SelectionType::Simple,
            SelectionType::Semantic,
            SelectionType::Lines,
            SelectionType::Block,
        ] {
            for reverse in [false, true] {
                let mut term = Terminal::new(12, 3);
                term.process(b"alpha beta\r\nmiddle\r\nlast");
                let (start, end) = if reverse { (4, 0) } else { (0, 4) };
                let mut selection = Selection::new(Point::new(0, start), kind);
                selection.update(Point::new(0, end));
                term.set_selection(Some(selection));
                let text = term.get_selection_text();
                term.process(b"\x1b[3;1HX\x1b[1;1H\x1b[31m");
                assert_eq!(term.get_selection_text(), text);
                assert_eq!(term.selection().unwrap().start, Point::new(0, start));
                // Even an identical printable rewrite replaces selected cells.
                term.process(b"a");
                assert!(term.selection().is_none());
            }
        }
        let mut term = Terminal::new(12, 3);
        term.process(b"anchor\r\none\r\ntwo\r\nthree");
        let mut selection = Selection::new(Point::new(-1, 0), SelectionType::Simple);
        selection.update(Point::new(-1, 5));
        term.set_selection(Some(selection));
        term.process(b"\x1b[Hchanged");
        assert_eq!(term.get_selection_text().as_deref(), Some("anchor"));

        let mut selection = Selection::new(Point::new(0, 0), SelectionType::Simple);
        selection.update(Point::new(0, 6));
        term.set_selection(Some(selection));
        term.process(b"\x1b[?2026h\x1b[Hhidden!");
        assert_eq!(term.get_selection_text().as_deref(), Some("changed"));
        term.process(b"\x1b[?2026l");
        assert!(term.selection().is_none());
    }

    #[test]
    fn grapheme_growth_erases_top_row_collisions_and_preserves_original_rendition() {
        for selected in [Point::new(0, 0), Point::new(1, 2), Point::new(3, 11)] {
            let mut term = Terminal::new(12, 4);
            term.process("\x1b[1;31;44m\x1b[1\"q❤".as_bytes());
            let original = term.grid().cell(0, 0).unwrap().clone();
            term.process(b"\x1b[0m\x1b[0\"q\x1b]66;s=2:w=2;old\x07\x1b[1;2H");
            term.set_selection(Some(Selection::new(selected, SelectionType::Block)));
            term.process("\u{fe0f}".as_bytes());
            assert_eq!(term.selection().is_some(), selected.row == 3);
            let grown = term.grid().cell(0, 0).unwrap();
            assert_eq!(grown.content.to_text(), "❤\u{fe0f}");
            assert_eq!(
                (grown.fg_spec, grown.bg_spec, grown.attrs),
                (original.fg_spec, original.bg_spec, original.attrs)
            );
            assert!(grown.flags.contains(CellFlags::PROTECTED));
            assert!((0..4).all(|row| {
                (0..12).all(|col| !term.grid().cell(row, col).unwrap().multicell.is_present())
            }));
            assert_wide_invariants(&term);
        }
    }

    #[test]
    fn grapheme_growth_skips_lower_multicell_rows_without_invalidating_them() {
        for cols in [6, 12] {
            for autowrap in [false, true] {
                for selected_block in [false, true] {
                    let mut term = Terminal::new(cols, 4);
                    term.process(b"\x1b[1;2H\x1b]66;s=2:w=2;old\x07\x1b[2;1H");
                    term.process("❤".as_bytes());
                    if !autowrap {
                        term.process(b"\x1b[?7l");
                    }
                    let destination = if cols == 6 {
                        Point::new(2, 0)
                    } else {
                        Point::new(1, 5)
                    };
                    let selected = if selected_block {
                        Point::new(0, 1)
                    } else {
                        destination
                    };
                    term.set_selection(Some(Selection::new(selected, SelectionType::Block)));
                    term.process("\u{fe0f}".as_bytes());
                    assert_eq!(term.selection().is_some(), selected_block);
                    if selected_block {
                        assert_eq!(term.get_selection_text().as_deref(), Some("old"));
                    }
                    assert!(term.grid().multicell_region_at(0, 1).is_some());
                    assert!(term.grid().cell(1, 0).unwrap().content.is_empty());
                    assert_eq!(
                        term.grid()
                            .cell(destination.row as usize, destination.col)
                            .unwrap()
                            .content
                            .to_text(),
                        "❤\u{fe0f}"
                    );
                    assert_wide_invariants(&term);
                }
            }
        }
    }

    #[test]
    fn grapheme_growth_wrap_collisions_preserve_erase_space_and_frozen_selection_rules() {
        for covered_top in [false, true] {
            for frozen in [false, true] {
                let mut term = Terminal::new(12, 4);
                term.process(b"\x1b[?1049h\x1b[2;1H\x1b]66;s=2:w=2;old\x07");
                if covered_top {
                    term.process(b"\x1b[?69h\x1b[2;11s\x1b[1;11H");
                } else {
                    term.process(b"\x1b[1;12H");
                }
                term.process("❤".as_bytes());
                term.set_selection(Some(Selection::new(Point::new(1, 0), SelectionType::Block)));
                if frozen {
                    term.process(b"\x1b[?2026h");
                }
                term.process("\u{fe0f}".as_bytes());
                let left = usize::from(covered_top);
                assert_eq!(
                    term.grid().cell(1, left).unwrap().content.to_text(),
                    "❤\u{fe0f}"
                );
                assert!(!term.grid().cell(2, 3).unwrap().multicell.is_present());
                assert_eq!(
                    term.grid().cell(2, 3).unwrap().content.char(),
                    covered_top.then_some(' ')
                );
                if frozen {
                    assert_eq!(term.get_selection_text().as_deref(), Some("old"));
                    term.process(b"\x1b[?2026l");
                }
                assert!(term.selection().is_none());
                assert_wide_invariants(&term);
            }
        }
    }

    #[test]
    fn grapheme_width_growth_cannot_leave_partial_multicell_descriptors() {
        let mut term = Terminal::new(12, 4);
        term.process("❤\x1b]66;s=2:w=2;old\x07\x1b[1;2H".as_bytes());
        term.process("\u{fe0f}".as_bytes());
        assert_wide_invariants(&term);
        for (row, col, cell) in term.grid().iter_cells() {
            assert!(
                !cell.multicell.is_present() || term.grid().multicell_region_at(row, col).is_some(),
                "orphan descriptor at {row},{col}"
            );
        }
    }

    #[test]
    fn ordinary_unicode_writes_invalidate_complete_glyph_damage_including_wrap_destinations() {
        for (selected_col, write_col) in [(0, 1), (1, 0)] {
            let mut term = Terminal::new(6, 3);
            term.process("界x".as_bytes());
            term.set_selection(Some(Selection::new(
                Point::new(0, selected_col),
                SelectionType::Block,
            )));
            term.grid_mut().set_cursor(0, write_col);
            term.write_char('a');
            assert!(term.selection().is_none());
        }
        for initial in ["e", "❤"] {
            let mut term = Terminal::new(6, 3);
            term.process(initial.as_bytes());
            term.set_selection(Some(Selection::new(Point::new(0, 0), SelectionType::Block)));
            term.process("\u{301}".as_bytes());
            assert!(term.selection().is_none());
        }
        let mut term = Terminal::new(6, 3);
        term.process("❤x".as_bytes());
        term.set_selection(Some(Selection::new(Point::new(0, 1), SelectionType::Block)));
        term.grid_mut().set_cursor(0, 1);
        term.process("\u{fe0f}".as_bytes());
        assert!(term.selection().is_none());
        for point in [Point::new(0, 2), Point::new(1, 0)] {
            let mut term = Terminal::new(3, 3);
            term.process("\x1b[2;1HQ\x1b[1;1Hab❤".as_bytes());
            term.set_selection(Some(Selection::new(point, SelectionType::Block)));
            term.process("\u{fe0f}".as_bytes());
            assert!(term.selection().is_none(), "{point:?}");
        }
        let mut term = Terminal::new(6, 3);
        term.process("界x".as_bytes());
        term.set_selection(Some(Selection::new(Point::new(0, 0), SelectionType::Block)));
        term.grid_mut().set_cursor(0, 2);
        term.write_char('y');
        assert_eq!(term.get_selection_text().as_deref(), Some("界"));
    }

    #[test]
    fn ordinary_writes_invalidate_changed_multicell_blocks_but_not_skipped_lower_rows() {
        let mut term = Terminal::new(10, 4);
        term.process(b"\x1b]66;s=2:w=2;A\x07");
        term.set_selection(Some(Selection::new(Point::new(0, 0), SelectionType::Block)));
        term.grid_mut().set_cursor(1, 0);
        term.write_char('x'); // Skips the block and writes at column four.
        assert_eq!(term.get_selection_text().as_deref(), Some("A"));
        term.set_selection(Some(Selection::new(Point::new(1, 0), SelectionType::Block)));
        term.grid_mut().set_cursor(0, 1);
        term.write_char('y');
        assert!(term.selection().is_none());

        let mut term = Terminal::new(10, 4);
        term.process(b"\x1b]66;s=2;A\x07");
        term.set_selection(Some(Selection::new(Point::new(1, 1), SelectionType::Block)));
        term.process("\u{301}".as_bytes());
        assert!(term.selection().is_none());

        let mut term = Terminal::new(12, 4);
        term.process(b"....\x1b]66;s=2;A\x07");
        term.set_selection(Some(Selection::new(Point::new(1, 4), SelectionType::Block)));
        term.process(b"\x1b[H\x1b[4hx");
        assert!(term.selection().is_none());
        assert!(term.grid().multicell_region_at(1, 4).is_none());
    }

    #[test]
    fn partial_scroll_keeps_selection_on_its_moved_text() {
        let mut term = Terminal::new(8, 5);
        term.process(b"zero\r\none\r\ntwo\r\nthree\r\nfour");
        let mut selection = Selection::new(Point::new(2, 0), SelectionType::Simple);
        selection.update(Point::new(2, 2));
        term.set_selection(Some(selection));
        term.process(b"\x1b[2;4r\x1b[S");
        assert_eq!(term.selection().unwrap().start, Point::new(1, 0));
        assert_eq!(term.get_selection_text().as_deref(), Some("two"));
    }

    #[test]
    fn vertical_region_commands_preserve_survivors_and_clear_lost_or_split_selections() {
        let commands: &[(&[u8], i32)] = &[
            (b"\x1b[S", -1),
            (b"\x1b[T", 1),
            (b"\x1b[2;3H\x1b[L", 1),
            (b"\x1b[2;3H\x1b[M", -1),
            (b"\x1b[4;3H\n", -1),
            (b"\x1b[2;3H\x1bM", 1),
        ];
        for partial_width in [false, true] {
            for &(command, delta) in commands {
                for kind in [
                    SelectionType::Simple,
                    SelectionType::Semantic,
                    SelectionType::Block,
                    SelectionType::Lines,
                ] {
                    for reverse in [false, true] {
                        let mut term = Terminal::new(8, 5);
                        term.process(
                            b"00aaaa00\r\n11bbbb11\r\n22cccc22\r\n33dddd33\r\n44eeee44\x1b[2;4r",
                        );
                        if partial_width {
                            term.process(b"\x1b[?69h\x1b[3;6s");
                        }
                        let (start, end) = if reverse { (5, 2) } else { (2, 5) };
                        let mut selection = Selection::new(Point::new(2, start), kind);
                        selection.update(Point::new(2, end));
                        selection.finish();
                        term.set_selection(Some(selection));
                        let text = term.get_selection_text();
                        term.process(command);
                        if partial_width && kind == SelectionType::Lines {
                            assert!(term.selection().is_none()); // A whole line splits at side margins.
                        } else {
                            let selection = term.selection().unwrap();
                            assert_eq!(selection.start, Point::new(2 + delta, start));
                            assert_eq!(selection.end, Point::new(2 + delta, end));
                            assert!(!selection.active);
                            assert_eq!(term.get_selection_text(), text);
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn region_selection_preserves_outside_cells_and_rejects_erasure_or_mixed_motion() {
        for (start, end, keep) in [
            (Point::new(2, 0), Point::new(2, 1), true),
            (Point::new(0, 2), Point::new(0, 5), true),
            (Point::new(1, 2), Point::new(1, 5), false),
            (Point::new(2, 1), Point::new(2, 5), false),
            (Point::new(0, 2), Point::new(2, 5), false),
        ] {
            let mut term = Terminal::new(8, 5);
            term.process(b"00aaaa00\r\n11bbbb11\r\n22cccc22\r\n33dddd33\r\n44eeee44\x1b[2;4r\x1b[?69h\x1b[3;6s");
            let mut selection = Selection::new(start, SelectionType::Simple);
            selection.update(end);
            term.set_selection(Some(selection));
            let text = term.get_selection_text();
            term.process(b"\x1b[S");
            assert_eq!(term.selection().is_some(), keep, "{start:?} {end:?}");
            if keep {
                assert_eq!(term.get_selection_text(), text);
            }
        }
        let mut term = Terminal::new(8, 5);
        term.process(b"zero\r\none\r\ntwo\r\nthree\r\nfour");
        let mut selection = Selection::new(Point::new(2, 0), SelectionType::Simple);
        selection.update(Point::new(2, 2));
        term.set_selection(Some(selection));
        term.process(b"\x1b[2;4r\x1b[?2026h\x1b[S");
        assert_eq!(term.selection().unwrap().start.row, 2);
        assert_eq!(term.get_selection_text().as_deref(), Some("two"));
        term.process(b"\x1b[?2026l");
        assert_eq!(term.selection().unwrap().start.row, 1);
        term.process(b"\x1b[99S");
        assert!(term.selection().is_none());
    }

    #[test]
    fn clearing_history_retires_only_selections_with_removed_backing_rows() {
        let mut term = Terminal::new(12, 3);
        term.process(b"history\r\nvisible\r\ntail\r\nlast");
        let mut selection = Selection::new(Point::new(-1, 0), SelectionType::Simple);
        selection.update(Point::new(0, 6));
        term.set_selection(Some(selection));
        term.process(b"\x1b[3J");
        assert_eq!(term.grid().scrollback_len(), 0);
        assert!(term.selection().is_none());

        let mut selection = Selection::new(Point::new(0, 0), SelectionType::Simple);
        selection.update(Point::new(0, 6));
        term.set_selection(Some(selection));
        term.process(b"\x1b[3J");
        assert_eq!(term.get_selection_text().as_deref(), Some("visible"));

        // Removing hidden history must not remove a frozen visible selection.
        term.process(b"\x1b[?2026h\r\nnew\r\nnew\r\nnew\x1b[3J");
        assert_eq!(term.get_selection_text().as_deref(), Some("visible"));
    }

    #[test]
    fn byte_eviction_clears_selection_that_no_longer_has_backing_rows() {
        let mut term = Terminal::new(20, 2);
        term.set_scrollback_limits(10_000, 512);
        term.set_selection(Some(Selection::new(
            Point::new(0, 0),
            SelectionType::Simple,
        )));
        for line in 0..100 {
            term.process(format!("line-{line:03}\r\n").as_bytes());
        }
        assert!(term.selection().is_none());
        assert!(term.memory_usage().scrollback_bytes <= 512);
    }

    #[test]
    fn alternate_screen_is_viewport_only_and_cannot_double_the_history_budget() {
        let mut term = Terminal::new(24, 2);
        term.enter_alt_screen();
        for line in 0..50 {
            term.process(format!("default-alt-{line:03}\r\n").as_bytes());
        }
        assert_eq!(term.grid().scrollback_len(), 0);
        term.exit_alt_screen();

        term.set_scrollback_limits(10_000, 8 * 1024);
        for line in 0..100 {
            term.process(format!("primary-{line:03}\r\n").as_bytes());
        }
        let primary = term.memory_usage();
        assert!(primary.scrollback_lines > 0);
        assert!(primary.scrollback_bytes <= 8 * 1024);

        term.enter_alt_screen();
        for line in 0..500 {
            term.process(
                format!("\x1b[3{}malt-{line:03}-界-e\u{301}\x1b[0m\r\n", line % 8).as_bytes(),
            );
        }
        assert_eq!(term.grid().scrollback_len(), 0);
        let with_alternate_output = term.memory_usage();
        assert_eq!(
            with_alternate_output.scrollback_bytes,
            primary.scrollback_bytes
        );
        assert_eq!(
            with_alternate_output.scrollback_lines,
            primary.scrollback_lines
        );

        // Runtime reductions remain authoritative even while the primary
        // screen is inactive, and alternate output cannot consume the share.
        term.set_scrollback_limits(10_000, 512);
        let reduced = term.memory_usage();
        assert!(reduced.scrollback_bytes <= 512);
        assert_eq!(term.grid().scrollback_len(), 0);

        term.exit_alt_screen();
        assert!(term.grid().scrollback_len() > 0);
        assert!(term.memory_usage().scrollback_bytes <= 512);
    }

    #[test]
    fn hyperlink_registry_is_bounded_and_pruned_with_history() {
        let mut term = Terminal::new(20, 2);
        term.set_scrollback_limits(10_000, 4 * 1024);
        for line in 0..700 {
            term.set_hyperlink(
                format!("https://example.com/{line}/{}", "x".repeat(1024)),
                None,
            );
            term.process(b"x");
            term.reset_hyperlink();
            term.process(b"\r\n");
        }
        let usage = term.memory_usage();
        assert!(usage.scrollback_bytes <= 4 * 1024);
        assert!(usage.hyperlink_bytes < 1024 * 1024);
        assert!(term.hyperlink_registry().len() < 256);
    }

    #[test]
    fn dynamic_colors_are_terminal_local_resettable_and_generation_checked() {
        use crate::event::{DynamicColorOperation, DynamicColorTarget};
        let mut first = Terminal::new(10, 2);
        let second = Terminal::new(10, 2);
        let color = Color::rgb(12, 34, 56);
        first.apply_dynamic_color(DynamicColorOperation::Set {
            target: DynamicColorTarget::Palette(42),
            color,
        });
        assert_eq!(first.dynamic_colors().palette(42), Some(color));
        assert_eq!(second.dynamic_colors().palette(42), None);
        let generation = first.dynamic_colors().generation();
        first.apply_dynamic_color(DynamicColorOperation::Set {
            target: DynamicColorTarget::Palette(42),
            color,
        });
        assert_eq!(first.dynamic_colors().generation(), generation);
        first.apply_dynamic_color(DynamicColorOperation::Reset(DynamicColorTarget::Palette(
            42,
        )));
        assert_eq!(first.dynamic_colors().palette(42), None);
        assert!(first.dynamic_colors().generation() > generation);
    }

    #[test]
    fn synchronized_output_retains_presented_hyperlinks_until_release() {
        let mut term = Terminal::new(30, 2);
        term.process(b"\x1b]8;;https://visible.test\x1b\\old\x1b]8;;\x1b\\");
        let visible_id = term.grid().cell(0, 0).unwrap().hyperlink_id.unwrap();
        term.process(b"\x1b[?2026h\x1b[H\x1b]8;;https://hidden.test\x1b\\new\x1b]8;;\x1b\\");
        term.set_scrollback_limits(0, 0);
        assert_eq!(
            term.get_hyperlink_url(visible_id),
            Some("https://visible.test")
        );
        assert_eq!(
            term.presentation_hyperlink_at(0, 0),
            Some("https://visible.test")
        );
        term.process(b"\x1b[?2026l");
        term.set_scrollback_limits(0, 0);
        assert_eq!(term.get_hyperlink_url(visible_id), None);
        assert_eq!(
            term.presentation_hyperlink_at(0, 0),
            Some("https://hidden.test")
        );
    }

    #[test]
    fn synchronized_hyperlink_hit_testing_keeps_scrolled_screen_identity_until_timeout() {
        let mut term = Terminal::new(12, 2);
        term.process(b"\x1b]8;;https://history.test\x1b\\old\x1b]8;;\x1b\\\r\none\r\n\x1b]8;;https://offscreen.test\x1b\\two\x1b]8;;\x1b\\");
        term.grid_mut().set_scroll_offset(1);
        assert_eq!(term.presentation_hyperlink_at(2, 0), None);
        assert_eq!(
            term.presentation_hyperlink_at(0, 0),
            Some("https://history.test")
        );
        term.process(b"\x1b[?2026h");
        term.grid_mut().set_scroll_offset(0);
        term.process(b"\x1b[?1049h\x1b]8;;https://alternate.test\x1b\\new\x1b]8;;\x1b\\");
        assert_eq!(
            term.presentation_hyperlink_at(0, 0),
            Some("https://history.test")
        );
        assert_eq!(term.presentation_hyperlink_at(2, 0), None);
        assert_eq!(term.presentation_hyperlink_at(0, 12), None);
        assert!(term.expire_synchronized_output(term.synchronized_output_deadline().unwrap()));
        assert_eq!(
            term.presentation_hyperlink_at(0, 0),
            Some("https://alternate.test")
        );
    }

    #[test]
    fn synchronized_output_freezes_text_colors_and_images_at_the_mode_boundary() {
        use crate::event::{DynamicColorOperation, DynamicColorTarget};

        let mut term = Terminal::new(12, 3);
        term.process(b"old");
        term.apply_dynamic_color(DynamicColorOperation::Set {
            target: DynamicColorTarget::Foreground,
            color: Color::rgb(1, 2, 3),
        });
        term.process(b"\x1b[3 q");
        term.handle_graphics_command(b"a=T,f=32,s=1,v=1,i=7,C=1;AQIDBA==");
        let live_pixels = term.get_image(ImageId(7)).unwrap().data.clone();

        // Output after DECSET 2026 in the same parser call must remain live
        // internally without becoming presentable.
        term.process(b"\x1b[?2026h\rnew");
        term.apply_dynamic_color(DynamicColorOperation::Set {
            target: DynamicColorTarget::Foreground,
            color: Color::rgb(9, 8, 7),
        });
        term.process(b"\x1b[6 q");
        term.handle_graphics_command(b"a=d,d=a");

        assert!(term.is_synchronized_output());
        assert!(term.rows_text(0, 0).starts_with("new"));
        assert_eq!(term.bounded_presentation_text(0, 64).0, "old");
        assert_eq!(term.presentation_grid().row(0).unwrap().to_string(), "old");
        assert_eq!(
            term.presentation_dynamic_colors().foreground(),
            Some(Color::rgb(1, 2, 3))
        );
        assert_eq!(term.graphics_placement_count(), 0);
        assert_eq!(term.cursor_style_parameter(), 6);
        assert_eq!(
            term.presentation_cursor_style()
                .map(TerminalCursorStyle::parameter),
            Some(3)
        );
        assert_eq!(term.presentation_images().len(), 1);
        assert!(std::sync::Arc::ptr_eq(
            &live_pixels,
            &term.presentation_images()[0].image.data
        ));

        term.process(b"\x1b[?2026l");
        assert!(!term.is_synchronized_output());
        assert!(
            term.presentation_grid()
                .row(0)
                .unwrap()
                .to_string()
                .starts_with("new")
        );
        assert_eq!(
            term.presentation_dynamic_colors().foreground(),
            Some(Color::rgb(9, 8, 7))
        );
        assert!(term.presentation_images().is_empty());
        assert_eq!(
            term.presentation_cursor_style()
                .map(TerminalCursorStyle::parameter),
            Some(6)
        );
    }

    #[test]
    fn bounded_presentation_text_is_utf8_safe_and_alternate_screen_is_viewport_only() {
        let mut term = Terminal::new(8, 2);
        term.process(b"old-one\r\nold-two\r\n");
        let (text, history, truncated) = term.bounded_presentation_text(8, 4);
        assert!(std::str::from_utf8(text.as_bytes()).is_ok());
        assert!(text.len() <= 4);
        assert!(truncated);
        assert!(history <= 8);

        term.enter_alt_screen();
        term.process("界".as_bytes());
        let (text, history, _) = term.bounded_presentation_text(8, 4);
        assert_eq!(text, "界");
        assert_eq!(history, 0);
        assert!(term.alternate_screen_active());

        term.exit_alt_screen();
        term.process(b"\x1b[?2026h\x1b[?1049halt");
        assert!(term.alternate_screen_active());
        assert!(!term.presentation_alternate_screen_active());
        assert!(!term.bounded_presentation_text(8, 64).0.contains("alt"));
        term.process(b"\x1b[?2026l");
        assert!(term.presentation_alternate_screen_active());
    }

    #[test]
    fn synchronized_output_repeated_begin_does_not_extend_the_recovery_deadline() {
        let mut term = Terminal::new(8, 2);
        term.process(b"\x1b[?2026h");
        let deadline = term.synchronized_output_deadline().unwrap();

        term.process(b"\x1b[?2026h");
        assert_eq!(term.synchronized_output_deadline(), Some(deadline));
        assert!(!term.expire_synchronized_output(deadline - Duration::from_millis(1)));
        assert!(term.expire_synchronized_output(deadline));
        assert!(!term.is_synchronized_output());
        assert!(term.synchronized_output_deadline().is_none());
    }

    #[test]
    fn dec_special_graphics_maps_the_complete_vt100_repertoire() {
        let mut term = Terminal::new(80, 2);
        term.process(b"\x1b(0_`abcdefghijklmnopqrstuvwxyz{|}~");
        assert_eq!(term.rows_text(0, 0), " ◆▒␉␌␍␊°±␤␋┘┐┌└┼⎺⎻─⎼⎽├┤┴┬│≤≥π≠£·");
    }

    #[test]
    fn g0_g1_shift_save_restore_alt_screen_and_ris_do_not_leak() {
        let mut term = Terminal::new(20, 2);
        term.process(b"q\x1b)0q\x0eq\x0fq\x1b(A#");
        assert_eq!(term.rows_text(0, 0), "qq─q£");

        let mut saved = Terminal::new(20, 2);
        saved.process(b"\x1b(0\x1b7\x1b(Bq\x1b8q");
        assert_eq!(saved.rows_text(0, 0), "─");

        let mut alternate = Terminal::new(20, 2);
        alternate.process(b"\x1b(0\x1b[?1049h\x1b(Bq\x1b[?1049lq");
        assert_eq!(alternate.rows_text(0, 0), "─");

        alternate.process(b"\x1bcq");
        assert_eq!(alternate.rows_text(0, 0), "q");
    }

    #[test]
    fn g2_g3_single_and_locking_shifts_are_one_shot_saved_and_resettable() {
        let mut term = Terminal::new(20, 2);
        term.process(b"\x1b*0\x1b+A\x1b(B\x1bNqq\x1bO##\x1bnq\x1bo#");
        assert_eq!(term.rows_text(0, 0), "─q£#─£");

        let mut saved = Terminal::new(10, 2);
        saved.process(b"\x1b*0\x1b+A\x1bn\x1bO\x1b7\x1b*B\x1b(B\x1b8#q");
        assert_eq!(saved.rows_text(0, 0), "£─");

        saved.process(b"\x1bc\x1bNq");
        assert_eq!(saved.rows_text(0, 0), "q");
    }

    #[test]
    fn presentation_state_reports_are_byte_exact_bounded_and_strict() {
        let mut term = Terminal::new(12, 4);
        term.process(
            b"\x1b(A\x1b)0\x1b*A\x1b+0\x1bo\x1b~\x1b[1;2;3;4:3;6;7m\x1b[1\"q\x1b[?6h\x1b[4;12HX\x1bN\x1b[1$w",
        );
        assert_eq!(
            term.drain_events(),
            vec![TerminalEvent::PtyWrite(
                b"\x1bP1$u4;12;1;O;A;K;3;1;@;A0A0\x1b\\".to_vec()
            )]
        );

        term.process(b"\x1b[3g\x1b[1;3H\x1bH\x1b[1;7H\x1bH\x1b[2$w");
        assert_eq!(
            term.drain_events(),
            vec![TerminalEvent::PtyWrite(b"\x1bP2$u3/7\x1b\\".to_vec())]
        );

        // Zero/unknown reports, plural/subparameter forms, and unrelated
        // intermediates are ignored without a misleading response.
        term.process(b"\x1b[$w\x1b[0$w\x1b[3$w\x1b[1;2$w\x1b[1:0$w\x1b[1*w");
        assert!(term.drain_events().is_empty());
    }

    #[test]
    fn presentation_state_restore_round_trips_and_rejects_atomically() {
        let mut source = Terminal::new(12, 4);
        source.process(
            b"\x1b(A\x1b)0\x1b*A\x1b+0\x1bo\x1b~\x1b[1;4:3;6;7m\x1b[1\"q\x1b[?6h\x1b[4;12HX\x1bN\x1b[1$w",
        );
        let mut events = source.drain_events();
        let Some(TerminalEvent::PtyWrite(mut cursor_report)) = events.pop() else {
            panic!("expected one cursor information report");
        };
        assert!(events.is_empty());
        cursor_report[4] = b't';

        source.process(b"\x1b[3g\x1b[1;3H\x1bH\x1b[1;7H\x1bH\x1b[2$w");
        let mut events = source.drain_events();
        let Some(TerminalEvent::PtyWrite(mut tab_report)) = events.pop() else {
            panic!("expected one tab stop report");
        };
        assert!(events.is_empty());
        tab_report[4] = b't';

        let mut restored = Terminal::new(12, 4);
        restored.process(b"\x1b[8;9m");
        restored.process(&cursor_report);
        restored.process(&tab_report);
        assert!(restored.current_attrs.contains(CellAttrs::HIDDEN));
        assert!(restored.current_attrs.contains(CellAttrs::STRIKE));

        restored.process(b"\x1b[1$w\x1b[2$w");
        assert_eq!(
            restored.drain_events(),
            vec![
                TerminalEvent::PtyWrite(b"\x1bP1$u4;12;1;O;A;K;3;1;@;A0A0\x1b\\".to_vec()),
                TerminalEvent::PtyWrite(b"\x1bP2$u3/7\x1b\\".to_vec()),
            ]
        );

        // Invalid cursor coordinates/designators and malformed tab lists are
        // all-or-nothing and cannot partially replace live state.
        restored.process(
            b"\x1bP1$t99;12;1;O;A;K;3;1;@;A0A0\x1b\\\x1bP1$t4;12;1;O;A;K;3;1;@;A0AZ\x1b\\\x1bP1$t4;12;1;O;A;F;3;1;@;A0A0\x1b\\\x1bP2$t3//7\x1b\\",
        );
        restored.restore_presentation_state(1, &[b'1'; 129]);
        restored.restore_presentation_state(2, &[b'1'; 12 * 3 + 1]);
        restored.process(b"\x1b[1$w\x1b[2$w");
        assert_eq!(
            restored.drain_events(),
            vec![
                TerminalEvent::PtyWrite(b"\x1bP1$u4;12;1;O;A;K;3;1;@;A0A0\x1b\\".to_vec()),
                TerminalEvent::PtyWrite(b"\x1bP2$u3/7\x1b\\".to_vec()),
            ]
        );
    }

    #[test]
    fn dec_saved_cursor_restores_origin_delayed_wrap_and_screen_local_state() {
        let mut term = Terminal::new(4, 4);
        term.process(b"\x1b[2;4r\x1b[?6h\x1b[2;4HX\x1b7");
        assert_eq!((term.grid().cursor().row, term.grid().cursor().col), (2, 3));
        assert!(term.grid().cursor().wrap_pending);

        term.process(b"\x1b[?6l\x1b[HY\x1b8");
        assert!(term.mode().contains(TerminalMode::ORIGIN));
        assert_eq!((term.grid().cursor().row, term.grid().cursor().col), (2, 3));
        assert!(term.grid().cursor().wrap_pending);

        // Restored pending wrap is observable on the next graphic, and the
        // saved state remains available for repeated DECRC operations.
        term.process(b"Z");
        assert_eq!(term.grid().cell(3, 0).unwrap().content.char(), Some('Z'));
        assert_eq!((term.grid().cursor().row, term.grid().cursor().col), (3, 1));
        term.process(b"\x1b8");
        assert_eq!((term.grid().cursor().row, term.grid().cursor().col), (2, 3));
        assert!(term.grid().cursor().wrap_pending);

        // DECAWM is a live mode, not part of the saved cursor. Changing it
        // after DECSC remains authoritative after DECRC.
        term.process(b"\x1b[?7l\x1b7\x1b[?7h\x1b8");
        assert!(term.mode().contains(TerminalMode::AUTOWRAP));

        // Primary and alternate screens retain independent saved states.
        let mut screens = Terminal::new(6, 4);
        screens.process(b"\x1b[1;2H\x1b7\x1b[?47h\x1b[2;4r\x1b[?6h\x1b[2;5H\x1b7");
        screens.process(b"\x1b[?47l\x1b[H\x1b8");
        assert!(!screens.mode().contains(TerminalMode::ORIGIN));
        assert_eq!(
            (screens.grid().cursor().row, screens.grid().cursor().col),
            (0, 1)
        );
        screens.process(b"\x1b[?47h\x1b[H\x1b8");
        assert!(screens.mode().contains(TerminalMode::ORIGIN));
        assert_eq!(
            (screens.grid().cursor().row, screens.grid().cursor().col),
            (2, 4)
        );

        // With no prior DECSC, DECRC restores the defined home/default state.
        let mut unsaved = Terminal::new(6, 4);
        unsaved.process(b"\x1b[31m\x1b[1\"q\x1b[?6h\x1b[3;3H\x1b8Q");
        assert!(!unsaved.mode().contains(TerminalMode::ORIGIN));
        assert_eq!(
            (unsaved.grid().cursor().row, unsaved.grid().cursor().col),
            (0, 1)
        );
        let cell = unsaved.grid().cell(0, 0).unwrap();
        assert_eq!(cell.content.char(), Some('Q'));
        assert_eq!(cell.fg_spec, ColorSpec::Default);
        assert!(!cell.flags.contains(CellFlags::PROTECTED));

        // A saved absolute position remains memory-safe across a resize and
        // is clamped by the destination grid while retaining delayed wrap.
        let mut resized = Terminal::new(8, 4);
        resized.process(b"\x1b[4;8HX\x1b7");
        resized.resize(3, 2);
        resized.process(b"\x1b8");
        assert_eq!(
            (resized.grid().cursor().row, resized.grid().cursor().col),
            (1, 2)
        );
        assert!(resized.grid().cursor().wrap_pending);
    }

    #[test]
    fn kitty_apc_survives_arbitrary_process_boundaries() {
        let mut term = Terminal::new(8, 4);
        term.process(b"\x1b_Ga=t,f=32,s=1,");
        term.process(b"v=1,i=44;AQ");
        term.process(b"IDBA==\x1b");
        term.process(b"\\");

        assert_eq!(
            term.get_image(ImageId(44)).unwrap().data.as_ref(),
            [1, 2, 3, 4]
        );
        assert!(term.drain_events().iter().any(|event| matches!(
            event,
            TerminalEvent::PtyWrite(bytes) if bytes == b"\x1b_Gi=44;OK\x1b\\"
        )));
    }

    #[test]
    fn kitty_unicode_placeholders_follow_grid_text_and_exact_color_ids() {
        let mut term = Terminal::new(4, 3);
        term.set_pixel_geometry(4, 3);
        let cursor = term.grid().cursor();
        term.handle_graphics_command(
            b"a=T,U=1,i=42,p=7,c=2,r=2,f=32,s=2,v=2;AQIDBAUGBwgJCgsMDQ4PEA==",
        );
        assert_eq!(term.grid().cursor().row, cursor.row);
        assert_eq!(term.grid().cursor().col, cursor.col);
        assert_eq!(term.graphics_placements().count(), 0);

        term.process(
            "\x1b[38;5;42;58;5;7m\u{10eeee}\u{0305}\u{0305}\u{10eeee}\r\n\u{10eeee}\u{030d}\u{10eeee}\x1b[0m"
                .as_bytes(),
        );
        let first_cell = term.grid().cell(0, 0).unwrap();
        assert_eq!(first_cell.fg_spec, ColorSpec::Indexed(42));
        assert_eq!(first_cell.underline_color_spec, Some(ColorSpec::Indexed(7)));

        let presentations = term.presentation_images();
        assert_eq!(presentations.len(), 2);
        let positions: Vec<_> = presentations
            .iter()
            .map(|presentation| {
                (
                    presentation.placement.col,
                    presentation.placement.row,
                    presentation.source_uv,
                )
            })
            .collect();
        assert_eq!(
            positions,
            vec![
                (0, 0, Some([0.0, 0.0, 1.0, 0.5])),
                (0, 1, Some([0.0, 0.5, 1.0, 1.0])),
            ]
        );

        // The displayed instances are derived from the underlying cells, not
        // protocol placements, so editing text removes only that tile.
        term.set_cursor_position(1, 1);
        term.process(b"X");
        assert_eq!(term.presentation_images().len(), 2);
        term.handle_graphics_command(b"a=d,d=a");
        assert_eq!(term.presentation_images().len(), 2);
        term.handle_graphics_command(b"a=d,d=i,i=42,p=7");
        assert!(term.presentation_images().is_empty());
        assert!(term.get_image(ImageId(42)).is_some());
    }

    #[test]
    fn kitty_unicode_placeholders_follow_scrollback_and_screen_ownership() {
        let mut term = Terminal::new(3, 2);
        term.set_pixel_geometry(3, 2);
        term.handle_graphics_command(b"a=T,U=1,i=9,c=1,r=1,f=32,s=1,v=1;AQIDBA==");
        term.process("\x1b[38;5;9m\u{10eeee}\x1b[0m\r\none\r\ntwo".as_bytes());
        term.grid_mut().set_scroll_offset(1);
        let history = term.presentation_images();
        assert_eq!(history.len(), 1);
        assert_eq!(history[0].placement.row, -1);

        term.grid_mut().set_scroll_offset(0);
        term.process(b"\x1b[?1049h");
        term.process("\x1b[38;5;9m\u{10eeee}\x1b[0m".as_bytes());
        assert!(term.presentation_images().is_empty());
        term.process(b"\x1b[?1049l");
        term.grid_mut().set_scroll_offset(1);
        assert_eq!(term.presentation_images().len(), 1);

        term.grid_mut().set_scroll_offset(0);
        term.process(b"\x1b[?1049h");
        term.handle_graphics_command(b"a=T,U=1,i=10,c=1,r=1,f=32,s=1,v=1;AQIDBA==");
        term.process("\x1b[38;5;10m\u{10eeee}\x1b[0m".as_bytes());
        assert_eq!(term.presentation_images().len(), 1);
        term.process(b"\x1b[?1049l\x1b[?1049h");
        term.process("\x1b[38;5;10m\u{10eeee}\x1b[0m".as_bytes());
        assert!(term.presentation_images().is_empty());
    }

    #[test]
    fn kitty_animation_revisions_reach_presentation_without_leaking_through_sync() {
        let mut term = Terminal::new(4, 2);
        term.handle_graphics_command(b"a=T,f=32,s=1,v=1,i=71,c=1,r=1,C=1;AQIDBA==");
        term.handle_graphics_command(b"a=f,f=32,s=1,v=1,i=71,z=10;BQYHCA==");
        term.handle_graphics_command(b"a=a,i=71,r=1,z=10,s=3");

        let root = term.presentation_images()[0].image.clone();
        assert_eq!(root.data.as_ref(), &[1, 2, 3, 4]);
        let deadline = term.graphics_animation_deadline().unwrap();
        term.process(b"\x1b[?2026h");
        assert!(term.advance_graphics_animations(deadline));
        let frozen = term.presentation_images()[0].image.clone();
        assert_eq!(frozen.data.as_ref(), &[1, 2, 3, 4]);
        assert_eq!(frozen.revision, root.revision);

        term.process(b"\x1b[?2026l");
        let animated = term.presentation_images()[0].image.clone();
        assert_eq!(animated.data.as_ref(), &[5, 6, 7, 8]);
        assert_ne!(animated.revision, root.revision);
    }

    #[test]
    fn live_domain_graphics_relay_restores_decoded_namespace_once_without_replies() {
        let pixels = STANDARD.encode([
            255, 0, 0, 255, 0, 255, 0, 255, 0, 0, 255, 255, 255, 255, 0, 255,
        ]);
        let alternate = STANDARD.encode([
            0, 0, 0, 0, 32, 32, 32, 128, 64, 64, 64, 255, 96, 96, 96, 255,
        ]);
        let mut source = Terminal::new(8, 3);
        source.process(format!("\x1b_Ga=t,f=32,s=2,v=2,I=44;{pixels}\x1b\\").as_bytes());
        source.process(b"\x1b[2;3H\x1b_Ga=p,I=44,p=5,c=2,r=2,C=1\x1b\\");
        source.process(format!("\x1b_Ga=f,f=32,s=2,v=2,i=1,r=2,z=80;{alternate}\x1b\\").as_bytes());
        source.process(b"\x1b_Ga=a,i=1,s=1,c=2\x1b\\");
        source.drain_events();
        let expected = source.presentation_images();
        assert_eq!(expected.len(), 1);
        assert_eq!(
            expected[0].image.data.as_ref(),
            STANDARD.decode(alternate).unwrap()
        );

        let snapshot = source.graphics_relay_snapshot(Instant::now());
        let mut chunks = Vec::new();
        snapshot
            .for_each_escape_chunk(|chunk| {
                chunks.push(chunk);
                true
            })
            .unwrap();
        assert!(chunks.iter().all(|chunk| chunk.len() < 64 * 1024));

        let mut replica = Terminal::new(8, 3);
        replica.set_graphics_relay_snapshot_allowed(true);
        for chunk in &chunks {
            for fragment in chunk.as_bytes().chunks(7) {
                replica.process(fragment);
            }
        }
        let restored = replica.presentation_images();
        assert_eq!(restored.len(), expected.len());
        assert_eq!(
            restored[0].placement.image_id,
            expected[0].placement.image_id
        );
        assert_eq!(restored[0].placement.placement_id, Some(5));
        assert_eq!(
            (restored[0].placement.col, restored[0].placement.row),
            (2, 1)
        );
        assert_eq!(restored[0].image.data, expected[0].image.data);
        assert_eq!(restored[0].image.revision, expected[0].image.revision);
        assert!(replica.drain_events().is_empty());

        // Image-number aliases remain usable by later live PTY output.
        replica.process(b"\x1b[1;1H\x1b_Ga=p,I=44,p=6,C=1\x1b\\");
        assert_eq!(replica.presentation_images().len(), 2);

        // The internal import permission consumed itself at commit. A later
        // application-emitted begin marker cannot clear the restored state.
        replica.process(b"\x1b_TG;begin;0;cbf29ce484222325\x1b\\");
        assert_eq!(replica.presentation_images().len(), 2);

        let mut ordinary = Terminal::new(8, 3);
        for chunk in &chunks {
            ordinary.process(chunk.as_bytes());
        }
        // Standard quiet Kitty pixel loads are harmless, but the private
        // placement/counter commit is ignored outside an opted-in relay pane.
        assert!(ordinary.presentation_images().is_empty());
        assert!(ordinary.drain_events().is_empty());

        let mut rejected = Terminal::new(8, 3);
        rejected.set_graphics_relay_snapshot_allowed(true);
        rejected.process(b"\x1b_TG;begin;1;0000000000000000\x1b\\");
        rejected.process(b"\x1b_TG;chunk;%%%\x1b\\");
        for chunk in &chunks {
            rejected.process(chunk.as_bytes());
        }
        assert!(rejected.presentation_images().is_empty());
        assert!(rejected.drain_events().is_empty());
    }

    #[test]
    fn live_domain_graphics_relay_restores_virtual_and_relative_placement_identity() {
        const PLACEHOLDERS: &str = "\x1b[38;5;42;58;5;7m\u{10eeee}\u{0305}\u{0305}\u{10eeee}\r\n\u{10eeee}\u{030d}\u{10eeee}\x1b[0m";
        let mut source = Terminal::new(6, 3);
        source.set_pixel_geometry(6, 3);
        source.handle_graphics_command(
            b"a=T,U=1,i=42,p=7,c=2,r=2,f=32,s=2,v=2;AQIDBAUGBwgJCgsMDQ4PEA==",
        );
        source.process(PLACEHOLDERS.as_bytes());
        source.process(b"\x1b[2;3H\x1b_Ga=p,i=42,p=8,c=2,r=2,C=1\x1b\\");
        source.process(b"\x1b_Ga=p,i=42,p=9,P=42,Q=8,H=-1,V=1,c=2,r=2,C=1\x1b\\");
        source.drain_events();
        let expected = source.presentation_images();
        assert_eq!(expected.len(), 4);

        let snapshot = source.graphics_relay_snapshot(Instant::now());
        let mut replica = Terminal::new(6, 3);
        replica.set_pixel_geometry(6, 3);
        replica.process(PLACEHOLDERS.as_bytes());
        replica.set_graphics_relay_snapshot_allowed(true);
        snapshot
            .for_each_escape_chunk(|chunk| {
                replica.process(chunk.as_bytes());
                true
            })
            .unwrap();
        let restored = replica.presentation_images();
        assert_eq!(restored.len(), expected.len());
        for (restored, expected) in restored.iter().zip(&expected) {
            assert_eq!(restored.placement.image_id, expected.placement.image_id);
            assert_eq!(
                restored.placement.placement_id,
                expected.placement.placement_id
            );
            assert_eq!(
                (restored.placement.col, restored.placement.row),
                (expected.placement.col, expected.placement.row)
            );
            assert_eq!(restored.pixel_rect, expected.pixel_rect);
            assert_eq!(restored.source_uv, expected.source_uv);
        }

        // Deleting the restored named parent still cascades to its relative
        // child, proving that stable internal placement UIDs survived.
        replica.handle_graphics_command(b"a=d,d=i,i=42,p=8");
        assert_eq!(replica.presentation_images().len(), 2);
    }

    #[test]
    fn kitty_unicode_placeholders_follow_reflow_and_synchronized_output() {
        let mut term = Terminal::new(4, 2);
        term.set_pixel_geometry(4, 2);
        term.handle_graphics_command(b"a=T,U=1,i=5,c=1,r=1,f=32,s=1,v=1;AQIDBA==");
        term.process("AB\x1b[38;5;5m\u{10eeee}\x1b[0m".as_bytes());
        assert_eq!(
            (
                term.presentation_images()[0].placement.col,
                term.presentation_images()[0].placement.row,
            ),
            (2, 0)
        );

        term.resize(2, 3);
        let reflowed = term.presentation_images();
        assert_eq!(reflowed.len(), 1);
        assert_eq!(
            (reflowed[0].placement.col, reflowed[0].placement.row),
            (0, 1)
        );

        term.process(b"\x1b[?2026h");
        term.process(b"\x1b[2;1HX");
        assert_eq!(term.presentation_images().len(), 1);
        term.process(b"\x1b[?2026l");
        assert!(term.presentation_images().is_empty());
    }

    #[test]
    fn kitty_relative_placements_follow_unicode_parent_text_and_lifetime() {
        let mut term = Terminal::new(10, 6);
        term.handle_graphics_command(b"a=T,U=1,i=42,p=7,c=1,r=1,f=32,s=1,v=1;AQIDBA==");
        term.handle_graphics_command(b"a=t,i=8,f=32,s=1,v=1;BQYHCA==");
        let cursor_before = term.grid().cursor();
        term.handle_graphics_command(b"a=p,i=8,p=8,P=42,Q=7,H=1,V=2,c=1,r=1");
        assert_eq!(
            (term.grid().cursor().col, term.grid().cursor().row),
            (cursor_before.col, cursor_before.row)
        );
        assert!(term.presentation_images().is_empty());

        term.process(b"\x1b[4;5H\x1b[38;5;42;58;5;7m");
        term.process("\u{10eeee}\u{0305}\u{0305}".as_bytes());
        let positions = |term: &Terminal| {
            term.presentation_images()
                .into_iter()
                .map(|image| {
                    (
                        image.placement.image_id,
                        image.placement.col,
                        image.placement.row,
                    )
                })
                .collect::<HashSet<_>>()
        };
        assert_eq!(
            positions(&term),
            HashSet::from([(ImageId(42), 4, 3), (ImageId(8), 5, 5)])
        );

        // The virtual parent's origin is the independent minimum x/y across
        // all live placeholder cells that refer to its prototype.
        term.process(b"\x1b[5;3H");
        term.process("\u{10eeee}\u{0305}\u{0305}".as_bytes());
        assert!(positions(&term).contains(&(ImageId(8), 3, 5)));

        term.handle_graphics_command(b"a=d,d=i,i=42,p=7");
        assert!(term.presentation_images().is_empty());
        assert!(term.get_image(ImageId(8)).is_none());
    }

    #[test]
    fn kitty_image_numbers_allocate_reply_and_place_at_the_terminal_cursor() {
        let mut term = Terminal::new(8, 4);
        term.set_cursor_position(3, 2);
        term.handle_graphics_command(b"a=T,I=13,N=1,f=32,s=1,v=1,p=7,c=1,r=1,C=1;AQIDBA==");

        let placement = term.graphics_placements().next().unwrap();
        assert_eq!(placement.image_id, ImageId(1));
        assert_eq!(placement.placement_id, Some(7));
        assert_eq!((placement.col, placement.row), (1, 2));
        assert_eq!(
            term.get_image(ImageId(1)).unwrap().data.as_ref(),
            [1, 2, 3, 4]
        );
        assert!(term.drain_events().iter().any(|event| matches!(
            event,
            TerminalEvent::PtyWrite(bytes)
                if bytes == b"\x1b_Gi=1,I=13,p=7;OK\x1b\\"
        )));
    }

    #[test]
    fn kitty_placements_follow_screen_scroll_clear_and_alternate_lifecycle() {
        let mut term = Terminal::new(8, 4);
        term.set_cursor_position(3, 2);
        term.handle_graphics_command(b"a=T,f=32,s=1,v=1,i=1,c=1,r=1,C=1;AQIDBA==");
        assert_eq!(term.graphics_placements().next().unwrap().row, 2);

        term.scroll_up(1);
        assert_eq!(term.graphics_placements().next().unwrap().row, 1);
        term.enter_alt_screen();
        assert!(term.graphics_placements().next().is_none());
        term.handle_graphics_command(b"a=T,f=32,s=1,v=1,i=2,c=1,r=1,C=1;AQIDBA==");
        assert_eq!(term.graphics_placements().count(), 1);
        term.exit_alt_screen();
        assert_eq!(
            term.graphics_placements().next().unwrap().image_id,
            ImageId(1)
        );
        term.enter_alt_screen();
        assert!(term.graphics_placements().next().is_none());
        term.exit_alt_screen();

        term.erase_in_display(2);
        assert!(term.graphics_placements().next().is_none());
        // ED2 clears visibility but leaves reusable image data intact.
        assert!(term.get_image(ImageId(1)).is_some());
    }

    #[test]
    fn oversized_kitty_apc_is_bounded_and_reports_e2big() {
        let mut term = Terminal::new(8, 4);
        let mut sequence = b"\x1b_Ga=t,i=3;".to_vec();
        sequence.extend(std::iter::repeat_n(b'A', MAX_KITTY_APC_BYTES + 1));
        sequence.extend_from_slice(b"\x1b\\");
        term.process(&sequence);
        assert!(term.drain_events().iter().any(|event| matches!(
            event,
            TerminalEvent::PtyWrite(bytes) if bytes.starts_with(b"\x1b_Gi=0;E2BIG:")
        )));
        assert!(term.get_image(ImageId(3)).is_none());
    }

    #[test]
    fn current_kitty_128_kib_chunks_cross_apc_framing_without_replies() {
        let mut term = Terminal::new(120, 40);
        let mut first = b"\x1b_Ga=T,q=2,f=32,s=256,v=192,m=1,U=1,c=103,r=52,i=77;".to_vec();
        first.extend(std::iter::repeat_n(b'A', 128 * 1024));
        first.extend_from_slice(b"\x1b\\");
        term.process(&first);

        let mut final_chunk = b"\x1b_Gq=2,m=0;".to_vec();
        final_chunk.extend(std::iter::repeat_n(b'A', 128 * 1024));
        final_chunk.extend_from_slice(b"\x1b\\");
        term.process(&final_chunk);

        assert_eq!(
            term.get_image(ImageId(77)).unwrap().data.len(),
            256 * 192 * 4
        );
        assert!(term.drain_events().is_empty());
    }

    #[test]
    fn current_kitty_natural_placement_uses_image_pixels_and_moves_the_cursor() {
        let mut term = Terminal::new(100, 38);
        term.set_pixel_geometry(800, 760);
        let pixels = vec![0x7f; 80 * 80 * 4];
        let payload = format!(
            "\x1b_Ga=T,q=2,f=32,s=80,v=80;{}\x1b\\",
            STANDARD.encode(pixels)
        );

        term.process(payload.as_bytes());

        let placement = term.graphics_placements().next().unwrap();
        assert_eq!((placement.width, placement.height), (Some(10), Some(4)));
        assert_eq!(
            (placement.pixel_width, placement.pixel_height),
            (Some(80), Some(80))
        );
        assert_eq!(
            (term.grid().cursor().col, term.grid().cursor().row),
            (10, 4)
        );
        assert_eq!(term.presentation_images().len(), 1);
        assert!(term.drain_events().is_empty());
    }

    #[test]
    fn kitty_q2_suppresses_parse_and_overflow_errors() {
        let mut term = Terminal::new(8, 4);
        term.process(b"\x1b_Ga=T,q=2;%%%\x1b\\");

        let mut oversized = b"\x1b_Ga=t,q=2,i=3;".to_vec();
        oversized.extend(std::iter::repeat_n(b'A', MAX_KITTY_APC_BYTES + 1));
        oversized.extend_from_slice(b"\x1b\\");
        term.process(&oversized);

        assert!(term.drain_events().is_empty());
        assert!(term.get_image(ImageId(3)).is_none());
    }

    #[test]
    fn kitty_placements_remain_addressable_in_primary_scrollback() {
        let mut term = Terminal::new(8, 4);
        term.set_cursor_position(1, 1);
        term.handle_graphics_command(b"a=T,f=32,s=1,v=1,i=6,c=1,r=1,C=1;AQIDBA==");
        term.scroll_up(1);
        let placement = term.graphics_placements().next().unwrap();
        assert_eq!(placement.row, -1);
        assert_eq!(term.grid().scrollback_len(), 1);
        term.grid_mut().set_scroll_offset(1);
        assert_eq!(term.graphics_placements().next().unwrap().row, -1);
    }

    #[test]
    fn sixel_and_iterm2_framing_render_per_terminal_without_pty_leakage() {
        use base64::{Engine as _, engine::general_purpose::STANDARD};

        let mut png_bytes = Vec::new();
        {
            let mut encoder = png::Encoder::new(&mut png_bytes, 2, 1);
            encoder.set_color(png::ColorType::Rgba);
            encoder.set_depth(png::BitDepth::Eight);
            let mut writer = encoder.write_header().unwrap();
            writer
                .write_image_data(&[255, 0, 0, 255, 0, 255, 0, 255])
                .unwrap();
        }
        let encoded = STANDARD.encode(png_bytes);
        let osc =
            format!("\x1b]1337;File=inline=1;width=2;height=1;preserveAspectRatio=0:{encoded}\x07");

        let mut first_pane = Terminal::new(20, 10);
        first_pane.set_pixel_geometry(200, 100);
        first_pane.process(b"\x1b_Ga=T,f=32,s=1,v=1,i=7,C=1;AQIDBA==\x1b\\");
        first_pane.process(b"\x1bP0;1q\"1;1;4;6#1;2;100;0;0!4~\x1b\\");
        first_pane.process(osc.as_bytes());

        assert_eq!(first_pane.graphics_placement_count(), 3);
        assert_eq!(first_pane.graphics_image_count(), 3);
        assert!(first_pane.graphics_memory_usage() <= 64 * 1024 * 1024);
        assert!(first_pane.graphics_placements().any(|placement| {
            placement.pixel_width == Some(4) && placement.pixel_height == Some(6)
        }));
        assert!(first_pane.graphics_placements().any(|placement| {
            placement.pixel_width == Some(20) && placement.pixel_height == Some(10)
        }));

        let second_pane = Terminal::new(20, 10);
        assert_eq!(second_pane.graphics_placement_count(), 0);
        assert_eq!(second_pane.graphics_image_count(), 0);
        assert!(first_pane.drain_events().iter().all(
            |event| matches!(event, TerminalEvent::PtyWrite(bytes) if bytes.starts_with(b"\x1b_G"))
        ));
    }

    #[test]
    fn fragmented_sixel_aspect_controls_display_footprint_and_cursor_advance() {
        let mut term = Terminal::new(20, 10);
        term.set_pixel_geometry(40, 30);
        term.set_cursor_position(2, 4);

        term.process(b"\x1bP7q\"2;");
        assert_eq!(term.graphics_placement_count(), 0);
        term.process(b"1;2;6!2~\x1b");
        assert_eq!(term.graphics_placement_count(), 0);
        term.process(b"\\");

        let placement = term.graphics_placements().next().unwrap();
        assert_eq!((placement.col, placement.row), (3, 1));
        assert_eq!((placement.width, placement.height), (Some(1), Some(4)));
        assert_eq!(
            (placement.pixel_width, placement.pixel_height),
            (Some(2), Some(12))
        );
        let image = term.get_image(placement.image_id).unwrap();
        assert_eq!((image.width, image.height), (2, 6));
        assert_eq!(term.graphics_memory_usage(), 2 * 6 * 4);
        assert_eq!((term.grid().cursor().row, term.grid().cursor().col), (5, 3));
    }

    #[test]
    fn repeated_sixel_images_remain_inside_shared_resource_limits() {
        let mut term = Terminal::new(8, 4);
        term.set_pixel_geometry(80, 40);
        for _ in 0..300 {
            term.process(b"\x1bP0;1q!4~\x1b\\");
        }
        assert!(term.graphics_image_count() <= 256);
        assert!(term.graphics_placement_count() <= 256);
        assert!(term.graphics_memory_usage() <= 64 * 1024 * 1024);

        term.erase_in_display(2);
        assert_eq!(term.graphics_image_count(), 0);
        assert_eq!(term.graphics_memory_usage(), 0);
    }

    #[test]
    fn selective_erase_preserves_protected_unicode_and_cell_style() {
        let mut term = Terminal::new(8, 3);
        term.process(b"\x1b[1\"qA\x1b[0\"qB\x1b[1\"q");
        term.process("界".as_bytes());
        term.process(b"\x1b[2\"qC\x1b[48;2;1;2;3m\x1b[?2K");

        assert_eq!((term.grid().cursor().row, term.grid().cursor().col), (0, 5));
        assert_eq!(term.rows_text(0, 0), "A 界");
        for col in [0, 2, 3] {
            assert!(
                term.grid()
                    .cell(0, col)
                    .unwrap()
                    .flags
                    .contains(CellFlags::PROTECTED)
            );
        }
        assert!(term.grid().cell(0, 2).unwrap().is_wide_char());
        assert!(term.grid().cell(0, 3).unwrap().is_wide_char_spacer());
        for col in [1, 4, 6, 7] {
            let cell = term.grid().cell(0, col).unwrap();
            assert!(cell.content.is_empty());
            assert_eq!(cell.bg_spec, ColorSpec::Default);
            assert!(!cell.flags.contains(CellFlags::PROTECTED));
        }

        // Invalid, plural, and subparameter DECSCA forms are ignored. The
        // following character therefore remains protected.
        term.process(b"\x1b[1\"q\x1b[0m\x1b[9\"q\x1b[1;0\"q\x1b[1:0\"q\x1b7\x1b[0\"q\x1b8D");
        assert!(
            term.grid()
                .cell(0, 5)
                .unwrap()
                .flags
                .contains(CellFlags::PROTECTED)
        );

        // Ordinary EL ignores selective protection, and RIS returns DECSCA
        // to erasable without changing the semantics of SGR.
        term.process(b"\x1b[2K\x1b[1\"q\x1bcE");
        let cell = term.grid().cell(0, 0).unwrap();
        assert_eq!(cell.content.char(), Some('E'));
        assert!(!cell.flags.contains(CellFlags::PROTECTED));

        // Protection is durable cell state, not parser-only state: compact
        // scrollback and subsequent reflow retain it.
        let mut history = Terminal::new(4, 2);
        history.process(b"\x1b[1\"qA\r\n\x1b[0\"qB\r\nC");
        history.resize(2, 2);
        let oldest = -(history.grid().scrollback_len() as i32);
        let retained = (oldest..history.rows() as i32).any(|row| {
            history.grid().row_at_offset(row).is_some_and(|row| {
                row.iter().any(|cell| {
                    cell.content.char() == Some('A') && cell.flags.contains(CellFlags::PROTECTED)
                })
            })
        });
        assert!(retained);
    }

    #[test]
    fn rectangular_fill_and_erase_apply_attributes_clipping_and_origin() {
        let mut term = Terminal::new(8, 4);
        term.process(b"abcdefgh\r\nijklmnop\r\nqrstuvwx\r\nyz012345");
        term.process(b"\x1b[2;7H\x1b[1;38;5;2;48;2;1;2;3m\x1b[1\"q");
        let cursor = term.grid().cursor();
        term.process(b"\x1b[88;2;2;3;4$x");

        assert_eq!(
            (term.grid().cursor().row, term.grid().cursor().col),
            (cursor.row, cursor.col)
        );
        assert_eq!(term.grid().cursor().wrap_pending, cursor.wrap_pending);
        assert_eq!(
            term.rows_text(0, 3),
            "abcdefgh\niXXXmnop\nqXXXuvwx\nyz012345"
        );
        for row in 1..=2 {
            for col in 1..=3 {
                let cell = term.grid().cell(row, col).unwrap();
                assert_eq!(cell.content.char(), Some('X'));
                assert!(cell.attrs.contains(CellAttrs::BOLD));
                assert_eq!(cell.fg_spec, ColorSpec::Indexed(2));
                assert_eq!(cell.bg_spec, ColorSpec::TrueColor(Color::rgb(1, 2, 3)));
                assert!(cell.flags.contains(CellFlags::PROTECTED));
            }
        }

        // Invalid fill characters, reversed rectangles, extra parameters, and
        // subparameters are ignored without moving the cursor.
        let before = term.rows_text(0, 3);
        term.process(b"\x1b[127;1;1;4;8$x\x1b[88;3;3;2;4$x\x1b[88;1;1;2;2;9$x\x1b[88:1;1;1;2;2$x");
        assert_eq!(term.rows_text(0, 3), before);
        assert_eq!(
            (term.grid().cursor().row, term.grid().cursor().col),
            (cursor.row, cursor.col)
        );

        // DECERA ignores protection, applies BCE, and clips oversized bounds.
        term.process(b"\x1b[48;5;5m\x1b[2;3;99;99$z");
        assert_eq!(term.rows_text(0, 3), "abcdefgh\niX\nqX\nyz");
        for row in 1..4 {
            for col in 2..8 {
                let cell = term.grid().cell(row, col).unwrap();
                assert!(cell.content.is_empty());
                assert_eq!(cell.bg_spec, ColorSpec::Indexed(5));
                assert!(!cell.flags.contains(CellFlags::PROTECTED));
            }
        }

        // DECOM makes rectangle coordinates relative to both margin pairs.
        term.process(b"\x1b[0m\x1b[2;4r\x1b[?69h\x1b[3;7s\x1b[?6h\x1b[90;1;1;2;2$x");
        assert_eq!(term.grid().cell(1, 2).unwrap().content.char(), Some('Z'));
        assert_eq!(term.grid().cell(2, 3).unwrap().content.char(), Some('Z'));
        assert_ne!(term.grid().cell(0, 0).unwrap().content.char(), Some('Z'));
        assert_ne!(term.grid().cell(3, 4).unwrap().content.char(), Some('Z'));

        let mut defaulted = Terminal::new(3, 2);
        defaulted.process(b"\x1b(0\x1b[113$x");
        assert_eq!(defaulted.rows_text(0, 1), "───\n───");
        defaulted.process(b"\x1b[$z");
        assert_eq!(defaulted.rows_text(0, 1), "\n");
        defaulted.process(b"\x1b[160;1;1;1;1$x");
        assert_eq!(
            defaulted.grid().cell(0, 0).unwrap().content.char(),
            Some('\u{a0}')
        );
    }

    #[test]
    fn rectangular_selective_erase_retains_cell_attributes_and_wide_integrity() {
        let mut term = Terminal::new(8, 2);
        term.process(b"\x1b[1;31mA\x1b[1\"qB\x1b[0\"q");
        term.process("界".as_bytes());
        term.process(b"C");
        let cursor = term.grid().cursor();
        term.process(b"\x1b[1;1;1;4${");

        assert_eq!(
            (term.grid().cursor().row, term.grid().cursor().col),
            (cursor.row, cursor.col)
        );
        assert_eq!(term.grid().cursor().wrap_pending, cursor.wrap_pending);
        let erased = term.grid().cell(0, 0).unwrap();
        assert!(erased.content.is_empty());
        assert!(erased.attrs.contains(CellAttrs::BOLD));
        assert_eq!(erased.fg_spec, ColorSpec::Indexed(1));
        assert_eq!(term.grid().cell(0, 1).unwrap().content.char(), Some('B'));
        assert!(
            term.grid()
                .cell(0, 1)
                .unwrap()
                .flags
                .contains(CellFlags::PROTECTED)
        );
        for col in 2..=3 {
            let cell = term.grid().cell(0, col).unwrap();
            assert!(cell.content.is_empty());
            assert!(cell.attrs.contains(CellAttrs::BOLD));
            assert!(!cell.is_wide_char());
            assert!(!cell.is_wide_char_spacer());
        }
        assert_eq!(term.grid().cell(0, 4).unwrap().content.char(), Some('C'));
    }

    #[test]
    fn rectangular_copy_is_overlap_safe_clipped_and_copies_complete_state() {
        let mut term = Terminal::new(8, 3);
        term.process(b"\x1b[1;34mAB\x1b[1\"qC\x1b[0\"qDEFGH");
        let cursor = term.grid().cursor();
        term.process(b"\x1b[1;1;1;6;1;1;3;1$v");
        assert_eq!(term.rows_text(0, 0), "ABABCDEF");
        assert_eq!(
            (term.grid().cursor().row, term.grid().cursor().col),
            (cursor.row, cursor.col)
        );
        assert_eq!(term.grid().cursor().wrap_pending, cursor.wrap_pending);
        assert!(
            term.grid()
                .cell(0, 4)
                .unwrap()
                .flags
                .contains(CellFlags::PROTECTED)
        );
        assert_eq!(
            term.grid().cell(0, 2).unwrap().fg_spec,
            ColorSpec::Indexed(4)
        );

        // Reverse-direction overlap must also read from the pre-copy snapshot.
        term.process(b"\x1b[1;3;1;8;9;1;1;7$v");
        assert_eq!(term.rows_text(0, 0), "ABCDEFEF");

        // Destination coordinates beyond the sole page clamp to its last cell;
        // page parameters likewise resolve to the only available page.
        term.process(b"\x1b[1;1;3;8;42;99;99;77$v");
        assert_eq!(term.grid().cell(2, 7).unwrap().content.char(), Some('A'));

        // A source boundary that intersects a wide pair cannot leave an orphan
        // half at the destination or damage the cursor.
        let mut wide = Terminal::new(6, 2);
        wide.process("A界BCD".as_bytes());
        let cursor = wide.grid().cursor();
        wide.process(b"\x1b[1;3;1;5;1;2;2;1$v");
        assert_eq!(
            (wide.grid().cursor().row, wide.grid().cursor().col),
            (cursor.row, cursor.col)
        );
        assert_eq!(wide.grid().cursor().wrap_pending, cursor.wrap_pending);
        for row in 0..wide.rows() {
            for col in 0..wide.cols() {
                let cell = wide.grid().cell(row, col).unwrap();
                if cell.is_wide_char() {
                    assert!(
                        wide.grid()
                            .cell(row, col + 1)
                            .unwrap()
                            .is_wide_char_spacer()
                    );
                }
                if cell.is_wide_char_spacer() {
                    assert!(col > 0 && wide.grid().cell(row, col - 1).unwrap().is_wide_char());
                }
            }
        }
    }

    #[test]
    fn rectangular_text_operations_share_image_placement_geometry() {
        let mut erased = Terminal::new(8, 4);
        erased.process(b"\x1b_Ga=T,f=32,s=1,v=1,i=7,c=4,r=4,C=1;AQIDBA==\x1b\\");
        assert_eq!(erased.graphics_placement_count(), 1);
        erased.process(b"\x1b[88;2;2;3;3$x");
        let fragments: Vec<_> = erased.graphics_placements().collect();
        assert_eq!(fragments.len(), 4);
        for row in 1..3_i64 {
            for col in 1..3_u32 {
                assert!(fragments.iter().all(|placement| {
                    col < placement.col
                        || col >= placement.col + placement.width.unwrap_or(1)
                        || row < placement.row
                        || row >= placement.row + i64::from(placement.height.unwrap_or(1))
                }));
            }
        }

        let mut copied = Terminal::new(8, 4);
        copied.process(b"\x1b_Ga=T,f=32,s=1,v=1,i=8,c=2,r=2,C=1;AQIDBA==\x1b\\");
        copied.process(b"\x1b[1;1;2;2;1;3;5;1$v");
        assert_eq!(copied.graphics_placement_count(), 2);
        assert!(copied.graphics_placements().any(|placement| {
            placement.col == 4
                && placement.row == 2
                && placement.width == Some(2)
                && placement.height == Some(2)
        }));

        // DECSERA is character-selective; image pixels do not carry DECSCA.
        copied.process(b"\x1b[3;5;4;6${");
        assert_eq!(copied.graphics_placement_count(), 2);
    }

    #[test]
    fn attribute_change_extent_obeys_stream_rectangle_origin_and_reset_lifecycle() {
        let mut term = Terminal::new(6, 3);
        term.process(b"AB\x1b[2;3HCD\x1b[3;5HEF");
        let cursor = term.grid().cursor();

        // Stream mode is the default. It wraps over the full page width but
        // changes only cells that have actually been drawn.
        term.process(b"\x1b[1;2;3;5;1$r");
        for (row, col) in [(0, 1), (1, 2), (1, 3), (2, 4)] {
            assert!(
                term.grid()
                    .cell(row, col)
                    .unwrap()
                    .attrs
                    .contains(CellAttrs::BOLD)
            );
        }
        for (row, col) in [(0, 0), (2, 5)] {
            assert!(
                !term
                    .grid()
                    .cell(row, col)
                    .unwrap()
                    .attrs
                    .contains(CellAttrs::BOLD)
            );
        }
        let untouched = term.grid().cell(1, 0).unwrap();
        assert!(untouched.content.is_empty());
        assert!(untouched.attrs.is_empty());

        // Exact rectangle mode changes only the requested columns and turns
        // never-drawn cells into explicit spaces. A later stream operation
        // therefore recognizes those cells as drawn.
        term.process(b"\x1b[2*x\x1b[1;2;3;5;7$r");
        let materialized = term.grid().cell(1, 1).unwrap();
        assert_eq!(materialized.content.char(), Some(' '));
        assert!(materialized.attrs.contains(CellAttrs::INVERSE));
        assert!(term.grid().cell(1, 0).unwrap().content.is_empty());
        term.process(b"\x1b[1*x\x1b[1;2;3;5;8$r");
        assert!(
            term.grid()
                .cell(1, 1)
                .unwrap()
                .attrs
                .contains(CellAttrs::HIDDEN)
        );

        assert_eq!(
            (term.grid().cursor().row, term.grid().cursor().col),
            (cursor.row, cursor.col)
        );
        assert_eq!(term.grid().cursor().wrap_pending, cursor.wrap_pending);

        // DECOM affects both endpoints before stream wrapping. Horizontal
        // margins do not constrain the wrapped portion itself.
        let mut origin = Terminal::new(8, 4);
        origin.process(b"abcdefgh\r\nijklmnop\r\nqrstuvwx\r\nyz012345");
        origin.process(b"\x1b[2;4r\x1b[?69h\x1b[3;7s\x1b[?6h\x1b[1;2;2;3;1$r");
        assert!(
            !origin
                .grid()
                .cell(0, 0)
                .unwrap()
                .attrs
                .contains(CellAttrs::BOLD)
        );
        assert!(
            !origin
                .grid()
                .cell(1, 2)
                .unwrap()
                .attrs
                .contains(CellAttrs::BOLD)
        );
        assert!(
            origin
                .grid()
                .cell(1, 3)
                .unwrap()
                .attrs
                .contains(CellAttrs::BOLD)
        );
        assert!(
            origin
                .grid()
                .cell(1, 7)
                .unwrap()
                .attrs
                .contains(CellAttrs::BOLD)
        );
        assert!(
            origin
                .grid()
                .cell(2, 0)
                .unwrap()
                .attrs
                .contains(CellAttrs::BOLD)
        );
        assert!(
            origin
                .grid()
                .cell(2, 4)
                .unwrap()
                .attrs
                .contains(CellAttrs::BOLD)
        );
        assert!(
            !origin
                .grid()
                .cell(2, 5)
                .unwrap()
                .attrs
                .contains(CellAttrs::BOLD)
        );

        assert_eq!(origin.attribute_change_extent_parameter(), 1);
        origin.process(b"\x1b[2*x\x1b[3*x\x1b[2;1*x\x1b[2:1*x");
        assert_eq!(origin.attribute_change_extent_parameter(), 2);
        origin.process(b"\x1b[!p");
        assert_eq!(origin.attribute_change_extent_parameter(), 2);
        origin.process(b"\x1bc");
        assert_eq!(origin.attribute_change_extent_parameter(), 1);
    }

    #[test]
    fn attribute_change_and_reverse_apply_only_the_documented_rendition_subset() {
        let mut term = Terminal::new(5, 2);
        term.process(
            b"\x1b]8;;https://example.com\x07\x1b[1\"q\x1b[1;2;4:3;6;7;8;31;44mX\x1b]8;;\x07\x1b[0\"q",
        );
        let cursor = term.grid().cursor();
        let original = term.grid().cell(0, 0).unwrap().clone();
        assert!(
            original
                .flags
                .contains(CellFlags::PROTECTED | CellFlags::HYPERLINK)
        );

        term.process(b"\x1b[1;1;1;1;0$r");
        let attrs = term.grid().cell(0, 0).unwrap().attrs;
        assert!(attrs.contains(CellAttrs::DIM | CellAttrs::RAPID_BLINK | CellAttrs::HIDDEN));
        assert!(!attrs.intersects(
            CellAttrs::BOLD | CellAttrs::ALL_UNDERLINES | CellAttrs::BLINK | CellAttrs::INVERSE
        ));

        term.process(b"\x1b[1;1;1;1;1;4;5;7;8$r");
        let attrs = term.grid().cell(0, 0).unwrap().attrs;
        assert!(attrs.contains(
            CellAttrs::BOLD
                | CellAttrs::UNDERLINE
                | CellAttrs::BLINK
                | CellAttrs::RAPID_BLINK
                | CellAttrs::INVERSE
                | CellAttrs::HIDDEN
                | CellAttrs::DIM
        ));
        assert!(!attrs.intersects(
            CellAttrs::DOUBLE_UNDERLINE
                | CellAttrs::CURLY_UNDERLINE
                | CellAttrs::DOTTED_UNDERLINE
                | CellAttrs::DASHED_UNDERLINE
        ));

        term.process(b"\x1b[1;1;1;1;22;24;25;27;28$r");
        let attrs = term.grid().cell(0, 0).unwrap().attrs;
        assert_eq!(attrs, CellAttrs::DIM | CellAttrs::RAPID_BLINK);

        term.process(b"\x1b[1;1;1;1;0;8$t");
        let reversed = term.grid().cell(0, 0).unwrap().attrs;
        assert!(reversed.contains(
            CellAttrs::DIM
                | CellAttrs::RAPID_BLINK
                | CellAttrs::BOLD
                | CellAttrs::UNDERLINE
                | CellAttrs::BLINK
                | CellAttrs::INVERSE
                | CellAttrs::HIDDEN
        ));
        term.process(b"\x1b[1;1;1;1;1;1;4;4;5;5;7;7;8;8$t");
        assert_eq!(term.grid().cell(0, 0).unwrap().attrs, reversed);

        // Empty/unknown parameter lists and malformed subparameters are true
        // no-ops. Exact DECRARA materializes untouched cells without changing
        // colors, protection, hyperlinks, cursor state, or wide-cell flags.
        term.process(b"\x1b[1;1;1;1$t\x1b[1;1;1;1;99$t\x1b[1:2;1;1;1;1$r");
        let retained = term.grid().cell(0, 0).unwrap();
        assert_eq!(retained.attrs, reversed);
        assert_eq!(retained.content, original.content);
        assert_eq!(retained.fg_spec, ColorSpec::Indexed(1));
        assert_eq!(retained.bg_spec, ColorSpec::Indexed(4));
        assert_eq!(retained.hyperlink_id, original.hyperlink_id);
        assert!(
            retained
                .flags
                .contains(CellFlags::PROTECTED | CellFlags::HYPERLINK)
        );
        term.process(b"\x1b[2*x\x1b[2;2;2;2;7$t");
        let blank = term.grid().cell(1, 1).unwrap();
        assert_eq!(blank.content.char(), Some(' '));
        assert!(blank.attrs.contains(CellAttrs::INVERSE));
        assert_eq!(blank.fg_spec, ColorSpec::Default);
        assert_eq!(blank.bg_spec, ColorSpec::Default);
        assert_eq!(
            (term.grid().cursor().row, term.grid().cursor().col),
            (cursor.row, cursor.col)
        );
        assert_eq!(term.grid().cursor().wrap_pending, cursor.wrap_pending);

        let mut wide = Terminal::new(4, 1);
        wide.process("界".as_bytes());
        wide.process(b"\x1b[2*x\x1b[1;2;1;2;7$r");
        assert!(wide.grid().cell(0, 0).unwrap().is_wide_char());
        assert!(wide.grid().cell(0, 1).unwrap().is_wide_char_spacer());
    }

    #[test]
    fn rectangular_checksum_is_policy_gated_bounded_and_vt420_compatible() {
        let mut term = Terminal::new(5, 2);
        term.process(b"A B");
        let cursor = term.grid().cursor();

        // A child cannot scrape visible cells until the embedding application
        // explicitly enables reports.
        term.process(b"\x1b[7;99;1;1;1;3*y");
        assert!(term.drain_events().is_empty());
        assert!(!term.screen_checksum_reporting());

        term.set_screen_checksum_reporting(true);
        term.process(b"\x1b[7;99;1;1;1;3*y");
        assert_eq!(
            term.drain_events(),
            vec![TerminalEvent::PtyWrite(b"\x1bP7!~FF7D\x1b\\".to_vec())]
        );

        // Reversed rectangles produce a zero report. Excess and subparameter
        // forms are malformed and cannot accidentally trigger a reply.
        term.process(b"\x1b[8;1;2;2;1;1*y\x1b[1;1;1;1;1;1;1*y\x1b[1:2;1*y");
        assert_eq!(
            term.drain_events(),
            vec![TerminalEvent::PtyWrite(b"\x1bP8!~0000\x1b\\".to_vec())]
        );
        assert_eq!(
            (term.grid().cursor().row, term.grid().cursor().col),
            (cursor.row, cursor.col)
        );
        assert_eq!(term.grid().cursor().wrap_pending, cursor.wrap_pending);

        // The host policy is not terminal state and survives both resets.
        term.process(b"\x1b[!p\x1bc\x1b[9;1;1;1;1;1*y");
        assert!(term.screen_checksum_reporting());
        assert_eq!(
            term.drain_events(),
            vec![TerminalEvent::PtyWrite(b"\x1bP9!~0000\x1b\\".to_vec())]
        );
    }

    #[test]
    fn kitty_file_policy_defaults_off_and_survives_terminal_resets() {
        let mut term = Terminal::new(5, 2);
        assert!(!term.kitty_file_transmission());
        term.set_kitty_file_transmission(true);
        term.process(b"\x1b[!p\x1bc");
        assert!(term.kitty_file_transmission());
        term.set_kitty_file_transmission(false);
        assert!(!term.kitty_file_transmission());
    }

    #[test]
    fn rectangular_checksum_reports_legacy_attributes_origin_and_dec_graphics() {
        let mut attributes = Terminal::new(3, 1);
        attributes.set_screen_checksum_reporting(true);
        attributes.process(b"\x1b[1\"q\x1b[1;2;3;4:2;5;6;7;8;9;38;2;1;2;3;48;5;4mA");
        attributes.process(b"\x1b[4;1;1;1;1;1*y");
        assert_eq!(
            attributes.drain_events(),
            vec![TerminalEvent::PtyWrite(b"\x1bP4!~FEC3\x1b\\".to_vec())]
        );

        // Never-drawn cells are skipped. The first drawn space is included,
        // while later ordinary spaces are omitted like DEC/xterm defaults.
        let mut blanks = Terminal::new(4, 1);
        blanks.set_screen_checksum_reporting(true);
        blanks.process(b" A B\x1b[5;1;1;1;1;4*y");
        assert_eq!(
            blanks.drain_events(),
            vec![TerminalEvent::PtyWrite(b"\x1bP5!~FF5D\x1b\\".to_vec())]
        );

        let mut origin = Terminal::new(4, 3);
        origin.set_screen_checksum_reporting(true);
        origin.process(b"ABCD\r\nEFGH\r\nIJKL");
        origin.process(b"\x1b[2;3r\x1b[?69h\x1b[2;4s\x1b[?6h\x1b[6;0;1;1;1;1*y");
        assert_eq!(
            origin.drain_events(),
            vec![TerminalEvent::PtyWrite(b"\x1bP6!~FFBA\x1b\\".to_vec())]
        );

        let mut graphics = Terminal::new(2, 1);
        graphics.set_screen_checksum_reporting(true);
        graphics.process(b"\x1b(0q_\x1b[10;1;1;1;1;1*y\x1b[11;1;1;2;1;2*y");
        assert_eq!(
            graphics.grid().cell(0, 0).unwrap().content.char(),
            Some('─')
        );
        assert_eq!(
            graphics.grid().cell(0, 1).unwrap().content.char(),
            Some(' ')
        );
        assert_eq!(
            graphics.drain_events(),
            vec![
                TerminalEvent::PtyWrite(b"\x1bP10!~FF8F\x1b\\".to_vec()),
                TerminalEvent::PtyWrite(b"\x1bP11!~FFA1\x1b\\".to_vec()),
            ]
        );

        let mut uk = Terminal::new(1, 1);
        uk.set_screen_checksum_reporting(true);
        uk.process(b"\x1b(A#\x1b[12;1;1;1;1;1*y");
        assert_eq!(uk.grid().cell(0, 0).unwrap().content.char(), Some('£'));
        assert_eq!(
            uk.drain_events(),
            vec![TerminalEvent::PtyWrite(b"\x1bP12!~FFDD\x1b\\".to_vec())]
        );

        let mut repeated = Terminal::new(2, 1);
        repeated.set_screen_checksum_reporting(true);
        repeated.process(b"\x1b(0q\x1b(B\x1b[b\x1b[13;1;1;2;1;2*y");
        assert_eq!(
            repeated.grid().cell(0, 1).unwrap().content.char(),
            Some('─')
        );
        assert_eq!(
            repeated.drain_events(),
            vec![TerminalEvent::PtyWrite(b"\x1bP13!~FF8F\x1b\\".to_vec())]
        );
    }

    #[test]
    fn decsed_modes_are_bounded_and_reject_malformed_requests() {
        let mut term = Terminal::new(4, 3);
        term.process(b"\x1b[1\"qA\x1b[0\"qa\r\n\x1b[1\"qB\x1b[0\"qb\r\n\x1b[1\"qC\x1b[0\"qc");

        // Mode 0 starts at the cursor, preserving both protected cells and
        // erasable text before the cursor.
        term.process(b"\x1b[2;2H\x1b[?0J");
        assert_eq!(term.rows_text(0, 2), "Aa\nB\nC");

        // Restore erasable suffixes, then mode 1 erases through the cursor.
        term.process(b"\x1b[2;2Ha\x1b[3;2Hb\x1b[2;2H\x1b[?1J");
        assert_eq!(term.rows_text(0, 2), "A\nB\nCb");

        // Plural and subparameter private requests are malformed and must not
        // accidentally fall through to ordinary ED. Singular mode 2 then
        // erases every erasable cell while retaining protected letters.
        term.process(b"\x1b[?1;2J\x1b[?2:0J");
        assert_eq!(term.rows_text(0, 2), "A\nB\nCb");
        term.process(b"\x1b[?2J");
        assert_eq!(term.rows_text(0, 2), "A\nB\nC");
    }

    #[test]
    fn shell_regions_follow_scrollback_and_retire_live_text_after_eviction() {
        let mut term = Terminal::new(16, 3);
        term.process(
            b"\x1b]133;A\x07\x1b]133;B\x07echo stable\r\n\x1b]133;C\x07stable\r\n\x1b]133;D;0\x07",
        );
        let id = term
            .shell_integration()
            .prompt_tracker
            .completed_region(0)
            .unwrap()
            .id;
        term.process(b"one\r\ntwo\r\n");

        let region = term
            .shell_integration()
            .prompt_tracker
            .completed_region_by_id(id)
            .unwrap();
        assert_eq!(region.prompt_row, -2);
        assert_eq!(region.output_row, Some(-1));
        assert_eq!(
            term.rows_text(region.command_row.unwrap(), region.command_row.unwrap()),
            "echo stable"
        );

        term.set_scrollback_limits(1, 1024 * 1024);
        let evicted = term
            .shell_integration()
            .prompt_tracker
            .completed_region_by_id(id)
            .unwrap();
        assert!(!evicted.content_available);
        assert!(
            term.shell_integration()
                .prompt_tracker
                .command_at(evicted.prompt_row)
                .is_none()
        );
    }

    #[test]
    fn shell_region_boundaries_follow_column_reflow() {
        let mut term = Terminal::new(12, 4);
        term.process(
            b"\x1b]133;A\x07\x1b]133;B\x07echo 12345678\r\n\x1b]133;C\x07result value\r\n\x1b]133;D;0\x07",
        );
        term.process(b"later one\r\nlater two\r\n");
        let before = term
            .shell_integration()
            .prompt_tracker
            .completed_region(0)
            .unwrap()
            .clone();
        assert!(before.prompt_row < 0);
        let command_before = term.rows_text(
            before.command_row.unwrap(),
            before.output_row.unwrap().saturating_sub(1),
        );
        let output_before = term.rows_text(
            before.output_row.unwrap(),
            before.finish_row.unwrap().saturating_sub(1),
        );

        term.resize(6, 7);

        let after = term
            .shell_integration()
            .prompt_tracker
            .completed_region_by_id(before.id)
            .unwrap();
        let command_after = term.rows_text(
            after.command_row.unwrap(),
            after.output_row.unwrap().saturating_sub(1),
        );
        let output_after = term.rows_text(
            after.output_row.unwrap(),
            after.finish_row.unwrap().saturating_sub(1),
        );
        let compact = |text: &str| text.split_whitespace().collect::<String>();
        assert_eq!(compact(&command_after), compact(&command_before));
        assert_eq!(compact(&output_after), compact(&output_before));
        assert_eq!(
            term.shell_integration()
                .prompt_tracker
                .command_at(after.output_row.unwrap())
                .unwrap()
                .id,
            before.id
        );
    }

    #[test]
    fn prompt_navigation_walks_history_and_survives_reflow_without_stale_rows() {
        let mut term = Terminal::new(14, 4);
        for index in 0..5 {
            term.process(b"\x1b]133;A\x07\x1b]133;B\x07");
            term.process(format!("echo {index}\r\n").as_bytes());
            term.process(b"\x1b]133;C\x07output\r\n\x1b]133;D;0\x07");
        }
        term.process(b"\x1b]133;A\x07");

        let cursor_row = i32::try_from(term.grid().cursor().row).unwrap();
        let prompts = term
            .shell_integration()
            .prompt_tracker
            .all_prompts()
            .collect::<Vec<_>>();
        let backwards = prompts
            .iter()
            .copied()
            .filter(|row| *row < cursor_row)
            .rev()
            .collect::<Vec<_>>();
        assert!(backwards.len() >= 4);
        assert!(backwards.iter().any(|row| *row < 0));

        for expected in &backwards {
            assert_eq!(term.previous_prompt(), Some(*expected));
            assert_eq!(
                term.grid().scroll_offset(),
                usize::try_from(expected.saturating_neg()).unwrap_or(0)
            );
        }
        assert_eq!(term.previous_prompt(), None);

        for expected in prompts
            .iter()
            .copied()
            .filter(|row| *row > *backwards.last().unwrap())
        {
            assert_eq!(term.next_prompt(), Some(expected));
        }
        assert_eq!(term.next_prompt(), None);

        // Direct scrolling invalidates the navigation cursor. Starting again
        // from the live viewport must select the newest prior prompt, not
        // continue from an invisible stale selection.
        let newest_previous = backwards[0];
        assert_eq!(term.previous_prompt(), Some(newest_previous));
        let reflow_target = term.previous_prompt().unwrap();
        assert!(reflow_target < 0);
        let target_region_id = term
            .shell_integration()
            .prompt_tracker
            .command_at(reflow_target)
            .unwrap()
            .id;
        term.grid_mut().set_scroll_offset(0);
        assert_eq!(term.previous_prompt(), Some(newest_previous));
        assert_eq!(term.previous_prompt(), Some(reflow_target));

        term.resize(7, 6);
        let remapped_target = term
            .shell_integration()
            .prompt_tracker
            .completed_region_by_id(target_region_id)
            .unwrap()
            .prompt_row;
        let expected_older = term
            .shell_integration()
            .prompt_tracker
            .previous_prompt(remapped_target);
        assert_eq!(term.previous_prompt(), expected_older);

        term.set_scrollback_limits(0, 0);
        assert!(
            term.shell_integration()
                .prompt_tracker
                .all_prompts()
                .all(|row| row >= 0)
        );
        assert!(term.previous_prompt().is_none_or(|row| row >= 0));

        term.process(b"\x1b[?1049h");
        assert_eq!(term.previous_prompt(), None);
        assert_eq!(term.next_prompt(), None);
    }

    #[test]
    fn selected_prompt_stays_visible_during_output_until_the_user_scrolls() {
        let mut term = Terminal::new(14, 4);
        for index in 0..3 {
            term.process(b"\x1b]133;A\x07\x1b]133;B\x07");
            term.process(format!("echo {index}\r\n").as_bytes());
            term.process(b"\x1b]133;C\x07output\r\n\x1b]133;D;0\x07");
        }
        term.process(b"\x1b]133;A\x07");

        let selected = term.previous_prompt().unwrap();
        assert!(selected >= 0);
        let selected_id = term
            .shell_integration()
            .prompt_tracker
            .command_at(selected)
            .unwrap()
            .id;
        assert_eq!(term.grid().scroll_offset(), 0);

        term.process(b"more\r\nmore\r\n");
        let moved = term
            .shell_integration()
            .prompt_tracker
            .completed_region_by_id(selected_id)
            .unwrap()
            .prompt_row;
        assert!(moved < 0);
        assert_eq!(
            term.grid().scroll_offset(),
            usize::try_from(moved.saturating_neg()).unwrap()
        );

        // A direct scroll is an explicit change of navigation origin. Later
        // PTY output must not pull the user back to the old selected prompt.
        term.grid_mut().set_scroll_offset(0);
        term.process(b"new-output\r\n");
        assert_eq!(term.grid().scroll_offset(), 0);
    }

    #[test]
    fn destructive_screen_lifecycle_retires_only_erased_shell_content() {
        let mut term = Terminal::new(16, 4);
        term.process(
            b"\x1b]133;A\x07\x1b]133;B\x07echo old\r\n\x1b]133;C\x07old\r\n\x1b]133;D;0\x07",
        );
        term.process(b"one\r\ntwo\r\nthree\r\nfour\r\n");
        let historical = term
            .shell_integration()
            .prompt_tracker
            .completed_region(0)
            .unwrap()
            .clone();
        assert!(historical.finish_row.unwrap() < 0);

        term.process(b"\x1b]133;A\x07\x1b]133;B\x07echo active\x1b]133;C\x07");
        let active_id = term
            .shell_integration()
            .prompt_tracker
            .active_region()
            .unwrap()
            .id;
        assert!(
            term.shell_integration()
                .prompt_tracker
                .active_region()
                .unwrap()
                .prompt_row
                >= 0
        );

        term.process(b"\x1b[2J");
        assert!(
            term.shell_integration()
                .prompt_tracker
                .completed_region_by_id(historical.id)
                .unwrap()
                .content_available
        );
        assert!(
            !term
                .shell_integration()
                .prompt_tracker
                .active_region()
                .unwrap()
                .content_available
        );
        assert!(
            term.shell_integration()
                .prompt_tracker
                .all_prompts()
                .all(|row| row < 0)
        );

        // Completion identity survives the erase and is still emitted once;
        // only its authority to resolve erased terminal text is retired.
        term.process(b"\x1b]133;D;1\x07");
        let active_completion = term
            .shell_integration()
            .prompt_tracker
            .completed_region_by_id(active_id)
            .unwrap();
        assert!(!active_completion.content_available);
        assert_eq!(active_completion.exit_code, Some(1));

        term.clear_scrollback();
        assert_eq!(term.grid().scrollback_len(), 0);
        assert!(
            term.shell_integration()
                .prompt_tracker
                .all_prompts()
                .all(|row| row >= 0)
        );
        assert!(
            !term
                .shell_integration()
                .prompt_tracker
                .completed_region_by_id(historical.id)
                .unwrap()
                .content_available
        );
    }

    #[test]
    fn ris_preserves_valid_historical_marks_but_retires_the_erased_viewport() {
        let mut term = Terminal::new(16, 4);
        term.process(
            b"\x1b]133;A\x07\x1b]133;B\x07echo old\r\n\x1b]133;C\x07old\r\n\x1b]133;D;0\x07",
        );
        term.process(b"one\r\ntwo\r\nthree\r\nfour\r\n");
        let historical = term
            .shell_integration()
            .prompt_tracker
            .completed_region(0)
            .unwrap()
            .clone();
        assert!(historical.finish_row.unwrap() < 0);

        term.process(b"\x1b]133;A\x07\x1b]133;B\x07echo visible\x1b]133;C\x07\x1b]133;D;0\x07");
        let visible = term
            .shell_integration()
            .prompt_tracker
            .completed_region(1)
            .unwrap()
            .clone();
        assert!(visible.prompt_row >= 0);
        let oldest_offset = term.grid().scrollback_len();
        term.grid_mut().set_scroll_offset(oldest_offset);

        term.process(b"\x1bc");
        assert_eq!(term.grid().scroll_offset(), 0);
        assert!(
            term.shell_integration()
                .prompt_tracker
                .completed_region_by_id(historical.id)
                .unwrap()
                .content_available
        );
        assert!(
            !term
                .shell_integration()
                .prompt_tracker
                .completed_region_by_id(visible.id)
                .unwrap()
                .content_available
        );
        assert!(
            term.shell_integration()
                .prompt_tracker
                .all_prompts()
                .all(|row| row < 0)
        );
    }

    #[test]
    fn alternate_screen_cannot_publish_primary_shell_marks() {
        let mut term = Terminal::new(16, 4);
        term.process(b"\x1b]133;A\x07");
        let prompts_before = term
            .shell_integration()
            .prompt_tracker
            .all_prompts()
            .collect::<Vec<_>>();
        let regions_before = term.shell_integration().prompt_tracker.completed_count();

        term.process(b"\x1b[?1049h");
        term.process(b"\x1b]133;A\x07\x1b]133;B\x07untrusted\x1b]133;C\x07\x1b]133;D;9\x07");
        term.process(b"\x1b[3J\x1b[?1049l");

        assert_eq!(
            term.shell_integration()
                .prompt_tracker
                .all_prompts()
                .collect::<Vec<_>>(),
            prompts_before
        );
        assert_eq!(
            term.shell_integration().prompt_tracker.completed_count(),
            regions_before
        );
    }

    #[test]
    fn fragmented_osc_133_lifecycle_is_strict_and_streaming_safe() {
        let mut term = Terminal::new(16, 4);
        let valid =
            b"\x1b]133;A\x07\x1b]133;B\x07echo split\r\n\x1b]133;C\x07out\r\n\x1b]133;D;255\x07";
        for byte in valid {
            term.process(std::slice::from_ref(byte));
        }
        let first = term
            .shell_integration()
            .prompt_tracker
            .completed_region(0)
            .unwrap();
        assert_eq!(first.exit_code, Some(255));

        // Prompt-only and out-of-order cycles must not add completed regions.
        let malformed =
            b"\x1b]133;A\x07\x1b]133;B\x07\x1b]133;D;0\x07\x1b]133;C\x07\x1b]133;D;0\x07";
        for split in malformed.chunks(2) {
            term.process(split);
        }
        assert_eq!(term.shell_integration().prompt_tracker.completed_count(), 1);

        term.process(
            b"\x1b]133;A\x07\x1b]133;B\x07echo recover\r\n\x1b]133;C\x07out\r\n\x1b]133;D;256\x07",
        );
        let recovered = term
            .shell_integration()
            .prompt_tracker
            .completed_region(1)
            .unwrap();
        assert_eq!(recovered.exit_code, None);
    }

    #[test]
    fn completed_notification_icons_have_a_separate_pending_event_byte_ceiling() {
        let mut term = Terminal::new(8, 2);
        for index in 0..3 {
            term.push_event(TerminalEvent::Notification(
                crate::event::NotificationRequest {
                    title: Some(format!("icon-{index}")),
                    icon_cache_id: Some(format!("cache-{index}")),
                    icon_data: Some(
                        vec![index as u8; MAX_NOTIFICATION_ICON_BYTES].into_boxed_slice(),
                    ),
                    ..crate::event::NotificationRequest::default()
                },
            ));
        }
        let events = term.drain_events();
        assert_eq!(events.len(), 3);
        assert!(
            matches!(&events[0], TerminalEvent::Notification(request) if request.icon_data.is_some())
        );
        assert!(
            matches!(&events[1], TerminalEvent::Notification(request) if request.icon_data.is_some())
        );
        assert!(
            matches!(&events[2], TerminalEvent::Notification(request) if request.icon_data.is_none() && request.icon_cache_id.is_none())
        );

        term.push_event(TerminalEvent::Notification(
            crate::event::NotificationRequest {
                title: Some("after-drain".into()),
                icon_data: Some(vec![0; MAX_NOTIFICATION_ICON_BYTES].into_boxed_slice()),
                ..crate::event::NotificationRequest::default()
            },
        ));
        assert!(matches!(
            term.drain_events().as_slice(),
            [TerminalEvent::Notification(request)] if request.icon_data.is_some()
        ));
    }
}
