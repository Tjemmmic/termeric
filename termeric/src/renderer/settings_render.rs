//! Settings panel rendering — centered modal with search, flat list, and category headers

use super::{Renderer, adjust_alpha};
use crate::ui::constants::*;
use crate::ui::settings::{SettingsModalLayout, SettingsPanelInfo};

impl Renderer {
    /// Build settings panel render data — centered modal with scrim
    pub(super) fn build_settings_panel(
        &mut self,
        info: &SettingsPanelInfo,
        window_width: f32,
        window_height: f32,
    ) {
        let anim = info.animation;
        if anim <= 0.0 {
            return;
        }

        let cell_width = self.cell_width;
        let cell_height = self.cell_height;

        // ── Scrim (full-window dark overlay) ──
        let scrim = adjust_alpha(self.ui.palette_scrim, self.ui.palette_scrim[3] * anim);
        self.rect_pipeline
            .push_rect(0.0, 0.0, window_width, window_height, scrim);

        // ── Geometry via the shared layout struct ──
        let layout = SettingsModalLayout::calculate(
            window_width,
            window_height,
            cell_width,
            cell_height,
            anim,
            info.entries.len(),
        );

        // ── Panel background ──
        let panel_bg = adjust_alpha(self.ui.palette_bg, anim);
        self.rect_pipeline.push_rounded_rect(
            layout.panel_x,
            layout.panel_y,
            layout.panel_width,
            layout.panel_height,
            panel_bg,
            CARD_CORNER_RADIUS,
        );

        // ── Panel border ──
        let border = adjust_alpha(self.ui.palette_border, 0.4 * anim);
        self.rect_pipeline.push_rect(
            layout.panel_x + CARD_CORNER_RADIUS,
            layout.panel_y,
            layout.panel_width - CARD_CORNER_RADIUS * 2.0,
            BORDER_WIDTH,
            border,
        );
        self.rect_pipeline.push_rect(
            layout.panel_x + CARD_CORNER_RADIUS,
            layout.panel_y + layout.panel_height - BORDER_WIDTH,
            layout.panel_width - CARD_CORNER_RADIUS * 2.0,
            BORDER_WIDTH,
            border,
        );
        self.rect_pipeline.push_rect(
            layout.panel_x,
            layout.panel_y + CARD_CORNER_RADIUS,
            BORDER_WIDTH,
            layout.panel_height - CARD_CORNER_RADIUS * 2.0,
            border,
        );
        self.rect_pipeline.push_rect(
            layout.panel_x + layout.panel_width - BORDER_WIDTH,
            layout.panel_y + CARD_CORNER_RADIUS,
            BORDER_WIDTH,
            layout.panel_height - CARD_CORNER_RADIUS * 2.0,
            border,
        );

        // ── Title ──
        let title_y = layout.panel_y + layout.padding;
        let title_x = layout.panel_x + layout.padding;
        let title = "Settings";
        let title_color = adjust_alpha(self.ui.palette_text, anim);
        let title_scale = cell_width * 1.05;
        for (i, c) in title.chars().enumerate() {
            self.text_pipeline
                .push_char(c, title_x + i as f32 * title_scale, title_y, title_color);
        }

        // ── Search input ──
        let input_y = title_y + cell_height * 1.4 + layout.gap;
        let input_x = layout.panel_x + layout.padding;
        let input_w = layout.panel_width - layout.padding * 2.0;
        let input_bg = adjust_alpha(self.ui.palette_input_bg, 0.8 * anim);
        self.rect_pipeline.push_rounded_rect(
            input_x,
            input_y,
            input_w,
            layout.input_height,
            input_bg,
            8.0,
        );

        // Search icon "/"
        let text_y = input_y + (layout.input_height - cell_height) / 2.0;
        let prompt_color = adjust_alpha(self.ui.palette_accent, anim);
        self.text_pipeline
            .push_char('/', input_x + cell_width * 0.8, text_y, prompt_color);

        // Filter text
        let text_start_x = input_x + cell_width * 2.2;
        let mut text_x = text_start_x;
        let text_color = adjust_alpha(self.ui.palette_text, anim);
        for (byte_idx, c) in info.filter.char_indices() {
            if byte_idx == info.filter_cursor {
                let cursor_color = adjust_alpha(self.ui.palette_accent, 0.9 * anim);
                self.rect_pipeline
                    .push_rect(text_x, text_y, 2.0, cell_height, cursor_color);
            }
            self.text_pipeline.push_char(c, text_x, text_y, text_color);
            text_x += cell_width;
        }
        if info.filter_cursor >= info.filter.len() {
            let cursor_color = adjust_alpha(self.ui.palette_accent, 0.9 * anim);
            self.rect_pipeline
                .push_rect(text_x, text_y, 2.0, cell_height, cursor_color);
        }

        // Placeholder
        if info.filter.is_empty() {
            let hint = "Filter settings...";
            let hint_color = adjust_alpha(self.ui.palette_text_dim, 0.6 * anim);
            let mut hx = text_start_x;
            for c in hint.chars() {
                self.text_pipeline.push_char(c, hx, text_y, hint_color);
                hx += cell_width;
            }
        }

        // ── Separator ──
        let sep_y = input_y + layout.input_height + layout.gap;
        let sep_color = adjust_alpha(self.ui.palette_border, 0.3 * anim);
        self.rect_pipeline.push_rect(
            layout.panel_x + layout.padding,
            sep_y,
            layout.panel_width - layout.padding * 2.0,
            layout.separator_height,
            sep_color,
        );

        // ── Entry list (slice by scroll_offset) ──
        // `info.selected` indexes selectable entries only. Count how many
        // selectables lie before scroll_offset so we can compare correctly.
        let selectables_before_offset = info.entries[..info.scroll_offset.min(info.entries.len())]
            .iter()
            .filter(|e| !e.is_header)
            .count();
        let mut selectable_idx: usize = selectables_before_offset;

        let end = (info.scroll_offset + layout.visible_count).min(info.entries.len());
        for (i, entry) in info.entries[info.scroll_offset..end].iter().enumerate() {
            let ey = layout.list_y + i as f32 * layout.row_height;
            let text_cy = ey + (layout.row_height - cell_height) / 2.0;

            if entry.is_header {
                // ── Category header ──
                let header_sep_y = ey + layout.row_height - 1.0;
                let accent = adjust_alpha(self.ui.palette_accent, 0.5 * anim);
                self.rect_pipeline.push_rect(
                    layout.panel_x + layout.padding,
                    header_sep_y,
                    layout.panel_width - layout.padding * 2.0 - layout.scrollbar_width,
                    1.0,
                    accent,
                );

                let label_color = adjust_alpha(self.ui.palette_accent, 0.85 * anim);
                for (ci, c) in entry.label.chars().enumerate() {
                    let cx = layout.panel_x + layout.padding + ci as f32 * cell_width;
                    if cx + cell_width > layout.panel_x + layout.panel_width - layout.padding {
                        break;
                    }
                    self.text_pipeline.push_char(c, cx, text_cy, label_color);
                }
            } else {
                let is_selected = selectable_idx == info.selected;
                let is_hovered = info.hovered_item == Some(i);

                if is_selected {
                    let sel_bg = adjust_alpha(self.ui.palette_accent, 0.12 * anim);
                    self.rect_pipeline.push_rounded_rect(
                        layout.panel_x + layout.padding * 0.5,
                        ey,
                        layout.panel_width - layout.padding,
                        layout.row_height,
                        sel_bg,
                        4.0,
                    );
                    let accent = adjust_alpha(self.ui.palette_accent, anim);
                    self.rect_pipeline.push_rounded_rect(
                        layout.panel_x + layout.padding * 0.3,
                        ey,
                        3.0,
                        layout.row_height,
                        accent,
                        2.0,
                    );
                } else if is_hovered {
                    let hover_bg = adjust_alpha(self.ui.palette_accent, 0.06 * anim);
                    self.rect_pipeline.push_rounded_rect(
                        layout.panel_x + layout.padding * 0.5,
                        ey,
                        layout.panel_width - layout.padding,
                        layout.row_height,
                        hover_bg,
                        4.0,
                    );
                }

                let label_color = if is_selected {
                    adjust_alpha(self.ui.palette_text, anim)
                } else {
                    adjust_alpha(self.ui.palette_text_dim, 0.85 * anim)
                };
                for (ci, c) in entry.label.chars().enumerate() {
                    let cx = layout.panel_x + layout.padding + cell_width + ci as f32 * cell_width;
                    if cx + cell_width > layout.value_x - cell_width {
                        break;
                    }
                    self.text_pipeline.push_char(c, cx, text_cy, label_color);
                }

                let value_color = if is_selected {
                    adjust_alpha(self.ui.palette_text, anim)
                } else {
                    adjust_alpha(self.ui.palette_text_dim, 0.7 * anim)
                };
                let max_value_x = layout.panel_x + layout.panel_width
                    - layout.padding
                    - layout.scrollbar_width
                    - cell_width;

                match &entry.kind {
                    crate::ui::settings::SettingKind::Numeric { .. } => {
                        if is_selected {
                            let arrow_color = adjust_alpha(self.ui.palette_accent, anim);
                            self.text_pipeline
                                .push_char('◀', layout.value_x, text_cy, arrow_color);
                        }
                        let val_start = layout.value_x + cell_width * 2.0;
                        for (ci, c) in entry.value.chars().enumerate() {
                            let cx = val_start + ci as f32 * cell_width;
                            if cx + cell_width > max_value_x {
                                break;
                            }
                            self.text_pipeline.push_char(c, cx, text_cy, value_color);
                        }
                        if is_selected {
                            let arrow_x =
                                val_start + entry.value.len() as f32 * cell_width + cell_width;
                            if arrow_x + cell_width <= max_value_x {
                                let arrow_color = adjust_alpha(self.ui.palette_accent, anim);
                                self.text_pipeline
                                    .push_char('▶', arrow_x, text_cy, arrow_color);
                            }
                        }
                    }
                    crate::ui::settings::SettingKind::Dropdown { .. }
                    | crate::ui::settings::SettingKind::SteppedList { .. } => {
                        let val_start = layout.value_x + cell_width * 2.0;
                        for (ci, c) in entry.value.chars().enumerate() {
                            let cx = val_start + ci as f32 * cell_width;
                            if cx + cell_width > max_value_x {
                                break;
                            }
                            self.text_pipeline.push_char(c, cx, text_cy, value_color);
                        }
                        let chevron_x =
                            val_start + entry.value.len() as f32 * cell_width + cell_width;
                        if chevron_x + cell_width <= max_value_x {
                            let chevron_color = if is_selected {
                                adjust_alpha(self.ui.palette_accent, anim)
                            } else {
                                adjust_alpha(self.ui.palette_text_dim, 0.5 * anim)
                            };
                            self.text_pipeline
                                .push_char('▼', chevron_x, text_cy, chevron_color);
                        }
                    }
                    crate::ui::settings::SettingKind::ReadOnly => {
                        let val_start = layout.value_x + cell_width * 2.0;
                        let dim_color = adjust_alpha(self.ui.palette_text_dim, 0.6 * anim);
                        for (ci, c) in entry.value.chars().enumerate() {
                            let cx = val_start + ci as f32 * cell_width;
                            if cx + cell_width > max_value_x {
                                self.text_pipeline.push_char('.', cx, text_cy, dim_color);
                                break;
                            }
                            self.text_pipeline.push_char(c, cx, text_cy, dim_color);
                        }
                    }
                }

                selectable_idx += 1;
            }
        }

        // ── Scrollbar ──
        if info.entries.len() > layout.rows_that_fit {
            let sb_x =
                layout.panel_x + layout.panel_width - layout.padding * 0.5 - layout.scrollbar_width;
            let sb_track_h = layout.list_height;
            let total = info.entries.len() as f32;
            let visible = layout.rows_that_fit as f32;
            let thumb_h = (visible / total * sb_track_h).max(20.0);
            let scroll_frac = info.scroll_offset as f32 / (total - visible).max(1.0);
            let thumb_y = layout.list_y + scroll_frac * (sb_track_h - thumb_h);

            let track_color = adjust_alpha(self.ui.palette_border, 0.15 * anim);
            self.rect_pipeline.push_rounded_rect(
                sb_x,
                layout.list_y,
                layout.scrollbar_width,
                sb_track_h,
                track_color,
                2.0,
            );

            let thumb_color = adjust_alpha(self.ui.palette_text_dim, 0.4 * anim);
            self.rect_pipeline.push_rounded_rect(
                sb_x,
                thumb_y,
                layout.scrollbar_width,
                thumb_h,
                thumb_color,
                2.0,
            );
        }

        // ── Footer ──
        let footer_y = layout.panel_y + layout.panel_height - layout.footer_area;
        let footer_sep_color = adjust_alpha(self.ui.palette_border, 0.2 * anim);
        self.rect_pipeline.push_rect(
            layout.panel_x + layout.padding,
            footer_y,
            layout.panel_width - layout.padding * 2.0,
            1.0,
            footer_sep_color,
        );

        let footer_text = "↑↓ Navigate  ←→ Adjust  ⏎ Dropdown  ⌃S Save  Esc Close";
        let footer_text_y = footer_y + (layout.footer_area - cell_height) / 2.0;
        let footer_color = adjust_alpha(self.ui.palette_text_dim, 0.5 * anim);
        let footer_scale = cell_width * 0.85;
        for (i, c) in footer_text.chars().enumerate() {
            let cx = layout.panel_x + layout.padding + i as f32 * footer_scale;
            if cx + cell_width > layout.panel_x + layout.panel_width - layout.padding {
                break;
            }
            self.text_pipeline
                .push_char(c, cx, footer_text_y, footer_color);
        }
    }

