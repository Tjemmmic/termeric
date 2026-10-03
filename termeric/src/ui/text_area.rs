//! A multi-line text editor buffer: caret, selection, and undo.
//!
//! Layout-dependent movement (up and down across wrapped lines, clicks)
//! takes the paragraph's [`TextLayout`] and a width measure, so the editor
//! stays independent of fonts and can be tested with fixed advances.

use unicode_segmentation::UnicodeSegmentation;

use super::text_input::{TextInputEdit, edit_grapheme};
use super::text_layout::TextLayout;

/// Undo snapshots kept per editor.
const UNDO_LIMIT: usize = 200;

#[derive(Clone, Debug, PartialEq, Eq)]
struct Snapshot {
    text: String,
    caret: usize,
}

/// What the last edit was, so consecutive typing undoes as one step.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum EditKind {
    Typing,
    Deleting,
    Other,
}

/// A multi-line editable text buffer.
#[derive(Clone, Debug, Default)]
pub struct TextArea {
    text: String,
    caret: usize,
    /// The other end of the selection, when there is one.
    anchor: Option<usize>,
    /// Horizontal position kept across vertical moves.
    preferred_x: Option<f32>,
    undo: Vec<Snapshot>,
    redo: Vec<Snapshot>,
    last_edit: Option<EditKind>,
}

impl TextArea {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    pub fn is_empty(&self) -> bool {
        self.text.is_empty()
    }

    /// The caret's byte offset.
    pub fn caret(&self) -> usize {
        self.caret
    }

    /// The selected byte range, if any text is selected.
    pub fn selection(&self) -> Option<std::ops::Range<usize>> {
        let anchor = self.anchor?;
        (anchor != self.caret).then(|| anchor.min(self.caret)..anchor.max(self.caret))
    }

    pub fn selected_text(&self) -> Option<&str> {
        self.selection().map(|range| &self.text[range])
    }

    /// Replaces everything, as a single undoable step.
    pub fn set_text(&mut self, text: &str) {
        self.checkpoint(EditKind::Other);
        self.text = normalize(text);
        self.caret = self.text.len();
        self.anchor = None;
        self.preferred_x = None;
    }

    /// Empties the editor and forgets its history.
    pub fn clear(&mut self) {
        *self = Self::default();
    }

    fn checkpoint(&mut self, kind: EditKind) {
        let merges = self.last_edit == Some(kind) && kind != EditKind::Other;
        if !merges {
            self.undo.push(Snapshot {
                text: self.text.clone(),
                caret: self.caret,
            });
            if self.undo.len() > UNDO_LIMIT {
                self.undo.remove(0);
            }
        }
        self.redo.clear();
        self.last_edit = Some(kind);
    }

    fn delete_selection(&mut self) -> bool {
        let Some(range) = self.selection() else {
            self.anchor = None;
            return false;
        };
        self.text.replace_range(range.clone(), "");
        self.caret = range.start;
        self.anchor = None;
        true
    }

    /// Inserts text at the caret, replacing any selection.
    pub fn insert(&mut self, text: &str) {
        let text = normalize(text);
        if text.is_empty() && self.selection().is_none() {
            return;
        }
        let kind = if text.chars().count() == 1 && !text.contains('\n') {
            EditKind::Typing
        } else {
            EditKind::Other
        };
        self.checkpoint(if self.selection().is_some() {
            EditKind::Other
        } else {
            kind
        });
        self.delete_selection();
        self.text.insert_str(self.caret, &text);
        self.caret += text.len();
        self.preferred_x = None;
    }

