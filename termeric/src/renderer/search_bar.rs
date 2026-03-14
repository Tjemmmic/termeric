//! Search bar rendering and hit-testing

use super::{Renderer, adjust_alpha};
use super::types::*;
use crate::terminal::search::SearchBarInfo;

impl Renderer {
    pub(super) fn build_search_bar(&mut self, info: &SearchBarInfo, window_width: f32, tab_bar_height: f32) {
        let padding = 8.0;
        let bar_height = self.cell_height + padding * 2.0;
        let bar_y = tab_bar_height;

        // Background - warm dark tone
        let bar_bg = self.ui.search_bg;
        self.rect_pipeline.push_rect(0.0, bar_y, window_width, bar_height, bar_bg);

        // Bottom border
        let border_color = adjust_alpha(self.ui.tab_border, 0.5);
        self.rect_pipeline.push_rect(0.0, bar_y + bar_height - 1.0, window_width, 1.0, border_color);

        let text_y = bar_y + padding;
        let mut text_x = padding;

        // Search icon/prompt
        let prompt = "/";
        let prompt_color = self.ui.search_indicator;
        for c in prompt.chars() {
            self.text_pipeline.push_char(c, text_x, text_y, prompt_color);
            text_x += self.cell_width;
        }
        text_x += self.cell_width * 0.3;

        // Query text
        let text_color = self.ui.search_text;
        let mut char_index = 0;
        for c in info.query.chars() {
            // Draw cursor at current position
            if char_index == info.cursor_pos {
                let cursor_color = adjust_alpha(self.ui.cursor, 0.8);
                self.rect_pipeline.push_rect(
                    text_x, text_y,
                    2.0, self.cell_height,
                    cursor_color,
                );
            }

            self.text_pipeline.push_char(c, text_x, text_y, text_color);
            text_x += self.cell_width;
            char_index += c.len_utf8();
        }

        // Draw cursor at end if at end of query
        if info.cursor_pos >= info.query.len() {
            let cursor_color = adjust_alpha(self.ui.cursor, 0.8);
            self.rect_pipeline.push_rect(
                text_x, text_y,
                2.0, self.cell_height,
                cursor_color,
            );
        }
        text_x += self.cell_width;

        // Mode indicators on the right side
        let mut right_x = window_width - padding;

        // Navigation arrows ▲▼ (only show if there are matches)
        if info.match_count > 0 {
            // Down arrow ▼
            let arrow_color = self.ui.search_indicator;
            right_x -= self.cell_width;
            self.text_pipeline.push_char('▼', right_x, text_y, arrow_color);

            // Up arrow ▲
            right_x -= self.cell_width;
            self.text_pipeline.push_char('▲', right_x, text_y, arrow_color);

            right_x -= self.cell_width * 0.5; // Gap before count
        }

        // Match count indicator
        let count_text = if info.match_count > 0 {
            format!("{}/{}", info.current_match.unwrap_or(0), info.match_count)
        } else if !info.query.is_empty() {
            "No matches".to_string()
        } else {
            String::new()
        };

        if !count_text.is_empty() {
            let count_width = count_text.len() as f32 * self.cell_width;
            right_x -= count_width;

            let count_color = if info.match_count > 0 {
                self.ui.search_indicator
            } else {
                self.ui.search_error
            };

            for c in count_text.chars() {
                self.text_pipeline.push_char(c, right_x, text_y, count_color);
                right_x += self.cell_width;
            }
            right_x -= count_width + self.cell_width; // Reset and add gap
        }

        // Regex indicator [.*] - always show, dimmed when inactive
        {
            let indicator = "[.*]";
            let indicator_width = indicator.len() as f32 * self.cell_width;
            right_x -= indicator_width + self.cell_width * 0.5;

            let regex_color = if info.regex_error.is_some() {
                self.ui.search_error
            } else if info.use_regex {
                self.ui.search_indicator
            } else {
                adjust_alpha(self.ui.search_indicator, 0.35)
            };

            for c in indicator.chars() {
                self.text_pipeline.push_char(c, right_x, text_y, regex_color);
                right_x += self.cell_width;
            }
            right_x -= indicator_width + self.cell_width;
        }

        // Case sensitivity indicator [Aa] - always show, dimmed when inactive
        {
            let indicator = "[Aa]";
            let indicator_width = indicator.len() as f32 * self.cell_width;
            right_x -= indicator_width + self.cell_width * 0.5;

            // Bright when active, dimmed when inactive
            let indicator_color = if info.case_sensitive {
                self.ui.search_indicator
            } else {
                adjust_alpha(self.ui.search_indicator, 0.35)
            };

            for c in indicator.chars() {
                self.text_pipeline.push_char(c, right_x, text_y, indicator_color);
                right_x += self.cell_width;
            }
            right_x -= indicator_width + self.cell_width;
        }

        // Filter mode indicator [⊜] - always show, dimmed when inactive
        // Shows line count when active
        {
            let indicator = if info.filter_mode {
                format!("[⊜{}]", info.matching_line_count)
            } else {
                "[⊜]".to_string()
            };
            let indicator_width = indicator.chars().count() as f32 * self.cell_width;
            right_x -= indicator_width + self.cell_width * 0.5;

            // Bright when active, dimmed when inactive
            let indicator_color = if info.filter_mode {
                self.ui.search_indicator
            } else {
                adjust_alpha(self.ui.search_indicator, 0.35)
            };

            for c in indicator.chars() {
                self.text_pipeline.push_char(c, right_x, text_y, indicator_color);
                right_x += self.cell_width;
            }
            let _ = right_x; // Silence unused warning
        }

        // Show regex error if any
        if let Some(error) = &info.regex_error {
            let error_x = text_x + self.cell_width;
            let error_text = format!("Error: {}", error);
            let error_color = self.ui.search_error;

            let max_chars = ((window_width - error_x - padding * 2.0) / self.cell_width) as usize;
            for (i, c) in error_text.chars().take(max_chars.saturating_sub(3)).enumerate() {
                self.text_pipeline.push_char(c, error_x + i as f32 * self.cell_width, text_y, error_color);
            }
        }
    }

