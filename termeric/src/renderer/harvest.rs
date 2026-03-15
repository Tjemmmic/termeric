//! Harvest mode rendering — growth indicators, root cellar, sprout menu
//!
//! Three `pub(super)` functions called from `build_render_data()`:
//! - `build_harvest_indicators()` — draw growth phase characters in the left gutter
//! - `build_root_cellar()` — slide-in command history panel
//! - `build_sprout_menu()` — context menu for command actions

use super::{Renderer, adjust_alpha};
use super::draw::DrawCtx;
use crate::renderer::types::{IndicatorRenderInfo, CellarRenderInfo, SproutMenuRenderInfo};

impl Renderer {
    /// Draw growth indicators in the left gutter next to terminal rows.
    ///
    /// Each indicator is a single character (seedling, sprout, flower, etc.)
    /// positioned just left of the grid, colored by growth phase.
    pub(super) fn build_harvest_indicators(
        &mut self,
        indicators: &[IndicatorRenderInfo],
        content_y: f32,
    ) {
        let cell_width = self.cell_width;
        let cell_height = self.cell_height;
        // Position indicators in the left padding, one cell-width left of the grid
        let gutter_x = (self.padding - cell_width).max(0.0);

        for ind in indicators {
            let y = content_y + ind.row as f32 * cell_height;
            let color = [
                ind.color[0],
                ind.color[1],
                ind.color[2],
                ind.color[3] * ind.opacity,
            ];

            // Scale effect: slightly enlarge during pulse
            // We can't truly scale text in the pipeline, but we can adjust
            // position to center a "scaled" glyph
            let offset = (1.0 - ind.scale) * cell_width * 0.5;
            let x = gutter_x + offset;

            self.text_pipeline.push_char(ind.character, x, y, color);
        }
    }

    /// Draw the root cellar panel — a slide-in command history from the left.
    ///
    /// Shows recent commands with health indicators (exit codes) and a
    /// success rate summary at the top.
    pub(super) fn build_root_cellar(
        &mut self,
        cellar: &CellarRenderInfo,
        _window_width: f32,
        window_height: f32,
    ) {
        if cellar.animation_progress < 0.001 {
            return;
        }

        let cell_width = self.cell_width;
        let cell_height = self.cell_height;
        let anim = cellar.animation_progress;

        // Panel dimensions
        let panel_width = (cell_width * 35.0).min(350.0);
        let panel_height = window_height * 0.6;
        let panel_y = (window_height - panel_height) / 2.0;

        // Ease-out cubic for slide
        let eased = 1.0 - (1.0 - anim).powi(3);
        let panel_x = -panel_width + panel_width * eased;

        // Use DrawCtx for card and text rendering
        let bg = adjust_alpha(self.ui.settings_bg, 0.95 * anim);
        let border = adjust_alpha(self.ui.tab_border, 0.5 * anim);

        {
            let mut ctx = DrawCtx {
                rect: &mut self.rect_pipeline,
                text: &mut self.text_pipeline,
                cell_width,
                cell_height,
                ui: &self.ui,
            };

            // Scrim (dim area behind panel)
            let scrim_width = panel_x + panel_width;
            if scrim_width > 0.0 {
                ctx.draw_scrim(scrim_width, window_height, ctx.ui.scrim, anim * 0.5);
            }

            // Panel background
            ctx.draw_card(panel_x, panel_y, panel_width, panel_height, bg, border);

            // Header
            let header_y = panel_y + cell_height * 0.5;
            let text_color = adjust_alpha(ctx.ui.settings_header, anim);
            let dim_color = adjust_alpha(ctx.ui.settings_text_dim, anim);
            ctx.draw_text("Root Cellar", panel_x + cell_width, header_y, text_color);

            // Success rate badge
            let rate_text = format!("{:.0}% healthy", cellar.success_rate * 100.0);
            let rate_color = if cellar.success_rate > 0.8 {
                adjust_alpha([0.3, 0.8, 0.4, 1.0], anim) // green
            } else if cellar.success_rate > 0.5 {
                adjust_alpha([0.8, 0.7, 0.2, 1.0], anim) // yellow
            } else {
                adjust_alpha([0.8, 0.3, 0.2, 1.0], anim) // red
            };
            ctx.draw_text(&rate_text, panel_x + cell_width, header_y + cell_height, rate_color);

            // Separator
            let sep_y = header_y + cell_height * 2.5;
            ctx.draw_separator(panel_x + cell_width * 0.5, sep_y, panel_width - cell_width, border);

            // Entry list
            let list_top = sep_y + cell_height * 0.5;
            let max_visible = ((panel_height - (list_top - panel_y) - cell_height) / cell_height).floor() as usize;

            for (i, entry) in cellar.entries.iter().skip(cellar.scroll).take(max_visible).enumerate() {
                let entry_y = list_top + i as f32 * cell_height;
                let entry_color = adjust_alpha(entry.health_color, entry.opacity * anim);

                // Health indicator character
                let health_char = match entry.health {
                    0 => '\u{25CF}', // filled circle (healthy)
                    1 => '\u{25CB}', // empty circle (warning)
                    _ => '\u{2716}', // heavy X (sick)
                };
                ctx.text.push_char(health_char, panel_x + cell_width, entry_y, entry_color);

                // Exit code
                let code_text = match entry.exit_code {
                    Some(0) => "ok".to_string(),
                    Some(code) => format!("E{}", code),
                    None => "...".to_string(),
                };
                ctx.draw_text(&code_text, panel_x + cell_width * 3.0, entry_y, dim_color);

                // Row indicator
                let row_text = format!("L{}", entry.row);
                ctx.draw_text(&row_text, panel_x + cell_width * 8.0, entry_y, dim_color);
            }

            // Scrollbar
            let scrollbar_x = panel_x + panel_width - cell_width;
            ctx.draw_scrollbar(
                scrollbar_x, list_top,
                panel_height - (list_top - panel_y) - cell_height,
                cellar.entries.len(),
                max_visible,
                cellar.scroll,
                anim,
                4.0,
            );
        }
    }

