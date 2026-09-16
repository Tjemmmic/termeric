//! Whole-grapheme horizontal scrolling shared by single-line query fields.

use std::ops::Range;
use unicode_segmentation::UnicodeSegmentation;

#[derive(Debug, Clone, PartialEq)]
pub struct TextInputViewport {
    pub text_range: Range<usize>,
    pub text_x: f32,
    pub text_y: f32,
    pub cursor_x: f32,
    pub right: f32,
    pub columns: usize,
}

impl TextInputViewport {
    /// Nearest visible whole-grapheme boundary; callers own vertical hit testing.
    pub fn cursor_at(&self, text: &str, x: f32, cell_width: f32) -> Option<usize> {
        if !x.is_finite() || !cell_width.is_finite() || cell_width <= 0.0 {
            return None;
        }
        let visible = text.get(self.text_range.clone())?;
        let mut left = self.text_x;
        for (offset, grapheme) in visible.grapheme_indices(true) {
            let width = termeric_core::unicode::grapheme_width(grapheme) as f32 * cell_width;
            if x < left + width / 2.0 {
                return Some(self.text_range.start + offset);
            }
            left += width;
        }
        Some(self.text_range.end)
    }

    pub fn calculate(
        text: &str,
        cursor: usize,
        text_x: f32,
        text_y: f32,
        right: f32,
        cell_width: f32,
    ) -> Option<Self> {
        if !text.is_char_boundary(cursor) || !cell_width.is_finite() || cell_width <= 0.0 {
            return None;
        }
        let columns = ((right - text_x) / cell_width).floor().max(0.0) as usize;
        if columns == 0 {
            return None;
        }
        // A byte cursor inside a combining/ZWJ cluster paints at its start.
        // Locate that boundary without retaining an index for the whole query.
        let caret = text
            .grapheme_indices(true)
            .find(|(offset, grapheme)| offset + grapheme.len() > cursor)
            .map_or(text.len(), |(offset, _)| offset);
        let mut start_byte = caret;
        let mut cursor_columns = 0;
        for (offset, grapheme) in text[..caret].grapheme_indices(true).rev() {
            let width = termeric_core::unicode::grapheme_width(grapheme);
            if cursor_columns + width > columns - 1 {
                break;
            }
            cursor_columns += width;
            start_byte = offset;
        }
        let mut end_byte = start_byte;
        let mut used = 0;
        for (offset, grapheme) in text[start_byte..].grapheme_indices(true) {
            let width = termeric_core::unicode::grapheme_width(grapheme);
            if used + width > columns {
                break;
            }
            used += width;
            end_byte = start_byte + offset + grapheme.len();
        }
        Some(Self {
            text_range: start_byte..end_byte,
            text_x,
            text_y,
            cursor_x: text_x + cursor_columns as f32 * cell_width,
            right,
            columns,
        })
    }
}
