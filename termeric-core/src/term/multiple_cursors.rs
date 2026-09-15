//! Kitty multiple-cursors protocol state and query handling.

use super::Terminal;
use crate::event::TerminalEvent;
use crate::grid::Color;
use std::fmt::Write as _;

/// Maximum geometry accepted by live-domain pane resizes and by the multiple
/// cursors protocol. Keeping the same limit makes every allocation and query
/// response finite even when a child fills the complete viewport.
pub const MAX_EXTRA_CURSOR_DIMENSION: usize = 1_000;
pub const MAX_EXTRA_CURSOR_CELLS: usize = MAX_EXTRA_CURSOR_DIMENSION * MAX_EXTRA_CURSOR_DIMENSION;

const EMPTY: u8 = 0;
const BLOCK: u8 = 1;
const BEAM: u8 = 2;
const UNDERLINE: u8 = 3;
const FOLLOW_MAIN: u8 = 4;

/// Shape of one application-authored cursor in the visible viewport.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExtraCursorShape {
    Block,
    Beam,
    Underline,
    FollowMain,
}

impl ExtraCursorShape {
    fn from_parameter(value: u16) -> Option<Option<Self>> {
        match value {
            0 => Some(None),
            1 => Some(Some(Self::Block)),
            2 => Some(Some(Self::Beam)),
            3 => Some(Some(Self::Underline)),
            29 => Some(Some(Self::FollowMain)),
            _ => None,
        }
    }

    pub fn parameter(self) -> u8 {
        match self {
            Self::Block => 1,
            Self::Beam => 2,
            Self::Underline => 3,
            Self::FollowMain => 29,
        }
    }

    fn storage(self) -> u8 {
        match self {
            Self::Block => BLOCK,
            Self::Beam => BEAM,
            Self::Underline => UNDERLINE,
            Self::FollowMain => FOLLOW_MAIN,
        }
    }

    fn from_storage(value: u8) -> Option<Self> {
        match value {
            BLOCK => Some(Self::Block),
            BEAM => Some(Self::Beam),
            UNDERLINE => Some(Self::Underline),
            FOLLOW_MAIN => Some(Self::FollowMain),
            _ => None,
        }
    }
}

/// One color component from the Kitty multiple-cursors color pair.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ExtraCursorColor {
    /// Follow the corresponding main-cursor color.
    #[default]
    Unset,
    /// Resolve by reversing the cell colors as specified by the protocol.
    Special,
    Rgb(Color),
    Indexed(u8),
}

impl ExtraCursorColor {
    fn parse(parameters: &[u16]) -> Option<Self> {
        match parameters {
            [0] => Some(Self::Unset),
            [1] => Some(Self::Special),
            [2, red, green, blue] if [red, green, blue].iter().all(|value| **value <= 255) => {
                Some(Self::Rgb(Color::rgb(*red as u8, *green as u8, *blue as u8)))
            }
            [5, index] if *index <= 255 => Some(Self::Indexed(*index as u8)),
            _ => None,
        }
    }

    fn push_parameters(self, output: &mut String) {
        match self {
            Self::Unset => output.push('0'),
            Self::Special => output.push('1'),
            Self::Rgb(color) => {
                let _ = write!(output, "2:{}:{}:{}", color.r, color.g, color.b);
            }
            Self::Indexed(index) => {
                let _ = write!(output, "5:{index}");
            }
        }
    }
}

/// Shared color pair for every extra cursor in one terminal.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ExtraCursorColors {
    pub text: ExtraCursorColor,
    pub cursor: ExtraCursorColor,
}

/// One active extra cursor returned by deterministic viewport iteration.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ExtraCursor {
    pub row: usize,
    pub col: usize,
    pub shape: ExtraCursorShape,
}

/// Compact, lazy viewport map for Kitty multiple cursors.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExtraCursorState {
    cols: usize,
    rows: usize,
    cells: Option<Vec<u8>>,
    active: usize,
    colors: ExtraCursorColors,
}

impl ExtraCursorState {
    pub(super) fn new(cols: usize, rows: usize) -> Self {
        Self {
            cols,
            rows,
            cells: None,
            active: 0,
            colors: ExtraCursorColors::default(),
        }
    }

