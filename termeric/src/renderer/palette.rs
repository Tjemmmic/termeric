//! Command palette rendering — centered modal with search and results list

use super::Renderer;
use super::draw::FieldTextColors;
use crate::ui::color::scale_alpha;
use crate::ui::command_palette::PaletteInfo;
use crate::ui::constants::{BUTTON_CORNER_RADIUS, MODAL_CORNER_RADIUS};
use crate::ui::geometry::Bounds;
use crate::ui::list_dialog::ListDialogLayout;

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
    /// The query field keeps the shared `ListDialogLayout` geometry that caret,
    /// IME, pointer, and accessibility code use; rows draw in the UI face.
    pub(super) fn build_command_palette(
        &mut self,
        info: &PaletteInfo,
        layout: &ListDialogLayout,
        window_width: f32,
        window_height: f32,
    ) {
        let anim = info.animation;
        let cell_width = self.cell_width();
        let fade = |color: [f32; 4], alpha: f32| scale_alpha(color, alpha * anim);

        let scrim = fade(self.ui.palette_scrim, 1.0);
        self.rect_pipeline
            .push_rect(0.0, 0.0, window_width, window_height, scrim);

        let layout = *layout;
        let Bounds {
            x: panel_x,
            y: panel_y,
            width: panel_width,
            height: panel_height,
        } = layout.panel;
        let padding = layout.padding;

        self.build_modal_frame(panel_x, panel_y, panel_width, panel_height, anim);
        let line = self.px(1.0);

        // ── Query field ──
        let field = layout.input;
        self.draw_filter_field(
            field,
            fade(self.ui.palette_input_bg, 0.8),
            fade(self.ui.palette_text_dim, 1.0),
        );
        // One grapheme-safe, caret-following viewport also anchors native IME.
        if let Some(view) = layout.input_viewport(&info.input, info.cursor) {
            self.draw_field_text(
                field,
                &view,
                &info.input,
                &info.placeholder,
                FieldTextColors {
                    text: fade(self.ui.palette_text, 1.0),
                    placeholder: fade(self.ui.palette_text_dim, 0.6),
                },
                Some(fade(self.ui.palette_accent, 0.9)),
            );
        }

        // ── Separator ──
        let sep_y = field.bottom() + layout.gap;
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
        let list_y = layout.list.y;
        let text_x = panel_x + padding + cell_width * 0.5;
        let scrollbar_width = self.px(3.0);
        let right = panel_x + panel_width - padding - scrollbar_width - self.px(6.0);
        for (row_index, entry) in info.entries.iter().enumerate().take(layout.visible_count) {
            let row = layout.row(row_index);
            let selected = row_index == info.selected;
            if selected {
                self.rect_pipeline.push_rounded_rect(
                    row.x,
                    row.y,
                    row.width,
                    row.height,
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
                        row.y,
                        row.height,
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
                row.y,
                row.height,
                right - detail_width - text_x,
                size,
                text,
                fade(self.ui.palette_match_highlight, 1.0),
            );
        }

        // ── Scrollbar: a thin thumb only while results overflow ──
        self.draw_scroll_thumb(
            Bounds::new(
                panel_x + panel_width - padding * 0.5 - scrollbar_width,
                list_y,
                scrollbar_width,
                layout.list.height,
            ),
            layout.visible_count,
            info.total_entries,
            info.scroll_offset,
            fade(self.ui.menu_text_dim, 0.6),
        );

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
