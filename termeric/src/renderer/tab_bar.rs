//! Tab bar rendering and hit-testing

use super::types::*;
use super::{Renderer, adjust_alpha, contrast_text};
use crate::tabs::{TabInfo, TabType};

const MIN_TAB_WIDTH: f32 = 80.0;
const MAX_TAB_WIDTH: f32 = 200.0;
const PREFERRED_VERTICAL_TAB_WIDTH: f32 = 220.0;
const MIN_VERTICAL_TAB_WIDTH: f32 = 160.0;
const MIN_TERMINAL_CONTENT_WIDTH: f32 = 160.0;

#[derive(Debug, Clone, Copy, PartialEq)]
struct TabSlot {
    index: usize,
    x: f32,
    width: f32,
}

#[derive(Debug, Clone, PartialEq)]
struct TabBarLayout {
    tabs: Vec<TabSlot>,
    add_x: f32,
    previous_x: Option<f32>,
    next_x: Option<f32>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct VerticalTabSlot {
    index: usize,
    y: f32,
    height: f32,
}

#[derive(Debug, Clone, PartialEq)]
struct VerticalTabBarLayout {
    tabs: Vec<VerticalTabSlot>,
    add_y: f32,
    previous_y: Option<f32>,
    next_y: Option<f32>,
}

fn vertical_tab_bar_width(window_width: f32, scale_factor: f32) -> f32 {
    let scale_factor = scale_factor.max(0.1);
    let window_width = window_width.max(0.0);
    let max_rail = (window_width - MIN_TERMINAL_CONTENT_WIDTH * scale_factor)
        .max(window_width * 0.5)
        .min(window_width);
    (PREFERRED_VERTICAL_TAB_WIDTH * scale_factor)
        .min(max_rail)
        .max((MIN_VERTICAL_TAB_WIDTH * scale_factor).min(max_rail))
}

fn vertical_tab_bar_layout(
    tabs: &[TabInfo],
    window_height: f32,
    row_height: f32,
    anchor: usize,
) -> VerticalTabBarLayout {
    let rail_height = (window_height - row_height).max(0.0);
    let tab_space = (rail_height - row_height).max(0.0);
    let overflow = !tabs.is_empty() && tabs.len() as f32 * row_height > tab_space;
    if !overflow {
        let mut y = row_height;
        let slots = tabs
            .iter()
            .enumerate()
            .map(|(index, tab)| {
                let height = row_height * tab_animation_multiplier(tab);
                let slot = VerticalTabSlot { index, y, height };
                y += height;
                slot
            })
            .collect();
        return VerticalTabBarLayout {
            tabs: slots,
            add_y: y.min((window_height - row_height).max(row_height)),
            previous_y: None,
            next_y: None,
        };
    }

    let viewport_y = row_height * 2.0;
    let viewport_height = (window_height - row_height * 4.0).max(row_height);
    let visible_count = ((viewport_height / row_height).floor() as usize)
        .max(1)
        .min(tabs.len());
    let anchor = anchor.min(tabs.len() - 1);
    let start = anchor
        .saturating_sub(visible_count / 2)
        .min(tabs.len() - visible_count);
    let end = start + visible_count;
    let mut y = viewport_y;
    let slots = tabs[start..end]
        .iter()
        .enumerate()
        .map(|(offset, tab)| {
            let height = row_height * tab_animation_multiplier(tab);
            let slot = VerticalTabSlot {
                index: start + offset,
                y,
                height,
            };
            y += height;
            slot
        })
        .collect();
    VerticalTabBarLayout {
        tabs: slots,
        add_y: (window_height - row_height).max(row_height),
        previous_y: (start > 0).then_some(row_height),
        next_y: (end < tabs.len()).then_some((window_height - row_height * 2.0).max(row_height)),
    }
}

fn vertical_tab_bar_hit_test(
    layout: &VerticalTabBarLayout,
    tabs: &[TabInfo],
    x: f32,
    y: f32,
    rail_width: f32,
    row_height: f32,
    close_size: f32,
) -> TabBarClick {
    if x < 0.0 || x >= rail_width {
        return TabBarClick::None;
    }
    if layout
        .previous_y
        .is_some_and(|top| y >= top && y < top + row_height)
    {
        return TabBarClick::PreviousTab;
    }
    if layout
        .next_y
        .is_some_and(|top| y >= top && y < top + row_height)
    {
        return TabBarClick::NextTab;
    }
    if y >= layout.add_y && y < layout.add_y + row_height {
        return TabBarClick::NewTab;
    }
    for slot in &layout.tabs {
        if y < slot.y || y >= slot.y + slot.height {
            continue;
        }
        let close_x = rail_width - close_size - 6.0;
        let close_y = slot.y + (slot.height - close_size) / 2.0;
        if tab_animation_multiplier(&tabs[slot.index]) > 0.3
            && x >= close_x
            && x <= close_x + close_size
            && y >= close_y
            && y <= close_y + close_size
        {
            return TabBarClick::CloseTab(slot.index);
        }
        return TabBarClick::Tab(slot.index);
    }
    TabBarClick::None
}

fn tab_bar_layout(tabs: &[TabInfo], window_width: f32, height: f32, anchor: usize) -> TabBarLayout {
    let tab_strip_end = (window_width - height * 4.0).max(0.0);
    let overflow = !tabs.is_empty() && tabs.len() as f32 * MIN_TAB_WIDTH > tab_strip_end;
    if !overflow {
        let width = (tab_strip_end / tabs.len().max(1) as f32).clamp(MIN_TAB_WIDTH, MAX_TAB_WIDTH);
        let mut x = 0.0;
        let slots = tabs
            .iter()
            .enumerate()
            .map(|(index, tab)| {
                let animated_width = width * tab_animation_multiplier(tab);
                let slot = TabSlot {
                    index,
                    x,
                    width: animated_width,
                };
                x += animated_width;
                slot
            })
            .collect();
        return TabBarLayout {
            tabs: slots,
            add_x: x.min(tab_strip_end),
            previous_x: None,
            next_x: None,
        };
    }

    // Overflow controls permanently reserve one square on each side. Keeping
    // their geometry stable prevents the tab strip from jumping as the active
    // tab reaches either end.
    let viewport_x = height;
    let viewport_width = (tab_strip_end - height * 2.0).max(1.0);
    let visible_count = ((viewport_width / MIN_TAB_WIDTH).floor() as usize)
        .max(1)
        .min(tabs.len());
    let anchor = anchor.min(tabs.len() - 1);
    let start = anchor
        .saturating_sub(visible_count / 2)
        .min(tabs.len() - visible_count);
    let end = start + visible_count;
    let width = (viewport_width / visible_count as f32).min(MAX_TAB_WIDTH);
    let mut x = viewport_x;
    let slots = tabs[start..end]
        .iter()
        .enumerate()
        .map(|(offset, tab)| {
            let animated_width = width * tab_animation_multiplier(tab);
            let slot = TabSlot {
                index: start + offset,
                x,
                width: animated_width,
            };
            x += animated_width;
            slot
        })
        .collect();
    TabBarLayout {
        tabs: slots,
        add_x: tab_strip_end,
        previous_x: (start > 0).then_some(0.0),
        next_x: (end < tabs.len()).then_some(tab_strip_end - height),
    }
}

impl Renderer {
    /// Width reserved by a vertical tab rail for the current viewport.
    pub fn tab_bar_width(&self) -> f32 {
        self.tab_bar_width_for(self.window_size().0 as f32)
    }

