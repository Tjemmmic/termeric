//! Git branch completion dropdown rendering and bounded viewport layout.

use super::glyph_cache::FontStyle;
use super::icons::draw_icon;
use super::types::*;
use super::{Renderer, TextClip};
use crate::ui::icon::Icon;

const EDGE: f32 = 2.0;
const MAX_ITEMS: usize = 8;

/// Pixel position of the rendered cursor, including its owning grid's origin.
/// Keeping pixels explicit avoids mixing pane-local cells with window geometry.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct CompletionAnchor {
    x: f32,
    y: f32,
}

impl CompletionAnchor {
    pub(super) fn for_view(
        single_cursor: (f32, f32),
        panes: &[PaneRenderData<'_>],
        content_origin: (f32, f32),
        cell_size: (f32, f32),
        padding: f32,
    ) -> Option<Self> {
        let (origin, cursor) = if panes.is_empty() {
            (
                (content_origin.0 + padding, content_origin.1 + padding),
                single_cursor,
            )
        } else {
            let mut focused = panes.iter().filter(|pane| pane.is_focused);
            let pane = focused.next()?;
            if focused.next().is_some() {
                return None;
            }
            // Match the exact origins passed to build_pane_grid, including the
            // asymmetry: pane y already includes its content-area placement.
            (
                (
                    content_origin.0 + padding + pane.rect.x,
                    content_origin.1 + pane.rect.y,
                ),
                pane.cursor_pos,
            )
        };
        let anchor = Self {
            x: origin.0 + cursor.0 * cell_size.0,
            y: origin.1 + cursor.1 * cell_size.1,
        };
        (anchor.x.is_finite() && anchor.y.is_finite()).then_some(anchor)
    }
}

#[derive(Clone, Copy)]
struct CompletionViewport {
    content_left: f32,
    content_top: f32,
    content_width: f32,
    window_height: f32,
    cell_width: f32,
    cell_height: f32,
    padding: f32,
}

impl CompletionViewport {
    fn bounds(self) -> Option<TextClip> {
        if ![
            self.content_left,
            self.content_top,
            self.content_width,
            self.window_height,
            self.cell_width,
            self.cell_height,
            self.padding,
        ]
        .iter()
        .all(|value| value.is_finite())
            || self.content_left < 0.0
            || self.content_top < 0.0
            || self.cell_width <= 0.0
            || self.cell_height <= 0.0
            || self.padding < 0.0
        {
            return None;
        }
        let bounds = TextClip {
            left: self.content_left + self.padding,
            top: self.content_top + self.padding,
            right: self.content_left + self.content_width - self.padding,
            bottom: self.window_height - self.padding,
        };
        if ![bounds.left, bounds.top, bounds.right, bounds.bottom]
            .iter()
            .all(|value| value.is_finite())
        {
            return None;
        }
        // At least one item row, one character and horizontal text margins.
        (bounds.right - bounds.left >= self.cell_width * 3.0 + EDGE * 2.0
            && bounds.bottom - bounds.top >= self.cell_height * 1.6 + EDGE * 2.0)
            .then_some(bounds)
    }
}

#[derive(Debug)]
struct CompletionLayout {
    x: f32,
    y: f32,
    width: f32,
    height: f32,
    row_height: f32,
    start: usize,
    end: usize,
    header: bool,
    footer: bool,
}

impl CompletionLayout {
    fn calculate_at_anchor(
        info: &GitCompletionInfo,
        anchor: CompletionAnchor,
        view: CompletionViewport,
    ) -> Option<Self> {
        let bounds = view.bounds()?;
        let count = info.completions.len();
        if count == 0 || !anchor.x.is_finite() || !anchor.y.is_finite() {
            return None;
        }
        let row_height = view.cell_height * 1.6;
        let desired_rows = 1 + count.min(MAX_ITEMS) + usize::from(count > MAX_ITEMS);
        let desired_height = EDGE * 2.0 + desired_rows as f32 * row_height;
        let cursor_y = anchor.y.clamp(bounds.top, bounds.bottom);
        let below_y = (cursor_y + view.cell_height).min(bounds.bottom);
        let below = bounds.bottom - below_y;
        let above = cursor_y - bounds.top;
        let use_below = below >= desired_height || (above < desired_height && below >= above);
        let mut available = if use_below { below } else { above };
        let fallback = available < row_height + EDGE * 2.0;
        if fallback {
            // If neither side can hold a row, use the content viewport itself.
            // This can overlap the cursor, but never makes a selected item invisible.
            available = bounds.bottom - bounds.top;
        }
        let capacity =
            (((available - EDGE * 2.0) / row_height).floor() as usize).clamp(1, MAX_ITEMS + 2);
        let header = capacity >= 2;
        let mut visible = count.min(MAX_ITEMS).min(capacity - usize::from(header));
        let footer = count > visible && capacity >= 3;
        if footer {
            visible = visible.min(capacity - 2);
        }
        let selected = info.selected_index.unwrap_or(0).min(count - 1);
        let start = selected.saturating_sub(visible - 1).min(count - visible);
        let height =
            EDGE * 2.0 + (visible + usize::from(header) + usize::from(footer)) as f32 * row_height;
        let y = if fallback {
            cursor_y
        } else if use_below {
            below_y
        } else {
            cursor_y - height
        }
        .clamp(bounds.top, (bounds.bottom - height).max(bounds.top));
        let min_width = view.cell_width * 3.0 + EDGE * 2.0;
        let max_width = (view.content_width * 0.5)
            .max(min_width)
            .min(bounds.right - bounds.left);
        let width = ((info.completions.max_name_chars() + 4) as f32 * view.cell_width)
            .max(view.cell_width * 24.0)
            .min(max_width);
        let x = anchor
            .x
            .clamp(bounds.left, (bounds.right - width).max(bounds.left));
        Some(Self {
            x,
            y,
            width,
            height,
            row_height,
            start,
            end: start + visible,
            header,
            footer,
        })
    }

