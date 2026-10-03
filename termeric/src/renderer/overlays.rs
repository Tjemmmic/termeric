//! Overlay rendering — info overlay, diff review, context picker, guardrail confirmation
//!
//! These are centered modal overlays that dim the background with a scrim.

use super::Renderer;
use super::draw::DrawCtx;
use super::icons::draw_icon;
use super::types::*;
use crate::ui::color::{mix, scale_alpha, with_alpha};
use crate::ui::constants::*;
use crate::ui::guardrail::GuardrailLayout;
use crate::ui::icon::Icon;

impl Renderer {
    /// Build the info overlay (error/info centered card with Copy/Continue buttons)
    pub(super) fn build_info_overlay(
        &mut self,
        info: &InfoOverlayInfo,
        window_width: f32,
        window_height: f32,
    ) {
        let anim = info.animation;
        let cell_width = self.cell_width;
        let cell_height = self.cell_height;

        // ── Scrim ──
        let scrim = scale_alpha(self.ui.palette_scrim, 0.85 * anim);
        self.rect_pipeline
            .push_rect(0.0, 0.0, window_width, window_height, scrim);

        // The app computes this once for viewport clamping and hands the exact
        // same geometry to rendering; only clipped visible rows cross the
        // frame boundary.
        let layout = info.layout;
        let card_x = layout.card.x;
        let card_y = layout.card.y;
        let card_width = layout.card.width;
        let card_height = layout.card.height;
        let padding = layout.padding;
        let line_height = layout.line_height;
        let content_height = layout.content.height;
        let char_scale = layout.char_scale;
        // ── Drop shadow (layered for softness) ──
        let shadow_layers = 3;
        for i in 0..shadow_layers {
            let spread = (i as f32 + 1.0) * 4.0;
            let alpha = 0.08 * anim / (i as f32 + 1.0);
            let shadow_color = [0.0, 0.0, 0.0, alpha];
            self.rect_pipeline.push_rounded_rect(
                card_x - spread,
                card_y - spread * 0.5 + spread * 0.8,
                card_width + spread * 2.0,
                card_height + spread * 1.5,
                shadow_color,
                CARD_CORNER_RADIUS + spread,
            );
        }

        // ── Card background ──
        let card_bg = with_alpha(self.ui.palette_bg, anim);
        self.rect_pipeline.push_rounded_rect(
            card_x,
            card_y,
            card_width,
            card_height,
            card_bg,
            CARD_CORNER_RADIUS,
        );

        // ── Card border (subtle) ──
        let border = with_alpha(self.ui.palette_border, 0.3 * anim);
        // Top/bottom
        self.rect_pipeline.push_rect(
            card_x + CARD_CORNER_RADIUS,
            card_y,
            card_width - CARD_CORNER_RADIUS * 2.0,
            BORDER_WIDTH,
            border,
        );
        self.rect_pipeline.push_rect(
            card_x + CARD_CORNER_RADIUS,
            card_y + card_height - BORDER_WIDTH,
            card_width - CARD_CORNER_RADIUS * 2.0,
            BORDER_WIDTH,
            border,
        );
        // Left/right
        self.rect_pipeline.push_rect(
            card_x,
            card_y + CARD_CORNER_RADIUS,
            BORDER_WIDTH,
            card_height - CARD_CORNER_RADIUS * 2.0,
            border,
        );
        self.rect_pipeline.push_rect(
            card_x + card_width - BORDER_WIDTH,
            card_y + CARD_CORNER_RADIUS,
            BORDER_WIDTH,
            card_height - CARD_CORNER_RADIUS * 2.0,
            border,
        );

        // ── Outer glow accent line at top ──
        let glow_color = with_alpha(self.ui.palette_accent, 0.2 * anim);
        self.rect_pipeline.push_rect(
            card_x + 24.0,
            card_y,
            (card_width - 48.0).max(1.0),
            2.0,
            glow_color,
        );

        // ── Title — centered, accent ──
        let title_y = card_y + padding;
        let title_color = with_alpha(self.ui.palette_accent, anim);
        let max_title_chars = ((card_width - padding * 2.0) / cell_width).floor().max(1.0) as usize;
        let visible_title: String = info.title.chars().take(max_title_chars).collect();
        let title_x =
            card_x + (card_width - visible_title.chars().count() as f32 * cell_width) / 2.0;
        for (i, c) in visible_title.chars().enumerate() {
            self.text_pipeline
                .push_char(c, title_x + i as f32 * cell_width, title_y, title_color);
        }

        // ── Separator under title (faded edges) ──
        let sep_y = title_y + cell_height * 1.5;
        let sep_full = (card_width - padding * 2.0).max(1.0);
        let sep_fade_w = sep_full * 0.15;

        // Center segment (full opacity)
        let sep_color = with_alpha(self.ui.palette_border, 0.25 * anim);
        self.rect_pipeline.push_rect(
            card_x + padding + sep_fade_w,
            sep_y,
            sep_full - sep_fade_w * 2.0,
            1.0,
            sep_color,
        );
        // Left fade
        let sep_dim = with_alpha(self.ui.palette_border, 0.08 * anim);
        self.rect_pipeline
            .push_rect(card_x + padding, sep_y, sep_fade_w, 1.0, sep_dim);
        // Right fade
        self.rect_pipeline.push_rect(
            card_x + padding + sep_full - sep_fade_w,
            sep_y,
            sep_fade_w,
            1.0,
            sep_dim,
        );

        // ── Content lines ──
        let content_start_y = layout.content.y;
        let text_color = with_alpha(self.ui.palette_text, 0.9 * anim);
        let dim_color = with_alpha(self.ui.palette_text_dim, 0.7 * anim);
        let content_x = layout.content.x;
        let (mouse_x, mouse_y) = info.mouse_pos.unwrap_or((-1.0, -1.0));

        for (visible_row, (source_row, line)) in info.visible_lines.iter().enumerate() {
            let line_y = content_start_y + visible_row as f32 * line_height;
            let input = info
                .input
                .as_ref()
                .filter(|input| input.source_line == *source_row);
            let choice = info
                .choices
                .iter()
                .find(|choice| choice.source_line == *source_row);
            let choice_hovered = info.awaiting
                && choice.is_some()
                && layout.content.contains(mouse_x, mouse_y)
                && mouse_y >= line_y
                && mouse_y <= line_y + line_height;
            if input.is_some() {
                self.rect_pipeline.push_rounded_rect(
                    layout.content.x,
                    line_y,
                    layout.content.width,
                    line_height,
                    with_alpha(self.ui.palette_accent, 0.08 * anim),
                    BUTTON_CORNER_RADIUS,
                );
                self.rect_pipeline.push_rect(
                    layout.content.x,
                    line_y + line_height - BORDER_WIDTH,
                    layout.content.width,
                    BORDER_WIDTH,
                    with_alpha(self.ui.palette_accent, 0.55 * anim),
                );
            } else if choice.is_some() {
                let alpha = if choice_hovered { 0.14 } else { 0.06 };
                self.rect_pipeline.push_rounded_rect(
                    layout.content.x,
                    line_y,
                    layout.content.width,
                    line_height,
                    with_alpha(self.ui.palette_accent, alpha * anim),
                    BUTTON_CORNER_RADIUS,
                );
            }
            if line.is_empty() {
                continue;
            }
            let color = if choice_hovered {
                with_alpha(self.ui.palette_accent, anim)
            } else if line.starts_with("  ") {
                dim_color
            } else {
                text_color
            };
            for (i, c) in line.chars().enumerate() {
                let cx = content_x + i as f32 * char_scale;
                self.text_pipeline.push_char(c, cx, line_y, color);
            }
            if let Some(input) = input
                && info.focus == Some(crate::ui::info_overlay::InfoOverlayFocus::Input)
            {
                let cursor_column =
                    input.label.chars().count() + input.value[..input.cursor_byte].chars().count();
                if cursor_column >= info.horizontal_offset
                    && cursor_column
                        < info
                            .horizontal_offset
                            .saturating_add(layout.columns_that_fit)
                {
                    let cursor_x = content_x
                        + cursor_column.saturating_sub(info.horizontal_offset) as f32 * char_scale;
                    self.rect_pipeline.push_rounded_rect(
                        cursor_x,
                        line_y + cell_height * 0.12,
                        2.0_f32.max(char_scale * 0.12),
                        (line_height - cell_height * 0.24).max(1.0),
                        with_alpha(self.ui.palette_accent, anim),
                        1.0,
                    );
                }
            }
        }

        {
            let mut ctx = DrawCtx {
                rect: &mut self.rect_pipeline,
                text: &mut self.text_pipeline,
                cell_width,
                cell_height,
                ui: &self.ui,
            };
            ctx.draw_scrollbar(
                layout.scrollbar_x,
                layout.content.y,
                layout.content.height,
                info.total_lines,
                layout.rows_that_fit,
                info.scroll_offset,
                anim,
                4.0,
            );
        }

        // ── Separator above buttons (same faded style) ──
        let btn_sep_y = content_start_y + content_height + cell_height * 0.5;
        self.rect_pipeline.push_rect(
            card_x + padding + sep_fade_w,
            btn_sep_y,
            sep_full - sep_fade_w * 2.0,
            1.0,
            sep_color,
        );
        self.rect_pipeline
            .push_rect(card_x + padding, btn_sep_y, sep_fade_w, 1.0, sep_dim);
        self.rect_pipeline.push_rect(
            card_x + padding + sep_full - sep_fade_w,
            btn_sep_y,
            sep_fade_w,
            1.0,
            sep_dim,
        );

        // ── Two buttons: "Copy" and "Continue" ──
        let copy_label = "Copy";
        let continue_label = if info.input.is_some() {
            "Save"
        } else {
            "Continue"
        };
        let copy_btn_x = layout.copy_button.x;
        let copy_btn_w = layout.copy_button.width;
        let continue_btn_x = layout.continue_button.x;
        let continue_btn_w = layout.continue_button.width;
        let btn_y = layout.copy_button.y;
        let btn_h = layout.copy_button.height;

        // --- Hit-testing for hover/click ---
        let copy_hovered = info.awaiting && layout.copy_button.contains(mouse_x, mouse_y);
        let continue_hovered = info.awaiting && layout.continue_button.contains(mouse_x, mouse_y);
        let copy_pressed = copy_hovered && info.mouse_pressed;
        let continue_pressed = continue_hovered && info.mouse_pressed;

        // Helper: draw a single button
        let draw_button = |this: &mut Renderer,
                           bx: f32,
                           by: f32,
                           bw: f32,
                           bh: f32,
                           label: &str,
                           primary: bool,
                           hovered: bool,
                           pressed: bool| {
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

            let btn_bg = with_alpha(this.ui.palette_accent, bg_alpha * anim);
            this.rect_pipeline
                .push_rounded_rect(bx, by, bw, bh, btn_bg, BUTTON_CORNER_RADIUS);

            let border_alpha = if hovered { 0.65 } else { 0.35 };
            let btn_border = with_alpha(this.ui.palette_accent, border_alpha * anim);
            this.rect_pipeline.push_rect(
                bx + BUTTON_CORNER_RADIUS,
                by,
                bw - BUTTON_CORNER_RADIUS * 2.0,
                BORDER_WIDTH,
                btn_border,
            );
            this.rect_pipeline.push_rect(
                bx + BUTTON_CORNER_RADIUS,
                by + bh - BORDER_WIDTH,
                bw - BUTTON_CORNER_RADIUS * 2.0,
                BORDER_WIDTH,
                btn_border,
            );
            this.rect_pipeline.push_rect(
                bx,
                by + BUTTON_CORNER_RADIUS,
                BORDER_WIDTH,
                bh - BUTTON_CORNER_RADIUS * 2.0,
                btn_border,
            );
            this.rect_pipeline.push_rect(
                bx + bw - BORDER_WIDTH,
                by + BUTTON_CORNER_RADIUS,
                BORDER_WIDTH,
                bh - BUTTON_CORNER_RADIUS * 2.0,
                btn_border,
            );

            if hovered && !pressed {
                let glow = with_alpha(this.ui.palette_accent, 0.06 * anim);
                this.rect_pipeline.push_rounded_rect(
                    bx - 2.0,
                    by - 2.0,
                    bw + 4.0,
                    bh + 4.0,
                    glow,
                    8.0,
                );
            }

            let text_alpha = if hovered { 1.0 } else { 0.8 };
            let text_color = with_alpha(this.ui.palette_accent, text_alpha * anim);
            let text_x = bx + (bw - label.chars().count() as f32 * cell_width) / 2.0;
            let text_y = by + (bh - cell_height) / 2.0;
            for (i, c) in label.chars().enumerate() {
                this.text_pipeline
                    .push_char(c, text_x + i as f32 * cell_width, text_y, text_color);
            }
        };

        draw_button(
            self,
            copy_btn_x,
            btn_y,
            copy_btn_w,
            btn_h,
            copy_label,
            false,
            copy_hovered,
            copy_pressed,
        );
        draw_button(
            self,
            continue_btn_x,
            btn_y,
            continue_btn_w,
            btn_h,
            continue_label,
            true,
            continue_hovered,
            continue_pressed,
        );

        // Keyboard and assistive focus use the same geometry as hit testing.
        if let Some(focus) = &info.focus {
            use crate::ui::info_overlay::InfoOverlayFocus;
            let bounds = match focus {
                InfoOverlayFocus::Input => info.input.as_ref().and_then(|input| {
                    layout.source_line_rect(input.source_line, info.scroll_offset, info.total_lines)
                }),
                InfoOverlayFocus::Choice(id) => info
                    .choices
                    .iter()
                    .find(|choice| &choice.id == id)
                    .and_then(|choice| {
                        layout.source_line_rect(
                            choice.source_line,
                            info.scroll_offset,
                            info.total_lines,
                        )
                    }),
                InfoOverlayFocus::Copy => Some(layout.copy_button),
                InfoOverlayFocus::Continue => Some(layout.continue_button),
            };
            if let Some(bounds) = bounds {
                let color = with_alpha(self.ui.palette_accent, anim);
                self.rect_pipeline
                    .push_rect(bounds.x, bounds.y, bounds.width, 2.0, color);
                self.rect_pipeline.push_rect(
                    bounds.x,
                    bounds.y + bounds.height - 2.0,
                    bounds.width,
                    2.0,
                    color,
                );
                self.rect_pipeline
                    .push_rect(bounds.x, bounds.y, 2.0, bounds.height, color);
                self.rect_pipeline.push_rect(
                    bounds.x + bounds.width - 2.0,
                    bounds.y,
                    2.0,
                    bounds.height,
                    color,
                );
            }
        }

        // ── Keyboard hint ──
        let hint_y = btn_y + btn_h + cell_height * 0.5;
        let hint_color = with_alpha(self.ui.palette_text_dim, 0.35 * anim);
        let first_line = info
            .scroll_offset
            .saturating_add(1)
            .min(info.total_lines.max(1));
        let last_line = info
            .scroll_offset
            .saturating_add(layout.rows_that_fit)
            .min(info.total_lines);
        let hint_text = if info.total_lines > layout.rows_that_fit || info.horizontal_offset > 0 {
            format!(
                "Tab focus | Enter act | Esc close | {first_line}-{last_line}/{} c{}",
                info.total_lines,
                info.horizontal_offset + 1
            )
        } else {
            "Tab focus | Enter act | Esc close".into()
        };
        let hint_text: String = hint_text
            .chars()
            .take(((card_width - padding * 2.0) / char_scale).floor() as usize)
            .collect();
        let hint_w = hint_text.chars().count() as f32 * char_scale;
        let hint_x = card_x + (card_width - hint_w) / 2.0;
        for (i, c) in hint_text.chars().enumerate() {
            self.text_pipeline
                .push_char(c, hint_x + i as f32 * char_scale, hint_y, hint_color);
        }
    }