    /// Width reserved by a vertical tab rail for an explicit viewport.
    pub fn tab_bar_width_for(&self, window_width: f32) -> f32 {
        if self.tab_bar_position == crate::config::TabBarPosition::Left {
            vertical_tab_bar_width(window_width, self.scale_factor)
        } else {
            0.0
        }
    }

    pub(super) fn build_tab_bar(
        &mut self,
        tabs: &[TabInfo],
        window_width: f32,
        window_height: f32,
        tab_bar_height: f32,
    ) {
        if self.tab_bar_position == crate::config::TabBarPosition::Left {
            self.build_vertical_tab_bar(tabs, window_width, window_height, tab_bar_height);
            return;
        }
        let tab_bar_bg = self.ui.tab_bar_bg;
        self.rect_pipeline
            .push_rect(0.0, 0.0, window_width, tab_bar_height, tab_bar_bg);

        let active_tab_bg = self.ui.tab_active;
        let inactive_tab_bg = self.ui.tab_inactive;
        let activity_indicator = self.ui.tab_activity;
        let active_text_color = self.ui.tab_text_active;
        let inactive_text_color = self.ui.tab_text_inactive;

        // Reserve space for the session dropdown and gear icon on the right.
        // The + button floats after the last tab, not in the reserved zone.
        let gear_button_width = tab_bar_height;
        let session_button_width = tab_bar_height;
        let commands_button_width = tab_bar_height;
        let right_buttons_width = gear_button_width + session_button_width + commands_button_width;
        let layout = self.tab_bar_layout(tabs, window_width);
        let tab_padding = 8.0;
        let tab_height = tab_bar_height - 4.0; // Leave some margin
        let show_close_buttons = true;
        // Reserve space for close button inside tab
        let close_btn_size = self.cell_height;
        let close_btn_margin = 4.0;

        for slot in &layout.tabs {
            let i = slot.index;
            let tab = &tabs[i];
            // Calculate animation multiplier based on slide-in and close animations
            let anim_multiplier = tab_animation_multiplier(tab);

            // Animated tab width
            let tab_width = slot.width;

            let x = slot.x;
            let y = 2.0; // Small top margin

            // Only render if the tab has some visible width
            if tab_width > 2.0 {
                // Tab background with animated alpha for fade effect
                let base_bg_color = if tab.is_active {
                    active_tab_bg
                } else if tab.has_activity {
                    activity_indicator
                } else {
                    inactive_tab_bg
                };

                // Fade the background based on animation progress
                let bg_color = [
                    base_bg_color[0],
                    base_bg_color[1],
                    base_bg_color[2],
                    base_bg_color[3] * anim_multiplier,
                ];

                self.rect_pipeline.push_rounded_rect(
                    x + 1.0,
                    y,
                    tab_width - 2.0,
                    tab_height,
                    bg_color,
                    4.0,
                );

                // Tab text - only render when animation progress is significant
                if anim_multiplier > 0.3 {
                    let text_alpha = ((anim_multiplier - 0.3) / 0.7).min(1.0); // Fade text

                    let base_text_color = if tab.is_active {
                        active_text_color
                    } else {
                        inactive_text_color
                    };

                    let text_color = [
                        base_text_color[0],
                        base_text_color[1],
                        base_text_color[2],
                        base_text_color[3] * text_alpha,
                    ];

                    // Render tab title (truncate if needed)
                    // Reserve space for close button when visible
                    let right_reserve = if show_close_buttons {
                        close_btn_size + close_btn_margin * 2.0
                    } else {
                        tab_padding
                    };
                    let title = if tab.title.chars().count() > 15 {
                        format!("{}...", tab.title.chars().take(12).collect::<String>())
                    } else {
                        tab.title.clone()
                    };

                    // Add tab number prefix
                    let status = if tab.broadcast_active {
                        "!! "
                    } else if tab.broadcast_armed {
                        "?? "
                    } else if tab.read_only {
                        "RO "
                    } else if tab.pinned {
                        "PIN "
                    } else {
                        ""
                    };
                    let label = format!("{}{}: {}", status, i + 1, title);

                    // Render each character of the label
                    let text_y = y + (tab_height - self.cell_height) / 2.0; // Center vertically
                    let mut text_x = x + tab_padding;

                    // Plugin accent bar before the label — a short vertical
                    // stripe rather than a single glyph dot, so AI-backed
                    // tabs read as distinct at a glance even in crowded
                    // tab bars.
                    if matches!(&tab.tab_type, TabType::Plugin { .. }) {
                        let bar_x = text_x;
                        let bar_w = 3.0;
                        let bar_y = y + tab_height * 0.18;
                        let bar_h = tab_height * 0.64;
                        let accent = [
                            self.ui.palette_accent[0],
                            self.ui.palette_accent[1],
                            self.ui.palette_accent[2],
                            text_alpha,
                        ];
                        self.rect_pipeline
                            .push_rounded_rect(bar_x, bar_y, bar_w, bar_h, accent, 1.5);
                        text_x += bar_w + self.cell_width * 0.5;
                    }

                    for c in label.chars() {
                        if text_x + self.cell_width > x + tab_width - right_reserve {
                            break; // Don't overflow into close button area
                        }
                        if !c.is_whitespace() {
                            self.text_pipeline.push_char(c, text_x, text_y, text_color);
                        }
                        text_x += self.cell_width * 0.85;
                    }

                    // Diff count pill badge
                    if tab.unreviewed_diff_count > 0 {
                        let badge_x = text_x + 2.0;
                        let badge_y = text_y + 1.0;
                        let badge_w = self.cell_width * 1.8;
                        let badge_h = self.cell_height - 2.0;
                        // Only show if there's room before the close button
                        if badge_x + badge_w < x + tab_width - right_reserve {
                            let badge_color = [
                                self.ui.palette_accent[0],
                                self.ui.palette_accent[1],
                                self.ui.palette_accent[2],
                                0.8 * text_alpha,
                            ];
                            self.rect_pipeline.push_rounded_rect(
                                badge_x,
                                badge_y,
                                badge_w,
                                badge_h,
                                badge_color,
                                3.0,
                            );
                            // Badge text (diff count)
                            let count_str = format!("{}", tab.unreviewed_diff_count);
                            let count_x = badge_x
                                + (badge_w - count_str.len() as f32 * self.cell_width * 0.85) / 2.0;
                            let count_color = [0.0, 0.0, 0.0, text_alpha];
                            for c in count_str.chars() {
                                self.text_pipeline
                                    .push_char(c, count_x, text_y, count_color);
                            }
                        }
                    }

                    // ── Close (×) button ──
                    if show_close_buttons {
                        let close_x = x + tab_width - close_btn_size - close_btn_margin - 2.0;
                        let close_y = y + (tab_height - close_btn_size) / 2.0;

                        // Subtle rounded background for close button
                        let close_bg = adjust_alpha(inactive_tab_bg, 0.4 * text_alpha);
                        self.rect_pipeline.push_rounded_rect(
                            close_x,
                            close_y,
                            close_btn_size,
                            close_btn_size,
                            close_bg,
                            3.0,
                        );

                        // Draw × close icon — two smooth anti-aliased diagonal lines
                        {
                            let close_hover = matches!(self.tab_bar_hover, Some(TabBarClick::CloseTab(idx)) if idx == i);
                            let hover_t = if close_hover {
                                self.tab_bar_hover_progress
                            } else {
                                0.0
                            };
                            let scale = 1.0 + hover_t * 0.2;
                            let base_alpha = text_color[3] * (0.5 + hover_t * 0.5);
                            let close_color = [
                                (text_color[0] + hover_t * 0.3).min(1.0),
                                text_color[1],
                                text_color[2],
                                base_alpha,
                            ];
                            let arm = (close_btn_size * 0.25).min(4.5) * scale;
                            let line_w = 1.5 + hover_t * 0.5;
                            let cx = close_x + close_btn_size / 2.0;
                            let cy = close_y + close_btn_size / 2.0;
                            // Top-left to bottom-right
                            self.rect_pipeline.push_line(
                                cx - arm,
                                cy - arm,
                                cx + arm,
                                cy + arm,
                                line_w,
                                close_color,
                            );
                            // Top-right to bottom-left
                            self.rect_pipeline.push_line(
                                cx + arm,
                                cy - arm,
                                cx - arm,
                                cy + arm,
                                line_w,
                                close_color,
                            );
                        }
                    }
                }
            }
        }

        let border_color = adjust_alpha(self.ui.tab_border, 0.7);
        self.rect_pipeline
            .push_rect(0.0, tab_bar_height - 1.0, window_width, 1.0, border_color);

        // Helper: compute hover animation for a specific button
        let hover_for = |target: TabBarClick| -> f32 {
            if self.tab_bar_hover == Some(target) {
                self.tab_bar_hover_progress
            } else {
                0.0
            }
        };

        // ── Tab overflow controls ──
        for (target, x, points_right) in [
            (TabBarClick::PreviousTab, 0.0, false),
            (
                TabBarClick::NextTab,
                window_width - right_buttons_width - tab_bar_height * 2.0,
                true,
            ),
        ] {
            let enabled = match target {
                TabBarClick::PreviousTab => layout.previous_x.is_some(),
                TabBarClick::NextTab => layout.next_x.is_some(),
                _ => false,
            };
            if layout.previous_x.is_none() && layout.next_x.is_none() && !enabled {
                continue;
            }
            let hover_t = if enabled { hover_for(target) } else { 0.0 };
            let alpha = if enabled { 0.75 + hover_t * 0.25 } else { 0.22 };
            let bg = adjust_alpha(self.ui.tab_inactive, 0.35 + hover_t * 0.25);
            self.rect_pipeline.push_rounded_rect(
                x + 2.0,
                2.0,
                tab_bar_height - 4.0,
                tab_bar_height - 6.0,
                bg,
                4.0,
            );
            let cx = x + tab_bar_height / 2.0;
            let cy = tab_bar_height / 2.0;
            let arm = (tab_bar_height * 0.16).min(5.5) * (1.0 + hover_t * 0.15);
            let direction = if points_right { 1.0 } else { -1.0 };
            let color = adjust_alpha(self.ui.tab_text_inactive, alpha);
            self.rect_pipeline.push_line(
                cx - direction * arm * 0.45,
                cy - arm,
                cx + direction * arm * 0.45,
                cy,
                1.5 + hover_t * 0.3,
                color,
            );
            self.rect_pipeline.push_line(
                cx + direction * arm * 0.45,
                cy,
                cx - direction * arm * 0.45,
                cy + arm,
                1.5 + hover_t * 0.3,
                color,
            );
        }

        // ── Add (+) tab button — positioned right after the last tab ──
        let add_size = tab_bar_height;
        let add_x = layout.add_x;
        let add_y = 0.0;
        {
            let hover_t = hover_for(TabBarClick::NewTab);
            let bg_alpha = 0.35 + hover_t * 0.25;
            let add_bg = adjust_alpha(self.ui.tab_inactive, bg_alpha);
            self.rect_pipeline.push_rounded_rect(
                add_x + 2.0,
                add_y + 2.0,
                add_size - 4.0,
                tab_bar_height - 6.0,
                add_bg,
                4.0,
            );
            // Plus icon — two perpendicular lines
            let scale = 1.0 + hover_t * 0.15;
            let arm = (add_size * 0.16).min(5.5) * scale;
            let line_w = 1.5 + hover_t * 0.3;
            let cx = add_x + add_size / 2.0;
            let cy = add_y + tab_bar_height / 2.0;
            let brightness = 1.0 + hover_t * 0.3;
            let color = [
                (self.ui.tab_text_inactive[0] * brightness).min(1.0),
                (self.ui.tab_text_inactive[1] * brightness).min(1.0),
                (self.ui.tab_text_inactive[2] * brightness).min(1.0),
                self.ui.tab_text_inactive[3],
            ];
            // Horizontal bar
            self.rect_pipeline
                .push_line(cx - arm, cy, cx + arm, cy, line_w, color);
            // Vertical bar
            self.rect_pipeline
                .push_line(cx, cy - arm, cx, cy + arm, line_w, color);
        }

        // ── Session dropdown button ──
        let session_size = tab_bar_height;
        let session_x = window_width - gear_button_width - session_size;
        let session_y = 0.0;
        {
            let hover_t = hover_for(TabBarClick::SessionDropdown);
            let bg_alpha = 0.35 + hover_t * 0.25;
            let session_bg = adjust_alpha(self.ui.tab_inactive, bg_alpha);
            self.rect_pipeline.push_rounded_rect(
                session_x + 2.0,
                session_y + 2.0,
                session_size - 4.0,
                tab_bar_height - 6.0,
                session_bg,
                4.0,
            );
            // Chevron (▾) — two angled lines forming a V shape
            let scale = 1.0 + hover_t * 0.15;
            let arm = (session_size * 0.18).min(6.0) * scale;
            let cx = session_x + session_size / 2.0;
            let cy = session_y + tab_bar_height / 2.0;
            let line_w = 1.5 + hover_t * 0.3;
            let brightness = 1.0 + hover_t * 0.3;
            let color = [
                (self.ui.tab_text_inactive[0] * brightness).min(1.0),
                (self.ui.tab_text_inactive[1] * brightness).min(1.0),
                (self.ui.tab_text_inactive[2] * brightness).min(1.0),
                self.ui.tab_text_inactive[3],
            ];
            let half_h = arm * 0.55;
            // Left arm: top-left to center-bottom
            self.rect_pipeline
                .push_line(cx - arm, cy - half_h, cx, cy + half_h, line_w, color);
            // Right arm: center-bottom to top-right
            self.rect_pipeline
                .push_line(cx, cy + half_h, cx + arm, cy - half_h, line_w, color);
        }

        // ── Commands button (⚡) ──
        let commands_size = tab_bar_height;
        let commands_x = window_width - gear_button_width - session_button_width - commands_size;
        let commands_y = 0.0;
        {
            let hover_t = hover_for(TabBarClick::CommandsButton);
            let bg_alpha = 0.35 + hover_t * 0.25;
            let commands_bg = adjust_alpha(self.ui.tab_inactive, bg_alpha);
            self.rect_pipeline.push_rounded_rect(
                commands_x + 2.0,
                commands_y + 2.0,
                commands_size - 4.0,
                tab_bar_height - 6.0,
                commands_bg,
                4.0,
            );
            // ⚡ lightning bolt icon — drawn as two angled segments
            let scale = 1.0 + hover_t * 0.15;
            let cx = commands_x + commands_size / 2.0;
            let cy = commands_y + tab_bar_height / 2.0;
            let h = (commands_size * 0.28).min(9.0) * scale;
            let w = h * 0.4;
            let line_w = 1.5 + hover_t * 0.3;
            let brightness = 1.0 + hover_t * 0.3;
            let color = [
                (self.ui.tab_text_inactive[0] * brightness).min(1.0),
                (self.ui.tab_text_inactive[1] * brightness).min(1.0),
                (self.ui.tab_text_inactive[2] * brightness).min(1.0),
                self.ui.tab_text_inactive[3],
            ];
            // Top segment: upper-right to center-left
            self.rect_pipeline
                .push_line(cx + w * 0.3, cy - h, cx - w * 0.5, cy, line_w, color);
            // Middle bar: left to right through center
            self.rect_pipeline.push_line(
                cx - w * 0.3,
                cy - line_w * 0.25,
                cx + w * 0.7,
                cy - line_w * 0.25,
                line_w,
                color,
            );
            // Bottom segment: center-right to lower-left
            self.rect_pipeline
                .push_line(cx + w * 0.5, cy, cx - w * 0.3, cy + h, line_w, color);
        }

        // ── Settings icon button (right side of tab bar) ──
        let gear_size = tab_bar_height;
        let gear_x = window_width - gear_size;
        let gear_y = 0.0;
        {
            let hover_t = hover_for(TabBarClick::Gear);
            let bg_alpha = 0.5 + hover_t * 0.2;
            let gear_bg = adjust_alpha(self.ui.tab_inactive, bg_alpha);
            self.rect_pipeline.push_rounded_rect(
                gear_x + 2.0,
                gear_y + 2.0,
                gear_size - 4.0,
                tab_bar_height - 6.0,
                gear_bg,
                4.0,
            );
            // Three horizontal bars (hamburger) with varying widths
            let scale = 1.0 + hover_t * 0.15;
            let base_w = (gear_size * 0.16).min(5.5) * scale;
            let gap = 3.0_f32;
            let total_h = gap * 2.0;
            let cx = gear_x + gear_size / 2.0;
            let top_y = gear_y + (tab_bar_height - total_h) / 2.0;
            let line_w = 1.5 + hover_t * 0.3;
            let brightness = 1.0 + hover_t * 0.3;
            let color = [
                (self.ui.tab_text_inactive[0] * brightness).min(1.0),
                (self.ui.tab_text_inactive[1] * brightness).min(1.0),
                (self.ui.tab_text_inactive[2] * brightness).min(1.0),
                self.ui.tab_text_inactive[3],
            ];
            let widths = [base_w * 0.8, base_w, base_w * 0.8];
            for (i, w) in widths.into_iter().enumerate() {
                let y = top_y + i as f32 * gap;
                self.rect_pipeline
                    .push_line(cx - w, y, cx + w, y, line_w, color);
            }
        }
    }

