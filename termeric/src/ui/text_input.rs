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

/// A single-line text input buffer with cursor tracking.
pub struct TextInput {
    buffer: String,
    /// Byte offset into buffer
    cursor: usize,
}

impl TextInput {
    /// Create an empty text input.
    pub fn new() -> Self {
        Self {
            buffer: String::new(),
            cursor: 0,
        }
    }

    /// Get the current text.
    pub fn text(&self) -> &str {
        &self.buffer
    }

    /// Get the cursor position (byte offset).
    pub fn cursor(&self) -> usize {
        self.cursor
    }

    /// Place the cursor at an exact UTF-8 byte boundary.
    pub fn set_cursor_byte(&mut self, byte_offset: usize) -> bool {
        if byte_offset > self.buffer.len() || !self.buffer.is_char_boundary(byte_offset) {
            return false;
        }
        self.cursor = byte_offset;
        true
    }

    /// Clear the buffer and reset cursor.
    pub fn clear(&mut self) {
        self.buffer.clear();
        self.cursor = 0;
    }

    /// Replace the complete buffer and place the cursor at its end.
    pub fn replace(&mut self, value: impl Into<String>) {
        self.buffer = value.into();
        self.cursor = self.buffer.len();
    }

    /// Insert a character at the cursor position and advance cursor.
    pub fn insert_char(&mut self, c: char) {
        self.buffer.insert(self.cursor, c);
        self.cursor += c.len_utf8();
    }

    /// Insert already-validated text with one splice, not one suffix move per
    /// scalar. Callers own their field-specific size and content policy.
    pub fn insert_text(&mut self, text: &str) {
        self.buffer.insert_str(self.cursor, text);
        self.cursor += text.len();
    }

    /// Apply a whole-grapheme edit. Returns true only if text changed; moving
    /// the caret must not invalidate search results.
    pub fn edit_grapheme(&mut self, edit: TextInputEdit) -> bool {
        edit_grapheme(&mut self.buffer, &mut self.cursor, edit)
    }

    /// Move cursor to the end.
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
impl TextInput {
    fn from(s: impl Into<String>) -> Self {
        let buffer: String = s.into();
        let cursor = buffer.len();
        Self { buffer, cursor }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_splice_preserves_utf8_suffix_and_advances_by_bytes() {
        let mut input = TextInput::from("界z");
        assert!(input.set_cursor_byte("界".len()));
        input.insert_text("e\u{301}👩\u{200d}💻");
        assert_eq!(input.text(), "界e\u{301}👩\u{200d}💻z");
        assert_eq!(input.cursor(), input.text().len() - 1);
        input.insert_text("");
        assert_eq!(input.cursor(), input.text().len() - 1);
        assert!(input.edit_grapheme(TextInputEdit::Backspace));
        assert_eq!(input.text(), "界e\u{301}z");
    }

    #[test]
    fn word_edits_skip_whitespace_and_keep_punctuation_and_emoji_segments() {
        use TextInputEdit::*;
        let value = "one  two,👩\u{200d}💻";
        let mut input = TextInput::from(value);
        assert!(input.set_cursor_byte(0));
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
        let mut input = TextInput::from(value);
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
            let mut input = TextInput::from("\u{301}x");
            assert!(input.set_cursor_byte(0));
            input.insert_char('e');
            assert_eq!(input.cursor(), 1);
            assert!(input.edit_grapheme(edit));
            assert_eq!(input.text(), "x");
            assert_eq!(input.cursor(), 0);
        }
    }

    #[test]
    fn test_empty() {
        let input = TextInput::new();
        assert_eq!(input.cursor(), 0);
        assert_eq!(input.text(), "");
    }

    #[test]
    fn test_insert_char() {
        let mut input = TextInput::new();
        input.insert_char('h');
        input.insert_char('i');
        assert_eq!(input.text(), "hi");
        assert_eq!(input.cursor(), 2);
    }

    #[test]
    fn test_insert_at_middle() {
        let mut input = TextInput::from("hllo");
        assert!(input.set_cursor_byte(1));
        input.insert_char('e');
        assert_eq!(input.text(), "hello");
        assert_eq!(input.cursor(), 2);
    }

    #[test]
    fn test_clear() {
        let mut input = TextInput::from("hello");
        input.clear();
        assert_eq!(input.text(), "");
        assert_eq!(input.cursor(), 0);
    }

    #[test]
    fn test_replace_moves_cursor_to_utf8_end() {
        let mut input = TextInput::from("old");
        input.replace("new 🫚");
        assert_eq!(input.text(), "new 🫚");
        assert_eq!(input.cursor(), "new 🫚".len());
    }
}
