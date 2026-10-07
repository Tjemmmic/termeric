//! Command palette rendering — centered modal with search and results list

use super::Renderer;
use crate::ui::color::scale_alpha;
use crate::ui::command_palette::{PaletteInfo, PaletteLayout};
use crate::ui::constants::{BUTTON_CORNER_RADIUS, MODAL_CORNER_RADIUS};
use crate::ui::geometry::Bounds;

impl Renderer {
    /// A centred modal's soft shadow, hairline border, and raised surface,
    /// faded in with `animation`. Shared by the palette-style dialogs.
    pub(super) fn build_modal_frame(
        &mut self,
        x: f32,
        y: f32,
        width: f32,
        height: f32,
        animation: f32,
    ) {
        self.push_raised_surface(
            Bounds::new(x, y, width, height),
            MODAL_CORNER_RADIUS,
            &[(2.0, 0.18), (10.0, 0.10)],
            3.0,
            self.ui.menu_border,
            animation,
        );
    }

    /// Build command palette render data — centered modal with a light scrim.
    /// The query field keeps the shared `PaletteLayout` geometry that caret,
    /// IME, pointer, and accessibility code use; rows draw in the UI face.
    pub(super) fn build_command_palette(
        &mut self,
        info: &PaletteInfo,
        window_width: f32,
        window_height: f32,
    ) {
        let anim = info.animation;
        let cell_width = self.cell_width();
        let cell_height = self.cell_height();
        let fade = |color: [f32; 4], alpha: f32| scale_alpha(color, alpha * anim);

        let scrim = fade(self.ui.palette_scrim, 1.0);
        self.rect_pipeline
            .push_rect(0.0, 0.0, window_width, window_height, scrim);

        let layout = PaletteLayout::calculate(
            window_width,
            window_height,
            cell_width,
            cell_height,
            anim,
            info.entries.len(),
        );
        let (panel_x, panel_y) = (layout.panel_x.round(), layout.panel_y.round());
        let (panel_width, panel_height) = (layout.panel_width.round(), layout.panel_height.round());
        let padding = layout.padding;
        let row_height = layout.row_height;

        self.build_modal_frame(panel_x, panel_y, panel_width, panel_height, anim);
        let line = self.px(1.0);

        // ── Query field ──
        let input_x = panel_x + padding;
        let input_y = panel_y + padding;
        let input_w = panel_width - padding * 2.0;
        let input_height = layout.input_height;
        self.rect_pipeline.push_rounded_rect(
            input_x,
            input_y,
            input_w,
            input_height,
            fade(self.ui.palette_input_bg, 0.8),
            self.px(BUTTON_CORNER_RADIUS),
        );
        // Magnifier in the slot before the query text.
        self.push_magnifier(
            input_x + cell_width * crate::ui::constants::FILTER_ICON_CENTER_CELLS,
            input_y + input_height / 2.0,
            fade(self.ui.palette_text_dim, 1.0),
            fade(self.ui.palette_input_bg, 0.8),
        );

        // One grapheme-safe, caret-following viewport also anchors native IME.
        if let Some(view) = layout.input_viewport(&info.input, info.cursor) {
            let clip = super::text::TextClip {
                left: view.text_x,
                top: input_y,
                right: view.right,
                bottom: input_y + input_height,
            };
            if info.input.is_empty() {
                if let Some(hint) = layout.input_viewport(&info.placeholder, 0) {
                    self.text_pipeline.push_ui_run(
                        &info.placeholder[hint.text_range],
                        view.text_x,
                        view.text_y,
                        fade(self.ui.palette_text_dim, 0.7),
                        cell_width,
                        clip,
                    );
                }
            } else {
                self.text_pipeline.push_ui_run(
                    &info.input[view.text_range],
                    view.text_x,
                    view.text_y,
                    fade(self.ui.palette_text, 1.0),
                    cell_width,
                    clip,
                );
            }
            self.rect_pipeline.push_rect(
                view.cursor_x,
                view.text_y,
                self.px(2.0),
                cell_height,
                fade(self.ui.palette_accent, 0.9),
            );
        }

        // ── Separator ──
        let sep_y = input_y + input_height + layout.gap;
        self.rect_pipeline.push_rect(
            panel_x + line,
            sep_y.round(),
            panel_width - line * 2.0,
            layout.separator_height,
            fade(self.ui.menu_border, 1.0),
        );

        // ── Results ──
        // `info.entries` is already sliced to the visible window.
        let size = self.ui_label_px();
        let list_y = layout.list_y;
        let text_x = panel_x + padding + cell_width * 0.5;
        let scrollbar_width = self.px(3.0);
        let right = panel_x + panel_width - padding - scrollbar_width - self.px(6.0);
        for (row_index, entry) in info.entries.iter().enumerate().take(layout.visible_count) {
            let row_y = (list_y + row_index as f32 * row_height).round();
            let selected = row_index == info.selected;
            if selected {
                self.rect_pipeline.push_rounded_rect(
                    panel_x + padding * 0.5,
                    row_y,
                    panel_width - padding,
                    row_height,
                    fade(self.ui.menu_highlight, 1.0),
                    self.px(BUTTON_CORNER_RADIUS),
                );
            }
            let text = fade(self.ui.palette_text, if selected { 1.0 } else { 0.88 });
            let dim = fade(self.ui.menu_text_dim, 1.0);
            let detail = entry
                .shortcut
                .clone()
                .or_else(|| info.show_category.then(|| entry.category.clone()))
                .filter(|detail| !detail.is_empty());
            let detail_width = match detail {
                Some(detail) => {
                    let width = self.ui_text_width(&detail, size);
                    let available = (right - text_x) * 0.5;
                    self.push_ui_label(
                        &detail,
                        (right - width).max(right - available),
                        row_y,
                        row_height,
                        available,
                        size,
                        dim,
                    );
                    width.min(available) + self.px(16.0)
                }
                None => 0.0,
            };
            self.push_ui_label_highlighted(
                &entry.title,
                &entry.match_positions,
                text_x,
                row_y,
                row_height,
                right - detail_width - text_x,
                size,
                text,
                fade(self.ui.palette_match_highlight, 1.0),
            );
        }

        // ── Scrollbar: a thin thumb only while results overflow ──
        let visible_count = layout.visible_count;
        if info.total_entries > visible_count && visible_count > 0 {
            let track_x = panel_x + panel_width - padding * 0.5 - scrollbar_width;
            let track_h = layout.list_height;
            let thumb_h =
                (track_h * visible_count as f32 / info.total_entries as f32).max(self.px(24.0));
            let max_scroll = info.total_entries.saturating_sub(visible_count);
            let fraction = if max_scroll > 0 {
                info.scroll_offset as f32 / max_scroll as f32
            } else {
                0.0
            };
            self.rect_pipeline.push_rounded_rect(
                track_x,
                list_y + fraction * (track_h - thumb_h),
                scrollbar_width,
                thumb_h,
                fade(self.ui.menu_text_dim, 0.6),
                scrollbar_width / 2.0,
            );
        }

        // ── Footer: result count, or pending/error status ──
        let footer_y = (panel_y + panel_height - layout.footer_height - padding * 0.5).round();
        self.rect_pipeline.push_rect(
            panel_x + line,
            footer_y,
            panel_width - line * 2.0,
            line,
            fade(self.ui.menu_border, 1.0),
        );
        let noun = |count: usize| {
            if count == 1 {
                info.item_noun.clone()
            } else {
                format!("{}s", info.item_noun)
            }
        };
        let footer_text = if let Some(status) = info.status {
            status.to_string()
        } else if info.total_entries < info.total_count {
            format!(
                "{} of {} {}",
                info.total_entries,
                info.total_count,
                noun(info.total_count)
            )
        } else {
            format!("{} {}", info.total_count, noun(info.total_count))
        };
        self.push_ui_label(
            &footer_text,
            panel_x + padding,
            footer_y,
            layout.footer_height + padding * 0.5,
            panel_width - padding * 2.0,
            size * 0.9,
            fade(self.ui.menu_text_dim, 1.0),
        );
    }
}