    fn build_vertical_tab_bar(
        &mut self,
        tabs: &[TabInfo],
        window_width: f32,
        window_height: f32,
        row_height: f32,
    ) {
        let rail_width = self.tab_bar_width_for(window_width);
        let tab_bar_bg = self.ui.tab_bar_bg;
        self.rect_pipeline
            .push_rect(0.0, 0.0, window_width, row_height, tab_bar_bg);
        self.rect_pipeline.push_rect(
            0.0,
            row_height,
            rail_width,
            (window_height - row_height).max(0.0),
            tab_bar_bg,
        );

        let layout = self.vertical_tab_bar_layout(tabs, window_height);
        let tab_padding = 8.0;
        let close_size = self.cell_height;
        let close_margin = 4.0;
        for slot in &layout.tabs {
            let tab = &tabs[slot.index];
            let animation = tab_animation_multiplier(tab);
            if slot.height <= 2.0 {
                continue;
            }
            let y = slot.y + 1.0;
            let height = (slot.height - 2.0).max(0.0);
            let base = if tab.is_active {
                self.ui.tab_active
            } else if tab.has_activity {
                self.ui.tab_activity
            } else {
                self.ui.tab_inactive
            };
            self.rect_pipeline.push_rounded_rect(
                2.0,
                y,
                (rail_width - 4.0).max(0.0),
                height,
                [base[0], base[1], base[2], base[3] * animation],
                4.0,
            );
            if animation <= 0.3 {
                continue;
            }

            let text_alpha = ((animation - 0.3) / 0.7).min(1.0);
            let base_text = if tab.is_active {
                self.ui.tab_text_active
            } else {
                self.ui.tab_text_inactive
            };
            let text_color = [
                base_text[0],
                base_text[1],
                base_text[2],
                base_text[3] * text_alpha,
            ];
            let status = if tab.broadcast_active {
                "!! "
            } else if tab.broadcast_armed {
                "?? "
            } else if tab.read_only {
                "RO "
            } else if tab.pinned {
                "PIN "
            } else {
                ""
            };
            let label = format!("{}{}: {}", status, slot.index + 1, tab.title);
            let text_y = y + (height - self.cell_height) / 2.0;
            let mut text_x = tab_padding;
            if matches!(&tab.tab_type, TabType::Plugin { .. }) {
                let bar_h = height * 0.64;
                self.rect_pipeline.push_rounded_rect(
                    text_x,
                    y + height * 0.18,
                    3.0,
                    bar_h,
                    adjust_alpha(self.ui.palette_accent, text_alpha),
                    1.5,
                );
                text_x += 3.0 + self.cell_width * 0.5;
            }
            let right_reserve = close_size + close_margin * 2.0;
            for character in label.chars() {
                if text_x + self.cell_width > rail_width - right_reserve {
                    break;
                }
                if !character.is_whitespace() {
                    self.text_pipeline
                        .push_char(character, text_x, text_y, text_color);
                }
                text_x += self.cell_width * 0.85;
            }
            if tab.unreviewed_diff_count > 0 {
                let badge_w = self.cell_width * 1.8;
                let badge_h = (self.cell_height - 2.0).max(1.0);
                let badge_x = text_x + 2.0;
                if badge_x + badge_w < rail_width - right_reserve {
                    self.rect_pipeline.push_rounded_rect(
                        badge_x,
                        text_y + 1.0,
                        badge_w,
                        badge_h,
                        adjust_alpha(self.ui.palette_accent, 0.8 * text_alpha),
                        3.0,
                    );
                    let count = tab.unreviewed_diff_count.to_string();
                    let mut count_x =
                        badge_x + (badge_w - count.len() as f32 * self.cell_width * 0.85) / 2.0;
                    for character in count.chars() {
                        self.text_pipeline.push_char(
                            character,
                            count_x,
                            text_y,
                            [0.0, 0.0, 0.0, text_alpha],
                        );
                        count_x += self.cell_width * 0.85;
                    }
                }
            }

            let close_x = rail_width - close_size - close_margin - 2.0;
            let close_y = y + (height - close_size) / 2.0;
            let close_hover = matches!(self.tab_bar_hover, Some(TabBarClick::CloseTab(index)) if index == slot.index);
            let hover = if close_hover {
                self.tab_bar_hover_progress
            } else {
                0.0
            };
            self.rect_pipeline.push_rounded_rect(
                close_x,
                close_y,
                close_size,
                close_size,
                adjust_alpha(self.ui.tab_inactive, 0.4 * text_alpha),
                3.0,
            );
            let arm = (close_size * 0.25).min(4.5) * (1.0 + hover * 0.2);
            let center_x = close_x + close_size / 2.0;
            let center_y = close_y + close_size / 2.0;
            let close_color = adjust_alpha(text_color, 0.5 + hover * 0.5);
            self.rect_pipeline.push_line(
                center_x - arm,
                center_y - arm,
                center_x + arm,
                center_y + arm,
                1.5 + hover * 0.5,
                close_color,
            );
            self.rect_pipeline.push_line(
                center_x + arm,
                center_y - arm,
                center_x - arm,
                center_y + arm,
                1.5 + hover * 0.5,
                close_color,
            );
        }

        let hover_for = |target: TabBarClick| {
            if self.tab_bar_hover == Some(target) {
                self.tab_bar_hover_progress
            } else {
                0.0
            }
        };
        for (target, y, points_down) in [
            (TabBarClick::PreviousTab, layout.previous_y, false),
            (TabBarClick::NextTab, layout.next_y, true),
        ] {
            let Some(y) = y else { continue };
            let hover = hover_for(target);
            self.rect_pipeline.push_rounded_rect(
                2.0,
                y + 2.0,
                rail_width - 4.0,
                row_height - 4.0,
                adjust_alpha(self.ui.tab_inactive, 0.35 + hover * 0.25),
                4.0,
            );
            let center_x = rail_width / 2.0;
            let center_y = y + row_height / 2.0;
            let arm = (row_height * 0.16).min(5.5) * (1.0 + hover * 0.15);
            let direction = if points_down { 1.0 } else { -1.0 };
            let color = adjust_alpha(self.ui.tab_text_inactive, 0.75 + hover * 0.25);
            self.rect_pipeline.push_line(
                center_x - arm,
                center_y - direction * arm * 0.45,
                center_x,
                center_y + direction * arm * 0.45,
                1.5 + hover * 0.3,
                color,
            );
            self.rect_pipeline.push_line(
                center_x,
                center_y + direction * arm * 0.45,
                center_x + arm,
                center_y - direction * arm * 0.45,
                1.5 + hover * 0.3,
                color,
            );
        }

        let add_hover = hover_for(TabBarClick::NewTab);
        self.rect_pipeline.push_rounded_rect(
            2.0,
            layout.add_y + 2.0,
            rail_width - 4.0,
            row_height - 4.0,
            adjust_alpha(self.ui.tab_inactive, 0.35 + add_hover * 0.25),
            4.0,
        );
        let add_center_x = rail_width / 2.0;
        let add_center_y = layout.add_y + row_height / 2.0;
        let add_arm = (row_height * 0.16).min(5.5) * (1.0 + add_hover * 0.15);
        let add_color = adjust_alpha(self.ui.tab_text_inactive, 0.9 + add_hover * 0.1);
        self.rect_pipeline.push_line(
            add_center_x - add_arm,
            add_center_y,
            add_center_x + add_arm,
            add_center_y,
            1.5 + add_hover * 0.3,
            add_color,
        );
        self.rect_pipeline.push_line(
            add_center_x,
            add_center_y - add_arm,
            add_center_x,
            add_center_y + add_arm,
            1.5 + add_hover * 0.3,
            add_color,
        );

        self.build_top_toolbar_controls(window_width, row_height);
        let border = adjust_alpha(self.ui.tab_border, 0.7);
        self.rect_pipeline
            .push_rect(0.0, row_height - 1.0, window_width, 1.0, border);
        self.rect_pipeline.push_rect(
            rail_width - 1.0,
            row_height,
            1.0,
            (window_height - row_height).max(0.0),
            border,
        );
    }

