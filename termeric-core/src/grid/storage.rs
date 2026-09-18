//! Grid storage with scrollback buffer

use super::cell::{Cell, CellAttrs, CellContent, CellFlags, Color, ColorSpec, MultiCell};
use super::row::Row;
use std::collections::{HashSet, VecDeque};
use std::fmt;
use std::mem::size_of;
use std::ops::Deref;

/// Default maximum scrollback lines
const DEFAULT_SCROLLBACK_LIMIT: usize = 10_000;

/// Default per-terminal memory budget for archived scrollback.
pub const DEFAULT_SCROLLBACK_MEMORY_BYTES: usize = 128 * 1024 * 1024;

/// Estimated memory owned by the grid's visible and historical rows.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct GridMemoryUsage {
    pub visible_bytes: usize,
    pub scrollback_bytes: usize,
    pub grapheme_bytes: usize,
    pub style_bytes: usize,
    pub hyperlink_reference_bytes: usize,
    pub scrollback_lines: usize,
    pub scrollback_line_limit: usize,
    pub scrollback_byte_limit: usize,
}

/// A cell borrowed from the mutable grid or reconstructed from compact history.
pub enum CellRef<'a> {
    Borrowed(&'a Cell),
    Owned(Cell),
}

impl Deref for CellRef<'_> {
    type Target = Cell;

    fn deref(&self) -> &Self::Target {
        match self {
            Self::Borrowed(cell) => cell,
            Self::Owned(cell) => cell,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct CellStyle {
    fg: Color,
    bg: Color,
    fg_spec: ColorSpec,
    bg_spec: ColorSpec,
    underline_color: Option<Color>,
    underline_color_spec: Option<ColorSpec>,
    attrs: CellAttrs,
    flags: CellFlags,
    /// Raw identifier; `CellFlags::HYPERLINK` is the compact Option tag.
    hyperlink_id: u32,
    /// OSC 66 geometry is zero for ordinary cells. Covered cells intentionally
    /// form separate runs because their relative coordinates differ.
    multicell: MultiCell,
}

impl CellStyle {
    fn from_cell(cell: &Cell) -> Self {
        // Structural width flags are encoded by CompactCellContent and DIRTY
        // is transient. Hyperlink and DEC protection are durable cell state.
        let mut flags = cell.flags
            & (CellFlags::HYPERLINK
                | CellFlags::PROTECTED
                | CellFlags::DEC_SPECIAL_GRAPHICS
                | CellFlags::DEC_UK_CHARSET);
        flags.set(CellFlags::HYPERLINK, cell.hyperlink_id.is_some());
        Self {
            fg: cell.fg,
            bg: cell.bg,
            fg_spec: cell.fg_spec,
            bg_spec: cell.bg_spec,
            underline_color: cell.underline_color,
            underline_color_spec: cell.underline_color_spec,
            attrs: cell.attrs,
            flags,
            hyperlink_id: cell.hyperlink_id.unwrap_or_default(),
            multicell: cell.multicell,
        }
    }

    fn hyperlink_id(self) -> Option<u32> {
        self.flags
            .contains(CellFlags::HYPERLINK)
            .then_some(self.hyperlink_id)
    }

    fn apply(&self, content: CompactCellContent) -> Cell {
        let (content, structural_flags) = content.expand();
        Cell {
            content,
            fg: self.fg,
            bg: self.bg,
            fg_spec: self.fg_spec,
            bg_spec: self.bg_spec,
            underline_color: self.underline_color,
            underline_color_spec: self.underline_color_spec,
            attrs: self.attrs,
            flags: self.flags | structural_flags,
            hyperlink_id: self.hyperlink_id(),
            multicell: self.multicell,
        }
    }
}

/// Cell text plus width structure. Keeping width out of `CellStyle` prevents
/// every wide leader/spacer pair from fragmenting an otherwise uniform style
/// run while retaining exact reconstruction without remeasuring Unicode.
#[derive(Clone, Debug)]
enum CompactCellContent {
    Empty,
    Char(char),
    Grapheme(Box<str>),
    WideEmpty,
    WideChar(char),
    WideGrapheme(Box<str>),
    WideSpacer,
}

impl CompactCellContent {
    fn from_cell(cell: Cell) -> Self {
        if cell.flags.contains(CellFlags::WIDE_CHAR_SPACER) {
            return Self::WideSpacer;
        }
        let wide = cell.flags.contains(CellFlags::WIDE_CHAR);
        match (wide, cell.content) {
            (false, CellContent::Empty) => Self::Empty,
            (false, CellContent::Char(character)) => Self::Char(character),
            (false, CellContent::Grapheme(grapheme)) => Self::Grapheme(grapheme),
            (true, CellContent::Empty) => Self::WideEmpty,
            (true, CellContent::Char(character)) => Self::WideChar(character),
            (true, CellContent::Grapheme(grapheme)) => Self::WideGrapheme(grapheme),
        }
    }

    fn expand(self) -> (CellContent, CellFlags) {
        match self {
            Self::Empty => (CellContent::Empty, CellFlags::empty()),
            Self::Char(character) => (CellContent::Char(character), CellFlags::empty()),
            Self::Grapheme(grapheme) => (CellContent::Grapheme(grapheme), CellFlags::empty()),
            Self::WideEmpty => (CellContent::Empty, CellFlags::WIDE_CHAR),
            Self::WideChar(character) => (CellContent::Char(character), CellFlags::WIDE_CHAR),
            Self::WideGrapheme(grapheme) => (CellContent::Grapheme(grapheme), CellFlags::WIDE_CHAR),
            Self::WideSpacer => (CellContent::Empty, CellFlags::WIDE_CHAR_SPACER),
        }
    }

    fn grapheme_bytes(&self) -> usize {
        match self {
            Self::Grapheme(grapheme) | Self::WideGrapheme(grapheme) => grapheme.len(),
            _ => 0,
        }
    }

    fn is_wide_leader(&self) -> bool {
        matches!(
            self,
            Self::WideEmpty | Self::WideChar(_) | Self::WideGrapheme(_)
        )
    }

    fn is_wide_spacer(&self) -> bool {
        matches!(self, Self::WideSpacer)
    }

    fn has_unmarked_wide_content(&self) -> bool {
        match self {
            Self::Char(character) => {
                let mut encoded = [0_u8; 4];
                crate::unicode::grapheme_width(character.encode_utf8(&mut encoded)) == 2
            }
            Self::Grapheme(grapheme) => crate::unicode::grapheme_width(grapheme) == 2,
            _ => false,
        }
    }
}

#[derive(Clone, Debug)]
struct StyleRun {
    start: usize,
    style: CellStyle,
}

/// Immutable archival row. Content is trimmed and styles are run-length encoded.
#[derive(Clone, Debug)]
struct CompactRow {
    content: Vec<CompactCellContent>,
    styles: Vec<StyleRun>,
    cols: usize,
    wrapped: bool,
    wide_wrap_padding: bool,
    /// Immutable structural count: bookmarks need not scan archived payloads.
    wide_spacers: usize,
}

impl CompactRow {
    fn from_row(row: Row) -> Self {
        let cols = row.len();
        let wrapped = row.is_wrapped();
        let wide_wrap_padding = row.has_wide_wrap_padding();
        let mut cells = row.into_cells();
        while cells.last().is_some_and(Cell::is_history_default) {
            cells.pop();
        }

        let mut content = Vec::with_capacity(cells.len());
        let mut styles = Vec::new();
        let mut wide_spacers = 0;
        for (column, cell) in cells.into_iter().enumerate() {
            wide_spacers += usize::from(cell.is_wide_char_spacer());
            let style = CellStyle::from_cell(&cell);
            if styles
                .last()
                .is_none_or(|run: &StyleRun| run.style != style)
            {
                styles.push(StyleRun {
                    start: column,
                    style,
                });
            }
            content.push(CompactCellContent::from_cell(cell));
        }
        content.shrink_to_fit();
        styles.shrink_to_fit();

        Self {
            content,
            styles,
            cols,
            wrapped,
            wide_wrap_padding,
            wide_spacers,
        }
    }

    /// Compact a row while retaining its full-width cell allocation for the
    /// next empty viewport row. Steady output scrolls one row at a time, so
    /// dropping this buffer and allocating an identical one creates avoidable
    /// traffic proportional to all terminal output.
    fn from_row_recycling(row: Row) -> (Self, Row) {
        let cols = row.len();
        let wrapped = row.is_wrapped();
        let wide_wrap_padding = row.has_wide_wrap_padding();
        let mut cells = row.into_cells();
        let retained_len = cells
            .iter()
            .rposition(|cell| !cell.is_history_default())
            .map_or(0, |column| column + 1);
        let mut content = Vec::with_capacity(retained_len);
        let mut styles = Vec::new();
        let mut wide_spacers = 0;
        for (column, slot) in cells.iter_mut().take(retained_len).enumerate() {
            let cell = std::mem::take(slot);
            wide_spacers += usize::from(cell.is_wide_char_spacer());
            let style = CellStyle::from_cell(&cell);
            if styles
                .last()
                .is_none_or(|run: &StyleRun| run.style != style)
            {
                styles.push(StyleRun {
                    start: column,
                    style,
                });
            }
            content.push(CompactCellContent::from_cell(cell));
        }
        content.shrink_to_fit();
        styles.shrink_to_fit();

        for cell in &mut cells {
            *cell = Cell::default();
        }
        (
            Self {
                content,
                styles,
                cols,
                wrapped,
                wide_wrap_padding,
                wide_spacers,
            },
            Row::from_cells(cells, false),
        )
    }

    fn get(&self, col: usize) -> Option<Cell> {
        if col >= self.cols {
            return None;
        }
        let Some(content) = self.content.get(col) else {
            return Some(Cell::default());
        };
        let style = self
            .styles
            .iter()
            .rev()
            .find(|run| run.start <= col)
            .expect("stored history content has a style run");
        Some(style.style.apply(content.clone()))
    }

    fn into_reflow_cells(self) -> Vec<Cell> {
        let Self {
            content,
            styles,
            cols,
            wrapped,
            wide_wrap_padding,
            ..
        } = self;
        let stored_len = content.len();
        let mut cells = Vec::with_capacity(if wrapped { cols } else { stored_len });
        let mut runs = styles.into_iter().peekable();
        let mut style = runs
            .next()
            .map(|run| run.style)
            .unwrap_or_else(|| CellStyle::from_cell(&Cell::default()));
        for (column, content) in content.into_iter().enumerate() {
            if runs.peek().is_some_and(|run| run.start == column) {
                style = runs.next().expect("peeked style run exists").style;
            }
            cells.push(style.apply(content));
        }
        if wrapped {
            cells.resize_with(cols, Cell::default);
            if wide_wrap_padding {
                cells.pop();
            }
        }
        cells
    }

    /// A hard-broken compact row that already fits the new width needs no
    /// expansion into rich cells or recompression. Validate wide-cell
    /// structure before taking this path so malformed defensive inputs still
    /// flow through the repairing reference algorithm.
    fn retarget_width_without_reflow(&mut self, new_cols: usize) -> bool {
        if self.wrapped || self.content.len() > new_cols {
            return false;
        }
        for (column, content) in self.content.iter().enumerate() {
            if content.has_unmarked_wide_content()
                || (content.is_wide_leader()
                    && (column + 1 >= new_cols
                        || !self
                            .content
                            .get(column + 1)
                            .is_some_and(CompactCellContent::is_wide_spacer)))
                || (content.is_wide_spacer()
                    && (column == 0
                        || !self
                            .content
                            .get(column - 1)
                            .is_some_and(CompactCellContent::is_wide_leader)))
            {
                return false;
            }
        }
        self.cols = new_cols;
        true
    }

    fn to_row(&self) -> Row {
        let mut cells = Vec::with_capacity(self.cols);
        for col in 0..self.cols {
            cells.push(self.get(col).expect("history cell is in bounds"));
        }
        let mut row = Row::from_cells(cells, self.wrapped);
        if self.wide_wrap_padding {
            row.mark_wide_wrap_padding();
        }
        row
    }

    fn owned_bytes(&self) -> usize {
        size_of::<Self>()
            + self.content.capacity() * size_of::<CompactCellContent>()
            + self.styles.capacity() * size_of::<StyleRun>()
            + self
                .content
                .iter()
                .map(CompactCellContent::grapheme_bytes)
                .sum::<usize>()
    }

    fn grapheme_bytes(&self) -> usize {
        self.content
            .iter()
            .map(CompactCellContent::grapheme_bytes)
            .sum()
    }

    fn style_bytes(&self) -> usize {
        self.styles.capacity() * size_of::<StyleRun>()
    }

    fn hyperlink_reference_bytes(&self) -> usize {
        self.styles
            .iter()
            .filter(|run| run.style.hyperlink_id().is_some())
            .count()
            * size_of::<StyleRun>()
    }

    fn collect_hyperlink_ids(&self, identifiers: &mut HashSet<u32>) {
        identifiers.extend(
            self.styles
                .iter()
                .filter_map(|run| run.style.hyperlink_id()),
        );
    }
}

/// Read-only view over either a visible full-width row or compact history.
pub struct RowView<'a> {
    inner: RowViewInner<'a>,
}

enum RowViewInner<'a> {
    Visible(&'a Row),
    History(&'a CompactRow),
}

impl<'a> RowView<'a> {
    /// Count bookmark units without expanding or scanning compact history.
    pub fn reflow_unit_count(&self) -> usize {
        match self.inner {
            RowViewInner::Visible(_) => self.reflow_columns().count(),
            RowViewInner::History(row) => row
                .cols
                .saturating_sub(row.wide_spacers)
                .saturating_sub(usize::from(row.wide_wrap_padding)),
        }
    }

    /// Physical columns that contribute stable cursor-bookmark units to reflow.
    /// A wide grapheme counts once at any grid width; its spacer and synthetic
    /// wrap padding do not count. Ordinary blanks remain units. OSC 66 cells
    /// retain their physical-cell policy. This does not expand compact history
    /// or clone grapheme payloads.
    pub fn reflow_columns(&self) -> impl Iterator<Item = usize> + '_ {
        let end = self
            .len()
            .saturating_sub(usize::from(self.has_wide_wrap_padding()));
        (0..end).filter(|&col| match self.inner {
            RowViewInner::Visible(row) => {
                row.get(col).is_none_or(|cell| !cell.is_wide_char_spacer())
            }
            RowViewInner::History(row) => row
                .content
                .get(col)
                .is_none_or(|cell| !cell.is_wide_spacer()),
        })
    }

    /// Whether the final column is synthetic padding before a wrapped wide
    /// grapheme, rather than an ordinary blank belonging to the logical text.
    pub fn has_wide_wrap_padding(&self) -> bool {
        match self.inner {
            RowViewInner::Visible(row) => row.has_wide_wrap_padding(),
            RowViewInner::History(row) => row.wide_wrap_padding,
        }
    }

    fn visible(row: &'a Row) -> Self {
        Self {
            inner: RowViewInner::Visible(row),
        }
    }

    fn history(row: &'a CompactRow) -> Self {
        Self {
            inner: RowViewInner::History(row),
        }
    }

    pub fn get(&self, col: usize) -> Option<CellRef<'a>> {
        match self.inner {
            RowViewInner::Visible(row) => row.get(col).map(CellRef::Borrowed),
            RowViewInner::History(row) => row.get(col).map(CellRef::Owned),
        }
    }

    pub fn len(&self) -> usize {
        match self.inner {
            RowViewInner::Visible(row) => row.len(),
            RowViewInner::History(row) => row.cols,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn is_wrapped(&self) -> bool {
        match self.inner {
            RowViewInner::Visible(row) => row.is_wrapped(),
            RowViewInner::History(row) => row.wrapped,
        }
    }

    pub fn iter(&self) -> RowViewCells<'_, 'a> {
        RowViewCells {
            row: self,
            column: 0,
        }
    }

    pub fn to_ansi_string(&self) -> String {
        match self.inner {
            RowViewInner::Visible(row) => row.to_ansi_string(),
            RowViewInner::History(row) => row.to_row().to_ansi_string(),
        }
    }

    /// Extract plain text without reconstructing historical style cells.
    pub fn plain_text(&self, cols: usize) -> String {
        let mut text = String::with_capacity(cols);
        for cell in self.iter().take(cols) {
            if !cell.is_wide_char_spacer()
                && (!cell.multicell.is_present() || cell.multicell.is_leader())
            {
                push_content(&mut text, &cell.content);
            }
        }
        text
    }
}

fn push_content(text: &mut String, content: &CellContent) {
    match content {
        CellContent::Empty => text.push(' '),
        CellContent::Char(character) => text.push(*character),
        CellContent::Grapheme(grapheme) => text.push_str(grapheme),
    }
}

pub struct RowViewCells<'view, 'row> {
    row: &'view RowView<'row>,
    column: usize,
}

impl<'row> Iterator for RowViewCells<'_, 'row> {
    type Item = CellRef<'row>;

    fn next(&mut self) -> Option<Self::Item> {
        let cell = self.row.get(self.column)?;
        self.column += 1;
        Some(cell)
    }
}

impl fmt::Display for RowView<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut text = String::new();
        for cell in self.iter() {
            if cell.is_wide_char_spacer()
                || (cell.multicell.is_present() && !cell.multicell.is_leader())
            {
                continue;
            }
            match &cell.content {
                CellContent::Empty => text.push(' '),
                CellContent::Char(character) => text.push(*character),
                CellContent::Grapheme(grapheme) => text.push_str(grapheme),
            }
        }
        formatter.write_str(text.trim_end())
    }
}