    /// Build the diff review overlay
    pub(super) fn build_diff_review(
        &mut self,
        info: &DiffReviewInfo,
        window_width: f32,
        window_height: f32,
    ) {
        use crate::ui::diff_review::DiffReviewRow;

        let anim = info.animation;
        let cell_width = self.cell_width;
        let cell_height = self.cell_height;
        let layout = info.layout;
        let char_scale = layout.char_scale;

        let scrim = scale_alpha(self.ui.palette_scrim, 0.85 * anim);
        self.rect_pipeline
            .push_rect(0.0, 0.0, window_width, window_height, scrim);
        let card_x = layout.card.x;
        let card_y = layout.card.y;
        let card_width = layout.card.width;
        let card_height = layout.card.height;
        let padding = layout.padding;

        let card_bg = with_alpha(self.ui.palette_bg, anim);
        self.rect_pipeline.push_rounded_rect(
            card_x,
            card_y,
            card_width,
            card_height,
            card_bg,
            CARD_CORNER_RADIUS,
        );

        let border = with_alpha(self.ui.palette_border, 0.4 * anim);
        self.rect_pipeline
            .push_rounded_rect(card_x, card_y, card_width, 1.0, border, 0.0);

        let title_y = card_y + padding;
        let title = format!("Diff: {}", info.filename);
        let title_color = with_alpha(self.ui.palette_accent, anim);
        let title_x = card_x + padding;
        for (i, c) in title.chars().enumerate() {
            let cx = title_x + i as f32 * char_scale;
            if cx + char_scale > card_x + card_width - padding {
                break;
            }
            self.text_pipeline.push_char(c, cx, title_y, title_color);
        }

        let content_start_y = layout.content.y;
        let dim_color = with_alpha(self.ui.palette_text_dim, 0.7 * anim);
        let added_color = with_alpha(self.ui.success, anim);
        let removed_color = with_alpha(self.ui.error, anim);
        let header_color = with_alpha(self.ui.palette_accent, anim);
        let omitted_color = with_alpha(self.ui.warning, anim);
        let added_bg = with_alpha(self.ui.success, 0.15 * anim);
        let removed_bg = with_alpha(self.ui.error, 0.15 * anim);

        for (visible_row, (_, row)) in info.visible_rows.iter().enumerate() {
            let line_y = content_start_y + visible_row as f32 * layout.line_height;
            let (color, bg) = match row {
                DiffReviewRow::Header(_) => (header_color, None),
                DiffReviewRow::Context(_) => (dim_color, None),
                DiffReviewRow::Added(_) => (added_color, Some(added_bg)),
                DiffReviewRow::Removed(_) => (removed_color, Some(removed_bg)),
                DiffReviewRow::Omitted(_) => (omitted_color, None),
                DiffReviewRow::Blank => continue,
            };
            if let Some(bg_color) = bg {
                self.rect_pipeline.push_rect(
                    layout.content.x,
                    line_y,
                    layout.content.width,
                    layout.line_height,
                    bg_color,
                );
            }
            let display = row.display_text();
            for (column, character) in display
                .chars()
                .skip(info.horizontal_offset)
                .take(layout.columns_that_fit)
                .enumerate()
            {
                if !character.is_whitespace() {
                    self.text_pipeline.push_char(
                        character,
                        layout.content.x + column as f32 * char_scale,
                        line_y,
                        color,
                    );
                }
            }
        }

        let (mouse_x, mouse_y) = info.mouse_pos.unwrap_or((-1.0, -1.0));
        let copy_hovered = info.awaiting && layout.copy_button.contains(mouse_x, mouse_y);
        let done_hovered = info.awaiting && layout.done_button.contains(mouse_x, mouse_y);
        {
            let mut ctx = DrawCtx {
                rect: &mut self.rect_pipeline,
                text: &mut self.text_pipeline,
                cell_width,
                cell_height,
                ui: &self.ui,
            };
            ctx.draw_scrollbar(
                layout.scrollbar_x,
                layout.content.y,
                layout.content.height,
                info.total_rows,
                layout.rows_that_fit,
                info.scroll_offset,
                anim,
                4.0,
            );
            for (button, label) in [
                (layout.stage_button, "Stage"),
                (layout.revert_button, "Revert"),
            ] {
                let Some(button) = button else {
                    continue;
                };
                let hovered = info.awaiting && button.contains(mouse_x, mouse_y);
                ctx.draw_button(
                    button.x,
                    button.y,
                    button.width,
                    button.height,
                    label,
                    false,
                    hovered,
                    hovered && info.mouse_pressed,
                    anim,
                );
            }
            ctx.draw_button(
                layout.copy_button.x,
                layout.copy_button.y,
                layout.copy_button.width,
                layout.copy_button.height,
                "Copy",
                false,
                copy_hovered,
                copy_hovered && info.mouse_pressed,
                anim,
            );
            ctx.draw_button(
                layout.done_button.x,
                layout.done_button.y,
                layout.done_button.width,
                layout.done_button.height,
                "Done",
                true,
                done_hovered,
                done_hovered && info.mouse_pressed,
                anim,
            );
        }

        let first = info
            .scroll_offset
            .saturating_add(1)
            .min(info.total_rows.max(1));
        let last = info
            .scroll_offset
            .saturating_add(layout.rows_that_fit)
            .min(info.total_rows);
        let hint = if layout.stage_button.is_some() {
            format!(
                "{first}-{last}/{} | S stage | R revert | C copy | Esc done",
                info.total_rows
            )
        } else {
            format!(
                "{first}-{last}/{} c{} | arrows scroll | C copy | Enter/Esc done",
                info.total_rows,
                info.horizontal_offset + 1
            )
        };
        let hint: String = hint
            .chars()
            .take(((card_width - padding * 2.0) / char_scale).floor().max(1.0) as usize)
            .collect();
        let hint_color = with_alpha(self.ui.palette_text_dim, 0.5 * anim);
        let hint_x = card_x + (card_width - hint.chars().count() as f32 * char_scale) / 2.0;
        let hint_y = layout.done_button.y + layout.done_button.height + cell_height * 0.45;
        for (index, character) in hint.chars().enumerate() {
            self.text_pipeline.push_char(
                character,
                hint_x + index as f32 * char_scale,
                hint_y,
                hint_color,
            );
        }
    }

