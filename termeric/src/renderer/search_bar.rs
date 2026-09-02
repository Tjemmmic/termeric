//! Search bar rendering and hit-testing.

use super::types::*;
use super::{Renderer, adjust_alpha};
use crate::terminal::search::{SearchBarInfo, SearchBarLayout, SearchBarRect};

impl Renderer {
    pub(super) fn build_search_bar(
        &mut self,
        info: &SearchBarInfo,
        content_left: f32,
        content_width: f32,
        tab_bar_height: f32,
    ) {
        let layout = SearchBarLayout::calculate(
            info,
            content_left,
            content_width,
            tab_bar_height,
            self.cell_width,
            self.cell_height,
        );

        self.rect_pipeline.push_rect(
            layout.bar.x,
            layout.bar.y,
            layout.bar.width,
            layout.bar.height,
            self.ui.search_bg,
        );
        self.rect_pipeline.push_rect(
            layout.bar.x,
            layout.bar.y + layout.bar.height - 1.0,
            layout.bar.width,
            1.0,
            adjust_alpha(self.ui.tab_border, 0.5),
        );

        let prompt_color = self.ui.search_indicator;
        self.text_pipeline
            .push_char('/', layout.input.x, layout.text_y, prompt_color);
        let query_x = layout.input.x + self.cell_width * 1.3;
        let query_width = (layout.input.x + layout.input.width - query_x).max(0.0);
        let max_chars = (query_width / self.cell_width).floor() as usize;
        let cursor_char = info
            .query
            .get(..info.cursor_pos.min(info.query.len()))
            .filter(|_| {
                info.query
                    .is_char_boundary(info.cursor_pos.min(info.query.len()))
            })
            .map(str::chars)
            .map(Iterator::count)
            .unwrap_or_else(|| info.query.chars().count());
        let first_char = cursor_char.saturating_sub(max_chars.saturating_sub(1));
        let mut drawn = 0;
        if info.is_active && info.query.is_empty() && max_chars > 0 {
            self.draw_search_text(
                &"Type to search".chars().take(max_chars).collect::<String>(),
                query_x,
                layout.text_y,
                adjust_alpha(self.ui.search_text, 0.4),
            );
        }
        for (index, character) in info.query.chars().enumerate().skip(first_char) {
            if drawn >= max_chars {
                break;
            }
            let x = query_x + drawn as f32 * self.cell_width;
            if info.is_active && index == cursor_char {
                self.rect_pipeline.push_rect(
                    x,
                    layout.text_y,
                    2.0,
                    self.cell_height,
                    adjust_alpha(self.ui.cursor, 0.8),
                );
            }
            self.text_pipeline
                .push_char(character, x, layout.text_y, self.ui.search_text);
            drawn += 1;
        }
        if info.is_active && cursor_char >= info.query.chars().count() && drawn < max_chars {
            self.rect_pipeline.push_rect(
                query_x + drawn as f32 * self.cell_width,
                layout.text_y,
                2.0,
                self.cell_height,
                adjust_alpha(self.ui.cursor, 0.8),
            );
        }
        if let Some(error) = &info.regex_error
            && drawn.saturating_add(2) < max_chars
        {
            let error = format!(" Error: {error}");
            for character in error.chars().take(max_chars - drawn) {
                self.text_pipeline.push_char(
                    character,
                    query_x + drawn as f32 * self.cell_width,
                    layout.text_y,
                    self.ui.search_error,
                );
                drawn += 1;
            }
        }

        if let Some(rect) = layout.next {
            self.draw_search_text("▼", rect.x, layout.text_y, self.ui.search_indicator);
        }
        if let Some(rect) = layout.previous {
            self.draw_search_text("▲", rect.x, layout.text_y, self.ui.search_indicator);
        }

        let count = info.count_text();
        if !count.is_empty() {
            let color = if info.searching || info.match_count > 0 {
                self.ui.search_indicator
            } else {
                self.ui.search_error
            };
            self.draw_search_text(&count, layout.status.x, layout.text_y, color);
        }

        let regex_color = if info.regex_error.is_some() {
            self.ui.search_error
        } else if info.use_regex {
            self.ui.search_indicator
        } else {
            adjust_alpha(self.ui.search_indicator, 0.35)
        };
        self.draw_search_text("[.*]", layout.regex.x, layout.text_y, regex_color);

        let case_color = if info.case_sensitive {
            self.ui.search_indicator
        } else {
            adjust_alpha(self.ui.search_indicator, 0.35)
        };
        self.draw_search_text("[Aa]", layout.case_sensitive.x, layout.text_y, case_color);

        let filter = if info.filter_mode {
            format!("[⊜{}]", info.matching_line_count)
        } else {
            "[⊜]".to_string()
        };
        let filter_color = if info.filter_mode {
            self.ui.search_indicator
        } else {
            adjust_alpha(self.ui.search_indicator, 0.35)
        };
        self.draw_search_text(&filter, layout.filter.x, layout.text_y, filter_color);
        self.draw_search_text("×", layout.close.x, layout.text_y, self.ui.search_indicator);
    }

