//! Search bar rendering and hit-testing.

use super::Renderer;
use super::types::*;
use crate::ui::color::{scale_alpha, with_alpha};
use crate::ui::icon::Icon;
use crate::ui::search_bar::{SearchBarInfo, SearchBarLayout};
use crate::ui::text_input_viewport::TextInputViewport;

impl Renderer {
    pub(super) fn build_search_bar(
        &mut self,
        info: &SearchBarInfo,
        content_left: f32,
        content_width: f32,
        content_top: f32,
    ) {
        let layout = SearchBarLayout::calculate(
            info,
            content_left,
            content_width,
            content_top,
            self.cell_width,
            self.cell_height,
        );
        if layout.bar.width <= 0.0 {
            return;
        }
        let scale = self.scale_factor;
        let px = |logical: f32| (logical * scale).round().max(1.0);
        let text = self.ui.menu_text;
        let dim = self.ui.menu_text_dim;
        let accent = self.ui.palette_accent;
        let error_color = self.ui.search_error;
        let radius = px(8.0);
        let bar = layout.bar;

        // ── Floating surface with a hairline border ──
        let border = if info.regex_error.is_some() {
            with_alpha(error_color, 0.7)
        } else {
            self.ui.menu_border
        };
        self.rect_pipeline
            .push_rounded_rect(bar.x, bar.y, bar.width, bar.height, border, radius);
        let hairline = px(1.0);
        self.rect_pipeline.push_rounded_rect(
            bar.x + hairline,
            bar.y + hairline,
            bar.width - hairline * 2.0,
            bar.height - hairline * 2.0,
            self.ui.menu_bg,
            radius - hairline,
        );

        let icon = layout.icon;
        self.push_magnifier(
            icon.x + icon.width / 2.0,
            icon.y + icon.height / 2.0,
            dim,
            self.ui.menu_bg,
        );

        // ── Query ──
        if let Some(view) = layout.input_viewport(info, self.cell_width) {
            let clip = super::text::TextClip {
                left: view.text_x,
                top: layout.input.y,
                right: view.right,
                bottom: layout.input.y + layout.input.height,
            };
            if info.query.is_empty() {
                let hint = "Find";
                if let Some(hint_view) = TextInputViewport::calculate(
                    hint,
                    0,
                    view.text_x,
                    view.text_y,
                    view.right,
                    self.cell_width,
                ) {
                    self.text_pipeline.push_ui_run(
                        &hint[hint_view.text_range],
                        view.text_x,
                        view.text_y,
                        scale_alpha(dim, 0.7),
                        self.cell_width,
                        clip,
                    );
                }
            } else {
                self.text_pipeline.push_ui_run(
                    &info.query[view.text_range.clone()],
                    view.text_x,
                    view.text_y,
                    text,
                    self.cell_width,
                    clip,
                );
            }
            if info.is_active {
                self.rect_pipeline.push_rect(
                    view.cursor_x,
                    view.text_y,
                    px(2.0).min(view.right - view.cursor_x),
                    self.cell_height,
                    accent,
                );
            }
        }

        // ── Status, right-aligned in its slot ──
        let count = info.count_text();
        if !count.is_empty() {
            let size = self.ui_label_px() * 0.92;
            let status = layout.status;
            let width = self.ui_text_width(&count, size).min(status.width);
            let color = if info.regex_error.is_some() || (!info.searching && info.match_count == 0)
            {
                error_color
            } else {
                dim
            };
            self.push_ui_label(
                &count,
                status.x + status.width - width,
                status.y,
                status.height,
                status.width,
                size,
                color,
            );
        }

        // ── Option toggles, then navigation and close ──
        for (rect, label, on) in [
            (layout.case_sensitive, Some("Aa"), info.case_sensitive),
            (layout.regex, Some(".*"), info.use_regex),
            (layout.filter, None, info.filter_mode),
        ] {
            if on {
                self.rect_pipeline.push_rounded_rect(
                    rect.x,
                    rect.y,
                    rect.width,
                    rect.height,
                    with_alpha(accent, 0.28),
                    px(5.0),
                );
            }
            let color = if on { text } else { dim };
            match label {
                Some(label) => {
                    let size = self.ui_label_px();
                    let width = self.ui_text_width(label, size);
                    self.push_ui_label(
                        label,
                        rect.x + (rect.width - width) / 2.0,
                        rect.y,
                        rect.height,
                        rect.width,
                        size,
                        color,
                    );
                }
                None => self.push_icon(
                    Icon::Filter,
                    rect.x + rect.width / 2.0,
                    rect.y + rect.height / 2.0,
                    color,
                ),
            }
        }
        let navigation_color = if layout.navigable {
            dim
        } else {
            scale_alpha(dim, 0.4)
        };
        for (rect, icon) in [
            (layout.previous, Icon::ChevronUp),
            (layout.next, Icon::ChevronDown),
        ] {
            self.push_icon(
                icon,
                rect.x + rect.width / 2.0,
                rect.y + rect.height / 2.0,
                navigation_color,
            );
        }
        let close = layout.close;
        self.push_icon(
            Icon::Close,
            close.x + close.width / 2.0,
            close.y + close.height / 2.0,
            dim,
        );

        // ── Regex error on its own line ──
        if let (Some(line), Some(message)) = (layout.error, &info.regex_error) {
            let size = self.ui_label_px() * 0.92;
            self.push_ui_label(
                message,
                line.x,
                line.y,
                line.height,
                line.width,
                size,
                error_color,
            );
        }
    }

