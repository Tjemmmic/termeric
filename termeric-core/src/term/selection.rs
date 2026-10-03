//! Text selection handling

/// A point in the terminal grid
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Point {
    /// Row index (can be negative for scrollback)
    pub row: i32,
    /// Column index
    pub col: usize,
}

impl Point {
    /// Create a new point
    pub fn new(row: i32, col: usize) -> Self {
        Self { row, col }
    }
}

impl PartialOrd for Point {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Point {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        match self.row.cmp(&other.row) {
            std::cmp::Ordering::Equal => self.col.cmp(&other.col),
            ord => ord,
        }
    }
}

/// Type of text selection
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SelectionType {
    /// Normal character selection (click and drag)
    #[default]
    Simple,
    /// Block/rectangular selection (Alt+click and drag)
    Block,
    /// Semantic selection (double-click selects word)
    Semantic,
    /// Line selection (triple-click selects line)
    Lines,
}

/// Text selection state
#[derive(Clone, Debug)]
pub struct Selection {
    /// Start point of selection (where mouse was first pressed)
    pub start: Point,
    /// End point of selection (current mouse position)
    pub end: Point,
    /// Type of selection
    pub ty: SelectionType,
    /// Whether the selection is being actively modified
    pub active: bool,
}

impl Selection {
    /// Create a new selection starting at a point
    pub fn new(start: Point, ty: SelectionType) -> Self {
        Self {
            start,
            end: start,
            ty,
            active: true,
        }
    }

    /// Update the end point of the selection
    pub fn update(&mut self, point: Point) {
        self.end = point;
    }

    /// Finish the selection (mouse released)
    pub fn finish(&mut self) {
        self.active = false;
    }

    /// Whether any selected cell intersects a visible edit rectangle. First,
    /// last and interior rows cover every column shape without scanning the
    /// selection or incorrectly filling the gap between two stream endpoints.
    pub(crate) fn intersects_region(
        &self,
        rows: std::ops::Range<usize>,
        cols: std::ops::Range<usize>,
        total_cols: usize,
    ) -> bool {
        if rows.is_empty() || cols.is_empty() || total_cols == 0 {
            return false;
        }
        let (Ok(top), Ok(bottom)) = (i32::try_from(rows.start), i32::try_from(rows.end)) else {
            return false;
        };
        let (start, end) = self.bounds();
        let first = start.row.max(top);
        let last = end.row.min(bottom - 1);
        first <= last
            && [first, last, first.saturating_add(1).min(last)]
                .into_iter()
                .any(|row| {
                    self.col_range(row, total_cols).is_some_and(|range| {
                        !range.is_empty() && *range.start() < cols.end && *range.end() >= cols.start
                    })
                })
    }

    /// Follow one vertical rectangle move without inventing a discontiguous
    /// selection. Return false if selected cells are erased or only part of
    /// the selection moves. Full-screen upward history archival is different:
    /// its source rows survive and are handled by the terminal's history path.
    pub(super) fn scroll_in_region(
        &mut self,
        rows: std::ops::Range<usize>,
        cols: std::ops::Range<usize>,
        delta: i32,
        total_cols: usize,
    ) -> bool {
        if rows.is_empty() || cols.is_empty() || delta == 0 || total_cols == 0 {
            return true;
        }
        let (Ok(top), Ok(bottom)) = (i32::try_from(rows.start), i32::try_from(rows.end)) else {
            return false;
        };
        let (start, end) = self.bounds();
        // Stream selections have at most three column shapes: first row,
        // last row, and interior rows. Block and line selections are uniform.
        // Sampling these shapes keeps even very long history ranges O(1).
        let sample_rows = |first: i32, last: i32| [first, last, first.saturating_add(1).min(last)];
        if !self.intersects_region(rows, cols.clone(), total_cols) {
            return true;
        }
        if start.row < top || end.row >= bottom {
            return false;
        }
        if !sample_rows(start.row, end.row).into_iter().all(|row| {
            self.col_range(row, total_cols).is_some_and(|range| {
                !range.is_empty() && *range.start() >= cols.start && *range.end() < cols.end
            })
        }) {
            return false;
        }
        let (Some(first), Some(last)) = (start.row.checked_add(delta), end.row.checked_add(delta))
        else {
            return false;
        };
        if first < top || last >= bottom {
            return false;
        }
        self.start.row += delta;
        self.end.row += delta;
        true
    }

