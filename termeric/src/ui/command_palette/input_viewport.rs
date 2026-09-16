//! Shared query scroll/caret geometry for painting and native IME positioning.

use super::{PaletteInputViewport, PaletteLayout};
#[cfg(test)]
use unicode_segmentation::UnicodeSegmentation;

impl PaletteLayout {
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