    /// Check what element was clicked in the search bar
    /// Returns a SearchBarClick indicating which element (if any) was clicked
    ///
    /// Parameters must match the SearchBarInfo used for rendering:
    /// - match_count: number of search matches
    /// - current_match: current match index (1-based)
    /// - query_is_empty: whether the search query is empty
    /// - filter_mode: whether filter mode is active
    /// - matching_line_count: number of lines with matches (for filter button width)
    pub fn search_bar_hit_test(
        &self,
        x: f32,
        y: f32,
        window_width: f32,
        match_count: usize,
        current_match: Option<usize>,
        query_is_empty: bool,
        filter_mode: bool,
        matching_line_count: usize,
    ) -> SearchBarClick {
        let padding = 8.0;
        let bar_height = self.cell_height + padding * 2.0;
        let tab_bar_height = self.tab_bar_height();

        // Check if click is within search bar bounds
        if y < tab_bar_height || y > tab_bar_height + bar_height {
            return SearchBarClick::None;
        }

        // Calculate button positions from the right edge
        // This MUST exactly match the rendering logic in build_search_bar
        let mut right_x = window_width - padding;

        // Navigation arrows ▲▼ (only show if there are matches)
        if match_count > 0 {
            // Down arrow ▼
            right_x -= self.cell_width;
            let down_start = right_x;
            let down_end = right_x + self.cell_width;
            if x >= down_start && x < down_end {
                return SearchBarClick::NextMatch;
            }

            // Up arrow ▲
            right_x -= self.cell_width;
            let up_start = right_x;
            let up_end = right_x + self.cell_width;
            if x >= up_start && x < up_end {
                return SearchBarClick::PrevMatch;
            }

            right_x -= self.cell_width * 0.5; // Gap before count
        }

        // Match count indicator - calculate exact width like rendering does
        let count_text = if match_count > 0 {
            format!("{}/{}", current_match.unwrap_or(0), match_count)
        } else if !query_is_empty {
            "No matches".to_string()
        } else {
            String::new()
        };

        if !count_text.is_empty() {
            let count_width = count_text.len() as f32 * self.cell_width;
            right_x -= count_width;
            // Skip drawing the count (we're at the start of count now)
            // After rendering, right_x is reset: right_x -= count_width + self.cell_width
            right_x -= self.cell_width; // Gap after count
        }

        // Regex indicator [.*] - 4 chars
        let regex_width = 4.0 * self.cell_width;
        right_x -= regex_width + self.cell_width * 0.5;
        let regex_start = right_x;
        let regex_end = right_x + regex_width;
        if x >= regex_start && x < regex_end {
            return SearchBarClick::ToggleRegex;
        }
        // After drawing: right_x -= indicator_width + self.cell_width
        right_x -= self.cell_width;

        // Case sensitivity indicator [Aa] - 4 chars
        let case_width = 4.0 * self.cell_width;
        right_x -= case_width + self.cell_width * 0.5;
        let case_start = right_x;
        let case_end = right_x + case_width;
        if x >= case_start && x < case_end {
            return SearchBarClick::ToggleCaseSensitive;
        }
        right_x -= self.cell_width;

        // Filter mode indicator [⊜] or [⊜N] - variable width
        let filter_indicator = if filter_mode {
            format!("[⊜{}]", matching_line_count)
        } else {
            "[⊜]".to_string()
        };
        let filter_width = filter_indicator.chars().count() as f32 * self.cell_width;
        right_x -= filter_width + self.cell_width * 0.5;
        let filter_start = right_x;
        let filter_end = right_x + filter_width;
        if x >= filter_start && x < filter_end {
            return SearchBarClick::ToggleFilter;
        }

        SearchBarClick::None
    }
}
