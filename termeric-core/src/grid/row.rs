//! Terminal row definition

use super::cell::{Cell, CellAttrs, CellFlags};

/// A single row in the terminal grid
#[derive(Clone, Debug)]
pub struct Row {
    /// Cells in this row
    cells: Vec<Cell>,
    /// True if this row was soft-wrapped (content continues on next row)
    wrapped: bool,
    /// Final default cell inserted only because a wide pair did not fit.
    wide_wrap_padding: bool,
    /// Lazy descriptor presence. Every cell
    /// mutation must invalidate or recompute it; exposed mutable references
    /// must invalidate it. Keeping
    /// it on the row lets unchanged rows retain it through scrolling.
    multicells_present: Option<bool>,
}

impl Row {
    /// Create a new row with the specified number of columns
    pub fn new(cols: usize) -> Self {
        Self {
            cells: vec![Cell::default(); cols],
            wrapped: false,
            wide_wrap_padding: false,
            multicells_present: Some(false),
        }
    }

    /// Reuse a discarded viewport row's allocation with exactly `new` state.
    /// Unlike `clear`, fresh cells have no per-cell DIRTY flags. Dropping old
    /// cells also releases graphemes and other content.
    pub(crate) fn into_blank(mut self, cols: usize) -> Self {
        self.cells.clear();
        self.cells.resize_with(cols, Cell::default);
        self.wrapped = false;
        self.wide_wrap_padding = false;
        self.multicells_present = Some(false);
        self
    }

    /// Create a row from a vec of cells
    pub fn from_cells(cells: Vec<Cell>, wrapped: bool) -> Self {
        Self {
            cells,
            wrapped,
            wide_wrap_padding: false,
            multicells_present: None,
        }
    }

    /// Get the number of columns in this row
    pub fn len(&self) -> usize {
        self.cells.len()
    }

    pub(crate) fn capacity(&self) -> usize {
        self.cells.capacity()
    }

    /// Check if the row is empty (no columns)
    pub fn is_empty(&self) -> bool {
        self.cells.is_empty()
    }

    /// Get a reference to a cell
    pub fn get(&self, col: usize) -> Option<&Cell> {
        self.cells.get(col)
    }

    /// Get a mutable reference to a cell
    pub fn get_mut(&mut self, col: usize) -> Option<&mut Cell> {
        if col.checked_add(1) == Some(self.cells.len()) {
            self.wide_wrap_padding = false;
        }
        self.multicells_present = None;
        self.cells.get_mut(col)
    }

    pub(crate) fn has_multicells(&mut self) -> bool {
        *self
            .multicells_present
            .get_or_insert_with(|| self.cells.iter().any(|cell| cell.multicell.is_present()))
    }

    /// Clear all cells in the row
    pub fn clear(&mut self) {
        for cell in &mut self.cells {
            cell.clear();
        }
        self.wrapped = false;
        self.wide_wrap_padding = false;
        self.multicells_present = Some(false);
    }

    /// Resize the row to a new number of columns
    pub fn resize(&mut self, cols: usize) {
        self.cells.resize_with(cols, Cell::default);
        self.repair_wide_cells();
    }

    /// Check if this row was soft-wrapped (content continues on next row)
    pub fn is_wrapped(&self) -> bool {
        self.wrapped
    }

    /// Set the soft-wrap flag
    pub fn set_wrapped(&mut self, wrapped: bool) {
        self.wrapped = wrapped;
        if !wrapped {
            self.wide_wrap_padding = false;
        }
    }

    pub(crate) fn mark_wide_wrap_padding(&mut self) {
        self.wide_wrap_padding =
            self.wrapped && self.cells.last().is_some_and(Cell::is_history_default);
    }

    pub(crate) fn has_wide_wrap_padding(&self) -> bool {
        self.wide_wrap_padding
            && self.wrapped
            && self.cells.last().is_some_and(Cell::is_history_default)
    }

    pub(crate) fn into_reflow_cells(mut self) -> Vec<Cell> {
        if self.has_wide_wrap_padding() {
            self.cells.pop();
        }
        self.cells
    }

    /// Consume the row and return its cells
    pub(crate) fn into_cells(self) -> Vec<Cell> {
        self.cells
    }

    /// Iterate over all cells in the row
    pub fn iter(&self) -> impl Iterator<Item = &Cell> {
        self.cells.iter()
    }

    /// Iterate over all cells mutably
    pub fn iter_mut(&mut self) -> impl Iterator<Item = &mut Cell> {
        self.wide_wrap_padding = false;
        self.multicells_present = None;
        self.cells.iter_mut()
    }

