//! The centred dialogs that filter a list: the command palette (and the
//! pickers built on it) and Keyboard Shortcuts. Each is an optional title, a
//! filter field, a separator, the visible rows, and a footer line.

use crate::ui::geometry::Bounds;
use crate::ui::text_input_viewport::TextInputViewport;

/// What sets one list dialog apart from another.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ListDialogShape {
    /// Preferred width in cell widths, at least `min_width` and at most
    /// `max_window_share` of the window.
    pub width_cells: f32,
    pub min_width: f32,
    pub max_window_share: f32,
    /// The dialog's top as a share of the window height, once open.
    pub top_share: f32,
    /// Heights in cell heights. A dialog without a title has `title_cells`
    /// zero.
    pub title_cells: f32,
    pub row_cells: f32,
    pub footer_cells: f32,
}

/// Geometry for a list dialog — the single source of truth shared by the
/// renderer, hit tests, scrolling, the IME caret, and accessibility.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ListDialogLayout {
    /// The dialog, on whole pixels.
    pub panel: Bounds,
    pub padding: f32,
    /// Space above and below the separator under the filter field.
    pub gap: f32,
    pub separator_height: f32,
    /// The title row above the filter field; zero without a title.
    pub title_height: f32,
    pub input: Bounds,
    /// The visible rows, inset from the dialog's sides by half its padding.
    pub list: Bounds,
    pub row_height: f32,
    pub rows_that_fit: usize,
    pub visible_count: usize,
    pub footer_height: f32,
    pub cell_width: f32,
    pub cell_height: f32,
}

impl ListDialogLayout {
    pub fn calculate(
        shape: ListDialogShape,
        window_width: f32,
        window_height: f32,
        cell_width: f32,
        cell_height: f32,
        animation: f32,
        entry_count: usize,
    ) -> Self {
        let width = (cell_width * shape.width_cells)
            .max(shape.min_width)
            .min(window_width * shape.max_window_share);
        let padding = cell_width * 1.5;
        let row_height = cell_height * shape.row_cells;
        let input_height = cell_height * 2.0;
        let footer_height = cell_height * shape.footer_cells;
        let gap = 6.0;
        let separator_height = 1.0;
        let title_height = cell_height * shape.title_cells;
        let title_band = if title_height > 0.0 {
            title_height + gap
        } else {
            0.0
        };
        let header_area = padding + title_band + input_height + gap + separator_height + gap;
        let footer_area = footer_height + padding * 0.5;
        let max_list_height = (window_height * 0.75 - header_area - footer_area).max(row_height);
        let rows_that_fit = (max_list_height / row_height).floor().max(1.0) as usize;
        let visible_count = entry_count.min(rows_that_fit);
        let list_height = visible_count as f32 * row_height;
        let panel = Bounds::new(
            ((window_width - width) / 2.0).round(),
            (window_height * shape.top_share + (1.0 - animation) * -30.0).round(),
            width.round(),
            (header_area + list_height + footer_area).round(),
        );
        let input = Bounds::new(
            panel.x + padding,
            panel.y + padding + title_band,
            panel.width - padding * 2.0,
            input_height,
        );
        Self {
            panel,
            padding,
            gap,
            separator_height,
            title_height,
            input,
            list: Bounds::new(
                panel.x + padding * 0.5,
                input.bottom() + gap + separator_height + gap,
                panel.width - padding,
                list_height,
            ),
            row_height,
            rows_that_fit,
            visible_count,
            footer_height,
            cell_width,
            cell_height,
        }
    }

    pub fn contains(&self, x: f32, y: f32) -> bool {
        self.panel.contains(x, y)
    }

    /// The `visible_index`th visible row. Its edges are rounded to whole
    /// pixels, and each row's bottom is the next row's top, so the rows tile
    /// the list with no gap or overlap.
    pub fn row(&self, visible_index: usize) -> Bounds {
        let edge = |index: usize| (self.list.y + index as f32 * self.row_height).round();
        let top = edge(visible_index);
        Bounds::new(
            self.list.x,
            top,
            self.list.width,
            edge(visible_index + 1) - top,
        )
    }

    /// The visible row under a point, if any: the one whose drawn bounds
    /// contain it.
    pub fn row_at(&self, x: f32, y: f32) -> Option<usize> {
        (0..self.visible_count).find(|&index| self.row(index).contains(x, y))
    }

    /// Keep the caret visible without splitting a grapheme or drawing outside
    /// the field. The full query remains in the model and accessibility tree.
    pub fn input_viewport(&self, text: &str, cursor: usize) -> Option<TextInputViewport> {
        crate::ui::filter_field::text_viewport(
            text,
            cursor,
            self.input,
            self.cell_width,
            self.cell_height,
        )
    }

