//! Terminal row definition

use super::cell::Cell;

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
    pub fn insert_cells(&mut self, col: usize, count: usize) {
        let cols = self.cells.len();
        if count == 0 || col >= cols {
            return;
        }

        let shift_end = cols.saturating_sub(count);
        for c in (col..shift_end).rev() {
            self.cells[c + count] = self.cells[c].clone();
        }

        // Clear inserted cells
        for c in col..(col + count).min(cols) {
            self.cells[c].clear();
        }

        self.dirty = true;
    }

    /// Delete cells at a position, shifting content left
    pub fn delete_cells(&mut self, col: usize, count: usize) {
        let cols = self.cells.len();
        if count == 0 || col >= cols {
            return;
        }

        for c in col..cols {
            if let Some(src) = c.checked_add(count).filter(|&idx| idx < cols) {
                self.cells[c] = self.cells[src].clone();
            } else {
                self.cells[c].clear();
            }
        }

        self.dirty = true;
    }

    /// Erase cells from start to end (exclusive)
    pub fn erase_range(&mut self, start: usize, end: usize) {
        for c in start..end.min(self.cells.len()) {
            self.cells[c].clear();
        }
        self.dirty = true;
    }

    /// Get the string content of the row (for selection/copying)
    pub fn to_string(&self) -> String {
        let mut s = String::new();

        for cell in &self.cells {
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
        use super::cell::{Color, ColorSpec, CellAttrs, CellContent};

        let default_fg = Color::default_fg();
        let default_bg = Color::default_bg();

        let mut s = String::new();
        let mut cur_fg_spec = ColorSpec::Default;
        let mut cur_bg_spec = ColorSpec::Default;
        let mut cur_attrs = CellAttrs::empty();
        let mut has_style = false;

        // Find the last cell that has meaningful content: either non-empty text
        // OR non-default styling (colors/attributes).
        let last_meaningful = self.cells.iter().rposition(|c| {
            !c.content.is_empty()
                || c.fg != default_fg
                || c.bg != default_bg
                || c.attrs != CellAttrs::empty()
        }).unwrap_or(0);

        for (i, cell) in self.cells.iter().enumerate() {
            if i > last_meaningful {
                break;
            }

            let need_fg = cell.fg_spec != cur_fg_spec;
            let need_bg = cell.bg_spec != cur_bg_spec;
            let need_attrs = cell.attrs != cur_attrs;

            if need_fg || need_bg || need_attrs {
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
                    if cell.attrs.contains(CellAttrs::UNDERLINE) {
                        params.push("4".to_string());
                    }
                    if cell.attrs.contains(CellAttrs::INVERSE) {
                        params.push("7".to_string());
                    }
                    if cell.attrs.contains(CellAttrs::STRIKE) {
                        params.push("9".to_string());
                    }

                    // After reset, re-emit colors if non-default
                    Self::push_fg_spec(&mut params, &cell.fg_spec);
                    Self::push_bg_spec(&mut params, &cell.bg_spec);
                } else {
                    if need_fg {
                        Self::push_fg_spec(&mut params, &cell.fg_spec);
                    }
                    if need_bg {
                        Self::push_bg_spec(&mut params, &cell.bg_spec);
                    }
                }

                if !params.is_empty() {
                    s.push_str(&format!("\x1b[{}m", params.join(";")));
                    has_style = true;
                }

                cur_fg_spec = cell.fg_spec;
                cur_bg_spec = cell.bg_spec;
                cur_attrs = cell.attrs;
            }

            match &cell.content {
                CellContent::Empty => s.push(' '),
                CellContent::Char(c) => s.push(*c),
                CellContent::Grapheme(g) => s.push_str(g),
            }
        }

        if has_style {
            s.push_str("\x1b[0m");
        }

        s
    }

    /// Push SGR parameters for a foreground ColorSpec
    fn push_fg_spec(params: &mut Vec<String>, spec: &super::cell::ColorSpec) {
        use super::cell::ColorSpec;
        match spec {
            ColorSpec::Default => params.push("39".to_string()),
            ColorSpec::Indexed(n) => {
                match n {
                    0..=7 => params.push(format!("{}", 30 + n)),
                    8..=15 => params.push(format!("{}", 90 + n - 8)),
                    _ => params.push(format!("38;5;{}", n)),
                }
            }
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
            ColorSpec::Indexed(n) => {
                match n {
                    0..=7 => params.push(format!("{}", 40 + n)),
                    8..=15 => params.push(format!("{}", 100 + n - 8)),
                    _ => params.push(format!("48;5;{}", n)),
                }
            }
            ColorSpec::TrueColor(c) => {
                params.push(format!("48;2;{};{};{}", c.r, c.g, c.b));
            }
        }
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
}