    /// Iterate over cells with their column index
    pub(crate) fn iter_enumerated(&self) -> impl Iterator<Item = (usize, &Cell)> {
        self.cells.iter().enumerate()
    }

    #[cfg(test)]
    fn insert_cells(&mut self, col: usize, count: usize, blank: &Cell) {
        self.insert_cells_bounded_with_damage(col, self.cells.len(), count, blank, |_| {});
    }

    #[cfg(test)]
    fn delete_cells(&mut self, col: usize, count: usize, blank: &Cell) {
        self.delete_cells_bounded_with_damage(col, self.cells.len(), count, blank, |_| {});
    }

    #[cfg(test)]
    fn erase_range(&mut self, start: usize, end: usize, blank: &Cell) {
        self.erase_range_with_damage(start, end, blank, |_| {});
    }

    #[cfg(test)]
    fn selective_erase_range(&mut self, start: usize, end: usize, blank: &Cell) {
        self.selective_erase_range_with_damage(start, end, blank, |_| {});
    }

    /// Report cells erased by glyph repair after moving the surviving cells.
    pub(crate) fn insert_cells_bounded_with_damage(
        &mut self,
        col: usize,
        right: usize,
        count: usize,
        blank: &Cell,
        damage: impl FnMut(std::ops::Range<usize>),
    ) {
        let right = right.min(self.cells.len());
        if count == 0 || col >= right {
            return;
        }

        let count = count.min(right - col);
        let shift_end = right - count;
        for c in (col..shift_end).rev() {
            self.cells[c + count] = self.cells[c].clone();
        }

        // Clear inserted cells
        for c in col..col + count {
            self.cells[c] = blank.clone();
            self.cells[c].flags.insert(CellFlags::DIRTY);
        }

        self.repair_wide_cells_with_damage(blank, damage);
    }

    /// Report cells erased by glyph repair after moving the surviving cells.
    pub(crate) fn delete_cells_bounded_with_damage(
        &mut self,
        col: usize,
        right: usize,
        count: usize,
        blank: &Cell,
        damage: impl FnMut(std::ops::Range<usize>),
    ) {
        let right = right.min(self.cells.len());
        if count == 0 || col >= right {
            return;
        }

        for c in col..right {
            if let Some(src) = c.checked_add(count).filter(|&idx| idx < right) {
                self.cells[c] = self.cells[src].clone();
            } else {
                self.cells[c] = blank.clone();
                self.cells[c].flags.insert(CellFlags::DIRTY);
            }
        }

        self.repair_wide_cells_with_damage(blank, damage);
    }

    /// Copy a same-column cell range from another row. Partial-width vertical
    /// scrolling is presentation layout, not a logical soft wrap, so it breaks
    /// any reflow link on the destination row.
    pub(crate) fn copy_range_from(
        &mut self,
        source: &Self,
        left: usize,
        right: usize,
        blank: &Cell,
        damage: impl FnMut(std::ops::Range<usize>),
    ) {
        let right = right.min(self.cells.len()).min(source.cells.len());
        let left = left.min(right);
        self.cells[left..right].clone_from_slice(&source.cells[left..right]);
        self.wrapped = false;
        self.repair_wide_cells_with_damage(blank, damage);
    }

    /// Replace a bounded range with an already snapshotted set of cells.
    /// Snapshotting at the caller makes overlapping rectangular copies safe.
    pub(crate) fn replace_range(
        &mut self,
        start: usize,
        cells: &[Cell],
        blank: &Cell,
        mut damage: impl FnMut(std::ops::Range<usize>),
    ) {
        let start = start.min(self.cells.len());
        let count = cells.len().min(self.cells.len().saturating_sub(start));
        if count == 0 {
            return;
        }
        damage(start..start + count);
        self.cells[start..start + count].clone_from_slice(&cells[..count]);
        for cell in &mut self.cells[start..start + count] {
            cell.flags.insert(CellFlags::DIRTY);
        }
        self.repair_wide_cells_with_damage(blank, damage);
    }

    /// Fill a bounded range with one attributed character cell.
    pub(crate) fn fill_range(
        &mut self,
        start: usize,
        end: usize,
        cell: &Cell,
        blank: &Cell,
        mut damage: impl FnMut(std::ops::Range<usize>),
    ) {
        let start = start.min(self.cells.len());
        let end = end.min(self.cells.len()).max(start);
        if start == end {
            return;
        }
        damage(start..end);
        for target in &mut self.cells[start..end] {
            *target = cell.clone();
            target.flags.insert(CellFlags::DIRTY);
        }
        self.repair_wide_cells_with_damage(blank, damage);
    }

