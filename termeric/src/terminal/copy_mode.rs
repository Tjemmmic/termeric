//! Keyboard-only scrollback navigation, selection, and semantic hints.

use regex::Regex;
use termeric_core::{CellContent, CellFlags, Grid, Point, Selection, SelectionType, Terminal};

const HINT_ALPHABET: &[u8] = b"asdfghjklqwertyuiopzxcvbnm";
const MAX_QUICK_SELECT_MATCHES: usize = 4_096;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CopyMotion {
    Left,
    Right,
    Up,
    Down,
    LineStart,
    LineEnd,
    WordForward,
    WordBackward,
    PageUp,
    PageDown,
    Top,
    Bottom,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HintAction {
    Open,
    Copy,
}

impl HintAction {
    pub fn label(self) -> &'static str {
        match self {
            Self::Open => "open",
            Self::Copy => "copy",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SemanticMatch {
    pub label: String,
    pub value: String,
    pub start: Point,
    pub end: Point,
    pub source: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QuickSelect {
    pub matches: Vec<SemanticMatch>,
    pub prefix: String,
    pub action: HintAction,
}

#[derive(Debug, Clone)]
pub struct CopyMode {
    active: bool,
    cursor: Point,
    anchor: Option<Point>,
    selection_type: SelectionType,
    preferred_col: usize,
    pending_g: bool,
    quick_select: Option<QuickSelect>,
    reflow_bookmark: Option<(LogicalPoint, Option<LogicalPoint>)>,
}

#[derive(Debug, Clone, Copy)]
struct LogicalPoint {
    line: usize,
    cell: usize,
}

impl Default for CopyMode {
    fn default() -> Self {
        Self {
            active: false,
            cursor: Point::new(0, 0),
            anchor: None,
            selection_type: SelectionType::Simple,
            preferred_col: 0,
            pending_g: false,
            quick_select: None,
            reflow_bookmark: None,
        }
    }
}

impl CopyMode {
    pub fn is_active(&self) -> bool {
        self.active
    }

    pub fn cursor(&self) -> Point {
        self.cursor
    }

    pub fn quick_select(&self) -> Option<&QuickSelect> {
        self.quick_select.as_ref()
    }

    pub fn enter(&mut self, grid: &Grid) {
        self.active = true;
        self.anchor = None;
        self.selection_type = SelectionType::Simple;
        self.quick_select = None;
        self.pending_g = false;
        let offset = i32::try_from(grid.scroll_offset()).unwrap_or(i32::MAX);
        let cursor = grid.cursor();
        let viewport_bottom = (grid.rows().saturating_sub(1) as i32).saturating_sub(offset);
        self.cursor = if offset == 0 {
            Point::new(cursor.row as i32, cursor.col)
        } else {
            Point::new(viewport_bottom, 0)
        };
        self.clamp(grid);
    }

    pub fn exit(&mut self) {
        *self = Self::default();
    }

    pub fn set_pending_g(&mut self, pending: bool) {
        self.pending_g = pending;
    }

    pub fn take_pending_g(&mut self) -> bool {
        std::mem::take(&mut self.pending_g)
    }

    pub fn toggle_selection(&mut self, ty: SelectionType) {
        if self.anchor.is_some() && self.selection_type == ty {
            self.anchor = None;
        } else {
            self.anchor = Some(self.cursor);
            self.selection_type = ty;
        }
    }

    pub fn selection(&self) -> Option<Selection> {
        let anchor = self.anchor?;
        let mut selection = Selection::new(anchor, self.selection_type);
        selection.update(self.cursor);
        Some(selection)
    }

    pub fn move_cursor(&mut self, motion: CopyMotion, grid: &mut Grid) {
        if !self.active {
            return;
        }
        let oldest = -(grid.scrollback_len() as i32);
        let newest = grid.rows().saturating_sub(1) as i32;
        match motion {
            CopyMotion::Left => self.cursor.col = self.cursor.col.saturating_sub(1),
            CopyMotion::Right => {
                self.cursor.col = (self.cursor.col + 1).min(grid.cols().saturating_sub(1));
                if is_wide_spacer(grid, self.cursor) {
                    self.cursor.col = (self.cursor.col + 1).min(grid.cols().saturating_sub(1));
                }
            }
            CopyMotion::Up => self.cursor.row = self.cursor.row.saturating_sub(1).max(oldest),
            CopyMotion::Down => self.cursor.row = self.cursor.row.saturating_add(1).min(newest),
            CopyMotion::LineStart => self.cursor.col = 0,
            CopyMotion::LineEnd => self.cursor.col = row_last_content_col(grid, self.cursor.row),
            CopyMotion::PageUp => {
                self.cursor.row = self
                    .cursor
                    .row
                    .saturating_sub(grid.rows().saturating_sub(1) as i32)
                    .max(oldest);
            }
            CopyMotion::PageDown => {
                self.cursor.row = self
                    .cursor
                    .row
                    .saturating_add(grid.rows().saturating_sub(1) as i32)
                    .min(newest);
            }
            CopyMotion::Top => self.cursor = Point::new(oldest, 0),
            CopyMotion::Bottom => self.cursor = Point::new(newest, 0),
            CopyMotion::WordForward => self.cursor = word_forward(grid, self.cursor),
            CopyMotion::WordBackward => self.cursor = word_backward(grid, self.cursor),
        }
        if matches!(
            motion,
            CopyMotion::Up | CopyMotion::Down | CopyMotion::PageUp | CopyMotion::PageDown
        ) {
            self.cursor.col = self.preferred_col.min(grid.cols().saturating_sub(1));
        } else {
            self.preferred_col = self.cursor.col;
        }
        self.clamp(grid);
        reveal_cursor(grid, self.cursor);
    }

    pub fn jump_to(&mut self, point: Point, grid: &mut Grid) {
        self.cursor = point;
        self.preferred_col = point.col;
        self.clamp(grid);
        reveal_cursor(grid, self.cursor);
    }

    pub fn start_quick_select(
        &mut self,
        terminal: &Terminal,
        patterns: &[(String, Regex)],
        action: HintAction,
    ) {
        if !self.active {
            self.enter(terminal.grid());
        }
        self.quick_select = Some(QuickSelect {
            matches: semantic_matches(terminal, patterns),
            prefix: String::new(),
            action,
        });
    }

    pub fn toggle_hint_action(&mut self) {
        if let Some(quick) = &mut self.quick_select {
            quick.action = match quick.action {
                HintAction::Open => HintAction::Copy,
                HintAction::Copy => HintAction::Open,
            };
        }
    }

    pub fn cancel_quick_select(&mut self) {
        self.quick_select = None;
    }

    /// Rebuild visible semantic matches after the terminal grid reflows while
    /// retaining the user's selected action and any still-valid label prefix.
    pub fn refresh_quick_select(&mut self, terminal: &Terminal, patterns: &[(String, Regex)]) {
        let Some(quick) = self.quick_select.as_mut() else {
            return;
        };
        let matches = semantic_matches(terminal, patterns);
        if !matches
            .iter()
            .any(|candidate| candidate.label.starts_with(&quick.prefix))
        {
            quick.prefix.clear();
        }
        quick.matches = matches;
    }

    /// Add one label character. Returns the uniquely completed match and action.
    pub fn push_hint_key(&mut self, key: char) -> Option<(SemanticMatch, HintAction)> {
        let quick = self.quick_select.as_mut()?;
        let key = key.to_ascii_lowercase();
        if !HINT_ALPHABET.contains(&(key as u8)) {
            return None;
        }
        quick.prefix.push(key);
        let candidates: Vec<_> = quick
            .matches
            .iter()
            .filter(|candidate| candidate.label.starts_with(&quick.prefix))
            .collect();
        if candidates.len() == 1 && candidates[0].label == quick.prefix {
            return Some((candidates[0].clone(), quick.action));
        }
        if candidates.is_empty() {
            quick.prefix.clear();
        }
        None
    }

    pub fn backspace_hint(&mut self) {
        if let Some(quick) = &mut self.quick_select {
            quick.prefix.pop();
        }
    }

    pub fn clamp(&mut self, grid: &Grid) {
        let oldest = -(grid.scrollback_len() as i32);
        let newest = grid.rows().saturating_sub(1) as i32;
        self.cursor.row = self.cursor.row.clamp(oldest, newest);
        self.cursor.col = self.cursor.col.min(grid.cols().saturating_sub(1));
        if is_wide_spacer(grid, self.cursor) {
            self.cursor.col = self.cursor.col.saturating_sub(1);
        }
        if let Some(anchor) = &mut self.anchor {
            anchor.row = anchor.row.clamp(oldest, newest);
            anchor.col = anchor.col.min(grid.cols().saturating_sub(1));
            if is_wide_spacer(grid, *anchor) {
                anchor.col = anchor.col.saturating_sub(1);
            }
        }
    }

    pub fn prepare_reflow(&mut self, grid: &Grid) {
        if self.active {
            self.reflow_bookmark = Some((
                logical_point(grid, self.cursor),
                self.anchor.map(|anchor| logical_point(grid, anchor)),
            ));
        }
    }

    pub fn finish_reflow(&mut self, grid: &mut Grid) {
        let Some((cursor, anchor)) = self.reflow_bookmark.take() else {
            return;
        };
        self.cursor = point_from_logical(grid, cursor);
        self.anchor = anchor.map(|anchor| point_from_logical(grid, anchor));
        self.preferred_col = self.cursor.col;
        self.clamp(grid);
        reveal_cursor(grid, self.cursor);
    }
}

fn logical_point(grid: &Grid, point: Point) -> LogicalPoint {
    let oldest = -(grid.scrollback_len() as i32);
    let newest = grid.rows().saturating_sub(1) as i32;
    let target = point.row.clamp(oldest, newest);
    let mut line = 0;
    let mut cell = 0;
    for row_index in oldest..=newest {
        let Some(row) = grid.row_at_offset(row_index) else {
            continue;
        };
        if row_index == target {
            return LogicalPoint {
                line,
                cell: cell + point.col.min(row.len().saturating_sub(1)),
            };
        }
        if row.is_wrapped() {
            cell += row.len();
        } else {
            line += 1;
            cell = 0;
        }
    }
    LogicalPoint { line, cell }
}

fn point_from_logical(grid: &Grid, target: LogicalPoint) -> Point {
    let oldest = -(grid.scrollback_len() as i32);
    let newest = grid.rows().saturating_sub(1) as i32;
    let mut line = 0;
    let mut remaining = target.cell;
    for row_index in oldest..=newest {
        let Some(row) = grid.row_at_offset(row_index) else {
            continue;
        };
        if line == target.line {
            if remaining < row.len() || !row.is_wrapped() {
                return Point::new(row_index, remaining.min(row.len().saturating_sub(1)));
            }
            remaining = remaining.saturating_sub(row.len());
        }
        if !row.is_wrapped() {
            if line == target.line {
                return Point::new(row_index, row.len().saturating_sub(1));
            }
            line += 1;
            remaining = target.cell;
        }
    }
    Point::new(newest, grid.cols().saturating_sub(1))
}

fn reveal_cursor(grid: &mut Grid, cursor: Point) {
    let rows = grid.rows().saturating_sub(1) as i32;
    let offset = i32::try_from(grid.scroll_offset()).unwrap_or(i32::MAX);
    let top = -offset;
    let bottom = rows.saturating_sub(offset);
    let requested = if cursor.row < top {
        usize::try_from(-cursor.row).unwrap_or(usize::MAX)
    } else if cursor.row > bottom {
        usize::try_from(rows.saturating_sub(cursor.row)).unwrap_or(0)
    } else {
        grid.scroll_offset()
    };
    grid.set_scroll_offset(requested);
}

fn row_last_content_col(grid: &Grid, row: i32) -> usize {
    let Some(row) = grid.row_at_offset(row) else {
        return 0;
    };
    (0..row.len())
        .rev()
        .find(|&col| row.get(col).is_some_and(|cell| !cell.content.is_empty()))
        .unwrap_or(0)
}

fn is_word(cell: Option<termeric_core::grid::CellRef<'_>>) -> bool {
    cell.is_some_and(|cell| match &cell.content {
        CellContent::Char(character) => character.is_alphanumeric() || *character == '_',
        CellContent::Grapheme(grapheme) => grapheme
            .chars()
            .next()
            .is_some_and(|character| character.is_alphanumeric() || character == '_'),
        CellContent::Empty => false,
    })
}

fn is_wide_spacer(grid: &Grid, point: Point) -> bool {
    grid.row_at_offset(point.row)
        .and_then(|row| row.get(point.col))
        .is_some_and(|cell| cell.flags.contains(CellFlags::WIDE_CHAR_SPACER))
}

fn word_forward(grid: &Grid, point: Point) -> Point {
    let oldest = -(grid.scrollback_len() as i32);
    let newest = grid.rows().saturating_sub(1) as i32;
    let mut point = Point::new(
        point.row.clamp(oldest, newest),
        point.col.min(grid.cols().saturating_sub(1)),
    );
    let mut previous_was_word = grid
        .row_at_offset(point.row)
        .is_some_and(|line| is_word(line.get(point.col)));
    loop {
        if point.col + 1 < grid.cols() {
            point.col += 1;
        } else {
            if point.row >= newest {
                return Point::new(newest, grid.cols().saturating_sub(1));
            }
            point.row += 1;
            point.col = 0;
        }
        let word = grid
            .row_at_offset(point.row)
            .is_some_and(|line| is_word(line.get(point.col)));
        let hard_line_boundary = point.col == 0
            && grid
                .row_at_offset(point.row.saturating_sub(1))
                .is_none_or(|row| !row.is_wrapped());
        if word && (!previous_was_word || hard_line_boundary) {
            return point;
        }
        previous_was_word = word;
    }
}

fn word_backward(grid: &Grid, point: Point) -> Point {
    let oldest = -(grid.scrollback_len() as i32);
    let mut point = point;
    let mut found_word = false;
    loop {
        if point.col == 0 {
            if point.row <= oldest {
                return Point::new(oldest, 0);
            }
            point.row -= 1;
            point.col = grid.cols();
        }
        point.col -= 1;
        let Some(line) = grid.row_at_offset(point.row) else {
            continue;
        };
        if is_word(line.get(point.col)) {
            found_word = true;
            let continues_from_previous = point.col == 0
                && grid
                    .row_at_offset(point.row.saturating_sub(1))
                    .is_some_and(|row| row.is_wrapped());
            if point.col == 0 && !continues_from_previous {
                return point;
            }
        } else if found_word {
            if point.col + 1 < grid.cols() {
                point.col += 1;
                return point;
            }
            return Point::new(point.row.saturating_add(1), 0);
        }
    }
}

#[derive(Debug)]
struct TextCellSpan {
    byte_start: usize,
    byte_end: usize,
    start: Point,
    end: Point,
}

fn append_row_text(grid: &Grid, row_index: i32, text: &mut String, spans: &mut Vec<TextCellSpan>) {
    let Some(row) = grid.row_at_offset(row_index) else {
        return;
    };
    for col in 0..grid.cols() {
        let Some(cell) = row.get(col) else { break };
        if cell.flags.contains(CellFlags::WIDE_CHAR_SPACER) {
            continue;
        }
        let byte_start = text.len();
        match &cell.content {
            CellContent::Empty => text.push(' '),
            CellContent::Char(character) => text.push(*character),
            CellContent::Grapheme(grapheme) => text.push_str(grapheme),
        }
        let display_width = if cell.flags.contains(CellFlags::WIDE_CHAR) {
            2
        } else {
            1
        };
        spans.push(TextCellSpan {
            byte_start,
            byte_end: text.len(),
            start: Point::new(row_index, col),
            end: Point::new(
                row_index,
                col.saturating_add(display_width - 1)
                    .min(grid.cols().saturating_sub(1)),
            ),
        });
    }
}

fn append_pattern_matches(
    text: &str,
    spans: &[TextCellSpan],
    patterns: &[(String, Regex)],
    matches: &mut Vec<SemanticMatch>,
) {
    for (name, regex) in patterns {
        for found in regex.find_iter(text) {
            if matches.len() >= MAX_QUICK_SELECT_MATCHES {
                return;
            }
            if found.is_empty() {
                continue;
            }
            let Some(start) = spans
                .iter()
                .find(|span| span.byte_start <= found.start() && found.start() < span.byte_end)
                .map(|span| span.start)
            else {
                continue;
            };
            let Some(end) = spans
                .iter()
                .find(|span| span.byte_start < found.end() && found.end() <= span.byte_end)
                .map(|span| span.end)
            else {
                continue;
            };
            if matches.iter().any(|existing| {
                existing.source == "OSC 8 link" && start <= existing.end && end >= existing.start
            }) {
                continue;
            }
            matches.push(SemanticMatch {
                label: String::new(),
                value: found.as_str().to_string(),
                start,
                end,
                source: name.clone(),
            });
        }
    }
}

pub fn semantic_matches(terminal: &Terminal, patterns: &[(String, Regex)]) -> Vec<SemanticMatch> {
    let grid = terminal.grid();
    let offset = i32::try_from(grid.scroll_offset()).unwrap_or(i32::MAX);
    let first = -offset;
    let last = (grid.rows().saturating_sub(1) as i32).saturating_sub(offset);
    let mut matches: Vec<SemanticMatch> = Vec::new();

    'rows: for row_index in first..=last {
        let Some(row) = grid.row_at_offset(row_index) else {
            continue;
        };
        let mut col = 0;
        while col < grid.cols() {
            let id = row.get(col).and_then(|cell| cell.hyperlink_id);
            let Some(id) = id else {
                col += 1;
                continue;
            };
            let start = col;
            while col + 1 < grid.cols()
                && row.get(col + 1).and_then(|cell| cell.hyperlink_id) == Some(id)
            {
                col += 1;
            }
            if let Some(value) = terminal.get_hyperlink_url(id) {
                let continues_wrapped_link = start == 0
                    && grid
                        .row_at_offset(row_index.saturating_sub(1))
                        .is_some_and(|previous| previous.is_wrapped())
                    && matches.last().is_some_and(|previous| {
                        previous.source == "OSC 8 link"
                            && previous.value == value
                            && previous.end.row == row_index.saturating_sub(1)
                            && previous.end.col + 1 == grid.cols()
                    });
                if continues_wrapped_link {
                    if let Some(previous) = matches.last_mut() {
                        previous.end = Point::new(row_index, col);
                    }
                } else {
                    matches.push(SemanticMatch {
                        label: String::new(),
                        value: value.to_string(),
                        start: Point::new(row_index, start),
                        end: Point::new(row_index, col),
                        source: "OSC 8 link".into(),
                    });
                    if matches.len() >= MAX_QUICK_SELECT_MATCHES {
                        break 'rows;
                    }
                }
            }
            col += 1;
        }
    }

    let mut text = String::new();
    let mut spans = Vec::new();
    for row_index in first..=last {
        let Some(row) = grid.row_at_offset(row_index) else {
            continue;
        };
        append_row_text(grid, row_index, &mut text, &mut spans);
        if !row.is_wrapped() || row_index == last {
            append_pattern_matches(&text, &spans, patterns, &mut matches);
            text.clear();
            spans.clear();
            if matches.len() >= MAX_QUICK_SELECT_MATCHES {
                break;
            }
        }
    }

    matches.sort_by_key(|candidate| (candidate.start.row, candidate.start.col));
    let width = label_width(matches.len());
    for (index, candidate) in matches.iter_mut().enumerate() {
        candidate.label = hint_label(index, width);
    }
    matches
}

fn label_width(count: usize) -> usize {
    let mut capacity = HINT_ALPHABET.len();
    let mut width = 1;
    while count > capacity {
        width += 1;
        capacity = capacity.saturating_mul(HINT_ALPHABET.len());
    }
    width
}

fn hint_label(mut index: usize, width: usize) -> String {
    let mut label = vec![HINT_ALPHABET[0]; width];
    for slot in label.iter_mut().rev() {
        *slot = HINT_ALPHABET[index % HINT_ALPHABET.len()];
        index /= HINT_ALPHABET.len();
    }
    String::from_utf8(label).expect("hint alphabet is ASCII")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn motions_selection_and_viewport_cover_scrollback_and_unicode() {
        let mut terminal = Terminal::new(6, 2);
        terminal.process("one\r\nt界o\r\nthree".as_bytes());
        let mut mode = CopyMode::default();
        mode.enter(terminal.grid());
        mode.move_cursor(CopyMotion::Top, terminal.grid_mut());
        assert_eq!(
            mode.cursor().row,
            -(terminal.grid().scrollback_len() as i32)
        );
        mode.move_cursor(CopyMotion::WordForward, terminal.grid_mut());
        mode.toggle_selection(SelectionType::Simple);
        mode.move_cursor(CopyMotion::LineEnd, terminal.grid_mut());
        let selection = mode.selection().unwrap();
        assert!(selection.contains(mode.cursor().row, mode.cursor().col));
        assert!(!is_wide_spacer(terminal.grid(), mode.cursor()));
    }

    #[test]
    fn right_motion_crosses_a_wide_grapheme_instead_of_getting_stuck() {
        let mut terminal = Terminal::new(6, 2);
        terminal.process("a界b".as_bytes());
        let mut mode = CopyMode::default();
        mode.enter(terminal.grid());
        mode.jump_to(Point::new(0, 1), terminal.grid_mut());

        mode.move_cursor(CopyMotion::Right, terminal.grid_mut());

        assert_eq!(mode.cursor(), Point::new(0, 3));
        assert!(!is_wide_spacer(terminal.grid(), mode.cursor()));
    }

    #[test]
    fn quick_select_prioritizes_osc_links_and_maps_regex_bytes_to_cells() {
        let mut terminal = Terminal::new(40, 3);
        terminal.process(b"\x1b]8;;https://example.test\x1b\\link\x1b]8;;\x1b\\ /tmp/a abcdef1");
        let patterns = vec![
            ("path".into(), Regex::new(r"/[^ ]+").unwrap()),
            ("hash".into(), Regex::new(r"\b[0-9a-f]{7,40}\b").unwrap()),
        ];
        let matches = semantic_matches(&terminal, &patterns);
        assert_eq!(matches.len(), 3);
        assert_eq!(matches[0].source, "OSC 8 link");
        assert_eq!(matches[1].value, "/tmp/a");
        assert_eq!(matches[2].value, "abcdef1");
        assert_eq!(matches[0].label, "a");
    }

    #[test]
    fn one_osc_link_keeps_one_hint_when_it_wraps() {
        let mut terminal = Terminal::new(5, 3);
        terminal.process(b"\x1b]8;;https://example.test\x1b\\abcdef\x1b]8;;\x1b\\");
        let matches = semantic_matches(&terminal, &[]);
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].start, Point::new(0, 0));
        assert_eq!(matches[0].end, Point::new(1, 0));
    }

    #[test]
    fn wrapped_osc_link_suppresses_regex_hints_on_every_covered_row() {
        let mut terminal = Terminal::new(5, 3);
        terminal.process(b"\x1b]8;;https://example.test\x1b\\abcdefgh\x1b]8;;\x1b\\");
        let patterns = vec![("word".into(), Regex::new(r"[a-z]+").unwrap())];

        let matches = semantic_matches(&terminal, &patterns);

        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].source, "OSC 8 link");
        assert_eq!(matches[0].end, Point::new(1, 2));
    }

    #[test]
    fn configured_regex_can_span_soft_wrapped_rows() {
        let mut terminal = Terminal::new(5, 4);
        terminal.process(b"xx /tmp/example yy");
        let patterns = vec![("path".into(), Regex::new(r"/[^ ]+").unwrap())];

        let matches = semantic_matches(&terminal, &patterns);

        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].value, "/tmp/example");
        assert_eq!(matches[0].start, Point::new(0, 3));
        assert_eq!(matches[0].end, Point::new(2, 4));
    }

    #[test]
    fn configured_pattern_matches_have_a_hard_visible_viewport_cap() {
        let mut terminal = Terminal::new(100, 50);
        terminal.process("a".repeat(5_000).as_bytes());
        let patterns = vec![("cell".into(), Regex::new("a").unwrap())];

        let matches = semantic_matches(&terminal, &patterns);

        assert_eq!(matches.len(), MAX_QUICK_SELECT_MATCHES);
    }

    #[test]
    fn character_line_and_block_selections_remain_explicit() {
        let mut terminal = Terminal::new(8, 3);
        terminal.process(b"alpha\r\nbeta");
        let mut mode = CopyMode::default();
        mode.enter(terminal.grid());
        mode.jump_to(Point::new(0, 1), terminal.grid_mut());
        for ty in [
            SelectionType::Simple,
            SelectionType::Lines,
            SelectionType::Block,
        ] {
            mode.toggle_selection(ty);
            mode.move_cursor(CopyMotion::Down, terminal.grid_mut());
            assert_eq!(mode.selection().unwrap().ty, ty);
            mode.toggle_selection(ty);
            mode.move_cursor(CopyMotion::Up, terminal.grid_mut());
        }
    }

    #[test]
    fn hint_labels_are_stable_and_prefix_activation_is_exact() {
        assert_eq!(hint_label(0, 1), "a");
        assert_eq!(hint_label(25, 1), "m");
        assert_eq!(hint_label(26, 2), "sa");
        let mut mode = CopyMode {
            active: true,
            quick_select: Some(QuickSelect {
                matches: vec![SemanticMatch {
                    label: "a".into(),
                    value: "https://example.test".into(),
                    start: Point::new(0, 0),
                    end: Point::new(0, 3),
                    source: "test".into(),
                }],
                prefix: String::new(),
                action: HintAction::Copy,
            }),
            ..CopyMode::default()
        };
        let activated = mode.push_hint_key('a').unwrap();
        assert_eq!(activated.0.value, "https://example.test");
        assert_eq!(activated.1, HintAction::Copy);
    }

    #[test]
    fn cursor_and_selection_follow_logical_text_across_reflow() {
        let mut terminal = Terminal::new(8, 3);
        terminal.process(b"abcdefghijklmno");
        let mut mode = CopyMode::default();
        mode.enter(terminal.grid());
        mode.jump_to(Point::new(1, 2), terminal.grid_mut());
        mode.toggle_selection(SelectionType::Simple);
        mode.move_cursor(CopyMotion::Right, terminal.grid_mut());
        mode.prepare_reflow(terminal.grid());
        terminal.resize(5, 4);
        mode.finish_reflow(terminal.grid_mut());
        assert_eq!(mode.cursor(), Point::new(2, 1));
        assert_eq!(mode.selection().unwrap().start, Point::new(2, 0));
    }

    #[test]
    fn quick_select_rebuilds_coordinates_and_retains_action_after_reflow() {
        let mut terminal = Terminal::new(24, 3);
        terminal.process(b"abcdefghijkl /tmp/x");
        let patterns = vec![("path".into(), Regex::new(r"/[^ ]+").unwrap())];
        let mut mode = CopyMode::default();
        mode.enter(terminal.grid());
        mode.start_quick_select(&terminal, &patterns, HintAction::Copy);
        assert_eq!(mode.quick_select().unwrap().matches[0].start.row, 0);

        mode.prepare_reflow(terminal.grid());
        terminal.resize(12, 4);
        mode.finish_reflow(terminal.grid_mut());
        mode.refresh_quick_select(&terminal, &patterns);

        let quick = mode.quick_select().unwrap();
        assert_eq!(quick.action, HintAction::Copy);
        assert_eq!(quick.matches[0].value, "/tmp/x");
        assert!(quick.matches[0].start.row > 0);
    }
}
