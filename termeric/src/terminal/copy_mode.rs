//! Keyboard-only scrollback navigation, selection, and semantic hints.

use regex::Regex;
use std::collections::HashMap;
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
    pub working_directory: Option<String>,
    synchronized_output_deadline: Option<std::time::Instant>,
    source: QuickSelectSource,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct QuickSelectSource {
    revision: (u64, u64),
    viewport: (usize, usize, usize),
}

impl QuickSelectSource {
    fn capture(terminal: &Terminal) -> Self {
        let grid = terminal.presentation_grid();
        Self {
            revision: grid.content_revision(),
            viewport: (grid.cols(), grid.rows(), grid.scroll_offset()),
        }
    }
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
    reflow_bookmark: Option<termeric_core::grid::ResizeTrackingToken>,
    synchronized_output_deadline: Option<std::time::Instant>,
    presentation_identity: Option<u64>,
    live_revision: Option<(u64, u64)>,
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
            synchronized_output_deadline: None,
            presentation_identity: None,
            live_revision: None,
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
        self.presentation_identity = Some(grid.content_revision().0);
        self.live_revision = None;
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
        self.preferred_col = self.cursor.col;
    }

    pub fn enter_presentation(&mut self, terminal: &Terminal) {
        self.enter(terminal.presentation_grid());
        self.reflow_bookmark = None;
        self.synchronized_output_deadline = terminal.synchronized_output_deadline();
    }

    pub fn presentation_matches(&self, terminal: &Terminal) -> bool {
        self.synchronized_output_deadline == terminal.synchronized_output_deadline()
            && self.presentation_identity == Some(terminal.presentation_grid().content_revision().0)
    }

    /// Publish an invisible live anchor independently of whether a range is
    /// selected. Frozen coordinates stay solely in the modal presentation.
    pub fn sync_live_cursor(&mut self, terminal: &mut Terminal) {
        terminal.set_navigation_cursor(
            terminal
                .synchronized_output_deadline()
                .is_none()
                .then_some(self.cursor),
        );
        self.live_revision = Some(terminal.presentation_grid().content_revision());
    }

    pub fn owns_navigation_cursor(&self, terminal: &Terminal) -> bool {
        self.live_revision
            .is_some_and(|revision| revision.0 == terminal.grid().content_revision().0)
    }

    pub fn reconcile_live_output(&mut self, terminal: &mut Terminal) {
        if self.reflow_bookmark.is_some()
            || terminal.synchronized_output_deadline().is_some()
            || self.live_revision == Some(terminal.grid().content_revision())
        {
            return;
        }
        let previous = self.cursor;
        if let Some(cursor) = terminal.navigation_cursor() {
            self.cursor = cursor;
            if self.anchor.is_some() {
                self.anchor = terminal.selection().map(|selection| selection.start);
                if let Some(selection) = terminal.selection() {
                    self.selection_type = selection.ty;
                }
            }
        } else {
            self.cursor = Point::new(
                terminal.grid().cursor().row as i32,
                terminal.grid().cursor().col,
            );
            self.anchor = None;
            terminal.set_selection(None);
        }
        self.clamp(terminal.grid());
        if previous.col != self.cursor.col {
            self.preferred_col = self.cursor.col;
        }
        self.pending_g = false;
        self.sync_live_cursor(terminal);
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
        working_directory: Option<String>,
    ) {
        if !self.active {
            self.enter_presentation(terminal);
        }
        self.quick_select = Some(QuickSelect {
            matches: semantic_matches(terminal, patterns),
            prefix: String::new(),
            action,
            working_directory,
            synchronized_output_deadline: terminal.synchronized_output_deadline(),
            source: QuickSelectSource::capture(terminal),
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

    pub fn quick_select_presentation_matches(&self, terminal: &Terminal) -> bool {
        self.quick_select.as_ref().is_none_or(|quick| {
            quick.synchronized_output_deadline == terminal.synchronized_output_deadline()
                && quick.source.revision.0 == terminal.presentation_grid().content_revision().0
        })
    }

    /// Revalidate captured targets after output, viewport movement, or reflow.
    /// Labels never transfer to a different payload/source. New targets require
    /// a fresh invocation, preserving the captured action and directory.
    pub fn refresh_quick_select(&mut self, terminal: &Terminal, patterns: &[(String, Regex)]) {
        if !self.quick_select_presentation_matches(terminal) {
            self.cancel_quick_select();
            return;
        }
        let Some(quick) = self.quick_select.as_mut() else {
            return;
        };
        let source = QuickSelectSource::capture(terminal);
        if quick.source == source {
            return;
        }
        // Borrow payload keys from the bounded old match list. Reverse stacks
        // pair identical targets in display order without a quadratic search
        // or cloned payload keys; identical actions retain distinct labels.
        let mut captured: HashMap<&str, HashMap<&str, Vec<&SemanticMatch>>> = HashMap::new();
        for previous in quick.matches.iter().rev() {
            captured
                .entry(&previous.value)
                .or_default()
                .entry(&previous.source)
                .or_default()
                .push(previous);
        }
        let matches: Vec<_> = semantic_matches(terminal, patterns)
            .into_iter()
            .filter_map(|mut candidate| {
                let previous = captured
                    .get_mut(candidate.value.as_str())?
                    .get_mut(candidate.source.as_str())?
                    .pop()?;
                candidate.label.clone_from(&previous.label);
                Some(candidate)
            })
            .collect();
        if matches.is_empty()
            || !matches
                .iter()
                .any(|candidate| candidate.label.starts_with(&quick.prefix))
        {
            self.cancel_quick_select();
            return;
        }
        quick.matches = matches;
        quick.source = source;
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

    pub fn prepare_reflow(&mut self, grid: &mut Grid) {
        if self.active {
            self.reflow_bookmark =
                Some(grid.begin_resize_tracking(self.cursor, self.anchor, self.selection_type));
        }
    }

    pub fn finish_reflow(&mut self, grid: &mut Grid) {
        let Some(token) = self.reflow_bookmark.take() else {
            return;
        };
        let [cursor, anchor] = grid.finish_resize_tracking(token).unwrap_or([None; 2]);
        // A missing cursor no longer names retained text. Restart navigation
        // at the terminal cursor, never transfer its old selection elsewhere.
        self.cursor =
            cursor.unwrap_or_else(|| Point::new(grid.cursor().row as i32, grid.cursor().col));
        self.anchor = cursor.and(anchor);
        self.pending_g = false;
        self.preferred_col = self.cursor.col;
        self.clamp(grid);
        reveal_cursor(grid, self.cursor);
    }
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
    for col in row.reflow_columns() {
        let Some(cell) = row.get(col) else { break };
        if cell.multicell.is_present() && !cell.multicell.is_leader() {
            continue;
        }
        let byte_start = text.len();
        match &cell.content {
            CellContent::Empty => text.push(' '),
            CellContent::Char(character) => text.push(*character),
            CellContent::Grapheme(grapheme) => text.push_str(grapheme),
        }
        let display_width = cell.width();
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
    let grid = terminal.presentation_grid();
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
                    && matches.last().is_some_and(|previous| {
                        previous.source == "OSC 8 link"
                            && previous.value == value
                            && previous.end.row == row_index.saturating_sub(1)
                            && grid.row_at_offset(previous.end.row).is_some_and(|row| {
                                row.is_wrapped()
                                    && previous.end.col + 1
                                        == row.len().saturating_sub(usize::from(
                                            row.has_wide_wrap_padding(),
                                        ))
                            })
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
    fn quick_select_snapshot_identity_survives_resize_but_not_release_or_restart() {
        let patterns = vec![("path".into(), Regex::new(r"\./[^ ]+").unwrap())];
        for restart in [false, true] {
            let mut terminal = Terminal::new(30, 3);
            terminal.process(b"./visible\x1b[?2026h\x1b[H\x1b[2K./hidden");
            let mut mode = CopyMode::default();
            mode.start_quick_select(
                &terminal,
                &patterns,
                HintAction::Copy,
                Some("/visible".into()),
            );
            terminal.resize(6, 4);
            mode.refresh_quick_select(&terminal, &patterns);
            assert_eq!(mode.quick_select().unwrap().matches[0].value, "./visible");
            assert_eq!(
                mode.quick_select().unwrap().working_directory.as_deref(),
                Some("/visible")
            );
            terminal.process(b"\x1b[?2026l");
            if restart {
                terminal.process(b"\x1b[?2026h");
            }
            assert!(!mode.quick_select_presentation_matches(&terminal));
            mode.refresh_quick_select(&terminal, &patterns);
            assert!(mode.quick_select().is_none());

            terminal.process(b"\x1b[?2026l");
            mode.start_quick_select(&terminal, &patterns, HintAction::Copy, None);
            terminal.process(b"\x1b[?2026h");
            assert!(!mode.quick_select_presentation_matches(&terminal));
            mode.refresh_quick_select(&terminal, &patterns);
            assert!(mode.quick_select().is_none());
        }
    }

    #[test]
    fn quick_select_reads_the_frozen_presentation_not_hidden_replacements() {
        let mut terminal = Terminal::new(40, 3);
        terminal.process(b"\x1b]8;;https://visible.test\x1b\\link\x1b]8;;\x1b\\ ./visible\x1b[?2026h\x1b[H\x1b[2K\x1b]8;;https://hidden.test\x1b\\link\x1b]8;;\x1b\\ ./hidden");
        let patterns = vec![("path".into(), Regex::new(r"\./[^ ]+").unwrap())];
        let matches = semantic_matches(&terminal, &patterns);
        assert_eq!(
            matches
                .iter()
                .map(|item| item.value.as_str())
                .collect::<Vec<_>>(),
            ["https://visible.test", "./visible"]
        );
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
    fn quick_select_regex_ignores_only_synthetic_wide_wrap_padding() {
        for text in ["ab界Z", "ab👩\u{200d}💻Z"] {
            let patterns = vec![("unicode".into(), Regex::new(&regex::escape(text)).unwrap())];
            for archived in [false, true] {
                let mut terminal = Terminal::new(3, 3);
                terminal.process(text.as_bytes());
                if archived {
                    terminal.process(b"\r\nx\r\ny\r\nz\r\nw");
                    let history = terminal.grid().scrollback_len();
                    terminal.grid_mut().set_scroll_offset(history);
                }
                let first = -(terminal.grid().scroll_offset() as i32);
                let matches = semantic_matches(&terminal, &patterns);
                assert_eq!(matches.len(), 1, "text={text} archived={archived}");
                assert_eq!(matches[0].value, text);
                assert_eq!(matches[0].start, Point::new(first, 0));
                assert_eq!(matches[0].end, Point::new(first + 1, 2));
            }
        }

        let mut terminal = Terminal::new(3, 3);
        terminal.process("ab界Z".as_bytes());
        terminal.process(b"\x1b[1;3H "); // A real blank replaces synthetic padding.
        let patterns = vec![("blank".into(), Regex::new("ab ?界Z").unwrap())];
        let matches = semantic_matches(&terminal, &patterns);
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].value, "ab 界Z");

        let mut terminal = Terminal::new(3, 3);
        terminal.process("ab\r\n界Z".as_bytes());
        assert!(semantic_matches(&terminal, &patterns).is_empty());
    }

    #[test]
    fn quick_select_osc_link_crosses_synthetic_padding_but_not_real_gaps() {
        let mut terminal = Terminal::new(3, 3);
        terminal.process("\x1b]8;;https://example.test\x1b\\ab界Z\x1b]8;;\x1b\\".as_bytes());
        let patterns = vec![("unicode".into(), Regex::new("ab界Z").unwrap())];
        let matches = semantic_matches(&terminal, &patterns);
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].source, "OSC 8 link");
        assert_eq!(matches[0].start, Point::new(0, 0));
        assert_eq!(matches[0].end, Point::new(1, 2));

        terminal.process(b"\x1b[1;3H ");
        assert_eq!(semantic_matches(&terminal, &patterns).len(), 2);

        let mut terminal = Terminal::new(3, 3);
        terminal.process(
            "\x1b]8;;https://example.test\x1b\\ab界Z\x1b]8;;\x1b\\\r\nx\r\ny\r\nz\r\nw".as_bytes(),
        );
        let history = terminal.grid().scrollback_len();
        terminal.grid_mut().set_scroll_offset(history);
        let matches = semantic_matches(&terminal, &patterns);
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].start, Point::new(-(history as i32), 0));
        assert_eq!(matches[0].end, Point::new(1 - history as i32, 2));

        let mut terminal = Terminal::new(3, 3);
        terminal.process("\x1b]8;;https://example.test\x1b\\ab\r\n界Z\x1b]8;;\x1b\\".as_bytes());
        assert_eq!(semantic_matches(&terminal, &patterns).len(), 2);
    }

    #[test]
    fn quick_select_unicode_hints_survive_padding_created_by_reflow() {
        let patterns = vec![("unicode".into(), Regex::new("ab界Z").unwrap())];
        for linked in [false, true] {
            let mut terminal = Terminal::new(5, 6);
            if linked {
                terminal.process(b"\x1b]8;;https://example.test\x1b\\");
            }
            terminal.process("ab界Z".as_bytes());
            terminal.process(b"\x1b]8;;\x1b\\");
            let mut mode = CopyMode::default();
            mode.enter(terminal.grid());
            mode.start_quick_select(&terminal, &patterns, HintAction::Copy, None);
            for (cols, end) in [
                (3, Point::new(1, 2)),
                (1, Point::new(3, 0)),
                (5, Point::new(0, 4)),
            ] {
                mode.prepare_reflow(terminal.grid_mut());
                terminal.resize(cols, 6);
                mode.finish_reflow(terminal.grid_mut());
                mode.refresh_quick_select(&terminal, &patterns);
                let quick = mode.quick_select().unwrap();
                assert_eq!(quick.action, HintAction::Copy);
                assert_eq!(quick.matches.len(), 1, "cols={cols} linked={linked}");
                assert_eq!(quick.matches[0].start, Point::new(0, 0));
                assert_eq!(quick.matches[0].end, end);
                assert_eq!(
                    quick.matches[0].value,
                    if linked {
                        "https://example.test"
                    } else {
                        "ab界Z"
                    }
                );
            }
        }
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
    fn quick_select_updates_positions_without_rebinding_captured_labels() {
        let mut terminal = Terminal::new(40, 3);
        terminal.process(b"/one /two");
        let patterns = vec![("path".into(), Regex::new(r"/\w+").unwrap())];
        let mut mode = CopyMode::default();
        mode.start_quick_select(&terminal, &patterns, HintAction::Copy, None);
        let label = mode.quick_select().unwrap().matches[1].label.clone();
        terminal.process(b"\r\x1b[2K/two /new");
        mode.refresh_quick_select(&terminal, &patterns);
        let quick = mode.quick_select().unwrap();
        assert_eq!(
            quick.matches.len(),
            1,
            "new output must not acquire an old label"
        );
        assert_eq!(quick.matches[0].value, "/two");
        assert_eq!(quick.matches[0].label, label);
        assert_eq!(quick.matches[0].start, Point::new(0, 0));
    }

    #[test]
    fn quick_select_partial_labels_survive_only_for_their_original_targets() {
        let patterns = vec![("path".into(), Regex::new(r"/\d+").unwrap())];
        let original = (0..27)
            .map(|index| format!("/{index:02}"))
            .collect::<Vec<_>>()
            .join(" ");
        for survives in [false, true] {
            let mut terminal = Terminal::new(80, 4);
            terminal.process(original.as_bytes());
            let mut mode = CopyMode::default();
            mode.start_quick_select(
                &terminal,
                &patterns,
                HintAction::Copy,
                Some("/captured".into()),
            );
            let label = mode.quick_select().unwrap().matches[26].label.clone();
            assert_eq!(label.len(), 2);
            assert!(mode.push_hint_key(label.chars().next().unwrap()).is_none());
            terminal.process(b"\x1b[H\x1b[2J");
            terminal.process(if survives { b"/26 /99" } else { b"/00 /99" });
            mode.refresh_quick_select(&terminal, &patterns);
            if survives {
                let quick = mode.quick_select().unwrap();
                assert_eq!(quick.prefix.len(), 1);
                assert_eq!(quick.matches.len(), 1);
                assert_eq!(quick.working_directory.as_deref(), Some("/captured"));
                let (target, action) = mode.push_hint_key(label.chars().nth(1).unwrap()).unwrap();
                assert_eq!(target.value, "/26");
                assert_eq!(action, HintAction::Copy);
            } else {
                assert!(mode.quick_select().is_none());
                assert!(mode.push_hint_key(label.chars().nth(1).unwrap()).is_none());
            }
        }
    }

    #[test]
    fn quick_select_tracks_viewport_and_ignores_hidden_frozen_output() {
        let patterns = vec![("path".into(), Regex::new(r"/\w+").unwrap())];
        let mut terminal = Terminal::new(30, 3);
        terminal.process(b"/one\r\n/two\r\n/three");
        let mut mode = CopyMode::default();
        mode.start_quick_select(&terminal, &patterns, HintAction::Open, None);
        let label = mode.quick_select().unwrap().matches[1].label.clone();
        let allocation = mode.quick_select().unwrap().matches.as_ptr();
        terminal.process(b"\x1b[3;9H"); // Cursor movement is not a content change.
        mode.refresh_quick_select(&terminal, &patterns);
        assert_eq!(mode.quick_select().unwrap().matches.as_ptr(), allocation);
        terminal.process(b"\r\n/new");
        mode.refresh_quick_select(&terminal, &patterns);
        assert_eq!(mode.quick_select().unwrap().matches.len(), 2);
        assert_eq!(mode.quick_select().unwrap().matches[0].value, "/two");
        assert_eq!(mode.quick_select().unwrap().matches[0].label, label);
        assert_eq!(
            mode.quick_select().unwrap().matches[0].start,
            Point::new(0, 0)
        );
        terminal.grid_mut().set_scroll_offset(1);
        mode.refresh_quick_select(&terminal, &patterns);
        assert_eq!(mode.quick_select().unwrap().source.viewport.2, 1);
        assert!(
            !mode
                .quick_select()
                .unwrap()
                .matches
                .iter()
                .any(|item| item.value == "/one"),
            "retired labels are never resurrected"
        );

        terminal.process(b"\x1b[?2026h");
        mode.start_quick_select(&terminal, &patterns, HintAction::Copy, None);
        let frozen = mode.quick_select().unwrap().clone();
        terminal.process(b"\x1b[H\x1b[2J/hidden\r\nextra\r\nmore");
        mode.refresh_quick_select(&terminal, &patterns);
        assert_eq!(mode.quick_select(), Some(&frozen));
        terminal.process(b"\x1b[?2026l");
        mode.refresh_quick_select(&terminal, &patterns);
        assert!(mode.quick_select().is_none());
    }

    #[test]
    fn quick_select_duplicate_targets_keep_distinct_bounded_labels() {
        let patterns = vec![("cell".into(), Regex::new("a").unwrap())];
        let mut terminal = Terminal::new(100, 50);
        terminal.process("a".repeat(5_000).as_bytes());
        let mut mode = CopyMode::default();
        mode.start_quick_select(&terminal, &patterns, HintAction::Copy, None);
        let labels: Vec<_> = mode
            .quick_select()
            .unwrap()
            .matches
            .iter()
            .map(|item| item.label.clone())
            .collect();
        terminal.process(b"\x1b[1;1Haa");
        mode.refresh_quick_select(&terminal, &patterns);
        let quick = mode.quick_select().unwrap();
        assert_eq!(quick.matches.len(), MAX_QUICK_SELECT_MATCHES);
        assert_eq!(
            quick
                .matches
                .iter()
                .map(|item| item.label.clone())
                .collect::<Vec<_>>(),
            labels
        );
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
                working_directory: None,
                synchronized_output_deadline: None,
                source: QuickSelectSource::capture(&Terminal::new(8, 3)),
            }),
            ..CopyMode::default()
        };
        let activated = mode.push_hint_key('a').unwrap();
        assert_eq!(activated.0.value, "https://example.test");
        assert_eq!(activated.1, HintAction::Copy);
    }

    #[test]
    fn copy_resize_tracks_retained_text_instead_of_history_relative_line_numbers() {
        fn locate(term: &Terminal, character: char) -> Option<Point> {
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
        }
        for limit in [0, 1, 8] {
            for cols in [1, 2, 4, 8, 12] {
                for rows in [1, 3, 8] {
                    for (first, last) in
                        [('A', 'B'), ('C', 'J'), ('K', 'N'), ('O', 'V'), ('S', 'V')]
                    {
                        for reverse in [false, true] {
                            let mut terminal = Terminal::new(8, 3);
                            terminal.grid_mut().set_scrollback_limit(limit);
                            terminal.process(b"AB\r\nCDEFGHIJ\r\nKLMN\r\nOPQRSTUV");
                            let (Some(mut start), Some(mut end)) =
                                (locate(&terminal, first), locate(&terminal, last))
                            else {
                                continue;
                            };
                            if reverse {
                                std::mem::swap(&mut start, &mut end);
                            }
                            let mut mode = CopyMode::default();
                            mode.enter(terminal.grid());
                            mode.jump_to(start, terminal.grid_mut());
                            mode.toggle_selection(SelectionType::Simple);
                            mode.jump_to(end, terminal.grid_mut());
                            terminal.set_selection(mode.selection());
                            let original = terminal.get_selection_text();
                            mode.prepare_reflow(terminal.grid_mut());
                            terminal.resize(cols, rows);
                            mode.finish_reflow(terminal.grid_mut());
                            if let (Some(mut start), Some(mut end)) =
                                (locate(&terminal, first), locate(&terminal, last))
                            {
                                if reverse {
                                    std::mem::swap(&mut start, &mut end);
                                }
                                let selection =
                                    mode.selection().expect("retained endpoints must survive");
                                assert_eq!(
                                    (selection.start, selection.end),
                                    (start, end),
                                    "limit={limit} cols={cols} rows={rows}"
                                );
                                terminal.set_selection(Some(selection));
                                assert_eq!(terminal.get_selection_text(), original);
                            } else {
                                assert!(
                                    mode.selection().is_none(),
                                    "evicted or clipped endpoint cannot alias other text"
                                );
                            }
                            assert!(mode.is_active());
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn copy_resize_retires_geometric_ranges_but_keeps_the_surviving_cursor() {
        for ty in [SelectionType::Block, SelectionType::Lines] {
            let mut terminal = Terminal::new(8, 4);
            terminal.process(b"abcdefghijklmno");
            let mut mode = CopyMode::default();
            mode.enter(terminal.grid());
            mode.jump_to(Point::new(0, 1), terminal.grid_mut());
            mode.toggle_selection(ty);
            mode.jump_to(Point::new(1, 2), terminal.grid_mut());
            mode.prepare_reflow(terminal.grid_mut());
            terminal.resize(8, 3);
            mode.finish_reflow(terminal.grid_mut());
            assert_eq!(mode.selection().unwrap().ty, ty);
            mode.prepare_reflow(terminal.grid_mut());
            terminal.resize(5, 4);
            mode.finish_reflow(terminal.grid_mut());
            assert!(mode.selection().is_none());
            assert_eq!(mode.cursor(), Point::new(2, 0));
        }
    }

    #[test]
    fn copy_resize_glyph_repair_only_retires_damaged_selection() {
        for selected_block in [false, true] {
            let mut terminal = Terminal::new(8, 4);
            terminal.process(b"abcd\x1b]66;s=2;X\x07");
            let mut mode = CopyMode::default();
            mode.enter(terminal.grid());
            mode.jump_to(
                Point::new(0, if selected_block { 4 } else { 0 }),
                terminal.grid_mut(),
            );
            mode.toggle_selection(SelectionType::Simple);
            mode.prepare_reflow(terminal.grid_mut());
            terminal.resize(3, 6);
            mode.finish_reflow(terminal.grid_mut());
            assert_eq!(mode.selection().is_none(), selected_block);
            if !selected_block {
                terminal.set_selection(mode.selection());
                assert_eq!(terminal.get_selection_text().as_deref(), Some("a"));
            }
        }
    }

    #[test]
    fn resize_bookmarks_do_not_alias_evicted_history() {
        let mut terminal = Terminal::new(8, 3);
        terminal.grid_mut().set_scrollback_limit(1);
        terminal.process(b"abcdefgh\r\nijklmnop\r\nqrstuvwx");
        let mut mode = CopyMode::default();
        mode.enter(terminal.grid());
        mode.jump_to(Point::new(0, 0), terminal.grid_mut());
        mode.toggle_selection(SelectionType::Simple);
        mode.jump_to(Point::new(0, 3), terminal.grid_mut());
        mode.prepare_reflow(terminal.grid_mut());
        terminal.resize(2, 2);
        mode.finish_reflow(terminal.grid_mut());
        assert!(
            mode.selection().is_none(),
            "evicted text must not select a different logical line"
        );
    }

    #[test]
    fn unicode_copy_bookmarks_follow_both_endpoints_through_width_changes() {
        let mut terminal = Terminal::new(5, 6);
        terminal.process("ab界ZQ".as_bytes());
        let mut mode = CopyMode::default();
        mode.enter(terminal.grid());
        mode.jump_to(Point::new(0, 2), terminal.grid_mut());
        mode.toggle_selection(SelectionType::Simple);
        mode.jump_to(Point::new(1, 0), terminal.grid_mut());
        for cols in [3, 1, 5, 3, 5] {
            mode.prepare_reflow(terminal.grid_mut());
            terminal.resize(cols, 6);
            mode.finish_reflow(terminal.grid_mut());
            let selection = mode.selection().unwrap();
            for (point, expected) in [(mode.cursor(), "Q"), (selection.start, "界")] {
                assert_eq!(
                    terminal
                        .grid()
                        .row_at_offset(point.row)
                        .unwrap()
                        .get(point.col)
                        .unwrap()
                        .content
                        .to_text(),
                    expected,
                    "width={cols} point={point:?}"
                );
            }
        }
    }

    #[test]
    fn unicode_history_selection_keeps_text_and_direction_through_reflow() {
        for reverse in [false, true] {
            let mut terminal = Terminal::new(5, 3);
            terminal.process("ab界ZQ\r\nx\r\ny\r\nz".as_bytes());
            assert_eq!(terminal.grid().scrollback_len(), 2);
            let (start, end) = (Point::new(-2, 2), Point::new(-1, 0));
            let (start, end) = if reverse { (end, start) } else { (start, end) };
            let mut mode = CopyMode::default();
            mode.enter(terminal.grid());
            mode.jump_to(start, terminal.grid_mut());
            mode.toggle_selection(SelectionType::Simple);
            mode.jump_to(end, terminal.grid_mut());
            for cols in [3, 1, 5] {
                mode.prepare_reflow(terminal.grid_mut());
                terminal.resize(cols, 3);
                mode.finish_reflow(terminal.grid_mut());
                let selection = mode.selection().unwrap();
                assert_eq!(selection.start > selection.end, reverse);
                terminal.set_selection(Some(selection));
                assert_eq!(
                    terminal.get_selection_text().as_deref(),
                    Some("界ZQ"),
                    "width={cols} reverse={reverse}"
                );
            }
        }
    }

    #[test]
    fn unicode_selection_survives_reflow_across_the_history_viewport_boundary() {
        for reverse in [false, true] {
            let mut terminal = Terminal::new(3, 2);
            terminal.process("ab界ZQ".as_bytes());
            assert_eq!(terminal.grid().scrollback_len(), 1);
            let (start, end) = (Point::new(-1, 0), Point::new(1, 0));
            let (start, end) = if reverse { (end, start) } else { (start, end) };
            let mut mode = CopyMode::default();
            mode.enter(terminal.grid());
            mode.jump_to(start, terminal.grid_mut());
            mode.toggle_selection(SelectionType::Simple);
            mode.jump_to(end, terminal.grid_mut());
            for cols in [5, 1, 3, 5] {
                mode.prepare_reflow(terminal.grid_mut());
                terminal.resize(cols, 2);
                mode.finish_reflow(terminal.grid_mut());
                let selection = mode.selection().unwrap();
                assert_eq!(selection.start > selection.end, reverse);
                terminal.set_selection(Some(selection));
                assert_eq!(
                    terminal.get_selection_text().as_deref(),
                    Some("ab界ZQ"),
                    "cols={cols} reverse={reverse}"
                );
                if cols == 5 {
                    assert_eq!(terminal.grid().scrollback_len(), 0);
                }
            }
        }
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
        mode.prepare_reflow(terminal.grid_mut());
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
        mode.start_quick_select(&terminal, &patterns, HintAction::Copy, None);
        assert_eq!(mode.quick_select().unwrap().matches[0].start.row, 0);

        mode.prepare_reflow(terminal.grid_mut());
        terminal.resize(12, 4);
        mode.finish_reflow(terminal.grid_mut());
        mode.refresh_quick_select(&terminal, &patterns);

        let quick = mode.quick_select().unwrap();
        assert_eq!(quick.action, HintAction::Copy);
        assert_eq!(quick.matches[0].value, "/tmp/x");
        assert!(quick.matches[0].start.row > 0);
    }
}