    /// Build the context picker overlay
    pub(super) fn build_context_picker(
        &mut self,
        info: &ContextPickerInfo,
        window_width: f32,
        window_height: f32,
    ) {
        use crate::ui::context_picker::ContextPickerPhase;

        let snapshot = &info.snapshot;
        let anim = snapshot.animation;
        let cell_width = self.cell_width;
        let cell_height = self.cell_height;
        let layout = info.layout;
        let char_scale = layout.char_scale;

        // Scrim
        let scrim = scale_alpha(self.ui.palette_scrim, anim);
        self.rect_pipeline
            .push_rect(0.0, 0.0, window_width, window_height, scrim);

        let card_x = layout.card.x;
        let card_y = layout.card.y;
        let card_width = layout.card.width;
        let card_height = layout.card.height;
        let padding = layout.padding;

        // Card background
        let card_bg = with_alpha(self.ui.palette_bg, anim);
        self.rect_pipeline.push_rounded_rect(
            card_x,
            card_y,
            card_width,
            card_height,
            card_bg,
            CARD_CORNER_RADIUS,
        );

        // Card border
        let border = with_alpha(self.ui.palette_border, 0.4 * anim);
        self.rect_pipeline
            .push_rounded_rect(card_x, card_y, card_width, 1.0, border, 0.0);

        // Title
        let title_y = card_y + padding;
        let title = match snapshot.phase {
            ContextPickerPhase::Sources => "Insert Context",
            ContextPickerPhase::FilePaths => "Choose Workspace Files",
            ContextPickerPhase::Gathering => "Gathering Context",
        };
        let title_color = with_alpha(self.ui.palette_accent, anim);
        let title_x = card_x + (card_width - title.chars().count() as f32 * cell_width) / 2.0;
        for (i, c) in title.chars().enumerate() {
            self.text_pipeline
                .push_char(c, title_x + i as f32 * cell_width, title_y, title_color);
        }

        let content_start_y = layout.content.y;
        let text_color = with_alpha(self.ui.palette_text, anim);
        let dim_color = with_alpha(self.ui.palette_text_dim, 0.5 * anim);
        let highlight_color = with_alpha(self.ui.palette_accent, 0.12 * anim);
        let check_color = with_alpha(self.ui.palette_accent, anim);

        match snapshot.phase {
            ContextPickerPhase::Sources => {
                for (visible_row, (source_index, source)) in snapshot
                    .sources
                    .iter()
                    .enumerate()
                    .skip(snapshot.scroll_offset)
                    .take(layout.visible_count)
                    .enumerate()
                {
                    let line_y = content_start_y + visible_row as f32 * layout.row_height;
                    if source_index == snapshot.selected_index {
                        self.rect_pipeline.push_rounded_rect(
                            layout.content.x,
                            line_y,
                            layout.content.width,
                            layout.row_height,
                            highlight_color,
                            4.0,
                        );
                    }
                    let checkbox = if source.selected { "[✓] " } else { "[ ] " };
                    let color = if source.available {
                        check_color
                    } else {
                        dim_color
                    };
                    let display = format!("{checkbox}{}", source.label);
                    for (column, character) in display
                        .chars()
                        .take((layout.content.width / char_scale).floor() as usize)
                        .enumerate()
                    {
                        if !character.is_whitespace() {
                            self.text_pipeline.push_char(
                                character,
                                layout.content.x + column as f32 * char_scale,
                                line_y + (layout.row_height - cell_height) / 2.0,
                                color,
                            );
                        }
                    }
                }
            }
            ContextPickerPhase::FilePaths => {
                let help = "Comma-separated paths inside the working directory:";
                for (column, character) in help.chars().enumerate() {
                    if column as f32 * char_scale >= layout.content.width {
                        break;
                    }
                    if !character.is_whitespace() {
                        self.text_pipeline.push_char(
                            character,
                            layout.content.x + column as f32 * char_scale,
                            layout.content.y,
                            dim_color,
                        );
                    }
                }
                self.rect_pipeline.push_rounded_rect(
                    layout.file_input.x,
                    layout.file_input.y,
                    layout.file_input.width,
                    layout.file_input.height,
                    with_alpha(self.ui.palette_input_bg, anim),
                    BUTTON_CORNER_RADIUS,
                );
                let columns = (layout.file_input.width / char_scale).floor().max(1.0) as usize;
                let total = snapshot.file_input.chars().count();
                let visible: String = snapshot
                    .file_input
                    .chars()
                    .skip(total.saturating_sub(columns))
                    .collect();
                for (column, character) in visible.chars().enumerate() {
                    if !character.is_whitespace() {
                        self.text_pipeline.push_char(
                            character,
                            layout.file_input.x + column as f32 * char_scale,
                            layout.file_input.y + (layout.file_input.height - cell_height) / 2.0,
                            text_color,
                        );
                    }
                }
            }
            ContextPickerPhase::Gathering => {
                let status = snapshot
                    .status
                    .as_deref()
                    .unwrap_or("Gathering bounded context…");
                let visible: String = status
                    .chars()
                    .take((layout.content.width / char_scale).floor() as usize)
                    .collect();
                let x = layout.content.x
                    + (layout.content.width - visible.chars().count() as f32 * char_scale) / 2.0;
                let y = layout.content.y + (layout.content.height - cell_height) / 2.0;
                for (column, character) in visible.chars().enumerate() {
                    if !character.is_whitespace() {
                        self.text_pipeline.push_char(
                            character,
                            x + column as f32 * char_scale,
                            y,
                            text_color,
                        );
                    }
                }
            }
        }

        let (mouse_x, mouse_y) = info.mouse_pos.unwrap_or((-1.0, -1.0));
        let cancel_hovered = snapshot.awaiting && layout.cancel_button.contains(mouse_x, mouse_y);
        let primary_enabled = snapshot.awaiting && snapshot.phase != ContextPickerPhase::Gathering;
        let primary_hovered = primary_enabled && layout.primary_button.contains(mouse_x, mouse_y);
        {
            let mut ctx = DrawCtx {
                rect: &mut self.rect_pipeline,
                text: &mut self.text_pipeline,
                cell_width,
                cell_height,
                ui: &self.ui,
            };
            if snapshot.phase == ContextPickerPhase::Sources {
                ctx.draw_scrollbar(
                    layout.scrollbar_x,
                    layout.content.y,
                    layout.content.height,
                    snapshot.sources.len(),
                    layout.visible_count,
                    snapshot.scroll_offset,
                    anim,
                    4.0,
                );
            }
            ctx.draw_button(
                layout.cancel_button.x,
                layout.cancel_button.y,
                layout.cancel_button.width,
                layout.cancel_button.height,
                snapshot.cancel_label(),
                false,
                cancel_hovered,
                cancel_hovered && info.mouse_pressed,
                anim,
            );
            ctx.draw_button(
                layout.primary_button.x,
                layout.primary_button.y,
                layout.primary_button.width,
                layout.primary_button.height,
                snapshot.primary_label(),
                true,
                primary_hovered,
                primary_hovered && info.mouse_pressed,
                if primary_enabled { anim } else { anim * 0.4 },
            );
        }

        let hint = snapshot.status.as_deref().unwrap_or(match snapshot.phase {
            ContextPickerPhase::Sources => {
                "↑↓ Navigate · Space Toggle/Edit · Enter Gather · Esc Cancel"
            }
            ContextPickerPhase::FilePaths => "Type paths · Enter Save · Esc Back",
            ContextPickerPhase::Gathering => "Esc cancels this request",
        });
        let hint: String = hint
            .chars()
            .take(((card_width - padding * 2.0) / char_scale).floor().max(1.0) as usize)
            .collect();
        let hint_color = with_alpha(self.ui.palette_text_dim, 0.5 * anim);
        let hint_x = card_x + (card_width - hint.chars().count() as f32 * char_scale) / 2.0;
        let hint_y = layout.cancel_button.y + layout.cancel_button.height + cell_height * 0.45;
        for (i, c) in hint.chars().enumerate() {
            self.text_pipeline
                .push_char(c, hint_x + i as f32 * char_scale, hint_y, hint_color);
        }
    }