    fn supported_geometry(cols: usize, rows: usize) -> bool {
        cols <= MAX_EXTRA_CURSOR_DIMENSION
            && rows <= MAX_EXTRA_CURSOR_DIMENSION
            && cols
                .checked_mul(rows)
                .is_some_and(|cells| cells <= MAX_EXTRA_CURSOR_CELLS)
    }

    pub fn len(&self) -> usize {
        self.active
    }

    pub fn is_empty(&self) -> bool {
        self.active == 0
    }

    pub fn owned_bytes(&self) -> usize {
        self.cells.as_ref().map_or(0, Vec::capacity)
    }

    pub fn colors(&self) -> ExtraCursorColors {
        self.colors
    }

    pub fn cursors(&self) -> impl Iterator<Item = ExtraCursor> + '_ {
        self.cells
            .as_deref()
            .into_iter()
            .flat_map(|cells| cells.iter().copied().enumerate())
            .filter_map(|(index, value)| {
                ExtraCursorShape::from_storage(value).map(|shape| ExtraCursor {
                    row: index / self.cols,
                    col: index % self.cols,
                    shape,
                })
            })
    }

    fn clear(&mut self) {
        self.cells = None;
        self.active = 0;
    }

    fn resize(&mut self, cols: usize, rows: usize) {
        let old_cols = self.cols;
        let old_rows = self.rows;
        self.cols = cols;
        self.rows = rows;
        let Some(old_cells) = self.cells.take() else {
            return;
        };
        if !Self::supported_geometry(cols, rows) || cols == 0 || rows == 0 {
            self.active = 0;
            return;
        }

        let mut new_cells = vec![EMPTY; cols * rows];
        let copy_rows = old_rows.min(rows);
        let copy_cols = old_cols.min(cols);
        let mut active = 0;
        for row in 0..copy_rows {
            for col in 0..copy_cols {
                let value = old_cells[row * old_cols + col];
                new_cells[row * cols + col] = value;
                active += usize::from(value != EMPTY);
            }
        }
        self.active = active;
        self.cells = (active != 0).then_some(new_cells);
    }

    fn set_cell(&mut self, row: usize, col: usize, shape: Option<ExtraCursorShape>) {
        if row >= self.rows || col >= self.cols || !Self::supported_geometry(self.cols, self.rows) {
            return;
        }
        let index = row * self.cols + col;
        if shape.is_none() && self.cells.is_none() {
            return;
        }
        let cells = self
            .cells
            .get_or_insert_with(|| vec![EMPTY; self.cols * self.rows]);
        let previous = cells[index];
        let next = shape.map_or(EMPTY, ExtraCursorShape::storage);
        cells[index] = next;
        match (previous == EMPTY, next == EMPTY) {
            (true, false) => self.active += 1,
            (false, true) => self.active -= 1,
            _ => {}
        }
        if self.active == 0 {
            self.cells = None;
        }
    }

    fn set_rectangle(
        &mut self,
        top: usize,
        left: usize,
        bottom: usize,
        right: usize,
        shape: Option<ExtraCursorShape>,
    ) {
        if top > bottom || left > right || self.rows == 0 || self.cols == 0 {
            return;
        }
        let top = top.saturating_sub(1).min(self.rows);
        let left = left.saturating_sub(1).min(self.cols);
        let bottom = bottom.min(self.rows);
        let right = right.min(self.cols);
        if top >= bottom || left >= right {
            return;
        }
        for row in top..bottom {
            for col in left..right {
                self.set_cell(row, col, shape);
            }
        }
    }

    fn cursor_report(&self) -> Vec<u8> {
        let mut response = String::from("\x1b[>100");
        let Some(cells) = self.cells.as_deref() else {
            response.push_str(" q");
            return response.into_bytes();
        };
        for row in 0..self.rows {
            let mut col = 0;
            while col < self.cols {
                let value = cells[row * self.cols + col];
                if value == EMPTY {
                    col += 1;
                    continue;
                }
                let start = col;
                col += 1;
                while col < self.cols && cells[row * self.cols + col] == value {
                    col += 1;
                }
                let shape = ExtraCursorShape::from_storage(value)
                    .expect("only validated cursor shapes are stored");
                let _ = write!(
                    response,
                    ";{}:4:{}:{}:{}:{}",
                    shape.parameter(),
                    row + 1,
                    start + 1,
                    row + 1,
                    col,
                );
            }
        }
        response.push_str(" q");
        response.into_bytes()
    }

    fn color_report(&self) -> Vec<u8> {
        let mut response = String::from("\x1b[>101;30:");
        self.colors.text.push_parameters(&mut response);
        response.push_str(";40:");
        self.colors.cursor.push_parameters(&mut response);
        response.push_str(" q");
        response.into_bytes()
    }

    /// Standard protocol sequences that recreate the state without producing
    /// query replies. Returns no partial cursor map when the caller's budget
    /// cannot hold the complete state.
    pub fn replay(&self, max_bytes: usize) -> Vec<u8> {
        if self.is_empty() && self.colors == ExtraCursorColors::default() {
            return Vec::new();
        }
        let mut output = String::new();
        for (which, color) in [(30, self.colors.text), (40, self.colors.cursor)] {
            let _ = write!(output, "\x1b[>{which};");
            color.push_parameters(&mut output);
            output.push_str(" q");
        }
        let colors_len = output.len();
        if let Some(cells) = self.cells.as_deref() {
            for row in 0..self.rows {
                let mut col = 0;
                while col < self.cols {
                    let value = cells[row * self.cols + col];
                    if value == EMPTY {
                        col += 1;
                        continue;
                    }
                    let start = col;
                    col += 1;
                    while col < self.cols && cells[row * self.cols + col] == value {
                        col += 1;
                    }
                    let shape = ExtraCursorShape::from_storage(value)
                        .expect("only validated cursor shapes are stored");
                    let _ = write!(
                        output,
                        "\x1b[>{};4:{}:{}:{}:{} q",
                        shape.parameter(),
                        row + 1,
                        start + 1,
                        row + 1,
                        col,
                    );
                    if output.len() > max_bytes {
                        output.truncate(colors_len);
                        break;
                    }
                }
                if output.len() == colors_len && self.active != 0 {
                    break;
                }
            }
        }
        if output.len() <= max_bytes {
            output.into_bytes()
        } else {
            Vec::new()
        }
    }
}

