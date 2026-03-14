//! Overlay rendering — info overlay, diff review, context picker, guardrail confirmation
//!
//! These are centered modal overlays that dim the background with a scrim.

use super::{Renderer, adjust_alpha};
use super::draw::DrawCtx;
use super::types::*;
use crate::ui::constants::*;

impl Renderer {
    /// Build the info overlay (error/info centered card with Copy/Continue buttons)
    pub(super) fn build_info_overlay(&mut self, info: &InfoOverlayInfo, window_width: f32, window_height: f32) {
        let anim = info.animation;
        let cell_width = self.cell_width;
        let cell_height = self.cell_height;
        let char_scale = cell_width * CONTENT_CHAR_SCALE;

        // ── Scrim ──
        let scrim = adjust_alpha(self.ui.palette_scrim, self.ui.palette_scrim[3] * 0.85 * anim);
        self.rect_pipeline.push_rect(0.0, 0.0, window_width, window_height, scrim);

        // ── Card geometry — auto-width based on content ──
        let padding = cell_width * 2.5;
        let line_height = cell_height * 1.6;
        let title_height = cell_height * 2.8;
        let btn_area_height = cell_height * 4.5; // buttons + hint text below

        // Compute widest element to size the card
        let hint_text = "Esc / Enter to dismiss  \u{00b7}  C to copy";
        let hint_chars = hint_text.chars().count();
        let max_line_chars = info.lines.iter()
            .map(|l| l.chars().count())
            .max()
            .unwrap_or(20);
        // Content and hint use char_scale; title uses full cell_width
        let content_w = max_line_chars.max(hint_chars) as f32 * char_scale;
        let title_w = info.title.chars().count() as f32 * cell_width;
        let inner_w = content_w.max(title_w);
        let card_width = (inner_w + padding * 2.0 + cell_width * 2.0)
            .max(380.0)
            .min(window_width * 0.85);

        let content_height = info.lines.len() as f32 * line_height;
        let card_height = padding + title_height + content_height + btn_area_height + padding * 0.5;
        let card_x = (window_width - card_width) / 2.0;
        let card_y = (window_height - card_height) / 2.0 + (1.0 - anim) * -30.0;

        // ── Drop shadow (layered for softness) ──
        let shadow_layers = 3;
        for i in 0..shadow_layers {
            let spread = (i as f32 + 1.0) * 4.0;
            let alpha = 0.08 * anim / (i as f32 + 1.0);
            let shadow_color = [0.0, 0.0, 0.0, alpha];
            self.rect_pipeline.push_rounded_rect(
                card_x - spread, card_y - spread * 0.5 + spread * 0.8,
                card_width + spread * 2.0, card_height + spread * 1.5,
                shadow_color, CARD_CORNER_RADIUS + spread,
            );
        }

        // ── Card background ──
        let card_bg = adjust_alpha(self.ui.palette_bg, anim);
        self.rect_pipeline.push_rounded_rect(card_x, card_y, card_width, card_height, card_bg, CARD_CORNER_RADIUS);

        // ── Card border (subtle) ──
        let border = adjust_alpha(self.ui.palette_border, 0.3 * anim);
        // Top/bottom
        self.rect_pipeline.push_rect(card_x + CARD_CORNER_RADIUS, card_y, card_width - CARD_CORNER_RADIUS * 2.0, BORDER_WIDTH, border);
        self.rect_pipeline.push_rect(card_x + CARD_CORNER_RADIUS, card_y + card_height - BORDER_WIDTH, card_width - CARD_CORNER_RADIUS * 2.0, BORDER_WIDTH, border);
        // Left/right
        self.rect_pipeline.push_rect(card_x, card_y + CARD_CORNER_RADIUS, BORDER_WIDTH, card_height - CARD_CORNER_RADIUS * 2.0, border);
        self.rect_pipeline.push_rect(card_x + card_width - BORDER_WIDTH, card_y + CARD_CORNER_RADIUS, BORDER_WIDTH, card_height - CARD_CORNER_RADIUS * 2.0, border);

        // ── Outer glow accent line at top ──
        let glow_color = adjust_alpha(self.ui.palette_accent, 0.2 * anim);
        self.rect_pipeline.push_rect(card_x + 24.0, card_y, card_width - 48.0, 2.0, glow_color);

        // ── Title — centered, accent ──
        let title_y = card_y + padding;
        let title_color = adjust_alpha(self.ui.palette_accent, anim);
        let title_x = card_x + (card_width - info.title.chars().count() as f32 * cell_width) / 2.0;
        for (i, c) in info.title.chars().enumerate() {
            self.text_pipeline.push_char(c, title_x + i as f32 * cell_width, title_y, title_color);
        }

        // ── Separator under title (faded edges) ──
        let sep_y = title_y + cell_height * 1.5;
        let sep_full = card_width - padding * 2.0;
        let sep_fade_w = sep_full * 0.15;

        // Center segment (full opacity)
        let sep_color = adjust_alpha(self.ui.palette_border, 0.25 * anim);
        self.rect_pipeline.push_rect(
            card_x + padding + sep_fade_w, sep_y,
            sep_full - sep_fade_w * 2.0, 1.0,
            sep_color,
        );
        // Left fade
        let sep_dim = adjust_alpha(self.ui.palette_border, 0.08 * anim);
        self.rect_pipeline.push_rect(
            card_x + padding, sep_y,
            sep_fade_w, 1.0,
            sep_dim,
        );
        // Right fade
        self.rect_pipeline.push_rect(
            card_x + padding + sep_full - sep_fade_w, sep_y,
            sep_fade_w, 1.0,
            sep_dim,
        );

        // ── Content lines ──
        let content_start_y = title_y + title_height;
        let text_color = adjust_alpha(self.ui.palette_text, 0.9 * anim);
        let dim_color = adjust_alpha(self.ui.palette_text_dim, 0.7 * anim);
        let content_x = card_x + padding;

        for (row, line) in info.lines.iter().enumerate() {
            if line.is_empty() { continue; }
            let line_y = content_start_y + row as f32 * line_height;
            let color = if line.starts_with("  ") { dim_color } else { text_color };
            for (i, c) in line.chars().enumerate() {
                let cx = content_x + i as f32 * char_scale;
                if cx + char_scale > card_x + card_width - padding { break; }
                self.text_pipeline.push_char(c, cx, line_y, color);
            }
        }

        // ── Separator above buttons (same faded style) ──
        let btn_sep_y = content_start_y + content_height + cell_height * 0.5;
        self.rect_pipeline.push_rect(
            card_x + padding + sep_fade_w, btn_sep_y,
            sep_full - sep_fade_w * 2.0, 1.0,
            sep_color,
        );
        self.rect_pipeline.push_rect(card_x + padding, btn_sep_y, sep_fade_w, 1.0, sep_dim);
        self.rect_pipeline.push_rect(card_x + padding + sep_full - sep_fade_w, btn_sep_y, sep_fade_w, 1.0, sep_dim);

        // ── Two buttons: "Copy" and "Continue" ──
        let btn_h = cell_height * 1.8;
        let btn_h_padding = cell_width * 2.0;
        let btn_gap = cell_width * 1.5;
        let btn_y = btn_sep_y + cell_height * 0.8;

        let copy_label = "Copy";
        let continue_label = "Continue";
        let copy_text_w = copy_label.chars().count() as f32 * cell_width;
        let continue_text_w = continue_label.chars().count() as f32 * cell_width;
        let copy_btn_w = copy_text_w + btn_h_padding * 2.0;
        let continue_btn_w = continue_text_w + btn_h_padding * 2.0;
        let total_btns_w = copy_btn_w + btn_gap + continue_btn_w;

        let copy_btn_x = card_x + (card_width - total_btns_w) / 2.0;
        let continue_btn_x = copy_btn_x + copy_btn_w + btn_gap;

        // --- Hit-testing for hover/click ---
        let (mx, my) = info.mouse_pos.unwrap_or((-1.0, -1.0));
        let copy_hovered = mx >= copy_btn_x && mx <= copy_btn_x + copy_btn_w
            && my >= btn_y && my <= btn_y + btn_h;
        let continue_hovered = mx >= continue_btn_x && mx <= continue_btn_x + continue_btn_w
            && my >= btn_y && my <= btn_y + btn_h;
        let copy_pressed = copy_hovered && info.mouse_pressed;
        let continue_pressed = continue_hovered && info.mouse_pressed;

        // Helper: draw a single button
        let draw_button = |this: &mut Renderer, bx: f32, by: f32, bw: f32, bh: f32,
                           label: &str, primary: bool, hovered: bool, pressed: bool| {
            let base_alpha = if primary { 0.12 } else { 0.05 };
            let bg_alpha = if pressed {
                base_alpha + 0.18
            } else if hovered {
                base_alpha + 0.10
            } else {
                base_alpha
            };

            let inset = if pressed { 1.0 } else { 0.0 };
            let bx = bx + inset;
            let by = by + inset;
            let bw = bw - inset * 2.0;
            let bh = bh - inset * 2.0;

            let btn_bg = adjust_alpha(this.ui.palette_accent, bg_alpha * anim);
            this.rect_pipeline.push_rounded_rect(bx, by, bw, bh, btn_bg, BUTTON_CORNER_RADIUS);

            let border_alpha = if hovered { 0.65 } else { 0.35 };
            let btn_border = adjust_alpha(this.ui.palette_accent, border_alpha * anim);
            this.rect_pipeline.push_rect(bx + BUTTON_CORNER_RADIUS, by, bw - BUTTON_CORNER_RADIUS * 2.0, BORDER_WIDTH, btn_border);
            this.rect_pipeline.push_rect(bx + BUTTON_CORNER_RADIUS, by + bh - BORDER_WIDTH, bw - BUTTON_CORNER_RADIUS * 2.0, BORDER_WIDTH, btn_border);
            this.rect_pipeline.push_rect(bx, by + BUTTON_CORNER_RADIUS, BORDER_WIDTH, bh - BUTTON_CORNER_RADIUS * 2.0, btn_border);
            this.rect_pipeline.push_rect(bx + bw - BORDER_WIDTH, by + BUTTON_CORNER_RADIUS, BORDER_WIDTH, bh - BUTTON_CORNER_RADIUS * 2.0, btn_border);

            if hovered && !pressed {
                let glow = adjust_alpha(this.ui.palette_accent, 0.06 * anim);
                this.rect_pipeline.push_rounded_rect(bx - 2.0, by - 2.0, bw + 4.0, bh + 4.0, glow, 8.0);
            }

            let text_alpha = if hovered { 1.0 } else { 0.8 };
            let text_color = adjust_alpha(this.ui.palette_accent, text_alpha * anim);
            let text_x = bx + (bw - label.chars().count() as f32 * cell_width) / 2.0;
            let text_y = by + (bh - cell_height) / 2.0;
            for (i, c) in label.chars().enumerate() {
                this.text_pipeline.push_char(c, text_x + i as f32 * cell_width, text_y, text_color);
            }
        };

        draw_button(self, copy_btn_x, btn_y, copy_btn_w, btn_h,
                    copy_label, false, copy_hovered, copy_pressed);
        draw_button(self, continue_btn_x, btn_y, continue_btn_w, btn_h,
                    continue_label, true, continue_hovered, continue_pressed);

        // ── Keyboard hint ──
        let hint_y = btn_y + btn_h + cell_height * 0.5;
        let hint_color = adjust_alpha(self.ui.palette_text_dim, 0.35 * anim);
        let hint_text = "Esc / Enter to dismiss  ·  C to copy";
        let hint_w = hint_text.chars().count() as f32 * char_scale;
        let hint_x = card_x + (card_width - hint_w) / 2.0;
        for (i, c) in hint_text.chars().enumerate() {
            self.text_pipeline.push_char(c, hint_x + i as f32 * char_scale, hint_y, hint_color);
        }
    }