/// Grid storage manages the visible grid and scrollback buffer
pub struct GridStorage {
    /// Visible rows
    rows: Vec<Row>,
    /// Scrollback buffer (historical lines)
    scrollback: VecDeque<CompactRow>,
    /// Maximum scrollback lines
    scrollback_limit: usize,
    /// Maximum estimated bytes owned by compact scrollback rows
    scrollback_byte_limit: usize,
    /// Cached estimated bytes owned by compact scrollback rows
    scrollback_bytes: usize,
    /// Number of columns
    cols: usize,
}

impl GridStorage {
    /// Create new grid storage
    pub fn new(cols: usize, rows: usize) -> Self {
        Self {
            rows: (0..rows).map(|_| Row::new(cols)).collect(),
            scrollback: VecDeque::new(),
            scrollback_limit: DEFAULT_SCROLLBACK_LIMIT,
            scrollback_byte_limit: DEFAULT_SCROLLBACK_MEMORY_BYTES,
            scrollback_bytes: 0,
            cols,
        }
    }

    /// Set the scrollback limit
    pub fn set_scrollback_limit(&mut self, limit: usize) {
        self.scrollback_limit = limit;
        if self.enforce_scrollback_limits() > 0 {
            self.scrollback.shrink_to_fit();
        }
    }

