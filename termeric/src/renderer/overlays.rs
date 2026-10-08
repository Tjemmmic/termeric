//! Overlay rendering — info overlay, diff review, context picker, guardrail confirmation
//!
//! These are centered modal overlays that dim the background with a scrim.
//! Their chrome text is drawn in the UI face; text addressed by column (the
//! info overlay's lines and field, diff rows, the file-path field, and the
//! guarded command) keeps the monospace grid it is laid out and hit-tested in.

use super::Renderer;
use super::icons::draw_icon;
use super::types::*;
use crate::ui::color::{mix, scale_alpha, with_alpha};
use crate::ui::constants::*;
use crate::ui::geometry::Bounds;
use crate::ui::icon::Icon;

/// How wide a card's scrollbar track is, in logical pixels.
const CARD_SCROLLBAR_WIDTH: f32 = 4.0;

impl Renderer {
    /// A border-width rule across `width` whose outer sixths fade out.
    fn draw_faded_separator(&mut self, x: f32, y: f32, width: f32, anim: f32) {
        let fade_width = width * 0.15;
        let thickness = self.px(BORDER_WIDTH);
        let center = with_alpha(self.ui.palette_border, 0.25 * anim);
        let faded = with_alpha(self.ui.palette_border, 0.08 * anim);
        self.rect_pipeline.push_rect(
            x + fade_width,
            y,
            width - fade_width * 2.0,
            thickness,
            center,
        );
        self.rect_pipeline
            .push_rect(x, y, fade_width, thickness, faded);
        self.rect_pipeline
            .push_rect(x + width - fade_width, y, fade_width, thickness, faded);
    }