    fn build_top_toolbar_controls(&mut self, window_width: f32, height: f32) {
        let hover_for = |target: TabBarClick| {
            if self.tab_bar_hover == Some(target) {
                self.tab_bar_hover_progress
            } else {
                0.0
            }
        };
        for (offset, target) in [
            (3.0, TabBarClick::CommandsButton),
            (2.0, TabBarClick::SessionDropdown),
            (1.0, TabBarClick::Gear),
        ] {
            let x = window_width - height * offset;
            let hover = hover_for(target);
            self.rect_pipeline.push_rounded_rect(
                x + 2.0,
                2.0,
                height - 4.0,
                height - 6.0,
                adjust_alpha(self.ui.tab_inactive, 0.4 + hover * 0.25),
                4.0,
            );
            let center_x = x + height / 2.0;
            let center_y = height / 2.0;
            let arm = (height * 0.16).min(5.5) * (1.0 + hover * 0.15);
            let color = adjust_alpha(self.ui.tab_text_inactive, 0.8 + hover * 0.2);
            match target {
                TabBarClick::CommandsButton => {
                    self.rect_pipeline.push_line(
                        center_x + arm * 0.25,
                        center_y - arm,
                        center_x - arm * 0.45,
                        center_y,
                        1.5 + hover * 0.3,
                        color,
                    );
                    self.rect_pipeline.push_line(
                        center_x + arm * 0.45,
                        center_y,
                        center_x - arm * 0.25,
                        center_y + arm,
                        1.5 + hover * 0.3,
                        color,
                    );
                }
                TabBarClick::SessionDropdown => {
                    self.rect_pipeline.push_line(
                        center_x - arm,
                        center_y - arm * 0.4,
                        center_x,
                        center_y + arm * 0.4,
                        1.5 + hover * 0.3,
                        color,
                    );
                    self.rect_pipeline.push_line(
                        center_x,
                        center_y + arm * 0.4,
                        center_x + arm,
                        center_y - arm * 0.4,
                        1.5 + hover * 0.3,
                        color,
                    );
                }
                TabBarClick::Gear => {
                    for (index, width) in [0.8, 1.0, 0.8].into_iter().enumerate() {
                        let y = center_y + (index as f32 - 1.0) * 3.0;
                        self.rect_pipeline.push_line(
                            center_x - arm * width,
                            y,
                            center_x + arm * width,
                            y,
                            1.5 + hover * 0.3,
                            color,
                        );
                    }
                }
                _ => {}
            }
        }
    }