    /// Build dropdown overlay render data for settings — rendered last (on top)
    pub(super) fn build_dropdown_overlay(
        &mut self,
        info: &SettingsPanelInfo,
        window_width: f32,
        window_height: f32,
    ) {
        if !info.dropdown_open {
            return;
        }
        let anim = info.animation;
        if anim <= 0.0 {
            return;
        }

        let cell_width = self.cell_width;
        let cell_height = self.cell_height;

        let layout = SettingsModalLayout::calculate(
            window_width,
            window_height,
            cell_width,
            cell_height,
            anim,
            info.entries.len(),
        );

        // Find the selected entry's absolute index.
        let Some(abs_idx) = info
            .entries
            .iter()
            .enumerate()
            .filter(|(_, e)| !e.is_header)
            .nth(info.selected)
            .map(|(i, _)| i)
        else {
            return;
        };
        let entry = &info.entries[abs_idx];

        let (options_text, option_count): (Vec<String>, usize) = match &entry.kind {
            crate::ui::settings::SettingKind::Dropdown { options, .. } => {
                (options.clone(), options.len())
            }
            crate::ui::settings::SettingKind::SteppedList { values, .. } => {
                (values.iter().map(|v| v.to_string()).collect(), values.len())
            }
            _ => return,
        };
        if option_count == 0 {
            return;
        }

        let max_option_len = options_text.iter().map(|s| s.len()).max().unwrap_or(10);
        let Some(rect) =
            layout.dropdown_rect(abs_idx, info.scroll_offset, option_count, max_option_len)
        else {
            return;
        };

        // Shadow
        self.rect_pipeline.push_rounded_rect(
            rect.x + 2.0,
            rect.y + 2.0,
            rect.width,
            rect.height,
            [0.0, 0.0, 0.0, 0.3 * anim],
            6.0,
        );

        // Background (fully opaque so content behind doesn't bleed)
        let dd_bg = [
            self.ui.palette_bg[0],
            self.ui.palette_bg[1],
            self.ui.palette_bg[2],
            1.0,
        ];
        self.rect_pipeline
            .push_rounded_rect(rect.x, rect.y, rect.width, rect.height, dd_bg, 6.0);

        // Border
        let border_color = adjust_alpha(self.ui.palette_border, 0.5 * anim);
        self.rect_pipeline.push_rounded_rect(
            rect.x,
            rect.y,
            rect.width,
            rect.height,
            border_color,
            6.0,
        );
        self.rect_pipeline.push_rounded_rect(
            rect.x + 1.0,
            rect.y + 1.0,
            rect.width - 2.0,
            rect.height - 2.0,
            dd_bg,
            5.0,
        );

        // Render options
        for (oidx, opt_text) in options_text.iter().enumerate().take(rect.visible_options) {
            let oy = rect.y + 4.0 + oidx as f32 * rect.option_height;
            let is_highlighted = oidx == info.dropdown_highlight;

            if is_highlighted {
                let sel = adjust_alpha(self.ui.palette_accent, 0.12 * anim);
                self.rect_pipeline.push_rounded_rect(
                    rect.x + 4.0,
                    oy,
                    rect.width - 8.0,
                    rect.option_height,
                    sel,
                    4.0,
                );
                let accent = adjust_alpha(self.ui.palette_accent, anim);
                self.rect_pipeline.push_rounded_rect(
                    rect.x + 2.0,
                    oy,
                    3.0,
                    rect.option_height,
                    accent,
                    2.0,
                );
            } else if info.dropdown_hovered == Some(oidx) {
                let hover = adjust_alpha(self.ui.palette_accent, 0.06 * anim);
                self.rect_pipeline.push_rounded_rect(
                    rect.x + 4.0,
                    oy,
                    rect.width - 8.0,
                    rect.option_height,
                    hover,
                    4.0,
                );
            }

            let opt_text_y = oy + (rect.option_height - cell_height) / 2.0;
            let opt_color = if is_highlighted {
                adjust_alpha(self.ui.palette_text, anim)
            } else {
                adjust_alpha(self.ui.palette_text_dim, 0.85 * anim)
            };
            for (ci, c) in opt_text.chars().enumerate() {
                let cx = rect.x + cell_width * 1.5 + ci as f32 * cell_width;
                if cx + cell_width > rect.x + rect.width - 8.0 {
                    self.text_pipeline.push_char(
                        '.',
                        cx,
                        opt_text_y,
                        adjust_alpha(self.ui.palette_text_dim, 0.5 * anim),
                    );
                    break;
                }
                self.text_pipeline.push_char(c, cx, opt_text_y, opt_color);
            }
        }
    }