    /// Build the info overlay (error/info centered card with Copy/Continue buttons)
    pub(super) fn build_info_overlay(
        &mut self,
        info: &InfoOverlayInfo,
        window_width: f32,
        window_height: f32,
    ) {
        let anim = info.animation;
        let cell_height = self.cell_height();

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
        let padding = layout.padding;
        let line_height = layout.line_height;
        let content_height = layout.content.height;
        let char_scale = layout.char_scale;
        self.draw_card_shadow(layout.card, 0.08 * anim, 3);
        self.draw_card(
            layout.card,
            with_alpha(self.ui.palette_bg, anim),
            with_alpha(self.ui.palette_border, 0.3 * anim),
        );

        // ── Accent line along the top, clear of the corners ──
        self.draw_card_accent(
            layout.card,
            with_alpha(self.ui.palette_accent, 0.2 * anim),
            2.0,
        );

        // ── Title — centered, accent ──
        let title_y = card_y + padding;
        let label_size = self.ui_label_px();
        self.push_ui_label_centered(
            &info.title,
            card_x + card_width / 2.0,
            title_y,
            cell_height,
            card_width - padding * 2.0,
            label_size,
            with_alpha(self.ui.palette_accent, anim),
        );

        let separator_width = (card_width - padding * 2.0).max(1.0);
        self.draw_faded_separator(
            card_x + padding,
            title_y + cell_height * 1.5,
            separator_width,
            anim,
        );

        // ── Content lines ──
        let content_start_y = layout.content.y;
        let text_color = with_alpha(self.ui.palette_text, 0.9 * anim);
        let dim_color = with_alpha(self.ui.palette_text_dim, 0.7 * anim);
        let content_x = layout.content.x;
        let (mouse_x, mouse_y) = info.mouse_pos.unwrap_or((-1.0, -1.0));
        // The row a click there would choose: hover and clicks share one
        // half-open geometry, so a boundary pixel lights one row.
        let hovered_row =
            layout.content_row_at(mouse_x, mouse_y, info.scroll_offset, info.total_lines);

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
            let choice_hovered =
                info.awaiting && choice.is_some() && hovered_row == Some(*source_row);
            if input.is_some() {
                self.rect_pipeline.push_rounded_rect(
                    layout.content.x,
                    line_y,
                    layout.content.width,
                    line_height,
                    with_alpha(self.ui.palette_accent, 0.08 * anim),
                    self.px(BUTTON_CORNER_RADIUS),
                );
                let underline = self.px(BORDER_WIDTH);
                self.rect_pipeline.push_rect(
                    layout.content.x,
                    line_y + line_height - underline,
                    layout.content.width,
                    underline,
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
                    self.px(BUTTON_CORNER_RADIUS),
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

        self.draw_scrollbar(
            card_scrollbar_track(
                layout.card,
                layout.padding,
                layout.content,
                self.px(CARD_SCROLLBAR_WIDTH),
            ),
            layout.rows_that_fit,
            info.total_lines,
            info.scroll_offset,
            anim,
        );

        self.draw_faded_separator(
            card_x + padding,
            content_start_y + content_height + cell_height * 0.5,
            separator_width,
            anim,
        );

        // ── Two buttons: "Copy" and "Continue" ──
        let copy_label = "Copy";
        let continue_label = if info.input.is_some() {
            "Save"
        } else {
            "Continue"
        };
        let copy_hovered = info.awaiting && layout.copy_button.contains(mouse_x, mouse_y);
        let continue_hovered = info.awaiting && layout.continue_button.contains(mouse_x, mouse_y);
        self.draw_button(
            layout.copy_button,
            copy_label,
            false,
            copy_hovered,
            copy_hovered && info.mouse_pressed,
            anim,
        );
        self.draw_button(
            layout.continue_button,
            continue_label,
            true,
            continue_hovered,
            continue_hovered && info.mouse_pressed,
            anim,
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
                self.draw_focus_ring(
                    bounds,
                    with_alpha(self.ui.focus_ring, anim),
                    BUTTON_CORNER_RADIUS,
                );
            }
        }

        // ── Keyboard hint ──
        let hint_y = layout.copy_button.bottom() + cell_height * 0.5;
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
        self.push_ui_label_centered(
            &hint_text,
            card_x + card_width / 2.0,
            hint_y,
            cell_height,
            card_width - padding * 2.0,
            label_size,
            with_alpha(self.ui.palette_text_dim, 0.35 * anim),
        );
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
        let cell_height = self.cell_height();
        let layout = info.layout;
        let char_scale = layout.char_scale;

        let scrim = scale_alpha(self.ui.palette_scrim, 0.85 * anim);
        self.rect_pipeline
            .push_rect(0.0, 0.0, window_width, window_height, scrim);
        let card_x = layout.card.x;
        let card_y = layout.card.y;
        let card_width = layout.card.width;
        let padding = layout.padding;

        self.draw_card_shadow(layout.card, 0.08 * anim, 3);
        self.draw_card(
            layout.card,
            with_alpha(self.ui.palette_bg, anim),
            with_alpha(self.ui.palette_border, 0.4 * anim),
        );

        let label_size = self.ui_label_px();
        self.push_ui_label(
            &format!("Diff: {}", info.filename),
            card_x + padding,
            card_y + padding,
            cell_height,
            card_width - padding * 2.0,
            label_size,
            with_alpha(self.ui.palette_accent, anim),
        );

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
        self.draw_scrollbar(
            card_scrollbar_track(
                layout.card,
                layout.padding,
                layout.content,
                self.px(CARD_SCROLLBAR_WIDTH),
            ),
            layout.rows_that_fit,
            info.total_rows,
            info.scroll_offset,
            anim,
        );
        for (button, label) in [
            (layout.stage_button, "Stage"),
            (layout.revert_button, "Revert"),
        ] {
            let Some(button) = button else {
                continue;
            };
            let hovered = info.awaiting && button.contains(mouse_x, mouse_y);
            self.draw_button(
                button,
                label,
                false,
                hovered,
                hovered && info.mouse_pressed,
                anim,
            );
        }
        self.draw_button(
            layout.copy_button,
            "Copy",
            false,
            copy_hovered,
            copy_hovered && info.mouse_pressed,
            anim,
        );
        self.draw_button(
            layout.done_button,
            "Done",
            true,
            done_hovered,
            done_hovered && info.mouse_pressed,
            anim,
        );

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
        self.push_ui_label_centered(
            &hint,
            card_x + card_width / 2.0,
            layout.done_button.bottom() + cell_height * 0.5,
            cell_height,
            card_width - padding * 2.0,
            label_size,
            with_alpha(self.ui.palette_text_dim, 0.5 * anim),
        );
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
        let cell_height = self.cell_height();
        let layout = info.layout;
        let char_scale = layout.char_scale;

        // Scrim
        let scrim = scale_alpha(self.ui.palette_scrim, anim);
        self.rect_pipeline
            .push_rect(0.0, 0.0, window_width, window_height, scrim);

        let card_x = layout.card.x;
        let card_y = layout.card.y;
        let card_width = layout.card.width;
        let padding = layout.padding;

        self.draw_card_shadow(layout.card, 0.08 * anim, 3);
        self.draw_card(
            layout.card,
            with_alpha(self.ui.palette_bg, anim),
            with_alpha(self.ui.palette_border, 0.4 * anim),
        );

        // Title
        let title_y = card_y + padding;
        let title = match snapshot.phase {
            ContextPickerPhase::Sources => "Insert Context",
            ContextPickerPhase::FilePaths => "Choose Workspace Files",
            ContextPickerPhase::Gathering => "Gathering Context",
        };
        let label_size = self.ui_label_px();
        self.push_ui_label_centered(
            title,
            card_x + card_width / 2.0,
            title_y,
            cell_height,
            card_width - padding * 2.0,
            label_size,
            with_alpha(self.ui.palette_accent, anim),
        );

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
                            self.px(BUTTON_CORNER_RADIUS),
                        );
                    }
                    let color = if source.available {
                        check_color
                    } else {
                        dim_color
                    };
                    // A checkbox, ticked while the source is chosen.
                    let size = self.px(12.0);
                    let check = Bounds::new(
                        (layout.content.x + self.px(4.0)).round(),
                        (line_y + (layout.row_height - size) / 2.0).round(),
                        size,
                        size,
                    );
                    self.rect_pipeline.push_rounded_outline(
                        check.x,
                        check.y,
                        check.width,
                        check.height,
                        color,
                        self.px(3.0),
                        self.px(BORDER_WIDTH),
                    );
                    if source.selected {
                        let [cx, cy] = check.center();
                        self.push_icon(Icon::Check, cx, cy, color);
                    }
                    let label_x = check.right() + self.px(8.0);
                    self.push_ui_label(
                        &source.label,
                        label_x,
                        line_y,
                        layout.row_height,
                        layout.content.right() - label_x,
                        label_size,
                        color,
                    );
                }
            }
            ContextPickerPhase::FilePaths => {
                self.push_ui_label(
                    "Comma-separated paths inside the working directory:",
                    layout.content.x,
                    layout.content.y,
                    cell_height,
                    layout.content.width,
                    label_size,
                    dim_color,
                );
                self.rect_pipeline.push_rounded_rect(
                    layout.file_input.x,
                    layout.file_input.y,
                    layout.file_input.width,
                    layout.file_input.height,
                    with_alpha(self.ui.palette_input_bg, anim),
                    self.px(BUTTON_CORNER_RADIUS),
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
                self.push_ui_label_centered(
                    status,
                    layout.content.center()[0],
                    layout.content.y,
                    layout.content.height,
                    layout.content.width,
                    label_size,
                    text_color,
                );
            }
        }

        let (mouse_x, mouse_y) = info.mouse_pos.unwrap_or((-1.0, -1.0));
        let cancel_hovered = snapshot.awaiting && layout.cancel_button.contains(mouse_x, mouse_y);
        let primary_enabled = snapshot.awaiting && snapshot.phase != ContextPickerPhase::Gathering;
        let primary_hovered = primary_enabled && layout.primary_button.contains(mouse_x, mouse_y);
        if snapshot.phase == ContextPickerPhase::Sources {
            self.draw_scrollbar(
                card_scrollbar_track(
                    layout.card,
                    layout.padding,
                    layout.content,
                    self.px(CARD_SCROLLBAR_WIDTH),
                ),
                layout.visible_count,
                snapshot.sources.len(),
                snapshot.scroll_offset,
                anim,
            );
        }
        self.draw_button(
            layout.cancel_button,
            snapshot.cancel_label(),
            false,
            cancel_hovered,
            cancel_hovered && info.mouse_pressed,
            anim,
        );
        self.draw_button(
            layout.primary_button,
            snapshot.primary_label(),
            true,
            primary_hovered,
            primary_hovered && info.mouse_pressed,
            if primary_enabled { anim } else { anim * 0.4 },
        );

        let hint = snapshot.status.as_deref().unwrap_or(match snapshot.phase {
            ContextPickerPhase::Sources => {
                "↑↓ Navigate · Space Toggle/Edit · Enter Gather · Esc Cancel"
            }
            ContextPickerPhase::FilePaths => "Type paths · Enter Save · Esc Back",
            ContextPickerPhase::Gathering => "Esc cancels this request",
        });
        self.push_ui_label_centered(
            hint,
            card_x + card_width / 2.0,
            layout.cancel_button.bottom() + cell_height * 0.45,
            cell_height,
            card_width - padding * 2.0,
            label_size,
            with_alpha(self.ui.palette_text_dim, 0.5 * anim),
        );
    }

    /// Build the guardrail confirmation overlay
    pub(super) fn build_guardrail_confirmation(
        &mut self,
        info: &GuardrailInfo,
        window_width: f32,
        window_height: f32,
    ) {
        let anim = info.animation;
        let cell_width = self.cell_width();
        let cell_height = self.cell_height();
        let char_scale = cell_width * CONTENT_CHAR_SCALE;
        let label_size = self.ui_label_px();

        // Scrim — stronger for dangerous operations
        let scrim_alpha = if info.is_critical { 0.6 } else { 0.4 };
        self.rect_pipeline.push_rect(
            0.0,
            0.0,
            window_width,
            window_height,
            scale_alpha(self.ui.palette_scrim, scrim_alpha * anim),
        );

        // Card geometry — shared with pointer and accessibility hit targets.
        let layout = &info.layout;
        let padding = layout.padding;
        let card = layout.card;
        let text_width = card.width - padding * 2.0;

        // The card is the theme's surface tinted by the severity's hue, so it
        // reads correctly on light and dark themes alike.
        let severity_color = if info.is_critical {
            self.ui.error
        } else {
            self.ui.warning
        };
        let card_bg = with_alpha(mix(self.ui.palette_bg, severity_color, 0.12), anim);
        let border_color = with_alpha(severity_color, 0.8 * anim);
        self.draw_card_shadow(card, 0.06 * anim, 2);
        self.draw_card(card, card_bg, border_color);

        // Severity-colored accent line along the top, clear of the corners.
        self.draw_card_accent(card, border_color, 3.0);

        let title_y = card.y + padding;
        self.push_ui_label_centered(
            if info.is_critical {
                "DANGEROUS COMMAND"
            } else {
                "Caution"
            },
            card.center()[0],
            title_y,
            cell_height,
            text_width,
            label_size,
            border_color,
        );

        // The command itself stays in the terminal's monospace grid.
        let cmd_y = title_y + layout.title_height;
        let cmd_color = with_alpha(self.ui.palette_text, anim);
        let mut x = card.x + padding;
        for character in format!("> {}", info.command).chars() {
            if x + char_scale > card.right() - padding {
                break;
            }
            if !character.is_whitespace() {
                self.text_pipeline.push_char(character, x, cmd_y, cmd_color);
            }
            x += char_scale;
        }

        let reason_y = cmd_y + layout.command_height;
        self.push_ui_label(
            &info.reason,
            card.x + padding,
            reason_y,
            cell_height,
            text_width,
            label_size,
            with_alpha(self.ui.palette_text_dim, 0.8 * anim),
        );

        let context_y = reason_y + layout.reason_height;
        self.push_ui_label(
            &format!("Where: {}", info.target),
            card.x + padding,
            context_y,
            cell_height,
            text_width,
            label_size,
            with_alpha(self.ui.palette_text_dim, 0.6 * anim),
        );

        // The close cross spans 9 of its icon's 16 units.
        draw_icon(
            &mut self.rect_pipeline,
            Icon::Close,
            layout.close_button.center(),
            cell_height * 0.4 * 16.0 / 9.0,
            with_alpha(self.ui.palette_text_dim, 0.4 * anim),
        );

        // Footer hint keeps the keyboard path discoverable without making
        // Enter the visually preferred destructive choice.
        self.push_ui_label_centered(
            "Enter/Esc Cancel  Ctrl+Enter Run anyway",
            card.center()[0],
            layout.cancel_button.y - cell_height * 1.25,
            cell_height,
            text_width,
            label_size,
            with_alpha(self.ui.palette_text_dim, 0.5 * anim),
        );

        self.draw_button(layout.cancel_button, "Cancel", true, false, false, anim);
        self.draw_button(
            layout.confirm_button,
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

/// A card's scrollbar track, `width` physical pixels wide: beside its
/// content rows, against the padding inside the card's right edge, in the
/// gutter its layout leaves between the two.
fn card_scrollbar_track(card: Bounds, padding: f32, content: Bounds, width: f32) -> Bounds {
    Bounds::new(
        card.right() - padding - width,
        content.y,
        width,
        content.height,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::context_picker::{ContextPickerLayout, ContextPickerPhase};
    use crate::ui::text_card::TextCard;

    #[test]
    fn a_cards_scrollbar_track_scales_within_the_gutter_beside_its_content() {
        for scale in [1.0_f32, 1.5, 2.0, 3.0] {
            // The smallest font's cells, at this scale.
            let (cell_width, cell_height) = (6.4 * scale, 13.0 * scale);
            let width = super::super::physical_px(CARD_SCROLLBAR_WIDTH, scale);
            let text = TextCard::calculate(
                2_000.0 * scale,
                1_200.0 * scale,
                cell_width,
                cell_height,
                1.0,
                80,
                20,
                380.0,
                1_000,
            );
            let picker = ContextPickerLayout::calculate(
                2_000.0 * scale,
                1_200.0 * scale,
                cell_width,
                cell_height,
                1.0,
                ContextPickerPhase::Sources,
                40,
            );
            for (card, padding, content) in [
                (text.card, text.padding, text.content),
                (picker.card, picker.padding, picker.content),
            ] {
                let track = card_scrollbar_track(card, padding, content, width);
                assert_eq!(track.width, (4.0 * scale).round());
                assert_eq!(track.right(), card.right() - padding);
                assert!(
                    track.x >= content.right(),
                    "{scale}: {track:?} over {content:?}"
                );
                assert_eq!((track.y, track.height), (content.y, content.height));
            }
        }
    }
}