    /// Set the estimated byte budget for compact scrollback rows.
    pub fn set_scrollback_byte_limit(&mut self, limit: usize) {
        self.scrollback_byte_limit = limit;
        if self.enforce_scrollback_limits() > 0 {
            self.scrollback.shrink_to_fit();
        }
    }

    fn push_scrollback(&mut self, row: Row) -> usize {
        // A zero limit is used by viewport-only buffers such as the alternate
        // screen. Avoid allocating a compact row merely to evict it again.
        if self.scrollback_limit == 0 || self.scrollback_byte_limit == 0 {
            return 1;
        }
        let compact = CompactRow::from_row(row);
        self.scrollback_bytes = self.scrollback_bytes.saturating_add(compact.owned_bytes());
        self.scrollback.push_back(compact);
        self.enforce_scrollback_limits()
    }

    fn push_scrollback_recycling(&mut self, row: Row) -> (usize, Row) {
        if self.scrollback_limit == 0 || self.scrollback_byte_limit == 0 {
            let cols = row.len();
            let mut cells = row.into_cells();
            for cell in &mut cells {
                *cell = Cell::default();
            }
            debug_assert_eq!(cells.len(), cols);
            return (1, Row::from_cells(cells, false));
        }
        let (compact, recycled) = CompactRow::from_row_recycling(row);
        self.scrollback_bytes = self.scrollback_bytes.saturating_add(compact.owned_bytes());
        self.scrollback.push_back(compact);
        (self.enforce_scrollback_limits(), recycled)
    }

    fn enforce_scrollback_limits(&mut self) -> usize {
        let mut evicted = 0;
        while self.scrollback.len() > self.scrollback_limit
            || self.scrollback_bytes > self.scrollback_byte_limit
        {
            let Some(row) = self.scrollback.pop_front() else {
                break;
            };
            self.scrollback_bytes = self.scrollback_bytes.saturating_sub(row.owned_bytes());
            evicted += 1;
        }
        evicted
    }

    /// Get a reference to a cell
    pub fn cell(&self, row: usize, col: usize) -> Option<&Cell> {
        self.rows.get(row)?.get(col)
    }

    /// Get a mutable reference to a cell
    pub fn cell_mut(&mut self, row: usize, col: usize) -> Option<&mut Cell> {
        self.rows.get_mut(row)?.get_mut(col)
    }

    /// Get a reference to a row
    pub fn row(&self, row: usize) -> Option<&Row> {
        self.rows.get(row)
    }

    /// Get a mutable reference to a row
    pub fn row_mut(&mut self, row: usize) -> Option<&mut Row> {
        self.rows.get_mut(row)
    }

    /// Scroll the grid up by one line
    ///
    /// The top line is pushed to scrollback, and a new empty line appears at the bottom.
    pub fn scroll_up(&mut self) -> usize {
        if self.rows.is_empty() {
            return 0;
        }

        let top_row = self.rows.remove(0);
        let (evicted, recycled) = self.push_scrollback_recycling(top_row);

        self.rows.push(recycled);
        evicted
    }

    /// Append a copy of every visible row to compact scrollback without
    /// changing the viewport. Used by Kitty's ED 22 extension, whose copy
    /// must retain complete multicell descriptors before the screen is erased.
    pub(crate) fn archive_visible(&mut self) -> usize {
        let rows = self.rows.clone();
        rows.into_iter()
            .fold(0_usize, |evicted, row| evicted + self.push_scrollback(row))
    }

    /// Scroll the grid down by one line (reverse scroll)
    ///
    /// A new empty line appears at the top, and the bottom line is discarded.
    pub fn scroll_down(&mut self) {
        if self.rows.is_empty() {
            return;
        }

        // Remove bottom row (discarded)
        self.rows.pop();

        // Add new empty row at top
        self.rows.insert(0, Row::new(self.cols));
    }

    /// Scroll a region up by one line
    pub fn scroll_region_up(&mut self, top: usize, bottom: usize) -> usize {
        if top >= bottom || bottom > self.rows.len() {
            return 0;
        }

        // Remove the top line of the region
        let removed = self.rows.remove(top);

        // Insert new line at bottom of region
        self.rows.insert(bottom - 1, Row::new(self.cols));

        // Optionally push to scrollback if top == 0
        if top == 0 {
            self.push_scrollback(removed)
        } else {
            0
        }
    }

    /// Scroll a region down by one line
    pub fn scroll_region_down(&mut self, top: usize, bottom: usize) {
        if top >= bottom || bottom > self.rows.len() {
            return;
        }

        // Remove the bottom line of the region
        self.rows.remove(bottom - 1);

        // Insert new line at top of region
        self.rows.insert(top, Row::new(self.cols));
    }

    /// Scroll a partial-width rectangle up by one row without affecting
    /// scrollback or cells outside the rectangle.
    pub fn scroll_rect_up(
        &mut self,
        top: usize,
        bottom: usize,
        left: usize,
        right: usize,
        blank: &Cell,
        mut damage: impl FnMut(usize, std::ops::Range<usize>),
    ) {
        let bottom = bottom.min(self.rows.len());
        let right = right.min(self.cols);
        if top >= bottom || left >= right {
            return;
        }
        for row in top..bottom - 1 {
            let (destinations, sources) = self.rows.split_at_mut(row + 1);
            destinations[row]
                .copy_range_from(&sources[0], left, right, blank, |cols| damage(row, cols));
        }
        self.rows[bottom - 1]
            .erase_range_with_damage(left, right, blank, |cols| damage(bottom - 1, cols));
        self.rows[bottom - 1].set_wrapped(false);
    }

    /// Scroll a partial-width rectangle down by one row without affecting
    /// scrollback or cells outside the rectangle.
    pub fn scroll_rect_down(
        &mut self,
        top: usize,
        bottom: usize,
        left: usize,
        right: usize,
        blank: &Cell,
        mut damage: impl FnMut(usize, std::ops::Range<usize>),
    ) {
        let bottom = bottom.min(self.rows.len());
        let right = right.min(self.cols);
        if top >= bottom || left >= right {
            return;
        }
        for row in (top + 1..bottom).rev() {
            let (sources, destinations) = self.rows.split_at_mut(row);
            destinations[0].copy_range_from(&sources[row - 1], left, right, blank, |cols| {
                damage(row, cols)
            });
        }
        self.rows[top].erase_range_with_damage(left, right, blank, |cols| damage(top, cols));
        self.rows[top].set_wrapped(false);
    }

    /// Clear all visible cells
    pub fn clear(&mut self) {
        for row in &mut self.rows {
            row.clear();
        }
    }

    /// Clear a specific row
    pub fn clear_row(&mut self, row: usize) {
        if let Some(r) = self.rows.get_mut(row) {
            r.clear();
        }
    }

