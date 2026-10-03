//! Shared single-line text input buffer with grapheme-aware editing.

use unicode_segmentation::UnicodeSegmentation;

/// A whole-grapheme edit in a single-line field.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TextInputEdit {
    Left,
    Right,
    Home,
    End,
    Backspace,
    Delete,
    WordLeft,
    WordRight,
    BackspaceWord,
    DeleteWord,
}

/// How much one field holds: characters for what people read, bytes for
/// what is sent on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TextLimit {
    Chars(usize),
    Bytes(usize),
}

impl std::fmt::Display for TextLimit {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Chars(limit) => write!(formatter, "{limit} characters"),
            Self::Bytes(limit) => write!(formatter, "{limit} UTF-8 bytes"),
        }
    }
}

impl TextLimit {
    /// Whether `held` and `added` together stay within the limit.
    fn admits(self, held: &str, added: &str) -> bool {
        match self {
            Self::Bytes(limit) => held.len().saturating_add(added.len()) <= limit,
            // A character is at most four bytes, so longer text cannot fit.
            Self::Chars(limit) => {
                added.len() <= limit.saturating_mul(4)
                    && held.chars().count() + added.chars().count() <= limit
            }
        }
    }
}

/// A single-line text field: its text, caret, and limit. Every change is
/// whole or nothing; text with a line break or control character, or that
/// would overflow the limit, is refused rather than cut or filtered.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TextInput {
    buffer: String,
    /// Byte offset into buffer
    cursor: usize,
    limit: TextLimit,
}

impl TextInput {
    pub fn new(limit: TextLimit) -> Self {
        Self {
            buffer: String::new(),
            cursor: 0,
            limit,
        }
    }

    pub fn text(&self) -> &str {
        &self.buffer
    }

    /// The caret, as a byte offset.
    pub fn cursor(&self) -> usize {
        self.cursor
    }

    pub fn limit(&self) -> TextLimit {
        self.limit
    }

    /// Whether `text` could be this field's whole value.
    pub fn accepts(&self, text: &str) -> bool {
        !text.chars().any(crate::safe_text::breaks_single_line) && self.limit.admits("", text)
    }

    /// Place the caret on a grapheme boundary; anywhere else changes nothing.
    pub fn place_caret(&mut self, cursor: usize) -> bool {
        if cursor != self.buffer.len()
            && !self
                .buffer
                .grapheme_indices(true)
                .any(|(offset, _)| offset == cursor)
        {
            return false;
        }
        self.cursor = cursor;
        true
    }

    pub fn clear(&mut self) {
        self.buffer.clear();
        self.cursor = 0;
    }

    /// Replace the whole value, caret at its end. Returns whether it was
    /// accepted.
    pub fn replace(&mut self, text: &str) -> bool {
        if !self.accepts(text) {
            return false;
        }
        self.buffer.clear();
        self.buffer.push_str(text);
        self.cursor = self.buffer.len();
        true
    }

    /// Whether [`insert`](Self::insert) would accept `text`.
    pub fn can_insert(&self, text: &str) -> bool {
        !text.is_empty()
            && !text.chars().any(crate::safe_text::breaks_single_line)
            && self.limit.admits(&self.buffer, text)
    }

    /// Insert typed, committed, or pasted text at the caret with one splice.
    /// Returns whether it was accepted; empty text is not a change.
    pub fn insert(&mut self, text: &str) -> bool {
        if !self.can_insert(text) {
            return false;
        }
        self.buffer.insert_str(self.cursor, text);
        self.cursor += text.len();
        true
    }

    /// Apply a whole-grapheme edit. Returns true only if text changed; moving
    /// the caret must not invalidate search results.
    pub fn edit_grapheme(&mut self, edit: TextInputEdit) -> bool {
        edit_grapheme(&mut self.buffer, &mut self.cursor, edit)
    }

    pub fn cursor_end(&mut self) {
        self.cursor = self.buffer.len();
    }
}

