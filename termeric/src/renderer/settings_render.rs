//! Settings panel rendering

use super::{Renderer, adjust_alpha};
use crate::ui::settings::SettingsPanelInfo;

impl Renderer {
    pub(super) fn build_settings_panel(&mut self, info: &SettingsPanelInfo, window_width: f32, window_height: f32) {
        let anim = info.animation;
        if anim <= 0.0 {
            return;
        }

        // UI character width — use actual cell width from font metrics.
        // This ensures proper character spacing regardless of font.
        let ui_cw = self.cell_width;

        // Panel dimensions — all derived from font metrics so layout adapts
        // Sidebar: padding + icon(3 cells) + longest label("Scrollback"=10) + padding = ~16.5 cells
        let padding_x = ui_cw * 1.5;
        let sidebar_width = padding_x + ui_cw * 3.0 + ui_cw * 10.0 + padding_x;
        // Content: label area + gap + value area + padding
        let min_content_width = ui_cw * 30.0; // needs room for labels + values
        let panel_width = ((sidebar_width + min_content_width).max(window_width * 0.35)).min(window_width * 0.55);
        let panel_x = window_width - panel_width * anim; // slide in from right
        let panel_y = 0.0;
        let panel_h = window_height;

        // Content area is whatever remains after sidebar
        let content_x = panel_x + sidebar_width;
        let content_width = panel_width - sidebar_width;

        let row_height = (self.cell_height * 2.2).max(32.0);
        let padding_y = self.cell_height * 0.5;

        // ── Scrim (warm-tinted dimmed overlay on terminal behind panel) ──
        let scrim = self.ui.scrim;
        self.rect_pipeline.push_rect(
            0.0, 0.0,
            panel_x, window_height,
            [scrim[0], scrim[1], scrim[2], scrim[3] * anim],
        );

        // ── Sidebar background ──
        self.rect_pipeline.push_rect(
            panel_x, panel_y,
            sidebar_width, panel_h,
            self.ui.settings_sidebar_bg,
        );

        // ── Main panel background ──
        self.rect_pipeline.push_rect(
            content_x, panel_y,
            content_width, panel_h,
            self.ui.settings_bg,
        );

        // ── Sidebar border (thin line between sidebar and content) ──
        self.rect_pipeline.push_rect(
            content_x - 1.0, panel_y,
            1.0, panel_h,
            self.ui.settings_separator,
        );

        // ── Panel title in sidebar ──
        let title = "Settings";
        let title_y = panel_y + self.cell_height * 1.5;
        let title_x = panel_x + padding_x;
        for (i, c) in title.chars().enumerate() {
            self.text_pipeline.push_char(
                c,
                title_x + i as f32 * ui_cw,
                title_y,
                self.ui.settings_accent,
            );
        }

        // ── Section buttons in sidebar ──
        let section_start_y = title_y + self.cell_height * 2.5;
        for (idx, (section, is_active)) in info.sections.iter().enumerate() {
            let sy = section_start_y + idx as f32 * row_height;

            // Active section highlight bar
            if *is_active {
                // Left accent bar
                self.rect_pipeline.push_rounded_rect(
                    panel_x, sy,
                    3.0, row_height,
                    self.ui.settings_accent,
                    2.0,
                );
                // Highlight background
                self.rect_pipeline.push_rounded_rect(
                    panel_x + 3.0, sy,
                    sidebar_width - 3.0, row_height,
                    self.ui.settings_selected,
                    4.0,
                );
            } else if info.hovered_section == Some(idx) {
                // Subtle hover highlight for non-active sections
                let hover_color = [
                    self.ui.settings_selected[0],
                    self.ui.settings_selected[1],
                    self.ui.settings_selected[2],
                    self.ui.settings_selected[3] * 0.4,
                ];
                self.rect_pipeline.push_rounded_rect(
                    panel_x + 3.0, sy,
                    sidebar_width - 3.0, row_height,
                    hover_color,
                    4.0,
                );
            }

            // Section label
            let label_x = panel_x + padding_x;
            let text_y = sy + (row_height - self.cell_height) / 2.0;
            let color = if *is_active { self.ui.settings_text } else { self.ui.settings_text_dim };
            for (i, c) in section.label().chars().enumerate() {
                self.text_pipeline.push_char(
                    c,
                    label_x + i as f32 * ui_cw,
                    text_y,
                    color,
                );
            }
        }

        // ── Keyboard hints at bottom of sidebar ──
        let hints = ["↑↓ Navigate", "←→ Adjust", "Tab Section", "Esc Close"];
        let hint_start_y = window_height - (hints.len() as f32 * self.cell_height * 1.6) - self.cell_height;
        for (idx, hint) in hints.iter().enumerate() {
            let hy = hint_start_y + idx as f32 * self.cell_height * 1.6;
            let hx = panel_x + padding_x;
            for (i, c) in hint.chars().enumerate() {
                if hx + i as f32 * ui_cw * 0.85 < content_x - padding_x {
                    self.text_pipeline.push_char(
                        c,
                        hx + i as f32 * ui_cw * 0.85,
                        hy,
                        self.ui.settings_text_dim,
                    );
                }
            }
        }

        // ── Content area: section header ──
        let header_y = panel_y + self.cell_height * 1.5;
        let header_x = content_x + padding_x;
        let section_title = info.section.label();
        for (i, c) in section_title.chars().enumerate() {
            self.text_pipeline.push_char(
                c,
                header_x + i as f32 * ui_cw * 1.1,
                header_y,
                self.ui.settings_header,
            );
        }

        // Separator under header
        let sep_y = header_y + self.cell_height * 1.8;
        self.rect_pipeline.push_rect(
            content_x + padding_x, sep_y,
            content_width - padding_x * 2.0, 1.0,
            self.ui.settings_separator,
        );

        // ── Content area: setting items ──
        let items_start_y = sep_y + padding_y + self.cell_height * 0.5;

        // Dynamic value-area X: positioned after the longest label + a gap,
        // clamped so it never exceeds 65% of content width.
        let max_label_len = info.items.iter()
            .map(|item| item.label.len())
            .max()
            .unwrap_or(0);
        let label_end = padding_x + max_label_len as f32 * ui_cw;
        let min_gap = ui_cw * 2.0;
        let max_value_area = content_width * 0.65;
        let value_area_offset = (label_end + min_gap).min(max_value_area);
        let value_area_x = content_x + value_area_offset;

        // Calculate dropdown occlusion region (if dropdown is open, items below
        // the selected item may be hidden behind the dropdown overlay).
        // Since all text renders on top of all rects, we must skip rendering
        // text for occluded items to avoid text-on-text confusion.
        // (dd_x, dd_top, dd_bottom) — X is the left edge of the dropdown
        let dropdown_occludes: Option<(f32, f32, f32)> = if info.dropdown_open {
            if let Some(sel_item) = info.items.get(info.selected_item) {
                let (options_text, option_count) = match &sel_item.kind {
                    crate::ui::settings::SettingKind::Dropdown { options, .. } => (options.clone(), options.len()),
                    crate::ui::settings::SettingKind::SteppedList { values, .. } => {
                        (values.iter().map(|v| v.to_string()).collect::<Vec<_>>(), values.len())
                    }
                    _ => (vec![], 0),
                };
                if option_count > 0 {
                    // Match the exact dropdown position calculation from build_dropdown_overlay
                    let max_option_len = options_text.iter().map(|s| s.len()).max().unwrap_or(10);
                    let min_dd_width = (max_option_len as f32 + 4.0) * ui_cw;
                    let remaining_space = content_width - value_area_offset;
                    let dd_width = min_dd_width.max(remaining_space).min(content_width - ui_cw * 2.0);

                    // Shift dropdown left if it would exceed panel bounds
                    let dd_value_area_x = content_x + value_area_offset;
                    let dd_x = if dd_value_area_x + dd_width > panel_x + panel_width - ui_cw {
                        (panel_x + panel_width - dd_width - ui_cw).max(content_x + ui_cw)
                    } else {
                        dd_value_area_x
                    };

                    let visible_count = option_count.min(8);
                    let option_h = row_height * 0.85;
                    let dd_top = items_start_y + info.selected_item as f32 * row_height + row_height;
                    let dd_bottom = dd_top + visible_count as f32 * option_h + 8.0;
                    Some((dd_x, dd_top, dd_bottom))
                } else {
                    None
                }
            } else {
                None
            }
        } else {
            None
        };

        for (idx, item) in info.items.iter().enumerate() {
            let iy = items_start_y + idx as f32 * row_height;
            let is_selected = idx == info.selected_item;

            // Check if this item's text would be occluded by the dropdown
            let is_occluded = if let Some((_dd_x, dd_top, dd_bottom)) = dropdown_occludes {
                let item_top = iy;
                let item_bottom = iy + row_height;
                // Item is occluded if it overlaps the dropdown region
                item_top < dd_bottom && item_bottom > dd_top
            } else {
                false
            };

            // Selection highlight (keyboard-selected)
            if is_selected {
                self.rect_pipeline.push_rounded_rect(
                    content_x + padding_x * 0.5, iy,
                    content_width - padding_x, row_height,
                    self.ui.settings_selected,
                    4.0,
                );
                // Left accent bar for selected item
                self.rect_pipeline.push_rounded_rect(
                    content_x, iy,
                    3.0, row_height,
                    self.ui.settings_accent,
                    2.0,
                );
            } else if info.hovered_item == Some(idx) {
                // Subtle hover highlight for non-selected items
                let hover_color = [
                    self.ui.settings_selected[0],
                    self.ui.settings_selected[1],
                    self.ui.settings_selected[2],
                    self.ui.settings_selected[3] * 0.4,
                ];
                self.rect_pipeline.push_rounded_rect(
                    content_x + padding_x * 0.5, iy,
                    content_width - padding_x, row_height,
                    hover_color,
                    4.0,
                );
            }

            // If occluded by dropdown, still draw labels but skip value text below

            let text_y = iy + (row_height - self.cell_height) / 2.0;

            // Label — clip at dropdown boundary if occluded
            let label_x = content_x + padding_x;
            let label_color = if is_selected { self.ui.settings_text } else { self.ui.settings_text_dim };
            let label_clip_x = if is_occluded {
                if let Some((dd_x, _, _)) = dropdown_occludes { dd_x } else { f32::MAX }
            } else {
                f32::MAX
            };
            for (i, c) in item.label.chars().enumerate() {
                let cx = label_x + i as f32 * ui_cw;
                if cx + ui_cw > label_clip_x {
                    break; // Don't render characters that extend into dropdown
                }
                self.text_pipeline.push_char(c, cx, text_y, label_color);
            }

            // Value (right-aligned area) — skip if occluded by dropdown
            if !is_occluded {
                // value_area_x already computed above from longest label
                let value_color = if is_selected { self.ui.settings_value } else { self.ui.settings_text };
                let max_x = content_x + content_width - padding_x;

                match &item.kind {
                    crate::ui::settings::SettingKind::Numeric { .. } => {
                        // Draw ◀ value ▶ controls for numeric
                        if is_selected {
                            self.text_pipeline.push_char(
                                '◀',
                                value_area_x,
                                text_y,
                                self.ui.settings_accent,
                            );
                        }

                        let val_x = value_area_x + ui_cw * 2.0;
                        for (i, c) in item.value.chars().enumerate() {
                            let cx = val_x + i as f32 * ui_cw;
                            if cx + ui_cw > max_x { break; }
                            self.text_pipeline.push_char(c, cx, text_y, value_color);
                        }

                        if is_selected {
                            let arrow_x = val_x + item.value.len() as f32 * ui_cw + ui_cw;
                            if arrow_x + ui_cw <= max_x {
                                self.text_pipeline.push_char('▶', arrow_x, text_y, self.ui.settings_accent);
                            }
                        }
                    }
                    crate::ui::settings::SettingKind::Dropdown { .. } | crate::ui::settings::SettingKind::SteppedList { .. } => {
                        // Draw value with ▼ chevron (click or Enter to open dropdown)
                        let val_x = value_area_x + ui_cw * 2.0;
                        for (i, c) in item.value.chars().enumerate() {
                            let cx = val_x + i as f32 * ui_cw;
                            if cx + ui_cw > max_x { break; }
                            self.text_pipeline.push_char(c, cx, text_y, value_color);
                        }

                        // Down chevron — only if it fits
                        let chevron_x = val_x + item.value.len() as f32 * ui_cw + ui_cw;
                        if chevron_x + ui_cw <= max_x {
                            let chevron_color = if is_selected { self.ui.settings_accent } else { self.ui.settings_text_dim };
                            self.text_pipeline.push_char('▼', chevron_x, text_y, chevron_color);
                        }
                    }
                    crate::ui::settings::SettingKind::ReadOnly => {
                        // Just display the value, truncated with '.'
                        let val_x = value_area_x + ui_cw * 2.0;
                        for (i, c) in item.value.chars().enumerate() {
                            let cx = val_x + i as f32 * ui_cw;
                            if cx + ui_cw > max_x {
                                self.text_pipeline.push_char('.', cx, text_y, self.ui.settings_text_dim);
                                break;
                            }
                            self.text_pipeline.push_char(
                                c, cx, text_y,
                                self.ui.settings_text_dim,
                            );
                        }
                    }
                }
            }

            // Thin separator between items
            if idx + 1 < info.items.len() {
                self.rect_pipeline.push_rect(
                    content_x + padding_x, iy + row_height - 0.5,
                    content_width - padding_x * 2.0, 0.5,
                    self.ui.settings_separator,
                );
            }
        }

        // ── Footer: Save hint ──
        let footer_y = window_height - self.cell_height * 2.0;
        let footer_text = "Ctrl+S Save  |  Ctrl+R Reset";
        let footer_x = content_x + padding_x;
        // Separator above footer
        self.rect_pipeline.push_rect(
            content_x + padding_x, footer_y - padding_y,
            content_width - padding_x * 2.0, 1.0,
            self.ui.settings_separator,
        );

        // Skip footer text if occluded by dropdown
        let footer_occluded = if let Some((_, _dd_top, dd_bottom)) = dropdown_occludes {
            footer_y < dd_bottom
        } else {
            false
        };
        if !footer_occluded {
            let footer_max_x = content_x + content_width - padding_x;
            for (i, c) in footer_text.chars().enumerate() {
                let cx = footer_x + i as f32 * ui_cw * 0.9;
                if cx + ui_cw > footer_max_x { break; }
                self.text_pipeline.push_char(c, cx, footer_y, self.ui.settings_text_dim);
            }
        }

        // ── Dropdown overlay rendered separately in build_dropdown_overlay ──
    }