    pub fn search_bar_layout(&self, info: &SearchBarInfo, window_width: f32) -> SearchBarLayout {
        SearchBarLayout::calculate(
            info,
            self.tab_bar_width_for(window_width),
            self.content_width_for(window_width),
            self.content_top(),
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
        search_bar_click(&layout, info, self.cell_width, x, y)
    }
}

fn search_bar_click(
    layout: &SearchBarLayout,
    info: &SearchBarInfo,
    cell_width: f32,
    x: f32,
    y: f32,
) -> SearchBarClick {
    if !layout.bar.contains(x, y) {
        return SearchBarClick::None;
    }
    let navigation = layout.navigable;
    for (rect, enabled, click) in [
        (layout.close, true, SearchBarClick::Close),
        (layout.next, navigation, SearchBarClick::NextMatch),
        (layout.previous, navigation, SearchBarClick::PrevMatch),
        (layout.regex, true, SearchBarClick::ToggleRegex),
        (
            layout.case_sensitive,
            true,
            SearchBarClick::ToggleCaseSensitive,
        ),
        (layout.filter, true, SearchBarClick::ToggleFilter),
    ] {
        if rect.contains(x, y) {
            // A disabled control still owns its pixels.
            return if enabled {
                click
            } else {
                SearchBarClick::Background
            };
        }
    }
    if layout.input.contains(x, y)
        && let Some(cursor) = layout
            .input_viewport(info, cell_width)
            .and_then(|view| view.cursor_at(&info.query, x, cell_width))
    {
        return SearchBarClick::Query(cursor);
    }
    SearchBarClick::Background
}

#[cfg(test)]
mod tests {
    use super::*;
    use unicode_segmentation::UnicodeSegmentation;

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
    fn search_pointer_uses_scrolled_graphemes_and_preserves_control_priority() {
        let mut info = info();
        for cell_width in [6.0, 9.5, 24.0] {
            for width in [100.0, 800.0, 1600.0] {
                for text in [
                    "",
                    "a界e\u{301}👩\u{200d}💻🇺🇸z",
                    &"a界e\u{301}👩\u{200d}💻".repeat(60),
                ] {
                    info.query = text.into();
                    for cursor in [0, text.len()] {
                        info.cursor_pos = cursor;
                        let layout =
                            SearchBarLayout::calculate(&info, 120.0, width, 55.0, cell_width, 25.0);
                        let bar = layout.bar;
                        assert_eq!(
                            search_bar_click(&layout, &info, cell_width, bar.x - 1.0, bar.y + 1.0),
                            SearchBarClick::None
                        );
                        if bar.width > 2.0 {
                            assert_eq!(
                                search_bar_click(
                                    &layout,
                                    &info,
                                    cell_width,
                                    bar.x + 1.0,
                                    bar.y + 1.0
                                ),
                                SearchBarClick::Background
                            );
                        }
                        let Some(view) = layout.input_viewport(&info, cell_width) else {
                            continue;
                        };
                        let mut x = view.text_x;
                        for (byte, grapheme) in text[view.text_range.clone()].grapheme_indices(true)
                        {
                            let width = termeric_core::unicode::grapheme_width(grapheme) as f32
                                * cell_width;
                            for (fraction, expected) in
                                [(0.25, byte), (0.75, byte + grapheme.len())]
                            {
                                assert_eq!(
                                    search_bar_click(
                                        &layout,
                                        &info,
                                        cell_width,
                                        x + fraction * width,
                                        view.text_y
                                    ),
                                    SearchBarClick::Query(view.text_range.start + expected)
                                );
                            }
                            x += width;
                        }
                        assert_eq!(
                            search_bar_click(
                                &layout,
                                &info,
                                cell_width,
                                layout.input.x,
                                view.text_y
                            ),
                            SearchBarClick::Query(view.text_range.start)
                        );
                        assert_eq!(
                            search_bar_click(
                                &layout,
                                &info,
                                cell_width,
                                layout.input.x + layout.input.width - 0.01,
                                view.text_y
                            ),
                            SearchBarClick::Query(view.text_range.end)
                        );
                        for (rect, expected) in [
                            (layout.close, SearchBarClick::Close),
                            (layout.regex, SearchBarClick::ToggleRegex),
                            (layout.case_sensitive, SearchBarClick::ToggleCaseSensitive),
                            (layout.filter, SearchBarClick::ToggleFilter),
                            (layout.next, SearchBarClick::NextMatch),
                            (layout.previous, SearchBarClick::PrevMatch),
                        ] {
                            assert_eq!(
                                search_bar_click(
                                    &layout,
                                    &info,
                                    cell_width,
                                    rect.x + rect.width / 2.0,
                                    rect.y
                                ),
                                expected
                            );
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn search_query_viewport_preserves_whole_graphemes_and_reserves_the_caret() {
        let mut info = info();
        info.query = "界e\u{301}👩\u{200d}💻ab".repeat(80);
        for width in [100.0, 450.0, 800.0, 1400.0] {
            let layout = SearchBarLayout::calculate(&info, 120.0, width, 55.0, 9.5, 20.0);
            for cursor in info
                .query
                .char_indices()
                .map(|(byte, _)| byte)
                .chain([info.query.len()])
            {
                info.cursor_pos = cursor;
                let Some(view) = layout.input_viewport(&info, 9.5) else {
                    assert!(layout.input.width < 9.5 * 2.3);
                    continue;
                };
                assert!(view.cursor_x >= view.text_x);
                assert!(view.cursor_x + 9.5 <= view.right + 0.001);
                let visible = &info.query[view.text_range.clone()];
                assert!(
                    visible
                        .graphemes(true)
                        .map(termeric_core::unicode::grapheme_width)
                        .sum::<usize>()
                        <= view.columns
                );
                for boundary in [view.text_range.start, view.text_range.end] {
                    assert!(
                        boundary == info.query.len()
                            || info
                                .query
                                .grapheme_indices(true)
                                .any(|(byte, _)| byte == boundary)
                    );
                }
            }
        }
    }

    #[test]
    fn count_text_discloses_recent_window_truncation() {
        let mut info = info();
        info.match_count = 300_000;
        info.current_match = Some(37_857);
        info.results_truncated = true;
        assert_eq!(info.count_text(), "37857 of 300000 · recent");
        info.match_count = 12;
        info.current_match = Some(1);
        info.results_truncated = false;
        assert_eq!(info.count_text(), "1 of 12");
        info.searching = true;
        assert_eq!(info.count_text(), "Searching…");
    }

    #[test]
    fn shared_layout_keeps_controls_disjoint_and_inside_bar() {
        let mut info = info();
        let layout = SearchBarLayout::calculate(&info, 120.0, 680.0, 30.0, 8.0, 16.0);
        // The box floats at the top right of the content, clear of the
        // scrollbar strip, and never spans the whole width.
        assert!(layout.bar.x > 120.0 && layout.bar.y > 30.0);
        assert!(layout.bar.x + layout.bar.width <= 800.0 - 16.0);
        assert!(layout.error.is_none());
        for rect in [
            layout.filter,
            layout.case_sensitive,
            layout.regex,
            layout.previous,
            layout.next,
            layout.close,
        ] {
            assert!(layout.bar.contains(rect.x, rect.y));
            assert!(layout.bar.contains(rect.x + rect.width - 0.01, rect.y));
        }
        assert!(layout.input.x + layout.input.width <= layout.status.x);
        assert!(layout.status.x + layout.status.width <= layout.case_sensitive.x);
        for (left, right) in [
            (layout.case_sensitive, layout.regex),
            (layout.regex, layout.filter),
            (layout.filter, layout.previous),
            (layout.previous, layout.next),
            (layout.next, layout.close),
        ] {
            assert!(left.x + left.width <= right.x);
        }

        // Navigation space stays reserved without matches, and an invalid
        // pattern adds a line for its message.
        info.retained_match_count = 0;
        info.regex_error = Some("unclosed group".into());
        let without = SearchBarLayout::calculate(&info, 120.0, 680.0, 30.0, 8.0, 16.0);
        assert!(!without.navigable);
        assert_eq!(without.next, layout.next);
        assert_eq!(
            search_bar_click(
                &without,
                &info,
                8.0,
                without.next.x + 1.0,
                without.next.y + 1.0
            ),
            SearchBarClick::Background
        );
        let error = without.error.unwrap();
        assert!(without.bar.height > layout.bar.height);
        assert!(error.y + error.height <= without.bar.y + without.bar.height);
    }
}