    /// Build the guardrail confirmation overlay
    pub(super) fn build_guardrail_confirmation(
        &mut self,
        info: &GuardrailInfo,
        window_width: f32,
        window_height: f32,
    ) {
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
            ctx.draw_scrim(
                window_width,
                window_height,
                self.ui.palette_scrim,
                scrim_alpha * anim,
            );

            // Draw the severity label via DrawCtx helpers so all DrawCtx fields
            // (`text`, `cell_width`, `cell_height`, `ui`) are exercised.
            let severity_label = if info.is_critical {
                "BLOCKED"
            } else {
                "REVIEW"
            };
            let label_color = with_alpha(ctx.ui.palette_text_dim, 0.12 * anim);
            let label_y = window_height * 0.15;
            ctx.draw_centered_text(severity_label, window_width / 2.0, label_y, label_color);
            // Thin separator line under the scrim label, spaced by cell_height
            let sep_color = with_alpha(ctx.ui.palette_border, 0.06 * anim);
            ctx.draw_separator(
                window_width * 0.3,
                label_y + ctx.cell_height,
                window_width * 0.4,
                sep_color,
            );
        }

        // Card geometry — shared with pointer and accessibility hit targets.
        let layout =
            GuardrailLayout::calculate(window_width, window_height, cell_width, cell_height, anim);
        let padding = layout.padding;
        let title_height = layout.title_height;
        let cmd_height = layout.command_height;
        let reason_height = layout.reason_height;
        let card_height = layout.card.height;
        let card_width = layout.card.width;
        let card_x = layout.card.x;
        let card_y = layout.card.y;