/// Edit an existing buffer without copying it. Invalid UTF-8 byte cursors are
/// rejected unchanged. The result reports text changes, not caret movement.
pub(crate) fn edit_grapheme(buffer: &mut String, cursor: &mut usize, edit: TextInputEdit) -> bool {
    if !buffer.is_char_boundary(*cursor) {
        return false;
    }
    let previous = || {
        buffer
            .grapheme_indices(true)
            .map(|(offset, _)| offset)
            .take_while(|offset| *offset < *cursor)
            .last()
            .unwrap_or(0)
    };
    let next = || {
        buffer
            .grapheme_indices(true)
            .map(|(offset, _)| offset)
            .find(|offset| *offset > *cursor)
            .unwrap_or(buffer.len())
    };
    match edit {
        TextInputEdit::Left => *cursor = previous(),
        TextInputEdit::Right => *cursor = next(),
        TextInputEdit::Home => *cursor = 0,
        TextInputEdit::End => *cursor = buffer.len(),
        TextInputEdit::WordLeft
        | TextInputEdit::WordRight
        | TextInputEdit::BackspaceWord
        | TextInputEdit::DeleteWord => {
            let backwards = matches!(edit, TextInputEdit::WordLeft | TextInputEdit::BackspaceWord);
            let target = word_boundary(buffer, *cursor, backwards);
            if matches!(edit, TextInputEdit::WordLeft | TextInputEdit::WordRight) {
                *cursor = target;
                return false;
            }
            // Expand either deletion edge to an extended-grapheme boundary,
            // including carets inside clusters joined by a recent insertion.
            let (start, end) = if backwards {
                (target, grapheme_boundary(buffer, *cursor, false))
            } else {
                (grapheme_boundary(buffer, *cursor, true), target)
            };
            if start == end {
                return false;
            }
            buffer.drain(start..end);
            *cursor = start;
            return true;
        }
        TextInputEdit::Backspace | TextInputEdit::Delete => {
            if matches!(edit, TextInputEdit::Backspace) && *cursor == 0
                || matches!(edit, TextInputEdit::Delete) && *cursor == buffer.len()
            {
                return false;
            }
            let at_boundary = *cursor == buffer.len()
                || buffer
                    .grapheme_indices(true)
                    .any(|(offset, _)| offset == *cursor);
            // An insertion can join a following combining mark. If the
            // caret is now inside that cluster, delete the whole cluster.
            let start = if matches!(edit, TextInputEdit::Backspace) || !at_boundary {
                previous()
            } else {
                *cursor
            };
            let end = if matches!(edit, TextInputEdit::Backspace) && at_boundary {
                *cursor
            } else {
                next()
            };
            buffer.drain(start..end);
            *cursor = start;
            return true;
        }
    }
    false
}

/// Unicode word-boundary segments (including punctuation/emoji), with
/// neighboring whitespace skipped. Work is linear in the bounded input;
/// there is no secondary string or boundary-index allocation.
fn word_boundary(text: &str, cursor: usize, backwards: bool) -> usize {
    let mut segments = text
        .split_word_bound_indices()
        .filter(|(_, segment)| !segment.chars().all(char::is_whitespace));
    let boundary = if backwards {
        segments
            .map(|(start, _)| start)
            .take_while(|start| *start < cursor)
            .last()
            .unwrap_or(0)
    } else {
        segments
            .find(|(start, segment)| start + segment.len() > cursor)
            .map(|(start, segment)| start + segment.len())
            .unwrap_or(text.len())
    };
    grapheme_boundary(text, boundary, backwards)
}

