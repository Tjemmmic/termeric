//! Shared query scroll/caret geometry for painting and native IME positioning.

use super::{PaletteInputViewport, PaletteLayout};
#[cfg(test)]
use unicode_segmentation::UnicodeSegmentation;

impl PaletteLayout {
    /// Matches the rounded input background's bounding rectangle in the renderer.
    pub fn input_contains(&self, x: f32, y: f32) -> bool {
        x.is_finite()
            && y.is_finite()
            && x >= self.panel_x + self.padding
            && x < self.panel_x + self.panel_width - self.padding
            && y >= self.panel_y + self.padding
            && y < self.panel_y + self.padding + self.input_height
    }

    /// Resolve the nearest visible grapheme boundary, including scrolled text.
    /// Blank space selects the visible edge, never an off-screen byte position.
    pub fn input_cursor_at(&self, text: &str, cursor: usize, x: f32, y: f32) -> Option<usize> {
        if !self.input_contains(x, y) {
            return None;
        }
        self.input_viewport(text, cursor)?
            .cursor_at(text, x, self.cell_width)
    }

    /// Keep the caret visible without splitting a grapheme or drawing outside
    /// the field. The full query remains in the model and accessibility tree.
    pub fn input_viewport(&self, text: &str, cursor: usize) -> Option<PaletteInputViewport> {
        let text_x = self.panel_x + self.padding + self.cell_width * 2.2;
        let right = self.panel_x + self.panel_width - self.padding - self.cell_width * 0.8;
        PaletteInputViewport::calculate(
            text,
            cursor,
            text_x,
            self.panel_y + self.padding + (self.input_height - self.cell_height) / 2.0,
            right,
            self.cell_width,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pointer_caret_uses_visible_grapheme_midpoints_at_every_scale_and_animation() {
        for cell_width in [6.0, 9.5, 24.0] {
            for animation in [0.0, 0.5, 1.0] {
                let layout = PaletteLayout::calculate(800.0, 600.0, cell_width, 25.0, animation, 9);
                for text in [
                    "",
                    "a界e\u{301}👩\u{200d}💻🇺🇸z",
                    &"a界e\u{301}👩\u{200d}💻".repeat(24),
                ] {
                    for cursor in [0, text.len()] {
                        let view = layout.input_viewport(text, cursor).unwrap();
                        let y = view.text_y;
                        let mut x = view.text_x;
                        for (byte, grapheme) in text[view.text_range.clone()].grapheme_indices(true)
                        {
                            let width = termeric_core::unicode::grapheme_width(grapheme) as f32
                                * cell_width;
                            for (fraction, expected) in
                                [(0.25, byte), (0.75, byte + grapheme.len())]
                            {
                                assert_eq!(
                                    layout.input_cursor_at(text, cursor, x + fraction * width, y),
                                    Some(view.text_range.start + expected)
                                );
                            }
                            x += width;
                        }
                        assert_eq!(
                            layout.input_cursor_at(
                                text,
                                cursor,
                                layout.panel_x + layout.padding,
                                y
                            ),
                            Some(view.text_range.start)
                        );
                        assert_eq!(
                            layout.input_cursor_at(
                                text,
                                cursor,
                                layout.panel_x + layout.panel_width - layout.padding - 0.01,
                                y
                            ),
                            Some(view.text_range.end)
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn pointer_caret_rejects_outside_invalid_and_collapsed_fields() {
        let layout = PaletteLayout::calculate(800.0, 600.0, 10.0, 20.0, 1.0, 4);
        let view = layout.input_viewport("界", 0).unwrap();
        for (x, y) in [
            (f32::NAN, view.text_y),
            (view.text_x, f32::INFINITY),
            (layout.panel_x, view.text_y),
            (view.text_x, layout.list_y),
            (
                view.text_x,
                layout.panel_y + layout.padding + layout.input_height,
            ),
        ] {
            assert_eq!(layout.input_cursor_at("界", 0, x, y), None);
        }
        assert_eq!(
            layout.input_cursor_at("界", 1, view.text_x, view.text_y),
            None
        );
        let narrow = PaletteLayout::calculate(20.0, 600.0, 10.0, 20.0, 1.0, 0);
        assert_eq!(
            narrow.input_cursor_at("界", 0, narrow.panel_x, narrow.panel_y),
            None
        );
    }

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