    fn items_y(&self) -> f32 {
        self.y + EDGE + if self.header { self.row_height } else { 0.0 }
    }

    fn clip(&self) -> TextClip {
        TextClip {
            left: self.x + EDGE,
            top: self.y + EDGE,
            right: self.x + self.width - EDGE,
            bottom: self.y + self.height - EDGE,
        }
    }

    #[cfg(test)]
    fn calculate(
        info: &GitCompletionInfo,
        cursor: (f32, f32),
        view: CompletionViewport,
    ) -> Option<Self> {
        let anchor = CompletionAnchor::for_view(
            cursor,
            &[],
            (view.content_left, view.content_top),
            (view.cell_width, view.cell_height),
            view.padding,
        )?;
        Self::calculate_at_anchor(info, anchor, view)
    }
}

impl Renderer {
    /// Same minimum viewport contract as the render layout. Input must not
    /// consume completion keys when no complete item can be displayed.
    pub fn git_completion_fits(&self) -> bool {
        CompletionViewport {
            content_left: self.tab_bar_width(),
            content_top: self.content_top(),
            content_width: self.content_width_for(self.size.0 as f32),
            window_height: self.size.1 as f32,
            cell_width: self.cell_width(),
            cell_height: self.cell_height(),
            padding: self.padding,
        }
        .bounds()
        .is_some()
    }

    // Bound queued text to its visible cells, including the ellipsis; do not
    // enqueue a multi-megabyte branch and rely solely on a GPU scissor.
    fn git_completion_text(
        &mut self,
        value: &str,
        x: f32,
        y: f32,
        color: [f32; 4],
        clip: TextClip,
    ) {
        let cells = ((clip.right - x).max(0.0) / self.cell_width()).floor() as usize;
        for (column, character) in completion_text_chars(value, cells).enumerate() {
            self.text_pipeline.push_styled_char(
                character,
                x + column as f32 * self.cell_width(),
                y,
                color,
                FontStyle::default(),
                clip,
            );
        }
    }

