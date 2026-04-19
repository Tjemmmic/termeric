//! Tab bar rendering and hit-testing

use super::types::*;
use super::{Renderer, adjust_alpha};
use crate::tabs::{TabInfo, TabType};

impl Renderer {
    pub(super) fn build_tab_bar(
        &mut self,
        tabs: &[TabInfo],
        window_width: f32,
        tab_bar_height: f32,
    ) {
        let tab_bar_bg = self.ui.tab_bar_bg;
        self.rect_pipeline
            .push_rect(0.0, 0.0, window_width, tab_bar_height, tab_bar_bg);

        let active_tab_bg = self.ui.tab_active;
        let inactive_tab_bg = self.ui.tab_inactive;
        let activity_indicator = self.ui.tab_activity;
        let active_text_color = self.ui.tab_text_active;
        let inactive_text_color = self.ui.tab_text_inactive;

        // Calculate tab width - minimum 80px, maximum 200px, or divide space evenly
        // Reserve space for the session dropdown and gear icon on the right
        // The + button floats after the last tab, not in the reserved zone
        let gear_button_width = tab_bar_height;
        let session_button_width = tab_bar_height;
        let commands_button_width = tab_bar_height;
        let add_button_width = tab_bar_height;
        let right_buttons_width = gear_button_width + session_button_width + commands_button_width;
        let available_tab_width = window_width - right_buttons_width - add_button_width;
        let tab_count = tabs.len().max(1);
        let full_tab_width = ((available_tab_width / tab_count as f32).min(200.0)).max(80.0);
        let tab_padding = 8.0;
        let tab_height = tab_bar_height - 4.0; // Leave some margin
        let show_close_buttons = true;
        // Reserve space for close button inside tab
        let close_btn_size = self.cell_height;
        let close_btn_margin = 4.0;

        // Track cumulative x position (accounting for animated widths)
        let mut current_x = 0.0;

        for (i, tab) in tabs.iter().enumerate() {
            // Calculate animation multiplier based on slide-in and close animations
            let anim_multiplier = if tab.close_progress > 0.0 {
                // Closing: shrink from full to 0 with ease-out cubic so the
                // motion matches the rest of the UI (settles instead of
                // accelerating off-screen).
                let t = tab.close_progress;
                let eased = 1.0 - (1.0 - t).powi(3);
                1.0 - eased
            } else {
                // Opening: grow from 0 to full with ease-out cubic
                let t = tab.slide_progress;
                1.0 - (1.0 - t).powi(3)
            };

            // Animated tab width
            let tab_width = full_tab_width * anim_multiplier;

            let x = current_x;
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
                    let title = if tab.title.len() > 15 {
                        format!("{}...", &tab.title[..12])
                    } else {
                        tab.title.clone()
                    };

                    // Add tab number prefix
                    let label = format!("{}: {}", i + 1, title);

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

            current_x += tab_width;
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

        // ── Add (+) tab button — positioned right after the last tab ──
        let add_size = tab_bar_height;
        let max_add_x = window_width - right_buttons_width - add_size;
        let add_x = current_x.min(max_add_x);
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
            for i in 0..3 {
                let y = top_y + i as f32 * gap;
                let w = widths[i];
                self.rect_pipeline
                    .push_line(cx - w, y, cx + w, y, line_w, color);
            }
        }
    }

    pub fn tab_bar_hit_test(
        &self,
        x: f32,
        y: f32,
        tabs: &[TabInfo],
        window_width: f32,
    ) -> TabBarClick {
        let tab_bar_height = self.tab_bar_height();

        // Only handle clicks within the tab bar vertical bounds
        if y >= tab_bar_height {
            return TabBarClick::None;
        }

        let gear_button_width = tab_bar_height;
        let session_button_width = tab_bar_height;
        let commands_button_width = tab_bar_height;
        let add_button_width = tab_bar_height;
        let right_buttons_width = gear_button_width + session_button_width + commands_button_width;

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

        // ── Compute tab layout to find where + button sits ──
        let available_tab_width = window_width - right_buttons_width - add_button_width;
        let tab_count = tabs.len().max(1);
        let full_tab_width = ((available_tab_width / tab_count as f32).min(200.0)).max(80.0);
        let tab_height = tab_bar_height - 4.0;
        let show_close_buttons = true;
        let close_btn_size = self.cell_height;
        let close_btn_margin = 4.0;

        // Walk through tabs to find cumulative end position
        let mut current_x = 0.0_f32;
        for tab in tabs.iter() {
            let anim_multiplier = if tab.close_progress > 0.0 {
                let t = tab.close_progress;
                1.0 - t * t
            } else {
                let t = tab.slide_progress;
                1.0 - (1.0 - t) * (1.0 - t)
            };
            current_x += full_tab_width * anim_multiplier;
        }

        // ── Add (+) button — right after last tab ──
        let max_add_x = window_width - right_buttons_width - add_button_width;
        let add_x = current_x.min(max_add_x);
        if x >= add_x && x < add_x + add_button_width {
            return TabBarClick::NewTab;
        }

        let mut current_x = 0.0f32;

        for (i, tab) in tabs.iter().enumerate() {
            let anim_multiplier = if tab.close_progress > 0.0 {
                let t = tab.close_progress;
                1.0 - t * t
            } else {
                let t = tab.slide_progress;
                1.0 - (1.0 - t) * (1.0 - t)
            };
            let tab_width = full_tab_width * anim_multiplier;

            if x >= current_x && x < current_x + tab_width {
                // Click is within this tab — check if it hit the close button
                if show_close_buttons && tab_width > 2.0 && anim_multiplier > 0.3 {
                    let tab_y = 2.0;
                    let close_x = current_x + tab_width - close_btn_size - close_btn_margin - 2.0;
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

            current_x += tab_width;
        }

        TabBarClick::None
    }
}
