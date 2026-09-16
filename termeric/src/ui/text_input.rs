//! Shared single-line text input buffer with cursor tracking
//!
//! Encapsulates the insert-at-cursor and UTF-8-aware backspace logic
//! previously duplicated across CommandPalette, CommandMode, CommandsPanel,
//! and Search.

use unicode_segmentation::UnicodeSegmentation;

/// Whole-grapheme editing for single-line fields that opt into it.
#[derive(Debug, Clone, Copy)]
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

#[allow(dead_code)]
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

    /// Apply a whole-grapheme edit. Returns true only if text changed; moving
    /// the caret must not invalidate search results. Existing scalar-based
    /// methods remain unchanged for other consumers.
    pub fn edit_grapheme(&mut self, edit: TextInputEdit) -> bool {
        edit_grapheme(&mut self.buffer, &mut self.cursor, edit)
    }

    /// Delete the character before the cursor (backspace).
    /// Returns true if a character was deleted.
    pub fn backspace(&mut self) -> bool {
        if self.cursor == 0 {
            return false;
        }
        let prev = self.buffer[..self.cursor]
            .char_indices()
            .last()
            .map(|(i, _)| i)
            .unwrap_or(0);
        self.buffer.remove(prev);
        self.cursor = prev;
        true
    }

    /// Move cursor to the beginning.
    pub fn cursor_home(&mut self) {
        self.cursor = 0;
    }

    /// Move cursor to the end.
    pub fn cursor_end(&mut self) {
        self.cursor = self.buffer.len();
    }

    /// Move cursor one character to the left.
    /// Returns false if already at the beginning.
    pub fn cursor_left(&mut self) -> bool {
        if self.cursor == 0 {
            return false;
        }
        let prev = self.buffer[..self.cursor]
            .char_indices()
            .last()
            .map(|(i, _)| i)
            .unwrap_or(0);
        self.cursor = prev;
        true
    }

    /// Move cursor one character to the right.
    /// Returns false if already at the end.
    pub fn cursor_right(&mut self) -> bool {
        if self.cursor >= self.buffer.len() {
            return false;
        }
        let next = self.buffer[self.cursor..]
            .char_indices()
            .nth(1)
            .map(|(i, _)| self.cursor + i)
            .unwrap_or(self.buffer.len());
        self.cursor = next;
        true
    }

    /// Delete the character at the cursor position (forward delete).
    /// Returns true if a character was deleted.
    pub fn delete(&mut self) -> bool {
        if self.cursor >= self.buffer.len() {
            return false;
        }
        let next = self.buffer[self.cursor..]
            .char_indices()
            .nth(1)
            .map(|(i, _)| self.cursor + i)
            .unwrap_or(self.buffer.len());
        self.buffer.drain(self.cursor..next);
        true
    }

    /// Push a character to the end of the buffer.
    pub fn push(&mut self, c: char) {
        self.buffer.push(c);
    }

    /// Pop the last character from the buffer.
    pub fn pop(&mut self) -> Option<char> {
        let c = self.buffer.pop();
        if self.cursor > self.buffer.len() {
            self.cursor = self.buffer.len();
        }
        c
    }

    /// Take the buffer contents, leaving the input empty.
    pub fn take(&mut self) -> String {
        let s = std::mem::take(&mut self.buffer);
        self.cursor = 0;
        s
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

fn grapheme_boundary(text: &str, cursor: usize, backwards: bool) -> usize {
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

    fn set_cursor(&mut self, pos: usize) {
        self.cursor = pos.min(self.buffer.len());
    }

    fn is_empty(&self) -> bool {
        self.buffer.is_empty()
    }

    fn len(&self) -> usize {
        self.buffer.len()
    }
}

impl std::fmt::Display for TextInput {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.buffer)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn word_edits_skip_whitespace_and_keep_punctuation_and_emoji_segments() {
        use TextInputEdit::*;
        let value = "one  two,👩\u{200d}💻";
        let mut input = TextInput::from(value);
        input.cursor_home();
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
            input.cursor_home();
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
        assert!(input.is_empty());
        assert_eq!(input.len(), 0);
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
        input.set_cursor(1);
        input.insert_char('e');
        assert_eq!(input.text(), "hello");
        assert_eq!(input.cursor(), 2);
    }

    #[test]
    fn test_backspace() {
        let mut input = TextInput::from("hello");
        assert!(input.backspace());
        assert_eq!(input.text(), "hell");
        assert_eq!(input.cursor(), 4);
    }

    #[test]
    fn test_backspace_at_start() {
        let mut input = TextInput::from("hello");
        input.set_cursor(0);
        assert!(!input.backspace());
        assert_eq!(input.text(), "hello");
    }

    #[test]
    fn test_delete() {
        let mut input = TextInput::from("hello");
        input.set_cursor(0);
        assert!(input.delete());
        assert_eq!(input.text(), "ello");
        assert_eq!(input.cursor(), 0);
    }

    #[test]
    fn test_delete_at_end() {
        let mut input = TextInput::from("hello");
        assert!(!input.delete());
        assert_eq!(input.text(), "hello");
    }

    #[test]
    fn test_cursor_movement() {
        let mut input = TextInput::from("hello");
        input.cursor_home();
        assert_eq!(input.cursor(), 0);
        assert!(input.cursor_right());
        assert_eq!(input.cursor(), 1);
        input.cursor_end();
        assert_eq!(input.cursor(), 5);
        assert!(!input.cursor_right());
        input.cursor_home();
        assert!(!input.cursor_left());
    }

    #[test]
    fn test_utf8_handling() {
        let mut input = TextInput::new();
        input.insert_char('a');
        input.insert_char('é'); // 2-byte UTF-8
        input.insert_char('🎉'); // 4-byte UTF-8
        assert_eq!(input.text(), "aé🎉");
        assert_eq!(input.cursor(), 7); // 1 + 2 + 4

        assert!(input.backspace());
        assert_eq!(input.text(), "aé");
        assert_eq!(input.cursor(), 3);

        assert!(input.backspace());
        assert_eq!(input.text(), "a");
        assert_eq!(input.cursor(), 1);
    }

    #[test]
    fn test_clear() {
        let mut input = TextInput::from("hello");
        input.clear();
        assert!(input.is_empty());
        assert_eq!(input.cursor(), 0);
    }

    #[test]
    fn test_replace_moves_cursor_to_utf8_end() {
        let mut input = TextInput::from("old");
        input.replace("new 🫚");
        assert_eq!(input.text(), "new 🫚");
        assert_eq!(input.cursor(), "new 🫚".len());
    }

    #[test]
    fn test_take() {
        let mut input = TextInput::from("hello");
        let taken = input.take();
        assert_eq!(taken, "hello");
        assert!(input.is_empty());
        assert_eq!(input.cursor(), 0);
    }

    #[test]
    fn test_push_pop() {
        let mut input = TextInput::new();
        input.push('a');
        input.push('b');
        assert_eq!(input.text(), "ab");
        assert_eq!(input.pop(), Some('b'));
        assert_eq!(input.text(), "a");
    }
}