impl Terminal {
    /// Current live extra-cursor state.
    pub fn extra_cursors(&self) -> &ExtraCursorState {
        &self.extra_cursors
    }

    /// Extra-cursor state corresponding to [`Terminal::presentation_grid`].
    pub fn presentation_extra_cursors(&self) -> &ExtraCursorState {
        self.synchronized_output_snapshot
            .as_ref()
            .map_or(&self.extra_cursors, |snapshot| &snapshot.extra_cursors)
    }

    pub(super) fn clear_extra_cursors(&mut self) {
        self.extra_cursors.clear();
    }

    pub(super) fn resize_extra_cursors(&mut self, cols: usize, rows: usize) {
        self.extra_cursors.resize(cols, rows);
        if let Some(snapshot) = &mut self.synchronized_output_snapshot {
            snapshot.extra_cursors.resize(cols, rows);
        }
    }

    pub(crate) fn handle_multiple_cursor_csi(&mut self, parameters: &[Vec<u16>]) {
        // vte canonicalizes an omitted CSI parameter to one scalar zero. An
        // explicit shape-zero command without coordinates is itself a no-op,
        // so treating that indistinguishable form as the support query is safe.
        if parameters.is_empty() || (parameters.len() == 1 && parameters[0].as_slice() == [0]) {
            self.push_event(TerminalEvent::PtyWrite(
                b"\x1b[>1;2;3;29;30;40;100;101 q".to_vec(),
            ));
            return;
        }
        let Some([operation]) = parameters.first().map(Vec::as_slice) else {
            return;
        };
        match *operation {
            100 if parameters.len() == 1 => {
                self.push_event(TerminalEvent::PtyWrite(self.extra_cursors.cursor_report()));
            }
            101 if parameters.len() == 1 => {
                self.push_event(TerminalEvent::PtyWrite(self.extra_cursors.color_report()));
            }
            30 | 40 if parameters.len() == 2 => {
                let Some(color) = ExtraCursorColor::parse(&parameters[1]) else {
                    return;
                };
                if *operation == 30 {
                    self.extra_cursors.colors.text = color;
                } else {
                    self.extra_cursors.colors.cursor = color;
                }
            }
            shape_parameter => {
                let Some(shape) = ExtraCursorShape::from_parameter(shape_parameter) else {
                    return;
                };
                for group in &parameters[1..] {
                    let Some((&coordinate_type, coordinates)) = group.split_first() else {
                        continue;
                    };
                    match coordinate_type {
                        0 if coordinates.is_empty() => {
                            let cursor = self.grid().cursor();
                            self.extra_cursors.set_cell(cursor.row, cursor.col, shape);
                        }
                        2 => {
                            for point in coordinates.as_chunks::<2>().0 {
                                let row = usize::from(point[0]);
                                let col = usize::from(point[1]);
                                if row != 0 && col != 0 {
                                    self.extra_cursors.set_cell(row - 1, col - 1, shape);
                                }
                            }
                        }
                        4 if coordinates.is_empty() => self
                            .extra_cursors
                            .set_rectangle(1, 1, self.rows, self.cols, shape),
                        4 => {
                            for rectangle in coordinates.as_chunks::<4>().0 {
                                self.extra_cursors.set_rectangle(
                                    usize::from(rectangle[0]),
                                    usize::from(rectangle[1]),
                                    usize::from(rectangle[2]),
                                    usize::from(rectangle[3]),
                                    shape,
                                );
                            }
                        }
                        _ => {}
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cursors(terminal: &Terminal) -> Vec<ExtraCursor> {
        terminal.extra_cursors().cursors().collect()
    }

    #[test]
    fn compact_state_is_lazy_bounded_and_released_when_empty() {
        let mut state = ExtraCursorState::new(80, 24);
        assert_eq!(state.owned_bytes(), 0);
        state.set_cell(1, 2, Some(ExtraCursorShape::Block));
        assert_eq!(state.len(), 1);
        assert_eq!(state.owned_bytes(), 80 * 24);
        state.set_cell(1, 2, None);
        assert!(state.is_empty());
        assert_eq!(state.owned_bytes(), 0);

        let mut oversized = ExtraCursorState::new(MAX_EXTRA_CURSOR_DIMENSION + 1, 1);
        oversized.set_cell(0, 0, Some(ExtraCursorShape::Beam));
        assert!(oversized.is_empty());
        assert_eq!(oversized.owned_bytes(), 0);
    }

    #[test]
    fn resize_preserves_only_the_intersection() {
        let mut state = ExtraCursorState::new(4, 3);
        state.set_cell(0, 0, Some(ExtraCursorShape::Block));
        state.set_cell(2, 3, Some(ExtraCursorShape::Beam));
        state.resize(2, 2);
        assert_eq!(
            state.cursors().collect::<Vec<_>>(),
            vec![ExtraCursor {
                row: 0,
                col: 0,
                shape: ExtraCursorShape::Block,
            }]
        );
    }

    #[test]
    fn query_groups_horizontal_runs_deterministically() {
        let mut state = ExtraCursorState::new(5, 2);
        state.set_cell(0, 1, Some(ExtraCursorShape::Beam));
        state.set_cell(0, 2, Some(ExtraCursorShape::Beam));
        state.set_cell(1, 4, Some(ExtraCursorShape::FollowMain));
        assert_eq!(
            state.cursor_report(),
            b"\x1b[>100;2:4:1:2:1:3;29:4:2:5:2:5 q"
        );
    }

    #[test]
    fn parser_support_query_is_byte_exact_fifo_and_c1_aware() {
        let mut terminal = Terminal::new(8, 4);
        terminal.process(b"\x1b[> q\x1b[c\x1b G\x1b[>100 q\x1b[>101 q");
        assert_eq!(
            terminal.drain_events(),
            vec![
                TerminalEvent::PtyWrite(b"\x1b[>1;2;3;29;30;40;100;101 q".to_vec()),
                TerminalEvent::PtyWrite(b"\x1b[?1;2;4;8c".to_vec()),
                TerminalEvent::PtyWrite(b"\x9b>100 q".to_vec()),
                TerminalEvent::PtyWrite(b"\x9b>101;30:0;40:0 q".to_vec()),
            ]
        );
    }

    #[test]
    fn parser_preserves_colon_groups_across_arbitrary_input_fragments() {
        let mut terminal = Terminal::new(8, 4);
        for fragment in [b"\x1b[>2;2:".as_slice(), b"1:2:1", b":3;4:2:4:", b"3:5 q"] {
            terminal.process(fragment);
        }
        assert_eq!(
            cursors(&terminal),
            vec![
                ExtraCursor {
                    row: 0,
                    col: 1,
                    shape: ExtraCursorShape::Beam,
                },
                ExtraCursor {
                    row: 0,
                    col: 2,
                    shape: ExtraCursorShape::Beam,
                },
                ExtraCursor {
                    row: 1,
                    col: 3,
                    shape: ExtraCursorShape::Beam,
                },
                ExtraCursor {
                    row: 1,
                    col: 4,
                    shape: ExtraCursorShape::Beam,
                },
                ExtraCursor {
                    row: 2,
                    col: 3,
                    shape: ExtraCursorShape::Beam,
                },
                ExtraCursor {
                    row: 2,
                    col: 4,
                    shape: ExtraCursorShape::Beam,
                },
            ]
        );
    }

    #[test]
    fn points_rectangles_main_cursor_clipping_and_incomplete_tails_are_exact() {
        let mut terminal = Terminal::new(6, 4);
        terminal.process(
            b"\x1b[2;3H\x1b[>1;0 q\x1b[>2;2:1:1:2:3:4:6:4:99:3 q\x1b[>3;4:2:2:3:4:0:0:2:2:4 q",
        );
        assert_eq!(terminal.extra_cursors().len(), 10);
        assert!(cursors(&terminal).contains(&ExtraCursor {
            row: 3,
            col: 5,
            shape: ExtraCursorShape::Beam,
        }));
        // The later rectangle overwrites its complete clipped intersection.
        assert!(cursors(&terminal).contains(&ExtraCursor {
            row: 1,
            col: 1,
            shape: ExtraCursorShape::Underline,
        }));
        assert!(cursors(&terminal).contains(&ExtraCursor {
            row: 2,
            col: 3,
            shape: ExtraCursorShape::Underline,
        }));
        // Zero/out-of-range points and the odd trailing coordinate are ignored.
        assert!(
            !cursors(&terminal)
                .iter()
                .any(|cursor| cursor.row == 3 && cursor.col == 2)
        );

        terminal.process(b"\x1b[>29;4 q");
        assert_eq!(terminal.extra_cursors().len(), 24);
        assert!(
            cursors(&terminal)
                .iter()
                .all(|cursor| cursor.shape == ExtraCursorShape::FollowMain)
        );
        terminal.process(b"\x1b[>0;2:1:1;4:4:6:4:6 q");
        assert_eq!(terminal.extra_cursors().len(), 22);
    }

    #[test]
    fn colors_parse_query_and_reject_malformed_updates_atomically() {
        let mut terminal = Terminal::new(4, 2);
        terminal.process(b"\x1b[>30;2:1:2:3 q\x1b[>40;5:196 q\x1b[>101 q");
        assert_eq!(
            terminal.extra_cursors().colors(),
            ExtraCursorColors {
                text: ExtraCursorColor::Rgb(Color::rgb(1, 2, 3)),
                cursor: ExtraCursorColor::Indexed(196),
            }
        );
        assert_eq!(
            terminal.drain_events(),
            vec![TerminalEvent::PtyWrite(
                b"\x1b[>101;30:2:1:2:3;40:5:196 q".to_vec()
            )]
        );

        terminal.process(b"\x1b[>30;2:1:2:999 q\x1b[>40;5:256 q\x1b[>30;1:2 q");
        assert_eq!(
            terminal.extra_cursors().colors(),
            ExtraCursorColors {
                text: ExtraCursorColor::Rgb(Color::rgb(1, 2, 3)),
                cursor: ExtraCursorColor::Indexed(196),
            }
        );
        terminal.process(b"\x1b[>30;1 q\x1b[>40;0 q");
        assert_eq!(
            terminal.extra_cursors().colors(),
            ExtraCursorColors {
                text: ExtraCursorColor::Special,
                cursor: ExtraCursorColor::Unset,
            }
        );
    }

    #[test]
    fn destructive_lifecycle_clears_while_scroll_keeps_screen_coordinates() {
        for erase in [b"\x1b[2J".as_slice(), b"\x1b[3J", b"\x1b[22J"] {
            let mut terminal = Terminal::new(4, 3);
            terminal.process(b"\x1b[>1;2:2:2 q");
            terminal.process(erase);
            assert!(terminal.extra_cursors().is_empty());
        }

        let mut terminal = Terminal::new(4, 3);
        terminal.process(b"\x1b[>2;2:2:2 q\x1bD\x1bM");
        assert_eq!(
            cursors(&terminal),
            vec![ExtraCursor {
                row: 1,
                col: 1,
                shape: ExtraCursorShape::Beam,
            }]
        );
        terminal.process(b"\x1b[?1049h");
        assert!(terminal.extra_cursors().is_empty());
        terminal.process(b"\x1b[>3;2:1:1 q\x1b[?1049l");
        assert!(terminal.extra_cursors().is_empty());
        terminal.process(b"\x1b[>1;2:1:1 q\x1b[!p");
        assert!(terminal.extra_cursors().is_empty());
        terminal.process(b"\x1b[>1;2:1:1 q\x1bc");
        assert!(terminal.extra_cursors().is_empty());
    }

    #[test]
    fn synchronized_output_freezes_extra_cursors_and_colors() {
        let mut terminal = Terminal::new(4, 2);
        terminal.process(b"\x1b[>1;2:1:1 q\x1b[>30;2:1:2:3 q\x1b[?2026h");
        terminal.process(b"\x1b[>0;4 q\x1b[>30;1 q");
        assert_eq!(terminal.extra_cursors().len(), 0);
        assert_eq!(terminal.presentation_extra_cursors().len(), 1);
        assert_eq!(
            terminal.presentation_extra_cursors().colors().text,
            ExtraCursorColor::Rgb(Color::rgb(1, 2, 3))
        );
        terminal.process(b"\x1b[?2026l");
        assert!(terminal.presentation_extra_cursors().is_empty());
        assert_eq!(
            terminal.presentation_extra_cursors().colors().text,
            ExtraCursorColor::Special
        );
    }

    #[test]
    fn resize_and_memory_accounting_are_bounded() {
        let mut terminal = Terminal::new(8, 4);
        terminal.process(b"\x1b[>1;4 q");
        assert_eq!(terminal.extra_cursors().len(), 32);
        assert_eq!(terminal.memory_usage().extra_cursor_bytes, 32);
        terminal.resize(4, 2);
        assert_eq!(terminal.extra_cursors().len(), 8);
        assert_eq!(terminal.memory_usage().extra_cursor_bytes, 8);
        terminal.resize(MAX_EXTRA_CURSOR_DIMENSION + 1, 1);
        assert!(terminal.extra_cursors().is_empty());
        assert_eq!(terminal.memory_usage().extra_cursor_bytes, 0);
    }

    #[test]
    fn replay_is_compact_round_trippable_and_never_partial() {
        let mut source = Terminal::new(80, 24);
        source.process(b"\x1b[>29;4 q\x1b[>30;2:1:2:3 q\x1b[>40;5:196 q");
        let replay = source.extra_cursors().replay(1024);
        assert!(replay.len() < 1024);

        let mut destination = Terminal::new(80, 24);
        destination.process(&replay);
        assert_eq!(destination.extra_cursors(), source.extra_cursors());

        let colors_only = source.extra_cursors().replay(64);
        let mut bounded_destination = Terminal::new(80, 24);
        bounded_destination.process(&colors_only);
        assert!(bounded_destination.extra_cursors().is_empty());
        assert_eq!(
            bounded_destination.extra_cursors().colors(),
            source.extra_cursors().colors()
        );
        assert!(colors_only.len() <= 64);
    }
}