    /// Resize the grid with text reflow when columns change.
    /// Accepts cursor position and returns the new cursor position after reflow.
    pub fn resize(
        &mut self,
        new_cols: usize,
        new_rows: usize,
        cursor_row: usize,
        cursor_col: usize,
    ) -> (usize, usize) {
        let old_cols = self.cols;
        let mut new_cursor = (cursor_row, cursor_col);

        if new_cols != old_cols {
            // A wrapped history tail continues into the first visible row.
            // Keep that one logical line with the viewport reflow so the
            // storage boundary never becomes an artificial hard line break.
            let mut old_scrollback = std::mem::take(&mut self.scrollback);
            let boundary = if self.rows.is_empty() {
                old_scrollback.len()
            } else {
                old_scrollback
                    .iter()
                    .rposition(|row| !row.wrapped)
                    .map_or(0, |index| index + 1)
            };
            let continuation = old_scrollback.split_off(boundary);
            let prefix_rows = continuation.len();
            self.scrollback = Self::reflow_scrollback(old_scrollback, new_cols);
            self.recalculate_scrollback_bytes();

            // Expand only the crossing history line, moving its graphemes.
            // Its rows are all wrapped; into_reflow_cells removes synthetic
            // padding before these cells reach the shared logical-line path.
            let old_visible = std::mem::take(&mut self.rows);
            let rows = continuation
                .into_iter()
                .map(|row| Row::from_cells(row.into_reflow_cells(), true))
                .chain(old_visible);
            let cursor_info = Some((cursor_row.saturating_add(prefix_rows), cursor_col));
            let (reflowed, tracked_cursor) =
                Self::reflow_rows_with_cursor(rows, new_cols, cursor_info);
            self.rows = reflowed;
            if let Some(pos) = tracked_cursor {
                new_cursor = pos;
            }
        }

        // Adjust number of visible rows
        while self.rows.len() < new_rows {
            self.rows.push(Row::new(new_cols));
        }

        // When shrinking, remove excess rows in three phases to keep the
        // cursor visible and preserve as much content as possible:
        //   1. Trim trailing empty rows (no data loss, no cursor impact)
        //   2. Push rows from the top to scrollback, but stop before
        //      pushing the cursor row itself (keeps cursor on-screen)
        //   3. If still too many, pop from the bottom (content below cursor)
        let mut rows_pushed_top = 0usize;

        // Phase 1: trim trailing empty rows
        while self.rows.len() > new_rows {
            if self
                .rows
                .last()
                .map(|r| !r.is_wrapped() && r.iter().all(Cell::is_history_default))
                .unwrap_or(false)
            {
                self.rows.pop();
            } else {
                break;
            }
        }

        // Phase 2: push from top to scrollback, preserving cursor visibility
        if self.rows.len() > new_rows {
            let excess = self.rows.len() - new_rows;
            // Don't push more rows than the cursor position — that would
            // push the cursor into scrollback and lose its correct position.
            let max_push = excess.min(new_cursor.0);
            // Move the archive prefix in one batch. Repeated remove(0) would
            // shift a long boundary-spanning line quadratically.
            if max_push > 0 {
                let visible = self.rows.split_off(max_push);
                for row in std::mem::replace(&mut self.rows, visible) {
                    self.push_scrollback(row);
                }
            }
            rows_pushed_top = max_push;
        }

        // Phase 3: if still too many, pop from bottom (content below cursor)
        while self.rows.len() > new_rows {
            self.rows.pop();
        }

        // Adjust cursor for rows pushed to scrollback
        if rows_pushed_top > 0 {
            new_cursor.0 = new_cursor.0.saturating_sub(rows_pushed_top);
        }

        // Clamp cursor to visible area
        new_cursor.0 = new_cursor.0.min(new_rows.saturating_sub(1));
        new_cursor.1 = new_cursor.1.min(new_cols.saturating_sub(1));

        // Enforce scrollback limit after reflow (reflow can change line count)
        self.enforce_scrollback_limits();

        self.cols = new_cols;
        new_cursor
    }

    /// Reflow compact history one logical line at a time. This avoids expanding
    /// the complete archive into a second full-width rich-cell buffer.
    fn reflow_scrollback(rows: VecDeque<CompactRow>, new_cols: usize) -> VecDeque<CompactRow> {
        let mut result = VecDeque::new();
        let mut logical_line = Vec::new();

        for mut row in rows {
            if logical_line.is_empty() && row.retarget_width_without_reflow(new_cols) {
                result.push_back(row);
                continue;
            }
            let wrapped = row.wrapped;
            logical_line.extend(row.into_reflow_cells());
            if !wrapped {
                let mut reflowed = Vec::new();
                Self::rewrap_line(&mut reflowed, std::mem::take(&mut logical_line), new_cols);
                result.extend(reflowed.into_iter().map(CompactRow::from_row));
            }
        }
        if !logical_line.is_empty() {
            let mut reflowed = Vec::new();
            Self::rewrap_line(&mut reflowed, logical_line, new_cols);
            result.extend(reflowed.into_iter().map(CompactRow::from_row));
        }
        result
    }

    fn recalculate_scrollback_bytes(&mut self) {
        self.scrollback_bytes = self.scrollback.iter().map(CompactRow::owned_bytes).sum();
    }

    /// Reflow rows with optional cursor tracking.
    ///
    /// If `cursor` is Some((row, col)), tracks that position through the reflow
    /// and returns the new (row, col) in the output. This is critical for keeping
    /// the cursor at the correct position after resize, which prevents the shell's
    /// SIGWINCH response from corrupting content.
    fn reflow_rows_with_cursor(
        rows: impl IntoIterator<Item = Row>,
        new_cols: usize,
        cursor: Option<(usize, usize)>,
    ) -> (Vec<Row>, Option<(usize, usize)>) {
        let mut result: Vec<Row> = Vec::new();
        let mut new_cursor: Option<(usize, usize)> = None;

        // Group rows into logical lines using the `wrapped` flag.
        let mut line_cells: Vec<Cell> = Vec::new();
        let mut in_logical_line = false;
        // Track cursor: which cell index in the current logical line corresponds to the cursor?
        let mut cursor_cell_offset: Option<usize> = None;

        for (row_idx, row) in rows.into_iter().enumerate() {
            let is_wrapped = row.is_wrapped();
            let row_len = row.len();
            let cells = row.into_reflow_cells();

            if in_logical_line {
                // Check if cursor is in this row of the logical line
                if let Some((cr, cc)) = cursor
                    && cr == row_idx
                    && cursor_cell_offset.is_none()
                {
                    cursor_cell_offset = Some(line_cells.len() + cc.min(row_len.saturating_sub(1)));
                }
                line_cells.extend(cells);
            } else {
                // Check if cursor is in this row
                if let Some((cr, cc)) = cursor
                    && cr == row_idx
                {
                    cursor_cell_offset = Some(cc.min(row_len.saturating_sub(1)));
                }
                line_cells = cells;
            }

            if is_wrapped {
                in_logical_line = true;
            } else {
                // End of logical line — reflow and emit
                let output_start = result.len();
                let mapped_cursor = Self::rewrap_line_with_cursor(
                    &mut result,
                    line_cells,
                    new_cols,
                    cursor_cell_offset.take(),
                );

                // If cursor was in this logical line, compute its new position
                if let Some(cell_offset) = mapped_cursor {
                    new_cursor = Some(Self::find_cursor_in_output(
                        &result,
                        output_start,
                        cell_offset,
                        new_cols,
                    ));
                }

                line_cells = Vec::new();
                in_logical_line = false;
            }
        }

        // Flush any remaining logical line
        if !line_cells.is_empty() {
            let output_start = result.len();
            let mapped_cursor = Self::rewrap_line_with_cursor(
                &mut result,
                line_cells,
                new_cols,
                cursor_cell_offset.take(),
            );

            if let Some(cell_offset) = mapped_cursor {
                new_cursor = Some(Self::find_cursor_in_output(
                    &result,
                    output_start,
                    cell_offset,
                    new_cols,
                ));
            }
        }

        (result, new_cursor)
    }

    /// Find cursor position in the output rows given a cell offset within a logical line.
    ///
    /// `output_start` is the index of the first row in `output` that belongs to this logical line.
    /// `cell_offset` is the cursor's position as a cell index within the flattened logical line.
    fn find_cursor_in_output(
        output: &[Row],
        output_start: usize,
        cell_offset: usize,
        new_cols: usize,
    ) -> (usize, usize) {
        // Rewrapping has already translated the source offset through inserted
        // padding/spacers and removed spacers. Only final row/column conversion
        // and the existing trailing-empty-cell clamp remain.
        if new_cols == 0 {
            return (output_start.min(output.len().saturating_sub(1)), 0);
        }
        let row_within_line = cell_offset / new_cols;
        let col_within_line = cell_offset % new_cols;
        let output_row = output_start + row_within_line;
        // Clamp to actual output bounds
        let clamped_row = output_row.min(output.len().saturating_sub(1));
        let clamped_col = if clamped_row < output.len() {
            col_within_line.min(output[clamped_row].len().saturating_sub(1))
        } else {
            col_within_line
        };
        (clamped_row, clamped_col)
    }

    /// Re-wrap a single logical line (flat cell buffer) at the given column width
    /// and append the resulting rows to `output`.
    fn rewrap_line(output: &mut Vec<Row>, cells: Vec<Cell>, cols: usize) {
        let _ = Self::rewrap_line_with_cursor(output, cells, cols, None);
    }