    /// Draw a warm, high-contrast focus outline without covering tab content.
    pub(super) fn build_tab_bar_keyboard_focus(
        &mut self,
        target: TabBarClick,
        tabs: &[TabInfo],
        window_width: f32,
        window_height: f32,
    ) {
        let Some((x, y, width, height)) =
            self.tab_bar_element_rect(target, tabs, window_width, window_height)
        else {
            return;
        };
        let inset = 3.0;
        let left = x + inset;
        let top = y + inset;
        let right = x + width - inset;
        let bottom = y + height - inset;
        if right <= left || bottom <= top {
            return;
        }
        let color = adjust_alpha(contrast_text(self.ui.tab_bar_bg), 0.98);
        let thickness = 2.0;
        self.rect_pipeline
            .push_line(left, top, right, top, thickness, color);
        self.rect_pipeline
            .push_line(right, top, right, bottom, thickness, color);
        self.rect_pipeline
            .push_line(right, bottom, left, bottom, thickness, color);
        self.rect_pipeline
            .push_line(left, bottom, left, top, thickness, color);
    }

    /// Exact rendered bounds for a tab-bar control. Accessibility and focus
    /// rendering share this geometry with the visible bar.
    pub fn tab_bar_element_rect(
        &self,
        target: TabBarClick,
        tabs: &[TabInfo],
        window_width: f32,
        window_height: f32,
    ) -> Option<(f32, f32, f32, f32)> {
        let height = self.tab_bar_height();
        if self.tab_bar_position == crate::config::TabBarPosition::Left {
            let rail_width = self.tab_bar_width_for(window_width);
            let layout = self.vertical_tab_bar_layout(tabs, window_height);
            return match target {
                TabBarClick::Tab(index) => {
                    let slot = layout.tabs.iter().find(|slot| slot.index == index)?;
                    Some((0.0, slot.y, rail_width, slot.height))
                }
                TabBarClick::CloseTab(index) => {
                    let slot = layout.tabs.iter().find(|slot| slot.index == index)?;
                    let size = self.cell_height;
                    Some((
                        rail_width - size - 6.0,
                        slot.y + (slot.height - size) / 2.0,
                        size,
                        size,
                    ))
                }
                TabBarClick::NewTab => Some((0.0, layout.add_y, rail_width, height)),
                TabBarClick::PreviousTab => layout.previous_y.map(|y| (0.0, y, rail_width, height)),
                TabBarClick::NextTab => layout.next_y.map(|y| (0.0, y, rail_width, height)),
                TabBarClick::CommandsButton => {
                    Some((window_width - height * 3.0, 0.0, height, height))
                }
                TabBarClick::SessionDropdown => {
                    Some((window_width - height * 2.0, 0.0, height, height))
                }
                TabBarClick::Gear => Some((window_width - height, 0.0, height, height)),
                TabBarClick::None => None,
            };
        }
        let layout = self.tab_bar_layout(tabs, window_width);
        let tab_height = height - 4.0;
        match target {
            TabBarClick::Tab(index) => {
                let slot = layout.tabs.iter().find(|slot| slot.index == index)?;
                Some((slot.x, 2.0, slot.width, tab_height))
            }
            TabBarClick::CloseTab(index) => {
                let slot = layout.tabs.iter().find(|slot| slot.index == index)?;
                let size = self.cell_height;
                Some((
                    slot.x + slot.width - size - 6.0,
                    2.0 + (tab_height - size) / 2.0,
                    size,
                    size,
                ))
            }
            TabBarClick::NewTab => Some((layout.add_x, 0.0, height, height)),
            TabBarClick::PreviousTab => layout.previous_x.map(|x| (x, 0.0, height, height)),
            TabBarClick::NextTab => layout.next_x.map(|x| (x, 0.0, height, height)),
            TabBarClick::CommandsButton => Some((window_width - height * 3.0, 0.0, height, height)),
            TabBarClick::SessionDropdown => {
                Some((window_width - height * 2.0, 0.0, height, height))
            }
            TabBarClick::Gear => Some((window_width - height, 0.0, height, height)),
            TabBarClick::None => None,
        }
    }