    /// Apply a rendition-only operation to a cell range. Stream operations
    /// skip cells without drawn content; exact rectangles first materialize
    /// those cells as spaces. The row is dirtied only when state actually
    /// changes, avoiding redraw work for no-op attribute requests.
    pub(crate) fn change_attributes_range(
        &mut self,
        start: usize,
        end: usize,
        exact_rectangle: bool,
        mut apply: impl FnMut(&mut CellAttrs),
    ) {
        if start < self.cells.len() && end >= self.cells.len() {
            self.wide_wrap_padding = false;
        }
        self.multicells_present = None;
        let start = start.min(self.cells.len());
        let end = end.min(self.cells.len()).max(start);
        for cell in &mut self.cells[start..end] {
            let drawn = !cell.content.is_empty()
                || cell.is_wide_char_spacer()
                || cell.multicell.is_present();
            if !exact_rectangle && !drawn {
                continue;
            }
            if exact_rectangle && !drawn {
                cell.set_char(' ');
            }
            let previous = cell.attrs;
            apply(&mut cell.attrs);
            if cell.attrs != previous {
                cell.flags.insert(CellFlags::DIRTY);
            }
        }
    }

    /// Report the exact expanded glyph span replaced by this erase.
    pub(crate) fn erase_range_with_damage(
        &mut self,
        start: usize,
        end: usize,
        blank: &Cell,
        mut damage: impl FnMut(std::ops::Range<usize>),
    ) {
        let mut start = start.min(self.cells.len());
        let mut end = end.min(self.cells.len()).max(start);
        if start == end {
            return;
        }
        if start < self.cells.len() && self.cells[start].is_wide_char_spacer() {
            start = start.saturating_sub(1);
        }
        if end > 0 && self.cells[end - 1].is_wide_char() {
            end = (end + 1).min(self.cells.len());
        }
        damage(start..end);
        for c in start..end {
            self.cells[c] = blank.clone();
            self.cells[c].flags.insert(CellFlags::DIRTY);
        }
        if start == 0 && end == self.cells.len() {
            self.wrapped = false;
        }
        self.repair_wide_cells_with(blank);
    }

    /// Emit only unprotected grapheme spans actually erased, including both
    /// halves of a wide pair when the requested range touches just one half.
    pub(crate) fn selective_erase_range_with_damage(
        &mut self,
        start: usize,
        end: usize,
        blank: &Cell,
        mut damage: impl FnMut(std::ops::Range<usize>),
    ) {
        let start = start.min(self.cells.len());
        let end = end.min(self.cells.len()).max(start);
        let mut col = start;
        let mut preserved = false;

        while col < end {
            let (leader, width) = if self.cells[col].is_wide_char_spacer() && col > 0 {
                (col - 1, 2)
            } else if self.cells[col].is_wide_char() {
                (col, 2)
            } else {
                (col, 1)
            };
            let grapheme_end = (leader + width).min(self.cells.len());
            let protected = self.cells[leader..grapheme_end]
                .iter()
                .any(|cell| cell.flags.contains(CellFlags::PROTECTED));
            if protected {
                preserved = true;
            } else {
                damage(leader..grapheme_end);
                for target in leader..grapheme_end {
                    self.cells[target].clear_content_preserving_attrs();
                }
            }
            col = grapheme_end.max(col + 1);
        }

        if start == 0 && end == self.cells.len() && !preserved {
            self.wrapped = false;
        }
        self.repair_wide_cells_with(blank);
    }

    /// Clear a grapheme atomically when an edit targets either half of a wide
    /// cell pair.
    pub(crate) fn clear_grapheme_at(&mut self, col: usize) {
        self.wide_wrap_padding = false;
        self.multicells_present = None;
        if col >= self.cells.len() {
            return;
        }
        if self.cells[col].flags.contains(CellFlags::WIDE_CHAR_SPACER) {
            if col > 0 {
                self.cells[col - 1].clear();
            }
            self.cells[col].clear();
        } else if self.cells[col].flags.contains(CellFlags::WIDE_CHAR) {
            self.cells[col].clear();
            if col + 1 < self.cells.len() {
                self.cells[col + 1].clear();
            }
        } else {
            self.cells[col].clear();
        }
    }

    /// Remove orphan leaders/spacers after cell-based edit operations.
    pub(crate) fn repair_wide_cells(&mut self) {
        self.repair_wide_cells_with(&Cell::default());
    }

    /// Remove orphan leaders/spacers using the caller's active blank-cell
    /// background policy.
    fn repair_wide_cells_with(&mut self, blank: &Cell) {
        self.repair_wide_cells_with_damage(blank, |_| {});
    }