    /// Translate one cursor offset alongside the cell moves, without allocating
    /// a per-cell mapping. Comparisons use original source indices; only the
    /// returned offset includes transformations before the cursor.
    fn rewrap_line_with_cursor(
        output: &mut Vec<Row>,
        mut cells: Vec<Cell>,
        cols: usize,
        cursor: Option<usize>,
    ) -> Option<usize> {
        use super::cell::CellFlags;
        let mut mapped_cursor = cursor;

        // Only default padding is disposable. Text-empty cells can still own
        // backgrounds, rendition, protection, hyperlinks, or block geometry.
        // Use the same lossless omission rule as compact scrollback.
        while cells.last().is_some_and(Cell::is_history_default) {
            cells.pop();
        }

        // Empty logical line → emit one empty row
        if cells.is_empty() {
            output.push(Row::new(cols));
            return mapped_cursor;
        }

        if cols == 0 {
            output.push(Row::new(0));
            return mapped_cursor;
        }

        // Re-wrap by moving cells into their new rows. History resize used to
        // clone every rich cell here, doubling allocation traffic.
        let mut cells = cells.into_iter().enumerate().peekable();
        while cells.peek().is_some() {
            let mut row_cells: Vec<Cell> = Vec::with_capacity(cols);
            let mut wide_wrap_padding = false;

            while row_cells.len() < cols && cells.peek().is_some() {
                if cells.peek().is_some_and(|(_, cell)| {
                    !cell.multicell.is_present()
                        && (cell.flags.contains(CellFlags::WIDE_CHAR)
                            || (!cell.content.is_empty()
                                && crate::unicode::grapheme_width(&cell.content.to_text()) == 2))
                }) {
                    // A one-column grid cannot represent a pair. Preserve the
                    // grapheme in its sole cell without a wide flag and consume
                    // the old spacer; never discard user text during reflow.
                    if cols < 2 {
                        let (_, mut leader) = cells.next().expect("peeked wide cell exists");
                        leader.flags.remove(CellFlags::WIDE_CHAR);
                        row_cells.push(leader);
                        if cells
                            .peek()
                            .is_some_and(|(_, cell)| cell.is_wide_char_spacer())
                        {
                            let (index, _) = cells.next().expect("peeked spacer exists");
                            if cursor.is_some_and(|offset| offset >= index) {
                                mapped_cursor =
                                    mapped_cursor.map(|offset| offset.saturating_sub(1));
                            }
                        }
                        continue;
                    }
                    // Only 1 slot left on this row — pad and break to next row.
                    if row_cells.len() + 2 > cols {
                        let index = cells.peek().expect("peeked wide cell exists").0;
                        if cursor.is_some_and(|offset| offset >= index) {
                            mapped_cursor = mapped_cursor.map(|offset| offset.saturating_add(1));
                        }
                        row_cells.resize_with(cols, Cell::default);
                        wide_wrap_padding = true;
                        break;
                    }
                    let (index, mut leader) = cells.next().expect("peeked wide cell exists");
                    leader.flags.insert(CellFlags::WIDE_CHAR);
                    row_cells.push(leader);
                    // Push the spacer if it exists
                    if cells
                        .peek()
                        .is_some_and(|(_, cell)| cell.is_wide_char_spacer())
                    {
                        row_cells.push(cells.next().expect("peeked spacer exists").1);
                    } else {
                        // Rebuild coverage from the surviving leader after a
                        // one-column collapse. Preserve its hyperlink and
                        // rendition without cloning the grapheme payload.
                        let spacer = CellStyle::from_cell(
                            row_cells.last().expect("wide leader was just appended"),
                        )
                        .apply(CompactCellContent::WideSpacer);
                        row_cells.push(spacer);
                        if cursor.is_some_and(|offset| offset > index) {
                            mapped_cursor = mapped_cursor.map(|offset| offset.saturating_add(1));
                        }
                    }
                } else if cells
                    .peek()
                    .is_some_and(|(_, cell)| cell.is_wide_char_spacer())
                {
                    // Orphaned spacer (wide char was on previous row) — skip
                    let (index, _) = cells.next().expect("peeked spacer exists");
                    if cursor.is_some_and(|offset| offset > index) {
                        mapped_cursor = mapped_cursor.map(|offset| offset.saturating_sub(1));
                    }
                } else {
                    row_cells.push(cells.next().expect("peeked cell exists").1);
                }
            }

            let is_last = cells.peek().is_none();
            // Pad row to full width
            row_cells.resize_with(cols, Cell::default);
            // All rows except the last are wrapped
            let mut row = Row::from_cells(row_cells, !is_last);
            if wide_wrap_padding {
                row.mark_wide_wrap_padding();
            }
            output.push(row);
        }
        mapped_cursor
    }

    /// Get the number of lines in scrollback
    pub fn scrollback_len(&self) -> usize {
        self.scrollback.len()
    }

