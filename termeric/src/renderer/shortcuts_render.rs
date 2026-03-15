//! Shortcuts panel rendering — centered modal with search, categories, and key recording

use super::{Renderer, adjust_alpha};
use crate::ui::constants::*;
use crate::ui::shortcuts_panel::ShortcutsPanelInfo;

impl Renderer {
    /// Build shortcuts panel render data — centered modal with scrim
    pub(super) fn build_shortcuts_panel(&mut self, info: &ShortcutsPanelInfo, window_width: f32, window_height: f32) {
        let anim = info.animation;
        let cell_width = self.cell_width;
        let cell_height = self.cell_height;

        // ── Scrim (full-window dark overlay) ──
        let scrim = adjust_alpha(self.ui.palette_scrim, self.ui.palette_scrim[3] * anim);
        self.rect_pipeline.push_rect(0.0, 0.0, window_width, window_height, scrim);

        // ── Panel geometry ──
        let panel_width = (cell_width * 60.0).max(500.0).min(window_width * 0.7);
        let panel_x = (window_width - panel_width) / 2.0;
        let padding = cell_width * 1.5;
        let row_height = cell_height * 2.0;
        let input_height = cell_height * 2.0;
        let footer_height = cell_height * 1.6;
        let gap = 6.0;
        let separator_height = 1.0;
        let header_area = padding + cell_height * 1.4 + gap + input_height + gap + separator_height + gap;
        let footer_area = footer_height + padding * 0.5;
        let max_panel_height = window_height * 0.75;
        let max_list_height = max_panel_height - header_area - footer_area;
        let rows_that_fit = (max_list_height / row_height).floor().max(1.0) as usize;
        let visible_count = info.entries.len().min(rows_that_fit);
        let list_height = visible_count as f32 * row_height;
        let panel_height = header_area + list_height + footer_area;

        // Animate: slide in from top with fade
        let panel_y = (window_height * 0.10) + (1.0 - anim) * -30.0;

        // ── Panel background ──
        let panel_bg = adjust_alpha(self.ui.palette_bg, anim);
        self.rect_pipeline.push_rounded_rect(
            panel_x, panel_y,
            panel_width, panel_height,
            panel_bg, CARD_CORNER_RADIUS,
        );

        // ── Panel border ──
        let border = adjust_alpha(self.ui.palette_border, 0.4 * anim);
        self.rect_pipeline.push_rect(panel_x + CARD_CORNER_RADIUS, panel_y, panel_width - CARD_CORNER_RADIUS * 2.0, BORDER_WIDTH, border);
        self.rect_pipeline.push_rect(panel_x + CARD_CORNER_RADIUS, panel_y + panel_height - BORDER_WIDTH, panel_width - CARD_CORNER_RADIUS * 2.0, BORDER_WIDTH, border);
        self.rect_pipeline.push_rect(panel_x, panel_y + CARD_CORNER_RADIUS, BORDER_WIDTH, panel_height - CARD_CORNER_RADIUS * 2.0, border);
        self.rect_pipeline.push_rect(panel_x + panel_width - BORDER_WIDTH, panel_y + CARD_CORNER_RADIUS, BORDER_WIDTH, panel_height - CARD_CORNER_RADIUS * 2.0, border);

        // ── Title ──
        let title_y = panel_y + padding;
        let title_x = panel_x + padding;
        let title = "Keyboard Shortcuts";
        let title_color = adjust_alpha(self.ui.palette_text, anim);
        let title_scale = cell_width * 1.05;
        for (i, c) in title.chars().enumerate() {
            self.text_pipeline.push_char(c, title_x + i as f32 * title_scale, title_y, title_color);
        }

        // ── Search input ──
        let input_y = title_y + cell_height * 1.4 + gap;
        let input_x = panel_x + padding;
        let input_w = panel_width - padding * 2.0;
        let input_bg = adjust_alpha(self.ui.palette_input_bg, 0.8 * anim);
        self.rect_pipeline.push_rounded_rect(
            input_x, input_y,
            input_w, input_height,
            input_bg, 8.0,
        );

        // Search icon "/"
        let text_y = input_y + (input_height - cell_height) / 2.0;
        let prompt_color = adjust_alpha(self.ui.palette_accent, anim);
        self.text_pipeline.push_char('/', input_x + cell_width * 0.8, text_y, prompt_color);

        // Filter text
        let text_start_x = input_x + cell_width * 2.2;
        let mut text_x = text_start_x;
        let text_color = adjust_alpha(self.ui.palette_text, anim);
        for (byte_idx, c) in info.filter.char_indices() {
            if byte_idx == info.cursor {
                let cursor_color = adjust_alpha(self.ui.palette_accent, 0.9 * anim);
                self.rect_pipeline.push_rect(text_x, text_y, 2.0, cell_height, cursor_color);
            }
            self.text_pipeline.push_char(c, text_x, text_y, text_color);
            text_x += cell_width;
        }
        if info.cursor >= info.filter.len() {
            let cursor_color = adjust_alpha(self.ui.palette_accent, 0.9 * anim);
            self.rect_pipeline.push_rect(text_x, text_y, 2.0, cell_height, cursor_color);
        }

        // Placeholder
        if info.filter.is_empty() {
            let hint = "Filter shortcuts...";
            let hint_color = adjust_alpha(self.ui.palette_text_dim, 0.6 * anim);
            let mut hx = text_start_x;
            for c in hint.chars() {
                self.text_pipeline.push_char(c, hx, text_y, hint_color);
                hx += cell_width;
            }
        }

        // ── Separator ──
        let sep_y = input_y + input_height + gap;
        let sep_color = adjust_alpha(self.ui.palette_border, 0.3 * anim);
        self.rect_pipeline.push_rect(
            panel_x + padding, sep_y,
            panel_width - padding * 2.0, separator_height,
            sep_color,
        );

        // ── Entry list ──
        let list_y = sep_y + separator_height + gap;
        let scrollbar_width = 4.0;
        let binding_right = panel_x + panel_width - padding - scrollbar_width - cell_width * 0.5;

        // Track the selectable index for highlighting
        let mut selectable_idx: usize = 0;

        for (row_idx, entry) in info.entries.iter().enumerate() {
            let entry_y = list_y + row_idx as f32 * row_height;

            // Clip past footer
            if entry_y + row_height > panel_y + panel_height - footer_height {
                break;
            }

            // Category header
            if entry.is_header {
                let header_y_text = entry_y + (row_height - cell_height) / 2.0;
                let header_color = adjust_alpha(self.ui.palette_accent, 0.7 * anim);
                let header_scale = cell_width * 0.85;
                for (i, c) in entry.label.chars().enumerate() {
                    self.text_pipeline.push_char(c, panel_x + padding + i as f32 * header_scale, header_y_text, header_color);
                }
                // Thin separator after header label
                let sep_after_x = panel_x + padding + entry.label.chars().count() as f32 * header_scale + cell_width * 0.5;
                let sep_after_w = panel_x + panel_width - padding - sep_after_x;
                if sep_after_w > 0.0 {
                    let sep_line_color = adjust_alpha(self.ui.palette_border, 0.15 * anim);
                    self.rect_pipeline.push_rect(sep_after_x, header_y_text + cell_height * 0.5, sep_after_w, 1.0, sep_line_color);
                }
                continue;
            }

            let is_selected = selectable_idx == info.selected;
            selectable_idx += 1;

            // ── Row background ──
            if is_selected {
                let sel_bg = adjust_alpha(self.ui.palette_accent, 0.12 * anim);
                self.rect_pipeline.push_rounded_rect(
                    panel_x + padding * 0.5, entry_y,
                    panel_width - padding, row_height,
                    sel_bg, 6.0,
                );
                // Left accent border
                let accent = adjust_alpha(self.ui.palette_accent, anim);
                self.rect_pipeline.push_rounded_rect(
                    panel_x + padding * 0.5, entry_y + 4.0,
                    3.0, row_height - 8.0,
                    accent, 2.0,
                );
            }

            let entry_text_y = entry_y + (row_height - cell_height) / 2.0;

            // ── Action label ──
            let label_color = if is_selected {
                adjust_alpha(self.ui.palette_text, anim)
            } else {
                adjust_alpha(self.ui.palette_text, 0.85 * anim)
            };
            let label_x = panel_x + padding + cell_width * 0.5;
            for (i, c) in entry.label.chars().enumerate() {
                self.text_pipeline.push_char(c, label_x + i as f32 * cell_width, entry_text_y, label_color);
            }

            // ── Binding keycap (right-aligned) ──
            if is_selected && info.recording {
                // Pulsing "Press a key..." text
                let elapsed = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs_f32();
                let pulse = (elapsed * 4.0).sin() * 0.3 + 0.7;
                let recording_color = adjust_alpha(self.ui.palette_accent, pulse * anim);
                let text = "Press a key...";
                let text_w = text.chars().count() as f32 * cell_width * 0.85;
                let rx = binding_right - text_w;
                for (i, c) in text.chars().enumerate() {
                    self.text_pipeline.push_char(c, rx + i as f32 * cell_width * 0.85, entry_text_y, recording_color);
                }
            } else if entry.binding == "Unset" {
                // Dim "Unset"
                let unset_color = adjust_alpha(self.ui.palette_text_dim, 0.4 * anim);
                let unset_w = 5.0 * cell_width * 0.8;
                let ux = binding_right - unset_w;
                for (i, c) in "Unset".chars().enumerate() {
                    self.text_pipeline.push_char(c, ux + i as f32 * cell_width * 0.8, entry_text_y, unset_color);
                }
            } else {
                // Keycap-style binding display (like command palette shortcuts)
                let parts: Vec<&str> = entry.binding.split('+').collect();
                let mut key_right = binding_right;

                for part in parts.iter().rev() {
                    let part = part.trim();
                    let char_count = part.chars().count();
                    let keycap_text_w = char_count as f32 * cell_width * 0.75;
                    let keycap_w = keycap_text_w + cell_width * 0.8;
                    let keycap_h = cell_height * 1.15;
                    let keycap_x = key_right - keycap_w;
                    let keycap_y = entry_text_y + (cell_height - keycap_h) / 2.0;

                    let kc_bg = adjust_alpha(self.ui.palette_keycap_bg, anim);
                    self.rect_pipeline.push_rounded_rect(
                        keycap_x, keycap_y,
                        keycap_w, keycap_h,
                        kc_bg, 4.0,
                    );

                    let kc_text_color = adjust_alpha(self.ui.palette_keycap_text, anim);
                    let kc_text_x = keycap_x + (keycap_w - keycap_text_w) / 2.0;
                    let kc_text_y = keycap_y + (keycap_h - cell_height) / 2.0;
                    for (i, c) in part.chars().enumerate() {
                        self.text_pipeline.push_char(c, kc_text_x + i as f32 * cell_width * 0.75, kc_text_y, kc_text_color);
                    }

                    key_right = keycap_x - cell_width * 0.3;
                }
            }
        }

        // ── Scrollbar ──
        let selectable_total = info.total_entries;
        if selectable_total > rows_that_fit && rows_that_fit > 0 {
            let track_x = panel_x + panel_width - padding * 0.5 - scrollbar_width;
            let track_y = list_y;
            let track_h = list_height;

            let track_color = adjust_alpha(self.ui.palette_border, 0.15 * anim);
            self.rect_pipeline.push_rounded_rect(
                track_x, track_y,
                scrollbar_width, track_h,
                track_color, 2.0,
            );

            let thumb_frac = rows_that_fit as f32 / selectable_total as f32;
            let thumb_h = (track_h * thumb_frac).max(cell_height);
            let max_scroll = selectable_total.saturating_sub(rows_that_fit);
            let scroll_frac = if max_scroll > 0 {
                info.scroll_offset as f32 / max_scroll as f32
            } else {
                0.0
            };
            let thumb_y = track_y + scroll_frac * (track_h - thumb_h);
            let thumb_color = adjust_alpha(self.ui.palette_accent, 0.5 * anim);
            self.rect_pipeline.push_rounded_rect(
                track_x, thumb_y,
                scrollbar_width, thumb_h,
                thumb_color, 2.0,
            );
        }

        // ── Footer ──
        let footer_y = panel_y + panel_height - footer_height;
        let footer_sep = adjust_alpha(self.ui.palette_border, 0.2 * anim);
        self.rect_pipeline.push_rect(
            panel_x + padding, footer_y,
            panel_width - padding * 2.0, 1.0,
            footer_sep,
        );

        let footer_text_y = footer_y + (footer_height - cell_height) / 2.0;

        // Left: count
        let footer_text = if info.total_entries < BindableAction::all().len() {
            format!("{} of {} shortcuts", info.total_entries, BindableAction::all().len())
        } else {
            format!("{} shortcuts", info.total_entries)
        };
        let footer_color = adjust_alpha(self.ui.palette_footer, anim);
        let footer_text_x = panel_x + padding;
        for (i, c) in footer_text.chars().enumerate() {
            self.text_pipeline.push_char(c, footer_text_x + i as f32 * cell_width * 0.85, footer_text_y, footer_color);
        }

        // Right: hints
        let hint_parts = if info.recording {
            &[("Key", "Bind"), ("Del", "Clear"), ("Esc", "Cancel")][..]
        } else {
            &[("↑↓", "Navigate"), ("Enter", "Record"), ("Del", "Clear"), ("Esc", "Close")][..]
        };
        let mut hint_x = panel_x + panel_width - padding;
        let hint_color = adjust_alpha(self.ui.palette_footer, 0.6 * anim);
        let hint_scale = cell_width * 0.75;

        for (key, label) in hint_parts.iter().rev() {
            let label_w = label.chars().count() as f32 * hint_scale;
            hint_x -= label_w;
            for (i, c) in label.chars().enumerate() {
                self.text_pipeline.push_char(c, hint_x + i as f32 * hint_scale, footer_text_y, hint_color);
            }
            hint_x -= cell_width * 0.3;
            let key_w = key.chars().count() as f32 * hint_scale;
            hint_x -= key_w;
            let key_color = adjust_alpha(self.ui.palette_text_dim, 0.5 * anim);
            for (i, c) in key.chars().enumerate() {
                self.text_pipeline.push_char(c, hint_x + i as f32 * hint_scale, footer_text_y, key_color);
            }
            hint_x -= cell_width * 1.0;
        }

        // ── Conflict warning (below footer, overlapping panel border) ──
        if let Some(ref conflict) = info.conflict_label {
            let warning = format!("Reassigned from: {}", conflict);
            let warn_y = footer_y - cell_height * 1.2;
            let warn_color = adjust_alpha(self.ui.palette_accent, 0.6 * anim);
            let warn_scale = cell_width * 0.8;
            let warn_x = panel_x + padding;
            for (i, c) in warning.chars().enumerate() {
                self.text_pipeline.push_char(c, warn_x + i as f32 * warn_scale, warn_y, warn_color);
            }
        }
    }
}

use crate::input::keybindings::BindableAction;