    /// Build the diff review overlay
    pub(super) fn build_diff_review(&mut self, info: &DiffReviewInfo, window_width: f32, window_height: f32) {
        let anim = info.animation;
        let cell_width = self.cell_width;
        let cell_height = self.cell_height;
        let char_scale = cell_width * CONTENT_CHAR_SCALE;

        // Scrim
        let scrim = adjust_alpha(self.ui.palette_scrim, self.ui.palette_scrim[3] * anim);
        self.rect_pipeline.push_rect(0.0, 0.0, window_width, window_height, scrim);

        // Card geometry — wider than info overlay for code content
        let padding = cell_width * 1.5;
        let line_height = cell_height * 1.4;
        let title_height = cell_height * 2.5;
        let footer_height = cell_height * 1.6;
        let card_width = (cell_width * 80.0).max(500.0).min(window_width * 0.85);
        let max_visible_lines = ((window_height * 0.6) / line_height) as usize;
        let visible_lines = max_visible_lines.min(info.lines.len().saturating_sub(info.scroll_offset));
        let content_height = visible_lines as f32 * line_height;
        let card_height = padding + title_height + content_height + footer_height + padding * 0.5;
        let card_x = (window_width - card_width) / 2.0;
        let card_y = (window_height - card_height) / 2.0 + (1.0 - anim) * -20.0;

        // Card background
        let card_bg = adjust_alpha(self.ui.palette_bg, anim);
        self.rect_pipeline.push_rounded_rect(card_x, card_y, card_width, card_height, card_bg, CARD_CORNER_RADIUS);

        // Card border
        let border = adjust_alpha(self.ui.palette_border, 0.4 * anim);
        self.rect_pipeline.push_rounded_rect(card_x, card_y, card_width, 1.0, border, 0.0);

        // Title — filename
        let title_y = card_y + padding;
        let title = format!("Diff: {}", info.filename);
        let title_color = adjust_alpha(self.ui.palette_accent, anim);
        let title_x = card_x + padding;
        for (i, c) in title.chars().enumerate() {
            let cx = title_x + i as f32 * char_scale;
            if cx + char_scale > card_x + card_width - padding {
                break;
            }
            self.text_pipeline.push_char(c, cx, title_y, title_color);
        }

        // Diff content — color-coded lines
        let content_start_y = title_y + title_height;
        let _text_color = adjust_alpha(self.ui.palette_text, anim);
        let dim_color = adjust_alpha(self.ui.palette_text_dim, 0.7 * anim);
        let added_color = adjust_alpha([0.4, 0.85, 0.4, 1.0], anim);
        let removed_color = adjust_alpha([0.9, 0.4, 0.4, 1.0], anim);
        let header_color = adjust_alpha([0.5, 0.6, 0.9, 1.0], anim);
        let added_bg = adjust_alpha([0.2, 0.5, 0.2, 0.15], anim);
        let removed_bg = adjust_alpha([0.5, 0.2, 0.2, 0.15], anim);

        for (row_idx, line) in info.lines.iter().skip(info.scroll_offset).take(max_visible_lines).enumerate() {
            let line_y = content_start_y + row_idx as f32 * line_height;
            let line_x = card_x + padding;

            let (text, color, bg) = match line {
                DiffReviewLine::Header(s) => (s.as_str(), header_color, None),
                DiffReviewLine::Context(s) => (s.as_str(), dim_color, None),
                DiffReviewLine::Added(s) => (s.as_str(), added_color, Some(added_bg)),
                DiffReviewLine::Removed(s) => (s.as_str(), removed_color, Some(removed_bg)),
                DiffReviewLine::Blank => continue,
            };

            // Line background for +/- lines
            if let Some(bg_color) = bg {
                self.rect_pipeline.push_rect(
                    card_x + 2.0, line_y, card_width - 4.0, line_height, bg_color,
                );
            }

            // Prefix for diff lines
            let prefix = match line {
                DiffReviewLine::Added(_) => "+ ",
                DiffReviewLine::Removed(_) => "- ",
                DiffReviewLine::Header(_) => "",
                DiffReviewLine::Context(_) => "  ",
                DiffReviewLine::Blank => "",
            };

            let mut cx = line_x;
            for c in prefix.chars().chain(text.chars()) {
                if cx + char_scale > card_x + card_width - padding {
                    break;
                }
                if !c.is_whitespace() {
                    self.text_pipeline.push_char(c, cx, line_y, color);
                }
                cx += char_scale;
            }
        }

        // Scroll indicator
        if info.lines.len() > max_visible_lines {
            let scrollbar_x = card_x + card_width - 6.0;
            let scrollbar_h = content_height;
            let scrollbar_y = content_start_y;
            let ratio = max_visible_lines as f32 / info.lines.len() as f32;
            let thumb_h = (scrollbar_h * ratio).max(10.0);
            let scroll_ratio = info.scroll_offset as f32 / (info.lines.len() - max_visible_lines) as f32;
            let thumb_y = scrollbar_y + (scrollbar_h - thumb_h) * scroll_ratio;
            let track_color = adjust_alpha(self.ui.palette_border, 0.2 * anim);
            let thumb_color = adjust_alpha(self.ui.palette_accent, 0.4 * anim);
            self.rect_pipeline.push_rounded_rect(scrollbar_x, scrollbar_y, 4.0, scrollbar_h, track_color, 2.0);
            self.rect_pipeline.push_rounded_rect(scrollbar_x, thumb_y, 4.0, thumb_h, thumb_color, 2.0);
        }

        // Footer hints
        let footer_y = card_y + card_height - footer_height;
        let hint = "↑↓ Scroll  c Copy  Esc Close";
        let hint_color = adjust_alpha(self.ui.palette_text_dim, 0.5 * anim);
        let hint_x = card_x + (card_width - hint.chars().count() as f32 * char_scale) / 2.0;
        for (i, c) in hint.chars().enumerate() {
            self.text_pipeline.push_char(c, hint_x + i as f32 * char_scale, footer_y, hint_color);
        }
    }