    pub fn tab_bar_hit_test(
        &self,
        x: f32,
        y: f32,
        tabs: &[TabInfo],
        window_width: f32,
        window_height: f32,
    ) -> TabBarClick {
        let tab_bar_height = self.tab_bar_height();

        if self.tab_bar_position == crate::config::TabBarPosition::Left {
            if y < tab_bar_height {
                if x >= window_width - tab_bar_height {
                    return TabBarClick::Gear;
                }
                if x >= window_width - tab_bar_height * 2.0 {
                    return TabBarClick::SessionDropdown;
                }
                if x >= window_width - tab_bar_height * 3.0 {
                    return TabBarClick::CommandsButton;
                }
                return TabBarClick::None;
            }
            let rail_width = self.tab_bar_width_for(window_width);
            if x >= rail_width || y >= window_height {
                return TabBarClick::None;
            }
            let layout = self.vertical_tab_bar_layout(tabs, window_height);
            return vertical_tab_bar_hit_test(
                &layout,
                tabs,
                x,
                y,
                rail_width,
                tab_bar_height,
                self.cell_height,
            );
        }

        // Only handle clicks within the tab bar vertical bounds
        if y >= tab_bar_height {
            return TabBarClick::None;
        }

        let gear_button_width = tab_bar_height;
        let session_button_width = tab_bar_height;
        let commands_button_width = tab_bar_height;

        // ── Gear icon (rightmost) ──
        let gear_x = window_width - gear_button_width;
        if x >= gear_x {
            return TabBarClick::Gear;
        }

        // ── Session dropdown (▾) button (left of gear) ──
        let session_x = window_width - gear_button_width - session_button_width;
        if x >= session_x {
            return TabBarClick::SessionDropdown;
        }

        // ── Commands (⚡) button (left of session) ──
        let commands_x =
            window_width - gear_button_width - session_button_width - commands_button_width;
        if x >= commands_x {
            return TabBarClick::CommandsButton;
        }

        let layout = self.tab_bar_layout(tabs, window_width);
        let tab_height = tab_bar_height - 4.0;
        let show_close_buttons = true;
        let close_btn_size = self.cell_height;
        let close_btn_margin = 4.0;

        if layout
            .previous_x
            .is_some_and(|left| x >= left && x < left + tab_bar_height)
        {
            return TabBarClick::PreviousTab;
        }
        if layout
            .next_x
            .is_some_and(|left| x >= left && x < left + tab_bar_height)
        {
            return TabBarClick::NextTab;
        }

        // ── Add (+) button — right after last tab ──
        if x >= layout.add_x && x < layout.add_x + tab_bar_height {
            return TabBarClick::NewTab;
        }

        for slot in &layout.tabs {
            let i = slot.index;
            let tab = &tabs[i];
            let anim_multiplier = tab_animation_multiplier(tab);
            let tab_width = slot.width;

            if x >= slot.x && x < slot.x + tab_width {
                // Click is within this tab — check if it hit the close button
                if show_close_buttons && tab_width > 2.0 && anim_multiplier > 0.3 {
                    let tab_y = 2.0;
                    let close_x = slot.x + tab_width - close_btn_size - close_btn_margin - 2.0;
                    let close_y = tab_y + (tab_height - close_btn_size) / 2.0;

                    if x >= close_x
                        && x <= close_x + close_btn_size
                        && y >= close_y
                        && y <= close_y + close_btn_size
                    {
                        return TabBarClick::CloseTab(i);
                    }
                }
                return TabBarClick::Tab(i);
            }
        }

        TabBarClick::None
    }
}

impl Renderer {
    fn tab_bar_layout(&self, tabs: &[TabInfo], window_width: f32) -> TabBarLayout {
        let anchor = match self.tab_bar_keyboard_focus {
            Some(TabBarClick::Tab(index) | TabBarClick::CloseTab(index)) => index,
            _ => tabs.iter().position(|tab| tab.is_active).unwrap_or(0),
        };
        tab_bar_layout(tabs, window_width, self.tab_bar_height(), anchor)
    }

