//! Command palette rendering — centered modal with search and results list

use super::{Renderer, adjust_alpha};
use crate::ui::command_palette::{PaletteInfo, PaletteLayout};
use crate::ui::constants::*;

impl Renderer {
    /// Build command palette render data — centered modal with scrim
    pub(super) fn build_command_palette(
        &mut self,
        info: &PaletteInfo,
        window_width: f32,
        window_height: f32,
    ) {
        let anim = info.animation;
        let cell_width = self.cell_width;
        let cell_height = self.cell_height;

        // ── Scrim (full-window dark overlay) ──
        let scrim = adjust_alpha(self.ui.palette_scrim, self.ui.palette_scrim[3] * anim);
        self.rect_pipeline
            .push_rect(0.0, 0.0, window_width, window_height, scrim);

        // ── Geometry via the shared layout struct ──
        let layout = PaletteLayout::calculate(
            window_width,
            window_height,
            cell_width,
            cell_height,
            anim,
            info.entries.len(),
        );
        let panel_x = layout.panel_x;
        let panel_y = layout.panel_y;
        let panel_width = layout.panel_width;
        let panel_height = layout.panel_height;
        let padding = layout.padding;
        let row_height = layout.row_height;
        let input_height = layout.input_height;
        let footer_height = layout.footer_height;
        let gap = layout.gap;
        let separator_height = layout.separator_height;
        let list_height = layout.list_height;
        let visible_count = layout.visible_count;

        // ── Panel background (rounded) ──
        let panel_bg = adjust_alpha(self.ui.palette_bg, anim);
        self.rect_pipeline.push_rounded_rect(
            panel_x,
            panel_y,
            panel_width,
            panel_height,
            panel_bg,
            CARD_CORNER_RADIUS,
        );

        // ── Panel border ──
        let border = adjust_alpha(self.ui.palette_border, 0.4 * anim);
        self.rect_pipeline.push_rect(
            panel_x + CARD_CORNER_RADIUS,
            panel_y,
            panel_width - CARD_CORNER_RADIUS * 2.0,
            BORDER_WIDTH,
            border,
        );
        self.rect_pipeline.push_rect(
            panel_x + CARD_CORNER_RADIUS,
            panel_y + panel_height - BORDER_WIDTH,
            panel_width - CARD_CORNER_RADIUS * 2.0,
            BORDER_WIDTH,
            border,
        );
        self.rect_pipeline.push_rect(
            panel_x,
            panel_y + CARD_CORNER_RADIUS,
            BORDER_WIDTH,
            panel_height - CARD_CORNER_RADIUS * 2.0,
            border,
        );
        self.rect_pipeline.push_rect(
            panel_x + panel_width - BORDER_WIDTH,
            panel_y + CARD_CORNER_RADIUS,
            BORDER_WIDTH,
            panel_height - CARD_CORNER_RADIUS * 2.0,
            border,
        );

        // ── Search input area ──
        let input_y = panel_y + padding;
        let input_x = panel_x + padding;
        let input_w = panel_width - padding * 2.0;
        let input_bg = adjust_alpha(self.ui.palette_input_bg, 0.8 * anim);
        self.rect_pipeline.push_rounded_rect(
            input_x,
            input_y,
            input_w,
            input_height,
            input_bg,
            8.0,
        );

        // Prompt ">"
        let text_y = input_y + (input_height - cell_height) / 2.0;
        let prompt_color = adjust_alpha(self.ui.palette_accent, anim);
        self.text_pipeline
            .push_char('>', input_x + cell_width * 0.8, text_y, prompt_color);

        // Input text
        let text_start_x = input_x + cell_width * 2.2;
        let mut text_x = text_start_x;
        let text_color = adjust_alpha(self.ui.palette_text, anim);
        for (byte_idx, c) in info.input.char_indices() {
            if byte_idx == info.cursor {
                let cursor_color = adjust_alpha(self.ui.palette_accent, 0.9 * anim);
                self.rect_pipeline
                    .push_rect(text_x, text_y, 2.0, cell_height, cursor_color);
            }
            self.text_pipeline.push_char(c, text_x, text_y, text_color);
            text_x += cell_width;
        }
        if info.cursor >= info.input.len() {
            let cursor_color = adjust_alpha(self.ui.palette_accent, 0.9 * anim);
            self.rect_pipeline
                .push_rect(text_x, text_y, 2.0, cell_height, cursor_color);
        }

        // Placeholder when empty
        if info.input.is_empty() {
            let hint = &info.placeholder;
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
            panel_x + padding,
            sep_y,
            panel_width - padding * 2.0,
            separator_height,
            sep_color,
        );

        // ── Result list ──
        // info.entries is already sliced to only visible entries by build_info()
        let list_y = sep_y + separator_height + gap;
        let title_x = panel_x + padding + cell_width * 0.5;
        let scrollbar_width = 4.0;
        let shortcut_right = panel_x + panel_width - padding - scrollbar_width - cell_width * 0.5;

        for (row_idx, entry) in info.entries.iter().enumerate() {
            let entry_y = list_y + row_idx as f32 * row_height;

            // Clip: skip entries that would render past the footer
            if entry_y + row_height > panel_y + panel_height - footer_height {
                break;
            }

            let is_selected = row_idx == info.selected;

            // ── Row background ──
            if is_selected {
                let sel_bg = adjust_alpha(self.ui.palette_accent, 0.12 * anim);
                self.rect_pipeline.push_rounded_rect(
                    panel_x + padding * 0.5,
                    entry_y,
                    panel_width - padding,
                    row_height,
                    sel_bg,
                    6.0,
                );
                // Left accent border
                let accent = adjust_alpha(self.ui.palette_accent, anim);
                self.rect_pipeline.push_rounded_rect(
                    panel_x + padding * 0.5,
                    entry_y + 4.0,
                    3.0,
                    row_height - 8.0,
                    accent,
                    2.0,
                );
            }

            // Destructive row — subtle left border accent
            if entry.destructive {
                let mut destruct_color = self.ui.palette_destructive;
                destruct_color[3] = 0.45 * anim;
                // Left border
                self.rect_pipeline.push_rounded_rect(
                    panel_x + padding * 0.5,
                    entry_y + 3.0,
                    2.0,
                    row_height - 6.0,
                    destruct_color,
                    1.0,
                );
            }

            let entry_text_y = entry_y + (row_height - cell_height) / 2.0;

            // ── Title with fuzzy match highlighting ──
            let title_color = if is_selected {
                adjust_alpha(self.ui.palette_text, anim)
            } else {
                adjust_alpha(self.ui.palette_text, 0.85 * anim)
            };
            let highlight_color = adjust_alpha(self.ui.palette_match_highlight, anim);

            let mut tx = title_x;
            // Draw icon character before label if present
            if let Some(icon) = entry.icon {
                self.text_pipeline
                    .push_char(icon, tx, entry_text_y, title_color);
                tx += cell_width * 1.5;
            }
            for (char_idx, c) in entry.title.chars().enumerate() {
                let color = if entry.match_positions.contains(&char_idx) {
                    highlight_color
                } else {
                    title_color
                };
                self.text_pipeline.push_char(c, tx, entry_text_y, color);
                tx += cell_width;
            }

            // ── Category tag (dimmed, after title) ──
            let cat_color = adjust_alpha(self.ui.palette_category, 0.5 * anim);
            let cat_x = tx + cell_width * 1.0;
            let cat_scale = cell_width * 0.85;
            for (ci, c) in entry.category.chars().enumerate() {
                self.text_pipeline.push_char(
                    c,
                    cat_x + ci as f32 * cat_scale,
                    entry_text_y,
                    cat_color,
                );
            }

            // ── Shortcut keycaps (right-aligned) ──
            if let Some(shortcut) = &entry.shortcut {
                let parts: Vec<&str> = shortcut.split('+').collect();
                let mut key_right = shortcut_right;

                for part in parts.iter().rev() {
                    let part = part.trim();
                    let char_count = part.chars().count();
                    let keycap_text_w = char_count as f32 * cell_width * 0.75;
                    let keycap_w = keycap_text_w + cell_width * 0.8;
                    let keycap_h = cell_height * 1.15;
                    let keycap_x = key_right - keycap_w;
                    let keycap_y = entry_text_y + (cell_height - keycap_h) / 2.0;

                    let kc_bg = adjust_alpha(self.ui.palette_keycap_bg, anim);
                    self.rect_pipeline
                        .push_rounded_rect(keycap_x, keycap_y, keycap_w, keycap_h, kc_bg, 4.0);

                    let kc_text_color = adjust_alpha(self.ui.palette_keycap_text, anim);
                    let kc_text_x = keycap_x + (keycap_w - keycap_text_w) / 2.0;
                    let kc_text_y = keycap_y + (keycap_h - cell_height) / 2.0;
                    for (i, c) in part.chars().enumerate() {
                        self.text_pipeline.push_char(
                            c,
                            kc_text_x + i as f32 * cell_width * 0.75,
                            kc_text_y,
                            kc_text_color,
                        );
                    }

                    key_right = keycap_x - cell_width * 0.3;
                }
            }
        }

        // ── Scrollbar (when more entries than visible) ──
        if info.total_entries > visible_count && visible_count > 0 {
            let track_x = panel_x + panel_width - padding * 0.5 - scrollbar_width;
            let track_y = list_y;
            let track_h = list_height;

            // Track background
            let track_color = adjust_alpha(self.ui.palette_border, 0.15 * anim);
            self.rect_pipeline.push_rounded_rect(
                track_x,
                track_y,
                scrollbar_width,
                track_h,
                track_color,
                2.0,
            );

            // Thumb
            let thumb_frac = visible_count as f32 / info.total_entries as f32;
            let thumb_h = (track_h * thumb_frac).max(cell_height);
            let max_scroll = info.total_entries.saturating_sub(visible_count);
            let scroll_frac = if max_scroll > 0 {
                info.scroll_offset as f32 / max_scroll as f32
            } else {
                0.0
            };
            let thumb_y = track_y + scroll_frac * (track_h - thumb_h);
            let thumb_color = adjust_alpha(self.ui.palette_accent, 0.5 * anim);
            self.rect_pipeline.push_rounded_rect(
                track_x,
                thumb_y,
                scrollbar_width,
                thumb_h,
                thumb_color,
                2.0,
            );
        }

        // ── Footer ──
        let footer_y = panel_y + panel_height - footer_height;
        let footer_sep = adjust_alpha(self.ui.palette_border, 0.2 * anim);
        self.rect_pipeline.push_rect(
            panel_x + padding,
            footer_y,
            panel_width - padding * 2.0,
            1.0,
            footer_sep,
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
        let footer_color = adjust_alpha(self.ui.palette_footer, anim);
        let footer_text_y = footer_y + (footer_height - cell_height) / 2.0;
        let footer_text_x = panel_x + padding;
        let footer_capacity =
            ((panel_width - padding * 2.0) / (cell_width * 0.85)).max(0.0) as usize;
        for (i, c) in footer_text.chars().take(footer_capacity).enumerate() {
            self.text_pipeline.push_char(
                c,
                footer_text_x + i as f32 * cell_width * 0.85,
                footer_text_y,
                footer_color,
            );
        }

        // Pending/error status owns the footer; selection hints would overlap
        // it and advertise actions that are unavailable while there are no rows.
        if info.status.is_some() {
            return;
        }

        // Hint on right side of footer
        let hint_parts = [
            ("Up", "↑"),
            ("Down", "↓"),
            ("Enter", "Select"),
            ("Esc", info.escape_label.as_str()),
        ];
        let mut hint_x = panel_x + panel_width - padding;
        let hint_color = adjust_alpha(self.ui.palette_footer, 0.6 * anim);
        let hint_scale = cell_width * 0.75;
        // Draw hints right-to-left
        for (key, label) in hint_parts.iter().rev() {
            // Label
            let label_w = label.chars().count() as f32 * hint_scale;
            hint_x -= label_w;
            for (i, c) in label.chars().enumerate() {
                self.text_pipeline.push_char(
                    c,
                    hint_x + i as f32 * hint_scale,
                    footer_text_y,
                    hint_color,
                );
            }
            // Key
            hint_x -= cell_width * 0.3;
            let key_w = key.chars().count() as f32 * hint_scale;
            hint_x -= key_w;
            let key_color = adjust_alpha(self.ui.palette_text_dim, 0.5 * anim);
            for (i, c) in key.chars().enumerate() {
                self.text_pipeline.push_char(
                    c,
                    hint_x + i as f32 * hint_scale,
                    footer_text_y,
                    key_color,
                );
            }
            hint_x -= cell_width * 1.0; // gap between hint groups
        }
    }
}