    fn draw_search_text(&mut self, text: &str, x: f32, y: f32, color: [f32; 4]) {
        for (index, character) in text.chars().enumerate() {
            self.text_pipeline
                .push_char(character, x + index as f32 * self.cell_width, y, color);
        }
    }

    pub fn search_bar_layout(&self, info: &SearchBarInfo, window_width: f32) -> SearchBarLayout {
        SearchBarLayout::calculate(
            info,
            self.tab_bar_width_for(window_width),
            (window_width - self.tab_bar_width_for(window_width)).max(0.0),
            self.tab_bar_height() + self.chrome_bar_height(),
            self.cell_width,
            self.cell_height,
        )
    }

    /// Check what exact painted control was clicked in the search bar.
    pub fn search_bar_hit_test(
        &self,
        x: f32,
        y: f32,
        window_width: f32,
        info: &SearchBarInfo,
    ) -> SearchBarClick {
        let layout = self.search_bar_layout(info, window_width);
        if !layout.bar.contains(x, y) {
            return SearchBarClick::None;
        }
        for (rect, click) in [
            (Some(layout.close), SearchBarClick::Close),
            (layout.next, SearchBarClick::NextMatch),
            (layout.previous, SearchBarClick::PrevMatch),
            (Some(layout.regex), SearchBarClick::ToggleRegex),
            (
                Some(layout.case_sensitive),
                SearchBarClick::ToggleCaseSensitive,
            ),
            (Some(layout.filter), SearchBarClick::ToggleFilter),
        ] {
            if rect.is_some_and(|rect: SearchBarRect| rect.contains(x, y)) {
                return click;
            }
        }
        SearchBarClick::None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn info() -> SearchBarInfo {
        SearchBarInfo {
            query: "needle".into(),
            case_sensitive: false,
            use_regex: false,
            filter_mode: false,
            current_match: Some(1),
            match_count: 12,
            retained_match_count: 12,
            results_truncated: false,
            searching: false,
            matching_line_count: 4,
            regex_error: None,
            cursor_pos: 6,
            is_active: true,
        }
    }

    #[test]
    fn count_text_discloses_recent_window_truncation() {
        let mut info = info();
        info.match_count = 300_000;
        info.current_match = Some(37_857);
        info.results_truncated = true;
        assert_eq!(info.count_text(), "37857/300000 · recent");
        info.match_count = 12;
        info.current_match = Some(1);
        info.results_truncated = false;
        assert_eq!(info.count_text(), "1/12");
        info.searching = true;
        assert_eq!(info.count_text(), "Searching…");
    }

    #[test]
    fn shared_layout_keeps_controls_disjoint_and_inside_bar() {
        let info = info();
        let layout = SearchBarLayout::calculate(&info, 120.0, 680.0, 30.0, 8.0, 16.0);
        assert_eq!(layout.bar.x, 120.0);
        assert_eq!(layout.bar.width, 680.0);
        for rect in [
            layout.filter,
            layout.case_sensitive,
            layout.regex,
            layout.previous.unwrap(),
            layout.next.unwrap(),
            layout.close,
        ] {
            assert!(layout.bar.contains(rect.x, rect.y));
            assert!(layout.bar.contains(rect.x + rect.width - 0.01, rect.y));
        }
        assert!(layout.input.x + layout.input.width <= layout.filter.x);
    }
}
