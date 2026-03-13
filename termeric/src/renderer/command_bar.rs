//! Command bar and autocomplete dropdown rendering

use super::{Renderer, adjust_alpha, shade};
use super::types::*;
use crate::ui::constants::*;
use crate::ui::command_palette::CommandInputPhase;

impl Renderer {
    pub(super) fn build_command_bar(&mut self, info: &CommandBarInfo, window_width: f32, window_height: f32, bar_height: f32) {
        let padding = self.padding.max(8.0);
        let bar_y = window_height - bar_height;

        let bar_bg = self.ui.command_bg;
        self.rect_pipeline.push_rect(0.0, bar_y, window_width, bar_height, bar_bg);

        let border_color = adjust_alpha(self.ui.command_border, 0.8);
        self.rect_pipeline.push_rect(0.0, bar_y, window_width, 1.0, border_color);

        // Render message and/or input
        let text_y = bar_y + padding;
        let mut text_x = padding;

        // Reserve space for input line if we're typing
        let has_input = !info.input.is_empty();
        let input_line_height = if has_input { self.cell_height + padding } else { 0.0 };

        if let Some(msg) = &info.message {
            // Show message (supports multi-line and scrolling)
            let text_color = if info.is_error {
                self.ui.command_error
            } else {
                self.ui.command_success
            };

            let mut current_x = text_x;
            let max_y = bar_y + bar_height - padding - input_line_height; // Leave room for input if typing
            let mut line_number = 0; // Track which line we're on (accounting for wraps)
            let scroll_offset = info.message_scroll_offset;
            let mut visible_line_y = text_y; // Y position for rendering (after applying scroll)

            for c in msg.chars() {
                if c == '\n' {
                    // Move to next line
                    line_number += 1;
                    current_x = text_x;
                    // Update visible Y if we're past scroll offset
                    if line_number > scroll_offset {
                        visible_line_y += self.cell_height;
                        if visible_line_y + self.cell_height > max_y {
                            break;
                        }
                    }
                    continue;
                }

                // Word wrap if line is too long
                if current_x + self.cell_width > window_width - padding {
                    line_number += 1;
                    current_x = text_x;
                    if line_number > scroll_offset {
                        visible_line_y += self.cell_height;
                        if visible_line_y + self.cell_height > max_y {
                            break;
                        }
                    }
                }

                // Only render if we're past the scroll offset
                if line_number >= scroll_offset {
                    self.text_pipeline.push_char(c, current_x, visible_line_y, text_color);
                }
                current_x += self.cell_width; // Use same spacing as terminal for readability
            }
        }

        // Show command input (always show if there's input, positioned at bottom when message is present)
        if has_input || info.message.is_none() {
            let input_text_y = if info.message.is_some() && has_input {
                // Position at bottom when message is showing
                bar_y + bar_height - self.cell_height - padding
            } else {
                // Center text vertically when no message
                bar_y + (bar_height - self.cell_height) / 2.0
            };

            text_x = padding; // Reset x position

            let (prompt_text, prompt_color) = match info.phase {
                crate::ui::command_palette::CommandInputPhase::SelectingCommand => {
                    ("> ", self.ui.prompt_selecting)
                }
                crate::ui::command_palette::CommandInputPhase::EnteringArguments => {
                    (": ", self.ui.prompt_args)
                }
                crate::ui::command_palette::CommandInputPhase::BrowsingHistory => {
                    ("⏮ ", self.ui.prompt_history)
                }
            };
            let text_color = self.ui.command_text;

            // Draw prompt
            for c in prompt_text.chars() {
                self.text_pipeline.push_char(c, text_x, input_text_y, prompt_color);
                text_x += self.cell_width;
            }
            text_x += self.cell_width * 0.2; // Small space after prompt

            // Draw input text
            let mut char_index = 0;
            for c in info.input.chars() {
                if text_x + self.cell_width > window_width - padding {
                    break;
                }

                // Draw cursor at current position
                if char_index == info.cursor_pos {
                    let cursor_color = adjust_alpha(self.ui.cursor, 0.8);
                    self.rect_pipeline.push_rect(
                        text_x, input_text_y,
                        2.0, self.cell_height,
                        cursor_color,
                    );
                }

                self.text_pipeline.push_char(c, text_x, input_text_y, text_color);
                text_x += self.cell_width;
                char_index += c.len_utf8();
            }

            // Draw cursor at end if at end of input
            if info.cursor_pos >= info.input.len() {
                let cursor_color = adjust_alpha(self.ui.cursor, 0.8);
                self.rect_pipeline.push_rect(
                    text_x, input_text_y,
                    2.0, self.cell_height,
                    cursor_color,
                );
            }

            // Phase-aware hint text on the right (only when not showing message)
            if info.message.is_none() {
                let hint = match info.phase {
                    crate::ui::command_palette::CommandInputPhase::SelectingCommand => {
                        "↑↓ Navigate  ⭾ Complete  Ctrl+↑↓ History  Esc Cancel"
                    }
                    crate::ui::command_palette::CommandInputPhase::EnteringArguments => {
                        "↑↓ Navigate  ⭾ Complete  ⏎ Execute  Esc Cancel"
                    }
                    crate::ui::command_palette::CommandInputPhase::BrowsingHistory => {
                        "Ctrl+↑↓ Browse  ⏎ Select  Esc Cancel"
                    }
                };
                let hint_color = self.ui.command_hint;
                let hint_width = hint.chars().count() as f32 * self.cell_width * 0.6;
                let mut hint_x = window_width - hint_width - padding;

                for c in hint.chars() {
                    self.text_pipeline.push_char(c, hint_x, input_text_y, hint_color);
                    hint_x += self.cell_width * 0.6;
                }
            }
        }

        // Render autocomplete dropdown above command bar
        if !info.suggestions.is_empty() {
            self.build_autocomplete_dropdown(info, bar_y, window_width, self.tab_bar_height());
        }
    }