    /// Build dropdown overlay render data — rendered last so it appears on top of everything
    pub(super) fn build_dropdown_overlay(&mut self, info: &SettingsPanelInfo, window_width: f32, _window_height: f32) {
        // ALL coordinates MUST match build_settings_panel exactly
        let ui_cw = self.cell_width;
        let padding_x = ui_cw * 1.5;
        let sidebar_width = padding_x + ui_cw * 3.0 + ui_cw * 10.0 + padding_x;
        let min_content_width = ui_cw * 30.0;
        let panel_width = ((sidebar_width + min_content_width).max(window_width * 0.35)).min(window_width * 0.55);
        let anim = info.animation;
        let panel_x = window_width - panel_width * anim;
        let panel_y = 0.0_f32;

        let content_x = panel_x + sidebar_width;
        let content_width = panel_width - sidebar_width;

        let row_height = (self.cell_height * 2.2).max(32.0);
        let padding_y = self.cell_height * 0.5;

        let header_y = panel_y + self.cell_height * 1.5;
        let sep_y = header_y + self.cell_height * 1.8;
        let items_start_y = sep_y + padding_y + self.cell_height * 0.5;

        if let Some(item) = info.items.get(info.selected_item) {
            let (options_text, option_count): (Vec<String>, usize) = match &item.kind {
                crate::ui::settings::SettingKind::Dropdown { options, .. } => {
                    (options.clone(), options.len())
                }
                crate::ui::settings::SettingKind::SteppedList { values, .. } => {
                    (values.iter().map(|v| v.to_string()).collect(), values.len())
                }
                _ => (vec![], 0),
            };

            if option_count > 0 {
                // Dynamic value-area: match build_settings_panel
                let max_label_len = info.items.iter()
                    .map(|item| item.label.len())
                    .max()
                    .unwrap_or(0);
                let padding_x = ui_cw * 1.5;
                let label_end = padding_x + max_label_len as f32 * ui_cw;
                let min_gap = ui_cw * 2.0;
                let max_value_area = content_width * 0.65;
                let value_area_offset = (label_end + min_gap).min(max_value_area);
                let dd_value_area_x = content_x + value_area_offset;
                let anchor_y = items_start_y + info.selected_item as f32 * row_height + row_height;

                // Calculate dropdown width based on longest option text
                let max_option_len = options_text.iter()
                    .map(|s| s.len())
                    .max()
                    .unwrap_or(10);
                // Minimum width: longest option + padding on both sides
                let min_dd_width = (max_option_len as f32 + 4.0) * ui_cw;
                // Maximum width: remaining space to right edge
                let remaining_space = content_width - value_area_offset;
                // Use larger of minimum needed or remaining space, but cap at content width
                let dd_width = min_dd_width.max(remaining_space).min(content_width - ui_cw * 2.0);

                // Shift dropdown left if it would exceed panel bounds
                let dd_x = if dd_value_area_x + dd_width > panel_x + panel_width - ui_cw {
                    // Shift left to fit within panel
                    (panel_x + panel_width - dd_width - ui_cw).max(content_x + ui_cw)
                } else {
                    dd_value_area_x
                };

                let option_h = row_height * 0.85;
                let max_visible = 8usize;
                let visible_count = option_count.min(max_visible);
                let dd_height = visible_count as f32 * option_h + 8.0;

                // Shadow
                let shadow_color = [0.0, 0.0, 0.0, 0.3];
                self.rect_pipeline.push_rounded_rect(
                    dd_x + 2.0, anchor_y + 2.0,
                    dd_width, dd_height,
                    shadow_color, 6.0,
                );

                // Background
                // Use fully opaque background so content behind doesn't bleed through
                let dd_bg = [
                    self.ui.settings_bg[0],
                    self.ui.settings_bg[1],
                    self.ui.settings_bg[2],
                    1.0,
                ];
                self.rect_pipeline.push_rounded_rect(
                    dd_x, anchor_y,
                    dd_width, dd_height,
                    dd_bg, 6.0,
                );

                // Border
                let border_color = self.ui.settings_separator;
                self.rect_pipeline.push_rounded_rect(
                    dd_x, anchor_y,
                    dd_width, dd_height,
                    adjust_alpha(border_color, 0.5), 6.0,
                );
                // Inner background (slightly inset to simulate border)
                self.rect_pipeline.push_rounded_rect(
                    dd_x + 1.0, anchor_y + 1.0,
                    dd_width - 2.0, dd_height - 2.0,
                    dd_bg, 5.0,
                );

                // Render options
                for (oidx, opt_text) in options_text.iter().enumerate().take(visible_count) {
                    let oy = anchor_y + 4.0 + oidx as f32 * option_h;
                    let is_highlighted = oidx == info.dropdown_highlight;

                    if is_highlighted {
                        self.rect_pipeline.push_rounded_rect(
                            dd_x + 4.0, oy,
                            dd_width - 8.0, option_h,
                            self.ui.settings_selected, 4.0,
                        );
                        // Accent bar
                        self.rect_pipeline.push_rounded_rect(
                            dd_x + 2.0, oy,
                            3.0, option_h,
                            self.ui.settings_accent, 2.0,
                        );
                    } else if info.dropdown_hovered == Some(oidx) {
                        // Subtle hover highlight for non-highlighted options
                        let hover_color = [
                            self.ui.settings_selected[0],
                            self.ui.settings_selected[1],
                            self.ui.settings_selected[2],
                            self.ui.settings_selected[3] * 0.4,
                        ];
                        self.rect_pipeline.push_rounded_rect(
                            dd_x + 4.0, oy,
                            dd_width - 8.0, option_h,
                            hover_color, 4.0,
                        );
                    }

                    let opt_text_y = oy + (option_h - self.cell_height) / 2.0;
                    let opt_color = if is_highlighted {
                        self.ui.settings_value
                    } else {
                        self.ui.settings_text
                    };

                    for (ci, c) in opt_text.chars().enumerate() {
                        let cx = dd_x + ui_cw * 1.5 + ci as f32 * ui_cw;
                        if cx + ui_cw > dd_x + dd_width - 8.0 {
                            self.text_pipeline.push_char('.', cx, opt_text_y, self.ui.settings_text_dim);
                            break;
                        }
                        self.text_pipeline.push_char(c, cx, opt_text_y, opt_color);
                    }
                }
            }
        }
    }

    /// Draw a wavy/curly underline (approximated with small segments)
    pub(super) fn draw_wavy_underline(&mut self, x: f32, y: f32, width: f32, color: [f32; 4]) {
        // Approximate wavy line with a series of small rectangles at varying heights
        // Using a visible wave pattern similar to LSP error underlines
        let wave_period: f32 = 6.0; // Pixels per wave cycle (wider = more visible waves)
        let wave_amplitude: f32 = 2.0; // Pixels of vertical displacement
        let segment_width: f32 = 1.5; // Smaller segments = smoother curve
        let thickness: f32 = 1.5; // Slightly thicker for visibility

        let mut current_x = x;
        let mut phase: f32 = 0.0;

        while current_x < x + width {
            // Sine wave approximation - center the wave vertically
            let offset_y = (phase * std::f32::consts::TAU).sin() * wave_amplitude;
            self.rect_pipeline.push_rect(
                current_x,
                y + offset_y - wave_amplitude, // Offset so wave is centered around y
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
            self.rect_pipeline.push_rect(current_x, y, dot_size.min(x + width - current_x), 1.0, color);
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
            self.rect_pipeline.push_rect(current_x, y, dash_length.min(x + width - current_x), 1.0, color);
            current_x += step;
        }
    }

}