    /// Applies a horizontal edit or movement. With `extend`, movement grows
    /// the selection instead of clearing it. Returns whether the text changed.
    pub fn edit(&mut self, edit: TextInputEdit, extend: bool) -> bool {
        self.preferred_x = None;
        let deletes = matches!(
            edit,
            TextInputEdit::Backspace
                | TextInputEdit::Delete
                | TextInputEdit::BackspaceWord
                | TextInputEdit::DeleteWord
        );
        if deletes {
            if self.selection().is_some() {
                self.checkpoint(EditKind::Other);
                return self.delete_selection();
            }
            let mut text = self.text.clone();
            let mut caret = self.caret;
            if !edit_grapheme(&mut text, &mut caret, edit) {
                return false;
            }
            self.checkpoint(EditKind::Deleting);
            self.text = text;
            self.caret = caret;
            self.anchor = None;
            return true;
        }
        // Moving without extending collapses a selection to its edge.
        if !extend && let Some(range) = self.selection() {
            self.anchor = None;
            match edit {
                TextInputEdit::Left | TextInputEdit::WordLeft => {
                    self.caret = range.start;
                    return false;
                }
                TextInputEdit::Right | TextInputEdit::WordRight => {
                    self.caret = range.end;
                    return false;
                }
                _ => {}
            }
        }
        self.begin_move(extend);
        match edit {
            TextInputEdit::Home => self.caret = self.line_start(self.caret),
            TextInputEdit::End => self.caret = self.line_end(self.caret),
            _ => {
                let mut caret = self.caret;
                edit_grapheme(&mut self.text, &mut caret, edit);
                self.caret = caret;
            }
        }
        self.last_edit = Some(EditKind::Other);
        false
    }

    fn begin_move(&mut self, extend: bool) {
        if extend {
            self.anchor.get_or_insert(self.caret);
        } else {
            self.anchor = None;
        }
    }

    fn line_start(&self, offset: usize) -> usize {
        self.text[..offset].rfind('\n').map_or(0, |index| index + 1)
    }

    fn line_end(&self, offset: usize) -> usize {
        self.text[offset..]
            .find('\n')
            .map_or(self.text.len(), |index| offset + index)
    }

    /// Moves the caret to the start or end of the whole text.
    pub fn move_to_edge(&mut self, end: bool, extend: bool) {
        self.begin_move(extend);
        self.caret = if end { self.text.len() } else { 0 };
        self.preferred_x = None;
    }

    pub fn select_all(&mut self) {
        self.anchor = Some(0);
        self.caret = self.text.len();
    }

    /// Places the caret at a byte offset, optionally extending the selection.
    pub fn place(&mut self, offset: usize, extend: bool) {
        let offset = crate::safe_text::floor_char_boundary(&self.text, offset);
        self.begin_move(extend);
        self.caret = offset;
        self.preferred_x = None;
    }

    /// Selects the word around `offset`.
    pub fn select_word(&mut self, offset: usize) {
        let offset = offset.min(self.text.len());
        let mut start = 0;
        for (index, word) in self.text.split_word_bound_indices() {
            let end = index + word.len();
            if offset < end || end == self.text.len() {
                start = index;
                self.anchor = Some(start);
                self.caret = end;
                return;
            }
        }
        self.anchor = Some(start);
        self.caret = self.text.len();
    }

    /// Moves up (`-1`) or down (`1`) a visual line in `layout`. Returns false
    /// when there is no line in that direction, so callers can use the key
    /// for something else (such as recalling an earlier message).
    pub fn move_vertically(
        &mut self,
        layout: &TextLayout,
        direction: i32,
        extend: bool,
        measure: &mut dyn FnMut(&str) -> f32,
    ) -> bool {
        let lines = line_ranges(layout, &self.text);
        let (line, x) = caret_line_x(layout, &lines, &self.text, self.caret, measure);
        let target = line as i64 + i64::from(direction);
        if target < 0 || target >= lines.len() as i64 {
            return false;
        }
        let x = *self.preferred_x.get_or_insert(x);
        let offset = offset_in_line(layout, &lines, target as usize, x, measure);
        self.begin_move(extend);
        self.caret = offset;
        self.preferred_x = Some(x);
        true
    }

    /// The caret's visual line and x offset within `layout`.
    pub fn caret_point(
        &self,
        layout: &TextLayout,
        measure: &mut dyn FnMut(&str) -> f32,
    ) -> (usize, f32) {
        let lines = line_ranges(layout, &self.text);
        caret_line_x(layout, &lines, &self.text, self.caret, measure)
    }

    /// The byte offset nearest to a point inside `layout`.
    pub fn offset_at(
        &self,
        layout: &TextLayout,
        x: f32,
        y: f32,
        measure: &mut dyn FnMut(&str) -> f32,
    ) -> usize {
        let lines = line_ranges(layout, &self.text);
        let line = layout
            .lines
            .iter()
            .position(|line| y < line.top + line.height)
            .unwrap_or(layout.lines.len().saturating_sub(1));
        offset_in_line(layout, &lines, line, x, measure)
    }