    fn vertical_tab_bar_layout(
        &self,
        tabs: &[TabInfo],
        window_height: f32,
    ) -> VerticalTabBarLayout {
        let anchor = match self.tab_bar_keyboard_focus {
            Some(TabBarClick::Tab(index) | TabBarClick::CloseTab(index)) => index,
            _ => tabs.iter().position(|tab| tab.is_active).unwrap_or(0),
        };
        vertical_tab_bar_layout(tabs, window_height, self.tab_bar_height(), anchor)
    }
}

fn tab_animation_multiplier(tab: &TabInfo) -> f32 {
    if tab.close_progress > 0.0 {
        let eased = 1.0 - (1.0 - tab.close_progress).powi(3);
        1.0 - eased
    } else {
        1.0 - (1.0 - tab.slide_progress).powi(3)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tabs(count: usize, active: usize) -> Vec<TabInfo> {
        (0..count)
            .map(|index| TabInfo {
                title: format!("Tab {}", index + 1),
                pinned: false,
                broadcast_armed: false,
                broadcast_active: false,
                read_only: false,
                is_active: index == active,
                has_activity: false,
                slide_progress: 1.0,
                close_progress: 0.0,
                tab_type: TabType::Shell,
                unreviewed_diff_count: 0,
            })
            .collect()
    }

    #[test]
    fn crowded_tab_bar_keeps_active_tab_inside_bounded_viewport() {
        let tabs = tabs(12, 10);
        let layout = tab_bar_layout(&tabs, 640.0, 32.0, 10);

        assert!(layout.previous_x.is_some());
        assert!(layout.tabs.iter().any(|slot| slot.index == 10));
        assert!(layout.tabs.iter().all(|slot| slot.x >= 32.0));
        assert!(layout.tabs.iter().all(|slot| slot.x + slot.width <= 480.0));
        assert_eq!(layout.add_x, 512.0);
    }

    #[test]
    fn crowded_tab_bar_reveals_hidden_tabs_from_each_end() {
        let tabs = tabs(12, 0);
        let first = tab_bar_layout(&tabs, 640.0, 32.0, 0);
        assert!(first.previous_x.is_none());
        assert!(first.next_x.is_some());
        assert_eq!(first.tabs.first().map(|slot| slot.index), Some(0));

        let last = tab_bar_layout(&tabs, 640.0, 32.0, 11);
        assert!(last.previous_x.is_some());
        assert!(last.next_x.is_none());
        assert_eq!(last.tabs.last().map(|slot| slot.index), Some(11));
    }

    #[test]
    fn ordinary_tab_bar_preserves_full_strip_without_overflow_controls() {
        let tabs = tabs(3, 1);
        let layout = tab_bar_layout(&tabs, 640.0, 32.0, 1);

        assert_eq!(layout.tabs.len(), 3);
        assert!(layout.previous_x.is_none());
        assert!(layout.next_x.is_none());
        assert!(layout.tabs.iter().all(|slot| slot.width > MIN_TAB_WIDTH));
        assert!(layout.add_x <= 512.0);
    }

    #[test]
    fn vertical_tab_bar_keeps_active_tab_visible_and_reserves_controls() {
        let tabs = tabs(20, 17);
        let layout = vertical_tab_bar_layout(&tabs, 600.0, 32.0, 17);

        assert!(layout.previous_y.is_some());
        assert!(layout.tabs.iter().any(|slot| slot.index == 17));
        assert!(layout.tabs.iter().all(|slot| slot.y >= 64.0));
        assert!(layout.tabs.iter().all(|slot| slot.y + slot.height <= 536.0));
        assert_eq!(layout.add_y, 568.0);
    }

    #[test]
    fn vertical_tab_width_preserves_a_minimum_terminal_viewport() {
        assert_eq!(vertical_tab_bar_width(800.0, 1.0), 220.0);
        assert_eq!(vertical_tab_bar_width(320.0, 1.0), 160.0);
        assert_eq!(vertical_tab_bar_width(1_600.0, 2.0), 440.0);
    }

    #[test]
    fn vertical_tab_hit_testing_uses_the_rendered_slots_and_controls() {
        let tabs = tabs(20, 17);
        let layout = vertical_tab_bar_layout(&tabs, 600.0, 32.0, 17);
        let active = layout
            .tabs
            .iter()
            .find(|slot| slot.index == 17)
            .expect("active slot");
        assert_eq!(
            vertical_tab_bar_hit_test(&layout, &tabs, 20.0, active.y + 5.0, 220.0, 32.0, 18.0,),
            TabBarClick::Tab(17)
        );
        assert_eq!(
            vertical_tab_bar_hit_test(
                &layout,
                &tabs,
                205.0,
                active.y + active.height / 2.0,
                220.0,
                32.0,
                18.0,
            ),
            TabBarClick::CloseTab(17)
        );
        assert_eq!(
            vertical_tab_bar_hit_test(
                &layout,
                &tabs,
                20.0,
                layout.previous_y.unwrap() + 5.0,
                220.0,
                32.0,
                18.0,
            ),
            TabBarClick::PreviousTab
        );
        assert_eq!(
            vertical_tab_bar_hit_test(&layout, &tabs, 20.0, layout.add_y + 5.0, 220.0, 32.0, 18.0,),
            TabBarClick::NewTab
        );
    }
}