    /// Get a row from scrollback (0 = oldest)
    #[cfg(test)]
    fn scrollback_row(&self, idx: usize) -> Option<RowView<'_>> {
        self.scrollback.get(idx).map(RowView::history)
    }

    /// Clear the scrollback buffer
    pub fn clear_scrollback(&mut self) {
        self.scrollback.clear();
        self.scrollback.shrink_to_fit();
        self.scrollback_bytes = 0;
    }

    /// Get a row, accounting for scroll offset
    ///
    /// - Positive offset: visible grid (0 = top visible row)
    /// - Negative offset: scrollback (-1 = most recent scrollback line)
    pub fn row_at_offset(&self, offset: i32) -> Option<RowView<'_>> {
        if offset >= 0 {
            self.rows.get(offset as usize).map(RowView::visible)
        } else {
            let scrollback_idx = self.scrollback.len() as i32 + offset;
            if scrollback_idx >= 0 {
                self.scrollback
                    .get(scrollback_idx as usize)
                    .map(RowView::history)
            } else {
                None
            }
        }
    }

    pub fn scrollback_cell(&self, idx: usize, col: usize) -> Option<CellRef<'_>> {
        self.scrollback.get(idx)?.get(col).map(CellRef::Owned)
    }

    /// Iterate over all scrollback rows (oldest first)
    pub fn iter_scrollback(&self) -> impl Iterator<Item = RowView<'_>> {
        self.scrollback.iter().map(RowView::history)
    }

    /// Iterate over all visible rows
    pub fn iter_visible(&self) -> impl Iterator<Item = &Row> {
        self.rows.iter()
    }

    pub fn memory_usage(&self) -> GridMemoryUsage {
        let visible_bytes = self
            .rows
            .iter()
            .map(|row| size_of::<Row>() + row.capacity() * size_of::<Cell>())
            .sum();
        GridMemoryUsage {
            visible_bytes,
            scrollback_bytes: self.scrollback_bytes,
            grapheme_bytes: self.scrollback.iter().map(CompactRow::grapheme_bytes).sum(),
            style_bytes: self.scrollback.iter().map(CompactRow::style_bytes).sum(),
            hyperlink_reference_bytes: self
                .scrollback
                .iter()
                .map(CompactRow::hyperlink_reference_bytes)
                .sum(),
            scrollback_lines: self.scrollback.len(),
            scrollback_line_limit: self.scrollback_limit,
            scrollback_byte_limit: self.scrollback_byte_limit,
        }
    }

    pub fn collect_hyperlink_ids(&self, identifiers: &mut HashSet<u32>) {
        for row in &self.scrollback {
            row.collect_hyperlink_ids(identifiers);
        }
        for cell in self.rows.iter().flat_map(Row::iter) {
            if let Some(identifier) = cell.hyperlink_id {
                identifiers.insert(identifier);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_storage_creation() {
        let storage = GridStorage::new(80, 24);
        assert_eq!(storage.iter_visible().count(), 24);
        assert_eq!(storage.row(0).unwrap().len(), 80);
        assert_eq!(storage.scrollback_len(), 0);
    }

    #[test]
    fn test_scroll_up() {
        let mut storage = GridStorage::new(80, 3);

        // Set content in first row
        if let Some(cell) = storage.cell_mut(0, 0) {
            cell.set_char('A');
        }

        storage.scroll_up();

        // First row should now be in scrollback
        assert_eq!(storage.scrollback_len(), 1);

        // New first row should be empty
        assert!(storage.cell(0, 0).unwrap().is_empty());
    }

    #[test]
    fn test_scrollback_limit() {
        let mut storage = GridStorage::new(80, 1);
        storage.set_scrollback_limit(3);

        for _ in 0..5 {
            storage.scroll_up();
        }

        assert_eq!(storage.scrollback_len(), 3);
    }

    #[test]
    fn test_resize() {
        let mut storage = GridStorage::new(80, 24);
        storage.resize(100, 30, 0, 0);
        assert_eq!(storage.iter_visible().count(), 30);
        assert_eq!(storage.row(0).unwrap().len(), 100);
    }

    /// Helper: fill a row with characters starting from a given char
    fn fill_row(storage: &mut GridStorage, row_idx: usize, text: &str) {
        for (col, ch) in text.chars().enumerate() {
            if let Some(cell) = storage.cell_mut(row_idx, col) {
                cell.set_char(ch);
            }
        }
    }

    /// Helper: extract text content from a row
    fn row_text(storage: &GridStorage, row_idx: usize) -> String {
        storage.row(row_idx).unwrap().to_string()
    }

    #[test]
    fn test_reflow_shrink() {
        // 10-col row with "ABCDEFGHIJ" → shrink to 5 cols → 2 wrapped rows
        let mut storage = GridStorage::new(10, 1);
        fill_row(&mut storage, 0, "ABCDEFGHIJ");

        // Simulate autowrap: mark the row as wrapped since it was full
        storage.row_mut(0).unwrap().set_wrapped(true);

        // Actually we need a non-wrapped row to test shrink reflow.
        // A single non-wrapped 10-char row shrunk to 5 should produce 2 rows.
        storage.row_mut(0).unwrap().set_wrapped(false);

        storage.resize(5, 3, 0, 0);

        // Should have 3 visible rows (padded), first two contain content
        assert_eq!(storage.iter_visible().count(), 3);
        assert_eq!(row_text(&storage, 0), "ABCDE");
        assert_eq!(row_text(&storage, 1), "FGHIJ");
        // First row should be wrapped, second should not
        assert!(storage.row(0).unwrap().is_wrapped());
        assert!(!storage.row(1).unwrap().is_wrapped());
    }

    #[test]
    fn test_reflow_grow() {
        // Two 5-col wrapped rows → grow to 10 cols → single row
        let mut storage = GridStorage::new(5, 2);
        fill_row(&mut storage, 0, "ABCDE");
        fill_row(&mut storage, 1, "FGHIJ");
        storage.row_mut(0).unwrap().set_wrapped(true);

        storage.resize(10, 2, 0, 0);

        assert_eq!(row_text(&storage, 0), "ABCDEFGHIJ");
        assert!(!storage.row(0).unwrap().is_wrapped());
    }

    #[test]
    fn test_reflow_round_trip() {
        // 10 chars → shrink to 5 → grow back to 10 → same content
        let mut storage = GridStorage::new(10, 1);
        fill_row(&mut storage, 0, "ABCDEFGHIJ");

        storage.resize(5, 3, 0, 0);
        storage.resize(10, 1, 0, 0);

        assert_eq!(row_text(&storage, 0), "ABCDEFGHIJ");
    }

    #[test]
    fn reflow_joins_a_logical_line_across_the_history_viewport_boundary() {
        let mut storage = GridStorage::new(3, 2);
        let mut hard_line = Row::new(3);
        hard_line.get_mut(0).unwrap().set_char('!');
        storage.push_scrollback(hard_line);
        let mut prefix = Row::new(3);
        for (col, ch) in "abc".chars().enumerate() {
            prefix.get_mut(col).unwrap().set_char(ch);
        }
        prefix.set_wrapped(true);
        storage.push_scrollback(prefix);
        fill_row(&mut storage, 0, "def");
        storage.row_mut(0).unwrap().set_wrapped(true);
        fill_row(&mut storage, 1, "ghZ");
        let mut cursor = (1, 2);
        for cols in [9, 3, 5, 1, 9] {
            cursor = storage.resize(cols, 2, cursor.0, cursor.1);
            assert_eq!(
                storage.cell(cursor.0, cursor.1).unwrap().content.to_text(),
                "Z"
            );
            let mut logical_lines = Vec::new();
            let mut text = String::new();
            for index in -(storage.scrollback_len() as i32)..2 {
                let row = storage.row_at_offset(index).unwrap();
                text.push_str(&row.to_string());
                if !row.is_wrapped() {
                    logical_lines.push(std::mem::take(&mut text));
                }
            }
            logical_lines.retain(|line| !line.is_empty());
            assert_eq!(logical_lines, ["!", "abcdefghZ"], "cols={cols}");
            if cols == 9 {
                assert_eq!(storage.scrollback_len(), 1);
                assert_eq!(row_text(&storage, 0), "abcdefghZ");
                assert_eq!(cursor, (0, 8));
            }
        }
    }

    #[test]
    fn boundary_reflow_preserves_history_accounting_and_eviction_limits() {
        for (line_limit, byte_limit) in [(64, 65_536), (10_000, 2_048)] {
            let mut storage = GridStorage::new(4, 2);
            storage.set_scrollback_limit(line_limit);
            storage.set_scrollback_byte_limit(byte_limit);
            for _ in 0..32 {
                let mut prefix = Row::new(4);
                for cell in prefix.iter_mut() {
                    cell.set_char('a');
                }
                prefix.set_wrapped(true);
                storage.push_scrollback(prefix);
            }
            assert!(storage.scrollback_len() > 0);
            fill_row(&mut storage, 0, "abcd");
            storage.row_mut(0).unwrap().set_wrapped(true);
            fill_row(&mut storage, 1, "efgQ");
            let mut cursor = (1, 3);
            for cols in [1, 8, 3, 64] {
                cursor = storage.resize(cols, 2, cursor.0, cursor.1);
                assert_eq!(
                    storage.cell(cursor.0, cursor.1).unwrap().content.to_text(),
                    "Q"
                );
                let usage = storage.memory_usage();
                assert!(usage.scrollback_lines <= line_limit);
                assert!(usage.scrollback_bytes <= byte_limit);
                assert_eq!(
                    usage.scrollback_bytes,
                    storage
                        .scrollback
                        .iter()
                        .map(CompactRow::owned_bytes)
                        .sum::<usize>()
                );
            }
        }
    }

    #[test]
    fn test_reflow_hard_break_preserved() {
        // Two separate non-wrapped rows should stay as two rows
        let mut storage = GridStorage::new(10, 2);
        fill_row(&mut storage, 0, "Hello");
        fill_row(&mut storage, 1, "World");

        storage.resize(5, 4, 0, 0);

        assert_eq!(row_text(&storage, 0), "Hello");
        assert_eq!(row_text(&storage, 1), "World");
        assert!(!storage.row(0).unwrap().is_wrapped());
        assert!(!storage.row(1).unwrap().is_wrapped());
    }

    #[test]
    fn test_reflow_empty_rows_preserved() {
        // Empty row between content rows should be preserved
        let mut storage = GridStorage::new(10, 3);
        fill_row(&mut storage, 0, "AAA");
        // row 1 is empty
        fill_row(&mut storage, 2, "BBB");

        storage.resize(5, 5, 0, 0);

        assert_eq!(row_text(&storage, 0), "AAA");
        assert_eq!(row_text(&storage, 1), "");
        assert_eq!(row_text(&storage, 2), "BBB");
    }

    #[test]
    fn test_reflow_scrollback() {
        // Content in scrollback should also reflow
        let mut storage = GridStorage::new(10, 1);
        fill_row(&mut storage, 0, "ABCDEFGHIJ");
        storage.scroll_up(); // moves row to scrollback

        storage.resize(5, 1, 0, 0);

        // Scrollback should now have 2 rows
        assert_eq!(storage.scrollback_len(), 2);
        assert_eq!(storage.scrollback_row(0).unwrap().to_string(), "ABCDE");
        assert_eq!(storage.scrollback_row(1).unwrap().to_string(), "FGHIJ");
    }

    #[test]
    fn test_cursor_not_pushed_to_scrollback() {
        // When shrinking to very few rows, cursor must stay visible.
        // 10 rows of content, cursor at row 9 (bottom). Shrink to 3 rows.
        let mut storage = GridStorage::new(10, 10);
        for i in 0..10 {
            fill_row(&mut storage, i, &format!("LINE{}", i));
        }

        let (new_row, new_col) = storage.resize(10, 3, 9, 0);

        // Cursor should be at the bottom of the 3-row visible area
        assert_eq!(new_row, 2);
        assert_eq!(new_col, 0);
        // The visible area should contain the last 3 rows of content
        assert_eq!(row_text(&storage, 2), "LINE9");
    }

    #[test]
    fn reflow_cursor_tracks_wide_padding_and_one_column_collapse() {
        let mut row = Row::new(5);
        row.get_mut(0).unwrap().set_char('a');
        row.get_mut(1).unwrap().set_char('b');
        row.get_mut(2).unwrap().set_char('界');
        row.get_mut(2).unwrap().flags.insert(CellFlags::WIDE_CHAR);
        row.get_mut(3)
            .unwrap()
            .flags
            .insert(CellFlags::WIDE_CHAR_SPACER);
        row.get_mut(4).unwrap().set_char('Z');
        for (width, positions) in [
            (3, [(0, 0), (0, 1), (1, 0), (1, 1), (1, 2)]),
            (1, [(0, 0), (1, 0), (2, 0), (2, 0), (3, 0)]),
        ] {
            for (col, expected) in positions.into_iter().enumerate() {
                let (_, cursor) =
                    GridStorage::reflow_rows_with_cursor(vec![row.clone()], width, Some((0, col)));
                assert_eq!(cursor, Some(expected), "width={width} original_col={col}");
            }
        }
    }

    #[test]
    fn reflow_cursor_tracks_repaired_spacers_wrapped_lines_and_empty_tail() {
        let mut unmarked = Row::new(2);
        unmarked.get_mut(0).unwrap().set_char('界');
        unmarked.get_mut(1).unwrap().set_char('Z');
        let (output, cursor) =
            GridStorage::reflow_rows_with_cursor(vec![unmarked], 2, Some((0, 1)));
        assert_eq!(cursor, Some((1, 0)));
        assert_eq!(output[1].get(0).unwrap().content.to_text(), "Z");

        let mut orphan = Row::new(2);
        orphan
            .get_mut(0)
            .unwrap()
            .flags
            .insert(CellFlags::WIDE_CHAR_SPACER);
        orphan.get_mut(1).unwrap().set_char('Z');
        let (_, cursor) = GridStorage::reflow_rows_with_cursor(vec![orphan], 2, Some((0, 1)));
        assert_eq!(cursor, Some((0, 0)));

        let mut row = Row::new(5);
        row.get_mut(0).unwrap().set_char('a');
        row.get_mut(1).unwrap().set_char('b');
        row.get_mut(2).unwrap().set_char('界');
        row.get_mut(2).unwrap().flags.insert(CellFlags::WIDE_CHAR);
        row.get_mut(3)
            .unwrap()
            .flags
            .insert(CellFlags::WIDE_CHAR_SPACER);
        let (_, cursor) = GridStorage::reflow_rows_with_cursor(vec![row.clone()], 3, Some((0, 4)));
        assert_eq!(
            cursor,
            Some((1, 2)),
            "cursor in trimmed blank tail follows inserted padding"
        );
        row.get_mut(4).unwrap().set_char('Z');
        row.set_wrapped(true);
        let mut continuation = Row::new(5);
        continuation.get_mut(0).unwrap().set_char('Q');
        let (output, cursor) =
            GridStorage::reflow_rows_with_cursor(vec![row, continuation], 3, Some((1, 0)));
        assert_eq!(cursor, Some((2, 0)));
        assert_eq!(output[2].get(0).unwrap().content.to_text(), "Q");
        let (_, cursor) = GridStorage::reflow_rows_with_cursor(vec![Row::new(5)], 0, Some((0, 4)));
        assert_eq!(cursor, Some((0, 0)));
    }

    #[test]
    fn reflow_retains_styled_empty_tail_in_visible_and_compact_rows() {
        let mut variants = vec![Cell::default(); 7];
        variants[0].bg = Color::rgb(3, 5, 7);
        variants[1].bg_spec = ColorSpec::Indexed(0);
        variants[2].fg_spec = ColorSpec::Indexed(2);
        variants[3].attrs = CellAttrs::INVERSE;
        variants[4].underline_color_spec = Some(ColorSpec::Indexed(3));
        variants[5].flags = CellFlags::PROTECTED;
        variants[6].hyperlink_id = Some(17);
        variants[6].flags = CellFlags::HYPERLINK;
        for (case, cell) in variants.into_iter().enumerate() {
            let mut row = Row::new(5);
            row.get_mut(0).unwrap().set_char('A');
            *row.get_mut(4).unwrap() = cell.clone();
            let (visible, _) = GridStorage::reflow_rows_with_cursor(vec![row.clone()], 3, None);
            assert_eq!(visible.len(), 2, "case={case}");
            assert_eq!(visible[1].get(1), Some(&cell), "case={case}");
            let compact =
                GridStorage::reflow_scrollback(VecDeque::from([CompactRow::from_row(row)]), 3);
            assert_eq!(compact.len(), 2, "case={case}");
            assert_eq!(compact[1].get(1), Some(cell), "case={case}");
        }
    }

    #[test]
    fn height_shrink_archives_content_instead_of_dropping_styled_empty_bottom_row() {
        let mut storage = GridStorage::new(3, 3);
        fill_row(&mut storage, 0, "top");
        fill_row(&mut storage, 1, "mid");
        let blank = Cell {
            bg: Color::rgb(3, 5, 7),
            bg_spec: ColorSpec::Indexed(4),
            flags: CellFlags::PROTECTED,
            ..Cell::default()
        };
        *storage.row_mut(2).unwrap().get_mut(2).unwrap() = blank.clone();
        let cursor = storage.resize(3, 2, 2, 0);
        assert_eq!(cursor, (1, 0));
        assert_eq!(storage.scrollback_len(), 1);
        assert_eq!(storage.scrollback_row(0).unwrap().to_string(), "top");
        assert_eq!(row_text(&storage, 0), "mid");
        assert_eq!(storage.row(1).unwrap().get(2), Some(&blank));
    }

    #[test]
    fn height_shrink_trims_only_unwrapped_default_padding() {
        for wrapped in [false, true] {
            let mut storage = GridStorage::new(3, 3);
            fill_row(&mut storage, 0, "top");
            fill_row(&mut storage, 1, "mid");
            let row = storage.row_mut(2).unwrap();
            row.get_mut(2).unwrap().flags.insert(CellFlags::DIRTY);
            row.set_wrapped(wrapped);
            storage.resize(3, 2, 2, 0);
            assert_eq!(storage.scrollback_len(), usize::from(wrapped));
            assert_eq!(row_text(&storage, 0), if wrapped { "mid" } else { "top" });
            assert_eq!(storage.row(1).unwrap().is_wrapped(), wrapped);
        }
    }

    #[test]
    fn reflow_columns_match_visible_and_compact_rows_without_losing_real_blanks() {
        let mut row = Row::new(6);
        row.get_mut(0).unwrap().set_char('a');
        row.get_mut(1).unwrap().set_grapheme("👩\u{200d}💻");
        row.get_mut(1).unwrap().flags.insert(CellFlags::WIDE_CHAR);
        row.get_mut(2)
            .unwrap()
            .flags
            .insert(CellFlags::WIDE_CHAR_SPACER);
        row.get_mut(3).unwrap().bg_spec = ColorSpec::Indexed(4);
        row.get_mut(4).unwrap().set_char(' ');
        row.set_wrapped(true);
        row.mark_wide_wrap_padding();
        let expected = vec![0, 1, 3, 4];
        assert_eq!(RowView::visible(&row).reflow_unit_count(), expected.len());
        assert_eq!(
            RowView::visible(&row).reflow_columns().collect::<Vec<_>>(),
            expected
        );
        let compact = CompactRow::from_row(row.clone());
        assert_eq!(
            RowView::history(&compact).reflow_unit_count(),
            expected.len()
        );
        assert_eq!(
            RowView::history(&compact)
                .reflow_columns()
                .collect::<Vec<_>>(),
            expected
        );
        let (compact, _) = CompactRow::from_row_recycling(row.clone());
        assert_eq!(
            RowView::history(&compact).reflow_unit_count(),
            expected.len()
        );
        assert_eq!(
            RowView::history(&compact)
                .reflow_columns()
                .collect::<Vec<_>>(),
            expected
        );
        row.get_mut(5).unwrap().set_char(' ');
        assert_eq!(RowView::visible(&row).reflow_unit_count(), 5);
        assert_eq!(
            RowView::visible(&row).reflow_columns().collect::<Vec<_>>(),
            vec![0, 1, 3, 4, 5]
        );
        let mut narrow = Row::new(1);
        narrow.get_mut(0).unwrap().set_char('界');
        assert_eq!(
            RowView::visible(&narrow)
                .reflow_columns()
                .collect::<Vec<_>>(),
            vec![0]
        );
        row.set_wrapped(false);
        let mut compact = CompactRow::from_row(row);
        assert!(compact.retarget_width_without_reflow(12));
        let view = RowView::history(&compact);
        assert_eq!(view.reflow_unit_count(), 11);
        assert_eq!(view.reflow_unit_count(), view.reflow_columns().count());
        let empty = CompactRow::from_row(Row::new(3));
        assert_eq!(RowView::history(&empty).reflow_unit_count(), 3);
    }

    #[test]
    fn reflow_recreates_spacer_style_from_surviving_wide_leader() {
        let mut leader = Cell::default();
        leader.set_grapheme("👩\u{200d}💻");
        leader.flags = CellFlags::WIDE_CHAR | CellFlags::PROTECTED | CellFlags::HYPERLINK;
        leader.hyperlink_id = Some(7);
        leader.fg_spec = ColorSpec::Indexed(2);
        leader.bg_spec = ColorSpec::Indexed(4);
        leader.attrs = CellAttrs::BOLD;
        let style = CellStyle::from_cell(&leader);
        let spacer = style.apply(CompactCellContent::WideSpacer);
        let mut narrow = Vec::new();
        GridStorage::rewrap_line(&mut narrow, vec![leader, spacer], 1);
        let cells = narrow
            .into_iter()
            .flat_map(Row::into_reflow_cells)
            .collect();
        let mut wide = Vec::new();
        GridStorage::rewrap_line(&mut wide, cells, 2);
        assert_eq!(wide.len(), 1);
        assert_eq!(wide[0].get(0).unwrap().content.to_text(), "👩\u{200d}💻");
        let spacer = wide[0].get(1).unwrap();
        assert!(spacer.is_wide_char_spacer());
        assert_eq!(CellStyle::from_cell(spacer), style);
    }

    #[test]
    fn test_cursor_preserved_on_small_reflow() {
        // Reflow 80-col content to 10 cols with only 3 visible rows.
        // Cursor at prompt on last row must stay visible.
        let mut storage = GridStorage::new(20, 5);
        fill_row(&mut storage, 0, "ABCDEFGHIJKLMNOPQRST"); // 20 chars
        fill_row(&mut storage, 1, "UVWXYZ1234567890abcd"); // 20 chars
        // rows 2-3 empty
        fill_row(&mut storage, 4, "$ "); // prompt with cursor

        let (new_row, _new_col) = storage.resize(5, 3, 4, 2);

        // Cursor must be in the visible area [0..2]
        assert!(
            new_row < 3,
            "cursor row {} is outside visible area",
            new_row
        );
    }

    #[test]
    fn compact_history_preserves_sparse_rich_cells() {
        let mut row = Row::new(32);
        row.get_mut(0).unwrap().set_char('A');
        row.get_mut(0)
            .unwrap()
            .flags
            .insert(CellFlags::DEC_SPECIAL_GRAPHICS);
        row.get_mut(3).unwrap().set_grapheme("e\u{301}");
        row.get_mut(3)
            .unwrap()
            .flags
            .insert(CellFlags::DEC_UK_CHARSET);
        let styled = row.get_mut(7).unwrap();
        styled.set_char('界');
        styled.attrs = CellAttrs::BOLD | CellAttrs::CURLY_UNDERLINE;
        styled.fg = Color::rgb(1, 2, 3);
        styled.fg_spec = ColorSpec::TrueColor(styled.fg);
        styled.hyperlink_id = Some(42);
        styled.flags |= CellFlags::WIDE_CHAR | CellFlags::HYPERLINK;
        row.get_mut(8)
            .unwrap()
            .flags
            .insert(CellFlags::WIDE_CHAR_SPACER);
        let multicell = MultiCell::new(
            1,
            2,
            0,
            0,
            crate::grid::MultiCellAlignment::Start,
            crate::grid::MultiCellAlignment::Start,
            0,
            0,
        )
        .unwrap();
        row.get_mut(10).unwrap().set_grapheme("exact");
        row.get_mut(10).unwrap().multicell = multicell;
        row.get_mut(11).unwrap().multicell = multicell.at(1, 0).unwrap();

        let original = row.clone();
        let compact = CompactRow::from_row(row);
        assert_eq!(compact.content.len(), 12);
        assert!(compact.owned_bytes() < 32 * size_of::<Cell>());
        for column in 0..32 {
            let mut expected = original.get(column).unwrap().clone();
            expected.flags.remove(CellFlags::DIRTY);
            assert_eq!(compact.get(column).unwrap(), expected);
        }
    }

    #[test]
    fn recycling_compaction_matches_standard_compaction_and_clears_the_row() {
        let mut row = Row::new(12);
        row.set_wrapped(true);
        row.get_mut(0).unwrap().set_char('A');
        row.get_mut(1).unwrap().set_char('界');
        row.get_mut(1)
            .unwrap()
            .flags
            .insert(CellFlags::WIDE_CHAR | CellFlags::HYPERLINK);
        row.get_mut(1).unwrap().hyperlink_id = Some(7);
        row.get_mut(2)
            .unwrap()
            .flags
            .insert(CellFlags::WIDE_CHAR_SPACER | CellFlags::HYPERLINK);
        row.get_mut(2).unwrap().hyperlink_id = Some(7);
        row.get_mut(5).unwrap().attrs.insert(CellAttrs::ITALIC);

        let expected = CompactRow::from_row(row.clone());
        let (actual, recycled) = CompactRow::from_row_recycling(row);

        assert_eq!(actual.cols, expected.cols);
        assert_eq!(actual.wrapped, expected.wrapped);
        assert_eq!(actual.owned_bytes(), expected.owned_bytes());
        for column in 0..actual.cols {
            assert_eq!(actual.get(column), expected.get(column));
            assert_eq!(recycled.get(column), Some(&Cell::default()));
        }
        assert!(!recycled.is_wrapped());
    }

    #[test]
    fn wide_cell_structure_does_not_fragment_uniform_style_runs() {
        let mut row = Row::new(4);
        for cell in row.iter_mut() {
            cell.fg = Color::rgb(1, 2, 3);
            cell.fg_spec = ColorSpec::TrueColor(cell.fg);
        }
        row.get_mut(0).unwrap().set_char('A');
        row.get_mut(1).unwrap().set_char('界');
        row.get_mut(1).unwrap().flags.insert(CellFlags::WIDE_CHAR);
        row.get_mut(2)
            .unwrap()
            .flags
            .insert(CellFlags::WIDE_CHAR_SPACER);
        row.get_mut(3).unwrap().set_char('B');

        let compact = CompactRow::from_row(row);
        assert_eq!(compact.styles.len(), 1);
        assert!(compact.get(1).unwrap().flags.contains(CellFlags::WIDE_CHAR));
        assert!(
            compact
                .get(2)
                .unwrap()
                .flags
                .contains(CellFlags::WIDE_CHAR_SPACER)
        );
    }

    #[test]
    fn byte_budget_evicts_even_below_line_limit_and_releases_accounting() {
        let mut storage = GridStorage::new(80, 1);
        storage.set_scrollback_limit(10_000);
        storage.set_scrollback_byte_limit(4 * 1024);
        for line in 0..200 {
            fill_row(&mut storage, 0, &format!("line-{line:04}"));
            storage.scroll_up();
        }

        let usage = storage.memory_usage();
        assert!(usage.scrollback_bytes <= 4 * 1024);
        assert!(usage.scrollback_lines < 200);
        assert!(usage.style_bytes > 0);

        storage.set_scrollback_byte_limit(1);
        let usage = storage.memory_usage();
        assert_eq!(usage.scrollback_lines, 0);
        assert_eq!(usage.scrollback_bytes, 0);
    }

    #[test]
    fn compact_reflow_matches_full_row_reference_across_generated_cases() {
        let mut state = 0x8d26_75a3_u64;
        for new_cols in [3, 5, 9, 17, 31] {
            let mut rows = Vec::new();
            for row_index in 0..64 {
                let mut row = Row::new(24);
                for column in 0..24 {
                    state = state
                        .wrapping_mul(6_364_136_223_846_793_005)
                        .wrapping_add(1);
                    if !state.is_multiple_of(5) {
                        let cell = row.get_mut(column).unwrap();
                        cell.set_char(char::from(b'a' + (state % 26) as u8));
                        if state.is_multiple_of(7) {
                            cell.attrs.insert(CellAttrs::ITALIC);
                            cell.fg_spec = ColorSpec::Indexed((state % 16) as u8);
                        }
                    }
                }
                row.set_wrapped(row_index % 4 == 0);
                rows.push(row);
            }

            let reference = GridStorage::reflow_rows_with_cursor(rows.clone(), new_cols, None).0;
            let compact = rows.into_iter().map(CompactRow::from_row).collect();
            let actual = GridStorage::reflow_scrollback(compact, new_cols);
            assert_eq!(actual.len(), reference.len());
            for (actual, expected) in actual.iter().zip(reference) {
                assert_eq!(actual.wrapped, expected.is_wrapped());
                for column in 0..new_cols {
                    let mut expected_cell = expected.get(column).unwrap().clone();
                    expected_cell.flags.remove(CellFlags::DIRTY);
                    assert_eq!(actual.get(column).unwrap(), expected_cell);
                }
            }
        }
    }

    #[test]
    fn fitting_hard_broken_compact_rows_reuse_owned_buffers_during_resize() {
        let mut row = Row::new(24);
        for (column, character) in "styled retained row".chars().enumerate() {
            let cell = row.get_mut(column).unwrap();
            cell.set_char(character);
            cell.attrs.insert(CellAttrs::ITALIC);
            cell.fg_spec = ColorSpec::Indexed(3);
        }
        let compact = CompactRow::from_row(row);
        let content_pointer = compact.content.as_ptr();
        let style_pointer = compact.styles.as_ptr();

        let resized = GridStorage::reflow_scrollback(VecDeque::from([compact]), 80);
        let row = resized.front().unwrap();

        assert_eq!(row.cols, 80);
        assert!(!row.wrapped);
        assert_eq!(row.content.as_ptr(), content_pointer);
        assert_eq!(row.styles.as_ptr(), style_pointer);
        assert_eq!(row.get(0).unwrap().content, CellContent::Char('s'));
        assert_eq!(row.get(79).unwrap(), Cell::default());
    }
}