    pub fn undo(&mut self) -> bool {
        let Some(snapshot) = self.undo.pop() else {
            return false;
        };
        self.redo.push(Snapshot {
            text: std::mem::replace(&mut self.text, snapshot.text),
            caret: self.caret,
        });
        self.caret = snapshot.caret.min(self.text.len());
        self.anchor = None;
        self.last_edit = None;
        true
    }

    pub fn redo(&mut self) -> bool {
        let Some(snapshot) = self.redo.pop() else {
            return false;
        };
        self.undo.push(Snapshot {
            text: std::mem::replace(&mut self.text, snapshot.text),
            caret: self.caret,
        });
        self.caret = snapshot.caret.min(self.text.len());
        self.anchor = None;
        self.last_edit = None;
        true
    }
}

/// Line breaks become `\n`; other control characters except tabs are
/// dropped, since the editor only ever shows text.
fn normalize(text: &str) -> String {
    text.replace("\r\n", "\n")
        .replace('\r', "\n")
        .chars()
        .filter(|character| !character.is_control() || matches!(character, '\n' | '\t'))
        .collect()
}

/// The byte range each visual line covers. Lines with no glyphs (empty
/// lines) start just after the previous line's newline.
fn line_ranges(layout: &TextLayout, text: &str) -> Vec<std::ops::Range<usize>> {
    let mut ranges: Vec<std::ops::Range<usize>> = Vec::with_capacity(layout.lines.len());
    for line in &layout.lines {
        let start = line.runs.iter().map(|run| run.start).min();
        let end = line.runs.iter().map(|run| run.start + run.text.len()).max();
        let range = match (start, end) {
            (Some(start), Some(end)) => start..end,
            _ => {
                let start = match ranges.last() {
                    None => 0,
                    Some(previous) => text[previous.end..]
                        .find('\n')
                        .map_or(text.len(), |newline| previous.end + newline + 1),
                };
                start..start
            }
        };
        ranges.push(range);
    }
    ranges
}

fn caret_line_x(
    layout: &TextLayout,
    lines: &[std::ops::Range<usize>],
    text: &str,
    caret: usize,
    measure: &mut dyn FnMut(&str) -> f32,
) -> (usize, f32) {
    // A caret between two wrapped lines belongs to the later one, unless it
    // ends the text or sits before a newline.
    let line = lines
        .iter()
        .position(|range| range.start <= caret && caret < range.end)
        .or_else(|| lines.iter().rposition(|range| range.end == caret))
        .or_else(|| lines.iter().rposition(|range| range.start <= caret))
        .unwrap_or(0);
    let Some(laid) = layout.lines.get(line) else {
        return (0, 0.0);
    };
    let x = laid
        .runs
        .iter()
        .find(|run| run.start <= caret && caret <= run.start + run.text.len())
        .map_or_else(
            || laid.runs.last().map_or(0.0, |run| run.x + run.width),
            |run| {
                let prefix = &text[run.start..caret.min(run.start + run.text.len())];
                run.x + measure(prefix)
            },
        );
    (line, x)
}

fn offset_in_line(
    layout: &TextLayout,
    lines: &[std::ops::Range<usize>],
    line: usize,
    x: f32,
    measure: &mut dyn FnMut(&str) -> f32,
) -> usize {
    let Some(laid) = layout.lines.get(line) else {
        return lines.last().map_or(0, |range| range.end);
    };
    let range = lines[line].clone();
    let Some(run) = laid
        .runs
        .iter()
        .find(|run| x < run.x + run.width)
        .or(laid.runs.last())
    else {
        return range.start;
    };
    if x <= run.x {
        return run.start;
    }
    // The grapheme boundary nearest to x.
    let mut best = run.start + run.text.len();
    let mut previous_x = run.x;
    for (index, grapheme) in run.text.grapheme_indices(true) {
        let end = index + grapheme.len();
        let end_x = run.x + measure(&run.text[..end]);
        if x < end_x {
            best = if x - previous_x <= end_x - x {
                run.start + index
            } else {
                run.start + end
            };
            break;
        }
        previous_x = end_x;
    }
    // A wrapped line's trailing space leads to the next line; stop before it.
    let is_last = line + 1 == lines.len();
    if !is_last && best == range.end && best > range.start && lines[line + 1].start == best {
        return floor_boundary_before(best, &run.text, run.start);
    }
    best
}