    pub(super) fn build_git_completions(
        &mut self,
        info: &GitCompletionInfo,
        anchor: CompletionAnchor,
        content_left: f32,
        content_top: f32,
        content_width: f32,
        window_height: f32,
    ) {
        let Some(layout) = CompletionLayout::calculate_at_anchor(
            info,
            anchor,
            CompletionViewport {
                content_left,
                content_top,
                content_width,
                window_height,
                cell_width: self.cell_width(),
                cell_height: self.cell_height(),
                padding: self.padding,
            },
        ) else {
            return;
        };
        let cw = self.cell_width();
        let cell_h = self.cell_height();
        let accent = self.ui.settings_accent;
        let text_color = self.ui.settings_text;
        let bg = self.ui.settings_bg;
        let clip = layout.clip();
        let text_offset = (layout.row_height - cell_h) / 2.0;
        self.rect_pipeline.push_rounded_rect(
            layout.x,
            layout.y,
            layout.width,
            layout.height,
            [bg[0], bg[1], bg[2], 0.95],
            6.0,
        );
        let border = [accent[0], accent[1], accent[2], 0.3];
        for (x, y, width, height) in [
            (layout.x, layout.y, layout.width, 1.0),
            (layout.x, layout.y + layout.height - 1.0, layout.width, 1.0),
            (layout.x, layout.y, 1.0, layout.height),
            (layout.x + layout.width - 1.0, layout.y, 1.0, layout.height),
        ] {
            self.rect_pipeline.push_rect(x, y, width, height, border);
        }

        if layout.header {
            let header_y = layout.y + EDGE + text_offset;
            let icon_cx = layout.x + cw * 1.3;
            let icon_cy = header_y + cell_h * 0.5;
            // The branch's stem spans 10 of its icon's 16 units.
            let arm = (cell_h * 0.25).min(cw);
            draw_icon(
                &mut self.rect_pipeline,
                Icon::Branch,
                [icon_cx, icon_cy],
                arm * 2.0 * 16.0 / 10.0,
                accent,
            );
            self.git_completion_text(
                " branches",
                layout.x + cw * 2.3,
                header_y,
                [accent[0], accent[1], accent[2], 0.7],
                clip,
            );
            self.rect_pipeline.push_rect(
                layout.x + cw * 0.5,
                layout.items_y() - 1.0,
                layout.width - cw,
                0.5,
                self.ui.settings_separator,
            );
        }
        for (row, index) in (layout.start..layout.end).enumerate() {
            let y = layout.items_y() + row as f32 * layout.row_height;
            let selected = info.selected_index == Some(index);
            if selected {
                self.rect_pipeline.push_rounded_rect(
                    layout.x + EDGE,
                    y,
                    layout.width - EDGE * 2.0,
                    layout.row_height,
                    [accent[0], accent[1], accent[2], 0.15],
                    4.0,
                );
            }
            self.git_completion_text(
                &info.completions[index],
                layout.x + cw,
                y + text_offset,
                if selected { accent } else { text_color },
                clip,
            );
        }
        if layout.footer {
            let label = format!(
                "{}–{} of {}",
                layout.start + 1,
                layout.end,
                info.completions.len()
            );
            self.git_completion_text(
                &label,
                layout.x + cw,
                layout.items_y()
                    + (layout.end - layout.start) as f32 * layout.row_height
                    + text_offset,
                [text_color[0], text_color[1], text_color[2], 0.5],
                clip,
            );
        }
    }
}

fn completion_text_chars(value: &str, cells: usize) -> impl Iterator<Item = char> + '_ {
    let mut chars = value.chars().peekable();
    (0..cells).map_while(move |column| {
        let character = chars.next()?;
        Some(if column + 1 == cells && chars.peek().is_some() {
            '…'
        } else {
            character
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::geometry::Bounds;

    fn info(count: usize, selected: Option<usize>) -> GitCompletionInfo {
        GitCompletionInfo {
            completions: (0..count)
                .map(|index| format!("branch-{index}"))
                .collect::<Vec<_>>()
                .into(),
            selected_index: selected,
        }
    }

    fn view() -> CompletionViewport {
        CompletionViewport {
            content_left: 100.0,
            content_top: 50.0,
            content_width: 800.0,
            window_height: 1000.0,
            cell_width: 10.0,
            cell_height: 20.0,
            padding: 8.0,
        }
    }

    fn pane<'a>(
        terminal: &'a termeric_core::Terminal,
        urls: &'a crate::terminal::url::UrlCache,
        origin: (f32, f32),
        cursor: (f32, f32),
        focused: bool,
    ) -> PaneRenderData<'a> {
        PaneRenderData {
            terminal_key: terminal as *const _ as usize,
            grid: terminal.presentation_grid(),
            selection: None,
            dynamic_colors: terminal.presentation_dynamic_colors(),
            extra_cursors: terminal.presentation_extra_cursors(),
            rect: Bounds {
                x: origin.0,
                y: origin.1,
                width: 320.0,
                height: 240.0,
            },
            is_focused: focused,
            broadcast_target: None,
            read_only: false,
            is_zoomed: false,
            cursor_pos: cursor,
            cursor_style: crate::config::CursorStyle::Block,
            cursor_opacity: 1.0,
            url_cache: urls,
            copy_mode: None,
            images: Vec::new(),
        }
    }

    #[test]
    fn split_popup_anchor_matches_the_focused_grid_pixel_origin() {
        let terminal = termeric_core::Terminal::new(80, 24);
        let urls = crate::terminal::url::UrlCache::new(24);
        for origin in [(0.0, 30.0), (100.0, 50.0)] {
            for cell in [(10.0, 20.0), (9.75, 23.4)] {
                for padding in [0.0, 8.0, 17.5] {
                    for second_origin in [(320.0, 0.0), (0.0, 240.0)] {
                        for focused in [0, 1] {
                            let panes = [
                                pane(&terminal, &urls, (0.0, 0.0), (1.25, 2.5), focused == 0),
                                pane(&terminal, &urls, second_origin, (2.5, 3.25), focused == 1),
                            ];
                            let anchor = CompletionAnchor::for_view(
                                (999.0, 999.0),
                                &panes,
                                origin,
                                cell,
                                padding,
                            )
                            .unwrap();
                            let pane = &panes[focused];
                            // Same pixel origin used by build_pane_grid; the
                            // unrelated top-level cursor must never be used.
                            let expected_x =
                                origin.0 + padding + pane.rect.x + pane.cursor_pos.0 * cell.0;
                            let expected_y = origin.1 + pane.rect.y + pane.cursor_pos.1 * cell.1;
                            assert!((anchor.x - expected_x).abs() < 0.001);
                            assert!((anchor.y - expected_y).abs() < 0.001);
                            let viewport = CompletionViewport {
                                content_left: origin.0,
                                content_top: origin.1,
                                content_width: 1200.0,
                                window_height: 1400.0,
                                cell_width: cell.0,
                                cell_height: cell.1,
                                padding,
                            };
                            let layout = CompletionLayout::calculate_at_anchor(
                                &info(1, Some(0)),
                                anchor,
                                viewport,
                            )
                            .unwrap();
                            assert!((layout.x - expected_x).abs() < 0.001);
                            assert!((layout.y - (expected_y + cell.1)).abs() < 0.001);
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn popup_anchor_follows_focus_resize_zoom_move_and_single_grid_collapse() {
        let terminal = termeric_core::Terminal::new(80, 24);
        let urls = crate::terminal::url::UrlCache::new(24);
        let mut panes = [
            pane(&terminal, &urls, (0.0, 0.0), (1.0, 2.0), true),
            pane(&terminal, &urls, (320.0, 240.0), (3.5, 4.25), false),
        ];
        let anchor = |panes: &[PaneRenderData<'_>]| {
            CompletionAnchor::for_view((999.0, 999.0), panes, (100.0, 50.0), (10.0, 20.0), 8.0)
                .unwrap()
        };
        assert_eq!(anchor(&panes), CompletionAnchor { x: 118.0, y: 90.0 });
        panes[0].is_focused = false;
        panes[1].is_focused = true;
        assert_eq!(anchor(&panes), CompletionAnchor { x: 463.0, y: 375.0 });
        panes[1].rect.x = 400.0;
        panes[1].rect.y = 300.0;
        assert_eq!(anchor(&panes), CompletionAnchor { x: 543.0, y: 435.0 });
        panes[1].is_zoomed = true;
        panes[1].rect.x = 0.0;
        panes[1].rect.y = 0.0;
        assert_eq!(anchor(&panes[1..]), CompletionAnchor { x: 143.0, y: 135.0 });
        let moved =
            CompletionAnchor::for_view((0.0, 0.0), &panes[1..], (230.0, 90.0), (10.0, 20.0), 8.0)
                .unwrap();
        assert_eq!(moved, CompletionAnchor { x: 273.0, y: 175.0 });
        let collapsed =
            CompletionAnchor::for_view(panes[1].cursor_pos, &[], (100.0, 50.0), (10.0, 20.0), 8.0)
                .unwrap();
        // Single-grid rendering has vertical outer padding, unlike pane origins.
        assert_eq!(collapsed, CompletionAnchor { x: 143.0, y: 143.0 });
    }

    #[test]
    fn popup_anchor_refuses_missing_ambiguous_or_nonfinite_pane_focus() {
        let terminal = termeric_core::Terminal::new(80, 24);
        let urls = crate::terminal::url::UrlCache::new(24);
        let mut panes = [
            pane(&terminal, &urls, (0.0, 0.0), (1.0, 2.0), false),
            pane(&terminal, &urls, (320.0, 240.0), (3.0, 4.0), false),
        ];
        let anchor = |panes: &[PaneRenderData<'_>]| {
            CompletionAnchor::for_view((0.0, 0.0), panes, (100.0, 50.0), (10.0, 20.0), 8.0)
        };
        assert!(anchor(&panes).is_none());
        panes[0].is_focused = true;
        panes[1].is_focused = true;
        assert!(anchor(&panes).is_none());
        panes[0].is_focused = false;
        panes[1].cursor_pos.0 = f32::NAN;
        assert!(anchor(&panes).is_none());
        panes[1].cursor_pos = (3.0, 4.0);
        assert!(
            CompletionAnchor::for_view(
                (f32::NAN, f32::NAN),
                &panes,
                (100.0, 50.0),
                (10.0, 20.0),
                8.0,
            )
            .is_some()
        );
    }

    #[test]
    fn lower_right_pane_anchor_still_flips_and_clamps_to_window_bounds() {
        let terminal = termeric_core::Terminal::new(80, 24);
        let urls = crate::terminal::url::UrlCache::new(24);
        let panes = [pane(&terminal, &urls, (600.0, 700.0), (5.0, 10.0), true)];
        let anchor =
            CompletionAnchor::for_view((0.0, 0.0), &panes, (100.0, 50.0), (10.0, 20.0), 8.0)
                .unwrap();
        assert_eq!(anchor, CompletionAnchor { x: 758.0, y: 950.0 });
        let layout =
            CompletionLayout::calculate_at_anchor(&info(20, Some(18)), anchor, view()).unwrap();
        assert_eq!(layout.x + layout.width, 892.0);
        assert_eq!(layout.y + layout.height, anchor.y);
        assert!((layout.start..layout.end).contains(&18));
    }

    #[test]
    fn popup_counts_header_items_and_footer_and_keeps_eight_items_visible() {
        for count in [1, 8, 9, 100] {
            let layout =
                CompletionLayout::calculate(&info(count, Some(0)), (0.0, 0.0), view()).unwrap();
            assert!(layout.header);
            assert_eq!(layout.footer, count > 8);
            assert_eq!(layout.end - layout.start, count.min(8));
            let rows = 1 + count.min(8) + usize::from(count > 8);
            assert_eq!(layout.height, EDGE * 2.0 + rows as f32 * layout.row_height);
            assert!(
                layout.items_y() + (layout.end - layout.start) as f32 * layout.row_height
                    <= layout.y + layout.height - EDGE
            );
        }
        let last = CompletionLayout::calculate(&info(8, Some(7)), (0.0, 0.0), view()).unwrap();
        assert_eq!((last.start, last.end), (0, 8));
        let overflow =
            CompletionLayout::calculate(&info(100, Some(99)), (0.0, 0.0), view()).unwrap();
        assert_eq!((overflow.start, overflow.end), (92, 100));
        assert!(overflow.footer); // Shows the range even with only earlier items hidden.
    }

    #[test]
    fn popup_flips_above_cursor_and_can_fall_back_to_one_item() {
        let layout =
            CompletionLayout::calculate(&info(9, Some(8)), (1000.0, 45.0), view()).unwrap();
        assert!(layout.y + layout.height <= 958.0);
        assert_eq!(layout.x + layout.width, 892.0);
        let tiny = CompletionViewport {
            window_height: 102.0,
            ..view()
        };
        let layout = CompletionLayout::calculate(&info(100, Some(80)), (0.0, 0.0), tiny).unwrap();
        assert!(!layout.header);
        assert!(!layout.footer);
        assert_eq!((layout.start, layout.end), (80, 81));
        assert_eq!(layout.y, 58.0);
        assert_eq!(layout.height, 36.0);
    }

    #[test]
    fn popup_bounds_and_selected_item_survive_viewport_scale_and_stale_indices() {
        for width in [1.0, 40.0, 80.0, 320.0, 800.0] {
            for height in [1.0, 70.0, 110.0, 250.0, 600.0] {
                for (cw, ch) in [(8.0, 16.0), (10.5, 23.4), (24.0, 48.0)] {
                    let viewport = CompletionViewport {
                        content_width: width,
                        window_height: height,
                        cell_width: cw,
                        cell_height: ch,
                        ..view()
                    };
                    for count in [1, 8, 9, 100] {
                        for selected in [None, Some(0), Some(7), Some(count - 1), Some(usize::MAX)]
                        {
                            for cursor in [(-5.0, -5.0), (0.0, 0.0), (15.0, 2.0), (999.0, 999.0)] {
                                let layout = CompletionLayout::calculate(
                                    &info(count, selected),
                                    cursor,
                                    viewport,
                                );
                                assert_eq!(layout.is_some(), viewport.bounds().is_some());
                                let Some(layout) = layout else { continue };
                                let bounds = viewport.bounds().unwrap();
                                assert!(layout.x >= bounds.left && layout.y >= bounds.top);
                                assert!(layout.x + layout.width <= bounds.right + 0.001);
                                assert!(layout.y + layout.height <= bounds.bottom + 0.001);
                                assert!(layout.start < layout.end && layout.end <= count);
                                let selected = selected.unwrap_or(0).min(count - 1);
                                assert!((layout.start..layout.end).contains(&selected));
                                assert!(layout.end - layout.start <= MAX_ITEMS);
                                let end = layout.items_y()
                                    + (layout.end - layout.start + usize::from(layout.footer))
                                        as f32
                                        * layout.row_height;
                                assert!((end - (layout.y + layout.height - EDGE)).abs() < 0.001);
                            }
                        }
                    }
                }
            }
        }
        assert!(CompletionLayout::calculate(&info(0, None), (0.0, 0.0), view()).is_none());
        assert!(CompletionLayout::calculate(&info(1, None), (f32::NAN, 0.0), view()).is_none());
        for viewport in [
            CompletionViewport {
                cell_width: 0.0,
                ..view()
            },
            CompletionViewport {
                cell_height: f32::INFINITY,
                ..view()
            },
            CompletionViewport {
                padding: -1.0,
                ..view()
            },
        ] {
            assert!(viewport.bounds().is_none());
        }
    }

    #[test]
    fn popup_text_queues_only_visible_scalars_and_reserves_ellipsis() {
        for (text, cells, expected) in [
            ("main", 0, ""),
            ("main", 1, "…"),
            ("main", 4, "main"),
            ("main", 5, "main"),
            ("界ébranch", 3, "界é…"),
            ("e\u{301}x", 2, "e…"),
        ] {
            assert_eq!(
                completion_text_chars(text, cells).collect::<String>(),
                expected
            );
        }
        let huge = "x".repeat(4 * 1024 * 1024);
        assert_eq!(completion_text_chars(&huge, 3).collect::<String>(), "xx…");
    }
}