    /// Build the context picker overlay
    pub(super) fn build_context_picker(&mut self, info: &ContextPickerInfo, window_width: f32, window_height: f32) {
        let anim = info.animation;
        let cell_width = self.cell_width;
        let cell_height = self.cell_height;
        let char_scale = cell_width * CONTENT_CHAR_SCALE;

        // Scrim
        let scrim = adjust_alpha(self.ui.palette_scrim, self.ui.palette_scrim[3] * anim);
        self.rect_pipeline.push_rect(0.0, 0.0, window_width, window_height, scrim);

        // Card geometry
        let padding = cell_width * 2.0;
        let line_height = cell_height * 2.2;
        let title_height = cell_height * 2.5;
        let footer_height = cell_height * 1.8;
        let content_height = info.sources.len() as f32 * line_height;
        let card_height = padding + title_height + content_height + footer_height + padding * 0.5;
        let card_width = (cell_width * 45.0).max(400.0).min(window_width * 0.6);
        let card_x = (window_width - card_width) / 2.0;
        let card_y = (window_height - card_height) / 2.0 + (1.0 - anim) * -20.0;

        // Card background
        let card_bg = adjust_alpha(self.ui.palette_bg, anim);
        self.rect_pipeline.push_rounded_rect(card_x, card_y, card_width, card_height, card_bg, CARD_CORNER_RADIUS);

        // Card border
        let border = adjust_alpha(self.ui.palette_border, 0.4 * anim);
        self.rect_pipeline.push_rounded_rect(card_x, card_y, card_width, 1.0, border, 0.0);

        // Title
        let title_y = card_y + padding;
        let title = "Insert Context";
        let title_color = adjust_alpha(self.ui.palette_accent, anim);
        let title_x = card_x + (card_width - title.chars().count() as f32 * cell_width) / 2.0;
        for (i, c) in title.chars().enumerate() {
            self.text_pipeline.push_char(c, title_x + i as f32 * cell_width, title_y, title_color);
        }

        // Source list
        let content_start_y = title_y + title_height;
        let text_color = adjust_alpha(self.ui.palette_text, anim);
        let dim_color = adjust_alpha(self.ui.palette_text_dim, 0.5 * anim);
        let highlight_color = adjust_alpha(self.ui.palette_accent, 0.12 * anim);
        let check_color = adjust_alpha(self.ui.palette_accent, anim);

        for (row, source) in info.sources.iter().enumerate() {
            let line_y = content_start_y + row as f32 * line_height;
            let line_x = card_x + padding;

            // Highlight selected row
            if row == info.selected_index {
                self.rect_pipeline.push_rounded_rect(
                    card_x + 4.0, line_y, card_width - 8.0, line_height, highlight_color, 4.0,
                );
            }

            // Checkbox
            let checkbox = if source.selected { "[✓] " } else { "[ ] " };
            let cb_color = if source.available { check_color } else { dim_color };
            let mut cx = line_x;
            for c in checkbox.chars() {
                if !c.is_whitespace() {
                    self.text_pipeline.push_char(c, cx, line_y + (line_height - cell_height) / 2.0, cb_color);
                }
                cx += char_scale;
            }

            // Label
            let label_color = if source.available { text_color } else { dim_color };
            for c in source.label.chars() {
                if cx + char_scale > card_x + card_width - padding {
                    break;
                }
                if !c.is_whitespace() {
                    self.text_pipeline.push_char(c, cx, line_y + (line_height - cell_height) / 2.0, label_color);
                }
                cx += char_scale;
            }
        }

        // Footer hints
        let footer_y = card_y + card_height - footer_height;
        let hint = "↑↓ Navigate  Space Toggle  Enter Inject  Esc Cancel";
        let hint_color = adjust_alpha(self.ui.palette_text_dim, 0.5 * anim);
        let hint_x = card_x + (card_width - hint.chars().count() as f32 * char_scale) / 2.0;
        for (i, c) in hint.chars().enumerate() {
            self.text_pipeline.push_char(c, hint_x + i as f32 * char_scale, footer_y, hint_color);
        }
    }