    fn repair_wide_cells_with_damage(
        &mut self,
        blank: &Cell,
        mut damage: impl FnMut(std::ops::Range<usize>),
    ) {
        // Structural edits materialize any old wrap padding as an ordinary
        // blank rather than allowing moved metadata to discard edited cells.
        self.wide_wrap_padding = false;
        // Range/resize edits delegate here. Their inserted/copied blank cells
        // can carry descriptors too, so invalidate even if no wide pair needs
        // repair. Other mutation paths handle their own cache invalidation.
        self.multicells_present = None;
        for col in 0..self.cells.len() {
            self.repair_wide_cell_with_damage(col, blank, &mut damage);
        }
    }

    /// A new multicell range contains no ordinary wide halves. Only the cells
    /// immediately outside it can become orphaned; avoid a whole-row scan for
    /// every naturally segmented OSC 66 grapheme.
    pub(crate) fn repair_multicell_placement_edges(
        &mut self,
        start: usize,
        end: usize,
        blank: &Cell,
        mut damage: impl FnMut(std::ops::Range<usize>),
    ) {
        if start >= end || end > self.cells.len() {
            return;
        }
        self.wide_wrap_padding = false;
        self.multicells_present = None;
        if start > 0 {
            self.repair_wide_cell_with_damage(start - 1, blank, &mut damage);
        }
        if end < self.cells.len() {
            self.repair_wide_cell_with_damage(end, blank, &mut damage);
        }
    }

    fn repair_wide_cell_with_damage(
        &mut self,
        col: usize,
        blank: &Cell,
        damage: &mut impl FnMut(std::ops::Range<usize>),
    ) {
        let orphaned = if self.cells[col].is_wide_char() {
            !self
                .cells
                .get(col + 1)
                .is_some_and(Cell::is_wide_char_spacer)
        } else {
            self.cells[col].is_wide_char_spacer()
                && (col == 0 || !self.cells[col - 1].is_wide_char())
        };
        if orphaned {
            damage(col..col + 1);
            self.cells[col] = blank.clone();
            self.cells[col].flags.insert(CellFlags::DIRTY);
        }
    }

    fn plain_text(&self) -> String {
        let mut s = String::new();

        for cell in &self.cells {
            if cell.is_wide_char_spacer()
                || (cell.multicell.is_present() && !cell.multicell.is_leader())
            {
                continue;
            }
            match &cell.content {
                super::cell::CellContent::Empty => s.push(' '),
                super::cell::CellContent::Char(c) => s.push(*c),
                super::cell::CellContent::Grapheme(g) => s.push_str(g),
            }
        }

        // Trim trailing whitespace
        s.trim_end().to_string()
    }