        // The card is the theme's surface tinted by the severity's hue, so it
        // reads correctly on light and dark themes alike.
        let severity_color = if info.is_critical {
            self.ui.error
        } else {
            self.ui.warning
        };
        let card_bg = with_alpha(mix(self.ui.palette_bg, severity_color, 0.12), anim);
        let border_color = with_alpha(severity_color, 0.8 * anim);

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
            ctx.draw_card(
                card_x,
                card_y,
                card_width,
                card_height,
                card_bg,
                border_color,
            );

            // severity-colored top accent bar
            ctx.rect
                .push_rounded_rect(card_x, card_y, card_width, 3.0, border_color, 0.0);

            // Title — centered with char_scale
            let title_y = card_y + padding;
            let icon = if info.is_critical {
                "DANGEROUS COMMAND"
            } else {
                "Caution"
            };
            ctx.draw_centered_text_scaled(
                icon,
                card_x + card_width / 2.0,
                title_y,
                border_color,
                char_scale,
            );

            // Command display — clipped
            let cmd_y = title_y + title_height;
            let cmd_label = format!("> {}", info.command);
            let cmd_color = with_alpha(ctx.ui.palette_text, anim);
            ctx.draw_clipped_text_scaled(
                &cmd_label,
                card_x + padding,
                cmd_y,
                cmd_color,
                char_scale,
                card_x + card_width - padding,
            );

