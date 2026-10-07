//! Non-color-only keyboard copy cursor, mode status, and quick-select labels.

use super::{CopyModeRenderInfo, Renderer};
use crate::ui::color::with_alpha;

fn visible_row(row: i32, scroll_offset: usize, pane_rows: usize) -> Option<usize> {
    let offset = i32::try_from(scroll_offset).unwrap_or(i32::MAX);
    usize::try_from(row.saturating_add(offset))
        .ok()
        .filter(|row| *row < pane_rows)
}

impl Renderer {
    pub(super) fn build_copy_mode_overlay(
        &mut self,
        info: &CopyModeRenderInfo,
        scroll_offset: usize,
        offset_x: f32,
        offset_y: f32,
        pane_width: f32,
        pane_rows: usize,
        window_height: f32,
    ) {
        let cell_width = self.cell_width();
        let cell_height = self.cell_height();
        let visible_rows = (((window_height - self.padding - offset_y).max(cell_height)
            / cell_height)
            .floor() as usize)
            .min(pane_rows)
            .max(1);
        let cursor_row = visible_row(info.cursor.row, scroll_offset, visible_rows);

        // Four physical edges make the copy cursor distinguishable without
        // relying on hue or opacity alone.
        if let Some(visual_row) = cursor_row {
            let x = offset_x + info.cursor.col as f32 * cell_width;
            let y = offset_y + visual_row as f32 * cell_height;
            let color = self.ui.palette_accent;
            let edge = 2.0;
            self.rect_pipeline.push_rect(x, y, cell_width, edge, color);
            self.rect_pipeline
                .push_rect(x, y + cell_height - edge, cell_width, edge, color);
            self.rect_pipeline.push_rect(x, y, edge, cell_height, color);
            self.rect_pipeline
                .push_rect(x + cell_width - edge, y, edge, cell_height, color);
        }

        for hint in &info.hints {
            let Some(row) = visible_row(hint.row, scroll_offset, visible_rows) else {
                continue;
            };
            let x = offset_x + hint.col as f32 * cell_width;
            let y = offset_y + row as f32 * cell_height;
            let width = (hint.label.chars().count().max(1) as f32 * cell_width) + 4.0;
            let accent = if hint.matches_prefix {
                self.ui.palette_accent
            } else {
                with_alpha(self.ui.palette_border, 0.92)
            };
            let background = with_alpha(self.ui.palette_bg, 0.98);
            self.rect_pipeline
                .push_rounded_rect(x, y, width, cell_height, accent, 3.0);
            self.rect_pipeline.push_rounded_rect(
                x + 1.0,
                y + 1.0,
                width - 2.0,
                cell_height - 2.0,
                background,
                2.0,
            );
            let mut text_x = x + 2.0;
            for character in hint.label.chars() {
                self.text_pipeline
                    .push_char(character, text_x, y, self.ui.palette_text);
                text_x += cell_width;
            }
        }

        let status_width = ((info.status.chars().count() as f32 + 2.0) * cell_width)
            .min(pane_width)
            .max(cell_width * 8.0);
        let status_y = offset_y + visible_rows.saturating_sub(1) as f32 * cell_height;
        self.rect_pipeline.push_rounded_rect(
            offset_x,
            status_y,
            status_width,
            cell_height,
            with_alpha(self.ui.palette_bg, 0.96),
            3.0,
        );
        let mut text_x = offset_x + cell_width;
        for character in info.status.chars().take(pane_width as usize) {
            self.text_pipeline
                .push_char(character, text_x, status_y, self.ui.palette_text);
            text_x += cell_width;
            if text_x + cell_width > offset_x + pane_width {
                break;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::renderer::QuickSelectHintRenderInfo;

    /// Golden contract for the scrollback-to-overlay coordinate mapping. The
    /// actual native harness separately captures the rendered X11 surface.
    #[test]
    fn copy_and_hint_overlay_visual_rows_match_the_golden_snapshot() {
        let info = CopyModeRenderInfo {
            cursor: termeric_core::Point::new(-3, 7),
            status: "COPY MODE".into(),
            hints: vec![
                QuickSelectHintRenderInfo {
                    label: "a".into(),
                    row: -5,
                    col: 2,
                    matches_prefix: false,
                },
                QuickSelectHintRenderInfo {
                    label: "s".into(),
                    row: 1,
                    col: 9,
                    matches_prefix: true,
                },
            ],
        };
        let snapshot = format!(
            "cursor={:?}; hints={:?}",
            visible_row(info.cursor.row, 5, 6),
            info.hints
                .iter()
                .map(|hint| (hint.label.as_str(), visible_row(hint.row, 5, 6), hint.col))
                .collect::<Vec<_>>()
        );
        assert_eq!(
            snapshot,
            "cursor=Some(2); hints=[(\"a\", Some(0), 2), (\"s\", None, 9)]"
        );
    }
}