    /// Follow one horizontal row edit. A selection can stay stationary or
    /// translate intact; erased cells or mixed motion retire the whole range.
    pub(super) fn shift_columns_in_row(
        &mut self,
        row: usize,
        cols: std::ops::Range<usize>,
        delta: isize,
        total_cols: usize,
    ) -> bool {
        if delta == 0
            || !self.intersects_region(row..row.saturating_add(1), cols.clone(), total_cols)
        {
            return true;
        }
        let Ok(row) = i32::try_from(row) else {
            return false;
        };
        if self.start.row != row || self.end.row != row {
            return false;
        }
        let Some(range) = self.col_range(row, total_cols) else {
            return false;
        };
        if *range.start() < cols.start || *range.end() >= cols.end {
            return false;
        }
        let (Some(first), Some(last)) = (
            range.start().checked_add_signed(delta),
            range.end().checked_add_signed(delta),
        ) else {
            return false;
        };
        if first < cols.start || last >= cols.end {
            return false;
        }
        let (Some(start), Some(end)) = (
            self.start.col.checked_add_signed(delta),
            self.end.col.checked_add_signed(delta),
        ) else {
            return false;
        };
        self.start.col = start;
        self.end.col = end;
        true
    }

    /// Get the normalized bounds (start <= end)
    pub fn bounds(&self) -> (Point, Point) {
        if self.start <= self.end {
            (self.start, self.end)
        } else {
            (self.end, self.start)
        }
    }

    /// Check if a point is within the selection
    pub fn contains(&self, row: i32, col: usize) -> bool {
        let (start, end) = self.bounds();
        let point = Point::new(row, col);

        match self.ty {
            SelectionType::Simple | SelectionType::Semantic => {
                // Simple selection spans from start to end
                if point.row < start.row || point.row > end.row {
                    return false;
                }

                if start.row == end.row {
                    // Single line selection
                    return point.col >= start.col && point.col <= end.col;
                }

                if point.row == start.row {
                    return point.col >= start.col;
                }

                if point.row == end.row {
                    return point.col <= end.col;
                }

                // Point is on a middle row
                true
            }
            SelectionType::Block => {
                // Block selection is a rectangle
                let min_col = start.col.min(end.col);
                let max_col = start.col.max(end.col);

                point.row >= start.row && point.row <= end.row && col >= min_col && col <= max_col
            }
            SelectionType::Lines => {
                // Line selection includes entire rows
                point.row >= start.row && point.row <= end.row
            }
        }
    }

    /// Check if the selection is empty (start == end)
    pub fn is_empty(&self) -> bool {
        self.start == self.end
    }

    /// Get the range of rows included in the selection
    pub(crate) fn row_range(&self) -> std::ops::RangeInclusive<i32> {
        let (start, end) = self.bounds();
        start.row..=end.row
    }