    /// Get the string content with ANSI escape sequences for colors and attributes.
    ///
    /// Emits SGR sequences using the original color specification (indexed, true color,
    /// or default) so that replaying through a terminal re-resolves indexed colors
    /// through the current theme. This allows session restore to adapt to theme changes.
    pub fn to_ansi_string(&self) -> String {
        use super::cell::{CellAttrs, CellContent, Color, ColorSpec};

        let default_fg = Color::default_fg();
        let default_bg = Color::default_bg();

        let mut s = String::new();
        let mut cur_fg_spec = ColorSpec::Default;
        let mut cur_bg_spec = ColorSpec::Default;
        let mut cur_attrs = CellAttrs::empty();
        let mut cur_underline_color = None;
        let mut cur_underline_color_spec = None;
        let mut cur_protected = false;
        let mut has_style = false;

        // Find the last cell that has meaningful content: either non-empty text
        // OR non-default styling (colors/attributes).
        let last_meaningful = self
            .cells
            .iter()
            .rposition(|c| {
                !c.is_wide_char_spacer()
                    && (!c.multicell.is_present() || c.multicell.is_leader())
                    && (!c.content.is_empty()
                        || c.fg != default_fg
                        || c.bg != default_bg
                        || c.attrs != CellAttrs::empty()
                        || c.underline_color.is_some()
                        || c.underline_color_spec.is_some()
                        || c.flags.contains(CellFlags::PROTECTED))
            })
            .unwrap_or(0);

        for (i, cell) in self.cells.iter().enumerate() {
            if i > last_meaningful {
                break;
            }
            if cell.is_wide_char_spacer() {
                continue;
            }
            if cell.multicell.is_present() && !cell.multicell.is_leader() {
                if cell.multicell.relative_row() > 0 && cell.multicell.relative_col() == 0 {
                    s.push_str(&format!("\x1b[{}C", cell.multicell.columns()));
                }
                continue;
            }

            let protected = cell.flags.contains(CellFlags::PROTECTED);
            if protected != cur_protected {
                s.push_str(if protected { "\x1b[1\"q" } else { "\x1b[0\"q" });
                cur_protected = protected;
            }

            let need_fg = cell.fg_spec != cur_fg_spec;
            let need_bg = cell.bg_spec != cur_bg_spec;
            let need_attrs = cell.attrs != cur_attrs;
            let need_underline_color = cell.underline_color != cur_underline_color
                || cell.underline_color_spec != cur_underline_color_spec;

            if need_fg || need_bg || need_attrs || need_underline_color {
                let mut params: Vec<String> = Vec::new();

                if need_attrs {
                    // Reset and re-apply everything
                    params.push("0".to_string());

                    if cell.attrs.contains(CellAttrs::BOLD) {
                        params.push("1".to_string());
                    }
                    if cell.attrs.contains(CellAttrs::DIM) {
                        params.push("2".to_string());
                    }
                    if cell.attrs.contains(CellAttrs::ITALIC) {
                        params.push("3".to_string());
                    }
                    if cell.attrs.contains(CellAttrs::CURLY_UNDERLINE) {
                        params.push("4:3".to_string());
                    } else if cell.attrs.contains(CellAttrs::DOUBLE_UNDERLINE) {
                        params.push("4:2".to_string());
                    } else if cell.attrs.contains(CellAttrs::DOTTED_UNDERLINE) {
                        params.push("4:4".to_string());
                    } else if cell.attrs.contains(CellAttrs::DASHED_UNDERLINE) {
                        params.push("4:5".to_string());
                    } else if cell.attrs.contains(CellAttrs::UNDERLINE) {
                        params.push("4".to_string());
                    }
                    if cell.attrs.contains(CellAttrs::BLINK) {
                        params.push("5".to_string());
                    }
                    if cell.attrs.contains(CellAttrs::RAPID_BLINK) {
                        params.push("6".to_string());
                    }
                    if cell.attrs.contains(CellAttrs::INVERSE) {
                        params.push("7".to_string());
                    }
                    if cell.attrs.contains(CellAttrs::HIDDEN) {
                        params.push("8".to_string());
                    }
                    if cell.attrs.contains(CellAttrs::STRIKE) {
                        params.push("9".to_string());
                    }

                    // After reset, re-emit colors if non-default
                    Self::push_fg_spec(&mut params, &cell.fg_spec);
                    Self::push_bg_spec(&mut params, &cell.bg_spec);
                    if let Some(spec) = cell.underline_color_spec {
                        Self::push_extended_color_spec(&mut params, 58, spec);
                    }
                } else {
                    if need_fg {
                        Self::push_fg_spec(&mut params, &cell.fg_spec);
                    }
                    if need_bg {
                        Self::push_bg_spec(&mut params, &cell.bg_spec);
                    }
                    if need_underline_color {
                        if let Some(spec) = cell.underline_color_spec {
                            Self::push_extended_color_spec(&mut params, 58, spec);
                        } else {
                            params.push("59".to_string());
                        }
                    }
                }

                if !params.is_empty() {
                    s.push_str(&format!("\x1b[{}m", params.join(";")));
                    has_style = true;
                }

                cur_fg_spec = cell.fg_spec;
                cur_bg_spec = cell.bg_spec;
                cur_attrs = cell.attrs;
                cur_underline_color = cell.underline_color;
                cur_underline_color_spec = cell.underline_color_spec;
            }

            if cell.multicell.is_leader() {
                let descriptor = cell.multicell;
                let mut metadata =
                    format!("s={}:w={}", descriptor.scale(), descriptor.logical_width());
                if descriptor.denominator() != 0 {
                    metadata.push_str(&format!(
                        ":n={}:d={}",
                        descriptor.numerator(),
                        descriptor.denominator()
                    ));
                }
                if descriptor.vertical_alignment() as u8 != 0 {
                    metadata.push_str(&format!(":v={}", descriptor.vertical_alignment() as u8));
                }
                if descriptor.horizontal_alignment() as u8 != 0 {
                    metadata.push_str(&format!(":h={}", descriptor.horizontal_alignment() as u8));
                }
                s.push_str("\x1b]66;");
                s.push_str(&metadata);
                s.push(';');
                s.push_str(&cell.content.to_text());
                s.push('\x07');
            } else {
                match &cell.content {
                    CellContent::Empty => s.push(' '),
                    CellContent::Char(c) => s.push(*c),
                    CellContent::Grapheme(g) => s.push_str(g),
                }
            }
        }

        if has_style {
            s.push_str("\x1b[0m");
        }
        if cur_protected {
            s.push_str("\x1b[0\"q");
        }

        s
    }

