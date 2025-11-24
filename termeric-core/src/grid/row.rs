//! Terminal row definition

use super::cell::Cell;

/// A single row in the terminal grid
#[derive(Clone, Debug)]
pub struct Row {
    /// Cells in this row
    cells: Vec<Cell>,
    /// Whether this row has been modified since last render
    dirty: bool,
}

impl Row {
    /// Create a new row with the specified number of columns
    pub fn new(cols: usize) -> Self {
        Self {
            cells: vec![Cell::default(); cols],
            dirty: true,
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

        // Shift cells right
        for c in (col..cols).rev() {
            if c + count < cols {
                self.cells.swap(c, c + count);
            }
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

        // Shift cells left
        for c in col..cols {
            if c + count < cols {
                self.cells.swap(c, c + count);
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