            // Reason text — clipped
            let reason_y = cmd_y + cmd_height;
            let reason_color = with_alpha(ctx.ui.palette_text_dim, 0.8 * anim);
            ctx.draw_clipped_text_scaled(
                &info.reason,
                card_x + padding,
                reason_y,
                reason_color,
                char_scale,
                card_x + card_width - padding,
            );

            // Context line — clipped at cell_width scale (uses draw_clipped_text)
            let context_y = reason_y + reason_height;
            let context_label = format!("Where: {} · As: {}", info.target, info.user);
            let context_color = with_alpha(ctx.ui.palette_text_dim, 0.6 * anim);
            ctx.draw_clipped_text(
                &context_label,
                card_x + padding,
                context_y,
                context_color,
                card_x + card_width - padding,
            );
            // Also render separator text via draw_text_scaled
            let mid_sep_color = with_alpha(ctx.ui.palette_border, 0.15 * anim);
            ctx.draw_text_scaled(
                "---",
                card_x + padding,
                context_y + cell_height,
                mid_sep_color,
                char_scale,
            );

            // Close icon in top-right corner
            let close_color = with_alpha(ctx.ui.palette_text_dim, 0.4 * anim);
            // The close cross spans 9 of its icon's 16 units.
            draw_icon(
                ctx.rect,
                Icon::Close,
                [card_x + card_width - padding, card_y + padding],
                cell_height * 0.4 * 16.0 / 9.0,
                close_color,
            );