    /// Push SGR parameters for a foreground ColorSpec
    fn push_fg_spec(params: &mut Vec<String>, spec: &super::cell::ColorSpec) {
        use super::cell::ColorSpec;
        match spec {
            ColorSpec::Default => params.push("39".to_string()),
            ColorSpec::Indexed(n) => match n {
                0..=7 => params.push(format!("{}", 30 + n)),
                8..=15 => params.push(format!("{}", 90 + n - 8)),
                _ => params.push(format!("38;5;{}", n)),
            },
            ColorSpec::TrueColor(c) => {
                params.push(format!("38;2;{};{};{}", c.r, c.g, c.b));
            }
        }
    }

    /// Push SGR parameters for a background ColorSpec
    fn push_bg_spec(params: &mut Vec<String>, spec: &super::cell::ColorSpec) {
        use super::cell::ColorSpec;
        match spec {
            ColorSpec::Default => params.push("49".to_string()),
            ColorSpec::Indexed(n) => match n {
                0..=7 => params.push(format!("{}", 40 + n)),
                8..=15 => params.push(format!("{}", 100 + n - 8)),
                _ => params.push(format!("48;5;{}", n)),
            },
            ColorSpec::TrueColor(c) => {
                params.push(format!("48;2;{};{};{}", c.r, c.g, c.b));
            }
        }
    }

    fn push_extended_color_spec(
        params: &mut Vec<String>,
        selector: u8,
        spec: super::cell::ColorSpec,
    ) {
        use super::cell::ColorSpec;
        match spec {
            ColorSpec::Default => {}
            ColorSpec::Indexed(index) => params.push(format!("{selector};5;{index}")),
            ColorSpec::TrueColor(color) => {
                params.push(format!("{selector};2;{};{};{}", color.r, color.g, color.b))
            }
        }
    }
}

impl std::fmt::Display for Row {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.plain_text())
    }
}

impl std::ops::Index<usize> for Row {
    type Output = Cell;

    fn index(&self, col: usize) -> &Self::Output {
        &self.cells[col]
    }
}

