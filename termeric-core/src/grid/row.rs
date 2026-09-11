//! Terminal row definition

use super::cell::{Cell, CellAttrs, CellFlags};

/// A single row in the terminal grid
#[derive(Clone, Debug)]
pub struct Row {
    /// Cells in this row
    cells: Vec<Cell>,
    /// Whether this row has been modified since last render
    dirty: bool,
    /// True if this row was soft-wrapped (content continues on next row)
    wrapped: bool,
}

impl Row {
    /// Create a new row with the specified number of columns
    pub fn new(cols: usize) -> Self {
        Self {
            cells: vec![Cell::default(); cols],
            dirty: true,
            wrapped: false,
        }
    }

    /// Create a row from a vec of cells
    pub fn from_cells(cells: Vec<Cell>, wrapped: bool) -> Self {
        Self {
            cells,
            dirty: true,
            wrapped,
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
        self.dirty = true;
        self.cells.get_mut(col)
    }

    /// Clear all cells in the row
    pub fn clear(&mut self) {
        for cell in &mut self.cells {
            cell.clear();
        }
        self.dirty = true;
        self.wrapped = false;
    }

    /// Resize the row to a new number of columns
    pub fn resize(&mut self, cols: usize) {
        self.cells.resize_with(cols, Cell::default);
        self.repair_wide_cells();
        self.dirty = true;
    }

    /// Check if the row is dirty (needs re-rendering)
    pub fn is_dirty(&self) -> bool {
        self.dirty
    }

    /// Mark the row as clean
    pub fn set_clean(&mut self) {
        self.dirty = false;
    }

    /// Mark the row as dirty
    pub fn set_dirty(&mut self) {
        self.dirty = true;
    }

    /// Check if this row was soft-wrapped (content continues on next row)
    pub fn is_wrapped(&self) -> bool {
        self.wrapped
    }

    /// Set the soft-wrap flag
    pub fn set_wrapped(&mut self, wrapped: bool) {
        self.wrapped = wrapped;
    }

    /// Consume the row and return its cells
    pub fn into_cells(self) -> Vec<Cell> {
        self.cells
    }

    /// Iterate over all cells in the row
    pub fn iter(&self) -> impl Iterator<Item = &Cell> {
        self.cells.iter()
    }

    /// Iterate over all cells mutably
    pub fn iter_mut(&mut self) -> impl Iterator<Item = &mut Cell> {
        self.dirty = true;
        self.cells.iter_mut()
    }

    /// Iterate over cells with their column index
    pub fn iter_enumerated(&self) -> impl Iterator<Item = (usize, &Cell)> {
        self.cells.iter().enumerate()
    }

    /// Insert blank cells at a position, shifting content right
    pub fn insert_cells(&mut self, col: usize, count: usize, blank: &Cell) {
        self.insert_cells_bounded(col, self.cells.len(), count, blank);
    }

    /// Insert blank cells, shifting only within `[col, right)`.
    pub(crate) fn insert_cells_bounded(
        &mut self,
        col: usize,
        right: usize,
        count: usize,
        blank: &Cell,
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

        self.repair_wide_cells_with(blank);
        self.dirty = true;
    }

    /// Delete cells at a position, shifting content left
    pub fn delete_cells(&mut self, col: usize, count: usize, blank: &Cell) {
        self.delete_cells_bounded(col, self.cells.len(), count, blank);
    }

    /// Delete cells, shifting only within `[col, right)`.
    pub(crate) fn delete_cells_bounded(
        &mut self,
        col: usize,
        right: usize,
        count: usize,
        blank: &Cell,
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

        self.repair_wide_cells_with(blank);
        self.dirty = true;
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
    ) {
        let right = right.min(self.cells.len()).min(source.cells.len());
        let left = left.min(right);
        self.cells[left..right].clone_from_slice(&source.cells[left..right]);
        self.wrapped = false;
        self.repair_wide_cells_with(blank);
        self.dirty = true;
    }

    /// Replace a bounded range with an already snapshotted set of cells.
    /// Snapshotting at the caller makes overlapping rectangular copies safe.
    pub(crate) fn replace_range(&mut self, start: usize, cells: &[Cell], blank: &Cell) {
        let start = start.min(self.cells.len());
        let count = cells.len().min(self.cells.len().saturating_sub(start));
        if count == 0 {
            return;
        }
        self.cells[start..start + count].clone_from_slice(&cells[..count]);
        for cell in &mut self.cells[start..start + count] {
            cell.flags.insert(CellFlags::DIRTY);
        }
        self.repair_wide_cells_with(blank);
        self.dirty = true;
    }

    /// Fill a bounded range with one attributed character cell.
    pub(crate) fn fill_range(&mut self, start: usize, end: usize, cell: &Cell, blank: &Cell) {
        let start = start.min(self.cells.len());
        let end = end.min(self.cells.len()).max(start);
        if start == end {
            return;
        }
        for target in &mut self.cells[start..end] {
            *target = cell.clone();
            target.flags.insert(CellFlags::DIRTY);
        }
        self.repair_wide_cells_with(blank);
        self.dirty = true;
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
        let start = start.min(self.cells.len());
        let end = end.min(self.cells.len()).max(start);
        let mut changed = false;
        for cell in &mut self.cells[start..end] {
            let drawn = !cell.content.is_empty()
                || cell.is_wide_char_spacer()
                || cell.multicell.is_present();
            if !exact_rectangle && !drawn {
                continue;
            }
            if exact_rectangle && !drawn {
                cell.set_char(' ');
                changed = true;
            }
            let previous = cell.attrs;
            apply(&mut cell.attrs);
            if cell.attrs != previous {
                cell.flags.insert(CellFlags::DIRTY);
                changed = true;
            }
        }
        self.dirty |= changed;
    }

    /// Erase cells from start to end (exclusive)
    pub fn erase_range(&mut self, start: usize, end: usize, blank: &Cell) {
        let mut start = start.min(self.cells.len());
        let mut end = end.min(self.cells.len()).max(start);
        if start < self.cells.len() && self.cells[start].is_wide_char_spacer() {
            start = start.saturating_sub(1);
        }
        if end > 0 && self.cells[end - 1].is_wide_char() {
            end = (end + 1).min(self.cells.len());
        }
        for c in start..end {
            self.cells[c] = blank.clone();
            self.cells[c].flags.insert(CellFlags::DIRTY);
        }
        if start == 0 && end == self.cells.len() {
            self.wrapped = false;
        }
        self.repair_wide_cells_with(blank);
        self.dirty = true;
    }

    /// Erase the content of unprotected graphemes while retaining every
    /// cell's visual and DECSCA protection attributes. Wide leader/spacer
    /// pairs are treated atomically: intersecting either half erases both only
    /// when neither half is protected.
    pub fn selective_erase_range(&mut self, start: usize, end: usize, blank: &Cell) {
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
        self.dirty = true;
    }

    /// Clear a grapheme atomically when an edit targets either half of a wide
    /// cell pair.
    pub fn clear_grapheme_at(&mut self, col: usize) {
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
        self.dirty = true;
    }

    /// Remove orphan leaders/spacers after cell-based edit operations.
    pub fn repair_wide_cells(&mut self) {
        self.repair_wide_cells_with(&Cell::default());
    }

    /// Remove orphan leaders/spacers using the caller's active blank-cell
    /// background policy.
    fn repair_wide_cells_with(&mut self, blank: &Cell) {
        for col in 0..self.cells.len() {
            if self.cells[col].flags.contains(CellFlags::WIDE_CHAR) {
                let paired = self
                    .cells
                    .get(col + 1)
                    .is_some_and(|cell| cell.flags.contains(CellFlags::WIDE_CHAR_SPACER));
                if !paired {
                    self.cells[col] = blank.clone();
                    self.cells[col].flags.insert(CellFlags::DIRTY);
                }
            } else if self.cells[col].flags.contains(CellFlags::WIDE_CHAR_SPACER) {
                let paired = col > 0 && self.cells[col - 1].flags.contains(CellFlags::WIDE_CHAR);
                if !paired {
                    self.cells[col] = blank.clone();
                    self.cells[col].flags.insert(CellFlags::DIRTY);
                }
            }
        }
        self.dirty = true;
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
        self.dirty = true;
        &mut self.cells[col]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{CellAttrs, Color, ColorSpec};

    #[test]
    fn test_row_creation() {
        let row = Row::new(80);
        assert_eq!(row.len(), 80);
        assert!(row.is_dirty());
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