    /// Build autocomplete dropdown above command bar
    pub(super) fn build_autocomplete_dropdown(&mut self, info: &CommandBarInfo, command_bar_y: f32, window_width: f32, tab_bar_height: f32) {
        const SUGGESTION_HEIGHT: f32 = 28.0; // Slightly more compact
        const PADDING: f32 = PANEL_PADDING; // More padding for breathing room
        const SPACING: f32 = 1.0; // Tighter spacing between items
        const MARGIN: f32 = 16.0; // Margin from window edges
        const BORDER_WIDTH: f32 = 1.0;

        // Calculate how many suggestions can fit above the command bar
        let available_height = command_bar_y - tab_bar_height - 8.0; // 8px gap from tab bar
        let max_suggestions_that_fit = ((available_height - PADDING * 2.0 - BORDER_WIDTH * 2.0) / (SUGGESTION_HEIGHT + SPACING)).floor() as usize;
        let num_suggestions = info.suggestions.len().min(6).min(max_suggestions_that_fit.max(1));

        // Dropdown dimensions - centered with margins
        let dropdown_width = window_width - MARGIN * 2.0;
        let dropdown_height = num_suggestions as f32 * (SUGGESTION_HEIGHT + SPACING) - SPACING + PADDING * 2.0 + BORDER_WIDTH * 2.0;
        let dropdown_x = MARGIN;
        let dropdown_y = command_bar_y - dropdown_height - 8.0; // 8px gap above command bar

        // Shadow effect - draw multiple slightly offset darker rects
        let shadow_color = self.ui.dropdown_shadow;
        for i in 1..=3 {
            let offset = i as f32 * 2.0;
            self.rect_pipeline.push_rounded_rect(
                dropdown_x + offset,
                dropdown_y + offset,
                dropdown_width,
                dropdown_height,
                shadow_color,
                8.0,
            );
        }

        // Main dropdown background - solid dark background
        let dropdown_bg = self.ui.dropdown_bg;
        self.rect_pipeline.push_rounded_rect(dropdown_x, dropdown_y, dropdown_width, dropdown_height, dropdown_bg, 8.0);

        let border_color = adjust_alpha(self.ui.dropdown_border, 0.6);
        // Top border
        self.rect_pipeline.push_rect(dropdown_x, dropdown_y, dropdown_width, BORDER_WIDTH, border_color);
        // Bottom border
        self.rect_pipeline.push_rect(dropdown_x, dropdown_y + dropdown_height - BORDER_WIDTH, dropdown_width, BORDER_WIDTH, border_color);
        // Left border
        self.rect_pipeline.push_rect(dropdown_x, dropdown_y, BORDER_WIDTH, dropdown_height, border_color);
        // Right border
        self.rect_pipeline.push_rect(dropdown_x + dropdown_width - BORDER_WIDTH, dropdown_y, BORDER_WIDTH, dropdown_height, border_color);

        // Render each suggestion (only those that fit)
        for (idx, suggestion) in info.suggestions.iter().take(num_suggestions).enumerate() {
            let suggestion_y = dropdown_y + BORDER_WIDTH + PADDING + idx as f32 * (SUGGESTION_HEIGHT + SPACING);
            let is_selected = idx == info.selected_suggestion;

            // Selection highlight with rounded effect
            if is_selected {
                let selection_color = adjust_alpha(self.ui.cursor, 0.2);
                let highlight_margin = 4.0;
                self.rect_pipeline.push_rounded_rect(
                    dropdown_x + highlight_margin,
                    suggestion_y,
                    dropdown_width - highlight_margin * 2.0,
                    SUGGESTION_HEIGHT,
                    selection_color,
                    4.0,
                );

                // Left accent bar for selected item
                let accent_color = self.ui.cursor;
                let accent_width = 3.0;
                self.rect_pipeline.push_rounded_rect(
                    dropdown_x + highlight_margin,
                    suggestion_y,
                    accent_width,
                    SUGGESTION_HEIGHT,
                    accent_color,
                    2.0,
                );
            }

            let text_y = suggestion_y + (SUGGESTION_HEIGHT - self.cell_height) / 2.0;
            let mut text_x = dropdown_x + PADDING + (if is_selected { 10.0 } else { 6.0 });

            // Category icon/prefix
            let category_color = if is_selected {
                self.ui.cursor
            } else {
                adjust_alpha(self.ui.prompt_args, 0.7)
            };

            let category_icon = match suggestion.category {
                "Theme" => "◆",
                "Font" => "A",
                "Config" => "⚙",
                "Help" => "?",
                _ => "▸",
            };

            for c in category_icon.chars() {
                self.text_pipeline.push_char(c, text_x, text_y, category_color);
                text_x += self.cell_width; // Standard character width
            }
            text_x += self.cell_width * 0.5; // Space after icon

            // Command name
            let name_color = if is_selected {
                self.ui.foreground
            } else {
                self.ui.command_text
            };

            for c in suggestion.name.chars() {
                self.text_pipeline.push_char(c, text_x, text_y, name_color);
                text_x += self.cell_width;
            }

            // Alias (if present)
            if let Some(alias) = suggestion.alias {
                text_x += self.cell_width * 0.5;
                let alias_color = adjust_alpha(self.ui.command_text, 0.7);

                self.text_pipeline.push_char('(', text_x, text_y, alias_color);
                text_x += self.cell_width;

                for c in alias.chars() {
                    self.text_pipeline.push_char(c, text_x, text_y, alias_color);
                    text_x += self.cell_width;
                }

                self.text_pipeline.push_char(')', text_x, text_y, alias_color);
                text_x += self.cell_width;
            }

            // Description (right side) - add some spacing
            text_x += self.cell_width * 2.0; // Add spacing before description
            let max_desc_x = dropdown_x + dropdown_width - PADDING - 4.0; // Leave margin on right

            let desc_color = if is_selected {
                adjust_alpha(self.ui.command_text, 0.95)
            } else {
                adjust_alpha(self.ui.command_text, 0.75)
            };

            // Add separator before description
            let separator = "—";
            for c in separator.chars() {
                if text_x + self.cell_width < max_desc_x {
                    self.text_pipeline.push_char(c, text_x, text_y, adjust_alpha(self.ui.command_text, 0.5));
                    text_x += self.cell_width * 0.8;
                }
            }
            text_x += self.cell_width * 0.5; // Small gap after separator

            for c in suggestion.description.chars() {
                if text_x + self.cell_width * 3.0 > max_desc_x {
                    // Add ellipsis if we're running out of space
                    if text_x + self.cell_width * 1.5 <= max_desc_x {
                        self.text_pipeline.push_char('.', text_x, text_y, desc_color);
                        text_x += self.cell_width * 0.4;
                        self.text_pipeline.push_char('.', text_x, text_y, desc_color);
                        text_x += self.cell_width * 0.4;
                        self.text_pipeline.push_char('.', text_x, text_y, desc_color);
                    }
                    break;
                }

                self.text_pipeline.push_char(c, text_x, text_y, desc_color);
                text_x += self.cell_width * 0.95; // Slightly compressed for descriptions
            }
        }
    }

}