impl std::ops::IndexMut<usize> for Row {
    fn index_mut(&mut self, col: usize) -> &mut Self::Output {
        self.multicells_present = None;
        &mut self.cells[col]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{CellAttrs, Color, ColorSpec};

    #[test]
    fn erase_damage_reports_atomic_wide_spans_and_skips_protected_pairs() {
        let mut terminal = crate::Terminal::new(4, 2);
        terminal.process("A界B".as_bytes());
        let mut row = terminal.grid().row(0).unwrap().clone();
        row.get_mut(0).unwrap().flags.insert(CellFlags::PROTECTED);
        // Protecting only the spacer still preserves the complete pair.
        row.get_mut(2).unwrap().flags.insert(CellFlags::PROTECTED);
        let blank = Cell::default();
        let mut damage = Vec::new();
        row.erase_range_with_damage(2, 2, &blank, |range| damage.push(range));
        assert!(damage.is_empty());
        assert_eq!(row.to_string(), "A界B");
        row.selective_erase_range_with_damage(0, 4, &blank, |range| damage.push(range));
        assert_eq!(damage.len(), 1);
        assert_eq!(damage[0], 3..4);
        assert_eq!(row.to_string(), "A界");
        damage.clear();
        row.erase_range_with_damage(2, 3, &blank, |range| damage.push(range));
        assert_eq!(damage.len(), 1);
        assert_eq!(damage[0], 1..3);
        assert_eq!(row.to_string(), "A");
    }
    fn descriptor_cell() -> Cell {
        use crate::grid::{MultiCell, MultiCellAlignment};
        Cell {
            multicell: MultiCell::new(
                2,
                1,
                0,
                0,
                MultiCellAlignment::Start,
                MultiCellAlignment::Start,
                0,
                0,
            )
            .unwrap(),
            ..Cell::with_char('x')
        }
    }

    #[test]
    fn recycled_blank_has_fresh_row_state_and_reuses_capacity() {
        for cols in [0, 2, 8, 16] {
            let mut row = Row::new(8);
            row[0] = descriptor_cell();
            row[1].set_grapheme("e\u{301}");
            row[1].hyperlink_id = Some(17);
            row[1].attrs.insert(CellAttrs::BOLD);
            row[1]
                .flags
                .insert(CellFlags::HYPERLINK | CellFlags::PROTECTED);
            row.set_wrapped(true);
            row.mark_wide_wrap_padding();
            assert!(row.has_wide_wrap_padding());
            assert!(row.has_multicells());
            let pointer = row.cells.as_ptr();
            let row = row.into_blank(cols);
            assert_eq!(row.cells, vec![Cell::default(); cols]);
            assert!(!row.is_wrapped());
            assert!(!row.has_wide_wrap_padding());
            assert_eq!(row.multicells_present, Some(false));
            if cols <= 8 {
                assert_eq!(row.cells.as_ptr(), pointer);
                assert_eq!(row.capacity(), 8);
            }
        }
    }

    #[test]
    fn multicell_presence_cache_tracks_all_cell_mutation_routes() {
        type Edit = fn(&mut Row, &Cell);
        let edits: &[(&str, Edit)] = &[
            ("get_mut", |row, cell| {
                *row.get_mut(3).unwrap() = cell.clone()
            }),
            ("index_mut", |row, cell| row[3] = cell.clone()),
            ("iter_mut", |row, cell| {
                *row.iter_mut().nth(3).unwrap() = cell.clone()
            }),
            ("clear", |row, _| row.clear()),
            ("resize", |row, _| row.resize(2)),
            ("insert", |row, cell| row.insert_cells(2, 1, cell)),
            ("delete", |row, cell| row.delete_cells(2, 1, cell)),
            ("copy", |row, cell| {
                row.copy_range_from(
                    &Row::from_cells(vec![cell.clone(); 8], false),
                    2,
                    5,
                    &Cell::default(),
                    |_| {},
                )
            }),
            ("replace", |row, cell| {
                row.replace_range(2, std::slice::from_ref(cell), &Cell::default(), |_| {})
            }),
            ("fill", |row, cell| {
                row.fill_range(2, 5, cell, &Cell::default(), |_| {})
            }),
            ("attributes", |row, _| {
                row.change_attributes_range(0, 8, true, |attrs| attrs.insert(CellAttrs::BOLD))
            }),
            ("erase", |row, cell| row.erase_range(0, 8, cell)),
            ("selective_erase", |row, cell| {
                row.selective_erase_range(0, 8, cell)
            }),
            ("clear_grapheme", |row, _| row.clear_grapheme_at(3)),
            ("repair_wide", |row, _| row.repair_wide_cells()),
        ];
        for (name, edit) in edits {
            for initial in [Cell::default(), descriptor_cell()] {
                for replacement in [Cell::default(), descriptor_cell()] {
                    let mut row = Row::new(8);
                    row[3] = initial.clone();
                    // Also exercise descriptor removal by wide-cell repair.
                    row[3].flags.insert(CellFlags::WIDE_CHAR);
                    assert_eq!(row.has_multicells(), initial.multicell.is_present());
                    edit(&mut row, &replacement);
                    let expected = row.iter().any(|cell| cell.multicell.is_present());
                    assert_eq!(row.has_multicells(), expected, "{name}");
                    assert_eq!(row.multicells_present, Some(expected), "{name}");
                }
            }
        }
    }

    #[test]
    fn wide_wrap_padding_is_validated_and_retired_by_edits() {
        let mut row = Row::new(3);
        row.set_wrapped(true);
        row.mark_wide_wrap_padding();
        assert!(row.has_wide_wrap_padding());
        assert_eq!(row.clone().into_reflow_cells().len(), 2);
        let mut copy = row.clone();
        copy.get_mut(2).unwrap().set_char(' ');
        assert!(!copy.has_wide_wrap_padding());
        assert_eq!(copy.into_reflow_cells().len(), 3);
        for mutation in 0..5 {
            let mut copy = row.clone();
            match mutation {
                0 => {
                    copy.iter_mut().next();
                }
                1 => copy.insert_cells(0, 1, &Cell::default()),
                2 => copy.change_attributes_range(2, 3, true, |_| {}),
                3 => copy.clear(),
                _ => copy.set_wrapped(false),
            }
            assert!(!copy.has_wide_wrap_padding(), "mutation={mutation}");
        }
        row.get_mut(2).unwrap().bg_spec = super::super::ColorSpec::Indexed(4);
        row.mark_wide_wrap_padding();
        assert!(!row.has_wide_wrap_padding());
    }

    #[test]
    fn multicell_presence_cache_is_row_owned_and_independent_of_render_dirtiness() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<Row>();
        // The byte-sized cache fits in the previous row header's padding.
        let old_size = (size_of::<Vec<Cell>>() + 2).next_multiple_of(align_of::<Vec<Cell>>());
        assert_eq!(size_of::<Row>(), old_size);

        let mut row = Row::from_cells(vec![descriptor_cell()], true);
        assert_eq!(row.multicells_present, None);
        assert!(row.has_multicells());
        let mut clone = row.clone();
        row.clear();
        assert!(!row.has_multicells());
        assert!(clone.has_multicells());
        clone.set_wrapped(false);
        assert_eq!(clone.multicells_present, Some(true));
        clone.clear();
        for _ in 0..100 {
            assert!(!clone.has_multicells());
            assert_eq!(clone.multicells_present, Some(false));
        }
    }

