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
        // Keep the adjacent slices and remaining iterator so a large cluster
        // is not segmented again when computing the visible prefix/suffix.
        let mut following = text.grapheme_indices(true);
        let mut previous = None;
        let next = loop {
            match following.next() {
                Some((offset, grapheme)) if offset + grapheme.len() <= cursor => {
                    previous = Some((offset, grapheme));
                }
                next => break next,
            }
        };
        let caret = next.map_or(text.len(), |(offset, _)| offset);
        let mut start_byte = caret;
        let mut cursor_columns = 0;
        let prefix_end = previous.map_or(0, |(offset, _)| offset);
        for (offset, grapheme) in previous
            .into_iter()
            .chain(text[..prefix_end].grapheme_indices(true).rev())
        {
            let width = termeric_core::unicode::grapheme_width(grapheme);
            if cursor_columns + width > columns - 1 {
                break;
            }
            cursor_columns += width;
            start_byte = offset;
        }
        // The prefix already fits; reuse its width instead of rescanning it.
        let mut end_byte = caret;
        let mut used = cursor_columns;
        for (offset, grapheme) in next.into_iter().chain(following) {
            let width = termeric_core::unicode::grapheme_width(grapheme);
            if used + width > columns {
                break;
            }
            used += width;
            end_byte = offset + grapheme.len();
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