    /// Draw the sprout context menu at a click position.
    ///
    /// Shows available actions for a command region (copy, rerun, explain, etc.)
    pub(super) fn build_sprout_menu(&mut self, menu: &SproutMenuRenderInfo) {
        if menu.opacity < 0.001 {
            return;
        }

        let cell_width = self.cell_width;
        let cell_height = self.cell_height;
        let anim = menu.opacity;

        let bg = adjust_alpha(self.ui.palette_bg, 0.95 * anim);
        let border = adjust_alpha(self.ui.palette_border, 0.6 * anim);
        let text_color = adjust_alpha(self.ui.palette_text, anim);
        let highlight_bg = adjust_alpha(self.ui.palette_accent, 0.15 * anim);
        let icon_color = adjust_alpha(self.ui.palette_accent, 0.8 * anim);

        {
            let mut ctx = DrawCtx {
                rect: &mut self.rect_pipeline,
                text: &mut self.text_pipeline,
                cell_width,
                cell_height,
                ui: &self.ui,
            };

            // Card shadow + background
            ctx.draw_card_shadow(menu.x, menu.y, menu.width, menu.height, 0.2, 2);
            ctx.draw_card(menu.x, menu.y, menu.width, menu.height, bg, border);

            // Action items
            let item_height = cell_height * 1.4;
            let padding = cell_width * 0.5;

            for (i, (name, icon)) in menu.actions.iter().enumerate() {
                let item_y = menu.y + padding + i as f32 * item_height;

                // Highlight selected item
                if i == menu.selected {
                    ctx.rect.push_rounded_rect(
                        menu.x + 2.0, item_y,
                        menu.width - 4.0, item_height,
                        highlight_bg,
                        4.0,
                    );
                }

                // Icon
                ctx.text.push_char(*icon, menu.x + padding, item_y + (item_height - cell_height) / 2.0, icon_color);

                // Action name
                ctx.draw_text(name, menu.x + padding + cell_width * 2.0, item_y + (item_height - cell_height) / 2.0, text_color);
            }
        }
    }
}