    #[test]
    fn test_row_creation() {
        let row = Row::new(80);
        assert_eq!(row.len(), 80);
    }

    #[test]
    fn test_row_clear() {
        let mut row = Row::new(80);
        row[0].set_char('A');
        row.clear();
        assert!(row[0].is_empty());
    }

    #[test]
    fn test_row_resize() {
        let mut row = Row::new(80);
        row.resize(100);
        assert_eq!(row.len(), 100);
        row.resize(40);
        assert_eq!(row.len(), 40);
    }

    #[test]
    fn test_row_to_string() {
        let mut row = Row::new(10);
        row[0].set_char('H');
        row[1].set_char('i');
        let s = row.to_string();
        assert_eq!(s, "Hi");
    }

    #[test]
    fn ansi_export_preserves_every_supported_sgr_attribute() {
        let mut row = Row::new(3);
        row[0].set_char('A');
        row[0].attrs = CellAttrs::BOLD
            | CellAttrs::DIM
            | CellAttrs::ITALIC
            | CellAttrs::RAPID_BLINK
            | CellAttrs::INVERSE
            | CellAttrs::HIDDEN
            | CellAttrs::STRIKE
            | CellAttrs::CURLY_UNDERLINE;
        row[0].underline_color = Some(Color::rgb(12, 34, 56));
        row[0].underline_color_spec = Some(ColorSpec::TrueColor(Color::rgb(12, 34, 56)));
        row[1].set_char('B');

        let exported = row.to_ansi_string();
        let mut restored = crate::Terminal::new(3, 1);
        restored.process(exported.as_bytes());
        let first = restored.grid().cell(0, 0).unwrap();
        assert_eq!(first.attrs, row[0].attrs);
        assert_eq!(first.underline_color, row[0].underline_color);
        assert_eq!(first.underline_color_spec, row[0].underline_color_spec);
        assert!(restored.grid().cell(0, 1).unwrap().attrs.is_empty());
        assert_eq!(restored.grid().cell(0, 1).unwrap().underline_color, None);
    }

    #[test]
    fn ansi_export_preserves_indexed_underline_color_identity() {
        let mut row = Row::new(1);
        row[0].set_char(crate::IMAGE_PLACEHOLDER);
        row[0].fg_spec = ColorSpec::Indexed(42);
        row[0].fg = Color::rgb(1, 2, 3);
        row[0].underline_color_spec = Some(ColorSpec::Indexed(7));
        row[0].underline_color = Some(Color::rgb(4, 5, 6));

        let exported = row.to_ansi_string();
        assert!(exported.contains("38;5;42"));
        assert!(exported.contains("58;5;7"));
        let mut restored = crate::Terminal::new(1, 1);
        restored.process(exported.as_bytes());
        let cell = restored.grid().cell(0, 0).unwrap();
        assert_eq!(cell.fg_spec, ColorSpec::Indexed(42));
        assert_eq!(cell.underline_color_spec, Some(ColorSpec::Indexed(7)));
    }

    #[test]
    fn ansi_export_preserves_dec_character_protection_without_leaking() {
        let mut row = Row::new(3);
        row[0].set_char('A');
        row[0].flags.insert(CellFlags::PROTECTED);
        row[1].set_char('B');

        let exported = row.to_ansi_string();
        assert!(exported.contains("\x1b[1\"qA\x1b[0\"qB"));

        let mut restored = crate::Terminal::new(3, 1);
        restored.process(exported.as_bytes());
        assert!(
            restored
                .grid()
                .cell(0, 0)
                .unwrap()
                .flags
                .contains(CellFlags::PROTECTED)
        );
        assert!(
            !restored
                .grid()
                .cell(0, 1)
                .unwrap()
                .flags
                .contains(CellFlags::PROTECTED)
        );
        restored.process(b"C");
        assert!(
            !restored
                .grid()
                .cell(0, 2)
                .unwrap()
                .flags
                .contains(CellFlags::PROTECTED)
        );
    }
}