/// The nearest extended-grapheme boundary at or before (`backwards`) or at or
/// after `cursor`; the end of `text` counts as a boundary.
pub(crate) fn grapheme_boundary(text: &str, cursor: usize, backwards: bool) -> usize {
    let mut boundaries = text
        .grapheme_indices(true)
        .map(|(offset, _)| offset)
        .chain([text.len()]);
    if backwards {
        boundaries
            .take_while(|offset| *offset <= cursor)
            .last()
            .unwrap_or(0)
    } else {
        boundaries
            .find(|offset| *offset >= cursor)
            .unwrap_or(text.len())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input(text: &str) -> TextInput {
        let mut input = TextInput::new(TextLimit::Bytes(64));
        assert!(input.replace(text));
        input
    }

    #[test]
    fn text_splice_preserves_utf8_suffix_and_advances_by_bytes() {
        let mut input = input("界z");
        assert!(input.place_caret("界".len()));
        assert!(input.insert("e\u{301}👩\u{200d}💻"));
        assert_eq!(input.text(), "界e\u{301}👩\u{200d}💻z");
        assert_eq!(input.cursor(), input.text().len() - 1);
        assert!(!input.insert(""));
        assert_eq!(input.cursor(), input.text().len() - 1);
        assert!(input.edit_grapheme(TextInputEdit::Backspace));
        assert_eq!(input.text(), "界e\u{301}z");
    }

    #[test]
    fn changes_are_whole_or_nothing() {
        let mut chars = TextInput::new(TextLimit::Chars(3));
        assert!(chars.insert("界界"));
        assert!(!chars.insert("ab"), "would hold four characters");
        assert!(chars.insert("a"));
        assert_eq!(chars.text(), "界界a");

        let mut bytes = TextInput::new(TextLimit::Bytes(4));
        assert!(!bytes.insert("界界"), "six bytes");
        assert!(bytes.insert("界"));
        assert!(!bytes.insert("界"));
        assert!(bytes.insert("a"));

        for line_break in ["a\nb", "a\rb", "\t", "\u{1b}", "a\u{2028}"] {
            assert!(!chars.accepts(line_break), "{line_break:?}");
            assert!(!input("").insert(line_break), "{line_break:?}");
        }
        assert!(!bytes.replace("toolong"));
        assert_eq!(bytes.text(), "界a");
        assert!(bytes.replace(""));
        assert_eq!((bytes.text(), bytes.cursor()), ("", 0));
    }

    #[test]
    fn carets_land_only_on_grapheme_boundaries() {
        let mut input = input("e\u{301}x");
        assert!(!input.place_caret(1), "inside e + combining acute");
        assert!(input.place_caret(3));
        assert!(input.place_caret(4));
        assert!(!input.place_caret(5));
        assert_eq!(input.cursor(), 4);
    }

    #[test]
    fn word_edits_skip_whitespace_and_keep_punctuation_and_emoji_segments() {
        use TextInputEdit::*;
        let value = "one  two,👩\u{200d}💻";
        let mut input = input(value);
        assert!(input.place_caret(0));
        for cursor in [3, 8, 9, value.len(), value.len()] {
            assert!(!input.edit_grapheme(WordRight));
            assert_eq!(input.cursor(), cursor);
            assert_eq!(input.text(), value);
        }
        for cursor in [9, 8, 5, 0, 0] {
            assert!(!input.edit_grapheme(WordLeft));
            assert_eq!(input.cursor(), cursor);
        }
        assert!(!input.edit_grapheme(BackspaceWord));
        assert!(input.edit_grapheme(DeleteWord));
        assert_eq!(input.text(), "  two,👩\u{200d}💻");
        assert!(input.edit_grapheme(DeleteWord));
        assert_eq!(input.text(), ",👩\u{200d}💻");
        input.cursor_end();
        assert!(!input.edit_grapheme(DeleteWord));
        assert!(input.edit_grapheme(BackspaceWord));
        assert_eq!(input.text(), ",");
        assert!(input.edit_grapheme(BackspaceWord));
        assert_eq!(input.text(), "");
        assert!(!input.edit_grapheme(DeleteWord));
    }

    #[test]
    fn word_deletion_expands_joined_graphemes_and_rejects_invalid_byte_cursors() {
        use TextInputEdit::*;
        for cluster in ["e\u{301}", "👩\u{200d}💻", "🇺🇸"] {
            for edit in [BackspaceWord, DeleteWord] {
                let mut text = format!("{cluster} tail");
                let mut cursor = cluster.chars().next().unwrap().len_utf8();
                assert!(edit_grapheme(&mut text, &mut cursor, edit));
                assert_eq!(text, " tail");
                assert_eq!(cursor, 0);
            }
        }
        for edit in [WordLeft, WordRight, BackspaceWord, DeleteWord] {
            for invalid in [1, 2, 99, usize::MAX] {
                let mut text = "界".to_owned();
                let mut cursor = invalid;
                assert!(!edit_grapheme(&mut text, &mut cursor, edit));
                assert_eq!((text.as_str(), cursor), ("界", invalid));
            }
        }
        for (edit, expected, position) in
            [(BackspaceWord, "pha beta", 0), (DeleteWord, "al beta", 2)]
        {
            let mut text = "alpha beta".to_owned();
            let mut cursor = 2;
            assert!(edit_grapheme(&mut text, &mut cursor, edit));
            assert_eq!((text.as_str(), cursor), (expected, position));
        }
    }

    #[test]
    fn grapheme_edits_traverse_and_remove_combining_emoji_and_flags_whole() {
        let value = "ae\u{301}👩\u{200d}💻🇺🇸z";
        let boundaries: Vec<_> = value
            .grapheme_indices(true)
            .map(|(offset, _)| offset)
            .chain([value.len()])
            .collect();
        let mut input = input(value);
        assert!(!input.edit_grapheme(TextInputEdit::Home));
        for boundary in boundaries.iter().skip(1) {
            assert!(!input.edit_grapheme(TextInputEdit::Right));
            assert_eq!(input.cursor(), *boundary);
        }
        for boundary in boundaries.iter().rev().skip(1) {
            assert!(!input.edit_grapheme(TextInputEdit::Left));
            assert_eq!(input.cursor(), *boundary);
        }
        assert!(!input.edit_grapheme(TextInputEdit::Backspace));
        for boundary in boundaries.iter().skip(1) {
            assert!(input.edit_grapheme(TextInputEdit::Delete));
            assert_eq!(input.text(), &value[*boundary..]);
        }
        assert!(!input.edit_grapheme(TextInputEdit::Delete));
        input.replace(value);
        for boundary in boundaries.iter().rev().skip(1) {
            assert!(input.edit_grapheme(TextInputEdit::Backspace));
            assert_eq!(input.text(), &value[..*boundary]);
        }
    }

    #[test]
    fn edits_repair_a_caret_inside_a_newly_joined_grapheme() {
        for edit in [TextInputEdit::Backspace, TextInputEdit::Delete] {
            let mut input = input("\u{301}x");
            assert!(input.place_caret(0));
            assert!(input.insert("e"));
            assert_eq!(input.cursor(), 1);
            assert!(input.edit_grapheme(edit));
            assert_eq!(input.text(), "x");
            assert_eq!(input.cursor(), 0);
        }
    }
}
