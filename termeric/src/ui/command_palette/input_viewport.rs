//! Shared query scroll/caret geometry for painting and native IME positioning.

use super::PaletteLayout;
use std::ops::Range;
use unicode_segmentation::UnicodeSegmentation;

#[derive(Debug, Clone, PartialEq)]
pub struct PaletteInputViewport {
    pub text_range: Range<usize>,
    pub text_x: f32,
    pub text_y: f32,
    pub cursor_x: f32,
    pub right: f32,
    pub columns: usize,
}

impl PaletteLayout {
    /// Keep the caret visible without splitting a grapheme or drawing outside
    /// the field. The full query remains in the model and accessibility tree.
    pub fn input_viewport(&self, text: &str, cursor: usize) -> Option<PaletteInputViewport> {
        if !text.is_char_boundary(cursor) || !self.cell_width.is_finite() || self.cell_width <= 0.0
        {
            return None;
        }
        let text_x = self.panel_x + self.padding + self.cell_width * 2.2;
        let right = self.panel_x + self.panel_width - self.padding - self.cell_width * 0.8;
        let columns = ((right - text_x) / self.cell_width).floor().max(0.0) as usize;
        if columns == 0 {
            return None;
        }
        let graphemes: Vec<_> = text.grapheme_indices(true).collect();
        // A byte cursor inside a combining/ZWJ cluster paints at its start.
        let caret =
            graphemes.partition_point(|(offset, grapheme)| offset + grapheme.len() <= cursor);
        let mut start = caret;
        let mut cursor_columns = 0;
        while start > 0 {
            let width = termeric_core::unicode::grapheme_width(graphemes[start - 1].1);
            if cursor_columns + width > columns - 1 {
                break;
            }
            cursor_columns += width;
            start -= 1;
        }
        let start_byte = graphemes
            .get(start)
            .map_or(text.len(), |(offset, _)| *offset);
        let mut end_byte = start_byte;
        let mut used = 0;
        for (offset, grapheme) in &graphemes[start..] {
            let width = termeric_core::unicode::grapheme_width(grapheme);
            if used + width > columns {
                break;
            }
            used += width;
            end_byte = offset + grapheme.len();
        }
        Some(PaletteInputViewport {
            text_range: start_byte..end_byte,
            text_x,
            text_y: self.panel_y + self.padding + (self.input_height - self.cell_height) / 2.0,
            cursor_x: text_x + cursor_columns as f32 * self.cell_width,
            right,
            columns,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn query_viewport_keeps_every_caret_and_complete_grapheme_inside_the_field() {
        for window_width in [160.0, 450.0, 800.0, 1920.0] {
            for cell_width in [6.0, 9.5, 24.0] {
                for animation in [0.0, 0.5, 1.0] {
                    let layout = PaletteLayout::calculate(
                        window_width,
                        800.0,
                        cell_width,
                        25.0,
                        animation,
                        9,
                    );
                    for text in ["", "short", &"ab界e\u{301}👩\u{200d}💻".repeat(32)] {
                        for cursor in text
                            .char_indices()
                            .map(|(byte, _)| byte)
                            .chain([text.len()])
                        {
                            let Some(view) = layout.input_viewport(text, cursor) else {
                                assert!(
                                    layout.panel_width < layout.padding * 2.0 + cell_width * 4.0
                                );
                                continue;
                            };
                            assert!(view.cursor_x >= view.text_x);
                            assert!(view.cursor_x + cell_width <= view.right + 0.001);
                            let visible = &text[view.text_range.clone()];
                            let width: usize = visible
                                .graphemes(true)
                                .map(termeric_core::unicode::grapheme_width)
                                .sum();
                            assert!(width <= view.columns);
                            for edge in [view.text_range.start, view.text_range.end] {
                                assert!(
                                    edge == text.len()
                                        || text
                                            .grapheme_indices(true)
                                            .any(|(offset, _)| offset == edge)
                                );
                            }
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn long_query_scrolls_without_losing_its_utf8_tail_and_invalid_cursors_are_refused() {
        let layout = PaletteLayout::calculate(800.0, 600.0, 10.0, 20.0, 1.0, 0);
        let query = "界".repeat(256);
        let view = layout.input_viewport(&query, query.len()).unwrap();
        assert!(view.text_range.start > 0);
        assert_eq!(view.text_range.end, query.len());
        assert!(layout.input_viewport(&query, 1).is_none());
        assert!(layout.input_viewport(&query, query.len() + 1).is_none());
        assert_eq!(
            layout.input_viewport(&query, 0).unwrap().cursor_x,
            view.text_x
        );
    }
}