    /// Draw a wavy/curly underline (approximated with small segments)
    pub(super) fn draw_wavy_underline(&mut self, x: f32, y: f32, width: f32, color: [f32; 4]) {
        let wave_period: f32 = 6.0;
        let wave_amplitude: f32 = 2.0;
        let segment_width: f32 = 1.5;
        let thickness: f32 = 1.5;

        let mut current_x = x;
        let mut phase: f32 = 0.0;

        while current_x < x + width {
            let offset_y = (phase * std::f32::consts::TAU).sin() * wave_amplitude;
            self.rect_pipeline.push_rect(
                current_x,
                y + offset_y - wave_amplitude,
                segment_width.min(x + width - current_x),
                thickness,
                color,
            );
            current_x += segment_width;
            phase += segment_width / wave_period;
        }
    }

    /// Draw a dotted underline
    pub(super) fn draw_dotted_underline(&mut self, x: f32, y: f32, width: f32, color: [f32; 4]) {
        let dot_size: f32 = 1.5;
        let gap: f32 = 2.0;
        let step = dot_size + gap;

        let mut current_x = x;
        while current_x < x + width {
            self.rect_pipeline.push_rect(
                current_x,
                y,
                dot_size.min(x + width - current_x),
                1.0,
                color,
            );
            current_x += step;
        }
    }

    /// Draw a dashed underline
    pub(super) fn draw_dashed_underline(&mut self, x: f32, y: f32, width: f32, color: [f32; 4]) {
        let dash_length: f32 = 4.0;
        let gap: f32 = 2.0;
        let step = dash_length + gap;

        let mut current_x = x;
        while current_x < x + width {
            self.rect_pipeline.push_rect(
                current_x,
                y,
                dash_length.min(x + width - current_x),
                1.0,
                color,
            );
            current_x += step;
        }
    }
}
