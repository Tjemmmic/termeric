//! Overlay rendering — diff review, context picker, guardrail confirmation
//!
//! These are centered modal overlays that dim the background with a scrim.
//! Their chrome text is drawn in the UI face; text addressed by column
//! (diff rows, the file-path field, and the guarded command) keeps the
//! monospace grid it is laid out and hit-tested in.

use super::Renderer;
use super::draw::ButtonState;
use super::icons::draw_icon;
use super::types::*;
use crate::ui::color::{mix, scale_alpha, with_alpha};
use crate::ui::constants::*;
use crate::ui::dialog::ButtonRole;
use crate::ui::geometry::Bounds;
use crate::ui::icon::Icon;

/// How wide a card's scrollbar track is, in logical pixels.
const CARD_SCROLLBAR_WIDTH: f32 = 4.0;

impl Renderer {
    /// Build the diff review overlay
    pub(super) fn build_diff_review(
        &mut self,
        info: &DiffReviewInfo,
        window_width: f32,
        window_height: f32,
    ) {
        use crate::ui::diff_review::{DiffReviewButton, DiffReviewRow};

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

        self.push_card_heading(
            &format!("Changes in {}", info.filename),
            card_x + padding,
            card_y + padding,
            cell_height,
            card_width - padding * 2.0,
            with_alpha(self.ui.palette_text, anim),
        );

        let dim_color = with_alpha(self.ui.palette_text_dim, 0.7 * anim);
        let added_color = with_alpha(self.ui.success, anim);
        let removed_color = with_alpha(self.ui.error, anim);
        let header_color = with_alpha(self.ui.palette_accent, anim);
        let omitted_color = with_alpha(self.ui.warning, anim);
        let added_bg = with_alpha(self.ui.success, 0.15 * anim);
        let removed_bg = with_alpha(self.ui.error, 0.15 * anim);

        for (visible_row, (_, row)) in info.visible_rows.iter().enumerate() {
            let line = layout.row(visible_row);
            let (color, bg) = match row {
                DiffReviewRow::Header(_) => (header_color, None),
                DiffReviewRow::Context(_) => (dim_color, None),
                DiffReviewRow::Added(_) => (added_color, Some(added_bg)),
                DiffReviewRow::Removed(_) => (removed_color, Some(removed_bg)),
                DiffReviewRow::Omitted(_) => (omitted_color, None),
                DiffReviewRow::Blank => continue,
            };
            if let Some(bg_color) = bg {
                self.rect_pipeline
                    .push_rect(line.x, line.y, line.width, line.height, bg_color);
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
                        line.x + column as f32 * char_scale,
                        line.y,
                        color,
                    );
                }
            }
        }

        let (mouse_x, mouse_y) = info.mouse_pos.unwrap_or((-1.0, -1.0));
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
        for button in DiffReviewButton::row(layout.stage_button.is_some()) {
            let Some(bounds) = layout.button(*button) else {
                continue;
            };
            let hovered = info.awaiting && bounds.contains(mouse_x, mouse_y);
            self.draw_button(
                bounds,
                button.label(),
                if *button == DiffReviewButton::Done {
                    ButtonRole::Suggested
                } else {
                    ButtonRole::Neutral
                },
                ButtonState {
                    hovered,
                    pressed: hovered && info.mouse_pressed,
                    focused: info.awaiting && info.focused_button == Some(*button),
                    disabled: false,
                },
                anim,
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
            ContextPickerPhase::Sources => "Insert context",
            ContextPickerPhase::FilePaths => "Choose files",
            ContextPickerPhase::Gathering => "Gathering context…",
        };
        let label_size = self.ui_label_px();
        self.push_card_heading(
            title,
            card_x + padding,
            title_y,
            cell_height,
            card_width - padding * 2.0,
            with_alpha(self.ui.palette_text, anim),
        );

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
                    let row = layout.row(visible_row);
                    if source_index == snapshot.selected_index {
                        self.rect_pipeline.push_rounded_rect(
                            row.x,
                            row.y,
                            row.width,
                            row.height,
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
                        (row.y + (row.height - size) / 2.0).round(),
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
                        row.y,
                        row.height,
                        layout.content.right() - label_x,
                        label_size,
                        color,
                    );
                }
            }
            ContextPickerPhase::FilePaths => {
                self.push_ui_label(
                    "Files in the working folder, separated by commas:",
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
                let status = snapshot.status.as_deref().unwrap_or("Gathering context…");
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
            ButtonRole::Cancel,
            ButtonState {
                hovered: cancel_hovered,
                pressed: cancel_hovered && info.mouse_pressed,
                ..ButtonState::default()
            },
            anim,
        );
        self.draw_button(
            layout.primary_button,
            snapshot.primary_label(),
            ButtonRole::Suggested,
            ButtonState {
                hovered: primary_hovered,
                pressed: primary_hovered && info.mouse_pressed,
                focused: false,
                disabled: !primary_enabled,
            },
            anim,
        );

        // What the picker is doing or why it stopped; never a key list.
        if let Some(status) = snapshot.status.as_deref() {
            self.push_ui_label_centered(
                status,
                layout.status.center()[0],
                layout.status.y,
                layout.status.height,
                layout.status.width,
                label_size,
                with_alpha(self.ui.dialog_text, anim),
            );
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
        self.push_card_heading(
            if info.is_critical {
                "Run a dangerous command?"
            } else {
                "Run this command?"
            },
            card.x + padding,
            title_y,
            cell_height,
            text_width,
            with_alpha(self.ui.palette_text, anim),
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

        // Enter and Escape cancel, so Cancel shows the focus ring; only
        // Ctrl+Enter or a press runs the command.
        self.draw_button(
            layout.cancel_button,
            "Cancel",
            ButtonRole::Cancel,
            ButtonState {
                focused: true,
                ..ButtonState::default()
            },
            anim,
        );
        self.draw_button(
            layout.confirm_button,
            if info.is_critical {
                "Run anyway"
            } else {
                "Run"
            },
            if info.is_critical {
                ButtonRole::Destructive
            } else {
                ButtonRole::Neutral
            },
            ButtonState::default(),
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
    use crate::ui::diff_review::DiffReviewLayout;

    #[test]
    fn a_cards_scrollbar_track_scales_within_the_gutter_beside_its_content() {
        for scale in [1.0_f32, 1.5, 2.0, 3.0] {
            // The smallest font's cells, at this scale.
            let (cell_width, cell_height) = (6.4 * scale, 13.0 * scale);
            let width = super::super::physical_px(CARD_SCROLLBAR_WIDTH, scale);
            let diff = DiffReviewLayout::calculate(
                2_000.0 * scale,
                1_200.0 * scale,
                cell_width,
                cell_height,
                1.0,
                "fixture.rs",
                1_000,
                80,
                true,
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
                (diff.card, diff.padding, diff.content),
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
