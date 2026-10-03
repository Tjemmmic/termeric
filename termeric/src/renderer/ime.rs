//! Platform input-method preedit rendering.

use unicode_segmentation::UnicodeSegmentation;

use super::{ImePreeditRenderInfo, Renderer};
use crate::ui::color::with_alpha;

/// Where the preedit is drawn: its box in window pixels and the text that
/// fits in it.
struct PreeditBox<'a> {
    x: f32,
    y: f32,
    width: f32,
    height: f32,
    visible_text: &'a str,
}

const PREEDIT_PADDING: f32 = 5.0;

/// The preedit box for `info` in a window of `window` pixels with cells of
/// `cell` pixels: at the caret, as wide as the text that fits beside it,
/// and above the caret when there is no room below.
fn preedit_box(
    info: &ImePreeditRenderInfo,
    window: (f32, f32),
    cell: (f32, f32),
) -> PreeditBox<'_> {
    let (window_width, window_height) = window;
    let (cell_width, cell_height) = cell;
    let padding = PREEDIT_PADDING;
    let available_width = info
        .max_width
        .min((window_width - padding).max(1.0))
        .max(1.0);
    // A narrow remainder may fit no complete grapheme. Never force a glyph
    // past the owning field/pane merely to show one character.
    let text_capacity = ((available_width - padding * 2.0).max(0.0) / cell_width).floor() as usize;
    let visible_text = preedit_prefix(&info.text, text_capacity);
    let text_columns = display_columns(visible_text).max(1);
    let natural_width = text_columns as f32 * cell_width + padding * 2.0;
    let width = natural_width
        .min(available_width)
        .min((window_width - padding * 2.0).max(1.0));
    let height = cell_height + padding * 2.0;
    let x = info.x.min((window_width - width - padding).max(padding));
    let y = if info.y + height <= window_height - padding {
        info.y
    } else {
        (info.y - height).max(padding)
    };
    PreeditBox {
        x,
        y,
        width,
        height,
        visible_text,
    }
}

impl Renderer {
    /// The box the preedit for `info` is drawn in, as x, y, width, and
    /// height in window pixels. The input method is given it as the text
    /// being composed, so its candidate window opens beside the box rather
    /// than over it.
    pub fn ime_preedit_bounds(&self, info: &ImePreeditRenderInfo) -> [f32; 4] {
        let (width, height) = self.size;
        let preedit = preedit_box(
            info,
            (width as f32, height as f32),
            (self.cell_width(), self.cell_height()),
        );
        [preedit.x, preedit.y, preedit.width, preedit.height]
    }

    pub(super) fn build_ime_preedit(
        &mut self,
        info: &ImePreeditRenderInfo,
        window_width: f32,
        window_height: f32,
    ) {
        let padding = PREEDIT_PADDING;
        let PreeditBox {
            x,
            y,
            width,
            height,
            visible_text,
        } = preedit_box(
            info,
            (window_width, window_height),
            (self.cell_width(), self.cell_height()),
        );

        self.rect_pipeline.push_rounded_rect(
            x,
            y,
            width,
            height,
            with_alpha(self.ui.palette_input_bg, 0.98),
            5.0,
        );
        self.rect_pipeline.push_rect(
            x + 3.0,
            y + height - 2.0,
            (width - 6.0).max(1.0),
            1.0,
            with_alpha(self.ui.palette_accent, 0.75),
        );

        let text_x = x + padding;
        let text_y = y + padding;
        self.text_pipeline.push_ui_run(
            visible_text,
            text_x,
            text_y,
            self.ui.palette_text,
            self.cell_width(),
            super::text::TextClip {
                left: x,
                top: y,
                right: x + width,
                bottom: y + height,
            },
        );

        if let Some((start, end)) = info.cursor
            && start <= visible_text.len()
        {
            let end = end.min(visible_text.len());
            let prefix_columns = display_columns(&visible_text[..start]);
            let selected_columns = display_columns(&visible_text[start..end]);
            let cursor_x = text_x + prefix_columns as f32 * self.cell_width();
            if start == end {
                self.rect_pipeline.push_rect(
                    cursor_x.min(x + width - 2.0),
                    text_y,
                    2.0,
                    self.cell_height(),
                    self.ui.palette_accent,
                );
            } else {
                self.rect_pipeline.push_rect(
                    cursor_x,
                    y + height - 3.0,
                    (selected_columns.max(1) as f32 * self.cell_width())
                        .min((x + width - cursor_x).max(1.0)),
                    2.0,
                    self.ui.palette_accent,
                );
            }
        }
    }
}

fn display_columns(text: &str) -> usize {
    UnicodeSegmentation::graphemes(text, true)
        .map(termeric_core::unicode::grapheme_width)
        .sum()
}

fn preedit_prefix(text: &str, max_columns: usize) -> &str {
    let mut columns = 0_usize;
    let mut end = 0;
    for (offset, grapheme) in UnicodeSegmentation::grapheme_indices(text, true) {
        let width = termeric_core::unicode::grapheme_width(grapheme);
        if columns.saturating_add(width) > max_columns {
            break;
        }
        columns += width;
        end = offset + grapheme.len();
    }
    &text[..end]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cursor_geometry_uses_grapheme_width_not_scalar_count() {
        assert_eq!(display_columns("e\u{301}"), 1);
        assert_eq!(display_columns("👩\u{200d}💻"), 2);
        assert_eq!(display_columns("日本"), 4);
    }

    fn info(text: &str, x: f32, y: f32) -> ImePreeditRenderInfo {
        ImePreeditRenderInfo {
            text: text.into(),
            cursor: None,
            x,
            y,
            max_width: 400.0,
        }
    }

    #[test]
    fn the_preedit_box_holds_the_whole_text_and_stays_in_the_window() {
        // Two wide characters take four cells, plus padding each side.
        let below_info = info("日本", 100.0, 50.0);
        let below = preedit_box(&below_info, (800.0, 600.0), (10.0, 20.0));
        assert_eq!(
            (below.x, below.y, below.width, below.height),
            (100.0, 50.0, 50.0, 30.0)
        );
        // Near the bottom it goes above the caret, and the input method is
        // told so, so its candidates never land on the composed text.
        let above_info = info("日本", 100.0, 580.0);
        let above = preedit_box(&above_info, (800.0, 600.0), (10.0, 20.0));
        assert_eq!((above.y, above.height), (550.0, 30.0));
        // At the right edge it moves left to stay whole.
        let right_info = info("日本", 790.0, 50.0);
        let right = preedit_box(&right_info, (800.0, 600.0), (10.0, 20.0));
        assert_eq!(right.x + right.width, 795.0);
    }

    #[test]
    fn pane_local_preedit_clipping_preserves_complete_graphemes() {
        assert_eq!(preedit_prefix("ab👩\u{200d}💻cd", 4), "ab👩\u{200d}💻");
        assert_eq!(preedit_prefix("e\u{301}日本", 3), "e\u{301}日");
        assert_eq!(preedit_prefix("界", 1), "");
    }
}
