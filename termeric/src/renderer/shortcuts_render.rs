//! Shortcuts panel rendering — centered modal with a filter, category
//! headings, and key recording, styled like the command palette.

use super::Renderer;
use super::draw::FieldTextColors;
use crate::input::keybindings::BindableAction;
use crate::ui::color::scale_alpha;
use crate::ui::constants::BUTTON_CORNER_RADIUS;
use crate::ui::geometry::Bounds;
use crate::ui::list_dialog::ListDialogLayout;
use crate::ui::shortcuts_panel::{SHORTCUTS_DIALOG, ShortcutsPanelInfo};

impl Renderer {
    /// Build shortcuts panel render data — centered modal with a light scrim.
    /// Geometry comes from `ListDialogLayout`, which the filter caret,
    /// IME, pointer, and accessibility code share; text uses the UI face.
    pub(super) fn build_shortcuts_panel(
        &mut self,
        info: &ShortcutsPanelInfo,
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

        let layout = ListDialogLayout::calculate(
            SHORTCUTS_DIALOG,
            window_width,
            window_height,
            cell_width,
            cell_height,
            anim,
            info.entries.len(),
        );
        let Bounds {
            x: panel_x,
            y: panel_y,
            width: panel_width,
            height: panel_height,
        } = layout.panel;
        let padding = layout.padding;
        let row_height = layout.row_height;
        self.build_modal_frame(panel_x, panel_y, panel_width, panel_height, anim);
        let line = self.px(1.0);
        let size = self.ui_label_px();
        let text = fade(self.ui.palette_text, 1.0);
        let dim = fade(self.ui.menu_text_dim, 1.0);
        let accent = fade(self.ui.palette_accent, 1.0);

        // ── Title ──
        self.push_ui_label(
            "Keyboard Shortcuts",
            panel_x + padding,
            panel_y + padding,
            layout.title_height,
            panel_width - padding * 2.0,
            size * 1.15,
            text,
        );

        // ── Filter field ──
        let input = layout.input;
        self.draw_filter_field(input, fade(self.ui.palette_input_bg, 0.8), dim);
        if let Some(view) = layout.input_viewport(&info.filter, info.cursor) {
            self.draw_field_text(
                input,
                &view,
                &info.filter,
                "Filter shortcuts",
                FieldTextColors {
                    text,
                    placeholder: fade(self.ui.palette_text_dim, 0.6),
                },
                (!info.recording).then(|| fade(self.ui.palette_accent, 0.9)),
            );
        }

        // ── Separator ──
        let separator_y = (input.y + input.height + layout.gap).round();
        self.rect_pipeline.push_rect(
            panel_x + line,
            separator_y,
            panel_width - line * 2.0,
            layout.separator_height,
            fade(self.ui.menu_border, 1.0),
        );

        // ── Headings and shortcuts (already sliced to the visible window) ──
        let list_y = layout.list.y;
        let scrollbar_width = self.px(3.0);
        let label_x = panel_x + padding + cell_width * 0.5;
        let right = panel_x + panel_width - padding - scrollbar_width - self.px(6.0);
        for (row_index, entry) in info.entries.iter().enumerate().take(layout.rows_that_fit) {
            let row_y = (list_y + row_index as f32 * row_height).round();
            if entry.is_header {
                self.push_ui_label(
                    entry.label,
                    panel_x + padding,
                    row_y,
                    row_height,
                    right - panel_x - padding,
                    size * 0.9,
                    dim,
                );
                continue;
            }
            if entry.selected {
                self.rect_pipeline.push_rounded_rect(
                    panel_x + padding * 0.5,
                    row_y,
                    panel_width - padding,
                    row_height,
                    fade(self.ui.menu_highlight, 1.0),
                    self.px(BUTTON_CORNER_RADIUS),
                );
            }
            let (binding, binding_color) = if entry.selected && info.recording {
                ("Press a shortcut…", accent)
            } else if entry.binding == "Unset" {
                ("Not set", fade(self.ui.menu_text_dim, 0.6))
            } else {
                (entry.binding.as_str(), dim)
            };
            let available = (right - label_x) * 0.5;
            let binding_width = self.ui_text_width(binding, size).min(available);
            self.push_ui_label(
                binding,
                right - binding_width,
                row_y,
                row_height,
                available,
                size,
                binding_color,
            );
            let label_color = fade(
                self.ui.palette_text,
                if entry.selected { 1.0 } else { 0.88 },
            );
            self.push_ui_label(
                entry.label,
                label_x,
                row_y,
                row_height,
                right - binding_width - self.px(16.0) - label_x,
                size,
                label_color,
            );
        }

        // ── Scrollbar: a thin thumb only while rows overflow ──
        self.draw_scroll_thumb(
            Bounds::new(
                panel_x + panel_width - padding * 0.5 - scrollbar_width,
                list_y,
                scrollbar_width,
                layout.list.height,
            ),
            layout.rows_that_fit,
            info.total_rows,
            info.scroll_offset,
            fade(self.ui.menu_text_dim, 0.6),
        );

        // ── Footer: count on the left, one plain instruction on the right ──
        let footer_y = (panel_y + panel_height - layout.footer_height - padding * 0.5).round();
        let footer_height = layout.footer_height + padding * 0.5;
        self.rect_pipeline.push_rect(
            panel_x + line,
            footer_y,
            panel_width - line * 2.0,
            line,
            fade(self.ui.menu_border, 1.0),
        );
        let total = BindableAction::all().len();
        let count = if info.total_entries < total {
            format!("{} of {total} shortcuts", info.total_entries)
        } else {
            format!("{total} shortcuts")
        };
        let footer_size = size * 0.9;
        let count_width = self.push_ui_label(
            &count,
            panel_x + padding,
            footer_y,
            footer_height,
            panel_width - padding * 2.0,
            footer_size,
            dim,
        );
        let (instruction, instruction_color) = if info.recording {
            ("Esc cancels · Delete clears".to_string(), dim)
        } else if let Some(notice) = &info.notice {
            (notice.message(), accent)
        } else {
            ("Enter changes · Delete clears".to_string(), dim)
        };
        let instruction_right = panel_x + panel_width - padding;
        let available = instruction_right - (panel_x + padding + count_width + self.px(24.0));
        let width = self
            .ui_text_width(&instruction, footer_size)
            .min(available.max(0.0));
        self.push_ui_label(
            &instruction,
            instruction_right - width,
            footer_y,
            footer_height,
            available,
            footer_size,
            instruction_color,
        );
    }
}