    /// Get the column range for a specific row
    pub(crate) fn col_range(
        &self,
        row: i32,
        total_cols: usize,
    ) -> Option<std::ops::RangeInclusive<usize>> {
        if !self.row_range().contains(&row) {
            return None;
        }

        let (start, end) = self.bounds();

        match self.ty {
            SelectionType::Simple | SelectionType::Semantic => {
                let start_col = if row == start.row { start.col } else { 0 };
                let end_col = if row == end.row {
                    end.col
                } else {
                    total_cols.saturating_sub(1)
                };
                Some(start_col..=end_col)
            }
            SelectionType::Block => {
                let min_col = start.col.min(end.col);
                let max_col = start.col.max(end.col).min(total_cols.saturating_sub(1));
                Some(min_col..=max_col)
            }
            SelectionType::Lines => Some(0..=total_cols.saturating_sub(1)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn horizontal_selection_mapping_matches_cellwise_uniform_motion() {
        let points: Vec<_> = (-1..3)
            .flat_map(|row| (0..4).map(move |col| Point::new(row, col)))
            .collect();
        for ty in [
            SelectionType::Simple,
            SelectionType::Semantic,
            SelectionType::Lines,
            SelectionType::Block,
        ] {
            for &start in &points {
                for &end in &points {
                    let mut original = Selection::new(start, ty);
                    original.update(end);
                    original.finish();
                    for row in 0..3 {
                        for cols in [0..4, 1..4, 1..3, 2..2, 0..1] {
                            for delta in [-4_isize, -2, -1, 0, 1, 2, 4] {
                                let mut uniform = None;
                                let mut expected = true;
                                for point in points
                                    .iter()
                                    .filter(|point| original.contains(point.row, point.col))
                                {
                                    let movement =
                                        if point.row == row as i32 && cols.contains(&point.col) {
                                            delta
                                        } else {
                                            0
                                        };
                                    if movement != 0
                                        && point
                                            .col
                                            .checked_add_signed(movement)
                                            .is_none_or(|col| !cols.contains(&col))
                                    {
                                        expected = false;
                                    }
                                    if uniform.is_some_and(|prior| prior != movement) {
                                        expected = false;
                                    }
                                    uniform = Some(movement);
                                }
                                let mut actual = original.clone();
                                assert_eq!(
                                    actual.shift_columns_in_row(row, cols.clone(), delta, 4),
                                    expected,
                                    "{ty:?} {start:?} {end:?} row={row} cols={cols:?} delta={delta}"
                                );
                                if expected {
                                    let shift = uniform.unwrap_or(0);
                                    assert_eq!(
                                        actual.start,
                                        Point::new(
                                            start.row,
                                            start.col.checked_add_signed(shift).unwrap()
                                        )
                                    );
                                    assert_eq!(
                                        actual.end,
                                        Point::new(
                                            end.row,
                                            end.col.checked_add_signed(shift).unwrap()
                                        )
                                    );
                                    assert_eq!(actual.ty, ty);
                                    assert!(!actual.active);
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn region_selection_mapping_matches_a_cellwise_uniform_motion_oracle() {
        let kinds = [
            SelectionType::Simple,
            SelectionType::Semantic,
            SelectionType::Lines,
            SelectionType::Block,
        ];
        let rectangles = [
            (0..4, 0..4),
            (1..3, 0..4),
            (0..4, 1..3),
            (1..4, 1..4),
            (2..3, 2..3),
            (0..0, 0..4),
        ];
        let points: Vec<_> = (-1..4)
            .flat_map(|row| (0..4).map(move |col| Point::new(row, col)))
            .collect();
        for kind in kinds {
            for &start in &points {
                for &end in &points {
                    let mut original = Selection::new(start, kind);
                    original.update(end);
                    let cells: Vec<_> = points
                        .iter()
                        .copied()
                        .filter(|point| original.contains(point.row, point.col))
                        .collect();
                    for (rows, cols) in &rectangles {
                        let intersects = cells.iter().any(|cell| {
                            rows.contains(&(cell.row as usize)) && cols.contains(&cell.col)
                        });
                        assert_eq!(
                            original.intersects_region(rows.clone(), cols.clone(), 4),
                            intersects
                        );
                        for delta in [-2, -1, 0, 1, 2] {
                            let mut uniform = None;
                            let mut expected_keep = true;
                            for cell in &cells {
                                let inside =
                                    rows.contains(&(cell.row as usize)) && cols.contains(&cell.col);
                                let movement = if inside { delta } else { 0 };
                                if inside && !rows.contains(&((cell.row + movement) as usize)) {
                                    expected_keep = false;
                                }
                                if uniform.is_some_and(|previous| previous != movement) {
                                    expected_keep = false;
                                }
                                uniform = Some(movement);
                            }
                            let mut actual = original.clone();
                            let keep =
                                actual.scroll_in_region(rows.clone(), cols.clone(), delta, 4);
                            assert_eq!(
                                keep, expected_keep,
                                "{kind:?} {start:?} {end:?} rows={rows:?} cols={cols:?} delta={delta}"
                            );
                            if keep {
                                let shift = uniform.unwrap_or(0);
                                assert_eq!(actual.start, Point::new(start.row + shift, start.col));
                                assert_eq!(actual.end, Point::new(end.row + shift, end.col));
                                assert_eq!(actual.ty, kind);
                                assert_eq!(actual.active, original.active);
                            }
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn test_point_ordering() {
        let p1 = Point::new(0, 5);
        let p2 = Point::new(0, 10);
        let p3 = Point::new(1, 0);

        assert!(p1 < p2);
        assert!(p2 < p3);
        assert!(p1 < p3);
    }

    #[test]
    fn test_simple_selection() {
        let mut sel = Selection::new(Point::new(0, 5), SelectionType::Simple);
        sel.update(Point::new(0, 10));

        assert!(sel.contains(0, 5));
        assert!(sel.contains(0, 7));
        assert!(sel.contains(0, 10));
        assert!(!sel.contains(0, 4));
        assert!(!sel.contains(0, 11));
    }

    #[test]
    fn test_multiline_selection() {
        let mut sel = Selection::new(Point::new(0, 5), SelectionType::Simple);
        sel.update(Point::new(2, 10));

        // First row: from col 5 to end
        assert!(sel.contains(0, 5));
        assert!(sel.contains(0, 100));
        assert!(!sel.contains(0, 4));

        // Middle row: all columns
        assert!(sel.contains(1, 0));
        assert!(sel.contains(1, 100));

        // Last row: from start to col 10
        assert!(sel.contains(2, 0));
        assert!(sel.contains(2, 10));
        assert!(!sel.contains(2, 11));
    }

    #[test]
    fn test_block_selection() {
        let mut sel = Selection::new(Point::new(0, 5), SelectionType::Block);
        sel.update(Point::new(2, 10));

        assert!(sel.contains(1, 7));
        assert!(!sel.contains(1, 4));
        assert!(!sel.contains(1, 11));
    }

    #[test]
    fn test_bounds() {
        let mut sel = Selection::new(Point::new(2, 10), SelectionType::Simple);
        sel.update(Point::new(0, 5));

        let (start, end) = sel.bounds();
        assert_eq!(start, Point::new(0, 5));
        assert_eq!(end, Point::new(2, 10));
    }
}