            // Footer hint keeps the keyboard path discoverable without making
            // Enter the visually preferred destructive choice.
            let footer_y = layout.cancel_button.y - cell_height * 1.25;
            let hint_color = with_alpha(ctx.ui.palette_text_dim, 0.5 * anim);
            ctx.draw_centered_text_scaled(
                "Enter/Esc Cancel  Ctrl+Enter Run anyway",
                card_x + card_width / 2.0,
                footer_y,
                hint_color,
                char_scale,
            );

            // Draw a scrollbar (zero items so it won't actually show, but wires the method)
            ctx.draw_scrollbar(
                card_x + card_width - 6.0,
                card_y + padding,
                card_height - padding * 2.0,
                0,
                0,
                0,
                anim,
                4.0,
            );

            let cancel = layout.cancel_button;
            ctx.draw_button(
                cancel.x,
                cancel.y,
                cancel.width,
                cancel.height,
                "Cancel",
                true,
                false,
                false,
                anim,
            );
            let confirm = layout.confirm_button;
            ctx.draw_button(
                confirm.x,
                confirm.y,
                confirm.width,
                confirm.height,
                if info.is_critical {
                    "Run anyway"
                } else {
                    "Confirm"
                },
                false,
                false,
                false,
                anim,
            );
        }
    }
}