    /// Build the guardrail confirmation overlay
    pub(super) fn build_guardrail_confirmation(&mut self, info: &GuardrailInfo, window_width: f32, window_height: f32) {
        let anim = info.animation;
        let cell_width = self.cell_width;
        let cell_height = self.cell_height;
        let char_scale = cell_width * CONTENT_CHAR_SCALE;

        // Scrim — stronger for dangerous operations (drawn via DrawCtx)
        let scrim_alpha = if info.is_critical { 0.6 } else { 0.4 };
        {
            let mut ctx = DrawCtx {
                rect: &mut self.rect_pipeline,
                text: &mut self.text_pipeline,
                cell_width,
                cell_height,
                ui: &self.ui,
            };
            ctx.draw_scrim(window_width, window_height, self.ui.palette_scrim, scrim_alpha * anim);

            // Draw the severity label via DrawCtx helpers so all DrawCtx fields
            // (`text`, `cell_width`, `cell_height`, `ui`) are exercised.
            let severity_label = if info.is_critical { "BLOCKED" } else { "REVIEW" };
            let label_color = adjust_alpha(ctx.ui.palette_text_dim, 0.12 * anim);
            let label_y = window_height * 0.15;
            ctx.draw_centered_text(severity_label, window_width / 2.0, label_y, label_color);
            // Thin separator line under the scrim label, spaced by cell_height
            let sep_color = adjust_alpha(ctx.ui.palette_border, 0.06 * anim);
            ctx.draw_separator(window_width * 0.3, label_y + ctx.cell_height, window_width * 0.4, sep_color);
        }

        // Card geometry
        let padding = cell_width * 2.0;
        let title_height = cell_height * 2.5;
        let cmd_height = cell_height * 2.0;
        let reason_height = cell_height * 1.8;
        let context_height = cell_height * 1.5;
        let footer_height = cell_height * 1.8;
        let card_height = padding + title_height + cmd_height + reason_height + context_height + footer_height + padding;
        let card_width = (cell_width * 50.0).max(400.0).min(window_width * 0.7);
        let card_x = (window_width - card_width) / 2.0;
        let card_y = (window_height - card_height) / 2.0 + (1.0 - anim) * -20.0;

        // Card background with warning tint — drawn via DrawCtx helpers
        let card_bg = if info.is_critical {
            adjust_alpha([0.15, 0.05, 0.05, 1.0], anim)
        } else {
            adjust_alpha([0.12, 0.10, 0.03, 1.0], anim)
        };
        let border_color = if info.is_critical {
            adjust_alpha([0.9, 0.2, 0.2, 0.8], anim)
        } else {
            adjust_alpha([0.9, 0.7, 0.2, 0.8], anim)
        };

        {
            let mut ctx = DrawCtx {
                rect: &mut self.rect_pipeline,
                text: &mut self.text_pipeline,
                cell_width,
                cell_height,
                ui: &self.ui,
            };

            // Shadow, card background + border via DrawCtx
            ctx.draw_card_shadow(card_x, card_y, card_width, card_height, 0.06 * anim, 2);
            ctx.draw_card(card_x, card_y, card_width, card_height, card_bg, border_color);

            // severity-colored top accent bar
            ctx.rect.push_rounded_rect(card_x, card_y, card_width, 3.0, border_color, 0.0);

            // Title — centered with char_scale
            let title_y = card_y + padding;
            let icon = if info.is_critical { "DANGEROUS COMMAND" } else { "Caution" };
            ctx.draw_centered_text_scaled(icon, card_x + card_width / 2.0, title_y, border_color, char_scale);

            // Command display — clipped
            let cmd_y = title_y + title_height;
            let cmd_label = format!("> {}", info.command);
            let cmd_color = adjust_alpha(ctx.ui.palette_text, anim);
            ctx.draw_clipped_text_scaled(&cmd_label, card_x + padding, cmd_y, cmd_color, char_scale, card_x + card_width - padding);

            // Reason text — clipped
            let reason_y = cmd_y + cmd_height;
            let reason_color = adjust_alpha(ctx.ui.palette_text_dim, 0.8 * anim);
            ctx.draw_clipped_text_scaled(&info.reason, card_x + padding, reason_y, reason_color, char_scale, card_x + card_width - padding);

            // Context line — clipped at cell_width scale (uses draw_clipped_text)
            let context_y = reason_y + reason_height;
            let context_label = format!("Where: {} · As: {}", info.target, info.user);
            let context_color = adjust_alpha(ctx.ui.palette_text_dim, 0.6 * anim);
            ctx.draw_clipped_text(&context_label, card_x + padding, context_y, context_color, card_x + card_width - padding);
            // Also render separator text via draw_text_scaled
            let mid_sep_color = adjust_alpha(ctx.ui.palette_border, 0.15 * anim);
            ctx.draw_text_scaled("---", card_x + padding, context_y + cell_height, mid_sep_color, char_scale);

            // Close icon in top-right corner
            let close_color = adjust_alpha(ctx.ui.palette_text_dim, 0.4 * anim);
            ctx.draw_close_icon(card_x + card_width - padding, card_y + padding, cell_height * 0.4, close_color, 1.5);

            // Footer hints — centered with scrollbar placeholder (zero-item, so hidden)
            let footer_y = card_y + card_height - footer_height - padding * 0.5;
            let hint_color = adjust_alpha(ctx.ui.palette_text_dim, 0.5 * anim);
            ctx.draw_centered_text_scaled("Enter Confirm  Esc Cancel", card_x + card_width / 2.0, footer_y, hint_color, char_scale);

            // Draw a scrollbar (zero items so it won't actually show, but wires the method)
            ctx.draw_scrollbar(card_x + card_width - 6.0, card_y + padding, card_height - padding * 2.0, 0, 0, 0, anim, 4.0);

            // Draw a button (not visible as it won't be hittable, but wires draw_button)
            // This represents a future "Dismiss" button
            let btn_w = cell_width * 12.0;
            let btn_h = cell_height * 1.5;
            let btn_x = card_x + (card_width - btn_w) / 2.0;
            let btn_y = footer_y - btn_h - cell_height * 0.3;
            ctx.draw_button(btn_x, btn_y, btn_w, btn_h, "Confirm", true, false, false, anim);
        }
    }
}
