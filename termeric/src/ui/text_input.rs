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
        let previous = || {
            self.buffer
                .grapheme_indices(true)
                .map(|(offset, _)| offset)
                .take_while(|offset| *offset < self.cursor)
                .last()
                .unwrap_or(0)
        };
        let next = || {
            self.buffer
                .grapheme_indices(true)
                .map(|(offset, _)| offset)
                .find(|offset| *offset > self.cursor)
                .unwrap_or(self.buffer.len())
        };
        match edit {
            TextInputEdit::Left => self.cursor = previous(),
            TextInputEdit::Right => self.cursor = next(),
            TextInputEdit::Home => self.cursor = 0,
            TextInputEdit::End => self.cursor = self.buffer.len(),
            TextInputEdit::Backspace | TextInputEdit::Delete => {
                if matches!(edit, TextInputEdit::Backspace) && self.cursor == 0
                    || matches!(edit, TextInputEdit::Delete) && self.cursor == self.buffer.len()
                {
                    return false;
                }
                let at_boundary = self.cursor == self.buffer.len()
                    || self
                        .buffer
                        .grapheme_indices(true)
                        .any(|(offset, _)| offset == self.cursor);
                // An insertion can join a following combining mark. If the
                // caret is now inside that cluster, delete the whole cluster.
                let start = if matches!(edit, TextInputEdit::Backspace) || !at_boundary {
                    previous()
                } else {
                    self.cursor
                };
                let end = if matches!(edit, TextInputEdit::Backspace) && at_boundary {
                    self.cursor
                } else {
                    next()
                };
                self.buffer.drain(start..end);
                self.cursor = start;
                return true;
            }
        }
        false
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