    /// Resolve the nearest visible grapheme boundary, including scrolled text.
    /// Blank space selects the visible edge, never an off-screen byte position.
    pub fn input_cursor_at(&self, text: &str, cursor: usize, x: f32, y: f32) -> Option<usize> {
        if !self.input.contains(x, y) {
            return None;
        }
        self.input_viewport(text, cursor)?
            .cursor_at(text, x, self.cell_width)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::command_palette::PALETTE_DIALOG;
    use crate::ui::shortcuts_panel::SHORTCUTS_DIALOG;
    use unicode_segmentation::UnicodeSegmentation;

    #[test]
    fn rows_answer_only_where_they_are_drawn_and_the_dialog_sits_on_whole_pixels() {
        for shape in [PALETTE_DIALOG, SHORTCUTS_DIALOG] {
            let layout = ListDialogLayout::calculate(shape, 1001.0, 701.0, 9.5, 19.5, 0.5, 3);
            let panel = layout.panel;
            assert_eq!(
                [panel.x, panel.y, panel.width, panel.height].map(f32::fract),
                [0.0; 4]
            );
            assert!(layout.input.bottom() < layout.list.y);
            let last = layout.row(2);
            assert_eq!(layout.row_at(last.x, last.y + 0.5), Some(2));
            assert_eq!(
                layout.row_at(last.right() - 0.5, last.bottom() - 0.5),
                Some(2)
            );
            // The side padding and the edges after the last row belong to the frame.
            assert_eq!(layout.row_at(last.x - 0.5, last.y + 0.5), None);
            assert_eq!(layout.row_at(last.right(), last.y + 0.5), None);
            assert_eq!(layout.row_at(last.x, last.bottom()), None);
        }
        let titled =
            ListDialogLayout::calculate(SHORTCUTS_DIALOG, 1000.0, 700.0, 10.0, 20.0, 1.0, 0);
        assert_eq!(
            titled.input.y,
            titled.panel.y + titled.padding + titled.title_height + titled.gap
        );
        let untitled =
            ListDialogLayout::calculate(PALETTE_DIALOG, 1000.0, 700.0, 10.0, 20.0, 1.0, 0);
        assert_eq!(untitled.input.y, untitled.panel.y + untitled.padding);
    }

    #[test]
    fn every_row_edge_belongs_to_the_row_drawn_below_it() {
        for shape in [PALETTE_DIALOG, SHORTCUTS_DIALOG] {
            for cell_height in [13.7, 19.5, 23.3, 31.9] {
                let layout =
                    ListDialogLayout::calculate(shape, 1280.0, 4000.0, 9.5, cell_height, 0.7, 60);
                assert!(layout.visible_count > 20);
                for index in 0..layout.visible_count {
                    let row = layout.row(index);
                    assert_eq!([row.y, row.height].map(f32::fract), [0.0; 2]);
                    assert_eq!(layout.row_at(row.x, row.y), Some(index));
                    assert_eq!(
                        layout.row_at(row.x, row.y.next_down()),
                        index.checked_sub(1)
                    );
                    assert_eq!(layout.row_at(row.x, row.bottom().next_down()), Some(index));
                    if index + 1 < layout.visible_count {
                        assert_eq!(row.bottom(), layout.row(index + 1).y);
                    }
                }
            }
        }
    }

    #[test]
    fn pointer_caret_uses_visible_grapheme_midpoints_at_every_scale_and_animation() {
        for cell_width in [6.0, 9.5, 24.0] {
            for animation in [0.0, 0.5, 1.0] {
                let layout = ListDialogLayout::calculate(
                    PALETTE_DIALOG,
                    800.0,
                    600.0,
                    cell_width,
                    25.0,
                    animation,
                    9,
                );
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
                                layout.panel.x + layout.padding,
                                y
                            ),
                            Some(view.text_range.start)
                        );
                        assert_eq!(
                            layout.input_cursor_at(
                                text,
                                cursor,
                                layout.panel.right() - layout.padding - 0.01,
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
        let layout = ListDialogLayout::calculate(PALETTE_DIALOG, 800.0, 600.0, 10.0, 20.0, 1.0, 4);
        let view = layout.input_viewport("界", 0).unwrap();
        for (x, y) in [
            (f32::NAN, view.text_y),
            (view.text_x, f32::INFINITY),
            (layout.panel.x, view.text_y),
            (view.text_x, layout.list.y),
            (view.text_x, layout.input.bottom()),
        ] {
            assert_eq!(layout.input_cursor_at("界", 0, x, y), None);
        }
        assert_eq!(
            layout.input_cursor_at("界", 1, view.text_x, view.text_y),
            None
        );
        let narrow = ListDialogLayout::calculate(PALETTE_DIALOG, 20.0, 600.0, 10.0, 20.0, 1.0, 0);
        assert_eq!(
            narrow.input_cursor_at("界", 0, narrow.panel.x, narrow.panel.y),
            None
        );
    }

    #[test]
    fn query_viewport_keeps_every_caret_and_complete_grapheme_inside_the_field() {
        for window_width in [160.0, 450.0, 800.0, 1920.0] {
            for cell_width in [6.0, 9.5, 24.0] {
                for animation in [0.0, 0.5, 1.0] {
                    let layout = ListDialogLayout::calculate(
                        PALETTE_DIALOG,
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
                                    layout.panel.width < layout.padding * 2.0 + cell_width * 4.0
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
        let layout = ListDialogLayout::calculate(PALETTE_DIALOG, 800.0, 600.0, 10.0, 20.0, 1.0, 0);
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