fn floor_boundary_before(offset: usize, run_text: &str, run_start: usize) -> usize {
    run_text[..offset - run_start]
        .grapheme_indices(true)
        .next_back()
        .map_or(offset, |(index, _)| run_start + index)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::text_layout::{FixedMeasure, TextMeasure, TextSize, TextSpan, UiFace};

    fn layout(text: &str, width: f32) -> std::sync::Arc<TextLayout> {
        FixedMeasure { advance: 1.0 }.layout(
            &[TextSpan {
                text,
                face: UiFace::SANS,
            }],
            TextSize {
                font_px: 10.0,
                line_px: 10.0,
            },
            Some(width),
        )
    }

    fn width(text: &str) -> f32 {
        text.chars().count() as f32
    }

    #[test]
    fn typing_replaces_the_selection_and_undoes_as_one_step() {
        let mut area = TextArea::new();
        for character in "hello".chars() {
            area.insert(&character.to_string());
        }
        area.insert(" world");
        assert_eq!(area.text(), "hello world");
        area.edit(TextInputEdit::WordLeft, true);
        assert_eq!(area.selected_text(), Some("world"));
        area.insert("there");
        assert_eq!(area.text(), "hello there");
        assert!(area.undo());
        assert_eq!(area.text(), "hello world");
        assert!(area.undo());
        assert_eq!(area.text(), "hello");
        assert!(area.undo());
        assert_eq!(area.text(), "");
        assert!(!area.undo());
        assert!(area.redo());
        assert_eq!(area.text(), "hello");
    }

    #[test]
    fn deleting_respects_graphemes_and_selections() {
        let mut area = TextArea::new();
        area.insert("ae\u{301}b");
        area.edit(TextInputEdit::Left, false);
        assert!(area.edit(TextInputEdit::Backspace, false));
        assert_eq!(area.text(), "ab");
        area.select_all();
        assert!(area.edit(TextInputEdit::Delete, false));
        assert_eq!(area.text(), "");
        assert!(!area.edit(TextInputEdit::Backspace, false));
    }

    #[test]
    fn pasted_text_is_normalized() {
        let mut area = TextArea::new();
        area.insert("one\r\ntwo\rthree\u{1b}[31m\tend");
        assert_eq!(area.text(), "one\ntwo\nthree[31m\tend");
    }

    #[test]
    fn home_and_end_stay_on_the_current_line() {
        let mut area = TextArea::new();
        area.insert("first\nsecond");
        area.edit(TextInputEdit::Home, false);
        assert_eq!(area.caret(), 6);
        area.edit(TextInputEdit::End, true);
        assert_eq!(area.selected_text(), Some("second"));
        area.move_to_edge(false, false);
        assert_eq!(area.caret(), 0);
    }

    #[test]
    fn vertical_movement_follows_wrapped_and_empty_lines() {
        let mut area = TextArea::new();
        area.insert("abcd efgh\n\nxy");
        let laid = layout(area.text(), 5.0);
        // Lines: "abcd " | "efgh" | "" | "xy"
        area.place(2, false);
        assert!(area.move_vertically(&laid, 1, false, &mut width));
        assert_eq!(area.caret(), 7);
        assert!(area.move_vertically(&laid, 1, false, &mut width));
        assert_eq!(area.caret(), 10);
        assert!(area.move_vertically(&laid, 1, false, &mut width));
        // The preferred column survives the empty line.
        assert_eq!(area.caret(), 13);
        assert!(!area.move_vertically(&laid, 1, false, &mut width));
        assert!(area.move_vertically(&laid, -1, true, &mut width));
        assert_eq!(area.selected_text(), Some("\nxy"));
    }

    #[test]
    fn points_map_to_offsets_and_back() {
        let mut area = TextArea::new();
        area.insert("abcd efgh");
        let laid = layout(area.text(), 5.0);
        assert_eq!(area.offset_at(&laid, 1.4, 12.0, &mut width), 6);
        assert_eq!(area.offset_at(&laid, 99.0, 2.0, &mut width), 4);
        area.place(6, false);
        assert_eq!(area.caret_point(&laid, &mut width), (1, 1.0));
        area.place(9, false);
        assert_eq!(area.caret_point(&laid, &mut width), (1, 4.0));
        area.place(5, false);
        assert_eq!(area.caret_point(&laid, &mut width), (1, 0.0));
    }

    #[test]
    fn words_are_selected_whole() {
        let mut area = TextArea::new();
        area.insert("run cargo test");
        area.select_word(6);
        assert_eq!(area.selected_text(), Some("cargo"));
    }
}
